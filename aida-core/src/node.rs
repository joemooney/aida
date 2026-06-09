// trace:ARCH-distributed-node | ai:claude
//! Node identity and workspace configuration for distributed AIDA.
//!
//! A **node** is a single clone/installation of AIDA. Each node gets a unique
//! ID — a `String` matching `[A-Za-z][A-Za-z0-9_-]*` (typically initials like
//! "JM" or a sequential number like "1") — via the git CAS push loop at
//! `aida init`. After registration, the node can generate globally unique
//! object IDs offline indefinitely.
//!
//! Node IDs were `u32` pre-EPIC-9; the deserializer below accepts either a
//! string or a number for back-compat with existing `nodes.toml` files.
//!
//! A **workspace** groups multiple code repos that share a single AIDA database.
//! The workspace config is discovered by walking up the directory tree.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::dispenser::IdMode;

// ---------------------------------------------------------------------------
// Node ID type + back-compat deserializer
// ---------------------------------------------------------------------------

/// Validate a candidate node id against the kernel's format rules:
/// `[A-Za-z0-9][A-Za-z0-9_-]*`, length 1–32. Returns Err with a human-
/// readable message on rejection.
/// trace:STORY-41 | ai:claude
pub fn validate_node_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("node id must not be empty".into());
    }
    if id.len() > 32 {
        return Err(format!("node id '{}' exceeds 32 characters", id));
    }
    let mut chars = id.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return Err(format!(
            "node id '{}' must start with an alphanumeric character",
            id
        ));
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!(
                "node id '{}' contains invalid character '{}' (use letters, digits, '-', '_')",
                id, c
            ));
        }
    }
    Ok(())
}

/// Deserialize a node id from either a string or an integer. Pre-EPIC-9
/// stores wrote `id = 1`; new stores write `id = "JM"` or `id = "1"`.
/// trace:STORY-41 | ai:claude
fn deserialize_node_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Str(String),
        Num(u64),
    }
    Ok(match Repr::deserialize(deserializer)? {
        Repr::Str(s) => s,
        Repr::Num(n) => n.to_string(),
    })
}

/// Same as [`deserialize_node_id`] but for `Option<String>`. Reserved for
/// future use (e.g., parent-node references in NodeRegistryEntry).
#[allow(dead_code)]
fn deserialize_node_id_opt<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Str(String),
        Num(u64),
        None,
    }
    Ok(match Option::<Repr>::deserialize(deserializer)? {
        None | Some(Repr::None) => None,
        Some(Repr::Str(s)) => Some(s),
        Some(Repr::Num(n)) => Some(n.to_string()),
    })
}

// ---------------------------------------------------------------------------
// ID Format Policy (EPIC-1-052 Phase 2)
// ---------------------------------------------------------------------------

/// How a project chooses between node-aware ids (FR-1-005) and pre-allocated
/// agreed-id blocks (FR-005). Configured via `.aida/config.toml`:
///
/// ```toml
/// [id_format]
/// policy = "blocks-then-fallback"  # default
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum IdFormatPolicy {
    /// Always issue node-aware ids (`<TYPE>-<NODE>-<SEQ>`). Useful for
    /// projects that don't want to manage block allocation, or for solo
    /// developers who don't need stable agreed-ids.
    NodeAwareOnly,

    /// Try a pre-allocated block first (yields short `<TYPE>-<SEQ>`); fall
    /// through to a node-aware id if no block is allocated for the type.
    /// **Default** — matches the existing `use_agreed_blocks = true`
    /// behavior and is what the user picked for `aida init` defaults.
    #[default]
    BlocksThenFallback,

    /// Require an allocated block for every id; error out otherwise. Use
    /// when the project policy is "agreed-ids only — never let a node-aware
    /// id leak into trace comments or PR titles."
    BlocksOnly,
}

impl IdFormatPolicy {
    /// String label suitable for display ("node-aware-only", etc.).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NodeAwareOnly => "node-aware-only",
            Self::BlocksThenFallback => "blocks-then-fallback",
            Self::BlocksOnly => "blocks-only",
        }
    }

    /// Parse from the kebab-case string used in config.toml. Returns Err
    /// for unknown values so callers can surface a clear error.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "node-aware-only" => Ok(Self::NodeAwareOnly),
            "blocks-then-fallback" => Ok(Self::BlocksThenFallback),
            "blocks-only" => Ok(Self::BlocksOnly),
            other => Err(format!(
                "unknown id_format policy '{}': expected one of node-aware-only, \
                 blocks-then-fallback, blocks-only",
                other
            )),
        }
    }

    /// True when the policy permits dispensing from a block.
    pub fn uses_blocks(self) -> bool {
        matches!(self, Self::BlocksThenFallback | Self::BlocksOnly)
    }

    /// True when the policy requires a block — i.e., missing block is an error.
    pub fn requires_block(self) -> bool {
        matches!(self, Self::BlocksOnly)
    }
}

/// Whether the dispenser maintains a separate counter per type prefix
/// (`FR-1`, `BUG-1`, `EPIC-1` independent) or a single global counter
/// (`FR-1`, `BUG-2`, `EPIC-3`). Configured via `.aida/config.toml`:
///
/// ```toml
/// [id_format]
/// counter_scope = "global"  # or "per-type"
/// ```
///
/// **Global** is the default for projects created from 2026-05-09 onwards
/// — it makes the id space unambiguous on first contact ("did I make 5
/// reqs or 1?"). Existing projects without the field default to PerType
/// for back-compat — flipping a live store would conflate FR-100 and
/// BUG-100 numerically. trace:FR-271 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum IdCounterScope {
    /// One counter per type prefix. `FR-1`, `BUG-1`, `EPIC-1` are all
    /// distinct and start fresh.
    #[default]
    PerType,
    /// One counter shared across all types. `FR-1`, `BUG-2`, `EPIC-3` —
    /// the prefix labels what each id is *for*, but the number is
    /// globally unique within a node's block range.
    Global,
}

impl IdCounterScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PerType => "per-type",
            Self::Global => "global",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "per-type" | "pertype" | "PerType" => Ok(Self::PerType),
            "global" | "Global" => Ok(Self::Global),
            other => Err(format!(
                "unknown counter_scope '{}': expected 'per-type' or 'global'",
                other
            )),
        }
    }

    /// Sentinel `type_prefix` value used by the block registry to mark a
    /// block that covers all types under [`IdCounterScope::Global`]. Not a
    /// valid type prefix on its own — the dispenser substitutes the
    /// caller-requested prefix at dispense time.
    pub const GLOBAL_TYPE_PREFIX: &'static str = "*";
}

// ---------------------------------------------------------------------------
// Node Identity
// ---------------------------------------------------------------------------

/// Information about a registered node (persisted locally, gitignored).
/// Node IDs became `String` in EPIC-9; the deserializer accepts both for
/// back-compat with pre-EPIC-9 stores. trace:STORY-41 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// The assigned node ID (unique within the workspace).
    #[serde(deserialize_with = "deserialize_node_id")]
    pub node_id: String,
    /// The user ID who owns this node
    pub user_id: u32,
    /// Hostname at registration time (informational)
    pub hostname: String,
    /// User email at registration time (from `git config user.email`,
    /// optional for entries written before EPIC-1-052).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// When this node was registered
    pub registered_at: DateTime<Utc>,
}

impl NodeConfig {
    /// Load node config from a TOML file.
    #[cfg(feature = "native")]
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: NodeConfig = toml::from_str(&content)?;
        Ok(config)
    }

    /// Save node config to a TOML file.
    #[cfg(feature = "native")]
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Get the IdMode for this node.
    pub fn id_mode(&self) -> IdMode {
        IdMode::Distributed {
            node_id: self.node_id.clone(),
        }
    }
}

/// A node registration entry in the shared registry (committed to git).
/// trace:STORY-41 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRegistryEntry {
    /// The assigned node ID. Strings as of EPIC-9; numeric ids from older
    /// stores deserialize as their decimal string ("1", "2", ...).
    #[serde(deserialize_with = "deserialize_node_id")]
    pub id: String,
    /// The user ID who owns this node
    pub user_id: u32,
    /// Hostname at registration time
    pub hostname: String,
    /// User email at registration time (`git config user.email`).
    /// Optional so entries written before EPIC-1-052 deserialize cleanly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Absolute path of the clone's `.aida-store/` parent at register time.
    /// Used by `aida node acquire --hijack` (STORY-43) to mark a same-host-
    /// same-user clone as obsolete instead of silently re-attributing its
    /// blocks. Optional so pre-EPIC-9 entries deserialize cleanly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_path: Option<PathBuf>,
    /// Registration timestamp
    pub registered: DateTime<Utc>,
}

/// The shared node registry (committed to git, append-only).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeRegistry {
    /// All registered nodes
    #[serde(default)]
    pub nodes: Vec<NodeRegistryEntry>,
}

impl NodeRegistry {
    /// Get the next available numeric-style node ID — used as the
    /// fallback when no preferred id is configured. Walks existing
    /// numeric ids and returns max+1 (or "1" when none exist).
    /// String-typed ids that aren't pure-numeric are ignored when
    /// computing the next numeric.
    /// trace:STORY-41 | ai:claude
    pub fn next_node_id(&self) -> String {
        let max_numeric: u32 = self
            .nodes
            .iter()
            .filter_map(|n| n.id.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        (max_numeric + 1).to_string()
    }

    /// Get the lowest-numbered free string id of the form `<prefix><N>`
    /// where N >= 1. Returns `prefix` itself when free; otherwise
    /// `<prefix>2`, `<prefix>3`, …. Used by STORY-42's auto-suffix flow
    /// when a preferred id is taken (e.g., "JM" → "JM2").
    /// trace:STORY-42 | ai:claude
    pub fn next_free_with_prefix(&self, prefix: &str) -> String {
        if !self.is_registered(prefix) {
            return prefix.to_string();
        }
        for n in 2..u32::MAX {
            let candidate = format!("{}{}", prefix, n);
            if !self.is_registered(&candidate) {
                return candidate;
            }
        }
        // Practically unreachable.
        format!("{}-overflow", prefix)
    }

    /// Check if a node ID is already registered.
    pub fn is_registered(&self, node_id: &str) -> bool {
        self.nodes.iter().any(|n| n.id.as_str() == node_id)
    }

    /// Get a node entry by ID.
    pub fn get(&self, node_id: &str) -> Option<&NodeRegistryEntry> {
        self.nodes.iter().find(|n| n.id.as_str() == node_id)
    }

    /// Register a new node. Returns the assigned node ID.
    pub fn register(&mut self, user_id: u32, hostname: String) -> String {
        self.register_with_email(user_id, hostname, None)
    }

    /// Register a new node, capturing the user's email at registration.
    /// Stamping both hostname AND email disambiguates two clones of the
    /// same repo on the same host (EPIC-1-052 Q4).
    pub fn register_with_email(
        &mut self,
        user_id: u32,
        hostname: String,
        email: Option<String>,
    ) -> String {
        let id = self.next_node_id();
        self.register_specific(id.clone(), user_id, hostname, email);
        id
    }

    /// Register at a specific node id. Caller is responsible for checking
    /// the id isn't already taken (use `is_registered`).
    /// trace:EPIC-1-052 | ai:claude
    pub fn register_specific(
        &mut self,
        id: String,
        user_id: u32,
        hostname: String,
        email: Option<String>,
    ) {
        self.register_specific_full(id, user_id, hostname, email, None);
    }

    /// Register with full provenance — including the absolute clone path
    /// for STORY-43 (hijack mark-in-place).
    /// trace:STORY-41 | ai:claude
    pub fn register_specific_full(
        &mut self,
        id: String,
        user_id: u32,
        hostname: String,
        email: Option<String>,
        clone_path: Option<PathBuf>,
    ) {
        self.nodes.push(NodeRegistryEntry {
            id,
            user_id,
            hostname,
            email,
            clone_path,
            registered: Utc::now(),
        });
    }

    /// Remove a node by id. Returns true if the entry existed.
    pub fn remove(&mut self, node_id: &str) -> bool {
        let before = self.nodes.len();
        self.nodes.retain(|n| n.id.as_str() != node_id);
        self.nodes.len() != before
    }

    /// Load from a TOML file.
    #[cfg(feature = "native")]
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let registry: NodeRegistry = toml::from_str(&content)?;
        Ok(registry)
    }

    /// Save to a TOML file.
    #[cfg(feature = "native")]
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Hijack marker (STORY-43)
// ---------------------------------------------------------------------------

/// Marker file dropped into an abandoned clone's `.aida-store/.aida/` when
/// another clone hijacks its node id. Subsequent `aida` invocations in the
/// abandoned clone read this and print a loud warning so the user doesn't
/// keep issuing requirements with a now-reassigned node id.
///
/// Path: `<clone>/.aida-store/.aida/HIJACKED.toml`. trace:STORY-43
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HijackMarker {
    /// The node id that was hijacked (used to be ours).
    pub node_id: String,
    /// Hostname of the new owner clone.
    pub new_owner_hostname: String,
    /// Email of the new owner clone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_owner_email: Option<String>,
    /// Absolute path of the new owner clone (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_owner_clone_path: Option<PathBuf>,
    /// When the hijack happened.
    pub hijacked_at: DateTime<Utc>,
}

impl HijackMarker {
    /// Standard filename written into `<clone>/.aida-store/.aida/HIJACKED.toml`.
    pub const FILENAME: &'static str = "HIJACKED.toml";

    /// Compute the marker path inside an `.aida-store/` worktree.
    pub fn path_in_store(store_path: &Path) -> PathBuf {
        store_path.join(".aida").join(Self::FILENAME)
    }

    #[cfg(feature = "native")]
    pub fn load(path: &Path) -> anyhow::Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(path)?;
        let marker: Self = toml::from_str(&content)?;
        Ok(Some(marker))
    }

    #[cfg(feature = "native")]
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// User Identity
// ---------------------------------------------------------------------------

/// A user registration entry in the shared registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRegistryEntry {
    /// The assigned user ID
    pub id: u32,
    /// Display name
    pub name: String,
    /// Email address (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Registration timestamp
    pub registered: DateTime<Utc>,
}

/// The shared user registry (committed to git, append-only).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserRegistry {
    #[serde(default)]
    pub users: Vec<UserRegistryEntry>,
}

impl UserRegistry {
    /// Get the next available user ID.
    pub fn next_user_id(&self) -> u32 {
        self.users.iter().map(|u| u.id).max().unwrap_or(0) + 1
    }

    /// Register a new user. Returns the assigned user ID.
    pub fn register(&mut self, name: String, email: Option<String>) -> u32 {
        let id = self.next_user_id();
        self.users.push(UserRegistryEntry {
            id,
            name,
            email,
            registered: Utc::now(),
        });
        id
    }

    /// Find a user by name.
    pub fn find_by_name(&self, name: &str) -> Option<&UserRegistryEntry> {
        self.users
            .iter()
            .find(|u| u.name.eq_ignore_ascii_case(name))
    }
}

// ---------------------------------------------------------------------------
// Pre-allocated Agreed ID Blocks (FR-2-005)
// ---------------------------------------------------------------------------

/// A pre-allocated block of agreed IDs for a node+type.
///
/// Each node claims a contiguous range of agreed IDs (e.g., FR-300..FR-399)
/// from the shared block registry on the aida-store branch. This allows
/// trace comments like `// trace:FR-300` to use stable, agreed IDs immediately
/// without waiting for merge-gate, even when working offline.
///
/// When `next > range_end` the block is exhausted; the node must claim a new
/// block (requires network to push). When `next >= range_end - LOW_THRESHOLD`
/// a warning is printed on `aida add`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgreedIdBlock {
    /// Node that owns this block. Strings as of EPIC-9; numeric ids from
    /// older stores deserialize as decimal strings. trace:STORY-41 | ai:claude
    #[serde(deserialize_with = "deserialize_node_id")]
    pub node_id: String,
    /// Human-readable owner label (e.g., "joe@work")
    pub owner: String,
    /// Hostname at claim time (informational)
    pub hostname: String,
    /// Type prefix this block covers (e.g., "FR", "BUG")
    pub type_prefix: String,
    /// First ID in the range (inclusive)
    pub range_start: u32,
    /// Last ID in the range (inclusive)
    pub range_end: u32,
    /// Next ID to dispense from this block
    pub next: u32,
    /// When this block was claimed
    pub allocated_at: DateTime<Utc>,
}

impl AgreedIdBlock {
    /// Number of IDs remaining in this block (including `next`).
    pub fn remaining(&self) -> u32 {
        if self.next > self.range_end {
            0
        } else {
            self.range_end - self.next + 1
        }
    }

    /// True when the block has no more IDs to dispense.
    pub fn is_exhausted(&self) -> bool {
        self.next > self.range_end
    }

    /// True when the block is running low (≤ LOW_THRESHOLD remaining).
    pub fn is_low(&self) -> bool {
        self.remaining() <= Self::LOW_THRESHOLD
    }

    /// Number of remaining IDs that triggers a low-block warning.
    pub const LOW_THRESHOLD: u32 = 10;

    /// Dispense the next agreed ID from this block, updating `next`.
    /// Returns the formatted ID (e.g., "FR-300") or None if exhausted.
    /// For global-scope blocks (type_prefix = `*`), prefer
    /// [`dispense_with_prefix`] so the caller's requested type label is
    /// used in the output instead of the literal `*`.
    pub fn dispense(&mut self) -> Option<String> {
        self.dispense_with_prefix(&self.type_prefix.clone())
    }

    /// Dispense the next id and format it with `prefix` instead of the
    /// block's own `type_prefix`. Lets a single global block (`*`) serve
    /// any type request. trace:FR-271 | ai:claude
    pub fn dispense_with_prefix(&mut self, prefix: &str) -> Option<String> {
        if self.is_exhausted() {
            return None;
        }
        let id = format!("{}-{}", prefix.to_uppercase(), self.next);
        self.next += 1;
        Some(id)
    }
}

/// The shared block registry — stored at `.aida-store/registry/blocks.yaml`.
///
/// This file is committed to the aida-store branch. Claiming a new block
/// requires a push (atomic via git push-wins-retry semantics). Using IDs
/// from an already-claimed block is purely local — no network needed.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BlockRegistry {
    #[serde(default)]
    pub blocks: Vec<AgreedIdBlock>,
}

impl BlockRegistry {
    /// Load from a YAML file. Returns an empty registry if the file does not exist.
    #[cfg(feature = "native")]
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        // Read through `read_atomic`: a concurrent `aida add` writer may be
        // mid-`write_atomic` rename on Windows, which surfaces as a transient
        // PermissionDenied/NotFound. The bounded retry absorbs it.
        // trace:BUG-474 | ai:claude
        let content = crate::read_atomic(path)?;
        let registry: BlockRegistry = serde_yaml::from_str(&content)?;
        Ok(registry)
    }

    /// Save to a YAML file.
    ///
    /// Writes atomically (tempfile + rename) so a concurrent reader — or a
    /// crash — never observes a torn, half-written block registry. The
    /// advisory lock in [`with_dispense_lock`](Self::with_dispense_lock)
    /// serializes the read-modify-write writers; the atomic rename
    /// additionally guarantees the on-disk file is never half-updated.
    /// trace:BUG-474 | ai:claude
    #[cfg(feature = "native")]
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_yaml::to_string(self)?;
        crate::write_atomic(path, content)?;
        Ok(())
    }

    /// Run `f` while holding an exclusive advisory lock tied to the block
    /// registry at `blocks_path`.
    ///
    /// The agreed-id dispense path is a read-modify-write: `load` → find
    /// active block → `dispense` (advances `next`) → `save`. Without a lock
    /// two concurrent `aida add` processes both load `next = N`, both
    /// dispense `<TYPE>-N`, and both save → a **duplicate stable id**. This
    /// serializes the whole sequence on a sibling `<blocks>.lock` file using
    /// an `fs2` exclusive advisory lock, mirroring the pattern TASK-331 used
    /// for the `FileDispenser` counter. The closure performs the full
    /// load→dispense→save under the lock; callers must not load/save the
    /// registry outside it. The lock is released when this returns (success
    /// or error).
    ///
    /// trace:BUG-474 | ai:claude
    #[cfg(feature = "native")]
    pub fn with_dispense_lock<T, F>(blocks_path: &Path, f: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> anyhow::Result<T>,
    {
        use fs2::FileExt;
        use std::fs::OpenOptions;

        if let Some(parent) = blocks_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock_path = blocks_path.with_extension("lock");
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        lock_file.lock_exclusive()?;

        let result = f();

        // Release the lock regardless of how the closure exited.
        let _ = lock_file.unlock();
        result
    }

    /// Find the active (non-exhausted) block for a given node + type prefix.
    /// Returns the index into `self.blocks` if found.
    pub fn find_active_block(&self, node_id: &str, type_prefix: &str) -> Option<usize> {
        let prefix = type_prefix.to_uppercase();
        self.blocks
            .iter()
            .enumerate()
            .find(|(_, b)| {
                b.node_id.as_str() == node_id
                    && b.type_prefix.to_uppercase() == prefix
                    && !b.is_exhausted()
            })
            .map(|(i, _)| i)
    }

    /// Like [`find_active_block`] but falls back to the node's
    /// [`IdCounterScope::GLOBAL_TYPE_PREFIX`] (`*`) block when no exact
    /// match exists. Used at dispense time so a global-scope project
    /// (single shared counter per node) can serve any type request.
    /// trace:FR-271 | ai:claude
    pub fn find_active_block_or_global(&self, node_id: &str, type_prefix: &str) -> Option<usize> {
        if let Some(idx) = self.find_active_block(node_id, type_prefix) {
            return Some(idx);
        }
        self.find_active_block(node_id, IdCounterScope::GLOBAL_TYPE_PREFIX)
    }

    /// Return the next range_start for a new block of the given type prefix.
    /// This is max(range_end) across all blocks of that type + 1, or 1 if none exist.
    pub fn next_range_start(&self, type_prefix: &str) -> u32 {
        let prefix = type_prefix.to_uppercase();
        self.blocks
            .iter()
            .filter(|b| b.type_prefix.to_uppercase() == prefix)
            .map(|b| b.range_end)
            .max()
            .map(|max_end| max_end + 1)
            .unwrap_or(1)
    }

    /// Return the next range_start for a new block, accounting for both
    /// existing blocks AND the agreed-id counter floor. The counter floor
    /// is the highest already-issued agreed id for this type — a block
    /// must start strictly above it to avoid colliding with existing reqs
    /// (e.g., post-merge-gate or post-retire-legacy-ids stores).
    /// trace:FR-1-073 | ai:claude
    pub fn next_range_start_above_counter(&self, type_prefix: &str, counter_floor: u32) -> u32 {
        let from_blocks = self.next_range_start(type_prefix);
        from_blocks.max(counter_floor + 1)
    }

    /// Append a new block for the given node. Returns the claimed block.
    pub fn claim_block(
        &mut self,
        node_id: String,
        owner: String,
        hostname: String,
        type_prefix: String,
        size: u32,
    ) -> AgreedIdBlock {
        self.claim_block_with_floor(node_id, owner, hostname, type_prefix, size, 0)
    }

    /// Append a new block, ensuring the range starts strictly above the
    /// supplied counter floor. Use this when a store already has issued
    /// agreed-ids (e.g., from prior merge-gate runs or retire-legacy-ids
    /// migrations) — the floor is the current counter value.
    /// trace:FR-1-073 | ai:claude
    pub fn claim_block_with_floor(
        &mut self,
        node_id: String,
        owner: String,
        hostname: String,
        type_prefix: String,
        size: u32,
        counter_floor: u32,
    ) -> AgreedIdBlock {
        let range_start = self.next_range_start_above_counter(&type_prefix, counter_floor);
        let range_end = range_start + size - 1;
        let block = AgreedIdBlock {
            node_id,
            owner,
            hostname,
            type_prefix: type_prefix.to_uppercase(),
            range_start,
            range_end,
            next: range_start,
            allocated_at: Utc::now(),
        };
        self.blocks.push(block.clone());
        block
    }

    /// Dispense the next agreed ID for a node+type, updating the block in-place.
    /// Falls back to the node's global (`*`) block when no per-type
    /// match exists — formats the id with `type_prefix` either way so
    /// global-scope projects produce `FR-1`, `BUG-2`, `EPIC-3` from a
    /// single shared counter. trace:FR-271 | ai:claude
    /// Returns `(id, is_low)` or None if no active block / exhausted.
    pub fn dispense(&mut self, node_id: &str, type_prefix: &str) -> Option<(String, bool)> {
        let idx = self.find_active_block_or_global(node_id, type_prefix)?;
        let block = &mut self.blocks[idx];
        let id = block.dispense_with_prefix(type_prefix)?;
        let is_low = block.is_low();
        Some((id, is_low))
    }

    /// Aggregate-low threshold across all non-exhausted blocks owned by a
    /// single (node, type) pair. The per-block [`AgreedIdBlock::LOW_THRESHOLD`]
    /// (10) still controls `is_low()` for individual blocks; user-facing
    /// "claim soon" warnings branch off the aggregate so a near-empty
    /// lower block does not nag when a fresh higher block has been
    /// claimed. trace:BUG-115 | ai:claude
    pub const AGGREGATE_LOW_THRESHOLD: u32 = 20;

    /// Sum of remaining IDs across every non-exhausted block owned by
    /// `node_id` for `type_prefix`. Exhausted blocks contribute zero;
    /// they are kept in the registry for history but no longer reduce
    /// the user's headroom. Case-insensitive on the type prefix.
    /// trace:BUG-115 | ai:claude
    pub fn aggregate_remaining(&self, node_id: &str, type_prefix: &str) -> u32 {
        let prefix = type_prefix.to_uppercase();
        self.blocks
            .iter()
            .filter(|b| {
                b.node_id == node_id && b.type_prefix.to_uppercase() == prefix && !b.is_exhausted()
            })
            .map(|b| b.remaining())
            .sum()
    }

    /// Count of non-exhausted blocks owned by `node_id` for `type_prefix`.
    /// Used to render the "across N blocks" suffix in user-facing output.
    /// trace:BUG-115 | ai:claude
    pub fn active_block_count(&self, node_id: &str, type_prefix: &str) -> usize {
        let prefix = type_prefix.to_uppercase();
        self.blocks
            .iter()
            .filter(|b| {
                b.node_id == node_id && b.type_prefix.to_uppercase() == prefix && !b.is_exhausted()
            })
            .count()
    }

    /// True when the user has at least one active block for the type AND
    /// the aggregate remaining across those active blocks is at or below
    /// [`AGGREGATE_LOW_THRESHOLD`]. Returns false when every block is
    /// exhausted — that state is surfaced as a separate
    /// "exhausted, run claim" message, not a "low" warning.
    /// trace:BUG-115 | ai:claude
    pub fn aggregate_is_low(&self, node_id: &str, type_prefix: &str) -> bool {
        self.active_block_count(node_id, type_prefix) > 0
            && self.aggregate_remaining(node_id, type_prefix) <= Self::AGGREGATE_LOW_THRESHOLD
    }
}

// ---------------------------------------------------------------------------
// Agreed ID Counters
// ---------------------------------------------------------------------------

/// Per-type counters for agreed IDs assigned at merge-to-trunk.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgreedCounters {
    /// Maps type prefix to the last assigned agreed sequence number.
    /// e.g., {"FR": 422, "FEAT": 89}
    #[serde(flatten)]
    pub counters: std::collections::HashMap<String, u32>,
}

impl AgreedCounters {
    /// Get the next agreed ID for a type and increment the counter.
    pub fn next(&mut self, type_prefix: &str) -> u32 {
        let counter = self.counters.entry(type_prefix.to_uppercase()).or_insert(0);
        *counter += 1;
        *counter
    }

    /// Peek at the next agreed ID without incrementing.
    pub fn peek(&self, type_prefix: &str) -> u32 {
        self.counters
            .get(&type_prefix.to_uppercase())
            .copied()
            .unwrap_or(0)
            + 1
    }

    /// Format an agreed ID: `FR-423`
    pub fn format_agreed_id(type_prefix: &str, seq: u32) -> String {
        format!("{}-{}", type_prefix.to_uppercase(), seq)
    }
}

// ---------------------------------------------------------------------------
// Workspace Configuration
// ---------------------------------------------------------------------------

/// Workspace configuration — discovered by walking up the directory tree.
///
/// A workspace groups multiple code repos that share a single AIDA database
/// (the `aida_path` repo). All nodes within a workspace share the same
/// node registry and agreed ID counters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    /// Workspace name (e.g., "gdms-disruptive")
    pub workspace: String,
    /// Path to the aida repo (relative to workspace root)
    #[serde(default = "default_aida_path")]
    pub aida_path: String,
    /// Code repos in this workspace
    #[serde(default)]
    pub repos: Vec<String>,
}

fn default_aida_path() -> String {
    "./aida".to_string()
}

impl WorkspaceConfig {
    /// Discover a workspace config by walking up the directory tree
    /// looking for `.aida-workspace`.
    #[cfg(feature = "native")]
    pub fn discover(from: &Path) -> Option<(PathBuf, Self)> {
        let mut current = from.to_path_buf();
        loop {
            let candidate = current.join(".aida-workspace");
            if candidate.exists() {
                if let Ok(content) = std::fs::read_to_string(&candidate) {
                    if let Ok(config) = toml::from_str::<WorkspaceConfig>(&content) {
                        return Some((current, config));
                    }
                }
            }
            if !current.pop() {
                return None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Deployment Mode
// ---------------------------------------------------------------------------

/// The deployment mode for an AIDA instance.
///
/// This is the top-level configuration that determines how IDs are generated,
/// how storage works, and whether distributed features are enabled.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode")]
#[derive(Default)]
pub enum DeploymentMode {
    /// Centralized: single PostgreSQL or SQLite database, simple sequential IDs.
    /// This is the default for teams with always-available connectivity.
    /// IDs: `FR-001`, `FEAT-042`
    #[serde(rename = "centralized")]
    #[default]
    Centralized,

    /// Distributed: git-based event log, node-namespaced IDs, offline-capable.
    /// IDs: `FR-7-001`, `FEAT-3-042` (with optional agreed IDs on trunk)
    #[serde(rename = "distributed")]
    Distributed {
        /// Path to the shared aida git repo (absolute or relative to workspace)
        #[serde(default)]
        aida_repo_path: Option<String>,
    },
}

impl DeploymentMode {
    /// Whether this mode uses node-namespaced IDs.
    pub fn is_distributed(&self) -> bool {
        matches!(self, Self::Distributed { .. })
    }

    /// Get the IdMode for the dispenser.
    /// In centralized mode, returns `IdMode::Centralized`.
    /// In distributed mode, requires a node_id (from NodeConfig).
    pub fn id_mode(&self, node_id: Option<&str>) -> IdMode {
        match self {
            Self::Centralized => IdMode::Centralized,
            Self::Distributed { .. } => IdMode::Distributed {
                node_id: node_id
                    .expect("distributed mode requires a node_id")
                    .to_string(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_registry_sequential_ids() {
        let mut registry = NodeRegistry::default();
        let id1 = registry.register(1, "laptop".into());
        let id2 = registry.register(1, "workstation".into());
        let id3 = registry.register(2, "alice-dev".into());
        assert_eq!(id1, "1");
        assert_eq!(id2, "2");
        assert_eq!(id3, "3");
        assert!(registry.is_registered("1"));
        assert!(!registry.is_registered("99"));
    }

    #[test]
    fn test_validate_node_id() {
        // trace:STORY-41 | ai:claude
        assert!(validate_node_id("JM").is_ok());
        assert!(validate_node_id("1").is_ok());
        assert!(validate_node_id("clone-2").is_ok());
        assert!(validate_node_id("alice_dev").is_ok());
        assert!(validate_node_id("J").is_ok());
        assert!(validate_node_id("").is_err());
        assert!(validate_node_id("-leading-dash").is_err());
        assert!(validate_node_id("has space").is_err());
        assert!(validate_node_id("has.dot").is_err());
        let too_long = "a".repeat(33);
        assert!(validate_node_id(&too_long).is_err());
    }

    #[test]
    fn test_next_free_with_prefix() {
        // trace:STORY-42 | ai:claude
        let mut registry = NodeRegistry::default();
        assert_eq!(registry.next_free_with_prefix("JM"), "JM");
        registry.register_specific("JM".into(), 1, "imac".into(), None);
        assert_eq!(registry.next_free_with_prefix("JM"), "JM2");
        registry.register_specific("JM2".into(), 1, "spock".into(), None);
        assert_eq!(registry.next_free_with_prefix("JM"), "JM3");
    }

    #[test]
    fn test_back_compat_numeric_id_deserializes() {
        // trace:STORY-41 | ai:claude
        // Pre-EPIC-9 nodes.toml wrote `id = 1`. Post-EPIC-9 reads it as "1".
        let toml_str = r#"
[[nodes]]
id = 1
user_id = 5
hostname = "imac"
registered = "2026-05-09T00:00:00Z"
"#;
        let registry: NodeRegistry = toml::from_str(toml_str).unwrap();
        assert_eq!(registry.nodes[0].id, "1");
        assert_eq!(registry.nodes[0].user_id, 5);
    }

    #[test]
    fn test_user_registry() {
        let mut registry = UserRegistry::default();
        let id1 = registry.register("Joe".into(), Some("joe@example.com".into()));
        let id2 = registry.register("Alice".into(), None);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert!(registry.find_by_name("joe").is_some());
        assert!(registry.find_by_name("JOE").is_some()); // case-insensitive
        assert!(registry.find_by_name("bob").is_none());
    }

    #[test]
    fn test_agreed_counters() {
        let mut counters = AgreedCounters::default();
        assert_eq!(counters.peek("FR"), 1);
        assert_eq!(counters.next("FR"), 1);
        assert_eq!(counters.next("FR"), 2);
        assert_eq!(counters.next("FEAT"), 1);
        assert_eq!(counters.peek("FR"), 3);

        assert_eq!(AgreedCounters::format_agreed_id("FR", 423), "FR-423");
    }

    #[test]
    fn test_deployment_mode_centralized() {
        let mode = DeploymentMode::Centralized;
        assert!(!mode.is_distributed());
        assert_eq!(mode.id_mode(None), IdMode::Centralized);
    }

    #[test]
    fn test_deployment_mode_distributed() {
        let mode = DeploymentMode::Distributed {
            aida_repo_path: Some("./aida".into()),
        };
        assert!(mode.is_distributed());
        assert_eq!(
            mode.id_mode(Some("7")),
            IdMode::Distributed {
                node_id: "7".to_string()
            }
        );
    }

    #[test]
    fn test_deployment_mode_serde_roundtrip() {
        let centralized = DeploymentMode::Centralized;
        let json = serde_json::to_string(&centralized).unwrap();
        assert_eq!(json, r#"{"mode":"centralized"}"#);

        let distributed = DeploymentMode::Distributed {
            aida_repo_path: Some("./aida".into()),
        };
        let json = serde_json::to_string(&distributed).unwrap();
        let back: DeploymentMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, distributed);
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_node_config_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.toml");

        let config = NodeConfig {
            node_id: "7".to_string(),
            user_id: 102,
            hostname: "joe-laptop".into(),
            email: Some("joe@example.com".into()),
            registered_at: Utc::now(),
        };
        config.save(&path).unwrap();

        let loaded = NodeConfig::load(&path).unwrap();
        assert_eq!(loaded.node_id, "7");
        assert_eq!(loaded.user_id, 102);
        assert_eq!(loaded.hostname, "joe-laptop");
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_node_registry_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nodes.toml");

        let mut registry = NodeRegistry::default();
        registry.register(102, "joe-laptop".into());
        registry.register(102, "joe-workstation".into());
        registry.save(&path).unwrap();

        let loaded = NodeRegistry::load(&path).unwrap();
        assert_eq!(loaded.nodes.len(), 2);
        assert_eq!(loaded.nodes[0].hostname, "joe-laptop");
        assert_eq!(loaded.nodes[1].hostname, "joe-workstation");
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_workspace_config_serde() {
        let config = WorkspaceConfig {
            workspace: "gdms-disruptive".into(),
            aida_path: "./aida".into(),
            repos: vec!["pacgate".into(), "pacinet".into()],
        };
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let back: WorkspaceConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(back.workspace, "gdms-disruptive");
        assert_eq!(back.repos.len(), 2);
    }

    #[test]
    fn test_block_registry_claim_and_dispense() {
        let mut registry = BlockRegistry::default();
        let block = registry.claim_block(
            "2".into(),
            "joe@work".into(),
            "workstation".into(),
            "FR".into(),
            100,
        );
        assert_eq!(block.range_start, 1);
        assert_eq!(block.range_end, 100);
        assert_eq!(block.next, 1);

        let (id, is_low) = registry.dispense("2", "FR").unwrap();
        assert_eq!(id, "FR-1");
        assert!(!is_low);

        let (id2, _) = registry.dispense("2", "FR").unwrap();
        assert_eq!(id2, "FR-2");
    }

    #[test]
    fn test_block_registry_next_range_start() {
        let mut registry = BlockRegistry::default();
        registry.claim_block(
            "1".into(),
            "joe@home".into(),
            "home".into(),
            "FR".into(),
            100,
        );
        registry.claim_block(
            "2".into(),
            "joe@work".into(),
            "work".into(),
            "FR".into(),
            100,
        );

        // home claims 1..100, work claims 101..200, next start should be 201
        assert_eq!(registry.next_range_start("FR"), 201);
        // BUG has no blocks yet
        assert_eq!(registry.next_range_start("BUG"), 1);
    }

    #[test]
    fn test_block_exhaustion() {
        let mut registry = BlockRegistry::default();
        registry.claim_block("1".into(), "a".into(), "h".into(), "FR".into(), 3);

        assert_eq!(registry.dispense("1", "FR").unwrap().0, "FR-1");
        assert_eq!(registry.dispense("1", "FR").unwrap().0, "FR-2");
        assert_eq!(registry.dispense("1", "FR").unwrap().0, "FR-3");
        assert!(registry.dispense("1", "FR").is_none()); // exhausted
        assert_eq!(registry.blocks[0].remaining(), 0);
        assert!(registry.blocks[0].is_exhausted());
    }

    #[test]
    fn test_block_low_threshold() {
        let mut registry = BlockRegistry::default();
        registry.claim_block("1".into(), "a".into(), "h".into(), "FR".into(), 15);

        // Consume until 5 remain — should trigger is_low
        for _ in 0..5 {
            registry.dispense("1", "FR");
        }
        // 10 remain at this point — exactly at threshold, is_low = true
        let (_, is_low) = registry.dispense("1", "FR").unwrap();
        assert!(is_low);
    }

    // trace:BUG-115 | ai:claude
    #[test]
    fn test_aggregate_remaining_sums_across_blocks() {
        let mut registry = BlockRegistry::default();
        registry.claim_block("1".into(), "a".into(), "h".into(), "BUG".into(), 100);
        registry.claim_block("1".into(), "a".into(), "h".into(), "BUG".into(), 100);
        // Drain the first block down to 3 remaining (97 dispensed).
        for _ in 0..97 {
            registry.dispense("1", "BUG");
        }
        // 3 remaining in lowest block + 100 in highest = 103 aggregate.
        assert_eq!(registry.aggregate_remaining("1", "BUG"), 103);
        // Lowest block alone is "low" per the per-block rule, but the
        // aggregate is well above the warning threshold — no nag.
        assert!(!registry.aggregate_is_low("1", "BUG"));
        assert_eq!(registry.active_block_count("1", "BUG"), 2);
    }

    // trace:BUG-115 | ai:claude
    #[test]
    fn test_aggregate_is_low_only_when_aggregate_dips() {
        let mut registry = BlockRegistry::default();
        registry.claim_block("1".into(), "a".into(), "h".into(), "BUG".into(), 15);
        registry.claim_block("1".into(), "a".into(), "h".into(), "BUG".into(), 15);
        // Drain both blocks down so aggregate falls to 20 (== threshold).
        // First block: dispense 10 → 5 remaining. Second untouched (15).
        for _ in 0..10 {
            registry.dispense("1", "BUG");
        }
        // 5 + 15 = 20, exactly the threshold → low.
        assert_eq!(registry.aggregate_remaining("1", "BUG"), 20);
        assert!(registry.aggregate_is_low("1", "BUG"));
    }

    // trace:BUG-115 | ai:claude
    #[test]
    fn test_aggregate_ignores_exhausted_blocks() {
        let mut registry = BlockRegistry::default();
        registry.claim_block("1".into(), "a".into(), "h".into(), "BUG".into(), 5);
        registry.claim_block("1".into(), "a".into(), "h".into(), "BUG".into(), 100);
        // Drain the first block to exhaustion (5 dispenses on a size-5 block).
        for _ in 0..5 {
            registry.dispense("1", "BUG");
        }
        assert!(registry.blocks[0].is_exhausted());
        // Aggregate counts only the still-active second block.
        assert_eq!(registry.aggregate_remaining("1", "BUG"), 100);
        assert_eq!(registry.active_block_count("1", "BUG"), 1);
        assert!(!registry.aggregate_is_low("1", "BUG"));
    }

    // trace:BUG-115 | ai:claude
    #[test]
    fn test_aggregate_zero_when_all_exhausted() {
        let mut registry = BlockRegistry::default();
        registry.claim_block("1".into(), "a".into(), "h".into(), "BUG".into(), 3);
        for _ in 0..3 {
            registry.dispense("1", "BUG");
        }
        assert_eq!(registry.aggregate_remaining("1", "BUG"), 0);
        assert_eq!(registry.active_block_count("1", "BUG"), 0);
        // `aggregate_is_low` is false when no active blocks remain — the
        // user gets a separate "exhausted, claim" message, not "low".
        assert!(!registry.aggregate_is_low("1", "BUG"));
    }

    // trace:BUG-115 | ai:claude
    #[test]
    fn test_aggregate_scopes_to_node_and_type() {
        let mut registry = BlockRegistry::default();
        registry.claim_block("1".into(), "a".into(), "h".into(), "BUG".into(), 50);
        registry.claim_block("2".into(), "b".into(), "h".into(), "BUG".into(), 50);
        registry.claim_block("1".into(), "a".into(), "h".into(), "FR".into(), 50);
        assert_eq!(registry.aggregate_remaining("1", "BUG"), 50);
        assert_eq!(registry.aggregate_remaining("2", "BUG"), 50);
        assert_eq!(registry.aggregate_remaining("1", "FR"), 50);
        // Case-insensitive on the type prefix.
        assert_eq!(registry.aggregate_remaining("1", "bug"), 50);
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_block_registry_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocks.yaml");

        let mut registry = BlockRegistry::default();
        registry.claim_block(
            "2".into(),
            "joe@work".into(),
            "work".into(),
            "FR".into(),
            100,
        );
        registry.save(&path).unwrap();

        let loaded = BlockRegistry::load(&path).unwrap();
        assert_eq!(loaded.blocks.len(), 1);
        assert_eq!(loaded.blocks[0].range_start, 1);
        assert_eq!(loaded.blocks[0].range_end, 100);
        assert_eq!(loaded.blocks[0].node_id, "2");
    }

    // BUG-474: two serialized load→dispense→save sequences under
    // `with_dispense_lock` must yield distinct ids. This is the unit-level
    // proof that the lock-wrapped read-modify-write advances the persisted
    // `next` pointer between dispenses rather than replaying it.
    #[cfg(feature = "native")]
    #[test]
    fn with_dispense_lock_serialized_dispenses_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocks.yaml");

        let mut registry = BlockRegistry::default();
        registry.claim_block("1".into(), "a".into(), "h".into(), "FR".into(), 100);
        registry.save(&path).unwrap();

        // Each closure mirrors the `aida add` call site: load, dispense, save.
        let dispense_once = |path: &Path| -> anyhow::Result<String> {
            BlockRegistry::with_dispense_lock(path, || {
                let mut reg = BlockRegistry::load(path)?;
                let (id, _is_low) = reg
                    .dispense("1", "FR")
                    .ok_or_else(|| anyhow::anyhow!("no active block"))?;
                reg.save(path)?;
                Ok(id)
            })
        };

        let first = dispense_once(&path).unwrap();
        let second = dispense_once(&path).unwrap();
        assert_eq!(first, "FR-1");
        assert_eq!(second, "FR-2");
        assert_ne!(first, second, "serialized dispenses replayed an id");
    }

    // BUG-474: concurrent-writer stress test mirroring the FileDispenser
    // concurrency test in dispenser.rs. N threads each run the full
    // load→dispense→save sequence under `with_dispense_lock` against the
    // SAME blocks file. The lock serializes the read-modify-write, so every
    // id handed out must be unique and the dispensed ids must cover a
    // contiguous range — a missing lock would let two threads both load the
    // same `next` and emit a duplicate stable id (BUG-474's exact failure).
    #[cfg(feature = "native")]
    #[test]
    fn concurrent_dispense_under_lock_allocates_unique_ids() {
        use std::sync::Arc;
        const THREADS: usize = 8;
        const PER_THREAD: usize = 25;
        const CAPACITY: u32 = (THREADS * PER_THREAD) as u32;

        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("blocks.yaml"));

        let mut registry = BlockRegistry::default();
        registry.claim_block("1".into(), "a".into(), "h".into(), "FR".into(), CAPACITY);
        registry.save(&path).unwrap();

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let path = Arc::clone(&path);
                std::thread::spawn(move || {
                    (0..PER_THREAD)
                        .map(|_| {
                            BlockRegistry::with_dispense_lock(&path, || {
                                let mut reg = BlockRegistry::load(&path)?;
                                let (id, _is_low) = reg
                                    .dispense("1", "FR")
                                    .ok_or_else(|| anyhow::anyhow!("exhausted"))?;
                                reg.save(&path)?;
                                Ok(id)
                            })
                            .unwrap()
                        })
                        .collect::<Vec<String>>()
                })
            })
            .collect();

        let mut ids: Vec<String> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        let total = ids.len();
        assert_eq!(total, THREADS * PER_THREAD);

        ids.sort();
        ids.dedup();
        assert_eq!(
            ids.len(),
            total,
            "dispense handed out duplicate agreed-ids under concurrency"
        );

        // The block (1..=CAPACITY) is fully consumed and the persisted file
        // still parses — no torn write replayed or lost a counter.
        let mut reopened = BlockRegistry::load(&path).unwrap();
        assert!(reopened.blocks[0].is_exhausted());
        assert!(reopened.dispense("1", "FR").is_none());
    }
}
