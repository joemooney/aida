//! STORY-1916 — the disk guard's emergency tier, stuck escalation and status.
//!
//! Every fixture is a tempdir; the usage probe, the clock and the delete are
//! injected, so nothing here reads or writes the real build filesystem.
// trace:STORY-1916 | ai:claude

use super::*;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const HOUR: Duration = Duration::from_secs(3600);

/// Set the mtime of `path` and everything under it to `at`.
fn age_tree(path: &Path, at: SystemTime) {
    let md = std::fs::symlink_metadata(path).unwrap();
    if md.is_dir() {
        for entry in std::fs::read_dir(path).unwrap().flatten() {
            age_tree(&entry.path(), at);
        }
    }
    std::fs::File::open(path).unwrap().set_modified(at).unwrap();
}

/// A main checkout with `target/{agent,worktrees}` in a tempdir. Returns
/// (main, cache_root).
fn layout(root: &Path) -> (PathBuf, PathBuf) {
    let main = root.join("main");
    std::fs::create_dir_all(main.join("target/agent")).unwrap();
    std::fs::write(main.join("target/agent/aida"), b"live").unwrap();
    std::fs::create_dir_all(main.join("target/worktrees")).unwrap();
    let cache_root = main.join("target/worktrees").canonicalize().unwrap();
    (main.canonicalize().unwrap(), cache_root)
}

/// A cache `<cache_root>/<name>` with a `debug/` profile holding `fill`
/// bytes, all written `ago` before now.
fn cache(cache_root: &Path, name: &str, fill: usize, ago: Duration) -> PathBuf {
    let dir = cache_root.join(name);
    std::fs::create_dir_all(dir.join("debug/deps")).unwrap();
    std::fs::write(dir.join("debug/deps/lib.rlib"), vec![0u8; fill]).unwrap();
    std::fs::write(dir.join("debug/.cargo-lock"), b"").unwrap();
    age_tree(&dir, SystemTime::now() - ago);
    dir
}

fn cfg() -> EmergencyConfig {
    EmergencyConfig {
        used_pct: 95,
        margin_pct: 5,
        idle_hours: 2,
    }
}

fn allow(main: &Path, cache_root: &Path) -> Allowlist {
    Allowlist::for_main(main, cache_root.to_path_buf(), None, &[])
}

fn candidate(path: &Path, reason: Reason, idle: Duration) -> Candidate {
    Candidate {
        path: path.to_path_buf(),
        bytes: 100,
        idle,
        reason,
        holder: (reason == Reason::IdleLiveLease).then(|| Holder {
            worktree: PathBuf::from("/wt/x"),
            scope: "STORY-1".into(),
            pid: Some(1),
        }),
    }
}

/// A used% probe that drops by `step` after each delete the test records.
struct Usage {
    pct: Cell<u8>,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_config_defaults_overrides_and_disable() {
    assert_eq!(
        emergency_config(None),
        EmergencyConfig {
            used_pct: 95,
            margin_pct: 5,
            idle_hours: 2
        }
    );
    let tuned: toml::Value = "[doctor.disk_headroom]\nemergency_used_pct = 90\nemergency_margin_pct = 10\nemergency_idle_hours = 6\n"
        .parse()
        .unwrap();
    let c = emergency_config(Some(&tuned));
    assert_eq!((c.used_pct, c.margin_pct, c.idle_hours), (90, 10, 6));
    assert_eq!(c.stop_below(), 80);
    let off: toml::Value = "[doctor.disk_headroom]\nemergency_used_pct = 0\n"
        .parse()
        .unwrap();
    assert!(!emergency_config(Some(&off)).enabled());
    let bogus: toml::Value =
        "[doctor.disk_headroom]\nemergency_used_pct = 400\nemergency_idle_hours = 0\n"
            .parse()
            .unwrap();
    let c = emergency_config(Some(&bogus));
    assert_eq!(
        (c.used_pct, c.idle_hours),
        (95, 2),
        "out of range falls back"
    );
}

// ---------------------------------------------------------------------------
// Allowlist
// ---------------------------------------------------------------------------

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_allowlist_accepts_only_direct_cache_children() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, root) = layout(tmp.path());
    let ok = cache(&root, "take-1", 10, 3 * HOUR);
    let a = allow(&main, &root);
    a.check(&ok).expect("a direct child cache is deletable");

    // The live dev build, the main target, the main checkout, a nested dir.
    for bad in [
        main.join("target/agent"),
        main.join("target"),
        main.clone(),
        ok.join("debug"),
        tmp.path().join("elsewhere"),
    ] {
        let err = a.check(&bad).expect_err("must refuse");
        assert!(err.to_string().contains("refusing to delete"), "{err}");
    }
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_allowlist_refuses_a_symlinked_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, root) = layout(tmp.path());
    let outside = tmp.path().join("precious");
    std::fs::create_dir_all(&outside).unwrap();
    let link = root.join("take-2");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let err = allow(&main, &root)
        .check(&link)
        .expect_err("symlink refused");
    assert!(err.to_string().contains("symlink"), "{err}");
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_allowlist_refuses_overlap_with_sources_and_live_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, root) = layout(tmp.path());
    let c = cache(&root, "odd", 10, 3 * HOUR);
    // A worktree source living inside the cache dir.
    let src = c.join("src-checkout");
    std::fs::create_dir_all(&src).unwrap();
    let wts = vec![WorktreeEntry {
        path: src.clone(),
        branch: Some("b".into()),
    }];
    let a = Allowlist::for_main(&main, root.clone(), None, &wts);
    assert!(
        a.check(&c).is_err(),
        "a cache containing a worktree source is refused"
    );

    // The live binary directory inside the cache dir (home placed there).
    let c2 = cache(&root, "home", 10, 3 * HOUR);
    std::fs::create_dir_all(c2.join(".local/share/aida/live")).unwrap();
    let a = Allowlist::for_main(&main, root.clone(), Some(&c2), &[]);
    let err = a.check(&c2).expect_err("live binary overlap refused");
    assert!(err.to_string().contains("protected path"), "{err}");
}

// ---------------------------------------------------------------------------
// Idle test and planning
// ---------------------------------------------------------------------------

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_a_fresh_file_deep_in_the_tree_blocks_deletion() {
    let tmp = tempfile::tempdir().unwrap();
    let (_main, root) = layout(tmp.path());
    let c = cache(&root, "take-3", 10, 5 * HOUR);
    // Cargo writes deep; the root's own mtime stays old.
    let deep = c.join("debug/deps/new.o");
    std::fs::write(&deep, b"x").unwrap();
    std::fs::File::open(c.join("debug/deps"))
        .unwrap()
        .set_modified(SystemTime::now() - 5 * HOUR)
        .unwrap();
    std::fs::File::open(&c)
        .unwrap()
        .set_modified(SystemTime::now() - 5 * HOUR)
        .unwrap();
    let survey = EmergencySurvey {
        caches: vec![CacheEntry {
            walk: walk_cache(&c),
            path: c.clone(),
        }],
        worktrees: Vec::new(),
        holders: Vec::new(),
        finished_branches: Default::default(),
        finished_prs: Default::default(),
        now: SystemTime::now(),
        idle: 2 * HOUR,
    };
    let (candidates, held) = plan(&survey);
    assert!(
        candidates.is_empty(),
        "a cache written just now is never a candidate"
    );
    assert_eq!(held.len(), 1);
    assert!(held[0].1.contains("idle window"), "{:?}", held[0]);
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_future_mtime_is_fresh() {
    let walk = CacheWalk {
        bytes: 1,
        newest: Some(SystemTime::now() + HOUR),
    };
    assert_eq!(idle_for(&walk, SystemTime::now()), None);
    assert_eq!(idle_for(&CacheWalk::default(), SystemTime::now()), None);
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_plan_orders_by_tier_then_oldest_idle() {
    let tmp = tempfile::tempdir().unwrap();
    let (_main, root) = layout(tmp.path());
    let gone = cache(&root, "gone", 10, 3 * HOUR);
    let merged = cache(&root, "merged-wt", 10, 4 * HOUR);
    let idle_old = cache(&root, "idle-old", 10, 9 * HOUR);
    let idle_new = cache(&root, "idle-new", 10, 3 * HOUR);
    let live = cache(&root, "live-wt", 10, 20 * HOUR);
    let wt = |n: &str, b: &str| WorktreeEntry {
        path: tmp.path().join("wts").join(n),
        branch: Some(b.into()),
    };
    let survey = EmergencySurvey {
        caches: [&gone, &merged, &idle_old, &idle_new, &live]
            .iter()
            .map(|p| CacheEntry {
                walk: walk_cache(p),
                path: p.to_path_buf(),
            })
            .collect(),
        worktrees: vec![
            wt("merged-wt", "done"),
            wt("idle-old", "a"),
            wt("idle-new", "b"),
            wt("live-wt", "c"),
        ],
        holders: vec![Holder {
            worktree: tmp.path().join("wts/live-wt"),
            scope: "STORY-9".into(),
            pid: Some(42),
        }],
        finished_branches: [("done".to_string(), Reason::Merged)].into_iter().collect(),
        finished_prs: Default::default(),
        now: SystemTime::now(),
        idle: 2 * HOUR,
    };
    let (candidates, _) = plan(&survey);
    let order: Vec<(String, Reason)> = candidates
        .iter()
        .map(|c| {
            (
                c.path.file_name().unwrap().to_string_lossy().into(),
                c.reason,
            )
        })
        .collect();
    assert_eq!(
        order,
        vec![
            ("merged-wt".into(), Reason::Merged),
            ("gone".into(), Reason::WorktreeGone),
            ("idle-old".into(), Reason::Idle),
            ("idle-new".into(), Reason::Idle),
            ("live-wt".into(), Reason::IdleLiveLease),
        ],
        "tier 0 first (oldest idle first within it), the live-held cache last"
    );
    assert_eq!(candidates[4].holder.as_ref().unwrap().scope, "STORY-9");
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_plan_maps_detached_take_worktree_to_its_closed_pr() {
    let tmp = tempfile::tempdir().unwrap();
    let (_main, root) = layout(tmp.path());
    let c = cache(&root, "take-2830", 10, 3 * HOUR);
    let survey = EmergencySurvey {
        caches: vec![CacheEntry {
            walk: walk_cache(&c),
            path: c,
        }],
        worktrees: vec![WorktreeEntry {
            path: tmp.path().join("take-2830"),
            branch: None,
        }],
        holders: Vec::new(),
        finished_branches: Default::default(),
        finished_prs: [(2830, Reason::Closed)].into_iter().collect(),
        now: SystemTime::now(),
        idle: 2 * HOUR,
    };
    let (candidates, _) = plan(&survey);
    assert_eq!(candidates[0].reason, Reason::Closed);
}

// ---------------------------------------------------------------------------
// Apply: stop condition, tier 2, busy, refusal
// ---------------------------------------------------------------------------

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_apply_stops_once_below_ceiling_minus_margin() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, root) = layout(tmp.path());
    let a = cache(&root, "a", 10, 9 * HOUR);
    let b = cache(&root, "b", 10, 8 * HOUR);
    let c = cache(&root, "c", 10, 7 * HOUR);
    let plan = vec![
        candidate(&a, Reason::WorktreeGone, 9 * HOUR),
        candidate(&b, Reason::Idle, 8 * HOUR),
        candidate(&c, Reason::Idle, 7 * HOUR),
    ];
    // 97% -> 93% after the first delete -> 89% after the second.
    let usage = Usage { pct: Cell::new(97) };
    let outcome = apply(
        &allow(&main, &root),
        &plan,
        &cfg(),
        SystemTime::now(),
        || Some(usage.pct.get()),
        |p| {
            std::fs::remove_dir_all(p)?;
            usage.pct.set(usage.pct.get() - 4);
            Ok(())
        },
        |_| {},
        |_| {},
    );
    assert!(!a.exists() && !b.exists(), "deleted until below 90%");
    assert!(c.exists(), "left warm once usage fell below the margin");
    assert_eq!(outcome.stopped, Stop::BelowMargin);
    assert_eq!(outcome.deleted.len(), 2);
    assert_eq!(outcome.used_pct_before, Some(97));
    assert_eq!(outcome.used_pct_after, Some(89));
    assert!(matches!(
        outcome.rows.first(),
        Some(LedgerRow::Delete {
            reason: Reason::WorktreeGone,
            used_pct_before: Some(97),
            ..
        })
    ));
    assert!(matches!(
        outcome.rows.last(),
        Some(LedgerRow::Run { deleted: 2, .. })
    ));
    assert!(
        main.join("target/agent/aida").is_file(),
        "live build untouched"
    );
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_tier2_runs_only_while_still_at_or_above_the_ceiling() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, root) = layout(tmp.path());
    let held = cache(&root, "held", 10, 9 * HOUR);
    let plan = vec![candidate(&held, Reason::IdleLiveLease, 9 * HOUR)];
    let mailed = RefCell::new(Vec::new());

    // 93%: above the margin, below the ceiling — tier 2 must not run.
    let outcome = apply(
        &allow(&main, &root),
        &plan,
        &cfg(),
        SystemTime::now(),
        || Some(93),
        |p| std::fs::remove_dir_all(p),
        |c| mailed.borrow_mut().push(c.path.clone()),
        |_| {},
    );
    assert!(held.exists());
    assert_eq!(outcome.stopped, Stop::Tier2BelowCeiling);
    assert!(mailed.borrow().is_empty());

    // 96%: still over the ceiling after tiers 0-1 — tier 2 runs and mails.
    let outcome = apply(
        &allow(&main, &root),
        &plan,
        &cfg(),
        SystemTime::now(),
        || Some(96),
        |p| std::fs::remove_dir_all(p),
        |c| mailed.borrow_mut().push(c.path.clone()),
        |_| {},
    );
    assert!(!held.exists());
    assert_eq!(outcome.deleted.len(), 1);
    assert_eq!(mailed.borrow().as_slice(), &[held.clone()]);
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_held_cargo_lock_skips_the_cache_as_busy() {
    use fs2::FileExt;
    let tmp = tempfile::tempdir().unwrap();
    let (main, root) = layout(tmp.path());
    let c = cache(&root, "building", 10, 9 * HOUR);
    // A cargo build holds the lock (its own open file description).
    let lock = std::fs::File::open(c.join("debug/.cargo-lock")).unwrap();
    lock.lock_exclusive().unwrap();
    let outcome = apply(
        &allow(&main, &root),
        &[candidate(&c, Reason::Idle, 9 * HOUR)],
        &cfg(),
        SystemTime::now(),
        || Some(99),
        |p| std::fs::remove_dir_all(p),
        |_| {},
        |_| {},
    );
    assert!(
        c.exists(),
        "a cache whose cargo lock is held is never deleted"
    );
    assert!(
        matches!(&outcome.rows[0], LedgerRow::Skip { why, .. } if why.contains("busy") && why.contains(".cargo-lock")),
        "{:?}",
        outcome.rows
    );
    FileExt::unlock(&lock).unwrap();
    assert_eq!(
        busy_reason(&c, SystemTime::now(), 2 * HOUR),
        None,
        "released lock, idle tree"
    );
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_a_write_since_planning_skips_the_cache_as_busy() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, root) = layout(tmp.path());
    let c = cache(&root, "restarted", 10, 9 * HOUR);
    // A build started between planning and the delete.
    std::fs::write(c.join("debug/deps/fresh.o"), b"x").unwrap();
    let why = busy_reason(&c, SystemTime::now(), 2 * HOUR).expect("busy");
    assert!(why.contains("60 s"), "{why}");
    let outcome = apply(
        &allow(&main, &root),
        &[candidate(&c, Reason::Idle, 9 * HOUR)],
        &cfg(),
        SystemTime::now(),
        || Some(99),
        |p| std::fs::remove_dir_all(p),
        |_| {},
        |_| {},
    );
    assert!(c.exists());
    assert!(outcome.deleted.is_empty());
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_a_refusal_stops_the_run_and_keeps_earlier_ledger_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, root) = layout(tmp.path());
    let valid = cache(&root, "valid", 10, 9 * HOUR);
    let agent = main.join("target/agent");
    let after = cache(&root, "after", 10, 9 * HOUR);
    let removed = RefCell::new(Vec::new());
    let outcome = apply(
        &allow(&main, &root),
        &[
            candidate(&valid, Reason::WorktreeGone, 9 * HOUR),
            // A forged entry reaching the live dev build.
            candidate(&agent, Reason::WorktreeGone, 9 * HOUR),
            candidate(&after, Reason::Idle, 9 * HOUR),
        ],
        &cfg(),
        SystemTime::now(),
        || Some(99),
        |p| {
            removed.borrow_mut().push(p.to_path_buf());
            std::fs::remove_dir_all(p)
        },
        |_| {},
        |_| {},
    );
    assert_eq!(
        removed.borrow().as_slice(),
        &[valid.clone()],
        "nothing after the refusal"
    );
    assert!(agent.join("aida").is_file() && after.exists());
    assert_eq!(outcome.stopped, Stop::Refused);
    assert!(outcome
        .refusal
        .as_deref()
        .is_some_and(|r| r.contains("refusing to delete")));
    // The delete before the refusal is still ledgered, then the refusal,
    // then the run summary.
    assert!(
        matches!(&outcome.rows[0], LedgerRow::Delete { path, .. } if path.ends_with("valid")),
        "{:?}",
        outcome.rows
    );
    assert!(
        matches!(&outcome.rows[1], LedgerRow::Fail { error, .. } if error.contains("refusing"))
    );
    assert!(
        matches!(&outcome.rows[2], LedgerRow::Run { stopped, deleted: 1, .. } if stopped == "refused")
    );
    let (title, body) = emergency_notice(&outcome, &cfg()).expect("a refusal is news");
    assert!(title.contains("refused"), "{title}");
    assert!(body.contains("Stopped by the allowlist"), "{body}");
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_apply_stops_when_the_probe_is_lost() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, root) = layout(tmp.path());
    let c = cache(&root, "a", 10, 9 * HOUR);
    let outcome = apply(
        &allow(&main, &root),
        &[candidate(&c, Reason::Idle, 9 * HOUR)],
        &cfg(),
        SystemTime::now(),
        || None,
        |p| std::fs::remove_dir_all(p),
        |_| {},
        |_| {},
    );
    assert!(c.exists());
    assert_eq!(outcome.stopped, Stop::ProbeLost);
}

// ---------------------------------------------------------------------------
// Notice, ledger, status
// ---------------------------------------------------------------------------

fn outcome_with(deleted: Vec<Candidate>, after: Option<u8>) -> RunOutcome {
    RunOutcome {
        rows: Vec::new(),
        freed_bytes: deleted.iter().map(|c| c.bytes).sum(),
        deleted,
        stopped: Stop::BelowMargin,
        used_pct_before: Some(97),
        used_pct_after: after,
        refusal: None,
    }
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_notice_only_when_something_happened_or_recovery_failed() {
    assert!(emergency_notice(&outcome_with(Vec::new(), Some(80)), &cfg()).is_none());
    let c = candidate(Path::new("/c/take-1"), Reason::Merged, HOUR);
    let (title, body) = emergency_notice(&outcome_with(vec![c], Some(85)), &cfg()).unwrap();
    assert!(title.contains("auto-reclaimed"), "{title}");
    assert!(
        body.contains("/c/take-1") && body.contains("merged"),
        "{body}"
    );
    let (title, body) = emergency_notice(&outcome_with(Vec::new(), Some(96)), &cfg()).unwrap();
    assert!(title.contains("still at 96%"), "{title}");
    assert!(body.contains("aida worktree reclaim"), "{body}");
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_ledger_round_trips_and_reports_the_last_run() {
    let tmp = tempfile::tempdir().unwrap();
    let ts = Utc::now();
    let rows = vec![
        LedgerRow::Delete {
            ts,
            path: "/c/a".into(),
            bytes: 5,
            reason: Reason::Idle,
            idle_secs: 7200,
            used_pct_before: Some(96),
        },
        LedgerRow::Run {
            ts,
            freed_bytes: 5,
            deleted: 1,
            used_pct_before: Some(96),
            used_pct_after: Some(88),
            stopped: "below-margin".into(),
        },
    ];
    append_ledger(tmp.path(), &rows).unwrap();
    let text = std::fs::read_to_string(ledger_path(tmp.path())).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(text.contains(r#""kind":"delete""#) && text.contains(r#""reason":"idle""#));
    let last = last_reclaim(tmp.path()).unwrap();
    assert_eq!(
        (last.freed_bytes, last.deleted, last.used_pct_after),
        (5, 1, Some(88))
    );
    assert!(last_reclaim(&tmp.path().join("none")).is_none());
}

// trace:STORY-1916.ac641c6e | ai:claude
#[test]
fn story_1916_stuck_escalation_dedupes_by_time_and_growth() {
    let now = Utc::now();
    assert!(should_escalate(&GuardState::default(), now, GIB));
    let state = GuardState {
        last_stuck_at: Some(now - chrono::Duration::hours(1)),
        last_stuck_deficit: Some(20 * GIB),
        ..GuardState::default()
    };
    assert!(
        !should_escalate(&state, now, 25 * GIB),
        "within 6h and grew < 10 GiB"
    );
    assert!(
        should_escalate(&state, now, 30 * GIB),
        "deficit grew 10 GiB"
    );
    assert!(should_escalate(
        &state,
        now + chrono::Duration::hours(6),
        20 * GIB
    ));

    let tmp = tempfile::tempdir().unwrap();
    save_guard_state(tmp.path(), &state).unwrap();
    assert_eq!(load_guard_state(tmp.path()), state);
}

// trace:STORY-1916.ac641c6e | ai:claude
#[test]
fn story_1916_stuck_summary_names_the_shortfall_and_top_consumers() {
    let text = stuck_summary(
        3 * GIB,
        40 * GIB,
        &[(PathBuf::from("/t/worktrees/take-2845"), 39 * GIB)],
    );
    assert!(text.starts_with("STUCK"), "{text}");
    assert!(text.contains("3.0 GiB of the 40.0 GiB"), "{text}");
    assert!(text.contains("39.0 GiB /t/worktrees/take-2845"), "{text}");
}

// trace:STORY-1916.ac641c6e | ai:claude
#[test]
fn story_1916_top_consumers_counts_each_worktree_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, root) = layout(tmp.path());
    cache(&root, "big", 5000, HOUR);
    cache(&root, "small", 10, HOUR);
    let top = top_consumers(&main, 2);
    assert_eq!(top.len(), 2);
    assert!(top[0].0.ends_with("big"), "{top:?}");
    assert!(
        top.iter().all(|(p, _)| !p.ends_with("worktrees")),
        "{top:?}"
    );
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_finished_rows_open_wins_and_merged_beats_closed() {
    let rows: Vec<serde_json::Value> = serde_json::from_str(
        r#"[
          {"number": 1, "headRefName": "reopened", "state": "CLOSED"},
          {"number": 2, "headRefName": "reopened", "state": "OPEN"},
          {"number": 3, "headRefName": "landed", "state": "CLOSED"},
          {"number": 4, "headRefName": "landed", "state": "MERGED"},
          {"number": 5, "headRefName": "dropped", "state": "CLOSED"}
        ]"#,
    )
    .unwrap();
    let (branches, numbers) = finished_from_rows(&rows);
    assert!(
        !branches.contains_key("reopened"),
        "an open PR keeps the branch live"
    );
    assert_eq!(branches.get("landed"), Some(&Reason::Merged));
    assert_eq!(branches.get("dropped"), Some(&Reason::Closed));
    assert_eq!(numbers.get(&1), Some(&Reason::Closed));
    assert!(!numbers.contains_key(&2));
}

// trace:STORY-1916.Visibility | ai:claude
#[test]
fn story_1916_probe_paths_adds_the_cache_fs_only_when_distinct() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, _) = layout(tmp.path());
    let same = |_: &Path| {
        Some(FsStats {
            total: 100,
            avail: 50,
            dev: 1,
        })
    };
    assert_eq!(probe_paths(&main, same).len(), 1, "one filesystem, one row");
    let target = main.join("target");
    let split = |p: &Path| {
        Some(FsStats {
            total: 100,
            avail: 4,
            dev: if p == target { 2 } else { 1 },
        })
    };
    let rows = probe_paths(&main, split);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].0, "build-cache");
    assert_eq!(rows[1].2.used_pct(), 96);
}

// trace:STORY-1916.Visibility | ai:claude
#[test]
fn story_1916_status_block_shows_floor_ceiling_last_run_and_reclaim() {
    let at = "2026-10-23T17:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let s = DiskStatus {
        filesystems: vec![FsStatus {
            role: "build-cache",
            path: "/mnt/build/aida-target".into(),
            free_bytes: 60 * GIB,
            total_bytes: 469 * GIB,
            used_pct: 87,
        }],
        floor_gib: 60,
        emergency: cfg(),
        last_guard_run: Some(at),
        last_guard_result: Some("ok".into()),
        last_reclaim: Some(LastReclaim {
            at,
            freed_bytes: 77 * GIB,
            deleted: 6,
            used_pct_after: Some(71),
            stopped: "below-margin".into(),
        }),
    };
    let text = render_status(&s);
    for want in [
        "build-cache",
        "60.0 GiB free of 469.0 GiB (87% used)",
        "floor        60 GiB",
        "ceiling      95% used",
        "down to 90%",
        "last run     2026-10-23 17:00 UTC (ok)",
        "freed 77.0 GiB across 6 cache(s), 71% after",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    let json = serde_json::to_value(&s).unwrap();
    assert_eq!(json["emergency"]["used_pct"], 95);
    assert_eq!(json["last_reclaim"]["deleted"], 6);

    let off = DiskStatus {
        emergency: EmergencyConfig {
            used_pct: 0,
            ..cfg()
        },
        last_guard_run: None,
        last_reclaim: None,
        ..s
    };
    let text = render_status(&off);
    assert!(
        text.contains("ceiling      off")
            && text.contains("never recorded")
            && text.contains("last reclaim none")
    );
}

// trace:STORY-1916.Visibility | ai:claude
#[test]
fn story_1916_fs_stats_reads_a_real_tempdir() {
    let tmp = tempfile::tempdir().unwrap();
    let s = fs_stats(tmp.path()).expect("statvfs on a tempdir");
    assert!(s.total > 0 && s.avail <= s.total);
    assert!(s.used_pct() <= 100);
}

// trace:STORY-1916.ac40b516 | ai:claude
#[test]
fn story_1916_emergency_notice_dedupes_a_repeating_nothing_deleted_run() {
    let now = Utc::now();
    let stuck = outcome_with(Vec::new(), Some(97));
    assert!(should_notify_emergency(&GuardState::default(), now, &stuck));
    let recent = GuardState {
        last_emergency_stuck_at: Some(now - chrono::Duration::minutes(30)),
        ..GuardState::default()
    };
    assert!(
        !should_notify_emergency(&recent, now, &stuck),
        "the next 30m tick stays quiet"
    );
    assert!(should_notify_emergency(
        &recent,
        now + chrono::Duration::hours(6),
        &stuck
    ));
    let deleted = outcome_with(
        vec![candidate(Path::new("/c/take-1"), Reason::Idle, HOUR)],
        Some(97),
    );
    assert!(
        should_notify_emergency(&recent, now, &deleted),
        "a delete is always news"
    );
}

/// TASK-1-483: a cross-target build holds `<cache>/<triple>/<profile>/
/// .cargo-lock`; the busy probe must find it one level deeper, not only the
/// mtime re-walk.
// trace:TASK-2272 | ai:claude (TASK-1-483)
#[test]
fn task_1_483_cross_target_cargo_lock_is_probed_too() {
    use fs2::FileExt;
    let tmp = tempfile::tempdir().unwrap();
    let (_main, root) = layout(tmp.path());
    let c = cache(&root, "cross", 10, 9 * HOUR);
    let triple = c.join("x86_64-unknown-linux-musl/release");
    std::fs::create_dir_all(&triple).unwrap();
    std::fs::write(triple.join(".cargo-lock"), b"").unwrap();
    age_tree(&c, SystemTime::now() - 9 * HOUR);
    let lock = std::fs::File::open(triple.join(".cargo-lock")).unwrap();
    lock.lock_exclusive().unwrap();
    let why = busy_reason(&c, SystemTime::now(), 2 * HOUR).expect("busy");
    assert!(why.contains(".cargo-lock"), "{why}");
    FileExt::unlock(&lock).unwrap();
    assert_eq!(
        busy_reason(&c, SystemTime::now(), 2 * HOUR),
        None,
        "released cross-target lock, idle tree"
    );
}

/// TASK-1-484: a refusal from a persistent plan bug re-notifies on the stuck
/// cadence, not every 30 m tick; a NEW refusal is news at once.
// trace:TASK-2272 | ai:claude (TASK-1-484)
#[test]
fn task_1_484_a_repeating_refusal_is_deduped_a_new_one_notifies() {
    let now = Utc::now();
    let mut refused = outcome_with(Vec::new(), Some(97));
    refused.refusal = Some("refusing to delete x".into());
    assert!(
        should_notify_emergency(&GuardState::default(), now, &refused),
        "a first refusal is news"
    );
    let recent = GuardState {
        last_emergency_stuck_at: Some(now - chrono::Duration::minutes(30)),
        last_emergency_refusal: Some("refusing to delete x".into()),
        ..GuardState::default()
    };
    assert!(
        !should_notify_emergency(&recent, now, &refused),
        "the same refusal 30 m later stays quiet"
    );
    assert!(
        should_notify_emergency(&recent, now + chrono::Duration::hours(6), &refused),
        "and re-notifies on the stuck cadence"
    );
    let mut other = outcome_with(Vec::new(), Some(97));
    other.refusal = Some("refusing to delete y".into());
    assert!(
        should_notify_emergency(&recent, now, &other),
        "a different refusal is news at once"
    );
}

/// TASK-1-482: the production glue — the lock, the gh / offline fallback,
/// the incremental ledger append (TASK-1-481) and the notice dedupe state
/// write — driven end to end with only the usage probe injected.
// trace:TASK-2272 | ai:claude (TASK-1-481, TASK-1-482)
#[test]
fn task_1_482_run_emergency_glue_locks_ledgers_and_dedupes() {
    use fs2::FileExt;
    let tmp = tempfile::tempdir().unwrap();
    let (main, root) = layout(tmp.path());
    let idle = cache(&root, "idle-a", 10, 9 * HOUR);
    let cfg = cfg();

    // Under the ceiling: no run, no lock taken, no ledger.
    assert!(run_emergency_probed(&main, &root, &cfg, || Some(50)).is_none());
    assert!(!ledger_path(&main).exists());

    // Over the ceiling: the idle cache goes, and every row was appended to
    // the ledger as it happened (the file matches the outcome row for row).
    let outcome = run_emergency_probed(&main, &root, &cfg, || Some(97)).expect("ran");
    assert_eq!(outcome.deleted.len(), 1);
    assert!(!idle.exists());
    let ledger = std::fs::read_to_string(ledger_path(&main)).unwrap();
    assert_eq!(ledger.lines().count(), outcome.rows.len());
    assert!(
        load_guard_state(&main).last_emergency_stuck_at.is_none(),
        "a run that deleted something sets no dedupe state"
    );

    // Nothing left to delete and still over the margin: the notice dedupe
    // state is written, and a tick 30 m later stays quiet (state unchanged).
    run_emergency_probed(&main, &root, &cfg, || Some(97)).expect("ran");
    let stuck_at = load_guard_state(&main)
        .last_emergency_stuck_at
        .expect("stuck state written");
    run_emergency_probed(&main, &root, &cfg, || Some(97)).expect("ran");
    assert_eq!(
        load_guard_state(&main).last_emergency_stuck_at,
        Some(stuck_at),
        "the repeat tick did not re-notify"
    );

    // Another holder of the emergency lock: the run steps aside.
    let lock = std::fs::File::create(main.join(".aida/disk-emergency.lock")).unwrap();
    lock.lock_exclusive().unwrap();
    assert!(run_emergency_probed(&main, &root, &cfg, || Some(97)).is_none());
}

// trace:STORY-1916.ac641c6e | ai:claude
#[test]
fn story_1916_target_on_device_matches_a_single_filesystem_host() {
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::tempdir().unwrap();
    let (main, _) = layout(tmp.path());
    let dev = std::fs::metadata(&main).unwrap().dev();
    assert!(
        target_on_device(&main, dev),
        "target/ on the project filesystem"
    );
    assert!(!target_on_device(&main, dev.wrapping_add(1)));
    assert!(!target_on_device(&tmp.path().join("no-such"), dev));
}
