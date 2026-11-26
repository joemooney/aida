//! Commits in `base..head` whose change is already on `base` by patch-id: a
//! promotion's integration history whose squashes landed on main, even with
//! main merged into the branch (which empties `--cherry-pick`'s left side).
// trace:BUG-2733 | ai:claude

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{ensure, Context, Result};

/// The most base-side commits whose patch-ids are compared.
const BASE_WINDOW: usize = 2000;
/// Each git call's ceiling; a train check's scoped deadline tightens it.
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// `git log -p` output ceiling (binary hunks included); past it the read
/// fails and every commit is kept.
const PATCH_CAP: u64 = 64 * 1024 * 1024;
/// Ceiling for listings (`rev-list`, `patch-id` output).
const LIST_CAP: u64 = 4 * 1024 * 1024;

/// Diff options for patch-id input: no external diff, textconv, signature
/// or rename driver runs, and binary content is hashed in full so two
/// different binary changes never share a patch-id. `--no-prefix` makes a
/// reversed diff hash like the forward one.
// trace:BUG-2733 | ai:claude
const PATCH_FLAGS: &[&str] = &[
    "--no-merges",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--no-renames",
    "--no-relative",
    "--full-index",
    "--binary",
    "--no-prefix",
    "--no-show-signature",
    "--diff-algorithm=myers",
];

/// Full SHAs of the non-merge commits in `base..head` whose patch-id matches a
/// non-merge commit on `base` made since the branch forked. Empty when the
/// fork point cannot be told (no evidence ⇒ nothing is dropped). A patch-id
/// that a later base commit reverses (a revert) is not on base: the branch's
/// copy would re-introduce it, so its commit is kept.
// trace:BUG-2733 | ai:claude
pub(crate) fn already_on_base(repo: &Path, base: &str, head: &str) -> Result<HashSet<String>> {
    let range = format!("{base}..{head}");
    let listing = git_stdout(
        repo,
        &[
            "rev-list",
            "--parents",
            crate::git_arg_guard::END_OF_OPTIONS,
            &range,
            "--",
        ],
    )?;
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
    let mut base_revs = vec![base];
    base_revs.extend(stops.iter().map(String::as_str));
    let base_ids = |reverse| patch_ids(repo, Some(&window), &base_revs, reverse);
    let mut on_base: HashSet<String> = base_ids(false)?.into_keys().collect();
    // When in doubt, keep the commit: the supervised gate then applies.
    for reverted in base_ids(true)?.into_keys() {
        on_base.remove(&reverted);
    }
    Ok(patch_ids(repo, None, &[&range], false)?
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

/// `git <args>` in `repo` through the host git hardening (seat-clone
/// refusal, contained-tree pins, fsmonitor off), bounded by [`CALL_TIMEOUT`]
/// and `cap` output bytes.
// trace:BUG-2733 | ai:claude
fn bounded_git(repo: &Path, args: &[&str], stdin: Stdio, cap: u64) -> Result<Vec<u8>> {
    let mut cmd = aida_core::git_ops::host_git_command(repo);
    cmd.args(args).stdin(stdin);
    use crate::bounded_process::{command_output_with_timeout_capped, BoundedCommandOutput};
    let out = match command_output_with_timeout_capped(cmd, CALL_TIMEOUT, cap) {
        BoundedCommandOutput::Completed(out) => out,
        BoundedCommandOutput::TimedOut => anyhow::bail!("git {} timed out", args[0]),
        BoundedCommandOutput::SpawnFailed => anyhow::bail!("could not start git {}", args[0]),
    };
    ensure!(
        out.stdout.len() as u64 <= cap && out.stderr.len() as u64 <= cap,
        "git {} exceeded the output limit",
        args[0]
    );
    ensure!(
        out.status.success(),
        "git {} failed: {}",
        args[0],
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(out.stdout)
}

fn git_stdout(repo: &Path, args: &[&str]) -> Result<String> {
    let out = bounded_git(repo, args, Stdio::null(), LIST_CAP)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// patch-id → the non-merge commits in `revs` that carry it; `reverse`
/// hashes each commit's inverse (what a revert of it would carry).
// trace:BUG-2733 | ai:claude
fn patch_ids(
    repo: &Path,
    window: Option<&str>,
    revs: &[&str],
    reverse: bool,
) -> Result<HashMap<String, Vec<String>>> {
    let mut args = vec!["log", "-p", "--format=commit %H"];
    args.extend(window);
    args.extend_from_slice(PATCH_FLAGS);
    if reverse {
        args.push("-R");
    }
    args.push(crate::git_arg_guard::END_OF_OPTIONS);
    args.extend_from_slice(revs);
    args.push("--");
    let log = bounded_git(repo, &args, Stdio::null(), PATCH_CAP).context("reading git log -p")?;
    // Fed through a pipe so `patch-id` runs as its own bounded child; a
    // killed reader ends the feeder with EPIPE.
    let (reader, mut writer) = std::io::pipe().context("piping git log -p")?;
    let feeder = std::thread::spawn(move || {
        let _ = std::io::copy(&mut log.as_slice(), &mut writer);
    });
    let pid = bounded_git(
        repo,
        &["patch-id", "--stable"],
        Stdio::from(reader),
        LIST_CAP,
    );
    let _ = feeder.join();
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for line in String::from_utf8_lossy(&pid?).lines() {
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
