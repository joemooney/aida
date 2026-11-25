//! Commits in `base..head` whose change is already on `base` by patch-id: a
//! promotion's integration history whose squashes landed on main, even with
//! main merged into the branch (which empties `--cherry-pick`'s left side).
// trace:BUG-2733 | ai:claude

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{ensure, Context, Result};

/// The most base-side commits whose patch-ids are compared.
const BASE_WINDOW: usize = 2000;

/// Full SHAs of the non-merge commits in `base..head` whose patch-id matches a
/// non-merge commit on `base` made since the branch forked. Empty when the
/// fork point cannot be told (no evidence ⇒ nothing is dropped).
// trace:BUG-2733 | ai:claude
pub(crate) fn already_on_base(repo: &Path, base: &str, head: &str) -> Result<HashSet<String>> {
    let range = format!("{base}..{head}");
    let listing = git_stdout(repo, &["rev-list", "--parents", &range, "--"])?;
    let rows: Vec<Vec<&str>> = listing
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .filter(|r| !r.is_empty())
        .collect();
    let branch: HashSet<&str> = rows.iter().map(|r| r[0]).collect();
    // A merge's parents say nothing about where the work forked: a branch
    // that merged main has main's tip as a parent. Only single-parent commits
    // whose parent is outside the range mark the fork point(s) on base.
    let stops: BTreeSet<String> = rows
        .iter()
        .filter(|r| r.len() == 2 && !branch.contains(r[1]))
        .map(|r| format!("^{}", r[1]))
        .collect();
    if stops.is_empty() {
        return Ok(HashSet::new());
    }
    let window = format!("--max-count={BASE_WINDOW}");
    let mut base_args = vec![window.as_str(), base];
    base_args.extend(stops.iter().map(String::as_str));
    let on_base: HashSet<String> = patch_ids(repo, &base_args)?.into_keys().collect();
    Ok(patch_ids(repo, &[&range])?
        .into_iter()
        .filter(|(pid, _)| on_base.contains(pid))
        .flat_map(|(_, shas)| shas)
        .collect())
}

/// Drop from `items` the commits [`already_on_base`]; every item is kept when
/// the patch-id read fails, so an unreadable history never hides a spec.
// trace:BUG-2733 | ai:claude
pub(crate) fn retain_new<T>(
    repo: &Path,
    base: &str,
    head: &str,
    items: &mut Vec<T>,
    sha_of: impl Fn(&T) -> &str,
) {
    let Ok(landed) = already_on_base(repo, base, head) else {
        return;
    };
    items.retain(|item| {
        let sha = sha_of(item);
        // `%h` abbreviations are prefixes of the full sha.
        sha.is_empty() || !landed.iter().any(|full| full.starts_with(sha))
    });
}

fn git_stdout(repo: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).args(args);
    let out = crate::forge_lookup::deadline_output(cmd, Command::output)
        .with_context(|| format!("running git {}", args.join(" ")))?;
    ensure!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// patch-id → the non-merge commits in `revs` that carry it.
fn patch_ids(repo: &Path, revs: &[&str]) -> Result<HashMap<String, Vec<String>>> {
    let mut log = Command::new("git");
    log.arg("-C")
        .arg(repo)
        .args([
            "log",
            "-p",
            "--no-merges",
            "--no-color",
            "--no-ext-diff",
            "--format=commit %H",
        ])
        .args(revs)
        .arg("--")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut log = log.spawn().context("running git log -p")?;
    let stdin = log.stdout.take().context("git log -p has no stdout")?;
    let pid = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["patch-id", "--stable"])
        .stdin(Stdio::from(stdin))
        .output()
        .context("running git patch-id")?;
    let status = log.wait().context("waiting for git log -p")?;
    ensure!(status.success(), "git log -p {} failed", revs.join(" "));
    ensure!(pid.status.success(), "git patch-id failed");
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for line in String::from_utf8_lossy(&pid.stdout).lines() {
        if let Some((pid, sha)) = line.split_once(' ') {
            out.entry(pid.to_owned())
                .or_default()
                .push(sha.trim().to_owned());
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "tests/bug_2733_base_patch_equivalents_tests.rs"]
mod tests;
