//! Relay end notices (STORY-1843 D2b): the durable outbox.
//!
//! When a relay run ends with mail its sessions never read, the mail stays
//! with the run (never deleted, renamed or acknowledged) and the people who
//! sent it are told. Each notice is one outbox entry keyed by (run UUID,
//! terminal generation, original delivery, destination intent), created
//! with create-new under the handle namespace lock, so a crash or a retry
//! never inserts a second logical notice. One entry goes to the sender and
//! one to the advisor role; a sender who is also an advisor is told once. A
//! delivery that is itself an end notice gets no notice: a bounce is never
//! bounced.
//!
//! Lock order: entries are inserted and marked under the namespace lock;
//! the notice mail itself is written with no lock held, because a mailbox
//! write takes the mailbox Lifecycle lock, which is never taken under the
//! namespace lock. A destination that cannot be resolved now (a dead or
//! unknown sender, no live advisor) stays pending and visible
//! (`aida relay-mail outbox`), and is retried by the next `relay-mail end`,
//! `relay-mail bind` or `aida agent gc`. Ordinary recipient resolution is
//! never weakened to force a notice through.
// trace:STORY-1843 | ai:claude

use super::*;
use aida_core::mailbox_delivery::{DeliverySidecar, RecipientUuid};
use aida_core::mailbox_logical::{MailboxContext, ReadLimits, RelayRunDelivery};
use sha2::{Digest, Sha256};

const OUTBOX_DIR: &str = "outbox";
const OUTBOX_SCHEMA: u32 = 1;
const MAX_OUTBOX_ENTRIES: usize = 4096;
const MAX_OUTBOX_ENTRY_BYTES: u64 = 65536;
const ADVISOR_ROLE: &str = "advisor";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DestinationKind {
    Sender,
    Advisor,
}

/// Who a notice goes to. `intent` is stable from insertion on; `recipient`
/// is the sender's recipient UUID when its handle reservation named one at
/// insertion, and a notice is delivered only while that still holds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Destination {
    pub(crate) kind: DestinationKind,
    pub(crate) intent: String,
    pub(crate) address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) recipient: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "state")]
pub(crate) enum NoticeState {
    Pending {
        attempts: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_error: Option<String>,
    },
    Delivered {
        /// The notice message; `None` when the advisor notice was folded
        /// into the sender's (the sender is the only advisor reader).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        at: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OutboxEntry {
    pub(crate) schema: u32,
    pub(crate) key: String,
    pub(crate) run: String,
    pub(crate) label: String,
    pub(crate) terminal_generation: u64,
    pub(crate) terminal_reason: TerminalReason,
    pub(crate) delivery_id: String,
    pub(crate) message_id: String,
    pub(crate) thread_id: String,
    pub(crate) original_from: String,
    /// The original subject, for the sender's own notice only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subject: Option<String>,
    pub(crate) destination: Destination,
    pub(crate) inserted_at: DateTime<Utc>,
    pub(crate) state: NoticeState,
}

impl OutboxEntry {
    pub(crate) fn pending(&self) -> bool {
        matches!(self.state, NoticeState::Pending { .. })
    }

    /// The notice message's ID: derived from the key, so a retried write
    /// replaces the same message rather than adding another.
    fn notice_message_id(&self) -> String {
        let h = &self.key;
        format!(
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        )
    }
}

fn outbox_key(run: &str, generation: u64, delivery: &str, intent: &str) -> String {
    let digest = Sha256::digest(format!("{run}\n{generation}\n{delivery}\n{intent}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A run's outbox: beside its live record, or in the archive once the run
/// is archived (nothing is written there after that).
fn outbox_dir(project_root: &Path, run_id: &str) -> PathBuf {
    let archived = ended_runs_dir(project_root).join(run_id);
    if archived.is_dir() {
        return archived.join(OUTBOX_DIR);
    }
    runs_dir(project_root).join(run_id).join(OUTBOX_DIR)
}

fn entry_path(project_root: &Path, run_id: &str, key: &str) -> PathBuf {
    outbox_dir(project_root, run_id).join(format!("{key}.json"))
}

fn load_entry(path: &Path) -> Result<Option<OutboxEntry>> {
    let Some(body) = agent_handle::read_allocation_text(path, MAX_OUTBOX_ENTRY_BYTES)? else {
        return Ok(None);
    };
    let entry: OutboxEntry = serde_json::from_str(&body).context("invalid relay outbox entry")?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if entry.schema != OUTBOX_SCHEMA || entry.key != stem {
        bail!(
            "relay outbox entry {} does not match its name",
            path.display()
        );
    }
    Ok(Some(entry))
}

/// Every outbox entry of `run_id`, bounded.
pub(crate) fn list_entries(project_root: &Path, run_id: &str) -> Result<Vec<OutboxEntry>> {
    let entries = match std::fs::read_dir(outbox_dir(project_root, run_id)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context("reading relay outbox"),
    };
    let mut out = Vec::new();
    for (n, entry) in entries.enumerate() {
        if n >= MAX_OUTBOX_ENTRIES {
            bail!("relay outbox entry budget exceeded for run {run_id}");
        }
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Some(e) = load_entry(&path)? {
            out.push(e);
        }
    }
    out.sort_by(|a, b| (a.inserted_at, &a.key).cmp(&(b.inserted_at, &b.key)));
    Ok(out)
}

fn write_entry(project_root: &Path, entry: &OutboxEntry) -> Result<()> {
    let body = serde_json::to_string_pretty(entry)?;
    aida_core::write_atomic(
        &entry_path(project_root, &entry.run, &entry.key),
        body.as_bytes(),
    )
    .context("writing relay outbox entry")
}

/// Create `entry` unless its key exists. Exactly-once logical insertion.
fn create_entry(project_root: &Path, entry: &OutboxEntry) -> Result<bool> {
    use std::io::Write;
    let dir = outbox_dir(project_root, &entry.run);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating relay outbox {}", dir.display()))?;
    let path = entry_path(project_root, &entry.run, &entry.key);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e).context("creating relay outbox entry"),
    };
    file.write_all(serde_json::to_string_pretty(entry)?.as_bytes())?;
    file.sync_all()?;
    Ok(true)
}

fn store_root(project_root: &Path) -> PathBuf {
    project_root.join(".aida-store")
}

fn mailbox_context(project_root: &Path) -> Result<MailboxContext> {
    let store = store_root(project_root);
    let store = std::fs::symlink_metadata(&store).is_ok().then_some(store);
    MailboxContext::resolve_or_local(project_root, store.as_deref())
        .map_err(|code| anyhow::anyhow!("cannot read the relay run's mail: {code:?}"))
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct InsertReport {
    pub(crate) inserted: usize,
    /// Undelivered deliveries the snapshot could not fully observe; a later
    /// pass inserts what it missed under the same keys.
    pub(crate) partial: bool,
}

/// The destinations one undelivered delivery is reported to.
fn destinations(project_root: &Path, label: &str, d: &RelayRunDelivery) -> Vec<Destination> {
    let mut out = Vec::new();
    let from = d.from.trim();
    if !from.is_empty() && !from.eq_ignore_ascii_case(label) {
        let recipient = agent_handle::checked_reservation(project_root, from)
            .ok()
            .flatten()
            .and_then(|r| RecipientUuid::parse(&r.recipient_id))
            .map(|r| r.as_str().to_string());
        out.push(Destination {
            kind: DestinationKind::Sender,
            intent: format!("sender:{}", from.to_lowercase()),
            address: from.to_string(),
            recipient,
        });
    }
    if !from.eq_ignore_ascii_case(ADVISOR_ROLE) {
        out.push(Destination {
            kind: DestinationKind::Advisor,
            intent: format!("role:{ADVISOR_ROLE}"),
            address: ADVISOR_ROLE.to_string(),
            recipient: None,
        });
    }
    out
}

/// The notices an ending run owes: each undelivered delivery (never one
/// that is itself a notice) with each of its destinations. Taken with no
/// lock held.
pub(crate) struct NoticePlan {
    items: Vec<(RelayRunDelivery, Destination)>,
    complete: bool,
}

pub(crate) fn plan_end_notices(project_root: &Path, run: &RelayRun) -> Result<NoticePlan> {
    let uuid = RecipientUuid::parse(&run.run_id).context("relay run id is not a recipient UUID")?;
    let ctx = mailbox_context(project_root)?;
    let found = aida_core::mailbox_logical::relay_run_unread_deliveries(
        &ctx,
        &run.label,
        &uuid,
        &ReadLimits::explicit_read(),
    );
    let mut items = Vec::new();
    // A delivery that is itself an end notice is never reported back.
    for d in found.unread.into_iter().filter(|d| !d.notice) {
        for destination in destinations(project_root, &run.label, &d) {
            items.push((d.clone(), destination));
        }
    }
    Ok(NoticePlan {
        items,
        complete: found.complete,
    })
}

/// Create the planned entries of ended `run` under the held namespace
/// lock. Keys already present are left as they are.
pub(crate) fn insert_planned(
    project_root: &Path,
    run: &RelayRun,
    plan: &NoticePlan,
    _lock: &agent_handle::NamespaceLock,
) -> Result<usize> {
    let Some(terminal) = &run.terminal else {
        return Ok(0);
    };
    let now = Utc::now();
    let mut inserted = 0;
    for (d, destination) in &plan.items {
        let entry = OutboxEntry {
            schema: OUTBOX_SCHEMA,
            key: outbox_key(
                &run.run_id,
                terminal.generation,
                &d.delivery_id,
                &destination.intent,
            ),
            run: run.run_id.clone(),
            label: run.label.clone(),
            terminal_generation: terminal.generation,
            terminal_reason: terminal.reason,
            delivery_id: d.delivery_id.clone(),
            message_id: d.message_id.clone(),
            thread_id: d.thread_id.clone(),
            original_from: d.from.clone(),
            subject: (destination.kind == DestinationKind::Sender)
                .then(|| d.subject.clone())
                .flatten(),
            destination: destination.clone(),
            inserted_at: now,
            state: NoticeState::Pending {
                attempts: 0,
                last_error: None,
            },
        };
        if create_entry(project_root, &entry)? {
            inserted += 1;
        }
    }
    Ok(inserted)
}

/// Insert the end notices of ended run `run_id` (a catch-up pass after the
/// terminal record). The snapshot is taken with no lock held.
pub(crate) fn insert_end_notices(project_root: &Path, run_id: &str) -> Result<InsertReport> {
    let Some(run) = load_run(project_root, run_id)? else {
        bail!("unknown relay run {run_id}");
    };
    if run.terminal.is_none() {
        return Ok(InsertReport::default());
    }
    let plan = plan_end_notices(project_root, &run)?;
    let mut report = InsertReport {
        inserted: 0,
        partial: !plan.complete,
    };
    if plan.items.is_empty() {
        return Ok(report);
    }
    let lock = agent_handle::lock_namespace(project_root)?;
    report.inserted = insert_planned(project_root, &run, &plan, &lock)?;
    Ok(report)
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct SettleReport {
    pub(crate) inserted: usize,
    /// Ended runs whose undelivered mail could not be fully read; retried.
    pub(crate) partial_runs: usize,
    pub(crate) delivered: usize,
    pub(crate) pending: usize,
    /// Settled runs moved out of the live records.
    pub(crate) archived: usize,
}

/// Maintenance for ended runs: insert any end notice not yet inserted
/// (until one complete pass after the run ended), then deliver every
/// pending notice. Run by `end`, `bind` and `aida agent gc`, never by send.
pub(crate) fn settle_ended_runs(project_root: &Path) -> Result<SettleReport> {
    debug_assert!(!agent_handle::namespace_lock_held());
    let mut report = SettleReport::default();
    for run in list_runs(project_root)?
        .into_iter()
        .filter(|r| r.terminal.is_some() && !r.notices_settled)
    {
        let inserted = insert_end_notices(project_root, &run.run_id)?;
        report.inserted += inserted.inserted;
        if inserted.partial {
            report.partial_runs += 1;
            continue;
        }
        let _lock = agent_handle::lock_namespace(project_root)?;
        if let Some(mut current) = load_run(project_root, &run.run_id)? {
            current.notices_settled = true;
            write_run(project_root, &current)?;
        }
    }
    let delivered = deliver_pending_notices(project_root)?;
    report.delivered = delivered.delivered;
    report.pending = delivered.pending;
    report.archived = archive_settled_runs(project_root)?;
    Ok(report)
}

/// Archive every ended run whose notices are all inserted and delivered,
/// so it stops counting against the live-record budget. A run with a
/// pending notice stays live and is retried by the next maintenance pass.
fn archive_settled_runs(project_root: &Path) -> Result<usize> {
    let mut archived = 0;
    for run in list_runs(project_root)?
        .into_iter()
        .filter(|r| r.terminal.is_some() && r.notices_settled)
    {
        if list_entries(project_root, &run.run_id)?
            .iter()
            .any(OutboxEntry::pending)
        {
            continue;
        }
        let _lock = agent_handle::lock_namespace(project_root)?;
        let Some(current) = load_run(project_root, &run.run_id)? else {
            continue;
        };
        if !(current.terminal.is_some() && current.notices_settled)
            || list_entries(project_root, &run.run_id)?
                .iter()
                .any(OutboxEntry::pending)
        {
            continue;
        }
        archive_run(project_root, &current)?;
        archived += 1;
    }
    Ok(archived)
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct DeliverReport {
    pub(crate) delivered: usize,
    pub(crate) pending: usize,
}

/// A notice ready to write: the message and its delivery record.
struct Prepared {
    message: aida_core::mailbox::Message,
    sidecar: Option<DeliverySidecar>,
}

#[cfg(test)]
thread_local! {
    /// Whether the namespace lock was held at each notice mail write.
    pub(crate) static LOCK_AT_MAIL_WRITE: std::cell::RefCell<Vec<bool>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Deliver every pending notice of every ended run. Never called with the
/// namespace lock held.
pub(crate) fn deliver_pending_notices(project_root: &Path) -> Result<DeliverReport> {
    let mut report = DeliverReport::default();
    let mut directory = None;
    for run in list_runs(project_root)?
        .into_iter()
        .filter(|r| r.terminal.is_some())
    {
        let entries = list_entries(project_root, &run.run_id)?;
        if !entries.iter().any(OutboxEntry::pending) {
            continue;
        }
        let directory = match &mut directory {
            Some(d) => d,
            None => directory.insert(crate::mailbox_resolver::RecipientDirectory::load(
                project_root,
                &store_root(project_root),
            )?),
        };
        for entry in entries.iter().filter(|e| e.pending()) {
            let outcome = prepare(project_root, directory, entry, &entries).and_then(|p| {
                let Some(p) = p else {
                    return Ok(None);
                };
                #[cfg(test)]
                LOCK_AT_MAIL_WRITE
                    .with(|v| v.borrow_mut().push(agent_handle::namespace_lock_held()));
                debug_assert!(!agent_handle::namespace_lock_held());
                crate::mailbox_store::write_message_with_deliveries(
                    project_root,
                    &p.message,
                    p.sidecar.as_ref(),
                )?;
                Ok(Some(p.message.id))
            });
            let _lock = agent_handle::lock_namespace(project_root)?;
            let path = entry_path(project_root, &entry.run, &entry.key);
            let Some(mut current) = load_entry(&path)? else {
                continue;
            };
            if !current.pending() {
                continue;
            }
            match outcome {
                Ok(message_id) => {
                    current.state = NoticeState::Delivered {
                        message_id,
                        at: Utc::now(),
                    };
                    report.delivered += 1;
                }
                Err(e) => {
                    let attempts = match current.state {
                        NoticeState::Pending { attempts, .. } => attempts + 1,
                        NoticeState::Delivered { .. } => unreachable!("checked pending"),
                    };
                    current.state = NoticeState::Pending {
                        attempts,
                        last_error: Some(format!("{e:#}")),
                    };
                    report.pending += 1;
                }
            }
            write_entry(project_root, &current)?;
        }
    }
    Ok(report)
}

/// The notice mail for `entry`, or `None` when nobody is left to tell (an
/// advisor notice whose only reader already got the sender's notice).
/// Refuses (stays pending) when the destination cannot be resolved now.
fn prepare(
    project_root: &Path,
    directory: &crate::mailbox_resolver::RecipientDirectory,
    entry: &OutboxEntry,
    siblings: &[OutboxEntry],
) -> Result<Option<Prepared>> {
    use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
    let id = entry.notice_message_id();
    let reason = format!("{:?}", entry.terminal_reason).to_lowercase();
    let (address, sidecar, subject, body, thread_id, in_reply_to) = match entry.destination.kind {
        DestinationKind::Sender => {
            let resolved = directory.resolve(&entry.destination.address)?;
            let sidecar = match &entry.destination.recipient {
                Some(recipient) => {
                    let owner = agent_handle::checked_reservation(project_root, &resolved.address)?
                        .map(|r| r.recipient_id);
                    if owner.as_deref() != Some(recipient.as_str()) {
                        bail!(
                            "the sender's handle `{}` now belongs to another recipient",
                            resolved.address
                        );
                    }
                    let recipient = RecipientUuid::parse(recipient)
                        .context("stored sender recipient is not a UUID")?;
                    Some(DeliverySidecar::notice(&id, &recipient, &entry.delivery_id))
                }
                None => crate::mailbox_resolver::delivery_sidecar(&resolved, &id, now_ms(), false)?
                    .0
                    .map(|s| s.as_notice_of(&entry.delivery_id)),
            };
            let about = entry
                .subject
                .clone()
                .unwrap_or_else(|| format!("message {}", entry.message_id));
            (
                resolved.address,
                sidecar,
                format!("Relay {} ended before reading: {about}", entry.label),
                format!(
                    "Relay run {} ({}) ended ({reason}) without reading your message {}. \
                     The message stays with that run; it was not deleted or acknowledged.",
                    entry.label, entry.run, entry.message_id
                ),
                entry.thread_id.clone(),
                Some(entry.message_id.clone()),
            )
        }
        DestinationKind::Advisor => {
            let resolved = directory.resolve_send(ADVISOR_ROLE, false)?;
            // The sender is told directly; an advisor who sent the message
            // is not told twice.
            let told: Vec<&str> = siblings
                .iter()
                .filter(|s| {
                    s.delivery_id == entry.delivery_id
                        && s.destination.kind == DestinationKind::Sender
                })
                .filter_map(|s| s.destination.recipient.as_deref())
                .collect();
            let Some(cohort) = &resolved.cohort else {
                bail!("the advisor role did not resolve to a reader cohort");
            };
            let readers: Vec<RecipientUuid> = cohort
                .readers
                .iter()
                .filter(|r| !told.contains(&r.as_str()))
                .cloned()
                .collect();
            if readers.is_empty() && !cohort.readers.is_empty() {
                return Ok(None);
            }
            if readers.is_empty() {
                bail!("the advisor role has no reader with a recipient UUID");
            }
            (
                resolved.address,
                Some(
                    DeliverySidecar::cohort(&id, &cohort.role, now_ms(), &readers)
                        .as_notice_of(&entry.delivery_id),
                ),
                format!("Relay {} ended with undelivered mail", entry.label),
                format!(
                    "Relay run {} ({}) ended ({reason}) with an unread message from {} ({}). \
                     The message stays with that run.",
                    entry.label, entry.run, entry.original_from, entry.message_id
                ),
                id.clone(),
                None,
            )
        }
    };
    Ok(Some(Prepared {
        message: Message {
            id,
            thread_id,
            from: entry.label.clone(),
            to: Recipient::Agent(address),
            timestamp: now_ms(),
            in_reply_to,
            body,
            subject: Some(subject),
            urgent: false,
            intent: Intent::Fyi,
            retracted: false,
            deleted: false,
            archived: false,
            from_source: SenderSource::Explicit,
            from_role: None,
            relayed_from: None,
        },
        sidecar,
    }))
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
