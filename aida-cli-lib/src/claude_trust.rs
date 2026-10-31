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
//! for the one directory a claude lane is spawned in, at the spawn. Growth is
//! bounded by design rather than by wiring every teardown path: when
//! `[worktree_pool] worktree_parent` is configured, every write also prunes
//! the `projects` entries directly under that root whose directory no longer
//! exists, so a tree torn down by any path loses its entry at the next write.
//! Dismissal removes the entry at once as a fast path. Entries outside the
//! worktrees root, and every other key, keep their values (serde_json
//! re-sorts object keys; Claude Code does not depend on key order).
//!
//! Guard rails:
//! - opt-in: `[worktree_pool] trust_seat_worktrees = true` in the default
//!   branch's committed `.aida/config.toml` (never the branch-local copy), or
//!   `AIDA_TRUST_SEAT_WORKTREES=1|0` to turn it on or off on one host (the
//!   variable wins). Off means no writes at all: no trust, no removal, no
//!   prune;
//! - only an existing directory directly inside the worktrees root (never the
//!   main checkout, never anything else);
//! - only a tree that runs the lane's own work: an implementer seat clone
//!   (`aida-seat-*`) or a compete lane (`aida-compete-*`). A reviewer tree
//!   (`aida-pr-N`, `take-N`, `pr-N`, or any name carrying a PR number) is
//!   never trusted: trust makes Claude Code apply the checked-out tree's own
//!   `.claude/settings.json` allow rules and hooks, so trusting code under
//!   review would let a PR configure the session reviewing it;
//! - the prune only ever runs under an explicitly configured
//!   `worktree_parent`: the fallback root (the main checkout's parent) is
//!   typically a directory of unrelated projects;
//! - the read-modify-write runs under an OS advisory lock (`flock`) on the
//!   sidecar `~/.claude.json.aida.lock`, waited on with a timeout, so two AIDA
//!   writers never edit at once; the file is replaced with an atomic rename
//!   that keeps its permissions. An unparseable file is never overwritten.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The `.aida/config.toml` key (under `[worktree_pool]`) that turns the write on.
pub(crate) const CONFIG_KEY: &str = "trust_seat_worktrees";
/// The host override: `1`/`true`/`yes`/`on` enables, `0`/`false`/`no`/`off` disables.
pub(crate) const ENV_OVERRIDE: &str = "AIDA_TRUST_SEAT_WORKTREES";
/// The MCP server a trusted seat is pre-approved for.
const MCP_SERVER: &str = "aida";
/// How long to wait for the `~/.claude.json` lock before giving up.
const LOCK_WAIT: Duration = Duration::from_secs(5);
/// The sidecar file AIDA's writers `flock` (inside the home directory).
pub(crate) const LOCK_FILE: &str = ".claude.json.aida.lock";

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// What [`ensure_seat_trusted`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrustWrite {
    /// The entry was added or completed.
    Written(PathBuf),
    /// The entry already carried both fields; the file was not rewritten
    /// (unless stale entries were pruned).
    AlreadyTrusted(PathBuf),
    /// The opt-in is off for this project / host.
    Disabled,
    /// The path is not an existing seat under the worktrees root.
    Refused(String),
}

/// Is the write on? The host override wins over the project config; the
/// default is off.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac891c92 | ai:claude
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

/// The project opt-in, read from the TRUSTED default-branch copy of the
/// config (BUG-1723): trusting a tree makes Claude Code honour that tree's own
/// `.claude/settings.json` allow-list, so a branch must not be able to switch
/// the write on for itself. Local refs only (this runs on every launch);
/// fail-closed (off) when no default branch or config resolves.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac891c92 | ai:claude
fn config_enabled(main: &Path) -> bool {
    let Some(body) = crate::trusted_config::read_trusted_config_toml_local(main) else {
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
/// (Existence is checked on the write side only: a dismissal runs after the
/// directory is gone.)
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

/// The directory-name prefixes of the trees a pre-trust may be written for:
/// trees that run the lane's own work.
pub(crate) const TRUSTABLE_PREFIXES: &[&str] = &[
    aida_core::seat_clone_detect::CLONE_DIR_PREFIX,
    "aida-compete-",
];

/// May `seat` be pre-trusted? Only an implementer seat clone or a compete
/// lane — never a tree that checks out code under review (whose own
/// `.claude/settings.json` trust would then apply to its reviewer). Any name
/// carrying a PR number (`aida-pr-N`, `take-N`, `pr-N`, `…-pr-N…`) is refused
/// even under an allowed prefix. Pure.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude
pub(crate) fn is_trustable_tree(seat: &Path) -> bool {
    let Some(name) = seat.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    TRUSTABLE_PREFIXES.iter().any(|p| name.starts_with(p)) && !names_a_pr(name)
}

/// Does a directory name carry a PR number (`pr-<digits>` as a whole
/// `-`-separated segment pair, or a review-tree prefix)?
fn names_a_pr(name: &str) -> bool {
    if crate::worktree_reclaim::pr_number_from_dir(name).is_some() {
        return true;
    }
    let segments: Vec<&str> = name.split('-').collect();
    segments.windows(2).any(|w| {
        w[0].eq_ignore_ascii_case("pr")
            && !w[1].is_empty()
            && w[1].bytes().all(|b| b.is_ascii_digit())
    })
}

/// The worktrees root when it is EXPLICITLY configured
/// (`[worktree_pool] worktree_parent`), else `None`. The prune and the stale
/// scan only ever run under this root.
fn explicit_worktrees_root(main: &Path) -> Option<PathBuf> {
    crate::worktree_pool_config_worktree_parent(main).map(|_| worktrees_root(main))
}

/// The main checkout `seat` belongs to (a seat clone's verified main, or a
/// git worktree's main worktree).
fn main_for_seat(seat: &Path) -> PathBuf {
    crate::agent_launch_project_root_from(seat)
}

/// Pre-trust `seat` — the directory a claude lane is about to be spawned in —
/// in the operator's `~/.claude.json`. Called only at the spawn points (the
/// headless launcher and `agent new --bg`). Best-effort: a failure is
/// returned, and the caller warns and launches anyway (the lane is then no
/// worse off than before).
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.acaf7e35 | ai:claude
pub(crate) fn ensure_seat_trusted(seat: &Path) -> Result<TrustWrite> {
    let Some(home) = crate::home_dir() else {
        bail!("no home directory, so ~/.claude.json cannot be located");
    };
    ensure_seat_trusted_in(&home, &main_for_seat(seat), seat)
}

/// [`ensure_seat_trusted`] for a launch whose role is known (`agent new
/// --bg`): only an implementer lane (or one launched without a role) is
/// trusted; a reviewer, advisor or any other role reads code it did not
/// write, so its tree is never trusted whatever its name.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude
pub(crate) fn ensure_role_seat_trusted(seat: &Path, role: Option<&str>) -> Result<TrustWrite> {
    if let Some(refused) = refuse_role(role) {
        return Ok(refused);
    }
    ensure_seat_trusted(seat)
}

/// The refusal for a role that must not get a trusted tree; `None` when the
/// role may (implementer, or no role). Pure.
pub(crate) fn refuse_role(role: Option<&str>) -> Option<TrustWrite> {
    match role {
        None | Some("implementer") => None,
        Some(other) => Some(TrustWrite::Refused(format!(
            "a `{other}` launch is not an implementer lane; only implementer seats are trusted"
        ))),
    }
}

/// [`ensure_seat_trusted`] with the home and main checkout given — the
/// testable core.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.acaf7e35 | ai:claude
pub(crate) fn ensure_seat_trusted_in(home: &Path, main: &Path, seat: &Path) -> Result<TrustWrite> {
    if !trust_enabled(main) {
        return Ok(TrustWrite::Disabled);
    }
    let key = match seat_trust_key(main, seat) {
        Ok(key) => key,
        Err(why) => return Ok(TrustWrite::Refused(why)),
    };
    if !key.is_dir() {
        return Ok(TrustWrite::Refused(format!(
            "{} is not an existing directory a lane is spawned in",
            key.display()
        )));
    }
    if !is_trustable_tree(&key) {
        return Ok(TrustWrite::Refused(format!(
            "{} is not an implementer seat or compete lane (review trees are never trusted)",
            key.display()
        )));
    }
    let key_str = key.to_string_lossy().into_owned();
    let changed = edit_claude_json(home, main, |root| {
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

/// Remove `seat`'s entry when the seat is torn down — the fast path; the
/// prune on every write is what bounds growth. A no-op when the opt-in is off
/// (off means AIDA writes nothing to `~/.claude.json`; an entry left from
/// while it was on is reported by doctor), and only for a path inside the
/// worktrees root. Returns whether an entry was removed; errors are swallowed
/// by the caller (teardown must not fail on it).
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac15c23c | ai:claude
pub(crate) fn remove_seat_trust(main: &Path, seat: &Path) -> Result<bool> {
    let Some(home) = crate::home_dir() else {
        return Ok(false);
    };
    remove_seat_trust_in(&home, main, seat)
}

/// [`remove_seat_trust`] with the home given.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac15c23c | ai:claude
pub(crate) fn remove_seat_trust_in(home: &Path, main: &Path, seat: &Path) -> Result<bool> {
    if !trust_enabled(main) {
        return Ok(false);
    }
    let Ok(key) = seat_trust_key(main, seat) else {
        return Ok(false);
    };
    if !home.join(".claude.json").exists() {
        return Ok(false);
    }
    let key_str = key.to_string_lossy().into_owned();
    edit_claude_json(home, main, |root| {
        Ok(root
            .get_mut("projects")
            .and_then(serde_json::Value::as_object_mut)
            .is_some_and(|projects| projects.remove(&key_str).is_some()))
    })
}

/// `projects` keys directly inside `main`'s explicitly configured worktrees
/// root (seat clones, spec worktrees, reviewer and compete worktrees alike)
/// whose directory no longer exists — what doctor reports and what every
/// write prunes. Empty without a configured `worktree_parent`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac891c92 | ai:claude
pub(crate) fn stale_seat_entries(home: &Path, main: &Path) -> Vec<PathBuf> {
    let Ok(body) = std::fs::read_to_string(home.join(".claude.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Vec::new();
    };
    let Some(root) = explicit_worktrees_root(main) else {
        return Vec::new();
    };
    value
        .get("projects")
        .and_then(serde_json::Value::as_object)
        .map(|projects| {
            projects
                .keys()
                .map(PathBuf::from)
                .filter(|p| is_stale_under(p, &root))
                .collect()
        })
        .unwrap_or_default()
}

/// A `projects` key directly inside the worktrees `root` whose directory is
/// gone. Anything else (outside the root, nested, live) is never touched.
fn is_stale_under(key: &Path, root: &Path) -> bool {
    key.parent() == Some(root) && !key.exists()
}

/// Drop every stale entry under the worktrees `root`; returns how many.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac15c23c | ai:claude
fn prune_stale(file: &mut serde_json::Map<String, serde_json::Value>, root: &Path) -> usize {
    let Some(projects) = file
        .get_mut("projects")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return 0;
    };
    let before = projects.len();
    projects.retain(|key, _| !is_stale_under(Path::new(key), root));
    before - projects.len()
}

/// Locked read-modify-write of `<home>/.claude.json`. `edit` returns whether
/// it changed anything; every write also prunes the stale entries under
/// `main`'s explicitly configured worktrees root (none without one). The file is rewritten only when either changed
/// something. Returns `edit`'s answer.
fn edit_claude_json(
    home: &Path,
    main: &Path,
    edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<bool>,
) -> Result<bool> {
    let path = home.join(".claude.json");
    let wt_root = explicit_worktrees_root(main);
    let _lock = ClaudeJsonLock::acquire(home, LOCK_WAIT)?;
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
    let changed = edit(root)?;
    let pruned = wt_root
        .as_deref()
        .map_or(0, |wt_root| prune_stale(root, wt_root));
    if !changed && pruned == 0 {
        return Ok(false);
    }
    let body = serde_json::to_string_pretty(&value)?;
    write_replacing(&path, body.as_bytes(), mode)?;
    Ok(changed)
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

/// An exclusive `flock` on the sidecar [`LOCK_FILE`], held until drop. The
/// kernel releases it when the holder dies, so there is no stale lock to
/// break (and so no break race). The sidecar file stays in place: deleting a
/// lock file another writer has open would let two writers each hold "the"
/// lock.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.acaf7e35 | ai:claude
pub(crate) struct ClaudeJsonLock(std::fs::File);

impl ClaudeJsonLock {
    /// Take the lock in `home`, waiting at most `wait`.
    pub(crate) fn acquire(home: &Path, wait: Duration) -> Result<Self> {
        use fs2::FileExt;
        let path = home.join(LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open lock {}", path.display()))?;
        let deadline = Instant::now() + wait;
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(Self(file)),
                Err(e) if e.kind() == fs2::lock_contended_error().kind() => {
                    if Instant::now() >= deadline {
                        bail!(
                            "{} is held by another writer; ~/.claude.json was not changed",
                            path.display()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => return Err(e).with_context(|| format!("lock {}", path.display())),
            }
        }
    }
}

impl Drop for ClaudeJsonLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
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
