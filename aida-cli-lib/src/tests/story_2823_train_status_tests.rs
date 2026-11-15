//! `aida pr train --status`: the status is read from the journal directory
//! with scripted process and CI probes.
// trace:STORY-2823 | ai:claude
use super::*;

const NOW: &str = "2026-11-14T12:00:00Z";

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(NOW)
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn built(prs: &[u64]) -> Built {
    Built {
        base: "b".repeat(40),
        tip: "c".repeat(40),
        cars: prs
            .iter()
            .map(|pr| Car {
                member: Member {
                    pr: *pr,
                    head: format!("{pr:040}"),
                    ..Default::default()
                },
                commit: "d".repeat(40),
                patch_id: "e".repeat(40),
            })
            .collect(),
        dropped: Vec::new(),
    }
}

/// Journal a two-member train at `step` as train PR #900, then drop the
/// in-process handle the way a dead driver would.
fn journal_train(dir: &Path, step: journal::Step) {
    let mut entry = journal::Entry::new(built(&[11, 12]), "train/abc-1".into());
    entry.change_id = Some(900);
    entry.step = step;
    journal::activate(dir, entry).unwrap();
    journal::before("watch CI").unwrap();
    journal::clear();
}

fn row(name: &str, bucket: &str) -> crate::ci_gate::CheckRow {
    crate::ci_gate::CheckRow {
        name: name.into(),
        workflow: "CI".into(),
        bucket: bucket.into(),
        run_id: Some("77".into()),
    }
}

use aida_core::proc_identity::{DeadReason, UnknownReason};

fn alive() -> Option<Verdict> {
    Some(Verdict::Alive)
}
fn dead() -> Option<Verdict> {
    Some(Verdict::Dead(DeadReason::Absent))
}
fn foreign() -> Option<Verdict> {
    Some(Verdict::ForeignUnknown(UnknownReason::OtherNode))
}

fn status_with(
    dir: &Path,
    verdict: fn() -> Option<Verdict>,
    ci: &dyn Fn(u64) -> std::result::Result<Vec<crate::ci_gate::CheckRow>, String>,
) -> TrainStatus {
    collect(
        dir,
        &Probes {
            now: now(),
            observe: &|_| verdict(),
            ci,
        },
    )
    .unwrap()
}

fn no_ci(_: u64) -> std::result::Result<Vec<crate::ci_gate::CheckRow>, String> {
    Ok(Vec::new())
}

#[test]
fn no_journal_and_no_driver_is_no_train() {
    let dir = tempfile::tempdir().unwrap();
    // A directory that does not exist yet reads the same as an empty one.
    for dir in [dir.path().to_path_buf(), dir.path().join("absent")] {
        let status = status_with(&dir, dead, &no_ci);
        assert_eq!(status.state, State::None);
        assert_eq!(status.exit_code, NO_TRAIN_EXIT_CODE);
        assert!(status.driver.is_none() && status.trains.is_empty());
        assert!(
            render(&status).starts_with("No train"),
            "{}",
            render(&status)
        );
    }
    assert!(
        !dir.path().join("absent").exists(),
        "status creates nothing"
    );
}

#[test]
fn a_held_lock_reports_the_recorded_driver_pid_and_the_train() {
    let dir = tempfile::tempdir().unwrap();
    let _lock = journal::lock(dir.path()).unwrap();
    journal_train(dir.path(), journal::Step::Ci);

    let status = status_with(dir.path(), alive, &|pr| {
        assert_eq!(pr, 900);
        Ok(vec![
            row("CI (1/3)", "pass"),
            row("CI (2/3)", "pending"),
            row("CI (3/3)", "fail"),
            row("Build (ubuntu-latest)", "pending"),
        ])
    });
    assert_eq!(status.state, State::Running);
    assert_eq!(status.exit_code, 0);
    // The sandbox may refuse to capture this process's identity; the pid is
    // whatever the driver record holds.
    let recorded = journal::driver_record(dir.path()).unwrap();
    let pid = recorded.holder.identity().map(|id| id.pid);
    assert_eq!(status.driver.as_ref().unwrap().pid, pid);
    if let Some(pid) = pid {
        assert_eq!(pid, std::process::id());
    }

    let train = &status.trains[0];
    assert_eq!(train.train_pr, Some(900));
    assert_eq!(train.journal, "abc-1.json");
    assert_eq!(train.step, "Ci");
    assert_eq!(train.last_call, "watch CI");
    assert_eq!(
        train
            .members
            .iter()
            .map(|m| (m.pr, m.head.clone(), m.state))
            .collect::<Vec<_>>(),
        vec![
            (11, format!("{:040}", 11), "riding"),
            (12, format!("{:040}", 12), "riding"),
        ]
    );
    let ci = train.ci.as_ref().unwrap();
    assert_eq!(
        (ci.checks, ci.passed, ci.failed, ci.pending),
        (4, 1, 1, 2),
        "{ci:?}"
    );
    assert_eq!((ci.shards, ci.shards_passed), (3, 1));

    let text = render(&status);
    if let Some(pid) = pid {
        assert!(text.contains(&format!("driver PID {pid}")), "{text}");
    }
    assert!(text.contains("train PR #900 (train/abc-1)  step Ci for "));
    assert!(text.contains("#11 @ 0000000000"), "{text}");
    assert!(text.contains("CI: 1/3 shards passed"), "{text}");
    assert!(
        !text.contains("--abandon"),
        "a live train needs no recovery"
    );
}

#[test]
fn a_journal_whose_recorded_driver_is_dead_is_orphaned() {
    let dir = tempfile::tempdir().unwrap();
    drop(journal::lock(dir.path()).unwrap());
    journal_train(dir.path(), journal::Step::Merge);

    let status = status_with(dir.path(), dead, &|_| Err("gh is not installed".into()));
    assert_eq!(status.state, State::Orphaned);
    assert_eq!(status.exit_code, ORPHANED_EXIT_CODE);
    assert!(status.driver.is_none());
    assert_eq!(status.trains[0].step, "Merge");
    assert_eq!(
        status.trains[0].ci_unavailable.as_deref(),
        Some("gh is not installed")
    );
    let text = render(&status);
    assert!(text.contains("no live driver (orphaned journal)"), "{text}");
    assert!(text.contains("CI: unavailable (gh is not installed)"));
    assert!(
        text.contains("`aida pr train 11 12`") && text.contains("--abandon abc-1.json"),
        "{text}"
    );
}

/// A driver record this process cannot judge (another node or namespace)
/// falls back to the lock itself: held means running, free means orphaned.
#[test]
fn an_unjudgeable_driver_record_is_decided_by_the_run_lock() {
    let dir = tempfile::tempdir().unwrap();
    journal_train(dir.path(), journal::Step::Gate);

    let lock = journal::lock(dir.path()).unwrap();
    let held = status_with(dir.path(), foreign, &no_ci);
    assert_eq!(held.state, State::Running);
    assert_eq!(held.driver.as_ref().unwrap().pid, None);
    assert!(render(&held).contains("PID is not visible"));

    drop(lock);
    for verdict in [foreign as fn() -> Option<Verdict>, || None] {
        let free = status_with(dir.path(), verdict, &no_ci);
        assert_eq!(free.state, State::Orphaned);
    }
}

/// A lock held by a driver older than the driver record still reads running.
#[test]
fn a_held_lock_with_no_driver_record_is_running() {
    let dir = tempfile::tempdir().unwrap();
    journal_train(dir.path(), journal::Step::Ci);
    let _lock = journal::lock(dir.path()).unwrap();
    std::fs::write(dir.path().join("run.lock"), b"").unwrap();
    // The verdict must not be consulted without a record.
    let status = status_with(dir.path(), dead, &no_ci);
    assert_eq!(status.state, State::Running);
    assert_eq!(status.driver.unwrap().pid, None);
}

#[test]
fn a_live_driver_before_any_journal_is_running() {
    let dir = tempfile::tempdir().unwrap();
    let _lock = journal::lock(dir.path()).unwrap();
    let status = status_with(dir.path(), alive, &no_ci);
    assert_eq!(status.state, State::Running);
    assert!(status.trains.is_empty());
    assert!(render(&status).contains("no train built yet"));
}

#[test]
fn time_in_step_restarts_when_the_step_changes_and_survives_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    journal_train(dir.path(), journal::Step::Ci);
    let path = dir.path().join("abc-1.json");
    let clock = journal::step_clock(&path).unwrap();
    assert_eq!(clock.step, "Ci");

    // Age the clock; a checkpoint in the same step must not reset it.
    let aged = journal::StepClock {
        step: "Ci".into(),
        since: "2026-11-14T11:48:00Z".into(),
    };
    std::fs::write(
        path.with_extension("step"),
        serde_json::to_vec(&aged).unwrap(),
    )
    .unwrap();
    let entry = journal::peek(dir.path()).unwrap().remove(0).1;
    journal::activate(dir.path(), entry.clone()).unwrap();
    journal::before("read CI rows").unwrap();
    journal::clear();
    assert_eq!(journal::step_clock(&path).unwrap(), aged);
    let status = status_with(dir.path(), dead, &no_ci);
    assert_eq!(status.trains[0].time_in_step_secs, Some(12 * 60));
    assert!(render(&status).contains("step Ci for 12m 0s"));

    // Entering the next step restarts it.
    let mut next = entry;
    next.step = journal::Step::Gate;
    journal::activate(dir.path(), next).unwrap();
    journal::clear();
    assert_eq!(journal::step_clock(&path).unwrap().step, "Gate");

    // A journal with no clock (written by an older binary) reports no time.
    std::fs::remove_file(path.with_extension("step")).unwrap();
    let status = status_with(dir.path(), dead, &no_ci);
    assert_eq!(status.trains[0].time_in_step_secs, None);
    assert!(render(&status).contains("step Gate for an unknown time"));
}

#[test]
fn landed_and_rejected_members_are_marked_and_json_carries_the_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut entry = journal::Entry::new(built(&[11, 12, 13]), "train/abc-1".into());
    entry.change_id = Some(900);
    entry.step = journal::Step::Members;
    journal::activate(dir.path(), entry).unwrap();
    journal::complete_member(11).unwrap();
    journal::rejected(&[Ejected {
        pr: 12,
        reason: "head moved".into(),
    }])
    .unwrap();
    journal::clear();

    let status = status_with(dir.path(), dead, &no_ci);
    let states: Vec<_> = status.trains[0]
        .members
        .iter()
        .map(|m| (m.pr, m.state, m.reason.as_deref()))
        .collect();
    assert_eq!(
        states,
        vec![
            (11, "landed", None),
            (12, "rejected", Some("head moved")),
            (13, "riding", None)
        ]
    );
    let json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&status).unwrap()).unwrap();
    assert_eq!(json["state"], "orphaned");
    assert_eq!(json["exit_code"], ORPHANED_EXIT_CODE);
    assert_eq!(json["driver"], serde_json::Value::Null);
    assert_eq!(json["trains"][0]["train_pr"], 900);
    assert_eq!(json["trains"][0]["step"], "Members");
    assert_eq!(json["trains"][0]["members"][1]["reason"], "head moved");
}

#[test]
fn status_changes_nothing_and_finishing_or_abandoning_removes_the_clock() {
    let dir = tempfile::tempdir().unwrap();
    journal_train(dir.path(), journal::Step::Ci);
    let listing = |dir: &Path| {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|f| {
                let f = f.unwrap();
                (f.file_name(), std::fs::read(f.path()).unwrap())
            })
            .collect();
        files.sort();
        files
    };
    let before = listing(dir.path());
    status_with(dir.path(), dead, &no_ci);
    assert_eq!(listing(dir.path()), before);

    let clock = dir.path().join("abc-1.step");
    assert!(clock.exists());
    journal::abandon(dir.path(), "abc-1.json").unwrap();
    assert!(!clock.exists(), "abandon removes the step clock");

    journal_train(dir.path(), journal::Step::Ci);
    let mut done = journal::peek(dir.path()).unwrap().remove(0).1;
    done.step = journal::Step::Done;
    journal::activate(dir.path(), done).unwrap();
    assert!(!clock.exists(), "a finished train removes the step clock");
}

#[test]
fn the_exit_codes_reach_the_process_without_an_error_message() {
    for code in [NO_TRAIN_EXIT_CODE, ORPHANED_EXIT_CODE] {
        let err = anyhow::Error::new(TrainStatusExit(code)).context(crate::TypedPayloadEmitted);
        assert_eq!(crate::exit_code_for_error(&err), code);
        assert!(err.downcast_ref::<crate::TypedPayloadEmitted>().is_some());
    }
    assert_eq!(State::Running.exit_code(), 0);
}

#[test]
fn status_flag_stands_alone() {
    let parse =
        |args: &[&str]| crate::TEST_CLI.try_parse_from([&["aida", "pr", "train"], args].concat());
    assert!(parse(&["--status"]).is_ok());
    assert!(parse(&["--status", "--json"]).is_ok());
    for extra in [
        &["12"][..],
        &["--all-ready"],
        &["--dry-run"],
        &["--no-pull"],
        &["--abandon", "abc-1.json"],
    ] {
        assert!(
            parse(&[&["--status"], extra].concat()).is_err(),
            "{extra:?}"
        );
    }
}

// trace:STORY-2823 | ai:codex
#[test]
fn lock_occupancy_overrides_stale_or_live_driver_records() {
    let dir = tempfile::tempdir().unwrap();
    journal_train(dir.path(), journal::Step::Ci);
    let lock = journal::lock(dir.path()).unwrap();
    let held = status_with(dir.path(), dead, &no_ci);
    assert_eq!(held.state, State::Running);
    assert_eq!(held.driver.unwrap().pid, None);
    drop(lock);
    let free = status_with(dir.path(), alive, &no_ci);
    assert_eq!(free.state, State::Orphaned);
    assert!(free.driver.is_none());
}

// trace:STORY-2823 | ai:codex
#[test]
fn forge_classification_preserves_stale_config_and_bounds_origin_lookup() {
    use crate::forge::{train_status_forge_kind_with_origin, ForgeKind};
    use std::process::Command;
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".aida")).unwrap();
    let config = dir.path().join(".aida/config.toml");
    let body = "[forge]\nprovider = \"pure-git\"\n";
    std::fs::write(&config, body).unwrap();
    let mut origin = Command::new("printf");
    origin.arg("https://github.com/example/project.git");
    assert_eq!(
        train_status_forge_kind_with_origin(dir.path(), origin, Duration::from_secs(1)).unwrap(),
        ForgeKind::GitHub
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), body);
    let mut stalled = Command::new("sleep");
    stalled.arg("30");
    let started = Instant::now();
    assert!(
        train_status_forge_kind_with_origin(dir.path(), stalled, Duration::from_millis(100))
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(std::fs::read_to_string(config).unwrap(), body);
}

// trace:STORY-2823 | ai:codex
#[test]
fn journal_retirement_after_listing_is_a_finite_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    journal_train(dir.path(), journal::Step::Ci);
    let path = dir.path().join("abc-1.json");
    let body = std::fs::read(&path).unwrap();
    let entries = journal::peek_snapshot(dir.path(), &|| {
        // Join a concurrent remover at the exact listing/open boundary.
        std::thread::scope(|scope| {
            scope
                .spawn(|| std::fs::remove_file(&path).unwrap())
                .join()
                .unwrap();
        });
        // A new receipt is outside this snapshot, so it is not rediscovered.
        let mut next: journal::Entry = serde_json::from_slice(&body).unwrap();
        next.branch = "train/next-1".into();
        std::fs::write(
            dir.path().join("next-1.json"),
            serde_json::to_vec(&next).unwrap(),
        )
        .unwrap();
    })
    .unwrap();
    assert!(entries.is_empty());
    assert_eq!(journal::peek(dir.path()).unwrap().len(), 1);
}

// trace:STORY-2823 | ai:codex
#[test]
fn journal_snapshot_still_refuses_corrupt_and_unsafe_receipts() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    std::fs::write(&path, "{").unwrap();
    assert!(journal::peek(dir.path()).is_err());
    std::fs::remove_file(&path).unwrap();
    // Even a dangling link must refuse rather than look like a retired file.
    symlink(dir.path().join("absent"), &path).unwrap();
    assert!(journal::peek(dir.path()).is_err());
    std::fs::remove_file(&path).unwrap();
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(journal::peek(dir.path()).is_err());
}

// trace:STORY-2823 | ai:codex
#[test]
fn journal_snapshot_refuses_an_exhausted_listing_budget() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..4097 {
        std::fs::write(dir.path().join(format!("{index}.step")), "").unwrap();
    }
    let err = journal::peek(dir.path()).unwrap_err();
    assert!(err.to_string().contains("snapshot budget"), "{err:#}");
}
