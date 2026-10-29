//! Friendly node aliases for node-local spec IDs (STORY-b6b1…-078, slice 1 of
//! EPIC-b6b1…-001).
//!
//! A clone that is not an allocation authority files node-local IDs such as
//! `TASK-B6B1278890774EF993D2EBC327862470-095`. This module binds a short
//! operator-chosen LABEL to a node UUID (`HOME -> b6b12788…`) so that:
//!
//! - **input** accepts `TASK-HOME-095` anywhere a spec ID is taken and expands
//!   it to the canonical form before lookup ([`expand_spec_id`]);
//! - **output** on human/TOON surfaces shows `TASK-HOME-095` where the
//!   canonical node-local ID would appear ([`display_spec_id`]).
//!
//! It is a PRESENTATION and INPUT layer only. Nothing AIDA writes (store
//! objects, relationships, generated traces or trailers) ever carries an
//! alias, so changing or losing this map cannot corrupt stored data.
//!
//! The map is a shared record on the orphan `aida-store` branch,
//! `registry/node_aliases.toml`, next to (but separate from) the person-alias
//! map (`registry/aliases.toml`, TASK-845). It is append-only: a label is
//! never pointed at a different node, and a rename RETIRES the old label (it
//! keeps resolving to the same node for input) instead of deleting it.
//
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};

/// `registry/node_aliases.toml` relative to the store worktree root.
pub const NODE_ALIASES_TOML_REL: &str = "registry/node_aliases.toml";

/// Prefixes a label may never equal: every requirement type prefix plus the
/// forge/allocation prefixes that already have a meaning in an ID's first
/// segment.
const RESERVED_LABELS: &[&str] = &["SPEC", "WORK", "PR", "MR", "GH", "GL"];

/// One label → node binding. `retired` bindings keep resolving for input but
/// are never displayed and can never be rebound.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeAliasBinding {
    /// Upper-case label, `^[A-Z][A-Z0-9]{1,7}$`.
    pub label: String,
    /// Lower-case 32-hex node UUID.
    pub node: String,
    /// Retired by `aida node alias rename`; still resolves for input.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retired: bool,
}

/// The shared node-alias map (`registry/node_aliases.toml`).
///
/// ```toml
/// [[binding]]
/// label = "HOME"
/// node = "b6b1278890774ef993d2ebc327862470"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeAliasMap {
    #[serde(default, rename = "binding")]
    pub bindings: Vec<NodeAliasBinding>,
}

/// Validate and normalize a label: upper-cased, `^[A-Z][A-Z0-9]{1,7}$`, not
/// all hex digits (it would read as a node fragment), and not a requirement
/// type prefix. `@` and `.` fail the character class, so a hostname or email
/// can never become a label.
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
pub fn validate_label(raw: &str) -> Result<String, String> {
    let label = raw.trim().to_ascii_uppercase();
    let mut chars = label.chars();
    let shape_ok = chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && (2..=8).contains(&label.len())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    if !shape_ok {
        return Err(format!(
            "invalid node label '{raw}': use 2-8 letters or digits, starting with a letter"
        ));
    }
    if label.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "invalid node label '{raw}': a label made only of hex digits reads as part of a node id"
        ));
    }
    let is_type_prefix = crate::RequirementType::ALL
        .iter()
        .any(|t| t.default_prefix() == label);
    if is_type_prefix || RESERVED_LABELS.contains(&label.as_str()) {
        return Err(format!(
            "invalid node label '{raw}': it is already a requirement type prefix"
        ));
    }
    Ok(label)
}

/// Normalize a node UUID (32 hex, or hyphenated) to lower-case 32 hex.
pub fn normalize_node(raw: &str) -> Result<String, String> {
    let node: String = raw
        .trim()
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_ascii_lowercase();
    if node.len() == 32 && node.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(node)
    } else {
        Err(format!("invalid node id '{raw}': expected a node UUID"))
    }
}

/// Split `PREFIX-MIDDLE-SEQ` into its three parts when PREFIX is a type-like
/// token (letter first, alphanumeric, optional leading `$`) and SEQ is digits.
fn split_three(id: &str) -> Option<(&str, &str, &str)> {
    let mut parts = id.trim().splitn(3, '-');
    let prefix = parts.next()?;
    let middle = parts.next()?;
    let seq = parts.next()?;
    let bare = prefix.strip_prefix('$').unwrap_or(prefix);
    let prefix_ok = bare.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && bare.chars().all(|c| c.is_ascii_alphanumeric());
    let seq_ok = !seq.is_empty() && seq.chars().all(|c| c.is_ascii_digit());
    (prefix_ok && seq_ok && !middle.is_empty()).then_some((prefix, middle, seq))
}

impl NodeAliasMap {
    fn path(store_root: &Path) -> PathBuf {
        store_root.join(NODE_ALIASES_TOML_REL)
    }

    /// Best-effort load: a missing, unreadable or malformed file is an empty
    /// map (display falls back to canonical; alias input is then not found).
    #[cfg(feature = "native")]
    pub fn load(store_root: &Path) -> Self {
        Self::load_strict(store_root).unwrap_or_default()
    }

    /// Load for a WRITE: a malformed file is an error, so a write never
    /// overwrites bindings it could not read.
    #[cfg(feature = "native")]
    pub fn load_strict(store_root: &Path) -> anyhow::Result<Self> {
        let path = Self::path(store_root);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        toml::from_str(&content).map_err(|e| {
            anyhow::anyhow!(
                "{} is malformed ({e}); fix it by hand before changing node labels",
                path.display()
            )
        })
    }

    /// Write the map to the store worktree. The caller commits and pushes.
    #[cfg(feature = "native")]
    pub fn save(&self, store_root: &Path) -> anyhow::Result<()> {
        let path = Self::path(store_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    fn binding(&self, label: &str) -> Option<&NodeAliasBinding> {
        self.bindings
            .iter()
            .find(|b| b.label.eq_ignore_ascii_case(label))
    }

    /// The node a label (current or retired) names.
    pub fn node_for_label(&self, label: &str) -> Option<&str> {
        self.binding(label).map(|b| b.node.as_str())
    }

    /// A node's current (display) label.
    pub fn current_label(&self, node: &str) -> Option<&str> {
        self.bindings
            .iter()
            .find(|b| !b.retired && b.node.eq_ignore_ascii_case(node))
            .map(|b| b.label.as_str())
    }

    /// Bind `label` to `node`. Returns whether the map changed (re-binding a
    /// node to its own current label is a no-op). Refuses when the label
    /// names (or once named) another node, or when the node already has a
    /// different current label (that is a rename).
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    pub fn set(&mut self, label: &str, node: &str) -> Result<bool, String> {
        let label = validate_label(label)?;
        let node = normalize_node(node)?;
        if let Some(existing) = self.binding(&label) {
            if existing.node != node {
                return Err(format!(
                    "label {label} already names node {}{}; refusing to bind it to node {node}",
                    existing.node,
                    if existing.retired { " (retired)" } else { "" }
                ));
            }
            if existing.retired {
                return Err(format!(
                    "label {label} is a retired label of node {node}; use `aida node alias rename {} {label}` to make it current again",
                    self.current_label(&node).unwrap_or("<current>")
                ));
            }
            return Ok(false);
        }
        if let Some(current) = self.current_label(&node) {
            return Err(format!(
                "node {node} is already labelled {current}; use `aida node alias rename {current} {label}`"
            ));
        }
        self.bindings.push(NodeAliasBinding {
            label,
            node,
            retired: false,
        });
        Ok(true)
    }

    /// Make `new` the current label of the node `old` currently names, and
    /// retire `old` (it keeps resolving for input). `new` may be fresh or a
    /// retired label of the SAME node; a label of any other node refuses.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    pub fn rename(&mut self, old: &str, new: &str) -> Result<bool, String> {
        let old = validate_label(old)?;
        let new = validate_label(new)?;
        let node = match self.binding(&old) {
            Some(b) if !b.retired => b.node.clone(),
            Some(b) => {
                return Err(format!(
                    "label {old} is retired; node {} is currently labelled {}",
                    b.node,
                    self.current_label(&b.node).unwrap_or("(none)")
                ))
            }
            None => return Err(format!("unknown node label {old}")),
        };
        if old == new {
            return Ok(false);
        }
        if let Some(existing) = self.binding(&new) {
            if existing.node != node {
                return Err(format!(
                    "label {new} already names node {}{}; refusing to bind it to node {node}",
                    existing.node,
                    if existing.retired { " (retired)" } else { "" }
                ));
            }
        }
        for b in &mut self.bindings {
            if b.label == old {
                b.retired = true;
            } else if b.label == new {
                b.retired = false;
            }
        }
        if self.binding(&new).is_none() {
            self.bindings.push(NodeAliasBinding {
                label: new,
                node,
                retired: false,
            });
        }
        Ok(true)
    }

    /// Expand `TYPE-<LABEL>-NNN` (any case) to the canonical
    /// `TYPE-<NODE>-NNN` (upper-case, matching stored filenames). `None` when
    /// `id` is not alias-shaped or names an unknown label — the caller then
    /// looks up `id` as given, so an unknown label stays a not-found.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    pub fn expand(&self, id: &str) -> Option<String> {
        let (prefix, middle, seq) = split_three(id)?;
        let node = self.node_for_label(middle)?;
        Some(format!("{prefix}-{node}-{seq}").to_ascii_uppercase())
    }

    /// Display form of `id`: a node-local ID whose node has a current label
    /// becomes `TYPE-<LABEL>-NNN`; any other node-local ID is upper-cased
    /// (one case on every surface); everything else is returned unchanged.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    pub fn display(&self, id: &str, canonical: bool) -> String {
        let Some((prefix, middle, seq)) = split_three(id) else {
            return id.to_string();
        };
        let Ok(node) = normalize_node(middle) else {
            return id.to_string();
        };
        match self.current_label(&node).filter(|_| !canonical) {
            Some(label) => format!("{}-{label}-{seq}", prefix.to_ascii_uppercase()),
            None => id.trim().to_ascii_uppercase(),
        }
    }
}

// ---------------------------------------------------------------------------
// Process-wide installed map: the CLI and MCP server install the store's map
// once per process so every lookup and formatter shares it without threading
// a parameter through hundreds of call sites.
// ---------------------------------------------------------------------------

fn slot() -> &'static RwLock<Arc<NodeAliasMap>> {
    static SLOT: OnceLock<RwLock<Arc<NodeAliasMap>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(Arc::new(NodeAliasMap::default())))
}

static CANONICAL_OVERRIDE: AtomicBool = AtomicBool::new(false);

/// Install `map` as this process's node-alias map.
pub fn install(map: NodeAliasMap) {
    if let Ok(mut guard) = slot().write() {
        *guard = Arc::new(map);
    }
}

/// Load the store's map (best-effort) and install it.
#[cfg(feature = "native")]
pub fn install_from_store(store_root: &Path) {
    install(NodeAliasMap::load(store_root));
}

/// This process's installed map (empty until [`install`] runs).
pub fn installed() -> Arc<NodeAliasMap> {
    slot()
        .read()
        .map(|g| Arc::clone(&g))
        .unwrap_or_else(|_| Arc::new(NodeAliasMap::default()))
}

/// `--canonical-ids`: turn display substitution off for this process.
pub fn set_canonical_ids(on: bool) {
    CANONICAL_OVERRIDE.store(on, Ordering::Relaxed);
}

/// Whether display substitution is off (`--canonical-ids` or
/// `AIDA_CANONICAL_IDS=1`).
pub fn canonical_ids() -> bool {
    CANONICAL_OVERRIDE.load(Ordering::Relaxed)
        || std::env::var("AIDA_CANONICAL_IDS")
            .is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
}

/// Expand an alias-form spec ID with the installed map (see
/// [`NodeAliasMap::expand`]). Input always resolves, even under
/// `--canonical-ids`.
pub fn expand_spec_id(id: &str) -> Option<String> {
    installed().expand(id)
}

/// [`expand_spec_id`], or `id` unchanged when it is not a known alias form.
/// For parsers of hand-written text (commit trailers, `trace:` comments).
pub fn expand_or_keep(id: &str) -> String {
    expand_spec_id(id).unwrap_or_else(|| id.to_string())
}

/// Display form of a spec ID with the installed map (see
/// [`NodeAliasMap::display`]), honouring `--canonical-ids`.
pub fn display_spec_id(id: &str) -> String {
    installed().display(id, canonical_ids())
}

/// Commit-and-push a mutation of the node-alias map with the registry CAS
/// push-wins loop (mirrors [`crate::alias::link_cas`]): pull, strict-load,
/// mutate, save, commit, push; a rejected push resets and retries. Solo (no
/// `origin`) commits locally. Returns whether the map changed.
#[cfg(feature = "native")]
pub fn mutate_cas(
    store_root: &Path,
    describe: &str,
    mutate: impl Fn(&mut NodeAliasMap) -> Result<bool, String>,
) -> anyhow::Result<bool> {
    use crate::git_ops;

    const MAX_RETRIES: u32 = 10;
    let branch = git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    let local_only = !git_ops::has_remote(store_root, "origin");

    for attempt in 0..MAX_RETRIES {
        let committed = crate::db::with_store_write_lock(store_root, || {
            if attempt > 0 && !local_only {
                git_ops::pull_rebase(store_root, "origin", &branch)?;
            }
            let mut map = NodeAliasMap::load_strict(store_root)?;
            if !mutate(&mut map).map_err(anyhow::Error::msg)? {
                return Ok(None);
            }
            map.save(store_root)?;
            git_ops::add(store_root, &[NODE_ALIASES_TOML_REL])?;
            git_ops::commit(store_root, &format!("chore(registry): {describe}"))?;
            Ok(Some(git_ops::head_sha(store_root)?))
        })?;
        let Some(sha) = committed else {
            return Ok(false);
        };
        if local_only || git_ops::push_exact(store_root, "origin", &sha, &branch)? {
            return Ok(true);
        }
        let _ = git_ops::rollback_commit_if_head_matches(store_root, &sha);
    }
    anyhow::bail!(
        "could not write the node label after {MAX_RETRIES} attempts (store push kept being rejected) — run `aida db sync --pull` and retry"
    )
}

#[cfg(test)]
#[path = "node_alias_tests.rs"]
mod tests;
