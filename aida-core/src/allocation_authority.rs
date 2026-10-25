//! Clone-local allocation authority. Shared store content never grants authority.
// trace:BUG-2017 | ai:codex
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "aida-allocation.toml";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationMode {
    pub version: u32,
    pub authoritative: bool,
}

/// Resolve aliases before policy discovery, including a not-yet-created leaf.
fn destination(path: &Path) -> Result<PathBuf> {
    let mut existing = std::path::absolute(path)?;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(&existing) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    existing
                        .file_name()
                        .context("invalid allocation destination")?
                        .to_os_string(),
                );
                if !existing.pop() {
                    bail!("cannot resolve allocation destination")
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    let mut resolved = existing.canonicalize()?;
    for part in missing.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

/// Resolve only filesystem Git identity, never process Git overrides or store config.
/// A linked code/store worktree shares its common directory with the main clone.
pub fn common_dir(from: &Path) -> Result<PathBuf> {
    let start = destination(from)?;
    for root in start.ancestors() {
        let dotgit = root.join(".git");
        let meta = match std::fs::symlink_metadata(&dotgit) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let gitdir = if meta.is_dir() {
            dotgit
        } else if meta.is_file() && meta.len() < 4096 {
            let text = std::fs::read_to_string(&dotgit)?;
            let target = text
                .trim()
                .strip_prefix("gitdir: ")
                .context("invalid clone Git identity")?;
            root.join(target)
        } else {
            bail!("invalid clone Git identity")
        };
        let common = gitdir.join("commondir");
        return match std::fs::read_to_string(common) {
            Ok(text) if !text.trim().is_empty() && text.len() < 4096 => {
                Ok(gitdir.join(text.trim()).canonicalize()?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(gitdir.canonicalize()?),
            _ => bail!("invalid clone common Git directory"),
        };
    }
    bail!("allocation requires a clone-local Git identity")
}

pub fn resolve(root: &Path) -> Result<AllocationMode> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let path = common_dir(root)?.join(CONFIG_FILE);
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AllocationMode {
                version: 1,
                authoritative: false,
            })
        }
        Err(_) => bail!("allocation setting could not be safely read"),
    };
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o022 != 0
        || meta.len() > 4096
    {
        bail!("allocation setting must be a bounded owned regular file without shared writes")
    }
    let mut text = String::new();
    file.by_ref().take(4097).read_to_string(&mut text)?;
    if text.len() > 4096 {
        bail!("allocation setting exceeds its bound")
    }
    let mode: AllocationMode =
        toml::from_str(&text).map_err(|_| anyhow::anyhow!("invalid allocation setting"))?;
    if mode.version != 1 {
        bail!("unsupported allocation setting version")
    }
    Ok(mode)
}

/// A file-only serializer outside any Git clone is not an allocation writer.
/// Permission/identity errors in a clone remain errors, never authority.
pub fn in_git_clone(root: &Path) -> Result<bool> {
    let resolved = destination(root)?;
    for p in resolved.ancestors() {
        match std::fs::symlink_metadata(p.join(".git")) {
            Ok(_) => return Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(false)
}

pub fn require_global(root: &Path) -> Result<()> {
    if !resolve(root)?.authoritative {
        bail!("global ID/block allocation requires an explicitly authoritative clone; this clone may allocate only node-local IDs")
    }
    Ok(())
}

pub fn effective_policy(
    root: &Path,
    configured: crate::IdFormatPolicy,
) -> Result<crate::IdFormatPolicy> {
    if resolve(root)?.authoritative {
        return Ok(configured);
    }
    if configured == crate::IdFormatPolicy::BlocksOnly {
        require_global(root)?;
    }
    Ok(crate::IdFormatPolicy::NodeAwareOnly)
}

fn node(root: &Path) -> Result<String> {
    let node = crate::NodeConfig::load(&root.join(".aida/node.toml"))?.node_id;
    if node.is_empty()
        || node.len() > 128
        || !node.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        bail!("node-local allocation requires a valid explicit node binding")
    }
    Ok(node)
}

fn local_id(id: &str, node: &str) -> bool {
    let Some((prefix, sequence)) = id.rsplit_once(&format!("-{node}-")) else {
        return false;
    };
    !prefix.is_empty()
        && prefix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '$' || c == '_')
        && !sequence.is_empty()
        && sequence.chars().all(|c| c.is_ascii_digit())
        && sequence.parse::<u32>().is_ok_and(|n| n > 0)
}

/// Admit identity changes before any object/metadata writes. Imported existing
/// global identities may be edited, but a new UUID or new global alias refuses.
pub fn admit(
    root: &Path,
    incoming: &[crate::Requirement],
    existing: &[crate::Requirement],
) -> Result<()> {
    if resolve(root)?.authoritative {
        return Ok(());
    }
    let old: std::collections::HashMap<_, _> = existing.iter().map(|r| (r.id, r)).collect();
    let mut binding = None;
    for req in incoming {
        let prior = old.get(&req.id);
        for (id, previous) in [
            (
                req.spec_id.as_deref(),
                prior.and_then(|r| r.spec_id.as_deref()),
            ),
            (
                req.agreed_id.as_deref(),
                prior.and_then(|r| r.agreed_id.as_deref()),
            ),
        ] {
            if id == previous && prior.is_some() {
                continue;
            }
            if let Some(id) = id {
                let binding = match &binding {
                    Some(b) => b,
                    None => binding.insert(node(root)?),
                };
                if !local_id(id, binding) {
                    bail!("new or changed global display IDs refuse on a non-authoritative clone; nothing was written")
                }
            } else if previous.is_none() && req.spec_id.is_none() {
                bail!("node-local allocation failed; requirement has no display ID")
            }
        }
    }
    Ok(())
}

/// Metadata identities ($USER/$TEAM) share the same allocation boundary.
pub fn admit_meta_ids(
    root: &Path,
    incoming: &[(uuid::Uuid, Option<String>)],
    existing: &[(uuid::Uuid, Option<String>)],
) -> Result<()> {
    if resolve(root)?.authoritative {
        return Ok(());
    }
    for (uuid, id) in incoming {
        if let Some(id) = id {
            if existing
                .iter()
                .any(|(old_uuid, old_id)| old_uuid == uuid && old_id.as_ref() == Some(id))
            {
                continue;
            }
            if !local_id(id, &node(root)?) {
                bail!("new global metadata IDs refuse on a non-authoritative clone")
            }
        }
    }
    Ok(())
}

/// Registry serializers cannot mint claims through alternate paths or aliases.
pub fn admit_registry_write(path: &Path) -> Result<()> {
    let resolved = destination(path)?;
    let parent = resolved.parent().context("invalid registry destination")?;
    if in_git_clone(parent)? {
        require_global(parent)?;
    }
    Ok(())
}

/// File-only exports outside Git retain their serialization semantics. Direct
/// object writers inside a clone share the same admission as the backend.
pub fn admit_object_file(objects: &Path, req: &crate::Requirement) -> Result<()> {
    let resolved_objects = destination(objects)?;
    let root = resolved_objects.parent().context("invalid object root")?;
    if !in_git_clone(root)? {
        return Ok(());
    }
    if resolve(root)?.authoritative {
        return Ok(());
    }
    let existing = crate::object_store::list_objects(objects)?
        .into_iter()
        .map(|(sid, _)| crate::object_store::read_object(objects, &sid))
        .collect::<Result<Vec<_>>>()?;
    admit(root, std::slice::from_ref(req), &existing)
}

/// Direct file dispensers under a clone cannot spend centralized counters.
pub fn admit_file_dispenser(path: &Path, mode: &crate::dispenser::IdMode) -> Result<()> {
    let resolved = destination(path)?;
    let path = resolved.as_path();
    let parent = path.parent().context("invalid dispenser location")?;
    let root = if parent.file_name().and_then(|s| s.to_str()) == Some(".aida") {
        parent.parent().context("invalid dispenser location")?
    } else {
        parent
    };
    if !in_git_clone(root)? {
        return Ok(());
    }
    if resolve(root)?.authoritative {
        return Ok(());
    }
    if mode
        != &(crate::dispenser::IdMode::Distributed {
            node_id: node(root)?,
        })
    {
        bail!("non-authoritative clone requires its bound node-local dispenser")
    }
    Ok(())
}

/// Infallible legacy model APIs receive an erroring dispenser rather than an
/// absent dispenser. They can never enter the internal-global-counter fallback.
struct CloneDispenser {
    root: PathBuf,
    inner: Option<crate::models::DispenserHandle>,
}
impl CloneDispenser {
    fn inner(&self) -> Result<crate::models::DispenserHandle> {
        let mode = resolve(&self.root)?;
        if mode.authoritative {
            return self
                .inner
                .clone()
                .context("authoritative allocator has no configured dispenser");
        }
        let binding = node(&self.root)?;
        if let Some(inner) = &self.inner {
            if inner.state()?.mode != (crate::dispenser::IdMode::Distributed { node_id: binding }) {
                bail!("dispenser does not match this clone's node binding")
            }
            return Ok(inner.clone());
        }
        Ok(crate::models::DispenserHandle(std::sync::Arc::new(
            crate::dispenser::FileDispenser::open(
                self.root.join(".aida/dispenser.toml"),
                crate::dispenser::IdMode::Distributed { node_id: binding },
            )?,
        )))
    }
}
impl crate::dispenser::Dispenser for CloneDispenser {
    fn admit_requirement(&self, req: &crate::Requirement) -> Result<()> {
        if resolve(&self.root)?.authoritative {
            return Ok(());
        }
        if req.spec_id.is_none() && req.agreed_id.is_none() {
            return Ok(());
        }
        let objects = self.root.join("objects");
        let existing = crate::object_store::list_objects(&objects)?
            .into_iter()
            .map(|(sid, _)| crate::object_store::read_object(&objects, &sid))
            .collect::<Result<Vec<_>>>()?;
        // An absent spec_id will be generated after admission; check only
        // caller-supplied identities here, then validate the final row at save.
        if req.spec_id.is_none() && req.agreed_id.is_none() {
            resolve(&self.root)?;
            return Ok(());
        }
        let mut probe = req.clone();
        if probe.spec_id.is_none() {
            probe.spec_id = Some(format!("REQ-{}-1", node(&self.root)?));
        }
        admit(&self.root, &[probe], &existing)
    }
    fn format_id(&self, prefix: &str, seq: u32) -> Result<String> {
        if prefix.is_empty()
            || prefix.len() > 128
            || !prefix
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '$' || c == '_')
        {
            bail!("invalid node-local allocation prefix")
        }
        let inner = self.inner()?;
        inner.format_id(prefix, seq)
    }
    fn next_id(&self, prefix: &str) -> Result<String> {
        self.format_id(prefix, 1)?;
        let seq = self.next(prefix)?;
        self.format_id(prefix, seq)
    }
    fn next(&self, prefix: &str) -> Result<u32> {
        self.inner()?.next(prefix)
    }
    fn peek(&self, prefix: &str) -> Result<u32> {
        self.inner()?.peek(prefix)
    }
    fn state(&self) -> Result<crate::dispenser::DispenserState> {
        self.inner()?.state()
    }
}
pub fn dispenser(
    root: &Path,
    inner: Option<crate::models::DispenserHandle>,
) -> Option<crate::models::DispenserHandle> {
    if resolve(root).is_ok_and(|m| m.authoritative) {
        return inner;
    }
    Some(crate::models::DispenserHandle(std::sync::Arc::new(
        CloneDispenser {
            root: root.into(),
            inner,
        },
    )))
}

/// Existing CRUD fixtures explicitly model HOME; never enroll a real clone.
#[cfg(test)]
pub(crate) fn home_fixture_backend(root: &Path) -> Result<crate::GitBackend> {
    let resolved = destination(root)?;
    anyhow::ensure!(
        resolved.starts_with(std::env::temp_dir().canonicalize()?),
        "fixture authority requires a temporary repository"
    );
    std::fs::create_dir_all(root)?;
    if common_dir(root).is_err() {
        crate::git_ops::init(root)?;
    }
    let common = common_dir(root)?;
    std::fs::write(common.join(CONFIG_FILE), "version=1\nauthoritative=true\n")?;
    crate::GitBackend::new(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DatabaseBackend, GitBackend, Requirement};
    fn fixture(authoritative: bool) -> (tempfile::TempDir, GitBackend) {
        let dir = tempfile::tempdir().unwrap();
        crate::git_ops::init(dir.path()).unwrap();
        if authoritative {
            std::fs::write(
                dir.path().join(".git").join(CONFIG_FILE),
                "version = 1\nauthoritative = true\n",
            )
            .unwrap();
        }
        let aida = dir.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(
            aida.join("node.toml"),
            "node_id = '7'\nuser_id = 0\nhostname = ''\nregistered_at = '2026-01-01T00:00:00Z'\n",
        )
        .unwrap();
        let backend = GitBackend::new(dir.path()).unwrap().with_auto_commit(false);
        (dir, backend)
    }
    #[test]
    fn bug_2017_missing_is_local_malformed_and_shared_settings_refuse() {
        let (dir, _) = fixture(false);
        assert!(!resolve(dir.path()).unwrap().authoritative);
        assert!(require_global(dir.path()).is_err());
        std::fs::create_dir_all(dir.path().join("registry")).unwrap();
        std::fs::write(
            dir.path().join("registry").join(CONFIG_FILE),
            "version=1\nauthoritative=true",
        )
        .unwrap();
        assert!(!resolve(dir.path()).unwrap().authoritative);
        for text in [
            "",
            "version=1",
            "version=2\nauthoritative=true",
            "version=1\nauthoritative='true'",
            "version=1\nauthoritative=true\nunknown=0",
        ] {
            std::fs::write(dir.path().join(".git").join(CONFIG_FILE), text).unwrap();
            assert!(resolve(dir.path()).is_err());
        }
    }
    #[test]
    fn bug_2017_work_backend_and_bulk_only_allocate_local() {
        let (dir, backend) = fixture(false);
        let mut req = Requirement::new("generic defect".into(), "fixture".into());
        req.req_type = crate::RequirementType::Bug;
        let first = backend.add_requirement(req).unwrap();
        assert_eq!(first.spec_id.as_deref(), Some("BUG-7-001"));
        let mut writer = backend.bulk_writer().unwrap();
        let second = writer
            .add(Requirement::new("bulk".into(), "fixture".into()))
            .unwrap();
        assert!(second.spec_id.as_deref().unwrap().contains("-7-"));
        writer.finish("fixture bulk import").unwrap();
        assert!(!dir.path().join("registry/blocks.yaml").exists());
        assert_eq!(
            backend.load().unwrap().next_spec_number,
            crate::RequirementsStore::new().next_spec_number
        );
    }
    #[test]
    fn bug_2017_explicit_global_and_agreed_ids_refuse_without_counter_or_object_write() {
        let (dir, backend) = fixture(false);
        for agreed in [false, true] {
            let mut req = Requirement::new("refused".into(), "fixture".into());
            if agreed {
                req.agreed_id = Some("BUG-42".into())
            } else {
                req.spec_id = Some("BUG-42".into())
            }
            assert!(backend.add_requirement(req.clone()).is_err());
            assert!(backend.bulk_writer().unwrap().add(req).is_err());
            assert!(!dir.path().join(".aida/dispenser.toml").exists());
            assert!(!dir.path().join("metadata.yaml").exists());
            assert!(
                crate::object_store::list_objects(&dir.path().join("objects"))
                    .unwrap()
                    .is_empty()
            );
        }
    }
    #[test]
    fn bug_2017_save_import_repair_and_transaction_cannot_mint_global() {
        let (_dir, backend) = fixture(false);
        let mut store = crate::RequirementsStore::new();
        let mut req = Requirement::new("refused".into(), "fixture".into());
        req.spec_id = Some("BUG-42".into());
        store.requirements.push(req.clone());
        assert!(backend.save(&store).is_err());
        assert!(backend.save_imported(&store).is_err());
        assert!(backend
            .update_atomically(|s| s.requirements.push(req))
            .is_err());
        assert!(backend.load().unwrap().requirements.is_empty());
    }
    #[test]
    fn bug_2017_existing_global_uuid_edit_allowed_new_global_alias_refused() {
        let (dir, backend) = fixture(true);
        let mut req = Requirement::new("existing".into(), "fixture".into());
        req.spec_id = Some("BUG-42".into());
        let mut req = backend.add_requirement(req).unwrap();
        std::fs::remove_file(dir.path().join(".git").join(CONFIG_FILE)).unwrap();
        req.title = "edited".into();
        backend.update_requirement(&req).unwrap();
        req.agreed_id = Some("BUG-43".into());
        assert!(backend.update_requirement(&req).is_err());
        assert!(backend
            .get_requirement(&req.id)
            .unwrap()
            .unwrap()
            .agreed_id
            .is_none());
    }
    #[test]
    fn bug_2017_missing_binding_and_dispenser_failure_never_use_global_fallback() {
        let (dir, backend) = fixture(false);
        std::fs::remove_file(dir.path().join(".aida/node.toml")).unwrap();
        assert!(backend
            .add_requirement(Requirement::new("missing".into(), "fixture".into()))
            .is_err());
        assert!(!dir.path().join("metadata.yaml").exists());
        assert!(!dir.path().join(".aida/dispenser.toml").exists());
        let (dir, backend) = fixture(false);
        std::fs::write(dir.path().join(".aida/dispenser.toml"), "invalid").unwrap();
        assert!(backend
            .add_requirement(Requirement::new("broken".into(), "fixture".into()))
            .is_err());
        assert!(!dir.path().join("metadata.yaml").exists());
    }
    #[test]
    fn bug_2017_alternate_dispenser_and_symlink_object_destination_cannot_bypass() {
        use crate::dispenser::{Dispenser, FileDispenser, IdMode};
        let (dir, _) = fixture(false);
        assert!(
            FileDispenser::open(dir.path().join(".aida/alternate.toml"), IdMode::Centralized)
                .is_err()
        );
        assert!(!dir.path().join(".aida/alternate.toml").exists());
        let outside = tempfile::tempdir().unwrap();
        let alias = outside.path().join("object-alias");
        std::os::unix::fs::symlink(dir.path().join("objects"), &alias).unwrap();
        let mut req = Requirement::new("refused".into(), "fixture".into());
        req.spec_id = Some("BUG-99".into());
        assert!(crate::object_store::write_object(&alias, &req).is_err());
        assert!(crate::object_store::write_object_if_changed(&alias, &req).is_err());
        assert!(
            crate::object_store::list_objects(&dir.path().join("objects"))
                .unwrap()
                .is_empty()
        );
        let local = FileDispenser::open(
            dir.path().join(".aida/alternate.toml"),
            IdMode::Distributed {
                node_id: "7".into(),
            },
        )
        .unwrap();
        assert_eq!(local.next_id("BUG").unwrap(), "BUG-7-001");
    }
    #[test]
    fn bug_2017_invalid_prefix_and_wrong_dispenser_mode_refuse_before_counters() {
        use crate::dispenser::{IdMode, MemoryDispenser};
        let (dir, backend) = fixture(false);
        let mut req = Requirement::new("bad prefix".into(), "fixture".into());
        req.prefix_override = Some("BUG-42".into());
        assert!(backend.add_requirement(req).is_err());
        assert!(!dir.path().join(".aida/dispenser.toml").exists());
        let memory = std::sync::Arc::new(MemoryDispenser::new(IdMode::Centralized));
        let backend = backend.with_dispenser(crate::models::DispenserHandle(memory.clone()));
        assert!(backend
            .add_requirement(Requirement::new("wrong mode".into(), "fixture".into()))
            .is_err());
        assert!(crate::dispenser::Dispenser::state(memory.as_ref())
            .unwrap()
            .sequences
            .is_empty());
        assert!(!dir.path().join("metadata.yaml").exists());
    }
    #[test]
    fn bug_2017_meta_ids_and_global_counter_repair_refuse_without_persistence() {
        let (dir, backend) = fixture(false);
        assert!(backend
            .update_atomically(|s| {
                s.next_spec_number += 1;
            })
            .is_err());
        assert!(!dir.path().join("metadata.yaml").exists());
        assert!(backend
            .update_atomically(|s| {
                let user = crate::models::User::new_with_spec_id(
                    "fixture".into(),
                    "".into(),
                    "fixture".into(),
                    "$USER-1".into(),
                );
                s.users.push(user);
            })
            .is_err());
        assert!(!dir.path().join("metadata.yaml").exists());
    }
    #[test]
    fn bug_2017_registry_aliases_and_alternate_locations_refuse_before_writes() {
        let (dir, _) = fixture(false);
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("registry")).unwrap();
        let alias = outside.path().join("registry-alias");
        std::os::unix::fs::symlink(dir.path().join("registry"), &alias).unwrap();
        for path in [dir.path().join("blocks.yaml"), alias.join("blocks.yaml")] {
            assert!(crate::BlockRegistry::default().save(&path).is_err());
            assert!(!path.exists());
        }
        let target = dir.path().join("registry/nodes.toml");
        std::fs::write(&target, "unchanged").unwrap();
        let leaf = outside.path().join("nodes-link.toml");
        std::os::unix::fs::symlink(&target, &leaf).unwrap();
        assert!(crate::NodeRegistry::default().save(&leaf).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "unchanged");
        assert!(crate::NodeRegistry::default()
            .save(&alias.join("other-nodes.toml"))
            .is_err());
    }
    #[test]
    fn bug_2017_sqlite_dispenser_refuses_before_database_creation() {
        let (dir, _) = fixture(false);
        let path = dir.path().join(".aida/other.db");
        assert!(crate::dispenser::SqliteDispenser::open(
            path.clone(),
            crate::dispenser::IdMode::Centralized
        )
        .is_err());
        assert!(!path.exists());
        assert!(!path.with_extension("db-wal").exists());
    }
    #[test]
    fn bug_2017_legacy_assignment_and_preview_use_bound_local_ids() {
        let (_dir, backend) = fixture(false);
        let mut store = backend.load().unwrap();
        let before = store.next_spec_number;
        store
            .requirements
            .push(Requirement::new("legacy".into(), "fixture".into()));
        assert!(store.peek_next_spec_id().contains("-7-"));
        store.assign_spec_ids();
        assert_eq!(store.requirements[0].spec_id.as_deref(), Some("SPEC-7-001"));
        assert_eq!(store.next_spec_number, before);
        backend.save(&store).unwrap();
    }
    #[test]
    fn bug_2017_core_canonical_promotion_refuses_before_registry_creation() {
        let (dir, _) = fixture(false);
        assert!(crate::git_ops::merge_gate(dir.path()).is_err());
        assert!(!dir.path().join("registry").exists());
    }
    #[test]
    fn bug_2017_cached_global_blocks_and_explicit_blocks_only_are_not_authority() {
        let (dir, _) = fixture(false);
        assert_eq!(
            effective_policy(dir.path(), crate::IdFormatPolicy::BlocksThenFallback).unwrap(),
            crate::IdFormatPolicy::NodeAwareOnly
        );
        assert!(effective_policy(dir.path(), crate::IdFormatPolicy::BlocksOnly).is_err());
        let mut blocks = crate::BlockRegistry::default();
        blocks.claim_block("7".into(), "".into(), "".into(), "BUG".into(), 100);
        assert!(blocks
            .save(&dir.path().join("registry/blocks.yaml"))
            .is_err());
        assert!(!dir.path().join("registry").exists());
    }
}
