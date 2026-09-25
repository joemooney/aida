//! BUG-1609: reviewer preflight (`aida queue work --role reviewer`) must
//! resolve change metadata through the configured Forge implementation, not
//! `gh`, when `[forge] provider = "gitlab"`. Covers:
//!   - `pr_base_head` resolving a GitLab MR's real source/target branches
//!     through an injected `glab`, never touching `gh`.
//!   - the review-story title round-tripping to `ReviewForge::GitLab`
//!     (the actual BUG-1609 root cause: a hardcoded "Review PR-N" title
//!     made `parse_review_scope` mis-tag a GitLab MR as a GitHub PR
//!     downstream).
//!   - `reviewer_preflight_pr_n` firing for any forge, not just GitHub.
//!   - the `aida pr auto-queue-review` help text being forge-neutral.
//! trace:BUG-1609 | ai:claude

use super::*;

#[cfg(unix)]
fn write_executable(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// A fake `glab` that answers `api -X GET projects/:id/merge_requests/<n>`
/// with a real-shaped MR object whose source/target branches are NOT
/// GitHub-shaped (`mr-1`/`main`) — so a test that still sees those values
/// proves the fallback fired instead of the forge-routed read.
#[cfg(unix)]
fn fake_glab_mr(
    dir: &std::path::Path,
    source_branch: &str,
    target_branch: &str,
) -> std::path::PathBuf {
    let glab = dir.join("glab");
    write_executable(
        &glab,
        &format!(
            "#!/bin/sh\n\
             printf '{{\"iid\":1,\"source_branch\":\"{source}\",\"target_branch\":\"{target}\",\"title\":\"t\",\"state\":\"opened\",\"sha\":\"deadbeef\"}}'\n",
            source = source_branch,
            target = target_branch,
        ),
    );
    glab
}

/// A fake `gh` that writes a marker file if it is EVER invoked, and exits
/// non-zero — proves the GitLab path never shells out to `gh`.
#[cfg(unix)]
fn fake_gh_sentinel(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let marker = dir.join("gh-was-called.marker");
    let gh = dir.join("gh");
    write_executable(
        &gh,
        &format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    );
    (gh, marker)
}

#[cfg(unix)]
#[test]
fn pr_base_head_resolves_real_gitlab_mr_branches_via_forge_not_gh() {
    let tmp = tempfile::tempdir().unwrap();
    let glab = fake_glab_mr(tmp.path(), "story-52-work", "main");
    let (gh, gh_marker) = fake_gh_sentinel(tmp.path());
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    let project = tempfile::tempdir().unwrap();
    let (base, head) = pr_base_head(project.path(), ReviewForge::GitLab, 1)
        .expect("GitLab MR metadata resolves through the forge-routed glab");

    assert_eq!(base, "main");
    assert_eq!(head, "story-52-work");
    // Neither the GitHub-shaped fallback name nor `gh` fired.
    assert_ne!(head, "pr-1");
    assert!(
        !gh_marker.exists(),
        "`gh` must never be invoked when resolving a GitLab MR"
    );
}

#[cfg(unix)]
#[test]
fn pr_base_head_falls_back_when_glab_is_unavailable_and_names_mr_not_pr() {
    // No AIDA_TEST_GLAB_BINARY override and no real glab on PATH: force the
    // fallback path and confirm it stays GitLab-shaped (`mr-N`), never the
    // GitHub-shaped `pr-N` the live BUG-1609 report observed.
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", "/nonexistent/glab-does-not-exist"),
        ("AIDA_TEST_GH_BINARY", "/nonexistent/gh-does-not-exist"),
    ]);
    let project = tempfile::tempdir().unwrap();
    let (base, head) =
        pr_base_head(project.path(), ReviewForge::GitLab, 1).expect("fallback never errors");
    assert_eq!(base, "main");
    assert_eq!(head, "mr-1", "GitLab fallback head must stay MR-shaped");
}

/// BUG-1609 root cause: `auto_queue_review` used to hardcode
/// `format!("Review PR-{}: …", …)` regardless of forge. Fixed to use
/// `format_review_label(review_forge, …)`. This pins the round-trip: a
/// GitLab review-story title must parse back to `ReviewForge::GitLab`, not
/// silently resolve to GitHub downstream (`parse_review_scope`,
/// `derive_scope_from_entry`, the reviewer preflight gate).
#[test]
fn gitlab_review_story_title_round_trips_to_gitlab_forge() {
    let pr_number = 1u64;
    let title = format!(
        "Review {}: {}",
        format_review_label(ReviewForge::GitLab, pr_number),
        "some MR title"
    );
    assert_eq!(title, "Review MR-1: some MR title");

    // What `derive_scope_from_entry` (queue_cmd.rs) does with a
    // "Review <LABEL>: …" title: strip the prefix, take the first token,
    // parse it back through the same `parse_review_scope` it uses.
    let rest = title.strip_prefix("Review ").unwrap();
    let pr_token = rest.split([':', ' ']).next().unwrap_or("");
    assert_eq!(
        parse_review_scope(pr_token),
        Some((ReviewForge::GitLab, pr_number)),
        "a GitLab MR's title must parse back to ReviewForge::GitLab, not GitHub"
    );
}

/// STORY-281 / TASK-480: the reviewer pre-flight predicate must fire for
/// ANY forge with a resolved review target, not just GitHub — the BUG-1609
/// call-site regression (`Some((ReviewForge::GitHub, pr_n))` silently
/// dropped every GitLab MR).
#[test]
fn reviewer_preflight_pr_n_fires_for_every_forge() {
    assert_eq!(
        crate::queue_cmd::reviewer_preflight_pr_n(Some((ReviewForge::GitHub, 7))),
        Some(7)
    );
    assert_eq!(
        crate::queue_cmd::reviewer_preflight_pr_n(Some((ReviewForge::GitLab, 7))),
        Some(7),
        "a GitLab MR must also trigger the reviewer preflight checks"
    );
    assert_eq!(crate::queue_cmd::reviewer_preflight_pr_n(None), None);
}

/// The `aida pr auto-queue-review` help text used to say it detects the
/// change via `gh pr list`, even though the command already handles GitLab
/// MRs. It must be forge-neutral.
// trace:BUG-1609 | ai:claude
#[test]
fn auto_queue_review_help_text_is_forge_neutral() {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let pr_cmd = cmd
        .get_subcommands()
        .find(|s| s.get_name() == "pr")
        .expect("pr subcommand exists")
        .clone();
    let mut auto_queue_review = pr_cmd
        .get_subcommands()
        .find(|s| s.get_name() == "auto-queue-review")
        .expect("auto-queue-review subcommand exists")
        .clone();
    let help = auto_queue_review.render_long_help().to_string();
    assert!(
        !help.contains("gh pr list"),
        "help text must not hardcode the GitHub-only lookup: {help}"
    );
    assert!(
        help.contains("glab") && help.contains("gh "),
        "help text should name both providers' CLIs: {help}"
    );
}
