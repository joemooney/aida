use super::*;

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A fresh repo on `main` with one initial commit.
fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    git(&root, &["init", "--initial-branch=main", "--quiet"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "init\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "chore: init"]);
    (tmp, root)
}

fn commit(root: &std::path::Path, file: &str, body: &str, subject: &str) {
    std::fs::write(root.join(file), body).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", subject]);
}

fn req(spec_id: &str) -> aida_core::Requirement {
    let mut r = aida_core::Requirement::new(format!("test {spec_id}"), String::new());
    r.spec_id = Some(spec_id.to_string());
    r
}

/// TASK-234: a mixed-state queue — one spec merged to main, one on a
/// feature branch, one with no commit — buckets into stuck /
/// awaiting / no_commit. Pure git, no gh.
#[test]
fn classify_buckets_mixed_state_queue() {
    let (_tmp, root) = init_repo();
    // STUCK: a commit on main referencing the spec.
    commit(&root, "a.txt", "x", "feat: stuck work (TASK-801) (#7)");
    // AWAITING: a commit on a feature branch, not merged to main.
    git(&root, &["checkout", "-q", "-b", "feature/x"]);
    commit(&root, "b.txt", "y", "feat: in-flight work (TASK-802)");
    git(&root, &["checkout", "-q", "main"]);

    let specs = [req("TASK-801"), req("TASK-802"), req("TASK-803")];
    let refs: Vec<&aida_core::Requirement> = specs.iter().collect();
    let b = classify_in_flight_specs(&refs, &root);

    assert_eq!(b.stuck.len(), 1, "one stuck spec");
    assert_eq!(b.stuck[0].0, "TASK-801");
    assert_eq!(b.stuck[0].1, Some(7), "squash PR parsed for the stuck spec");

    assert_eq!(
        b.awaiting.get("feature/x").map(|v| v.as_slice()),
        Some(&["TASK-802".to_string()][..])
    );

    assert_eq!(b.no_commit, vec!["TASK-803".to_string()]);
}

/// BUG-229: a Done spec whose referencing commit lives on BOTH the
/// reviewer-worktree snapshot branch (`pr-71`, left by `git fetch
/// origin pull/71/head:pr-71`) and the real PR-source branch
/// (`task-282`). The classifier must bucket it under the real
/// branch — picking the `pr-N` snapshot is what mis-rendered the row
/// as "no PR opened yet" while the PR was under review.
// trace:BUG-229 | ai:claude
#[test]
fn classify_prefers_pr_source_branch_over_reviewer_snapshot() {
    let (_tmp, root) = init_repo();
    git(&root, &["checkout", "-q", "-b", "task-282"]);
    commit(&root, "feature.txt", "work", "feat: ship it (TASK-282)");
    // The reviewer snapshot branch points at the SAME commit.
    git(&root, &["branch", "pr-71"]);
    git(&root, &["checkout", "-q", "main"]);

    let specs = [req("TASK-282")];
    let refs: Vec<&aida_core::Requirement> = specs.iter().collect();
    let b = classify_in_flight_specs(&refs, &root);

    assert_eq!(
        b.awaiting.get("task-282").map(|v| v.as_slice()),
        Some(&["TASK-282".to_string()][..]),
        "bucketed under the real PR-source branch, not the pr-N snapshot"
    );
    assert!(
        !b.awaiting.contains_key("pr-71"),
        "the `pr-71` reviewer snapshot must not drive the bucket key"
    );
}

/// BUG-229: when the ONLY branch containing the commit is a `pr-N`
/// reviewer snapshot (the real PR-source branch was pruned), the
/// classifier falls back to it rather than losing the spec — a
/// degraded but honest key, better than `(branch unknown)`.
// trace:BUG-229 | ai:claude
#[test]
fn classify_falls_back_to_pr_n_when_it_is_the_only_branch() {
    let (_tmp, root) = init_repo();
    git(&root, &["checkout", "-q", "-b", "pr-71"]);
    commit(&root, "snap.txt", "work", "feat: snapshot (TASK-283)");
    git(&root, &["checkout", "-q", "main"]);

    let specs = [req("TASK-283")];
    let refs: Vec<&aida_core::Requirement> = specs.iter().collect();
    let b = classify_in_flight_specs(&refs, &root);

    assert_eq!(
        b.awaiting.get("pr-71").map(|v| v.as_slice()),
        Some(&["TASK-283".to_string()][..]),
        "falls back to the pr-N branch when no real branch contains the commit"
    );
}

/// TASK-234: an empty queue yields three empty buckets — the
/// no-Done-items path.
#[test]
fn classify_handles_no_done_items() {
    let (_tmp, root) = init_repo();
    let b = classify_in_flight_specs(&[], &root);
    assert!(b.stuck.is_empty() && b.awaiting.is_empty() && b.no_commit.is_empty());
}

// STORY-553: `aida review <SPEC>` surface classification. Pure — drives
// off a constructed GitLinkage + ChangeLookup, no repo needed.
fn linkage(
    shipped: bool,
    shipped_pr: Option<u64>,
    branch: Option<&str>,
    commits: usize,
) -> GitLinkage {
    GitLinkage {
        commits: (0..commits)
            .map(|i| (format!("sha{i}"), format!("s{i}"), format!("subj {i}")))
            .collect(),
        files: Vec::new(),
        shipped,
        branch: branch.map(|b| b.to_string()),
        worktree: None,
        shipped_pr,
    }
}

#[test]
fn review_surface_open_change_only_on_found() {
    let l = linkage(false, None, Some("story-553"), 2);
    let found = crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
        id: 42,
        url: "https://example/pr/42".to_string(),
        branch: "story-553".to_string(),
        base: "main".to_string(),
        title: None,
    });
    match classify_review_surface(&l, Some(found)) {
        ReviewSurface::OpenChange { number, .. } => assert_eq!(number, 42),
        other => panic!("expected OpenChange, got {other:?}"),
    }
}

#[test]
fn review_surface_closed_pr_is_not_asserted_open() {
    // BUG-493: a closed/absent PR must degrade to BranchNoChange, never
    // be reported as an open change.
    for lookup in [
        crate::forge::ChangeLookup::NoChange,
        crate::forge::ChangeLookup::CliMissing,
        crate::forge::ChangeLookup::CliFailed("boom".into()),
        crate::forge::ChangeLookup::Unreachable("offline".into()),
    ] {
        let l = linkage(false, None, Some("story-553"), 3);
        match classify_review_surface(&l, Some(lookup)) {
            ReviewSurface::BranchNoChange { branch, commits } => {
                assert_eq!(branch, "story-553");
                assert_eq!(commits, 3);
            }
            other => panic!("expected BranchNoChange, got {other:?}"),
        }
    }
}

#[test]
fn review_surface_shipped_wins_over_branch() {
    let l = linkage(true, Some(7), Some("story-553"), 1);
    match classify_review_surface(&l, None) {
        ReviewSurface::Shipped { number } => assert_eq!(number, Some(7)),
        other => panic!("expected Shipped, got {other:?}"),
    }
}

#[test]
fn review_surface_local_when_no_branch() {
    let l = linkage(false, None, None, 0);
    assert!(matches!(
        classify_review_surface(&l, None),
        ReviewSurface::Local
    ));
}

// BUG-582: a finished (`Completed`) spec is NEVER reviews-awaiting, even
// when a lingering local branch still classifies as an OPEN review surface
// (the stale-Agent-tool-worktree false positive that put already-merged
// BUG-581 / PR-1048 back on the operator's seat). Constructed off the same
// GitLinkage + ChangeLookup seam the surface tests use — no repo/forge.
// trace:BUG-582 | ai:claude
#[test]
fn completed_spec_with_open_surface_is_not_review_awaiting() {
    // A genuinely-open PR surface — the legitimate "needs review" shape...
    let l = linkage(false, None, Some("worktree-agent-abc"), 2);
    let found = crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
        id: 1048,
        url: "https://example/pr/1048".to_string(),
        branch: "worktree-agent-abc".to_string(),
        base: "main".to_string(),
        title: None,
    });
    let surface = classify_review_surface(&l, Some(found));
    assert!(
        matches!(surface, ReviewSurface::OpenChange { .. }),
        "precondition: lingering branch classifies as an open surface"
    );
    // ...but a Completed spec must STILL be excluded — the invariant.
    assert!(
        !spec_eligible_for_review_awaiting(aida_core::RequirementStatus::Completed, &surface),
        "Completed spec must never be reviews-awaiting, even with an open surface"
    );
    // Rejected (abandoned) is likewise excluded.
    assert!(!spec_eligible_for_review_awaiting(
        aida_core::RequirementStatus::Rejected,
        &surface
    ));
}

// BUG-582: the LEGITIMATE case must still pass — a Done spec with an open
// PR not yet merged IS reviews-awaiting. trace:BUG-582 | ai:claude
#[test]
fn done_spec_with_open_pr_is_still_review_awaiting() {
    let l = linkage(false, None, Some("story-553"), 2);
    let found = crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
        id: 42,
        url: "https://example/pr/42".to_string(),
        branch: "story-553".to_string(),
        base: "main".to_string(),
        title: None,
    });
    let surface = classify_review_surface(&l, Some(found));
    for status in [
        aida_core::RequirementStatus::Done,
        aida_core::RequirementStatus::InProgress,
        aida_core::RequirementStatus::Approved,
    ] {
        assert!(
            spec_eligible_for_review_awaiting(status, &surface),
            "an active spec with an OPEN PR must remain reviews-awaiting"
        );
    }
}

// BUG-582: a merged (`Shipped`) surface is not a review even for an active
// status — the merged-PR signal drops it. trace:BUG-582 | ai:claude
#[test]
fn merged_surface_is_not_review_awaiting() {
    let l = linkage(true, Some(7), Some("story-553"), 1);
    let surface = classify_review_surface(&l, None);
    assert!(matches!(surface, ReviewSurface::Shipped { .. }));
    assert!(!spec_eligible_for_review_awaiting(
        aida_core::RequirementStatus::Done,
        &surface
    ));
}

// BUG-722 class (a): a spec whose only branch is the `aida-store` orphan
// requirements-store branch is NEVER reviews-awaiting — that branch is not a
// code-review target (the STORY-760 false positive). Even Done, even with a
// pushed branch that classifies as an open review surface. Pure — same
// GitLinkage seam the surface tests use, no repo/forge. trace:BUG-722
#[test]
fn store_branch_spec_is_not_review_awaiting() {
    let l = linkage(false, None, Some("aida-store"), 2);
    let surface = classify_review_surface(&l, None);
    assert!(
        matches!(surface, ReviewSurface::BranchNoChange { .. }),
        "precondition: the store branch classifies as an open branch surface"
    );
    assert_eq!(
        classify_human_review_bucket(aida_core::RequirementStatus::Done, false, false, &surface,),
        HumanReviewBucket::Excluded,
        "the aida-store store branch must never be reviews-awaiting"
    );
}

// BUG-722 class (b): a deferred spec with a live branch is NOT reviews-
// awaiting — a deferred spec is hidden from the human's seat exactly as
// `aida list` hides it (the TASK-963 false positive). trace:BUG-722
#[test]
fn deferred_spec_with_branch_is_not_review_awaiting() {
    let l = linkage(false, None, Some("forge-slice2-change-metadata"), 2);
    let surface = classify_review_surface(&l, None);
    assert_eq!(
        classify_human_review_bucket(
            aida_core::RequirementStatus::Draft,
            false, // not archived
            true,  // deferred
            &surface,
        ),
        HumanReviewBucket::Excluded,
        "a deferred spec is not a review gate even with a live branch"
    );
}

// BUG-722 class (c): an archived spec with a live branch is NOT reviews-
// awaiting — same view-state hide as `aida list`. trace:BUG-722
#[test]
fn archived_spec_with_branch_is_not_review_awaiting() {
    let l = linkage(false, None, Some("story-archived"), 2);
    let surface = classify_review_surface(&l, None);
    assert_eq!(
        classify_human_review_bucket(
            aida_core::RequirementStatus::InProgress,
            true,  // archived
            false, // not deferred
            &surface,
        ),
        HumanReviewBucket::Excluded,
        "an archived spec is not a review gate even with a live branch"
    );
}

// BUG-722 class (d): a draft spec with only a pushed WIP branch and no PR
// lands under `wip-branches`, NOT `reviews-awaiting` (operator decision
// 2026-07-12: nothing awaits a human until there's a PR or a Done claim).
// The same draft WITH an open PR is still a genuine review. trace:BUG-722
#[test]
fn draft_wip_branch_no_pr_is_wip_not_review_awaiting() {
    let l = linkage(false, None, Some("wip-draft"), 3);
    let surface = classify_review_surface(&l, None);
    assert!(
        matches!(surface, ReviewSurface::BranchNoChange { .. }),
        "precondition: a pushed branch with no PR is BranchNoChange"
    );
    assert_eq!(
        classify_human_review_bucket(aida_core::RequirementStatus::Draft, false, false, &surface,),
        HumanReviewBucket::WipBranch,
        "a draft with only a WIP branch and no PR is loose WIP, not a review gate"
    );
    // ...but the same draft WITH an open PR is a genuine review.
    let found = crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
        id: 55,
        url: "https://example/pr/55".to_string(),
        branch: "wip-draft".to_string(),
        base: "main".to_string(),
        title: None,
    });
    let l2 = linkage(false, None, Some("wip-draft"), 3);
    let pr_surface = classify_review_surface(&l2, Some(found));
    assert_eq!(
        classify_human_review_bucket(
            aida_core::RequirementStatus::Draft,
            false,
            false,
            &pr_surface,
        ),
        HumanReviewBucket::ReviewsAwaiting,
        "a draft with an OPEN PR is a genuine review, not loose WIP"
    );
}

// BUG-539: `aida review <SPEC>` must short-circuit on a terminal/merged
// spec rather than re-running a full review + Approve/Request-changes menu.
#[test]
fn review_terminal_noop_short_circuits_completed_and_rejected() {
    assert!(
        review_is_terminal_noop(&RequirementStatus::Completed),
        "Completed (merged) work has nothing to review"
    );
    assert!(
        review_is_terminal_noop(&RequirementStatus::Rejected),
        "Rejected work has nothing to review"
    );
}

#[test]
fn review_terminal_noop_leaves_pre_merge_states_reviewable() {
    // Done is the normal pre-review state — must NOT short-circuit.
    for s in [
        RequirementStatus::Draft,
        RequirementStatus::Approved,
        RequirementStatus::Planned,
        RequirementStatus::InProgress,
        RequirementStatus::Done,
    ] {
        assert!(
            !review_is_terminal_noop(&s),
            "{s:?} still has a live review surface"
        );
    }
}

/// TASK-241: a spec nothing references — no commits, no trace files.
#[test]
fn linkage_for_never_touched_spec_is_empty() {
    let (_tmp, root) = init_repo();
    let l = collect_git_linkage(&root, &["TASK-804".to_string()]);
    assert!(l.commits.is_empty(), "no commits reference the spec");
    assert!(l.files.is_empty(), "no trace files reference the spec");
    assert!(!l.shipped);
}

/// TASK-241: a shipped spec — commit on main + a squash `(#NN)`
/// subject + a trace comment in a source file.
#[test]
fn linkage_for_shipped_spec() {
    let (_tmp, root) = init_repo();
    std::fs::write(root.join("touched.rs"), "// trace:TASK-805\nfn ship() {}\n").unwrap();
    commit(
        &root,
        "touched.rs",
        "// trace:TASK-805\nfn ship() {}\n",
        "feat: ship it (TASK-805) (#42)",
    );

    let l = collect_git_linkage(&root, &["TASK-805".to_string()]);
    assert_eq!(l.commits.len(), 1, "one referencing commit");
    assert!(l.shipped, "commit is an ancestor of main");
    assert_eq!(
        l.shipped_pr,
        Some(42),
        "PR number parsed from squash subject"
    );
    assert_eq!(l.files.len(), 1, "the trace comment was found");
    assert_eq!(l.files[0].0, "touched.rs");
}

/// TASK-241: an in-flight spec — commit on a feature branch, not on
/// main; the feature branch is reported and `shipped` is false.
#[test]
fn linkage_for_in_flight_spec() {
    let (_tmp, root) = init_repo();
    git(&root, &["checkout", "-q", "-b", "feature/y"]);
    commit(&root, "c.txt", "z", "feat: flying (TASK-806)");
    git(&root, &["checkout", "-q", "main"]);

    let l = collect_git_linkage(&root, &["TASK-806".to_string()]);
    assert_eq!(l.commits.len(), 1);
    assert!(!l.shipped, "commit is not on main");
    assert_eq!(l.branch.as_deref(), Some("feature/y"));
}

/// BUG-720: a spec whose ONLY referencing commit lives on the orphan
/// `aida-store` requirements-store branch — a cross-node store-lineage
/// merge commit that names the spec in a trailing paren group, exactly
/// the shape `aida db sync` produces (`merge: reconcile gitlab store
/// lineage (node-6 spock registrations) into unified store (SPEC-ID)`)
/// — must resolve to NO review branch, and specifically never to
/// `aida-store` itself (bare or remote-qualified, e.g. `gitlab/aida-store`
/// on a multi-hub project like STORY-760). Falling back to it made `aida
/// review` / `aida human review` offer to open a PR from the
/// requirements store as if it were a code change.
#[test]
fn linkage_for_spec_referenced_only_by_orphan_store_branch_has_no_branch() {
    let (_tmp, root) = init_repo();
    git(&root, &["checkout", "-q", "--orphan", "aida-store"]);
    std::fs::write(root.join("objects.yaml"), "id: TASK-808\n").unwrap();
    git(&root, &["add", "."]);
    git(
        &root,
        &[
            "commit",
            "-q",
            "-m",
            "merge: reconcile gitlab store lineage (node-6 spock registrations) \
                 into unified store (TASK-808)",
        ],
    );
    // A multi-hub project mirrors the orphan branch under more than one
    // remote (STORY-760); a synthetic remote-tracking ref reproduces the
    // exact candidate `git branch --all --contains` reported in the
    // observed bug: `gitlab/aida-store`, not just the bare local name.
    let sha = git(&root, &["rev-parse", "aida-store"]);
    git(
        &root,
        &["update-ref", "refs/remotes/gitlab/aida-store", &sha],
    );
    git(&root, &["checkout", "-q", "main"]);

    let l = collect_git_linkage(&root, &["TASK-808".to_string()]);
    assert_eq!(l.commits.len(), 1, "the store-lineage commit is found");
    assert!(!l.shipped, "the orphan branch is not an ancestor of main");
    assert_eq!(
        l.branch, None,
        "must never resolve to the orphan aida-store branch (bare or remote-qualified)"
    );
}

/// BUG-720: the guard predicate itself, isolated from git — a bare
/// `aida-store` and every remote-qualified short ref for it must match;
/// a real code branch that merely contains the substring must not.
#[test]
fn is_orphan_store_branch_matches_bare_and_remote_qualified_refs() {
    assert!(is_orphan_store_branch("aida-store"));
    assert!(is_orphan_store_branch("origin/aida-store"));
    assert!(is_orphan_store_branch("gitlab/aida-store"));
    assert!(is_orphan_store_branch("AIDA-STORE"), "case-insensitive");
    assert!(!is_orphan_store_branch("aida-store-backup"));
    assert!(!is_orphan_store_branch("feature/aida-store-migration"));
    assert!(!is_orphan_store_branch("main"));
}

/// BUG-550: `collect_git_linkage_opts(.., false)` — the path the widened
/// `aida human` reviews bucket uses — must still resolve the commit/branch/
/// shipped state (the only fields the review-surface classifier reads) while
/// SKIPPING the source-tree walk that populates `files`. That skip is what
/// keeps the now-wider candidate set fast on this hot command.
#[test]
fn linkage_opts_no_scan_keeps_review_state_drops_files() {
    let (_tmp, root) = init_repo();
    std::fs::write(root.join("touched.rs"), "// trace:TASK-807\nfn ship() {}\n").unwrap();
    commit(
        &root,
        "touched.rs",
        "// trace:TASK-807\nfn ship() {}\n",
        "feat: ship it (TASK-807) (#43)",
    );

    // With the tree scan ON, the trace comment is found (control).
    let scanned = collect_git_linkage_opts(&root, &["TASK-807".to_string()], true);
    assert_eq!(scanned.files.len(), 1, "scan ON finds the trace comment");

    // With it OFF, commit/shipped state is identical but `files` is empty.
    let lean = collect_git_linkage_opts(&root, &["TASK-807".to_string()], false);
    assert_eq!(lean.commits.len(), 1, "commit still resolved without scan");
    assert!(lean.shipped, "shipped state still computed without scan");
    assert_eq!(lean.shipped_pr, Some(43), "PR number still parsed");
    assert!(
        lean.files.is_empty(),
        "scan OFF skips the source-tree walk, so files stays empty"
    );
}

/// A fresh repo with `--initial-branch=<name>` and one initial
/// commit — used for the BUG-380 master-default coverage.
fn init_repo_on(branch: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    git(
        &root,
        &["init", &format!("--initial-branch={}", branch), "--quiet"],
    );
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "init\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "chore: init"]);
    (tmp, root)
}

/// BUG-380: a repo whose default branch is `master` (no `main`
/// exists). The ancestor check must use `master`, not splice
/// `fatal: Not a valid object name 'main'` to stderr — and the
/// shipped commit must still be detected.
#[test]
fn linkage_for_shipped_spec_on_master_default_repo() {
    let (_tmp, root) = init_repo_on("master");
    commit(
        &root,
        "touched.rs",
        "// trace:BUG-380\nfn ship() {}\n",
        "fix: master default branch (BUG-380) (#380)",
    );

    let l = collect_git_linkage(&root, &["BUG-380".to_string()]);
    assert_eq!(l.commits.len(), 1, "one referencing commit");
    assert!(
        l.shipped,
        "commit on master should be shipped when master is the default branch"
    );
    assert_eq!(l.shipped_pr, Some(380));
}

/// BUG-380: regression — a `main`-default repo must keep working
/// the way it did before the resolver landed.
#[test]
fn linkage_for_shipped_spec_on_main_default_repo_unchanged() {
    let (_tmp, root) = init_repo_on("main");
    commit(
        &root,
        "touched.rs",
        "// trace:BUG-380\nfn ship() {}\n",
        "fix: main default still works (BUG-380) (#381)",
    );

    let l = collect_git_linkage(&root, &["BUG-380".to_string()]);
    assert_eq!(l.commits.len(), 1);
    assert!(l.shipped);
    assert_eq!(l.shipped_pr, Some(381));
}

/// BUG-380: the resolver picks the master ref when only master
/// exists, so the caller never passes a literal `"main"` to
/// `git merge-base --is-ancestor`.
#[test]
fn resolve_default_branch_picks_master_when_main_absent() {
    let (_tmp, root) = init_repo_on("master");
    assert_eq!(resolve_default_branch_ref(&root).as_deref(), Some("master"));
}

/// BUG-380: the resolver picks `main` on a main-default repo.
#[test]
fn resolve_default_branch_picks_main_when_present() {
    let (_tmp, root) = init_repo_on("main");
    assert_eq!(resolve_default_branch_ref(&root).as_deref(), Some("main"));
}
