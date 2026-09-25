//! Rule-gated operator notifications.
//!
//! `aida notify check` reads `.aida/events.jsonl`, evaluates the configured
//! `[notify.rules]`, and sends one plain-text message per due rule through the
//! single configured shell command. The message is written on stdin; only the
//! command template's `{title}` / `{rule}` placeholders are substituted.
//!
//! trace:STORY-1029 | ai:codex

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveTime, Timelike, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::NotifyCommand;
use crate::events::{self, Event, EventKind};

const STATE_FILE: &str = "notify-highwater";
const LOG_FILE: &str = "notify.log";

#[derive(Debug, Clone, PartialEq, Eq)]
struct NotifyConfig {
    command: String,
    min_interval: Duration,
    quiet_hours: Option<QuietHours>,
    rules: NotifyRules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NotifyRules {
    idle_with_work: bool,
    shelve: bool,
    escalation: bool,
}

impl Default for NotifyRules {
    fn default() -> Self {
        Self {
            idle_with_work: true,
            shelve: true,
            escalation: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuietHours {
    start_minute: u16,
    end_minute: u16,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct NotifyState {
    #[serde(default)]
    highwater: u64,
    #[serde(default)]
    rules: BTreeMap<String, RuleState>,
    #[serde(default)]
    pending: Vec<PendingNotification>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RuleState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_fire: Option<DateTime<Utc>>,
    #[serde(default)]
    suppressed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingNotification {
    rule: String,
    title: String,
    message: String,
    #[serde(default)]
    queued_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuleFire {
    rule: &'static str,
    title: String,
    message: String,
}

pub(crate) fn handle_notify_command(project_root: &Path, cmd: &NotifyCommand) -> Result<()> {
    match cmd {
        NotifyCommand::Check => {
            let outcome = run_check(project_root, true)?;
            if outcome.configured {
                println!(
                    "notify: {} sent, {} suppressed, {} pending",
                    outcome.sent, outcome.suppressed, outcome.pending
                );
            }
            Ok(())
        }
        NotifyCommand::Test => notify_test(project_root),
        NotifyCommand::Status => notify_status(project_root),
    }
}

pub(crate) fn passive_check(project_root: &Path) -> Result<()> {
    run_check(project_root, false).map(|_| ())
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DirectNotifyOutcome {
    pub(crate) configured: bool,
    pub(crate) sent: usize,
    pub(crate) suppressed: usize,
    pub(crate) pending: usize,
}

/// What [`send_direct`] actually did with one message.
// trace:TASK-1492 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectDelivery {
    /// `[notify] command` is unset; nothing was sent.
    NotConfigured,
    /// The command ran and succeeded.
    Sent,
    /// Quiet hours: queued, and sent by the next `aida notify check` after
    /// they end.
    Deferred,
    /// The rule fired within its `min_interval`; dropped, not queued.
    Suppressed,
}

impl DirectNotifyOutcome {
    /// The single-message outcome.
    // trace:TASK-1492 | ai:claude
    pub(crate) fn delivery(&self) -> DirectDelivery {
        if !self.configured {
            DirectDelivery::NotConfigured
        } else if self.sent > 0 {
            DirectDelivery::Sent
        } else if self.pending > 0 {
            DirectDelivery::Deferred
        } else {
            DirectDelivery::Suppressed
        }
    }
}

#[derive(Debug, Clone, Default)]
struct CheckOutcome {
    configured: bool,
    sent: usize,
    suppressed: usize,
    pending: usize,
}

pub(crate) fn send_direct(
    project_root: &Path,
    rule: &str,
    title: &str,
    message: &str,
) -> Result<DirectNotifyOutcome> {
    send_direct_bounded(project_root, rule, title, message, None)
}

/// [`send_direct`] with a bound on how long the notify command may run: a
/// caller with a deadline (the night-shift tick) passes one, and a command
/// still running at the bound is killed and reported as a failure.
// trace:TASK-1492 | ai:claude
pub(crate) fn send_direct_bounded(
    project_root: &Path,
    rule: &str,
    title: &str,
    message: &str,
    timeout: Option<Duration>,
) -> Result<DirectNotifyOutcome> {
    let Some(config) = NotifyConfig::load(project_root)? else {
        return Ok(DirectNotifyOutcome::default());
    };
    let mut state = load_state(project_root).unwrap_or_default();
    let mut outcome = DirectNotifyOutcome {
        configured: true,
        ..DirectNotifyOutcome::default()
    };

    let now = Utc::now();
    if should_suppress(&mut state, rule, now, config.min_interval) {
        outcome.suppressed += 1;
        save_state(project_root, &state)?;
        return Ok(outcome);
    }

    if config.is_quiet(Local::now().time()) {
        state.pending.push(PendingNotification {
            rule: rule.to_string(),
            title: title.to_string(),
            message: message.to_string(),
            queued_at: Some(now),
        });
        outcome.pending += 1;
        save_state(project_root, &state)?;
        return Ok(outcome);
    }

    match run_command_bounded(project_root, &config, rule, title, message, timeout) {
        Ok(()) => {
            mark_sent(&mut state, rule, now);
            outcome.sent += 1;
        }
        Err(err) => {
            log_failure(project_root, err);
            save_state(project_root, &state)?;
            anyhow::bail!("notify command failed; see .aida/{LOG_FILE}");
        }
    }
    save_state(project_root, &state)?;
    Ok(outcome)
}

fn run_check(project_root: &Path, report_failures: bool) -> Result<CheckOutcome> {
    let Some(config) = NotifyConfig::load(project_root)? else {
        return Ok(CheckOutcome::default());
    };
    let mut state = load_state(project_root).unwrap_or_default();
    let mut outcome = CheckOutcome {
        configured: true,
        ..CheckOutcome::default()
    };

    let now = Utc::now();
    let local_now = Local::now().time();
    if config.is_quiet(local_now) {
        let (fires, highwater) = collect_fires(project_root, state.highwater, &config.rules)?;
        state.highwater = highwater;
        for fire in fires {
            if should_suppress(&mut state, fire.rule, now, config.min_interval) {
                outcome.suppressed += 1;
            } else {
                state.pending.push(PendingNotification {
                    rule: fire.rule.to_string(),
                    title: fire.title,
                    message: fire.message,
                    queued_at: Some(now),
                });
                outcome.pending += 1;
            }
        }
        save_state(project_root, &state)?;
        return Ok(outcome);
    }

    let pending = std::mem::take(&mut state.pending);
    for note in pending {
        let sent = run_command(
            project_root,
            &config,
            &note.rule,
            &note.title,
            &note.message,
        );
        if sent.is_ok() {
            mark_sent(&mut state, &note.rule, now);
            outcome.sent += 1;
        } else {
            state.pending.push(note);
            outcome.pending += 1;
            log_failure(project_root, sent.unwrap_err());
        }
    }

    let (fires, highwater) = collect_fires(project_root, state.highwater, &config.rules)?;
    state.highwater = highwater;
    for fire in fires {
        if should_suppress(&mut state, fire.rule, now, config.min_interval) {
            outcome.suppressed += 1;
            continue;
        }
        match run_command(project_root, &config, fire.rule, &fire.title, &fire.message) {
            Ok(()) => {
                mark_sent(&mut state, fire.rule, now);
                outcome.sent += 1;
            }
            Err(err) => {
                log_failure(project_root, err);
                if report_failures {
                    eprintln!("notify: command failed; see .aida/{LOG_FILE}");
                }
            }
        }
    }
    save_state(project_root, &state)?;
    Ok(outcome)
}

fn notify_test(project_root: &Path) -> Result<()> {
    let Some(config) = NotifyConfig::load(project_root)? else {
        println!("notify: off ([notify].command is unset)");
        return Ok(());
    };
    let message = format!(
        "aida notify test\nrule: test\nproject: {}\nlocal time: {}\n",
        project_root.display(),
        Local::now().format("%Y-%m-%d %H:%M:%S %Z")
    );
    let status = run_command(project_root, &config, "test", "aida notify test", &message);
    match status {
        Ok(()) => println!("notify test: exit 0"),
        Err(err) => {
            log_failure(project_root, err);
            println!("notify test: failed; see .aida/{LOG_FILE}");
        }
    }
    Ok(())
}

fn notify_status(project_root: &Path) -> Result<()> {
    let Some(config) = NotifyConfig::load(project_root)? else {
        println!("notify: off ([notify].command is unset)");
        return Ok(());
    };
    let state = load_state(project_root).unwrap_or_default();
    println!("notify: on");
    println!("command: {}", config.command);
    println!("min_interval: {}s", config.min_interval.as_secs());
    if let Some(q) = config.quiet_hours {
        println!(
            "quiet_hours: {:02}:{:02}-{:02}:{:02}",
            q.start_minute / 60,
            q.start_minute % 60,
            q.end_minute / 60,
            q.end_minute % 60
        );
    }
    for rule in ["idle_with_work", "shelve", "escalation"] {
        let enabled = match rule {
            "idle_with_work" => config.rules.idle_with_work,
            "shelve" => config.rules.shelve,
            "escalation" => config.rules.escalation,
            _ => false,
        };
        let rs = state.rules.get(rule).cloned().unwrap_or_default();
        let last = rs
            .last_fire
            .map(|ts| {
                ts.with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S %Z")
                    .to_string()
            })
            .unwrap_or_else(|| "never".to_string());
        println!(
            "rule {rule}: {} · last_fire: {last} · suppressed: {}",
            if enabled { "on" } else { "off" },
            rs.suppressed
        );
    }
    if !state.pending.is_empty() {
        println!("pending: {}", state.pending.len());
    }
    Ok(())
}

impl NotifyConfig {
    fn load(project_root: &Path) -> Result<Option<Self>> {
        let path = project_root.join(".aida").join("config.toml");
        let Ok(body) = std::fs::read_to_string(&path) else {
            return Ok(None);
        };
        Self::from_toml(&body)
    }

    fn from_toml(body: &str) -> Result<Option<Self>> {
        let value: toml::Value = toml::from_str(body)?;
        let Some(notify) = value.get("notify").and_then(|v| v.as_table()) else {
            return Ok(None);
        };
        let Some(command) = notify
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
        else {
            return Ok(None);
        };
        let min_interval = notify
            .get("min_interval")
            .and_then(|v| v.as_str())
            .and_then(parse_duration)
            .unwrap_or_else(|| Duration::from_secs(30 * 60));
        let quiet_hours = notify
            .get("quiet_hours")
            .and_then(|v| v.as_str())
            .and_then(QuietHours::parse);
        let mut rules = NotifyRules::default();
        if let Some(table) = notify.get("rules").and_then(|v| v.as_table()) {
            if let Some(v) = table.get("idle_with_work").and_then(|v| v.as_bool()) {
                rules.idle_with_work = v;
            }
            if let Some(v) = table.get("shelve").and_then(|v| v.as_bool()) {
                rules.shelve = v;
            }
            if let Some(v) = table.get("escalation").and_then(|v| v.as_bool()) {
                rules.escalation = v;
            }
        }
        Ok(Some(Self {
            command,
            min_interval,
            quiet_hours,
            rules,
        }))
    }

    fn is_quiet(&self, local_time: NaiveTime) -> bool {
        self.quiet_hours
            .map(|q| q.contains(local_time))
            .unwrap_or(false)
    }
}

impl QuietHours {
    fn parse(raw: &str) -> Option<Self> {
        let (start, end) = raw.split_once('-')?;
        Some(Self {
            start_minute: parse_time_minutes(start)?,
            end_minute: parse_time_minutes(end)?,
        })
    }

    fn contains(&self, time: NaiveTime) -> bool {
        let minute = (time.hour() * 60 + time.minute()) as u16;
        if self.start_minute <= self.end_minute {
            minute >= self.start_minute && minute < self.end_minute
        } else {
            minute >= self.start_minute || minute < self.end_minute
        }
    }
}

fn collect_fires(
    project_root: &Path,
    highwater: u64,
    rules: &NotifyRules,
) -> Result<(Vec<RuleFire>, u64)> {
    let path = events::events_path(project_root);
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(_) => return Ok((Vec::new(), highwater)),
    };
    let start = highwater.min(body.len() as u64) as usize;
    let slice = &body[start..];
    let highwater_next = body.len() as u64;
    Ok((evaluate_rules(slice, rules), highwater_next))
}

fn evaluate_rules(body: &str, rules: &NotifyRules) -> Vec<RuleFire> {
    let mut shelved = Vec::new();
    let mut escalated = Vec::new();
    let mut drained_idle = Vec::new();
    let mut run_uuid = String::new();

    for line in body.lines() {
        let Ok(ev) = serde_json::from_str::<Event>(line.trim()) else {
            continue;
        };
        if !ev.run_uuid.is_empty() {
            run_uuid = ev.run_uuid.clone();
        }
        match ev.kind {
            EventKind::SpecShelved { .. } | EventKind::PuntFiled { .. } => {
                if let Some(spec) = ev.spec.clone().or_else(|| punt_spec(&ev.kind)) {
                    shelved.push(spec);
                }
            }
            EventKind::AdvisorEscalated { reason } => {
                escalated.push(format!(
                    "{}{}",
                    ev.spec.unwrap_or_else(|| "drain".into()),
                    if reason.is_empty() {
                        String::new()
                    } else {
                        format!(" ({reason})")
                    }
                ));
            }
            EventKind::QueueDrained {
                shipped,
                shelved,
                excluded_from_batch,
                ineligible,
            } => {
                // TASK-1297: a batch drain excluding approved, role-routed
                // work is alarm-worthy even when it shipped fine — that is
                // exactly the "3 open members re-driven for hours while 16
                // sat outside the filter" incident shape.
                // trace:TASK-1297 | ai:claude
                if shipped == 0
                    || shelved > 0
                    || excluded_from_batch.is_some_and(|count| count > 0)
                    || !ineligible.is_empty()
                {
                    let mut msg = format!("drain shipped {shipped}, shelved {shelved}");
                    if !ineligible.is_empty() {
                        msg.push_str(&format!(
                            "; {} member{} remain ineligible: {}",
                            ineligible.len(),
                            if ineligible.len() == 1 { "" } else { "s" },
                            ineligible
                                .iter()
                                .map(|member| format!("{} ({})", member.spec, member.reason))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    if let Some(excluded_from_batch) =
                        excluded_from_batch.filter(|count| *count > 0)
                    {
                        msg.push_str(&format!(
                            "; {excluded_from_batch} other approved routed spec{} excluded by the batch filter",
                            if excluded_from_batch == 1 { "" } else { "s" }
                        ));
                    }
                    drained_idle.push(msg);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    if rules.shelve && !shelved.is_empty() {
        out.push(rule_fire(
            "shelve",
            "aida: shelved work",
            &shelved,
            run_uuid.as_str(),
            "aida awaiting",
        ));
    }
    if rules.escalation && !escalated.is_empty() {
        out.push(rule_fire(
            "escalation",
            "aida: advisor escalation",
            &escalated,
            run_uuid.as_str(),
            "aida awaiting",
        ));
    }
    if rules.idle_with_work
        && (!drained_idle.is_empty() || !shelved.is_empty() || !escalated.is_empty())
    {
        let mut items = drained_idle;
        items.extend(shelved.iter().map(|s| format!("open work: {s}")));
        items.extend(escalated.iter().map(|s| format!("escalation: {s}")));
        out.push(rule_fire(
            "idle_with_work",
            "aida: idle with open work",
            &items,
            run_uuid.as_str(),
            "aida awaiting",
        ));
    }
    out
}

fn punt_spec(kind: &EventKind) -> Option<String> {
    match kind {
        EventKind::PuntFiled { spec } => Some(spec.clone()),
        _ => None,
    }
}

fn rule_fire(
    rule: &'static str,
    title: &str,
    items: &[String],
    run_uuid: &str,
    recovery: &str,
) -> RuleFire {
    let mut message = String::new();
    message.push_str(&format!("rule: {rule}\n"));
    message.push_str(&format!("title: {title}\n"));
    if !run_uuid.is_empty() {
        message.push_str(&format!("drain id: {run_uuid}\n"));
    }
    message.push_str(&format!(
        "local time: {}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S %Z")
    ));
    message.push_str("items:\n");
    for item in items {
        message.push_str(&format!("- {item}\n"));
    }
    message.push_str(&format!("recovery: {recovery}\n"));
    RuleFire {
        rule,
        title: title.to_string(),
        message,
    }
}

fn should_suppress(
    state: &mut NotifyState,
    rule: &str,
    now: DateTime<Utc>,
    min_interval: Duration,
) -> bool {
    let rs = state.rules.entry(rule.to_string()).or_default();
    let suppress = rs
        .last_fire
        .and_then(|last| (now - last).to_std().ok())
        .map(|elapsed| elapsed < min_interval)
        .unwrap_or(false);
    if suppress {
        rs.suppressed += 1;
    }
    suppress
}

fn mark_sent(state: &mut NotifyState, rule: &str, now: DateTime<Utc>) {
    state.rules.entry(rule.to_string()).or_default().last_fire = Some(now);
}

/// Quote a substitution value for the platform shell. Unix: single-quote
/// wrapping with `'\''` for embedded quotes. Windows `cmd`: no reliable quote
/// exists, so strip the cmd metacharacters instead — these are display
/// strings, not data that must round-trip.
fn shell_quote(value: &str) -> String {
    #[cfg(unix)]
    {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
    #[cfg(windows)]
    {
        let cleaned: String = value
            .chars()
            .filter(|c| !matches!(c, '&' | '|' | '<' | '>' | '^' | '%' | '"' | '`'))
            .collect();
        format!("\"{cleaned}\"")
    }
}

fn run_command(
    project_root: &Path,
    config: &NotifyConfig,
    rule: &str,
    title: &str,
    message: &str,
) -> Result<()> {
    run_command_bounded(project_root, config, rule, title, message, None)
}

/// Run the notify command; with `timeout`, a command still running at the
/// bound is killed and the send fails, so a hung command cannot hold its
/// caller past a deadline.
// trace:TASK-1492 | ai:claude
fn run_command_bounded(
    project_root: &Path,
    config: &NotifyConfig,
    rule: &str,
    title: &str,
    message: &str,
    timeout: Option<Duration>,
) -> Result<()> {
    // Substituted values are spliced into a shell command line, and spec
    // titles carry shell-active characters (backticks, quotes, $). Quote them
    // so title content is data, never code.
    let command = config
        .command
        .replace("{title}", &shell_quote(title))
        .replace("{rule}", &shell_quote(rule));
    #[cfg(unix)]
    let mut child = {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&command)
            .current_dir(project_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // A bounded run gets its own process group, so a timeout can end
        // the whole command (a compound command or a pipeline forks
        // grandchildren that a signal to `sh` alone would orphan).
        // trace:TASK-1492 | ai:claude
        if timeout.is_some() {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        cmd.spawn()
            .with_context(|| format!("spawn notify command `{command}`"))?
    };
    #[cfg(windows)]
    let mut child = Command::new("cmd")
        .arg("/C")
        .arg(&command)
        .current_dir(project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn notify command `{command}`"))?;
    let Some(timeout) = timeout else {
        if let Some(mut stdin) = child.stdin.take() {
            write_message(&mut stdin, message)?;
        }
        let output = child.wait_with_output()?;
        if output.status.success() {
            return Ok(());
        }
        anyhow::bail!(
            "notify command exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
    };
    // The message is written on a thread: a hung command that never reads
    // stdin must not hold the caller past the bound once the message is
    // larger than the pipe buffer. The group kill closes the read end, so
    // the blocked write then fails with EPIPE and the thread ends.
    // trace:BUG-1623 | ai:claude
    let (stdin_tx, stdin_rx) = std::sync::mpsc::channel();
    if let Some(mut stdin) = child.stdin.take() {
        let body = message.to_string();
        std::thread::spawn(move || {
            let _ = stdin_tx.send(write_message(&mut stdin, &body));
        });
    }
    // stderr is drained on a thread so a chatty command cannot block on a
    // full pipe. The pipe may outlive the shell: a descendant that left the
    // group (`setsid`, a daemonizing helper) keeps it open everywhere, and
    // off Linux a background child of a command that exited on its own is
    // not group-killed either (only Linux kills the group after a normal
    // exit). So the thread is never joined blindly: its text is awaited for
    // a bounded moment and it is otherwise detached.
    // trace:TASK-1492 trace:BUG-1623 | ai:claude
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = std::io::Read::read_to_string(&mut stderr, &mut buf);
            let _ = tx.send(buf);
        })
    });
    let started = std::time::Instant::now();
    let polled = loop {
        match poll_exit(&mut child) {
            Ok(ChildPoll::Running) => {}
            other => break other.map(|p| (p, false)),
        }
        if started.elapsed() >= timeout {
            break Ok((ChildPoll::Running, true));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // An unreaped shell's pid is still the live group id: end every process
    // the command started (a timed-out one, or a background leftover), then
    // reap the shell. A poll error leaves the shell unreaped too, so it gets
    // the same cleanup before the error is returned, except ECHILD: the
    // shell is already gone (reaped elsewhere), its pid may have been reused,
    // and a group kill could hit an unrelated group. A shell already reaped
    // by the poll (the non-Linux fallback) is never group-killed either.
    // trace:BUG-1623 | ai:claude
    let (status, timed_out) = match polled {
        Ok((ChildPoll::Reaped(status), _)) => (status, false),
        Ok((_, timed_out)) => {
            kill_group(&mut child);
            (child.wait()?, timed_out)
        }
        Err(e) => {
            if !is_echild(&e) {
                kill_group(&mut child);
                let _ = child.wait();
            }
            return Err(e).context("poll notify command");
        }
    };
    let stderr = rx.recv_timeout(Duration::from_secs(1)).unwrap_or_default();
    if let Some(reader) = reader {
        if reader.is_finished() {
            let _ = reader.join();
        }
    }
    if timed_out {
        anyhow::bail!(
            "notify command `{command}` still running after {}s; killed",
            timeout.as_secs_f32()
        );
    }
    // A write still blocked here belongs to a stdin holder outside the group;
    // it is not waited for.
    if let Ok(Err(e)) = stdin_rx.recv_timeout(Duration::from_millis(100)) {
        return Err(e).context("write notify message");
    }
    if status.success() {
        return Ok(());
    }
    anyhow::bail!("notify command exited {status}: {}", stderr.trim())
}

/// Write the message to the command's stdin. A notify command is not
/// required to read stdin (`notify-send` takes everything as arguments); if
/// it exits first the write sees EPIPE, which is not a delivery failure.
// trace:BUG-1623 | ai:claude
fn write_message(stdin: &mut std::process::ChildStdin, message: &str) -> std::io::Result<()> {
    match stdin.write_all(message.as_bytes()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

/// Did the poll fail because the child no longer exists (already reaped)?
// trace:BUG-1623 | ai:claude
fn is_echild(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(libc::ECHILD)
    }
    #[cfg(not(unix))]
    {
        let _ = e;
        false
    }
}

/// One poll of the bounded command.
// trace:BUG-1623 | ai:claude
enum ChildPoll {
    Running,
    /// Exited but not reaped: its pid is still reserved as the group id.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    ExitedUnreaped,
    /// Exited and already reaped by the poll.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Reaped(std::process::ExitStatus),
}

/// Linux: checked WITHOUT reaping the child (`waitid` with `WNOWAIT`), so
/// its pid stays reserved as the process-group id until [`kill_group`] ran
/// and background leftovers of a finished command are ended too.
// trace:TASK-1492 trace:BUG-1623 | ai:claude
#[cfg(target_os = "linux")]
fn poll_exit(child: &mut std::process::Child) -> std::io::Result<ChildPoll> {
    // SAFETY: waitid only writes the zeroed siginfo we own.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: si_pid is valid for a waitid result; 0 means still running.
    Ok(if unsafe { info.si_pid() } != 0 {
        ChildPoll::ExitedUnreaped
    } else {
        ChildPoll::Running
    })
}

/// Elsewhere (macOS/BSD `waitid` with `WNOHANG | WNOWAIT` is not relied on,
/// and Windows has no process groups here): a plain `try_wait`, which reaps.
/// A timed-out command is still group-killed while unreaped; background
/// leftovers of a command that exited on its own are not ended.
// trace:BUG-1623 | ai:claude
#[cfg(not(target_os = "linux"))]
fn poll_exit(child: &mut std::process::Child) -> std::io::Result<ChildPoll> {
    Ok(match child.try_wait()? {
        Some(status) => ChildPoll::Reaped(status),
        None => ChildPoll::Running,
    })
}

/// SIGKILL the command's whole process group (its pgid is the unreaped
/// shell's pid). Windows: the child only.
// trace:TASK-1492 | ai:claude
#[cfg(unix)]
fn kill_group(child: &mut std::process::Child) {
    // SAFETY: plain syscall; the group is ours (process_group(0) at spawn)
    // and the leader is unreaped, so the id cannot have been reused.
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(windows)]
fn kill_group(child: &mut std::process::Child) {
    let _ = child.kill();
}

fn load_state(project_root: &Path) -> Result<NotifyState> {
    let path = state_path(project_root);
    let body = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&body)?)
}

fn save_state(project_root: &Path, state: &NotifyState) -> Result<()> {
    let path = state_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(state)? + "\n")?;
    Ok(())
}

fn state_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(STATE_FILE)
}

fn log_failure(project_root: &Path, err: anyhow::Error) {
    let path = project_root.join(".aida").join(LOG_FILE);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let line = format!("{} {}\n", Utc::now().to_rfc3339(), err);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

fn parse_duration(raw: &str) -> Option<Duration> {
    let s = raw.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 60 * 60)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1)
    } else {
        (s, 1)
    };
    num.trim()
        .parse::<u64>()
        .ok()
        .map(|n| Duration::from_secs(n.saturating_mul(mult)))
}

fn parse_time_minutes(raw: &str) -> Option<u16> {
    let (h, m) = raw.trim().split_once(':')?;
    let h = h.parse::<u16>().ok()?;
    let m = m.parse::<u16>().ok()?;
    if h < 24 && m < 60 {
        Some(h * 60 + m)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(spec: Option<&str>, kind: EventKind) -> String {
        serde_json::to_string(&Event::new(spec.map(ToOwned::to_owned), "run-123", kind)).unwrap()
    }

    #[test]
    fn config_unset_disables_notifications() {
        assert!(NotifyConfig::from_toml("[notify]\nmin_interval = \"5m\"\n")
            .unwrap()
            .is_none());
    }

    #[test]
    fn rule_evaluation_groups_shelve_escalation_and_idle() {
        let body = format!(
            "{}\n{}\n{}\n",
            event(
                Some("TASK-1"),
                EventKind::SpecShelved {
                    phase: "ci".into(),
                    kind: "ci-red".into(),
                    detail: None,
                    recovery_hint: None,
                }
            ),
            event(
                Some("TASK-2"),
                EventKind::AdvisorEscalated {
                    reason: "needs-human".into()
                }
            ),
            event(
                None,
                EventKind::QueueDrained {
                    shipped: 0,
                    shelved: 1,
                    excluded_from_batch: Some(0),
                    ineligible: vec![],
                }
            )
        );
        let fires = evaluate_rules(&body, &NotifyRules::default());
        assert_eq!(
            fires.iter().map(|f| f.rule).collect::<Vec<_>>(),
            vec!["shelve", "escalation", "idle_with_work"]
        );
        assert!(fires[0].message.contains("TASK-1"));
        assert!(fires[1].message.contains("TASK-2"));
        assert!(fires[2].message.contains("aida awaiting"));
    }

    // TASK-1297: a batch drain that shipped fine but excluded approved,
    // role-routed specs must still fire `idle_with_work` — the silent
    // filter, not a lack of shipping, is what a monitor needs to alarm on.
    // trace:TASK-1297 | ai:claude
    #[test]
    fn queue_drained_with_excluded_from_batch_fires_idle_with_work_even_when_shipped() {
        let body = event(
            None,
            EventKind::QueueDrained {
                shipped: 3,
                shelved: 0,
                excluded_from_batch: Some(16),
                ineligible: vec![],
            },
        );
        let fires = evaluate_rules(&format!("{body}\n"), &NotifyRules::default());
        assert_eq!(
            fires.iter().map(|f| f.rule).collect::<Vec<_>>(),
            vec!["idle_with_work"]
        );
        assert!(
            fires[0]
                .message
                .contains("16 other approved routed specs excluded"),
            "{}",
            fires[0].message
        );
    }

    // trace:BUG-1422 | ai:codex
    #[test]
    fn queue_drained_with_ineligible_members_fires_idle_with_work_and_names_them() {
        let body = event(
            None,
            EventKind::QueueDrained {
                shipped: 0,
                shelved: 0,
                excluded_from_batch: Some(0),
                ineligible: vec![crate::events::IneligibleBatchMember {
                    spec: "TASK-1277".into(),
                    reason: "blocked by in-flight work".into(),
                }],
            },
        );
        let fires = evaluate_rules(&format!("{body}\n"), &NotifyRules::default());
        assert_eq!(fires.len(), 1);
        assert_eq!(fires[0].rule, "idle_with_work");
        assert!(fires[0]
            .message
            .contains("TASK-1277 (blocked by in-flight work)"));
    }

    #[test]
    fn min_interval_suppresses_repeated_rule() {
        let mut state = NotifyState::default();
        let now = Utc::now();
        assert!(!should_suppress(
            &mut state,
            "shelve",
            now,
            Duration::from_secs(60)
        ));
        mark_sent(&mut state, "shelve", now);
        assert!(should_suppress(
            &mut state,
            "shelve",
            now + chrono::TimeDelta::seconds(10),
            Duration::from_secs(60)
        ));
        assert_eq!(state.rules["shelve"].suppressed, 1);
    }

    #[test]
    fn quiet_hours_handles_overnight_windows() {
        let q = QuietHours::parse("23:00-07:00").unwrap();
        assert!(q.contains(NaiveTime::from_hms_opt(23, 30, 0).unwrap()));
        assert!(q.contains(NaiveTime::from_hms_opt(6, 59, 0).unwrap()));
        assert!(!q.contains(NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn command_receives_message_on_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("capture.txt");
        let cfg = NotifyConfig {
            command: format!("cat > {}", capture.display()),
            min_interval: Duration::from_secs(0),
            quiet_hours: None,
            rules: NotifyRules::default(),
        };
        run_command(dir.path(), &cfg, "shelve", "ignored", "hello from stdin\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(capture).unwrap(),
            "hello from stdin\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn title_substitution_is_data_not_code() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("title.txt");
        let sentinel = dir.path().join("injected.txt");
        let cfg = NotifyConfig {
            command: format!("printf %s {{title}} > {}", capture.display()),
            min_interval: Duration::from_secs(0),
            quiet_hours: None,
            rules: NotifyRules::default(),
        };
        let title = format!("spec `touch {}` and 'quotes' $(id)", sentinel.display());
        run_command(dir.path(), &cfg, "shelve", &title, "msg\n").unwrap();
        assert!(
            !sentinel.exists(),
            "backticks in a title must not execute inside the notify shell"
        );
        assert_eq!(std::fs::read_to_string(capture).unwrap(), title);
    }

    // A hung notify command is killed at the bound instead of holding the
    // caller (the night-shift tick) past its deadline.
    // trace:TASK-1492 | ai:claude
    #[cfg(unix)]
    #[test]
    fn notify_bounded_command_is_killed_at_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = NotifyConfig {
            command: "sleep 5".to_string(),
            min_interval: Duration::from_secs(0),
            quiet_hours: None,
            rules: NotifyRules::default(),
        };
        let started = std::time::Instant::now();
        let err = run_command_bounded(
            dir.path(),
            &cfg,
            "mail-latency",
            "t",
            "m\n",
            Some(Duration::from_millis(300)),
        )
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10), "{err:#}");
        assert!(format!("{err:#}").contains("killed"), "{err:#}");
        // A command that finishes inside the bound still succeeds or fails
        // on its own exit status.
        let ok = NotifyConfig {
            command: "cat >/dev/null".to_string(),
            ..cfg.clone()
        };
        run_command_bounded(
            dir.path(),
            &ok,
            "r",
            "t",
            "m\n",
            Some(Duration::from_secs(10)),
        )
        .unwrap();
        let bad = NotifyConfig {
            command: "echo boom >&2; exit 3".to_string(),
            ..cfg
        };
        let err = run_command_bounded(
            dir.path(),
            &bad,
            "r",
            "t",
            "m\n",
            Some(Duration::from_secs(10)),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("boom"), "{err:#}");
    }

    // m2: a hung command that never reads stdin cannot block the caller in
    // the stdin write: a message far larger than the pipe buffer still
    // returns at the bound.
    // trace:BUG-1623 | ai:claude
    #[cfg(unix)]
    #[test]
    fn notify_bounded_stdin_write_cannot_outlive_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = NotifyConfig {
            command: "sleep 5".to_string(),
            min_interval: Duration::from_secs(0),
            quiet_hours: None,
            rules: NotifyRules::default(),
        };
        let message = "x".repeat(4 * 1024 * 1024);
        let started = std::time::Instant::now();
        let err = run_command_bounded(
            dir.path(),
            &cfg,
            "r",
            "t",
            &message,
            Some(Duration::from_millis(300)),
        )
        .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert!(format!("{err:#}").contains("killed"), "{err:#}");
        // A command that reads the whole large message still succeeds.
        let ok = NotifyConfig {
            command: "cat >/dev/null".to_string(),
            ..cfg
        };
        run_command_bounded(
            dir.path(),
            &ok,
            "r",
            "t",
            &message,
            Some(Duration::from_secs(10)),
        )
        .unwrap();
    }

    // m6: on Linux a background child the notify command leaves behind is
    // ended when the command exits (it shares the command's process group).
    // trace:BUG-1623 | ai:claude
    #[cfg(target_os = "linux")]
    #[test]
    fn notify_background_children_are_killed_when_the_command_exits() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("bg-pid");
        let cfg = NotifyConfig {
            command: format!(
                "sleep 7.456 & echo $! > {}; exit 0",
                shell_quote(&pidfile.display().to_string())
            ),
            min_interval: Duration::from_secs(0),
            quiet_hours: None,
            rules: NotifyRules::default(),
        };
        let started = std::time::Instant::now();
        run_command_bounded(
            dir.path(),
            &cfg,
            "r",
            "t",
            "m\n",
            Some(Duration::from_secs(5)),
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(4));
        let pid: libc::pid_t = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        // SAFETY: signal 0 only probes whether the pid still exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "background child {pid} outlived the notify command"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    // N1: a timed-out compound command or pipeline leaves no process of its
    // group behind (a signal to `sh` alone would orphan the grandchildren),
    // and the call returns within the bound.
    // trace:TASK-1492 | ai:claude
    #[cfg(unix)]
    #[test]
    fn notify_timeout_kills_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        for (i, body) in ["sleep 7.123 ; true", "sleep 7.321 | cat ; true"]
            .iter()
            .enumerate()
        {
            let pidfile = dir.path().join(format!("pgid-{i}"));
            let cfg = NotifyConfig {
                command: format!(
                    "echo $$ > {} ; {body}",
                    shell_quote(&pidfile.display().to_string())
                ),
                min_interval: Duration::from_secs(0),
                quiet_hours: None,
                rules: NotifyRules::default(),
            };
            let started = std::time::Instant::now();
            let err = run_command_bounded(
                dir.path(),
                &cfg,
                "r",
                "t",
                "m\n",
                Some(Duration::from_millis(500)),
            )
            .unwrap_err();
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "{body}: {:?}",
                started.elapsed()
            );
            assert!(format!("{err:#}").contains("killed"), "{err:#}");
            let pgid: libc::pid_t = std::fs::read_to_string(&pidfile)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            // Killed members are reaped by their new parent shortly after;
            // poll until the group is empty (bounded).
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                // SAFETY: signal 0 only probes for group members.
                let alive = unsafe { libc::killpg(pgid, 0) } == 0;
                if !alive {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "{body}: process group {pgid} still has members"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    // send_direct reports what really happened: a message dropped by the
    // rule's min_interval is Suppressed, not Sent, and a timed-out command is
    // an error that leaves the rule unmarked.
    // trace:TASK-1492 | ai:claude
    #[cfg(unix)]
    #[test]
    fn notify_send_direct_reports_the_real_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(
            send_direct(root, "mail-latency", "t", "m")
                .unwrap()
                .delivery(),
            DirectDelivery::NotConfigured
        );
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida").join("config.toml"),
            "[notify]\ncommand = \"cat >/dev/null\"\nmin_interval = \"30m\"\n",
        )
        .unwrap();
        assert_eq!(
            send_direct(root, "mail-latency", "t", "m")
                .unwrap()
                .delivery(),
            DirectDelivery::Sent
        );
        assert_eq!(
            send_direct(root, "mail-latency", "t", "m")
                .unwrap()
                .delivery(),
            DirectDelivery::Suppressed
        );
        let deferred = DirectNotifyOutcome {
            configured: true,
            pending: 1,
            ..Default::default()
        };
        assert_eq!(deferred.delivery(), DirectDelivery::Deferred);

        std::fs::write(
            root.join(".aida").join("config.toml"),
            "[notify]\ncommand = \"sleep 5\"\nmin_interval = \"0s\"\n",
        )
        .unwrap();
        let started = std::time::Instant::now();
        assert!(send_direct_bounded(
            root,
            "other-rule",
            "t",
            "m",
            Some(Duration::from_millis(300))
        )
        .is_err());
        assert!(started.elapsed() < Duration::from_secs(10));
        let state = load_state(root).unwrap();
        assert!(state
            .rules
            .get("other-rule")
            .is_none_or(|r| r.last_fire.is_none()));
    }
}
