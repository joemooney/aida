//! distributed store detection and attach, store sync and ID allocation config (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreAutoPushMode {
    Manual,
    SessionEnd,
    PerWrite,
    Periodic,
}

impl StoreAutoPushMode {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(Self::Manual),
            "session-end" | "session_end" | "sessionend" => Some(Self::SessionEnd),
            "per-write" | "per_write" | "perwrite" => Some(Self::PerWrite),
            "periodic" => Some(Self::Periodic),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::SessionEnd => "session-end",
            Self::PerWrite => "per-write",
            Self::Periodic => "periodic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoreSyncConfig {
    pub(crate) auto_push: StoreAutoPushMode,
    pub(crate) periodic_threshold: Option<u64>,
    pub(crate) periodic_interval: Option<String>,
    // Extra remotes to mirror the store push to after `origin` succeeds — the
    // drift-prevention fan-out. Best-effort: a non-ff / unreachable mirror leg
    // warns, never fails the sync. trace:TASK-1096 | ai:claude
    pub(crate) mirror_remotes: Vec<String>,
    pub(crate) source: String,
}

impl Default for StoreSyncConfig {
    fn default() -> Self {
        Self {
            auto_push: StoreAutoPushMode::Manual,
            periodic_threshold: None,
            periodic_interval: None,
            mirror_remotes: Vec::new(),
            source: "default".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoreAllocationConfig {
    pub(crate) retry_max: usize,
}

impl Default for StoreAllocationConfig {
    fn default() -> Self {
        Self { retry_max: 3 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpecIdCollision {
    pub(crate) spec_id: String,
    pub(crate) claimants: Vec<SpecIdClaimant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpecIdClaimant {
    pub(crate) uuid: Uuid,
    pub(crate) title: String,
}

/// Detect if the current directory has a distributed store configured.
/// Walks up from CWD looking for `.aida/config.toml` with a store_path.
///
/// `AIDA_STORE` overrides everything: when it points at an existing
/// git-canonical store directory (one containing `objects/`), the whole CLI
/// (and the MCP server, which shares this resolver) retargets there instead of
/// walking up from CWD. This is the throwaway dev-playground / sandbox path
/// (SPIKE-48) — run drains and test scenarios against a discardable store
/// without touching the project's real `aida-store` orphan branch. The pointed
/// directory must already exist and look like a store; a missing or malformed
/// `AIDA_STORE` is ignored so a stale export never silently writes to a
/// half-formed location. trace:SPIKE-48 | ai:claude
/// trace:BUG-57 | ai:claude
pub(crate) fn detect_distributed_store() -> Option<std::path::PathBuf> {
    if let Some(store) = aida_store_override() {
        return Some(store);
    }
    let cwd = std::env::current_dir().ok()?;
    detect_distributed_store_from(&cwd)
}

/// Run `command` against a resolved git-canonical store directory. The MCP
/// server and the Jira / GitHub / GitLab tracker commands take a `Storage`;
/// pointing it at the store directory makes its load/save delegate to
/// GitBackend, so their reads and writes land in `objects/` like every other
/// command's. Everything else goes through the git-backend dispatcher.
///
/// `project_root_hint`, when given, is used as `mcp-serve`'s project root
/// instead of walking cwd up to a `.git` — the explicit `--file <dir>` caller
/// passes its own directory, since that directory IS the project root there
/// (no separate project to find, and cwd may be unrelated to it entirely).
/// The other callers pass `None` and keep the existing cwd-walk behavior.
// trace:BUG-310 trace:TASK-1486 trace:TASK-1487 | ai:claude
pub(crate) fn run_on_distributed_store(
    command: &Command,
    store_path: &std::path::Path,
    project_root_hint: Option<&std::path::Path>,
) -> Result<()> {
    let storage = Storage::new(store_path);
    match command {
        // Immutable corpus reads bypass cache/dispenser setup.
        // trace:TASK-1729 | ai:codex
        Command::Regen(cmd) => crate::regen::handle_command(store_path, cmd),
        // MCP server reads/writes through the same canonical git store the CLI
        // uses. The previous YAML-snapshot approach wrote to
        // `.aida/mcp-cache.yaml` only — invisible to the CLI and overwritten on
        // every MCP restart. trace:BUG-310 | ai:claude
        Command::McpServe => {
            let project_root =
                mcp_serve_project_root(store_path, project_root_hint, find_project_root().ok());
            mcp::run_mcp_server(&storage, project_root)
        }
        // Tracker imports (`aida jira/github pull`) write new requirements
        // through this `Storage`, same as every other distributed-store
        // write path — they need the same node id and per-write auto-push
        // `handle_git_backend_command` gives its own callers, just reached
        // through a different signature (`&Storage`, not `&GitBackend`).
        // trace:TASK-1487 | ai:claude
        Command::Jira(cmd) => run_tracker_command(store_path, command, |storage| {
            tracker_cmd::handle_jira_command(cmd, storage)
        }),
        Command::Github(cmd) => run_tracker_command(store_path, command, |storage| {
            tracker_cmd::handle_github_command(cmd, storage)
        }),
        Command::Gitlab(cmd) => run_tracker_command(store_path, command, |storage| {
            tracker_cmd::handle_gitlab_command(cmd, storage)
        }),
        _ => git_backend_cmd::handle_git_backend_command(store_path, command),
    }
}

/// Pure decision for `mcp-serve`'s project root: an explicit hint (the
/// `--file <dir>` caller's own directory) always wins over a cwd-derived git
/// root, since cwd may be unrelated to — or simply not inside — the
/// directory the user explicitly pointed at. Falls back to the store path
/// itself only when neither resolves. Takes the cwd-walk result as a plain
/// value (rather than calling `find_project_root()` itself) so this decision
/// stays unit-testable without touching the process's actual cwd.
///
/// The hint is a STORE path (the `--file`/`AIDA_STORE` convention: it points
/// at `<repo>/.aida-store`, the directory `GitBackend::new` expects
/// `objects/` directly under — see the ~20 `project_root.join(".aida-store")`
/// call sites), not the project root itself. Adopting it as-is regressed
/// leases, roles, mailbox and ledger paths (all keyed off the real project
/// root) and double-nested the store. `project_root_from_store_hint` derives
/// the real root from it instead.
// trace:TASK-1487 | ai:claude
pub(crate) fn mcp_serve_project_root(
    store_path: &std::path::Path,
    project_root_hint: Option<&std::path::Path>,
    cwd_project_root: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    project_root_hint
        .map(project_root_from_store_hint)
        .or(cwd_project_root)
        .unwrap_or_else(|| store_path.to_path_buf())
}

/// Derive the project root a `--file <dir>`/`AIDA_STORE` hint implies, per
/// the store-path convention:
/// 1. The hint's final component is `.aida-store` → its parent is the root
///    (the overwhelmingly common case: `--file` pointed straight at the
///    worktree, the same as every other resolver in this codebase).
/// 2. Otherwise the hint already looks like a project root itself (it has
///    its own `.git` or `.aida/config.toml`) → use it as-is.
/// 3. Otherwise walk up from the hint looking for an enclosing project root,
///    guarded — like every other `.aida`-seeking walk-up in this codebase —
///    against ever adopting a shared system temp root (BUG-1598).
/// 4. Otherwise (a bare sandbox store — e.g. the `AIDA_STORE` dev-playground
///    path, SPIKE-48 — with no enclosing project) the hint has no project
///    root to point to; keep it as-is.
// trace:TASK-1487 | ai:claude
pub(crate) fn project_root_from_store_hint(hint: &std::path::Path) -> std::path::PathBuf {
    project_root_from_store_hint_with_roots(hint, &aida_core::store_locate::real_temp_roots())
}

/// [`project_root_from_store_hint`], parameterized on the temp roots to
/// guard against, so a test can exercise the guard against a fake root
/// without touching the real, shared system temp dir.
// trace:TASK-1487 | ai:claude
pub(crate) fn project_root_from_store_hint_with_roots(
    hint: &std::path::Path,
    temp_roots: &[std::path::PathBuf],
) -> std::path::PathBuf {
    if hint.file_name() == Some(std::ffi::OsStr::new(".aida-store")) {
        if let Some(parent) = hint.parent() {
            return parent.to_path_buf();
        }
    }
    if looks_like_project_root(hint) {
        return hint.to_path_buf();
    }
    let canonical_roots = aida_core::store_locate::canonicalize_roots(temp_roots);
    let mut current = hint.parent();
    while let Some(dir) = current {
        // BUG-1598: never walk INTO a temp root and adopt it (or whatever's
        // in it) as the project — same guard every other walk-up here uses.
        if aida_core::store_locate::is_in_canonical_roots(dir, &canonical_roots) {
            break;
        }
        if looks_like_project_root(dir) {
            return dir.to_path_buf();
        }
        current = dir.parent();
    }
    hint.to_path_buf()
}

/// Does `dir` look like an AIDA/git project root on its own — a `.git`
/// directory or an `.aida/config.toml` file? Shared by both the direct-hint
/// check and the walk-up in [`project_root_from_store_hint_with_roots`].
// trace:TASK-1487 | ai:claude
pub(crate) fn looks_like_project_root(dir: &std::path::Path) -> bool {
    dir.join(".git").exists() || dir.join(".aida").join("config.toml").exists()
}

/// Run a tracker (jira/github/gitlab) command against the distributed store
/// with the same two things `handle_git_backend_command` gives every other
/// distributed-store command — reused here, not reimplemented, since tracker
/// commands take a `&Storage` and can't go through that dispatcher's match:
/// - the dispenser's node id, so oplog entries this command writes (a tracker
///   pull's bulk import) aren't stamped node_id "0";
/// - the per-write store auto-push, gated the same way
///   `command_triggers_per_write_auto_push` gates every other write command,
///   and only fired on success (an error return skips it, same as the
///   git-backend dispatcher's post-match auto-push never running on an early
///   `?` return from inside its match).
// trace:TASK-1487 | ai:claude
pub(crate) fn run_tracker_command(
    store_path: &std::path::Path,
    command: &Command,
    handler: impl FnOnce(&Storage) -> Result<()>,
) -> Result<()> {
    let storage = Storage::new(store_path).with_dispenser(load_dispenser(store_path)?);
    handler(&storage)?;
    if command_triggers_per_write_auto_push(command) {
        maybe_auto_push_store(store_path, StoreAutoPushMode::PerWrite, "per-write");
    }
    Ok(())
}

/// Where `aida init --refresh` seeds missing type protocols.
// trace:TASK-1486 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefreshSeedTarget {
    /// A resolved store: the distributed store, or a legacy store in a project
    /// that is not distributed.
    Store(std::path::PathBuf),
    /// The project is distributed but its store isn't attached here. Seeding
    /// a legacy `requirements.db` would write into stale data.
    DistributedUnattached,
    /// No store resolves at all.
    NoStore,
}

/// Pure decision for [`RefreshSeedTarget`]. The legacy resolver runs only when
/// the project is not distributed, so a stale local `requirements.db` in a
/// distributed project is never chosen.
// trace:TASK-1486 | ai:claude
pub(crate) fn refresh_seed_target(
    distributed_store: Option<std::path::PathBuf>,
    distributed_unattached: bool,
    legacy: impl FnOnce() -> Result<std::path::PathBuf>,
) -> RefreshSeedTarget {
    if let Some(store) = distributed_store {
        return RefreshSeedTarget::Store(store);
    }
    if distributed_unattached {
        return RefreshSeedTarget::DistributedUnattached;
    }
    legacy()
        .map(RefreshSeedTarget::Store)
        .unwrap_or(RefreshSeedTarget::NoStore)
}

/// The project root when `start` is inside a distributed AIDA project: one
/// whose `.aida/config.toml` declares distributed mode, whose git repo has an
/// `aida-store` branch, or whose store is physically attached at
/// `<root>/.aida-store` despite neither of those (the BUG-433 shape: a
/// session worktree forked from a commit that predates the committed
/// scaffolding, or a symlink into another repo's store). The same signals the
/// main resolver uses (see the `distributed_root` computation above) to
/// refuse the legacy fallback when the store isn't resolvable through
/// `detect_distributed_store`.
// trace:TASK-1486 trace:BUG-428 trace:BUG-442 trace:BUG-433 trace:TASK-1487 | ai:claude
pub(crate) fn unattached_distributed_root(start: &std::path::Path) -> Option<std::path::PathBuf> {
    distributed_mode_declared_from(start).or_else(|| {
        let root = start.ancestors().find(|d| d.join(".git").exists())?;
        if branch_exists_anywhere(root, "aida-store") {
            return Some(root.to_path_buf());
        }
        attached_store_present(root).then(|| root.to_path_buf())
    })
}

/// BUG-433: is a git-canonical store physically attached at `<root>/.aida-store`?
/// True when `.aida-store/objects/` is a directory — the hallmark of an attached
/// orphan-store worktree, resolved through a symlink too. Used to detect
/// distributed mode from the store's SHAPE when neither `.aida/config.toml` nor
/// an `aida-store` branch is present, so plain `aida` uses the real store
/// instead of silently serving legacy data. Delegates to `aida-core` so this
/// stays in lockstep with every other caller of the SAME resolver (e.g.
/// `aida-tui`'s mail scope).
// trace:BUG-433 trace:TASK-1141 | ai:claude
pub(crate) fn attached_store_present(project_root: &std::path::Path) -> bool {
    aida_core::store_locate::is_store_attached(project_root)
}

/// Resolve the `AIDA_STORE` env override into a usable store path, or `None`
/// when unset / pointing at something that isn't a git-canonical store. A valid
/// store directory is one that exists and contains an `objects/` subdirectory
/// (the per-spec YAML tree GitBackend manages). Validation is deliberately
/// strict — a typo'd or not-yet-created path falls THROUGH to normal
/// resolution rather than erroring, so the override is opt-in and never the
/// thing that breaks a forgotten-export shell (SPIKE-48). Use `aida sandbox
/// create` to produce a directory this accepts.
///
/// BUG-567 Finding 1: the fall-through is no longer SILENT. When `AIDA_STORE`
/// is set but unusable we emit exactly ONE stderr notice naming the path, the
/// reason, and that we fell back to normal resolution — then still fall
/// through (no error, no behavior change; informational only). Suppress with
/// `AIDA_QUIET` so scripts can mute it. The never-break-a-forgotten-export
/// intent (SPIKE-48) is preserved — we inform, we don't error.
/// trace:SPIKE-48 trace:BUG-567 | ai:claude
pub(crate) fn aida_store_override() -> Option<std::path::PathBuf> {
    let raw = std::env::var("AIDA_STORE").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // trace:BUG-567 | ai:claude
    match aida_store_override_from(std::path::Path::new(raw)) {
        StoreOverride::Usable(p) => Some(p),
        StoreOverride::Unusable { reason } => {
            if !aida_quiet() {
                eprintln!(
                    "{} AIDA_STORE points at `{raw}` but it's unusable ({reason}); \
                     falling back to normal store resolution.\n  {} set AIDA_QUIET=1 \
                     to silence this, or point AIDA_STORE at a directory holding an \
                     `objects/` subdir (see `aida sandbox create`).",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    "→".cyan(),
                );
            }
            None
        }
    }
}

/// BUG-568: detect a "shared-store-but-multi-repo" context — a workspace where
/// a single `.aida-store` is shared by two or more code repos (via a
/// `.aida-workspace` manifest, as written by `aida init --sibling` /
/// `init_workspace`).
///
/// The completion/linkage scanners (`auto_bump_done_to_completed`,
/// `handle_db_reconcile_status`, `collect_git_linkage*`, `scan_trace_graph`)
/// each scan only the *local* repo's history/source tree. When the store spans
/// multiple repos, a spec whose referencing commit / trace comment lives in a
/// SIBLING repo is silently missed. Full multi-repo scanning is deferred to
/// SPIKE-62 (it needs the repo-identity model); until then we at least make the
/// limitation VISIBLE rather than silent.
///
/// Returns the other repo names (those whose `path` is not the current repo)
/// when a multi-repo workspace is detected, or `None` otherwise. The common
/// single-repo case (no manifest, or a manifest with <2 repos) returns `None`
/// → callers warn nothing → ZERO behavior change. The detection is cheap: walk
/// up from cwd for `.aida-workspace`, parse it, count repos.
// trace:BUG-568 | ai:claude
pub(crate) fn detect_multi_repo_shared_store(from: &std::path::Path) -> Option<Vec<String>> {
    let (workspace_root, manifest) = aida_core::workspace::WorkspaceManifest::discover(from)?;
    if manifest.repos.len() < 2 {
        return None;
    }
    // Identify which repo (if any) we're currently inside, so we can name the
    // OTHERS in the warning. Canonicalize both sides so a symlinked/relative
    // cwd still matches.
    let here = from.canonicalize().ok();
    let others: Vec<String> = manifest
        .repos
        .iter()
        .filter(|r| {
            let repo_abs = workspace_root.join(&r.path);
            match (&here, repo_abs.canonicalize().ok()) {
                (Some(h), Some(repo)) => !h.starts_with(&repo),
                _ => true,
            }
        })
        .map(|r| {
            if r.name.is_empty() {
                r.path.clone()
            } else {
                r.name.clone()
            }
        })
        .collect();
    Some(others)
}

/// Resolve the workspace repo slug for the repo containing `from` — the
/// canonical `origin.repo` join key (ADR-12) shared by specs and every
/// linkage artifact. `None` when no `.aida-workspace` manifest is
/// discoverable, or when `from` sits outside every manifest repo: the
/// single-repo case, where linkage stays unqualified (legacy bare-SHA
/// semantics, zero behavior change).
// trace:STORY-634 | ai:claude
pub(crate) fn workspace_repo_slug(from: &std::path::Path) -> Option<String> {
    let (workspace_root, manifest) = aida_core::workspace::WorkspaceManifest::discover(from)?;
    manifest.repo_slug_containing(&workspace_root, from)
}

/// BUG-568: emit ONE clear stderr warning that a completion/linkage scan
/// covered only the local repo while the store is shared across multiple repos,
/// so cross-repo completions/linkage may be missed. `scan_label` names the scan
/// (e.g. "auto-bump", "linkage scan") so the user can tell which surface was
/// limited. Suppressible via `AIDA_QUIET` (uniform with the BUG-567 store
/// fall-through notice). No-ops in the single-repo case (the detector returns
/// `None`). trace:BUG-568 | ai:claude
pub(crate) fn warn_multi_repo_scan_limited(from: &std::path::Path, scan_label: &str) {
    if aida_quiet() {
        return;
    }
    // Warn at most once per process — several scan sites can fire in one
    // command (e.g. `collect_git_linkage_opts` delegates to `scan_trace_graph`,
    // and a single `aida pull` runs auto-bump then renders linkage), and one
    // clear notice beats a wall of repeats. trace:BUG-568 | ai:claude
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    let Some(others) = detect_multi_repo_shared_store(from) else {
        return;
    };
    if WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    let other_repos = if others.is_empty() {
        String::new()
    } else {
        format!(" (other repos sharing this store: {})", others.join(", "))
    };
    eprintln!(
        "{} {scan_label} covered only the local repo, but this store is shared across \
         multiple repos{other_repos}. Cross-repo completions/linkage may be missed \
         (full multi-repo scanning is not yet implemented).\n  {} run this from each \
         repo, or set AIDA_QUIET=1 to silence.",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
        "→".cyan(),
    );
}

/// Is output suppression requested? BUG-567: honor `AIDA_QUIET` (any non-empty,
/// non-"0"/"false" value) so the informational store fall-through notice can be
/// muted by scripts. trace:BUG-567 | ai:claude
pub(crate) fn aida_quiet() -> bool {
    match std::env::var("AIDA_QUIET") {
        Ok(v) => {
            let v = v.trim();
            !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("false"))
        }
        Err(_) => false,
    }
}

/// Path-resolution core of [`aida_store_override`], split out so it can be
/// unit-tested without mutating process env. Returns [`StoreOverride::Usable`]
/// (canonicalized) when `path` exists and holds an `objects/` directory, else
/// [`StoreOverride::Unusable`] carrying the specific reason it was rejected.
/// Delegates to `aida-core::store_locate::classify_store_path` — the
/// canonical resolver — so this CLI and any other caller (`aida-tui`'s mail
/// scope) never drift apart.
// trace:SPIKE-48 trace:BUG-567 trace:TASK-1141 | ai:claude
pub(crate) fn aida_store_override_from(path: &std::path::Path) -> StoreOverride {
    aida_core::store_locate::classify_store_path(path)
}

/// Walk up from `start` and return the project root whose `.aida/config.toml`
/// declares `mode = "distributed"`. Used to distinguish two states that look
/// identical to [`detect_distributed_store`] (both return `None` for the
/// store path): a *legacy* single-file project (no distributed config → the
/// YAML/SQLite fallback is correct) vs. a *distributed* project whose
/// `.aida-store/` worktree just isn't set up in this working copy — the
/// hallmark of a fresh clone, since the worktree is gitignored and only
/// created by `aida init`. In the latter case the caller must refuse the
/// legacy fallback (it would show stale data) and point at `aida init`.
/// trace:BUG-428 | ai:claude
pub(crate) fn distributed_mode_declared_from(
    start: &std::path::Path,
) -> Option<std::path::PathBuf> {
    distributed_mode_declared_from_with_roots(start, &aida_core::store_locate::real_temp_roots())
}

/// [`distributed_mode_declared_from`], parameterized on the temp roots to
/// guard against. Must agree with `detect_distributed_store_from` on where
/// the walk-up stops — otherwise a stray `.aida/config.toml` sitting
/// directly in a temp root (see `aida_core::store_locate` for why that
/// happens) makes THIS function claim distributed mode where the store
/// resolver finds nothing, producing a misleading "run aida init" refusal
/// instead of the correct legacy fallback. Factored out (rather than calling
/// `is_system_temp_dir` inline) so a test can exercise the guard against a
/// fake root without mutating `TMPDIR` or touching the real, shared system
/// temp dir.
// trace:BUG-1598 | ai:claude
pub(crate) fn distributed_mode_declared_from_with_roots(
    start: &std::path::Path,
    temp_roots: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    // Canonicalize the root set ONCE, before the loop — not on every
    // ancestor level.
    let canonical_roots = aida_core::store_locate::canonicalize_roots(temp_roots);
    let mut current = start;
    loop {
        if aida_core::store_locate::is_in_canonical_roots(current, &canonical_roots) {
            return None;
        }
        let config_path = current.join(".aida").join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            // The first `.aida/config.toml` we hit walking up decides the
            // mode — a config without distributed mode means a legacy
            // project, so stop (don't keep walking to a parent project).
            return config_declares_distributed(&content).then(|| current.to_path_buf());
        }
        current = current.parent()?;
    }
}

pub(crate) fn distributed_mode_declared() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    distributed_mode_declared_from(&cwd)
}

/// TASK-621: attach the `.aida-store` worktree from the existing `aida-store`
/// branch so read commands "just work" on a fresh clone without a manual
/// `aida init`. Reuses the exact fetch + worktree-add sequence the post-clone
/// init bootstrap uses (handle_init_post_clone) — but NOT its node-id /
/// scaffolding steps. Those stay with `aida init` / `aida node acquire`: a
/// node id is the namespace for newly-issued spec ids, needed only before
/// WRITING. Reading the store needs only the worktree. Idempotent at the
/// git-ops layer (create_store_worktree no-ops when the worktree already
/// exists), but in practice only called when the worktree is absent.
/// trace:TASK-621 | ai:claude
pub(crate) fn try_attach_store_worktree(
    project_root: &std::path::Path,
) -> Result<std::path::PathBuf> {
    use aida_core::git_ops;
    let worktree_dir = ".aida-store";
    let branch = "aida-store";
    // BUG-559: on a fresh clone of an AIDA-on-GitLab repo the working tree can
    // land ON the orphan `aida-store` branch — GitLab's push-to-create adopts
    // the first-pushed branch as the project default, so a clone checks out the
    // internal YAML store as if it were the code. In that state
    // `git fetch origin aida-store:aida-store` refuses ("refusing to fetch into
    // branch refs/heads/aida-store checked out at <clone>") because git won't
    // write into a branch ref that's currently checked out.
    //
    // Fix (b) — harden auto-attach: instead of erroring, RECOVER. Switch the
    // working tree OFF `aida-store` first — to a code branch (`main`/`master`,
    // local or origin) when one exists, else to a detached HEAD — which frees
    // the `aida-store` ref so the fetch + worktree-add can proceed exactly as
    // on a healthy clone. This only fires in the already-failing GitLab state
    // (`aida-store` is the checked-out branch); a normal clone is untouched, so
    // it cannot regress the working path. The source-side prevention (making
    // the GitLab default branch `main` at init) is tracked separately under
    // BUG-559's follow-up.
    // trace:TASK-821 trace:BUG-559 | ai:claude
    if git_ops::current_branch(project_root).ok().as_deref() == Some(branch) {
        let candidates = local_code_branch_candidates(project_root);
        match choose_store_attach_recovery(project_root, &candidates) {
            StoreAttachRecovery::CheckoutCodeBranch(code_branch) => {
                git_ops::checkout_branch(project_root, &code_branch)?;
            }
            StoreAttachRecovery::DetachHead => {
                git_ops::detach_head(project_root)?;
            }
        }
    }
    // Create the local tracking branch from origin/<branch> (network fetch;
    // fails closed → caller drops to the explicit `aida init` guidance).
    git_ops::fetch_branch_into_local(project_root, "origin", branch)?;
    git_ops::create_store_worktree(project_root, worktree_dir, branch)
}

/// The recovery action chosen when `aida-store` is the checked-out branch on a
/// fresh clone (the BUG-559 GitLab state). Pure value so the decision is
/// unit-testable without running git. trace:BUG-559 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreAttachRecovery {
    /// Switch the working tree to this code branch before fetching `aida-store`.
    CheckoutCodeBranch(String),
    /// No code branch is available — detach HEAD to free the `aida-store` ref.
    DetachHead,
}

/// The conventional code-branch names to prefer when recovering, in priority
/// order. trace:BUG-559 | ai:claude
pub(crate) const CODE_BRANCH_PREFERENCE: [&str; 2] = ["main", "master"];

/// Collect the code-branch names available to switch to, in preference order:
/// a local `main`/`master` if it exists, else an `origin/main`/`origin/master`
/// (referenced by short name so `git checkout` sets up tracking). Used as input
/// to [`choose_store_attach_recovery`]; split out so the decision logic stays
/// pure and testable. trace:BUG-559 | ai:claude
pub(crate) fn local_code_branch_candidates(project_root: &std::path::Path) -> Vec<String> {
    use aida_core::git_ops;
    let mut out = Vec::new();
    for name in CODE_BRANCH_PREFERENCE {
        if git_ops::local_branch_exists(project_root, name) {
            out.push(name.to_string());
        } else if git_ops::remote_branch_exists(project_root, "origin", name) {
            // `git checkout main` against `origin/main` creates the local
            // tracking branch — git's DWIM behaviour.
            out.push(name.to_string());
        }
    }
    out
}

/// Decide how to get the working tree off the `aida-store` branch so a fetch
/// into the `aida-store` ref can succeed: check out the first available code
/// branch, or detach HEAD when none is available. Pure — given the candidate
/// list it returns the action, no git side effects. trace:BUG-559 | ai:claude
pub(crate) fn choose_store_attach_recovery(
    _project_root: &std::path::Path,
    code_branch_candidates: &[String],
) -> StoreAttachRecovery {
    match code_branch_candidates.first() {
        Some(branch) => StoreAttachRecovery::CheckoutCodeBranch(branch.clone()),
        None => StoreAttachRecovery::DetachHead,
    }
}

/// What `aida init` should push to a fresh/empty origin, and in what order, so a
/// forge that adopts the first-pushed branch as its default (GitLab's
/// push-to-create) ends up with the **code** branch as default — never the
/// orphan `aida-store` (which would make a fresh clone check out the internal
/// YAML store as if it were the project). Root-cause prevention for BUG-559.
/// trace:TASK-844 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InitPushPlan {
    /// A code branch exists and has commits → push it FIRST (so the forge
    /// adopts it as default), then the orphan store. The string is the code
    /// branch name to push.
    CodeBranchFirst(String),
    /// No code branch with commits to push (brand-new repo / nothing committed)
    /// → fall back to the legacy behavior: push only the orphan store. Pushing
    /// uncommitted/empty code would surprise the user (blast-radius guardrail),
    /// so we don't. The set-default-branch call can still run after the push.
    OrphanOnly,
}

/// Decide the init push plan from two facts about the working repo: whether a
/// code branch (`main`/`master`) **exists**, and whether it **has commits**.
/// Pure so the push-order decision is unit-testable without a live remote.
///
/// - code branch exists AND has commits → [`InitPushPlan::CodeBranchFirst`]
/// - otherwise → [`InitPushPlan::OrphanOnly`] (conservative fallback; never
///   pushes code the user hasn't committed).
///
/// trace:TASK-844 | ai:claude
pub(crate) fn decide_init_push_plan(
    code_branch: Option<&str>,
    code_branch_has_commits: bool,
) -> InitPushPlan {
    match code_branch {
        Some(name) if code_branch_has_commits => InitPushPlan::CodeBranchFirst(name.to_string()),
        _ => InitPushPlan::OrphanOnly,
    }
}

#[cfg(test)]
#[path = "tests/task_844_init_push_order_tests.rs"]
mod task_844_init_push_order_tests;

#[cfg(test)]
#[path = "tests/bug_559_clone_recovery_tests.rs"]
mod bug_559_clone_recovery_tests;

#[cfg(test)]
#[path = "tests/bug_1969_allocation_push_tests.rs"]
mod bug_1969_allocation_push_tests;

/// Walk-up resolver split out from `detect_distributed_store` so the search
/// path is testable without changing process cwd. Returns the absolute store
/// path on the first ancestor whose `.aida/config.toml` declares one.
///
/// Delegates to `aida-core::store_locate::detect_distributed_store_from` —
/// the canonical resolver (including its BUG-331 main-worktree fallback for
/// a linked/nested git worktree) — so this CLI and any other caller
/// (`aida-tui`'s mail scope) resolve to the exact same project root.
// trace:BUG-57 trace:BUG-331 trace:TASK-1141 | ai:claude
pub(crate) fn detect_distributed_store_from(start: &std::path::Path) -> Option<std::path::PathBuf> {
    aida_core::store_locate::detect_distributed_store_from(start)
}

/// Read the `[id_format] policy` from `.aida/config.toml`. Honors the legacy
/// `use_agreed_blocks` boolean as a fallback when the new section is missing
/// (so existing projects keep working unchanged).
///
/// Resolution order:
///   1. `[id_format] policy = "..."`  → parsed (errors on unknown values)
///   2. legacy `use_agreed_blocks = false`  → `node-aware-only`
///   3. legacy `use_agreed_blocks = true`   → `blocks-then-fallback`
///   4. neither set                          → `blocks-then-fallback` (default)
///      trace:EPIC-1-052 | ai:claude
///      Read the agreed-id counter for a given type from the orphan store's
///      `registry/agreed_counters.toml`. Returns 0 when the file doesn't exist
///      or the type has no entry — both mean "no ids issued yet for this type".
///      Used as the floor when claiming a new block so the block doesn't
///      overlap with already-issued agreed-ids.
///      trace:FR-1-073 | ai:claude
pub(crate) fn read_agreed_counter(store_path: &std::path::Path, type_prefix: &str) -> u32 {
    let path = store_path.join("registry").join("agreed_counters.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return 0;
    };
    let prefix_upper = type_prefix.to_uppercase();
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let (key, val) = match (parts.next(), parts.next()) {
            (Some(k), Some(v)) => (k.trim().trim_matches('"'), v.trim()),
            _ => continue,
        };
        if key.eq_ignore_ascii_case(&prefix_upper) {
            return val.parse::<u32>().unwrap_or(0);
        }
    }
    0
}

pub(crate) fn read_id_format_policy(project_dir: &std::path::Path) -> aida_core::IdFormatPolicy {
    read_id_format_settings(project_dir).0
}

/// Read counter_scope from `.aida/config.toml`. When absent, defaults to
/// PerType (back-compat — flipping a live store would conflate FR-100
/// and BUG-100 numerically). Projects created from 2026-05-09 onwards
/// have `counter_scope = "global"` written explicitly at init.
/// trace:FR-271 | ai:claude
#[allow(dead_code)]
pub(crate) fn read_id_counter_scope(project_dir: &std::path::Path) -> aida_core::IdCounterScope {
    read_id_format_settings(project_dir).1
}

/// Single pass over `.aida/config.toml` returning both the policy and
/// the counter scope. Cheaper than two separate reads when a caller
/// needs both. trace:FR-271 | ai:claude
pub(crate) fn read_id_format_settings(
    project_dir: &std::path::Path,
) -> (aida_core::IdFormatPolicy, aida_core::IdCounterScope) {
    let config_path = project_dir.join(".aida").join("config.toml");
    let default_policy = aida_core::IdFormatPolicy::default();
    let default_scope = aida_core::IdCounterScope::default();
    let Ok(content) = std::fs::read_to_string(&config_path) else {
        return (default_policy, default_scope);
    };

    let mut in_id_format = false;
    let mut policy_explicit: Option<aida_core::IdFormatPolicy> = None;
    let mut scope_explicit: Option<aida_core::IdCounterScope> = None;
    let mut legacy_use_blocks: Option<bool> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_id_format = line == "[id_format]";
            continue;
        }
        if in_id_format && line.starts_with("policy") {
            if let Some(val) = line.split('=').nth(1) {
                let s = val.trim().trim_matches('"').trim_matches('\'');
                match aida_core::IdFormatPolicy::parse(s) {
                    Ok(p) => policy_explicit = Some(p),
                    Err(e) => eprintln!("Warning: {} — using default", e),
                }
            }
        }
        if in_id_format && line.starts_with("counter_scope") {
            if let Some(val) = line.split('=').nth(1) {
                let s = val.trim().trim_matches('"').trim_matches('\'');
                match aida_core::IdCounterScope::parse(s) {
                    Ok(c) => scope_explicit = Some(c),
                    Err(e) => eprintln!("Warning: {} — using default", e),
                }
            }
        }
        if !in_id_format && line.starts_with("use_agreed_blocks") {
            if let Some(val) = line.split('=').nth(1) {
                legacy_use_blocks = Some(val.trim() != "false");
            }
        }
    }

    let policy = policy_explicit.unwrap_or(match legacy_use_blocks {
        Some(false) => aida_core::IdFormatPolicy::NodeAwareOnly,
        Some(true) => aida_core::IdFormatPolicy::BlocksThenFallback,
        None => default_policy,
    });
    let scope = scope_explicit.unwrap_or(default_scope);
    (policy, scope)
}

pub(crate) fn line_for_key(body: &str, section: &str, key: &str) -> Option<usize> {
    let mut in_section = false;
    for (idx, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed.trim_matches(&['[', ']'][..]) == section;
            continue;
        }
        if in_section && trimmed.starts_with(key) {
            return Some(idx + 1);
        }
    }
    None
}

pub(crate) fn read_store_sync_config(project_root: &std::path::Path) -> Result<StoreSyncConfig> {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(StoreSyncConfig::default());
    };
    let value: toml::Value = toml::from_str(&body)
        .map_err(|err| anyhow::anyhow!(config_parse_error_message(&path, &body, &err)))?;
    let Some(sync) = value.get("store").and_then(|s| s.get("sync")) else {
        return Ok(StoreSyncConfig {
            source: path.display().to_string(),
            ..StoreSyncConfig::default()
        });
    };
    let raw = sync
        .get("auto_push")
        .and_then(|v| v.as_str())
        .unwrap_or("manual");
    let Some(auto_push) = StoreAutoPushMode::parse(raw) else {
        let line = line_for_key(&body, "store.sync", "auto_push")
            .map(|n| format!(":{n}"))
            .unwrap_or_default();
        anyhow::bail!(
            "{}{}: invalid [store.sync] auto_push value `{}` (expected: manual, session-end, per-write, periodic)",
            path.display(),
            line,
            raw
        );
    };
    // TASK-1096: mirror_remotes = ["gitlab", ...] — extra hubs the store push
    // fans out to. Non-string entries are skipped; a bare string is tolerated as
    // a single-remote shorthand.
    let mirror_remotes = match sync.get("mirror_remotes") {
        Some(toml::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect(),
        Some(toml::Value::String(s)) => vec![s.to_string()],
        _ => Vec::new(),
    };
    Ok(StoreSyncConfig {
        auto_push,
        periodic_threshold: sync
            .get("periodic_threshold")
            .and_then(|v| v.as_integer())
            .and_then(|n| u64::try_from(n).ok()),
        periodic_interval: sync
            .get("periodic_interval")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        mirror_remotes,
        source: path.display().to_string(),
    })
}

pub(crate) fn read_store_allocation_config(
    project_root: &std::path::Path,
) -> Result<StoreAllocationConfig> {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(StoreAllocationConfig::default());
    };
    let value: toml::Value = toml::from_str(&body)
        .map_err(|err| anyhow::anyhow!(config_parse_error_message(&path, &body, &err)))?;
    let Some(allocation) = value
        .get("store")
        .and_then(|s| s.get("allocation"))
        .and_then(|v| v.as_table())
    else {
        return Ok(StoreAllocationConfig::default());
    };
    let retry_max = allocation
        .get("retry_max")
        .and_then(|v| v.as_integer())
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .unwrap_or(StoreAllocationConfig::default().retry_max);
    Ok(StoreAllocationConfig { retry_max })
}

pub(crate) fn warn_if_periodic_auto_push(project_root: &std::path::Path) {
    if let Ok(cfg) = read_store_sync_config(project_root) {
        if cfg.auto_push == StoreAutoPushMode::Periodic {
            eprintln!(
                "{} [store.sync] auto_push = \"periodic\" requires aida-worker (EPIC-30); falling back to manual until shipped",
                "Warning:".yellow().bold()
            );
        }
    }
}

pub(crate) fn find_spec_id_collisions(store: &RequirementsStore) -> Vec<SpecIdCollision> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut by_spec: BTreeMap<String, Vec<SpecIdClaimant>> = BTreeMap::new();
    for req in &store.requirements {
        let Some(spec_id) = req.spec_id.as_deref() else {
            continue;
        };
        by_spec
            .entry(spec_id.to_ascii_uppercase())
            .or_default()
            .push(SpecIdClaimant {
                uuid: req.id,
                title: req.title.clone(),
            });
    }

    by_spec
        .into_iter()
        .filter_map(|(spec_id, mut claimants)| {
            let unique_uuids: BTreeSet<Uuid> = claimants.iter().map(|c| c.uuid).collect();
            if unique_uuids.len() <= 1 {
                return None;
            }
            claimants.sort_by(|a, b| a.uuid.cmp(&b.uuid));
            Some(SpecIdCollision { spec_id, claimants })
        })
        .collect()
}

pub(crate) fn spec_id_collision_recovery_message(
    collisions: &[SpecIdCollision],
    store_path: &std::path::Path,
) -> String {
    let mut out = String::new();
    out.push_str("duplicate AIDA spec IDs detected after syncing the git-canonical store\n");
    out.push_str(
        "AIDA is refusing to continue before a divergent SPEC-ID silently drops content.\n\n",
    );
    for collision in collisions.iter().take(5) {
        out.push_str(&format!("  {} is claimed by:\n", collision.spec_id));
        for claimant in &collision.claimants {
            out.push_str(&format!("    - {} — {}\n", claimant.uuid, claimant.title));
        }
    }
    if collisions.len() > 5 {
        out.push_str(&format!(
            "  ... plus {} more duplicate id(s)\n",
            collisions.len() - 5
        ));
    }
    out.push_str("\nPaste-ready recovery:\n");
    out.push_str(&format!("  cd {}\n", store_path.display()));
    out.push_str("  git status\n");
    out.push_str("  # inspect the duplicate object(s), then preserve both contents manually\n");
    out.push_str("  # planned tooling: aida db check --collisions --show-conflict\n");
    out.push_str("  # planned tooling: aida db check --collisions --repair\n");
    out.push_str(
        "\nDo not use `git rebase --skip` unless you intentionally want to drop one side.\n",
    );
    out
}

/// Group the flat `(spec_id, uuid, title)` rows the cache returns into
/// `SpecIdCollision`s — one per spec_id claimed by ≥2 distinct uuids. Same shape
/// `find_spec_id_collisions` produces from a full store, so the recovery message
/// is identical whichever path detected the clash.
// trace:BUG-701 | ai:claude
pub(crate) fn group_spec_id_collisions(rows: Vec<(String, Uuid, String)>) -> Vec<SpecIdCollision> {
    use std::collections::BTreeMap;
    let mut by_spec: BTreeMap<String, Vec<SpecIdClaimant>> = BTreeMap::new();
    for (spec_id, uuid, title) in rows {
        by_spec
            .entry(spec_id)
            .or_default()
            .push(SpecIdClaimant { uuid, title });
    }
    by_spec
        .into_iter()
        .filter_map(|(spec_id, mut claimants)| {
            claimants.sort_by(|a, b| a.uuid.cmp(&b.uuid));
            claimants.dedup_by(|a, b| a.uuid == b.uuid);
            if claimants.len() <= 1 {
                return None;
            }
            Some(SpecIdCollision { spec_id, claimants })
        })
        .collect()
}

/// [`ensure_no_spec_id_collisions`] for a caller holding the store write
/// lock: a full, strict scan of the canonical objects (an unreadable or
/// unparsable object is an error, not a skip), never the cache. Refreshing
/// the cache would mean waiting on the cache lock while holding the store
/// lock, and a stale cache could miss a duplicate the rebase just brought in.
// trace:TASK-1717 | ai:claude
pub(crate) fn ensure_no_spec_id_collisions_canonical(store_path: &std::path::Path) -> Result<()> {
    let mut store = RequirementsStore::default();
    store.requirements = aida_core::git_ops::load_requirements_strict(store_path)
        .context("cannot check for duplicate ids: the canonical store did not read completely")?;
    let collisions = find_spec_id_collisions(&store);
    if collisions.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "{}",
        spec_id_collision_recovery_message(&collisions, store_path)
    );
}

pub(crate) fn ensure_no_spec_id_collisions(store_path: &std::path::Path) -> Result<()> {
    // BUG-701: on the hot `aida add` path this ran a full O(n) `GitBackend::load()`
    // (re-parsing every spec YAML) purely to detect duplicate spec_ids — ~2s and
    // growing with the store. Use the cache's indexed spec_id group-by instead
    // (sub-ms, size-independent). `spec_id_collisions` freshens the cache first,
    // so a collision a just-completed pre-allocation pull introduced is still
    // caught. Fall back to the authoritative full-store scan whenever the cache
    // is unavailable/unreadable (legacy centralized mode, torn cache) so the
    // duplicate guard never silently weakens. trace:BUG-701 | ai:claude
    let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
    if let Ok(backend) = aida_core::CachedGitBackend::open(store_path, &cache_path) {
        if let Ok(rows) = backend.spec_id_collisions() {
            let collisions = group_spec_id_collisions(rows);
            if collisions.is_empty() {
                return Ok(());
            }
            anyhow::bail!(
                "{}",
                spec_id_collision_recovery_message(&collisions, store_path)
            );
        }
    }

    // Fallback: authoritative full-store scan (cache absent or unreadable).
    let backend = aida_core::GitBackend::new(store_path)?;
    let store = backend.load()?;
    let collisions = find_spec_id_collisions(&store);
    if collisions.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "{}",
        spec_id_collision_recovery_message(&collisions, store_path)
    );
}

// TASK-857: opt-out for the pre-allocation remote sync. `aida add`'s remaining
// multi-second cost is the unconditional network round-trips guarding the
// cross-clone duplicate-id race. Setting AIDA_ADD_NO_REMOTE_SYNC truthy makes
// every store-syncing step on the add path purely local — no ls-remote probe,
// no pull --rebase, no push. The local collision check still runs, so the
// in-clone duplicate guard is preserved; the cross-clone guard degrades to
// "reconverge on the next online add / `aida db sync`". For fully-offline or
// solo-clone workflows where the network floor is pure latency.
// trace:TASK-857 | ai:claude
pub(crate) fn add_remote_sync_disabled() -> bool {
    std::env::var("AIDA_ADD_NO_REMOTE_SYNC")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub(crate) fn pull_store_before_id_allocation(
    store_path: &std::path::Path,
    project_root: &std::path::Path,
) -> Result<()> {
    use aida_core::git_ops;

    if !git_ops::is_git_repo(store_path) || !git_ops::has_remote(store_path, "origin") {
        ensure_no_spec_id_collisions(store_path)?;
        return Ok(());
    }

    // TASK-857: explicit purely-local mode — skip every network leg, keep the
    // local collision guard. trace:TASK-857 | ai:claude
    if add_remote_sync_disabled() {
        return ensure_no_spec_id_collisions(store_path);
    }

    // trace:TASK-1717 | ai:claude — the pending commit is a store-writer
    // transaction of its own; the commit is kept whatever the network does.
    // A failed stage/commit (a refusing hook) stops the allocation with the
    // original error; the pending changes stay as they were.
    aida_core::db::with_store_write_lock(store_path, || {
        if git_ops::has_changes_checked(store_path)
            .context("cannot read the store status before id allocation")?
        {
            git_ops::add(store_path, &["."])
                .and_then(|_| {
                    git_ops::commit(
                        store_path,
                        "chore: sync pending changes before id allocation",
                    )
                })
                .context("could not commit pending store changes before id allocation")?;
        }
        Ok(())
    })?;

    let cfg = read_store_allocation_config(project_root)?;
    let branch = git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());

    // TASK-857 (option a): the network floor. Instead of an UNCONDITIONAL full
    // `pull --rebase` (fetch + rebase the whole orphan store) on every add,
    // first ask the remote for just the branch head SHA via a single
    // `ls-remote` (refs only, no object transfer). Three outcomes:
    //
    //   * remote head == local HEAD  → the local store already contains the
    //     remote's latest state. Allocating from it is provably safe, so SKIP
    //     the heavy pull entirely and just run the local collision check. This
    //     is the common steady-state case and the whole latency win.
    //   * remote head != local HEAD  → the remote moved; fall through to the
    //     existing pull --rebase retry loop to converge before allocating
    //     (duplicate-id guarantee preserved exactly as before).
    //   * ls-remote returns None     → remote unreachable (offline). Rather
    //     than hard-failing every add while offline, degrade gracefully: warn
    //     once and allocate from local state under the local collision guard.
    //     The push-after-allocation leg re-converges with the remote (and is
    //     itself offline-tolerant), so a sibling clone reconciles on its next
    //     online add. trace:TASK-857 | ai:claude
    match git_ops::remote_branch_head_sha(store_path, "origin", &branch) {
        Some(remote_sha) => {
            if let Ok(local_sha) = git_ops::head_sha(store_path) {
                if remote_sha == local_sha {
                    // Already current with origin — no objects to fetch.
                    return ensure_no_spec_id_collisions(store_path);
                }
            }
            // Remote diverged (or couldn't read local HEAD) → heavy pull below.
        }
        None => {
            // Offline / unreachable: file locally, reconverge later.
            eprintln!(
                "{} origin unreachable — filing offline; the new id syncs on the next online `aida add` or `aida db sync --push`.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            );
            return ensure_no_spec_id_collisions(store_path);
        }
    }

    for attempt in 0..cfg.retry_max {
        // trace:TASK-1717 | ai:claude — each attempt's rebase, its own
        // cleanup and the collision re-read of the result share one lock
        // window, so the allocator never starts from a store another writer
        // changed between the pull and the check.
        let pulled = aida_core::db::with_store_write_lock(store_path, || {
            git_ops::pull_rebase(store_path, "origin", &branch)?;
            Ok(ensure_no_spec_id_collisions_canonical(store_path))
        });
        match pulled {
            Ok(checked) => return checked,
            Err(e) if attempt + 1 < cfg.retry_max => {
                eprintln!(
                    "{} store allocation pull failed ({}) — retrying ({}/{})",
                    "Warning:".yellow().bold(),
                    e,
                    attempt + 1,
                    cfg.retry_max
                );
            }
            Err(e) => {
                anyhow::bail!(
                    "store allocation pull failed after {} attempt(s): {}\n\
                     To recover:\n  cd {} && git rebase --abort\n  aida db sync --pull",
                    cfg.retry_max,
                    e,
                    store_path.display()
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn push_store_after_id_allocation(
    store_path: &std::path::Path,
    project_root: &std::path::Path,
    spec_id: &str,
) -> Result<()> {
    use aida_core::git_ops;

    if !git_ops::is_git_repo(store_path) || !git_ops::has_remote(store_path, "origin") {
        return Ok(());
    }

    // TASK-857: explicit purely-local mode — the new id is committed locally and
    // will publish on the next online `aida add` / `aida db sync --push`.
    // trace:TASK-857 | ai:claude
    if add_remote_sync_disabled() {
        return Ok(());
    }

    let cfg = read_store_allocation_config(project_root)?;
    ensure_no_spec_id_collisions(store_path)?;
    // trace:TASK-1717 | ai:claude — capture the exact commit to publish under
    // the store write lock, push it by SHA outside the lock, and on rejection
    // rebase the CURRENT store (keeping commits other writers made meanwhile)
    // under a fresh lock window before capturing the next SHA.
    let (branch, mut sha) = aida_core::db::with_store_write_lock(store_path, || {
        let branch =
            git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());
        Ok((branch, git_ops::head_sha(store_path)?))
    })?;
    for attempt in 0..cfg.retry_max {
        let pushed = git_ops::push_exact_outcome(store_path, "origin", &sha, &branch);
        match allocation_push_step(pushed, spec_id, &branch) {
            AllocationPushStep::Published => return Ok(()),
            AllocationPushStep::Retry if attempt + 1 < cfg.retry_max => {
                eprintln!(
                    "{} store push rejected after allocating {} — pulling/retrying ({}/{})",
                    "Warning:".yellow().bold(),
                    spec_id,
                    attempt + 1,
                    cfg.retry_max
                );
                sha = aida_core::db::with_store_write_lock(store_path, || {
                    git_ops::pull_rebase(store_path, "origin", &branch)?;
                    ensure_no_spec_id_collisions_canonical(store_path)?;
                    git_ops::head_sha(store_path)
                })?;
            }
            AllocationPushStep::Retry => {
                anyhow::bail!(
                    "store push rejected after allocating {} and {} attempt(s) were exhausted.\n\
                     Your local store still has the new spec commit; do not re-file blindly.\n\
                     To recover:\n  aida db sync --pull\n  aida db sync --push",
                    spec_id,
                    cfg.retry_max
                );
            }
            // TASK-857: a push that cannot reach origin is the offline /
            // network-down case. A spec is already filed and committed locally;
            // the duplicate-id guarantee only requires the publish to land
            // *eventually* (a sibling clone reconciles when it next pulls). So
            // DEFER rather than fail the whole `aida add`: warn and return Ok.
            // The CAS-reject retry above is untouched — that's the real
            // serialize-the-winners guard and only fires when online.
            // trace:TASK-857 | ai:claude
            AllocationPushStep::Defer { detail: None } => {
                eprintln!(
                    "{} {} filed locally but not yet published (origin unreachable). \
                     It syncs on the next online `aida add` or `aida db sync --push`.",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    spec_id
                );
                return Ok(());
            }
            // trace:BUG-1969 | ai:claude — an unrecognised push failure is
            // most often a flaky network (proxy, 5xx, dropped ssh), and the
            // spec is already committed locally, so defer with git's own
            // words rather than fail `aida add` (and skip its `--queue`).
            AllocationPushStep::Defer {
                detail: Some(stderr),
            } => {
                eprintln!(
                    "{} {} filed locally, not yet published: the store push failed.\n\
                     git said:\n{}\n\
                     It syncs on the next successful `aida add` or `aida db sync --push`.",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    spec_id,
                    stderr
                );
                return Ok(());
            }
            AllocationPushStep::Fail(message) => anyhow::bail!(message),
        }
    }
    Ok(())
}

/// What `push_store_after_id_allocation` does with one push attempt.
// trace:BUG-1969 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AllocationPushStep {
    Published,
    /// Non-fast-forward: pull/rebase and retry (or give up when exhausted).
    Retry,
    /// Keep the local commit and publish later. `detail: None` is the
    /// recognised offline case; `Some(stderr)` an unclassified failure.
    Defer {
        detail: Option<String>,
    },
    /// A push that can never succeed by retrying or waiting (permission,
    /// protected branch, server hook) or that could not run at all: stop
    /// with the real message and the recovery hint.
    Fail(String),
}

/// Map one classified push attempt to the allocation's next step. Only the
/// positively identified persistent causes (permission, protected ref,
/// server hook) fail; they used to print the offline guidance too, so the
/// operator retried a push that could never succeed. Unreachable and
/// unclassified failures defer, as every push error did before.
// trace:BUG-1969 | ai:claude
pub(crate) fn allocation_push_step(
    pushed: Result<aida_core::git_ops::PushOutcome>,
    spec_id: &str,
    branch: &str,
) -> AllocationPushStep {
    use aida_core::git_ops::{PushFailure, PushOutcome};
    let (kind, stderr) = match pushed {
        Ok(PushOutcome::Pushed) => return AllocationPushStep::Published,
        Ok(PushOutcome::Failed { kind, stderr }) => (kind, stderr),
        Err(e) => {
            return fail_step(
                spec_id,
                "the store push could not run",
                false,
                &format!("{e:#}"),
            )
        }
    };
    let cause = match kind {
        PushFailure::NonFastForward => return AllocationPushStep::Retry,
        PushFailure::Unreachable => return AllocationPushStep::Defer { detail: None },
        PushFailure::Other => {
            return AllocationPushStep::Defer {
                detail: Some(stderr),
            }
        }
        PushFailure::PermissionDenied => {
            "origin refused the push: missing credentials or no write permission".to_string()
        }
        PushFailure::ProtectedRef => {
            format!("origin refused the push: branch `{branch}` is protected")
        }
        PushFailure::RemoteDeclined => {
            "origin refused the push: a server-side hook or policy declined it".to_string()
        }
    };
    fail_step(spec_id, &cause, true, &stderr)
}

/// The `Fail` step's operator message: cause, the real detail, and the
/// recovery hint. `persistent`: retrying cannot succeed until the operator
/// fixes the cause.
// trace:BUG-1969 | ai:claude
fn fail_step(spec_id: &str, cause: &str, persistent: bool, detail: &str) -> AllocationPushStep {
    let retry_note = if persistent {
        "Retrying will not help until that is fixed. "
    } else {
        ""
    };
    AllocationPushStep::Fail(format!(
        "{spec_id} was filed locally but not published: {cause}.\n\
         Details:\n{detail}\n\
         {retry_note}Your local store still has the new spec commit; do not re-file it.\n\
         To recover, fix the cause above, then run:\n  aida db sync --push"
    ))
}

pub(crate) fn humanize_relative(t: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let delta = now.signed_duration_since(t);
    let secs = delta.num_seconds();
    if secs < 0 {
        return "just now".to_string();
    }
    if secs < 60 {
        return format!("{}s ago", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m ago", mins);
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h ago", hours);
    }
    let days = hours / 24;
    format!("{}d ago", days)
}

/// Statusline cache freshness state. Two independent axes folded into a
/// single label by severity:
///   1. cache.db vs local orphan branch HEAD — `Stale` when they differ
///      (next read will rebuild the cache).
///   2. local orphan branch HEAD vs `origin/aida-store` — `Behind` when
///      local lags origin (run `aida db sync --pull`).
///      `Unknown` covers "can't tell about origin yet" — either no recent
///      fetch (STORY-79 hasn't run, or it's been >threshold) or no
///      `origin/aida-store` ref locally. `Fresh` is the all-good state and
///      doesn't render. trace:STORY-78 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheFreshness {
    Fresh,
    Stale,
    Behind,
    Unknown,
    /// No `.aida-store` attached in this worktree (e.g. a harness /
    /// general-purpose worktree with no orphan store checked out).
    /// Distinct from `Unknown` (origin freshness is transiently
    /// unknowable in an *attached* store): with no store at all there is
    /// nothing to report, so the segment is omitted rather than rendering
    /// a misleading `cache:?`. trace:BUG-518 | ai:claude
    NoStore,
}

impl CacheFreshness {
    /// One-letter label used by statusline. None means "don't render"
    /// (the Fresh case — boring, hide it; and the NoStore case — there is
    /// no store to report on, BUG-518). trace:STORY-78 | ai:claude
    pub(crate) fn label(self) -> Option<&'static str> {
        match self {
            CacheFreshness::Fresh => None,
            CacheFreshness::Stale => Some("stale"),
            CacheFreshness::Behind => Some("behind"),
            CacheFreshness::Unknown => Some("?"),
            // No store attached → omit the segment entirely. trace:BUG-518
            CacheFreshness::NoStore => None,
        }
    }
}

/// Decide which freshness state to render given pre-collected inputs.
/// Pure — no I/O, no env reads — so the precedence rules can be
/// exhaustively tested.
///
/// `local_behind_origin` is a tri-state:
///   - Some(true)  → origin has commits local doesn't (pull needed)
///   - Some(false) → local is equal-to or strictly-ahead-of origin
///     (push may be needed but no pull) — render Fresh
///   - None        → we couldn't determine direction (no recent fetch,
///     no origin ref, or rev-list lookup failed)
///
/// The "strictly ahead → Fresh" semantics keep us from nagging the user
/// to pull when they have unpushed local commits — `cache:behind` would
/// be misleading because there's nothing on origin to pull.
/// trace:STORY-78 | ai:claude
pub(crate) fn classify_cache_freshness(
    recorded_cache_sha: Option<&str>,
    local_sha: &str,
    local_behind_origin: Option<bool>,
    last_fetch_age_secs: Option<u64>,
    freshness_threshold_secs: u64,
) -> CacheFreshness {
    // No local store attached (e.g. a harness / general-purpose worktree
    // with no `.aida-store` checked out) → there is nothing to report.
    // Signal NoStore so the statusline omits the segment rather than
    // rendering a misleading `cache:?` (which means "store attached,
    // origin freshness transiently unknown"). trace:BUG-518 | ai:claude
    if local_sha.is_empty() {
        return CacheFreshness::NoStore;
    }
    // Axis 1: cache vs local. Stale wins over every other state because
    // the immediate consequence (slow next read while cache rebuilds) is
    // user-visible regardless of remote state.
    let cache_matches_local = recorded_cache_sha
        .map(|r| sha_prefix_match(r, local_sha))
        .unwrap_or(false);
    if !cache_matches_local {
        return CacheFreshness::Stale;
    }
    // Axis 2: local vs origin. Requires both a recent fetch AND a
    // direction signal — either missing collapses to Unknown rather
    // than false-fresh. STORY-79 is the producer of the last-fetch
    // timestamp; until it ships, last_fetch_age_secs is always None and
    // we render `?` whenever cache matches local. trace:STORY-79 | ai:claude
    let fetch_is_recent = last_fetch_age_secs
        .map(|age| age <= freshness_threshold_secs)
        .unwrap_or(false);
    if !fetch_is_recent {
        return CacheFreshness::Unknown;
    }
    match local_behind_origin {
        None => CacheFreshness::Unknown,
        Some(true) => CacheFreshness::Behind,
        Some(false) => CacheFreshness::Fresh,
    }
}

/// Decide whether `local_sha` is strictly behind `origin_sha` in the
/// orphan store at `store_path`. "Strictly behind" means: origin has at
/// least one commit not reachable from local. Returns None when git
/// can't answer (e.g. unknown commit, fork without merge-base).
///
/// `local == origin` short-circuits to Some(false). Strictly-ahead also
/// returns Some(false) — we explicitly do NOT report "behind" for the
/// have-unpushed-commits case. trace:STORY-78 | ai:claude
pub(crate) fn local_lags_origin(
    store_path: &std::path::Path,
    local_sha: &str,
    origin_sha: &str,
) -> Option<bool> {
    if sha_prefix_match(local_sha, origin_sha) {
        return Some(false);
    }
    let range = format!("{}..{}", local_sha, origin_sha);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(store_path)
        .args(["rev-list", "--count", &range])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let count: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(count > 0)
}

/// Read `~/.aida/cache/last-fetch.toml` and return the age (in seconds)
/// of the most recent successful fetch for `project_root`. Returns None
/// when the file is absent, the entry is missing, the last result was
/// an error, or the timestamp is unparseable — all of which collapse to
/// "Unknown" in the freshness classifier. trace:STORY-78 | ai:claude
pub(crate) fn read_last_fetch_age_secs(project_root: &std::path::Path) -> Option<u64> {
    let home = aida_home_dir()?;
    let toml_path = home.join(".aida/cache/last-fetch.toml");
    let raw = std::fs::read_to_string(&toml_path).ok()?;
    let value: toml::Value = toml::from_str(&raw).ok()?;
    // Schema (per STORY-79): top-level keys are project root absolute
    // paths; each entry is a table { last_fetch = "<rfc3339>", result =
    // "ok" | "error" }. Look up by canonical project_root path.
    let canon = project_root.canonicalize().ok()?;
    let key = canon.to_string_lossy();
    let entry = value.get(key.as_ref())?;
    let result = entry.get("result").and_then(|v| v.as_str()).unwrap_or("ok");
    if result != "ok" {
        return None;
    }
    let ts_str = entry.get("last_fetch").and_then(|v| v.as_str())?;
    let ts = chrono::DateTime::parse_from_rfc3339(ts_str).ok()?;
    let now = chrono::Utc::now();
    let age = now.signed_duration_since(ts.with_timezone(&chrono::Utc));
    let secs = age.num_seconds();
    if secs < 0 {
        Some(0)
    } else {
        Some(secs as u64)
    }
}

/// `git rev-parse origin/aida-store` against the orphan-store worktree.
/// Returns None when the ref doesn't exist (no remote configured, never
/// fetched) or the git invocation otherwise fails. The caller maps None
/// to `CacheFreshness::Unknown` rather than treating it as
/// "no-difference". trace:STORY-78 | ai:claude
pub(crate) fn rev_parse_origin_aida_store(project_root: &std::path::Path) -> Option<String> {
    let store_path = if project_root.join(".aida-store").exists() {
        project_root.join(".aida-store")
    } else if project_root.join("aida-store").exists() {
        project_root.join("aida-store")
    } else {
        return None;
    };
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&store_path)
        .args(["rev-parse", "--short", "origin/aida-store"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Threshold above which a last-fetch timestamp is treated as "stale"
/// (i.e. we render `cache:?` rather than `cache:fresh|behind`). Matches
/// STORY-79's default background-fetch interval so a single missed fetch
/// cycle is the boundary. Overridable via `AIDA_FETCH_FRESHNESS_SECS` so
/// CI / tests can dial it without hardcoding `300`. trace:STORY-78 | ai:claude
pub(crate) fn cache_freshness_threshold_secs() -> u64 {
    std::env::var("AIDA_FETCH_FRESHNESS_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
}

/// STORY-78 Part 2: the implementation of `--sync` on read commands.
/// Mirrors `aida db sync --pull` but without the commit-pending step
/// (read commands have no pending work to commit) and without conflict
/// detection (the caller is about to read the result anyway). Designed
/// to NEVER fail the caller: every failure mode is converted to a
/// warning and the command continues with the local view.
///
/// Failure modes handled here:
///   - store missing / not a git repo → warn, skip
///   - orphan store has uncommitted changes → warn, skip (pull --rebase
///     would abort partway and leave weirder state than we started with)
///   - offline / unreachable origin → warn, skip
///   - any other git error during rebase → abort the rebase, warn, skip
///
/// On success, updates `~/.aida/cache/last-fetch.toml` so the statusline
/// `cache:behind|?` indicator immediately reflects the pull (instead of
/// waiting for STORY-79's background fetcher).
/// trace:STORY-78 | ai:claude
pub(crate) fn maybe_sync_pull(store_path: &std::path::Path) -> Result<()> {
    if !aida_core::git_ops::is_git_repo(store_path) {
        eprintln!(
            "{} --sync: store at {} is not a git repo; skipping pull",
            "Warning:".yellow().bold(),
            store_path.display()
        );
        return Ok(());
    }
    // trace:TASK-1717 | ai:claude — the dirty check, the pull --rebase and
    // the clean-up of a rebase THIS pull started run under the store write
    // lock. A rebase that was already in progress (someone else's recovery)
    // is reported and left alone, never aborted.
    let pulled = aida_core::db::with_store_write_lock(store_path, || {
        // A failing `git status` is a failed pull, not a clean tree.
        match aida_core::git_ops::has_changes_checked(store_path) {
            Ok(true) => return Ok(SyncPull::Dirty),
            Ok(false) => {}
            Err(e) => return Ok(SyncPull::Failed(e)),
        }
        let branch = aida_core::git_ops::current_branch(store_path)
            .unwrap_or_else(|_| "aida-store".to_string());
        eprintln!("{} pulling origin/{} (--sync)", "→".dimmed(), branch);
        Ok(
            match aida_core::git_ops::pull_rebase(store_path, "origin", &branch) {
                Ok(()) => SyncPull::Pulled,
                Err(e) => {
                    // pull --rebase aborts the rebase it started; if that abort
                    // failed too, retry it once while still holding the lock —
                    // the rebase is ours, since none was in progress when the
                    // lock was taken. trace:STORY-78 | ai:claude
                    if aida_core::git_ops::rebase_in_progress(store_path) {
                        let _ = std::process::Command::new("git")
                            .arg("-C")
                            .arg(store_path)
                            .args(aida_core::git_ops::transaction_git_config())
                            .args(["rebase", "--abort"])
                            .output();
                    }
                    SyncPull::Failed(e)
                }
            },
        )
    });
    match pulled {
        Ok(SyncPull::Dirty) => {
            eprintln!(
                "{} --sync: orphan store has uncommitted changes; skipping pull",
                "Warning:".yellow().bold()
            );
            eprintln!("  Run `aida db sync --pull` manually after committing or stashing.");
        }
        Ok(SyncPull::Pulled) => {
            // Best-effort: refresh the statusline freshness signal so
            // the user doesn't see `cache:?` on the very next prompt
            // after they explicitly pulled. Silent on failure — losing
            // this write costs us a stale-looking indicator for one
            // render cycle, no worse.
            let _ = touch_last_fetch_ok(store_path);
            // TASK-1033: opportunistic store maintenance rides the sync —
            // ensure the lowered gc.auto is set, then `git gc --auto` (a cheap
            // no-op unless the threshold is exceeded). Best-effort.
            aida_core::git_ops::opportunistic_store_gc(store_path);
        }
        Ok(SyncPull::Failed(e)) | Err(e) => {
            let first_line = e
                .to_string()
                .lines()
                .next()
                .unwrap_or("unknown error")
                .to_string();
            eprintln!(
                "{} --sync pull failed: {}",
                "Warning:".yellow().bold(),
                first_line
            );
            eprintln!("  Falling back to local view.");
        }
    }
    Ok(())
}

/// Outcome of `maybe_sync_pull`'s locked window.
// trace:TASK-1717 | ai:claude
enum SyncPull {
    Dirty,
    Pulled,
    Failed(anyhow::Error),
}

/// Stamp `~/.aida/cache/last-fetch.toml` with "ok"/now for the project
/// whose orphan store is `store_path`. Best-effort — keys on the
/// canonicalized parent of `store_path` so reads (via
/// `read_last_fetch_age_secs(project_root)`) match writes.
/// trace:STORY-78 | ai:claude
pub(crate) fn touch_last_fetch_ok(store_path: &std::path::Path) -> Result<()> {
    write_last_fetch_entry(store_path, "ok")
}

/// Write a result string (`"ok"` for success, `"error: <msg>"` for
/// failure) plus a fresh timestamp into `~/.aida/cache/last-fetch.toml`
/// for the project whose orphan store is `store_path`. Shared between
/// the `--sync` path (STORY-78) and the background fetch worker
/// (STORY-79). trace:STORY-78 | ai:claude
pub(crate) fn write_last_fetch_entry(store_path: &std::path::Path, result: &str) -> Result<()> {
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("store path has no parent"))?;
    let canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let home = aida_home_dir().context("no home dir")?;
    let cache_dir = home.join(".aida/cache");
    std::fs::create_dir_all(&cache_dir)?;
    let path = cache_dir.join("last-fetch.toml");

    let mut root: toml::value::Table = match std::fs::read_to_string(&path) {
        Ok(s) => toml::from_str::<toml::Value>(&s)
            .ok()
            .and_then(|v| v.as_table().cloned())
            .unwrap_or_default(),
        Err(_) => toml::value::Table::new(),
    };
    let mut entry = toml::value::Table::new();
    entry.insert(
        "last_fetch".into(),
        toml::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    entry.insert("result".into(), toml::Value::String(result.into()));
    root.insert(
        canon.to_string_lossy().to_string(),
        toml::Value::Table(entry),
    );
    let serialized = toml::to_string(&toml::Value::Table(root))?;
    std::fs::write(&path, serialized)?;
    Ok(())
}

/// Default seconds between background fetches. Statusline skips spawning
/// when the most recent fetch (or attempt) is within this window. Overridable
/// via `AIDA_BG_FETCH_INTERVAL_SECS`. trace:STORY-79 | ai:claude
pub(crate) fn bg_fetch_interval_secs() -> u64 {
    std::env::var("AIDA_BG_FETCH_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
}

/// Whether the background fetcher is enabled. `AIDA_BG_FETCH=false`
/// (or `0`, `no`, `off`) disables it entirely. Any other value (or
/// unset) leaves it enabled. trace:STORY-79 | ai:claude
pub(crate) fn bg_fetch_enabled() -> bool {
    match std::env::var("AIDA_BG_FETCH") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Path to the per-project lockfile used to coordinate background
/// fetches across concurrent shells. The basename mixes the project
/// dir name and a hex digest of the canonical path: dir name keeps it
/// debuggable, digest disambiguates two projects with the same
/// basename. trace:STORY-79 | ai:claude
pub(crate) fn bg_fetch_lock_path(store_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let project_root = store_path.parent()?;
    let canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let basename = project_root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into());
    // Cheap deterministic digest — we just need enough disambiguation
    // for human + scripted use, not collision resistance. FNV-1a 64-bit
    // by hand to avoid pulling in `siphasher`/`twox-hash`.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in canon.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let home = aida_home_dir()?;
    Some(
        home.join(".aida/cache")
            .join(format!(".bg-fetch-{}-{:016x}.lock", basename, h)),
    )
}

/// Best-effort: ensure `~/.aida/cache/` exists for the lock + toml writes.
/// Returns Ok even if creation fails — caller treats missing dir as
/// "skip background fetch this render". trace:STORY-79 | ai:claude
pub(crate) fn ensure_cache_dir() -> Option<std::path::PathBuf> {
    let home = aida_home_dir()?;
    let cache = home.join(".aida/cache");
    std::fs::create_dir_all(&cache).ok()?;
    Some(cache)
}

/// Should statusline kick off a fresh background fetch for this project?
/// Returns true when ALL conditions hold: feature enabled, no recent
/// fetch attempt (per `last-fetch.toml`), and no live lockfile.
/// Stale lockfiles (>3× interval) are considered dead and overridden.
/// Pure decision: callers do the lockfile create + spawn separately.
/// trace:STORY-79 | ai:claude
pub(crate) fn should_spawn_bg_fetch(
    last_fetch_age_secs: Option<u64>,
    lock_age_secs: Option<u64>,
    interval_secs: u64,
) -> bool {
    // Recent successful fetch (or attempt) means we're current.
    if matches!(last_fetch_age_secs, Some(age) if age < interval_secs) {
        return false;
    }
    // Live lockfile: another shell beat us to it. Treat as live for
    // 3× the interval to absorb slow git fetches on weak networks
    // without permanently wedging if a fetch crashed mid-flight.
    if matches!(lock_age_secs, Some(age) if age < interval_secs.saturating_mul(3)) {
        return false;
    }
    true
}

/// Spawn the `_bg-fetch` worker for `store_path` if the project is due
/// for a fetch and no other shell is already on it. All failure modes
/// (no home dir, can't create cache dir, lockfile-create race lost,
/// spawn failed) collapse to "skip silently" so statusline stays
/// silent and sub-50ms regardless. trace:STORY-79 | ai:claude
pub(crate) fn maybe_spawn_bg_fetch(project_root: &std::path::Path, store_path: &std::path::Path) {
    if !bg_fetch_enabled() {
        return;
    }
    let Some(_cache_dir) = ensure_cache_dir() else {
        return;
    };
    let Some(lock_path) = bg_fetch_lock_path(store_path) else {
        return;
    };
    let interval = bg_fetch_interval_secs();
    let last_fetch_age = read_last_fetch_age_secs(project_root);
    let lock_age = std::fs::metadata(&lock_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
        .map(|d| d.as_secs());
    if !should_spawn_bg_fetch(last_fetch_age, lock_age, interval) {
        return;
    }
    // Atomic claim: O_EXCL create. If another shell beat us between the
    // staleness check above and this call, the create fails and we
    // back off. trace:STORY-79 | ai:claude
    let claim = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path);
    let Ok(mut f) = claim else { return };
    use std::io::Write;
    let _ = writeln!(f, "{}", std::process::id());
    // Spawn ourselves as the hidden `_bg-fetch` worker, detached with
    // all stdio nulled. The current binary path comes from argv[0] when
    // it's absolute (cargo dev / installed); otherwise fall back to
    // `aida` on PATH. trace:STORY-79 | ai:claude
    let exe = aida_exe_path();
    let _ = std::process::Command::new(exe)
        .arg("_bg-fetch")
        .arg(store_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn_retrying_etxtbsy();
}
