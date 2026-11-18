pub fn resume(id: Option<String>, limit: usize) -> Result<()> {
    resume_with(&headless_worktree_root(), || match id {
        Some(prefix) => resolve_resume_target(&prefix),
        None => pick_interactive(limit),
    })
}

/// [`resume`] with the project root and the target lookup supplied, so a test
/// can drive the containment decision without a real session store.
// trace:BUG-2393 | ai:claude
pub(crate) fn resume_with(
    project_root: &Path,
    resolve_target: impl FnOnce() -> Result<ResumeTarget>,
) -> Result<()> {
    // BUG-2157: a contained parent (strict parse, so a typo refuses) resumes
    // contained. Read before the picker so a refusal precedes any interaction.
    // BUG-2393: configured containment counts too, not only the env.
    // trace:BUG-2157 BUG-2393 | ai:claude
    let parent_contained = drain_contained(project_root)?;
    let target = resolve_target()?;
    if parent_contained {
        let cwd = std::env::current_dir().context("failed to read the current directory")?;
        let mut cmd = build_contained_resume_command(&target, &cwd)?;
        return crate::launch::spawn::exec_agent(&mut cmd);
    }
    exec_resume_command(&target, None, false)
}

/// BUG-2157 (ruling on BUG-2151: a contained parent's env ALWAYS wins). The
/// `aida session resume` command for a contained parent: a claude session
/// resumes AIDA-contained, a codex session under Codex's native sandbox
/// (BUG-2386), and agy is refused — never resumed uncontained. A claude
/// resume keeps the contained flags and `AIDA_CONTAINED=1` on the child. It runs in the session's lease tree when a host lease
/// minted it (TASK-2031), else in the checkout whose Claude project store
/// listed it, and that tree's git root is the write boundary. Claude keys
/// transcripts by cwd, so a lease-tree resume first copies the one transcript
/// into the lease tree's project store ([`stage_resume_transcript`],
/// TASK-2112). `launcher_cwd` is the fallback only when the target has no
/// checkout recorded or it is gone. No git root means no write roots (fail
/// closed).
// trace:BUG-2157 | ai:claude
// trace:TASK-1952 | trace:TASK-2112 | ai:claude
pub(crate) fn build_contained_resume_command(
    target: &ResumeTarget,
    launcher_cwd: &Path,
) -> Result<std::process::Command> {
    // TASK-2032: the host main this resume is anchored to, resolved from the
    // launcher's own directory (never from the target's recorded checkout).
    // trace:TASK-2032 | ai:claude
    let host_main = crate::seat_clone::host_main_from(launcher_cwd);
    // BUG-2386: a Codex session resumes contained under Codex's own native
    // sandbox when `[contained.vendors]` opts it in, else is refused — never
    // resumed uncontained. trace:BUG-2386 | ai:claude
    if target.agent == "codex" {
        require_vendor_containment(HeadlessVendor::Codex, true, "resumed codex session")
            .with_context(|| {
                format!(
                    "refusing to resume codex session `{}` uncontained",
                    target.id
                )
            })?;
        let mut cmd = codex_session_resume_command(host_main.as_deref(), target, true)?;
        if cmd.get_current_dir().is_none() {
            cmd.current_dir(launcher_cwd);
        }
        return Ok(cmd);
    }
    if target.agent != "claude" {
        anyhow::bail!(
            "[contained] this session is contained (AIDA_CONTAINED), but `{}` sessions cannot \
             run AIDA-contained — refusing to resume `{}` uncontained. Resume it from an \
             uncontained shell.",
            target.agent,
            target.id
        );
    }
    // trace:BUG-2131 | ai:claude
    aida_core::git_ops::ensure_git_supports_contained()?;
    // TASK-2031: a session minted by a lease resumes in that lease's tree
    // (cwd and write boundary both), not in the main checkout whose store
    // listed it: a cwd of main would leave main inside Claude Code's own
    // working-directory permissions.
    // trace:TASK-1952 | trace:TASK-2031 | ai:claude
    let lease_tree = resume_lease_tree(host_main.as_deref(), target);
    if let (Some(tree), Some(listed_in)) = (&lease_tree, &target.worktree) {
        stage_resume_transcript(&target.id, listed_in, tree)?;
    }
    let worktree = lease_tree
        .or_else(|| target.worktree.clone().filter(|p| p.is_dir()))
        .unwrap_or_else(|| launcher_cwd.to_path_buf());
    prepare_contained_mailbox_for_launch(host_main.as_deref(), &worktree)?;
    let mut cmd = std::process::Command::new("claude");
    cmd.args(resume_command_for(target).1);
    cmd.args(claude_contained_flags_for_roots(
        host_main.as_deref(),
        &contained_session_write_roots_from(&worktree),
    )?);
    cmd.env("AIDA_CONTAINED", "1");
    // trace:TASK-2631 | ai:codex — do not inherit a native parent's display marker.
    cmd.env("AIDA_CONTAINMENT_POSTURE", "aida");
    cmd.current_dir(&worktree);
    Ok(cmd)
}

/// TASK-2112: make the transcript of Claude session `id`, listed from the
/// project store of `listed_in`, visible to `claude --resume` run from
/// `cwd`. Claude keys transcripts by cwd, so a resume from the lease tree
/// would not find a transcript stored under the main checkout.
///
/// Copies only `<id>.jsonl`, as a regular file owned by this user, never
/// following or writing through a symlink, created mode 0600. An existing
/// destination with the same bytes is reused; one with different bytes
/// refuses the resume rather than being overwritten. Nothing to copy (no
/// source, or both directories are one store) is not an error: `--resume`
/// then reports a missing session itself.
// trace:TASK-1952 | trace:TASK-2112 | ai:claude
pub(crate) fn stage_resume_transcript(id: &str, listed_in: &Path, cwd: &Path) -> Result<()> {
    let valid_id = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    anyhow::ensure!(valid_id, "refusing to resume `{id}`: not a session id");
    let from_dir = claude_project_dir(listed_in)?;
    let to_dir = claude_project_dir(cwd)?;
    if from_dir == to_dir {
        return Ok(());
    }
    let name = format!("{id}.jsonl");
    let source = from_dir.join(&name);
    let refuse = |why: &str| {
        anyhow::anyhow!(
            "[contained] refusing to resume `{id}` in {}: {why}",
            cwd.display()
        )
    };
    // TASK-2315: open the source O_NOFOLLOW and fstat the HANDLE, so the
    // checks and the read share one file description and a symlink swapped
    // in between them is never followed. O_NONBLOCK so a planted FIFO cannot
    // park the open until a writer appears (TASK-2321): harmless for a
    // regular file, and the fstat below refuses anything that is not one.
    // trace:TASK-2272 | ai:claude (TASK-2315)
    // trace:TASK-2321 | ai:claude
    let mut source_options = std::fs::OpenOptions::new();
    source_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        source_options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut source_file = match source_options.open(&source) {
        Ok(f) => f,
        // No source to copy is not an error: `--resume` reports it itself.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        // O_NOFOLLOW refuses a symlink with ELOOP.
        #[cfg(unix)]
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(refuse("its transcript is not a regular file"));
        }
        Err(e) => {
            return Err(e).with_context(|| format!("reading transcript {}", source.display()));
        }
    };
    let meta = source_file
        .metadata()
        .with_context(|| format!("reading transcript {}", source.display()))?;
    if !meta.file_type().is_file() {
        return Err(refuse("its transcript is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and cannot fail.
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err(refuse("its transcript belongs to another user"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // The file is a confirmed regular file; drop O_NONBLOCK before the
        // read so the handle carries no surprising mode. trace:TASK-2321 | ai:claude
        // SAFETY: fcntl F_GETFL/F_SETFL on an fd this function owns and keeps open.
        unsafe {
            let flags = libc::fcntl(source_file.as_raw_fd(), libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(
                    source_file.as_raw_fd(),
                    libc::F_SETFL,
                    flags & !libc::O_NONBLOCK,
                );
            }
        }
    }
    let mut bytes = Vec::new();
    {
        use std::io::Read;
        source_file
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading transcript {}", source.display()))?;
    }
    if std::fs::symlink_metadata(&to_dir).is_ok_and(|m| !m.file_type().is_dir()) {
        return Err(refuse(
            "the lease tree's Claude project directory is not a directory",
        ));
    }
    std::fs::create_dir_all(&to_dir).with_context(|| format!("creating {}", to_dir.display()))?;
    let dest = to_dir.join(&name);
    if let Ok(existing) = std::fs::symlink_metadata(&dest) {
        if !existing.file_type().is_file() {
            return Err(refuse(
                "a transcript in the lease tree's store is not a regular file",
            ));
        }
        if std::fs::read(&dest).ok().as_deref() == Some(bytes.as_slice()) {
            return Ok(());
        }
        return Err(refuse(
            "the lease tree's store already holds a different transcript for it",
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    use std::io::Write;
    let mut file = options
        .open(&dest)
        .with_context(|| format!("creating {}", dest.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-159 | ai:codex
pub(crate) fn interactive_session_vendor() -> Result<String> {
    let root = crate::find_main_worktree_root().unwrap_or(std::env::current_dir()?);
    Ok(aida_core::agents_config::resolve_default_vendor(&root)
        .unwrap_or_else(|| "claude".to_string()))
}

// Validate the actual vendor and trusted posture before prompts, launch logs,
// worktrees or leases. The tracked launcher repeats its own ordinary guards.
// trace:BUG-b6b1278890774ef993d2ebc327862470-159 | ai:codex
pub(crate) fn preflight_interactive_session(
    vendor: &str,
    permission_mode: Option<&str>,
    contained: bool,
) -> Result<()> {
    anyhow::ensure!(
        matches!(vendor, "claude" | "codex"),
        "unsupported interactive session vendor `{vendor}`"
    );
    anyhow::ensure!(
        crate::find_executable_on_path(vendor).is_some(),
        "`{vendor}` is not on PATH — install the configured vendor before launching"
    );
    let root = crate::find_main_worktree_root().unwrap_or(std::env::current_dir()?);
    crate::enforce_agent_type_enabled(&root, vendor)?;
    if contained {
        containment_posture_in(&root, vendor, true)?;
    }
    if vendor == "claude" {
        return refuse_uncontained_session_under_contained_parent(permission_mode, contained);
    }
    anyhow::ensure!(
        permission_mode.is_none(),
        "--permission-mode is only supported by claude"
    );
    let config = codex_interactive_session_config(contained)?;
    crate::refuse_uncontained_child_of_contained_parent(&config)
}

/// Refuse an explicit `--name` that cannot become a Codex agent handle, before
/// any worktree or lease exists (the tracked launcher would refuse it later).
// trace:BUG-b6b1278890774ef993d2ebc327862470-159 | ai:claude
pub(crate) fn preflight_codex_agent_handle(name: &str) -> Result<()> {
    let root = crate::find_main_worktree_root().unwrap_or(std::env::current_dir()?);
    let reserved = crate::agent_handle::reserved_addresses(&root);
    crate::agent_handle::validate_explicit_handle(name, &reserved)
}

// Use the existing sanctioned Codex flags and trusted native opt-in; never
// interpret native as AIDA containment or widen the child's write boundary.
// trace:BUG-b6b1278890774ef993d2ebc327862470-159 | ai:codex
fn codex_interactive_session_config(contained: bool) -> Result<crate::AgentLaunchConfig> {
    let root = crate::find_main_worktree_root().unwrap_or(std::env::current_dir()?);
    let mut config = crate::AgentLaunchConfig {
        delegate_seats: Vec::new(),
        agent_type: "codex",
        binary: "codex",
        default_args: vec![crate::CODEX_NO_DAEMON_FLAG.to_string()],
        prompt_style: crate::AgentPromptStyle::Positional,
        contained: false,
    };
    if contained {
        containment_posture_in(&root, "codex", true)?;
        config
            .default_args
            // No host main is pinned on this path (it may run after a
            // chdir into the seat), so a seat clone is not recognised and
            // its write set fails closed (TASK-2032).
            // trace:TASK-2032 | ai:claude
            .extend(crate::tool_contained_flags(None, "codex")?);
    }
    crate::apply_agent_default_flags(
        &mut config,
        &root,
        crate::AgentDefaultFlagOptions::new(true, Vec::new(), None),
        contained,
    )?;
    Ok(config)
}

/// The child role a `session start --launch` hands the codex launcher: the
/// `--role` flag, else the scope default the session records (`reviewer` for
/// a review target, otherwise `implementer`), so the child runs the role its
/// lease names. The launcher's `AIDA_SESSION_ROLE` is deliberately not
/// consulted (TASK-1462: a launcher's role is never cloned into its child).
/// Resolved in preflight so the launcher never opens its interactive role
/// picker after the worktree and lease exist (a cancel there orphaned both).
// trace:TASK-2742 | ai:claude
pub(crate) fn start_launch_child_role(explicit: Option<&str>, owns: &str) -> String {
    explicit
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::agent_launch::agent_new_scope_role(owns).to_string())
}

/// `aida session new` — capture role + title up-front, append a record
/// to `~/.aida/session-launches.log`, then exec the configured
/// interactive vendor (`claude --permission-mode <mode>`, or codex through the
/// tracked launcher). Subsequent `aida session list` calls read
/// the launches log and join it with the .jsonl files (cwd + start-time
/// match) to surface the user-chosen title and authoritative role —
/// instead of falling back to the grep heuristic.
/// trace:FR-1-044 | ai:claude
///
/// STORY-495: `permission_mode` is now `Option<&str>`. `None` means honor
/// Claude's native permission posture — no `--permission-mode` is injected
/// (the faithful-launcher default). `Some(mode)` injects it explicitly.
///
/// Callers run [`preflight_interactive_session`] once, before any setup; this
/// launcher does not repeat it. A start --launch pins the vendor before
/// allocating the seat, including when project agents.toml is absent in the
/// newly created child worktree.
///
/// `host_main` is the host main the caller resolved before any chdir into a
/// seat ([`crate::seat_clone::launcher_main`]); every contained boundary
/// builder below takes it explicitly and none re-derives it from the cwd,
/// which after `session start --launch` names the seat clone (TASK-2032).
// trace:BUG-b6b1278890774ef993d2ebc327862470-159 | ai:claude
// trace:TASK-2032 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn new_session_for_vendor(
    host_main: Option<&Path>,
    vendor: &str,
    spec: Option<String>,
    title: Option<String>,
    permission_mode: Option<&str>,
    role_override: Option<String>,
    display_name: Option<String>,
    contained: bool,
    set_terminal_title: bool,
) -> Result<()> {
    if vendor == "codex" {
        // The tracked launcher owns child grant/subject/scope, registration,
        // finalization and terminal restoration. BUG-2398's adapter belongs
        // inside that launcher; this wrapper does not copy its pending policy.
        let cwd = std::env::current_dir()?;
        let root = crate::find_main_worktree_root().unwrap_or(cwd.clone());
        let Some(role) = crate::resolve_child_role(&root, role_override, "codex")? else {
            return Ok(());
        };
        return crate::agent_new_with_config(
            codex_interactive_session_config(contained)?,
            Some(role),
            spec,
            false,
            Some(&cwd),
            crate::AgentContextOptions::new(true, false),
            set_terminal_title,
            false,
            false,
            false,
            crate::AgentPromptOptions::new(None, true),
            crate::AgentResumeOptions::new(false, None, false, true),
            crate::AgentDefaultFlagOptions::new(false, Vec::new(), None),
            // `aida session start --name` / the derived session name, same as
            // the display name the Claude path passes to `claude --name`.
            display_name,
            title,
            // Configured bypass retains the shared launcher's consent downgrade.
            // Only a selected sandbox posture is explicit on this wrapper.
            contained,
        );
    }
    // BUG-2157: fail closed — a contained parent never launches an
    // uncontained or re-postured claude, whichever caller got here.
    // trace:BUG-2157 | ai:claude
    refuse_uncontained_session_under_contained_parent(permission_mode, contained)?;
    let role = role_override
        .or_else(|| std::env::var("AIDA_SESSION_ROLE").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "-".to_string());
    // Persona sessions receive the same single-source capability envelope as
    // `aida agent new`; ordinary seats retain their normal empty first prompt.
    // trace:TASK-1261 | ai:codex
    let persona_guidance = crate::queue_cmd::stakeholder_persona_guidance(&role);

    let title = match title {
        Some(t) if !t.trim().is_empty() => t,
        _ => inquire::Text::new("Title for this session?")
            .with_help_message("Shown in `aida session list`. Leave blank to skip.")
            .prompt()
            .unwrap_or_default(),
    };

    // STORY-495: record `native` in the launch-log when no mode is injected.
    append_launch_log(&role, permission_mode.unwrap_or("native"), &title)?;
    let mut title_restore = None;
    if set_terminal_title {
        let project_root = crate::find_main_worktree_root()
            .or_else(|_| crate::find_project_root())
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        if crate::agent_registry::terminal_title_enabled(&project_root) {
            let terminal = crate::agent_registry::current_terminal_identity();
            let session_id = uuid::Uuid::now_v7().to_string();
            let scope = std::env::var("AIDA_SESSION_SCOPE").ok();
            let title = crate::agent_registry::launch_title(&role, scope.as_deref(), &session_id);
            title_restore = Some(crate::agent_registry::apply_terminal_title(
                &title,
                terminal.as_ref(),
            ));
        }
    }

    let name_for_log = display_name.as_deref().unwrap_or("(auto)");
    let mode_display = if contained {
        "contained sandbox".to_string()
    } else {
        permission_mode
            .map(|m| format!("--permission-mode {}", m))
            .unwrap_or_else(|| "(native permission posture)".to_string())
    };
    eprintln!(
        "{} {} → claude {} (name: {})",
        glyph(crate::glyphs::Glyph::FlowActive).green().bold(),
        format!("session new (role:{}, title:{:?})", role, title).dimmed(),
        mode_display,
        name_for_log,
    );

    run_claude_session(
        host_main,
        permission_mode,
        display_name.as_deref(),
        persona_guidance,
        None,
        contained,
        None,
        title_restore,
    )
}

/// Refuse an explicit posture override when the parent requires containment.
// trace:BUG-2172 | ai:codex
pub(crate) fn refuse_contained_permission_override(
    permission_mode: Option<&str>,
    contained: bool,
) -> Result<()> {
    if contained {
        if let Some(m) = permission_mode {
            anyhow::bail!(
                "[contained] this session is contained (AIDA_CONTAINED), so every agent it \
                 launches is contained too — refusing `--permission-mode {m}`, which would run \
                 the child with a different posture. Drop --permission-mode, or launch from an \
                 uncontained shell."
            );
        }
    }
    Ok(())
}

/// Shared posture for the headless Claude burndown and intake launchers.
// trace:BUG-2172 | ai:codex
pub(crate) fn resolve_headless_claude_posture(
    permission_mode: Option<&str>,
    contained: bool,
) -> Result<String> {
    refuse_contained_permission_override(permission_mode, contained)?;
    Ok(if contained {
        "dontAsk"
    } else {
        permission_mode.unwrap_or("bypassPermissions")
    }
    .to_string())
}

/// BUG-2157: the `session new` launch guard. Under a contained parent
/// (`AIDA_CONTAINED`, strict parse) the launch must be contained and carry no
/// `--permission-mode`.
// trace:BUG-2157 | ai:claude
pub(crate) fn refuse_uncontained_session_under_contained_parent(
    permission_mode: Option<&str>,
    contained: bool,
) -> Result<()> {
    if aida_contained_env()? && (!contained || permission_mode.is_some()) {
        anyhow::bail!(
            "[contained] this session is contained (AIDA_CONTAINED), so the claude it launches \
             must be contained too — refusing an uncontained launch. Drop --permission-mode, or \
             launch from an uncontained shell."
        );
    }
    Ok(())
}

/// TASK-31: derive a claude `--name` value from session metadata. Keeps the
/// /resume picker and terminal title legible when multiple concurrent
/// worktrees are open.
///
/// Convention:
///   - role=reviewer            → `review@<scope>`               (PR/MR work)
///   - scope=EPIC-N + batchM    → `EPIC-N:batchM`                (epic-batch)
///   - other implementer shapes → `<role-label>@<scope>:<suffix>` or
///     `<role-label>@<scope>` if no suffix
///
/// `<suffix>` is the part of the branch name after `<scope-slug>-`. Returns
/// `None` when scope is empty (defensive — the caller already validates).
/// Result is truncated to 64 chars to fit common terminal-title budgets.
/// trace:TASK-31 | ai:claude
pub fn derive_session_name(scope: &str, branch: &str, role: &str) -> Option<String> {
    if scope.trim().is_empty() {
        return None;
    }
    let scope_display = normalize_scope_for_display(scope);
    let role_lower = role.to_ascii_lowercase();
    if role_lower == "reviewer" {
        return Some(truncate(&format!("review@{}", scope_display), 64));
    }
    let is_epic = scope_display.starts_with("EPIC-");
    if is_epic {
        if let Some(batch) = extract_batch_suffix(branch) {
            return Some(truncate(&format!("{}:{}", scope_display, batch), 64));
        }
    }
    let role_label = role_label_for(&role_lower);
    let result = match extract_branch_suffix(branch, scope) {
        Some(s) if !s.is_empty() => format!("{}@{}:{}", role_label, scope_display, s),
        _ => format!("{}@{}", role_label, scope_display),
    };
    Some(truncate(&result, 64))
}

fn normalize_scope_for_display(scope: &str) -> String {
    // Uppercase only when it looks like a SPEC-ID (TYPE-NUM); leave
    // free-form scopes (path globs, "feature:auth") alone.
    let parts: Vec<&str> = scope.splitn(2, '-').collect();
    if parts.len() == 2
        && !parts[1].is_empty()
        && parts[1].chars().all(|c| c.is_ascii_digit())
        && parts[0].chars().all(|c| c.is_ascii_alphabetic())
    {
        return scope.to_uppercase();
    }
    scope.to_string()
}

fn role_label_for(role_lower: &str) -> String {
    match role_lower {
        "implementer" => "impl".to_string(),
        "reviewer" => "review".to_string(),
        "" | "-" => "session".to_string(),
        other => other.to_string(),
    }
}

fn extract_batch_suffix(branch: &str) -> Option<&str> {
    let last = branch.rsplit('-').next()?;
    if last.len() > 5 && last.starts_with("batch") && last[5..].chars().all(|c| c.is_ascii_digit())
    {
        Some(last)
    } else {
        None
    }
}

fn extract_branch_suffix<'a>(branch: &'a str, scope: &str) -> Option<&'a str> {
    let scope_lower = scope.to_ascii_lowercase();
    let prefix = format!("{}-", scope_lower);
    let branch_lower = branch.to_ascii_lowercase();
    if branch_lower.starts_with(&prefix) {
        let tail = &branch[prefix.len()..];
        if !tail.is_empty() {
            return Some(tail);
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

const LAUNCH_LOG_REL: &str = ".aida/session-launches.log";

fn launch_log_path() -> Result<PathBuf> {
    let home = crate::home_dir().context("HOME not set; cannot locate launches log")?;
    Ok(home.join(LAUNCH_LOG_REL))
}

/// Append a launch record. Format: TSV with these fields, one record per
/// line (newline-terminated):
///   iso_ts \t role-or-dash \t cwd \t permission_mode \t title
/// trace:FR-1-044 | ai:claude
fn append_launch_log(role: &str, permission_mode: &str, title: &str) -> Result<()> {
    use std::io::Write;
    let path = launch_log_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\n",
        chrono::Utc::now().to_rfc3339(),
        if role.is_empty() { "-" } else { role },
        cwd.display(),
        permission_mode,
        sanitize_for_tsv(title),
    );
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

fn sanitize_for_tsv(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// Replace this process with `claude --permission-mode <mode>`. When `name`
/// is `Some(...)`, also passes `--name <n>` so the launched session is
/// labeled in the /resume picker and terminal title. trace:TASK-31 | ai:claude
/// STORY-42 / TASK-112: replace this process with `claude
/// --permission-mode <mode> [--name <n>] --session-id <uuid>
/// <initial_prompt>`. The initial prompt becomes claude's first message
/// — pass `/aida-pickup` / `/aida-review --pr N` so the agent routes
/// into the right skill on launch with no extra typing. `aida queue
/// work` mints the UUID up front so it can record it in the session
/// manifest (and a later `--resume` can find the conversation) before
/// `exec` replaces this process. `session_id` must be a valid UUID —
/// claude rejects anything else. trace:STORY-42, TASK-112 | ai:claude
// trace:TASK-1594 | ai:claude
pub fn exec_claude_with_session(
    host_main: Option<&Path>,
    permission_mode: Option<&str>,
    name: Option<&str>,
    initial_prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
    grant_id: Option<&str>,
) -> Result<()> {
    exec_claude(
        host_main,
        permission_mode,
        name,
        Some(initial_prompt),
        Some(session_id),
        contained,
        model,
        grant_id,
    )
}

/// Build the argv (after the `claude` program name) for an interactive
/// `aida queue work` launch — `--permission-mode`, optional `--name` /
/// `--session-id`, and a trailing positional initial prompt. Shared by
/// `exec_claude` (process replacement) and the interactive reviewer launch
/// plan ([`interactive_reviewer_launch_plan`], spawn + wait, BUG-226/BUG-1607)
/// so the launch paths can never drift.
/// trace:BUG-226 | ai:claude
///
/// STORY-495: `permission_mode` is `Option<&str>`. `None` omits
/// `--permission-mode` entirely so the spawned `claude` uses its native
/// permission posture (the faithful-launcher default). The headless launch
/// path uses a separate [`claude_headless_args_with_posture`] builder that
/// always forces `bypassPermissions`, so this change never touches the
/// unattended drain.
///
/// Errors when a contained launch must be refused (BUG-1982).
// trace:BUG-1982 | ai:claude
pub fn claude_session_args(
    host_main: Option<&Path>,
    permission_mode: Option<&str>,
    name: Option<&str>,
    initial_prompt: Option<&str>,
    session_id: Option<&str>,
    contained: bool,
    model: Option<&str>,
) -> Result<Vec<String>> {
    let mut args = Vec::new();
    if let Some(m) = permission_mode {
        args.push("--permission-mode".to_string());
        args.push(m.to_string());
    }
    if contained {
        args.extend(claude_contained_flags(host_main)?);
    }
    if let Some(n) = name {
        args.push("--name".to_string());
        args.push(n.to_string());
    }
    // TASK-112: a caller-minted session id, so the conversation is
    // addressable for `aida queue work --resume`.
    if let Some(sid) = session_id {
        args.push("--session-id".to_string());
        args.push(sid.to_string());
    }
    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    if let Some(p) = initial_prompt {
        // Positional first-message — claude treats trailing positionals
        // as the initial prompt for the session.
        args.push(p.to_string());
    }
    Ok(args)
}

// trace:BUG-2006 | ai:antigravity
pub(crate) fn build_claude_exec_command(
    host_main: Option<&Path>,
    permission_mode: Option<&str>,
    name: Option<&str>,
    initial_prompt: Option<&str>,
    session_id: Option<&str>,
    contained: bool,
    model: Option<&str>,
    grant_id: Option<&str>,
) -> Result<std::process::Command> {
    let (program, args) = interactive_claude_program_and_args(
        host_main,
        contained,
        claude_session_args(
            host_main,
            permission_mode,
            name,
            initial_prompt,
            session_id,
            contained,
            model,
        )?,
    )?;
    // trace:STORY-2326 TASK-2032 | ai:claude
    let ctx = interactive_claude_context(
        host_main,
        aida_core::launch_policy::LaunchKind::Exec,
        program,
        args,
        contained,
    )?;
    let mut cmd = crate::launch::spawn_agent(&ctx)?;
    crate::launch::assign_seat(
        &mut cmd,
        ctx.kind(),
        crate::launch::SeatDecision::ThisSession { grant: grant_id },
    )?;
    Ok(cmd)
}

/// The context of an interactive Claude session this process becomes or
/// waits on: it inherits this process's cwd.
// trace:STORY-2326 | ai:claude
fn interactive_claude_context(
    host_main: Option<&Path>,
    kind: aida_core::launch_policy::LaunchKind,
    program: String,
    args: Vec<String>,
    contained: bool,
) -> Result<crate::launch::LaunchContext> {
    let posture = if contained {
        ContainmentPosture::Aida
    } else {
        ContainmentPosture::None
    };
    // The process cwd, as the contained flags resolve it (tests pin it).
    let cwd = match contained_session_cwd() {
        Some(cwd) => cwd,
        None => std::env::current_dir()?,
    };
    Ok({
        let req = crate::launch::LaunchRequest {
            vendor: aida_core::launch_policy::LaunchVendor::Claude,
            kind,
            program: program.into(),
            args,
            tail: Vec::new(),
            posture,
            cwd,
            set_cwd: false,
            host_main: host_main.map(Path::to_path_buf),
        };
        // trace:STORY-2538 | ai:codex
        let exec = crate::session::capture_launch_exec(&req.cwd, req.host_main.as_deref())?;
        crate::launch::LaunchContext::resolve(&exec, req)
    }?)
}

/// BUG-2169 S2: an interactive contained `claude` runs inside the
/// `[contained] os_wrap` OS boundary like the headless and foreground
/// `agent new` launches, scoped to the git root of the session's cwd. With
/// os_wrap off, or for an uncontained launch, the program stays `claude`.
// trace:BUG-2169 | ai:claude
pub(crate) fn interactive_claude_program_and_args(
    host_main: Option<&Path>,
    contained: bool,
    args: Vec<String>,
) -> Result<(String, Vec<String>)> {
    if !contained {
        return Ok(("claude".to_string(), args));
    }
    let cwd = contained_session_cwd().unwrap_or_else(|| PathBuf::from("."));
    os_wrapped_program_and_args(host_main, &cwd, "claude", args)
}

// trace:TASK-1594 | ai:claude
fn exec_claude(
    host_main: Option<&Path>,
    permission_mode: Option<&str>,
    name: Option<&str>,
    initial_prompt: Option<&str>,
    session_id: Option<&str>,
    contained: bool,
    model: Option<&str>,
    grant_id: Option<&str>,
) -> Result<()> {
    let mut cmd = build_claude_exec_command(
        host_main,
        permission_mode,
        name,
        initial_prompt,
        session_id,
        contained,
        model,
        grant_id,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        anyhow::bail!("failed to exec claude: {}", err);
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().context("failed to spawn claude")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

// trace:STORY-994 | ai:codex
struct TerminalTitleRestoreGuard(Option<crate::agent_registry::TerminalTitleRestore>);

impl TerminalTitleRestoreGuard {
    fn new(restore: Option<crate::agent_registry::TerminalTitleRestore>) -> Self {
        Self(restore)
    }

    fn restore_now(&mut self) {
        if let Some(restore) = self.0.take() {
            crate::agent_registry::restore_terminal_title(restore);
        }
    }
}

impl Drop for TerminalTitleRestoreGuard {
    fn drop(&mut self) {
        self.restore_now();
    }
}

// trace:BUG-2006 | ai:antigravity
pub(crate) fn build_run_claude_session_command(
    host_main: Option<&Path>,
    permission_mode: Option<&str>,
    name: Option<&str>,
    initial_prompt: Option<&str>,
    session_id: Option<&str>,
    contained: bool,
    model: Option<&str>,
) -> Result<std::process::Command> {
    // trace:BUG-2169 | ai:claude
    let (program, args) = interactive_claude_program_and_args(
        host_main,
        contained,
        claude_session_args(
            host_main,
            permission_mode,
            name,
            initial_prompt,
            session_id,
            contained,
            model,
        )?,
    )?;
    // trace:STORY-2326 TASK-2032 | ai:claude
    let ctx = interactive_claude_context(
        host_main,
        aida_core::launch_policy::LaunchKind::Fresh,
        program,
        args,
        contained,
    )?;
    let mut cmd = crate::launch::spawn_agent(&ctx)?;
    crate::launch::assign_seat(
        &mut cmd,
        ctx.kind(),
        crate::launch::SeatDecision::ThisSession { grant: None },
    )?;
    Ok(cmd)
}

// trace:STORY-994 | ai:codex
fn run_claude_session(
    host_main: Option<&Path>,
    permission_mode: Option<&str>,
    name: Option<&str>,
    initial_prompt: Option<&str>,
    session_id: Option<&str>,
    contained: bool,
    model: Option<&str>,
    title_restore: Option<crate::agent_registry::TerminalTitleRestore>,
) -> Result<()> {
    let mut title_restore = TerminalTitleRestoreGuard::new(title_restore);
    let mut cmd = build_run_claude_session_command(
        host_main,
        permission_mode,
        name,
        initial_prompt,
        session_id,
        contained,
        model,
    )?;
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            anyhow::bail!("failed to spawn claude: {}", err);
        }
    };
    let status = match child.wait() {
        Ok(status) => status,
        Err(err) => {
            anyhow::bail!("failed to wait for claude: {}", err);
        }
    };
    title_restore.restore_now();
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// BUG-226: spawn `claude --resume <id>` and wait — the spawn counterpart
/// of `exec_claude_resume` for the standalone reviewer summary path.
/// trace:BUG-226 | ai:claude
pub fn spawn_claude_resume(
    host_main: Option<&Path>,
    id: &str,
    permission_mode: Option<&str>,
    contained: bool,
) -> Result<std::process::ExitStatus> {
    spawn_claude_resume_with_grant(host_main, id, permission_mode, contained, None, None)
}

/// BUG-2149: [`spawn_claude_resume`] with an explicit seat grant for the
/// child, so a caller that minted the child's grant knows exactly which grant
/// the resumed session records under. `None` inherits the ambient grant.
/// `prompt` supplies a new message when refreshing policy in an older thread.
// trace:BUG-2149 | ai:claude
pub(crate) fn spawn_claude_resume_with_grant(
    host_main: Option<&Path>,
    id: &str,
    permission_mode: Option<&str>,
    contained: bool,
    grant_id: Option<&str>,
    prompt: Option<&str>,
) -> Result<std::process::ExitStatus> {
    // trace:BUG-2172 | ai:codex
    let parent_contained = aida_contained_env()?;
    let contained = contained || parent_contained;
    refuse_contained_permission_override(permission_mode, parent_contained)?;
    let prog = resolve_agent_program("claude");
    let mut cmd = std::process::Command::new(prog);
    for key in crate::agent_handle::INHERITED_SESSION_ENV {
        cmd.env_remove(key);
    }
    if let Some(grant) = grant_id {
        cmd.env(crate::gate::seat_authority::GRANT_ENV, grant)
            .env("AIDA_SESSION_ROLE", "reviewer");
        // A resume keeps the session's handle. trace:BUG-2187 | ai:claude
        apply_resumed_child_agent_identity(&mut cmd, HeadlessVendor::Claude, "reviewer", id);
    }
    // trace:BUG-2139 | ai:claude
    scrub_launcher_session_env(&mut cmd);
    cmd.args(["--resume", id]);
    if let Some(m) = permission_mode {
        cmd.args(["--permission-mode", m]);
    }
    if contained {
        cmd.args(claude_contained_flags(host_main)?);
    }
    // A positional message is submitted to the resumed interactive session.
    // trace:BUG-2546 | ai:codex
    if let Some(prompt) = prompt {
        cmd.arg(prompt);
    }
    // trace:BUG-2006 | ai:antigravity
    if contained {
        cmd.env("AIDA_CONTAINED", "1");
        // trace:TASK-2631 | ai:codex — do not inherit a native parent's display marker.
        cmd.env("AIDA_CONTAINMENT_POSTURE", "aida");
    } else {
        cmd.env_remove("AIDA_CONTAINED");
        // trace:TASK-2631 | ai:codex
        cmd.env("AIDA_CONTAINMENT_POSTURE", "none");
    }
    if contained {
        prepare_contained_mailbox_for_launch(host_main, &std::env::current_dir()?)?;
    }
    // A watch-launched reviewer is recorded before the wait. trace:TASK-1993 | ai:claude
    crate::review_watch::reviewer_status(&mut cmd).context("failed to spawn claude")
}

/// TASK-895: build the argv (after the `codex` program name) for an interactive
/// Codex session hosted in an `aida tui` tab. Codex's interactive CLI takes the
/// initial prompt as a trailing positional (`codex [PROMPT]`) — the analogue of
/// claude's positional first message. Unlike claude there is no caller-minted
/// `--session-id` and no AIDA-addressable `--resume`, so AIDA hosts a fresh
/// Codex session per tab (resume-parity is a follow-up). Faithful-launcher
/// posture: native launches carry no approval/sandbox bypass flag; the uniform
/// `[agents] bypass = true` posture maps to Codex's explicit bypass flag.
/// Pure — the flag set is unit-tested without spawning codex.
// trace:TASK-895 | ai:claude
// trace:BUG-743 | ai:codex
pub fn codex_session_args(initial_prompt: &str, bypass: bool, model: Option<&str>) -> Vec<String> {
    let mut args = Vec::new();
    if bypass {
        args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
    }
    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    // Interactive Codex does not expand custom /aida-* slash commands.
    // trace:BUG-2387 | ai:codex
    args.push(codex_interactive_prompt(initial_prompt));
    args
}

/// BUG-2394: force Codex's read-only sandbox onto an assembled argv. A plan
/// lane must not write, so every bypass flag and every earlier sandbox choice
/// (including the native posture's `--sandbox workspace-write` default) is
/// removed and exactly one `--sandbox read-only` is prepended. Approval flags
/// are kept: under `--ask-for-approval never` a write is refused, not asked.
// trace:BUG-2394 | ai:claude
pub(crate) fn codex_read_only_args(args: Vec<String>) -> Vec<String> {
    let mut out = vec!["--sandbox".to_string(), "read-only".to_string()];
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--dangerously-bypass-approvals-and-sandbox" || arg.starts_with("--sandbox=") {
            continue;
        }
        // TASK-2712: a plan lane writes nothing, so the native write set's
        // `--add-dir` grants go with the workspace-write posture.
        // trace:TASK-2712 | ai:claude
        if arg == "--sandbox" || arg == "-s" || arg == "--add-dir" {
            iter.next();
            continue;
        }
        if arg.starts_with("--add-dir=") {
            continue;
        }
        out.push(arg);
    }
    out
}

/// BUG-2394: whether a resolved queue-work permission mode asks Codex for the
/// read-only plan posture (`--plan-only`, or an explicit `--permission-mode plan`).
// trace:BUG-2394 | ai:claude
pub(crate) fn codex_plan_read_only(permission_mode: Option<&str>) -> bool {
    permission_mode == Some("plan")
}

/// BUG-2394: the MCP server names a Codex launch running in `launch_cwd` would
/// load from its config files — `$CODEX_HOME/config.toml` (default
/// `~/.codex/config.toml`) and, only when Codex trusts that directory, its
/// `.codex/config.toml`. Codex resolves trust for the directory it runs in (the
/// seat), not for the launcher's checkout.
/// Codex ignores an untrusted project's config, and a `-c` override for a
/// server it never loaded creates a transport-less entry that refuses to start,
/// so such servers are not listed. Only the table names are read; no command,
/// env or credential value leaves this function. Unreadable / malformed files
/// contribute nothing (Codex itself refuses to start on a malformed config).
// trace:BUG-2394 | ai:claude
pub(crate) fn codex_configured_mcp_servers(launch_cwd: &Path) -> Vec<String> {
    let codex_home = std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::home_dir().map(|h| h.join(".codex")));
    let home_config = codex_home.and_then(|home| read_toml_table(&home.join("config.toml")));
    let project_config = codex_project_trusted(home_config.as_ref(), launch_cwd)
        .then(|| read_toml_table(&launch_cwd.join(".codex").join("config.toml")))
        .flatten();
    let mut names = Vec::new();
    for table in [home_config.as_ref(), project_config.as_ref()]
        .into_iter()
        .flatten()
    {
        if let Some(servers) = table.get("mcp_servers").and_then(|v| v.as_table()) {
            for name in servers.keys() {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
    }
    names.sort();
    names
}

fn read_toml_table(path: &Path) -> Option<toml::Table> {
    std::fs::read_to_string(path)
        .ok()?
        .parse::<toml::Table>()
        .ok()
}

/// BUG-2394: whether Codex loads `project_root`'s own `.codex/config.toml`:
/// its user config marks the project `[projects."<path>"] trust_level =
/// "trusted"`, under the path as given or canonicalized. Anything else is
/// untrusted, so no override is emitted for that file's servers.
// trace:BUG-2394 | ai:claude
pub(crate) fn codex_project_trusted(
    home_config: Option<&toml::Table>,
    project_root: &Path,
) -> bool {
    let Some(projects) = home_config
        .and_then(|c| c.get("projects"))
        .and_then(|v| v.as_table())
    else {
        return false;
    };
    let canonical = std::fs::canonicalize(project_root).ok();
    [Some(project_root.to_path_buf()), canonical]
        .into_iter()
        .flatten()
        .any(|path| {
            path.to_str()
                .and_then(|key| projects.get(key))
                .and_then(|entry| entry.get("trust_level"))
                .and_then(|v| v.as_str())
                == Some("trusted")
        })
}

/// BUG-2394: the `-c` overrides that hold a Codex seat to `surface`, given the
/// MCP servers its config files declare (`configured`).
///
/// - `off`: every configured server gets `mcp_servers.<name>.enabled=false`.
/// - `aida`: the same for every server except `aida`; `aida` is enabled, and
///   when it is not configured AIDA's own vouched definition is injected.
/// - `native`: nothing — Codex's own configuration applies.
///
/// Codex addresses `-c` keys by a plain dotted path, so a server name outside
/// `[A-Za-z0-9_-]` cannot be targeted exactly; that is an error rather than a
/// silently unenforced posture. Servers contributed by Codex plugins are not in
/// the config files and reject an `enabled` override (probed on codex-cli
/// 0.166.0: `invalid transport`), so they are outside this enforcement —
/// [`CODEX_MCP_PLUGIN_NOTE`] says so wherever the posture is reported.
// trace:BUG-2394 | ai:claude
pub(crate) fn codex_mcp_flags(
    surface: AgentMcpSurface,
    configured: &[String],
) -> Result<Vec<String>> {
    if surface == AgentMcpSurface::Native {
        return Ok(Vec::new());
    }
    let keep_aida = surface == AgentMcpSurface::Aida;
    let mut args = Vec::new();
    for name in configured {
        if keep_aida && name == "aida" {
            continue;
        }
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            anyhow::bail!(
                "[agents] mcp `{}` cannot be enforced for codex: MCP server `{name}` in the \
                 Codex config has a name a `-c mcp_servers.<name>.enabled=false` override cannot \
                 address. Rename that server, or set [agents] mcp = \"native\" to leave Codex's \
                 MCP configuration alone",
                surface.as_str()
            );
        }
        args.extend([
            "-c".to_string(),
            format!("mcp_servers.{name}.enabled=false"),
        ]);
    }
    if keep_aida {
        if configured.iter().any(|n| n == "aida") {
            args.extend([
                "-c".to_string(),
                "mcp_servers.aida.enabled=true".to_string(),
            ]);
        } else {
            args.extend([
                "-c".to_string(),
                "mcp_servers.aida.command=\"aida\"".to_string(),
                "-c".to_string(),
                "mcp_servers.aida.args=[\"mcp-serve\"]".to_string(),
            ]);
        }
    }
    Ok(args)
}

/// BUG-2394: the `[agents] mcp` surface configured for `project_root` and the
/// `-c` overrides that enforce it on a Codex seat running in `launch_cwd`.
// trace:BUG-2394 | ai:claude
pub(crate) fn codex_mcp_launch_flags(
    project_root: &Path,
    launch_cwd: &Path,
) -> Result<(AgentMcpSurface, Vec<String>)> {
    let surface = crate::agent_config::load_codex_agents_mcp(project_root)?;
    let flags = codex_mcp_flags(surface, &codex_configured_mcp_servers(launch_cwd))?;
    Ok((surface, flags))
}

/// BUG-2394: the limit of [`codex_mcp_flags`], stated wherever the Codex MCP
/// posture is reported.
// trace:BUG-2394 | ai:claude
pub(crate) const CODEX_MCP_PLUGIN_NOTE: &str =
    "config-file servers only; MCP servers contributed by Codex plugins are not covered";

/// TASK-895: replace this process with an interactive `codex <prompt>` session —
/// the Codex analogue of [`exec_claude_with_session`], for a Codex tab hosted by
/// the AIDA TUI. The hosted `aida queue work` process is replaced by `codex`, so
/// all lease / worktree / manifest setup has already run by the time this is
/// reached.
// trace:BUG-2006 | ai:antigravity
// trace:STORY-2326 | ai:codex
pub(crate) fn build_codex_session_command(
    initial_prompt: &str,
    bypass: bool,
    read_only: bool,
    model: Option<&str>,
    grant_id: Option<&str>,
) -> Result<std::process::Command> {
    let ctx = {
        let req = crate::launch::LaunchRequest {
            vendor: aida_core::launch_policy::LaunchVendor::Codex,
            kind: aida_core::launch_policy::LaunchKind::Exec,
            program: "codex".into(),
            // Launcher-owned posture switches; prompt/model are values, not a
            // default_flags or operator template tail. trace:STORY-2326 | ai:codex
            args: codex_interactive_session_args(initial_prompt, bypass, read_only, model),
            tail: Vec::new(),
            posture: ContainmentPosture::None,
            cwd: std::env::current_dir()?,
            set_cwd: false,
            // Native/None constructors do not cross the AIDA boundary.
            host_main: None,
        };
        // trace:STORY-2538 | ai:codex
        let exec = crate::session::capture_launch_exec(&req.cwd, req.host_main.as_deref())?;
        crate::launch::LaunchContext::resolve(&exec, req)
    }?;
    let mut cmd = crate::launch::spawn_agent(&ctx)?;
    crate::launch::assign_seat(
        &mut cmd,
        ctx.kind(),
        crate::launch::SeatDecision::ThisSession { grant: grant_id },
    )?;
    Ok(cmd)
}

/// BUG-2394: [`codex_session_args`] with the plan lane's read-only posture
/// applied. Read-only wins over bypass: a plan lane never writes.
// trace:BUG-2394 | ai:claude
pub(crate) fn codex_interactive_session_args(
    initial_prompt: &str,
    bypass: bool,
    read_only: bool,
    model: Option<&str>,
) -> Vec<String> {
    let args = codex_session_args(initial_prompt, bypass && !read_only, model);
    if read_only {
        codex_read_only_args(args)
    } else {
        args
    }
}

// trace:TASK-895 | ai:claude
// trace:TASK-1594 | ai:claude
pub fn exec_codex_session(
    host_main: Option<&Path>,
    initial_prompt: &str,
    bypass: bool,
    read_only: bool,
    model: Option<&str>,
    grant_id: Option<&str>,
    role: Option<&str>,
) -> Result<()> {
    // trace:TASK-2631 | ai:codex
    let requested = drain_contained(&headless_worktree_root())?;
    let posture = containment_posture(HeadlessVendor::Codex, requested)?;
    let mut cmd = if posture == ContainmentPosture::Native {
        // TASK-2712: the write set of the lane's own tree and role (the gate
        // ledger only for a reviewer).
        // trace:TASK-2712 TASK-2032 | ai:claude
        let mut args = crate::codex_native_sandbox::codex_native_sandbox_flags_in(
            host_main,
            &headless_worktree_root(),
            role,
        )?;
        args.extend(codex_session_args(initial_prompt, false, model));
        // trace:BUG-2394 | ai:claude — a plan lane narrows the native sandbox.
        if read_only {
            args = codex_read_only_args(args);
        }
        // trace:STORY-2326 | ai:codex
        let ctx = {
            let req = crate::launch::LaunchRequest {
                vendor: aida_core::launch_policy::LaunchVendor::Codex,
                kind: aida_core::launch_policy::LaunchKind::Exec,
                program: "codex".into(),
                // Native sandbox/MCP flags and codex_session_args are generated
                // here by trusted composers; no caller argv tail is accepted.
                args,
                tail: Vec::new(),
                posture,
                cwd: headless_worktree_root(),
                set_cwd: false,
                host_main: host_main.map(Path::to_path_buf),
            };
            // trace:STORY-2538 | ai:codex
            let exec = crate::session::capture_launch_exec(&req.cwd, req.host_main.as_deref())?;
            crate::launch::LaunchContext::resolve(&exec, req)
        }?;
        let mut cmd = crate::launch::spawn_agent(&ctx)?;
        crate::launch::assign_seat(
            &mut cmd,
            ctx.kind(),
            crate::launch::SeatDecision::ThisSession { grant: grant_id },
        )?;
        eprintln!("  codex: {}", posture.label());
        cmd
    } else {
        build_codex_session_command(initial_prompt, bypass, read_only, model, grant_id)?
    };
    stamp_containment_posture(&mut cmd, posture);
    // trace:BUG-2398 | ai:codex — this is the already-delegated tab's seat.
    cmd.env(
        crate::gate::seat_authority::GRANT_ENV,
        grant_id.unwrap_or(""),
    )
    .env("AIDA_USER", crate::current_user_id(None))
    .env("AIDA_PROJECT_ROOT", headless_worktree_root())
    .env("AIDA_AGENT_TYPE", "codex");
    if let Some(id) = grant_id.filter(|id| !id.is_empty()) {
        if let crate::gate::seat_authority::GrantCheck::Valid(grant) =
            crate::gate::seat_authority::grant_check_for(
                &headless_worktree_root(),
                id,
                &crate::current_user_id(None),
            )
        {
            cmd.env("AIDA_SESSION_ROLE", grant.seat);
        }
    }
    for key in [
        "AIDA_SESSION_ID",
        "AIDA_SESSION_SCOPE",
        "AIDA_AGENT_NAME",
        "AIDA_AGENT_CONTEXT_FILE",
    ] {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    crate::vendors::profile::finalize_codex(&mut cmd)?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        anyhow::bail!("failed to exec codex: {}", err);
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().context("failed to spawn codex")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// TASK-2712: the lane a native Codex launch writes for: its own tree and its
/// role (`None` when unknown: no gate ledger).
// trace:TASK-2712 | ai:claude
#[derive(Debug, Clone, Copy)]
pub(crate) struct CodexLane<'a> {
    pub(crate) tree: &'a Path,
    pub(crate) role: Option<&'a str>,
    /// The launcher's pinned host main, the only anchor that recognises
    /// `tree` as a host-recorded seat clone (TASK-2032).
    // trace:TASK-2032 | ai:claude
    pub(crate) host_main: Option<&'a Path>,
}

/// BUG-2386: an interactive `codex resume <thread>` on the posture a fresh
/// interactive Codex launch resolves ([`exec_codex_session`]): a contained
/// request (the parent's `AIDA_CONTAINED` or the `[agents] contained` knob)
/// takes Codex's native sandbox or is refused, never run uncontained;
/// otherwise `bypass` maps to Codex's bypass flag. `grant_id` binds the
/// child's seat grant. `contained` is the caller's own request (e.g.
/// `queue work --sandbox`), OR-ed with the parent env and the knob so a
/// flag-only request is never dropped. `read_only` keeps a plan lane's
/// read-only sandbox (BUG-2394) on the resumed thread; it wins over bypass.
/// `lane` is the RESUMED lane's own tree and role, whose write set a native
/// resume gets (TASK-2712 F8), never this process's cwd.
// trace:BUG-2386 | ai:claude
pub(crate) fn build_codex_resume_command(
    thread_id: &str,
    bypass: bool,
    read_only: bool,
    grant_id: Option<&str>,
    contained: bool,
    lane: CodexLane<'_>,
) -> Result<std::process::Command> {
    let requested = contained || drain_contained(&headless_worktree_root())?;
    require_vendor_containment(HeadlessVendor::Codex, requested, "resumed codex session")?;
    let posture = containment_posture(HeadlessVendor::Codex, requested)?;
    let native = posture == ContainmentPosture::Native;
    let mut cmd =
        std::process::Command::new(resolve_agent_program(HeadlessVendor::Codex.program()));
    cmd.args(codex_interactive_resume_args(
        thread_id,
        native.then_some(lane),
        bypass && !native,
        read_only,
    )?);
    // An explicit grant decision either way: the delegated grant, or the
    // empty baseline selector (BUG-2398 finalization refuses neither-nor).
    // trace:BUG-2386 | ai:claude
    cmd.env(
        crate::gate::seat_authority::GRANT_ENV,
        grant_id.unwrap_or(""),
    );
    // trace:BUG-2139 | ai:claude
    scrub_launcher_session_env(&mut cmd);
    cmd.env_remove("AIDA_CONTAINED");
    stamp_containment_posture(&mut cmd, posture);
    if native {
        eprintln!("  codex: {}", posture.label());
    }
    Ok(cmd)
}

/// BUG-2386: replace this process with [`build_codex_resume_command`].
// trace:BUG-2386 | ai:claude
pub fn exec_codex_resume(
    thread_id: &str,
    bypass: bool,
    read_only: bool,
    grant_id: Option<&str>,
    contained: bool,
    lane: CodexLane<'_>,
) -> Result<()> {
    let mut cmd =
        build_codex_resume_command(thread_id, bypass, read_only, grant_id, contained, lane)?;
    finalize_codex_resume(&mut cmd, &headless_worktree_root())?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        anyhow::bail!("failed to exec codex: {}", err);
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().context("failed to spawn codex")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// BUG-2386: finalize an interactive Codex resume built by
/// [`build_codex_resume_command`] exactly like a fresh interactive Codex
/// launch ([`exec_codex_session`]): public child metadata, the seat the
/// delegated grant names (unless the caller already set one), then
/// [`crate::vendors::profile::finalize_codex`]. Call last, after the caller's
/// cwd/seat customization and before any stdio or spawn/exec. A caller that
/// issued the grant revokes it when this refuses.
// trace:BUG-2386 | ai:claude
pub(crate) fn finalize_codex_resume(
    cmd: &mut std::process::Command,
    project_root: &Path,
) -> Result<()> {
    let explicit = |cmd: &std::process::Command, key: &str| {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
    };
    let user = crate::current_user_id(None);
    if explicit(cmd, "AIDA_SESSION_ROLE").is_none() {
        if let Some(id) =
            explicit(cmd, crate::gate::seat_authority::GRANT_ENV).filter(|id| !id.is_empty())
        {
            if let crate::gate::seat_authority::GrantCheck::Valid(grant) =
                crate::gate::seat_authority::grant_check_for(project_root, &id, &user)
            {
                cmd.env("AIDA_SESSION_ROLE", grant.seat);
            }
        }
    }
    cmd.env("AIDA_USER", user)
        .env("AIDA_PROJECT_ROOT", project_root)
        .env("AIDA_AGENT_TYPE", "codex");
    crate::vendors::profile::finalize_codex(cmd)
}

/// BUG-1607: the interactive reviewer launch a resolved `vendor` maps to —
/// PURE decision (which program + argv, or a refusal for an unsupported
/// vendor); no process is spawned. Every interactive reviewer launch site
/// (the standalone `aida queue work --role reviewer` launch and the inline
/// `aida review` / `aida human review` launch) builds its plan through this
/// ONE function and then hands it to [`spawn_reviewer_launch_plan`], so a
/// vendor knob can never route one site and miss the other — the exact
/// BUG-1607 failure mode. Unit-tested directly, without spawning
/// `claude`/`codex`.
// trace:BUG-1607 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewerLaunchPlan {
    pub(crate) vendor: HeadlessVendor,
    /// The binary to spawn — routed through [`resolve_agent_program`], so a
    /// test can swap it for a mock via `AIDA_AGENT_CMD` without needing a
    /// real `claude`/`codex` installed. Unset, it is the vendor's own
    /// binary name, byte-identical to before this seam existed.
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    // trace:BUG-2006 | ai:antigravity
    pub(crate) contained: bool,
    /// TASK-2032: the host main the plan's entry point resolved before any
    /// chdir into a seat; the spawn builds its boundary from this, never
    /// from the process cwd.
    // trace:TASK-2032 | ai:claude
    pub(crate) host_main: Option<PathBuf>,
}

// trace:BUG-1607 | ai:claude
pub(crate) fn interactive_reviewer_launch_plan(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    permission_mode: Option<&str>,
    name: Option<&str>,
    prompt: &str,
    session_id: &str,
    contained: bool,
) -> Result<ReviewerLaunchPlan> {
    // trace:BUG-2172 | ai:codex
    let parent_contained = aida_contained_env()?;
    let contained = contained || parent_contained;
    // BUG-2144: a contained reviewer on a vendor that cannot be contained is
    // refused, never downgraded. trace:BUG-2144 | ai:claude
    require_vendor_containment(vendor, contained, "reviewer session")?;
    refuse_contained_permission_override(permission_mode, parent_contained)?;
    match vendor {
        HeadlessVendor::Claude => Ok(ReviewerLaunchPlan {
            vendor,
            program: resolve_agent_program(vendor.program()),
            args: claude_session_args(
                host_main,
                permission_mode,
                name,
                Some(prompt),
                Some(session_id),
                contained,
                None,
            )?,
            contained: containment_applied(vendor, contained, None),
            host_main: host_main.map(Path::to_path_buf),
        }),
        HeadlessVendor::Codex => {
            // Interactive queue-work's Codex-tab launch maps the uniform
            // `[agents] bypass` posture (surfaced here as
            // `--permission-mode bypassPermissions`) to Codex's own bypass
            // flag (BUG-743); a reviewer session follows the same mapping.
            let bypass = permission_mode == Some("bypassPermissions");
            Ok(ReviewerLaunchPlan {
                vendor,
                program: resolve_agent_program(vendor.program()),
                // trace:TASK-2631 | ai:codex
                args: if contained {
                    // TASK-2712 F2: a reviewer's write set (gate ledger
                    // included) for the tree it launches in.
                    // trace:TASK-2712 | ai:claude
                    // trace:TASK-2032 | ai:claude
                    let mut args = crate::codex_native_sandbox::codex_native_sandbox_flags_in(
                        host_main,
                        &headless_worktree_root(),
                        Some("reviewer"),
                    )?;
                    args.extend(codex_session_args(prompt, false, None));
                    args
                } else {
                    codex_session_args(prompt, bypass, None)
                },
                contained: containment_applied(vendor, contained, None),
                host_main: host_main.map(Path::to_path_buf),
            })
        }
        HeadlessVendor::Agy => anyhow::bail!(
            "interactive reviewer launch does not support vendor `agy` yet. Recovery: \
             re-run with `--vendor claude` or `--vendor codex`, or use `--no-human` for a \
             headless AGY reviewer."
        ),
    }
}

/// The interactive advisor plan: the reviewer plan's vendor mapping, with an
/// advisor-worded containment refusal checked first. Pure: it reserves no
/// handle, so the dry-run preview uses it too.
// trace:BUG-2389 | ai:claude
pub(crate) fn advisor_interactive_launch_plan(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    contained: bool,
) -> Result<ReviewerLaunchPlan> {
    let contained = aida_contained_env()? || contained;
    require_vendor_containment(vendor, contained, "interactive advisor session")?;
    let prompt =
        crate::intake::materialize_skill_invocation(prompt, vendor == HeadlessVendor::Claude);
    interactive_reviewer_launch_plan(
        host_main,
        vendor,
        None,
        None,
        &prompt,
        &uuid::Uuid::now_v7().to_string(),
        contained,
    )
}

/// Use the shared interactive vendor plan with an advisor's own child grant.
// trace:BUG-2389 | ai:codex
pub(crate) fn build_advisor_interactive_command(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    grant_env: &str,
    contained: bool,
) -> Result<std::process::Command> {
    let contained = aida_contained_env()? || contained;
    let root = headless_worktree_root();
    let plan = advisor_interactive_launch_plan(host_main, vendor, prompt, contained)?;
    let (program, args) = os_wrapped_program_and_args(host_main, &root, &plan.program, plan.args)?;
    // trace:STORY-2326 TASK-2032 | ai:claude
    let ctx = headless_launch_context(
        host_main,
        vendor,
        program,
        args,
        containment_posture(vendor, contained)?,
    )?;
    let mut cmd = crate::launch::spawn_agent(&ctx)?;
    crate::launch::assign_seat(
        &mut cmd,
        ctx.kind(),
        crate::launch::SeatDecision::Child {
            role: "advisor",
            grant: grant_env,
        },
    )?;
    apply_child_agent_identity(&mut cmd, vendor, "advisor");
    apply_headless_harness_version(host_main, &mut cmd, vendor, &root);
    Ok(cmd)
}

/// Spawn (not exec) an [`interactive_reviewer_launch_plan`] and wait,
/// returning the exit status — the only place an interactive reviewer
/// launch plan is actually turned into a process.
// trace:BUG-1607 | ai:claude
pub(crate) fn spawn_reviewer_launch_plan(
    plan: &ReviewerLaunchPlan,
    project_root: &std::path::Path,
) -> Result<std::process::ExitStatus> {
    spawn_reviewer_launch_plan_granted(plan, project_root).map(|(status, _)| status)
}

/// BUG-2149: [`spawn_reviewer_launch_plan`], also returning the id of the
/// child reviewer grant it issued.
// trace:BUG-2149 | ai:claude
pub(crate) fn spawn_reviewer_launch_plan_granted(
    plan: &ReviewerLaunchPlan,
    project_root: &std::path::Path,
) -> Result<(std::process::ExitStatus, String)> {
    // Reject a stale/uncontained plan before issuing any launch authority.
    // Builders inherit containment; direct callers cannot bypass that boundary.
    // trace:BUG-2172 | ai:codex
    let parent_contained = aida_contained_env()?;
    require_vendor_containment(
        plan.vendor,
        parent_contained || plan.contained,
        "reviewer session",
    )?;
    if parent_contained && !plan.contained {
        anyhow::bail!("[contained] reviewer launch plan is uncontained; rebuild it under the current parent before launching");
    }
    // trace:TASK-2631 | ai:codex
    let posture = if plan.contained {
        ContainmentPosture::Aida
    } else if plan.vendor == HeadlessVendor::Codex
        && plan
            .args
            .windows(2)
            .any(|w| w == ["--sandbox", "workspace-write"])
    {
        ContainmentPosture::Native
    } else {
        ContainmentPosture::None
    };
    // The boundary (the contained mailbox) is set up before the grant exists.
    // trace:STORY-2326 | ai:claude
    let ctx = {
        let req = crate::launch::LaunchRequest {
            vendor: crate::launch::context::launch_vendor(plan.vendor),
            kind: aida_core::launch_policy::LaunchKind::Fresh,
            program: plan.program.clone().into(),
            args: plan.args.clone(),
            tail: Vec::new(),
            posture,
            cwd: std::env::current_dir()?,
            set_cwd: false,
            // trace:TASK-2032 | ai:claude
            host_main: plan.host_main.clone(),
        };
        // trace:STORY-2538 | ai:codex
        let exec = crate::session::capture_launch_exec(&req.cwd, req.host_main.as_deref())?;
        crate::launch::LaunchContext::resolve(&exec, req)
    }?;
    let mut cmd = crate::launch::spawn_agent(&ctx)?;
    // A contained reviewer's tool calls run in a sandbox, where only a
    // keyring-held key presents its grant: refuse before the review is spent.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
    let grant = crate::launch::issue_launch_child(
        project_root,
        "reviewer",
        &crate::current_user_id(None),
        plan.contained,
    )?;
    // trace:BUG-2187 | ai:claude
    // trace:STORY-1843 | ai:codex
    let mut handle_guard = child_reservation_guard();
    apply_child_agent_identity(&mut cmd, plan.vendor, "reviewer");
    // trace:BUG-2139 STORY-2326 | ai:claude
    crate::launch::assign_seat(
        &mut cmd,
        ctx.kind(),
        crate::launch::SeatDecision::Child {
            role: "reviewer",
            grant: &grant.id,
        },
    )?;
    if posture == ContainmentPosture::Native {
        eprintln!("  codex: {}", posture.label());
    }
    if plan.vendor == HeadlessVendor::Codex {
        cmd.env("AIDA_USER", crate::current_user_id(None))
            .env("AIDA_PROJECT_ROOT", project_root);
        if let Err(error) = crate::vendors::profile::finalize_codex(&mut cmd) {
            revoke_headless_child_grant(&grant.id);
            return Err(error);
        }
    }
    // A watch-launched reviewer is recorded before the wait. trace:TASK-1993 | ai:claude
    let status = reviewer_status_with_handle_guard(&mut cmd, &mut handle_guard)
        .with_context(|| format!("failed to spawn {}", plan.program))?;
    Ok((status, grant.id))
}

#[derive(Debug, Clone)]
pub(crate) struct ResumeTarget {
    pub(crate) agent: String,
    pub(crate) id: String,
    /// The checkout whose Claude project store the transcript was found in:
    /// where a contained resume runs and what bounds its writes.
    // trace:BUG-2157 | ai:claude
    pub(crate) worktree: Option<PathBuf>,
}

fn pick_interactive(limit: usize) -> Result<ResumeTarget> {
    let mut sessions = collect_sessions(limit)?;
    if sessions.is_empty() {
        anyhow::bail!("no past sessions in this directory");
    }
    fill_branches(&mut sessions);
    fill_recent_focus(&mut sessions);
    let labels: Vec<String> = sessions
        .iter()
        .map(|s| {
            format!(
                "{} {:<8}  {:<6}  {:<11}  {:<10}  {:<12}  {:<24}  {}",
                liveness_indicator(s.age_seconds),
                &s.id[..s.id.len().min(8)],
                humanize_age(s.age_seconds),
                s.agent,
                s.role.as_deref().unwrap_or("-"),
                s.spec.as_deref().unwrap_or("-"),
                format_worktree_label(s.last_cwd.as_deref(), s.branch.as_deref(), 24),
                s.recent_focus.as_deref().unwrap_or("-"),
            )
        })
        .collect();

    let pick = crate::prompt_select::select("Resume which session?", labels)
        .with_help_message("arrows to move, type to filter, Enter to resume, Esc to cancel")
        .prompt()
        .context("interactive picker cancelled")?;

    // Map the picked label back to its session id. Labels start with a
    // 1-char liveness glyph + space; the id sits at chars [2..10].
    // STORY-59: account for the leading liveness column.
    let id_prefix: String = pick.chars().skip(2).take(8).collect();
    let chosen = sessions
        .iter()
        .find(|s| s.id.starts_with(id_prefix.trim()))
        .ok_or_else(|| anyhow::anyhow!("could not match picked label back to a session id"))?;
    Ok(ResumeTarget {
        agent: chosen.agent.clone(),
        id: chosen.id.clone(),
        // `collect_sessions` scans this cwd's project store.
        // trace:BUG-2157 | ai:claude
        worktree: std::env::current_dir().ok(),
    })
}

/// Resolve a (possibly truncated) session id prefix to a full id. Errors
/// when the prefix matches zero or multiple sessions.
fn resolve_resume_target(prefix: &str) -> Result<ResumeTarget> {
    // Allow a generous walk — user might have written down the id from a
    // weeks-old session; collect everything and filter.
    let all: Vec<SessionMeta> =
        collect_global_project_sessions(usize::MAX, None, None, Some(prefix), |_| true)?.collect();
    let matches: Vec<&SessionMeta> = all.iter().filter(|s| s.id.starts_with(prefix)).collect();
    match matches.len() {
        0 => anyhow::bail!("no session matches id prefix `{}`", prefix),
        1 => Ok(ResumeTarget {
            agent: matches[0].agent.clone(),
            id: matches[0].id.clone(),
            // `collect_global_project_sessions` scans the main worktree's
            // project store. trace:BUG-2157 | ai:claude
            worktree: crate::find_main_worktree_root().ok(),
        }),
        _ => anyhow::bail!(
            "{} sessions match `{}` — use a longer prefix",
            matches.len(),
            prefix
        ),
    }
}

fn resume_command_for(target: &ResumeTarget) -> (String, Vec<String>) {
    crate::vendors::resume_command(&target.agent, &target.id)
}

// trace:BUG-2386 | ai:claude
fn exec_resume_command(
    target: &ResumeTarget,
    permission_mode: Option<&str>,
    contained: bool,
) -> Result<()> {
    // TASK-2032: this resume's entry point resolves the host main once.
    // trace:TASK-2032 | ai:claude
    let host_main = crate::seat_clone::launcher_main();
    match target.agent.as_str() {
        "claude" => exec_claude_resume(
            host_main.as_deref(),
            &target.id,
            permission_mode,
            contained,
            None,
        ),
        // BUG-2386: exec `codex resume <id>` rather than printing it. The
        // resume is native (no bypass): a resume never widens the posture a
        // fresh `aida session new` would need consent for.
        "codex" => crate::launch::spawn::exec_agent(&mut codex_session_resume_command(
            host_main.as_deref(),
            target,
            contained,
        )?),
        _ => {
            let (program, args) = resume_command_for(target);
            anyhow::bail!(
                "{AGY_RESUME_REFUSAL} (`{} {}` is not a verified resume command)",
                program,
                crate::shell_join_display(&args)
            )
        }
    }
}

/// BUG-2386: the `codex resume <id>` an `aida session resume` of a Codex
/// session runs (exec'd, never printed): native posture (no bypass — a
/// resume never widens what a fresh launch would need consent for), the
/// caller's containment request honoured, in the session's own tree.
// trace:BUG-2386 | ai:claude
pub(crate) fn codex_session_resume_command(
    host_main: Option<&Path>,
    target: &ResumeTarget,
    contained: bool,
) -> Result<std::process::Command> {
    // TASK-2712 F8: the write set of the resumed session's own tree (the
    // process cwd only when that tree is unknown). No role is recorded for
    // a session resume, so no gate ledger.
    // trace:TASK-2712 TASK-2032 | ai:claude
    let cwd = codex_resume_cwd(host_main, target);
    let tree = cwd.clone().unwrap_or_else(headless_worktree_root);
    let mut cmd = build_codex_resume_command(
        &target.id,
        false,
        false,
        None,
        contained,
        CodexLane {
            tree: &tree,
            role: None,
            host_main,
        },
    )?;
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    finalize_codex_resume(&mut cmd, &headless_worktree_root())?;
    Ok(cmd)
}

/// BUG-2386: where a Codex session resumes: the tree of the lease that minted
/// it, else the cwd its rollout recorded, when that still exists.
// trace:BUG-2386 | ai:claude
fn codex_resume_cwd(host_main: Option<&Path>, target: &ResumeTarget) -> Option<PathBuf> {
    resume_lease_tree(host_main, target).or_else(|| {
        let root = crate::vendor_activity::codex_sessions_root()?;
        crate::codex_resume::find_rollout(&root, &target.id)?
            .cwd
            .filter(|p| p.is_dir())
    })
}

/// Replace this process with `claude --resume <id>`. Falls back to spawn
/// + wait on platforms without exec semantics. `permission_mode`, when
///   given, is passed through so a resumed `aida queue work` session keeps
///   the same permission posture as a fresh one. trace:TASK-112 | ai:claude
// trace:BUG-2006 | ai:antigravity
pub(crate) fn build_claude_resume_exec_command(
    host_main: Option<&Path>,
    id: &str,
    permission_mode: Option<&str>,
    contained: bool,
    grant_id: Option<&str>,
) -> Result<std::process::Command> {
    use std::process::Command;
    let mut args = vec!["--resume".to_string(), id.to_string()];
    if let Some(m) = permission_mode {
        args.extend(["--permission-mode".to_string(), m.to_string()]);
    }
    if contained {
        args.extend(claude_contained_flags(host_main)?);
    }
    // BUG-2169 S2: a contained interactive resume runs inside os_wrap too.
    // trace:BUG-2169 | ai:claude
    let (program, args) = interactive_claude_program_and_args(host_main, contained, args)?;
    // trace:BUG-2218 TASK-2328 | ai:codex
    if contained {
        prepare_contained_mailbox_for_launch(host_main, &std::env::current_dir()?)?;
    }
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(grant) = grant_id {
        cmd.env(crate::gate::seat_authority::GRANT_ENV, grant);
    }
    if contained {
        cmd.env("AIDA_CONTAINED", "1");
        // trace:TASK-2631 | ai:codex — do not inherit a native parent's display marker.
        cmd.env("AIDA_CONTAINMENT_POSTURE", "aida");
    } else {
        cmd.env_remove("AIDA_CONTAINED");
        // trace:TASK-2631 | ai:codex
        cmd.env("AIDA_CONTAINMENT_POSTURE", "none");
    }
    Ok(cmd)
}

// trace:TASK-1594 | ai:claude
pub fn exec_claude_resume(
    host_main: Option<&Path>,
    id: &str,
    permission_mode: Option<&str>,
    contained: bool,
    grant_id: Option<&str>,
) -> Result<()> {
    let mut cmd =
        build_claude_resume_exec_command(host_main, id, permission_mode, contained, grant_id)?;
    crate::launch::spawn::exec_agent(&mut cmd)
}

/// TASK-2031: the tree a contained `aida session resume` is bounded to: the
/// worktree of the lease that minted the Claude session `target.id`, when the
/// host lease names one that belongs to the same main checkout (a linked
/// worktree of it or a host-recorded seat clone of it). A lease naming any
/// other tree is ignored, so a forged lease can never widen the boundary to
/// another repository. `None` keeps the process-cwd boundary.
///
/// TASK-2032: the only anchor is `host_main`, the main the launcher resolved
/// on the host side ([`crate::seat_clone::host_main_from`] its own
/// directory); the lease is read from that main's `.aida/sessions`.
/// `target.worktree` (where the transcript was found, which may be a seat
/// clone and so seat-writable) never anchors anything. A `host_main` a seat
/// could have forged ([`crate::seat_clone::plausible_host_main`]) is refused.
// trace:TASK-1952 | trace:TASK-2031 | ai:claude
// trace:TASK-2032 | ai:claude
pub(crate) fn resume_lease_tree(
    host_main: Option<&Path>,
    target: &ResumeTarget,
) -> Option<PathBuf> {
    let main = host_main?.canonicalize().ok()?;
    if crate::seat_clone::main_for(&main) != crate::seat_clone::MainFor::NotAClone
        || !crate::seat_clone::plausible_host_main(&main)
    {
        return None;
    }
    let (_, _, tree, _) = crate::orchestrator_leases::find_orchestrated_lease(&main, &target.id)?;
    if !tree.is_dir() {
        return None;
    }
    let canon = |p: &Path| std::fs::canonicalize(p).ok();
    let tree_main = match crate::seat_clone::host_recorded_clone(&main, &tree) {
        Some(clone) => clone.main,
        // A clone the host main does not vouch for anchors nothing.
        None if crate::seat_clone::main_for(&tree) != crate::seat_clone::MainFor::NotAClone => {
            return None;
        }
        None => crate::main_worktree_root_from(&tree),
    };
    (canon(&tree_main) == Some(main)).then_some(tree)
}
