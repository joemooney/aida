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
#[error("OutcomeUnknown: canonical transaction requires operator recovery at {}; see docs/advanced/store-redo-recovery.md", .0.display())]
pub struct OutcomeUnknown(pub PathBuf);
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
/// Separate from Git HEAD: canonical installs can precede publication.
pub(crate) fn cache_generation(root: &Path) -> Result<String> {
    let path = root.join(".aida/redo/cache-dirty");
    if !present(&path)? {
        return Ok(String::new());
    }
    Ok(hash(&read(&path)?))
}
// Read paths never quarantine or otherwise mutate journal evidence.
fn known_decision(path: &Path) -> Result<Decision> {
    decode(path).map_err(|_| OutcomeUnknown(path.to_path_buf()).into())
}
fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    replace_with_fault(path, bytes, &mut |_| Ok(()))
}
// Expose the record's pre-acceptance boundaries to the restart harness too.
// Temporary files are never decisions: only a directory-synced .pending name
// establishes acceptance, and canonical installation must come after it.
fn replace_with_fault(
    path: &Path,
    bytes: &[u8],
    fault: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    replace_in(path, path.parent().unwrap(), bytes, fault)
}
fn replace_in(
    path: &Path,
    staging: &Path,
    bytes: &[u8],
    fault: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let tmp = staging.join(format!(
        "{}.tmp-{}",
        path.file_stem().unwrap().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(bytes)?;
    fault("record_write")?;
    f.sync_all()?;
    fault("record_sync")?;
    fs::rename(&tmp, path)?;
    fault("record_rename")?;
    sync(path.parent().unwrap())?;
    if staging != path.parent().unwrap() {
        sync(staging)?;
    }
    Ok(())
}
// Only our UUID-suffixed staging names are disposable, never accepted records.
fn temporary(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("tmp-"))
        .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
}
fn existing_journal(root: &Path) -> Result<Option<PathBuf>> {
    let dir = root.join(".aida/redo");
    if !present(&dir)? {
        return Ok(None);
    }
    for path in [root.join(".aida"), dir.clone()] {
        let md = fs::symlink_metadata(path)?;
        ensure!(
            md.is_dir() && !md.file_type().is_symlink(),
            "unsafe transaction directory"
        );
    }
    let marker = dir.join("format");
    if !present(&marker)? {
        for entry in fs::read_dir(&dir)? {
            ensure!(
                temporary(&entry?.path()),
                "OutcomeUnknown: missing transaction format"
            );
        }
        return Ok(None);
    }
    ensure!(
        read(&marker)? == b"1\n",
        "unsupported canonical transaction format"
    );
    Ok(Some(dir))
}
fn journal(root: &Path) -> Result<PathBuf> {
    journal_with_fault(root, &mut |_| Ok(()))
}
fn journal_with_fault(root: &Path, fault: &mut impl FnMut(&str) -> Result<()>) -> Result<PathBuf> {
    if let Some(dir) = existing_journal(root)? {
        return Ok(dir);
    }
    // Do this only at initialization, before writing even the format temporary.
    if root.join(".git").exists() {
        crate::git_ops::ensure_store_redo_excluded(root)?;
    }
    let dir = root.join(".aida/redo");
    directory(&root.join(".aida"))?;
    directory(&dir)?;
    replace_with_fault(&dir.join("format"), b"1\n", &mut |point| {
        fault(&point.replacen("record_", "marker_", 1))
    })?;
    Ok(dir)
}
// Recovery owns the writer lock. Never follow directory links while removing
// staging debris left by older same-directory canonical replacements.
fn clean_temporaries(dir: &Path, recursive: bool) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_file() && temporary(&entry.path()) {
            fs::remove_file(entry.path())?;
            sync(dir)?;
        } else if recursive && kind.is_dir() {
            clean_temporaries(&entry.path(), true)?;
        }
    }
    Ok(())
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
        crate::object_store::object_path(&root.join("objects"), &i.spec_id)?;
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
        // Keep canonical staging bytes under the journal's Git exclusion even
        // if the process dies before rename. Both directories are synced.
        replace_in(
            &path,
            &root.join(".aida/redo"),
            i.after.as_bytes(),
            &mut |point| fault(&point.replacen("record_", "object_", 1)),
        )?;
        fault("install")?;
    }
    // Derived cache must not remain authoritative after an uncommitted install.
    // Readers compare this generation separately from Git HEAD. A second stamp
    // invalidates snapshots that raced the installation; neither stamp is the
    // transaction commit point.
    replace(
        &root.join(".aida/redo/cache-dirty"),
        uuid::Uuid::new_v4().to_string().as_bytes(),
    )?;
    fault("durable")?;
    Ok(())
}
/// Called once at the OUTERMOST shared lock acquisition, before any writer acts.
pub(crate) fn recover(root: &Path) -> Result<()> {
    recover_inner(root).map_err(|error| {
        if error.downcast_ref::<OutcomeUnknown>().is_some() {
            error
        } else {
            OutcomeUnknown(root.join(".aida/redo")).into()
        }
    })
}
fn recover_inner(root: &Path) -> Result<()> {
    let dir = root.join(".aida/redo");
    match fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
        Ok(_) => (),
    }
    let dir = journal(root)?;
    clean_temporaries(&dir, false)?;
    let objects = root.join("objects");
    if present(&objects)? {
        let md = fs::symlink_metadata(&objects)?;
        ensure!(
            md.is_dir() && !md.file_type().is_symlink(),
            "unsafe canonical directory"
        );
        clean_temporaries(&objects, true)?;
    }
    let mut pending = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|s| s == "unknown") {
            return Err(OutcomeUnknown(path).into());
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
            // A process may have died after renaming the fsynced record but
            // before syncing this directory. Make that visible decision durable
            // before recovery starts replacing canonical objects as well.
            sync(&dir)?;
            install(root, &d, &mut |_| Ok(()))?;
            fs::rename(&path, path.with_extension("done"))?;
            sync(&dir)
        })();
        if result.is_err() {
            // Preserve the decision, and fence future writers. Never discard it.
            fs::rename(&path, path.with_extension("unknown"))?;
            sync(&dir)?;
            return Err(OutcomeUnknown(path.with_extension("unknown")).into());
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
    let Some(dir) = existing_journal(root)? else {
        return Ok(None);
    };
    let path = dir.join(format!("{epoch}-{id}.done"));
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
    let Some(dir) = existing_journal(root)? else {
        return Ok(false);
    };
    let path = dir.join(format!("{epoch}-{id}.done"));
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
    let dir = journal_with_fault(root, fault)?;
    // Quota is per launch epoch. Retain old retry evidence without blocking
    // new launches; never evict an unresolved request.
    let mut retained = 0;
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(&format!("{}-", d.epoch)))
            && path
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
    replace_with_fault(&path, &bytes, fault).map_err(|_| OutcomeUnknown(path.clone()))?; // file AND directory fsynced before any canonical write
    let accepted = (|| -> Result<()> {
        fault("journal")?;
        install(root, &d, fault)?;
        fs::rename(&path, path.with_extension("done"))?;
        sync(&dir)?;
        fault("reply")?;
        Ok(())
    })();
    if accepted.is_err() {
        let done = path.with_extension("done");
        let diagnostic = if present(&done).unwrap_or(false) {
            done
        } else {
            path
        };
        return Err(OutcomeUnknown(diagnostic).into());
    }
    Ok(d.result)
}

#[cfg(test)]
mod tests;
