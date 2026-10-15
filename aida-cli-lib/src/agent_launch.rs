//! Session/terminal/agent dispatchers, dispatch health, agent new/resume and tracked agent launch (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

pub(crate) fn validate_session_start_args(args: &[String]) -> Result<()> {
    let has_owns = args
        .iter()
        .any(|arg| arg == "--owns" || arg.starts_with("--owns="));
    let has_spec = args
        .iter()
        .any(|arg| arg == "--spec" || arg.starts_with("--spec="));
    if has_owns && has_spec {
        return Err(anyhow::anyhow!(
            "error: Cannot specify both --owns and --spec"
        ));
    }
    Ok(())
}

/// trace:FR-1-043 | ai:claude
pub(crate) fn handle_session_command(cmd: &SessionCommand) -> Result<()> {
    match cmd {
        // trace:BUG-522 | ai:claude — renamed from `session list`;
        // `list` stays a deprecated clap alias routing to the same
        // handler. Emit a one-line pointer when the old name is used.
        SessionCommand::Conversations {
            limit,
            no_color,
            all,
        } => {
            if std::env::args().any(|a| a == "list") {
                eprintln!(
                    "note: `aida session list` is deprecated — use `aida session conversations` (the historical conversation view). `aida session leases` shows live work leases."
                );
            }
            session::list(*limit, *no_color, *all)
        }
        SessionCommand::Resume { id, limit } => session::resume(id.clone(), *limit),
        SessionCommand::Focus { target } => terminal_cmd::focus_session(target),
        SessionCommand::Send {
            target,
            text,
            enter,
            mail,
        } => terminal_cmd::send_session(target, text, *enter, *mail),
        SessionCommand::New {
            title,
            permission_mode,
            sandbox,
            role,
            no_title,
        } => {
            // STORY-495: faithful default — resolve to None (native) unless an
            // explicit `--permission-mode` or the uniform `[agents] bypass`
            // knob says otherwise.
            let mut mode = resolve_interactive_launch_mode(permission_mode.as_deref(), *sandbox)?;
            let mut argv = mode
                .mode
                .iter()
                .flat_map(|m| ["--permission-mode".to_string(), m.clone()])
                .collect::<Vec<_>>();
            gate_agent_bypass(
                &find_main_worktree_root().unwrap_or(std::env::current_dir()?),
                "claude",
                &mut argv,
                false,
                permission_mode.is_some(),
            )?;
            if mode.mode.as_deref() == Some("bypassPermissions")
                && !argv.iter().any(|a| a == "bypassPermissions")
            {
                mode.mode = None;
            }
            if mode.mode.is_none() && !mode.contained {
                maybe_show_faithful_launcher_notice();
            }
            session::new_session(
                title.clone(),
                mode.mode.as_deref(),
                role.clone(),
                None,
                mode.contained,
                !*no_title,
            )
        }
        SessionCommand::Start {
            owns,
            branch,
            base,
            reuse_branch,
            path,
            forge,
            branch_style,
            launch,
            title,
            no_title,
            name,
            permission_mode,
            sandbox,
            role,
            force_claim,
            pool,
            no_pool,
        } => {
            let args: Vec<String> = std::env::args().collect();
            validate_session_start_args(&args)?;
            // STORY-495: faithful default for the `--launch` path.
            let mut mode = resolve_interactive_launch_mode(permission_mode.as_deref(), *sandbox)?;
            if *launch {
                let mut argv = mode
                    .mode
                    .iter()
                    .flat_map(|m| ["--permission-mode".to_string(), m.clone()])
                    .collect::<Vec<_>>();
                gate_agent_bypass(
                    &find_main_worktree_root().unwrap_or(std::env::current_dir()?),
                    "claude",
                    &mut argv,
                    false,
                    permission_mode.is_some(),
                )?;
                if mode.mode.as_deref() == Some("bypassPermissions")
                    && !argv.iter().any(|a| a == "bypassPermissions")
                {
                    mode.mode = None;
                }
            }
            if *launch && mode.mode.is_none() && !mode.contained {
                maybe_show_faithful_launcher_notice();
            }
            // STORY-714: --pool / --no-pool win over the config; None = config.
            let use_pool = if *pool {
                Some(true)
            } else if *no_pool {
                Some(false)
            } else {
                None
            };
            session_start(
                owns,
                branch.as_deref(),
                base.as_deref(),
                *reuse_branch,
                path.as_deref(),
                forge.as_deref(),
                branch_style,
                *launch,
                title.clone(),
                !*no_title,
                name.clone(),
                mode.mode.as_deref(),
                mode.contained,
                role.clone(),
                *force_claim,
                use_pool,
            )
        }
        SessionCommand::HarnessWorktreeRegister {
            agent_id,
            cwd,
            agent_type,
            branch,
            scope,
        } => session_harness_worktree_register(
            agent_id,
            cwd,
            agent_type.as_deref(),
            branch.as_deref(),
            scope.as_deref(),
        ),
        SessionCommand::HarnessWorktreeRelease { agent_id } => {
            session_harness_worktree_release(agent_id)
        }
        // trace:TASK-1454 | ai:claude
        SessionCommand::PendingApprovalSet {
            session,
            tool,
            message,
        } => {
            let project_root = find_main_worktree_root()?;
            pending_approval::write_marker(
                &project_root,
                session,
                tool.as_deref(),
                message.as_deref(),
                chrono::Utc::now(),
            )?;
            println!("recorded pending-approval marker for session {session}");
            Ok(())
        }
        // trace:TASK-1454 | ai:claude
        SessionCommand::PendingApprovalClear { session } => {
            let project_root = find_main_worktree_root()?;
            pending_approval::clear_marker(&project_root, session)?;
            println!("cleared pending-approval marker for session {session}");
            Ok(())
        }
        SessionCommand::End {
            id,
            spec,
            branch,
            yes,
            force,
            purge_cc,
            wait_ci,
            watch_ci,
            skip_ci,
            return_to_pool,
            remove,
            include_in_use,
        } => session_end(
            id.as_deref(),
            spec.as_deref(),
            branch.as_deref(),
            *yes,
            *force,
            *purge_cc,
            *wait_ci,
            *watch_ci,
            *skip_ci,
            *return_to_pool,
            *remove,
            *include_in_use,
        ),
        SessionCommand::Leases {
            verbose,
            all,
            json,
            prune_stale,
            yes,
        } => {
            // BUG-1764: `--prune-stale` is a MUTATION, not a listing — it
            // releases foreign claims the staleness predicate reports dead.
            // Route it before the read-only renderer so the two never
            // interleave output. trace:BUG-1764 | ai:claude
            if *prune_stale {
                session_leases_prune_stale(*yes)
            } else {
                session_leases(*verbose, *all, *json)
            }
        }
        SessionCommand::Show { id, plan } => session_show(id.as_deref(), *plan),
        SessionCommand::Handoff {
            check,
            write,
            show,
            seat,
        } => {
            if write.is_some() || *show {
                session_handoff_seat(write.as_deref(), *show, seat.as_deref())
            } else {
                session_handoff_check(*check)
            }
        }
        SessionCommand::Prune {
            days,
            dry_run,
            yes,
            orphans,
            escalations,
        } => session_prune(*days, *dry_run, *yes, *orphans, *escalations),
        SessionCommand::Gc { dry_run, yes } => session_gc(*dry_run, *yes),
        // trace:TASK-1177 | ai:claude
        SessionCommand::Reap { dry_run, yes, json } => {
            session_reap::run_session_reap(session_reap::ReapOptions {
                dry_run: *dry_run,
                yes: *yes,
                json: *json,
                quiet_when_empty: false,
            })
        }
        SessionCommand::Manifest { cmd } => session_manifest_dispatch(cmd),
    }
}

// trace:STORY-995 | ai:codex
pub(crate) fn handle_terminal_command(cmd: &TerminalCommand) -> Result<()> {
    match cmd {
        TerminalCommand::Install { target } => terminal_cmd::install_terminal(target),
    }
}

pub(crate) fn handle_agent_command(cmd: &AgentCommand) -> Result<()> {
    match cmd {
        // trace:TASK-837 | ai:claude — bare `aida agent new` (no agent-type
        // subcommand): at a TTY, open an arrow-key picker of the agent types,
        // then dispatch the chosen type with its default options. Non-TTY keeps
        // clap's help-on-missing-subcommand behavior (don't block scripts).
        AgentCommand::New { command } => match command {
            Some(c) => dispatch_agent_new(c),
            // Esc/cancel (or non-TTY) → clean no-op exit, same as the role picker.
            None => match pick_agent_type_interactively()? {
                Some(c) => dispatch_agent_new(&c),
                None => Ok(()),
            },
        },
        AgentCommand::Register {
            pid,
            agent_type,
            role,
            spec,
            name,
            description,
        } => agent_register(
            *pid,
            agent_type,
            role,
            spec.as_deref(),
            name.as_deref(),
            description.as_deref(),
        ),
        AgentCommand::Ls { all, stale, ended } => agent_ls(*all, *stale, *ended),
        AgentCommand::Status { all, stale, ended } => agent_ls(*all, *stale, *ended),
        AgentCommand::Gc {
            dry_run,
            older_than,
        } => agent_gc(*dry_run, *older_than),
        AgentCommand::DispatchHealth { force } => agent_dispatch_health(*force),
        AgentCommand::Pause {
            agent,
            reason,
            resets,
        } => agent_pause(agent, reason, resets.as_deref()),
        AgentCommand::Resume { agent } => agent_resume(agent),
        AgentCommand::Describe { agent, description } => agent_describe(agent, description),
        AgentCommand::Stop { name } => agent_stop(name),
        AgentCommand::ListRoles { json } => handle_agent_list_roles(*json),
    }
}

// trace:STORY-759 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispatchHealthState {
    Moving,
    Stalled,
    Salvageable,
}

impl DispatchHealthState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Moving => "moving",
            Self::Stalled => "stalled",
            Self::Salvageable => "salvageable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DispatchReportInput {
    pub(crate) agent: String,
    pub(crate) agent_type: String,
    pub(crate) spec: Option<String>,
    pub(crate) status: agent_registry::AgentStatus,
    pub(crate) liveness_stalled: Option<bool>,
    pub(crate) paused: bool,
    pub(crate) worktree: std::path::PathBuf,
    pub(crate) branch: Option<String>,
    pub(crate) dirty: bool,
    pub(crate) ahead_main: Option<u32>,
    pub(crate) ahead_upstream: Option<u32>,
    pub(crate) has_upstream: bool,
    pub(crate) pending_briefs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DispatchReportRow {
    pub(crate) agent: String,
    pub(crate) agent_type: String,
    pub(crate) spec: String,
    pub(crate) state: DispatchHealthState,
    pub(crate) worktree: String,
    pub(crate) branch: String,
    pub(crate) dirty: bool,
    pub(crate) ahead_main: Option<u32>,
    pub(crate) pushed: bool,
    pub(crate) pending_briefs: usize,
    pub(crate) guidance: String,
}

pub(crate) fn dispatch_report_row(
    input: &DispatchReportInput,
    paused_vendor_types: &[String],
    force_fallback: bool,
) -> DispatchReportRow {
    let state = if input.status == agent_registry::AgentStatus::Stale && input.dirty {
        DispatchHealthState::Salvageable
    } else if input.liveness_stalled == Some(true) {
        DispatchHealthState::Stalled
    } else {
        DispatchHealthState::Moving
    };
    let spec = input.spec.clone().unwrap_or_else(|| "(none)".to_string());
    let branch = input
        .branch
        .clone()
        .unwrap_or_else(|| "(unknown)".to_string());
    let pushed = input.ahead_main.unwrap_or(0) > 0
        && input.has_upstream
        && input.ahead_upstream.unwrap_or(0) == 0;
    let fallback = select_dispatch_fallback(
        &input.agent_type,
        &["codex", "antigravity", "claude"],
        paused_vendor_types,
        force_fallback,
    );
    let mut guidance = match state {
        DispatchHealthState::Moving => {
            if input.paused {
                "paused marker set; do not route new fallback unless forced".to_string()
            } else {
                "watch for HEAD or dirty-status progress".to_string()
            }
        }
        DispatchHealthState::Salvageable => format!(
            "dead agent with dirty worktree; salvage in `{}` and commit before rebriefing",
            input.worktree.display()
        ),
        DispatchHealthState::Stalled => {
            "dead agent with no worktree progress; resume from durable branch state".to_string()
        }
    };
    if pushed && spec != "(none)" {
        guidance = format!(
            "fresh-launch resume: aida agent new {} --spec {} --cwd {}",
            input.agent_type,
            spec,
            input.worktree.display()
        );
    } else if matches!(state, DispatchHealthState::Stalled) {
        if let Some(vendor) = fallback {
            guidance = dispatch_fallback_guidance(&vendor, &spec);
        }
    }

    DispatchReportRow {
        agent: input.agent.clone(),
        agent_type: input.agent_type.clone(),
        spec,
        state,
        worktree: input.worktree.display().to_string(),
        branch,
        dirty: input.dirty,
        ahead_main: input.ahead_main,
        pushed,
        pending_briefs: input.pending_briefs,
        guidance,
    }
}

pub(crate) fn select_dispatch_fallback(
    current_vendor: &str,
    candidates: &[&str],
    paused_vendor_types: &[String],
    force: bool,
) -> Option<String> {
    candidates
        .iter()
        .copied()
        .filter(|v| *v != current_vendor)
        .find(|v| force || !paused_vendor_types.iter().any(|p| p == v))
        .map(str::to_string)
}

pub(crate) fn dispatch_fallback_guidance(vendor: &str, spec: &str) -> String {
    match compete::vendor_adapter(vendor) {
        Some(compete::VendorAdapter::HumanBriefed) => {
            format!("human-briefed fallback: aida brief {vendor} {spec} --notify")
        }
        Some(adapter) => {
            let argv = compete::headless_argv(&adapter, "BRIEF")
                .unwrap_or_default()
                .into_iter()
                .take_while(|arg| arg != "BRIEF")
                .collect::<Vec<_>>()
                .join(" ");
            format!("headless fallback: aida agent new {vendor} --spec {spec} ({argv})")
        }
        None => format!("unknown fallback vendor `{vendor}`"),
    }
}

// The no-progress liveness DELTA-comparator: given a prior and a current
// snapshot, decide whether an agent is stalled (child reaped AND neither HEAD
// nor the dirty fingerprint moved). Fully unit-tested, but not yet wired into
// the `dispatch health` report through per-clone snapshot persistence.
// trace:STORY-759 | ai:codex+claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(crate) struct DispatchLivenessSnapshot {
    pub(crate) child_reaped: bool,
    pub(crate) head: String,
    pub(crate) dirty_fingerprint: String,
}

pub(crate) fn dispatch_liveness_stalled(
    previous: &DispatchLivenessSnapshot,
    current: &DispatchLivenessSnapshot,
) -> bool {
    current.child_reaped
        && previous.head == current.head
        && previous.dirty_fingerprint == current.dirty_fingerprint
}

pub(crate) type DispatchSnapshotLedger =
    std::collections::BTreeMap<String, DispatchLivenessSnapshot>;

pub(crate) fn dispatch_snapshot_path(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida").join("dispatch-snapshots.json")
}

pub(crate) fn load_dispatch_snapshots(project_root: &std::path::Path) -> DispatchSnapshotLedger {
    let path = dispatch_snapshot_path(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return DispatchSnapshotLedger::new();
    };
    serde_json::from_str(&body).unwrap_or_default()
}

pub(crate) fn persist_dispatch_snapshots(
    project_root: &std::path::Path,
    snapshots: &DispatchSnapshotLedger,
) {
    let path = dispatch_snapshot_path(project_root);
    let Some(parent) = path.parent() else {
        return;
    };
    // trace:TASK-1094 | ai:codex
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(body) = serde_json::to_string_pretty(snapshots) else {
        return;
    };
    let _ = std::fs::write(path, body);
}

pub(crate) fn dispatch_liveness_snapshot(
    status: agent_registry::AgentStatus,
    head: String,
    dirty_fingerprint: String,
) -> DispatchLivenessSnapshot {
    DispatchLivenessSnapshot {
        child_reaped: status == agent_registry::AgentStatus::Stale,
        head,
        dirty_fingerprint,
    }
}

pub(crate) fn render_dispatch_toon(rows: &[DispatchReportRow]) -> String {
    let body: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.agent.clone(),
                r.spec.clone(),
                r.state.as_str().to_string(),
                r.agent_type.clone(),
                r.branch.clone(),
                r.dirty.to_string(),
                r.ahead_main.map(|n| n.to_string()).unwrap_or_default(),
                r.pushed.to_string(),
                r.pending_briefs.to_string(),
                r.guidance.clone(),
            ]
        })
        .collect();
    crate::toon::table_raw(
        "dispatch",
        &[
            "agent",
            "spec",
            "state",
            "vendor",
            "branch",
            "dirty",
            "ahead_main",
            "pushed",
            "pending_briefs",
            "guidance",
        ],
        &body,
    )
}

pub(crate) fn dispatch_worktree_dirty_fingerprint(path: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}

pub(crate) fn agent_identity(view: &agent_registry::AgentRegistryView) -> String {
    view.name
        .clone()
        .unwrap_or_else(|| format!("{}#{}", view.agent_type, view.pid))
}

pub(crate) fn agent_dispatch_health(force_fallback: bool) -> Result<()> {
    let project_root =
        main_worktree_root_from(&find_aida_project_root_from(&std::env::current_dir()?)?);
    let leases = list_leases(&project_root);
    let ctx = build_agent_classify_context(&project_root, &leases);
    let registry_agents = agent_registry::list_agent_views(&project_root, &ctx);
    let agents =
        merge_agent_views_with_lease_fallback(&project_root, &leases, registry_agents, &ctx);
    let pending: std::collections::HashMap<String, usize> =
        collect_pending_brief_counts(&project_root)
            .into_iter()
            .collect();
    let paused_vendor_types: Vec<String> = agents
        .iter()
        .filter(|a| a.availability.is_paused())
        .map(|a| a.agent_type.clone())
        .collect();
    let previous_snapshots = load_dispatch_snapshots(&project_root);
    let mut current_snapshots = DispatchSnapshotLedger::new();
    let rows: Vec<DispatchReportRow> = agents
        .iter()
        .filter(|a| a.current_spec.is_some())
        .map(|agent| {
            let worktree = &agent.worktree_path;
            let agent_name = agent_identity(agent);
            let branch = current_branch_at(worktree);
            let dirty_text = dispatch_worktree_dirty_fingerprint(worktree);
            let dirty = !dirty_text.is_empty() && dirty_text != "unknown";
            let current_snapshot = dispatch_liveness_snapshot(
                agent.status,
                git_capture(worktree, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into()),
                dirty_text.clone(),
            );
            let liveness_stalled = previous_snapshots
                .get(&agent_name)
                .map(|previous| dispatch_liveness_stalled(previous, &current_snapshot));
            current_snapshots.insert(agent_name.clone(), current_snapshot);
            let (ahead_main, _behind_main) = branch
                .as_deref()
                .and_then(|b| {
                    ahead_behind_vs_ref(worktree, b, "origin/main")
                        .or_else(|| ahead_behind_vs_ref(worktree, b, "main"))
                })
                .map(|(a, b)| (Some(a), Some(b)))
                .unwrap_or((None, None));
            let upstream = upstream_ref_for(worktree, branch.as_deref().unwrap_or(""));
            let (ahead_upstream, has_upstream) = match (branch.as_deref(), upstream) {
                (Some(b), Some(u)) => (ahead_behind_vs_ref(worktree, b, &u).map(|(a, _)| a), true),
                _ => (None, false),
            };
            let input = DispatchReportInput {
                agent: agent_name,
                agent_type: agent.agent_type.clone(),
                spec: agent.current_spec.clone(),
                status: agent.status,
                liveness_stalled,
                paused: agent.availability.is_paused(),
                worktree: worktree.clone(),
                branch,
                dirty,
                ahead_main,
                ahead_upstream,
                has_upstream,
                pending_briefs: pending.get(&agent.agent_type).copied().unwrap_or(0),
            };
            dispatch_report_row(&input, &paused_vendor_types, force_fallback)
        })
        .collect();
    persist_dispatch_snapshots(&project_root, &current_snapshots);

    if agent_output_mode() {
        println!("{}", render_dispatch_toon(&rows));
        return Ok(());
    }

    println!("{}", "Dispatch health".bold());
    if rows.is_empty() {
        println!("  (no active agent/spec rows)");
        return Ok(());
    }
    for row in rows {
        println!(
            "  {} [{}] {} via {} on {}",
            row.spec.bold(),
            row.state.as_str(),
            row.agent.cyan(),
            row.agent_type,
            row.branch
        );
        println!(
            "      worktree: {}{}",
            row.worktree.dimmed(),
            if row.dirty {
                " (dirty)".yellow().to_string()
            } else {
                String::new()
            }
        );
        if let Some(ahead) = row.ahead_main {
            println!(
                "      branch: {} ahead of main{}",
                ahead,
                if row.pushed { " (pushed)" } else { "" }
            );
        }
        if row.pending_briefs > 0 {
            println!("      pending briefs: {}", row.pending_briefs);
        }
        println!("      guidance: {}", row.guidance);
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/dispatch_health_tests.rs"]
mod dispatch_health_tests;

/// TASK-837: the canonical list of agent types `aida agent new` can launch, in
/// menu order. Derived from the `AgentNewCommand` variants — keep in sync if a
/// new launcher variant is added. Each entry is `(label, type-token)`: the
/// label is what the picker shows, the token is the `agent new <token>` lane.
// trace:TASK-837 | ai:claude
pub(crate) fn agent_type_picker_choices() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Claude", "claude"),
        ("Codex", "codex"),
        ("Antigravity", "antigravity"),
    ]
}

pub(crate) fn agent_selection_allows_agent_type(
    selection: init_cmd::AgentSelection,
    agent_type: &str,
) -> bool {
    match agent_type {
        "claude" => selection.claude,
        "codex" => selection.codex,
        "antigravity" => selection.antigravity,
        _ => true,
    }
}

// trace:BUG-1020 | ai:codex
pub(crate) fn enabled_agent_type_picker_choices_for_project(
    project_root: &std::path::Path,
) -> Vec<(&'static str, &'static str)> {
    let choices = agent_type_picker_choices();
    let Some(selection) = init_cmd::read_enabled_agent_selection(project_root) else {
        return choices;
    };
    choices
        .into_iter()
        .filter(|(_, token)| agent_selection_allows_agent_type(selection, token))
        .collect()
}

pub(crate) fn disabled_agent_type_hint(agent_type: &str, project_root: &std::path::Path) -> String {
    let source_path = init_cmd::read_enabled_agent_selection_with_source(project_root)
        .map(|s| s.path)
        .unwrap_or_else(|| project_root.join(".aida/config.toml"));
    let enabled = enabled_agent_type_picker_choices_for_project(project_root)
        .into_iter()
        .map(|(_, token)| token)
        .collect::<Vec<_>>()
        .join(", ");
    let enabled_hint = if enabled.is_empty() {
        "none".to_string()
    } else {
        enabled
    };
    format!(
        "`{agent_type}` is disabled by `[agents] enabled` in {} (enabled: {enabled_hint}). \
         Enable it there, or launch one of the enabled agent types.",
        source_path.display()
    )
}

pub(crate) fn enforce_agent_type_enabled(
    project_root: &std::path::Path,
    agent_type: &str,
) -> Result<()> {
    let Some(selection) = init_cmd::read_enabled_agent_selection(project_root) else {
        return Ok(());
    };
    if agent_selection_allows_agent_type(selection, agent_type) {
        return Ok(());
    }
    anyhow::bail!("{}", disabled_agent_type_hint(agent_type, project_root))
}

/// TASK-837: map a picked agent-type token to its default-options
/// `AgentNewCommand` (the same value clap would build for a bare
/// `aida agent new <type>` with no extra flags).
// trace:TASK-837 | ai:claude
pub(crate) fn agent_new_command_for_type(token: &str) -> Option<AgentNewCommand> {
    match token {
        "claude" => Some(AgentNewCommand::Claude {
            role: None,
            spec: None,
            force: false,
            cwd: None,
            permission_mode: None,
            model: None,
            sandbox: false,
            no_context: false,
            no_title: false,
            show_context: false,
            noexec: false,
            show_prompt: false,
            verbose: false,
            prompt: None,
            prompt_file: None,
            no_prompt: false,
            no_resume: false,
            resume: None,
            allow_duplicate: false,
            no_duplicate_check: false,
            no_default_flags: false,
            extra_flags: Vec::new(),
            name: None,
            description: None,
            bg: false,
        }),
        "codex" => Some(AgentNewCommand::Codex {
            role: None,
            spec: None,
            force: false,
            cwd: None,
            bypass_sandbox: false,
            model: None,
            no_context: false,
            no_title: false,
            show_context: false,
            noexec: false,
            show_prompt: false,
            verbose: false,
            prompt: None,
            prompt_file: None,
            no_prompt: false,
            no_resume: false,
            resume: None,
            allow_duplicate: false,
            no_duplicate_check: false,
            no_default_flags: false,
            extra_flags: Vec::new(),
            name: None,
            description: None,
        }),
        "antigravity" => Some(AgentNewCommand::Antigravity {
            role: None,
            spec: None,
            force: false,
            cwd: None,
            bypass_sandbox: false,
            no_context: false,
            no_title: false,
            show_context: false,
            noexec: false,
            show_prompt: false,
            verbose: false,
            prompt: None,
            prompt_file: None,
            no_prompt: false,
            no_resume: false,
            resume: None,
            allow_duplicate: false,
            no_duplicate_check: false,
            no_default_flags: false,
            extra_flags: Vec::new(),
            name: None,
            description: None,
        }),
        _ => None,
    }
}

/// TASK-837: arrow-key agent-type picker for a bare `aida agent new`. Mirrors
/// the role picker (`pick_role_with_header`): `inquire::Select` writes its
/// prompt to stderr (so eval-captured stdout stays clean), up/down move, Enter
/// selects, Esc/Ctrl-C cancels. Returns `Ok(Some(cmd))` on selection,
/// `Ok(None)` on cancel. Non-interactive stdin → `Ok(None)` so scripts get the
/// usual clap help/usage instead of a hung prompt.
// trace:TASK-837 | ai:claude
pub(crate) fn pick_agent_type_interactively() -> Result<Option<AgentNewCommand>> {
    use clap::CommandFactory;
    use std::io::IsTerminal;
    // Non-TTY (piped/headless): don't block on a picker — fall through to the
    // historical clap-help behavior by re-emitting the help for `agent new`.
    if !std::io::stdin().is_terminal() {
        let mut cmd = Cli::command();
        if let Some(agent) = cmd.find_subcommand_mut("agent") {
            if let Some(new) = agent.find_subcommand_mut("new") {
                let _ = new.clone().print_help();
                println!();
            }
        }
        return Ok(None);
    }

    let project_root =
        main_worktree_root_from(&find_aida_project_root_from(&std::env::current_dir()?)?);
    let choices = enabled_agent_type_picker_choices_for_project(&project_root);
    if choices.is_empty() {
        let source_path = init_cmd::read_enabled_agent_selection_with_source(&project_root)
            .map(|s| s.path)
            .unwrap_or_else(|| project_root.join(".aida/config.toml"));
        anyhow::bail!(
            "no agent launch profiles are enabled by `[agents] enabled` in {}",
            source_path.display()
        );
    }
    if choices.len() == 1 {
        return Ok(agent_new_command_for_type(choices[0].1));
    }
    let labels: Vec<&str> = choices.iter().map(|(label, _)| *label).collect();
    let select = inquire::Select::new("Select an agent type to launch:", labels).with_help_message(
        "Use arrow keys to move, type to filter, Enter to select, Esc to cancel",
    );

    match select.raw_prompt() {
        Ok(choice) => {
            let token = choices[choice.index].1;
            Ok(agent_new_command_for_type(token))
        }
        Err(inquire::InquireError::OperationCanceled)
        | Err(inquire::InquireError::OperationInterrupted) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("agent-type picker failed: {e}")),
    }
}

/// TASK-837: dispatch a resolved `AgentNewCommand` to its launcher. Factored
/// out of `handle_agent_command` so both the explicit `aida agent new <type>`
/// path and the interactive picker share one code path.
// trace:TASK-837 | ai:claude
pub(crate) fn dispatch_agent_new(cmd: &AgentNewCommand) -> Result<()> {
    match cmd {
        AgentNewCommand::Claude {
            role,
            spec,
            force,
            cwd,
            permission_mode,
            model,
            sandbox,
            no_context,
            no_title,
            show_context,
            noexec,
            show_prompt,
            verbose,
            prompt,
            prompt_file,
            no_prompt,
            no_resume,
            resume,
            allow_duplicate,
            no_duplicate_check,
            no_default_flags,
            extra_flags,
            name,
            bg,
            description,
        } => agent_new_claude(
            role.clone(),
            spec.clone(),
            *force,
            cwd.as_deref(),
            permission_mode.as_deref(),
            *sandbox,
            AgentContextOptions::new(!*no_context, *show_context),
            !*no_title,
            *noexec,
            *show_prompt,
            *verbose,
            AgentPromptOptions::new(
                resolve_launch_prompt(prompt.clone(), prompt_file.as_deref(), *no_prompt)?,
                *no_prompt,
            ),
            AgentResumeOptions::new(
                !*no_resume,
                resume.clone(),
                *allow_duplicate,
                !*no_duplicate_check,
            ),
            AgentDefaultFlagOptions::new(!*no_default_flags, extra_flags.clone(), model.clone()),
            name.clone(),
            description.clone(),
            *bg,
        ),
        AgentNewCommand::Codex {
            role,
            spec,
            force,
            cwd,
            bypass_sandbox,
            model,
            no_context,
            no_title,
            show_context,
            noexec,
            show_prompt,
            verbose,
            prompt,
            prompt_file,
            no_prompt,
            no_resume,
            resume,
            allow_duplicate,
            no_duplicate_check,
            no_default_flags,
            extra_flags,
            name,
            description,
        } => agent_new_codex(
            role.clone(),
            spec.clone(),
            *force,
            cwd.as_deref(),
            *bypass_sandbox,
            AgentContextOptions::new(!*no_context, *show_context),
            !*no_title,
            *noexec,
            *show_prompt,
            *verbose,
            AgentPromptOptions::new(
                resolve_launch_prompt(prompt.clone(), prompt_file.as_deref(), *no_prompt)?,
                *no_prompt,
            ),
            AgentResumeOptions::new(
                !*no_resume,
                resume.clone(),
                *allow_duplicate,
                !*no_duplicate_check,
            ),
            AgentDefaultFlagOptions::new(!*no_default_flags, extra_flags.clone(), model.clone()),
            name.clone(),
            description.clone(),
        ),
        AgentNewCommand::Antigravity {
            role,
            spec,
            force,
            cwd,
            bypass_sandbox,
            no_context,
            no_title,
            show_context,
            noexec,
            show_prompt,
            verbose,
            prompt,
            prompt_file,
            no_prompt,
            no_resume,
            resume,
            allow_duplicate,
            no_duplicate_check,
            no_default_flags,
            extra_flags,
            name,
            description,
        } => agent_new_antigravity(
            role.clone(),
            spec.clone(),
            *force,
            cwd.as_deref(),
            *bypass_sandbox,
            AgentContextOptions::new(!*no_context, *show_context),
            !*no_title,
            *noexec,
            *show_prompt,
            *verbose,
            AgentPromptOptions::new(
                resolve_launch_prompt(prompt.clone(), prompt_file.as_deref(), *no_prompt)?,
                *no_prompt,
            ),
            AgentResumeOptions::new(
                !*no_resume,
                resume.clone(),
                *allow_duplicate,
                !*no_duplicate_check,
            ),
            AgentDefaultFlagOptions::new(!*no_default_flags, extra_flags.clone(), None),
            name.clone(),
            description.clone(),
        ),
    }
}

// trace:STORY-528 | ai:claude
pub(crate) fn agent_pause(agent: &str, reason: &str, resets: Option<&str>) -> Result<()> {
    let project_root =
        main_worktree_root_from(&find_aida_project_root_from(&std::env::current_dir()?)?);
    let reason = agent_registry::PauseReason::parse(reason).ok_or_else(|| {
        anyhow::anyhow!(
            "invalid pause reason '{}' (expected: budget, rate-limit, manual, unknown)",
            reason
        )
    })?;
    let expected_back = match resets {
        Some(when) => Some(parse_resets_when(when)?),
        None => None,
    };
    let entry = agent_registry::pause_agent(&project_root, agent, reason, expected_back)?;
    let identity = entry
        .name
        .clone()
        .unwrap_or_else(|| format!("{}#{}", entry.agent_type, entry.pid));
    let detail = agent_registry::pause_detail(reason, entry.expected_back);
    println!("{} {} {}", "⏸".yellow(), identity.cyan(), detail);
    Ok(())
}

// trace:STORY-528 | ai:claude
pub(crate) fn agent_resume(agent: &str) -> Result<()> {
    let project_root =
        main_worktree_root_from(&find_aida_project_root_from(&std::env::current_dir()?)?);
    match agent_registry::resume_agent(&project_root, agent) {
        Ok(entry) => {
            let identity = entry
                .name
                .clone()
                .unwrap_or_else(|| format!("{}#{}", entry.agent_type, entry.pid));
            println!(
                "{} {} resumed (available)",
                crate::glyph(crate::glyphs::Glyph::FlowActive).green(),
                identity.cyan()
            );
            Ok(())
        }
        Err(live_err) => {
            match agent_registry::resolve_resumable_ended_agent(&project_root, agent) {
                Ok(entry) => agent_resume_ended(&project_root, entry),
                Err(ended_err) => anyhow::bail!("{live_err}; {ended_err}"),
            }
        }
    }
}

// trace:STORY-790 | ai:codex
pub(crate) fn agent_resume_ended(
    project_root: &std::path::Path,
    entry: agent_registry::AgentRegistryEntry,
) -> Result<()> {
    if !entry.worktree_path.exists() {
        anyhow::bail!(
            "cannot resume {}: recorded worktree is gone ({})",
            entry.name.as_deref().unwrap_or(entry.id.as_str()),
            entry.worktree_path.display()
        );
    }
    let native_session_id = entry.native_session_id.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "cannot resume {}: registry entry has no native session id",
            entry.name.as_deref().unwrap_or(entry.id.as_str())
        )
    })?;
    verify_agent_native_session_available(
        &entry.agent_type,
        &entry.worktree_path,
        native_session_id,
    )?;
    let binary_name = match entry.agent_type.as_str() {
        "claude" => "claude",
        "codex" => "codex",
        "antigravity" => "agy",
        other => anyhow::bail!("agent type `{other}` does not have a supported resume adapter"),
    };
    let binary = find_executable_on_path(binary_name).ok_or_else(|| {
        anyhow::anyhow!(
            "`{}` is not on PATH — install {} or activate the environment that provides it",
            binary_name,
            entry.agent_type
        )
    })?;
    let resume_prompt =
        "Read the fresh AIDA resume context first: cat \"$AIDA_AGENT_CONTEXT_FILE\"";
    let config = AgentLaunchConfig {
        agent_type: match entry.agent_type.as_str() {
            "claude" => "claude",
            "codex" => "codex",
            "antigravity" => "antigravity",
            _ => unreachable!(),
        },
        binary: binary_name,
        default_args: agent_resume_args(&entry.agent_type, native_session_id, resume_prompt)?,
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    let plan = AgentLaunchPlan {
        project_root: project_root.to_path_buf(),
        launch_cwd: entry.worktree_path.clone(),
        role: entry.role.clone(),
        role_instance: RoleInstanceKind::Driver,
        current_spec: entry.current_spec.clone(),
        name: entry
            .name
            .clone()
            .unwrap_or_else(|| format!("{}-resumed", entry.id)),
        lease_id: None,
        native_session_id: Some(native_session_id.to_string()),
        resumed_from: Some(entry.id.clone()),
    };
    let launch_context = prepare_agent_resume_context(&config, &plan, &entry)?;
    eprintln!(
        "{} resuming {} (stable name: {}) in {}",
        crate::glyph(crate::glyphs::Glyph::FlowActive)
            .green()
            .bold(),
        config.agent_type,
        plan.name.cyan(),
        plan.launch_cwd.display().to_string().cyan()
    );
    eprintln!(
        "  {}: {}",
        "native session".bold(),
        native_session_id.cyan()
    );
    eprintln!(
        "  {}: {}",
        "context".bold(),
        launch_context.path.display().to_string().cyan()
    );
    run_tracked_agent(
        &binary,
        &config,
        &plan,
        Some(&launch_context),
        &[],
        entry.description.clone(),
        true,
        false,
    )
}

// trace:STORY-790 | ai:codex
pub(crate) fn verify_agent_native_session_available(
    agent_type: &str,
    worktree: &std::path::Path,
    native_session_id: &str,
) -> Result<()> {
    match agent_type {
        "claude" => {
            let transcript =
                session::claude_project_dir(worktree)?.join(format!("{native_session_id}.jsonl"));
            if !transcript.exists() {
                anyhow::bail!(
                    "cannot resume claude session {native_session_id}: transcript not found at {}",
                    transcript.display()
                );
            }
        }
        "antigravity" => {
            let Some(home) = crate::home_dir() else {
                anyhow::bail!(
                    "cannot resume antigravity session {native_session_id}: no home directory"
                );
            };
            let brain = home
                .join(".gemini")
                .join("antigravity-cli")
                .join("brain")
                .join(native_session_id);
            if !brain.exists() {
                anyhow::bail!(
                    "cannot resume antigravity session {native_session_id}: brain directory not found at {}",
                    brain.display()
                );
            }
        }
        _ => {}
    }
    Ok(())
}

/// Parse a `--resets` value: an RFC3339 timestamp, or a relative duration
/// (`2h` / `90m` / `45s`) added to now. trace:STORY-528 | ai:claude
pub(crate) fn parse_resets_when(raw: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    let raw = raw.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Ok(dt.with_timezone(&chrono::Utc));
    }
    let (num, unit) = raw.split_at(raw.find(|c: char| !c.is_ascii_digit()).unwrap_or(raw.len()));
    let n: i64 = num.parse().map_err(|_| {
        anyhow::anyhow!(
            "invalid --resets value '{}' (expected RFC3339 timestamp or relative duration like 2h/90m/45s)",
            raw
        )
    })?;
    let delta = match unit.trim() {
        "h" => chrono::Duration::hours(n),
        "m" => chrono::Duration::minutes(n),
        "s" => chrono::Duration::seconds(n),
        other => anyhow::bail!("invalid --resets unit '{}' (expected h, m, or s)", other),
    };
    Ok(chrono::Utc::now() + delta)
}

// trace:BUG-2124 | ai:antigravity
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentLaunchConfig {
    pub(crate) agent_type: &'static str,
    pub(crate) binary: &'static str,
    pub(crate) default_args: Vec<String>,
    pub(crate) prompt_style: AgentPromptStyle,
    pub(crate) contained: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentPromptStyle {
    Positional,
    Flag(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentLaunchPlan {
    pub(crate) project_root: std::path::PathBuf,
    pub(crate) launch_cwd: std::path::PathBuf,
    pub(crate) role: Option<String>,
    pub(crate) role_instance: RoleInstanceKind,
    pub(crate) current_spec: Option<String>,
    pub(crate) name: String,
    /// SPIKE-34: id of the lease created by `prepare_agent_launch` when
    /// `--spec` was supplied. None otherwise. Lets the `--bg` dispatch
    /// path attach Claude Code's sessionId back to the AIDA manifest so
    /// `aida status` cross-references the right pair. trace:SPIKE-34
    pub(crate) lease_id: Option<String>,
    // trace:STORY-790 | ai:codex
    /// Native vendor conversation id when AIDA can seed it before launch.
    /// `agent resume` uses this rather than an AIDA lease id.
    pub(crate) native_session_id: Option<String>,
    // trace:STORY-790 | ai:codex
    /// Registry id of the ended launcher session this launch resumes.
    pub(crate) resumed_from: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentContextOptions {
    pub(crate) enabled: bool,
    pub(crate) show: bool,
}

impl AgentContextOptions {
    pub(crate) fn new(enabled: bool, show: bool) -> Self {
        Self { enabled, show }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentLaunchContext {
    pub(crate) path: std::path::PathBuf,
    pub(crate) token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentPromptOptions {
    pub(crate) explicit_prompt: Option<String>,
    pub(crate) auto_prompt: bool,
}

/// BUG-1696: resolve a launch prompt from `--prompt` or `--prompt-file`.
///
/// Orchestrator seats build dispatches as `--prompt "$(cat brief.txt)"` inside a nested
/// `bash -lc`. When that quoting collapses the launcher receives an empty string, spawns an
/// interactive agent that sits at an idle prompt, and blocks the caller on it — once for
/// 3h04m, reporting exit 0. `--prompt-file` takes the shell out of the path, and the
/// emptiness check makes the remaining failure loud instead of silent.
// trace:BUG-1696 | ai:claude
pub(crate) fn resolve_launch_prompt(
    prompt: Option<String>,
    prompt_file: Option<&std::path::Path>,
    no_prompt: bool,
) -> Result<Option<String>> {
    if no_prompt {
        return Ok(None);
    }
    let resolved = match prompt_file {
        Some(path) => Some(
            std::fs::read_to_string(path)
                .with_context(|| format!("failed to read --prompt-file `{}`", path.display()))?,
        ),
        None => prompt,
    };
    let Some(text) = resolved else {
        return Ok(None);
    };
    if text.trim().is_empty() {
        let (source, fix) = match prompt_file {
            Some(path) => (
                format!("--prompt-file `{}` is empty", path.display()),
                "Write the brief into that file",
            ),
            None => (
                "--prompt resolved to an empty string (nested shell quoting does this)".to_string(),
                "Pass the brief with `--prompt-file <PATH>` instead of interpolating it into the \
                 command line",
            ),
        };
        anyhow::bail!(
            "{source}; refusing to launch an agent with no initial message — it would sit idle \
             at a prompt while the caller blocks on it. {fix}, or use `--no-prompt` for a \
             deliberately unprompted seat."
        );
    }
    Ok(Some(text))
}

impl AgentPromptOptions {
    pub(crate) fn new(explicit_prompt: Option<String>, no_prompt: bool) -> Self {
        Self {
            explicit_prompt: if no_prompt { None } else { explicit_prompt },
            auto_prompt: !no_prompt,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentDefaultFlagOptions {
    pub(crate) use_config_defaults: bool,
    pub(crate) extra_flags: Vec<String>,
    pub(crate) model_override: Option<String>,
}

impl AgentDefaultFlagOptions {
    pub(crate) fn new(
        use_config_defaults: bool,
        extra_flags: Vec<String>,
        model_override: Option<String>,
    ) -> Self {
        Self {
            use_config_defaults,
            extra_flags,
            model_override,
        }
    }
}

/// STORY-495: each agent's bypass flag(s), injected when the uniform
/// `[agents] bypass = true` knob is on and the launch has no explicit posture.
/// Mirrors the per-tool opt-in flags the launchers already inject for
/// `--bypass-sandbox` / `--permission-mode`. trace:STORY-495 | ai:claude
pub(crate) fn tool_bypass_flags(agent_type: &str) -> Vec<String> {
    match agent_type {
        "claude" => vec![
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
        ],
        "codex" => vec!["--dangerously-bypass-approvals-and-sandbox".to_string()],
        "antigravity" => vec!["--dangerously-skip-permissions".to_string()],
        _ => Vec::new(),
    }
}

/// BUG-1699: split per-tool `default_flags` into (kept, dropped-as-conflicting) for a launch
/// whose posture the launcher already set explicitly. A posture flag and its value are dropped
/// together, in both the `--flag value` and `--flag=value` spellings, so the emitted argv
/// carries exactly one posture rather than two that the vendor CLI silently arbitrates.
/// Pure and total, so the partition is unit-tested without spawning.
// trace:BUG-1699 | ai:claude
pub(crate) fn split_posture_flags(
    agent_type: &str,
    flags: Vec<String>,
) -> (Vec<String>, Vec<String>) {
    // Flags that select an approval/sandbox posture. `true` = consumes a following value.
    let posture: &[(&str, bool)] = match agent_type {
        "claude" => &[
            ("--permission-mode", true),
            ("--settings", true),
            ("--setting-sources", true),
            ("--dangerously-skip-permissions", false),
        ],
        "codex" => &[
            ("--sandbox", true),
            ("--ask-for-approval", true),
            ("--dangerously-bypass-approvals-and-sandbox", false),
            ("--full-auto", false),
        ],
        "antigravity" => &[("--dangerously-skip-permissions", false)],
        _ => &[],
    };
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    let mut idx = 0;
    while idx < flags.len() {
        let arg = &flags[idx];
        let matched = posture.iter().find(|(name, takes_value)| {
            arg == name || (*takes_value && arg.starts_with(&format!("{name}=")))
        });
        match matched {
            Some((name, takes_value)) => {
                dropped.push(arg.clone());
                // `--flag value` spends the next token too; `--flag=value` does not.
                if *takes_value && arg == name {
                    if let Some(value) = flags.get(idx + 1) {
                        dropped.push(value.clone());
                        idx += 1;
                    }
                }
                idx += 1;
            }
            None => {
                kept.push(arg.clone());
                idx += 1;
            }
        }
    }
    (kept, dropped)
}

// trace:BUG-1178 | ai:codex
pub(crate) fn tool_contained_flags(agent_type: &str) -> Vec<String> {
    match agent_type {
        "claude" => {
            let mut flags = Vec::new();
            flags.extend(session::claude_contained_flags());
            flags
        }
        // trace:STORY-1124 | ai:codex
        "codex" => vec![
            "--sandbox".to_string(),
            "workspace-write".to_string(),
            "--ask-for-approval".to_string(),
            "never".to_string(),
        ],
        _ => Vec::new(),
    }
}

// trace:BUG-1178 | ai:codex
pub(crate) fn claude_args_request_dontask(args: &[String]) -> bool {
    args.windows(2)
        .any(|w| w[0] == "--permission-mode" && w[1] == "dontAsk")
}

// trace:BUG-1178 | ai:codex
pub(crate) fn guard_interactive_claude_dontask(agent_type: &str, args: &[String]) -> Result<()> {
    if agent_type == "claude" && claude_args_request_dontask(args) {
        anyhow::bail!(
            "refusing to launch interactive Claude with --permission-mode dontAsk; \
             use contained mode without dontAsk for a promptable TTY, or run a headless drain \
             when no human can answer permission prompts"
        );
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentResumeOptions {
    pub(crate) prompt_enabled: bool,
    pub(crate) direct_target: Option<String>,
    pub(crate) allow_duplicate: bool,
    pub(crate) duplicate_check: bool,
}

impl AgentResumeOptions {
    pub(crate) fn new(
        prompt_enabled: bool,
        direct_target: Option<String>,
        allow_duplicate: bool,
        duplicate_check: bool,
    ) -> Self {
        Self {
            prompt_enabled,
            direct_target,
            allow_duplicate,
            duplicate_check,
        }
    }
}

// trace:STORY-991 | ai:codex
pub(crate) fn agent_resume_prompt_enabled(project_root: &std::path::Path) -> bool {
    let cfg = read_project_config_value(project_root);
    config_lookup(cfg.as_ref(), "agent", "resume_prompt")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

pub(crate) fn spec_rank(agent: &agent_registry::AgentRegistryView, spec: Option<&str>) -> u8 {
    match spec {
        Some(spec)
            if agent
                .current_spec
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(spec)) =>
        {
            0
        }
        _ => 1,
    }
}

pub(crate) fn agent_launch_views(
    project_root: &std::path::Path,
) -> Vec<agent_registry::AgentRegistryView> {
    let leases = list_leases(project_root);
    let ctx = build_agent_classify_context(project_root, &leases);
    let registry_agents = agent_registry::list_agent_views(project_root, &ctx);
    merge_agent_views_with_lease_fallback(project_root, &leases, registry_agents, &ctx)
}

pub(crate) fn matching_live_duplicate(
    agents: &[agent_registry::AgentRegistryView],
    agent_type: &str,
    role: Option<&str>,
    spec: Option<&str>,
) -> Option<agent_registry::AgentRegistryView> {
    let mut matches: Vec<_> = agents
        .iter()
        .filter(|agent| {
            same_agent_role_view(agent, agent_type, role)
                && agent.ended_at.is_none()
                && agent.status != agent_registry::AgentStatus::Stale
        })
        .cloned()
        .collect();
    matches.sort_by(|a, b| {
        spec_rank(a, spec)
            .cmp(&spec_rank(b, spec))
            .then_with(|| b.started_at.cmp(&a.started_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    matches.into_iter().next()
}

pub(crate) fn matching_ended_resume_candidates(
    project_root: &std::path::Path,
    agent_type: &str,
    role: Option<&str>,
    spec: Option<&str>,
    limit: usize,
) -> Vec<agent_registry::AgentRegistryView> {
    let leases = list_leases(project_root);
    let ctx = build_agent_classify_context(project_root, &leases);
    matching_ended_resume_views_from(
        agent_registry::ended_resumable_agent_views(project_root, &ctx),
        agent_type,
        role,
        spec,
        limit,
    )
}

pub(crate) fn matching_ended_resume_views_from(
    agents: Vec<agent_registry::AgentRegistryView>,
    agent_type: &str,
    role: Option<&str>,
    spec: Option<&str>,
    limit: usize,
) -> Vec<agent_registry::AgentRegistryView> {
    let mut matches: Vec<_> = agents
        .into_iter()
        .filter(|agent| {
            same_agent_role_view(agent, agent_type, role)
                && agent.ended_at.is_some()
                && agent.native_session_id.is_some()
        })
        .collect();
    matches.sort_by(|a, b| {
        spec_rank(a, spec)
            .cmp(&spec_rank(b, spec))
            .then_with(|| b.ended_at.cmp(&a.ended_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    matches.truncate(limit);
    matches
}

pub(crate) fn agent_launch_should_prompt(
    headless: bool,
    stdin_tty: bool,
    stderr_tty: bool,
) -> bool {
    stdin_tty && stderr_tty && !headless
}

// trace:BUG-1019 | ai:codex
pub(crate) fn live_duplicate_allows_anyway_prompt(
    project_root: &std::path::Path,
    role: Option<&str>,
    current_spec: Option<&str>,
    worktree_path: &std::path::Path,
) -> bool {
    let cfg = agent_registry::Config::load(project_root);
    agent_registry::same_scope_conflict(project_root, &cfg, role, current_spec, worktree_path)
        .is_none()
}

pub(crate) fn agent_identity_for_view(agent: &agent_registry::AgentRegistryView) -> String {
    agent
        .name
        .clone()
        .unwrap_or_else(|| format!("{}#{}", agent.agent_type, agent.pid))
}

pub(crate) fn local_time_label(at: chrono::DateTime<chrono::Utc>) -> String {
    at.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M:%S %Z")
        .to_string()
}

pub(crate) fn render_live_duplicate_block(agent: &agent_registry::AgentRegistryView) -> String {
    let now = chrono::Utc::now();
    let elapsed = agent_registry::humanize_elapsed(agent_registry::elapsed_secs_clamped(
        now,
        agent.started_at,
    ));
    let identity = agent_identity_for_view(agent);
    format!(
        "A live {vendor}/{role} agent already exists:\n  name: {name}\n  spec: {spec}\n  pid: {pid}\n  tty: {tty}\n  started: {started}\n  elapsed: {elapsed}\n  resume: aida agent resume {name}",
        vendor = agent.agent_type,
        role = agent.role.as_deref().unwrap_or("(none)"),
        name = identity,
        spec = agent.current_spec.as_deref().unwrap_or("(none)"),
        pid = if agent.source == "lease" {
            "-".to_string()
        } else {
            agent.pid.to_string()
        },
        tty = agent.tty.as_deref().unwrap_or("(unknown)"),
        started = local_time_label(agent.started_at),
        elapsed = elapsed,
    )
}

pub(crate) fn resume_unavailable_reason(
    agent: &agent_registry::AgentRegistryView,
) -> Option<String> {
    if !agent.worktree_path.exists() {
        return Some(format!(
            "worktree missing: {}",
            agent.worktree_path.display()
        ));
    }
    let native = agent.native_session_id.as_deref()?;
    verify_agent_native_session_available(&agent.agent_type, &agent.worktree_path, native)
        .err()
        .map(|err| err.to_string())
}

pub(crate) fn format_agent_resume_candidate(agent: &agent_registry::AgentRegistryView) -> String {
    let now = chrono::Utc::now();
    let age = agent
        .ended_at
        .map(|at| agent_registry::humanize_elapsed(agent_registry::elapsed_secs_clamped(now, at)))
        .unwrap_or_else(|| "?".to_string());
    let id = agent_identity_for_view(agent);
    let title = agent.description.as_deref().unwrap_or("(none)");
    let base = format!(
        "{id}  age:{age}  role:{role}  spec:{spec}  worktree:{worktree}  title:{title}",
        role = agent.role.as_deref().unwrap_or("(none)"),
        spec = agent.current_spec.as_deref().unwrap_or("(none)"),
        worktree = agent.worktree_path.display(),
    );
    match resume_unavailable_reason(agent) {
        Some(reason) => format!("{}  unavailable: {}", base.dimmed(), reason.dimmed()),
        None => base,
    }
}

pub(crate) fn choose_latest_resume_candidate(
    project_root: &std::path::Path,
    agent_type: &str,
    role: Option<&str>,
    spec: Option<&str>,
) -> Result<Option<agent_registry::AgentRegistryEntry>> {
    let Some(candidate) = matching_ended_resume_candidates(project_root, agent_type, role, spec, 1)
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    agent_registry::resolve_resumable_ended_agent(project_root, &candidate.id).map(Some)
}

pub(crate) fn maybe_handle_agent_resume_or_duplicate(
    project_root: &std::path::Path,
    agent_type: &str,
    role: Option<&str>,
    spec: Option<&str>,
    resume: &AgentResumeOptions,
) -> Result<bool> {
    if let Some(target) = resume.direct_target.as_deref() {
        let entry = if target.eq_ignore_ascii_case("latest") {
            choose_latest_resume_candidate(project_root, agent_type, role, spec)?.ok_or_else(
                || anyhow::anyhow!("no ended resumable {agent_type} agent found for this role"),
            )?
        } else {
            agent_registry::resolve_resumable_ended_agent(project_root, target)?
        };
        agent_resume_ended(project_root, entry)?;
        return Ok(true);
    }

    let headless = std::env::var("AIDA_HEADLESS").as_deref() == Ok("1");
    let interactive = agent_launch_should_prompt(
        headless,
        std::io::stdin().is_terminal(),
        std::io::stderr().is_terminal(),
    );

    if resume.duplicate_check {
        if let Some(live) =
            matching_live_duplicate(&agent_launch_views(project_root), agent_type, role, spec)
        {
            let block = render_live_duplicate_block(&live);
            if !live_duplicate_allows_anyway_prompt(project_root, role, spec, project_root) {
                eprintln!("{block}");
                enforce_agent_singleton_preflight(project_root, role, spec, project_root)?;
                return Ok(true);
            }
            if interactive {
                eprintln!("{block}");
                if !resume.allow_duplicate {
                    // ?-exempt: the duplicate block above is the local context card.
                    let launch = inquire::Confirm::new("Launch another anyway?")
                        .with_default(false)
                        .prompt()
                        .unwrap_or(false);
                    if !launch {
                        eprintln!("Launch cancelled.");
                        return Ok(true);
                    }
                }
            } else {
                eprintln!("warning: {block}");
            }
        }
    }

    if !(resume.prompt_enabled && agent_resume_prompt_enabled(project_root) && interactive) {
        return Ok(false);
    }

    let candidates = matching_ended_resume_candidates(project_root, agent_type, role, spec, 5);
    if candidates.is_empty() {
        return Ok(false);
    }
    let fresh_label = "Start a new session".to_string();
    let labels: Vec<String> = std::iter::once(fresh_label.clone())
        .chain(candidates.iter().map(format_agent_resume_candidate))
        .collect();
    let prompt = format!(
        "Resume a recent {agent_type}/{role} session?",
        role = role.unwrap_or("implementer")
    );
    let pick = match inquire::Select::new(&prompt, labels.clone())
        .with_help_message("arrows to move, type to filter, Enter to choose, Esc to start new")
        .prompt()
    {
        Ok(pick) => pick,
        Err(inquire::InquireError::OperationCanceled)
        | Err(inquire::InquireError::OperationInterrupted) => return Ok(false),
        Err(e) => return Err(anyhow::anyhow!("agent resume picker failed: {e}")),
    };
    if pick == fresh_label {
        return Ok(false);
    }
    let idx = labels
        .iter()
        .position(|l| l == &pick)
        .map(|p| p.saturating_sub(1))
        .unwrap_or(0);
    if let Some(reason) = resume_unavailable_reason(&candidates[idx]) {
        eprintln!(
            "cannot resume {}: {reason}",
            agent_identity_for_view(&candidates[idx])
        );
        return Ok(true);
    }
    let entry = agent_registry::resolve_resumable_ended_agent(project_root, &candidates[idx].id)?;
    agent_resume_ended(project_root, entry)?;
    Ok(true)
}

/// BUG-2151: the Claude `agent new` launch posture.
// trace:BUG-2151 | ai:claude
pub(crate) struct ClaudeNewPosture {
    pub(crate) default_args: Vec<String>,
    pub(crate) contained: bool,
    /// The launcher set the posture itself, so the `[agents]` knobs and
    /// per-tool posture flags must not add another.
    pub(crate) explicit: bool,
}

/// BUG-2151 (ruling on BUG-2133: containment is monotonic). Resolve the
/// posture for `aida agent new claude`:
///
/// - A contained PARENT (`AIDA_CONTAINED`, parsed strictly by the caller)
///   always wins: the child is contained, and `--permission-mode` (the only
///   claude posture flag) is refused rather than honoured.
/// - `--permission-mode` / `--sandbox` are operator posture flags and may
///   override the `[agents] contained` CONFIG knob, never the env.
/// - `--bg` is NOT a posture flag. Under the contained knob a detached launch
///   stays contained; only an uncontained `--bg` launch is forced to
///   `bypassPermissions` (no answerable TTY).
// trace:BUG-2151 | ai:claude
pub(crate) fn resolve_claude_new_posture(
    permission_mode: Option<&str>,
    sandbox: bool,
    bg: bool,
    parent_contained: bool,
    knob_contained: impl FnOnce() -> Result<bool>,
) -> Result<ClaudeNewPosture> {
    // STORY-495: faithful default — inject `--permission-mode` only when the
    // operator passed it explicitly. Otherwise leave Claude on its native
    // posture and let the uniform `[agents] bypass` knob (applied in
    // `apply_agent_default_flags`) decide whether to flip the whole fleet.
    if sandbox && permission_mode.is_some() {
        anyhow::bail!(
            "--sandbox and --permission-mode are mutually exclusive Claude launch postures"
        );
    }
    if parent_contained {
        if let Some(m) = permission_mode {
            anyhow::bail!(
                "[contained] this session is contained (AIDA_CONTAINED), so every agent it \
                 launches is contained too — refusing `--permission-mode {m}`, which would run \
                 the child with a different posture. Drop --permission-mode, or launch from an \
                 uncontained shell."
            );
        }
    }
    let contained_posture = || -> Result<ClaudeNewPosture> {
        // trace:BUG-2131 | ai:claude
        aida_core::git_ops::ensure_git_supports_contained()?;
        Ok(ClaudeNewPosture {
            default_args: tool_contained_flags("claude"),
            contained: true,
            explicit: true,
        })
    };
    if let Some(m) = permission_mode {
        return Ok(ClaudeNewPosture {
            default_args: vec!["--permission-mode".to_string(), m.to_string()],
            contained: false,
            explicit: true,
        });
    }
    if sandbox || parent_contained {
        return contained_posture();
    }
    if bg {
        if knob_contained()? {
            eprintln!(
                "{} --bg under [agents] contained: the detached session stays contained.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            );
            return contained_posture();
        }
        // STORY-495: a detached `--bg` launch has no answerable TTY, so a
        // native (prompting) child would hang forever on the first
        // permission prompt. Force bypass and say so. An explicit
        // `--permission-mode` still wins (handled above).
        eprintln!(
            "{} --bg detaches from this terminal (no answerable prompt); forcing \
             --permission-mode bypassPermissions. Pass --permission-mode <mode> to override.",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
        );
        return Ok(ClaudeNewPosture {
            default_args: vec![
                "--permission-mode".to_string(),
                "bypassPermissions".to_string(),
            ],
            contained: false,
            explicit: true,
        });
    }
    Ok(ClaudeNewPosture {
        default_args: Vec::new(),
        contained: false,
        explicit: false,
    })
}

/// BUG-2151: fail closed on the final `agent new` argv. When this process is
/// contained (`AIDA_CONTAINED`, parsed strictly — a typo errors), the child
/// must be a containable tool, must be marked contained (so the child env
/// keeps `AIDA_CONTAINED=1`), and must carry no posture flag beyond the
/// containment flags themselves (no `--permission-mode`, no
/// `--dangerously-*`, no second `--settings`) — whatever the launcher flags,
/// the agents.toml defaults, or a `--` passthrough tried to add.
// trace:BUG-2151 | ai:claude
pub(crate) fn refuse_uncontained_child_of_contained_parent(
    config: &AgentLaunchConfig,
) -> Result<()> {
    if !session::aida_contained_env()? {
        return Ok(());
    }
    let containable = session::HeadlessVendor::parse(config.agent_type)
        .is_some_and(|v| session::containment_applied(v, true));
    if !containable {
        anyhow::bail!(
            "[contained] this session is contained (AIDA_CONTAINED), but `{}` cannot run an \
             AIDA-contained session — refusing to launch it uncontained. Launch \
             `aida agent new claude`, or launch from an uncontained shell.",
            config.agent_type
        );
    }
    let (_, posture) = split_posture_flags(config.agent_type, config.default_args.clone());
    let mut posture_names: Vec<&str> = posture
        .iter()
        .filter(|a| a.starts_with("--"))
        .map(|a| a.split('=').next().unwrap_or(a))
        .collect();
    posture_names.sort_unstable();
    let only_containment = posture_names == ["--setting-sources", "--settings"];
    if !is_contained_launch(config) || !only_containment {
        anyhow::bail!(
            "[contained] this session is contained (AIDA_CONTAINED), so every agent it \
             launches must be contained too — refusing to launch `{}` with posture flags \
             [{}]. Drop the posture override, or launch from an uncontained shell.",
            config.agent_type,
            posture.join(" ")
        );
    }
    Ok(())
}

// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_new_claude(
    role: Option<String>,
    spec: Option<String>,
    force: bool,
    cwd: Option<&std::path::Path>,
    permission_mode: Option<&str>,
    sandbox: bool,
    context: AgentContextOptions,
    title: bool,
    noexec: bool,
    show_prompt: bool,
    verbose: bool,
    prompt: AgentPromptOptions,
    resume: AgentResumeOptions,
    flag_options: AgentDefaultFlagOptions,
    name: Option<String>,
    description: Option<String>,
    bg: bool,
) -> Result<()> {
    // BUG-2151: resolve the launch posture BEFORE the role picker, so a
    // contained parent (or a malformed AIDA_CONTAINED) refuses before any
    // interaction. trace:BUG-2151 | ai:claude
    let parent_contained = session::aida_contained_env()?;
    let knob_contained = || -> Result<bool> {
        if !flag_options.use_config_defaults {
            return Ok(false);
        }
        let base = cwd
            .map(std::path::PathBuf::from)
            .unwrap_or(std::env::current_dir()?);
        let root = agent_launch_project_root_from(&find_aida_project_root_from(&base)?);
        crate::agent_config::load_agents_contained(&root)
    };
    let ClaudeNewPosture {
        mut default_args,
        contained,
        explicit,
    } = resolve_claude_new_posture(
        permission_mode,
        sandbox,
        bg,
        parent_contained,
        knob_contained,
    )?;
    // TASK-646: resolve the child role (flag → picker → implementer default)
    // before any launch work. A cancelled picker aborts cleanly.
    let role = match resolve_child_role(&child_role_project_root(cwd)?, role, "claude")? {
        Some(r) => Some(r),
        None => {
            eprintln!(
                "{} launch cancelled — no role selected.",
                crate::glyph(crate::glyphs::Glyph::Cross).yellow()
            );
            return Ok(());
        }
    };
    // SPIKE-34: when `--bg` is set, append it to the argv so `claude --bg`
    // dispatches the session to the background supervisor instead of
    // running foreground in this terminal. Forces the bg-aware dispatch
    // path below. trace:SPIKE-34 | ai:claude
    if bg {
        default_args.push("--bg".to_string());
    }
    let config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args,
        prompt_style: AgentPromptStyle::Positional,
        contained,
    };
    if bg {
        agent_new_bg_dispatch(
            config,
            role,
            spec,
            force,
            cwd,
            context,
            title,
            noexec,
            show_prompt,
            verbose,
            prompt,
            resume,
            flag_options,
            name,
            description,
            explicit,
        )
    } else {
        agent_new_with_config(
            config,
            role,
            spec,
            force,
            cwd,
            context,
            title,
            noexec,
            show_prompt,
            verbose,
            prompt,
            resume,
            flag_options,
            name,
            description,
            explicit,
        )
    }
}

// trace:STORY-433 | ai:codex
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_new_codex(
    role: Option<String>,
    spec: Option<String>,
    force: bool,
    cwd: Option<&std::path::Path>,
    bypass_sandbox: bool,
    context: AgentContextOptions,
    title: bool,
    noexec: bool,
    show_prompt: bool,
    verbose: bool,
    prompt: AgentPromptOptions,
    resume: AgentResumeOptions,
    flag_options: AgentDefaultFlagOptions,
    name: Option<String>,
    description: Option<String>,
) -> Result<()> {
    // TASK-646: resolve the child role before launch (flag → picker → default).
    let role = match resolve_child_role(&child_role_project_root(cwd)?, role, "codex")? {
        Some(r) => Some(r),
        None => {
            eprintln!(
                "{} launch cancelled — no role selected.",
                crate::glyph(crate::glyphs::Glyph::Cross).yellow()
            );
            return Ok(());
        }
    };
    // BUG-2007: a codex TUI attached to the shared app-server daemon runs its
    // hooks (and tools) with the daemon's environment, i.e. whichever shell
    // first started the daemon, so the child got another session's role and
    // grant. Run it standalone so its own AIDA_SESSION_ROLE/grant apply.
    // trace:BUG-2007 | ai:claude
    let mut default_args = vec![CODEX_NO_DAEMON_FLAG.to_string()];
    if bypass_sandbox {
        default_args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
    }
    let config = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args,
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    // STORY-495: `--bypass-sandbox` is an explicit posture; without it the
    // uniform knob is free to inject the bypass flag.
    agent_new_with_config(
        config,
        role,
        spec,
        force,
        cwd,
        context,
        title,
        noexec,
        show_prompt,
        verbose,
        prompt,
        resume,
        flag_options,
        name,
        description,
        bypass_sandbox,
    )
}

// trace:STORY-434 | ai:codex
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_new_antigravity(
    role: Option<String>,
    spec: Option<String>,
    force: bool,
    cwd: Option<&std::path::Path>,
    bypass_sandbox: bool,
    context: AgentContextOptions,
    title: bool,
    noexec: bool,
    show_prompt: bool,
    verbose: bool,
    prompt: AgentPromptOptions,
    resume: AgentResumeOptions,
    flag_options: AgentDefaultFlagOptions,
    name: Option<String>,
    description: Option<String>,
) -> Result<()> {
    // TASK-646: resolve the child role before launch (flag → picker → default).
    let role = match resolve_child_role(&child_role_project_root(cwd)?, role, "antigravity")? {
        Some(r) => Some(r),
        None => {
            eprintln!(
                "{} launch cancelled — no role selected.",
                crate::glyph(crate::glyphs::Glyph::Cross).yellow()
            );
            return Ok(());
        }
    };
    let mut default_args = Vec::new();
    if bypass_sandbox {
        default_args.push("--dangerously-skip-permissions".to_string());
    }
    let config = AgentLaunchConfig {
        agent_type: "antigravity",
        binary: "agy",
        default_args,
        prompt_style: AgentPromptStyle::Flag("--prompt-interactive"),
        contained: false,
    };
    // STORY-495: `--bypass-sandbox` is an explicit posture; without it the
    // uniform knob is free to inject the bypass flag.
    agent_new_with_config(
        config,
        role,
        spec,
        force,
        cwd,
        context,
        title,
        noexec,
        show_prompt,
        verbose,
        prompt,
        resume,
        flag_options,
        name,
        description,
        bypass_sandbox,
    )
}

// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_new_with_config(
    mut config: AgentLaunchConfig,
    role: Option<String>,
    spec: Option<String>,
    force: bool,
    cwd: Option<&std::path::Path>,
    context: AgentContextOptions,
    title: bool,
    noexec: bool,
    show_prompt: bool,
    verbose: bool,
    prompt: AgentPromptOptions,
    resume: AgentResumeOptions,
    flag_options: AgentDefaultFlagOptions,
    name: Option<String>,
    description: Option<String>,
    explicit_permission: bool,
) -> Result<()> {
    let base = cwd
        .map(std::path::PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let discovered_root = find_aida_project_root_from(&base)?;
    let project_root = agent_launch_project_root_from(&discovered_root);
    enforce_agent_type_enabled(&project_root, config.agent_type)?;
    let binary = find_executable_on_path(config.binary).ok_or_else(|| {
        anyhow::anyhow!(
            "`{}` is not on PATH — install {} or activate the environment \
             that provides it, then retry `aida agent new {}`",
            config.binary,
            config.agent_type,
            config.agent_type
        )
    })?;
    apply_agent_default_flags(
        &mut config,
        &project_root,
        flag_options,
        explicit_permission,
    )?;
    // BUG-2151: a contained parent contains every child, whatever the flags.
    // Checked before any preview or side effect. trace:BUG-2151 | ai:claude
    refuse_uncontained_child_of_contained_parent(&config)?;
    guard_interactive_claude_dontask(config.agent_type, &config.default_args)?;

    // STORY-495: surface the one-time faithful-launcher pointer when a Claude
    // launch lands on the native posture (no `--permission-mode` in the argv).
    // A forced/explicit bypass (incl. `--bg`) injects the flag, so this no-ops
    // for those paths.
    if config.agent_type == "claude"
        && !config.default_args.iter().any(|a| a == "--permission-mode")
    {
        maybe_show_faithful_launcher_notice();
    }

    // BUG-408: `--show-context` is a PURE PREVIEW. Render the launch-context
    // snapshot from a dry plan and return without launching — no `session_start`,
    // so no worktree, no lease, no spec status flip. Previously this fell through
    // to the full launch path, creating the worktree + lease + flipping the spec
    // to InProgress before printing — the opposite of a dry preview.
    // trace:BUG-408 | ai:claude
    if context.show {
        return print_dry_launch_context(&project_root, role, spec, &config, name);
    }
    // TASK-1467: `--show-prompt` is a narrower dry preview than `--no-exec` —
    // just the initial prompt that would be sent, explicit or generated.
    // Same dry plan, same "no side effects" contract, checked before
    // `--no-exec` so `--show-prompt --no-exec` degrades to the narrower ask.
    if show_prompt {
        let plan = prepare_agent_launch_dry(&project_root, role, spec, config.agent_type, name)?;
        config.default_args.extend(agent_seed_session_args(
            config.agent_type,
            plan.native_session_id.as_deref(),
        ));
        let prompt_args = agent_initial_prompt_args(&config, &plan, &prompt);
        return print_agent_show_prompt(&prompt, &prompt_args);
    }
    if noexec {
        let plan = prepare_agent_launch_dry(&project_root, role, spec, config.agent_type, name)?;
        config.default_args.extend(agent_seed_session_args(
            config.agent_type,
            plan.native_session_id.as_deref(),
        ));
        let prompt_args = agent_initial_prompt_args(&config, &plan, &prompt);
        return print_agent_launch_noexec(
            &binary,
            &config,
            &plan,
            &prompt,
            &prompt_args,
            true,
            context.enabled,
        );
    }

    // Check ownership before any bypass/resume prompt; previews above remain pure.
    // trace:BUG-2002 | ai:codex
    preflight_agent_launch_lease(&project_root, role.as_deref(), spec.as_deref(), cwd)?;

    // Validate before any real agent launch even when --verbose is absent;
    // preview mode validates in its renderer above. Resolve the binary BEFORE
    // the consent gate so an unresolvable build fails fast rather than after a
    // human has already answered the bypass prompt. trace:TASK-1499 | ai:codex
    let _resolved_aida = aida_bin::process()?;

    gate_agent_bypass(
        &project_root,
        config.agent_type,
        &mut config.default_args,
        false,
        explicit_permission,
    )?;

    // STORY-717: focus-scope drift guard at the agent-launch work-start moment.
    // Spawning an agent on `--spec` outside the active focus subtree applies the
    // [focus] out_of_scope policy (warn nudges + proceeds, block refuses without
    // --force, off silent). Runs AFTER the `--show-context` preview return (a
    // dry preview is never blocked) and before the worktree/lease/status-flip
    // side effects of `prepare_agent_launch`. trace:STORY-717 | ai:claude
    if let Some(spec_id) = spec.as_deref() {
        // BUG-653: an epic isn't directly implementable — give the
        // epic-appropriate message before the readiness gate dead-ends.
        epic_agent_new_guard(&project_root, spec_id)?;
        // BUG-1701: a drain-groomed spec belongs in the one-shot lane, not here.
        // trace:BUG-1701 | ai:claude
        drain_mode_agent_new_guard(&project_root, spec_id, force, true)?;
        focus_scope_guard_for_spec(&project_root, spec_id, force)?;
    }

    if maybe_handle_agent_resume_or_duplicate(
        &project_root,
        config.agent_type,
        role.as_deref(),
        spec.as_deref(),
        &resume,
    )? {
        return Ok(());
    }

    let early_role_instance = resolve_role_instance_for_launch(
        &project_root,
        role.as_deref(),
        spec.as_deref(),
        &project_root,
    );
    // trace:BUG-1697 | ai:claude
    if resume.duplicate_check && early_role_instance == RoleInstanceKind::Driver {
        enforce_agent_singleton_preflight(
            &project_root,
            role.as_deref(),
            spec.as_deref(),
            &project_root,
        )?;
    }
    let plan =
        prepare_agent_launch_with_cwd(&project_root, role, spec, config.agent_type, name, cwd)?;
    config.default_args.extend(agent_seed_session_args(
        config.agent_type,
        plan.native_session_id.as_deref(),
    ));
    enforce_agent_singleton(&project_root, &plan, resume.duplicate_check)?;
    let description = resolve_agent_description(plan.role.as_deref(), description)?;
    // TASK-965: worktree-tangle spawn gate — refuse a fan-out into the primary
    // checkout. trace:TASK-965 | ai:claude
    assert_no_worktree_tangle(&plan, &project_root)?;
    let prompt_args = agent_initial_prompt_args(&config, &plan, &prompt);

    eprintln!(
        "{} launching {} (stable name: {}) in {}",
        crate::glyph(crate::glyphs::Glyph::FlowActive)
            .green()
            .bold(),
        config.agent_type,
        plan.name.cyan(),
        plan.launch_cwd.display().to_string().cyan()
    );
    if let Some(spec) = &plan.current_spec {
        eprintln!("  {}: {}", "spec".bold(), spec.cyan());
    }
    if let Some(role) = &plan.role {
        eprintln!("  {}: {}", "role".bold(), role.cyan());
    }
    if plan.role_instance == RoleInstanceKind::Companion {
        eprintln!(
            "  {}: {} (driver seat already live; read/converse/draft only)",
            "instance".bold(),
            plan.role_instance.as_str().cyan()
        );
    }
    // BUG-558: surface the agent's addressable identity — the stable name is now
    // its mailbox/queue handle (AIDA_USER), so a coordinator reaches it with
    // `aida mailbox send --to <name>` / `aida queue add --for <name>`.
    eprintln!(
        "  {}: {} (e.g. `aida mailbox send --to {}`)",
        "address".bold(),
        plan.name.cyan(),
        plan.name
    );
    let launch_context = prepare_agent_launch_context(&config, &plan, context)?;
    if let Some(ctx) = &launch_context {
        eprintln!(
            "  {}: {}",
            "context".bold(),
            ctx.path.display().to_string().cyan()
        );
    }
    eprintln!(
        "  {} multiple concurrent {} sessions are supported; registry entries are PID-keyed.",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
        config.agent_type
    );

    // TASK-1498: `--verbose` launch-time diagnostics — a REAL launch (unlike
    // `--no-exec`, which previews and exits). Printed to stderr, same as the
    // banner lines above, so stdout stays exactly what it is today when
    // `--verbose` is not passed. trace:TASK-1498 | ai:claude
    if verbose {
        eprint!(
            "{}",
            render_agent_launch_diagnostics(
                &binary,
                &config,
                &plan,
                &prompt,
                &prompt_args,
                true,
                context.enabled,
            )?
        );
    }

    run_tracked_agent(
        &binary,
        &config,
        &plan,
        launch_context.as_ref(),
        &prompt_args,
        description,
        title,
        verbose,
    )
}

/// SPIKE-34: shape-mirror of `agent_new_with_config` for the `claude --bg`
/// dispatch path. The semantic difference is fundamental:
///   - Foreground: AIDA spawns claude, waits for exit, owns the PID.
///   - Background: `claude --bg` returns ~immediately after handing the
///     session to Claude Code's per-user supervisor; the PID AIDA would
///     see is the launcher, not the session.
///
/// So we don't call `run_tracked_agent` here — we shell out, capture
/// stdout, parse the `backgrounded · <short-id>` line, write the
/// captured sessionId onto the lease's manifest (when one exists), and
/// return success. After that, `aida status`'s SPIKE-30 cross-substrate
/// section + `claude agents` both see the session.
/// trace:SPIKE-34 | ai:claude
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_new_bg_dispatch(
    mut config: AgentLaunchConfig,
    role: Option<String>,
    spec: Option<String>,
    force: bool,
    cwd: Option<&std::path::Path>,
    context: AgentContextOptions,
    _title: bool,
    noexec: bool,
    show_prompt: bool,
    verbose: bool,
    prompt: AgentPromptOptions,
    resume: AgentResumeOptions,
    flag_options: AgentDefaultFlagOptions,
    name: Option<String>,
    description: Option<String>,
    explicit_permission: bool,
) -> Result<()> {
    let base = cwd
        .map(std::path::PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let discovered_root = find_aida_project_root_from(&base)?;
    let project_root = agent_launch_project_root_from(&discovered_root);
    enforce_agent_type_enabled(&project_root, config.agent_type)?;
    let binary = find_executable_on_path(config.binary).ok_or_else(|| {
        anyhow::anyhow!(
            "`claude` is not on PATH — install Claude Code (or activate the \
             environment that provides it), then retry `aida agent new claude --bg`"
        )
    })?;
    apply_agent_default_flags(
        &mut config,
        &project_root,
        flag_options,
        explicit_permission,
    )?;
    // BUG-2151: a contained parent contains every child, whatever the flags.
    // Checked before any preview or side effect. trace:BUG-2151 | ai:claude
    refuse_uncontained_child_of_contained_parent(&config)?;
    guard_interactive_claude_dontask(config.agent_type, &config.default_args)?;

    // STORY-495: surface the one-time faithful-launcher pointer when a Claude
    // launch lands on the native posture (no `--permission-mode` in the argv).
    // A forced/explicit bypass (incl. `--bg`) injects the flag, so this no-ops
    // for those paths.
    if config.agent_type == "claude"
        && !config.default_args.iter().any(|a| a == "--permission-mode")
    {
        maybe_show_faithful_launcher_notice();
    }

    // BUG-408: `--show-context` is a PURE PREVIEW. Render the launch-context
    // snapshot from a dry plan and return without launching — no `session_start`,
    // so no worktree, no lease, no spec status flip. Previously this fell through
    // to the full launch path, creating the worktree + lease + flipping the spec
    // to InProgress before printing — the opposite of a dry preview.
    // trace:BUG-408 | ai:claude
    if context.show {
        return print_dry_launch_context(&project_root, role, spec, &config, name);
    }
    // TASK-1467: see the foreground path's comment — same narrower-preview-first
    // ordering and no-side-effects contract for the `--bg` dispatch.
    if show_prompt {
        let plan = prepare_agent_launch_dry(&project_root, role, spec, config.agent_type, name)?;
        config.default_args.extend(agent_seed_session_args(
            config.agent_type,
            plan.native_session_id.as_deref(),
        ));
        let prompt_args = agent_initial_prompt_args(&config, &plan, &prompt);
        return print_agent_show_prompt(&prompt, &prompt_args);
    }
    if noexec {
        let plan = prepare_agent_launch_dry(&project_root, role, spec, config.agent_type, name)?;
        config.default_args.extend(agent_seed_session_args(
            config.agent_type,
            plan.native_session_id.as_deref(),
        ));
        let prompt_args = agent_initial_prompt_args(&config, &plan, &prompt);
        return print_agent_launch_noexec(
            &binary,
            &config,
            &plan,
            &prompt,
            &prompt_args,
            false,
            context.enabled,
        );
    }

    // Check ownership before any bypass/resume prompt; previews above remain pure.
    // trace:BUG-2002 | ai:codex
    preflight_agent_launch_lease(&project_root, role.as_deref(), spec.as_deref(), cwd)?;

    // Validate before the detached/background launch too, and before the
    // consent gate for the same fail-fast reason. trace:TASK-1499 | ai:codex
    let _resolved_aida = aida_bin::process()?;

    gate_agent_bypass(
        &project_root,
        config.agent_type,
        &mut config.default_args,
        true,
        false,
    )?;

    // STORY-717: focus-scope drift guard (same as the foreground path) for the
    // `--bg` dispatch. trace:STORY-717 | ai:claude
    if let Some(spec_id) = spec.as_deref() {
        // BUG-653: epic-aware dead-end guard, same as the foreground path.
        epic_agent_new_guard(&project_root, spec_id)?;
        // BUG-1701: same routing guard; `--bg` detaches, so the message drops the
        // "holds your caller" clause. trace:BUG-1701 | ai:claude
        drain_mode_agent_new_guard(&project_root, spec_id, force, false)?;
        focus_scope_guard_for_spec(&project_root, spec_id, force)?;
    }

    if maybe_handle_agent_resume_or_duplicate(
        &project_root,
        config.agent_type,
        role.as_deref(),
        spec.as_deref(),
        &resume,
    )? {
        return Ok(());
    }

    // trace:BUG-1697 | ai:claude
    if resume.duplicate_check {
        enforce_agent_singleton_preflight(
            &project_root,
            role.as_deref(),
            spec.as_deref(),
            &project_root,
        )?;
    }
    let plan =
        prepare_agent_launch_with_cwd(&project_root, role, spec, config.agent_type, name, cwd)?;
    config.default_args.extend(agent_seed_session_args(
        config.agent_type,
        plan.native_session_id.as_deref(),
    ));
    enforce_agent_singleton(&project_root, &plan, resume.duplicate_check)?;
    let _description = resolve_agent_description(plan.role.as_deref(), description)?;
    // TASK-965: worktree-tangle spawn gate — refuse a fan-out into the primary
    // checkout. trace:TASK-965 | ai:claude
    assert_no_worktree_tangle(&plan, &project_root)?;
    let prompt_args = agent_initial_prompt_args(&config, &plan, &prompt);

    eprintln!(
        "{} dispatching {} (stable name: {}) to Claude Code's background supervisor in {}",
        crate::glyph(crate::glyphs::Glyph::FlowActive)
            .green()
            .bold(),
        config.agent_type,
        plan.name.cyan(),
        plan.launch_cwd.display().to_string().cyan()
    );
    if let Some(spec) = &plan.current_spec {
        eprintln!("  {}: {}", "spec".bold(), spec.cyan());
    }
    if let Some(role) = &plan.role {
        eprintln!("  {}: {}", "role".bold(), role.cyan());
    }

    let launch_context = prepare_agent_launch_context(&config, &plan, context)?;
    if let Some(ctx) = &launch_context {
        eprintln!(
            "  {}: {}",
            "context".bold(),
            ctx.path.display().to_string().cyan()
        );
    }

    // TASK-1498: `--verbose` pre-spawn diagnostics, same contract as the
    // foreground path — printed to stderr before the child is spawned.
    // trace:TASK-1498 | ai:claude
    if verbose {
        eprint!(
            "{}",
            render_agent_launch_diagnostics(
                &binary,
                &config,
                &plan,
                &prompt,
                &prompt_args,
                false,
                context.enabled,
            )?
        );
    }

    let mut command = build_agent_new_bg_command(
        &binary,
        &config,
        &plan,
        &prompt_args,
        launch_context.as_ref(),
    )?;

    let output = command
        .output_retrying_etxtbsy()
        .with_context(|| format!("failed to spawn {}", binary.display()))?;
    // TASK-1498: `--verbose` child-result diagnostics for the `--bg` shape —
    // `claude --bg` returns immediately after handing off, so "exit" here is
    // the dispatcher's own exit, not the backgrounded session's.
    // trace:TASK-1498 | ai:claude
    if verbose {
        eprintln!(
            "  {}: dispatcher exited: success={} code={}",
            "diagnostics".bold(),
            output.status.success(),
            output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string())
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        eprintln!("{}", stderr.trim_end());
        anyhow::bail!(
            "`claude --bg` exited with status {}",
            output.status.code().unwrap_or(-1)
        );
    }
    // Mirror claude's own output so the operator sees the helper
    // commands (`claude attach <id>`, `claude logs <id>`, etc.) without
    // an extra step. trace:SPIKE-34 | ai:claude
    print!("{stdout}");
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }

    let session_id = parse_bg_session_id(&stdout);
    match (&session_id, &plan.lease_id) {
        (Some(sid), Some(lease_id)) => {
            // Update or create the manifest. trace:SPIKE-34 | ai:claude
            let manifest_path = session_manifest::manifest_path(&plan.project_root, lease_id);
            let mut manifest = session_manifest::load(&manifest_path).unwrap_or_else(|_| {
                session_manifest::SessionManifest {
                    session_id: lease_id.clone(),
                    planned_at: chrono::Utc::now(),
                    plan_source: "agent new --bg".to_string(),
                    claude_session_id: None,
                    batch_name: None,
                    plan: None,
                    items: Vec::new(),
                }
            });
            manifest.claude_session_id = Some(sid.clone());
            if let Err(err) = session_manifest::save(&manifest_path, &manifest) {
                eprintln!(
                    "  {} couldn't record claude sessionId on lease manifest: {err}",
                    "warning:".yellow().bold()
                );
            } else {
                eprintln!(
                    "  {}: lease {} ↔ claude {}",
                    "linked".green().bold(),
                    &lease_id[..lease_id.len().min(8)].yellow(),
                    sid.chars().take(8).collect::<String>().yellow()
                );
            }
        }
        (Some(_), None) => {} // no lease to link
        (None, _) => {
            eprintln!(
                "  {} couldn't parse `backgrounded · <id>` line from claude output \
                 — lease will not be linked to the Claude session",
                "warning:".yellow().bold()
            );
        }
    }

    // The session is now owned by Claude's daemon. AIDA doesn't track
    // its PID here — `aida status` (SPIKE-30) reads `claude agents
    // --json` for the truth and joins via the manifest's
    // claude_session_id we just wrote.
    Ok(())
}

/// SPIKE-34: pull the short sessionId from `claude --bg`'s stdout. The
/// observed format is one `backgrounded · <8-hex>` line among the
/// helper-command rows. trace:SPIKE-34 | ai:claude
pub(crate) fn parse_bg_session_id(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("backgrounded") else {
            continue;
        };
        let id = rest.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
        if !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Some(id.to_string());
        }
    }
    None
}

// trace:STORY-790 | ai:codex
pub(crate) fn agent_native_session_id_for_new(agent_type: &str) -> Option<String> {
    match agent_type {
        "claude" => Some(Uuid::now_v7().to_string()),
        _ => None,
    }
}

// trace:STORY-790 | ai:codex
pub(crate) fn agent_seed_session_args(
    agent_type: &str,
    native_session_id: Option<&str>,
) -> Vec<String> {
    match (agent_type, native_session_id) {
        ("claude", Some(id)) => vec!["--session-id".to_string(), id.to_string()],
        _ => Vec::new(),
    }
}

/// Keeps an interactive codex off the shared app-server daemon (BUG-2007).
pub(crate) const CODEX_NO_DAEMON_FLAG: &str = "--no-daemon";

/// codex rejects a repeated `--no-daemon` ("cannot be used multiple times"), so
/// when the operator's `--extra-flag` or agents.toml `default_flags` already
/// carry it, the injected copy must not double it. Keeps the first occurrence.
// trace:BUG-2007 | ai:claude
pub(crate) fn dedupe_codex_no_daemon(agent_type: &str, args: &mut Vec<String>) {
    if agent_type != "codex" {
        return;
    }
    let mut seen = false;
    args.retain(|a| {
        if a != CODEX_NO_DAEMON_FLAG {
            return true;
        }
        !std::mem::replace(&mut seen, true)
    });
}

// trace:STORY-790 | ai:codex
pub(crate) fn agent_resume_args(
    agent_type: &str,
    native_session_id: &str,
    prompt: &str,
) -> Result<Vec<String>> {
    let mut args = match agent_type {
        "claude" => vec!["--resume".to_string(), native_session_id.to_string()],
        // trace:BUG-2007 | ai:claude
        "codex" => vec![
            "resume".to_string(),
            native_session_id.to_string(),
            CODEX_NO_DAEMON_FLAG.to_string(),
        ],
        "antigravity" => vec![
            "--conversation".to_string(),
            native_session_id.to_string(),
            "--prompt-interactive".to_string(),
        ],
        other => anyhow::bail!("agent type `{other}` does not have a supported resume adapter"),
    };
    // trace:BUG-2007 | ai:claude
    dedupe_codex_no_daemon(agent_type, &mut args);
    if !prompt.trim().is_empty() {
        args.push(prompt.to_string());
    }
    Ok(args)
}

#[cfg(test)]
#[path = "tests/spike_34_bg_dispatch_tests.rs"]
mod spike_34_bg_dispatch_tests;

pub(crate) fn resolve_interactive_launch_mode(
    flag: Option<&str>,
    sandbox: bool,
) -> Result<ResolvedClaudeLaunch> {
    let root = find_main_worktree_root().ok();
    resolve_interactive_launch_mode_for_root(flag, sandbox, root.as_deref())
}

// trace:BUG-1178 | ai:codex
pub(crate) fn resolve_interactive_launch_mode_for_root(
    flag: Option<&str>,
    sandbox: bool,
    root: Option<&std::path::Path>,
) -> Result<ResolvedClaudeLaunch> {
    if sandbox && flag.filter(|s| !s.is_empty()).is_some() {
        anyhow::bail!(
            "--sandbox and --permission-mode are mutually exclusive Claude launch postures"
        );
    }
    if let Some(m) = flag.filter(|s| !s.is_empty()) {
        return Ok(ResolvedClaudeLaunch {
            mode: Some(m.to_string()),
            contained: false,
        });
    }
    if let Some(root) = root {
        let bypass = load_agents_bypass(root).unwrap_or(false);
        // BUG-2146: a malformed agents.toml refuses the launch. trace:BUG-2146 | ai:claude
        let contained = sandbox || load_agents_contained(root)?;
        if bypass && contained {
            anyhow::bail!(
                "[agents] bypass and [agents] contained are mutually exclusive launch postures"
            );
        }
        if contained {
            // trace:BUG-2131 | ai:claude
            aida_core::git_ops::ensure_git_supports_contained()?;
            return Ok(ResolvedClaudeLaunch {
                mode: None,
                contained: true,
            });
        }
        if bypass {
            return Ok(ResolvedClaudeLaunch {
                mode: Some("bypassPermissions".to_string()),
                contained: false,
            });
        }
    }
    Ok(ResolvedClaudeLaunch {
        mode: None,
        contained: sandbox,
    })
}

/// STORY-495: one-time, discoverable migration pointer printed the first time
/// an interactive Claude launch lands on the faithful native default (no
/// `--permission-mode` injected). A marker under `~/.aida/` makes it fire at
/// most once per machine. Best-effort: any IO error silently no-ops so the
/// notice never blocks a launch.
pub(crate) fn maybe_show_faithful_launcher_notice() {
    let Some(home) = aida_home_dir() else {
        return;
    };
    let marker = home.join(".faithful-launcher-notice");
    if marker.exists() {
        return;
    }
    eprintln!(
        "{} {}",
        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
        "AIDA launchers now honor Claude's native permission posture (it will \
         prompt). To restore bypass-by-default for every agent, set `[agents] \
         bypass = true` in ~/.aida/agents.toml (or per-project .aida/agents.toml), \
         or pass --permission-mode bypassPermissions per launch."
            .dimmed()
    );
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&marker, "shown\n");
}

pub(crate) fn agent_initial_prompt_args(
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    options: &AgentPromptOptions,
) -> Vec<String> {
    let prompt = if let Some(prompt) = options
        .explicit_prompt
        .as_deref()
        .filter(|prompt| !prompt.trim().is_empty())
    {
        Some(prompt.to_string())
    } else if options.auto_prompt {
        // STORY-1471: every launch gets a role-aware first message — the
        // role's contract for `--spec`, or a short orientation without one.
        // trace:STORY-1471 | ai:claude
        Some(agent_launch_prompt::role_launch_prompt(
            &plan.project_root,
            config.agent_type,
            plan.role.as_deref(),
            plan.current_spec.as_deref(),
            orchestrated_implementer_ship_instruction(
                std::env::var(orchestrator::VARIANT_ENV).ok().as_deref(),
            ),
        ))
    } else {
        None
    };
    let Some(prompt) = prompt else {
        return Vec::new();
    };
    match config.prompt_style {
        AgentPromptStyle::Positional => vec![prompt],
        AgentPromptStyle::Flag(flag) => vec![flag.to_string(), prompt],
    }
}

pub(crate) fn orchestrated_implementer_ship_instruction(variant: Option<&str>) -> &'static str {
    match variant.map(str::trim).filter(|v| !v.is_empty()) {
        Some("full" | "through-ci" | "through-merge" | "skip-build") => {
            "then use `aida pr ship --no-merge` (or plain `aida pr ship`, which will refuse the merge leg inside the drive) and exit after the PR-open checkpoint."
        }
        _ => "then use `aida pr ship` and exit after the IMPLEMENTER COMPLETE banner.",
    }
}

#[cfg(test)]
pub(crate) fn render_agent_initial_prompt_with_ship_instruction(
    agent_type: &str,
    spec: &str,
    ship_instruction: &str,
) -> String {
    agent_launch_prompt::render_role_launch_prompt(
        agent_type,
        Some("implementer"),
        Some(spec),
        None,
        ship_instruction,
    )
}

#[cfg(test)]
mod bug742_pickup_contract_tests {
    use super::{
        orchestrated_implementer_ship_instruction,
        render_agent_initial_prompt_with_ship_instruction,
    };

    #[test]
    fn through_ci_pickup_guidance_opens_pr_without_merge() {
        let instruction = orchestrated_implementer_ship_instruction(Some("through-ci"));
        assert!(instruction.contains("aida pr ship --no-merge"));
        assert!(instruction.contains("PR-open checkpoint"));

        let prompt =
            render_agent_initial_prompt_with_ship_instruction("codex", "BUG-742", instruction);
        assert!(prompt.contains("aida pr ship --no-merge"));
        assert!(!prompt.contains("IMPLEMENTER COMPLETE banner"));
    }

    #[test]
    fn plain_pickup_guidance_keeps_direct_ship_cadence() {
        let instruction = orchestrated_implementer_ship_instruction(None);
        assert!(instruction.contains("aida pr ship"));
        assert!(instruction.contains("IMPLEMENTER COMPLETE banner"));
        assert!(!instruction.contains("--no-merge"));
    }
}

// BUG-1510: the dispatch-time integrity check. Only fires when exactly one
// pending (unacked) brief exists for the agent — with zero pending briefs
// there is nothing to compare against, and with more than one there is no
// single "the brief" driving this dispatch (an agent's ordinary backlog of
// future work is not a mismatch). This mirrors the one incident this spec
// was filed from: one live brief, one live lease, disagreeing spec ids.
// trace:BUG-1510 | ai:claude
pub(crate) fn dispatch_lease_brief_conflict<'a>(
    briefs: &'a [BriefListEntry],
    dispatch_spec: &str,
) -> Option<&'a BriefListEntry> {
    let mut pending = briefs.iter().filter(|b| !b.acked);
    let only = pending.next()?;
    if pending.next().is_some() {
        return None;
    }
    if only.spec_id.eq_ignore_ascii_case(dispatch_spec) {
        None
    } else {
        Some(only)
    }
}

/// An explicit owning cwd is read-only re-entry, never a claim or forced takeover.
/// Both launch paths call this before prompts, and plan creation checks it again.
// trace:BUG-2002 | ai:codex
fn preflight_agent_launch_lease(
    project_root: &std::path::Path,
    role: Option<&str>,
    spec: Option<&str>,
    explicit_cwd: Option<&std::path::Path>,
) -> Result<Option<SessionLease>> {
    if role.is_some_and(queue_cmd::role_is_stakeholder_only) {
        return Ok(None);
    }
    let Some(spec) = spec else { return Ok(None) };
    let leases = list_leases_strict(project_root, &[spec]).map_err(anyhow::Error::msg)?;
    let mut matching = leases.into_iter().filter(|lease| {
        lease_blocks_agent_dispatch(lease) && lease.scope.eq_ignore_ascii_case(spec)
    });
    let Some(lease) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() {
        anyhow::bail!("multiple sessions own this work; inspect `aida session leases --all` before dispatching");
    }
    let same_cwd = explicit_cwd
        .and_then(|path| path.canonicalize().ok())
        .zip(lease.worktree_path.canonicalize().ok())
        .is_some_and(|(cwd, owned)| cwd == owned && owned.is_dir());
    let same_role = role.is_none_or(|role| lease.role.as_deref() == Some(role));
    let live = process_probe::probe_live_claude_sessions();
    let active = agent_launch_lease_is_active(project_root, &lease, &live);
    if !same_cwd || !same_role || active {
        anyhow::bail!(
            "this work is already owned by session {} ({}, worktree: {}).\n  \
             Abort this dispatch, or re-enter the owning worktree after its agent exits: \
             use `--cwd '{}'` with the owning role and `--no-resume`.\n  \
             Inspect every blocking lease with `aida session leases --all`; \
             do not end the session merely to redispatch it.",
            lease.id,
            lease.role.as_deref().unwrap_or("unset role"),
            lease.worktree_path.display(),
            sh_single_quote(&lease.worktree_path.display().to_string()),
        );
    }
    defer_cmd::enforce_reasoned_operator_hold(project_root, spec)?;
    let branch = std::process::Command::new("git")
        .current_dir(&lease.worktree_path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .context("could not verify the owning worktree branch")?;
    if !branch.status.success() || String::from_utf8_lossy(&branch.stdout).trim() != lease.branch {
        anyhow::bail!(
            "the owning worktree no longer matches its recorded branch; inspect `aida session leases --all` before redispatching"
        );
    }
    Ok(Some(lease))
}

// A child gets its own lease identity, or sheds the launcher's unrelated one.
// trace:BUG-2002 | ai:codex
fn apply_agent_lease_identity(command: &mut std::process::Command, lease_id: Option<&str>) {
    if let Some(id) = lease_id {
        command.env("AIDA_SESSION_ID", id);
    } else {
        command.env_remove("AIDA_SESSION_ID");
    }
}

// trace:BUG-2002 | ai:codex
fn agent_launch_lease_is_active(
    project_root: &std::path::Path,
    lease: &SessionLease,
    live: &[process_probe::LiveSession],
) -> bool {
    live.iter().any(|session| {
        // A lease may record a symlink while /proc reports the real cwd.
        // Preserve the recorded path if it cannot be resolved.
        // trace:BUG-2002 | ai:codex
        let owned = lease
            .worktree_path
            .canonicalize()
            .unwrap_or_else(|_| lease.worktree_path.clone());
        let cwd = session
            .cwd
            .canonicalize()
            .unwrap_or_else(|_| session.cwd.clone());
        !session.stale_cwd && (cwd == owned || cwd.starts_with(&owned))
    }) || matches!(
        lease_state_for(lease, live, chrono::Utc::now()),
        LeaseState::Live
    ) || agent_registry::live_agents_covering_cwd(project_root, &lease.worktree_path) > 0
}

pub(crate) fn prepare_agent_launch(
    project_root: &std::path::Path,
    role: Option<String>,
    spec: Option<String>,
    agent_type: &str,
    custom_name: Option<String>,
) -> Result<AgentLaunchPlan> {
    prepare_agent_launch_with_cwd(project_root, role, spec, agent_type, custom_name, None)
}

// trace:BUG-2002 | ai:codex
fn prepare_agent_launch_with_cwd(
    project_root: &std::path::Path,
    role: Option<String>,
    spec: Option<String>,
    agent_type: &str,
    custom_name: Option<String>,
    explicit_cwd: Option<&std::path::Path>,
) -> Result<AgentLaunchPlan> {
    // Persona launches never turn an optional `--spec` into ownership. They
    // stay in the project checkout and carry no lease/worktree.
    // trace:TASK-1261 | ai:codex
    if role
        .as_deref()
        .is_some_and(queue_cmd::role_is_stakeholder_only)
    {
        let name = agent_registry::generate_or_validate_name(
            project_root,
            agent_type,
            role.as_deref(),
            custom_name.as_deref(),
        )?;
        ensure_agent_launch_cwd_not_git_metadata(project_root)?;
        return Ok(AgentLaunchPlan {
            project_root: project_root.to_path_buf(),
            launch_cwd: project_root.to_path_buf(),
            role,
            role_instance: RoleInstanceKind::Driver,
            current_spec: None,
            name,
            lease_id: None,
            native_session_id: agent_native_session_id_for_new(agent_type),
            resumed_from: None,
        });
    }
    match spec {
        Some(spec) => {
            // BUG-1510: refuse to mint a lease whose scope disagrees with the
            // one pending brief this agent is about to act on. Observed
            // end-to-end 2026-09-20 — a session's lease named one spec, its
            // brief named another, the session correctly did the brief's
            // work, and every downstream artifact (status/PR/verdict/shelve)
            // attached to the wrong spec. Checked here, before the lease is
            // minted, so a mismatch never gets the chance to be recorded.
            let pending_briefs =
                collect_agent_briefs_inner(project_root, Some(agent_type), false, false)
                    .unwrap_or_default();
            if let Some(conflict) = dispatch_lease_brief_conflict(&pending_briefs, &spec) {
                anyhow::bail!(
                    "refusing to dispatch: the lease about to be taken names `{}`, but the one \
                     pending brief for this agent names `{}` ({}).\n  \
                     A session's lease and its brief must name the same spec, or work done \
                     correctly against the brief gets recorded against the wrong one. \
                     Re-run with `--spec {}`, or ack the stale brief first: `aida brief ack {}`.",
                    spec,
                    conflict.spec_id,
                    conflict.path.display(),
                    conflict.spec_id,
                    conflict.path.display()
                );
            }

            // Recheck after prompts too: a lease may have appeared since preflight.
            // trace:BUG-2002 | ai:codex
            if let Some(lease) = preflight_agent_launch_lease(
                project_root,
                role.as_deref(),
                Some(&spec),
                explicit_cwd,
            )? {
                let plan_role = lease.role.clone().or(role);
                let name = agent_registry::generate_or_validate_name(
                    project_root,
                    agent_type,
                    plan_role.as_deref(),
                    custom_name.as_deref(),
                )?;
                ensure_agent_launch_cwd_not_git_metadata(&lease.worktree_path)?;
                return Ok(AgentLaunchPlan {
                    project_root: project_root.to_path_buf(),
                    launch_cwd: lease.worktree_path,
                    role: plan_role,
                    role_instance: RoleInstanceKind::Driver,
                    current_spec: Some(spec),
                    name,
                    lease_id: Some(lease.id),
                    native_session_id: agent_native_session_id_for_new(agent_type),
                    resumed_from: None,
                });
            }

            let previous_cwd = std::env::current_dir()?;
            std::env::set_current_dir(project_root)
                .with_context(|| format!("failed to cd to {}", project_root.display()))?;
            let start_result = session_start(
                &spec,
                /* branch */ None,
                /* base */ None,
                /* reuse_branch */ false,
                /* explicit_path */ None,
                /* forge_override */ None,
                /* branch_style */ "auto",
                /* launch_claude */ false,
                /* launch_title */ None,
                /* launch_set_title */ false,
                /* launch_name */ None,
                // STORY-495: inert here (launch_claude=false). The agent-new
                // launch path injects its own posture via `config.default_args`.
                /* launch_permission_mode */
                None,
                /* launch_contained */
                false,
                role.clone(),
                /* force_claim */ false,
                // STORY-714: config-driven (no per-command flag on this path
                // yet) — [worktree_pool] enabled decides.
                /* use_pool */
                None,
            );
            let restore_result = std::env::set_current_dir(&previous_cwd);
            restore_result
                .with_context(|| format!("failed to restore cwd to {}", previous_cwd.display()))?;
            start_result.with_context(|| {
                format!(
                    "failed to create session worktree for `{}`; run `aida session leases` \
                     to inspect conflicting sessions",
                    spec
                )
            })?;

            let lease = list_leases(project_root)
                .into_iter()
                .filter(|lease| lease.scope.eq_ignore_ascii_case(&spec))
                .max_by_key(|lease| lease.started_at)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "session_start completed but no lease for `{}` is visible",
                        spec
                    )
                })?;
            let plan_role = lease.role.clone().or(role.clone());
            let name = agent_registry::generate_or_validate_name(
                project_root,
                agent_type,
                plan_role.as_deref(),
                custom_name.as_deref(),
            )?;
            let lease_id = Some(lease.id.clone());
            ensure_agent_launch_cwd_not_git_metadata(&lease.worktree_path)?;
            Ok(AgentLaunchPlan {
                project_root: project_root.to_path_buf(),
                launch_cwd: lease.worktree_path,
                role: plan_role,
                role_instance: RoleInstanceKind::Driver,
                current_spec: Some(spec),
                name,
                lease_id,
                native_session_id: agent_native_session_id_for_new(agent_type),
                resumed_from: None,
            })
        }
        None => {
            let plan_role = role.clone();
            let role_instance = resolve_role_instance_for_launch(
                project_root,
                plan_role.as_deref(),
                None,
                project_root,
            );
            let name = agent_registry::generate_or_validate_name(
                project_root,
                agent_type,
                plan_role.as_deref(),
                custom_name.as_deref(),
            )?;
            ensure_agent_launch_cwd_not_git_metadata(project_root)?;
            Ok(AgentLaunchPlan {
                project_root: project_root.to_path_buf(),
                launch_cwd: project_root.to_path_buf(),
                role: plan_role,
                role_instance,
                current_spec: None,
                name,
                lease_id: None,
                native_session_id: agent_native_session_id_for_new(agent_type),
                resumed_from: None,
            })
        }
    }
}

// trace:STORY-791 | ai:codex
pub(crate) fn enforce_agent_singleton(
    project_root: &std::path::Path,
    plan: &AgentLaunchPlan,
    duplicate_check: bool,
) -> Result<()> {
    // BUG-1697: `--no-duplicate-check` is documented to skip the same-vendor/same-role
    // check entirely. It used to stop at the resume path, so the new-launch singleton had
    // no escape hatch and a hung holder could block every later seat on the scope.
    // trace:BUG-1697 | ai:claude
    if !duplicate_check {
        return Ok(());
    }
    if plan.role_instance == RoleInstanceKind::Companion {
        return Ok(());
    }
    enforce_agent_singleton_preflight(
        project_root,
        plan.role.as_deref(),
        plan.current_spec.as_deref(),
        &plan.launch_cwd,
    )
}

// trace:STORY-791 | ai:codex
pub(crate) fn enforce_agent_singleton_preflight(
    project_root: &std::path::Path,
    role: Option<&str>,
    current_spec: Option<&str>,
    worktree_path: &std::path::Path,
) -> Result<()> {
    let cfg = agent_registry::Config::load(project_root);
    if let Some(conflict) =
        agent_registry::same_scope_conflict(project_root, &cfg, role, current_spec, worktree_path)
    {
        anyhow::bail!(
            "agent role `{}` is already active on this scope: {} (session {}, spec {}, pid {}).",
            role.unwrap_or("(none)"),
            conflict.name_or_id,
            conflict.session_id,
            conflict.spec.as_deref().unwrap_or("(none)"),
            conflict.pid
        );
    }
    Ok(())
}

// trace:STORY-791 | ai:codex
pub(crate) fn resolve_agent_description(
    role: Option<&str>,
    description: Option<String>,
) -> Result<Option<String>> {
    let description = agent_registry::normalize_description(description);
    if description.is_some() {
        return Ok(description);
    }
    let needs_prompt = role
        .map(|r| matches!(r.to_ascii_lowercase().as_str(), "advisor" | "product"))
        .unwrap_or(false);
    if !needs_prompt || !std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let answer = inquire::Text::new("Agent description (optional):")
        .with_help_message(
            "Optional — one line describing what this advisor/product agent is for; leave blank to skip",
        )
        .prompt()?;
    Ok(agent_registry::normalize_description(Some(answer)))
}

// trace:STORY-436 | ai:codex
pub(crate) fn prepare_agent_launch_context(
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    options: AgentContextOptions,
) -> Result<Option<AgentLaunchContext>> {
    if !options.enabled {
        return Ok(None);
    }
    let token = std::env::var("AIDA_AGENT_REGISTRY_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let dir = plan
        .project_root
        .join(".aida")
        .join("agents")
        .join("context");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating agent context dir {}", dir.display()))?;
    let path = dir.join(format!("{}-{token}.context.md", config.agent_type));
    let body = render_agent_launch_context(config, plan, &token)?;
    std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;
    if options.show {
        println!("{body}");
    }
    Ok(Some(AgentLaunchContext { path, token }))
}

// trace:STORY-790 | ai:codex
pub(crate) fn prepare_agent_resume_context(
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    previous: &agent_registry::AgentRegistryEntry,
) -> Result<AgentLaunchContext> {
    let token = Uuid::now_v7().to_string();
    let dir = plan
        .project_root
        .join(".aida")
        .join("agents")
        .join("context");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating agent context dir {}", dir.display()))?;
    let path = dir.join(format!("{}-resume-{token}.context.md", config.agent_type));
    let mut body = render_agent_resume_drift_brief(plan, previous)?;
    body.push('\n');
    body.push_str(&render_agent_launch_context(config, plan, &token)?);
    std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;
    Ok(AgentLaunchContext { path, token })
}

// trace:STORY-790 | ai:codex
pub(crate) fn render_agent_resume_drift_brief(
    plan: &AgentLaunchPlan,
    previous: &agent_registry::AgentRegistryEntry,
) -> Result<String> {
    let mut out = String::new();
    out.push_str("# DRIFT BRIEF\n\n");
    out.push_str("This is a fresh AIDA snapshot for a resumed native agent transcript.\n\n");
    out.push_str(&format!("- Resumed from registry entry: {}\n", previous.id));
    if let Some(ended_at) = previous.ended_at {
        out.push_str(&format!(
            "- Previous session ended at: {}\n",
            ended_at.to_rfc3339()
        ));
    }
    if let Some(spec) = plan.current_spec.as_deref() {
        // Then-vs-now: the status frozen when the previous session ended,
        // against the status at resume time — the drift itself, not just the
        // present. trace:STORY-790 | ai:claude
        let now_status = current_spec_status_line(&plan.project_root, spec)
            .unwrap_or_else(|| "unknown".to_string());
        match previous.spec_status_at_end.as_deref() {
            Some(then) if then != now_status => {
                out.push_str(&format!(
                    "- Spec status: {then} (at exit) → {now_status} (now)\n"
                ));
            }
            Some(then) => {
                out.push_str(&format!("- Spec status: {then} (unchanged since exit)\n"));
            }
            None => {
                out.push_str(&format!(
                    "- Spec status now: {now_status} (status at previous exit not recorded)\n"
                ));
            }
        }
        out.push_str(&format!(
            "- PR state now: {}\n",
            pr_state_for_spec(&plan.project_root, spec)
                .unwrap_or_else(|| "not detected".to_string())
        ));
    }
    if let Some(ended_at) = previous.ended_at {
        out.push_str(&format!(
            "- Main commits since previous exit: {}\n",
            main_commit_count_since(&plan.project_root, ended_at)
                .unwrap_or_else(|| "unknown".to_string())
        ));
        // Coordination drift, scoped to ended_at: mail addressed to this
        // agent (or broadcast) and briefs filed since it exited — the two
        // channels where work routes to an absent agent.
        // trace:STORY-790 | ai:claude
        out.push_str(&format!(
            "- Mail for this agent since previous exit: {}\n",
            mail_count_for_agent_since(&plan.project_root, previous, ended_at)
        ));
        out.push_str(&format!(
            "- Briefs filed for this agent since previous exit: {}\n",
            brief_count_for_agent_since(&plan.project_root, previous, ended_at)
        ));
    }
    out.push_str("\nResume rule: reconcile this drift before continuing the old transcript.\n");
    Ok(out)
}

/// Mail addressed to the resumed agent — by name, role, agent type, or
/// broadcast — with a timestamp at or after the previous session's end.
// trace:STORY-790 | ai:claude
pub(crate) fn mail_count_for_agent_since(
    project_root: &std::path::Path,
    previous: &agent_registry::AgentRegistryEntry,
    ended_at: chrono::DateTime<chrono::Utc>,
) -> usize {
    let cutoff = ended_at.timestamp_millis();
    let msgs = mailbox_store::read_local_messages(project_root).unwrap_or_default();
    msgs.iter()
        .filter(|m| m.timestamp >= cutoff)
        .filter(|m| match &m.to {
            aida_core::mailbox::Recipient::Broadcast => true,
            aida_core::mailbox::Recipient::Agent(who) => {
                previous.name.as_deref() == Some(who.as_str())
                    || previous.role.as_deref() == Some(who.as_str())
                    || previous.agent_type == *who
            }
        })
        .count()
}

/// Briefs filed under `.aida/agent-briefs/<name|type>/` since the previous
/// session ended (file mtime is the filing time).
// trace:STORY-790 | ai:claude
pub(crate) fn brief_count_for_agent_since(
    project_root: &std::path::Path,
    previous: &agent_registry::AgentRegistryEntry,
    ended_at: chrono::DateTime<chrono::Utc>,
) -> usize {
    let cutoff: std::time::SystemTime = ended_at.into();
    let mut count = 0;
    for who in [previous.name.as_deref(), Some(previous.agent_type.as_str())]
        .into_iter()
        .flatten()
    {
        let dir = project_root.join(".aida").join("agent-briefs").join(who);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                if let Ok(mtime) = e.metadata().and_then(|m| m.modified()) {
                    if mtime >= cutoff {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

// trace:STORY-790 | ai:codex
pub(crate) fn current_spec_status_line(
    project_root: &std::path::Path,
    spec: &str,
) -> Option<String> {
    let storage = Storage::new(project_root.join(".aida-store"));
    let store = storage.load().ok()?;
    let req = store
        .requirements
        .iter()
        .find(|req| spec_matches(req, spec))?;
    Some(format!("{} — {}", req.status, req.title))
}

// trace:STORY-790 | ai:codex
pub(crate) fn main_commit_count_since(
    project_root: &std::path::Path,
    since: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let default_ref = resolve_default_branch_ref(project_root)?;
    let since_arg = since.to_rfc3339();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-list",
            "--count",
            &format!("--since={since_arg}"),
            &default_ref,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// trace:STORY-790 | ai:codex
pub(crate) fn pr_state_for_spec(project_root: &std::path::Path, spec: &str) -> Option<String> {
    let gh = resolve_gh_binary()?;
    let out = std::process::Command::new(gh)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--state",
            "all",
            "--search",
            spec,
            "--json",
            "number,state,title",
            "--limit",
            "5",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let first = value.as_array()?.first()?;
    let number = first.get("number")?.as_u64()?;
    let state = first.get("state")?.as_str()?;
    let title = first.get("title").and_then(|v| v.as_str()).unwrap_or("");
    Some(format!("PR-{number} {state} — {title}"))
}

/// BUG-408: the dry counterpart of `prepare_agent_launch`. Computes what the
/// launch plan WOULD be — resolved role, generated stable name, current spec —
/// WITHOUT any side effects: no `session_start`, so no worktree is created, no
/// lease is written, and the spec's status is not flipped to InProgress. Used
/// by `--show-context`, which is a pure preview. The working directory is
/// reported as the project root (the dedicated worktree only exists after a
/// real launch); the preview banner makes that explicit. `generate_or_validate_name`
/// is read-only (it inspects the registry to pick a unique name but reserves
/// nothing until the agent record is written at launch), so it is safe here.
/// trace:BUG-408 | ai:claude
pub(crate) fn prepare_agent_launch_dry(
    project_root: &std::path::Path,
    role: Option<String>,
    spec: Option<String>,
    agent_type: &str,
    custom_name: Option<String>,
) -> Result<AgentLaunchPlan> {
    let plan_role = role;
    let persona = plan_role
        .as_deref()
        .is_some_and(queue_cmd::role_is_stakeholder_only);
    let role_instance = resolve_role_instance_for_launch(
        project_root,
        plan_role.as_deref(),
        spec.as_deref(),
        project_root,
    );
    let name = agent_registry::generate_or_validate_name(
        project_root,
        agent_type,
        plan_role.as_deref(),
        custom_name.as_deref(),
    )?;
    ensure_agent_launch_cwd_not_git_metadata(project_root)?;
    Ok(AgentLaunchPlan {
        project_root: project_root.to_path_buf(),
        launch_cwd: project_root.to_path_buf(),
        role: plan_role,
        role_instance,
        // Personas may discuss a supplied spec but never own it.
        current_spec: if persona { None } else { spec },
        name,
        lease_id: None,
        native_session_id: agent_native_session_id_for_new(agent_type),
        resumed_from: None,
    })
}

/// BUG-408: shared `--show-context` preview for both the foreground and `--bg`
/// launch paths. Builds a dry plan, renders the launch-context snapshot, and
/// prints a preview banner + the body. No session is started, no worktree or
/// lease is created, and the spec status is untouched. trace:BUG-408 | ai:claude
pub(crate) fn print_dry_launch_context(
    project_root: &std::path::Path,
    role: Option<String>,
    spec: Option<String>,
    config: &AgentLaunchConfig,
    name: Option<String>,
) -> Result<()> {
    let plan = prepare_agent_launch_dry(project_root, role, spec, config.agent_type, name)?;
    let token = std::env::var("AIDA_AGENT_REGISTRY_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let body = render_agent_launch_context(config, &plan, &token)?;
    println!(
        "# DRY PREVIEW — no session was started.\n\
         A dedicated worktree + lease are created (and the spec flips to InProgress) \
         only on a real launch (drop `--show-context`).\n"
    );
    let confirm = bypass_confirm::load(project_root);
    // trace:BUG-1720 | ai:codex
    let has_bypass = bypass_confirm::args_have_bypass(&config.default_args);
    let decision = bypass_confirm::decide(
        has_bypass,
        bypass_confirm::BypassSource::Configured,
        confirm.on,
        authority_stdin_is_terminal() && authority_stdout_is_terminal(),
    );
    println!(
        "confirm_bypass: {} ({}) · preview decision: {:?} (stdin was not read)",
        if confirm.on { "on" } else { "off" },
        confirm.source,
        decision
    );
    println!("{body}");
    Ok(())
}

// trace:STORY-436 | ai:codex
pub(crate) fn render_agent_launch_context(
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    token: &str,
) -> Result<String> {
    let role = plan.role.as_deref().unwrap_or("unspecified");
    let mut out = String::new();
    out.push_str("# AIDA Launch Context\n\n");
    out.push_str("This is a point-in-time spawn snapshot. Briefs, queue changes, leases, and registry heartbeats filed after launch are not reflected here; use AIDA's shell/event-driven watchers rather than recurring model-side polling.\n\n");
    out.push_str("## Launch\n\n");
    out.push_str(&format!("- Agent: {}\n", config.agent_type));
    out.push_str(&format!("- Role: {role}\n"));
    out.push_str(&format!(
        "- Current spec: {}\n",
        plan.current_spec.as_deref().unwrap_or("(none)")
    ));
    out.push_str(&format!(
        "- Project root: {}\n",
        plan.project_root.display()
    ));
    out.push_str(&format!(
        "- Working directory: {}\n",
        plan.launch_cwd.display()
    ));
    // TASK-1482: surface any intentional presentation-env inheritance in the
    // durable snapshot too, not just the `--no-exec` preview — the operator
    // reading this file later should see it, not just the launcher's stdout.
    out.push_str(&format!(
        "- Presentation env: {}\n",
        describe_presentation_env(&resolve_presentation_env_policy(&plan.project_root)?)
    ));
    // STORY-711 slice 2: when a pending brief for this spec/agent carries an
    // `authorized_by` token, carry it into the durable launch-context
    // snapshot — the automatic advisor-lock gate reads this line at commit
    // time (`locking_gate::my_lock_token`), never a bare env var (spoofable +
    // vanishes on respawn). trace:TASK-1140 | ai:claude
    if let Some(spec) = plan.current_spec.as_deref() {
        if let Some(authorized_by) =
            launch_authorized_by(&plan.project_root, config.agent_type, spec)
        {
            out.push_str(&format!("- Authorized by: {authorized_by}\n"));
        }
    }
    out.push_str(&format!("- Context token: {token}\n\n"));

    out.push_str("## Role Guidance\n\n");
    out.push_str(&role_guidance_for(&plan.project_root, role));
    out.push_str("\n\n");

    // A cold product/advisor session must see the independence boundary and
    // the live item at its gate without relying on a prior conversation.
    // Mechanised recurring duties stay in the Due Jobs section below.
    // trace:STORY-1351 | ai:codex
    if matches!(canonical_role_name(role).as_str(), "product" | "advisor") {
        out.push_str("## Seat Gate\n\n");
        out.push_str(&render_seat_gate_section(&plan.project_root, role));
        out.push('\n');
    }

    out.push_str("## Active Session\n\n");
    if let Some(spec) = &plan.current_spec {
        let ship_instruction = orchestrated_implementer_ship_instruction(
            std::env::var(orchestrator::VARIANT_ENV).ok().as_deref(),
        );
        out.push_str(&format!(
            "- **Spec: {spec} (your scope for this session)**\n"
        ));
        let lease = list_leases(&plan.project_root)
            .into_iter()
            .filter(|lease| lease.scope.eq_ignore_ascii_case(spec))
            .max_by_key(|lease| lease.started_at);
        if let Some(lease) = lease {
            out.push_str(&format!("- Lease: {}\n", lease.id));
            out.push_str(&format!(
                "- Lease role: {}\n",
                lease.role.as_deref().unwrap_or("(unset)")
            ));
            out.push_str(&format!("- Branch: {}\n", lease.branch));
            out.push_str(&format!("- Worktree: {}\n", lease.worktree_path.display()));
        } else {
            out.push_str(&format!("- No active lease found for {spec}.\n"));
        }
        out.push_str(&format!(
            "\n**SCOPE BINDING**: This session was launched with `--spec {spec}`. \
You are scoped to {spec}. Drive it to completion, {ship_instruction} Do not \
pick up other specs from this session; if you need the next pickup, start a \
fresh `aida agent new ... --spec NEXT-ID` session.\n"
        ));
    } else {
        out.push_str("- No spec was provided at launch; start from the relevant queue head or pending brief.\n");
    }
    out.push('\n');

    out.push_str("## Pending Briefs\n\n");
    // BUG-569: bare agent-type snapshot scan — stay silent on type-class ambiguity.
    let briefs =
        collect_agent_briefs_inner(&plan.project_root, Some(config.agent_type), false, false)
            .unwrap_or_default();
    if briefs.is_empty() {
        out.push_str("_No pending briefs for this agent at launch._\n\n");
    } else {
        for brief in briefs {
            let title = brief_title(&brief.path).unwrap_or_else(|| "(title unavailable)".into());
            out.push_str(&format!(
                "- {} — {} — {}\n",
                brief.spec_id,
                title,
                brief.path.display()
            ));
        }
        out.push('\n');
    }

    // STORY-619: make the launch snapshot self-describing about the mailbox for
    // EVERY vendor. Only Claude Code gets the prompt-bound aida-mail-notice.sh
    // hook; codex/antigravity/etc. get nothing automatic. The launcher already
    // writes this context file, so embedding the current unread snapshot + the
    // zero-token shell/event wait here closes the Claude-only delivery-awareness
    // gap. The agent's identity is plan.name (the
    // AIDA_USER the launcher exports for the spawned agent, per BUG-558).
    // trace:STORY-619 | ai:claude
    out.push_str("## Mailbox\n\n");
    out.push_str(&render_launch_mailbox_section(
        &plan.project_root,
        &plan.name,
    ));
    out.push('\n');

    // STORY-1226: the seat's due jobs from the `[schedule]` registry, so a
    // freshly launched seat starts with the periodic work that applies to it
    // — identical under every vendor (one registry, one view). No vendor is
    // instructed to create an in-session recurring poll.
    // trace:STORY-1226 | ai:claude
    out.push_str("## Due Jobs\n\n");
    out.push_str(&render_launch_due_jobs_section(
        &plan.project_root,
        plan.role.as_deref(),
        config.agent_type,
    ));
    out.push('\n');

    out.push_str("## Queue Snapshot\n\n");
    if plan.current_spec.is_none() {
        if let Some(line) = queue_head_line(&plan.project_root, plan.role.as_deref()) {
            out.push_str(&line);
            out.push('\n');
        } else {
            out.push_str("- No queue head could be resolved for this role.\n");
        }
    } else {
        out.push_str("- A spec was provided explicitly; queue head selection is bypassed.\n");
    }
    out.push('\n');

    out.push_str("## Next Commands\n\n");
    out.push_str(&format!(
        "- **What do I do next?** `{}`\n",
        seat_next_command(role)
    ));
    out.push_str("- Read AGENTS.md and any agent-specific setup document under docs/agents/.\n");
    out.push_str("- Check pending briefs with `aida brief list --for-agent ");
    out.push_str(config.agent_type);
    out.push_str("`.\n");
    if let Some(spec) = &plan.current_spec {
        out.push_str(&format!(
            "- Inspect the assigned spec with `aida show {spec}`.\n"
        ));
    }
    out.push_str("- Ship with a trailing-parens spec trailer in the commit subject, ");
    out.push_str(orchestrated_implementer_ship_instruction(
        std::env::var(orchestrator::VARIANT_ENV).ok().as_deref(),
    ));
    out.push('\n');
    Ok(out)
}

// trace:STORY-1351 | ai:codex
pub(crate) fn seat_next_command(role: &str) -> String {
    match canonical_role_name(role).as_str() {
        "advisor" => "aida advisor".to_string(),
        "product" => "aida queue next --for product".to_string(),
        other => format!("aida queue next --for {other}"),
    }
}

// trace:STORY-1351 | ai:codex
pub(crate) fn render_seat_gate_section(project_root: &std::path::Path, role: &str) -> String {
    let seat = canonical_role_name(role);
    let responsibility = match seat.as_str() {
        "advisor" => "Independent gate: disposition, grounded design forks, rework sufficiency, and merge readiness. Never implement or merge code you authored.",
        "product" => "Product gate: requirement quality, acceptance, ordering, and wave continuity. Never approve your own disputed judgment or merge implementation.",
        _ => "Follow the active role contract.",
    };
    let waiting = queue_head_line(project_root, Some(&seat))
        .unwrap_or_else(|| format!("- Nothing is waiting on the `{seat}` role queue."));
    format!("{responsibility}\n\nAt your gate now:\n{waiting}\n")
}

/// STORY-619: build the launch-context `## Mailbox` body for a spawned agent.
///
/// Closes the cross-vendor delivery-awareness gap: only Claude Code gets the
/// prompt-bound `aida-mail-notice.sh` hook, so codex/antigravity/etc. otherwise
/// get no mailbox awareness at all. Because the launcher writes the context file
/// as plain text and EVERY vendor reads it at startup, embedding (a) the current
/// unread-mail snapshot and (b) the explicit inbox command + zero-token wait
/// makes the snapshot self-describing for any vendor.
///
/// Reuses the existing notice renderer (`render_mailbox_notice`) and the pure
/// `build_notice` core rather than duplicating inbox logic. `agent_name` is the
/// spawned agent's stable identity (the `AIDA_USER` the launcher exports, per
/// BUG-558), so the snapshot is keyed to the agent's own inbox. The guidance
/// line always renders (even when caught up) so the agent learns the mailbox
/// exists and how to wait for it without waking the model.
// trace:STORY-619 trace:BUG-1589 | ai:claude+codex
pub(crate) fn render_launch_mailbox_section(
    project_root: &std::path::Path,
    agent_name: &str,
) -> String {
    let mut out = String::new();

    // The orphan-store worktree (canonical mailbox layer) lives at
    // <project_root>/.aida-store; the local layer is <project_root>/.aida/mailbox.
    // Mirror handle_mailbox_command's derivation. Reads return empty (never
    // error) when the store/mailbox is absent, so an offline or fresh clone
    // simply yields the caught-up branch + guidance.
    let store_root = project_root.join(".aida-store");
    let local = mailbox_store::read_local_messages(project_root).unwrap_or_default();
    let canonical = mailbox_store::read_canonical_messages(&store_root).unwrap_or_default();
    let merged = aida_core::mailbox::merge_dedup(&local, &canonical);
    let watermarks = mailbox_store::read_all_watermarks(project_root).unwrap_or_default();
    let identities = [agent_name.to_string()];
    let summary = aida_core::mailbox::build_notice(
        identities.iter().map(String::as_str),
        &merged,
        &watermarks,
        aida_core::mailbox::NOTICE_DEFAULT_CAP,
    );

    if summary.is_empty() {
        out.push_str(&format!(
            "- No unread mailbox messages for `{agent_name}` at launch.\n"
        ));
    } else {
        out.push_str(&render_mailbox_notice(&summary, &identities));
        out.push('\n');
    }

    // Always include the guidance — only Claude Code gets the auto-hook, so every
    // other vendor relies on this line to know the mailbox exists and how to
    // wait outside the model until it becomes actionable.
    out.push_str(
        "Other agents send you mail here. Only Claude Code auto-surfaces new mail; \
every other vendor must use a shell/event-driven wait. Check your inbox with \
`aida mailbox inbox` (reads + acks) or `aida mailbox notice` (ambient, \
non-marking peek). For unattended waiting, keep the model asleep behind a \
`Monitor` over `aida watch --emit-wakes` or a background shell wait around \
`aida awaiting --notice`; never create model-side CronCreate, /loop, or \
ScheduleWakeup mailbox polls.\n",
    );

    out
}

/// STORY-1226: build the launch-context `## Due Jobs` body for a spawned seat.
///
/// Lists the registry's seat jobs that apply to `role` — due ones as
/// due-lines (with the `aida schedule done <job>` report-back), the rest as
/// "next due" so the seat knows its periodic responsibilities at launch. A
/// Every vendor gets the same jobs and report-back contract, while recurring
/// waiting stays in the shell/substrate rather than in model turns.
// trace:STORY-1226 trace:BUG-1589 | ai:claude+codex
pub(crate) fn render_launch_due_jobs_section(
    project_root: &std::path::Path,
    role: Option<&str>,
    agent_type: &str,
) -> String {
    let mut out = String::new();
    let seat = role.map(canonical_role_name);
    let due = maintenance_schedule::due_seat_jobs(project_root, seat.as_deref());
    if due.is_empty() {
        out.push_str(&format!(
            "- No scheduled seat jobs are due for `{}` at launch. Poll with `aida schedule due{}`.\n",
            seat.as_deref().unwrap_or("any seat"),
            seat.as_deref()
                .map(|s| format!(" --seat {s}"))
                .unwrap_or_default()
        ));
    } else {
        out.push_str(&maintenance_schedule::render_due_jobs_block(
            &due,
            seat.as_deref().unwrap_or("*"),
        ));
    }
    let _ = agent_type;
    out.push_str(
        "Recurring waits belong to AIDA's shell/event scheduler. Never mirror seat jobs with \
model-side CronCreate, /loop, or ScheduleWakeup; report completed jobs with \
`aida schedule done <job>` so the shared ledger stays true.\n",
    );
    out
}

/// STORY-711 slice 2: the `authorized_by` token recorded on the most recent
/// pending brief for `agent_type`/`spec_id`, if any. Lets
/// `render_agent_launch_context` carry an advisor's `aida brief <agent>
/// <SPEC> --authorized-by <advisor-id>` into the spawned session's durable
/// launch-context snapshot, which the automatic advisor-lock gate
/// (`locking_gate::my_lock_token`) later reads at commit time. `None` when no
/// pending brief for this spec/agent carries a token (the overwhelmingly
/// common case — most briefs carry no lock authorization at all).
// trace:TASK-1140 | ai:claude
pub(crate) fn launch_authorized_by(
    project_root: &std::path::Path,
    agent_type: &str,
    spec_id: &str,
) -> Option<String> {
    // BUG-569: bare agent-type snapshot scan — stay silent on type-class
    // ambiguity, mirroring the other launch-context reads in this function.
    let briefs = collect_agent_briefs_inner(project_root, Some(agent_type), false, false).ok()?;
    briefs
        .iter()
        .rev()
        .find(|b| b.spec_id.eq_ignore_ascii_case(spec_id))
        .and_then(|b| b.authorized_by.clone())
}

// trace:STORY-436 | ai:codex
pub(crate) fn brief_title(path: &std::path::Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    body.lines()
        .find_map(|line| line.trim().strip_prefix("- Title:").map(str::trim))
        .filter(|title| !title.is_empty())
        .map(str::to_string)
}

// trace:STORY-436 | ai:codex
pub(crate) fn queue_head_line(
    project_root: &std::path::Path,
    role: Option<&str>,
) -> Option<String> {
    let storage = Storage::new(project_root.join(".aida-store"));
    let store = storage.load().ok()?;
    let user_id = current_user_id(None);
    // Role-routed work a peer queued lives in the PEER's file; fold it in so
    // the role-context snapshot's queue head matches `aida queue list`.
    // trace:BUG-774 | ai:claude
    let mut entries =
        queue_role_fallback::queue_list_with_role_fallback(&storage, &user_id, role, false).ok()?;
    entries.sort_by_key(|entry| entry.position);
    let head = entries.into_iter().find(|entry| match role {
        Some(role) => entry.for_role.as_deref() == Some(role),
        None => entry.for_role.is_none(),
    })?;
    let req = store
        .requirements
        .iter()
        .find(|req| req.id == head.requirement_id)?;
    Some(format!(
        "- Queue head: {} — {}",
        req.display_id(),
        req.title
    ))
}

// trace:TASK-1232 | ai:codex
pub(crate) fn resolved_agent_program_and_args(
    binary: &std::path::Path,
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    prompt_args: &[String],
    wrap_claude: bool,
) -> Result<(std::ffi::OsString, Vec<String>)> {
    let mut all_args: Vec<String> = config.default_args.clone();
    all_args.extend(prompt_args.iter().cloned());
    if wrap_claude && config.agent_type == "claude" {
        let (prog, args) = crate::session::os_wrapped_program_and_args(
            &plan.launch_cwd,
            &binary.to_string_lossy(),
            all_args,
        )?;
        Ok((std::ffi::OsString::from(prog), args))
    } else {
        Ok((binary.as_os_str().to_os_string(), all_args))
    }
}

// trace:TASK-1232 | ai:codex
// trace:TASK-1467 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn print_agent_launch_noexec(
    binary: &std::path::Path,
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    prompt: &AgentPromptOptions,
    prompt_args: &[String],
    wrap_claude: bool,
    context_enabled: bool,
) -> Result<()> {
    print!(
        "{}",
        render_agent_launch_noexec(
            binary,
            config,
            plan,
            prompt,
            prompt_args,
            wrap_claude,
            context_enabled
        )?
    );
    Ok(())
}

/// TASK-1467: the complete resolved launch contract — argv, active role,
/// explicit-vs-generated prompt, the launch-context snapshot PATH it would
/// write (not its body — that's `--show-context`), the AIDA environment
/// inputs the child process would see, and the repository guidance files
/// (AGENTS.md/CLAUDE.md) the child is expected to consume. Built entirely
/// from the DRY plan (`prepare_agent_launch_dry`) — no worktree, lease,
/// status transition, network sync, or file write happens to produce it.
// trace:TASK-1232 | ai:codex
// trace:TASK-1467 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_agent_launch_noexec(
    binary: &std::path::Path,
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    prompt: &AgentPromptOptions,
    prompt_args: &[String],
    wrap_claude: bool,
    context_enabled: bool,
) -> Result<String> {
    let (program, exec_args) =
        resolved_agent_program_and_args(binary, config, plan, prompt_args, wrap_claude)?;
    let mut argv = vec![program.to_string_lossy().to_string()];
    argv.extend(exec_args.clone());
    let agents_bypass = load_agents_bypass(&plan.project_root).unwrap_or(false);
    // trace:BUG-2146 | ai:claude
    let agents_contained = load_agents_contained(&plan.project_root)?;
    // TASK-1467: a dry-computed path — same naming scheme `prepare_agent_launch_context`
    // uses, but nothing is written here (no side effects). The real launch mints
    // its own token, so this path is illustrative of the SHAPE, not a promise of
    // the exact filename a subsequent real launch will use.
    let context_path = context_enabled.then(|| {
        plan.project_root
            .join(".aida")
            .join("agents")
            .join("context")
            .join(format!("{}-<token>.context.md", config.agent_type))
    });

    let mut out = String::new();
    out.push_str(
        "# AIDA agent launch preview — complete launch contract; no process was started.\n",
    );
    out.push_str(&format!("agent: {}\n", config.agent_type));
    out.push_str(&format!("name: {}\n", plan.name));
    if let Some(role) = &plan.role {
        out.push_str(&format!("role: {role}\n"));
    }
    out.push_str(&format!("role_instance: {}\n", plan.role_instance.as_str()));
    if let Some(spec) = &plan.current_spec {
        out.push_str(&format!("spec: {spec}\n"));
    }
    out.push_str(&format!("cwd: {}\n", plan.launch_cwd.display()));
    out.push_str(&format!("command: {}\n", shell_join_display(&argv)));
    // Resolve independently at preview time so operators can see the exact
    // coordinating executable and profile the child environment will inherit.
    // trace:TASK-1499 | ai:codex
    let aida = aida_bin::process()?;
    out.push_str(&format!(
        "aida_executable: {} (source: {}, profile: {})\n",
        aida.path.display(),
        aida.source,
        aida.profile.map(|p| p.name()).unwrap_or("n/a")
    ));
    if aida.stale {
        out.push_str("aida_build_stale: alternate build is newer than selected build\n");
    }
    out.push_str("permission_posture:\n");
    let confirm = bypass_confirm::load(&plan.project_root);
    out.push_str(&format!(
        "  confirm bypass: {} ({})\n",
        if confirm.on { "on" } else { "off" },
        confirm.source
    ));
    // trace:BUG-1720 | ai:codex
    let bypass_in_argv = bypass_confirm::args_have_bypass(&exec_args);
    let preview_decision = bypass_confirm::decide(
        bypass_in_argv,
        bypass_confirm::BypassSource::Configured,
        confirm.on,
        authority_stdin_is_terminal() && authority_stdout_is_terminal(),
    );
    out.push_str(&format!(
        "  confirm decision: {preview_decision:?} (preview; stdin was not read)\n"
    ));
    out.push_str(&format!("  agents.toml bypass: {agents_bypass}\n"));
    out.push_str(&format!("  agents.toml contained: {agents_contained}\n"));
    out.push_str(&format!(
        "  resolved: {}\n",
        resolved_agent_permission_summary(config.agent_type, &exec_args)
    ));
    if config.agent_type == "claude" {
        // TASK-1558 AC4: never make the operator guess which AIDA surface the seat got.
        // trace:TASK-1558 | ai:claude
        let surface = load_agents_mcp(&plan.project_root)
            .map(|s| s.as_str())
            .unwrap_or("(invalid — see [agents] mcp)");
        out.push_str(&format!("  aida surface (agents.toml mcp): {surface}\n"));
    }
    if config.agent_type == "codex" {
        out.push_str(&format!(
            "  codex sandbox: {}\n",
            resolved_flag_value(&exec_args, "--sandbox").unwrap_or("codex default")
        ));
        out.push_str(&format!(
            "  codex ask-for-approval: {}\n",
            resolved_flag_value(&exec_args, "--ask-for-approval").unwrap_or("codex default")
        ));
    }

    // TASK-1467: prompt source, distinguishing explicit --prompt from an
    // auto-generated one so a reader isn't guessing which they're looking at.
    out.push_str(&format!(
        "prompt_source: {}\n",
        prompt_source_label(prompt, prompt_args)
    ));

    // TASK-1467: where the launch-context snapshot WOULD be written (its
    // body is `--show-context`, not repeated here).
    match &context_path {
        Some(path) => out.push_str(&format!(
            "launch_context_snapshot: {} (not written by this preview)\n",
            path.display()
        )),
        None => out.push_str("launch_context_snapshot: (disabled — --no-context)\n"),
    }

    // TASK-1467: the AIDA_* environment the child process would see, mirroring
    // exactly what `run_tracked_agent` sets on the real launch.
    out.push_str("env:\n");
    for (key, value) in agent_launch_env_inputs(config, plan, context_path.as_deref()) {
        out.push_str(&format!("  {key}={value}\n"));
    }

    // TASK-1482: the operator-local presentation env this launch would strip
    // (default) or keep (explicit `[agents] inherit_env` opt-in) — distinct
    // from the `env:` section above, which is AIDA's OWN injected vars, not
    // ambient ones read from the launching shell.
    out.push_str(&format!(
        "presentation_env: {}\n",
        describe_presentation_env(&resolve_presentation_env_policy(&plan.project_root)?)
    ));

    // TASK-1467: repository guidance files the child is expected to read on
    // startup, with whether each is actually present in this checkout.
    out.push_str("guidance_files:\n");
    for (rel_path, present) in agent_guidance_files(config.agent_type, &plan.project_root) {
        let marker = if present { "present" } else { "absent" };
        out.push_str(&format!("  {rel_path} ({marker})\n"));
    }

    Ok(out)
}

/// TASK-1498: build a DISPLAY-only copy of `prompt_args` for verbose
/// diagnostics — the actual prompt text (last element; see
/// `agent_initial_prompt_args`, which returns `[]`, `[prompt]`, or `[flag,
/// prompt]`) is replaced with a length-only placeholder. The real
/// `prompt_args` passed to the actual launch is untouched; this copy is
/// used ONLY to build the diagnostic command line, so the initial message —
/// which may be long, may embed a `--prompt`-supplied secret, or may simply
/// not be the operator's business to have echoed to a terminal/log by
/// default — never appears in `--verbose` output. `prompt_source` already
/// says whether it was explicit or generated.
// trace:TASK-1498 | ai:claude
pub(crate) fn redact_prompt_arg_for_diagnostics(prompt_args: &[String]) -> Vec<String> {
    let Some((last, rest)) = prompt_args.split_last() else {
        return Vec::new();
    };
    let mut out = rest.to_vec();
    out.push(format!(
        "<redacted prompt — {} chars; see prompt_source>",
        last.chars().count()
    ));
    out
}

/// TASK-1498: apply the STORY-582 `redact_secrets` scrub to only the
/// `command: ...` line of a rendered launch contract, leaving every other
/// line (and their newlines) untouched. `redact_secrets` operates on
/// whitespace-split tokens and joins with a single space, which would
/// collapse a multi-line report onto one line if applied to the whole body —
/// scoping it to the one line that carries the generated child argv avoids
/// that while still catching a token/secret-looking value an `--extra-flag`
/// or default arg might carry.
// trace:TASK-1498 | ai:claude
pub(crate) fn redact_launch_command_line(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("command: ") {
            let (content, newline) = match rest.strip_suffix('\n') {
                Some(c) => (c, "\n"),
                None => (rest, ""),
            };
            out.push_str("command: ");
            out.push_str(&redact_secrets(content));
            out.push_str(newline);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// TASK-1498: `--verbose` pre-spawn diagnostics for a launch that REALLY
/// executes — distinct from `--no-exec`, which previews the identical
/// contract and exits without spawning. Reuses `render_agent_launch_noexec`
/// verbatim for the body (argv, permission posture, prompt source, context
/// snapshot path, AIDA env inputs, guidance files) so the two can never
/// drift apart; only the banner line differs (this one says a process IS
/// being spawned), the prompt text is replaced by a length-only placeholder
/// (`redact_prompt_arg_for_diagnostics`), and the `command:` line is passed
/// through `redact_secrets` (`redact_launch_command_line`) so a
/// secret-looking argv value is never printed.
// trace:TASK-1498 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_agent_launch_diagnostics(
    binary: &std::path::Path,
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    prompt: &AgentPromptOptions,
    prompt_args: &[String],
    wrap_claude: bool,
    context_enabled: bool,
) -> Result<String> {
    let display_prompt_args = redact_prompt_arg_for_diagnostics(prompt_args);
    let body = render_agent_launch_noexec(
        binary,
        config,
        plan,
        prompt,
        &display_prompt_args,
        wrap_claude,
        context_enabled,
    )?;
    let body = redact_launch_command_line(&body);
    let rest = body.splitn(2, '\n').nth(1).unwrap_or("");
    Ok(format!(
        "# AIDA agent launch diagnostics (--verbose) — resolved launch contract; spawning now.\n{rest}"
    ))
}

/// TASK-1467: label the prompt that would be sent as `explicit` (from
/// `--prompt`) or `generated` (the role-aware launch prompt), or note that no
/// initial message would be sent at all.
pub(crate) fn prompt_source_label(
    prompt: &AgentPromptOptions,
    prompt_args: &[String],
) -> &'static str {
    let explicit = prompt
        .explicit_prompt
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .is_some();
    match (explicit, prompt_args.is_empty()) {
        (true, _) => "explicit (--prompt/--prompt-file)",
        // trace:STORY-1471 | ai:claude
        (false, false) => "generated (role launch prompt)",
        (false, true) => "none (--no-prompt)",
    }
}

/// TASK-1467: `--show-prompt` — print ONLY the exact initial prompt text
/// that would be sent (explicit or generated), then exit. Narrower than
/// `--no-exec`; same no-side-effects contract (built from the dry plan).
pub(crate) fn print_agent_show_prompt(
    prompt: &AgentPromptOptions,
    prompt_args: &[String],
) -> Result<()> {
    print!("{}", render_agent_show_prompt(prompt, prompt_args));
    Ok(())
}

pub(crate) fn render_agent_show_prompt(
    prompt: &AgentPromptOptions,
    prompt_args: &[String],
) -> String {
    let mut out = String::new();
    out.push_str("# AIDA agent launch preview — initial prompt only; no process was started.\n");
    out.push_str(&format!(
        "source: {}\n\n",
        prompt_source_label(prompt, prompt_args)
    ));
    match prompt_args.last() {
        Some(text) => {
            out.push_str(text);
            out.push('\n');
        }
        None => out.push_str("(no initial message would be sent)\n"),
    }
    out
}

// trace:BUG-2006 | ai:antigravity
pub(crate) fn build_agent_new_bg_command(
    binary: &std::path::Path,
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    prompt_args: &[String],
    launch_context: Option<&AgentLaunchContext>,
) -> Result<std::process::Command> {
    let mut command = std::process::Command::new(binary);
    command
        .current_dir(&plan.launch_cwd)
        .args(&config.default_args)
        .args(prompt_args)
        .env("AIDA_AGENT_TYPE", config.agent_type)
        .env("AIDA_AGENT_NAME", &plan.name)
        // BUG-558: the spawned agent's mailbox/queue identity (BUG-89 resolves
        // it from AIDA_USER → USER) must be the agent's OWN stable name, not the
        // launching human's shell USER — otherwise the agent reads the human's
        // inbox and a coordinator's `--to <agent>` is invisible. Export
        // AIDA_USER = the stable name so mailbox + queue agree with the
        // registry/brief identity. trace:BUG-558 | ai:claude
        .env("AIDA_USER", &plan.name)
        .env("AIDA_PROJECT_ROOT", &plan.project_root)
        .env_remove("AIDA_SESSION_GRANT")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(role) = &plan.role {
        let grant = seat_authority::issue_child(
            &plan.project_root,
            &canonical_role_name(role),
            &plan.name,
        )?;
        command
            .env("AIDA_SESSION_ROLE", role)
            .env(seat_authority::GRANT_ENV, grant.id);
    } else {
        command.env_remove("AIDA_SESSION_ROLE");
    }
    if let Some(spec) = &plan.current_spec {
        command.env("AIDA_SESSION_SCOPE", spec);
    }
    apply_agent_lease_identity(&mut command, plan.lease_id.as_deref());
    if let Some(ctx) = launch_context {
        command
            .env("AIDA_AGENT_CONTEXT_FILE", &ctx.path)
            .env("AIDA_AGENT_REGISTRY_TOKEN", &ctx.token);
    }
    // TASK-1482: same default strip as the foreground path — a `--bg`
    // dispatch must not silently hand its detached child the operator's
    // local output-format/agent-mode/glyph/quiet preferences either.
    // trace:TASK-1482 | ai:claude
    apply_presentation_env_policy(
        &mut command,
        &resolve_presentation_env_policy(&plan.project_root)?,
    );
    // trace:BUG-2006 | ai:antigravity
    if is_contained_launch(config) {
        command.env("AIDA_CONTAINED", "1");
    } else {
        command.env_remove("AIDA_CONTAINED");
    }
    Ok(command)
}

// trace:BUG-2006 | ai:antigravity
// trace:BUG-2124 | ai:antigravity
pub(crate) fn is_contained_launch(config: &AgentLaunchConfig) -> bool {
    let vendor = match session::HeadlessVendor::parse(config.agent_type) {
        Some(v) => v,
        None => return false,
    };
    session::containment_applied(vendor, config.contained)
}

/// TASK-1467: the `AIDA_*` environment variables `run_tracked_agent` sets on
/// the spawned child, mirrored here read-only for the `--no-exec` preview.
/// Keep in sync with `run_tracked_agent`'s `.env(...)` calls.
pub(crate) fn agent_launch_env_inputs(
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    context_path: Option<&std::path::Path>,
) -> Vec<(&'static str, String)> {
    let mut out = vec![
        ("AIDA_AGENT_TYPE", config.agent_type.to_string()),
        ("AIDA_AGENT_NAME", plan.name.clone()),
        ("AIDA_USER", plan.name.clone()),
        ("AIDA_PROJECT_ROOT", plan.project_root.display().to_string()),
        (
            "AIDA_ROLE_INSTANCE",
            plan.role_instance.as_str().to_string(),
        ),
    ];
    // trace:BUG-2006 | ai:antigravity
    if is_contained_launch(config) {
        out.push(("AIDA_CONTAINED", "1".to_string()));
    }
    if let Some(role) = &plan.role {
        out.push(("AIDA_SESSION_ROLE", role.clone()));
    }
    if let Some(spec) = &plan.current_spec {
        out.push(("AIDA_SESSION_SCOPE", spec.clone()));
    }
    if let Some(path) = context_path {
        out.push(("AIDA_AGENT_CONTEXT_FILE", path.display().to_string()));
        out.push((
            "AIDA_AGENT_REGISTRY_TOKEN",
            "<generated at launch>".to_string(),
        ));
    }
    out
}

/// TASK-1467: repository guidance files the spawned child is expected to
/// read on startup, keyed off vendor (`CLAUDE.md` for claude, `AGENTS.md`
/// for codex/antigravity) plus the universal AIDA discipline pointer.
/// Returns (repo-relative path, exists-on-disk) pairs — existence is
/// read-only `Path::is_file`, never a write.
pub(crate) fn agent_guidance_files(
    agent_type: &str,
    project_root: &std::path::Path,
) -> Vec<(String, bool)> {
    let mut candidates: Vec<&str> = match agent_type {
        "claude" => vec!["CLAUDE.md"],
        "codex" => vec!["AGENTS.md"],
        "antigravity" => vec!["AGENTS.md"],
        _ => vec!["CLAUDE.md", "AGENTS.md"],
    };
    candidates.push(".aida/discipline/README.md");
    candidates
        .into_iter()
        .map(|rel| {
            let present = project_root.join(rel).is_file();
            (rel.to_string(), present)
        })
        .collect()
}

// trace:TASK-1232 | ai:codex
pub(crate) fn resolved_flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().enumerate().find_map(|(idx, arg)| {
        if arg == flag {
            return args.get(idx + 1).map(String::as_str);
        }
        arg.strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
    })
}

// trace:TASK-1232 | ai:codex
pub(crate) fn resolved_agent_permission_summary(agent_type: &str, args: &[String]) -> String {
    match agent_type {
        "claude" => {
            if let Some(mode) = resolved_flag_value(args, "--permission-mode") {
                format!("claude --permission-mode {mode}")
            } else if args.iter().any(|arg| arg == "--settings") {
                "claude contained sandbox settings".to_string()
            } else {
                "claude native permission posture".to_string()
            }
        }
        "codex" => {
            if args
                .iter()
                .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
            {
                "codex approvals and sandbox bypassed".to_string()
            } else {
                let sandbox = resolved_flag_value(args, "--sandbox").unwrap_or("codex default");
                let approval =
                    resolved_flag_value(args, "--ask-for-approval").unwrap_or("codex default");
                format!("codex sandbox={sandbox}, ask-for-approval={approval}")
            }
        }
        "antigravity" => {
            if args
                .iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
            {
                "antigravity permissions skipped".to_string()
            } else {
                "antigravity native permission posture".to_string()
            }
        }
        other => format!("{other} native permission posture"),
    }
}

pub(crate) fn run_tracked_agent(
    binary: &std::path::Path,
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    launch_context: Option<&AgentLaunchContext>,
    prompt_args: &[String],
    description: Option<String>,
    title: bool,
    verbose: bool,
) -> Result<()> {
    // TASK-864: route the INTERACTIVE foreground launch through the same os_wrap
    // (bwrap) boundary the headless paths use. When `[contained] os_wrap` is on
    // (or the TASK-876 `AIDA_OS_WRAP` per-host override is set) and the agent is
    // claude, the resolved binary + all args are wrapped in `bwrap …` with the
    // SAME fail-closed contract (bwrap missing / userns blocked → error with
    // remediation, never a silent unconfined launch). When os_wrap is OFF this
    // returns the bare program + args, so a normal `aida agent new` is
    // byte-identical to today's behavior. Non-claude agents keep the bare path.
    // trace:TASK-864 | ai:claude
    let (program, exec_args) =
        resolved_agent_program_and_args(binary, config, plan, prompt_args, true)?;
    let mut command = std::process::Command::new(&program);
    command
        .current_dir(&plan.launch_cwd)
        .args(&exec_args)
        .env("AIDA_AGENT_TYPE", config.agent_type)
        .env("AIDA_AGENT_NAME", &plan.name)
        // BUG-558: export AIDA_USER = the stable name so the spawned agent's
        // mailbox/queue identity is its own, not the launching human's shell
        // USER (see the bg-dispatch block above). trace:BUG-558 | ai:claude
        .env("AIDA_USER", &plan.name)
        .env("AIDA_PROJECT_ROOT", &plan.project_root)
        .env_remove("AIDA_SESSION_GRANT")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    if let Some(role) = &plan.role {
        let grant = seat_authority::issue_child(
            &plan.project_root,
            &canonical_role_name(role),
            &plan.name,
        )?;
        command
            .env("AIDA_SESSION_ROLE", role)
            .env(seat_authority::GRANT_ENV, grant.id);
    } else {
        command.env_remove("AIDA_SESSION_ROLE");
    }
    command.env("AIDA_ROLE_INSTANCE", plan.role_instance.as_str());
    if let Some(spec) = &plan.current_spec {
        command.env("AIDA_SESSION_SCOPE", spec);
    }
    apply_agent_lease_identity(&mut command, plan.lease_id.as_deref());
    if let Some(ctx) = launch_context {
        command
            .env("AIDA_AGENT_CONTEXT_FILE", &ctx.path)
            .env("AIDA_AGENT_REGISTRY_TOKEN", &ctx.token);
    }
    // TASK-1482: strip operator-local presentation env (AIDA_OUTPUT_FORMAT,
    // AIDA_AGENT_OUTPUT, …) from the child unless `[agents] inherit_env`
    // explicitly opts it back in. `Command` otherwise inherits the full
    // parent env, so this is subtractive on top of that default. Covers the
    // direct foreground launch AND any shell-wrapped (bwrap) launch, since
    // both go through this one `Command` — bwrap execs the wrapped program
    // with this same env unless `--clearenv` is passed, which it isn't.
    // trace:TASK-1482 | ai:claude
    apply_presentation_env_policy(
        &mut command,
        &resolve_presentation_env_policy(&plan.project_root)?,
    );
    // trace:BUG-2006 | ai:antigravity
    if is_contained_launch(config) {
        command.env("AIDA_CONTAINED", "1");
    } else {
        command.env_remove("AIDA_CONTAINED");
    }

    let mut child = command
        .spawn_retrying_etxtbsy()
        .with_context(|| format!("failed to spawn {}", binary.display()))?;
    let child_pid = child.id();
    let terminal = agent_registry::current_terminal_identity();
    let title_enabled = title && agent_registry::terminal_title_enabled(&plan.project_root);
    let mut title_restore = None;
    if title_enabled {
        let session_id = plan
            .native_session_id
            .as_deref()
            .map(str::to_string)
            .unwrap_or_else(|| child_pid.to_string());
        let title = agent_registry::launch_title(
            plan.role.as_deref().unwrap_or(config.agent_type),
            plan.current_spec.as_deref(),
            &session_id,
        );
        title_restore = Some(agent_registry::apply_terminal_title(
            &title,
            terminal.as_ref(),
        ));
    }
    let binary = agent_registry::AgentBinaryIdentity::new(
        env!("CARGO_PKG_VERSION").to_string(),
        env!("AIDA_BUILD_GIT_SHA").to_string(),
    );
    let description = match (plan.role_instance, description) {
        (RoleInstanceKind::Companion, Some(text)) => Some(format!("companion: {text}")),
        (RoleInstanceKind::Companion, None) => Some(
            "companion: read/converse/draft only; no disposition or drain authority".to_string(),
        ),
        (RoleInstanceKind::Driver, text) => text,
    };
    let registry_entry = agent_registry::register_spawned_agent(
        &plan.project_root,
        config.agent_type,
        child_pid,
        plan.role.clone(),
        plan.current_spec.clone(),
        plan.launch_cwd.clone(),
        Some(&binary),
        Some(plan.name.clone()),
        plan.native_session_id.clone(),
        plan.resumed_from.clone(),
        description,
    )?;
    // TASK-1498: `--verbose` process-registration diagnostics — confirms the
    // spawned pid and the registry entry id it was recorded under, right
    // after the write that makes it visible to `aida agent ls`.
    // trace:TASK-1498 | ai:claude
    if verbose {
        eprintln!(
            "  {}: spawned pid {} registered as {}",
            "diagnostics".bold(),
            child_pid,
            registry_entry.id
        );
    }
    let signal_forwarder = install_child_signal_forwarder(child_pid)?;
    let status = child
        .wait()
        .with_context(|| format!("failed to wait for {}", config.agent_type))?;
    signal_forwarder.stop();
    // TASK-1498: `--verbose` child exit/result diagnostics — reported clearly
    // without changing launch semantics (the exit-code propagation below is
    // unchanged). trace:TASK-1498 | ai:claude
    if verbose {
        eprintln!(
            "  {}: pid {} exited: success={} code={}",
            "diagnostics".bold(),
            child_pid,
            status.success(),
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string())
        );
    }
    if let Some(restore) = title_restore {
        agent_registry::restore_terminal_title(restore);
    }
    if let Err(err) =
        agent_registry::mark_agent_ended(&plan.project_root, config.agent_type, child_pid)
    {
        eprintln!(
            "{} failed to mark agent registry entry ended for {}#{}: {err}",
            "warning:".yellow().bold(),
            config.agent_type,
            child_pid
        );
    }
    if let Some(ctx) = launch_context {
        if let Err(err) = std::fs::remove_file(&ctx.path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "{} failed to remove agent context file {}: {err}",
                    "warning:".yellow().bold(),
                    ctx.path.display()
                );
            }
        }
    }
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

// trace:BUG-2002 | ai:codex
#[cfg(test)]
#[path = "tests/bug_2002_launch_lease_tests.rs"]
mod bug_2002_launch_lease_tests;
