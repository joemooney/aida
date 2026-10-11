//! Advisor command handlers (handoff, register, dashboard), AdvisorPass and calibration mode (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// STORY-363 (SPIKE-10 Track B): write a checked-in advisor handoff brief into
/// a sibling project. The brief lands at `<to>/docs/<date>-advisor-handoff.md`
/// and carries five sections — parent identity (auto-derived from the current
/// project root), vision, decided things, substrate slice (scoped by
/// `--focus`), and latitude. Only parent identity and the focus topic are
/// filled in; the strategic sections are placeholders the operator authors
/// before committing the brief to the child project. Bounded by design: this
/// is a template generator, not a context-extraction pipeline.
// trace:STORY-363 | ai:claude
pub(crate) fn handle_advisor_handoff(to: &std::path::Path, focus: &str, force: bool) -> Result<()> {
    let focus = focus.trim();
    if focus.is_empty() {
        anyhow::bail!("--focus cannot be empty — name the topic this handoff hands off");
    }
    if !to.exists() {
        anyhow::bail!(
            "target project does not exist: {} — pass an existing project path with --to",
            to.display()
        );
    }
    // Parent identity is auto-derived from the current project root (falling
    // back to the cwd when not inside an AIDA project — the handoff is still
    // useful, just with a thinner identity line).
    let parent_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let parent_name = parent_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let authored_by = brief_generated_by();
    let body = render_advisor_handoff(&parent_name, &parent_root, focus, &date, &authored_by);

    let docs_dir = to.join("docs");
    std::fs::create_dir_all(&docs_dir)
        .with_context(|| format!("failed to create {}", docs_dir.display()))?;
    let path = docs_dir.join(format!("{date}-advisor-handoff.md"));

    if path.exists() && !force {
        anyhow::bail!(
            "a handoff brief for today already exists: {} (pass --force to overwrite)",
            path.display()
        );
    }
    std::fs::write(&path, body).with_context(|| format!("failed to write {}", path.display()))?;

    println!(
        "{} {}",
        "Wrote advisor handoff:".green().bold(),
        path.display()
    );
    println!(
        "  Fill in the vision / decided-things / latitude sections, then commit it to {}.",
        parent_name.cyan()
    );
    Ok(())
}

/// Pure assembly of the advisor-handoff brief body — no I/O, fully testable in
/// isolation. Five sections per SPIKE-10 Gap 2: parent identity (auto), vision
/// (operator), decided things (operator-pruned), substrate slice (scoped by
/// `--focus`), and latitude (operator). The strategic sections ship as
/// placeholders the operator authors before committing.
// trace:STORY-363 | ai:claude
pub(crate) fn render_advisor_handoff(
    parent_name: &str,
    parent_root: &std::path::Path,
    focus: &str,
    date: &str,
    authored_by: &str,
) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("kind: {}\n", yaml_scalar("advisor-handoff")));
    out.push_str(&format!("parent: {}\n", yaml_scalar(parent_name)));
    out.push_str(&format!("focus: {}\n", yaml_scalar(focus)));
    out.push_str(&format!("date: {}\n", yaml_scalar(date)));
    out.push_str(&format!("authored_by: {}\n", yaml_scalar(authored_by)));
    out.push_str("status: draft\n");
    out.push_str("---\n\n");

    out.push_str(&format!("# Advisor handoff — {focus}\n\n"));
    out.push_str(&format!(
        "Generated {date} from **{parent_name}**. This brief hands off the advisor \
         context for **{focus}** to a sibling project. Parent identity and the focus \
         topic are filled in; author the remaining sections before committing.\n\n"
    ));

    out.push_str("## 1. Parent identity (auto)\n\n");
    out.push_str(&format!("- Parent project: **{parent_name}**\n"));
    out.push_str(&format!("- Parent root: `{}`\n", parent_root.display()));
    out.push_str(&format!("- Handoff focus: **{focus}**\n"));
    out.push_str(&format!("- Generated: {date}\n\n"));

    out.push_str("## 2. Vision (operator)\n\n");
    out.push_str(
        "_Why does this child project exist, and how does it relate to the parent's \
         mission? What does success look like 3-6 months out?_\n\n",
    );
    out.push_str("- TODO: state the vision in the operator's own words.\n\n");

    out.push_str("## 3. Decided things (operator-pruned)\n\n");
    out.push_str(
        "_Decisions the child should inherit rather than re-litigate — architecture \
         choices, conventions, tooling, non-goals. Prune anything that doesn't carry \
         over._\n\n",
    );
    out.push_str("- TODO: list the load-bearing decisions the child inherits.\n\n");

    out.push_str(&format!("## 4. Substrate slice — {focus}\n\n"));
    out.push_str(&format!(
        "_The slice of the parent's substrate (specs, docs, conventions, code) that is \
         relevant to **{focus}**. Point at concrete artifacts; do not paste them._\n\n"
    ));
    out.push_str("- TODO: link the specs / docs / modules that scope this focus.\n\n");

    out.push_str("## 5. Latitude (operator)\n\n");
    out.push_str(
        "_Where the child has freedom to diverge, and where it must stay aligned with \
         the parent. Name the guardrails and the open questions explicitly._\n\n",
    );
    out.push_str("- TODO: state the latitude and the guardrails.\n");

    out
}

#[cfg(test)]
#[path = "tests/story_363_handoff_tests.rs"]
mod story_363_handoff_tests;

pub(crate) fn handle_advisor_register(
    uuid: Option<&str>,
    project_slug: Option<&str>,
) -> Result<()> {
    let uuid = uuid
        .map(|s| s.to_string())
        .or_else(|| std::env::var("CLAUDE_CODE_SESSION_ID").ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no session UUID — pass `--uuid <id>` or run from a Claude session that \
                 exports CLAUDE_CODE_SESSION_ID"
            )
        })?;

    let cwd = std::env::current_dir().context("cannot read current directory")?;
    let project_slug = project_slug
        .map(|s| s.to_string())
        .unwrap_or_else(|| process_probe::encode_cwd_for_projects(&cwd));

    // Best-effort: record the ancestor `claude` PID so the liveness check has
    // something more authoritative than JSONL mtime.
    let claude_pid = process_probe::probe_live_claude_sessions()
        .into_iter()
        .find_map(|s| {
            // Match either by recent-jsonl uuid or by cwd containment so we
            // don't pick a stranger's claude.
            if s.jsonl
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|f| f.to_string_lossy() == uuid)
                .unwrap_or(false)
            {
                Some(s.pid)
            } else {
                None
            }
        });

    let reg = advisor::AdvisorRegistration {
        uuid: uuid.clone(),
        project_slug: project_slug.clone(),
        project_root: cwd.to_string_lossy().to_string(),
        registered_at: chrono::Utc::now().to_rfc3339(),
        claude_pid,
    };
    advisor::write_registration(&reg)?;

    println!("Registered live advisor session.");
    println!("  uuid:          {}", uuid);
    println!("  project slug:  {}", project_slug);
    println!("  project root:  {}", cwd.display());
    if let Some(pid) = claude_pid {
        println!("  claude pid:    {}", pid);
    } else {
        println!("  claude pid:    (none detected — liveness will use JSONL mtime)");
    }
    println!("\nThe `--no-human=both` orchestrator will now fork this session for advisor punts.");
    println!("Run `aida advisor unregister` to revert to cold-boot.");
    Ok(())
}

/// STORY-559: the narrow live-advisor registration block — the pre-dashboard
/// `aida advisor status` output, now reachable via `--registration` (and the
/// dashboard's "Live advisor" section). Reads only `~/.aida/advisor.toml` +
/// `.aida/config.toml [advisor]`; no requirement store. trace:STORY-559 | ai:claude
pub(crate) fn handle_advisor_registration_status(json: bool) -> Result<()> {
    let reg = advisor::read_registration();
    let cfg_root = find_project_root().ok();
    let cfg = match cfg_root.as_deref() {
        Some(root) => advisor::AdvisorConfig::load(root),
        None => advisor::AdvisorConfig::default(),
    };

    if json {
        let value = match &reg {
            Some(r) => {
                let advisor = locate_for_status(r);
                let (alive, size_bytes, est_cost) = match &advisor {
                    Some(a) => (
                        advisor::is_alive(a, r.claude_pid),
                        Some(a.jsonl_size_bytes),
                        Some(advisor::estimated_fork_cost_usd(a.jsonl_size_bytes)),
                    ),
                    None => (false, None, None),
                };
                serde_json::json!({
                    "registered": true,
                    "uuid": r.uuid,
                    "project_slug": r.project_slug,
                    "project_root": r.project_root,
                    "registered_at": r.registered_at,
                    "claude_pid": r.claude_pid,
                    "alive": alive,
                    "jsonl_size_bytes": size_bytes,
                    "estimated_fork_cost_usd": est_cost,
                    "fork_mode": match cfg.fork_mode {
                        advisor::ForkMode::Auto => "auto",
                        advisor::ForkMode::Always => "always",
                        advisor::ForkMode::Never => "never",
                    },
                })
            }
            None => serde_json::json!({
                "registered": false,
                "fork_mode": match cfg.fork_mode {
                    advisor::ForkMode::Auto => "auto",
                    advisor::ForkMode::Always => "always",
                    advisor::ForkMode::Never => "never",
                },
            }),
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    match reg {
        Some(r) => {
            println!("Advisor registration");
            println!("  uuid:          {}", r.uuid);
            println!("  project slug:  {}", r.project_slug);
            println!("  project root:  {}", r.project_root);
            println!("  registered at: {}", r.registered_at);
            if let Some(pid) = r.claude_pid {
                println!("  claude pid:    {}", pid);
            }
            let advisor = locate_for_status(&r);
            match advisor {
                Some(a) => {
                    let alive = advisor::is_alive(&a, r.claude_pid);
                    let mb = a.jsonl_size_bytes as f64 / (1024.0 * 1024.0);
                    let cost = advisor::estimated_fork_cost_usd(a.jsonl_size_bytes);
                    println!(
                        "  source jsonl:  {} ({:.2} MB)",
                        a.source_jsonl.display(),
                        mb
                    );
                    println!(
                        "  alive:         {}",
                        if alive {
                            "yes"
                        } else {
                            "no (cold-boot fallback)"
                        }
                    );
                    println!(
                        "  per-fork cost: ~${:.2} (first fork; subsequent within cache TTL ~$0.03)",
                        cost
                    );
                }
                None => {
                    println!("  source jsonl:  (missing — JSONL not found on disk)");
                    println!("  alive:         no (cold-boot fallback)");
                }
            }
        }
        None => {
            println!("No live advisor registered.");
            println!("The orchestrator will cold-boot the headless advisor for every punt.");
            println!("Run `aida advisor register` from inside your live advisor session to enable forking.");
        }
    }
    println!(
        "  fork_mode:     {} (set in .aida/config.toml [advisor])",
        match cfg.fork_mode {
            advisor::ForkMode::Auto => "auto",
            advisor::ForkMode::Always => "always",
            advisor::ForkMode::Never => "never",
        }
    );
    Ok(())
}

/// STORY-559: the advisor's read-only situational dashboard — the default
/// `aida advisor status`. PURE AGGREGATION of existing surfaces: every count
/// reuses the same query the dedicated command runs (no reimplemented counts),
/// and every row points at the canonical command to act on it. Read-only — no
/// writes, no auto-queue, no auto-approve. The narrow registration view (the
/// pre-dashboard output) lives behind `--registration`.
// trace:STORY-559 | ai:claude
pub(crate) fn handle_advisor_dashboard(
    json: bool,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // --- Live advisor (the former narrow output, now one section) ----------
    let reg = advisor::read_registration();
    let cfg = advisor::AdvisorConfig::load(&project_root);
    let fork_mode_label = match cfg.fork_mode {
        advisor::ForkMode::Auto => "auto",
        advisor::ForkMode::Always => "always",
        advisor::ForkMode::Never => "never",
    };
    let live = reg.as_ref().and_then(|r| {
        let a = locate_for_status(r)?;
        let alive = advisor::is_alive(&a, r.claude_pid);
        let cost = advisor::estimated_fork_cost_usd(a.jsonl_size_bytes);
        Some((a, alive, cost))
    });

    // --- Intake: drafts to triage (same status filter /aida-triage uses) ---
    // Reuses the cache-backed status-filter query — no reimplemented count.
    // The cache excludes archived rows by default.
    // BUG-593: exclude META + the other standing-artifact types the SAME way
    // `aida status` does (BUG-464 / TASK-773) — a fresh `aida init` seeds 6 META
    // prompts that are NOT intake work. The status draft-count already strips
    // them; the advisor surface must agree or it over-reports the inbox.
    // trace:BUG-593 | ai:claude
    let drafts = backend
        .list_summaries(&aida_core::ListFilter {
            status: Some("draft".to_string()),
            ..Default::default()
        })
        .map(|s| {
            s.iter()
                .filter(|r| !is_standing_artifact_type(&r.req_type))
                .count()
        })
        .unwrap_or(0);

    // --- Decisions: pending questions (reuses `aida questions` query) ------
    let pending_decisions = collect_decision_requests(backend)
        .map(|(pending, _)| pending.len())
        .unwrap_or(0);

    // --- Findings awaiting triage (reuses `aida findings list` query) ------
    // findings = draft requirements carrying a finding tag; plus the
    // NeedsAttention cohort (punts + shelvings), exactly as findings list sums.
    let findings_filter = aida_core::ListFilter {
        status: Some("draft".to_string()),
        ..Default::default()
    };
    let findings_total = backend
        .list_summaries(&findings_filter)
        .map(|s| {
            let sections = findings::build_findings_view(&s, &findings::FindingsFilter::default());
            findings::count_findings(&sections)
        })
        .unwrap_or(0);
    let needs_attention = backend
        .list_summaries(&aida_core::ListFilter {
            status: Some("needs-attention".to_string()),
            ..Default::default()
        })
        .map(|s| s.len())
        .unwrap_or(0);
    let findings_awaiting = findings_total + needs_attention;

    // --- Backlog: approved/planned/draft not queued (reuses `aida backlog`)
    let storage = Storage::new(store_path.to_path_buf());
    let user_id = current_user_id(None);
    let queued = queued_requirement_ids(&storage, &user_id).unwrap_or_default();
    let store = storage.load()?;
    let backlog = store
        .requirements
        .iter()
        .filter(|r| {
            !r.archived && r.status == RequirementStatus::Approved && !queued.contains(&r.id)
        })
        .count();

    // --- Burndown readiness (reuses the exact `aida burndown plan` resolver)
    let (ready, awaiting_signoff, _serialize_held, parked, _supervised, _titles) =
        resolve_burndown_sets("approved", None, None, None).unwrap_or_else(|_| {
            (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Default::default(),
            )
        });

    // --- Queue depth + in-flight (Done-awaiting-merge) ---------------------
    let queue_depth = backend
        .queue_list(&user_id, false)
        .map(|q| q.len())
        .unwrap_or(0);
    let in_flight_specs = store
        .requirements
        .iter()
        .filter(|r| matches!(r.status, RequirementStatus::Done))
        .count();

    // --- Live sessions: leases, flagging any held on a closed spec --------
    let leases = list_leases(&project_root);
    // Map each lease scope to a requirement status (best-effort: scope is a
    // raw string, often a SPEC-ID). Flag leases on archived/rejected/completed
    // specs — catches BUG-492-style states where work is held on dead specs.
    let stale_lease = |scope: &str| -> Option<String> {
        let scope = scope.trim();
        let r = store.requirements.iter().find(|r| {
            r.spec_id.as_deref() == Some(scope)
                || r.agreed_id.as_deref() == Some(scope)
                || r.display_id() == scope
        })?;
        if r.archived {
            return Some("archived".to_string());
        }
        match r.status {
            RequirementStatus::Rejected => Some("rejected".to_string()),
            RequirementStatus::Completed => Some("completed".to_string()),
            _ => None,
        }
    };

    if json {
        let leases_json: Vec<serde_json::Value> = leases
            .iter()
            .map(|l| {
                serde_json::json!({
                    "id": l.id,
                    "scope": l.scope,
                    "owner": l.owner,
                    "branch": l.branch,
                    "stale_spec_state": stale_lease(&l.scope),
                })
            })
            .collect();
        let live_json = match &live {
            Some((a, alive, cost)) => serde_json::json!({
                "registered": true,
                "alive": alive,
                "jsonl_size_bytes": a.jsonl_size_bytes,
                "estimated_fork_cost_usd": cost,
            }),
            None => serde_json::json!({
                "registered": reg.is_some(),
                "alive": false,
            }),
        };
        let value = serde_json::json!({
            "live_advisor": live_json,
            "fork_mode": fork_mode_label,
            "intake_drafts": drafts,
            "pending_decisions": pending_decisions,
            "findings_awaiting_triage": findings_awaiting,
            "backlog_not_queued": backlog,
            "burndown": {
                "ready": ready.len(),
                "awaiting_signoff": awaiting_signoff.len(),
                "parked": parked.len(),
            },
            "queue_depth": queue_depth,
            "in_flight": in_flight_specs,
            "live_sessions": leases_json,
        });
        println!("{}", crate::cache_output::json_pretty(&value)?);
        return Ok(());
    }

    println!("{}", "Advisor dashboard".bold());
    println!();

    // Live advisor
    println!("{}", "Live advisor".cyan().bold());
    match (&reg, &live) {
        (Some(_), Some((a, alive, cost))) => {
            let mb = a.jsonl_size_bytes as f64 / (1024.0 * 1024.0);
            println!(
                "  registered, {} — {} ({:.2} MB, ~${:.2}/fork)",
                if *alive {
                    "alive (fork-ready)".green().to_string()
                } else {
                    "not alive (cold-boot fallback)".yellow().to_string()
                },
                a.source_jsonl.display(),
                mb,
                cost
            );
        }
        (Some(_), None) => {
            println!(
                "  registered, but {} — source JSONL not found on disk",
                "cold-boot fallback".yellow()
            );
        }
        (None, _) => {
            println!(
                "  {} — orchestrator cold-boots every punt. Enable forking: {}",
                "no live advisor registered".dimmed(),
                "aida advisor register".cyan()
            );
        }
    }
    println!("  fork_mode: {fork_mode_label} (set in .aida/config.toml [advisor])");
    println!(
        "  Registration detail: {}",
        "aida advisor status --registration".dimmed()
    );
    println!();

    // Each remaining row: count + the command to act.
    println!("{}", "Intake".cyan().bold());
    println!(
        "  {drafts} draft(s) to triage → {}",
        "/aida-triage".dimmed()
    );
    println!();

    println!("{}", "Decisions".cyan().bold());
    println!(
        "  {pending_decisions} pending question(s) → {}",
        "aida questions".dimmed()
    );
    println!();

    println!("{}", "Findings".cyan().bold());
    println!(
        "  {findings_awaiting} awaiting triage (incl. shelved / needs-attention) → {}",
        "aida findings list".dimmed()
    );
    println!();

    println!("{}", "Backlog".cyan().bold());
    println!(
        "  {backlog} approved-not-queued → {} / {}",
        "aida backlog list".dimmed(),
        "groom".dimmed()
    );
    println!();

    println!("{}", "Burndown readiness".cyan().bold());
    println!(
        "  {} ready · {} awaiting sign-off · {} parked → {}",
        ready.len(),
        awaiting_signoff.len(),
        parked.len(),
        "aida burndown plan".dimmed()
    );
    println!();

    println!("{}", "Queue".cyan().bold());
    println!(
        "  depth {queue_depth} · {in_flight_specs} in-flight (Done — awaiting merge) → {}",
        "aida queue list".dimmed()
    );
    println!();

    println!("{}", "Live sessions".cyan().bold());
    if leases.is_empty() {
        println!(
            "  (no active sessions) → {}",
            "aida session leases".dimmed()
        );
    } else {
        for l in &leases {
            match stale_lease(&l.scope) {
                Some(state) => println!(
                    "  {} {} ({}) — {}",
                    l.scope,
                    l.owner.dimmed(),
                    l.branch.dimmed(),
                    format!(
                        "{} spec {state}",
                        crate::glyph(crate::glyphs::Glyph::Warning)
                    )
                    .yellow()
                ),
                None => println!("  {} {} ({})", l.scope, l.owner.dimmed(), l.branch.dimmed()),
            }
        }
        println!("  → {}", "aida session leases".dimmed());
    }

    Ok(())
}

/// STORY-347: which kind of advisor pass to spawn — a fresh cold-boot
/// `claude -p` (substrate only) or a fork-from-live `claude --resume` on a
/// copied JSONL. The orchestrator decides which up front; this enum is the
/// internal type that flows into `spawn_advisor_session`.
/// trace:STORY-347 | ai:claude
pub(crate) enum AdvisorPass {
    ColdBoot,
    Fork(advisor::ForkPlan),
}

/// STORY-347: resolve the effective calibration mode for the current
/// orchestrator run. The `AIDA_CALIBRATE=1`/`0` env var (set by the
/// `--calibrate`/`--no-calibrate` queue-work flags) overrides
/// `[advisor] calibration_mode` from `.aida/config.toml`.
/// trace:STORY-347 | ai:claude
pub(crate) fn effective_calibration_mode(cfg: &advisor::AdvisorConfig) -> advisor::CalibrationMode {
    match std::env::var("AIDA_CALIBRATE").ok().as_deref() {
        Some("1") | Some("true") | Some("on") | Some("yes") => advisor::CalibrationMode::On,
        Some("0") | Some("false") | Some("off") | Some("no") => advisor::CalibrationMode::Off,
        _ => cfg.calibration_mode,
    }
}
