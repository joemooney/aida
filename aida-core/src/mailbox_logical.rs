//! Read-only logical mailbox snapshot across the product worktrees of one
//! repository (STORY-1843 slice B1).
//!
//! Every ordinary reader today composes its own view of "my mail" from one
//! checkout's local `.aida/mailbox/` plus the canonical store mailbox, so a
//! message written in a sibling worktree is invisible from main. This module
//! is the one composition: a typed [`MailboxContext`] that keeps the requested
//! checkout independent of the canonical store root, a bounded inventory of
//! the Git-registered product worktrees of the SAME common Git directory, and
//! a pure-inspection [`logical_snapshot`] that reconciles messages by
//! immutable ID, filters to the resolved audience before anything is
//! projected, and reports per-source coverage and typed errors.
//!
//! The snapshot never writes: no receipts, watermarks, markers or directories
//! are created. An empty `records` list means nothing on its own — callers use
//! [`LogicalSnapshot::caught_up`], which is `None` unless every source was
//! fully observed. Retention of mail from retired worktrees is not
//! established here (signed slice C). Explicit per-ID receipts are read from
//! the sources and from the private receipt anchor under the common Git
//! directory; the post-flush ACK that writes the anchor is
//! `mailbox_lifecycle::ack` (slice B2c).
//!
//! Source-local write paths (digest, archive/retract/delete, migration,
//! lifecycle) keep using the explicit per-source readers in
//! [`crate::mailbox`]; this global view grants no write scope.
// trace:STORY-1843 | ai:claude

use crate::mailbox::{
    message_state_rank, mtime_millis, normalize_message_value, timestamp_is_known,
    written_timestamp, Message, Recipient,
};
use crate::mailbox_delivery::{
    binding_dir_components, binding_file_name, parse_handover, DeliveryBinding, DeliveryRecord,
    DeliverySidecar, RecipientUuid, DELIVERIES_DIR, DELIVERY_FILE_CAP, HANDOVER_DIR,
};
use crate::mailbox_identity::{classified_identities, IdentityClass, MailIdentityFacts};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Branch the canonical store worktree checks out; never a product source.
const STORE_BRANCH: &str = "refs/heads/aida-store";
/// Upper bound on `git worktree list` output.
const INVENTORY_BYTES: usize = 1024 * 1024;
/// Upper bound on small pointer/watermark/receipt files.
const SMALL_FILE: u64 = 4096;

/// Why a source (or the inventory) could not be fully observed. Codes are
/// safe to show: they never carry message filenames, bodies or recipients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceErrorCode {
    /// The path is not a Git worktree root.
    NotAGitCheckout,
    /// The source belongs to a different common Git directory.
    CommonGitMismatch,
    /// A registered worktree is gone from disk.
    WorktreeMissing,
    /// Git reports the registered worktree as prunable.
    WorktreePrunable,
    /// `git worktree list` failed or was refused.
    InventoryFailed,
    /// More worktrees, inventory bytes or time than the read limits allow.
    InventoryOverBudget,
    /// A symlink, hardlink, FIFO or other non-regular entry.
    UnsafeEntry,
    /// Not owned by the current user, or world-writable.
    OwnershipRefused,
    /// An I/O error other than absence.
    Unreadable,
    /// A message file that does not parse, or carries an unsafe ID.
    CorruptMessage,
    /// The mailbox path is a layout manifest or other non-directory.
    LayoutUnsupported,
    /// Entry, byte or time budget exhausted while reading the source.
    OverBudget,
    /// The source directory was replaced while it was being read.
    SourceChanged,
    /// A watermark or read receipt that does not parse.
    CorruptReceipt,
}

/// The checkout the caller is reading from. Never replaced by main or the
/// store's parent: a pooled checkout without a `.aida-store` alias still
/// reads its own local mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRoot(PathBuf);

impl CheckoutRoot {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// The canonical store worktree root (its `mailbox/` is the canonical layer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreRoot(PathBuf);

impl StoreRoot {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// The canonicalized common Git directory every source must share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCommonGit(PathBuf);

impl VerifiedCommonGit {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// What the context knows about the canonical layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalCoverage {
    /// No store was located for this repository.
    NotConfigured,
    /// A verified store worktree of the same common Git directory.
    Store(StoreRoot),
    /// A store was named but could not be verified; its mail is not read.
    Refused(SourceErrorCode),
}

/// Typed facts a logical read starts from, resolved before any mail is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxContext {
    requested_checkout: CheckoutRoot,
    /// `None` for a local-only context (see [`Self::resolve_or_local`]).
    repository: Option<VerifiedCommonGit>,
    canonical: CanonicalCoverage,
}

impl MailboxContext {
    /// Verify `checkout` (a worktree root) and, independently, the store
    /// root located for it (`None` when no store exists). A store of another
    /// common Git directory is refused, not read.
    pub fn resolve(checkout: &Path, store: Option<&Path>) -> Result<Self, SourceErrorCode> {
        let checkout =
            std::fs::canonicalize(checkout).map_err(|_| SourceErrorCode::NotAGitCheckout)?;
        let repository = common_git_of(&checkout)?;
        let canonical = match store {
            None => CanonicalCoverage::NotConfigured,
            Some(store) => match std::fs::canonicalize(store) {
                Err(_) => CanonicalCoverage::Refused(SourceErrorCode::WorktreeMissing),
                Ok(store) => match common_git_of(&store) {
                    Ok(common) if common == repository => {
                        CanonicalCoverage::Store(StoreRoot(store))
                    }
                    Ok(_) => CanonicalCoverage::Refused(SourceErrorCode::CommonGitMismatch),
                    Err(code) => CanonicalCoverage::Refused(code),
                },
            },
        };
        Ok(Self {
            requested_checkout: CheckoutRoot(checkout),
            repository: Some(VerifiedCommonGit(repository)),
            canonical,
        })
    }

    /// [`Self::resolve`], except that a directory with no `.git` entry in it
    /// or any ancestor (a local-only, non-Git project) still reads its own local
    /// mailbox. Such a context has no verified repository: no sibling is
    /// inventoried, no store is read (it could be another repository's), and
    /// every snapshot reports [`SourceErrorCode::NotAGitCheckout`] as its
    /// inventory error, so it is never complete or caught up.
    // trace:STORY-1843 | ai:claude
    pub fn resolve_or_local(
        checkout: &Path,
        store: Option<&Path>,
    ) -> Result<Self, SourceErrorCode> {
        match Self::resolve(checkout, store) {
            Err(SourceErrorCode::NotAGitCheckout) if checkout.is_dir() => {
                let checkout = std::fs::canonicalize(checkout)
                    .map_err(|_| SourceErrorCode::NotAGitCheckout)?;
                // Inside a Git checkout (say a subdirectory of one) is a
                // wrong root, not a local-only project.
                if checkout
                    .ancestors()
                    .any(|dir| std::fs::symlink_metadata(dir.join(".git")).is_ok())
                {
                    return Err(SourceErrorCode::NotAGitCheckout);
                }
                Ok(Self {
                    requested_checkout: CheckoutRoot(checkout),
                    repository: None,
                    canonical: match store {
                        None => CanonicalCoverage::NotConfigured,
                        Some(_) => CanonicalCoverage::Refused(SourceErrorCode::NotAGitCheckout),
                    },
                })
            }
            other => other,
        }
    }

    pub fn requested_checkout(&self) -> &CheckoutRoot {
        &self.requested_checkout
    }

    /// The verified common Git directory; `None` for a local-only context.
    pub fn repository(&self) -> Option<&VerifiedCommonGit> {
        self.repository.as_ref()
    }

    pub fn canonical(&self) -> &CanonicalCoverage {
        &self.canonical
    }
}

/// Recipient keys a reader is authorized for, classified. Built only from
/// the output of the mailbox identity policy, never from a raw `--agent`
/// argument or an inherited environment value. There is deliberately no
/// public constructor that turns strings into recipient authority:
///
/// ```compile_fail
/// use aida_core::mailbox_logical::ResolvedAudience;
/// let _ = ResolvedAudience::from_identity_policy(vec!["someone".into()], vec![], vec![]);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedAudience {
    private: Vec<String>,
    human: Vec<String>,
    shared: Vec<String>,
    /// The `shared` keys that are roles, not vendor groups (slice D1): only
    /// role mail can carry a delivery sidecar.
    roles: Vec<String>,
    /// The verified session's recipient UUID (slice D1): the only key a
    /// role-cohort delivery is visible and acknowledged under.
    recipient: Option<RecipientUuid>,
}

impl ResolvedAudience {
    /// `private`: this session's own handle/name; `human`: intentional
    /// people-catalog inboxes; `shared`: role and vendor audiences. Each key
    /// must be a bounded mailbox identity. Crate-private: only the verified
    /// identity path may mint recipient authority.
    pub(crate) fn from_identity_policy(
        private: Vec<String>,
        human: Vec<String>,
        shared: Vec<String>,
    ) -> Result<Self, String> {
        for key in private.iter().chain(&human).chain(&shared) {
            if key.is_empty()
                || key.len() > 128
                || key.chars().any(|c| c.is_control() || c == '/' || c == '\\')
            {
                return Err("audience key is not a bounded mailbox identity".into());
            }
        }
        // Callers of this constructor name roles only; `from_facts` splits
        // the vendor group out.
        Ok(Self {
            private,
            human,
            roles: shared.clone(),
            shared,
            recipient: None,
        })
    }

    /// The audience the mailbox identity policy grants for `facts`. Keys that
    /// are not bounded mailbox identities are dropped, never widened.
    // trace:STORY-1843 | ai:claude
    pub fn from_facts(facts: &MailIdentityFacts) -> Self {
        let bounded = |key: &str| {
            !key.is_empty()
                && key.len() <= 128
                && !key.chars().any(|c| c.is_control() || c == '/' || c == '\\')
        };
        let mut private = Vec::new();
        let mut human = Vec::new();
        let mut shared = Vec::new();
        for (key, class) in classified_identities(facts) {
            if !bounded(&key) {
                continue;
            }
            match class {
                IdentityClass::Private => private.push(key),
                IdentityClass::Human => human.push(key),
                IdentityClass::Shared => shared.push(key),
            }
        }
        let mut audience = Self::from_identity_policy(private, human, shared)
            .expect("every key was checked as a bounded mailbox identity");
        audience.recipient = facts.own_recipient().cloned();
        let role = facts
            .role()
            .map(|raw| crate::mailbox_identity::effective_role(Some(raw)).0);
        audience
            .roles
            .retain(|k| role.as_deref().is_some_and(|r| r == k));
        audience
    }

    /// This session's recipient UUID, when its handle reservation names it.
    pub fn recipient(&self) -> Option<&RecipientUuid> {
        self.recipient.as_ref()
    }

    /// Whether `key` is an ACK key this audience holds: one of its keys, or
    /// its recipient UUID's receipt key.
    pub fn holds_key(&self, key: &str) -> bool {
        self.keys().any(|k| k == key)
            || self
                .recipient
                .as_ref()
                .is_some_and(|r| r.receipt_key() == key)
    }

    /// This session's own handle/names (or an unbound user).
    pub fn private_keys(&self) -> &[String] {
        &self.private
    }

    /// Intentional people-catalog inboxes.
    pub fn human_keys(&self) -> &[String] {
        &self.human
    }

    /// Role and agent-type audiences several sessions share.
    pub fn shared_keys(&self) -> &[String] {
        &self.shared
    }

    /// The shared keys that are roles (never a vendor group).
    // trace:STORY-1843 | ai:claude
    pub fn role_keys(&self) -> &[String] {
        &self.roles
    }

    /// Every key, private first.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.private
            .iter()
            .chain(&self.human)
            .chain(&self.shared)
            .map(String::as_str)
    }

    /// This audience restricted to the one key spelled `key`
    /// (case-insensitively), or `None` when `key` is not one of its keys: a
    /// selector can only narrow the authority the facts granted.
    pub fn narrowed_to(&self, key: &str) -> Option<Self> {
        let pick = |keys: &[String]| -> Vec<String> {
            keys.iter()
                .filter(|k| k.eq_ignore_ascii_case(key.trim()))
                .take(1)
                .cloned()
                .collect()
        };
        let narrowed = Self {
            private: pick(&self.private),
            human: pick(&self.human),
            shared: pick(&self.shared),
            roles: pick(&self.roles),
            recipient: self.recipient.clone(),
        };
        let any = narrowed.keys().next().is_some();
        any.then_some(narrowed)
    }

    /// Whether a private key of this audience sent `m` (its own sent mail).
    pub fn sent_by_own(&self, m: &Message) -> bool {
        self.private.iter().any(|k| *k == m.from)
    }

    /// Audience keys this message is addressed to: direct or broadcast,
    /// never the key's own sent mail, never a deleted message.
    pub fn matching_keys(&self, m: &Message) -> Vec<&str> {
        self.keys().filter(|key| m.addressed_to(key)).collect()
    }
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-178 | ai:codex
// Private policy input shared by own-reader and diagnostic paths. This type
// cannot be passed to ACK, binding, or any identity constructor.
struct PolicySubject<'a> {
    private: &'a [String],
    human: &'a [String],
    shared: &'a [String],
    roles: &'a [String],
    recipient: Option<&'a RecipientUuid>,
}
impl<'a> PolicySubject<'a> {
    fn own(a: &'a ResolvedAudience) -> Self {
        Self {
            private: &a.private,
            human: &a.human,
            shared: &a.shared,
            roles: &a.roles,
            recipient: a.recipient(),
        }
    }
    fn keys(&self) -> impl Iterator<Item = &str> {
        self.private
            .iter()
            .chain(self.human)
            .chain(self.shared)
            .map(String::as_str)
    }
    fn matching_keys(&self, m: &Message) -> Vec<&str> {
        self.keys().filter(|k| m.addressed_to(k)).collect()
    }
    fn sent_by_own(&self, m: &Message) -> bool {
        self.private.contains(&m.from)
    }
    fn role_keys(&self) -> &[String] {
        self.roles
    }
    fn recipient(&self) -> Option<&RecipientUuid> {
        self.recipient
    }
}

/// Read-only diagnostic state, deliberately separate from ACK evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectionState {
    Unread,
    Acknowledged,
    ReadLegacy,
    Pending,
}

/// Minimal routing metadata; contains no payload, delivery record, or ACK basis.
#[derive(Debug)]
pub struct InspectionRecord {
    pub id: String,
    pub to: Recipient,
    pub from: String,
    pub timestamp: i64,
    pub intent: crate::mailbox::Intent,
    pub subject: String,
    pub state: InspectionState,
}

/// Absence is conclusive only when complete and no local unknown IDs remain.
#[derive(Debug)]
pub struct MailInspection {
    pub records: Vec<InspectionRecord>,
    pub unknown_ids: Vec<String>,
    pub complete: bool,
}

/// Source coverage when there are no verified live inspection subjects.
/// Even an empty recipient directory cannot turn an unreadable source clear.
pub fn inspection_sources_complete(ctx: &MailboxContext, limits: &ReadLimits) -> bool {
    let policy = PolicySubject {
        private: &[],
        human: &[],
        shared: &[],
        roles: &[],
        recipient: None,
    };
    snapshot_for_policy(ctx, &policy, &Query::Inbox, limits)
        .0
        .complete()
}

/// Observe delivery eligibility using exactly the own-inbox policy, without
/// constructing the subject's audience or exposing its bodies or ACK plan.
pub fn inspect_mail(
    ctx: &MailboxContext,
    subject: &crate::mailbox_identity::InspectionSubject,
    limits: &ReadLimits,
) -> MailInspection {
    let private = vec![subject.handle.clone()];
    let policy = PolicySubject {
        private: &private,
        human: &[],
        shared: &subject.role,
        roles: &subject.role,
        recipient: Some(&subject.recipient),
    };
    let (snapshot, unknown_ids, pending) = snapshot_for_policy(ctx, &policy, &Query::Inbox, limits);
    let complete = snapshot.complete();
    let project = |m: &Message, state| InspectionRecord {
        id: m.id.clone(),
        to: m.to.clone(),
        from: m.from.clone(),
        timestamp: m.timestamp,
        intent: m.intent,
        subject: crate::mailbox::subject_line(m, 80),
        state,
    };
    let mut records = snapshot
        .records
        .iter()
        .filter(|r| !r.message.archived && !unknown_ids.contains(&r.message.id))
        .map(|r| {
            project(
                &r.message,
                match r.read {
                    ReadEvidence::Unread => InspectionState::Unread,
                    ReadEvidence::Acknowledged { .. } => InspectionState::Acknowledged,
                    ReadEvidence::LegacyWatermark { .. } => InspectionState::ReadLegacy,
                },
            )
        })
        .collect::<Vec<_>>();
    records.extend(pending.iter().map(|m| project(m, InspectionState::Pending)));
    MailInspection {
        records,
        unknown_ids,
        complete,
    }
}

/// What the snapshot is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    /// Every message visible to the audience.
    Inbox,
    /// One message by exact ID.
    ExactId(String),
    /// The visible members of one thread.
    Thread(String),
    /// Every message visible to the audience plus the audience's own sent
    /// mail: the set a message ID, prefix or thread reference resolves
    /// against, so every ID an inbox lists also resolves in the follow-up
    /// verbs.
    Addressable,
}

impl Query {
    /// Whether the audience's own sent mail is part of the answer. An inbox
    /// never lists the reader's own sends; a lookup by ID or thread does.
    fn includes_own_sent(&self) -> bool {
        !matches!(self, Self::Inbox)
    }
}

/// One shared budget for a whole snapshot: the inventory, every source and
/// every receipt lookup draw from the same deadline; `max_entries` counts
/// directory entries across all sources together, and `max_total_bytes`
/// counts inventory output, message files and receipts together.
/// [`MailboxContext::resolve`] runs before the budget starts; it is a fixed
/// number of small-file reads (no listing, no git process).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimits {
    pub deadline: Instant,
    pub max_roots: usize,
    pub max_entries: usize,
    pub max_message_bytes: u64,
    pub max_total_bytes: u64,
}

impl ReadLimits {
    /// The bounded foreground profile for an explicit read (2 s). Passive
    /// callers pass their own shorter deadline.
    pub fn explicit_read() -> Self {
        Self::with_deadline(Instant::now() + Duration::from_secs(2))
    }

    pub fn with_deadline(deadline: Instant) -> Self {
        Self {
            deadline,
            max_roots: 512,
            max_entries: 16_384,
            max_message_bytes: 1024 * 1024,
            max_total_bytes: 32 * 1024 * 1024,
        }
    }
}

/// Where a source lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    /// A product worktree's `.aida/mailbox/`.
    Local(PathBuf),
    /// The store worktree's `mailbox/`, read once per snapshot.
    Canonical(PathBuf),
}

/// How one source was observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceStatus {
    /// Fully read; `messages` files parsed.
    Observed { messages: usize },
    /// The source exists but has no mailbox yet (a successful observation).
    Absent,
    /// Could not be fully observed; its mail may be missing from the view.
    Error(SourceErrorCode),
}

/// The pinned mailbox directory a source was read from, and its observation
/// cut. This is current-source identity only, not a retention generation
/// (signed slice C): an ACK plan refuses when the directory was replaced
/// (`dev`/`ino`) or a planned message's bytes changed since the cut; an
/// unrelated arrival (which moves `mtime_ns`) does not refuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceIdentity {
    pub dev: u64,
    pub ino: u64,
    /// Directory mtime in nanoseconds, taken before the listing.
    pub mtime_ns: i128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReport {
    pub kind: SourceKind,
    pub status: SourceStatus,
    /// `Some` when the mailbox directory was pinned and fully read.
    pub identity: Option<SourceIdentity>,
}

/// Where one replica of a record was read: the index into
/// [`LogicalSnapshot::sources`] and the SHA-256 of its raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub source: usize,
    pub fingerprint: String,
}

/// Where an explicit per-ID receipt was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckOrigin {
    /// A local source's `.read-receipts/` (index into
    /// [`LogicalSnapshot::sources`]).
    Source(usize),
    /// The private receipt ledger under the verified common Git directory
    /// (slice B2c). It outlives every source, so removing a worktree never
    /// resurrects mail acknowledged there. `legacy`: the ACK resolved mail
    /// covered only by a legacy watermark, so its time is when it was
    /// acknowledged, not a read latency (TASK-1271).
    Anchor { legacy: bool },
}

/// Read evidence for one visible record, for the key it is addressed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadEvidence {
    /// An explicit per-ID receipt in a local source or the receipt anchor.
    Acknowledged {
        origin: AckOrigin,
        key: String,
        at: i64,
    },
    /// Covered only by a legacy timestamp watermark in a local source that
    /// physically holds this ID. Compatibility evidence: not proof the ID was
    /// present when the watermark was set, and carries no read time.
    LegacyWatermark { source: usize, key: String },
    /// No evidence of a read.
    Unread,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRecord {
    /// The reconciled message. A retracted record carries normal redaction:
    /// its body and subject are cleared before it leaves the snapshot.
    pub message: Message,
    pub provenance: Vec<Provenance>,
    pub read: ReadEvidence,
    /// Set when the message is a role-cohort delivery owned by this
    /// audience's recipient UUID (slice D1): it is read and acknowledged
    /// under that UUID's receipt key only.
    pub delivered_to: Option<RecipientUuid>,
    /// The delivery record `delivered_to` owns (slice D2b): the original,
    /// a queued delivery bound to it, or a handover.
    pub owned_delivery: Option<DeliveryRecord>,
}

/// A queued role delivery that this audience's reader is eligible to bind
/// (slice D1). Its message stays hidden until the binding exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDelivery {
    pub message_id: String,
    pub delivery_id: String,
}

/// The ACK keys of `record` for `audience`: its recipient UUID's receipt
/// key for a cohort delivery, else the audience keys it is addressed to.
// trace:STORY-1843 | ai:claude
pub fn receipt_keys(audience: &ResolvedAudience, record: &SnapshotRecord) -> Vec<String> {
    match &record.delivered_to {
        Some(uuid) => vec![uuid.receipt_key()],
        None => audience
            .matching_keys(&record.message)
            .into_iter()
            .map(str::to_string)
            .collect(),
    }
}

// ── Relay run inspection (slice D2b) ────────────────────────────────────────

/// The audience of one relay run: its label as the private key and its run
/// UUID as the recipient. Crate-private, like every audience constructor:
/// only the inspections below use it, and they expose counts or the
/// run's own undelivered deliveries, never another reader's view.
// trace:STORY-1843 | ai:claude
fn relay_run_audience(label: &str, run: &RecipientUuid) -> Option<ResolvedAudience> {
    let mut audience =
        ResolvedAudience::from_identity_policy(vec![label.to_string()], Vec::new(), Vec::new())
            .ok()?;
    audience.recipient = Some(run.clone());
    Some(audience)
}

/// Unread mail of one relay run, as counts only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayRunUnread {
    /// Deliveries the run owns (its own, and handovers to it) with no read.
    pub owned: usize,
    /// Mail addressed to the label with no delivery record (written before
    /// relay ownership, or by a writer that bypasses it): readable by
    /// whoever holds the label, so it is never handed over.
    pub legacy_label: usize,
    /// Every source and receipt was fully observed.
    pub complete: bool,
}

impl RelayRunUnread {
    /// Whether a caller that cannot choose must refuse: some unread mail,
    /// or a count it cannot trust.
    pub fn any_or_unknown(&self) -> bool {
        !self.complete || self.owned > 0 || self.legacy_label > 0
    }
}

/// One undelivered delivery of a relay run, for its end notices and its
/// handover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayRunDelivery {
    pub message_id: String,
    pub thread_id: String,
    pub delivery_id: String,
    pub from: String,
    pub subject: Option<String>,
    /// This delivery is itself a relay end notice.
    pub notice: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayRunDeliveries {
    pub unread: Vec<RelayRunDelivery>,
    pub legacy_label: usize,
    pub complete: bool,
    /// The receipt anchor could not be read, so mail already read may be
    /// listed in `unread`; `complete` is then false. Nothing may be told
    /// or handed over from such a list.
    // trace:STORY-1843 | ai:claude
    pub receipts_unknown: bool,
}

fn unread_record(record: &SnapshotRecord) -> bool {
    !record.message.archived && !matches!(record.read, ReadEvidence::Acknowledged { .. })
}

/// The undelivered deliveries of relay run `run` (label `label`): the
/// mail its sessions never read. A legacy watermark is not a read here.
// trace:STORY-1843 | ai:claude
pub fn relay_run_unread_deliveries(
    ctx: &MailboxContext,
    label: &str,
    run: &RecipientUuid,
    limits: &ReadLimits,
) -> RelayRunDeliveries {
    let Some(audience) = relay_run_audience(label, run) else {
        return RelayRunDeliveries::default();
    };
    let snapshot = logical_snapshot(ctx, &audience, &Query::Inbox, limits);
    // D1's unsighted rule (ruling 97), not the acknowledgement standard: a
    // missing or prunable sibling worktree never makes the count unknown.
    // An unreadable receipt anchor does: read mail would look unread
    // (advisor ruling on TASK-...-077). trace:STORY-1843 | ai:claude
    let receipts_unknown = snapshot.anchor_error.is_some();
    let mut out = RelayRunDeliveries {
        complete: snapshot.live_sources_observed() && !receipts_unknown,
        receipts_unknown,
        ..Default::default()
    };
    for record in snapshot.records.iter().filter(|r| unread_record(r)) {
        match &record.owned_delivery {
            Some(delivery) => out.unread.push(RelayRunDelivery {
                message_id: record.message.id.clone(),
                thread_id: record.message.thread_id.clone(),
                delivery_id: delivery.delivery_id.clone(),
                from: record.message.from.clone(),
                subject: record.message.subject.clone(),
                notice: delivery.notice_of.is_some(),
            }),
            None => out.legacy_label += 1,
        }
    }
    out
}

/// How much unread mail relay run `run` (label `label`) holds. Counts
/// only: no IDs, senders or bodies.
pub fn relay_run_unread(
    ctx: &MailboxContext,
    label: &str,
    run: &RecipientUuid,
    limits: &ReadLimits,
) -> RelayRunUnread {
    let found = relay_run_unread_deliveries(ctx, label, run, limits);
    RelayRunUnread {
        owned: found.unread.len(),
        legacy_label: found.legacy_label,
        complete: found.complete,
    }
}

/// Retention of mail from retired worktrees. Always unestablished until the
/// retention barrier (signed slice C) exists: a removed worktree's mail is
/// not covered by this snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionCoverage {
    Unestablished,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalSnapshot {
    /// Visible, reconciled records, oldest first.
    pub records: Vec<SnapshotRecord>,
    /// IDs of visible messages whose replicas disagree; never projected.
    pub conflicts: Vec<String>,
    pub sources: Vec<SourceReport>,
    /// Wall-clock epoch millis when the snapshot started.
    pub observed_at_ms: i64,
    pub inventory_error: Option<SourceErrorCode>,
    pub canonical: CanonicalCoverage,
    pub retention: RetentionCoverage,
    /// For [`Query::ExactId`]: `Some(false)` only when coverage is complete.
    pub exact_found: Option<bool>,
    /// The receipt anchor could not be read for some key: acknowledgements
    /// recorded there may be missing, so the view is partial.
    pub anchor_error: Option<SourceErrorCode>,
    /// Queued role deliveries this reader may bind (slice D1).
    pub pending_deliveries: Vec<PendingDelivery>,
    /// Diagnostic-only local unknowns. These preserve the established own-reader
    /// complete/ACK contract; inspection must not treat them as cleared mail.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-178 | ai:codex
    pub inspection_unknown_ids: Vec<String>,
}

impl LogicalSnapshot {
    /// Every inventoried source, the canonical layer (when configured) and
    /// every receipt lookup was fully observed.
    pub fn complete(&self) -> bool {
        self.inventory_error.is_none()
            && self.anchor_error.is_none()
            && !matches!(self.canonical, CanonicalCoverage::Refused(_))
            && self
                .sources
                .iter()
                .all(|s| !matches!(s.status, SourceStatus::Error(_)))
            && self.conflicts.is_empty()
    }

    /// Records with no read evidence (archived records excluded).
    pub fn unread_ids(&self) -> Vec<&str> {
        self.ids_where(|read| *read == ReadEvidence::Unread)
    }

    /// Records covered only by a legacy watermark (archived records
    /// excluded): neither unread nor acknowledged. An adapter decides how to
    /// render them; it must not count them as read.
    pub fn legacy_watermark_ids(&self) -> Vec<&str> {
        self.ids_where(|read| matches!(read, ReadEvidence::LegacyWatermark { .. }))
    }

    fn ids_where(&self, keep: impl Fn(&ReadEvidence) -> bool) -> Vec<&str> {
        self.records
            .iter()
            .filter(|r| !r.message.archived && keep(&r.read))
            .map(|r| r.message.id.as_str())
            .collect()
    }

    /// `Some(true)` only when coverage is complete and every non-archived
    /// record has an explicit per-ID receipt; `None` when any source is
    /// partial — never "empty means caught up". A legacy watermark alone is
    /// unread-compatible, not proof of a read, so it keeps this `false`:
    /// on a complete snapshot, `Some(false)` holds exactly when
    /// [`Self::unread_ids`] or [`Self::legacy_watermark_ids`] is non-empty.
    // trace:STORY-1843 | ai:claude
    pub fn caught_up(&self) -> Option<bool> {
        self.complete().then(|| {
            self.records
                .iter()
                .all(|r| r.message.archived || matches!(r.read, ReadEvidence::Acknowledged { .. }))
        })
    }

    /// Whether an explicit ACK may be planned from this snapshot: every
    /// source and the receipt anchor were fully observed and no replicas
    /// disagree. A local-only (non-Git) project, whose one source is all
    /// there is, also qualifies. A partial view never advances an
    /// acknowledgement (signoff 48 B): an unread sibling may hold a newer
    /// state of a displayed message.
    // trace:STORY-1843 | ai:claude
    pub fn ack_eligible(&self) -> bool {
        let local_only = self.inventory_error == Some(SourceErrorCode::NotAGitCheckout);
        (self.inventory_error.is_none() || local_only)
            && self.anchor_error.is_none()
            && match self.canonical {
                CanonicalCoverage::Refused(SourceErrorCode::NotAGitCheckout) => local_only,
                CanonicalCoverage::Refused(_) => false,
                _ => true,
            }
            && self
                .sources
                .iter()
                .all(|s| !matches!(s.status, SourceStatus::Error(_)))
            && self.conflicts.is_empty()
    }

    /// Whether every live source was read: the inventory was complete (a
    /// local-only project's one mailbox qualifies), no present worktree's
    /// read failed, and no replicas disagree. Unlike [`Self::ack_eligible`],
    /// a missing or prunable worktree, another repository's worktree, a
    /// refused canonical store and a receipt-anchor error do not count
    /// (ruling 97, the same sources D1's fail-closed gate ignores).
    // trace:STORY-1843 | ai:claude
    pub fn live_sources_observed(&self) -> bool {
        self.inventory_error
            .is_none_or(|code| code == SourceErrorCode::NotAGitCheckout)
            && self.conflicts.is_empty()
            && self.sources.iter().all(|s| match (&s.kind, &s.status) {
                (SourceKind::Local(_), SourceStatus::Error(code)) => matches!(
                    code,
                    SourceErrorCode::WorktreeMissing
                        | SourceErrorCode::WorktreePrunable
                        | SourceErrorCode::CommonGitMismatch
                ),
                _ => true,
            })
    }
}

/// Take a pure-inspection snapshot for `audience`. Never writes.
pub fn logical_snapshot(
    ctx: &MailboxContext,
    audience: &ResolvedAudience,
    query: &Query,
    limits: &ReadLimits,
) -> LogicalSnapshot {
    snapshot_for_policy(ctx, &PolicySubject::own(audience), query, limits).0
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-178 | ai:codex
fn snapshot_for_policy(
    ctx: &MailboxContext,
    audience: &PolicySubject<'_>,
    query: &Query,
    limits: &ReadLimits,
) -> (LogicalSnapshot, Vec<String>, Vec<Message>) {
    let observed_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let mut budget = Budget::new(limits);
    let (roots, inventory_error) = inventory(ctx, &mut budget);
    let mut sources = Vec::new();
    let mut observed: Vec<Option<ObservedSource>> = Vec::new();
    // A live source whose `.deliveries/` went unread may hold the sidecar of
    // any role message, since a message carries no origin (ruling F2,
    // narrowed): a present worktree whose read failed, one inventory could
    // not resolve (`common_git_of` failed), or any worktree an inventory
    // that could not enumerate them all left out. A missing or prunable
    // worktree's files are gone or retained (the retained lookup covers
    // them), another repository's worktree is not a source, and the
    // canonical store never carries sidecars (F5), so none of those counts.
    // trace:STORY-1843 | ai:claude
    let mut unsighted =
        inventory_error.is_some_and(|code| code != SourceErrorCode::NotAGitCheckout);
    let mut salvaged: Vec<DeliverySidecar> = Vec::new();
    for root in roots {
        let (path, result) = match root {
            Err((path, code)) => {
                unsighted |= !matches!(
                    code,
                    SourceErrorCode::WorktreeMissing
                        | SourceErrorCode::WorktreePrunable
                        | SourceErrorCode::CommonGitMismatch
                );
                (path, Err(code))
            }
            Ok(path) => {
                let result = read_source(&path, &[".aida", "mailbox"], &mut budget);
                if result.is_err() {
                    match read_source_deliveries(&path, &[".aida", "mailbox"], &mut budget) {
                        Ok(sidecars) => salvaged.extend(sidecars),
                        Err(_) => unsighted = true,
                    }
                }
                (path, result)
            }
        };
        let (status, identity, obs) = observation(result);
        sources.push(SourceReport {
            kind: SourceKind::Local(path),
            status,
            identity,
        });
        observed.push(obs);
    }
    let local_count = sources.len();
    // The canonical layer is read exactly once, however many worktrees
    // carry a `.aida-store` alias to it.
    if let CanonicalCoverage::Store(store) = &ctx.canonical {
        let (status, identity, obs) =
            observation(read_source(store.path(), &["mailbox"], &mut budget));
        sources.push(SourceReport {
            kind: SourceKind::Canonical(store.path().to_path_buf()),
            status,
            identity,
        });
        observed.push(obs);
    }
    settle_unstamped_timestamps(&mut observed, local_count);

    let in_query = |id: &str, thread: &str| match query {
        Query::Inbox | Query::Addressable => true,
        Query::ExactId(want) => id == want,
        Query::Thread(want) => thread == want,
    };
    let mut anchor = AnchorEvidence::new(ctx.repository.as_ref().map(|r| r.path()));
    let (delivered, pending_deliveries, delivery_conflicts, mut unknown_ids) = delivery_gates(
        ctx,
        &observed,
        salvaged,
        unsighted,
        audience,
        &mut anchor,
        &mut budget,
    );
    let (mut records, mut conflicts) = reconcile(
        &observed,
        local_count,
        audience,
        query.includes_own_sent(),
        &delivered,
    );
    conflicts.extend(delivery_conflicts);
    records.retain(|(m, _)| in_query(&m.id, &m.thread_id));
    let conflicts = conflicts
        .into_iter()
        .filter(|(id, threads)| threads.iter().any(|t| in_query(id, t)))
        .map(|(id, _)| id)
        .collect();

    let pending_ids: HashSet<&str> = pending_deliveries
        .iter()
        .map(|p| p.message_id.as_str())
        .collect();
    let pending_gates = delivered
        .iter()
        .filter(|(id, _)| !pending_ids.contains(id.as_str()))
        .map(|(id, gate)| (id.clone(), gate.clone()))
        .collect();
    let (pending_records, _) = if pending_ids.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        reconcile(&observed, local_count, audience, false, &pending_gates)
    };
    let pending_messages = pending_records
        .into_iter()
        .filter(|(m, _)| pending_ids.contains(m.id.as_str()) && !m.archived)
        .map(|(m, _)| m)
        .collect();
    let mut receipts = ReceiptCache::default();
    let mut out_records = Vec::with_capacity(records.len());
    for (mut message, provenance) in records {
        let delivered_to = match delivered.get(&message.id) {
            Some(DeliveryGate::Owned(uuid, _)) if !audience.matching_keys(&message).is_empty() => {
                Some(uuid.clone())
            }
            _ => None,
        };
        let owned_delivery = match (&delivered_to, delivered.get(&message.id)) {
            (Some(_), Some(DeliveryGate::Owned(_, record))) => Some((**record).clone()),
            _ => None,
        };
        let keys: Vec<String> = match &delivered_to {
            Some(uuid) => vec![uuid.receipt_key()],
            None => audience
                .matching_keys(&message)
                .into_iter()
                .map(str::to_string)
                .collect(),
        };
        let mut receipt_unknown = false;
        let read = read_evidence(
            &message,
            &keys,
            &provenance,
            &observed,
            local_count,
            &mut sources,
            &mut receipts,
            &mut anchor,
            &mut budget,
            &mut receipt_unknown,
        );
        if receipt_unknown && !matches!(read, ReadEvidence::Acknowledged { .. }) {
            unknown_ids.push(message.id.clone());
        }
        if message.retracted {
            message.body.clear();
            message.subject = None;
        }
        out_records.push(SnapshotRecord {
            message,
            provenance,
            read,
            delivered_to,
            owned_delivery,
        });
    }

    let mut snapshot = LogicalSnapshot {
        records: out_records,
        conflicts,
        sources,
        observed_at_ms,
        inventory_error,
        canonical: ctx.canonical.clone(),
        retention: RetentionCoverage::Unestablished,
        exact_found: None,
        anchor_error: anchor.error,
        inspection_unknown_ids: unknown_ids.clone(),
        pending_deliveries: pending_deliveries
            .into_iter()
            .filter(|_| matches!(query, Query::Inbox | Query::Addressable))
            .collect(),
    };
    if let Query::ExactId(id) = query {
        let found = snapshot.records.iter().any(|r| &r.message.id == id)
            || snapshot.conflicts.iter().any(|c| c == id);
        snapshot.exact_found = (found || snapshot.complete()).then_some(found);
    }
    (snapshot, unknown_ids, pending_messages)
}

/// The outcome of one exact per-ID receipt lookup across every inventoried
/// LOCAL source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptLookup {
    /// Some local source holds a receipt; `at` is its read time (epoch ms).
    Read { at: i64 },
    /// The receipt anchor holds an explicit ACK that resolved a legacy
    /// watermark: read, at an unknown time.
    ReadUntimed,
    /// Every source answered and none holds a receipt.
    NotFound,
    /// At least one source could not answer (a worktree the inventory could
    /// not establish, an unreadable or corrupt receipt, the deadline): a typed
    /// partial, never "no receipt".
    Unknown,
}

/// Exact per-ID read-receipt lookups in every product worktree of a
/// context, for the fleet-wide counters that count every recipient's unread
/// mail (night-shift latency, maintenance predicates, launch context,
/// dead-letter list, operator inspection). It exposes which IDs were read,
/// never message content, so it needs no audience; it is the same receipt
/// evidence [`logical_snapshot`] treats as acknowledgement.
///
/// No receipt directory is ever listed: a lookup opens one receipt name per
/// source (each source's per-key directory is opened once and cached), so
/// its cost is bounded by the inventoried root count and is not charged to
/// a directory-listing budget. Never writes.
pub struct ReceiptInspector {
    /// One slot per inventoried root; `None` for a root the inventory could
    /// not establish (every lookup that would need it is `Unknown`).
    roots: Vec<Option<PathBuf>>,
    inventory_complete: bool,
    deadline: Instant,
    key_dirs: RefCell<HashMap<(usize, String), KeyDir>>,
    /// The verified common Git directory whose receipt anchor is consulted
    /// first; `None` for a local-only context.
    anchor: Option<PathBuf>,
    /// Each key's anchor ledger, read once; `None` when it could not be read
    /// (or the deadline or byte budget ran out first).
    ledgers: RefCell<HashMap<String, Option<ReceiptLedger>>>,
    /// Ledger bytes still allowed: what the inventory left of
    /// `limits.max_total_bytes`, shared by every key's ledger read.
    ledger_bytes_left: Cell<u64>,
    partial: Cell<bool>,
}

/// One source's receipt directory for one key.
enum KeyDir {
    Dir(PinnedDir),
    /// Absent, or an unusable entry (a regular file or symlink where the
    /// directory should be), which can hold no receipt and is skipped.
    Absent,
    /// The directory exists but could not be opened safely.
    Unknown,
}

impl ReceiptInspector {
    /// Inventory `ctx`'s product worktrees. Only the inventory draws from
    /// `limits`' listing budget; `limits.deadline` also bounds every later
    /// lookup.
    // trace:STORY-1843 | ai:claude
    pub fn new(ctx: &MailboxContext, limits: &ReadLimits) -> Self {
        let mut budget = Budget::new(limits);
        let (roots, inventory_error) = inventory(ctx, &mut budget);
        // A local-only context has no siblings to inventory: its one root
        // answers for the whole project.
        let inventory_complete = match inventory_error {
            None => true,
            Some(SourceErrorCode::NotAGitCheckout) => ctx.repository.is_none(),
            Some(_) => false,
        };
        Self {
            roots: roots.into_iter().map(Result::ok).collect(),
            inventory_complete,
            deadline: limits.deadline,
            key_dirs: RefCell::default(),
            anchor: ctx.repository.as_ref().map(|r| r.path().to_path_buf()),
            ledgers: RefCell::default(),
            ledger_bytes_left: Cell::new(budget.bytes_left),
            partial: Cell::new(!inventory_complete),
        }
    }

    /// An inspector for a context that could not be resolved: every lookup
    /// is `Unknown`.
    pub fn unavailable() -> Self {
        Self {
            roots: Vec::new(),
            inventory_complete: false,
            deadline: Instant::now(),
            key_dirs: RefCell::default(),
            anchor: None,
            ledgers: RefCell::default(),
            ledger_bytes_left: Cell::new(0),
            partial: Cell::new(true),
        }
    }

    /// Whether some lookup (or the inventory) could not be answered fully.
    pub fn partial(&self) -> bool {
        self.partial.get()
    }

    /// Look up `key`'s receipt for message `id` in every source. A receipt
    /// anywhere is `Read`; otherwise any source that could not answer makes
    /// it `Unknown` (and the inspector partial).
    // trace:STORY-1843 | ai:claude
    pub fn lookup(&self, key: &str, id: &str) -> ReceiptLookup {
        if !safe_id(id) {
            // Writers never name a receipt after an unsafe ID.
            return ReceiptLookup::NotFound;
        }
        // The receipt anchor first (slice B2c): it outlives every source.
        // An unreadable ledger, or one the deadline or byte budget (signoff
        // 60 #6) left unread, leaves the answer to the local sources, but
        // their silence is then Unknown, never NotFound.
        let mut anchor_unknown = false;
        if let Some(common) = &self.anchor {
            let mut ledgers = self.ledgers.borrow_mut();
            let ledger = ledgers.entry(key.to_string()).or_insert_with(|| {
                if Instant::now() >= self.deadline {
                    return None;
                }
                let (ledger, len) = read_ledger_sized(common, key).ok()?.unwrap_or_default();
                let left = self.ledger_bytes_left.get().checked_sub(len as u64)?;
                self.ledger_bytes_left.set(left);
                Some(ledger)
            });
            match ledger {
                Some(ledger) => {
                    if let Some(at) = ledger.receipts.get(id) {
                        return if ledger.legacy.contains(id) {
                            ReceiptLookup::ReadUntimed
                        } else {
                            ReceiptLookup::Read { at: *at }
                        };
                    }
                }
                None => anchor_unknown = true,
            }
        }
        let key = sanitize_key(key);
        let name = format!("{id}.txt");
        let mut unknown = anchor_unknown || !self.inventory_complete;
        let mut key_dirs = self.key_dirs.borrow_mut();
        for (index, root) in self.roots.iter().enumerate() {
            let Some(root) = root else {
                unknown = true;
                continue;
            };
            if Instant::now() >= self.deadline {
                unknown = true;
                break;
            }
            let dir = key_dirs
                .entry((index, key.clone()))
                .or_insert_with(|| open_key_dir(root, &key));
            let dir = match dir {
                KeyDir::Dir(dir) => dir,
                KeyDir::Absent => continue,
                KeyDir::Unknown => {
                    unknown = true;
                    continue;
                }
            };
            match dir.read_file(OsStr::new(&name), SMALL_FILE) {
                Ok(None) => {}
                Ok(Some(bytes)) => match parse_millis(&bytes) {
                    Ok(at) => return ReceiptLookup::Read { at },
                    Err(_) => unknown = true,
                },
                // An unsafe receipt file (a symlink, or hard-linked) is
                // evidence that could not be read, the same as in
                // `logical_snapshot`: Unknown, never NotFound.
                Err(_) => unknown = true,
            }
        }
        if unknown {
            self.partial.set(true);
            ReceiptLookup::Unknown
        } else {
            ReceiptLookup::NotFound
        }
    }
}

/// An unusable entry on the path to a key's receipt directory (a regular
/// file where a directory belongs, or a symlink) cannot hold a receipt and
/// is skipped, never followed. Receipt files themselves are never skipped.
fn skippable(code: SourceErrorCode) -> bool {
    matches!(
        code,
        SourceErrorCode::LayoutUnsupported | SourceErrorCode::UnsafeEntry
    )
}

fn open_key_dir(root: &Path, key: &str) -> KeyDir {
    let Ok(mut dir) = PinnedDir::open_root(root) else {
        return KeyDir::Unknown;
    };
    for component in [".aida", "mailbox", ".read-receipts", key] {
        match dir.child_dir(component) {
            Ok(Some(child)) => dir = child,
            Ok(None) => return KeyDir::Absent,
            Err(code) if skippable(code) => return KeyDir::Absent,
            Err(_) => return KeyDir::Unknown,
        }
    }
    KeyDir::Dir(dir)
}

fn observation(
    result: Result<Option<ObservedSource>, SourceErrorCode>,
) -> (SourceStatus, Option<SourceIdentity>, Option<ObservedSource>) {
    match result {
        Ok(Some(obs)) => (
            SourceStatus::Observed {
                messages: obs.messages.len(),
            },
            Some(obs.identity),
            Some(obs),
        ),
        Ok(None) => (SourceStatus::Absent, None, None),
        Err(code) => (SourceStatus::Error(code), None, None),
    }
}

// ── Budget ───────────────────────────────────────────────────────────────────

struct Budget {
    deadline: Instant,
    max_roots: usize,
    entries_left: usize,
    max_message_bytes: u64,
    bytes_left: u64,
}

impl Budget {
    fn new(limits: &ReadLimits) -> Self {
        Self {
            deadline: limits.deadline,
            max_roots: limits.max_roots,
            entries_left: limits.max_entries,
            max_message_bytes: limits.max_message_bytes,
            bytes_left: limits.max_total_bytes,
        }
    }

    fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// Draw one directory entry from the snapshot-wide entry budget.
    fn take_entry(&mut self) -> Result<(), SourceErrorCode> {
        self.entries_left = self
            .entries_left
            .checked_sub(1)
            .ok_or(SourceErrorCode::OverBudget)?;
        Ok(())
    }

    fn take(&mut self, bytes: usize) -> Result<(), SourceErrorCode> {
        self.bytes_left = self
            .bytes_left
            .checked_sub(bytes as u64)
            .ok_or(SourceErrorCode::OverBudget)?;
        Ok(())
    }
}

// ── Git identity and inventory ───────────────────────────────────────────────

fn read_small(path: &Path) -> Option<String> {
    let mut s = String::new();
    File::open(path)
        .ok()?
        .take(SMALL_FILE)
        .read_to_string(&mut s)
        .ok()?;
    Some(s.trim().to_string())
}

/// The canonical common Git directory of worktree root `root`, verified from
/// its `.git` pointer and the admin directory's back-pointer. No git process.
fn common_git_of(root: &Path) -> Result<PathBuf, SourceErrorCode> {
    let dotgit = root.join(".git");
    let meta = std::fs::symlink_metadata(&dotgit).map_err(|_| SourceErrorCode::NotAGitCheckout)?;
    if meta.is_dir() {
        return std::fs::canonicalize(&dotgit).map_err(|_| SourceErrorCode::NotAGitCheckout);
    }
    if !meta.is_file() {
        return Err(SourceErrorCode::NotAGitCheckout);
    }
    let pointer = read_small(&dotgit).ok_or(SourceErrorCode::NotAGitCheckout)?;
    let admin = pointer
        .strip_prefix("gitdir:")
        .map(|p| root.join(p.trim()))
        .ok_or(SourceErrorCode::NotAGitCheckout)?;
    let admin = std::fs::canonicalize(admin).map_err(|_| SourceErrorCode::CommonGitMismatch)?;
    // The admin directory must point back at this worktree, so a copied or
    // forged `.git` file cannot borrow another repository's identity.
    let back = read_small(&admin.join("gitdir")).ok_or(SourceErrorCode::CommonGitMismatch)?;
    let back_ok = std::fs::canonicalize(admin.join(&back))
        .ok()
        .zip(std::fs::canonicalize(&dotgit).ok())
        .is_some_and(|(a, b)| a == b);
    if !back_ok {
        return Err(SourceErrorCode::CommonGitMismatch);
    }
    let common = read_small(&admin.join("commondir")).ok_or(SourceErrorCode::CommonGitMismatch)?;
    std::fs::canonicalize(admin.join(common)).map_err(|_| SourceErrorCode::CommonGitMismatch)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct WorktreeRecord {
    path: PathBuf,
    branch: Option<String>,
    bare: bool,
    prunable: bool,
}

/// Parse `git worktree list --porcelain -z`: NUL-terminated fields, records
/// ended by an empty field. A record without a `worktree` line is an error.
fn parse_worktree_porcelain_z(bytes: &[u8]) -> Result<Vec<WorktreeRecord>, SourceErrorCode> {
    let mut out = Vec::new();
    let mut current: Option<WorktreeRecord> = None;
    let mut fields = bytes.split(|b| *b == 0);
    while let Some(field) = fields.next() {
        if field.is_empty() {
            if let Some(rec) = current.take() {
                out.push(rec);
            }
            continue;
        }
        let (key, value) = match field.iter().position(|b| *b == b' ') {
            Some(i) => (&field[..i], &field[i + 1..]),
            None => (field, &field[field.len()..]),
        };
        if key == b"worktree" {
            if current.is_some() {
                return Err(SourceErrorCode::InventoryFailed);
            }
            current = Some(WorktreeRecord {
                path: PathBuf::from(OsStr::from_bytes(value)),
                ..Default::default()
            });
            continue;
        }
        let rec = current.as_mut().ok_or(SourceErrorCode::InventoryFailed)?;
        match key {
            b"branch" => rec.branch = Some(String::from_utf8_lossy(value).into_owned()),
            b"bare" => rec.bare = true,
            b"prunable" => rec.prunable = true,
            _ => {}
        }
    }
    if let Some(rec) = current {
        out.push(rec);
    }
    Ok(out)
}

/// Bounded `git worktree list --porcelain -z` from `checkout`.
fn list_worktrees(checkout: &Path, budget: &mut Budget) -> Result<Vec<u8>, SourceErrorCode> {
    let mut child = crate::git_ops::host_git_command(checkout)
        .args(["worktree", "list", "--porcelain", "-z"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| SourceErrorCode::InventoryFailed)?;
    let stdout = child
        .stdout
        .take()
        .ok_or(SourceErrorCode::InventoryFailed)?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout
            .take(INVENTORY_BYTES as u64 + 1)
            .read_to_end(&mut buf);
        buf
    });
    loop {
        if budget.expired() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SourceErrorCode::InventoryOverBudget);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader
                    .join()
                    .map_err(|_| SourceErrorCode::InventoryFailed)?;
                if out.len() > INVENTORY_BYTES || budget.take(out.len()).is_err() {
                    return Err(SourceErrorCode::InventoryOverBudget);
                }
                return if status.success() {
                    Ok(out)
                } else {
                    Err(SourceErrorCode::InventoryFailed)
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => return Err(SourceErrorCode::InventoryFailed),
        }
    }
}

type RootEntry = Result<PathBuf, (PathBuf, SourceErrorCode)>;

/// The requested checkout first, then every other registered product
/// worktree of the same common Git directory. The store worktree and bare
/// entries are not product sources; aliases of one directory appear once.
fn inventory(
    ctx: &MailboxContext,
    budget: &mut Budget,
) -> (Vec<RootEntry>, Option<SourceErrorCode>) {
    let requested = ctx.requested_checkout.path().to_path_buf();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    if let Ok(meta) = std::fs::metadata(&requested) {
        seen.insert((meta.dev(), meta.ino()));
    }
    let store = match &ctx.canonical {
        CanonicalCoverage::Store(s) => Some(s.path().to_path_buf()),
        _ => None,
    };
    let mut roots = vec![Ok(requested.clone())];
    let Some(repository) = &ctx.repository else {
        return (roots, Some(SourceErrorCode::NotAGitCheckout));
    };
    let records =
        match list_worktrees(&requested, budget).and_then(|b| parse_worktree_porcelain_z(&b)) {
            Ok(records) => records,
            Err(code) => return (roots, Some(code)),
        };
    let mut inventory_error = None;
    for rec in records {
        if rec.bare || rec.branch.as_deref() == Some(STORE_BRANCH) {
            continue;
        }
        // Only a path that is not there is missing. A present worktree that
        // cannot be resolved (EACCES, ELOOP, ...) and that git does not
        // report prunable is unreadable, so its `.deliveries/` went unread
        // and the F2 gate fails closed (advisor ruling).
        // trace:STORY-1843 | ai:claude
        let (canonical, unresolvable) = match std::fs::canonicalize(&rec.path) {
            Ok(path) => (Some(path), false),
            Err(e) => (None, e.kind() != std::io::ErrorKind::NotFound),
        };
        if canonical.is_some() && canonical == store {
            continue;
        }
        if canonical.as_ref() == Some(&requested) {
            continue;
        }
        if roots.len() >= budget.max_roots {
            inventory_error = Some(SourceErrorCode::InventoryOverBudget);
            break;
        }
        let Some(path) = canonical else {
            let code = if rec.prunable {
                SourceErrorCode::WorktreePrunable
            } else if unresolvable {
                SourceErrorCode::Unreadable
            } else {
                SourceErrorCode::WorktreeMissing
            };
            roots.push(Err((rec.path, code)));
            continue;
        };
        if rec.prunable {
            roots.push(Err((path, SourceErrorCode::WorktreePrunable)));
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            if !seen.insert((meta.dev(), meta.ino())) {
                continue;
            }
        }
        match common_git_of(&path) {
            Ok(common) if common == *repository.path() => roots.push(Ok(path)),
            Ok(_) => roots.push(Err((path, SourceErrorCode::CommonGitMismatch))),
            Err(code) => roots.push(Err((path, code))),
        }
    }
    (roots, inventory_error)
}

// ── Pinned, no-follow, bounded source reads ──────────────────────────────────

/// A directory opened without following symlinks; children are opened
/// relative to its descriptor so a swapped ancestor cannot redirect reads.
struct PinnedDir {
    fd: OwnedFd,
}

fn errno_code(e: &std::io::Error) -> SourceErrorCode {
    match e.raw_os_error() {
        Some(libc::ELOOP) | Some(libc::EMLINK) => SourceErrorCode::UnsafeEntry,
        Some(libc::ENOTDIR) => SourceErrorCode::LayoutUnsupported,
        Some(libc::EACCES) | Some(libc::EPERM) => SourceErrorCode::OwnershipRefused,
        _ => SourceErrorCode::Unreadable,
    }
}

fn set_errno(value: libc::c_int) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        *libc::__errno_location() = value
    };
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    unsafe {
        *libc::__error() = value
    };
}

/// Classify a NULL from `readdir` by the errno it left: zero is the end of
/// the directory, anything else an incomplete listing (fail closed).
fn readdir_end(errno: Option<i32>) -> Result<(), SourceErrorCode> {
    match errno {
        None | Some(0) => Ok(()),
        Some(_) => Err(SourceErrorCode::Unreadable),
    }
}

fn owned_ok(meta: &std::fs::Metadata) -> bool {
    meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o002 == 0
}

fn openat(dir: Option<&OwnedFd>, name: &OsStr, flags: libc::c_int) -> std::io::Result<OwnedFd> {
    let c = CString::new(name.as_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    let base = dir.map_or(libc::AT_FDCWD, |d| d.as_raw_fd());
    let fd = unsafe {
        libc::openat(
            base,
            c.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

impl PinnedDir {
    /// Open the source root itself. The root path is canonicalized (no
    /// symlink components); everything below it is opened relative to the
    /// pinned descriptor, no-follow.
    fn open_root(path: &Path) -> Result<Self, SourceErrorCode> {
        let fd = openat(None, path.as_os_str(), libc::O_RDONLY | libc::O_DIRECTORY)
            .map_err(|e| errno_code(&e))?;
        Ok(Self { fd })
    }

    /// `Ok(None)` when `name` does not exist.
    fn child_dir(&self, name: &str) -> Result<Option<PinnedDir>, SourceErrorCode> {
        match openat(
            Some(&self.fd),
            OsStr::new(name),
            libc::O_RDONLY | libc::O_DIRECTORY,
        ) {
            Ok(fd) => {
                let dir = PinnedDir { fd };
                if !owned_ok(&dir.metadata()?) {
                    return Err(SourceErrorCode::OwnershipRefused);
                }
                Ok(Some(dir))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            // Linux reports a symlink opened O_DIRECTORY|O_NOFOLLOW as
            // ENOTDIR, not ELOOP; a symlink is unsafe, not a layout file.
            Err(e) if e.raw_os_error() == Some(libc::ENOTDIR) && self.is_symlink(name) => {
                Err(SourceErrorCode::UnsafeEntry)
            }
            Err(e) => Err(errno_code(&e)),
        }
    }

    fn is_symlink(&self, name: &str) -> bool {
        let Ok(c) = CString::new(name) else {
            return false;
        };
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::fstatat(
                self.fd.as_raw_fd(),
                c.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        rc == 0 && st.st_mode & libc::S_IFMT == libc::S_IFLNK
    }

    fn metadata(&self) -> Result<std::fs::Metadata, SourceErrorCode> {
        let dup = self
            .fd
            .try_clone()
            .map_err(|_| SourceErrorCode::Unreadable)?;
        File::from(dup)
            .metadata()
            .map_err(|_| SourceErrorCode::Unreadable)
    }

    /// Read regular file `name` (owned, single link, at most `limit` bytes).
    /// `Ok(None)` when it does not exist.
    fn read_file(&self, name: &OsStr, limit: u64) -> Result<Option<Vec<u8>>, SourceErrorCode> {
        Ok(self.read_file_meta(name, limit)?.map(|(bytes, _)| bytes))
    }

    /// [`Self::read_file`] with the metadata of the file that was read.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-032 | ai:claude
    fn read_file_meta(
        &self,
        name: &OsStr,
        limit: u64,
    ) -> Result<Option<(Vec<u8>, std::fs::Metadata)>, SourceErrorCode> {
        let fd = match openat(Some(&self.fd), name, libc::O_RDONLY) {
            Ok(fd) => fd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(errno_code(&e)),
        };
        let mut file = File::from(fd);
        let meta = file.metadata().map_err(|_| SourceErrorCode::Unreadable)?;
        if !meta.is_file() || meta.nlink() != 1 {
            return Err(SourceErrorCode::UnsafeEntry);
        }
        if !owned_ok(&meta) {
            return Err(SourceErrorCode::OwnershipRefused);
        }
        if meta.len() > limit {
            return Err(SourceErrorCode::OverBudget);
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| SourceErrorCode::Unreadable)?;
        if bytes.len() as u64 > limit {
            return Err(SourceErrorCode::OverBudget);
        }
        Ok(Some((bytes, meta)))
    }

    /// Entry names of this directory, drawn from the snapshot-wide entry
    /// budget. A listing cut short by an I/O error is never "complete".
    fn names(&self, budget: &mut Budget) -> Result<Vec<Vec<u8>>, SourceErrorCode> {
        let dup = self
            .fd
            .try_clone()
            .map_err(|_| SourceErrorCode::Unreadable)?;
        let raw = std::os::fd::IntoRawFd::into_raw_fd(dup);
        let dirp = unsafe { libc::fdopendir(raw) };
        if dirp.is_null() {
            unsafe { libc::close(raw) };
            return Err(SourceErrorCode::Unreadable);
        }
        // Rewind: the duplicate shares the original's offset.
        unsafe { libc::rewinddir(dirp) };
        let mut out = Vec::new();
        let result = loop {
            if budget.expired() {
                break Err(SourceErrorCode::OverBudget);
            }
            // readdir signals both end-of-directory and failure with NULL;
            // only a cleared errno tells them apart.
            set_errno(0);
            let entry = unsafe { libc::readdir(dirp) };
            if entry.is_null() {
                break readdir_end(std::io::Error::last_os_error().raw_os_error());
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                if let Err(code) = budget.take_entry() {
                    break Err(code);
                }
                out.push(name.to_vec());
            }
        };
        unsafe { libc::closedir(dirp) };
        result.map(|()| out)
    }
}

struct ObservedMessage {
    message: Message,
    fingerprint: String,
    recorded: Recorded,
    /// The file carried its own time; `false` when the timestamp is a
    /// fallback (the file's mtime) that replicas need not agree on.
    stamped: bool,
}

struct ObservedSource {
    mailbox: PinnedDir,
    identity: SourceIdentity,
    messages: Vec<ObservedMessage>,
    /// Delivery sidecars (slice D1).
    deliveries: Vec<DeliverySidecar>,
    /// Handover deliveries (slice D2b).
    handovers: Vec<DeliveryRecord>,
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Mirror of the writers' filename mapping for identities.
fn sanitize_key(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn fingerprint(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// The sidecars of a source whose full read failed: its `.deliveries/`
/// read on its own, no-follow and budgeted. An absent mailbox or
/// `.deliveries/` holds none. `Err`: they could not be read (ruling F2).
// trace:STORY-1843 | ai:claude
fn read_source_deliveries(
    root: &Path,
    components: &[&str],
    budget: &mut Budget,
) -> Result<Vec<DeliverySidecar>, SourceErrorCode> {
    let mut dir = PinnedDir::open_root(root)?;
    for component in components.iter().copied().chain([DELIVERIES_DIR]) {
        match dir.child_dir(component)? {
            Some(child) => dir = child,
            None => return Ok(Vec::new()),
        }
    }
    read_sidecars(&dir, budget)
}

/// Read every top-level `*.json` message under `root/<components…>`.
/// `Ok(None)` when the mailbox directory (or an ancestor) does not exist.
fn read_source(
    root: &Path,
    components: &[&str],
    budget: &mut Budget,
) -> Result<Option<ObservedSource>, SourceErrorCode> {
    let mut dir = PinnedDir::open_root(root)?;
    for component in components {
        match dir.child_dir(component)? {
            Some(child) => dir = child,
            None => return Ok(None),
        }
    }
    let meta = dir.metadata()?;
    let identity = SourceIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
        mtime_ns: i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
    };
    let mut messages = Vec::new();
    for name in dir.names(budget)? {
        if !name.ends_with(b".json") || name.starts_with(b".") {
            continue;
        }
        if budget.expired() {
            return Err(SourceErrorCode::OverBudget);
        }
        let Some((bytes, meta)) =
            dir.read_file_meta(OsStr::from_bytes(&name), budget.max_message_bytes)?
        else {
            // Listed but gone: the source changed under the read.
            return Err(SourceErrorCode::SourceChanged);
        };
        budget.take(bytes.len())?;
        let mut value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| SourceErrorCode::CorruptMessage)?;
        let recorded = Recorded::of(&value);
        // An unstamped or seconds-scale time is normalized like every other
        // reader's, the unstamped one dated by this replica's mtime until
        // reconciliation settles it across replicas.
        // trace:BUG-B6B1278890774EF993D2EBC327862470-032 | ai:claude
        let stamped = written_timestamp(&value).is_some_and(|ts| ts > 0);
        normalize_message_value(&mut value, || mtime_millis(&meta));
        let message: Message =
            serde_json::from_value(value).map_err(|_| SourceErrorCode::CorruptMessage)?;
        if !safe_id(&message.id) {
            return Err(SourceErrorCode::CorruptMessage);
        }
        messages.push(ObservedMessage {
            fingerprint: fingerprint(&bytes),
            message,
            recorded,
            stamped,
        });
    }
    let mut handovers = Vec::new();
    let deliveries = match dir.child_dir(DELIVERIES_DIR)? {
        Some(sidecars) => {
            // trace:STORY-1843 | ai:claude
            if let Some(handover) = sidecars.child_dir(HANDOVER_DIR)? {
                for name in handover.names(budget)? {
                    if !name.ends_with(b".json") || name.starts_with(b".") {
                        continue;
                    }
                    if budget.expired() {
                        return Err(SourceErrorCode::OverBudget);
                    }
                    let Some(bytes) =
                        handover.read_file(OsStr::from_bytes(&name), DELIVERY_FILE_CAP)?
                    else {
                        return Err(SourceErrorCode::SourceChanged);
                    };
                    budget.take(bytes.len())?;
                    let name =
                        std::str::from_utf8(&name).map_err(|_| SourceErrorCode::CorruptMessage)?;
                    handovers.push(
                        parse_handover(&bytes, name)
                            .map_err(|_| SourceErrorCode::CorruptMessage)?,
                    );
                }
            }
            read_sidecars(&sidecars, budget)?
        }
        None => Vec::new(),
    };
    // Recheck that the path still names the directory that was read.
    let mut again = PinnedDir::open_root(root)?;
    for component in components {
        again = again
            .child_dir(component)?
            .ok_or(SourceErrorCode::SourceChanged)?;
    }
    if again.metadata().map(|m| (m.dev(), m.ino()))? != (identity.dev, identity.ino) {
        return Err(SourceErrorCode::SourceChanged);
    }
    Ok(Some(ObservedSource {
        mailbox: dir,
        identity,
        messages,
        deliveries,
        handovers,
    }))
}

// ── Delivery ownership (slice D1) ───────────────────────────────────────────

/// How a message with a delivery sidecar relates to this audience.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeliveryGate {
    /// This audience's recipient owns a delivery of it: the record it owns.
    Owned(RecipientUuid, Box<DeliveryRecord>),
    /// Not owned here: a cohort this reader is not in, a queued delivery
    /// bound to someone else, or one this reader may still bind.
    NotOwned,
}

/// Sidecars by message ID, the gate for every message the audience is
/// addressed to, the queued deliveries this reader may bind, and the IDs
/// whose sidecar replicas disagree (never projected).
///
/// A role message with no live sidecar is looked up by ID in the retained
/// copies of retired sources, so a replica elsewhere keeps its cohort gate
/// after the sending worktree is retired. Faults stay local to their
/// message: a retained sidecar or a queued delivery's binding that cannot be
/// read or parsed hides that one message (its ownership is unknown) and
/// offers nothing to bind, without making the whole view partial. Only a
/// retained area that cannot be opened at all is a view-wide anchor error.
///
/// `salvaged`: the sidecars of live sources whose message read failed but
/// whose `.deliveries/` was read on its own.
///
/// Fail closed (ruling F2): when `unsighted` (a live source's `.deliveries/`
/// was not read, so the snapshot is already partial), a
/// role message with no sidecar found anywhere has unknown ownership and is
/// hidden from every reader of this snapshot, never served as legacy role
/// mail. Vendor-group mail never carries a sidecar and is never gated.
// trace:STORY-1843 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-178 | ai:codex
fn delivery_gates(
    ctx: &MailboxContext,
    observed: &[Option<ObservedSource>],
    salvaged: Vec<DeliverySidecar>,
    unsighted: bool,
    audience: &PolicySubject<'_>,
    anchor: &mut AnchorEvidence,
    budget: &mut Budget,
) -> (
    HashMap<String, DeliveryGate>,
    Vec<PendingDelivery>,
    Conflicts,
    Vec<String>,
) {
    let mut sidecars: BTreeMap<String, Vec<DeliverySidecar>> = BTreeMap::new();
    let mut handovers: HashMap<&str, Vec<&DeliveryRecord>> = HashMap::new();
    let mut threads: HashMap<&str, BTreeSet<String>> = HashMap::new();
    let mut addressed: HashSet<&str> = HashSet::new();
    let mut role_addressed: BTreeSet<&str> = BTreeSet::new();
    for sidecar in salvaged {
        sidecars
            .entry(sidecar.message_id.clone())
            .or_default()
            .push(sidecar);
    }
    for source in observed.iter().flatten() {
        for sidecar in &source.deliveries {
            sidecars
                .entry(sidecar.message_id.clone())
                .or_default()
                .push(sidecar.clone());
        }
        for record in &source.handovers {
            handovers
                .entry(&record.message_id)
                .or_default()
                .push(record);
        }
        for m in &source.messages {
            threads
                .entry(&m.message.id)
                .or_default()
                .insert(m.message.thread_id.clone());
            let undeleted = Message {
                deleted: false,
                ..m.message.clone()
            };
            let keys = audience.matching_keys(&undeleted);
            if !keys.is_empty() {
                addressed.insert(&m.message.id);
            }
            if keys
                .iter()
                .any(|k| audience.role_keys().iter().any(|s| s == k))
            {
                role_addressed.insert(&m.message.id);
            }
        }
    }
    let mut unknown_ids = BTreeSet::new();
    let mut gates = HashMap::new();
    let wanted: Vec<&str> = role_addressed
        .into_iter()
        .filter(|id| !sidecars.contains_key(*id))
        .collect();
    if let (false, Some(common)) = (wanted.is_empty(), &ctx.repository) {
        match RetainedDeliveries::open(common.path(), budget) {
            Ok(retained) => {
                for &id in &wanted {
                    match retained.lookup(id, budget) {
                        Ok(found) if !found.is_empty() => {
                            sidecars.insert(id.to_string(), found);
                        }
                        Ok(_) => {}
                        Err(()) => {
                            unknown_ids.insert(id.to_string());
                            gates.insert(id.to_string(), DeliveryGate::NotOwned);
                        }
                    }
                }
            }
            Err(code) => {
                anchor.error.get_or_insert(code);
                for &id in &wanted {
                    unknown_ids.insert(id.to_string());
                    gates.insert(id.to_string(), DeliveryGate::NotOwned);
                }
            }
        }
    }
    if unsighted {
        for id in wanted.into_iter().filter(|id| !sidecars.contains_key(*id)) {
            unknown_ids.insert(id.to_string());
            gates.insert(id.to_string(), DeliveryGate::NotOwned);
        }
    }
    let mut pending = Vec::new();
    let mut conflicts = Vec::new();
    for (id, replicas) in &sidecars {
        let id = id.as_str();
        if replicas.iter().any(|r| *r != replicas[0]) {
            gates.insert(id.to_string(), DeliveryGate::NotOwned);
            if addressed.contains(id) {
                conflicts.push((id.to_string(), threads.remove(id).unwrap_or_default()));
            }
            continue;
        }
        let sidecar = &replicas[0];
        if !addressed.contains(id) {
            gates.insert(id.to_string(), DeliveryGate::NotOwned);
            continue;
        }
        let mut bindings = HashMap::new();
        let mut unknown = false;
        for delivery_id in sidecar.queued_ids() {
            match read_binding(ctx, delivery_id, budget) {
                Ok(bound) => {
                    bindings.insert(delivery_id.to_string(), bound);
                }
                Err(_) => unknown = true,
            }
        }
        if unknown {
            unknown_ids.insert(id.to_string());
        }
        let handed = valid_handovers(sidecar, handovers.get(id).map(Vec::as_slice));
        let owned = audience
            .recipient()
            .and_then(|own| owned_record(sidecar, &handed, own, &bindings));
        let gate = match audience.recipient() {
            Some(own) if owned.is_some() => {
                DeliveryGate::Owned(own.clone(), Box::new(owned.expect("checked")))
            }
            Some(_) if !unknown => {
                pending.extend(
                    sidecar
                        .queued_ids()
                        .filter(|d| bindings.get(*d).is_some_and(Option::is_none))
                        .map(|d| PendingDelivery {
                            message_id: id.to_string(),
                            delivery_id: d.to_string(),
                        }),
                );
                DeliveryGate::NotOwned
            }
            _ => DeliveryGate::NotOwned,
        };
        gates.insert(id.to_string(), gate);
    }
    (gates, pending, conflicts, unknown_ids.into_iter().collect())
}

/// Where retired sources' mailboxes are kept, below the common Git dir:
/// `retained/<source-key>/<generation>/data/` mirrors the mailbox.
const RETAINED_PATH: [&str; 3] = ["aida", "mail-lifecycle", "retained"];

/// The `.deliveries/` directories of the retained copies of retired
/// sources, pinned once per snapshot (one directory per retired
/// generation that holds sidecars), then looked up by exact message ID:
/// no sidecar is read unless the view holds a message it names.
// trace:STORY-1843 | ai:claude
struct RetainedDeliveries {
    dirs: Vec<PinnedDir>,
}

impl RetainedDeliveries {
    fn open(common: &Path, budget: &mut Budget) -> Result<Self, SourceErrorCode> {
        let mut dir = PinnedDir::open_root(common)?;
        for component in RETAINED_PATH {
            match dir.child_dir(component)? {
                Some(child) => dir = child,
                None => return Ok(Self { dirs: Vec::new() }),
            }
        }
        let mut dirs = Vec::new();
        for key in dir.names(budget)? {
            let Some(key_dir) = dir.child_dir(&String::from_utf8_lossy(&key))? else {
                continue;
            };
            for generation in key_dir.names(budget)? {
                let Some(gen_dir) = key_dir.child_dir(&String::from_utf8_lossy(&generation))?
                else {
                    continue;
                };
                if let Some(sidecars) = gen_dir
                    .child_dir("data")?
                    .map(|data| data.child_dir(DELIVERIES_DIR))
                    .transpose()?
                    .flatten()
                {
                    dirs.push(sidecars);
                }
            }
        }
        Ok(Self { dirs })
    }

    /// Every retained sidecar for message `id`. `Err` when one exists but
    /// cannot be read or parsed: that message's ownership is unknown.
    fn lookup(&self, id: &str, budget: &mut Budget) -> Result<Vec<DeliverySidecar>, ()> {
        let name = format!("{id}.json");
        let mut out = Vec::new();
        for dir in &self.dirs {
            let bytes = dir
                .read_file(OsStr::new(&name), DELIVERY_FILE_CAP)
                .map_err(|_| ())?;
            let Some(bytes) = bytes else { continue };
            budget.take(bytes.len()).map_err(|_| ())?;
            out.push(DeliverySidecar::parse(&bytes, id).map_err(|_| ())?);
        }
        Ok(out)
    }
}

/// Parse every `<message-id>.json` sidecar in a `.deliveries/` directory.
fn read_sidecars(
    sidecars: &PinnedDir,
    budget: &mut Budget,
) -> Result<Vec<DeliverySidecar>, SourceErrorCode> {
    let mut out = Vec::new();
    for name in sidecars.names(budget)? {
        let Some(stem) = name
            .strip_suffix(b".json")
            .filter(|_| !name.starts_with(b"."))
        else {
            continue;
        };
        if budget.expired() {
            return Err(SourceErrorCode::OverBudget);
        }
        let Some(bytes) = sidecars.read_file(OsStr::from_bytes(&name), DELIVERY_FILE_CAP)? else {
            return Err(SourceErrorCode::SourceChanged);
        };
        budget.take(bytes.len())?;
        let stem = std::str::from_utf8(stem).map_err(|_| SourceErrorCode::CorruptMessage)?;
        out.push(
            DeliverySidecar::parse(&bytes, stem).map_err(|_| SourceErrorCode::CorruptMessage)?,
        );
    }
    Ok(out)
}

/// The handovers of `sidecar`'s message whose chain reaches one of its
/// original deliveries. A handover naming any other delivery owns nothing.
// trace:STORY-1843 | ai:claude
fn valid_handovers<'a>(
    sidecar: &DeliverySidecar,
    found: Option<&[&'a DeliveryRecord]>,
) -> Vec<&'a DeliveryRecord> {
    let mut known: HashSet<&str> = sidecar
        .deliveries
        .iter()
        .map(|d| d.delivery_id.as_str())
        .collect();
    let mut pending: Vec<&DeliveryRecord> = found.unwrap_or_default().to_vec();
    pending.sort_by(|a, b| a.delivery_id.cmp(&b.delivery_id));
    pending.dedup_by(|a, b| a == b);
    let mut valid = Vec::new();
    // Each pass admits handovers of already-known deliveries, so a chain
    // (old run -> new run -> newer run) resolves in order.
    loop {
        let before = valid.len();
        pending.retain(|h| {
            let reaches = h.handover_of.as_deref().is_some_and(|o| known.contains(o));
            if reaches {
                valid.push(*h);
            }
            !reaches
        });
        if valid.len() == before {
            break;
        }
        for h in &valid[before..] {
            known.insert(h.delivery_id.as_str());
        }
    }
    valid
}

/// The delivery record of `sidecar`'s message that `own` owns: an original
/// delivery, a queued delivery bound to it, or a valid handover.
fn owned_record(
    sidecar: &DeliverySidecar,
    handed: &[&DeliveryRecord],
    own: &RecipientUuid,
    bindings: &HashMap<String, Option<RecipientUuid>>,
) -> Option<DeliveryRecord> {
    sidecar
        .deliveries
        .iter()
        .find(|d| match (&d.recipient_uuid, d.queued_for_role) {
            (Some(r), _) => r == own,
            (None, true) => bindings.get(&d.delivery_id).cloned().flatten().as_ref() == Some(own),
            (None, false) => false,
        })
        .or_else(|| {
            handed
                .iter()
                .copied()
                .find(|h| h.recipient_uuid.as_ref() == Some(own))
        })
        .cloned()
}

/// The recipient a queued delivery is bound to, read no-follow and bounded
/// from the private anchor (or a local-only project's mailbox).
fn read_binding(
    ctx: &MailboxContext,
    delivery_id: &str,
    budget: &mut Budget,
) -> Result<Option<RecipientUuid>, SourceErrorCode> {
    if budget.expired() {
        return Err(SourceErrorCode::OverBudget);
    }
    let (root, components) = match &ctx.repository {
        Some(common) => (common.path(), binding_dir_components(true)),
        None => (ctx.requested_checkout.path(), binding_dir_components(false)),
    };
    let mut dir = PinnedDir::open_root(root)?;
    for component in components {
        match dir.child_dir(component)? {
            Some(child) => dir = child,
            None => return Ok(None),
        }
    }
    let name = binding_file_name(delivery_id);
    match dir.read_file(OsStr::new(&name), DELIVERY_FILE_CAP)? {
        None => Ok(None),
        Some(bytes) => {
            budget.take(bytes.len())?;
            DeliveryBinding::parse(&bytes, delivery_id)
                .map(|b| Some(b.recipient_uuid))
                .map_err(|_| SourceErrorCode::CorruptReceipt)
        }
    }
}

// ── Reconciliation ───────────────────────────────────────────────────────────

/// Fields that never change across state markers; replicas must agree.
fn audience_tuple(m: &Message) -> (&str, &str, &Recipient, i64, Option<&str>) {
    (
        &m.thread_id,
        &m.from,
        &m.to,
        m.timestamp,
        m.in_reply_to.as_deref(),
    )
}

/// The immutable payload and marker state; equal-rank replicas must agree.
type Payload<'a> = (
    (&'a str, &'a str, &'a Recipient, i64, Option<&'a str>),
    &'a str,
    (bool, bool, bool),
);

fn payload(m: &Message) -> Payload<'_> {
    (
        audience_tuple(m),
        &m.body,
        (m.retracted, m.archived, m.deleted),
    )
}

/// The optional, writer-enriched metadata keys, in [`Recorded`] order.
const METADATA_KEYS: [&str; 6] = [
    "subject",
    "urgent",
    "intent",
    "from_source",
    "from_role",
    "relayed_from",
];

/// Which [`METADATA_KEYS`] a replica's file actually carried (non-null).
/// An older writer omits them and they read back as the default, so only
/// the raw key tells an omitted field from an explicitly written default.
// trace:STORY-1843 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Recorded([bool; METADATA_KEYS.len()]);

impl Recorded {
    fn of(value: &serde_json::Value) -> Self {
        Self(METADATA_KEYS.map(|key| value.get(key).is_some_and(|v| !v.is_null())))
    }
}

/// Fill one optional, writer-enriched field from another replica. A field
/// the other replica did not record is never a disagreement; two different
/// recorded values — including an explicit default against a non-default —
/// are a contradiction (`false`).
fn fill<T: PartialEq + Clone>(
    into: &mut T,
    into_recorded: &mut bool,
    other: &T,
    other_recorded: bool,
) -> bool {
    if !other_recorded || (*into_recorded && into == other) {
        true
    } else if !*into_recorded {
        *into = other.clone();
        *into_recorded = true;
        true
    } else {
        false
    }
}

/// Merge the optional metadata of `other` into `into` (the superset of what
/// the replicas recorded). Payload fields are compared separately.
fn merge_metadata(
    into: &mut Message,
    into_recorded: &mut Recorded,
    other: &Message,
    other_recorded: Recorded,
) -> bool {
    let [subject, urgent, intent, from_source, from_role, relayed_from] = &mut into_recorded.0;
    let o = other_recorded.0;
    fill(&mut into.subject, subject, &other.subject, o[0])
        & fill(&mut into.urgent, urgent, &other.urgent, o[1])
        & fill(&mut into.intent, intent, &other.intent, o[2])
        & fill(&mut into.from_source, from_source, &other.from_source, o[3])
        & fill(&mut into.from_role, from_role, &other.from_role, o[4])
        & fill(
            &mut into.relayed_from,
            relayed_from,
            &other.relayed_from,
            o[5],
        )
}

/// Give every replica of a message whose file carried no time one shared
/// timestamp, so the mtime fallback (which differs per replica, and in a git
/// checkout changes on every pull) is never an audience disagreement: the
/// time a stamped replica recorded, the canonical one first, else the
/// earliest fallback. A sync persists that stamp into the canonical copy.
// trace:BUG-B6B1278890774EF993D2EBC327862470-032 | ai:claude
fn settle_unstamped_timestamps(observed: &mut [Option<ObservedSource>], local_count: usize) {
    let mut settled: HashMap<String, (bool, i64)> = HashMap::new();
    let mut unstamped = false;
    for (index, source) in observed.iter().enumerate() {
        for m in source.iter().flat_map(|s| &s.messages) {
            unstamped |= !m.stamped;
            // Rank: a canonical stamp, then any stamp, then the earliest
            // fallback.
            let candidate = (m.stamped, m.message.timestamp);
            let better = |held: &(bool, i64)| match (candidate.0, held.0) {
                (true, false) => true,
                (true, true) => index >= local_count,
                (false, false) => {
                    timestamp_is_known(candidate.1)
                        && (!timestamp_is_known(held.1) || candidate.1 < held.1)
                }
                (false, true) => false,
            };
            settled
                .entry(m.message.id.clone())
                .and_modify(|held| {
                    if better(held) {
                        *held = candidate;
                    }
                })
                .or_insert(candidate);
        }
    }
    if !unstamped {
        return;
    }
    for source in observed.iter_mut().flatten() {
        for m in source.messages.iter_mut().filter(|m| !m.stamped) {
            if let Some((_, ts)) = settled.get(&m.message.id) {
                m.message.timestamp = *ts;
            }
        }
    }
}

/// IDs of conflicting records, each with the thread IDs its replicas claim
/// (so a query can scope them).
type Conflicts = Vec<(String, BTreeSet<String>)>;

/// Reconcile all observed replicas by immutable ID, then filter to the
/// audience. The highest state rank wins (a delete marker suppresses every
/// replica). Equal-rank replicas must share the immutable payload; optional
/// writer-enriched metadata merges to the superset, starting from the
/// canonical copy. Replicas that disagree on audience, on payload at equal
/// rank, or on a recorded metadata value are a conflict: no replica is
/// chosen. Only IDs visible to the audience are reported.
fn reconcile(
    observed: &[Option<ObservedSource>],
    local_count: usize,
    audience: &PolicySubject<'_>,
    include_own_sent: bool,
    delivered: &HashMap<String, DeliveryGate>,
) -> (Vec<(Message, Vec<Provenance>)>, Conflicts) {
    let owned = |m: &Message| {
        delivered
            .get(&m.id)
            .is_none_or(|gate| matches!(gate, DeliveryGate::Owned(..)))
    };
    let reaches = |m: &Message| {
        (!audience.matching_keys(m).is_empty() && owned(m))
            || (include_own_sent && audience.sent_by_own(m))
    };
    let mut by_id: BTreeMap<&str, Vec<(usize, &ObservedMessage)>> = BTreeMap::new();
    for (index, source) in observed.iter().enumerate() {
        for m in source.iter().flat_map(|s| &s.messages) {
            by_id
                .entry(m.message.id.as_str())
                .or_default()
                .push((index, m));
        }
    }
    let mut records = Vec::new();
    let mut conflicts = Vec::new();
    for (id, replicas) in by_id {
        let visible = replicas.iter().any(|(_, m)| reaches(&m.message))
            || replicas.iter().any(|(_, m)| {
                // A delete marker hides the message, but its audience is the
                // original one; check visibility on the undeleted shape.
                m.message.deleted
                    && reaches(&Message {
                        deleted: false,
                        ..m.message.clone()
                    })
            });
        if !visible {
            continue;
        }
        let first = audience_tuple(&replicas[0].1.message);
        let audience_conflict = replicas
            .iter()
            .any(|(_, m)| audience_tuple(&m.message) != first);
        let top_rank = replicas
            .iter()
            .map(|(_, m)| message_state_rank(&m.message))
            .max()
            .unwrap_or(0);
        let top: Vec<_> = replicas
            .iter()
            .filter(|(_, m)| message_state_rank(&m.message) == top_rank)
            .collect();
        let winner = top
            .iter()
            .find(|(source, _)| *source >= local_count)
            .unwrap_or(&top[0]);
        let mut message = winner.1.message.clone();
        let mut recorded = winner.1.recorded;
        let payload_conflict = top
            .iter()
            .any(|(_, m)| payload(&m.message) != payload(&message));
        let metadata_conflict = !top.iter().fold(true, |ok, (_, m)| {
            merge_metadata(&mut message, &mut recorded, &m.message, m.recorded) & ok
        });
        if audience_conflict || payload_conflict || metadata_conflict {
            let threads = replicas
                .iter()
                .map(|(_, m)| m.message.thread_id.clone())
                .collect();
            conflicts.push((id.to_string(), threads));
            continue;
        }
        if message.deleted || !reaches(&message) {
            continue;
        }
        // A retraction is monotone: a higher-rank replica elsewhere (say the
        // recipient archived its copy in main while the sender retracted in a
        // sibling) must not resurrect the retracted body.
        // trace:STORY-1843 | ai:claude
        if replicas.iter().any(|(_, m)| m.message.retracted) {
            message.retracted = true;
        }
        let provenance = replicas
            .iter()
            .map(|(source, m)| Provenance {
                source: *source,
                fingerprint: m.fingerprint.clone(),
            })
            .collect();
        records.push((message, provenance));
    }
    records.sort_by(|a, b| {
        a.0.timestamp
            .cmp(&b.0.timestamp)
            .then_with(|| a.0.id.cmp(&b.0.id))
    });
    (records, conflicts)
}

// ── Read evidence ────────────────────────────────────────────────────────────

/// One local source's receipt directory and watermark for one key, opened
/// once per snapshot however many records are looked up against it.
struct KeyEvidence {
    receipts: Option<PinnedDir>,
    watermark: Option<i64>,
}

#[derive(Default)]
struct ReceiptCache {
    by_source_key: HashMap<(usize, String), Result<KeyEvidence, SourceErrorCode>>,
}

fn key_evidence(
    mailbox: &PinnedDir,
    key: &str,
    budget: &mut Budget,
) -> Result<KeyEvidence, SourceErrorCode> {
    let receipts = match mailbox.child_dir(".read-receipts")? {
        Some(dir) => dir.child_dir(&sanitize_key(key))?,
        None => None,
    };
    Ok(KeyEvidence {
        receipts,
        watermark: watermark(mailbox, key, budget)?,
    })
}

/// Exact per-ID lookups only (no receipt directory scan). The receipt
/// anchor acknowledges a message only when EVERY key it is addressed to
/// holds its receipt there: one key's ledger alone is a partly published
/// ACK (each key's ledger has its own rename), so the message is shown again
/// and the next ACK plans the missing keys. An explicit receipt in any LOCAL
/// source for a key the message is addressed to is acknowledgement. A LOCAL watermark counts only for IDs physically present
/// in that same source, and only as legacy compatibility evidence.
/// Unreadable receipt evidence, or a deadline reached mid-lookup, marks that
/// source as errored.
#[allow(clippy::too_many_arguments)]
// trace:BUG-b6b1278890774ef993d2ebc327862470-178 | ai:codex
fn read_evidence(
    message: &Message,
    keys: &[String],
    provenance: &[Provenance],
    observed: &[Option<ObservedSource>],
    local_count: usize,
    sources: &mut [SourceReport],
    cache: &mut ReceiptCache,
    anchor: &mut AnchorEvidence,
    budget: &mut Budget,
    unknown: &mut bool,
) -> ReadEvidence {
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let mut anchored = None;
    let mut every_key = !keys.is_empty();
    for key in &keys {
        match anchor.lookup(key, &message.id, budget) {
            Some(hit) => {
                anchored.get_or_insert((*key, hit));
            }
            None => every_key = false,
        }
    }
    *unknown |= keys.iter().any(|key| anchor.failed_keys.contains(*key));
    if let (true, Some((key, (at, legacy)))) = (every_key, anchored) {
        return ReadEvidence::Acknowledged {
            origin: AckOrigin::Anchor { legacy },
            key: key.to_string(),
            at,
        };
    }
    let holders: BTreeSet<usize> = provenance.iter().map(|p| p.source).collect();
    let mut legacy = None;
    for (index, source) in observed.iter().enumerate().take(local_count) {
        let Some(source) = source else { continue };
        for key in &keys {
            if budget.expired() {
                *unknown = true;
                sources[index].status = SourceStatus::Error(SourceErrorCode::OverBudget);
                return legacy.unwrap_or(ReadEvidence::Unread);
            }
            let evidence = cache
                .by_source_key
                .entry((index, key.to_string()))
                .or_insert_with(|| key_evidence(&source.mailbox, key, budget));
            let evidence = match evidence {
                Ok(evidence) => evidence,
                Err(code) => {
                    *unknown = true;
                    sources[index].status = SourceStatus::Error(*code);
                    continue;
                }
            };
            if let Some(dir) = &evidence.receipts {
                match receipt_at(dir, &message.id, budget) {
                    Ok(Some(at)) => {
                        return ReadEvidence::Acknowledged {
                            origin: AckOrigin::Source(index),
                            key: key.to_string(),
                            at,
                        }
                    }
                    Ok(None) => {}
                    Err(code) => {
                        *unknown = true;
                        sources[index].status = SourceStatus::Error(code);
                    }
                }
            }
            if legacy.is_none()
                && holders.contains(&index)
                && evidence
                    .watermark
                    .is_some_and(|mark| message.timestamp <= mark)
            {
                legacy = Some(ReadEvidence::LegacyWatermark {
                    source: index,
                    key: key.to_string(),
                });
            }
        }
    }
    legacy.unwrap_or(ReadEvidence::Unread)
}

fn parse_millis(bytes: &[u8]) -> Result<i64, SourceErrorCode> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .ok_or(SourceErrorCode::CorruptReceipt)
}

fn receipt_at(
    receipts: &PinnedDir,
    id: &str,
    budget: &mut Budget,
) -> Result<Option<i64>, SourceErrorCode> {
    let name = format!("{id}.txt");
    match receipts.read_file(OsStr::new(&name), SMALL_FILE)? {
        None => Ok(None),
        Some(bytes) => {
            budget.take(bytes.len())?;
            parse_millis(&bytes).map(Some)
        }
    }
}

fn watermark(
    mailbox: &PinnedDir,
    key: &str,
    budget: &mut Budget,
) -> Result<Option<i64>, SourceErrorCode> {
    let Some(dir) = mailbox.child_dir(".read")? else {
        return Ok(None);
    };
    let name = format!("{}.txt", sanitize_key(key));
    match dir.read_file(OsStr::new(&name), SMALL_FILE)? {
        None => Ok(None),
        Some(bytes) => {
            budget.take(bytes.len())?;
            parse_millis(&bytes).map(Some)
        }
    }
}

// ── The receipt anchor ──────────────────────────────────────────────────────

/// Where the receipt ledgers live, below the verified common Git directory:
/// inside the mail-lifecycle anchor, so they share its ownership, mode and
/// no-follow rules and outlive every worktree.
pub const RECEIPT_ANCHOR_PATH: [&str; 3] = ["aida", "mail-lifecycle", "receipts"];
/// Byte cap on one key's ledger.
pub const RECEIPT_LEDGER_CAP: u64 = 4 << 20;
/// Entry cap on one key's ledger. A full ledger refuses new receipts; it is
/// never pruned to make room (signoff 60 #5).
pub const RECEIPT_LEDGER_MAX_ENTRIES: usize = 32_768;
/// Ledger schema version.
pub const RECEIPT_LEDGER_SCHEMA: u32 = 1;

/// One recipient key's explicit per-ID receipts: message ID to the epoch
/// millis of the ACK that recorded it. Written only by the post-flush ACK
/// (`mailbox_lifecycle::ack`), under Lifecycle then Receipt.
// trace:STORY-1843 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReceiptLedger {
    pub schema: u32,
    pub key: String,
    pub receipts: BTreeMap<String, i64>,
    /// IDs in `receipts` whose ACK resolved a legacy watermark: acknowledged,
    /// but their real read time is unknown.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub legacy: BTreeSet<String>,
}

impl ReceiptLedger {
    /// Parse and validate a ledger read for `key`: the schema, the key it
    /// names, the entry cap and every ID's shape.
    pub fn parse(bytes: &[u8], key: &str) -> Result<Self, SourceErrorCode> {
        let ledger: Self =
            serde_json::from_slice(bytes).map_err(|_| SourceErrorCode::CorruptReceipt)?;
        if ledger.schema != RECEIPT_LEDGER_SCHEMA
            || ledger.key != key
            || ledger.receipts.len() > RECEIPT_LEDGER_MAX_ENTRIES
            || !ledger.receipts.keys().all(|id| safe_id(id))
            || !ledger
                .legacy
                .iter()
                .all(|id| ledger.receipts.contains_key(id))
        {
            return Err(SourceErrorCode::CorruptReceipt);
        }
        Ok(ledger)
    }
}

/// The ledger file name for `key`: a digest, so no identity spelling can
/// collide with another or escape the directory.
pub fn receipt_ledger_name(key: &str) -> String {
    format!("{}.json", fingerprint(key.as_bytes()))
}

/// Read `key`'s ledger under `common`, no-follow and bounded. `Ok(None)`
/// when no ledger (or no anchor) exists yet.
// trace:STORY-1843 | ai:claude
pub fn read_receipt_ledger(
    common: &Path,
    key: &str,
) -> Result<Option<ReceiptLedger>, SourceErrorCode> {
    Ok(read_ledger_sized(common, key)?.map(|(ledger, _)| ledger))
}

/// [`read_receipt_ledger`] plus the ledger's size in bytes.
fn read_ledger_sized(
    common: &Path,
    key: &str,
) -> Result<Option<(ReceiptLedger, usize)>, SourceErrorCode> {
    let mut dir = PinnedDir::open_root(common)?;
    for component in RECEIPT_ANCHOR_PATH {
        match dir.child_dir(component)? {
            Some(child) => dir = child,
            None => return Ok(None),
        }
    }
    let name = receipt_ledger_name(key);
    match dir.read_file(OsStr::new(&name), RECEIPT_LEDGER_CAP)? {
        None => Ok(None),
        Some(bytes) => ReceiptLedger::parse(&bytes, key).map(|l| Some((l, bytes.len()))),
    }
}

/// The receipt anchor as one snapshot sees it: each key's ledger read once.
struct AnchorEvidence {
    common: Option<PathBuf>,
    ledgers: HashMap<String, Option<ReceiptLedger>>,
    failed_keys: HashSet<String>,
    error: Option<SourceErrorCode>,
}

impl AnchorEvidence {
    fn new(common: Option<&Path>) -> Self {
        Self {
            common: common.map(Path::to_path_buf),
            ledgers: HashMap::new(),
            failed_keys: HashSet::new(),
            error: None,
        }
    }

    /// `key`'s anchored receipt for `id` and whether it resolved a legacy
    /// watermark. A ledger that cannot be read records the error (the
    /// snapshot is then partial) and answers `None`.
    fn lookup(&mut self, key: &str, id: &str, budget: &mut Budget) -> Option<(i64, bool)> {
        let common = self.common.as_deref()?;
        if !self.ledgers.contains_key(key) {
            let read = if budget.expired() {
                Err(SourceErrorCode::OverBudget)
            } else {
                read_ledger_sized(common, key).and_then(|read| {
                    budget.take(read.as_ref().map_or(0, |(_, len)| *len))?;
                    Ok(read.map(|(ledger, _)| ledger))
                })
            };
            let ledger = read.unwrap_or_else(|code| {
                self.failed_keys.insert(key.to_string());
                self.error.get_or_insert(code);
                None
            });
            self.ledgers.insert(key.to_string(), ledger);
        }
        let ledger = self.ledgers.get(key)?.as_ref()?;
        let at = *ledger.receipts.get(id)?;
        Some((at, ledger.legacy.contains(id)))
    }
}

/// Re-read one replica for an ACK commit: the fingerprint of message `id`'s
/// file under `root/<components…>` and the identity of the directory that
/// holds it. `Ok(None)` when the file (or the directory) is gone. Read-only,
/// no-follow and bounded, like every snapshot read.
// trace:STORY-1843 | ai:claude
pub fn replica_fingerprint(
    root: &Path,
    components: &[&str],
    id: &str,
) -> Result<Option<(String, u64, u64)>, SourceErrorCode> {
    if !safe_id(id) {
        return Err(SourceErrorCode::CorruptMessage);
    }
    let mut dir = PinnedDir::open_root(root)?;
    for component in components {
        match dir.child_dir(component)? {
            Some(child) => dir = child,
            None => return Ok(None),
        }
    }
    let meta = dir.metadata()?;
    let name = format!("{id}.json");
    Ok(dir
        .read_file(OsStr::new(&name), 1024 * 1024)?
        .map(|bytes| (fingerprint(&bytes), meta.dev(), meta.ino())))
}

#[cfg(test)]
#[path = "mailbox_logical_tests.rs"]
mod tests;

// trace:STORY-1843 | ai:claude
#[cfg(test)]
#[path = "mailbox_relay_handover_tests.rs"]
mod relay_handover_tests;
