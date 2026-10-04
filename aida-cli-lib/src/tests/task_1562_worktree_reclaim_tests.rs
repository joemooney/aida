//! TASK-1562 — `aida worktree reclaim` rails.
//!
//! The fixture AC asks for "a main checkout, a live-lease worktree, an open-PR
//! worktree and a stale worktree: exactly the stale one is deleted", so these
//! build real directories in a tempdir rather than mocking a filesystem. The
//! only injected inputs are the ones a test cannot otherwise control: the
//! clock (rail 6) and process liveness (rail 4, pre-resolved into `LeaseHold`).
// trace:TASK-1562 | ai:claude

use crate::worktree_reclaim::{
    apply, human_bytes, parse_worktree_entries, plan, protected_target, LeaseHold, Plan,
    Reclaimable, Skip, Survey, WorktreeEntry,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How far ahead of the fixture's creation time the planner's clock is set, so
/// a just-written `target/` reads as cold to rail 6. Ageing the survey's
/// injected `now` rather than rewriting directory mtimes keeps the fixture
/// deterministic and needs no filesystem-time crate.
const COLD: Duration = Duration::from_secs(6 * 3600);

/// A worktree dir with a `target/` holding `fill` bytes.
fn worktree(root: &Path, name: &str, fill: usize) -> PathBuf {
    let wt = root.join(name);
    let target = wt.join("target");
    std::fs::create_dir_all(target.join("debug")).unwrap();
    std::fs::write(target.join("debug").join("blob.bin"), vec![0u8; fill]).unwrap();
    wt
}

/// A survey whose clock sits `COLD` past the fixture, i.e. every `target/`
/// reads as 6h stale unless a test overrides `now`.
fn survey(root: &Path, worktrees: Vec<WorktreeEntry>) -> Survey {
    Survey {
        main_root: root.join("main"),
        worktrees,
        open_pr_branches: Some(Vec::new()),
        leases: Vec::new(),
        min_age_mins: 60,
        include_live: false,
        include_open_prs: false,
        now: SystemTime::now() + COLD,
    }
}

fn entry(path: &Path, branch: &str) -> WorktreeEntry {
    WorktreeEntry {
        path: path.to_path_buf(),
        branch: Some(branch.to_string()),
    }
}

fn targets(p: &Plan) -> Vec<PathBuf> {
    p.reclaim.iter().map(|r| r.target.clone()).collect()
}

fn skip_for(p: &Plan, path: &Path) -> Option<Skip> {
    p.skipped
        .iter()
        .find(|(sp, _)| sp == path)
        .map(|(_, s)| s.clone())
}

// ---------------------------------------------------------------------------
// AC5 — the four-worktree fixture: exactly the stale one is reclaimed.
// ---------------------------------------------------------------------------

// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_fixture_reclaims_exactly_the_stale_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // The main checkout's target/ is deliberately the LARGEST, so passing this
    // test cannot be an artifact of size ordering.
    let main = worktree(root, "main", 40_000);
    let live = worktree(root, "aida-live", 8_000);
    let open_pr = worktree(root, "aida-open-pr", 8_000);
    let stale = worktree(root, "aida-stale", 8_000);

    let mut s = survey(
        root,
        vec![
            entry(&main, "main"),
            entry(&live, "task-live"),
            entry(&open_pr, "task-open-pr"),
            entry(&stale, "task-stale"),
        ],
    );
    s.open_pr_branches = Some(vec!["task-open-pr".to_string()]);
    s.leases = vec![LeaseHold {
        worktree_path: live.clone(),
        alive: true,
        pid: Some(4242),
    }];

    let p = plan(&s);

    assert_eq!(
        targets(&p),
        vec![stale.join("target")],
        "exactly the stale worktree's target/ is a candidate"
    );
    assert_eq!(
        skip_for(&p, &main),
        Some(Skip::MainCheckout),
        "the main checkout must be skipped as the main checkout, not for some other reason"
    );
    assert!(matches!(
        skip_for(&p, &live),
        Some(Skip::LiveSession { pid: Some(4242) })
    ));
    assert!(matches!(skip_for(&p, &open_pr), Some(Skip::OpenPr { .. })));

    // And the delete pass touches only that one.
    let done = apply(&p, None, |_| None, |path| std::fs::remove_dir_all(path)).unwrap();
    assert_eq!(done.deleted.len(), 1);
    assert!(done.failed.is_empty());
    assert!(!stale.join("target").exists(), "the stale cache is gone");
    assert!(main.join("target").is_dir(), "the main cache survives");
    assert!(live.join("target").is_dir(), "the live cache survives");
    assert!(
        open_pr.join("target").is_dir(),
        "the open-PR cache survives"
    );
}

// ---------------------------------------------------------------------------
// AC3 — rail 2 holds by assertion at the DELETE SITE, not by ordering.
// ---------------------------------------------------------------------------

/// The AC names the property precisely: the main checkout's `target/` is
/// refused "by an assertion, not by ordering — a test proves the command
/// leaves it in place even when it is the largest candidate". So this test
/// bypasses `plan` entirely and hands `apply` a hand-built plan whose only
/// entry IS the protected path — i.e. it simulates the upstream filter bug that
/// rail 2's first layer is supposed to prevent. Going through `plan` would only
/// prove the filter works, which is a different claim.
// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_apply_refuses_the_main_checkout_target_even_as_the_only_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let main = worktree(root, "main", 40_000);
    let protected = protected_target(&main);

    let forged = Plan {
        main_root: main.clone(),
        // The largest candidate, and the first one `apply` will reach.
        reclaim: vec![Reclaimable {
            target: protected.clone(),
            branch: Some("main".to_string()),
            bytes: u64::MAX,
        }],
        skipped: Vec::new(),
    };

    let err = apply(
        &forged,
        None,
        |_| None,
        |path| std::fs::remove_dir_all(path),
    )
    .expect_err("apply must refuse the main checkout's target/");

    assert!(
        err.to_string().contains("refusing to delete"),
        "the refusal must say so: {err}"
    );
    assert!(
        protected.is_dir(),
        "the main checkout's target/ must still be on disk after the refusal"
    );
    assert!(
        protected.join("debug").join("blob.bin").is_file(),
        "and its contents must be untouched, not merely the directory"
    );
}

/// Rail 2 again, from the other side: `plan` refuses it too, so the protection
/// is not load-bearing on `apply` alone.
// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_plan_never_lists_the_main_checkout_target() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let main = worktree(root, "main", 90_000);

    let s = survey(root, vec![entry(&main, "main")]);
    let p = plan(&s);

    assert!(p.reclaim.is_empty());
    assert_eq!(p.reclaimable_bytes(), 0);
    assert!(matches!(skip_for(&p, &main), Some(Skip::MainCheckout)));
}

// ---------------------------------------------------------------------------
// AC4 — an open PR is skipped by default, with an opt-in override.
// ---------------------------------------------------------------------------

// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_open_pr_is_skipped_by_default_and_reclaimed_with_the_override() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("main")).unwrap();
    let wt = worktree(root, "aida-bug-1693", 8_000);

    let mut s = survey(root, vec![entry(&wt, "bug-1693")]);
    s.open_pr_branches = Some(vec!["bug-1693".to_string()]);

    let default = plan(&s);
    assert!(default.reclaim.is_empty());
    assert_eq!(
        skip_for(&default, &wt),
        Some(Skip::OpenPr {
            branch: "bug-1693".to_string()
        })
    );

    s.include_open_prs = true;
    assert_eq!(targets(&plan(&s)), vec![wt.join("target")]);
}

/// The fail-safe arm: when open-PR state cannot be read (no `gh`), every
/// branch is held back rather than every branch being reclaimed. The reference
/// shell script had this inverted, which is the one rail regression worth
/// fixing while porting it.
// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_unknown_pr_state_holds_everything_back_unless_waived() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("main")).unwrap();
    let wt = worktree(root, "aida-stale", 8_000);

    let mut s = survey(root, vec![entry(&wt, "task-stale")]);
    s.open_pr_branches = None; // gh unreachable

    let held = plan(&s);
    assert!(
        held.reclaim.is_empty(),
        "unknown PR state must not reclaim anything"
    );
    assert_eq!(skip_for(&held, &wt), Some(Skip::PrStateUnknown));

    s.include_open_prs = true;
    assert_eq!(
        targets(&plan(&s)),
        vec![wt.join("target")],
        "--include-open-prs waives the fail-safe hold too"
    );
}

// ---------------------------------------------------------------------------
// Rails 3, 4 and 6.
// ---------------------------------------------------------------------------

// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_store_worktree_is_never_a_candidate() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("main")).unwrap();
    let store = worktree(root, ".aida-store", 50_000);

    let s = survey(root, vec![entry(&store, "aida-store")]);
    let p = plan(&s);
    assert!(p.reclaim.is_empty());
    assert_eq!(skip_for(&p, &store), Some(Skip::Store));
}

/// A lease whose process is gone is not a hold: that is the whole point of
/// resolving liveness through the shared staleness predicate rather than
/// treating the presence of a lease file as ownership.
// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_a_dead_lease_does_not_hold_a_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("main")).unwrap();
    let wt = worktree(root, "aida-dead", 8_000);

    let mut s = survey(root, vec![entry(&wt, "task-dead")]);
    s.leases = vec![LeaseHold {
        worktree_path: wt.clone(),
        alive: false,
        pid: Some(999_999),
    }];

    assert_eq!(targets(&plan(&s)), vec![wt.join("target")]);
}

// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_live_lease_is_reclaimed_only_with_include_live() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("main")).unwrap();
    let wt = worktree(root, "aida-live", 8_000);

    let mut s = survey(root, vec![entry(&wt, "task-live")]);
    s.leases = vec![LeaseHold {
        worktree_path: wt.clone(),
        alive: true,
        pid: Some(1234),
    }];
    assert!(plan(&s).reclaim.is_empty());

    s.include_live = true;
    assert_eq!(targets(&plan(&s)), vec![wt.join("target")]);
}

// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_recently_touched_target_is_skipped_as_a_possible_build() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("main")).unwrap();
    let wt = worktree(root, "aida-warm", 8_000);

    let mut s = survey(root, vec![entry(&wt, "task-warm")]);
    // 10 minutes old, against the 60-minute default floor.
    s.now = SystemTime::now() + Duration::from_secs(600);
    let p = plan(&s);
    assert!(p.reclaim.is_empty());
    assert_eq!(skip_for(&p, &wt), Some(Skip::RecentlyTouched { mins: 60 }));

    // Lower the floor below the cache's age and it becomes a candidate.
    s.min_age_mins = 5;
    assert_eq!(targets(&plan(&s)), vec![wt.join("target")]);
}

// ---------------------------------------------------------------------------
// Ordering, reporting, --target-pct, and the porcelain parser.
// ---------------------------------------------------------------------------

/// Largest-first, so a `--target-pct` run frees the most space per deletion.
// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_candidates_are_ordered_largest_first_and_bytes_are_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("main")).unwrap();
    let small = worktree(root, "aida-small", 1_000);
    let big = worktree(root, "aida-big", 40_000);
    let mid = worktree(root, "aida-mid", 9_000);

    let s = survey(
        root,
        vec![entry(&small, "a"), entry(&big, "b"), entry(&mid, "c")],
    );
    let p = plan(&s);

    assert_eq!(
        targets(&p),
        vec![big.join("target"), mid.join("target"), small.join("target")]
    );
    assert!(
        p.reclaimable_bytes() >= 50_000,
        "reclaimable bytes are reported before anything is deleted: {}",
        p.reclaimable_bytes()
    );
}

/// `--target-pct` stops once the floor is met, leaving the rest warm — and it
/// is consulted per deletion, not once up front.
// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_target_pct_stops_early_and_leaves_the_rest_warm() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("main")).unwrap();
    let big = worktree(root, "aida-big", 40_000);
    let small = worktree(root, "aida-small", 1_000);

    let s = survey(root, vec![entry(&big, "b"), entry(&small, "s")]);
    let p = plan(&s);
    assert_eq!(p.reclaim.len(), 2);

    // 95% before the first delete, 80% after it: the second entry must be left
    // alone once the 85% floor is satisfied.
    let calls = std::cell::Cell::new(0u32);
    let done = apply(
        &p,
        Some(85),
        |_| {
            let n = calls.get();
            calls.set(n + 1);
            Some(if n == 0 { 95 } else { 80 })
        },
        |path| std::fs::remove_dir_all(path),
    )
    .unwrap();

    assert_eq!(done.deleted.len(), 1, "only the largest was deleted");
    assert_eq!(done.deleted[0].0, big.join("target"));
    assert_eq!(done.stopped_early, vec![small.join("target")]);
    assert!(
        small.join("target").is_dir(),
        "the cache left warm is still on disk"
    );
}

// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_porcelain_parser_reads_paths_and_shortens_branches() {
    let parsed = parse_worktree_entries(concat!(
        "worktree /home/joe/ai/aida\n",
        "HEAD abc123\n",
        "branch refs/heads/main\n",
        "\n",
        "worktree /home/joe/ai/aida-worktrees/aida-task-1562\n",
        "HEAD def456\n",
        "branch refs/heads/task-1562\n",
        "\n",
        "worktree /home/joe/ai/aida-worktrees/detached\n",
        "HEAD 0f0f0f\n",
        "detached\n",
    ));

    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0].path, PathBuf::from("/home/joe/ai/aida"));
    assert_eq!(parsed[0].branch.as_deref(), Some("main"));
    assert_eq!(parsed[1].branch.as_deref(), Some("task-1562"));
    assert_eq!(
        parsed[2].branch, None,
        "a detached record carries no branch, and must not inherit the previous one"
    );
}

/// The skip reason is user-facing: it must name the rail, because under disk
/// pressure the operator needs to know WHICH worktree was held back and why.
// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_skip_reasons_name_their_rail() {
    assert!(Skip::MainCheckout.reason().contains("CARGO_TARGET_DIR"));
    assert_eq!(Skip::MainCheckout.code(), "main-checkout");
    assert!(
        Skip::OpenPr {
            branch: "bug-1693".into()
        }
        .reason()
        .contains("bug-1693"),
        "the open-PR reason must name the branch it held back"
    );
    assert!(
        Skip::PrStateUnknown.reason().contains("--include-open-prs"),
        "the fail-safe hold must tell the operator how to waive it"
    );
    assert!(
        Skip::LiveSession { pid: Some(77) }.reason().contains("77"),
        "a live hold must name the pid so it can be checked"
    );
}

// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_human_bytes_reads_as_sizes() {
    assert_eq!(human_bytes(512), "512 B");
    assert_eq!(human_bytes(2048), "2.0 KiB");
    assert_eq!(human_bytes(8 * 1024 * 1024 * 1024), "8.0 GiB");
}

// ---------------------------------------------------------------------------
// The CLI surface.
// ---------------------------------------------------------------------------

// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_reclaim_is_dry_run_by_default() {
    use crate::cli::{Cli, Command, WorktreeCommand};
    use clap::Parser;

    let cli = Cli::try_parse_from(["aida", "worktree", "reclaim"])
        .expect("`aida worktree reclaim` should parse");
    match cli.command {
        Command::Worktree(WorktreeCommand::Reclaim {
            apply,
            min_age_mins,
            target_pct,
            include_live,
            include_open_prs,
            json,
        }) => {
            assert!(!apply, "reclaim must be DRY RUN by default");
            assert_eq!(min_age_mins, 60);
            assert_eq!(target_pct, None);
            assert!(!include_live);
            assert!(!include_open_prs);
            assert!(!json);
        }
        other => panic!("expected Worktree::Reclaim, got {other:?}"),
    }
}

// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_reclaim_parses_every_rail_override() {
    use crate::cli::{Cli, Command, WorktreeCommand};
    use clap::Parser;

    let cli = Cli::try_parse_from([
        "aida",
        "worktree",
        "reclaim",
        "--apply",
        "--min-age-mins",
        "180",
        "--target-pct",
        "80",
        "--include-live",
        "--include-open-prs",
        "--json",
    ])
    .expect("every reclaim flag should parse");
    match cli.command {
        Command::Worktree(WorktreeCommand::Reclaim {
            apply,
            min_age_mins,
            target_pct,
            include_live,
            include_open_prs,
            json,
        }) => {
            assert!(apply);
            assert_eq!(min_age_mins, 180);
            assert_eq!(target_pct, Some(80));
            assert!(include_live);
            assert!(include_open_prs);
            assert!(json);
        }
        other => panic!("expected Worktree::Reclaim, got {other:?}"),
    }
}

/// AC1 — the disk-headroom finding must no longer recommend `cargo clean` as
/// the reclaim for this condition without saying what it actually targets, and
/// must name the command that can do the job. Read at runtime from the source
/// so a later edit to the string cannot drift past this test.
// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_disk_headroom_remediation_names_the_command_that_reclaims() {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("doctor_cmd.rs"),
    )
    .expect("doctor_cmd.rs is readable");
    let start = src
        .find("category: \"disk-headroom\"")
        .expect("the disk-headroom finding exists");
    let finding = &src[start..start + 1600.min(src.len() - start)];

    assert!(
        finding.contains("aida worktree reclaim"),
        "the disk-headroom remediation must name `aida worktree reclaim`"
    );
    assert!(
        finding.contains("CARGO_TARGET_DIR"),
        "where it still mentions `cargo clean` it must say that inside a worktree that \
         targets the SHARED CARGO_TARGET_DIR — the live cache, not the stale one"
    );

    let unqualified = finding.contains("(`cargo clean`)");
    assert!(
        !unqualified,
        "`cargo clean` must not be offered as a bare remedy for this finding"
    );
}

/// AC1 for the scaffolded guard: a project `aida init` creates must get the
/// corrected prompt, not just this repository's own config.
// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_scaffolded_disk_headroom_route_prompt_is_corrected() {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("init_cmd.rs"),
    )
    .expect("init_cmd.rs is readable");
    let start = src
        .find("disk-headroom-guard-route")
        .expect("the scaffolded route job exists");
    let block = &src[start..start + 1200.min(src.len() - start)];

    assert!(
        block.contains("aida worktree reclaim"),
        "the scaffolded route prompt must name the command that reclaims this space"
    );
    assert!(
        block.contains("CARGO_TARGET_DIR"),
        "and must explain what `cargo clean` actually targets"
    );
}

/// The renderer is selected from the resolved output mode, not from the local
/// `--json` bool alone — BUG-1749 was precisely that bug in `aida doctor`.
// trace:TASK-1562 | ai:claude
#[test]
fn task_1562_json_rendering_honours_a_format_pin_not_just_the_local_flag() {
    use crate::worktree_reclaim::renders_json;
    use crate::OutputFormat;

    assert!(renders_json(true, None), "--json alone still renders JSON");
    assert!(
        renders_json(false, Some(OutputFormat::Json)),
        "`--format json` / AIDA_OUTPUT_FORMAT=json must render JSON without --json"
    );
    assert!(
        !renders_json(false, Some(OutputFormat::Human)),
        "`--format human` must not render JSON"
    );
    assert!(!renders_json(false, None));
    assert!(
        renders_json(true, Some(OutputFormat::Human)),
        "an explicit --json wins over a human pin, matching doctor's precedence"
    );
}
