//! A relay run's launch record and restart lineage (STORY-1706).
//!
//! `bin/relay.sh` writes `<run dir>/relay.launch` (0600) when a run takes its
//! lock: how it was started, by whom, and whether a human started it at a
//! terminal. The drive tick restarts a crashed run (its EXIT trap never ran)
//! only from this record, never from a grant: the restarted relay's argv is
//! the record's `arg=` lines plus `--max-sessions` (advisor condition C1).
//! The tick appends one line per restart to `<run dir>/relay.restarts`, so
//! the 3-restart cap is per launch lineage and survives the tick (C2).
//!
//! Every reader fails closed: a missing, symlinked, non-0600, foreign-owned,
//! unknown-version or malformed record means no restart. Its trust is the
//! user's other same-uid AIDA state (mandate records, session grants): a
//! same-uid process that forges files is outside the threat model.
// trace:STORY-1706 | ai:claude
// mandate-gate

use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

pub(crate) const RECORD_FILE: &str = "relay.launch";
pub(crate) const RESTARTS_FILE: &str = "relay.restarts";
pub(crate) const NORESTART_FILE: &str = "relay.norestart";
/// Set on a restart to the record's sha256, so relay.sh keeps the record.
pub(crate) const RESTART_ENV: &str = "AIDA_RELAY_RESTART";
pub(crate) const RESTART_LOG: &str = "relay.restart.log";
/// Restarts allowed per launch lineage.
pub(crate) const MAX_RESTARTS: usize = 3;
const VERSION: &str = "1";

/// One parsed `relay.launch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchRecord {
    /// sha256 of the record's bytes: the lineage key.
    pub sha256: String,
    pub run: String,
    pub launched_at: String,
    pub launch_cwd: PathBuf,
    pub project_root: PathBuf,
    pub relay_sh: PathBuf,
    pub agent: String,
    pub session_role: String,
    pub force_seat: bool,
    pub max_sessions: u32,
    pub start_session: u32,
    pub profile_ref: String,
    pub profile_file: Option<PathBuf>,
    pub profile_sha256: Option<String>,
    pub principal: String,
    pub launched_from_tty: bool,
    /// The spec the run works, as the launch named it (`--spec` or the
    /// profile's `spec` key); never guessed. Unvalidated: the drive tick
    /// counts it only as an exact id the store knows.
    // trace:STORY-1706 | ai:claude
    pub spec: Option<String>,
    /// The argv a restart runs, in order.
    pub args: Vec<String>,
}

/// One `relay.restarts` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestartLine {
    pub generation: u32,
    pub at: DateTime<Utc>,
    /// The launch record sha this restart continued.
    pub launch: String,
    pub mandate: String,
    /// The `--max-sessions` the restart passed.
    pub max_sessions: u32,
    /// Sessions the lineage had used before this restart.
    pub used_before: u32,
}

impl RestartLine {
    pub(crate) fn to_line(&self) -> String {
        format!(
            "gen={} at={} launch={} mandate={} max_sessions={} used_before={}\n",
            self.generation,
            self.at.to_rfc3339(),
            self.launch,
            self.mandate,
            self.max_sessions,
            self.used_before
        )
    }

    fn parse(line: &str) -> Option<Self> {
        let mut fields = std::collections::BTreeMap::new();
        for part in line.split_whitespace() {
            let (k, v) = part.split_once('=')?;
            if fields.insert(k, v).is_some() {
                return None;
            }
        }
        Some(Self {
            generation: fields.get("gen")?.parse().ok()?,
            at: DateTime::parse_from_rfc3339(fields.get("at")?)
                .ok()?
                .with_timezone(&Utc),
            launch: fields.get("launch")?.to_string(),
            mandate: fields.get("mandate")?.to_string(),
            max_sessions: fields.get("max_sessions")?.parse().ok()?,
            used_before: fields.get("used_before")?.parse().ok()?,
        })
    }
}

/// Undo relay.sh's escaping (`%25`, `%0A`, `%0D`). Any other `%` sequence is
/// not something relay.sh writes, so the value is refused.
pub(crate) fn decode(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let esc = rest.get(i + 1..i + 3)?;
        out.push(match esc {
            "25" => '%',
            "0A" => '\n',
            "0D" => '\r',
            _ => return None,
        });
        rest = &rest[i + 3..];
    }
    out.push_str(rest);
    Some(out)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The sha256 of a file's bytes, or `None` when it cannot be read.
pub(crate) fn sha256_file(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|b| sha256_hex(&b))
}

/// Open a private file without following a symlink and check what was
/// opened: a regular file, mode 0600, owned by this user.
fn open_private(path: &Path) -> Result<std::fs::File, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| format!("{} unreadable ({e})", path.display()))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("{} unreadable ({e})", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let mode = meta.mode() & 0o7777;
    if mode != 0o600 {
        return Err(format!("{} has mode {mode:o}, not 600", path.display()));
    }
    // SAFETY: geteuid takes no arguments and cannot fail.
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        return Err(format!(
            "{} is owned by uid {}, not {uid}",
            path.display(),
            meta.uid()
        ));
    }
    Ok(file)
}

/// Read and validate `<run_dir>/relay.launch`.
pub(crate) fn load(run_dir: &Path) -> Result<LaunchRecord, String> {
    let path = run_dir.join(RECORD_FILE);
    let mut bytes = Vec::new();
    open_private(&path)?
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{} unreadable ({e})", path.display()))?;
    parse(&bytes).map_err(|why| format!("{}: {why}", path.display()))
}

/// Parse a record's bytes. Pure.
pub(crate) fn parse(bytes: &[u8]) -> Result<LaunchRecord, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8".to_string())?;
    let mut scalars = std::collections::BTreeMap::<&str, String>::new();
    let mut args = Vec::new();
    for line in text.lines() {
        let Some((key, raw)) = line.split_once('=') else {
            return Err(format!("malformed line `{line}`"));
        };
        let value = decode(raw).ok_or_else(|| format!("bad escape in `{key}`"))?;
        match key {
            "arg" => args.push(value),
            // Provenance only (the verbatim launch argv); never executed.
            "orig_arg" => {}
            _ => {
                if scalars.insert(key, value).is_some() {
                    return Err(format!("`{key}` appears twice"));
                }
            }
        }
    }
    let get = |key: &str| {
        scalars
            .get(key)
            .cloned()
            .ok_or_else(|| format!("missing `{key}`"))
    };
    let flag = |key: &str| match get(key)?.as_str() {
        "0" => Ok(false),
        "1" => Ok(true),
        other => Err(format!("`{key}` is `{other}`, not 0 or 1")),
    };
    let number = |key: &str| {
        get(key)?
            .parse::<u32>()
            .map_err(|_| format!("`{key}` is not a number"))
    };
    let absolute = |key: &str| {
        let p = PathBuf::from(get(key)?);
        if p.is_absolute() {
            Ok(p)
        } else {
            Err(format!("`{key}` is not an absolute path"))
        }
    };
    let v = get("v")?;
    if v != VERSION {
        return Err(format!("unknown record version `{v}`"));
    }
    let optional = |key: &str| get(key).map(|v| Some(v).filter(|v| !v.is_empty()));
    let record = LaunchRecord {
        sha256: sha256_hex(bytes),
        run: get("run")?,
        launched_at: get("launched_at")?,
        launch_cwd: absolute("launch_cwd")?,
        project_root: absolute("project_root")?,
        relay_sh: absolute("relay_sh")?,
        agent: get("agent")?,
        session_role: get("session_role")?,
        force_seat: flag("force_seat")?,
        max_sessions: number("max_sessions")?,
        start_session: number("start_session")?,
        profile_ref: get("profile_ref")?,
        profile_file: optional("profile_file")?.map(PathBuf::from),
        profile_sha256: optional("profile_sha256")?,
        principal: get("principal")?,
        launched_from_tty: flag("launched_from_tty")?,
        // Optional: records written before the field existed lack it.
        spec: scalars.get("spec").cloned().filter(|v| !v.is_empty()),
        args,
    };
    if record.run.is_empty() || record.principal.trim().is_empty() {
        return Err("empty `run` or `principal`".into());
    }
    if record.start_session == 0 {
        return Err("`start_session` is 0".into());
    }
    if record.args.is_empty() {
        return Err("no `arg` lines".into());
    }
    if record.profile_file.is_some() != record.profile_sha256.is_some() {
        return Err("a profile without its sha256 (or the reverse)".into());
    }
    Ok(record)
}

/// The restarts of the lineage `launch_sha`, oldest first. An absent file is
/// none; an unreadable file or a malformed line refuses (fail closed).
pub(crate) fn restarts(run_dir: &Path, launch_sha: &str) -> Result<Vec<RestartLine>, String> {
    let path = run_dir.join(RESTARTS_FILE);
    if path.symlink_metadata().is_err() {
        return Ok(Vec::new());
    }
    let mut text = String::new();
    open_private(&path)?
        .read_to_string(&mut text)
        .map_err(|e| format!("{} unreadable ({e})", path.display()))?;
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed = RestartLine::parse(line)
            .ok_or_else(|| format!("{} line {} is malformed", path.display(), n + 1))?;
        if parsed.launch == launch_sha {
            out.push(parsed);
        }
    }
    Ok(out)
}

/// Append one restart line (0600, never through a symlink), synced before
/// the tick execs anything.
pub(crate) fn append_restart(run_dir: &Path, line: &RestartLine) -> Result<(), String> {
    let path = run_dir.join(RESTARTS_FILE);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|e| format!("cannot open {} ({e})", path.display()))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("{} unreadable ({e})", path.display()))?;
    if !meta.is_file() || meta.mode() & 0o7777 != 0o600 {
        return Err(format!("{} is not a private regular file", path.display()));
    }
    file.write_all(line.to_line().as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("cannot write {} ({e})", path.display()))
}

#[cfg(test)]
#[path = "tests/story_1706_relay_launch_tests.rs"]
mod story_1706_relay_launch_tests;
