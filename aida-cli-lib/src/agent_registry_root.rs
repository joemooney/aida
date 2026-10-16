//! `aida agent ls/gc/stop/register`, registered-process validation and child signal forwarding (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

// trace:TASK-542 | ai:antigravity
// trace:TASK-1184 | ai:codex
pub(crate) fn agent_ls(show_all: bool, stale_only: bool, ended_only: bool) -> Result<()> {
    let project_root =
        main_worktree_root_from(&find_aida_project_root_from(&std::env::current_dir()?)?);
    if !show_all && !stale_only && !ended_only {
        let _ = agent_registry::gc_dead_agents(&project_root, false, None);
    }
    let leases = list_leases(&project_root);
    let agent_ctx = build_agent_classify_context(&project_root, &leases);
    let mut agents = if ended_only {
        agent_registry::ended_resumable_agent_views(&project_root, &agent_ctx)
    } else {
        let registry_agents = agent_registry::list_agent_views(&project_root, &agent_ctx);
        merge_agent_views_with_lease_fallback(&project_root, &leases, registry_agents, &agent_ctx)
    };
    if !show_all || stale_only || ended_only {
        agents.retain(|agent| {
            let is_stale = agent.status == agent_registry::AgentStatus::Stale;
            if stale_only {
                is_stale
            } else if ended_only {
                agent.ended_at.is_some() && agent.native_session_id.is_some()
            } else {
                !is_stale
            }
        });
    }

    if output_format_is_json() {
        println!("{}", serde_json::to_string_pretty(&agents)?);
        return Ok(());
    }

    if agents.is_empty() {
        if ended_only {
            println!("No ended resumable agents found.");
        } else if stale_only {
            println!("No stale agents found.");
        } else {
            println!("No active agents found.");
        }
        return Ok(());
    }

    println!(
        "{:<30} {:<10} {:<20} {:<8} {:<11} {:<12} {:<18} {:<6} {:<8} {:<8} {:<24} WORKTREE",
        "NAME/ID", "PID", "TTY", "KIND", "ROLE", "SPEC", "SCOPE", "STATUS", "AGE", "CPU", "DESC"
    );
    let now = chrono::Utc::now();
    for agent in agents {
        let elapsed = agent_registry::humanize_elapsed(agent_registry::elapsed_secs_clamped(
            now,
            agent.last_active_at,
        ));
        let identity = if let Some(ref name) = agent.name {
            name.clone()
        } else if agent.source == "lease" {
            let short = agent.id.trim_start_matches("lease-");
            let short = &short[..short.len().min(8)];
            format!("{}#{}", agent.agent_type, short)
        } else {
            format!("{}#{}", agent.agent_type, agent.pid)
        };
        let pid_str = if agent.source == "lease" {
            "-".to_string()
        } else {
            agent.pid.to_string()
        };
        let scope = agent_registry::agent_scope(
            agent.role.as_deref(),
            agent.current_spec.as_deref(),
            &agent.worktree_path,
        );
        let desc = agent.description.as_deref().unwrap_or("(none)");
        let terminal = agent_registry::terminal_cell(agent.terminal.as_ref(), agent.tty.as_deref());
        // STORY-528: surface paused-availability inline after the worktree.
        let paused_note = match agent_registry::paused_glyph(&agent) {
            Some(g) => format!("  {g}"),
            None => String::new(),
        };
        if ended_only {
            let ended = agent
                .ended_at
                .map(|at| {
                    agent_registry::humanize_elapsed(agent_registry::elapsed_secs_clamped(now, at))
                })
                .unwrap_or_else(|| "?".to_string());
            println!(
                "{:<30} {:<10} {:<20} {:<11} {:<12} {:<6} {:<8} {}  resume:{}{}",
                identity,
                pid_str,
                terminal,
                agent.role.as_deref().unwrap_or("(none)"),
                agent.current_spec.as_deref().unwrap_or("(none)"),
                "ended",
                format!("({ended})"),
                agent.worktree_path.display(),
                agent.native_session_id.as_deref().unwrap_or("(unknown)"),
                agent
                    .resumed_from
                    .as_deref()
                    .map(|id| format!("  from:{id}"))
                    .unwrap_or_default()
            );
        } else {
            // BUG-1701 AC5: CPU consumed is what separates a seat still working from one that
            // finished its turn and is idling at a prompt — 3-46 SECONDS across hours, for seats
            // the status column called `busy`. Process-backed rows only; a lease has no pid.
            // trace:BUG-1701 | ai:claude
            let cpu = if agent.source == "lease" {
                "-".to_string()
            } else {
                agent_registry::process_cpu_secs(agent.pid)
                    .map(|secs| agent_registry::humanize_elapsed(secs))
                    .unwrap_or_else(|| "?".to_string())
            };
            println!(
                "{:<30} {:<10} {:<20} {:<8} {:<11} {:<12} {:<18} {:<6} {:<8} {:<8} {:<24} {}{}",
                identity,
                pid_str,
                terminal,
                agent_registry::view_kind(&agent),
                agent.role.as_deref().unwrap_or("(none)"),
                agent.current_spec.as_deref().unwrap_or("(none)"),
                scope,
                agent.status.as_str(),
                format!("({elapsed})"),
                cpu,
                desc,
                agent.worktree_path.display(),
                paused_note
            );
        }
    }
    Ok(())
}

// trace:TASK-1184 | ai:codex
pub(crate) fn agent_gc(dry_run: bool, older_than: Option<u64>) -> Result<()> {
    let project_root =
        main_worktree_root_from(&find_aida_project_root_from(&std::env::current_dir()?)?);
    let report = agent_registry::gc_dead_agents(&project_root, dry_run, older_than)?;
    let verb = if dry_run { "would remove" } else { "removed" };
    println!(
        "Agent registry gc: {} {} dead entr{}{}.",
        verb,
        report.removed_count(),
        if report.removed_count() == 1 {
            "y"
        } else {
            "ies"
        },
        if report.retained.is_empty() {
            String::new()
        } else {
            format!(
                "; retained {} ended/resumable entr{}",
                report.retained.len(),
                if report.retained.len() == 1 {
                    "y"
                } else {
                    "ies"
                }
            )
        }
    );
    for row in &report.removed {
        println!(
            "  {} {}#{} ({}) {}",
            if dry_run { "would remove" } else { "removed" },
            row.agent_type,
            row.pid,
            row.source,
            row.path.display()
        );
    }
    for row in &report.retained {
        println!(
            "  retained {}#{} ({}) — resume id {}",
            row.agent_type,
            row.pid,
            row.source,
            row.resumable_session_id.as_deref().unwrap_or("(unknown)")
        );
    }
    Ok(())
}

// trace:TASK-542 | ai:antigravity
// trace:BUG-2148 | ai:antigravity
pub(crate) fn agent_stop(name: &str) -> Result<()> {
    let name_trimmed = name.trim();
    if name_trimmed.is_empty() {
        anyhow::bail!("stop target name cannot be empty");
    }
    let project_root =
        main_worktree_root_from(&find_aida_project_root_from(&std::env::current_dir()?)?);
    agent_stop_in_project(&project_root, name_trimmed)
}

// trace:BUG-2148 trace:TASK-1782 | ai:antigravity
fn join_pids(pids: impl IntoIterator<Item = u32>, sep: &str) -> String {
    pids.into_iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(sep)
}

// trace:BUG-2148 | ai:antigravity
pub(crate) fn agent_stop_in_project(
    project_root: &std::path::Path,
    name_trimmed: &str,
) -> Result<()> {
    let leases = list_leases(project_root);
    let agent_ctx = build_agent_classify_context(project_root, &leases);
    let registry_agents = agent_registry::list_agent_views(project_root, &agent_ctx);

    // BUG-1703: the registry is PID-keyed, so relaunching a name leaves SEVERAL entries under it.
    // Taking only the first match signalled a stale pid, reaped that entry, and printed success
    // while the real seat kept running and kept its caller blocked — observed on 3 of 8 stops.
    // trace:BUG-1703 | ai:claude
    let matches: Vec<_> = registry_agents
        .into_iter()
        .filter(|agent| {
            if let Some(ref active_name) = agent.name {
                active_name.eq_ignore_ascii_case(name_trimmed)
            } else {
                let fallback_id = format!("{}#{}", agent.agent_type, agent.pid);
                let fallback_id_alt = format!("{}-{}", agent.agent_type, agent.pid);
                fallback_id.eq_ignore_ascii_case(name_trimmed)
                    || fallback_id_alt.eq_ignore_ascii_case(name_trimmed)
            }
        })
        .collect();

    if matches.is_empty() {
        anyhow::bail!("no active agent found with name '{}'", name_trimmed);
    }

    // BUG-2148: Partition matches into:
    // 1) genuinely gone from the OS -> reap stale entry.
    // 2) alive in OS, but start-time identity is unconfirmed:
    //    - Recycled (recorded start identity differs from live process): agent is gone; suggest `aida agent gc`.
    //    - Unknown (pid_start missing/unreadable): may be the agent; suggest `ps -p <pid>`, then manual kill.
    //    - Corrupt (pid <= 1): invalid PID in registry entry; remove manually.
    //    Neither is signalled, and registry entries are kept in place (TASK-1782).
    // 3) confirmed live matching process -> signal with grace.
    // trace:BUG-2148 trace:TASK-1782 | ai:antigravity
    let mut confirmed = Vec::new();
    let mut recycled = Vec::new();
    let mut unknown = Vec::new();
    let mut corrupt = Vec::new();
    for agent in &matches {
        if !aida_core::liveness::pid_is_alive(agent.pid) {
            println!(
                "  pid {} was already gone — reaping its stale registry entry",
                agent.pid
            );
            let _ = agent_registry::remove_agent(project_root, &agent.agent_type, agent.pid);
        } else if agent.pid <= 1 {
            eprintln!(
                "  pid {} was not signalled: registry entry has an invalid PID (<= 1). \
                 Remove the corrupt entry manually.",
                agent.pid
            );
            corrupt.push(agent);
        } else if agent.pid_start.as_deref().is_some_and(|start| {
            aida_core::liveness::process_identity_matches_strict(agent.pid, start)
        }) {
            confirmed.push(agent);
        } else if agent.pid_start.as_deref().is_some_and(|start| {
            !aida_core::liveness::process_identity_is_alive(agent.pid, Some(start))
        }) {
            eprintln!(
                "  pid {} was not signalled: the recorded agent process is gone and its PID has been \
                 recycled onto an unrelated process. Run `aida agent gc` to reap stale registry entries.",
                agent.pid
            );
            recycled.push(agent);
        } else {
            eprintln!(
                "  pid {} was not signalled: its process identity could not be confirmed, \
                 so it may be an unrelated process. Inspect with `ps -p {}`, then kill manually if it is the agent.",
                agent.pid, agent.pid
            );
            unknown.push(agent);
        }
    }

    if confirmed.is_empty() && recycled.is_empty() && unknown.is_empty() && corrupt.is_empty() {
        println!(
            "{} Agent '{}' was already stopped; {} stale registry entr{} reaped.",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            name_trimmed,
            matches.len(),
            if matches.len() == 1 { "y" } else { "ies" }
        );
        return Ok(());
    }

    if !confirmed.is_empty() {
        println!(
            "Stopping agent '{}' (PID{} {})...",
            name_trimmed,
            if confirmed.len() == 1 { "" } else { "s" },
            join_pids(confirmed.iter().map(|a| a.pid), ", ")
        );
        let targets: Vec<status_sections::TerminateTarget> = confirmed
            .iter()
            .map(|a| status_sections::TerminateTarget::new(a.pid, a.pid_start.clone()))
            .collect();
        let survivors = status_sections::terminate_pids_with_grace(&targets, 5);

        for agent in confirmed.iter().filter(|a| !survivors.contains(&a.pid)) {
            let _ = agent_registry::remove_agent(project_root, &agent.agent_type, agent.pid);
        }

        // AC1: never claim a stop that did not happen.
        if !survivors.is_empty() {
            anyhow::bail!(
                "agent `{}` is STILL ALIVE after SIGTERM and SIGKILL: pid(s) {}. Its registry entries \
                 were left in place. Check for a process that re-parents or respawns, and inspect the \
                 tree with `ps -o pid,ppid,time,args -p {}`.",
                name_trimmed,
                join_pids(survivors.iter().copied(), ", "),
                join_pids(survivors.iter().copied(), ",")
            );
        }
    }

    if !recycled.is_empty() || !unknown.is_empty() || !corrupt.is_empty() {
        let mut msg = format!("agent `{}` was not stopped:", name_trimmed);
        if !recycled.is_empty() {
            msg.push_str(&format!(
                "\n  recycled PID(s) {}: the recorded agent process is gone. Run `aida agent gc` to clean up.",
                join_pids(recycled.iter().map(|a| a.pid), ", ")
            ));
        }
        if !unknown.is_empty() {
            let pids = join_pids(unknown.iter().map(|a| a.pid), ", ");
            let pids_comma = join_pids(unknown.iter().map(|a| a.pid), ",");
            msg.push_str(&format!(
                "\n  unknown identity for live PID(s) {}: inspect with `ps -p {}`, then kill manually if it is the agent.",
                pids, pids_comma
            ));
        }
        if !corrupt.is_empty() {
            msg.push_str(&format!(
                "\n  corrupt registry entry with invalid PID(s) {}: remove manually.",
                join_pids(corrupt.iter().map(|a| a.pid), ", ")
            ));
        }
        msg.push_str("\nIts registry entries were left in place.");
        anyhow::bail!("{msg}");
    }

    println!(
        "{} Agent '{}' stopped (pid{} {}).",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        name_trimmed,
        if confirmed.len() == 1 { "" } else { "s" },
        join_pids(confirmed.iter().map(|a| a.pid), ", ")
    );
    Ok(())
}

// trace:TASK-543 | ai:codex
pub(crate) fn agent_register(
    pid: u32,
    agent_type: &str,
    role: &str,
    spec: Option<&str>,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<()> {
    let project_root =
        main_worktree_root_from(&find_aida_project_root_from(&std::env::current_dir()?)?);
    let agent_type = validate_registered_agent_type(agent_type)?;
    let role = validate_registered_agent_role(role)?;
    let spec = match spec {
        Some(spec) => {
            let spec = spec.trim().to_string();
            if !looks_like_spec_id(&spec) {
                anyhow::bail!("--spec must be a SPEC-ID like TASK-543, got `{spec}`");
            }
            Some(spec)
        }
        None => None,
    };
    let name = match name {
        Some(name) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                anyhow::bail!("--name cannot be empty");
            }
            Some(trimmed.to_string())
        }
        None => None,
    };
    let description =
        agent_registry::normalize_description(description.map(std::string::ToString::to_string));

    validate_register_pid(pid)?;
    let worktree_path = registered_process_cwd(pid).unwrap_or_else(|| project_root.clone());
    let entry = agent_registry::register_existing_agent(
        &project_root,
        &agent_type,
        pid,
        role,
        spec,
        worktree_path,
        name,
        description,
    )?;
    println!(
        "{} registered {}#{} ({}) at {}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        entry.agent_type,
        entry.pid,
        entry.role.as_deref().unwrap_or("(none)"),
        entry.worktree_path.display()
    );
    Ok(())
}

// trace:STORY-791 | ai:codex
pub(crate) fn agent_describe(agent: &str, description: &str) -> Result<()> {
    let project_root =
        main_worktree_root_from(&find_aida_project_root_from(&std::env::current_dir()?)?);
    let entry = agent_registry::describe_agent(&project_root, agent, description.to_string())?;
    let identity = entry
        .name
        .clone()
        .unwrap_or_else(|| format!("{}#{}", entry.agent_type, entry.pid));
    println!(
        "{} {} described.",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        identity.cyan()
    );
    Ok(())
}

// trace:TASK-543 | ai:codex
pub(crate) fn validate_registered_agent_type(raw: &str) -> Result<String> {
    let normalized = agent_registry::normalize_agent_type(raw.to_string());
    match normalized.as_str() {
        "claude" | "codex" | "antigravity" | "web" => Ok(normalized),
        _ => anyhow::bail!(
            "--type must be one of: codex, claude, antigravity, web (got `{}`)",
            raw
        ),
    }
}

// trace:TASK-543 | ai:codex
pub(crate) fn validate_register_pid(pid: u32) -> Result<()> {
    if pid == 0 {
        anyhow::bail!("pid must be a positive process id");
    }
    if !process_probe::pid_is_alive(pid) {
        anyhow::bail!("pid {pid} is not alive");
    }
    validate_register_pid_owner(pid)
}

#[cfg(unix)]
// trace:TASK-543 | ai:codex
pub(crate) fn validate_register_pid_owner(pid: u32) -> Result<()> {
    let uid = proc_status_uid(pid)
        .with_context(|| format!("reading /proc/{pid}/status to verify process owner"))?;
    let current = unsafe { libc::geteuid() } as u32;
    if uid != current {
        anyhow::bail!("pid {pid} is owned by uid {uid}, not current uid {current}");
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn validate_register_pid_owner(_pid: u32) -> Result<()> {
    // Non-Unix process-owner APIs differ by platform. `pid_is_alive` still
    // prevents stale registrations; Windows owner enforcement can be tightened
    // when a native user-token helper lands.
    Ok(())
}

#[cfg(unix)]
// trace:TASK-543 | ai:codex
pub(crate) fn proc_status_uid(pid: u32) -> Result<u32> {
    let body = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    parse_proc_status_uid(&body).ok_or_else(|| anyhow::anyhow!("Uid line missing"))
}

#[cfg(unix)]
// trace:TASK-543 | ai:codex
pub(crate) fn parse_proc_status_uid(body: &str) -> Option<u32> {
    body.lines().find_map(|line| {
        let rest = line.strip_prefix("Uid:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

#[cfg(unix)]
// trace:TASK-543 | ai:codex
pub(crate) fn registered_process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(not(unix))]
pub(crate) fn registered_process_cwd(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

pub(crate) struct ChildSignalForwarder {
    pub(crate) stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl ChildSignalForwarder {
    pub(crate) fn stop(self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(unix)]
pub(crate) fn install_child_signal_forwarder(child_pid: u32) -> Result<ChildSignalForwarder> {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_for_thread = std::sync::Arc::clone(&stop);
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ])
    .context("installing agent launcher signal handlers")?;
    std::thread::spawn(move || {
        for signal in signals.forever() {
            if stop_for_thread.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            forward_signal_to_child(child_pid, signal);
        }
    });
    Ok(ChildSignalForwarder { stop })
}

#[cfg(not(unix))]
pub(crate) fn install_child_signal_forwarder(_child_pid: u32) -> Result<ChildSignalForwarder> {
    // Windows delivers console control events to foreground console processes
    // differently from Unix signals; the launcher still waits for Claude and
    // performs registry cleanup when the child exits.
    Ok(ChildSignalForwarder {
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    })
}

#[cfg(unix)]
pub(crate) fn forward_signal_to_child(child_pid: u32, signal: i32) {
    unsafe {
        libc::kill(child_pid as libc::pid_t, signal);
    }
}

#[cfg(not(unix))]
pub(crate) fn forward_signal_to_child(_child_pid: u32, _signal: i32) {
    // On Windows, console control events are delivered to the foreground
    // process group; the launcher still waits and deregisters the child.
}

pub(crate) fn find_aida_project_root_from(start: &std::path::Path) -> Result<std::path::PathBuf> {
    let mut current = if start.is_file() {
        start.parent().unwrap_or(start).to_path_buf()
    } else {
        start.to_path_buf()
    };
    current = current
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", start.display()))?;
    loop {
        // BUG-1598: never adopt a temp root itself as the project root —
        // see `aida_core::store_locate` for the shared rationale.
        // trace:BUG-1598 | ai:claude
        if aida_core::store_locate::is_system_temp_dir(&current) {
            anyhow::bail!(
                "not inside an AIDA project (no .aida/config.toml found from {})",
                start.display()
            );
        }
        if current.join(".aida").join("config.toml").exists() {
            return Ok(current);
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => {
                anyhow::bail!(
                    "not inside an AIDA project (no .aida/config.toml found from {})",
                    start.display()
                )
            }
        }
    }
}

// trace:BUG-1093 | ai:codex
pub(crate) fn agent_launch_project_root_from(
    discovered_root: &std::path::Path,
) -> std::path::PathBuf {
    let main_root = main_worktree_root_from(discovered_root);
    if is_git_metadata_path(&main_root) || !main_root.join(".aida").join("config.toml").exists() {
        return discovered_root.to_path_buf();
    }
    main_root
}

// trace:BUG-1093 | ai:codex
pub(crate) fn ensure_agent_launch_cwd_not_git_metadata(path: &std::path::Path) -> Result<()> {
    if is_git_metadata_path(path) {
        anyhow::bail!(
            "refusing to launch agent from Git metadata directory {}; \
             run `aida agent new` from the repository working tree instead",
            path.display()
        );
    }
    Ok(())
}

// trace:BUG-1093 | ai:codex
pub(crate) fn is_git_metadata_path(path: &std::path::Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == std::ffi::OsStr::new(".git"))
}

pub(crate) fn find_executable_on_path(name: &str) -> Option<std::path::PathBuf> {
    let candidate = std::path::Path::new(name);
    if candidate.components().count() > 1 {
        return is_executable_file(candidate).then(|| candidate.to_path_buf());
    }

    let path = std::env::var_os("PATH")?;
    let extensions = executable_extensions();
    for dir in std::env::split_paths(&path) {
        for ext in &extensions {
            let path = dir.join(format!("{name}{ext}"));
            if is_executable_file(&path) {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(windows)]
pub(crate) fn executable_extensions() -> Vec<String> {
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.BAT;.CMD".to_string());
    let mut exts = vec![String::new()];
    exts.extend(pathext.split(';').map(|ext| ext.to_string()));
    exts
}

#[cfg(not(windows))]
pub(crate) fn executable_extensions() -> Vec<String> {
    vec![String::new()]
}

pub(crate) fn is_executable_file(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
#[path = "tests/agent_launcher_tests.rs"]
mod agent_launcher_tests;

// trace:TASK-1482 | ai:claude
#[cfg(test)]
#[path = "tests/presentation_env_tests.rs"]
mod presentation_env_tests;
