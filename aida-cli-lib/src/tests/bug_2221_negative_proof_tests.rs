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
    let ContextSnapshot::Supported(ctx) = capture_context(repo, far_deadline())? else {
        panic!("fixture repo must be a supported context");
    };
    let qk = query_key(&ids(queries));
    let path = cache_dir(&ctx).join(state_file_name(&ctx, &qk));
    let bytes = std::fs::read(path).ok()?;
    Some(serde_json::from_slice(&bytes).expect("state file parses"))
}

fn namespace_dir(repo: &Path) -> PathBuf {
    let ContextSnapshot::Supported(ctx) =
        capture_context(repo, far_deadline()).expect("context captures")
    else {
        panic!("supported context");
    };
    cache_dir(&ctx)
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
        outcomes.push(outcome);
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

    // Replace-control override → refuse (never --no-replace-objects around it).
    {
        let _guard = crate::test_env::EnvVarGuard::set("GIT_NO_REPLACE_OBJECTS", "1");
        assert_eq!(
            probe(&repo, &["TASK-7"], &tuning(1024, usize::MAX)),
            NegativeProbe::RunOriginal
        );
    }

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
    let ContextSnapshot::Supported(ctx) = capture_context(&repo, far_deadline()).unwrap() else {
        panic!("supported");
    };
    let qk = query_key(&ids(&["TASK-7"]));
    let state_path = cache_dir(&ctx).join(state_file_name(&ctx, &qk));
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
    let lock_path = cache_dir(&ctx).join(".lock");
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

/// Production cold path: a fully cold key defers to the ORIGINAL collector
/// (no batching, no incomplete answer) and a complete zero-reference oracle
/// answer seeds every current root CLOSED — so the next show is a warm read
/// with no batch at all. A snapshot that moves between the probe and the
/// seed publishes nothing.
#[test]
fn cold_oracle_seed_warms_key_only_on_unchanged_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit(&repo, "update TASK-7"); // grep-positive, parser-negative
    commit(&repo, "feat: tip");

    let (outcome, seed) = probe_inner(&repo, &ids(&["TASK-7"]), &oracle_tuning());
    assert_eq!(
        outcome,
        NegativeProbe::RunOriginal,
        "cold defers to the original"
    );
    let seed = seed.expect("a cold supported key hands back a seed");
    assert!(
        state_on_disk(&repo, &["TASK-7"]).is_none(),
        "deferral writes nothing"
    );
    assert!(publish_oracle_negative(&repo, seed, far_deadline()));
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

    // A ref moving between the deferral and the seed: nothing is published.
    let (_, seed) = probe_inner(&repo, &ids(&["TASK-8"]), &oracle_tuning());
    let seed = seed.expect("cold TASK-8 key");
    commit(&repo, "feat: fleet moved meanwhile");
    assert!(
        !publish_oracle_negative(&repo, seed, far_deadline()),
        "a moved root snapshot must not be seeded as complete"
    );
    assert!(state_on_disk(&repo, &["TASK-8"]).is_none());

    // Integrated: the cold show path answers COMPLETE (the oracle's own
    // answer), equal to the unbounded collector, and warms the key.
    let wanted = ids(&["TASK-9"]);
    let shown = collect_git_linkage_until(&repo, &wanted, Instant::now() + Duration::from_secs(30));
    let oracle = collect_git_linkage_opts(&repo, &wanted, false);
    assert_eq!(shown.commits, oracle.commits);
    assert!(
        !shown.commits_scan_incomplete,
        "cold show keeps the complete answer"
    );
    assert!(
        state_on_disk(&repo, &["TASK-9"]).is_some(),
        "and seeds the key"
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
    let (_, seed) = probe_inner(&repo, &ids(&["TASK-7"]), &oracle_tuning());
    assert!(publish_oracle_negative(
        &repo,
        seed.unwrap(),
        far_deadline()
    ));
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

/// The config digest ignores upstream-tracking/remote records (rewritten by
/// every `worktree add -b` / `push -u` in a fleet) but keeps everything else.
#[test]
fn semantic_config_ignores_only_branch_and_remote_records() {
    let z = "core.bare\nfalse\0branch.main.remote\norigin\0Remote.origin.url\nx\0grep.patternType\nperl\0";
    assert_eq!(
        semantic_config(z),
        "core.bare\nfalse\0grep.patternType\nperl\0"
    );
}
