//! BUG-049: pre-trust a seat worktree in Claude Code's `~/.claude.json`
//! before a lane's `claude` starts in it.
//!
//! A seat worktree is created fresh at launch, so Claude Code has never seen
//! the path. It then treats the workspace as untrusted and drops the project's
//! `.claude/settings.json` `permissions.allow` list ("Ignoring N
//! permissions.allow entries … this workspace has not been trusted"), and the
//! lane's Bash calls are denied. Adding the trust record by hand only works
//! after the seat exists, which is always too late.
//!
//! This module writes exactly one entry, `projects["<seat>"]`, with
//! `hasTrustDialogAccepted = true` and `"aida"` in `enabledMcpjsonServers`,
//! and removes it again when the seat is torn down so the file does not grow
//! without bound. Every other key in the file is left as it was.
//!
//! Guard rails:
//! - opt-in: `[worktree_pool] trust_seat_worktrees = true` in
//!   `.aida/config.toml`, or `AIDA_TRUST_SEAT_WORKTREES=1|0` to turn it on or
//!   off on one host (the variable wins);
//! - only a directory directly inside the configured worktrees root (never
//!   the main checkout, never anything else);
//! - the read-modify-write runs under `~/.claude.json.lock` (the lock
//!   directory Claude Code's own writer uses) and replaces the file with an
//!   atomic rename that keeps its permissions. An unparseable file is never
//!   overwritten.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// The `.aida/config.toml` key (under `[worktree_pool]`) that turns the write on.
pub(crate) const CONFIG_KEY: &str = "trust_seat_worktrees";
/// The host override: `1`/`true`/`yes`/`on` enables, `0`/`false`/`no`/`off` disables.
pub(crate) const ENV_OVERRIDE: &str = "AIDA_TRUST_SEAT_WORKTREES";
/// The MCP server a trusted seat is pre-approved for.
const MCP_SERVER: &str = "aida";
/// How long to wait for the `~/.claude.json` lock before giving up.
const LOCK_WAIT: Duration = Duration::from_secs(5);
/// A lock directory older than this is a crashed writer's (the same rule as
/// the `proper-lockfile` convention Claude Code uses).
const LOCK_STALE: Duration = Duration::from_secs(10);

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// What [`ensure_seat_trusted`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrustWrite {
    /// The entry was added or completed.
    Written(PathBuf),
    /// The entry already carried both fields; the file was not rewritten.
    AlreadyTrusted(PathBuf),
    /// The opt-in is off for this project / host.
    Disabled,
    /// The path is not a seat under the worktrees root.
    Refused(String),
}

/// Is the write on? The host override wins over the project config; the
/// default is off.
pub(crate) fn trust_enabled(main: &Path) -> bool {
    std::env::var_os(ENV_OVERRIDE)
        .and_then(|raw| parse_override(&raw.to_string_lossy()))
        .unwrap_or_else(|| config_enabled(main))
}

/// The host override's value; `None` (fall back to the config) when unset or
/// unrecognised.
pub(crate) fn parse_override(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn config_enabled(main: &Path) -> bool {
    let Ok(body) = std::fs::read_to_string(main.join(".aida").join("config.toml")) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return false;
    };
    value
        .get("worktree_pool")
        .and_then(|t| t.get(CONFIG_KEY))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// The directory seat worktrees are created in: the configured
/// `[worktree_pool] worktree_parent` (relative resolved against `main`), else
/// `main`'s own parent — the same rule the pool uses.
pub(crate) fn worktrees_root(main: &Path) -> PathBuf {
    let root = match crate::worktree_pool_config_worktree_parent(main) {
        Some(p) if p.is_absolute() => p,
        Some(p) => main.join(p),
        None => main
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| main.to_path_buf()),
    };
    canonical_or_lexical(&root)
}

fn canonical_or_lexical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| aida_core::worktree_pool::normalize_path_lexically(path))
}

/// The `projects` key for `seat`, or why it is refused: the seat must sit
/// directly inside the worktrees root and must not be the main checkout.
pub(crate) fn seat_trust_key(main: &Path, seat: &Path) -> Result<PathBuf, String> {
    let root = worktrees_root(main);
    let seat = canonical_or_lexical(seat);
    let main = canonical_or_lexical(main);
    if seat == main {
        return Err(format!(
            "{} is the main checkout, not a seat",
            seat.display()
        ));
    }
    if seat.parent() != Some(root.as_path()) {
        return Err(format!(
            "{} is not a seat directly inside the worktrees root {}",
            seat.display(),
            root.display()
        ));
    }
    Ok(seat)
}

/// The main checkout `seat` belongs to (a seat clone's verified main, or a
/// git worktree's main worktree).
fn main_for_seat(seat: &Path) -> PathBuf {
    crate::agent_launch_project_root_from(seat)
}

/// Pre-trust `seat` in the operator's `~/.claude.json`. Best-effort: a
/// failure is returned, and the caller warns and launches anyway (the lane
/// is then no worse off than before).
pub(crate) fn ensure_seat_trusted(seat: &Path) -> Result<TrustWrite> {
    let Some(home) = crate::home_dir() else {
        bail!("no home directory, so ~/.claude.json cannot be located");
    };
    ensure_seat_trusted_in(&home, &main_for_seat(seat), seat)
}

/// [`ensure_seat_trusted`] with the home and main checkout given — the
/// testable core.
pub(crate) fn ensure_seat_trusted_in(home: &Path, main: &Path, seat: &Path) -> Result<TrustWrite> {
    if !trust_enabled(main) {
        return Ok(TrustWrite::Disabled);
    }
    let key = match seat_trust_key(main, seat) {
        Ok(key) => key,
        Err(why) => return Ok(TrustWrite::Refused(why)),
    };
    let key_str = key.to_string_lossy().into_owned();
    let changed = edit_claude_json(home, |root| {
        let projects = root
            .entry("projects")
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        let Some(projects) = projects.as_object_mut() else {
            bail!("~/.claude.json `projects` is not an object");
        };
        let entry = projects
            .entry(key_str.clone())
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        let Some(entry) = entry.as_object_mut() else {
            bail!("~/.claude.json `projects[{key_str}]` is not an object");
        };
        let mut changed = false;
        if entry.get("hasTrustDialogAccepted") != Some(&serde_json::Value::Bool(true)) {
            entry.insert("hasTrustDialogAccepted".into(), true.into());
            changed = true;
        }
        let servers = entry
            .entry("enabledMcpjsonServers")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        if !servers.is_array() {
            *servers = serde_json::Value::Array(Vec::new());
        }
        let servers = servers.as_array_mut().expect("just made an array");
        if !servers.iter().any(|s| s.as_str() == Some(MCP_SERVER)) {
            servers.push(MCP_SERVER.into());
            changed = true;
        }
        Ok(changed)
    })?;
    Ok(if changed {
        TrustWrite::Written(key)
    } else {
        TrustWrite::AlreadyTrusted(key)
    })
}

/// Remove `seat`'s entry when the seat is torn down. Runs whether or not the
/// opt-in is on (an entry written while it was on must not outlive the seat),
/// but only for a path inside the worktrees root. Returns whether an entry was
/// removed; errors are swallowed by the caller (teardown must not fail on it).
pub(crate) fn remove_seat_trust(main: &Path, seat: &Path) -> Result<bool> {
    let Some(home) = crate::home_dir() else {
        return Ok(false);
    };
    remove_seat_trust_in(&home, main, seat)
}

/// [`remove_seat_trust`] with the home given.
pub(crate) fn remove_seat_trust_in(home: &Path, main: &Path, seat: &Path) -> Result<bool> {
    let Ok(key) = seat_trust_key(main, seat) else {
        return Ok(false);
    };
    if !home.join(".claude.json").exists() {
        return Ok(false);
    }
    let key_str = key.to_string_lossy().into_owned();
    edit_claude_json(home, |root| {
        Ok(root
            .get_mut("projects")
            .and_then(serde_json::Value::as_object_mut)
            .is_some_and(|projects| projects.remove(&key_str).is_some()))
    })
}

/// `projects` keys that are seats of `main` (directly inside its worktrees
/// root) whose directory no longer exists — what doctor reports.
pub(crate) fn stale_seat_entries(home: &Path, main: &Path) -> Vec<PathBuf> {
    let Ok(body) = std::fs::read_to_string(home.join(".claude.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Vec::new();
    };
    let root = worktrees_root(main);
    let prefix = aida_core::seat_clone_detect::CLONE_DIR_PREFIX;
    value
        .get("projects")
        .and_then(serde_json::Value::as_object)
        .map(|projects| {
            projects
                .keys()
                .map(PathBuf::from)
                .filter(|p| {
                    p.parent() == Some(root.as_path())
                        && p.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with(prefix))
                        && !p.exists()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Locked read-modify-write of `<home>/.claude.json`. `edit` returns whether
/// it changed anything; the file is rewritten only then.
fn edit_claude_json(
    home: &Path,
    edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<bool>,
) -> Result<bool> {
    let path = home.join(".claude.json");
    let _lock = LockDir::acquire(&home.join(".claude.json.lock"))?;
    let (mut value, mode) = match std::fs::read_to_string(&path) {
        Ok(body) => {
            let value: serde_json::Value = serde_json::from_str(&body).with_context(|| {
                format!(
                    "{} is not valid JSON; it was left untouched",
                    path.display()
                )
            })?;
            (value, file_mode(&path))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (serde_json::Value::Object(Default::default()), None)
        }
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let Some(root) = value.as_object_mut() else {
        bail!(
            "{} is not a JSON object; it was left untouched",
            path.display()
        );
    };
    if !edit(root)? {
        return Ok(false);
    }
    let body = serde_json::to_string_pretty(&value)?;
    write_replacing(&path, body.as_bytes(), mode)?;
    Ok(true)
}

#[cfg(unix)]
fn file_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).ok().map(|m| m.permissions().mode())
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> Option<u32> {
    None
}

/// Stage in a sibling temp file with the original's permissions (default
/// 0600: the file holds account state), then rename over the target.
fn write_replacing(path: &Path, body: &[u8], mode: Option<u32>) -> Result<()> {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{file_name}.aida-tmp.{}.{seq}", std::process::id()));
    let result = (|| -> Result<()> {
        std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &tmp,
                std::fs::Permissions::from_mode(mode.unwrap_or(0o600) & 0o7777),
            )?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// A `mkdir` lock: creating the directory is the atomic acquire; dropping
/// removes it.
struct LockDir(PathBuf);

impl LockDir {
    fn acquire(path: &Path) -> Result<Self> {
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match std::fs::create_dir(path) {
                Ok(()) => return Ok(Self(path.to_path_buf())),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(path) {
                        let _ = std::fs::remove_dir(path);
                        continue;
                    }
                    if Instant::now() >= deadline {
                        bail!(
                            "{} is held by another writer; ~/.claude.json was not changed",
                            path.display()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e).with_context(|| format!("create lock {}", path.display())),
            }
        }
    }
}

fn lock_is_stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age > LOCK_STALE)
}

impl Drop for LockDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// Print one line about a pre-launch trust write. Silent when the opt-in is
/// off, nothing changed, or the launch is not in a seat (a reviewer in the
/// main checkout, say) — only a write or a failure is news.
pub(crate) fn report(outcome: &Result<TrustWrite>) {
    match outcome {
        Ok(TrustWrite::Written(key)) => {
            eprintln!("Trusted {} for Claude Code before launch", key.display())
        }
        Ok(TrustWrite::AlreadyTrusted(_) | TrustWrite::Disabled | TrustWrite::Refused(_)) => {}
        Err(error) => eprintln!(
            "warning: could not pre-trust the workspace for Claude Code ({error:#}); \
             the session may ignore the project's allowed commands"
        ),
    }
}

#[cfg(test)]
#[path = "tests/bug_b6b_049_claude_trust_tests.rs"]
mod tests;
