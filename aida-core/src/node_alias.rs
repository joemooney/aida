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
    /// looks up `id` as given, so an unknown label stays a not-found. The
    /// sequence is re-padded the way the node-local allocator writes it
    /// (`{seq:03}`), so `BUG-HOME-36` finds `BUG-<node>-036`.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    // trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
    pub fn expand(&self, id: &str) -> Option<String> {
        let (prefix, middle, seq) = split_three(id)?;
        let node = self.node_for_label(middle)?;
        let seq = seq
            .parse::<u64>()
            .map_or_else(|_| seq.to_string(), |n| format!("{n:03}"));
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

/// Whether a spec ID exists literally in the store (`spec_id` or
/// `agreed_id`). Text parsers with no store handle of their own use it to keep
/// a literal id that happens to be label-shaped (see [`expand_or_keep`]).
pub type LiteralCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;

fn literal_slot() -> &'static RwLock<Option<LiteralCheck>> {
    static SLOT: OnceLock<RwLock<Option<LiteralCheck>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Load the store's map (best-effort) and install it, together with a
/// literal-id check over the store's objects. The check scans `objects/`
/// lazily, on the first label-shaped token a text parser meets, so commands
/// that never see one pay nothing.
#[cfg(feature = "native")]
pub fn install_from_store(store_root: &Path) {
    install(NodeAliasMap::load(store_root));
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — `[ids]`
    // widths live in the project's `.aida/config.toml`, beside the store.
    if let Some(project) = store_root.parent() {
        crate::id_identity::install_widths(crate::id_identity::IdWidths::load(project));
    }
    let objects = store_root.join("objects");
    let ids: Arc<OnceLock<std::collections::HashSet<String>>> = Arc::new(OnceLock::new());
    let check: LiteralCheck = Arc::new(move |id: &str| {
        ids.get_or_init(|| stored_spec_ids(&objects))
            .contains(&crate::id_identity::id_key(id))
    });
    if let Ok(mut guard) = literal_slot().write() {
        *guard = Some(check);
    }
}

/// Every stored object's filename stem, `spec_id` and `agreed_id`,
/// upper-cased. An unreadable object contributes only its stem.
#[cfg(feature = "native")]
fn stored_spec_ids(objects: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for (stem, path) in crate::object_store::list_objects(objects).unwrap_or_default() {
        // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — keyed
        // by numeric identity, so any padding of a stored id is literal.
        out.insert(crate::id_identity::id_key(&stem));
        if let Ok(req) = crate::object_store::read_object_from_path(&path) {
            out.extend(req.spec_id.as_deref().map(crate::id_identity::id_key));
            out.extend(req.agreed_id.as_deref().map(crate::id_identity::id_key));
        }
    }
    out
}

/// The installed literal-id check; a thread-scoped one ([`install_for_thread`])
/// wins. `None` (nothing installed) treats no id as literal.
fn installed_literal_check() -> Option<LiteralCheck> {
    if let Some(check) = THREAD_LITERAL.with(|t| t.borrow().clone()) {
        return check;
    }
    literal_slot().read().ok().and_then(|g| g.clone())
}

thread_local! {
    static THREAD_MAP: std::cell::RefCell<Option<Arc<NodeAliasMap>>> =
        const { std::cell::RefCell::new(None) };
    static THREAD_LITERAL: std::cell::RefCell<Option<Option<LiteralCheck>>> =
        const { std::cell::RefCell::new(None) };
}

/// Restores the previous thread-scoped map and literal check on drop (see
/// [`install_for_thread`]).
pub struct ThreadMapGuard(Option<Arc<NodeAliasMap>>, Option<Option<LiteralCheck>>);

impl Drop for ThreadMapGuard {
    fn drop(&mut self) {
        let prev = self.0.take();
        THREAD_MAP.with(|t| *t.borrow_mut() = prev);
        let prev_literal = self.1.take();
        THREAD_LITERAL.with(|t| *t.borrow_mut() = prev_literal);
    }
}

/// Install `map` for the CURRENT THREAD only, shadowing the process-wide map
/// until the guard drops. For tests: a long-lived server (MCP) re-installs the
/// process map on every call, which must not race a test's own map. No id
/// counts as literal on this thread (see [`install_for_thread_with_literals`]).
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
pub fn install_for_thread(map: NodeAliasMap) -> ThreadMapGuard {
    install_for_thread_with(map, None)
}

/// [`install_for_thread`] plus a thread-scoped literal-id check.
pub fn install_for_thread_with_literals(
    map: NodeAliasMap,
    is_literal: impl Fn(&str) -> bool + Send + Sync + 'static,
) -> ThreadMapGuard {
    install_for_thread_with(map, Some(Arc::new(is_literal)))
}

fn install_for_thread_with(map: NodeAliasMap, check: Option<LiteralCheck>) -> ThreadMapGuard {
    ThreadMapGuard(
        THREAD_MAP.with(|t| t.replace(Some(Arc::new(map)))),
        THREAD_LITERAL.with(|t| t.replace(Some(check))),
    )
}

/// This process's installed map (empty until [`install`] runs); a
/// thread-scoped map ([`install_for_thread`]) wins.
pub fn installed() -> Arc<NodeAliasMap> {
    if let Some(map) = THREAD_MAP.with(|t| t.borrow().clone()) {
        return map;
    }
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

/// THE resolution order for a spec ID, shared by every store resolver: run
/// `lookup` on `id` exactly as given (native `spec_id`, then `agreed_id`, is
/// the lookup's own rule); only when that finds nothing, expand a node-label
/// form and look the canonical id up. A literal id such as `BUG-JM-001` is
/// therefore never shadowed by label JM. A lookup error on the literal id
/// (e.g. an ambiguous id) is returned as-is, never retried as an alias.
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
pub fn resolve_literal_first<T, E>(
    id: &str,
    mut lookup: impl FnMut(&str) -> Result<Option<T>, E>,
) -> Result<Option<T>, E> {
    if let Some(hit) = lookup(id)? {
        return Ok(Some(hit));
    }
    match expand_spec_id(id) {
        Some(canonical) => lookup(&canonical),
        None => Ok(None),
    }
}

/// [`resolve_literal_first`] for an infallible lookup.
pub fn resolve_literal_first_opt<T>(
    id: &str,
    mut lookup: impl FnMut(&str) -> Option<T>,
) -> Option<T> {
    match resolve_literal_first(id, |i| Ok::<_, std::convert::Infallible>(lookup(i))) {
        Ok(hit) => hit,
        Err(never) => match never {},
    }
}

/// The canonical id a hand-written spec-ID token (commit trailer, `Refs`
/// group, `trace:` comment) names, by [`resolve_literal_first`]: `id` itself
/// when it exists literally in the store (the installed literal check) or is
/// not a known alias form, else its expanded canonical id. The literal check
/// runs only for label-shaped tokens, so plain ids never trigger a store scan.
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
pub fn expand_or_keep(id: &str) -> String {
    if expand_spec_id(id).is_none() {
        return id.to_string();
    }
    let is_literal = installed_literal_check();
    // The literal id "resolves" only when stored; the expanded id is the
    // canonical one by construction.
    resolve_literal_first_opt(id, |candidate| {
        let stored = candidate != id || is_literal.as_ref().is_some_and(|check| check(candidate));
        stored.then(|| candidate.to_string())
    })
    .unwrap_or_else(|| id.to_string())
}

/// Display form of a spec ID with the installed map (see
/// [`NodeAliasMap::display`]), honouring `--canonical-ids`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — the sequence is
// rendered at the configured `[ids]` width first (on the canonical node id,
// so node-local widths apply), then the label is substituted. Stored ids are
// never re-padded; `--canonical-ids` turns off labels, not width.
pub fn display_spec_id(id: &str) -> String {
    installed().display(&crate::id_identity::render_id(id), canonical_ids())
}

// ---------------------------------------------------------------------------
// Bare ids vs node-local numbers (BUG-b6b1…-036).
//
// A bare id (`STORY-100`) is the globally agreed name; a node-local id is
// always `TYPE-<node>-NNN` on disk, so a bare lookup never resolves to one.
// The two number spaces overlap, though: `STORY-100` (agreed, legacy) and
// `STORY-HOME-100` (local) are different specs. These helpers find the local
// "twin" of a bare id so surfaces can point at it instead of staying silent.
// ---------------------------------------------------------------------------

/// `TYPE-NNN` split into its upper-cased type prefix and number. `None` for
/// anything else, including node-local and alias forms (three segments).
// trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
pub fn split_bare_id(id: &str) -> Option<(String, u32)> {
    let (prefix, seq) = id.trim().split_once('-')?;
    let prefix_ok = prefix
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && prefix.chars().all(|c| c.is_ascii_alphabetic());
    let seq_ok = !seq.is_empty() && seq.chars().all(|c| c.is_ascii_digit());
    if !(prefix_ok && seq_ok) {
        return None;
    }
    Some((prefix.to_ascii_uppercase(), seq.parse().ok()?))
}

/// Whether `candidate` is a node-local id (`TYPE-<32-hex node>-NNN`) with the
/// same type prefix and number as the bare id `bare`. Leading zeros do not
/// matter: `BUG-30` and `BUG-030` both have the local twin `BUG-<node>-030`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
pub fn is_local_twin(bare: &str, candidate: &str) -> bool {
    let Some((prefix, n)) = split_bare_id(bare) else {
        return false;
    };
    let Some((c_prefix, middle, seq)) = split_three(candidate) else {
        return false;
    };
    c_prefix.eq_ignore_ascii_case(&prefix)
        && middle.len() == 32
        && normalize_node(middle).is_ok()
        && seq.parse::<u32>().ok() == Some(n)
}

/// The `(TYPE, number)` key of a node-local id (`TYPE-<32-hex node>-NNN`),
/// equal to [`split_bare_id`] of each bare id it is a local twin of. Lets a
/// caller index the store's local ids once instead of testing every pair.
// trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
pub fn local_id_key(id: &str) -> Option<(String, u32)> {
    let (prefix, middle, seq) = split_three(id)?;
    let bare = format!("{prefix}-{seq}");
    (middle.len() == 32 && normalize_node(middle).is_ok() && is_local_twin(&bare, id))
        .then(|| split_bare_id(&bare))
        .flatten()
}

/// Every id in `known` that is a local twin of the bare id `bare` (see
/// [`is_local_twin`]), upper-cased, sorted and deduplicated.
// trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
pub fn local_twins<'a>(bare: &str, known: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = known
        .into_iter()
        .filter(|k| is_local_twin(bare, k))
        .map(|k| k.trim().to_ascii_uppercase())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// How a surface names a local twin: its agreed id when it has one, else its
/// alias form (`STORY-HOME-100`). Never the bare number alone.
// trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
pub fn local_twin_label(local_id: &str, agreed_id: Option<&str>) -> String {
    match agreed_id.map(str::trim).filter(|agreed| !agreed.is_empty()) {
        Some(agreed) => agreed.to_ascii_uppercase(),
        None => display_spec_id(local_id),
    }
}

impl NodeAliasMap {
    /// The canonical local-twin ids a bare id could have on the nodes this map
    /// labels (current or retired labels name the same node), formatted the
    /// way the node-local allocator writes them (`{seq:03}`). For a cheap
    /// existence probe that does not scan the store.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
    pub fn local_twin_candidates(&self, bare: &str) -> Vec<String> {
        let Some((prefix, n)) = split_bare_id(bare) else {
            return Vec::new();
        };
        let mut out: Vec<String> = self
            .bindings
            .iter()
            .map(|b| format!("{prefix}-{}-{n:03}", b.node.to_ascii_uppercase()))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// Bare `TYPE-NNN` tokens in free text, with their 1-based line numbers. A
/// token is bounded by characters that cannot continue an id (so the `1-096`
/// inside `TASK-1-096` and the `HOME-030` inside `BUG-HOME-030` are not
/// tokens), and its prefix must be at least two letters.
// trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
pub fn bare_id_tokens(text: &str) -> Vec<(usize, String)> {
    fn id_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    }
    let mut out = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        for token in line.split(|c: char| !id_char(c)) {
            let token = token.trim_end_matches('-');
            if split_bare_id(token).is_some_and(|(prefix, _)| prefix.len() >= 2) {
                out.push((line_no + 1, token.to_ascii_uppercase()));
            }
        }
    }
    out
}

/// Whether `id` is `TYPE-<label>-NNN` (any case) for `label`.
fn id_uses_label(id: &str, label: &str) -> bool {
    split_three(id).is_some_and(|(_, middle, _)| middle.eq_ignore_ascii_case(label))
}

/// Refuse any label `after` binds that `before` did not when
/// `TYPE-<label>-NNN` would name two different requirements: the label equals
/// a node id registered in `registry/nodes.toml`, or a stored `spec_id` or
/// `agreed_id` already has the label as its middle segment (display of the
/// labelled node's ids would then parse back to that literal spec). Shared by
/// every write (`set` and `rename`) because it runs inside [`mutate_cas`].
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
#[cfg(feature = "native")]
fn refuse_colliding_labels(
    store_root: &Path,
    before: &NodeAliasMap,
    after: &NodeAliasMap,
) -> anyhow::Result<()> {
    let added: Vec<&str> = after
        .bindings
        .iter()
        .filter(|b| before.binding(&b.label).is_none())
        .map(|b| b.label.as_str())
        .collect();
    if added.is_empty() {
        return Ok(());
    }
    let roster = crate::node::NodeRegistry::load(&store_root.join("registry").join("nodes.toml"))
        .unwrap_or_default();
    if let Some(label) = added.iter().find(|label| {
        roster
            .nodes
            .iter()
            .any(|n| n.id.as_str().eq_ignore_ascii_case(label))
    }) {
        anyhow::bail!(
            "label {label} is already a registered node id; spec IDs like TASK-{label}-001 belong to that node"
        );
    }
    let objects = store_root.join("objects");
    for (stem, path) in crate::object_store::list_objects(&objects)? {
        let mut ids = vec![stem];
        // An unreadable object cannot hold an agreed_id we could resolve to
        // either, so skipping it keeps the label usable.
        if let Ok(req) = crate::object_store::read_object_from_path(&path) {
            ids.extend(req.spec_id);
            ids.extend(req.agreed_id);
        }
        for id in ids {
            if let Some(label) = added.iter().find(|label| id_uses_label(&id, label)) {
                anyhow::bail!(
                    "label {label} is already the middle of stored spec ID {}; choose another label",
                    id.to_ascii_uppercase()
                );
            }
        }
    }
    Ok(())
}

/// The reverse of [`refuse_colliding_labels`]: refuse registering node id
/// `node_id` when it equals a current or retired label in
/// `registry/node_aliases.toml`, since `TYPE-<node_id>-NNN` would then also be
/// the labelled node's display form. Called by every node-registration path
/// inside its store write lock, so a label bound concurrently is seen.
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
#[cfg(feature = "native")]
pub fn refuse_node_id_colliding_with_label(store_root: &Path, node_id: &str) -> anyhow::Result<()> {
    let map = NodeAliasMap::load_strict(store_root)?;
    if let Some(b) = map.binding(node_id.trim()) {
        anyhow::bail!(
            "node id {node_id} is already {} node label {} for node {}; spec IDs like TASK-{}-001 belong to that node — pick a different node id",
            if b.retired { "a retired" } else { "the" },
            b.label,
            b.node,
            b.label
        );
    }
    Ok(())
}

/// What [`mutate_cas`] did with a node-alias write.
// trace:BUG-B6B1278890774EF993D2EBC327862470-025 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutateCasOutcome {
    /// The map already said this; nothing was committed.
    Unchanged,
    /// The write is committed locally as `sha` on `branch`.
    Committed {
        sha: String,
        branch: String,
        origin: OriginPublish,
    },
}

impl MutateCasOutcome {
    pub fn changed(&self) -> bool {
        matches!(self, Self::Committed { .. })
    }
}

/// How a committed node-alias write fared against `origin`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-025 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginPublish {
    Pushed,
    /// No `origin` remote: a solo clone, the commit stays local.
    NoOrigin,
    /// The push failed for a reason other than a moved ref (offline, no
    /// access, a declining hook). The local commit is kept, since the CAS
    /// already won locally; carries git's stderr.
    Unreached(String),
}

/// Commit-and-push a mutation of the node-alias map with the registry CAS
/// push-wins loop (mirrors [`crate::alias::link_cas`]): pull, strict-load,
/// mutate, save, commit, push; a rejected push resets and retries. Solo (no
/// `origin`) commits locally. An `origin` that cannot be reached keeps the
/// local commit and reports [`OriginPublish::Unreached`] so the caller can
/// publish to the other hubs and warn, instead of failing a write that landed.
// trace:BUG-B6B1278890774EF993D2EBC327862470-025 | ai:claude
#[cfg(feature = "native")]
pub fn mutate_cas(
    store_root: &Path,
    describe: &str,
    mutate: impl Fn(&mut NodeAliasMap) -> Result<bool, String>,
) -> anyhow::Result<MutateCasOutcome> {
    use crate::git_ops::{self, PushFailure, PushOutcome};

    const MAX_RETRIES: u32 = 10;
    let branch = git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    let local_only = !git_ops::has_remote(store_root, "origin");

    for attempt in 0..MAX_RETRIES {
        let committed = crate::db::with_store_write_lock(store_root, || {
            if attempt > 0 && !local_only {
                git_ops::pull_rebase(store_root, "origin", &branch)?;
            }
            let mut map = NodeAliasMap::load_strict(store_root)?;
            let before = map.clone();
            if !mutate(&mut map).map_err(anyhow::Error::msg)? {
                return Ok(None);
            }
            refuse_colliding_labels(store_root, &before, &map)?;
            map.save(store_root)?;
            git_ops::add(store_root, &[NODE_ALIASES_TOML_REL])?;
            git_ops::commit(store_root, &format!("chore(registry): {describe}"))?;
            Ok(Some(git_ops::head_sha(store_root)?))
        })?;
        let Some(sha) = committed else {
            return Ok(MutateCasOutcome::Unchanged);
        };
        let origin = if local_only {
            OriginPublish::NoOrigin
        } else {
            match git_ops::push_exact_outcome(store_root, "origin", &sha, &branch)? {
                PushOutcome::Pushed => OriginPublish::Pushed,
                PushOutcome::Failed {
                    kind: PushFailure::NonFastForward,
                    ..
                } => {
                    let _ = git_ops::rollback_commit_if_head_matches(store_root, &sha);
                    continue;
                }
                PushOutcome::Failed { stderr, .. } => OriginPublish::Unreached(stderr),
            }
        };
        return Ok(MutateCasOutcome::Committed {
            sha,
            branch,
            origin,
        });
    }
    anyhow::bail!(
        "could not write the node label after {MAX_RETRIES} attempts (store push kept being rejected) — run `aida db sync --pull` and retry"
    )
}

#[cfg(test)]
#[path = "node_alias_tests.rs"]
mod tests;
