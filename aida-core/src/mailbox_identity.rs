//! Which inboxes a mail reader may see (STORY-1843 slice B2b).
//!
//! The mailbox identity policy used to live in the CLI, so core could not
//! mint a non-empty [`crate::mailbox_logical::ResolvedAudience`] and any
//! crate could hand the snapshot a list of strings as recipient authority.
//! The policy now lives here, over inputs that only core produces:
//!
//! - [`IdentityEnv`]: this process's own environment, read by core.
//! - [`PeopleCatalogView`]: the store's people catalog, read by core.
//! - [`SessionBindingFacts`]: a persisted agent-registry row, read by core
//!   and accepted only when this process's own evidence binds it (the row's
//!   process is this process or an ancestor, or its native session id is in
//!   this process's hook payload or client session environment).
//!
//! None of them can be built from caller-supplied strings outside tests, so
//! the audience a snapshot filters to is always derived from the reader's
//! own process and store, never from a raw `--agent` value.
// trace:STORY-1843 | ai:claude

use crate::mailbox_delivery::RecipientUuid;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// Environment keys the identity policy reads.
const HANDLE_ENV: &str = "AIDA_HANDLE";
const ALIAS_ENVS: [&str; 2] = [HANDLE_ENV, "AIDA_AGENT_NAME"];
/// Upper bound on a registry row this module will parse.
const REGISTRY_ROW_BYTES: u64 = 64 * 1024;

fn nonempty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

/// Canonical role spelling: `dialog` is the advisor; `human`, `guest` and
/// `requester` fold to lowercase; anything else is kept as written.
pub fn canonical_role_name(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("dialog") {
        "advisor".to_string()
    } else if raw.eq_ignore_ascii_case("human")
        || raw.eq_ignore_ascii_case("guest")
        || raw.eq_ignore_ascii_case("requester")
    {
        raw.to_ascii_lowercase()
    } else {
        raw.to_string()
    }
}

/// The effective role for a raw `AIDA_SESSION_ROLE`: blank is the implicit
/// `implementer` default (`true`), anything else is canonicalized.
pub fn effective_role(raw: Option<&str>) -> (String, bool) {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(r) => (canonical_role_name(r), false),
        None => ("implementer".to_string(), true),
    }
}

/// The short agent type a vendor spelling names.
pub fn normalize_agent_type(raw: &str) -> String {
    match raw.to_ascii_lowercase().replace(['_', '-'], "").as_str() {
        "claude" | "claudecode" => "claude",
        "codex" => "codex",
        "antigravity" | "gemini" => "antigravity",
        "shell" => "shell",
        "web" => "web",
        _ => "other",
    }
    .to_string()
}

/// The identity-relevant part of this process's environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityEnv {
    /// `AIDA_USER`, else `USER`, else `USERNAME`, else `default`.
    user: String,
    /// Non-empty handle/launch-name values, in precedence order.
    aliases: Vec<String>,
    /// Raw non-empty `AIDA_SESSION_ROLE`.
    role: Option<String>,
    /// Raw non-empty `AIDA_AGENT_TYPE`.
    vendor: Option<String>,
    /// Session id from the hook payload this process was handed.
    payload_session: Option<String>,
    /// Client session variables, as `(vendor, value)`.
    client_sessions: Vec<(&'static str, String)>,
    /// `CLAUDE_PID`, when it parses.
    claude_pid: Option<u32>,
}

impl IdentityEnv {
    /// Read this process's own environment.
    pub fn observed() -> Self {
        let user = ["AIDA_USER", "USER", "USERNAME"]
            .into_iter()
            .find_map(|k| std::env::var(k).ok())
            .unwrap_or_else(|| "default".to_string());
        let client_sessions = [
            ("claude", "CLAUDE_CODE_SESSION_ID"),
            ("codex", "CODEX_THREAD_ID"),
            ("codex", "CODEX_SESSION_ID"),
        ]
        .into_iter()
        .filter_map(|(vendor, key)| nonempty_env(key).map(|v| (vendor, v)))
        .collect();
        Self {
            user,
            aliases: ALIAS_ENVS.into_iter().filter_map(nonempty_env).collect(),
            role: nonempty_env("AIDA_SESSION_ROLE"),
            vendor: nonempty_env("AIDA_AGENT_TYPE"),
            payload_session: std::env::var("AIDA_HOOK_PAYLOAD")
                .ok()
                .and_then(|raw| session_id_from_payload(&raw)),
            client_sessions,
            claude_pid: std::env::var("CLAUDE_PID")
                .ok()
                .and_then(|p| p.trim().parse().ok()),
        }
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    /// Whether `sid` is this process's own native session id for a client
    /// of `vendor`: from the hook payload, or that vendor's own session
    /// variable (a child of another vendor may inherit the parent's).
    fn owns_session(&self, vendor: &str, sid: &str) -> bool {
        self.payload_session.as_deref() == Some(sid)
            || self
                .client_sessions
                .iter()
                .any(|(v, value)| *v == vendor && value == sid)
    }

    /// Test-only environment.
    #[cfg(test)]
    pub(crate) fn fixture(user: &str, aliases: &[&str], role: Option<&str>) -> Self {
        Self {
            user: user.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            role: role.map(str::to_string),
            ..Self::default()
        }
    }
}

/// A client payload's session id: Claude/Codex `session_id`, Agy
/// `conversationId`.
pub fn session_id_from_payload(raw: &str) -> Option<String> {
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

/// The people catalog of one store: team members, roster keys, user
/// handles and person aliases, keyed case-folded, valued by the stored
/// spelling (first source wins). Human inboxes need no running process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeopleCatalogView {
    by_key: HashMap<String, String>,
}

impl PeopleCatalogView {
    /// Read the catalog of the store at `store_root`. A missing store reads
    /// as an empty catalog.
    pub fn load(store_root: &Path) -> Self {
        let mut by_key = HashMap::new();
        let mut add = |s: &str| {
            let t = s.trim();
            if !t.is_empty() {
                by_key
                    .entry(t.to_lowercase())
                    .or_insert_with(|| t.to_string());
            }
        };
        for member in crate::team::build_team_members(store_root) {
            add(&member.user_id);
        }
        for (user, _) in crate::team::TeamRoster::load(store_root).entries() {
            add(&user);
        }
        if let Ok(store) = crate::GitBackend::read_metadata_only(store_root) {
            for user in store.users {
                add(&user.handle);
            }
        }
        let aliases = crate::alias::AliasRegistry::load(store_root);
        for (alias, canonical) in &aliases.aliases {
            add(canonical);
            add(alias);
        }
        Self { by_key }
    }

    /// Case-folded key → stored spelling.
    pub fn spellings(&self) -> &HashMap<String, String> {
        &self.by_key
    }

    pub fn contains(&self, name: &str) -> bool {
        self.by_key.contains_key(&name.trim().to_lowercase())
    }

    #[cfg(test)]
    pub(crate) fn fixture(people: &[&str]) -> Self {
        Self {
            by_key: people
                .iter()
                .map(|p| (p.to_lowercase(), p.to_string()))
                .collect(),
        }
    }
}

/// Fields of a persisted agent-registry row this module needs. Unknown
/// fields are ignored, so newer writers stay readable.
#[derive(Debug, serde::Deserialize)]
struct RegistryRow {
    id: String,
    agent_type: String,
    pid: u32,
    #[serde(default)]
    pid_start: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    native_session_id: Option<String>,
    #[serde(default)]
    claude_session_id: Option<String>,
    #[serde(default)]
    ended_at: Option<toml::Value>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    mail_wake: Option<String>,
}

/// Why a registry row was not accepted as this process's session binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingRefusal {
    /// The row could not be read as a bounded regular file.
    Unreadable,
    /// The row does not parse, or its id does not match its file name.
    Corrupt,
    /// The session the row records has ended.
    Ended,
    /// The row carries no valid handle.
    NoHandle,
    /// Nothing in this process's own evidence binds it to the row.
    NotThisProcess,
    /// The row is not armed for a mail-wake watcher.
    NotArmed,
    /// The row's process is not provably the live process it recorded.
    NotLive,
}

/// The persisted session binding of this process: its handle and vendor,
/// taken from one registry row that this process's own evidence binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBindingFacts {
    handle: String,
    vendor: String,
    /// The handle reservation's recipient UUID, when the reservation names
    /// this row (slice D1). `None`: a UUID-less reader.
    recipient: Option<RecipientUuid>,
}

impl SessionBindingFacts {
    /// Accept the registry row at `row` (a `<id>.toml` file the caller's
    /// session lookup selected) only when it is live and bound to this
    /// process: its process is this process or an ancestor (start identity
    /// matching when recorded), or its native session id is this process's
    /// own (hook payload or that vendor's session variable), or, for a
    /// Claude row, `CLAUDE_PID` names it.
    pub fn verify(row: &Path, env: &IdentityEnv) -> Result<Self, BindingRefusal> {
        let body = read_bounded_regular(row).ok_or(BindingRefusal::Unreadable)?;
        let parsed: RegistryRow = toml::from_str(&body).map_err(|_| BindingRefusal::Corrupt)?;
        if row.file_stem().and_then(|s| s.to_str()) != Some(parsed.id.as_str()) {
            return Err(BindingRefusal::Corrupt);
        }
        if parsed.ended_at.is_some() {
            return Err(BindingRefusal::Ended);
        }
        let handle = parsed
            .name
            .clone()
            .filter(|n| valid_handle(n))
            .ok_or(BindingRefusal::NoHandle)?;
        let vendor = normalize_agent_type(&parsed.agent_type);
        let by_session = [&parsed.claude_session_id, &parsed.native_session_id]
            .into_iter()
            .flatten()
            .any(|sid| !sid.trim().is_empty() && env.owns_session(&vendor, sid));
        let by_claude_pid = vendor == "claude" && env.claude_pid == Some(parsed.pid);
        if !(by_session || by_claude_pid || is_self_or_ancestor(&parsed)) {
            return Err(BindingRefusal::NotThisProcess);
        }
        let sessions: Vec<&str> = [&parsed.claude_session_id, &parsed.native_session_id]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        let recipient = row
            .parent()
            .and_then(|agents| reservation_recipient(agents, &handle, &parsed.id, &sessions));
        Ok(Self {
            handle,
            vendor,
            recipient,
        })
    }

    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// The recipient UUID this binding owns, when its reservation names it.
    pub fn recipient(&self) -> Option<&RecipientUuid> {
        self.recipient.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn fixture(handle: &str, vendor: &str) -> Self {
        Self {
            handle: handle.to_string(),
            vendor: vendor.to_string(),
            recipient: None,
        }
    }
}

/// The handle reservation as the identity policy reads it.
#[derive(Deserialize)]
struct ReservationRow {
    handle: String,
    recipient_id: String,
    #[serde(default)]
    native_session_id: Option<String>,
    #[serde(default)]
    registry_id: Option<String>,
}

/// The recipient UUID the reservation of `handle` (under `agents_dir`'s
/// `handles/`) gives the registry row `row_id`: only when the reservation
/// names that row, or one of the row's native sessions (`sessions`). A
/// reservation naming neither, a missing, unreadable or malformed one, or a
/// non-UUID `recipient_id` yields `None`, never a UUID. Bounded, no-follow.
// trace:STORY-1843 | ai:claude
pub fn reservation_recipient(
    agents_dir: &Path,
    handle: &str,
    row_id: &str,
    sessions: &[&str],
) -> Option<RecipientUuid> {
    if !valid_handle(handle) {
        return None;
    }
    let path = agents_dir.join("handles").join(format!("{handle}.toml"));
    let body = read_bounded_regular(&path)?;
    reservation_from_body(&body, handle, row_id, sessions)
}

fn reservation_from_body(
    body: &str,
    handle: &str,
    row_id: &str,
    sessions: &[&str],
) -> Option<RecipientUuid> {
    let r: ReservationRow = toml::from_str(body).ok()?;
    if r.handle != handle {
        return None;
    }
    let by_row = r.registry_id.as_deref() == Some(row_id);
    let by_session = r
        .native_session_id
        .as_deref()
        .is_some_and(|sid| !sid.trim().is_empty() && sessions.contains(&sid));
    (by_row || by_session)
        .then(|| RecipientUuid::parse(&r.recipient_id))
        .flatten()
}

/// Longest handle registry rows are allocated under.
pub const HANDLE_MAX_LEN: usize = 63;

/// Why `name` is not a valid handle spelling, or `None` when it is.
pub fn handle_spelling_rejection(name: &str) -> Option<String> {
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

/// The handle spelling registry rows are allocated under.
fn valid_handle(name: &str) -> bool {
    handle_spelling_rejection(name).is_none()
}

fn read_bounded_regular(path: &Path) -> Option<String> {
    read_regular(path, false)
}

fn read_regular(path: &Path, inspection: bool) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file()
        || meta.len() > REGISTRY_ROW_BYTES
        || (inspection
            && (meta.nlink() != 1
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o022 != 0))
    {
        return None;
    }
    let mut body = String::new();
    file.take(REGISTRY_ROW_BYTES)
        .read_to_string(&mut body)
        .ok()?;
    Some(body)
}

/// Whether the row's process is this process or one of its ancestors. A
/// recorded start identity must positively match the live process under its
/// own scheme, so a recycled pid never binds; a row with no recorded start,
/// a record in an unknown scheme, or one that cannot be compared refuses
/// (such a row binds only by session id or `CLAUDE_PID`).
fn is_self_or_ancestor(row: &RegistryRow) -> bool {
    let Ok(chain) = crate::process_probe::ProbeSource::system().ancestry(std::process::id()) else {
        return false;
    };
    row_binds_chain(
        row.pid,
        row.pid_start.as_deref(),
        chain.iter().map(|p| p.identity.pid),
        crate::liveness::process_identity_matches_strict,
    )
}

/// [`is_self_or_ancestor`] over an explicit ancestry and identity matcher.
fn row_binds_chain(
    pid: u32,
    pid_start: Option<&str>,
    mut chain: impl Iterator<Item = u32>,
    matches: impl Fn(u32, &str) -> bool,
) -> bool {
    // trace:STORY-1843 | ai:claude
    chain.any(|p| p == pid) && pid_start.is_some_and(|recorded| matches(pid, recorded))
}

/// What this process's session lookup found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionBinding {
    /// A registry row this process's own evidence binds.
    Bound(SessionBindingFacts),
    /// No session row names this process (a plain shell or CI runner).
    Unbound,
    /// A row was selected but could not be verified. Fails closed: no
    /// private inbox is granted, not even the inherited user's.
    Unverified(BindingRefusal),
}

/// The agent registry directory of the repository holding `project_root`:
/// the main checkout's `.aida/agents` (the verified common Git directory's
/// parent), else `project_root`'s own for a non-Git project.
pub fn registry_dir_for(project_root: &Path) -> std::path::PathBuf {
    let main = crate::mailbox_logical::MailboxContext::resolve(project_root, None)
        .ok()
        .and_then(|ctx| {
            ctx.repository()
                .and_then(|git| git.path().parent().map(Path::to_path_buf))
        })
        .unwrap_or_else(|| project_root.to_path_buf());
    main.join(".aida").join("agents")
}

/// Upper bound on registry rows a session lookup reads.
const REGISTRY_ROWS_MAX: usize = 1024;

impl SessionBinding {
    /// The verified session lookup for a process with no CLI session layer
    /// (the TUI): among the live rows in the registry directory
    /// `agents_dir`, the row of this process's nearest registered ancestor
    /// (this process first), else a row whose native session id this process
    /// owns, accepted only through [`SessionBindingFacts::verify`]. A row
    /// with no handle is skipped; when every candidate fails verification
    /// the binding is [`Self::Unverified`] and grants no private inbox. No
    /// candidate is [`Self::Unbound`]. Mirrors the CLI's nearest-ancestor
    /// lookup, so a surface started from an agent session's shell reads as
    /// that session and never takes the inherited environment as its own.
    pub fn lookup(agents_dir: &Path, env: &IdentityEnv) -> Self {
        let chain: Vec<u32> = crate::process_probe::ProbeSource::system()
            .ancestry(std::process::id())
            .map(|c| c.iter().map(|p| p.identity.pid).collect())
            .unwrap_or_default();
        Self::lookup_in_chain(agents_dir, env, &chain)
    }

    fn lookup_in_chain(agents_dir: &Path, env: &IdentityEnv, chain: &[u32]) -> Self {
        let Ok(dir) = std::fs::read_dir(agents_dir) else {
            return Self::Unbound;
        };
        let mut candidates: Vec<(usize, std::path::PathBuf)> = Vec::new();
        for entry in dir.flatten().take(REGISTRY_ROWS_MAX) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Some(row) = read_bounded_regular(&path)
                .and_then(|body| toml::from_str::<RegistryRow>(&body).ok())
            else {
                continue;
            };
            if row.ended_at.is_some() {
                continue;
            }
            let vendor = normalize_agent_type(&row.agent_type);
            let rank = match chain.iter().position(|pid| *pid == row.pid) {
                Some(depth) => depth,
                None if [&row.claude_session_id, &row.native_session_id]
                    .into_iter()
                    .flatten()
                    .any(|sid| !sid.trim().is_empty() && env.owns_session(&vendor, sid)) =>
                {
                    chain.len()
                }
                None => continue,
            };
            candidates.push((rank, path));
        }
        candidates.sort();
        let mut refusal = None;
        for (_, row) in candidates {
            match SessionBindingFacts::verify(&row, env) {
                Ok(facts) => return Self::Bound(facts),
                Err(BindingRefusal::NoHandle) => {}
                Err(why) => {
                    refusal.get_or_insert(why);
                }
            }
        }
        refusal.map_or(Self::Unbound, Self::Unverified)
    }
}

/// Everything the identity policy decides from, gathered once. Fields are
/// private: production builds it only through [`Self::observed`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailIdentityFacts {
    own_handle: Option<String>,
    own_recipient: Option<RecipientUuid>,
    own_vendor: Option<String>,
    user: String,
    /// The user is an intentional person in the people catalog. Only
    /// consulted when a bound handle differs from the user.
    user_is_known_person: bool,
    aliases: Vec<String>,
    role: Option<String>,
    env_vendor: Option<String>,
    /// A selected session row failed verification: no private inbox.
    unverified: bool,
}

impl MailIdentityFacts {
    /// Combine this process's session binding, the store's people catalog
    /// (consulted only when a bound handle differs from the user) and its
    /// environment.
    pub fn observed(
        binding: SessionBinding,
        catalog: &PeopleCatalogView,
        env: IdentityEnv,
    ) -> Self {
        let (own_handle, own_vendor, own_recipient, unverified) = match binding {
            SessionBinding::Bound(b) => (Some(b.handle), Some(b.vendor), b.recipient, false),
            SessionBinding::Unbound => (None, None, None, false),
            SessionBinding::Unverified(_) => (None, None, None, true),
        };
        let user_is_known_person = (unverified
            || own_handle
                .as_deref()
                .is_some_and(|h| !h.eq_ignore_ascii_case(&env.user)))
            && catalog.contains(&env.user);
        Self {
            own_handle,
            own_recipient,
            own_vendor,
            user: env.user,
            user_is_known_person,
            aliases: if unverified { Vec::new() } else { env.aliases },
            role: env.role,
            env_vendor: env.vendor,
            unverified,
        }
    }

    /// The facts a mail-wake watcher reads for the session the registry row
    /// at `row` records: that row's own handle (and reservation UUID) plus
    /// its recorded role, never this process's environment, user or vendor
    /// group. Accepted only when the row is armed (`mail_wake = "armed"`),
    /// has not ended, carries a valid handle, and its process is still the
    /// one it recorded (start identity strictly matching), so arming a row
    /// is the only authority and a dead or recycled pid never reads.
    // trace:STORY-HOME-096 | ai:claude
    pub fn watched(row: &Path) -> Result<Self, BindingRefusal> {
        Self::watched_with(row, crate::liveness::process_identity_matches_strict)
    }

    /// [`Self::watched`] with an explicit live-identity matcher.
    pub(crate) fn watched_with(
        row: &Path,
        live: impl Fn(u32, &str) -> bool,
    ) -> Result<Self, BindingRefusal> {
        let body = read_bounded_regular(row).ok_or(BindingRefusal::Unreadable)?;
        let parsed: RegistryRow = toml::from_str(&body).map_err(|_| BindingRefusal::Corrupt)?;
        if row.file_stem().and_then(|s| s.to_str()) != Some(parsed.id.as_str()) {
            return Err(BindingRefusal::Corrupt);
        }
        if parsed.ended_at.is_some() {
            return Err(BindingRefusal::Ended);
        }
        let handle = parsed
            .name
            .clone()
            .filter(|n| valid_handle(n))
            .ok_or(BindingRefusal::NoHandle)?;
        if parsed.mail_wake.as_deref() != Some("armed") {
            return Err(BindingRefusal::NotArmed);
        }
        if !parsed
            .pid_start
            .as_deref()
            .is_some_and(|start| live(parsed.pid, start))
        {
            return Err(BindingRefusal::NotLive);
        }
        let sessions: Vec<&str> = [&parsed.claude_session_id, &parsed.native_session_id]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        let own_recipient = row
            .parent()
            .and_then(|agents| reservation_recipient(agents, &handle, &parsed.id, &sessions));
        Ok(Self {
            own_handle: Some(handle.clone()),
            own_recipient,
            own_vendor: None,
            user: handle,
            user_is_known_person: false,
            aliases: Vec::new(),
            role: parsed.role.filter(|r| !r.trim().is_empty()),
            env_vendor: None,
            unverified: false,
        })
    }

    /// Whether a session row was found that could not be verified.
    pub fn binding_unverified(&self) -> bool {
        self.unverified
    }

    /// The bound session's handle, when there is one.
    pub fn own_handle(&self) -> Option<&str> {
        self.own_handle.as_deref()
    }

    /// The bound session's recipient UUID, when its reservation names it.
    pub fn own_recipient(&self) -> Option<&RecipientUuid> {
        self.own_recipient.as_ref()
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    /// The raw session role.
    pub fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    /// Test-only: these facts with a recipient UUID.
    #[cfg(test)]
    pub(crate) fn with_recipient(mut self, recipient: RecipientUuid) -> Self {
        self.own_recipient = Some(recipient);
        self
    }

    /// Test-only facts.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn fixture(
        own_handle: Option<&str>,
        own_vendor: Option<&str>,
        user: &str,
        user_is_known_person: bool,
        aliases: &[&str],
        role: Option<&str>,
        env_vendor: Option<&str>,
    ) -> Self {
        Self {
            own_handle: own_handle.map(str::to_string),
            own_recipient: None,
            own_vendor: own_vendor.map(str::to_string),
            user: user.to_string(),
            user_is_known_person,
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            role: role.map(str::to_string),
            env_vendor: env_vendor.map(str::to_string),
            unverified: false,
        }
    }
}

/// How the policy classes one inbox identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityClass {
    /// This session's own handle, launch names, or an unbound user.
    Private,
    /// An intentional person from the people catalog.
    Human,
    /// A role or agent-type audience several sessions share.
    Shared,
}

/// The inbox identities a session reads, in order, each classed. Deduped.
///
/// The persisted handle wins for private mail: once a session has its own
/// handle, an inherited `AIDA_USER` adds no private inbox (only an
/// intentional catalog person stays, as a human inbox), and a launch-name
/// alias other than the handle is ignored. The role (`dialog` → `advisor`)
/// and the agent type (the binding's vendor before an inherited
/// `AIDA_AGENT_TYPE`) are shared audiences. This changes only mail
/// visibility, never the principal used for grants or queue ownership.
pub fn classified_identities(facts: &MailIdentityFacts) -> Vec<(String, IdentityClass)> {
    let own_handle = facts.own_handle.as_deref();
    let user = &facts.user;
    let mut ids: Vec<(String, IdentityClass)> = Vec::new();
    let mut push = |ids: &mut Vec<(String, IdentityClass)>, id: String, class| {
        if !ids.iter().any(|(i, _)| i.eq_ignore_ascii_case(&id)) {
            ids.push((id, class));
        }
    };
    if facts.unverified {
        if facts.user_is_known_person {
            push(&mut ids, user.clone(), IdentityClass::Human);
        }
    } else if own_handle.is_none_or(|own| own.eq_ignore_ascii_case(user)) {
        push(&mut ids, user.clone(), IdentityClass::Private);
    } else if facts.user_is_known_person {
        push(&mut ids, user.clone(), IdentityClass::Human);
    }
    for alias in &facts.aliases {
        if own_handle.is_some_and(|own| own != alias) {
            continue;
        }
        push(&mut ids, alias.clone(), IdentityClass::Private);
    }
    if let Some(raw) = &facts.role {
        let (role, _default) = effective_role(Some(raw));
        push(&mut ids, role, IdentityClass::Shared);
    }
    if let Some(raw) = facts
        .own_vendor
        .clone()
        .or_else(|| facts.env_vendor.clone())
        .filter(|s| !s.trim().is_empty())
    {
        push(&mut ids, normalize_agent_type(&raw), IdentityClass::Shared);
    }
    if let Some(handle) = own_handle {
        push(&mut ids, handle.to_string(), IdentityClass::Private);
    }
    ids
}

/// [`classified_identities`] without the classes.
pub fn inbox_identities(facts: &MailIdentityFacts) -> Vec<String> {
    classified_identities(facts)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

#[cfg(test)]
#[path = "mailbox_identity_tests.rs"]
mod tests;

// trace:BUG-b6b1278890774ef993d2ebc327862470-178 | ai:codex
/// Verified third-party inspection facts. Not an identity, audience, or ACK
/// capability; only the read-only mailbox projection accepts this type.
#[derive(Debug)]
pub struct InspectionSubject {
    pub(crate) handle: String,
    pub(crate) role: Vec<String>,
    pub(crate) recipient: RecipientUuid,
}

impl InspectionSubject {
    /// Observe an existing registry row and its reservation, without arming,
    /// binding, granting, or assuming the process identity of its reader.
    pub fn observe(row: &Path) -> Result<Self, BindingRefusal> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        // Pin the registry and reservation directories, refuse links and unsafe
        // ownership/modes, then read bounded no-follow children through handles.
        let pin = |path: &Path| -> Result<std::fs::File, BindingRefusal> {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)
                .map_err(|_| BindingRefusal::Unreadable)?;
            let meta = file.metadata().map_err(|_| BindingRefusal::Unreadable)?;
            if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
                return Err(BindingRefusal::Unreadable);
            }
            Ok(file)
        };
        let parent = row.parent().ok_or(BindingRefusal::Unreadable)?;
        let registry = pin(parent)?;
        let registry_path =
            std::path::PathBuf::from(format!("/proc/self/fd/{}", registry.as_raw_fd()));
        let pinned_row = registry_path.join(row.file_name().ok_or(BindingRefusal::Unreadable)?);
        let body = read_regular(&pinned_row, true).ok_or(BindingRefusal::Unreadable)?;
        let parsed: RegistryRow = toml::from_str(&body).map_err(|_| BindingRefusal::Corrupt)?;
        if row.file_stem().and_then(|s| s.to_str()) != Some(parsed.id.as_str()) {
            return Err(BindingRefusal::Corrupt);
        }
        if parsed.ended_at.is_some() {
            return Err(BindingRefusal::Ended);
        }
        let handle = parsed
            .name
            .filter(|n| valid_handle(n))
            .ok_or(BindingRefusal::NoHandle)?;
        if !parsed.pid_start.as_deref().is_some_and(|start| {
            crate::liveness::process_identity_matches_strict(parsed.pid, start)
        }) {
            return Err(BindingRefusal::NotLive);
        }
        let sessions: Vec<&str> = [&parsed.claude_session_id, &parsed.native_session_id]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        let handles = pin(&registry_path.join("handles"))?;
        let reservation_path =
            std::path::PathBuf::from(format!("/proc/self/fd/{}", handles.as_raw_fd()))
                .join(format!("{handle}.toml"));
        let reservation =
            read_regular(&reservation_path, true).ok_or(BindingRefusal::Unreadable)?;
        let recipient = reservation_from_body(&reservation, &handle, &parsed.id, &sessions)
            .ok_or(BindingRefusal::Unreadable)?;
        // A replaced directory is an unknown observation, not fresh evidence.
        for (path, pinned) in [
            (parent.to_path_buf(), &registry),
            (registry_path.join("handles"), &handles),
        ] {
            let again = pin(&path)?;
            let a = again.metadata().map_err(|_| BindingRefusal::Unreadable)?;
            let b = pinned.metadata().map_err(|_| BindingRefusal::Unreadable)?;
            if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
                return Err(BindingRefusal::Unreadable);
            }
        }
        let role = parsed
            .role
            .filter(|r| !r.trim().is_empty())
            .map(|r| effective_role(Some(&r)).0)
            .into_iter()
            .collect();
        Ok(Self {
            handle,
            role,
            recipient,
        })
    }
}
