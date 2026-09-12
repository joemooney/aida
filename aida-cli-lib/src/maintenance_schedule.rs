//! Native no-daemon maintenance task registry.
//!
//! This is distinct from the older advisor schedule surface in `schedule.rs`:
//! advisor schedules file TASKs into the queue, while this module runs a small
//! allowlist of existing AIDA maintenance commands directly.
//!
//! trace:STORY-1047 | ai:codex

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use crate::cli::MaintenanceScheduleCommand;

const DEFAULT_MIN_GAP: &str = "60s";

#[derive(Debug, Clone, Deserialize)]
struct ConfigFile {
    schedule: Option<ScheduleConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct ScheduleConfig {
    #[serde(default = "default_min_gap")]
    min_gap: String,
    #[serde(default)]
    tasks: Vec<TaskConfig>,
}

fn default_min_gap() -> String {
    DEFAULT_MIN_GAP.to_string()
}

#[derive(Debug, Clone, Deserialize)]
struct TaskConfig {
    name: String,
    command: String,
    interval: String,
    #[serde(default, alias = "quiet-hours")]
    quiet_hours: Option<String>,
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Clone)]
struct Task {
    name: String,
    command: ScheduledCommand,
    interval: Duration,
    quiet_hours: Option<QuietHours>,
    enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduledCommand {
    display: &'static str,
    args: &'static [&'static str],
    hook_allowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuietHours {
    start: NaiveTime,
    end: NaiveTime,
}

impl QuietHours {
    fn contains(&self, now: NaiveTime) -> bool {
        if self.start <= self.end {
            now >= self.start && now < self.end
        } else {
            now >= self.start || now < self.end
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ScheduleState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_tick_at: Option<DateTime<Utc>>,
    #[serde(default)]
    tasks: BTreeMap<String, TaskState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TaskState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_success_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_status: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskOutcome {
    status: i32,
    stdout: String,
    stderr: String,
}

pub(crate) fn handle_schedule_command(
    cmd: &MaintenanceScheduleCommand,
    store_path: &Path,
) -> Result<()> {
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;
    match cmd {
        MaintenanceScheduleCommand::Tick { hook } => {
            let outcome = tick(project_root, *hook)?;
            if !outcome.is_empty() {
                println!("{}", outcome.join("\n"));
            }
        }
        MaintenanceScheduleCommand::Run { name } => {
            let outcome = run_now(project_root, name.as_deref())?;
            if outcome.is_empty() {
                println!("No scheduled maintenance tasks configured.");
            } else {
                println!("{}", outcome.join("\n"));
            }
        }
        MaintenanceScheduleCommand::Status { json } => status(project_root, *json)?,
        MaintenanceScheduleCommand::EmitCron => emit_cron(project_root)?,
    }
    Ok(())
}

fn tick(project_root: &Path, hook: bool) -> Result<Vec<String>> {
    let Some(config) = load_config(project_root)? else {
        return Ok(vec![]);
    };
    let mut state = load_state(project_root)?;
    let now = Utc::now();
    if let Some(last_tick) = state.last_tick_at {
        let min_gap = parse_duration(&config.min_gap)
            .with_context(|| format!("invalid [schedule] min_gap '{}'", config.min_gap))?;
        if now.signed_duration_since(last_tick) < min_gap {
            return Ok(vec![format!(
                "schedule tick: suppressed by min-gap {}",
                config.min_gap
            )]);
        }
    }
    tick_with_executor(
        project_root,
        config,
        &mut state,
        now,
        hook,
        run_aida_command,
    )
}

fn run_now(project_root: &Path, only: Option<&str>) -> Result<Vec<String>> {
    let Some(config) = load_config(project_root)? else {
        return Ok(vec![]);
    };
    if let Some(name) = only {
        if !config.tasks.iter().any(|task| task.name == name) {
            anyhow::bail!("no scheduled maintenance task named '{name}'");
        }
    }
    let mut state = load_state(project_root)?;
    let now = Utc::now();
    run_with_executor(
        project_root,
        config,
        &mut state,
        now,
        only,
        run_aida_command,
    )
}

fn status(project_root: &Path, json: bool) -> Result<()> {
    let Some(config) = load_config(project_root)? else {
        if json {
            println!("[]");
        } else {
            println!("No scheduled maintenance tasks configured.");
        }
        return Ok(());
    };
    let state = load_state(project_root)?;
    if json {
        let rows: Vec<_> = config
            .tasks
            .iter()
            .map(|task| status_row(task, &state))
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!(
        "{:<22} {:<18} {:<10} {:<22} {:<22} Command",
        "Name", "Interval", "Status", "Last run", "Next due"
    );
    for task in &config.tasks {
        let row = status_row(task, &state);
        println!(
            "{:<22} {:<18} {:<10} {:<22} {:<22} {}",
            row.name, row.interval, row.status, row.last_run, row.next_due, row.command
        );
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct StatusRow {
    name: String,
    interval: String,
    command: String,
    enabled: bool,
    status: String,
    last_run: String,
    next_due: String,
}

fn status_row(task: &Task, state: &ScheduleState) -> StatusRow {
    let task_state = state.tasks.get(&task.name);
    let last_run_at = task_state.and_then(|s| s.last_run_at);
    let next_due = last_run_at.map(|last| last + task.interval);
    let status = if !task.enabled {
        "disabled"
    } else if quiet_now(task) {
        "quiet"
    } else if due_at(task, task_state, Utc::now()) {
        "due"
    } else {
        "pending"
    };
    StatusRow {
        name: task.name.clone(),
        interval: format_duration(task.interval),
        command: task.command.display.to_string(),
        enabled: task.enabled,
        status: status.to_string(),
        last_run: last_run_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "(never)".to_string()),
        next_due: next_due
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "now".to_string()),
    }
}

fn emit_cron(project_root: &Path) -> Result<()> {
    let Some(config) = load_config(project_root)? else {
        println!("# No scheduled maintenance tasks configured.");
        return Ok(());
    };
    println!("# AIDA scheduled maintenance. Paste into crontab with `crontab -e`.");
    println!("# Run from: {}", project_root.display());
    for task in config.tasks.iter().filter(|task| task.enabled) {
        println!(
            "{} cd {} && aida schedule run {}",
            cron_interval(task.interval),
            shell_quote(&project_root.display().to_string()),
            shell_quote(&task.name)
        );
    }
    Ok(())
}

fn tick_with_executor<F>(
    project_root: &Path,
    config: LoadedScheduleConfig,
    state: &mut ScheduleState,
    now: DateTime<Utc>,
    hook: bool,
    exec: F,
) -> Result<Vec<String>>
where
    F: Fn(&Path, &ScheduledCommand) -> Result<TaskOutcome>,
{
    let mut ran_any = false;
    let mut lines = Vec::new();
    for task in &config.tasks {
        if !task.enabled {
            continue;
        }
        if hook && !task.command.hook_allowed {
            continue;
        }
        let task_state = state.tasks.get(&task.name);
        if !due_at(task, task_state, now) || quiet_now(task) {
            continue;
        }
        let outcome = exec(project_root, &task.command).unwrap_or_else(|e| TaskOutcome {
            status: 1,
            stdout: String::new(),
            stderr: e.to_string(),
        });
        ran_any = true;
        record_outcome(project_root, state, task, now, &outcome)?;
        lines.push(format!(
            "schedule tick: {} {}",
            task.name,
            if outcome.status == 0 { "ok" } else { "failed" }
        ));
    }
    if ran_any {
        state.last_tick_at = Some(now);
        save_state(project_root, state)?;
    }
    Ok(lines)
}

fn run_with_executor<F>(
    project_root: &Path,
    config: LoadedScheduleConfig,
    state: &mut ScheduleState,
    now: DateTime<Utc>,
    only: Option<&str>,
    exec: F,
) -> Result<Vec<String>>
where
    F: Fn(&Path, &ScheduledCommand) -> Result<TaskOutcome>,
{
    let mut ran_any = false;
    let mut lines = Vec::new();
    for task in &config.tasks {
        if only.is_some_and(|name| task.name != name) {
            continue;
        }
        if only.is_none() && !task.enabled {
            continue;
        }
        let outcome = exec(project_root, &task.command).unwrap_or_else(|e| TaskOutcome {
            status: 1,
            stdout: String::new(),
            stderr: e.to_string(),
        });
        ran_any = true;
        record_outcome(project_root, state, task, now, &outcome)?;
        lines.push(format!(
            "schedule run: {} {}",
            task.name,
            if outcome.status == 0 { "ok" } else { "failed" }
        ));
    }
    if ran_any {
        state.last_tick_at = Some(now);
        save_state(project_root, state)?;
    }
    Ok(lines)
}

type LoadedScheduleConfig = ScheduleConfigLoaded;

#[derive(Debug, Clone)]
struct ScheduleConfigLoaded {
    min_gap: String,
    tasks: Vec<Task>,
}

fn load_config(project_root: &Path) -> Result<Option<LoadedScheduleConfig>> {
    let path = project_root.join(".aida").join("config.toml");
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let parsed: ConfigFile =
        toml::from_str(&body).with_context(|| format!("failed to parse {}", path.display()))?;
    let Some(raw) = parsed.schedule else {
        return Ok(None);
    };
    let mut tasks = Vec::new();
    for task in raw.tasks {
        let command = parse_scheduled_command(&task.command)?;
        tasks.push(Task {
            name: task.name,
            command,
            interval: parse_duration(&task.interval).with_context(|| {
                format!("invalid interval for scheduled task '{}'", task.command)
            })?,
            quiet_hours: task
                .quiet_hours
                .as_deref()
                .map(parse_quiet_hours)
                .transpose()?,
            enabled: task.enabled,
        });
    }
    Ok(Some(ScheduleConfigLoaded {
        min_gap: raw.min_gap,
        tasks,
    }))
}

fn parse_scheduled_command(s: &str) -> Result<ScheduledCommand> {
    let normalized = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let command = match normalized.as_str() {
        "cache verify" => ScheduledCommand {
            display: "cache verify",
            args: &["cache", "verify"],
            hook_allowed: true,
        },
        "session reap" => ScheduledCommand {
            display: "session reap",
            args: &["session", "reap", "--yes"],
            hook_allowed: true,
        },
        "queue gc" => ScheduledCommand {
            display: "queue gc",
            args: &["queue", "gc"],
            hook_allowed: true,
        },
        "notify check" => ScheduledCommand {
            display: "notify check",
            args: &["notify", "check"],
            hook_allowed: true,
        },
        "fetch --code-only" => ScheduledCommand {
            display: "fetch --code-only",
            args: &["fetch", "--code-only", "--quiet"],
            hook_allowed: false,
        },
        "store compact" | "store gc" => ScheduledCommand {
            display: "store compact",
            args: &["store", "compact"],
            hook_allowed: false,
        },
        _ => anyhow::bail!(
            "unknown scheduled task command '{}'; valid commands: {}",
            s,
            valid_commands().join(", ")
        ),
    };
    Ok(command)
}

fn valid_commands() -> Vec<&'static str> {
    vec![
        "cache verify",
        "session reap",
        "queue gc",
        "notify check",
        "fetch --code-only",
        "store compact",
        "store gc",
    ]
}

fn run_aida_command(project_root: &Path, command: &ScheduledCommand) -> Result<TaskOutcome> {
    let exe = std::env::current_exe().context("failed to resolve current aida executable")?;
    let output = ProcessCommand::new(exe)
        .current_dir(project_root)
        .env("AIDA_SCHEDULE_CHILD", "1")
        .args(command.args)
        .output()
        .with_context(|| format!("failed to run `aida {}`", command.display))?;
    Ok(TaskOutcome {
        status: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn due_at(task: &Task, task_state: Option<&TaskState>, now: DateTime<Utc>) -> bool {
    match task_state.and_then(|s| s.last_run_at) {
        None => true,
        Some(last) => now.signed_duration_since(last) >= task.interval,
    }
}

fn quiet_now(task: &Task) -> bool {
    task.quiet_hours
        .is_some_and(|quiet| quiet.contains(Local::now().time()))
}

fn record_outcome(
    project_root: &Path,
    state: &mut ScheduleState,
    task: &Task,
    now: DateTime<Utc>,
    outcome: &TaskOutcome,
) -> Result<()> {
    let entry = state.tasks.entry(task.name.clone()).or_default();
    entry.last_run_at = Some(now);
    entry.last_status = Some(outcome.status);
    if outcome.status == 0 {
        entry.last_success_at = Some(now);
    } else {
        append_log(project_root, task, outcome)?;
    }
    Ok(())
}

fn state_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("schedule-state.json")
}

fn log_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("schedule.log")
}

fn load_state(project_root: &Path) -> Result<ScheduleState> {
    let path = state_path(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(ScheduleState::default());
    };
    serde_json::from_str(&body).with_context(|| format!("failed to parse {}", path.display()))
}

fn save_state(project_root: &Path, state: &ScheduleState) -> Result<()> {
    let path = state_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(state)?;
    aida_core::write_atomic(&path, &body)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn append_log(project_root: &Path, task: &Task, outcome: &TaskOutcome) -> Result<()> {
    use std::io::Write;
    let path = log_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(
        file,
        "[{}] task={} command={} status={} stderr={}",
        Utc::now().to_rfc3339(),
        task.name,
        task.command.display,
        outcome.status,
        outcome.stderr.trim()
    )?;
    Ok(())
}

fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty duration");
    }
    let split = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| anyhow::anyhow!("duration '{s}' has no unit"))?;
    if split == 0 {
        anyhow::bail!("duration '{s}' has no leading number");
    }
    let (num, unit) = s.split_at(split);
    let n: i64 = num.parse()?;
    if n <= 0 {
        anyhow::bail!("duration '{s}' must be positive");
    }
    match unit {
        "s" => Ok(Duration::seconds(n)),
        "m" => Ok(Duration::minutes(n)),
        "h" => Ok(Duration::hours(n)),
        "d" => Ok(Duration::days(n)),
        "w" => Ok(Duration::weeks(n)),
        _ => anyhow::bail!("duration '{s}' has unknown unit '{unit}'"),
    }
}

fn parse_quiet_hours(s: &str) -> Result<QuietHours> {
    let (start, end) = s
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("quiet-hours must be HH:MM-HH:MM"))?;
    Ok(QuietHours {
        start: NaiveTime::parse_from_str(start.trim(), "%H:%M")?,
        end: NaiveTime::parse_from_str(end.trim(), "%H:%M")?,
    })
}

fn format_duration(duration: Duration) -> String {
    if duration.num_weeks() > 0 && duration == Duration::weeks(duration.num_weeks()) {
        format!("{}w", duration.num_weeks())
    } else if duration.num_days() > 0 && duration == Duration::days(duration.num_days()) {
        format!("{}d", duration.num_days())
    } else if duration.num_hours() > 0 && duration == Duration::hours(duration.num_hours()) {
        format!("{}h", duration.num_hours())
    } else if duration.num_minutes() > 0 && duration == Duration::minutes(duration.num_minutes()) {
        format!("{}m", duration.num_minutes())
    } else {
        format!("{}s", duration.num_seconds())
    }
}

fn cron_interval(duration: Duration) -> &'static str {
    if duration <= Duration::hours(1) {
        "0 * * * *"
    } else if duration <= Duration::days(1) {
        "0 3 * * *"
    } else if duration <= Duration::weeks(1) {
        "0 3 * * 0"
    } else {
        "0 3 1 * *"
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 11, hour, 0, 0).unwrap()
    }

    fn task(name: &str, interval: &str, command: &str) -> Task {
        Task {
            name: name.to_string(),
            command: parse_scheduled_command(command).unwrap(),
            interval: parse_duration(interval).unwrap(),
            quiet_hours: None,
            enabled: true,
        }
    }

    fn config(tasks: Vec<Task>) -> LoadedScheduleConfig {
        LoadedScheduleConfig {
            min_gap: DEFAULT_MIN_GAP.to_string(),
            tasks,
        }
    }

    #[test]
    fn rejects_unknown_commands_with_valid_set() {
        let err = parse_scheduled_command("cache veryfy")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown scheduled task command"));
        assert!(err.contains("cache verify"));
    }

    #[test]
    fn tick_runs_only_due_tasks() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = ScheduleState::default();
        state.tasks.insert(
            "recent".to_string(),
            TaskState {
                last_run_at: Some(at(11)),
                ..TaskState::default()
            },
        );
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen_exec = Rc::clone(&seen);
        let out = tick_with_executor(
            tmp.path(),
            config(vec![
                task("due", "1h", "cache verify"),
                task("recent", "24h", "queue gc"),
            ]),
            &mut state,
            at(12),
            false,
            move |_root, cmd| {
                seen_exec.borrow_mut().push(cmd.display.to_string());
                Ok(TaskOutcome {
                    status: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            },
        )
        .unwrap();
        assert_eq!(&*seen.borrow(), &["cache verify"]);
        assert_eq!(out, vec!["schedule tick: due ok"]);
    }

    #[test]
    fn min_gap_suppresses_recent_tick() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            r#"
[schedule]
min_gap = "60s"

[[schedule.tasks]]
name = "cache"
command = "cache verify"
interval = "1h"
enabled = true
"#,
        )
        .unwrap();
        let state = ScheduleState {
            last_tick_at: Some(Utc::now()),
            tasks: BTreeMap::new(),
        };
        save_state(tmp.path(), &state).unwrap();
        let lines = tick(tmp.path(), false).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("suppressed by min-gap"));
    }

    #[test]
    fn failing_task_logs_and_later_task_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = ScheduleState::default();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen_exec = Rc::clone(&seen);
        tick_with_executor(
            tmp.path(),
            config(vec![
                task("bad", "1h", "cache verify"),
                task("good", "1h", "queue gc"),
            ]),
            &mut state,
            at(12),
            false,
            move |_root, cmd| {
                seen_exec.borrow_mut().push(cmd.display.to_string());
                Ok(TaskOutcome {
                    status: if cmd.display == "cache verify" { 2 } else { 0 },
                    stdout: String::new(),
                    stderr: "boom".to_string(),
                })
            },
        )
        .unwrap();
        assert_eq!(&*seen.borrow(), &["cache verify", "queue gc"]);
        let log = std::fs::read_to_string(log_path(tmp.path())).unwrap();
        assert!(log.contains("task=bad"));
        assert!(log.contains("boom"));
    }

    #[test]
    fn emit_cron_uses_valid_crontab_prefixes() {
        assert_eq!(cron_interval(parse_duration("1h").unwrap()), "0 * * * *");
        assert_eq!(cron_interval(parse_duration("24h").unwrap()), "0 3 * * *");
        assert_eq!(cron_interval(parse_duration("7d").unwrap()), "0 3 * * 0");
    }
}
