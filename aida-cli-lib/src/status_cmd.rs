//! `aida status` command cluster — `handle_status_spec`,
//! `handle_status_command_distributed`, `handle_status_command` and their
//! status-rendering helpers, extracted from `lib.rs` (SPIKE-78 / STORY-771;
//! pure movement, no behavior change). Shared cache/store helpers stay in
//! `lib.rs`, reached via `crate::`.
// trace:STORY-771 | ai:claude

use crate::*;

const DEFAULT_ABSENCE_DAYS: i64 = 14;

// trace:STORY-976 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
struct AbsenceReport {
    last_activity_at: chrono::DateTime<chrono::Utc>,
    absence_days: i64,
    created_count: usize,
    completed_count: usize,
    rejected_count: usize,
    top_titles: Vec<AbsenceChangeTitle>,
    live_leases: usize,
    stale_leases: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AbsenceChangeTitle {
    spec_id: String,
    title: String,
}

/// `aida status <spec>` — per-spec liveness inspection. Resolves the spec ID
/// in the store, finds its spec-scoped session lease, and classifies liveness
/// (live / stale / flag-only / no-session), demoting alive-but-idle sessions
/// past `idle_minutes` to stale (BUG-623). Renders JSON (`json`) or a human
/// report: status badge, liveness verdict + lease detail, any orchestrator
/// failure reason, and the `Next:` command block.
// trace:TASK-145 | ai:claude
pub(crate) fn handle_status_spec(spec: &str, idle_minutes: u64, json: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;

    let want = spec.trim().to_ascii_uppercase();
    let req = store
        .requirements
        .iter()
        .find(|r| {
            [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                .into_iter()
                .flatten()
                .any(|s| s.eq_ignore_ascii_case(&want))
        })
        .ok_or_else(|| {
            anyhow::anyhow!("no spec found matching `{spec}` — check the ID with `aida list`.")
        })?;

    let disp = req
        .agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| req.id.to_string());
    let status_label = req.status.to_string();
    let in_progress = matches!(req.status, aida_core::RequirementStatus::InProgress);

    // Find the spec-scoped lease (the happy path for AIDA-launched work) and
    // classify its liveness off the SAME machinery `aida session leases` uses.
    let leases = list_leases(&project_root);
    let mut id_owned: Vec<String> = Vec::new();
    if let Some(a) = req.agreed_id.as_deref() {
        id_owned.push(a.to_string());
    }
    if let Some(s) = req.spec_id.as_deref() {
        id_owned.push(s.to_string());
    }
    let id_refs: Vec<&str> = id_owned.iter().map(|s| s.as_str()).collect();
    let lease = spec_scoped_lease(&leases, &id_refs);

    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    let lease_state = lease.map(|l| lease_state_for(l, &live, now));
    let mut verdict = classify_spec_liveness(lease_state, in_progress);

    // BUG-623: an idle backstop on top of pid-liveness. A lease whose process
    // is alive but whose worktree has sat idle past the threshold with no spec
    // movement (no `modified_at` bump) is a hung/abandoned session — demote the
    // `Live` verdict to `Stale`. (TASK-894 was alive-but-idle for ~26h.)
    let elapsed_secs = lease
        .map(|l| now.signed_duration_since(l.started_at).num_seconds().max(0) as u64)
        .unwrap_or(0);
    let idle_secs = now
        .signed_duration_since(req.modified_at)
        .num_seconds()
        .max(0) as u64;
    let idle_threshold_secs = idle_minutes.saturating_mul(60);
    let idle_stalled =
        verdict == SpecLiveness::Live && idle_threshold_secs > 0 && idle_secs > idle_threshold_secs;
    if idle_stalled {
        verdict = SpecLiveness::Stale;
    }

    if json {
        let lease_json = lease.map(|l| {
            serde_json::json!({
                "session_id": l.id,
                "scope": l.scope,
                "role": l.role,
                "worktree": l.worktree_path.display().to_string(),
                "branch": l.branch,
                "started_at": l.started_at.to_rfc3339(),
                "elapsed_secs": elapsed_secs,
            })
        });
        let verdict_key = match verdict {
            SpecLiveness::Live => "live",
            SpecLiveness::Stale => "stale",
            SpecLiveness::FlagOnly => "flag-only",
            SpecLiveness::NoSession => "no-session",
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "spec": disp,
                "status": status_label,
                "status_lens": status_display::needs_attention_lens(req).map(|lens| lens.label()),
                "in_progress": in_progress,
                "liveness": verdict_key,
                "live": verdict == SpecLiveness::Live,
                "idle_secs": idle_secs,
                "idle_stalled": idle_stalled,
                "session": lease_json,
                // STORY-732: inline the orchestrator failure for machine consumers.
                "failure_reason": req.failure_reason.as_ref().map(|fr| serde_json::json!({
                    "phase": fr.phase,
                    "cause": crate::auto_complete_telemetry::failure_cause_label(Some(&fr.kind)),
                    "detail": fr.detail,
                    "hint": fr.recovery_hint,
                })),
            }))?
        );
        return Ok(());
    }

    println!(
        "{} {}  {}",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        disp.cyan().bold(),
        status_display::parked_status_badge(req),
    );
    println!();
    println!("{}", "Liveness".bold());

    let warn = crate::glyph(crate::glyphs::Glyph::Warning);
    match verdict {
        SpecLiveness::Live => {
            let l = lease.expect("Live verdict implies a lease");
            println!(
                "  {} {}",
                "● live".green().bold(),
                "a live process is working this".dimmed()
            );
            print_lease_detail(l, elapsed_secs);
        }
        SpecLiveness::Stale => {
            let l = lease.expect("Stale verdict implies a lease");
            // STORY-732 (FIX 3): a TERMINAL spec (Completed/Rejected) with a
            // dormant lease is a CLEANUP item, not doubt about whether it's done.
            // The old "the In-Progress flag is orphaned" line contradicted the
            // Completed badge `aida why` prints — to a human that reads "is it
            // done or not?". Frame it as housekeeping instead. trace:STORY-732
            let terminal = matches!(
                req.status,
                aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
            );
            let end_hint = short_lease_id(l, &leases);
            if terminal {
                println!(
                    "  {} {} {}",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    status_label.green().bold(),
                    stale_cleanup_suffix(&end_hint).dimmed()
                );
                print_lease_detail(l, elapsed_secs);
            } else {
                let why = if idle_stalled {
                    format!(
                        "idle {} with no spec movement",
                        humanize_duration_secs(idle_secs)
                    )
                } else {
                    "no live process".to_string()
                };
                println!(
                    "{}",
                    format!(
                        "  {warn} {}",
                        stale_orphaned_line(&why, &humanize_duration_secs(elapsed_secs))
                    )
                    .yellow()
                );
                print_lease_detail(l, elapsed_secs);
                println!(
                    "  {} {}",
                    "clear it:".dimmed(),
                    format!("aida session end {end_hint}").cyan()
                );
            }
        }
        SpecLiveness::FlagOnly => {
            println!(
                "{}",
                format!(
                    "  {warn} flag-only — status is In-Progress but no live session is linked to \
                     this spec (the flag is not liveness-backed)"
                )
                .yellow()
            );
            println!(
                "{}",
                "  (advisor Agent-tool fan-outs take generic harness-worktree leases — not \
                 spec-linked — so they correctly read flag-only)"
                    .dimmed()
            );
        }
        SpecLiveness::NoSession => {
            println!(
                "  {} (status: {})",
                "no active session".dimmed(),
                status_label
            );
        }
    }
    // STORY-732 (FIX 2): a shelved (NeedsAttention) spec carries the
    // orchestrator's FailureReason. The liveness block above only says "no active
    // session"; inline WHAT failed (phase + detail + hint) so `aida status <spec>`
    // answers "why is this stuck?" in one command. trace:STORY-732 | ai:claude
    if let Some(fr) = &req.failure_reason {
        println!();
        println!("{}", "Why it's parked".bold());
        for line in failure_reason_lines(fr) {
            println!("  {}", line.magenta());
        }
    }
    // STORY-727: the per-spec next-command block — `aida status <spec>` is a
    // per-spec inspection surface, so it gets the same human `Next:` block as
    // `show` / `why`, leading with `aida zen <id>` for an Approved/Planned spec.
    // The json branch returned above. trace:STORY-727 | ai:claude
    let next = crate::help_next::spec_next(&status_label, &disp);
    if let Some(block) = crate::help_next::render_human(&next) {
        println!("{block}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_status_command_distributed(
    no_dev_context: bool,
    short: bool,
    json: bool,
    queue_only: bool,
    ci_only: bool,
    no_ci: bool,
    cleanup: bool,
    activity: bool,
    activity_since: Option<&str>,
    awaiting: bool,
    verbose: bool,
    no_hygiene: bool,
    all: bool,
    stale: bool,
    full: bool,
    store_path: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    // TASK-1065: the `DatabaseBackend::load` trait method is no longer called on
    // this path — the rich view is built from the cache read-projection via
    // `build_status_store_from_cache`. trace:TASK-1065 | ai:claude

    // STORY-707: the BARE `aida status` (no flags) takes the FAST cache-backed
    // path — sub-second, NO `backend.load()`, NO `gh`/network, NO live-session
    // probe. Every focus/rich flag (`--short`, `--json`, `--queue`, `--ci`,
    // `--cleanup`, `--activity`, `--awaiting`, `--full`, `--all`, `--stale`)
    // routes to the heavy path below, so the opt-in rich view and the existing
    // focus-mode semantics are fully preserved. The heavy diagnostics (PR/CI,
    // liveness, worktrees, roster, coordination, hygiene) moved to `aida doctor`.
    // The `spec` form is dispatched even earlier (before storage init).
    // trace:STORY-707 | ai:claude
    let any_flag = short
        || json
        || queue_only
        || ci_only
        || cleanup
        || activity
        || awaiting
        || full
        || all
        || stale;
    if !any_flag {
        let project_root = std::env::current_dir()?;
        // trace:BUG-1044 | ai:codex
        if let Some(line) = roleless_recovery_line(&project_root) {
            println!("{}", line.yellow().bold());
            println!();
        }
        if let Some(report) = collect_absence_report(&project_root, store_path) {
            print_absence_report(&report);
        }
        // TASK-965: stranded-primary alarm — loud banner ABOVE the snapshot when the
        // primary checkout is parked on a feature branch with in-flight leases. All
        // local reads (git symbolic-ref + lease dir scan), so the fast path's
        // no-network/no-full-load contract holds. trace:TASK-965 | ai:claude
        if let Some(stranded) = detect_stranded_primary(&project_root) {
            print_stranded_primary_banner(&stranded);
        }
        let drain_root = find_main_worktree_root().unwrap_or_else(|_| project_root.clone());
        print_live_drain_status_line(&drain_root);
        let snap = collect_fast_status_snapshot(&project_root);
        // TASK-964: AGENT-MODE renders the token-efficient TOON snapshot; the
        // human TTY path keeps the byte-identical emoji/rule snapshot.
        // STORY-730: each LEADS with the morning-after drain banner when a recent
        // un-acknowledged outcome is persisted. trace:TASK-964 trace:STORY-730
        if agent_output_mode() {
            print_morning_after_toon(&project_root);
            print_toon_status(&snap, &project_root);
        } else {
            print_morning_after_banner(&project_root);
            print_fast_status(&snap);
        }
        return Ok(());
    }

    // TASK-1065: the rich `aida status` view (any flag, incl. `--full`) is built
    // from the CACHE read-projection, NOT a full `backend.load()` over every
    // object YAML. `build_status_store_from_cache` assembles a `RequirementsStore`
    // from `list_summaries` (the same sqlite projection `aida list` reads) plus a
    // single cheap `metadata.yaml` read — finishing the STORY-707 floor: the bare
    // status was already load-free; this takes the `--full` hygiene/cleanup doctor
    // scans, the queue snapshot, and the Project/Requirements sections off the
    // full-store load too. The one section that needs YAML-only data (the pending
    // decision-inbox count) reads the `has_pending_decision` cache column instead.
    // trace:TASK-1065 | ai:claude
    let store = build_status_store_from_cache(backend)?;
    let project_root = std::env::current_dir()?;
    let absence_report = collect_absence_report(&project_root, store_path);

    // BUG-609: `--all` reveals stale agents AND lists every worktree; `--stale`
    // is the narrow form that only reveals the dead-PID agent corpses. Both feed
    // the agent section's `show_stale`; only `--all` expands the worktree list.
    // trace:BUG-609 | ai:claude
    let show_all = all;
    let show_stale_agents = all || stale;

    // STORY-673: terse default + opt-in detail. `--full` (and `--all`, which
    // implies it) expands every long-tail roster the default now folds behind a
    // one-line summary — open-PR roster, recently-merged tail, inferred remote
    // activity, cross-clone coordination, the recent-activity feed, the
    // per-status requirement breakdown, and the AIDA-dev-context block. The
    // default leads with the answer (awaiting / session / branch / PR / queue)
    // and keeps the fleet/orientation long-tails one line each so the
    // orientation command fits roughly one screen — the Trojan-horse principle:
    // quiet depth on demand, not a wall. Reuses the same I/O-cheap memoized
    // probes BUG-613 introduced; the collapse only changes RENDERING, never
    // adds a scan. trace:STORY-673 | ai:claude
    let show_full = all || full;

    if should_print_rich_live_drain_status_line(RichStatusLiveDrainFlags {
        json,
        cleanup,
        activity,
        awaiting,
        short,
        queue_only,
        ci_only,
        show_full,
    }) {
        let drain_root = find_main_worktree_root().unwrap_or_else(|_| project_root.clone());
        print_live_drain_status_line(&drain_root);
    }

    // STORY-385: `--cleanup` focuses on the "Needs attention" section.
    // `--cleanup --json` emits the structured report; otherwise we render
    // the text version and exit, skipping the rest of the status surface.
    // trace:STORY-385 | ai:claude
    if cleanup {
        let report = collect_cleanup_report(&project_root, &store);
        if json {
            println!("{}", serde_json::to_string_pretty(&report.to_json())?);
        } else {
            let stdout = std::io::stdout();
            let _ = report.render(verbose, stdout.lock());
        }
        return Ok(());
    }

    if activity {
        let since = activity_since
            .map(parse_status_activity_since_arg)
            .transpose()?;
        print_status_advisor_activity_full(&project_root, since)?;
        return Ok(());
    }

    // TASK-1055: the rich status view fires several INDEPENDENT gh network
    // probes as it renders — the current-branch PR/CI facts (`gh pr view`),
    // the open-PR roster (`gh pr list --state open`), and the recently-merged
    // tail (`gh pr list --state merged`). Resolved serially section-by-section,
    // a `--full` run pays the SUM of those round-trips (~22s observed on this
    // repo). Each is now behind a process-lifetime memo, so warm all three
    // concurrently up front; the sequential render below then hits warm caches
    // and the wall-clock collapses to the SLOWEST single probe instead of the
    // sum. Output is byte-identical — only the fetch ordering changes, and the
    // warm step is gated on `!no_ci` so `--no-ci` stays network-free.
    // trace:TASK-1055
    if !no_ci {
        warm_status_network_probes(&project_root);
    }

    // TASK-220: gather the unified-view facts once. Each section
    // graceful-degrades on its own data — missing PR or missing session
    // is not an error, it's just an absent section.
    let user_ctx = collect_user_context(&project_root, &store, backend, no_ci);

    // STORY-465: `--awaiting` focuses on the human-gate report. Same
    // contract as `--cleanup`: `--json` emits the structured report,
    // otherwise render text and exit. trace:STORY-465 | ai:claude
    if awaiting {
        let report = collect_awaiting_report(&project_root, backend, &user_ctx, no_ci);
        if json {
            println!("{}", serde_json::to_string_pretty(&report.to_json())?);
        } else if report.is_empty() {
            // Echo a quiet all-clear so `aida status --awaiting` doesn't
            // silently exit with nothing on stdout — the focus-mode user
            // explicitly asked.
            println!("{}", "─── Awaiting you (0) ───".bold().dimmed());
            println!("  Nothing awaits you right now.");
            println!();
        } else {
            let stdout = std::io::stdout();
            let _ = report.render(verbose, stdout.lock());
        }
        return Ok(());
    }

    if json {
        let awaiting_report = collect_awaiting_report(&project_root, backend, &user_ctx, no_ci);
        // TASK-662: persist the findings-delta snapshot against the main
        // worktree root so the baseline is consistent regardless of cwd —
        // same root the text path's `last-status.toml` uses. trace:TASK-662
        let delta_root = find_main_worktree_root().unwrap_or_else(|_| project_root.clone());
        return print_status_json(
            &user_ctx,
            backend,
            store_path,
            &delta_root,
            queue_only,
            ci_only,
            &awaiting_report,
            absence_report.as_ref().map(absence_report_json),
        );
    }

    if short {
        print_status_short(&user_ctx);
        return Ok(());
    }

    if queue_only {
        print_status_queue_section(&user_ctx, true);
        return Ok(());
    }

    if ci_only {
        print_status_pr_section(&user_ctx, true);
        return Ok(());
    }

    // Default: print every section. The "Awaiting you" report leads —
    // when the operator IS the bottleneck, nothing else matters until
    // they've cleared their gates. Hidden on quiet days so the section
    // appearing is itself the signal. trace:STORY-465 | ai:claude
    let awaiting_report = collect_awaiting_report(&project_root, backend, &user_ctx, no_ci);
    if let Some(report) = absence_report.as_ref() {
        print_absence_report(report);
    }
    let stdout = std::io::stdout();
    let _ = awaiting_report.render(verbose, stdout.lock());

    print_status_presence_line(&project_root);
    print_status_presence_consumers(&project_root, backend);
    print_status_session_section(&user_ctx);
    print_status_branch_section(&user_ctx);
    if !no_ci {
        print_status_pr_section(&user_ctx, false);
    }
    print_status_queue_section(&user_ctx, false);

    // TASK-648 (ADR-3): surface the draft-inbox depth. Drafts are untriaged
    // intake awaiting an advisor disposition (keep → queue / backlog → archive
    // / unclear → needs-attention). Shown only when non-empty so an empty
    // inbox stays quiet, and reads like a queue to clear. trace:TASK-648
    // BUG-464: exclude META (and other non-triageable seed rows) the same way
    // `aida list` hides META by default — otherwise a fresh `aida init` reports
    // its 6 seeded META prompts as "6 untriaged drafts", sending a brand-new
    // user to /aida-triage for AI-prompt templates that aren't real intake.
    // BUG-593: widened from a meta-only check to the shared standing-artifact
    // helper so this count and the advisor dashboard's draft count derive the
    // groomable-draft set from a SINGLE source of truth and can never disagree.
    // trace:BUG-464 trace:BUG-593 | ai:claude
    let draft_inbox = backend
        .list_summaries(&aida_core::ListFilter {
            status: Some("draft".to_string()),
            ..Default::default()
        })
        .map(|v| {
            v.iter()
                .filter(|r| !is_standing_artifact_type(&r.req_type))
                .count()
        })
        .unwrap_or(0);
    if draft_inbox > 0 {
        println!(
            "  {} {} untriaged draft{} — clear with {}",
            "Inbox:".bold().yellow(),
            draft_inbox,
            if draft_inbox == 1 { "" } else { "s" },
            "/aida-triage".cyan()
        );
        println!();
    }

    // TASK-502: surface urgent (notify'd) agent briefs from the `.pending`
    // sentinels, so an idle agent's `aida status` flags work routed to it
    // without a heartbeat. Filtered to the current agent when identifiable,
    // else all agents. trace:TASK-502 | ai:claude
    let pending_briefs = collect_pending_brief_counts(&project_root);
    if !pending_briefs.is_empty() {
        for (agent, count) in &pending_briefs {
            println!(
                "  📬 {} pending brief{} for {} — run {}",
                count,
                if *count == 1 { "" } else { "s" },
                agent.cyan(),
                format!("aida brief list --for-agent {agent}").cyan()
            );
        }
        println!();
    }

    // TASK-294: a one-line pending-directive summary so the worker control
    // channel has the same visibility as the work queue. Silent when the
    // directive file is empty/absent.
    let directives = worker::parse_directives(&worker::worker_cmd_path(&project_root));
    if let Some(line) = worker::status_line(&directives) {
        println!("  {}", line);
        println!();
    }

    print_status_agents_section(&user_ctx, show_stale_agents);
    print_status_claude_code_section(&project_root);

    // STORY-456: unified worktrees + open-PRs + recently-merged panes —
    // git worktree + session lease + liveness + commits-ahead + open PR in
    // one view, then the open-PR queue and a recently-merged tail for
    // orientation. All display-only; each silent when empty. The PR data is a
    // single batched gh snapshot per section, not a call per row.
    // trace:STORY-456 | ai:claude
    print_status_worktrees_section(&project_root, show_all);
    // STORY-673: the open-PR roster, recently-merged tail, inferred remote
    // activity, and cross-clone coordination are orientation long-tails — folded
    // to a one-line count by default, expanded with `--full` / `--all`.
    // trace:STORY-673 | ai:claude
    print_status_open_prs_section(&project_root, show_full);
    print_status_recently_merged_section(&project_root, 5, show_full);

    // STORY-452: inferred cloud / cross-machine agent activity from commit
    // trailers on lease-less remote branches — local agents already appear in
    // the "Active agents" section. Read-only; silent when no remote signal.
    // trace:STORY-452 | ai:claude
    print_status_remote_activity_section(&project_root, 5, show_full);

    // STORY-640 (coordination slice 3): active cross-clone `coordination/`
    // claims (leases + drain + solo) held across all clones — who holds what,
    // where. Distinct from the LOCAL leases above; silent when no claims exist.
    // trace:STORY-640 | ai:claude
    print_status_coordination_section(store_path, chrono::Utc::now(), show_full);

    // STORY-405: live state-affecting advisor/external activity recorded by
    // AIDA verbs such as `aida pr ship`. Read-only and silent when absent or
    // older than the default window.
    // trace:STORY-405 | ai:codex
    print_status_advisor_activity_footer(&project_root)?;

    // TASK-539: pending-findings backlog — silent when empty. Surfaced before
    // the working-tree section so triage-able items are visible without
    // remembering `aida findings list`. trace:TASK-539 | ai:claude
    print_status_findings_section(backend);

    // STORY-457: working-tree state — modified / staged / untracked (with
    // safe-to-remove cruft auto-flagged). Display-only; silent when clean.
    // Reads the main worktree so it's consistent regardless of cwd.
    // trace:STORY-457 | ai:claude
    let wt_root = find_main_worktree_root().unwrap_or_else(|_| project_root.clone());
    print_status_working_tree_section(&wt_root);

    // STORY-49: warn when the code HEAD's paired store SHA (`Aida-Store:`
    // trailer) has drifted from the current orphan-store HEAD. Warn-only —
    // the cheap, high-signal half of EPIC-21 v2; the full read-only
    // `aida store checkout` time-travel is deferred. Silent unless there's an
    // unambiguous drift signal. trace:STORY-49 | ai:claude
    print_status_store_drift_section(&wt_root);

    // STORY-410: one-line substrate-drift notice. The opt-in memory pack
    // grows inside the aida binary; a project that scaffolded it months ago
    // has no other signal that it's behind. Only shown when the pack EXISTS
    // locally and is behind — a project that never opted into `--with-memories`
    // stays quiet (no nag to adopt a feature they declined). trace:STORY-410
    print_status_memory_drift_section();

    println!("{}", "─── Project ───".bold());
    let name = if store.name.is_empty() {
        "(unnamed)"
    } else {
        &store.name
    };
    println!("  Name:         {}", name.white().bold());
    println!(
        "  Mode:         {} (orphan branch)",
        "distributed git-canonical".cyan()
    );
    println!("  Store path:   {}", store_path.display());
    println!();

    // Requirement counts grouped by status. Archived rows are included so
    // the project total reflects all on-disk specs, but META rows (AI-prompt
    // customization seeded by `aida init`) are excluded so the Requirements
    // panel agrees with `aida list`. `aida list` hides META by default
    // (BUG-27); counting them here made `aida status` report a phantom
    // "Total 7 / Draft 6" on a fresh store (1 real TASK + 6 Draft META) while
    // `aida list --all` showed only the single real requirement, eroding
    // trust in the counts. trace:BUG-415 | ai:claude
    let summaries = backend.list_summaries(&aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        ..Default::default()
    })?;
    let real: Vec<&aida_core::db::RequirementSummary> = summaries
        .iter()
        .filter(|s| is_real_requirement_summary(&s.req_type))
        .collect();
    let total = real.len();
    let mut by_status: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for s in &real {
        *by_status.entry(s.status.clone()).or_insert(0) += 1;
    }
    println!("{}", "─── Requirements ───".bold());
    println!("  Total:        {}", total);
    if show_full {
        // STORY-673: the full per-status breakdown is detail — one line per
        // status. trace:STORY-673 | ai:claude
        for (status, count) in &by_status {
            println!("    {:<14} {}", status, count);
        }
    } else {
        // STORY-673: terse default — one compact line leading with OPEN work
        // (what's actionable), then the closed tallies, then a pointer to the
        // full breakdown. trace:STORY-673 | ai:claude
        println!("  {}", requirement_breakdown_summary_line(&by_status));
    }
    println!();

    // Cache state. This row is a cache-freshness integrity check, so it
    // compares the cache's raw row count against the store's raw row count —
    // both include META rows, otherwise a healthy cache would falsely read
    // "Rows: 7 (store has 1)". The META exclusion above is a display concern
    // for the Requirements panel only. trace:BUG-415 | ai:claude
    let cache = backend.cache();
    let cached = cache.requirement_count()?;
    let store_rows = summaries.len();
    let recorded_sha = cache.source_head_sha()?.unwrap_or_default();
    let actual_sha = aida_core::git_ops::head_sha(store_path).unwrap_or_default();
    let stale = recorded_sha != actual_sha || recorded_sha.is_empty();
    println!("{}", "─── Cache ───".bold());
    println!("  Path:         {}", cache.path().display());
    println!("  Rows:         {} (store has {})", cached, store_rows);
    println!(
        "  Status:       {}",
        if stale && !actual_sha.is_empty() {
            format!("{} — run `aida cache rebuild`", "STALE".yellow())
        } else {
            "FRESH".green().to_string()
        }
    );
    println!();

    // Sync state — orphan-branch ahead/behind origin/aida-store.
    if let Some((ahead, behind)) = orphan_branch_sync_state(store_path) {
        println!("{}", "─── Sync ───".bold());
        match (ahead, behind) {
            (0, 0) => println!("  Branch aida-store: in sync with origin"),
            (a, 0) => println!(
                "  Branch aida-store: {} ahead of origin (run `aida push`)",
                a.to_string().yellow()
            ),
            (0, b) => println!(
                "  Branch aida-store: {} behind origin (run `aida db sync --pull`)",
                b.to_string().yellow()
            ),
            (a, b) => println!(
                "  Branch aida-store: {} ahead, {} behind (diverged — `aida db sync --pull` then `aida push`)",
                a.to_string().red(),
                b.to_string().red()
            ),
        }
        println!();
    }

    // Recent activity — top 5 most recently modified user-authored reqs.
    // META rows (AI prompt customization seeded by init) are excluded so a
    // brand-new project doesn't show "Recent activity: META-001..006" as
    // its entire feed. trace:BUG-30 | ai:claude
    let mut recent: Vec<_> = summaries
        .iter()
        .filter(|r| !r.req_type.eq_ignore_ascii_case("meta"))
        .cloned()
        .collect();
    recent.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
    // STORY-673: the recent-activity feed is orientation detail. The terse
    // default shows only the single most-recent row plus a "+N more" pointer;
    // `--full` / `--all` shows the top-5 feed. trace:STORY-673 | ai:claude
    let recent_shown = if show_full { 5 } else { 1 };
    println!("{}", "─── Recent activity ───".bold());
    for r in recent.iter().take(recent_shown) {
        let id = r
            .agreed_id
            .as_deref()
            .or(r.spec_id.as_deref())
            .unwrap_or("?");
        let modified = r.modified_at.split('T').next().unwrap_or(&r.modified_at);
        // TASK-269: unified status palette — pad-then-colour for the column.
        // trace:TASK-269 | ai:claude
        let status_label = r.status.to_string();
        let status_cell =
            status_display::paint_status(&format!("{:<12}", status_label), &status_label);
        println!("  {:<14} {} {} — {}", id, status_cell, modified, r.title);
    }
    if recent.is_empty() {
        println!("  (no user requirements yet — try `aida add --type vision --title \"...\"`)");
    } else if !show_full && recent.len() > recent_shown {
        // STORY-673: pointer to the full feed. The default shows only the
        // single newest row; `--full` shows the top-5 feed (NOT every spec —
        // so we don't headline the whole store's size here). trace:STORY-673
        println!(
            "  {}",
            "… more recent activity — `aida status --full`".dimmed()
        );
    }
    println!();

    // Scaffolding freshness — only useful for non-AIDA-self projects, since
    // AIDA's own .claude/ uses symlinks into aida-core/templates/ and can't
    // drift. The aida-self block below has its own template-symlink check.
    if !is_aida_repo(&project_root) {
        print_scaffolding_freshness(&project_root, &store, store_path);
    }

    // AIDA-self developer context — only when this project IS the aida repo.
    // STORY-673: this is a heavy dev-only block (and `cross_platform_ci_status`
    // makes a network call) — folded to a one-line summary by default,
    // expanded with `--full` / `--all`. Folding it away is also a perf win on
    // the default path (no CI network probe). trace:STORY-673 | ai:claude
    if !no_dev_context && is_aida_repo(&project_root) {
        if show_full {
            print_aida_dev_context(&project_root);
        } else {
            print_aida_dev_context_summary(&project_root);
        }
    }

    // STORY-464: passive doctor scan integrated into `aida status` under `Hygiene` section
    // trace:STORY-464 | ai:antigravity
    print_status_hygiene_section(&project_root, &store, verbose, no_hygiene)?;

    // STORY-385: one-line summary at the bottom of default `aida status`
    // when the cleanup report is non-empty — silent otherwise. Points the
    // operator at `--cleanup` for details.
    // trace:STORY-385 | ai:claude
    let cleanup_report = collect_cleanup_report(&project_root, &store);
    if let Some(line) = cleanup_report.summary_line() {
        println!("  {}", line);
        println!();
    }

    Ok(())
}

/// Surface an in-flight drain at the top of `aida status` using the same
/// local-only liveness probe as `statusline` and `statusbar`. Stale/no drain
/// renders nothing, preserving existing bytes on quiet projects.
// trace:TASK-1194 | ai:codex
fn print_live_drain_status_line(project_root: &std::path::Path) {
    if let Some(probe) = crate::drain_state::drain_liveness_probe(project_root) {
        println!("{}", probe.status_line().cyan().bold());
        println!();
    }
}

#[derive(Debug, Clone, Copy)]
struct RichStatusLiveDrainFlags {
    json: bool,
    cleanup: bool,
    activity: bool,
    awaiting: bool,
    short: bool,
    queue_only: bool,
    ci_only: bool,
    show_full: bool,
}

/// The rich status banner belongs only to the human full-status surface.
/// Focus modes keep their previous bytes, and JSON must remain byte-valid.
// trace:TASK-1194 | ai:codex
fn should_print_rich_live_drain_status_line(flags: RichStatusLiveDrainFlags) -> bool {
    flags.show_full
        && !flags.json
        && !flags.cleanup
        && !flags.activity
        && !flags.awaiting
        && !flags.short
        && !flags.queue_only
        && !flags.ci_only
}

fn absence_threshold_days(project_root: &std::path::Path) -> i64 {
    if let Ok(raw) = std::env::var("AIDA_ABSENCE_DAYS") {
        if let Ok(days) = raw.trim().parse::<i64>() {
            return days.max(0);
        }
    }
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(path) else {
        return DEFAULT_ABSENCE_DAYS;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return DEFAULT_ABSENCE_DAYS;
    };
    value
        .get("status")
        .and_then(|s| s.get("absence_days"))
        .and_then(|v| v.as_integer())
        .map(|v| v.max(0))
        .unwrap_or(DEFAULT_ABSENCE_DAYS)
}

fn collect_absence_report(
    project_root: &std::path::Path,
    store_path: &std::path::Path,
) -> Option<AbsenceReport> {
    let last_activity_at = newest_activity_at(project_root, store_path)?;
    let now = chrono::Utc::now();
    let absence_days = now
        .signed_duration_since(last_activity_at)
        .num_days()
        .max(0);
    if absence_days < absence_threshold_days(project_root) {
        return None;
    }

    let digest = collect_absence_change_digest(store_path, last_activity_at);
    let (live_leases, stale_leases) = count_live_and_stale_leases(project_root);
    Some(AbsenceReport {
        last_activity_at,
        absence_days,
        created_count: digest.created_count,
        completed_count: digest.completed_count,
        rejected_count: digest.rejected_count,
        top_titles: digest.top_titles,
        live_leases,
        stale_leases,
    })
}

fn newest_activity_at(
    project_root: &std::path::Path,
    store_path: &std::path::Path,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let mut newest = newest_store_commit_at(store_path);
    newest = newest.max(newest_event_stream_at(project_root));
    newest = newest.max(newest_usage_at(project_root));
    newest
}

fn newest_store_commit_at(store_path: &std::path::Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(store_path)
        .args(["log", "-1", "--pretty=format:%aI"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_rfc3339_utc(String::from_utf8_lossy(&out.stdout).trim())
}

fn newest_event_stream_at(project_root: &std::path::Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let body = std::fs::read_to_string(events::events_path(project_root)).ok()?;
    body.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<events::Event>(line).ok())
        .map(|event| event.ts)
        .next()
}

fn newest_usage_at(project_root: &std::path::Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let scopes = known_project_scopes(project_root);
    usage::read_events()
        .into_iter()
        .filter(|event| {
            event
                .scope
                .as_deref()
                .map(|scope| scopes.iter().any(|known| known.eq_ignore_ascii_case(scope)))
                .unwrap_or(true)
        })
        .filter_map(|event| parse_rfc3339_utc(&event.ts))
        .max()
}

fn known_project_scopes(project_root: &std::path::Path) -> std::collections::HashSet<String> {
    let mut scopes = std::collections::HashSet::new();
    if let Some(name) = project_root.file_name().and_then(|n| n.to_str()) {
        scopes.insert(name.to_string());
    }
    if let Ok(branch) = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["branch", "--show-current"])
        .output()
    {
        if branch.status.success() {
            let name = String::from_utf8_lossy(&branch.stdout).trim().to_string();
            if !name.is_empty() {
                scopes.insert(name);
            }
        }
    }
    scopes
}

fn parse_rfc3339_utc(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

#[derive(Default)]
struct AbsenceChangeDigest {
    created_count: usize,
    completed_count: usize,
    rejected_count: usize,
    top_titles: Vec<AbsenceChangeTitle>,
}

fn collect_absence_change_digest(
    store_path: &std::path::Path,
    since: chrono::DateTime<chrono::Utc>,
) -> AbsenceChangeDigest {
    let opts = history::HistoryOpts {
        limit: 250,
        max_commits: 2_000,
        events_mode: true,
        id_filter: None,
        type_filter: None,
        author_filter: None,
        since: Some(since.to_rfc3339()),
        until: None,
        status_changes_only: false,
        shipped_only: false,
        comments_only: false,
        oneline: false,
        archived_specs: std::collections::HashSet::new(),
        archived_only_specs: None,
        deferred_specs: std::collections::HashSet::new(),
        deferred_only_specs: None,
        exclude_meta: true,
    };
    let Ok(events) = history::collect_event_records(store_path, &opts) else {
        return AbsenceChangeDigest::default();
    };
    summarize_absence_events(&events)
}

fn summarize_absence_events(events: &[history::HistoryEventRecord]) -> AbsenceChangeDigest {
    let mut digest = AbsenceChangeDigest::default();
    let mut seen_titles = std::collections::HashSet::new();
    for event in events {
        match event.kind.as_str() {
            "added" => digest.created_count += 1,
            "status_change" => {
                let to = event
                    .detail
                    .get("to")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if to.eq_ignore_ascii_case("Completed") {
                    digest.completed_count += 1;
                } else if to.eq_ignore_ascii_case("Rejected") {
                    digest.rejected_count += 1;
                }
            }
            _ => {}
        }
        if digest.top_titles.len() < 5 && seen_titles.insert(event.spec_id.clone()) {
            let title = event
                .detail
                .get("title")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| event.summary.clone());
            digest.top_titles.push(AbsenceChangeTitle {
                spec_id: event.spec_id.clone(),
                title,
            });
        }
    }
    digest
}

fn count_live_and_stale_leases(project_root: &std::path::Path) -> (usize, usize) {
    let leases = list_leases(project_root);
    let live = process_probe::probe_live_claude_sessions();
    let now = chrono::Utc::now();
    let mut live_count = 0;
    let mut stale_count = 0;
    for lease in &leases {
        match lease_state_for(lease, &live, now) {
            LeaseState::Live | LeaseState::Dormant => live_count += 1,
            LeaseState::Stale => stale_count += 1,
        }
    }
    (live_count, stale_count)
}

fn print_absence_report(report: &AbsenceReport) {
    let local = report.last_activity_at.with_timezone(&chrono::Local);
    println!("{}", "─── Welcome back ───".bold());
    println!(
        "  Last activity: {}, {} day{} ago",
        local.format("%A %Y-%m-%d %H:%M %Z"),
        report.absence_days,
        if report.absence_days == 1 { "" } else { "s" }
    );
    println!(
        "  Changed since: {} created · {} completed · {} rejected",
        report.created_count.to_string().cyan(),
        report.completed_count.to_string().green(),
        report.rejected_count.to_string().red()
    );
    for item in &report.top_titles {
        println!("    {} {}", item.spec_id.bold(), item.title);
    }
    println!(
        "  Leases: {} live · {} stale",
        report.live_leases.to_string().green(),
        report.stale_leases.to_string().yellow()
    );
    println!();
}

fn absence_report_json(report: &AbsenceReport) -> serde_json::Value {
    serde_json::json!({
        "absence_days": report.absence_days,
        "last_activity_at": report.last_activity_at.to_rfc3339(),
        "created_count": report.created_count,
        "completed_count": report.completed_count,
        "rejected_count": report.rejected_count,
        "top_titles": report.top_titles.iter().map(|item| {
            serde_json::json!({
                "spec_id": item.spec_id,
                "title": item.title,
            })
        }).collect::<Vec<_>>(),
        "live_leases": report.live_leases,
        "stale_leases": report.stale_leases,
    })
}

pub(crate) fn absence_notice_line(
    project_root: &std::path::Path,
    store_path: &std::path::Path,
) -> Option<String> {
    let report = collect_absence_report(project_root, store_path)?;
    Some(format!(
        "Welcome back: back after {} day{}, run aida status.",
        report.absence_days,
        if report.absence_days == 1 { "" } else { "s" }
    ))
}

#[cfg(test)]
mod task_1194_live_drain_status_tests {
    use super::{should_print_rich_live_drain_status_line, RichStatusLiveDrainFlags};

    fn flags() -> RichStatusLiveDrainFlags {
        RichStatusLiveDrainFlags {
            json: false,
            cleanup: false,
            activity: false,
            awaiting: false,
            short: false,
            queue_only: false,
            ci_only: false,
            show_full: true,
        }
    }

    #[test]
    fn full_human_status_prints_live_drain_line() {
        assert!(should_print_rich_live_drain_status_line(flags()));
    }

    #[test]
    fn json_status_modes_do_not_print_live_drain_line() {
        let mut plain_json = flags();
        plain_json.json = true;
        assert!(!should_print_rich_live_drain_status_line(plain_json));

        let mut cleanup_json = plain_json;
        cleanup_json.cleanup = true;
        assert!(!should_print_rich_live_drain_status_line(cleanup_json));

        let mut awaiting_json = plain_json;
        awaiting_json.awaiting = true;
        assert!(!should_print_rich_live_drain_status_line(awaiting_json));
    }

    #[test]
    fn focused_human_status_modes_keep_existing_bytes() {
        let mut cleanup = flags();
        cleanup.cleanup = true;
        assert!(!should_print_rich_live_drain_status_line(cleanup));

        let mut awaiting = flags();
        awaiting.awaiting = true;
        assert!(!should_print_rich_live_drain_status_line(awaiting));

        let mut short = flags();
        short.short = true;
        assert!(!should_print_rich_live_drain_status_line(short));
    }
}

#[cfg(test)]
mod story_976_absence_tests {
    use super::*;
    use tempfile::tempdir;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    fn commit_backdated_store(
        store: &std::path::Path,
        spec: &str,
        title: &str,
        when: chrono::DateTime<chrono::Utc>,
    ) {
        std::fs::create_dir_all(store.join("objects").join("STORY")).unwrap();
        std::fs::write(
            store.join("objects").join("STORY").join(format!("{spec}.yaml")),
            format!(
                "id: 019e71f4-0000-7000-8000-000000000000\nspec_id: {spec}\ntitle: {title}\ndescription: fixture\nstatus: Draft\npriority: Low\nreq_type: Story\ncreated_at: {when}\nmodified_at: {when}\n"
            ),
        )
        .unwrap();
        git(store, &["init", "-q"]);
        git(store, &["config", "user.email", "test@example.com"]);
        git(store, &["config", "user.name", "Test User"]);
        git(store, &["add", "."]);
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(store)
            .args(["commit", "-q", "-m", &format!("add {spec} — {title}")])
            .env("GIT_AUTHOR_DATE", when.to_rfc3339())
            .env("GIT_COMMITTER_DATE", when.to_rfc3339())
            .status()
            .unwrap();
        assert!(status.success(), "backdated git commit failed");
    }

    #[test]
    fn synthetic_store_thirty_days_old_triggers_absence_report() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        let aida_dir = project.path().join(".aida");
        std::fs::create_dir_all(&aida_dir).unwrap();
        std::fs::write(
            aida_dir.join("config.toml"),
            "[status]\nabsence_days = 14\n",
        )
        .unwrap();
        let store = project.path().join(".aida-store");
        let old = chrono::Utc::now() - chrono::Duration::days(30);
        commit_backdated_store(&store, "STORY-976", "welcome back digest", old);

        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("HOME", home.path().to_str().unwrap()),
            ("AIDA_ABSENCE_DAYS", "14"),
        ]);
        let report =
            collect_absence_report(project.path(), &store).expect("absence over threshold");

        assert!(report.absence_days >= 29, "{report:?}");
        assert_eq!(report.created_count, 1);
        assert_eq!(report.completed_count, 0);
        assert_eq!(report.rejected_count, 0);
        assert_eq!(report.top_titles[0].spec_id, "STORY-976");
        assert_eq!(report.top_titles[0].title, "welcome back digest");
    }

    #[test]
    fn below_threshold_stays_silent() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".aida")).unwrap();
        let store = project.path().join(".aida-store");
        let recent = chrono::Utc::now() - chrono::Duration::days(2);
        commit_backdated_store(&store, "STORY-977", "recent activity", recent);

        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("HOME", home.path().to_str().unwrap()),
            ("AIDA_ABSENCE_DAYS", "14"),
        ]);
        assert!(
            collect_absence_report(project.path(), &store).is_none(),
            "below-threshold status must preserve existing bytes"
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdvisorActivityEvent {
    ts: chrono::DateTime<chrono::Utc>,
    command: String,
    step: String,
    status: String,
    pr: Option<u64>,
}

impl AdvisorActivityEvent {
    fn target_label(&self) -> String {
        self.pr
            .map(|n| format!("PR #{n}"))
            .unwrap_or_else(|| "-".to_string())
    }
}

pub(crate) fn advisor_activity_path(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida").join("advisor-activity.jsonl")
}

pub(crate) fn parse_advisor_activity_line(line: &str) -> Option<AdvisorActivityEvent> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let ts_raw = value.get("ts")?.as_str()?;
    let ts = chrono::DateTime::parse_from_rfc3339(ts_raw)
        .ok()?
        .with_timezone(&chrono::Utc);
    let command = value
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("aida")
        .to_string();
    let step = value
        .get("step")
        .and_then(|v| v.as_str())
        .unwrap_or("activity")
        .to_string();
    let status = value
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let pr = value.get("pr").and_then(|v| v.as_u64());
    Some(AdvisorActivityEvent {
        ts,
        command,
        step,
        status,
        pr,
    })
}

pub(crate) fn read_advisor_activity_events(
    project_root: &std::path::Path,
    since: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<AdvisorActivityEvent> {
    let path = advisor_activity_path(project_root);
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut events: Vec<_> = body
        .lines()
        .filter_map(parse_advisor_activity_line)
        .filter(|e| since.map(|cutoff| e.ts >= cutoff).unwrap_or(true))
        .collect();
    events.sort_by(|a, b| b.ts.cmp(&a.ts));
    events
}

pub(crate) fn parse_status_activity_since_arg(raw: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    parse_since_arg(raw)
}

pub(crate) fn print_status_advisor_activity_full(
    project_root: &std::path::Path,
    since: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<()> {
    let events = read_advisor_activity_events(project_root, since);
    println!("{}", "─── Recent advisor activity ───".bold());
    if events.is_empty() {
        println!("  (no recorded advisor activity)");
        println!();
        return Ok(());
    }
    for event in events {
        println!(
            "  {}  {:<12} {:<8} {:<8} {}",
            event.ts.to_rfc3339(),
            event.step,
            event.status,
            event.target_label(),
            event.command.dimmed()
        );
    }
    println!();
    Ok(())
}

pub(crate) fn print_status_advisor_activity_footer(project_root: &std::path::Path) -> Result<()> {
    let since = chrono::Utc::now() - chrono::Duration::minutes(30);
    let events = read_advisor_activity_events(project_root, Some(since));
    if events.is_empty() {
        return Ok(());
    }
    println!("{}", "─── Recent advisor activity ───".bold());
    for event in events.iter().take(5) {
        println!(
            "  {:<12} {:<8} {} — {}",
            event.step,
            event.status,
            event.target_label(),
            event.ts.to_rfc3339()
        );
    }
    if events.len() > 5 {
        println!(
            "  {}",
            format!("… {} more; run `aida status --activity`", events.len() - 5).dimmed()
        );
    }
    println!();
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_405_advisor_activity_tests.rs"]
mod story_405_advisor_activity_tests;

/// Compare a project's `.claude/skills/`, `.claude/commands/`, `.claude/hooks/`
/// (and CLAUDE.md / AGENTS.md / .mcp.json) against the templates embedded in
/// the running aida binary. Reports counts of files that match exactly vs
/// files that have drifted, and suggests `aida scaffold apply --force` if
/// there's drift. Quiet when the project has no scaffolding at all.
/// trace:EPIC-1-001 | ai:claude
pub(crate) fn print_scaffolding_freshness(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
    db_path: &std::path::Path,
) {
    use aida_core::scaffolding::{ScaffoldConfig, Scaffolder};

    // BUG-43: drive the scaffolder with the *actual* store and the
    // *actual* db_path, matching how init/scaffold-apply construct the
    // scaffolder. AIDA.md bakes both store-derived data (req count) and
    // db_path-derived data (`database_filename()`) into its content, so
    // any mismatch on either input falsely reports drift on a fresh
    // init. trace:BUG-43 | ai:claude
    let config = ScaffoldConfig::default();
    let mut scaffolder =
        Scaffolder::with_database(project_root.to_path_buf(), config, db_path.to_path_buf());
    let preview = scaffolder.preview(store);

    use aida_core::scaffolding::FileCategory;

    let mut total = 0usize;
    let mut present = 0usize;
    let mut matches = 0usize;
    // BUG-42: split drift by file category. Template-category drift is a
    // problem (AIDA-owned files shouldn't differ from embedded). Seed-
    // category drift is *expected* once the user customizes CLAUDE.md /
    // AGENTS.md — it's not really drift, it's their project. Reporting
    // them under the same STALE banner trains users to ignore the warning.
    // ManagedMerge sits with seed for now (user-owned post-init in v1).
    let mut template_drift: Vec<std::path::PathBuf> = Vec::new();
    let mut seed_drift: Vec<std::path::PathBuf> = Vec::new();

    for artifact in &preview.artifacts {
        total += 1;
        let full = project_root.join(&artifact.path);
        if !full.exists() {
            // missing; not "drifted" (probably user opted out via --no-skills)
            continue;
        }
        present += 1;
        let on_disk = match std::fs::read(&full) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if on_disk == artifact.content.as_bytes() {
            matches += 1;
            continue;
        }
        // .claude/AIDA.md's `## Claude Code skills` section is gated by
        // generate_skills, but this drift check always regenerates with the
        // default (skills-on) config — it has no record of an `aida init
        // --no-skills`. A raw byte compare would therefore flag a clean
        // --no-skills init as STALE-on-arrival. Use the section-tolerant
        // matcher for AIDA.md instead. trace:TASK-125 | ai:claude
        if artifact.path.file_name().and_then(|s| s.to_str()) == Some("AIDA.md") {
            let on_disk_str = String::from_utf8_lossy(&on_disk);
            if aida_core::scaffolding::aida_md_matches(&on_disk_str, &artifact.content) {
                matches += 1;
                continue;
            }
        }
        match FileCategory::from_path(&artifact.path) {
            FileCategory::Template => template_drift.push(artifact.path.clone()),
            FileCategory::Seed | FileCategory::ManagedMerge => {
                seed_drift.push(artifact.path.clone());
            }
        }
    }

    // No scaffolding present at all — stay quiet (probably a non-aida project
    // that just happens to have a .aida/config.toml from somewhere unrelated).
    if present == 0 {
        return;
    }

    println!("{}", "─── Scaffolding ───".bold());
    println!(
        "  Templates compared: {} total, {} present in project",
        total, present
    );
    if template_drift.is_empty() {
        println!(
            "  Status:             {} — all {} AIDA-owned file(s) match the embedded templates",
            "FRESH".green(),
            matches + seed_drift.len()
        );
    } else {
        println!(
            "  Status:             {} — {} AIDA-owned file(s) differ from the embedded templates",
            "STALE".yellow(),
            template_drift.len()
        );
        for path in template_drift.iter().take(5) {
            println!("    - {}", path.display());
        }
        if template_drift.len() > 5 {
            println!("    ... and {} more", template_drift.len() - 5);
        }
        println!(
            "  Refresh with:       {} (or `aida scaffold apply --dry-run` to preview)",
            "aida scaffold apply --force".cyan()
        );
    }
    if !seed_drift.is_empty() {
        // Seed customizations are expected — report them informationally
        // so the user knows their CLAUDE.md / AGENTS.md tweaks were
        // detected, but don't roll them into the STALE count.
        // trace:BUG-42 | ai:claude
        let label = format!(
            "  Customized:         {} user-owned file(s) (drift expected post-init): {}",
            seed_drift.len(),
            seed_drift
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("{}", label.dimmed());
    }
    println!();
}

/// Legacy-mode status: minimal output via the file-based Storage class.
pub(crate) fn handle_status_command(
    no_dev_context: bool,
    store_path_override: Option<&std::path::Path>,
    storage: &Storage,
) -> Result<()> {
    let store = storage.load()?;
    let project_root = std::env::current_dir()?;

    println!("{}", "─── Project ───".bold());
    let name = if store.name.is_empty() {
        "(unnamed)"
    } else {
        &store.name
    };
    println!("  Name:         {}", name.white().bold());
    let mode = if storage.is_sqlite() {
        "centralized SQLite (deprecated)"
    } else {
        "centralized YAML (deprecated)"
    };
    println!("  Mode:         {}", mode.yellow());
    println!("  Store path:   {}", storage.path().display());
    println!();

    let total = store.requirements.len();
    let mut by_status: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for r in &store.requirements {
        *by_status.entry(r.effective_status()).or_insert(0) += 1;
    }
    println!("{}", "─── Requirements ───".bold());
    println!("  Total:        {}", total);
    for (status, count) in &by_status {
        println!("    {:<14} {}", status, count);
    }
    println!();

    println!(
        "{}: this project is on a deprecated centralized backend.",
        "WARN".yellow()
    );
    println!(
        "      Migrate by running `aida db export-git -o aida-store && aida init` to switch to git-canonical."
    );
    println!();

    if !no_dev_context && is_aida_repo(&project_root) {
        print_aida_dev_context(&project_root);
    }

    let _ = store_path_override; // reserved for future use
    Ok(())
}
