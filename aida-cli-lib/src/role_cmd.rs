//! `aida role` command cluster — the role show/enter/end/list/add/repair/
//! delete/scaffold/scope/prompt handlers plus their role-command-exclusive
//! helpers (interactive picker entry, the eval-payload emitter).
//!
//! Extracted verbatim from main.rs (SPIKE-78 — pure movement, no behavior
//! change). Shared role machinery — the role-file layer (`load_role`,
//! `save_role_at`, `role_save_path`, `list_roles`, `parse_role_lenient`),
//! `resolve_role_name`, `active_role_scope`, `canonical_role_name`,
//! `record_role_activity`, the shared picker (`pick_role_with_header`,
//! `format_role_picker_option`, `RolePickerRow`), and `sh_single_quote` /
//! `eval_subcommand_hint` — stay in main.rs and are reached via `crate::`.

use crate::*;
use anyhow::Result;
use colored::Colorize;
use std::io::IsTerminal;

pub(crate) fn handle_role_command(cmd: &RoleCommand) -> Result<()> {
    let project_root = statusline_project_root();
    match cmd {
        RoleCommand::Enter { name, cd } => handle_role_enter(&project_root, name.as_deref(), *cd),
        RoleCommand::Add {
            name,
            purpose,
            global,
        } => handle_role_add(&project_root, name, purpose.as_deref(), *global),
        RoleCommand::List => handle_role_list(&project_root),
        RoleCommand::Show { name } => handle_role_show(&project_root, name.as_deref()),
        RoleCommand::Repair { name } => handle_role_repair(&project_root, name.as_deref()),
        RoleCommand::Active => handle_role_active(),
        RoleCommand::Current { check } => handle_role_current(*check),
        RoleCommand::End => handle_role_end(),
        RoleCommand::Delete { name, yes } => handle_role_delete(&project_root, name, *yes),
        RoleCommand::Scaffold => handle_role_scaffold(),
        RoleCommand::Scope(scope_cmd) => handle_role_scope(&project_root, scope_cmd),
        RoleCommand::Prompt(prompt_cmd) => handle_role_prompt(&project_root, prompt_cmd),
    }
}

// trace:TASK-1-022 | ai:claude
fn handle_role_prompt(project_root: &std::path::Path, cmd: &RolePromptCommand) -> Result<()> {
    match cmd {
        RolePromptCommand::Set {
            name,
            content,
            content_flag,
            stdin,
        } => {
            let role_name = resolve_role_name(name.as_deref())?;
            let (mut state, path) = load_role(project_root, &role_name)?;

            let text = if *stdin {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf.trim_end().to_string()
            } else if let Some(c) = content_flag.as_deref().or(content.as_deref()) {
                c.to_string()
            } else {
                anyhow::bail!(
                    "No content given. Pass it as a positional argument, via --content, \
                     or pipe with --stdin."
                );
            };

            if text.trim().is_empty() {
                anyhow::bail!(
                    "Empty addendum. Use `aida role prompt clear` to remove an existing one."
                );
            }
            state.system_prompt = Some(text);
            save_role_at(&state, &path)?;
            print_role_prompt(&state);
        }
        RolePromptCommand::Show { name } => {
            let role_name = resolve_role_name(name.as_deref())?;
            let (state, _) = load_role(project_root, &role_name)?;
            print_role_prompt(&state);
        }
        RolePromptCommand::Clear { name } => {
            let role_name = resolve_role_name(name.as_deref())?;
            let (mut state, path) = load_role(project_root, &role_name)?;
            state.system_prompt = None;
            save_role_at(&state, &path)?;
            println!("Role: {}", state.name.cyan());
            println!("  {}", "Addendum cleared.".dimmed());
        }
    }
    Ok(())
}

fn print_role_prompt(state: &RoleState) {
    println!("Role: {}", state.name.cyan());
    match &state.system_prompt {
        None => println!("  {}", "No system-prompt addendum set.".dimmed()),
        Some(text) => {
            println!("  {} ({} chars):", "Addendum".bold(), text.len());
            for line in text.lines() {
                println!("    {}", line);
            }
        }
    }
}

fn handle_role_scope(project_root: &std::path::Path, cmd: &RoleScopeCommand) -> Result<()> {
    match cmd {
        RoleScopeCommand::Set { name, tags, status } => {
            if tags.is_none() && status.is_none() {
                anyhow::bail!(
                    "At least one of --tags or --status is required.\n\
                     Use `aida role scope clear` to remove an existing scope."
                );
            }
            let role_name = resolve_role_name(name.as_deref())?;
            let (mut state, path) = load_role(project_root, &role_name)?;
            if let Some(t) = tags {
                state.scope_tags = t
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            if let Some(s) = status {
                state.scope_status = Some(s.clone());
            }
            save_role_at(&state, &path)?;
            print_role_scope(&state);
        }
        RoleScopeCommand::Show { name } => {
            let role_name = resolve_role_name(name.as_deref())?;
            let (state, _) = load_role(project_root, &role_name)?;
            print_role_scope(&state);
        }
        RoleScopeCommand::Clear { name, tags, status } => {
            let role_name = resolve_role_name(name.as_deref())?;
            let (mut state, path) = load_role(project_root, &role_name)?;
            // No flags = clear everything; otherwise clear only the specified field(s).
            let clear_all = !*tags && !*status;
            if clear_all || *tags {
                state.scope_tags.clear();
            }
            if clear_all || *status {
                state.scope_status = None;
            }
            save_role_at(&state, &path)?;
            print_role_scope(&state);
        }
    }
    Ok(())
}

fn print_role_scope(state: &RoleState) {
    println!("{} {}", "Role:".bold(), state.name.cyan());
    if state.scope_tags.is_empty() && state.scope_status.is_none() {
        println!("  {}", "No scope filters set.".dimmed());
        return;
    }
    if !state.scope_tags.is_empty() {
        println!("  {}: {}", "tags".bold(), state.scope_tags.join(", "));
    }
    if let Some(s) = &state.scope_status {
        println!("  {}: {}", "status".bold(), s);
    }
}

fn handle_role_enter(project_root: &std::path::Path, name: Option<&str>, cd: bool) -> Result<()> {
    // TASK-644: resolve the role name. When the name is omitted, or names a
    // role that doesn't exist, fall back to an interactive picker — but ONLY
    // when stdin is a TTY. The primary caller is `eval "$(aida role enter)"`,
    // so stdout is a captured pipe; gate on stdin, draw to /dev/tty (stderr
    // fallback), and keep stdout carrying nothing but the eval payload.
    let resolved = match name {
        Some(n) if load_role(project_root, n).is_ok() => n.to_string(),
        explicit => {
            if std::io::stdin().is_terminal() {
                match pick_role_interactively(project_root, explicit)? {
                    Some(chosen) => chosen,
                    None => return Ok(()), // cancelled — stdout stays empty, eval is a no-op
                }
            } else {
                // Non-interactive: preserve the deterministic error + exit code.
                let n = explicit.unwrap_or("");
                if n.is_empty() {
                    anyhow::bail!(
                        "No role name given and stdin is not a terminal.\n\
                         Pass a name (`aida role enter <name>`) or run interactively.\n\
                         See available roles with: `aida role list`"
                    );
                }
                anyhow::bail!(
                    "No such role: {}\n\
                     Create it with: `aida role add {}`\n\
                     See available roles with: `aida role list`",
                    n,
                    n
                );
            }
        }
    };
    let (mut state, _) = load_role(project_root, &resolved).map_err(|_| {
        anyhow::anyhow!(
            "No such role: {}\n\
             Create it with: `aida role add {}`\n\
             See available roles with: `aida role list`",
            resolved,
            resolved
        )
    })?;
    state.last_active_at = chrono::Utc::now();
    state.working_directory = std::env::current_dir().ok();
    let save_path = role_save_path(project_root, &state)?;
    save_role_at(&state, &save_path)?;
    // STORY-768: entering the advisor seat under tmux registers this pane so
    // `aida human audit --inject` can send-keys the reconcile pass here even
    // when the advisor is idle. Idempotent; a no-op (and never an error) when
    // TMUX_PANE is unset, so a non-tmux `role enter advisor` is unaffected.
    // trace:STORY-768 | ai:claude
    if canonical_role_name(&resolved) == "advisor" {
        let _ = human_audit::register_pane_from_env(project_root);
    }
    emit_role_enter_eval(project_root, &state, cd, /* was_existing */ true);
    Ok(())
}

/// TASK-644: interactive role picker for `aida role enter` (no name / unknown
/// name on a TTY). Renders the list and reads the selection over `/dev/tty`
/// (falling back to stderr+stdin) so stdout stays clean for the eval payload.
/// Returns `Ok(Some(name))` on selection, `Ok(None)` if the user cancels.
fn pick_role_interactively(
    project_root: &std::path::Path,
    unknown: Option<&str>,
) -> Result<Option<String>> {
    let header = match unknown.filter(|u| !u.is_empty()) {
        Some(u) => format!("No such role: {} — pick one of the existing roles:", u),
        None => "Select a role to enter:".to_string(),
    };
    // role-enter marks the *active* shell role; no spawn-default to highlight.
    pick_role_with_header(project_root, &header, None)
}

fn handle_role_add(
    project_root: &std::path::Path,
    name: &str,
    purpose: Option<&str>,
    global: bool,
) -> Result<()> {
    if let Ok((existing, path)) = load_role(project_root, name) {
        // trace:TASK-667 — emit the wrapper-correct enter form.
        anyhow::bail!(
            "Role '{}' already exists at {}.\n\
             Resume it with: `{}`\n\
             See its details with: `aida role show {}`\n\
             ({})",
            name,
            path.display(),
            eval_subcommand_hint(&format!("role enter {name}")),
            name,
            if existing.global {
                "currently a global role"
            } else {
                "currently a project role"
            }
        );
    }
    let state = RoleState {
        name: name.to_string(),
        purpose: purpose.map(String::from),
        created_at: chrono::Utc::now(),
        last_active_at: chrono::Utc::now(),
        working_directory: std::env::current_dir().ok(),
        notes: None,
        global,
        activity: Vec::new(),
        scope_tags: Vec::new(),
        scope_status: None,
        system_prompt: None,
    };
    let save_path = role_save_path(project_root, &state)?;
    save_role_at(&state, &save_path)?;
    emit_role_enter_eval(
        project_root,
        &state,
        /* cd */ false,
        /* was_existing */ false,
    );
    Ok(())
}

fn emit_role_enter_eval(
    project_root: &std::path::Path,
    state: &RoleState,
    cd: bool,
    was_existing: bool,
) {
    // Emit shell code for eval. The `aida()` shell wrapper installed by
    // `aida dev shell-init --install` automatically eval's our stdout for
    // role enter / dev activate / etc., so direct invocation Just Works in
    // an interactive shell. Scripts can still pipe to `eval "$(...)"`.
    let cwd = state
        .working_directory
        .as_deref()
        .unwrap_or(project_root)
        .display();
    println!("# aida role enter — {}", state.name);
    // Strip ALL `(role:NAME) ` prefixes from PS1, regardless of which role
    // is currently in AIDA_SESSION_ROLE. The earlier single-pattern strip
    // (keyed off AIDA_SESSION_ROLE) leaked prefixes whenever the env var
    // went stale: a subshell that didn't inherit the var, a manual unset,
    // or a `role end` that fired without `role enter` having matched —
    // each leaves an orphan `(role:foo) ` in PS1 that the next `role enter`
    // wouldn't see. The loop walks PS1, extracts each `(role:NAME)` token,
    // and strips it globally. trace:BUG-60 | ai:claude
    println!("if [ -n \"${{PS1+x}}\" ]; then");
    println!("    while case \"$PS1\" in *'(role:'*') '*) true;; *) false;; esac; do");
    println!("        _aida_old_ps1=\"$PS1\"");
    println!("        _aida_after=\"${{PS1#*'(role:'}}\"");
    println!("        _aida_name=\"${{_aida_after%%') '*}}\"");
    println!("        PS1=\"${{PS1//'(role:'$_aida_name') '/}}\"");
    println!("        [ \"$PS1\" = \"$_aida_old_ps1\" ] && break");
    println!("    done");
    println!("    unset _aida_old_ps1 _aida_after _aida_name");
    println!("fi");
    println!("export AIDA_SESSION_ROLE='{}'", state.name);
    if let Some(p) = &state.purpose {
        println!("export AIDA_SESSION_PURPOSE='{}'", sh_single_quote(p));
    } else {
        println!("unset AIDA_SESSION_PURPOSE");
    }
    println!("export AIDA_SESSION_PROJECT='{}'", project_root.display());
    println!("if [ -n \"${{PS1+x}}\" ]; then");
    println!("    export PS1=\"(role:{}) $PS1\"", state.name);
    println!("fi");
    if cd {
        println!("cd '{}'", cwd);
    }
    let verb = if was_existing {
        "Resumed"
    } else {
        "Created and entered"
    };
    let scope = if state.global { " [global]" } else { "" };
    // Escape name + purpose: both are free-text and the echo is eval'd.
    // verb + scope are fixed literals. trace:BUG-427 | ai:claude
    let purpose_suffix = state
        .purpose
        .as_ref()
        .map(|p| format!(" — {}", p))
        .unwrap_or_default();
    println!(
        "echo '{} {} role: {}{}{}'",
        crate::glyph(crate::glyphs::Glyph::Check),
        verb,
        sh_single_quote(&state.name),
        scope,
        sh_single_quote(&purpose_suffix)
    );
    // TASK-48 / TASK-49: load store + queue once (best-effort) so both
    // sections below can resolve spec_id → title and surface the
    // role's queue head. Failures are silent — falls back to the
    // legacy minimal rendering. trace:TASK-48 | ai:claude
    //
    // BUG-83: the lookup also carries a preferred display id
    // (agreed_id when assigned, else spec_id) so the "Queued for
    // this role" and "Last touched" sections render the short form
    // once `aida db merge-gate` has run. The map is keyed by both
    // spec_id AND agreed_id so an activity-log entry recorded under
    // either form resolves to the same row.
    // trace:BUG-83 | ai:claude
    let store_path = project_root.join(".aida-store");
    let (lookup, queue_for_role): (
        std::collections::HashMap<String, (String, String)>,
        Vec<aida_core::QueueEntry>,
    ) = if store_path.exists() {
        let storage = Storage::new(store_path);
        let lookup = storage
            .load()
            .map(|s| {
                let mut map: std::collections::HashMap<String, (String, String)> =
                    std::collections::HashMap::new();
                for r in &s.requirements {
                    let display = r.display_id();
                    let title = r.title.clone();
                    if let Some(sid) = r.spec_id.as_deref() {
                        map.insert(sid.to_string(), (title.clone(), display.clone()));
                    }
                    if let Some(aid) = r.agreed_id.as_deref() {
                        map.entry(aid.to_string())
                            .or_insert((title.clone(), display.clone()));
                    }
                }
                map
            })
            .unwrap_or_default();
        // BUG-89: route through the canonical helper so the role-show
        // queue head matches what `aida queue list` would show in the
        // same shell (previously this path skipped the USERNAME
        // fallback). trace:BUG-89 | ai:claude
        let user_id = current_user_id(None);
        let queue_for_role = storage
            .queue_list(&user_id, /* include_completed */ false)
            .map(|entries| {
                entries
                    .into_iter()
                    // TASK-586: a `dialog`-routed item matches the advisor role.
                    .filter(|e| {
                        e.for_role.as_deref().map(canonical_role_name).as_deref()
                            == Some(state.name.as_str())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        (lookup, queue_for_role)
    } else {
        (Default::default(), Vec::new())
    };

    // TASK-48: queue head, capped at 5, with a "+N more" hint when the
    // role queue is deeper. Empty case still renders so the absence
    // itself is signal. trace:TASK-48 | ai:claude
    if was_existing {
        let total = queue_for_role.len();
        let show_n = total.min(5);
        if total == 0 {
            println!("echo ''");
            println!("echo '  Queued for this role: (empty)'");
        } else {
            println!("echo ''");
            println!("echo '  Queued for this role ({}):'", total);
            // Load store once more to look up reqs by uuid for title
            // resolution. The queue entry only stores uuid; we need the
            // spec_id + title from the store.
            let store_snapshot = if !queue_for_role.is_empty() {
                let storage = Storage::new(project_root.join(".aida-store"));
                storage.load().ok()
            } else {
                None
            };
            for entry in queue_for_role.iter().take(show_n) {
                // BUG-83: prefer agreed_id (short merge-gated id) over
                // spec_id so this section stops drifting from `aida queue
                // list` / `aida queue next`. trace:BUG-83 | ai:claude
                let (display_id, title) = store_snapshot
                    .as_ref()
                    .and_then(|s| s.requirements.iter().find(|r| r.id == entry.requirement_id))
                    .map(|r| (r.display_id(), truncate(&r.title, 60).to_string()))
                    .unwrap_or_else(|| ("(deleted)".into(), String::new()));
                println!("echo '    {:<12} {}'", display_id, sh_single_quote(&title));
            }
            if total > show_n {
                let remaining = total - show_n;
                println!(
                    "echo '    … and {} more (run `aida queue list`)'",
                    remaining
                );
            }
        }
    }

    // Surface what the user was last working on under this role —
    // makes "resume" feel like a real session, not just a label switch.
    // TASK-49: add titles alongside spec_ids in the activity rows.
    // trace:TASK-49 | ai:claude
    if was_existing && !state.activity.is_empty() {
        println!("echo ''");
        println!("echo '  Last touched while in this role:'");
        for entry in state.activity.iter().take(5) {
            let when = humanize_relative(entry.at);
            // BUG-83: resolve the activity-log key (recorded as either
            // spec_id or agreed_id form depending on what the user typed
            // when the event fired) to the requirement's current preferred
            // display id, so merge-gated rows show as the short form here
            // too. Falls back to the raw stored id if the requirement is
            // gone from the store. trace:BUG-83 | ai:claude
            let (title, display_id) = match lookup.get(&entry.spec_id) {
                Some((t, d)) => (truncate(t, 60).to_string(), d.clone()),
                None => (String::new(), entry.spec_id.clone()),
            };
            // Match the queue-head column widths so the two sections
            // line up: 12-wide id, then title, then action+time.
            if title.is_empty() {
                println!("echo '    {:<12} {} ({})'", display_id, entry.action, when);
            } else {
                println!(
                    "echo '    {:<12} {:<60} {} ({})'",
                    display_id,
                    sh_single_quote(&title),
                    entry.action,
                    when
                );
            }
        }
    }
}

/// `aida role active` — one-line stub that prints just the active role
/// name, scriptable counterpart to `git branch --show-current` and
/// `git config --get user.email`. Pure read of `$AIDA_SESSION_ROLE` so
/// it never loads the project store; exits 1 with empty stdout when no
/// role is active so shell guards like `[ -n "$(aida role active)" ]`
// work without parsing. trace:TASK-42 | ai:claude
fn handle_role_active() -> Result<()> {
    match std::env::var("AIDA_SESSION_ROLE") {
        Ok(role) if !role.is_empty() => {
            println!("{}", role);
            Ok(())
        }
        _ => std::process::exit(1),
    }
}

// `aida role current` — print the active role's name (empty line when no
// role is active) and exit 0 either way. `--check` exits 1 instead when no
// role is active. Pure read of `$AIDA_SESSION_ROLE`, no project-store load.
// Distinct from `role active` (TASK-42), which exits 1 on no-role with no
// trailing newline — `current` always prints a line so a scripting caller
// can capture the value and branch on exit code separately.
// trace:STORY-64 | ai:claude
fn handle_role_current(check: bool) -> Result<()> {
    let role = std::env::var("AIDA_SESSION_ROLE").unwrap_or_default();
    println!("{}", role);
    if check && role.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

fn handle_role_end() -> Result<()> {
    // Use a uniquely-named env var rather than `local` so the eval works
    // both at the shell top level and inside a wrapper function.
    println!("# aida role end");
    println!("__AIDA_ROLE_END_PREV=\"${{AIDA_SESSION_ROLE:-}}\"");
    // Strip ALL `(role:NAME) ` prefixes from PS1 — same rationale as the
    // entry path: stale env state can leave orphan prefixes that a single
    // pattern strip misses. trace:BUG-60 | ai:claude
    println!("if [ -n \"${{PS1+x}}\" ]; then");
    println!("    while case \"$PS1\" in *'(role:'*') '*) true;; *) false;; esac; do");
    println!("        _aida_old_ps1=\"$PS1\"");
    println!("        _aida_after=\"${{PS1#*'(role:'}}\"");
    println!("        _aida_name=\"${{_aida_after%%') '*}}\"");
    println!("        PS1=\"${{PS1//'(role:'$_aida_name') '/}}\"");
    println!("        [ \"$PS1\" = \"$_aida_old_ps1\" ] && break");
    println!("    done");
    println!("    unset _aida_old_ps1 _aida_after _aida_name");
    println!("fi");
    println!("unset AIDA_SESSION_ROLE AIDA_SESSION_PURPOSE AIDA_SESSION_PROJECT");
    println!("if [ -n \"$__AIDA_ROLE_END_PREV\" ]; then");
    println!(
        "    echo \"{} Deactivated role: $__AIDA_ROLE_END_PREV\"",
        crate::glyph(crate::glyphs::Glyph::Check)
    );
    println!("else");
    println!("    echo 'No role active.'");
    println!("fi");
    println!("unset __AIDA_ROLE_END_PREV");
    Ok(())
}

fn handle_role_list(project_root: &std::path::Path) -> Result<()> {
    let roles = list_roles(project_root)?;
    let active = std::env::var("AIDA_SESSION_ROLE").ok();
    if roles.is_empty() {
        println!("(no roles defined for {})", project_root.display());
        println!(
            "Create one with: {} {}",
            "aida role add".cyan(),
            "<name>".dimmed()
        );
        println!("Or install a starter set: {}", "aida role scaffold".cyan());
        return Ok(());
    }
    println!("Roles for {}:", project_root.display());
    for role in &roles {
        let marker = if active.as_deref() == Some(&role.name) {
            "*".green().to_string()
        } else {
            " ".to_string()
        };
        let scope = if role.global {
            " [global]".dimmed().to_string()
        } else {
            String::new()
        };
        let last = humanize_relative(role.last_active_at);
        let purpose = role
            .purpose
            .as_deref()
            .map(|p| format!(" — {}", p))
            .unwrap_or_default();
        // TASK-586: `advisor` is the canonical name now (roles are
        // canonicalized on load), so the bare name is the identity.
        let display_name = role.name.clone();
        println!(
            "  {} {:<16}{} last active {}{}",
            marker,
            display_name.bold(),
            scope,
            last,
            purpose
        );
    }
    Ok(())
}

fn handle_role_show(project_root: &std::path::Path, name: Option<&str>) -> Result<()> {
    let resolved = match name {
        Some(n) => n.to_string(),
        None => std::env::var("AIDA_SESSION_ROLE").map_err(|_| {
            anyhow::anyhow!(
                "No role active and no name given. Use `aida role list` to see options."
            )
        })?,
    };
    // BUG-228: lenient load — a corrupted activity entry is reported as a
    // warning and the rest of the role still renders, rather than the whole
    // command fail-stopping on an unparseable file.
    let (state, path, warnings) = load_role_with_warnings(project_root, &resolved)?;
    println!(
        "Role:        {}{}",
        state.name.bold(),
        if state.global {
            " [global]".dimmed().to_string()
        } else {
            String::new()
        }
    );
    // TASK-586: `state.name` is canonicalized on load, so the role's name
    // is its identity now — no separate user-facing-identity line needed.
    println!("Stored at:   {}", path.display());
    println!(
        "Purpose:     {}",
        state.purpose.as_deref().unwrap_or("(none)")
    );
    println!("Created:     {}", state.created_at.to_rfc3339());
    println!(
        "Last active: {} ({})",
        state.last_active_at.to_rfc3339(),
        humanize_relative(state.last_active_at)
    );
    if let Some(d) = &state.working_directory {
        println!("Last cwd:    {}", d.display());
    }
    if let Some(n) = &state.notes {
        println!("Notes:       {}", n);
    }
    // trace:TASK-1-021 | ai:claude
    if !state.scope_tags.is_empty() || state.scope_status.is_some() {
        let mut parts: Vec<String> = Vec::new();
        if !state.scope_tags.is_empty() {
            parts.push(format!("tags={}", state.scope_tags.join(",")));
        }
        if let Some(s) = &state.scope_status {
            parts.push(format!("status={}", s));
        }
        println!("Scope:       {}", parts.join(" "));
    }
    // trace:TASK-1-022 | ai:claude
    if let Some(text) = &state.system_prompt {
        let preview: String = text.lines().next().unwrap_or("").chars().take(80).collect();
        let suffix = if text.len() > preview.len() {
            "…"
        } else {
            ""
        };
        println!("Addendum:    {} chars — {}{}", text.len(), preview, suffix);
    }
    if !state.activity.is_empty() {
        println!();
        println!("Recent activity (newest first):");
        // BUG-83: resolve each log entry's stored id to the requirement's
        // preferred display id (agreed_id when assigned) so this section
        // matches `aida role enter`'s "Last touched" view.
        // trace:BUG-83 | ai:claude
        let display_lookup = build_display_id_lookup(project_root);
        for entry in &state.activity {
            let display_id = display_lookup
                .get(&entry.spec_id)
                .cloned()
                .unwrap_or_else(|| entry.spec_id.clone());
            println!(
                "  {:<14} {:<10} {}",
                display_id,
                entry.action,
                humanize_relative(entry.at)
            );
        }
    }
    // BUG-228: surface any salvaged corruption instead of fail-stopping.
    if !warnings.is_empty() {
        eprintln!();
        for w in &warnings {
            eprintln!("{}: role activity log — {}", "Warning".yellow(), w);
        }
        eprintln!(
            "Run `{}` to quarantine the corrupted fragment(s) and rewrite the file cleanly.",
            format!("aida role repair {}", resolved).cyan()
        );
    }
    Ok(())
}

/// Repair a corrupted role file (BUG-228). Quarantines any unparseable
/// activity-log entries, preserves the header and every well-formed entry,
/// backs the original up, and rewrites the file cleanly. A healthy file is
// left untouched. trace:BUG-228 | ai:claude
fn handle_role_repair(project_root: &std::path::Path, name: Option<&str>) -> Result<()> {
    let resolved = resolve_role_name(name)?;
    // Resolve the on-disk path the way load_role does, but read the raw
    // bytes so we can salvage a file the strict parser rejects outright.
    let path = {
        let project_path = project_role_file(project_root, &resolved);
        if project_path.exists() {
            project_path
        } else {
            global_role_file(&resolved)
                .filter(|g| g.exists())
                .ok_or_else(|| anyhow::anyhow!("No such role: {}", resolved))?
        }
    };
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read role file {}", path.display()))?;

    if toml::from_str::<RoleState>(&content).is_ok() {
        println!(
            "{}: role file {} is healthy — nothing to repair.",
            "OK".green(),
            path.display()
        );
        return Ok(());
    }

    let (state, warnings) = parse_role_lenient(&content).with_context(|| {
        format!(
            "role file {} is corrupted in its header, beyond the activity log — \
             auto-repair can't recover it; inspect the file by hand",
            path.display()
        )
    })?;

    // Back the original up before rewriting so nothing is lost.
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let backup = path.with_extension(format!("toml.corrupt-{}", stamp));
    std::fs::copy(&path, &backup)
        .with_context(|| format!("Failed to back up to {}", backup.display()))?;

    save_role_at(&state, &path)?;

    println!("{}: repaired {}", "OK".green(), path.display());
    println!("  Original backed up to {}", backup.display());
    if warnings.is_empty() {
        println!("  No activity entries needed quarantining (corruption was structural).");
    } else {
        println!("  Quarantined {} corrupt fragment(s):", warnings.len());
        for w in &warnings {
            println!("    - {}", w);
        }
    }
    println!(
        "  Kept {} valid activity entr{}.",
        state.activity.len(),
        if state.activity.len() == 1 {
            "y"
        } else {
            "ies"
        }
    );
    Ok(())
}

fn handle_role_delete(project_root: &std::path::Path, name: &str, yes: bool) -> Result<()> {
    let (state, path) = load_role(project_root, name)?;
    let scope = if state.global { " [global]" } else { "" };
    if !yes {
        // BUG-671: deleting a role removes the file — genuinely destructive, so
        // (unlike the reversible `queue done`) we DON'T auto-confirm. But never
        // silently cancel a non-interactive write either: with no human to
        // answer the prompt, FAIL LOUDLY with a machine-actionable error naming
        // the override flag so an agent can self-correct instead of believing
        // the delete happened. trace:BUG-671 | ai:claude
        if non_interactive_confirm() {
            anyhow::bail!(
                "role delete needs confirmation in non-interactive mode — re-run with -y to \
                 delete role '{}'.",
                name
            );
        }
        eprintln!(
            "Delete role '{}{}'? (purpose: {}, last active: {})",
            name,
            scope,
            state.purpose.as_deref().unwrap_or("(none)"),
            humanize_relative(state.last_active_at)
        );
        eprintln!("Type 'y' to confirm:");
        let mut answer = String::new();
        if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
            eprintln!("Cancelled.");
            return Ok(());
        }
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            eprintln!("Cancelled.");
            return Ok(());
        }
    }
    std::fs::remove_file(&path)?;
    println!(
        "{}: deleted role '{}' ({})",
        "OK".green(),
        name,
        path.display()
    );
    Ok(())
}

fn handle_role_scaffold() -> Result<()> {
    let project_root = statusline_project_root();
    println!("Installing starter global roles at ~/.aida/roles/");
    println!();
    let (created, skipped) = scaffold_starter_roles(&project_root)?;
    // Report per role in STARTER_ROLES order (created vs already-present).
    for (name, purpose) in STARTER_ROLES {
        if created.contains(name) {
            println!("  {} {} — {}", "+".green(), name, purpose);
        } else {
            println!("  {} {} (already exists, skipped)", "~".yellow(), name);
        }
    }
    println!();
    if created.is_empty() {
        println!(
            "{}: all {} starter role(s) already exist — nothing to do.",
            "OK".green(),
            skipped.len()
        );
    } else {
        println!(
            "{}: scaffolded {} role(s){}.",
            "OK".green(),
            created.len(),
            if !skipped.is_empty() {
                format!(" ({} already existed)", skipped.len())
            } else {
                String::new()
            }
        );
        println!();
        // trace:TASK-667 — wrapper-correct enter form.
        println!(
            "Try them: {}",
            eval_subcommand_hint("role enter implementer").cyan()
        );
        println!("List all: {}", "aida role list".cyan());
    }
    Ok(())
}
