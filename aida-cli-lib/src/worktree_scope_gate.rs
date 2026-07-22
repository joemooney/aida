//! Worktree-scope guard — detect-not-prevent isolation check at the commit
//! boundary (TASK-1178).
//!
//! ## The failure it catches
//!
//! An agent working in a **scoped session** (its own git worktree, taken via
//! `aida worktree enter` / `aida session start` / an orchestrator fan-out) can
//! still write files into the SHARED main checkout — a stale `cd`, an absolute
//! path copied from an earlier session, a tool invoked with the wrong working
//! directory. The edits look fine locally, then land on a branch nobody
//! expected, or sit in the shared tree confusing the next agent.
//!
//! AIDA cannot **prevent** that: what a harness writes where is a filesystem /
//! agent-harness concern, not something a requirements tool can gate. But the
//! commit boundary is a place where the mistake becomes *visible*, and that is
//! where this guard lives.
//!
//! ## Contract
//!
//!   - **Warn-only.** [`enforce_at_commit`] always returns `Ok(())`. It never
//!     fails the pre-commit hook, never aborts a commit. Blocking here would be
//!     wrong twice over: the check is heuristic (a session may legitimately
//!     commit elsewhere), and a hard block just teaches agents `--no-verify`,
//!     which also skips the advisor-code gate, the locking gate and the doc-
//!     comment leak gate above it in the same hook (the BUG-651 lesson).
//!   - **Silent unless it fires.** A commit whose staged paths all resolve
//!     INSIDE the session worktree prints nothing.
//!   - **Non-scoped sessions are untouched.** No `AIDA_SESSION_ID`, or no
//!     worktree-backed lease behind it (a review/claim lease has no worktree),
//!     and the gate is a no-op before it reads anything expensive.
//!
//! ## How the scope is resolved
//!
//!   1. `AIDA_WT_LEASE_FILE` — the direct pointer `aida worktree enter` exports.
//!   2. Otherwise `AIDA_SESSION_ID` is matched (by id prefix, the same rule
//!      `aida session end` uses) against the lease TOMLs under
//!      `.aida/sessions/` — both in the repo root the commit is happening in AND
//!      in the main checkout (`git rev-parse --git-common-dir`'s parent), since
//!      leases are written where `session start` ran, which for the stray case
//!      is exactly not where we are.
//!
//! Every step is best-effort: a missing lease, an unreadable file, a git hiccup
//! all read as "no scope known" and the gate stays silent.
//!
//! trace:TASK-1178 | ai:claude

use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use colored::Colorize;

/// Max stray paths listed in the warning before it collapses to a count. A
/// mis-targeted commit can carry hundreds of files; the point is to make the
/// mistake obvious, not to reprint the diff.
// trace:TASK-1178 | ai:claude
const MAX_LISTED_STRAY: usize = 10;

// ---------------------------------------------------------------------------
// Pure path scoping
// ---------------------------------------------------------------------------

/// PURE: lexically normalize a path — drop `.` components and resolve `..`
/// against the accumulated prefix, without touching the filesystem. A leading
/// `..` that cannot be popped is preserved so a genuinely-escaping relative
/// path stays escaping.
// trace:TASK-1178 | ai:claude
pub(crate) fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // `pop` fails on an empty buffer or one ending in `..`/root.
                let popped = match out.components().next_back() {
                    None | Some(Component::ParentDir) => false,
                    Some(Component::RootDir) => true, // `/..` is `/`
                    Some(_) => out.pop(),
                };
                if !popped {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// PURE-ish: resolve a path to the form used for containment comparison —
/// lexically normalized, then symlink-resolved as far as the filesystem allows.
///
/// A staged path frequently does NOT exist by the time we look (a staged
/// deletion, or a file the agent already moved), so `canonicalize` on the whole
/// path is not enough. We canonicalize the deepest existing ANCESTOR and
/// re-attach the remaining tail, which is what makes the symlink case work: a
/// worktree reached through a symlinked parent resolves to the same real prefix
/// as the files inside it.
// trace:TASK-1178 | ai:claude
pub(crate) fn resolve_for_scope(p: &Path) -> PathBuf {
    let lex = lexical_normalize(p);
    if let Ok(c) = lex.canonicalize() {
        return c;
    }
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = lex.clone();
    while let Some(parent) = cur.parent().map(Path::to_path_buf) {
        match cur.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            // No file name left (root, or a residual `..`) — nothing more to
            // strip; fall back to the lexical form.
            None => return lex,
        }
        if let Ok(canon) = parent.canonicalize() {
            let mut out = canon;
            for name in tail.iter().rev() {
                out.push(name);
            }
            return out;
        }
        cur = parent;
    }
    lex
}

/// PURE: is `path` inside `base` (or equal to it)? Both are expected to be
/// already resolved via [`resolve_for_scope`].
// trace:TASK-1178 | ai:claude
pub(crate) fn is_within(base: &Path, path: &Path) -> bool {
    path.starts_with(base)
}

/// PURE-ish: the staged paths that resolve OUTSIDE the session worktree.
///
/// `staged` entries are repo-relative names exactly as git prints them, so they
/// are joined onto `repo_root` first. Both sides are resolved with
/// [`resolve_for_scope`], so symlinked worktree paths, `..` traversal and a
/// relative vs absolute `repo_root` all compare correctly. The returned strings
/// are the ORIGINAL staged names — the shape the operator recognises from
/// `git status` — not the resolved absolute forms.
// trace:TASK-1178 | ai:claude
pub(crate) fn stray_paths(
    session_worktree: &Path,
    repo_root: &Path,
    staged: &[String],
) -> Vec<String> {
    let base = resolve_for_scope(session_worktree);
    let root = resolve_for_scope(repo_root);
    staged
        .iter()
        .filter(|name| {
            let abs = if Path::new(name.as_str()).is_absolute() {
                PathBuf::from(name.as_str())
            } else {
                root.join(name.as_str())
            };
            !is_within(&base, &resolve_for_scope(&abs))
        })
        .cloned()
        .collect()
}

/// PURE: the warning body for a set of stray paths. Split from the printing so
/// the wording is testable without a process environment. Returns `None` when
/// there is nothing to say (the in-worktree, stays-silent case).
///
/// Deliberately carries NO spec id: this is user-facing stderr.
// trace:TASK-1178 | ai:claude
pub(crate) fn format_warning(
    session_worktree: &Path,
    repo_root: &Path,
    stray: &[String],
) -> Option<String> {
    if stray.is_empty() {
        return None;
    }
    let mut s = format!(
        "worktree scope: this commit touches {} path(s) OUTSIDE your session worktree.\n",
        stray.len()
    );
    s.push_str(&format!(
        "  session worktree: {}\n",
        session_worktree.display()
    ));
    s.push_str(&format!("  committing from:  {}\n", repo_root.display()));
    for name in stray.iter().take(MAX_LISTED_STRAY) {
        s.push_str(&format!("    - {name}\n"));
    }
    if stray.len() > MAX_LISTED_STRAY {
        s.push_str(&format!(
            "    … and {} more\n",
            stray.len() - MAX_LISTED_STRAY
        ));
    }
    s.push_str(
        "  Warn-only — the commit is proceeding. If this was not deliberate, the edits were \
         probably made\n  in the wrong checkout: undo them here and redo the work inside the \
         session worktree above.",
    );
    Some(s)
}

// ---------------------------------------------------------------------------
// Scope resolution (IO)
// ---------------------------------------------------------------------------

/// The one field of a session lease this gate needs. Every field is optional so
/// a lease shape from any AIDA version parses — an unknown/absent key can never
/// make the gate noisy or fatal.
// trace:TASK-1178 | ai:claude
#[derive(Debug, Default, serde::Deserialize)]
struct LeaseScope {
    #[serde(default)]
    worktree_path: PathBuf,
}

/// Read a lease TOML's worktree path. `None` for an unreadable/unparseable file
/// or a worktree-less advisory lease (`aida review` / `aida claim` write an
/// empty `worktree_path` by convention).
// trace:TASK-1178 | ai:claude
fn worktree_from_lease_file(path: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(path).ok()?;
    let lease: LeaseScope = toml::from_str(&content).ok()?;
    if lease.worktree_path.as_os_str().is_empty() {
        None
    } else {
        Some(lease.worktree_path)
    }
}

/// PURE: does a lease file stem identify `session_id`? The env var may carry a
/// short prefix of the lease id (the `aida session end` resolution rule), so a
/// match either direction counts.
// trace:TASK-1178 | ai:claude
pub(crate) fn lease_stem_matches(stem: &str, session_id: &str) -> bool {
    !session_id.is_empty()
        && !stem.is_empty()
        && (stem.starts_with(session_id) || session_id.starts_with(stem))
}

/// Find the worktree path of the lease identified by `session_id` under
/// `<dir>/*.toml`.
// trace:TASK-1178 | ai:claude
fn worktree_from_leases_dir(dir: &Path, session_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        if !lease_stem_matches(stem, session_id) {
            continue;
        }
        if let Some(wt) = worktree_from_lease_file(&p) {
            return Some(wt);
        }
    }
    None
}

/// The main checkout's root — the parent of `git rev-parse --git-common-dir`.
/// From a linked worktree this is the SHARED checkout, which is where session
/// leases live; from the main checkout it is the same dir we are already in.
// trace:TASK-1178 | ai:claude
fn main_checkout_root(cwd: &Path) -> Option<PathBuf> {
    let raw = git_capture(cwd, &["rev-parse", "--git-common-dir"])?;
    let p = PathBuf::from(&raw);
    let abs = if p.is_absolute() { p } else { cwd.join(p) };
    abs.parent().map(Path::to_path_buf)
}

/// The session worktree this commit is scoped to, or `None` when the session is
/// not worktree-scoped (which is the "leave it alone" case).
// trace:TASK-1178 | ai:claude
pub(crate) fn session_worktree(root: &Path) -> Option<PathBuf> {
    // 1. The direct pointer `aida worktree enter` exports.
    if let Some(lease_file) = std::env::var("AIDA_WT_LEASE_FILE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        if let Some(wt) = worktree_from_lease_file(Path::new(&lease_file)) {
            return Some(wt);
        }
    }
    // 2. Match the session id against the lease store — here AND in the main
    //    checkout, because a stray commit is by definition happening somewhere
    //    other than where the session was minted.
    let session_id = std::env::var("AIDA_SESSION_ID")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    let mut dirs = vec![root.join(".aida").join("sessions")];
    if let Some(main_root) = main_checkout_root(root) {
        let d = main_root.join(".aida").join("sessions");
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs.iter()
        .find_map(|d| worktree_from_leases_dir(d, &session_id))
}

// ---------------------------------------------------------------------------
// Git reads
// ---------------------------------------------------------------------------

/// `git -C <cwd> <args>` → trimmed stdout, `None` on any failure.
// trace:TASK-1178 | ai:claude
fn git_capture(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
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

/// The staged path names, NUL-separated so a path with spaces / quoting-worthy
/// bytes survives intact. Falls back to diffing against the empty tree so an
/// unborn branch (the very first commit) still reports its staged set instead of
/// erroring out.
// trace:TASK-1178 | ai:claude
fn staged_names(repo_root: &Path) -> Vec<String> {
    const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    let raw = git_capture(repo_root, &["diff", "--cached", "--name-only", "-z"]).or_else(|| {
        git_capture(
            repo_root,
            &["diff", "--cached", "--name-only", "-z", EMPTY_TREE],
        )
    });
    raw.map(|s| {
        s.split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

/// PURE-ish: the whole decision, given an already-resolved scope. `None` means
/// "say nothing" and covers all three silent cases in one place:
///   - `session_worktree` is `None` — the session is not worktree-scoped, so a
///     non-scoped session sees no behaviour change at all;
///   - nothing is staged;
///   - everything staged is inside the session worktree.
///
/// Split out from [`enforce_at_commit`] so each of those is testable without
/// mutating the process environment or shelling out to git.
// trace:TASK-1178 | ai:claude
pub(crate) fn verdict(
    session_worktree: Option<&Path>,
    repo_root: &Path,
    staged: &[String],
) -> Option<String> {
    let worktree = session_worktree?;
    if staged.is_empty() {
        return None;
    }
    let stray = stray_paths(worktree, repo_root, staged);
    format_warning(worktree, repo_root, &stray)
}

/// Warn (never block) when a scoped session's commit touches paths outside its
/// session worktree. Called by the scaffolded git pre-commit hook via
/// `aida internal worktree-scope-gate`, so the check binds ANY vendor's commit —
/// Claude, Codex, a raw terminal, a headless child — not just the one harness
/// that happens to have a tool hook installed.
///
/// Always `Ok(())`: this is instrumentation, not enforcement.
// trace:TASK-1178 | ai:claude
pub(crate) fn enforce_at_commit(root: &Path) -> Result<()> {
    let Some(worktree) = session_worktree(root) else {
        return Ok(()); // not a worktree-scoped session — untouched.
    };
    // Where git will actually record this commit. Prefer git's own answer so a
    // nested cwd inside the repo doesn't skew the join below.
    let repo_root = git_capture(root, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .unwrap_or_else(|| root.to_path_buf());
    let staged = staged_names(&repo_root);
    if let Some(body) = verdict(Some(&worktree), &repo_root, &staged) {
        eprintln!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            body
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/worktree_scope_gate_tests.rs"]
mod worktree_scope_gate_tests;
