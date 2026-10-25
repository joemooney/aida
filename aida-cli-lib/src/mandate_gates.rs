//! STORY-1700 (ADR-77): the human gates a drive mandate never acts past.
//!
//! `[mandate.human_gates] ops = [...]` lists the actions that stay human
//! even under a mandate; it is empty by default (gates are opt-in). Agents
//! must not be able to loosen them (R1), so the layers are:
//!
//! - the COMMITTED layer, read from the `origin/main` blob of
//!   `.aida/config.toml` — never the working tree, so an edit on a branch or
//!   an uncommitted edit in the main checkout changes nothing;
//! - the OPERATOR layer, `~/.aida/config.toml`.
//!
//! The result is their union: a layer can only add gates. An unreadable
//! committed layer is an `Err`, which every caller treats as "every op
//! gated" (fail closed). [`GATE_PROTECTED_PATHS`] names the files a PR may
//! not touch and still ship under a mandate; `protected_paths = [...]` in
//! either layer adds to it, never removes from it (STORY-1888).
// trace:STORY-1700 | ai:claude
// mandate-gate

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

/// An action a human gate can keep human.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum GateOp {
    Ship,
    SketchSignoff,
    Dispatch,
    /// Listed so an operator can state it; drive never clears holds.
    HoldClear,
    /// The drive tick's restart of a crashed relay.
    // trace:STORY-1706 | ai:claude
    RelayRestart,
}

impl GateOp {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ship => "ship",
            Self::SketchSignoff => "sketch-signoff",
            Self::Dispatch => "dispatch",
            Self::HoldClear => "hold-clear",
            Self::RelayRestart => "relay-restart",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        [
            Self::Ship,
            Self::SketchSignoff,
            Self::Dispatch,
            Self::HoldClear,
            Self::RelayRestart,
        ]
        .into_iter()
        .find(|op| op.as_str() == raw.trim())
    }
}

/// The union of both layers.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HumanGates {
    pub ops: BTreeSet<GateOp>,
    /// `[mandate.drive] max_actions_per_tick`, honoured from the operator
    /// layer only.
    pub max_actions_per_tick: Option<usize>,
    /// The floor plus both layers' `protected_paths` (STORY-1888).
    pub protected: ProtectedPaths,
}

impl HumanGates {
    pub(crate) fn gates(&self, op: GateOp) -> bool {
        self.ops.contains(&op)
    }

    pub(crate) fn describe(&self) -> String {
        if self.ops.is_empty() {
            "none".into()
        } else {
            self.ops
                .iter()
                .map(|op| op.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
}

/// One parsed layer.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct GateLayer {
    pub ops: BTreeSet<GateOp>,
    pub max_actions_per_tick: Option<usize>,
    /// `[mandate.human_gates] protected_paths`, validated by
    /// [`parse_protected_entry`].
    pub protected_paths: BTreeSet<String>,
}

/// Parse the `[mandate]` table of one config layer. An unknown op, an
/// unknown key under `[mandate]`, `[mandate.human_gates]` or `[mandate.drive]`,
/// or a mistyped value is an error.
// trace:STORY-1700 | ai:claude
pub(crate) fn parse_layer(text: &str, source: &str) -> Result<GateLayer> {
    let value: toml::Value =
        toml::from_str(text).with_context(|| format!("{source} is not valid TOML"))?;
    let mut layer = GateLayer::default();
    let Some(mandate) = value.get("mandate") else {
        return Ok(layer);
    };
    let mandate = mandate
        .as_table()
        .with_context(|| format!("{source}: [mandate] must be a table"))?;
    for (key, section) in mandate {
        let table = section
            .as_table()
            .with_context(|| format!("{source}: [mandate.{key}] must be a table"))?;
        match key.as_str() {
            "human_gates" => {
                for (k, v) in table {
                    match k.as_str() {
                        "ops" => {
                            let ops = v.as_array().with_context(|| {
                                format!("{source}: [mandate.human_gates] ops must be a list")
                            })?;
                            for op in ops {
                                let raw = op.as_str().with_context(|| {
                                    format!(
                                        "{source}: [mandate.human_gates] ops holds a non-string"
                                    )
                                })?;
                                let Some(op) = GateOp::parse(raw) else {
                                    bail!(
                                        "{source}: [mandate.human_gates] unknown op `{raw}` (known: ship, sketch-signoff, dispatch, hold-clear, relay-restart)"
                                    );
                                };
                                layer.ops.insert(op);
                            }
                        }
                        // trace:STORY-1888 | ai:claude
                        "protected_paths" => {
                            let entries = v.as_array().with_context(|| {
                                format!(
                                    "{source}: [mandate.human_gates] protected_paths must be a list"
                                )
                            })?;
                            for entry in entries {
                                let raw = entry.as_str().with_context(|| {
                                    format!(
                                        "{source}: [mandate.human_gates] protected_paths holds a non-string"
                                    )
                                })?;
                                let entry = parse_protected_entry(raw).with_context(|| {
                                    format!(
                                        "{source}: [mandate.human_gates] protected_paths entry `{raw}`"
                                    )
                                })?;
                                layer.protected_paths.insert(entry);
                            }
                        }
                        other => bail!("{source}: [mandate.human_gates] unknown key `{other}`"),
                    }
                }
            }
            "drive" => {
                for (k, v) in table {
                    match k.as_str() {
                        "max_actions_per_tick" => {
                            let n = v.as_integer().filter(|n| *n > 0).with_context(|| {
                                format!(
                                    "{source}: [mandate.drive] max_actions_per_tick must be a positive integer"
                                )
                            })?;
                            layer.max_actions_per_tick = Some(n as usize);
                        }
                        other => bail!("{source}: [mandate.drive] unknown key `{other}`"),
                    }
                }
            }
            other => bail!("{source}: unknown table [mandate.{other}]"),
        }
    }
    Ok(layer)
}

/// The union of the two layers. Only the operator layer sets the budget.
// trace:STORY-1700 | ai:claude
pub(crate) fn union(committed: GateLayer, operator: GateLayer) -> HumanGates {
    let mut ops = committed.ops;
    ops.extend(operator.ops);
    let mut protected = ProtectedPaths::floor();
    protected.extend(PathSource::Committed, committed.protected_paths);
    protected.extend(PathSource::Operator, operator.protected_paths);
    HumanGates {
        ops,
        max_actions_per_tick: operator.max_actions_per_tick,
        protected,
    }
}

const COMMITTED_REF: &str = "origin/main";
const CONFIG_PATH: &str = ".aida/config.toml";

/// The committed layer: `origin/main:.aida/config.toml`. `Err` when the ref
/// cannot be read; a ref whose tree has no config file is an empty layer.
// trace:STORY-1700 | ai:claude
pub(crate) fn committed_layer(project_root: &Path) -> Result<GateLayer> {
    let has_ref = aida_core::git_ops::host_git_command(project_root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{COMMITTED_REF}^{{commit}}"))
        .output()
        .context("git failed to run")?;
    if !has_ref.status.success() {
        bail!("no {COMMITTED_REF} ref to read the committed gate layer from");
    }
    let listed = aida_core::git_ops::host_git_command(project_root)
        .args(["ls-tree", "--name-only", COMMITTED_REF, "--", CONFIG_PATH])
        .output()
        .context("git failed to run")?;
    if !listed.status.success() {
        bail!(
            "cannot list {COMMITTED_REF}:{CONFIG_PATH}: {}",
            String::from_utf8_lossy(&listed.stderr).trim()
        );
    }
    if String::from_utf8_lossy(&listed.stdout).trim().is_empty() {
        return Ok(GateLayer::default());
    }
    let text = crate::history::git_show_blob(project_root, COMMITTED_REF, CONFIG_PATH)?;
    parse_layer(&text, &format!("{COMMITTED_REF}:{CONFIG_PATH}"))
}

/// The operator layer: `~/.aida/config.toml`; absent = empty.
// trace:STORY-1700 | ai:claude
pub(crate) fn operator_layer() -> Result<GateLayer> {
    let home = crate::aida_home_dir().context("cannot locate the user's home directory")?;
    let path = home.join(".aida").join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_layer(&text, &path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GateLayer::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Both layers, unioned. `Err` = treat every op as gated.
// trace:STORY-1700 | ai:claude
pub(crate) fn load_human_gates(project_root: &Path) -> Result<HumanGates> {
    Ok(union(committed_layer(project_root)?, operator_layer()?))
}

/// Why `op` is gated, if it is. `Err` gates everything.
// trace:STORY-1700 | ai:claude
pub(crate) fn gate_reason(
    gates: &std::result::Result<HumanGates, String>,
    op: GateOp,
) -> Option<String> {
    match gates {
        Err(why) => Some(format!("gate config unreadable: {why}")),
        Ok(g) if g.gates(op) => Some(format!(
            "`{}` is listed in [mandate.human_gates]",
            op.as_str()
        )),
        Ok(_) => None,
    }
}

const UNREADABLE: &str = "unreadable (every action gated)";
const DESCRIBE_CACHE: &str = "mandate-gates-cache.json";

/// The `origin/main` commit id, read from the ref files without spawning
/// git when it can be (the per-turn notice's latency budget), else through
/// one `git rev-parse`.
// trace:STORY-1700 | ai:claude
pub(crate) fn origin_main_oid(project_root: &Path) -> Option<String> {
    let dot_git = project_root.join(".git");
    let git_dir = if dot_git.is_dir() {
        Some(dot_git)
    } else {
        std::fs::read_to_string(&dot_git)
            .ok()
            .and_then(|t| {
                t.trim()
                    .strip_prefix("gitdir:")
                    .map(|p| p.trim().to_string())
            })
            .map(|p| project_root.join(p))
    };
    let common = git_dir.map(|d| match std::fs::read_to_string(d.join("commondir")) {
        Ok(rel) => d.join(rel.trim()),
        Err(_) => d,
    });
    let is_oid = |s: &str| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit());
    if let Some(common) = common {
        if let Ok(text) = std::fs::read_to_string(common.join("refs/remotes/origin/main")) {
            let oid = text.trim();
            if is_oid(oid) {
                return Some(oid.to_string());
            }
        }
        if let Ok(packed) = std::fs::read_to_string(common.join("packed-refs")) {
            if let Some(oid) = packed.lines().find_map(|l| {
                l.strip_suffix(" refs/remotes/origin/main")
                    .filter(|o| is_oid(o))
            }) {
                return Some(oid.to_string());
            }
        }
    }
    let out = aida_core::git_ops::host_git_command(project_root)
        .args(["rev-parse", "--verify", "--quiet", "origin/main^{commit}"])
        .output()
        .ok()?;
    let oid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && is_oid(&oid)).then_some(oid)
}

/// What a cached description is keyed on: the committed layer's commit and
/// the operator layer's modification time.
fn describe_key(project_root: &Path) -> Option<String> {
    let oid = origin_main_oid(project_root)?;
    let home = crate::aida_home_dir()?.join(".aida").join("config.toml");
    let mtime = match std::fs::metadata(&home) {
        Ok(meta) => meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos()
            .to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "absent".into(),
        Err(_) => return None,
    };
    Some(format!("{oid}:{mtime}"))
}

/// The human gates in force, for display. Cached in
/// `.aida/mandate-gates-cache.json` keyed on [`describe_key`], so the
/// per-turn notice spawns no git while neither layer changes; an unreadable
/// layer is never cached.
// trace:STORY-1700 | ai:claude
pub(crate) fn describe(main_worktree: Option<&Path>) -> String {
    let Some(root) = main_worktree else {
        return UNREADABLE.into();
    };
    let key = describe_key(root);
    let cache = root.join(".aida").join(DESCRIBE_CACHE);
    if let Some(key) = &key {
        let hit = std::fs::read(&cache)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .filter(|v| v["key"].as_str() == Some(key.as_str()))
            .and_then(|v| v["describe"].as_str().map(str::to_string));
        if let Some(text) = hit {
            return text;
        }
    }
    match load_human_gates(root) {
        Ok(g) => {
            let text = g.describe();
            if let Some(key) = key {
                let body = serde_json::json!({ "key": key, "describe": text });
                let tmp = cache.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
                if std::fs::write(&tmp, body.to_string()).is_ok() {
                    let _ = std::fs::rename(&tmp, &cache);
                }
            }
            text
        }
        Err(_) => UNREADABLE.into(),
    }
}

/// Files a PR may not change and still ship under a mandate (R1, condition
/// C1). `.aida/config.toml` counts whole: the gate table lives in it and a
/// path filter cannot see which table a hunk touches. This floor is never
/// read from config, so no layer can shrink it (STORY-1888).
// trace:STORY-1700 | ai:claude
pub(crate) const GATE_PROTECTED_PATHS: &[&str] = &[
    ".aida/config.toml",
    "aida-cli-lib/src/dispatch.rs",
    "aida-cli-lib/src/dispatch_flow.rs",
    "aida-cli-lib/src/mandate.rs",
    "aida-cli-lib/src/mandate_gates.rs",
    "aida-cli-lib/src/mandate_drive.rs",
    "aida-cli-lib/src/mandate_ship.rs",
    "aida-cli-lib/src/mandate_traces.rs",
    "aida-cli-lib/src/relay_launch.rs",
    "aida-cli-lib/src/rework_hold_release.rs",
    "aida-cli-lib/src/fleet_lanes.rs",
    "aida-cli-lib/src/pr_ship.rs",
    // STORY-1887: the batch that drives human-gated merges. trace:STORY-1887 | ai:claude
    "aida-cli-lib/src/pr_ship_batch.rs",
    // Gate authority, code and its tests together (STORY-1889).
    "aida-cli-lib/src/gate/",
    // The drift ratchet that keeps this list complete (STORY-1888 C1).
    "aida-cli-lib/src/tests/story_1888_gate_paths_tests.rs",
];

/// STORY-1888: gate-authority files the drift ratchet finds outside
/// [`GATE_PROTECTED_PATHS`] today. This list may only shrink: the ratchet
/// fails when an entry stops looking like gate authority, so code moved
/// into the floor takes its line with it. STORY-1889 burns it to zero.
// trace:STORY-1888 | ai:claude
pub(crate) const KNOWN_UNPROTECTED: &[&str] = &[
    // Gate authority today (approval, merge and hold refusals).
    "aida-cli-lib/src/pr_cmd.rs",
    "aida-cli-lib/src/pr_merge_gate.rs",
    // A name match that reads lease state rather than decides; STORY-1889
    // confirms it before it moves or is exempted.
    "aida-cli-lib/src/session_reap.rs",
];

/// STORY-1889: functions whose NAME reads as gate authority but which decide
/// no ship, merge, hold, verdict or seat outcome: `(path, fn, reason)`. The
/// drift ratchet skips exactly these functions, so a new matching function
/// in the same file is still caught. Shrink-only: an entry whose function no
/// longer exists, or no longer matches, fails the ratchet. Growing it needs
/// a human merge (this file is on the floor).
// trace:STORY-1889 | ai:claude
pub(crate) const NOT_GATE_AUTHORITY: &[(&str, &str, &str)] = &[
    (
        "aida-core/src/git_ops.rs",
        "merge_gate",
        "assigns agreed store ids on merge; no ship or merge authority",
    ),
    (
        "aida-cli-lib/src/human.rs",
        "pr_review_decision",
        "classifies a PR's review state for the awaiting view; display only",
    ),
    (
        "aida-cli-lib/src/status_prs.rs",
        "detect_pr_review_decision",
        "probes the forge's review decision for display",
    ),
    (
        "aida-cli-lib/src/leases.rs",
        "review_source_branch_conflict_decision",
        "lease coexistence on reviewer pickup; reviewer independence is decided by identity in review_authority.rs",
    ),
    (
        "aida-cli-lib/src/session_start.rs",
        "review_pickup_conflict_decision",
        "lease coexistence on reviewer pickup; reviewer independence is decided by identity in review_authority.rs",
    ),
    (
        "aida-cli-lib/src/pr_cmd.rs",
        "pr_head_sha_for_merge_gate",
        "reads the PR head sha from the forge; the gate that uses it lives in gate/",
    ),
];

/// STORY-1889: unprotected files that may re-export `crate::gate` items while
/// the STORY-1889 moves land. A re-export outside the floor could later be
/// repointed at a non-gate function, so the ratchet refuses any other, and
/// this list only shrinks; the final STORY-1889 PR empties it.
// trace:STORY-1889 | ai:claude
pub(crate) const TRANSITIONAL_GATE_REEXPORTS: &[&str] = &[
    "aida-cli-lib/src/defer_cmd.rs",
    "aida-cli-lib/src/lib.rs",
    "aida-cli-lib/src/merge_hold.rs",
    "aida-cli-lib/src/role.rs",
];

/// Where a protected path comes from, strongest first.
// trace:STORY-1888 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PathSource {
    BuiltIn,
    Committed,
    Operator,
}

impl PathSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Committed => "origin/main:.aida/config.toml",
            Self::Operator => "~/.aida/config.toml",
        }
    }
}

/// One protected entry: an exact path, or a directory prefix ending in `/`.
// trace:STORY-1888 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtectedEntry {
    pub pattern: String,
    pub source: PathSource,
}

impl ProtectedEntry {
    /// Match on a path-component boundary: `foo/` matches `foo/x.rs` and
    /// never `foobar/x.rs` or `foo`; an entry without a trailing `/` matches
    /// the exact path and any file under that directory (`foo/x.rs`), never
    /// `foobar/x.rs` or `bar.rs.bak` (TASK-2299).
    // trace:TASK-2168 trace:TASK-2299 | ai:agy
    fn matches(&self, path: &str) -> bool {
        if self.pattern.ends_with('/') {
            path.starts_with(&self.pattern)
        } else {
            path == self.pattern
                || path
                    .strip_prefix(&self.pattern)
                    .is_some_and(|rest| rest.starts_with('/'))
        }
    }
}

/// The effective protected set: the floor, then each layer's additions.
/// There is no way to remove an entry, and the floor is always seeded first,
/// so a layer listing a floor path leaves it `BuiltIn`.
// trace:STORY-1888 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtectedPaths {
    entries: Vec<ProtectedEntry>,
}

impl Default for ProtectedPaths {
    fn default() -> Self {
        Self::floor()
    }
}

impl ProtectedPaths {
    pub(crate) fn floor() -> Self {
        Self {
            entries: GATE_PROTECTED_PATHS
                .iter()
                .map(|p| ProtectedEntry {
                    pattern: (*p).to_string(),
                    source: PathSource::BuiltIn,
                })
                .collect(),
        }
    }

    /// The floor, shared (for callers and tests that need a `'static` set).
    pub(crate) fn floor_ref() -> &'static Self {
        static FLOOR: std::sync::OnceLock<ProtectedPaths> = std::sync::OnceLock::new();
        FLOOR.get_or_init(Self::floor)
    }

    pub(crate) fn extend(
        &mut self,
        source: PathSource,
        patterns: impl IntoIterator<Item = String>,
    ) {
        for pattern in patterns {
            if !self.entries.iter().any(|e| e.pattern == pattern) {
                self.entries.push(ProtectedEntry { pattern, source });
            }
        }
    }

    pub(crate) fn entries(&self) -> &[ProtectedEntry] {
        &self.entries
    }

    /// The protected set of a gate load; an unreadable load still checks
    /// the floor (fail closed).
    pub(crate) fn of<E>(gates: &std::result::Result<HumanGates, E>) -> Self {
        gates
            .as_ref()
            .map(|g| g.protected.clone())
            .unwrap_or_else(|_| Self::floor())
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        self.entries
            .iter()
            .map(|e| serde_json::json!({ "path": e.pattern, "source": e.source.as_str() }))
            .collect()
    }
}

/// Validate one `protected_paths` entry: a relative, forward-slash path with
/// no `..` segment and no glob characters. A leading `./` is dropped.
// trace:STORY-1888 | ai:claude
pub(crate) fn parse_protected_entry(raw: &str) -> Result<String> {
    let entry = raw.trim();
    let entry = entry.strip_prefix("./").unwrap_or(entry);
    if entry.is_empty() || entry == "/" {
        bail!("is empty");
    }
    if entry.starts_with('/') || entry.contains(':') {
        bail!("must be relative to the repository root");
    }
    if entry.contains('\\') {
        bail!("must use `/` separators");
    }
    if entry.contains(['*', '?', '[', ']']) {
        bail!("globs are not supported; use an exact path or a `dir/` prefix");
    }
    // At most one trailing `/` marks a prefix; `foo//` keeps an empty
    // segment and is refused (it could never match a normalised path).
    if entry
        .strip_suffix('/')
        .unwrap_or(entry)
        .split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        bail!("has an empty, `.` or `..` segment");
    }
    Ok(entry.to_string())
}

/// The protected entry a PR path hits.
// trace:STORY-1888 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateMatch {
    pub path: String,
    pub entry: String,
    pub source: PathSource,
}

impl GateMatch {
    /// "`path`, protected by `entry` (source)", for refusal messages.
    pub(crate) fn describe(&self) -> String {
        if self.entry == self.path {
            format!("`{}` (protected, {})", self.path, self.source.as_str())
        } else {
            format!(
                "`{}`, protected by `{}` ({})",
                self.path,
                self.entry,
                self.source.as_str()
            )
        }
    }
}

/// The first path in `paths` that `protected` covers, if any.
// trace:STORY-1700 | ai:claude
// trace:STORY-1888 | ai:claude
pub(crate) fn touches_gates(paths: &[String], protected: &ProtectedPaths) -> Option<GateMatch> {
    paths
        .iter()
        .map(|p| p.trim().trim_start_matches("./"))
        .find_map(|p| {
            protected
                .entries
                .iter()
                .find(|e| e.matches(p))
                .map(|e| GateMatch {
                    path: p.to_string(),
                    entry: e.pattern.clone(),
                    source: e.source,
                })
        })
}

/// `aida mandate status --json` `protected_paths`: the effective set with
/// each entry's source, or `{"error": ...}` (then only the floor is known
/// to hold, and every op is gated).
// trace:STORY-1888 | ai:claude
pub(crate) fn protected_paths_json(project_root: &Path) -> serde_json::Value {
    match load_human_gates(project_root) {
        Ok(g) => g.protected.to_json(),
        Err(e) => serde_json::json!({
            "error": format!("{e:#}"),
            "floor": ProtectedPaths::floor().to_json(),
        }),
    }
}

/// The `aida mandate status` block listing the effective protected set.
// trace:STORY-1888 | ai:claude
pub(crate) fn protected_paths_text(project_root: &Path) -> String {
    let (set, note) = match load_human_gates(project_root) {
        Ok(g) => (g.protected, String::new()),
        Err(e) => (
            ProtectedPaths::floor(),
            format!(" (gate config unreadable, built-in floor only: {e:#})"),
        ),
    };
    let mut out = format!("  protected paths{note}:\n");
    for e in set.entries() {
        out.push_str(&format!("    {}  [{}]\n", e.pattern, e.source.as_str()));
    }
    out
}

#[cfg(test)]
#[path = "tests/story_1888_gate_paths_tests.rs"]
mod story_1888_gate_paths_tests;
