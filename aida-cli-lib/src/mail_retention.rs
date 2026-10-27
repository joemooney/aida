//! CLI side of the mail-retention guard (STORY-1843 C2): retire a session's
//! tree before it is removed, reset or handed on, and the recovery-ownership
//! ending when retirement refuses.
//!
//! A refusal keeps the tree, the lease, the claim, the pin and the manifest.
//! Security still proceeds: the lease's child grants are revoked, and the
//! lease is stamped *ending-retained* (`ending_retained_at`), so it reads as
//! neither active work nor cleaned up. The next end, reap or heal retries.
//!
//! trace:STORY-1843 | ai:claude

use aida_core::mailbox_lifecycle::guard::{self, Gate, RetentionRefusal, Retirement};
use std::path::Path;

/// Retire the mail source of `worktree`, a seat clone when `is_clone`. A
/// clone's retention home is its main checkout's anchor, resolved only
/// through the verified host-recorded binding; without one, a clone that
/// owns a mailbox refuses, mail or not (it cannot be fenced), and only a
/// clone with no `.aida/` of its own may go.
pub(crate) fn retire_tree(
    project_root: &Path,
    worktree: &Path,
    is_clone: bool,
    reason: &str,
) -> Gate {
    if is_clone {
        let main = verified_clone_main(project_root, worktree);
        guard::retire_clone_checkout(main.as_deref(), worktree, reason)
    } else {
        guard::retire_checkout(worktree, reason)
    }
}

/// `main` as the verified host binding of the seat clone at `clone`, or
/// `None` when the binding is missing, unverified, stale, or names another
/// main.
pub(crate) fn verified_clone_main(main: &Path, clone: &Path) -> Option<std::path::PathBuf> {
    aida_core::seat_clone_detect::note_host_main(main);
    let recorded = crate::seat_clone::host_recorded_clone(clone)?;
    let main = main.canonicalize().ok()?;
    (recorded.main == main).then_some(main)
}

/// Remove the linked worktree at `worktree` behind the barrier, for the CLI
/// entries that used to run a raw `git worktree remove`.
pub(crate) fn remove_worktree(
    project_root: &Path,
    worktree: &Path,
    force: bool,
    reason: &str,
) -> anyhow::Result<()> {
    guard::remove_worktree_retained(project_root, worktree, force, reason)
}

/// Settle a retirement after a step that did not destroy the tree (it was
/// kept for another reason): reopen it. Best-effort; a failure is reported
/// and the next admission reopens the abandoned source.
pub(crate) fn reopen_quietly(retirement: Option<Retirement>) {
    if let Some(retirement) = retirement {
        if let Err(e) = retirement.reopen() {
            eprintln!("warning: the retired mailbox could not be reopened: {e}");
        }
    }
}

/// The recovery-ownership ending for a refused retirement: revoke the lease's
/// child grants and stamp it ending-retained. The lease, claim and manifest
/// stay.
pub(crate) fn keep_ending_retained(lease_file: &Path, refusal: &RetentionRefusal) {
    crate::session::revoke_lease_child_grant(lease_file);
    if let Err(e) = stamp_ending_retained(lease_file, &refusal.reason) {
        eprintln!(
            "warning: could not mark lease {} ending-retained: {e:#}",
            lease_file.display()
        );
    }
}

/// Patch `ending_retained_at` / `ending_retained_reason` into the lease as
/// generic TOML keys, so keys this binary does not model survive.
fn stamp_ending_retained(lease_file: &Path, reason: &str) -> anyhow::Result<()> {
    let body = std::fs::read_to_string(lease_file)?;
    let mut table: toml::Table = body.parse()?;
    table.insert(
        "ending_retained_at".into(),
        toml::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    table.insert(
        "ending_retained_reason".into(),
        toml::Value::String(reason.to_string()),
    );
    aida_core::write_atomic(lease_file, toml::to_string_pretty(&table)?.as_bytes())?;
    Ok(())
}

/// The operator-facing lines for a refusal, naming the activation command
/// when that is what is missing.
pub(crate) fn refusal_lines(refusal: &RetentionRefusal) -> Vec<String> {
    let mut lines = vec![format!("{refusal}")];
    if refusal.needs_activation {
        lines.push(format!(
            "check whether every mail writer on this machine takes a write permit: `{}`",
            aida_core::mailbox_lifecycle::ACTIVATION_COMMAND
        ));
    }
    if let Some(dir) = &refusal.retained_dir {
        lines.push(format!("a preserved copy is kept at {}", dir.display()));
    }
    lines
}

#[cfg(test)]
thread_local! {
    static BEFORE_GUARDED_REMOVAL: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Fixture seam: run `hook` once, on this thread, right before the next
/// guarded removal of a retired tree (a late writer landing mail after the
/// retirement committed).
#[cfg(test)]
pub(crate) fn on_before_guarded_removal(hook: impl FnOnce() + 'static) {
    BEFORE_GUARDED_REMOVAL.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
}

/// Called by the session-ending entries between retirement and their
/// guarded removal. Production builds pass through.
pub(crate) fn before_guarded_removal() {
    #[cfg(test)]
    if let Some(hook) = BEFORE_GUARDED_REMOVAL.with(|h| h.borrow_mut().take()) {
        hook();
    }
}

/// The [`RetentionRefusal`] behind `error`, when the guarded step refused:
/// the final quiet check found mail written after the retirement committed.
pub(crate) fn retention_refusal(error: &anyhow::Error) -> Option<&RetentionRefusal> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<RetentionRefusal>())
}

/// The guarded command an operator runs to remove a tree an entry left in
/// place. Never a raw `git worktree remove`: that runs no fence and would
/// delete mail reopened into the tree (R3-F2).
pub(crate) fn guarded_removal_command(tree: &Path) -> String {
    format!("aida worktree dismiss {}", tree.display())
}

/// The operator-facing lines for a refusal that came after the retirement
/// committed (R3-F1): the tree, lease, claim and manifest were all kept, so
/// the session did not end. `retry` names what re-runs the ending.
// trace:STORY-1843 | ai:claude
pub(crate) fn late_refusal_lines(refusal: &RetentionRefusal, retry: &str) -> Vec<String> {
    let mut lines = refusal_lines(refusal);
    lines.push(String::new());
    lines.push(format!(
        "The session was NOT ended: mail reached {} after its mail was retired, and was put \
         back. Its lease is kept (marked ending-retained, its grant revoked) with its claim \
         and manifest, and the tree was not removed or reset. Read or move that mail, then \
         retry `{retry}`.",
        refusal.checkout.display()
    ));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(needs_activation: bool) -> RetentionRefusal {
        RetentionRefusal {
            checkout: "/w".into(),
            reason: "the mailbox holds mail".into(),
            needs_activation,
            retained_dir: Some("/anchor/retained/k/3".into()),
        }
    }

    #[test]
    fn a_refused_end_stamps_the_lease_ending_retained_and_keeps_its_keys() {
        let dir = tempfile::tempdir().unwrap();
        let lease = dir.path().join("s.toml");
        std::fs::write(&lease, "id = \"s\"\nunmodelled_key = \"kept\"\n").unwrap();
        keep_ending_retained(&lease, &refusal(true));
        let table: toml::Table = std::fs::read_to_string(&lease).unwrap().parse().unwrap();
        assert_eq!(table["id"].as_str(), Some("s"), "the lease is kept");
        assert_eq!(table["unmodelled_key"].as_str(), Some("kept"));
        assert!(table["ending_retained_at"].as_str().is_some());
        assert_eq!(
            table["ending_retained_reason"].as_str(),
            Some("the mailbox holds mail")
        );
    }

    fn lease_toml(extra: &str) -> String {
        format!(
            "id = \"s1\"\nscope = \"TASK-1\"\nslug = \"task-1\"\nowner = \"o\"\n\
             worktree_path = \"/w\"\nbranch = \"b\"\n\
             started_at = \"2026-10-27T00:00:00Z\"\nhostname = \"h\"\n{extra}"
        )
    }

    #[test]
    fn the_stamp_is_read_by_the_typed_lease_and_survives_a_typed_rewrite() {
        // F2: the stamp written as generic keys is modelled, so ps, leases,
        // reap, doctor and the statusline see it, and a typed rewrite (e.g.
        // a branch update) keeps it.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("s1.toml");
        std::fs::write(&file, lease_toml("")).unwrap();
        keep_ending_retained(&file, &refusal(true));
        let lease: crate::SessionLease =
            toml::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(crate::leases::is_ending_retained(&lease));
        assert_eq!(
            lease.ending_retained_reason.as_deref(),
            Some("the mailbox holds mail")
        );
        let rewritten = toml::to_string_pretty(&lease).unwrap();
        let again: crate::SessionLease = toml::from_str(&rewritten).unwrap();
        assert_eq!(again.ending_retained_at, lease.ending_retained_at);
        assert_eq!(again.ending_retained_reason, lease.ending_retained_reason);

        let plain: crate::SessionLease = toml::from_str(&lease_toml("")).unwrap();
        assert!(!crate::leases::is_ending_retained(&plain));
        assert!(!toml::to_string(&plain).unwrap().contains("ending_retained"));
    }

    #[test]
    fn every_reader_shows_an_ending_retained_lease_as_neither_active_nor_cleaned_up() {
        use crate::dispatch_health_ps::{apply_ending_retained, DispatchState as D};
        let stamped: crate::SessionLease = toml::from_str(&lease_toml(
            "ending_retained_at = \"2026-10-27T01:00:00Z\"\n\
             ending_retained_reason = \"no activation\"\n",
        ))
        .unwrap();
        let plain: crate::SessionLease = toml::from_str(&lease_toml("")).unwrap();
        // aida ps: overrides every observed state, including moving.
        for state in [D::Moving, D::Stalled, D::Stopped, D::Unknown] {
            assert_eq!(apply_ending_retained(state, true), D::EndingRetained);
            assert_eq!(apply_ending_retained(state, false), state);
        }
        assert_eq!(D::EndingRetained.label(), "ending-retained");
        // session leases: never hidden as stale.
        let stale = aida_core::liveness::LeaseState::Stale;
        let mut harness = stamped.clone();
        harness.scope = String::new();
        assert!(crate::leases::lease_visible_in_default_listing(
            &harness, stale
        ));
        let mut harness_plain = plain.clone();
        harness_plain.scope = String::new();
        assert!(!crate::leases::lease_visible_in_default_listing(
            &harness_plain,
            stale
        ));
        // statusline.
        assert!(crate::statusline_cmd::ending_retained_segment(&stamped).is_some());
        assert!(crate::statusline_cmd::ending_retained_segment(&plain).is_none());
        // reap rows.
        let note = crate::session_reap::ending_retained_note(&stamped, "x".into());
        assert!(
            note.starts_with("ending-retained") && note.contains("no activation"),
            "{note}"
        );
        assert_eq!(
            crate::session_reap::ending_retained_note(&plain, "x".into()),
            "x"
        );
    }

    #[test]
    fn the_refusal_names_the_activation_command_only_when_it_is_missing() {
        let command = aida_core::mailbox_lifecycle::ACTIVATION_COMMAND;
        let lines = refusal_lines(&refusal(true)).join("\n");
        assert!(lines.contains(command), "{lines}");
        assert!(lines.contains("/anchor/retained/k/3"), "{lines}");
        assert!(!refusal_lines(&refusal(false)).join("\n").contains(command));
    }
}

#[cfg(test)]
#[path = "tests/story_1843_c2_refusal_tests.rs"]
mod story_1843_c2_refusal_tests;
