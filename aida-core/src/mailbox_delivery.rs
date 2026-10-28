//! Additive delivery ownership for role mail (STORY-1843 slice D1).
//!
//! A message sent to a role keeps its original shape (`to` is still the
//! role). Beside it, in the same mailbox source, a sidecar
//! `.deliveries/<message-id>.json` records who it was delivered to: one
//! [`DeliveryRecord`] per live role reader that had a recipient UUID when
//! the message was sent (the role cohort), or one pending record for an
//! explicit `--queue-for-role` send to a role with no readers. The message
//! file stays immutable; IDs, threads, tombstones and historical receipts
//! are untouched, and a message with no sidecar keeps the legacy
//! shared-role semantics. Legacy addresses get no inferred ownership.
//!
//! A cohort message is visible only to the UUIDs in its cohort, and each
//! reader acknowledges it under its own UUID key
//! ([`RecipientUuid::receipt_key`]), so readers' receipts are independent
//! and a shared role watermark never becomes a private ACK.
//!
//! A pending record is bound once, to the first eligible reader whose
//! snapshot sees it: [`bind_queued_delivery`] creates (O_EXCL) one binding
//! file under the private lifecycle anchor, holding the Lifecycle lock alone,
//! after re-checking that the binder still owns its UUID. A local-only
//! (non-Git) project binds into its one mailbox with the same create-new
//! rule. Sidecars and bindings stay private: canonical publication does not
//! carry them (a canonical-only clone sees a cohort message as legacy role
//! mail; documented in docs/mailbox.md).
// trace:STORY-1843 | ai:claude

use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;

/// Directory, inside a mailbox source, holding delivery sidecars.
pub const DELIVERIES_DIR: &str = ".deliveries";
/// Under the lifecycle anchor (or a local-only mailbox's
/// [`DELIVERIES_DIR`]): one create-new binding file per queued delivery.
pub const BINDINGS_DIR: &str = "bound";
/// Sidecar schema version.
pub const DELIVERY_SCHEMA: u32 = 1;
/// Most delivery records one sidecar may carry.
pub const MAX_DELIVERIES: usize = 256;
/// Byte cap on one sidecar or binding file.
pub const DELIVERY_FILE_CAP: u64 = 256 * 1024;

/// A logical recipient's UUID (the handle reservation's `recipient_id`).
/// Opaque: the only constructor is [`RecipientUuid::parse`], which accepts
/// only the canonical lowercase hyphenated UUID form. A UUID is an address,
/// never an authority on its own: a reader holds one only through its
/// verified session binding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RecipientUuid(String);

impl RecipientUuid {
    /// Accept `raw` only in canonical 8-4-4-4-12 lowercase hex form.
    pub fn parse(raw: &str) -> Option<Self> {
        let groups: Vec<&str> = raw.split('-').collect();
        let shape = [8, 4, 4, 4, 12];
        let ok = groups.len() == shape.len()
            && groups.iter().zip(shape).all(|(g, len)| {
                g.len() == len
                    && g.chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            });
        ok.then(|| Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The receipt-anchor key a cohort delivery to this recipient is
    /// acknowledged under. No mailbox identity contains `:`, so it never
    /// collides with a handle, role, vendor or person key.
    pub fn receipt_key(&self) -> String {
        format!("uuid:{}", self.0)
    }
}

impl<'de> Deserialize<'de> for RecipientUuid {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom("not a recipient UUID"))
    }
}

/// The role readers a message was snapshotted to at send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleCohort {
    pub role: String,
    /// Epoch millis of the send-time reader snapshot.
    pub snapshot_at: i64,
    /// Every reader UUID in the snapshot (empty for a queued delivery).
    pub reader_uuids: Vec<RecipientUuid>,
}

/// One delivery of a message to one recipient (or, queued, to the first
/// eligible reader of a role).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryRecord {
    pub delivery_id: String,
    pub message_id: String,
    /// The recipient; `None` only for a still-unbound queued delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_uuid: Option<RecipientUuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cohort: Option<RoleCohort>,
    #[serde(default)]
    pub queued_for_role: bool,
    /// The relay run that owns the recipient (slice D2); `None` here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_uuid: Option<String>,
    /// For a handover delivery (slice D2b): the delivery it hands over.
    /// The original keeps its recipient and receipts.
    // trace:STORY-1843 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handover_of: Option<String>,
    /// For a relay end notice (slice D2b): the undelivered delivery it
    /// reports. A notice never produces another notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_of: Option<String>,
}

/// The sidecar file beside one message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverySidecar {
    pub schema: u32,
    pub message_id: String,
    pub deliveries: Vec<DeliveryRecord>,
}

/// Why a sidecar or binding was rejected. Safe to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryFormatError {
    Corrupt,
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl DeliverySidecar {
    /// A cohort sidecar: one record per reader UUID.
    pub fn cohort(
        message_id: &str,
        role: &str,
        snapshot_at: i64,
        readers: &[RecipientUuid],
    ) -> Self {
        let cohort = RoleCohort {
            role: role.to_string(),
            snapshot_at,
            reader_uuids: readers.to_vec(),
        };
        Self {
            schema: DELIVERY_SCHEMA,
            message_id: message_id.to_string(),
            deliveries: readers
                .iter()
                .map(|r| DeliveryRecord {
                    delivery_id: uuid::Uuid::new_v4().to_string(),
                    message_id: message_id.to_string(),
                    recipient_uuid: Some(r.clone()),
                    cohort: Some(cohort.clone()),
                    queued_for_role: false,
                    run_uuid: None,
                    handover_of: None,
                    notice_of: None,
                })
                .collect(),
        }
    }

    /// One pending delivery for an explicit `--queue-for-role` send.
    pub fn queued(message_id: &str, role: &str, snapshot_at: i64) -> Self {
        Self {
            schema: DELIVERY_SCHEMA,
            message_id: message_id.to_string(),
            deliveries: vec![DeliveryRecord {
                delivery_id: uuid::Uuid::new_v4().to_string(),
                message_id: message_id.to_string(),
                recipient_uuid: None,
                cohort: Some(RoleCohort {
                    role: role.to_string(),
                    snapshot_at,
                    reader_uuids: Vec::new(),
                }),
                queued_for_role: true,
                run_uuid: None,
                handover_of: None,
                notice_of: None,
            }],
        }
    }

    /// One delivery to the relay run `run`, which owns the label the
    /// message is addressed to (slice D2b).
    // trace:STORY-1843 | ai:claude
    pub fn relay(message_id: &str, run: &RecipientUuid) -> Self {
        Self::single(DeliveryRecord {
            delivery_id: uuid::Uuid::new_v4().to_string(),
            message_id: message_id.to_string(),
            recipient_uuid: Some(run.clone()),
            cohort: None,
            queued_for_role: false,
            run_uuid: Some(run.as_str().to_string()),
            handover_of: None,
            notice_of: None,
        })
    }

    /// A relay end notice to `recipient` about `original_delivery`
    /// (slice D2b).
    pub fn notice(message_id: &str, recipient: &RecipientUuid, original_delivery: &str) -> Self {
        Self::single(DeliveryRecord {
            delivery_id: uuid::Uuid::new_v4().to_string(),
            message_id: message_id.to_string(),
            recipient_uuid: Some(recipient.clone()),
            cohort: None,
            queued_for_role: false,
            run_uuid: None,
            handover_of: None,
            notice_of: Some(original_delivery.to_string()),
        })
    }

    /// Mark every delivery of this sidecar as a notice about
    /// `original_delivery`.
    pub fn as_notice_of(mut self, original_delivery: &str) -> Self {
        for d in &mut self.deliveries {
            d.notice_of = Some(original_delivery.to_string());
        }
        self
    }

    fn single(record: DeliveryRecord) -> Self {
        Self {
            schema: DELIVERY_SCHEMA,
            message_id: record.message_id.clone(),
            deliveries: vec![record],
        }
    }

    /// Whether any delivery of this message is a relay end notice.
    pub fn is_notice(&self) -> bool {
        self.deliveries.iter().any(|d| d.notice_of.is_some())
    }

    /// Parse and validate the sidecar read from `<stem>.json`.
    pub fn parse(bytes: &[u8], stem: &str) -> Result<Self, DeliveryFormatError> {
        let sidecar: Self =
            serde_json::from_slice(bytes).map_err(|_| DeliveryFormatError::Corrupt)?;
        let valid = sidecar.schema == DELIVERY_SCHEMA
            && sidecar.message_id == stem
            && safe_id(&sidecar.message_id)
            && !sidecar.deliveries.is_empty()
            && sidecar.deliveries.len() <= MAX_DELIVERIES
            && sidecar.deliveries.iter().all(|d| {
                d.message_id == sidecar.message_id
                    && safe_id(&d.delivery_id)
                    && d.handover_of.is_none()
                    && d.notice_of.as_deref().is_none_or(safe_id)
                    && (d.queued_for_role || d.recipient_uuid.is_some())
                    && d.cohort
                        .as_ref()
                        .is_none_or(|c| c.reader_uuids.len() <= MAX_DELIVERIES)
            });
        valid.then_some(sidecar).ok_or(DeliveryFormatError::Corrupt)
    }

    /// The queued deliveries still waiting for a reader.
    pub fn queued_ids(&self) -> impl Iterator<Item = &str> {
        self.deliveries
            .iter()
            .filter(|d| d.queued_for_role)
            .map(|d| d.delivery_id.as_str())
    }

    /// The recipients that own this message, given the bindings of its
    /// queued deliveries.
    pub fn owners<'a>(
        &'a self,
        binding: impl Fn(&str) -> Option<RecipientUuid> + 'a,
    ) -> Vec<RecipientUuid> {
        let mut out: Vec<RecipientUuid> = self
            .deliveries
            .iter()
            .filter_map(|d| match (&d.recipient_uuid, d.queued_for_role) {
                (Some(r), _) => Some(r.clone()),
                (None, true) => binding(&d.delivery_id),
                (None, false) => None,
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// Inside [`DELIVERIES_DIR`]: one file per handed-over delivery (slice
/// D2b), named `<message-id>.<new-recipient>.json`. Each holds one
/// [`DeliveryRecord`] whose `handover_of` names the delivery it hands over.
/// The original sidecar is never rewritten.
pub const HANDOVER_DIR: &str = "handover";

/// The handover file name for `record`.
pub fn handover_file_name(message_id: &str, recipient: &RecipientUuid) -> String {
    format!("{message_id}.{}.json", recipient.as_str())
}

/// A handover delivery: `recipient` takes over `original` (a delivery of
/// `message_id` owned by an ended relay run). Its delivery ID is derived
/// from both, so writing it twice yields the same record.
// trace:STORY-1843 | ai:claude
pub fn handover_record(
    message_id: &str,
    original: &str,
    recipient: &RecipientUuid,
) -> DeliveryRecord {
    let digest = {
        use sha2::{Digest, Sha256};
        Sha256::digest(format!("{original}\n{}", recipient.as_str()).as_bytes())
    };
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    DeliveryRecord {
        delivery_id: format!("handover-{hex}"),
        message_id: message_id.to_string(),
        recipient_uuid: Some(recipient.clone()),
        cohort: None,
        queued_for_role: false,
        run_uuid: Some(recipient.as_str().to_string()),
        handover_of: Some(original.to_string()),
        notice_of: None,
    }
}

/// Parse a handover file named `name` (`<message-id>.<recipient>.json`).
pub fn parse_handover(bytes: &[u8], name: &str) -> Result<DeliveryRecord, DeliveryFormatError> {
    let record: DeliveryRecord =
        serde_json::from_slice(bytes).map_err(|_| DeliveryFormatError::Corrupt)?;
    let valid = record
        .recipient_uuid
        .as_ref()
        .is_some_and(|r| handover_file_name(&record.message_id, r) == name)
        && safe_id(&record.message_id)
        && safe_id(&record.delivery_id)
        && record.handover_of.as_deref().is_some_and(safe_id)
        && record.notice_of.is_none()
        && !record.queued_for_role
        && record.cohort.is_none();
    valid.then_some(record).ok_or(DeliveryFormatError::Corrupt)
}

/// Write handover `record` into `project_root`'s local mailbox, under the
/// same write admission as a message.
#[cfg(all(unix, feature = "native"))]
pub fn write_handover(project_root: &Path, record: &DeliveryRecord) -> std::io::Result<()> {
    let recipient = record
        .recipient_uuid
        .as_ref()
        .ok_or_else(|| std::io::Error::other("handover without a recipient"))?;
    let name = handover_file_name(&record.message_id, recipient);
    let json = serde_json::to_vec_pretty(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    parse_handover(&json, &name).map_err(|_| std::io::Error::other("invalid handover record"))?;
    let mailbox = crate::mailbox::local_mailbox_dir(project_root);
    crate::mailbox::mailbox_dir_guard(&mailbox)?;
    let coverage = crate::mailbox_lifecycle::cover_local_write(project_root)?;
    let dir = mailbox.join(DELIVERIES_DIR).join(HANDOVER_DIR);
    std::fs::create_dir_all(&dir)?;
    crate::write_atomic(&dir.join(name), &json)?;
    if let crate::mailbox_lifecycle::AdmissionCoverage::Admitted(permit) = coverage {
        let _ = permit.complete();
    }
    Ok(())
}

/// One queued delivery's binding to the reader that took it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryBinding {
    pub schema: u32,
    pub delivery_id: String,
    pub recipient_uuid: RecipientUuid,
    pub bound_at: i64,
}

impl DeliveryBinding {
    pub fn parse(bytes: &[u8], delivery_id: &str) -> Result<Self, DeliveryFormatError> {
        let b: Self = serde_json::from_slice(bytes).map_err(|_| DeliveryFormatError::Corrupt)?;
        (b.schema == DELIVERY_SCHEMA && b.delivery_id == delivery_id)
            .then_some(b)
            .ok_or(DeliveryFormatError::Corrupt)
    }
}

/// The binding file name for a delivery: a digest, so no ID spelling can
/// escape the directory.
pub fn binding_file_name(delivery_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(delivery_id.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{hex}.json")
}

/// Path components of the binding directory: below the verified common Git
/// directory when the project has one, else below the checkout's mailbox.
pub fn binding_dir_components(anchored: bool) -> &'static [&'static str] {
    if anchored {
        &["aida", "mail-lifecycle", "deliveries", BINDINGS_DIR]
    } else {
        &[".aida", "mailbox", DELIVERIES_DIR, BINDINGS_DIR]
    }
}

/// Write `sidecar` into `project_root`'s local mailbox. The caller writes it
/// before the message, under the same admission, so a message is never
/// visible without its ownership.
pub(crate) fn write_sidecar(project_root: &Path, sidecar: &DeliverySidecar) -> std::io::Result<()> {
    if !safe_id(&sidecar.message_id) {
        return Err(std::io::Error::other(
            "unsafe message id for a delivery sidecar",
        ));
    }
    let dir = crate::mailbox::local_mailbox_dir(project_root).join(DELIVERIES_DIR);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(sidecar)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::write_atomic(&dir.join(format!("{}.json", sidecar.message_id)), &json)
}

/// The outcome of binding a queued delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindOutcome {
    /// This reader took it.
    Bound,
    /// Another reader (or this one, earlier) already holds it.
    AlreadyBound(RecipientUuid),
}

/// Why a bind was not attempted or failed. Retryable; never shown with IDs.
#[derive(Debug)]
pub enum BindError {
    /// The reader no longer owns the UUID it would bind.
    NotOwned,
    /// The lifecycle anchor or lock refused (including a busy lock).
    #[cfg(all(unix, feature = "native"))]
    Lifecycle(crate::mailbox_lifecycle::LifecycleError),
    Io(std::io::Error),
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindError::NotOwned => f.write_str("the reader no longer owns its recipient UUID"),
            #[cfg(all(unix, feature = "native"))]
            BindError::Lifecycle(e) => write!(f, "mailbox lifecycle refused: {e}"),
            BindError::Io(e) => write!(f, "binding write failed: {e}"),
        }
    }
}

impl std::error::Error for BindError {}

impl From<std::io::Error> for BindError {
    fn from(e: std::io::Error) -> Self {
        BindError::Io(e)
    }
}

/// Bind queued delivery `delivery_id` to `reader`, exactly once across the
/// repository. `common` is the verified common Git directory (`None`: a
/// local-only project, which binds into `checkout`'s mailbox). The binding
/// is a create-new file; when it already exists the holder is reported.
/// With an anchor, the Lifecycle lock is held (and no other lock taken)
/// around the ownership re-check and the create; `lock_timeout` bounds the
/// wait, so a passive caller passes a short one and skips on contention.
/// `still_owned` is a read-only re-check that the binder still owns
/// `reader`.
// trace:STORY-1843 | ai:claude
#[cfg(all(unix, feature = "native"))]
pub fn bind_queued_delivery(
    common: Option<&Path>,
    checkout: &Path,
    delivery_id: &str,
    reader: &RecipientUuid,
    still_owned: impl FnOnce() -> bool,
    lock_timeout: std::time::Duration,
) -> Result<BindOutcome, BindError> {
    use crate::mailbox_lifecycle as lc;
    if !safe_id(delivery_id) {
        return Err(BindError::Io(std::io::Error::other("unsafe delivery id")));
    }
    let binding = DeliveryBinding {
        schema: DELIVERY_SCHEMA,
        delivery_id: delivery_id.to_string(),
        recipient_uuid: reader.clone(),
        bound_at: chrono::Utc::now().timestamp_millis(),
    };
    let bytes = serde_json::to_vec_pretty(&binding)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let name = binding_file_name(delivery_id);
    match common {
        Some(common) => {
            // Facts before the lock: the anchor and its directories exist.
            let anchor = lc::ensure_anchor(common).map_err(BindError::Lifecycle)?;
            let deliveries = anchor.join("deliveries");
            lc::ensure_dir(&deliveries, true).map_err(BindError::Lifecycle)?;
            let dir = deliveries.join(BINDINGS_DIR);
            lc::ensure_dir(&dir, true).map_err(BindError::Lifecycle)?;
            let held = lc::lock(&anchor, lock_timeout).map_err(BindError::Lifecycle)?;
            let outcome = create_binding(&dir, &name, delivery_id, &bytes, still_owned);
            drop(held);
            outcome
        }
        None => {
            let dir = crate::mailbox::local_mailbox_dir(checkout)
                .join(DELIVERIES_DIR)
                .join(BINDINGS_DIR);
            std::fs::create_dir_all(&dir)?;
            // No anchor, so no Lifecycle lock: a lock file in the bindings
            // directory serializes binders, so no binder ever reads a
            // binding another is still writing.
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join(".lock"))?;
            let start = std::time::Instant::now();
            loop {
                match fs2::FileExt::try_lock_exclusive(&lock) {
                    Ok(()) => break,
                    Err(e) if crate::file_lock::is_lock_contended(&e) => {
                        if start.elapsed() >= lock_timeout {
                            return Err(BindError::Io(std::io::Error::new(
                                std::io::ErrorKind::WouldBlock,
                                "delivery binding lock busy",
                            )));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            let outcome = create_binding(&dir, &name, delivery_id, &bytes, still_owned);
            let _ = fs2::FileExt::unlock(&lock);
            outcome
        }
    }
}

#[cfg(all(unix, feature = "native"))]
fn create_binding(
    dir: &Path,
    name: &str,
    delivery_id: &str,
    bytes: &[u8],
    still_owned: impl FnOnce() -> bool,
) -> Result<BindOutcome, BindError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = dir.join(name);
    if let Some(existing) = crate::mailbox_lifecycle::read_checked_capped(&path, DELIVERY_FILE_CAP)
        .map_err(BindError::Lifecycle)?
    {
        let held = DeliveryBinding::parse(&existing, delivery_id)
            .map_err(|_| std::io::Error::other("corrupt delivery binding"))?;
        return Ok(BindOutcome::AlreadyBound(held.recipient_uuid));
    }
    if !still_owned() {
        return Err(BindError::NotOwned);
    }
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = crate::mailbox_lifecycle::read_checked_capped(&path, DELIVERY_FILE_CAP)
                .map_err(BindError::Lifecycle)?
                .ok_or_else(|| std::io::Error::other("delivery binding vanished"))?;
            let held = DeliveryBinding::parse(&existing, delivery_id)
                .map_err(|_| std::io::Error::other("corrupt delivery binding"))?;
            return Ok(BindOutcome::AlreadyBound(held.recipient_uuid));
        }
        Err(e) => return Err(e.into()),
    };
    file.write_all(bytes)?;
    file.sync_all()?;
    crate::mailbox_lifecycle::fsync_dir(dir).map_err(BindError::Lifecycle)?;
    Ok(BindOutcome::Bound)
}

#[cfg(test)]
#[path = "mailbox_delivery_tests.rs"]
mod tests;
