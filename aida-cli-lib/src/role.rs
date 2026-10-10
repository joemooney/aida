//! Role State and Activities (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RoleState {
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) purpose: Option<String>,
    pub(crate) created_at: chrono::DateTime<chrono::Utc>,
    pub(crate) last_active_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) working_directory: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) notes: Option<String>,

    /// True if this role file lives in ~/.aida/roles/ rather than per-project.
    /// Persisted so `aida role list` can mark global roles distinctly without
    /// re-checking the filesystem location.
    #[serde(default)]
    pub(crate) global: bool,

    /// Last N requirements touched while this role was active. Newest first.
    /// Bounded at ACTIVITY_MAX entries; older entries fall off the end.
    #[serde(default)]
    pub(crate) activity: Vec<RoleActivity>,

    /// Phase 3 scope filter: tags AND'd into the default filter for
    /// `aida list` and `aida queue list/next` while this role is active.
    /// Empty = no tag scope. Override on a single command with explicit
    /// --tags or --no-scope.
    /// trace:TASK-1-021 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) scope_tags: Vec<String>,

    /// Phase 3 scope filter: status auto-applied while this role is active.
    /// None = no status scope. Override on a single command with explicit
    /// --status or --no-scope.
    /// trace:TASK-1-021 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scope_status: Option<String>,

    /// Phase 3 system-prompt addendum: free-form text injected into Claude
    /// Code's context at SessionStart (via the aida-role-context.sh hook)
    /// when this role is active. Lets you keep role-specific instructions
    /// to the model alongside the role itself.
    /// trace:TASK-1-022 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) system_prompt: Option<String>,

    /// Initial message `aida agent new` injects when launched in this role
    /// WITHOUT `--spec`. Overrides the embedded per-role orientation; the
    /// launch-context read commands are always prepended. Placeholders:
    /// `{role}`, `{agent}`.
    // trace:STORY-1471 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) launch_prompt: Option<String>,

    /// Initial message `aida agent new` injects when launched in this role
    /// WITH `--spec`. Same contract as `launch_prompt`, plus `{spec}`.
    // trace:STORY-1471 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) launch_prompt_spec: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RoleActivity {
    /// Requirement spec_id (or agreed_id) touched
    pub(crate) spec_id: String,
    /// What the user did: "show", "edit", "add", "comment"
    pub(crate) action: String,
    pub(crate) at: chrono::DateTime<chrono::Utc>,
}

/// Per-project role storage: <project>/.aida/roles/
pub(crate) fn project_roles_dir(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida/roles")
}

/// Global role storage: ~/.aida/roles/ — for personas you carry across
/// projects (e.g., "triage", "code-review").
pub(crate) fn global_roles_dir() -> Option<std::path::PathBuf> {
    // trace:BUG-1021 | ai:claude
    // Test hook: `dirs::home_dir()` ignores `$HOME` on Windows (it reads the
    // profile folder), so tests that redirect the home dir set AIDA_TEST_HOME
    // — the same override `glyphs.rs` / `user_alias.rs` honor.
    #[cfg(test)]
    if let Some(home) = std::env::var_os("AIDA_TEST_HOME") {
        let home = std::path::PathBuf::from(home);
        // trace:BUG-1642 | ai:claude — refuse an override naming the real home.
        crate::test_home::assert_hermetic(&home);
        return Some(home.join(".aida/roles"));
    }
    // BUG-1642: under cfg(test) this panics instead of returning the real
    // home, so the role-activity recorder can never append to the operator's
    // `~/.aida/roles/<role>.toml`.
    crate::home_dir().map(|h| h.join(".aida/roles"))
}

pub(crate) fn project_role_file(project_root: &std::path::Path, name: &str) -> std::path::PathBuf {
    project_roles_dir(project_root).join(format!("{}.toml", name))
}

pub(crate) fn global_role_file(name: &str) -> Option<std::path::PathBuf> {
    global_roles_dir().map(|d| d.join(format!("{}.toml", name)))
}

/// Parse a role file leniently. A clean file behaves exactly like a strict
/// `toml::from_str`. When the strict parse fails, salvage the header table
/// and every well-formed `[[activity]]` entry, dropping — and reporting,
/// via the returned warnings — any entry that won't parse. This keeps one
/// corrupted activity append (BUG-228: a torn concurrent write left a
/// stray `"`) from taking down the whole role. Returns `Err` only when the
/// *header* itself is unparseable, since nothing useful survives that.
/// trace:BUG-228 | ai:claude
pub(crate) fn parse_role_lenient(content: &str) -> Result<(RoleState, Vec<String>)> {
    // Fast path: a clean file parses strictly, no salvage needed.
    let strict_err = match toml::from_str::<RoleState>(content) {
        Ok(state) => return Ok((state, Vec::new())),
        Err(e) => e,
    };

    let lines: Vec<&str> = content.lines().collect();
    let Some(split) = lines.iter().position(|l| l.trim() == "[[activity]]") else {
        // No activity region to salvage — the corruption is in the header.
        return Err(anyhow::Error::new(strict_err).context("role file header is unparseable"));
    };

    let header = lines[..split].join("\n");
    let mut state: RoleState = toml::from_str(&header)
        .map_err(|e| anyhow::Error::new(e).context("role file header is unparseable"))?;
    state.activity.clear();

    // Group the activity region into blocks: each starts at an
    // `[[activity]]` line and runs up to just before the next one.
    let mut blocks: Vec<(usize, Vec<&str>)> = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(split) {
        if line.trim() == "[[activity]]" {
            blocks.push((i, Vec::new()));
        } else if let Some((_, body)) = blocks.last_mut() {
            body.push(*line);
        }
    }

    let mut warnings: Vec<String> = Vec::new();
    for (line_no, body_lines) in blocks {
        // Activity entries only ever hold simple single-line string
        // fields, so dropping any non-blank line that isn't a `key = …`
        // assignment is safe — and rescues the entry from trailing junk.
        let mut body = String::new();
        for ln in &body_lines {
            let t = ln.trim();
            if t.is_empty() || is_toml_kv_line(t) {
                body.push_str(ln);
                body.push('\n');
            } else {
                warnings.push(format!(
                    "activity entry near line {}: dropped malformed line `{}`",
                    line_no + 1,
                    t
                ));
            }
        }
        match toml::from_str::<RoleActivity>(&body) {
            Ok(activity) => state.activity.push(activity),
            Err(e) => warnings.push(format!(
                "activity entry near line {} skipped — unparseable ({})",
                line_no + 1,
                e
            )),
        }
    }
    state.activity.truncate(ACTIVITY_MAX);
    Ok((state, warnings))
}

/// Load a role by name. Looks in the project first, then the global dir.
/// Returns the state, the path it was loaded from (for save-back), and any
/// salvage warnings from `parse_role_lenient` (empty for a clean file).
/// trace:BUG-228 | ai:claude
pub(crate) fn load_role_with_warnings(
    project_root: &std::path::Path,
    name: &str,
) -> Result<(RoleState, std::path::PathBuf, Vec<String>)> {
    // TASK-586: resolve under the canonical name, but also accept the legacy
    // `dialog.toml` file as the advisor role on machines not yet migrated.
    // The loaded state's name is canonicalized so callers always see
    // `advisor`, never `dialog`.
    let canonical = canonical_role_name(name);
    let mut candidates = vec![canonical.clone()];
    if canonical == "advisor" {
        candidates.push("dialog".to_string());
    }
    for cand in &candidates {
        for path in [
            Some(project_role_file(project_root, cand)),
            global_role_file(cand),
        ]
        .into_iter()
        .flatten()
        {
            if path.exists() {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read role file {}", path.display()))?;
                let (mut state, warnings) = parse_role_lenient(&content)
                    .with_context(|| format!("Failed to parse role file {}", path.display()))?;
                state.name = canonical_role_name(&state.name);
                return Ok((state, path, warnings));
            }
        }
    }
    anyhow::bail!(
        "No such role: {} (looked at {} and {})",
        name,
        project_role_file(project_root, &canonical).display(),
        global_role_file(&canonical)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(no global dir)".into())
    )
}

/// Load a role by name, discarding salvage warnings. Most callers only
/// want the state; `aida role show` / `aida role repair` use
/// `load_role_with_warnings` to surface what was quarantined.
pub(crate) fn load_role(
    project_root: &std::path::Path,
    name: &str,
) -> Result<(RoleState, std::path::PathBuf)> {
    let (state, path, _warnings) = load_role_with_warnings(project_root, name)?;
    Ok((state, path))
}

/// Save back to the same location the role was loaded from (or the
/// project / global location based on `state.global` for fresh roles).
pub(crate) fn save_role_at(state: &RoleState, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(state)?;
    // BUG-228: atomic write — a bare std::fs::write let two concurrent
    // `aida` processes interleave their bytes into a torn, unparseable file.
    write_atomic(path, &content).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

pub(crate) fn role_save_path(
    project_root: &std::path::Path,
    state: &RoleState,
) -> Result<std::path::PathBuf> {
    if state.global {
        global_role_file(&state.name)
            .ok_or_else(|| anyhow::anyhow!("Cannot determine $HOME for global role storage"))
    } else {
        Ok(project_role_file(project_root, &state.name))
    }
}

pub(crate) fn list_roles(project_root: &std::path::Path) -> Result<Vec<RoleState>> {
    let mut roles = Vec::new();
    for dir in [Some(project_roles_dir(project_root)), global_roles_dir()]
        .into_iter()
        .flatten()
    {
        if !dir.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension() == Some(std::ffi::OsStr::new("toml")) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    // BUG-228: lenient parse — a role with a corrupted
                    // activity entry still appears in `aida role list`
                    // instead of silently vanishing from it.
                    if let Ok((mut state, _warnings)) = parse_role_lenient(&content) {
                        // TASK-586: surface `dialog.toml` as the advisor role.
                        state.name = canonical_role_name(&state.name);
                        roles.push(state);
                    }
                }
            }
        }
    }
    // trace:BUG-1116 | ai:codex
    // Recovery hints must be driven by stored role activity, not filesystem
    // traversal order; equal timestamps use a deterministic role-name winner.
    roles.sort_by(|a, b| {
        b.last_active_at
            .cmp(&a.last_active_at)
            .then_with(|| a.name.cmp(&b.name))
    });
    // TASK-586: a machine mid-migration can have both `advisor.toml` and the
    // legacy `dialog.toml` — both canonicalize to `advisor`. Keep the
    // most-recently-active (the sort above already put it first) and drop the
    // duplicate so `aida role list` shows one advisor.
    let mut seen = std::collections::HashSet::new();
    roles.retain(|r| seen.insert(r.name.clone()));
    Ok(roles)
}

/// True when this process carries a currently valid session-bound seat grant.
///
/// The read side still defaults to `implementer` when unset, but operator-facing
/// recovery surfaces need to distinguish "defaulted" from "seated".
// trace:BUG-1044 | ai:codex
pub(crate) fn active_role_env_present() -> bool {
    seat_authority::current_grant(&statusline_project_root()).is_some()
}

/// Most recently used role, derived from the same role files `aida role list`
/// sorts by recency. Best-effort: a machine with no role files simply has no
/// recovery hint to print.
// trace:BUG-1044 | ai:codex
pub(crate) fn last_used_role_name(project_root: &std::path::Path) -> Option<String> {
    list_roles(project_root)
        .ok()
        .and_then(|roles| roles.into_iter().next())
        .map(|role| role.name)
}

/// Pure core for [`roleless_recovery_line`], split out so tests don't need to
/// mutate process-global environment variables.
// trace:BUG-1044 | ai:codex
pub(crate) fn roleless_recovery_line_for(
    project_root: &std::path::Path,
    active_role_present: bool,
) -> Option<String> {
    if active_role_present {
        return None;
    }
    let role = last_used_role_name(project_root)?;
    Some(format!(
        "No active role. Last-used role: {role}. Run: `aida role enter {role}`"
    ))
}

/// One-line recovery hint for shells that are relying on the implicit default
/// role after a reboot/new terminal. Shared so `aida status` and gated-operation
/// refusals use the same copyable command.
// trace:BUG-1044 | ai:codex
pub(crate) fn roleless_recovery_line(project_root: &std::path::Path) -> Option<String> {
    roleless_recovery_line_for(project_root, active_role_env_present())
}

/// Append the roleless recovery line to advisor-authority refusals when the
/// current shell is roleless. Empty when no role recovery clue is available.
// trace:BUG-1044 | ai:codex
pub(crate) fn roleless_recovery_sentence() -> String {
    let Ok(project_root) = find_project_root() else {
        return String::new();
    };
    roleless_recovery_line(&project_root)
        .map(|line| format!(" {line}."))
        .unwrap_or_default()
}

pub(crate) fn role_restore_prompt_enabled(project_root: &std::path::Path) -> bool {
    role_restore_prompt_enabled_from(
        &std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
            .unwrap_or_default(),
    )
}

pub(crate) fn role_restore_prompt_enabled_from(content: &str) -> bool {
    let Ok(value) = content.parse::<toml::Value>() else {
        return false;
    };
    value
        .get("role")
        .and_then(|role| role.get("restore_prompt"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

pub(crate) fn role_restore_prompt_marker_name(tty: &str) -> String {
    let mut out = String::from(".role-restore-prompt-");
    for ch in tty.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    out
}

pub(crate) fn role_restore_prompt_tty_id() -> Option<String> {
    std::fs::read_link("/proc/self/fd/0")
        .ok()
        .map(|p| p.display().to_string())
        .filter(|s| !s.trim().is_empty())
}

pub(crate) fn role_restore_prompt_should_skip(command: &Command) -> bool {
    matches!(
        command,
        Command::Init { .. }
            | Command::Role(_)
            | Command::McpServe
            | Command::Internal { .. }
            | Command::Statusline { .. }
            | Command::Statusbar { .. }
            | Command::Contract { .. }
    )
}

pub(crate) fn maybe_prompt_role_restore(command: &Command) -> Result<()> {
    if role_restore_prompt_should_skip(command)
        || active_role_env_present()
        || std::env::var("AIDA_HEADLESS").as_deref() == Ok("1")
        || agent_output_mode()
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
    {
        return Ok(());
    }
    let Ok(project_root) = find_project_root() else {
        return Ok(());
    };
    if !role_restore_prompt_enabled(&project_root) {
        return Ok(());
    }
    let Some(role) = last_used_role_name(&project_root) else {
        return Ok(());
    };
    let tty = role_restore_prompt_tty_id().unwrap_or_else(|| "unknown".to_string());
    let marker = project_root
        .join(".aida")
        .join(role_restore_prompt_marker_name(&tty));
    if marker.exists() {
        return Ok(());
    }
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&marker, chrono::Utc::now().to_rfc3339());
    let restore = format!("aida role enter {role}");
    let yes = prompt_yes_no(
        &format!("No active AIDA role. Restore last-used role `{role}`? [y/N] "),
        false,
    )
    .unwrap_or(false);
    if yes {
        println!("Run: `{restore}`");
    } else {
        println!("Skipped role restore. Run later: `{restore}`");
    }
    Ok(())
}

/// Append an activity entry to the active role's log (best-effort; silently
/// no-op if no role active or the role file is unwriteable). Called from
/// the show/edit/add/comment paths so resuming a role surfaces what the
/// user was working on last.
pub(crate) fn record_role_activity(spec_id: &str, action: &str) {
    let role_name = match std::env::var("AIDA_SESSION_ROLE") {
        Ok(n) if !n.is_empty() => n,
        _ => return,
    };
    let project = match std::env::var("AIDA_SESSION_PROJECT") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => statusline_project_root(),
    };

    // STORY-56: when this shell is inside a session lease's worktree,
    // route the per-spec activity to the session-local log so concurrent
    // sessions don't clobber each other's @SPEC. The project-level role
    // file still gets `last_active_at` bumped so `aida role list --recent`
    // keeps working as a "what role was used most recently" view, but its
    // `activity` stream stays at whatever the last non-session shell saw
    // until session-end flattens the session log into it.
    // trace:STORY-56 | ai:claude
    let lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(&project, &cwd));

    if let Some(lease) = lease {
        let _ = append_session_activity(&project, &lease.id, &role_name, spec_id, action);
        if let Ok((mut state, path)) = load_role(&project, &role_name) {
            state.last_active_at = chrono::Utc::now();
            let _ = save_role_at(&state, &path);
        }
        return;
    }

    let (mut state, path) = match load_role(&project, &role_name) {
        Ok(t) => t,
        Err(_) => return,
    };
    // BUG-65: LRU-by-(spec_id, action). Drop any prior entry with the
    // same key, then insert at the front — interleaved sequences like
    // [show A, edit B, show A] no longer leave a stale duplicate behind.
    // trace:BUG-65 | ai:claude
    let entry = RoleActivity {
        spec_id: spec_id.to_string(),
        action: action.to_string(),
        at: chrono::Utc::now(),
    };
    state
        .activity
        .retain(|prev| !(prev.spec_id == entry.spec_id && prev.action == entry.action));
    state.activity.insert(0, entry);
    state.activity.truncate(ACTIVITY_MAX);
    state.last_active_at = chrono::Utc::now();
    let _ = save_role_at(&state, &path);
}

/// Resolve a role name from --name or AIDA_SESSION_ROLE; error if neither.
/// trace:TASK-1-021 | ai:claude
pub(crate) fn resolve_role_name(name: Option<&str>) -> Result<String> {
    // TASK-586: canonicalize so an explicit `--name dialog` or a stale
    // `AIDA_SESSION_ROLE=dialog` shell still resolves to the advisor role.
    if let Some(n) = name {
        return Ok(canonical_role_name(n));
    }
    match std::env::var("AIDA_SESSION_ROLE") {
        Ok(n) if !n.is_empty() => Ok(canonical_role_name(&n)),
        _ => anyhow::bail!(
            "No role active and no --name given. Either `aida role enter <name>` first \
             or pass --name to target a specific role."
        ),
    }
}

/// Read the active role's scope (tags, status), if any. Returns None when
/// no role is active or the role file is unreadable. Used by `aida list`
/// and `aida queue list/next` to compose default filters.
/// trace:TASK-1-021 | ai:claude
pub(crate) fn active_role_scope() -> Option<(Vec<String>, Option<String>)> {
    let role_name = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.is_empty())?;
    let project = std::env::var("AIDA_SESSION_PROJECT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| statusline_project_root());
    let (state, _) = load_role(&project, &role_name).ok()?;
    if state.scope_tags.is_empty() && state.scope_status.is_none() {
        return None;
    }
    Some((state.scope_tags, state.scope_status))
}

// trace:TASK-713 trace:TASK-817
/// One displayable role row in the interactive picker. Decoupled from
/// `RoleState` so the label layout (`format_role_picker_option`) is pure and
/// testable without a real TTY read.
pub(crate) struct RolePickerRow {
    /// Arrow = offered default, `*` = currently-active shell role, ` ` otherwise.
    pub(crate) marker: String,
    pub(crate) name: String,
    pub(crate) global: bool,
    /// Pre-humanized recency, e.g. "3h ago".
    pub(crate) recency: String,
    pub(crate) purpose: Option<String>,
    /// Pre-humanized age for a live agent driver with this role, e.g. "1m ago".
    pub(crate) live_driver_recency: Option<String>,
    /// True for repo-wide single-instance seats such as advisor/product.
    pub(crate) single_instance: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RolePickerLaunchAnnotation {
    pub(crate) live_driver_recency: Option<String>,
    pub(crate) single_instance: bool,
}

/// TASK-713: word-wrap `text` to `width` columns, capped at `max_lines`
/// (the last kept line is truncated with `…` if there's overflow). Greedy
/// word-wrap; a single word longer than `width` is hard-split. Pure.
pub(crate) fn wrap_role_purpose(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        // Hard-split a word that can't fit on its own line.
        let mut word = word.to_string();
        while word.chars().count() > width {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            let head: String = word.chars().take(width).collect();
            lines.push(head);
            word = word.chars().skip(width).collect();
        }
        if cur.is_empty() {
            cur = word;
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(&word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        // Mark the final kept line as truncated.
        if let Some(last) = lines.last_mut() {
            let trimmed: String = last
                .chars()
                .take(width.saturating_sub(1))
                .collect::<String>()
                .trim_end()
                .to_string();
            *last = format!("{trimmed}…");
        }
    }
    lines
}

/// TASK-644/TASK-646/TASK-817: shared role picker. Renders `header` then the
/// role list with arrow-key navigation via `inquire::Select` — up/down move the
/// highlight, Enter selects, Esc cancels. inquire writes its prompt to stderr
/// (not stdout), so callers whose stdout is a captured pipe (`eval "$(...)"`)
/// stay uncorrupted. `highlight`, when set, marks that role with an arrow and is
/// pre-selected (cursor parked on it) when the picker opens. Returns
/// `Ok(Some(name))` on selection, `Ok(None)` on cancel.
// trace:TASK-817 | ai:claude
pub(crate) fn pick_role_with_header(
    project_root: &std::path::Path,
    header: &str,
    highlight: Option<&str>,
    launch_annotations: Option<&std::collections::BTreeMap<String, RolePickerLaunchAnnotation>>,
) -> Result<Option<String>> {
    let roles = list_roles(project_root)?;
    if roles.is_empty() {
        anyhow::bail!(
            "No roles defined for {}.\n\
             Create one with: `aida role add <name>`\n\
             Or install a starter set: `aida role scaffold`",
            project_root.display()
        );
    }

    let active = std::env::var("AIDA_SESSION_ROLE").ok();
    let highlight_idx = highlight.and_then(|h| roles.iter().position(|r| r.name == h));

    // TASK-817: build one scannable single-line label per role from the same
    // RolePickerRow data the old numeric picker used — name + scope + recency,
    // with the purpose appended (truncated to keep the option on one line).
    let rows: Vec<RolePickerRow> = roles
        .iter()
        .enumerate()
        .map(|(i, role)| {
            // `*` = currently-active shell role; the Arrow glyph = the offered default.
            let marker = if Some(i) == highlight_idx {
                crate::glyph(crate::glyphs::Glyph::Arrow).to_string()
            } else if active.as_deref() == Some(&role.name) {
                "*".to_string()
            } else {
                " ".to_string()
            };
            let annotation = launch_annotations.and_then(|a| a.get(&role.name));
            RolePickerRow {
                marker,
                name: role.name.clone(),
                global: role.global,
                recency: humanize_relative(role.last_active_at),
                purpose: role.purpose.clone(),
                live_driver_recency: annotation.and_then(|a| a.live_driver_recency.clone()),
                single_instance: annotation.map(|a| a.single_instance).unwrap_or(false),
            }
        })
        .collect();

    let role_labels: Vec<String> = rows
        .iter()
        .map(|r| format_role_picker_option(r, picker_terminal_width()))
        .collect();
    let role_names: Vec<String> = roles.iter().map(|r| r.name.clone()).collect();

    // TASK-1239: append the stakeholder personas (guest/requester) as a
    // separate, clearly-labeled sub-section — launchable, but visually distinct
    // from and never mixed into the driver/build seats.
    let (labels, targets) = assemble_role_picker_items(role_labels, &role_names, active.as_deref());

    let mut select = inquire::Select::new(header, labels).with_help_message(
        "Use arrow keys to move, type to filter, Enter to select, Esc to cancel",
    );
    // TASK-817: park the cursor on the offered default so Enter accepts it.
    if let Some(idx) = highlight_idx {
        select = select.with_starting_cursor(idx);
    }

    match select.raw_prompt() {
        // raw_prompt returns the picked ListOption, whose index maps straight
        // back to the targets list (roles first, then the stakeholder section).
        Ok(choice) => match targets.get(choice.index) {
            Some(RolePickTarget::Role(name)) | Some(RolePickTarget::Persona(name)) => {
                Ok(Some(name.clone()))
            }
            // The section heading is not a real selection → treat as cancel.
            Some(RolePickTarget::Divider) | None => Ok(None),
        },
        // Esc / Ctrl-C cancel → Ok(None), matching the old q/blank behavior.
        Err(inquire::InquireError::OperationCanceled)
        | Err(inquire::InquireError::OperationInterrupted) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("role picker failed: {e}")),
    }
}

/// TASK-1239: one entry in the `aida agent new` role picker — a build/driver
/// role, a stakeholder persona, or the non-selectable section heading.
// trace:TASK-1239 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RolePickTarget {
    Role(String),
    Persona(String),
    Divider,
}

/// TASK-1239: append the stakeholder personas (guest/requester) to the role
/// picker as a SEPARATE, clearly-labeled sub-section, mirroring `aida role
/// list`'s "Stakeholder personas" block. Personas are programmatic
/// AIDA_SESSION_ROLE gates (STORY-1110), NOT role files or queue-routable build
/// seats — so they land after all driver roles, under a heading, each tagged
/// "not a build seat". Pure so the section layout is testable without the
/// interactive picker. Returns aligned (labels, targets); the divider index maps
/// to no selection.
// trace:TASK-1239 | ai:claude
pub(crate) fn assemble_role_picker_items(
    role_labels: Vec<String>,
    role_names: &[String],
    active: Option<&str>,
) -> (Vec<String>, Vec<RolePickTarget>) {
    let mut labels = role_labels;
    let mut targets: Vec<RolePickTarget> = role_names
        .iter()
        .map(|n| RolePickTarget::Role(n.clone()))
        .collect();

    labels.push("  ── Stakeholder personas · least-privilege · not a build seat ──".to_string());
    targets.push(RolePickTarget::Divider);

    for (name, blurb) in [
        ("guest", "least-privilege read-only; not a build seat"),
        ("requester", "least-privilege read/intake; not a build seat"),
    ] {
        let marker = if active == Some(name) { "*" } else { " " };
        labels.push(format!("{marker} {name} — {blurb}"));
        targets.push(RolePickTarget::Persona(name.to_string()));
    }

    (labels, targets)
}

/// TASK-817: format one role as a single-line `inquire::Select` option label
/// from its `RolePickerRow`. Shows `<marker> <name>[ [global]] · <recency>`
/// with the purpose appended after `—`, truncated so the whole option stays on
/// one line within `width`. Pure so the label layout is testable.
// trace:TASK-817 | ai:claude
pub(crate) fn format_role_picker_option(row: &RolePickerRow, width: usize) -> String {
    let scope = if row.global { " [global]" } else { "" };
    let mut label = format!("{} {}{} · {}", row.marker, row.name, scope, row.recency);

    if let Some(seat) = format_role_picker_launch_status(row) {
        label.push_str(" · ");
        label.push_str(&seat);
    }

    if let Some(purpose) = row
        .purpose
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        // Reserve room for the current label + " — " separator; truncate the
        // purpose (with an ellipsis) so the option never wraps the terminal.
        let used = label.chars().count() + 3;
        let avail = width.saturating_sub(used);
        if avail >= 8 {
            let purpose = wrap_role_purpose(purpose, avail, 1)
                .into_iter()
                .next()
                .unwrap_or_else(|| purpose.to_string());
            label.push_str(" — ");
            label.push_str(&purpose);
        }
    }
    label
}

// trace:TASK-1236 | ai:codex
pub(crate) fn format_role_picker_launch_status(row: &RolePickerRow) -> Option<String> {
    match (row.live_driver_recency.as_deref(), row.single_instance) {
        (Some(age), true) => Some(format!("{age} live driver (single-instance)")),
        (Some(age), false) => Some(format!("{age} live driver")),
        (None, true) => Some("free (single-instance)".to_string()),
        (None, false) => None,
    }
}

/// Starter role set installed by `aida role scaffold`. Idempotent — skips
/// any name that already exists (anywhere). All starter roles are global
/// since they're meant to apply across projects.
///
/// The default set is the **agent-wired** role taxonomy — the roles the
/// orchestrator actually drives and routes work to (`implementer`,
/// `advisor`, `reviewer`, `integrator`) plus the product intake seat that
/// captures requirements and routes work before implementation.
/// `architect` and `triage` are deliberately NOT scaffolded by default:
/// they have no orchestrator phase and sit empirically dormant, so shipping
/// them as starters invites the "what's this role for?" first-impression
/// confusion. They remain valid role names — install them opt-in with
/// `aida role add <name>`. The canonical taxonomy lives in
/// `validate_registered_agent_role`.
// trace:TASK-608 | ai:claude
// trace:STORY-460 | ai:claude — integrator joins the agent-wired starter set
// trace:TASK-1200 | ai:codex — product joins the first-machine starter set
pub(crate) const STARTER_ROLES: &[(&str, &str, Option<&str>)] = &[
    (
        "implementer",
        "Heads-down coding on a specific feature or fix. Drive a requirement to completed.",
        None,
    ),
    (
        "product",
        "Intake and product-owner seat. Groom drafts, capture requirements, sharpen acceptance criteria, and route work to the right queue. Distinct from advisor: product owns requirement capture; advisor owns strategic counsel, disposition, and design-fork judgment.",
        Some("You own product intake and wave continuity. Turn observed needs into bounded specs with testable acceptance, order the ready queue, and launch the next eligible wave; never approve your own disputed product judgment or merge implementation. The advisor independently gates disposition, design forks, rework quality, and merge readiness. Read `.aida/discipline/two-seat-protocol.md`; use `.aida/discipline/rework-brief-craft.md` and `.aida/discipline/seat-recovery-playbooks.md` when those cases arise. Recurring duties arrive as due jobs, not as rules to remember."),
    ),
    (
        "advisor",
        "Trusted counsel across the project's lifetime. Surfaces friction, articulates mental models, gardens the queue, curates memory across sessions. Produces specs and comments, not code; routes implementation to doer roles via `aida queue add --for <role>`.",
        Some("You are the independent judgment gate. You approve or reject dispositions, resolve grounded design forks, gate merges/rework, and turn review verdicts into actionable rework briefs. Never implement or merge code you authored, and never waive an unresolved gate merely to keep work moving. Read `.aida/discipline/two-seat-protocol.md`; follow `.aida/discipline/rework-brief-craft.md` for every requested-change handoff and `.aida/discipline/seat-recovery-playbooks.md` for recovery. Recurring duties arrive as due jobs, not as rules to remember. Wait for mail through a zero-token shell/event watcher; never create model-side CronCreate, /loop, or ScheduleWakeup mailbox polls."),
    ),
    (
        "reviewer",
        "Code/PR review. Walk diffs, check trace comments, verify against requirements.",
        None,
    ),
    (
        "integrator",
        "Owns the merge cascade — rebases PRs, resolves mechanical conflicts, watches CI, squash-merges CI-green-and-verdict-present PRs, deletes merged branches, runs `aida pull`. Escalates design-judgment conflicts to the advisor; routes missing-verdict PRs to the reviewer.",
        None,
    ),
];

/// Core of `aida role scaffold` (TASK-608): install the [`STARTER_ROLES`] into
/// global `~/.aida/roles/`, skipping any that already exist (idempotent,
/// non-destructive). Returns the (created, skipped) role names so callers can
/// report in their own voice — both the `role scaffold` command and `aida init`
/// (TASK-638) reuse this rather than re-deriving the role set.
/// trace:TASK-608 TASK-638 | ai:claude
pub(crate) fn scaffold_starter_roles(
    project_root: &std::path::Path,
) -> Result<(Vec<&'static str>, Vec<&'static str>)> {
    let mut created: Vec<&'static str> = Vec::new();
    let mut skipped: Vec<&'static str> = Vec::new();
    for (name, purpose, system_prompt) in STARTER_ROLES {
        if load_role(project_root, name).is_ok() {
            skipped.push(name);
            continue;
        }
        let state = RoleState {
            name: (*name).to_string(),
            purpose: Some((*purpose).to_string()),
            created_at: chrono::Utc::now(),
            last_active_at: chrono::Utc::now(),
            working_directory: None,
            notes: None,
            global: true,
            activity: Vec::new(),
            scope_tags: Vec::new(),
            scope_status: None,
            system_prompt: system_prompt.map(|prompt| (*prompt).to_string()),
            launch_prompt: None,
            launch_prompt_spec: None,
        };
        let path = role_save_path(project_root, &state)?;
        save_role_at(&state, &path)?;
        created.push(name);
    }
    Ok((created, skipped))
}

#[cfg(test)]
#[path = "tests/role_repair_tests.rs"]
mod role_repair_tests;

// trace:TASK-586 | ai:claude
#[cfg(test)]
#[path = "tests/role_identity_tests.rs"]
mod role_identity_tests;

// trace:STORY-1133 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoleInstanceKind {
    Driver,
    Companion,
}

impl RoleInstanceKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Driver => "driver",
            Self::Companion => "companion",
        }
    }
}

/// TASK-646/TASK-1462: resolve the role for a SPAWNED CHILD agent. ADR-2
/// ordering:
///   1. `--role X` → use X (no prompt).
///   2. no `--role`, stdin is a TTY → prompt via the shared role picker,
///      pre-highlighting the operator's active role (`AIDA_SESSION_ROLE`) when
///      one is set and appears in the project's role list; otherwise falls
///      back to `implementer` (the dominant spawn), exactly as before. Blank
///      Enter accepts the highlighted role, `q` cancels.
///   3. no `--role`, non-interactive → default `implementer` + a one-line
///      notice; never errors, never hangs, never launches role-less.
///      The launching shell's `AIDA_SESSION_ROLE` is deliberately NOT inherited
///      into the LAUNCHED CHILD's role — advisor/product spawning an
///      implementer is the common case, so cloning the launcher's hat would be
///      wrong. It is still consulted here, read-only, to pick the picker's
///      starting cursor. Returns `Ok(None)` only when the user cancels the
///      picker, so the caller aborts the launch cleanly.
// trace:TASK-1462 | ai:claude
pub(crate) fn resolve_child_role(
    project_root: &std::path::Path,
    role: Option<String>,
    agent_type: &str,
) -> Result<Option<String>> {
    if let Some(r) = role {
        return Ok(Some(r));
    }
    if std::io::stdin().is_terminal() {
        let header = format!("Select a role for the new {} agent:", agent_type);
        let annotations = role_picker_launch_annotations(project_root, chrono::Utc::now());
        // TASK-1462: prefer the operator's active shell role as the picker's
        // default cursor; `implementer` remains the fallback when no role is
        // active. `pick_role_with_header` itself no-ops the highlight if the
        // name isn't in the project's role list (falls back to the top of the
        // list), so an active role from a different project is harmless here.
        let (active_role, is_default) = effective_role_resolved();
        let default_highlight = child_role_picker_default_highlight(&active_role, is_default);
        // `pick_role_with_header` returns Ok(None) on cancel → propagate as
        // the abort signal.
        pick_role_with_header(
            project_root,
            &header,
            Some(&default_highlight),
            Some(&annotations),
        )
    } else {
        eprintln!(
            "{} no --role given, defaulting to {} (non-interactive launch)",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            "implementer".cyan()
        );
        Ok(Some("implementer".to_string()))
    }
}

/// TASK-1462: the role picker's starting-cursor name — the operator's active
/// shell role when one is set (`is_default == false`), `implementer`
/// otherwise. Pure so the default-vs-active choice is testable without a TTY
/// or `AIDA_SESSION_ROLE`. `pick_role_with_header` separately no-ops a
/// highlight that isn't in the project's role list.
// trace:TASK-1462 | ai:claude
pub(crate) fn child_role_picker_default_highlight(active_role: &str, is_default: bool) -> String {
    if is_default {
        "implementer".to_string()
    } else {
        active_role.to_string()
    }
}

/// TASK-646: project root for child-role resolution, derived from the
/// launch cwd (or the process cwd) the same way `agent_new_with_config`
/// derives it — so the picker lists the right project's roles.
pub(crate) fn child_role_project_root(cwd: Option<&std::path::Path>) -> Result<std::path::PathBuf> {
    let base = cwd
        .map(std::path::PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let discovered_root = find_aida_project_root_from(&base)?;
    Ok(main_worktree_root_from(&discovered_root))
}

pub(crate) fn same_agent_role_view(
    agent: &agent_registry::AgentRegistryView,
    agent_type: &str,
    role: Option<&str>,
) -> bool {
    agent.agent_type.eq_ignore_ascii_case(agent_type)
        && agent
            .role
            .as_deref()
            .zip(role)
            .map(|(a, b)| canonical_role_name(a) == canonical_role_name(b))
            .unwrap_or(false)
}

// trace:TASK-1236 | ai:codex
pub(crate) fn role_picker_launch_annotations(
    project_root: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
) -> std::collections::BTreeMap<String, RolePickerLaunchAnnotation> {
    let cfg = agent_registry::Config::load(project_root);
    let mut annotations: std::collections::BTreeMap<String, RolePickerLaunchAnnotation> =
        AGENT_ROLES
            .iter()
            .map(|role| {
                (
                    (*role).to_string(),
                    RolePickerLaunchAnnotation {
                        single_instance: role_picker_role_is_single_instance(&cfg, role),
                        live_driver_recency: None,
                    },
                )
            })
            .collect();

    for agent in agent_launch_views(project_root)
        .into_iter()
        .filter(|agent| {
            agent.ended_at.is_none() && agent.status != agent_registry::AgentStatus::Stale
        })
    {
        let Some(role) = agent.role.as_deref().map(canonical_role_name) else {
            continue;
        };
        let elapsed = agent_registry::humanize_elapsed(agent_registry::elapsed_secs_clamped(
            now,
            agent.started_at,
        ));
        annotations
            .entry(role)
            .or_default()
            .live_driver_recency
            .get_or_insert(elapsed);
    }

    annotations
}

// The picker copy uses "single-instance" for repo-wide human-visible seats.
// Scoped duplicate prevention for implementer/reviewer remains enforced later
// by `same_scope_conflict`, but those roles are not labelled as taken seats.
// trace:TASK-1236 | ai:codex
pub(crate) fn role_picker_role_is_single_instance(
    cfg: &agent_registry::Config,
    role: &str,
) -> bool {
    cfg.role_is_singleton(role)
        && matches!(canonical_role_name(role).as_str(), "advisor" | "product")
}

// trace:STORY-1133 | ai:codex
pub(crate) fn resolve_role_instance_for_launch(
    project_root: &std::path::Path,
    role: Option<&str>,
    current_spec: Option<&str>,
    worktree_path: &std::path::Path,
) -> RoleInstanceKind {
    let Some(role) = role else {
        return RoleInstanceKind::Driver;
    };
    if current_spec.is_some() {
        return RoleInstanceKind::Driver;
    }
    let role_lc = role.to_ascii_lowercase();
    if !matches!(role_lc.as_str(), "advisor" | "product") {
        return RoleInstanceKind::Driver;
    }
    let cfg = agent_registry::Config::load(project_root);
    if agent_registry::same_scope_conflict(project_root, &cfg, Some(role), None, worktree_path)
        .is_some()
    {
        RoleInstanceKind::Companion
    } else {
        RoleInstanceKind::Driver
    }
}

// trace:STORY-436 | ai:codex
pub(crate) fn role_guidance_for(project_root: &std::path::Path, role: &str) -> String {
    if role != "unspecified" {
        if let Ok((state, path)) = load_role(project_root, role) {
            let mut out = String::new();
            out.push_str(&format!("Loaded role file: {}\n\n", path.display()));
            if let Some(purpose) = state.purpose.as_deref().filter(|s| !s.trim().is_empty()) {
                out.push_str("Purpose:\n");
                out.push_str(purpose.trim());
                out.push_str("\n\n");
            }
            if let Some(prompt) = state
                .system_prompt
                .as_deref()
                .filter(|s| !s.trim().is_empty())
            {
                out.push_str("System prompt:\n");
                out.push_str(prompt.trim());
                return out;
            }
            if !out.trim().is_empty() {
                return out.trim_end().to_string();
            }
        }
    }

    default_role_guidance(role)
}

/// The built-in per-role guidance used when no stored role file (`~/.aida/roles/
/// <role>.toml` or a project role) provides a Purpose / system prompt. Every
/// agent-wired seat (`implementer`, `advisor`, `reviewer`, `integrator`) plus
/// the product intake seat gets a first-class arm so a fresh machine with no
/// scaffolded role file still ships seat-specific context; unknown roles fall
/// through to the generic pointer.
/// Split out from [`role_guidance_for`] so the arms are unit-testable without a
/// machine's role files shadowing them.
// trace:STORY-718 | ai:claude
pub(crate) fn default_role_guidance(role: &str) -> String {
    if let Some(guidance) = queue_cmd::stakeholder_persona_guidance(role) {
        return guidance.to_string();
    }
    match role {
        "advisor" | "dialog" => "You are advising the operator. Triage punts/findings, route implementation, clarify design forks, and avoid changing code unless explicitly asked. Wait for mail through a zero-token shell/event watcher; never create model-side CronCreate, /loop, or ScheduleWakeup mailbox polls.".to_string(),
        "implementer" => "You are implementing. Read the assigned spec/brief, work in the supervised worktree, keep changes bounded to acceptance, run relevant tests, commit with the spec trailer, and finish with `aida pr ship`.".to_string(),
        // trace:TASK-1200 | ai:codex
        "product" => "You are wearing the product seat. Groom drafts, capture requirements, sharpen acceptance criteria, and route work to the right queue. Focus on intake and requirement capture; leave strategic counsel and disposition calls to the advisor.".to_string(),
        // A cold-booted reviewer must discover the durable writer here; hand-
        // writing JSON omits the provenance that makes approval enforceable.
        // trace:BUG-1467 | ai:codex
        "reviewer" => "You are reviewing. Inspect the PR and linked spec, prioritize correctness/regression risks, run targeted tests when useful, and produce a clear verdict/finding rather than taking over implementation. Record the result through `aida review record <SPEC> --pr <N> --verdict approved|request-changes|rejected --summary \"<why>\"` (repeat `--finding` as needed); never hand-write verdict JSON.".to_string(),
        // The integrator seat's role-context prompt, mirroring the arms above.
        // Mechanical merge cascade only; escalate anything that turns on judgment.
        "integrator" => "You are integrating. Land finished work (Done specs with an open PR) on the default branch one PR at a time in dependency order: rebase stale branches, resolve MECHANICAL conflicts only, watch CI, squash-merge the green-and-reviewed PRs, delete merged branches, and run `aida pull` to auto-bump. Never make a design call — escalate design-judgment conflicts to the advisor and route missing-verdict PRs to the reviewer.".to_string(),
        "unspecified" => "No role was provided. Determine whether you are acting as product, advisor, implementer, reviewer, or integrator before making changes.".to_string(),
        other => format!("No stored role file was found for `{other}`. Follow the project discipline in AGENTS.md and the active spec/brief context."),
    }
}

// trace:TASK-587 | ai:antigravity
pub const AGENT_ROLES: &[&str] = &["implementer", "advisor", "reviewer", "integrator"];

// The agent-role taxonomy is CLOSED because each role maps to a stage the
// orchestrator drives — that's the accurate framing (the prior version invented
// per-role lease caps and mischaracterized the roles). Structure (struct +
// const + subcommand) is AGY's TASK-587; the content is corrected here.
// trace:BUG-421 | ai:claude
#[derive(serde::Serialize)]
pub(crate) struct AgentRoleInfo {
    pub(crate) role: &'static str,
    pub(crate) orchestrator_phase: &'static str,
    pub(crate) summary: &'static str,
}

// trace:BUG-421 | ai:claude
pub(crate) const AGENT_ROLE_INFOS: &[AgentRoleInfo] = &[
    AgentRoleInfo {
        role: "implementer",
        orchestrator_phase: "phase 1 — implement",
        summary: "Drives one spec to completion in its own dedicated worktree, then opens a PR.",
    },
    AgentRoleInfo {
        role: "advisor",
        orchestrator_phase: "escalation / advisory tier",
        summary: "Strategic partner + escalation seat — routes work, resolves punted design-forks, captures friction. The headless advisor tier in --no-human drains.",
    },
    AgentRoleInfo {
        role: "reviewer",
        orchestrator_phase: "phase 3 — review",
        summary: "Reviews a PR (walks the diff, checks trace comments, verifies against the spec) and reaches a merge verdict.",
    },
    AgentRoleInfo {
        role: "integrator",
        orchestrator_phase: "integration",
        summary: "Watches open PRs, merges clean ones, and handles branch updates.",
    },
];

// trace:BUG-421 | ai:claude
pub(crate) fn handle_agent_list_roles(json: bool) -> Result<()> {
    if json {
        let serialized = serde_json::to_string_pretty(AGENT_ROLE_INFOS)?;
        println!("{}", serialized);
    } else {
        println!("{:<12} | {:<28} | Summary", "Role", "Orchestrator phase");
        println!("{:-<12}-+-{:-<28}-+-{:-<60}", "", "", "");
        for info in AGENT_ROLE_INFOS {
            println!(
                "{:<12} | {:<28} | {}",
                info.role, info.orchestrator_phase, info.summary
            );
        }
        println!();
        println!("These are the closed agent-role set (valid for `aida agent new --role`) — one per orchestrator stage.");
        println!("See also: `aida role list` — operator/human personas (an open set; overlaps on implementer/advisor/reviewer).");
    }
    Ok(())
}

// trace:TASK-543 | ai:codex
pub(crate) fn validate_registered_agent_role(raw: &str) -> Result<String> {
    let role = raw.trim().to_ascii_lowercase();
    if AGENT_ROLES.contains(&role.as_str()) {
        Ok(role)
    } else {
        anyhow::bail!(
            "--role must be one of: {} (got `{}`)",
            AGENT_ROLES.join(", "),
            raw
        )
    }
}

/// BUG-511: like [`in_flight_lease_scopes`] but keeps each live lease's
/// role, so the open-spec explainer can say *what kind* of work holds the
/// spec ("being reviewed" vs the generic in-flight line). Same liveness
/// rules: [`lease_state_for`] over every lease, keep the `Live` ones.
/// trace:BUG-511 | ai:claude
pub(crate) fn in_flight_lease_role_map(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, Option<String>> {
    let leases = list_leases(project_root);
    if leases.is_empty() {
        return std::collections::HashMap::new();
    }
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    leases
        .iter()
        .filter_map(|l| match lease_state_for(l, &live, now) {
            LeaseState::Live => Some((l.scope.to_ascii_lowercase(), l.role.clone())),
            _ => None,
        })
        .collect()
}

/// Fold a closed session's activity entries back into each participating
/// role's project-level `activity` stream. Called from `session_end` so
/// long-running views (`aida role show`, `aida statusline` outside any
/// session) still surface what was worked on under the session. Does NOT
/// delete the activity log file — `session_end` handles that after this
/// returns successfully.
///
/// Per role: take the newest session entry per spec_id, merge in front of
/// the project role's existing activity, dedupe by spec_id, truncate to
/// ACTIVITY_MAX. Best-effort — malformed/missing role files are skipped
/// (the project role might have been deleted while the session ran).
/// trace:STORY-56 | ai:claude
pub(crate) fn aggregate_session_activity_into_roles(
    project_root: &std::path::Path,
    session_id: &str,
) {
    let log = load_session_activity(project_root, session_id);
    if log.entries.is_empty() {
        return;
    }
    // Group newest-first by role; within each role the first entry per
    // spec_id wins (newest, since `entries` is newest-first by design).
    use std::collections::{BTreeMap, BTreeSet};
    let mut per_role: BTreeMap<String, Vec<RoleActivity>> = BTreeMap::new();
    for entry in &log.entries {
        let bucket = per_role.entry(entry.role.clone()).or_default();
        if !bucket.iter().any(|a| a.spec_id == entry.spec_id) {
            bucket.push(RoleActivity {
                spec_id: entry.spec_id.clone(),
                action: entry.action.clone(),
                at: entry.at,
            });
        }
    }
    for (role_name, mut new_entries) in per_role {
        let Ok((mut state, path)) = load_role(project_root, &role_name) else {
            continue;
        };
        // Merge: prepend session-newest entries, then append project's
        // existing entries skipping any spec_id already brought forward.
        let promoted: BTreeSet<String> = new_entries.iter().map(|e| e.spec_id.clone()).collect();
        new_entries.extend(
            state
                .activity
                .iter()
                .filter(|e| !promoted.contains(&e.spec_id))
                .cloned(),
        );
        new_entries.truncate(ACTIVITY_MAX);
        state.activity = new_entries;
        state.last_active_at = chrono::Utc::now();
        let _ = save_role_at(&state, &path);
    }
}

/// TASK-244: render the statusline `role:` segment, surfacing a mismatch
/// between the shell-persistent role (`$AIDA_SESSION_ROLE`, set by `aida
/// role enter`) and the active AIDA session's role (the lease). When
/// TASK-645: the one place "what role am I" is answered. An unset (or
/// blank) `AIDA_SESSION_ROLE` resolves to `implementer` — the read-side
/// default — so no command ever faces a roleless session. The env var is
/// never written by this path; it stays unset until the user explicitly
/// `eval "$(aida role enter <role>)"`s a different role.
pub(crate) fn effective_role() -> String {
    effective_role_resolved().0
}

/// Same resolution as [`effective_role`], but also reports whether the
/// result is the implicit default (`true`) versus an explicitly-entered
/// role (`false`). Callers that want to *show* the default-ness (e.g. the
/// statusline marking `implementer (default)`) use the flag. Pure over the
/// passed-in value so it is unit-testable without touching the process env.
pub(crate) fn resolve_effective_role(raw: Option<&str>) -> (String, bool) {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(r) => (canonical_role_name(r), false),
        None => ("implementer".to_string(), true),
    }
}

pub(crate) fn effective_role_resolved() -> (String, bool) {
    resolve_effective_role(std::env::var("AIDA_SESSION_ROLE").ok().as_deref())
}

/// Resolve the authorization seat only from a validated grant. The roster is
/// a ceiling, not an active seat; the environment role remains display/routing
/// metadata. Missing or invalid grants get the least-privilege implementer
/// baseline.
// trace:STORY-1473 | ai:codex
// trace:TASK-1593 | ai:antigravity
pub(crate) fn effective_role_with_roster() -> (String, team::RoleSource) {
    match find_project_root() {
        Ok(root) => match seat_authority::current_seat(&root) {
            Some(seat) => (seat, team::RoleSource::Grant),
            None => ("implementer".to_string(), team::RoleSource::Default),
        },
        Err(_) => ("implementer".to_string(), team::RoleSource::Default),
    }
}

// trace:STORY-1473 | ai:codex
/// STORY-647 (team RBAC slice 2): enforce the team permission map for a gated
/// op. Resolves the `[team]` policy + the caller's gated effective role (strict
/// mode honored), and bails with a clear, role-naming refusal when the role
/// doesn't satisfy the op's minimum role — UNLESS `force` is set (the audited
/// guardrail escape hatch). Best-effort: an unreachable store/config falls back
/// to the all-default (non-strict) policy and never hard-blocks. Live
/// orchestrators keep their workflow carve-out; TTY presence only issues a
/// seat grant and does not bypass this role gate.
pub(crate) fn enforce_team_gate(op: permissions::GatedOp, force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    let project_root = match find_project_root() {
        Ok(r) => r,
        // No project root → degraded; don't block. trace:STORY-647
        Err(_) => return Ok(()),
    };
    let config = permissions::TeamPermissions::load(&project_root);
    // Live orchestrators retain their workflow carve-out. TTY presence is not
    // authority; the role is resolved from a validated grant below.
    // trace:STORY-1473
    if authority_carveout_active() {
        return Ok(());
    }
    let user_id = current_user_id(None);
    let store_root = project_root.join(".aida-store");
    let (role, source) =
        permissions::gated_effective_role_for_user(Some(&store_root), &user_id, &config);
    if permissions::permits(op, &role, &config) {
        return Ok(());
    }
    anyhow::bail!(permissions::refusal_message(op, &role, source, &config));
}

// trace:STORY-1473 | ai:codex
/// Live orchestrator only. A TTY is an issuance requirement, not an authority
/// bypass.
pub(crate) fn authority_carveout_active() -> bool {
    let orchestrated = find_main_worktree_root()
        .map(|root| {
            matches!(
                orchestrator::detect(&root),
                orchestrator::OrchestratorContext::Orchestrated
            )
        })
        .unwrap_or(false);
    orchestrated
}

/// STORY-647: the protected-spec variant of [`enforce_team_gate`] — gates
/// editing/transitioning a spec carrying any `[team] protected_tags` entry on
/// the configured `protected_role` (advisor by default). A no-op when the spec
/// carries no protected tag, when `force` is set, or when the caller holds
/// advisor authority (live drain / advisor grant). Best-effort.
/// trace:STORY-647 | ai:claude
pub(crate) fn enforce_protected_spec_gate<'a, I>(tags: I, force: bool) -> Result<()>
where
    I: IntoIterator<Item = &'a String>,
{
    if force {
        return Ok(());
    }
    let project_root = match find_project_root() {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    let config = permissions::TeamPermissions::load(&project_root);
    if !config.spec_is_protected(tags) {
        return Ok(());
    }
    // TTY / live-orchestrator carve-out only — not the env-role leg (strict mode
    // must not be env-bypassable). trace:STORY-647
    if authority_carveout_active() {
        return Ok(());
    }
    let user_id = current_user_id(None);
    let store_root = project_root.join(".aida-store");
    let (role, source) =
        permissions::gated_effective_role_for_user(Some(&store_root), &user_id, &config);
    if permissions::permits(permissions::GatedOp::ProtectedSpec, &role, &config) {
        return Ok(());
    }
    anyhow::bail!(permissions::refusal_message(
        permissions::GatedOp::ProtectedSpec,
        &role,
        source,
        &config
    ));
}

/// TASK-647 (ADR-3): "advisor authority" — permission to produce approved+
/// specs and queue work for execution. Held by an explicit advisor role OR a
/// human at an interactive terminal (stdin is a TTY). Headless `claude -p`,
/// spawned agents, and drain/auto captures have neither, so their intake
/// lands `draft` for advisor triage. This is the hard gate (substrate-as-
/// bouncer): a soft default would be bypassed by `--status approved`.
///
/// NB: the MCP server runs non-TTY but may inherit an advisor `AIDA_SESSION_ROLE`
/// from the launching shell — it therefore does NOT use this helper and gates
/// unconditionally (see `tool_add_requirement`).
/// Pure core of [`has_advisor_authority`] (BUG-460): advisor authority is held
/// by an advisor role, an interactive (TTY) session, OR an operation that is
/// corroborated as running under a live orchestrator. The orchestrator case is
/// the fix: the drain's own auto-queue and its headless implementer/reviewer
/// phases are *workflow system actions* (advancing a spec through its
/// lifecycle), not a bare agent self-advancing — and corroboration requires a
/// token matching the live drain-state run, which a bare agent cannot forge, so
/// the TASK-647/ADR-3 gate still blocks an un-orchestrated agent.
/// trace:BUG-460 trace:TASK-647 | ai:claude
pub(crate) fn advisor_authority_from(role: &str, is_tty: bool, orchestrated: bool) -> bool {
    let _ = is_tty; // TTY is used to issue a grant, never as authority by itself.
    role == "advisor" || orchestrated
}

// trace:TASK-1594 | ai:claude
pub(crate) fn hold_authority_from(role: &str, is_tty: bool, _orchestrated: bool) -> bool {
    // ADR-66 carve-out: holds are the supervision floor and emergency brake.
    // The brake must stay reachable by a human at a TTY even when grant issuance is unhealthy.
    // Orchestrated hold authority routes through the validated grant (the `role` check).
    matches!(role, "product" | "advisor" | "operator") || is_tty
}

/// Dispatch authority permits routing already-disposed work without granting
/// the advisor's power to dispose it. Product, advisor, and integrator seats
/// may dispatch; a corroborated live orchestrator may continue its own routing.
/// Implementers and reviewers remain consumers of routed work.
// trace:STORY-1353 | ai:codex
pub(crate) fn dispatch_authority_from(role: &str, orchestrated: bool) -> bool {
    matches!(role, "product" | "advisor" | "integrator") || orchestrated
}

/// Integrity floors require a human who is present at an interactive terminal.
/// No workflow role or orchestrator corroboration can grant this authority.
// trace:STORY-1353 | ai:codex
pub(crate) fn integrity_floor_authority_from(human_present: bool) -> bool {
    human_present
}

// trace:STORY-1133 | ai:codex
pub(crate) fn current_role_instance_is_companion() -> bool {
    std::env::var("AIDA_ROLE_INSTANCE")
        .map(|v| v.eq_ignore_ascii_case("companion"))
        .unwrap_or(false)
}

/// STORY-646: a one-line clause for an advisor-authority refusal that names the
/// caller's *durable team role* when the roster supplies one. Empty when the
/// effective role came from the env/default (the refusal message already covers
/// that case). The guardrail-not-security caveat lives in `aida team set-role`,
/// not here. trace:STORY-646 | ai:claude
pub(crate) fn team_role_refusal_clause() -> String {
    let (role, source) = effective_role_with_roster();
    if source == team::RoleSource::Grant && role != "advisor" {
        format!(
            " Your active session seat is `{}` — ask an advisor to review the requested action.",
            role
        )
    } else {
        String::new()
    }
}

pub(crate) fn has_advisor_authority() -> bool {
    if current_role_instance_is_companion() {
        return false;
    }
    // BUG-460: a CLI op spawned by (or under) a live --auto-complete drain
    // inherits AIDA_AUTO_COMPLETE + the run token, so `orchestrator::detect`
    // corroborates it against the drain-state file. Grant it authority so
    // --no-human drains can auto-queue the review + let the phase children
    // mutate; a bare headless agent (no live orchestrator) stays gated.
    let orchestrated = find_main_worktree_root()
        .map(|root| {
            matches!(
                orchestrator::detect(&root),
                orchestrator::OrchestratorContext::Orchestrated
            )
        })
        .unwrap_or(false);
    // STORY-646: consult the durable per-user roster first so the guardrail
    // survives a forgotten `AIDA_SESSION_ROLE`. Non-rostered users / absent
    // store resolve identically to the pre-646 env-only behavior.
    advisor_authority_from(
        &effective_role_with_roster().0,
        authority_stdin_is_terminal(), // trace:BUG-1618 | ai:claude
        orchestrated,
    )
}

// trace:BUG-1793 | ai:antigravity
pub(crate) fn has_hold_authority() -> bool {
    if current_role_instance_is_companion() {
        return false;
    }
    let orchestrated = find_main_worktree_root()
        .map(|root| {
            matches!(
                orchestrator::detect(&root),
                orchestrator::OrchestratorContext::Orchestrated
            )
        })
        .unwrap_or(false);
    hold_authority_from(
        &effective_role_with_roster().0,
        authority_stdin_is_terminal(),
        orchestrated,
    )
}

// trace:STORY-1353 | ai:codex
pub(crate) fn has_dispatch_authority() -> bool {
    if current_role_instance_is_companion() {
        return false;
    }
    let orchestrated = find_main_worktree_root()
        .map(|root| {
            matches!(
                orchestrator::detect(&root),
                orchestrator::OrchestratorContext::Orchestrated
            )
        })
        .unwrap_or(false);
    dispatch_authority_from(&effective_role_with_roster().0, orchestrated)
}

// trace:STORY-1353 | ai:codex
pub(crate) fn has_integrity_floor_authority() -> bool {
    integrity_floor_authority_from(authority_stdin_is_terminal()) // trace:BUG-1618 | ai:claude
}
