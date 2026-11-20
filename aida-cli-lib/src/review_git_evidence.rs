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
    let mut cmd = aida_core::git_ops::host_git_command(project_root);
    cmd.args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{rev}^{{commit}}"));
    // A scoped read deadline (`train --check --timeout`) bounds the read.
    // trace:TASK-3792 | ai:claude
    crate::forge_lookup::deadline_output(cmd, std::process::Command::output)
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
    // Git's two-dot syntax defaults either omitted endpoint to HEAD.
    // Pass the same endpoints to commands that take revisions separately.
    let base = if base.is_empty() { "HEAD" } else { base };
    let head = if head.is_empty() { "HEAD" } else { head };
    let trees = |range: &str, reverse: bool| -> Result<Vec<(String, String)>> {
        let mut cmd = aida_core::git_ops::host_git_command(repo);
        cmd.args(["log", "--first-parent", "--format=%H %T"]);
        if reverse {
            cmd.arg("--reverse");
        }
        cmd.args([git_arg_guard::END_OF_OPTIONS, range, "--"]);
        // trace:TASK-3792 | ai:claude
        let out = crate::forge_lookup::deadline_output(cmd, std::process::Command::output)?;
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
    // trace:BUG-2632 | ai:codex
    // Shallow PR checkouts (and unrelated histories) may have no visible
    // common ancestor. Without that evidence, retain every declaration.
    if common.status.code() == Some(1) {
        return Ok(range);
    }
    anyhow::ensure!(
        common.status.success(),
        "cannot resolve completion range ancestry: {}",
        String::from_utf8_lossy(&common.stderr).trim()
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

/// Read first-parent constituent merges down to the preceding promotion.
// trace:BUG-2660 | ai:codex
pub(crate) fn promotion_merge_subjects(
    repo: &std::path::Path,
    base: &str,
    branch: &str,
    head: &str,
) -> Result<Vec<String>> {
    // Read local, fully qualified refs only. Prefer remote-tracking evidence
    // because the promotion may have been opened from a different checkout.
    let resolve = |branch: &str| -> Result<String> {
        for namespace in ["refs/remotes/origin", "refs/heads"] {
            let rev = format!("{namespace}/{branch}^{{commit}}");
            let out = aida_core::git_ops::host_git_command(repo)
                .args([
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    git_arg_guard::END_OF_OPTIONS,
                    &rev,
                ])
                .output()?;
            if out.status.success() {
                return Ok(String::from_utf8(out.stdout)?.trim().to_owned());
            }
        }
        anyhow::bail!("missing local ref for `{branch}`; fetch origin and retry")
    };
    let first_promotion = branch == "promote/integration-1";
    let base = resolve(base)?;
    anyhow::ensure!(
        crate::forge::is_full_commit_sha(head),
        "invalid promotion head"
    );
    // Refuse partial history: a shallow boundary can hide the previous
    // promotion or truncate a run of equal trees.
    let shallow = aida_core::git_ops::host_git_command(repo)
        .args(["rev-parse", "--is-shallow-repository"])
        .output()?;
    anyhow::ensure!(
        shallow.status.success() && shallow.stdout.trim_ascii_end() == b"false",
        "complete local promotion history is required"
    );
    const LIMIT: usize = 500;
    let limit = format!("--max-count={}", LIMIT + 1);
    let log = |range: &str, format: &str| -> Result<String> {
        let mut cmd = aida_core::git_ops::host_git_command(repo);
        cmd.args([
            "log",
            "--first-parent",
            &limit,
            format,
            git_arg_guard::END_OF_OPTIONS,
            range,
            "--",
        ]);
        let cap = 1024 * 1024;
        let crate::BoundedCommandOutput::Completed(out) =
            crate::command_output_with_timeout_capped(cmd, std::time::Duration::from_secs(3), cap)
        else {
            anyhow::bail!("promotion history lookup did not finish");
        };
        anyhow::ensure!(
            out.stdout.len() as u64 <= cap && out.stderr.len() as u64 <= cap,
            "promotion history exceeded the output limit"
        );
        anyhow::ensure!(
            out.status.success(),
            "cannot read promotion history: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };

    // A train squash can contain concurrent main changes, so its tree may
    // differ from the original promotion head. Its member trailer pins that
    // original head. Plain squash landings can still match by tree.
    let landed = log(
        &base,
        "--format=%T%x1f%s%x1f%(trailers:key=Aida-Train-Member,valueonly,separator=%x1e,unfold=true)",
    )?;
    let mut landed_trees = std::collections::HashSet::new();
    let mut landed_heads = std::collections::HashSet::new();
    for row in landed.lines().take(LIMIT) {
        let Some((tree, rest)) = row.split_once('\x1f') else {
            anyhow::bail!("malformed base promotion history");
        };
        let Some((subject, receipts)) = rest.rsplit_once('\x1f') else {
            anyhow::bail!("malformed base promotion history");
        };
        // An unrelated main commit can share a tree (for example, a revert).
        // Only the established promotion suffix identifies a plain landing.
        let promotion_subject = subject
            .rsplit_once(" [promotion ")
            .is_some_and(|(_, tail)| {
                tail.split_once(']').is_some_and(|(number, after)| {
                    !number.is_empty()
                        && number.bytes().all(|b| b.is_ascii_digit())
                        && (after.is_empty()
                            || crate::pr_lookup::extract_pr_number_from_commit_subject(after)
                                .is_some())
                })
            });
        if receipts.is_empty() && promotion_subject {
            landed_trees.insert(tree);
        }
        for receipt in receipts.split('\x1e') {
            if let Some((number, sha)) = receipt.trim().split_once(" at ") {
                if number
                    .strip_prefix('#')
                    .is_some_and(|n| n.parse::<u64>().is_ok())
                    && crate::forge::is_full_commit_sha(sha)
                {
                    landed_heads.insert(sha.to_ascii_lowercase());
                }
            }
        }
    }

    let history = log(&format!("{base}..{head}"), "--format=%H%x1f%T%x1f%P%x1f%s")?;
    let mut subjects = Vec::new();
    let mut tree_cut = None;
    let mut ambiguous_tree = false;
    for (index, row) in history.lines().enumerate() {
        let mut fields = row.splitn(4, '\x1f');
        let (Some(sha), Some(tree), Some(parents), Some(subject)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            anyhow::bail!("malformed promotion history");
        };
        if let Some((cut_tree, _)) = tree_cut {
            if tree != cut_tree {
                break;
            }
        }
        anyhow::ensure!(index < LIMIT, "promotion history exceeded {LIMIT} commits");
        if landed_heads.contains(sha) {
            return Ok(subjects);
        }
        let constituent = parents.split_whitespace().count() == 2
            && crate::integration_cmd::review::merged_pr(subject).is_some();
        if landed_trees.contains(tree) {
            // An exact receipt can resolve a run of equal trees. A tree match
            // alone cannot tell which merges in that run were promoted.
            if let Some((_, len)) = tree_cut {
                ambiguous_tree |= subjects.len() != len || constituent;
            } else {
                tree_cut = Some((tree, subjects.len()));
            }
        }
        if constituent {
            subjects.push(subject.to_owned());
        }
    }
    if let Some((_, len)) = tree_cut {
        anyhow::ensure!(
            !ambiguous_tree,
            "equal-tree commits obscure the promotion boundary"
        );
        subjects.truncate(len);
        return Ok(subjects);
    }
    anyhow::ensure!(
        history.is_empty() || first_promotion && history.lines().count() <= LIMIT,
        "promotion boundary could not be established within {LIMIT} commits"
    );
    Ok(subjects)
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
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "log",
            "--pretty=format:%B%n--END--",
            git_arg_guard::END_OF_OPTIONS,
            &range,
            "--",
        ]);
    // trace:TASK-3792 | ai:claude
    let out = crate::forge_lookup::deadline_output(cmd, std::process::Command::output)
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
    fn shallow_pr_checkout_retains_completion_declarations() {
        let source = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let git = |repo: &std::path::Path, args: &[&str]| {
            let out = crate::test_git::git_command()
                .current_dir(repo)
                .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        };
        git(source.path(), &["init", "-b", "main"]);
        git(source.path(), &["commit", "--allow-empty", "-m", "base"]);
        git(source.path(), &["checkout", "-b", "topic"]);
        git(
            source.path(),
            &["commit", "--allow-empty", "-m", "claim (BUG-4)"],
        );
        git(source.path(), &["checkout", "main"]);
        git(
            source.path(),
            &["commit", "--allow-empty", "-m", "main work"],
        );
        let url = format!("file://{}", source.path().display());
        git(
            checkout.path(),
            &["clone", "--depth=1", "--branch=topic", &url, "."],
        );
        // Match CI: fetch main as FETCH_HEAD and a new remote branch ref,
        // without deepening the PR head's ancestry.
        git(
            checkout.path(),
            &[
                "fetch",
                "--depth=1",
                "origin",
                "main:refs/remotes/origin/main",
            ],
        );
        assert_eq!(
            completion_range(checkout.path(), "origin/main", "HEAD").unwrap(),
            "origin/main..HEAD"
        );
        let commits = crate::read_commits_in_range(checkout.path(), "origin/main..HEAD").unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].1, "claim (BUG-4)");
        // Missing ancestry is conservative; an invalid ref must still refuse
        // rather than silently discard the completion inspection error.
        let error = completion_range(checkout.path(), "refs/heads/missing", "HEAD").unwrap_err();
        assert!(error
            .to_string()
            .contains("cannot inspect completion range"));
    }

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

    // trace:BUG-2660 | ai:codex
    #[test]
    fn bug_2660_reads_only_new_promotion_merge_subjects() {
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
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-b", "main"]);
        git(&["commit", "--allow-empty", "-m", "base"]);
        git(&["switch", "-c", "integration"]);
        git(&["switch", "-c", "old-work"]);
        std::fs::write(repo.join("old"), "old").unwrap();
        git(&["add", "old"]);
        git(&["commit", "-m", "old work (BUG-1)"]);
        git(&["switch", "integration"]);
        git(&["merge", "--no-ff", "-m", "merge PR #1 (BUG-1)", "old-work"]);
        git(&["switch", "-c", "old-slice"]);
        std::fs::write(repo.join("old-slice"), "old slice").unwrap();
        git(&["add", "old-slice"]);
        git(&["commit", "-m", "old slice implementation"]);
        git(&["switch", "integration"]);
        git(&[
            "merge",
            "--no-ff",
            "-m",
            "merge PR #10 (Refs STORY-1)",
            "old-slice",
        ]);
        let prior = git(&["rev-parse", "HEAD"]);
        git(&["switch", "main"]);
        git(&["merge", "--squash", "integration"]);
        git(&["commit", "-m", "promotion 1 (BUG-1) [promotion 1] (#1)"]);
        git(&["switch", "integration"]);
        git(&["switch", "-c", "new-work"]);
        std::fs::write(repo.join("new"), "new").unwrap();
        git(&["add", "new"]);
        git(&["commit", "-m", "implementation detail (BUG-99)"]);
        git(&["switch", "-c", "promote/integration-2", &prior]);
        git(&["merge", "--no-ff", "-m", "merge PR #2 (BUG-2)", "new-work"]);
        git(&["switch", "-c", "slice-work"]);
        std::fs::write(repo.join("slice"), "slice").unwrap();
        git(&["add", "slice"]);
        git(&["commit", "-m", "slice implementation"]);
        git(&["switch", "promote/integration-2"]);
        git(&[
            "merge",
            "--no-ff",
            "-m",
            "merge PR #3 (Refs STORY-3)",
            "slice-work",
        ]);
        git(&["merge", "--no-ff", "-m", "merge main (STORY-2737)", "main"]);
        let coverage = |title| {
            crate::review_gates::auto_queue_coverage(
                repo,
                "promote/integration-2",
                title,
                "main",
                &git(&["rev-parse", "promote/integration-2"]),
            )
            .unwrap()
        };
        let inferred = coverage("promotion");
        assert_eq!(inferred.delivered, ["BUG-2"]);
        assert_eq!(inferred.slice_refs, ["STORY-3"]);

        let titled = coverage("promotion (BUG-2)");
        assert_eq!(titled.delivered, ["BUG-2"]);
        assert_eq!(titled.slice_refs, ["STORY-3"]);
        assert_eq!(
            crate::review_gates::review_story_links(&titled.delivered, &titled.slice_refs)
                .map(|(id, kind)| (id.as_str(), kind))
                .collect::<Vec<_>>(),
            [("BUG-2", "implements"), ("STORY-3", "covers")]
        );

        let mixed =
            coverage("promotion mentions BUG-99 (BUG-4 Refs STORY-3 STORY-4) [promotion 2]");
        assert_eq!(mixed.delivered, ["BUG-4"]);
        assert_eq!(mixed.slice_refs, ["STORY-3", "STORY-4"]);
        assert_eq!(
            crate::review_gates::review_story_links(&mixed.delivered, &mixed.slice_refs)
                .map(|(id, kind)| (id.as_str(), kind))
                .collect::<Vec<_>>(),
            [
                ("BUG-4", "implements"),
                ("STORY-3", "covers"),
                ("STORY-4", "covers")
            ]
        );
        // Work landed by the prior promotion (BUG-1, Refs STORY-1) never
        // re-enters coverage, whichever path the title takes.
        for cov in [&inferred, &titled, &mixed] {
            let all: Vec<_> = cov
                .delivered
                .iter()
                .chain(&cov.slice_refs)
                .chain(&cov.referenced)
                .collect();
            assert!(
                !all.iter().any(|id| *id == "BUG-1" || *id == "STORY-1"),
                "{all:?}"
            );
        }
        let overlap = coverage("promotion (BUG-2 STORY-3)");
        assert_eq!(overlap.delivered, ["BUG-2", "STORY-3"]);
        assert!(overlap.slice_refs.is_empty());

        // A promotion ref can advance after the forge observation. The
        // pinned head still produces the original constituent set.
        let branch = "promote/integration-2";
        let head = git(&["rev-parse", branch]);
        git(&[
            "commit",
            "--allow-empty",
            "-m",
            "later local commit (BUG-99)",
        ]);
        assert_eq!(
            promotion_merge_subjects(repo, "main", branch, &head).unwrap(),
            ["merge PR #3 (Refs STORY-3)", "merge PR #2 (BUG-2)"]
        );

        // Equal trees cannot establish whether a no-op merge was promoted.
        git(&["switch", "-c", "promote/ambiguous", &prior]);
        git(&[
            "merge",
            "-s",
            "ours",
            "--no-ff",
            "-m",
            "merge PR #3 (BUG-3)",
            "new-work",
        ]);
        let ambiguous = git(&["rev-parse", "HEAD"]);
        assert!(
            promotion_merge_subjects(repo, "main", "promote/ambiguous", &ambiguous)
                .unwrap_err()
                .to_string()
                .contains("equal-tree")
        );

        // Simulate a train squash that includes concurrent main changes.
        // Its tree is different, but the receipt still identifies the cut.
        git(&["switch", "main"]);
        std::fs::write(repo.join("train"), "concurrent").unwrap();
        git(&["add", "train"]);
        git(&[
            "commit",
            "--amend",
            "-m",
            "promotion 1 (#1)",
            "-m",
            &format!("Aida-Train-Member: #1 at {prior}"),
        ]);
        assert_eq!(coverage("promotion").delivered, ["BUG-2"]);
        git(&["commit", "--amend", "-m", "promotion without a receipt"]);
        let branch = "promote/integration-2";
        let head = git(&["rev-parse", branch]);
        assert!(promotion_merge_subjects(repo, "main", branch, &head).is_err());
        assert!(crate::review_gates::auto_queue_coverage(
            repo,
            branch,
            "promotion (BUG-2)",
            "main",
            &head
        )
        .is_err());
        assert!(promotion_merge_subjects(repo, "missing", branch, &head).is_err());
        assert!(promotion_merge_subjects(repo, "main", branch, &"0".repeat(head.len())).is_err());
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
        let implicit_head = crate::read_commits_in_range(repo, "main..").unwrap();
        assert_eq!(implicit_head, commits);
        git(&["checkout", "main"]);
        let implicit_base = crate::read_commits_in_range(repo, "..promote/integration-2").unwrap();
        assert_eq!(implicit_base, commits);
        git(&["checkout", "promote/integration-2"]);
        // Explicit new declarations with no tree change still belong to this PR.
        git(&["commit", "--allow-empty", "-m", "new claim (BUG-3)"]);
        let messages = crate::git_log_messages(repo, "main", "HEAD").unwrap();
        assert_eq!(
            crate::review_coverage_from_commits(&messages).delivered,
            ["BUG-3", "BUG-2"]
        );
    }
}
