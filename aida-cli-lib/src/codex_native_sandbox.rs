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
//! - `<main>/.aida/gate-ledger`, ONLY for the reviewer role, so a reviewer
//!   can place the merge hold its verdict requires (the 2026-11-03 "could not
//!   protect PR-… with a merge hold" failure). An implementer has no reason to
//!   write verdict or hold events and does not get it (advisor ruling
//!   2026-11-03 18:32). A launch whose role is not known does not get it
//!   either. A contained Claude lane never gets it, so there is no Claude
//!   drift to remove;
//! - `<clone>/.git`, ONLY when the tree is a seat clone the host recorded.
//!   The main `.git`, the common git directory and a linked worktree's git
//!   directory are never granted.
//!
//! Each grant is one `--add-dir <path>` after the posture flags.
//!
//! Codex 0.166 re-protects `<clone>/.git` at run time despite that grant (the
//! N3 host probe, 2026-11-03 18:45), so a native lane still cannot run `git
//! add` or `git commit`. AIDA does not try to defeat that protection. Per the
//! advisor's fallback ruling, the HOST commits a native seat's pending work:
//! `aida ship --from-seat <clone> --commit-pending <MSG>` runs `git add` and
//! `git commit` for the clone inside AIDA's own bwrap job, with the clone
//! writable and hooks off ([`crate::seat_clone::commit_pending`]), then
//! publishes through the unchanged `ship --from-seat` path (N2). `aida ship`
//! inside a native lane refuses up front with [`refuse_native_publication`]'s
//! named error and that recovery.
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

/// The one role whose native Codex launch gets the gate ledger.
// trace:TASK-2712 | ai:claude
pub(crate) const GATE_LEDGER_ROLE: &str = "reviewer";

/// The directories a native Codex lane in `role` whose git root is
/// `write_roots[0]` may write beyond its workspace. Empty when there are no
/// roots (outside a git repository: fail closed). The gate ledger is added
/// only for [`GATE_LEDGER_ROLE`]. A path that does not exist is dropped (the
/// sandbox cannot bind it), as is anything at or under a denied main git
/// directory spelling, whatever produced it.
// trace:TASK-2712 | ai:claude
pub(crate) fn codex_native_write_paths(
    write_roots: &[PathBuf],
    role: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let Some(first) = write_roots.first() else {
        return Ok(Vec::new());
    };
    let set = crate::session::contained_write_set(write_roots)?;
    let mut paths = set.grant_paths.clone();
    // Only for a reviewer, and only beside a store grant: a launch that was
    // refused the store (an old git, an unmarkable store) stays without the
    // ledger too.
    if role == Some(GATE_LEDGER_ROLE) && !paths.is_empty() {
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
/// [`codex_native_write_paths`] entry, for the lane in `role` whose git root
/// is `write_roots[0]`. A path that is not valid UTF-8 is left out (fail
/// closed).
// trace:TASK-2712 | ai:claude
pub(crate) fn codex_native_sandbox_flags_for_roots(
    write_roots: &[PathBuf],
    role: Option<&str>,
) -> Result<Vec<String>> {
    let mut flags: Vec<String> = CODEX_NATIVE_POSTURE.iter().map(|s| s.to_string()).collect();
    for path in codex_native_write_paths(write_roots, role)? {
        if let Some(path) = path.to_str() {
            flags.push("--add-dir".to_string());
            flags.push(path.to_string());
        }
    }
    Ok(flags)
}

/// [`codex_native_sandbox_flags_for_roots`] for the lane in `role` that runs
/// in `tree`.
// trace:TASK-2712 | ai:claude
pub(crate) fn codex_native_sandbox_flags_in(
    tree: &Path,
    role: Option<&str>,
) -> Result<Vec<String>> {
    codex_native_sandbox_flags_for_roots(
        &crate::session::contained_session_write_roots_from(tree),
        role,
    )
}

/// [`codex_native_sandbox_flags_for_roots`] for a lane launched in this
/// process's working tree (the same resolution a contained Claude launch's
/// settings use). The role is not known yet, so there is no gate ledger: a
/// launcher that learns the lane's tree and role later rescopes with
/// [`rescope_codex_native_flags`].
// trace:TASK-2712 | ai:claude
pub(crate) fn codex_native_sandbox_flags() -> Result<Vec<String>> {
    codex_native_sandbox_flags_for_roots(&crate::session::contained_session_write_roots(), None)
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

/// Re-derive the write set of a native Codex launch whose tree and role are
/// now known to be `tree` and `role`: the posture and the `--add-dir` run
/// directly after it (what [`codex_native_sandbox_flags_for_roots`] emitted)
/// are replaced with the set for `tree` and `role`. Arguments without the
/// native posture are left alone.
// trace:TASK-2712 | ai:claude
pub(crate) fn rescope_codex_native_flags(
    args: &mut Vec<String>,
    tree: &Path,
    role: Option<&str>,
) -> Result<()> {
    let Some(start) = native_posture_at(args) else {
        return Ok(());
    };
    let mut end = start + CODEX_NATIVE_POSTURE.len();
    while end + 1 < args.len() && args[end] == "--add-dir" {
        end += 2;
    }
    let fresh = codex_native_sandbox_flags_in(tree, role)?;
    args.splice(start..end, fresh);
    Ok(())
}

/// A native Codex lane cannot commit anywhere: Codex keeps every `.git`
/// read-only at run time, a seat clone's included. So `aida ship` run by a
/// native lane (`AIDA_CONTAINMENT_POSTURE=native`) refuses up front with the
/// reason and the way out, instead of failing later on `index.lock`. In a
/// seat clone the way out is the host commit and publication
/// (`aida ship --from-seat <clone> --commit-pending <MSG>` from the main
/// checkout); anywhere else it is a relaunch in a seat clone. A launch that
/// is not native Codex (the posture unset or another posture) is never
/// refused here.
// trace:TASK-2712 | ai:claude
pub(crate) fn refuse_native_publication(posture: Option<&str>, tree: &Path) -> Result<()> {
    if posture != Some("native") {
        return Ok(());
    }
    if let Some(clone) = aida_core::seat_clone_detect::seat_clone_root(tree) {
        anyhow::bail!(
            "native Codex sandbox: cannot commit or publish from {clone}. Codex keeps this \
             seat's git directory read-only. Your edits are untouched: leave them uncommitted \
             and hand off, so the host commits them (hooks off) and publishes. From the main \
             checkout: `aida ship <SPEC> --from-seat {clone} --commit-pending \
             '<type(scope): summary> (<SPEC>)'`",
            clone = clone.display()
        );
    }
    anyhow::bail!(
        "native Codex sandbox: cannot publish from {}. Codex keeps this tree's git directory \
         read-only, and only a seat clone's pending work can be committed by the host. Your \
         edits are untouched. Recovery: relaunch the work in a seat clone \
         (`aida agent new codex --spec <ID>`), or have the operator publish this tree from the \
         main checkout",
        tree.display()
    )
}

#[cfg(test)]
#[path = "tests/task_2712_codex_native_sandbox_tests.rs"]
mod tests;
