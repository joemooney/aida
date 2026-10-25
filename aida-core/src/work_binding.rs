//! Explicit WORK lifecycle bootstrap; ordinary backend reads never invent identity.
// trace:BUG-2017 | ai:codex
use crate::dispenser::{DispenserState, IdMode};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const BINDING_FILE: &str = "aida-work-node.toml";
pub const COUNTER_FILE: &str = "aida-work-dispenser.toml";

/// Additive representation: legacy NodeConfig keeps its original meanings.
/// No host, user, email, friendly name or path can enter this serialization.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkBinding {
    pub version: u32,
    pub node_id: String,
}
impl WorkBinding {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.version == 1, "unsupported WORK binding version");
        anyhow::ensure!(
            self.node_id.len() == 32 && self.node_id.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid opaque WORK node"
        );
        uuid::Uuid::parse_str(&self.node_id)?;
        Ok(())
    }
}

/// Bounded handle read: links, pipes, foreign/shared writers and malformed state refuse.
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    // Atomic replacement can unlink the inode between open and fstat. Reopen
    // that transient snapshot only; hard links and every unsafe type still
    // refuse. Each attempt retains O_NOFOLLOW/O_NONBLOCK and the byte bound.
    // trace:BUG-2017 | ai:codex
    for attempt in 0..8 {
        let mut f = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let m = f.metadata()?;
        if m.nlink() == 0 && attempt < 7 {
            std::thread::yield_now();
            continue;
        }
        anyhow::ensure!(
            m.is_file()
                && m.nlink() == 1
                && m.uid() == unsafe { libc::geteuid() }
                && m.mode() & 0o022 == 0
                && m.len() <= 1024 * 1024,
            "unsafe WORK allocation state"
        );
        let mut s = String::new();
        f.by_ref().take(1024 * 1024 + 1).read_to_string(&mut s)?;
        anyhow::ensure!(s.len() <= 1024 * 1024, "oversize WORK allocation state");
        return Ok(Some(toml::from_str(&s)?));
    }
    bail!("WORK allocation state changed repeatedly while opening")
}

pub fn load(root: &Path) -> Result<Option<WorkBinding>> {
    let binding: Option<WorkBinding> =
        read(&crate::allocation_authority::common_dir(root)?.join(BINDING_FILE))?;
    if let Some(b) = &binding {
        b.validate()?;
    }
    Ok(binding)
}

pub fn counter_path(root: &Path) -> Result<PathBuf> {
    Ok(crate::allocation_authority::common_dir(root)?.join(COUNTER_FILE))
}

/// Check the original provider path before following any leaf link. Opaque
/// bound state is read by bounded O_NOFOLLOW/O_NONBLOCK handle on every read.
pub fn bound_counter_state(path: &Path) -> Result<Option<DispenserState>> {
    let parent = path.parent().context("invalid counter path")?;
    if !crate::allocation_authority::in_git_clone(parent)? {
        return Ok(None);
    }
    if crate::allocation_authority::resolve(parent)?.authoritative {
        return Ok(None);
    }
    let Some(binding) = load(parent)? else {
        return Ok(None);
    };
    let original = parent
        .canonicalize()?
        .join(path.file_name().context("invalid counter file")?);
    anyhow::ensure!(
        original == counter_path(parent)?,
        "alternate WORK counter path refused"
    );
    let state: DispenserState =
        read(&original)?.context("bound WORK counter missing; explicit recovery required")?;
    anyhow::ensure!(
        state.mode
            == IdMode::Distributed {
                node_id: binding.node_id
            },
        "WORK counter binding mismatch"
    );
    Ok(Some(state))
}

/// Establish binding and persistent provider before init's first allocation.
/// Store writer precedes the common dispenser lock. Restart after publishing
/// the counter but before publishing the binding recovers that same UUID.
pub fn bootstrap(root: &Path) -> Result<Option<WorkBinding>> {
    if crate::allocation_authority::resolve(root)?.authoritative {
        return Ok(None);
    }
    crate::db::with_store_write_lock(root, || {
        let common = crate::allocation_authority::common_dir(root)?;
        let counter = common.join(COUNTER_FILE);
        crate::BlockRegistry::with_dispense_lock(&counter, || {
            if let Some(b) = load(root)? {
                let state: DispenserState =
                    read(&counter)?.context("WORK counter missing; explicit recovery required")?;
                anyhow::ensure!(
                    state.mode
                        == IdMode::Distributed {
                            node_id: b.node_id.clone()
                        },
                    "WORK counter binding mismatch"
                );
                return Ok(Some(b));
            }
            let mut state: DispenserState = match read(&counter)? {
                Some(state) => state,
                None => DispenserState::new(IdMode::Distributed {
                    node_id: uuid::Uuid::new_v4().simple().to_string(),
                }),
            };
            let node_id = match &state.mode {
                IdMode::Distributed { node_id } => node_id.clone(),
                _ => bail!("invalid WORK counter mode"),
            };
            let binding = WorkBinding {
                version: 1,
                node_id,
            };
            binding.validate()?;
            // Legacy counter state is read once under its own permanent lock.
            // Old native IDs/aliases/UUIDs are never rewritten by migration.
            let legacy = root.join(".aida/dispenser.toml");
            crate::BlockRegistry::with_dispense_lock(&legacy, || {
                // A legacy node without its provider may have lost consumed
                // counters. Require the complete original pair; only absence of
                // both records is cold bootstrap. Never infer a zero high-water.
                // trace:BUG-2017 | ai:codex
                let node = read::<crate::node::NodeConfig>(&root.join(".aida/node.toml"))?;
                let old = read::<DispenserState>(&legacy)?;
                match (&node, &old) {
                    (None, None) => {}
                    (Some(node), Some(old)) => {
                        crate::node::validate_node_id(&node.node_id).map_err(anyhow::Error::msg)?;
                        anyhow::ensure!(
                            old.mode == node.id_mode(),
                            "legacy node/provider binding mismatch"
                        );
                    }
                    _ => bail!("incomplete legacy node/provider pair; explicit recovery required"),
                }
                if let Some(old) = old {
                    for (prefix, n) in old.sequences {
                        let high = state.sequences.entry(prefix).or_default();
                        *high = (*high).max(n);
                    }
                }
                crate::write_atomic(&counter, toml::to_string_pretty(&state)?)?;
                crate::write_atomic(
                    &common.join(BINDING_FILE),
                    toml::to_string_pretty(&binding)?,
                )?;
                Ok(())
            })?;
            Ok(Some(binding))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispenser::Dispenser;
    #[test]
    fn bootstrap_restart_is_opaque_and_never_resets_consumed_sequences() {
        let d = tempfile::tempdir().unwrap();
        crate::git_ops::init(d.path()).unwrap();
        let first = bootstrap(d.path()).unwrap().unwrap();
        let serial = toml::to_string(&first).unwrap();
        assert_eq!(
            toml::from_str::<toml::Value>(&serial)
                .unwrap()
                .as_table()
                .unwrap()
                .len(),
            2
        );
        let handle = crate::allocation_authority::dispenser(d.path(), None).unwrap();
        assert_eq!(
            handle.next_id("BUG").unwrap(),
            format!("BUG-{}-001", first.node_id)
        );
        let second = bootstrap(d.path()).unwrap().unwrap();
        assert_eq!(second.node_id, first.node_id);
        assert_eq!(
            crate::allocation_authority::dispenser(d.path(), None)
                .unwrap()
                .next_id("BUG")
                .unwrap(),
            format!("BUG-{}-002", first.node_id)
        );
        assert!(!d.path().join("registry/nodes.toml").exists());
        assert!(!d.path().join(".git/aida-allocation.toml").exists());
    }
    #[test]
    fn legacy_high_water_survives_one_time_migration_and_retry() {
        let d = tempfile::tempdir().unwrap();
        crate::git_ops::init(d.path()).unwrap();
        std::fs::create_dir_all(d.path().join(".aida")).unwrap();
        std::fs::write(
            d.path().join(".aida/node.toml"),
            "node_id='7'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n",
        )
        .unwrap();
        let old = "[mode.Distributed]\nnode_id='7'\n[sequences]\nBUG=91\nFR=8\n";
        std::fs::write(d.path().join(".aida/dispenser.toml"), old).unwrap();
        let b = bootstrap(d.path()).unwrap().unwrap();
        assert_eq!(
            crate::allocation_authority::dispenser(d.path(), None)
                .unwrap()
                .next_id("BUG")
                .unwrap(),
            format!("BUG-{}-092", b.node_id)
        );
        bootstrap(d.path()).unwrap();
        assert_eq!(
            crate::allocation_authority::dispenser(d.path(), None)
                .unwrap()
                .next("BUG")
                .unwrap(),
            93
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join(".aida/dispenser.toml")).unwrap(),
            old
        );
    }
    #[test]
    fn legacy_bootstrap_refuses_invalid_or_incomplete_pairs_without_writes() {
        let valid = "node_id='7'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n";
        let provider = "[mode.Distributed]\nnode_id='7'\n[sequences]\nBUG=91\n";
        for (node, counter) in [
            (
                Some(valid),
                Some(provider.replace("node_id='7'", "node_id='8'")),
            ),
            (
                Some(valid),
                Some("mode='Centralized'\n[sequences]\nBUG=91\n".into()),
            ),
            (Some("node_id='7'\nhostname=''\n"), Some(provider.into())),
            (Some("invalid toml !"), Some(provider.into())),
            (Some(valid), None),
            (None, Some(provider.into())),
        ] {
            let d = tempfile::tempdir().unwrap();
            crate::git_ops::init(d.path()).unwrap();
            std::fs::create_dir(d.path().join(".aida")).unwrap();
            if let Some(n) = node {
                std::fs::write(d.path().join(".aida/node.toml"), n).unwrap();
            }
            if let Some(c) = counter {
                std::fs::write(d.path().join(".aida/dispenser.toml"), c).unwrap();
            }
            let files = [
                ".aida/node.toml",
                ".aida/dispenser.toml",
                "metadata.yaml",
                "oplog.yaml",
                ".git/HEAD",
            ];
            let before: Vec<_> = files
                .iter()
                .map(|p| std::fs::read(d.path().join(p)).ok())
                .collect();
            assert!(bootstrap(d.path()).is_err());
            let after: Vec<_> = files
                .iter()
                .map(|p| std::fs::read(d.path().join(p)).ok())
                .collect();
            assert_eq!(before, after);
            assert!(!d.path().join(".git").join(BINDING_FILE).exists());
            assert!(!counter_path(d.path()).unwrap().exists());
            assert!(!d.path().join("objects").exists());
        }
    }

    #[test]
    fn linked_worktrees_share_one_opaque_binding_and_concurrent_counter() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("main");
        std::fs::create_dir(&root).unwrap();
        crate::git_ops::init(&root).unwrap();
        let git = |args: &[&str]| {
            let out = crate::test_git::git_command()
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ]);
        let linked = d.path().join("linked");
        git(&[
            "worktree",
            "add",
            "-b",
            "aida-store",
            linked.to_str().unwrap(),
        ]);
        let handles: Vec<_> = (0..8)
            .map(|n| {
                let path = if n % 2 == 0 {
                    root.clone()
                } else {
                    linked.clone()
                };
                std::thread::spawn(move || {
                    let node = bootstrap(&path).unwrap().unwrap().node_id;
                    let dispenser = crate::allocation_authority::dispenser(&path, None).unwrap();
                    (
                        node,
                        (0..20)
                            .map(|_| dispenser.next("BUG").unwrap())
                            .collect::<Vec<_>>(),
                    )
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|(node, _)| node == &results[0].0));
        let mut numbers: Vec<_> = results.into_iter().flat_map(|(_, ns)| ns).collect();
        numbers.sort_unstable();
        assert_eq!(numbers, (1..=160).collect::<Vec<_>>());
        assert_eq!(
            crate::allocation_authority::dispenser(&linked, None)
                .unwrap()
                .next("BUG")
                .unwrap(),
            161
        );
    }

    #[test]
    fn home_promotion_preserves_native_alias_uuid_links_and_is_idempotent() {
        use crate::DatabaseBackend;
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        crate::git_ops::init(root).unwrap();
        crate::git_ops::configure_user(root, "Synthetic HOME", "home@example.invalid").unwrap();
        bootstrap(root).unwrap();
        let backend = crate::GitBackend::new(root)
            .unwrap()
            .with_auto_commit(false);
        let mut store = backend.load().unwrap();
        let mut req = crate::Requirement::new("work item".into(), "trace:BUG-2017".into());
        req.req_type = crate::RequirementType::Bug;
        let uuid = req.id;
        store.add_requirement_with_id(req, None, Some("BUG"));
        backend.save(&store).unwrap();
        let native = store.requirements[0].spec_id.clone().unwrap();
        let before = std::fs::read(root.join("metadata.yaml")).unwrap();
        assert!(crate::git_ops::merge_gate(root).is_err());
        assert_eq!(std::fs::read(root.join("metadata.yaml")).unwrap(), before);
        // Synthetic exact HOME; production enrollment is never invoked here.
        std::fs::write(
            root.join(".git/aida-allocation.toml"),
            "version=1\nauthoritative=true\n",
        )
        .unwrap();
        let pairs = crate::git_ops::merge_gate(root).unwrap();
        assert_eq!(pairs.len(), 1);
        let after = backend.load().unwrap();
        let old = after.get_requirement_by_spec_id(&native).unwrap();
        assert_eq!(old.id, uuid);
        assert_eq!(old.description, "trace:BUG-2017");
        assert_eq!(old.spec_id.as_deref(), Some(native.as_str()));
        assert_eq!(
            after.get_requirement_by_spec_id(&pairs[0].1).unwrap().id,
            uuid
        );
        assert!(crate::git_ops::merge_gate(root).unwrap().is_empty());
    }

    #[test]
    fn opaque_lowercase_ids_survive_full_save_and_keep_snapshot_cas() {
        use crate::DatabaseBackend;
        let d = tempfile::tempdir().unwrap();
        crate::git_ops::init(d.path()).unwrap();
        bootstrap(d.path()).unwrap();
        let backend = crate::GitBackend::new(d.path())
            .unwrap()
            .with_auto_commit(false);
        let mut store = backend.load().unwrap();
        store.add_requirement_with_id(
            crate::Requirement::new("first".into(), "".into()),
            None,
            Some("BUG"),
        );
        backend.save(&store).unwrap();
        let old = store.requirements[0].clone();
        let mut loaded = backend.load().unwrap();
        loaded.add_requirement_with_id(
            crate::Requirement::new("second".into(), "".into()),
            None,
            Some("BUG"),
        );
        backend.save(&loaded).unwrap();
        backend.save(&loaded).unwrap();
        let after = backend.load().unwrap();
        assert_eq!(after.requirements.len(), 2);
        assert_eq!(
            after
                .get_requirement_unambiguous(old.spec_id.as_deref().unwrap())
                .unwrap()
                .unwrap()
                .id,
            old.id
        );
        let mut stale = backend.load().unwrap();
        let mut changed = stale.clone();
        changed.requirements[0].title = "concurrent".into();
        backend.save(&changed).unwrap();
        stale.requirements[0].title = "stale edit".into();
        assert!(backend.save(&stale).is_err());
        assert_eq!(backend.load().unwrap().requirements[0].title, "concurrent");
    }

    #[test]
    fn redirected_and_hardlinked_bound_counters_refuse_without_reading_or_writing() {
        use std::os::unix::fs::symlink;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("clone");
        std::fs::create_dir(&root).unwrap();
        crate::git_ops::init(&root).unwrap();
        bootstrap(&root).unwrap();
        let path = counter_path(&root).unwrap();
        let original = std::fs::read(&path).unwrap();
        let outside = d.path().join("outside.toml");
        std::fs::write(&outside, &original).unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();
        let provider = crate::allocation_authority::dispenser(&root, None).unwrap();
        assert!(provider.next("BUG").is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), original);
        std::fs::remove_file(&path).unwrap();
        std::fs::hard_link(&outside, &path).unwrap();
        assert!(provider.next("BUG").is_err());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(std::fs::read(&outside).unwrap(), original);
    }

    #[test]
    fn bound_fifo_refusal_has_process_watchdog() {
        const CHILD_ROOT: &str = "AIDA_WORK_BINDING_FIFO_CHILD";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let provider = crate::allocation_authority::dispenser(Path::new(&root), None).unwrap();
            assert!(provider.next("BUG").is_err());
            return;
        }
        let d = tempfile::tempdir().unwrap();
        crate::git_ops::init(d.path()).unwrap();
        bootstrap(d.path()).unwrap();
        let path = counter_path(d.path()).unwrap();
        std::fs::remove_file(&path).unwrap();
        let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "work_binding::tests::bound_fifo_refusal_has_process_watchdog",
                "--nocapture",
            ])
            .env(CHILD_ROOT, d.path())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if std::time::Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("bound counter FIFO blocked instead of refusing");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn explicit_home_transition_keeps_work_binding_but_opens_home_provider() {
        let d = tempfile::tempdir().unwrap();
        crate::git_ops::init(d.path()).unwrap();
        let binding = bootstrap(d.path()).unwrap().unwrap();
        std::fs::write(
            d.path().join(".git/aida-allocation.toml"),
            "version=1\nauthoritative=true\n",
        )
        .unwrap();
        let provider = crate::dispenser::FileDispenser::open(
            d.path().join(".aida/dispenser.toml"),
            IdMode::Distributed {
                node_id: "1".into(),
            },
        )
        .unwrap();
        assert_eq!(provider.next("BUG").unwrap(), 1);
        assert_eq!(load(d.path()).unwrap().unwrap().node_id, binding.node_id);
        assert!(bootstrap(d.path()).unwrap().is_none());
    }

    #[test]
    fn lifecycle_load_and_rejected_global_identity_do_not_migrate_legacy_counters() {
        use crate::DatabaseBackend;
        let d = tempfile::tempdir().unwrap();
        crate::git_ops::init(d.path()).unwrap();
        std::fs::create_dir_all(d.path().join(".aida")).unwrap();
        let node = "node_id='7'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n";
        std::fs::write(d.path().join(".aida/node.toml"), node).unwrap();
        let old = "[mode.Distributed]\nnode_id='7'\n[sequences]\nBUG=41\n";
        std::fs::write(d.path().join(".aida/dispenser.toml"), old).unwrap();
        let backend = crate::GitBackend::new(d.path())
            .unwrap()
            .with_auto_commit(false)
            .with_dispenser(crate::allocation_authority::lifecycle_dispenser(d.path()));
        let mut store = backend.load().unwrap();
        let mut bad = crate::Requirement::new("global refusal".into(), "".into());
        bad.spec_id = Some("BUG-001".into());
        store.add_requirement_with_id(bad, None, Some("BUG"));
        assert!(backend.save(&store).is_err());
        assert!(load(d.path()).unwrap().is_none());
        assert!(!counter_path(d.path()).unwrap().exists());
        assert_eq!(
            std::fs::read_to_string(d.path().join(".aida/dispenser.toml")).unwrap(),
            old
        );
        let mut store = backend.load().unwrap();
        store.add_requirement_with_id(
            crate::Requirement::new("local admitted".into(), "".into()),
            None,
            Some("BUG"),
        );
        backend.save(&store).unwrap();
        let binding = load(d.path()).unwrap().unwrap();
        assert_eq!(
            store.requirements[0].spec_id.as_deref(),
            Some(format!("BUG-{}-042", binding.node_id).as_str())
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join(".aida/node.toml")).unwrap(),
            node
        );
    }

    #[test]
    fn partial_bootstrap_recovers_counter_uuid_and_missing_counter_refuses() {
        let d = tempfile::tempdir().unwrap();
        crate::git_ops::init(d.path()).unwrap();
        let b = bootstrap(d.path()).unwrap().unwrap();
        let common = crate::allocation_authority::common_dir(d.path()).unwrap();
        std::fs::remove_file(common.join(BINDING_FILE)).unwrap();
        assert_eq!(bootstrap(d.path()).unwrap().unwrap().node_id, b.node_id);
        std::fs::remove_file(common.join(COUNTER_FILE)).unwrap();
        let provider = crate::allocation_authority::dispenser(d.path(), None).unwrap();
        assert!(provider.next("BUG").is_err());
        assert!(crate::dispenser::FileDispenser::open(
            common.join(COUNTER_FILE),
            crate::dispenser::IdMode::Distributed {
                node_id: b.node_id.clone()
            }
        )
        .is_err());
        assert!(!common.join(COUNTER_FILE).exists());
        assert!(bootstrap(d.path()).is_err());
    }
}
