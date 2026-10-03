use super::{
    preclaim_branch_state, preclaim_collision_check, OpenPrInfo, PrLookup, PreclaimBranchState,
    PreclaimDecision,
};
use std::cell::Cell;

fn merged_pr_lookup() -> PrLookup {
    PrLookup::Found(OpenPrInfo {
        number: 2201,
        title: "shipped months ago".into(),
        url: "https://github.com/example/aida/pull/2201".into(),
        head_branch: Some("bug-9".into()),
    })
}

// AC1: a merged local branch is the normal residue of this repo's retention
// policy; re-pickup proceeds and the branch refusal is not emitted.
// trace:BUG-1763 | ai:claude
#[test]
fn merged_branch_is_not_a_collision() {
    let state = preclaim_branch_state(true, merged_pr_lookup);
    assert_eq!(state, PreclaimBranchState::Merged);
    assert_eq!(
        preclaim_collision_check("BUG-9", None, "bug-9", state, false),
        PreclaimDecision::Proceed
    );
}

// AC2: an existing UNMERGED branch is the true collision class and still
// refuses, naming the branch.
// trace:BUG-1763 | ai:claude
#[test]
fn unmerged_branch_still_refuses_naming_branch() {
    let state = preclaim_branch_state(true, || PrLookup::NoOpenPr);
    assert_eq!(state, PreclaimBranchState::Unmerged);
    let PreclaimDecision::Refuse(message) =
        preclaim_collision_check("BUG-9", None, "bug-9", state, false)
    else {
        panic!("expected refusal");
    };
    assert!(message.contains("bug-9"), "must name the branch: {message}");
    assert!(message.contains("no merged PR"), "{message}");
}

// AC3: an open PR refuses regardless of branch state — PR before branch,
// as BUG-1761 established.
// trace:BUG-1763 | ai:claude
#[test]
fn open_pr_refuses_even_when_branch_is_merged() {
    let decision = preclaim_collision_check(
        "BUG-9",
        Some((
            2319,
            "active work",
            "https://github.com/example/aida/pull/2319",
        )),
        "bug-9",
        PreclaimBranchState::Merged,
        false,
    );
    let PreclaimDecision::Refuse(message) = decision else {
        panic!("expected PR refusal");
    };
    assert!(message.contains("2319"), "{message}");
    assert!(message.contains("OPEN"), "{message}");
}

// AC4: merged-ness must come from the forge, not `git merge-base
// --is-ancestor`. This repo squash-merges: construct a real squash-merged
// branch, prove ancestry reports NO (an ancestry-based implementation would
// classify the branch Unmerged and refuse), and show the forge-based path
// allows the claim.
// trace:BUG-1763 | ai:claude
#[test]
fn squash_merged_branch_is_allowed_where_ancestry_would_refuse() {
    fn git(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("spawn git");
        out
    }
    fn git_ok(dir: &std::path::Path, args: &[&str]) {
        let out = git(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let tmp = tempfile::TempDir::new().unwrap();
    let repo = tmp.path();
    git_ok(repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "base\n").unwrap();
    git_ok(repo, &["add", "."]);
    git_ok(repo, &["commit", "-m", "base"]);
    git_ok(repo, &["checkout", "-b", "bug-9"]);
    std::fs::write(repo.join("fix.txt"), "fix\n").unwrap();
    git_ok(repo, &["add", "."]);
    git_ok(repo, &["commit", "-m", "the fix"]);
    git_ok(repo, &["checkout", "main"]);
    // Squash merge: the content lands on main as a NEW commit; the branch
    // tip is not an ancestor of main — exactly what `gh pr merge --squash`
    // leaves behind.
    git_ok(repo, &["merge", "--squash", "bug-9"]);
    git_ok(repo, &["commit", "-m", "the fix (squashed)"]);

    let ancestry = git(repo, &["merge-base", "--is-ancestor", "bug-9", "main"]);
    assert!(
        !ancestry.status.success(),
        "fixture invalid: ancestry should report NOT-an-ancestor for a squash merge"
    );

    // The forge knows the truth: `gh pr list --head bug-9 --state merged`
    // returns the merged PR. The forge-based classification allows the
    // claim; an ancestry-based one would have refused on the assertion
    // above's evidence.
    let state = preclaim_branch_state(true, merged_pr_lookup);
    assert_eq!(state, PreclaimBranchState::Merged);
    assert_eq!(
        preclaim_collision_check("BUG-9", None, "bug-9", state, false),
        PreclaimDecision::Proceed
    );
}

// AC5: a forge lookup that errors, is missing, or is unreachable ALLOWS the
// claim — "cannot tell" is threaded as MergeStateUnknown, never collapsed
// onto Unmerged.
// trace:BUG-1763 | ai:claude
#[test]
fn forge_failure_allows_the_claim() {
    for lookup in [
        PrLookup::GhMissing,
        PrLookup::GhFailed("boom".into()),
        PrLookup::GhUnreachable("api down".into()),
    ] {
        let state = preclaim_branch_state(true, move || lookup);
        assert_eq!(state, PreclaimBranchState::MergeStateUnknown);
        assert_eq!(
            preclaim_collision_check("BUG-9", None, "bug-9", state, false),
            PreclaimDecision::Proceed
        );
    }
}

// AC6: no forge probe runs when the branch does not exist — counted, not
// assumed.
// trace:BUG-1763 | ai:claude
#[test]
fn no_probe_invocation_when_branch_absent() {
    let calls = Cell::new(0u32);
    let state = preclaim_branch_state(false, || {
        calls.set(calls.get() + 1);
        PrLookup::NoOpenPr
    });
    assert_eq!(calls.get(), 0, "absent branch must pay no gh call");
    assert_eq!(state, PreclaimBranchState::Absent);
    assert_eq!(
        preclaim_collision_check("BUG-9", None, "bug-9", state, false),
        PreclaimDecision::Proceed
    );

    // And exactly one probe when the branch does exist.
    let state = preclaim_branch_state(true, || {
        calls.set(calls.get() + 1);
        PrLookup::NoOpenPr
    });
    assert_eq!(calls.get(), 1);
    assert_eq!(state, PreclaimBranchState::Unmerged);
}

// AC7: the bypass chain BUG-1761 established short-circuits ahead of the
// branch arm — an unmerged branch under bypass still proceeds. (The rework
// / review / orchestrator / --resume / --branch discriminators feed the
// same `bypass` parameter; their derivation is pinned by the BUG-1761
// tests.)
// trace:BUG-1763 | ai:claude
#[test]
fn bypass_still_bypasses_the_unmerged_branch_arm() {
    assert_eq!(
        preclaim_collision_check("BUG-9", None, "bug-9", PreclaimBranchState::Unmerged, true),
        PreclaimDecision::Proceed
    );
}

// Wiring pin: the preclaim block in handle_queue_work must resolve
// merged-ness through the forge helper and must not consult ancestry.
// Mirrors bug_1761's guard_is_before_claim_boundary source-order idiom.
// trace:BUG-1763 | ai:claude
#[test]
fn preclaim_block_probes_forge_not_ancestry() {
    let source = include_str!("../queue_cmd.rs");
    let work = source
        .split_once("pub(crate) fn handle_queue_work(")
        .unwrap()
        .1;
    let guard_start = work.find("// BUG-1761: the suffix allocator").unwrap();
    let claim = work.find("    session_start(").unwrap();
    let preclaim_block = &work[guard_start..claim];
    assert!(
        preclaim_block.contains("detect_merged_pr_for_branch_via_forge"),
        "branch arm must resolve merged-ness via the forge"
    );
    assert!(
        !preclaim_block.contains("is_ancestor") && !preclaim_block.contains("merge-base"),
        "branch arm must not use ancestry: squash merges make it lie"
    );
}
