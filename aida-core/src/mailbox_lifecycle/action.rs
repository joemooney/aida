//! Recipient assertions of action, independent of post-flush read ACKs.
//! trace:TASK-3074 | ai:codex
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    ack, check_owned_dir, ensure_anchor, lock, read_checked_capped, write_durable,
    LIFECYCLE_LOCK_TIMEOUT,
};
use crate::mailbox::{Intent, Message};
use crate::mailbox_logical::{receipt_keys, LogicalSnapshot, ResolvedAudience, SnapshotRecord};

const CAP: u64 = 4 * 1024 * 1024;
const MAX_REVISIONS: usize = 4096;
const MAX_TEXT: usize = 4096;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const FILE: &str = "actions-v1.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Acted,
    Declined,
    Deferred,
}

/// A recipient's assertion, never verified completion of protected work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionReceipt {
    pub message_id: String,
    pub fingerprint: String,
    /// Identity of the redacted withdrawal projection; original hash is retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn_fingerprint: Option<String>,
    pub actor: String,
    pub recipient_uuid: Option<crate::mailbox_delivery::RecipientUuid>,
    pub keys: Vec<String>,
    pub outcome: Outcome,
    pub recorded_at: i64,
    pub reason: String,
    pub evidence: Option<String>,
    pub until: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    schema: u32,
    revisions: Vec<ActionReceipt>,
}

fn text_valid(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= MAX_TEXT && !s.contains('\0')
}

fn validate_input(
    outcome: Outcome,
    reason: &str,
    evidence: Option<&str>,
    until: Option<i64>,
    now: i64,
) -> Result<()> {
    if !text_valid(reason) {
        bail!("reason must contain 1..4096 bytes of nonempty text");
    }
    if evidence.is_some_and(|s| !text_valid(s)) {
        bail!("evidence must contain 1..4096 bytes of nonempty text");
    }
    if outcome == Outcome::Acted && evidence.is_none() {
        bail!("acted requires --evidence (a recipient assertion, not verified completion)");
    }
    if outcome == Outcome::Deferred {
        if !until.is_some_and(|t| t > now && t <= now.saturating_add(DAY_MS)) {
            bail!("deferred requires --until in the future, at most 24 hours away");
        }
    } else if until.is_some() {
        bail!("--until is only valid for deferred");
    }
    Ok(())
}

/// Fixed v1 request identity: view markers and future Message fields do not
/// invalidate recorded evidence. Keep this field set stable.
pub fn fingerprint(message: &Message) -> Result<String> {
    let immutable = serde_json::json!({
        "id": message.id,
        "thread_id": message.thread_id,
        "from": message.from,
        "to": message.to,
        "timestamp": message.timestamp,
        "in_reply_to": message.in_reply_to,
        "subject": message.subject,
        "body": message.body,
        "intent": message.intent,
    });
    Ok(super::hex(&Sha256::digest(serde_json::to_vec(&immutable)?)))
}

fn read_ledger(anchor: &Path) -> Result<Ledger> {
    // Passive reads create nothing and refuse unsafe parent directories too.
    for dir in [anchor.parent().context("missing anchor parent")?, anchor] {
        match std::fs::symlink_metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Ledger {
                    schema: 1,
                    revisions: vec![],
                })
            }
            Err(e) => return Err(e.into()),
            Ok(_) => check_owned_dir(dir, dir == anchor)?,
        }
    }
    let Some(bytes) = read_checked_capped(&anchor.join(FILE), CAP)? else {
        return Ok(Ledger {
            schema: 1,
            revisions: vec![],
        });
    };
    if bytes.len() as u64 > CAP {
        bail!("action ledger is full");
    }
    let ledger: Ledger = serde_json::from_slice(&bytes).context("invalid action ledger")?;
    if ledger.schema != 1 || ledger.revisions.len() > MAX_REVISIONS {
        bail!("unsupported or full action ledger");
    }
    for r in &ledger.revisions {
        validate_input(
            r.outcome,
            &r.reason,
            r.evidence.as_deref(),
            r.until,
            r.recorded_at,
        )?;
        if r.message_id.is_empty()
            || r.message_id.len() > 256
            || !text_valid(&r.actor)
            || r.keys.is_empty()
            || r.keys.len() > ack::MAX_PLAN_KEYS
            || r.keys.iter().any(|k| !text_valid(k) || k.len() > 256)
            || r.withdrawn_fingerprint
                .as_ref()
                .is_some_and(|fp| fp.len() != 64 || !fp.bytes().all(|b| b.is_ascii_hexdigit()))
            || r.fingerprint.len() != 64
            || !r.fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
        {
            bail!("invalid action receipt");
        }
    }
    Ok(ledger)
}

/// Latest authorized revision, or unknown. Reading never initializes a ledger.
pub fn latest(common: &Path, message: &Message) -> Result<Option<ActionReceipt>> {
    let fp = fingerprint(message)?;
    let withdrawn = message
        .retracted
        .then(|| withdrawn_fingerprint(message))
        .transpose()?;
    Ok(read_ledger(&common.join("aida/mail-lifecycle"))?
        .revisions
        .into_iter()
        .rev()
        .find(|r| {
            r.message_id == message.id
                && (r.fingerprint == fp
                    || withdrawn
                        .as_ref()
                        .is_some_and(|fp| r.withdrawn_fingerprint.as_ref() == Some(fp)))
        }))
}

// Withdrawal physically clears body, and logical views also redact subject.
// Bind that exact tombstone projection while retaining the full original hash.
fn withdrawn_fingerprint(message: &Message) -> Result<String> {
    fingerprint(&Message {
        body: String::new(),
        subject: None,
        ..message.clone()
    })
}

/// Carries exact replicas and authority from observation to locked validation.
pub struct ActionPlan {
    common: PathBuf,
    validation: ack::AckPlan,
    audience: ResolvedAudience,
    receipt: ActionReceipt,
}

impl ActionPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        snapshot: &LogicalSnapshot,
        common: &Path,
        audience: &ResolvedAudience,
        record: &SnapshotRecord,
        outcome: Outcome,
        reason: &str,
        evidence: Option<&str>,
        until: Option<i64>,
    ) -> Result<Self> {
        let now = chrono::Utc::now().timestamp_millis();
        validate_input(outcome, reason, evidence, until, now)?;
        if !snapshot.ack_eligible() {
            bail!("partial or conflicting mail snapshot; action unknown");
        }
        // An ID must be an actual record in this snapshot, not a caller-built message.
        if !snapshot.records.iter().any(|r| std::ptr::eq(r, record)) {
            bail!("request is not in the snapshot");
        }
        if record.message.intent != Intent::Request
            || record.message.deleted
            || record.message.retracted
        {
            bail!("dispositions require an available kind=request message");
        }
        if record.message.id.is_empty() || record.message.id.len() > 256 {
            bail!("message ID exceeds the action ledger bounds");
        }
        let keys = receipt_keys(audience, record);
        if keys.is_empty()
            || keys.len() > ack::MAX_PLAN_KEYS
            || keys.iter().any(|k| !audience.holds_key(k))
        {
            bail!("only a current recipient may record a disposition");
        }
        if keys.iter().all(|key| audience.shared_keys().contains(key))
            && !audience.has_verified_binding()
        {
            bail!("shared-inbox dispositions require a verified session binding; a role hint alone grants no action authority");
        }
        let actor = audience
            .private_keys()
            .first()
            .or_else(|| audience.human_keys().first())
            .context("no concrete recipient identity")?
            .clone();
        Ok(Self {
            common: common.to_path_buf(),
            validation: ack::AckPlan::prepare_action(snapshot, common, audience, record)?,
            audience: audience.clone(),
            receipt: ActionReceipt {
                message_id: record.message.id.clone(),
                fingerprint: fingerprint(&record.message)?,
                withdrawn_fingerprint: Some(withdrawn_fingerprint(&record.message)?),
                actor,
                recipient_uuid: audience.recipient().cloned(),
                keys,
                outcome,
                recorded_at: now,
                reason: reason.to_owned(),
                evidence: evidence.map(str::to_owned),
                until,
            },
        })
    }

    /// `current` must re-observe inbox authority without Git/network/locks.
    /// Called after Lifecycle -> action-ledger; changed/expired bindings refuse.
    pub fn commit(
        self,
        current: impl FnOnce() -> Result<ResolvedAudience>,
    ) -> Result<ActionReceipt> {
        let anchor = ensure_anchor(&self.common)?;
        let held = lock(&anchor, LIFECYCLE_LOCK_TIMEOUT)?;
        let _ledger = ack::ledger_lock(&anchor, &held, ack::RECEIPT_LOCK_TIMEOUT, "actions.lock")?;
        if current()? != self.audience {
            bail!("recipient authority changed; disposition refused");
        }
        self.validation.validate_sources(&anchor, &held)?;
        self.validation.validate_replicas()?;
        let now = chrono::Utc::now().timestamp_millis();
        self.commit_at(&anchor, now)
    }

    fn commit_at(mut self, anchor: &Path, now: i64) -> Result<ActionReceipt> {
        validate_input(
            self.receipt.outcome,
            &self.receipt.reason,
            self.receipt.evidence.as_deref(),
            self.receipt.until,
            now,
        )?;
        self.receipt.recorded_at = now;
        let mut ledger = read_ledger(anchor)?;
        if ledger.revisions.len() >= MAX_REVISIONS {
            bail!("action ledger is full; no history was pruned");
        }
        ledger.revisions.push(self.receipt.clone());
        let bytes = serde_json::to_vec(&ledger)?;
        if bytes.len() as u64 > CAP {
            bail!("action ledger is full; no history was pruned");
        }
        write_durable(&anchor.join(FILE), &bytes)?;
        Ok(self.receipt)
    }
}

#[cfg(test)]
mod tests;
