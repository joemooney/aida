//! Cooperative admission exclusion. Kernel flock is the exclusion authority.
//! Legacy review/train controllers share a mode; land and manual runs exclude it.
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
        // Remove our diagnostic while still holding exclusion. Never unlink locks.
        let _ = std::fs::remove_file(&self.receipt);
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
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    let serial = open(&dir.join("receipts.lock"))?;
    serial.lock_exclusive()?;
    let rows = receipts(&dir)?;
    let lock = open(&dir.join("mode.lock"))?;
    let result = if legacy {
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
    if !legacy {
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
    if crate::pr_train::admission_train_running(root)? {
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
    acquire(root, "manual train/ship", false).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let Ok(root) = std::env::var("AIDA_TEST_ADMISSION_ROOT") else {
            return;
        };
        let root = Path::new(&root);
        if std::env::var_os("AIDA_TEST_ADMISSION_HANDOFF").is_some() {
            assert!(train(root, false).unwrap().is_none());
            return;
        }
        let error = if std::env::var_os("AIDA_TEST_ADMISSION_MANUAL").is_some() {
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
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
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
        let status = std::process::Command::new(std::env::current_exe().unwrap())
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
