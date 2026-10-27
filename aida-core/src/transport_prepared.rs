//! Exact-byte, one-use WORK mutation under the enclosing admission and
//! allocator callback. No effect is permitted during construction/validation.
// trace:STORY-1742 | ai:codex
// trace:BUG-2017 | ai:codex
use super::{Admission, Snapshot};
use crate::git_ops::PreparedCommit;
use crate::work_binding::candidate::Sealed;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub(crate) struct FileEffect {
    pub(crate) path: String,
    pub(crate) before: Option<Vec<u8>>,
    pub(crate) after: Option<Vec<u8>>,
}

pub(crate) struct PreparedMutation<'scope> {
    root: PathBuf,
    effects: Vec<FileEffect>,
    counter: Sealed<'scope>,
    commit: PreparedCommit,
    admission: &'scope Admission,
}

/// Modes every prepared artifact is written with, whatever the process umask:
/// a WORK write must never leave the store in a shape the next one refuses.
// trace:STORY-1742 | ai:claude
const FILE_MODE: u32 = 0o644;
const DIR_MODE: u32 = 0o755;

/// Shared-write store files are refused, never silently accepted; the
/// message names the store-relative path and the fix.
fn shared_write(relative: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "WORK store path {relative} is group- or other-writable; enabled WORK intake writes \
         only owner-writable files. Run `chmod go-w <store>/{relative}` (or `chmod -R go-w \
         <store>` for the whole store) and retry. Nothing was changed."
    )
}

fn safe_parents(root: &Path, relative: &str) -> Result<()> {
    let mut path = root.to_owned();
    let components: Vec<_> = relative.split('/').collect();
    let mut walked = String::new();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        path.push(component);
        if !walked.is_empty() {
            walked.push('/');
        }
        walked.push_str(component);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } => {
                if meta.mode() & 0o022 != 0 {
                    return Err(shared_write(&walked));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            _ => bail!("Unsupported or changed WORK artifact directory"),
        }
    }
    Ok(())
}

/// The append-only oplog has its own per-record bound in the policy; every
/// other artifact keeps the ordinary per-file bound.
fn bound(relative: &str) -> usize {
    if relative == "oplog.yaml" {
        crate::transport_sanitization::MAX_OPLOG_BYTES
    } else {
        crate::transport_sanitization::MAX_BYTES
    }
}

/// Write `bytes` with exact modes: directories 0755, the file 0644, staged
/// beside the target and renamed into place.
fn write_exact(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(DIR_MODE)
                .create(parent)?;
        }
    }
    let tmp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp)?;
        // The create mode is masked by the umask; set it exactly.
        file.set_permissions(std::fs::Permissions::from_mode(FILE_MODE))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

pub(crate) fn read(root: &Path, relative: &str) -> Result<Option<Vec<u8>>> {
    safe_parents(root, relative)?;
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join(relative))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("WORK artifact is unavailable or unsafe"),
    };
    let meta = file.metadata()?;
    let limit = bound(relative);
    anyhow::ensure!(
        meta.is_file()
            && meta.nlink() == 1
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.len() <= limit as u64,
        "WORK artifact is unavailable or exceeds its bound"
    );
    if meta.mode() & 0o022 != 0 {
        return Err(shared_write(relative));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= limit, "WORK artifact exceeds its bound");
    Ok(Some(bytes))
}

impl<'scope> PreparedMutation<'scope> {
    pub(crate) fn prepare(
        root: &Path,
        effects: Vec<FileEffect>,
        counter: Sealed<'scope>,
        message: &str,
        admission: &'scope Admission,
    ) -> Result<Self> {
        anyhow::ensure!(
            !effects.is_empty() && effects.len() <= 10_000,
            "WORK mutation artifact limit exceeded or empty plan"
        );
        let snapshot = admission
            .snapshot()
            .context("WORK prepared admission is missing")?;
        let mut paths = BTreeMap::new();
        let mut total = 0usize;
        for effect in &effects {
            snapshot
                .policy
                .path_format(&effect.path)
                .map_err(|_| anyhow::anyhow!("WORK artifact path refused"))?;
            anyhow::ensure!(
                paths.insert(&effect.path, ()).is_none(),
                "Duplicate WORK artifact destination"
            );
            if let Some(bytes) = &effect.after {
                // The oplog's own ceiling is enforced by its policy format.
                if effect.path != "oplog.yaml" {
                    total = total
                        .checked_add(bytes.len())
                        .context("WORK artifact size overflow")?;
                    anyhow::ensure!(total <= 64 * 1_048_576, "WORK mutation byte limit exceeded");
                }
                // The push gate's own rule: policy covers what this write
                // adds, never history. trace:STORY-1742 | ai:claude
                snapshot
                    .policy
                    .change(&effect.path, effect.before.as_deref(), bytes)
                    .map_err(|category| {
                        anyhow::anyhow!("WORK prepared artifact refused: {category}")
                    })?;
            }
        }
        for (name, bytes) in counter.private_artifacts() {
            // These names are closed allocator-owned structural names, not
            // arbitrary paths. Even their fixed spellings honor deny tokens.
            anyhow::ensure!(
                !snapshot.policy.deny_tokens.iter().any(|token| name
                    .to_ascii_lowercase()
                    .contains(&token.to_ascii_lowercase())),
                "WORK private artifact name refused"
            );
            snapshot
                .policy
                .document("toml", bytes)
                .map_err(|category| anyhow::anyhow!("WORK prepared counter refused: {category}"))?;
        }
        // Allocation admission consumes the exact decoded rows that will be
        // written, not an earlier caller-owned model. The existing allocator
        // retains namespace ownership and global metadata-counter authority.
        let inventory = crate::object_store::list_objects_strict(&root.join("objects"))?;
        anyhow::ensure!(
            inventory.len() <= 10_000,
            "WORK allocation inventory limit exceeded"
        );
        let existing = inventory
            .into_iter()
            .map(|(id, _)| {
                let path = crate::object_store::relative_object_path(&id)?;
                let bytes = read(root, &path)?.context("WORK allocation inventory changed")?;
                serde_yaml::from_slice::<crate::Requirement>(&bytes).map_err(Into::into)
            })
            .collect::<Result<Vec<_>>>()?;
        let mut incoming = Vec::new();
        for effect in effects
            .iter()
            .filter(|effect| effect.path.starts_with("objects/"))
        {
            let bytes = effect
                .after
                .as_ref()
                .or(effect.before.as_ref())
                .context("Empty WORK object effect")?;
            let row: crate::Requirement = serde_yaml::from_slice(bytes)?;
            let id = row
                .spec_id
                .as_deref()
                .context("WORK object has no display identity")?;
            anyhow::ensure!(
                crate::object_store::relative_object_path(id)? == effect.path,
                "WORK object path does not match its admitted identity"
            );
            if effect.after.is_some() {
                incoming.push(row);
            }
        }
        counter.admit_rows(&incoming, &existing)?;
        for row in &incoming {
            if let Some(old) = existing.iter().find(|old| old.id == row.id) {
                anyhow::ensure!(
                    row.filed_at == old.filed_at,
                    "WORK intake cannot change existing write-once filing provenance"
                );
            }
        }
        for effect in effects
            .iter()
            .filter(|effect| effect.path == "metadata.yaml")
        {
            let before: serde_yaml::Value = serde_yaml::from_slice(
                effect
                    .before
                    .as_deref()
                    .context("WORK metadata initialization is not a prepared mutation")?,
            )?;
            let after: serde_yaml::Value = serde_yaml::from_slice(
                effect
                    .after
                    .as_deref()
                    .context("WORK metadata deletion is unsupported")?,
            )?;
            for key in [
                "next_spec_number",
                "next_feature_number",
                "prefix_counters",
                "meta_counters",
            ] {
                anyhow::ensure!(
                    before.get(key) == after.get(key),
                    "Global metadata counters cannot change in WORK intake"
                );
            }
            #[derive(serde::Deserialize)]
            struct Id {
                id: uuid::Uuid,
                #[serde(default)]
                spec_id: Option<String>,
            }
            fn ids(value: &serde_yaml::Value) -> Result<Vec<(uuid::Uuid, Option<String>)>> {
                let mut ids = Vec::new();
                for key in ["users", "teams"] {
                    if let Some(value) = value.get(key) {
                        for row in serde_yaml::from_value::<Vec<Id>>(value.clone())? {
                            ids.push((row.id, row.spec_id));
                        }
                    }
                }
                Ok(ids)
            }
            counter.admit_metadata_ids(&ids(&after)?, &ids(&before)?)?;
        }
        let commit = PreparedCommit::prepare(root, message, &snapshot.policy)?;
        let prepared = Self {
            root: root.canonicalize()?,
            effects,
            counter,
            commit,
            admission,
        };
        prepared.revalidate(snapshot)?;
        Ok(prepared)
    }

    fn revalidate(&self, snapshot: &Snapshot) -> Result<()> {
        self.admission.revalidate()?;
        self.counter.revalidate()?;
        self.commit.revalidate(&self.root, &snapshot.policy)?;
        for effect in &self.effects {
            anyhow::ensure!(
                read(&self.root, &effect.path)? == effect.before,
                "WORK artifact changed during preparation; no planned effects applied"
            );
        }
        Ok(())
    }

    pub(crate) fn apply(self) -> Result<()> {
        let snapshot = self
            .admission
            .snapshot()
            .context("WORK prepared admission is missing")?;
        // Last common check before the FIRST counter/object/oplog/ref effect.
        self.revalidate(snapshot)?;
        self.counter.apply()?;
        for effect in &self.effects {
            let path = self.root.join(&effect.path);
            let result = match &effect.after {
                Some(bytes) => write_exact(&path, bytes),
                None => match std::fs::remove_file(path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    result => result,
                },
            };
            if result.is_err() {
                bail!("Admitted WORK mutation may be partially persisted; inspect local state before retry");
            }
        }
        for effect in &self.effects {
            if crate::git_ops::add_all(&self.root, &effect.path).is_err() {
                bail!("Admitted WORK files persisted; staging failed before commit");
            }
        }
        let paths: Vec<_> = self
            .effects
            .iter()
            .map(|effect| effect.path.as_str())
            .collect();
        self.commit.commit(&self.root, &paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_unsafe_final_artifact_refuses_the_complete_batch_before_any_effect() {
        let root = super::super::tests::enrolled();
        std::fs::create_dir(root.path().join("objects")).unwrap();
        std::fs::write(root.path().join("metadata.yaml"), b"name: Generic\n").unwrap();
        let admission = Admission::enter(root.path()).unwrap();
        crate::work_binding::candidate::with_unspent(root.path(), |candidate| {
            let id = candidate.dispenser().next_id("BUG")?;
            let mut row =
                crate::Requirement::new("Generic failure".into(), "Synthetic reproduction".into());
            row.spec_id = Some(id.clone());
            let effects = vec![
                FileEffect {
                    path: crate::object_store::relative_object_path(&id)?,
                    before: None,
                    after: Some(serde_yaml::to_string(&row)?.into_bytes()),
                },
                FileEffect {
                    path: "metadata.yaml".into(),
                    before: Some(b"name: Generic\n".to_vec()),
                    after: Some(b"name: private-marker\n".to_vec()),
                },
            ];
            let error = PreparedMutation::prepare(
                root.path(),
                effects,
                candidate.seal()?,
                "add generic defect",
                &admission,
            )
            .err()
            .context("unsafe batch unexpectedly admitted")?;
            assert_eq!(
                error.to_string(),
                "WORK prepared artifact refused: reviewed deny token"
            );
            Ok(())
        })
        .unwrap();
        assert!(!crate::work_binding::counter_path(root.path())
            .unwrap()
            .exists());
        assert!(crate::work_binding::load(root.path()).unwrap().is_none());
        assert!(
            crate::object_store::list_objects(&root.path().join("objects"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            std::fs::read(root.path().join("metadata.yaml")).unwrap(),
            b"name: Generic\n"
        );
    }
}
