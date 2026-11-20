//! Launch receipts for reviewer draft handoffs.
//!
//! Before a launcher spawns a headless reviewer it registers the draft slot
//! here, in the user's AIDA home beside the grant store, and seals the
//! receipt with the draft's digest once the reviewer has exited and the
//! draft validated. A receipt holds no secret and grants nothing: it is the
//! durable record that a launcher holding reviewer delegation registered
//! this slot for this PR, head and reviewer session. Recording a draft from
//! a receipt is a separate, later step; nothing in this module records.
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

const RECEIPT_DIR: &str = "review-handoffs";
const LOCK_DIR: &str = "review-handoff-locks";
/// Directory names a contained lane must never be given write access to.
pub(crate) const HOST_ONLY_DIRS: [&str; 2] = [RECEIPT_DIR, LOCK_DIR];
const VERSION: u32 = 1;
const RECEIPT_LIMIT: u64 = 64 * 1024;
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
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
}

impl State {
    fn label(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Consumed => "consumed",
            State::Superseded => "superseded",
            State::Discarded => "discarded",
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

/// The per-receipt lock, beside (never inside) the receipt directory.
fn lock(launch: &str) -> Result<std::fs::File> {
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
    let deadline = std::time::Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
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
    Ok(receipt)
}

/// The receipt registered for `launch` (`draft-<uuid>`).
pub(crate) fn load(launch: &str) -> Result<Receipt> {
    parse(&receipt_dir()?.join(file_name(launch)?), launch)
}

/// Register the draft slot before the reviewer is spawned. `Ok(None)` when
/// this process holds no grant delegating `reviewer`: only a launcher with
/// that delegation may register a slot, and the spawn itself reports the
/// missing authority. The receipt is created exclusively; an existing one is
/// an error, never an overwrite.
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
    let name = file_name(&receipt.launch)?;
    let dir = receipt_dir()?;
    create_private_dir(&dir)?;
    let tmp = stage(&dir, &receipt)?;
    // A hard link fails when the name exists: exclusive and never partial.
    let linked = std::fs::hard_link(&tmp, dir.join(name));
    let _ = std::fs::remove_file(&tmp);
    linked.context("cannot create the review handoff receipt")?;
    sweep(receipt.created_at);
    Ok(Some(receipt))
}

/// Past retention, and no draft waits on this receipt to be recorded.
fn expired(receipt: &Receipt, now: DateTime<Utc>) -> bool {
    now - receipt.created_at > RETENTION
        && !matches!(draft_status(receipt), "unrecorded" | "unreadable")
}

/// Best effort: drop expired receipts and their locks so one file per review
/// launch does not accumulate for ever. Unreadable files are left alone.
fn sweep(now: DateTime<Utc>) {
    let Ok((receipts, _)) = list() else {
        return;
    };
    for receipt in receipts.iter().filter(|r| expired(r, now)) {
        let _ = (|| -> Result<()> {
            let held = lock(&receipt.launch)?;
            let path = receipt_dir()?.join(file_name(&receipt.launch)?);
            // Decide again on what the receipt says under its lock.
            if expired(&parse(&path, &receipt.launch)?, now) {
                std::fs::remove_file(&path)?;
                std::fs::remove_file(lock_path(&receipt.launch)?)?;
            }
            drop(held);
            Ok(())
        })();
    }
}

/// Read-modify-write under the receipt's lock. `change` returns whether
/// there is anything to write.
fn update(launch: &str, change: impl FnOnce(&mut Receipt) -> Result<bool>) -> Result<Receipt> {
    let name = file_name(launch)?;
    let held = lock(launch)?;
    let dir = receipt_dir()?;
    let mut receipt = parse(&dir.join(&name), launch)?;
    if change(&mut receipt)? {
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
    ensure!(
        sha256.len() == 64
            && sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid draft digest"
    );
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

/// The launcher recorded the draft and renamed it: close the receipt.
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
        let Some(id) = name
            .strip_suffix(".json")
            .filter(|_| !name.starts_with('.'))
        else {
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

/// What the receipt's draft directory holds now. Read-only and advisory:
/// nothing is authorized from this answer.
pub(crate) fn draft_status(receipt: &Receipt) -> &'static str {
    let dir = receipt
        .worktree
        .join(".aida/review-verdicts")
        .join(&receipt.launch);
    let meta = match std::fs::symlink_metadata(&dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return "directory-missing",
        Err(_) => return "unreadable",
    };
    if !meta.is_dir() || meta.dev() != receipt.dir_dev || meta.ino() != receipt.dir_ino {
        return "directory-replaced";
    }
    let present = |name: &str| std::fs::symlink_metadata(dir.join(name)).is_ok();
    if present("draft.json") {
        "unrecorded"
    } else if present("recorded.json") {
        "recorded"
    } else {
        "not-written"
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

/// `aida review draft list`.
pub(crate) fn handle_list(all: bool, json: bool) -> Result<()> {
    let (mut receipts, unreadable) = list()?;
    if !all {
        let root = crate::find_project_root()?;
        let root = root.canonicalize().unwrap_or(root);
        receipts.retain(|r| r.project_root == root);
    }
    let now = Utc::now();
    if json {
        let rows: Vec<_> = receipts
            .iter()
            .map(|r| {
                let age = now - r.created_at;
                let mut row = serde_json::to_value(r)?;
                row["draft"] = draft_status(r).into();
                row["sealed"] = r.seal.is_some().into();
                row["age_seconds"] = age.num_seconds().max(0).into();
                row["within_automatic_window"] = (age <= AUTO_RECOVERY_WINDOW).into();
                Ok(row)
            })
            .collect::<Result<_>>()?;
        let unreadable: Vec<_> = unreadable
            .iter()
            .map(|(file, error)| serde_json::json!({"file": file, "error": error}))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"receipts": rows, "unreadable": unreadable})
            )?
        );
        return Ok(());
    }
    if receipts.is_empty() {
        println!(
            "No reviewer draft handoffs are registered{}.",
            if all { "" } else { " for this project" }
        );
    }
    for r in &receipts {
        let age = now - r.created_at;
        println!(
            "{}  PR-{} @ {}  {}  {}  draft: {}  age {}{}",
            r.launch,
            r.pr,
            &r.head[..r.head.len().min(12)],
            r.state.label(),
            if r.seal.is_some() {
                "sealed"
            } else {
                "unsealed"
            },
            draft_status(r),
            age_label(age),
            if r.state == State::Open && age > AUTO_RECOVERY_WINDOW {
                " (past the 24h automatic window)"
            } else {
                ""
            },
        );
        println!(
            "    worktree {}  reviewer session {}",
            r.worktree.display(),
            r.reviewer_session
        );
        if all {
            println!("    project {}", r.project_root.display());
        }
    }
    for (file, error) in &unreadable {
        println!("unreadable receipt {file}: {error}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/bug_2669_review_handoff_tests.rs"]
mod tests;
