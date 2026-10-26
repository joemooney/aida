//! Addressable agent handles (STORY-1843 PR2b).
//!
//! A handle is the one short name an operator reads off a session's
//! statusline and relays to another agent (`aida mailbox send --to <handle>`).
//! Each handle is owned by one logical recipient UUID, recorded in a
//! reservation file under `.aida/agents/handles/` in the main checkout, so
//! sibling worktrees share the namespace and unrelated projects do not.
//! Allocation and binding run under one common-root lock, so two concurrent
//! launches never pick the same name.
//!
//! The registry entry's `name` is the handle (the mailbox resolver already
//! routes private mail by it); the reservation keyed by that name carries
//! the UUID and the native session it is bound to. A handle
//! confers no authority: it is an address, never a grant, author or reviewer
//! identity.
//!
//! Lifetime: a reservation is held while a registry entry carrying the
//! handle may still be live, or, before the launched process registers,
//! for [`UNBOUND_RESERVATION_TTL_SECS`]. The same native session (a resume
//! of that session) reconnects to its UUID; a session with a new native id
//! (including `/clear`, which starts a new client session) and any session
//! that reuses a retired spelling get a new UUID.
//!
//! Only the process a launcher started may bind that launcher's unbound
//! reservation: the reservation records a launch token the launcher hands
//! its child in [`LAUNCH_TOKEN_ENV`], and binding requires it. A reservation
//! without a live, matching token (released, expired, never launched) is a
//! retired spelling.
// trace:STORY-1843 | ai:claude

use crate::agent_registry::{self, AgentRegistryEntry};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Longest handle accepted, so a handle fits a statusline and a DNS label.
pub(crate) const HANDLE_MAX_LEN: usize = aida_core::mailbox_identity::HANDLE_MAX_LEN;

/// How long a launcher's reservation holds its name before the launched
/// process registers under it.
pub(crate) const UNBOUND_RESERVATION_TTL_SECS: i64 = 10 * 60;

/// Unverified (PID namespace unobservable) holders keep their handle only
/// while their heartbeat is this recent, matching the mail resolver.
const UNVERIFIED_HOLDER_TTL_SECS: i64 = 24 * 60 * 60;

/// Carries a launcher's reservation token to the process it launches.
pub(crate) const LAUNCH_TOKEN_ENV: &str = "AIDA_HANDLE_LAUNCH_TOKEN";

/// Carries a headless child's own handle. `AIDA_AGENT_NAME` stays the
/// child's `<vendor>-<seat>` seat identity (what review attestation records);
/// this is the address its mail is sent from and read under.
pub(crate) const HANDLE_ENV: &str = "AIDA_HANDLE";

/// Session identity a client sets for its own children. A launched agent of
/// any vendor must not inherit its parent's: it would resolve the parent's
/// handle and read the parent's mail.
pub(crate) const INHERITED_SESSION_ENV: &[&str] = &[
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PID",
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "AIDA_HOOK_PAYLOAD",
    LAUNCH_TOKEN_ENV,
    HANDLE_ENV,
];

/// Vendor names and aliases. A handle may not be one: `--to codex` must
/// keep meaning the vendor group.
const VENDOR_ALIASES: &[&str] = &["claude", "codex", "antigravity", "agy", "web", "shell"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct HandleReservation {
    pub(crate) handle: String,
    /// The logical recipient that owns this handle.
    pub(crate) recipient_id: String,
    pub(crate) agent_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
    pub(crate) reserved_at: DateTime<Utc>,
    /// The native client session bound to this recipient (Claude session id,
    /// Codex thread id, Agy conversation id). Unset until the session's own
    /// startup binds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native_session_id: Option<String>,
    /// The registry row that registered under this handle, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) registry_id: Option<String>,
    /// Earlier recipients that held this spelling, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) previous_recipients: Vec<String>,
    /// A launcher's proof token: only a requester presenting it may bind
    /// this unbound reservation. Cleared once bound or released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) launch_token: Option<String>,
    /// The launcher process holding the unbound reservation, so it can hand
    /// the token to its child and release the name if the launch fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) launcher_pid: Option<u32>,
}

/// What a caller wants allocated or bound.
#[derive(Debug, Clone, Default)]
pub(crate) struct HandleRequest<'a> {
    pub(crate) agent_type: &'a str,
    pub(crate) role: Option<&'a str>,
    /// A launch `-n` name (or the name a launch already gave this session).
    pub(crate) explicit: Option<&'a str>,
    /// The native session asking, when known.
    pub(crate) native_session_id: Option<&'a str>,
    /// The registry entry asking, so a re-registering process keeps its own
    /// handle instead of colliding with itself.
    pub(crate) registry_id: Option<&'a str>,
    /// The launch token the requester's launcher handed it, when any.
    pub(crate) launch_token: Option<&'a str>,
    /// A launcher reserving for a process it is about to start: the new
    /// reservation records a fresh launch token and this process's pid.
    pub(crate) launcher: bool,
}

/// Why `name` is not a valid handle spelling, or `None` when it is.
pub(crate) fn handle_spelling_rejection(name: &str) -> Option<String> {
    aida_core::mailbox_identity::handle_spelling_rejection(name)
}

/// A valid spelling close to `name`, offered when `name` is refused; never
/// applied silently.
pub(crate) fn suggest_handle(name: &str) -> Option<String> {
    let mut out = String::new();
    for c in name.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    let out: String = out.chars().take(HANDLE_MAX_LEN).collect();
    let out = out.trim_end_matches('-').to_string();
    (!out.is_empty() && handle_spelling_rejection(&out).is_none()).then_some(out)
}

/// Addresses a handle may not take: roles, vendor names and people.
pub(crate) fn reserved_addresses(project_root: &Path) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = crate::AGENT_ROLES.iter().map(|r| r.to_string()).collect();
    out.extend(VENDOR_ALIASES.iter().map(|v| v.to_string()));
    out.extend(["operator", "person"].map(String::from));
    for role in crate::list_roles(project_root).unwrap_or_default() {
        out.insert(role.name.to_lowercase());
    }
    out.extend(crate::known_mailbox_people(&project_root.join(".aida-store")).into_keys());
    out
}

/// Refuse `name` as an explicit handle, with a suggested spelling when one
/// exists.
pub(crate) fn validate_explicit_handle(name: &str, reserved: &BTreeSet<String>) -> Result<()> {
    validate_address(name, reserved)?;
    // A name shaped like a generated `<vendor>-<role>` handle without its
    // number would collide with the allocator's namespace.
    let in_generated_namespace = ["claude-", "codex-", "antigravity-", "agy-", "web-"]
        .iter()
        .filter_map(|prefix| name.strip_prefix(prefix))
        .any(|rest| match rest.rsplit_once('-') {
            Some((_, suffix)) => suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_digit()),
            None => true,
        });
    if in_generated_namespace {
        bail!("invalid agent name `{name}`: conflicts with agent type+role prefix");
    }
    Ok(())
}

/// Spelling and reserved-address checks every handle passes.
fn validate_address(name: &str, reserved: &BTreeSet<String>) -> Result<()> {
    if let Some(why) = handle_spelling_rejection(name) {
        match suggest_handle(name).filter(|s| !reserved.contains(s)) {
            Some(s) => bail!("invalid agent name `{name}`: {why}; try `{s}`"),
            None => bail!("invalid agent name `{name}`: {why}"),
        }
    }
    if reserved.contains(name) {
        bail!(
            "invalid agent name `{name}`: it names a role, vendor or person; \
             try `{name}-01` or another name"
        );
    }
    Ok(())
}

/// The display alias of a vendor inside a generated handle.
pub(crate) fn vendor_alias(agent_type: &str) -> String {
    match agent_registry::normalize_agent_type(agent_type.to_string()).as_str() {
        "antigravity" => "agy".to_string(),
        other => other.to_string(),
    }
}

/// `<vendor>-<role>-NN`: two-digit suffix that grows past 99.
pub(crate) fn generated_handle(agent_type: &str, role: Option<&str>, seq: u32) -> String {
    let role = role
        .map(|r| crate::kernel::canonical_role_name(r.trim()))
        .filter(|r| !r.is_empty() && handle_spelling_rejection(r).is_none())
        .unwrap_or_else(|| "agent".to_string());
    format!("{}-{}-{:02}", vendor_alias(agent_type), role, seq)
}

pub(crate) fn handles_dir(project_root: &Path) -> PathBuf {
    agent_registry::agents_dir(project_root).join("handles")
}

fn reservation_path(project_root: &Path, handle: &str) -> PathBuf {
    handles_dir(project_root).join(format!("{handle}.toml"))
}

pub(crate) struct NamespaceLock {
    file: std::fs::File,
}

impl Drop for NamespaceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// The common-root allocation lock. Held across read-decide-write so a
/// reservation is atomic with respect to every other allocator.
pub(crate) fn lock_namespace(project_root: &Path) -> Result<NamespaceLock> {
    let dir = handles_dir(project_root);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating handle directory {}", dir.display()))?;
    let path = dir.join(".lock");
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(&path)
        .with_context(|| format!("opening handle lock {}", path.display()))?;
    if !file.metadata()?.is_file() {
        bail!("invalid handle allocation lock");
    }
    // SessionStart is advisory: never wait behind a wedged allocator.
    // trace:STORY-1843 | ai:codex
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => break,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) => return Err(e).context("handle allocation lock busy; retry after agent gc"),
        }
    }
    Ok(NamespaceLock { file })
}

// Every allocation input is a bounded regular file. In particular, FIFO
// counters/session indexes must not defeat the SessionStart time budget.
// trace:STORY-1843 | ai:codex
fn read_allocation_text(path: &Path, limit: u64) -> Result<Option<String>> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
    };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > limit {
        bail!(
            "invalid or oversized handle allocation record {}",
            path.display()
        );
    }
    let mut body = String::new();
    file.take(limit + 1).read_to_string(&mut body)?;
    if body.len() as u64 > limit {
        bail!("oversized handle allocation record {}", path.display());
    }
    Ok(Some(body))
}

fn checked_reservation(root: &Path, handle: &str) -> Result<Option<HandleReservation>> {
    if handle_spelling_rejection(handle).is_some() {
        bail!("invalid reservation handle");
    }
    let body = match read_allocation_text(&reservation_path(root, handle), 65536)? {
        Some(body) => Some(body),
        None => read_allocation_text(
            &handles_dir(root)
                .join("retired")
                .join(format!("{handle}.toml")),
            65536,
        )?,
    };
    body.map(|body| {
        let r: HandleReservation = toml::from_str(&body).context("invalid handle reservation")?;
        if r.handle != handle {
            bail!("handle reservation filename mismatch");
        }
        Ok(r)
    })
    .transpose()
}

pub(crate) fn load_reservation(project_root: &Path, handle: &str) -> Option<HandleReservation> {
    checked_reservation(project_root, handle).ok().flatten()
}

fn write_reservation(project_root: &Path, r: &HandleReservation) -> Result<()> {
    let body = toml::to_string_pretty(r).context("serialising handle reservation")?;
    let path = reservation_path(project_root, &r.handle);
    aida_core::write_atomic(&path, body.as_bytes())
        .with_context(|| format!("writing handle reservation {}", path.display()))?;
    if let Some(sid) = &r.native_session_id {
        let index = session_index_path(project_root, sid);
        std::fs::create_dir_all(index.parent().unwrap())?;
        aida_core::write_atomic(&index, r.handle.as_bytes())?;
    }
    Ok(())
}

/// Whether a registry entry may still be a live holder: observably alive,
/// or unobservable from here with a recent heartbeat.
fn entry_may_hold(project_root: &Path, entry: &AgentRegistryEntry, now: DateTime<Utc>) -> bool {
    entry_may_hold_from(
        project_root,
        entry,
        now,
        agent_registry::PidObserver::current(),
    )
}

/// [`entry_may_hold`] from an explicit observer. An isolated sandbox cannot
/// vouch for rows it cannot see (BUG-2212), so they hold on heartbeat.
fn entry_may_hold_from(
    project_root: &Path,
    entry: &AgentRegistryEntry,
    now: DateTime<Utc>,
    observer: agent_registry::PidObserver,
) -> bool {
    if entry.source == "role-enter" {
        return false;
    }
    agent_registry::observed_liveness_from(project_root, entry, observer).unwrap_or_else(|| {
        let age = now.signed_duration_since(entry.last_active_at);
        age >= chrono::Duration::zero()
            && age <= chrono::Duration::seconds(UNVERIFIED_HOLDER_TTL_SECS)
    })
}

/// Whether `req` proves it is the process the launcher reserved `r` for.
fn proves_launch(r: &HandleReservation, req: &HandleRequest<'_>) -> bool {
    r.native_session_id.is_none()
        && r.launch_token.as_deref().is_some_and(|own| {
            req.launch_token
                .is_some_and(|token| !token.is_empty() && token == own)
        })
}

/// The ways a holder of `handle` other than the requester may still be live.
fn other_live_holder(
    project_root: &Path,
    entries: &[AgentRegistryEntry],
    reservation: Option<&HandleReservation>,
    handle: &str,
    req: &HandleRequest<'_>,
    now: DateTime<Utc>,
) -> Option<String> {
    // The launched session binding its launcher's reservation: the
    // launcher's own registry row for it is not a competing holder.
    let binding_launch = reservation.is_some_and(|r| proves_launch(r, req));
    for entry in entries {
        if binding_launch && entry.source == "agent-launcher" {
            continue;
        }
        if !entry
            .name
            .as_deref()
            .is_some_and(|n| n.trim().eq_ignore_ascii_case(handle))
        {
            continue;
        }
        if req.registry_id == Some(entry.id.as_str()) || same_session(entry, req) {
            continue;
        }
        if entry_may_hold(project_root, entry, now) {
            return Some(format!("{} (PID {})", entry.id, entry.pid));
        }
    }
    let r = reservation?;
    if binding_launch
        || session_matches(r, req.native_session_id)
        || (req.registry_id.is_some() && r.registry_id.as_deref() == req.registry_id)
    {
        return None;
    }
    if now.signed_duration_since(r.reserved_at)
        > chrono::Duration::seconds(UNBOUND_RESERVATION_TTL_SECS)
    {
        return None;
    }
    match r.native_session_id.as_deref() {
        // An unbound reservation is the launcher's promise to the process it
        // is about to start; only that process (its launch token, handled
        // above) may bind it. Every other requester sees it held.
        None => Some(format!(
            "a launch reserved it at {}",
            r.reserved_at.to_rfc3339()
        )),
        // A session that bound it but has not written its registry row yet
        // still holds it; once it has a row, the rows above decide.
        Some(sid) => {
            let bound = HandleRequest {
                native_session_id: Some(sid),
                ..Default::default()
            };
            (!entries.iter().any(|e| same_session(e, &bound))).then(|| {
                format!(
                    "a starting session bound it at {}",
                    r.reserved_at.to_rfc3339()
                )
            })
        }
    }
}

/// Session ids match exactly: a prefix of one id can name an unrelated
/// session.
// trace:STORY-1843 | ai:claude
fn same_session(entry: &AgentRegistryEntry, req: &HandleRequest<'_>) -> bool {
    let Some(sid) = req.native_session_id.filter(|s| !s.is_empty()) else {
        return false;
    };
    [&entry.claude_session_id, &entry.native_session_id]
        .into_iter()
        .flatten()
        .any(|own| own == sid)
}

fn session_matches(reservation: &HandleReservation, sid: Option<&str>) -> bool {
    match (reservation.native_session_id.as_deref(), sid) {
        (Some(own), Some(sid)) if !own.is_empty() => own == sid,
        _ => false,
    }
}

/// Take `handle` for the requester under the held lock. Same session keeps
/// its recipient; a fresh holder of a free spelling gets a new one.
fn take(
    project_root: &Path,
    handle: &str,
    existing: Option<HandleReservation>,
    req: &HandleRequest<'_>,
    now: DateTime<Utc>,
) -> Result<HandleReservation> {
    let sid = req.native_session_id.filter(|s| !s.is_empty());
    let (launch_token, launcher_pid) = if req.launcher {
        (
            Some(uuid::Uuid::new_v4().to_string()),
            Some(std::process::id()),
        )
    } else {
        (None, None)
    };
    let reservation = match existing {
        Some(r) if session_matches(&r, sid) => r,
        // The launcher reserved it for this process, which proved it with
        // the launch token. A released or expired reservation carries no
        // token and falls through to a new recipient below.
        Some(mut r) if proves_launch(&r, req) => {
            r.native_session_id = sid.map(str::to_string);
            r.registry_id = req.registry_id.map(str::to_string).or(r.registry_id);
            r.launch_token = None;
            r.launcher_pid = None;
            r
        }
        // The same registry entry re-registering without a session id.
        Some(r)
            if sid.is_none()
                && req.registry_id.is_some()
                && r.registry_id.as_deref() == req.registry_id =>
        {
            r
        }
        Some(old) => HandleReservation {
            handle: handle.to_string(),
            recipient_id: uuid::Uuid::new_v4().to_string(),
            agent_type: agent_registry::normalize_agent_type(req.agent_type.to_string()),
            role: req.role.map(str::to_string),
            reserved_at: now,
            native_session_id: sid.map(str::to_string),
            registry_id: req.registry_id.map(str::to_string),
            previous_recipients: {
                let mut prev = old.previous_recipients;
                prev.push(old.recipient_id);
                prev
            },
            launch_token,
            launcher_pid,
        },
        None => HandleReservation {
            handle: handle.to_string(),
            recipient_id: uuid::Uuid::new_v4().to_string(),
            agent_type: agent_registry::normalize_agent_type(req.agent_type.to_string()),
            role: req.role.map(str::to_string),
            reserved_at: now,
            native_session_id: sid.map(str::to_string),
            registry_id: req.registry_id.map(str::to_string),
            previous_recipients: Vec::new(),
            launch_token,
            launcher_pid,
        },
    };
    write_reservation(project_root, &reservation)?;
    Ok(reservation)
}

/// Reserve (or re-bind) a handle under the common-root lock.
///
/// An explicit name is validated and refused on a live collision. Without
/// one, the session's existing reservation is reused, else the lowest free
/// `<vendor>-<role>-NN` is allocated.
pub(crate) fn reserve_handle(
    project_root: &Path,
    req: &HandleRequest<'_>,
) -> Result<HandleReservation> {
    let reserved = reserved_addresses(project_root);
    if let Some(name) = req.explicit {
        validate_explicit_handle(name, &reserved)?;
    }
    let _lock = lock_namespace(project_root)?;
    let now = Utc::now();
    let (handle, existing) = decide(project_root, req, &reserved, now)?;
    if req.explicit.is_none() {
        if let Some((stem, seq)) = handle.rsplit_once('-') {
            let prefix = generated_handle(req.agent_type, req.role, 0);
            if seq.chars().all(|c| c.is_ascii_digit())
                && prefix.rsplit_once('-').is_some_and(|(p, _)| p == stem)
            {
                let path = handles_dir(project_root).join("counters").join(&prefix);
                let previous = read_allocation_text(&path, 64)?
                    .map(|s| s.trim().parse::<u32>())
                    .transpose()
                    .context("invalid handle allocation counter")?
                    .unwrap_or(0);
                let seq = seq
                    .parse::<u32>()
                    .context("invalid generated handle sequence")?;
                std::fs::create_dir_all(path.parent().unwrap())?;
                aida_core::write_atomic(&path, previous.max(seq).to_string().as_bytes())?;
            }
        }
    }
    let taken = take(project_root, &handle, existing, req, now)?;
    release_other_bindings(project_root, &taken)?;
    Ok(taken)
}

/// The handle [`reserve_handle`] would pick, without locking or writing.
/// For previews only: the answer can be stale by the time a launch runs.
pub(crate) fn preview_handle(project_root: &Path, req: &HandleRequest<'_>) -> Result<String> {
    let reserved = reserved_addresses(project_root);
    if let Some(name) = req.explicit {
        validate_explicit_handle(name, &reserved)?;
    }
    Ok(decide(project_root, req, &reserved, Utc::now())?.0)
}

fn decide(
    project_root: &Path,
    req: &HandleRequest<'_>,
    reserved: &BTreeSet<String>,
    now: DateTime<Utc>,
) -> Result<(String, Option<HandleReservation>)> {
    if req.explicit.is_none() {
        if let Some(sid) = req.native_session_id.filter(|s| !s.is_empty()) {
            if let Some(r) = reservation_of_session(project_root, sid)? {
                return Ok((r.handle.clone(), Some(r)));
            }
        }
    }
    let entries: Vec<AgentRegistryEntry> =
        bounded_records(&agent_registry::agents_dir(project_root))?;
    if let Some(name) = req.explicit {
        let existing = checked_reservation(project_root, name)?;
        if let Some(holder) =
            other_live_holder(project_root, &entries, existing.as_ref(), name, req, now)
        {
            bail!("agent name `{name}` is already in use by {holder}");
        }
        return Ok((name.to_string(), existing));
    }
    // No second native-session lookup here: the early one above already ran
    // for this request. Under `reserve_handle` the namespace lock gives a
    // stable snapshot of cooperating reservation writers, so a repeat lookup
    // could not find a binding the first one missed. `preview_handle` is
    // unlocked and best effort by design; the real reservation redecides
    // under the lock.
    // trace:TASK-2346 | ai:claude
    let prefix = generated_handle(req.agent_type, req.role, 0);
    let counter = handles_dir(project_root).join("counters").join(&prefix);
    let start = match read_allocation_text(&counter, 64)? {
        Some(raw) => raw
            .trim()
            .parse::<u32>()
            .context("invalid handle allocation counter")?,
        None => {
            // One bounded bootstrap for pre-counter installations. GC seeds
            // each stem's counter before archiving, so unrelated archived
            // history never participates in a new stem's startup.
            let mut last = 0;
            for dir in [handles_dir(project_root)] {
                for r in bounded_records::<HandleReservation>(&dir)? {
                    if let Some((stem, seq)) = r.handle.rsplit_once('-') {
                        if prefix.rsplit_once('-').is_some_and(|(p, _)| p == stem) {
                            last = last.max(seq.parse::<u32>().unwrap_or(0));
                        }
                    }
                }
            }
            last
        }
    };
    // At most 64 path/liveness probes, regardless of historical numbering.
    // trace:STORY-1843 | ai:codex
    for offset in 1..=64 {
        let seq = start
            .checked_add(offset)
            .context("handle sequence exhausted")?;
        let candidate = generated_handle(req.agent_type, req.role, seq);
        if handle_spelling_rejection(&candidate).is_some() || reserved.contains(&candidate) {
            bail!("cannot generate a valid handle for `{}`", req.agent_type);
        }
        let existing = checked_reservation(project_root, &candidate)?;
        // Mail is keyed by the spelling, so a generated spelling another
        // recipient held is never handed out again: the next session would
        // read that holder's unread mail. Numbers grow instead.
        // trace:STORY-1843 | ai:claude
        if existing
            .as_ref()
            .is_some_and(|r| !held_by_requester(r, req))
        {
            continue;
        }
        if other_live_holder(
            project_root,
            &entries,
            existing.as_ref(),
            &candidate,
            req,
            now,
        )
        .is_none()
        {
            // Previews remain read-only; the real allocator persists this
            // under the namespace lock before writing the reservation.
            return Ok((candidate, existing));
        }
    }
    bail!("handle allocation probe budget exhausted; run aida agent gc and retry")
}

/// The handle a native session already holds, so a resume of it keeps it.
// trace:STORY-1843 | ai:claude
pub(crate) fn handle_of_session(project_root: &Path, native_session_id: &str) -> Option<String> {
    reservation_of_session(project_root, native_session_id)
        .ok()
        .flatten()
        .map(|r| r.handle)
}

/// Whether `r` is already the requester's: its session, its launcher's
/// promise to it, or its own registry entry.
fn held_by_requester(r: &HandleReservation, req: &HandleRequest<'_>) -> bool {
    session_matches(r, req.native_session_id)
        || proves_launch(r, req)
        || (req.registry_id.is_some() && r.registry_id.as_deref() == req.registry_id)
}

/// A session holds one handle: binding it to `taken` unbinds any other
/// reservation the same session held (e.g. a generated handle a `--bg`
/// session got before its launch renamed it). The unbound spelling keeps
/// its recipient and history and becomes free once no live entry names it.
fn release_other_bindings(project_root: &Path, taken: &HandleReservation) -> Result<()> {
    let Some(sid) = taken.native_session_id.as_deref() else {
        return Ok(());
    };
    for mut r in bounded_records::<HandleReservation>(&handles_dir(project_root))? {
        if r.handle != taken.handle && session_matches(&r, Some(sid)) {
            r.native_session_id = None;
            // Not a fresh launch promise: an unbound reservation older than
            // the TTL holds nothing, and with no token nothing can re-bind it.
            r.reserved_at = DateTime::<Utc>::UNIX_EPOCH;
            r.launch_token = None;
            r.launcher_pid = None;
            write_reservation(project_root, &r)?;
        }
    }
    Ok(())
}

/// The token for `handle` when this process is the launcher still holding
/// its unbound reservation; the launcher hands it to the child it spawns.
pub(crate) fn launch_token_for(project_root: &Path, handle: &str) -> Option<String> {
    load_reservation(project_root, handle)
        .filter(|r| r.native_session_id.is_none() && r.launcher_pid == Some(std::process::id()))
        .and_then(|r| r.launch_token)
}

/// Point `command` at this launcher's reservation for `handle`, and never
/// hand it a token (or session identity) inherited from this process.
pub(crate) fn apply_launch_env(
    command: &mut std::process::Command,
    project_root: &Path,
    handle: &str,
) {
    for key in INHERITED_SESSION_ENV {
        command.env_remove(key);
    }
    if let Some(token) = launch_token_for(project_root, handle) {
        command.env(LAUNCH_TOKEN_ENV, token);
    }
}

/// Compare-release: drop `reservation` only while it is still unbound and
/// still the same recipient, so a session that bound it meanwhile keeps it.
/// A spelling with history stays as a retired (tokenless, expired) record.
pub(crate) fn release_unbound(project_root: &Path, reservation: &HandleReservation) -> Result<()> {
    let _lock = lock_namespace(project_root)?;
    release_unbound_locked(project_root, reservation)
}

fn release_unbound_locked(project_root: &Path, reservation: &HandleReservation) -> Result<()> {
    let Some(mut current) = load_reservation(project_root, &reservation.handle) else {
        return Ok(());
    };
    if current.native_session_id.is_some() || current.recipient_id != reservation.recipient_id {
        return Ok(());
    }
    if current.previous_recipients.is_empty() {
        let path = reservation_path(project_root, &current.handle);
        return std::fs::remove_file(&path)
            .or_else(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .with_context(|| format!("removing handle reservation {}", path.display()));
    }
    current.reserved_at = DateTime::<Utc>::UNIX_EPOCH;
    current.launch_token = None;
    current.launcher_pid = None;
    write_reservation(project_root, &current)
}

/// Releases the unbound reservations this launcher process made while the
/// guard was live when it is dropped armed, so a launch that fails before
/// its child starts leaves no name held. An earlier launch's reservation,
/// still waiting for its child to bind it, is not this guard's. Disarm once
/// the child is spawned: the child binds the name.
pub(crate) struct LaunchReservationGuard {
    project_root: PathBuf,
    armed: bool,
    /// `(handle, recipient_id)` of this process's unbound reservations that
    /// predate the guard.
    // trace:STORY-1843 | ai:claude
    earlier: BTreeSet<(String, String)>,
}

impl LaunchReservationGuard {
    pub(crate) fn new(project_root: &Path) -> Self {
        let earlier = own_unbound_reservations(project_root)
            .map(|r| (r.handle, r.recipient_id))
            .collect();
        Self {
            project_root: project_root.to_path_buf(),
            armed: true,
            earlier,
        }
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for LaunchReservationGuard {
    fn drop(&mut self) {
        // Nothing reserved means nothing to release (and no directory to
        // create for the lock).
        if !self.armed || !handles_dir(&self.project_root).is_dir() {
            return;
        }
        let Ok(_lock) = lock_namespace(&self.project_root) else {
            return;
        };
        for r in own_unbound_reservations(&self.project_root) {
            if !self
                .earlier
                .contains(&(r.handle.clone(), r.recipient_id.clone()))
            {
                let _ = release_unbound_locked(&self.project_root, &r);
            }
        }
    }
}

/// Unbound reservations this process holds as their launcher.
fn own_unbound_reservations(project_root: &Path) -> impl Iterator<Item = HandleReservation> {
    let me = Some(std::process::id());
    list_reservations(project_root)
        .into_iter()
        .filter(move |r| r.launcher_pid == me && r.launch_token.is_some())
}

/// Give a registering session its handle, in place on `entry.name`.
/// Fail-open: a startup hook must never fail on handle allocation. A legacy
/// name that is not a valid handle spelling keeps its existing inbox; a
/// valid name another live session holds is replaced by a generated handle,
/// so two sessions never share one.
pub(crate) fn assign_session_handle(
    project_root: &Path,
    entry: &mut AgentRegistryEntry,
    native_session_id: Option<&str>,
) {
    if entry.source == "role-enter" {
        return;
    }
    let explicit = entry
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    let agent_type = entry.agent_type.clone();
    let role = entry.role.clone();
    let id = entry.id.clone();
    // The token a launcher handed this process proves it may bind that
    // launcher's reservation.
    let token = std::env::var(LAUNCH_TOKEN_ENV)
        .ok()
        .filter(|t| !t.trim().is_empty());
    let base = HandleRequest {
        agent_type: &agent_type,
        role: role.as_deref(),
        explicit: None,
        native_session_id,
        registry_id: Some(&id),
        launch_token: token.as_deref(),
        launcher: false,
    };
    if let Some(name) = explicit.as_deref() {
        if handle_spelling_rejection(name).is_some() {
            return;
        }
        let req = HandleRequest {
            explicit: Some(name),
            ..base.clone()
        };
        if reserve_handle(project_root, &req).is_ok() {
            return;
        }
    }
    if let Ok(r) = reserve_handle(project_root, &base) {
        entry.name = Some(r.handle);
    }
}

// SessionStart work is bounded even on an old, uncollected namespace.
// Refuse allocation instead of silently ignoring a possible live owner.
// trace:STORY-1843 | ai:codex
const MAX_ALLOCATION_RECORDS: usize = 4096;
fn bounded_records<T: serde::de::DeserializeOwned>(dir: &Path) -> Result<Vec<T>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut records = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
    for (n, entry) in entries.enumerate() {
        if n >= MAX_ALLOCATION_RECORDS || std::time::Instant::now() >= deadline {
            bail!("handle allocation record budget exceeded; run aida agent gc");
        }
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let Some(body) = read_allocation_text(&path, 65536)? else {
            continue;
        };
        // Other agent metadata lives in the registry directory too.
        if let Ok(record) = toml::from_str(&body) {
            records.push(record);
        }
    }
    Ok(records)
}

// Retired records retain exact-session resume and recipient history without
// participating in every new session's allocation scan.
// trace:STORY-1843 | ai:codex
fn session_index_path(root: &Path, sid: &str) -> PathBuf {
    handles_dir(root)
        .join("sessions")
        .join(format!("{:x}", Sha256::digest(sid.as_bytes())))
}
fn reservation_of_session(root: &Path, sid: &str) -> Result<Option<HandleReservation>> {
    if let Some(handle) =
        read_allocation_text(&session_index_path(root, sid), HANDLE_MAX_LEN as u64)?
    {
        if let Some(r) =
            checked_reservation(root, handle.trim())?.filter(|r| session_matches(r, Some(sid)))
        {
            return Ok(Some(r));
        }
    }
    Ok(bounded_records::<HandleReservation>(&handles_dir(root))?
        .into_iter()
        .find(|r| session_matches(r, Some(sid))))
}

/// Agent GC archives reservations after their launch grace has expired and
/// no live/unverified recent registry row holds the spelling. Keep mail and
/// history untouched; generated counters never issue an archived spelling.
// trace:STORY-1843 | ai:codex
pub(crate) fn collect_retired_reservations(root: &Path, dry_run: bool) -> Result<usize> {
    if !handles_dir(root).is_dir() {
        return Ok(0);
    }
    let _lock = lock_namespace(root)?;
    let entries = agent_registry::load_entries(root)
        .into_iter()
        .map(|(_, e)| e)
        .collect::<Vec<_>>();
    let now = Utc::now();
    let mut count = 0;
    for mut r in list_reservations(root) {
        if now.signed_duration_since(r.reserved_at)
            <= chrono::Duration::seconds(UNBOUND_RESERVATION_TTL_SECS)
            || entries
                .iter()
                .any(|e| e.name.as_deref() == Some(&r.handle) && entry_may_hold(root, e, now))
        {
            continue;
        }
        count += 1;
        if dry_run {
            continue;
        }
        if let Some((stem, seq)) = r.handle.rsplit_once('-') {
            if let Ok(seq) = seq.parse::<u32>() {
                let path = handles_dir(root)
                    .join("counters")
                    .join(format!("{stem}-00"));
                let previous = read_allocation_text(&path, 64)?
                    .map(|s| s.trim().parse::<u32>())
                    .transpose()
                    .context("invalid handle allocation counter")?
                    .unwrap_or(0);
                std::fs::create_dir_all(path.parent().unwrap())?;
                aida_core::write_atomic(&path, previous.max(seq).to_string().as_bytes())?;
            }
        }
        if let Some(sid) = &r.native_session_id {
            let index = session_index_path(root, sid);
            std::fs::create_dir_all(index.parent().unwrap())?;
            aida_core::write_atomic(&index, r.handle.as_bytes())?;
        }
        // Expired launch tokens grant no future binding, even from archive.
        r.launch_token = None;
        r.launcher_pid = None;
        write_reservation(root, &r)?;
        let retired = handles_dir(root).join("retired");
        std::fs::create_dir_all(&retired)?;
        std::fs::rename(
            reservation_path(root, &r.handle),
            retired.join(format!("{}.toml", r.handle)),
        )?;
    }
    Ok(count)
}

pub(crate) fn list_reservations(project_root: &Path) -> Vec<HandleReservation> {
    let mut out = Vec::new();
    if let Ok(dir) = std::fs::read_dir(handles_dir(project_root)) {
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            if let Some(r) = read_allocation_text(&path, 65536)
                .ok()
                .flatten()
                .and_then(|body| toml::from_str::<HandleReservation>(&body).ok())
            {
                out.push(r);
            }
        }
    }
    out.sort_by(|a, b| a.handle.cmp(&b.handle));
    out
}

/// The native session id of the process asking, from the hook payload the
/// hook relay exported, else the running client's own per-session
/// environment. A shared daemon's inherited role or name is never consulted.
pub(crate) fn current_native_session_id() -> Option<String> {
    current_native_session_id_for(&agent_registry::detect_agent_type())
}

/// [`current_native_session_id`] for a known client vendor. Only that
/// vendor's session variable counts: a Codex or Agy process launched from a
/// Claude session may still carry the parent's `CLAUDE_CODE_SESSION_ID`.
/// Agy exports no session variable; its id comes from its payload only.
pub(crate) fn current_native_session_id_for(vendor: &str) -> Option<String> {
    if let Some(sid) = std::env::var("AIDA_HOOK_PAYLOAD")
        .ok()
        .and_then(|raw| session_id_from_payload(&raw))
    {
        return Some(sid);
    }
    native_session_env_keys(vendor)
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
}

fn native_session_env_keys(vendor: &str) -> &'static [&'static str] {
    match agent_registry::normalize_agent_type(vendor.to_string()).as_str() {
        "claude" => &["CLAUDE_CODE_SESSION_ID"],
        "codex" => &["CODEX_THREAD_ID", "CODEX_SESSION_ID"],
        _ => &[],
    }
}

/// A client payload's session id: Claude/Codex `session_id`, Agy
/// `conversationId`.
pub(crate) fn session_id_from_payload(raw: &str) -> Option<String> {
    aida_core::mailbox_identity::session_id_from_payload(raw)
}

/// Unended session rows whose process may still be live, so a stale row
/// whose pid was reused never lends its handle.
// trace:STORY-1843 | ai:claude
///
/// Only rows `candidate` accepts, with a valid handle, are probed: a render
/// probes the few rows that could be its session, not the whole registry.
fn live_session_entries(
    project_root: &Path,
    candidate: impl Fn(&AgentRegistryEntry) -> bool,
) -> Vec<AgentRegistryEntry> {
    let now = Utc::now();
    let observer = agent_registry::PidObserver::current();
    agent_registry::load_active_entries(project_root)
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| e.ended_at.is_none() && valid_handle(e).is_some() && candidate(e))
        .filter(|e| entry_may_hold_from(project_root, e, now, observer))
        .collect()
}

fn valid_handle(e: &AgentRegistryEntry) -> Option<String> {
    e.name
        .as_deref()
        .filter(|n| handle_spelling_rejection(n).is_none())
        .map(str::to_string)
}

/// The handle of the session this process runs in: matched by native
/// session id, else, for a Claude process, by the client's pid
/// (`CLAUDE_PID` is a recorded value, so it matches even inside a
/// PID-namespaced sandbox). `None` when nothing identifies it: a session
/// never borrows another session's handle.
pub(crate) fn current_session_handle(
    project_root: &Path,
    native_session_id: Option<&str>,
) -> Option<String> {
    current_session_handle_for(
        project_root,
        native_session_id,
        &agent_registry::detect_agent_type(),
    )
}

/// [`current_session_handle`] for a known client vendor. `CLAUDE_PID` is
/// consulted only for a Claude process: any other vendor's copy was
/// inherited from a parent Claude session.
pub(crate) fn current_session_handle_for(
    project_root: &Path,
    native_session_id: Option<&str>,
    vendor: &str,
) -> Option<String> {
    current_session_entry_for(project_root, native_session_id, vendor)
        .and_then(|e| valid_handle(&e))
}

// Keep handle and vendor from the same resolved session row: raw children
// can inherit both a parent's handle aliases and its AIDA_AGENT_TYPE.
// trace:STORY-1843 | ai:codex
fn current_session_entry_for(
    project_root: &Path,
    native_session_id: Option<&str>,
    vendor: &str,
) -> Option<AgentRegistryEntry> {
    let req = HandleRequest {
        native_session_id: native_session_id.filter(|s| !s.trim().is_empty()),
        ..Default::default()
    };
    let claude = agent_registry::normalize_agent_type(vendor.to_string()) == "claude";
    let claude_pid = std::env::var("CLAUDE_PID")
        .ok()
        .filter(|_| claude)
        .and_then(|p| p.trim().parse::<u32>().ok());
    let by_pid = |e: &AgentRegistryEntry| Some(e.pid) == claude_pid && e.agent_type == "claude";
    let entries = live_session_entries(project_root, |e| same_session(e, &req) || by_pid(e));
    let newest = |hit: &dyn Fn(&AgentRegistryEntry) -> bool| {
        entries
            .iter()
            .filter(|e| hit(e))
            .max_by_key(|e| e.last_active_at)
            .cloned()
    };
    newest(&|e| same_session(e, &req)).or_else(|| newest(&by_pid))
}

/// The handle of the agent session running in a tmux pane, found from the
/// pane's shell pid: the live registry entry whose process descends from
/// it, nearest the pane first. A `#(aida statusline)` render runs in tmux's
/// own environment, where no client session variable exists.
pub(crate) fn session_handle_in_pane(project_root: &Path, pane_pid: u32) -> Option<String> {
    live_session_entries(project_root, |e| {
        agent_registry::ancestor_depth(e.pid, pane_pid).is_some()
    })
    .into_iter()
    .filter_map(|e| {
        let depth = agent_registry::ancestor_depth(e.pid, pane_pid)?;
        Some((depth, e))
    })
    .min_by_key(|(depth, e)| (*depth, std::cmp::Reverse(e.last_active_at)))
    .and_then(|(_, e)| valid_handle(&e))
}

/// The handle of the nearest agent session among `pid` and its ancestors:
/// a client that spawns `aida statusline` itself is its parent.
pub(crate) fn session_handle_by_ancestry(project_root: &Path, pid: u32) -> Option<String> {
    let mut chain = Vec::new();
    let mut current = Some(pid);
    while let Some(p) = current.filter(|p| *p > 1 && !chain.contains(p)) {
        chain.push(p);
        current = agent_registry::parent_pid_of(p);
    }
    let entries = live_session_entries(project_root, |e| chain.contains(&e.pid));
    chain.iter().find_map(|p| {
        entries
            .iter()
            .filter(|e| e.pid == *p)
            .max_by_key(|e| e.last_active_at)
            .and_then(valid_handle)
    })
}

/// [`current_session_handle`] for the project containing the cwd, using
/// this process's own session evidence.
pub(crate) fn current_session_handle_for_cwd() -> Option<String> {
    current_session_entry_for_cwd().and_then(|e| valid_handle(&e))
}

/// The registry row file of [`current_session_entry_for_cwd`], for the
/// mailbox identity policy to verify in core.
// trace:STORY-1843 | ai:claude
pub(crate) fn current_session_row_in(project_root: &Path) -> Option<PathBuf> {
    let root = crate::main_worktree_root_from(project_root);
    let entry = current_session_entry_in(&root)?;
    Some(agent_registry::registry_path(&root, &entry.id))
}

/// The same persisted session used for private mail, including its vendor.
// trace:STORY-1843 | ai:codex
pub(crate) fn current_session_entry_for_cwd() -> Option<AgentRegistryEntry> {
    let cwd = std::env::current_dir().ok()?;
    let root = crate::main_worktree_root_from(&crate::find_aida_project_root_from(&cwd).ok()?);
    current_session_entry_in(&root)
}

/// [`current_session_entry_for_cwd`] for the registry of the main checkout
/// `root`, using this process's own session evidence.
// trace:STORY-1843 | ai:claude
fn current_session_entry_in(root: &Path) -> Option<AgentRegistryEntry> {
    // A raw client may inherit its parent's vendor/session environment.
    // Prefer the nearest actual vendor process (even an unregistered one)
    // before considering those variables. A new child cannot borrow a more
    // distant registered parent's inbox while its own hook is still starting.
    // trace:STORY-1843 | ai:codex
    let me = std::process::id();
    let nearest = ["claude", "codex", "antigravity"]
        .into_iter()
        .filter_map(|vendor| {
            let pid = agent_registry::session_start_agent_pid(me, vendor)?;
            let depth = agent_registry::ancestor_depth(me, pid)?;
            Some((depth, pid, vendor))
        })
        .min_by_key(|(depth, _, _)| *depth);
    if let Some((_, pid, vendor)) = nearest {
        return live_session_entries(root, |e| e.pid == pid && e.agent_type == vendor)
            .into_iter()
            .max_by_key(|e| e.last_active_at);
    }
    // Inside a PID namespace the host client may be invisible; its native
    // session evidence remains the supported lookup in that case.
    current_session_entry_for(
        root,
        current_native_session_id().as_deref(),
        &agent_registry::detect_agent_type(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        project: &Path,
        id: &str,
        name: Option<&str>,
        sid: Option<&str>,
    ) -> AgentRegistryEntry {
        let now = Utc::now();
        AgentRegistryEntry {
            id: id.to_string(),
            agent_type: "claude".to_string(),
            pid: std::process::id(),
            pid_start: crate::process_probe::process_start_identity(std::process::id()),
            pid_namespace: agent_registry::current_pid_namespace(),
            name: name.map(str::to_string),
            description: None,
            tty: None,
            terminal: None,
            started_at: now,
            last_active_at: now,
            role: Some("implementer".to_string()),
            current_spec: None,
            worktree_path: project.to_path_buf(),
            source: "session-start-hook".to_string(),
            binary_version: None,
            build_sha: None,
            availability: Default::default(),
            paused_since: None,
            paused_reason: None,
            expected_back: None,
            native_session_id: None,
            claude_session_id: sid.map(str::to_string),
            ended_at: None,
            spec_status_at_end: None,
            resumed_from: None,
        }
    }

    fn req<'a>(explicit: Option<&'a str>, sid: Option<&'a str>) -> HandleRequest<'a> {
        HandleRequest {
            agent_type: "claude",
            role: Some("implementer"),
            explicit,
            native_session_id: sid,
            registry_id: None,
            launch_token: None,
            launcher: false,
        }
    }

    /// What `aida agent new` reserves before it spawns.
    fn launcher_req(explicit: Option<&str>) -> HandleRequest<'_> {
        HandleRequest {
            launcher: true,
            ..req(explicit, None)
        }
    }

    // trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn spelling_bounds_refuse_with_a_suggestion_and_reserve_aliases() {
        let reserved: BTreeSet<String> =
            ["advisor", "codex", "agy", "joe"].map(String::from).into();
        assert!(validate_explicit_handle("rework-2736", &reserved).is_ok());
        let err = validate_explicit_handle("Rework_2736", &reserved).unwrap_err();
        assert!(err.to_string().contains("try `rework-2736`"), "{err}");
        let err = validate_explicit_handle("-lead", &reserved).unwrap_err();
        assert!(err.to_string().contains("start with a letter"), "{err}");
        let long = "a".repeat(HANDLE_MAX_LEN + 1);
        assert!(validate_explicit_handle(&long, &reserved).is_err());
        assert!(validate_explicit_handle(&"a".repeat(HANDLE_MAX_LEN), &reserved).is_ok());
        for alias in ["advisor", "codex", "agy", "joe"] {
            let err = validate_explicit_handle(alias, &reserved).unwrap_err();
            assert!(err.to_string().contains("role, vendor or person"), "{err}");
        }
    }

    // trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn generated_handles_use_vendor_role_and_two_digit_suffix_that_grows() {
        assert_eq!(
            generated_handle("claude", Some("implementer"), 1),
            "claude-implementer-01"
        );
        assert_eq!(
            generated_handle("antigravity", Some("reviewer"), 7),
            "agy-reviewer-07"
        );
        assert_eq!(generated_handle("codex", None, 100), "codex-agent-100");
    }

    // trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn allocation_skips_live_holders_and_the_same_session_keeps_its_recipient() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        agent_registry::write_entry(
            project,
            &entry(
                project,
                "claude-1",
                Some("claude-implementer-01"),
                Some("s-other"),
            ),
        )
        .unwrap();
        let first = reserve_handle(project, &req(None, Some("s-mine"))).unwrap();
        assert_eq!(first.handle, "claude-implementer-02");
        assert_eq!(first.native_session_id.as_deref(), Some("s-mine"));
        // A second startup of the same session (resume, /clear) reconnects.
        let again = reserve_handle(project, &req(None, Some("s-mine"))).unwrap();
        assert_eq!(again.handle, first.handle);
        assert_eq!(again.recipient_id, first.recipient_id);
        // A different session gets a different handle and recipient.
        let other = reserve_handle(project, &req(None, Some("s-third"))).unwrap();
        assert_eq!(other.handle, "claude-implementer-03");
        assert_ne!(other.recipient_id, first.recipient_id);
    }

    // trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn explicit_live_collision_refuses_and_a_launch_reservation_binds_to_its_session() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        // The launcher reserves before spawn.
        let launched = reserve_handle(project, &launcher_req(Some("impl-1700"))).unwrap();
        assert!(launched.native_session_id.is_none());
        let token = launch_token_for(project, "impl-1700").expect("launcher holds the token");
        // A concurrent launch asking for the same name is refused.
        let err = reserve_handle(project, &launcher_req(Some("impl-1700"))).unwrap_err();
        assert!(err.to_string().contains("already in use"), "{err}");
        // The launched session's own startup binds the reservation.
        let bound = reserve_handle(
            project,
            &HandleRequest {
                launch_token: Some(&token),
                ..req(Some("impl-1700"), Some("s-1"))
            },
        )
        .unwrap();
        assert_eq!(bound.recipient_id, launched.recipient_id);
        assert_eq!(bound.native_session_id.as_deref(), Some("s-1"));
        assert!(bound.launch_token.is_none(), "a bound name keeps no token");
        // A live registry holder refuses another session's explicit claim.
        agent_registry::write_entry(
            project,
            &entry(project, "claude-9", Some("impl-1700"), Some("s-1")),
        )
        .unwrap();
        let err = reserve_handle(project, &req(Some("impl-1700"), Some("s-2"))).unwrap_err();
        assert!(
            err.to_string().contains("already in use by claude-9"),
            "{err}"
        );
    }

    // trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn reusing_a_retired_spelling_mints_a_new_recipient_and_keeps_provenance() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let old = reserve_handle(project, &req(Some("rework-2736"), Some("s-old"))).unwrap();
        let mut ended = entry(project, "claude-5", Some("rework-2736"), Some("s-old"));
        ended.ended_at = Some(Utc::now());
        agent_registry::write_entry(project, &ended).unwrap();
        let new = reserve_handle(project, &req(Some("rework-2736"), Some("s-new"))).unwrap();
        assert_ne!(new.recipient_id, old.recipient_id);
        assert_eq!(new.previous_recipients, vec![old.recipient_id]);
    }

    // trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn concurrent_allocators_never_share_a_handle() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().to_path_buf();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let project = project.clone();
                std::thread::spawn(move || {
                    let sid = format!("s-{i}");
                    reserve_handle(&project, &req(None, Some(&sid)))
                        .unwrap()
                        .handle
                })
            })
            .collect();
        let got: BTreeSet<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(got.len(), 8, "{got:?}");
    }

    #[test]
    fn payload_session_ids_cover_claude_and_agy_shapes() {
        assert_eq!(
            session_id_from_payload(r#"{"session_id":"abc"}"#).as_deref(),
            Some("abc")
        );
        assert_eq!(
            session_id_from_payload(r#"{"conversationId":"c-1"}"#).as_deref(),
            Some("c-1")
        );
        assert_eq!(session_id_from_payload("not json"), None);
    }

    // trace:STORY-1843.aceb5213 | ai:claude
    #[test]
    fn current_session_handle_matches_by_session_never_borrows_another() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        agent_registry::write_entry(
            project,
            &entry(
                project,
                "claude-77",
                Some("claude-implementer-04"),
                Some("sess-a"),
            ),
        )
        .unwrap();
        assert_eq!(
            current_session_handle(project, Some("sess-a")).as_deref(),
            Some("claude-implementer-04")
        );
        // An unknown session does not inherit it.
        let mut unrelated = entry(
            project,
            "claude-78",
            Some("claude-implementer-05"),
            Some("sess-b"),
        );
        unrelated.pid = 4_000_000;
        unrelated.pid_start = None;
        agent_registry::write_entry(project, &unrelated).unwrap();
        assert_ne!(
            current_session_handle(project, Some("sess-zzz")).as_deref(),
            Some("claude-implementer-05")
        );
    }

    // trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn session_start_registers_a_generated_handle_or_keeps_the_launch_name() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let binary = agent_registry::AgentBinaryIdentity::new("0.16.0".into(), "abc".into());
        {
            // trace:TASK-2349 | ai:claude
            let _env = crate::test_env::AgentSessionIsolation::apply(&[
                ("AIDA_AGENT_TYPE", Some("antigravity")),
                ("AIDA_SESSION_ROLE", Some("reviewer")),
            ]);
            let raw = agent_registry::touch_session_start_agent(
                project,
                project,
                Some("conv-1"),
                &binary,
            )
            .unwrap();
            assert_eq!(raw.name.as_deref(), Some("agy-reviewer-01"));
            let bound = load_reservation(project, "agy-reviewer-01").unwrap();
            assert_eq!(bound.native_session_id.as_deref(), Some("conv-1"));
            // The hook firing again for the same session keeps its handle.
            let again = agent_registry::touch_session_start_agent(
                project,
                project,
                Some("conv-1"),
                &binary,
            )
            .unwrap();
            assert_eq!(again.name, raw.name);
            assert_eq!(
                load_reservation(project, "agy-reviewer-01")
                    .unwrap()
                    .recipient_id,
                bound.recipient_id
            );
        }
        let _env = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_AGENT_TYPE", Some("codex")),
            ("AIDA_SESSION_ROLE", Some("implementer")),
            ("AIDA_AGENT_NAME", Some("impl-1700")),
        ]);
        let launched =
            agent_registry::touch_session_start_agent(project, project, Some("thread-9"), &binary)
                .unwrap();
        assert_eq!(launched.name.as_deref(), Some("impl-1700"));
        assert_eq!(
            load_reservation(project, "impl-1700")
                .unwrap()
                .native_session_id
                .as_deref(),
            Some("thread-9")
        );
    }

    // trace:STORY-1843.acfeb946 | ai:claude
    #[test]
    fn mail_to_a_handle_reaches_the_session_holding_it_and_nothing_else() {
        use aida_core::mailbox::{build_notice, Intent, Message, Recipient, SenderSource};
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let handle = reserve_handle(project, &req(None, Some("sess-a")))
            .unwrap()
            .handle;
        agent_registry::write_entry(
            project,
            &entry(project, "claude-77", Some(&handle), Some("sess-a")),
        )
        .unwrap();
        let dir = crate::mailbox_resolver::RecipientDirectory::load(
            project,
            &project.join(".aida-store"),
        )
        .unwrap();
        let resolved = dir.resolve(&handle).unwrap();
        assert_eq!(resolved.address, handle);
        // The holder's per-turn notice reads the handle's inbox.
        let identities = vec![current_session_handle(project, Some("sess-a")).unwrap()];
        let to_handle = Message {
            subject: None,
            id: "m1".to_string(),
            thread_id: "t1".to_string(),
            from: "advisor".to_string(),
            to: Recipient::Agent(resolved.address.clone()),
            timestamp: 100,
            in_reply_to: None,
            body: "addressed by handle".to_string(),
            urgent: false,
            intent: Intent::Request,
            retracted: false,
            deleted: false,
            archived: false,
            from_source: SenderSource::Explicit,
            from_role: None,
            relayed_from: None,
        };
        let notice = build_notice(
            identities.iter().map(String::as_str),
            std::slice::from_ref(&to_handle),
            &std::collections::HashMap::new(),
            aida_core::mailbox::NOTICE_DEFAULT_CAP,
        );
        assert_eq!(notice.total, 1, "{notice:?}");
        // Another session's notice does not see it. (Pinned: a parallel test
        // points `CLAUDE_PID` at this process.)
        let _env = crate::test_env::EnvVarsGuard::apply(&[("CLAUDE_PID", None)]);
        assert!(current_session_handle(project, Some("sess-other")).is_none());
        // A handle no live agent holds is refused with a hint, before any write.
        let err = dir
            .resolve("claude-implementer-99")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unknown recipient") && err.contains("operator"),
            "{err}"
        );
    }

    // trace:STORY-1843.aceb5213 | ai:claude
    #[test]
    fn statusline_session_evidence_covers_all_three_clients() {
        // Claude and Agy pipe a payload naming the session.
        assert_eq!(
            session_id_from_payload(r#"{"session_id":"claude-s","model":{}}"#).as_deref(),
            Some("claude-s")
        );
        assert_eq!(
            session_id_from_payload(r#"{"conversationId":"agy-c","model":"gemini"}"#).as_deref(),
            Some("agy-c")
        );
        // Codex renders through its title/tmux integration with no payload;
        // its per-session thread id names the session.
        // trace:TASK-2349 | ai:claude
        let _env =
            crate::test_env::AgentSessionIsolation::apply(&[("CODEX_THREAD_ID", Some("codex-t"))]);
        assert_eq!(current_native_session_id().as_deref(), Some("codex-t"));
    }

    // trace:STORY-1843.ac97b025 | ai:claude
    #[test]
    fn agent_ls_views_list_every_vendor_with_handle_role_spec_and_liveness() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let binary = agent_registry::AgentBinaryIdentity::new("0.16.0".into(), "abc".into());
        for (vendor, role, spec, sid) in [
            ("claude", "implementer", "STORY-1843", "claude-s"),
            ("codex", "reviewer", "TASK-1", "codex-t"),
            ("antigravity", "advisor", "BUG-2", "agy-c"),
        ] {
            // trace:TASK-2349 | ai:claude
            let _env = crate::test_env::AgentSessionIsolation::apply(&[
                ("AIDA_AGENT_TYPE", Some(vendor)),
                ("AIDA_SESSION_ROLE", Some(role)),
                ("AIDA_SESSION_SCOPE", Some(spec)),
            ]);
            agent_registry::touch_session_start_agent(project, project, Some(sid), &binary)
                .unwrap();
        }
        let mut gone = entry(
            project,
            "claude-4000000",
            Some("claude-implementer-09"),
            Some("old"),
        );
        gone.pid = 4_000_000;
        gone.pid_start = Some("1".to_string());
        agent_registry::write_entry(project, &gone).unwrap();

        let ctx = agent_registry::AgentClassifyContext::new(Utc::now(), 30, Vec::new());
        let views = agent_registry::list_agent_views(project, &ctx);
        let row = |handle: &str| views.iter().find(|v| v.name.as_deref() == Some(handle));
        for (handle, role, spec) in [
            ("claude-implementer-01", "implementer", "STORY-1843"),
            ("codex-reviewer-01", "reviewer", "TASK-1"),
            ("agy-advisor-01", "advisor", "BUG-2"),
        ] {
            let v = row(handle).unwrap_or_else(|| panic!("{handle} missing: {views:?}"));
            assert_eq!(v.role.as_deref(), Some(role));
            assert_eq!(v.current_spec.as_deref(), Some(spec));
            assert_ne!(
                v.status,
                agent_registry::AgentStatus::Stale,
                "{handle} is live"
            );
        }
        // A dead row is marked, never passed off as live.
        let dead = row("claude-implementer-09").expect("dead row listed under --all");
        assert_eq!(dead.status, agent_registry::AgentStatus::Stale);
    }
    // Review item 1: only the process a launcher started may bind its
    // reservation. trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn an_unrelated_session_cannot_bind_a_launchers_reservation() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        // `aida agent new` without -n reserves claude-implementer-01.
        let launched = reserve_handle(project, &launcher_req(None)).unwrap();
        assert_eq!(launched.handle, "claude-implementer-01");
        let token = launch_token_for(project, &launched.handle).unwrap();
        // An unrelated session's generated allocation skips it.
        let unrelated = reserve_handle(project, &req(None, Some("s-unrelated"))).unwrap();
        assert_eq!(unrelated.handle, "claude-implementer-02");
        // Naming it explicitly without the token is refused, even with a
        // session id or a forged token.
        let err =
            reserve_handle(project, &req(Some("claude-implementer-01"), Some("s-x"))).unwrap_err();
        assert!(err.to_string().contains("a launch reserved it"), "{err}");
        let forged = HandleRequest {
            launch_token: Some("not-the-token"),
            ..req(Some("claude-implementer-01"), Some("s-x"))
        };
        assert!(reserve_handle(project, &forged).is_err());
        // The launched child, carrying the token, binds the same recipient.
        let child = HandleRequest {
            launch_token: Some(&token),
            ..req(Some("claude-implementer-01"), Some("s-child"))
        };
        let bound = reserve_handle(project, &child).unwrap();
        assert_eq!(bound.recipient_id, launched.recipient_id);
        // A grandchild inheriting the spent token cannot take it over.
        let grandchild = HandleRequest {
            launch_token: Some(&token),
            ..req(Some("claude-implementer-01"), Some("s-grandchild"))
        };
        let err = reserve_handle(project, &grandchild).unwrap_err();
        assert!(err.to_string().contains("already in use"), "{err}");
    }

    // Review item 1 (launcher row): a live launcher row keeps its name even
    // after the reservation TTL when no token is presented.
    // trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn a_live_launcher_row_is_not_skipped_for_a_session_without_the_token() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let launched = reserve_handle(project, &launcher_req(Some("impl-7"))).unwrap();
        let mut row = entry(project, "claude-launch", Some("impl-7"), None);
        row.source = "agent-launcher".to_string();
        agent_registry::write_entry(project, &row).unwrap();
        let mut expired = launched.clone();
        expired.reserved_at =
            Utc::now() - chrono::Duration::seconds(UNBOUND_RESERVATION_TTL_SECS + 60);
        write_reservation(project, &expired).unwrap();
        let err = reserve_handle(project, &req(Some("impl-7"), Some("s-other"))).unwrap_err();
        assert!(err.to_string().contains("claude-launch"), "{err}");
    }

    // Review item 3: an unbound (released or expired) reservation is a
    // retired spelling, never re-bound with its old recipient.
    // trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn released_and_expired_reservations_mint_a_new_recipient() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        // Released: s-old held rework-1 then took another name.
        let old = reserve_handle(project, &req(Some("rework-1"), Some("s-old"))).unwrap();
        reserve_handle(project, &req(Some("rework-2"), Some("s-old"))).unwrap();
        let released = load_reservation(project, "rework-1").unwrap();
        assert!(released.native_session_id.is_none());
        let new = reserve_handle(project, &req(Some("rework-1"), Some("s-new"))).unwrap();
        assert_ne!(new.recipient_id, old.recipient_id);
        assert_eq!(new.previous_recipients, vec![old.recipient_id]);
        // Expired: a failed launch's reservation, past its TTL.
        let launched = reserve_handle(project, &launcher_req(Some("impl-9"))).unwrap();
        let mut expired = launched.clone();
        expired.reserved_at =
            Utc::now() - chrono::Duration::seconds(UNBOUND_RESERVATION_TTL_SECS + 60);
        write_reservation(project, &expired).unwrap();
        let taker = reserve_handle(project, &req(Some("impl-9"), Some("s-late"))).unwrap();
        assert_ne!(taker.recipient_id, launched.recipient_id);
        assert_eq!(taker.previous_recipients, vec![launched.recipient_id]);
    }

    // Review item 4: a failed launch or register releases its reservation,
    // and only while it is still unbound and still its own.
    // trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn a_failed_launch_releases_its_reservation_for_an_immediate_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        {
            let _guard = LaunchReservationGuard::new(project);
            reserve_handle(project, &launcher_req(Some("foo"))).unwrap();
            // The launch fails here; the guard drops armed.
        }
        assert!(load_reservation(project, "foo").is_none());
        let retry = reserve_handle(project, &launcher_req(Some("foo"))).unwrap();
        // A spawned launch disarms, and its child keeps the name.
        {
            let mut guard = LaunchReservationGuard::new(project);
            guard.disarm();
        }
        assert_eq!(
            load_reservation(project, "foo").unwrap().recipient_id,
            retry.recipient_id
        );
        // Compare-release leaves a reservation a session bound meanwhile.
        let token = launch_token_for(project, "foo").unwrap();
        reserve_handle(
            project,
            &HandleRequest {
                launch_token: Some(&token),
                ..req(Some("foo"), Some("s-1"))
            },
        )
        .unwrap();
        release_unbound(project, &retry).unwrap();
        assert_eq!(
            load_reservation(project, "foo")
                .unwrap()
                .native_session_id
                .as_deref(),
            Some("s-1")
        );
    }

    // Review item 2: a child of another vendor never resolves its parent
    // Claude session's handle from inherited env, and the launcher scrubs
    // that env. trace:STORY-1843.aceb5213 | ai:claude
    #[test]
    fn inherited_claude_session_env_does_not_identify_a_codex_or_agy_child() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        // A live parent: only live rows lend a handle.
        let parent = entry(
            project,
            "claude-parent",
            Some("claude-implementer-01"),
            Some("parent-s"),
        );
        agent_registry::write_entry(project, &parent).unwrap();
        let parent_pid = parent.pid.to_string();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HOOK_PAYLOAD", None),
            ("CLAUDE_CODE_SESSION_ID", Some("parent-s")),
            ("CLAUDE_PID", Some(parent_pid.as_str())),
            ("CODEX_THREAD_ID", None),
            ("CODEX_SESSION_ID", None),
        ]);
        for vendor in ["codex", "antigravity"] {
            let sid = current_native_session_id_for(vendor);
            assert_eq!(sid, None, "{vendor}");
            assert_eq!(
                current_session_handle_for(project, sid.as_deref(), vendor),
                None,
                "{vendor}"
            );
        }
        // The parent itself still resolves.
        let sid = current_native_session_id_for("claude");
        assert_eq!(
            current_session_handle_for(project, sid.as_deref(), "claude").as_deref(),
            Some("claude-implementer-01")
        );
        // The launcher hands a child none of it.
        let mut command = std::process::Command::new("true");
        apply_launch_env(&mut command, project, "no-such-handle");
        let removed: BTreeSet<String> = command
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        for key in [
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_PID",
            "CODEX_THREAD_ID",
            LAUNCH_TOKEN_ENV,
        ] {
            assert!(removed.contains(key), "{key} not scrubbed: {removed:?}");
        }
    }

    // Review item 5: the Codex tmux integration renders the pane's handle,
    // and a client-spawned render (Agy, whose statusline payload is not
    // documented upstream) finds its session by process ancestry.
    // trace:STORY-1843.aceb5213 | ai:claude
    #[test]
    fn statusline_handle_resolves_from_a_tmux_pane_and_from_the_spawning_client() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let mut codex = entry(project, "codex-1", Some("codex-implementer-01"), None);
        codex.agent_type = "codex".to_string();
        agent_registry::write_entry(project, &codex).unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HOOK_PAYLOAD", None),
            ("AIDA_AGENT_TYPE", Some("codex")),
            ("CODEX_THREAD_ID", None),
            ("CODEX_SESSION_ID", None),
            ("CLAUDE_PID", None),
        ]);
        // tmux's env has no session variable: no pane, no handle.
        assert_eq!(
            crate::statusline_cmd::statusline_session_handle(project, None, None, None),
            None
        );
        // The pane's shell is the agent's ancestor.
        let pane = agent_registry::parent_pid_of(std::process::id()).unwrap();
        assert_eq!(
            crate::statusline_cmd::statusline_session_handle(project, None, None, Some(pane))
                .as_deref(),
            Some("codex-implementer-01")
        );
        // A pane running no agent shows none.
        assert_eq!(
            crate::statusline_cmd::statusline_session_handle(project, None, None, Some(4_000_002)),
            None
        );
        // A client-spawned render with a payload lacking a session id.
        assert_eq!(
            crate::statusline_cmd::statusline_session_handle(
                project,
                Some("agy"),
                Some("{}"),
                None
            )
            .as_deref(),
            Some("codex-implementer-01")
        );
    }

    // Review item 6: an isolated sandbox cannot vouch that an unstamped row
    // is dead, so its holder keeps the name. trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn an_isolated_observer_keeps_an_unstamped_holder_on_its_heartbeat() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let mut legacy = entry(project, "claude-legacy", Some("impl-3"), None);
        legacy.pid = 4_000_003;
        legacy.pid_start = None;
        legacy.pid_namespace = None;
        let now = Utc::now();
        let initial = Some(agent_registry::INITIAL_PID_NAMESPACE);
        let isolated = agent_registry::PidObserver {
            namespace: initial,
            isolated: true,
        };
        let host = agent_registry::PidObserver {
            namespace: initial,
            isolated: false,
        };
        assert!(entry_may_hold_from(project, &legacy, now, isolated));
        assert!(!entry_may_hold_from(project, &legacy, now, host));
    }

    // Second review F2: `--bg` adoption never writes a name the namespace
    // refused. trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn bg_adoption_keeps_the_sessions_handle_when_the_name_is_held() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        reserve_handle(project, &req(Some("impl-1"), Some("s-b"))).unwrap();
        agent_registry::write_entry(
            project,
            &entry(project, "claude-9", Some("impl-1"), Some("s-b")),
        )
        .unwrap();
        let mut adoptee = entry(
            project,
            "claude-10",
            Some("claude-implementer-01"),
            Some("s-a"),
        );
        adoptee.pid = 4_000_004;
        adoptee.pid_start = None;
        agent_registry::write_entry(project, &adoptee).unwrap();
        agent_registry::adopt_bg_session_identity(project, "s-a", "impl-1", None, None, project);
        let rows = agent_registry::load_entries(project);
        let named: Vec<_> = rows
            .iter()
            .filter(|(_, e)| e.name.as_deref() == Some("impl-1"))
            .map(|(_, e)| e.id.clone())
            .collect();
        assert_eq!(named, vec!["claude-9".to_string()]);
    }

    // Second review F5/F6: one validator for every entry point, and the
    // HANDLE column never shows a non-address. trace:STORY-1843.aca8ca0d | ai:claude
    #[test]
    fn one_validator_refuses_generated_namespace_names_and_ls_marks_no_handle() {
        let reserved = BTreeSet::new();
        for bad in ["claude-implementer", "agy-foo", "codex-reviewer-x"] {
            let err = validate_explicit_handle(bad, &reserved).unwrap_err();
            assert!(err.to_string().contains("type+role prefix"), "{err}");
        }
        assert!(validate_explicit_handle("claude-implementer-01", &reserved).is_ok());
        assert_eq!(crate::agent_registry_root::ls_handle_cell(None), "-");
        assert_eq!(
            crate::agent_registry_root::ls_handle_cell(Some("impl-1")),
            "impl-1"
        );
    }

    fn unread_mail_to(handle: &str) -> aida_core::mailbox::Message {
        use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
        Message {
            subject: None,
            id: format!("m-{handle}"),
            thread_id: "t1".to_string(),
            from: "advisor".to_string(),
            to: Recipient::Agent(handle.to_string()),
            timestamp: 100,
            in_reply_to: None,
            body: "stale instruction".to_string(),
            urgent: false,
            intent: Intent::Request,
            retracted: false,
            deleted: false,
            archived: false,
            from_source: SenderSource::Explicit,
            from_role: None,
            relayed_from: None,
        }
    }

    fn command_env(cmd: &std::process::Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
    }

    // Round 5 F2: a resumed headless child keeps the handle its session
    // holds; the resume reserves no new one.
    // trace:STORY-1843 | ai:claude
    #[test]
    fn a_resumed_child_keeps_its_sessions_handle() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let _cwd = crate::session::contained_cwd_seam::pin(project);
        let held = reserve_handle(
            project,
            &HandleRequest {
                role: Some("reviewer"),
                ..req(None, Some("sess-r"))
            },
        )
        .unwrap();
        let mut cmd = std::process::Command::new("true");
        crate::session::apply_resumed_child_agent_identity(
            &mut cmd,
            crate::session::HeadlessVendor::Claude,
            "reviewer",
            "sess-r",
        );
        assert_eq!(
            command_env(&cmd, HANDLE_ENV).as_deref(),
            Some(held.handle.as_str())
        );
        assert_eq!(command_env(&cmd, LAUNCH_TOKEN_ENV), None);
        assert_eq!(list_reservations(project).len(), 1, "no new reservation");
        // The resumed session's start keeps its handle and recipient.
        let mut resumed = entry(project, "claude-r", Some(&held.handle), Some("sess-r"));
        resumed.role = Some("reviewer".to_string());
        assign_session_handle(project, &mut resumed, Some("sess-r"));
        assert_eq!(resumed.name.as_deref(), Some(held.handle.as_str()));
        assert_eq!(
            load_reservation(project, &held.handle)
                .unwrap()
                .recipient_id,
            held.recipient_id
        );
    }

    // Round 4 F1/F7: concurrent headless children of one seat each get a
    // launcher-reserved handle, bound by that child's launch token, and a
    // reply to each sender reaches only that child.
    // trace:STORY-1843 | ai:claude
    #[test]
    fn concurrent_children_of_one_seat_get_distinct_handles_and_replies_reach_each() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let _cwd = crate::session::contained_cwd_seam::pin(project);
        let launch = || {
            let mut cmd = std::process::Command::new("true");
            crate::session::apply_child_agent_identity(
                &mut cmd,
                crate::session::HeadlessVendor::Claude,
                "reviewer",
            );
            // The seat identity attestation records is unchanged.
            assert_eq!(
                command_env(&cmd, "AIDA_AGENT_NAME").as_deref(),
                Some("claude-reviewer")
            );
            (
                command_env(&cmd, HANDLE_ENV).unwrap(),
                command_env(&cmd, LAUNCH_TOKEN_ENV).unwrap(),
            )
        };
        let children = [launch(), launch()];
        assert_eq!(children[0].0, "claude-reviewer-01");
        assert_eq!(children[1].0, "claude-reviewer-02");
        for (i, (name, token)) in children.iter().enumerate() {
            let sid = format!("child-{i}");
            // trace:TASK-2349 | ai:claude
            let _env = crate::test_env::AgentSessionIsolation::apply(&[
                ("AIDA_AGENT_NAME", Some("claude-reviewer")),
                (HANDLE_ENV, Some(name.as_str())),
                (LAUNCH_TOKEN_ENV, Some(token.as_str())),
            ]);
            let mut child = entry(
                project,
                &format!("claude-child-{i}"),
                Some(name),
                Some(&sid),
            );
            child.role = Some("reviewer".to_string());
            assign_session_handle(project, &mut child, Some(&sid));
            assert_eq!(child.name.as_deref(), Some(name.as_str()));
            assert_eq!(
                load_reservation(project, name)
                    .unwrap()
                    .native_session_id
                    .as_deref(),
                Some(sid.as_str())
            );
            // The From address the child's mail carries is its handle.
            let (from, _) = crate::queue_misc::resolve_mail_sender_identity(None);
            assert_eq!(&from, name);
            agent_registry::write_entry(project, &child).unwrap();
        }
        let dir = crate::mailbox_resolver::RecipientDirectory::load(
            project,
            &project.join(".aida-store"),
        )
        .unwrap();
        for (i, (name, _)) in children.iter().enumerate() {
            assert_eq!(&dir.resolve(name).unwrap().address, name);
            let mine = current_session_handle_for(project, Some(&format!("child-{i}")), "claude");
            assert_eq!(mine.as_ref(), Some(name));
            let other = &children[1 - i].0;
            let notice = aida_core::mailbox::build_notice(
                mine.iter().map(String::as_str),
                &[unread_mail_to(other)],
                &std::collections::HashMap::new(),
                aida_core::mailbox::NOTICE_DEFAULT_CAP,
            );
            assert_eq!(notice.total, 0, "child {i} must not see {other}'s mail");
        }
    }

    // Round 4 F2: a dead holder's generated spelling is never handed out
    // again, so its unread mail is not shown to the next session.
    // trace:STORY-1843 | ai:claude
    #[test]
    fn a_dead_holders_generated_spelling_is_not_reused_for_its_unread_mail() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let first = reserve_handle(project, &req(None, Some("s-dead"))).unwrap();
        assert_eq!(first.handle, "claude-implementer-01");
        let mut dead = entry(project, "claude-dead", Some(&first.handle), Some("s-dead"));
        dead.ended_at = Some(Utc::now());
        agent_registry::write_entry(project, &dead).unwrap();
        let next = reserve_handle(project, &req(None, Some("s-next"))).unwrap();
        assert_eq!(next.handle, "claude-implementer-02");
        agent_registry::write_entry(
            project,
            &entry(project, "claude-next", Some(&next.handle), Some("s-next")),
        )
        .unwrap();
        let identities: Vec<String> = current_session_handle_for(project, Some("s-next"), "claude")
            .into_iter()
            .collect();
        assert_eq!(identities, vec![next.handle.clone()]);
        let notice = aida_core::mailbox::build_notice(
            identities.iter().map(String::as_str),
            &[unread_mail_to(&first.handle)],
            &std::collections::HashMap::new(),
            aida_core::mailbox::NOTICE_DEFAULT_CAP,
        );
        assert_eq!(notice.total, 0, "{notice:?}");
        // The same session resuming still reconnects to its own spelling.
        let again = reserve_handle(project, &req(None, Some("s-next"))).unwrap();
        assert_eq!(again.handle, next.handle);
    }

    // Round 4 F3: a payload session id that matched no row never falls back
    // to ancestry, which would render the parent session's handle.
    // trace:STORY-1843 | ai:claude
    #[test]
    fn an_unregistered_payload_session_never_borrows_its_parents_handle() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        agent_registry::write_entry(
            project,
            &entry(
                project,
                "claude-parent",
                Some("claude-implementer-01"),
                Some("parent-s"),
            ),
        )
        .unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HOOK_PAYLOAD", None),
            ("CLAUDE_CODE_SESSION_ID", None),
            ("CLAUDE_PID", None),
        ]);
        assert_eq!(
            crate::statusline_cmd::statusline_session_handle(
                project,
                Some("claude"),
                Some(r#"{"session_id":"child-not-registered-yet"}"#),
                None
            ),
            None
        );
        // Without a payload session id, the spawning client's row is found.
        assert_eq!(
            crate::statusline_cmd::statusline_session_handle(
                project,
                Some("claude"),
                Some("{}"),
                None
            )
            .as_deref(),
            Some("claude-implementer-01")
        );
    }

    // Round 4 F4 (sketch-mandatory): a handle confers no authority. A bound
    // handle spelling a reviewer or advisor seat, or a launch `-n` name, as
    // this process's agent name changes no grant, commit authority or
    // review-record identity, and an author stays refused.
    // trace:STORY-1843 | ai:claude
    #[test]
    fn a_handle_spelling_a_seat_or_launch_name_confers_no_authority() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let leases: Vec<crate::leases::SessionLease> = Vec::new();
        let baseline = {
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("AIDA_AGENT_NAME", None),
                (crate::gate::seat_authority::GRANT_ENV, None),
            ]);
            crate::gate::review_authority::RecorderIdentity::current(&leases)
        };
        let reviewer = reserve_handle(
            project,
            &HandleRequest {
                role: Some("reviewer"),
                ..req(None, Some("s-rev"))
            },
        )
        .unwrap();
        let advisor = reserve_handle(project, &req(Some("advisor-01"), Some("s-adv"))).unwrap();
        let launched = reserve_handle(project, &launcher_req(Some("reviewer-2864"))).unwrap();
        for (i, handle) in [&reviewer.handle, &advisor.handle, &launched.handle]
            .into_iter()
            .enumerate()
        {
            agent_registry::write_entry(
                project,
                &entry(project, &format!("claude-h{i}"), Some(handle), None),
            )
            .unwrap();
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("AIDA_AGENT_NAME", Some(handle.as_str())),
                (crate::gate::seat_authority::GRANT_ENV, None),
            ]);
            assert!(
                matches!(
                    crate::gate::seat_authority::grant_check(project),
                    crate::gate::seat_authority::GrantCheck::Absent
                ),
                "{handle}"
            );
            assert!(crate::gate::seat_authority::current_seat(project).is_none());
            assert!(
                crate::gate::seat_authority::resolve_commit_authority(&project.join(".aida-store"))
                    .is_none(),
                "{handle}"
            );
            let recorder = crate::gate::review_authority::RecorderIdentity::current(&leases);
            assert_eq!(recorder.tokens, baseline.tokens, "{handle}");
            assert!(!recorder.tokens.iter().any(|t| t.contains(handle.as_str())));
            // The author gate keys on session identity: the author recording
            // under a seat-shaped handle is still refused.
            let mut tokens = recorder.tokens.clone();
            tokens.push("session:author-s".to_string());
            let recorder = crate::gate::review_authority::RecorderIdentity::with_context(
                tokens,
                &[],
                &[],
                None,
            );
            let author = crate::gate::ship::AuthorSession {
                label: "implementer".to_string(),
                tokens: vec!["session:author-s".to_string()],
            };
            assert!(matches!(
                crate::gate::review_author::review_author_gate(
                    &[author],
                    &recorder,
                    true,
                    false,
                    || true
                ),
                crate::gate::review_author::ReviewAuthorGate::Refused(_)
            ));
        }
    }

    // trace:STORY-1843 | ai:claude (TASK-2302)
    #[test]
    fn a_failed_second_launch_releases_only_its_own_reservation() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let first = {
            let mut guard = LaunchReservationGuard::new(project);
            let r = reserve_handle(project, &launcher_req(Some("first"))).unwrap();
            guard.disarm();
            r
        };
        {
            let _guard = LaunchReservationGuard::new(project);
            reserve_handle(project, &launcher_req(Some("second"))).unwrap();
            // The second launch fails before its child spawns.
        }
        assert!(load_reservation(project, "second").is_none());
        let kept = load_reservation(project, "first").expect("first launch keeps its name");
        assert_eq!(kept.recipient_id, first.recipient_id);
        assert!(launch_token_for(project, "first").is_some());
    }

    // trace:STORY-1843 | ai:claude (TASK-2303)
    #[test]
    fn a_stale_row_with_a_dead_pid_lends_no_handle() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let mut stale = entry(project, "claude-stale", Some("impl-old"), Some("sess-dead"));
        stale.pid = 4_000_005;
        stale.pid_start = None;
        agent_registry::write_entry(project, &stale).unwrap();
        assert!(current_session_handle_for(project, Some("sess-dead"), "claude").is_none());
        assert!(session_handle_by_ancestry(project, 4_000_005).is_none());
        // The same row with a live process still resolves.
        agent_registry::write_entry(
            project,
            &entry(project, "claude-live", Some("impl-new"), Some("sess-live")),
        )
        .unwrap();
        assert_eq!(
            current_session_handle_for(project, Some("sess-live"), "claude").as_deref(),
            Some("impl-new")
        );
    }

    // trace:STORY-1843 | ai:claude (TASK-2304)
    #[test]
    fn a_session_id_prefix_never_matches_another_session() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let long = reserve_handle(project, &req(None, Some("sess-abcdef12"))).unwrap();
        let short = reserve_handle(project, &req(None, Some("sess-ab"))).unwrap();
        assert_ne!(short.handle, long.handle);
        assert_ne!(short.recipient_id, long.recipient_id);
        agent_registry::write_entry(
            project,
            &entry(
                project,
                "claude-1",
                Some(&long.handle),
                Some("sess-abcdef12"),
            ),
        )
        .unwrap();
        assert!(current_session_handle_for(project, Some("sess-abc"), "claude").is_none());
        assert!(current_session_handle_for(project, Some("sess-abcdef12-x"), "claude").is_none());
        assert_eq!(
            current_session_handle_for(project, Some("sess-abcdef12"), "claude").as_deref(),
            Some(long.handle.as_str())
        );
    }
    // trace:STORY-1843 | ai:codex
    #[test]
    fn story_1843_pr3_gc_archives_retired_preserving_resume_and_mail() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut retired = reserve_handle(root, &req(None, Some("old-session"))).unwrap();
        retired.reserved_at = Utc::now() - chrono::Duration::days(2);
        write_reservation(root, &retired).unwrap();
        let mail = root.join(".aida/mailbox/inboxes").join(&retired.handle);
        std::fs::create_dir_all(&mail).unwrap();
        std::fs::write(mail.join("unread"), "private old mail").unwrap();
        let live = reserve_handle(root, &launcher_req(None)).unwrap();
        assert_eq!(collect_retired_reservations(root, true).unwrap(), 1);
        assert!(reservation_path(root, &retired.handle).exists());
        assert_eq!(collect_retired_reservations(root, false).unwrap(), 1);
        assert!(!reservation_path(root, &retired.handle).exists());
        assert!(reservation_path(root, &live.handle).exists());
        assert_eq!(
            handle_of_session(root, "old-session"),
            Some(retired.handle.clone())
        );
        let resumed = reserve_handle(root, &req(None, Some("old-session"))).unwrap();
        assert_eq!(resumed.recipient_id, retired.recipient_id);
        let next = reserve_handle(root, &req(None, Some("new-session"))).unwrap();
        assert_ne!(next.handle, retired.handle);
        assert_eq!(
            std::fs::read_to_string(mail.join("unread")).unwrap(),
            "private old mail"
        );
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn story_1843_pr3_allocation_uses_counter_and_bounds_probes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        reserve_handle(root, &req(None, Some("first"))).unwrap();
        let counter = handles_dir(root).join("counters/claude-implementer-00");
        std::fs::write(&counter, "1000000").unwrap();
        // A corrupt historical candidate must not be probed from 1 again.
        std::fs::write(reservation_path(root, "claude-implementer-02"), "invalid").unwrap();
        let next = reserve_handle(root, &req(None, Some("second"))).unwrap();
        assert_eq!(next.handle, "claude-implementer-1000001");
        for seq in 1000002..=1000065 {
            let mut r = next.clone();
            r.handle = format!("claude-implementer-{seq}");
            r.native_session_id = Some(format!("other-{seq}"));
            write_reservation(root, &r).unwrap();
        }
        let err = reserve_handle(root, &req(None, Some("third"))).unwrap_err();
        assert!(err.to_string().contains("probe budget"), "{err:#}");
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn story_1843_pr3_session_start_binds_child_handle_env() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let r = reserve_handle(root, &launcher_req(Some("child-start"))).unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_AGENT_TYPE", Some("claude")),
            ("AIDA_HANDLE", Some("child-start")),
            (LAUNCH_TOKEN_ENV, r.launch_token.as_deref()),
            ("AIDA_AGENT_NAME", Some("claude-reviewer")),
            ("AIDA_SESSION_SCOPE", None),
        ]);
        let binary = agent_registry::AgentBinaryIdentity::new("test".into(), "test".into());
        let e =
            agent_registry::touch_session_start_agent(root, root, Some("child-native"), &binary)
                .unwrap();
        assert_eq!(e.name.as_deref(), Some("child-start"));
        assert_eq!(
            load_reservation(root, "child-start").unwrap().recipient_id,
            r.recipient_id
        );
        assert_eq!(
            handle_of_session(root, "child-native").as_deref(),
            Some("child-start")
        );
    }
    // trace:STORY-1843 | ai:codex
    #[test]
    fn story_1843_pr3_new_stem_ignores_large_retired_history() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let retired = handles_dir(root).join("retired");
        std::fs::create_dir_all(&retired).unwrap();
        for n in 0..=MAX_ALLOCATION_RECORDS {
            std::fs::write(retired.join(format!("old-{n}.toml")), "old history").unwrap();
        }
        let reservation = reserve_handle(
            root,
            &HandleRequest {
                agent_type: "antigravity",
                role: Some("reviewer"),
                native_session_id: Some("new-stem"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(reservation.handle, "agy-reviewer-01");
    }

    // A named resume must retain its UUID without interpreting a custom
    // spelling as a generated numeric counter.
    // trace:STORY-1843 | ai:codex
    #[test]
    fn story_1843_pr3_named_resume_keeps_recipient() {
        let tmp = tempfile::tempdir().unwrap();
        let original =
            reserve_handle(tmp.path(), &req(Some("named-child"), Some("named-session"))).unwrap();
        let resumed = reserve_handle(tmp.path(), &req(None, Some("named-session"))).unwrap();
        assert_eq!(original, resumed);
    }

    // FIFO regression has a process watchdog, so removing O_NONBLOCK cannot
    // hang a test shard or the SessionStart path unnoticed.
    // trace:STORY-1843 | ai:codex
    #[test]
    #[cfg(unix)]
    fn story_1843_pr3_allocation_refuses_fifo_without_blocking() {
        use std::os::unix::ffi::OsStrExt;
        if std::env::var_os("AIDA_TEST_HANDLE_FIFO_CHILD").is_some() {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("record.toml");
            let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
            assert!(bounded_records::<HandleReservation>(tmp.path()).is_err());
            for name in ["counter", "session-index"] {
                let path = tmp.path().join(name);
                let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
                assert!(read_allocation_text(&path, 64).is_err());
            }
            let lock = lock_namespace(tmp.path()).unwrap();
            let started = std::time::Instant::now();
            assert!(reserve_handle(tmp.path(), &req(None, Some("blocked"))).is_err());
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
            drop(lock);
            return;
        }
        let name = "agent_handle::tests::story_1843_pr3_allocation_refuses_fifo_without_blocking";
        let mut child = std::process::Command::new(
            crate::aida_bin::current_executable_path().expect("resolve current test executable"),
        )
        .args(["--exact", name])
        .env("AIDA_TEST_HANDLE_FIFO_CHILD", "1")
        .spawn()
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("allocation blocked on a FIFO or busy namespace lock");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    // trace:STORY-1843 | ai:codex
    #[test]
    fn story_1843_pr3_ended_registry_history_is_collectable_without_losing_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let original = reserve_handle(root, &req(None, Some("kept-session"))).unwrap();
        for n in 0..=MAX_ALLOCATION_RECORDS {
            let mut row = entry(
                root,
                &format!("claude-old-{n}"),
                Some("old-handle"),
                Some(&format!("old-session-{n}")),
            );
            row.ended_at = Some(Utc::now());
            agent_registry::write_entry(root, &row).unwrap();
        }
        assert_eq!(
            handle_of_session(root, "kept-session"),
            Some(original.handle.clone())
        );
        assert!(reserve_handle(root, &req(None, Some("fresh"))).is_err());
        assert_eq!(
            agent_registry::archive_ended_entries(root, false).unwrap(),
            MAX_ALLOCATION_RECORDS + 1
        );
        assert_eq!(
            agent_registry::load_entries(root).len(),
            MAX_ALLOCATION_RECORDS + 1
        );
        assert!(agent_registry::load_active_entries(root).is_empty());
        let resumed = reserve_handle(root, &req(None, Some("kept-session"))).unwrap();
        assert_eq!(resumed.recipient_id, original.recipient_id);
        assert!(reserve_handle(root, &req(None, Some("fresh"))).is_ok());
    }
}
