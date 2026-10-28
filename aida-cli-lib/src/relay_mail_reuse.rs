//! Reusing an ended relay run's label (STORY-1843 D2b).
//!
//! Mail sent to a relay label is owned by the run that held the label, so a
//! new run under the same label does not read the old run's unread mail. A
//! new run may take the label only after an explicit choice about that mail:
//! leave it with the old run (`--leave-with-old`), or hand it over
//! (`--handover`), which adds the new run as an owner of each unread
//! delivery. Handover never renames, copies or acknowledges anything, so
//! the old run's receipts stay intact and the new run acknowledges under its
//! own UUID.
//!
//! The old run's unread mail is inspected by count only, and only by a
//! caller that proved the controller and its run lock and holds the new
//! run's capability. Label mail written without a delivery record (legacy
//! mail) is readable by whoever holds the spelling, so a label with legacy
//! unread mail is never reused: it would be read by the new run. A count
//! that could not be completed is treated as unread mail.
// trace:STORY-1843 | ai:claude

use super::*;
use aida_core::mailbox_delivery::RecipientUuid;
use aida_core::mailbox_logical::{MailboxContext, ReadLimits, RelayRunDelivery};

/// What happens to the old run's unread mail when its label is reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReuseChoice {
    /// The mail stays with the old run; the new run does not see it.
    LeaveWithOld,
    /// Each unread delivery gains the new run as an owner.
    Handover,
}

/// The old run whose label a bind takes, and its unread mail.
pub(crate) struct ReusePlan {
    pub(crate) previous: String,
    choice: Option<ReuseChoice>,
    unread: Vec<RelayRunDelivery>,
}

/// What a reusing bind did with the old run's unread mail.
#[derive(Debug, Serialize)]
pub(crate) struct ReuseReport {
    pub(crate) previous_run: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) choice: Option<ReuseChoice>,
    pub(crate) unread: usize,
    pub(crate) handed_over: usize,
    /// Handovers that could not be written; that mail stays with the old run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) handover_errors: Vec<String>,
}

/// The ended relay run that currently holds `label`'s reservation, if any.
pub(crate) fn ended_run_of_label(project_root: &Path, label: &str) -> Result<Option<RelayRun>> {
    let Some(reservation) = agent_handle::checked_reservation(project_root, label)? else {
        return Ok(None);
    };
    Ok(load_run(project_root, &reservation.recipient_id)
        .ok()
        .flatten()
        .filter(|run| run.terminal.is_some() && run.label == reservation.handle))
}

fn mailbox_context(project_root: &Path) -> Result<MailboxContext> {
    let store = project_root.join(".aida-store");
    let store = std::fs::symlink_metadata(&store).is_ok().then_some(store);
    MailboxContext::resolve_or_local(project_root, store.as_deref())
        .map_err(|code| anyhow::anyhow!("cannot read the earlier relay run's mail: {code:?}"))
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Decide whether a new run may take `label` from the ended run that holds
/// it. `capability` must be the new run's, matching `new_run_digest`; only
/// then is the old run's unread mail counted. Taken with no lock held (the
/// mailbox read never nests under the namespace lock); the bind re-checks
/// under the lock that the same run still holds the label.
pub(crate) fn plan_reuse(
    project_root: &Path,
    label: &str,
    new_run_digest: &str,
    capability: &str,
    choice: Option<ReuseChoice>,
) -> Result<Option<ReusePlan>> {
    if !constant_time_eq(
        capability_digest(capability).as_bytes(),
        new_run_digest.as_bytes(),
    ) {
        bail!("relay label reuse refused: the new run's capability does not match");
    }
    let Some(previous) = ended_run_of_label(project_root, label)? else {
        return Ok(None);
    };
    let uuid =
        RecipientUuid::parse(&previous.run_id).context("relay run id is not a recipient UUID")?;
    let found = aida_core::mailbox_logical::relay_run_unread_deliveries(
        &mailbox_context(project_root)?,
        &previous.label,
        &uuid,
        &ReadLimits::explicit_read(),
    );
    let run = &previous.run_id;
    if !found.complete {
        bail!(
            "relay label `{label}` belonged to relay run {run}, whose unread mail could not be fully counted; it is treated as unread, so the label is not reused. Retry, or choose a new label"
        );
    }
    if found.legacy_label > 0 {
        bail!(
            "relay label `{label}` has {} unread message{} addressed to the label with no delivery record; the new run would read {}, so the label cannot be reused. Choose a new label",
            found.legacy_label,
            plural(found.legacy_label),
            if found.legacy_label == 1 { "it" } else { "them" }
        );
    }
    let owned = found.unread.len();
    if owned > 0 && choice.is_none() {
        bail!(
            "relay label `{label}` belonged to relay run {run}, which has {owned} unread message{}; pass --leave-with-old to leave {} with that run, or --handover to let the new run read {}",
            plural(owned),
            if owned == 1 { "it" } else { "them" },
            if owned == 1 { "it" } else { "them" }
        );
    }
    Ok(Some(ReusePlan {
        previous: previous.run_id,
        choice,
        unread: found.unread,
    }))
}

/// Carry out `plan` for the new run `new_run`. Call with no lock held:
/// writing a handover takes the mailbox lifecycle lock, which is never
/// taken under the namespace lock.
pub(crate) fn apply_reuse(project_root: &Path, plan: ReusePlan, new_run: &str) -> ReuseReport {
    debug_assert!(!agent_handle::namespace_lock_held());
    let mut report = ReuseReport {
        previous_run: plan.previous,
        choice: plan.choice,
        unread: plan.unread.len(),
        handed_over: 0,
        handover_errors: Vec::new(),
    };
    if plan.choice != Some(ReuseChoice::Handover) {
        return report;
    }
    let Some(recipient) = RecipientUuid::parse(new_run) else {
        report
            .handover_errors
            .push(format!("relay run id {new_run} is not a recipient UUID"));
        return report;
    };
    for d in &plan.unread {
        let record =
            aida_core::mailbox_delivery::handover_record(&d.message_id, &d.delivery_id, &recipient);
        match aida_core::mailbox_delivery::write_handover(project_root, &record) {
            Ok(()) => report.handed_over += 1,
            Err(e) => report
                .handover_errors
                .push(format!("{}: {e}", d.message_id)),
        }
    }
    report
}
