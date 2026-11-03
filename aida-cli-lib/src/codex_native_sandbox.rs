//! TASK-2712: the write set of a native Codex launch.
//!
//! Every Codex launch AIDA makes runs in Codex's own `workspace-write`
//! sandbox with approvals off (`native sandbox, not AIDA-contained`). That
//! sandbox makes the workspace writable and nothing else, and it keeps every
//! `.git` under a writable root read-only. A native Codex lane therefore could
//! not claim its spec (the cache and store live in the main checkout), could
//! not record a review verdict's merge hold (the gate ledger lives there too),
//! and could not commit.
//!
//! Per the advisor ruling on TASK-2712 (N1), a native Codex launch gets the
//! SAME write set a contained Claude lane gets, from the same function
//! ([`crate::session::contained_write_set`]): the store, the cache directory,
//! the mailbox, the deputy inbox, and the build slots or the seat clone's own
//! cargo target. On top of that it gets:
//!
//! - `<main>/.aida/gate-ledger`, so a reviewer can place the merge hold its
//!   verdict requires (the 2026-11-03 "could not protect PR-… with a merge
//!   hold" failure);
//! - `<clone>/.git`, ONLY when the tree is a seat clone the host recorded.
//!   Codex lifts its read-only `.git` rule for an explicit writable root at or
//!   under it, so the seat can commit in its own private repository. The main
//!   `.git`, the common git directory and a linked worktree's git directory
//!   are never granted: a native Codex implementer in a linked worktree
//!   cannot commit, and `aida ship` there refuses with
//!   [`refuse_native_publication_outside_clone`]'s named error.
//!
//! Each grant is one `--add-dir <path>` after the posture flags. Publication
//! from a seat clone goes through the unchanged `aida ship --from-seat` (N2).
//!
//! trace:TASK-2712 | ai:claude

use std::path::{Path, PathBuf};

use anyhow::Result;

/// The native Codex sandbox posture: workspace-write, approvals never.
// trace:TASK-2712 trace:STORY-1124 | ai:claude
pub(crate) const CODEX_NATIVE_POSTURE: [&str; 4] = [
    "--sandbox",
    "workspace-write",
    "--ask-for-approval",
    "never",
];

/// The directories a native Codex lane whose git root is `write_roots[0]`
/// may write beyond its workspace. Empty when there are no roots (outside a
/// git repository: fail closed). A path that does not exist is dropped (the
/// sandbox cannot bind it), as is anything at or under a denied main git
/// directory spelling, whatever produced it.
// trace:TASK-2712 | ai:claude
pub(crate) fn codex_native_write_paths(write_roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let Some(first) = write_roots.first() else {
        return Ok(Vec::new());
    };
    let set = crate::session::contained_write_set(write_roots)?;
    let mut paths = set.grant_paths.clone();
    // Only beside a store grant: a launch that was refused the store (an old
    // git, an unmarkable store) stays without the ledger too.
    if !paths.is_empty() {
        let main = crate::main_worktree_root_from(first);
        paths.extend(crate::contained_boundary::contained_gate_ledger_write_paths(&main)?);
    }
    if let Some(clone) = &set.clone {
        paths.push(clone.root.join(".git"));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for path in paths {
        if set.deny_paths.iter().any(|deny| path.starts_with(deny)) {
            continue;
        }
        if !path.is_dir() || out.contains(&path) {
            continue;
        }
        out.push(path);
    }
    Ok(out)
}

/// The native Codex posture plus one `--add-dir` per
/// [`codex_native_write_paths`] entry, for the lane whose git root is
/// `write_roots[0]`. A path that is not valid UTF-8 is left out (fail closed).
// trace:TASK-2712 | ai:claude
pub(crate) fn codex_native_sandbox_flags_for_roots(write_roots: &[PathBuf]) -> Result<Vec<String>> {
    let mut flags: Vec<String> = CODEX_NATIVE_POSTURE.iter().map(|s| s.to_string()).collect();
    for path in codex_native_write_paths(write_roots)? {
        if let Some(path) = path.to_str() {
            flags.push("--add-dir".to_string());
            flags.push(path.to_string());
        }
    }
    Ok(flags)
}

/// [`codex_native_sandbox_flags_for_roots`] for the lane that runs in `tree`.
// trace:TASK-2712 | ai:claude
pub(crate) fn codex_native_sandbox_flags_in(tree: &Path) -> Result<Vec<String>> {
    codex_native_sandbox_flags_for_roots(&crate::session::contained_session_write_roots_from(tree))
}

/// [`codex_native_sandbox_flags_for_roots`] for a lane launched in this
/// process's working tree (the same resolution a contained Claude launch's
/// settings use). A launcher that learns the lane's tree later rescopes with
/// [`rescope_codex_native_flags`].
// trace:TASK-2712 | ai:claude
pub(crate) fn codex_native_sandbox_flags() -> Result<Vec<String>> {
    codex_native_sandbox_flags_for_roots(&crate::session::contained_session_write_roots())
}

/// Whether `args` carry the native Codex posture.
// trace:TASK-2712 | ai:claude
pub(crate) fn carries_native_posture(args: &[String]) -> bool {
    native_posture_at(args).is_some()
}

fn native_posture_at(args: &[String]) -> Option<usize> {
    args.windows(CODEX_NATIVE_POSTURE.len())
        .position(|w| w.iter().zip(CODEX_NATIVE_POSTURE).all(|(a, b)| a == b))
}

/// Re-derive the write set of a native Codex launch whose tree is now known
/// to be `tree`: the posture and the `--add-dir` run directly after it (what
/// [`codex_native_sandbox_flags_for_roots`] emitted) are replaced with the
/// set for `tree`. Arguments without the native posture are left alone.
// trace:TASK-2712 | ai:claude
pub(crate) fn rescope_codex_native_flags(args: &mut Vec<String>, tree: &Path) -> Result<()> {
    let Some(start) = native_posture_at(args) else {
        return Ok(());
    };
    let mut end = start + CODEX_NATIVE_POSTURE.len();
    while end + 1 < args.len() && args[end] == "--add-dir" {
        end += 2;
    }
    let fresh = codex_native_sandbox_flags_in(tree)?;
    args.splice(start..end, fresh);
    Ok(())
}

/// N1: a native Codex implementer can commit only in a seat clone the host
/// recorded. Anywhere else (a linked worktree, the main checkout) its git
/// directory is read-only by design, so `aida ship` refuses up front with
/// the reason and the way out, instead of failing later on `index.lock`.
/// A launch that is not native Codex (`AIDA_CONTAINMENT_POSTURE` unset or
/// another posture) is never refused here.
// trace:TASK-2712 | ai:claude
pub(crate) fn refuse_native_publication_outside_clone(
    posture: Option<&str>,
    tree: &Path,
) -> Result<()> {
    if posture != Some("native") {
        return Ok(());
    }
    if crate::seat_clone::host_recorded_clone(tree).is_some() {
        return Ok(());
    }
    anyhow::bail!(
        "native Codex sandbox: cannot publish from {}. Codex keeps this tree's git directory \
         read-only, and only a seat clone the host recorded gets its own writable `.git`. \
         Your edits are untouched. Recovery: relaunch the work in a seat clone \
         (`aida agent new codex --spec <ID>`), or have the operator publish this tree \
         from the main checkout",
        tree.display()
    )
}

#[cfg(test)]
#[path = "tests/task_2712_codex_native_sandbox_tests.rs"]
mod tests;
