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

    match run_command(project_root, &config, rule, title, message) {
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
            EventKind::QueueDrained { shipped, shelved } => {
                if shipped == 0 || shelved > 0 {
                    drained_idle.push(format!("drain shipped {shipped}, shelved {shelved}"));
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
    // Substituted values are spliced into a shell command line, and spec
    // titles carry shell-active characters (backticks, quotes, $). Quote them
    // so title content is data, never code.
    let command = config
        .command
        .replace("{title}", &shell_quote(title))
        .replace("{rule}", &shell_quote(rule));
    #[cfg(unix)]
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn notify command `{command}`"))?;
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
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(message.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "notify command exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
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
                    kind: "ci-red".into()
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
                    shelved: 1
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
}
