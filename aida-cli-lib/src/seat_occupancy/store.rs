//! Seat record storage (TASK-1607, sketch section 2).
//!
//! One record per seat at `<main worktree>/.aida/seat-occupancy/<seat>.json`,
//! guarded by a permanent `<seat>.lock` sidecar that is never unlinked (an
//! unlinked flock file lets two processes lock different inodes). Writes use
//! atomic replacement. Sibling worktrees resolve the same main worktree
//! through git's common dir; if that cannot be resolved, occupancy fails
//! closed instead of splitting the record per worktree.
//!
//! Lock order (ADR-70): seat lock first, then any queue lock. Never hold the
//! seat lock across mail delivery, peer waits or child completion.
//!
//! trace:TASK-1607 trace:ADR-70 | ai:claude

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::fs::File;
use std::path::{Path, PathBuf};

use super::{SeatRecord, SCHEMA_VERSION};

const DIR: &str = "seat-occupancy";

pub(crate) struct SeatStore {
    dir: PathBuf,
}

/// An exclusively locked seat record. Dropping it unlocks.
pub(crate) struct SeatGuard {
    _lock: File,
    path: PathBuf,
    original: SeatRecord,
    pub rec: SeatRecord,
}

impl SeatStore {
    /// The store for the repo containing `start`, rooted at its main worktree.
    pub fn for_project(start: &Path) -> Result<Self> {
        Ok(SeatStore {
            dir: main_worktree_root(start)?.join(".aida").join(DIR),
        })
    }

    /// Like [`SeatStore::for_project`], but `Ok(None)` outside any repository.
    pub fn for_project_opt(start: &Path) -> Result<Option<Self>> {
        Ok(main_worktree_root_opt(start)?.map(|root| SeatStore {
            dir: root.join(".aida").join(DIR),
        }))
    }

    /// A store at an explicit directory (tests).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        SeatStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Lock `seat` exclusively and load its record.
    pub fn lock(&self, seat: &str) -> Result<SeatGuard> {
        validate_seat_name(seat)?;
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("cannot create {}", self.dir.display()))?;
        let lock_path = self.dir.join(format!("{seat}.lock"));
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("cannot open {}", lock_path.display()))?;
        lock.lock_exclusive()
            .with_context(|| format!("cannot lock {}", lock_path.display()))?;
        let path = self.dir.join(format!("{seat}.json"));
        let rec = read_record(&path, seat)?;
        Ok(SeatGuard {
            _lock: lock,
            path,
            original: rec.clone(),
            rec,
        })
    }

    /// Read without locking (status/inspection). Atomic replacement means a
    /// reader sees either the old or the new record, never a torn one.
    pub fn peek(&self, seat: &str) -> Result<SeatRecord> {
        validate_seat_name(seat)?;
        read_record(&self.dir.join(format!("{seat}.json")), seat)
    }
}

impl SeatGuard {
    /// Persist the record if it changed.
    pub fn save(&mut self) -> Result<()> {
        if self.rec == self.original {
            return Ok(());
        }
        let bytes = serde_json::to_vec_pretty(&self.rec)?;
        aida_core::fs_atomic::write_atomic(&self.path, bytes)
            .with_context(|| format!("cannot write {}", self.path.display()))?;
        self.original = self.rec.clone();
        Ok(())
    }
}

fn validate_seat_name(seat: &str) -> Result<()> {
    if seat.is_empty() || !seat.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
        bail!("invalid seat name `{seat}`");
    }
    Ok(())
}

fn read_record(path: &Path, seat: &str) -> Result<SeatRecord> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SeatRecord::empty(seat)),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("seat record {} is unreadable; refusing", path.display()))
        }
    };
    let rec: SeatRecord = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "seat record {} is malformed; refusing rather than guessing who holds the seat",
            path.display()
        )
    })?;
    if rec.schema != SCHEMA_VERSION || rec.seat != seat {
        bail!(
            "seat record {} has schema {} for seat `{}`; this build expects schema {SCHEMA_VERSION} for `{seat}`",
            path.display(),
            rec.schema,
            rec.seat
        );
    }
    Ok(rec)
}

/// Main worktree root via git's common dir. Unlike `main_worktree_root_from`,
/// this never falls back to `start`: a per-worktree record would let two
/// sibling worktrees each hold the seat.
pub(crate) fn main_worktree_root(start: &Path) -> Result<PathBuf> {
    main_worktree_root_opt(start)?.with_context(|| {
        format!(
            "seat occupancy cannot locate the main worktree: {} is not inside a git repository",
            start.display()
        )
    })
}

/// `Ok(None)` only when git positively reports that `start` is not inside a
/// repository; any other failure is an error (fail closed).
pub(crate) fn main_worktree_root_opt(start: &Path) -> Result<Option<PathBuf>> {
    // external-prose-classifier: seat_occupancy::store::main_worktree_root_opt
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(start)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .env("LC_ALL", "C")
        .output()
        .context("seat occupancy needs git to locate the main worktree")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // git exits 128 for every rev-parse failure; its C-locale message is
        // the only signal that separates "no repository" from a real error.
        if stderr.contains("not a git repository") {
            return Ok(None);
        }
        bail!(
            "seat occupancy cannot locate the main worktree from {}: {}",
            start.display(),
            stderr.trim()
        );
    }
    let common = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    match common.file_name().and_then(|n| n.to_str()) {
        Some(".git") => Ok(Some(
            common
                .parent()
                .map(Path::to_path_buf)
                .context("git common dir has no parent")?,
        )),
        _ => bail!(
            "seat occupancy needs a non-bare repository with a main worktree (git common dir: {})",
            common.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_record_is_empty_and_save_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = SeatStore::at(dir.path());
        let mut g = store.lock("orchestrator").unwrap();
        assert_eq!(g.rec, SeatRecord::empty("orchestrator"));
        g.rec.generation = 3;
        g.save().unwrap();
        drop(g);
        assert_eq!(store.peek("orchestrator").unwrap().generation, 3);
        assert!(dir.path().join("orchestrator.lock").exists());
    }

    #[test]
    fn malformed_record_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("orchestrator.json"), "{not json").unwrap();
        let store = SeatStore::at(dir.path());
        let err = store.lock("orchestrator").err().unwrap().to_string();
        assert!(err.contains("malformed"), "{err}");
    }

    #[test]
    fn seat_names_cannot_escape_the_directory() {
        let store = SeatStore::at("/nonexistent");
        assert!(store.peek("../x").is_err());
        assert!(store.peek("").is_err());
    }

    #[test]
    fn lock_is_exclusive_across_handles() {
        let dir = tempfile::tempdir().unwrap();
        let store = SeatStore::at(dir.path());
        let g = store.lock("orchestrator").unwrap();
        let other = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.path().join("orchestrator.lock"))
            .unwrap();
        assert!(
            other.try_lock_exclusive().is_err(),
            "second lock must contend"
        );
        drop(g);
        assert!(other.try_lock_exclusive().is_ok());
    }

    #[test]
    fn sibling_worktrees_share_the_main_root() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let run = |args: &[&str], cwd: &Path| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        std::fs::create_dir_all(&main).unwrap();
        run(&["init", "-q", "-b", "main"], &main);
        run(
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
            &main,
        );
        let wt = dir.path().join("wt");
        run(
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "b"],
            &main,
        );
        let a = main_worktree_root(&main).unwrap();
        let b = main_worktree_root(&wt).unwrap();
        assert_eq!(a.canonicalize().unwrap(), b.canonicalize().unwrap());
    }
}
