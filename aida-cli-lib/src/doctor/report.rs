// trace:TASK-3781 | ai:codex

use super::*;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub(crate) struct DoctorHealResult {
    pub(super) category: String,
    pub(super) id: String,
    pub(crate) action: String,
    pub(crate) status: String,
    pub(super) detail: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub(crate) struct DoctorReport {
    pub(super) total: usize,
    pub(super) findings: Vec<DoctorFinding>,
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub(super) hidden_completed_without_commit: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) healed: Vec<DoctorHealResult>,
    /// TASK-865: read-only bubblewrap OS-sandbox availability status line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) bwrap: Option<String>,
    /// Machine boundary consumed by the performance scheduler. This is an
    /// allow-list, not captured stdout.
    // trace:BUG-1573 | ai:codex
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) performance_audits: Vec<schedule_ledger::PerformanceAudit>,
    /// BUG-1745: the failure reason, carried INSIDE the single stdout document
    /// instead of appended to it as a second document.
    // trace:BUG-1745 | ai:codex
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
    /// STORY-1462: what the runaway-seat watchdog could and could not see.
    // trace:STORY-1462 | ai:claude
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) runaway_seats: Option<crate::runaway_seats::Coverage>,
    /// STORY-1916: the disk guard at a glance (free space, floor, ceiling,
    /// last run, last automatic reclaim). Set by the disk-headroom light path.
    // trace:STORY-1916 | ai:claude
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) disk_status: Option<crate::disk_emergency::DiskStatus>,
    // trace:TASK-2631 | ai:codex
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) permission_postures: Vec<String>,
    // trace:BUG-2486 | ai:codex
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) codex_token_lifetime: Option<String>,
    // trace:BUG-2484 | ai:codex
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) vendor_binaries: Vec<String>,
}

// trace:BUG-1749 | ai:codex
#[cfg(test)]
pub(crate) fn bug_1745_test_report() -> DoctorReport {
    let mut report = DoctorReport::from_findings(vec![DoctorFinding {
        category: "performance".into(),
        id: "synthetic".into(),
        summary: "synthetic finding".into(),
        action: "inspect".into(),
        safe_heal: false,
    }]);
    report.error = Some("synthetic failure reason".into());
    report
        .performance_audits
        .push(schedule_ledger::PerformanceAudit {
            command: "synthetic".into(),
            budget_ms: 1,
            over_budget: 1,
            denominator: 1,
            proportion_millipercent: 1_000,
            tolerated_millipercent: 0,
            window_hours: 1,
            worst_ms: Some(2),
            excluded_samples: 0,
            lineage_scoped: false,
            ceiling_ms: None,
            ceiling_breached: false,
        });
    report
}

#[cfg(test)]
pub(crate) fn bug_1745_test_envelope() -> String {
    serde_json::to_string_pretty(&bug_1745_test_report()).unwrap()
}

pub(super) fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

/// The heavy orientation diagnostics moved off bare `aida status`
/// and onto `aida doctor`. Prints the PR/CI status (a `gh` network call),
/// live-session/lease liveness + fleet roster, the Claude Code presence line,
/// the worktree pane, the open-PR roster, and cross-clone coordination claims —
/// the same section printers `aida status --full` uses. Constructs the cached
/// backend on demand (the doctor's own load goes through the legacy `Storage`
/// path) so `collect_user_context` has the cache-backed handle it needs. Each
/// section graceful-degrades on its own data; best-effort — a backend that can't
/// be opened simply prints nothing here rather than failing the doctor run.
// trace:STORY-707 | ai:claude
pub(super) fn print_doctor_status_diagnostics(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
) {
    let store_path = project_root.join(".aida-store");
    let Ok(dispenser) = load_dispenser(&store_path) else {
        return;
    };
    let Ok(inner) = aida_core::GitBackend::new(&store_path).map(|b| b.with_dispenser(dispenser))
    else {
        return;
    };
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let Ok(backend) = aida_core::CachedGitBackend::with_inner(inner, &cache_path) else {
        return;
    };

    println!();
    println!("{}", "─── Status diagnostics ───".bold());
    println!(
        "  {}",
        "(moved off `aida status` so the default snapshot stays instant — STORY-707)".dimmed()
    );
    println!();

    // The full user-context gather: PR/CI (gh), branch facts, queue snapshot,
    // and the live-session-probed agent roster. This is the ~16s the fast
    // `aida status` no longer pays — it lives here now.
    let user_ctx = collect_user_context(project_root, store, &backend, false);

    print_status_pr_section(&user_ctx, false);
    print_status_queue_section(&user_ctx, false);
    print_status_presence_line(project_root);
    print_status_agents_section(&user_ctx, true);
    print_status_claude_code_section(project_root);
    // trace:BUG-2396 | ai:claude
    print_status_codex_section(project_root);
    print_status_worktrees_section(project_root, true);
    print_status_open_prs_section(project_root, true);
    print_status_coordination_section(&store_path, chrono::Utc::now(), true);
}

// trace:BUG-1749 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DoctorRenderMode {
    Human,
    Toon,
    Json,
}

// trace:BUG-1749 | ai:codex
pub(crate) fn doctor_render_mode_from(
    json_flag: bool,
    pin: Option<crate::OutputFormat>,
    agent: bool,
) -> DoctorRenderMode {
    if json_flag || matches!(pin, Some(crate::OutputFormat::Json)) {
        DoctorRenderMode::Json
    } else if matches!(pin, Some(crate::OutputFormat::Human)) {
        DoctorRenderMode::Human
    } else if matches!(pin, Some(crate::OutputFormat::Toon)) || agent {
        DoctorRenderMode::Toon
    } else {
        DoctorRenderMode::Human
    }
}

// trace:BUG-1749 | ai:codex
pub(super) fn doctor_render_mode(json_flag: bool) -> DoctorRenderMode {
    doctor_render_mode_from(
        json_flag,
        crate::output_format_override(),
        crate::agent_output_mode(),
    )
}

// trace:BUG-1749 | ai:codex
pub(crate) fn render_doctor_report_toon(report: &DoctorReport) -> String {
    use crate::toon::{error_block, scalar, table_raw};
    let mut blocks = vec![scalar("total", &report.total.to_string())];
    // trace:TASK-2631 | ai:codex
    if !report.permission_postures.is_empty() {
        let rows = report
            .permission_postures
            .iter()
            .map(|p| vec![p.clone()])
            .collect::<Vec<_>>();
        blocks.push(table_raw("permission_postures", &["posture"], &rows));
    }
    if let Some(lifetime) = &report.codex_token_lifetime {
        blocks.push(scalar(
            crate::vendors::codex_auth_hints::REPORT_FIELD,
            lifetime,
        ));
    }
    if !report.vendor_binaries.is_empty() {
        let rows = report
            .vendor_binaries
            .iter()
            .map(|p| vec![p.clone()])
            .collect::<Vec<_>>();
        blocks.push(table_raw("vendor_binaries", &["binary"], &rows));
    }
    if report.hidden_completed_without_commit != 0 {
        blocks.push(scalar(
            "hidden_completed_without_commit",
            &report.hidden_completed_without_commit.to_string(),
        ));
    }
    if !report.findings.is_empty() {
        let rows = report
            .findings
            .iter()
            .map(|f| {
                vec![
                    f.category.clone(),
                    f.id.clone(),
                    f.summary.clone(),
                    f.action.clone(),
                    f.safe_heal.to_string(),
                ]
            })
            .collect::<Vec<_>>();
        blocks.push(table_raw(
            "findings",
            &["category", "id", "summary", "action", "safe_heal"],
            &rows,
        ));
    }
    if !report.healed.is_empty() {
        let rows = report
            .healed
            .iter()
            .map(|r| {
                vec![
                    r.category.clone(),
                    r.id.clone(),
                    r.action.clone(),
                    r.status.clone(),
                    r.detail.clone().unwrap_or_default(),
                ]
            })
            .collect::<Vec<_>>();
        blocks.push(table_raw(
            "healed",
            &["category", "id", "action", "status", "detail"],
            &rows,
        ));
    }
    if let Some(v) = &report.bwrap {
        blocks.push(scalar("bwrap", v));
    }
    if !report.performance_audits.is_empty() {
        let rows = report
            .performance_audits
            .iter()
            .map(|a| {
                vec![
                    a.command.clone(),
                    a.budget_ms.to_string(),
                    a.over_budget.to_string(),
                    a.denominator.to_string(),
                    a.proportion_millipercent.to_string(),
                    a.tolerated_millipercent.to_string(),
                    a.window_hours.to_string(),
                    a.worst_ms.map(|v| v.to_string()).unwrap_or_default(),
                    a.excluded_samples.to_string(),
                    a.lineage_scoped.to_string(),
                    a.ceiling_ms.map(|v| v.to_string()).unwrap_or_default(),
                    a.ceiling_breached.to_string(),
                ]
            })
            .collect::<Vec<_>>();
        blocks.push(table_raw(
            "performance_audits",
            &[
                "command",
                "budget_ms",
                "over_budget",
                "denominator",
                "proportion_millipercent",
                "tolerated_millipercent",
                "window_hours",
                "worst_ms",
                "excluded_samples",
                "lineage_scoped",
                "ceiling_ms",
                "ceiling_breached",
            ],
            &rows,
        ));
    }
    if let Some(c) = &report.runaway_seats {
        for (k, v) in [
            ("runaway_seats_files_scanned", c.files_scanned.to_string()),
            ("runaway_seats_bytes_read", c.bytes_read.to_string()),
            ("runaway_seats_sessions", c.sessions.to_string()),
            (
                "runaway_seats_sessions_timing_unknown",
                c.sessions_timing_unknown.to_string(),
            ),
            ("runaway_seats_partial", c.partial.to_string()),
            ("runaway_seats_elapsed_ms", c.elapsed_ms.to_string()),
        ] {
            blocks.push(scalar(k, &v));
        }
        blocks.push(scalar("runaway_seats_verdict", &c.verdict));
        if !c.sources.is_empty() {
            let rows = c
                .sources
                .iter()
                .map(|s| {
                    vec![
                        s.source.clone(),
                        s.path.clone(),
                        s.state.clone(),
                        s.detail.clone(),
                    ]
                })
                .collect::<Vec<_>>();
            blocks.push(table_raw(
                "runaway_seats_sources",
                &["source", "path", "state", "detail"],
                &rows,
            ));
        }
        // trace:BUG-1962 | ai:antigravity
        if !c.still_tripped.is_empty() {
            let rows = c
                .still_tripped
                .iter()
                .map(|v| vec![v.clone()])
                .collect::<Vec<_>>();
            blocks.push(table_raw(
                "runaway_seats_still_tripped",
                &["finding"],
                &rows,
            ));
        }
    }
    // trace:STORY-1916.Visibility | ai:claude
    if let Some(d) = &report.disk_status {
        let rows = d
            .filesystems
            .iter()
            .map(|f| {
                vec![
                    f.role.to_string(),
                    f.path.clone(),
                    f.free_bytes.to_string(),
                    f.total_bytes.to_string(),
                    f.used_pct.to_string(),
                ]
            })
            .collect::<Vec<_>>();
        blocks.push(table_raw(
            "disk_filesystems",
            &["role", "path", "free_bytes", "total_bytes", "used_pct"],
            &rows,
        ));
        blocks.push(scalar("disk_floor_gib", &d.floor_gib.to_string()));
        blocks.push(scalar(
            "disk_ceiling_pct",
            &d.emergency.used_pct.to_string(),
        ));
        blocks.push(scalar(
            "disk_stop_below_pct",
            &d.emergency.stop_below().to_string(),
        ));
        blocks.push(scalar(
            "disk_idle_hours",
            &d.emergency.idle_hours.to_string(),
        ));
        blocks.push(scalar(
            "disk_last_guard_run",
            &d.last_guard_run.map_or("never".into(), |t| t.to_rfc3339()),
        ));
        blocks.push(scalar(
            "disk_last_guard_result",
            d.last_guard_result.as_deref().unwrap_or("-"),
        ));
        blocks.push(scalar(
            "disk_last_reclaim",
            &d.last_reclaim.as_ref().map_or("none".into(), |r| {
                format!(
                    "{} freed={} deleted={} after={} stopped={}",
                    r.at.to_rfc3339(),
                    r.freed_bytes,
                    r.deleted,
                    r.used_pct_after.map_or("?".into(), |p| p.to_string()),
                    r.stopped
                )
            }),
        ));
    }
    if let Some(v) = &report.error {
        blocks.push(error_block(v, None));
    }
    blocks.join("\n")
}

// trace:BUG-1749 | ai:codex
pub(super) fn emit_doctor_report(
    mut report: DoctorReport,
    reason: Option<String>,
    mode: DoctorRenderMode,
    healed: bool,
    human_extras: impl FnOnce(&DoctorReport),
) -> Result<()> {
    match mode {
        DoctorRenderMode::Json => {
            report.error = reason.clone();
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        DoctorRenderMode::Toon => {
            report.error = reason.clone();
            print!("{}", render_doctor_report_toon(&report));
        }
        DoctorRenderMode::Human => {
            render_doctor_report(&report, healed)?;
            human_extras(&report);
        }
    }
    match reason {
        None => Ok(()),
        Some(_) if mode != DoctorRenderMode::Human => {
            Err(anyhow::Error::new(crate::TypedPayloadEmitted))
        }
        Some(msg) => Err(anyhow::anyhow!("{msg}")),
    }
}

pub(super) fn render_doctor_report(report: &DoctorReport, healed: bool) -> Result<()> {
    println!("{}", "─── AIDA doctor ───".bold());
    // trace:TASK-2631 | ai:codex
    for posture in &report.permission_postures {
        println!("  {posture}");
    }
    if let Some(lifetime) = &report.codex_token_lifetime {
        println!("  {lifetime}");
    }
    for binary in &report.vendor_binaries {
        println!("  {binary}");
    }
    if report.findings.is_empty() && report.hidden_completed_without_commit == 0 {
        println!(
            "  {} no multi-agent state drift detected",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        let mut current = "";
        for finding in &report.findings {
            if current != finding.category {
                current = &finding.category;
                println!();
                println!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::Bullet).cyan(),
                    current.bold()
                );
            }
            let safety = if finding.safe_heal { "safe" } else { "manual" };
            println!("  - {} [{}]", finding.summary, safety.dimmed());
            println!("    → {}", finding.action.dimmed());
        }
        if report.hidden_completed_without_commit > 0 {
            if current != "completed-without-commit" {
                println!();
                println!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::Bullet).cyan(),
                    "completed-without-commit".bold()
                );
            }
            println!(
                "  ({} older completed-without-commit finding{} hidden — pass --all to list)",
                report.hidden_completed_without_commit,
                if report.hidden_completed_without_commit == 1 {
                    ""
                } else {
                    "s"
                }
            );
        }
        println!();
        println!(
            "  Total: {} visible finding{}",
            report.findings.len(),
            if report.findings.len() == 1 { "" } else { "s" }
        );
    }

    if healed {
        println!();
        println!("{}", "─── Heal results ───".bold());
        if report.healed.is_empty() {
            println!("  (no heal actions applied)");
        }
        for result in &report.healed {
            println!(
                "  - {} {}: {}",
                result.status.as_str().bold(),
                result.id,
                result.action
            );
            if let Some(detail) = &result.detail {
                println!("    {}", detail.dimmed());
            }
        }
    } else if !report.findings.is_empty() {
        println!("  Re-run with {} to apply safe fixes.", "--heal".cyan());
    }

    // TASK-865: report read-only environment facts the operator wants when
    // triaging launch confinement — currently the bubblewrap OS-sandbox status.
    // Availability only; this never enables `os_wrap`. trace:TASK-865 | ai:claude
    println!();
    println!("{}", "─── Environment ───".bold());
    render_doctor_bwrap_row();
    render_doctor_forge_row();
    Ok(())
}

/// Render the forge-CLI availability row in `aida doctor`'s environment section.
/// `aida pr auto-queue-review` (and the rest of the PR/CI lifecycle) fails hard
/// when the project's forge CLI (`gh` for GitHub, `glab` for GitLab) is not on
/// PATH, so surface it here — OK / missing-with-install-hint / none-needed
/// (pure-git) — rather than letting it stay silent until the first `pr` command.
// Colourised to match the doctor-check output style. trace:TASK-860 | ai:claude
pub(super) fn render_doctor_forge_row() {
    let project_root = match find_project_root() {
        Ok(root) => main_worktree_root_from(&root),
        // No project context (e.g. run outside a repo) — fall back to CWD so the
        // row still reports something useful rather than panicking.
        Err(_) => std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    };
    let (kind, msg) = crate::forge::forge_cli_status(&project_root);
    let glyph = if kind == crate::forge::ForgeKind::None {
        // Pure-git needs no forge CLI — informational, not a warning.
        crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
    } else if kind.cli_on_path() {
        crate::glyph(crate::glyphs::Glyph::Check).green()
    } else {
        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
    };
    println!("  {} {}", glyph, msg);
}

/// Render the bwrap availability row in `aida doctor`'s environment section,
/// colourised to match the doctor-check output style. When confinement is
/// blocked or bwrap is missing, print the EXACT copy-pasteable remediation
/// (not just a one-line prose hint) plus a pointer at the guided setup command;
// when it's ready, confirm how to opt in. trace:TASK-865 | ai:claude
// trace:STORY-665 | ai:claude
pub(super) fn render_doctor_bwrap_row() {
    let avail = crate::session::bwrap_availability();
    let glyph = match avail {
        crate::session::BwrapAvailability::Ok => crate::glyph(crate::glyphs::Glyph::Check).green(),
        crate::session::BwrapAvailability::NotInstalled => {
            crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
        }
        crate::session::BwrapAvailability::UsernsBlocked { .. } => {
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        }
    };
    println!("  {} {}", glyph, bwrap_status_line());
    match avail {
        crate::session::BwrapAvailability::Ok => {
            // Ready — confirm + point at how to opt in (it's off by default).
            println!(
                "    {}",
                "OS sandbox is ready. Opt in per-host with `export AIDA_OS_WRAP=1` (recommended — \
                 no shared-config change), or repo-wide with [contained] os_wrap = true in .aida/config.toml."
                    .dimmed()
            );
            // BUG-2434: say what an implementer lane does with it.
            println!("    {}", doctor_lane_tests_note(true).dimmed());
        }
        crate::session::BwrapAvailability::NotInstalled => {
            // Exact install command, then the guided-setup pointer.
            println!("    {}", "Install bubblewrap, then enable:".dimmed());
            println!("      {}", crate::session::BWRAP_INSTALL_DEBIAN.cyan());
            println!(
                "    {}",
                "Then run `aida doctor --fix-sandbox` for the full setup steps.".dimmed()
            );
        }
        crate::session::BwrapAvailability::UsernsBlocked { .. } => {
            // The kernel blocks unprivileged userns — print the EXACT runtime +
            // persist sysctl commands, clearly marked as sudo. trace:STORY-665
            println!(
                "    {}",
                "Kernel blocks unprivileged user namespaces. Fix (run these yourself):".dimmed()
            );
            println!(
                "      {}  {}",
                crate::session::BWRAP_USERNS_SYSCTL_RUNTIME.cyan(),
                "# this boot".dimmed()
            );
            println!(
                "      {}  {}",
                crate::session::BWRAP_USERNS_SYSCTL_PERSIST.cyan(),
                "# persist".dimmed()
            );
            println!(
                "    {}",
                "Then run `aida doctor --fix-sandbox` for the full setup + verify steps.".dimmed()
            );
        }
    }
    if avail != crate::session::BwrapAvailability::Ok {
        // BUG-2434: the cost of a host without working bubblewrap, stated
        // here rather than only in a lane log. trace:BUG-2434 | ai:claude
        println!("    {}", doctor_lane_tests_note(false).yellow());
    }
}

/// `aida doctor`'s note on what bubblewrap means for headless implementer
/// lanes' local tests. trace:BUG-2434 | ai:claude
pub(crate) fn doctor_lane_tests_note(bwrap_live: bool) -> &'static str {
    if bwrap_live {
        "Headless implementer lanes use it for their test runner when os_wrap is unset, so \
         they can run their tests (AIDA_OS_WRAP=0 opts out)."
    } else {
        "Until bubblewrap works here, headless implementer lanes cannot run local tests: \
         every lane result says untested=true."
    }
}

impl DoctorReport {
    pub(super) fn from_findings(findings: Vec<DoctorFinding>) -> Self {
        Self {
            total: findings.len(),
            findings,
            hidden_completed_without_commit: 0,
            healed: Vec::new(),
            bwrap: Some(bwrap_status_line()),
            performance_audits: Vec::new(),
            error: None,
            runaway_seats: None,
            disk_status: None,
            permission_postures: Vec::new(),
            codex_token_lifetime: None,
            vendor_binaries: Vec::new(),
        }
    }
}

/// One guarded command shape and the latency it must stay under.
///
/// Per-shape rather than one global number, so guarding a second command is a
/// new row rather than a reshaped config.
///
/// ADR-53: `budget_ms` alone cannot express a single-call ceiling — a
/// proportion is invariant to the magnitude of its own outliers, and improves
/// as its denominator grows, so it can go quiet while the worst call gets
/// worse. `ceiling_ms` is the second, independent limb: ANY single call over
/// it trips the gate regardless of how healthy the proportion looks.
// trace:STORY-1422 trace:ADR-53 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PerformanceBudget {
    pub(crate) cmd: String,
    pub(crate) budget_ms: u64,
    pub(crate) ceiling_ms: Option<u64>,
}

/// Read `[performance.budgets]` — a table of command shape to millisecond
/// budget, in one of two forms:
///
/// ```toml
/// [performance.budgets]
/// show = 1000                                    # budget only, no ceiling
/// "queue list" = { budget_ms = 2000, ceiling_ms = 8000 }
/// ```
///
/// A TABLE rather than `show_budget_ms`-style keys because a command shape can
/// contain a space (`queue list`), which a key suffix cannot express without
/// mangling. Absent section means no guarded commands, which is not a failure —
/// a project that has not opted in has nothing to breach. `ceiling_ms` is
/// optional (ADR-53): a shape with no ceiling is judged on proportion alone,
/// same as before this limb existed.
// trace:STORY-1422 trace:ADR-53 | ai:claude
pub(crate) fn performance_budgets(cfg: Option<&toml::Value>) -> Vec<PerformanceBudget> {
    let Some(table) = cfg
        .and_then(|c| c.get("performance"))
        .and_then(|p| p.get("budgets"))
        .and_then(|b| b.as_table())
    else {
        return Vec::new();
    };
    let mut budgets: Vec<PerformanceBudget> = table
        .iter()
        .filter_map(|(cmd, value)| {
            if let Some(ms) = value.as_integer() {
                let budget_ms = (ms > 0).then_some(ms as u64)?;
                return Some(PerformanceBudget {
                    cmd: cmd.to_string(),
                    budget_ms,
                    ceiling_ms: None,
                });
            }
            let inline = value.as_table()?;
            let budget_ms = inline.get("budget_ms")?.as_integer().filter(|ms| *ms > 0)? as u64;
            let ceiling_ms = inline
                .get("ceiling_ms")
                .and_then(|v| v.as_integer())
                .filter(|ms| *ms > 0)
                .map(|ms| ms as u64);
            Some(PerformanceBudget {
                cmd: cmd.to_string(),
                budget_ms,
                ceiling_ms,
            })
        })
        .collect();
    budgets.sort_by(|a, b| a.cmd.cmp(&b.cmd));
    budgets
}

/// How much of a shape's recent traffic may exceed its budget before the gate
/// trips, and over what window.
///
/// A PROPORTION, not a median and not "any call over budget" — ruled after the
/// distribution was measured rather than assumed. The `show` population is
/// BIMODAL: 4,438 calls under 1s and a second cluster at 9-10s, sharing one
/// command name and indistinguishable by every field the usage log records.
/// Against that shape the obvious statistics all fail — the median sits at
/// ~665ms and never trips even with the regression live, p90 swings from 812ms
/// to 9653ms depending on the window, and "any call over budget" is true 18-21%
/// of the time including after a fix.
// trace:STORY-1422 | ai:claude
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PerformancePolicy {
    /// Tolerated fraction of over-budget calls, as a percentage.
    pub(crate) tolerated_pct: f64,
    /// How far back to look.
    pub(crate) window_hours: i64,
}

impl Default for PerformancePolicy {
    fn default() -> Self {
        // 10% over 24h, PROVISIONAL. It trips today at 20.3% with 2x margin and
        // clears a post-fix residual estimated at 7.1% with only 1.4x. That
        // second margin is thin and known to be thin: the 1s-4s band may be
        // partial-cache cases that vanish with the fix, or a genuine tail that
        // stays, and nothing in the data distinguishes those futures. The number
        // changes only alongside a re-measurement, never by tuning until green.
        Self {
            tolerated_pct: 10.0,
            window_hours: 24,
        }
    }
}

/// Read `[performance]` policy knobs, falling back to the provisional default.
// trace:STORY-1422 | ai:claude
pub(crate) fn performance_policy(cfg: Option<&toml::Value>) -> PerformancePolicy {
    let section = cfg.and_then(|c| c.get("performance"));
    let mut policy = PerformancePolicy::default();
    if let Some(pct) = section
        .and_then(|p| p.get("tolerated_over_budget_pct"))
        .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
        .filter(|pct| (0.0..=100.0).contains(pct))
    {
        policy.tolerated_pct = pct;
    }
    if let Some(hours) = section
        .and_then(|p| p.get("window_hours"))
        .and_then(|v| v.as_integer())
        .filter(|h| *h > 0)
    {
        policy.window_hours = hours;
    }
    policy
}

/// BUG-1572: which binaries' calls the performance gate is allowed to count.
///
/// `~/.aida/usage.jsonl` is MACHINE-GLOBAL — every `aida` binary on the box
/// appends to it — and this repo routinely runs 20-60 git worktrees, each able
/// to build and run its own binary. Measured 2026-09-21, ELEVEN distinct
/// `binary_sha` values contributed to one 24h `show` window. So a single lane
/// on a stale build, or on a branch carrying a performance regression of its
/// own (precisely what such a lane is often built to investigate), could hold
/// MAIN's gate red indefinitely, and the aggregate finding gave the reading
/// seat no way to tell "main regressed" from "a lane is noisy".
///
/// The scope is ONE BINARY LINEAGE: ancestors of the current HEAD. That is the
/// same ancestry test `aida dev status` performs to decide whether the active
/// binary matches the branch, and it is reused rather than rewritten —
/// [`crate::current_branch_head_sha`] + [`crate::classify_sha_match`].
///
/// FAIL OPEN, NEVER CLOSED. Only a sha git POSITIVELY places off-lineage
/// ([`crate::ShaMatch::Unrelated`] — `merge-base --is-ancestor` exited a clean
/// `1` with both commits resolved) is excluded. A row with NO `binary_sha`, a
/// sha too short or too mangled for git to resolve, and a sha from another
/// project's repo or from a build whose commit was never pushed all classify
/// [`crate::ShaMatch::Unknown`] (exit `128`) and are COUNTED. Silently dropping
/// unplaceable rows would shrink the denominator, which is the very defect this
/// fixes; failing closed on them would make the gate unusable on any machine
/// whose log predates this repo.
// trace:BUG-1572 | ai:claude
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BinaryLineage {
    /// Lowercased shas git positively placed OFF this HEAD's lineage.
    pub(super) off_lineage: std::collections::BTreeSet<String>,
    /// `false` = count every row (not a git repo, git unavailable, or opted out).
    pub(super) scoped: bool,
}

impl BinaryLineage {
    /// The pre-BUG-1572 behaviour: every row in the window counts.
    pub(crate) fn unscoped() -> Self {
        Self::default()
    }

    /// Scope to HEAD's lineage, excluding exactly the given shas.
    pub(crate) fn scoped<I: IntoIterator<Item = String>>(off_lineage: I) -> Self {
        Self {
            off_lineage: off_lineage
                .into_iter()
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            scoped: true,
        }
    }

    pub(crate) fn is_scoped(&self) -> bool {
        self.scoped
    }

    /// Does a call served by `sha` count toward the gate's decision? Pure, so
    /// the fail-open rule is unit-testable without a git repo.
    pub(crate) fn counts(&self, sha: Option<&str>) -> bool {
        if !self.scoped {
            return true;
        }
        match sha {
            // Absent sha: unplaceable, so counted. See the fail-open note above.
            None => true,
            Some(s) => !self.off_lineage.contains(&s.trim().to_ascii_lowercase()),
        }
    }
}

/// Env opt-out for the BUG-1572 lineage scope — restores the pre-fix
/// count-every-binary population. Documented in `docs/environment-variables.md`.
// trace:BUG-1572 | ai:claude
pub(crate) const PERF_GATE_ALL_BINARIES_ENV: &str = "AIDA_PERF_GATE_ALL_BINARIES";

/// Pure: does this env value turn the lineage scope off?
// trace:BUG-1572 | ai:claude
pub(crate) fn perf_gate_scope_disabled_by(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim).unwrap_or(""),
        "1" | "true" | "yes" | "on"
    )
}

/// Normalize a row's `binary_sha` — an empty/whitespace field is the same as
/// an absent one.
// trace:BUG-1572 | ai:claude
pub(super) fn event_binary_sha(ev: &crate::usage::UsageEvent) -> Option<&str> {
    ev.binary_sha
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Resolve the lineage scope for the guarded shapes' WINDOW population.
///
/// `repo` is `aida doctor`'s project root, which `main_worktree_root_from`
/// already resolves to the MAIN worktree — so the gate asks the same question
/// of the same repo from whichever of the 20-60 worktrees it is run in, and
/// two seats reading it get the same verdict. A lane's own unmerged build is
/// therefore off-lineage, which is the intended reading: this gate judges the
/// mainline, not whatever branch the caller happens to be standing on.
///
/// Deliberately narrowed to rows a budget actually guards that fall inside the
/// window: `read_events` returns the whole log, and classifying every sha it
/// has ever seen would fork one `git merge-base` per historical build. The
/// guarded window is ~10 distinct binaries.
// trace:BUG-1572 | ai:claude
pub(crate) fn resolve_binary_lineage(
    repo: &std::path::Path,
    events: &[crate::usage::UsageEvent],
    budgets: &[PerformanceBudget],
    policy: &PerformancePolicy,
    now: chrono::DateTime<chrono::Utc>,
) -> BinaryLineage {
    if perf_gate_scope_disabled_by(std::env::var(PERF_GATE_ALL_BINARIES_ENV).ok().as_deref()) {
        return BinaryLineage::unscoped();
    }
    // No HEAD means nothing to be an ancestor OF — run unscoped rather than
    // excluding everything.
    let Some(head) = crate::current_branch_head_sha(repo) else {
        return BinaryLineage::unscoped();
    };
    let cutoff = now - chrono::Duration::hours(policy.window_hours);
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for ev in events {
        if !budgets.iter().any(|b| b.cmd == ev.cmd) {
            continue;
        }
        if !usage_event_time(&ev.ts).is_some_and(|t| t >= cutoff) {
            continue;
        }
        if let Some(sha) = event_binary_sha(ev) {
            seen.insert(sha.to_ascii_lowercase());
        }
    }
    let off_lineage: Vec<String> = seen
        .into_iter()
        .filter(|sha| {
            matches!(
                crate::classify_sha_match(repo, sha, &head),
                crate::ShaMatch::Unrelated
            )
        })
        .collect();
    BinaryLineage::scoped(off_lineage)
}

/// The label used for a row whose `binary_sha` field is absent or blank.
pub(crate) const UNKNOWN_BINARY_LABEL: &str = "unknown";

/// One binary's slice of a guarded shape's window population.
///
/// BUG-1572: the finding carries these so a MIXED population is VISIBLE
/// instead of silently averaged. "Main regressed" and "one lane is noisy"
/// produce the same aggregate proportion; only the per-binary denominators
/// tell them apart, and the seat reading the gate should not have to
/// hand-write the grouping query that discovered the defect.
// trace:BUG-1572 | ai:claude
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BinaryTally {
    pub(crate) sha: String,
    pub(crate) samples: usize,
    pub(crate) over_budget: usize,
    pub(crate) over_pct: f64,
    /// Did this binary's calls count toward the gate's decision?
    pub(crate) counted: bool,
}

/// Group a shape's window population by `binary_sha`. Pure; sorted by sample
/// count descending then sha, so the breakdown is stable across runs.
// trace:BUG-1572 | ai:claude
pub(crate) fn tally_by_binary(
    window: &[(Option<&str>, u64)],
    budget_ms: u64,
    lineage: &BinaryLineage,
) -> Vec<BinaryTally> {
    let mut by_sha: std::collections::BTreeMap<String, (usize, usize, bool)> =
        std::collections::BTreeMap::new();
    for (sha, ms) in window {
        let key = sha
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| UNKNOWN_BINARY_LABEL.to_string());
        let entry = by_sha.entry(key).or_insert((0, 0, lineage.counts(*sha)));
        entry.0 += 1;
        if *ms > budget_ms {
            entry.1 += 1;
        }
    }
    let mut tallies: Vec<BinaryTally> = by_sha
        .into_iter()
        .map(|(sha, (samples, over_budget, counted))| BinaryTally {
            sha,
            samples,
            over_budget,
            over_pct: (over_budget as f64) * 100.0 / (samples as f64),
            counted,
        })
        .collect();
    tallies.sort_by(|a, b| b.samples.cmp(&a.samples).then(a.sha.cmp(&b.sha)));
    tallies
}

/// Render the per-binary denominators onto a breach summary.
///
/// Every binary is listed, counted and excluded alike — truncating the list
/// would re-hide exactly the mixed population BUG-1572 exists to expose.
// trace:BUG-1572 | ai:claude
pub(crate) fn binary_breakdown_phrase(per_binary: &[BinaryTally], lineage_scoped: bool) -> String {
    fn render(t: &BinaryTally) -> String {
        let short = t.sha.get(..t.sha.len().min(10)).unwrap_or(&t.sha);
        format!("{} {:.1}% of {}", short, t.over_pct, t.samples)
    }
    let counted: Vec<String> = per_binary
        .iter()
        .filter(|t| t.counted)
        .map(render)
        .collect();
    let excluded: Vec<&BinaryTally> = per_binary.iter().filter(|t| !t.counted).collect();
    let mut phrase = format!("; per binary — counted: {}", counted.join(", "));
    if !lineage_scoped {
        phrase.push_str(
            " (lineage scope OFF — this denominator spans every aida binary on the machine)",
        );
    }
    if !excluded.is_empty() {
        let n: usize = excluded.iter().map(|t| t.samples).sum();
        phrase.push_str(&format!(
            "; excluded {} calls from {} binar{} outside this HEAD's lineage: {}",
            n,
            excluded.len(),
            if excluded.len() == 1 { "y" } else { "ies" },
            excluded
                .into_iter()
                .map(render)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    phrase
}

/// ADR-53's ceiling limb, rendered onto a breach summary. Reports the
/// ceiling's presence and status even when it did NOT trip the gate, so a
/// reader can tell "no ceiling configured" from "ceiling configured and
/// clear" — the same absent-evidence-is-not-good-evidence rule the
/// unobserved-budget finding already follows.
// trace:ADR-53 | ai:claude
pub(super) fn ceiling_clause(ceiling_ms: Option<u64>, worst_ms: u64, breached: bool) -> String {
    match ceiling_ms {
        Some(c) if breached => {
            format!("; CEILING BREACHED — a single call ran {worst_ms} ms against a {c} ms ceiling")
        }
        Some(c) => format!("; ceiling {c} ms not breached"),
        None => String::new(),
    }
}

/// One breached budget, kept machine-readable for the ledger.
///
/// `budget_ms` and `tolerated_pct` are both recorded because a CONFIGURED
/// threshold is a number someone can quietly raise the moment it trips — the
/// documented way latency gates die. A later reader must be able to tell a
/// fixed regression from a raised ceiling, and that is only possible if the
/// numbers in force AT THE TIME OF THE TRIP are in the record.
// trace:STORY-1422 | ai:claude
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PerformanceBreach {
    pub(crate) cmd: String,
    pub(crate) budget_ms: u64,
    pub(crate) over_budget: usize,
    pub(crate) samples: usize,
    pub(crate) over_pct: f64,
    pub(crate) tolerated_pct: f64,
    pub(crate) window_hours: i64,
    pub(crate) worst_ms: u64,
    /// BUG-1572: the FULL window population split by `binary_sha` — counted
    /// and excluded binaries alike — so a mixed population is visible rather
    /// than silently averaged into one proportion.
    pub(crate) per_binary: Vec<BinaryTally>,
    /// Calls in the window dropped because their binary is off this HEAD's
    /// lineage. `0` when the gate runs unscoped.
    pub(crate) excluded_samples: usize,
    /// Was the lineage scope ACTIVE for this decision? Recorded because
    /// "nothing was excluded" and "the scope was off" otherwise render
    /// identically, and only the second means the denominator is machine-wide.
    pub(crate) lineage_scoped: bool,
    /// ADR-53's second limb: the configured single-call ceiling, if any.
    pub(crate) ceiling_ms: Option<u64>,
    /// Did `worst_ms` exceed `ceiling_ms`? A breach can be recorded on this
    /// limb alone even while `over_pct` sits comfortably under `tolerated_pct`
    /// — that gap is the whole reason ADR-53 exists.
    pub(crate) ceiling_breached: bool,
}

/// Parse a usage event's timestamp, tolerating both `Z` and offset forms.
// trace:STORY-1422 | ai:claude
pub(super) fn usage_event_time(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// A guarded shape's window population: every in-window call, paired with the
/// binary that served it. Kept together because BUG-1572 needs BOTH the scoped
/// subset (the gate's decision) and the full split (the finding's breakdown).
// trace:BUG-1572 | ai:claude
pub(super) fn window_population<'a>(
    events: &'a [crate::usage::UsageEvent],
    cmd: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Vec<(Option<&'a str>, u64)> {
    events
        .iter()
        .filter(|ev| ev.cmd == cmd)
        .filter(|ev| usage_event_time(&ev.ts).is_some_and(|t| t >= cutoff))
        .map(|ev| (event_binary_sha(ev), ev.duration_ms))
        .collect()
}

/// Which guarded shapes exceed their budget TOO OFTEN inside the window.
///
/// Pure over its inputs INCLUDING `now` and `lineage`, so the decision is
/// testable without a usage log on disk, without a git repo, and without
/// depending on the wall clock.
///
/// BUG-1572: `lineage` scopes the DECISION population to one binary lineage.
/// The reported `per_binary` split still covers every binary in the window —
/// scoping the gate without showing what was scoped out would just relocate
/// the blind spot.
// trace:STORY-1422 trace:BUG-1572 | ai:claude
pub(crate) fn performance_breaches(
    events: &[crate::usage::UsageEvent],
    budgets: &[PerformanceBudget],
    policy: &PerformancePolicy,
    now: chrono::DateTime<chrono::Utc>,
    lineage: &BinaryLineage,
) -> Vec<PerformanceBreach> {
    let cutoff = now - chrono::Duration::hours(policy.window_hours);
    let mut breaches = Vec::new();
    for budget in budgets {
        let window = window_population(events, &budget.cmd, cutoff);
        let per_binary = tally_by_binary(&window, budget.budget_ms, lineage);
        let counted: Vec<u64> = window
            .iter()
            .filter(|(sha, _)| lineage.counts(*sha))
            .map(|(_, ms)| *ms)
            .collect();
        if counted.is_empty() {
            // No in-lineage samples in the window is NOT a pass —
            // `performance_unobserved` reports it (and says how many
            // off-lineage calls were dropped), so silence is explained
            // rather than assumed.
            continue;
        }
        let over_budget = counted.iter().filter(|ms| **ms > budget.budget_ms).count();
        let over_pct = (over_budget as f64) * 100.0 / (counted.len() as f64);
        let worst_ms = counted.iter().copied().max().unwrap_or(0);
        // ADR-53: a single call over the ceiling trips the gate on its own —
        // a proportion cannot express this, because it is invariant to the
        // magnitude of its own outliers and improves as its denominator grows.
        let ceiling_breached = budget.ceiling_ms.is_some_and(|c| worst_ms > c);
        if over_pct > policy.tolerated_pct || ceiling_breached {
            breaches.push(PerformanceBreach {
                cmd: budget.cmd.clone(),
                budget_ms: budget.budget_ms,
                over_budget,
                samples: counted.len(),
                over_pct,
                tolerated_pct: policy.tolerated_pct,
                window_hours: policy.window_hours,
                worst_ms,
                per_binary,
                excluded_samples: window.len() - counted.len(),
                lineage_scoped: lineage.is_scoped(),
                ceiling_ms: budget.ceiling_ms,
                ceiling_breached,
            });
        }
    }
    breaches
}

/// Guarded shapes that produced NO samples at all.
///
/// Reported separately and deliberately: a budget naming a command that never
/// ran is indistinguishable from a healthy one if both are silent, and a typo
/// in a shape name is exactly how a scheduled guard watches nothing for months
/// while reporting success. Absent evidence is not good evidence.
// trace:STORY-1422 | ai:claude
///
/// BUG-1572: "unobserved" is judged against the SCOPED population, because a
/// gate scoped to one binary lineage has no evidence about that lineage when
/// every in-window call came from another one. `excluded_samples` records how
/// many calls were dropped so the message can say WHY it is silent — a bare
/// "no recorded calls" would be false where 800 calls exist off-lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnobservedBudget {
    pub(crate) cmd: String,
    /// In-window calls dropped as off-lineage. `0` = genuinely no calls.
    pub(crate) excluded_samples: usize,
}

pub(crate) fn performance_unobserved(
    events: &[crate::usage::UsageEvent],
    budgets: &[PerformanceBudget],
    policy: &PerformancePolicy,
    now: chrono::DateTime<chrono::Utc>,
    lineage: &BinaryLineage,
) -> Vec<UnobservedBudget> {
    let cutoff = now - chrono::Duration::hours(policy.window_hours);
    budgets
        .iter()
        .filter_map(|budget| {
            let window = window_population(events, &budget.cmd, cutoff);
            let counted = window
                .iter()
                .filter(|(sha, _)| lineage.counts(*sha))
                .count();
            if counted > 0 {
                return None;
            }
            Some(UnobservedBudget {
                cmd: budget.cmd.clone(),
                excluded_samples: window.len(),
            })
        })
        .collect()
}

/// The `performance` category's findings: shapes over budget too often, plus
/// guarded shapes that produced no samples in the window.
///
/// BOTH ARE FINDINGS, and that is the load-bearing decision. The instrument
/// this category replaces could not distinguish "over budget", "under budget"
/// and "pointed at a command that does not exist" — three states, one exit
/// code, which is how a scheduled guard watches nothing for months and reports
/// health. An unobserved budget is a finding so silence has to be explained.
// trace:STORY-1422 | ai:claude
pub(crate) fn performance_findings(
    events: &[crate::usage::UsageEvent],
    budgets: &[PerformanceBudget],
    policy: &PerformancePolicy,
    now: chrono::DateTime<chrono::Utc>,
    lineage: &BinaryLineage,
) -> Vec<DoctorFinding> {
    let mut findings: Vec<DoctorFinding> =
        performance_breaches(events, budgets, policy, now, lineage)
            .into_iter()
            .map(|b| DoctorFinding {
                category: "performance".to_string(),
                id: b.cmd.clone(),
                summary: format!(
                    "`aida {}` exceeded its {} ms budget on {:.1}% of {} calls in the last {}h \
                 (tolerated {:.1}%, worst {} ms){}{}",
                    b.cmd,
                    b.budget_ms,
                    b.over_pct,
                    b.samples,
                    b.window_hours,
                    b.tolerated_pct,
                    b.worst_ms,
                    ceiling_clause(b.ceiling_ms, b.worst_ms, b.ceiling_breached),
                    binary_breakdown_phrase(&b.per_binary, b.lineage_scoped)
                ),
                action: if b.ceiling_breached {
                    "a single call exceeded the ceiling — that trips regardless of the \
                     proportion, because a proportion cannot express one bad call; \
                     investigate that call, or raise the ceiling deliberately in \
                     [performance.budgets] with the consequence in front of you"
                        .to_string()
                } else {
                    "investigate the regression, or change the budget deliberately in \
                 [performance] — raising a threshold the moment it trips is how a latency gate \
                 dies, so the numbers in force are recorded with the trip"
                        .to_string()
                },
                safe_heal: false,
            })
            .collect();

    findings.extend(
        performance_unobserved(events, budgets, policy, now, lineage)
            .into_iter()
            .map(|u| DoctorFinding {
                category: "performance".to_string(),
                id: u.cmd.clone(),
                summary: if u.excluded_samples > 0 {
                    // BUG-1572: say WHY it is silent. "No recorded calls" would
                    // be plainly false with hundreds of off-lineage calls in
                    // the window, and a reader would chase the wrong cause.
                    format!(
                        "`aida {}` has a budget but NO recorded calls from this HEAD's binary \
                         lineage in the last {}h — {} in-window call(s) were served by binaries \
                         outside it and were not counted",
                        u.cmd, policy.window_hours, u.excluded_samples
                    )
                } else {
                    format!(
                        "`aida {}` has a budget but NO recorded calls in the last {}h — the \
                         guard is watching a command it never sees",
                        u.cmd, policy.window_hours
                    )
                },
                action: if u.excluded_samples > 0 {
                    "run the guarded command on a binary built from this HEAD's lineage, or set \
                     AIDA_PERF_GATE_ALL_BINARIES=1 to count every binary on the machine"
                        .to_string()
                } else {
                    "check the shape spelling in [performance.budgets] and that telemetry \
                     is enabled; an unobserved budget proves nothing"
                        .to_string()
                },
                safe_heal: false,
            }),
    );

    findings
}

/// Typed, allow-listed evidence for the scheduler boundary. This deliberately
/// repeats the pure gate calculation rather than asking a consumer to parse a
/// human sentence. A zero denominator is an explicit record, not division.
// trace:BUG-1573 | ai:codex
pub(super) fn performance_audit_records(
    events: &[crate::usage::UsageEvent],
    budgets: &[PerformanceBudget],
    policy: &PerformancePolicy,
    now: chrono::DateTime<chrono::Utc>,
    lineage: &BinaryLineage,
) -> Vec<schedule_ledger::PerformanceAudit> {
    let mut out: Vec<_> = performance_breaches(events, budgets, policy, now, lineage)
        .into_iter()
        .map(|b| schedule_ledger::PerformanceAudit {
            command: b.cmd,
            budget_ms: b.budget_ms,
            over_budget: b.over_budget,
            denominator: b.samples,
            proportion_millipercent: (b.over_pct * 1000.0).round() as u32,
            tolerated_millipercent: (b.tolerated_pct * 1000.0).round() as u32,
            window_hours: b.window_hours,
            worst_ms: Some(b.worst_ms),
            excluded_samples: b.excluded_samples,
            lineage_scoped: b.lineage_scoped,
            ceiling_ms: b.ceiling_ms,
            ceiling_breached: b.ceiling_breached,
        })
        .collect();
    for u in performance_unobserved(events, budgets, policy, now, lineage) {
        let Some(budget) = budgets.iter().find(|b| b.cmd == u.cmd) else {
            continue;
        };
        out.push(schedule_ledger::PerformanceAudit {
            command: u.cmd,
            budget_ms: budget.budget_ms,
            over_budget: 0,
            denominator: 0,
            proportion_millipercent: 0,
            tolerated_millipercent: (policy.tolerated_pct * 1000.0).round() as u32,
            window_hours: policy.window_hours,
            worst_ms: None,
            excluded_samples: u.excluded_samples,
            lineage_scoped: lineage.is_scoped(),
            ceiling_ms: budget.ceiling_ms,
            ceiling_breached: false,
        });
    }
    out.sort_by(|a, b| a.command.cmp(&b.command));
    out.truncate(schedule_ledger::MAX_PERFORMANCE_AUDITS);
    out
}
