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

/// The `permissions.deny` rules protecting `path`: `Edit(//<path>)` and
/// `Edit(//<path>/**)`. Claude Code applies an `Edit` path rule to every
/// file-writing tool and warns at launch that a `Write`/`MultiEdit`/
/// `NotebookEdit` path deny never matches, so the deny side emits `Edit` only.
/// Empty when [`root_rule_path`] cannot express `path`.
// trace:BUG-b6b1278890774ef993d2ebc327862470-029 | ai:claude
pub(crate) fn path_deny_rules(path: &Path) -> Vec<String> {
    root_rule_path(path)
        .map(|abs| vec![format!("Edit(/{abs})"), format!("Edit(/{abs}/**)")])
        .unwrap_or_default()
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

/// The shared runtime mailbox alone; never grant the enclosing `.aida`
/// directory (which contains grants, leases and other coordination authority).
/// Refuse symlinked mailbox/state directories rather than binding their target.
// trace:BUG-2218 | ai:codex
pub(crate) fn contained_mailbox_write_paths(main: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let main = std::fs::canonicalize(main)?;
    let state = main.join(".aida");
    let mailbox = state.join("mailbox");
    for path in [&state, &mailbox] {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            anyhow::ensure!(
                !meta.file_type().is_symlink(),
                "refusing sandbox mailbox path {}: symbolic link",
                path.display()
            );
        }
    }
    anyhow::ensure!(
        root_rule_path(&mailbox).is_some(),
        "cannot express sandbox mailbox path {}",
        mailbox.display()
    );
    Ok(vec![mailbox])
}

/// The submit side of `aida deputy` alone: `<main>/.aida/deputy/incoming`,
/// where `aida deputy submit` and `cancel` write. Never the enclosing
/// `.aida/deputy` (the running jobs, results, ledger, TTY policy and terminal
/// markers stay unwritable to a contained session). Refuse a symlinked state,
/// deputy or incoming directory rather than binding its target.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
pub(crate) fn contained_deputy_write_paths(main: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let main = std::fs::canonicalize(main)?;
    let incoming = crate::deputy::deputy_incoming_base(&main);
    let state = main.join(".aida");
    for path in [
        state.as_path(),
        &crate::deputy::deputy_base(&main),
        &incoming,
    ] {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            anyhow::ensure!(
                !meta.file_type().is_symlink(),
                "refusing sandbox deputy path {}: symbolic link",
                path.display()
            );
        }
    }
    anyhow::ensure!(
        root_rule_path(&incoming).is_some(),
        "cannot express sandbox deputy path {}",
        incoming.display()
    );
    Ok(vec![incoming])
}

/// Create the deputy submit directory (0700) at launch, so the sandbox can
/// bind it (a path missing at launch is skipped).
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
pub(crate) fn prepare_deputy_incoming(main: &Path) -> anyhow::Result<()> {
    for incoming in contained_deputy_write_paths(main)? {
        if let Some(base) = incoming.parent() {
            crate::deputy::create_private_dir(base)?;
        }
        crate::deputy::create_private_dir(&incoming)?;
    }
    Ok(())
}

/// Prime the exact directory at launch, after the path calculation validated it.
// trace:BUG-2218 TASK-2328 | ai:codex
pub(crate) fn prepare_mailbox(main: &Path) -> anyhow::Result<()> {
    for mailbox in contained_mailbox_write_paths(main)? {
        // Born with its creation nonce, like every other mailbox creator.
        // trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
        let root = mailbox.parent().and_then(Path::parent).unwrap_or(main);
        aida_core::mailbox::ensure_local_mailbox_dir(root)
            .map_err(|e| anyhow::anyhow!("creating sandbox mailbox {}: {e}", mailbox.display()))?;
    }
    Ok(())
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

/// BUG-2012: when a contained session's write root is a main checkout (where
/// `<root>/.git` is either a real directory or a main-checkout `.git` gitfile
/// pointing at its `gitdir:` target), `//<root>/**` would otherwise cover the
/// main checkout's `.git/hooks/*` and `.git/config`. Deny only the main
/// checkout's real `.git` (directory or gitfile target, plus the `.git` file
/// itself) directly from `write_roots` even when `store_write_paths` is empty.
///
/// Linked worktrees (whose `gitdir:` target is `<common>/worktrees/<name>`
/// with a `commondir` resolving back to `<common>`) and non-checkout
/// directories (no `.git`, or a nested `.git` outside the checkout root) are
/// not main checkouts, so their gitdirs stay allowed; only a linked
/// worktree's `.git` pointer file itself is denied (TASK-1989). A `.git` symlink is
/// followed to its directory or gitfile. No seat-clone name or marker exempts
/// a main checkout's real `.git`: those markers are writable by the contained
/// session.
///
/// BUG-2169 S2: the one exemption is a seat clone the HOST recorded
/// ([`crate::seat_clone::host_recorded_clone`]: a `clone` lease in the main
/// checkout's `.aida/sessions`, outside every granted root). That `.git` is
/// the seat's own repository, which it must write to commit; no host git ever
/// runs in it.
// trace:BUG-2012 | ai:antigravity
// trace:BUG-2169 | ai:claude
pub(crate) fn contained_main_checkout_git_deny_paths(write_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut denied = Vec::new();
    let mut push_unique = |p: PathBuf| {
        if !denied.contains(&p) {
            denied.push(p);
        }
    };
    for root in write_roots {
        let git_entry = root.join(".git");
        let Ok(meta) = std::fs::symlink_metadata(&git_entry) else {
            continue;
        };
        // trace:BUG-2169 | ai:claude
        if crate::seat_clone::host_recorded_clone(root).is_some() {
            continue;
        }
        if meta.is_dir() || git_entry.is_dir() {
            // TASK-2036: a linked worktree's `.git` is never a directory, so a
            // `commondir` file planted in a `.git` directory exempts nothing.
            // trace:TASK-1952 | trace:TASK-2036 | ai:claude
            push_unique(git_entry.clone());
            if let Ok(canon) = std::fs::canonicalize(&git_entry) {
                push_unique(canon);
            }
        } else if git_entry.is_file() {
            // TASK-2006: `is_file()` follows a `.git` symlink to a gitfile,
            // which `symlink_metadata` reports as a symlink and the old
            // `meta.is_file()` test skipped, denying nothing. The symlink's
            // gitfile (canonical `.git`) and its `gitdir:` target are denied;
            // like git, a relative `gitdir:` resolves against `<root>`.
            // trace:TASK-1952 | trace:TASK-2006 | ai:antigravity
            let Some(raw_target) = aida_core::git_ops::worktree_gitdir_path(root) else {
                continue;
            };
            let target = normalize_lexical(&raw_target);
            push_unique(git_entry.clone());
            if is_linked_worktree_gitdir(&target) {
                // TASK-1989: a linked worktree keeps its own gitdir writable,
                // but not the `.git` pointer file: a rewritten pointer would
                // aim the operator's next git command in this tree at a
                // planted gitdir (hooks, core.hooksPath) outside the sandbox.
                // trace:TASK-1952 | trace:TASK-1989 | ai:claude
                continue;
            }
            if let Ok(canon_entry) = std::fs::canonicalize(&git_entry) {
                push_unique(canon_entry);
            }
            push_unique(target.clone());
            if let Ok(canon_target) = std::fs::canonicalize(&target) {
                push_unique(canon_target);
            }
        }
    }
    denied
}

/// Normalize `.` and `..` components lexically so a relative `gitdir:` target
/// joined onto a write root can be expressed by [`root_rule_path`].
// trace:BUG-2012 | ai:antigravity
pub(crate) fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether `gitdir` (the target of a `.git` file) is a linked worktree's own
/// gitdir rather than a main checkout's `--separate-git-dir` target.
///
/// TASK-2005: a directory merely named `worktrees/<name>` is not enough. The
/// gitdir must hold a `commondir` file that resolves (absolute, or relative to
/// `gitdir`) to an existing common git directory `C` such that `gitdir` is
/// exactly `C/worktrees/<name>`, the layout `git worktree add` creates.
// trace:BUG-2012 | ai:antigravity
// trace:TASK-1952 | trace:TASK-2005 | ai:antigravity
pub(crate) fn is_linked_worktree_gitdir(gitdir: &Path) -> bool {
    let Ok(body) = std::fs::read_to_string(gitdir.join("commondir")) else {
        return false;
    };
    let raw = body.trim();
    if raw.is_empty() {
        return false;
    }
    let common = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        gitdir.join(raw)
    };
    let (Ok(common), Ok(gitdir)) = (std::fs::canonicalize(common), std::fs::canonicalize(gitdir))
    else {
        return false;
    };
    common.is_dir()
        && gitdir.file_name().is_some()
        && gitdir.parent() == Some(common.join("worktrees").as_path())
}

/// The `sandbox.filesystem` object for `store_write_paths`: `allowWrite` from
/// [`sandbox_allow_write_entries`] and `denyWrite` for each allowed store's
/// `.git` pointer ([`contained_store_deny_write_paths`]). A store directory
/// whose pointer cannot be denied is not allowed either (fails closed). `None`
/// when nothing is allowed.
// trace:BUG-1929 | ai:claude
pub(crate) fn sandbox_filesystem(store_write_paths: &[PathBuf]) -> Option<serde_json::Value> {
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
    let allow_write = sandbox_allow_write_entries(&guarded);
    if allow_write.is_empty() {
        return None;
    }
    let deny_write = sandbox_allow_write_entries(&contained_store_deny_write_paths(&guarded));
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
#[path = "tests/bug_053_contained_add_tests.rs"]
mod contained_add_tests;

#[cfg(test)]
#[path = "tests/bug_1982_main_git_metadata_tests.rs"]
mod main_git_tests;

#[cfg(test)]
#[path = "tests/bug_2201_contained_baseline_tests.rs"]
mod baseline_tests;

#[cfg(test)]
#[path = "tests/task_1952_contained_launch_tests.rs"]
mod task_1952_launch_tests;
