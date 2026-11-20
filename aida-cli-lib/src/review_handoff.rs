//! Launch receipts for reviewer draft handoffs.
//!
//! Before a launcher spawns a headless reviewer it registers the draft slot
//! here, in the user's AIDA home beside the grant store, and seals the
//! receipt with the draft's digest once the reviewer has exited and the
//! draft validated. A receipt holds no secret and grants nothing: it is the
//! durable record that a launcher holding reviewer delegation registered
//! this slot for this PR, head and reviewer session. Recording a draft from
//! a receipt is a separate, later step; nothing in this module records.
//!
//! A version-1 receipt (the only version written so far) is never eligible
//! for automatic recovery: see [`VERSION`].
//! Design: `docs/plans/2026-11-20-bug-2669-retained-draft-recovery.md`.
// trace:BUG-2669 | ai:claude
use crate::gate::seat_authority as sa;
use crate::review_draft::LaunchDraft;
use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const RECEIPT_DIR: &str = "review-handoffs";
const LOCK_DIR: &str = "review-handoff-locks";
/// The receipt format this build writes and reads. Version 1 receipts are
/// written by launchers that do not yet supersede the earlier drafts of a
/// review checkout, so nothing protects an unsealed version-1 draft from a
/// later vendor session launched there. They are therefore ineligible for
/// automatic recovery whatever else they say; only a later version, written
/// once supersession exists, can be eligible.
const VERSION: u32 = 1;
/// The first receipt version automatic recovery may act on.
const FIRST_AUTO_RECOVERY_VERSION: u32 = 2;
const RECEIPT_LIMIT: u64 = 64 * 1024;
/// How long one seal or close waits for its own receipt's lock.
const LOCK_WAIT: Duration = Duration::from_secs(2);
/// Receipt maintenance examines at most this many receipts per run, within
/// [`SWEEP_BUDGET`], and never waits for a lock.
const SWEEP_BATCH: usize = 16;
const SWEEP_BUDGET: Duration = Duration::from_millis(200);
/// How long a receipt is kept once nothing waits on it.
const RETENTION: chrono::Duration = chrono::Duration::days(30);
/// A draft older than this is outside the automatic recovery window.
pub(crate) const AUTO_RECOVERY_WINDOW: chrono::Duration = chrono::Duration::hours(24);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum State {
    Open,
    Consumed,
    Superseded,
    Discarded,
    /// The launch's verdict was collected without this draft slot: the
    /// launcher found no draft to read. Nothing was recorded from a draft.
    Unused,
}

impl State {
    fn label(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Consumed => "consumed",
            State::Superseded => "superseded",
            State::Discarded => "discarded",
            State::Unused => "unused",
        }
    }
}

/// The digest the launcher read and validated before its first recording
/// attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Seal {
    pub(crate) sha256: String,
    pub(crate) sealed_at: DateTime<Utc>,
}

/// A closed schema: an unknown field makes the receipt unreadable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub(crate) version: u32,
    /// The draft directory name, `draft-<uuid>`: the handoff stamp's launch.
    pub(crate) launch: String,
    pub(crate) project_root: PathBuf,
    pub(crate) worktree: PathBuf,
    pub(crate) dir_dev: u64,
    pub(crate) dir_ino: u64,
    pub(crate) pr: u64,
    pub(crate) head: String,
    pub(crate) reviewer_session: String,
    pub(crate) launcher_grant: String,
    pub(crate) principal: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) state: State,
    /// Evidence only: known once the spawn returns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) child_grant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) seal: Option<Seal>,
}

fn aida_dir() -> Result<PathBuf> {
    Ok(sa::grant_dir()?
        .parent()
        .context("session grant store has no parent directory")?
        .to_path_buf())
}

pub(crate) fn receipt_dir() -> Result<PathBuf> {
    Ok(aida_dir()?.join(RECEIPT_DIR))
}

/// The receipt file name for a launch: only a canonical `draft-<uuid>`.
fn file_name(launch: &str) -> Result<String> {
    let id = launch
        .strip_prefix("draft-")
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .context("invalid review draft launch id")?;
    ensure!(
        format!("draft-{id}") == launch,
        "invalid review draft launch id"
    );
    Ok(format!("{id}.json"))
}

fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))
}

fn lock_path(launch: &str) -> Result<PathBuf> {
    let name = file_name(launch)?.replace(".json", ".lock");
    Ok(aida_dir()?.join(LOCK_DIR).join(name))
}

/// The per-receipt lock, beside (never inside) the receipt directory. A
/// zero `wait` tries once.
fn lock(launch: &str, wait: Duration) -> Result<std::fs::File> {
    use fs2::FileExt;
    let path = lock_path(launch)?;
    create_private_dir(path.parent().context("lock path has no parent")?)?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .context("cannot open the review handoff receipt lock")?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e).context("the review handoff receipt lock stayed busy"),
        }
    }
}

/// Stage the receipt under a private name in `dir`.
fn stage(dir: &Path, receipt: &Receipt) -> Result<PathBuf> {
    let tmp = dir.join(format!(".{}.{}.tmp", receipt.launch, uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)?;
    let written = file
        .write_all(&serde_json::to_vec_pretty(receipt)?)
        .and_then(|()| file.sync_all());
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(tmp)
}

fn parse(path: &Path, launch: &str) -> Result<Receipt> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .context("review handoff receipt is missing or unreadable")?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.nlink() == 1 && meta.len() <= RECEIPT_LIMIT,
        "review handoff receipt is not a bounded singly linked regular file"
    );
    let mut body = Vec::new();
    file.take(RECEIPT_LIMIT + 1).read_to_end(&mut body)?;
    ensure!(
        body.len() as u64 <= RECEIPT_LIMIT,
        "review handoff receipt exceeds its size limit"
    );
    let receipt: Receipt =
        serde_json::from_slice(&body).context("review handoff receipt is malformed")?;
    ensure!(
        receipt.version == VERSION,
        "unsupported review handoff receipt version {}",
        receipt.version
    );
    ensure!(
        receipt.launch == launch,
        "review handoff receipt does not match its file name"
    );
    validate(&receipt)?;
    Ok(receipt)
}

fn sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Field rules every reader relies on, so a receipt that breaks one is
/// unreadable as a whole instead of being shown or acted on in part.
fn validate(receipt: &Receipt) -> Result<()> {
    ensure!(
        crate::review_draft::full_sha(&receipt.head),
        "review handoff receipt does not name a full head SHA"
    );
    ensure!(
        receipt.pr > 0,
        "review handoff receipt names no pull request"
    );
    ensure!(
        receipt.project_root.is_absolute() && receipt.worktree.is_absolute(),
        "review handoff receipt paths are not absolute"
    );
    ensure!(
        !receipt.reviewer_session.trim().is_empty()
            && !receipt.launcher_grant.trim().is_empty()
            && !receipt.principal.trim().is_empty(),
        "review handoff receipt is missing its reviewer session, launcher grant or principal"
    );
    ensure!(
        receipt
            .seal
            .as_ref()
            .is_none_or(|seal| sha256_hex(&seal.sha256)),
        "review handoff receipt carries an invalid seal digest"
    );
    Ok(())
}

/// Whether automatic recovery may ever act on this receipt. Always `false`
/// for the version-1 receipts this build writes: see [`VERSION`].
pub(crate) fn automatic_recovery_eligible(receipt: &Receipt) -> bool {
    receipt.version >= FIRST_AUTO_RECOVERY_VERSION
}

/// The receipt registered for `launch` (`draft-<uuid>`).
pub(crate) fn load(launch: &str) -> Result<Receipt> {
    parse(&receipt_dir()?.join(file_name(launch)?), launch)
}

/// Register the draft slot before the reviewer is spawned. `Ok(None)` when
/// this process holds no grant delegating `reviewer`: only a launcher with
/// that delegation may register a slot, and the spawn itself reports the
/// missing authority. The receipt is published under its one name by a
/// no-replace rename: an existing one is an error, never an overwrite, and
/// the returned receipt is the one read back from that name.
pub(crate) fn register(
    project_root: &Path,
    worktree: &Path,
    draft: &LaunchDraft,
    reviewer_session: &str,
) -> Result<Option<Receipt>> {
    let Ok(launcher) = sa::validate_child_delegation(project_root, "reviewer") else {
        return Ok(None);
    };
    ensure!(
        !reviewer_session.trim().is_empty(),
        "missing routed reviewer session"
    );
    let (dir_dev, dir_ino) = draft.dir_identity()?;
    let receipt = Receipt {
        version: VERSION,
        launch: draft.launch_id()?,
        project_root: project_root.canonicalize()?,
        worktree: worktree.canonicalize()?,
        dir_dev,
        dir_ino,
        pr: draft.pr(),
        head: draft.head().to_string(),
        reviewer_session: reviewer_session.to_string(),
        launcher_grant: launcher.id,
        principal: launcher.principal,
        created_at: Utc::now(),
        state: State::Open,
        child_grant: None,
        seal: None,
    };
    validate(&receipt)?;
    let dir = receipt_dir()?;
    let dest = dir.join(file_name(&receipt.launch)?);
    create_private_dir(&dir)?;
    let tmp = stage(&dir, &receipt)?;
    // The staged file moves to its final name only when that name is free,
    // and never has a second link once this returns `Ok`.
    if let Err(e) = crate::gate::review_verdict::publish_verdict_no_replace(&tmp, &dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).context("cannot create the review handoff receipt");
    }
    let published = parse(&dest, &receipt.launch)
        .context("the review handoff receipt could not be read back after it was written")?;
    ensure!(
        published == receipt,
        "the review handoff receipt on disk is not the one this launch wrote"
    );
    Ok(Some(published))
}

/// Past retention, and the draft directory holds no draft (and can be read
/// well enough to say so).
fn expired(receipt: &Receipt, now: DateTime<Utc>) -> bool {
    now - receipt.created_at > RETENTION
        && !matches!(
            draft_files(receipt),
            DraftFiles::Present | DraftFiles::Unreadable
        )
}

/// The canonical id of a receipt file name (`<uuid>.json`).
fn receipt_id(name: &str) -> Option<&str> {
    name.strip_suffix(".json")
        .filter(|_| !name.starts_with('.'))
}

/// Bounded maintenance, called after a review launch has finished and never
/// before a reviewer is spawned: drop expired receipts and their locks so
/// one file per launch does not accumulate for ever. Each run examines at
/// most [`SWEEP_BATCH`] receipts within [`SWEEP_BUDGET`], starting at a
/// random point so successive runs cover the whole store, and skips a
/// receipt whose lock is busy. Returns how many receipts it removed.
pub(crate) fn sweep() -> usize {
    sweep_from(
        Utc::now(),
        &uuid::Uuid::new_v4().to_string(),
        SWEEP_BATCH,
        Instant::now() + SWEEP_BUDGET,
    )
}

fn sweep_from(now: DateTime<Utc>, pivot: &str, batch: usize, deadline: Instant) -> usize {
    let Ok(dir) = receipt_dir() else {
        return 0;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    // Names only: no receipt is opened until it is one of this run's batch.
    let mut ids = Vec::new();
    for entry in entries.flatten() {
        if Instant::now() >= deadline {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(id) = receipt_id(&name) {
            ids.push(id.to_string());
        }
    }
    ids.sort_unstable();
    let start = ids.partition_point(|id| id.as_str() < pivot);
    let mut removed = 0;
    for id in ids.iter().cycle().skip(start).take(batch.min(ids.len())) {
        if Instant::now() >= deadline {
            break;
        }
        if sweep_one(&dir, &format!("draft-{id}"), now).unwrap_or(false) {
            removed += 1;
        }
    }
    removed
}

/// Remove one receipt when it is expired. An unreadable receipt, a busy
/// lock or a failed removal leaves it in place.
fn sweep_one(dir: &Path, launch: &str, now: DateTime<Utc>) -> Result<bool> {
    let path = dir.join(file_name(launch)?);
    if !expired(&parse(&path, launch)?, now) {
        return Ok(false);
    }
    let held = lock(launch, Duration::ZERO)?;
    // Decide again on what the receipt says under its lock.
    if !expired(&parse(&path, launch)?, now) {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    let _ = std::fs::remove_file(lock_path(launch)?);
    drop(held);
    Ok(true)
}

/// Read-modify-write under the receipt's lock. `change` returns whether
/// there is anything to write.
fn update(launch: &str, change: impl FnOnce(&mut Receipt) -> Result<bool>) -> Result<Receipt> {
    let name = file_name(launch)?;
    let dir = receipt_dir()?;
    // No lock file is created for a launch that was never registered.
    std::fs::symlink_metadata(dir.join(&name))
        .context("review handoff receipt is missing or unreadable")?;
    let held = lock(launch, LOCK_WAIT)?;
    let mut receipt = parse(&dir.join(&name), launch)?;
    if change(&mut receipt)? {
        validate(&receipt)?;
        let tmp = stage(&dir, &receipt)?;
        if let Err(e) = std::fs::rename(&tmp, dir.join(name)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).context("cannot replace the review handoff receipt");
        }
    }
    drop(held);
    Ok(receipt)
}

/// Seal an open receipt with the digest of the draft the launcher read and
/// validated. Sealing again with the same digest is a no-op; a different
/// digest, or a receipt that is no longer open, is refused.
pub(crate) fn seal(launch: &str, sha256: &str, child_grant: Option<&str>) -> Result<Receipt> {
    ensure!(sha256_hex(sha256), "invalid draft digest");
    update(launch, |receipt| {
        ensure!(
            receipt.state == State::Open,
            "review handoff receipt is {}, not open",
            receipt.state.label()
        );
        match &receipt.seal {
            Some(seal) if seal.sha256 == sha256 => return Ok(false),
            Some(_) => bail!("review handoff receipt is already sealed with a different draft"),
            None => {}
        }
        receipt.seal = Some(Seal {
            sha256: sha256.to_string(),
            sealed_at: Utc::now(),
        });
        if receipt.child_grant.is_none() {
            receipt.child_grant = child_grant.map(str::to_string);
        }
        Ok(true)
    })
}

/// The launcher recorded the verdict from this slot's validated draft and
/// renamed the draft: close the receipt.
pub(crate) fn mark_consumed(launch: &str) -> Result<Receipt> {
    update(launch, |receipt| match receipt.state {
        State::Consumed => Ok(false),
        State::Open => {
            receipt.state = State::Consumed;
            Ok(true)
        }
        other => bail!("review handoff receipt is {}, not open", other.label()),
    })
}

/// The launch's verdict was collected without a draft (the launcher found
/// none to read): close the receipt as unused. A sealed receipt is refused,
/// because sealing means a draft was read and validated.
pub(crate) fn mark_unused(launch: &str) -> Result<Receipt> {
    update(launch, |receipt| match receipt.state {
        State::Unused => Ok(false),
        State::Open => {
            ensure!(
                receipt.seal.is_none(),
                "review handoff receipt is sealed with a draft; it cannot be closed as unused"
            );
            receipt.state = State::Unused;
            Ok(true)
        }
        other => bail!("review handoff receipt is {}, not open", other.label()),
    })
}

/// Every receipt in the store, oldest first, and the files that could not
/// be read as one (`file name`, `why`).
pub(crate) fn list() -> Result<(Vec<Receipt>, Vec<(String, String)>)> {
    let dir = receipt_dir()?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
    };
    let (mut receipts, mut unreadable) = (Vec::new(), Vec::new());
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().to_string();
        let Some(id) = receipt_id(&name) else {
            continue;
        };
        let launch = format!("draft-{id}");
        match file_name(&launch).and_then(|file| parse(&dir.join(file), &launch)) {
            Ok(receipt) => receipts.push(receipt),
            Err(e) => unreadable.push((name, format!("{e:#}"))),
        }
    }
    receipts.sort_by(|a, b| (a.created_at, &a.launch).cmp(&(b.created_at, &b.launch)));
    unreadable.sort();
    Ok((receipts, unreadable))
}

/// Which files the receipt's draft directory holds now. This is file state
/// only: it says nothing about the launcher or the gate ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DraftFiles {
    /// `draft.json` is there.
    Present,
    /// No `draft.json`; `recorded.json`, the name a launcher gives a draft
    /// once it has recorded it, is there.
    MarkedRecorded,
    /// Neither file is there.
    Absent,
    DirectoryMissing,
    /// The path is no longer the directory the receipt registered.
    DirectoryReplaced,
    /// A lookup failed for a reason other than the entry not existing.
    Unreadable,
}

impl DraftFiles {
    pub(crate) fn label(self) -> &'static str {
        match self {
            DraftFiles::Present => "present",
            DraftFiles::MarkedRecorded => "marked-recorded",
            DraftFiles::Absent => "absent",
            DraftFiles::DirectoryMissing => "directory-missing",
            DraftFiles::DirectoryReplaced => "directory-replaced",
            DraftFiles::Unreadable => "unreadable",
        }
    }
}

/// `Ok(false)` only when the entry does not exist; any other failed lookup
/// is an error, never "absent".
fn entry_exists(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Read-only and advisory: nothing is authorized from this answer.
pub(crate) fn draft_files(receipt: &Receipt) -> DraftFiles {
    let dir = receipt
        .worktree
        .join(".aida/review-verdicts")
        .join(&receipt.launch);
    let meta = match std::fs::symlink_metadata(&dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return DraftFiles::DirectoryMissing,
        Err(_) => return DraftFiles::Unreadable,
    };
    if !meta.is_dir() || meta.dev() != receipt.dir_dev || meta.ino() != receipt.dir_ino {
        return DraftFiles::DirectoryReplaced;
    }
    match (
        entry_exists(&dir.join("draft.json")),
        entry_exists(&dir.join("recorded.json")),
    ) {
        (Ok(true), _) => DraftFiles::Present,
        (Err(_), _) | (_, Err(_)) => DraftFiles::Unreadable,
        (Ok(false), Ok(true)) => DraftFiles::MarkedRecorded,
        (Ok(false), Ok(false)) => DraftFiles::Absent,
    }
}

/// Whether a writable lane mount at `path` would lie inside the host's
/// receipt store or its lock directory. Scoped to those two resolved
/// directories: an unrelated directory that merely shares a name is not
/// refused. When the user's AIDA home cannot be resolved there is no store
/// to compare against, and any path with a component of either name is
/// refused instead.
pub(crate) fn inside_host_only_root(path: &Path) -> bool {
    let Ok(home) = aida_dir() else {
        return path
            .components()
            .any(|p| p.as_os_str() == RECEIPT_DIR || p.as_os_str() == LOCK_DIR);
    };
    let resolved = resolve(path);
    [RECEIPT_DIR, LOCK_DIR].iter().any(|name| {
        let root = home.join(name);
        path.starts_with(&root) || resolved.starts_with(resolve(&root))
    })
}

/// `path` with its longest existing prefix canonicalized, so a directory
/// that does not exist yet still compares by its real location.
fn resolve(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(real) = cur.canonicalize() {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                cur = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn age_label(age: chrono::Duration) -> String {
    let minutes = age.num_minutes().max(0);
    match minutes {
        m if m < 60 => format!("{m}m"),
        m if m < 48 * 60 => format!("{}h{:02}m", m / 60, m % 60),
        m => format!("{}d", m / (24 * 60)),
    }
}

/// `aida review draft list`. Without `all`, the receipts of this project:
/// the main checkout the launcher registers them under, resolved the same
/// way from a linked checkout.
pub(crate) fn handle_list(all: bool, json: bool) -> Result<()> {
    let project = if all {
        None
    } else {
        Some(crate::find_main_worktree_root()?)
    };
    print!("{}", list_report(project.as_deref(), json, Utc::now())?);
    Ok(())
}

/// The listing for one project's main checkout, or for every project.
fn list_report(project: Option<&Path>, json: bool, now: DateTime<Utc>) -> Result<String> {
    let (mut receipts, unreadable) = list()?;
    if let Some(project) = project {
        let root = project
            .canonicalize()
            .unwrap_or_else(|_| project.to_path_buf());
        receipts.retain(|r| r.project_root == root);
    }
    render_list(&receipts, &unreadable, project.is_none(), json, now)
}

fn render_list(
    receipts: &[Receipt],
    unreadable: &[(String, String)],
    all: bool,
    json: bool,
    now: DateTime<Utc>,
) -> Result<String> {
    use std::fmt::Write as _;
    let mut out = String::new();
    if json {
        let rows: Vec<_> = receipts
            .iter()
            .map(|r| {
                let age = now - r.created_at;
                let mut row = serde_json::to_value(r)?;
                row["draft"] = draft_files(r).label().into();
                row["sealed"] = r.seal.is_some().into();
                row["age_seconds"] = age.num_seconds().max(0).into();
                row["within_automatic_window"] = (age <= AUTO_RECOVERY_WINDOW).into();
                row["automatic_recovery_eligible"] = automatic_recovery_eligible(r).into();
                Ok(row)
            })
            .collect::<Result<_>>()?;
        let unreadable: Vec<_> = unreadable
            .iter()
            .map(|(file, error)| serde_json::json!({"file": file, "error": error}))
            .collect();
        writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"receipts": rows, "unreadable": unreadable})
            )?
        )?;
        return Ok(out);
    }
    if receipts.is_empty() {
        writeln!(
            out,
            "No reviewer draft handoffs are registered{}.",
            if all { "" } else { " for this project" }
        )?;
    }
    for r in receipts {
        let age = now - r.created_at;
        let recovery = if r.state != State::Open {
            ""
        } else if !automatic_recovery_eligible(r) {
            " (not eligible for automatic recovery)"
        } else if age > AUTO_RECOVERY_WINDOW {
            " (past the 24h automatic window)"
        } else {
            ""
        };
        writeln!(
            out,
            "{}  PR-{} @ {}  {}  {}  draft: {}  age {}{}",
            r.launch,
            r.pr,
            r.head.get(..12).unwrap_or(&r.head),
            r.state.label(),
            if r.seal.is_some() {
                "sealed"
            } else {
                "unsealed"
            },
            draft_files(r).label(),
            age_label(age),
            recovery,
        )?;
        writeln!(
            out,
            "    worktree {}  reviewer session {}",
            r.worktree.display(),
            r.reviewer_session
        )?;
        if all {
            writeln!(out, "    project {}", r.project_root.display())?;
        }
    }
    for (file, error) in unreadable {
        writeln!(out, "unreadable receipt {file}: {error}")?;
    }
    Ok(out)
}

#[cfg(test)]
#[path = "tests/bug_2669_review_handoff_tests.rs"]
mod tests;
