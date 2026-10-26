//! Synchronous native WORK preparation. Ordinary clones retain their existing
//! optimized write paths. Models here receive only the scoped detached handle.
// trace:STORY-1742 | ai:codex
// trace:BUG-2017 | ai:codex
use super::*;
use crate::transport_policy::{read_artifact, Admission, FileEffect, PreparedMutation};
use std::collections::{HashMap, HashSet};

impl GitBackend {
    pub(super) fn work_update<F>(
        &self,
        admission: &Admission,
        subject: Option<&str>,
        update: F,
    ) -> Result<(RequirementsStore, AtomicWriteSummary)>
    where
        F: FnOnce(&mut RequirementsStore) -> Result<()>,
    {
        anyhow::ensure!(
            self.auto_commit,
            "WORK native mutation requires its prepared commit boundary"
        );
        crate::work_binding::candidate::with_unspent(&self.root, |candidate| {
            // Bound and reject alias/recursion encodings before legacy model
            // readers parse an existing snapshot. This is not trust promotion.
            let inventory = object_store::list_objects_strict(&self.objects_root)?;
            anyhow::ensure!(
                inventory.len() <= 10_000,
                "WORK source inventory limit exceeded"
            );
            let mut total = 0usize;
            for path in std::iter::once("metadata.yaml".to_string()).chain(
                inventory
                    .iter()
                    .map(|(id, _)| object_store::relative_object_path(id))
                    .collect::<Result<Vec<_>>>()?,
            ) {
                let bytes =
                    read_artifact(&self.root, &path)?.context("WORK source snapshot changed")?;
                total += bytes.len();
                anyhow::ensure!(
                    total <= 64 * 1_048_576
                        && !bytes.iter().any(|b| matches!(b, b'&' | b'*' | b'!')),
                    "Unsupported WORK source snapshot grammar or size"
                );
            }
            let mut store = self.load()?;
            let original = store.requirements.clone();
            let by_uuid: HashMap<_, _> = original.iter().map(|row| (row.id, row)).collect();
            let baseline = original
                .iter()
                .map(|row| Ok((row.id, serde_yaml::to_string(row)?)))
                .collect::<Result<HashMap<_, _>>>()?;
            let old_meta = Self::extract_metadata(&store);
            store.dispenser = Some(candidate.dispenser());
            update(&mut store)?;
            let mut uuids = HashSet::new();
            anyhow::ensure!(
                store.requirements.iter().all(|row| uuids.insert(row.id)),
                "WORK mutation left duplicate requirement UUIDs"
            );
            candidate.admit(&store.requirements, &original)?;
            crate::hypothesis::enforce(&original, &mut store.requirements)?;

            let context = crate::provenance::WriteContext::capture();
            let mut effects = Vec::new();
            let mut summary = AtomicWriteSummary::default();
            let mut operations = Vec::new();
            let mut retained = HashSet::new();
            let mut record_authority = false;
            for row in &mut store.requirements {
                let prior = by_uuid.get(&row.id).copied();
                if prior.is_none() {
                    // Optional filing context can identify the work machine.
                    // Required actor/history/comment context is preserved and
                    // must pass the same complete-artifact scanner.
                    row.filed_at = None;
                    const KEY: &str = "filing_provenance_omission_reason";
                    const REASON: &str =
                        "WORK sanitized creation omits optional machine and session filing context";
                    if let Some(value) = row.custom_fields.get(KEY) {
                        anyhow::ensure!(
                            value == REASON,
                            "Reserved WORK omission reason conflicts with supplied data"
                        );
                    }
                    row.custom_fields.insert(KEY.into(), REASON.into());
                }
                let (stamped, ops, authority) =
                    Self::prepare_requirement_update(row, prior, &context)?;
                *row = stamped;
                let id = row
                    .spec_id
                    .as_deref()
                    .context("WORK allocation did not produce a display identity")?;
                let path = object_store::relative_object_path(id)?;
                retained.insert(path.clone());
                // These bytes become the sole canonical write payload.
                let bytes = serde_yaml::to_string(row)?.into_bytes();
                if baseline
                    .get(&row.id)
                    .is_some_and(|old| old.as_bytes() == bytes)
                {
                    continue;
                }
                let before = read_artifact(&self.root, &path)?;
                effects.push(FileEffect {
                    path,
                    before,
                    after: Some(bytes),
                });
                record_authority |= authority;
                for kind in ops {
                    operations.push((row.id, kind));
                }
                if prior.is_none() {
                    summary.created.push(id.to_owned());
                    operations.push((
                        row.id,
                        crate::oplog::OpKind::Create {
                            title: row.title.clone(),
                            description: row.description.clone(),
                            req_type: format!("{:?}", row.req_type),
                            status: row.effective_status(),
                            priority: row.effective_priority(),
                        },
                    ));
                }
                summary.written.push(row.clone());
            }
            for row in &original {
                let id = row
                    .spec_id
                    .as_deref()
                    .context("WORK source has no display identity")?;
                let path = object_store::relative_object_path(id)?;
                if !retained.contains(&path) {
                    effects.push(FileEffect {
                        before: read_artifact(&self.root, &path)?,
                        path,
                        after: None,
                    });
                    if !store.requirements.iter().any(|next| next.id == row.id) {
                        summary.deleted.push(row.id);
                        operations.push((row.id, crate::oplog::OpKind::Archive));
                    }
                }
            }
            let metadata = Self::extract_metadata(&store);
            if metadata_value(&metadata)? != metadata_value(&old_meta)? {
                effects.push(FileEffect {
                    path: "metadata.yaml".into(),
                    before: read_artifact(&self.root, "metadata.yaml")?,
                    after: Some(serde_yaml::to_string(&metadata)?.into_bytes()),
                });
                summary.metadata_changed = true;
            }
            if self.oplog_enabled && !operations.is_empty() {
                let before = read_artifact(&self.root, "oplog.yaml")?;
                let mut log: crate::oplog::OpLog = match &before {
                    Some(bytes) => serde_yaml::from_slice(bytes)?,
                    None => crate::oplog::OpLog::new(candidate.node().to_owned()),
                };
                // Existing operations retain their original node evidence.
                // Only the current append context follows this clone binding.
                log.node_id = candidate.node().to_owned();
                for (id, kind) in operations {
                    log.append(id, "aida".into(), kind);
                }
                effects.push(FileEffect {
                    path: "oplog.yaml".into(),
                    before,
                    after: Some(serde_yaml::to_string(&log)?.into_bytes()),
                });
            }
            if effects.is_empty() {
                return Ok((self.load()?, summary));
            }
            let message = if summary.created.len() == 1
                && summary.written.len() == 1
                && summary.deleted.is_empty()
            {
                format!("add {} - {}", summary.created[0], summary.written[0].title)
            } else {
                format!(
                    "chore: update {} requirements, delete {}",
                    summary.written.len(),
                    summary.deleted.len()
                )
            };
            let message = match subject {
                Some(subject) => format!(
                    "{subject}: update {} requirements, delete {}",
                    summary.written.len(),
                    summary.deleted.len()
                ),
                None => message,
            };
            let message = self.with_authority_trailers(&message, record_authority);
            PreparedMutation::prepare(&self.root, effects, candidate.seal()?, &message, admission)?
                .apply()?;
            // Authoritative caller/server state is returned only after the
            // admitted mutation and commit succeed. Failed attempts return none.
            Ok((self.load()?, summary))
        })
    }

    pub(super) fn work_add_to_store(
        store: &mut RequirementsStore,
        mut row: Requirement,
    ) -> Result<()> {
        if row.spec_id.is_some() {
            store.requirements.push(row);
            return Ok(());
        }
        let prefix = store.get_type_prefix(&row.req_type);
        for _ in 0..10_000 {
            store.add_requirement_with_id(row, None, prefix.as_deref());
            row = store
                .requirements
                .pop()
                .context("WORK allocation did not return a candidate")?;
            let id = row
                .spec_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .context("WORK candidate allocation failed")?;
            if !store.requirements.iter().any(|old| {
                old.spec_id.as_deref().is_some_and(|old| {
                    object_store::canonical_spec_id(old) == object_store::canonical_spec_id(id)
                })
            }) {
                store.requirements.push(row);
                return Ok(());
            }
            row.spec_id = None;
        }
        anyhow::bail!("WORK candidate collision limit exceeded")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport_policy::{Private, Profile, POLICY_FILE, PROFILE_FILE};
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn git(root: &Path, args: &[&str]) -> Vec<u8> {
        let result = crate::git_ops::host_git_command(root)
            .args(args)
            .output()
            .unwrap();
        assert!(result.status.success(), "fixture Git command failed");
        result.stdout
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, GitBackend) {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        crate::git_ops::init(&project).unwrap();
        crate::git_ops::configure_user(&project, "AIDA Generic", "aida").unwrap();
        std::fs::write(project.join("README"), "Generic fixture\n").unwrap();
        crate::git_ops::add(&project, &["README"]).unwrap();
        crate::git_ops::commit(&project, "seed fixture").unwrap();
        let root =
            crate::git_ops::create_store_worktree(&project, ".aida-store", "aida-store").unwrap();
        let backend = GitBackend::new(&root).unwrap();
        let metadata = GitBackend::extract_metadata(&RequirementsStore::new());
        std::fs::write(
            root.join("metadata.yaml"),
            serde_yaml::to_string(&metadata).unwrap(),
        )
        .unwrap();
        crate::git_ops::ensure_aida_runtime_excluded(&root).unwrap();
        crate::git_ops::ensure_store_lock_excluded(&root).unwrap();
        crate::git_ops::add(&root, &["metadata.yaml"]).unwrap();
        crate::git_ops::commit(&root, "seed store").unwrap();
        let head = String::from_utf8(git(&root, &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let common = crate::allocation_authority::common_dir(&root).unwrap();
        let private = Private::open(&common).unwrap();
        private
            .write(
                PROFILE_FILE,
                &serde_json::to_vec(&Profile {
                    version: 1,
                    name: "work".into(),
                    origin_sha256: "a".repeat(64),
                    baseline: Some(head),
                })
                .unwrap(),
            )
            .unwrap();
        private
            .write(
                POLICY_FILE,
                &serde_json::to_vec(&crate::transport_sanitization::Policy {
                    version: 1,
                    reviewed: true,
                    deny_tokens: vec!["private-marker".into()],
                    git_name: "AIDA Generic".into(),
                    git_identity: "aida".into(),
                })
                .unwrap(),
            )
            .unwrap();
        (temp, root, backend)
    }

    fn snapshot(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        let common = crate::allocation_authority::common_dir(root).unwrap();
        let mut result = vec![("HEAD".into(), Some(git(root, &["rev-parse", "HEAD"])))];
        for name in [
            crate::work_binding::COUNTER_FILE,
            crate::work_binding::BINDING_FILE,
        ] {
            result.push((name.into(), std::fs::read(common.join(name)).ok()));
        }
        for name in ["metadata.yaml", "oplog.yaml"] {
            result.push((name.into(), std::fs::read(root.join(name)).ok()));
        }
        fn walk(root: &Path, path: &Path, rows: &mut Vec<(String, Option<Vec<u8>>)>) {
            for entry in std::fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                if entry.file_type().unwrap().is_dir() {
                    rows.push((name, None));
                    walk(root, &path, rows);
                } else {
                    rows.push((name, Some(std::fs::read(path).unwrap())));
                }
            }
        }
        walk(root, &root.join("objects"), &mut result);
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }

    #[test]
    #[cfg(unix)]
    fn native_work_commits_admitted_bytes_and_never_spends_on_refusal() {
        const NAME: &str = "db::git_backend::work_intake::tests::native_work_commits_admitted_bytes_and_never_spends_on_refusal";
        if std::env::var("AIDA_WORK_NATIVE_FIXTURE").ok().as_deref() != Some(NAME) {
            // HOME and identity mutations must not race unlocked Git readers
            // in the parent suite. Exact-test child plus watchdog, no PATH swap.
            let home = tempfile::tempdir().unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", NAME, "--nocapture", "--test-threads=1"]);
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("GIT_") {
                    command.env_remove(key);
                }
            }
            command
                .env("HOME", home.path())
                .env("XDG_CONFIG_HOME", home.path())
                .env("AIDA_WORK_NATIVE_FIXTURE", NAME)
                .env("AIDA_AUTHOR", "Fixture")
                .env_remove("AIDA_SESSION_GRANT")
                .process_group(0);
            let mut child = command.spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(45);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    return;
                }
                if Instant::now() >= deadline {
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let _ = child.wait();
                    panic!("native WORK boundary fixture exceeded watchdog");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let (_temp, root, backend) = fixture();
        let baseline = snapshot(&root);
        assert!(backend
            .add_requirement(Requirement::new(
                "private-marker".into(),
                "Synthetic reproduction".into()
            ))
            .is_err());
        assert_eq!(snapshot(&root), baseline);
        assert!(crate::work_binding::load(&root).unwrap().is_none());
        let row = backend
            .add_requirement(Requirement::new(
                "Generic failure".into(),
                "Synthetic reproduction".into(),
            ))
            .unwrap();
        assert!(row.spec_id.as_deref().unwrap().ends_with("-001"));
        assert!(row.filed_at.is_none());
        assert!(row
            .custom_fields
            .contains_key("filing_provenance_omission_reason"));
        let accepted = snapshot(&root);
        let mut env = crate::test_env::EnvVarsGuard::apply(&[(
            "GIT_AUTHOR_EMAIL",
            Some("private@node.invalid"),
        )]);
        assert!(backend
            .add_requirement(Requirement::new(
                "Another generic failure".into(),
                "Synthetic reproduction".into()
            ))
            .is_err());
        assert_eq!(snapshot(&root), accepted);
        env.unset_key("GIT_AUTHOR_EMAIL");
        let cached =
            crate::CachedGitBackend::open(&root, &root.parent().unwrap().join("cache.db")).unwrap();
        let mut changed = row.clone();
        changed.description = "private-marker".into();
        assert!(cached.update_requirement(&changed).is_err());
        assert_eq!(snapshot(&root), accepted);
        changed.description = "Generic clarified reproduction".into();
        cached.update_requirement(&changed).unwrap();
        let persisted = backend.get_requirement(&row.id).unwrap().unwrap();
        assert_eq!(persisted.description, changed.description);
        assert!(persisted.filed_at.is_none());
        let common = crate::allocation_authority::common_dir(&root).unwrap();
        assert!(!common
            .join(crate::allocation_authority::CONFIG_FILE)
            .exists());
        assert!(!root.join("registry/nodes.toml").exists());
    }
}
