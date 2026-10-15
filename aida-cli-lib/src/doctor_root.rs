//! `aida doctor` findings collection, completed-without-commit scan and GC headers (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct DoctorFinding {
    pub(crate) category: String,
    pub(crate) id: String,
    pub(crate) summary: String,
    pub(crate) action: String,
    pub(crate) safe_heal: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct StoreMirrorFanoutFailure {
    pub(crate) repo: String,
    pub(crate) branch: String,
    pub(crate) remote: String,
    pub(crate) reason: String,
    pub(crate) observed_at: String,
}

pub(crate) const STORE_MIRROR_FANOUT_FAILURES_FILE: &str = "store-mirror-fanout-failures.json";

pub(crate) const STORE_MIRROR_FANOUT_NOTIFY_RULE: &str = "store-mirror-fanout-failed";

pub(crate) fn store_mirror_fanout_failures_path(
    project_root: &std::path::Path,
) -> std::path::PathBuf {
    project_root
        .join(".aida")
        .join(STORE_MIRROR_FANOUT_FAILURES_FILE)
}

pub(crate) fn read_store_mirror_fanout_failures(
    project_root: &std::path::Path,
) -> Vec<StoreMirrorFanoutFailure> {
    let path = store_mirror_fanout_failures_path(project_root);
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&body).unwrap_or_default()
}

pub(crate) fn write_store_mirror_fanout_failures(
    project_root: &std::path::Path,
    failures: &[StoreMirrorFanoutFailure],
) -> std::io::Result<()> {
    let path = store_mirror_fanout_failures_path(project_root);
    if failures.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(failures).map_err(std::io::Error::other)?;
    std::fs::write(path, format!("{body}\n"))
}

pub(crate) fn same_store_mirror_fanout_leg(
    failure: &StoreMirrorFanoutFailure,
    repo: &std::path::Path,
    branch: &str,
    remote: &str,
) -> bool {
    failure.repo == repo.display().to_string()
        && failure.branch == branch
        && failure.remote == remote
}

pub(crate) fn record_store_mirror_fanout_failure(
    project_root: &std::path::Path,
    repo: &std::path::Path,
    branch: &str,
    remote: &str,
    reason: &str,
) {
    let mut failures = read_store_mirror_fanout_failures(project_root);
    failures.retain(|f| !same_store_mirror_fanout_leg(f, repo, branch, remote));
    failures.push(StoreMirrorFanoutFailure {
        repo: repo.display().to_string(),
        branch: branch.to_string(),
        remote: remote.to_string(),
        reason: reason.to_string(),
        observed_at: chrono::Utc::now().to_rfc3339(),
    });
    if let Err(e) = write_store_mirror_fanout_failures(project_root, &failures) {
        eprintln!(
            "  {} could not record mirror drift finding: {e}",
            "Warning:".yellow().bold()
        );
    }

    // trace:TASK-1227 | ai:codex
    let title = format!("AIDA mirror `{remote}` stopped syncing `{branch}`");
    let message = format!(
        "AIDA mirror fan-out failed for `{branch}` to `{remote}` from `{}`: {reason}\n\
         Canonical push continued. Run `aida doctor --category remote-drift` and then \
         `aida remote reconcile` once the mirror is healthy.",
        repo.display()
    );
    if let Err(e) = crate::notify::send_direct(
        project_root,
        STORE_MIRROR_FANOUT_NOTIFY_RULE,
        &title,
        &message,
    ) {
        eprintln!(
            "  {} mirror failure notification skipped: {e}",
            "Warning:".yellow().bold()
        );
    }
}

pub(crate) fn clear_store_mirror_fanout_failure(
    project_root: &std::path::Path,
    repo: &std::path::Path,
    branch: &str,
    remote: &str,
) {
    let mut failures = read_store_mirror_fanout_failures(project_root);
    let before = failures.len();
    failures.retain(|f| !same_store_mirror_fanout_leg(f, repo, branch, remote));
    if failures.len() != before {
        if let Err(e) = write_store_mirror_fanout_failures(project_root, &failures) {
            eprintln!(
                "  {} could not clear mirror drift finding: {e}",
                "Warning:".yellow().bold()
            );
        }
    }
}

pub(crate) fn scan_store_mirror_fanout_failures(
    project_root: &std::path::Path,
) -> Vec<DoctorFinding> {
    read_store_mirror_fanout_failures(project_root)
        .into_iter()
        .map(|failure| DoctorFinding {
            category: "remote-drift".to_string(),
            id: format!(
                "remote-drift-mirror-fanout-{}-{}",
                sanitize_doctor_finding_token(&failure.remote),
                sanitize_doctor_finding_token(&failure.branch)
            ),
            summary: format!(
                "mirror `{}` failed to sync branch `{}` from `{}` at {}: {}",
                failure.remote, failure.branch, failure.repo, failure.observed_at, failure.reason
            ),
            action:
                "restore the mirror remote/auth, then rerun `aida push` or `aida remote reconcile`"
                    .to_string(),
            safe_heal: false,
        })
        .collect()
}

pub(crate) fn sanitize_doctor_finding_token(raw: &str) -> String {
    let token = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if token.is_empty() {
        "unknown".to_string()
    } else {
        token
    }
}

/// TASK-1122: read a single `git config --get <key>` value in `dir`, or None.
// trace:TASK-1122 | ai:claude
pub(crate) fn git_config_value(dir: &std::path::Path, key: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "--get", key])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// TASK-752: detect tracked legacy centralized-backend artifacts —
/// `requirements.yaml`, dated `requirements_*.yaml` snapshots, and a stray
/// `scaffold-report.html` — when the project DECLARES git-canonical/distributed
/// storage but the git tree still TRACKS those files. This is storage-model
/// drift: config says git-canonical (the live store is the orphan `aida-store`
/// branch), but the tree carries the centralized model's files (PR-651 swept
/// ~2.3MB of exactly this by hand on the AIDA repo itself).
///
/// GUARD: gated strictly on git-canonical mode — `.aida/config.toml` declares
/// `mode = "distributed"` AND the orphan `aida-store` branch exists. On a legacy
/// `--centralized` project that legitimately USES `requirements.yaml` as its
/// active store, neither condition holds, so we return nothing and never nuke an
/// active backend. Returns the repo-relative tracked paths to flag (empty when
/// clean or when the project is not git-canonical).
// trace:TASK-752 | ai:claude
pub(crate) fn detect_legacy_store_cruft(project_root: &std::path::Path) -> Vec<String> {
    // Gate 1: config declares distributed/git-canonical mode.
    if distributed_mode_declared_from(project_root).is_none() {
        return Vec::new();
    }
    // Gate 2: the orphan `aida-store` branch (the actual writer of record)
    // exists — confirms the live store really is git-canonical, not a
    // half-migrated config.
    if !branch_exists_anywhere(project_root, "aida-store") {
        return Vec::new();
    }

    // `git ls-files` lists only TRACKED paths — an untracked-but-gitignored
    // `requirements.yaml` (the post-heal state) is correctly NOT flagged.
    let Ok(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["ls-files", "-z"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let mut hits: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .filter(|p| is_legacy_store_cruft_path(p))
        .map(|p| p.to_string())
        .collect();
    hits.sort();
    hits.dedup();
    hits
}

/// Whether a repo-relative path is a legacy centralized-backend artifact:
/// a top-level `requirements*.yaml` (the legacy store + dated snapshots like
/// `requirements_20251206_205840.yaml`) or a top-level `scaffold-report.html`.
/// Top-level only — never matches nested paths (e.g. a `requirements.yaml`
/// fixture deep in a test tree).
// trace:TASK-752 | ai:claude
pub(crate) fn is_legacy_store_cruft_path(path: &str) -> bool {
    // Top-level only: no path separator.
    if path.contains('/') {
        return false;
    }
    path == "scaffold-report.html" || (path.starts_with("requirements") && path.ends_with(".yaml"))
}

/// Whether a `.aida/config.toml` body declares git-canonical (distributed)
/// mode.
///
/// Factored out so the walk-up in `distributed_mode_declared_from_with_roots`
/// and the ROOT-LOCAL read in [`detect_legacy_store_in_root`] cannot drift on
/// what "declares distributed" means. They deliberately differ on *where* they
/// read, and only on that.
// trace:BUG-1734 | ai:claude
pub(crate) fn config_declares_distributed(content: &str) -> bool {
    content.lines().any(|l| {
        let l = l.trim();
        l.starts_with("mode") && l.contains("distributed")
    })
}

/// Whether `relative` is tracked by git in `project_root`.
///
/// `--error-unmatch` makes the question a status code rather than a parse:
/// a non-zero exit means "not tracked", and so does a git that will not run at
/// all, which is the answer that keeps [`detect_legacy_store_in_root`] talking
/// about a file rather than falling silent.
// trace:BUG-1734 | ai:claude
pub(crate) fn path_is_tracked(project_root: &std::path::Path, relative: &str) -> bool {
    aida_core::git_ops::host_git_command(project_root)
        .args(["ls-files", "--error-unmatch", "--", relative])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A legacy centralized store sitting in the root of a git-canonical project.
// trace:BUG-1734 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LegacyStoreInRoot {
    /// The file's name, relative to the project root.
    pub(crate) name: String,
    pub(crate) len: u64,
    /// Modified time as `YYYY-MM-DD` in **UTC**, or `None` when the platform
    /// withholds it. UTC rather than local time so the string does not depend
    /// on the host's zone; the message labels it, because an operator will
    /// compare it against `ls`, which prints local time and can therefore show
    /// the previous day.
    pub(crate) modified: Option<String>,
}

/// BUG-1734: detect an UNTRACKED legacy store (`requirements.db` /
/// `requirements.yaml`) in the root of a project whose live store is
/// git-canonical.
///
/// Distinct from [`detect_legacy_store_cruft`] in all three of its parts, which
/// is why it is its own category rather than another arm of that one:
///
/// - **Mechanism.** That detector runs `git ls-files`, so it sees TRACKED paths
///   only. This file is gitignored, hence untracked, hence invisible to it —
///   and being invisible is the entire reason the condition survived for months.
///   This one stats the filesystem.
/// - **Remedy.** That one's heal is `git rm` (`safe_heal: true`). This file may
///   be the only copy of another project's data, so the remedy is *relocate*,
///   never delete, and it must never be auto-healed. `heal_doctor_finding`
///   dispatches on the category name alone, so a separate category is what
///   keeps this away from that healer — as defence in depth, not as the only
///   defence: `heal_doctor_legacy_store_cruft` re-confirms the path is TRACKED
///   before removing anything, and this detector only ever reports untracked
///   paths, so sharing the category would have been caught there too. The
///   separation means this check does not *depend* on that guard staying.
/// - **Scope.** That one covers `requirements*.yaml` snapshots and
///   `scaffold-report.html`; this one covers only the two names that are a
///   *store*. Where the two could overlap — a TRACKED `requirements.yaml` —
///   this one stands down, so a single file never yields two findings with
///   contradictory remedies.
///
/// Why this matters at all, given distributed mode does not read the file:
/// BUG-1732 was `load_store_for_lookup` resolving its legacy fallback against
/// the process CWD instead of the `project_root` it was handed. The defect was
/// in the code and is fixed there — but it stayed unnoticed for months because
/// it is only *observable* on a host whose cwd holds a legacy store. CI has no
/// such file, so CI was green throughout. BUG-1574 is a second data point: its
/// author hand-bypassed the same fallback for one call site and wrote the hazard
/// into a doc comment without generalising it. Naming the condition turns the
/// next cwd-relative resolver to slip through into a configuration smell caught
/// here rather than a months-later mystery.
///
/// Gates, both required:
///
/// 1. **Root-local** `.aida/config.toml` declares distributed mode. Deliberately
///    NOT `distributed_mode_declared_from`, which walks UP and would adopt a
///    distributed parent's mode for a root that declares nothing itself. This
///    is a contract choice, stated honestly: `aida doctor` resolves its
///    `project_root` through a guarded walk-up that always lands on a root
///    carrying its own `.aida`, so no field scenario is known where the two
///    disagree for this caller. The root-local read is preferred because the
///    root is already in hand — re-deriving it can only introduce a way to be
///    wrong — and the choice is pinned by test rather than left implicit.
/// 2. **The distributed store is really present** (`.aida-store/`, or the orphan
///    `aida-store` branch). A config that declares distributed mode before its
///    store exists is mid-migration, and there the legacy file may still be
///    what gets read — telling that operator to relocate their live store would
///    be worse than saying nothing. Mirrors `detect_legacy_store_cruft`'s own
///    second gate, and beyond the filed acceptance on purpose.
// trace:BUG-1734 | ai:claude
pub(crate) fn detect_legacy_store_in_root(
    project_root: &std::path::Path,
) -> Vec<LegacyStoreInRoot> {
    let config = project_root.join(".aida").join("config.toml");
    let Ok(body) = std::fs::read_to_string(&config) else {
        return Vec::new();
    };
    if !config_declares_distributed(&body) {
        return Vec::new();
    }
    let store_present = project_root.join(".aida-store").is_dir()
        || branch_exists_anywhere(project_root, "aida-store");
    if !store_present {
        return Vec::new();
    }

    let mut hits = Vec::new();
    for name in ["requirements.db", "requirements.yaml"] {
        // Exactly partition with `legacy-store-cruft` so one file can never
        // produce two findings with contradictory remedies: a path that
        // detector would report (a cruft-shaped name that is TRACKED) is
        // theirs, everything else is ours. `requirements.db` is not a
        // cruft-shaped name at all, so it is always ours and the partition
        // leaves no gap.
        if is_legacy_store_cruft_path(name) && path_is_tracked(project_root, name) {
            continue;
        }
        let path = project_root.join(name);
        // `metadata`, not `exists()`: a dangling symlink is not a store, and
        // the size and mtime are the whole point of the message.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        hits.push(LegacyStoreInRoot {
            name: name.to_string(),
            len: meta.len(),
            modified: meta.modified().ok().map(|t| {
                chrono::DateTime::<chrono::Utc>::from(t)
                    .format("%Y-%m-%d")
                    .to_string()
            }),
        });
    }
    hits
}

/// The operator-facing summary for a [`LegacyStoreInRoot`].
///
/// Split from the detector so a test can pin the wording without a fixture, and
/// because AC4 is a claim about this string: it must say relocate, and must not
/// say delete.
// trace:BUG-1734 | ai:claude
pub(crate) fn legacy_store_in_root_summary(hit: &LegacyStoreInRoot) -> String {
    let when = hit
        .modified
        .as_deref()
        .map(|d| format!(", last modified {d} UTC"))
        .unwrap_or_default();
    format!(
        "legacy store `{}` ({} bytes{}) sits in the root of a git-canonical project; \
         distributed mode does not read it, so it is inert for normal operation, but any \
         code path that resolves a store relative to the process CWD can still pick it up \
         (see BUG-1732)",
        hit.name, hit.len, when
    )
}

/// The operator-facing action for a [`LegacyStoreInRoot`].
///
/// Says relocate, deliberately not delete: the file may be the only copy of
/// another project's data. Nothing in this check moves or removes anything, and
/// the category has no heal arm, so `aida doctor --heal` reports it as
/// diagnostic-only.
// trace:BUG-1734 | ai:claude
pub(crate) fn legacy_store_in_root_action(hit: &LegacyStoreInRoot) -> String {
    format!(
        "move `{}` out of the project root (it may be the only copy of another project's \
         data — relocate it, do not delete it)",
        hit.name
    )
}

/// BUG-563: detect per-clone RUNTIME files wrongly TRACKED on the orphan
/// `aida-store` branch — `.aida/node.toml` (the strictly-per-clone "I am node N"
/// pointer), `.aida/dispenser.toml` (per-node id counter), `.aida/*.lock`, and
/// `.aida/cache.db*` (the rebuildable read projection). If the orphan branch was
/// created (or had these committed) BEFORE the store-branch gitignore landed,
/// each clone's auto-sync commits rewrite `node.toml` with that clone's own node
/// id, so every cross-clone store-leg rebase conflicts on it forever — stranding
/// the rebase and breaking `aida push` with TOML parse errors from conflict
/// markers.
///
/// GUARD: gated strictly on git-canonical mode — `.aida/config.toml` declares
/// `mode = "distributed"` AND the orphan `aida-store` worktree is attached at
/// `.aida-store/`. The scan runs `git ls-files` IN THE STORE WORKTREE (not the
/// main repo), so it inspects the orphan branch's tree, never the project tree.
/// Returns the store-worktree-relative tracked paths to flag (empty when clean
/// or when the project is not git-canonical / the store worktree is absent).
// trace:BUG-563 | ai:claude
pub(crate) fn detect_store_tracked_runtime(project_root: &std::path::Path) -> Vec<String> {
    // Gate 1: config declares distributed/git-canonical mode.
    if distributed_mode_declared_from(project_root).is_none() {
        return Vec::new();
    }
    // Gate 2: the orphan `aida-store` worktree is actually attached — we scan
    // its tree, so it must be present on disk.
    let store_worktree = project_root.join(".aida-store");
    if !store_worktree.join("objects").is_dir() {
        return Vec::new();
    }

    // `git ls-files` in the STORE worktree lists only paths TRACKED on the
    // orphan branch — an untracked-but-gitignored `node.toml` (the post-heal
    // state) is correctly NOT flagged.
    let Ok(output) = aida_core::git_ops::host_git_command(&store_worktree)
        .args(["ls-files", "-z"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let mut hits: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .filter(|p| is_store_tracked_runtime_path(p))
        .map(|p| p.to_string())
        .collect();
    hits.sort();
    hits.dedup();
    hits
}

/// TASK-1547: detect an attached git-canonical store whose tracked `.gitignore`
/// lacks any atomic-write staging pattern. This check is independent of the
/// tracked-runtime scan: healed stores still need the newer staging rules.
// trace:TASK-1547 | ai:codex
pub(crate) fn detect_store_missing_staging_ignores(project_root: &std::path::Path) -> Vec<String> {
    if distributed_mode_declared_from(project_root).is_none() {
        return Vec::new();
    }
    let store_worktree = project_root.join(".aida-store");
    if !store_worktree.join("objects").is_dir() {
        return Vec::new();
    }
    let existing = std::fs::read_to_string(store_worktree.join(".gitignore")).unwrap_or_default();
    let lines: std::collections::HashSet<&str> = existing.lines().map(str::trim).collect();
    aida_core::fs_atomic::STORE_STAGING_IGNORE_PATTERNS
        .iter()
        .filter(|pattern| !lines.contains(**pattern))
        .map(|pattern| (*pattern).to_string())
        .collect()
}

/// Whether a store-worktree-relative path is a per-clone runtime file that must
/// never be tracked on the orphan `aida-store` branch: `.aida/node.toml`,
/// `.aida/dispenser.toml`, any `.aida/*.lock`, or any `.aida/cache.db*`
/// (`cache.db`, `cache.db-journal`, `cache.db-wal`, `cache.db-shm`). Scoped to
/// the top-level `.aida/` dir only — never matches nested paths or shared,
/// legitimately-tracked store files (`config.toml`, `metadata.yaml`, the
/// `objects/`/`registry/` trees, requirement yaml).
// trace:BUG-563 | ai:claude
pub(crate) fn is_store_tracked_runtime_path(path: &str) -> bool {
    // Only files directly under a top-level `.aida/` dir.
    let Some(name) = path.strip_prefix(".aida/") else {
        return false;
    };
    // No further nesting — `.aida/<name>` only, not `.aida/sub/<name>`.
    if name.contains('/') {
        return false;
    }
    name == "node.toml"
        || name == "dispenser.toml"
        || name.ends_with(".lock")
        || name == "cache.db"
        || name.starts_with("cache.db-")
}

pub(crate) fn collect_doctor_findings(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
    category_filter: Option<&str>,
) -> Result<Vec<DoctorFinding>> {
    let filter = category_filter.map(normalize_doctor_category).transpose()?;
    let mut out = Vec::new();
    let cleanup = collect_cleanup_report(project_root, store);
    let leases = list_leases(project_root);
    let live_sessions = process_probe::probe_live_claude_sessions();
    let now = chrono::Utc::now();

    let mut push = |finding: DoctorFinding| {
        if filter
            .as_deref()
            .is_none_or(|want| want == finding.category)
        {
            out.push(finding);
        }
    };

    // A parent edge is canonical; parent:* tags are a denormalized query aid.
    // Surface either direction of drift without touching the graph.
    // trace:BUG-1252 | ai:codex
    for req in &store.requirements {
        let mut expected = std::collections::BTreeSet::new();
        for rel in &req.relationships {
            if rel.rel_type == aida_core::models::RelationshipType::Child {
                if let Some(parent) = store.requirements.iter().find(|r| r.id == rel.target_id) {
                    if let Some(id) = parent.spec_id.as_deref() {
                        expected.insert(format!("parent:{id}"));
                    }
                }
            }
        }
        let actual: std::collections::BTreeSet<String> = req
            .tags
            .iter()
            .filter(|tag| tag.starts_with("parent:"))
            .cloned()
            .collect();
        if actual != expected {
            push(DoctorFinding {
                category: "parent-tag-drift".to_string(),
                id: req.spec_id.clone().unwrap_or_else(|| req.id.to_string()),
                summary: format!(
                    "parent relationship expects [{}], tags contain [{}]",
                    expected.iter().cloned().collect::<Vec<_>>().join(","),
                    actual.iter().cloned().collect::<Vec<_>>().join(",")
                ),
                action: "align parent:* tags to the Parent relationship".to_string(),
                safe_heal: true,
            });
        }
    }

    // One id answering to more than one requirement (a merge-gate agreed_id
    // equal to another object's native spec_id). Every such id is reported
    // with every object it resolves to. Not auto-healed: renumbering a spec
    // is a data migration on shared state and a disposition call.
    // trace:BUG-1535 | ai:claude
    for finding in id_collision_findings(store) {
        push(finding);
    }

    // A detached/mid-rebase store can accept commits that disappear on
    // `rebase --abort`; surface this before any further maintenance writes.
    // trace:BUG-1229 | ai:codex
    let store_worktree = project_root.join(".aida-store");
    if let Some(state) = aida_core::git_ops::store_worktree_issue(&store_worktree) {
        push(DoctorFinding {
            category: "store-worktree".to_string(),
            id: "store-worktree-unsafe-head".to_string(),
            summary: state.to_string(),
            action: format!(
                "run `git -C {} rebase --continue` or `git -C {} rebase --abort`, then retry",
                store_worktree.display(),
                store_worktree.display()
            ),
            safe_heal: false,
        });
    }

    // BUG-915: repos with container tooling and linked worktrees need the
    // worktree `.git` file's target mounted inside the container. Otherwise
    // `git rev-parse`, hooks, revision capture, and repo discovery fail from
    // inside the dev container. Detection only; the fix belongs in the
    // project's container wrapper/mount config.
    // trace:BUG-915 | ai:codex
    let container_markers = aida_core::git_ops::container_tooling_markers(project_root);
    if !container_markers.is_empty() {
        let project_canon = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        for wt in aida_core::git_ops::list_worktree_paths(project_root) {
            let wt_canon = wt.canonicalize().unwrap_or_else(|_| wt.clone());
            if wt_canon == project_canon {
                continue;
            }
            if let Some(gitdir) = aida_core::git_ops::worktree_gitdir_path(&wt) {
                push(DoctorFinding {
                    category: "worktree-container-gitdir".to_string(),
                    id: wt.display().to_string(),
                    summary: format!(
                        "container tooling detected ({}) and linked worktree {} uses gitdir {}",
                        container_markers.join(", "),
                        wt.display(),
                        gitdir.display()
                    ),
                    action: format!(
                        "mount {} into dev containers, or run git from a checkout whose shared gitdir is visible",
                        gitdir.display()
                    ),
                    safe_heal: false,
                });
            }
        }
    }

    // TASK-696: an ANCESTOR CLAUDE.md / CLAUDE.local.md / AGENTS.md whose
    // @-imports resolve OUTSIDE this project bleeds into every child project —
    // it pollutes the child's context AND trips Claude Code's "Allow external
    // CLAUDE.md file imports?" prompt on launch. The classic cause is an
    // accidental `aida init` in a parent-of-projects (TASK-686 now prevents new
    // ones; this detects existing ones). Read-only finding — removing a stray
    // ancestor file is a follow-up heal. trace:TASK-696 | ai:claude
    for ancestor in project_root.ancestors().skip(1) {
        for fname in external_import_bleed::ANCESTOR_INSTRUCTION_FILES {
            let file = ancestor.join(fname);
            let Ok(content) = std::fs::read_to_string(&file) else {
                continue;
            };
            let escaping: Vec<String> = external_import_bleed::parse_at_imports(&content)
                .into_iter()
                .filter(|imp| {
                    external_import_bleed::import_escapes_project(ancestor, imp, project_root)
                })
                .collect();
            if !escaping.is_empty() {
                push(DoctorFinding {
                    category: "external-import-bleed".to_string(),
                    id: file.display().to_string(),
                    summary: format!(
                        "ancestor {} has {} @-import(s) resolving outside this project ({}) — it bleeds into every child + trips Claude Code's external-import prompt",
                        file.display(),
                        escaping.len(),
                        escaping.join(", ")
                    ),
                    action: format!(
                        "review {} — if it's a stray scaffold (e.g. an accidental `aida init` in a parent-of-projects), remove the stray ancestor instruction file",
                        file.display()
                    ),
                    safe_heal: false,
                });
            }
        }
    }

    // STORY-762: vendor-binary check — is every vendor this project RESOLVES
    // to actually on PATH? Codex-only machines (no claude installed) must get
    // a clean doctor: only vendors a surface can reach are probed, so an
    // un-configured claude on a codex machine is never flagged (and vice
    // versa). Surfaces: the interactive/agents default ([agents] vendor knob,
    // else claude), the orchestrator headless vendor, and the TUI tab vendor
    // when explicitly configured. trace:STORY-762 | ai:claude
    {
        let mut vendors: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        // The effective default vendor for interactive/agent launches.
        vendors.insert(
            aida_core::agents_config::resolve_default_vendor(project_root)
                .unwrap_or_else(|| "claude".to_string()),
        );
        // The orchestrator headless vendor (env > [orchestrator] > knob > claude).
        vendors.insert(
            crate::session::resolve_headless_vendor(project_root)
                .as_str()
                .to_string(),
        );
        // The TUI tab vendor only when the project explicitly configures it.
        let cfg = read_project_config_value(project_root);
        if let Some(v) = config_lookup(cfg.as_ref(), "tui", "vendor").and_then(|v| v.as_str()) {
            let t = v.trim().to_ascii_lowercase();
            if t == "claude" || t == "codex" {
                vendors.insert(t);
            }
        }
        for vendor in vendors {
            if which_binary(&vendor).is_none() {
                push(DoctorFinding {
                    category: "vendor-binary".to_string(),
                    id: vendor.clone(),
                    summary: format!(
                        "resolved agent vendor `{vendor}` is not on PATH — launches routed to it will fail"
                    ),
                    action: format!(
                        "install the {vendor} CLI, or point the project at an installed vendor (agents.toml `[agents] vendor`, or the per-surface knobs)"
                    ),
                    safe_heal: false,
                });
            }
        }
    }

    let cache_path =
        aida_core::CachedGitBackend::default_cache_path(&project_root.join(".aida-store"));
    // TASK-1484: owner liveness is PID-reuse aware (process start identity) and
    // fails closed when the identity cannot be read. A dead owner's record past
    // the age threshold is a safe heal; a LIVE owner past its expected duration
    // is diagnostic evidence only (never healed). trace:TASK-1484 | ai:claude
    // BUG-1644: the sidecar is resolved from the SHARED (symlink-resolved)
    // cache location; an unshared sidecar an older binary left beside the
    // symlinked cache path is checked too, and reported even when its owner
    // is alive (its arm precedes the generic overrun arm so a live, overdue
    // stray keeps its explanation). trace:BUG-1644 | ai:claude
    let stray_lock_info = aida_core::stray_cache_lock_info_path(&cache_path);
    let lock_info_paths = std::iter::once(aida_core::cache_lock_info_path(&cache_path))
        .chain(stray_lock_info.clone());
    for lock_info_path in lock_info_paths {
        let is_stray = stray_lock_info.as_ref() == Some(&lock_info_path);
        let Some(obs) = aida_core::observe_lock_info_file(&lock_info_path)? else {
            continue;
        };
        let stale_secs = std::env::var("AIDA_CACHE_LOCK_STALE_SECS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(300);
        let age_secs = obs.age_secs(now).unwrap_or(0) as i64;
        let command = if obs.info.command.trim().is_empty() {
            "unknown command"
        } else {
            obs.info.command.as_str()
        };
        match obs.owner {
            aida_core::LockOwnerState::Dead { pid_reused } if age_secs >= stale_secs => {
                push(DoctorFinding {
                    category: "stale-locks".to_string(),
                    id: lock_info_path.display().to_string(),
                    summary: format!(
                        "cache lock-info from dead pid {} ({}) is {} old{}",
                        obs.info.pid,
                        command,
                        humanize_duration_secs(age_secs.max(0) as u64),
                        if pid_reused {
                            " (the pid now belongs to a different process)"
                        } else {
                            ""
                        }
                    ),
                    action: format!("remove stale lock-info file {}", lock_info_path.display()),
                    safe_heal: true,
                });
            }
            // trace:TASK-1526 | ai:codex
            aida_core::LockOwnerState::Unknown if obs.overrun.is_some() => {
                push(DoctorFinding {
                    category: "stale-locks".to_string(),
                    id: lock_info_path.display().to_string(),
                    summary: format!("cache lock-info for pid {} ({command}) has unknown owner identity and is past its expected duration (diagnostic only)", obs.info.pid),
                    action: "check the recorded host and process namespace; preserve the record while ownership is unknown".to_string(),
                    safe_heal: false,
                });
            }
            _ if is_stray && obs.owner == aida_core::LockOwnerState::Alive => {
                push(DoctorFinding {
                    category: "stale-locks".to_string(),
                    id: lock_info_path.display().to_string(),
                    summary: format!(
                        "unshared lock-info beside a symlinked cache, from live pid {} ({}); current binaries read the sidecar at the resolved cache location, so this record is ignored{}",
                        obs.info.pid,
                        command,
                        obs.overrun_note()
                            .map(|note| format!("; {note}"))
                            .unwrap_or_default()
                    ),
                    action: format!(
                        "upgrade the aida binary that pid {} runs; the file is removed once its owner exits",
                        obs.info.pid
                    ),
                    safe_heal: false,
                });
            }
            _ if obs.live_overrun().is_some() => {
                push(DoctorFinding {
                    category: "stale-locks".to_string(),
                    id: lock_info_path.display().to_string(),
                    summary: format!(
                        "cache lock held by live pid {} ({}) for {}; {}",
                        obs.info.pid,
                        command,
                        humanize_duration_secs(age_secs.max(0) as u64),
                        obs.overrun_note().unwrap_or_default()
                    ),
                    action: format!(
                        "check pid {}; the lock clears when it finishes (not removed while the owner is alive)",
                        obs.info.pid
                    ),
                    safe_heal: false,
                });
            }
            _ => {}
        }
    }

    // STORY-496: reap stale agent-registry entries. A record under
    // `.aida/agents/` whose creator process is gone, or whose role-enter shell
    // no longer has a live descendant agent session, is confirmed multi-agent
    // drift. trace:STORY-496 | ai:claude trace:BUG-1156 | ai:codex
    {
        let agent_ctx = agent_registry::AgentClassifyContext::new(now, 30, Vec::new());
        for view in agent_registry::list_agent_views(project_root, &agent_ctx) {
            if view.status == agent_registry::AgentStatus::Stale {
                push(DoctorFinding {
                    category: "dead-agents".to_string(),
                    id: format!("{}#{}", view.agent_type, view.pid),
                    summary: format!(
                        "agent `{}` (pid {}) is stale — its registry entry under \
                         .aida/agents/ was never reaped",
                        view.agent_type, view.pid
                    ),
                    action: format!(
                        "remove stale-agent registry entry for {}#{}",
                        view.agent_type, view.pid
                    ),
                    safe_heal: true,
                });
            }
        }
    }

    for lease in &leases {
        let worktree_exists = lease.worktree_path.exists();
        // BUG-511: review-verb leases classify by creator PID, not worktree.
        let state = lease_state_for(lease, &live_sessions, now);
        let pid_dead = lease
            .creator_pid
            .is_some_and(|pid| !process_probe::pid_is_alive(pid));
        let branch_merged = matches!(
            detect_merged_pr_for_branch_via_forge(project_root, &lease.branch),
            PrLookup::Found(_)
        );
        if matches!(state, LeaseState::Stale) || pid_dead || branch_merged {
            push(DoctorFinding {
                category: "stale-leases".to_string(),
                id: lease.id.clone(),
                summary: format!(
                    "{} owns {} on branch `{}` ({})",
                    lease.id,
                    lease.scope,
                    lease.branch,
                    if !worktree_exists {
                        "worktree missing".to_string()
                    } else if pid_dead {
                        "creator pid dead".to_string()
                    } else if branch_merged {
                        "branch merged".to_string()
                    } else {
                        state.label().to_string()
                    }
                ),
                action: format!("save patch if dirty, then end lease {}", lease.id),
                safe_heal: true,
            });
        }

        if matches!(
            store
                .get_requirement_by_spec_id(&lease.scope)
                .map(|req| &req.status),
            Some(RequirementStatus::Approved)
        ) {
            push(DoctorFinding {
                category: "spec-status-drift".to_string(),
                id: lease.scope.clone(),
                summary: format!(
                    "{} is Approved but lease {} is active",
                    lease.scope, lease.id
                ),
                action: "bump spec to In Progress".to_string(),
                safe_heal: true,
            });
        }
        if matches!(
            store
                .get_requirement_by_spec_id(&lease.scope)
                .map(|req| &req.status),
            Some(RequirementStatus::Done)
        ) {
            push(DoctorFinding {
                category: "spec-status-drift".to_string(),
                id: lease.id.clone(),
                summary: format!(
                    "{} is Done but lease {} is still active",
                    lease.scope, lease.id
                ),
                action: format!("confirm and end lease {}", lease.id),
                safe_heal: true,
            });
        }
    }

    for item in &cleanup.sticky_in_progress {
        push(DoctorFinding {
            category: "spec-status-drift".to_string(),
            id: item.spec_id.clone(),
            summary: format!(
                "{} is In Progress but no active lease holds it",
                item.spec_id
            ),
            action: "revert spec to Approved (no active lease found)".to_string(),
            safe_heal: true,
        });
    }

    for item in &cleanup.uncommitted_wip {
        let Some(scope) = item.scope.as_ref() else {
            continue;
        };
        let behind = branch_behind_main(project_root, &item.branch)
            .map(|(n, _)| n)
            .unwrap_or(0);
        if behind >= 100 {
            push(DoctorFinding {
                category: "abandoned-leases".to_string(),
                id: scope.clone(),
                summary: format!(
                    "{} has {} modified file(s) on `{}` {} commits behind main",
                    scope, item.modified_files, item.branch, behind
                ),
                action: "save salvage patch, end lease, remove worktree, re-brief from salvage"
                    .to_string(),
                safe_heal: true,
            });
        }
    }

    for item in &cleanup.branches_ahead_no_pr {
        push(DoctorFinding {
            category: "orphan-branches".to_string(),
            id: item.branch.clone(),
            summary: format!(
                "`{}` is {} commit(s) ahead of main with no open PR",
                item.branch, item.commits_ahead
            ),
            action: "operator decision: open PR, keep, or delete with --yes --force".to_string(),
            safe_heal: false,
        });
    }

    for item in &cleanup.stale_reviewer_leases {
        push(DoctorFinding {
            category: "stale-reviewer-leases".to_string(),
            id: item.lease_id.clone(),
            summary: format!(
                "reviewer lease {} points at merged PR-{}",
                item.lease_id, item.pr_number
            ),
            action: format!("end lease {}", item.lease_id),
            safe_heal: true,
        });
    }

    let lease_worktrees: std::collections::HashSet<std::path::PathBuf> = leases
        .iter()
        .map(|lease| {
            lease
                .worktree_path
                .canonicalize()
                .unwrap_or_else(|_| lease.worktree_path.clone())
        })
        .collect();
    let project_canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    for wt in list_worktrees(project_root) {
        let wt_canon = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
        if wt_canon == project_canon || wt.branch.as_deref() == Some("aida-store") {
            continue;
        }
        if !lease_worktrees.contains(&wt_canon) {
            push(DoctorFinding {
                category: "orphan-worktrees".to_string(),
                id: wt.path.display().to_string(),
                summary: format!("worktree {} has no active lease", wt.path.display()),
                action: "save patch if dirty, then git worktree remove".to_string(),
                safe_heal: true,
            });
        }
    }

    // TASK-570: surface orphan queue entries — rows whose backing spec
    // was deleted ("??? (deleted)" ghosts from auto-queued reviewer items).
    // `aida queue prune --orphaned` (TASK-537) is the direct cleanup verb;
    // the doctor mirrors its detection so `--heal` can route through this
    // category as part of the default "fix what's drifted" sweep.
    // trace:TASK-570 | ai:claude
    let queue_user_id = current_user_id(None);
    let queue_storage = Storage::new(project_root.join(".aida-store"));
    if let Ok(queue_entries) =
        queue_storage.queue_list(&queue_user_id, /* include_completed */ false)
    {
        let existing_ids: std::collections::HashSet<uuid::Uuid> =
            store.requirements.iter().map(|r| r.id).collect();
        for entry in &queue_entries {
            if existing_ids.contains(&entry.requirement_id) {
                continue;
            }
            let role = entry
                .for_role
                .as_deref()
                .map(|r| format!(" [for:{r}]"))
                .unwrap_or_default();
            let note = entry
                .note
                .as_deref()
                .map(|n| format!(" — {n}"))
                .unwrap_or_default();
            push(DoctorFinding {
                category: "orphan-queue-entries".to_string(),
                id: entry.requirement_id.to_string(),
                summary: format!(
                    "queue position {}{} points at deleted spec{}",
                    entry.position, role, note
                ),
                action: "remove orphan queue entry (aida queue prune --orphaned)".to_string(),
                safe_heal: true,
            });
        }
    }

    let pending_briefs = collect_agent_briefs(project_root, None, false)?;
    let lease_scopes: std::collections::HashSet<String> =
        leases.iter().map(|lease| lease.scope.clone()).collect();
    for brief in pending_briefs {
        if lease_scopes.contains(&brief.spec_id) {
            push(DoctorFinding {
                category: "brief-lease-drift".to_string(),
                id: brief.path.display().to_string(),
                summary: format!(
                    "pending brief for {} exists while a lease is active",
                    brief.spec_id
                ),
                action: format!(
                    "ack brief or end/re-brief: aida brief ack {}",
                    brief.path.display()
                ),
                safe_heal: false,
            });
        }
        if matches!(
            store
                .get_requirement_by_spec_id(&brief.spec_id)
                .map(|req| &req.status),
            Some(RequirementStatus::Completed | RequirementStatus::Rejected)
        ) {
            push(DoctorFinding {
                category: "brief-spec-drift".to_string(),
                id: brief.path.display().to_string(),
                summary: format!("pending brief targets terminal spec {}", brief.spec_id),
                action: format!("ack OBE brief {}", brief.path.display()),
                safe_heal: true,
            });
            push(DoctorFinding {
                category: "OBE-briefs".to_string(),
                id: brief.path.display().to_string(),
                summary: format!("brief for {} is obsolete", brief.spec_id),
                action: format!("ack OBE brief {}", brief.path.display()),
                safe_heal: true,
            });
        }
    }

    // STORY-835: Antigravity's project profile is only useful when both its
    // instruction channel (AGENTS.md) and user-profile MCP registration are in
    // place. Skip silently when the wrapper is not installed: a persisted
    // profile may predate installation on this machine.
    if init_cmd::read_enabled_agent_selection(project_root).is_some_and(|s| s.antigravity)
        && init_cmd::antigravity_binary_detected()
    {
        let agents_md = project_root.join("AGENTS.md");
        let agents_md_wired = std::fs::read_to_string(&agents_md)
            .map(|body| {
                body.contains("<!-- AIDA-AUTOGEN-BEGIN -->")
                    && body.contains("<!-- AIDA-AUTOGEN-END -->")
            })
            .unwrap_or(false);
        if !agents_md_wired {
            push(DoctorFinding {
                category: "agents-wiring".to_string(),
                id: "antigravity/agents-md".to_string(),
                summary:
                    "Antigravity profile is enabled but AGENTS.md lacks the AIDA-AUTOGEN block"
                        .to_string(),
                action: "aida scaffold refresh".to_string(),
                safe_heal: false,
            });
        }
        if !init_cmd::antigravity_aida_mcp_registered() {
            push(DoctorFinding {
                category: "agents-wiring".to_string(),
                id: "antigravity/mcp".to_string(),
                summary:
                    "Antigravity profile is enabled but AIDA MCP is not registered in the user profile"
                        .to_string(),
                action: init_cmd::antigravity_aida_mcp_add_command(),
                safe_heal: false,
            });
        }
    }

    // TASK-752: tracked legacy centralized-backend artifacts on a git-canonical
    // project (requirements*.yaml / scaffold-report.html still in the tree while
    // the live store is the orphan aida-store branch). Cheap `git ls-files`
    // scan; the detector self-gates on distributed mode + orphan branch, so it
    // is a no-op on a legacy --centralized project that legitimately uses
    // requirements.yaml as its active store. trace:TASK-752 | ai:claude
    for path in detect_legacy_store_cruft(project_root) {
        push(DoctorFinding {
            category: "legacy-store-cruft".to_string(),
            id: path.clone(),
            summary: format!(
                "tracked legacy-store artifact `{}` on a git-canonical project",
                path
            ),
            action: "git rm the file + gitignore it (live store is the orphan aida-store branch)"
                .to_string(),
            safe_heal: true,
        });
    }

    // BUG-1734: an UNTRACKED legacy store in the project root of a git-canonical
    // project. `legacy-store-cruft` above cannot see it (`git ls-files` lists
    // tracked paths only, and this file is gitignored), and must not: its heal
    // is `git rm`, while this file may be the only copy of another project's
    // data. Diagnostic-only — the category has no heal arm, so `--heal` skips
    // it. trace:BUG-1734 | ai:claude
    for hit in detect_legacy_store_in_root(project_root) {
        push(DoctorFinding {
            category: "legacy-store-in-root".to_string(),
            id: hit.name.clone(),
            summary: legacy_store_in_root_summary(&hit),
            action: legacy_store_in_root_action(&hit),
            safe_heal: false,
        });
    }

    // BUG-563: per-clone RUNTIME files (`.aida/node.toml`, `.aida/dispenser.toml`,
    // `.aida/*.lock`, `.aida/cache.db*`) wrongly TRACKED on the orphan aida-store
    // branch. Each clone's auto-sync rewrites node.toml with its own node id, so
    // every cross-clone store-leg rebase conflicts on it forever. Cheap
    // `git ls-files` scan IN THE `.aida-store` WORKTREE; the detector self-gates
    // on distributed mode + an attached store worktree. trace:BUG-563 | ai:claude
    for path in detect_store_tracked_runtime(project_root) {
        push(DoctorFinding {
            category: "store-tracked-runtime".to_string(),
            id: path.clone(),
            summary: format!(
                "per-clone runtime file `{}` is tracked on the orphan aida-store branch (every cross-clone rebase conflicts on it forever)",
                path
            ),
            action: "git rm --cached the file in the store worktree + gitignore it (per-clone runtime state must stay untracked)"
                .to_string(),
            safe_heal: true,
        });
    }

    // TASK-1547: staged atomic-write files are protected in the store's
    // tracked .gitignore even after all BUG-563 runtime files are untracked.
    // trace:TASK-1547 | ai:codex
    if !detect_store_missing_staging_ignores(project_root).is_empty() {
        push(DoctorFinding {
            category: "store-staging-ignore".to_string(),
            id: ".gitignore".to_string(),
            summary: "store .gitignore is missing the staging-ignore patterns".to_string(),
            action: "append missing atomic-write staging patterns to the store .gitignore and commit on the orphan branch".to_string(),
            safe_heal: true,
        });
    }

    // STORY-1463: a registered `[schedule]` job only ever runs when
    // something invokes `aida schedule tick`; nothing does that by default.
    // Gated on the category filter (like the network-touching `ci` check
    // above) so an unrelated `--category` selection never pays for the
    // crontab probe; `scheduler_driver_doctor_findings` itself gates the
    // shell-out on evidence existing (a repo with no enabled substrate jobs
    // never calls `crontab`). trace:STORY-1463 | ai:claude
    if filter
        .as_deref()
        .is_none_or(|want| want == "scheduler-driver")
    {
        for finding in maintenance_schedule::scheduler_driver_doctor_findings(project_root)? {
            push(finding);
        }
    }

    out.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    Ok(out)
}

/// Locate `binary` on PATH (the portable `command -v`). `None` when absent.
/// Used by the doctor vendor-binary check so a codex-only machine (no claude
/// installed) reports cleanly for the vendors it actually resolves to.
// trace:STORY-762 | ai:claude
pub(crate) fn which_binary(binary: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        // Windows: also try the .exe form.
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{binary}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

/// Canonical `aida doctor` categories and their accepted aliases.
///
/// This is the SINGLE source of truth for category names: both
/// `normalize_doctor_category`'s alias resolution and the user-facing
/// "valid categories" list in its unknown-category error are derived from
/// this table. Adding a category here makes it dispatch correctly AND
/// appear in the error message, with no second edit required — the drift
/// this fixed (a hand-typed error-message list silently falling behind
/// the normalizer) is structurally impossible now.
// trace:BUG-1554 | ai:claude
pub(crate) static DOCTOR_CATEGORY_ALIASES: &[(&[&str], &str)] = &[
    (
        &["merge-hold", "merge-hold-integrity"],
        "merge-hold-integrity",
    ),
    // trace:TASK-1527 | ai:claude
    (
        &["cache-refresh", "refresh-worker", "cache-worker"],
        "cache-refresh",
    ),
    (&["stale-lease", "stale-leases", "leases"], "stale-leases"),
    (
        &["abandoned-lease", "abandoned-leases", "abandoned"],
        "abandoned-leases",
    ),
    (&["brief-lease", "brief-lease-drift"], "brief-lease-drift"),
    (&["brief-spec", "brief-spec-drift"], "brief-spec-drift"),
    (&["spec-status", "spec-status-drift"], "spec-status-drift"),
    (
        &["orphan-worktree", "orphan-worktrees", "worktrees"],
        "orphan-worktrees",
    ),
    (
        &["orphan-branch", "orphan-branches", "branches"],
        "orphan-branches",
    ),
    // TASK-717: stale REMOTE branches (origin/*) verify-and-prune.
    // trace:TASK-717
    (
        &[
            "stale-remote-branch",
            "stale-remote-branches",
            "remote-branch",
            "remote-branches",
            "remote-branch-prune",
        ],
        "stale-remote-branches",
    ),
    // TASK-878: merged Agent-tool isolation worktrees
    // (.claude/worktrees/agent-* on worktree-agent-* branches) that
    // accumulate. trace:TASK-878 | ai:claude
    (
        &[
            "merged-agent-worktree",
            "merged-agent-worktrees",
            "agent-worktree",
            "agent-worktrees",
            "worktree-gc",
            "agent-worktree-gc",
        ],
        "merged-agent-worktrees",
    ),
    // STORY-781: the checked-in `.aida/project.toml`. trace:STORY-781
    (
        &[
            "project-manifest",
            "project-manifests",
            "manifest",
            "manifests",
            "project-metadata",
        ],
        "project-manifest",
    ),
    (
        &[
            "stale-reviewer",
            "stale-reviewer-lease",
            "stale-reviewer-leases",
        ],
        "stale-reviewer-leases",
    ),
    (&["stale-lock", "stale-locks", "locks"], "stale-locks"),
    (
        &["obe-brief", "obe-briefs", "obsolete-briefs"],
        "OBE-briefs",
    ),
    // TASK-570: orphan queue entries — "??? (deleted)" ghosts in
    // `aida queue list`. trace:TASK-570 | ai:claude
    (
        &[
            "orphan-queue",
            "orphan-queue-entry",
            "orphan-queue-entries",
            "queue-orphans",
        ],
        "orphan-queue-entries",
    ),
    // STORY-496: dead-PID agent-registry entries (corpses under
    // `.aida/agents/`). trace:STORY-496 | ai:claude
    (
        &[
            "dead-agent",
            "dead-agents",
            "stale-agent",
            "stale-agents",
            "agents",
        ],
        "dead-agents",
    ),
    // TASK-673: Completed specs git can't corroborate. trace:TASK-673 | ai:claude
    (
        &[
            "completed-without-commit",
            "completed-without-commits",
            "completed-no-commit",
            "uncorroborated-completed",
            "integrity",
        ],
        "completed-without-commit",
    ),
    // TASK-696: ancestor CLAUDE.md/AGENTS.md @-imports resolving outside the
    // project (external-import bleed). trace:TASK-696 | ai:claude
    (
        &[
            "external-import-bleed",
            "external-imports",
            "ancestor-claude",
            "ancestor-bleed",
            "bleed",
        ],
        "external-import-bleed",
    ),
    // TASK-752: tracked legacy centralized-backend artifacts
    // (requirements*.yaml / scaffold-report.html) on a git-canonical
    // project. trace:TASK-752 | ai:claude
    (
        &[
            "legacy-store-cruft",
            "legacy-store",
            "store-cruft",
            "legacy-cruft",
            "requirements-yaml",
        ],
        "legacy-store-cruft",
    ),
    // BUG-1734: an untracked legacy store (`requirements.db` /
    // `requirements.yaml`) in the root of a git-canonical project. Separate
    // from `legacy-store-cruft` because its remedy is relocate, not `git rm`,
    // and the heal dispatcher routes on the category name alone.
    // trace:BUG-1734 | ai:claude
    (
        &[
            "legacy-store-in-root",
            "legacy-store-root",
            "root-legacy-store",
            "requirements-db",
            "stray-store",
        ],
        "legacy-store-in-root",
    ),
    // BUG-563: per-clone runtime files (node.toml / dispenser.toml /
    // *.lock / cache.db*) wrongly tracked on the orphan aida-store branch.
    // trace:BUG-563 | ai:claude
    (
        &[
            "store-tracked-runtime",
            "store-runtime",
            "tracked-runtime",
            "store-node-toml",
            "store-runtime-cruft",
        ],
        "store-tracked-runtime",
    ),
    // TASK-1095: shared branches (trunk + store) holding different tips
    // across configured remotes (github vs gitlab drift).
    // trace:TASK-1095 | ai:claude
    (
        &["remote-drift", "remote-sync", "drift", "remotes"],
        "remote-drift",
    ),
    // trace:STORY-1043 | ai:codex
    (
        &[
            "ci",
            "cross-platform",
            "cross-platform-ci",
            "nightly-red",
            "nightly",
        ],
        "ci",
    ),
    // STORY-762: a vendor this project resolves to (interactive default,
    // headless, or configured TUI) whose CLI binary is missing from PATH.
    // trace:STORY-762 | ai:claude
    (
        &["vendor-binary", "vendor-binaries", "vendor", "vendors"],
        "vendor-binary",
    ),
    // STORY-1127: effective sandbox/approval posture across AIDA agent
    // launch defaults and native Codex config.
    // trace:STORY-1127 | ai:codex
    (
        &[
            "permission-posture",
            "permission-postures",
            "permissions",
            "agent-permissions",
            "sandbox-posture",
        ],
        "permission-posture",
    ),
    (
        &["parent-tag-drift", "parent-tags", "parent-drift"],
        "parent-tag-drift",
    ),
    // BUG-1535: an id that resolves to more than one requirement.
    (
        &[
            "id-collisions",
            "id-collision",
            "ambiguous-ids",
            "ambiguous-id",
            "duplicate-ids",
        ],
        "id-collisions",
    ),
    // A guarded command shape that exceeds its budget on too large a
    // fraction of recent calls, or a budget watching a shape that never
    // ran. trace:STORY-1422 | ai:claude
    (
        &["performance", "perf", "latency", "budgets"],
        "performance",
    ),
    // TASK-1124: deployed vendor prompts/skills (project .claude/.codex +
    // ~/.codex/prompts) drifted from the binary's embedded source templates
    // — rule-delivery-rot. trace:TASK-1124 | ai:claude
    (
        &[
            "scaffold-drift",
            "scaffold",
            "rule-delivery",
            "rule-delivery-rot",
            "delivery-rot",
        ],
        "scaffold-drift",
    ),
    // BUG-1505: review-verdict files that do not match the canonical shape
    // (spelling, legacy keys, no sha, UNKNOWN verdicts). Report-only.
    // trace:BUG-1505 | ai:claude
    (
        &[
            "review-verdicts",
            "review-verdict",
            "verdicts",
            "verdict-schema",
        ],
        "review-verdicts",
    ),
    // TASK-1122: raw machine identity (corporate email/hostname) already in
    // the store despite configured redaction. trace:TASK-1122 | ai:claude
    (
        &["store-scrub", "scrub", "identity-leak", "store-identity"],
        "store-scrub",
    ),
    // STORY-835: enabled agent profile lacks one of its required runtime
    // wiring surfaces (instruction file or MCP registration).
    (
        &["agents-wiring", "agent-wiring", "wiring"],
        "agents-wiring",
    ),
    // BUG-915: linked worktree `.git` file points at shared git metadata
    // that container wrappers must mount.
    (
        &[
            "worktree-container-gitdir",
            "container-gitdir",
            "container-worktree",
            "devcontainer-gitdir",
            "worktree-gitdir",
        ],
        "worktree-container-gitdir",
    ),
    // TASK-1313: round-trip artifacts in stored spec text — a swallowed TOON
    // row header or a UTF-8-as-Latin-1 mojibake sequence left by a
    // read-modify-write through rendered `aida show` output.
    (
        &[
            "round-trip-artifacts",
            "round-trip",
            "round-trip-artifact",
            "mojibake",
            "toon-leak",
        ],
        "round-trip-artifacts",
    ),
    // STORY-1463: a registered `[schedule]` job that nothing ever ticks —
    // no crontab driver installed, or an installed one that isn't actually
    // firing (a substrate job overdue by more than 2x its interval).
    (
        &[
            "scheduler-driver",
            "scheduler",
            "schedule-driver",
            "schedule-tick",
            "cron",
        ],
        "scheduler-driver",
    ),
    // STORY-1367: free disk headroom on the filesystem holding the project
    // root, checked against a configurable floor
    // (`[doctor.disk_headroom] min_free_gib`, default 60). Zero-token,
    // substrate-only — the first job STORY-1367 registers to catch the class
    // of incident where a full disk silently kills a drain.
    // trace:STORY-1367 | ai:claude
    (
        &[
            "disk-headroom",
            "disk-space",
            "disk",
            "headroom",
            "free-space",
        ],
        "disk-headroom",
    ),
    // BUG-1700: the folder-trust gate on a freshly created worktree. Claude
    // Code stops an INTERACTIVE launch in a directory it has never seen behind
    // a "Quick safety check" modal, so a fresh per-spec worktree strands
    // `aida agent new claude --spec <ID>` in a PTY nobody is watching. Trust
    // inherits from a parent directory, so `[worktree_pool] worktree_parent`
    // plus one operator grant retires the whole class. Read-only: AIDA never
    // writes ~/.claude.json. trace:BUG-1700 | ai:claude
    (
        &[
            "agent-launch",
            "agent-trust",
            "folder-trust",
            "worktree-trust",
        ],
        "agent-launch",
    ),
    // STORY-1462: runaway-seat watchdog — per-session wake-rate, token-rate,
    // repeated-injected-prompt, idle-ratio, context-ceiling, compaction and
    // model-side-mail-poll anomalies read from on-disk session transcripts,
    // plus the project's trailing-24h token spend against `[watchdog]`
    // thresholds. Zero-token, substrate-only. trace:STORY-1462 | ai:claude
    (
        &[
            "runaway-seats",
            "runaway-seat",
            "runaway",
            "watchdog",
            "seat-watchdog",
        ],
        "runaway-seats",
    ),
    // BUG-1738: the pre-push mirror hook is generated once, at
    // `aida remote mirror` time, and never re-generated — so a generator fix
    // (BUG-1706's --dry-run forwarding) silently never reaches hooks
    // installed before it. Compares the installed hook against the current
    // generator; recognizes only AIDA's own banner, so a custom pre-push
    // hook is never flagged. Report-only. trace:BUG-1738 | ai:claude
    (
        &[
            "mirror-hook-drift",
            "mirror-hook",
            "pre-push-hook",
            "hook-drift",
        ],
        "mirror-hook-drift",
    ),
    // BUG-2009: AIDA git hooks git would not run as this build ships them —
    // no executable bit (git skips the hook silently) or a stale generated
    // body (store writes refuse it). Report-only; the remedy is
    // `aida scaffold upgrade --only git-hooks`. trace:BUG-2009 | ai:claude
    (
        &[
            "git-hooks",
            "aida-hooks",
            "hook-mode",
            "stale-hooks",
            "non-executable-hooks",
        ],
        "git-hooks",
    ),
    // TASK-1330: identity hygiene — when `[identity] allowed_emails` is
    // configured, report a git identity outside the allowlist and a pre-push
    // hook that does not run the fail-closed identity gate. Report-only.
    // trace:TASK-1330 | ai:claude
    (
        &[
            "identity",
            "identity-hygiene",
            "allowed-emails",
            "email-allowlist",
        ],
        "identity",
    ),
    // STORY-1648: uncommitted edits, untracked files, an off-main HEAD, or
    // origin divergence in the primary checkout. Report-only.
    // trace:STORY-1648 | ai:claude
    (
        &[
            "main-checkout-drift",
            "main-checkout",
            "primary-checkout",
            "checkout-drift",
        ],
        "main-checkout-drift",
    ),
];

/// `id-collisions` doctor findings: one per id that resolves to more than one
/// requirement, naming every candidate and the handle that reaches it, plus a
/// count check comparing UNIQUE ids to store objects. Row count vs file count
/// is not that check: seven duplicated rows over seven double-claimed ids
/// made the two equal by coincidence.
// trace:BUG-1535 | ai:claude
pub(crate) fn id_collision_findings(
    store: &aida_core::models::RequirementsStore,
) -> Vec<DoctorFinding> {
    let mut out: Vec<DoctorFinding> =
        aida_core::id_collisions::find_requirement_id_collisions(store.requirements.iter())
            .into_iter()
            .map(|collision| {
                let lines = aida_core::id_collisions::describe_candidates(
                    &collision.id,
                    &collision.candidates,
                );
                DoctorFinding {
                    category: "id-collisions".to_string(),
                    id: collision.id.clone(),
                    summary: format!(
                        "{} resolves to {} requirements: {}",
                        collision.id,
                        collision.candidates.len(),
                        lines.join("; ")
                    ),
                    action: "decide which object keeps the id, then give the other a free \
                             agreed id (not auto-healed: renumbering is a data migration)"
                        .to_string(),
                    safe_heal: false,
                }
            })
            .collect();
    let objects = store.requirements.len();
    let unique: std::collections::HashSet<String> = store
        .requirements
        .iter()
        .map(|r| r.display_id().to_ascii_uppercase())
        .collect();
    if unique.len() != objects {
        out.push(DoctorFinding {
            category: "id-collisions".to_string(),
            id: "store-count".to_string(),
            summary: format!(
                "{objects} objects in the store but only {} unique displayed ids \
                 ({} object(s) display an id another object also displays)",
                unique.len(),
                objects - unique.len()
            ),
            action: "resolve the id collisions listed above".to_string(),
            safe_heal: false,
        });
    }
    out
}

pub(crate) fn normalize_doctor_category(raw: &str) -> Result<String> {
    let s = raw.trim().to_ascii_lowercase().replace('_', "-");
    for (aliases, canonical) in DOCTOR_CATEGORY_ALIASES {
        if aliases.contains(&s.as_str()) {
            return Ok((*canonical).to_string());
        }
    }
    let valid = DOCTOR_CATEGORY_ALIASES
        .iter()
        .map(|(_, canonical)| *canonical)
        .collect::<Vec<_>>()
        .join(", ");
    anyhow::bail!("unknown doctor category `{}` (valid: {})", s, valid);
}

#[cfg(test)]
#[path = "tests/story_1463_scheduler_tick_cron_tests.rs"]
mod story_1463_scheduler_tick_cron_tests;

/// TASK-1089 (criterion 2): an explicit opt-out. A Completed spec tagged
/// `doctor:no-code` legitimately produces no code commit — an ops/cleanup task
/// whose deliverable is an action (e.g. a prune run), or a decision/doc whose
/// deliverable is not code — so the completed-without-commit scan skips it.
/// Opt-in by tag (not derivable from type alone: `task` covers both code work
/// and ops chores).
// trace:TASK-1089 | ai:claude
pub(crate) fn completed_without_commit_opted_out(tags: &std::collections::HashSet<String>) -> bool {
    tags.contains("doctor:no-code")
}

pub(crate) fn completed_without_commit_expects_code(req_type: &RequirementType) -> bool {
    // trace:TASK-755 | ai:codex
    matches!(
        req_type,
        RequirementType::Functional
            | RequirementType::NonFunctional
            | RequirementType::System
            | RequirementType::User
            | RequirementType::ChangeRequest
            | RequirementType::Bug
            | RequirementType::Story
            | RequirementType::Task
    )
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CompletedWithoutCommitScan {
    pub(crate) findings: Vec<DoctorFinding>,
    pub(crate) hidden_older: usize,
}

/// TASK-673: spec-graph ⟂ git tripwire. Returns a `completed-without-commit`
/// finding for every spec in `Completed` status that NO commit on the default
/// CODE branch references (by `(SPEC-ID)` subject/trailer) and that NO tracked
/// file at that branch carries a `// trace:SPEC-ID`. A Completed spec with zero
/// corroboration means the requirement graph is asserting work git has never
/// seen — the defense-in-depth companion to the BUG-449 MCP gate.
///
/// CRITICAL: scans the default CODE branch only (resolved via
/// `resolve_default_branch_ref`), never the orphan `aida-store` branch — whose
/// `update SPEC-NNN` bookkeeping commits mention the bare id and would mask
/// every violation. Plan commits are skipped (their trailer names planned, not
/// delivered, specs). `Done` specs are deliberately excluded: that is the
/// queue's "awaiting commit" transient, surfaced separately, so this check
/// targets only the harder Completed-without-corroboration case. Read-only.
///
/// `since` (a git ref/tag or ISO date) exempts specs last modified before that
/// point, quieting noise on legacy history predating trace conventions. The
/// reference scan itself always walks the FULL default-branch history so an old
/// corroborating commit is never missed. trace:TASK-673 | ai:claude
pub(crate) fn scan_completed_without_commit(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
    since: Option<&str>,
) -> Vec<DoctorFinding> {
    scan_completed_without_commit_with_options(project_root, store, since, false).findings
}

/// Build the corroboration set: every spec-id any commit message references.
/// Scans the FULL message (subject + body) of each commit — a squash-merge
/// concatenates each child commit's `(SPEC-ID)` trailer into the BODY, so a
/// subject-only scan false-flags every squash-merged spec (BUG-606). Unions the
/// subject-trailer extractor (`(SPEC-ID)` / leading `SPEC-ID:`) with the
/// body-trailer extractor (skips code-like lines per BUG-412). Plan commits are
/// skipped — they name PLANNED, not shipped, specs (BUG-426). Pure + testable.
/// trace:BUG-606 | ai:claude
/// Full-history corroboration set for the default branch: every spec-id any
/// commit message references (subject + body, squash-body-aware). The
/// completed-without-commit and claimed-done-divergence detectors both rely on
/// this being COMPLETE — an incomplete set false-flags shipped specs.
/// trace:BUG-606 | ai:claude
pub(crate) fn referenced_spec_ids_on_default_branch(
    project_root: &std::path::Path,
) -> std::collections::HashSet<String> {
    let Some(default) = detect_default_branch_ref(project_root) else {
        return std::collections::HashSet::new();
    };
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", "-z", "--pretty=format:%B", &default])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            referenced_spec_ids_from_messages(text.split('\0'))
        }
        _ => std::collections::HashSet::new(),
    }
}

pub(crate) fn referenced_spec_ids_from_messages<'a>(
    messages: impl IntoIterator<Item = &'a str>,
) -> std::collections::HashSet<String> {
    // Liberal token match: a spec-id can appear ANYWHERE in the message — a
    // clean `(TASK-1)` trailer, a squash body `… (BUG-110) (#144)`, a
    // punctuated `(BUG-109).`, or mid-prose `(ADR-3 / TASK-647), so …`. For a
    // CORROBORATION check ("is there ANY git evidence this spec exists"),
    // matching the id token anywhere is correct and conservative — a benign
    // over-match just means we don't false-flag, which the check explicitly
    // prefers over a missed reference. Anchored to UPPERCASE 2+-letter prefixes
    // so lowercase prose ("step-1") doesn't pollute; coincidental tokens like
    // `UTF-8` are harmless (no spec collides). trace:BUG-606 | ai:claude
    let re =
        regex::Regex::new(r"\b[A-Z]{2,}-[0-9]+(?:-[0-9]+)?\b").expect("valid spec-id token regex");
    // A bare `update SPEC-ID` subject is the `aida edit` orphan-store
    // bookkeeping commit — it exists for EVERY edited spec, so counting it as
    // corroboration would mask every violation (STORY-462). trace:BUG-606
    let bookkeeping_re = regex::Regex::new(r"(?i)^update\s+[A-Za-z]+-[0-9]+(?:-[0-9]+)?$")
        .expect("valid bookkeeping-subject regex");
    let mut referenced = std::collections::HashSet::new();
    for message in messages {
        let subject = message
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        // Plan commits name PLANNED, not shipped, specs (BUG-426); bare
        // `update SPEC-ID` is store bookkeeping (STORY-462) — neither
        // corroborates a completion.
        if subject.is_empty() || is_plan_commit_subject(subject) || bookkeeping_re.is_match(subject)
        {
            continue;
        }
        for m in re.find_iter(message) {
            referenced.insert(m.as_str().to_ascii_uppercase());
        }
    }
    referenced
}

pub(crate) fn scan_completed_without_commit_with_options(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
    since: Option<&str>,
    include_older: bool,
) -> CompletedWithoutCommitScan {
    use std::process::Command as PCmd;

    let completed: Vec<&aida_core::models::Requirement> = store
        .requirements
        .iter()
        .filter(|r| matches!(r.status, RequirementStatus::Completed))
        .filter(|r| completed_without_commit_expects_code(&r.req_type))
        // TASK-1089: explicit `doctor:no-code` opt-out for ops/cleanup work.
        .filter(|r| !completed_without_commit_opted_out(&r.tags))
        .filter(|r| r.spec_id.is_some() || r.agreed_id.is_some())
        .collect();
    if completed.is_empty() {
        return CompletedWithoutCommitScan::default();
    }

    // No resolvable default branch (e.g. a store with no code repo) → we cannot
    // corroborate, so stay silent rather than flag every Completed spec.
    let Some(default_ref) = resolve_default_branch_ref(project_root) else {
        return CompletedWithoutCommitScan::default();
    };

    let git = |args: &[&str]| -> Option<String> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };

    // ---- Reference set: spec ids named by commits on the default branch. ----
    // Full history (no --max-count) so an old corroborating commit is never
    // missed — the `since` cutoff bounds *flagging*, not the reference scan.
    // BUG-606: scan the FULL message (%B), not just the subject (%s). A
    // squash-merge keeps only the PR title as its subject and concatenates each
    // child commit's `(SPEC-ID)` trailer into the BODY, so a subject-only scan
    // false-flagged every squash-merged Completed spec (observed: ~1464 false
    // positives — specs with 10+ body-referencing commits reported as
    // "no commit referencing it"). `-z` NUL-delimits commits so multi-line
    // bodies parse unambiguously. trace:BUG-606 | ai:claude
    let mut referenced: std::collections::HashSet<String> =
        match git(&["log", "-z", "--pretty=format:%B", &default_ref]) {
            Some(log) => referenced_spec_ids_from_messages(log.split('\0')),
            None => std::collections::HashSet::new(),
        };

    // ---- Reference set, second source: `// trace:SPEC-ID` in tracked files. ----
    // `git grep <ref>` scans only tracked blobs at the default branch tree —
    // exactly "a tracked file carries the trace". Unioning trace hits keeps the
    // reference set as complete as possible so the check stays conservative
    // (false positives erode trust faster than the rare miss).
    if let Some(grep) = git(&[
        "grep",
        "-hoI",
        "-E",
        r"trace:[A-Za-z]+-[0-9]+(-[0-9]+)?",
        &default_ref,
    ]) {
        for line in grep.lines() {
            if let Some(id) = parse_trace_id_token(line) {
                referenced.insert(id);
            }
        }
    }

    // ---- Optional legacy-exemption cutoff. ----
    // `aida doctor` validates `--since` up front and fails on a bad value;
    // this scan also backs `aida status`, which stays best-effort.
    // trace:BUG-1622 | ai:claude
    let cutoff = since.and_then(|s| resolve_completed_since_cutoff(project_root, s).ok());

    let mut findings = Vec::new();
    let mut hidden_older = 0;
    for req in completed {
        let spec_id = req.spec_id.as_deref().or(req.agreed_id.as_deref());
        let Some(spec_id) = spec_id else { continue };
        let referenced_by_id = referenced.contains(&spec_id.to_ascii_uppercase())
            || req
                .agreed_id
                .as_deref()
                .map(|a| referenced.contains(&a.to_ascii_uppercase()))
                .unwrap_or(false)
            || req
                .spec_id
                .as_deref()
                .map(|s| referenced.contains(&s.to_ascii_uppercase()))
                .unwrap_or(false);
        if referenced_by_id {
            continue;
        }
        // TASK-1089: compare CREATED_AT, not modified_at. The git-canonical
        // migration rewrote every spec's modified_at to a recent bulk timestamp,
        // so a modified_at cutoff hid nothing (353 pre-migration completions all
        // read as "recent"). created_at reflects the spec's true age and was NOT
        // migration-reset, so a completed spec BORN before the recent window is
        // legacy history — exempt it. A genuinely-recent stranded completion
        // (created in-window, no corroborating commit) still flags.
        // trace:TASK-1089 | ai:claude
        if let Some(cut) = cutoff {
            if !include_older && req.created_at < cut {
                hidden_older += 1;
                continue;
            }
        }
        findings.push(DoctorFinding {
            category: "completed-without-commit".to_string(),
            id: spec_id.to_string(),
            summary: format!(
                "Completed spec {spec_id} has no commit referencing it on {default_ref} \
                 (and no tracked trace comment) — git cannot corroborate the completion"
            ),
            action: format!(
                "land a commit carrying `({spec_id})` then `aida pull`, or, once you have \
                 checked the work did not land, reopen it yourself \
                 (`aida edit {spec_id} --status done --force`)"
            ),
            // BUG-1637: the doctor never reopens a terminal status itself (its
            // heal only reports); the reopen is a person's recorded edit. The
            // category stays force-gated so the heal never runs unattended.
            // trace:BUG-1637 | ai:claude
            safe_heal: false,
        });
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    CompletedWithoutCommitScan {
        findings,
        hidden_older,
    }
}
