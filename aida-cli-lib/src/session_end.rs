//! `aida session end`, session-to-end resolution and the per-session env/cargo target dir (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// trace:STORY-73 | ai:claude
/// TASK-243: format the `Claude session: <id>… (last active …)` line for
/// `aida session end`'s ambiguity prompt. Pure — `age_secs` is resolved
/// by the caller. trace:TASK-243 | ai:claude
pub(crate) fn format_claude_session_line(claude_id: &str, age_secs: Option<u64>) -> String {
    let short: String = claude_id.chars().take(8).collect();
    match age_secs {
        Some(s) => format!(
            "Claude session: {}… (last active {} ago)",
            short,
            humanize_age_secs(s)
        ),
        None => format!("Claude session: {}…", short),
    }
}

/// TASK-243: one-line ambiguity-resolution summary for `aida session
/// end` — `(headline, optional Claude-session line)`. Pure formatter so
/// the role / Claude-id surfacing is unit-testable independent of the
/// interactive prompt. trace:TASK-243 | ai:claude
pub(crate) fn format_session_end_summary(
    id: &str,
    role: Option<&str>,
    scope: &str,
    branch: &str,
    claude_id: Option<&str>,
    age_secs: Option<u64>,
) -> (String, Option<String>) {
    let headline = format!(
        "{} (role:{}, scope {}, branch {})",
        &id[..id.len().min(8)],
        role.unwrap_or("unset"),
        scope,
        branch
    );
    let claude_line = claude_id.map(|cid| format_claude_session_line(cid, age_secs));
    (headline, claude_line)
}

/// TASK-243: resolve the Claude conversation linked to a lease — the
/// `claude_session_id` recorded in the session manifest (TASK-112) plus
/// a best-effort "last active" age from the conversation JSONL's mtime.
/// Returns `(None, None)` when there's no manifest or no recorded id.
/// trace:TASK-243 | ai:claude
pub(crate) fn resolve_lease_claude_session(lease: &SessionLease) -> (Option<String>, Option<u64>) {
    let project_root = find_main_worktree_root()
        .ok()
        .or_else(|| lease.parent_project_root.clone());
    let Some(root) = project_root else {
        return (None, None);
    };
    let manifest_path = session_manifest::manifest_path(&root, &lease.id);
    let Some(claude_id) = session_manifest::load(&manifest_path)
        .ok()
        .and_then(|m| m.claude_session_id)
    else {
        return (None, None);
    };
    let age_secs = session::claude_project_dir(&lease.worktree_path)
        .ok()
        .map(|dir| dir.join(format!("{}.jsonl", claude_id)))
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs());
    (Some(claude_id), age_secs)
}

pub(crate) fn resolve_session_to_end(
    id_query: Option<&str>,
    spec_query: Option<&str>,
    branch_query: Option<&str>,
    leases: &[SessionLease],
    yes: bool,
) -> Result<SessionLease> {
    // TASK-489: explicit --spec / --branch take precedence over the
    // positional id resolution. Both are scoped lookups against the
    // lease list rather than the id-prefix lookup.
    // trace:TASK-489 | ai:claude
    if let Some(q) = spec_query {
        return find_lease_by_spec(q, leases);
    }
    if let Some(q) = branch_query {
        return find_lease_by_branch(q, leases);
    }

    if let Some(q) = id_query {
        // TASK-489: if the positional looks like a SPEC-ID (uppercase
        // alpha-dash-digits — `TASK-489`, `STORY-86`, `FR-1-001`), the
        // user almost certainly means the spec, not the lease id. Skip
        // the hex-prefix path and route to the spec lookup so a 0/many
        // mismatch surfaces a spec-shaped error instead of "no session
        // matching `TASK-489`". The shared `looks_like_spec_id` accepts
        // lowercase too — `task-489` should route to the branch path —
        // so apply an extra uppercase-prefix guard here.
        // trace:TASK-489 | ai:claude
        if looks_like_spec_id(q) && positional_has_uppercase_spec_prefix(q) {
            return find_lease_by_spec(q, leases);
        }
        // Otherwise, prefer the existing 8-char lease-id-prefix lookup
        // (back-compat), and fall back to branch-name lookup on miss.
        // The branch fallback lets `aida session end task-489` resolve
        // without the user reaching for `--branch`. When both miss we
        // surface the branch error — it points the user at
        // `aida session leases`, the right next step regardless of
        // whether the user thought they were typing an id or a branch.
        // trace:TASK-489 | ai:claude
        return match find_lease_by_id_prefix(q, leases) {
            Ok(l) => Ok(l),
            Err(id_err) => {
                // An "ambiguous id" miss is a real prefix collision —
                // surface that error so the operator picks a longer
                // prefix. Only treat a "no session matching" miss as a
                // signal to fall through to branch resolution.
                let msg = id_err.to_string();
                // prose-ok: classifies our own find_lease_by_id_prefix error (ambiguous-prefix vs no-match), not external text
                if msg.contains("ambiguous") {
                    return Err(id_err);
                }
                find_lease_by_branch(q, leases)
            }
        };
    }

    // 1. cwd
    if let Ok(cwd) = std::env::current_dir() {
        let canon = cwd.canonicalize().unwrap_or(cwd);
        if let Some(l) = leases.iter().find(|&l| lease_covers_cwd(l, &canon)) {
            return Ok(l.clone());
        }
    }

    // 2. AIDA_SESSION_ID env var
    if let Ok(env_id) = std::env::var("AIDA_SESSION_ID") {
        if !env_id.trim().is_empty() {
            if let Ok(l) = find_lease_by_id_prefix(&env_id, leases) {
                eprintln!(
                    "{} resolved session via {} env var",
                    "→".dimmed(),
                    "AIDA_SESSION_ID".cyan()
                );
                return Ok(l);
            }
        }
    }

    // 3. ancestor PID
    let my_chain = process_probe::walk_ancestor_pids(std::process::id());
    let chain_set: std::collections::HashSet<u32> = my_chain.into_iter().collect();
    let ancestor_match: Vec<&SessionLease> = leases
        .iter()
        .filter(|l| {
            l.creator_pid
                .map(|p| chain_set.contains(&p))
                .unwrap_or(false)
        })
        .collect();
    match ancestor_match.len() {
        0 => {}
        1 => {
            eprintln!(
                "{} resolved session via ancestor PID match (creator_pid {})",
                "→".dimmed(),
                ancestor_match[0].creator_pid.unwrap()
            );
            return Ok(ancestor_match[0].clone());
        }
        n => {
            eprintln!(
                "{} {} active leases all have an ancestor PID match — pass an id explicitly",
                "Warning:".yellow().bold(),
                n
            );
        }
    }

    // 4. single-active fallback
    if leases.len() == 1 {
        let only = &leases[0];
        // TASK-243: surface role + the linked Claude session so the
        // user can tell which session they're about to end without a
        // separate `aida session leases` cross-reference.
        let (claude_id, age_secs) = resolve_lease_claude_session(only);
        let (headline, claude_line) = format_session_end_summary(
            &only.id,
            only.role.as_deref(),
            &only.scope,
            &only.branch,
            claude_id.as_deref(),
            age_secs,
        );
        if yes {
            eprintln!(
                "{} only one active session ({}); -y given, ending it",
                "→".dimmed(),
                headline
            );
            return Ok(only.clone());
        }
        eprintln!("Only one active session: {}", headline);
        if let Some(line) = &claude_line {
            eprintln!("  {}", line.dimmed());
        }
        eprint!("End it? [y/N] ");
        use std::io::Write;
        let _ = std::io::stderr().flush();
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return Ok(only.clone());
        }
        anyhow::bail!("aborted by user");
    }

    // 5. error with listing
    let mut msg = String::from(
        "no active session resolvable from this shell — pass an id or `aida session leases`\n\nActive sessions:",
    );
    for l in leases {
        // TASK-243: include role here too so the multi-session listing
        // is consistent with the single-active prompt and `aida session
        // leases`. trace:TASK-243 | ai:claude
        msg.push_str(&format!(
            "\n  {} role:{} {} ({})",
            &l.id[..8],
            l.role.as_deref().unwrap_or("unset"),
            l.scope,
            l.worktree_path.display()
        ));
    }
    anyhow::bail!(msg)
}

#[cfg(test)]
#[path = "tests/session_end_summary_tests.rs"]
mod session_end_summary_tests;

/// Look up a lease by id prefix (case-insensitive). Errors on no-match
/// or ambiguous prefix. trace:STORY-73 | ai:claude
pub(crate) fn find_lease_by_id_prefix(
    query: &str,
    leases: &[SessionLease],
) -> Result<SessionLease> {
    let q = query.to_lowercase();
    let matches: Vec<&SessionLease> = leases
        .iter()
        .filter(|l| l.id.to_lowercase().starts_with(&q))
        .collect();
    match matches.len() {
        0 => anyhow::bail!("no session matching `{}`", query),
        1 => Ok(matches[0].clone()),
        n => {
            // BUG-312: an honest "use a longer prefix" left the operator
            // grepping `.aida/sessions/` to find HOW long was enough. List
            // every match's full id + scope so the operator can pick.
            // trace:BUG-312 | ai:claude
            let listing: String = matches
                .iter()
                .map(|l| format!("  {}  ({})", l.id, l.scope))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "ambiguous session id `{}` — matches {} sessions:\n{}",
                query,
                n,
                listing
            )
        }
    }
}

/// TASK-489: is the alpha prefix of `s` (the chars before the first `-`)
/// all uppercase? Used to disambiguate the positional resolution between
/// SPEC-ID (`TASK-489` → spec lookup) and branch name (`task-489` →
/// branch lookup) when the shared `looks_like_spec_id` would accept both.
/// trace:TASK-489 | ai:claude
pub(crate) fn positional_has_uppercase_spec_prefix(s: &str) -> bool {
    let bytes = s.trim().as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        if !bytes[i].is_ascii_uppercase() {
            return false;
        }
        i += 1;
    }
    i > 0
}

/// TASK-489: look up a lease by spec ID (case-insensitive equality against
/// `lease.scope`). Errors with the message shape the user friction asked
/// for: zero matches surfaces `aida session leases` as the diagnostic;
/// many matches lists every candidate plus its lease id so the operator
/// can disambiguate. The lease's `scope` field holds the raw `--owns`
/// argument from session start — for normal pickups that's the SPEC-ID,
/// matching the operator's mental model exactly.
/// trace:TASK-489 | ai:claude
pub(crate) fn find_lease_by_spec(query: &str, leases: &[SessionLease]) -> Result<SessionLease> {
    let q = query.trim().to_lowercase();
    let matches: Vec<&SessionLease> = leases
        .iter()
        .filter(|l| l.scope.to_lowercase() == q)
        .collect();
    match matches.len() {
        0 => anyhow::bail!(
            "No lease found for spec `{}` — run `aida session leases` to see active leases",
            query
        ),
        1 => Ok(matches[0].clone()),
        n => {
            let listing: String = matches
                .iter()
                .map(|l| format!("  {}  scope:{}  branch:{}", l.id, l.scope, l.branch))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "{} leases own spec `{}` — disambiguate via lease id:\n{}",
                n,
                query,
                listing
            )
        }
    }
}

/// TASK-489: look up a lease by branch name (case-sensitive — git refs
/// are case-sensitive). Same error shape as `find_lease_by_spec`.
/// trace:TASK-489 | ai:claude
pub(crate) fn find_lease_by_branch(query: &str, leases: &[SessionLease]) -> Result<SessionLease> {
    let q = query.trim();
    let matches: Vec<&SessionLease> = leases.iter().filter(|l| l.branch == q).collect();
    match matches.len() {
        0 => anyhow::bail!(
            "No lease found for branch `{}` — run `aida session leases` to see active leases",
            query
        ),
        1 => Ok(matches[0].clone()),
        n => {
            let listing: String = matches
                .iter()
                .map(|l| format!("  {}  scope:{}  branch:{}", l.id, l.scope, l.branch))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "{} leases ride branch `{}` — disambiguate via --spec or lease id:\n{}",
                n,
                query,
                listing
            )
        }
    }
}

/// BUG-312: shortest prefix of `target_id` that does not collide with any
/// other id in `all_ids`, floored at `min_len`. Used by `aida session
/// leases` to render an id that the operator can paste straight back into
/// `aida session end`. HLC-derived UUIDs put the timestamp at the start,
/// so two leases created in the same generation window collide on the
/// historical 8-char prefix; this function bumps just those colliding
/// rows wider while leaving solitary ids at the short form.
/// trace:BUG-312 | ai:claude
pub(crate) fn unique_prefix_len(target_id: &str, all_ids: &[&str], min_len: usize) -> usize {
    let max_len = target_id.len();
    let floor = min_len.min(max_len);
    for len in floor..=max_len {
        let prefix = &target_id[..len];
        let collides = all_ids
            .iter()
            .any(|other| *other != target_id && other.starts_with(prefix));
        if !collides {
            return len;
        }
    }
    max_len
}

/// STORY-52: locate the parent project's cargo `target/` directory so a
/// session worktree can reuse its build cache. Returns the canonicalized
/// path when `target/` exists (Rust project that has been built), `None`
/// otherwise. Pure-function over the filesystem so callers can test the
/// session-start flow with a temp dir.
/// trace:STORY-52 | ai:claude
pub(crate) fn detect_cargo_target_dir(
    project_root: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let target = project_root.join("target");
    if !target.is_dir() {
        return None;
    }
    Some(target.canonicalize().unwrap_or(target))
}

/// BUG-1783: Isolate a shared target dir to a worktree-specific path.
/// If the worktree path lacks a file name component, falls back to the parent
/// target dir with a warning so we don't silently collapse back into the
/// same collision hazard one directory deeper.
/// trace:BUG-1783 | ai:codex
pub(crate) fn isolate_cargo_target_dir(
    parent_target: &std::path::Path,
    worktree_path: &std::path::Path,
) -> std::path::PathBuf {
    use colored::Colorize;
    match worktree_path.file_name() {
        Some(name) if !name.is_empty() => parent_target.join("worktrees").join(name),
        _ => {
            eprintln!(
                "  {} worktree path {} lacks a valid dir name; falling back to unisolated parent target {}",
                "warning:".yellow(),
                worktree_path.display(),
                parent_target.display()
            );
            parent_target.to_path_buf()
        }
    }
}

/// STORY-52: write the worktree-local `.aida/session-env.sh` that the user
/// sources after `cd`-ing into the session worktree. Sourcing it sets
/// `CARGO_TARGET_DIR` to the parent's `target/worktrees/<worktree-dir-name>` (BUG-1783) so
/// cargo reuses the build cache without colliding with main. The file is written
/// into the worktree's `.aida/` (created here if it doesn't already exist), which
/// lives alongside the symlinked runtime subdirs (sessions/, roles/,
/// cache.db, etc.) that `session_start` set up moments earlier.
/// trace:STORY-52 | ai:claude
/// trace:BUG-1783 | ai:codex
pub(crate) fn write_session_env_file(
    worktree_path: &std::path::Path,
    cargo_target_dir: &std::path::Path,
) -> Result<()> {
    let aida_dir = worktree_path.join(".aida");
    std::fs::create_dir_all(&aida_dir)?;
    let env_path = aida_dir.join("session-env.sh");
    let agent_type = std::env::var("AIDA_AGENT_TYPE")
        .ok()
        .filter(|s| !s.trim().is_empty());
    // BUG-766: bake the coordinating binary into the shim so sourcing it in
    // any shell resolves `aida` to the build that created this worktree,
    // never a stale installed binary earlier on the raw PATH.
    let aida_bin = resolve_aida_exe();
    let aida_bin = (aida_bin.is_absolute() && aida_bin.exists()).then_some(aida_bin);
    let body =
        render_session_env_file(cargo_target_dir, agent_type.as_deref(), aida_bin.as_deref());
    std::fs::write(&env_path, body)?;
    Ok(())
}

/// STORY-52: build the body of `.aida/session-env.sh`. Split out so unit
/// tests can assert the export shape without touching the filesystem.
/// trace:STORY-52 | ai:claude
pub(crate) fn render_session_env_file(
    cargo_target_dir: &std::path::Path,
    agent_type: Option<&str>,
    aida_bin: Option<&std::path::Path>,
) -> String {
    let mut body = format!(
        "# Generated by `aida session start` — source after cd-ing into\n\
         # this worktree to share the parent project's cargo build cache.\n\
         # trace:STORY-52 | ai:claude\n\
         export CARGO_TARGET_DIR={}\n",
        shell_single_quote(&cargo_target_dir.display().to_string())
    );
    if let Some(agent_type) = agent_type {
        body.push_str(&format!(
            "export AIDA_AGENT_TYPE={}\n",
            shell_single_quote(agent_type)
        ));
    }
    // BUG-766: pin the coordinating binary. The PATH line is a plain
    // assignment (PATH is already exported in any shell) so the narrow
    // `export VAR='value'` parser in `parse_session_env` skips it instead
    // of mangling the `"$PATH"` reference; the in-process launch path gets
    // the same prepend from `export_coordinating_bin_env` at startup.
    if let Some(bin) = aida_bin {
        body.push_str(&format!(
            "export AIDA_BIN={}\n",
            shell_single_quote(&bin.display().to_string())
        ));
        if let Some(dir) = bin.parent() {
            body.push_str(&format!(
                "PATH={}:\"$PATH\"\n",
                shell_single_quote(&dir.display().to_string())
            ));
        }
    }
    body
}

/// TASK-63: parse a `.aida/session-env.sh` body into `(name, value)`
/// pairs. Pure: no env mutation, so unit tests can exercise the parser
/// without racing the live process env across parallel test threads.
///
/// Parser is intentionally narrow: it understands only the shape that
/// `render_session_env_file` writes — `export VAR='single-quoted-value'`
/// with `'\''` close-reopen escaping. Lines we don't recognize are
/// skipped silently, so a manually-edited shim with extra noise doesn't
/// poison the env. trace:TASK-63 | ai:claude
pub(crate) fn parse_session_env(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("export ") else {
            continue;
        };
        let Some((name, raw_value)) = rest.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let mut chars = name.chars();
        let first = chars.next();
        let valid_first = matches!(first, Some(c) if c.is_ascii_alphabetic() || c == '_');
        let valid_rest = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid_first || !valid_rest {
            continue;
        }
        let value = unquote_shell_single_quoted(raw_value.trim());
        out.push((name.to_string(), value));
    }
    out
}

// trace:TASK-1577 | ai:codex
pub(crate) fn is_session_cargo_target_dir(
    target_dir: &std::path::Path,
    worktree_path: &std::path::Path,
) -> bool {
    let Some(worktree_name) = worktree_path.file_name() else {
        return false;
    };
    let mut components = target_dir.components().rev();
    matches!(components.next(), Some(std::path::Component::Normal(name)) if name == worktree_name)
        && matches!(components.next(), Some(std::path::Component::Normal(name)) if name == "worktrees")
}

// trace:TASK-1577 | ai:codex
pub(crate) fn session_cargo_target_dir(
    worktree_path: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let body = std::fs::read_to_string(worktree_path.join(".aida/session-env.sh")).ok()?;
    parse_session_env(&body)
        .into_iter()
        .find(|(name, _)| name == "CARGO_TARGET_DIR")
        .map(|(_, value)| std::path::PathBuf::from(value))
}

// trace:TASK-1577 | ai:codex
pub(crate) fn remove_session_cargo_target_dir(
    target_dir: &std::path::Path,
    worktree_path: &std::path::Path,
) {
    use colored::Colorize;
    if !is_session_cargo_target_dir(target_dir, worktree_path) || !target_dir.exists() {
        return;
    }
    if let Err(error) = std::fs::remove_dir_all(target_dir) {
        eprintln!(
            "  {} could not remove session cargo target {}: {}",
            "warning:".yellow(),
            target_dir.display(),
            error
        );
    }
}

/// TASK-63: apply parsed env pairs to the current process so the
/// subsequent `exec claude` inherits them. Returns the names that were
/// applied so the caller can echo them to the user. Calls
/// `std::env::set_var` for each pair — the wrapping `unsafe { }` is
/// forward-compatibility with Edition 2024 where `set_var` is marked
/// unsafe; safe here because `session_start --launch` is single-threaded
/// between parse and exec. trace:TASK-63 | ai:claude
pub(crate) fn apply_session_env_to_process(body: &str) -> Vec<String> {
    // Only the allowlisted names, never PATH / LD_PRELOAD / BASH_ENV from a
    // branch-committed file. trace:BUG-1624 | ai:claude
    let pairs = trusted_session_env(body, &resolve_aida_exe());
    let mut applied = Vec::with_capacity(pairs.len());
    for (name, value) in pairs {
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(&name, &value);
        }
        applied.push(name);
    }
    applied
}

/// The only names [`render_session_env_file`] writes, and so the only names
/// taken from a `.aida/session-env.sh`. The file lives in the worktree, where
/// a branch can commit its own copy, so anything else (`PATH`,
/// `PROMPT_COMMAND`, `BASH_ENV`, `LD_PRELOAD`, ...) is dropped.
// trace:BUG-1624 | ai:claude
pub(crate) const SESSION_ENV_NAMES: [&str; 3] = ["CARGO_TARGET_DIR", "AIDA_AGENT_TYPE", "AIDA_BIN"];

/// The trustworthy subset of a `.aida/session-env.sh` body: allowlisted
/// names only, `CARGO_TARGET_DIR` only when absolute, and `AIDA_BIN`
/// never taken from the file. When the file pins a binary, the pin is
/// replaced by `running_exe` (this process's own binary) if that is an
/// absolute, existing path, and dropped otherwise.
// trace:BUG-1624 | ai:claude
pub(crate) fn trusted_session_env(
    body: &str,
    running_exe: &std::path::Path,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, value) in parse_session_env(body) {
        if !SESSION_ENV_NAMES.contains(&name.as_str()) || out.iter().any(|(n, _)| *n == name) {
            continue;
        }
        // A NUL byte can't be carried by an env var (`set_var` panics), so a
        // value containing one is dropped. trace:BUG-1627 | ai:claude
        if value.contains('\0') {
            continue;
        }
        let value = match name.as_str() {
            "CARGO_TARGET_DIR" if !std::path::Path::new(&value).is_absolute() => continue,
            // Any non-empty AIDA_AGENT_TYPE turns on the advisor code-gate
            // agent carve-out and adds a type mailbox identity, and the session
            // lease does not record the type. So only a known agent type is
            // taken, in its canonical spelling. trace:BUG-1627 | ai:claude
            "AIDA_AGENT_TYPE" => match trusted_agent_type(&value) {
                Some(canonical) => canonical,
                None => continue,
            },
            "AIDA_BIN" => {
                if !(running_exe.is_absolute() && running_exe.is_file()) {
                    continue;
                }
                running_exe.display().to_string()
            }
            _ => value,
        };
        out.push((name, value));
    }
    out
}

/// The canonical spelling of a known agent type (`claude`, `codex`,
/// `antigravity`, `shell`, `web`), or `None` for anything
/// [`agent_registry::normalize_agent_type`] does not recognize.
// trace:BUG-1627 | ai:claude
pub(crate) fn trusted_agent_type(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let canonical = agent_registry::normalize_agent_type(raw.to_string());
    (canonical != "other").then_some(canonical)
}

/// Inverse of `shell_single_quote` for the narrow shape we write.
/// `'X'` → `X`. `'a'\''b'` → `a'b`. Bare (unquoted) values are returned
/// as-is — POSIX-y enough for the shim's purposes. trace:TASK-63 | ai:claude
pub(crate) fn unquote_shell_single_quoted(raw: &str) -> String {
    let bytes = raw.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'\'' || bytes[bytes.len() - 1] != b'\'' {
        return raw.to_string();
    }
    let inner = &raw[1..raw.len() - 1];
    inner.replace("'\\''", "'")
}

/// Wrap a string in POSIX single quotes for safe inclusion in shell source.
/// `'` inside the value is escaped via the standard `'\''` close-reopen trick.
/// trace:STORY-52 | ai:claude
pub(crate) fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Render an argv slice as a copy-pasteable shell command line: an
/// element with shell-special characters (whitespace, quotes, globs, …)
/// is wrapped in POSIX single quotes; a plain word is left bare so the
/// output stays readable. trace:BUG-225 | ai:claude
pub(crate) fn shell_join_display(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            let bare = !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_./=:-+@".contains(c));
            if bare {
                a.clone()
            } else {
                shell_single_quote(a)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionEndUnshippedWork {
    pub(crate) lease_id: String,
    pub(crate) scope: String,
    pub(crate) branch: String,
    pub(crate) commits_ahead: u32,
}

pub(crate) fn classify_session_end_unshipped_work(
    lease_id: &str,
    scope: &str,
    branch: &str,
    commits_ahead: Option<u32>,
    pr_state: workflow_hints::PrState,
) -> Option<SessionEndUnshippedWork> {
    let commits = commits_ahead.unwrap_or(0);
    if commits == 0 || !matches!(pr_state, workflow_hints::PrState::Absent) {
        return None;
    }
    Some(SessionEndUnshippedWork {
        lease_id: lease_id.to_string(),
        scope: scope.to_string(),
        branch: branch.to_string(),
        commits_ahead: commits,
    })
}

pub(crate) fn session_end_pr_state_for_scope_or_branch(
    project_root: &std::path::Path,
    scope: &str,
    branch: &str,
) -> workflow_hints::PrState {
    // trace:BUG-853 | ai:codex
    // Reviewer/fixup sessions may use a local alias branch like `pr-1648`
    // while the forge PR head is still the implementation branch. In that
    // shape, branch-keyed lookup misses; the PR/MR scope is the durable id.
    match change_lookup_for_branch(project_root, branch) {
        crate::forge::ChangeLookup::Found(c) => return workflow_hints::PrState::Open(c.id),
        crate::forge::ChangeLookup::NoChange => {}
        crate::forge::ChangeLookup::CliMissing
        | crate::forge::ChangeLookup::CliFailed(_)
        | crate::forge::ChangeLookup::Unreachable(_) => return workflow_hints::PrState::Unknown,
    }

    let Some(pr_number) = pr_number_from_scope(scope) else {
        return workflow_hints::PrState::Absent;
    };
    let mut sink = network_retry::NoopSink;
    match crate::forge::forge_for(project_root).change_metadata(pr_number, &mut sink) {
        Ok(metadata) => match metadata.state {
            crate::forge::ChangeState::Open => workflow_hints::PrState::Open(pr_number),
            crate::forge::ChangeState::Merged => workflow_hints::PrState::Open(pr_number),
            crate::forge::ChangeState::Closed => workflow_hints::PrState::Absent,
        },
        Err(_) => workflow_hints::PrState::Absent,
    }
}

pub(crate) fn session_end_unshipped_warning_lines(work: &SessionEndUnshippedWork) -> Vec<String> {
    vec![
        format!(
            "session end detected {} unshipped commit{} for {} on branch `{}` with no open PR.",
            work.commits_ahead,
            if work.commits_ahead == 1 { "" } else { "s" },
            work.scope,
            work.branch
        ),
        format!(
            "Lease {} will be removed, but the branch is still local/unshipped; run `git push` + `aida pr ship` or recover manually.",
            work.lease_id
        ),
    ]
}

pub(crate) fn session_end_unshipped_punt_record(
    work: &SessionEndUnshippedWork,
    now: chrono::DateTime<chrono::Utc>,
) -> punt::PuntRecord {
    punt::PuntRecord {
        timestamp: now,
        spec: work.scope.clone(),
        category: aida_core::PuntCategory::Other,
        detail: format!(
            "session end observed {} commit{} ahead on branch `{}` with no open PR; work may need manual push/PR recovery",
            work.commits_ahead,
            if work.commits_ahead == 1 { "" } else { "s" },
            work.branch
        ),
        lean: Some("recover by pushing the branch and opening/shipping a PR".to_string()),
        raised_by: Some("session-end".to_string()),
        resolution_path: "punted".to_string(),
        classification: Some("UNSHIPPED-SESSION-END".to_string()),
        escalation_reason: None,
        answer: None,
        answered_by: None,
        decision: Some("visibility-warning".to_string()),
        principle_link: None,
        calibration_pair: None,
        paused_at: Some(now),
        resolved_at: None,
    }
}

pub(crate) fn emit_session_end_unshipped_warning(work: &SessionEndUnshippedWork) {
    for (idx, line) in session_end_unshipped_warning_lines(work).iter().enumerate() {
        if idx == 0 {
            eprintln!("{} {}", "warning:".yellow().bold(), line);
        } else {
            eprintln!("  {}", line);
        }
    }
}

pub(crate) fn record_session_end_unshipped_work(
    project_root: &std::path::Path,
    work: &SessionEndUnshippedWork,
) {
    let record = session_end_unshipped_punt_record(work, chrono::Utc::now());
    if let Err(e) = punt::append_to_ledger(project_root, &record) {
        eprintln!(
            "{} could not write session-end punt ledger record: {e}",
            "warning:".yellow().bold()
        );
    }
}

/// BUG-422: should `aida session end` auto-skip the CI/PR probe because we're
/// in a non-interactive context? The probe shells out to `gh` with no timeout;
/// with no TTY to Ctrl+C or answer a prompt it can hang the lease teardown
/// forever — which breaks the multi-agent handoff (tearing down a finished
/// worktree from outside the agent). True only when the probe WOULD otherwise
/// run (`!skip_ci && !force`), the caller did NOT explicitly ask to wait for CI
/// (`--wait-ci`/`--watch-ci` is honored even headless — that's an explicit
/// choice), and stdin is not a TTY. trace:BUG-422 | ai:claude
pub(crate) fn non_tty_skips_ci_probe(
    skip_ci: bool,
    force: bool,
    explicit_ci_wait: bool,
    stdin_is_tty: bool,
) -> bool {
    !skip_ci && !force && !explicit_ci_wait && !stdin_is_tty
}

#[cfg(test)]
#[path = "tests/bug422_ci_probe_tests.rs"]
mod bug422_ci_probe_tests;

/// What `aida session end`'s confirmation gate should do.
// trace:BUG-776 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionEndConfirm {
    /// `--yes` / `--force` supplied — end the session without asking.
    Skip,
    /// Real TTY, no skip flag — ask `Continue? [y/N]`.
    Prompt,
    /// No TTY and no skip flag — nobody can answer. Fail loudly instead of
    /// no-op'ing: a silent exit that leaves the lease behind looks like the
    /// command ran and is how leases get orphaned.
    RefuseNonInteractive,
}

/// Pure decision for the confirmation gate, so the "never silently no-op"
/// contract is unit-testable.
// trace:BUG-776 | ai:claude
pub(crate) fn session_end_confirm_action(
    yes: bool,
    force: bool,
    stdin_is_tty: bool,
) -> SessionEndConfirm {
    if yes || force {
        SessionEndConfirm::Skip
    } else if stdin_is_tty {
        SessionEndConfirm::Prompt
    } else {
        SessionEndConfirm::RefuseNonInteractive
    }
}

/// The refusal text for [`SessionEndConfirm::RefuseNonInteractive`]. Names
/// the surviving lease explicitly so the caller can't read the exit as "it
/// ran".
// trace:BUG-776 | ai:claude
pub(crate) fn session_end_confirm_refusal(
    session_id: &str,
    lease_path: &std::path::Path,
) -> String {
    format!(
        "session {} was NOT ended — confirmation is required and stdin is not a terminal, \
         so nobody can answer the prompt. The lease at {} is still held. \
         Re-run with `--yes` (or `--force`) to confirm non-interactively.",
        session_id,
        lease_path.display()
    )
}

#[cfg(test)]
#[path = "tests/bug776_session_end_confirm_tests.rs"]
mod bug776_session_end_confirm_tests;

/// BUG-779: did this `session end` invocation name a specific session? With
/// zero active leases that distinction decides the exit code, and the exit code
/// is what the `aida()` shell wrapper branches on: a BARE `session end` with
/// nothing to end is a benign no-op (exit 0, empty stdout, eval is a no-op),
/// while an EXPLICIT target that cannot exist is a failed request that must
/// exit non-zero so its error text is never eval'd as shell.
///
/// Returns the named target (first of id / --spec / --branch), ignoring blanks.
// trace:BUG-779 | ai:claude
pub(crate) fn session_end_explicit_target(
    id_query: Option<&str>,
    spec_query: Option<&str>,
    branch_query: Option<&str>,
) -> Option<String> {
    [id_query, spec_query, branch_query]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|q| !q.is_empty())
        .map(str::to_string)
}

// The shell matrix (bash + zsh-when-present) the wrapper tests below drive.
// trace:TASK-1174 | ai:claude
#[cfg(test)]
#[path = "tests/shell_wrapper_harness.rs"]
pub(crate) mod shell_wrapper_harness;

#[cfg(test)]
#[path = "tests/bug_779_wrapper_eval_tests.rs"]
mod bug_779_wrapper_eval_tests;

// trace:TASK-1171 | ai:claude
#[cfg(test)]
#[path = "tests/task_1171_eval_channel_tests.rs"]
mod task_1171_eval_channel_tests;

// trace:BUG-0445 | ai:claude
#[cfg(test)]
#[path = "tests/bug_0445_init_side_channel_tests.rs"]
mod bug_0445_init_side_channel_tests;

// trace:BUG-2000 | ai:claude
#[cfg(test)]
#[path = "tests/bug_2000_zsh_local_tests.rs"]
mod bug_2000_zsh_local_tests;

/// BUG-1952: the root `session end` resolves every runtime-state path against.
/// A main checkout (real `.git` directory) is returned unchanged. From a linked
/// worktree, git's own registry names the main checkout (via the shared
/// common dir, which survives the worktree's `.aida` symlinks being stripped),
/// and that is used when it is a real checkout. Anything else — a bare main, a
/// submodule, git unavailable — keeps the invocation root, which is the
/// pre-fix behavior.
// trace:BUG-1952 | ai:claude
pub(crate) fn resolve_session_end_root(invocation_root: &std::path::Path) -> std::path::PathBuf {
    if invocation_root.join(".git").is_dir() {
        return invocation_root.to_path_buf();
    }
    let main = main_worktree_root_from(invocation_root);
    if main != invocation_root && main.join(".git").is_dir() {
        main
    } else {
        invocation_root.to_path_buf()
    }
}

// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn session_end(
    id_query: Option<&str>,
    spec_query: Option<&str>,
    branch_query: Option<&str>,
    yes: bool,
    force: bool,
    purge_cc: bool,
    wait_ci: bool,
    watch_ci: bool,
    skip_ci: bool,
    // STORY-714: explicit --return (back-compat; pooled trees now return by
    // default). TASK-985: --remove forces deletion instead of returning.
    return_to_pool: bool,
    remove: bool,
    // BUG-1986: the dedicated live-process override. `--force` alone no
    // longer removes a tree another live process is using.
    include_in_use: bool,
) -> Result<()> {
    // Returning is now the default; the flag is kept for compat.
    let _ = return_to_pool;
    // BUG-1952: run from inside the worktree being ended, `find_project_root`
    // stops at the worktree's `.git` file. Resolve the MAIN checkout now, before
    // the runtime symlinks are stripped, so the mcp-claim, manifest, BUG-483
    // peer guard, pool checks and store detection below never resolve against
    // the dying tree. trace:BUG-1952 | ai:claude
    let project_root = resolve_session_end_root(&find_project_root()?);
    let leases = list_leases(&project_root);
    if leases.is_empty() {
        // BUG-779: a BARE `session end` with nothing to end is a benign no-op
        // (exit 0 — the shell wrapper evals empty stdout and moves on). But an
        // EXPLICIT target that cannot exist is a failed request; exiting 0 there
        // let the wrapper treat the error text as shell code.
        // trace:BUG-779 | ai:claude
        if let Some(q) = session_end_explicit_target(id_query, spec_query, branch_query) {
            anyhow::bail!(
                "No lease found for `{}` — there are no active sessions in this project. \
                 Run `aida session leases` to see active leases",
                q
            );
        }
        eprintln!("(no active sessions)");
        return Ok(());
    }

    let target = resolve_session_to_end(id_query, spec_query, branch_query, &leases, yes)?;

    // BUG-361: visibility-first guard for the ceiling variant where an agent
    // commits locally and exits without any lifecycle command (`queue done`,
    // `/aida-pr`, or `aida pr ship`). `session end` is not a hard blocker here:
    // it emits a BUG-360-style warning and writes a STORY-325 ledger record so
    // the operator's next sweep sees the unshipped branch. This only fires when
    // GH confirms no open PR; unknown GH states never assert "missing PR".
    // trace:BUG-361
    let commits_ahead = branch_unshipped_patch_count_default(&project_root, &target.branch)
        .or_else(|| branch_commits_ahead_main(&project_root, &target.branch));
    let pr_state =
        session_end_pr_state_for_scope_or_branch(&project_root, &target.scope, &target.branch);
    // BUG-367: if the spec the lease owns has auto-bumped to Completed, or if the PR
    // for this branch has already been squash-merged / merged, the work is shipped
    // regardless of what the local branch looks like. Suppress warning.
    // trace:BUG-367 | ai:antigravity
    let mut is_completed = false;
    let store_path = project_root.join(".aida-store");
    if store_path.exists() {
        if let Ok(storage) = Storage::new(store_path).load() {
            if let Some(req) = storage.get_requirement_by_spec_id(&target.scope) {
                if req.status == RequirementStatus::Completed {
                    is_completed = true;
                }
            }
        }
    }
    if !is_completed {
        if let PrLookup::Found(_) =
            detect_merged_pr_for_branch_via_forge(&project_root, &target.branch)
        {
            is_completed = true;
        }
    }
    let session_end_unshipped_work = if is_completed {
        None
    } else {
        classify_session_end_unshipped_work(
            &target.id,
            &target.scope,
            &target.branch,
            commits_ahead,
            pr_state,
        )
    };
    if let Some(work) = &session_end_unshipped_work {
        emit_session_end_unshipped_warning(work);
    }

    // STORY-127 detector (4): leaving this role's session while a DIFFERENT
    // role's queue has work waiting. Warn (don't block) so the user can
    // switch role first instead of walking away from queued work.
    // trace:STORY-127 | ai:claude
    {
        let role_counts = read_queue_role_counts(&project_root);
        let waiting = cross_role_queue_waiting(target.role.as_deref(), &role_counts);
        if let Some(msg) = session_end_cross_role_warning(&waiting) {
            eprintln!("  {} {}", "Warning:".yellow().bold(), msg);
        }
    }

    // TASK-111: CI awareness. Probe the branch for an open PR's CI state
    // and surface info / prompt / warn based on the state. --skip-ci and
    // --force both bypass the probe (force already gets a noisy override
    // path; --skip-ci is the quieter explicit "don't bother"). Probe
    // happens BEFORE the BUG-61 live-claude check so the user can decide
    // whether to bail-and-keep-fixing without first wading through the
    // claude-process disclosure. trace:TASK-111 | ai:claude
    if non_tty_skips_ci_probe(
        skip_ci,
        force,
        wait_ci || watch_ci,
        std::io::stdin().is_terminal(),
    ) {
        // BUG-422: non-interactive shell — the `gh` CI/PR probe has no timeout
        // and would hang the lease teardown with no TTY to interrupt it. Skip
        // it (equivalent to the proven `--skip-ci` workaround). trace:BUG-422
        eprintln!(
            "  {} non-interactive shell — skipping the CI/PR probe (it would hang the lease \
             teardown with no TTY). Pass --wait-ci from a TTY to force it, or --skip-ci to silence.",
            "Note:".dimmed()
        );
    } else if !skip_ci && !force {
        // STORY-516: forge-routed. Probe on the session's resolved
        // `project_root` rather than the process cwd, so the forge that answers
        // is the driven repository's. trace:TASK-1273 | ai:claude
        let forge_kind = crate::forge::resolve_forge_kind(&project_root);
        let probe = ci_probe_with_forge(&project_root, forge_kind, &target.branch);
        // TASK-233: --watch-ci blocks like --wait-ci, so it produces the
        // same `CiAction::Wait`; the difference is the live display.
        match decide_ci_action(&probe, wait_ci || watch_ci, yes) {
            CiAction::Proceed => {}
            CiAction::Wait => {
                let final_probe = if watch_ci {
                    eprintln!(
                        "{} CI in progress — watching until terminal state (Ctrl+C to stop watching)",
                        "→".cyan()
                    );
                    // TASK-1165: hand the already-resolved session root down so
                    // the CiTerminal emit can't re-derive one from the cwd.
                    watch_ci_for_context_via_forge(&project_root, &target.branch, false)
                // STORY-516
                } else {
                    eprintln!(
                        "{} CI in progress — waiting for terminal state (poll every 30s; Ctrl+C to skip)",
                        "→".cyan()
                    );
                    wait_for_ci_terminal(Some(&project_root), &target.branch)
                };
                match decide_ci_action(&final_probe, false, yes) {
                    CiAction::Proceed => {}
                    CiAction::Wait => {} // can't loop on Wait again
                    CiAction::Cancel(msg) => {
                        eprintln!("{}", msg);
                        return Ok(());
                    }
                }
            }
            CiAction::Cancel(msg) => {
                eprintln!("{}", msg);
                return Ok(());
            }
        }
    }

    // BUG-734: a lease with no worktree (empty `worktree_path` — e.g. a
    // reviewer lease that never created one) has nothing on disk to tear
    // down. An empty path must never reach the worktree scans below: the
    // live-claude probe, the dirty gate, and `git worktree remove` would
    // all evaluate the caller's cwd (or every path) instead of a real
    // worktree, blocking the end of a lease that only needs its record
    // deleted. trace:BUG-734 | ai:claude
    let has_worktree = !target.worktree_path.as_os_str().is_empty();
    // Capture before `git worktree remove` deletes the shim. The pure guard
    // prevents malformed or legacy fallback values from targeting parent `target/`.
    // trace:TASK-1577 | ai:codex
    let session_target_dir = has_worktree
        .then(|| session_cargo_target_dir(&target.worktree_path))
        .flatten()
        .filter(|path| is_session_cargo_target_dir(path, &target.worktree_path));

    // BUG-61: detect live `claude` processes whose cwd is under the
    // worktree we're about to remove. If we don't, `git worktree remove`
    // succeeds (cwd is just a soft handle), the dir is unlinked, and the
    // claude keeps running with `(deleted)` as its cwd — leaking memory,
    // confusing future liveness probes, and potentially firing hooks at
    // paths that no longer exist. Default refuses with a clear message;
    // `--include-in-use` SIGTERMs them with a 5s grace, then SIGKILLs.
    // `--force` alone no longer does: a sweep passing `--force` for a dirty
    // tree or a missing TTY must not kill a live agent. trace:BUG-1986 | ai:claude
    // trace:BUG-61 | ai:claude
    // BUG-734: worktree-less lease ⇒ no process-leak check — nothing to
    // remove, so nothing can leak. trace:BUG-734 | ai:claude
    let leaked = if has_worktree {
        probe_live_claudes_in_worktree(&target.worktree_path)
    } else {
        Vec::new()
    };
    if !leaked.is_empty() {
        if !include_in_use {
            let mut msg = format!(
                "{} live claude process{} inside worktree {} — would leak with `(deleted)` cwd if we removed the worktree:",
                leaked.len(),
                if leaked.len() == 1 { "" } else { "es" },
                target.worktree_path.display()
            );
            for p in &leaked {
                msg.push_str(&format!("\n  pid {} cwd {}", p.pid, p.cwd.display()));
            }
            msg.push_str(
                "\n\nExit those claude(s) first, or pass --include-in-use to SIGTERM them.",
            ); // trace:BUG-1986 | ai:claude
            anyhow::bail!(msg)
        }
        // With --include-in-use they are terminated later, after the
        // confirmation and the dirty gate: a declined prompt, a non-TTY
        // refusal or a dirty-tree refusal must leave the agents running.
        // trace:BUG-1986 | ai:claude
    }

    // Refuse to delete a worktree any live process is still using: a cwd
    // inside it, an open file under it, or a program running from it. A bulk
    // sweep that loops `session end` over sessions it believes are stale is
    // therefore safe by default, because each call refuses on its own. The
    // caller's own process chain (this command, its shell, the agent that
    // owns the session when run from inside the tree) is exempt, so ending
    // your own session still works. `--include-in-use` overrides; `--force`
    // does not (trace:BUG-1986 | ai:claude). Checked whenever
    // the tree is torn down for this session (not shared with a peer lease),
    // INCLUDING a return to the warm pool: that return runs a forced
    // detached checkout and a hard reset, which rewrites every file under a
    // running build just as surely as a removal does. Checked before the
    // lease is touched, so a refusal changes nothing. This is the
    // interactive form: a scan that cannot be verified, and found no holder,
    // warns and proceeds. It is re-run after the confirmation and again
    // immediately before the tree is removed or reset (BUG-1986).
    // trace:BUG-1947 | ai:claude
    let tears_down_worktree = has_worktree
        && peer_lease_sharing_worktree(&leases, &target.id, &target.worktree_path).is_none();
    // BUG-1988: one scan for all three checks below, so an unverified scan
    // (e.g. a separate pid namespace) warns once per session end, not three
    // times. trace:BUG-1988 | ai:claude
    let unverified_warned = std::cell::Cell::new(false);
    let holder_check = interactive_holder_check(
        aida_core::worktree_holders::check_worktree_removal,
        &unverified_warned,
        |warning| eprintln!("{warning}"),
    );
    if tears_down_worktree {
        if let Err(in_use) = holder_check(&target.worktree_path) {
            if !include_in_use {
                anyhow::bail!(
                    "{}\n\nThe session was NOT ended; its lease and worktree are unchanged.",
                    in_use.refusal(INCLUDE_IN_USE_FLAG)
                );
            }
            eprintln!(
                "{} worktree {} — removing anyway ({INCLUDE_IN_USE_FLAG})",
                "Warning:".yellow().bold(),
                in_use.summary()
            );
        }
    }

    // STORY-73: human output to stderr, eval-friendly `unset` to stdout
    // when wrapped — same shape as session_start's `export`. Stdin/stderr
    // for the prompt still works inside `eval "$(...)"` because $(...)
    // captures stdout only. trace:STORY-73 | ai:claude
    eprintln!("About to end session {}:", target.id.yellow());
    eprintln!("  scope:    {}", target.scope);
    eprintln!("  branch:   {}", target.branch);
    eprintln!("  worktree: {}", target.worktree_path.display());
    eprintln!();
    eprintln!("Effects:");
    eprintln!(
        "  - delete lease at {}",
        lease_path(&project_root, &target.id).display()
    );
    // BUG-734: only promise a worktree removal when there is one.
    if has_worktree {
        eprintln!(
            "  - run `git worktree remove {}` (branch {} kept; merge/discard manually)",
            target.worktree_path.display(),
            target.branch
        );
        if let Some(path) = session_target_dir.as_ref().filter(|path| path.is_dir()) {
            eprintln!("  - remove session cargo target dir {}", path.display());
        }
    } else {
        eprintln!("  - no worktree attached — nothing on disk to remove");
    }

    // BUG-706: `--yes` and `--force` both skip the confirmation. On a
    // non-terminal stdin (a headless drain reclaiming a dead lease) there is
    // nobody to answer, so the old unconditional prompt read EOF and always
    // aborted — blocking unattended cleanup. Now: prompt only on a real TTY
    // without a skip flag; non-TTY without `--yes`/`--force` fails with a
    // clear, actionable error instead of a silent "Aborted."
    // BUG-776: route the gate through a pure decision fn, and make the
    // non-interactive refusal say the session was NOT ended and the lease
    // survives — the preamble above otherwise reads like the teardown ran.
    // trace:BUG-776 | ai:claude
    match session_end_confirm_action(yes, force, std::io::stdin().is_terminal()) {
        SessionEndConfirm::Skip => {}
        SessionEndConfirm::RefuseNonInteractive => {
            anyhow::bail!(session_end_confirm_refusal(
                &target.id,
                &lease_path(&project_root, &target.id)
            ));
        }
        SessionEndConfirm::Prompt => {
            use std::io::Write;
            eprint!("\nContinue? [y/N] ");
            std::io::stderr().flush()?;
            let mut ans = String::new();
            std::io::stdin().read_line(&mut ans)?;
            if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                eprintln!("Aborted — session {} is still open.", target.id);
                return Ok(());
            }
        }
    }

    // BUG-1986: the operator may have sat at the prompt for a while; a build
    // started in the tree meanwhile must still stop the teardown. Nothing
    // has been touched yet, so a refusal here leaves everything as it was.
    session_end_test_pause("after-confirm"); // trace:BUG-1986 | ai:claude
    if tears_down_worktree {
        if let Err(refusal) = recheck_holders_before_teardown(
            &target.worktree_path,
            include_in_use,
            &holder_check, // trace:BUG-1988 | ai:claude
        ) {
            anyhow::bail!(
                "{refusal}\n\nThe session was NOT ended; its lease and worktree are unchanged."
            );
        }
    }

    // BUG-67: refuse to nuke a worktree that has real uncommitted work.
    // `git status --porcelain` excludes ignored entries by default, so
    // `target/`, `.aida/cache.db`, etc. don't trip this check — those are
    // by definition disposable. Tracked-but-modified or untracked-and-
    // unignored files DO trip it and require `--force`. We run this BEFORE
    // deleting the lease, so a refusal leaves the session intact and
    // recoverable. trace:BUG-67 | ai:claude
    // BUG-1986: it also runs before the session's runtime symlinks are
    // stripped (they are only stripped once the final holder recheck has
    // passed), so the links themselves are filtered out of the entries
    // here. A refusal therefore leaves the tree fully usable.
    // trace:BUG-1986 | ai:claude
    // BUG-734: no worktree ⇒ no dirty gate — `git -C ""` would report the
    // caller's cwd, refusing on unrelated dirt. trace:BUG-734 | ai:claude
    let dirty_entries = if has_worktree {
        dirty_entries_ignoring_runtime_links(
            &target.worktree_path,
            worktree_dirty_entries(&target.worktree_path),
        )
    } else {
        Vec::new()
    };
    // BUG-652: `--return` resets the worktree (reset --hard + clean -fd), so a
    // dirty pooled tree must NOT be refused here — refusing leaves the tree
    // leased and silently breaks reuse (the next acquire creates a fresh tree).
    // Instead salvage a patch (so no work is lost) and let it through to the
    // return path. Non-pool trees / non-return still refuse without --force.
    // TASK-985: a pooled tree returns by DEFAULT now — `--remove` opts out.
    // BUG-1986: the salvage itself runs later, after the final recheck and
    // the link strip, so a late refusal writes no patch and the patch never
    // captures the runtime links. trace:BUG-1986 | ai:claude
    // trace:BUG-652 trace:STORY-714 trace:TASK-985 | ai:claude
    let returning_pool_tree = !remove
        && has_worktree
        && aida_core::worktree_pool::is_pool_worktree(&project_root, &target.worktree_path);
    let salvage_before_return =
        match dirty_gate_outcome(!dirty_entries.is_empty(), force, returning_pool_tree) {
            DirtyGateOutcome::Proceed => false,
            DirtyGateOutcome::Salvage => true,
            DirtyGateOutcome::Refuse => {
                let mut msg = format!(
                    "worktree {} has uncommitted changes:\n",
                    target.worktree_path.display()
                );
                for line in &dirty_entries {
                    msg.push_str(&format!("  {}\n", line));
                }
                msg.push_str(
                "\nPass `--force` to `aida session end` to discard these and remove the worktree.\n\
                 (Gitignored files like target/ and .aida/cache.db never require --force.)",
            );
                anyhow::bail!(msg);
            }
        };

    // BUG-61 under --include-in-use: terminate live claudes in the tree only
    // now, once the confirmation and the dirty gate have both passed.
    // trace:BUG-1986 | ai:claude
    if !leaked.is_empty() {
        eprintln!(
            "{} {} live claude process(es) inside worktree — sending SIGTERM, then SIGKILL after 5s",
            "→".dimmed(),
            leaked.len()
        );
        let _ = terminate_pids_with_grace(&leaked.iter().map(|p| p.pid).collect::<Vec<_>>(), 5);
    }

    // BUG-1986: the last holder check before the tree is removed or reset.
    // It runs here, before the lease is deleted and before the chdir to the
    // project root below: after that chdir the caller is outside the tree,
    // so the agent's own helpers (an MCP server with its cwd in the tree)
    // would lose their self-chain exemption. Nothing has been written yet
    // (no punt-ledger record, role activity, symlink strip, salvage patch or
    // lease delete), so a refusal leaves the lease, the pool entry and the
    // worktree, runtime links included, exactly as they were.
    // trace:BUG-1986 | ai:claude
    session_end_test_pause("before-teardown");
    if tears_down_worktree {
        if let Err(refusal) = recheck_holders_before_teardown(
            &target.worktree_path,
            include_in_use,
            &holder_check, // trace:BUG-1988 | ai:claude
        ) {
            anyhow::bail!(
                "{refusal}\n\nThe session was NOT ended; its lease is kept and its worktree \
                 was not removed or reset."
            );
        }
    }

    // Snapshot the lease file's authoritative on-disk location BEFORE we
    // touch the worktree's symlinks. When session_end runs from inside the
    // worktree (the natural flow), `project_root` IS the worktree path —
    // and `<worktree>/.aida/sessions/<id>.toml` traverses the symlink
    // installed at session_start to reach the parent's real file. Once we
    // strip that symlink (or git removes the whole worktree), the
    // worktree-relative path stops resolving. canonicalize() follows the
    // symlink now and gives us a stable target for the unlink later.
    // trace:BUG-56 | ai:claude
    let lease_file_via_symlink = lease_path(&project_root, &target.id);
    let canonical_lease = lease_file_via_symlink.canonicalize().ok();

    // Clean the symlinks we created at session start before git tries to
    // remove the worktree — they count as untracked files and `git worktree
    // remove` would otherwise refuse without --force.
    //   - .aida-store/ is a whole-directory symlink (top-level)
    //   - .aida/ itself is a real dir (with tracked content) — leave it
    //     alone, but strip the runtime symlinks inside it that
    //     session_start created. trace:BUG-52 | ai:claude
    // BUG-734: no worktree ⇒ no session symlinks to strip (an empty path
    // would resolve the checks against the caller's cwd). trace:BUG-734 | ai:claude
    if has_worktree {
        let store_link = target.worktree_path.join(".aida-store");
        if store_link.is_symlink() {
            let _ = std::fs::remove_file(&store_link);
        }
        unlink_worktree_aida_runtime(&target.worktree_path.join(".aida"));
    }

    // BUG-652 salvage for a dirty pooled tree, now that the links are gone.
    // BUG-1988: a failed salvage stops the end HERE, before anything that
    // ends the session (punt record, role activity merge, lease delete):
    // the return would otherwise fail its own salvage and keep the tree,
    // leaving a pool entry leased by a session that no longer exists. The
    // runtime links are put back so the tree stays usable for the retry.
    // trace:BUG-1986 trace:BUG-1988 | ai:claude
    if salvage_before_return {
        match salvage_worktree_patch(&project_root, "pool-return", None, &target.worktree_path) {
            Ok(Some(p)) => eprintln!(
                "{} salvaged uncommitted changes to {} before returning the pooled worktree (it will be reset)",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                p.display().to_string().dimmed()
            ),
            Ok(None) => {}
            Err(e) => {
                // trace:BUG-1988 | ai:claude
                #[cfg(unix)]
                let relink = crate::worktree_gc::link_worktree_runtime_state(
                    &project_root,
                    &target.worktree_path,
                )
                .err()
                .map(|err| format!("\nThe worktree's runtime links could not be restored: {err}"))
                .unwrap_or_default();
                #[cfg(not(unix))]
                let relink = String::new();
                anyhow::bail!(
                    "{}",
                    session_end_salvage_failure_message(
                        &target.id,
                        &target.worktree_path,
                        &project_root,
                        &e,
                        &relink
                    )
                );
            }
        }
    }

    if let Some(work) = &session_end_unshipped_work {
        record_session_end_unshipped_work(&project_root, work);
    }

    // STORY-56: flatten the session's activity log into each
    // participating role's project-level activity stream BEFORE we delete
    // the lease — once the lease is gone the activity file is orphaned.
    // For each role, take the newest entry per spec_id from the session
    // log, merge into the project role's `activity` (newest first, dedupe
    // by spec_id, truncate to ACTIVITY_MAX) so post-session views like
    // `aida role show` still surface what was worked on under the closed
    // session. trace:STORY-56 | ai:claude
    aggregate_session_activity_into_roles(&project_root, &target.id);
    let activity_file = session_activity_path(&project_root, &target.id);
    let canonical_activity = activity_file.canonicalize().ok();
    let activity_target: std::path::PathBuf = canonical_activity
        .clone()
        .unwrap_or_else(|| activity_file.clone());

    // Delete the lease file BEFORE removing the worktree. The canonical
    // path resolved above points at the parent's sessions dir, so the
    // unlink succeeds regardless of whether the worktree's symlink chain
    // is still intact. Reporting is honest: success printed only on
    // actual success; missing file is a quiet no-op (already-released
    // lease from a previous partial run is fine); other errors warn so
    // the user can clean up manually instead of trusting a stale check mark.
    // trace:BUG-56 | ai:claude
    let lease_target: &std::path::Path = canonical_lease
        .as_deref()
        .unwrap_or(&lease_file_via_symlink);
    match std::fs::remove_file(lease_target) {
        Ok(_) => eprintln!(
            "{} lease deleted",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Already gone — keep quiet.
        }
        Err(e) => {
            eprintln!(
                "{} could not delete lease at {}: {} — remove manually",
                "Warning:".yellow().bold(),
                lease_target.display(),
                e
            );
        }
    }

    // BUG-694: reap the sibling MCP work-claim for this scope. An MCP client
    // that claimed the spec via `claim_task` drops `mcp-claim.<spec>.toml`
    // alongside the session lease; without this, `session end` released the
    // session lease but left the mcp-claim behind, so the spec kept surfacing
    // as stale running work long after the worktree was gone (TASK-702's claim
    // lingered 24 days). Best-effort + NotFound-quiet, mirroring the lease
    // unlink above. trace:BUG-694 | ai:claude
    let mcp_claim = mcp::mcp_claim_path(&leases_dir(&project_root), &target.scope);
    match std::fs::remove_file(&mcp_claim) {
        Ok(_) => eprintln!(
            "{} mcp-claim reaped ({})",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            target.scope
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No MCP claim for this scope — the common case. Quiet.
        }
        Err(e) => {
            eprintln!(
                "{} could not reap mcp-claim {}: {} — remove manually",
                "Warning:".yellow().bold(),
                mcp_claim.display(),
                e
            );
        }
    }

    // BUG-1988: the mcp-claim reap deliberately stays BEFORE the remove/return
    // (it is a local unlink, so it does not widen the recheck window): the
    // claim file sits in the sessions dir and parses as a lease, so leaving
    // it in place would make the BUG-483 peer check below treat this
    // session's own MCP claim as a peer sharing the tree and skip the
    // removal. trace:BUG-1988 | ai:claude

    // TASK-68: detect whether the user's shell was sitting inside the
    // worktree we're about to nuke. We capture cwd BEFORE we chdir away
    // for the git ops, so we can later emit a `cd <parent>` for the
    // shell wrapper (Layer 2 of the fix). Canonicalize both sides so
    // symlink chains don't mask the match. trace:TASK-68 | ai:claude
    let starting_cwd = std::env::current_dir().ok();
    let canonical_worktree = target
        .worktree_path
        .canonicalize()
        .unwrap_or_else(|_| target.worktree_path.clone());
    // BUG-734: `starts_with` on an empty canonical path matches every cwd —
    // a worktree-less lease must never trigger the cd-out. trace:BUG-734 | ai:claude
    let cwd_was_inside_worktree = has_worktree
        && starting_cwd
            .as_ref()
            .and_then(|c| c.canonicalize().ok())
            .map(|c| c.starts_with(&canonical_worktree))
            .unwrap_or(false);

    // TASK-68 Layer 1: chdir to the parent project before running git.
    // `git worktree remove` refuses outright when the calling process's
    // cwd is inside the worktree being removed. Switching the Rust
    // process's cwd here doesn't affect the parent shell — that's
    // handled separately by Layer 2's eval'd `cd`. trace:TASK-68
    let _ = std::env::set_current_dir(&project_root);

    // BUG-483: before force-removing the worktree, check whether another
    // registered lease shares this same `worktree_path`. `aida agent new`
    // can land two sessions in one worktree (BUG-416); force-removing it
    // out from under the peer strands them (TASK-0396 cross-worktree
    // fingerprint hazard + BUG-108 `(deleted)` cwd). The target lease was
    // already deleted above, so the live lease set is peers-only, but we
    // still filter by id for robustness. Be conservative: any peer ⇒ skip
    // the removal entirely (leaving a worktree is recoverable; deleting a
    // shared one is not). We still ended THIS lease/registry entry above.
    // trace:BUG-483 | ai:claude
    let remaining_leases = list_leases(&project_root);
    use std::io::Write;
    // BUG-734: two worktree-less leases share the same empty path — don't let
    // them count as worktree peers. trace:BUG-734 | ai:claude
    let shared_peer = if has_worktree {
        peer_lease_sharing_worktree(&remaining_leases, &target.id, &target.worktree_path)
    } else {
        None
    };
    // STORY-714: when `--return` is set and this is a registered warm-pool
    // worktree, hand it back to the pool (reset to a clean detached base, mark
    // idle) instead of deleting it. The directory persists so the next acquire
    // reuses it warm. trace:STORY-714 | ai:claude
    let mut returned_to_pool = false;
    if !has_worktree {
        // BUG-734: worktree-less lease — the record is gone; nothing on disk
        // to remove or return. trace:BUG-734 | ai:claude
        eprintln!(
            "{} no worktree attached to this session — lease record removed; nothing on disk to clean up",
            "→".dimmed()
        );
    } else if let Some(peer) = shared_peer {
        // Skip the removal; this lease/registry entry is already gone, so the
        // session has ended — we just leave the shared worktree standing.
        eprintln!(
            "{} worktree {} is shared by lease {} ({}) — ending this session but leaving the shared worktree in place",
            "→".dimmed(),
            target.worktree_path.display(),
            peer.id.yellow(),
            peer.scope
        );
    } else if !remove
        && aida_core::worktree_pool::is_pool_worktree(&project_root, &target.worktree_path)
    {
        // TASK-985: returning a pooled tree is the default; `--remove` opts out.
        match aida_core::worktree_pool::return_to_pool(&project_root, &target.worktree_path) {
            Ok(_) => {
                returned_to_pool = true;
                eprintln!(
                    "{} worktree returned to the warm-pool (reset to a clean base, marked idle — directory + build cache kept)",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
            }
            Err(e) => eprintln!(
                "{} could not return worktree to the pool: {} — left in place",
                "Warning:".yellow().bold(),
                e
            ),
        }
    } else {
        // BUG-67: always pass `--force`. We've already gated on the dirty
        // check above, so by this point the only non-clean state is
        // gitignored (target/, .aida/cache.db, etc.) — which git still
        // refuses to remove without `--force`. trace:BUG-67 | ai:claude
        let res = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args([
                "worktree",
                "remove",
                "--force",
                "--", // trace:BUG-1622 | ai:claude
                target.worktree_path.to_str().unwrap_or_default(),
            ])
            .output();
        match res {
            Ok(o) if o.status.success() => {
                let _ = std::io::stderr().write_all(&o.stdout);
                let _ = std::io::stderr().write_all(&o.stderr);
                eprintln!(
                    "{} worktree removed",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
                if let Some(path) = &session_target_dir {
                    remove_session_cargo_target_dir(path, &target.worktree_path);
                }
            }
            Ok(o) => {
                let _ = std::io::stderr().write_all(&o.stdout);
                let _ = std::io::stderr().write_all(&o.stderr);
                eprintln!(
                    "{} `git worktree remove` failed; you may need to run it manually with --force",
                    "Warning:".yellow().bold()
                );
            }
            Err(_) => {
                eprintln!(
                    "{} `git worktree remove` failed to spawn; you may need to run it manually",
                    "Warning:".yellow().bold()
                );
            }
        }
    }
    // BUG-1988: the steps below used to run between the final holder recheck
    // and the remove/return above. They are moved here so that window holds
    // only local filesystem steps and the salvage diff: `release_claim` in
    // particular pulls, commits and pushes the shared store, which can take
    // long enough for a new process to start in the tree. They run whatever
    // the remove/return outcome was, exactly as before the move: a failed
    // `git worktree remove` or pool return only warns (no early return), and
    // the lease is already deleted by then, so the session HAS ended and its
    // store claim, needs-human marker, manifest and activity log must go
    // with it rather than strand the scope.
    // trace:BUG-1988 | ai:claude
    // STORY-637: release the cross-clone lease claim on the shared store so a
    // peer clone can take this scope. Best-effort by design — staleness (pid /
    // TTL) reclaims a never-released claim, so a failed release never
    // deadlocks. trace:STORY-637 | ai:claude
    coordination::release_claim(
        &project_root.join(".aida-store"),
        &target.scope,
        &project_root,
    );

    // STORY-564: drop this session's `--zen` needs-human marker, if any, so a
    // future session that reuses the (time-ordered, effectively-unique) id
    // never inherits a stale "pause at finish" signal. Best-effort, quiet.
    // trace:STORY-564 | ai:claude
    zen::clear_needs_human_marker(&project_root, &target.id);

    // BUG-80: drop the planned-cluster manifest for this session, if any.
    // Manifests are runtime per-session state — pinning them around past
    // a closed lease leaks files into `.aida/sessions/`. Quiet on missing.
    // trace:BUG-80 | ai:claude
    let manifest_target = session_manifest::manifest_path(&project_root, &target.id);
    if manifest_target.exists() {
        match std::fs::remove_file(&manifest_target) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!(
                    "{} could not delete session manifest at {}: {}",
                    "Warning:".yellow().bold(),
                    manifest_target.display(),
                    e
                );
            }
        }
    }

    // STORY-56: drop the session activity log. Quiet on missing — short
    // sessions that never recorded any activity won't have one. We've
    // already aggregated entries into the project-level role(s) above.
    // trace:STORY-56 | ai:claude
    if activity_target.exists() {
        match std::fs::remove_file(&activity_target) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!(
                    "{} could not delete session activity log at {}: {}",
                    "Warning:".yellow().bold(),
                    activity_target.display(),
                    e
                );
            }
        }
    }

    // BUG-734: the branch-retained hint only makes sense when a worktree
    // (with its checked-out branch) was actually removed. trace:BUG-734 | ai:claude
    if has_worktree {
        eprintln!(
            "  branch {} retained — merge or `git branch -D {}` when ready",
            target.branch.cyan(),
            target.branch
        );
    }

    // BUG-614: lossless prune of administrative worktree entries whose
    // directories are already gone. `git worktree prune` ONLY removes the
    // bookkeeping for worktrees whose on-disk dir no longer exists — it never
    // touches a live worktree — so it is always safe and cheap. Running it on
    // session-end keeps `.git/worktrees/` from accumulating stale entries
    // (which scale the per-worktree git spawns `aida status` makes). Quiet,
    // best-effort: a failure here never fails the session teardown.
    // trace:BUG-614 | ai:claude
    lossless_prune_worktrees(&project_root);

    // TASK-70: handle the Claude Code project dir orphaned by the
    // worktree removal. Claude Code stores per-session jsonls under
    // `~/.claude/projects/<encoded-cwd>/`; once the cwd vanishes, the
    // jsonls are stranded — `claude --resume <id>` may fail or behave
    // unexpectedly. Default: warn + point at `session prune --orphans`.
    // With `--purge-cc`: remove the dir atomically here.
    // BUG-483: skip entirely when we kept a shared worktree — the dir is
    // still live (the peer is using it), so it's neither orphaned nor safe
    // to purge. STORY-714: also skip when we returned the tree to the pool —
    // the directory persists, so its cwd is not orphaned.
    // BUG-734: no worktree was ever created, so no Claude Code project dir
    // can have been orphaned by its removal. trace:BUG-734 | ai:claude
    // trace:TASK-70 BUG-483 STORY-714 | ai:claude
    if has_worktree && shared_peer.is_none() && !returned_to_pool {
        if let Ok(cc_dir) = session::claude_project_dir(&canonical_worktree) {
            if cc_dir.is_dir() {
                if purge_cc {
                    match std::fs::remove_dir_all(&cc_dir) {
                        Ok(_) => eprintln!(
                            "{} purged Claude Code project dir {}",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            cc_dir.display().to_string().dimmed()
                        ),
                        Err(e) => eprintln!(
                            "{} could not purge {}: {} — remove manually if you don't want it",
                            "Warning:".yellow().bold(),
                            cc_dir.display(),
                            e
                        ),
                    }
                } else {
                    eprintln!(
                        "{} Claude Code project dir {} is now orphaned (cwd gone). \
                         Run `aida session prune --orphans` to clean up, or pass \
                         `--purge-cc` on `aida session end` next time.",
                        crate::glyph(crate::glyphs::Glyph::Info).dimmed(),
                        cc_dir.display().to_string().dimmed()
                    );
                }
            }
        }
    }

    // STORY-66: when the just-ended session's branch has an open PR, file a
    // Story-typed review item routed to the `reviewer` role with `implements`
    // relations to every spec referenced in the PR's commit messages. Best
    // effort: any failure here logs a warning and is otherwise silent — we
    // never fail `session_end` because of a queue side-effect.
    //
    // BUG-72: always print the outcome (filed / skipped + reason), so the
    // user sees whether the hook fired. The silent-skip case was painful in
    // PR-7 wrap-up: STORY-66 silently no-op'd and the reviewer queue stayed
    // empty with no explanation. trace:STORY-66 BUG-72 | ai:claude
    //
    // STORY-90: this is the backup trigger now — primary fires from
    // /aida-pr right after `gh pr create`. The idempotency check inside
    // `try_auto_queue_pr_review` (Review PR-N story already exists) means
    // both firing the same PR is safe; the second one renders as
    // `AlreadyExists`. trace:STORY-90 | ai:claude
    // BUG-107: the auto-queue side-effect shells out to `gh`, `git`, and
    // `aida`, all of which need a working directory that still exists.
    // When `aida session end` is run from inside the worktree it just
    // removed, `project_root` (resolved from cwd at entry) now points at
    // a deleted directory and every spawn fails with a misleading
    // ENOENT blamed on the gh binary. Prefer the lease's recorded parent
    // project root — the main worktree, which `session end` never
    // removes. trace:BUG-107 | ai:claude
    let outcome = match auto_queue_working_dir(target.parent_project_root.as_deref(), &project_root)
    {
        Some(root) => try_auto_queue_pr_review(
            &root,
            &target.branch,
            &target.id,
            AutoQueueOrigin::SessionEnd,
        ),
        None => AutoQueueOutcome::skipped_needs_attention(format!(
            "auto-queue: no surviving project directory to query `gh` from \
                 (worktree removed, parent project root unrecorded) — reviewer \
                 story for branch `{}` not filed; run `aida pr auto-queue-review \
                 --branch {}` from the main repo",
            target.branch, target.branch
        )),
    };
    render_auto_queue_outcome(&outcome);

    // STORY-106: workflow hint when a review story is now queued for this
    // session's PR. Fires for both Filed (just minted) and AlreadyExists
    // (idempotent re-fire) — either way, the reviewer story is in the
    // queue and the user's next action is `aida queue work`.
    // trace:STORY-106 | ai:claude
    if matches!(
        outcome.status,
        AutoQueueStatus::Filed | AutoQueueStatus::AlreadyExists
    ) {
        // BUG-776: never render the `PR #0 next: ...` chain. A synthetic id-0
        // change ref names no reviewable PR, so every step of the hint
        // (`aida queue work PR-0`, merge PR-0) points at nothing.
        // trace:BUG-776 | ai:claude
        if let Some(pr_n) = outcome.pr_number.filter(|n| *n > 0) {
            workflow_hints::after_session_end_with_pr(
                Some(&project_root),
                pr_n,
                &outcome.covered_specs,
            );
        }
    }

    if let Some(store_path) = detect_distributed_store_from(&project_root) {
        // STORY-493: durably digest this session's local mailbox traffic into
        // the git-canonical orphan store before the store is pushed, so a
        // session's inter-agent messages are replayable/shareable once it
        // closes. Best-effort + non-fatal — a digest failure must never abort
        // session cleanup. Runs before the push so the digest commit rides the
        // same push. trace:STORY-493 | ai:claude
        maybe_digest_mailbox_best_effort(&store_path, "session-end");
        maybe_auto_push_store(&store_path, StoreAutoPushMode::SessionEnd, "session-end");
    }

    // STORY-73: emit `unset AIDA_SESSION_ID` to stdout when wrapped via the
    // shell helper's `eval "$(...)"`, so the calling shell's env var
    // doesn't go stale after the lease it pointed at is gone.
    // trace:STORY-73 | ai:claude
    //
    // TASK-68 Layer 2: if the user's shell was sitting inside the
    // worktree we just removed, their cwd is now a dangling (deleted)
    // inode — every path-relative command would fail until they
    // manually cd out. Emit a `cd <parent>` to stdout so the shell
    // wrapper's `eval` lands them in the parent project naturally.
    // No-op for users running `aida session end` from outside the
    // worktree (and for direct (non-wrapped) invocation, since stdout
    // is a TTY then). trace:TASK-68 | ai:claude
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        // trace:TASK-1171 | ai:claude — stdout inside this scope is shell code
        // (the interleaved `eprintln!` below is stderr, so it stays prose).
        let _eval = crate::shell_eval::EvalBlock::open();
        println!("unset AIDA_SESSION_ID");
        // BUG-483: only emit a `cd` out when we actually removed the worktree.
        // When a peer lease shares it we left it standing, so the shell's cwd
        // is still valid — yanking the user out of it would be a surprise.
        // trace:BUG-483 | ai:claude
        if cwd_was_inside_worktree && shared_peer.is_none() {
            let escaped = project_root.display().to_string().replace('\'', "'\\''");
            println!("cd '{}'", escaped);
            eprintln!(
                "  {} cd'd back to parent project {}",
                "↩".dimmed(),
                project_root.display().to_string().cyan()
            );
        }
    } else if cwd_was_inside_worktree && shared_peer.is_none() {
        // BUG-59: direct (non-wrapped) invocation can't `eval` a `cd` into
        // the parent shell, so the caller's cwd is left dangling on the
        // now-deleted worktree inode. Any subsequent process spawn from that
        // shell — notably Claude Code's Stop hook firing `/bin/sh` — then
        // fails with a noisy `ENOENT ... posix_spawn '/bin/sh'`. We can't
        // mutate the parent shell from here, so surface a clear `cd` hint
        // and let the caller move out before their next command.
        // trace:BUG-59 | ai:claude
        for line in session_end_stale_cwd_hint_lines(&project_root) {
            eprintln!("{}", line);
        }
    }
    Ok(())
}

/// Test-only pause inside `session end`, so an integration test can start a
/// process in the worktree between two steps and prove the in-flow holder
/// rechecks catch it. Active only in debug builds, and only when
/// `AIDA_TEST_SESSION_END_PAUSE_AT` names `stage`: it then creates `paused`
/// in `AIDA_TEST_SESSION_END_PAUSE_DIR` and waits (up to 60s) for `resume`.
// trace:BUG-1986 | ai:claude
#[cfg(debug_assertions)]
fn session_end_test_pause(stage: &str) {
    if std::env::var("AIDA_TEST_SESSION_END_PAUSE_AT")
        .ok()
        .as_deref()
        != Some(stage)
    {
        return;
    }
    let Some(dir) = std::env::var_os("AIDA_TEST_SESSION_END_PAUSE_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::write(dir.join("paused"), stage);
    let resume = dir.join("resume");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !resume.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

// trace:BUG-1986 | ai:claude
#[cfg(not(debug_assertions))]
fn session_end_test_pause(_stage: &str) {}

/// Drop the session's own runtime symlinks (`.aida-store` and the links
/// inside `.aida/`) from `git status --porcelain` entries. The dirty gate
/// runs while those links are still in place, and they are not work: the
/// teardown strips them itself. Only an untracked entry that IS a symlink at
/// one of those exact paths is dropped; anything else, including a real
/// file or directory at the same path, still counts.
// trace:BUG-1986 | ai:claude
pub(crate) fn dirty_entries_ignoring_runtime_links(
    worktree: &std::path::Path,
    entries: Vec<String>,
) -> Vec<String> {
    let is_runtime_link = |entry: &str| {
        let Some(path) = entry.strip_prefix("?? ") else {
            return false;
        };
        let path = path.trim_end_matches('/');
        let known = path == ".aida-store"
            || path.strip_prefix(".aida/").is_some_and(|rest| {
                crate::worktree_gc::WORKTREE_AIDA_RUNTIME_LINKS.contains(&rest)
            });
        known && worktree.join(path).is_symlink()
    };
    // BUG-1988: with `.aida/` not gitignored and nothing tracked under it,
    // plain `git status` collapses it to one `?? .aida/` entry, which the
    // per-link filter cannot judge. Expand an untracked `.aida/` into its
    // files (`-uall`; git lists a symlink as a file and never descends it),
    // so it only counts when something under it is not a runtime link.
    // trace:BUG-1988 | ai:claude
    entries
        .into_iter()
        .flat_map(|entry| {
            // Porcelain v1 (`worktree_dirty_entries`). trace:BUG-1988 | ai:claude
            if entry == "?? .aida/" {
                untracked_entries_under(worktree, ".aida/").unwrap_or_else(|| vec![entry])
            } else {
                vec![entry]
            }
        })
        .filter(|entry| !is_runtime_link(entry))
        .collect()
}

/// Every untracked file under `dir` in `worktree`, one porcelain entry each.
/// `None` when git cannot answer, so the caller keeps the collapsed entry.
// trace:BUG-1988 | ai:claude
fn untracked_entries_under(worktree: &std::path::Path, dir: &str) -> Option<Vec<String>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["status", "--porcelain", "--untracked-files=all", "--", dir])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// BUG-1988: the interactive holder scan for one `session end` run. `scan`
/// is the strict scan; an unverified result with no holder is downgraded to
/// a warning, which `emit` receives at most once per `warned` cell however
/// many times the returned check runs.
// trace:BUG-1988 | ai:claude
pub(crate) fn interactive_holder_check<'a>(
    scan: impl Fn(&std::path::Path) -> Result<(), aida_core::worktree_holders::WorktreeInUse> + 'a,
    warned: &'a std::cell::Cell<bool>,
    emit: impl Fn(&str) + 'a,
) -> impl Fn(&std::path::Path) -> Result<(), aida_core::worktree_holders::WorktreeInUse> + 'a {
    move |worktree| {
        let warning = aida_core::worktree_holders::downgrade_unverified(scan(worktree))?;
        if let Some(warning) = warning {
            if !warned.replace(true) {
                emit(&warning);
            }
        }
        Ok(())
    }
}

/// BUG-1988: the refusal when the pre-return salvage of a dirty pooled tree
/// fails. Nothing that ends the session has run, so it says the session was
/// NOT ended and names the command that finishes the end once the cause is
/// fixed.
// trace:BUG-1988 | ai:claude
pub(crate) fn session_end_salvage_failure_message(
    session_id: &str,
    worktree: &std::path::Path,
    project_root: &std::path::Path,
    error: &dyn std::fmt::Display,
    extra: &str,
) -> String {
    format!(
        "could not salvage the uncommitted changes in pooled worktree {} before returning it \
         to the pool: {error}\n\n\
         The session was NOT ended; its lease is kept and its worktree was not reset.{extra}\n\
         Fix the cause (salvage patches go under {}), or commit or stash the changes in the \
         worktree, then re-run `aida session end {session_id}`.",
        worktree.display(),
        project_root.join(".aida").join("salvage").display(),
    )
}

/// The session-end override flag for the live-process refusal.
// trace:BUG-1986 | ai:claude
pub(crate) const INCLUDE_IN_USE_FLAG: &str = "--include-in-use";

/// Re-run the live-process check right before a destructive teardown step.
/// `check` is the scan (injectable for tests). `Err` carries the full
/// refusal text naming each holder and the override flag. With
/// `include_in_use` the scan is skipped: the operator already chose to
/// remove the tree regardless.
// trace:BUG-1986 | ai:claude
pub(crate) fn recheck_holders_before_teardown(
    worktree: &std::path::Path,
    include_in_use: bool,
    check: impl Fn(&std::path::Path) -> Result<(), aida_core::worktree_holders::WorktreeInUse>,
) -> std::result::Result<(), String> {
    if include_in_use || worktree.as_os_str().is_empty() {
        return Ok(());
    }
    check(worktree).map_err(|in_use| in_use.refusal(INCLUDE_IN_USE_FLAG))
}

/// BUG-59: ready-to-print hint lines telling the caller to leave the deleted
/// worktree cwd before running anything else (so a follow-on `/bin/sh` spawn —
/// e.g. Claude Code's Stop hook — doesn't ENOENT on the dangling inode). The
/// emitted `cd` targets the parent project root that survived the removal.
pub(crate) fn session_end_stale_cwd_hint_lines(project_root: &std::path::Path) -> Vec<String> {
    let escaped = project_root.display().to_string().replace('\'', "'\\''");
    vec![
        format!(
            "  {} your shell is still inside the removed worktree — run this before your next command:",
            "↩".dimmed()
        ),
        format!("      cd '{}'", escaped),
    ]
}

#[cfg(test)]
#[path = "tests/bug_1986_teardown_recheck_tests.rs"]
mod bug_1986_teardown_recheck_tests;
