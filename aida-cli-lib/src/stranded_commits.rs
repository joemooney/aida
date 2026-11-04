//! `aida doctor` check for work committed in an agent worktree that exists on
//! this machine only. Two shapes:
//!
//! 1. **Unpushed branch** — a worktree's branch whose tip is on no remote, with
//!    content that has not landed on the default branch.
//! 2. **Dangling commit** — a commit named in a worktree's `HEAD` or branch
//!    reflog whose subject names a spec, and which no ref (branch, tag, remote,
//!    or any worktree's `HEAD`) reaches any more: the branch was reset or
//!    deleted before it was pushed.
//!
//! The unshipped-work surface only sees refs; a reset branch leaves the commit
//! reachable from the reflog alone, which `git gc` eventually expires. Reflogs
//! only, a bounded number of entries per worktree, one batched reachability
//! call: no `git fsck`. Pinning a dangling commit to a `recovered/<spec>`
//! branch is the only heal; nothing is ever deleted.
// trace:TASK-1595 | ai:claude

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Canonical `aida doctor` category name.
pub(crate) const CATEGORY: &str = "stranded-commits";

/// Reflog entries read per log (a worktree's `HEAD`, its branch). Enough to
/// cover weeks of an agent session; bounded so the main checkout's long pull
/// history cannot make the check slow.
const REFLOG_ENTRIES: &str = "400";

/// Prefix of a dangling-commit finding id: `dangling:<spec>:<sha>`.
const DANGLING_PREFIX: &str = "dangling:";
/// Prefix of an unpushed-branch finding id: `unpushed:<branch>`.
const UNPUSHED_PREFIX: &str = "unpushed:";

/// One commit read from a reflog.
#[derive(Debug, Clone)]
struct ReflogCommit {
    sha: String,
    time: i64,
    subject: String,
}

/// Run git in `dir`; stdout on success.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = aida_core::git_ops::host_git_command(dir)
        .args(aida_core::git_ops::NO_AUTO_MAINTENANCE)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The spec a commit subject names for authorship (`… (TASK-1)`).
fn subject_spec(subject: &str) -> Option<String> {
    crate::pr_ship::extract_authorship_spec_ids_from_subject(subject)
        .into_iter()
        .next()
        .map(|id| id.to_ascii_uppercase())
}

/// A subject without GitHub's trailing ` (#N)` squash suffix, so a squashed
/// or rebased rewrite compares equal to its original.
fn normalized_subject(subject: &str) -> String {
    let s = subject.trim();
    if let Some(open) = s.rfind(" (#") {
        let tail = &s[open + 3..];
        if let Some(digits) = tail.strip_suffix(')') {
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return s[..open].trim_end().to_string();
            }
        }
    }
    s.to_string()
}

/// `3d`, `5h`, `12m` since `time` (unix seconds).
fn age(time: i64, now: i64) -> String {
    let secs = (now - time).max(0);
    if secs >= 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs >= 3_600 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}m", secs / 60)
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

/// The branch a dangling commit for `spec` is pinned to.
pub(crate) fn recovery_branch(spec: &str) -> String {
    format!("recovered/{}", spec.to_ascii_lowercase())
}

/// Commits from `<dir>`'s reflog for `refname`, newest first.
fn read_reflog(dir: &Path, refname: &str) -> Vec<ReflogCommit> {
    let Some(out) = git(
        dir,
        &[
            "log",
            "-g",
            "-n",
            REFLOG_ENTRIES,
            "--format=%H%x1f%ct%x1f%s",
            refname,
            "--",
        ],
    ) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\u{1f}');
            let sha = parts.next()?.trim().to_string();
            let time = parts.next()?.trim().parse().ok()?;
            let subject = parts.next().unwrap_or("").to_string();
            (sha.len() >= 40).then_some(ReflogCommit { sha, time, subject })
        })
        .collect()
}

/// Branch names that are local-only by design: the store branch, recovery
/// pins, and review snapshots fetched from `refs/pull/N/head`.
fn local_only_by_design(branch: &str) -> bool {
    if branch == "aida-store" || branch.starts_with("recovered/") {
        return true;
    }
    let lower = branch.to_ascii_lowercase();
    ["pr-", "mr-"].iter().any(|p| {
        lower
            .strip_prefix(p)
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    })
}

/// Every stranded-work finding across `project_root`'s worktrees.
/// `is_terminal(spec)` says a spec is closed (Completed, Rejected,
/// Superseded): its local leftovers are not stranded work.
// trace:TASK-1595 | ai:claude
pub(crate) fn scan(
    project_root: &Path,
    is_terminal: &dyn Fn(&str) -> bool,
) -> Vec<crate::DoctorFinding> {
    let worktrees: Vec<(PathBuf, Option<String>)> = crate::status_prs::list_worktrees(project_root)
        .into_iter()
        .filter(|w| !w.clone && w.path.is_dir())
        .map(|w| (w.path, w.branch))
        .collect();
    let now = chrono::Utc::now().timestamp();
    let mut findings = unpushed_branch_findings(project_root, &worktrees, is_terminal, now);
    findings.extend(dangling_commit_findings(
        project_root,
        &worktrees,
        is_terminal,
        now,
    ));
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    findings
}

fn unpushed_branch_findings(
    project_root: &Path,
    worktrees: &[(PathBuf, Option<String>)],
    is_terminal: &dyn Fn(&str) -> bool,
    now: i64,
) -> Vec<crate::DoctorFinding> {
    let default_ref = crate::worktree_gc::detect_default_branch_ref(project_root);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (path, branch) in worktrees {
        let Some(branch) = branch else { continue };
        if local_only_by_design(branch) || !seen.insert(branch.clone()) {
            continue;
        }
        let refname = format!("refs/heads/{branch}");
        let ahead: usize = git(
            path,
            &["rev-list", "--count", &refname, "--not", "--remotes"],
        )
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
        if ahead == 0 {
            continue;
        }
        let Some(tip) = git(path, &["log", "-1", "--format=%H%x1f%ct%x1f%s", &refname]) else {
            continue;
        };
        let mut parts = tip.trim().splitn(3, '\u{1f}');
        let sha = parts.next().unwrap_or("").to_string();
        let time: i64 = parts.next().and_then(|t| t.parse().ok()).unwrap_or(now);
        let subject = parts.next().unwrap_or("");
        let spec = crate::status_prs::work_spec_id_from_branch(branch)
            .or_else(|| subject_spec(subject))
            .map(|s| s.to_ascii_uppercase());
        if spec.as_deref().is_some_and(is_terminal) {
            continue;
        }
        // A squash-merged branch whose remote was deleted after the merge is
        // on no remote, but its content is on the default branch.
        if let Some(default_ref) = default_ref.as_deref() {
            if branch != default_ref.trim_start_matches("origin/")
                && crate::doctor_cmd::branch_content_fully_landed(
                    project_root,
                    default_ref,
                    &refname,
                )
            {
                continue;
            }
        }
        out.push(crate::DoctorFinding {
            category: CATEGORY.to_string(),
            id: format!("{UNPUSHED_PREFIX}{branch}"),
            summary: format!(
                "branch {branch} in {} has {ahead} commit(s) on no remote (spec {}, tip {}, {} old); \
                 they exist on this machine only",
                path.display(),
                spec.as_deref().unwrap_or("unknown"),
                short(&sha),
                age(time, now),
            ),
            action: format!(
                "push it: `git -C {} push -u origin {branch}` (or ship it with `aida pr ship`)",
                path.display()
            ),
            safe_heal: false,
        });
    }
    out
}

fn dangling_commit_findings(
    project_root: &Path,
    worktrees: &[(PathBuf, Option<String>)],
    is_terminal: &dyn Fn(&str) -> bool,
    now: i64,
) -> Vec<crate::DoctorFinding> {
    // sha -> (commit, spec, worktree it was found in); the first worktree wins.
    let mut candidates: HashMap<String, (ReflogCommit, String, PathBuf)> = HashMap::new();
    for (path, branch) in worktrees {
        if branch.as_deref() == Some("aida-store") {
            continue;
        }
        let mut logs = vec![read_reflog(path, "HEAD")];
        if let Some(branch) = branch {
            logs.push(read_reflog(path, &format!("refs/heads/{branch}")));
        }
        for commit in logs.into_iter().flatten() {
            let Some(spec) = subject_spec(&commit.subject) else {
                continue;
            };
            if is_terminal(&spec) {
                continue;
            }
            candidates
                .entry(commit.sha.clone())
                .or_insert_with(|| (commit, spec, path.clone()));
        }
    }
    if candidates.is_empty() {
        return Vec::new();
    }

    // One reachability call: what the candidates reach that no ref does.
    // `--all` covers every branch, tag and remote plus each worktree's HEAD.
    let mut args: Vec<&str> = vec!["rev-list", "--ignore-missing"];
    args.extend(candidates.keys().map(String::as_str));
    args.extend(["--not", "--all"]);
    let Some(unreachable) = git(project_root, &args) else {
        return Vec::new();
    };
    let unreachable: HashSet<&str> = unreachable.lines().map(str::trim).collect();
    let mut dangling: Vec<&(ReflogCommit, String, PathBuf)> = candidates
        .iter()
        .filter(|(sha, _)| unreachable.contains(sha.as_str()))
        .map(|(_, c)| c)
        .collect();
    if dangling.is_empty() {
        return Vec::new();
    }

    // A rebase or amend leaves the original dangling while its rewrite, with
    // the same subject, is on a branch. Compare against reachable subjects
    // since the oldest candidate.
    let oldest = dangling.iter().map(|(c, _, _)| c.time).min().unwrap_or(now);
    let since = format!("--since={}", oldest - 86_400);
    let reachable: HashSet<String> = git(project_root, &["log", "--all", &since, "--format=%s"])
        .unwrap_or_default()
        .lines()
        .map(normalized_subject)
        .collect();
    dangling.retain(|(c, _, _)| !reachable.contains(&normalized_subject(&c.subject)));

    // One finding per (worktree, spec): the newest dangling commit, which
    // pins its dangling ancestors with it.
    let mut groups: HashMap<(PathBuf, String), Vec<&ReflogCommit>> = HashMap::new();
    for (commit, spec, path) in dangling {
        groups
            .entry((path.clone(), spec.clone()))
            .or_default()
            .push(commit);
    }
    groups
        .into_iter()
        .map(|((path, spec), mut commits)| {
            commits.sort_by(|a, b| b.time.cmp(&a.time).then(a.sha.cmp(&b.sha)));
            let newest = commits[0];
            let pin = recovery_branch(&spec);
            crate::DoctorFinding {
                category: CATEGORY.to_string(),
                id: format!("{DANGLING_PREFIX}{spec}:{}", newest.sha),
                summary: format!(
                    "{} dangling commit(s) for {spec} in {}'s reflog are on no branch \
                     (newest {} \"{}\", {} old); git gc will eventually delete them",
                    commits.len(),
                    path.display(),
                    short(&newest.sha),
                    newest.subject,
                    age(newest.time, now),
                ),
                action: format!(
                    "pin it: `git branch {pin} {}` (or `aida doctor --heal --category {CATEGORY}`)",
                    newest.sha
                ),
                safe_heal: true,
            }
        })
        .collect()
}

/// What [`heal`] did: `healed`, the action taken, and why it skipped.
#[derive(Debug)]
pub(crate) struct HealOutcome {
    pub(crate) healed: bool,
    pub(crate) action: String,
    pub(crate) detail: Option<String>,
}

/// Pin a dangling-commit finding to `recovered/<spec>` (or
/// `recovered/<spec>-<short-sha>` when that name is taken). Never moves or
/// deletes an existing ref; an unpushed-branch finding is left to the
/// operator, since pushing publishes.
// trace:TASK-1595 | ai:claude
pub(crate) fn heal(project_root: &Path, finding_id: &str) -> HealOutcome {
    let skipped = |detail: &str| HealOutcome {
        healed: false,
        action: "left in place".to_string(),
        detail: Some(detail.to_string()),
    };
    let healed = |action: String| HealOutcome {
        healed: true,
        action,
        detail: None,
    };
    let Some((spec, sha)) = finding_id
        .strip_prefix(DANGLING_PREFIX)
        .and_then(|rest| rest.rsplit_once(':'))
    else {
        return skipped("not auto-healed: push the branch yourself (pushing publishes)");
    };
    if git(
        project_root,
        &["cat-file", "-e", &format!("{sha}^{{commit}}")],
    )
    .is_none()
    {
        return skipped("the commit is gone from the object store");
    }
    let base = recovery_branch(spec);
    for name in [base.clone(), format!("{base}-{}", short(sha))] {
        let refname = format!("refs/heads/{name}");
        if let Some(existing) = git(
            project_root,
            &["rev-parse", "--verify", "--quiet", &refname],
        ) {
            if existing.trim() == sha {
                return healed(format!("already pinned at {name}"));
            }
            continue;
        }
        // `update-ref <ref> <new> <zero-oid>` creates the ref only if absent.
        let zero = "0".repeat(sha.len());
        if git(project_root, &["update-ref", &refname, sha, &zero]).is_some() {
            return healed(format!("pinned {} to {name}", short(sha)));
        }
    }
    skipped("both recovery branch names are taken; pin it by hand")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_git::git_command;

    fn run(dir: &Path, args: &[&str]) -> String {
        let out = git_command()
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@localhost",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str, subject: &str) -> String {
        std::fs::write(dir.join(file), subject).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-q", "--no-verify", "-m", subject]);
        run(dir, &["rev-parse", "HEAD"])
    }

    /// A clone of a bare remote with one pushed commit on main, plus a linked
    /// worktree on `branch` forked from it.
    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        wt: PathBuf,
    }

    fn fixture(branch: &str) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote.git");
        let root = tmp.path().join("main");
        let out = git_command()
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(&remote)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let out = git_command()
            .args(["clone", "-q"])
            .arg(&remote)
            .arg(&root)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        run(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        commit(&root, "base.txt", "base");
        run(&root, &["push", "-q", "-u", "origin", "main"]);
        let wt = tmp.path().join("wt");
        run(
            &root,
            &["worktree", "add", "-q", "-b", branch, wt.to_str().unwrap()],
        );
        Fixture {
            _tmp: tmp,
            root,
            wt,
        }
    }

    fn open(_: &str) -> bool {
        false
    }

    #[test]
    fn clean_worktrees_produce_no_findings() {
        let f = fixture("task-1");
        commit(&f.wt, "a.txt", "[AI:claude] feat(x): a (TASK-1)");
        run(&f.wt, &["push", "-q", "-u", "origin", "task-1"]);
        assert!(scan(&f.root, &open).is_empty());
    }

    #[test]
    fn unpushed_branch_is_reported_with_its_ahead_count() {
        let f = fixture("task-2");
        commit(&f.wt, "a.txt", "[AI:claude] feat(x): a (TASK-2)");
        commit(&f.wt, "b.txt", "[AI:claude] feat(x): b (TASK-2)");
        let findings = scan(&f.root, &open);
        assert_eq!(findings.len(), 1, "{findings:?}");
        let finding = &findings[0];
        assert_eq!(finding.category, CATEGORY);
        assert_eq!(finding.id, "unpushed:task-2");
        assert!(
            finding.summary.contains("2 commit(s) on no remote"),
            "{}",
            finding.summary
        );
        assert!(finding.summary.contains("TASK-2"), "{}", finding.summary);
        assert!(
            finding.action.contains("push -u origin task-2"),
            "{}",
            finding.action
        );
        assert!(!finding.safe_heal);
        // A closed spec's leftover branch is not stranded work.
        assert!(scan(&f.root, &|s: &str| s == "TASK-2").is_empty());
    }

    #[test]
    fn branch_reset_past_a_spec_commit_reports_the_dangling_sha_and_pins_it() {
        let f = fixture("task-3");
        let lost = commit(
            &f.wt,
            "a.txt",
            "[AI:claude] feat(x): split slice 1 (TASK-3)",
        );
        run(&f.wt, &["reset", "-q", "--hard", "origin/main"]);
        let findings = scan(&f.root, &open);
        assert_eq!(findings.len(), 1, "{findings:?}");
        let finding = &findings[0];
        assert_eq!(finding.id, format!("dangling:TASK-3:{lost}"));
        assert!(finding.summary.contains(&lost[..10]), "{}", finding.summary);
        assert!(
            finding
                .action
                .contains(&format!("git branch recovered/task-3 {lost}")),
            "{}",
            finding.action
        );
        assert!(finding.safe_heal);

        let healed = heal(&f.root, &finding.id);
        assert!(healed.healed, "{healed:?}");
        assert_eq!(run(&f.root, &["rev-parse", "recovered/task-3"]), lost);
        // Pinned, it is reachable again — and the pin is local-only by design.
        assert!(scan(&f.root, &open).is_empty());
        // Healing again is idempotent and never moves the pin.
        assert!(heal(&f.root, &finding.id).healed);
    }

    #[test]
    fn heal_never_overwrites_an_existing_recovery_branch() {
        let f = fixture("task-4");
        let base = run(&f.root, &["rev-parse", "HEAD"]);
        run(&f.root, &["branch", "recovered/task-4", &base]);
        let lost = commit(&f.wt, "a.txt", "[AI:claude] feat(x): a (TASK-4)");
        run(&f.wt, &["reset", "-q", "--hard", "origin/main"]);
        let finding = scan(&f.root, &open)
            .into_iter()
            .find(|f| f.id.starts_with("dangling:"))
            .expect("dangling finding");
        assert!(heal(&f.root, &finding.id).healed);
        assert_eq!(run(&f.root, &["rev-parse", "recovered/task-4"]), base);
        assert_eq!(
            run(
                &f.root,
                &["rev-parse", &format!("recovered/task-4-{}", &lost[..10])]
            ),
            lost
        );
    }

    #[test]
    fn a_rebased_or_amended_original_whose_rewrite_is_on_a_branch_is_not_reported() {
        let f = fixture("task-5");
        commit(&f.wt, "a.txt", "[AI:claude] feat(x): a (TASK-5)");
        run(
            &f.wt,
            &[
                "commit",
                "-q",
                "--amend",
                "--no-verify",
                "--allow-empty",
                "--no-edit",
            ],
        );
        run(&f.wt, &["push", "-q", "-u", "origin", "task-5"]);
        assert!(scan(&f.root, &open).is_empty());
    }

    #[test]
    fn a_squash_merged_branch_whose_remote_was_deleted_is_not_reported() {
        let f = fixture("task-6");
        commit(&f.wt, "a.txt", "[AI:claude] feat(x): a (TASK-6)");
        // Land the same content on main as a squash commit.
        run(&f.root, &["merge", "-q", "--squash", "task-6"]);
        run(
            &f.root,
            &["commit", "-q", "--no-verify", "-m", "squash (TASK-6) (#9)"],
        );
        run(&f.root, &["push", "-q", "origin", "main"]);
        run(&f.root, &["fetch", "-q", "origin"]);
        assert!(
            scan(&f.root, &open).is_empty(),
            "{:?}",
            scan(&f.root, &open)
        );
    }

    #[test]
    fn subjects_normalize_the_squash_suffix() {
        assert_eq!(
            normalized_subject("feat: x (TASK-1) (#42)"),
            "feat: x (TASK-1)"
        );
        assert_eq!(normalized_subject("feat: x (TASK-1)"), "feat: x (TASK-1)");
        assert!(local_only_by_design("pr-12"));
        assert!(local_only_by_design("recovered/task-1"));
        assert!(!local_only_by_design("pr-fix"));
    }
}
