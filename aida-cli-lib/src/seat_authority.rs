//! Session-bound role grants. `AIDA_SESSION_ROLE` is retained for display and
//! routing only; authorization comes from a validated local grant record.
// trace:STORY-1473 | ai:codex
// mandate-gate

use aida_core::store_locate::{MissedStore, StoreMiss, StoreOverride};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub(crate) const GRANT_ENV: &str = "AIDA_SESSION_GRANT";
/// A grant-file handle a launcher may carry. Named here, beside
/// [`GRANT_ENV`], so the grant env names live in this module alone; the
/// standalone implementer's child scrub reads it.
// trace:STORY-1662 | ai:claude
pub(crate) const GRANT_FILE_ENV: &str = "AIDA_SESSION_GRANT_FILE";
const GRANT_DIR: &str = "session-grants";
const MAX_GRANT_AGE: Duration = Duration::hours(24);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SeatGrant {
    pub id: String,
    /// Person whose roster ceiling authorized this grant.
    pub principal: String,
    /// Queue/process identity bound to this particular session.
    pub subject: String,
    pub session_id: String,
    pub seat: String,
    pub tty_issued_at: DateTime<Utc>,
    pub delegable_seats: Vec<String>,
    pub parent_grant_id: Option<String>,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// BUG-1934: the processes this grant is bound to, as strict
    /// `proc:`/`sid:` `<pid>@<start>` tokens. Only a presenter that runs under
    /// one of them (an ancestor, itself, or its POSIX session leader) may use
    /// the grant. Empty on a record minted before binding, which is refused.
    /// (`session_id` above is the old, never-checked binding; kept so older
    /// records still parse.) SKETCH v2: a `key:<sha256>` token binds the
    /// grant to whoever possesses its secret in a session keyring
    /// ([`crate::grant_keyring`]), which survives a sandbox's PID namespace.
    // trace:BUG-1934 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binding: Vec<String>,
}

pub(crate) fn grant_dir() -> Result<PathBuf> {
    // BUG-1642 discipline: never resolve the home directly — `crate::aida_home_dir`
    // honours `$AIDA_HOME` / `$HOME` overrides and is fenced to the hermetic
    // temp home under `cfg(test)`. trace:STORY-1473 | ai:claude
    let home = crate::aida_home_dir().context("cannot locate the user's home directory")?;
    Ok(home.join(".aida").join(GRANT_DIR))
}

fn grant_path(id: &str) -> Result<PathBuf> {
    if Uuid::parse_str(id).is_err() {
        bail!("invalid session grant handle");
    }
    Ok(grant_dir()?.join(format!("{id}.json")))
}

pub(crate) fn read_grant(id: &str) -> Result<SeatGrant> {
    let path = grant_path(id)?;
    let bytes = std::fs::read(&path).context("session grant is missing or unreadable")?;
    let grant: SeatGrant = serde_json::from_slice(&bytes).context("session grant is malformed")?;
    if grant.id != id {
        bail!("session grant handle does not match its record");
    }
    Ok(grant)
}

fn write_grant(grant: &SeatGrant) -> Result<()> {
    let dir = grant_dir()?;
    std::fs::create_dir_all(&dir)?;
    // Derive the final path from the SAME resolved dir as the staging file:
    // `grant_path` re-resolves the home from the environment, and under the
    // test harness a sibling test may legitimately swap `$AIDA_HOME`/`$HOME`
    // between the two reads — the rename target would then sit in a directory
    // this call never created. trace:STORY-1473 | ai:claude
    if Uuid::parse_str(&grant.id).is_err() {
        bail!("invalid session grant handle");
    }
    let path = dir.join(format!("{}.json", grant.id));
    let bytes = serde_json::to_vec_pretty(grant)?;
    // A per-write staging name: two writers of one grant (a launch stamp and
    // a revoke) must not collide on `create_new`. trace:BUG-1934 | ai:claude
    let tmp = dir.join(format!(".{}.{}.tmp", grant.id, Uuid::new_v4()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?
            .write_all(&bytes)?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// BUG-1934: read-modify-write grant `id` under an exclusive lock on
/// `session-grant-locks/<id>.lock` (beside, never inside, the grant store,
/// whose readers parse every file as a grant), then re-read and confirm `applied` holds, retrying
/// when a concurrent writer replaced the record between our write and the
/// re-read. Every rewrite of an existing record (pid stamps, revocation) goes
/// through here, so a stamp cannot drop a revocation or another stamp.
/// `change` returns `false` when there is nothing to write.
// trace:BUG-1934 | ai:claude
fn update_grant(
    id: &str,
    mut change: impl FnMut(&mut SeatGrant) -> bool,
    applied: impl Fn(&SeatGrant) -> bool,
) -> Result<bool> {
    use fs2::FileExt;
    const ATTEMPTS: usize = 5;
    let dir = grant_dir()?;
    if Uuid::parse_str(id).is_err() {
        bail!("invalid session grant handle");
    }
    let locks = grant_lock_dir()?;
    std::fs::create_dir_all(&locks)?;
    for _ in 0..ATTEMPTS {
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(locks.join(format!("{id}.lock")))
            .context("cannot open the session grant lock")?;
        lock.lock_exclusive()
            .context("cannot lock the session grant record")?;
        let mut grant = read_grant(id)?;
        let changed = change(&mut grant);
        if changed {
            write_grant(&grant)?;
        }
        drop(lock);
        if !changed {
            return Ok(false);
        }
        if read_grant(id).is_ok_and(|now| applied(&now)) {
            return Ok(true);
        }
    }
    bail!("session grant {id} kept changing under concurrent writers; the update was not applied")
}

/// BUG-1934: `session-grant-locks/`, beside the grant store.
// trace:BUG-1934 | ai:claude
fn grant_lock_dir() -> Result<PathBuf> {
    Ok(grant_dir()?
        .parent()
        .context("session grant store has no parent directory")?
        .join("session-grant-locks"))
}

/// BUG-1934 (round 2): remove the lock file of every grant that can no
/// longer be rewritten usefully: revoked, expired, or gone. Best effort; a
/// lock still held by a late writer only costs that writer a retry.
// trace:BUG-1934 | ai:claude
pub(crate) fn sweep_grant_locks(now: DateTime<Utc>) {
    let Ok(entries) = grant_lock_dir().and_then(|d| Ok(std::fs::read_dir(d)?)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(id) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".lock"))
        else {
            continue;
        };
        let stale = match read_grant(id) {
            Ok(grant) => grant.revoked_at.is_some() || grant.expires_at <= now,
            Err(_) => true,
        };
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Why the seat check could not reach a team roster. Distinct from a roster
/// that was read and does not list the seat, so a refusal never blames the
/// roster (or suggests `aida team allow-seat`) when it was never consulted.
// trace:BUG-2200 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RosterStoreError {
    /// No `.aida/config.toml` at or above `probed` names an existing store;
    /// `cause` is the resolver's own account of why.
    // trace:TASK-2072, TASK-2055 | ai:claude
    NotDetected { probed: PathBuf, cause: StoreMiss },
    /// The store resolved but holds no `registry/team.toml`.
    NoRoster { store: PathBuf },
}

impl std::fmt::Display for RosterStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotDetected { probed, cause } => {
                write!(f, "no AIDA store was found from `{}`: ", probed.display())?;
                match cause {
                    StoreMiss::NoConfig => {
                        write!(f, "no `.aida/config.toml` exists at or above it")?
                    }
                    StoreMiss::TempRoot { root } => write!(
                        f,
                        "the search stopped at the system temp directory `{}` before finding a `.aida/config.toml`",
                        root.display()
                    )?,
                    // trace:TASK-2169 | ai:claude
                    StoreMiss::StoreMissing {
                        config,
                        stores,
                        main_worktree,
                    } => {
                        write!(
                            f,
                            "`{}` names the store directory {}",
                            config.display(),
                            missed_stores(stores)
                        )?;
                        if !main_worktree.is_empty() {
                            write!(
                                f,
                                "; the main worktree's {} was tried too",
                                missed_stores(main_worktree)
                            )?;
                        }
                    }
                    StoreMiss::NoStorePath { config } => write!(
                        f,
                        "`{}` names no store directory (`store_path`)",
                        config.display()
                    )?,
                }
                // trace:TASK-2170 | ai:claude — a set `AIDA_STORE` is named
                // by the refusal note, never read from the environment here.
                Ok(())
            }
            Self::NoRoster { store } => write!(
                f,
                "the AIDA store at `{}` has no team roster (`{}` is missing); an interactive human can create it with `aida team allow-seat`",
                store.display(),
                roster_path(store).display()
            ),
        }
    }
}

/// Each missed store path with why it did not resolve, joined by "or".
// trace:TASK-2169 | ai:claude
fn missed_stores(stores: &[MissedStore]) -> String {
    stores
        .iter()
        .map(|store| {
            let why = if store.exists {
                "exists, but is not a directory"
            } else {
                "does not exist"
            };
            format!("`{}` ({why})", store.path.display())
        })
        .collect::<Vec<_>>()
        .join(" or ")
}

fn roster_path(store: &Path) -> PathBuf {
    store.join("registry").join("team.toml")
}

/// The store whose `registry/team.toml` is the seat authority for
/// `project_root`. Resolves through the same cwd walk-up
/// (`store_locate::locate_distributed_store_from`) the command dispatcher
/// uses to hand `aida team` its store, so the two always read one roster, and
/// a miss carries that walk's own account of why. The roster file itself is
/// the authority: a fresh solo store with no `objects/` yet is still a valid
/// roster source. `AIDA_STORE` is deliberately not honored here — an
/// environment variable must not be able to redirect the seat authority to
/// another roster.
// trace:BUG-2200 | ai:claude
// trace:TASK-2072, TASK-2055 | ai:claude
pub(crate) fn roster_store(project_root: &Path) -> std::result::Result<PathBuf, RosterStoreError> {
    let store =
        aida_core::store_locate::locate_distributed_store_from(project_root).map_err(|cause| {
            RosterStoreError::NotDetected {
                probed: project_root.to_path_buf(),
                cause,
            }
        })?;
    if !roster_path(&store).is_file() {
        return Err(RosterStoreError::NoRoster { store });
    }
    Ok(store)
}

/// Fail-closed: an unreachable store or missing roster never allows a seat.
pub(crate) fn roster_allows(project_root: &Path, principal: &str, seat: &str) -> bool {
    roster_store(project_root).is_ok_and(|store| store_allows(&store, principal, seat))
}

/// Does the roster in an already-resolved `store` list `seat` for `principal`?
// trace:TASK-2072, TASK-1667 | ai:claude
fn store_allows(store: &Path, principal: &str, seat: &str) -> bool {
    aida_core::team::TeamRoster::load(store)
        .seats_for(principal)
        .is_some_and(|seats| {
            seats
                .iter()
                .any(|allowed| aida_core::team::canonical_role(allowed) == seat)
        })
}

/// Why [`roster_check`] refused a seat. `note`, when set, names an
/// `AIDA_STORE` that resolves to another store than the one checked: `aida
/// team` reads that store's roster instead, so the two can disagree.
// trace:TASK-2072, TASK-1667, TASK-2054 | ai:claude
#[derive(Debug)]
pub(crate) enum RosterRefusal {
    /// No roster could be read at all.
    Unreachable {
        why: RosterStoreError,
        note: Option<String>,
    },
    /// The roster at `roster` was read and does not list the seat.
    Denied {
        roster: PathBuf,
        note: Option<String>,
    },
}

impl RosterRefusal {
    /// The refusal for a caller whose own wording of the denial is `denied`
    /// (unused for an unreachable roster, which explains itself).
    pub(crate) fn describe(&self, denied: &str) -> String {
        let (mut msg, note) = match self {
            Self::Unreachable { why, note } => (why.to_string(), note),
            Self::Denied { roster, note } => (
                format!("{denied} by the team roster (`{}`)", roster.display()),
                note,
            ),
        };
        if let Some(note) = note {
            msg.push_str(&format!(" ({note})"));
        }
        msg
    }
}

/// The one roster check behind seat issuance and child delegation: resolve
/// the roster store once, then read that store's roster.
// trace:TASK-2072, TASK-1667 | ai:claude
pub(crate) fn roster_check(
    project_root: &Path,
    principal: &str,
    seat: &str,
) -> std::result::Result<PathBuf, RosterRefusal> {
    // trace:TASK-2171 | ai:claude — passed unresolved: only a refusal reads it.
    roster_check_with_override(
        project_root,
        principal,
        seat,
        crate::store_sync::aida_store_setting,
    )
}

/// [`roster_check`] with the `AIDA_STORE` lookup (as
/// `store_sync::aida_store_setting` resolves it) passed in, so a test never
/// mutates the process environment. The lookup runs only on a refusal, so an
/// allowed seat never resolves `AIDA_STORE`.
// trace:TASK-2072, TASK-2054, TASK-1666 | ai:claude
// trace:TASK-2170, TASK-2171 | ai:claude
pub(crate) fn roster_check_with_override(
    project_root: &Path,
    principal: &str,
    seat: &str,
    aida_store: impl FnOnce() -> Option<(String, StoreOverride)>,
) -> std::result::Result<PathBuf, RosterRefusal> {
    let store = match roster_store(project_root) {
        Ok(store) => store,
        Err(why) => {
            let note = match &why {
                RosterStoreError::NoRoster { store } => divergence(store, aida_store),
                RosterStoreError::NotDetected { .. } => aida_store().map(|(raw, setting)| {
                    match setting {
                        StoreOverride::Usable(other) => diverging_note(&other),
                        StoreOverride::Unusable { reason } => format!(
                            "AIDA_STORE is `{raw}`, which is unusable ({reason}); seat checks read the roster from the project's own store, never from an environment override"
                        ),
                    }
                }),
            };
            return Err(RosterRefusal::Unreachable { why, note });
        }
    };
    if store_allows(&store, principal, seat) {
        return Ok(store);
    }
    Err(RosterRefusal::Denied {
        roster: roster_path(&store),
        note: divergence(&store, aida_store),
    })
}

/// The note for a usable `AIDA_STORE` that resolves to another store than
/// `store`; an unusable one falls back to the project's store, as `aida team`
/// does too, so it diverges from nothing.
// trace:TASK-2072, TASK-2054 | ai:claude
fn divergence(
    store: &Path,
    aida_store: impl FnOnce() -> Option<(String, StoreOverride)>,
) -> Option<String> {
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    match aida_store()? {
        (_, StoreOverride::Usable(other)) if canonical(store) != canonical(&other) => {
            Some(diverging_note(&other))
        }
        _ => None,
    }
}

fn diverging_note(other: &Path) -> String {
    format!(
        "AIDA_STORE points at `{}`, so `aida team` reads that store's roster; seat checks always read the project's own store",
        other.display()
    )
}

/// Why an exported grant failed validation. The checks and their order are
/// exactly those [`current_grant`] has always applied; this only names which
/// one failed so a refusal can say "expired" instead of "no active role".
// trace:BUG-1983 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GrantRejection {
    /// The exported handle is not a grant id.
    BadHandle,
    /// No readable record for the handle.
    Unreadable,
    /// The record does not parse, or names a different id.
    Malformed,
    Revoked {
        at: DateTime<Utc>,
    },
    Expired {
        at: DateTime<Utc>,
    },
    /// The grant was minted for another identity than this process runs as.
    SubjectMismatch {
        grant_subject: String,
        caller: String,
    },
    /// The team roster no longer allows the principal this seat.
    RosterDenied {
        principal: String,
        seat: String,
    },
    /// The parent grant failed one of the checks above.
    Parent(Box<GrantRejection>),
    /// The parent belongs to another principal than the child.
    ParentPrincipalMismatch,
    /// The parent grant does not delegate the child's seat.
    ParentNotDelegating {
        parent_seat: String,
    },
    /// BUG-1934: the record carries no process binding (minted before
    /// grants were bound), so no presenter can be shown to own it.
    Unbound,
    /// BUG-1934: the grant is bound to other processes than the ones this
    /// process runs under. `bound_to` names them by pid.
    PresenterNotBound {
        bound_to: String,
    },
    /// BUG-1934 (SKETCH v2): this process runs in another PID namespace (an
    /// agent sandbox) than the one the grant's processes live in, and it does
    /// not hold the grant's session key.
    ForeignNamespace {
        bound_to: String,
    },
    /// BUG-1934 (SKETCH v2): the kernel refused the session keyring, so key
    /// possession cannot be shown, and no bound process is an ancestor. Never
    /// a reason to allow.
    KeyringUnavailable {
        bound_to: String,
    },
}

impl std::fmt::Display for GrantRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stamp = |at: &DateTime<Utc>| at.format("%Y-%m-%dT%H:%MZ").to_string();
        match self {
            Self::BadHandle => write!(f, "the exported handle is not a grant id"),
            Self::Unreadable => write!(f, "no grant record exists for it on this machine"),
            Self::Malformed => write!(f, "its record is malformed"),
            Self::Revoked { at } => write!(f, "it was revoked at {}", stamp(at)),
            Self::Expired { at } => write!(f, "it expired at {}", stamp(at)),
            Self::SubjectMismatch {
                grant_subject,
                caller,
            } => write!(
                f,
                "it was issued to `{grant_subject}` but this process runs as `{caller}`"
            ),
            Self::RosterDenied { principal, seat } => write!(
                f,
                "the team roster no longer allows `{principal}` the `{seat}` seat"
            ),
            Self::Parent(inner) => write!(f, "the grant it was delegated from is invalid: {inner}"),
            Self::ParentPrincipalMismatch => {
                write!(
                    f,
                    "the grant it was delegated from belongs to another principal"
                )
            }
            Self::ParentNotDelegating { parent_seat } => write!(
                f,
                "the `{parent_seat}` grant it was delegated from no longer delegates this seat"
            ),
            // trace:BUG-1934 | ai:claude
            Self::Unbound => write!(
                f,
                "it was issued before grants were bound to the process that holds them, so no process may present it"
            ),
            Self::PresenterNotBound { bound_to } => write!(
                f,
                "it is bound to {bound_to}, and this process does not run under it"
            ),
            Self::ForeignNamespace { bound_to } => write!(
                f,
                "this command runs in a sandboxed process namespace and does not hold the grant's session key (it is bound to {bound_to})"
            ),
            Self::KeyringUnavailable { bound_to } => write!(
                f,
                "the session keyring is unavailable to this process, so it cannot show it holds the grant's key, and it does not run under {bound_to}"
            ),
        }
    }
}

/// The verdict on the grant this process exports through `AIDA_SESSION_GRANT`.
// trace:BUG-1983 | ai:claude
#[derive(Debug, Clone)]
pub(crate) enum GrantCheck {
    /// No grant is exported.
    Absent,
    Valid(SeatGrant),
    /// A grant is exported but fails validation. `seat` is known when the
    /// record could be read.
    Rejected {
        id: String,
        seat: Option<String>,
        reason: GrantRejection,
    },
}

impl GrantCheck {
    pub(crate) fn into_valid(self) -> Option<SeatGrant> {
        match self {
            Self::Valid(grant) => Some(grant),
            _ => None,
        }
    }

    /// One sentence for a refusal, naming the grant by an 8-character prefix
    /// only (never the full handle) and the failed check. `None` unless the
    /// grant was rejected.
    // trace:BUG-1983 | ai:claude
    pub(crate) fn rejection_sentence(&self) -> Option<String> {
        let Self::Rejected { id, seat, reason } = self else {
            return None;
        };
        let prefix: String = id.chars().take(8).collect();
        let seat_label = seat.as_deref().unwrap_or("unknown seat");
        let recovery = match (seat.as_deref(), reason) {
            // BUG-1934 C1 (v2 C6): a binding refusal names how to re-enter
            // for a foreground session and for a background or relay one,
            // and what --bind-pid can still serve. trace:BUG-1934 | ai:claude
            (
                Some(seat),
                GrantRejection::Unbound
                | GrantRejection::PresenterNotBound { .. }
                | GrantRejection::ForeignNamespace { .. }
                | GrantRejection::KeyringUnavailable { .. },
            ) => rebind_recovery(seat),
            (Some(seat), _) => format!("Re-enter it at a terminal: `aida role enter {seat}`"),
            (None, _) => "Re-enter your role at a terminal: `aida role enter <role>`".to_string(),
        };
        Some(format!(
            "Session grant {prefix}… ({seat_label}) was rejected: {reason}. {recovery}"
        ))
    }
}

/// BUG-1934 C1 / v2 C6: how to re-establish a seat after a binding refusal.
// trace:BUG-1934 | ai:claude
pub(crate) fn rebind_recovery(seat: &str) -> String {
    format!(
        "Re-enter it at a terminal with `eval \"$(aida role enter {seat})\"`, then start this session from that same terminal: a foreground agent directly, a background or relay agent by launching it there (`aida agent new` without --bg, or `nohup <agent> &`). A session the agent daemon (`--bg`) or a tmux server starts does not descend from that terminal and never holds the grant's key. `aida role enter {seat} --bind-pid <harness pid>` binds an already-running session, but only for its unsandboxed hooks and MCP server, not its sandboxed tool calls"
    )
}

fn check_lifetime(grant: &SeatGrant, now: DateTime<Utc>) -> Result<(), GrantRejection> {
    if let Some(at) = grant.revoked_at {
        return Err(GrantRejection::Revoked { at });
    }
    if now >= grant.expires_at {
        return Err(GrantRejection::Expired {
            at: grant.expires_at,
        });
    }
    Ok(())
}

fn check_roster(project_root: &Path, grant: &SeatGrant) -> Result<(), GrantRejection> {
    if roster_allows(project_root, &grant.principal, &grant.seat) {
        Ok(())
    } else {
        Err(GrantRejection::RosterDenied {
            principal: grant.principal.clone(),
            seat: grant.seat.clone(),
        })
    }
}

fn read_grant_checked(id: &str) -> Result<SeatGrant, GrantRejection> {
    if Uuid::parse_str(id).is_err() {
        return Err(GrantRejection::BadHandle);
    }
    let path = grant_path(id).map_err(|_| GrantRejection::BadHandle)?;
    let bytes = std::fs::read(&path).map_err(|_| GrantRejection::Unreadable)?;
    let grant: SeatGrant = serde_json::from_slice(&bytes).map_err(|_| GrantRejection::Malformed)?;
    if grant.id != id {
        return Err(GrantRejection::Malformed);
    }
    Ok(grant)
}

// trace:BUG-1983 | ai:claude
pub(crate) fn grant_check(project_root: &Path) -> GrantCheck {
    grant_check_at(project_root, Utc::now())
}

/// Register the CLI's commit-authority resolver with `aida_core::lifecycle`.
// trace:TASK-1737 | ai:antigravity
pub(crate) fn ensure_commit_authority_resolver_registered() {
    aida_core::lifecycle::register_commit_authority_resolver(resolve_commit_authority);
}

/// Resolve the project root that owns a `store_root` (`<project_root>/.aida-store`)
/// without changing the shared `roster_store()` helper or reading process cwd.
// trace:TASK-1737 | ai:antigravity
fn project_root_for_store_root(store_root: &Path) -> PathBuf {
    store_root
        .parent()
        .filter(|parent| crate::detect_distributed_store_from(parent).is_some())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| store_root.to_path_buf())
}

/// Resolve the active authority (`Aida-Authority: grant|orchestrator`) for a
/// store commit rooted at `store_root`, built on [`grant_check`] and
/// [`crate::orchestrator::detect`]. Emits trailers only when the advance is
/// authorized by a validated `advisor` seat grant or a live corroborated
/// orchestrator run; human-at-TTY advances without an advisor grant or with a
/// non-advisor grant emit no authority trailer.
// trace:TASK-1737 | ai:antigravity
pub(crate) fn resolve_commit_authority(
    store_root: &Path,
) -> Option<aida_core::lifecycle::CommitAuthority> {
    use aida_core::lifecycle::CommitAuthority;

    if crate::role::current_role_instance_is_companion() {
        return None;
    }

    let project_root = project_root_for_store_root(store_root);
    if let GrantCheck::Valid(grant) = grant_check(&project_root) {
        let seat = crate::canonical_role_name(&grant.seat);
        if seat == "advisor" {
            let grant_prefix: String = grant.id.chars().take(8).collect();
            return Some(CommitAuthority::Grant { seat, grant_prefix });
        }
    }

    let mut candidate_roots = vec![project_root];
    if candidate_roots[0] != store_root {
        candidate_roots.push(store_root.to_path_buf());
    }
    if candidate_roots.iter().any(|root| {
        matches!(
            crate::orchestrator::detect(root),
            crate::orchestrator::OrchestratorContext::Orchestrated
        )
    }) {
        return Some(CommitAuthority::Orchestrator);
    }

    None
}

// Shared clock seam; notices never implement a second authority validator.
// trace:BUG-2014 | ai:codex
pub(crate) fn grant_check_at(project_root: &Path, now: DateTime<Utc>) -> GrantCheck {
    let Some(id) = std::env::var(GRANT_ENV)
        .ok()
        .filter(|id| !id.trim().is_empty())
    else {
        return GrantCheck::Absent;
    };
    // BUG-1934: an env-presented grant must also be bound to this process.
    // trace:BUG-1934 | ai:claude
    with_presenter(
        grant_check_for_at(project_root, &id, &crate::current_user_id(None), now),
        &presenter_tokens(),
    )
}

/// BUG-1934 (SKETCH v2): a fresh grant secret: 32 random bytes, hex encoded
/// (the session-key payload). Held in memory and in the keyring only.
// trace:BUG-1934 | ai:claude
pub(crate) fn new_key_secret() -> String {
    let mut hex = String::with_capacity(64);
    for b in Uuid::new_v4()
        .as_bytes()
        .iter()
        .chain(Uuid::new_v4().as_bytes())
    {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// BUG-1934 (SKETCH v2): the `key:<sha256>` binding token for a secret.
// trace:BUG-1934 | ai:claude
pub(crate) fn key_token(secret: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(secret);
    let mut hex = String::with_capacity(64);
    for b in digest.iter() {
        hex.push_str(&format!("{b:02x}"));
    }
    format!("key:{hex}")
}

fn has_key_binding(grant: &SeatGrant) -> bool {
    grant.binding.iter().any(|b| b.starts_with("key:"))
}

/// BUG-1934 (SKETCH v2): the `key:` token this process can show for `grant`:
/// `Some` only when it possesses the grant's session key and the key's
/// digest is in the grant's binding.
// trace:BUG-1934 | ai:claude
pub(crate) fn possession_token(grant: &SeatGrant) -> Option<String> {
    if !has_key_binding(grant) {
        return None;
    }
    match crate::grant_keyring::read(&grant.id) {
        crate::grant_keyring::KeyRead::Held(secret) => {
            let token = key_token(&secret);
            grant.binding.contains(&token).then_some(token)
        }
        _ => None,
    }
}

/// BUG-1934: everything this process presents for `grant`: its process
/// tokens plus, when it holds the grant's session key, the `key:` token.
// trace:BUG-1934 | ai:claude
pub(crate) fn presenter_tokens_for(grant: &SeatGrant) -> Vec<String> {
    let mut out = presenter_tokens();
    out.extend(possession_token(grant));
    out
}

/// BUG-1934 (SKETCH v2): does this process run in a PID namespace other than
/// the host's (an agent sandbox, a container)? Its POSIX session leader is
/// then outside the namespace (`getsid` = 0), or its pid 1 is not the host
/// init. Only picks the refusal sentence; the check fails closed either way.
// trace:BUG-1934 | ai:claude
fn in_foreign_pid_namespace() -> bool {
    #[cfg(test)]
    if let Some(forced) = test_support::FOREIGN_NAMESPACE.with(|f| f.get()) {
        return forced;
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: getsid(0) only reads the calling process's session id.
        let sid = unsafe { libc::getsid(0) };
        let init = std::fs::read_to_string("/proc/1/comm").unwrap_or_default();
        foreign_namespace_from(sid, init.trim())
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Pure core of [`in_foreign_pid_namespace`].
// trace:BUG-1934 | ai:claude
pub(crate) fn foreign_namespace_from(sid: i32, init_comm: &str) -> bool {
    sid == 0 || !matches!(init_comm, "systemd" | "init" | "launchd")
}

/// BUG-1934: the identity tokens of THIS process as a grant presenter: its
/// whole ancestry plus its POSIX session leader.
// trace:BUG-1934 | ai:claude
pub(crate) fn presenter_tokens() -> Vec<String> {
    #[allow(unused_mut)]
    let mut out = presenter_source().presenter_tokens();
    // Under `cfg(test)` the test process presents every in-process check,
    // also beside an injected source. trace:BUG-1934 | ai:claude
    #[cfg(test)]
    if let Some(me) = own_process_token() {
        if !out.contains(&me) {
            out.push(me);
        }
    }
    out
}

/// The identity source behind [`presenter_tokens`] and direct issuance.
/// Under `cfg(test)` a source no test injected falls back to the REAL
/// ancestry: the test process (or a test binary re-exec'd as a child of it)
/// really is the presenter, and the grant fixtures are bound to it. An
/// injected source is used as is.
// trace:BUG-1934 | ai:claude
fn presenter_source() -> crate::review_authority::IdentitySource {
    #[allow(unused_mut)]
    let mut source = crate::review_authority::IdentitySource::live();
    #[cfg(test)]
    if source.chain.is_empty() {
        source.chain = crate::review_authority::live_ancestry();
        source.session_leader = crate::review_authority::live_session_leader();
    }
    source
}

/// `proc:<pid>@<start>` for this very process, when its start identity reads.
// trace:BUG-1934 | ai:claude
fn own_process_token() -> Option<String> {
    pid_binding_token(std::process::id())
}

/// `proc:<pid>@<start>` for live process `pid`; `None` when its start
/// identity cannot be read (a binding without one could never match).
// trace:BUG-1934 | ai:claude
fn pid_binding_token(pid: u32) -> Option<String> {
    let start = crate::process_probe::process_start_identity(pid)?;
    Some(crate::review_authority::process_token(
        "proc",
        pid,
        Some(&start),
    ))
}

/// BUG-1934: why `presenter` may not use `grant`, or `None` when it may.
// trace:BUG-1934 | ai:claude
pub(crate) fn presenter_rejection(
    grant: &SeatGrant,
    presenter: &[String],
) -> Option<GrantRejection> {
    if grant.binding.is_empty() {
        return Some(GrantRejection::Unbound);
    }
    if crate::review_authority::binding_overlaps(&grant.binding, presenter) {
        return None;
    }
    Some(GrantRejection::PresenterNotBound {
        bound_to: describe_binding(&grant.binding),
    })
}

/// BUG-1934: the presenter tokens that satisfy `grant`'s binding: what a
/// review attestation and the advisor turn stamp carry to ship time.
// trace:BUG-1934 | ai:claude
pub(crate) fn matched_binding(grant: &SeatGrant, presenter: &[String]) -> Vec<String> {
    presenter
        .iter()
        .filter(|p| {
            grant
                .binding
                .iter()
                .any(|b| crate::review_authority::binding_tokens_match(b, p))
        })
        .cloned()
        .collect()
}

/// `process 4242, terminal session 4200` for a refusal or status line.
// trace:BUG-1934 | ai:claude
pub(crate) fn describe_binding(binding: &[String]) -> String {
    let parts: Vec<String> = binding
        .iter()
        .filter_map(|t| {
            let (kind, value) = t.split_once(':')?;
            if kind == "key" {
                return None;
            }
            let pid = value.split('@').next().filter(|p| !p.is_empty())?;
            Some(match kind {
                "sid" => format!("terminal session {pid}"),
                _ => format!("process {pid}"),
            })
        })
        .chain(
            binding
                .iter()
                .any(|t| t.starts_with("key:"))
                .then(|| "the session key of the shell or launcher that holds it".to_string()),
        )
        .collect();
    if parts.is_empty() {
        "no process".to_string()
    } else {
        parts.join(", ")
    }
}

/// BUG-1934 v2: name a `PresenterNotBound` refusal more exactly: the
/// keyring was unavailable for a key-bound grant, or the presenter runs in a
/// foreign PID namespace. It maps a refusal to a refusal only (C5: an
/// unavailable keyring never fails open).
// trace:BUG-1934 | ai:claude
pub(crate) fn classify_refusal(
    reason: GrantRejection,
    key_bound: bool,
    keyring_available: bool,
    foreign_namespace: bool,
) -> GrantRejection {
    match reason {
        GrantRejection::PresenterNotBound { bound_to } if key_bound && !keyring_available => {
            GrantRejection::KeyringUnavailable { bound_to }
        }
        GrantRejection::PresenterNotBound { bound_to } if foreign_namespace => {
            GrantRejection::ForeignNamespace { bound_to }
        }
        other => other,
    }
}

/// Apply the presenter check to an otherwise-valid verdict. `presenter` is
/// this process's process tokens; its `key:` token, when it holds the
/// grant's session key, is added here. A `PresenterNotBound` is named more
/// exactly when the keyring is unavailable or the process is sandboxed.
// trace:BUG-1934 | ai:claude
fn with_presenter(check: GrantCheck, presenter: &[String]) -> GrantCheck {
    let GrantCheck::Valid(grant) = check else {
        return check;
    };
    let mut presenter = presenter.to_vec();
    presenter.extend(possession_token(&grant));
    let rejection = presenter_rejection(&grant, &presenter).map(|reason| {
        classify_refusal(
            reason,
            has_key_binding(&grant),
            crate::grant_keyring::available(),
            in_foreign_pid_namespace(),
        )
    });
    match rejection {
        None => GrantCheck::Valid(grant),
        Some(reason) => GrantCheck::Rejected {
            id: grant.id.clone(),
            seat: Some(grant.seat),
            reason,
        },
    }
}

/// Revalidate a recorded reviewer through the same grant verdict as record time.
// trace:STORY-1687 | ai:codex
pub(crate) fn grant_check_for(project_root: &Path, id: &str, caller: &str) -> GrantCheck {
    grant_check_for_at(project_root, id, caller, Utc::now())
}

// trace:STORY-1687 | ai:codex
// trace:BUG-2014 | ai:codex
fn grant_check_for_at(
    project_root: &Path,
    id: &str,
    caller: &str,
    now: DateTime<Utc>,
) -> GrantCheck {
    let rejected = |seat: Option<String>, reason| GrantCheck::Rejected {
        id: id.to_string(),
        seat,
        reason,
    };
    let grant = match read_grant_checked(id) {
        Ok(grant) => grant,
        Err(reason) => return rejected(None, reason),
    };
    // Checks run in the order `current_grant` always used: revoked, expired,
    // subject, roster; then the parent's revoked, expired, principal,
    // roster, delegable.
    let own = check_lifetime(&grant, now).and_then(|()| {
        if grant.subject == caller {
            check_roster(project_root, &grant)
        } else {
            Err(GrantRejection::SubjectMismatch {
                grant_subject: grant.subject.clone(),
                caller: caller.to_string(),
            })
        }
    });
    if let Err(reason) = own {
        return rejected(Some(grant.seat), reason);
    }
    if let Some(parent_id) = &grant.parent_grant_id {
        let parent = read_grant_checked(parent_id)
            .and_then(|parent| check_lifetime(&parent, now).map(|()| parent))
            .map_err(|reason| GrantRejection::Parent(Box::new(reason)))
            .and_then(|parent| {
                if parent.principal == grant.principal {
                    Ok(parent)
                } else {
                    Err(GrantRejection::ParentPrincipalMismatch)
                }
            })
            .and_then(|parent| {
                check_roster(project_root, &parent)
                    .map(|()| parent)
                    .map_err(|reason| GrantRejection::Parent(Box::new(reason)))
            });
        let parent = match parent {
            Ok(parent) => parent,
            Err(reason) => return rejected(Some(grant.seat), reason),
        };
        if !parent
            .delegable_seats
            .iter()
            .any(|seat| seat == &grant.seat)
        {
            return rejected(
                Some(grant.seat),
                GrantRejection::ParentNotDelegating {
                    parent_seat: parent.seat,
                },
            );
        }
    }
    GrantCheck::Valid(grant)
}

pub(crate) fn current_grant(project_root: &Path) -> Option<SeatGrant> {
    grant_check(project_root).into_valid()
}

// Shared local authority validation for current callers and persisted takeover
// requesters: the same verdict as `grant_check_for_at`, collapsed to Option.
// trace:TASK-1607 | ai:codex
pub(crate) fn validated_grant(
    project_root: &Path,
    id: &str,
    subject: &str,
    now: DateTime<Utc>,
) -> Option<SeatGrant> {
    grant_check_for_at(project_root, id, subject, now).into_valid()
}

pub(crate) fn current_seat(project_root: &Path) -> Option<String> {
    current_grant(project_root).map(|grant| grant.seat)
}

/// The raw grant handle this process exports, when it is set and non-blank,
/// UNVALIDATED. Only for callers that compare the handle or look its record up
/// themselves; every authority decision goes through [`grant_check`].
// trace:TASK-1752 | ai:claude
pub(crate) fn current_grant_id() -> Option<String> {
    std::env::var(GRANT_ENV)
        .ok()
        .filter(|id| !id.trim().is_empty())
}

/// What a display surface (the statusline) should show as the session's
/// role. The `AIDA_SESSION_ROLE` label is only a display/routing hint, so a
/// label naming a seat the session holds no valid grant for must read as
/// ungranted rather than as authority the session doesn't have.
// trace:BUG-1924 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RoleDisplay {
    /// The session holds this seat: a validated grant, or the `implementer`
    /// baseline, which needs no grant.
    Held(String),
    /// The label names this seat, but there is no valid grant for it
    /// (missing, expired, revoked, or minted in another shell).
    Ungranted(String),
    /// Neither a label nor a grant: the implicit `implementer` baseline.
    Default,
}

/// Pure classifier behind [`role_display`]: `grant_seat` is the seat of the
/// validated grant, if any.
// trace:BUG-1924 | ai:claude
pub(crate) fn classify_role_display(label: Option<&str>, grant_seat: Option<&str>) -> RoleDisplay {
    if let Some(seat) = grant_seat.map(str::trim).filter(|s| !s.is_empty()) {
        return RoleDisplay::Held(crate::canonical_role_name(seat));
    }
    match label.map(str::trim).filter(|s| !s.is_empty()) {
        None => RoleDisplay::Default,
        Some(label) => {
            let role = crate::canonical_role_name(label);
            // The implementer baseline and the lower-privilege stakeholder
            // personas confer nothing a grant would gate, so the label is
            // already the truth for them.
            if is_baseline_seat(&role) {
                RoleDisplay::Held(role)
            } else {
                RoleDisplay::Ungranted(role)
            }
        }
    }
}

/// Resolve the displayed role from the validated grant (local file reads
/// only — cheap enough for the per-prompt statusline), falling back to the
/// label marked ungranted.
// trace:BUG-1924 | ai:claude
pub(crate) fn role_display(project_root: &Path, label: Option<&str>) -> RoleDisplay {
    let seat = current_seat(project_root);
    classify_role_display(label, seat.as_deref())
}

/// A seat grant inside this window is warned about (SessionStart hook,
/// statusline), so expiry never surfaces first as a mid-run refusal.
// trace:TASK-1736 | ai:claude
pub(crate) const EXPIRY_WARN_WITHIN: Duration = Duration::hours(2);

/// When this session's seat grant stops conferring its seat.
// trace:TASK-1736 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GrantExpiry {
    pub seat: String,
    /// The earlier of the grant's and its parent's expiry: a child grant
    /// dies with its parent.
    pub expires_at: DateTime<Utc>,
}

/// What `AIDA_SESSION_GRANT` resolves to for expiry purposes.
// trace:TASK-1736 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GrantLookup {
    /// No grant handle in the environment.
    NoGrant,
    /// A grant for a seat the baseline confers anyway: its expiry costs nothing.
    Baseline,
    /// The handle is set but names no usable record (missing, malformed,
    /// expired, revoked, subject/roster rejected, or its parent is invalid).
    /// A launcher must not treat
    /// this as "nothing to worry about".
    Unresolved(String),
    /// A live record and when it stops conferring its seat.
    Expiry(GrantExpiry),
}

/// Classify the exported grant through the same verdict as authority gates.
/// Informational expired-record display is kept in `session_grant_expiry`.
// trace:TASK-1736 | ai:claude
pub(crate) fn session_grant_lookup() -> GrantLookup {
    // Use the same root resolver and verdict as the authority gates. A missing
    // project fails closed through the roster check, while Absent stays silent.
    // trace:BUG-2011 | ai:codex
    let root = crate::find_project_root().unwrap_or_else(|_| PathBuf::from("."));
    match grant_check(&root) {
        GrantCheck::Absent => GrantLookup::NoGrant,
        GrantCheck::Rejected { reason, .. } => {
            // Keep the existing informational revocation wording; the decision
            // and all other reasons come from the shared typed verdict.
            let reason = match reason {
                GrantRejection::Revoked { .. } => "session grant was revoked".to_string(),
                other @ GrantRejection::Unreadable => {
                    format!("{other} (grant record missing)")
                }
                other => other.to_string(),
            };
            GrantLookup::Unresolved(reason)
        }
        GrantCheck::Valid(grant) => {
            let seat = crate::canonical_role_name(&grant.seat);
            let mut expires_at = grant.expires_at;
            if let Some(parent_id) = grant.parent_grant_id.as_deref() {
                match read_grant(parent_id) {
                    Ok(parent) => expires_at = expires_at.min(parent.expires_at),
                    Err(error) => return GrantLookup::Unresolved(error.to_string()),
                }
            }
            GrantLookup::Expiry(GrantExpiry { seat, expires_at })
        }
    }
}

// Informational statusline metadata deliberately includes expired records.
// It conveys no authority and is never consumed by the launch-refusal gate.
// Keeping it separate preserves the existing expired-record warning contract.
// trace:BUG-2011 | ai:codex
fn display_grant_lookup() -> GrantLookup {
    let Some(id) = std::env::var(GRANT_ENV)
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
    else {
        return GrantLookup::NoGrant;
    };
    let grant = match read_grant(&id) {
        Ok(grant) => grant,
        Err(err) => return GrantLookup::Unresolved(err.to_string()),
    };
    if grant.revoked_at.is_some() {
        return GrantLookup::Unresolved("session grant was revoked".to_string());
    }
    let seat = crate::canonical_role_name(&grant.seat);
    if matches!(seat.as_str(), "implementer" | "guest" | "requester") {
        return GrantLookup::Baseline;
    }
    let mut expires_at = grant.expires_at;
    if let Some(parent_id) = grant.parent_grant_id.as_deref() {
        match read_grant(parent_id) {
            Ok(parent) if parent.revoked_at.is_some() => {
                return GrantLookup::Unresolved(
                    "the grant this session's grant was delegated from was revoked".to_string(),
                )
            }
            Ok(parent) => expires_at = expires_at.min(parent.expires_at),
            Err(err) => {
                return GrantLookup::Unresolved(format!(
                    "the grant this session's grant was delegated from: {err}"
                ))
            }
        }
    }
    GrantLookup::Expiry(GrantExpiry { seat, expires_at })
}

// Informational metadata only, never an authority-bearing grant.
// trace:BUG-2014 | ai:codex
pub(crate) struct NoticeGrant {
    pub id: String,
    pub expiry: GrantExpiry,
}

// Only Valid or typed own/parent Expired verdicts supply notice metadata.
// Expired verdicts contribute captured timestamps/seats. Own expiry uses a
// consistency-checked metadata read only to preserve a parent's earlier expiry.
// trace:BUG-2014 | ai:codex
pub(crate) fn notice_grant(root: &Path, now: DateTime<Utc>) -> Result<Option<NoticeGrant>> {
    notice_grant_from_check(grant_check_at(root, now), read_grant)
}

// Keep the captured verdict and subsequent metadata read independently testable.
// trace:BUG-2014 | ai:codex
fn notice_grant_from_check(
    check: GrantCheck,
    read: impl Fn(&str) -> Result<SeatGrant>,
) -> Result<Option<NoticeGrant>> {
    let (id, seat, expires_at) = match check {
        GrantCheck::Valid(grant) => {
            let mut at = grant.expires_at;
            if let Some(parent) = grant.parent_grant_id.as_deref() {
                at = at.min(read(parent)?.expires_at);
            }
            (grant.id, grant.seat, at)
        }
        GrantCheck::Rejected {
            id,
            seat: Some(seat),
            reason: GrantRejection::Expired { at },
        } => {
            // Own expiry wins the validator's rejection order. If both child
            // and parent expired, display their minimum, retaining only
            // consistent timestamp/seat metadata from the expired record.
            let record = read(&id)?;
            if record.expires_at != at || record.seat != seat {
                return Ok(None);
            }
            let effective = match record.parent_grant_id.as_deref() {
                Some(parent) => at.min(read(parent)?.expires_at),
                None => at,
            };
            (id, seat, effective)
        }
        GrantCheck::Rejected {
            id,
            seat: Some(seat),
            reason: GrantRejection::Parent(inner),
        } => match *inner {
            GrantRejection::Expired { at } => (id, seat, at),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    let seat = crate::canonical_role_name(&seat);
    if matches!(seat.as_str(), "implementer" | "guest" | "requester") {
        return Ok(None);
    }
    Ok(Some(NoticeGrant {
        id,
        expiry: GrantExpiry { seat, expires_at },
    }))
}

/// The expiry of the grant named by `AIDA_SESSION_GRANT`. `None` when there
/// is no grant, it is unresolvable or revoked, or its seat is one the
/// baseline confers anyway (its expiry costs nothing).
// trace:TASK-1736 | ai:claude
pub(crate) fn session_grant_expiry() -> Option<GrantExpiry> {
    match display_grant_lookup() {
        GrantLookup::Expiry(expiry) => Some(expiry),
        _ => None,
    }
}

/// `1h05m` / `42m` / `under 1m` for a positive span.
// trace:TASK-1736 | ai:claude
pub(crate) fn format_remaining(left: Duration) -> String {
    let mins = left.num_minutes();
    match mins {
        m if m < 1 => "under 1m".to_string(),
        m if m < 60 => format!("{m}m"),
        m => format!("{}h{:02}m", m / 60, m % 60),
    }
}

fn format_expiry_time(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

fn reenter_command(seat: &str) -> String {
    format!("eval \"$(aida role enter {seat})\"")
}

/// Short statusline suffix for a grant inside `within` of expiry:
/// `expires in 42m`, or `grant expired`.
// trace:TASK-1736 | ai:claude
pub(crate) fn expiry_suffix(
    expiry: &GrantExpiry,
    now: DateTime<Utc>,
    within: Duration,
) -> Option<String> {
    let left = expiry.expires_at - now;
    if left >= within {
        None
    } else if left <= Duration::zero() {
        Some("grant expired".to_string())
    } else {
        Some(format!("expires in {}", format_remaining(left)))
    }
}

/// One-line warning for a grant inside `within` of expiry, naming the
/// expiry time and the command that renews it.
// trace:TASK-1736 | ai:claude
pub(crate) fn expiry_warning(
    expiry: &GrantExpiry,
    now: DateTime<Utc>,
    within: Duration,
) -> Option<String> {
    let left = expiry.expires_at - now;
    if left >= within {
        return None;
    }
    let seat = &expiry.seat;
    let at = format_expiry_time(expiry.expires_at);
    let cmd = reenter_command(seat);
    Some(if left <= Duration::zero() {
        format!(
            "The {seat} seat grant expired at {at}. Commands gated on {seat} will refuse until a human re-enters it at a terminal: {cmd}"
        )
    } else {
        format!(
            "The {seat} seat grant expires at {at} (in {}). Commands gated on {seat} will refuse after that. A human renews it at a terminal: {cmd}",
            format_remaining(left)
        )
    })
}

/// The refusal a relay prints instead of launching a session whose grant
/// has less than `within` left.
// trace:TASK-1736 | ai:claude
pub(crate) fn launch_refusal(
    expiry: &GrantExpiry,
    now: DateTime<Utc>,
    within: Duration,
) -> Option<String> {
    let left = expiry.expires_at - now;
    if left >= within {
        return None;
    }
    let seat = &expiry.seat;
    let at = format_expiry_time(expiry.expires_at);
    let state = if left <= Duration::zero() {
        format!("expired at {at}")
    } else {
        format!("expires at {at}, in {}", format_remaining(left))
    };
    Some(format!(
        "Not launching: the {seat} seat grant {state}, and a session needs at least {} left. Re-enter the seat at a terminal, then start again: {}",
        format_remaining(within),
        reenter_command(seat)
    ))
}

/// The refusal a relay prints when `AIDA_SESSION_GRANT` is set but names no
/// usable grant record: launching would only meet refusals mid-run. `label`
/// is the session's role label, used to name the renew command when known.
// trace:TASK-1736 | ai:claude
pub(crate) fn unresolved_launch_refusal(reason: &str, label: Option<&str>) -> String {
    let seat = label
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| crate::canonical_role_name(&s.to_ascii_lowercase()))
        .unwrap_or_else(|| "<seat>".to_string());
    format!(
        "Not launching: this session's seat grant cannot be used ({reason}). Re-enter the seat at a terminal, then start again: {}",
        reenter_command(&seat)
    )
}

pub(crate) fn require_direct_tty() -> Result<()> {
    if !(crate::authority_stdin_is_terminal() && crate::authority_stderr_is_terminal()) {
        bail!("role grants require a human at an interactive TTY; run `aida role enter` from your terminal");
    }
    Ok(())
}

pub(crate) fn require_non_agent_session() -> Result<()> {
    if ["AIDA_AGENT_NAME", "AIDA_AGENT_TYPE"].iter().any(|key| {
        std::env::var(key)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    }) {
        bail!("AIDA-managed agent sessions cannot issue direct TTY grants; request a scoped child seat from the launcher");
    }
    Ok(())
}

// Shared by direct seat issuance and operator-only ship explanations.
// trace:TASK-1602 | ai:codex
pub(crate) fn require_direct_human() -> Result<()> {
    require_non_agent_session()?;
    require_direct_tty()?;
    Ok(())
}

// trace:TASK-1787 | ai:antigravity
pub(crate) const DEFAULT_MAX_GRANT_HOURS: u32 = 24;
pub(crate) const HARD_CEILING_GRANT_HOURS: u32 = 168;

/// Parse `[seat] max_grant_hours` from a config file's contents.
// trace:TASK-1787 | ai:antigravity
pub(crate) fn parse_max_grant_hours_from_str(body: &str) -> Option<u32> {
    let value: toml::Value = match toml::from_str(body) {
        Ok(v) => v,
        Err(_) => {
            static WARNED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("Warning: failed to parse config TOML");
            }
            return None;
        }
    };
    let seat_table = value.get("seat")?;
    let hours_val = seat_table.get("max_grant_hours")?;
    if let Some(i) = hours_val.as_integer() {
        match u32::try_from(i) {
            Ok(val) => Some(std::cmp::min(val, HARD_CEILING_GRANT_HOURS)),
            Err(_) => {
                eprintln!("Warning: [seat] max_grant_hours must be a positive integer, got `{i}`");
                None
            }
        }
    } else {
        eprintln!("Warning: [seat] max_grant_hours must be an integer, got `{hours_val}`");
        None
    }
}

/// Read `[seat] max_grant_hours` from a TOML file path.
// trace:TASK-1787 | ai:antigravity
pub(crate) fn read_max_grant_hours_file(path: &Path) -> Option<u32> {
    let body = std::fs::read_to_string(path).ok()?;
    parse_max_grant_hours_from_str(&body)
}

/// Resolve the configured maximum grant lifetime in hours:
/// Global `~/.aida/config.toml` and project `.aida/config.toml`.
/// Effective cap = min(global, project) when both are set.
/// The project value alone may only lower the 24h default's ceiling.
// trace:TASK-1787 | ai:antigravity
pub(crate) fn resolve_max_grant_hours(project_root: &Path) -> u32 {
    let global_val = crate::aida_home_dir()
        .map(|h| h.join(".aida").join("config.toml"))
        .as_deref()
        .and_then(read_max_grant_hours_file);
    let project_path = project_root.join(".aida").join("config.toml");
    let project_val = read_max_grant_hours_file(&project_path);
    let cap = match (global_val, project_val) {
        (Some(g), Some(p)) => std::cmp::min(g, p),
        (Some(g), None) => g,
        (None, Some(p)) => std::cmp::min(DEFAULT_MAX_GRANT_HOURS, p),
        (None, None) => DEFAULT_MAX_GRANT_HOURS,
    };
    std::cmp::min(cap, HARD_CEILING_GRANT_HOURS)
}

/// Parse and validate `--ttl <dur>` against the configured cap and hard ceiling.
/// Returns `Ok(None)` when `--ttl` is omitted (signalling the 24h default).
// trace:TASK-1787 | ai:antigravity
pub(crate) fn parse_ttl(project_root: &Path, dur_str: Option<&str>) -> Result<Option<Duration>> {
    let Some(dur_str) = dur_str else {
        return Ok(None);
    };
    let dur_str = dur_str.trim();
    if dur_str.is_empty() {
        return Ok(None);
    }
    if !dur_str.chars().any(|c| c.is_ascii_alphabetic()) {
        bail!("--ttl needs a unit, e.g. 72h or 90m");
    }
    let std_dur = crate::drain_caps::parse_duration(dur_str).ok_or_else(|| {
        anyhow::anyhow!("invalid grant TTL `{dur_str}`: expected a duration like 72h, 24h, or 90m")
    })?;
    let dur = Duration::from_std(std_dur)
        .map_err(|e| anyhow::anyhow!("invalid grant TTL `{dur_str}`: {e}"))?;
    if dur <= Duration::zero() {
        bail!("grant TTL must be positive");
    }
    if dur > Duration::hours(HARD_CEILING_GRANT_HOURS as i64) {
        bail!("requested TTL `{dur_str}` exceeds the 168h maximum (not configurable)");
    }
    let configured_cap = resolve_max_grant_hours(project_root);
    if dur > Duration::hours(configured_cap as i64) {
        bail!(
            "requested TTL `{dur_str}` exceeds the configured cap of {configured_cap}h ([seat] max_grant_hours)"
        );
    }
    Ok(Some(dur))
}

/// Format a grant's TTL for display (e.g. `72h`, `24h`, `30m`, `1h30m`).
// trace:TASK-1787 | ai:antigravity
pub(crate) fn format_ttl(ttl: Duration) -> String {
    let total_mins = ttl.num_minutes();
    if total_mins <= 0 {
        return "0m".to_string();
    }
    if total_mins % 60 == 0 {
        format!("{}h", total_mins / 60)
    } else if total_mins < 60 {
        format!("{total_mins}m")
    } else {
        format!("{}h{:02}m", total_mins / 60, total_mins % 60)
    }
}

/// The roster store that authorizes `principal` for `seat`, or the refusal.
/// An unreachable store or missing roster is named as such (with the path
/// probed) before the roster's contents are ever blamed.
// trace:BUG-2200 | ai:claude
// trace:TASK-1953, TASK-2054 | ai:claude — the refusal names the roster file read.
// trace:TASK-2072, TASK-1667 | ai:claude
fn issuance_roster_store(project_root: &Path, principal: &str, seat: &str) -> Result<PathBuf> {
    roster_check(project_root, principal, seat).map_err(|refusal| match refusal {
        unreachable @ RosterRefusal::Unreachable { .. } => anyhow::anyhow!(
            "cannot check seat `{seat}` for `{principal}`: {}",
            unreachable.describe("")
        ),
        denied => anyhow::anyhow!(
            "{}; an interactive human must update the roster with `aida team allow-seat`",
            denied.describe(&format!("seat `{seat}` for `{principal}` is not allowed"))
        ),
    })
}

/// BUG-1934 (SKETCH v2): a directly issued grant plus, unless it was bound
/// with `--bind-pid`, the one-time secret whose digest its binding carries.
/// The secret is never serialized: the `aida role enter` eval text hands it
/// to `aida role adopt-key`, which puts it in the shell's session keyring.
// trace:BUG-1934 | ai:claude
pub(crate) struct IssuedGrant {
    pub grant: SeatGrant,
    pub key_secret: Option<String>,
}

/// Never prints the secret.
impl std::fmt::Debug for IssuedGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedGrant")
            .field("grant", &self.grant)
            .field(
                "key_secret",
                &self.key_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl std::ops::Deref for IssuedGrant {
    type Target = SeatGrant;
    fn deref(&self) -> &SeatGrant {
        &self.grant
    }
}

// trace:STORY-1473, TASK-1592 | ai:antigravity
// trace:TASK-1787 | ai:antigravity
pub(crate) fn issue_direct(
    project_root: &Path,
    seat: &str,
    delegable_seats: Vec<String>,
    ttl: Option<Duration>,
    bind_pid: Option<u32>,
) -> Result<IssuedGrant> {
    require_direct_human()?;
    // SKETCH v2: a grant for THIS terminal is also bound to a session-key
    // secret where the kernel offers a session keyring (elsewhere it stays
    // process-bound only); `--bind-pid` stays pid-only (Q4).
    // trace:BUG-1934 | ai:claude
    let key_secret = (bind_pid.is_none() && crate::grant_keyring::available()).then(new_key_secret);
    // Round 2: the eval text then carries that secret, so it is only ever
    // written to a pipe the shell eval's, never printed to the terminal.
    // trace:BUG-1934 | ai:claude
    if key_secret.is_some() && crate::authority_stdout_is_terminal() {
        bail!(
            "`aida role enter` prints shell code holding a one-time secret; run it as `eval \"$(aida role enter {seat})\"` (or through the aida shell helper) so it is never shown on the terminal"
        );
    }
    // BUG-1934: bind before anything is written; an unbindable grant is
    // never minted. trace:BUG-1934 | ai:claude
    let caller = presenter_source();
    let mut binding = match bind_pid {
        None => direct_issue_binding(&caller)?,
        Some(pid) => vec![check_bind_target(
            pid,
            probe_bind_target(pid).as_ref(),
            &caller,
            current_uid(),
        )?],
    };
    // trace:TASK-1594 | ai:claude
    let subject = crate::current_user_id(None);
    let principal = subject.clone();
    let store = issuance_roster_store(project_root, &principal, seat)?;
    let allowed = aida_core::team::TeamRoster::load(&store)
        .seats_for(&principal)
        .unwrap_or_default()
        .into_iter()
        .map(|value| aida_core::team::canonical_role(&value))
        .collect::<std::collections::BTreeSet<_>>();
    let mut delegable_seats = delegable_seats
        .into_iter()
        .map(|seat| aida_core::team::canonical_role(&seat))
        .collect::<Vec<_>>();
    delegable_seats.sort();
    delegable_seats.dedup();
    if delegable_seats.iter().any(|seat| !allowed.contains(seat)) {
        bail!("delegated seats must all be present in the team roster");
    }
    let lifetime = ttl.unwrap_or(MAX_GRANT_AGE);
    let now = Utc::now();
    let grant = SeatGrant {
        id: Uuid::new_v4().to_string(),
        subject,
        principal,
        session_id: Uuid::new_v4().to_string(),
        seat: seat.to_string(),
        tty_issued_at: now,
        delegable_seats,
        parent_grant_id: None,
        issued_at: now,
        expires_at: now + lifetime,
        revoked_at: None,
        binding: {
            binding.extend(key_secret.as_deref().map(|k| key_token(k.as_bytes())));
            binding
        },
    };
    write_grant(&grant)?;
    Ok(IssuedGrant { grant, key_secret })
}

/// BUG-1934 (SKETCH v2): `aida role adopt-key <id>`: install grant `id`'s
/// secret (read from stdin by the caller) into a fresh session keyring and
/// hand that keyring to the PARENT process, the shell that eval'd `aida role
/// enter`. Only a secret whose digest the grant's binding carries is
/// accepted, and only for a live grant of this user; the key expires with
/// the grant.
// trace:BUG-1934 | ai:claude
pub(crate) fn adopt_key(id: &str, secret: &str) -> Result<()> {
    let secret = secret.trim();
    let grant = read_grant(id)?;
    if grant.subject != crate::current_user_id(None) {
        bail!(
            "session grant {} was not issued to this user",
            &id[..8.min(id.len())]
        );
    }
    check_lifetime(&grant, Utc::now())
        .map_err(|why| anyhow::anyhow!("cannot adopt the key: {why}"))?;
    if secret.is_empty() || !grant.binding.contains(&key_token(secret.as_bytes())) {
        bail!(
            "that secret does not belong to session grant {}",
            &id[..8.min(id.len())]
        );
    }
    let ttl = (grant.expires_at - Utc::now())
        .num_seconds()
        .clamp(1, u32::MAX as i64) as u32;
    crate::grant_keyring::install_to_parent(id, secret.as_bytes(), ttl)
}

/// BUG-1934: what `aida role enter` binds a grant to: the shell that ran it
/// (its parent) and that shell's POSIX session leader, so every process in
/// that terminal session (an agent started there, its tool calls) can
/// present it and nothing outside the session can. The shell's start
/// identity must read; a leader without one is left out.
// trace:BUG-1934 | ai:claude
pub(crate) fn direct_issue_binding(
    caller: &crate::review_authority::IdentitySource,
) -> Result<Vec<String>> {
    let token = |kind: &str, p: &crate::review_authority::ProcEntry| {
        p.start
            .as_deref()
            .filter(|s| !s.trim().is_empty() && p.pid > 1)
            .map(|start| crate::review_authority::process_token(kind, p.pid, Some(start)))
    };
    let shell = caller
        .chain
        .get(1)
        .and_then(|p| token("proc", p))
        .context(
            "cannot bind the grant: the start identity of the shell running `aida role enter` could not be read, so no grant was issued",
        )?;
    let mut binding = vec![shell];
    if let Some(leader) = caller.session_leader.as_ref().and_then(|p| token("sid", p)) {
        if !binding
            .iter()
            .any(|b| crate::review_authority::binding_tokens_match(b, &leader))
        {
            binding.push(leader);
        }
    }
    Ok(binding)
}

/// BUG-1934: a live process `aida role enter --bind-pid` may bind a grant to.
// trace:BUG-1934 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindTarget {
    pub entry: crate::review_authority::ProcEntry,
    /// Owning uid, when the platform reports one.
    pub uid: Option<u32>,
}

/// Interactive shells `--bind-pid` accepts besides agent harnesses.
const BIND_SHELLS: &[&str] = &[
    "bash", "zsh", "sh", "dash", "fish", "ksh", "mksh", "tcsh", "csh", "nu", "pwsh",
];

/// BUG-1934: the binding token for `--bind-pid <pid>`, or why it is refused.
/// The target must be a live process of the same user, an agent harness or
/// an interactive shell, with a readable start identity, and neither pid 1
/// nor an ancestor of the caller above the caller's own terminal session
/// (an init, session manager, terminal emulator or multiplexer would bind
/// far more than one session).
// trace:BUG-1934 | ai:claude
pub(crate) fn check_bind_target(
    pid: u32,
    target: Option<&BindTarget>,
    caller: &crate::review_authority::IdentitySource,
    caller_uid: Option<u32>,
) -> Result<String> {
    if pid <= 1 {
        bail!("--bind-pid {pid}: refusing to bind a grant to pid {pid}");
    }
    let Some(target) = target.filter(|t| t.entry.pid == pid) else {
        bail!("--bind-pid {pid}: no running process has that pid");
    };
    if caller_uid.is_none() || target.uid != caller_uid {
        bail!("--bind-pid {pid}: the process does not belong to the user entering the role");
    }
    let name = target.entry.name.trim_start_matches('-');
    let is_shell = BIND_SHELLS.contains(&name);
    if !is_shell
        && !crate::review_authority::is_agent_harness(&target.entry.name, &target.entry.cmd)
    {
        bail!(
            "--bind-pid {pid}: `{}` is neither an agent harness (claude, codex, agy, ...) nor a shell",
            target.entry.name
        );
    }
    if let Some(at) = caller.chain.iter().position(|p| p.pid == pid) {
        let leader_at = caller
            .session_leader
            .as_ref()
            .and_then(|l| caller.chain.iter().position(|p| p.pid == l.pid));
        if leader_at.is_none_or(|leader_at| at > leader_at) {
            bail!(
                "--bind-pid {pid}: the process is above this terminal session in your own process tree; name the agent session's harness pid instead"
            );
        }
    }
    target
        .entry
        .start
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|start| crate::review_authority::process_token("proc", pid, Some(start)))
        .with_context(|| format!("--bind-pid {pid}: the process start identity could not be read"))
}

/// Live probe behind `--bind-pid`: one single-pid refresh, never a table scan.
// trace:BUG-1934 | ai:claude
fn probe_bind_target(pid: u32) -> Option<BindTarget> {
    use sysinfo::{ProcessRefreshKind, System, UpdateKind};
    let spid = sysinfo::Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_pids_specifics(
        &[spid],
        ProcessRefreshKind::new()
            .with_cmd(UpdateKind::Always)
            .with_user(UpdateKind::Always),
    );
    let proc = sys.process(spid)?;
    #[cfg(unix)]
    let uid = proc.user_id().map(|u| **u);
    #[cfg(not(unix))]
    let uid = None;
    Some(BindTarget {
        entry: crate::review_authority::ProcEntry {
            pid,
            name: proc.name().to_string(),
            cmd: proc.cmd().to_vec(),
            start: crate::process_probe::process_start_identity(pid),
        },
        uid,
    })
}

fn current_uid() -> Option<u32> {
    #[cfg(unix)]
    {
        // SAFETY: getuid has no preconditions and cannot fail.
        Some(unsafe { libc::getuid() })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Read-only launch preflight; issuance repeats this validation after setup.
// trace:TASK-1337 | ai:codex
pub(crate) fn validate_child_delegation(
    project_root: &Path,
    requested_seat: &str,
) -> Result<SeatGrant> {
    let parent = current_grant(project_root).context("no valid parent seat grant is active")?;
    if !parent
        .delegable_seats
        .iter()
        .any(|seat| seat == requested_seat)
    {
        // trace:TASK-1597 | ai:claude
        bail!(
            "the active `{}` grant does not delegate `{requested_seat}` (re-enter the role to include it: `aida role enter {} --delegate-seat {requested_seat}`)",

            parent.seat,
            parent.seat
        );
    }
    // trace:BUG-2200 | ai:claude
    // trace:TASK-1953, TASK-2054 | ai:claude — the refusal names the roster file read.
    // trace:TASK-2072, TASK-1667 | ai:claude
    roster_check(project_root, &parent.principal, requested_seat).map_err(
        |refusal| match refusal {
            unreachable @ RosterRefusal::Unreachable { .. } => anyhow::anyhow!(
                "cannot check child seat `{requested_seat}`: {}",
                unreachable.describe("")
            ),
            denied => anyhow::anyhow!(
                "{}",
                denied.describe(&format!(
                    "requested child seat `{requested_seat}` is no longer allowed"
                ))
            ),
        },
    )?;
    Ok(parent)
}

pub(crate) fn issue_child(
    project_root: &Path,
    requested_seat: &str,
    child_subject: &str,
) -> Result<SeatGrant> {
    let parent = validate_child_delegation(project_root, requested_seat)?;
    // BUG-1934: a child grant is bound to the launcher minting it. An exec'd
    // worker keeps this pid and start; a spawned worker descends from it; a
    // detached worker is stamped with [`bind_grant_to_pid`] after spawn.
    // trace:BUG-1934 | ai:claude
    let launcher = own_process_token().context(
        "cannot bind the child grant: this launcher's process start identity could not be read",
    )?;
    let now = Utc::now();
    let id = Uuid::new_v4().to_string();
    // SKETCH v2: the launcher's session keyring holds the child's key before
    // it spawns or execs the worker, so the worker and its sandboxed tool
    // calls inherit it. When the keyring is unavailable the grant keeps only
    // its process binding (a sandboxed worker is then refused, never
    // allowed: C5). trace:BUG-1934 | ai:claude
    let secret = new_key_secret();
    let ttl = (parent.expires_at - now)
        .num_seconds()
        .clamp(1, u32::MAX as i64) as u32;
    let mut binding = vec![launcher];
    match crate::grant_keyring::install(&id, secret.as_bytes(), ttl) {
        Ok(()) => binding.push(key_token(secret.as_bytes())),
        // TASK-2195 (round 3): never discard a keyring failure. Quota
        // exhaustion refuses the launch: a proc-only child could never be
        // presented by a sandboxed worker. Any other failure is named.
        // trace:BUG-1934 | ai:claude
        Err(e) if e.quota_exhausted() => bail!("{}", key_quota_refusal(requested_seat, &e)),
        Err(e) => eprintln!(
            "Warning: the child `{requested_seat}` grant is bound to this launcher's process only: {e}. A worker whose tool calls run in a sandbox cannot present it"
        ),
    }
    let grant = SeatGrant {
        id,
        principal: parent.principal,
        subject: child_subject.to_string(),
        session_id: Uuid::new_v4().to_string(),
        seat: requested_seat.to_string(),
        tty_issued_at: parent.tty_issued_at,
        delegable_seats: Vec::new(),
        parent_grant_id: Some(parent.id),
        issued_at: now,
        expires_at: parent.expires_at,
        revoked_at: None,
        binding,
    };
    write_grant(&grant)?;
    Ok(grant)
}

/// TASK-2195: why a child launch is refused when the kernel key quota is
/// exhausted, and how to free it.
// trace:BUG-1934 | ai:claude
pub(crate) fn key_quota_refusal(seat: &str, err: &crate::grant_keyring::InstallError) -> String {
    format!(
        "cannot launch the `{seat}` worker: its grant key does not fit in your kernel key quota ({err}; kernel.keys.maxkeys / kernel.keys.maxbytes). A worker without the key is refused in a sandbox. End stale role sessions and launchers (`eval \"$(aida role end)\"`), or raise the quota (`sudo sysctl kernel.keys.maxkeys=<n> kernel.keys.maxbytes=<n>`), then launch again"
    )
}

/// BUG-2139: the seats that confer nothing a grant gates — the implementer
/// baseline and the lower-privilege stakeholder personas. ONE list, read by
/// both the statusline classification ([`classify_role_display`]) and the
/// headless-child baseline fallback ([`preflight_headless_child`]), so the two
/// can never disagree about which seats are safe to run ungranted.
// trace:BUG-2139 | ai:claude
pub(crate) const BASELINE_SEATS: &[&str] = &["implementer", "guest", "requester"];

/// Whether `seat` (any spelling) is one of [`BASELINE_SEATS`].
// trace:BUG-2139 | ai:claude
pub(crate) fn is_baseline_seat(seat: &str) -> bool {
    let seat = aida_core::team::canonical_role(seat.trim());
    BASELINE_SEATS.contains(&seat.as_str())
}

/// BUG-1934: `--bg` hands the session to the agent's own background daemon,
/// which this launcher never parents and whose pid it never learns, so a seat
/// grant cannot be bound to the session. A baseline seat (implementer, ...)
/// runs ungranted; any other seat is refused before launch rather than
/// started with a grant every gated command would reject.
// STORY-1888: seat-authority gate code, so it lives on the mandate floor.
// trace:BUG-1934 | ai:claude
// trace:STORY-1888 | ai:claude
pub(crate) fn bg_seat_refusal(role: &str) -> Result<()> {
    let seat = crate::canonical_role_name(role);
    if is_baseline_seat(&seat) {
        return Ok(());
    }
    anyhow::bail!(
        "`--bg` cannot launch a `{seat}` session: it runs under the agent's background daemon, outside this launcher's process tree, so its seat grant cannot be bound to it. Launch it without `--bg` from a terminal that entered the role (`eval \"$(aida role enter {seat})\"`); `--bind-pid` would serve only its unsandboxed hooks and MCP server, not its tool calls"
    )
}

/// BUG-2139: the authority a fresh headless queue-work child will run with.
/// It never inherits the launcher's grant.
// trace:BUG-2139 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeadlessChildAuthority {
    /// The launcher's grant delegates the seat; a child grant is minted.
    Delegated,
    /// The launcher cannot delegate a BASELINE seat; the child runs with
    /// `AIDA_SESSION_GRANT=""` (baseline authority only). `cause` says why.
    Baseline { cause: String },
}

/// BUG-2139: read-only preflight for a fresh headless child in `seat`. A seat
/// the launcher's grant delegates is [`HeadlessChildAuthority::Delegated`]. A
/// baseline seat it cannot delegate (no grant, or a grant that does not
/// delegate it) falls back to [`HeadlessChildAuthority::Baseline`]. Every other
/// seat (reviewer, advisor, integrator, ...) fails closed.
// trace:BUG-2139 | ai:claude
pub(crate) fn preflight_headless_child(
    project_root: &Path,
    seat: &str,
) -> Result<HeadlessChildAuthority> {
    let seat = aida_core::team::canonical_role(seat.trim());
    match validate_child_delegation(project_root, &seat) {
        Ok(_) => Ok(HeadlessChildAuthority::Delegated),
        Err(cause) if is_baseline_seat(&seat) => Ok(HeadlessChildAuthority::Baseline {
            cause: format!("{cause:#}"),
        }),
        Err(cause) => Err(cause),
    }
}

/// BUG-2139: the grant a fresh headless child was launched with.
// trace:BUG-2139 | ai:claude
#[derive(Debug, Clone)]
pub(crate) enum HeadlessChildGrant {
    /// A child grant minted for this launch (seat = routed role,
    /// `delegable_seats` empty, parent = the launcher's grant).
    Issued(SeatGrant),
    /// No grant: the child runs at baseline authority. `cause` says why the
    /// launcher could not delegate the seat.
    Baseline { cause: String },
}

impl HeadlessChildGrant {
    /// The value the child's `AIDA_SESSION_GRANT` is set to — ALWAYS set,
    /// never inherited: the minted id, or `""` for a baseline child.
    pub(crate) fn env_value(&self) -> &str {
        match self {
            Self::Issued(grant) => &grant.id,
            Self::Baseline { .. } => "",
        }
    }

    /// The minted grant id, if any.
    pub(crate) fn id(&self) -> Option<&str> {
        match self {
            Self::Issued(grant) => Some(&grant.id),
            Self::Baseline { .. } => None,
        }
    }
}

/// BUG-2139: mint the authority for a fresh headless child in `seat`.
/// [`preflight_headless_child`] decides; a delegated seat goes through
/// [`issue_child`], which re-validates, so a roster edit or a parent revoked
/// between preflight and launch still fails closed.
// trace:BUG-2139 | ai:claude
pub(crate) fn issue_headless_child(
    project_root: &Path,
    seat: &str,
    child_subject: &str,
) -> Result<HeadlessChildGrant> {
    let seat = aida_core::team::canonical_role(seat.trim());
    match preflight_headless_child(project_root, &seat)? {
        HeadlessChildAuthority::Delegated => Ok(HeadlessChildGrant::Issued(issue_child(
            project_root,
            &seat,
            child_subject,
        )?)),
        HeadlessChildAuthority::Baseline { cause } => Ok(HeadlessChildGrant::Baseline { cause }),
    }
}

pub(crate) fn revoke_current() -> Result<bool> {
    let Some(id) = std::env::var(GRANT_ENV).ok() else {
        return Ok(false);
    };
    revoke_by_id(&id)
}

/// Revoke grant `id` now. `Ok(false)` when it does not exist or is already
/// revoked. BUG-2139: a headless child's grant is revoked this way when the
/// child exits and when its lease ends or is reaped.
// trace:BUG-2139 | ai:claude
pub(crate) fn revoke_by_id(id: &str) -> Result<bool> {
    if read_grant(id).is_err() {
        return Ok(false);
    }
    // BUG-1934 v2 C2: a revoked grant's session key is invalidated too,
    // wherever this process can reach it (the role shell re-entering or
    // exiting, a launcher ending its worker). trace:BUG-1934 | ai:claude
    crate::grant_keyring::invalidate(id);
    // BUG-1934: under the record lock, so a concurrent pid stamp cannot
    // write back a pre-revocation copy. trace:BUG-1934 | ai:claude
    let revoked = update_grant(
        id,
        |grant| {
            if grant.revoked_at.is_some() {
                return false;
            }
            grant.revoked_at = Some(Utc::now());
            true
        },
        |grant| grant.revoked_at.is_some(),
    )?;
    // Round 2: the revoked grant's lock (and any expired grant's) is
    // removed. trace:BUG-1934 | ai:claude
    sweep_grant_locks(Utc::now());
    Ok(revoked)
}

/// BUG-1934 (C4): bind grant `id` to live process `pid` as well: a launcher
/// stamps the worker it detached (setsid, nohup, a new terminal), which no
/// longer descends from the launcher the grant was minted bound to. Locked
/// and re-verified ([`update_grant`]), so concurrent stamps on one grant
/// all land. Fails when `pid`'s start identity cannot be read: the worker
/// then stays unbound and is refused, never silently trusted.
// trace:BUG-1934 | ai:claude
pub(crate) fn bind_grant_to_pid(id: &str, pid: u32) -> Result<()> {
    let token = pid_binding_token(pid).with_context(|| {
        format!("cannot bind session grant to process {pid}: its start identity could not be read")
    })?;
    bind_grant_token(id, &token)
}

/// Token-level core of [`bind_grant_to_pid`].
// trace:BUG-1934 | ai:claude
pub(crate) fn bind_grant_token(id: &str, token: &str) -> Result<()> {
    update_grant(
        id,
        |grant| {
            if grant.binding.iter().any(|b| b == token) {
                return false;
            }
            grant.binding.push(token.to_string());
            true
        },
        |grant| grant.binding.iter().any(|b| b == token),
    )?;
    Ok(())
}

/// BUG-1918: did grant `id` authorize seat `seat` at instant `at`? Re-read
/// from the grant store — never trusted from a recorded copy. The grant (and
/// its parent, for a delegated child) must exist, carry exactly that seat,
/// have been issued before `at`, not expired or been revoked by `at`, and the
/// seat must still be allowed by the roster. An implementer seat never
/// authorizes an approval.
///
/// BUG-1934: `presented_by` is what the recorder carried for the grant's
/// binding (an attestation's `grant_binding`, the advisor turn stamp's
/// binding): it must strictly match the grant's own binding, so an unbound
/// grant, or one presented outside its process tree, authorizes nothing.
// trace:BUG-1918 | ai:claude
// trace:BUG-1934 | ai:claude
pub(crate) fn grant_authorized_at(
    project_root: &Path,
    id: &str,
    seat: &str,
    at: DateTime<Utc>,
    presented_by: &[String],
) -> bool {
    let seat = aida_core::team::canonical_role(seat.trim());
    if seat.is_empty() || seat == "implementer" {
        return false;
    }
    let valid_at = |g: &SeatGrant| {
        g.issued_at <= at && at < g.expires_at && g.revoked_at.is_none_or(|r| r > at)
    };
    let Ok(grant) = read_grant(id) else {
        return false;
    };
    if aida_core::team::canonical_role(&grant.seat) != seat
        || !crate::review_authority::binding_overlaps(&grant.binding, presented_by)
        || !valid_at(&grant)
        || !roster_allows(project_root, &grant.principal, &seat)
    {
        return false;
    }
    match &grant.parent_grant_id {
        None => true,
        Some(parent_id) => read_grant(parent_id).is_ok_and(|parent| {
            valid_at(&parent)
                && parent.principal == grant.principal
                && parent.delegable_seats.iter().any(|s| s == &grant.seat)
        }),
    }
}

/// BUG-1934: what `aida role status` reports about the exported grant.
// trace:BUG-1934 | ai:claude
#[derive(Debug, Clone, Serialize)]
pub(crate) struct GrantStatusGrant {
    pub id_prefix: String,
    pub seat: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub valid: bool,
    pub binding: Vec<String>,
    pub bound_to: String,
    /// Whether this process may present the grant: `None` when its record
    /// cannot be read.
    pub binding_ok: Option<bool>,
    /// SKETCH v2: how this process satisfies the binding: `key` (it holds
    /// the grant's session key), `proc` (a bound process is an ancestor) or
    /// `none`. `None` when the record cannot be read.
    pub via: Option<&'static str>,
    pub rejection: Option<String>,
}

/// BUG-1934: `aida role status --json`.
// trace:BUG-1934 | ai:claude
#[derive(Debug, Clone, Serialize)]
pub(crate) struct GrantStatus {
    pub grant: Option<GrantStatusGrant>,
    /// This session's agent harness: the pid `--bind-pid` should name.
    /// `None` when there is no stable one: no harness above this command, or
    /// the walk ends in a sandbox's PID namespace. TASK-2159: the transient
    /// shell a tool call runs in is never offered.
    // trace:TASK-2159 | ai:claude
    pub harness_pid: Option<u32>,
}

// trace:BUG-1934 | ai:claude
pub(crate) fn grant_status(project_root: &Path) -> GrantStatus {
    let source = crate::review_authority::IdentitySource::live();
    let harness_pid = source.agent_root().map(|p| p.pid).filter(|p| *p > 1);
    let check = grant_check(project_root);
    let grant = current_grant_id().map(|id| {
        let record = read_grant(&id).ok();
        let binding = record
            .as_ref()
            .map(|g| g.binding.clone())
            .unwrap_or_default();
        GrantStatusGrant {
            id_prefix: id.chars().take(8).collect(),
            seat: record.as_ref().map(|g| g.seat.clone()),
            expires_at: record.as_ref().map(|g| g.expires_at),
            valid: matches!(check, GrantCheck::Valid(_)),
            bound_to: describe_binding(&binding),
            binding_ok: record
                .as_ref()
                .map(|g| presenter_rejection(g, &presenter_tokens_for(g)).is_none()),
            via: record.as_ref().map(|g| {
                if possession_token(g).is_some() {
                    "key"
                } else if crate::review_authority::binding_overlaps(&g.binding, &presenter_tokens())
                {
                    "proc"
                } else {
                    "none"
                }
            }),
            binding,
            rejection: check.rejection_sentence(),
        }
    });
    GrantStatus { grant, harness_pid }
}

/// BUG-1934: the `seat-grant` doctor category.
pub(crate) const DOCTOR_CATEGORY: &str = "seat-grant";

/// BUG-1934: a finding when this session exports a grant that is refused,
/// naming the reason (an unbound grant, one presented outside its process
/// tree, ...) instead of leaving a silent "no role". Silent otherwise.
// trace:BUG-1934 | ai:claude
pub(crate) fn doctor_scan(project_root: &Path) -> Vec<crate::DoctorFinding> {
    let mut findings: Vec<crate::DoctorFinding> = ptrace_scope_finding(
        current_grant_id().is_some(),
        std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope")
            .ok()
            .as_deref(),
    )
    .into_iter()
    .collect();
    let check = grant_check(project_root);
    let Some(sentence) = check.rejection_sentence() else {
        return findings;
    };
    let GrantCheck::Rejected { id, seat, .. } = &check else {
        return findings;
    };
    let prefix: String = id.chars().take(8).collect();
    findings.push(crate::DoctorFinding {
        category: DOCTOR_CATEGORY.into(),
        id: format!("{DOCTOR_CATEGORY}:{prefix}"),
        summary: sentence,
        action: doctor_action(seat.as_deref()),
        safe_heal: false,
    });
    findings
}

/// BUG-1934 round 3: the doctor action for a refused grant. It steers to the
/// round-2 recovery (re-enter in the terminal that starts the agent), never
/// to `--bind-pid`, which cannot reach a sandboxed session's tool calls.
// trace:BUG-1934 | ai:claude
pub(crate) fn doctor_action(seat: Option<&str>) -> String {
    let seat = seat.unwrap_or("<role>");
    format!(
        "run `eval \"$(aida role enter {seat})\"` in a terminal, then start the agent session from that terminal (`--bind-pid` binds only an already-running session's unsandboxed hooks and MCP server)"
    )
}

/// BUG-1934 (SKETCH v2): with yama `ptrace_scope` 0, any same-user process
/// may ptrace a key holder and read the grant's session key. Warned only
/// while this session holds a grant; scope 1 or higher (or no yama) is silent.
// trace:BUG-1934 | ai:claude
pub(crate) fn ptrace_scope_finding(
    holds_grant: bool,
    ptrace_scope: Option<&str>,
) -> Option<crate::DoctorFinding> {
    (holds_grant && ptrace_scope.map(str::trim) == Some("0")).then(|| crate::DoctorFinding {
        category: DOCTOR_CATEGORY.into(),
        id: format!("{DOCTOR_CATEGORY}:ptrace-scope"),
        summary: "kernel.yama.ptrace_scope is 0: any process of yours can read a seat grant's session key by tracing the shell that holds it".into(),
        action: "set kernel.yama.ptrace_scope to 1 (`sudo sysctl kernel.yama.ptrace_scope=1`, and persist it under /etc/sysctl.d/)".into(),
        safe_heal: false,
    })
}

pub(crate) fn grant_id(grant: &SeatGrant) -> &str {
    &grant.id
}

// trace:STORY-1473 | ai:claude
/// Hermetic grant establishment for tests.
///
/// ADR-66 removes every ambient authority path — `AIDA_SESSION_ROLE`, a bare
/// TTY, and roster-membership-as-seat — so a test that exercises an authorized
/// operation must establish a REAL grant the same way `aida role enter` does:
/// a roster ceiling in the project store plus a validated grant record in the
/// (hermetic) home, exported through `AIDA_SESSION_GRANT`. This writes exactly
/// the records production validates; nothing here is reachable from, or
/// weakens, the production issuance paths (`issue_direct` still requires a
/// controlling TTY).
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    thread_local! {
        /// BUG-1934: force [`in_foreign_pid_namespace`] for this test thread
        /// (`None` = probe for real). Unit tests default to "not foreign" so
        /// a test suite run inside a sandbox keeps its expected sentences.
        pub(crate) static FOREIGN_NAMESPACE: std::cell::Cell<Option<bool>> =
            const { std::cell::Cell::new(Some(false)) };
    }

    /// Panic unless `store` is a scratch store a test may write: under a
    /// system temp root, or under this workspace's `target/` (fixtures that
    /// must live outside `/tmp`). A test that resolves a real checkout's store
    /// fails here instead of rewriting it (BUG-2209).
    // trace:BUG-2209 | ai:claude
    pub(crate) fn assert_scratch_store(store: &Path) {
        if aida_core::team::is_under_temp_root(store) {
            return;
        }
        let target = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|workspace| workspace.join("target"));
        let canonical = store.canonicalize().unwrap_or_else(|_| store.to_path_buf());
        let in_target = target
            .and_then(|t| t.canonicalize().ok())
            .is_some_and(|t| canonical.starts_with(t));
        assert!(
            in_target,
            "test wrote a roster to `{}`, which is neither under a temp directory nor under \
             the workspace target/: the fixture resolved a real store",
            store.display()
        );
    }

    /// Write the store plumbing (`.aida/config.toml`, `objects/`) plus a
    /// roster ceiling allowing `seat` ∪ `delegable` for `subject`, then mint
    /// a direct grant bound to `subject` and return its handle. The caller
    /// exports it via `AIDA_SESSION_GRANT` through the shared env guards
    /// (`test_env::AmbientGuard::hermetic_with_seat` bundles all of it).
    pub(crate) fn mint_grant_for(
        project_root: &Path,
        subject: &str,
        seat: &str,
        delegable: &[&str],
    ) -> String {
        mint_grant_for_with_ttl(project_root, subject, seat, delegable, None)
    }

    // trace:TASK-1787 | ai:antigravity
    pub(crate) fn mint_grant_for_with_ttl(
        project_root: &Path,
        subject: &str,
        seat: &str,
        delegable: &[&str],
        ttl: Option<Duration>,
    ) -> String {
        // The mint resolves env-derived paths (the grant-store home via
        // `$AIDA_HOME`/`$HOME`); hold the process env lock for the duration
        // unless the caller already does, so a sibling test's guard cannot
        // swap those variables mid-mint and strand the record in a home this
        // mint never prepared. trace:STORY-1473 | ai:claude
        let _lock = (!crate::test_env::holds_env_lock()).then(crate::test_env::env_lock);
        let aida_dir = project_root.join(".aida");
        std::fs::create_dir_all(&aida_dir).unwrap();
        let config = aida_dir.join("config.toml");
        if !config.exists() {
            std::fs::write(&config, "store_path = \".aida-store\"\n").unwrap();
        }
        let store = crate::detect_distributed_store_from(project_root).unwrap_or_else(|| {
            // The fixture config exists but names no reachable store (e.g. an
            // `[agents]`-only config): append the default so the validation
            // path detects the same store this fixture seeds.
            let store = project_root.join(".aida-store");
            std::fs::create_dir_all(store.join("objects")).unwrap();
            let content = std::fs::read_to_string(&config).unwrap();
            if !content.contains("store_path") {
                // Prepended so the key stays top-level (appending after a
                // `[table]` header would land inside that table).
                std::fs::write(&config, format!("store_path = \".aida-store\"\n{content}"))
                    .unwrap();
            }
            store
        });
        std::fs::create_dir_all(store.join("objects")).unwrap();
        let registry = store.join("registry");
        std::fs::create_dir_all(&registry).unwrap();

        // Merge (never clobber) the roster ceiling for `subject`, and only
        // into a scratch store: a fixture whose walk-up escaped to a real
        // checkout once wrote this roster over the operator's (BUG-2209).
        // trace:BUG-2209 | ai:claude
        assert_scratch_store(&store);
        let seat = aida_core::team::canonical_role(seat);
        aida_core::team::write_member_seats(&store, subject, None, |current| {
            let mut seats = current.map(<[String]>::to_vec).unwrap_or_default();
            for requested in std::iter::once(seat.as_str()).chain(delegable.iter().copied()) {
                let canonical = aida_core::team::canonical_role(requested);
                if !seats.iter().any(|have| *have == canonical) {
                    seats.push(canonical);
                }
            }
            Some(seats)
        })
        .unwrap();

        let lifetime = ttl.unwrap_or(MAX_GRANT_AGE);
        let now = Utc::now();
        let grant = SeatGrant {
            id: Uuid::new_v4().to_string(),
            principal: subject.to_string(),
            subject: subject.to_string(),
            session_id: Uuid::new_v4().to_string(),
            seat,
            tty_issued_at: now,
            delegable_seats: delegable
                .iter()
                .map(|role| aida_core::team::canonical_role(role))
                .collect(),
            parent_grant_id: None,
            issued_at: now,
            expires_at: now + lifetime,
            revoked_at: None,
            // BUG-1934: bound to the test process, the presenter of every
            // in-process check and an ancestor of every binary a test spawns.
            // trace:BUG-1934 | ai:claude
            binding: own_process_token().into_iter().collect(),
        };
        write_grant(&grant).unwrap();
        grant.id
    }
}

// trace:STORY-1473 | ai:claude
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::EnvVarsGuard;
    use std::ffi::OsString;

    /// BUG-2125: the fixed identity every grant in this module is bound to.
    /// These tests used to read `current_user_id(None)` and the grant store
    /// (`$AIDA_HOME`) outside the env lock, between short-lived env guards, so
    /// a sibling test setting `AIDA_USER` could make the minted subject differ
    /// from the caller the later validation read, and the grant failed its
    /// subject check. Each grant-store test now holds ONE env guard for its
    /// whole body with `AIDA_USER` pinned here; per-step env changes go
    /// through [`Scoped`] under that same acquisition.
    // trace:BUG-2125 | ai:claude
    const SUBJECT: &str = "seat-authority-fixture-user";

    /// Hold the env lock for the whole test with `AIDA_USER` = [`SUBJECT`].
    // trace:BUG-2125 | ai:claude
    fn pinned_env() -> EnvVarsGuard {
        let mut env = EnvVarsGuard::snapshot(&["AIDA_USER"]);
        env.set_key("AIDA_USER", SUBJECT);
        env
    }

    /// Apply `pairs` for this value's lifetime under the test's already-held
    /// [`pinned_env`] guard (the env lock is not reentrant), restoring each
    /// key's prior value on drop.
    // trace:BUG-2125 | ai:claude
    struct Scoped<'a> {
        env: &'a mut EnvVarsGuard,
        prev: Vec<(&'static str, Option<OsString>)>,
    }

    impl<'a> Scoped<'a> {
        fn apply(env: &'a mut EnvVarsGuard, pairs: &[(&'static str, Option<&str>)]) -> Self {
            let mut prev = Vec::with_capacity(pairs.len());
            for (key, value) in pairs {
                prev.push((*key, std::env::var_os(key)));
                match value {
                    Some(v) => env.set_key(key, v),
                    None => env.unset_key(key),
                }
            }
            Self { env, prev }
        }
    }

    impl Drop for Scoped<'_> {
        fn drop(&mut self) {
            for (key, prev) in self.prev.iter().rev() {
                match prev {
                    Some(v) => self.env.set_key(key, v),
                    None => self.env.unset_key(key),
                }
            }
        }
    }

    // A record can change between the validator and informational metadata read.
    // Reject either inconsistency; matching metadata still emits an expired notice.
    // trace:BUG-2014 | ai:codex
    #[test]
    fn expired_notice_rejects_changed_timestamp_or_seat() {
        let at = Utc::now();
        let record = SeatGrant {
            id: "fixture".into(),
            principal: "notice-user".into(),
            subject: "notice-user".into(),
            session_id: "fixture-session".into(),
            seat: "advisor".into(),
            tty_issued_at: at,
            delegable_seats: Vec::new(),
            parent_grant_id: None,
            issued_at: at,
            expires_at: at,
            revoked_at: None,
            binding: Vec::new(),
        };
        for (expires_at, seat, emits) in [
            (at, "advisor", true),
            (at + Duration::minutes(1), "advisor", false),
            (at, "reviewer", false),
        ] {
            let mut reread = record.clone();
            reread.expires_at = expires_at;
            reread.seat = seat.into();
            let notice = notice_grant_from_check(
                GrantCheck::Rejected {
                    id: record.id.clone(),
                    seat: Some(record.seat.clone()),
                    reason: GrantRejection::Expired { at },
                },
                |_| Ok(reread.clone()),
            )
            .unwrap();
            assert_eq!(
                notice.is_some(),
                emits,
                "timestamp={expires_at}, seat={seat}"
            );
        }
    }

    /// ADR-66 acceptance: an env-only `AIDA_SESSION_ROLE=advisor` confers no
    /// seat; a validated grant does; revocation and a shrunk roster ceiling
    /// fail closed.
    #[test]
    fn env_role_alone_confers_no_seat_and_grants_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut env = pinned_env(); // trace:BUG-2125 | ai:claude

        // Env-only role, no grant: no seat.
        {
            let _env = Scoped::apply(
                &mut env,
                &[("AIDA_SESSION_ROLE", Some("advisor")), (GRANT_ENV, None)],
            );
            assert_eq!(current_seat(root), None, "env role alone must not seat");
        }

        // trace:TASK-1594 | ai:claude
        let id = test_support::mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);

        // A validated grant seats the session.
        {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
            assert_eq!(current_seat(root).as_deref(), Some("advisor"));

            // trace:TASK-1337 | ai:codex
            let before = std::fs::read_dir(grant_dir().unwrap()).unwrap().count();
            validate_child_delegation(root, "implementer").unwrap();
            validate_child_delegation(root, "reviewer")
                .expect_err("preflight must preserve delegation gate");
            assert_eq!(
                std::fs::read_dir(grant_dir().unwrap()).unwrap().count(),
                before,
                "read-only validation must not issue a child grant"
            );
            // The grant delegates only its explicit subset.
            let child = issue_child(root, "implementer", "child-subject")
                .expect("delegable child seat is issuable");
            assert_eq!(child.seat, "implementer");
            assert_eq!(child.parent_grant_id.as_deref(), Some(id.as_str()));
            assert!(
                child.delegable_seats.is_empty(),
                "children default to an empty delegation set"
            );
            issue_child(root, "reviewer", "child-subject")
                .expect_err("a seat outside the delegation set must refuse");

            // Revocation fails closed.
            assert!(revoke_current().unwrap());
            assert_eq!(current_seat(root), None, "revoked grant must not seat");
        }

        // A fresh grant dies when the roster ceiling no longer allows it.
        let id = test_support::mint_grant_for(root, SUBJECT, "reviewer", &[]);
        {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
            assert_eq!(current_seat(root).as_deref(), Some("reviewer"));
            let store = crate::detect_distributed_store_from(root).unwrap();
            std::fs::write(store.join("registry").join("team.toml"), "[members]\n").unwrap();
            assert_eq!(
                current_seat(root),
                None,
                "a shrunk roster ceiling invalidates the grant"
            );
        }
    }

    /// BUG-2139: a fresh headless child never inherits the launcher's grant.
    /// A delegated seat mints a distinct child grant (routed seat, no
    /// delegable seats, parent = launcher); a baseline seat the launcher
    /// cannot delegate falls back to an ungranted child; every other seat
    /// fails closed. The child grant is revocable by id.
    // trace:BUG-2139 | ai:claude
    #[test]
    fn headless_child_gets_its_own_grant_or_baseline_or_refusal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut env = pinned_env();
        // Use a short parent lifetime so a child defaulting to MAX_GRANT_AGE
        // cannot accidentally satisfy the expiry assertion.
        // trace:TASK-1789 | ai:codex
        let advisor = test_support::mint_grant_for_with_ttl(
            root,
            SUBJECT,
            "advisor",
            &["implementer", "reviewer"],
            Some(Duration::minutes(7)),
        );
        let parent_expiry = read_grant(&advisor).unwrap().expires_at;
        {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(advisor.as_str()))]);
            for seat in ["implementer", "reviewer"] {
                assert_eq!(
                    preflight_headless_child(root, seat).unwrap(),
                    HeadlessChildAuthority::Delegated
                );
                let HeadlessChildGrant::Issued(child) =
                    issue_headless_child(root, seat, SUBJECT).unwrap()
                else {
                    panic!("a delegated {seat} seat must mint a child grant");
                };
                assert_ne!(
                    child.id, advisor,
                    "the child never reuses the launcher grant"
                );
                assert_eq!(child.seat, seat);
                assert!(
                    child.delegable_seats.is_empty(),
                    "the child delegates nothing"
                );
                assert_eq!(child.parent_grant_id.as_deref(), Some(advisor.as_str()));
                // trace:TASK-1789 | ai:codex
                assert_eq!(child.expires_at, parent_expiry, "{seat} child expiry");
                assert_eq!(read_grant(&child.id).unwrap().expires_at, parent_expiry);
                // Revocation by id ends exactly the child, never the launcher.
                assert!(revoke_by_id(&child.id).unwrap());
                assert!(!revoke_by_id(&child.id).unwrap(), "idempotent");
                assert!(read_grant(&child.id).unwrap().revoked_at.is_some());
                assert!(read_grant(&advisor).unwrap().revoked_at.is_none());
            }
        }
        // A launcher whose grant does not delegate the seat.
        let narrow = test_support::mint_grant_for(root, SUBJECT, "advisor", &["product"]);
        for grant in [Some(narrow.as_str()), None] {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, grant)]);
            // Baseline seats fall back to an ungranted child...
            for seat in BASELINE_SEATS {
                let got = issue_headless_child(root, seat, SUBJECT).unwrap();
                assert!(
                    matches!(got, HeadlessChildGrant::Baseline { .. }),
                    "{seat} with grant {grant:?}: {got:?}"
                );
                assert_eq!(got.env_value(), "", "a baseline child's grant env is empty");
                assert_eq!(got.id(), None);
            }
            // ...every other seat is refused.
            for seat in ["reviewer", "advisor", "integrator", "product"] {
                if grant.is_some() && seat == "product" {
                    continue; // `product` IS delegated by the narrow grant.
                }
                issue_headless_child(root, seat, SUBJECT)
                    .expect_err("a non-baseline seat must fail closed");
                preflight_headless_child(root, seat)
                    .expect_err("the preflight agrees with issuance");
            }
        }
    }

    /// BUG-2139: the baseline seat list is the one `classify_role_display`
    /// reads, so the statusline and the headless fallback cannot drift.
    // trace:BUG-2139 | ai:claude
    #[test]
    fn baseline_seats_match_role_display() {
        for seat in BASELINE_SEATS {
            assert!(is_baseline_seat(seat));
            assert_eq!(
                classify_role_display(Some(seat), None),
                RoleDisplay::Held((*seat).to_string())
            );
        }
        for seat in ["reviewer", "advisor", "integrator", "product"] {
            assert!(!is_baseline_seat(seat));
        }
    }

    /// A role label with no valid grant reads as ungranted; a validated grant
    /// reads as held; the implementer baseline needs no grant.
    // trace:BUG-1924 | ai:claude
    #[test]
    fn role_display_follows_the_grant_not_the_label() {
        assert_eq!(
            classify_role_display(Some("advisor"), None),
            RoleDisplay::Ungranted("advisor".into())
        );
        assert_eq!(
            classify_role_display(Some("dialog"), None),
            RoleDisplay::Ungranted("advisor".into()),
            "the deprecated alias canonicalizes before display"
        );
        assert_eq!(
            classify_role_display(Some("advisor"), Some("advisor")),
            RoleDisplay::Held("advisor".into())
        );
        assert_eq!(
            classify_role_display(Some("advisor"), Some("reviewer")),
            RoleDisplay::Held("reviewer".into()),
            "the grant, not the label, names the displayed seat"
        );
        assert_eq!(
            classify_role_display(None, Some("reviewer")),
            RoleDisplay::Held("reviewer".into())
        );
        assert_eq!(
            classify_role_display(Some("implementer"), None),
            RoleDisplay::Held("implementer".into())
        );
        assert_eq!(classify_role_display(None, None), RoleDisplay::Default);
        assert_eq!(
            classify_role_display(Some("  "), None),
            RoleDisplay::Default
        );
    }

    /// End to end through the grant store: an env-only advisor label is
    /// ungranted, a minted advisor grant is held, and revocation drops it
    /// back to ungranted.
    // trace:BUG-1924 | ai:claude
    #[test]
    fn role_display_validates_the_grant_store() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut env = pinned_env(); // trace:BUG-2125 | ai:claude
        {
            let _env = Scoped::apply(
                &mut env,
                &[("AIDA_SESSION_ROLE", Some("advisor")), (GRANT_ENV, None)],
            );
            assert_eq!(
                role_display(root, Some("advisor")),
                RoleDisplay::Ungranted("advisor".into())
            );
        }
        let id = test_support::mint_grant_for(root, SUBJECT, "advisor", &[]);
        {
            let _env = Scoped::apply(
                &mut env,
                &[
                    ("AIDA_SESSION_ROLE", Some("advisor")),
                    (GRANT_ENV, Some(id.as_str())),
                ],
            );
            assert_eq!(
                role_display(root, Some("advisor")),
                RoleDisplay::Held("advisor".into())
            );
            assert!(revoke_current().unwrap());
            assert_eq!(
                role_display(root, Some("advisor")),
                RoleDisplay::Ungranted("advisor".into()),
                "a revoked grant reads as ungranted"
            );
        }
    }

    fn expiry_in(seat: &str, left: Duration) -> (GrantExpiry, DateTime<Utc>) {
        let now = Utc::now();
        (
            GrantExpiry {
                seat: seat.to_string(),
                expires_at: now + left,
            },
            now,
        )
    }

    /// A grant near expiry warns and refuses a launch; one with time to
    /// spare does neither; an expired one says it expired.
    // trace:TASK-1736 | ai:claude
    #[test]
    fn expiry_messages_follow_the_time_left() {
        let (fresh, now) = expiry_in("advisor", Duration::hours(3));
        assert_eq!(expiry_warning(&fresh, now, EXPIRY_WARN_WITHIN), None);
        assert_eq!(expiry_suffix(&fresh, now, EXPIRY_WARN_WITHIN), None);
        assert_eq!(launch_refusal(&fresh, now, Duration::hours(1)), None);

        let (soon, now) = expiry_in("advisor", Duration::minutes(90));
        let warning = expiry_warning(&soon, now, EXPIRY_WARN_WITHIN).expect("warns inside 2h");
        assert!(warning.contains("expires at"), "{warning}");
        assert!(warning.contains("(in 1h30m)"), "{warning}");
        assert!(
            warning.contains("eval \"$(aida role enter advisor)\""),
            "{warning}"
        );
        assert_eq!(
            expiry_suffix(&soon, now, EXPIRY_WARN_WITHIN).as_deref(),
            Some("expires in 1h30m")
        );
        assert_eq!(
            launch_refusal(&soon, now, Duration::hours(1)),
            None,
            "90m left is enough to launch"
        );

        let (close, now) = expiry_in("reviewer", Duration::minutes(30));
        let refusal = launch_refusal(&close, now, Duration::hours(1)).expect("refuses under 1h");
        assert!(refusal.starts_with("Not launching"), "{refusal}");
        assert!(refusal.contains("in 30m"), "{refusal}");
        assert!(refusal.contains("aida role enter reviewer"), "{refusal}");

        let (gone, now) = expiry_in("advisor", -Duration::minutes(5));
        let warning = expiry_warning(&gone, now, EXPIRY_WARN_WITHIN).unwrap();
        assert!(warning.contains("expired at"), "{warning}");
        assert_eq!(
            expiry_suffix(&gone, now, EXPIRY_WARN_WITHIN).as_deref(),
            Some("grant expired")
        );
        assert!(launch_refusal(&gone, now, Duration::hours(1))
            .unwrap()
            .contains("expired at"));
    }

    /// A session needs at least the requested window, including exact equality.
    // trace:TASK-1372 | ai:codex
    #[test]
    fn expiry_launch_refusal_allows_exact_window_boundary() {
        let within = Duration::hours(1);
        let (expiry, now) = expiry_in("advisor", within);
        assert_eq!(launch_refusal(&expiry, now, within), None);
        assert_eq!(
            launch_refusal(&expiry, now - Duration::nanoseconds(1), within),
            None,
        );
        assert!(launch_refusal(&expiry, now + Duration::nanoseconds(1), within).is_some());
    }

    /// A set handle naming no usable record is `Unresolved`, never silently
    /// `NoGrant`; the launch refusal names the renew command.
    // trace:TASK-1736 | ai:claude
    #[test]
    fn unresolvable_grant_is_distinguished_from_no_grant() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut env = pinned_env(); // trace:BUG-2125 | ai:claude
        {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some("  "))]);
            assert_eq!(session_grant_lookup(), GrantLookup::NoGrant);
        }
        let missing = Uuid::new_v4().to_string();
        for handle in [missing.as_str(), "not-a-uuid"] {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(handle))]);
            assert!(
                matches!(session_grant_lookup(), GrantLookup::Unresolved(_)),
                "{handle}"
            );
        }
        let id = test_support::mint_grant_for(root, SUBJECT, "advisor", &[]);
        let mut grant = read_grant(&id).unwrap();
        grant.revoked_at = Some(Utc::now());
        write_grant(&grant).unwrap();
        {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
            assert_eq!(
                session_grant_lookup(),
                GrantLookup::Unresolved("session grant was revoked".into())
            );
        }
        let refusal = unresolved_launch_refusal("session grant was revoked", Some("Advisor"));
        assert!(refusal.starts_with("Not launching"), "{refusal}");
        assert!(refusal.contains("aida role enter advisor"), "{refusal}");
        assert!(unresolved_launch_refusal("x", None).contains("aida role enter <seat>"));
    }

    // trace:TASK-1736 | ai:claude
    #[test]
    fn format_remaining_is_compact() {
        assert_eq!(format_remaining(Duration::seconds(20)), "under 1m");
        assert_eq!(format_remaining(Duration::minutes(42)), "42m");
        assert_eq!(format_remaining(Duration::minutes(65)), "1h05m");
        assert_eq!(format_remaining(Duration::hours(2)), "2h00m");
    }

    /// The expiry comes from the raw record, so an expired grant still
    /// reports; a child grant dies with its parent; a revoked grant and the
    /// implementer baseline report nothing.
    // trace:TASK-1736 | ai:claude
    #[test]
    fn session_grant_expiry_reads_the_record_and_its_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut env = pinned_env(); // trace:BUG-2125 | ai:claude
        let rewrite = |id: &str, edit: &dyn Fn(&mut SeatGrant)| {
            let mut grant = read_grant(id).unwrap();
            edit(&mut grant);
            write_grant(&grant).unwrap();
            grant
        };

        {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, None)]);
            assert_eq!(session_grant_expiry(), None, "no grant, nothing to report");
        }

        let id = test_support::mint_grant_for(root, SUBJECT, "advisor", &["reviewer"]);
        let soon = Utc::now() + Duration::minutes(30);
        rewrite(&id, &|g| g.expires_at = soon);
        {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
            assert_eq!(
                session_grant_expiry(),
                Some(GrantExpiry {
                    seat: "advisor".into(),
                    expires_at: soon,
                })
            );
        }

        // A child carrying a later expiry than its parent still dies with it.
        let parent = rewrite(&id, &|g| g.expires_at = soon);
        let mut kid = parent.clone();
        kid.id = Uuid::new_v4().to_string();
        kid.seat = "reviewer".into();
        kid.parent_grant_id = Some(parent.id.clone());
        kid.expires_at = soon + Duration::hours(10);
        write_grant(&kid).unwrap();
        {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(kid.id.as_str()))]);
            assert_eq!(session_grant_expiry().map(|e| e.expires_at), Some(soon));
        }

        // Already expired: current_grant drops it, but the expiry still reports.
        let past = Utc::now() - Duration::minutes(5);
        rewrite(&id, &|g| g.expires_at = past);
        {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
            assert!(current_grant(root).is_none());
            assert_eq!(session_grant_expiry().map(|e| e.expires_at), Some(past));
        }

        rewrite(&id, &|g| g.revoked_at = Some(Utc::now()));
        {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
            assert_eq!(
                session_grant_expiry(),
                None,
                "a revoked grant reports nothing"
            );
        }

        let implementer = test_support::mint_grant_for(root, SUBJECT, "implementer", &[]);
        rewrite(&implementer, &|g| g.expires_at = soon);
        {
            let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(implementer.as_str()))]);
            assert_eq!(
                session_grant_expiry(),
                None,
                "the implementer baseline needs no grant, so its expiry costs nothing"
            );
        }
    }
}

// trace:BUG-2011 | ai:codex
#[cfg(test)]
#[path = "tests/bug_2011_grant_expiry_verdict_tests.rs"]
mod bug_2011_grant_expiry_verdict_tests;

// trace:TASK-1787 | ai:antigravity
#[cfg(test)]
#[path = "tests/task_1787_grant_ttl_tests.rs"]
mod task_1787_grant_ttl_tests;

// trace:TASK-1752 | ai:claude
#[cfg(test)]
#[path = "tests/task_1752_grant_env_guard_tests.rs"]
mod task_1752_grant_env_guard_tests;

// trace:BUG-2200 | ai:claude
#[cfg(test)]
#[path = "tests/bug_2200_roster_store_tests.rs"]
mod bug_2200_roster_store_tests;

// trace:BUG-1934 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1934_grant_binding_tests.rs"]
mod bug_1934_grant_binding_tests;

// trace:BUG-1934 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1934_keyring_tests.rs"]
mod bug_1934_keyring_tests;

// trace:BUG-2209 | ai:claude
#[cfg(test)]
#[path = "tests/bug_2209_roster_leak_tests.rs"]
mod bug_2209_roster_leak_tests;
