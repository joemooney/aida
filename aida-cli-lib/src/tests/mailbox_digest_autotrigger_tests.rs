//! STORY-493: the session-end / drain-end auto-trigger calls
//! `maybe_digest_mailbox_best_effort`, which wraps `digest_mailbox_to_canonical`.
//! The call sites themselves `std::process::exit`, so the behavior under test
//! is the shared helper + its best-effort guarantee. trace:STORY-493 | ai:claude
use super::*;
use aida_core::mailbox::{Message, Recipient};
use tempfile::tempdir;

fn msg(id: &str) -> Message {
    Message {
        id: id.to_string(),
        thread_id: "t1".to_string(),
        from: "codex".to_string(),
        to: Recipient::Broadcast,
        timestamp: 1,
        in_reply_to: None,
        body: format!("body-{id}"),
        urgent: false,
        intent: aida_core::mailbox::Intent::Fyi,
        retracted: false,
        deleted: false,
    }
}

/// A project root with a git-initialized `.aida-store/` worktree, mirroring
/// the on-disk layout the auto-triggers run against. Returns (project_root, store_root).
fn project_with_store() -> (tempfile::TempDir, std::path::PathBuf) {
    let proj = tempdir().unwrap();
    let store_root = proj.path().join(".aida-store");
    std::fs::create_dir_all(&store_root).unwrap();
    aida_core::git_ops::init(&store_root).unwrap();
    aida_core::git_ops::configure_user(&store_root, "Test", "test@localhost").unwrap();
    (proj, store_root)
}

#[test]
fn helper_digests_local_messages_and_commits() {
    let (proj, store_root) = project_with_store();
    mailbox_store::write_message(proj.path(), &msg("a")).unwrap();
    mailbox_store::write_message(proj.path(), &msg("b")).unwrap();

    let n = digest_mailbox_to_canonical(&store_root, proj.path()).unwrap();
    assert_eq!(n, 2, "both local messages digest into the canonical store");
    let canon = mailbox_store::read_canonical_messages(&store_root).unwrap();
    assert_eq!(canon.len(), 2);
    // The digest committed on the orphan store: a commit now exists whose
    // message names the digest (confirms staged + committed, not left dirty).
    let log = std::process::Command::new("git")
        .args(["-C", store_root.to_str().unwrap(), "log", "--oneline"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&log.stdout);
    assert!(
        log.contains("mailbox: digest 2 message(s)"),
        "digest should have committed the mailbox dir; git log was:\n{log}"
    );
}

#[test]
fn helper_is_idempotent_double_fire_is_safe() {
    // Double-firing (session-end then drain-end) must not duplicate or error.
    let (proj, store_root) = project_with_store();
    mailbox_store::write_message(proj.path(), &msg("a")).unwrap();

    assert_eq!(
        digest_mailbox_to_canonical(&store_root, proj.path()).unwrap(),
        1
    );
    assert_eq!(
        digest_mailbox_to_canonical(&store_root, proj.path()).unwrap(),
        0,
        "re-digesting writes nothing new (id-keyed, idempotent)"
    );
    assert_eq!(
        mailbox_store::read_canonical_messages(&store_root)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn best_effort_no_local_mailbox_is_a_clean_noop() {
    // No `.aida/mailbox/` layer at all: must not error.
    // `_proj` is kept bound so the TempDir guard isn't dropped early.
    let (_proj, store_root) = project_with_store();
    // Should not panic / not write anything.
    maybe_digest_mailbox_best_effort(&store_root, "test");
    assert!(mailbox_store::read_canonical_messages(&store_root)
        .unwrap()
        .is_empty());
}

#[test]
fn best_effort_swallows_a_digest_error() {
    // Store path with no parent can't derive a project root; the best-effort
    // wrapper must return quietly rather than panic. (A non-git store dir
    // also exercises the Err arm of the wrapper without aborting.)
    let proj = tempdir().unwrap();
    let store_root = proj.path().join(".aida-store");
    std::fs::create_dir_all(&store_root).unwrap();
    // local message present but the store is NOT a git repo → commit fails;
    // the wrapper must log + continue, not propagate.
    mailbox_store::write_message(proj.path(), &msg("a")).unwrap();
    maybe_digest_mailbox_best_effort(&store_root, "test"); // must not panic
}

// ── STORY-643: auto mailbox sync (publish leg) ────────────────────────

/// The publish leg writes the local layer into canonical WITHOUT committing
/// (the store leg's own commit folds it in) and is idempotent + id-keyed:
// re-running stages nothing new. trace:STORY-643 | ai:claude
#[test]
fn publish_for_sync_stages_canonical_without_committing_and_is_idempotent() {
    let (proj, store_root) = project_with_store();
    mailbox_store::write_message(proj.path(), &msg("a")).unwrap();
    mailbox_store::write_message(proj.path(), &msg("b")).unwrap();

    let n = maybe_publish_mailbox_for_sync(&store_root, "test");
    assert_eq!(n, 2, "both local messages publish into the canonical store");
    let canon = mailbox_store::read_canonical_messages(&store_root).unwrap();
    assert_eq!(canon.len(), 2);
    // Publish does NOT commit on its own — the canonical files are left as
    // a pending change for the store leg's commit to pick up.
    assert!(
        aida_core::git_ops::has_changes(&store_root).unwrap(),
        "publish leaves the digested files uncommitted for the store leg"
    );
    // Idempotent: re-running stages nothing new (id-keyed).
    assert_eq!(maybe_publish_mailbox_for_sync(&store_root, "test"), 0);
}

// The opt-out (env or config) disables the publish leg entirely. trace:STORY-643
#[test]
fn publish_for_sync_honors_the_opt_out() {
    let (proj, store_root) = project_with_store();
    mailbox_store::write_message(proj.path(), &msg("a")).unwrap();

    // Env opt-out wins; nothing is published.
    std::env::set_var("AIDA_MAILBOX_AUTOSYNC", "0");
    assert!(!mailbox_autosync_enabled(proj.path()));
    assert_eq!(maybe_publish_mailbox_for_sync(&store_root, "test"), 0);
    assert!(mailbox_store::read_canonical_messages(&store_root)
        .unwrap()
        .is_empty());
    std::env::remove_var("AIDA_MAILBOX_AUTOSYNC");

    // Default (no env, no config) is on.
    assert!(mailbox_autosync_enabled(proj.path()));

    // Config opt-out is honored when the env is unset.
    std::fs::create_dir_all(proj.path().join(".aida")).unwrap();
    std::fs::write(
        proj.path().join(".aida").join("config.toml"),
        "[mailbox]\nautosync = false\n",
    )
    .unwrap();
    assert!(!mailbox_autosync_enabled(proj.path()));
    // Env presence overrides the config-file false.
    std::env::set_var("AIDA_MAILBOX_AUTOSYNC", "1");
    assert!(mailbox_autosync_enabled(proj.path()));
    std::env::remove_var("AIDA_MAILBOX_AUTOSYNC");
}
