//! Advisor handshake (`aida mailbox hello`): a launched non-advisor agent's
//! first act is a mail round trip with the advisor, so a broken mail path
//! shows up within minutes instead of when an operator relays mail by hand.
//!
//! - **Hello.** On SessionStart, a session `aida agent new` launched with its
//!   own handle, whose role is not advisor, mails the `advisor` role once, naming its handle, role, spec and
//!   worktree/clone path. With no registered advisor the send is queued for
//!   the role (durable) and the agent is told so.
//! - **Ack.** An advisor's per-turn notice lists unacknowledged hellos;
//!   reading them with `aida mailbox inbox` (or MCP `read_inbox` with
//!   `mark_seen`) acks each one with a reply to the sender's handle.
//! - **Unreachable.** With no ack after the window (default 10 minutes,
//!   `[mailbox] handshake_timeout_mins`), the agent's notice, `aida agent ls`
//!   and `aida doctor` say `advisor unreachable: <reason>`, naming the mailbox
//!   the hello was written to.
//!
//! The hello is ordinary mail typed by its thread id (`aida-hello-<handle>`:
//! one advisor thread per handle, across rotations), so
//! the message schema is unchanged. The sender keeps a per-handle record at
//! `<mailbox home>/.aida/mailbox/.handshake/<handle>.json`: inside the
//! mailbox directory a contained seat can already write.
// trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude

use std::path::{Path, PathBuf};

use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::agent_registry::AgentRegistryEntry;
use crate::mailbox_store::{self, MailboxHome};
use crate::DoctorFinding;

/// Thread-id prefix that marks a message as a handshake hello.
pub(crate) const THREAD_PREFIX: &str = "aida-hello-";
pub(crate) const DOCTOR_CATEGORY: &str = "advisor-handshake";
pub(crate) const ADVISOR_ROLE: &str = "advisor";
/// `AIDA_ADVISOR_HANDSHAKE=0` turns the handshake off for one session.
pub(crate) const DISABLE_ENV: &str = "AIDA_ADVISOR_HANDSHAKE";
const DEFAULT_WINDOW_MINS: i64 = 10;
/// An advisor's notice lists unacked hellos sent within this horizon.
const ADVISOR_NOTICE_HORIZON_HOURS: i64 = 24;
const RECORD_DIR: &str = ".handshake";

/// `[mailbox] advisor_handshake` / `handshake_timeout_mins`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Policy {
    pub enabled: bool,
    pub window: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: true,
            window: Duration::minutes(DEFAULT_WINDOW_MINS),
        }
    }
}

impl Policy {
    pub(crate) fn from_config(cfg: Option<&toml::Value>) -> Self {
        let table = cfg.and_then(|c| c.get("mailbox"));
        let enabled = table
            .and_then(|t| t.get("advisor_handshake"))
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        let mins = table
            .and_then(|t| t.get("handshake_timeout_mins"))
            .and_then(toml::Value::as_integer)
            .filter(|m| *m > 0)
            .unwrap_or(DEFAULT_WINDOW_MINS);
        Self {
            enabled,
            window: Duration::minutes(mins),
        }
    }

    pub(crate) fn load(project_root: &Path) -> Self {
        let disabled = std::env::var(DISABLE_ENV).is_ok_and(|v| v.trim() == "0");
        let policy = Self::from_config(crate::read_project_config_value(project_root).as_ref());
        Self {
            enabled: policy.enabled && !disabled,
            ..policy
        }
    }
}

/// Which mailbox the hello was written to (see [`MailboxHome`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum HomeKind {
    Checkout,
    SeatPrimary,
    UnverifiedClone,
}

impl HomeKind {
    pub(crate) fn of(home: &MailboxHome) -> Self {
        match home {
            MailboxHome::Checkout(_) => Self::Checkout,
            MailboxHome::SeatPrimary(_) => Self::SeatPrimary,
            MailboxHome::UnverifiedClone(_) => Self::UnverifiedClone,
        }
    }
}

/// The sender's record of its hello.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HelloRecord {
    pub handle: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    /// The worktree or clone the agent runs in.
    pub path: String,
    pub hello_id: String,
    pub sent_at: DateTime<Utc>,
    /// The mailbox directory the hello was written to.
    pub mailbox: String,
    pub home: HomeKind,
    /// A live advisor could read the role's mail when the hello was sent.
    pub advisor_live: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked_by: Option<String>,
    /// The agent's notice already announced the ack.
    #[serde(default)]
    pub ack_reported: bool,
    /// Set when `aida agent new --rotate` relaunched this handle after a
    /// context rotation: the generation (2 on) and the session it replaced.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<Rotation>,
}

/// A rotated session's continuity with the one before it.
// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Rotation {
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_native_session_id: Option<String>,
}

impl Rotation {
    /// The rotation this session runs as, from the supervisor's env.
    pub(crate) fn from_env() -> Option<Self> {
        let generation: u64 = env_nonempty(crate::agent_rotation::GENERATION_ENV)?
            .parse()
            .ok()?;
        (generation >= 2).then(|| Self {
            generation,
            previous_native_session_id: env_nonempty(crate::agent_rotation::PREVIOUS_SESSION_ENV),
        })
    }
}

/// PURE: `handle`, or `handle #N (rotated)` for a rotated generation.
// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
pub(crate) fn handle_label(handle: &str, rotation: Option<&Rotation>) -> String {
    match rotation {
        Some(r) => format!("{handle} #{} (rotated)", r.generation),
        None => handle.to_string(),
    }
}

/// Where a hello stands for its sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum State {
    Acked { by: Option<String> },
    Waiting,
    Unreachable(String),
}

/// PURE: whether this session is a launched non-advisor agent that says
/// hello. `agent_name` is `AIDA_AGENT_NAME`, which `aida agent new` (and its
/// seats) sets to the handle the session runs under; `own_handle` is the
/// handle this session resolves to. Only a session launched with its own
/// handle says hello: one that inherited a parent's `AIDA_AGENT_NAME` (a
/// parent-only startup) or a headless queue child, whose `AIDA_AGENT_NAME`
/// is its seat label beside a separate `AIDA_HANDLE`, does not.
pub(crate) fn eligible(
    agent_name: Option<&str>,
    own_handle: Option<&str>,
    role: Option<&str>,
) -> bool {
    let launched = agent_name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .is_some_and(|n| own_handle.map(str::trim) == Some(n));
    let role = role
        .map(crate::canonical_role_name)
        .filter(|r| !r.is_empty());
    launched && role.is_some_and(|r| r != ADVISOR_ROLE)
}

/// PURE: is `m` a handshake hello addressed to the advisor role?
pub(crate) fn is_hello(m: &Message) -> bool {
    m.thread_id.starts_with(THREAD_PREFIX)
        && m.in_reply_to.is_none()
        && !m.deleted
        && !m.retracted
        && matches!(&m.to, Recipient::Agent(a) if crate::canonical_role_name(a) == ADVISOR_ROLE)
}

/// PURE: the ack's message id, derived from the hello's so a second read of
/// the same hello never writes a second ack.
pub(crate) fn ack_id(hello_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("aida-hello-ack:{hello_id}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_random_bytes(bytes)
        .into_uuid()
        .to_string()
}

/// What this process can tell about the advisor seat. The agent registry is
/// one witness, not the only one: a live advisor whose registry row is
/// missing still holds its seat grant, so "no registered advisor" is never
/// reported as "no live advisor".
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdvisorPresence {
    /// A live registry row carries the advisor role.
    Registered,
    /// No registry row, but other evidence shows a live advisor; the string
    /// names it (a seat grant held by a live process, a recent advisor turn).
    Unregistered(String),
    /// No registered advisor and no other evidence of one.
    Unknown,
}

/// The advisor's presence as `live` (the live registry rows) and the
/// advisor seat grants show it. Grants are read only when no row is found.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
pub(crate) fn advisor_presence(
    project_root: &Path,
    live: &[AgentRegistryEntry],
    now: DateTime<Utc>,
) -> AdvisorPresence {
    if !live_advisors(live).is_empty() {
        return AdvisorPresence::Registered;
    }
    let grants = crate::gate::seat_authority::held_grants(ADVISOR_ROLE, now);
    let recent_turn = crate::mandate_drive::advisor_live(project_root, now);
    presence_from(live, &grants, recent_turn)
}

/// PURE core of [`advisor_presence`]: `recent_turn` is whether an advisor
/// grant's per-turn notice ran recently.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
pub(crate) fn presence_from(
    live: &[AgentRegistryEntry],
    grants: &[crate::gate::seat_authority::HeldGrant],
    recent_turn: bool,
) -> AdvisorPresence {
    if !live_advisors(live).is_empty() {
        return AdvisorPresence::Registered;
    }
    if let Some((grant, pid)) = grants
        .iter()
        .find_map(|g| g.live_holder().map(|pid| (g, pid)))
    {
        let prefix: String = grant.id.chars().take(8).collect();
        return AdvisorPresence::Unregistered(format!(
            "pid {pid} holds advisor seat grant {prefix} until {}",
            grant.expires_at.format("%Y-%m-%dT%H:%MZ")
        ));
    }
    if recent_turn {
        return AdvisorPresence::Unregistered(
            "an advisor seat grant ran a turn within the last 30 minutes".to_string(),
        );
    }
    AdvisorPresence::Unknown
}

/// The hello's state at `now`. `presence` is asked only when the hello is
/// past its window unacked, so a quiet per-turn hook reads no grants.
pub(crate) fn state(
    rec: &HelloRecord,
    presence: impl FnOnce() -> AdvisorPresence,
    policy: &Policy,
    now: DateTime<Utc>,
) -> State {
    if rec.acked_at.is_some() {
        return State::Acked {
            by: rec.acked_by.clone(),
        };
    }
    if now - rec.sent_at < policy.window {
        return State::Waiting;
    }
    State::Unreachable(unreachable_reason(rec, &presence(), policy))
}

/// PURE: why no ack arrived, naming the mailbox path the hello used.
pub(crate) fn unreachable_reason(
    rec: &HelloRecord,
    presence: &AdvisorPresence,
    policy: &Policy,
) -> String {
    if rec.home == HomeKind::UnverifiedClone {
        return format!(
            "the hello stayed in this seat clone's local mailbox ({}), which the primary inbox does not read; run `aida mailbox sync` to publish it",
            rec.mailbox
        );
    }
    match presence {
        AdvisorPresence::Unknown => {
            return format!(
                "no registered advisor (no advisor row for a running process in the agent registry); the hello is queued in {}",
                rec.mailbox
            )
        }
        AdvisorPresence::Unregistered(evidence) => {
            return format!(
                "an advisor is live but unregistered ({evidence}); it has not acked within {}m and the hello is queued in {}",
                policy.window.num_minutes(),
                rec.mailbox
            )
        }
        AdvisorPresence::Registered => {}
    }
    format!(
        "the live advisor has not acked within {}m; the hello is in {}",
        policy.window.num_minutes(),
        rec.mailbox
    )
}

fn record_dir(root: &Path) -> PathBuf {
    mailbox_store::mailbox_dir(root).join(RECORD_DIR)
}

fn record_path(root: &Path, handle: &str) -> PathBuf {
    record_dir(root).join(format!("{}.json", mailbox_store::sanitize_id(handle)))
}

/// The record for `handle` under `root`'s mailbox, if any.
pub(crate) fn load_record(root: &Path, handle: &str) -> Option<HelloRecord> {
    let body = std::fs::read_to_string(record_path(root, handle)).ok()?;
    serde_json::from_str(&body).ok()
}

pub(crate) fn save_record(root: &Path, rec: &HelloRecord) -> Result<()> {
    let dir = record_dir(root);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = record_path(root, &rec.handle);
    let body = serde_json::to_string_pretty(rec)?;
    aida_core::write_atomic(&path, body.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

fn load_records(root: &Path) -> Vec<HelloRecord> {
    let Ok(entries) = std::fs::read_dir(record_dir(root)) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|body| serde_json::from_str(&body).ok())
        .collect()
}

/// The roots whose mailbox may hold a record: the primary checkout (seat
/// clones and the primary write there), then each live agent's own checkout
/// (a linked worktree or an unverified clone writes its own).
pub(crate) fn record_roots(main: &Path, live: &[AgentRegistryEntry]) -> Vec<PathBuf> {
    let mut roots = vec![main.to_path_buf()];
    for e in live {
        if !roots.contains(&e.worktree_path) {
            roots.push(e.worktree_path.clone());
        }
    }
    roots
}

/// The newest record for `handle` across `roots`, so an earlier session's
/// record for a reused handle in the primary never shadows the current one
/// in a linked worktree or clone.
pub(crate) fn find_record(roots: &[PathBuf], handle: &str) -> Option<(PathBuf, HelloRecord)> {
    roots
        .iter()
        .filter_map(|root| load_record(root, handle).map(|rec| (root.clone(), rec)))
        .max_by_key(|(_, rec)| rec.sent_at)
}

fn is_live_advisor(e: &AgentRegistryEntry) -> bool {
    e.role
        .as_deref()
        .is_some_and(|r| crate::canonical_role_name(r) == ADVISOR_ROLE)
}

fn live_advisors(live: &[AgentRegistryEntry]) -> Vec<&AgentRegistryEntry> {
    live.iter().filter(|e| is_live_advisor(e)).collect()
}

/// Who and where the hello comes from.
#[derive(Debug, Clone)]
pub(crate) struct HelloContext {
    pub handle: String,
    pub from_source: SenderSource,
    pub role: String,
    pub spec: Option<String>,
    pub path: PathBuf,
    // trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
    pub rotation: Option<Rotation>,
    /// When this session's registry entry started; a record sent before it
    /// belongs to an earlier session that reused the handle.
    pub session_started: Option<DateTime<Utc>>,
}

/// PURE: whether `rec` was sent by the session that started at `started`
/// (or the session is unknown), rather than an earlier one with this handle.
pub(crate) fn sent_this_session(rec: &HelloRecord, started: Option<DateTime<Utc>>) -> bool {
    started.is_none_or(|s| rec.sent_at >= s)
}

/// What [`send_hello`] did.
#[derive(Debug)]
pub(crate) enum SendOutcome {
    /// A live advisor reads the role's mail.
    Sent(HelloRecord),
    /// No live advisor: the hello waits for the first one.
    Queued(HelloRecord),
    /// This handle already said hello (a resumed session).
    AlreadySent(HelloRecord),
}

impl SendOutcome {
    pub(crate) fn record(&self) -> &HelloRecord {
        match self {
            Self::Sent(r) | Self::Queued(r) | Self::AlreadySent(r) => r,
        }
    }
}

/// PURE: the thread every hello from `handle` belongs to.
// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
pub(crate) fn hello_thread_id(handle: &str) -> String {
    format!(
        "{THREAD_PREFIX}{}",
        mailbox_store::sanitize_id(handle.trim())
    )
}

pub(crate) fn hello_message(
    id: &str,
    ctx: &HelloContext,
    to: &str,
    mailbox: &Path,
    now: DateTime<Utc>,
) -> Message {
    let spec = ctx.spec.as_deref().unwrap_or("(none)");
    // A rotated session says hello as the same handle, naming its generation
    // and the session it replaced. trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
    let rotation = ctx
        .rotation
        .as_ref()
        .map(|r| {
            format!(
                "generation: {} (rotated; previous session {})\n",
                r.generation,
                r.previous_native_session_id.as_deref().unwrap_or("unknown")
            )
        })
        .unwrap_or_default();
    let body = format!(
        "Hello from {label}.\n\nhandle: {handle}\n{rotation}role: {role}\nspec: {spec}\npath: {path}\nmailbox: {mailbox}\n\n\
         Reading this with `aida mailbox inbox` acks it.",
        label = handle_label(&ctx.handle, ctx.rotation.as_ref()),
        handle = ctx.handle,
        role = ctx.role,
        path = ctx.path.display(),
        mailbox = mailbox.display(),
    );
    Message {
        id: id.to_string(),
        // One advisor thread per handle: every hello from this handle (a
        // rotated generation, or a later session reusing it) continues it.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
        thread_id: hello_thread_id(&ctx.handle),
        from: ctx.handle.clone(),
        to: Recipient::Agent(to.to_string()),
        timestamp: now.timestamp_millis(),
        in_reply_to: None,
        body,
        subject: Some(format!(
            "hello from {} ({})",
            handle_label(&ctx.handle, ctx.rotation.as_ref()),
            ctx.role
        )),
        urgent: false,
        intent: Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: ctx.from_source,
        from_role: Some(ctx.role.clone()),
        relayed_from: None,
    }
}

/// Mail the advisor role once per session for `ctx.handle`. With no live
/// advisor the send is queued for the role, as `--queue-for-role` does. A
/// record left by an earlier session with the same handle is replaced.
pub(crate) fn send_hello(
    home: &MailboxHome,
    store_root: &Path,
    ctx: &HelloContext,
    now: DateTime<Utc>,
) -> Result<SendOutcome> {
    let root = home.root();
    if let Some(rec) =
        load_record(root, &ctx.handle).filter(|r| sent_this_session(r, ctx.session_started))
    {
        return Ok(SendOutcome::AlreadySent(rec));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let directory = crate::mailbox_resolver::RecipientDirectory::load_for_send(root, store_root)?;
    let resolved = directory.resolve_send(ADVISOR_ROLE, true)?;
    let queued = resolved.cohort.as_ref().is_some_and(|c| c.queued);
    let (deliveries, _warnings) =
        crate::mailbox_resolver::delivery_sidecar(&resolved, &id, now.timestamp_millis(), true)?;
    let mailbox = mailbox_store::mailbox_dir(root);
    let msg = hello_message(&id, ctx, &resolved.address, &mailbox, now);
    mailbox_store::write_message_with_deliveries(root, &msg, deliveries.as_ref())?;
    crate::events::emit(
        root,
        &crate::events::Event::new(
            None,
            "",
            crate::events::EventKind::MailReceived {
                to: resolved.address.clone(),
            },
        ),
    );
    let rec = HelloRecord {
        handle: ctx.handle.clone(),
        role: ctx.role.clone(),
        spec: ctx.spec.clone(),
        path: ctx.path.display().to_string(),
        hello_id: id,
        sent_at: now,
        mailbox: mailbox.display().to_string(),
        home: HomeKind::of(home),
        advisor_live: !queued,
        acked_at: None,
        acked_by: None,
        ack_reported: false,
        rotation: ctx.rotation.clone(),
    };
    save_record(root, &rec)?;
    Ok(if queued {
        SendOutcome::Queued(rec)
    } else {
        SendOutcome::Sent(rec)
    })
}

fn read_message_at(path: &Path) -> Option<Message> {
    let bytes = std::fs::read(path).ok()?;
    aida_core::mailbox::parse_message_file(path, &bytes).ok()
}

/// The ack for `rec`, searched in `dirs` (mailbox directories).
pub(crate) fn find_ack(rec: &HelloRecord, dirs: &[PathBuf]) -> Option<Message> {
    let file = format!("{}.json", ack_id(&rec.hello_id));
    dirs.iter()
        .find_map(|d| read_message_at(&d.join(&file)))
        .filter(|m| !m.deleted && m.in_reply_to.as_deref() == Some(rec.hello_id.as_str()))
}

/// Mailbox directories an ack to the agent at `home_root` may land in.
fn ack_dirs(
    home_root: &Path,
    main: &Path,
    store_root: &Path,
    live: &[AgentRegistryEntry],
) -> Vec<PathBuf> {
    let mut dirs = vec![
        mailbox_store::mailbox_dir(home_root),
        mailbox_store::mailbox_dir(main),
        mailbox_store::canonical_dir(store_root),
    ];
    for e in live_advisors(live) {
        dirs.push(mailbox_store::mailbox_dir(
            mailbox_store::mailbox_home(&e.worktree_path).root(),
        ));
    }
    dirs.dedup();
    dirs
}

fn ack_message(
    hello: &Message,
    to: &str,
    acker: &str,
    from_source: SenderSource,
    now: DateTime<Utc>,
) -> Message {
    Message {
        id: ack_id(&hello.id),
        thread_id: hello.thread_id.clone(),
        from: acker.to_string(),
        to: Recipient::Agent(to.to_string()),
        timestamp: now.timestamp_millis(),
        in_reply_to: Some(hello.id.clone()),
        body: format!(
            "Ack from {acker}: your hello reached the advisor's inbox. The mail path works."
        ),
        subject: Some(format!("ack: hello from {}", hello.from)),
        urgent: false,
        intent: Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source,
        from_role: Some(ADVISOR_ROLE.to_string()),
        relayed_from: None,
    }
}

/// Ack every hello among `shown` (the mail an advisor's inbox read just
/// displayed): one reply to the sender's handle each, written once, and the
/// sender's record marked acked when this checkout can reach it. Returns the
/// handles whose ack this call wrote, so a re-read of an old hello reports
/// nothing. Best-effort per hello.
pub(crate) fn ack_hellos(
    home_root: &Path,
    store_root: &Path,
    shown: &[&Message],
    now: DateTime<Utc>,
) -> Vec<String> {
    let hellos: Vec<&Message> = shown.iter().copied().filter(|m| is_hello(m)).collect();
    if hellos.is_empty() {
        return Vec::new();
    }
    let (acker, from_source) = crate::resolve_mail_sender_identity(None);
    let main = crate::main_worktree_root_from(home_root);
    let live = crate::agent_registry::load_live_entries(&main);
    let roots = record_roots(&main, &live);
    let directory =
        crate::mailbox_resolver::RecipientDirectory::load_for_send(home_root, store_root).ok();
    let own_dirs = [
        mailbox_store::mailbox_dir(home_root),
        mailbox_store::canonical_dir(store_root),
    ];
    let mut acked = Vec::new();
    for hello in hellos {
        let exists = own_dirs
            .iter()
            .any(|d| d.join(format!("{}.json", ack_id(&hello.id))).exists());
        if !exists {
            let to = directory
                .as_ref()
                .and_then(|d| d.resolve(&hello.from).ok())
                .map(|r| r.address)
                .unwrap_or_else(|| hello.from.clone());
            let msg = ack_message(hello, &to, &acker, from_source, now);
            if let Err(e) = mailbox_store::write_message(home_root, &msg) {
                eprintln!(
                    "warning: could not ack the hello from {}: {e:#}",
                    hello.from
                );
                continue;
            }
            acked.push(hello.from.clone());
        }
        if let Some((root, mut rec)) = find_record(&roots, &hello.from) {
            if rec.hello_id == hello.id && rec.acked_at.is_none() {
                rec.acked_at = Some(now);
                rec.acked_by = Some(acker.clone());
                let _ = save_record(&root, &rec);
            }
        }
    }
    acked
}

/// A hello waiting for an advisor, as the advisor's notice lists it.
/// A hello stranded in an unverified clone never reached the advisor inbox,
/// so the line points at the clone's `aida mailbox sync` instead.
fn pending_line(rec: &HelloRecord, now: DateTime<Utc>) -> String {
    let how = if rec.home == HomeKind::UnverifiedClone {
        format!(
            "the hello is stranded in {}; run `aida mailbox sync` from that clone to deliver it",
            rec.mailbox
        )
    } else {
        "`aida mailbox inbox` acks it".to_string()
    };
    format!(
        "Advisor handshake: hello from {} ({} · {} · {}), waiting {} — {how}.",
        handle_label(&rec.handle, rec.rotation.as_ref()),
        rec.role,
        rec.spec.as_deref().unwrap_or("no spec"),
        rec.path,
        crate::mail_backlog::format_age(now - rec.sent_at)
    )
}

/// PURE: the advisor's notice lines for `records`: each unacked hello sent
/// within the horizon, oldest first.
pub(crate) fn advisor_lines(records: &[HelloRecord], now: DateTime<Utc>) -> Vec<String> {
    let horizon = Duration::hours(ADVISOR_NOTICE_HORIZON_HOURS);
    let mut pending: Vec<&HelloRecord> = records
        .iter()
        .filter(|r| r.acked_at.is_none() && now - r.sent_at < horizon)
        .collect();
    pending.sort_by_key(|r| r.sent_at);
    pending.iter().map(|r| pending_line(r, now)).collect()
}

/// The newest record per handle across `roots` (see [`find_record`]).
pub(crate) fn all_records(roots: &[PathBuf]) -> Vec<HelloRecord> {
    let mut newest = std::collections::BTreeMap::<String, HelloRecord>::new();
    for rec in roots.iter().flat_map(|root| load_records(root)) {
        match newest.get(&rec.handle) {
            Some(cur) if cur.sent_at >= rec.sent_at => {}
            _ => {
                newest.insert(rec.handle.clone(), rec);
            }
        }
    }
    newest.into_values().collect()
}

/// The agent's own notice line for `rec`, or `None` when there is nothing to
/// say. A newly seen ack is announced once.
fn agent_line(rec: &HelloRecord, st: &State) -> Option<String> {
    match st {
        State::Acked { by } if !rec.ack_reported => Some(format!(
            "Advisor handshake: ack from {} — the mail path to the advisor works.",
            by.as_deref().unwrap_or("the advisor")
        )),
        State::Acked { .. } | State::Waiting => None,
        State::Unreachable(reason) => Some(format!(
            "advisor unreachable: {reason}. Details: `aida doctor check {DOCTOR_CATEGORY}`."
        )),
    }
}

fn send_line(outcome: &SendOutcome, live: &[AgentRegistryEntry]) -> Option<String> {
    match outcome {
        SendOutcome::Sent(rec) => {
            let names: Vec<String> = live_advisors(live)
                .iter()
                .map(|e| crate::mail_backlog::handle(e))
                .collect();
            let to = if names.is_empty() {
                "the advisor".to_string()
            } else {
                names.join(", ")
            };
            Some(format!(
                "Advisor handshake: hello sent to {to} ({}); waiting for the ack.",
                rec.mailbox
            ))
        }
        // trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
        SendOutcome::Queued(rec) => Some(format!(
            "Advisor handshake: no registered advisor; hello queued ({}). Check: `aida doctor check {DOCTOR_CATEGORY}`.",
            rec.mailbox
        )),
        SendOutcome::AlreadySent(_) => None,
    }
}

/// The checkout and its primary: a verified seat clone's primary is its
/// mailbox home; anything else resolves through git.
fn main_root(home: &MailboxHome, checkout: &Path) -> PathBuf {
    match home {
        MailboxHome::SeatPrimary(primary) => primary.clone(),
        _ => crate::main_worktree_root_from(checkout),
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// `aida mailbox hello`: the agent's hello and status, or, for an advisor,
/// the hellos waiting for it. `hook` is the per-turn hook call: it sends
/// only on SessionStart and prints only what changed or is overdue.
pub(crate) fn run(
    home: &MailboxHome,
    checkout: &Path,
    store_root: &Path,
    hook: bool,
) -> Result<Vec<String>> {
    let main = main_root(home, checkout);
    let policy = Policy::load(&main);
    if !policy.enabled {
        return Ok(if hook {
            Vec::new()
        } else {
            vec!["Advisor handshake is off ([mailbox] advisor_handshake or AIDA_ADVISOR_HANDSHAKE=0).".into()]
        });
    }
    let now = Utc::now();
    let role = crate::resolve_mail_sender_role();
    if role.as_deref() == Some(ADVISOR_ROLE) {
        let live = crate::agent_registry::load_live_entries(&main);
        let lines = advisor_lines(&all_records(&record_roots(&main, &live)), now);
        if lines.is_empty() && !hook {
            return Ok(vec![
                "Advisor handshake: no hellos waiting for an ack.".into()
            ]);
        }
        return Ok(lines);
    }
    // Eligibility first, so a per-turn hook in any other session pays no
    // registry read.
    let agent_name = env_nonempty("AIDA_AGENT_NAME");
    let (handle, from_source) = crate::resolve_mail_sender_identity(None);
    if !eligible(agent_name.as_deref(), Some(&handle), role.as_deref()) {
        return Ok(if hook {
            Vec::new()
        } else {
            vec!["Advisor handshake: only agents launched with their own handle by `aida agent new` in a non-advisor role say hello.".into()]
        });
    }
    let live = crate::agent_registry::load_live_entries(&main);
    let is_start = {
        let payload = std::env::var("AIDA_HOOK_PAYLOAD").unwrap_or_default();
        let event = std::env::var("AIDA_HOOK_EVENT").ok();
        crate::presence::parse_hook_payload_with_event(&payload, event.as_deref()).1
    };
    let mut lines = Vec::new();
    let root = home.root().to_path_buf();
    let mut rec = if is_start || !hook {
        let entry = crate::agent_handle::current_session_entry_for_cwd();
        let ctx = HelloContext {
            handle: handle.clone(),
            from_source,
            role: role.clone().unwrap_or_default(),
            spec: env_nonempty("AIDA_SESSION_SCOPE")
                .or_else(|| entry.as_ref().and_then(|e| e.current_spec.clone())),
            session_started: entry.as_ref().map(|e| e.started_at),
            rotation: Rotation::from_env(),
            path: crate::find_aida_project_root_from(checkout)
                .unwrap_or_else(|_| checkout.to_path_buf()),
        };
        let outcome = send_hello(home, store_root, &ctx, now)?;
        if let Some(note) = home
            .send_note()
            .filter(|_| !matches!(outcome, SendOutcome::AlreadySent(_)))
        {
            lines.push(format!("  {note}"));
        }
        lines.splice(0..0, send_line(&outcome, &live));
        outcome.record().clone()
    } else {
        match load_record(&root, &handle) {
            Some(rec) => rec,
            None => return Ok(lines),
        }
    };
    if rec.acked_at.is_none() {
        if let Some(ack) = find_ack(&rec, &ack_dirs(&root, &main, store_root, &live)) {
            rec.acked_at = Some(now);
            rec.acked_by = Some(ack.from);
        }
    }
    let st = state(&rec, || advisor_presence(&main, &live, now), &policy, now);
    if let Some(line) = agent_line(&rec, &st) {
        lines.push(line);
    } else if !hook && st == State::Waiting {
        lines.push(format!(
            "Advisor handshake: hello {} sent {} ago; waiting for the ack.",
            rec.hello_id,
            crate::mail_backlog::format_age(now - rec.sent_at)
        ));
    }
    let announce = matches!(st, State::Acked { .. }) && !rec.ack_reported;
    let stored = load_record(&root, &handle);
    if announce {
        rec.ack_reported = true;
    }
    if stored.as_ref() != Some(&rec) {
        let _ = save_record(&root, &rec);
    }
    Ok(lines)
}

/// What `aida agent ls` and `aida doctor` read to judge each agent's hello,
/// so both report the same agents unreachable.
pub(crate) struct Census {
    main: PathBuf,
    store_root: PathBuf,
    live: Vec<AgentRegistryEntry>,
    roots: Vec<PathBuf>,
    policy: Policy,
    now: DateTime<Utc>,
    presence: std::cell::OnceCell<AdvisorPresence>,
}

impl Census {
    pub(crate) fn new(
        main: &Path,
        store_root: PathBuf,
        live: Vec<AgentRegistryEntry>,
        policy: Policy,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            roots: record_roots(main, &live),
            main: main.to_path_buf(),
            store_root,
            live,
            policy,
            now,
            presence: std::cell::OnceCell::new(),
        }
    }

    /// The advisor's presence, read once per census.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
    pub(crate) fn presence(&self) -> AdvisorPresence {
        self.presence
            .get_or_init(|| advisor_presence(&self.main, &self.live, self.now))
            .clone()
    }

    pub(crate) fn load(project_root: &Path) -> Self {
        let store_root = crate::detect_distributed_store_from(project_root)
            .unwrap_or_else(|| project_root.join(".aida-store"));
        Self::new(
            project_root,
            store_root,
            crate::agent_registry::load_live_entries(project_root),
            Policy::load(project_root),
            Utc::now(),
        )
    }

    /// The record and reason when `handle`'s hello from the session that
    /// started at `started` has no ack past the window. An ack already in a
    /// mailbox counts even before the agent's record is marked.
    pub(crate) fn unreachable(
        &self,
        handle: Option<&str>,
        started: DateTime<Utc>,
    ) -> Option<(HelloRecord, String)> {
        if !self.policy.enabled {
            return None;
        }
        let (root, rec) = find_record(&self.roots, handle?)?;
        if !sent_this_session(&rec, Some(started)) {
            return None;
        }
        if rec.acked_at.is_none()
            && find_ack(
                &rec,
                &ack_dirs(&root, &self.main, &self.store_root, &self.live),
            )
            .is_some()
        {
            return None;
        }
        match state(&rec, || self.presence(), &self.policy, self.now) {
            State::Unreachable(reason) => Some((rec, reason)),
            _ => None,
        }
    }
}

/// The `aida agent ls` suffix for an agent whose hello went unacked past
/// the window, else empty.
pub(crate) fn ls_note(census: &Census, handle: Option<&str>, started: DateTime<Utc>) -> String {
    census
        .unreachable(handle, started)
        .map(|(_, reason)| format!("  ⚠ advisor unreachable: {reason}"))
        .unwrap_or_default()
}

/// Doctor scan: one finding per live agent whose hello has no ack past the
/// window, naming the mail path that failed.
pub(crate) fn doctor_scan(project_root: &Path) -> Vec<DoctorFinding> {
    let census = Census::load(project_root);
    census
        .live
        .iter()
        .filter_map(|e| census.unreachable(e.name.as_deref(), e.started_at))
        .map(|(rec, reason)| finding(&rec, &reason, census.now))
        .collect()
}

/// `aida doctor check advisor-handshake`: the problem findings of
/// [`doctor_scan`] plus what they were judged from, so the check never
/// comes back empty while an agent's notice says the advisor is missing:
/// the registered advisors, the advisor seat grants (live but unregistered
/// evidence), and each recent hello's state.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
pub(crate) fn doctor_report(project_root: &Path) -> Vec<DoctorFinding> {
    let census = Census::load(project_root);
    let grants = crate::gate::seat_authority::held_grants(ADVISOR_ROLE, census.now);
    let recent_turn = crate::mandate_drive::advisor_live(project_root, census.now);
    let foreign = crate::gate::seat_authority::probes_from_foreign_namespace();
    let mut out = report_findings(&census, &grants, recent_turn, foreign);
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// [`doctor_report`] over an explicit census and grant set.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
pub(crate) fn report_findings(
    census: &Census,
    grants: &[crate::gate::seat_authority::HeldGrant],
    recent_turn: bool,
    foreign_namespace: bool,
) -> Vec<DoctorFinding> {
    let row = |id: &str, summary: String, action: String| DoctorFinding {
        category: DOCTOR_CATEGORY.into(),
        id: format!("{DOCTOR_CATEGORY}:{id}"),
        summary,
        action,
        safe_heal: false,
    };
    let mut out = Vec::new();
    let presence = presence_from(&census.live, grants, recent_turn);
    let registered: Vec<String> = live_advisors(&census.live)
        .iter()
        .map(|e| format!("{} ({})", crate::mail_backlog::handle(e), e.id))
        .collect();
    out.push(if registered.is_empty() {
        row(
            "registered-advisors",
            "registered advisors: none (no advisor row for a running process in .aida/agents)".into(),
            "A live advisor registers itself on its next prompt. If one is running, have it submit a prompt, then re-run this check.".into(),
        )
    } else {
        row(
            "registered-advisors",
            format!("registered advisors: {}", registered.join(", ")),
            "None needed.".into(),
        )
    });
    if registered.is_empty() {
        for g in grants {
            let prefix: String = g.id.chars().take(8).collect();
            let holders = if g.holders.is_empty() {
                "bound to no process".to_string()
            } else {
                g.holders
                    .iter()
                    .map(|(pid, alive)| match (alive, foreign_namespace) {
                        (true, _) => format!("pid {pid} live"),
                        (false, true) => {
                            format!("pid {pid} not observable from this sandbox")
                        }
                        (false, false) => format!("pid {pid} not running"),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push(row(
                &format!("grant:{prefix}"),
                format!(
                    "advisor seat grant {prefix} (expires {}): {holders}",
                    g.expires_at.format("%Y-%m-%dT%H:%MZ")
                ),
                "A live holder is an advisor with no registry row; its next prompt registers it."
                    .into(),
            ));
        }
        if let AdvisorPresence::Unregistered(evidence) = &presence {
            out.push(row(
                "unregistered-advisor",
                format!("an advisor is live but unregistered: {evidence}"),
                "Have the advisor submit a prompt (its per-prompt hook registers it), then re-run this check.".into(),
            ));
        } else if foreign_namespace && !grants.is_empty() {
            out.push(row(
                "unregistered-advisor",
                "grant holders cannot be probed from this sandbox; run this check from a host terminal to see whether one is live".into(),
                "Run `aida doctor check advisor-handshake` outside the sandbox.".into(),
            ));
        }
    }
    let problems: Vec<DoctorFinding> = census
        .live
        .iter()
        .filter_map(|e| census.unreachable(e.name.as_deref(), e.started_at))
        .map(|(rec, reason)| finding(&rec, &reason, census.now))
        .collect();
    let horizon = Duration::hours(ADVISOR_NOTICE_HORIZON_HOURS);
    for rec in all_records(&census.roots)
        .into_iter()
        .filter(|r| census.now - r.sent_at < horizon)
    {
        let id = format!("{DOCTOR_CATEGORY}:{}", rec.handle);
        if problems.iter().any(|p| p.id == id) {
            continue;
        }
        let mut rec = rec;
        if rec.acked_at.is_none() {
            let root = PathBuf::from(&rec.path);
            if let Some(ack) = find_ack(
                &rec,
                &ack_dirs(&root, &census.main, &census.store_root, &census.live),
            ) {
                rec.acked_at = Some(census.now);
                rec.acked_by = Some(ack.from);
            }
        }
        let what = match state(&rec, || presence.clone(), &census.policy, census.now) {
            State::Acked { by } => format!("acked by {}", by.as_deref().unwrap_or("the advisor")),
            State::Waiting => format!(
                "waiting {} for the ack",
                crate::mail_backlog::format_age(census.now - rec.sent_at)
            ),
            State::Unreachable(reason) => format!("unacked: {reason}"),
        };
        out.push(row(
            &format!("hello:{}", rec.handle),
            format!(
                "hello from {} ({} · {}), sent {} ago: {what}",
                handle_label(&rec.handle, rec.rotation.as_ref()),
                rec.role,
                rec.spec.as_deref().unwrap_or("no spec"),
                crate::mail_backlog::format_age(census.now - rec.sent_at)
            ),
            format!("The hello is in {}.", rec.mailbox),
        ));
    }
    out.extend(problems);
    out
}

/// PURE: the doctor finding for an unacked hello.
pub(crate) fn finding(rec: &HelloRecord, reason: &str, now: DateTime<Utc>) -> DoctorFinding {
    let action = match rec.home {
        HomeKind::UnverifiedClone => format!(
            "The mail path that failed: {} is a seat-clone local layer the primary inbox does not read. Run `aida mailbox sync` from the clone, then check that the clone verifies as a seat clone.",
            rec.mailbox
        ),
        _ => format!(
            "The hello is in {}. Start or wake an advisor; it acks the hello by reading `aida mailbox inbox`.",
            rec.mailbox
        ),
    };
    DoctorFinding {
        category: DOCTOR_CATEGORY.into(),
        id: format!("{DOCTOR_CATEGORY}:{}", rec.handle),
        summary: format!(
            "{} ({}): advisor unreachable for {}: {reason}",
            rec.handle,
            rec.role,
            crate::mail_backlog::format_age(now - rec.sent_at)
        ),
        action,
        safe_heal: false,
    }
}

#[cfg(test)]
#[path = "tests/story_b6b_100_handshake_tests.rs"]
mod tests;
