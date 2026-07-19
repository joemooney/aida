//! `aida health` vital-signs command cluster (STORY-658).
//!
//! The at-a-glance system-vitals health report: gathers backlog + coordination
//! inputs and feeds them to the pure scorers in [`crate::health`]. Extracted
//! verbatim from `main.rs` (SPIKE-78, pure movement) — the I/O + presentation
//! layer only; all the judgment lives in `crate::health`.

use anyhow::Result;
use colored::Colorize;

use crate::*;

/// `aida health` — a fast, honest, at-a-glance vital-signs read.
///
/// Gathers the inputs (open specs + their idle/blocked/in-flight state, recent
/// burn-down velocity, and the coordination substrate: queue depth, leases,
/// drain lock, open findings, parked work), then feeds them to the pure scorers
/// in [`crate::health`]. All the judgment lives in that module; this function is
/// I/O + presentation only. Exits 0 on a successful read regardless of grade.
// trace:STORY-658 | ai:claude
pub(crate) fn handle_health_vitals_command(json: bool, brief: bool) -> Result<()> {
    use crate::health::{
        backlog_vitals, coordination_vitals, Axis, CoordinationInputs, Grade, HealthReport,
        OpenSpec, Thresholds, Vital,
    };
    use chrono::Datelike;

    // Shared `.aida/` root so a child worktree reads the orchestrator's
    // lock/leases (mirrors `burndown status`). Fail safe to CWD.
    let project_root = find_main_worktree_root()
        .or_else(|_| std::env::current_dir())
        .unwrap_or_else(|_| std::path::PathBuf::from("."));

    let thresholds = Thresholds::default();
    let now = chrono::Utc::now();

    // ── Backlog axis: read the store once (cache-backed lookup path). ──
    let store = load_store_for_lookup(&project_root);
    let in_flight_scopes = in_flight_lease_scopes(&project_root);

    let mut open_specs: Vec<OpenSpec> = Vec::new();
    let mut needs_attention = 0usize;
    let mut open_finding_count = 0usize;
    // Velocity inputs: every spec's created-day + (optional) completed-day.
    let mut lifecycle: Vec<crate::health_metrics::SpecLifecycleDays> = Vec::new();

    if let Some(store) = store.as_ref() {
        let open_status = |s: &aida_core::RequirementStatus| {
            matches!(
                s,
                aida_core::RequirementStatus::Draft
                    | aida_core::RequirementStatus::Approved
                    | aida_core::RequirementStatus::Planned
                    | aida_core::RequirementStatus::InProgress
                    | aida_core::RequirementStatus::NeedsAttention
            )
        };
        let completed_id = store
            .requirements
            .iter()
            .map(|r| {
                (
                    r.id,
                    matches!(r.status, aida_core::RequirementStatus::Completed),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();

        for r in &store.requirements {
            // Velocity: created day always; completed day from history walk,
            // falling back to modified_at for currently-Completed specs.
            let created_day = r.created_at.date_naive().num_days_from_ce() as i64;
            let completed_day = completed_day_for(r);
            lifecycle.push(crate::health_metrics::SpecLifecycleDays {
                created_day,
                completed_day,
            });

            let tags: Vec<String> = r.tags.iter().cloned().collect();

            if matches!(r.status, aida_core::RequirementStatus::NeedsAttention) {
                needs_attention += 1;
            }
            // Open findings = draft specs carrying a finding source tag.
            if matches!(r.status, aida_core::RequirementStatus::Draft)
                && crate::findings::is_finding(&tags)
            {
                open_finding_count += 1;
            }

            if !open_status(&r.status) {
                continue;
            }

            let idle_days = now.signed_duration_since(r.modified_at).num_days().max(0);
            // Blocked = has a BlockedBy edge to a spec that isn't Completed.
            let blocked = r.relationships.iter().any(|rel| {
                matches!(rel.rel_type, aida_core::RelationshipType::BlockedBy)
                    && !completed_id.get(&rel.target_id).copied().unwrap_or(false)
            });
            let display = r.display_id().to_ascii_lowercase();
            let in_flight = in_flight_scopes.contains(&display);

            open_specs.push(OpenSpec {
                status: format!("{:?}", r.status),
                idle_days,
                blocked,
                in_flight,
            });
        }
    }

    // Velocity over the trailing 14-day window.
    let today = now.date_naive().num_days_from_ce() as i64;
    let velocity = if lifecycle.is_empty() {
        None
    } else {
        crate::health_metrics::burn_down_velocity(&lifecycle, today - 13, today).net_per_day()
    };

    // ── Coordination axis: probe the runtime substrate. ──
    let lock = drain_lock::probe_lock(&project_root);
    let drain_running = matches!(lock, drain_lock::LockStatus::Running(_));
    let drain_stale = matches!(lock, drain_lock::LockStatus::Stale(_));

    let live_sessions = process_probe::probe_live_claude_sessions();
    let mut live_leases = 0usize;
    let mut stale_leases = 0usize;
    for l in list_leases(&project_root) {
        match lease_state_for(&l, &live_sessions, now) {
            LeaseState::Live => live_leases += 1,
            LeaseState::Stale => stale_leases += 1,
            LeaseState::Dormant => {}
        }
    }

    let queue_depth = read_queue_depth(&project_root, Some("implementer")).unwrap_or(0);

    let coord = CoordinationInputs {
        queue_depth,
        live_leases,
        stale_leases,
        open_findings: open_finding_count,
        drain_running,
        drain_stale,
        needs_attention,
    };

    let report = HealthReport::build(
        backlog_vitals(&open_specs, velocity, &thresholds),
        coordination_vitals(&coord, &thresholds),
    );

    if json {
        let vitals_json: Vec<serde_json::Value> = report
            .vitals
            .iter()
            .map(|v| {
                serde_json::json!({
                    "axis": v.axis.label(),
                    "key": v.key,
                    "label": v.label,
                    "grade": v.grade.token(),
                    "value": v.value,
                    "detail": v.detail,
                    "remedy": v.remedy,
                })
            })
            .collect();
        let (h, w, c) = report.counts();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "overall": report.overall.token(),
                "headline": report.headline(),
                "store_loaded": store.is_some(),
                "counts": { "healthy": h, "watch": w, "critical": c },
                "vitals": vitals_json,
            }))?
        );
        return Ok(());
    }

    // Color the grade glyph + label consistently. Healthy=check/green,
    // Watch=warning/yellow, Critical=cross/red. Glyphs route through the
    // registry per STORY-628. trace:STORY-658 | ai:claude
    let grade_glyph = |g: Grade| match g {
        Grade::Healthy => crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        Grade::Watch => crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
        Grade::Critical => crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
    };
    let headline_colored = match report.overall {
        Grade::Healthy => report.headline().green().bold(),
        Grade::Watch => report.headline().yellow().bold(),
        Grade::Critical => report.headline().red().bold(),
    };

    if brief {
        println!("{} {}", grade_glyph(report.overall), headline_colored);
        return Ok(());
    }

    if store.is_none() {
        println!(
            "{}",
            "  (no requirement store found — backlog vitals are empty; \
             coordination vitals only)"
                .dimmed()
        );
    }

    println!();
    println!("  {} {}", grade_glyph(report.overall), headline_colored);
    let (h, w, c) = report.counts();
    println!(
        "  {}",
        format!("{h} healthy · {w} watch · {c} critical").dimmed()
    );

    for axis in [Axis::Backlog, Axis::Coordination] {
        println!();
        println!("  {}", axis.label().to_uppercase().bold());
        let rows: Vec<&Vital> = report.vitals.iter().filter(|v| v.axis == axis).collect();
        let label_w = rows.iter().map(|v| v.label.len()).max().unwrap_or(0);
        for v in rows {
            let value = match v.grade {
                Grade::Healthy => v.value.normal(),
                Grade::Watch => v.value.yellow(),
                Grade::Critical => v.value.red(),
            };
            println!(
                "    {} {:<width$}  {}",
                grade_glyph(v.grade),
                v.label,
                value,
                width = label_w
            );
            // Fold in the one-line meaning as a dimmed detail line for any
            // non-healthy vital, so the worst-first issue list reads with
            // context inline; a healthy read stays quiet. trace:TASK-853
            if v.grade != Grade::Healthy {
                println!("      {}", v.detail.dimmed());
            }
            // Surface the remedy only when there's something to act on, so a
            // healthy read stays quiet (honest, not noisy). trace:STORY-658
            if let Some(remedy) = v.remedy {
                println!(
                    "      {} {}",
                    crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                    remedy.dimmed()
                );
            }
        }
    }
    println!();

    Ok(())
}

/// STORY-658: the ordinal day a spec reached Completed, for burn-down velocity.
/// Walks the spec `history:` for the most recent `status` change whose
/// `new_value` is `Completed`; falls back to `modified_at` for a currently-
/// Completed spec with no such history row. `None` when the spec never
/// completed.
// trace:STORY-658 | ai:claude
fn completed_day_for(r: &aida_core::Requirement) -> Option<i64> {
    use chrono::Datelike;
    let from_history = r
        .history
        .iter()
        .filter(|h| {
            h.changes.iter().any(|c| {
                c.field_name.eq_ignore_ascii_case("status")
                    && c.new_value.eq_ignore_ascii_case("Completed")
            })
        })
        .map(|h| h.timestamp)
        .max();
    match from_history {
        Some(ts) => Some(ts.date_naive().num_days_from_ce() as i64),
        None if matches!(r.status, aida_core::RequirementStatus::Completed) => {
            Some(r.modified_at.date_naive().num_days_from_ce() as i64)
        }
        None => None,
    }
}
