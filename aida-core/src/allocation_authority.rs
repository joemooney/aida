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

/// The allocation policy this clone may use, given the configured one.
///
/// The root uses the configured policy, but only while `registry/root.yaml`
/// (when present) names this clone's node: a clone whose local setting says
/// root while the store records another holder refuses before dispensing.
/// A non-root clone may use blocks only for a type it holds a spendable
/// delegated block for ([`delegated_block`]); otherwise it allocates
/// node-local IDs.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
pub fn effective_policy(
    root: &Path,
    configured: crate::IdFormatPolicy,
) -> Result<crate::IdFormatPolicy> {
    effective_policy_for(root, configured, None)
}

/// [`effective_policy`] for one type: a non-root clone holding a spendable
/// delegated `type_prefix` block keeps the configured block policy.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
pub fn effective_policy_for(
    root: &Path,
    configured: crate::IdFormatPolicy,
    type_prefix: Option<&str>,
) -> Result<crate::IdFormatPolicy> {
    let mode = resolve(root)?;
    if mode.authoritative {
        refuse_foreign_root_record(root)?;
        return Ok(configured);
    }
    if configured.uses_blocks() {
        if let Some(typ) = type_prefix {
            if delegated_block(root, typ)?.is_some() {
                return Ok(configured);
            }
        }
    }
    if configured == crate::IdFormatPolicy::BlocksOnly {
        // Name the missing piece rather than only "not authoritative".
        // trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
        match mode.upstream {
            None => bail!(
                "id_format policy is `blocks-only` but this clone cannot use blocks: {}",
                crate::allocation_lineage::Unusable::NoUpstream
            ),
            Some(upstream) => bail!(
                "id_format policy is `blocks-only` but this clone holds no usable block granted by its upstream `{upstream}`; it may allocate only node-local IDs"
            ),
        }
    }
    Ok(crate::IdFormatPolicy::NodeAwareOnly)
}

/// Refuse when this clone's local setting says root but the store's
/// `registry/root.yaml` names a different holder (or this clone's node
/// cannot be determined to compare). No record means no claim to check.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
pub fn refuse_foreign_root_record(root: &Path) -> Result<()> {
    let Some(record) = crate::allocation_lineage::RootRecord::load(root)? else {
        return Ok(());
    };
    let node = node(root).with_context(|| {
        format!(
            "this clone is marked as the allocation root but its node is unknown, so it cannot be checked against node {}, the root recorded in {}",
            record.holder_node,
            crate::allocation_lineage::ROOT_YAML_REL
        )
    })?;
    if record.holder_node != node {
        bail!(
            "this clone is marked as the allocation root, but {} records node {} as the root and this clone is node {node}; refusing to dispense. Run `aida node show` and re-enroll or clear this clone's setting",
            crate::allocation_lineage::ROOT_YAML_REL,
            record.holder_node
        )
    }
    Ok(())
}

/// Who orders this clone's block claims, and on which remote.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimAuthority {
    /// The root claims, pushing to `origin`: the store records a root, or
    /// no hub is declared and this clone is the (legacy) authoritative one.
    Rooted,
    /// Single-hub mode: the store records no root and declares exactly one
    /// hub, which this clone reaches as `remote`. Claims are ordered by CAS
    /// push to that remote and stamped with the hub's pool id.
    HubCas { remote: String, hub: String },
    /// Rooted store, non-root clone: the root granted hub `hub` a child
    /// pool per type (`aida node hub add`), and this clone's declared
    /// upstream `remote` reaches that hub. Claims are carved inside the
    /// hub's pool and ordered by CAS push to that remote.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-170 | ai:claude
    Delegated { remote: String, hub: String },
}

impl ClaimAuthority {
    /// The remote a claim is pulled from and pushed to.
    pub fn remote(&self) -> &str {
        match self {
            Self::Rooted => "origin",
            Self::HubCas { remote, .. } | Self::Delegated { remote, .. } => remote,
        }
    }

    /// The pool id a new `type_prefix` claim is stamped with.
    pub fn pool_id(&self, type_prefix: &str) -> Option<String> {
        match self {
            Self::Rooted => None,
            Self::HubCas { hub, .. } | Self::Delegated { hub, .. } => {
                Some(crate::store_hubs::hub_pool_id(hub, type_prefix))
            }
        }
    }
}

/// Refusal when the store declares more than one hub and records no root.
pub const MULTI_HUB_NEEDS_ROOT: &str = "run `aida node enroll-root` on the primary clone: with more than one hub, only an enrolled root can split the number space between them";

/// The store's allocation mode, from its root record and declared hubs.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreMode {
    /// `registry/root.yaml` names a root: it splits the number space.
    Rooted { root: String, hubs: Vec<String> },
    /// One hub, no root: the hub's store ref orders claims.
    SingleHub { hub: String, remote: Option<String> },
    /// More than one hub and no root: claims are refused.
    MultiHubNoRoot { hubs: Vec<String> },
    /// No hub declared on the shared code default branch.
    Undeclared,
}

/// The store's allocation mode as this clone sees it.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
pub fn store_mode(root: &Path) -> Result<StoreMode> {
    let hubs = crate::store_hubs::committed(root)?;
    let names: Vec<String> = hubs.hubs.iter().map(|h| h.name.clone()).collect();
    if let Some(record) = crate::allocation_lineage::RootRecord::load(root)? {
        return Ok(StoreMode::Rooted {
            root: record.holder_node,
            hubs: names,
        });
    }
    Ok(match hubs.hubs.as_slice() {
        [] => StoreMode::Undeclared,
        [hub] => StoreMode::SingleHub {
            hub: hub.name.clone(),
            remote: crate::store_hubs::remote_reaching(root, &hubs, hub),
        },
        _ => StoreMode::MultiHubNoRoot { hubs: names },
    })
}

/// Who may order a block claim from this clone, or why none may. Replaces
/// the root-only gate at every claim site.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
pub fn claim_authority(root: &Path) -> Result<ClaimAuthority> {
    match store_mode(root)? {
        StoreMode::Rooted { .. } => {
            let mode = resolve(root)?;
            if !mode.authoritative {
                if let Some(upstream) = mode.upstream.as_deref() {
                    return delegated_authority(root, upstream);
                }
            }
            require_global(root)?;
            Ok(ClaimAuthority::Rooted)
        }
        StoreMode::Undeclared => {
            if resolve(root)?.authoritative {
                return Ok(ClaimAuthority::Rooted);
            }
            bail!("this store declares no hub, so this clone may allocate only node-local IDs. Declare the one git server the store syncs through under [store.hubs] in .aida/config.toml, commit it and push it to the shared default branch (or enroll a root with `aida node enroll-root`)")
        }
        StoreMode::SingleHub {
            hub,
            remote: Some(remote),
        } => Ok(ClaimAuthority::HubCas { remote, hub }),
        StoreMode::SingleHub { hub, remote: None } => {
            let hubs = crate::store_hubs::committed(root)?;
            let declared = hubs.get(&hub);
            let urls = declared.map(|h| h.urls.join(", ")).unwrap_or_default();
            let why = declared
                .map(|h| crate::store_hubs::partial_reach_problems(root, &hubs, h))
                .unwrap_or_default();
            let why = if why.is_empty() {
                String::new()
            } else {
                format!(" ({})", why.join("; "))
            };
            bail!("no git remote of this clone reaches the store's hub `{hub}` ({urls}){why}; claims are ordered only on the hub, so a remote counts only when every URL it fetches from and pushes to is the hub's and store pushes to it are not redirected. Push to the declared hub, or declare a second hub (that needs `aida node enroll-root`)")
        }
        StoreMode::MultiHubNoRoot { hubs } => bail!(
            "this store declares more than one hub ({}) and has no allocation root, so nothing orders their block claims; {MULTI_HUB_NEEDS_ROOT}. Blocks already held keep dispensing",
            hubs.join(", ")
        ),
    }
}

/// A non-root clone of a rooted store claims through its declared upstream
/// only when that remote reaches a declared hub the root granted a pool to.
// trace:STORY-B6B1278890774EF993D2EBC327862470-170 | ai:claude
fn delegated_authority(root: &Path, upstream: &str) -> Result<ClaimAuthority> {
    let hubs = crate::store_hubs::committed(root)?;
    let pools = crate::allocation_lineage::PoolRegistry::load(root)?;
    let reached: Vec<&crate::store_hubs::Hub> = hubs
        .hubs
        .iter()
        .filter(|h| crate::store_hubs::reach_problem(root, h, upstream).is_none())
        .collect();
    let granted = reached.iter().find(|h| {
        pools.pools.iter().any(|p| {
            p.parent_pool_id.is_some()
                && crate::allocation_lineage::promoted_hub(p) == Some(h.name.as_str())
        })
    });
    match (granted, reached.first()) {
        (Some(hub), _) => Ok(ClaimAuthority::Delegated {
            remote: upstream.to_string(),
            hub: hub.name.clone(),
        }),
        (None, Some(hub)) => bail!(
            "this clone's upstream `{upstream}` reaches hub `{}`, but the allocation root has granted that hub no ID pool yet; on the root clone run `aida node hub add {} --url <url>`, then pull the store",
            hub.name,
            hub.name
        ),
        (None, None) => bail!(
            "this clone's upstream `{upstream}` reaches no hub declared under [store.hubs] on the shared default branch, so it may allocate only node-local IDs; the root grants ID pools per hub with `aida node hub add`"
        ),
    }
}

/// Whether this clone may claim a `type_prefix` block now: it has claim
/// authority, and a delegated clone's hub holds a pool of that type.
// trace:STORY-B6B1278890774EF993D2EBC327862470-170 | ai:claude
pub fn can_claim_type(root: &Path, type_prefix: &str) -> bool {
    match claim_authority(root) {
        Ok(ClaimAuthority::Delegated { hub, .. }) => {
            crate::allocation_lineage::PoolRegistry::load(root).is_ok_and(|pools| {
                crate::allocation_lineage::hub_grant(&pools.pools, &hub, type_prefix).is_some()
            })
        }
        Ok(_) => true,
        Err(_) => false,
    }
}

/// The claims declared hub `hub`'s store ref records, as of this clone's
/// remote-tracking copy, while the store records no root. `None` once a
/// root is recorded (lineage decides then), when `hub` is not declared on
/// the shared default branch, or when no remote of this clone reaches it
/// (see [`crate::store_hubs::remote_reaching`]). Each hub-stamped block is
/// checked against its own stamped hub, so a clone reaching several
/// declared hubs keeps every block it holds; a store that gained a second
/// hub without a root refuses new claims, but blocks already ordered on a
/// hub keep dispensing.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
fn hub_claims(root: &Path, hub: &str) -> Result<Option<Vec<crate::node::AgreedIdBlock>>> {
    if crate::allocation_lineage::RootRecord::load(root)?.is_some() {
        return Ok(None);
    }
    let hubs = crate::store_hubs::committed(root)?;
    let Some(declared) = hubs.get(hub) else {
        return Ok(None);
    };
    match crate::store_hubs::remote_reaching(root, &hubs, declared) {
        Some(remote) => Ok(Some(hub_ref_blocks(root, &remote)?)),
        None => Ok(None),
    }
}

/// The hub context the lineage view's single-hub field carries: the first
/// declared hub this clone reaches, with its claims. Blocks stamped for any
/// other reached hub resolve through [`hub_remotes`].
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
fn single_hub_blocks(root: &Path) -> Result<Option<(String, Vec<crate::node::AgreedIdBlock>)>> {
    if crate::allocation_lineage::RootRecord::load(root)?.is_some() {
        return Ok(None);
    }
    let hubs = crate::store_hubs::committed(root)?;
    let Some((hub, remote)) = hubs.hubs.iter().find_map(|h| {
        crate::store_hubs::remote_reaching(root, &hubs, h).map(|r| (h.name.clone(), r))
    }) else {
        return Ok(None);
    };
    Ok(Some((hub, hub_ref_blocks(root, &remote)?)))
}

/// The block claims `remote`'s store ref records, as of this clone's
/// remote-tracking copy (`refs/remotes/<remote>/<store branch>`). Empty when
/// that ref or its `registry/blocks.yaml` does not exist (nothing is on the
/// hub yet).
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
// remote-creds-ok(TASK-2001): reads the local remote-tracking ref only; no network
fn hub_ref_blocks(root: &Path, remote: &str) -> Result<Vec<crate::node::AgreedIdBlock>> {
    let branch = crate::git_ops::current_branch(root).unwrap_or_else(|_| "aida-store".to_string());
    let out = crate::git_ops::host_git_command(root)
        .args([
            "show",
            &format!("refs/remotes/{remote}/{branch}:registry/blocks.yaml"),
        ])
        .output()?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    let registry: crate::node::BlockRegistry =
        serde_yaml::from_slice(&out.stdout).unwrap_or_default();
    Ok(registry.blocks)
}

/// The lineage inputs for this (non-root or root) clone, read from the store
/// and the clone-local setting.
fn with_lineage_view<T>(
    root: &Path,
    node: &str,
    mode: &AllocationMode,
    f: impl FnOnce(&crate::allocation_lineage::LineageView<'_>) -> T,
) -> Result<T> {
    let blocks = crate::node::BlockRegistry::load(&root.join("registry/blocks.yaml"))?;
    with_lineage_view_of(root, node, mode, &blocks.blocks, f)
}

/// Run `f` against this clone's lineage view (store records, clone-local
/// setting, and in single-hub mode the hub's claims), as `aida node show`
/// reports it.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
pub fn with_clone_lineage_view<T>(
    root: &Path,
    node: &str,
    f: impl FnOnce(&crate::allocation_lineage::LineageView<'_>) -> T,
) -> Result<T> {
    let mode = resolve(root)?;
    with_lineage_view(root, node, &mode, f)
}

/// [`with_lineage_view`] over a block list the caller already holds.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
fn with_lineage_view_of<T>(
    root: &Path,
    node: &str,
    mode: &AllocationMode,
    blocks: &[crate::node::AgreedIdBlock],
    f: impl FnOnce(&crate::allocation_lineage::LineageView<'_>) -> T,
) -> Result<T> {
    use crate::allocation_lineage::{HubSpend, LineageView, PoolRegistry, RootRecord};
    let pools = PoolRegistry::load(root)?;
    let root_record = RootRecord::load(root)?;
    let hub = single_hub_blocks(root)?;
    let hub_remotes = hub_remotes(root)?;
    let view = LineageView {
        pools: &pools.pools,
        blocks,
        root: root_record.as_ref(),
        node,
        role: mode.role,
        upstream: mode.upstream.as_deref(),
        hub: hub.as_ref().map(|(hub, on_hub)| HubSpend { hub, on_hub }),
        hub_remotes: &hub_remotes,
    };
    Ok(f(&view))
}

/// Each declared hub this clone reaches, with the remote it reaches it as
/// and the claims on that remote's store ref: promoted pools are checked
/// against them once a root is recorded, and before that each hub-stamped
/// block is checked against its own stamped hub's claims.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
fn hub_remotes(root: &Path) -> Result<Vec<crate::allocation_lineage::HubReach>> {
    let hubs = crate::store_hubs::committed(root)?;
    let mut out = Vec::new();
    for h in &hubs.hubs {
        let Some(remote) = crate::store_hubs::remote_reaching(root, &hubs, h) else {
            continue;
        };
        let on_hub = hub_ref_blocks(root, &remote)?;
        out.push(crate::allocation_lineage::HubReach {
            hub: h.name.clone(),
            remote,
            on_hub,
        });
    }
    Ok(out)
}

/// Why `block` may not be spent in single-hub mode although the dispenser
/// found it active: it carries the hub's pool id but the identical claim is
/// not on the hub's store ref (committed locally and never pushed, or only
/// present in an imported store). `None` outside single-hub mode, for a
/// block that is not hub-stamped, and for a claim that is on the hub. This
/// is the check an authoritative (legacy root) clone's dispense applies;
/// a non-root clone gets it through [`delegated_block_in`].
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
pub fn unpushed_hub_claim(
    root: &Path,
    block: &crate::node::AgreedIdBlock,
) -> Result<Option<crate::allocation_lineage::Unusable>> {
    let Some((name, typ)) = block
        .pool_id
        .as_deref()
        .and_then(crate::store_hubs::parse_hub_pool_id)
    else {
        return Ok(None);
    };
    if !typ.eq_ignore_ascii_case(&block.type_prefix) {
        return Ok(None);
    }
    if crate::allocation_lineage::RootRecord::load(root)?.is_some() {
        return Ok(None);
    }
    // Checked against the block's own stamped hub. A stamped hub this clone
    // does not reach (no remote whose every fetch and push URL is the hub's)
    // cannot vouch for the claim, so the block is not spent.
    let on_hub = hub_claims(root, name)?.unwrap_or_default();
    if on_hub
        .iter()
        .any(|b| crate::allocation_lineage::same_claim(b, block))
    {
        return Ok(None);
    }
    Ok(Some(crate::allocation_lineage::Unusable::NotOnHub {
        type_prefix: block.type_prefix.to_ascii_uppercase(),
        hub: name.to_string(),
    }))
}

/// Index (in `registry/blocks.yaml`) of the delegated `type_prefix` block a
/// non-root clone may dispense agreed IDs from, per
/// [`LineageView::spendable_block`](crate::allocation_lineage::LineageView::spendable_block).
/// `None` on the root (it dispenses from its own blocks) and whenever no
/// block is spendable.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
pub fn delegated_block(root: &Path, type_prefix: &str) -> Result<Option<usize>> {
    let blocks = crate::node::BlockRegistry::load(&root.join("registry/blocks.yaml"))?;
    delegated_block_in(root, type_prefix, &blocks.blocks)
}

/// [`delegated_block`] against a registry the caller already holds, so a
/// dispenser that reloaded `blocks.yaml` under its locks indexes the very
/// copy it will write (a pull or claim between an outside lookup and the
/// lock could otherwise reorder the blocks under a stale index).
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
pub fn delegated_block_in(
    root: &Path,
    type_prefix: &str,
    blocks: &[crate::node::AgreedIdBlock],
) -> Result<Option<usize>> {
    let mode = resolve(root)?;
    if mode.authoritative {
        return Ok(None);
    }
    let Ok(node) = node(root) else {
        return Ok(None);
    };
    with_lineage_view_of(root, &node, &mode, blocks, |view| {
        view.spendable_block(type_prefix).ok()
    })
}

/// Whether `agreed_id` lies inside a delegated block this non-root clone may
/// spend (same type, within the block's range, below its `next`). The one
/// way a non-root clone may write a bare agreed ID.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
fn dispensed_from_delegated_block(root: &Path, node: &str, agreed_id: &str) -> Result<bool> {
    let Some((prefix, seq)) = agreed_id.trim().rsplit_once('-') else {
        return Ok(false);
    };
    let Ok(seq) = seq.parse::<u32>() else {
        return Ok(false);
    };
    if prefix.contains('-') || !prefix.chars().all(|c| c.is_ascii_alphabetic()) {
        return Ok(false);
    }
    let mode = resolve(root)?;
    with_lineage_view(root, node, &mode, |view| {
        view.spendable_block(prefix).is_ok_and(|i| {
            let b = &view.blocks[i];
            b.range_start <= seq && seq < b.next
        })
    })
}

/// Why a non-root clone issues node-local `type_prefix` IDs although
/// blocks are configured, read from its upstream setting and the store's
/// lineage records. `None` on the root, which dispenses as before, and on a
/// clone holding a spendable delegated block (it dispenses from it).
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
pub fn downgrade_reason(root: &Path, type_prefix: &str) -> Result<Option<String>> {
    let mode = resolve(root)?;
    if mode.authoritative {
        return Ok(None);
    }
    let node = match node(root) {
        Ok(node) => node,
        Err(e) => return Ok(Some(format!("{e:#}"))),
    };
    with_lineage_view(root, &node, &mode, |view| {
        view.spendable_block(type_prefix)
            .err()
            .map(|reason| reason.to_string())
    })
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
    refuse_retired_node(root, &node)?;
    Ok(node)
}

/// This clone's own node id, for labels and identity display: the WORK
/// binding when there is one, else the store's legacy `.aida/node.toml`.
/// `None` on a clone with neither. Unlike [`node`] it does not refuse a
/// retired legacy node, so a caller can still name it. `aida node show`
/// reads it; lease stamps (`coordination::node_id_for_clone`) apply the same
/// WORK-binding-first rule before their per-clone `nodes.toml` lookup.
// trace:BUG-2378 | ai:claude
pub fn clone_node_id(root: &Path) -> Result<Option<String>> {
    if let Some(binding) = crate::work_binding::load(root)? {
        return Ok(Some(binding.node_id));
    }
    let legacy = root.join(".aida/node.toml");
    if !legacy.try_exists()? {
        return Ok(None);
    }
    Ok(Some(crate::NodeConfig::load(&legacy)?.node_id))
}

/// Every node id the shared store registers, lower-cased: a
/// `registry/nodes.toml` row, a node a `registry/node_aliases.toml` label
/// binds (the native nodes a HOME store names by label), and the
/// allocation root `registry/root.yaml` records. A root enrolled and given
/// blocks by the agreed-ID backfill is registered by its label and root
/// record and never gets a `nodes.toml` row. `aida db block verify` and
/// `aida node show` judge "registered" by it. `aida node acquire`'s taken
/// probe is stricter on purpose (it also refuses ids a label merely names),
/// and verify's "registered node without a block" check stays on
/// `nodes.toml` rows, the only registrations that are expected to own blocks.
/// Fails closed on a malformed `registry/node_aliases.toml`: a gate must not
/// treat an unreadable registry as "no labels".
// trace:BUG-2378 | ai:claude
pub fn registered_node_ids(root: &Path) -> Result<std::collections::BTreeSet<String>> {
    let mut ids = std::collections::BTreeSet::new();
    let nodes = root.join("registry/nodes.toml");
    if nodes.try_exists()? {
        for n in crate::node::NodeRegistry::load(&nodes)?.nodes {
            ids.insert(n.id.to_ascii_lowercase());
        }
    }
    for b in crate::node_alias::NodeAliasMap::load_strict(root)?.bindings {
        ids.insert(
            crate::node_alias::normalize_node(&b.node).unwrap_or_else(|_| b.node.to_lowercase()),
        );
    }
    if let Some(r) = crate::allocation_lineage::RootRecord::load(root)? {
        ids.insert(
            crate::node_alias::normalize_node(&r.holder_node)
                .unwrap_or_else(|_| r.holder_node.to_lowercase()),
        );
    }
    Ok(ids)
}

/// Refuse when `registry/nodes.toml` marks `node` retired: its specs were
/// renamed into another node's local IDs, so it must never allocate again.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
pub fn refuse_retired_node(root: &Path, node: &str) -> Result<()> {
    let path = root.join("registry/nodes.toml");
    if !path.try_exists()? {
        return Ok(());
    }
    if crate::node::NodeRegistry::load(&path)?.is_retired(node) {
        bail!(
            "node {node} is retired (registry/nodes.toml); it allocates no IDs and claims no blocks. Bind this clone to a live node with `aida node acquire`"
        )
    }
    Ok(())
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
    admit_with_binding(
        incoming,
        existing,
        || node(root),
        |node, id| dispensed_from_delegated_block(root, node, id),
    )
}

// Exactly the same ownership and local-ID admission for an unspent candidate.
// trace:STORY-1742 | ai:codex
// trace:BUG-2017 | ai:codex
pub(crate) fn admit_candidate(
    node: &str,
    incoming: &[crate::Requirement],
    existing: &[crate::Requirement],
) -> Result<()> {
    admit_with_binding(incoming, existing, || Ok(node.to_owned()), |_, _| Ok(false))
}

fn admit_with_binding(
    incoming: &[crate::Requirement],
    existing: &[crate::Requirement],
    mut resolve_node: impl FnMut() -> Result<String>,
    // Whether a new agreed ID was dispensed from a delegated block this
    // node may spend. trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
    mut delegated: impl FnMut(&str, &str) -> Result<bool>,
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
        for (agreed_slot, id, previous) in [
            (
                false,
                req.spec_id.as_deref(),
                prior.and_then(|r| r.spec_id.as_deref()),
            ),
            (
                true,
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
                let local = local_id(id, binding);
                if crate::object_store::valid_spec_id_format(id)
                    && !local
                    && agreed_slot
                    && delegated(binding, id)?
                {
                    continue;
                }
                if !crate::object_store::valid_spec_id_format(id) || !local {
                    bail!("new or changed global display IDs refuse on a non-authoritative clone; nothing was written")
                }
            } else if previous.is_none() && req.spec_id.is_none() && req.agreed_id.is_none() {
                // A row carrying only a delegated agreed ID gets its local
                // spec ID from the bound dispenser after admission.
                // trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
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
            // A bound root mints its local IDs from its bound node counter
            // (the agreed ID comes from a block); an unbound root keeps the
            // configured provider. trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
            if let Some(inner) = &self.inner {
                return Ok(inner.clone());
            }
            if crate::work_binding::load(&self.root)?.is_none() {
                bail!("authoritative allocator has no configured dispenser")
            }
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

    /// The bound counter, read without its lock, when this process cannot
    /// write the directory that holds it: a contained session, whose sandbox
    /// denies the main checkout's `.git` but grants the store worktree.
    /// `None` when the normal locked provider applies.
    ///
    /// Scope: a node-bound, non-authoritative clone. An authoritative clone
    /// and an unbound legacy clone keep the locked provider, so a contained
    /// add on one still fails on the read-only counter; contained seats run
    /// on bound node-local clones.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-053 | ai:claude
    fn contained_state(&self) -> Result<Option<crate::dispenser::DispenserState>> {
        if self.inner.is_some()
            || resolve(&self.root)?.authoritative
            || crate::work_binding::load(&self.root)?.is_none()
        {
            return Ok(None);
        }
        let path = crate::work_binding::counter_path(&self.root)?;
        if path.parent().is_none_or(dir_writable) {
            return Ok(None);
        }
        #[cfg(unix)]
        crate::transport_policy::refuse_unsupported(&self.root)?;
        let state = crate::work_binding::bound_counter_state(&path)?
            .context("bound WORK counter missing; explicit recovery required")?;
        let binding = node(&self.root)?;
        if state.mode != (crate::dispenser::IdMode::Distributed { node_id: binding }) {
            bail!(
                "dispenser does not match this clone's node binding; run `{}` for the repair",
                crate::work_binding::DOCTOR_ALLOCATION_COMMAND
            )
        }
        Ok(Some(state))
    }

    /// The highest sequence this node has filed under `prefix` in the store
    /// worktree, committed or still queued for the host (0 when none). A
    /// contained mint lands here before any counter knows about it.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-053 | ai:claude
    fn filed_high_water(&self, prefix: &str) -> Result<u32> {
        let node = node(&self.root)?;
        let Some(local) =
            crate::id_identity::IdKey::parse(&format!("{prefix}-{node}-1")).map(|k| k.prefix)
        else {
            return Ok(0);
        };
        Ok(
            crate::object_store::list_objects(&self.root.join("objects"))?
                .iter()
                .filter_map(|(stem, _)| crate::id_identity::IdKey::parse(stem))
                .filter(|k| k.prefix == local)
                .filter_map(|k| k.seq.parse::<u32>().ok())
                .max()
                .unwrap_or(0),
        )
    }

    /// A contained session's next number: past both the read-only counter
    /// and every number already filed in the store. Only under the store
    /// write lock, held until the new object is written, so no other seat
    /// and no host writer can pick the same number in between.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-053 | ai:claude
    fn contained_next(
        &self,
        state: &crate::dispenser::DispenserState,
        prefix: &str,
    ) -> Result<u32> {
        if !crate::db::store_write_lock_held(&self.root) {
            bail!(
                "this session cannot write the ID counter in {}, and a new ID can only be issued while holding the store write lock",
                crate::work_binding::counter_path(&self.root)?
                    .parent()
                    .unwrap_or(&self.root)
                    .display()
            )
        }
        self.contained_peek(state, prefix)
    }

    fn contained_peek(
        &self,
        state: &crate::dispenser::DispenserState,
        prefix: &str,
    ) -> Result<u32> {
        let counter = state
            .sequences
            .get(&prefix.to_uppercase())
            .copied()
            .unwrap_or(0);
        counter
            .max(self.filed_high_water(prefix)?)
            .checked_add(1)
            .context("node-local sequence exhausted")
    }
}

/// Whether this process may create files in `dir`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-053 | ai:claude
fn dir_writable(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `c` is a valid NUL-terminated path for the call's duration.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        true
    }
}

/// Whether a new ID minted at `root` is issued without the clone's counter
/// lock (a contained session), so the store write must be queued for the
/// host to commit. Test-only: production decides through the provider
/// itself, and this swallows a resolution error into `false`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-053 | ai:claude
#[cfg(test)]
fn contained_allocation(root: &Path) -> bool {
    CloneDispenser {
        root: root.into(),
        inner: None,
        migrate_registered_legacy: false,
    }
    .contained_state()
    .is_ok_and(|s| s.is_some())
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
        self.state()?;
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
    // trace:BUG-B6B1278890774EF993D2EBC327862470-053 | ai:claude — a contained
    // session mints past the read-only counter and the filed high-water; the
    // host's counter then jumps past what contained sessions filed.
    fn next(&self, prefix: &str) -> Result<u32> {
        if let Some(state) = self.contained_state()? {
            return self.contained_next(&state, prefix);
        }
        let inner = self.inner()?;
        inner.next_above(prefix, self.filed_high_water(prefix)?)
    }
    fn peek(&self, prefix: &str) -> Result<u32> {
        if let Some(state) = self.contained_state()? {
            return self.contained_peek(&state, prefix);
        }
        // Preview what `next` will issue: past the counter and the filed
        // high-water alike. trace:BUG-B6B1278890774EF993D2EBC327862470-053 | ai:claude
        let filed_next = self
            .filed_high_water(prefix)?
            .checked_add(1)
            .context("node-local sequence exhausted")?;
        Ok(self.inner()?.peek(prefix)?.max(filed_next))
    }
    fn state(&self) -> Result<crate::dispenser::DispenserState> {
        if let Some(state) = self.contained_state()? {
            return Ok(state);
        }
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

    // trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
    // trace:STORY-B6B1278890774EF993D2EBC327862470-105.ac5e9c08 | ai:claude
    #[test]
    fn story_105_root_setting_refuses_when_root_record_names_another_node() {
        let (dir, _) = fixture(true);
        let policy = crate::IdFormatPolicy::BlocksThenFallback;
        // No root.yaml yet: nothing to contradict the local setting.
        assert_eq!(effective_policy(dir.path(), policy).unwrap(), policy);
        std::fs::create_dir_all(dir.path().join("registry")).unwrap();
        let record = dir.path().join(crate::allocation_lineage::ROOT_YAML_REL);
        std::fs::write(&record, "holder_node: '9'\nepoch: 1\n").unwrap();
        let err = effective_policy(dir.path(), policy)
            .unwrap_err()
            .to_string();
        assert!(err.contains("records node 9"), "{err}");
        assert!(err.contains("this clone is node 7"), "{err}");
        std::fs::write(&record, "holder_node: '7'\nepoch: 1\n").unwrap();
        assert_eq!(effective_policy(dir.path(), policy).unwrap(), policy);
    }

    /// A non-root node 7 with upstream `gitlab` holding a BUG block carved
    /// from the root's pool on that upstream.
    fn delegated_fixture() -> (tempfile::TempDir, GitBackend) {
        let (dir, backend) = fixture(false);
        write_mode(dir.path(), AllocationRole::Node, Some("gitlab")).unwrap();
        let registry = dir.path().join("registry");
        std::fs::create_dir_all(&registry).unwrap();
        std::fs::write(registry.join("root.yaml"), "holder_node: R\nepoch: 1\n").unwrap();
        std::fs::write(
            registry.join("pools.yaml"),
            "pools:\n- pool_id: p-bug\n  type_prefix: BUG\n  range_start: 1\n  range_end: 1000\n  holder_node: R\n  home_ref: gitlab/aida-store\n",
        )
        .unwrap();
        std::fs::write(
            registry.join("blocks.yaml"),
            "blocks:\n- node_id: '7'\n  owner: o\n  hostname: h\n  type_prefix: BUG\n  range_start: 101\n  range_end: 200\n  next: 102\n  allocated_at: 2026-10-31T00:00:00Z\n  pool_id: p-bug\n",
        )
        .unwrap();
        (dir, backend)
    }

    // trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
    #[test]
    fn story_105_non_root_dispenses_from_a_spendable_delegated_block() {
        let (dir, _) = delegated_fixture();
        let policy = crate::IdFormatPolicy::BlocksThenFallback;
        assert_eq!(delegated_block(dir.path(), "BUG").unwrap(), Some(0));
        assert_eq!(delegated_block(dir.path(), "TASK").unwrap(), None);
        assert_eq!(
            effective_policy_for(dir.path(), policy, Some("BUG")).unwrap(),
            policy
        );
        assert_eq!(
            effective_policy_for(dir.path(), policy, Some("TASK")).unwrap(),
            crate::IdFormatPolicy::NodeAwareOnly
        );
        assert_eq!(downgrade_reason(dir.path(), "BUG").unwrap(), None);
        assert!(downgrade_reason(dir.path(), "TASK").unwrap().is_some());

        // An agreed ID the block has dispensed (below `next`) is admitted
        // beside a local ID; one it has not dispensed yet is refused.
        let mut req = Requirement::new("x".into(), "fixture".into());
        req.spec_id = Some("BUG-7-001".into());
        req.agreed_id = Some("BUG-101".into());
        admit(dir.path(), std::slice::from_ref(&req), &[]).unwrap();
        req.agreed_id = Some("BUG-150".into());
        assert!(admit(dir.path(), std::slice::from_ref(&req), &[]).is_err());
        // An agreed ID alone (the local ID is minted after admission).
        req.spec_id = None;
        req.agreed_id = Some("BUG-101".into());
        admit(dir.path(), std::slice::from_ref(&req), &[]).unwrap();

        // Saving an advanced `next` is allowed; re-ranging is not.
        let path = dir.path().join("registry/blocks.yaml");
        let mut reg = crate::node::BlockRegistry::load(&path).unwrap();
        reg.blocks[0].next = 103;
        reg.save_dispensed(&path, "7").unwrap();
        assert_eq!(
            crate::node::BlockRegistry::load(&path).unwrap().blocks[0].next,
            103
        );
        reg.blocks[0].range_end = 900;
        assert!(reg.save_dispensed(&path, "7").is_err());
        assert!(reg.save(&path).is_err(), "plain save stays root-only");
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
                    // A batch-internal alias conflict may refuse at the
                    // second add() or at finish(); either way it must be
                    // BUG-2017's refusal and nothing may persist.
                    "bulk" => {
                        let mut writer = backend.bulk_writer().unwrap();
                        writer.add(first).unwrap();
                        writer
                            .add(second)
                            .map(|_| ())
                            .and_then(|()| writer.finish("must not persist").map(|_| ()))
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

    // trace:BUG-B6B1278890774EF993D2EBC327862470-053 | ai:claude — a contained
    // session (the clone's `.git` read-only, the store worktree writable)
    // mints final IDs under the store write lock without touching the counter.
    #[cfg(unix)]
    mod contained {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        struct ReadOnlyGit(PathBuf);
        impl ReadOnlyGit {
            fn new(root: &Path) -> Self {
                let git = root.join(".git");
                std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o555)).unwrap();
                ReadOnlyGit(git)
            }
        }
        impl Drop for ReadOnlyGit {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            }
        }

        fn bound_clone() -> Option<(tempfile::TempDir, String)> {
            if unsafe { libc::geteuid() } == 0 {
                eprintln!("skipping: root ignores directory write bits");
                return None;
            }
            let dir = tempfile::tempdir().unwrap();
            crate::git_ops::init(dir.path()).unwrap();
            let node = crate::work_binding::bootstrap(dir.path())
                .unwrap()
                .unwrap()
                .node_id;
            // The host installs the store hooks once; a contained session
            // only verifies them.
            crate::git_ops::resolve_store_hooks_dir(dir.path()).unwrap();
            Some((dir, node))
        }

        fn add_bug(root: &Path, title: &str) -> String {
            let backend = GitBackend::new(root).unwrap().with_auto_commit(false);
            let mut req = Requirement::new(title.into(), "fixture".into());
            req.req_type = crate::RequirementType::Bug;
            backend.add_requirement(req).unwrap().spec_id.unwrap()
        }

        #[test]
        fn contained_add_mints_past_counter_and_filed_ids_and_host_jumps_past_them() {
            let Some((dir, node)) = bound_clone() else {
                return;
            };
            let root = dir.path();
            assert_eq!(add_bug(root, "host first"), format!("BUG-{node}-001"));
            let counter = crate::work_binding::counter_path(root).unwrap();
            let before = std::fs::read(&counter).unwrap();
            {
                let _ro = ReadOnlyGit::new(root);
                assert!(contained_allocation(root));
                assert_eq!(add_bug(root, "contained one"), format!("BUG-{node}-002"));
                assert_eq!(add_bug(root, "contained two"), format!("BUG-{node}-003"));
                // Readable in the same session before any host commit.
                let backend = GitBackend::new(root).unwrap();
                let store = backend.load().unwrap();
                assert!(store
                    .requirements
                    .iter()
                    .any(|r| r.spec_id.as_deref() == Some(&format!("BUG-{node}-003"))));
            }
            assert_eq!(
                std::fs::read(&counter).unwrap(),
                before,
                "counter untouched"
            );
            assert!(!contained_allocation(root));
            // The host counter still says 1; it jumps past what was filed,
            // and its preview agrees with what it will issue.
            assert_eq!(dispenser(root, None).unwrap().peek("BUG").unwrap(), 4);
            assert_eq!(add_bug(root, "host second"), format!("BUG-{node}-004"));
            assert_eq!(add_bug(root, "host third"), format!("BUG-{node}-005"));
        }

        #[test]
        fn concurrent_contained_seats_never_share_an_id() {
            let Some((dir, node)) = bound_clone() else {
                return;
            };
            let root = dir.path().to_path_buf();
            let _ro = ReadOnlyGit::new(&root);
            // Two seats racing three adds each interleave on the store lock;
            // every add rescans objects/, so keep the fixture small.
            let seats: Vec<_> = (0..2)
                .map(|seat| {
                    let root = root.clone();
                    std::thread::spawn(move || {
                        (0..3)
                            .map(|n| add_bug(&root, &format!("seat {seat} spec {n}")))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            let mut ids: Vec<String> = seats.into_iter().flat_map(|s| s.join().unwrap()).collect();
            ids.sort();
            let expected: Vec<String> = (1..=6).map(|n| format!("BUG-{node}-{n:03}")).collect();
            assert_eq!(ids, expected, "distinct, gap-free IDs");
        }

        #[test]
        fn contained_mint_refuses_outside_the_store_write_lock() {
            let Some((dir, _node)) = bound_clone() else {
                return;
            };
            let _ro = ReadOnlyGit::new(dir.path());
            let provider = dispenser(dir.path(), None).unwrap();
            let err = provider.next("BUG").unwrap_err();
            assert!(format!("{err:#}").contains("store write lock"), "{err:#}");
            // Peeking needs no lock and advances nothing.
            assert_eq!(provider.peek("BUG").unwrap(), 1);
        }
    }
}

#[cfg(test)]
#[path = "tests/claim_authority_tests.rs"]
mod claim_authority_tests;
