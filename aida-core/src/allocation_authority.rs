//! Clone-local allocation authority. Shared store content never grants authority.
// trace:BUG-2017 | ai:codex
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "aida-allocation.toml";

pub use crate::allocation_lineage::AllocationRole;

/// The clone-local allocation setting. Version 1 holds `authoritative`;
/// version 2 holds `role` and, off the root, the declared `upstream` remote.
/// `authoritative` stays the derived "this clone is the root" answer every
/// BUG-2017 gate keys on.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
#[derive(Clone, Debug, Serialize)]
pub struct AllocationMode {
    pub version: u32,
    pub authoritative: bool,
    pub role: AllocationRole,
    pub upstream: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeV1 {
    #[allow(dead_code)]
    version: u32,
    authoritative: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeV2 {
    #[allow(dead_code)]
    version: u32,
    role: AllocationRole,
    #[serde(default)]
    upstream: Option<String>,
}

impl AllocationMode {
    fn node_local() -> Self {
        Self {
            version: 1,
            authoritative: false,
            role: AllocationRole::Node,
            upstream: None,
        }
    }
}

/// A git remote name usable as an upstream: plain, bounded, no path parts.
pub fn valid_remote_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['.', '-'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn parse_mode(text: &str) -> Result<AllocationMode> {
    let invalid = || anyhow::anyhow!("invalid allocation setting");
    let table: toml::Table = toml::from_str(text).map_err(|_| invalid())?;
    match table.get("version").and_then(toml::Value::as_integer) {
        Some(1) => {
            let v1: ModeV1 = toml::from_str(text).map_err(|_| invalid())?;
            Ok(AllocationMode {
                version: 1,
                authoritative: v1.authoritative,
                role: if v1.authoritative {
                    AllocationRole::Root
                } else {
                    AllocationRole::Node
                },
                upstream: None,
            })
        }
        Some(2) => {
            let v2: ModeV2 = toml::from_str(text).map_err(|_| invalid())?;
            if let Some(upstream) = &v2.upstream {
                if v2.role == AllocationRole::Root {
                    bail!("invalid allocation setting: the root has no upstream")
                }
                if !valid_remote_name(upstream) {
                    bail!("invalid allocation setting: upstream is not a plain git remote name")
                }
            }
            Ok(AllocationMode {
                version: 2,
                authoritative: v2.role == AllocationRole::Root,
                role: v2.role,
                upstream: v2.upstream,
            })
        }
        Some(_) => bail!("unsupported allocation setting version"),
        None => Err(invalid()),
    }
}

/// Write the version-2 setting into the clone's common Git directory. Only
/// explicit operator commands call this (`aida node enroll-root`,
/// `aida node upstream`); shared store content never does.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
pub fn write_mode(root: &Path, role: AllocationRole, upstream: Option<&str>) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(upstream) = upstream {
        anyhow::ensure!(role == AllocationRole::Node, "the root has no upstream");
        anyhow::ensure!(
            valid_remote_name(upstream),
            "upstream must be a plain git remote name"
        );
    }
    use crate::toml_quote::toml_string;
    let mut text = format!(
        "version = 2\nrole = {}\n",
        toml_string(match role {
            AllocationRole::Root => "root",
            AllocationRole::Node => "node",
        })
    );
    if let Some(upstream) = upstream {
        text.push_str(&format!("upstream = {}\n", toml_string(upstream)));
    }
    let common = common_dir(root)?;
    let tmp = common.join(format!("{CONFIG_FILE}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&tmp, common.join(CONFIG_FILE))?;
    Ok(())
}

// The legacy model API is infallible and signals a refused allocation with
// an empty display ID. Keep the refusal's cause for the save boundary so the
// caller sees the real reason rather than a generic one.
// trace:BUG-b6b1278890774ef993d2ebc327862470-001 | ai:claude
thread_local! {
    static LAST_FAILURE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Record why an allocation was refused (latest refusal wins).
pub fn record_failure(error: &anyhow::Error) {
    LAST_FAILURE.with(|f| *f.borrow_mut() = Some(format!("{error:#}")));
}

/// Take the recorded refusal cause, clearing it.
pub fn take_failure() -> Option<String> {
    LAST_FAILURE.with(|f| f.borrow_mut().take())
}

/// `"<what>: <cause>; <outcome>"`, or `"<what>; <outcome>"` with no recorded cause.
pub fn failure_message(what: &str, outcome: &str) -> String {
    match take_failure() {
        Some(cause) => format!("{what}: {cause}; {outcome}"),
        None => format!("{what}; {outcome}"),
    }
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
            return Ok(AllocationMode::node_local())
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
    parse_mode(&text)
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
        // Name the missing piece rather than only "not authoritative".
        // trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
        match resolve(root)?.upstream {
            None => bail!(
                "id_format policy is `blocks-only` but this clone cannot use blocks: {}",
                crate::allocation_lineage::Unusable::NoUpstream
            ),
            Some(upstream) => bail!(
                "id_format policy is `blocks-only` but this clone cannot dispense from a block granted by its upstream `{upstream}` yet; it may allocate only node-local IDs"
            ),
        }
    }
    Ok(crate::IdFormatPolicy::NodeAwareOnly)
}

/// Why a non-root clone issues node-local `type_prefix` IDs although
/// blocks are configured, read from its upstream setting and the store's
/// lineage records. `None` on the root, which dispenses as before.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
pub fn downgrade_reason(root: &Path, type_prefix: &str) -> Result<Option<String>> {
    use crate::allocation_lineage::{LineageView, PoolRegistry, RootRecord};
    let mode = resolve(root)?;
    if mode.authoritative {
        return Ok(None);
    }
    let node = match node(root) {
        Ok(node) => node,
        Err(e) => return Ok(Some(format!("{e:#}"))),
    };
    let pools = PoolRegistry::load(root)?;
    let blocks = crate::node::BlockRegistry::load(&root.join("registry/blocks.yaml"))?;
    let root_record = RootRecord::load(root)?;
    let view = LineageView {
        pools: &pools.pools,
        blocks: &blocks.blocks,
        root: root_record.as_ref(),
        node: &node,
        role: mode.role,
        upstream: mode.upstream.as_deref(),
    };
    Ok(Some(match view.spendable_block(type_prefix) {
        // Claims, the only way a node gains such a block, wire dispensing.
        Ok(_) => format!(
            "this clone holds a delegated {} block, but dispensing from delegated blocks is not enabled yet",
            type_prefix.to_ascii_uppercase()
        ),
        Err(reason) => reason.to_string(),
    }))
}

/// True the first time `reason` is seen in this session (the AIDA session,
/// else the invoking shell), so the downgrade notice prints once, not on
/// every `aida add`. Every reason seen in the session is remembered (reasons
/// name their type, so this is per type); a new session starts empty. The
/// marker lives beside the clone-local setting: the session id on the first
/// line, then one seen reason per line.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
pub fn first_notice_this_session(root: &Path, reason: &str) -> bool {
    let session = std::env::var("AIDA_SESSION_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("ppid-{}", std::os::unix::process::parent_id()));
    let Ok(common) = common_dir(root) else {
        return true;
    };
    let marker = common.join("aida-allocation-notice");
    let reason = reason.replace(['\n', '\r'], " ");
    let text = std::fs::read_to_string(&marker).unwrap_or_default();
    let mut lines = text.lines();
    let mut seen: Vec<&str> = if lines.next() == Some(session.as_str()) {
        lines.collect()
    } else {
        Vec::new()
    };
    if seen.contains(&reason.as_str()) {
        return false;
    }
    seen.push(&reason);
    let _ = crate::write_atomic(&marker, format!("{session}\n{}\n", seen.join("\n")));
    true
}

/// Refusal for a clone that has never claimed a node (no work binding and no
/// legacy `.aida/node.toml`).
pub const NO_NODE_BINDING: &str = "this clone has no AIDA node binding yet; run `aida node acquire` (or `aida init`) to claim a node id before issuing new IDs";

pub fn node(root: &Path) -> Result<String> {
    if let Some(binding) = crate::work_binding::load(root)? {
        return Ok(binding.node_id);
    }
    // A fresh clone has neither binding; name the repair, not the bare ENOENT.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-019 | ai:claude
    let legacy = root.join(".aida/node.toml");
    if !legacy.try_exists()? {
        bail!("{NO_NODE_BINDING}")
    }
    let node = crate::NodeConfig::load(&legacy)?.node_id;
    if node.is_empty()
        || node.len() > 128
        || !node.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        bail!("node-local allocation requires a valid explicit node binding")
    }
    Ok(node)
}

fn local_id(id: &str, node: &str) -> bool {
    let id = id.to_ascii_uppercase();
    let marker = format!("-{}-", node.to_ascii_uppercase());
    let Some((prefix, sequence)) = id.rsplit_once(marker.as_str()) else {
        return false;
    };
    let prefix = prefix.strip_prefix('$').unwrap_or(prefix);
    !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_uppercase())
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
    admit_with_binding(incoming, existing, || node(root))
}

// Exactly the same ownership and local-ID admission for an unspent candidate.
// trace:STORY-1742 | ai:codex
// trace:BUG-2017 | ai:codex
pub(crate) fn admit_candidate(
    node: &str,
    incoming: &[crate::Requirement],
    existing: &[crate::Requirement],
) -> Result<()> {
    admit_with_binding(incoming, existing, || Ok(node.to_owned()))
}

fn admit_with_binding(
    incoming: &[crate::Requirement],
    existing: &[crate::Requirement],
    mut resolve_node: impl FnMut() -> Result<String>,
) -> Result<()> {
    // trace:BUG-2017 | ai:codex — reserve the complete candidate namespace
    // before any row can be persisted. Spec and agreed IDs share ownership;
    // aliases of the same UUID are valid, including canonical case variants.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — ownership
    // is keyed by NUMERIC identity (TASK-0042 == TASK-42). A (key, UUID)
    // pair already on disk is not a new claim, so a pre-existing collision
    // (repaired by `aida doctor --fix id-collisions`) never blocks unrelated
    // saves; only a claim this write introduces is checked.
    fn ids(req: &crate::Requirement) -> impl Iterator<Item = String> + '_ {
        [req.spec_id.as_deref(), req.agreed_id.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.trim().is_empty())
            .map(crate::id_identity::id_key)
    }
    let held: std::collections::HashSet<(String, uuid::Uuid)> = existing
        .iter()
        .flat_map(|r| ids(r).map(move |k| (k, r.id)))
        .collect();
    let mut holders: std::collections::HashMap<String, std::collections::HashSet<uuid::Uuid>> =
        std::collections::HashMap::new();
    for req in existing.iter().chain(incoming) {
        for key in ids(req) {
            holders.entry(key).or_default().insert(req.id);
        }
    }
    for req in incoming {
        for key in ids(req) {
            if held.contains(&(key.clone(), req.id)) {
                continue;
            }
            if holders
                .get(&key)
                .is_some_and(|owners| owners.iter().any(|o| *o != req.id))
            {
                bail!("display ID already belongs to another UUID; nothing was written")
            }
        }
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
                    None => binding.insert(resolve_node()?),
                };
                if !crate::object_store::valid_spec_id_format(id) || !local_id(id, binding) {
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
    admit_meta_with_binding(incoming, existing, || node(root))
}

pub(crate) fn admit_candidate_meta(
    node: &str,
    incoming: &[(uuid::Uuid, Option<String>)],
    existing: &[(uuid::Uuid, Option<String>)],
) -> Result<()> {
    admit_meta_with_binding(incoming, existing, || Ok(node.to_owned()))
}

fn admit_meta_with_binding(
    incoming: &[(uuid::Uuid, Option<String>)],
    existing: &[(uuid::Uuid, Option<String>)],
    mut resolve_node: impl FnMut() -> Result<String>,
) -> Result<()> {
    for (uuid, id) in incoming {
        if let Some(id) = id {
            if existing
                .iter()
                .any(|(old_uuid, old_id)| old_uuid == uuid && old_id.as_ref() == Some(id))
            {
                continue;
            }
            if !local_id(id, &resolve_node()?) {
                bail!("new global metadata IDs refuse on a non-authoritative clone")
            }
        }
    }
    Ok(())
}

/// Registry serializers cannot mint claims through alternate paths or aliases.
pub fn admit_registry_write(path: &Path) -> Result<()> {
    #[cfg(unix)]
    crate::transport_policy::refuse_unsupported(path)?;
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
    #[cfg(unix)]
    crate::transport_policy::refuse_unsupported(path)?;
    // trace:BUG-2017 | ai:codex — inspect the original bound path before resolving aliases.
    crate::work_binding::bound_counter_state(path)?;
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
    // trace:BUG-2017 | ai:codex — opaque nodes have one existing counter,
    // including direct provider construction, not merely the model wrapper.
    if crate::work_binding::load(root)?.is_some() {
        if path != crate::work_binding::counter_path(root)? || !path.try_exists()? {
            bail!(
                "bound WORK counter missing or alternate provider path; explicit recovery required"
            )
        }
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
    migrate_registered_legacy: bool,
}
impl CloneDispenser {
    fn inner(&self) -> Result<crate::models::DispenserHandle> {
        #[cfg(unix)]
        crate::transport_policy::refuse_unsupported(&self.root)?;
        let mode = resolve(&self.root)?;
        if mode.authoritative {
            return self
                .inner
                .clone()
                .context("authoritative allocator has no configured dispenser");
        }
        // trace:BUG-2017 | ai:codex — lifecycle migration is lazy, after model
        // identity admission. Loading a provider for a refused global request
        // must never publish a new binding or advance any counter.
        if self.migrate_registered_legacy
            && crate::work_binding::load(&self.root)?.is_none()
            && self.root.join(".aida/node.toml").try_exists()?
        {
            crate::work_binding::bootstrap(&self.root)?;
        }
        let binding = node(&self.root)?;
        let inner = if let Some(inner) = &self.inner {
            inner.clone()
        } else {
            // trace:BUG-2017 | ai:codex — a bound counter may never be re-created.
            let provider = if crate::work_binding::load(&self.root)?.is_some() {
                crate::dispenser::FileDispenser::open_existing(
                    crate::work_binding::counter_path(&self.root)?,
                    crate::dispenser::IdMode::Distributed {
                        node_id: binding.clone(),
                    },
                )?
            } else {
                crate::dispenser::FileDispenser::open(
                    self.root.join(".aida/dispenser.toml"),
                    crate::dispenser::IdMode::Distributed {
                        node_id: binding.clone(),
                    },
                )?
            };
            crate::models::DispenserHandle(std::sync::Arc::new(provider))
        };
        // Check lazy and supplied providers alike, even when a local counter
        // path aliases an independent file outside this clone.
        let state = inner.state()?;
        // A supplied lifecycle provider may have performed one-time migration.
        let binding = node(&self.root)?;
        if state.mode
            != (crate::dispenser::IdMode::Distributed {
                node_id: binding.clone(),
            })
        {
            // trace:BUG-b6b1278890774ef993d2ebc327862470-001 | ai:claude
            bail!(
                "dispenser does not match this clone's node binding (clone is node {binding}, dispenser records {}); run `{}` for the repair",
                crate::work_binding::mode_label(&state.mode),
                crate::work_binding::DOCTOR_ALLOCATION_COMMAND
            )
        }
        Ok(inner)
    }
}
impl crate::dispenser::Dispenser for CloneDispenser {
    fn node_local_only(&self) -> bool {
        true
    }
    fn admit_requirement(&self, req: &crate::Requirement) -> Result<()> {
        if resolve(&self.root)?.authoritative {
            return Ok(());
        }
        if req
            .prefix_override
            .as_ref()
            .is_some_and(|p| crate::Requirement::validate_prefix(p).is_none())
        {
            bail!("invalid node-local requirement prefix")
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
        let mut probe = req.clone();
        if probe.spec_id.is_none() {
            probe.spec_id = Some(format!("REQ-{}-1", node(&self.root)?));
        }
        admit(&self.root, &[probe], &existing)
    }
    fn format_id(&self, prefix: &str, seq: u32) -> Result<String> {
        if prefix.strip_prefix('$').unwrap_or(prefix).is_empty()
            || prefix.len() > 128
            || !prefix
                .strip_prefix('$')
                .unwrap_or(prefix)
                .chars()
                .all(|c| c.is_ascii_uppercase())
        {
            bail!("invalid node-local allocation prefix")
        }
        let inner = self.inner()?;
        inner.state()?;
        let binding = node(&self.root)?;
        if seq == 0 {
            bail!("node-local sequence is exhausted or invalid")
        }
        // The clone decides the namespace; a custom provider cannot turn an
        // admitted local sequence into a bare global display ID.
        // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — `[ids] node_seq_width`.
        let width = crate::id_identity::installed_widths().node_mint_width();
        Ok(format!("{prefix}-{binding}-{seq:0width$}"))
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
            migrate_registered_legacy: false,
        },
    )))
}

/// Supported CLI/MCP lifecycle provider: migrate registered legacy state on
/// the first admitted allocation, never merely while dispatching a request.
// trace:BUG-2017 | ai:codex
pub fn lifecycle_dispenser(root: &Path) -> crate::models::DispenserHandle {
    crate::models::DispenserHandle(std::sync::Arc::new(CloneDispenser {
        root: root.into(),
        inner: None,
        migrate_registered_legacy: true,
    }))
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
    anyhow::ensure!(
        common.starts_with(std::env::temp_dir().canonicalize()?),
        "fixture authority requires a temporary common Git directory"
    );
    let setting = common.join(CONFIG_FILE);
    if setting.exists() {
        require_global(root)?;
    } else {
        crate::write_atomic(&setting, "version=1\nauthoritative=true\n")?;
    }
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
    // trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
    #[test]
    fn v2_setting_carries_role_and_upstream_and_v1_reads_as_root() {
        let (dir, _) = fixture(true);
        let mode = resolve(dir.path()).unwrap();
        assert_eq!(
            (mode.role, mode.authoritative),
            (AllocationRole::Root, true)
        );
        write_mode(dir.path(), AllocationRole::Node, Some("gitlab")).unwrap();
        let mode = resolve(dir.path()).unwrap();
        assert_eq!(mode.version, 2);
        assert_eq!(mode.role, AllocationRole::Node);
        assert!(!mode.authoritative);
        assert_eq!(mode.upstream.as_deref(), Some("gitlab"));
        write_mode(dir.path(), AllocationRole::Root, None).unwrap();
        assert!(resolve(dir.path()).unwrap().authoritative);
        assert!(write_mode(dir.path(), AllocationRole::Root, Some("origin")).is_err());
        assert!(write_mode(dir.path(), AllocationRole::Node, Some("../x")).is_err());
        for text in [
            "version=2\nrole=\"root\"\nupstream=\"origin\"",
            "version=2\nrole=\"node\"\nupstream=\"a/b\"",
            "version=2\nrole=\"leaf\"",
            "version=2\nrole=\"node\"\nauthoritative=true",
        ] {
            std::fs::write(dir.path().join(".git").join(CONFIG_FILE), text).unwrap();
            assert!(resolve(dir.path()).is_err(), "{text}");
        }
    }

    // trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
    #[test]
    fn downgrade_reason_names_the_missing_piece_once_per_session() {
        let (dir, _) = fixture(false);
        let reason = downgrade_reason(dir.path(), "BUG").unwrap().unwrap();
        assert!(reason.contains("no upstream"), "{reason}");
        write_mode(dir.path(), AllocationRole::Node, Some("gitlab")).unwrap();
        let reason = downgrade_reason(dir.path(), "BUG").unwrap().unwrap();
        assert!(reason.contains("holds no BUG block"), "{reason}");
        let err = effective_policy(dir.path(), crate::IdFormatPolicy::BlocksOnly)
            .unwrap_err()
            .to_string();
        assert!(err.contains("upstream `gitlab`"), "{err}");
        let _session = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ID", "s-095-a");
        assert!(first_notice_this_session(dir.path(), &reason));
        assert!(!first_notice_this_session(dir.path(), &reason));
        assert!(first_notice_this_session(dir.path(), "a different reason"));
        drop(_session);
        let (home, _) = fixture(true);
        assert_eq!(downgrade_reason(home.path(), "BUG").unwrap(), None);
    }

    // trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
    #[test]
    fn downgrade_notice_remembers_every_reason_until_the_session_changes() {
        let (dir, _) = fixture(false);
        write_mode(dir.path(), AllocationRole::Node, Some("gitlab")).unwrap();
        let bug = downgrade_reason(dir.path(), "BUG").unwrap().unwrap();
        let task = downgrade_reason(dir.path(), "TASK").unwrap().unwrap();
        assert_ne!(bug, task);
        let mut session = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ID", "s-095-b");
        // Alternating types prints each reason once, never again.
        assert!(first_notice_this_session(dir.path(), &bug));
        assert!(first_notice_this_session(dir.path(), &task));
        assert!(!first_notice_this_session(dir.path(), &bug));
        assert!(!first_notice_this_session(dir.path(), &task));
        // A new session starts with nothing seen.
        session.reset("s-095-c");
        assert!(first_notice_this_session(dir.path(), &bug));
        assert!(!first_notice_this_session(dir.path(), &bug));
        assert!(first_notice_this_session(dir.path(), &task));
    }

    // trace:BUG-b6b1278890774ef993d2ebc327862470-019 | ai:claude
    #[test]
    fn unbound_clone_add_names_the_node_repair() {
        let (dir, backend) = fixture(false);
        std::fs::remove_file(dir.path().join(".aida/node.toml")).unwrap();
        let err = backend
            .add_requirement(Requirement::new("x".into(), "fixture".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains(NO_NODE_BINDING), "{err}");
        assert!(!err.contains("os error 2"), "{err}");
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
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046.ac5bd8aa | ai:claude
    #[test]
    fn bug_2017_imported_local_id_cannot_be_overwritten_by_new_uuid() {
        let (_dir, backend) = fixture(false);
        let mut imported = Requirement::new("imported local".into(), "fixture".into());
        imported.req_type = crate::RequirementType::Bug;
        imported.spec_id = Some("BUG-7-001".into());
        let imported = backend.add_requirement(imported).unwrap();
        let mut next = Requirement::new("new local".into(), "fixture".into());
        next.req_type = crate::RequirementType::Bug;
        // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — the
        // shared mint seam skips the taken number (it used to refuse the add
        // and leave the retry to the caller); the import is never overwritten.
        assert_eq!(
            backend.add_requirement(next).unwrap().spec_id.as_deref(),
            Some("BUG-7-002")
        );
        assert_eq!(
            backend
                .get_requirement_by_spec_id("BUG-7-001")
                .unwrap()
                .unwrap()
                .id,
            imported.id
        );
    }
    #[test]
    fn bug_2017_bulk_collision_reassigns_only_to_an_unused_local_id() {
        let (_dir, backend) = fixture(false);
        let mut batch = backend.bulk_writer().unwrap();
        let mut req = Requirement::new("batch local".into(), "fixture".into());
        req.req_type = crate::RequirementType::Bug;
        let batch_uuid = req.id;
        assert_eq!(
            batch.add(req).unwrap().spec_id.as_deref(),
            Some("BUG-7-001")
        );
        let mut imported = Requirement::new("intervening import".into(), "fixture".into());
        imported.req_type = crate::RequirementType::Bug;
        imported.spec_id = Some("BUG-7-001".into());
        let imported = backend.add_requirement(imported).unwrap();
        assert_eq!(batch.finish("fixture").unwrap(), 1);
        assert_eq!(
            backend
                .get_requirement_by_spec_id("BUG-7-001")
                .unwrap()
                .unwrap()
                .id,
            imported.id
        );
        let reassigned = backend.get_requirement(&batch_uuid).unwrap().unwrap();
        assert_eq!(reassigned.spec_id.as_deref(), Some("BUG-7-002"));
        assert!(reassigned.agreed_id.is_none());
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
    // trace:BUG-2017 | ai:codex — a rejected batch leaves every persistence
    // surface byte-for-byte intact, including a nonempty baseline and refs.
    fn persistence_snapshot(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        fn collect(
            root: &Path,
            path: &Path,
            out: &mut std::collections::BTreeMap<String, Vec<u8>>,
        ) {
            if !path.exists() {
                return;
            }
            if path.is_dir() {
                for entry in std::fs::read_dir(path).unwrap() {
                    collect(root, &entry.unwrap().path(), out);
                }
            } else {
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
        let mut out = std::collections::BTreeMap::new();
        for name in [
            "metadata.yaml",
            "oplog.yaml",
            "objects",
            ".aida/dispenser.toml",
        ] {
            collect(root, &root.join(name), &mut out);
        }
        let refs = crate::git_ops::host_git_command(root)
            .args(["show-ref", "--head"])
            .output()
            .unwrap();
        assert!(refs.status.success());
        out.insert("git-refs".into(), refs.stdout);
        out
    }

    #[test]
    fn bug_2017_batch_alias_conflicts_refuse_before_any_persistence() {
        for operation in ["save", "import", "atomic", "bulk"] {
            for cross_field in [false, true] {
                let (dir, backend) = fixture(false);
                let mut seed = Requirement::new("baseline".into(), "fixture".into());
                seed.spec_id = Some("BUG-7-100".into());
                backend.add_requirement(seed).unwrap();
                let mut baseline = backend.load().unwrap();
                baseline.title = "baseline".into();
                backend.save(&baseline).unwrap();
                let commit = crate::test_git::git_command()
                    .current_dir(dir.path())
                    .args([
                        "-c",
                        "user.name=Fixture",
                        "-c",
                        "user.email=fixture@example.invalid",
                        "add",
                        "metadata.yaml",
                        "oplog.yaml",
                        "objects",
                    ])
                    .status()
                    .unwrap();
                assert!(commit.success());
                let commit = crate::test_git::git_command()
                    .current_dir(dir.path())
                    .args([
                        "-c",
                        "user.name=Fixture",
                        "-c",
                        "user.email=fixture@example.invalid",
                        "commit",
                        "-qm",
                        "baseline",
                    ])
                    .status()
                    .unwrap();
                assert!(commit.success());
                let mut first = Requirement::new("first".into(), "fixture".into());
                first.spec_id = Some("BUG-7-001".into());
                first.agreed_id = Some("bug-7-009".into());
                let mut second = Requirement::new("second".into(), "fixture".into());
                second.spec_id = Some(
                    if cross_field {
                        "BUG-7-009"
                    } else {
                        "BUG-7-002"
                    }
                    .into(),
                );
                if !cross_field {
                    second.agreed_id = Some("BUG-7-009".into());
                }
                let before = persistence_snapshot(dir.path());
                let result = match operation {
                    "save" | "import" => {
                        let mut store = backend.load().unwrap();
                        store.title = "must not persist".into();
                        store.requirements.extend([first, second]);
                        if operation == "import" {
                            backend.save_imported(&store)
                        } else {
                            backend.save(&store)
                        }
                    }
                    "atomic" => backend
                        .update_atomically(|store| {
                            store.title = "must not persist".into();
                            store.requirements.extend([first, second]);
                        })
                        .map(|_| ()),
                    "bulk" => {
                        let mut writer = backend.bulk_writer().unwrap();
                        writer.add(first).unwrap();
                        writer.add(second).unwrap();
                        writer.finish("must not persist").map(|_| ())
                    }
                    _ => unreachable!(),
                };
                assert!(
                    result.unwrap_err().to_string().contains("another UUID"),
                    "{operation}/{cross_field}"
                );
                assert_eq!(
                    persistence_snapshot(dir.path()),
                    before,
                    "{operation}/{cross_field}"
                );
            }
        }
    }

    #[test]
    fn bug_2017_same_uuid_aliases_and_existing_ownership_are_preserved() {
        let (dir, backend) = fixture(false);
        let mut req = Requirement::new("same owner".into(), "fixture".into());
        req.spec_id = Some("BUG-7-001".into());
        req.agreed_id = Some("bug-7-001".into());
        let mut req = backend.add_requirement(req).unwrap();
        req.title = "allowed edit".into();
        backend.update_requirement(&req).unwrap();
        let mut other = Requirement::new("different owner".into(), "fixture".into());
        other.spec_id = Some("BUG-7-002".into());
        other.agreed_id = Some("bug-7-001".into());
        assert!(admit(dir.path(), &[other], &[req]).is_err());
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
        for prefix in ["BUG-42", "BAD_PREFIX", "$USER", ""] {
            let mut req = Requirement::new("bad prefix".into(), "fixture".into());
            req.prefix_override = Some(prefix.into());
            assert!(backend.add_requirement(req).is_err());
            assert!(!dir.path().join(".aida/dispenser.toml").exists());
        }
        let mut store = backend.load().unwrap();
        assert!(store
            .generate_requirement_id(None, Some("$USER"))
            .is_empty());
        assert!(store.next_meta_id("$").is_empty());
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
    fn bug_2017_sqlite_stale_mode_refuses_before_writable_setup() {
        let (dir, _) = fixture(false);
        let path = dir.path().join(".aida/stale.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE dispenser_meta (key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();
        conn.execute(
            "INSERT INTO dispenser_meta VALUES ('mode', ?1)",
            [serde_json::to_string(&crate::dispenser::IdMode::Centralized).unwrap()],
        )
        .unwrap();
        drop(conn);
        let before = std::fs::read(&path).unwrap();
        assert!(crate::dispenser::SqliteDispenser::open(
            path.clone(),
            crate::dispenser::IdMode::Distributed {
                node_id: "7".into()
            }
        )
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!path.with_extension("db-wal").exists());
        assert!(!path.with_extension("db-shm").exists());
    }
    #[test]
    fn bug_2017_concurrent_work_writers_reserve_distinct_local_sequences() {
        let (dir, backend) = fixture(false);
        let before = backend.load().unwrap().next_spec_number;
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let root = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    GitBackend::new(&root)
                        .unwrap()
                        .with_auto_commit(false)
                        .add_requirement(Requirement::new(format!("local {i}"), "fixture".into()))
                        .unwrap()
                        .spec_id
                        .unwrap()
                })
            })
            .collect();
        let ids: std::collections::HashSet<_> =
            threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(ids.len(), 8);
        assert!(ids.iter().all(|id| id.starts_with("FR-7-")));
        let after = backend.load().unwrap();
        assert_eq!(after.requirements.len(), 8);
        assert_eq!(after.next_spec_number, before);
        assert!(after.requirements.iter().all(|req| req.agreed_id.is_none()));
        assert!(!dir.path().join("registry/blocks.yaml").exists());
    }
    #[test]
    fn bug_2017_first_model_allocation_initializes_under_permanent_dispense_lock() {
        let (dir, _) = fixture(false);
        let path = dir.path().join(".aida/dispenser.toml");
        let handles = crate::BlockRegistry::with_dispense_lock(&path, || {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(33));
            let handles: Vec<_> = (0..32)
                .map(|_| {
                    let root = dir.path().to_path_buf();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        let mut store = GitBackend::new(&root).unwrap().load().unwrap();
                        barrier.wait();
                        store.generate_requirement_id(None, Some("BUG"))
                    })
                })
                .collect();
            barrier.wait();
            std::thread::sleep(std::time::Duration::from_millis(100));
            // This held lock must also protect first-time initialization.
            assert!(!path.exists());
            Ok(handles)
        })
        .unwrap();
        let ids: std::collections::HashSet<_> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(ids.len(), 32);
        assert!(ids.iter().all(|id| id.starts_with("BUG-7-")));
        let state: crate::dispenser::DispenserState =
            toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(state.sequences.values().sum::<u32>(), 32);
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
    fn bug_2017_home_allocator_failure_refuses_before_metadata_or_object_write() {
        struct Failed;
        impl crate::dispenser::Dispenser for Failed {
            fn next(&self, _: &str) -> Result<u32> {
                bail!("fixture failure")
            }
            fn peek(&self, _: &str) -> Result<u32> {
                bail!("fixture failure")
            }
            fn state(&self) -> Result<crate::dispenser::DispenserState> {
                Ok(crate::dispenser::DispenserState::new(
                    crate::dispenser::IdMode::Centralized,
                ))
            }
        }
        for format in [
            crate::models::IdFormat::SingleLevel,
            crate::models::IdFormat::TwoLevel,
        ] {
            let (dir, backend) = fixture(true);
            let mut store = backend.load().unwrap();
            store.id_config.format = format;
            backend.save(&store).unwrap();
            let metadata = std::fs::read(dir.path().join("metadata.yaml")).unwrap();
            let oplog = std::fs::read(dir.path().join("oplog.yaml")).ok();
            let backend =
                backend.with_dispenser(crate::models::DispenserHandle(std::sync::Arc::new(Failed)));
            assert!(backend
                .add_requirement(Requirement::new("failure".into(), "fixture".into()))
                .is_err());
            assert_eq!(
                std::fs::read(dir.path().join("metadata.yaml")).unwrap(),
                metadata
            );
            assert!(
                crate::object_store::list_objects(&dir.path().join("objects"))
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(dir.path().join("oplog.yaml")).ok(), oplog);
            for team in [false, true] {
                let mut store = backend.load().unwrap();
                let id = if team {
                    store.add_team_with_id("fixture".into(), "fixture".into(), None)
                } else {
                    store.add_user_with_id(
                        "fixture".into(),
                        "fixture@example.invalid".into(),
                        "fixture".into(),
                    )
                };
                assert!(id.is_empty());
                assert!(backend.save(&store).is_err());
                assert_eq!(
                    std::fs::read(dir.path().join("metadata.yaml")).unwrap(),
                    metadata
                );
                assert_eq!(std::fs::read(dir.path().join("oplog.yaml")).ok(), oplog);
            }
        }
    }
    // trace:BUG-b6b1278890774ef993d2ebc327862470-001 | ai:claude
    #[test]
    fn refused_allocation_reports_the_dispenser_cause() {
        struct Failed;
        impl crate::dispenser::Dispenser for Failed {
            fn next(&self, _: &str) -> Result<u32> {
                bail!("fixture failure")
            }
            fn peek(&self, _: &str) -> Result<u32> {
                bail!("fixture failure")
            }
            fn state(&self) -> Result<crate::dispenser::DispenserState> {
                Ok(crate::dispenser::DispenserState::new(
                    crate::dispenser::IdMode::Centralized,
                ))
            }
        }
        let (_dir, backend) = fixture(true);
        let backend =
            backend.with_dispenser(crate::models::DispenserHandle(std::sync::Arc::new(Failed)));
        let err = backend
            .add_requirement(Requirement::new("failure".into(), "fixture".into()))
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "requirement ID allocation failed: fixture failure; no metadata or object was written"
        );
        assert!(take_failure().is_none(), "the cause is consumed");
    }
    #[cfg(unix)]
    #[test]
    fn group_writable_node_toml_refusal_names_the_file_and_chmod() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, backend) = fixture(false);
        let node = dir.path().join(".aida/node.toml");
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o664)).unwrap();
        let err = backend
            .with_dispenser(lifecycle_dispenser(dir.path()))
            .add_requirement(Requirement::new("unsafe".into(), "fixture".into()))
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with("requirement ID allocation failed: "),
            "{err}"
        );
        assert!(err.contains("(mode 0664)"), "{err}");
        assert!(err.contains("run `chmod go-w "), "{err}");
        assert!(err.contains("node.toml"), "{err}");
    }
    #[cfg(unix)]
    #[test]
    fn legacy_dispenser_on_another_node_refusal_names_both_nodes() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, backend) = fixture(false);
        let node = dir.path().join(".aida/node.toml");
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o644)).unwrap();
        let legacy = dir.path().join(".aida/dispenser.toml");
        std::fs::write(
            &legacy,
            "[mode.Distributed]\nnode_id = \"2\"\n\n[sequences]\nGLOBAL = 9\n",
        )
        .unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = backend
            .with_dispenser(lifecycle_dispenser(dir.path()))
            .add_requirement(Requirement::new("mismatch".into(), "fixture".into()))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("legacy node/provider binding mismatch"),
            "{err}"
        );
        assert!(err.contains("bound to node 7"), "{err}");
        assert!(err.contains("records node 2"), "{err}");
    }
    #[test]
    fn bug_2017_two_level_work_refuses_before_counters_home_retains_format() {
        let (dir, backend) = fixture(false);
        let mut store = backend.load().unwrap();
        store.id_config.format = crate::models::IdFormat::TwoLevel;
        assert!(store
            .generate_requirement_id(Some("APP"), Some("BUG"))
            .is_empty());
        assert!(!dir.path().join(".aida/dispenser.toml").exists());
        let (_home, backend) = fixture(true);
        let memory = crate::models::DispenserHandle(std::sync::Arc::new(
            crate::dispenser::MemoryDispenser::new(crate::dispenser::IdMode::Centralized),
        ));
        let mut store = backend.with_dispenser(memory).load().unwrap();
        store.id_config.format = crate::models::IdFormat::TwoLevel;
        assert_eq!(
            store.generate_requirement_id(Some("APP"), Some("BUG")),
            "APP-BUG-001"
        );
    }
    #[test]
    fn bug_2017_stale_stored_mode_refuses_without_rewriting_counter_state() {
        for mode in [
            crate::dispenser::IdMode::Centralized,
            crate::dispenser::IdMode::Distributed {
                node_id: "other".into(),
            },
        ] {
            let (dir, backend) = fixture(false);
            let state =
                toml::to_string(&crate::dispenser::DispenserState::new(mode.clone())).unwrap();
            let path = dir.path().join(".aida/dispenser.toml");
            std::fs::write(&path, &state).unwrap();
            assert!(backend
                .add_requirement(Requirement::new("stale state".into(), "fixture".into()))
                .is_err());
            assert_eq!(std::fs::read_to_string(path).unwrap(), state);
            assert!(!dir.path().join("metadata.yaml").exists());
            let outside = tempfile::tempdir().unwrap();
            let counter = outside.path().join("counter.toml");
            std::fs::write(&counter, &state).unwrap();
            std::fs::remove_file(dir.path().join(".aida/dispenser.toml")).unwrap();
            std::os::unix::fs::symlink(&counter, dir.path().join(".aida/dispenser.toml")).unwrap();
            assert!(backend
                .add_requirement(Requirement::new(
                    "aliased stale state".into(),
                    "fixture".into()
                ))
                .is_err());
            assert_eq!(std::fs::read_to_string(counter).unwrap(), state);
            assert!(!dir.path().join("metadata.yaml").exists());
        }
    }
    #[test]
    fn bug_2017_opaque_binding_survives_case_insensitive_display_lookup_and_update() {
        let (dir, backend) = fixture(false);
        let path = dir.path().join(".aida/node.toml");
        let mut node = crate::NodeConfig::load(&path).unwrap();
        node.node_id = "0123456789abcdef0123456789abcdef".into();
        node.save(&path).unwrap();
        let mut req = backend
            .add_requirement(Requirement::new("opaque local".into(), "fixture".into()))
            .unwrap();
        req.spec_id = req.spec_id.map(|id| id.to_ascii_uppercase());
        backend.update_requirement(&req).unwrap();
        assert_eq!(
            backend.get_requirement(&req.id).unwrap().unwrap().id,
            req.id
        );
    }
    #[test]
    fn bug_2017_provider_cannot_format_a_local_sequence_as_global() {
        use crate::dispenser::{Dispenser, DispenserState, IdMode, MemoryDispenser};
        struct BareFormatting(MemoryDispenser);
        impl Dispenser for BareFormatting {
            fn next(&self, prefix: &str) -> Result<u32> {
                self.0.next(prefix)
            }
            fn peek(&self, prefix: &str) -> Result<u32> {
                self.0.peek(prefix)
            }
            fn state(&self) -> Result<DispenserState> {
                self.0.state()
            }
            fn format_id(&self, _prefix: &str, seq: u32) -> Result<String> {
                Ok(format!("BUG-{seq}"))
            }
        }
        let (_dir, backend) = fixture(false);
        let provider = crate::models::DispenserHandle(std::sync::Arc::new(BareFormatting(
            MemoryDispenser::new(IdMode::Distributed {
                node_id: "7".into(),
            }),
        )));
        let mut req = Requirement::new("bound format".into(), "fixture".into());
        req.req_type = crate::RequirementType::Bug;
        let req = backend
            .with_dispenser(provider)
            .add_requirement(req)
            .unwrap();
        assert_eq!(req.spec_id.as_deref(), Some("BUG-7-001"));
    }
    #[test]
    fn bug_2017_exhausted_sequence_refuses_without_reset_or_global_fallback() {
        let (dir, backend) = fixture(false);
        let mut state =
            crate::dispenser::DispenserState::new(crate::dispenser::IdMode::Distributed {
                node_id: "7".into(),
            });
        state.sequences.insert("GLOBAL".into(), u32::MAX);
        // trace:BUG-2017 | ai:codex — S2 WORK allocates per type; exhaust
        // that actual provider key as well, retaining the global no-fallback guard.
        state.sequences.insert("FR".into(), u32::MAX);
        let bytes = toml::to_string(&state).unwrap();
        let path = dir.path().join(".aida/dispenser.toml");
        std::fs::write(&path, &bytes).unwrap();
        assert!(backend
            .add_requirement(Requirement::new("exhausted".into(), "fixture".into()))
            .is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), bytes);
        assert!(!dir.path().join("metadata.yaml").exists());
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
