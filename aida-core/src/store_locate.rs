//! Distributed AIDA store location — resolves the git-canonical orphan-store
//! worktree path (and, from it, the project root) from a starting directory.
//!
//! This is the CANONICAL resolver: it's what `aida-cli`'s command dispatch
//! (and every `aida mailbox send`) ultimately routes through to find the
//! store, and thus the project root (`store_path.parent()`). It lives here in
//! `aida-core`, not `aida-cli`, so non-CLI callers that can't depend on the
//! bin-only `aida-cli` crate — e.g. `aida-tui`'s mail scope — resolve to the
//! SAME root the CLI does, instead of maintaining their own ad-hoc walk that
//! can silently diverge in a nested-git-worktree layout (BUG-331 territory).
// trace:TASK-1141 | ai:claude

use std::path::{Path, PathBuf};

/// BUG-1598: every walk-up-looking-for-`.aida` resolver in this codebase
/// (`aida-core`, `aida-cli-lib`, `aida-tui` all have one) must stop BEFORE
/// treating an OS temp directory as a legitimate project root. The system
/// temp dir is shared by every process on the machine — a `mktemp -d`/
/// `tempfile::tempdir()` fixture one or two levels under it has NO ancestor
/// of its own between itself and the temp root, so an unbounded walk-up
/// reaches it in a couple of hops. If anything (a stray manual `aida init`
/// run from `/tmp`, another test, another concurrent process on a shared dev
/// box) ever left a real `.aida/` sitting there, every later "isolated"
/// tempdir-rooted test or command would silently ADOPT that shared `.aida`
/// — sharing its cache, corrupting it with unrelated data ("duplicate AIDA
/// spec IDs detected"), and leaving `.aida`/`.aida-store` behind at the temp
/// root for the next run to trip over too.
///
/// A project literally rooted AT a temp directory (`aida init` run directly
/// in `/tmp`) is a case this deliberately does not support: every walk-up
/// listed above treats a temp root as "no project here", consistently, the
/// same as any other directory with no `.aida`. There is no user-facing
/// message for it — from the walk-up's point of view it's indistinguishable
/// from "not an AIDA project yet". A walk-up may still find and adopt a
/// project BELOW a temp root (e.g. `/tmp/real-project/.aida`); only the temp
/// root itself is off-limits. This holds even when `$TMPDIR` is explicitly
/// pointed at some other directory (a sandboxed session, a CI runner's
/// `RUNNER_TEMP`) — THAT directory becomes a guarded root too (see
/// `real_temp_roots`), so it likewise can never be adopted as a project
/// root, for the same reason: it's shared with whatever else in that
/// environment also honors `$TMPDIR`.
///
/// The guarded call sites, as of this writing (keep this list current when
/// adding a new `.aida`-looking-for walk-up — that's the "every walk-up"
/// claim above, made checkable):
/// - `aida-core`: [`detect_distributed_store_from`],
///   `CachedGitBackend::default_cache_path`.
/// - `aida-cli-lib`: `distributed_mode_declared_from`,
///   `find_aida_project_root_from`, `config_toml_exists_upward` (BUG-1574's
///   fail-open carve-out — see its doc comment for why disagreeing with
///   `detect_distributed_store_from` here specifically flips a refusal the
///   wrong way), `parent_project_root_for_session`, `statusline_project_root`
///   (backs `aida role` / `aida statusline` and the init tail —
///   `scaffold_starter_roles`, `refresh_agent_packs`,
///   `register_project_in_global_registry`; falls back to cwd on a temp
///   root, same as "nothing found").
/// - `aida-tui`: `lib.rs::ensure_project_context`,
///   `launcher.rs::ensure_project_context`, `dashboard.rs::project_root_of`,
///   `redesign/mail.rs::resolve_project_root` (its `.git`/`.aida` fallback;
///   its primary path already routes through the guarded
///   `resolve_project_root_from`), `redesign/store.rs::resolve_store_path`,
///   `config.rs::find_project_root`.
///
/// The real temp roots ([`real_temp_roots`]) are `std::env::temp_dir()` (so
/// `$TMPDIR`, a sandboxed session's redirect, or CI's `RUNNER_TEMP`-backed
/// override is always covered) PLUS the fixed, well-known Unix roots
/// (`/tmp`, `/var/tmp`, `/private/tmp` — macOS symlinks `/tmp` to
/// `/private/tmp`, and some sandboxes point `TMPDIR` at a per-session
/// directory while leaving the literal `/tmp` fully populated and
/// world-writable). Guarding only `temp_dir()` would miss `/tmp` itself
/// whenever `$TMPDIR` points elsewhere — exactly the sandboxed/CI case this
/// exists for.
// trace:BUG-1598 | ai:claude
pub fn is_system_temp_dir(path: &Path) -> bool {
    is_in_canonical_roots(path, canonical_temp_roots())
}

/// The fixed set of directories that must never be adopted as a project
/// root: `std::env::temp_dir()` plus, on Unix, the well-known system temp
/// roots regardless of what `$TMPDIR` currently points at. See
/// [`is_system_temp_dir`] for why each of these is included.
// trace:BUG-1598 | ai:claude
pub fn real_temp_roots() -> Vec<PathBuf> {
    let mut roots = vec![std::env::temp_dir()];
    #[cfg(unix)]
    {
        roots.push(PathBuf::from("/tmp"));
        roots.push(PathBuf::from("/var/tmp"));
        roots.push(PathBuf::from("/private/tmp"));
    }
    roots
}

/// [`real_temp_roots`], canonicalized once and cached for the life of the
/// process. `std::env::temp_dir()` (and thus the real root set) does not
/// change during a running process in practice, so every walk-up that
/// guards against the REAL temp roots via [`is_system_temp_dir`]
/// canonicalizes the root set ONCE total for the whole process, never once
/// per ancestor level. Perf follow-up to the round-1 fix, which
/// canonicalized `roots` freshly on every [`is_temp_root_in`] call (i.e. on
/// every level of every walk-up).
// trace:BUG-1598 | ai:claude
pub fn canonical_temp_roots() -> &'static [PathBuf] {
    static ROOTS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| canonicalize_roots(&real_temp_roots()))
}

/// Canonicalize each of `roots`, falling back to the raw path per-element
/// when `canonicalize` fails (e.g. a listed root like `/private/tmp` that
/// doesn't exist on this host). The "once per walk" half of the guard: a
/// `*_with_roots` walk-up loop calls this ONCE, before iterating, then
/// checks each ancestor with [`is_in_canonical_roots`] (which canonicalizes
/// only the path side).
// trace:BUG-1598 | ai:claude
pub fn canonicalize_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .map(|r| r.canonicalize().unwrap_or_else(|_| r.clone()))
        .collect()
}

/// Is `path` one of `canonical_roots`, which the caller has ALREADY
/// canonicalized (via [`canonicalize_roots`] or [`canonical_temp_roots`])?
/// Canonicalizes only `path` — the cheap, per-ancestor-level half of the
/// guard. Falls back to the raw path when `path` doesn't (yet) exist, e.g.
/// canonicalize fails for it.
// trace:BUG-1598 | ai:claude
pub fn is_in_canonical_roots(path: &Path, canonical_roots: &[PathBuf]) -> bool {
    let canon_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canonical_roots.contains(&canon_path)
}

/// Is `path` one of `roots`? Canonicalizes both sides before comparing (so a
/// symlinked root, e.g. macOS `/tmp` -> `/private/tmp`, still matches).
/// Convenience for a ONE-OFF check (a single test assertion, a call site
/// that isn't a loop) — canonicalizes `roots` fresh on every call, so a
/// walk-up loop must NOT call this per ancestor level. Use
/// [`canonicalize_roots`] once before the loop plus [`is_in_canonical_roots`]
/// per level instead (see any `*_with_roots` walk-up in this codebase for
/// the pattern).
///
/// Roots are passed in rather than hardcoded so callers — and tests — can
/// exercise the guard against a fake root without mutating process-global
/// env state (`TMPDIR`) or touching the real, shared system temp dir.
// trace:BUG-1598 | ai:claude
pub fn is_temp_root_in(path: &Path, roots: &[PathBuf]) -> bool {
    is_in_canonical_roots(path, &canonicalize_roots(roots))
}

/// Classification of a candidate store path: either it resolves to a usable
/// store, or it's set-but-unusable with a specific reason. Lets the
/// resolution core stay pure/unit-testable while a caller-side env wrapper
/// (e.g. `aida-cli`'s `AIDA_STORE` handling) decides whether to print a
/// fall-through notice.
// trace:BUG-567 | ai:claude
pub enum StoreOverride {
    /// The path points at a valid git-canonical store (canonicalized).
    Usable(PathBuf),
    /// The path was set but unusable; carries a human-readable reason so a
    /// caller can name WHY it fell through.
    Unusable { reason: String },
}

impl StoreOverride {
    pub fn is_some(&self) -> bool {
        matches!(self, StoreOverride::Usable(_))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, StoreOverride::Unusable { .. })
    }

    pub fn expect(self, msg: &str) -> PathBuf {
        match self {
            StoreOverride::Usable(p) => p,
            StoreOverride::Unusable { reason } => panic!("{msg}: unusable ({reason})"),
        }
    }
}

/// Classify a candidate store path: `Usable` (canonicalized) when it exists
/// and holds an `objects/` subdirectory (the per-spec YAML tree GitBackend
/// manages), else `Unusable` with a specific reason. Validation is
/// deliberately strict — a typo'd or not-yet-created path is `Unusable`
/// rather than erroring, so a stale override never silently misdirects
/// writes.
// trace:SPIKE-48 trace:BUG-567 | ai:claude
pub fn classify_store_path(path: &Path) -> StoreOverride {
    if !path.is_dir() {
        return StoreOverride::Unusable {
            reason: "not a directory".to_string(),
        };
    }
    if !path.join("objects").is_dir() {
        return StoreOverride::Unusable {
            reason: "missing an `objects/` subdirectory".to_string(),
        };
    }
    StoreOverride::Usable(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
}

/// Is a git-canonical store physically attached at `<project_root>/.aida-store`?
/// True when `.aida-store/objects/` is a directory — the hallmark of an
/// attached orphan-store worktree, resolved through a symlink too.
// trace:BUG-433 | ai:claude
pub fn is_store_attached(project_root: &Path) -> bool {
    project_root.join(".aida-store").join("objects").is_dir()
}

/// BUG-331: resolve `<main-worktree>/<rel_store>` from inside a linked git
/// worktree.
///
/// `git rev-parse --git-common-dir` yields the SHARED `.git` directory (e.g.
/// `/main/.git`) regardless of which worktree we're in; its parent is the main
/// worktree. The `.aida-store/` orphan-branch worktree is created (and
/// gitignored) only there, so a linked/nested worktree (a sibling created via
/// `git worktree add`, or a nested one under e.g. `.claude/worktrees/`) must
/// look here instead of falling back to centralized mode — or, for a caller
/// like `aida-tui`'s mail scope, silently reading/writing the wrong project
/// root's `.aida/mailbox/`.
///
/// Returns `None` when not in a git repo, git is unavailable/old, or the
/// store is genuinely absent — callers then fall through to their existing
/// resolution.
// trace:BUG-331 | ai:claude
fn main_worktree_store(current: &Path, rel_store: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(current)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let common_dir = Path::new(raw);
    let common_dir = if common_dir.is_absolute() {
        common_dir.to_path_buf()
    } else {
        current.join(common_dir)
    };
    let common_dir = common_dir.canonicalize().ok()?;
    let main_worktree = common_dir.parent()?;
    let store = main_worktree.join(rel_store);
    if store.exists() && store.is_dir() {
        Some(store)
    } else {
        None
    }
}

/// Walk up from `start` and return the store path declared by the nearest
/// ancestor's `.aida/config.toml` (its `store_path` key), resolved relative to
/// the directory holding that config — not to `start`, so calling from a
/// nested subdir still resolves correctly.
///
/// BUG-331: when the locally-resolved `store_path` doesn't exist (the
/// hallmark of a linked/nested worktree, which has the tracked
/// `.aida/config.toml` but not the gitignored `.aida-store/` worktree — that
/// one only lives in the MAIN worktree), fall back to resolving it at the main
/// worktree via `git rev-parse --git-common-dir` instead of giving up.
// trace:BUG-57 trace:BUG-331 | ai:claude
pub fn detect_distributed_store_from(start: &Path) -> Option<PathBuf> {
    detect_distributed_store_from_with_roots(start, &real_temp_roots())
}

/// [`detect_distributed_store_from`], parameterized on the temp roots to
/// guard against. Factored out so a test can exercise the guard against a
/// fake root without mutating `TMPDIR` or touching the real, shared system
/// temp dir.
// trace:BUG-1598 | ai:claude
fn detect_distributed_store_from_with_roots(
    start: &Path,
    temp_roots: &[PathBuf],
) -> Option<PathBuf> {
    // Canonicalize the root set ONCE, before the loop — not on every
    // ancestor level (`is_in_canonical_roots` below only canonicalizes the
    // per-level `current`, which can't be hoisted out of the loop).
    let canonical_roots = canonicalize_roots(temp_roots);
    let mut current = start;
    loop {
        // BUG-1598: never walk INTO a temp root and adopt whatever
        // `.aida/config.toml` some unrelated process left sitting there.
        if is_in_canonical_roots(current, &canonical_roots) {
            return None;
        }
        let config_path = current.join(".aida").join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            // store_path is relative to the directory containing config.toml,
            // not to the original start dir — otherwise a nested-dir caller
            // would resolve the store against the wrong base.
            // Try every candidate in order; the first that resolves wins.
            // An empty value joins to `current` itself, as it always has.
            // trace:BUG-1650 | ai:claude
            for val in store_path_candidates(&content) {
                let store_path = current.join(&val);
                if store_path.exists() && store_path.is_dir() {
                    return Some(store_path);
                }
                if let Some(main_store) = main_worktree_store(current, &val) {
                    return Some(main_store);
                }
            }
        }
        match current.parent() {
            Some(p) => current = p,
            None => return None,
        }
    }
}

/// The `store_path` candidates in a `.aida/config.toml` body, in the order a
/// resolver should try them — the ONE shared reader behind every store
/// resolver (`aida-core`, `aida-cli-lib`, `aida-tui`). A resolver takes the
/// first candidate that names an existing store.
///
/// 1. When the body is valid TOML, the parsed `[deployment].store_path`, or
///    failing that a top-level `store_path`. This reads every form a writer
///    can produce ([`crate::toml_quote::toml_string`] escapes, `'...'`
///    literals, multi-line strings) exactly.
/// 2. Then the legacy line-scan values, in file order, exactly as the old
///    readers produced them (text after the first `=`, surrounding quotes
///    trimmed). This keeps two legacy shapes resolving to the same store as
///    before: a `store_path` in any other table, and a pre-BUG-1649 value with
///    raw backslashes (`..\team-store`) that now parses as valid TOML with
///    `\t` applied as an escape, so only the raw text names the real
///    directory.
/// 3. Last, when the body is not valid TOML, each matching line parsed on its
///    own (an escaped value in an otherwise broken file).
///
/// Duplicates are dropped. An empty value is kept: it is a candidate that
/// joins to the config's own directory, as it always was.
// trace:BUG-1650 | ai:claude
pub fn store_path_candidates(config_toml: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |v: String| {
        if !out.contains(&v) {
            out.push(v);
        }
    };
    let parsed = toml::from_str::<toml::Table>(config_toml).ok();
    if let Some(v) = parsed.as_ref().and_then(store_path_from_table) {
        push(v);
    }
    for line in config_toml.lines() {
        if let Some(v) = legacy_store_path_from_line(line) {
            push(v);
        }
    }
    if parsed.is_none() {
        for line in config_toml.lines() {
            if let Some(v) = parsed_store_path_from_line(line) {
                push(v);
            }
        }
    }
    out
}

/// `[deployment].store_path`, else a top-level `store_path`. No other table
/// is consulted here; those only reach a resolver through the legacy scan.
fn store_path_from_table(table: &toml::Table) -> Option<String> {
    let as_string = |v: &toml::Value| v.as_str().map(str::to_owned);
    table
        .get("deployment")
        .and_then(|d| d.get("store_path"))
        .and_then(as_string)
        .or_else(|| table.get("store_path").and_then(as_string))
}

/// The pre-BUG-1650 reader, unchanged: a trimmed line starting with
/// `store_path`, the text between the first and second `=`, then surrounding
/// `"` and `'` trimmed.
fn legacy_store_path_from_line(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("store_path")?;
    let val = rest.split('=').nth(1)?;
    Some(val.trim().trim_matches('"').trim_matches('\'').to_owned())
}

/// A `store_path = ...` line parsed as TOML on its own.
fn parsed_store_path_from_line(line: &str) -> Option<String> {
    let table = toml::from_str::<toml::Table>(line.trim()).ok()?;
    table.get("store_path")?.as_str().map(str::to_owned)
}

/// Detect the distributed store from the process's current directory,
/// honoring the `AIDA_STORE` env override when it points at a usable store.
///
/// This mirrors `aida-cli`'s `detect_distributed_store()` resolver — minus
/// the diagnostic stderr notice for a set-but-unusable `AIDA_STORE`, which
/// stays a CLI-side concern (a background/TUI caller shouldn't print to a
/// terminal it doesn't own). A caller that wants that notice should classify
/// the env var itself via [`classify_store_path`].
// trace:SPIKE-48 | ai:claude
pub fn detect_distributed_store() -> Option<PathBuf> {
    if let Some(p) = env_store_override() {
        return Some(p);
    }
    let cwd = std::env::current_dir().ok()?;
    detect_distributed_store_from(&cwd)
}

/// `AIDA_STORE`, classified and unwrapped to `Some(path)` only when usable —
/// the env-override half of [`detect_distributed_store`], factored out so a
/// `start`-parameterized caller (like [`resolve_project_root_from`]) can
/// combine it with an explicit walk-up dir instead of the process cwd.
fn env_store_override() -> Option<PathBuf> {
    let raw = std::env::var("AIDA_STORE").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    match classify_store_path(Path::new(raw)) {
        StoreOverride::Usable(p) => Some(p),
        StoreOverride::Unusable { .. } => None,
    }
}

/// Resolve the AIDA project root the same way `aida-cli`'s
/// `handle_mailbox_command` does: find the attached distributed store (see
/// [`detect_distributed_store`]), then take its parent — the store worktree
/// lives at `<project_root>/.aida-store`, so its parent IS the project root
/// whose `.aida/mailbox/` (and everything else project-root-relative) the CLI
/// reads and writes.
///
/// `None` when no distributed store is resolvable (not an AIDA project here,
/// or a distributed project whose store isn't attached in this working copy)
/// — a caller with a legacy/non-distributed fallback should use it here.
// trace:TASK-1141 | ai:claude
pub fn resolve_project_root() -> Option<PathBuf> {
    detect_distributed_store()?
        .parent()
        .map(|p| p.to_path_buf())
}

/// Same as [`resolve_project_root`], but parameterized on the starting
/// directory instead of the process cwd — lets a caller resolve for a
/// specific `cwd` it already captured (testability) without a global env
/// mutation dance. `AIDA_STORE` is still honored first (it isn't
/// cwd-dependent), then the walk starts from `start`.
// trace:TASK-1141 | ai:claude
pub fn resolve_project_root_from(start: &Path) -> Option<PathBuf> {
    if let Some(p) = env_store_override() {
        return p.parent().map(|p| p.to_path_buf());
    }
    detect_distributed_store_from(start)?
        .parent()
        .map(|p| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn git(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git on PATH");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn classify_store_path_accepts_dir_with_objects() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().join("sandbox");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        let resolved = classify_store_path(&store).expect("should accept store");
        assert_eq!(resolved, store.canonicalize().unwrap());
    }

    #[test]
    fn classify_store_path_rejects_non_store_paths() {
        let tmp = TempDir::new().unwrap();

        assert!(classify_store_path(&tmp.path().join("nope")).is_none());

        let bare = tmp.path().join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        assert!(classify_store_path(&bare).is_none());

        let file = tmp.path().join("afile");
        std::fs::write(&file, "x").unwrap();
        assert!(classify_store_path(&file).is_none());
    }

    #[test]
    fn detect_distributed_store_walks_up_from_subdir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[deployment]\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".aida-store")).unwrap();

        let nested = root.join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();

        let resolved = detect_distributed_store_from(&nested).expect("should walk up");
        assert_eq!(resolved, root.join(".aida-store"));
    }

    // BUG-331 / TASK-1141: from a linked (sibling or nested) git worktree,
    // detection resolves the canonical store at the MAIN worktree (via
    // git-common-dir) instead of failing. The linked worktree has the
    // tracked `.aida/config.toml` but no local `.aida-store/`. This is the
    // exact case the TUI mail path must now agree with the CLI on.
    #[test]
    fn detect_distributed_store_resolves_from_linked_worktree() {
        let tmp = TempDir::new().unwrap();
        let main_root = tmp.path().join("main");
        std::fs::create_dir_all(&main_root).unwrap();
        git(&main_root, &["init", "-q", "-b", "main"]);
        git(&main_root, &["config", "user.email", "t@t.t"]);
        git(&main_root, &["config", "user.name", "t"]);
        std::fs::create_dir_all(main_root.join(".aida")).unwrap();
        std::fs::write(
            main_root.join(".aida/config.toml"),
            "[deployment]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        git(&main_root, &["add", "."]);
        git(&main_root, &["commit", "-q", "-m", "init"]);
        // The attached store worktree lives ONLY in the main worktree.
        std::fs::create_dir_all(main_root.join(".aida-store")).unwrap();

        let linked = tmp.path().join("linked");
        git(
            &main_root,
            &["worktree", "add", "--detach", linked.to_str().unwrap()],
        );

        // The linked worktree has NO `.aida-store/` of its own.
        assert!(!linked.join(".aida-store").exists());

        let resolved =
            detect_distributed_store_from(&linked).expect("should resolve via the main worktree");
        assert_eq!(
            resolved.canonicalize().unwrap(),
            main_root.join(".aida-store").canonicalize().unwrap(),
            "must resolve to the MAIN worktree's store, not fail"
        );

        // And the project root derived from it (store_path.parent()) is the
        // MAIN worktree's root, NOT the linked worktree's root — the exact
        // divergence TASK-1141 closes for the TUI mail path.
        assert_eq!(
            resolved.parent().unwrap().canonicalize().unwrap(),
            main_root.canonicalize().unwrap(),
        );
    }

    #[test]
    fn detect_distributed_store_returns_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(detect_distributed_store_from(&nested).is_none());
    }

    #[test]
    fn is_store_attached_detects_objects_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        assert!(!is_store_attached(root)); // bare project — no store
        std::fs::create_dir_all(root.join(".aida-store")).unwrap();
        assert!(!is_store_attached(root)); // .aida-store but not git-canonical
        std::fs::create_dir_all(root.join(".aida-store/objects")).unwrap();
        assert!(is_store_attached(root)); // objects/ present -> attached
    }

    // BUG-1598: the system temp dir itself must never be adopted as a
    // project root, even when a real `.aida/config.toml` happens to be
    // sitting there (left by a stray manual run, another test, or another
    // process on a shared dev box).
    #[test]
    fn is_system_temp_dir_identifies_the_os_temp_dir() {
        assert!(is_system_temp_dir(&std::env::temp_dir()));

        let tmp = TempDir::new().unwrap();
        assert!(!is_system_temp_dir(tmp.path()));
        assert!(!is_system_temp_dir(Path::new("/definitely/not/temp")));
    }

    #[test]
    fn is_temp_root_in_matches_only_listed_roots() {
        let tmp = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let roots = vec![tmp.path().to_path_buf()];
        assert!(is_temp_root_in(tmp.path(), &roots));
        assert!(!is_temp_root_in(other.path(), &roots));
        // A root that doesn't exist on this host must fall back to raw-path
        // comparison instead of erroring (canonicalize fails for it).
        let missing = tmp.path().join("does-not-exist");
        let missing_root = vec![missing.clone()];
        assert!(is_temp_root_in(&missing, &missing_root));
    }

    #[test]
    fn real_temp_roots_includes_the_fixed_unix_set() {
        let roots = real_temp_roots();
        assert!(roots.contains(&std::env::temp_dir()));
        #[cfg(unix)]
        {
            assert!(roots.contains(&PathBuf::from("/tmp")));
            assert!(roots.contains(&PathBuf::from("/var/tmp")));
            assert!(roots.contains(&PathBuf::from("/private/tmp")));
        }
    }

    #[test]
    fn detect_distributed_store_from_never_adopts_a_temp_root() {
        // Simulate the exact pollution scenario BUG-1598 describes: a real
        // `.aida/config.toml` sitting directly in a temp root (as a stray
        // manual `aida init` run, another test, or another process on a
        // shared dev box might leave), and a `mktemp -d`-style fixture one
        // level under it with no `.aida` of its own. The walk-up must stop
        // before reaching the temp root, never adopting the ambient config.
        //
        // The temp root is INJECTED as a fake — a plain tempdir standing in
        // for "a temp root" — rather than mutating `TMPDIR` or touching the
        // real, shared system temp dir, so this test can never race a
        // concurrent process's genuine use of it.
        let fake_temp_root = TempDir::new().unwrap();
        let roots = vec![fake_temp_root.path().to_path_buf()];

        let planted = fake_temp_root.path().join(".aida");
        std::fs::create_dir_all(&planted).unwrap();
        std::fs::write(
            planted.join("config.toml"),
            "[deployment]\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(fake_temp_root.path().join(".aida-store")).unwrap();

        let nested = fake_temp_root.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();

        let resolved = detect_distributed_store_from_with_roots(&nested, &roots);
        assert!(
            resolved.is_none(),
            "must never adopt a temp root's ambient .aida, got {resolved:?}"
        );

        // Sanity check: WITHOUT the guard (empty roots list), the same
        // fixture DOES resolve — proving the guard, not some other
        // difference, is what suppresses adoption above.
        let unguarded = detect_distributed_store_from_with_roots(&nested, &[]);
        assert!(
            unguarded.is_some(),
            "fixture must be adoptable when nothing is guarded, or this test proves nothing"
        );
    }

    // trace:BUG-1650 | ai:claude
    const BUG_1650_HOSTILE: [&str; 6] = [
        "../has\"quote/store",
        "../a=b/store",
        "C:\\Users\\RUNNER~1\\store",
        "../it's/store",
        "../multi\nline/store",
        "../all\"of=them\\'\n/store",
    ];

    fn first(body: &str) -> Option<String> {
        store_path_candidates(body).into_iter().next()
    }

    /// A project dir under `tmp` holding `.aida/config.toml` = `body`.
    fn project(tmp: &Path, name: &str, body: &str) -> PathBuf {
        let dir = tmp.join(name);
        std::fs::create_dir_all(dir.join(".aida")).unwrap();
        std::fs::write(dir.join(".aida/config.toml"), body).unwrap();
        dir
    }

    #[test]
    fn bug_1650_store_path_candidates_round_trip_toml_string_output() {
        use crate::toml_quote::toml_string;
        for value in BUG_1650_HOSTILE {
            let body = format!(
                "# AIDA distributed mode configuration\n[deployment]\nmode = \"distributed\"\nstore_path = {}\nbranch = \"aida-store\"\n",
                toml_string(value)
            );
            assert_eq!(first(&body).as_deref(), Some(value), "body {body:?}");
        }
    }

    #[test]
    fn bug_1650_store_path_candidates_read_literal_and_multiline_forms() {
        let cases = [
            ("[deployment]\nstore_path = '.aida-store'\n", ".aida-store"),
            (
                "[deployment]\nstore_path = 'C:\\x\\\"q\"'\n",
                "C:\\x\\\"q\"",
            ),
            ("store_path = \".aida-store\"\n", ".aida-store"),
            (
                "[deployment]\nstore_path = \"\"\"\n../multi\nline\"\"\"\n",
                "../multi\nline",
            ),
            ("[deployment]\nstore_path = '''a=b'''\n", "a=b"),
        ];
        for (body, want) in cases {
            assert_eq!(first(body).as_deref(), Some(want), "{body:?}");
        }
        assert!(store_path_candidates("[deployment]\nmode = \"x\"\n").is_empty());
    }

    /// Review finding 1: `[deployment]` beats a top-level key, which beats a
    /// `store_path` in any other table, whatever the file order.
    #[test]
    fn bug_1650_precedence_deployment_then_top_level_then_other_tables() {
        let body = "store_path = \"top\"\n\
                    [aaa]\nstore_path = \"aaa\"\n\
                    [deployment]\nstore_path = \"dep\"\n\
                    [zzz]\nstore_path = \"zzz\"\n";
        assert_eq!(store_path_candidates(body), ["dep", "top", "aaa", "zzz"]);

        let no_dep = "store_path = \"top\"\n\
                      [aaa]\nstore_path = \"aaa\"\n\
                      [deployment]\nmode = \"distributed\"\n\
                      [zzz]\nstore_path = \"zzz\"\n";
        assert_eq!(store_path_candidates(no_dep), ["top", "aaa", "zzz"]);

        // Resolution agrees when every candidate exists on disk.
        let tmp = TempDir::new().unwrap();
        let proj = project(tmp.path(), "proj", body);
        for d in ["dep", "top", "aaa", "zzz"] {
            std::fs::create_dir_all(proj.join(d)).unwrap();
        }
        assert_eq!(
            detect_distributed_store_from_with_roots(&proj, &[]),
            Some(proj.join("dep"))
        );
        std::fs::write(proj.join(".aida/config.toml"), no_dep).unwrap();
        assert_eq!(
            detect_distributed_store_from_with_roots(&proj, &[]),
            Some(proj.join("top"))
        );
    }

    /// Review finding 2: a pre-BUG-1649 config holds raw backslashes. When
    /// each one happens to start a valid TOML escape (`\t`, `\n`), the file
    /// parses, but to the wrong path. The raw legacy value must still
    /// resolve, instead of the walk-up adopting the enclosing project's store.
    #[test]
    fn bug_1650_legacy_unescaped_backslash_config_still_resolves() {
        for raw in ["..\\team-store", "stores\\new"] {
            let tmp = TempDir::new().unwrap();
            // An enclosing project whose store the walk-up must NOT adopt.
            let outer = project(
                tmp.path(),
                "outer",
                "[deployment]\nstore_path = \".outer\"\n",
            );
            std::fs::create_dir_all(outer.join(".outer")).unwrap();
            // Deliberately unescaped, as a pre-BUG-1649 writer produced it.
            let body = ["[deployment]\nstore_path = \"", raw, "\"\n"].concat();
            assert!(
                toml::from_str::<toml::Table>(&body).is_ok(),
                "{body:?} parses"
            );
            let proj = project(&outer, "proj", &body);
            // On Unix this is one file name containing `\`; on Windows it is
            // a real relative path. Either way `proj.join(raw)` names it.
            let legacy_store = proj.join(raw);
            std::fs::create_dir_all(&legacy_store).unwrap();

            let candidates = store_path_candidates(&body);
            assert_eq!(candidates.len(), 2, "{candidates:?}");
            assert_ne!(candidates[0], raw, "TOML applies the escape");
            assert_eq!(candidates[1], raw, "raw legacy value kept");
            assert_eq!(
                detect_distributed_store_from_with_roots(&proj, &[]),
                Some(legacy_store),
                "{raw:?}"
            );
        }
    }

    /// Proxy decision 3: `store_path = ""` keeps its old meaning (the config's
    /// own dir) and never walks up to an enclosing project.
    #[test]
    fn bug_1650_empty_store_path_keeps_legacy_resolution() {
        assert_eq!(
            store_path_candidates("[deployment]\nstore_path = \"\"\n"),
            [""]
        );
        let tmp = TempDir::new().unwrap();
        let outer = project(
            tmp.path(),
            "outer",
            "[deployment]\nstore_path = \".outer\"\n",
        );
        std::fs::create_dir_all(outer.join(".outer")).unwrap();
        let proj = project(&outer, "proj", "[deployment]\nstore_path = \"\"\n");
        assert_eq!(
            detect_distributed_store_from_with_roots(&proj, &[]),
            Some(proj.join(""))
        );
    }

    #[test]
    fn bug_1650_invalid_toml_falls_back_to_the_legacy_scan() {
        use crate::toml_quote::toml_string;
        assert_eq!(
            store_path_candidates("oops\nstore_path = '.aida-store'\n"),
            [".aida-store"]
        );
        assert_eq!(
            store_path_candidates("oops\nstore_path = .aida-store\n"),
            [".aida-store"]
        );
        // An escaped value in a broken file: the line parsed on its own is
        // also offered, after the raw legacy text.
        for value in BUG_1650_HOSTILE.iter().filter(|v| !v.contains('\n')) {
            let body = format!(
                "[deployment]\nthis is not toml\nstore_path = {}\n",
                toml_string(value)
            );
            assert!(
                store_path_candidates(&body).iter().any(|c| c == value),
                "{body:?}"
            );
        }
    }

    #[test]
    fn bug_1650_detect_resolves_store_path_with_quote_and_equals() {
        use crate::toml_quote::toml_string;
        let tmp = TempDir::new().unwrap();
        // No `"` here: Windows forbids it in file names.
        let rel = "st'o=re";
        let proj = project(
            tmp.path(),
            "proj",
            &format!("[deployment]\nstore_path = {}\n", toml_string(rel)),
        );
        std::fs::create_dir_all(proj.join(rel)).unwrap();
        assert_eq!(
            detect_distributed_store_from_with_roots(&proj, &[]),
            Some(proj.join(rel))
        );
    }
}
