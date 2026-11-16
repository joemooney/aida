// trace:TASK-3428 | ai:codex
use crate::{git_arg_guard, network_retry, ReviewForge};
use anyhow::{Context, Result};
use colored::Colorize;

/// trace:STORY-67 | ai:claude
pub(crate) fn pr_base_head(
    project_root: &std::path::Path,
    forge: ReviewForge,
    n: u64,
) -> Result<(String, String)> {
    // STORY-1164: route the review-prompt metadata read through the Forge trait
    // instead of hand-rolling `gh pr view` / `glab mr view` here. The provider
    // owns CLI resolution, retry behavior, and GitLab's REST-shaped metadata.
    if let Ok(m) = crate::forge::forge_for_kind(project_root, forge.forge_kind())
        .change_metadata(n, &mut network_retry::NoopSink)
    {
        if !m.base_ref.is_empty() && !m.head_ref.is_empty() {
            return Ok((m.base_ref, m.head_ref));
        }
    }

    // Fallback: pure-git. We don't know the contributor's base branch,
    // so we point at `main` (the most common case in this codebase) and
    // set `head` to the local review branch (`pr-N` / `mr-N`) the user
    // is presumed to have fetched via `aida session start --owns PR-N`
    // (STORY-61). Tell the user.
    eprintln!(
        "{} couldn't resolve PR base/head via {} — falling back to base=main, head={}-{}; \
         pass an explicit base via `git log <base>..<head>` if this is wrong.",
        "Note:".yellow().bold(),
        forge.cli_name(),
        if matches!(forge, ReviewForge::GitHub) {
            "pr"
        } else {
            "mr"
        },
        n
    );
    let head = match forge {
        ReviewForge::GitHub => format!("pr-{}", n),
        ReviewForge::GitLab => format!("mr-{}", n),
    };
    Ok(("main".to_string(), head))
}

/// How many commits `origin/<base>` (the default branch's last-fetched
/// state) is ahead of `head` — i.e. how far the PR's tree trails the
/// branch its checks and findings are compared against. Purely local: it
/// reads whatever `origin/<base>` already points at and never fetches, so
/// it's cheap enough to run on every prompt generation.
///
/// Returns `None` when the count can't be resolved (no such
/// remote-tracking ref locally, `head` not resolvable, git not on PATH,
/// …). Callers must render that as "unknown", never as `0` — a `0` reads
/// as "you're current" while `None` means "we don't know", and conflating
/// them turns absent evidence into false reassurance.
// trace:BUG-1434 | ai:claude
pub(crate) fn commits_behind_default(
    project_root: &std::path::Path,
    head: &str,
    base: &str,
) -> Option<u64> {
    let remote_base = format!("origin/{}", base);
    let range = format!("{}..{}", head, remote_base);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args(["rev-list", "--count", git_arg_guard::END_OF_OPTIONS, &range])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .ok()
}

/// `git diff --name-only <a>...<b>` (triple-dot: files changed on `b`
/// since its merge-base with `a`). Best-effort — any git failure yields an
/// empty list rather than an error, since overlap is a nicety layered on
/// top of the commits-behind count, not load-bearing on its own.
// trace:BUG-1434 | ai:claude
pub(crate) fn git_diff_name_only(project_root: &std::path::Path, a: &str, b: &str) -> Vec<String> {
    let range = format!("{}...{}", a, b);
    let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "diff",
            "--name-only",
            git_arg_guard::END_OF_OPTIONS,
            &range,
            "--",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Files the intervening commits (`head..origin/<base>`) touch that the
/// PR's own commits (`base..head`) also touch — the overlap where a stale
/// base is most likely to distort a check or finding (acceptance #2 of
/// this bug). Best-effort like `git_diff_name_only`: any git failure
/// yields an empty overlap rather than an error.
// trace:BUG-1434 | ai:claude
pub(crate) fn stale_base_file_overlap(
    project_root: &std::path::Path,
    head: &str,
    base: &str,
) -> Vec<String> {
    let remote_base = format!("origin/{}", base);
    let intervening = git_diff_name_only(project_root, head, &remote_base);
    let pr_own = git_diff_name_only(project_root, base, head);
    let pr_own_set: std::collections::HashSet<&str> = pr_own.iter().map(|s| s.as_str()).collect();
    let mut overlap: Vec<String> = intervening
        .into_iter()
        .filter(|f| pr_own_set.contains(f.as_str()))
        .collect();
    overlap.sort();
    overlap.dedup();
    overlap
}

/// Render the staleness section for `generate_review_prompt`'s `--pr` path.
/// `None` means "say nothing" — either the head is level with
/// `origin/<base>` (acceptance #5: no extra output when current) or the
/// figure legitimately can't be computed and the caller has already fallen
/// back to a different note. `commits_behind_default` returning `None`
/// (PRIN-5: absent evidence, not a false `0`) still produces `Some` text
/// here, stating "unknown" explicitly rather than omitting the section.
// trace:BUG-1434 | ai:claude
pub(crate) fn stale_base_note(
    project_root: &std::path::Path,
    head: &str,
    base: &str,
) -> Option<String> {
    match commits_behind_default(project_root, head, base) {
        None => Some(format!(
            "PR head is an unknown number of commits behind `{base}` — the local \
             `origin/{base}` ref couldn't be resolved, so staleness could not be computed. \
             Treat the checks and findings below as unverified against the current default \
             branch.\n"
        )),
        Some(0) => None,
        Some(n) => {
            let plural = if n == 1 { "" } else { "s" };
            let mut note = format!(
                "PR head is {n} commit{plural} behind `{base}` — the checks and findings below \
                 were computed against a tree that far behind the default branch.\n"
            );
            let overlap = stale_base_file_overlap(project_root, head, base);
            if !overlap.is_empty() {
                let files = overlap
                    .iter()
                    .map(|f| format!("`{f}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                note.push_str(&format!(
                    "This PR also touches file{} those {n} intervening commit{plural} touched, \
                     the case where findings are most likely distorted: {files}\n",
                    if overlap.len() == 1 { "" } else { "s" }
                ));
            }
            Some(note)
        }
    }
}

/// True when `rev` names a commit in the repo at `project_root`.
fn git_rev_resolves(project_root: &std::path::Path, rev: &str) -> bool {
    aida_core::git_ops::host_git_command(project_root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{rev}^{{commit}}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Refresh `refs/remotes/origin/<branch>` from origin. Best effort: a missing
/// branch or an unreachable origin leaves the ref as it was.
fn fetch_origin_branch(project_root: &std::path::Path, branch: &str) {
    use aida_core::git_ops::RemoteCredentialsExt as _;
    let _ = aida_core::git_ops::host_git_command(project_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["fetch", "--quiet", "--no-tags", "origin"])
        .arg(format!("+refs/heads/{branch}:refs/remotes/origin/{branch}"))
        // trace:BUG-2181 | ai:claude
        .remote_credentials(project_root, "origin")
        .output();
}

/// Resolve a forge's bare `base`/`head` branch names into a range this
/// checkout can actually log. A contained lane pushes its branch from a seat
/// clone, so the primary may have no local `<head>` at all, and whatever
/// `origin/<head>` it has may predate the lane's latest push. In that case
/// both ends are re-fetched from origin (a stale `origin/<base>` would pull
/// already-merged trailers into the range) and the range is
/// `origin/<base>..origin/<head>`. Leaves the names untouched when `head`
/// resolves locally or nothing on origin does, so the caller's `git log`
/// error names the range.
// trace:BUG-B6B1278890774EF993D2EBC327862470-035 | ai:claude
pub(crate) fn resolve_log_range(
    project_root: &std::path::Path,
    base: &str,
    head: &str,
) -> (String, String) {
    if git_rev_resolves(project_root, head) {
        return (base.to_string(), head.to_string());
    }
    fetch_origin_branch(project_root, head);
    let remote_head = format!("origin/{head}");
    if !git_rev_resolves(project_root, &remote_head) {
        return (base.to_string(), head.to_string());
    }
    fetch_origin_branch(project_root, base);
    let remote_base = format!("origin/{base}");
    let base = if git_rev_resolves(project_root, &remote_base) {
        remote_base
    } else {
        base.to_string()
    };
    (base, remote_head)
}

/// Exclude an integration prefix whose exact tree already landed through a
/// squash on the base. Only base commits outside the shared ancestry qualify;
/// ordinary shared/empty trees must not erase new completion declarations.
// trace:BUG-2632 | ai:codex
pub(crate) fn completion_range(repo: &std::path::Path, base: &str, head: &str) -> Result<String> {
    let trees = |range: &str, reverse: bool| -> Result<Vec<(String, String)>> {
        let mut cmd = aida_core::git_ops::host_git_command(repo);
        cmd.args(["log", "--first-parent", "--format=%H %T"]);
        if reverse {
            cmd.arg("--reverse");
        }
        let out = cmd
            .args([git_arg_guard::END_OF_OPTIONS, range, "--"])
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "cannot inspect completion range {range}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                line.split_once(' ')
                    .map(|(sha, tree)| (sha.to_string(), tree.to_string()))
            })
            .collect())
    };
    let range = format!("{base}..{head}");
    let landed = trees(&format!("{head}..{base}"), false)?;
    if landed.is_empty() {
        return Ok(range);
    }
    // A shared unchanged tree is not evidence of a squash landing: unrelated
    // empty commits on both branches must retain their new declarations.
    let common = aida_core::git_ops::host_git_command(repo)
        .args(["merge-base", base, head])
        .output()?;
    anyhow::ensure!(
        common.status.success(),
        "cannot resolve completion range ancestry"
    );
    let common_sha = String::from_utf8_lossy(&common.stdout).trim().to_string();
    let common_tree = aida_core::git_ops::host_git_command(repo)
        .args(["show", "-s", "--format=%T", &common_sha])
        .output()?;
    anyhow::ensure!(
        common_tree.status.success(),
        "cannot read completion range ancestor tree"
    );
    let common_tree = String::from_utf8_lossy(&common_tree.stdout)
        .trim()
        .to_string();
    let mut candidates = std::collections::HashMap::new();
    for (sha, tree) in trees(&range, true)? {
        // Preserve declarations made later without changing the landed tree.
        candidates.entry(tree).or_insert(sha);
    }
    for (_, tree) in landed {
        if tree == common_tree {
            continue;
        }
        if let Some(sha) = candidates.get(&tree) {
            return Ok(format!("{sha}..{head}"));
        }
    }
    Ok(range)
}

/// Run `git log <base>..<head> --pretty=format:%B%n--END--`. Returns
/// each commit message as a separate string. A head branch that exists only
/// on origin is resolved via [`resolve_log_range`]. trace:STORY-67 | ai:claude
pub(crate) fn git_log_messages(
    project_root: &std::path::Path,
    base: &str,
    head: &str,
) -> Result<Vec<String>> {
    let (base, head) = resolve_log_range(project_root, base, head);
    let range = completion_range(project_root, &base, &head)
        .with_context(|| format!("cannot read commits {base}..{head}"))?;
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "log",
            "--pretty=format:%B%n--END--",
            git_arg_guard::END_OF_OPTIONS,
            &range,
            "--",
        ])
        .output()
        .with_context(|| format!("running git log {}", range))?;
    if !out.status.success() {
        anyhow::bail!(
            "git log {} failed: {}",
            range,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let messages: Vec<String> = stdout
        .split("--END--")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Ok(messages)
}

#[cfg(test)]
mod bug_2632_tests {
    use super::*;

    // trace:BUG-2632 | ai:codex
    #[test]
    fn unrelated_empty_commits_do_not_establish_a_landed_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let git = |args: &[&str]| {
            let out = crate::test_git::git_command()
                .current_dir(repo)
                .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        };
        git(&["init", "-b", "main"]);
        git(&["commit", "--allow-empty", "-m", "base"]);
        git(&["checkout", "-b", "topic"]);
        git(&["commit", "--allow-empty", "-m", "claim (BUG-4)"]);
        git(&["checkout", "main"]);
        git(&["commit", "--allow-empty", "-m", "unrelated"]);
        let messages = crate::git_log_messages(repo, "main", "topic").unwrap();
        assert_eq!(
            crate::review_coverage_from_commits(&messages).delivered,
            ["BUG-4"]
        );
    }

    // trace:BUG-2632 | ai:codex
    #[test]
    fn second_promotion_only_covers_new_constituents() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let git = |args: &[&str]| {
            let out = crate::test_git::git_command()
                .current_dir(repo)
                .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        };
        git(&["init", "-b", "main"]);
        std::fs::write(repo.join("file"), "base").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "base"]);
        git(&["checkout", "-b", "promote/integration-1"]);
        std::fs::write(repo.join("file"), "first").unwrap();
        git(&["commit", "-am", "first (BUG-1)"]);
        let first = crate::git_log_messages(repo, "main", "HEAD").unwrap();
        assert_eq!(
            crate::review_coverage_from_commits(&first).delivered,
            ["BUG-1"]
        );
        git(&["checkout", "main"]);
        git(&["merge", "--squash", "promote/integration-1"]);
        git(&["commit", "-m", "promotion one (BUG-1)"]);
        // Later main-only work must not hide the historical squash boundary.
        std::fs::write(repo.join("main-only"), "unrelated").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "unrelated main work"]);
        git(&[
            "checkout",
            "-b",
            "promote/integration-2",
            "promote/integration-1",
        ]);
        std::fs::write(repo.join("file"), "second").unwrap();
        git(&["commit", "-am", "second (BUG-2)"]);
        let messages = crate::git_log_messages(repo, "main", "HEAD").unwrap();
        assert_eq!(
            crate::review_coverage_from_commits(&messages).delivered,
            ["BUG-2"]
        );
        let commits = crate::read_commits_in_range(repo, "main..HEAD").unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].1, "second (BUG-2)");
        // Explicit new declarations with no tree change still belong to this PR.
        git(&["commit", "--allow-empty", "-m", "new claim (BUG-3)"]);
        let messages = crate::git_log_messages(repo, "main", "HEAD").unwrap();
        assert_eq!(
            crate::review_coverage_from_commits(&messages).delivered,
            ["BUG-3", "BUG-2"]
        );
    }
}
