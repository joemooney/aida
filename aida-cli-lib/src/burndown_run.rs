//! burndown command, burndown run waves, wave PR integration and burndown config (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Install a per-invocation `--order` override by exporting
/// `AIDA_BURNDOWN_ORDER`, the top tier of [`resolved_burndown_ready_order`].
/// Exporting (rather than threading a parameter) means the headless drain
/// `burndown run` launches — and every `burndown plan` that drain re-resolves
/// through — inherits the operator's one-run choice for free. An unrecognized
/// token is a hard error rather than a silent fallback: the operator asked for a
/// specific order, so guessing would be worse than refusing.
// trace:TASK-1175 | ai:claude
pub(crate) fn install_burndown_order_override(raw: Option<&String>) -> Result<()> {
    let Some(raw) = raw else { return Ok(()) };
    let order = burndown::parse_ready_order(raw).ok_or_else(|| {
        anyhow::anyhow!("unknown --order value `{raw}` (expected: priority, queue)")
    })?;
    std::env::set_var(
        "AIDA_BURNDOWN_ORDER",
        match order {
            burndown::ReadyOrder::Priority => "priority",
            burndown::ReadyOrder::Queue => "queue",
        },
    );
    Ok(())
}

pub(crate) fn handle_burndown_command(cmd: &crate::cli::BurndownCommand) -> Result<()> {
    match cmd {
        crate::cli::BurndownCommand::Plan {
            status,
            tag,
            batch,
            r#type,
            candidates,
            order,
            json,
        } => {
            install_burndown_order_override(order.as_ref())?;
            handle_burndown_plan(
                status,
                tag.as_deref(),
                batch.as_deref(),
                r#type.as_deref(),
                *candidates,
                *json,
            )
        }
        crate::cli::BurndownCommand::Explain { json } => handle_burndown_explain(*json),
        crate::cli::BurndownCommand::Readiness {
            hours,
            lanes,
            specs,
            json,
        } => machine_readiness::run_command(*hours, *lanes, *specs, *json),
        crate::cli::BurndownCommand::Run {
            status,
            tag,
            batch,
            max,
            concurrency,
            order,
            permission_mode,
            dry_run,
            verbose,
            quiet,
            force,
            vendor,
            panes,
            require_head,
        } => {
            install_burndown_order_override(order.as_ref())?;
            handle_burndown_run(
                status,
                tag.as_deref(),
                batch.as_deref(),
                *max,
                *concurrency,
                permission_mode.as_deref(),
                *dry_run,
                *verbose,
                *quiet,
                *force,
                vendor.as_deref(),
                panes.as_deref(),
                *require_head,
            )
        }
        crate::cli::BurndownCommand::Status { json } => handle_burndown_status(*json),
    }
}

/// STORY-545: build the `/aida-burndown` slash-command string the headless
/// session runs, from the selector + caps. Pure + unit-testable. The skill
/// reads these args ($ARGUMENTS): `--status`, `--tag`, `--batch`, `--max`,
/// `--concurrency`. trace:STORY-545 | ai:claude
pub(crate) fn burndown_skill_prompt(
    status: &str,
    tag: Option<&str>,
    batch: Option<&str>,
    max: Option<usize>,
    concurrency: Option<usize>,
) -> String {
    let mut s = format!("/aida-burndown --status {status}");
    if let Some(t) = tag {
        s.push_str(&format!(" --tag {t}"));
    }
    if let Some(b) = batch {
        s.push_str(&format!(" --batch {b}"));
    }
    if let Some(m) = max {
        s.push_str(&format!(" --max {m}"));
    }
    if let Some(c) = concurrency {
        s.push_str(&format!(" --concurrency {c}"));
    }
    s
}

/// TASK-1159: resolve whether `aida burndown run` uses the live stream-json
/// launch. Precedence is explicit `--verbose`, then explicit `--quiet`, then
/// `[burndown] verbose` config (project before global), then the built-in quiet
/// default. Pure + unit-tested so script-facing defaults stay pinned.
// trace:TASK-1159 | ai:codex
pub(crate) fn resolve_burndown_verbose(
    verbose_flag: bool,
    quiet_flag: bool,
    project_config: Option<bool>,
    global_config: Option<bool>,
) -> bool {
    if verbose_flag {
        true
    } else if quiet_flag {
        false
    } else {
        project_config.or(global_config).unwrap_or(false)
    }
}

// trace:TASK-1159 | ai:codex
pub(crate) fn read_burndown_verbose_from_config_path(path: &std::path::Path) -> Option<bool> {
    let content = std::fs::read_to_string(path).ok()?;
    let value = toml::from_str::<toml::Value>(&content).ok()?;
    value
        .get("burndown")
        .and_then(|section| section.get("verbose"))
        .and_then(|v| v.as_bool())
}

// trace:TASK-1159 | ai:codex
pub(crate) fn read_burndown_verbose_config(
    project_root: &std::path::Path,
) -> (Option<bool>, Option<bool>) {
    let project =
        read_burndown_verbose_from_config_path(&project_root.join(".aida").join("config.toml"));
    let global = aida_home_dir()
        .and_then(|home| read_burndown_verbose_from_config_path(&home.join(".aida/config.toml")));
    (project, global)
}

/// TASK-1172: read `[burndown] order` from one config path. An unreadable file,
/// a missing key, or an unrecognized value all yield `None` so the caller falls
/// through to the next source (and ultimately the default).
// trace:TASK-1172 | ai:claude
pub(crate) fn read_burndown_order_from_config_path(
    path: &std::path::Path,
) -> Option<burndown::ReadyOrder> {
    let content = std::fs::read_to_string(path).ok()?;
    let value = toml::from_str::<toml::Value>(&content).ok()?;
    value
        .get("burndown")
        .and_then(|section| section.get("order"))
        .and_then(|v| v.as_str())
        .and_then(burndown::parse_ready_order)
}

/// TASK-1172 / TASK-1175: how the drain orders its ready set. `AIDA_BURNDOWN_ORDER`
/// env (what the `--order` flag exports, so a headless child drain inherits the
/// operator's one-run choice) → project config → machine-global config → the
/// `priority` default. Never fails: a missing or malformed value at any tier
/// simply falls through to the next one.
// trace:TASK-1172 | ai:claude
// trace:TASK-1175 | ai:claude
pub(crate) fn resolved_burndown_ready_order(
    project_root: &std::path::Path,
) -> burndown::ReadyOrder {
    std::env::var("AIDA_BURNDOWN_ORDER")
        .ok()
        .and_then(|raw| burndown::parse_ready_order(&raw))
        .or_else(|| {
            read_burndown_order_from_config_path(&project_root.join(".aida").join("config.toml"))
        })
        .or_else(|| {
            aida_home_dir().and_then(|home| {
                read_burndown_order_from_config_path(&home.join(".aida/config.toml"))
            })
        })
        .unwrap_or_default()
}

/// TASK-1169 / ADR-22: read `[burndown] bg_wait_ceiling_ms` from one config
/// path. Project config wins over the machine-global one; both are optional.
// trace:TASK-1169 | ai:claude
pub(crate) fn read_bg_wait_ceiling_from_config_path(path: &std::path::Path) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    let value = toml::from_str::<toml::Value>(&content).ok()?;
    value
        .get("burndown")
        .and_then(|section| section.get("bg_wait_ceiling_ms"))
        .and_then(|v| v.as_integer())
        .and_then(|n| u64::try_from(n).ok())
}

/// TASK-1169 / ADR-22: the bounded background-wait ceiling (ms) AIDA sets on
/// every headless child it spawns. Env override → project config → global
/// config → default, clamped to a non-zero floor so wait-forever (the retired
/// `=0` stopgap) is unreachable. Never fails: an unreadable config or a
/// unresolvable project root falls through to the default.
// trace:TASK-1169 | ai:claude
pub(crate) fn resolved_bg_wait_ceiling_ms(project_root: Option<&std::path::Path>) -> u64 {
    let configured = project_root
        .and_then(|root| {
            read_bg_wait_ceiling_from_config_path(&root.join(".aida").join("config.toml"))
        })
        .or_else(|| {
            aida_home_dir().and_then(|home| {
                read_bg_wait_ceiling_from_config_path(&home.join(".aida/config.toml"))
            })
        });
    let override_env = std::env::var(burndown::BG_WAIT_CEILING_OVERRIDE_ENV).ok();
    burndown::resolve_bg_wait_ceiling_ms(override_env.as_deref(), configured)
}

/// TASK-1169 / ADR-22: the `(key, value)` a headless spawn site sets so the
/// child's turn-end background-wait ceiling is AIDA's bounded value rather than
/// whatever the ambient shell happened to carry. Applied CENTRALLY (every
/// headless spawn in `session.rs`) and at both burndown launch sites, so a
/// hand-typed `aida burndown run` and a daemonized one are identical — the
/// divergence that produced the 2026-07-18 limp incident.
// trace:TASK-1169 | ai:claude
pub(crate) fn bg_wait_ceiling_env(
    project_root: Option<&std::path::Path>,
) -> (&'static str, String) {
    (
        burndown::BG_WAIT_CEILING_ENV,
        resolved_bg_wait_ceiling_ms(project_root).to_string(),
    )
}

#[cfg(test)]
#[path = "tests/burndown_run_tests.rs"]
mod burndown_run_tests;

#[cfg(test)]
#[path = "tests/task1169_integration_wait_tests.rs"]
mod task1169_integration_wait_tests;

/// STORY-545: `aida burndown run` — kick off and walk away. Preflights the
/// advisor-blessed ready set (the same gate `plan` shows), then launches a
/// headless `claude -p "/aida-burndown <selector>"` that fans out
/// worktree-isolated implementers, integrates PRs, and loops until drained.
/// Strategy B (SPIKE-51): reuse the proven skill fan-out, don't reimplement it.
/// trace:STORY-545 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_burndown_run(
    status: &str,
    tag: Option<&str>,
    batch: Option<&str>,
    max: Option<usize>,
    concurrency: Option<usize>,
    permission_mode: Option<&str>,
    dry_run: bool,
    // TASK-804 (facet a of STORY-604): stream live per-event progress. When set,
    // the headless drain is launched with stream-json flags and its events are
    // teed to `.aida/burndown/<drain-id>.jsonl` + rendered live. Control flow is
    // identical to the quiet launch — visibility only. trace:TASK-804 | ai:claude
    verbose: bool,
    // TASK-1159: explicit negation for a configured verbose default.
    quiet: bool,
    // STORY-647/STORY-1353: bypass the dispatch-authority drain-start guardrail.
    force: bool,
    // TASK-1116: per-invocation `--vendor`/`--agent`. burndown's implementer
    // fan-out is the Claude harness's native subagent primitive (Claude-only),
    // so this exports `AIDA_HEADLESS_VENDOR` for the drain — any vendor-agnostic
    // per-spec orchestration inherits it — but does NOT reroute the native
    // fan-out. trace:TASK-1116 | ai:claude
    vendor: Option<&str>,
    // TASK-1120: opt-in pane hosting exported to the drain — any
    // `aida queue work --auto-complete` it spawns then hosts its implementer in
    // a titled tmux window. Faithful-launcher: `None` leaves the env untouched.
    panes: Option<&str>,
    // STORY-1414: refuse (not just warn) when the dev binary is stale.
    require_head: bool,
) -> Result<()> {
    // TASK-1120: export the requested pane host so the headless drain (and any
    // per-spec auto-complete orchestration under it) inherits it. Absent → env
    // untouched → byte-identical background spawn.
    if let Some(host) = panes {
        std::env::set_var(pane_host::HOST_ENV, host);
    }
    // STORY-1133: a companion is a NON-AUTHORITATIVE instance of a driver role —
    // it may read / converse / draft / advise, but must never drive a drain.
    // Refuse BEFORE the team gate (and before any sync/preflight), mirroring the
    // `aida queue work --auto-complete` companion gate, so even a companion that
    // otherwise carries advisor authority cannot start (or gate-probe) a drain
    // via `burndown run`. `--force` does NOT bypass this — the driver seat does.
    // trace:STORY-1133 | ai:claude
    if current_role_instance_is_companion() {
        anyhow::bail!(
            "companion sessions cannot drive drains. Start the authoritative driver \
             seat for this role, or run `aida burndown run` from the driver."
        );
    }
    // Starting a wave is dispatch, not disposition. `--force` remains the
    // audited guardrail escape hatch. trace:STORY-647 trace:STORY-1353 | ai:codex
    if !force && !has_dispatch_authority() {
        anyhow::bail!(
            "starting an autonomous drain needs dispatch authority (product, advisor, or \
             integrator role, or a live orchestrator)"
        );
    }

    // TASK-1116: install the per-invocation headless-vendor override (top
    // precedence) and export `AIDA_HEADLESS_VENDOR` so the drain inherits it.
    // burndown's fan-out is Claude-harness-only, so a non-Claude vendor is
    // called out loudly and pointed at the vendor-agnostic per-spec path — no
    // silent misroute. An unrecognized token is a hard error. trace:TASK-1116
    if let Some(raw) = vendor {
        let resolved = session::install_headless_vendor_override(raw).ok_or_else(|| {
            anyhow::anyhow!("unknown --vendor/--agent value `{raw}` (expected: claude, codex, agy)")
        })?;
        if resolved != session::HeadlessVendor::Claude {
            eprintln!(
                "  {} burndown's implementer fan-out is Claude-harness-only; `--vendor {}` \
                 sets AIDA_HEADLESS_VENDOR for the drain but the native subagent fan-out stays \
                 on Claude. For a fully {}-driven drain run `aida queue work <SPEC> \
                 --auto-complete --vendor {}` per spec.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                resolved.as_str(),
                resolved.as_str(),
                resolved.as_str(),
            );
        }
    }

    // BUG-530: sync the orphan store before computing the blessed set. The
    // preflight reads the local `.aida-store/` worktree (via
    // resolve_burndown_sets → load_store_for_lookup → GitBackend), which can
    // LAG origin when a prior drain pushed Done→Completed bumps that this clone
    // hasn't pulled — so the preflight would otherwise announce stale specs
    // (e.g. already-Completed/unqueued ones). maybe_sync_pull is the proven
    // `--sync` primitive: fetch + ff/rebase, and it NEVER fails the caller (every
    // error → a warning), so a degraded/offline sync just falls back to the
    // local view. Runs on dry-run too, so the previewed set matches the real
    // drain. trace:BUG-530 | ai:claude
    if let Ok(project_root) = find_project_root() {
        if let Some(store_path) = detect_distributed_store_from(&project_root) {
            let _ = maybe_sync_pull(&store_path);
        }
    }

    // trace:TASK-149 — serialize-held specs are queued + blessed and drain in a
    // later wave; the runner never acts on them THIS wave, so they are not part
    // of the "nothing blessed" report below.
    let (ready, awaiting_signoff, _serialize_held, parked, _supervised, _titles) =
        resolve_burndown_sets(status, tag, batch, None)?;

    println!(
        "{} burndown run",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    println!(
        "  {}",
        burndown::selector_summary(status, tag, batch).dimmed()
    );

    // Nothing blessed → don't launch a session for no work.
    if ready.is_empty() {
        println!(
            "  {} 0 blessed specs to drain ({} pickable awaiting sign-off, {} parked).",
            "→".green(),
            awaiting_signoff.len(),
            parked.len()
        );
        if !awaiting_signoff.is_empty() {
            println!(
                "\n{} Pickable but not queued — bless with `aida queue add <id>`, then re-run:",
                "·".dimmed()
            );
            for id in &awaiting_signoff {
                println!("  {} {}", "+".yellow(), id);
            }
        }
        return Ok(());
    }

    println!(
        "  {} {} blessed spec(s) will drain: {}",
        "→".green(),
        ready.len(),
        ready.join(", ").cyan()
    );

    // TASK-1298: an unattended run must prove the host can sustain the work,
    // not merely that the spec substrate is healthy. Keep dry-run observational;
    // the real launch refuses on hard machine-readiness failures.
    // trace:TASK-1298 | ai:codex
    if !dry_run {
        let lanes = concurrency.unwrap_or(4).max(1);
        let specs = max.unwrap_or(ready.len()).min(ready.len()).max(1);
        let readiness_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        let report = machine_readiness::probe(&readiness_root, 20, lanes, specs);
        machine_readiness::print_human(&report);
        if !report.ready() {
            anyhow::bail!(
                "machine readiness failed — apply the remedies above, then re-run `aida burndown readiness --hours 20 --lanes {lanes} --specs {specs}`"
            );
        }
    }

    // The skill re-resolves the ready set authoritatively (same gate), so the
    // preflight list is what WILL drain. Build the headless invocation.
    let prompt = burndown_skill_prompt(status, tag, batch, max, concurrency);
    let mode = permission_mode.unwrap_or("bypassPermissions");
    let (project_verbose, global_verbose) = find_project_root()
        .ok()
        .map(|root| read_burndown_verbose_config(&root))
        .unwrap_or((None, None));
    let stream_verbose = resolve_burndown_verbose(verbose, quiet, project_verbose, global_verbose);

    if dry_run {
        println!("\n{} dry run — not launching. Would run:", "·".dimmed());
        if stream_verbose {
            // Mirror the quiet path's `{:?}` quoting of the prompt positional so
            // the previewed command is copy-paste-safe.
            println!(
                "  claude -p {:?} --permission-mode {} {}",
                prompt,
                mode.cyan(),
                "--output-format stream-json --verbose --include-partial-messages".dimmed()
            );
            println!(
                "  {} live progress would stream to {}",
                "→".dimmed(),
                ".aida/burndown/<drain-id>.jsonl".cyan()
            );
        } else {
            println!("  claude -p {:?} --permission-mode {}", prompt, mode.cyan());
        }
        return Ok(());
    }

    // BUG-538: take the global drain lock before launching. A second drain
    // (another `burndown run`, or a `queue work --auto-complete`) against the
    // same tree would double-drive it — two integrators racing on main. The
    // guard's Drop frees the lock on the Ok path below; the non-success
    // `std::process::exit` path drops it explicitly. trace:BUG-538 | ai:claude
    let drain_lock_command = format!(
        "burndown run ({})",
        burndown::selector_summary(status, tag, batch)
    );
    let project_root = find_main_worktree_root()?;
    // STORY-1414: a wave pins its launching binary — warn about a stale dev
    // build (refuse under --require-head) before the lock is taken.
    // trace:STORY-1414 | ai:claude
    crate::freshness_gate::enforce_wave_launch_gate(&project_root, require_head)?;
    // BUG-759: record the blessed spec set in the lock so `aida drain status`
    // can name what this launcher-held drain is working (pid + started +
    // specs) for its entire wall-clock — the launcher writes no per-phase
    // drain-state file. The guard is held across the whole resume loop below,
    // so lock lifetime == `claude -p` child lifetime. trace:BUG-759 | ai:claude
    let _drain_guard =
        drain_lock::acquire_drain_lock_with_specs(&project_root, &drain_lock_command, &ready)?;

    // BUG-660: keep the host awake for the unattended headless drain. Best-effort
    // + pid-scoped (auto-releases when this process exits). trace:BUG-660
    let _sleep_inhibitor = {
        let inhibitor = drive_robustness::SleepInhibitor::for_drive("aida burndown run");
        if let Some(tool) = inhibitor.tool() {
            eprintln!(
                "  {} sleep-prevention active for this drain (via {})",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                tool
            );
        }
        inhibitor
    };

    // BUG-607: a drain-state.json left behind by a KILLED or crashed
    // `queue work --auto-complete` drain (its orchestrator pid is dead) must not
    // make the spawned `/aida-burndown` agent falsely believe a live drain
    // already owns these specs and refuse to fan out. The lock we just acquired
    // already guarantees exclusivity (BUG-538), so a dead-pid drain-state is a
    // pure stale tombstone — clear it before launching. trace:BUG-607 | ai:claude
    if let status @ (crate::drain_state::DrainStatus::Stale(_)
    | crate::drain_state::DrainStatus::Stopped(_)) = crate::drain_state::probe(&project_root)
    {
        let _ = crate::drain_state::DrainState::clear(&project_root);
        let stopped = matches!(status, crate::drain_state::DrainStatus::Stopped(_));
        println!(
            "  {} cleared a {} drain-state (its orchestrator is no longer running)",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            if stopped { "stopped" } else { "stale" }
        );
    }

    // The default posture is faithful to the operator's walk-away intent: a
    // headless drain that pushes/merges/fans-out can't stall on prompts, so
    // default to bypassed permissions — but say so loudly and let `--permission-mode`
    // override. trace:STORY-545
    if permission_mode.is_none() {
        println!(
            "  {} launching headless `claude -p` with {} permissions (override: --permission-mode <mode>)",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            "BYPASSED".yellow().bold()
        );
    } else {
        println!(
            "  {} launching headless `claude -p` with permission mode `{}`",
            "→".green(),
            mode
        );
    }
    println!(
        "  {} {}",
        "→".green(),
        format!("claude -p {prompt:?}").dimmed()
    );
    println!();

    // BUG-755: the launcher — the long-lived process holding the drain lock —
    // must not exit while wave PRs are unmerged or blessed specs are unstarted.
    // A headless `claude -p` terminates at turn end, so any "background merge
    // watch" the session promises dies with it (observed 2026-07-18: three PRs
    // stranded "awaiting CI", four blessed specs never started). After each
    // session exit we probe the residual work; a clean exit that left residual
    // is a premature turn end → relaunch a bounded continuation turn. A
    // deliberate stop (the shelve/cap non-zero convention) or a spent resume
    // budget exits WITH an accurate handoff — never a claim of live watches.
    // trace:BUG-755 | ai:claude
    const MAX_RESUME_ROUNDS: usize = 3;
    let base_prompt = prompt.clone();
    let mut current_prompt = prompt.clone();
    let mut rounds_used = 0usize;
    // TASK-1169 / ADR-22: resolved ONCE for the whole drain so every relaunch
    // round carries the identical ceiling. trace:TASK-1169 | ai:claude
    let (ceiling_key, ceiling_value) = bg_wait_ceiling_env(Some(&project_root));
    println!(
        "  {} background-wait ceiling for this drain: {}m (override: {})",
        "→".green(),
        ceiling_value.parse::<u64>().unwrap_or_default() / 60_000,
        burndown::BG_WAIT_CEILING_OVERRIDE_ENV.dimmed(),
    );
    loop {
        // Propagate the drain's exit code either way so scripts can branch on
        // success/parked (the skill exits non-zero when it shelves work,
        // mirroring the orchestrator drain's exit-2 convention). `claude` is
        // resolved off PATH (matching every other launch site); a missing
        // binary surfaces as a guided error, not a raw ENOENT.
        let status_code = if stream_verbose {
            // TASK-804: stream-json + tee path. Redirect the drain's JSONL to a
            // discoverable log and render a live human progress line per event.
            run_burndown_verbose(&current_prompt, mode)?
        } else {
            build_burndown_quiet_command(&current_prompt, mode, ceiling_key, &ceiling_value)
                .status()
                .map_err(|e| {
                    anyhow::anyhow!(
                        "failed to launch `claude -p` ({e}) — the headless drain needs the Claude \
                     Code CLI on PATH. Install it, or use `aida queue work --auto-complete` \
                     (the orchestrator drain) instead."
                    )
                })?
        };

        let code = status_code.code().unwrap_or(1);
        // A `--max` cap makes "blessed specs remain unstarted" an EXPECTED stop,
        // so the unstarted probe is skipped; open wave PRs must still drain.
        let mut residual =
            probe_burndown_residual(&project_root, status, tag, batch, &ready, max.is_some());
        // TASK-1169 / ADR-21: the launcher integrates the open wave PRs ITSELF
        // — a Rust CI wait with no harness ceiling and no 10-minute tool cap —
        // BEFORE deciding whether another agent turn is even needed. Run on a
        // deliberate non-zero stop too: one spec shelving is no reason to
        // strand another spec's green PR. The residual is then RE-PROBED so the
        // follow-up decision sees the post-integration world (merged PRs gone,
        // parked specs excluded). trace:TASK-1169 | ai:claude
        if !residual.unmerged_prs.is_empty() {
            let rows = integrate_wave_prs(&project_root, &residual);
            if !rows.is_empty() {
                residual = probe_burndown_residual(
                    &project_root,
                    status,
                    tag,
                    batch,
                    &ready,
                    max.is_some(),
                );
            }
        }
        match burndown::drain_followup(code, residual.is_empty(), rounds_used, MAX_RESUME_ROUNDS) {
            burndown::DrainFollowup::Complete => {
                if code == 0 {
                    return Ok(());
                }
                // `std::process::exit` skips destructors — free the drain lock
                // explicitly so a non-zero (e.g. shelved-work, exit-2) drain
                // still leaves a clean lock for the next launch. trace:BUG-538
                drop(_drain_guard);
                std::process::exit(code);
            }
            burndown::DrainFollowup::Resume => {
                rounds_used += 1;
                println!(
                    "  {} the drain session ended its turn with unfinished work \
                     ({} open wave PR(s), {} unstarted blessed spec(s)) — relaunching a \
                     continuation turn ({rounds_used}/{MAX_RESUME_ROUNDS})",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    residual.unmerged_prs.len(),
                    residual.unstarted.len(),
                );
                current_prompt = burndown::continuation_prompt(&base_prompt, &residual);
            }
            burndown::DrainFollowup::Handoff => {
                print!("{}", burndown::render_drain_handoff(&residual));
                drop(_drain_guard);
                // Preserve a deliberate non-zero stop's code; a spent resume
                // budget surfaces as the shelved-work exit-2 convention so
                // scripts triage rather than read success.
                std::process::exit(if code != 0 { code } else { 2 });
            }
        }
    }
}

/// BUG-755: probe the residual work a drain session left behind when its
/// headless process exited — the impure half feeding the pure
/// [`burndown::drain_followup`] decision. Re-syncs the store (so the drain's
/// Done→Completed merge bumps are visible), re-resolves the blessed ready set
/// (the unstarted remainder — skipped when `--max` capped the run, where a
/// remainder is an expected stop), and matches the forge's open PRs against
/// the drain's scope (initial blessed set ∪ still-ready set). Every leg is
/// best-effort: a failed probe degrades to "no residual" rather than wedging
/// the launcher in a relaunch loop on bad data.
// trace:BUG-755 | ai:claude
pub(crate) fn probe_burndown_residual(
    project_root: &std::path::Path,
    status: &str,
    tag: Option<&str>,
    batch: Option<&str>,
    blessed: &[String],
    spec_capped: bool,
) -> burndown::ResidualWork {
    // Refresh the store view first — the drain's merges push Done→Completed
    // bumps this clone may not have pulled (same reason as the BUG-530 preflight
    // sync; never fails the caller).
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        let _ = maybe_sync_pull(&store_path);
    }
    let unstarted: Vec<String> = if spec_capped {
        Vec::new()
    } else {
        resolve_burndown_sets(status, tag, batch, None)
            .map(|(ready, ..)| ready)
            .unwrap_or_default()
    };
    // The drain's scope: what it set out to drain plus what is still blessed
    // (a blocker clearing mid-drain can add specs).
    let mut scope: Vec<String> = blessed.to_vec();
    for id in &unstarted {
        if !scope.contains(id) {
            scope.push(id.clone());
        }
    }
    // Per-spec store facts so a deliberately-held (`review:draft-only`) or
    // shelved (NeedsAttention) spec's open PR is NOT treated as stranded.
    let mut facts: std::collections::HashMap<String, burndown::WaveSpecFacts> =
        std::collections::HashMap::new();
    if let Some(store) = load_store_for_lookup(project_root) {
        let norm = |s: &str| -> String {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase()
        };
        for req in &store.requirements {
            let disp = req
                .agreed_id
                .clone()
                .or_else(|| req.spec_id.clone())
                .unwrap_or_else(|| req.id.to_string());
            if scope.contains(&disp) {
                facts.insert(
                    disp,
                    burndown::WaveSpecFacts {
                        status_norm: norm(&req.status.to_string()),
                        tags: req.tags.iter().cloned().collect(),
                        // TASK-1169: a non-`drain` execution_mode holds the
                        // merge for a human (BUG-727) — deliberate, not
                        // stranded. trace:TASK-1169 | ai:claude
                        supervised: pr_ship::merge_requires_supervision(req.execution_mode),
                    },
                );
            }
        }
    }
    // Fresh (uncached) forge snapshot — the memoized `collect_open_prs` would
    // replay the pre-drain state and mask just-merged PRs.
    let open: Vec<(u64, String, String)> = collect_open_prs_uncached(project_root)
        .by_branch
        .into_values()
        .map(|pr| (pr.number, pr.title, pr.head_branch))
        .collect();
    burndown::ResidualWork {
        unmerged_prs: burndown::match_wave_prs(&scope, &open, &facts),
        unstarted,
    }
}

/// TASK-1169 / ADR-21: the launcher's OWN integration leg — the impure half of
/// the ratified design's primary item.
///
/// For every open wave PR the drain session left behind, this blocks IN RUST on
/// [`wait_for_ci_terminal`] (TASK-968's re-arming idle timer + absolute
/// ceiling), then decides via the pure [`burndown::wave_pr_action`] gate:
/// squash-merge the clean ones (+ `aida pull` for the Done→Completed bump),
/// HOLD the supervised ones (BUG-727), and PARK the rest `NeedsAttention` with
/// a recorded reason + a finding — never terminate, never close a PR.
///
/// Why the launcher and not the agent: a headless turn reaps its background
/// tasks at turn end and its foreground tool calls are capped at ~10 minutes,
/// so neither agent-side shape can hold a 30-minute cross-platform CI run. This
/// process has no such bound, costs no tokens while it waits, and already holds
/// the drain lock.
///
/// Every leg is best-effort per PR: one PR's probe/merge failure parks that PR
/// and moves to the next (EPIC-28 punt-and-continue), never aborting the drain.
// trace:TASK-1169 | ai:claude
pub(crate) fn integrate_wave_prs(
    project_root: &std::path::Path,
    residual: &burndown::ResidualWork,
) -> Vec<burndown::IntegrationOutcome> {
    let mut rows: Vec<burndown::IntegrationOutcome> = Vec::new();
    if residual.unmerged_prs.is_empty() {
        return rows;
    }
    println!(
        "\n  {} integrating {} open wave PR(s) in the launcher — CI waits run here, \
         not in an agent turn",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        residual.unmerged_prs.len(),
    );

    for pr in &residual.unmerged_prs {
        println!(
            "  {} PR-{} ({}) — waiting for CI to reach a terminal state",
            crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
            pr.number,
            pr.spec.cyan(),
        );

        // BUG-727: a spec whose execution_mode is anything but `drain` (or is
        // unset — fail safe) must never be auto-merged. Checked FIRST so a
        // supervised PR doesn't even consume a CI wait.
        let supervision_label = wave_pr_supervision_label(project_root, &pr.spec);

        // The bounded foreground wait. `NoSignal` carrying a bound message is
        // the "wait expired" case ADR-22 requires us to PARK, not abort.
        let (ci, wait_expired) = if supervision_label.is_some() {
            (integrate::CiState::None, None)
        } else {
            match wait_for_ci_terminal(Some(project_root), &pr.branch) {
                CiProbe::Green { .. } => (integrate::CiState::Passing, None),
                CiProbe::Red { failed_summary, .. } => {
                    // A red verdict is not an expired wait — it belongs in the
                    // gate as `Failing`, with the detail surfaced in the log.
                    eprintln!("    {} {}", "·".dimmed(), failed_summary.dimmed());
                    (integrate::CiState::Failing, None)
                }
                CiProbe::PrNoChecks { .. } => (integrate::CiState::None, None),
                CiProbe::InProgress { .. } => (
                    integrate::CiState::Running,
                    Some("CI was still in progress when the wait returned".to_string()),
                ),
                CiProbe::NoSignal(reason) => {
                    // A bound (idle-stall / absolute ceiling) is a park; any
                    // other no-signal (no gh, no PR, network blip) is not — it
                    // degrades to "no CI signal" and the merge gate decides.
                    if reason.contains("ceiling") || reason.contains("idle") {
                        (integrate::CiState::Running, Some(reason))
                    } else {
                        eprintln!("    {} no CI signal: {}", "·".dimmed(), reason.dimmed());
                        (integrate::CiState::None, None)
                    }
                }
            }
        };

        // Review + mergeability, from the forge and the local verdict file —
        // either RequestChanges is a hard stop (never merge over a reviewer).
        let (forge_request_changes, mergeable, forge_head) =
            wave_pr_review_facts(project_root, pr.number);
        let local_request_changes =
            local_verdict_blocks_merge(project_root, pr.number, &pr.spec, forge_head.as_deref());

        let action = burndown::wave_pr_action(&burndown::WavePrFacts {
            supervision_label,
            wait_expired,
            ci,
            request_changes: forge_request_changes || local_request_changes,
            mergeable,
        });

        let succeeded = match &action {
            burndown::WavePrAction::Merge => merge_wave_pr(project_root, pr),
            burndown::WavePrAction::Hold(why) => {
                println!(
                    "  {} PR-{} held — {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    pr.number,
                    why
                );
                true
            }
            burndown::WavePrAction::Park(why) => {
                park_wave_pr(project_root, pr, why);
                true
            }
        };
        // A merge that was gated clean but failed at the forge is itself a
        // park — the work must never be left un-recorded.
        if matches!(action, burndown::WavePrAction::Merge) && !succeeded {
            park_wave_pr(
                project_root,
                pr,
                "the squash-merge call failed at the forge — PR left open for triage",
            );
        }
        rows.push(burndown::IntegrationOutcome {
            pr: pr.number,
            spec: pr.spec.clone(),
            action,
            succeeded,
        });
    }
    print!("{}", burndown::render_integration_report(&rows));
    rows
}

/// TASK-1169: the BUG-727 supervised-merge gate for one wave spec — `Some(label)`
/// when its `execution_mode` makes the merge supervised (anything but `drain`,
/// or unset).
///
/// BUG-1163: this gate FAILS CLOSED. When the spec's mode cannot be resolved —
/// the store is unreadable (e.g. a concurrent de-risk/groom store write, the
/// failure mode that leaked BUG-1156/STORY-1130/TASK-1233 to merge) or the spec
/// is absent — we return a hold label, NOT `None`. A supervision fence that
/// treats "can't tell" as "not supervised" would merge a drive/guided/operator
/// PR unreviewed; the safe direction is to HOLD (the PR is left open for a human
/// and merges on a later readable pass — never wedged, only conservatively
/// parked). The dangerous direction — merging supervised work unreviewed — is
/// exactly what this bug was.
// trace:BUG-1163 | ai:claude (supersedes the TASK-1169 fail-open tolerance)
pub(crate) fn wave_pr_supervision_label(
    project_root: &std::path::Path,
    spec: &str,
) -> Option<String> {
    let want = spec.to_ascii_uppercase();
    // `Some(mode)` ONLY when the spec's row was actually read from the store;
    // `None` means unresolvable — store unreadable (a concurrent write) OR spec
    // absent — and the fail-closed helper turns that into a hold.
    let resolved = load_store_for_lookup(project_root).and_then(|store| {
        store
            .requirements
            .iter()
            .find(|r| {
                r.spec_id
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case(&want))
                    .unwrap_or(false)
                    || r.agreed_id
                        .as_deref()
                        .map(|s| s.eq_ignore_ascii_case(&want))
                        .unwrap_or(false)
            })
            .map(|r| r.execution_mode)
    });
    supervision_hold_label(spec, resolved)
}

/// BUG-1163: the fail-closed supervision decision, split from store I/O so the
/// safety property is unit-testable. `resolved` is `Some(mode)` only when the
/// spec's row was read from the store (`mode` may itself be `None` = unset);
/// `None` means the mode was unresolvable (store unreadable OR spec absent) and
/// MUST hold — a supervision fence never merges on "can't tell", because the
/// dangerous direction is merging drive/guided/operator work unreviewed, while a
/// false hold only leaves a PR open for a human.
// trace:BUG-1163 | ai:claude
pub(crate) fn supervision_hold_label(
    spec: &str,
    resolved: Option<Option<aida_core::ExecutionMode>>,
) -> Option<String> {
    match resolved {
        None => Some(format!(
            "merge supervision unresolved for {spec} — held for safety"
        )),
        Some(mode) if pr_ship::merge_requires_supervision(mode) => {
            Some(pr_ship::supervision_mode_label(mode))
        }
        Some(_) => None,
    }
}

#[cfg(test)]
mod bug_1163_supervision_fail_closed_tests {
    use super::*;
    use aida_core::ExecutionMode;

    // BUG-1163: an unresolvable mode — store unreadable (a concurrent write) OR
    // the spec absent — MUST hold. This is the exact leak: three drive specs
    // merged because the resolver failed open on a concurrent-write read.
    #[test]
    fn unresolved_mode_holds_fail_closed() {
        assert!(
            supervision_hold_label("BUG-9999", None).is_some(),
            "unresolvable mode must HOLD (fail closed), never merge"
        );
    }

    // Every non-drain mode (incl. unset) holds; only drain merges.
    #[test]
    fn resolved_modes_gate_correctly() {
        for m in [
            ExecutionMode::Drive,
            ExecutionMode::Guided,
            ExecutionMode::Operator,
            ExecutionMode::Decide,
        ] {
            assert!(
                supervision_hold_label("X", Some(Some(m))).is_some(),
                "{m:?} must hold"
            );
        }
        assert!(
            supervision_hold_label("X", Some(None)).is_some(),
            "unset mode is supervised — must hold"
        );
        assert!(
            supervision_hold_label("X", Some(Some(ExecutionMode::Drain))).is_none(),
            "drain is the only mode that auto-merges"
        );
    }
}

/// TASK-1169: probe one PR's review decision + mergeability from the forge.
/// Degrades to `(false, Unknown)` on any probe failure — "couldn't tell" must
/// not manufacture a RequestChanges, and `Unknown` mergeability is handled
/// safely by the gate (the forge refuses an unmergeable merge; it never
/// corrupts).
// trace:TASK-1169 | ai:claude
pub(crate) fn wave_pr_review_facts(
    project_root: &std::path::Path,
    pr_number: u64,
) -> (bool, integrate::MergeableState, Option<String>) {
    let change_ref = forge::ChangeRef {
        id: pr_number,
        url: String::new(),
        branch: String::new(),
        base: String::new(),
        title: None,
    };
    match forge::forge_for(project_root).change_status(&change_ref) {
        Ok(status) => (
            status.review == forge::ReviewDecision::ChangesRequested,
            if status.mergeable {
                integrate::MergeableState::Mergeable
            } else {
                // `gh` reports non-mergeable for both a real conflict and a
                // not-yet-computed state; the gate treats Unknown optimistically
                // and the forge still refuses a dirty merge, so this degrade is
                // safe in both directions.
                integrate::MergeableState::Unknown
            },
            (!status.head_sha.trim().is_empty()).then_some(status.head_sha),
        ),
        Err(_) => (false, integrate::MergeableState::Unknown, None),
    }
}

/// TASK-1169: does a local `.aida/review-verdicts/PR-N.json` block the merge?
/// True for a RequestChanges/Rejected verdict, a reviewer escalation, or an
/// existing artifact that cannot be reconciled. The launcher must never merge
/// over a reviewer any more than the orchestrator does. A genuinely missing
/// file is not a block (the wave may simply not have used delegated review),
/// but an existing unreadable file is evidence whose meaning cannot be proven
/// and therefore fails closed.
// trace:TASK-1169 | ai:claude
pub(crate) fn local_verdict_blocks_merge(
    project_root: &std::path::Path,
    pr_number: u64,
    spec: &str,
    current_sha: Option<&str>,
) -> bool {
    let paths = [
        review_verdict::verdict_path(project_root, &format!("PR-{pr_number}")),
        review_verdict::verdict_path(project_root, spec),
    ];
    let existing: Vec<_> = paths.iter().filter(|path| path.exists()).collect();
    if existing.is_empty() {
        return false;
    }
    let bodies: Vec<String> = match existing
        .iter()
        .map(|path| std::fs::read_to_string(path))
        .collect::<Result<_, _>>()
    {
        Ok(bodies) => bodies,
        Err(_) => return true,
    };
    let Some(current_sha) = current_sha else {
        return true;
    };
    !matches!(
        review_verdict::reconcile_artifacts_for_sha(bodies.iter().map(String::as_str), current_sha),
        Ok(Some(review_verdict::VerdictKind::Approved))
    )
}

#[cfg(test)]
mod bug_1581_merge_block_tests {
    use super::*;

    #[test]
    fn local_merge_chokepoint_blocks_an_approved_top_level_with_opposition() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".aida/review-verdicts/PR-2066.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            r#"{
              "verdict":"Approved",
              "reviewed_sha":"ac772eaca9d389fa762a232156df996023bfdf7a",
              "recorded_by":"reviewer-a",
              "rounds":[{
                "verdict":"RequestChanges",
                "reviewed_sha":"ac772eaca9",
                "recorded_by":"reviewer-b"
              }]
            }"#,
        )
        .unwrap();
        assert!(local_verdict_blocks_merge(
            root.path(),
            2066,
            "BUG-1581",
            Some("ac772eaca9d389fa762a232156df996023bfdf7a")
        ));
    }

    #[test]
    fn local_merge_chokepoint_reconciles_pr_and_spec_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("PR-2090.json"),
            r#"{"verdict":"Approved","reviewed_sha":"ac772eaca9d389fa762a232156df996023bfdf7a","recorded_by":"reviewer-pr"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("STORY-1448.json"),
            r#"{"verdict":"RequestChanges","reviewed_sha":"ac772eaca9d389fa762a232156df996023bfdf7a","recorded_by":"reviewer-spec"}"#,
        )
        .unwrap();
        assert!(local_verdict_blocks_merge(
            root.path(),
            2090,
            "STORY-1448",
            Some("ac772eaca9d389fa762a232156df996023bfdf7a")
        ));
    }

    #[test]
    fn wave_gate_uses_forge_head_not_stale_pr_artifact_head() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("PR-2090.json"),
            r#"{"verdict":"Approved","reviewed_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","recorded_by":"reviewer-pr"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("STORY-1448.json"),
            r#"{"verdict":"RequestChanges","reviewed_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","recorded_by":"reviewer-spec"}"#,
        )
        .unwrap();
        assert!(local_verdict_blocks_merge(
            root.path(),
            2090,
            "STORY-1448",
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        ));
        assert!(local_verdict_blocks_merge(
            root.path(),
            2090,
            "STORY-1448",
            None
        ));
    }
}

/// TASK-1169: squash-merge one gated-clean wave PR, then run `aida pull` so the
/// merge-driven Done→Completed auto-bump fires. `--delete-branch` is
/// deliberately NOT set: the implementer's worktree still holds the branch, so
/// the forge-side delete's local cleanup grumbles and (in the agent's `&&`
/// chain) used to swallow the pull leg — worktree pruning owns branch deletion
/// (BUG-758). Returns whether the merge itself succeeded.
// trace:TASK-1169 | ai:claude
pub(crate) fn merge_wave_pr(project_root: &std::path::Path, pr: &burndown::ResidualPr) -> bool {
    // BUG-1163: the merge-EXECUTION chokepoint self-guards. Even though the wave
    // loop already gates on `wave_pr_supervision_label` via `wave_pr_action`,
    // this second check at the point the merge actually runs means no caller
    // (a future wave path, a refactor, a re-entry) can merge a supervised spec
    // by bypassing the upstream gate. It reuses the same fail-closed resolver,
    // so an unresolvable mode holds here too. trace:BUG-1163 | ai:claude
    if let Some(label) = wave_pr_supervision_label(project_root, &pr.spec) {
        // BUG-1167: persist the hold as a substrate marker so a CONCURRENT
        // merger (an integrate sweep, another agent session, a stale-binary
        // drain) also refuses at the merge_change chokepoint — not just this
        // in-process guard, which was the BUG-1167 gap. trace:BUG-1167 | ai:claude
        let _ = crate::merge_hold::write_typed_hold(
            project_root,
            &crate::merge_hold::typed_hold(
                pr.number,
                crate::merge_hold::HoldReasonKind::Supervision,
                &label,
                None,
            )
            .with_spec(&pr.spec),
        );
        // trace:BUG-1236 | ai:claude
        if let Err(err) = crate::merge_hold::sync_label(project_root, pr.number, true) {
            eprintln!(
                "  {} merge-hold label not applied on PR-{}: {err} — run `aida merge-hold list --fix`",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                pr.number
            );
        }
        eprintln!(
            "  {} refusing to auto-merge PR-{} ({}) — {}",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            pr.number,
            pr.spec,
            label,
        );
        return false;
    }
    // Re-probe the forge head and reconcile every local artifact at the
    // irreversible boundary. The earlier wave classification is only a
    // snapshot; evidence or the PR head may change before merge execution.
    // trace:BUG-1581 | ai:codex
    let (_, _, forge_head) = wave_pr_review_facts(project_root, pr.number);
    if local_verdict_blocks_merge(project_root, pr.number, &pr.spec, forge_head.as_deref()) {
        eprintln!(
            "  {} refusing to auto-merge PR-{} ({}) — current-head review evidence is conflicting, blocking, or unprovable",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            pr.number,
            pr.spec,
        );
        return false;
    }
    // TASK-1244 / ADR-41: the wave merge shares the per-branch merge-lease with
    // `aida pr ship` and the drain merge phase, so no AIDA merge path runs
    // unserialized. Contention here just skips the PR for this wave.
    // trace:TASK-1244 | ai:claude
    let lease_root = main_worktree_root_from(project_root);
    let lease_target = crate::pr_cmd::pr_ship_target_branch(pr.number);
    let _merge_lease = match crate::merge_lock::acquire(
        &lease_root,
        &lease_target,
        Some(pr.number),
        "aida burndown wave merge",
        crate::merge_lock::DEFAULT_WAIT,
    ) {
        Ok(lease) => lease,
        Err(e) => {
            eprintln!(
                "  {} PR-{} ({}) skipped this wave — {e} (`aida merge-lock` shows the holder)",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                pr.number,
                pr.spec,
            );
            return false;
        }
    };
    let mut sink = network_retry::StderrSink;
    let change_ref = forge::ChangeRef {
        id: pr.number,
        url: String::new(),
        branch: pr.branch.clone(),
        base: String::new(),
        title: None,
    };
    let opts = forge::MergeOptions {
        method: forge::MergeMethod::Squash,
        squash_subject: None,
        squash_body: None, // trace:TASK-1330 | ai:claude
        delete_branch: false,
        match_head: None,
    };
    match forge::forge_for(project_root).merge_change(&change_ref, &opts, &mut sink) {
        Ok(_) => {
            println!(
                "  {} merged PR-{} ({})",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                pr.number,
                pr.spec.cyan(),
            );
            // The auto-bump: `aida pull` promotes the spec Done → Completed
            // when the referencing commit lands on main. Its own step, never
            // chained, so a grumble in the merge cleanup can't drop it.
            let aida = aida_exe_path();
            let pulled = std::process::Command::new(aida)
                .current_dir(project_root)
                .arg("pull")
                .status_retrying_etxtbsy();
            if !matches!(&pulled, Ok(s) if s.success()) {
                eprintln!(
                    "    {} `aida pull` did not succeed after merging PR-{} — the spec may still \
                     read Done; recover with `aida db reconcile-status --spec {}`",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    pr.number,
                    pr.spec,
                );
            }
            true
        }
        Err(e) => {
            eprintln!(
                "  {} merging PR-{} failed: {e:#}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                pr.number,
            );
            false
        }
    }
}

/// TASK-1169 / ADR-22: park one wave spec `NeedsAttention` with a recorded
/// reason and file a finding — the punt-not-abort safety item. The PR is left
/// OPEN and untouched; nothing is terminated and no uncommitted work is
/// orphaned without a record. Both writes are best-effort: a store or finding
/// failure is reported, never fatal, so one bad park can't stop the drain.
// trace:TASK-1169 | ai:claude
pub(crate) fn park_wave_pr(project_root: &std::path::Path, pr: &burndown::ResidualPr, why: &str) {
    println!(
        "  {} PR-{} ({}) parked — {}",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        pr.number,
        pr.spec.cyan(),
        why,
    );
    let detail = format!("PR-{} ({}): {}", pr.number, pr.title, why);
    match shelve_spec_on_failure(
        project_root,
        &pr.spec,
        "integrate",
        4,
        "integration-wait",
        &detail,
        "review the PR, then re-run `aida burndown run` (or merge it by hand once it is green)",
    ) {
        Ok(Some(_)) => {}
        Ok(None) => eprintln!(
            "    {} {} could not be parked (already terminal or not in the store) — it is \
             reported in the handoff instead",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            pr.spec,
        ),
        Err(e) => eprintln!(
            "    {} could not park {}: {e:#}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            pr.spec,
        ),
    }
    if let Err(e) = file_integration_wait_finding(project_root, pr, why) {
        eprintln!(
            "    {} could not file the integration finding for PR-{}: {e:#}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            pr.number,
        );
    }
}

/// TASK-1169 / ADR-22: file the parked-integration finding so an expired or
/// refused wave integration lands in the triage surface the operator already
/// sweeps (`aida findings list`) rather than only in a drain's scrollback.
/// Mirrors `file_reviewer_verdict_unavailable_finding`'s shape.
// trace:TASK-1169 | ai:claude
pub(crate) fn file_integration_wait_finding(
    project_root: &std::path::Path,
    pr: &burndown::ResidualPr,
    why: &str,
) -> anyhow::Result<()> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        anyhow::bail!("no distributed store found");
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    let title = format!("Wave integration parked for {} (PR {})", pr.spec, pr.number);
    let note = format!(
        "The burndown launcher's integration wait did not merge this PR: {why}\n\n\
         The PR is still OPEN and nothing was terminated. Triage it, then merge by hand \
         or re-run `aida burndown run`."
    );
    let mut req = aida_core::Requirement::new(title, note);
    req.req_type = aida_core::RequirementType::Task;
    req.status = aida_core::RequirementStatus::Draft;
    req.owner = get_default_author();
    req.tags.insert(format!("from-review:PR-{}", pr.number));
    req.tags.insert("kind:IntegrationWaitParked".to_string());
    req.tags.insert("severity:major".to_string());
    req.tags.insert("aida:burndown".to_string());

    // CR-8: stamp filing provenance BEFORE the store write — the object is
    // rewritten below from the in-memory copy. trace:CR-8 | ai:claude
    aida_core::provenance::stamp_if_absent(&mut req);
    let store = backend.update_atomically(|store| {
        let type_prefix = store.get_type_prefix(&req.req_type);
        store.add_requirement_with_id(req.clone(), None, type_prefix.as_deref());
    })?;
    let written = store
        .requirements
        .last()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("add_requirement_with_id produced no requirement"))?;
    aida_core::object_store::write_object(&store_path.join("objects"), &written)?;
    record_role_activity(written.spec_id.as_deref().unwrap_or("?"), "findings-add");
    Ok(())
}

/// TASK-804 (facet a of STORY-604): build the argv (after the `claude` program
/// name) for the `--verbose` burndown drain. Identical base to the quiet launch
/// (`-p <prompt> --permission-mode <mode>`) PLUS the stream-json flags so the
/// drain's events can be teed and rendered live. The permission mode stays
/// CALLER-controlled (not the headless helper's baked-in `bypassPermissions`)
/// because `--verbose` must only add visibility — it must never change the
/// drain's permission posture or control flow. The prompt stays the trailing
/// positional. Pure + unit-tested. trace:TASK-804 | ai:claude
pub(crate) fn burndown_verbose_claude_args(prompt: &str, mode: &str) -> Vec<String> {
    vec![
        "-p".to_string(),
        "--permission-mode".to_string(),
        mode.to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
        prompt.to_string(),
    ]
}

/// TASK-804: the discoverable JSONL log path for a `--verbose` burndown drain:
/// `.aida/burndown/<drain-id>.jsonl`. Gitignored by the deny-by-default
/// `.aida/*` rule — pure per-clone runtime state. This is the path future
/// drain-status tooling (TASK-806) will read. trace:TASK-804 | ai:claude
pub(crate) fn burndown_drain_log_path(
    project_root: &std::path::Path,
    drain_id: &str,
) -> std::path::PathBuf {
    project_root
        .join(".aida")
        .join("burndown")
        .join(format!("{drain_id}.jsonl"))
}

/// TASK-806: the most recent burndown event log under `.aida/burndown/`, if any.
/// Drain ids are `%Y%m%dT%H%M%SZ-<uuid8>` (see [`run_burndown_verbose`]), so a
/// lexicographic sort of the `*.jsonl` file names is chronological — the last
/// entry is the newest run. The pointer is best-effort: a `--verbose` drain
/// writes one of these, a quiet drain does not, so absence is normal.
/// trace:TASK-806 | ai:claude
pub(crate) fn latest_burndown_log(project_root: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = project_root.join(".aida").join("burndown");
    let mut logs: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .collect();
    logs.sort();
    logs.pop()
}

/// TASK-806: one in-flight leased worktree the drain has fanned an implementer
/// into — the read-side answer to "what specs are being worked right now".
/// Derived from the live session leases (`.aida/sessions/*.toml`).
/// trace:TASK-806 | ai:claude
pub(crate) struct InFlightLease {
    pub(crate) scope: String,
    pub(crate) branch: String,
    pub(crate) role: String,
    pub(crate) worktree: String,
    /// TASK-834: most-recent activity in this lease's worktree (newest of the
    /// last commit time and the newest non-`.git`/non-`target` file mtime).
    /// `None` when the worktree is gone or git/fs probing failed — render the
    /// row without the activity suffix rather than error. trace:TASK-834
    pub(crate) last_activity: Option<chrono::DateTime<chrono::Utc>>,
}

/// TASK-805: spec ids with a live (non-stale) session lease — "actively being
/// worked" (the InProgress half of the running-drain marking). Reuses the same
/// lease-liveness classification `aida session leases` uses. A lease's `scope`
/// is the spec id for a single-spec session; epic/batch scopes simply won't
/// match a spec id, which is the correct (no-mark) outcome. trace:TASK-805
pub(crate) fn leased_spec_ids(project_root: &std::path::Path) -> std::collections::HashSet<String> {
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    list_leases(project_root)
        .into_iter()
        .filter(|l| !matches!(lease_state_for(l, &live, now), LeaseState::Stale))
        .map(|l| l.scope)
        .collect()
}

/// TASK-805: a LIVE-drain overlay for the read views (`queue list`, `burndown
/// plan`). When a `burndown run` / `queue work --auto-complete` drain holds the
/// BUG-538 lock, these views mark the specs it owns: **in-flight** (an
/// implementer is actively leased on it) vs **scheduled** (claimed by the
/// drain's blessed/queued set, not yet picked up). Reads the SAME
/// `.aida/drain.lock` (`drain_lock::probe_lock`) the drain writes — never a
/// parallel liveness probe (advisor note 2026-06-14). trace:TASK-805
pub(crate) struct DrainOverlay {
    pub(crate) pid: u32,
    /// Spec ids with a live session lease — actively being worked.
    pub(crate) in_flight: std::collections::HashSet<String>,
}

impl DrainOverlay {
    /// Probe the lock + live leases. `Some` only when a drain is ACTIVELY
    /// running (lock present + pid alive); `None` for no-drain or a stale lock
    /// — the marking is about a live drain, so a crashed lock adds no overlay
    /// (a reader who wants the crash signal uses `burndown status`). trace:TASK-805
    pub(crate) fn probe(project_root: &std::path::Path) -> Option<DrainOverlay> {
        match drain_lock::probe_lock(project_root) {
            drain_lock::LockStatus::Running(l) => Some(DrainOverlay {
                pid: l.pid,
                in_flight: leased_spec_ids(project_root),
            }),
            _ => None,
        }
    }

    /// Partition `specs` (a set the caller is displaying) into (in_flight,
    /// scheduled): in_flight = has a live lease, scheduled = the rest. Both
    /// sorted for stable output. trace:TASK-805
    pub(crate) fn partition(&self, specs: &[String]) -> (Vec<String>, Vec<String>) {
        let (mut in_flight, mut scheduled): (Vec<String>, Vec<String>) = specs
            .iter()
            .cloned()
            .partition(|id| self.in_flight.contains(id));
        in_flight.sort();
        scheduled.sort();
        (in_flight, scheduled)
    }
}

/// TASK-805: the "a drain is running" banner shared by `queue list` and
/// `burndown plan`. Pure over its inputs (the spec ids already partitioned by
/// the caller from the set it displays) so it renders identically in tests and
/// at the terminal. trace:TASK-805
pub(crate) fn drain_running_banner(pid: u32, in_flight: &[String], scheduled: &[String]) -> String {
    let cell = |label: &str, ids: &[String]| -> String {
        if ids.is_empty() {
            format!("{label}: none")
        } else {
            format!("{label}: {}", ids.join(", "))
        }
    };
    format!(
        "{} a drain is running (pid {pid}) — {}; {}",
        "⚡".yellow().bold(),
        cell("in-flight", in_flight),
        cell("scheduled", scheduled),
    )
}

/// TASK-806: `aida burndown status` — the read-side companion to `burndown
/// run`. Answers, from the substrate alone: is a drain running (the BUG-538
/// global drain lock + a PID-liveness probe), what is it working (the live
/// session leases — the fanned-out implementer worktrees), and where is the
/// live event log (`.aida/burndown/<drain-id>.jsonl`, TASK-804) to tail. Pure
/// read; exits 0 whether or not a drain is running. trace:TASK-806 | ai:claude
pub(crate) fn handle_burndown_status(json: bool) -> Result<()> {
    // Resolve the shared `.aida/` root from any worktree so a child in a sibling
    // worktree reads the *orchestrator's* lock / leases — mirrors `aida drain
    // status`. Fails safe to "no drain" on a resolution error.
    let project_root = find_main_worktree_root()
        .or_else(|_| std::env::current_dir())
        .unwrap_or_else(|_| std::path::PathBuf::from("."));

    let lock = drain_lock::probe_lock(&project_root);

    // In-flight = the live (non-stale) leases — the implementer worktrees the
    // drain fanned out. Same liveness classification `aida session leases` uses.
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    let in_flight: Vec<InFlightLease> = list_leases(&project_root)
        .into_iter()
        .filter(|l| !matches!(lease_state_for(l, &live, now), LeaseState::Stale))
        .map(|l| InFlightLease {
            scope: l.scope.clone(),
            branch: l.branch.clone(),
            role: l.role.clone().unwrap_or_else(|| "-".to_string()),
            worktree: l.worktree_path.display().to_string(),
            // TASK-834: best-effort per-agent activity probe from the worktree.
            last_activity: worktree_last_activity(&l.worktree_path),
        })
        .collect();

    let log = latest_burndown_log(&project_root);

    if json {
        println!(
            "{}",
            render_burndown_status_json(&lock, &in_flight, log.as_deref())
        );
    } else {
        print!(
            "{}",
            render_burndown_status_human(&lock, &in_flight, log.as_deref())
        );
        // TASK-833: open-PR section — the universal "work in flight, any source"
        // signal that leases miss (keyboard agents, Agent-tool fan-out, manual
        // PRs never register an AIDA lease, so they're invisible mid-flight until
        // the merge heartbeat). Best-effort: empty (forge-aware degrade) → skip.
        // trace:TASK-833 | ai:claude
        let mut open_prs: Vec<status_cleanup::OpenPrItem> = collect_open_prs(&project_root)
            .by_branch
            .into_values()
            .collect();
        if !open_prs.is_empty() {
            const PR_CAP: usize = 15;
            // Stable order: lowest PR number (oldest) first.
            open_prs.sort_by_key(|pr| pr.number);
            println!(
                "\n  {} Open PRs ({}) — awaiting merge:",
                "◆".cyan().bold(),
                open_prs.len()
            );
            for pr in open_prs.iter().take(PR_CAP) {
                println!(
                    "    {} {}  {}",
                    format!("#{}", pr.number).yellow(),
                    pr.title,
                    pr.head_branch.dimmed()
                );
            }
            if open_prs.len() > PR_CAP {
                println!(
                    "    {}",
                    format!("+{} more", open_prs.len() - PR_CAP).dimmed()
                );
            }
        }
        // TASK-829: recent-activity heartbeat. Without this, "no drain running"
        // reads as "nothing is happening" even when an advisor agent is clearing
        // specs at the keyboard (worktree → PR → merge, which never registers a
        // lease). Surfacing recent completions makes the view a liveness signal,
        // not just a drain-state probe. trace:TASK-829 | ai:claude
        if let Some(line) = recent_activity_line(&project_root, now) {
            println!("{line}");
        }
    }
    Ok(())
}

/// TASK-829: a one-line recent-activity heartbeat for `aida burndown status` /
/// `aida list inflight` — specs that reached Completed in the last 24h, read
/// from the store (`modified_at` is stamped on the status flip, incl. the
/// auto-bump on merge). Proves the system is making progress even when no drain
/// or lease is live. `None` only when the store can't be loaded.
/// trace:TASK-829 | ai:claude
pub(crate) fn recent_activity_line(
    project_root: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let store = load_store_for_lookup(project_root)?;
    let cutoff = now - chrono::Duration::hours(24);
    let mut done: Vec<(String, chrono::DateTime<chrono::Utc>)> = store
        .requirements
        .iter()
        .filter(|r| matches!(r.status, aida_core::RequirementStatus::Completed))
        .filter(|r| r.modified_at >= cutoff)
        .map(|r| {
            let id = r
                .agreed_id
                .clone()
                .or_else(|| r.spec_id.clone())
                .unwrap_or_else(|| r.id.to_string());
            (id, r.modified_at)
        })
        .collect();
    if done.is_empty() {
        return Some(format!(
            "\n  {} no completions in the last 24h · {} for the full feed",
            "○".dimmed(),
            "aida history".cyan()
        ));
    }
    done.sort_by(|a, b| b.1.cmp(&a.1));
    let (last_id, last_t) = &done[0];
    Some(format!(
        "\n  {} recent: {} spec{} completed in last 24h — last: {} ({}) · {}",
        "●".green(),
        done.len(),
        if done.len() == 1 { "" } else { "s" },
        last_id.cyan(),
        humanize_relative(*last_t),
        "aida history".cyan()
    ))
}

/// TASK-834: idle threshold past which a still-alive lease is flagged "possibly
/// stuck" — the process exists but nothing has moved in the worktree for a
/// while. trace:TASK-834
pub(crate) const ACTIVITY_STUCK_THRESHOLD_SECS: i64 = 10 * 60;

/// TASK-834: best-effort last-activity timestamp for a lease's worktree. Returns
/// the newest of (the worktree's `git log -1` commit time) and (the newest
/// non-`.git`/non-`target` file mtime). `None` when the worktree is missing or
/// every probe failed — callers render the row without an activity suffix rather
/// than erroring. trace:TASK-834 | ai:claude
pub(crate) fn worktree_last_activity(
    worktree: &std::path::Path,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if worktree.as_os_str().is_empty() || !worktree.exists() {
        return None;
    }

    let mut newest: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut consider = |t: chrono::DateTime<chrono::Utc>| {
        newest = Some(match newest {
            Some(cur) if cur >= t => cur,
            _ => t,
        });
    };

    // Leg 1: last commit time (epoch seconds) via `git log -1 --format=%ct`.
    if let Ok(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["log", "-1", "--format=%ct"])
        .output()
    {
        if output.status.success() {
            if let Ok(epoch) = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<i64>()
            {
                if let Some(dt) = chrono::DateTime::from_timestamp(epoch, 0) {
                    consider(dt);
                }
            }
        }
    }

    // Leg 2: newest non-`.git`/non-`target` file mtime, shallow walk of the
    // worktree's top entries (best-effort; depth-bounded to stay cheap).
    if let Some(dt) = newest_file_mtime(worktree, 3) {
        consider(dt);
    }

    newest
}

/// TASK-834: newest file mtime under `dir`, skipping `.git` and `target`,
/// bounded to `max_depth` levels. Best-effort: unreadable entries are skipped.
/// trace:TASK-834 | ai:claude
pub(crate) fn newest_file_mtime(
    dir: &std::path::Path,
    max_depth: usize,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let mut newest: Option<chrono::DateTime<chrono::Utc>> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == ".git" || name == "target" {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if max_depth > 0 {
                if let Some(dt) = newest_file_mtime(&path, max_depth - 1) {
                    if newest.map(|cur| dt > cur).unwrap_or(true) {
                        newest = Some(dt);
                    }
                }
            }
        } else if let Ok(mtime) = meta.modified() {
            let dt: chrono::DateTime<chrono::Utc> = mtime.into();
            if newest.map(|cur| dt > cur).unwrap_or(true) {
                newest = Some(dt);
            }
        }
    }
    newest
}

/// TASK-834: format the per-agent activity label from a last-activity timestamp
/// and the current time. Pure — the unit-tested core of the in-flight activity
/// signal. `… active 15s ago` when recent; `… idle 15m possibly stuck` (with a warning) once
/// the gap exceeds [`ACTIVITY_STUCK_THRESHOLD_SECS`]. trace:TASK-834 | ai:claude
pub(crate) fn format_activity_label(
    last_activity: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let secs = now
        .signed_duration_since(last_activity)
        .num_seconds()
        .max(0);
    let rel = humanize_idle(secs);
    if secs >= ACTIVITY_STUCK_THRESHOLD_SECS {
        format!(
            "idle {rel} {} possibly stuck",
            crate::glyph(crate::glyphs::Glyph::Warning)
        )
    } else {
        format!("active {rel} ago")
    }
}

/// TASK-834: compact `Ns`/`Nm`/`Nh`/`Nd` for an elapsed-seconds gap (no " ago"
/// suffix — callers add their own framing). trace:TASK-834 | ai:claude
pub(crate) fn humanize_idle(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// TASK-806: human-readable `burndown status` summary. Pure over its inputs so
/// the three lock states render identically in tests and at the terminal.
/// trace:TASK-806 | ai:claude
pub(crate) fn render_burndown_status_human(
    lock: &drain_lock::LockStatus,
    in_flight: &[InFlightLease],
    log: Option<&std::path::Path>,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} burndown status",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    let _ = writeln!(out);

    match lock {
        drain_lock::LockStatus::None => {
            let _ = writeln!(out, "  {} no drain running", "○".dimmed());
            let _ = writeln!(out, "  {}", "start one with: aida burndown run".dimmed());
        }
        drain_lock::LockStatus::Running(l) => {
            let when = burndown_lock_when(&l.started_at_utc);
            let _ = writeln!(
                out,
                "  {} drain running — pid {}, started {}",
                "●".green().bold(),
                l.pid,
                when
            );
            let _ = writeln!(out, "    {}", l.command.dimmed());
            if !l.host.is_empty() {
                let _ = writeln!(out, "    {}", format!("host: {}", l.host).dimmed());
            }
        }
        drain_lock::LockStatus::Stale(l) => {
            let when = burndown_lock_when(&l.started_at_utc);
            let _ = writeln!(
                out,
                "  {} stale drain lock — pid {} (started {}) is no longer running",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                l.pid,
                when
            );
            let _ = writeln!(out, "    {}", l.command.dimmed());
            let _ = writeln!(
                out,
                "    {}",
                "the drain crashed or exited without releasing the lock; the next `aida burndown run` reclaims it"
                    .dimmed()
            );
        }
    }

    // In-flight leased worktrees — what the drain is working right now.
    if !in_flight.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  {}",
            format!("In-flight ({} leased):", in_flight.len()).bold()
        );
        // TASK-834: per-agent activity suffix so a committing agent reads
        // differently from one idle/stuck 15m. Computed against now from each
        // lease's captured last-activity; absent (worktree gone / probe failed)
        // → no suffix, the row still renders. trace:TASK-834
        let now = chrono::Utc::now();
        for f in in_flight {
            let _ = write!(
                out,
                "    {:<20} {:<18} {:<14} {}",
                truncate(&f.scope, 20),
                truncate(&f.branch, 18),
                truncate(&f.role, 14),
                f.worktree.dimmed()
            );
            if let Some(t) = f.last_activity {
                let label = format_activity_label(t, now);
                let secs = now.signed_duration_since(t).num_seconds().max(0);
                let painted = if secs >= ACTIVITY_STUCK_THRESHOLD_SECS {
                    label.yellow().to_string()
                } else {
                    label.green().to_string()
                };
                let _ = write!(out, "  {painted}");
            }
            let _ = writeln!(out);
        }
    }

    // Live log pointer — the TASK-804 event stream to tail.
    if let Some(path) = log {
        let _ = writeln!(out);
        let _ = writeln!(out, "  Live log: {}", path.display().to_string().cyan());
        let _ = writeln!(
            out,
            "    {}",
            format!("tail -f {}", path.display()).dimmed()
        );
    }

    out
}

/// TASK-806: render the start time as `local-time (Nm ago)`, falling back to the
/// raw RFC-3339 string if it does not parse. trace:TASK-806 | ai:claude
pub(crate) fn burndown_lock_when(started_at_utc: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(started_at_utc) {
        Ok(dt) => {
            let utc = dt.with_timezone(&chrono::Utc);
            let local = dt
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string();
            format!("{local} ({})", humanize_relative(utc))
        }
        Err(_) => started_at_utc.to_string(),
    }
}

/// TASK-806: machine-readable `burndown status`. trace:TASK-806 | ai:claude
pub(crate) fn render_burndown_status_json(
    lock: &drain_lock::LockStatus,
    in_flight: &[InFlightLease],
    log: Option<&std::path::Path>,
) -> String {
    let drain = match lock {
        drain_lock::LockStatus::None => serde_json::json!({ "running": false }),
        drain_lock::LockStatus::Running(l) => serde_json::json!({
            "running": true,
            "stale": false,
            "pid": l.pid,
            "started_at": l.started_at_utc,
            "command": l.command,
            "host": l.host,
        }),
        drain_lock::LockStatus::Stale(l) => serde_json::json!({
            "running": false,
            "stale": true,
            "pid": l.pid,
            "started_at": l.started_at_utc,
            "command": l.command,
            "host": l.host,
        }),
    };
    let in_flight_json: Vec<serde_json::Value> = in_flight
        .iter()
        .map(|f| {
            // TASK-834: surface the activity signal to machine consumers too —
            // `last_activity` (RFC-3339) plus a derived `stuck` flag. null when
            // the worktree probe found nothing. trace:TASK-834
            let now = chrono::Utc::now();
            let stuck = f.last_activity.map(|t| {
                now.signed_duration_since(t).num_seconds() >= ACTIVITY_STUCK_THRESHOLD_SECS
            });
            serde_json::json!({
                "spec": f.scope,
                "branch": f.branch,
                "role": f.role,
                "worktree": f.worktree,
                "last_activity": f.last_activity.map(|t| t.to_rfc3339()),
                "stuck": stuck,
            })
        })
        .collect();
    let value = serde_json::json!({
        "drain": drain,
        "in_flight": in_flight_json,
        "log": log.map(|p| p.display().to_string()),
    });
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
}

/// TASK-804: launch the `--verbose` burndown drain. Same headless `claude -p`
/// drain as the quiet path (same auto-proceed, same exit-code semantics), but
/// the stream-json stdout is redirected to `.aida/burndown/<drain-id>.jsonl`
/// and a background tee renders a human-readable progress line per event to
/// stderr — so a long drain shows live progress instead of a silent terminal.
/// Reuses the proven `headless_tee` machinery (TASK-307) rather than rolling a
/// new renderer. trace:TASK-804 | ai:claude
pub(crate) fn run_burndown_verbose(prompt: &str, mode: &str) -> Result<std::process::ExitStatus> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    // TASK-1169 / ADR-22: same bounded ceiling the quiet launch sets.
    // trace:TASK-1169 | ai:claude
    let (ceiling_key, ceiling_value) = bg_wait_ceiling_env(Some(&project_root));
    // Time-prefixed id so logs sort chronologically and are easy to tail; the
    // short uuid suffix disambiguates two drains started in the same second.
    let drain_id = format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        &uuid::Uuid::now_v7().to_string()[..8]
    );
    let log_path = burndown_drain_log_path(&project_root, &drain_id);
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            anyhow::anyhow!(
                "could not create the burndown log dir {} ({e})",
                dir.display()
            )
        })?;
    }
    let log = std::fs::File::create(&log_path).map_err(|e| {
        anyhow::anyhow!(
            "could not create the burndown log {} ({e})",
            log_path.display()
        )
    })?;

    println!(
        "  {} live progress → {} (tail it live, or read it after)",
        "→".green(),
        log_path.display().to_string().cyan()
    );
    println!();

    // Tee the JSONL to stderr as human-readable lines. `from_env_and_flag(false)`
    // keeps the routine chatter on by default (the operator asked for `--verbose`),
    // while `AIDA_TEE_HEADLESS=0` can quiet it — errors/denials still surface loud.
    let tee_opts = headless_tee::TeeOptions::from_env_and_flag(false).with_label("burndown");
    let tee_handle = headless_tee::start_tee(&log_path, &tee_opts);

    let child_grant =
        seat_authority::issue_child(&project_root, "implementer", &current_user_id(None))?;
    let status_code =
        build_burndown_verbose_command(prompt, mode, &child_grant.id, ceiling_key, &ceiling_value)
            .stdout(std::process::Stdio::from(log))
            .status()
            .map_err(|e| {
                anyhow::anyhow!(
            "failed to launch `claude -p` ({e}) — the headless drain needs the Claude Code \
             CLI on PATH. Install it, or use `aida queue work --auto-complete` (the \
             orchestrator drain) instead."
        )
            });
    // Always stop the tee, even on spawn failure, so the background thread exits.
    tee_handle.stop();
    status_code
}

// trace:BUG-2124 | ai:antigravity
pub(crate) fn build_burndown_quiet_command(
    current_prompt: &str,
    mode: &str,
    ceiling_key: &str,
    ceiling_value: &str,
) -> std::process::Command {
    let mut cmd = std::process::Command::new("claude");
    cmd.arg("-p")
        .arg(current_prompt)
        .arg("--permission-mode")
        .arg(mode)
        .env("AIDA_BURNDOWN_LOCK_HELD", "1")
        .env(ceiling_key, ceiling_value);
    if crate::session::containment_applied(crate::session::HeadlessVendor::Claude, false) {
        cmd.env("AIDA_CONTAINED", "1");
    } else {
        cmd.env_remove("AIDA_CONTAINED");
    }
    cmd
}

// trace:BUG-2124 | ai:antigravity
pub(crate) fn build_burndown_verbose_command(
    prompt: &str,
    mode: &str,
    child_grant_id: &str,
    ceiling_key: &str,
    ceiling_value: &str,
) -> std::process::Command {
    let mut cmd = std::process::Command::new("claude");
    cmd.args(burndown_verbose_claude_args(prompt, mode))
        .env("AIDA_BURNDOWN_LOCK_HELD", "1")
        .env("AIDA_SESSION_ROLE", "implementer")
        .env(seat_authority::GRANT_ENV, child_grant_id)
        .env(ceiling_key, ceiling_value);
    if crate::session::containment_applied(crate::session::HeadlessVendor::Claude, false) {
        cmd.env("AIDA_CONTAINED", "1");
    } else {
        cmd.env_remove("AIDA_CONTAINED");
    }
    cmd
}

/// STORY-547: build the [`burndown::OpenFacts`] for every OPEN spec in the store
/// (non-archived, status not Completed/Rejected). Reasons are derived purely
/// from store signals + the live lease set — no new stored field. Shared by
/// `burndown explain` and `aida why`. trace:STORY-547 | ai:claude
pub(crate) fn collect_open_facts(
    store: &aida_core::RequirementsStore,
    // BUG-511: lowercased live-lease scope → role, so the explainer can say
    // "being reviewed" when the holder is a reviewer. From
    // [`in_flight_lease_role_map`].
    in_flight_scopes: &std::collections::HashMap<String, Option<String>>,
) -> Vec<burndown::OpenFacts> {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    };
    // TASK-723 (source #2): index FINDINGS by the spec they were filed against,
    // so each open spec can link its attempt-outcome findings without
    // recomputing them. A finding is a draft requirement carrying a
    // `from-implementer:<SPEC>` / `from-advisor:<SPEC>` origin tag, plus any
    // extra `linked:<SPEC>` tags. We reuse the existing finding cohort — single
    // source of truth for attempt outcomes. trace:TASK-723 | ai:claude
    let findings_by_spec = collect_findings_by_spec(store);
    let mut facts = Vec::new();
    for req in &store.requirements {
        if req.archived {
            continue;
        }
        // BUG-556: deferred is a view-flag orthogonal to status (STORY-584) —
        // `aida defer` parks a spec as primed/conditional work, hidden from
        // default views. The human worklist and `burndown explain` ARE default
        // views, so a deferred spec must drop out here exactly like an archived
        // one. (`explain_open` only recognized the legacy `deferred:` tag, not
        // the structural flag, so deferred specs leaked onto `aida human` as
        // ungroomed/umbrella.) trace:BUG-556 | ai:claude
        if req.deferred {
            continue;
        }
        // BUG-593: standing-artifact types (meta / folder / vision / principle /
        // term / constraint) are perpetual reference rows, NOT backlog work —
        // the same set `aida list` and `aida status` (BUG-464 / TASK-773) hide
        // from open-work views. Without this, a fresh `aida init`'s 6 seeded
        // META prompts surfaced on the advisor worklist's GROOM bucket (and on
        // `aida human`) as if an advisor should approve/reject AI-prompt
        // templates. trace:BUG-593 | ai:claude
        if is_standing_artifact_type(&format!("{:?}", req.req_type)) {
            continue;
        }
        // "Open" = not yet terminal. Completed/Rejected are done with.
        if matches!(
            req.status,
            aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
        ) {
            continue;
        }
        let id = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        let in_flight_entry = [req.agreed_id.as_deref(), req.spec_id.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|s| in_flight_scopes.get(&s.to_ascii_lowercase()));
        let in_flight = in_flight_entry.is_some();
        let in_flight_role = in_flight_entry.cloned().flatten();
        // TASK-723 (source #2): findings linked to this spec (deduped, stable
        // order). Don't link a finding to itself.
        let mut findings: Vec<String> = findings_by_spec
            .get(&id.to_ascii_uppercase())
            .cloned()
            .unwrap_or_default();
        findings.retain(|fid| !fid.eq_ignore_ascii_case(&id));
        // TASK-723 (source #3): residual `why-open:` notes recorded on this
        // spec's comments. Derivable state is never written here — these are
        // the non-derivable tail only.
        let residual_notes: Vec<String> = req
            .comments
            .iter()
            .filter_map(|c| burndown::parse_why_open_comment(&c.content))
            .collect();
        // BUG-543: for an epic, compute its child rollup (the same walk + tally
        // `aida graph tree` prints) so `explain_open` can surface a fully-
        // delivered epic ("N/N children Completed") as ready-to-close rather
        // than the generic umbrella. `None` for non-epics and childless epics.
        // TASK-884: also note whether any child is actually in motion
        // (Completed / Done / InProgress). An epic with children that are all
        // un-started reads as `Umbrella` (decompose lane), not the invisible
        // InProgress bucket. trace:TASK-884
        let (epic_rollup, epic_children_in_motion) =
            if matches!(req.req_type, aida_core::RequirementType::Epic) {
                let r = aida_core::graph_walk::child_status_rollup(store, req.id);
                let in_motion = r.completed + r.done + r.in_progress > 0;
                ((r.total > 0).then_some((r.completed, r.total)), in_motion)
            } else {
                (None, false)
            };
        facts.push(burndown::OpenFacts {
            id,
            req_type: format!("{:?}", req.req_type).to_ascii_lowercase(),
            status: norm(&format!("{:?}", req.status)),
            tags: req.tags.iter().cloned().collect(),
            has_unsatisfied_blocker: aida_core::pickability::blocked_by_incomplete(req, store),
            has_pending_decision: req
                .decision_request
                .as_ref()
                .map(|d| d.is_pending())
                .unwrap_or(false),
            in_flight,
            in_flight_role,
            findings,
            residual_notes,
            epic_rollup,
            // BUG-564: carry the spec's orthogonal human-only marker (the same
            // `req.human_only` the queue's "Blocked — human-only" bucket and the
            // spec-card `[human-only]` chip read) so the human worklist folds it
            // into `human_required` instead of a hardcoded `false`. A spec that
            // is human-required PURELY via this marker now surfaces in
            // `aida list human` exactly as it does in the queue.
            // trace:BUG-564 | ai:claude
            human_only: req.human_only,
            epic_children_in_motion,
        });
    }
    facts
}

/// TASK-723 (source #2): map UPPERCASE origin SPEC-ID → display-ids of the
/// findings filed against it. A finding is a draft requirement carrying a
/// `from-implementer:<SPEC>` or `from-advisor:<SPEC>` origin tag (and possibly
/// extra `linked:<SPEC>` tags). Reuses the existing finding cohort so the
/// explainer links — never recomputes — attempt outcomes. trace:TASK-723 | ai:claude
pub(crate) fn collect_findings_by_spec(
    store: &aida_core::RequirementsStore,
) -> std::collections::HashMap<String, Vec<String>> {
    let mut by_spec: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for req in &store.requirements {
        if req.archived {
            continue;
        }
        // Only findings still AWAITING TRIAGE count as open. `aida findings
        // list` defines that set as draft requirements carrying a finding tag
        // (`ListFilter { status: draft }` + `is_finding`); a finding that's
        // been resolved/escalated/promoted/dismissed leaves draft but may keep
        // its `from-*` tag. Gating on draft here keeps `aida human`'s open-
        // findings count equal to `aida findings list`'s awaiting count rather
        // than counting every finding row ever filed. trace:BUG-544
        if !matches!(req.status, RequirementStatus::Draft) {
            continue;
        }
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        if !findings::is_finding(&tags) {
            continue;
        }
        let finding_id = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        // Every SPEC-ID this finding points at: the origin tag's value plus any
        // `linked:<SPEC>` tags. `general` / non-spec origins are ignored.
        for tag in &tags {
            let t = tag.trim();
            let target = t
                .strip_prefix(findings::FROM_IMPLEMENTER_PREFIX)
                .or_else(|| t.strip_prefix(findings::FROM_ADVISOR_PREFIX))
                .or_else(|| t.strip_prefix(findings::LINKED_PREFIX));
            if let Some(spec) = target {
                let spec = spec.trim();
                if spec.is_empty() || spec.eq_ignore_ascii_case("general") {
                    continue;
                }
                let entry = by_spec.entry(spec.to_ascii_uppercase()).or_default();
                if !entry.iter().any(|e| e.eq_ignore_ascii_case(&finding_id)) {
                    entry.push(finding_id.clone());
                }
            }
        }
    }
    by_spec
}

/// STORY-547: `aida burndown explain` — classify every open spec into its
/// "why still open" bucket + one-line reason. The post-burndown companion to
/// `plan`: where `plan` answers "what can I fan out?", `explain` answers "why
/// is everything that's left still here?". trace:STORY-547 | ai:claude
pub(crate) fn handle_burndown_explain(json: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;
    let in_flight_scopes = in_flight_lease_role_map(&project_root);
    let facts = collect_open_facts(&store, &in_flight_scopes);

    // Classify, preserving store order within each bucket. TASK-723: each spec
    // now carries the FULL reason set (derived + finding-links + residual
    // notes), most-fundamental-first; the derived reason still drives the
    // bucket. trace:TASK-723
    let classified: Vec<(
        burndown::OpenFacts,
        burndown::OpenBucket,
        Vec<burndown::Reason>,
    )> = facts
        .into_iter()
        .map(|f| {
            let (bucket, reasons) = burndown::explain_reasons(&f);
            (f, bucket, reasons)
        })
        .collect();

    if json {
        let payload = classified
            .iter()
            .map(|(f, bucket, reasons)| {
                serde_json::json!({
                    "spec": f.id,
                    "bucket": bucket.key(),
                    // The primary (derived) reason — first, most-fundamental.
                    "reason": reasons.first().map(|r| r.text.as_str()).unwrap_or_default(),
                    // TASK-723: the full reason set, most-fundamental-first.
                    "reasons": reasons
                        .iter()
                        .map(|r| serde_json::json!({"source": r.source.key(), "text": r.text}))
                        .collect::<Vec<_>>(),
                    "findings": f.findings,
                    "needs_human": bucket.needs_human(),
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!(
        "{} why each open spec is still open",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    if classified.is_empty() {
        println!("  {}", "Nothing open — the backlog is clear.".dimmed());
        return Ok(());
    }
    let needs = classified
        .iter()
        .filter(|(_, b, _)| b.needs_human())
        .count();
    println!(
        "  {} {} open · {} need a human nudge, {} self-resolve through normal flow",
        "→".green(),
        classified.len(),
        needs,
        classified.len() - needs
    );

    // Group by bucket for a scannable read; ordering matches the precedence in
    // explain_open so the "needs you" buckets surface first.
    use burndown::OpenBucket::*;
    let order = [
        HeldForReview,
        AwaitingDecision,
        ReadyToClose,
        BuildSupervised,
        Ungroomed,
        Umbrella,
        Blocked,
        Deferred,
        InFlight,
        InProgress,
        AwaitingMerge,
        LongLived,
        Actionable,
    ];
    for bucket in order {
        let rows: Vec<&(
            burndown::OpenFacts,
            burndown::OpenBucket,
            Vec<burndown::Reason>,
        )> = classified.iter().filter(|(_, b, _)| *b == bucket).collect();
        if rows.is_empty() {
            continue;
        }
        let marker = if bucket.needs_human() {
            "●".yellow()
        } else {
            "·".dimmed()
        };
        // The bucket header glosses the derived reason — the first reason of the
        // first row (every row in this bucket shares the same bucket).
        let sample_reason = rows[0]
            .2
            .first()
            .map(|r| r.text.clone())
            .unwrap_or_default();
        println!(
            "\n{} {} — {}",
            marker,
            bucket.key().bold(),
            sample_reason.dimmed()
        );
        for (f, _, reasons) in rows {
            // The derived (first) reason: only repeat it when it differs from the
            // bucket header (e.g. `deferred:<why>` / decision tags vary per spec);
            // for uniform buckets just list the ID to keep the view scannable.
            let derived = reasons.first().map(|r| r.text.clone()).unwrap_or_default();
            if derived == sample_reason {
                println!("    {}", f.id.cyan());
            } else {
                println!("    {} {}", f.id.cyan(), format!("({derived})").dimmed());
            }
            // TASK-723: the additional reasons (finding-links + residual notes)
            // hang under the spec, most-fundamental-first, so the operator sees
            // every reason it's still open — not just the derived one.
            for r in reasons.iter().skip(1) {
                let tag = match r.source {
                    burndown::ReasonSource::Finding => "finding".magenta(),
                    burndown::ReasonSource::Residual => "note".yellow(),
                    burndown::ReasonSource::Derived => continue,
                };
                println!("      {} {}", tag, r.text.dimmed());
            }
        }
    }
    Ok(())
}

// TASK-773: perpetual standing-artifact types — the docs-layer / structural
// types that are NOT work and should be hidden from the default `aida list`
// open-work view. They surface via an explicit `--type <T>` filter or `--all`.
// `req_type` here is the cache summary's Display string (e.g. "Vision").
// trace:TASK-773
pub(crate) const STANDING_ARTIFACT_TYPES: [&str; 6] = [
    "vision",
    "principle",
    "term",
    "constraint",
    "folder",
    "meta",
];

// pub(crate) so the MCP status surface can mirror this exact exclusion (BUG-717).
pub(crate) fn is_standing_artifact_type(req_type: &str) -> bool {
    STANDING_ARTIFACT_TYPES
        .iter()
        .any(|s| req_type.eq_ignore_ascii_case(s))
}

#[cfg(test)]
#[path = "tests/task_773_standing_artifact_tests.rs"]
mod task_773_standing_artifact_tests;

#[cfg(test)]
#[path = "tests/bug_544_open_findings_count_tests.rs"]
mod bug_544_open_findings_count_tests;

#[cfg(test)]
#[path = "tests/bug_556_deferred_excluded_from_open_facts_tests.rs"]
mod bug_556_deferred_excluded_from_open_facts_tests;

/// STORY-545: resolve a selector to `(ready, awaiting_signoff, parked)` via the
/// pickability gate + the STORY-546 queue gate. Shared by `burndown plan` (which
/// renders it) and `burndown run` (which preflights + drains it) so the two can
/// NEVER disagree on what's drainable. trace:STORY-527 trace:STORY-546 trace:STORY-545
#[allow(clippy::type_complexity)]
// trace:BUG-532 — the title map (display-id → title) is built from the SAME
// single store load the set resolution already does, so `burndown plan`'s text
// output can show id+title with no extra store scan (no N+1 lookup).
// (ready, awaiting_signoff, serialize_held, parked, supervised, titles)
// `supervised` (STORY-610): specs tagged `supervised` — signed off for KEYBOARD
// pickup but excluded from the unattended drain. Surfaced as its own section in
// `burndown plan` so the route per spec (`queue work` vs drain) is visible.
// `serialize_held` (TASK-149): queued + blessed specs a sibling's
// `serialize:<group>` claim deferred to a LATER wave. Kept out of
// `awaiting_signoff` — nothing human is pending on them.
pub(crate) type BurndownSets = (
    Vec<String>,
    Vec<String>,
    Vec<burndown::SerializeHold>,
    Vec<(String, String)>,
    Vec<String>,
    std::collections::HashMap<String, String>,
);

pub(crate) fn resolve_burndown_sets(
    status: &str,
    tag: Option<&str>,
    batch: Option<&str>,
    type_filter: Option<&str>,
) -> Result<BurndownSets> {
    // BUG-784: normalize an explicit `--type` to the same lowercased `Debug`
    // form the per-spec `req_type` carries, so `--type non-functional` and
    // `--type adr` both compare cleanly. With no `--type`, the selector applies
    // the default work-item type-class filter instead (knowledge-class records —
    // decision / vision / term / principle — are authored, not implemented, and
    // must never be offered as drain candidates). trace:BUG-784 | ai:claude
    let type_filter_norm: Option<String> = match type_filter {
        Some(raw) => Some(format!("{:?}", parse_requirement_type(raw)?).to_ascii_lowercase()),
        None => None,
    };
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;

    // Normalize a status label to an alphanumeric-only lowercase key so
    // "in-progress" / "In Progress" / "InProgress" all compare equal.
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let want_status = norm(status);
    let batch_tag = batch.map(|b| format!("batch:{}", b.to_ascii_lowercase()));

    // uuid → status, so a BlockedBy edge can be checked for satisfaction.
    let status_by_id: std::collections::HashMap<uuid::Uuid, aida_core::RequirementStatus> = store
        .requirements
        .iter()
        .map(|r| (r.id, r.status.clone()))
        .collect();

    // STORY-546: queue membership = advisor sign-off. Union of every user's
    // queue (matches `aida list --queued`). A spec is drainable only if the
    // advisor deliberately queued it.
    let queued_ids = all_queued_requirement_ids(&project_root);
    // TASK-1175: the queue-insertion timestamps that order the ready set.
    // trace:TASK-1175 | ai:claude
    let queued_added_at = all_queued_added_at(&project_root);

    let mut candidates: Vec<burndown::BurndownCandidate> = Vec::new();
    // Display-ids of queued specs in the selector scope, so the pickable set can
    // be split into the blessed (queued) drain set vs awaiting-sign-off.
    // trace:STORY-546
    let mut queued_disp: std::collections::HashSet<String> = std::collections::HashSet::new();
    // trace:BUG-532 — display-id → title, populated from this same scan so the
    // text plan output can show id+title without re-loading the store.
    let mut titles: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // STORY-610: supervised specs collected for the keyboard-pickup section.
    let mut supervised: Vec<String> = Vec::new();
    // TASK-1172: display-id → priority label, populated from this same scan so
    // the ready set can be ordered by priority without a second store read.
    let mut priority_by_id: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // TASK-1175: display-id → queue-entry `added_at`, joined here (the only
    // place display id and requirement UUID are both in hand) so the ordering
    // sort downstream stays a pure function of its arguments.
    // trace:TASK-1175 | ai:claude
    let mut added_at_by_id: std::collections::HashMap<String, chrono::DateTime<chrono::Utc>> =
        std::collections::HashMap::new();
    for req in &store.requirements {
        // TASK-803: the per-spec filter/bucket decision now lives in the pure,
        // unit-tested `burndown::classify_spec`. The caller computes the probed
        // inputs (display id, normalized status, blocker satisfaction, pending
        // decision) and the helper applies the load-bearing filter order
        // (archived → deferred → status → tag → batch → supervised → candidate).
        // Inputs are computed eagerly for every spec; behavior is identical to
        // the prior inline loop (the skipped specs just never reach the title /
        // queued / candidate collections). trace:TASK-803 | ai:claude
        let status_norm = norm(&req.status.to_string());
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        let disp = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        // A BlockedBy edge is unsatisfied unless its target is Completed; a
        // dangling target (not in the store) is treated as unsatisfied so we
        // never fan out a spec whose blocker we can't verify.
        let has_unsatisfied_blocker = req.relationships.iter().any(|rel| {
            matches!(rel.rel_type, aida_core::RelationshipType::BlockedBy)
                && status_by_id
                    .get(&rel.target_id)
                    .map(|s| *s != aida_core::RequirementStatus::Completed)
                    .unwrap_or(true)
        });
        let has_pending_decision = req
            .decision_request
            .as_ref()
            .map(|d| d.is_pending())
            .unwrap_or(false);
        let req_type = format!("{:?}", req.req_type).to_ascii_lowercase();
        let input = burndown::SpecClassifyInput {
            archived: req.archived,
            deferred: req.deferred,
            status_norm: &status_norm,
            want_status: &want_status,
            tags: &tags,
            tag_filter: tag,
            batch_tag: batch_tag.as_deref(),
            // trace:BUG-784 | ai:claude
            type_filter: type_filter_norm.as_deref(),
            disp: &disp,
            req_type: &req_type,
            has_unsatisfied_blocker,
            has_pending_decision,
            // trace:BUG-1717 | ai:claude
            execution_mode: req.execution_mode,
        };
        match burndown::classify_spec(&input) {
            burndown::SpecDisposition::Skip => {}
            burndown::SpecDisposition::Supervised => {
                titles.insert(disp.clone(), req.title.clone());
                supervised.push(disp);
            }
            burndown::SpecDisposition::Candidate(candidate) => {
                if queued_ids.contains(&req.id) {
                    queued_disp.insert(disp.clone());
                }
                titles.insert(disp.clone(), req.title.clone());
                // trace:TASK-1172 | ai:claude
                priority_by_id.insert(disp.clone(), req.priority.to_string());
                // trace:TASK-1175 | ai:claude
                if let Some(at) = queued_added_at.get(&req.id) {
                    added_at_by_id.insert(disp.clone(), *at);
                }
                candidates.push(candidate);
            }
        }
    }

    // STORY-614: id → serialize:<group> lookup, built from the same scan so the
    // wave-builder can substrate-enforce the serialize convention without a
    // re-read. trace:STORY-614
    let groups_by_id: std::collections::HashMap<String, Vec<String>> = candidates
        .iter()
        .map(|c| (c.id.clone(), burndown::serialize_groups(&c.tags)))
        .collect();

    let (pickable, parked) = burndown::partition(&candidates);
    // STORY-546: split the pickable set by advisor sign-off (queue membership).
    // `ready` = blessed + drainable; `awaiting_signoff` = pickable but the
    // advisor hasn't queued it yet (the `--candidates` curation list).
    let (mut ready, awaiting_signoff) = burndown::split_by_signoff(pickable, &queued_disp);

    // TASK-1172: order the wave BEFORE it is fanned, so a high-priority spec
    // queued after a low-priority one still drains first. Dependencies are
    // unaffected — a blocked spec never entered this set.
    //
    // TASK-1175: both orders are anchored on the queue-entry `added_at`.
    // `[burndown] order = queue` (or `--order queue`) drains strictly oldest-
    // queued first; the default priority order uses that same insertion time as
    // the tiebreak within a band. The sort is TOTAL (final tiebreak = display
    // id), which is why no pre-sort is needed here for the serialize claim
    // below to stay stable across runs.
    //
    // STORY-614: substrate-enforce the `serialize:<group>` convention — the
    // fan-out set must never carry more than one spec per group, so a drain that
    // ignores the skill text still can't co-fan a collision group. The
    // deterministic order above is what makes the "first claims the group" pick
    // reproducible.
    //
    // TASK-149: the held members are their OWN bucket, not part of
    // `awaiting_signoff`. They are queued and blessed — only a sibling's group
    // claim defers them to a later wave — so labelling them "awaiting advisor
    // sign-off" told both the drain runner and the operator that a human gate
    // was missing when nothing was pending.
    // trace:STORY-614 trace:TASK-149 trace:TASK-1172 trace:TASK-1175 | ai:claude
    let ready_order = resolved_burndown_ready_order(&project_root);
    ready = burndown::sort_ready(ready, &priority_by_id, &added_at_by_id, ready_order);

    let (kept_ready, serialize_held) = burndown::collapse_serialize_groups(ready, &groups_by_id);
    ready = kept_ready;
    supervised.sort();
    Ok((
        ready,
        awaiting_signoff,
        serialize_held,
        parked,
        supervised,
        titles,
    ))
}

/// STORY-527 slice 1: resolve a selector to the ready + parked sets via the
/// pure pickability gate. Read-only — the deterministic input the
/// `/aida-burndown` skill fans out.
///
/// STORY-546: the ready set now ALSO requires queue membership — queueing a
/// spec IS the advisor sign-off (`queue add` is advisor-authority-gated, ADR-3
/// / TASK-647), so burndown can never drain a spec the advisor didn't bless.
/// Pickable-but-unqueued specs surface as "awaiting sign-off", and
/// `--candidates` shows exactly that set (the advisor's "what to bless next"
/// aid). trace:STORY-527 trace:STORY-546 | ai:claude
pub(crate) fn handle_burndown_plan(
    status: &str,
    tag: Option<&str>,
    batch: Option<&str>,
    // trace:BUG-784 | ai:claude
    type_filter: Option<&str>,
    candidates_view: bool,
    json: bool,
) -> Result<()> {
    let (ready, awaiting_signoff, serialize_held, parked, supervised, titles) =
        resolve_burndown_sets(status, tag, batch, type_filter)?;

    // trace:BUG-532 — render the spec's (truncated) title beside its id so the
    // plan reads as a scannable decision surface, via the shared
    // `spec_title_cell` helper (also used by `aida human`, BUG-535).
    let title_cell = |id: &str| -> String { spec_title_cell(&titles, id) };

    // TASK-805: the running-drain overlay (live lock + leases) also feeds the
    // JSON payload, so machine consumers polling `--json` see the same
    // in-flight/scheduled partition the human view shows. Resolve the SHARED
    // main-worktree root — the drain writes its lock there (see
    // `handle_burndown_run` → `find_main_worktree_root`), so a sibling worktree
    // must read the orchestrator's lock, not its own. trace:TASK-805
    let overlay = find_main_worktree_root()
        .ok()
        .and_then(|r| DrainOverlay::probe(&r));
    let (in_flight_ready, scheduled_ready) = match &overlay {
        Some(o) => o.partition(&ready),
        None => (Vec::new(), ready.clone()),
    };

    if json {
        let drain = match &overlay {
            Some(o) => serde_json::json!({
                "running": true,
                "pid": o.pid,
                "in_flight": in_flight_ready,
                "scheduled": scheduled_ready,
            }),
            None => serde_json::json!({ "running": false }),
        };
        let payload = serde_json::json!({
            "selector": { "status": status, "tag": tag, "batch": batch },
            "ready": ready,
            "awaiting_signoff": awaiting_signoff,
            // trace:TASK-149 — its own key, NOT folded into awaiting_signoff:
            // these are queued + blessed, deferred to a later wave by a
            // sibling's serialize-group claim. Each entry names the claiming
            // spec + group so a consumer can explain the hold.
            "serialize_held": serialize_held
                .iter()
                .map(|h| serde_json::json!({
                    "spec": h.spec,
                    "held_by": h.held_by,
                    "group": h.group,
                }))
                .collect::<Vec<_>>(),
            "supervised": supervised,
            "parked": parked.iter().map(|(id, reason)| serde_json::json!({ "spec": id, "reason": reason })).collect::<Vec<_>>(),
            "drain": drain,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    // `--candidates`: the advisor's curation view — approved + pickable + NOT
    // yet queued. Read-only; the answer to "what could I bless next?".
    // trace:STORY-546
    if candidates_view {
        println!(
            "{} burndown candidates",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
        );
        println!(
            "  {}",
            burndown::selector_summary(status, tag, batch).dimmed()
        );
        println!(
            "  {} {} pickable + unqueued — bless with `aida queue add <id>` to make them drainable",
            "→".green(),
            awaiting_signoff.len()
        );
        if awaiting_signoff.is_empty() {
            println!(
                "\n{} Nothing to bless — every pickable {} spec is already queued (or none are pickable).",
                "·".dimmed(),
                status
            );
        } else {
            println!(
                "\n{}",
                "Candidates to bless (pickable, not yet queued):".bold()
            );
            for id in &awaiting_signoff {
                println!("  {} {}", "+".yellow(), id.cyan());
            }
        }
        return Ok(());
    }

    // Plain-language header: glosses "selector" so a new user understands what
    // is shown and how to narrow it (no bare jargon). trace:STORY-544
    println!(
        "{} burndown plan",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    println!(
        "  {}",
        burndown::selector_summary(status, tag, batch).dimmed()
    );
    // trace:TASK-149 — serialize-held is counted separately from awaiting
    // sign-off (and only mentioned when non-empty, so the common line is
    // unchanged): nothing human is pending on a held spec.
    let held_summary = if serialize_held.is_empty() {
        String::new()
    } else {
        format!(", {} held for a later wave", serialize_held.len())
    };
    println!(
        "  {} {} ready to fan out, {} awaiting sign-off, {} supervised, {} parked{}",
        "→".green(),
        ready.len(),
        awaiting_signoff.len(),
        supervised.len(),
        parked.len(),
        held_summary
    );

    // TASK-805: if a drain is actively running, surface it as a banner +
    // partition the Ready set into in-flight (leased) vs scheduled (claimed,
    // not yet picked up) so the plan reflects what the drain is mid-way through
    // rather than reading as a fresh, untouched ready set. The overlay +
    // partition were computed above (shared with the JSON path). trace:TASK-805
    if let Some(o) = &overlay {
        println!(
            "\n  {}",
            drain_running_banner(o.pid, &in_flight_ready, &scheduled_ready)
        );
    }

    if !ready.is_empty() {
        println!(
            "\n{}",
            "Ready (queued + bounded + unblocked + decision-free):".bold()
        );
        for id in &ready {
            // TASK-805: when a drain owns this spec, mark its state: in-flight
            // (an implementer is leased on it now) / ◷ scheduled (claimed, not
            // yet picked up). Falls back to the plain check mark when no drain is live.
            let (glyph, suffix) = match &overlay {
                Some(o) if o.in_flight.contains(id) => (
                    crate::glyph(crate::glyphs::Glyph::FlowActive).cyan(),
                    format!("  {}", "in-flight".cyan()),
                ),
                Some(_) => ("◷".yellow(), format!("  {}", "scheduled".yellow())),
                None => (
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    String::new(),
                ),
            };
            println!("  {} {}{}{}", glyph, id.cyan(), title_cell(id), suffix);
        }
    }
    // STORY-546: pickable but not blessed — show them so the advisor knows what
    // they could queue, but they are NOT in the drain set.
    // trace:BUG-499 — wording: queueing IS the advisor sign-off (role-gated via
    // TASK-647), not a casual human bypass; the hint must say so explicitly.
    if !awaiting_signoff.is_empty() {
        println!(
            "\n{}",
            "Awaiting advisor sign-off (mechanical gate passed; the advisor blesses these into the drain with `aida queue add <id>` — advisor authority required):".bold()
        );
        for id in &awaiting_signoff {
            println!("  {} {}{}", "+".yellow(), id, title_cell(id));
        }
    }
    // TASK-149: serialize-held — queued AND blessed, just not this wave. Its own
    // section (never the awaiting-sign-off list) so neither the drain runner nor
    // a reading operator concludes a human gate is missing; each row names the
    // spec that claimed the group. trace:TASK-149 | ai:claude
    if !serialize_held.is_empty() {
        println!(
            "\n{}",
            "Held for a later wave (queued + blessed — one spec per `serialize:<group>` drains at a time; no action needed):"
                .bold()
        );
        for h in &serialize_held {
            println!(
                "  {} {}{} — {}",
                "◷".yellow(),
                h.spec.cyan(),
                title_cell(&h.spec),
                format!("held behind {} in serialize group `{}`", h.held_by, h.group).dimmed()
            );
        }
    }
    // STORY-610: the supervised section — the structural answer to the recurring
    // "how do I make progress on these?" question. These specs are signed off
    // for keyboard pickup but excluded from the unattended drain; the route per
    // spec is `aida queue work <id>` (advisor-watched), NOT `burndown run`.
    if !supervised.is_empty() {
        println!(
            "\n{}",
            "Supervised — work these at the keyboard (excluded from the drain; `aida queue work <id>`):"
                .bold()
        );
        for id in &supervised {
            println!(
                "  {} {}{}",
                crate::glyph(crate::glyphs::Glyph::Arrow).magenta(),
                id.cyan(),
                title_cell(id)
            );
        }
    }
    if !parked.is_empty() {
        println!("\n{}", "Parked:".bold());
        for (id, reason) in &parked {
            // trace:BUG-532 — keep the parked annotation (`reason`, e.g.
            // "tagged `deferred:design`") AND add the title between id and reason.
            println!(
                "  {} {}{} — {}",
                "·".dimmed(),
                id,
                title_cell(id),
                reason.dimmed()
            );
        }
    }

    // Next-step footer (AIDA house style): point the user at the runner —
    // `aida burndown run` (STORY-545), with /aida-burndown as the in-Claude
    // alternative. Suppressed under --json (handled above) and when nothing is
    // ready.
    // trace:STORY-544 trace:BUG-494
    if ready.is_empty() {
        if !awaiting_signoff.is_empty() {
            println!(
                "\n{} Nothing blessed yet — queue a candidate above (`aida queue add <id>`) to make it drainable.",
                "·".dimmed()
            );
        } else if !supervised.is_empty() {
            // STORY-610: nothing to drain, but supervised work IS actionable —
            // route the operator to the keyboard path rather than a dead end.
            println!(
                "\n{} Nothing for the drain, but {} supervised spec(s) above are ready to work at the keyboard — `aida queue work <id>`.",
                "·".dimmed(),
                supervised.len()
            );
        } else {
            println!(
                "\n{} Nothing ready to fan out — adjust the selector above or unblock parked specs.",
                "·".dimmed()
            );
        }
    } else {
        println!("\n{}", burndown::next_step_footer().bold());
    }
    Ok(())
}
