//! The destructive-entry guard (C SKETCH v1 section 5, STORY-1843 slice C2).
//!
//! Every entry that removes, resets or reuses a checkout retires its mail
//! source here *before* any irreversible or ownership-releasing step:
//!
//! ```text
//! retire_checkout ──Refused──▶ caller keeps tree, lease, claim, pin, manifest
//!        │
//!     Retired (claim held)
//!        │ destructive step
//!        ├─ removed ──▶ Retirement::complete        (Removed → Finalized)
//!        ├─ reset   ──▶ Retirement::complete_reuse  (old generation gone; the
//!        │                                           next writer mints a new one)
//!        └─ failed  ──▶ Retirement::reopen          (bytes copied back, Open)
//! ```
//!
//! Ending policy for a source left `Retired`: the caller either completes or
//! reopens it. A caller that dies in between leaves it *abandoned*; its claim
//! is free, so the next admission reopens it and the next retirement adopts
//! the committed operation. Dropping a [`Retirement`] without settling it is
//! that same abandoned ending, never a silent completion.
//!
//! A checkout with no `.aida/` of its own, or whose `.aida/` or mailbox is a
//! symlink into another checkout, owns no source: there is nothing to retire.
//!
//! Sources with no anchor (no verifiable Git, an orphaned worktree whose
//! admin directory is gone, a seat clone without a verified host binding)
//! cannot be fenced, so they refuse destruction and the tree is kept, even
//! when the mailbox looks empty: without a fence a writer can still land
//! bytes between the look and the removal (C SKETCH section 5).
//!
//! trace:STORY-1843 | ai:claude

use super::retire::{
    self, RefusalReason, RetireOptions, RetireOutcome, RetirementClaim, RetirementTicket,
};
use super::LifecycleError;
use std::path::{Path, PathBuf};

/// A retired source awaiting its caller's destructive step. Holds the
/// per-source retirement claim, so no other driver adopts the operation and
/// no admission reopens it until this settles or drops.
#[derive(Debug)]
#[must_use = "complete, complete_reuse or reopen the retirement once the destructive step ran"]
pub struct Retirement {
    ticket: RetirementTicket,
    generation: u64,
    retained_dir: PathBuf,
    mailbox: PathBuf,
    _claim: Option<RetirementClaim>,
}

impl Retirement {
    pub fn op_id(&self) -> &str {
        &self.ticket.op_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Where the preserved bytes are kept.
    pub fn retained_dir(&self) -> &Path {
        &self.retained_dir
    }

    /// The checkout is gone: record `Removed`, then forget the source.
    pub fn complete(self) -> std::io::Result<()> {
        retire::complete_retirement(&self.ticket).map_err(Into::into)
    }

    /// The checkout was reset for its next occupant and still exists. Its
    /// emptied mailbox directory is removed, so the next writer creates a new
    /// one (a new key and generation) and never sees this occupant's mail.
    /// Anything found in it was written after the commit by a writer that
    /// took no permit: the reuse fails and the source is reopened with it.
    pub fn complete_reuse(self) -> std::io::Result<()> {
        match remove_empty_tree(&self.mailbox) {
            Ok(()) => self.complete(),
            Err(e) => {
                let late = std::io::Error::other(format!(
                    "the retired mailbox {} is not empty after the reset: {e}",
                    self.mailbox.display()
                ));
                self.reopen()?;
                Err(late)
            }
        }
    }

    /// The destructive step did not happen: copy the preserved bytes back
    /// where absent (the retained copy is kept) and reopen the source under a
    /// new generation.
    pub fn reopen(self) -> std::io::Result<()> {
        retire::reopen_retirement(&self.ticket).map_err(Into::into)
    }
}

/// Why a destructive entry kept its tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionRefusal {
    pub checkout: PathBuf,
    pub reason: String,
    /// The source holds mail and no activation is recorded for this build
    /// (QC1); the reason names [`super::ACTIVATION_COMMAND`].
    pub needs_activation: bool,
    /// A preserved copy that is also kept.
    pub retained_dir: Option<PathBuf>,
}

impl std::fmt::Display for RetentionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "mail retention refused for {}: {}; the tree was kept",
            self.checkout.display(),
            self.reason
        )
    }
}

impl std::error::Error for RetentionRefusal {}

pub type Gate = std::result::Result<Option<Retirement>, RetentionRefusal>;

/// Retire the mail source of the checkout at `checkout` before a destructive
/// step. `Ok(None)` when there is nothing to retain: the checkout is already
/// gone or owns no mailbox (no `.aida/`, or one linked to another checkout).
/// A checkout with no anchor refuses, whatever its mailbox holds, and every
/// failure to run the barrier refuses (fail closed).
pub fn retire_checkout(checkout: &Path, reason: &str) -> Gate {
    retire_with(checkout, reason, |opts| {
        retire::retire_source_claimed(checkout, opts)
    })
}

/// [`retire_checkout`] for a seat clone. `main` is the checkout the caller
/// resolved through the host-recorded clone binding; `None` when the binding
/// is missing, unverified or stale, and then a clone that has a `.aida/` of
/// its own refuses (it cannot be fenced).
pub fn retire_clone_checkout(main: Option<&Path>, clone_root: &Path, reason: &str) -> Gate {
    match main {
        Some(main) => retire_with(clone_root, reason, |opts| {
            retire::retire_clone_claimed(main, clone_root, opts)
        }),
        None if !owns_a_mailbox(clone_root) => Ok(None),
        None => Err(unanchored(
            clone_root,
            "the seat clone has no verified host binding to its main checkout",
        )),
    }
}

fn retire_with(
    checkout: &Path,
    reason: &str,
    run: impl FnOnce(&RetireOptions) -> super::Result<(RetireOutcome, Option<RetirementClaim>)>,
) -> Gate {
    if !owns_a_mailbox(checkout) {
        return Ok(None);
    }
    let opts = RetireOptions {
        reason: reason.to_string(),
        ..RetireOptions::default()
    };
    let refusal = |reason: String, needs_activation, retained_dir| RetentionRefusal {
        checkout: checkout.to_path_buf(),
        reason,
        needs_activation,
        retained_dir,
    };
    match run(&opts) {
        Ok((
            RetireOutcome::Retired {
                ticket,
                generation,
                retained_dir,
            },
            claim,
        )) => Ok(Some(Retirement {
            ticket,
            generation,
            retained_dir,
            mailbox: crate::mailbox::local_mailbox_dir(checkout),
            _claim: claim,
        })),
        Ok((
            RetireOutcome::Refused {
                reason: RefusalReason::Unanchored,
                ..
            },
            _,
        )) => Err(unanchored(checkout, &RefusalReason::Unanchored.to_string())),
        Ok((
            RetireOutcome::Refused {
                reason,
                retained_dir,
            },
            _,
        )) => Err(refusal(
            reason.to_string(),
            matches!(
                reason,
                RefusalReason::MailBearingBeforeActivation
                    | RefusalReason::ActivationForOtherBuild { .. }
            ),
            retained_dir,
        )),
        Err(e) => Err(refusal(lifecycle_failure(&e), false, None)),
    }
}

/// Does `checkout` hold a mailbox of its own? Not when the checkout is gone,
/// when it has no `.aida/` at all, or when its `.aida/` (or the mailbox) is a
/// symlink into another checkout: that mailbox is the other checkout's
/// source, and retiring it here would move another source's mail.
fn owns_a_mailbox(checkout: &Path) -> bool {
    let aida = checkout.join(".aida");
    match std::fs::symlink_metadata(&aida) {
        Ok(meta) if meta.file_type().is_dir() => {}
        _ => return false,
    }
    match std::fs::symlink_metadata(aida.join("mailbox")) {
        Ok(meta) => !meta.file_type().is_symlink(),
        // Absent: still fenced, since a writer could create it meanwhile.
        Err(_) => true,
    }
}

/// No anchor to fence in: refuse, whatever the mailbox holds. Destroying an
/// unfenced source because it looks empty is a check-then-act race with any
/// writer that lands bytes in between.
// trace:STORY-1843 | ai:claude
fn unanchored(checkout: &Path, why: &str) -> RetentionRefusal {
    RetentionRefusal {
        checkout: checkout.to_path_buf(),
        reason: format!("{why}, so its mail cannot be fenced and retained"),
        needs_activation: false,
        retained_dir: None,
    }
}

fn lifecycle_failure(e: &LifecycleError) -> String {
    format!("the retention barrier could not run ({e}); retry")
}

/// Remove `dir` and its subdirectories, refusing if any entry is not a
/// directory. Absent is fine.
fn remove_empty_tree(dir: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if std::fs::symlink_metadata(&path)?.file_type().is_dir() {
            remove_empty_tree(&path)?;
        } else {
            return Err(std::io::Error::other(format!(
                "{} was written after the retirement",
                path.display()
            )));
        }
    }
    std::fs::remove_dir(dir)
}

/// Remove the linked worktree at `worktree_path` behind the barrier: retire,
/// `git worktree remove`, then complete, or reopen when the removal failed.
/// A refusal fails the removal and keeps the tree.
pub fn remove_worktree_retained(
    repo_root: &Path,
    worktree_path: &Path,
    force: bool,
    reason: &str,
) -> anyhow::Result<()> {
    let retirement = retire_checkout(worktree_path, reason)?;
    remove_worktree_with(repo_root, worktree_path, force, retirement)
}

/// The removal half of [`remove_worktree_retained`], for a caller that
/// already retired the source (session end retires before it deletes the
/// lease).
pub fn remove_worktree_with(
    repo_root: &Path,
    worktree_path: &Path,
    force: bool,
    retirement: Option<Retirement>,
) -> anyhow::Result<()> {
    remove_retired(
        worktree_path,
        retirement,
        || crate::git_ops::remove_worktree_unguarded(repo_root, worktree_path, force),
        Retirement::complete,
    )
}

/// Run the destructive `step` behind `retirement`, then settle it. The
/// caller's salvage and hooks ran since the commit, so the retired mailbox is
/// re-verified immediately before the step: new bytes, or a permit admitted
/// since, reopen the source and refuse with a [`RetentionRefusal`] (the step
/// never runs, the tree is kept). Binding 63 #1: a hook's writes are caught
/// by this check, never trusted.
// trace:STORY-1843 | ai:claude
pub fn remove_retired<T>(
    checkout: &Path,
    retirement: Option<Retirement>,
    step: impl FnOnce() -> anyhow::Result<T>,
    on_success: impl FnOnce(Retirement) -> std::io::Result<()>,
) -> anyhow::Result<T> {
    let Some(retirement) = retirement else {
        return step();
    };
    let why = match retire::verify_quiet(&retirement.ticket) {
        Ok(Ok(())) => return settle(step(), Some(retirement), on_success),
        Ok(Err(reason)) => reason.to_string(),
        Err(e) => lifecycle_failure(&e),
    };
    let refusal = RetentionRefusal {
        checkout: checkout.to_path_buf(),
        reason: why,
        needs_activation: false,
        retained_dir: Some(retirement.retained_dir.clone()),
    };
    match retirement.reopen() {
        Ok(()) => Err(refusal.into()),
        Err(reopen) => Err(anyhow::Error::new(refusal).context(format!(
            "and the retired mailbox could not be reopened ({reopen}); the next mail write \
             reopens it"
        ))),
    }
}

/// Settle `retirement` after a destructive step: `on_success` when it ran
/// (complete, or complete_reuse), reopen when it failed. The step's own error
/// wins over a reopen failure, which is appended.
pub fn settle<T>(
    step: anyhow::Result<T>,
    retirement: Option<Retirement>,
    on_success: impl FnOnce(Retirement) -> std::io::Result<()>,
) -> anyhow::Result<T> {
    let Some(retirement) = retirement else {
        return step;
    };
    match step {
        Ok(value) => {
            on_success(retirement).map_err(|e| {
                anyhow::anyhow!("the step ran, but its mail retirement could not be finished: {e}")
            })?;
            Ok(value)
        }
        Err(error) => match retirement.reopen() {
            Ok(()) => Err(error),
            Err(reopen) => Err(error.context(format!(
                "and the retired mailbox could not be reopened ({reopen}); the next mail \
                 write reopens it"
            ))),
        },
    }
}
