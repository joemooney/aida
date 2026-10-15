//! Autopilot subcommands and `aida intake` (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// STORY-472: `aida release` — a memorable top-level verb over the dev release
/// flow. `--check` previews the planned release (read-only); otherwise it
/// delegates to `handle_dev_release`, forwarding `--skip-xplat-check` via the
/// env the release script honors. trace:STORY-472 | ai:claude
/// STORY-527: `aida burndown` dispatch.
/// STORY-560: `aida intake` — the headless advisor INTAKE pass. The
/// advisor-side analog of `handle_burndown_run`: load the `[intake]` policy +
/// flag overrides, self-load the store, compute the BOUNDED candidate fence
/// (the do-not-approve classes + `needs-human`/`strategic` specs are excluded
/// TASK-1147: the advisor-autopilot auditability + reversal surface.
///
/// This is the read-only + audit + reversal HALF of EPIC-0428. `inspect`
/// dry-runs the four-gate policy envelope over the live groom candidates and
/// shows, per spec, what it WOULD decide — without touching a single spec.
/// `audit` lists recorded verdicts; `challenge` reverses one. Autopilot has NO
/// authority to approve/reject/queue here — the envelope is never wired to
/// execute a disposition. That keystone-autonomy slice is deferred.
// trace:TASK-1147 | ai:claude
pub(crate) fn handle_autopilot_command(cmd: &crate::cli::AutopilotCommand) -> Result<()> {
    use crate::cli::AutopilotCommand;
    match cmd {
        AutopilotCommand::Inspect {
            risk,
            only_tag,
            exclude_tag,
            record,
            json,
        } => handle_autopilot_inspect(
            risk,
            only_tag.as_deref(),
            exclude_tag.as_deref(),
            *record,
            *json,
        ),
        AutopilotCommand::Audit { limit, open, json } => {
            handle_autopilot_audit(*limit, *open, *json)
        }
        AutopilotCommand::Challenge { target, note } => {
            handle_autopilot_challenge(target, note.as_deref())
        }
        // TASK-1018: the DURABLE half — what autopilot actually did, and the
        // one command that undoes it.
        AutopilotCommand::Executions {
            limit,
            open,
            from_product,
            mode,
            json,
        } => handle_autopilot_executions(*limit, *open, *from_product, mode.as_deref(), *json),
        AutopilotCommand::Revert {
            target,
            dry_run,
            note,
            json,
        } => handle_autopilot_revert(target, *dry_run, note.as_deref(), *json),
        AutopilotCommand::Reindex { dry_run } => handle_autopilot_reindex(*dry_run),
    }
}

/// `aida autopilot reindex` — refill the per-clone execution index from the
/// durable audit comments on the specs. The comments are the source of truth;
/// the index is rebuildable, exactly like `.aida/cache.db` vs the orphan-branch
/// YAML.
// trace:TASK-1018 | ai:claude
pub(crate) fn handle_autopilot_reindex(dry_run: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let indexed = autopilot_audit::read_executions(&project_root)?;
    let recovered = autopilot_audit::recover_from_comments(&project_root)?;
    let missing = autopilot_audit::reindex_missing(&indexed, &recovered);

    if missing.is_empty() {
        println!(
            "{} the index already holds every recorded autopilot action ({} recovered, {} indexed).",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            recovered.len(),
            indexed.len()
        );
        return Ok(());
    }
    if !dry_run {
        autopilot_audit::append_executions(&project_root, &missing)?;
    }
    println!(
        "{} {} {} action(s) recovered from the durable trail:",
        if dry_run {
            "·".dimmed().to_string()
        } else {
            crate::glyph(crate::glyphs::Glyph::Check)
                .green()
                .to_string()
        },
        if dry_run { "would restore" } else { "restored" },
        missing.len()
    );
    for rec in &missing {
        println!(
            "    {} {}  {:<12} {}",
            "→".green(),
            rec.id.cyan(),
            rec.spec_id,
            rec.action
        );
    }
    Ok(())
}

/// `aida autopilot executions` — list the actions autopilot actually EXECUTED
/// (as opposed to `audit`'s dry-run projection), newest last.
///
/// TASK-1013: `--from-product` narrows the list to the actions whose evidence
/// recorded a product handoff. Product input is evidence, never authority — the
/// filter exists so an operator can see at a glance whether a non-privileged
/// product seat is steering what autopilot dispositions.
///
/// TASK-1014: `--mode` narrows to the actions taken under one composition mode
/// — the supervision context (`autopilot` alone, `zen+autopilot`,
/// `solo+autopilot`) that was in effect when the action landed.
// trace:TASK-1018 trace:TASK-1013 trace:TASK-1014 | ai:claude
pub(crate) fn handle_autopilot_executions(
    limit: usize,
    open_only: bool,
    from_product: bool,
    mode: Option<&str>,
    json: bool,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let executions = autopilot_audit::read_executions(&project_root)?;
    let reversals = autopilot_audit::read_reversals(&project_root)?;

    let mut rows: Vec<&autopilot_audit::ExecutionRecord> = executions.iter().collect();
    if open_only {
        rows.retain(|e| !autopilot_audit::is_reversed(&reversals, &e.id));
    }
    // Narrow BEFORE the limit so `--from-product --limit N` means "the last N
    // product-sourced actions", not "the product-sourced ones among the last N".
    // trace:TASK-1013 | ai:claude
    if from_product {
        rows.retain(|e| autopilot_audit::is_from_product(e));
    }
    // Same rule for the composition filter: narrow first, cap second.
    // trace:TASK-1014 | ai:claude
    if let Some(wanted) = mode {
        rows.retain(|e| autopilot_audit::mode_matches(e, wanted));
    }
    if limit > 0 && rows.len() > limit {
        rows = rows.split_off(rows.len() - limit);
    }

    if json {
        let out: Vec<_> = rows
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "ts": e.ts,
                    "spec_id": e.spec_id,
                    "action": e.action,
                    "authority": e.authority,
                    "gate": e.gate,
                    "grounding": e.grounding,
                    "risk": e.risk,
                    "actor": e.actor,
                    "source": e.source,
                    "reason": e.reason,
                    "evidence": e.evidence,
                    // `mode` is the raw recorded field; `composition` is the
                    // mode the filter and the table report (the field, else the
                    // surface it is derived from on a pre-`mode` row).
                    // trace:TASK-1014 | ai:claude
                    "mode": e.mode,
                    "composition": autopilot_audit::record_mode(e),
                    // `from_product` is the raw recorded field; `is_from_product`
                    // is the filter's verdict (flag, else evidence markers), and
                    // `product` names the seat when the marker carries a who.
                    // trace:TASK-1013 | ai:claude
                    "from_product": e.from_product,
                    "is_from_product": autopilot_audit::is_from_product(e),
                    "product": autopilot_audit::product_provenance(&e.evidence),
                    // The composition of the two: product input consumed with
                    // nobody in the loop. Evidence only — no authority came with
                    // it — but it is the row to review first after an unattended
                    // drain. trace:TASK-1022 | ai:claude
                    "unattended_product": autopilot_audit::unattended_product_decision(e),
                    "prior": e.prior,
                    "reverted": autopilot_audit::is_reversed(&reversals, &e.id),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::json!(out))?);
        return Ok(());
    }

    if rows.is_empty() {
        if let Some(wanted) = mode {
            // trace:TASK-1014 | ai:claude
            println!(
                "{} no autopilot execution here ran under `{wanted}`.",
                "·".dimmed()
            );
        } else if from_product {
            println!(
                "{} no autopilot execution here acted on a product handoff.",
                "·".dimmed()
            );
        } else {
            println!(
                "{} autopilot has not executed any action here yet.",
                "·".dimmed()
            );
        }
        return Ok(());
    }

    println!(
        "{} autopilot executions — {} {}action(s)",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        rows.len(),
        // Name whatever narrowed the list, so a short table is never mistaken
        // for a quiet trail. trace:TASK-1014 | ai:claude
        match (from_product, mode) {
            (true, Some(m)) => format!("product-sourced {m} "),
            (true, None) => "product-sourced ".to_string(),
            (false, Some(m)) => format!("{m} "),
            (false, None) => "recorded ".to_string(),
        }
    );
    for e in &rows {
        let reverted = autopilot_audit::is_reversed(&reversals, &e.id);
        let mark = if reverted {
            "↩ reverted".yellow().to_string()
        } else {
            "· in effect".dimmed().to_string()
        };
        println!(
            "  {} {}  {:<12} {:<8} {:<9} {}  {}",
            e.id.cyan(),
            e.ts.dimmed(),
            e.spec_id,
            e.action,
            e.authority,
            e.source.dimmed(),
            mark
        );
        if let Some(restore) = e.prior.describe() {
            println!("      {}", format!("restores: {restore}").dimmed());
        }
        // Only a COMPOSED mode is annotated: the bare envelope is the default
        // and needs no comment, so calling it out on every row would bury the
        // rows where something else was steering. trace:TASK-1014 | ai:claude
        if let Some(composition) = autopilot_audit::mode_annotation(e) {
            println!("      {}", composition.blue());
        }
        // The handoff is visible on every listing, not just under the filter —
        // an operator scanning the trail should not have to know to ask.
        // trace:TASK-1013 | ai:claude
        if let Some(handoff) = autopilot_audit::product_annotation(e) {
            println!("      {}", handoff.magenta());
        }
        // Loudest of the three, and only when both layers are present: a
        // non-privileged seat's input was consumed with the least supervision
        // the system offers. The gates guarantee it granted nothing; this line
        // guarantees the operator can still SEE it.
        // trace:TASK-1022 | ai:claude
        if let Some(unattended) = autopilot_audit::unattended_product_annotation(e) {
            println!("      {}", unattended.yellow());
        }
    }
    println!(
        "\n  {} undo one with `aida autopilot revert <id|SPEC-ID>` (add --dry-run to preview).",
        "note:".dimmed()
    );
    Ok(())
}

/// `aida autopilot revert <target>` — the one-command reversal. Resolves the
/// execution record, derives the restore plan from it alone, applies it through
/// the same in-process paths the CLI verbs use, and records the reversal.
// trace:TASK-1018 | ai:claude
pub(crate) fn handle_autopilot_revert(
    target: &str,
    dry_run: bool,
    note: Option<&str>,
    json: bool,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let executions = autopilot_audit::read_executions(&project_root)?;
    let reversals = autopilot_audit::read_reversals(&project_root)?;

    let record = autopilot_audit::resolve_revert_target(&executions, &reversals, target)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let plan = autopilot_audit::plan_reversal(&record).map_err(|e| anyhow::anyhow!("{e}"))?;

    let applied = autopilot_audit::apply_reversal(&project_root, &plan, dry_run)?;
    if !dry_run {
        // A reversal is itself an audited event — durable comment first (the
        // trail that replicates), then the fast index.
        let actor = current_user_id(None);
        let entry =
            autopilot_audit::reversal_record(&chrono::Utc::now().to_rfc3339(), &plan, &actor, note);
        let storage = Storage::new(project_root.join(".aida-store"));
        if let Err(e) = comment_cmd::add_comment_cli(
            &storage,
            &plan.spec_id,
            &autopilot_audit::reversal_comment(&entry),
            Some("autopilot"),
            None,
        ) {
            eprintln!("  warning: the durable reversal comment could not be written: {e}");
        }
        autopilot_audit::append_reversals(&project_root, &[entry])?;
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "target": plan.target,
                "spec_id": plan.spec_id,
                "action": record.action,
                "dry_run": dry_run,
                "complete": plan.complete,
                "steps": applied,
            }))?
        );
        return Ok(());
    }

    let verb = if dry_run { "would restore" } else { "restored" };
    println!(
        "{} {} {} ({} {}) — {verb}:",
        if dry_run {
            "·".dimmed().to_string()
        } else {
            crate::glyph(crate::glyphs::Glyph::Check)
                .green()
                .to_string()
        },
        plan.target.cyan(),
        plan.spec_id.bold(),
        record.action,
        record.source.dimmed(),
    );
    for line in &applied {
        println!("    {} {line}", "→".green());
    }
    if !plan.complete {
        println!(
            "  {} the appended note stays on the record — an append-only trail is \
             retracted, never erased.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
    if dry_run {
        println!(
            "  {} nothing was written. Drop --dry-run to apply.",
            "·".dimmed()
        );
    }
    Ok(())
}

/// `aida autopilot inspect` — dry-run the envelope over the current groom
/// candidates. Reuses the SAME candidate fence as `aida groom`
/// (`select_intake_candidates`) so the projection can never disagree with the
/// disposition path about what is touchable.
// trace:TASK-1147 | ai:claude
pub(crate) fn handle_autopilot_inspect(
    risk: &str,
    only_tag: Option<&str>,
    exclude_tag: Option<&str>,
    record: bool,
    json: bool,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let cfg = intake::IntakeConfig::load(&project_root);
    let max_risk = backlog::RiskLevel::parse(risk)?;
    let filters = intake::IntakeFilters {
        only_tag: only_tag.map(|s| s.to_string()),
        exclude_tag: exclude_tag.map(|s| s.to_string()),
        max_risk,
    };

    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;
    let queued_ids = all_queued_requirement_ids(&project_root);

    // Build the same candidate facts `aida groom` builds — plus the proposed
    // ACTION per spec (Draft -> approve, Approved-but-unqueued -> queue), which
    // the envelope grades. trace:TASK-1147
    let mut specs: Vec<intake::IntakeSpec> = Vec::new();
    let mut proposed: std::collections::HashMap<String, autopilot::ActionClass> =
        std::collections::HashMap::new();
    for req in &store.requirements {
        if req.archived {
            continue;
        }
        if is_standing_artifact_type(&format!("{:?}", req.req_type)) {
            continue;
        }
        let is_draft = matches!(req.status, aida_core::RequirementStatus::Draft);
        let is_approved_unqueued = matches!(req.status, aida_core::RequirementStatus::Approved)
            && !queued_ids.contains(&req.id);
        if !(is_draft || is_approved_unqueued) {
            continue;
        }
        let disp = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        let has_plan = !find_plan_files_for_spec(&project_root, &disp).is_empty();
        let (risk, risk_reason) = backlog::classify_risk_with_reason(req, has_plan);
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        let predicted_files = backlog::collect_spec_files(&project_root, &disp);
        let deferred = intake::is_deferred(req.deferred, &tags);
        let action = if is_draft {
            autopilot::ActionClass::Approve
        } else {
            autopilot::ActionClass::Queue
        };
        proposed.insert(disp.clone(), action);
        specs.push(intake::IntakeSpec {
            id: disp,
            req_type: format!("{:?}", req.req_type).to_ascii_lowercase(),
            tags,
            trivial_footprint: intake::predicted_footprint_is_trivial(&predicted_files),
            deferred,
            risk,
            risk_reason,
        });
    }
    specs.sort_by(|a, b| a.id.cmp(&b.id));

    let (eligible, fenced) = intake::select_intake_candidates(&specs, &cfg, &filters);
    let fenced_ids: std::collections::HashSet<String> = eligible.iter().cloned().collect();
    let risk_of: std::collections::HashMap<&str, backlog::RiskLevel> =
        specs.iter().map(|s| (s.id.as_str(), s.risk)).collect();

    // The envelope: the conservative default table, widened only by an explicit
    // `[autopilot]` config posture, then TIGHTENED (demote-only) by the runtime
    // context — a headless run cannot pause-and-ask so its propose tier
    // escalates; an active solo posture inherits the drain's safe/keystone
    // partition. `effective_envelope` is the ratified TASK-0432 precedence
    // contract, so this dry-run grades under exactly the envelope a real groom
    // in this context would. No auto-execution is wired regardless.
    // trace:TASK-1020 | ai:claude
    let config_text =
        std::fs::read_to_string(project_root.join(".aida").join("config.toml")).unwrap_or_default();
    let overrides = autopilot::parse_authority_overrides(&config_text);
    // trace:TASK-1022 | ai:claude — one reader of the flag, so the envelope
    // tightening and the audit trail's `headless` layer cannot disagree.
    let headless = autopilot::current_headless();
    let solo_active = presence::current_solo(chrono::Utc::now());
    // The candidates graded below are post-fence: gate 1 already excluded
    // keystone via the same `is_keystone_class` classifier, so an active solo
    // resolves to its safe partition here.
    let posture = presence::resolve_solo_posture(solo_active, false);
    let env = autopilot::effective_envelope(
        autopilot::AutopilotEnvelope::default().with_overrides(overrides),
        headless,
        posture,
    );

    // Build one Decision per eligible spec. Grounding is the advisor's runtime
    // judgment (it needs the live corpus read); the dry-run assumes the
    // best case — a groundable Type-A — so the projection reflects what the
    // OTHER gates would do. The verdict is therefore an UPPER BOUND on autonomy.
    let decisions: Vec<autopilot::Decision> = eligible
        .iter()
        .map(|id| {
            let action = proposed
                .get(id)
                .copied()
                .unwrap_or(autopilot::ActionClass::Approve);
            autopilot::Decision {
                spec_id: id.clone(),
                action,
                grounding: autopilot::Grounding::TypeA,
                risk: risk_of.get(id.as_str()).copied().unwrap_or(max_risk),
                reason: String::new(),
                evidence: vec![],
            }
        })
        .collect();
    let rows = autopilot::project_decisions(&env, &fenced_ids, &decisions);

    if json {
        let eligible_json: Vec<_> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "spec_id": r.spec_id,
                    "action": r.action.token(),
                    "verdict": r.outcome.verdict_token(),
                    "gate": r.outcome.gate_label(),
                })
            })
            .collect();
        let fenced_json: Vec<_> = fenced
            .iter()
            .map(|(id, reason)| serde_json::json!({ "spec_id": id, "fenced": reason.describe() }))
            .collect();
        let out = serde_json::json!({
            "risk_ceiling": max_risk.token(),
            "grounding_assumed": "type-a (best case; advisor grounds at run time)",
            "auto_execution": false,
            // TASK-1020: the runtime context the effective envelope was
            // composed under (demote-only tightening; autonomy doc §8).
            "context": { "headless": headless, "solo": solo_active },
            "eligible": eligible_json,
            "fenced": fenced_json,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        // Recording still honored under --json.
        if record {
            record_inspect_rows(&project_root, &rows)?;
        }
        return Ok(());
    }

    println!(
        "{} autopilot inspect — dry-run (nothing is written to any spec)",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    println!(
        "  {}",
        format!(
            "envelope: risk≤{} · grounding=required (assumed groundable) · approve={} reject={}",
            max_risk.token(),
            env.authority_for(autopilot::ActionClass::Approve).token(),
            env.authority_for(autopilot::ActionClass::Reject).token(),
        )
        .dimmed()
    );
    // TASK-1020: name the composition context whenever it tightened (or could
    // tighten) the envelope, so a headless/solo dry-run is legibly different
    // from the keyboard one.
    if headless || solo_active {
        let mut parts: Vec<&str> = Vec::new();
        if headless {
            parts.push("headless — propose-tier escalates (cannot pause-and-ask)");
        }
        if solo_active {
            parts.push("solo — safe partition (keystone already fenced at gate 1)");
        }
        println!(
            "  {}",
            format!(
                "context: {} · tightening is demote-only (autonomy doc §8)",
                parts.join(" · ")
            )
            .dimmed()
        );
    }

    if rows.is_empty() {
        println!(
            "  {} 0 specs in the fence ({} fenced out). Nothing for the envelope to weigh.",
            "→".green(),
            fenced.len()
        );
    } else {
        println!(
            "\n  {} envelope verdict per eligible candidate:",
            "→".green()
        );
        for r in &rows {
            let (glyph, label) = match r.outcome {
                autopilot::Outcome::Execute => (
                    crate::glyph(crate::glyphs::Glyph::Check)
                        .green()
                        .to_string(),
                    "EXECUTE".green().bold(),
                ),
                autopilot::Outcome::Hold => ("⏸".yellow().to_string(), "HOLD".yellow().bold()),
                autopilot::Outcome::Escalate(_) => (
                    crate::glyph(crate::glyphs::Glyph::Warning)
                        .red()
                        .to_string(),
                    "ESCALATE".red().bold(),
                ),
            };
            println!(
                "    {} {:<12} {:<8} {}  {}",
                glyph,
                r.spec_id,
                r.action.token(),
                label,
                r.outcome.gate_label().dimmed()
            );
        }
    }

    if !fenced.is_empty() {
        println!(
            "\n  {} {} fenced out at gate 1 (never touchable — the authority map cannot widen this):",
            "·".dimmed(),
            fenced.len()
        );
        for (id, reason) in &fenced {
            println!(
                "    {} {} — {}",
                "✕".dimmed(),
                id,
                reason.describe().dimmed()
            );
        }
    }

    if record {
        let n = record_inspect_rows(&project_root, &rows)?;
        println!(
            "\n  {} recorded {} verdict(s) to {}",
            "→".green(),
            n,
            autopilot::audit_log_path(&project_root).display()
        );
    }

    println!(
        "\n  {} projection only — autopilot has NO authority to approve/reject/queue here; \n     that keystone-autonomy is deliberately not wired. Review with `aida autopilot audit`,\n     reverse with `aida autopilot challenge <id>`.",
        "note:".dimmed()
    );
    Ok(())
}

/// Append the inspected verdicts to the local audit log; returns the count.
pub(crate) fn record_inspect_rows(
    project_root: &std::path::Path,
    rows: &[autopilot::InspectRow],
) -> Result<usize> {
    let ts = chrono::Utc::now().to_rfc3339();
    let entries: Vec<autopilot::AuditEntry> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            autopilot::decision_entry(
                &ts, i, &r.spec_id, r.action, r.outcome, &r.reason, "inspect",
            )
        })
        .collect();
    autopilot::append_audit_entries(project_root, &entries)?;
    Ok(entries.len())
}

/// `aida autopilot audit` — list recorded decisions (and their reversals).
// trace:TASK-1147 | ai:claude
pub(crate) fn handle_autopilot_audit(limit: usize, open_only: bool, json: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let entries = autopilot::read_audit_entries(&project_root)?;

    // Fold challenges onto the decisions they target.
    let mut decisions: Vec<&autopilot::AuditEntry> =
        entries.iter().filter(|e| e.kind == "decision").collect();
    if open_only {
        decisions.retain(|d| !autopilot::is_challenged(&entries, &d.id));
    }
    if limit > 0 && decisions.len() > limit {
        decisions = decisions.split_off(decisions.len() - limit);
    }

    if json {
        let rows: Vec<_> = decisions
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "ts": d.ts,
                    "spec_id": d.spec_id,
                    "action": d.action,
                    "verdict": d.verdict,
                    "gate": d.gate,
                    "source": d.source,
                    "challenged": autopilot::is_challenged(&entries, &d.id),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!(rows))?
        );
        return Ok(());
    }

    if decisions.is_empty() {
        println!(
            "{} no recorded autopilot decisions yet. Run `aida autopilot inspect --record` to log a dry-run.",
            "·".dimmed()
        );
        return Ok(());
    }

    println!(
        "{} autopilot audit — {} recorded decision(s)",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        decisions.len()
    );
    for d in &decisions {
        let challenged = autopilot::is_challenged(&entries, &d.id);
        let mark = if challenged {
            "⨯ CHALLENGED".red().to_string()
        } else {
            "· open".dimmed().to_string()
        };
        println!(
            "  {} {}  {:<12} {:<8} {:<9} {}  {}",
            d.id.cyan(),
            d.ts.dimmed(),
            d.spec_id.as_deref().unwrap_or("-"),
            d.action.as_deref().unwrap_or("-"),
            d.verdict.as_deref().unwrap_or("-"),
            d.gate.as_deref().unwrap_or("-").dimmed(),
            mark
        );
    }
    Ok(())
}

/// `aida autopilot challenge <target>` — record a reversal of a decision.
// trace:TASK-1147 | ai:claude
pub(crate) fn handle_autopilot_challenge(target: &str, note: Option<&str>) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let entries = autopilot::read_audit_entries(&project_root)?;

    let Some(decision_id) = autopilot::resolve_challenge_target(&entries, target) else {
        anyhow::bail!(
            "no matching autopilot decision for `{target}` — pass a decision id from \
             `aida autopilot audit`, or a SPEC-ID with an un-challenged recorded decision."
        );
    };
    if autopilot::is_challenged(&entries, &decision_id) {
        println!(
            "{} decision {} is already challenged — nothing to do.",
            "·".dimmed(),
            decision_id.cyan()
        );
        return Ok(());
    }
    let ts = chrono::Utc::now().to_rfc3339();
    let entry = autopilot::challenge_entry(&ts, &decision_id, note);
    autopilot::append_audit_entries(&project_root, &[entry])?;
    println!(
        "{} challenged autopilot decision {}{}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        decision_id.cyan(),
        note.map(|n| format!(" — {n}")).unwrap_or_default()
    );
    Ok(())
}

/// STORY-560: `aida groom` self-loads the store, computes its candidate fence
/// (P1/P2/P3 policy + always-on tag exclusions + keystone/deferred fences applied
/// HERE, programmatically — the agent never sees them as actionable), then
/// launch a headless `claude -p "/aida-assess [--apply]"` that reads the fenced
/// set, proposes dispositions, and (under `--apply`) approves within the fence
/// and grooms the queue. Propose-mode is the ultimate gate.
/// trace:STORY-560 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_intake_command(
    apply: bool,
    max_approvals: Option<usize>,
    only_tag: Option<&str>,
    exclude_tag: Option<&str>,
    risk: &str,
    then_drain: bool,
    dry_run: bool,
    permission_mode: Option<&str>,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let cfg = intake::IntakeConfig::load(&project_root);
    let max_risk = backlog::RiskLevel::parse(risk)?;
    let filters = intake::IntakeFilters {
        only_tag: only_tag.map(|s| s.to_string()),
        exclude_tag: exclude_tag.map(|s| s.to_string()),
        max_risk,
    };

    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;
    let queued_ids = all_queued_requirement_ids(&project_root);

    // The intake action set = open specs the agent could move forward: Drafts
    // (approve/reject/park candidates) + Approved-but-not-queued (queue
    // candidates). Build the IntakeSpec facts for each. trace:STORY-560
    let mut specs: Vec<intake::IntakeSpec> = Vec::new();
    for req in &store.requirements {
        if req.archived {
            continue;
        }
        // BUG-593: standing-artifact types (meta / folder / vision / principle /
        // term / constraint) are perpetual reference rows, NOT intake work — the
        // same set `aida status` / `aida advisor` / `aida list` exclude. A fresh
        // `aida init` seeds 6 META prompts; without this they surfaced as assess
        // candidates the cold-boot advisor could approve/queue. trace:BUG-593
        if is_standing_artifact_type(&format!("{:?}", req.req_type)) {
            continue;
        }
        let is_draft = matches!(req.status, aida_core::RequirementStatus::Draft);
        let is_approved_unqueued = matches!(req.status, aida_core::RequirementStatus::Approved)
            && !queued_ids.contains(&req.id);
        if !(is_draft || is_approved_unqueued) {
            continue;
        }
        let disp = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        let has_plan = !find_plan_files_for_spec(&project_root, &disp).is_empty();
        // BUG-595: carry the per-spec risk REASON so the assess fence is legible
        // (a fenced spec shows WHY, not just an opaque chip). trace:BUG-595
        let (risk, risk_reason) = backlog::classify_risk_with_reason(req, has_plan);
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        let predicted_files = backlog::collect_spec_files(&project_root, &disp);
        // BUG-561: mirror the `aida list` honor-both deferred predicate
        // (STORY-584) so the operator's deferral shelf is fenced out, not
        // re-blessed. trace:BUG-561 | ai:claude
        let deferred = intake::is_deferred(req.deferred, &tags);
        specs.push(intake::IntakeSpec {
            id: disp,
            req_type: format!("{:?}", req.req_type).to_ascii_lowercase(),
            tags,
            trivial_footprint: intake::predicted_footprint_is_trivial(&predicted_files),
            deferred,
            risk,
            risk_reason,
        });
    }
    // Deterministic output order.
    specs.sort_by(|a, b| a.id.cmp(&b.id));

    let (eligible, fenced) = intake::select_intake_candidates(&specs, &cfg, &filters);

    let effective_on_apply = if then_drain {
        intake::OnApply::Drain
    } else {
        cfg.on_apply
    };

    println!(
        "{} intake pass",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    println!(
        "  {}",
        format!(
            "bias={} · do-not-approve=[{}] · on_apply={} · risk≤{}",
            cfg.disposition_bias.as_str(),
            cfg.do_not_approve_classes.join(","),
            effective_on_apply.as_str(),
            max_risk.token(),
        )
        .dimmed()
    );

    if eligible.is_empty() {
        println!(
            "  {} 0 specs in the intake fence ({} fenced out). Nothing for the advisor to weigh.",
            "→".green(),
            fenced.len()
        );
        return Ok(());
    }

    println!(
        "  {} {} spec(s) the advisor will weigh: {}",
        "→".green(),
        eligible.len(),
        eligible.join(", ").cyan()
    );
    let trivial_proposals: Vec<String> = specs
        .iter()
        .filter(|spec| eligible.contains(&spec.id) && intake::proposes_trivial_lifecycle(spec))
        .map(|spec| spec.id.clone())
        .collect();
    for id in &trivial_proposals {
        println!("  {}", intake::trivial_lifecycle_proposal_line(id));
    }
    if !fenced.is_empty() {
        println!(
            "  {} {} fenced out (do-not-approve class / needs-human / keystone / deferred / tag / risk):",
            "·".dimmed(),
            fenced.len()
        );
        for (id, reason) in &fenced {
            println!(
                "    {} {} — {}",
                "✕".dimmed(),
                id,
                reason.describe().dimmed()
            );
        }
    }

    // Seed the cold-boot prompt with the live advisor's context file when present,
    // so the headless advisor doesn't re-derive priorities from scratch. trace:STORY-626
    let prompt = intake::seeded_assess_prompt(&project_root, apply);
    let mode = permission_mode.unwrap_or("bypassPermissions");

    if dry_run {
        println!("\n{} dry run — not launching. Would run:", "·".dimmed());
        println!("  claude -p {:?} --permission-mode {}", prompt, mode.cyan());
        println!(
            "  {} AIDA_INTAKE_CANDIDATES={}",
            "env".dimmed(),
            eligible.join(",")
        );
        return Ok(());
    }

    if permission_mode.is_none() {
        println!(
            "  {} launching headless `claude -p` with {} permissions (override: --permission-mode <mode>)",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            "BYPASSED".yellow().bold()
        );
    }
    if apply {
        println!(
            "  {} {} — the advisor APPROVES within the fence + grooms the queue{}",
            "→".green(),
            "--apply".yellow().bold(),
            if effective_on_apply == intake::OnApply::Drain {
                ", then DRAINS"
            } else {
                ""
            }
        );
    } else {
        println!(
            "  {} propose-mode — writes NOTHING; review the proposal, then re-run with --apply",
            "→".green()
        );
    }
    println!();

    // The skill reads the resolved policy + the bounded fence from env; the
    // prompt stays the human-facing surface (mirrors the advisor tier's
    // env-passed payload). trace:STORY-560
    let child_grant =
        seat_authority::issue_child(&project_root, "advisor", &current_user_id(None))?;
    let mut command = build_autopilot_intake_command(
        &prompt,
        mode,
        &child_grant.id,
        apply,
        &eligible,
        &trivial_proposals,
        &cfg,
        effective_on_apply.as_str(),
        max_risk,
        max_approvals,
    );

    let status_code = command.status().map_err(|e| {
        anyhow::anyhow!(
            "failed to launch `claude -p` ({e}) — the headless intake pass needs the Claude Code \
             CLI on PATH. Install it, or run `/aida-backlog-groom` interactively instead."
        )
    })?;

    if status_code.success() {
        Ok(())
    } else {
        let code = status_code.code().unwrap_or(1);
        std::process::exit(code);
    }
}

// trace:BUG-2124 | ai:antigravity
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_autopilot_intake_command(
    prompt: &str,
    mode: &str,
    child_grant_id: &str,
    apply: bool,
    eligible: &[String],
    trivial_proposals: &[String],
    cfg: &intake::IntakeConfig,
    effective_on_apply: &str,
    max_risk: backlog::RiskLevel,
    max_approvals: Option<usize>,
) -> std::process::Command {
    let mut command = std::process::Command::new("claude");
    command
        .arg("-p")
        .arg(prompt)
        .arg("--permission-mode")
        .arg(mode)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env(seat_authority::GRANT_ENV, child_grant_id)
        .env("AIDA_INTAKE_APPLY", if apply { "1" } else { "0" })
        .env("AIDA_INTAKE_CANDIDATES", eligible.join(","))
        .env("AIDA_INTAKE_TRIVIAL_PROPOSALS", trivial_proposals.join(","))
        .env(
            "AIDA_INTAKE_DISPOSITION_BIAS",
            cfg.disposition_bias.as_str(),
        )
        .env(
            "AIDA_INTAKE_DO_NOT_APPROVE_CLASSES",
            cfg.do_not_approve_classes.join(","),
        )
        .env("AIDA_INTAKE_ON_APPLY", effective_on_apply)
        .env("AIDA_INTAKE_RISK", max_risk.token());
    if let Some(n) = max_approvals {
        command.env("AIDA_INTAKE_MAX_APPROVALS", n.to_string());
    }
    if crate::session::containment_applied(crate::session::HeadlessVendor::Claude, false) {
        command.env("AIDA_CONTAINED", "1");
    } else {
        command.env_remove("AIDA_CONTAINED");
    }
    command
}
