//! Cooperative admission exclusion. Kernel flock is the exclusion authority.
//! Review controllers and cooperative trains share a mode; land excludes both.
// trace:STORY-2857 | ai:codex
use aida_core::proc_identity::{ProcIdentity, Verdict};
use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Receipt {
    version: u8,
    identity: ProcIdentity,
    purpose: String,
    token: String,
}
#[derive(Debug)]
pub(crate) struct AdmissionBusy(pub(crate) String);
impl std::fmt::Display for AdmissionBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "admission busy: {}", self.0)
    }
}
impl std::error::Error for AdmissionBusy {}

pub(crate) struct Claim {
    lock: File,
    receipt: PathBuf,
}
impl Drop for Claim {
    fn drop(&mut self) {
        // Match acquire's receipts -> mode ordering: while waiting for receipts,
        // retain our mode lock. Readers only try (never wait for) the mode lock.
        // trace:STORY-2857 | ai:codex
        if let Some(dir) = self.receipt.parent() {
            if let Ok(serial) = open(&dir.join("receipts.lock")) {
                if serial.lock_exclusive().is_ok() {
                    let _ = std::fs::remove_file(&self.receipt);
                }
            }
        }
        let _ = FileExt::unlock(&self.lock);
    }
}
fn open(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?)
}
fn directory(root: &Path) -> PathBuf {
    root.join(".aida/admission-owner")
}
fn receipts(dir: &Path) -> Result<Vec<Receipt>> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json") {
            let body =
                aida_core::seat_clone_detect::read_regular_file_result_with_limit(&path, 16384)
                    .context("admission holder receipt is unreadable")?;
            anyhow::ensure!(body.len() <= 16384, "admission holder receipt is too large");
            let row: Receipt = serde_json::from_str(&body)?;
            anyhow::ensure!(row.version == 1, "unsupported admission holder receipt");
            if matches!(
                aida_core::proc_identity::observe(&row.identity, true),
                Verdict::Dead(_)
            ) {
                std::fs::remove_file(path)?;
            } else {
                rows.push(row);
            }
        }
    }
    Ok(rows)
}
fn busy(rows: &[Receipt]) -> anyhow::Error {
    AdmissionBusy(if rows.is_empty() {
        "holder PID/start identity/purpose unknown".into()
    } else {
        rows.iter()
            .map(|r| {
                format!(
                    "PID {}, start {}:{} (pidns {}), purpose {}",
                    r.identity.pid,
                    r.identity.boot_id,
                    r.identity.starttime,
                    r.identity.pidns_inode,
                    r.purpose
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    })
    .into()
}

/// Acquire before discovery/launch, retain until all owned work has settled.
/// Shared locks admit only legacy controllers; every other caller is exclusive.
pub(crate) fn acquire(root: &Path, purpose: &str, legacy: bool) -> Result<Claim> {
    acquire_mode(
        root,
        purpose,
        if legacy {
            Mode::Legacy
        } else {
            Mode::Exclusive
        },
    )
}

// Explicitly separate manual cooperation from the legacy controller option.
// trace:BUG-2650 | ai:codex
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Exclusive,
    Legacy,
    CooperativeTrain,
}
fn acquire_mode(root: &Path, purpose: &str, mode: Mode) -> Result<Claim> {
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    let serial = open(&dir.join("receipts.lock"))?;
    serial.lock_exclusive()?;
    let rows = receipts(&dir)?;
    // Serialize trains from discovery onward, even before run.lock exists.
    // Review controllers alone may coexist with a manual train.
    // trace:BUG-2650 | ai:codex
    if mode == Mode::CooperativeTrain
        && rows.iter().any(|r| {
            matches!(
                r.purpose.as_str(),
                "legacy auto-train" | "cooperative manual train"
            )
        })
    {
        return Err(busy(&rows));
    }
    let lock = open(&dir.join("mode.lock"))?;
    let result = if mode != Mode::Exclusive {
        FileExt::try_lock_shared(&lock)
    } else {
        lock.try_lock_exclusive()
    };
    if let Err(error) = result {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Err(busy(&rows));
        }
        return Err(error.into());
    }
    // A dead controller may have surviving children. Unknown is not quiescent.
    if mode == Mode::Exclusive {
        crate::review_watch::admission_quiescent(root)?;
    }
    let book = crate::land::run::Book::read(root)?;
    let children: Vec<_> = book
        .children
        .iter()
        .filter(|c| crate::land::run::alive(c))
        .map(|c| Receipt {
            version: 1,
            identity: c.identity.clone(),
            purpose: format!("prior land {:?}", c.kind),
            token: String::new(),
        })
        .collect();
    if !children.is_empty() {
        return Err(busy(&children));
    }
    // A joining legacy controller may share the current auto-train's work.
    // Unknown/orphan trains still refuse, including when only a watcher remains.
    // trace:STORY-2857 | ai:codex
    let joining_train = mode == Mode::Legacy
        && rows.iter().any(|r| {
            r.purpose == "legacy auto-train"
                && matches!(
                    aida_core::proc_identity::observe(&r.identity, true),
                    Verdict::Alive
                )
        });
    if !joining_train && crate::pr_train::admission_train_running(root)? {
        return Err(AdmissionBusy(
            "prior train is not quiescent; holder identity unknown (inspect pr train --status)"
                .into(),
        )
        .into());
    }
    let row = Receipt {
        version: 1,
        identity: ProcIdentity::capture(std::process::id()).map_err(anyhow::Error::msg)?,
        purpose: purpose.into(),
        token: uuid::Uuid::new_v4().to_string(),
    };
    let receipt = dir.join(format!("{}.json", row.token));
    std::fs::write(&receipt, serde_json::to_vec(&row)?)?;
    Ok(Claim { lock, receipt })
}

/// Only a land controller's direct train child gets its cooperative handoff.
/// This conveys no grant or merge authority.
pub(crate) fn train(root: &Path, automatic: bool) -> Result<Option<Claim>> {
    train_mode(root, automatic, Mode::Exclusive)
}

// Manual merge trains alone opt into member-scoped review coexistence. Direct
// ship retains its existing exclusive/quiescent admission policy.
// trace:BUG-2650 | ai:codex
pub(crate) fn cooperative_train(root: &Path, automatic: bool) -> Result<Option<Claim>> {
    train_mode(root, automatic, Mode::CooperativeTrain)
}

fn train_mode(root: &Path, automatic: bool, mode: Mode) -> Result<Option<Claim>> {
    let dir = directory(root);
    if dir.exists() {
        let serial = open(&dir.join("receipts.lock"))?;
        serial.lock_exclusive()?;
        let rows = receipts(&dir)?;
        let parent = unsafe { libc::getppid() } as u32;
        if rows.iter().any(|r| {
            matches!(
                aida_core::proc_identity::observe(&r.identity, true),
                Verdict::Alive
            ) && ((automatic
                && r.identity.pid == std::process::id()
                && r.purpose == "legacy auto-train")
                || (r.identity.pid == parent && r.purpose == "land run"))
        }) {
            return Ok(None);
        }
    }
    let purpose = if mode == Mode::CooperativeTrain {
        "cooperative manual train"
    } else {
        "manual train/ship"
    };
    acquire_mode(root, purpose, mode).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    // trace:BUG-2650 | ai:codex
    #[test]
    fn bug2650_cooperative_trains_serialize_and_refuse_orphan_runs() {
        let root = tempfile::tempdir().unwrap();
        let _watcher = acquire(root.path(), "legacy review-watch", true).unwrap();
        assert!(train(root.path(), false).is_err());
        let first = cooperative_train(root.path(), false).unwrap();
        assert!(cooperative_train(root.path(), false).is_err());
        assert!(acquire(root.path(), "land run", false).is_err());
        drop(first);
        assert!(cooperative_train(root.path(), false).is_ok());
        let dir = root.path().join(".aida/train");
        std::fs::create_dir_all(&dir).unwrap();
        let orphan = open(&dir.join("run.lock")).unwrap();
        orphan.lock_exclusive().unwrap();
        assert!(cooperative_train(root.path(), false).is_err());
    }

    // trace:STORY-2857 | ai:codex
    #[test]
    fn story2857_legacy_watcher_joins_active_train_but_not_orphan() {
        let root = tempfile::tempdir().unwrap();
        let worker = acquire(root.path(), "legacy auto-train", true).unwrap();
        let dir = root.path().join(".aida/train");
        std::fs::create_dir_all(&dir).unwrap();
        let run = open(&dir.join("run.lock")).unwrap();
        run.lock_exclusive().unwrap();
        let original = std::fs::read(&worker.receipt).unwrap();
        let mut unknown: Receipt = serde_json::from_slice(&original).unwrap();
        unknown.identity.pidns_inode = u64::MAX;
        std::fs::write(&worker.receipt, serde_json::to_vec(&unknown).unwrap()).unwrap();
        assert!(acquire(root.path(), "legacy review-watch", true).is_err());
        std::fs::write(&worker.receipt, original).unwrap();
        let watcher = acquire(root.path(), "legacy review-watch", true).unwrap();
        for purpose in ["land run", "manual train/ship"] {
            let error = acquire(root.path(), purpose, false).err().unwrap();
            assert!(error.downcast_ref::<AdmissionBusy>().is_some());
        }
        drop(worker);
        assert!(acquire(root.path(), "legacy review-watch", true).is_err());
        drop(watcher);
        assert!(acquire(root.path(), "legacy review-watch", true).is_err());
        FileExt::unlock(&run).unwrap();
        assert!(acquire(root.path(), "legacy review-watch", true).is_ok());
    }

    // Hold the reader lock across a receipt snapshot and opening every row.
    // Drop must block until that snapshot has been consumed.
    // trace:STORY-2857 | ai:codex
    #[test]
    fn story2857_drop_serializes_with_receipt_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let worker = acquire(root.path(), "legacy auto-train", true).unwrap();
        let _watcher = acquire(root.path(), "legacy review-watch", true).unwrap();
        let dir = directory(root.path());
        let serial = open(&dir.join("receipts.lock")).unwrap();
        serial.lock_exclusive().unwrap();
        let paths: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        let receipt = worker.receipt.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let dropping = std::thread::spawn(move || {
            tx.send(()).unwrap();
            drop(worker);
            done_tx.send(()).unwrap();
        });
        rx.recv().unwrap();
        assert!(
            matches!(
                done_rx.recv_timeout(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "drop must wait for the reader's snapshot lock"
        );
        // A conflicting lock proves the reader excludes receipt deletion.
        let contender = open(&dir.join("receipts.lock")).unwrap();
        assert_eq!(
            contender.try_lock_exclusive().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        for path in paths {
            std::fs::read(path).unwrap();
        }
        assert_eq!(receipts(&dir).unwrap().len(), 2);
        FileExt::unlock(&serial).unwrap();
        dropping.join().unwrap();
        assert!(!receipt.exists());
        let error = acquire(root.path(), "manual train/ship", false)
            .err()
            .unwrap();
        assert!(error.downcast_ref::<AdmissionBusy>().is_some());
        assert!(error.to_string().contains("legacy review-watch"));
    }
    #[test]
    fn story2857_modes_and_manual_train_exclude_each_other() {
        let root = tempfile::tempdir().unwrap();
        let worker = acquire(root.path(), "legacy auto-train", true).unwrap();
        let review = acquire(root.path(), "legacy review-watch", true).unwrap();
        for purpose in ["land run", "manual train/ship"] {
            let error = acquire(root.path(), purpose, false).err().unwrap();
            assert!(error.downcast_ref::<AdmissionBusy>().is_some());
            let text = error.to_string();
            assert!(text.contains(&format!("PID {}", std::process::id())));
            assert!(text.contains("start ") && text.contains("purpose legacy auto-train"));
        }
        let error = train(root.path(), false).err().unwrap();
        assert!(error.downcast_ref::<AdmissionBusy>().is_some());
        drop(worker);
        assert!(acquire(root.path(), "land run", false).is_err());
        drop(review);
        let land = acquire(root.path(), "land run", false).unwrap();
        assert!(acquire(root.path(), "legacy auto-train", true).is_err());
        drop(land);
        assert!(acquire(root.path(), "manual train/ship", false).is_ok());
    }
    #[test]
    fn story2857_process_contender() {
        // trace:STORY-2857 | ai:codex
        let inputs = aida_core::exec_context::Environment::capture();
        let Some(root) = inputs.get("AIDA_TEST_ADMISSION_ROOT") else {
            return;
        };
        let root = Path::new(root);
        if inputs.get("AIDA_TEST_ADMISSION_HANDOFF").is_some() {
            assert!(train(root, false).unwrap().is_none());
            return;
        }
        let error = if inputs.get("AIDA_TEST_ADMISSION_MANUAL").is_some() {
            crate::pr_train::pr_train_handler(&crate::pr_train::TrainOptions {
                prs: &[42],
                all_ready: false,
                dry_run: false,
                local_ci: None,
                max: 10,
                no_pull: true,
            })
            .err()
            .expect("manual train must refuse")
        } else {
            acquire(root, "land run", false)
                .err()
                .expect("land must refuse")
        };
        assert!(error.downcast_ref::<AdmissionBusy>().is_some(), "{error:#}");
        assert!(error.to_string().contains("purpose legacy auto-train"));
        assert!(error.to_string().contains("start "));
    }
    #[test]
    fn story2857_cross_process_worker_blocks_land_and_manual() {
        let root = tempfile::tempdir().unwrap();
        let _worker = acquire(root.path(), "legacy auto-train", true).unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        for manual in [false, true] {
            // Resolve the running test image, preserving the exact child fixture.
            // trace:STORY-2857 | ai:codex
            let mut command =
                std::process::Command::new(crate::aida_bin::current_executable_path().unwrap());
            command
                .args([
                    "--exact",
                    "admission_owner::tests::story2857_process_contender",
                    "--nocapture",
                ])
                .env("AIDA_TEST_ADMISSION_ROOT", root.path())
                .current_dir(root.path());
            if manual {
                command.env("AIDA_TEST_ADMISSION_MANUAL", "1");
            }
            let status = command.status().unwrap();
            assert!(status.success());
        }
    }
    #[test]
    fn story2857_admission_does_not_hold_train_run_lock() {
        let root = tempfile::tempdir().unwrap();
        let _worker = acquire(root.path(), "legacy auto-train", true).unwrap();
        // Admission is retained through scans/idle; train journal exclusion is separate.
        assert!(!crate::pr_train::admission_train_running(root.path()).unwrap());
        assert!(train(root.path(), true).unwrap().is_none());
        assert!(!root.path().join(".aida/train/run.lock").exists());
    }
    #[test]
    fn story2857_transition_requires_quiescent_children_and_readable_evidence() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".aida/cache")).unwrap();
        let path = root.path().join(".aida/cache/land-children.json");
        let book = crate::land::run::Book {
            children: vec![crate::land::run::Child {
                kind: crate::land::run::Kind::Train,
                prs: vec![42],
                head: String::new(),
                spec: None,
                merge_tree: None,
                identity: ProcIdentity::capture(std::process::id()).unwrap(),
                started_at: chrono::Utc::now().to_rfc3339(),
                log: String::new(),
            }],
            ..Default::default()
        };
        std::fs::write(&path, serde_json::to_vec(&book).unwrap()).unwrap();
        let error = acquire(root.path(), "legacy auto-train", true)
            .err()
            .unwrap();
        assert!(error.downcast_ref::<AdmissionBusy>().is_some());
        assert!(error.to_string().contains("prior land Train"));
        std::fs::write(&path, b"broken").unwrap();
        assert!(acquire(root.path(), "land run", false).is_err());
        std::fs::remove_file(path).unwrap();
        assert!(acquire(root.path(), "land run", false).is_ok());
    }
    #[test]
    fn story2857_land_hands_admission_to_its_direct_train_child() {
        let root = tempfile::tempdir().unwrap();
        let _land = acquire(root.path(), "land run", false).unwrap();
        // trace:STORY-2857 | ai:codex
        let status =
            std::process::Command::new(crate::aida_bin::current_executable_path().unwrap())
                .args([
                    "--exact",
                    "admission_owner::tests::story2857_process_contender",
                    "--nocapture",
                ])
                .env("AIDA_TEST_ADMISSION_ROOT", root.path())
                .env("AIDA_TEST_ADMISSION_HANDOFF", "1")
                .status()
                .unwrap();
        assert!(status.success());
    }
}
