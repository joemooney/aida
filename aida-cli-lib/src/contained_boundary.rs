//! BUG-1928: the contained posture's WRITE boundary, as a list of absolute
//! allowed roots.
//!
//! A contained headless session runs `claude -p --permission-mode dontAsk` with
//! an inline `--settings` document. Under `dontAsk` every tool call that no
//! `permissions.allow` rule pre-approves is denied, so the allow list IS the
//! write boundary. The pre-BUG-1928 list was the single rule `Edit(/**)`. In
//! Claude Code's Read/Edit rule grammar a single leading `/` is anchored to the
//! project/settings root, not the filesystem root; the absolute form is `//`.
//! Probed against claude 2.1.296 with the session's cwd at the worktree root:
//! `Edit(/**)` denied a Write to `<worktree>/inside.txt`, while
//! `Edit(//<worktree>/**)` + `Write(//<worktree>/**)` allowed it and denied
//! `<worktree>/../outside.txt`. Every contained drain therefore shipped nothing.
//!
//! The boundary is a LIST of absolute roots so a later slice (BUG-1929: the
//! main checkout's `.aida-store` and cache) extends this one list instead of
//! adding a second rule source. Each root becomes one absolute rule per
//! file-writing tool. A root that cannot be expressed exactly (relative, the
//! filesystem root, `..`, non-UTF-8, or containing glob/rule metacharacters)
//! emits NO rule: the boundary fails closed, never wider.
//!
//! The roots come from the worktree the session actually runs in (the git root
//! of the spawning process's cwd, which `queue work` sets to the lease's
//! worktree, pool or named), never from a requested path the warm pool may have
//! replaced. trace:BUG-1928 | ai:claude

use std::path::{Component, Path, PathBuf};

/// The built-in Claude Code tools that write files. All use the Read/Edit path
/// rule grammar. Claude Code applies `Edit` rules to every file-editing tool;
/// the others are emitted explicitly too, so the boundary does not depend on
/// that aliasing.
// trace:BUG-1928 | ai:claude
pub(crate) const WRITE_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Characters that would change the meaning of a rule path (glob syntax, rule
/// delimiters, escapes). A root containing any of them is refused rather than
/// escaped.
// trace:BUG-1928 | ai:claude
const RULE_METACHARS: [char; 10] = ['*', '?', '[', ']', '{', '}', '(', ')', '\\', '!'];

/// The absolute roots a contained session may write under, derived from the
/// worktree it runs in: the path as given when it is absolute, plus its
/// canonical form when that differs (a symlinked parent), so the rule matches
/// whichever spelling Claude Code resolves a tool path to. Both spell the SAME
/// directory, so the list is never broader than the worktree.
// trace:BUG-1928 | ai:claude
pub(crate) fn contained_write_roots(worktree: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if worktree.is_absolute() {
        roots.push(worktree.to_path_buf());
    }
    if let Ok(canonical) = std::fs::canonicalize(worktree) {
        if !roots.contains(&canonical) {
            roots.push(canonical);
        }
    }
    roots
}

/// The rule-path spelling of `root` (`/a/b`, no trailing slash), or `None` when
/// the root cannot be expressed as an exact absolute directory.
// trace:BUG-1928 | ai:claude
pub(crate) fn root_rule_path(root: &Path) -> Option<String> {
    if !root.is_absolute() {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for component in root.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(segment) => parts.push(segment.to_str()?),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    if parts.is_empty() {
        // `/` itself: a rule over the whole filesystem is never a worktree.
        return None;
    }
    let unsafe_segment = parts.iter().any(|part| {
        part.chars()
            .any(|ch| ch.is_control() || RULE_METACHARS.contains(&ch))
    });
    if unsafe_segment {
        return None;
    }
    Some(format!("/{}", parts.join("/")))
}

/// The `permissions.allow` rules for `roots`: `<Tool>(//<root>/**)` for every
/// tool in [`WRITE_TOOLS`]. The `//` prefix is Claude Code's absolute-path
/// anchor (`/<root>` would be read relative to the project root).
// trace:BUG-1928 | ai:claude
pub(crate) fn write_allow_rules(roots: &[PathBuf]) -> Vec<String> {
    let mut rules: Vec<String> = Vec::new();
    for root in roots {
        let Some(abs) = root_rule_path(root) else {
            continue;
        };
        for tool in WRITE_TOOLS {
            let rule = format!("{tool}(/{abs}/**)");
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
    }
    rules
}

/// BUG-1929: the main checkout's AIDA store and cache, which a contained
/// session's `aida` binary (run through sandboxed Bash) must write to claim a
/// spec, comment, or finish a queue item. Per the operator ruling the set is
/// exactly `<main>/.aida-store` (a directory: the objects plus
/// `.aida/store-write.lock`) and the `<main>/.aida/cache.db*` files, for each
/// spelling of `main` that [`contained_write_roots`] yields. Nothing else in
/// the main checkout is added.
///
/// Claude Code's Linux sandbox binds each `sandbox.filesystem.allowWrite`
/// entry as is, with no glob expansion, and skips any path that does not
/// exist at launch, so the cache files are listed one by one rather than as
/// `cache.db*`. Their sidecars persist across connections
/// (`aida_core::db::prime_cache_sidecars`).
// trace:BUG-1929 | ai:claude
pub(crate) fn contained_store_write_paths(main: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for base in contained_write_roots(main) {
        paths.push(base.join(".aida-store"));
        let cache = base.join(".aida").join("cache.db");
        for suffix in std::iter::once("").chain(aida_core::db::CACHE_SANDBOX_SIDECARS) {
            let mut name = cache.as_os_str().to_os_string();
            name.push(suffix);
            paths.push(PathBuf::from(name));
        }
    }
    paths
}

/// BUG-2169: the git paths a contained session in a LINKED worktree may
/// write, so `git commit`, `fetch`, `merge` and `push` work in its own
/// worktree, plus the paths that stay read-only inside and beside them.
///
/// A linked worktree's git state lives outside the worktree: its own gitdir
/// (`<common>/worktrees/<name>`: index, HEAD, ORIG_HEAD, FETCH_HEAD,
/// MERGE_HEAD) and the shared common dir (`objects/`, `refs/`, `logs/`).
/// Only those are granted. The common dir's `config`, `hooks/` and `info/`,
/// the store's hook profile, and every other worktree's gitdir stay outside
/// the write set: a writable hook or `core.hooksPath` would run code on the
/// host. `packed-refs` is not granted either: git rewrites it through
/// `packed-refs.lock` and a rename in the common dir, which a file bind cannot
/// serve, and fetch, commit, merge and push write loose refs.
///
/// `deny` lists, for `sandbox.filesystem.denyWrite` (deny wins over an
/// enclosing allow): `<common>/config`, `hooks`, `info`; the gitdir's
/// `config.worktree`, `commondir` and `gitdir` pointers; the worktree's own
/// `.git` pointer; and the loose ref and reflog of every branch checked out in
/// ANOTHER worktree (the main checkout's `main`, the store's branch) and of
/// the default branch, so a seat can move its own branches but not someone
/// else's. Those denies bind only once [`materialize_protected_refs`] has
/// created the files: a packed branch has no loose ref to deny.
// trace:BUG-2169 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContainedGitPaths {
    /// The canonical common dir. The store's recursive deny of it is replaced
    /// by `deny`, because the sandbox cannot allow a path inside a denied one.
    pub(crate) common: PathBuf,
    pub(crate) gitdir: PathBuf,
    pub(crate) allow: Vec<PathBuf>,
    pub(crate) deny: Vec<PathBuf>,
    /// The branches the seat may not move (every branch checked out in
    /// another worktree, and the default branch): their loose ref and reflog
    /// are in `deny`, and [`materialize_protected_refs`] creates both before
    /// launch so the deny binds an existing path.
    pub(crate) refs: Vec<ProtectedRef>,
}

/// A branch the seat may not move, resolved at launch.
// trace:BUG-2169 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtectedRef {
    /// `refs/heads/<branch>`.
    pub(crate) name: String,
    /// `<common>/refs/heads/<branch>`.
    pub(crate) path: PathBuf,
    /// `<common>/logs/refs/heads/<branch>`.
    pub(crate) log: PathBuf,
}

/// The common-dir subdirectories a linked worktree's git must write.
// trace:BUG-2169 | ai:claude
pub(crate) const COMMON_GIT_WRITE_DIRS: [&str; 3] = ["objects", "refs", "logs"];

/// The common-dir entries named read-only explicitly (they sit outside every
/// allowed path anyway).
// trace:BUG-2169 | ai:claude
pub(crate) const COMMON_GIT_DENY: [&str; 3] = ["config", "hooks", "info"];

/// The gitdir files that redirect or configure git and stay read-only.
// trace:BUG-2169 | ai:claude
pub(crate) const GITDIR_DENY: [&str; 3] = ["config.worktree", "commondir", "gitdir"];

/// [`resolve_contained_git_paths`], printing the refusal on stderr. `None`
/// (worktree-only boundary) for a primary checkout, outside git, or on refusal.
// trace:BUG-2169 | ai:claude
pub(crate) fn contained_git_write_paths(worktree: &Path) -> Option<ContainedGitPaths> {
    let resolved = resolve_contained_git_paths(worktree).and_then(|paths| {
        if let Some(paths) = &paths {
            materialize_protected_refs(paths)?;
        }
        Ok(paths)
    });
    match resolved {
        Ok(paths) => paths,
        Err(reason) => {
            eprintln!(
                "warning: contained session in {} gets no git write access (commit, fetch and \
                 merge will fail): {reason}",
                worktree.display()
            );
            None
        }
    }
}

/// Create the loose ref and reflog file of every protected branch that lacks
/// one. The sandbox skips a `denyWrite` on a missing path, and `refs/` is
/// writable, so a PACKED branch (no loose file) would let the seat write a
/// loose `refs/heads/main`, which overrides `packed-refs`. A loose ref holding
/// the packed value changes nothing for git. The ref is written under git's
/// own `<ref>.lock` protocol and re-resolved under that lock, so a concurrent
/// update is never reverted. `Err` (the caller falls back to the
/// worktree-only boundary) when any ref cannot be materialized as a real file.
///
/// `objects/info/alternates` stays writable through `objects/`: it can only
/// add object sources to read, never run code or move a ref.
// trace:BUG-2169 | ai:claude
pub(crate) fn materialize_protected_refs(paths: &ContainedGitPaths) -> Result<(), String> {
    use std::io::Write;
    let err = |p: &Path, e: std::io::Error| format!("materializing {}: {e}", p.display());
    for r in &paths.refs {
        for file in [&r.path, &r.log] {
            let parent = file.parent().unwrap_or(&paths.common);
            std::fs::create_dir_all(parent).map_err(|e| err(parent, e))?;
            if std::fs::canonicalize(parent).ok().as_deref() != Some(parent) {
                return Err(format!("{} is not a real directory", parent.display()));
            }
        }
        if std::fs::symlink_metadata(&r.path).is_err() {
            let mut lock_name = r.path.clone().into_os_string();
            lock_name.push(".lock");
            let lock = PathBuf::from(lock_name);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock)
                .map_err(|e| err(&lock, e))?;
            // Under the lock: a loose ref written meanwhile wins; otherwise
            // write the value git resolves now (the packed one).
            let written = (|| {
                if std::fs::symlink_metadata(&r.path).is_ok() {
                    return Ok(false);
                }
                let out = std::process::Command::new("git")
                    .arg("--git-dir")
                    .arg(&paths.common)
                    .args(["rev-parse", "--verify", "--quiet"])
                    .arg(format!("{}^{{commit}}", r.name))
                    .output()
                    .map_err(|e| err(&r.path, e))?;
                let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !out.status.success() || sha.is_empty() {
                    return Err(format!("cannot resolve {}", r.name));
                }
                writeln!(file, "{sha}").map_err(|e| err(&lock, e))?;
                file.sync_all().map_err(|e| err(&lock, e))?;
                std::fs::rename(&lock, &r.path).map_err(|e| err(&r.path, e))?;
                Ok(true)
            })();
            if !matches!(written, Ok(true)) {
                let _ = std::fs::remove_file(&lock);
            }
            written?;
        }
        if std::fs::symlink_metadata(&r.log).is_err() {
            std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&r.log)
                .map_err(|e| err(&r.log, e))?;
        }
        for file in [&r.path, &r.log] {
            let meta = std::fs::symlink_metadata(file).map_err(|e| err(file, e))?;
            if !meta.file_type().is_file() {
                return Err(format!("{} is not a regular file", file.display()));
            }
        }
    }
    Ok(())
}

/// The git write grant for `worktree`, resolved by git itself and
/// canonicalized. `Ok(None)` when `worktree` is not a linked worktree (a
/// primary checkout's `.git` is inside its own root and stays denied by the
/// store boundary). `Err` when the layout is not exactly
/// `<common>/worktrees/<name>` with real (non-symlinked) `objects`, `refs` and
/// `logs` under the common dir, or when any path cannot be expressed exactly:
/// the caller then falls back to the worktree-only boundary.
// trace:BUG-2169 | ai:claude
pub(crate) fn resolve_contained_git_paths(
    worktree: &Path,
) -> Result<Option<ContainedGitPaths>, String> {
    // Read-only lookups: `rev-parse` and `worktree list` run no hook.
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .current_dir(worktree)
            .args(["-c", "core.fsmonitor=false"])
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let Some(dirs) = git(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-dir",
        "--git-common-dir",
    ]) else {
        return Ok(None);
    };
    let mut lines = dirs.lines();
    let (Some(gitdir), Some(common)) = (lines.next(), lines.next()) else {
        return Ok(None);
    };
    let canonical = |p: &Path| {
        std::fs::canonicalize(p).map_err(|e| format!("cannot resolve {}: {e}", p.display()))
    };
    let gitdir = canonical(Path::new(gitdir))?;
    let common = canonical(Path::new(common))?;
    if gitdir == common {
        return Ok(None);
    }
    let worktree_real = canonical(worktree)?;
    if common.starts_with(&worktree_real) || worktree_real.starts_with(&common) {
        return Err(format!(
            "the git common dir {} and the worktree {} overlap",
            common.display(),
            worktree_real.display()
        ));
    }
    if gitdir.parent() != Some(common.join("worktrees").as_path()) {
        return Err(format!(
            "the worktree's gitdir {} is not directly under {}/worktrees",
            gitdir.display(),
            common.display()
        ));
    }
    let mut allow = vec![gitdir.clone()];
    for name in COMMON_GIT_WRITE_DIRS {
        let expected = common.join(name);
        if canonical(&expected)? != expected || !expected.is_dir() {
            return Err(format!("{} is not a real directory", expected.display()));
        }
        allow.push(expected);
    }
    let mut deny: Vec<PathBuf> = COMMON_GIT_DENY.iter().map(|n| common.join(n)).collect();
    deny.extend(GITDIR_DENY.iter().map(|n| gitdir.join(n)));
    for root in contained_write_roots(worktree) {
        let pointer = root.join(".git");
        if !deny.contains(&pointer) {
            deny.push(pointer);
        }
    }
    let listing = git(&["worktree", "list", "--porcelain"])
        .ok_or_else(|| "`git worktree list` failed".to_string())?;
    // trace:BUG-2169 | ai:claude
    let mut protected: Vec<String> = Vec::new();
    let mut other = false;
    let mut primary: Option<Option<String>> = None;
    for line in listing.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            other = std::fs::canonicalize(path).map_or(true, |p| p != worktree_real);
            if primary.is_none() {
                primary = Some(None);
            }
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if primary == Some(None) {
                primary = Some(Some(branch.to_string()));
            }
            if other && !protected.iter().any(|b| b == branch) {
                protected.push(branch.to_string());
            }
        }
    }
    // The default branch is protected even when no other worktree has it
    // checked out: `origin/HEAD`'s target, else the primary checkout's branch.
    let default = git(&["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"])
        .and_then(|r| {
            r.trim()
                .strip_prefix("refs/remotes/origin/")
                .map(str::to_string)
        })
        .or_else(|| primary.flatten())
        .ok_or_else(|| "cannot determine the repository's default branch".to_string())?;
    if !protected.contains(&default) {
        protected.push(default);
    }
    if let Some(own) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        let own = own.trim().trim_start_matches("refs/heads/").to_string();
        if protected.contains(&own) {
            return Err(format!(
                "the worktree has the protected branch {own} checked out"
            ));
        }
    }
    let mut refs = Vec::new();
    for branch in &protected {
        git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}^{{commit}}"),
        ])
        .ok_or_else(|| format!("cannot resolve the protected branch {branch}"))?;
        let path = common.join("refs/heads").join(branch);
        let log = common.join("logs/refs/heads").join(branch);
        deny.push(path.clone());
        deny.push(log.clone());
        refs.push(ProtectedRef {
            name: format!("refs/heads/{branch}"),
            path,
            log,
        });
    }
    if let Some(bad) = allow
        .iter()
        .chain(&deny)
        .find(|p| root_rule_path(p).is_none())
    {
        return Err(format!(
            "{} cannot be expressed as an exact sandbox path",
            bad.display()
        ));
    }
    Ok(Some(ContainedGitPaths {
        common,
        gitdir,
        allow,
        deny,
        refs,
    }))
}

/// The `sandbox.filesystem` path entries (`allowWrite` or `denyWrite`) for
/// `paths`: `//<abs>` (Claude
/// Code's absolute-path anchor) per path. A path [`root_rule_path`] cannot
/// express exactly emits nothing (fails closed).
// trace:BUG-1929 | ai:claude
pub(crate) fn sandbox_allow_write_entries(paths: &[PathBuf]) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    for path in paths {
        let Some(abs) = root_rule_path(path) else {
            continue;
        };
        let entry = format!("/{abs}");
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    entries
}

/// Git authority paths that must remain read-only even when a contained
/// session's code root is the primary checkout. The store pointer/own git
/// subtree, main git directory, and the verified hook profile's actual common
/// directory are denied recursively. No host-executed profile is writable.
// trace:BUG-1929 | ai:codex
pub(crate) fn contained_store_deny_write_paths(store_write_paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut denied = Vec::new();
    for store in store_write_paths
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == ".aida-store"))
    {
        let mut paths = vec![store.join(".git")];
        if let Some(main) = store.parent() {
            paths.push(main.join(".git"));
        }
        if let Ok(Some(hooks)) = aida_core::git_ops::resolve_store_hooks_dir(store) {
            if let Some(common) = hooks.parent() {
                paths.push(common.to_path_buf());
            }
        }
        for path in paths {
            if !denied.contains(&path) {
                denied.push(path);
            }
        }
    }
    denied
}

/// The `sandbox.filesystem` object for `store_write_paths`: `allowWrite` from
/// [`sandbox_allow_write_entries`] and `denyWrite` for each allowed store's
/// `.git` pointer ([`contained_store_deny_write_paths`]). A store directory
/// whose pointer cannot be denied is not allowed either (fails closed). `None`
/// when nothing is allowed.
// trace:BUG-1929 | ai:claude
pub(crate) fn sandbox_filesystem(store_write_paths: &[PathBuf]) -> Option<serde_json::Value> {
    sandbox_filesystem_with_git(store_write_paths, None)
}

/// [`sandbox_filesystem`] plus BUG-2169's git grant for a linked worktree:
/// `git.allow` joins `allowWrite`, and the store's recursive deny of the git
/// common dir is replaced by `git.deny`. Claude Code's sandbox lets a deny
/// narrow an allow but never an allow reopen a denied directory, so keeping
/// the recursive deny would leave `objects/` and `refs/` read-only. The
/// common dir's other entries (config, hooks, the store's hook profile, other
/// worktrees' gitdirs) stay outside every allowed path, and the Edit/Write
/// tool layer keeps its recursive deny.
// trace:BUG-2169 | ai:claude
pub(crate) fn sandbox_filesystem_with_git(
    store_write_paths: &[PathBuf],
    git: Option<&ContainedGitPaths>,
) -> Option<serde_json::Value> {
    let guarded: Vec<PathBuf> = store_write_paths
        .iter()
        .filter(|p| {
            p.file_name().is_none_or(|n| n != ".aida-store")
                || contained_store_deny_write_paths(std::slice::from_ref(*p))
                    .iter()
                    .all(|deny| root_rule_path(deny).is_some())
        })
        .cloned()
        .collect();
    let mut allow_paths = guarded.clone();
    let mut deny_paths = contained_store_deny_write_paths(&guarded);
    if let Some(git) = git {
        allow_paths.extend(git.allow.iter().cloned());
        deny_paths.retain(|p| std::fs::canonicalize(p).map_or(true, |p| p != git.common));
        deny_paths.extend(git.deny.iter().cloned());
    }
    let allow_write = sandbox_allow_write_entries(&allow_paths);
    if allow_write.is_empty() {
        return None;
    }
    let deny_write = sandbox_allow_write_entries(&deny_paths);
    Some(serde_json::json!({ "allowWrite": allow_write, "denyWrite": deny_write }))
}

/// BUG-1929: why a contained session's write set would miss the store or cache
/// it resolves, one plain sentence per gap (empty when nothing is missing).
/// `resolved_store` / `resolved_cache` are what the CLI actually uses; a path
/// is covered when its canonical form is one of the write-set paths (or, for
/// the store, inside the store directory).
// trace:BUG-1929 | ai:claude
pub(crate) fn contained_store_boundary_gaps(
    main: &Path,
    resolved_store: Option<&Path>,
    resolved_cache: &Path,
) -> Vec<String> {
    let paths = contained_store_write_paths(main);
    let expressible: Vec<&PathBuf> = paths
        .iter()
        .filter(|p| root_rule_path(p).is_some())
        .collect();
    if expressible.is_empty() {
        return vec![format!(
            "the main checkout path {} cannot be expressed as an exact sandbox path, so \
             contained sessions get no write access to the AIDA store or cache",
            main.display()
        )];
    }
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let mut gaps = Vec::new();
    if let Some(store) = resolved_store {
        let store = canonical(store);
        let covered = expressible
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == ".aida-store"))
            .any(|p| store.starts_with(canonical(p)));
        if !covered {
            gaps.push(format!(
                "the AIDA store at {} is outside {}/.aida-store, so contained sessions \
                 cannot write it",
                store.display(),
                main.display()
            ));
        }
    }
    let cache = canonical(&aida_core::db::shared_cache_path(resolved_cache));
    if !expressible.iter().any(|p| canonical(p) == cache) {
        gaps.push(format!(
            "the AIDA cache at {} is not the main checkout's {}/.aida/cache.db, so \
             contained sessions cannot write it",
            cache.display(),
            main.display()
        ));
    }
    for path in contained_store_write_paths(&canonical(main)) {
        if root_rule_path(&path).is_some() && !path.exists() {
            gaps.push(format!(
                "{} does not exist, and the sandbox skips a write path that is missing \
                 at launch (any `aida` command run from the main checkout creates it)",
                path.display()
            ));
        }
    }
    gaps
}

/// The label for the workspace line a launching `queue work` child prints. The
/// orchestrator's phase-1 child carries `AIDA_AUTO_COMPLETE_PHASE=1`.
// trace:BUG-1928 | ai:claude
pub(crate) fn launched_workspace_label(phase_env: Option<&str>) -> &'static str {
    if phase_env.map(str::trim) == Some("1") {
        "phase 1 workspace"
    } else {
        "workspace"
    }
}

/// `<label>: <worktree> [<branch>]` — the workspace announcement, printed from
/// the lease's worktree once it exists, so it names the tree the session runs
/// in (a warm-pool tree included), not the path the orchestrator requested.
// trace:BUG-1928 | ai:claude
pub(crate) fn workspace_announcement(label: &str, worktree: &Path, branch: &str) -> String {
    format!("{label}: {} [{branch}]", worktree.display())
}

/// The `write boundary` line for a contained session launched outside any git
/// repository: there is no worktree, so there are no write roots.
// trace:BUG-1971 | ai:claude
pub(crate) const OUTSIDE_GIT_BOUNDARY_ANNOUNCEMENT: &str =
    "write boundary: none (the session is not inside a git repository, so there is no \
     worktree and every file edit will be denied)";

/// BUG-1971: the launch lines `queue work` prints once the lease exists: the
/// workspace line from the lease's worktree and, for a contained claude
/// launch, the write-boundary line from the SAME roots the launch's
/// `--settings` rules use (the git root of the process cwd), never from
/// `lease_worktree` as given.
// trace:BUG-1971 | ai:claude
pub(crate) fn queue_work_launch_lines(
    phase_env: Option<&str>,
    lease_worktree: &Path,
    branch: &str,
    contained_claude: bool,
) -> Vec<String> {
    let mut lines = vec![workspace_announcement(
        launched_workspace_label(phase_env),
        lease_worktree,
        branch,
    )];
    if contained_claude {
        lines.push(crate::session::contained_write_boundary_announcement());
    }
    lines
}

/// `write boundary: //<root>/**, …` — the operator-facing view of the allow
/// rules, built from the same roots.
// trace:BUG-1928 | ai:claude
pub(crate) fn write_boundary_announcement(roots: &[PathBuf]) -> String {
    let patterns: Vec<String> = roots
        .iter()
        .filter_map(|root| root_rule_path(root.as_path()))
        .map(|abs| format!("/{abs}/**"))
        .collect();
    if patterns.is_empty() {
        "write boundary: none (the worktree path cannot be expressed as an exact rule, so \
         every file edit will be denied)"
            .to_string()
    } else {
        format!("write boundary: {}", patterns.join(", "))
    }
}

/// Test-only evaluator for Claude Code's documented Read/Edit permission rule
/// grammar (gitignore-style), so the generated settings can be checked against
/// REAL filesystem paths rather than by string presence:
///
/// - `//p` — absolute path from the filesystem root
/// - `~/p` — relative to the home directory
/// - `/p`  — relative to the project/settings root
/// - `**` matches any number of path segments (a trailing `/**` at least one);
///   `*` and `?` match within one segment
/// - an `Edit` rule applies to every file-editing tool
/// - under `dontAsk`, a matching `deny` wins, then a matching `allow` permits,
///   and anything else is denied
///
/// Target paths are normalised lexically (`..` is resolved) before matching,
/// the way a tool's `file_path` is resolved before the check.
// trace:BUG-1928 | ai:claude
#[cfg(test)]
pub(crate) mod evaluator {
    use std::path::{Component, Path, PathBuf};

    pub(crate) fn normalize(path: &Path) -> Option<Vec<String>> {
        if !path.is_absolute() {
            return None;
        }
        let mut out: Vec<String> = Vec::new();
        for component in path.components() {
            match component {
                Component::RootDir | Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                Component::Normal(segment) => out.push(segment.to_str()?.to_string()),
                Component::Prefix(_) => return None,
            }
        }
        Some(out)
    }

    /// `Tool(spec)` → `(tool, spec)`.
    pub(crate) fn parse_rule(rule: &str) -> Option<(&str, &str)> {
        let open = rule.find('(')?;
        let inner = rule[open + 1..].strip_suffix(')')?;
        Some((&rule[..open], inner))
    }

    /// The rule's path pattern as absolute segments, or `None` for a form AIDA
    /// never emits (cwd-relative / unanchored).
    pub(crate) fn anchor(spec: &str, settings_root: &Path, home: &Path) -> Option<Vec<String>> {
        let (base, rest): (PathBuf, &str) = if let Some(rest) = spec.strip_prefix("//") {
            (PathBuf::from("/"), rest)
        } else if let Some(rest) = spec.strip_prefix("~/") {
            (home.to_path_buf(), rest)
        } else if let Some(rest) = spec.strip_prefix('/') {
            (settings_root.to_path_buf(), rest)
        } else {
            return None;
        };
        let mut segments = normalize(&base)?;
        segments.extend(
            rest.split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        );
        Some(segments)
    }

    fn segment_matches(pattern: &[char], text: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some((&'*', rest)) => (0..=text.len()).any(|i| segment_matches(rest, &text[i..])),
            Some((&'?', rest)) => !text.is_empty() && segment_matches(rest, &text[1..]),
            Some((c, rest)) => text.first() == Some(c) && segment_matches(rest, &text[1..]),
        }
    }

    pub(crate) fn path_matches(pattern: &[String], path: &[String]) -> bool {
        match pattern.split_first() {
            None => path.is_empty(),
            Some((head, rest)) if head == "**" => {
                if rest.is_empty() {
                    // `dir/**` covers everything INSIDE dir, not dir itself.
                    !path.is_empty()
                } else {
                    (0..=path.len()).any(|i| path_matches(rest, &path[i..]))
                }
            }
            Some((head, rest)) => match path.split_first() {
                Some((segment, path_rest)) => {
                    let p: Vec<char> = head.chars().collect();
                    let t: Vec<char> = segment.chars().collect();
                    segment_matches(&p, &t) && path_matches(rest, path_rest)
                }
                None => false,
            },
        }
    }

    fn rule_applies(rule_tool: &str, tool: &str) -> bool {
        rule_tool == tool || (rule_tool == "Edit" && super::WRITE_TOOLS.contains(&tool))
    }

    fn any_rule_matches(
        settings: &serde_json::Value,
        key: &str,
        tool: &str,
        target: &[String],
        settings_root: &Path,
        home: &Path,
    ) -> bool {
        settings
            .pointer(&format!("/permissions/{key}"))
            .and_then(|v| v.as_array())
            .map(|rules| {
                rules.iter().filter_map(|r| r.as_str()).any(|rule| {
                    let Some((rule_tool, spec)) = parse_rule(rule) else {
                        return false;
                    };
                    rule_applies(rule_tool, tool)
                        && anchor(spec, settings_root, home)
                            .map(|pattern| path_matches(&pattern, target))
                            .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    }

    /// Whether a `dontAsk` session with `settings` may run `tool` on `path`.
    pub(crate) fn allowed(
        settings: &serde_json::Value,
        tool: &str,
        path: &Path,
        settings_root: &Path,
        home: &Path,
    ) -> bool {
        let Some(target) = normalize(path) else {
            return false;
        };
        !any_rule_matches(settings, "deny", tool, &target, settings_root, home)
            && any_rule_matches(settings, "allow", tool, &target, settings_root, home)
    }
}

#[cfg(test)]
#[path = "tests/bug_1928_contained_boundary_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/bug_1929_contained_store_tests.rs"]
mod store_tests;

#[cfg(test)]
#[path = "tests/bug_2169_contained_git_tests.rs"]
mod git_tests;
