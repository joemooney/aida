// trace:TASK-3424 | ai:codex
use super::*;
use chrono::TimeZone;
use std::cell::RefCell;
use std::rc::Rc;

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 11, hour, 0, 0).unwrap()
}

/// STORY-2220 AC7: `aida schedule status` / `list` show the docs-sweep
/// job's last run, findings count and mode, and no other job grows a
/// detail line.
// trace:STORY-2220 | ai:claude
#[test]
fn story_2220_status_shows_the_docs_sweep_detail_line() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        "[[schedule.jobs]]\nname = \"docs-sweep\"\ncommand = \"docs sweep\"\nevery = \"1d\"\n\
             enabled = true\n\n[[schedule.jobs]]\nname = \"watchdog\"\ncommand = \"doctor\"\n\
             every = \"15m\"\nenabled = true\n",
    )
    .unwrap();
    let home = tempfile::tempdir().unwrap();
    let tasks = {
        let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path());
        all_tasks(tmp.path()).unwrap()
    };
    let before = status_rows(tmp.path(), &tasks, at(12)).unwrap();
    assert!(before.iter().all(|r| r.detail.is_none()), "no sweep yet");
    assert_eq!(detail_lines(&before), "");

    crate::docs_sweep::save_state(
        tmp.path(),
        &crate::docs_sweep::SweepState {
            last_swept_rev: Some("0123456789abcdef".into()),
            last_run_at: Some(at(9)),
            findings: 2,
            mode: Some(crate::docs_sweep::SweepMode::Report),
            ..Default::default()
        },
    )
    .unwrap();
    let rows = status_rows(tmp.path(), &tasks, at(12)).unwrap();
    let sweep = rows.iter().find(|r| r.name == "docs-sweep").unwrap();
    let detail = sweep.detail.as_deref().expect("docs-sweep detail");
    assert!(detail.contains("2 findings · mode report"), "{detail}");
    assert!(
        rows.iter()
            .filter(|r| r.name != "docs-sweep")
            .all(|r| r.detail.is_none()),
        "only the docs-sweep job carries a detail"
    );
    let text = detail_lines(&rows);
    assert_eq!(
            text,
            "\ndocs-sweep: last sweep 2026-09-11 09:00 UTC · 2 findings · mode report · swept to 0123456789\n"
        );
    let json = serde_json::to_value(sweep).unwrap();
    assert_eq!(json["detail"].as_str(), Some(detail));
}

fn task(name: &str, interval: &str, command: &str) -> Task {
    Task {
        name: name.to_string(),
        kind: JobKind::Substrate,
        seats: vec!["*".to_string()],
        command: Some(parse_scheduled_command(command).unwrap()),
        prompt: None,
        interval: Some(parse_duration(interval).unwrap()),
        on: Vec::new(),
        when: None,
        when_raw: None,
        while_: None,
        while_raw: None,
        quiet_hours: None,
        enabled: true,
        source: JobSource::Project,
        problem: None,
        invalid_command: None,
    }
}

fn seat_task(name: &str, seats: &[&str], interval: Option<&str>, on: &[&str]) -> Task {
    Task {
        name: name.to_string(),
        kind: JobKind::Seat,
        seats: seats.iter().map(|s| s.to_string()).collect(),
        command: None,
        prompt: Some(format!("do {name}")),
        interval: interval.map(|i| parse_duration(i).unwrap()),
        on: on
            .iter()
            .map(|s| EventTrigger::parse(name, s).unwrap())
            .collect(),
        when: None,
        when_raw: None,
        while_: None,
        while_raw: None,
        quiet_hours: None,
        enabled: true,
        source: JobSource::Project,
        problem: None,
        invalid_command: None,
    }
}

fn config(tasks: Vec<Task>) -> LoadedScheduleConfig {
    LoadedScheduleConfig {
        min_gap: DEFAULT_MIN_GAP.to_string(),
        tasks,
    }
}

// One actual maintenance tick runs the inline job. The scheduler's own
// ledger commit is a subsequent store write and remains inside the flow cap.
// trace:STORY-1661 | ai:codex
#[test]
fn story_1661_schedule_tick_runs_history_inline_and_skips_prompt_hooks() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = crate::history_indexer::tests::Fixture::new();
    for n in 1..=200 {
        fx.append(n);
    }
    let job = task(
        "history-index-tick",
        "15m",
        "cache refresh --history --if-behind",
    );
    assert!(!job.command.as_ref().unwrap().hook_allowed);
    assert_eq!(
        job.command.as_ref().unwrap().args,
        &["cache", "refresh", "--history", "--if-behind"]
    );
    let calls = std::cell::Cell::new(0);
    let mut state = ScheduleState::default();
    let lines = tick_with_executor(
        fx.root.path(),
        config(vec![job.clone()]),
        &mut state,
        at(12),
        true,
        |_root, _cmd| panic!("history indexing cannot run from a prompt hook"),
    )
    .unwrap();
    assert!(lines.is_empty());
    let lines = tick_with_executor(
        fx.root.path(),
        config(vec![job]),
        &mut state,
        at(13),
        false,
        |_root, _cmd| {
            calls.set(calls.get() + 1);
            assert_eq!(
                crate::history_indexer::run_tick(&fx.store).unwrap(),
                crate::history_indexer::TickOutcome::Advanced {
                    appended: 200,
                    lag: 0
                }
            );
            Ok(TaskOutcome {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(lines.len(), 1);
    assert!(schedule_ledger::load(&fx.store, "history-index-tick").is_some());
    let before = crate::history_cache::test_support::commit_count(&fx.db);
    crate::history_cache::serve_flow(&fx.store, &crate::history_indexer::tests::opts()).unwrap();
    let extra = crate::history_cache::test_support::commit_count(&fx.db) - before;
    assert_eq!(extra, 1, "the scheduler ledger is a real subsequent commit");
    assert!(
        extra <= crate::history_cache::FLOW_CATCH_UP_CAP as i64,
        "scheduler bookkeeping must not push the next flow beyond its existing cap"
    );
}

fn ok_exec(
    seen: Rc<RefCell<Vec<String>>>,
) -> impl Fn(&Path, &ScheduledCommand) -> Result<TaskOutcome> {
    move |_root, cmd| {
        seen.borrow_mut().push(cmd.display.to_string());
        Ok(TaskOutcome {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn parse_project(toml_body: &str) -> Result<Option<LoadedScheduleConfig>> {
    let parsed: ConfigFile = toml::from_str(toml_body)?;
    match parsed.schedule {
        Some(raw) => build_config(raw, JobSource::Project),
        None => Ok(None),
    }
}

// trace:TASK-1222 | ai:codex
#[test]
// trace:TASK-1222.ac14b158 | ai:codex
fn task_1222_weekly_gc_reports_to_advisor_without_apply() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let registry = load_config(repo_root).unwrap().unwrap();
    let guard = registry
        .tasks
        .iter()
        .find(|t| t.name == "worktree-lifecycle-gc")
        .unwrap();
    assert!(guard.enabled);
    assert_eq!(guard.interval, Some(Duration::days(7)));
    let command = guard.command.as_ref().unwrap();
    assert_eq!(
        command.args,
        &["worktree", "gc", "--json", "--fail-on-findings"]
    );
    assert!(!command.hook_allowed);
    assert_eq!(command_timeout(command), HYGIENE_COMMAND_TIMEOUT);
    assert!(parse_scheduled_command("worktree gc --apply").is_err());
    assert!(parse_scheduled_command("worktree gc --yes --force").is_err());
    let route = registry
        .tasks
        .iter()
        .find(|t| t.name == "worktree-lifecycle-gc-route")
        .unwrap();
    assert!(route.enabled);
    assert_eq!(route.seats, vec!["advisor"]);
    assert_eq!(
        route.on.iter().map(ToString::to_string).collect::<Vec<_>>(),
        vec!["CronJobFailed:worktree-lifecycle-gc"]
    );
    assert!(route.command.is_none());
}

// trace:TASK-1596 | ai:codex
#[test]
fn task_1596_monthly_guard_is_superseded_by_weekly_gc() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let registry = load_config(repo_root).unwrap().unwrap();
    assert!(!registry
        .tasks
        .iter()
        .any(|t| t.name.starts_with("merged-worktrees-guard")));
    assert!(registry
        .tasks
        .iter()
        .any(|t| t.name == "worktree-lifecycle-gc" && t.enabled));
}

// trace:BUG-1746 | ai:codex
#[test]
fn bug_1746_mirror_sync_is_scheduled_but_hook_forbidden() {
    let cmd = parse_scheduled_command("remote mirror-sync").unwrap();
    assert_eq!(cmd.display, "remote mirror-sync");
    assert_eq!(cmd.args, &["remote", "mirror-sync", "--json"]);
    assert!(!cmd.hook_allowed, "mirror sync makes a network call");
    assert!(!cmd.args.contains(&"--force") && !cmd.args.contains(&"-f"));

    let tmp = tempfile::tempdir().unwrap();
    let mut state = ScheduleState::default();
    let seen = Rc::new(RefCell::new(Vec::new()));
    tick_with_executor(
        tmp.path(),
        config(vec![task("hub-mirror-sync", "1h", "remote mirror-sync")]),
        &mut state,
        at(12),
        true,
        ok_exec(Rc::clone(&seen)),
    )
    .unwrap();
    assert!(
        seen.borrow().is_empty(),
        "hook ticks must skip network jobs"
    );
}

// This pins the cadence trigger itself: command allowlisting alone must
// not cause a run when the project has no enabled cadence registration.
// trace:BUG-1746 | ai:codex
#[test]
fn bug_1746_cadence_dispatches_only_when_registered_and_enabled() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("aida-cli-lib has a workspace parent");
    let parsed = load_config(repo_root).unwrap().unwrap();
    let registered: Vec<Task> = parsed
        .tasks
        .into_iter()
        .filter(|t| t.name == "hub-mirror-sync")
        .collect();
    assert_eq!(registered.len(), 1, "repo must register the mirror cadence");
    let tmp = tempfile::tempdir().unwrap();
    let mut state = ScheduleState::default();
    let seen = Rc::new(RefCell::new(Vec::new()));
    tick_with_executor(
        tmp.path(),
        config(registered),
        &mut state,
        at(12),
        false,
        ok_exec(Rc::clone(&seen)),
    )
    .unwrap();
    assert_eq!(&*seen.borrow(), &["remote mirror-sync"]);

    for tasks in [
        Vec::new(),
        vec![{
            let mut disabled = task("hub-mirror-sync", "1h", "remote mirror-sync");
            disabled.enabled = false;
            disabled
        }],
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = ScheduleState::default();
        let seen = Rc::new(RefCell::new(Vec::new()));
        tick_with_executor(
            tmp.path(),
            config(tasks),
            &mut state,
            at(12),
            false,
            ok_exec(Rc::clone(&seen)),
        )
        .unwrap();
        assert!(
            seen.borrow().is_empty(),
            "absent or disabled cadence must not dispatch"
        );
    }
}

// trace:BUG-1746 | ai:codex
#[test]
fn bug_1746_mirror_sync_failure_does_not_wake_unrelated_guard_routes() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("aida-cli-lib has a workspace parent");
    let parsed = load_config(repo_root).unwrap().unwrap();
    assert!(
        build_unbound_route_findings(&parsed.tasks).is_empty(),
        "repo route jobs must all be bound to their guard"
    );
    let route_names = [
        "hub-drift-guard-route",
        "disk-headroom-guard-route",
        "performance-guard-route",
        "watchdog-route",
        "worktree-lifecycle-gc-route",
    ];
    let routes: Vec<Task> = parsed
        .tasks
        .into_iter()
        .filter(|t| route_names.contains(&t.name.as_str()))
        .collect();
    assert_eq!(
        routes.len(),
        route_names.len(),
        "all selected repository route jobs are in fixture"
    );

    // The fake executor models an unreachable/rejecting mirror; tick must
    // persist the failure as CronJobFailed under the cadence job's name.
    let tmp = tempfile::tempdir().unwrap();
    let mut run_state = ScheduleState::default();
    tick_with_executor(
        tmp.path(),
        config(vec![task("hub-mirror-sync", "1h", "remote mirror-sync")]),
        &mut run_state,
        at(12),
        false,
        |_root, _cmd| {
            Ok(TaskOutcome {
                status: 1,
                stdout: String::new(),
                stderr: "mirror unavailable".into(),
            })
        },
    )
    .unwrap();
    let events = events::read_all(tmp.path());
    assert!(
        events.iter().any(|event| matches!(
            &event.kind,
            EventKind::CronJobFailed { job, error, .. }
                if job == "hub-mirror-sync" && error.contains("mirror unavailable")
        )),
        "failed cadence run must emit its own CronJobFailed: {events:?}"
    );

    bug_1655_route(tmp.path(), routes, &events);
    let store = store_root(tmp.path());
    let due: Vec<_> = route_names
        .iter()
        .filter(|name| schedule_ledger::load(&store, name).is_some_and(|l| l.due_since.is_some()))
        .collect();
    assert!(
        due.is_empty(),
        "transient mirror outage must not route guard jobs: {due:?}"
    );
}

// trace:STORY-1218 | ai:claude
#[test]
fn schedule_command_table_shift_tick_not_hook_allowed() {
    let cmd = parse_scheduled_command("shift tick").unwrap();
    assert_eq!(cmd.args, &["shift", "tick"]);
    assert!(
        !cmd.hook_allowed,
        "a per-turn hook tick must never be able to launch a drain"
    );
}

// STORY-2351: the hourly hygiene pass is a substrate job, off the
// per-turn hook path, with a longer kill ceiling than its soft budget.
// trace:STORY-2351 | ai:claude
#[test]
fn story_2351_session_hygiene_is_scheduled_off_the_hook_path() {
    let cmd = parse_scheduled_command("session  hygiene").unwrap();
    assert_eq!(cmd.args, &["session", "hygiene"]);
    assert!(
        !cmd.hook_allowed,
        "a per-turn hook must never delete caches"
    );
    assert_eq!(command_timeout(&cmd), HYGIENE_COMMAND_TIMEOUT);
    assert!(
        HYGIENE_COMMAND_TIMEOUT > crate::session_hygiene::SOFT_BUDGET,
        "the soft budget must defer the reclaim before the scheduler kills the run"
    );
    let reap = parse_scheduled_command("session reap").unwrap();
    assert_eq!(command_timeout(&reap), SCHEDULED_COMMAND_TIMEOUT);
    // The disk guard's floor reclaim runs under the default ceiling; its
    // own budget leaves the re-probe and report room before the kill.
    assert!(
        crate::doctor_cmd::FLOOR_RECLAIM_BUDGET * 2 <= SCHEDULED_COMMAND_TIMEOUT,
        "the floor reclaim must stop well before the guard child is killed"
    );
}

// STORY-2351: this repo registers the hygiene pass hourly and routes its
// must-keep failures (only those) to the advisor.
// trace:STORY-2351 | ai:claude
#[test]
fn story_2351_project_config_registers_hourly_hygiene_and_its_route() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.aida/config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let value: toml::Value = toml::from_str(&text).unwrap();
    let jobs = value["schedule"]["jobs"].as_array().unwrap();
    let job = |name: &str| {
        jobs.iter()
            .find(|j| j["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("{name} missing from .aida/config.toml"))
    };
    let hygiene = job("session-hygiene");
    assert_eq!(hygiene["command"].as_str(), Some("session hygiene"));
    assert_eq!(hygiene["every"].as_str(), Some("1h"));
    assert_eq!(hygiene["enabled"].as_bool(), Some(true));
    parse_scheduled_command(hygiene["command"].as_str().unwrap()).unwrap();
    let route = job("session-hygiene-route");
    assert_eq!(
        route["on"].as_array().unwrap(),
        &vec![toml::Value::from("CronJobFailed:session-hygiene")]
    );
    assert_eq!(
        route["seats"].as_array().unwrap(),
        &vec![toml::Value::from("advisor")]
    );
    assert_eq!(route["enabled"].as_bool(), Some(true));
}

// trace:STORY-2402 | ai:codex
#[test]
fn story2402_s4_resurface_is_timer_only() {
    let command = parse_scheduled_command("defer resurface").unwrap();
    assert_eq!(command.args, &["defer", "resurface"]);
    assert!(!command.hook_allowed);
}

#[test]
fn rejects_unknown_commands_with_valid_set() {
    let err = parse_scheduled_command("cache veryfy")
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown scheduled task command"));
    assert!(err.contains("cache verify"));
}

// Review follow-up: a scheduled child that never exits (a stalled network
// call inside a `doctor check` substrate job) must be killed at the
// timeout ceiling rather than left to run `run_now`/`try_tick_lock`
// indefinitely, and the outcome it produces must read as a FAILURE
// (PRIN-5: an unobserved result is never reported as ok), never as a
// silent success. Drives a real hanging `sleep` child through
// `run_with_kill_timeout` — the exact function `run_aida_command` calls
// in production — with a timeout far shorter than the sleep duration.
//
// unix-only: `sleep`/`sh` as fixtures, and the process-group kill this
// pins is itself a unix-only mechanism (see `kill_process_group` in
// lib.rs) — Windows keeps the pre-existing direct-child-only kill.
#[cfg(unix)]
#[test]
fn hanging_command_is_killed_at_the_timeout_and_reported_as_a_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = ProcessCommand::new("sleep");
    cmd.arg("30");
    let started = std::time::Instant::now();
    let outcome = run_with_kill_timeout(
        cmd,
        "sleep 30",
        tmp.path(),
        std::time::Duration::from_millis(300),
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the 30s sleep must be killed near the 300ms timeout, not waited out; took {elapsed:?}"
    );
    assert_ne!(outcome.status, 0, "a timed-out child must never read as ok");
    assert_eq!(outcome.status, 124, "the conventional timeout(1) sentinel");
    assert!(
        outcome.stderr.contains("did not complete within"),
        "{}",
        outcome.stderr
    );

    // Wire the same outcome through `failure_trip` / `record_outcome_local`
    // the way the real tick does, to pin that a timeout actually trips the
    // job (non-zero status is all `failure_trip` requires) rather than
    // merely LOOKING like a failure in isolation.
    let task = task(
        "hang-guard",
        "1h",
        "doctor check remote-drift --fail-on-findings",
    );
    let trip = failure_trip(&task, at(12), &outcome);
    assert!(trip.is_some(), "a 124 exit must mint a failure trip");
}

// Review follow-up: the GRANDCHILD case — a direct child that
// backgrounds a long-running descendant and then waits on it (`sh -c
// 'sleep 30 & wait'`, the same shape a credential-manager helper or a
// backgrounded git op takes) inherits the pipe write ends too. Killing
// only the direct child (old `Child::kill`-only behavior) would leave
// that grandchild alive, still holding stdout/stderr open, and the
// reader threads' `read_to_end` blocked on them — this is exactly the
// gap `kill_process_group`'s `killpg` closes: `sh` and `sleep` share one
// process group (`process_group(0)` at spawn), so one SIGKILL reaps
// both. Must return within about the ceiling plus slack, not anywhere
// near the 30s the grandchild alone would otherwise run.
#[cfg(unix)]
#[test]
fn hanging_grandchild_is_reaped_via_process_group_kill() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = ProcessCommand::new("sh");
    cmd.arg("-c").arg("sleep 30 & wait");
    let ceiling = std::time::Duration::from_secs(2);
    let started = std::time::Instant::now();
    let outcome = run_with_kill_timeout(cmd, "sh -c 'sleep 30 & wait'", tmp.path(), ceiling);
    let elapsed = started.elapsed();
    assert!(
        elapsed < ceiling + std::time::Duration::from_secs(3),
        "the backgrounded grandchild must be reaped with the parent via killpg, not \
             outlive it and wedge the read; ceiling {ceiling:?}, took {elapsed:?}"
    );
    assert_eq!(
        outcome.status, 124,
        "killed-on-timeout must read as failed, never ok"
    );
}

// BUG-1557: every `command = "..."` string the scaffolded config template
// ships (commented-out examples included — those are exactly the ones a
// project owner un-comments) must parse through `parse_scheduled_command`,
// or the job errors on every tick forever while sitting in config looking
// like a guard.
//
// Deliberately ONE-DIRECTIONAL: the allowlist may (and does) contain
// entries with no scaffolded example — `fetch --code-only`, `store
// compact`, `store gc` — and that is correct, not a defect. A set-equality
// check would fail today for those three false reasons and invite the
// check to be weakened or deleted. Only "scaffolded but unparseable" is a
// bug, so that is the only direction this test asserts.
//
// The extraction is scoped to `[[schedule.jobs]]` blocks specifically
// (tracking block boundaries line-by-line), NOT a blanket
// `command *= *"` grep. That is a deliberate exclusion, recorded here
// rather than discovered as a failure: the leading doc comment in
// `init_schedule_config_section` contains a literal placeholder,
// `command = "<allow-listed aida subcommand>"`, that documents the
// convention rather than declaring a job, and a naive extractor would
// pick it up and it would never parse.
// trace:BUG-1557 | ai:claude
fn scaffolded_job_commands(section: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut in_job_block = false;
    for line in section.lines() {
        let stripped = line.trim().trim_start_matches('#').trim();
        if stripped.starts_with("[[") {
            in_job_block = stripped == "[[schedule.jobs]]";
            continue;
        }
        if stripped.starts_with('[') {
            in_job_block = false;
            continue;
        }
        if !in_job_block {
            continue;
        }
        if let Some((key, value)) = stripped.split_once('=') {
            if key.trim() == "command" {
                if let Some(inner) = value
                    .trim()
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                {
                    commands.push(inner.to_string());
                }
            }
        }
    }
    commands
}

#[test]
fn scaffold_extraction_excludes_the_documentation_placeholder() {
    let section = crate::init_cmd::init_schedule_config_section();
    assert!(
        section.contains("<allow-listed aida subcommand>"),
        "the doc placeholder this test guards against moved or was removed; \
             update this test rather than deleting it silently"
    );
    let commands = scaffolded_job_commands(section);
    assert!(
        !commands.iter().any(|c| c.contains("allow-listed")),
        "extraction picked up the documentation placeholder, not a real job: {commands:?}"
    );
}

/// STORY-1689: this repository's own config registers the live-binary
/// promote job and binds its failure route to the advisor; the route's
/// `CronJobFailed:<job>` binding names a real job, so it loads enabled.
// trace:STORY-1689.ac481f1d | ai:claude
#[test]
fn repo_config_registers_dev_autopromote_and_its_advisor_route() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.aida/config.toml");
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::copy(&repo_config, tmp.path().join(".aida/config.toml")).unwrap();
    let mut config = load_config(tmp.path()).unwrap().expect("a [schedule]");
    disable_unknown_bindings(&mut config);
    let job = config
        .tasks
        .iter()
        .find(|t| t.name == "dev-autopromote")
        .expect("dev-autopromote job");
    assert!(job.enabled && job.problem.is_none());
    let command = job.command.as_ref().expect("a substrate command");
    assert_eq!(command.args, ["dev", "build", "--if-main-green"]);
    assert!(!command.hook_allowed, "it fetches; never on the hook path");
    let route = config
        .tasks
        .iter()
        .find(|t| t.name == "dev-autopromote-route")
        .expect("dev-autopromote-route");
    assert!(
        route.enabled && route.problem.is_none(),
        "{:?}",
        route.problem
    );
    assert_eq!(route.seats, vec!["advisor".to_string()]);
    assert_eq!(
        route.on.iter().map(ToString::to_string).collect::<Vec<_>>(),
        vec!["CronJobFailed:dev-autopromote".to_string()]
    );
    // TASK-2272 (TASK-2291): the route also fires for a tick that errored
    // or was killed on the scheduler's timeout, not only a failed build.
    // trace:TASK-2272 | ai:claude
    let prompt = route.prompt.as_deref().unwrap_or_default();
    assert!(prompt.contains("errored or timed out"), "{prompt}");
    assert!(
        !prompt.starts_with("The scheduled live-binary build failed"),
        "{prompt}"
    );
}

#[test]
fn every_scaffolded_job_command_parses() {
    let section = crate::init_cmd::init_schedule_config_section();
    let commands = scaffolded_job_commands(section);
    // Sanity floor so a broken extractor (e.g. one that stops matching
    // `[[schedule.jobs]]` blocks entirely) can't silently pass by finding
    // zero commands.
    assert!(
        commands.len() >= 5,
        "expected at least 5 scaffolded `command = ...` examples, found {}: {commands:?}",
        commands.len()
    );
    for command in &commands {
        parse_scheduled_command(command).unwrap_or_else(|e| {
            panic!(
                "scaffolded example `command = \"{command}\"` does not parse \
                     as an allowlisted scheduled command: {e}"
            )
        });
    }
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
    let out = tick_with_executor(
        tmp.path(),
        config(vec![
            task("due", "1h", "cache verify"),
            task("recent", "24h", "queue gc"),
        ]),
        &mut state,
        at(12),
        false,
        ok_exec(Rc::clone(&seen)),
    )
    .unwrap();
    assert_eq!(&*seen.borrow(), &["cache verify"]);
    assert_eq!(out, vec!["schedule tick: due ok"]);
    // The run is ledgered per job under the store root.
    let ledger = schedule_ledger::load(&store_root(tmp.path()), "due").unwrap();
    assert_eq!(ledger.last_run, Some(at(12)));
    assert_eq!(ledger.result.as_deref(), Some("ok"));
    assert_eq!(
        ledger.last_by.as_ref().map(|b| b.seat.as_str()),
        Some("substrate")
    );
}

// trace:TASK-1281 | ai:codex
#[test]
fn ledger_failure_does_not_lose_local_run_suppression() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(store_root(tmp.path()), "not a directory").unwrap();
    let mut state = ScheduleState::default();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let out = tick_with_executor(
        tmp.path(),
        config(vec![task("cache", "1h", "cache verify")]),
        &mut state,
        at(12),
        false,
        ok_exec(Rc::clone(&seen)),
    )
    .unwrap();
    assert_eq!(out, vec!["schedule tick: cache ok"]);
    assert_eq!(state.tasks["cache"].last_run_at, Some(at(12)));
    assert_eq!(
        load_state(tmp.path()).unwrap().tasks["cache"].last_run_at,
        Some(at(12))
    );
}

// trace:TASK-1281 | ai:codex
#[test]
fn legacy_fires_task_has_no_run_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy = Task {
        name: "legacy".into(),
        kind: JobKind::FiresTask,
        seats: vec!["*".into()],
        command: None,
        prompt: None,
        interval: Some(Duration::hours(1)),
        on: Vec::new(),
        when: None,
        when_raw: None,
        while_: None,
        while_raw: None,
        quiet_hours: None,
        enabled: true,
        source: JobSource::Project,
        problem: None,
        invalid_command: None,
    };
    let mut state = ScheduleState::default();
    let lines = run_with_executor(
        tmp.path(),
        config(vec![legacy]),
        &mut state,
        at(12),
        Some("legacy"),
        |_root, _command| unreachable!(),
    )
    .unwrap();
    assert!(lines.is_empty());
}

// trace:TASK-1281 | ai:codex
// trace:BUG-1595 | ai:claude
#[test]
fn tick_lock_is_nonblocking_and_reusable() {
    let tmp = tempfile::tempdir().unwrap();
    let first = try_tick_lock(tmp.path()).unwrap().unwrap();
    assert!(try_tick_lock(tmp.path()).unwrap().is_none());
    drop(first);

    // BUG-1595 (recurrence of BUG-1303): re-locking right after drop can
    // still observe contention under `--test-threads` parallelism, even
    // though this process's own fd for the lock file is closed.
    //
    // Root cause, not a test artifact: `try_tick_lock` takes an
    // fs2 `flock`(2)-style advisory lock, which is owned by the OPEN
    // FILE DESCRIPTION, not by this process's fd. `std::fs::File`
    // already opens with `O_CLOEXEC` (confirmed via strace: the
    // `openat` call carries `O_CLOEXEC`), so that is not the gap.
    // The gap is that `O_CLOEXEC` only takes effect at `execve()` — it
    // does nothing about `fork()` itself. `aida-cli-lib`'s test binary
    // shells out to git constantly (`std::process::Command`, 700+
    // call sites), and every such spawn forks the *whole* process,
    // which briefly duplicates every open fd — including this test's
    // lock fd, if it happens to be open at that instant — into the
    // child. For the short fork()..execve() window the child holds
    // its own reference to the same open file description, so the
    // advisory lock stays held even after `drop(first)` closes our
    // fd. Switching to Linux OFD locks (`fcntl(F_OFD_SETLK)`) would
    // not close this gap either: POSIX defines OFD locks as
    // inherited across `fork()` exactly like `flock()` (a copy of
    // the fd from `fork()` refers to the same open file description
    // and shares its OFD lock). So this window is not a bug in the
    // lock; it is real, unavoidable, and cannot be raced out of the
    // implementation by picking a different lock primitive. The
    // bounded retry below is what BUG-1595 calls for in that case.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let reacquired = loop {
        match try_tick_lock(tmp.path()).unwrap() {
            Some(guard) => break Some(guard),
            None if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            None => break None,
        }
    };
    assert!(
        reacquired.is_some(),
        "lock was not reacquired within 2s of dropping the first guard"
    );
}

// A chatty tick advances the store once, regardless of job count.
// trace:TASK-1280 | ai:codex
#[test]
fn tick_batches_three_job_ledgers_into_one_store_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_root(tmp.path());
    std::fs::create_dir_all(&store).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&store)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "AIDA Test"]);
    git(&["config", "user.email", "aida@example.invalid"]);

    let mut state = ScheduleState::default();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let out = tick_with_executor(
        tmp.path(),
        config(vec![
            task("one", "1h", "cache verify"),
            task("two", "1h", "queue gc"),
            task("three", "1h", "session reap"),
        ]),
        &mut state,
        at(12),
        false,
        ok_exec(Rc::clone(&seen)),
    )
    .unwrap();

    assert_eq!(out.len(), 3);
    assert_eq!(git(&["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(schedule_ledger::load_all(&store).len(), 3);
}

// trace:STORY-B6B1278890774EF993D2EBC327862470-097.ace81ddf | ai:claude
#[test]
fn bridge_job_reconciles_without_yes_and_routes_failures_to_advisor() {
    let tasks = bridge_tasks(None).unwrap();
    let job = &tasks[0];
    assert_eq!(job.name, crate::store_local::BRIDGE_JOB);
    assert_eq!(job.kind, JobKind::Substrate);
    assert_eq!(job.source, JobSource::Local);
    assert_eq!(job.interval, Some(Duration::hours(1)));
    assert!(job.enabled);
    let argv = job.command.as_ref().unwrap().args;
    assert_eq!(argv, ["remote", "reconcile", "--execute"]);
    assert!(
        !argv.contains(&"--yes"),
        "identity content must stop the run"
    );
    assert!(!job.command.as_ref().unwrap().hook_allowed);

    let route = &tasks[1];
    assert_eq!(route.name, crate::store_local::BRIDGE_ROUTE_JOB);
    assert_eq!(route.kind, JobKind::Seat);
    assert!(route.applies_to_seat("advisor"));
    assert_eq!(
        route.on,
        vec![EventTrigger {
            kind: "CronJobFailed".to_string(),
            job: Some(crate::store_local::BRIDGE_JOB.to_string()),
        }]
    );

    let slower = bridge_tasks(Some("6h")).unwrap();
    assert_eq!(slower[0].interval, Some(Duration::hours(6)));
    assert!(bridge_tasks(Some("soon")).is_err());
}

// trace:STORY-B6B1278890774EF993D2EBC327862470-097.acc25d32 | ai:claude
#[test]
fn tracked_reconcile_job_never_runs() {
    let cfg = parse_project(
        r#"
[schedule]
[[schedule.jobs]]
name = "reconcile-everywhere"
command = "remote reconcile --execute"
every = "1h"
enabled = true
"#,
    )
    .unwrap()
    .unwrap();
    let job = &cfg.tasks[0];
    assert!(
        job.command.is_none(),
        "a tracked job must not get a runnable command"
    );
    let invalid = job.invalid_command.as_ref().unwrap();
    assert!(invalid.error.contains("hub bridge"), "{}", invalid.error);
    // The machine-global layer applies to every clone too.
    let rebuilt = build_task(
        JobConfig {
            name: "g".into(),
            command: Some("remote reconcile --execute".into()),
            prompt: None,
            interval: Some("1h".into()),
            on: Vec::new(),
            when: None,
            while_: None,
            quiet_hours: None,
            enabled: true,
            seats: Vec::new(),
            kind: None,
        },
        JobSource::Global,
    )
    .unwrap();
    assert!(rebuilt.command.is_none() && rebuilt.invalid_command.is_some());
}

// trace:STORY-B6B1278890774EF993D2EBC327862470-097.acc25d32 | ai:claude
// trace:STORY-B6B1278890774EF993D2EBC327862470-097.ac3ff691 | ai:claude
#[test]
fn only_a_clone_marked_bridge_gets_the_bridge_jobs() {
    let home = tempfile::tempdir().unwrap();
    let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path());
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".aida")).unwrap();
    std::fs::write(
        project.path().join(".aida/config.toml"),
        r#"
[schedule]
[[schedule.jobs]]
name = "hub-drift-guard"
command = "doctor check remote-drift --fail-on-findings"
every = "6h"
enabled = true
"#,
    )
    .unwrap();
    let names = |root: &Path| -> Vec<String> {
        load_registry(root)
            .unwrap()
            .unwrap()
            .tasks
            .iter()
            .map(|t| t.name.clone())
            .collect()
    };
    assert_eq!(names(project.path()), vec!["hub-drift-guard"]);

    let layer = home
        .path()
        .join(".aida")
        .join(crate::store_local::LOCAL_LAYER_FILE);
    let key = crate::store_local::clone_key(project.path());
    crate::store_local::write_bridge(&layer, &key, true).unwrap();
    let registry = load_registry(project.path()).unwrap().unwrap();
    let got: Vec<&str> = registry.tasks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        got,
        vec![
            "hub-drift-guard",
            crate::store_local::BRIDGE_JOB,
            crate::store_local::BRIDGE_ROUTE_JOB
        ]
    );
    // The drift guard stays the escalation for what the bridge cannot union.
    assert!(registry.tasks[0].enabled && registry.tasks[0].problem.is_none());
    assert!(registry.tasks.iter().all(|t| t.enabled));

    // Another clone on the same machine is not the bridge.
    let other = tempfile::tempdir().unwrap();
    assert!(load_registry(other.path())
        .unwrap()
        .is_none_or(|c| c.tasks.is_empty()));
}

/// The install path's stopgap retirement removes the marker line on the
/// bridge clone and leaves the crontab alone on any other clone.
// trace:STORY-B6B1278890774EF993D2EBC327862470-097.ac9e5341 | ai:claude
#[test]
fn install_retires_the_stopgap_cron_only_on_the_bridge_clone() {
    let home = tempfile::tempdir().unwrap();
    let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path());
    let bridge = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let layer = home
        .path()
        .join(".aida")
        .join(crate::store_local::LOCAL_LAYER_FILE);
    crate::store_local::write_bridge(&layer, &crate::store_local::clone_key(bridge.path()), true)
        .unwrap();
    let stopgap = "# aida-store-reconcile\n\
                       0 * * * * flock -n ~/.aida/store-reconcile.lock aida remote reconcile --execute\n\
                       30 2 * * * backup.sh\n";
    let crontab = std::cell::RefCell::new(stopgap.to_string());
    let remove = || -> Result<bool> {
        let next = crate::store_local::crontab_without_stopgap(&crontab.borrow());
        Ok(next.map(|body| *crontab.borrow_mut() = body).is_some())
    };

    retire_stopgap_reconcile_cron(other.path(), remove);
    assert_eq!(
        *crontab.borrow(),
        stopgap,
        "a non-bridge clone keeps the stopgap"
    );

    retire_stopgap_reconcile_cron(bridge.path(), remove);
    assert_eq!(*crontab.borrow(), "30 2 * * * backup.sh\n");
}

// trace:TASK-1532 | ai:agy
#[test]
fn min_gap_suppresses_recent_tick() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", tmp.path());
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
    let lines = tick(tmp.path(), false, None).unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("suppressed by min-gap"));
}

// BUG-1600: `~/.aida/schedule-tick.log` must never grow without bound —
// a stale/misconfigured cron entry that fails identically every 15
// minutes must not flood it forever.
// trace:BUG-1600 | ai:claude
#[test]
fn bound_global_schedule_log_leaves_small_file_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", tmp.path());
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let log = tmp.path().join(".aida").join("schedule-tick.log");
    std::fs::write(&log, "small content\n").unwrap();

    bound_global_schedule_log();

    let body = std::fs::read_to_string(&log).unwrap();
    assert_eq!(body, "small content\n", "well under the cap → untouched");
}

#[test]
fn bound_global_schedule_log_truncates_when_over_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", tmp.path());
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let log = tmp.path().join(".aida").join("schedule-tick.log");
    // The exact repeated-failure shape BUG-1600 produced: the same
    // error line, over and over, forever.
    let line = "error: `aida schedule tick --format json` is unsupported\n";
    let repeats = (GLOBAL_SCHEDULE_LOG_MAX_BYTES as usize / line.len()) + 100;
    let big = line.repeat(repeats);
    std::fs::write(&log, &big).unwrap();
    let before_len = std::fs::metadata(&log).unwrap().len();
    assert!(
        before_len > GLOBAL_SCHEDULE_LOG_MAX_BYTES,
        "test setup sanity"
    );

    bound_global_schedule_log();

    let after = std::fs::read_to_string(&log).unwrap();
    assert!(
        (after.len() as u64) <= GLOBAL_SCHEDULE_LOG_MAX_BYTES,
        "must be at/under the cap after truncation: {} bytes",
        after.len()
    );
    assert!(
        after.contains("truncated"),
        "must leave evidence that truncation happened: {after}"
    );
    // The newest content (the tail of the repeated line) must survive —
    // truncation drops the OLDEST bytes, not the newest.
    assert!(
        after.trim_end().ends_with(line.trim_end()),
        "must keep the newest lines: {after}"
    );
}

#[test]
fn tick_bounds_global_schedule_log_for_timer_but_not_hook_invocation() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", tmp.path());
    // trace:BUG-1804 | ai:codex
    // The log is already test-local. Use separate project locks: a parallel
    // subprocess spawn can retain the hook's flock briefly after it returns
    // (see tick_lock_is_nonblocking_and_reusable), making a timer tick on the
    // same root skip housekeeping. Both projects still share this temp home.
    let hook_root = tmp.path().join("hook-project");
    let timer_root = tmp.path().join("timer-project");
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let log = tmp.path().join(".aida").join("schedule-tick.log");
    assert_eq!(global_schedule_log_path().as_deref(), Some(log.as_path()));
    let line = "error: unsupported\n";
    let big = line.repeat((GLOBAL_SCHEDULE_LOG_MAX_BYTES as usize / line.len()) + 100);
    let big_len = big.len() as u64;

    // A HOOK tick (the per-turn invoker) leaves the log alone — it's
    // meant to stay minimal, and the installed hook script redirects
    // its own output to /dev/null anyway.
    std::fs::write(&log, &big).unwrap();
    let hook_lines = tick(&hook_root, true, None).unwrap();
    assert!(hook_lines.is_empty(), "hook tick must run: {hook_lines:?}");
    let after_hook = std::fs::metadata(&log).unwrap().len();
    assert_eq!(
        after_hook, big_len,
        "a hook tick must not touch the global log"
    );

    // A timer/cron-shaped tick (hook = false — the shape the installed
    // crontab entry uses) bounds it.
    let timer_lines = tick(&timer_root, false, None).unwrap();
    assert!(
        timer_lines.is_empty(),
        "timer tick must run: {timer_lines:?}"
    );
    let after_timer = std::fs::metadata(&log).unwrap().len();
    assert!(
        after_timer <= GLOBAL_SCHEDULE_LOG_MAX_BYTES,
        "a non-hook (timer/cron) tick must bound the log: {after_timer} bytes"
    );
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
    let ledger = schedule_ledger::load(&store_root(tmp.path()), "bad").unwrap();
    assert_eq!(ledger.result.as_deref(), Some("failed:2"));
}

fn performance_json(denominator: usize) -> String {
    serde_json::json!({
            "total": 1,
            "findings": [],
            "performance_audits": [{
                "command": "show",
                "budget_ms": 1000,
                "over_budget": if denominator == 0 { 0 } else { 19 },
                "denominator": denominator,
                "proportion_millipercent": if denominator == 0 { 0 } else { 19700 },
                "tolerated_millipercent": 10000,
                "window_hours": 24,
                "worst_ms": if denominator == 0 { serde_json::Value::Null } else { serde_json::json!(165672) },
                "excluded_samples": 3,
                "lineage_scoped": true
            }]
        })
        .to_string()
}

// trace:BUG-1573 | ai:codex
#[test]
fn performance_failure_is_auditable_and_routed_by_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = ScheduleState::default();
    tick_with_executor(
        tmp.path(),
        config(vec![task(
            "performance-guard",
            "1h",
            "doctor check performance --fail-on-findings",
        )]),
        &mut state,
        at(12),
        false,
        |_root, _cmd| {
            Ok(TaskOutcome {
                status: 1,
                stdout: performance_json(96),
                stderr: "format hint that must not replace evidence".into(),
            })
        },
    )
    .unwrap();

    let ledger = schedule_ledger::load(&store_root(tmp.path()), "performance-guard").unwrap();
    let trip = ledger.failure_trips.last().unwrap();
    let audit = &trip.performance[0];
    assert_eq!(
        (audit.proportion_millipercent, audit.denominator),
        (19_700, 96)
    );
    assert_eq!(
        (audit.budget_ms, audit.tolerated_millipercent),
        (1000, 10_000)
    );
    assert_eq!(audit.worst_ms, Some(165_672));

    let events = events::read_all(tmp.path());
    let routed = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::CronJobFailed {
                trip_id,
                performance,
                ..
            } => Some((trip_id.as_deref(), performance)),
            _ => None,
        })
        .unwrap();
    assert_eq!(routed.0, Some(trip.trip_id.as_str()));
    assert_eq!(routed.1, &trip.performance);

    // Consumer-side proof: the routed event makes the advisor job due,
    // and carries enough evidence to perform its prompt without rerunning.
    let mut route_state = ScheduleState::default();
    route_state.tasks.insert(
        "performance-guard-route".into(),
        TaskState {
            last_seen_event_ts: Some(at(11)),
            ..Default::default()
        },
    );
    tick_core(
        tmp.path(),
        config(vec![seat_task(
            "performance-guard-route",
            &["advisor"],
            None,
            &["CronJobFailed"],
        )]),
        &mut route_state,
        at(13),
        false,
        |_root, _cmd| unreachable!(),
        |_| Snapshot::default(),
        &events,
    )
    .unwrap();
    let route = schedule_ledger::load(&store_root(tmp.path()), "performance-guard-route").unwrap();
    assert!(route.due_since.is_some());
    assert_eq!(
        route.due_failure.as_ref().map(|f| f.trip_id.as_str()),
        Some(trip.trip_id.as_str())
    );

    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        r#"
[[schedule.jobs]]
name = "performance-guard-route"
seats = ["advisor"]
on = ["CronJobFailed"]
prompt = "decide whether this is a regression"
enabled = true
"#,
    )
    .unwrap();
    let delivered = due_seat_jobs(tmp.path(), Some("advisor"));
    let artifact = render_due_jobs_block(&delivered, "advisor");
    assert!(artifact.contains(&trip.trip_id), "{artifact}");
    assert!(artifact.contains("19.700% over 1000 ms"), "{artifact}");
    assert!(artifact.contains("19 of 96 calls"), "{artifact}");
    assert!(artifact.contains("tolerance 10.000%"), "{artifact}");
    assert!(artifact.contains("worst 165672 ms"), "{artifact}");
    assert!(artifact.contains("lineage_scoped=true"), "{artifact}");

    let log = std::fs::read_to_string(log_path(tmp.path())).unwrap();
    assert!(log.contains("19.700%/96,budget=1000ms,tolerance=10.000%"));
    assert!(!log.contains(&performance_json(96)));
}

// trace:BUG-1573 | ai:codex
#[test]
fn performance_pass_has_no_failure_context_and_zero_denominator_is_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let perf = task(
        "performance-guard",
        "1h",
        "doctor check performance --fail-on-findings",
    );
    let mut state = ScheduleState::default();
    run_with_executor(
        tmp.path(),
        config(vec![perf.clone()]),
        &mut state,
        at(12),
        Some("performance-guard"),
        |_root, _cmd| {
            Ok(TaskOutcome {
                status: 0,
                stdout: "not parsed on success".into(),
                stderr: String::new(),
            })
        },
    )
    .unwrap();
    assert!(
        schedule_ledger::load(&store_root(tmp.path()), "performance-guard")
            .unwrap()
            .failure_trips
            .is_empty()
    );

    run_with_executor(
        tmp.path(),
        config(vec![perf]),
        &mut state,
        at(13),
        Some("performance-guard"),
        |_root, _cmd| {
            Ok(TaskOutcome {
                status: 1,
                stdout: performance_json(0),
                stderr: String::new(),
            })
        },
    )
    .unwrap();
    let ledger = schedule_ledger::load(&store_root(tmp.path()), "performance-guard").unwrap();
    let audit = &ledger.failure_trips[0].performance[0];
    assert_eq!(
        (
            audit.denominator,
            audit.proportion_millipercent,
            audit.worst_ms
        ),
        (0, 0, None)
    );
}

// trace:TASK-2347 | ai:codex — unchanged failures remain one due seat job;
// successful health recovery is visible in the existing substrate ledger.
#[test]
fn task_2347_health_due_route_deduplicates_and_recovery_is_visible() {
    let tmp = tempfile::tempdir().unwrap();
    let guard = task(
        "mail-health",
        "15m",
        "doctor check codex-mail-wake --fail-on-findings",
    );
    let mut state = ScheduleState::default();
    run_with_executor(tmp.path(), config(vec![guard.clone()]), &mut state, at(13), Some("mail-health"),
            |_, command| {
                assert_eq!(command.args, ["doctor", "check", "codex-mail-wake", "--json", "--fail-on-findings"]);
                Ok(TaskOutcome { status: 1, stdout: String::new(), stderr: "accepted_unacknowledged_count=1 oldest_known_notification_age_seconds=86400 recovery_required=false".into() })
            }).unwrap();
    let events = events::read_all(tmp.path());
    let route = seat_task(
        "mail-health-route",
        &["advisor"],
        None,
        &["CronJobFailed:mail-health"],
    );
    let mut route_state = ScheduleState::default();
    route_state.tasks.insert(
        route.name.clone(),
        TaskState {
            last_seen_event_ts: Some(at(12)),
            ..Default::default()
        },
    );
    for hour in [14, 15] {
        tick_core(
            tmp.path(),
            config(vec![route.clone()]),
            &mut route_state,
            at(hour),
            false,
            |_, _| unreachable!(),
            |_| Snapshot::default(),
            &events,
        )
        .unwrap();
    }
    let routed = schedule_ledger::load(&store_root(tmp.path()), "mail-health-route").unwrap();
    assert!(routed.due_since.is_some());
    assert!(routed.due_failure.is_some());
    let fired = events::read_all(tmp.path()).iter().filter(|e| matches!(&e.kind, EventKind::CronJobFired { job, .. } if job == "mail-health-route")).count();
    assert_eq!(fired, 1);
    run_with_executor(
        tmp.path(),
        config(vec![guard]),
        &mut state,
        at(16),
        Some("mail-health"),
        |_, _| {
            Ok(TaskOutcome {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        },
    )
    .unwrap();
    let recovered = schedule_ledger::load(&store_root(tmp.path()), "mail-health").unwrap();
    assert_eq!(recovered.result.as_deref(), Some("ok"));
}

// STORY-1367: the three first substrate jobs the story registers
// (hub-drift, stranded-branches, disk-headroom) all ride the same
// GATING `doctor check <category> --fail-on-findings` shape as
// performance-guard, with no job-specific evidence parsing — a plain
// non-zero exit is enough for `failure_trip` to mint a trip_id and for
// the routing seat job to pick it up. Drives the tick against a fixture
// where the check trips (asserts each job reports, once, through the
// existing CronJobFailed → due-seat-job surface) and a clean fixture
// (asserts silence): acceptance criteria 2, 3 and 5.
// trace:STORY-1367 | ai:claude
#[test]
fn each_new_guard_job_trips_and_routes_on_failure_and_is_silent_when_clean() {
    for command in [
        "doctor check remote-drift --fail-on-findings",
        "doctor check stale-remote-branches --fail-on-findings",
        "doctor check disk-headroom --fail-on-findings",
        "doctor check runaway-seats --fail-on-findings",
        // trace:STORY-1699 | ai:claude
        "doctor check relay-stall --fail-on-findings",
        // trace:TASK-2347 | ai:codex
        "doctor check codex-mail-wake --fail-on-findings",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let job_name = "guard";
        let guard = task(job_name, "6h", command);

        // Clean fixture: the check finds nothing, exits 0. Silent — no
        // CronJobFailed event, no failure trip.
        let mut state = ScheduleState::default();
        run_with_executor(
            tmp.path(),
            config(vec![guard.clone()]),
            &mut state,
            at(12),
            Some(job_name),
            |_root, _cmd| {
                Ok(TaskOutcome {
                    status: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            },
        )
        .unwrap();
        assert!(
            schedule_ledger::load(&store_root(tmp.path()), job_name)
                .unwrap()
                .failure_trips
                .is_empty(),
            "{command}: clean run must not trip"
        );
        assert!(
            events::read_all(tmp.path()).is_empty(),
            "{command}: clean run must emit nothing"
        );

        // Failing fixture: the check finds something, exits non-zero.
        // Reports exactly once, through CronJobFailed, and a routing seat
        // job on that event becomes due carrying the trip evidence.
        run_with_executor(
            tmp.path(),
            config(vec![guard]),
            &mut state,
            at(13),
            Some(job_name),
            |_root, _cmd| {
                Ok(TaskOutcome {
                    status: 1,
                    stdout: String::new(),
                    stderr: "finding(s) detected".into(),
                })
            },
        )
        .unwrap();
        let ledger = schedule_ledger::load(&store_root(tmp.path()), job_name).unwrap();
        let trip = ledger
            .failure_trips
            .last()
            .unwrap_or_else(|| panic!("{command}: failing run must trip"));

        let events = events::read_all(tmp.path());
        let routed = events
            .iter()
            .find_map(|event| match &event.kind {
                EventKind::CronJobFailed { trip_id, .. } => trip_id.as_deref(),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{command}: failure must emit CronJobFailed"));
        assert_eq!(routed, trip.trip_id, "{command}");

        // A cold cursor (no prior tick) seeds itself to "now" and skips
        // event replay on its first tick (see the STORY-1226 comment in
        // `tick_core`), so seed a cursor before the failing run's
        // timestamp — same setup `performance_failure_is_auditable_and_routed_by_trip`
        // uses — or the routing job would never see the event it exists
        // to route.
        let mut route_state = ScheduleState::default();
        route_state.tasks.insert(
            "guard-route".into(),
            TaskState {
                last_seen_event_ts: Some(at(12)),
                ..Default::default()
            },
        );
        tick_core(
            tmp.path(),
            config(vec![seat_task(
                "guard-route",
                &["advisor"],
                None,
                &["CronJobFailed"],
            )]),
            &mut route_state,
            at(14),
            false,
            |_root, _cmd| unreachable!(),
            |_| Snapshot::default(),
            &events,
        )
        .unwrap();
        let route = schedule_ledger::load(&store_root(tmp.path()), "guard-route").unwrap();
        assert!(route.due_since.is_some(), "{command}: routing job not due");
        assert_eq!(
            route.due_failure.as_ref().map(|f| f.trip_id.as_str()),
            Some(trip.trip_id.as_str()),
            "{command}"
        );
    }
}

// trace:BUG-1573 | ai:codex
#[test]
fn malformed_performance_output_fails_visibly_without_persisting_payload() {
    let task = task(
        "performance-guard",
        "1h",
        "doctor check performance --fail-on-findings",
    );
    let outcome = TaskOutcome {
        status: 1,
        stdout: "SECRET noisy payload".into(),
        stderr: String::new(),
    };
    let trip = failure_trip(&task, at(12), &outcome).unwrap();
    assert!(trip
        .audit_error
        .as_deref()
        .unwrap()
        .contains("malformed typed doctor output"));
    assert!(!serde_yaml::to_string(&trip).unwrap().contains("SECRET"));
}

// trace:BUG-1573 | ai:codex
#[test]
fn failure_history_has_a_deterministic_oldest_first_bound() {
    let task = task("bad", "1h", "cache verify");
    let outcome = TaskOutcome {
        status: 2,
        stdout: String::new(),
        stderr: "boom".into(),
    };
    let mut ledger = JobLedger::new("bad");
    for hour in 0..=schedule_ledger::MAX_FAILURE_TRIPS as u32 {
        let now = at(hour);
        let trip = failure_trip(&task, now, &outcome).unwrap();
        apply_outcome_ledger(&mut ledger, now, &outcome, Some(&trip));
    }
    assert_eq!(
        ledger.failure_trips.len(),
        schedule_ledger::MAX_FAILURE_TRIPS
    );
    assert_eq!(ledger.failure_trips.first().unwrap().at, at(1));
    assert_eq!(ledger.failure_trips.last().unwrap().at, at(20));
}

// BUG-1961: the tick persists through the batch writer, whose three-way
// merge dropped failure_trips (and the route job's due_failure) whenever
// the ledger file already existed. Two failing ticks must leave two trips,
// and a re-fired route job must name the NEW trip. trace:BUG-1961 | ai:claude
#[test]
fn two_failing_ticks_append_two_trips_and_route_names_the_newest() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_root(tmp.path());
    let guard = || {
        config(vec![task(
            "performance-guard",
            "1h",
            "doctor check performance --fail-on-findings",
        )])
    };
    let failing = |_root: &Path, _cmd: &ScheduledCommand| {
        Ok(TaskOutcome {
            status: 1,
            stdout: performance_json(96),
            stderr: String::new(),
        })
    };
    let route_cfg = || {
        config(vec![seat_task(
            "performance-guard-route",
            &["advisor"],
            None,
            &["CronJobFailed"],
        )])
    };
    let mut state = ScheduleState::default();
    let mut route_state = ScheduleState::default();
    route_state.tasks.insert(
        "performance-guard-route".into(),
        TaskState {
            last_seen_event_ts: Some(at(11)),
            ..Default::default()
        },
    );

    let mut trip_ids = Vec::new();
    for (n, hour) in [12u32, 14].into_iter().enumerate() {
        tick_with_executor(tmp.path(), guard(), &mut state, at(hour), false, failing).unwrap();
        let ledger = schedule_ledger::load(&store, "performance-guard").unwrap();
        assert_eq!(ledger.result.as_deref(), Some("failed:1"));
        assert_eq!(ledger.last_run, Some(at(hour)));
        assert_eq!(ledger.failure_trips.len(), n + 1, "{ledger:?}");
        let trip = ledger.failure_trips.last().unwrap();
        assert_eq!(trip.at, at(hour));
        assert_eq!(trip.performance[0].denominator, 96);
        trip_ids.push(trip.trip_id.clone());

        let events = events::read_all(tmp.path());
        tick_core(
            tmp.path(),
            route_cfg(),
            &mut route_state,
            at(hour) + chrono::Duration::minutes(5),
            false,
            |_root, _cmd| unreachable!(),
            |_| Snapshot::default(),
            &events,
        )
        .unwrap();
        let route = schedule_ledger::load(&store, "performance-guard-route").unwrap();
        assert_eq!(
            route.due_failure.as_ref().map(|f| f.trip_id.as_str()),
            Some(trip.trip_id.as_str()),
            "route job must point at the trip it was raised for"
        );
    }
    assert_ne!(trip_ids[0], trip_ids[1]);
    let ledger = schedule_ledger::load(&store, "performance-guard").unwrap();
    let ids: Vec<_> = ledger
        .failure_trips
        .iter()
        .map(|t| t.trip_id.clone())
        .collect();
    assert_eq!(ids, trip_ids);
}

#[test]
fn emit_cron_uses_valid_crontab_prefixes() {
    assert_eq!(cron_interval(parse_duration("1h").unwrap()), "0 * * * *");
    assert_eq!(cron_interval(parse_duration("24h").unwrap()), "0 3 * * *");
    assert_eq!(cron_interval(parse_duration("7d").unwrap()), "0 3 * * 0");
}

// trace:STORY-1226 | ai:claude
#[test]
fn legacy_task_config_still_parses() {
    // The STORY-1047 shape, verbatim: `interval`, `command`, `enabled`,
    // `quiet-hours`, under `[[schedule.tasks]]`.
    let cfg = parse_project(
        r#"
[schedule]
min_gap = "90s"

[[schedule.tasks]]
name = "cache-verify"
command = "cache verify"
interval = "24h"
quiet-hours = "22:00-06:00"
enabled = false
"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(cfg.min_gap, "90s");
    assert_eq!(cfg.tasks.len(), 1);
    let t = &cfg.tasks[0];
    assert_eq!(t.kind, JobKind::Substrate);
    assert_eq!(t.seats, vec!["*".to_string()]);
    assert_eq!(t.interval, Some(Duration::hours(24)));
    assert!(t.quiet_hours.is_some());
    assert!(!t.enabled);
    assert_eq!(t.schedule_summary(), "every 1d");
}

// trace:STORY-1226 | ai:claude
#[test]
fn new_job_shape_parses_and_validates() {
    let cfg = parse_project(
        r#"
[schedule]

[[schedule.jobs]]
name = "mailbox-triage"
seats = ["advisor"]
every = "30m"
on = ["MailReceived"]
prompt = "triage the mailbox"
enabled = true

[[schedule.jobs]]
name = "mailbox-latency"
when = "mail.oldest_unread_age > 15m"
command = "notify check"
enabled = true
"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(cfg.tasks.len(), 2);
    let triage = &cfg.tasks[0];
    assert_eq!(triage.kind, JobKind::Seat);
    assert!(triage.applies_to_seat("advisor"));
    assert!(triage.applies_to_seat("dialog"), "legacy alias folds");
    assert!(!triage.applies_to_seat("product"));
    assert_eq!(triage.schedule_summary(), "every 30m + on MailReceived");
    let latency = &cfg.tasks[1];
    assert_eq!(latency.kind, JobKind::Substrate);
    assert!(latency.when.is_some());

    // Validation errors name the job and the fix.
    let err = parse_project(
        r#"
[schedule]
[[schedule.jobs]]
name = "x"
prompt = "p"
on = ["PrMergd"]
"#,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("unknown event 'PrMergd'"), "{err}");
    let err = parse_project(
        r#"
[schedule]
[[schedule.jobs]]
name = "x"
prompt = "p"
"#,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("needs a schedule"), "{err}");
    let err = parse_project(
        r#"
[schedule]
[[schedule.jobs]]
name = "x"
every = "1h"
"#,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("needs a `command`"), "{err}");
}

// trace:STORY-1226 | ai:claude
#[test]
fn merge_registries_project_wins() {
    let project = config(vec![
        task("session-reap", "30m", "session reap"),
        seat_task("mailbox-triage", &["advisor"], Some("30m"), &[]),
    ]);
    let mut global_reap = task("session-reap", "6h", "session reap");
    global_reap.source = JobSource::Global;
    let mut global_doctor = task("doctor", "6h", "doctor");
    global_doctor.source = JobSource::Global;
    let global = LoadedScheduleConfig {
        min_gap: "5m".to_string(),
        tasks: vec![global_reap, global_doctor],
    };
    let merged = merge_registries(Some(project), Some(global)).unwrap();
    assert_eq!(merged.min_gap, DEFAULT_MIN_GAP, "project min_gap wins");
    let names: Vec<&str> = merged.tasks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["session-reap", "mailbox-triage", "doctor"]);
    let reap = merged
        .tasks
        .iter()
        .find(|t| t.name == "session-reap")
        .unwrap();
    assert_eq!(
        reap.interval,
        Some(Duration::minutes(30)),
        "project entry kept"
    );
    assert_eq!(reap.source, JobSource::Project);
    assert_eq!(
        merged
            .tasks
            .iter()
            .find(|t| t.name == "doctor")
            .unwrap()
            .source,
        JobSource::Global
    );
    // One-sided layers pass through; none → none.
    assert!(merge_registries(None, None).is_none());
    assert_eq!(
        merge_registries(None, Some(config(vec![task("a", "1h", "queue gc")])))
            .unwrap()
            .tasks
            .len(),
        1
    );
}

// trace:STORY-1226 | ai:claude
#[test]
fn seat_job_never_executes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = ScheduleState::default();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let out = tick_with_executor(
        tmp.path(),
        config(vec![seat_task(
            "mailbox-triage",
            &["advisor"],
            Some("30m"),
            &[],
        )]),
        &mut state,
        at(12),
        false,
        ok_exec(Rc::clone(&seen)),
    )
    .unwrap();
    assert!(
        seen.borrow().is_empty(),
        "executor must never see a seat job"
    );
    assert_eq!(
        out,
        vec!["schedule tick: mailbox-triage due (every 30m) → seat advisor"]
    );
    let store = store_root(tmp.path());
    let ledger = schedule_ledger::load(&store, "mailbox-triage").unwrap();
    assert_eq!(ledger.due_since, Some(at(12)));
    assert_eq!(ledger.due_reason.as_deref(), Some("every 30m"));
    assert!(ledger.last_run.is_none(), "due ≠ ran");
    // The event stream carries the nudge.
    let events = events::read_all(tmp.path());
    assert!(events.iter().any(|e| matches!(
        &e.kind,
        EventKind::CronJobFired { job, seat } if job == "mailbox-triage" && seat == "advisor"
    )));
    // A second tick does not re-flag (still due, not re-nudged).
    let out = tick_with_executor(
        tmp.path(),
        config(vec![seat_task(
            "mailbox-triage",
            &["advisor"],
            Some("30m"),
            &[],
        )]),
        &mut state,
        at(13),
        false,
        ok_exec(Rc::clone(&seen)),
    )
    .unwrap();
    assert!(out.is_empty());
    // `done` clears it and ledgers the reporter.
    schedule_ledger::write_cas(&store, "mailbox-triage", |l| {
        l.last_run = Some(at(13));
        l.due_since = None;
        l.due_reason = None;
        l.result = Some("done".into());
    })
    .unwrap();
    let ledger = schedule_ledger::load(&store, "mailbox-triage").unwrap();
    assert!(ledger.due_since.is_none());
    let t = seat_task("mailbox-triage", &["advisor"], Some("30m"), &[]);
    assert!(due_reason(&t, Some(&ledger), None, at(13)).is_none());
    assert_eq!(
        due_reason(&t, Some(&ledger), None, at(14)).as_deref(),
        Some("every 30m")
    );
}

// trace:STORY-1226 | ai:claude
// Advisor review round 1 (#1946). trace:STORY-1226 | ai:claude
#[test]
fn on_job_first_sight_initialises_cursor_to_now_without_replaying_history() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = ScheduleState::default();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mk = |ts: DateTime<Utc>, kind: EventKind| Event {
        ts,
        spec: None,
        run_uuid: String::new(),
        seat: None,
        kind,
    };
    // A week of historical merges before the job ever existed.
    let history = vec![
        mk(at(1), EventKind::PrMerged { pr: 1 }),
        mk(at(5), EventKind::PrMerged { pr: 2 }),
    ];
    let mut job = task("capture-sweep", "24h", "queue gc");
    job.interval = None;
    job.on = vec![EventTrigger::parse("t", "PrMerged").unwrap()];
    let run = |state: &mut ScheduleState, now, events: &[Event]| {
        tick_core(
            tmp.path(),
            config(vec![job.clone()]),
            state,
            now,
            false,
            ok_exec(Rc::clone(&seen)),
            |_| Snapshot::default(),
            events,
        )
        .unwrap()
    };
    // First sight: nothing fires, cursor = now.
    let out = run(&mut state, at(10), &history);
    assert!(out.is_empty(), "{out:?}");
    assert!(seen.borrow().is_empty());
    assert_eq!(
        state.tasks["capture-sweep"].last_seen_event_ts,
        Some(at(10))
    );
    // An event after the cursor fires once.
    let mut later = history.clone();
    later.push(mk(at(11), EventKind::PrMerged { pr: 3 }));
    let out = run(&mut state, at(12), &later);
    assert_eq!(out, vec!["schedule tick: capture-sweep ok"]);
    assert_eq!(
        state.tasks["capture-sweep"].last_seen_event_ts,
        Some(at(11))
    );
}

// Advisor review round 1 (#1946). trace:STORY-1226 | ai:claude
#[test]
fn load_global_config_surfaces_a_type_error_inside_the_schedule_table() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".aida");
    std::fs::create_dir_all(&dir).unwrap();
    // `name` must be a string: a typed error INSIDE [schedule] must not be
    // swallowed by the bare-shape fallback.
    std::fs::write(
        dir.join(GLOBAL_SCHEDULE_FILE),
        "[schedule]\n[[schedule.jobs]]\nname = 5\nevery = \"30m\"\ncommand = \"doctor\"\n",
    )
    .unwrap();
    let err = load_global_config(home.path()).unwrap_err();
    assert!(err.to_string().contains("failed to parse"), "{err}");
    // The bare shape (no `schedule` table) still loads.
    std::fs::write(
        dir.join(GLOBAL_SCHEDULE_FILE),
        "[[jobs]]\nname = \"doctor\"\nevery = \"6h\"\ncommand = \"doctor\"\n",
    )
    .unwrap();
    let cfg = load_global_config(home.path()).unwrap().unwrap();
    assert_eq!(cfg.tasks.len(), 1);
}

#[test]
fn on_job_fires_once_per_new_event() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = ScheduleState::default();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mk = |ts: DateTime<Utc>, kind: EventKind| Event {
        ts,
        spec: None,
        run_uuid: String::new(),
        seat: None,
        kind,
    };
    let events_a = vec![
        mk(at(9), EventKind::PrMerged { pr: 1 }),
        mk(at(10), EventKind::RunStarted),
    ];
    let mut on_reap = task("queue-gc", "24h", "queue gc");
    on_reap.interval = None;
    on_reap.on = vec![EventTrigger::parse("t", "PrMerged").unwrap()];
    let run = |state: &mut ScheduleState, now, events: &[Event]| {
        tick_core(
            tmp.path(),
            config(vec![on_reap.clone()]),
            state,
            now,
            false,
            ok_exec(Rc::clone(&seen)),
            |_| Snapshot::default(),
            events,
        )
        .unwrap()
    };
    // First tick: the job is seen for the first time — history is NOT
    // replayed (advisor round 1 on #1946); the cursor initialises to now.
    let out = run(&mut state, at(11), &events_a);
    assert!(out.is_empty(), "{out:?}");
    assert!(seen.borrow().is_empty());
    assert_eq!(
        state.tasks["queue-gc"].last_seen_event_ts,
        Some(at(11)),
        "cursor initialises to now on first sight"
    );
    // Same events again: nothing new → no run.
    let out = run(&mut state, at(12), &events_a);
    assert!(out.is_empty());
    assert!(seen.borrow().is_empty());
    // A new PrMerged after the cursor → runs once; an unlisted kind
    // (RunStarted) never triggers.
    let mut events_b = events_a.clone();
    events_b.push(mk(at(12), EventKind::RunStarted));
    events_b.push(mk(at(13), EventKind::PrMerged { pr: 2 }));
    let out = run(&mut state, at(14), &events_b);
    assert_eq!(out, vec!["schedule tick: queue-gc ok"]);
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(state.tasks["queue-gc"].last_seen_event_ts, Some(at(13)));
}

// trace:STORY-1226 | ai:claude
#[test]
fn when_job_fires_once_until_cleared() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = ScheduleState::default();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut latency = task("mailbox-latency", "1h", "notify check");
    latency.interval = None;
    latency.when = Some(schedule_predicate::parse("mail.oldest_unread_age > 15m").unwrap());
    latency.when_raw = Some("mail.oldest_unread_age > 15m".into());
    let run = |state: &mut ScheduleState, now, age_secs: i64| {
        tick_core(
            tmp.path(),
            config(vec![latency.clone()]),
            state,
            now,
            false,
            ok_exec(Rc::clone(&seen)),
            move |fields| {
                assert!(fields.contains(&Field::MailOldestUnreadAge));
                Snapshot {
                    mail_oldest_unread_age_secs: age_secs,
                    ..Default::default()
                }
            },
            &[],
        )
        .unwrap()
    };
    // false → nothing.
    assert!(run(&mut state, at(9), 60).is_empty());
    // turns true → fires once.
    assert_eq!(
        run(&mut state, at(10), 20 * 60),
        vec!["schedule tick: mailbox-latency ok"]
    );
    // still true → no re-fire.
    assert!(run(&mut state, at(11), 30 * 60).is_empty());
    assert_eq!(seen.borrow().len(), 1);
    // clears.
    assert!(run(&mut state, at(12), 0).is_empty());
    let ledger = schedule_ledger::load(&store_root(tmp.path()), "mailbox-latency").unwrap();
    assert_eq!(ledger.episode.as_ref().unwrap().cleared_at, Some(at(12)));
    // true again → a new episode fires.
    assert_eq!(
        run(&mut state, at(13), 40 * 60),
        vec!["schedule tick: mailbox-latency ok"]
    );
    assert_eq!(seen.borrow().len(), 2);
}

// trace:TASK-1281 | ai:codex
#[test]
fn substrate_when_firing_writes_one_ledger_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_root(tmp.path());
    std::fs::create_dir_all(&store).unwrap();
    let git = |args: &[&str]| {
        let output = ProcessCommand::new("git")
            .arg("-C")
            .arg(&store)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q", "-b", "aida-store"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Schedule Test"]);

    let mut job = task("mailbox-latency", "1h", "notify check");
    job.interval = None;
    job.when = Some(schedule_predicate::parse("mail.oldest_unread_age > 15m").unwrap());
    job.when_raw = Some("mail.oldest_unread_age > 15m".into());
    let mut state = ScheduleState::default();
    tick_core(
        tmp.path(),
        config(vec![job]),
        &mut state,
        at(12),
        false,
        |_root, _command| {
            Ok(TaskOutcome {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        },
        |_| Snapshot {
            mail_oldest_unread_age_secs: 20 * 60,
            ..Default::default()
        },
        &[],
    )
    .unwrap();

    assert_eq!(git(&["rev-list", "--count", "HEAD"]), "1");
    let ledger = schedule_ledger::load(&store, "mailbox-latency").unwrap();
    assert_eq!(ledger.result.as_deref(), Some("ok"));
    assert_eq!(ledger.episode.unwrap().fired_at, Some(at(12)));
}

// trace:TASK-1281 | ai:codex
#[test]
fn seat_when_stays_debounced_after_done_clears_due() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_root(tmp.path());
    let mut state = ScheduleState::default();
    let mut job = seat_task("mailbox-triage", &["advisor"], None, &[]);
    job.when = Some(schedule_predicate::parse("mail.oldest_unread_age > 15m").unwrap());
    job.when_raw = Some("mail.oldest_unread_age > 15m".into());
    let run = |state: &mut ScheduleState, now| {
        tick_core(
            tmp.path(),
            config(vec![job.clone()]),
            state,
            now,
            false,
            |_root, _command| panic!("seat jobs must never execute"),
            |_| Snapshot {
                mail_oldest_unread_age_secs: 20 * 60,
                ..Default::default()
            },
            &[],
        )
        .unwrap()
    };

    assert_eq!(
        run(&mut state, at(10)),
        vec!["schedule tick: mailbox-triage due (when) → seat advisor"]
    );
    let ledger = schedule_ledger::load(&store, "mailbox-triage").unwrap();
    assert!(ledger.episode.as_ref().is_some_and(|ep| ep.is_open()));

    // Simulate `aida schedule done`: it clears delivery state, but the
    // condition episode remains open until the predicate turns false.
    schedule_ledger::write_cas(&store, "mailbox-triage", |l| {
        l.last_run = Some(at(11));
        l.result = Some("done".into());
        l.due_since = None;
        l.due_reason = None;
    })
    .unwrap();

    assert!(run(&mut state, at(12)).is_empty());
    let ledger = schedule_ledger::load(&store, "mailbox-triage").unwrap();
    assert!(ledger.due_since.is_none());
    assert!(ledger.episode.as_ref().is_some_and(|ep| ep.is_open()));
}

// trace:STORY-1226 | ai:claude
#[test]
fn every_and_on_combined() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = ScheduleState::default();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let triage = seat_task(
        "mailbox-triage",
        &["advisor"],
        Some("30m"),
        &["MailReceived"],
    );
    let mail = Event {
        ts: at(12),
        spec: None,
        run_uuid: String::new(),
        seat: None,
        kind: EventKind::MailReceived {
            to: "advisor".into(),
        },
    };
    // Reported done at 11:50 → the interval is NOT due at 12:01, but the
    // 12:00 MailReceived is → due via the event fast path.
    let store = store_root(tmp.path());
    let done_at = Utc.with_ymd_and_hms(2026, 9, 11, 11, 50, 0).unwrap();
    schedule_ledger::write_cas(&store, "mailbox-triage", |l| {
        l.last_run = Some(done_at);
        l.result = Some("done".into());
    })
    .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 12, 1, 0).unwrap();
    let out = tick_core(
        tmp.path(),
        config(vec![triage.clone()]),
        &mut state,
        now,
        false,
        ok_exec(Rc::clone(&seen)),
        |_| Snapshot::default(),
        std::slice::from_ref(&mail),
    )
    .unwrap();
    assert_eq!(
        out,
        vec!["schedule tick: mailbox-triage due (on MailReceived) → seat advisor"]
    );
    assert!(seen.borrow().is_empty());
    // Clear it; with no new event, the interval fallback takes over at 12:30.
    schedule_ledger::write_cas(&store, "mailbox-triage", |l| {
        l.last_run = Some(now);
        l.due_since = None;
        l.due_reason = None;
    })
    .unwrap();
    let later = Utc.with_ymd_and_hms(2026, 9, 11, 12, 35, 0).unwrap();
    let out = tick_core(
        tmp.path(),
        config(vec![triage]),
        &mut state,
        later,
        false,
        ok_exec(Rc::clone(&seen)),
        |_| Snapshot::default(),
        std::slice::from_ref(&mail),
    )
    .unwrap();
    assert_eq!(
        out,
        vec!["schedule tick: mailbox-triage due (every 30m) → seat advisor"]
    );
}

// trace:STORY-1226 | ai:claude
#[test]
fn due_jobs_block_renders_report_back_line() {
    let due = vec![DueJob {
        name: "mailbox-triage".into(),
        kind: JobKind::Seat,
        seats: vec!["advisor".into()],
        schedule: "every 30m".into(),
        reason: "every 30m".into(),
        prompt: Some("triage the mailbox".into()),
        command: None,
        last_run: Some(Utc::now() - Duration::minutes(47)),
        last_by: Some("advisor/claude".into()),
        due_since: None,
        failure: None,
    }];
    let block = render_due_jobs_block(&due, "advisor");
    assert!(block.starts_with("DUE JOBS (seat: advisor):\n"));
    assert!(block.contains("- mailbox-triage (every 30m, last 47m ago) → triage the mailbox"));
    assert!(block.contains("aida schedule done <job>"));
    assert_eq!(render_due_jobs_block(&[], "advisor"), "");
}

// trace:STORY-1226 | ai:claude
#[test]
fn due_seat_jobs_reads_registry_and_ledger_file_only() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", tmp.path());
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        r#"
[schedule]

[[schedule.jobs]]
name = "mailbox-triage"
seats = ["advisor"]
every = "30m"
prompt = "triage the mailbox"
enabled = true

[[schedule.jobs]]
name = "capture-sweep"
seats = ["product"]
on = ["QueueDrained"]
prompt = "capture sweep"
enabled = true

[[schedule.jobs]]
name = "session-reap"
command = "session reap"
every = "30m"
enabled = true
"#,
    )
    .unwrap();
    // Never run → the interval job is due; the on-job is not (no tick flagged it).
    let due = due_seat_jobs(tmp.path(), Some("advisor"));
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].name, "mailbox-triage");
    assert_eq!(due[0].reason, "every 30m");
    assert!(due_seat_jobs(tmp.path(), Some("product")).is_empty());
    // Substrate jobs never surface through the seat read.
    assert!(due_seat_jobs(tmp.path(), None)
        .iter()
        .all(|j| j.kind == JobKind::Seat));
    // A tick-flagged on-job surfaces for its seat with the event reason.
    schedule_ledger::write_cas(&store_root(tmp.path()), "capture-sweep", |l| {
        l.due_since = Some(Utc::now());
        l.due_reason = Some("on QueueDrained".into());
    })
    .unwrap();
    let due = due_seat_jobs(tmp.path(), Some("product"));
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].reason, "on QueueDrained");
    // No seat filter → both.
    assert_eq!(due_seat_jobs(tmp.path(), None).len(), 2);
}

// trace:STORY-1226 | ai:claude
#[test]
fn global_layer_reads_schedule_toml_in_both_shapes() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".aida")).unwrap();
    std::fs::write(
        home.path().join(".aida/schedule.toml"),
        r#"
[[jobs]]
name = "doctor"
command = "doctor"
every = "6h"
enabled = true
"#,
    )
    .unwrap();
    let cfg = load_global_config(home.path()).unwrap().unwrap();
    assert_eq!(cfg.tasks.len(), 1);
    assert_eq!(cfg.tasks[0].source, JobSource::Global);
    std::fs::write(
        home.path().join(".aida/schedule.toml"),
        r#"
[schedule]
min_gap = "2m"
[[schedule.jobs]]
name = "doctor"
command = "doctor"
every = "6h"
enabled = true
"#,
    )
    .unwrap();
    let cfg = load_global_config(home.path()).unwrap().unwrap();
    assert_eq!(cfg.min_gap, "2m");
    assert_eq!(cfg.tasks.len(), 1);
    assert!(load_global_config(tempfile::tempdir().unwrap().path())
        .unwrap()
        .is_none());
}

/// STORY-1423 criterion 3 (THE TRIGGER): a dogfood regression test over
/// this repo's OWN `.aida/config.toml`, not a fixture. `aida schedule
/// list` reporting both jobs enabled was a point-in-time observation
/// (2026-09-21, PR #2062); nothing stopped a later config edit from
/// silently disabling or deleting either entry and letting the gate go
/// dark again — exactly the "commented example is a trigger for nobody"
/// defect this spec exists to close, recurring one layer out. Reads the
/// real file via `CARGO_MANIFEST_DIR` so this fails the moment the
/// dogfood surface regresses.
// trace:STORY-1423 | ai:claude
#[test]
fn this_repo_keeps_the_performance_guard_pair_registered_and_enabled() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("aida-cli-lib has a workspace parent");
    let cfg = load_config(repo_root)
        .unwrap()
        .expect("this repo's .aida/config.toml must declare a [schedule] section");

    let guard = cfg
        .tasks
        .iter()
        .find(|t| t.name == "performance-guard")
        .expect("the performance-guard substrate job must stay registered in this repo");
    assert_eq!(guard.kind, JobKind::Substrate);
    assert!(
        guard.enabled,
        "performance-guard must stay enabled — a disabled entry is a trigger for nobody"
    );

    let route = cfg
        .tasks
        .iter()
        .find(|t| t.name == "performance-guard-route")
        .expect("the performance-guard-route seat job must stay registered in this repo");
    assert_eq!(route.kind, JobKind::Seat);
    assert!(
        route.enabled,
        "performance-guard-route must stay enabled so a trip still reaches a seat"
    );
}

// ---- a guard's failure routes only to its own route job ----
// trace:BUG-1655 | ai:claude

const DISK: &str = "doctor check disk-headroom --fail-on-findings";
const DRIFT: &str = "doctor check remote-drift --fail-on-findings";

/// Run both guards once at `at(12)`; only the disk guard fails with
/// `status`. Returns the events the tick emitted.
fn bug_1655_trip_disk_guard(root: &Path, status: i32, stderr: &str) -> Vec<Event> {
    let mut state = ScheduleState::default();
    let stderr = stderr.to_string();
    tick_with_executor(
        root,
        config(vec![
            task("disk-headroom-guard", "30m", DISK),
            task("hub-drift-guard", "6h", DRIFT),
        ]),
        &mut state,
        at(12),
        false,
        move |_root, cmd| {
            Ok(TaskOutcome {
                status: if cmd.display == DISK { status } else { 0 },
                stdout: String::new(),
                stderr: if cmd.display == DISK {
                    stderr.clone()
                } else {
                    String::new()
                },
            })
        },
    )
    .unwrap();
    events::read_all(root)
}

/// Route every event through `routes` with cursors set before the trip.
fn bug_1655_route(root: &Path, routes: Vec<Task>, events: &[Event]) {
    let mut state = ScheduleState::default();
    for r in &routes {
        state.tasks.insert(
            r.name.clone(),
            TaskState {
                last_seen_event_ts: Some(at(11)),
                ..Default::default()
            },
        );
    }
    tick_core(
        root,
        config(routes),
        &mut state,
        at(13),
        false,
        |_root, _cmd| unreachable!("route jobs are never executed"),
        |_| Snapshot::default(),
        events,
    )
    .unwrap();
}

/// STORY-1916 AC3: when reclaim cannot restore the floor, the routed
/// item's evidence says so, with the top consumers. The guard's
/// `--fail-on-findings` reason is its last stderr line, and the route
/// carries exactly that line.
// trace:STORY-1916.ac641c6e | ai:claude
#[test]
fn story_1916_routed_disk_item_carries_the_stuck_sentence() {
    let tmp = tempfile::tempdir().unwrap();
    let finding = crate::doctor_root::DoctorFinding {
        category: "disk-headroom".into(),
        id: "disk-headroom-cache".into(),
        summary: format!(
            "free disk space is 12.0 GiB, below the 60 GiB floor. {}",
            crate::disk_emergency::stuck_summary(
                3 << 30,
                48 << 30,
                &[("/t/worktrees/take-2845".into(), 39 << 30)]
            )
        ),
        action: String::new(),
        safe_heal: false,
    };
    let reason = crate::doctor_cmd::disk_headroom_fail_reason(&[finding]).unwrap();
    let events = bug_1655_trip_disk_guard(tmp.path(), 1, &format!("warning: noise\n{reason}\n"));
    bug_1655_route(
        tmp.path(),
        vec![seat_task(
            "disk-headroom-guard-route",
            &["advisor"],
            None,
            &["CronJobFailed:disk-headroom-guard"],
        )],
        &events,
    );
    let routed = schedule_ledger::load(&store_root(tmp.path()), "disk-headroom-guard-route")
        .and_then(|l| l.due_failure)
        .and_then(|f| f.error)
        .expect("routed evidence");
    assert!(routed.starts_with("exit 1: STUCK:"), "{routed}");
    assert!(
        routed.contains("can free only 3.0 GiB of the 48.0 GiB"),
        "{routed}"
    );
    assert!(
        routed.contains("39.0 GiB /t/worktrees/take-2845"),
        "{routed}"
    );
}

#[test]
fn bug_1655_only_the_failing_guards_route_comes_due_with_its_own_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let events = bug_1655_trip_disk_guard(tmp.path(), 1, "free 12 GiB below floor 60 GiB");
    let failures: Vec<&str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::CronJobFailed { job, .. } => Some(job.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(failures, vec!["disk-headroom-guard"]);

    bug_1655_route(
        tmp.path(),
        vec![
            seat_task(
                "disk-headroom-guard-route",
                &["advisor"],
                None,
                &["CronJobFailed:disk-headroom-guard"],
            ),
            seat_task(
                "hub-drift-guard-route",
                &["advisor"],
                None,
                &["CronJobFailed:hub-drift-guard"],
            ),
        ],
        &events,
    );

    let store = store_root(tmp.path());
    let disk = schedule_ledger::load(&store, "disk-headroom-guard-route").unwrap();
    assert!(disk.due_since.is_some(), "the tripped guard's route is due");
    assert_eq!(
        disk.due_reason.as_deref(),
        Some("on CronJobFailed:disk-headroom-guard")
    );
    let failure = disk.due_failure.expect("routed evidence");
    assert!(
        failure.trip_id.starts_with("disk-headroom-guard@"),
        "{}",
        failure.trip_id
    );
    assert_eq!(failure.job.as_deref(), Some("disk-headroom-guard"));
    assert_eq!(
        failure.error.as_deref(),
        Some("exit 1: free 12 GiB below floor 60 GiB")
    );

    let drift = schedule_ledger::load(&store, "hub-drift-guard-route");
    assert!(
        drift
            .as_ref()
            .is_none_or(|l| l.due_since.is_none() && l.due_failure.is_none()),
        "a clean guard's route must not wake on another guard's trip: {drift:?}"
    );
}

#[test]
fn bug_1655_unbound_trigger_keeps_matching_any_job_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let events = bug_1655_trip_disk_guard(tmp.path(), 1, "below floor");
    bug_1655_route(
        tmp.path(),
        vec![seat_task(
            "legacy-route",
            &["advisor"],
            None,
            &["CronJobFailed"],
        )],
        &events,
    );
    let legacy = schedule_ledger::load(&store_root(tmp.path()), "legacy-route").unwrap();
    assert!(
        legacy.due_since.is_some(),
        "an unbound trigger is unchanged"
    );
    assert_eq!(
        legacy.due_failure.and_then(|f| f.job).as_deref(),
        Some("disk-headroom-guard")
    );
}

#[test]
fn bug_1655_timeout_is_labelled_as_not_a_finding() {
    let tmp = tempfile::tempdir().unwrap();
    let events = bug_1655_trip_disk_guard(
        tmp.path(),
        TIMEOUT_EXIT_STATUS,
        "timed out: doctor check disk-headroom did not complete within 120s (killed)",
    );
    bug_1655_route(
        tmp.path(),
        vec![seat_task(
            "disk-headroom-guard-route",
            &["advisor"],
            None,
            &["CronJobFailed:disk-headroom-guard"],
        )],
        &events,
    );
    let ledger =
        schedule_ledger::load(&store_root(tmp.path()), "disk-headroom-guard-route").unwrap();
    let due = DueJob {
        name: "disk-headroom-guard-route".into(),
        kind: JobKind::Seat,
        seats: vec!["advisor".into()],
        schedule: "on CronJobFailed:disk-headroom-guard".into(),
        reason: ledger.due_reason.clone().unwrap(),
        prompt: Some("reclaim space".into()),
        command: None,
        last_run: None,
        last_by: None,
        due_since: ledger.due_since,
        failure: ledger.due_failure.clone(),
    };
    let line = due.line(at(14));
    assert!(
        line.contains("disk-headroom-guard timed out before reporting — this is not a finding"),
        "{line}"
    );

    // A real finding reads as a failure, not a timeout.
    let mut finding = due.clone();
    finding.failure.as_mut().unwrap().error = Some("exit 1: below floor".into());
    let line = finding.line(at(14));
    assert!(
        line.contains("disk-headroom-guard failed: exit 1: below floor"),
        "{line}"
    );
    assert!(!line.contains("timed out"), "{line}");
}

#[test]
fn bug_1655_trigger_binding_is_validated_at_load() {
    assert!(EventTrigger::parse("r", "CronJobFailed:").is_err());
    assert!(EventTrigger::parse("r", "PrMerged:some-job").is_err());
    assert!(EventTrigger::parse("r", "NoSuchEvent:x").is_err());
    let t = EventTrigger::parse("r", "CronJobFailed:guard").unwrap();
    assert_eq!(t.to_string(), "CronJobFailed:guard");

    let known = parse_project(
        r#"
[[schedule.jobs]]
name = "guard"
command = "doctor"
every = "6h"
enabled = true

[[schedule.jobs]]
name = "guard-route"
seats = ["advisor"]
on = ["CronJobFailed:guard"]
prompt = "look"
enabled = true
"#,
    )
    .unwrap()
    .unwrap();
    let mut known = known;
    disable_unknown_bindings(&mut known);
    assert!(known.tasks.iter().all(|t| t.enabled && t.problem.is_none()));
    assert_eq!(known.tasks[1].schedule_summary(), "on CronJobFailed:guard");
}

/// A binding to a job that is not in the registry disables ONLY that
/// route (with a reason tick and status surface); every other job still
/// loads and runs.
#[test]
fn bug_1655_unknown_bound_job_disables_only_that_route() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        r#"
[[schedule.jobs]]
name = "watchdog"
command = "doctor"
every = "15m"
enabled = true

[[schedule.jobs]]
name = "watchdog-route"
seats = ["advisor"]
on = ["CronJobFailed:watchdog"]
prompt = "look"
enabled = true

[[schedule.jobs]]
name = "guard-route"
seats = ["advisor"]
on = ["CronJobFailed:gaurd"]
prompt = "look"
enabled = true
"#,
    )
    .unwrap();
    let home = tempfile::tempdir().unwrap();
    let loaded = {
        let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path());
        load_registry(tmp.path())
    };
    let mut cfg = loaded
        .expect("one bad route must not fail the registry")
        .unwrap();
    let by_name = |cfg: &LoadedScheduleConfig, n: &str| {
        cfg.tasks.iter().find(|t| t.name == n).cloned().unwrap()
    };
    let bad = by_name(&cfg, "guard-route");
    assert!(!bad.enabled);
    let problem = bad.problem.clone().unwrap();
    assert!(problem.contains("CronJobFailed:gaurd"), "{problem}");
    assert!(by_name(&cfg, "watchdog").enabled);
    assert!(by_name(&cfg, "watchdog-route").enabled);

    let row = status_row(&bad, None, None, at(12));
    assert_eq!(row.status, "invalid");
    assert_eq!(row.problem.as_deref(), Some(problem.as_str()));

    cfg.tasks.retain(|t| t.name != "watchdog-route");
    let mut state = ScheduleState::default();
    let ran = std::cell::Cell::new(0);
    let lines = tick_with_executor(tmp.path(), cfg, &mut state, at(12), false, |_r, _c| {
        ran.set(ran.get() + 1);
        Ok(TaskOutcome {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    })
    .unwrap();
    assert_eq!(ran.get(), 1, "the watchdog still runs: {lines:?}");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("schedule tick: guard-route skipped:")),
        "{lines:?}"
    );
}

/// Every scaffolded `<guard>-route` binds its `on` to `<guard>`, so no
/// route wakes on another guard's failure. (This repo's own config is
/// switched separately, once the installed binary understands bindings.)
#[test]
fn bug_1655_every_guard_route_binds_to_its_own_guard() {
    for (label, body) in [("scaffold", crate::init_cmd::init_schedule_config_section())] {
        let lines: Vec<&str> = body
            .lines()
            .map(|l| l.trim().trim_start_matches('#').trim())
            .collect();
        let mut routes = 0;
        for (i, line) in lines.iter().enumerate() {
            let Some(route) = line
                .strip_prefix("name = \"")
                .and_then(|r| r.strip_suffix("-route\""))
            else {
                continue;
            };
            routes += 1;
            let on = lines[i..]
                .iter()
                .find(|l| l.starts_with("on = "))
                .unwrap_or_else(|| panic!("{label}: {route}-route has no `on`"));
            assert_eq!(
                *on,
                format!("on = [\"CronJobFailed:{route}\"]"),
                "{label}: {route}-route must bind to its own guard"
            );
        }
        assert!(
            routes >= 4,
            "{label}: expected the guard routes, found {routes}"
        );
    }
}

#[test]
fn bug_1655_doctor_flags_an_enabled_route_with_an_unbound_trigger() {
    let mut disabled = seat_task("old-guard-route", &["advisor"], None, &["CronJobFailed"]);
    disabled.enabled = false;
    let tasks = vec![
        task("disk-headroom-guard", "1h", "doctor"),
        seat_task(
            "disk-headroom-guard-route",
            &["advisor"],
            None,
            &["CronJobFailed"],
        ),
        seat_task(
            "watchdog-route",
            &["advisor"],
            None,
            &["CronJobFailed:watchdog"],
        ),
        seat_task("any-failure-notice", &["advisor"], None, &["CronJobFailed"]),
        disabled,
    ];
    let findings = build_unbound_route_findings(&tasks);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(
        findings[0].id,
        "schedule-route-unbound:disk-headroom-guard-route"
    );
    assert!(
        findings[0]
            .action
            .contains("on = [\"CronJobFailed:disk-headroom-guard\"]"),
        "{}",
        findings[0].action
    );
}

// trace:TASK-1535 | ai:codex
#[test]
fn task_1535_doctor_finds_invalid_route_and_only_suggests_known_guard() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        r#"
[[schedule.jobs]]
name = "known-guard"
command = "doctor"
every = "1h"
enabled = false

[[schedule.jobs]]
name = "known-guard-route"
seats = ["advisor"]
on = ["CronJobFailed"]
prompt = "look"
enabled = true

[[schedule.jobs]]
name = "orphan-route"
seats = ["advisor"]
on = ["CronJobFailed"]
prompt = "look"
enabled = true

[[schedule.jobs]]
name = "renamed-route"
seats = ["advisor"]
on = ["CronJobFailed:missing-guard"]
prompt = "look"
enabled = true

[[schedule.jobs]]
name = "failure-notice"
seats = ["advisor"]
on = ["CronJobFailed:missing-guard"]
prompt = "look"
enabled = true

[[schedule.jobs]]
name = "user-disabled-route"
seats = ["advisor"]
on = ["CronJobFailed:missing-guard"]
prompt = "look"
enabled = false

[[schedule.jobs]]
name = "user-disabled-notice"
seats = ["advisor"]
on = ["CronJobFailed:missing-guard"]
prompt = "look"
enabled = false
"#,
    )
    .unwrap();
    let home = tempfile::tempdir().unwrap();
    let (findings, cfg) = {
        let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path());
        let findings = scheduler_driver_doctor_findings(tmp.path()).unwrap();
        let cfg = load_registry(tmp.path()).unwrap().unwrap();
        (findings, cfg)
    };
    let find = |id: &str| findings.iter().find(|f| f.id == id).unwrap();
    assert!(find("schedule-route-invalid:renamed-route")
        .summary
        .contains("missing-guard"));
    let failure_notice = cfg
        .tasks
        .iter()
        .find(|t| t.name == "failure-notice")
        .unwrap();
    assert!(!failure_notice.enabled);
    assert!(failure_notice
        .problem
        .as_deref()
        .unwrap()
        .contains("missing-guard"));
    assert!(find("schedule-route-invalid:failure-notice")
        .summary
        .contains("missing-guard"));
    assert!(find("schedule-route-unbound:known-guard-route")
        .action
        .contains("CronJobFailed:known-guard"));
    let orphan = &find("schedule-route-unbound:orphan-route").action;
    assert!(orphan.contains("bind it to the job it guards"), "{orphan}");
    assert!(!orphan.contains("CronJobFailed:orphan"), "{orphan}");
    assert!(!findings.iter().any(|f| f.id.contains("user-disabled")));
    let user_disabled_notice = cfg
        .tasks
        .iter()
        .find(|t| t.name == "user-disabled-notice")
        .unwrap();
    assert!(!user_disabled_notice.enabled);
    assert!(user_disabled_notice.problem.is_none());

    let mut state = ScheduleState::default();
    let lines = tick_with_executor(tmp.path(), cfg, &mut state, at(12), false, |_r, _c| {
        panic!("no substrate job should run")
    })
    .unwrap();
    assert!(lines.iter().any(|l| l.contains("renamed-route skipped")));
    assert!(lines.iter().any(|l| l.contains("failure-notice skipped")));
    assert!(!lines
        .iter()
        .any(|l| l.contains("user-disabled-route skipped")));
    assert!(!lines
        .iter()
        .any(|l| l.contains("user-disabled-notice skipped")));
}

#[test]
fn task_1535_spawn_failure_is_not_labelled_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let cmd = ProcessCommand::new(tmp.path().join("no-such-command"));
    let outcome = run_with_kill_timeout(
        cmd,
        "missing",
        tmp.path(),
        std::time::Duration::from_millis(50),
    );
    assert_eq!(outcome.status, 124);
    assert!(outcome.stderr.starts_with("could not be spawned:"));
    assert!(!outcome.stderr.contains("timed out"));
}

// -----------------------------------------------------------------
// BUG-2142: an unsupported job command fails only that job.
// -----------------------------------------------------------------

const BUG_2142_REGISTRY: &str = r#"
[[schedule.jobs]]
name = "watchdog"
command = "doctor"
every = "15m"
enabled = true

[[schedule.jobs]]
name = "history-index-tick"
command = "cache refresh --history --from-the-future"
every = "15m"
enabled = true

[[schedule.jobs]]
name = "history-index-tick-route"
seats = ["advisor"]
on = ["CronJobFailed:history-index-tick"]
prompt = "look"
enabled = true
"#;

// trace:BUG-2142 | ai:claude
#[test]
fn bug_2142_unknown_command_loads_as_an_invalid_job_not_a_registry_error() {
    let cfg = parse_project(BUG_2142_REGISTRY)
        .expect("one unknown command must not fail the registry")
        .unwrap();
    let bad = cfg
        .tasks
        .iter()
        .find(|t| t.name == "history-index-tick")
        .unwrap();
    assert!(bad.command.is_none());
    let invalid = bad.invalid_command.as_ref().unwrap();
    assert!(invalid.error.contains("unknown scheduled task command"));
    assert!(bad.enabled, "it stays enabled so each due tick records it");
    assert!(cfg
        .tasks
        .iter()
        .find(|t| t.name == "watchdog")
        .unwrap()
        .command
        .is_some());
    assert_eq!(
        unsupported_substrate_jobs(&cfg),
        vec!["history-index-tick (`aida cache refresh --history --from-the-future`)"]
    );

    let row = status_row(bad, None, None, at(12));
    assert_eq!(row.status, "invalid");
    assert!(row.problem.unwrap().contains("does not support"));
    assert!(row.command.contains("(unsupported)"));

    let findings = build_unsupported_command_findings(&cfg.tasks);
    assert_eq!(findings.len(), 1);
    assert_eq!(
        findings[0].id,
        "schedule-command-unsupported:history-index-tick"
    );
}

// trace:BUG-2142 | ai:claude
#[test]
fn bug_2142_tick_fails_only_the_unsupported_job_and_routes_it() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = parse_project(BUG_2142_REGISTRY).unwrap().unwrap();
    let routes: Vec<Task> = cfg
        .tasks
        .iter()
        .filter(|t| t.kind == JobKind::Seat)
        .cloned()
        .collect();
    let mut jobs = cfg.clone();
    jobs.tasks.retain(|t| t.kind == JobKind::Substrate);
    let mut state = ScheduleState::default();
    let ran = RefCell::new(Vec::new());
    let lines = tick_with_executor(tmp.path(), jobs, &mut state, at(12), false, |_r, cmd| {
        ran.borrow_mut().push(cmd.display);
        Ok(TaskOutcome {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    })
    .expect("an unsupported job must not abort the tick");
    assert_eq!(*ran.borrow(), vec!["doctor"], "every other due job ran");
    assert!(
        lines.contains(&"schedule tick: watchdog ok".to_string()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"schedule tick: history-index-tick failed".to_string()),
        "{lines:?}"
    );

    let ledger = schedule_ledger::load(&store_root(tmp.path()), "history-index-tick").unwrap();
    assert_eq!(
        ledger.result.as_deref(),
        Some(format!("failed:{UNSUPPORTED_COMMAND_STATUS}").as_str())
    );
    assert_eq!(ledger.failure_trips.len(), 1);
    assert_eq!(
        state.tasks["history-index-tick"].last_status,
        Some(UNSUPPORTED_COMMAND_STATUS)
    );

    let events = events::read_all(tmp.path());
    let failed: Vec<(&str, &str)> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::CronJobFailed { job, error, .. } => Some((job.as_str(), error.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0].0, "history-index-tick");
    assert!(failed[0].1.contains("does not support"), "{}", failed[0].1);

    // Its CronJobFailed route comes due like any other failure.
    bug_1655_route(tmp.path(), routes, &events);
    let route = schedule_ledger::load(&store_root(tmp.path()), "history-index-tick-route").unwrap();
    assert!(route.due_since.is_some());
    assert_eq!(
        route.due_failure.and_then(|f| f.job).as_deref(),
        Some("history-index-tick")
    );
}

// trace:BUG-2142 | ai:claude
#[test]
fn bug_2142_hook_tick_leaves_an_unsupported_job_to_the_full_tick() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = parse_project(BUG_2142_REGISTRY).unwrap().unwrap();
    cfg.tasks.retain(|t| t.name == "history-index-tick");
    let mut state = ScheduleState::default();
    let lines = tick_with_executor(tmp.path(), cfg, &mut state, at(12), true, |_r, _c| {
        unreachable!("nothing runnable in a hook tick")
    })
    .unwrap();
    assert!(lines.is_empty(), "{lines:?}");
    assert!(schedule_ledger::load(&store_root(tmp.path()), "history-index-tick").is_none());
}

// trace:BUG-2142 | ai:claude
#[test]
fn bug_2142_full_tick_runs_the_rest_and_reports_the_unsupported_job() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", tmp.path());
    let root = tmp.path().join("project");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        r#"
[[schedule.jobs]]
name = "history-index-tick"
command = "cache refresh --history --from-the-future"
every = "15m"
enabled = true

[[schedule.jobs]]
name = "nightly-review"
seats = ["advisor"]
every = "1h"
prompt = "review"
enabled = true
"#,
    )
    .unwrap();
    let run = run_tick(&root, false, None).expect("the tick itself must not abort");
    assert!(
        run.lines
            .contains(&"schedule tick: history-index-tick failed".to_string()),
        "{:?}",
        run.lines
    );
    assert!(
        run.lines
            .iter()
            .any(|l| l.starts_with("schedule tick: nightly-review due")),
        "the other due job still ran: {:?}",
        run.lines
    );
    assert_eq!(run.unsupported.len(), 1);
    assert!(run.unsupported[0].starts_with("history-index-tick"));

    let hook = run_tick(&root, true, None).unwrap();
    assert!(
        hook.unsupported.is_empty(),
        "a hook tick never reports them"
    );
}

// STORY-1688 criterion 3: no model-side polling. The lane-liveness check
// is a substrate command kept off the per-turn hook path, every seat job
// it routes to fires only on its own CronJobFired event (no interval, no
// schedule), and this repo registers the check alone: disabled, with no
// polling seat job or failure route of its own.
// trace:STORY-1688.acfeeb8f | ai:claude
#[test]
fn story_1688_lane_liveness_has_no_model_side_polling() {
    let command = parse_scheduled_command("doctor check lane-liveness").unwrap();
    assert!(
        !command.hook_allowed,
        "the forge-calling check never runs per turn"
    );
    assert_eq!(command.args, ["doctor", "check", "lane-liveness", "--json"]);

    for route in crate::lane_liveness::ROUTES {
        let name = crate::lane_liveness::route_job_name(route.seat);
        let task = builtin_routed_seat_task(Path::new("."), &name).unwrap();
        assert_eq!(task.kind, JobKind::Seat);
        assert_eq!(task.seats, [route.seat]);
        assert!(
            task.interval.is_none() && task.when.is_none(),
            "{name} polls"
        );
        assert_eq!(task.on.len(), 1);
        assert_eq!(task.on[0].kind, "CronJobFired");
        assert_eq!(task.on[0].job.as_deref(), Some(name.as_str()));
    }
    assert!(builtin_routed_seat_task(Path::new("."), "lane-liveness-merge").is_none());
    assert!(builtin_routed_seat_task(Path::new("."), "lane-liveness-operator").is_none());

    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let config = load_config(repo).unwrap().unwrap();
    let registered: Vec<&Task> = config
        .tasks
        .iter()
        .filter(|t| t.name.starts_with("lane-liveness"))
        .collect();
    assert_eq!(
        registered
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["lane-liveness"],
        "route prompts live in lane_liveness::ROUTES, not config"
    );
    let job = registered[0];
    assert_eq!(job.kind, JobKind::Substrate);
    assert_eq!(job.interval, Some(parse_duration("10m").unwrap()));
    assert!(
        !job.enabled,
        "ships disabled until enabled after a read-only run"
    );
    assert!(config.tasks.iter().all(|t| t
        .on
        .iter()
        .all(|e| e.job.as_deref() != Some("lane-liveness"))));
}

// A typo'd mail-backlog route name is not a job: `done` refuses it and
// writes no ledger. Routes the guard raised (ledger present) and the
// operator escalation are jobs.
// trace:TASK-B6B1278890774EF993D2EBC327862470-134 | ai:claude
#[test]
fn mail_backlog_route_jobs_are_only_routes_the_guard_raised() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_root(tmp.path());
    let typo = "mail-backlog-guard-route-typo";
    let err = done(tmp.path(), typo, None).unwrap_err();
    assert!(err.to_string().contains("no scheduled job named"), "{err}");
    assert!(
        schedule_ledger::load(&store, typo).is_none(),
        "stray ledger"
    );

    assert!(builtin_routed_seat_task(&store, "mail-backlog-guard-route-operator").is_some());
    let advisor = "mail-backlog-guard-route-advisor";
    assert!(builtin_routed_seat_task(&store, advisor).is_none());
    schedule_ledger::write_cas(&store, advisor, |l| l.due_since = Some(Utc::now())).unwrap();
    let task = builtin_routed_seat_task(&store, advisor).unwrap();
    assert_eq!(task.seats, ["advisor"]);
    done(tmp.path(), advisor, None).unwrap();
    assert!(schedule_ledger::load(&store, advisor)
        .unwrap()
        .due_since
        .is_none());
}

// Operator escalations reach `aida awaiting` only in sessions a human
// reads — the operator terminal and interactive advisor — never in a
// headless lane or a doer seat; other seats' jobs stay scoped away.
// trace:TASK-B6B1278890774EF993D2EBC327862470-135 | ai:claude
// trace:TASK-B6B1278890774EF993D2EBC327862470-142 | ai:claude
#[test]
fn operator_escalations_reach_awaiting_only_where_a_human_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_root(tmp.path());
    let op = "mail-backlog-guard-route-operator";
    let adv = "mail-backlog-guard-route-advisor";
    for job in [op, adv] {
        schedule_ledger::write_cas(&store, job, |l| l.due_since = Some(Utc::now())).unwrap();
    }
    let names = |seat: Option<&str>, headless: bool| {
        let mut names: Vec<String> = awaiting_seat_jobs(tmp.path(), seat, headless)
            .into_iter()
            .map(|j| j.name)
            .collect();
        names.sort();
        names
    };
    // The operator terminal (no seat, or seat `operator`) sees it.
    assert_eq!(names(None, false), [adv, op]);
    assert_eq!(names(Some("operator"), false), [op]);
    // An interactive advisor sees it alongside its own.
    assert_eq!(names(Some("advisor"), false), [adv, op]);
    // Doer seats never see it, attended or not.
    assert!(names(Some("implementer"), false).is_empty());
    assert!(names(Some("implementer"), true).is_empty());
    assert!(names(Some("reviewer"), false).is_empty());
    // Headless lanes never see it, whatever the seat.
    assert_eq!(names(Some("advisor"), true), [adv]);
    assert_eq!(names(None, true), [adv]);
    assert!(names(Some("operator"), true).is_empty());
    // The seat-scoped delivery read itself is unchanged.
    assert!(due_seat_jobs(tmp.path(), Some("implementer")).is_empty());
}

// A doer seat (or any headless lane) cannot clear the operator
// escalation; the operator terminal and an interactive advisor can.
// trace:TASK-B6B1278890774EF993D2EBC327862470-142 | ai:claude
#[test]
fn schedule_done_refuses_operator_escalation_outside_human_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_root(tmp.path());
    let op = "mail-backlog-guard-route-operator";
    let raise = || {
        schedule_ledger::write_cas(&store, op, |l| l.due_since = Some(Utc::now())).unwrap();
    };
    raise();
    for (seat, headless) in [
        (Some("implementer"), true),
        (Some("implementer"), false),
        (Some("reviewer"), false),
        (Some("advisor"), true),
        (None, true),
    ] {
        let err = done_as(tmp.path(), op, None, Ok(seat), headless).unwrap_err();
        assert!(err.to_string().contains("operator escalation"), "{err}");
        assert!(
            schedule_ledger::load(&store, op)
                .unwrap()
                .due_since
                .is_some(),
            "{seat:?}/{headless} cleared the escalation"
        );
    }
    for (seat, headless) in [
        (None, false),
        (Some("operator"), false),
        (Some("advisor"), false),
    ] {
        raise();
        done_as(tmp.path(), op, None, Ok(seat), headless).unwrap();
        assert!(schedule_ledger::load(&store, op)
            .unwrap()
            .due_since
            .is_none());
    }
}

// The DUE JOBS block tells seats to report every listed job with
// `aida schedule done <job>`; the synthetic mandate-renewal job is not in
// the registry, so it must be accepted as a no-op acknowledgement.
// trace:TASK-2805 | ai:claude
#[test]
fn schedule_done_accepts_synthetic_mandate_renewal_job() {
    let tmp = tempfile::tempdir().unwrap();
    for (seat, headless) in [
        (Some("advisor"), false),
        (Some("advisor"), true),
        (None, false),
    ] {
        done_as(tmp.path(), MANDATE_RENEWAL_JOB, None, Ok(seat), headless).unwrap();
    }
    let store = store_root(tmp.path());
    assert!(schedule_ledger::load(&store, MANDATE_RENEWAL_JOB).is_none());
    // An unknown job is still refused.
    let err = done_as(tmp.path(), "no-such-job", None, Ok(None), false).unwrap_err();
    assert!(err.to_string().contains("no scheduled job"), "{err}");
}

fn write_role_lease(root: &Path, worktree: &Path, role: &str) {
    let mut table: toml::Table = toml::from_str(
        r#"
id = "lease-impl"
scope = "STORY-9"
slug = "story-9"
owner = "tester"
branch = "claude/story-9"
started_at = "2026-10-22T00:00:00Z"
hostname = "h"
"#,
    )
    .unwrap();
    table.insert(
        "worktree_path".into(),
        toml::Value::String(worktree.to_string_lossy().into_owned()),
    );
    table.insert("role".into(), toml::Value::String(role.into()));
    let path = crate::lease_path(root, "lease-impl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, toml::to_string_pretty(&table).unwrap()).unwrap();
}

// `schedule done` resolves the seat exactly as the awaiting notice does
// (env, else the lease covering cwd): a role-leased implementer with no
// AIDA_SESSION_ROLE is refused, and an unresolvable seat fails closed.
// trace:TASK-B6B1278890774EF993D2EBC327862470-142 | ai:claude
#[test]
fn schedule_done_seat_comes_from_the_covering_lease_and_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let lane = root.join("lane");
    std::fs::create_dir_all(lane.join("src")).unwrap();
    write_role_lease(&root, &lane, "implementer");
    let store = store_root(&root);
    let op = "mail-backlog-guard-route-operator";
    schedule_ledger::write_cas(&store, op, |l| l.due_since = Some(Utc::now())).unwrap();

    let cwd = lane.join("src");
    let seat = resolve_done_seat(&root, None, Some(&cwd));
    assert_eq!(seat, Ok(Some("implementer".to_string())));
    // The notice's rule agrees on the same inputs.
    let (lease, role) = crate::session_seat(None, &crate::leases::list_leases(&root), Some(&cwd));
    assert!(lease.is_some());
    assert_eq!(role.as_deref(), Some("implementer"));

    let err = done_as(
        &root,
        op,
        None,
        seat.as_ref().map(|s| s.as_deref()).map_err(String::as_str),
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("operator escalation"), "{err}");
    // Unknown cwd: the seat cannot be resolved, so the gate refuses.
    let unresolved = resolve_done_seat(&root, None, None);
    assert!(unresolved.is_err());
    let err = done_as(&root, op, None, Err("no cwd"), false).unwrap_err();
    assert!(err.to_string().contains("could not be resolved"), "{err}");
    assert!(schedule_ledger::load(&store, op)
        .unwrap()
        .due_since
        .is_some());
    // The env still wins, as in the notice; outside any lease the
    // operator terminal (no seat) can clear it.
    assert_eq!(
        resolve_done_seat(&root, Some("advisor".into()), Some(&cwd)),
        Ok(Some("advisor".to_string()))
    );
    assert_eq!(resolve_done_seat(&root, None, Some(&root)), Ok(None));
    done_as(&root, op, None, Ok(None), false).unwrap();
}

// A launched agent's context never lists the operator escalation, even
// when its seat is unknown and every seat's due jobs are listed.
// trace:TASK-B6B1278890774EF993D2EBC327862470-142 | ai:claude
#[test]
fn launch_due_jobs_section_omits_operator_escalations() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_root(tmp.path());
    let op = "mail-backlog-guard-route-operator";
    let adv = "mail-backlog-guard-route-advisor";
    for job in [op, adv] {
        schedule_ledger::write_cas(&store, job, |l| l.due_since = Some(Utc::now())).unwrap();
    }
    assert!(due_seat_jobs(tmp.path(), None).iter().any(|j| j.name == op));
    for role in [None, Some("operator"), Some("advisor")] {
        let out = crate::agent_launch::render_launch_due_jobs_section(tmp.path(), role, "claude");
        assert!(!out.contains(op), "{role:?}: {out}");
    }
    let out = crate::agent_launch::render_launch_due_jobs_section(tmp.path(), None, "claude");
    assert!(out.contains(adv), "{out}");
}
