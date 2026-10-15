//! aida-cli-lib core helpers (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Get the default author from AIDA_AUTHOR environment variable or fall back to system user.
/// Format recommendation: "ai:claude:username" for AI-assisted work
pub(crate) fn get_default_author() -> String {
    if let Ok(author) = std::env::var("AIDA_AUTHOR") {
        author
    } else {
        // Fall back to system username
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME")) // Windows fallback
            .unwrap_or_else(|_| "Unknown".to_string())
    }
}

/// Short build SHA for telemetry tagging. Reads the same banner that
/// `--version` prints (set by build.rs). Returns None if the banner
/// helper isn't available. Cheap — just substring extraction.
pub(crate) fn build_sha_short() -> Option<String> {
    let banner = build_banner();
    // build_banner format: "<version> (built <ts>, sha <sha>[+dirty])"
    let sha = banner.split("sha ").nth(1)?;
    let sha = sha.split([')', '+']).next()?;
    let trimmed = sha.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// String form of [`is_terminal_status`] — used by `aida list` / `aida
/// history` to hide the archive by default (TASK-64). Case-insensitive;
/// tolerates display vs storage casing. Companion to the enum version
/// at line ~4807 (BUG-64); both live here so the list/history surface
/// and the parent-guard share the same notion of "this is closed work".
/// trace:TASK-64 | ai:claude
pub fn is_terminal_status_str(s: &str) -> bool {
    // trace:STORY-86 | ai:claude — "Done" is NOT terminal anymore (work
    // finished on a branch; auto-bumps to Completed once merged to main).
    let t = s.trim();
    // trace:TASK-1176 | ai:claude — Superseded is terminal too: adopted, then
    // replaced by a successor spec. Closed for every gate that asks "is this
    // still open?"; it differs from Rejected only in meaning and rendering.
    t.eq_ignore_ascii_case("completed")
        || t.eq_ignore_ascii_case("rejected")
        || t.eq_ignore_ascii_case("superseded")
}

/// Read y/n from stdin with a default. Treats empty input as the default,
/// any 'y'/'yes' as true, anything else as false.
pub(crate) fn prompt_yes_no(prompt: &str, default_yes: bool) -> Result<bool> {
    use std::io::Write;
    print!("{}", prompt);
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let trimmed = answer.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Ok(default_yes);
    }
    Ok(matches!(trimmed.as_str(), "y" | "yes"))
}

/// True when a requirement's status means "this work is done — no new
/// children should be filed under it without explicit override". Used by
/// the BUG-64 guard on `aida add --parent` and `aida rel add --type
/// child` to refuse parenting under closed work, and to keep `aida show
/// --tree` / `aida list --parent` views from accumulating mixed-status
/// trees. trace:BUG-64 | ai:claude
pub(crate) fn is_terminal_status(status: &RequirementStatus) -> bool {
    // TASK-741: "terminal" is single-sourced in the lifecycle model so the
    // archive invariant, the BUG-64 parent guard, and the diagram all read the
    // same definition. trace:TASK-741 | ai:claude
    aida_core::lifecycle::State::from_status(status).is_terminal()
}

/// Parse requirement ID - accepts either UUID or SPEC-ID. Used by the legacy
/// SQLite path; the git-canonical dispatch resolves IDs directly via
/// `get_requirement_by_spec_id` and uses `not_found::requirement_not_found`
/// at the call site (with the actual store path).
///
/// trace:FR-1-011 | ai:claude
pub(crate) fn parse_requirement_id(id_str: &str, store: &RequirementsStore) -> Result<Uuid> {
    // Try parsing as UUID first
    if let Ok(uuid) = Uuid::parse_str(id_str) {
        return Ok(uuid);
    }

    // Try as SPEC-ID. TASK-1468: an ambiguous id refuses (these callers
    // write comments and relationships). trace:TASK-1468 | ai:claude
    if let Some(req) = store.get_requirement_unambiguous(id_str)? {
        return Ok(req.id);
    }

    // BUG-601: a loaded, non-empty store proves the store IS attached, so the
    // failure is a simply-nonexistent spec — emit the "check the spec ID" hint
    // rather than the misleading "no aida store found / cd into project root"
    // guidance that the None-path variant prints. The store-path isn't threaded
    // through this legacy helper, but store-emptiness is the signal we need:
    // non-empty ⇒ store present (spec-missing), empty ⇒ likely no store / wrong
    // directory (the original common failure mode). trace:BUG-601 | ai:claude
    if store.requirements.is_empty() {
        Err(not_found::requirement_not_found(id_str, None))
    } else {
        Err(not_found::requirement_not_found_in_loaded_store(id_str))
    }
}

/// Handle feature management subcommands
/// Parse a project `.aida/config.toml` into a `toml::Value`, returning `None`
/// only when absent (so a missing file just means "all defaults"). Parse errors
/// are printed with file/line context before callers fall back, so malformed
/// config is never silently treated as default config.
/// trace:BUG-533 | ai:claude
pub(crate) fn read_project_config_value(project_root: &std::path::Path) -> Option<toml::Value> {
    let path = config_path_for_project(project_root);
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("{}: failed to read AIDA config: {e}", path.display());
            return None;
        }
    };
    match toml::from_str(&body) {
        Ok(value) => Some(value),
        Err(err) => {
            // trace:BUG-1025 | ai:codex
            eprintln!("{}", config_parse_error_message(&path, &body, &err));
            None
        }
    }
}

/// Look up `[section].key` in a parsed config, returning the raw `toml::Value`
/// when present. trace:BUG-533 | ai:claude
pub(crate) fn config_lookup<'a>(
    cfg: Option<&'a toml::Value>,
    section: &str,
    key: &str,
) -> Option<&'a toml::Value> {
    cfg?.get(section)?.get(key)
}

/// Escape an arbitrary string for safe interpolation inside a
/// single-quoted shell word: every `'` becomes `'\''` (close-quote,
/// escaped-quote, reopen-quote). Any value emitted into the `eval`-able
/// shell of `aida role enter` MUST pass through this — free-text role
/// purposes and spec titles routinely contain apostrophes/parens, and an
/// unescaped apostrophe closes the quote and exposes the rest to bare
/// bash (`syntax error near unexpected token )`). trace:BUG-427 | ai:claude
pub(crate) fn sh_single_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Canonicalize a role name. TASK-586 made `advisor` the canonical
/// identifier; `dialog` (TASK-279's old internal token) is now a
/// deprecated, silently-accepted alias so existing config / shells
/// (`AIDA_SESSION_ROLE=dialog`) / `dialog`-routed queue items / legacy
/// `dialog.toml` role files on not-yet-migrated machines keep resolving.
/// Applied at every role-name boundary: load, list, input resolution,
/// and queue routing. trace:TASK-586 | ai:claude
pub(crate) fn canonical_role_name(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("dialog") {
        "advisor".to_string()
    } else if is_human_route(raw) {
        // SPIKE-57 / TASK-747: `human` is a first-class route target, the
        // escalation-cascade terminus, symmetric with the agent roles. Normalize
        // any casing to the lowercase canonical form so `--for Human` /
        // `--for HUMAN` route identically and surface together in the view.
        HUMAN_ROUTE.to_string()
    } else if raw.eq_ignore_ascii_case("guest") || raw.eq_ignore_ascii_case("requester") {
        // Stakeholder roles are identities, not build-loop seats. Canonicalize
        // casing so the least-privilege gate applies uniformly from env, team
        // roster, and queue target validation. trace:STORY-1110 | ai:codex
        raw.to_ascii_lowercase()
    } else {
        raw.to_string()
    }
}

pub(crate) fn find_project_root() -> Result<std::path::PathBuf> {
    // BUG-1618: a test that pinned a hermetic project root (see
    // `test_env::AmbientGuard`) resolves here instead of walking up from the
    // process cwd, which inside a leased `aida worktree add` checkout reaches a
    // `.aida-store` symlink to the LIVE store (its team roster, drain state).
    // Compiled out of release builds. trace:BUG-1618 | ai:claude
    #[cfg(test)]
    if let Some(root) = test_ambient::project_root() {
        return Ok(root);
    }
    find_project_root_from(&std::env::current_dir()?)
}

/// BUG-1618: whether stdin is an interactive terminal, as the advisor-authority
/// checks see it. Production reads the real stdin; a test that pinned a
/// hermetic ambient context gets its injected answer, so running `cargo test`
/// from an interactive shell cannot grant the TTY carve-out to a test that
/// asserts a refusal.
// trace:BUG-1618 | ai:claude
pub(crate) fn authority_stdin_is_terminal() -> bool {
    #[cfg(test)]
    if let Some(tty) = test_ambient::stdin_is_terminal() {
        return tty;
    }
    std::io::stdin().is_terminal()
}

/// Whether stdout is an interactive terminal, through the same test seam as
/// [`authority_stdin_is_terminal`]. A human-at-terminal gate checks both: an
/// agent that pipes stdout (or stdin) is not a person at a terminal.
// trace:BUG-1667 | ai:claude
pub(crate) fn authority_stdout_is_terminal() -> bool {
    #[cfg(test)]
    if let Some(tty) = test_ambient::stdout_is_terminal() {
        return tty;
    }
    std::io::stdout().is_terminal()
}

/// BUG-1618: test-only, per-thread override of the ambient inputs the
/// authority checks read (project root discovered from cwd, stdin TTY-ness).
/// Thread-local, so a test pinning it never leaks into a sibling test running
/// on another libtest thread, and no process-global `chdir` is needed. Set
/// through `test_env::AmbientGuard`, never directly.
// trace:BUG-1618 | ai:claude
#[cfg(test)]
pub(crate) mod test_ambient {
    use std::cell::RefCell;
    use std::path::PathBuf;

    #[derive(Clone, Debug)]
    pub(crate) struct Ambient {
        pub(crate) project_root: PathBuf,
        pub(crate) stdin_is_terminal: bool,
        // trace:BUG-1667 | ai:claude
        pub(crate) stdout_is_terminal: bool,
    }

    thread_local! {
        static AMBIENT: RefCell<Option<Ambient>> = const { RefCell::new(None) };
    }

    /// Install `next`, returning the previous value for the caller to restore.
    pub(crate) fn replace(next: Option<Ambient>) -> Option<Ambient> {
        AMBIENT.with(|a| a.replace(next))
    }

    pub(crate) fn project_root() -> Option<PathBuf> {
        AMBIENT.with(|a| a.borrow().as_ref().map(|x| x.project_root.clone()))
    }

    pub(crate) fn stdin_is_terminal() -> Option<bool> {
        AMBIENT.with(|a| a.borrow().as_ref().map(|x| x.stdin_is_terminal))
    }

    // trace:BUG-1667 | ai:claude
    pub(crate) fn stdout_is_terminal() -> Option<bool> {
        AMBIENT.with(|a| a.borrow().as_ref().map(|x| x.stdout_is_terminal))
    }
}

/// [`find_project_root`] from an explicit start directory: the nearest
/// ancestor holding `.git` (a directory in the main checkout, a file in a
/// linked worktree, so a linked worktree resolves to ITSELF).
// trace:TASK-1470 | ai:claude
pub(crate) fn find_project_root_from(start: &std::path::Path) -> Result<std::path::PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        if cur.join(".git").exists() {
            return Ok(cur);
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => anyhow::bail!("not inside a git repository"),
        }
    }
}

/// Walk PATH (plus a handful of common install locations) looking for a
/// real, executable forge CLI binary (`exe_base`, e.g. `gh` or `glab`).
/// Returns the resolved path or None.
///
/// The original code (pre-BUG-74) used `Command::new("gh")` and trusted
/// `ErrorKind::NotFound` to flag "gh isn't installed." That trust
/// produced false-negatives when the spawned Rust process inherited a
/// PATH that didn't include the user's install dir — a common outcome
/// when shell helpers mutate PATH only inside the shell and non-login
/// process launches see a stripped environment.
///
/// `debug_env`=1 (AIDA_DEBUG_GH / AIDA_DEBUG_GLAB) prints the search trace
/// to stderr; `test_env` (AIDA_TEST_GH_BINARY / AIDA_TEST_GLAB_BINARY)
/// overrides resolution in tests. trace:BUG-74 trace:STORY-621 | ai:claude
pub(crate) fn resolve_forge_binary(
    exe_base: &str,
    test_env: &str,
    debug_env: &str,
) -> Option<std::path::PathBuf> {
    if let Ok(test_path) = std::env::var(test_env) {
        return Some(std::path::PathBuf::from(test_path));
    }
    let debug = std::env::var(debug_env)
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false);
    let dbg_label = format!("{debug_env}:");
    let mut tried: Vec<std::path::PathBuf> = Vec::new();
    let mut spawn_failures: Vec<(std::path::PathBuf, String)> = Vec::new();

    let exe_name = if cfg!(windows) {
        format!("{exe_base}.exe")
    } else {
        exe_base.to_string()
    };

    // BUG-79: closure that checks `is_executable` AND a sanity-spawn of
    // `gh --version`. The metadata check is necessary but not sufficient:
    // stale install records, broken symlinks, container mounts, and
    // bash-hash-table caching can all produce a path whose metadata
    // looks fine but whose spawn yields ENOENT. The sanity-spawn is the
    // ground truth — if it fails, we fall back to the next candidate
    // instead of returning a path that the real caller will choke on.
    let mut check_candidate =
        |candidate: &std::path::Path, source: &str| -> Option<std::path::PathBuf> {
            tried.push(candidate.to_path_buf());
            if !is_executable(candidate) {
                return None;
            }
            match std::process::Command::new(candidate)
                .arg("--version")
                .output_retrying_etxtbsy()
            {
                Ok(o) if o.status.success() => {
                    if debug {
                        eprintln!(
                            "{} found {} {} at {}",
                            dbg_label.dimmed(),
                            exe_base,
                            source,
                            candidate.display()
                        );
                    }
                    Some(candidate.to_path_buf())
                }
                Ok(o) => {
                    let reason = format!(
                        "exit {} stderr={}",
                        o.status,
                        String::from_utf8_lossy(&o.stderr).trim()
                    );
                    if debug {
                        eprintln!(
                            "{} is_executable({}) ok but `--version` reported {} — falling back",
                            dbg_label.dimmed(),
                            candidate.display(),
                            reason
                        );
                    }
                    spawn_failures.push((candidate.to_path_buf(), reason));
                    None
                }
                Err(e) => {
                    let reason = format!("{}", e);
                    if debug {
                        eprintln!(
                            "{} is_executable({}) ok but spawn failed: {} — falling back",
                            dbg_label.dimmed(),
                            candidate.display(),
                            reason
                        );
                    }
                    spawn_failures.push((candidate.to_path_buf(), reason));
                    None
                }
            }
        };

    // Pass 1: walk $PATH.
    if let Ok(path) = std::env::var("PATH") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path.split(sep) {
            if dir.is_empty() {
                continue;
            }
            let candidate = std::path::PathBuf::from(dir).join(&exe_name);
            if let Some(p) = check_candidate(&candidate, "on PATH") {
                return Some(p);
            }
        }
    }

    // Pass 2: common absolute paths, in case PATH was mangled or the
    // child process inherited an empty PATH. Conservative list — only
    // dirs where gh is regularly installed by the official installers
    // or distros.
    let fallbacks: Vec<std::path::PathBuf> = {
        let mut v = vec![
            std::path::PathBuf::from("/usr/bin").join(&exe_name),
            std::path::PathBuf::from("/usr/local/bin").join(&exe_name),
            std::path::PathBuf::from("/opt/homebrew/bin").join(&exe_name),
            std::path::PathBuf::from("/snap/bin").join(&exe_name),
        ];
        if let Some(home) = crate::home_dir() {
            v.push(home.join(".local").join("bin").join(&exe_name));
            v.push(home.join("bin").join(&exe_name));
        }
        v
    };
    for candidate in &fallbacks {
        if let Some(p) = check_candidate(candidate, "via absolute-path fallback") {
            return Some(p);
        }
    }

    if debug {
        eprintln!(
            "{} {} not found after searching {} location(s):",
            dbg_label.dimmed(),
            exe_base,
            tried.len()
        );
        eprintln!(
            "{} PATH = {}",
            dbg_label.dimmed(),
            std::env::var("PATH").unwrap_or_default()
        );
        for p in &tried {
            eprintln!("  - {}", p.display());
        }
        if !spawn_failures.is_empty() {
            eprintln!(
                "{} {} candidate(s) passed is_executable but failed to spawn:",
                dbg_label.dimmed(),
                spawn_failures.len()
            );
            for (p, reason) in &spawn_failures {
                eprintln!("  - {} ({})", p.display(), reason);
            }
        }
    }
    None
}

/// Resolve the `gh` (GitHub CLI) binary — thin wrapper over the
/// forge-generic resolver, preserving pre-Slice-0 behavior byte-for-byte.
/// trace:BUG-74 trace:STORY-621 | ai:claude
pub(crate) fn resolve_gh_binary() -> Option<std::path::PathBuf> {
    resolve_forge_binary("gh", "AIDA_TEST_GH_BINARY", "AIDA_DEBUG_GH")
}

/// BUG-1288: run `cmd` but never block past `timeout` waiting on it — a
/// portable (`Child::kill` works on every target) alternative to
/// `Command::output()` for a subprocess whose peer (a forge API) can stall
/// arbitrarily long. stdout/stderr are drained on background threads so the
/// child can never deadlock on a full pipe while the caller polls for exit;
/// on timeout the child (and, on unix, its whole process group — see
/// `kill_process_group`) is killed and `None` is returned — every existing
/// caller already treats `output().ok()` failure as "unknown, not zero"
/// (PRIN-5), so a timeout degrades exactly like any other unreachable-forge
/// failure already does.
///
/// Two review follow-ups folded in here, both about NOT hanging past
/// `timeout` even when the direct child has an uncooperative descendant:
/// - unix: spawned with `process_group(0)` and killed with `killpg` (see
///   `kill_process_group`) instead of `Child::kill`, which only ever
///   signals the one direct child. Windows keeps the pre-existing
///   direct-child-only `Child::kill` — no job-object process-tree kill
///   implemented yet, so a grandchild there can still outlive the timeout
///   and hold the pipes open; see the bounded read below for why that no
///   longer means blocking forever.
/// - the reader threads are joined through a channel with a BOUNDED wait,
///   not an unconditional `JoinHandle::join()`. On the happy path (child
///   exited on its own) the bound is generous and never realistically hit;
///   after a kill it is short, so a pipe that somehow stayed open past the
///   process-group kill (Windows; a grandchild that double-forked out of
///   the group) degrades to a partial/empty read instead of wedging this
///   function — and the caller — indefinitely.
// trace:BUG-1288 | ai:claude
// trace:TASK-1424 | ai:claude — pub(crate) so gitlab_mirror_link's bounded
// `git`/`gh` calls reuse this instead of a second timeout implementation.
pub(crate) fn command_output_with_timeout(
    cmd: std::process::Command,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    match command_output_with_timeout_detail(cmd, timeout) {
        BoundedCommandOutput::Completed(output) => Some(output),
        BoundedCommandOutput::SpawnFailed | BoundedCommandOutput::TimedOut => None,
    }
}

/// Forge-keyed CLI binary dispatch: GitHub → `gh`, GitLab → `glab`. `None`
/// (pure-git) names no forge CLI. The foundation for routing the main.rs gh
/// call sites through the configured forge. trace:STORY-621 | ai:claude
pub(crate) fn resolve_forge_cli(kind: crate::forge::ForgeKind) -> Option<std::path::PathBuf> {
    use crate::forge::ForgeKind;
    match kind {
        ForgeKind::GitHub => resolve_gh_binary(),
        ForgeKind::GitLab => resolve_glab_binary(),
        ForgeKind::None => None,
    }
}

/// TASK-32: cross-platform home-dir lookup with an `AIDA_HOME` override.
/// On Windows, `dirs::home_dir()` resolves via `SHGetKnownFolderPath`,
/// which ignores env vars — that breaks bg_worker tests that need to
/// isolate writes from the real user profile. Checking `AIDA_HOME` first
/// gives tests a deterministic hook on every platform without changing
/// the production lookup. trace:TASK-32 | ai:claude
pub(crate) fn aida_home_dir() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("AIDA_HOME") {
        if !p.is_empty() {
            let p = std::path::PathBuf::from(p);
            // trace:BUG-1642 | ai:claude
            #[cfg(test)]
            crate::test_home::assert_hermetic(&p);
            return Some(p);
        }
    }
    crate::home_dir()
}

/// BUG-1642: the one home-directory lookup for this crate. Production defers
/// to [`aida_core::home::home_dir`], which honours `$HOME` / `$USERPROFILE`
/// before the platform lookup so the answer is overridable on Windows too.
/// Under `cfg(test)` it resolves the temp `HOME` the lib test binary installs
/// before `main` and panics rather than return the operator's real home, so no
/// lib test can read or write the real `~/.aida`. Call this instead of the
/// `dirs` crate (a source-scan test enforces it).
// trace:BUG-1642 | ai:claude
// trace:TASK-1513 | ai:claude
pub(crate) fn home_dir() -> Option<std::path::PathBuf> {
    #[cfg(test)]
    {
        crate::test_home::home_dir()
    }
    #[cfg(not(test))]
    {
        aida_core::home::home_dir()
    }
}

/// Copy `text` to the system clipboard, trying the platform tools in turn.
/// Returns false when none is available (caller falls back to stdout).
pub(crate) fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    // Linux (Wayland then X11), macOS, Windows.
    let tools: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("pbcopy", &[]),
        ("clip", &[]),
    ];
    for (cmd, args) in tools {
        let Ok(mut child) = Command::new(cmd)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn_retrying_etxtbsy()
        else {
            continue;
        };
        let write_ok = match child.stdin.take() {
            Some(mut stdin) => stdin.write_all(text.as_bytes()).is_ok(),
            None => false,
        };
        // stdin dropped above → EOF; now wait for the tool to finish.
        match child.wait() {
            Ok(status) if write_ok && status.success() => return true,
            _ => continue,
        }
    }
    false
}

/// Parse a `--since` lookback window for the usage, health and metrics
/// surfaces: how far before now the bound lies. Uses the shared time-bound
/// grammar (`30d`, `12h`, `2w`, `24 hours ago`, an ISO date or datetime in
/// local time, or RFC3339). Returns an error on malformed input.
// trace:TASK-1509 | ai:claude
pub(crate) fn parse_days_arg(raw: &str) -> Result<chrono::Duration> {
    queue_cmd::parse_lookback(raw, "--since")
}

/// Best-effort resolution of the Claude/AIDA session id for the shell that
/// is adding a comment. Reads the session id AIDA exports on the launch path
/// (`AIDA_SESSION_ID`), falling back to Claude Code's own `CLAUDE_CODE_SESSION_ID`.
/// Returns `None` (never errors) when neither is set — comments added outside
/// a tracked session simply carry no session id. Stamping it lets tooling
/// correlate a comment back to the session that produced it.
// trace:TASK-330 | ai:claude
pub(crate) fn resolve_current_session_id() -> Option<String> {
    for var in ["AIDA_SESSION_ID", "CLAUDE_CODE_SESSION_ID"] {
        if let Ok(val) = std::env::var(var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Resolve the queue's "current user" identifier. Used by EVERY queue path
/// (add, list, next, done, remove, move, work, role-show queue head, …) so
/// items added in one shell are immediately visible in the same shell's
/// `aida queue list` without flags.
///
/// Resolution order:
///   1. Explicit `--user <id>` flag (override)
///   2. `AIDA_USER` env var (set by sessions / agent harnesses)
///   3. `USER` env var (POSIX shell convention)
///   4. `USERNAME` env var (Windows fallback)
///   5. The literal string "default" (last-ditch)
///
/// IMPORTANT: this is the SHELL's user identity, not the node identity from
/// `~/.aida/node.toml`, the email in `[node]`, or the role's stored user_id.
/// Those are different identity domains; mixing them caused BUG-89 (items
/// invisible to their own queuer because list resolved to one identity and
/// add resolved to another).
/// trace:BUG-89 | ai:claude
pub(crate) fn current_user_id(user_override: Option<&str>) -> String {
    user_override.map(str::to_string).unwrap_or_else(|| {
        std::env::var("AIDA_USER")
            .or_else(|_| std::env::var("USER"))
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "default".to_string())
    })
}

/// Resolve the aida binary path for all in-process `aida` subprocesses, once
/// per process so a mid-flight binary replacement cannot invalidate it.
///
/// Why this matters (BUG-217): the orchestrator drives multiple phases over
/// 15-30 minutes. The implementer's phase-1 Claude session often runs
/// `cargo build` (rebuilding the dev binary at `target/debug/aida`). On
/// Linux, `/proc/self/exe` then returns the original path with " (deleted)"
/// appended once the file is unlinked — and `Command::new("<path>
/// (deleted)").spawn()` fails with ENOENT. By resolving once at the start
/// (before phase 1 can rebuild) and stripping any pre-existing suffix, we
/// stabilise the path for the whole parent process.
///
/// Falls back to the bare "aida" name (PATH search) if the OS lookup
/// failed or the resolved path doesn't exist on disk.
///
/// trace:BUG-217 | ai:claude
pub(crate) fn aida_exe_path() -> std::path::PathBuf {
    static AIDA_EXE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    AIDA_EXE
        .get_or_init(|| {
            aida_bin::process()
                .map(|r| r.path)
                .unwrap_or_else(|_| resolve_aida_exe_from(std::env::current_exe().ok()))
        })
        .clone()
}

pub(crate) fn resolve_aida_exe() -> std::path::PathBuf {
    aida_exe_path()
}

/// TASK-84: quote-aware strip of an inline TOML comment.
/// Returns the slice of `s` before the first unquoted `#`. Tracks both
/// `"` and `'` so `key = "value with # inside"` round-trips correctly.
/// Spec-compliant TOML allows trailing `# comment` on key/value lines;
/// without this helper, a line like `permission_mode = "auto" # ...`
/// would parse to the literal string `auto" # ...` and get rejected by
/// `claude --permission-mode`. trace:TASK-84 | ai:claude
pub(crate) fn strip_toml_inline_comment(s: &str) -> &str {
    let mut in_dquote = false;
    let mut in_squote = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' if !in_squote => in_dquote = !in_dquote,
            '\'' if !in_dquote => in_squote = !in_squote,
            '#' if !in_dquote && !in_squote => return &s[..i],
            _ => {}
        }
    }
    s
}
