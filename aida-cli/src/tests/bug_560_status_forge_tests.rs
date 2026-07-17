use super::*;

/// Regression for BUG-560: on a non-GitHub forge, `collect_pr_facts` must
/// return `NotGitHub` WITHOUT spawning `gh` — otherwise a GitLab/pure-git
/// user gets gh's raw "known GitHub host" auth error in `aida status`.
/// Uses the `[forge] provider` config path so the test needs no git remote
/// or `gh` binary (deterministic).
#[test]
fn collect_pr_facts_skips_gh_on_gitlab_forge() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        "[forge]\nprovider = \"gitlab\"\n",
    )
    .unwrap();
    let facts = collect_pr_facts(tmp.path(), "feature-branch");
    assert!(
        matches!(
            facts.gh_status,
            GhStatus::NotGitHub(forge::ForgeKind::GitLab)
        ),
        "GitLab forge must yield NotGitHub(GitLab), got {:?}",
        facts.gh_status
    );
    assert_eq!(facts.number, 0);
}

/// A pure-git / unknown remote also skips gh (no leak), reported as the
// `None` forge. trace:BUG-560
#[test]
fn collect_pr_facts_skips_gh_on_pure_git() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        "[forge]\nprovider = \"none\"\n",
    )
    .unwrap();
    let facts = collect_pr_facts(tmp.path(), "feature-branch");
    assert!(matches!(
        facts.gh_status,
        GhStatus::NotGitHub(forge::ForgeKind::None)
    ));
}

/// TASK-1055: warming the status network probes concurrently must NOT
/// change the per-probe results — it only pre-populates the same memo the
/// sequential render reads. A `gitlab` forge short-circuits `gh` entirely
/// (deterministic, no network — the "mock"), so the probe answer is fixed;
/// the test asserts the warmed read matches a direct uncached probe and
/// that `warm_status_network_probes` runs cleanly end-to-end.
// trace:TASK-1055
#[test]
fn warming_status_probes_does_not_change_results() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    // A real (empty) git repo so `collect_branch_facts` resolves a branch
    // name and the warm step exercises the PR-facts thread.
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "t"],
    ] {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(&args)
            .status()
            .unwrap()
            .success());
    }
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[forge]\nprovider = \"gitlab\"\n",
    )
    .unwrap();

    // Ground truth: the raw, uncached probe.
    let uncached = collect_pr_facts_uncached(root, "feature-branch");

    // Warming pre-fills the memos concurrently; it must not change anything.
    warm_status_network_probes(root);
    let warmed = collect_pr_facts(root, "feature-branch");

    assert_eq!(warmed.number, uncached.number);
    assert_eq!(warmed.title, uncached.title);
    assert_eq!(warmed.state, uncached.state);
    assert_eq!(warmed.ci_rollup, uncached.ci_rollup);
    assert!(matches!(
        warmed.gh_status,
        GhStatus::NotGitHub(forge::ForgeKind::GitLab)
    ));

    // The open-PR snapshot is likewise short-circuited to empty on a
    // non-GitHub forge — warmed or not.
    assert!(collect_open_prs(root).by_branch.is_empty());
}

/// TASK-833: `parse_open_pr_snapshot` turns a `gh pr list --json` payload
/// into rows (number / title / head branch) without shelling out — the
/// open-PR section of `aida burndown status` / `aida list inflight` rides
// this. trace:TASK-833
#[test]
fn parse_open_pr_snapshot_parses_number_title_branch() {
    let json = r#"[
            {"number": 760, "title": "feat: add open-PR section", "headRefName": "agent-abc"},
            {"number": 12, "title": "fix: typo", "headRefName": "fix/typo"}
        ]"#;
    let snap = parse_open_pr_snapshot(json);
    assert_eq!(snap.by_branch.len(), 2);
    let a = snap.by_branch.get("agent-abc").expect("agent-abc row");
    assert_eq!(a.number, 760);
    assert_eq!(a.title, "feat: add open-PR section");
    assert_eq!(a.head_branch, "agent-abc");
    let b = snap.by_branch.get("fix/typo").expect("fix/typo row");
    assert_eq!(b.number, 12);
}

/// Empty array → no rows; malformed JSON / missing number → skip silently.
// trace:TASK-833
#[test]
fn parse_open_pr_snapshot_degrades_on_empty_and_malformed() {
    assert!(parse_open_pr_snapshot("[]").by_branch.is_empty());
    assert!(parse_open_pr_snapshot("not json").by_branch.is_empty());
    assert!(parse_open_pr_snapshot("{}").by_branch.is_empty());
    // a row missing the required `number` field is skipped, not panicked on
    let snap = parse_open_pr_snapshot(r#"[{"title": "no number", "headRefName": "b"}]"#);
    assert!(snap.by_branch.is_empty());
}
