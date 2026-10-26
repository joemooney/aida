//! BUG-2221: decisive hermetic regressions for the exact-query
//! CLOSED-negative ancestry proof behind `aida show`'s linkage commit scan.
//! Real temporary git DAGs; structural assertions (git-shim call records,
//! cache-file contents, probe outcomes) — no tight wall-clock assertions.
//! PATH-shim tests run in their own process via `run_path_isolated_test`.
// trace:BUG-2221 | ai:claude
use super::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn real_git() -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join("git"))
                .find(|p| p.is_file())
        })
        .expect("real git on PATH")
}

fn git(repo: &Path, args: &[&str]) -> String {
    // Resolved real git (fixtures later install PATH shims), with automatic
    // maintenance off like the crate's `test_git` helper.
    let out = std::process::Command::new(real_git())
        .args(aida_core::git_ops::NO_AUTO_MAINTENANCE)
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A fresh repo on `main` with identity config and one base commit (no spec
/// references).
fn init_repo(dir: &Path) -> PathBuf {
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main", "--quiet"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(
        &repo,
        &["commit", "-q", "--allow-empty", "-m", "base: scaffolding"],
    );
    repo
}

fn commit(repo: &Path, msg: &str) -> String {
    git(repo, &["commit", "-q", "--allow-empty", "-m", msg]);
    git(repo, &["rev-parse", "HEAD"])
}

fn far_deadline() -> Instant {
    Instant::now() + Duration::from_secs(600)
}

/// Batch-proof tuning: cold keys BATCH (no oracle deferral) so the proof
/// machinery itself is what these fixtures exercise.
fn tuning(batch_max: usize, max_batches: usize) -> ProbeTuning {
    ProbeTuning {
        deadline: far_deadline(),
        batch_deadline: far_deadline(),
        batch_max,
        max_batches,
        cold_oracle: false,
    }
}

/// Production-shaped tuning: a cold key defers to the original collector.
fn oracle_tuning() -> ProbeTuning {
    ProbeTuning {
        cold_oracle: true,
        ..tuning(1024, usize::MAX)
    }
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn probe(repo: &Path, queries: &[&str], t: &ProbeTuning) -> NegativeProbe {
    probe_negative_linkage_tuned(repo, &ids(queries), t)
}

/// The state file for (repo context, queries) — None when absent.
fn state_on_disk(repo: &Path, queries: &[&str]) -> Option<NegativeState> {
    let loc = locate(repo, far_deadline()).expect("fixture repo locates");
    let path = state_path(&loc, &query_key(&ids(queries)));
    let bytes = std::fs::read(path).ok()?;
    Some(serde_json::from_slice(&bytes).expect("state file parses"))
}

fn namespace_dir(repo: &Path) -> PathBuf {
    cache_dir(&locate(repo, far_deadline()).expect("fixture repo locates"))
}

/// Run the production cold path twice the way two ordinary shows do — the
/// first marks the key, the second is the snapshot-pinned seeding show —
/// settling each deferral with a complete zero-reference oracle answer.
fn seed_key(repo: &Path, queries: &[&str]) {
    let negative = || OracleAnswer {
        complete: true,
        references: 0,
        records: Vec::new(),
    };
    let (_, mark) = probe_inner(repo, &ids(queries), &oracle_tuning());
    assert!(
        matches!(mark, Some(OracleSeed::Mark { .. })),
        "cold key marks"
    );
    assert!(settle_oracle(
        repo,
        mark.unwrap(),
        negative(),
        far_deadline()
    ));
    let (_, seed) = probe_inner(repo, &ids(queries), &oracle_tuning());
    assert!(
        matches!(seed, Some(OracleSeed::Seed { .. })),
        "marked key seeds"
    );
    assert!(settle_oracle(
        repo,
        seed.unwrap(),
        negative(),
        far_deadline()
    ));
}

/// A logging git shim on PATH (records every argv line to the returned log
/// file). Hold the guard for the shim's lifetime.
#[cfg(unix)]
fn install_logging_shim(tmp: &Path) -> (PathBuf, crate::test_env::EnvVarGuard) {
    let real = real_git();
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let log_path = tmp.join("git-calls.log");
    crate::test_exec::write_executable(
        &shim_dir.join("git"),
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log_path.display(),
            real.display()
        ),
    );
    let mut paths = vec![shim_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let guard = crate::test_env::EnvVarGuard::set("PATH", std::env::join_paths(paths).unwrap());
    (log_path, guard)
}

/// CLOSED requires the commit AND every parent proved; a negative tip alone
/// closes nothing, and closure propagates child-ward only from proved-closed
/// parents (never inferred from a negative tip down its ancestry).
#[test]
fn negative_proof_requires_every_parent_closed() {
    // Pure closure-rule unit: c1 <- c2 <- c3.
    let mut state = NegativeState::default();
    state.absorb(vec![("c3".into(), vec!["c2".into()])]);
    assert!(state.closed.is_empty(), "a negative tip must not close");
    assert!(state.open.contains_key("c3"));
    state.absorb(vec![("c2".into(), vec!["c1".into()])]);
    assert!(state.closed.is_empty(), "open ancestry keeps children open");
    state.absorb(vec![("c1".into(), vec![])]);
    assert_eq!(
        state.closed.len(),
        3,
        "a parentless negative closes and propagates to proved children"
    );
    assert!(state.open.is_empty());

    // End-to-end on a real DAG (chain + merge): the full probe proves empty
    // and persists every commit CLOSED.
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: unrelated work");
    git(&repo, &["checkout", "-q", "-b", "side"]);
    commit(&repo, "chore: side work");
    git(&repo, &["checkout", "-q", "main"]);
    git(
        &repo,
        &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
    );
    git(&repo, &["tag", "-a", "-m", "annotated", "v1"]);
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    let state = state_on_disk(&repo, &["TASK-7"]).expect("state persisted");
    let all: Vec<String> = git(&repo, &["rev-list", "--all"])
        .lines()
        .map(str::to_string)
        .collect();
    for oid in &all {
        assert!(
            state.closed.contains(oid),
            "{oid} must be CLOSED in the persisted proof"
        );
    }
    assert!(
        state.open.is_empty(),
        "a complete proof leaves nothing open"
    );
}

/// A cold proof makes bounded progress, persists it, and never publishes a
/// false complete bit; the SAME shape resumed later finishes.
#[test]
fn cold_batches_resume_without_false_complete() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    for i in 0..5 {
        commit(&repo, &format!("feat: step {i}"));
    }
    // One two-commit discovery batch per probe call.
    let t = tuning(2, 1);
    assert_eq!(
        probe(&repo, &["TASK-7"], &t),
        NegativeProbe::Incomplete,
        "a partial proof must read incomplete, not complete-empty"
    );
    let first = state_on_disk(&repo, &["TASK-7"]).expect("partial progress persists");
    let first_nodes = first.closed.len() + first.open.len();
    assert!(first_nodes >= 1, "the batch's evidence must persist");
    assert!(
        !first.open.is_empty() || first.closed.len() < 6,
        "6 commits cannot all close after one 2-commit batch"
    );
    // Resume: progress grows monotonically.
    assert_eq!(probe(&repo, &["TASK-7"], &t), NegativeProbe::Incomplete);
    let second = state_on_disk(&repo, &["TASK-7"]).unwrap();
    assert!(
        second.closed.len() + second.open.len() > first_nodes,
        "a resumed probe must extend the proof, not restart it"
    );
}

/// A stable finite DAG completes over repeated ordinary requests (measurable
/// eventual progress — advisor amendment 3), within a bounded number of
/// probes, and the completed answer is the exact negative.
#[test]
fn stable_dag_eventually_completes_over_repeated_shows() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    for i in 0..7 {
        commit(&repo, &format!("feat: step {i}"));
    }
    let t = tuning(1, 1); // worst case: one commit per show
    let mut outcomes = Vec::new();
    for _ in 0..16 {
        let outcome = probe(&repo, &["TASK-7"], &t);
        outcomes.push(outcome.clone());
        if outcome == NegativeProbe::ProvedEmpty {
            break;
        }
        assert_eq!(
            outcome,
            NegativeProbe::Incomplete,
            "cold progress must be truthful incomplete until the proof closes"
        );
    }
    assert_eq!(
        *outcomes.last().unwrap(),
        NegativeProbe::ProvedEmpty,
        "a stable 8-commit DAG must close within 16 one-commit probes: {outcomes:?}"
    );
}

/// Warm complete proof: NO ancestry log at all — no `--grep` and no
/// `--date-order` git invocation happens on the warm read (structural shim
/// record, not a timing assertion).
#[cfg(unix)]
#[test]
fn warm_negative_skips_all_ref_history() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: more work");
    // Cold proof with the REAL git (shim not yet installed).
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    // Install a logging git shim, then re-probe warm.
    let (log_path, _path) = install_logging_shim(tmp.path());

    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    let calls = std::fs::read_to_string(&log_path).expect("shim recorded calls");
    assert!(!calls.trim().is_empty(), "the warm probe still ran git");
    for line in calls.lines() {
        assert!(
            !line.contains("--grep"),
            "warm proof must not re-grep history: {line}"
        );
        assert!(
            !line.contains("--date-order"),
            "warm proof must not run the original full walk: {line}"
        );
    }
    assert!(
        calls
            .lines()
            .any(|l| l.contains("--no-walk=unsorted") && l.contains("--all")),
        "the warm proof still snapshots the actual roots"
    );
}

/// New coverage-breaking tips — a brand-new ref AND a detached HEAD in
/// another linked worktree (reachable from nothing else) — must break the
/// proof until they are themselves proved. `rev-parse --all` would miss the
/// detached HEAD; the probe's Git-owned root listing must not.
#[test]
fn new_ref_and_detached_other_worktree_head_break_coverage() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: base work");
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );

    // (a) A new branch with a new commit: a zero-batch probe may not claim
    // the proof still covers everything.
    git(&repo, &["checkout", "-q", "-b", "feat-x"]);
    commit(&repo, "feat: new unproved work");
    git(&repo, &["checkout", "-q", "main"]);
    assert_ne!(
        probe(&repo, &["TASK-7"], &tuning(1024, 0)),
        NegativeProbe::ProvedEmpty,
        "a new root must invalidate warm coverage until proved"
    );
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty,
        "proving the new cone restores the exact negative"
    );

    // (b) A commit reachable ONLY via another worktree's detached HEAD.
    git(&repo, &["checkout", "-q", "-b", "throwaway"]);
    let orphan_tip = commit(&repo, "feat: only reachable via detached head");
    git(&repo, &["checkout", "-q", "main"]);
    let wt = tmp.path().join("wt-detached");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            wt.to_str().unwrap(),
            &orphan_tip,
        ],
    );
    git(&repo, &["branch", "-q", "-D", "throwaway"]);
    assert_ne!(
        probe(&repo, &["TASK-7"], &tuning(1024, 0)),
        NegativeProbe::ProvedEmpty,
        "an other-worktree detached HEAD is a real root and must break coverage"
    );
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
}

/// Rewinding/deleting refs only SHRINKS the admitted root set: the
/// content-addressed negative cones are reused and no history grep runs.
#[cfg(unix)]
#[test]
fn rewind_and_deleted_ref_reuse_only_negative_cones() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: step 1");
    let keep_tip = commit(&repo, "feat: step 2");
    git(&repo, &["checkout", "-q", "-b", "doomed"]);
    commit(&repo, "feat: doomed work");
    git(&repo, &["checkout", "-q", "main"]);
    commit(&repo, "feat: step 3");
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );

    // Rewind main and delete the branch.
    git(&repo, &["update-ref", "refs/heads/main", &keep_tip]);
    git(&repo, &["reset", "-q", "--hard", &keep_tip]);
    git(&repo, &["branch", "-q", "-D", "doomed"]);

    let real = real_git();
    let shim_dir = tmp.path().join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let log_path = tmp.path().join("git-calls.log");
    crate::test_exec::write_executable(
        &shim_dir.join("git"),
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log_path.display(),
            real.display()
        ),
    );
    let mut paths = vec![shim_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let _path = crate::test_env::EnvVarGuard::set("PATH", std::env::join_paths(paths).unwrap());

    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty,
        "the surviving roots' cones were already proved"
    );
    let calls = std::fs::read_to_string(&log_path).expect("shim recorded calls");
    assert!(
        calls.lines().all(|l| !l.contains("--grep")),
        "rewind/deletion must reuse proved cones, not re-grep: {calls}"
    );
}

/// A real parsed hit routes to the ORIGINAL collector: date-order, parser
/// filtering and the bookkeeping-commit exclusion are byte-identical to the
/// unbounded oracle — including a referencing commit reachable from BOTH the
/// store-like root and the code root (where a naive store-ancestry
/// subtraction would silently lose it).
#[test]
fn positive_hit_uses_original_date_order() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: unrelated");
    // The real referencing commit — reachable from main AND (below) from the
    // orphan-store-like branch stacked on top of it.
    git(
        &repo,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fix: real work (TASK-7)",
        ],
    );
    let hit_one = git(&repo, &["rev-parse", "HEAD"]);
    // Store-like branch stacked on the hit commit, with bare-mention
    // bookkeeping commits (grep-positive, parser-negative).
    git(&repo, &["checkout", "-q", "-b", "aida-store"]);
    commit(&repo, "update TASK-7");
    commit(&repo, "update TASK-7 again");
    git(&repo, &["checkout", "-q", "main"]);
    // A second, later referencing commit so ordering is observable.
    git(
        &repo,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "feat: follow-up work (TASK-7)",
        ],
    );
    let hit_two = git(&repo, &["rev-parse", "HEAD"]);

    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::PositiveHit,
        "a parsed hit must defer to the original positive path"
    );

    // Integrated show path (deadline) vs. the non-show oracle (no probe).
    let wanted = ids(&["TASK-7"]);
    let shown = collect_git_linkage_until(&repo, &wanted, Instant::now() + Duration::from_secs(30));
    let oracle = collect_git_linkage_opts(&repo, &wanted, false);
    assert_eq!(
        shown.commits, oracle.commits,
        "the probe-fronted show path must keep the oracle's commits and order"
    );
    let shas: Vec<&str> = shown
        .commits
        .iter()
        .map(|(full, _, _)| full.as_str())
        .collect();
    assert_eq!(
        shas,
        vec![hit_two.as_str(), hit_one.as_str()],
        "date-order newest-first, bookkeeping bare mentions excluded"
    );
    assert!(!shown.commits_scan_incomplete);
}

/// Bare mentions (grep hit, parser reject) and case variants (the original
/// `-F --grep` is case-sensitive) produce the SAME answer as the original
/// Git oracle: an exact complete negative.
#[test]
fn bare_mention_and_case_filter_match_existing_git() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "update TASK-7"); // bare mention: parser rejects
    commit(&repo, "fix: lowercase (task-7)"); // case: Git's own grep rejects
    commit(&repo, "docs: mentions TASK-71 only"); // different spec id
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty,
        "grep-positive/parser-negative commits are exact negatives"
    );
    // Equivalence against the original collector.
    let wanted = ids(&["TASK-7"]);
    let shown = collect_git_linkage_until(&repo, &wanted, Instant::now() + Duration::from_secs(30));
    let oracle = collect_git_linkage_opts(&repo, &wanted, false);
    assert_eq!(shown.commits, oracle.commits);
    assert!(shown.commits.is_empty());
    assert!(!shown.commits_scan_incomplete, "the proof IS complete");
    assert!(
        !shown.shipped,
        "complete empty linkage justifies shipped=false"
    );
    assert!(!shown.shipped_unknown);

    // TASK-71 state is keyed separately: proving TASK-7 says nothing about
    // TASK-71 (which has a real parser-positive commit? no — bare subject
    // mention without trailer parens is also parser-negative; assert the
    // probe agrees with the oracle for it too).
    let shown71 = collect_git_linkage_until(
        &repo,
        &ids(&["TASK-71"]),
        Instant::now() + Duration::from_secs(30),
    );
    let oracle71 = collect_git_linkage_opts(&repo, &ids(&["TASK-71"]), false);
    assert_eq!(shown71.commits, oracle71.commits);
}

/// Refs moving while the proof runs: the before/after root snapshot differs,
/// the probe retries exactly once, then reports unknown (incomplete) rather
/// than publishing a complete negative pinned to a stale snapshot.
#[cfg(unix)]
#[test]
fn moving_refs_retry_once_then_unknown() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: base");

    // A shim that creates a NEW commit on every root-listing call, so every
    // before/after snapshot pair differs — a permanently moving fleet.
    let real = real_git();
    let shim_dir = tmp.path().join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let count_path = tmp.path().join("roots-calls");
    std::fs::write(&count_path, "").unwrap();
    crate::test_exec::write_executable(
        &shim_dir.join("git"),
        format!(
            "#!/bin/sh\nall=0; nw=0\nfor a in \"$@\"; do\n  [ \"$a\" = \"--all\" ] && all=1\n  [ \"$a\" = \"--no-walk=unsorted\" ] && nw=1\ndone\nif [ \"$all\" = 1 ] && [ \"$nw\" = 1 ]; then\n  echo x >> '{count}'\n  n=$(wc -l < '{count}')\n  '{real}' -c maintenance.auto=false -c gc.auto=0 -C '{repo}' commit -q --allow-empty -m \"fleet movement $n\" >/dev/null 2>&1\nfi\nexec '{real}' \"$@\"\n",
            count = count_path.display(),
            real = real.display(),
            repo = repo.display(),
        ),
    );
    let mut paths = vec![shim_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let _path = crate::test_env::EnvVarGuard::set("PATH", std::env::join_paths(paths).unwrap());

    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::Incomplete,
        "a moving snapshot must end unknown, never a stale complete negative"
    );
    let roots_calls = std::fs::read_to_string(&count_path)
        .unwrap()
        .lines()
        .count();
    assert_eq!(
        roots_calls, 3,
        "exactly one retry: before + after + retried-after root listings"
    );
}

/// Shallow / graft / replace semantics and config changes: the fast path
/// refuses (original query preserved) and a proof made under one state is
/// never reused under another.
#[test]
fn shallow_graft_replace_and_config_change_disable_reuse() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    let c1 = commit(&repo, "feat: one");
    let c2 = commit(&repo, "feat: two");
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );

    // Replace ref present → refuse.
    git(&repo, &["replace", "-f", &c2, &c1]);
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::RunOriginal,
        "active replacement refs must keep the original Git query"
    );
    git(&repo, &["replace", "-d", &c2]);

    // Grafts file → refuse.
    let grafts = repo.join(".git/info/grafts");
    std::fs::create_dir_all(grafts.parent().unwrap()).unwrap();
    std::fs::write(&grafts, format!("{c2}\n")).unwrap();
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::RunOriginal
    );
    std::fs::remove_file(&grafts).unwrap();

    // Shallow repo → refuse.
    let shallow = tmp.path().join("shallow-clone");
    let out = std::process::Command::new(real_git())
        .args([
            "clone",
            "--quiet",
            "--no-local",
            "--depth",
            "1",
            &format!("file://{}", repo.display()),
            shallow.to_str().unwrap(),
        ])
        .output()
        .expect("clone runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        probe(&shallow, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::RunOriginal,
        "shallow history must never be reinterpreted by the proof"
    );

    // Config change → different S → the old proof is NOT reused.
    git(&repo, &["config", "custom.bug2221", "changed"]);
    assert_ne!(
        probe(&repo, &["TASK-7"], &tuning(1024, 0)),
        NegativeProbe::ProvedEmpty,
        "a config change must invalidate snapshot identity"
    );
    // ...and a fresh proof under the new S converges again.
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
}

/// A replace-control override refuses the fast path (never
/// `--no-replace-objects` around it). Process-isolated: the variable would
/// otherwise leak into every concurrently running probe fixture.
#[cfg(unix)]
#[test]
fn replace_control_override_disables_reuse() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: one");
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    let _guard = crate::test_env::EnvVarGuard::set("GIT_NO_REPLACE_OBJECTS", "1");
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::RunOriginal
    );
}

/// A missing/corrupt object or an unreadable batch certifies nothing.
#[test]
fn missing_object_or_truncated_batch_never_certifies() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());

    // Direct batch-level check: a fabricated OID that exists nowhere.
    let wanted: std::collections::HashSet<String> = ["TASK-7".to_string()].into();
    let bogus = "0123456789abcdef0123456789abcdef01234567".to_string();
    let outcome = run_batch(
        &repo,
        &["TASK-7".to_string()],
        &wanted,
        &[bogus],
        &NegativeState::default(),
        1024,
        far_deadline(),
    );
    assert!(
        matches!(outcome, BatchOutcome::Failed),
        "an unresolvable OID must fail the whole batch"
    );

    // Whole-probe check on a corrupted repo: delete a loose object out from
    // under the walk; the probe must degrade, never publish a complete
    // negative.
    let c2 = commit(&repo, "feat: doomed object");
    commit(&repo, "feat: tip");
    let (d, f) = c2.split_at(2);
    let obj = repo.join(format!(".git/objects/{d}/{f}"));
    assert!(obj.exists(), "fixture commit must be a loose object");
    std::fs::remove_file(&obj).unwrap();
    let outcome = probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX));
    assert_ne!(
        outcome,
        NegativeProbe::ProvedEmpty,
        "missing history can never be certified negative"
    );
}

/// Cache failure modes degrade truthfully: corruption and symlinked/oversize
/// state fall back to uncached bounded work; a contended lock skips
/// publication without blocking; an oversize tombstone keeps the fast path
/// off deterministically.
#[cfg(unix)]
#[test]
fn cache_lock_corruption_and_size_limit_degrade() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: work");
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    let ContextSnapshot::Supported(ctx) = capture_context(&repo, None, far_deadline()).unwrap()
    else {
        panic!("supported");
    };
    let qk = query_key(&ids(&["TASK-7"]));
    let state_path = state_path(&ctx.loc, &qk);
    assert!(state_path.is_file());

    // Corruption: garbage JSON is ignored (no reuse, no crash) and a fresh
    // probe re-proves and replaces it.
    std::fs::write(&state_path, b"{ definitely not json").unwrap();
    assert_ne!(
        probe(&repo, &["TASK-7"], &tuning(1024, 0)),
        NegativeProbe::ProvedEmpty,
        "corrupt state must not read as a warm proof"
    );
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    let replaced: NegativeState =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).expect("replaced with valid");
    assert!(!replaced.closed.is_empty());

    // Symlinked state file: the no-follow read refuses it.
    let decoy = tmp.path().join("decoy.json");
    std::fs::write(&decoy, serde_json::to_vec(&replaced).unwrap()).unwrap();
    std::fs::remove_file(&state_path).unwrap();
    std::os::unix::fs::symlink(&decoy, &state_path).unwrap();
    assert_ne!(
        probe(&repo, &["TASK-7"], &tuning(1024, 0)),
        NegativeProbe::ProvedEmpty,
        "a symlinked cache file must never be trusted"
    );
    std::fs::remove_file(&state_path).unwrap();

    // Oversize file: refused on read.
    let big = vec![b' '; (MAX_STATE_BYTES + 1) as usize];
    std::fs::write(&state_path, &big).unwrap();
    assert_ne!(
        probe(&repo, &["TASK-7"], &tuning(1024, 0)),
        NegativeProbe::ProvedEmpty,
        "an over-limit cache file must be ignored"
    );
    std::fs::remove_file(&state_path).unwrap();

    // Oversize TOMBSTONE: deterministic off-switch for this context/query.
    let mut tomb = NegativeState {
        schema: SCHEMA_VERSION,
        s_digest: ctx.s_digest.clone(),
        query_digest: qk.digest.clone(),
        queries: qk.strings.clone(),
        ..Default::default()
    };
    tomb.oversize = true;
    std::fs::write(&state_path, serde_json::to_vec(&tomb).unwrap()).unwrap();
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::RunOriginal,
        "an oversize tombstone keeps the fast path off without churn"
    );
    std::fs::remove_file(&state_path).unwrap();

    // Contended lock: the proof still completes THIS invocation, publication
    // is skipped, and nothing blocks.
    let lock_path = cache_dir(&ctx.loc).join(".lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty,
        "a held cache lock must not change the probe's truthful answer"
    );
    assert!(
        !state_path.exists(),
        "publication must be skipped under contention, never waited on"
    );
    fs2::FileExt::unlock(&lock).unwrap();
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    assert!(
        state_path.exists(),
        "publication resumes once the lock frees"
    );
}

/// Aggregate namespace bounds: eviction is deterministic (oldest mtime, then
/// name), scoped to our own strictly-named regular files, and a flood of
/// distinct queries stays within the caps.
#[test]
fn aggregate_limits_evict_deterministically_under_query_flood() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("ns");
    std::fs::create_dir_all(&dir).unwrap();
    let name = |i: usize| format!("{:016x}-{:016x}.json", i, i);
    for i in 0..6 {
        std::fs::write(dir.join(name(i)), b"x").unwrap();
    }
    // Not ours: wrong shape + a subdirectory — both untouched.
    std::fs::write(dir.join("README"), b"keep").unwrap();
    std::fs::create_dir_all(dir.join("subdir")).unwrap();
    std::fs::write(dir.join("subdir/inner.json"), b"keep").unwrap();

    enforce_aggregate_bounds_with(&dir, 3, u64::MAX);
    let mut left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    left.sort();
    assert_eq!(
        left,
        vec![name(3), name(4), name(5), "README".to_string()],
        "equal-mtime ties evict by name, lowest first; foreign files untouched"
    );
    assert!(dir.join("subdir/inner.json").exists());

    // Byte bound too (each file is 1 byte).
    enforce_aggregate_bounds_with(&dir, 10, 1);
    let survivors: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file() && e.file_name().to_string_lossy().ends_with(".json"))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(survivors, vec![name(5)], "byte cap evicts to the newest");

    // End-to-end flood: distinct queries produce distinct files, all capped.
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: work");
    for i in 0..5 {
        let q = format!("TASK-{i}");
        assert_eq!(
            probe(&repo, &[q.as_str()], &tuning(1024, usize::MAX)),
            NegativeProbe::ProvedEmpty
        );
    }
    let ns = namespace_dir(&repo);
    let count = std::fs::read_dir(&ns)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .count();
    assert_eq!(count, 5, "one state file per query key");
    assert!(count <= MAX_CACHE_FILES);
}

/// Changing any of the three message parsers (or the grep construction)
/// REQUIRES bumping [`SCHEMA_VERSION`]: persisted proofs were made under the
/// pinned behavior below. If this test fails because parser behavior
/// changed, bump the schema const in the same change.
#[test]
fn parser_behavior_pin() {
    assert_eq!(SCHEMA_VERSION, 1, "bump reminder: see test doc comment");
    let trailer = "fix: thing (TASK-7)\n";
    assert!(extract_spec_ids_from_commit(trailer)
        .iter()
        .any(|id| id.eq_ignore_ascii_case("TASK-7")));
    let bare = "update TASK-7\n";
    let mut resolved = extract_spec_ids_from_commit(bare);
    resolved.extend(extract_referenced_spec_ids_from_commit(bare));
    resolved.extend(extract_trace_line_spec_ids(bare));
    assert!(
        !resolved.iter().any(|id| id.eq_ignore_ascii_case("TASK-7")),
        "a bare subject mention must stay parser-negative"
    );
    let trace_line = "body\n\ntrace:TASK-7 | ai:claude\n";
    assert!(extract_trace_line_spec_ids(trace_line)
        .iter()
        .any(|id| id.eq_ignore_ascii_case("TASK-7")));
    let referenced = "fix follow-up\n\nRefs TASK-7\n";
    let _ = extract_referenced_spec_ids_from_commit(referenced); // shape pin only
}

/// Production cold path: a key with no state file defers to the ORIGINAL
/// collector with NO snapshot (positive and cold shows pay nothing in front
/// of the original query) and a complete zero-reference answer only MARKS
/// it. The next show captures S/R BEFORE the original runs; a complete
/// zero-reference answer against that unchanged snapshot seeds every
/// current root CLOSED — so the show after is a warm read with no batch. A
/// snapshot that moves between the probe and the seed publishes nothing.
#[test]
fn cold_oracle_seed_warms_key_only_on_unchanged_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "update TASK-7"); // grep-positive, parser-negative
    commit(&repo, "feat: tip");
    let negative = || OracleAnswer {
        complete: true,
        references: 0,
        records: Vec::new(),
    };

    let (outcome, mark) = probe_inner(&repo, &ids(&["TASK-7"]), &oracle_tuning());
    assert_eq!(
        outcome,
        NegativeProbe::RunOriginal,
        "cold defers to the original"
    );
    assert!(
        matches!(mark, Some(OracleSeed::Mark { .. })),
        "a key with no file takes no snapshot"
    );
    assert!(
        state_on_disk(&repo, &["TASK-7"]).is_none(),
        "deferral writes nothing"
    );
    assert!(settle_oracle(
        &repo,
        mark.unwrap(),
        negative(),
        far_deadline()
    ));
    let marker = state_on_disk(&repo, &["TASK-7"]).expect("marker published");
    assert!(
        marker.closed.is_empty() && marker.open.is_empty() && marker.s_digest.is_empty(),
        "a marker proves nothing"
    );
    assert_ne!(
        probe(&repo, &["TASK-7"], &tuning(1024, 0)),
        NegativeProbe::ProvedEmpty,
        "a marker is never read as a proof"
    );

    let (outcome, seed) = probe_inner(&repo, &ids(&["TASK-7"]), &oracle_tuning());
    assert_eq!(outcome, NegativeProbe::RunOriginal);
    assert!(matches!(seed, Some(OracleSeed::Seed { .. })));
    assert!(settle_oracle(
        &repo,
        seed.unwrap(),
        negative(),
        far_deadline()
    ));
    let state = state_on_disk(&repo, &["TASK-7"]).expect("seed published");
    let tips: std::collections::BTreeSet<String> = git(
        &repo,
        &["log", "--all", "--no-walk=unsorted", "--pretty=format:%H"],
    )
    .lines()
    .map(str::to_string)
    .collect();
    assert_eq!(
        state.closed, tips,
        "the seed closes exactly the current roots"
    );
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, 0)),
        NegativeProbe::ProvedEmpty,
        "a seeded key is a warm complete read with zero batches"
    );

    // A ref moving between the seeding probe and the settle: nothing.
    let (_, mark) = probe_inner(&repo, &ids(&["TASK-8"]), &oracle_tuning());
    assert!(settle_oracle(
        &repo,
        mark.unwrap(),
        negative(),
        far_deadline()
    ));
    let (_, seed) = probe_inner(&repo, &ids(&["TASK-8"]), &oracle_tuning());
    let seed = seed.expect("marked TASK-8 key");
    commit(&repo, "feat: fleet moved meanwhile");
    assert!(
        !settle_oracle(&repo, seed, negative(), far_deadline()),
        "a moved root snapshot must not be seeded as complete"
    );
    assert!(state_on_disk(&repo, &["TASK-8"])
        .is_some_and(|st| st.closed.is_empty() && st.open.is_empty()));

    // Integrated: two cold shows answer COMPLETE (the oracle's own answer),
    // equal to the unbounded collector; the second seeds the key.
    let wanted = ids(&["TASK-9"]);
    let oracle = collect_git_linkage_opts(&repo, &wanted, false);
    for _ in 0..2 {
        let shown =
            collect_git_linkage_until(&repo, &wanted, Instant::now() + Duration::from_secs(30));
        assert_eq!(shown.commits, oracle.commits);
        assert!(
            !shown.commits_scan_incomplete,
            "cold show keeps the complete answer"
        );
    }
    assert!(
        state_on_disk(&repo, &["TASK-9"]).is_some_and(|st| !st.closed.is_empty()),
        "the seeding show warms the key"
    );
}

/// Ruling 2026-10-25 (a): only an oracle that ran to COMPLETION seeds. A
/// timed-out / failed / partially framed oracle — on a cold key or on a
/// seeding show — publishes nothing; a positive answer drops the key.
#[test]
fn incomplete_oracle_seeds_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: work");
    let partial = || OracleAnswer {
        complete: false,
        references: 0,
        records: Vec::new(),
    };
    let (_, mark) = probe_inner(&repo, &ids(&["TASK-7"]), &oracle_tuning());
    assert!(!settle_oracle(
        &repo,
        mark.unwrap(),
        partial(),
        far_deadline()
    ));
    assert!(
        state_on_disk(&repo, &["TASK-7"]).is_none(),
        "an incomplete oracle does not even mark"
    );

    seed_key(&repo, &["TASK-8"]);
    commit(&repo, "feat: newer work");
    let (_, mark) = probe_inner(&repo, &ids(&["TASK-9"]), &oracle_tuning());
    assert!(settle_oracle(
        &repo,
        mark.unwrap(),
        OracleAnswer {
            complete: true,
            references: 0,
            records: Vec::new(),
        },
        far_deadline()
    ));
    let (_, seed) = probe_inner(&repo, &ids(&["TASK-9"]), &oracle_tuning());
    assert!(!settle_oracle(
        &repo,
        seed.unwrap(),
        partial(),
        far_deadline()
    ));
    assert!(
        state_on_disk(&repo, &["TASK-9"]).is_some_and(|st| st.closed.is_empty()),
        "a seeding show with an incomplete oracle closes nothing"
    );

    // A positive oracle answer on a seeding show drops the key's state.
    let (_, seed) = probe_inner(&repo, &ids(&["TASK-9"]), &oracle_tuning());
    assert!(!settle_oracle(
        &repo,
        seed.unwrap(),
        OracleAnswer {
            complete: true,
            references: 1,
            records: Vec::new(),
        },
        far_deadline()
    ));
    assert!(
        state_on_disk(&repo, &["TASK-9"]).is_none(),
        "a positive key returns to the cheap cold path"
    );
}

/// After a seed (closed = roots only), new commits on top are proved by
/// framing ONLY the new commits: the discovery walk is fenced at the CLOSED
/// boundary instead of re-framing old history — and an old commit reached
/// below the fence closes by inclusion in a CLOSED cone.
#[test]
fn fenced_walk_frames_only_unproved_commits() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    let old_tip = commit(&repo, "feat: old work");
    seed_key(&repo, &["TASK-7"]);
    let new: Vec<String> = (0..3)
        .map(|i| commit(&repo, &format!("feat: new {i}")))
        .collect();
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    let state = state_on_disk(&repo, &["TASK-7"]).unwrap();
    let mut expected: std::collections::BTreeSet<String> = new.into_iter().collect();
    expected.insert(old_tip);
    assert_eq!(
        state.closed, expected,
        "only the new commits were framed; the base commit below the CLOSED \
         seed was never re-walked"
    );

    // A merge of a side branch forked from the BASE commit (an ancestor of
    // the CLOSED seed, never itself recorded). The walk below the merge is
    // fenced at the closed first parent, so `base` is excluded — and closes
    // by inclusion in a CLOSED cone instead of costing another batch.
    let base = git(&repo, &["rev-list", "--max-parents=0", "HEAD"]);
    git(&repo, &["checkout", "-q", "-b", "side", &base]);
    let side = commit(&repo, "feat: side work");
    git(&repo, &["checkout", "-q", "main"]);
    git(
        &repo,
        &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
    );
    let merge = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["branch", "-q", "-D", "side"]);
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, 1)),
        NegativeProbe::ProvedEmpty,
        "one batch suffices: the merge, its side parent and the fenced base"
    );
    let state = state_on_disk(&repo, &["TASK-7"]).unwrap();
    assert!(state.closed.contains(&merge));
    assert!(state.closed.contains(&side));
    assert!(
        state.closed.contains(&base),
        "closed by inclusion in a CLOSED cone"
    );
}

/// Ruling 2026-10-25 (d): the S config digest is the WHOLE effective
/// configuration — remote/branch records change ref resolution, so a change
/// to them invalidates a proof like any other config change.
#[test]
fn branch_and_remote_config_change_invalidates_proof() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: work");
    for (key, value) in [
        ("branch.main.merge", "refs/heads/other"),
        ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
    ] {
        assert_eq!(
            probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
            NegativeProbe::ProvedEmpty
        );
        git(&repo, &["config", key, value]);
        assert_ne!(
            probe(&repo, &["TASK-7"], &tuning(1024, 0)),
            NegativeProbe::ProvedEmpty,
            "{key} must be part of the snapshot identity"
        );
    }
}

fn rev_list_all(repo: &Path) -> std::collections::BTreeSet<String> {
    git(repo, &["rev-list", "--all"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn merge(repo: &Path, branch: &str, msg: &str) {
    git(
        repo,
        &[
            "merge",
            "-q",
            "--no-ff",
            "--allow-unrelated-histories",
            "-m",
            msg,
            branch,
        ],
    );
}

/// Ruling 2026-10-25 (b): the `^fence` discovery walk plus closure by
/// inclusion is EQUIVALENT to the full walk. On a merge-heavy, multi-root
/// DAG grown in stages (side branches, criss-cross merges, an unrelated
/// history merged in, an unmerged orphan root), a proof extended
/// incrementally at several batch sizes closes EXACTLY the commits `git
/// rev-list --all` reaches — none missing, none extra — the same set a
/// from-scratch proof closes; and a referencing commit introduced behind
/// the fence (deep in a merged side branch, at an unrelated root, or on a
/// merge commit itself) is always found, matching the original oracle.
#[test]
fn fenced_walk_and_inclusion_match_full_walk_on_merge_heavy_multi_root_dag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    let batch_keys = [
        (1, "TASK-71"),
        (2, "TASK-72"),
        (3, "TASK-73"),
        (7, "TASK-77"),
    ];
    let positive_keys = ["TASK-101", "TASK-102", "TASK-103"];
    let assert_equivalent = |stage: &str| {
        let all = rev_list_all(&repo);
        for (batch, key) in batch_keys {
            assert_eq!(
                probe(&repo, &[key], &tuning(batch, usize::MAX)),
                NegativeProbe::ProvedEmpty,
                "{stage}: batch {batch}"
            );
            let state = state_on_disk(&repo, &[key]).unwrap();
            assert_eq!(
                state.closed, all,
                "{stage}: batch {batch} closes exactly rev-list --all"
            );
            assert!(state.open.is_empty(), "{stage}: batch {batch}");
        }
    };

    // Stage 0: a linear base, every key proved negative.
    for i in 0..3 {
        commit(&repo, &format!("feat: base {i}"));
    }
    assert_equivalent("stage 0");
    for key in positive_keys {
        assert_eq!(
            probe(&repo, &[key], &tuning(2, usize::MAX)),
            NegativeProbe::ProvedEmpty
        );
    }

    // Stage 1: side branches forked from proved (CLOSED) history, a hit deep
    // in one of them, and criss-cross merges — one carrying a hit itself.
    git(&repo, &["checkout", "-q", "-b", "s1", "main~2"]);
    commit(&repo, "feat: s1 a");
    commit(&repo, "fix: s1 deep hit (TASK-101)");
    commit(&repo, "feat: s1 c");
    git(&repo, &["checkout", "-q", "-b", "s2", "main~1"]);
    commit(&repo, "feat: s2 a");
    git(&repo, &["checkout", "-q", "main"]);
    merge(&repo, "s1", "merge s1");
    merge(&repo, "s2", "merge s2");
    git(&repo, &["checkout", "-q", "-b", "x"]);
    commit(&repo, "feat: x a");
    git(&repo, &["checkout", "-q", "main"]);
    commit(&repo, "feat: main after x fork");
    git(&repo, &["checkout", "-q", "x"]);
    merge(&repo, "main", "criss-cross main into x");
    git(&repo, &["checkout", "-q", "main"]);
    merge(&repo, "x", "criss-cross x into main (TASK-103)");
    assert_equivalent("stage 1");

    // Stage 2: an unrelated history merged in (a second parentless root) and
    // an unmerged orphan branch (a third root), one root carrying a hit.
    git(&repo, &["checkout", "-q", "--orphan", "other"]);
    git(&repo, &["rm", "-rfq", "--ignore-unmatch", "."]);
    commit(&repo, "feat: other root (TASK-102)");
    commit(&repo, "feat: other b");
    git(&repo, &["checkout", "-q", "main"]);
    merge(&repo, "other", "merge unrelated other");
    git(&repo, &["checkout", "-q", "--orphan", "lone"]);
    commit(&repo, "feat: lone root");
    commit(&repo, "feat: lone b");
    git(&repo, &["checkout", "-q", "main"]);
    let parentless = git(&repo, &["rev-list", "--all", "--max-parents=0"]);
    assert_eq!(parentless.lines().count(), 3, "three roots");
    assert_equivalent("stage 2");

    // From-scratch full proof: the same closed set.
    assert_eq!(
        probe(&repo, &["TASK-88"], &tuning(4096, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    assert_eq!(
        state_on_disk(&repo, &["TASK-88"]).unwrap().closed,
        state_on_disk(&repo, &["TASK-71"]).unwrap().closed
    );

    // Every hit behind the fence is found, as the original oracle finds it.
    for key in positive_keys {
        assert_eq!(
            probe(&repo, &[key], &tuning(2, usize::MAX)),
            NegativeProbe::PositiveHit,
            "{key} must never close by inclusion"
        );
        assert!(!collect_git_linkage_opts(&repo, &ids(&[key]), false)
            .commits
            .is_empty());
    }
}

/// Amendment 1 / ruling 2026-10-25: a PURE warm read (no batch this
/// invocation) still re-captures S and R after the walk; roots moving in
/// between are never answered as a complete negative.
#[cfg(unix)]
#[test]
fn pure_warm_read_rechecks_snapshot_after_walk() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: base");
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    // A shim that moves the fleet just before the SECOND root listing (the
    // after-capture).
    let real = real_git();
    let shim_dir = tmp.path().join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let count_path = tmp.path().join("roots-calls");
    std::fs::write(&count_path, "").unwrap();
    crate::test_exec::write_executable(
        &shim_dir.join("git"),
        format!(
            "#!/bin/sh\nall=0; nw=0\nfor a in \"$@\"; do\n  [ \"$a\" = \"--all\" ] && all=1\n  [ \"$a\" = \"--no-walk=unsorted\" ] && nw=1\ndone\nif [ \"$all\" = 1 ] && [ \"$nw\" = 1 ]; then\n  echo x >> '{count}'\n  if [ \"$(wc -l < '{count}')\" -eq 2 ]; then\n    '{real}' -c maintenance.auto=false -c gc.auto=0 -C '{repo}' commit -q --allow-empty -m 'fleet moved' >/dev/null 2>&1\n  fi\nfi\nexec '{real}' \"$@\"\n",
            count = count_path.display(),
            real = real.display(),
            repo = repo.display(),
        ),
    );
    let mut paths = vec![shim_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let _path = crate::test_env::EnvVarGuard::set("PATH", std::env::join_paths(paths).unwrap());

    assert_ne!(
        probe(&repo, &["TASK-7"], &tuning(1024, 0)),
        NegativeProbe::ProvedEmpty,
        "a warm read whose roots moved under it must not answer complete"
    );
    let listings = std::fs::read_to_string(&count_path)
        .unwrap()
        .lines()
        .count();
    assert!(listings >= 2, "the after-capture ran: {listings}");
}

/// Ruling 2026-10-25: positive and cold shows pay NO serial snapshot in
/// front of the original query — structurally, no snapshot-only git read
/// (`config --list`, the `--no-walk=unsorted` root listing) runs on a cold
/// positive show. That holds for a never-shown key and for a key that turned
/// positive after holding a negative proof (the hit drops its state). (P1:
/// the SECOND show of a positive key seeds its cone with a snapshot that
/// overlaps the original — `positive_seeding_snapshot_overlaps_original`.)
#[cfg(unix)]
#[test]
fn positive_show_takes_no_snapshot() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "fix: real work (TASK-7)");
    commit(&repo, "feat: unrelated");
    seed_key(&repo, &["TASK-8"]);
    commit(&repo, "fix: TASK-8 turns positive (TASK-8)");
    let (log_path, _path) = install_logging_shim(tmp.path());
    let show = |key: &str| {
        collect_git_linkage_until(
            &repo,
            &ids(&[key]),
            Instant::now() + Duration::from_secs(30),
        )
    };
    let snapshot_reads = || -> Vec<String> {
        std::fs::read_to_string(&log_path)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("--no-walk=unsorted") || l.contains("config --list"))
            .map(str::to_string)
            .collect()
    };

    assert_eq!(show("TASK-7").commits.len(), 1);
    assert_eq!(snapshot_reads(), Vec::<String>::new(), "positive key");
    assert!(
        state_on_disk(&repo, &["TASK-7"]).is_some_and(|st| st.positive && st.cone.is_none()),
        "a complete positive answer leaves a positive marker only"
    );

    // The first show after TASK-8 turned positive pays one warm snapshot and
    // finds the hit; the state is dropped, so the next show pays nothing.
    assert_eq!(show("TASK-8").commits.len(), 1);
    assert!(state_on_disk(&repo, &["TASK-8"]).is_none());
    std::fs::write(&log_path, "").unwrap();
    assert_eq!(show("TASK-8").commits.len(), 1);
    assert_eq!(
        snapshot_reads(),
        Vec::<String>::new(),
        "turned-positive key"
    );
}

/// Amendment 2: a symlinked namespace directory (`aida-cache` or
/// `show-negative-v1`) is never written through — publication is refused
/// (the proof answer itself stands) and nothing lands outside the git dir.
#[cfg(unix)]
#[test]
fn symlinked_namespace_dir_is_never_written_through() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: work");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let common = repo.join(".git");

    std::os::unix::fs::symlink(&outside, common.join("aida-cache")).unwrap();
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    std::fs::remove_file(common.join("aida-cache")).unwrap();

    std::fs::create_dir_all(common.join("aida-cache")).unwrap();
    std::os::unix::fs::symlink(&outside, common.join("aida-cache/show-negative-v1")).unwrap();
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

/// Eviction is least-recently-USED: a warm read touches its state file.
#[test]
fn warm_read_touches_state_for_lru_eviction() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "feat: work");
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
        NegativeProbe::ProvedEmpty
    );
    let loc = locate(&repo, far_deadline()).unwrap();
    let path = state_path(&loc, &query_key(&ids(&["TASK-7"])));
    let old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert_eq!(
        probe(&repo, &["TASK-7"], &tuning(1024, 0)),
        NegativeProbe::ProvedEmpty
    );
    let touched = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert!(touched > old, "a warm hit must refresh its eviction age");
}

// ---------------------------------------------------------------------------
// P1 (SKETCH v2): the positive cone.
// ---------------------------------------------------------------------------

/// A commit with pinned author/committer timestamps (`ct`, epoch seconds), so
/// fixtures control `--date-order` exactly, clock skew included.
fn commit_at(repo: &Path, msg: &str, ct: i64) -> String {
    git_at(repo, &["commit", "-q", "--allow-empty", "-m", msg], ct);
    git(repo, &["rev-parse", "HEAD"])
}

fn git_at(repo: &Path, args: &[&str], ct: i64) {
    let date = format!("@{ct} +0000");
    let out = std::process::Command::new(real_git())
        .args(aida_core::git_ops::NO_AUTO_MAINTENANCE)
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_COMMITTER_DATE", &date)
        .env("GIT_AUTHOR_DATE", &date)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn show_now(repo: &Path, key: &str) -> GitLinkage {
    collect_git_linkage_until(repo, &ids(&[key]), Instant::now() + Duration::from_secs(30))
}

/// The original query's records for `key`, in its order.
fn oracle_records(repo: &Path, key: &str) -> Vec<String> {
    git(
        repo,
        &[
            "log",
            "--all",
            "-F",
            "--date-order",
            "--grep",
            key,
            "--pretty=format:%H",
        ],
    )
    .lines()
    .map(str::to_string)
    .collect()
}

/// P1 (O1 under the P1' guards): across a DAG grown in stages (new hits
/// above the cone, a merge that is itself a hit, a rebased chain sharing one
/// committer second, clock skew that reorders unrelated hits (the Gap-1
/// counterexample), deleted refs holding hits (Gap 2)), every show answers
/// EXACTLY what the original `--all --date-order` collector answers, in its
/// order, and complete. A guard that cannot prove the order falls back to
/// the original; it never guesses.
#[test]
fn positive_order_matches_date_order_oracle() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    let keys = ["TASK-1", "TASK-2"];
    let check = |stage: &str| {
        for key in keys {
            let oracle = collect_git_linkage_opts(&repo, &ids(&[key]), false);
            for round in 0..3 {
                let shown = show_now(&repo, key);
                assert_eq!(
                    shown.commits, oracle.commits,
                    "{stage}: {key} round {round}"
                );
                assert!(
                    !shown.commits_scan_incomplete,
                    "{stage}: {key} round {round}"
                );
            }
        }
    };
    let cone_hits = |key: &str| {
        state_on_disk(&repo, &[key])
            .and_then(|st| st.cone)
            .map(|c| c.hits)
    };

    commit_at(&repo, "fix: one (TASK-1)", 1_000);
    commit_at(&repo, "fix: two (TASK-2)", 1_100);
    check("stage 0");
    assert_eq!(cone_hits("TASK-1"), Some(oracle_records(&repo, "TASK-1")));

    // Stage 1: new commits above the cone, a hit on a side branch, and a
    // merge commit that is itself a hit.
    commit_at(&repo, "feat: above", 2_000);
    git(&repo, &["checkout", "-q", "-b", "side"]);
    commit_at(&repo, "fix: side (TASK-1)", 2_100);
    git(&repo, &["checkout", "-q", "main"]);
    commit_at(&repo, "fix: main (TASK-2)", 2_200);
    git_at(
        &repo,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "merge side (TASK-1) (TASK-2)",
            "side",
        ],
        2_300,
    );
    check("stage 1");
    assert_eq!(
        cone_hits("TASK-1"),
        Some(oracle_records(&repo, "TASK-1")),
        "the warm read extended the cone in Git's order"
    );
    let warm = |key: &str| probe_inner(&repo, &ids(&[key]), &oracle_tuning()).0;
    commit_at(&repo, "fix: probe (TASK-1)", 2_400);
    assert!(
        matches!(warm("TASK-1"), NegativeProbe::ProvedPositive(_)),
        "a warm positive key answers from its cone, not the original"
    );

    // Stage 2: a rebased chain sharing one committer second (never two of
    // them ready at once, so the order is still unique).
    for i in 0..3 {
        commit_at(&repo, &format!("fix: chain {i} (TASK-1)"), 3_000);
    }
    check("stage 2");

    // Stage 3: Gap 1. base - A(hit) - C(skewed OLD date, tip of x) and
    // base - B(hit, tip of y): Git prints B before A.
    let base = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "-b", "x", &base]);
    commit_at(&repo, "fix: A (TASK-2)", 9_000);
    commit_at(&repo, "feat: C skewed", 10);
    git(&repo, &["checkout", "-q", "-b", "y", &base]);
    commit_at(&repo, "fix: B (TASK-2)", 5_000);
    git(&repo, &["checkout", "-q", "main"]);
    assert_eq!(
        warm("TASK-2"),
        NegativeProbe::RunOriginal,
        "a skewed NEW commit fails the order guard"
    );
    check("stage 3");

    // Stage 4: Gap 2. Refs holding hits are deleted; their commits leave
    // `--all`.
    git(&repo, &["branch", "-D", "x"]);
    assert_eq!(
        warm("TASK-2"),
        NegativeProbe::RunOriginal,
        "a lost cone root fails the reachability guard"
    );
    check("stage 4");
    git(&repo, &["branch", "-D", "y"]);
    check("stage 4b");

    // Stage 5: growth on top again, after the reseeds.
    commit_at(&repo, "fix: later (TASK-1) (TASK-2)", 20_000);
    check("stage 5");
    assert_eq!(cone_hits("TASK-2"), Some(oracle_records(&repo, "TASK-2")));
}

/// P1: a warm positive show walks only the commits added since the cone.
/// Structurally, no `--date-order` (the original full-history query) runs,
/// and the answer still equals the original's.
#[cfg(unix)]
#[test]
fn positive_warm_walks_only_new_commits() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    for i in 0..5 {
        commit_at(&repo, &format!("feat: history {i}"), 1_000 + i);
    }
    commit_at(&repo, "fix: work (TASK-7)", 2_000);
    show_now(&repo, "TASK-7"); // marks the key positive
    show_now(&repo, "TASK-7"); // seeds the cone
    assert!(state_on_disk(&repo, &["TASK-7"]).is_some_and(|st| st.cone.is_some()));
    commit_at(&repo, "feat: unrelated", 3_000);
    commit_at(&repo, "fix: more work (TASK-7)", 3_100);
    let oracle = collect_git_linkage_opts(&repo, &ids(&["TASK-7"]), false);
    assert_eq!(oracle.commits.len(), 2);

    let (log_path, _path) = install_logging_shim(tmp.path());
    for round in 0..2 {
        let shown = show_now(&repo, "TASK-7");
        assert_eq!(shown.commits, oracle.commits, "round {round}");
        assert!(!shown.commits_scan_incomplete);
    }
    let calls = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        !calls.lines().any(|l| l.contains("--date-order")),
        "a warm positive show never runs the full-history query:\n{calls}"
    );
}

/// P1 + ruling 2026-10-25: the seeding show's BEFORE snapshot overlaps the
/// original query instead of running in front of it. The shim holds every
/// `config --list` until the original (`--date-order`) query has started. A
/// serial snapshot would wait out the shim's generous liveness cap, which
/// the shim records.
#[cfg(unix)]
#[test]
fn positive_seeding_snapshot_overlaps_original() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, "fix: work (TASK-7)", 2_000);
    show_now(&repo, "TASK-7"); // cold: marks the key positive

    let real = real_git();
    let shim_dir = tmp.path().join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let gate = tmp.path().join("original-started");
    let serial = tmp.path().join("snapshot-waited-out");
    crate::test_exec::write_executable(
        &shim_dir.join("git"),
        format!(
            "#!/bin/sh\ncase \" $* \" in\n  *' --date-order '*) : > '{gate}' ;;\n  *' config --list '*)\n    i=0\n    while [ ! -e '{gate}' ]; do\n      i=$((i+1)); if [ \"$i\" -gt 200 ]; then : > '{serial}'; break; fi\n      sleep 0.1\n    done ;;\nesac\nexec '{real}' \"$@\"\n",
            gate = gate.display(),
            serial = serial.display(),
            real = real.display(),
        ),
    );
    let mut paths = vec![shim_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let _path = crate::test_env::EnvVarGuard::set("PATH", std::env::join_paths(paths).unwrap());

    let shown = show_now(&repo, "TASK-7");
    assert_eq!(shown.commits.len(), 1);
    assert!(
        !serial.exists(),
        "the seeding snapshot ran in front of the original"
    );
    assert!(
        state_on_disk(&repo, &["TASK-7"]).is_some_and(|st| st.cone.is_some()),
        "the overlapped snapshot still seeds the cone"
    );
}

/// P1 seeding follows ruling 2026-10-25 (a) verbatim: only a COMPLETE
/// original answer, pinned to a snapshot that is unchanged afterwards, seeds
/// a cone. An incomplete answer or a moved snapshot publishes nothing.
#[test]
fn positive_incomplete_oracle_seeds_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, "fix: work (TASK-7)", 2_000);
    let answer = |repo: &Path, complete: bool| OracleAnswer {
        complete,
        references: 1,
        records: oracle_records(repo, "TASK-7"),
    };
    let positive_marker = |repo: &Path| {
        state_on_disk(repo, &["TASK-7"]).is_some_and(|st| st.positive && st.cone.is_none())
    };
    let (_, mark) = probe_inner(&repo, &ids(&["TASK-7"]), &oracle_tuning());
    assert!(matches!(mark, Some(OracleSeed::Mark { .. })));
    assert!(settle_oracle(
        &repo,
        mark.unwrap(),
        answer(&repo, true),
        far_deadline()
    ));
    assert!(positive_marker(&repo));

    let (_, seed) = probe_inner(&repo, &ids(&["TASK-7"]), &oracle_tuning());
    assert!(matches!(seed, Some(OracleSeed::Concurrent { .. })));
    assert!(!settle_oracle(
        &repo,
        seed.unwrap(),
        answer(&repo, false),
        far_deadline()
    ));
    assert!(positive_marker(&repo), "an incomplete oracle seeds nothing");

    let (_, mut seed) = probe_inner(&repo, &ids(&["TASK-7"]), &oracle_tuning());
    if let Some(OracleSeed::Concurrent { pending, .. }) = seed.as_mut() {
        pending.finish(); // the BEFORE capture is done; then the fleet moves
    }
    commit_at(&repo, "feat: fleet moved meanwhile", 3_000);
    assert!(!settle_oracle(
        &repo,
        seed.unwrap(),
        answer(&repo, true),
        far_deadline()
    ));
    assert!(positive_marker(&repo), "a moved snapshot seeds nothing");

    let (_, seed) = probe_inner(&repo, &ids(&["TASK-7"]), &oracle_tuning());
    assert!(settle_oracle(
        &repo,
        seed.unwrap(),
        answer(&repo, true),
        far_deadline()
    ));
    assert_eq!(
        state_on_disk(&repo, &["TASK-7"])
            .and_then(|st| st.cone)
            .map(|c| c.hits),
        Some(oracle_records(&repo, "TASK-7"))
    );
}

/// Advisor 2026-10-26 02:50 (seeding race): the BEFORE roots are pinned
/// before the oracle reads any ref. Once the probe hands back a seeding
/// deferral its R is final, so a ref move that lands before the oracle's
/// ref read — a ship merge adding a referencing commit, or a deleted branch
/// taking a hit with it — makes the oracle's roots differ from BEFORE's, and
/// AFTER catches it: nothing seeds.
#[test]
fn positive_seed_ignores_ref_move_before_oracle_ref_read() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, "fix: work (TASK-7)", 2_000);
    git(&repo, &["checkout", "-q", "-b", "gone"]);
    commit_at(&repo, "fix: doomed (TASK-7)", 2_100);
    git(&repo, &["checkout", "-q", "main"]);
    show_now(&repo, "TASK-7"); // cold: marks the key positive
    let positive_marker = |repo: &Path| {
        state_on_disk(repo, &["TASK-7"]).is_some_and(|st| st.positive && st.cone.is_none())
    };
    assert!(positive_marker(&repo));
    let settle_after = |mv: &dyn Fn()| {
        let (_, seed) = probe_inner(&repo, &ids(&["TASK-7"]), &oracle_tuning());
        assert!(matches!(seed, Some(OracleSeed::Concurrent { .. })));
        mv(); // the fleet moves; only then does the oracle read refs
        let records = oracle_records(&repo, "TASK-7");
        settle_oracle(
            &repo,
            seed.unwrap(),
            OracleAnswer {
                complete: true,
                references: records.len(),
                records,
            },
            far_deadline(),
        )
    };

    assert!(!settle_after(&|| {
        commit_at(&repo, "fix: shipped (TASK-7)", 3_000);
    }));
    assert!(positive_marker(&repo), "a ref added after R seeds nothing");
    assert!(!settle_after(&|| {
        git(&repo, &["branch", "-D", "gone"]);
    }));
    assert!(
        positive_marker(&repo),
        "a ref deleted after R seeds nothing"
    );
}

/// The same race end to end through the collector: the shim moves a ref
/// right after the original (`--date-order`) query read refs, and holds the
/// roots read (`--no-walk=unsorted`) until that move happened (bounded). A
/// roots capture overlapping the original would read the moved refs, match
/// AFTER, and seed a cone whose roots and hits disagree. Pinned in front of
/// the original, it predates the move and nothing seeds.
#[cfg(unix)]
#[test]
fn positive_seed_roots_pinned_before_original_ref_read() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, "fix: work (TASK-7)", 2_000);
    show_now(&repo, "TASK-7"); // cold: marks the key positive

    let real = real_git();
    let shim_dir = tmp.path().join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let moved = tmp.path().join("moved");
    let log_path = tmp.path().join("git-calls.log");
    crate::test_exec::write_executable(
        &shim_dir.join("git"),
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\ncase \" $* \" in\n  *' --date-order '*)\n    '{real}' \"$@\"; rc=$?\n    if [ ! -e '{moved}' ]; then\n      c=$('{real}' -C '{repo}' commit-tree -m 'fix: shipped (TASK-7)' -p HEAD 'HEAD^{{tree}}')\n      '{real}' -C '{repo}' update-ref refs/heads/shipped \"$c\"\n      : > '{moved}'\n    fi\n    exit $rc ;;\n  *' --no-walk=unsorted '*)\n    i=0\n    while [ ! -e '{moved}' ] && [ \"$i\" -lt 5 ]; do i=$((i+1)); sleep 0.1; done ;;\nesac\nexec '{real}' \"$@\"\n",
            log = log_path.display(),
            moved = moved.display(),
            repo = repo.display(),
            real = real.display(),
        ),
    );
    let mut paths = vec![shim_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let _path = crate::test_env::EnvVarGuard::set("PATH", std::env::join_paths(paths).unwrap());

    let shown = show_now(&repo, "TASK-7");
    assert_eq!(shown.commits.len(), 1, "the original answered at its read");
    assert!(moved.exists(), "the shim moved a ref after the original");
    let calls = std::fs::read_to_string(&log_path).unwrap_or_default();
    let first = |needle: &str| {
        calls
            .lines()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no `{needle}` call:\n{calls}"))
    };
    assert!(
        first("--no-walk=unsorted") < first("--date-order"),
        "BEFORE's roots read precedes the original:\n{calls}"
    );
    assert!(
        state_on_disk(&repo, &["TASK-7"]).is_some_and(|st| st.positive && st.cone.is_none()),
        "a ref move between BEFORE and AFTER seeds nothing"
    );
}
