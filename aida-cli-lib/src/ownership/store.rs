//! Ownership record storage (STORY-2325 PR-B).
//!
//! One record per work key at `<main worktree>/.aida/ownership/<kind>/<key>.json`.
//! Every write takes the permanent `.aida/ownership/.lock` (never unlinked, so
//! two writers can never lock different inodes), re-reads the record, checks
//! the caller's expected generation, writes the new record atomically with
//! the generation bumped by one, and appends one line to
//! `.aida/ownership/journal.jsonl`. A lost race refuses and never overwrites.
//! A contained process is refused before it touches the directory.
//!
//! The directory lives on the main worktree only: sibling worktrees resolve
//! it through git's common dir, the same way seat occupancy does.
//!
//! trace:STORY-2325 | ai:claude

use super::types::{Ownership, WorkKey};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The directory under `.aida/`.
// trace:STORY-2325 | ai:claude
pub(crate) const DIR: &str = "ownership";
const LOCK_FILE: &str = ".lock";
const JOURNAL_FILE: &str = "journal.jsonl";
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_POLL: Duration = Duration::from_millis(20);

/// What a writer expects to find before it writes.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Expect {
    /// Whatever is there (the transition itself decides from the current
    /// record, read under the lock).
    Any,
    /// No record yet.
    Absent,
    /// Exactly this generation.
    Generation(u64),
}

/// One journal line: every committed write, in order.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JournalEntry {
    pub at: DateTime<Utc>,
    pub op: String,
    pub key: WorkKey,
    pub from_generation: Option<u64>,
    pub to_generation: u64,
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The next record for one key, from its current one; `None` = no change.
// trace:STORY-2325 | ai:claude
pub(crate) type Transition<'a> =
    Box<dyn FnOnce(Option<&Ownership>) -> Option<(Ownership, Option<String>)> + 'a>;

/// One key's write inside [`OwnershipStore::update_all`].
// trace:STORY-2325 | ai:claude
pub(crate) struct Step<'a> {
    pub key: WorkKey,
    pub op: String,
    pub expect: Expect,
    pub transition: Transition<'a>,
}

fn check_expect(path: &Path, expect: Expect, have: Option<u64>) -> Result<()> {
    match (expect, have) {
        (Expect::Any, _) | (Expect::Absent, None) => Ok(()),
        (Expect::Generation(want), Some(got)) if want == got => Ok(()),
        (Expect::Absent, Some(got)) => bail!(
            "the ownership record {} already exists (generation {got}); nothing was written",
            path.display()
        ),
        (Expect::Generation(want), got) => bail!(
            "the ownership record {} changed (expected generation {want}, found {}); \
             nothing was written",
            path.display(),
            got.map_or_else(|| "none".to_string(), |g| g.to_string())
        ),
    }
}

/// The outcome of one committed write.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Committed {
    pub record: Ownership,
    pub note: Option<String>,
}

// trace:STORY-2325 | ai:claude
pub(crate) struct OwnershipStore {
    dir: PathBuf,
}

impl OwnershipStore {
    /// The store for the repository containing `start`, rooted at its main
    /// worktree. `Ok(None)` outside any repository.
    // trace:STORY-2325 | ai:claude
    pub(crate) fn for_project(start: &Path) -> Result<Option<Self>> {
        Ok(crate::seat_occupancy::store::main_worktree_root_opt(start)?
            .map(|root| Self::at(root.join(".aida").join(DIR))))
    }

    /// A store at an explicit directory (tests).
    pub(crate) fn at(dir: impl Into<PathBuf>) -> Self {
        OwnershipStore { dir: dir.into() }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn journal_path(&self) -> PathBuf {
        self.dir.join(JOURNAL_FILE)
    }

    /// The file one key's record lives in.
    // trace:STORY-2325 | ai:claude
    pub(crate) fn path_for(&self, key: &WorkKey) -> PathBuf {
        let (kind, name) = file_name(key);
        self.dir.join(kind).join(format!("{name}.json"))
    }

    /// The record for `key`, or `None`. A malformed record is an error: it
    /// fails closed instead of reading as free.
    // trace:STORY-2325 | ai:claude
    pub(crate) fn load(&self, key: &WorkKey) -> Result<Option<Ownership>> {
        read_record(&self.path_for(key))
    }

    /// Every record, sorted by key. A malformed record is reported by path
    /// beside the readable ones.
    // trace:STORY-2325 | ai:claude
    pub(crate) fn list(&self) -> (Vec<Ownership>, Vec<(PathBuf, String)>) {
        let mut good = Vec::new();
        let mut bad = Vec::new();
        for kind in KINDS {
            let Ok(entries) = std::fs::read_dir(self.dir.join(kind)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                match read_record(&path) {
                    Ok(Some(rec)) => good.push(rec),
                    Ok(None) => {}
                    Err(e) => bad.push((path, format!("{e:#}"))),
                }
            }
        }
        good.sort_by(|a, b| a.key.cmp(&b.key));
        bad.sort();
        (good, bad)
    }

    /// The one write path, for one key. See [`OwnershipStore::update_all`].
    // trace:STORY-2325 | ai:claude
    pub(crate) fn update(
        &self,
        key: &WorkKey,
        op: &str,
        expect: Expect,
        transition: impl FnOnce(Option<&Ownership>) -> Option<(Ownership, Option<String>)>,
    ) -> Result<Option<Committed>> {
        let mut out = self.update_all(vec![Step {
            key: key.clone(),
            op: op.to_string(),
            expect,
            transition: Box::new(transition),
        }])?;
        Ok(out.pop().flatten())
    }

    /// The write path. Under the ownership lock: read every step's current
    /// record and check its `expect`, ask each `transition` for the next
    /// record (`None` = no change), and only when every step passed, commit
    /// each changed record with its generation bumped by one and journal it.
    ///
    /// A failed expectation writes nothing for any step: the batch refuses
    /// on expectation failures. The writes themselves are not staged: an
    /// I/O error part-way through a multi-key batch (a record write or a
    /// journal append) can leave the earlier steps written, and a record
    /// written without its journal line. Replaying the journal against the
    /// records reconciles that; staged writes arrive with PR-C (advisor
    /// ruling round 3 on PR #3291).
    ///
    /// A contained seat never writes here (A1, A6): the call refuses before
    /// it creates or locks anything. Its records are written host-side.
    // trace:STORY-2325 | ai:claude
    pub(crate) fn update_all(&self, steps: Vec<Step<'_>>) -> Result<Vec<Option<Committed>>> {
        if crate::session::process_contained() {
            bail!(
                "a contained seat never writes the ownership records; \
                 the host process that launched it records them"
            );
        }
        let _lock = self.lock(LOCK_TIMEOUT)?;
        let mut planned = Vec::with_capacity(steps.len());
        for step in steps {
            let path = self.path_for(&step.key);
            let current = read_record(&path)?;
            let have = current.as_ref().map(|r| r.generation);
            check_expect(&path, step.expect, have)?;
            let next = (step.transition)(current.as_ref());
            if let Some((rec, _)) = &next {
                if rec.key != step.key {
                    bail!("an ownership transition for one key returned a record for another");
                }
            }
            planned.push((step.key, step.op, path, have, next));
        }
        let mut out = Vec::with_capacity(planned.len());
        for (key, op, path, have, next) in planned {
            let Some((mut next, note)) = next else {
                out.push(None);
                continue;
            };
            next.generation = have.map_or(1, |g| g + 1);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            let body = serde_json::to_string_pretty(&next)?;
            aida_core::write_atomic(&path, body + "\n")
                .with_context(|| format!("writing {}", path.display()))?;
            self.append_journal(&JournalEntry {
                at: Utc::now(),
                op,
                key,
                from_generation: have,
                to_generation: next.generation,
                pid: std::process::id(),
                note: note.clone(),
            })?;
            out.push(Some(Committed { record: next, note }));
        }
        Ok(out)
    }

    /// Every journal line, oldest first. Unreadable lines are skipped.
    // trace:STORY-2325 | ai:claude
    pub(crate) fn journal(&self) -> Vec<JournalEntry> {
        std::fs::read_to_string(self.journal_path())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn append_journal(&self, entry: &JournalEntry) -> Result<()> {
        let mut line = serde_json::to_string(entry)?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.journal_path())
            .with_context(|| format!("opening {}", self.journal_path().display()))?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }

    fn lock(&self, timeout: Duration) -> Result<File> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        let path = self.dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(file),
                Err(e) if aida_core::file_lock::is_lock_contended(&e) => {
                    if Instant::now() >= deadline {
                        bail!(
                            "the ownership lock {} is held by another writer; waited {}s",
                            path.display(),
                            timeout.as_secs()
                        );
                    }
                    std::thread::sleep(LOCK_POLL);
                }
                Err(e) => return Err(e).with_context(|| format!("locking {}", path.display())),
            }
        }
    }
}

const KINDS: [&str; 5] = ["spec", "change", "train", "review", "mail"];

/// `(kind directory, file stem)` for a key. Every stem is built from
/// `[A-Za-z0-9._-]` only, so no key can name a path outside its directory.
// trace:STORY-2325 | ai:claude
pub(crate) fn file_name(key: &WorkKey) -> (&'static str, String) {
    match key {
        WorkKey::Spec { spec } => ("spec", spec.0.to_string()),
        WorkKey::Change { change } => ("change", format!("{}-{}", safe(&change.forge), change.n)),
        WorkKey::Train { train } => ("train", format!("{}-{}", safe(&train.forge), train.n)),
        WorkKey::Review { change, head } => (
            "review",
            format!("{}-{}-{}", safe(&change.forge), change.n, safe(head)),
        ),
        WorkKey::MailScope { anchor } => ("mail", safe(&anchor.0)),
    }
}

fn safe(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(120)
        .collect();
    if s.is_empty() {
        "_".to_string()
    } else {
        s
    }
}

fn read_record(path: &Path) -> Result<Option<Ownership>> {
    let body = match std::fs::read_to_string(path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    serde_json::from_str(&body)
        .map(Some)
        .with_context(|| format!("malformed ownership record {}", path.display()))
}
