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
//! for [`UNBOUND_RESERVATION_TTL_SECS`]. The same native session (resume,
//! `/clear` in the same process) reconnects to its UUID; a different session
//! that reuses a retired spelling gets a new UUID.
// trace:STORY-1843 | ai:claude

use crate::agent_registry::{self, AgentRegistryEntry};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Longest handle accepted, so a handle fits a statusline and a DNS label.
pub(crate) const HANDLE_MAX_LEN: usize = 63;

/// How long a launcher's reservation holds its name before the launched
/// process registers under it.
pub(crate) const UNBOUND_RESERVATION_TTL_SECS: i64 = 10 * 60;

/// Unverified (PID namespace unobservable) holders keep their handle only
/// while their heartbeat is this recent, matching the mail resolver.
const UNVERIFIED_HOLDER_TTL_SECS: i64 = 24 * 60 * 60;

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
}

/// Why `name` is not a valid handle spelling, or `None` when it is.
pub(crate) fn handle_spelling_rejection(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("is empty".to_string());
    }
    if name.len() > HANDLE_MAX_LEN {
        return Some(format!("is longer than {HANDLE_MAX_LEN} characters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Some("may contain only lowercase letters, digits and '-'".to_string());
    }
    if !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return Some("must start with a letter or digit".to_string());
    }
    None
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

struct NamespaceLock {
    file: std::fs::File,
}

impl Drop for NamespaceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// The common-root allocation lock. Held across read-decide-write so a
/// reservation is atomic with respect to every other allocator.
fn lock_namespace(project_root: &Path) -> Result<NamespaceLock> {
    let dir = handles_dir(project_root);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating handle directory {}", dir.display()))?;
    let path = dir.join(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening handle lock {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("locking {}", path.display()))?;
    Ok(NamespaceLock { file })
}

pub(crate) fn load_reservation(project_root: &Path, handle: &str) -> Option<HandleReservation> {
    let body = std::fs::read_to_string(reservation_path(project_root, handle)).ok()?;
    toml::from_str(&body).ok()
}

fn write_reservation(project_root: &Path, r: &HandleReservation) -> Result<()> {
    let body = toml::to_string_pretty(r).context("serialising handle reservation")?;
    let path = reservation_path(project_root, &r.handle);
    aida_core::write_atomic(&path, body.as_bytes())
        .with_context(|| format!("writing handle reservation {}", path.display()))
}

/// Whether a registry entry may still be a live holder: observably alive,
/// or unobservable from here with a recent heartbeat.
fn entry_may_hold(project_root: &Path, entry: &AgentRegistryEntry, now: DateTime<Utc>) -> bool {
    if entry.source == "role-enter" {
        return false;
    }
    let observer = agent_registry::current_pid_namespace();
    agent_registry::observed_liveness(project_root, entry, observer).unwrap_or_else(|| {
        let age = now.signed_duration_since(entry.last_active_at);
        age >= chrono::Duration::zero()
            && age <= chrono::Duration::seconds(UNVERIFIED_HOLDER_TTL_SECS)
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
    let binding_launch = req.native_session_id.is_some()
        && reservation.is_some_and(|r| r.native_session_id.is_none());
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
    if session_matches(r, req.native_session_id)
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
        // is about to start; only a session-bearing request (that process's
        // own startup) may bind it.
        None => req
            .native_session_id
            .is_none()
            .then(|| format!("a launch reserved it at {}", r.reserved_at.to_rfc3339())),
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

fn same_session(entry: &AgentRegistryEntry, req: &HandleRequest<'_>) -> bool {
    let Some(sid) = req.native_session_id.filter(|s| !s.is_empty()) else {
        return false;
    };
    [&entry.claude_session_id, &entry.native_session_id]
        .into_iter()
        .flatten()
        .any(|own| !own.is_empty() && (own.starts_with(sid) || sid.starts_with(own.as_str())))
}

fn session_matches(reservation: &HandleReservation, sid: Option<&str>) -> bool {
    match (reservation.native_session_id.as_deref(), sid) {
        (Some(own), Some(sid)) if !own.is_empty() && !sid.is_empty() => {
            own.starts_with(sid) || sid.starts_with(own)
        }
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
    let reservation = match existing {
        Some(r) if session_matches(&r, sid) => r,
        // The launcher reserved it for the process now binding it.
        Some(mut r) if r.native_session_id.is_none() && sid.is_some() => {
            r.native_session_id = sid.map(str::to_string);
            r.registry_id = r.registry_id.or(req.registry_id.map(str::to_string));
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
    let entries: Vec<AgentRegistryEntry> = agent_registry::load_entries(project_root)
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    if let Some(name) = req.explicit {
        let existing = load_reservation(project_root, name);
        if let Some(holder) =
            other_live_holder(project_root, &entries, existing.as_ref(), name, req, now)
        {
            bail!("agent name `{name}` is already in use by {holder}");
        }
        return Ok((name.to_string(), existing));
    }
    if let Some(sid) = req.native_session_id.filter(|s| !s.is_empty()) {
        if let Some(r) = list_reservations(project_root)
            .into_iter()
            .find(|r| session_matches(r, Some(sid)))
        {
            return Ok((r.handle.clone(), Some(r)));
        }
    }
    for seq in 1.. {
        let candidate = generated_handle(req.agent_type, req.role, seq);
        if handle_spelling_rejection(&candidate).is_some() || reserved.contains(&candidate) {
            bail!("cannot generate a valid handle for `{}`", req.agent_type);
        }
        let existing = load_reservation(project_root, &candidate);
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
            return Ok((candidate, existing));
        }
    }
    unreachable!("the handle sequence is unbounded")
}

/// A session holds one handle: binding it to `taken` unbinds any other
/// reservation the same session held (e.g. a generated handle a `--bg`
/// session got before its launch renamed it). The unbound spelling keeps
/// its recipient and history and becomes free once no live entry names it.
fn release_other_bindings(project_root: &Path, taken: &HandleReservation) -> Result<()> {
    let Some(sid) = taken.native_session_id.as_deref() else {
        return Ok(());
    };
    for mut r in list_reservations(project_root) {
        if r.handle != taken.handle && session_matches(&r, Some(sid)) {
            r.native_session_id = None;
            // Not a fresh launch promise: an unbound reservation older than
            // the TTL holds nothing.
            r.reserved_at = DateTime::<Utc>::UNIX_EPOCH;
            write_reservation(project_root, &r)?;
        }
    }
    Ok(())
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
    let base = HandleRequest {
        agent_type: &agent_type,
        role: role.as_deref(),
        explicit: None,
        native_session_id,
        registry_id: Some(&id),
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

pub(crate) fn list_reservations(project_root: &Path) -> Vec<HandleReservation> {
    let mut out = Vec::new();
    if let Ok(dir) = std::fs::read_dir(handles_dir(project_root)) {
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            if let Some(r) = std::fs::read_to_string(&path)
                .ok()
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
/// hook relay exported, else the client's own per-session environment.
/// A shared daemon's inherited role or name is never consulted.
pub(crate) fn current_native_session_id() -> Option<String> {
    if let Some(sid) = std::env::var("AIDA_HOOK_PAYLOAD")
        .ok()
        .and_then(|raw| session_id_from_payload(&raw))
    {
        return Some(sid);
    }
    [
        "CLAUDE_CODE_SESSION_ID",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
    ]
    .iter()
    .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
}

/// A client payload's session id: Claude/Codex `session_id`, Agy
/// `conversationId`.
pub(crate) fn session_id_from_payload(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    [
        "session_id",
        "sessionId",
        "conversationId",
        "conversation_id",
    ]
    .iter()
    .find_map(|k| v.get(*k).and_then(|s| s.as_str()).map(str::to_string))
    .filter(|s| !s.trim().is_empty())
}

/// The handle of the session this process runs in: matched by native
/// session id, else by the client's pid (`CLAUDE_PID` is a recorded value,
/// so it matches even inside a PID-namespaced sandbox). `None` when nothing
/// identifies it: a session never borrows another session's handle.
pub(crate) fn current_session_handle(
    project_root: &Path,
    native_session_id: Option<&str>,
) -> Option<String> {
    let entries: Vec<AgentRegistryEntry> = agent_registry::load_entries(project_root)
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| e.ended_at.is_none() && e.source != "role-enter")
        .collect();
    let valid = |e: &AgentRegistryEntry| {
        e.name
            .as_deref()
            .filter(|n| handle_spelling_rejection(n).is_none())
            .map(str::to_string)
    };
    if let Some(sid) = native_session_id.filter(|s| !s.trim().is_empty()) {
        let req = HandleRequest {
            native_session_id: Some(sid),
            ..Default::default()
        };
        if let Some(h) = entries
            .iter()
            .filter(|e| same_session(e, &req))
            .max_by_key(|e| e.last_active_at)
            .and_then(valid)
        {
            return Some(h);
        }
    }
    if let Some(pid) = std::env::var("CLAUDE_PID")
        .ok()
        .and_then(|p| p.trim().parse::<u32>().ok())
    {
        if let Some(h) = entries
            .iter()
            .filter(|e| e.pid == pid && e.agent_type == "claude")
            .max_by_key(|e| e.last_active_at)
            .and_then(valid)
        {
            return Some(h);
        }
    }
    None
}

/// [`current_session_handle`] for the project containing the cwd, using
/// this process's own session evidence.
pub(crate) fn current_session_handle_for_cwd() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let root = crate::main_worktree_root_from(&crate::find_aida_project_root_from(&cwd).ok()?);
    current_session_handle(&root, current_native_session_id().as_deref())
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
        let launched = reserve_handle(project, &req(Some("impl-1700"), None)).unwrap();
        assert!(launched.native_session_id.is_none());
        // A concurrent launch asking for the same name is refused.
        let err = reserve_handle(project, &req(Some("impl-1700"), None)).unwrap_err();
        assert!(err.to_string().contains("already in use"), "{err}");
        // The launched session's own startup binds the reservation.
        let bound = reserve_handle(project, &req(Some("impl-1700"), Some("s-1"))).unwrap();
        assert_eq!(bound.recipient_id, launched.recipient_id);
        assert_eq!(bound.native_session_id.as_deref(), Some("s-1"));
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
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("AIDA_AGENT_TYPE", Some("antigravity")),
                ("AIDA_SESSION_ROLE", Some("reviewer")),
                ("AIDA_SESSION_SCOPE", None),
                ("AIDA_AGENT_NAME", None),
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
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_AGENT_TYPE", Some("codex")),
            ("AIDA_SESSION_ROLE", Some("implementer")),
            ("AIDA_SESSION_SCOPE", None),
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
        // Another session's notice does not see it.
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
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HOOK_PAYLOAD", None),
            ("CLAUDE_CODE_SESSION_ID", None),
            ("CODEX_THREAD_ID", Some("codex-t")),
        ]);
        assert_eq!(current_native_session_id().as_deref(), Some("codex-t"));
    }
}
