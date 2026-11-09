//! Native-seat publication declarations. The host owns the launch binding;
//! the seat writes only its requested subject and optional body, never Git metadata.
// trace:TASK-2902 | ai:codex
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    version: u8,
    lease: String,
    spec: String,
    token: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    binding: Binding,
    subject: String,
    // trace:BUG-2493 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}

// Never let a seat-written FIFO, link or oversized file stall the host.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    anyhow::ensure!(
        meta.is_file() && meta.nlink() == 1 && meta.len() <= 16384,
        "invalid native publication file {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.by_ref().take(16385).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 16384, "native publication file too large");
    Ok(serde_json::from_slice(&bytes)?)
}

fn expected_path(main: &Path, lease: &str) -> PathBuf {
    // Lease IDs are opaque, but never allow them to supply path components.
    let safe: String = lease
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    main.join(".aida/native-publications")
        .join(format!("{safe}.json"))
}
fn declaration_path(tree: &Path) -> PathBuf {
    tree.join(".aida/native-publication.json")
}

/// Resolve the entire scope, not an ID extracted from its prefix.
pub(crate) fn resolve_scope(main: &Path, scope: &str) -> Result<String> {
    let store = crate::load_store_for_lookup(main).with_context(|| {
        format!("cannot resolve publication scope `{scope}`: store unavailable")
    })?;
    resolve_scope_in(&store, scope)
}

fn resolve_scope_in(store: &aida_core::RequirementsStore, scope: &str) -> Result<String> {
    let expanded = aida_core::node_alias::expand_or_keep(scope);
    let mut matches = store.requirements.iter().filter(|r| {
        [r.spec_id.as_deref(), r.agreed_id.as_deref()]
            .into_iter()
            .flatten()
            .any(|id| aida_core::id_identity::same_id(id, &expanded))
    });
    let req = matches.next().with_context(|| format!(
        "publication scope `{scope}` must name exactly one resolvable non-epic spec; multi-spec scopes are unsupported"))?;
    anyhow::ensure!(
        matches.next().is_none(),
        "publication scope `{scope}` is ambiguous"
    );
    anyhow::ensure!(
        req.req_type != aida_core::RequirementType::Epic,
        "publication scope `{scope}` names an epic; name one non-epic spec"
    );
    anyhow::ensure!(
        !req.archived && req.status != aida_core::RequirementStatus::Rejected,
        "publication scope `{scope}` is not live"
    );
    Ok(req.display_id()) // canonical-id: persisted publication scope and commit trailer
}

fn lease_for_tree(main: &Path, tree: &Path) -> Result<Option<crate::SessionLease>> {
    let tree = tree.canonicalize()?;
    let mut leases = crate::list_leases(main).into_iter().filter(|l| {
        l.tree_kind == crate::seat_clone::TreeKind::Clone
            && l.worktree_path.canonicalize().is_ok_and(|p| p == tree)
    });
    let lease = leases.next();
    anyhow::ensure!(
        leases.next().is_none(),
        "multiple leases name publication seat {}",
        tree.display()
    );
    Ok(lease)
}

/// A rotation continues the same publication handshake. Only a new launch
/// (non-rotating or generation one) may replace its binding and intent.
// trace:TASK-2902 | ai:codex
pub(crate) fn prepare_generation(main: &Path, tree: &Path, generation: Option<u64>) -> Result<()> {
    if generation.is_none_or(|generation| generation == 1) {
        prepare(main, tree)?;
    }
    Ok(())
}

/// Release host-owned publication state with its lease. The seat's declaration
/// can remain as stale evidence; a fresh launch always clears it.
// trace:TASK-2902 | ai:codex
pub(crate) fn release(main: &Path, lease: &str) {
    let path = expected_path(main, lease);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!(
            "Warning: cannot remove native publication binding {}: {e}",
            path.display()
        ),
    }
}

/// Clear the previous declaration and issue a fresh binding for each launch,
/// including reuse of the same lease, but not rotation generations.
/// No native write grants are added.
pub(crate) fn prepare(main: &Path, tree: &Path) -> Result<()> {
    let main = crate::main_worktree_root_from(main);
    let Some(lease) = lease_for_tree(&main, tree)? else {
        return Ok(());
    };
    let path = declaration_path(tree);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("cannot clear native publication declaration"),
    }
    let binding = Binding {
        version: 1,
        lease: lease.id.clone(),
        spec: lease.scope,
        token: uuid::Uuid::new_v4().to_string(),
    };
    let expected = expected_path(&main, &lease.id);
    std::fs::create_dir_all(expected.parent().unwrap())?;
    std::fs::write(expected, serde_json::to_vec(&binding)?)?;
    Ok(())
}

fn binding(main: &Path, lease: &crate::SessionLease) -> Result<Binding> {
    let b: Binding = read_json(&expected_path(main, &lease.id))?;
    anyhow::ensure!(
        b.version == 1 && b.lease == lease.id && b.spec == lease.scope,
        "stale or mismatched native publication binding for lease {}",
        lease.id
    );
    Ok(b)
}

fn validate_subject(main: &Path, spec: &str, subject: &str) -> Result<()> {
    anyhow::ensure!(
        !subject.is_empty() && !subject.contains(['\n', '\r', '\0']),
        "publication declaration must contain one nonempty subject line"
    );
    let footer = subject
        .trim_end()
        .rfind('(')
        .map(|at| &subject.trim_end()[at..])
        .unwrap_or("");
    anyhow::ensure!(
        crate::pr_ship::extract_authorship_spec_ids_from_subject(footer).len() == 1,
        "publication declaration must end in one `(ID)` or `(Refs ID)` group"
    );
    crate::ship_from_seat::pending_message_check(main, subject, None)?;
    let ids = crate::pr_ship::extract_authorship_spec_ids_from_subject(subject);
    anyhow::ensure!(
        ids.len() == 1,
        "publication declaration must name exactly one lease spec"
    );
    anyhow::ensure!(
        resolve_scope(main, &ids[0])? == spec,
        "publication declaration names `{}`, outside lease spec `{spec}`",
        ids[0]
    );
    Ok(())
}

// trace:BUG-2493 | ai:codex
pub(crate) fn split_message(message: &str) -> (&str, Option<&str>) {
    match message.split_once("\n\n") {
        Some((subject, body)) => (subject, Some(body)),
        None => (message, None),
    }
}

// trace:BUG-2493 | ai:codex
pub(crate) fn join_message(subject: &str, body: Option<&str>) -> String {
    match body {
        Some(body) => format!("{subject}\n\n{body}"),
        None => subject.to_string(),
    }
}

/// Authoring surface for native `aida ship -m`: declaration only.
pub(crate) fn declare(
    tree: &Path,
    explicit: Option<&str>,
    subject: &str,
    dry_run: bool,
) -> Result<()> {
    let main = crate::main_worktree_root_from(tree);
    let lease =
        lease_for_tree(&main, tree)?.context("no native publication lease for this seat")?;
    let spec = resolve_scope(&main, &lease.scope)?;
    if let Some(scope) = explicit {
        anyhow::ensure!(
            resolve_scope(&main, scope)? == spec,
            "publication scope `{scope}` does not belong to lease `{}`",
            lease.scope
        );
    }
    // trace:BUG-2493 | ai:codex
    let (subject, body) = split_message(subject);
    validate_subject(&main, &spec, subject)?;
    anyhow::ensure!(
        !body.unwrap_or("").contains('\0'),
        "commit body contains NUL"
    );
    let declaration = Declaration {
        binding: binding(&main, &lease)?,
        subject: subject.to_string(),
        body: body.map(str::to_string),
    };
    let bytes = serde_json::to_vec(&declaration)?;
    anyhow::ensure!(bytes.len() <= 16384, "native publication file too large");
    if !dry_run {
        let path = declaration_path(tree);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        use std::io::Write;
        file.write_all(&bytes)?;
        file.persist(&path).map_err(|e| e.error)?;
    }
    println!(
        "{}native publication declaration: {subject}",
        if dry_run {
            "Would record "
        } else {
            "Recorded "
        }
    );
    Ok(())
}

/// Host validation happens before forge probes, commits or pushes. Absence
/// preserves ordinary completion; malformed or stale declarations refuse.
pub(crate) fn read(main: &Path, tree: &Path, spec: &str) -> Result<Option<String>> {
    let Some(lease) = lease_for_tree(main, tree)? else {
        return Ok(None);
    };
    anyhow::ensure!(
        resolve_scope(main, &lease.scope)? == spec,
        "publication scope `{spec}` does not belong to lease `{}`",
        lease.scope
    );
    let d: Declaration = match read_json(&declaration_path(tree)) {
        Ok(d) => d,
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None)
        }
        Err(e) => return Err(e).context("invalid native publication declaration"),
    };
    anyhow::ensure!(
        d.binding == binding(main, &lease)?,
        "stale or mismatched native publication declaration"
    );
    validate_subject(main, spec, &d.subject)?;
    // Persist canonical display IDs even when the author used an origin ID.
    let completing = !crate::pr_ship::extract_trailing_spec_ids_from_subject(&d.subject).is_empty();
    let prefix = d
        .subject
        .rsplit_once('(')
        .context("missing publication trailer")?
        .0;
    anyhow::ensure!(
        !d.body.as_deref().unwrap_or("").contains('\0'),
        "commit body contains NUL"
    );
    Ok(Some(join_message(
        &format!("{prefix}({}{spec})", if completing { "" } else { "Refs " }),
        d.body.as_deref(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_2902_scope_is_exact_live_non_epic_and_canonical() {
        let mut store = aida_core::RequirementsStore::new();
        let mut req = aida_core::Requirement::new("single task".into(), String::new());
        req.spec_id = Some("TASK-b6b1278890774ef993d2ebc327862470-021".into());
        req.agreed_id = Some("TASK-2902".into());
        req.req_type = aida_core::RequirementType::Task;
        store.requirements.push(req.clone());
        for id in [req.spec_id.as_deref().unwrap(), "TASK-2902", "task-02902"] {
            assert_eq!(resolve_scope_in(&store, id).unwrap(), "TASK-2902");
        }
        for scope in [
            "TASK-2902 TASK-2903",
            "TASK-2902,TASK-2903",
            "epic:all",
            "unknown",
        ] {
            let error = resolve_scope_in(&store, scope).unwrap_err().to_string();
            assert!(error.contains(scope), "{error}");
        }
        store.requirements[0].req_type = aida_core::RequirementType::Epic;
        assert!(resolve_scope_in(&store, "TASK-2902")
            .unwrap_err()
            .to_string()
            .contains("epic"));
        store.requirements[0].req_type = aida_core::RequirementType::Task;
        store.requirements[0].archived = true;
        assert!(resolve_scope_in(&store, "TASK-2902").is_err());
        store.requirements[0].archived = false;
        store.requirements[0].status = aida_core::RequirementStatus::Rejected;
        assert!(resolve_scope_in(&store, "TASK-2902").is_err());
    }
}
