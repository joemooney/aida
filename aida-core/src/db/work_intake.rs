//! Synchronous native WORK preparation. Ordinary clones retain their existing
//! optimized write paths. Models here receive only the scoped detached handle.
// trace:STORY-1742 | ai:codex
// trace:BUG-2017 | ai:codex
use super::*;
use crate::transport_policy::{read_artifact, Admission, FileEffect, PreparedMutation};
use std::collections::{HashMap, HashSet};

/// Serialize metadata for a WORK write. Built-in reaction and type
/// definitions carry emoji and dotted prose that the closed policy refuses in
/// every fresh blob, so they are omitted when unchanged; loading an absent key
/// restores them.
// trace:STORY-1742 | ai:claude
fn work_metadata_bytes(metadata: &StoreMetadata) -> Result<Vec<u8>> {
    let mut value = serde_yaml::to_value(metadata)?;
    let map = value
        .as_mapping_mut()
        .context("WORK metadata is not a mapping")?;
    if metadata.reaction_definitions == crate::models::default_reaction_definitions() {
        map.remove("reaction_definitions");
    }
    if metadata.type_definitions == crate::models::default_type_definitions() {
        map.remove("type_definitions");
    }
    Ok(serde_yaml::to_string(&value)?.into_bytes())
}

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
                    after: Some(work_metadata_bytes(&metadata)?),
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

    /// Route an enabled-WORK clone to the prepared path. `None` keeps the
    /// ordinary writer unchanged. The admission is outer to every store lock.
    pub(super) fn work_admission(&self) -> Result<Option<Admission>> {
        let admission = Admission::enter(&self.root)?;
        Ok(admission.enabled().then_some(admission))
    }

    /// Batch replacement of existing rows, keyed by UUID. Creation is never
    /// implied: a row missing from the store refuses the whole batch.
    pub(super) fn work_bulk_update(
        &self,
        admission: &Admission,
        requirements: &[Requirement],
        commit_subject: &str,
    ) -> Result<usize> {
        let (_, summary) = self.work_update(admission, Some(commit_subject), |store| {
            for requirement in requirements {
                let row = store
                    .requirements
                    .iter_mut()
                    .find(|row| row.id == requirement.id)
                    .context("WORK bulk update target was not found; nothing was written")?;
                *row = requirement.clone();
            }
            Ok(())
        })?;
        Ok(summary.written.len())
    }

    /// The compare-and-swap sweep: every decision is taken on the snapshot
    /// loaded inside the store lock, then the whole batch is one prepared plan.
    pub(super) fn work_bulk_update_atomically<F>(
        &self,
        admission: &Admission,
        targets: &[Requirement],
        commit_subject: &str,
        mut keep: F,
    ) -> Result<BulkAtomicReport>
    where
        F: FnMut(&mut Requirement) -> bool,
    {
        let mut report = BulkAtomicReport::default();
        let mut accepted = Vec::new();
        let (stored, _) = self.work_update(admission, Some(commit_subject), |store| {
            for target in targets {
                let Some(spec_id) = target.spec_id.as_deref() else {
                    report.skipped_unreadable += 1;
                    continue;
                };
                let spec_id = object_store::canonical_spec_id(spec_id);
                let Some(row) = store.requirements.iter_mut().find(|row| {
                    row.spec_id
                        .as_deref()
                        .is_some_and(|id| object_store::canonical_spec_id(id) == spec_id)
                }) else {
                    report.skipped_missing += 1;
                    continue;
                };
                if row.id != target.id {
                    report.skipped_changed += 1;
                    continue;
                }
                let mut next = row.clone();
                if !keep(&mut next) {
                    report.skipped_changed += 1;
                    continue;
                }
                anyhow::ensure!(
                    next.id == row.id && next.spec_id == row.spec_id,
                    "update of {spec_id} tried to change its id or spec_id; nothing was written"
                );
                *row = next;
                accepted.push(row.id);
            }
            Ok(())
        })?;
        report.written = stored
            .requirements
            .into_iter()
            .filter(|row| accepted.contains(&row.id))
            .collect();
        Ok(report)
    }

    /// Per-spec compare-and-swap. A missing object is `None`, as ordinary.
    pub(super) fn work_update_spec<F>(
        &self,
        admission: &Admission,
        target: &Requirement,
        commit_subject: Option<&str>,
        update_fn: F,
    ) -> Result<Option<Requirement>>
    where
        F: FnOnce(&mut Requirement),
    {
        let spec_id = target
            .spec_id
            .as_deref()
            .map(object_store::canonical_spec_id);
        let spec_id =
            spec_id.context("Cannot update a requirement without a spec_id in the git store")?;
        let mut found = false;
        let (stored, _) = self.work_update(admission, commit_subject, |store| {
            let Some(row) = store.requirements.iter_mut().find(|row| {
                row.spec_id
                    .as_deref()
                    .is_some_and(|id| object_store::canonical_spec_id(id) == spec_id)
            }) else {
                return Ok(());
            };
            found = true;
            anyhow::ensure!(
                row.id == target.id,
                "{spec_id} now names a different requirement; nothing was written"
            );
            let mut next = row.clone();
            update_fn(&mut next);
            anyhow::ensure!(
                next.id == row.id && next.spec_id == row.spec_id,
                "update of {spec_id} tried to change its id or spec_id; nothing was written"
            );
            *row = next;
            Ok(())
        })?;
        Ok(found
            .then(|| {
                stored
                    .requirements
                    .into_iter()
                    .find(|row| row.id == target.id)
            })
            .flatten())
    }

    /// Whole-store save with the ordinary compare-and-swap rules, planned on
    /// the snapshot loaded inside the lock and admitted as one prepared plan.
    /// A conflict refuses before any effect, exactly like the ordinary save.
    pub(super) fn work_full_save(
        &self,
        admission: &Admission,
        incoming: &RequirementsStore,
    ) -> Result<SaveReport> {
        let snapshot = incoming.loaded_objects.as_ref();
        let mut report = SaveReport {
            kept_unloaded: Vec::new(),
            stale_untouched: Vec::new(),
        };
        let mine_meta = Self::extract_metadata(incoming);
        let mut deleted = Vec::new();
        self.work_update(admission, None, |store| {
            let fingerprint = |spec_id: &str| -> Result<Option<u64>> {
                Ok(object_store::read_object_text(&self.objects_root, spec_id)?
                    .as_deref()
                    .map(object_store::content_fingerprint))
            };
            let disk: Vec<Requirement> = std::mem::take(&mut store.requirements);
            let by_spec: HashMap<String, &Requirement> = disk
                .iter()
                .filter_map(|row| {
                    row.spec_id
                        .as_deref()
                        .map(|id| (object_store::canonical_spec_id(id), row))
                })
                .collect();
            let incoming_uuids: HashSet<_> = incoming.requirements.iter().map(|r| r.id).collect();
            let mut current = HashSet::new();
            let mut conflicts = Vec::new();
            let mut rows = Vec::new();
            for req in &incoming.requirements {
                // Identities are allocated only by the prepared add path.
                let spec_id =
                    req.spec_id.as_deref().filter(|id| !id.is_empty()).context(
                        "WORK full save cannot allocate identities; nothing was written",
                    )?;
                let canonical = object_store::canonical_spec_id(spec_id);
                current.insert(canonical.clone());
                let on_disk = by_spec.get(&canonical).copied();
                if let Some(snapshot) = snapshot {
                    let disk_fp = fingerprint(spec_id)?;
                    let unchanged = match (snapshot.disk(spec_id), disk_fp) {
                        (Some(loaded), Some(now)) => loaded == now,
                        (None, None) => true,
                        _ => false,
                    };
                    if !unchanged {
                        let mine = object_store::content_fingerprint(&serde_yaml::to_string(req)?);
                        if disk_fp == Some(mine) {
                            rows.extend(on_disk.cloned());
                        } else if snapshot.baseline(spec_id) == Some(mine) {
                            report.stale_untouched.push(spec_id.to_owned());
                            rows.extend(on_disk.cloned());
                        } else {
                            conflicts.push(spec_id.to_owned());
                        }
                        continue;
                    }
                }
                let prior = on_disk.or_else(|| disk.iter().find(|row| row.id == req.id));
                match prior {
                    Some(prior) if snapshot.is_none() && prior.modified_at > req.modified_at => {
                        conflicts.push(spec_id.to_owned());
                    }
                    Some(prior) => {
                        rows.push(Self::preserve_full_save_only_fields(req.clone(), prior))
                    }
                    None => rows.push(req.clone()),
                }
            }
            for row in &disk {
                let Some(spec_id) = row.spec_id.as_deref() else {
                    continue;
                };
                if current.contains(&object_store::canonical_spec_id(spec_id))
                    || incoming_uuids.contains(&row.id)
                {
                    continue;
                }
                match snapshot.and_then(|s| s.disk(spec_id)) {
                    None => {
                        report.kept_unloaded.push(spec_id.to_owned());
                        rows.push(row.clone());
                    }
                    Some(loaded) if fingerprint(spec_id)? != Some(loaded) => {
                        conflicts.push(spec_id.to_owned());
                    }
                    Some(_) => deleted.push(spec_id.to_owned()),
                }
            }
            let disk_meta = Self::extract_metadata(store);
            let meta_base = snapshot.and_then(|s| s.metadata_baseline());
            let plan = merge_metadata(
                meta_base.as_ref(),
                &mine_meta,
                Some(&disk_meta),
                incoming.id_counters_reset.is_set(),
            )?;
            if !conflicts.is_empty() || plan.is_err() {
                conflicts.sort();
                return Err(StoreConflictError {
                    specs: conflicts,
                    metadata_fields: plan.err().unwrap_or_default(),
                }
                .into());
            }
            if let Ok(MetadataMerge { write: Some(meta) }) = plan {
                let dispenser = store.dispenser.take();
                *store = self.assemble_store(meta, Vec::new());
                store.dispenser = dispenser;
            }
            store.requirements = rows;
            Ok(())
        })?;
        // Refresh the caller's load snapshot only after the admitted commit.
        if let Some(snapshot) = snapshot {
            snapshot.record_metadata(metadata_value(&mine_meta)?);
            for spec_id in &deleted {
                snapshot.remove(spec_id);
            }
            for req in &incoming.requirements {
                let Some(spec_id) = req.spec_id.as_deref() else {
                    continue;
                };
                match object_store::read_object_text(&self.objects_root, spec_id)? {
                    Some(text) => snapshot.record(
                        spec_id,
                        object_store::content_fingerprint(&text),
                        object_store::content_fingerprint(&serde_yaml::to_string(req)?),
                    ),
                    None => snapshot.remove(spec_id),
                }
            }
        }
        incoming.id_counters_reset.clear();
        report.kept_unloaded.sort();
        report.stale_untouched.sort();
        Ok(report)
    }

    /// Additive batch: identities are allocated only inside the prepared
    /// candidate scope at `finish`, never by a live dispenser during `add`.
    pub(super) fn work_bulk_finish(
        &self,
        admission: &Admission,
        staged: Vec<Requirement>,
        commit_subject: &str,
    ) -> Result<usize> {
        let count = staged.len();
        self.work_update(admission, Some(commit_subject), |store| {
            for row in staged {
                Self::work_add_to_store(store, row)?;
            }
            Ok(())
        })?;
        Ok(count)
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

    /// Run the calling test body in an exact-test child with an isolated HOME
    /// and no inherited Git routing: HOME and identity mutations must not
    /// race unlocked Git readers in the parent suite. Returns true in the
    /// child (run the body); false in the parent once the child passed.
    fn isolated(name: &str) -> bool {
        if std::env::var("AIDA_WORK_NATIVE_FIXTURE").ok().as_deref() == Some(name) {
            return true;
        }
        let home = tempfile::tempdir().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", name, "--nocapture", "--test-threads=1"]);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        command
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("AIDA_WORK_NATIVE_FIXTURE", name)
            .env("AIDA_AUTHOR", "Fixture")
            .env_remove("AIDA_SESSION_GRANT")
            .process_group(0);
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "isolated WORK fixture failed");
                return false;
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

    #[test]
    #[cfg(unix)]
    fn native_work_commits_admitted_bytes_and_never_spends_on_refusal() {
        if !isolated("db::git_backend::work_intake::tests::native_work_commits_admitted_bytes_and_never_spends_on_refusal") {
            return;
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

    fn row(title: &str) -> Requirement {
        Requirement::new(title.into(), "Synthetic reproduction".into())
    }

    #[test]
    #[cfg(unix)]
    fn story_1742_native_bulk_paths_admit_or_refuse_whole_batches() {
        // trace:STORY-1742 | ai:claude
        if !isolated("db::git_backend::work_intake::tests::story_1742_native_bulk_paths_admit_or_refuse_whole_batches") {
            return;
        }
        let (_temp, root, backend) = fixture();
        let a = backend.add_requirement(row("Generic failure")).unwrap();
        let b = backend
            .add_requirement(row("Second generic failure"))
            .unwrap();
        let node = crate::work_binding::load(&root).unwrap().unwrap().node_id;
        assert!(b.spec_id.as_deref().unwrap().contains(&node));

        // Bulk replacement: one unsafe row refuses the whole batch.
        let accepted = snapshot(&root);
        let (mut a2, mut b2) = (a.clone(), b.clone());
        a2.title = "Generic failure, clarified".into();
        b2.description = "private-marker".into();
        assert!(backend
            .bulk_update(&[a2.clone(), b2.clone()], "chore")
            .is_err());
        assert_eq!(snapshot(&root), accepted);
        b2.description = "Generic clarified reproduction".into();
        assert_eq!(
            backend
                .bulk_update(&[a2.clone(), b2.clone()], "chore")
                .unwrap(),
            2
        );
        let mut missing = row("Never filed");
        missing.spec_id = Some(format!("BUG-{node}-900"));
        let accepted = snapshot(&root);
        assert!(backend.bulk_update(&[missing], "chore").is_err());
        assert_eq!(snapshot(&root), accepted);

        // Compare-and-swap sweep decides on the locked snapshot.
        let report = backend
            .bulk_update_atomically(&[a.clone(), b.clone()], "chore", |r| {
                if r.id == a.id {
                    r.description = "Generic swept reproduction".into();
                    true
                } else {
                    false
                }
            })
            .unwrap();
        assert_eq!(report.written.len(), 1);
        assert_eq!(report.skipped_changed, 1);
        let accepted = snapshot(&root);
        assert!(backend
            .bulk_update_atomically(&[a.clone()], "chore", |r| {
                r.description = "private-marker".into();
                true
            })
            .is_err());
        assert_eq!(snapshot(&root), accepted);

        // Per-spec compare-and-swap.
        assert!(backend
            .update_spec_atomically(&a, |r| r.description = "private-marker".into())
            .is_err());
        assert_eq!(snapshot(&root), accepted);
        let updated = backend
            .update_spec_atomically(&a, |r| r.description = "Generic final reproduction".into())
            .unwrap()
            .unwrap();
        assert_eq!(updated.description, "Generic final reproduction");
        assert!(updated.filed_at.is_none());

        // Additive batch: identities only inside the prepared scope.
        let accepted = snapshot(&root);
        let mut writer = backend.bulk_writer().unwrap();
        writer.add(row("Batch generic failure")).unwrap();
        writer.add(row("private-marker")).unwrap();
        assert!(writer.finish("chore").is_err());
        assert_eq!(snapshot(&root), accepted);
        let mut writer = backend.bulk_writer().unwrap();
        assert!(writer
            .add(row("Batch generic failure"))
            .unwrap()
            .spec_id
            .is_none());
        writer.add(row("Another batch failure")).unwrap();
        assert_eq!(writer.finish("chore").unwrap(), 2);
        let store = backend.load().unwrap();
        assert_eq!(store.requirements.len(), 4);
        assert!(store
            .requirements
            .iter()
            .all(|r| r.spec_id.as_deref().unwrap().contains(&node)));

        // Writers without a prepared plan refuse before any effect.
        let accepted = snapshot(&root);
        assert!(backend.save_imported(&store).is_err());
        let entry = QueueEntry {
            user_id: "generic".into(),
            requirement_id: a.id,
            position: 0,
            added_by: "generic".into(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: None,
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        };
        assert!(backend.queue_add(entry).is_err());
        assert_eq!(snapshot(&root), accepted);
    }

    #[test]
    #[cfg(unix)]
    fn story_1742_native_full_save_keeps_conflict_rules_and_policy() {
        // trace:STORY-1742 | ai:claude
        if !isolated("db::git_backend::work_intake::tests::story_1742_native_full_save_keeps_conflict_rules_and_policy") {
            return;
        }
        let (_temp, root, backend) = fixture();
        let a = backend.add_requirement(row("Generic failure")).unwrap();
        let b = backend
            .add_requirement(row("Second generic failure"))
            .unwrap();

        let mut store = backend.load().unwrap();
        store.requirements[0].title = "Generic failure, clarified".into();
        backend.save(&store).unwrap();
        // The refreshed snapshot lets the same store save again.
        store.requirements[0].description = "Generic clarified reproduction".into();
        backend.save(&store).unwrap();
        let persisted = backend
            .get_requirement(&store.requirements[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.description, "Generic clarified reproduction");

        let accepted = snapshot(&root);
        let mut unsafe_store = backend.load().unwrap();
        unsafe_store.requirements[1].description = "private-marker".into();
        assert!(backend.save(&unsafe_store).is_err());
        assert_eq!(snapshot(&root), accepted);

        // A concurrent edit wins; the stale editor gets a conflict.
        let mut first = backend.load().unwrap();
        let mut second = backend.load().unwrap();
        first.requirements[1].title = "First editor title".into();
        backend.save(&first).unwrap();
        let accepted = snapshot(&root);
        second.requirements[1].title = "Second editor title".into();
        let error = backend.save(&second).unwrap_err();
        assert!(error.downcast_ref::<crate::StoreConflictError>().is_some());
        assert_eq!(snapshot(&root), accepted);

        // Identities and global counters are never minted by a full save.
        let mut fresh = backend.load().unwrap();
        fresh.requirements.push(row("Unallocated generic failure"));
        assert!(backend.save(&fresh).is_err());
        let mut fresh = backend.load().unwrap();
        fresh.next_spec_number += 1;
        assert!(backend.save(&fresh).is_err());
        assert_eq!(snapshot(&root), accepted);

        // Store-level metadata and deletions are admitted as one plan.
        let mut fresh = backend.load().unwrap();
        fresh.title = "Generic project".into();
        fresh.requirements.retain(|r| r.id != a.id);
        backend.save(&fresh).unwrap();
        let stored = backend.load().unwrap();
        assert_eq!(stored.title, "Generic project");
        // Built-in reaction and type definitions survive the metadata rewrite.
        assert_eq!(
            stored.reaction_definitions,
            crate::models::default_reaction_definitions()
        );
        assert_eq!(
            stored.type_definitions,
            crate::models::default_type_definitions()
        );
        assert_eq!(stored.requirements.len(), 1);
        assert_eq!(stored.requirements[0].id, b.id);
        let mut fresh = backend.load().unwrap();
        fresh.title = "private-marker".into();
        let accepted = snapshot(&root);
        assert!(backend.save(&fresh).is_err());
        assert_eq!(snapshot(&root), accepted);
    }
}
