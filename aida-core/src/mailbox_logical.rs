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
//! established here (signed slice C), and ACK is signed slice B2.
//!
//! Source-local write paths (digest, archive/retract/delete, migration,
//! lifecycle) keep using the explicit per-source readers in
//! [`crate::mailbox`]; this global view grants no write scope.
// trace:STORY-1843 | ai:claude

use crate::mailbox::{message_state_rank, Message, Recipient};
use crate::mailbox_identity::{classified_identities, IdentityClass, MailIdentityFacts};
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
        Ok(Self {
            private,
            human,
            shared,
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
        Self::from_identity_policy(private, human, shared)
            .expect("every key was checked as a bounded mailbox identity")
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
/// (signed slice C): a later ACK plan can refuse when the directory was
/// replaced (`dev`/`ino`) or its entries changed (`mtime_ns`) since the cut.
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

/// Read evidence for one visible record, for the key it is addressed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadEvidence {
    /// An explicit per-ID receipt in a local source.
    Acknowledged { source: usize, key: String, at: i64 },
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
}

impl LogicalSnapshot {
    /// Every inventoried source, the canonical layer (when configured) and
    /// every receipt lookup was fully observed.
    pub fn complete(&self) -> bool {
        self.inventory_error.is_none()
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
}

/// Take a pure-inspection snapshot for `audience`. Never writes.
pub fn logical_snapshot(
    ctx: &MailboxContext,
    audience: &ResolvedAudience,
    query: &Query,
    limits: &ReadLimits,
) -> LogicalSnapshot {
    let observed_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let mut budget = Budget::new(limits);
    let (roots, inventory_error) = inventory(ctx, &mut budget);
    let mut sources = Vec::new();
    let mut observed: Vec<Option<ObservedSource>> = Vec::new();
    for root in roots {
        let (path, result) = match root {
            Err((path, code)) => (path, Err(code)),
            Ok(path) => {
                let result = read_source(&path, &[".aida", "mailbox"], &mut budget);
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

    let in_query = |id: &str, thread: &str| match query {
        Query::Inbox | Query::Addressable => true,
        Query::ExactId(want) => id == want,
        Query::Thread(want) => thread == want,
    };
    let (mut records, conflicts) =
        reconcile(&observed, local_count, audience, query.includes_own_sent());
    records.retain(|(m, _)| in_query(&m.id, &m.thread_id));
    let conflicts = conflicts
        .into_iter()
        .filter(|(id, threads)| threads.iter().any(|t| in_query(id, t)))
        .map(|(id, _)| id)
        .collect();

    let mut receipts = ReceiptCache::default();
    let mut out_records = Vec::with_capacity(records.len());
    for (mut message, provenance) in records {
        let read = read_evidence(
            &message,
            &provenance,
            &observed,
            local_count,
            audience,
            &mut sources,
            &mut receipts,
            &mut budget,
        );
        if message.retracted {
            message.body.clear();
            message.subject = None;
        }
        out_records.push(SnapshotRecord {
            message,
            provenance,
            read,
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
    };
    if let Query::ExactId(id) = query {
        let found = snapshot.records.iter().any(|r| &r.message.id == id)
            || snapshot.conflicts.iter().any(|c| c == id);
        snapshot.exact_found = (found || snapshot.complete()).then_some(found);
    }
    snapshot
}

/// Every explicit per-ID read receipt in every inventoried LOCAL source,
/// keyed by receipt directory (the sanitized mailbox key). Operator
/// inspection for fleet-wide counters (night-shift latency, maintenance
/// predicates, launch context, dead-letter list) that count every
/// recipient's unread mail: it exposes which IDs were read, never message
/// content, so it needs no audience. The same receipt evidence
/// [`logical_snapshot`] treats as acknowledgement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiptInspection {
    by_key: HashMap<String, HashSet<String>>,
    /// Every inventoried source was fully listed.
    pub complete: bool,
}

impl ReceiptInspection {
    /// Whether some local source holds a receipt for `id` under `key`.
    pub fn receipted(&self, key: &str, id: &str) -> bool {
        self.by_key
            .get(&sanitize_key(key))
            .is_some_and(|ids| ids.contains(id))
    }
}

/// Inspect the read receipts of every product worktree of `ctx`. Never
/// writes; a source that cannot be listed leaves the result incomplete.
// trace:STORY-1843 | ai:claude
pub fn inspect_read_receipts(ctx: &MailboxContext, limits: &ReadLimits) -> ReceiptInspection {
    let mut budget = Budget::new(limits);
    let (roots, inventory_error) = inventory(ctx, &mut budget);
    let mut out = ReceiptInspection {
        complete: inventory_error.is_none(),
        ..Default::default()
    };
    for root in roots {
        let Ok(root) = root else {
            out.complete = false;
            continue;
        };
        if receipts_of_source(&root, &mut budget, &mut out.by_key).is_err() {
            out.complete = false;
        }
    }
    out
}

fn receipts_of_source(
    root: &Path,
    budget: &mut Budget,
    out: &mut HashMap<String, HashSet<String>>,
) -> Result<(), SourceErrorCode> {
    let mut dir = PinnedDir::open_root(root)?;
    for component in [".aida", "mailbox", ".read-receipts"] {
        match dir.child_dir(component)? {
            Some(child) => dir = child,
            None => return Ok(()),
        }
    }
    for key in dir.names(budget)? {
        if budget.expired() {
            return Err(SourceErrorCode::OverBudget);
        }
        if key.starts_with(b".") {
            continue;
        }
        let key = String::from_utf8_lossy(&key).into_owned();
        let Some(ids) = dir.child_dir(&key)? else {
            continue;
        };
        let slot = out.entry(key).or_default();
        for name in ids.names(budget)? {
            if let Some(id) = name.strip_suffix(b".txt") {
                slot.insert(String::from_utf8_lossy(id).into_owned());
            }
        }
    }
    Ok(())
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
        let canonical = std::fs::canonicalize(&rec.path).ok();
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
        Ok(Some(bytes))
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
}

struct ObservedSource {
    mailbox: PinnedDir,
    identity: SourceIdentity,
    messages: Vec<ObservedMessage>,
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
        let Some(bytes) = dir.read_file(OsStr::from_bytes(&name), budget.max_message_bytes)? else {
            // Listed but gone: the source changed under the read.
            return Err(SourceErrorCode::SourceChanged);
        };
        budget.take(bytes.len())?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| SourceErrorCode::CorruptMessage)?;
        let recorded = Recorded::of(&value);
        let message: Message =
            serde_json::from_value(value).map_err(|_| SourceErrorCode::CorruptMessage)?;
        if !safe_id(&message.id) {
            return Err(SourceErrorCode::CorruptMessage);
        }
        messages.push(ObservedMessage {
            fingerprint: fingerprint(&bytes),
            message,
            recorded,
        });
    }
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
    }))
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
    audience: &ResolvedAudience,
    include_own_sent: bool,
) -> (Vec<(Message, Vec<Provenance>)>, Conflicts) {
    let reaches = |m: &Message| {
        !audience.matching_keys(m).is_empty() || (include_own_sent && audience.sent_by_own(m))
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

/// Exact per-ID lookups only (no receipt directory scan). An explicit
/// receipt in any LOCAL source for a key the message is addressed to is
/// acknowledgement. A LOCAL watermark counts only for IDs physically present
/// in that same source, and only as legacy compatibility evidence.
/// Unreadable receipt evidence, or a deadline reached mid-lookup, marks that
/// source as errored.
#[allow(clippy::too_many_arguments)]
fn read_evidence(
    message: &Message,
    provenance: &[Provenance],
    observed: &[Option<ObservedSource>],
    local_count: usize,
    audience: &ResolvedAudience,
    sources: &mut [SourceReport],
    cache: &mut ReceiptCache,
    budget: &mut Budget,
) -> ReadEvidence {
    let keys = audience.matching_keys(message);
    let holders: BTreeSet<usize> = provenance.iter().map(|p| p.source).collect();
    let mut legacy = None;
    for (index, source) in observed.iter().enumerate().take(local_count) {
        let Some(source) = source else { continue };
        for key in &keys {
            if budget.expired() {
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
                    sources[index].status = SourceStatus::Error(*code);
                    continue;
                }
            };
            if let Some(dir) = &evidence.receipts {
                match receipt_at(dir, &message.id, budget) {
                    Ok(Some(at)) => {
                        return ReadEvidence::Acknowledged {
                            source: index,
                            key: key.to_string(),
                            at,
                        }
                    }
                    Ok(None) => {}
                    Err(code) => sources[index].status = SourceStatus::Error(code),
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

#[cfg(test)]
#[path = "mailbox_logical_tests.rs"]
mod tests;
