use super::*;
use crate::history::HistoryOpts;
use crate::history_cache::test_support;
use std::collections::HashSet;
use std::path::PathBuf;

// trace:STORY-1661 | ai:codex
pub(crate) struct Fixture {
    pub(crate) root: tempfile::TempDir,
    pub(crate) store: PathBuf,
    pub(crate) db: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        history_cache::set_test_serve_enabled(true);
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::create_dir(root.path().join(".aida")).unwrap();
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        let db = history_cache::history_db_path(&store);
        let fx = Self { root, store, db };
        fx.git(&["init", "-q", "-b", "aida-store"]);
        fx.git(&["config", "user.name", "Fixture"]);
        fx.git(&["config", "user.email", "fixture@example.invalid"]);
        fx.git(&["config", "commit.gpgsign", "false"]);
        fx.append(0);
        test_support::index(&fx.store, &fx.db, Budget::unbounded()).unwrap();
        fx
    }

    #[cfg(unix)]
    fn git(&self, args: &[&str]) {
        let output = std::process::Command::new("/usr/bin/git")
            .current_dir(&self.store)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub(crate) fn append(&self, n: usize) {
        let relative = aida_core::object_store::relative_object_path("TASK-1661").unwrap();
        let path = self.store.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!(
            "spec_id: TASK-1661\ntitle: History fixture\nreq_type: Task\nstatus: {}\npriority: Medium\nmodified_at: '2026-10-13T00:00:00Z'\ndescription: revision {n}\n",
            if n % 2 == 0 { "Draft" } else { "Approved" }
        )).unwrap();
        self.git(&["add", "-A"]);
        self.git(&[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "fixture update",
        ]);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        history_cache::set_test_serve_enabled(false);
        history_cache::set_catch_up_chunk(None);
    }
}

pub(crate) fn opts() -> HistoryOpts {
    HistoryOpts {
        limit: 1000,
        max_commits: 1000,
        max_commits_explicit: false,
        events_mode: true,
        id_filter: None,
        type_filter: None,
        author_filter: None,
        since: None,
        until: None,
        status_changes_only: true,
        shipped_only: false,
        to_status: None,
        from_status: None,
        opened_only: true,
        comments_only: false,
        oneline: false,
        archived_specs: HashSet::new(),
        archived_only_specs: None,
        deferred_specs: HashSet::new(),
        deferred_only_specs: None,
        exclude_meta: false,
    }
}

// trace:STORY-1661 | ai:codex
#[test]
fn tick_after_200_commits_reaches_zero_lag() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    for n in 1..=200 {
        fx.append(n);
    }
    history_cache::set_catch_up_chunk(Some(7));
    assert_eq!(
        run_tick(&fx.store).unwrap(),
        TickOutcome::Advanced {
            appended: 200,
            lag: 0
        }
    );
    let before = history_cache::index_pass_count();
    let answer = history_cache::serve_flow(&fx.store, &opts()).unwrap();
    assert!(!answer.events.is_empty());
    assert_eq!(
        history_cache::index_pass_count(),
        before,
        "the next flow needs no inline indexing"
    );
    assert_eq!(run_tick(&fx.store).unwrap(), TickOutcome::UpToDate);
}

// trace:STORY-1661 | ai:codex
#[test]
fn tick_is_single_flight_when_index_lock_held() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    fx.append(1);
    let stamp = test_support::meta(&fx.db, "updated_at");
    let lock = test_support::hold_lock(&fx.db);
    assert_eq!(run_tick(&fx.store).unwrap(), TickOutcome::Busy);
    assert_eq!(test_support::meta(&fx.db, "updated_at"), stamp);
    drop(lock);
    assert_eq!(
        run_tick(&fx.store).unwrap(),
        TickOutcome::Advanced {
            appended: 1,
            lag: 0
        }
    );
}

// trace:STORY-1661 | ai:codex
#[test]
fn tick_and_cache_refresh_run_concurrently_without_deadlock() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    fx.append(1);
    let lock = aida_core::db::cache_refresh::RefreshLock::try_acquire(
        &fx.root.path().join(".aida/cache.db"),
    )
    .unwrap()
    .unwrap();
    let store = fx.store.clone();
    let (send, recv) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        history_cache::set_test_serve_enabled(true);
        send.send(run_tick(&store)).unwrap();
        history_cache::set_test_serve_enabled(false);
    });
    let result = recv.recv_timeout(Duration::from_secs(5));
    drop(lock);
    worker.join().unwrap();
    assert_eq!(
        result
            .expect("history tick must finish while RefreshLock is held")
            .unwrap(),
        TickOutcome::Advanced {
            appended: 1,
            lag: 0
        }
    );
}

// trace:STORY-1661 | ai:codex
#[test]
fn tick_disabled_spawns_nothing() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "0");
    let fx = Fixture::new();
    let absent = fx.root.path().join("absent");
    assert_eq!(
        run_tick_at(&absent, &absent, RUNTIME_CAP).unwrap(),
        TickOutcome::Disabled
    );
    handle_tick(Some(&absent)).unwrap();
    assert!(!absent.exists());
    history_cache::set_test_serve_enabled(false);
    assert_eq!(run_tick(&fx.store).unwrap(), TickOutcome::Disabled);
}

// trace:STORY-1661 | ai:codex
#[test]
fn tick_does_not_build_missing_index() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    let absent = fx.root.path().join("not-built.db");
    assert_eq!(
        run_tick_at(&fx.store, &absent, RUNTIME_CAP).unwrap(),
        TickOutcome::NotBuilt
    );
    assert!(!absent.exists());
    assert!(!PathBuf::from(format!("{}.lock", absent.display())).exists());
}

// trace:STORY-1661 | ai:codex
#[test]
fn background_kept_chunk_clears_stall_counter() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    fx.append(1);
    test_support::set_meta(&fx.db, "catch_up_failures", "3");
    test_support::set_meta(
        &fx.db,
        "catch_up_stalled_at",
        &chrono::Utc::now().to_rfc3339(),
    );
    assert_eq!(
        run_tick(&fx.store).unwrap(),
        TickOutcome::Advanced {
            appended: 1,
            lag: 0
        }
    );
    assert!(test_support::meta(&fx.db, "catch_up_failures").is_none());
    fx.append(2);
    let before = history_cache::index_pass_count();
    history_cache::serve_flow(&fx.store, &opts()).unwrap();
    assert_eq!(
        history_cache::index_pass_count(),
        before + 1,
        "inline catch-up is re-enabled"
    );
}

// trace:STORY-1661 | ai:codex
#[test]
fn tick_deadline_stops_the_loop_and_reports_remaining_lag() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    fx.append(1);
    let stamp = test_support::meta(&fx.db, "updated_at");
    test_support::set_meta(&fx.db, "catch_up_failures", "3");
    assert_eq!(
        run_tick_at(&fx.store, &fx.db, Duration::ZERO).unwrap(),
        TickOutcome::Advanced {
            appended: 0,
            lag: 1
        }
    );
    assert_eq!(test_support::meta(&fx.db, "updated_at"), stamp);
    assert_eq!(
        test_support::meta(&fx.db, "catch_up_failures").as_deref(),
        Some("3")
    );
}

// trace:STORY-1661 | ai:codex
#[test]
fn history_cli_dispatch_skips_requirements_cache_and_busy_exits_zero() {
    use clap::Parser;
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    let cli = crate::Cli::try_parse_from([
        "aida",
        "cache",
        "refresh",
        "--history",
        "--if-behind",
        "--store",
        fx.store.to_str().unwrap(),
    ])
    .unwrap();
    let lock = test_support::hold_lock(&fx.db);
    crate::cache_cmd::early_refresh(&cli.command)
        .unwrap()
        .unwrap();
    drop(lock);
    assert!(!fx.root.path().join(".aida/cache.db").exists());
    for args in [
        vec!["aida", "cache", "refresh", "--if-behind"],
        vec!["aida", "cache", "refresh", "--history", "--worker"],
    ] {
        assert!(crate::Cli::try_parse_from(args).is_err());
    }
}

// trace:STORY-1661 | ai:codex
#[test]
fn tick_inherited_lock_window_is_a_successful_busy_skip() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    fx.append(1);
    let stamp = test_support::meta(&fx.db, "updated_at");
    let inherited = test_support::inherited_lock_copy(&fx.db);
    assert_eq!(run_tick(&fx.store).unwrap(), TickOutcome::Busy);
    handle_tick(Some(&fx.store)).unwrap();
    assert_eq!(test_support::meta(&fx.db, "updated_at"), stamp);
    fs2::FileExt::unlock(&inherited).unwrap();
    drop(inherited);
    assert_eq!(
        run_tick(&fx.store).unwrap(),
        TickOutcome::Advanced {
            appended: 1,
            lag: 0
        }
    );
}

// trace:STORY-1661 | ai:codex
#[test]
fn background_failure_does_not_increment_foreground_stall_counter() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    fx.append(1);
    test_support::set_meta(&fx.db, "catch_up_failures", "2");
    history_cache::fail_next_catch_up();
    assert!(run_tick(&fx.store).is_err());
    assert_eq!(
        test_support::meta(&fx.db, "catch_up_failures").as_deref(),
        Some("2")
    );
}

/// Index one more commit, keep its tip on `keep` (if given), then reset this
/// checkout's store HEAD to the commit before it.
// trace:STORY-1661 | ai:claude
fn move_head_behind_index(fx: &Fixture, keep: Option<&str>) {
    fx.append(1);
    assert_eq!(
        run_tick(&fx.store).unwrap(),
        TickOutcome::Advanced {
            appended: 1,
            lag: 0
        }
    );
    if let Some(branch) = keep {
        fx.git(&["branch", branch]);
    }
    fx.git(&["reset", "-q", "--hard", "HEAD~1"]);
}

// trace:STORY-1661 | ai:claude
fn assert_skip_without_write(fx: &Fixture) {
    let tip = test_support::meta(&fx.db, "tip_sha");
    let stamp = test_support::meta(&fx.db, "updated_at");
    assert!(tip.is_some() && stamp.is_some());
    assert_eq!(run_tick(&fx.store).unwrap(), TickOutcome::Skipped);
    handle_tick(Some(&fx.store)).unwrap();
    assert_eq!(test_support::meta(&fx.db, "tip_sha"), tip);
    assert_eq!(test_support::meta(&fx.db, "updated_at"), stamp);
}

// trace:STORY-1661 | ai:claude
#[test]
fn tick_skips_when_checkout_is_behind_shared_index() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    move_head_behind_index(&fx, Some("keep"));
    assert_skip_without_write(&fx);
}

// trace:STORY-1661 | ai:claude
#[test]
fn tick_skips_when_diverged_but_tip_is_live_elsewhere() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    move_head_behind_index(&fx, Some("keep"));
    fx.append(2);
    assert_skip_without_write(&fx);
}

// trace:STORY-1661 | ai:claude
#[test]
fn tick_still_errors_on_rewritten_history() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    move_head_behind_index(&fx, None);
    fx.append(2);
    let stamp = test_support::meta(&fx.db, "updated_at");
    let err = run_tick(&fx.store).unwrap_err().to_string();
    assert!(err.contains("aida cache rebuild --history"), "{err}");
    assert!(handle_tick(Some(&fx.store)).is_err());
    assert_eq!(test_support::meta(&fx.db, "updated_at"), stamp);
}

// trace:STORY-1661 | ai:claude
#[test]
fn tick_errors_on_replaced_root_even_when_a_branch_keeps_the_old_tip() {
    // The root check runs before liveness, as in the foreground refresh: a
    // branch still holds the pre-compact tip, but the history was replaced.
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    fx.git(&["branch", "keep-old", "HEAD"]);
    let out = std::process::Command::new("/usr/bin/git")
        .current_dir(&fx.store)
        .args(["commit-tree", "HEAD^{tree}", "-m", "squashed"])
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success());
    let squashed = String::from_utf8(out.stdout).unwrap().trim().to_string();
    fx.git(&["reset", "-q", "--soft", &squashed]);
    let tip = test_support::meta(&fx.db, "tip_sha");
    let err = run_tick(&fx.store).unwrap_err().to_string();
    assert!(err.contains("aida cache rebuild --history"), "{err}");
    assert!(handle_tick(Some(&fx.store)).is_err());
    assert_eq!(test_support::meta(&fx.db, "tip_sha"), tip);
}
