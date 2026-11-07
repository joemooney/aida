//! Durable canonical-object decisions. Recovery never re-runs operation logic.
//! trace:TASK-3223 | ai:codex
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const LIMIT: usize = 8 * 1024 * 1024;
#[derive(Debug, thiserror::Error)]
#[error("OutcomeUnknown: canonical transaction requires operator recovery")]
pub struct OutcomeUnknown;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub spec_id: String,
    pub before: String,
    pub after: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub version: u32,
    pub binding: String,
    pub epoch: uuid::Uuid,
    pub request_id: uuid::Uuid,
    pub digest: String,
    pub images: Vec<Image>,
    pub result: serde_json::Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    checksum: String,
    decision: Decision,
}
fn encode(d: &Decision) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&Record {
        checksum: hash(&serde_json::to_vec(d)?),
        decision: d.clone(),
    })?)
}
fn decode(path: &Path) -> Result<Decision> {
    let record: Record = serde_json::from_slice(&read(path)?)?;
    ensure!(
        record.checksum == hash(&serde_json::to_vec(&record.decision)?),
        "corrupt decision checksum"
    );
    validate(&record.decision)?;
    Ok(record.decision)
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn sync(dir: &Path) -> Result<()> {
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}
fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(md) => ensure!(
            md.is_dir() && !md.file_type().is_symlink(),
            "unsafe transaction directory"
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            sync(path.parent().unwrap())?;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
fn read(path: &Path) -> Result<Vec<u8>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let md = file.metadata()?;
    ensure!(
        md.is_file() && md.nlink() == 1 && md.len() <= LIMIT as u64,
        "invalid transaction file"
    );
    let mut bytes = Vec::new();
    file.take(LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= LIMIT, "oversized transaction file");
    Ok(bytes)
}
pub fn object_bytes(root: &Path, sid: &str) -> Result<Vec<u8>> {
    let path = crate::object_store::object_path(&root.join("objects"), sid)?;
    let mut dir = path.parent();
    while let Some(p) = dir {
        if p == root {
            break;
        }
        let md = fs::symlink_metadata(p)?;
        ensure!(
            md.is_dir() && !md.file_type().is_symlink(),
            "unsafe canonical directory"
        );
        dir = p.parent();
    }
    read(&path)
}
fn known_decision(path: &Path) -> Result<Decision> {
    match decode(path) {
        Ok(d) => Ok(d),
        Err(_) => {
            let _ = fs::rename(path, path.with_extension("unknown"));
            let _ = sync(path.parent().unwrap());
            Err(OutcomeUnknown.into())
        }
    }
}
fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    sync(path.parent().unwrap())
}
fn journal(root: &Path) -> Result<PathBuf> {
    if root.join(".git").exists() {
        crate::git_ops::ensure_store_redo_excluded(root)?;
    }
    let dir = root.join(".aida/redo");
    directory(&root.join(".aida"))?;
    directory(&dir)?;
    let marker = dir.join("format");
    if !present(&marker)? {
        ensure!(
            fs::read_dir(&dir)?.next().is_none(),
            "OutcomeUnknown: missing transaction format"
        );
        replace(&marker, b"1\n")?;
    }
    ensure!(
        read(&marker)? == b"1\n",
        "unsupported canonical transaction format"
    );
    Ok(dir)
}
fn name(d: &Decision) -> String {
    format!("{}-{}", d.epoch, d.request_id)
}
fn validate(d: &Decision) -> Result<()> {
    ensure!(
        d.version == 1 && d.binding.len() <= 4096 && d.digest.len() == 64 && d.images.len() <= 64,
        "invalid decision"
    );
    ensure!(
        serde_json::to_vec(&d.result)?.len() <= 60000,
        "oversized result"
    );
    let mut ids = std::collections::HashSet::new();
    for i in &d.images {
        ensure!(
            ids.insert(&i.spec_id) && i.before.len() == 64,
            "invalid image"
        );
        crate::object_store::relative_object_path(&i.spec_id)?;
        let row: crate::Requirement = serde_yaml::from_str(&i.after)?;
        ensure!(
            row.spec_id.as_deref() == Some(&i.spec_id),
            "image identity mismatch"
        );
    }
    Ok(())
}
fn install(root: &Path, d: &Decision, fault: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
    // Check ALL preconditions before the first replacement, including recovery.
    for i in &d.images {
        let path = crate::object_store::object_path(&root.join("objects"), &i.spec_id)?;
        let now = object_bytes(root, &i.spec_id)?;
        ensure!(
            hash(&now) == i.before || now == i.after.as_bytes(),
            "unexpected canonical bytes"
        );
    }
    replace(
        &root.join(".aida/redo/cache-dirty"),
        uuid::Uuid::new_v4().to_string().as_bytes(),
    )?;
    for i in &d.images {
        let path = crate::object_store::object_path(&root.join("objects"), &i.spec_id)?;
        replace(&path, i.after.as_bytes())?;
        fault("install")?;
    }
    // Derived cache must not remain authoritative after an uncommitted install.
    // Readers already compare canonical contents/mtime; this marker also lets
    // future cache routing recognize recovered changes without treating Git HEAD
    // or a cache acknowledgement as the transaction commit point.
    replace(
        &root.join(".aida/redo/cache-dirty"),
        uuid::Uuid::new_v4().to_string().as_bytes(),
    )?;
    fault("durable")?;
    Ok(())
}
/// Called once at the OUTERMOST shared lock acquisition, before any writer acts.
pub(crate) fn recover(root: &Path) -> Result<()> {
    recover_inner(root).map_err(|_| OutcomeUnknown.into())
}
fn recover_inner(root: &Path) -> Result<()> {
    let dir = root.join(".aida/redo");
    match fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
        Ok(_) => (),
    }
    let dir = journal(root)?;
    let mut pending = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|s| s == "unknown") {
            return Err(OutcomeUnknown.into());
        }
        if path.extension().is_some_and(|s| s == "pending") {
            pending.push(path);
        }
    }
    for path in pending {
        let result = (|| -> Result<()> {
            let d = decode(&path)?;
            ensure!(
                path.file_stem().and_then(|s| s.to_str()) == Some(name(&d).as_str()),
                "decision name mismatch"
            );
            install(root, &d, &mut |_| Ok(()))?;
            fs::rename(&path, path.with_extension("done"))?;
            sync(&dir)
        })();
        if result.is_err() {
            // Preserve the decision, and fence future writers. Never discard it.
            fs::rename(&path, path.with_extension("unknown"))?;
            sync(&dir)?;
            return Err(OutcomeUnknown.into());
        }
    }
    Ok(())
}
pub fn lookup(
    root: &Path,
    binding: &str,
    epoch: uuid::Uuid,
    id: uuid::Uuid,
    digest: &str,
) -> Result<Option<serde_json::Value>> {
    ensure!(
        super::store_write_lock_held(root),
        "transaction lookup requires store lock"
    );
    let path = journal(root)?.join(format!("{epoch}-{id}.done"));
    if !present(&path)? {
        return Ok(None);
    }
    let d = known_decision(&path)?;
    ensure!(
        d.binding == binding && d.epoch == epoch && d.request_id == id && d.digest == digest,
        "Conflict"
    );
    Ok(Some(d.result))
}
/// Durable status scoped to the fixed launch binding.
pub fn status(root: &Path, binding: &str, epoch: uuid::Uuid, id: uuid::Uuid) -> Result<bool> {
    ensure!(
        super::store_write_lock_held(root),
        "status requires store lock"
    );
    let path = journal(root)?.join(format!("{epoch}-{id}.done"));
    if !present(&path)? {
        return Ok(false);
    }
    let d = known_decision(&path)?;
    ensure!(
        d.binding == binding && d.epoch == epoch && d.request_id == id,
        "Conflict"
    );
    Ok(true)
}
pub fn commit(root: &Path, d: Decision) -> Result<serde_json::Value> {
    commit_with_fault(root, d, &mut |_| Ok(()))
}
fn commit_with_fault(
    root: &Path,
    d: Decision,
    fault: &mut impl FnMut(&str) -> Result<()>,
) -> Result<serde_json::Value> {
    ensure!(
        super::store_write_lock_held(root),
        "transaction decision requires store lock"
    );
    validate(&d)?;
    if let Some(r) = lookup(root, &d.binding, d.epoch, d.request_id, &d.digest)? {
        return Ok(r);
    }
    let dir = journal(root)?;
    // Retain all durable retry evidence; never evict an unresolved request.
    let mut retained = 0;
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|s| s == "pending" || s == "done" || s == "unknown")
        {
            retained += 1;
        }
    }
    ensure!(retained < 4096, "Busy");
    let bytes = encode(&d)?;
    ensure!(bytes.len() <= LIMIT, "decision too large");
    let path = dir.join(format!("{}.pending", name(&d)));
    ensure!(!present(&path)?, "OutcomeUnknown");
    replace(&path, &bytes).map_err(|_| OutcomeUnknown)?; // file AND directory fsynced before any canonical write
    let accepted = (|| -> Result<()> {
        fault("journal")?;
        install(root, &d, fault)?;
        fs::rename(&path, path.with_extension("done"))?;
        sync(&dir)?;
        fault("reply")?;
        Ok(())
    })();
    if accepted.is_err() {
        return Err(OutcomeUnknown.into());
    }
    Ok(d.result)
}

#[cfg(test)]
mod tests;
