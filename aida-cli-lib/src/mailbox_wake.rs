//! Bounded, non-consuming mailbox envelope for host notification adapters.
//!
//! STORY-1843 slice W: the envelope is built from the core logical snapshot
//! ([`crate::mailbox_view`]), with the audience a notice resolves and one
//! snapshot across every product worktree and the canonical store. It is a
//! pure read: it never binds a queued role delivery (in any mode), so it
//! cannot attach role mail to whatever session it runs under. Queued
//! deliveries are counted as `pending`; binding stays with explicit reads
//! and the per-turn notice. Unread means no receipt in the
//! common-Git anchor (or any other read evidence the snapshot holds), so a
//! read in a since-removed worktree stays read. Cohort mail owned by another
//! recipient UUID is not in the audience's view. A partial view is reported
//! as `partial: true`, never as an empty inbox.
// trace:TASK-2347 | ai:codex
// trace:STORY-1843 | ai:claude
use crate::mailbox_view::{read_own_mail, Coverage, OwnMail, ReadState};
use aida_core::mailbox_logical::{ReadLimits, ResolvedAudience};
use anyhow::{bail, Result};
use serde::Serialize;
use std::path::Path;

/// Version 1, extended additively: `partial`, `partial_codes` and `pending` are new;
/// `messages` and `acknowledged_ids` keep their meaning over the logical view.
#[derive(Serialize)]
pub(crate) struct Snapshot {
    version: u8,
    messages: Vec<Envelope>,
    acknowledged_ids: Vec<String>,
    /// Some source was not fully read: some unread mail may be missing.
    partial: bool,
    /// Safe source error codes (no paths, IDs or recipients).
    partial_codes: Vec<String>,
    /// Queued role deliveries waiting for a reader to take them (not bound
    /// here; their IDs stay out of `messages` until a binding read).
    pending: usize,
}
#[derive(Serialize)]
struct Envelope {
    id: String,
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The audience a notice for `recipient` resolves: this process's verified
/// audience narrowed to `recipient`. A name the audience does not hold
/// refuses; a selector never widens authority.
pub(crate) fn audience_for(own: &ResolvedAudience, recipient: &str) -> Result<ResolvedAudience> {
    if recipient.is_empty()
        || recipient.len() > 128
        || !recipient
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("Recipient must be a bounded exact mailbox identity");
    }
    match own.narrowed_to(recipient) {
        Some(audience) => Ok(audience),
        None => bail!(
            "`{recipient}` is not one of this process's inboxes; a wake snapshot covers only its own mail"
        ),
    }
}

/// Read `audience`'s logical view from `root` and project it to IDs.
/// Never binds: see the module docs.
pub(crate) fn snapshot(
    root: &Path,
    store: &Path,
    audience: &ResolvedAudience,
    limits: &ReadLimits,
) -> Result<Snapshot> {
    let view = read_own_mail(root, store, audience, limits);
    envelope(&view)
}

fn envelope(view: &OwnMail) -> Result<Snapshot> {
    let mut messages = Vec::new();
    let mut acknowledged = std::collections::BTreeSet::new();
    for record in view.records() {
        let id = &record.message.id;
        if !safe_id(id) {
            bail!("Mailbox message ID is not a bounded safe identity");
        }
        match ReadState::of(record) {
            ReadState::Unread if !record.message.archived => {
                messages.push(Envelope { id: id.clone() })
            }
            ReadState::Unread => {}
            ReadState::Read | ReadState::ReadLegacy => {
                acknowledged.insert(id.clone());
            }
        }
    }
    let (partial, partial_codes) = match view.coverage() {
        Coverage::Complete => (false, Vec::new()),
        Coverage::Partial { codes, conflicts } => {
            let mut names: Vec<String> = codes.iter().map(|c| format!("{c:?}")).collect();
            if *conflicts > 0 {
                names.push("ConflictingCopies".to_string());
            }
            names.sort();
            names.dedup();
            (true, names)
        }
    };
    Ok(Snapshot {
        version: 1,
        messages,
        acknowledged_ids: acknowledged.into_iter().collect(),
        partial,
        partial_codes,
        pending: view.pending_deliveries(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox_view::tests::{git, mail, mail_repo, put_canonical};
    use crate::mailbox_view::{ack_displayed, explicit_limits, read_own_mail, unbound_facts};

    fn advisor() -> ResolvedAudience {
        let own = ResolvedAudience::from_facts(&unbound_facts("operator", Some("advisor"), None));
        audience_for(&own, "advisor").unwrap()
    }

    fn wake(root: &Path, store: &Path, audience: &ResolvedAudience) -> serde_json::Value {
        serde_json::to_value(snapshot(root, store, audience, &explicit_limits()).unwrap()).unwrap()
    }

    fn unread(value: &serde_json::Value) -> Vec<String> {
        value["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn story1843_w_selector_never_widens_and_envelope_has_no_body() {
        let own = ResolvedAudience::from_facts(&unbound_facts("operator", Some("advisor"), None));
        assert!(audience_for(&own, "stranger").is_err());
        assert!(audience_for(&own, "../advisor").is_err());
        let r = mail_repo();
        let audience = advisor();
        crate::mailbox_store::write_message(&r.sib, &mail("sib-only", "advisor", 10, false))
            .unwrap();
        crate::mailbox_store::write_message(&r.main, &mail("other", "operator", 20, false))
            .unwrap();
        let value = wake(&r.main, &r.store, &audience);
        assert_eq!(unread(&value), vec!["sib-only"], "{value}");
        assert_eq!(value["partial"], false, "{value}");
        let text = value.to_string();
        assert!(!text.contains("body of"), "{text}");
        assert!(
            !r.main.join(".aida/mailbox/.read").exists(),
            "the snapshot moves no watermark"
        );
    }

    // Ruling test 1: mail read in a since-removed worktree does not wake.
    #[test]
    fn story1843_w_no_wake_for_mail_read_in_a_removed_worktree() {
        let r = mail_repo();
        let audience = advisor();
        let m = mail("read-there", "advisor", 10, false);
        crate::mailbox_store::write_message(&r.sib, &m).unwrap();
        put_canonical(&r.store, &m);
        assert_eq!(
            unread(&wake(&r.main, &r.store, &audience)),
            vec!["read-there"]
        );

        let view = read_own_mail(&r.sib, &r.store, &audience, &explicit_limits());
        let shown: Vec<_> = view.records().iter().collect();
        ack_displayed(&r.sib, &view, &shown).unwrap();
        git(
            &r.main,
            &["worktree", "remove", "--force", r.sib.to_str().unwrap()],
        );
        assert!(!r.sib.exists());

        let value = wake(&r.main, &r.store, &audience);
        assert!(unread(&value).is_empty(), "{value}");
        assert_eq!(value["acknowledged_ids"], serde_json::json!(["read-there"]));
        assert_eq!(value["partial"], false, "{value}");
    }

    // Ruling test 2: cohort mail another reader's UUID owns does not wake.
    #[test]
    fn story1843_w_no_wake_for_another_readers_cohort_mail() {
        let r = mail_repo();
        let audience = advisor();
        let sidecar = aida_core::mailbox_delivery::DeliverySidecar::queued("cohort", "advisor", 5);
        let delivery = sidecar.queued_ids().next().unwrap().to_string();
        crate::mailbox_store::write_message_with_deliveries(
            &r.sib,
            &mail("cohort", "advisor", 5, false),
            Some(&sidecar),
        )
        .unwrap();
        let other = aida_core::mailbox_delivery::RecipientUuid::parse(
            "00000000-0000-4000-8000-0000000000bb",
        )
        .unwrap();
        aida_core::mailbox_delivery::bind_queued_delivery(
            Some(&r.main.join(".git")),
            &r.sib,
            &delivery,
            &other,
            || true,
            aida_core::mailbox_lifecycle::LIFECYCLE_LOCK_TIMEOUT,
        )
        .unwrap();
        crate::mailbox_store::write_message(&r.main, &mail("plain", "advisor", 6, false)).unwrap();

        let value = wake(&r.main, &r.store, &audience);
        assert_eq!(unread(&value), vec!["plain"], "{value}");
        assert!(!value.to_string().contains("cohort"), "{value}");
    }

    // Ruling test 3: a partial source is flagged, never an empty success.
    #[test]
    fn story1843_w_partial_source_is_flagged() {
        let r = mail_repo();
        let audience = advisor();
        crate::mailbox_store::write_message(&r.main, &mail("seen", "advisor", 10, false)).unwrap();
        std::fs::remove_dir_all(&r.sib).unwrap();
        let value = wake(&r.main, &r.store, &audience);
        assert_eq!(value["partial"], true, "{value}");
        assert!(
            !value["partial_codes"].as_array().unwrap().is_empty(),
            "{value}"
        );
        assert_eq!(
            unread(&value),
            vec!["seen"],
            "what was read is still listed"
        );
        assert!(!value.to_string().contains(r.sib.to_str().unwrap()));
    }
    // Ruling on PR-3079 F1: a wake snapshot run under another session (here a
    // registered reviewer session with the role forced to advisor, as the
    // adapter's environment does) binds nothing: queued advisor role mail
    // stays queued and is only counted as pending.
    #[test]
    fn story1843_w_wake_snapshot_under_a_non_advisor_session_binds_nothing() {
        use crate::mailbox_view::tests::{register_self_as_reviewer, D1_SESSION};
        let r = mail_repo();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_SESSION_ROLE", Some("advisor")),
            ("AIDA_HANDLE", None),
            ("AIDA_AGENT_NAME", None),
            ("AIDA_HOOK_PAYLOAD", None),
            ("CODEX_THREAD_ID", Some(D1_SESSION)),
        ]);
        let own = register_self_as_reviewer(&r.main);
        assert!(own.recipient().is_some(), "the session owns a UUID");
        let audience = audience_for(&own, "advisor").unwrap();
        let sidecar = aida_core::mailbox_delivery::DeliverySidecar::queued("queued", "advisor", 5);
        crate::mailbox_store::write_message_with_deliveries(
            &r.sib,
            &mail("queued", "advisor", 5, false),
            Some(&sidecar),
        )
        .unwrap();

        for _ in 0..2 {
            let value = wake(&r.main, &r.store, &audience);
            assert!(unread(&value).is_empty(), "{value}");
            assert_eq!(value["pending"], 1, "{value}");
        }
        let view = read_own_mail(&r.main, &r.store, &audience, &explicit_limits());
        assert_eq!(view.pending_deliveries(), 1, "still queued, never bound");
        assert!(view.records().is_empty());
    }
}
