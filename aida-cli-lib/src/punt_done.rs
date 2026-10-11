//! `aida punt`/`aida done`/`aida punts` handlers, mention notifications and the completion crescendo (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

pub(crate) fn locate_for_status(
    reg: &advisor::AdvisorRegistration,
) -> Option<advisor::LiveAdvisor> {
    let home = crate::home_dir()?;
    let jsonl = home
        .join(".claude")
        .join("projects")
        .join(&reg.project_slug)
        .join(format!("{}.jsonl", reg.uuid));
    let meta = std::fs::metadata(&jsonl).ok()?;
    Some(advisor::LiveAdvisor {
        uuid: reg.uuid.clone(),
        project_slug: reg.project_slug.clone(),
        source_jsonl: jsonl,
        jsonl_size_bytes: meta.len(),
        discovery: advisor::Discovery::Registration,
    })
}

/// `aida punt` — pause a spec in `NeedsAttention` with a structured reason
/// instead of guessing past a design-fork (STORY-332). The transition is
/// enforced (`InProgress → NeedsAttention` only); a punt record is appended
/// to the ledger; control returns immediately so an orchestrator advances.
/// trace:STORY-332 | ai:claude
pub(crate) fn handle_punt_command(
    id: &str,
    category: &str,
    reason: &str,
    lean: Option<&str>,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let category = punt::parse_punt_category(category).map_err(|e| anyhow::anyhow!(e))?;

    let mut req = backend
        .get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());

    // STORY-332: a spec can only be punted out of In Progress — punting is
    // the "I was working this and hit a fork" move. This is the same edge
    // `forbidden_attention_transition` guards for `aida edit`, asserted here
    // directly so the error is punt-shaped.
    if forbidden_attention_transition(&req.status, &RequirementStatus::NeedsAttention).is_some() {
        anyhow::bail!(
            "{display_id} is {} — only an In Progress spec can be punted \
             (punting is the \"I was working this and hit a fork\" move). \
             Take it to In Progress first if you mean to pause it.",
            req.status
        );
    }

    let raised_by = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|r| !r.is_empty());
    let now = chrono::Utc::now();

    // BUG-1637: the CLI punt is caller-authored, recorded under the caller.
    // trace:BUG-1637 | ai:claude
    let from = std::mem::replace(&mut req.status, RequirementStatus::NeedsAttention);
    record_caller_status_transition(&mut req, &from);
    req.attention_reason = Some(AttentionReason {
        category,
        detail: reason.to_string(),
        lean: lean.map(|s| s.to_string()),
        raised_by: raised_by.clone(),
        raised_at: now,
    });
    req.modified_at = now;
    backend.update_requirement(&req)?;
    record_role_activity(&display_id, "punt");

    // Append the punt ledger record (`.aida/punts.jsonl`). Best-effort — a
    // ledger-write failure must not undo the status flip, which is the
    // load-bearing part; warn and continue. trace:STORY-325 | ai:claude
    if let Ok(project_root) = find_project_root() {
        let record = punt::PuntRecord {
            timestamp: now,
            spec: display_id.clone(),
            category,
            detail: reason.to_string(),
            lean: lean.map(|s| s.to_string()),
            raised_by: raised_by.clone(),
            resolution_path: "punted".to_string(),
            // STORY-306 advisor fields — a plain implementer punt the advisor
            // has not yet judged; the orchestrator's advisor tier fills these.
            classification: None,
            escalation_reason: None,
            answer: None,
            answered_by: None,
            decision: None,
            principle_link: None,
            calibration_pair: None,
            paused_at: Some(now),
            resolved_at: None,
        };
        if let Err(e) = punt::append_to_ledger(&project_root, &record) {
            eprintln!(
                "{} could not write punt ledger record: {e}",
                "Note:".dimmed()
            );
        }
    }

    // STORY-276: when an `--auto-complete` orchestrator launched this session
    // it set `AIDA_PUNT_SIGNAL_FILE` to a path it watches. Drop the signal
    // there so the orchestrator learns the spec was punted — not shipped —
    // once the implementer session exits. Best-effort: a missing var means a
    // standalone `aida punt` with no orchestrator, and a write failure must
    // not undo the status flip. trace:STORY-276 | ai:claude
    if let Some(signal_path) = std::env::var(punt::SIGNAL_FILE_ENV)
        .ok()
        .filter(|s| !s.is_empty())
    {
        let signal = punt::PuntSignal {
            spec: display_id.clone(),
            category,
            detail: reason.to_string(),
            lean: lean.map(|s| s.to_string()),
        };
        if let Err(e) = punt::write_signal(std::path::Path::new(&signal_path), &signal) {
            eprintln!("{} could not write punt signal file: {e}", "Note:".dimmed());
        }
    }

    println!(
        "{} {display_id} → {}",
        "Punted".magenta().bold(),
        "Needs Attention".magenta().bold()
    );
    println!("  {}  {category}", "Category:".dimmed());
    println!("  {}    {reason}", "Reason:".dimmed());
    if let Some(l) = lean {
        println!("  {}      {l}", "Lean:".dimmed());
    }
    println!(
        "{}",
        "The spec is paused awaiting triage. An orchestrator can continue to \
         the next item."
            .dimmed()
    );
    println!(
        "{}",
        format!(
            "Triage: `aida findings` lists it · resume with \
             `aida edit {display_id} --status in-progress` · \
             drop with `--status rejected`."
        )
        .dimmed()
    );

    // TASK-349: a blocked-dependency punt names the work it's blocked on — that
    // dependency belongs in the requirement graph as a blocked-by edge, not
    // just in the punt prose. If the reason/lean text named blocker spec(s),
    // nudge the operator to record the edge. A suggestion, not an auto-file:
    // id-parsing from free text is best-effort, so the operator confirms.
    // trace:TASK-349 | ai:claude
    if let Some(suggestion) = punt::suggest_blocked_by(&display_id, category, reason, lean) {
        let blockers = suggestion.blockers.join(", ");
        println!(
            "{} Looks blocked on {blockers} — record it in the graph so \
             `aida graph blocked-by {display_id}` sees the dependency:",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        println!("  {}", suggestion.suggested_command().cyan());
    }

    Ok(())
}

/// Send a best-effort notification message into `recipient`'s mailbox (the
/// fast local layer; STORY-643 auto-sync propagates it to other clones on the
/// next pull/push). Reuses the existing message-send path — no new notification
/// system. Failures are swallowed with a dimmed warning so a mailbox problem
/// never breaks the verb that triggered the notice (assign / comment).
/// trace:STORY-644 | ai:claude
pub(crate) fn send_notification(
    store_path: &std::path::Path,
    sender: &str,
    recipient: &str,
    body: String,
) {
    use aida_core::mailbox::{Intent, Message, Recipient};
    let project_root = match store_path.parent() {
        Some(p) => p,
        None => return,
    };
    let id = uuid::Uuid::new_v4().to_string();
    let msg = Message {
        subject: None,
        id: id.clone(),
        thread_id: id,
        from: sender.to_string(),
        to: Recipient::Agent(recipient.to_string()),
        timestamp: chrono::Utc::now().timestamp_millis(),
        in_reply_to: None,
        body,
        urgent: false,
        intent: Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        // The caller names `sender` explicitly (a fixed system/CLI identity,
        // e.g. "web", "aida-session-reap"), not an ambiguous env fallback.
        // trace:BUG-1533 | ai:claude
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    };
    if let Err(e) = mailbox_store::write_message(project_root, &msg) {
        eprintln!(
            "  {}",
            format!("(could not send mailbox notice to {recipient}: {e})").dimmed()
        );
    }
}

/// Extract distinct `@mention` handles from free text (STORY-644). Conservative
/// word-boundary parse: a `@` that is NOT preceded by a word char (so `foo@bar`
/// email locals and `a@b` are ignored) followed by `[A-Za-z0-9_.-]+`. The
/// trailing run is trimmed of `.` and `-` so sentence punctuation (`@bob.`) and
/// hyphen-tails don't leak into the handle. Returns handles in first-seen order,
/// deduped. trace:STORY-644 | ai:claude
pub(crate) fn extract_mentions(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '@' {
            // Word-boundary: skip `foo@bar` (email local-part / handle-in-word).
            let prev_is_word = i > 0 && {
                let p = chars[i - 1];
                p.is_alphanumeric() || p == '_'
            };
            if !prev_is_word {
                let mut j = i + 1;
                while j < chars.len() {
                    let c = chars[j];
                    if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
                        j += 1;
                    } else {
                        break;
                    }
                }
                let raw: String = chars[i + 1..j].iter().collect();
                // Trim trailing sentence punctuation so `@bob.` -> `bob`.
                let handle = raw.trim_end_matches(['.', '-']).to_string();
                if !handle.is_empty() && !out.contains(&handle) {
                    out.push(handle);
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// One-line snippet of `text` for a mention notice: collapsed whitespace,
/// truncated to `max` chars with an ellipsis. trace:STORY-644 | ai:claude
pub(crate) fn mention_snippet(text: &str, max: usize) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// `aida done <SPEC>` — the newcomer's "I finished it" verb. Marks a spec
/// Completed, the solo end of the capture → build → done loop. Found running a
/// novice's first session: there was no `aida done`, and `aida edit --status
/// completed` is jargon (and authority-gated off a TTY). A human at a terminal
/// IS the authority (TTY satisfies the advisor gate), so for a solo user this
/// just works. trace:TASK-727 | ai:claude
pub(crate) fn handle_done_command(
    id: &str,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    use aida_core::RequirementStatus;
    let mut req = backend
        .get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());
    if matches!(req.status, RequirementStatus::Completed) {
        println!(
            "{} {display_id} is already done.",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold()
        );
        return Ok(());
    }
    if status_advance_requires_advisor_authority(&req.status, &RequirementStatus::Completed)
        && !has_advisor_authority()
    {
        // BUG-585: name the ACTUAL escape hatches, not a circular re-run of the
        // same command. A non-TTY agent/script gets nothing new by re-running
        // `aida done`; the working path is a valid session grant established
        // through `aida role enter advisor` at a human TTY.
        // trace:BUG-585 | ai:claude
        anyhow::bail!(
            "marking {display_id} done needs advisor authority. Run `aida role enter advisor` at an interactive TTY, or ask an advisor session to do it."
        );
    }
    // BUG-1286 F1: `aida done` is an into-Completed transition and must emit the
    // durable ship record. STORY-1418: the stamp, the persist and the emission
    // go through the one seam so the rule and the call live in one place.
    // trace:BUG-1286 trace:STORY-1418 | ai:claude
    completion::transition_to_completed(
        &mut req,
        store_path.parent(),
        &display_id,
        "",
        "done",
        |req, prior| {
            // BUG-1637: caller-authored (same identity as before), through the
            // one shared history helper. trace:BUG-1637 | ai:claude
            aida_core::conflict::record_status_transition(req, &current_user_id(None), prior);
            backend.update_requirement(req)?;
            Ok(())
        },
    )?;
    record_role_activity(&display_id, "done");
    // STORY-738: `aida done` is always an into-Completed transition (the
    // already-Completed case returned early above), so the human path gets
    // the completion crescendo instead of the flat check-mark `done` line. The
    // agent/TOON surface keeps the terse machine line. trace:STORY-738
    if agent_output_mode() {
        println!(
            "{} {display_id} — {}",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            "done".green()
        );
    } else {
        render_completion_crescendo(&display_id, &req.title);
    }
    Ok(())
}

/// STORY-738: the completion crescendo. When a spec transitions INTO
/// Completed — the payoff state the thought→merged arc exists for — render a
/// distinct, felt close instead of the flat generic `Updated: <id>` line
/// (which is reused for any tag edit) or the bare check-mark `done` line. The
/// render names the spec + title, shows the arc terminating (filed → built →
/// merged → completed, the loop closed), and points forward to `aida show
/// <id>` for the commit that landed it. Honors STORY-700 AC3 (BINDING): the
/// completion is the felt moment, not a generic done message. Glyphs come
/// from the registry so it stays ascii-safe; the `aida show <id>` breadcrumb
/// is the one deliberate SPEC-ID in the copy (the caller just typed the id,
/// so naming it matches the done/Updated house style).
// trace:STORY-738 | ai:claude
pub(crate) fn render_completion_crescendo(display_id: &str, title: &str) {
    for line in completion_crescendo_lines(display_id, title) {
        println!("{line}");
    }
}

/// Pure renderer for [`render_completion_crescendo`] — returns the rendered
/// (colored) lines so the felt elements are unit-testable without capturing
/// stdout.
// trace:STORY-738 | ai:claude
pub(crate) fn completion_crescendo_lines(display_id: &str, title: &str) -> Vec<String> {
    use crate::glyphs::Glyph;
    let check = crate::glyph(Glyph::Check);
    let arrow = crate::glyph(Glyph::Arrow);
    let sub = crate::glyph(Glyph::SubArrow);
    let mut lines = Vec::new();
    lines.push(format!(
        "{} {} reached {} — the loop closed.",
        check.green().bold(),
        display_id.bold(),
        "Completed".green().bold()
    ));
    let title = title.trim();
    if !title.is_empty() {
        lines.push(format!("  {}", title.dimmed()));
    }
    let arc = format!("filed {arrow} built {arrow} merged {arrow} completed");
    lines.push(format!("  {} {}", sub.dimmed(), arc.dimmed()));
    lines.push(format!(
        "  {} {}  to see the commit that landed it.",
        sub.dimmed(),
        format!("aida show {display_id}").cyan()
    ));
    lines
}

/// STORY-738: is this status edit a transition INTO Completed? True only when
/// the prior status was something other than Completed and the new canonical
/// status is `Completed` — a no-op re-set of an already-Completed spec is not
/// a crescendo moment.
// trace:STORY-738 | ai:claude
pub(crate) fn is_into_completed_transition(
    old: &aida_core::RequirementStatus,
    new_canonical: &str,
) -> bool {
    !matches!(old, aida_core::RequirementStatus::Completed) && new_canonical == "Completed"
}

/// STORY-738: which render the `aida edit` status-change path emits. The
/// crescendo fires only on a true into-Completed transition AND only on the
/// human surface; the agent/TOON path and every non-completion edit keep the
/// flat Updated line.
// trace:STORY-738 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EditCompletionRender {
    Crescendo,
    Updated,
}

pub(crate) fn edit_completion_render(
    into_completed: bool,
    agent_mode: bool,
) -> EditCompletionRender {
    if into_completed && !agent_mode {
        EditCompletionRender::Crescendo
    } else {
        EditCompletionRender::Updated
    }
}

#[cfg(test)]
#[path = "tests/story_738_completion_crescendo_tests.rs"]
mod story_738_completion_crescendo_tests;

/// Local truncate helper for archive listings; mirrors `history::shorten`.
/// Char-boundary-safe: titles carry emoji/unicode (e.g. the pause/arrow glyphs),
/// so slicing by raw byte index panics mid-codepoint. Truncate by chars instead.
// trace:BUG-424 | ai:claude
pub(crate) fn shorten_text(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// TASK-730: when the default `aida list` view (no status filter) shows a MIX of
/// finished and unfinished work, point a returning user at `aida list open` —
/// the "what's left to do" view they'd otherwise never discover (Completed
/// specs stay visible until archived, by design). No tip when everything's done
/// or everything's open, or when a status filter is already applied.
// trace:TASK-730 | ai:claude
pub(crate) fn maybe_print_whats_left_tip(
    status_filter: Option<&str>,
    reqs: &[aida_core::RequirementSummary],
) {
    if status_filter.is_some() {
        return;
    }
    let has_done = reqs
        .iter()
        .any(|r| r.status.eq_ignore_ascii_case("completed"));
    let has_open = reqs
        .iter()
        .any(|r| !r.status.eq_ignore_ascii_case("completed"));
    if has_done && has_open {
        println!(
            "{}",
            "  Tip: `aida list open` shows just what's left to do.".dimmed()
        );
    }
}

/// STORY-584 (criterion 4): in the `--deferred` view, print each spec's revisit
/// trigger so the operator can scan "what is primed, and what brings each back."
/// The trigger comes from the `deferred_until` field when set; for rows deferred
/// only via a legacy `deferred:*` parking tag, fall back to the tag's suffix
/// (e.g. `deferred:stabilization-first` → "stabilization-first"). Prints nothing
/// outside the deferred view or when no rows carry a discoverable trigger.
/// trace:STORY-584 | ai:claude
pub(crate) fn print_deferred_triggers(deferred_view: bool, reqs: &[aida_core::RequirementSummary]) {
    if !deferred_view || reqs.is_empty() {
        return;
    }
    // Derive a trigger string for a row: explicit field first, else the
    // suffix of the first `deferred:*` tag.
    let trigger_of = |r: &aida_core::RequirementSummary| -> Option<String> {
        if let Some(cond) = r.deferred_until.as_deref() {
            if !cond.is_empty() {
                return Some(cond.to_string());
            }
        }
        r.tags
            .iter()
            .find_map(|t| t.strip_prefix("deferred:"))
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };

    println!("\n{}", "Revisit triggers:".bold());
    for r in reqs {
        let display_id = r
            .agreed_id
            .as_deref()
            .or(r.spec_id.as_deref())
            .unwrap_or("?");
        match trigger_of(r) {
            Some(cond) => println!("  {}  {}", display_id.bold(), cond.dimmed()),
            None => println!(
                "  {}  {}",
                display_id.bold(),
                "(no trigger — set one with `aida defer <id> --until \"…\"`)".dimmed()
            ),
        }
    }
}

pub(crate) fn handle_punts_command(cmd: PuntsCommand) -> Result<()> {
    let project_root = find_project_root()?;
    let records = punt::read_ledger(&project_root);

    match cmd {
        PuntsCommand::List {
            spec,
            category,
            resolution,
            all,
        } => {
            let mut filtered = records;
            // BUG-674: hide auto-filed session-end visibility warnings by
            // default so genuine decision-punts aren't buried; `--all` shows
            // them. trace:BUG-674 | ai:claude
            if !all {
                filtered.retain(|r| !punt::is_session_end_noise(r));
            }
            if let Some(s) = spec {
                filtered.retain(|r| r.spec.eq_ignore_ascii_case(&s));
            }
            if let Some(c) = category {
                filtered.retain(|r| r.category.to_string().eq_ignore_ascii_case(&c));
            }
            if let Some(r) = resolution {
                filtered.retain(|rec| rec.resolution_path.eq_ignore_ascii_case(&r));
            }

            if filtered.is_empty() {
                println!("No punt records found matching criteria.");
                return Ok(());
            }

            println!(
                "{:<24} {:<12} {:<20} {:<12} {}",
                "TIMESTAMP".dimmed(),
                "SPEC".dimmed(),
                "RESOLUTION PATH".dimmed(),
                "DECISION".dimmed(),
                "DETAIL".dimmed()
            );
            println!("{}", "─".repeat(80).dimmed());
            for r in filtered {
                let ts = r.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
                let dec = r.decision.as_deref().unwrap_or("-");
                let dec_colored = match dec {
                    "resolved" => dec.green(),
                    "escalated" => dec.red(),
                    "deferred" => dec.yellow(),
                    _ => dec.normal(),
                };
                let res_colored = match r.resolution_path.as_str() {
                    "advisor-resolved" => r.resolution_path.green(),
                    "escalated-to-human" => r.resolution_path.red(),
                    "punted" => r.resolution_path.yellow(),
                    _ => r.resolution_path.normal(),
                };
                println!(
                    "{:<24} {:<12} {:<20} {:<12} {}",
                    ts.dimmed(),
                    r.spec.cyan().bold(),
                    res_colored,
                    dec_colored,
                    r.detail
                );
            }
        }
        PuntsCommand::Analyze { all } => {
            // BUG-674: exclude auto-filed session-end visibility warnings from
            // the analytics by default (they are not decision-punts); `--all`
            // includes them. trace:BUG-674 | ai:claude
            let records: Vec<punt::PuntRecord> = if all {
                records
            } else {
                records
                    .into_iter()
                    .filter(|r| !punt::is_session_end_noise(r))
                    .collect()
            };
            let total = records.len();
            if total == 0 {
                println!("The punt ledger is empty.");
                return Ok(());
            }

            let mut punted_count = 0;
            let mut resolved_count = 0;
            let mut escalated_count = 0;
            let mut total_judged = 0;

            let mut category_counts = std::collections::HashMap::new();
            let mut classification_counts = std::collections::HashMap::new();
            let mut durations = Vec::new();

            for r in &records {
                *category_counts.entry(r.category).or_insert(0) += 1;
                if let Some(ref cls) = r.classification {
                    *classification_counts.entry(cls.clone()).or_insert(0) += 1;
                }

                // BUG-674: only a genuinely OPEN record is "Awaiting Triage".
                // Resolved / escalated (incl. the CLI/MCP direct-close slugs)
                // land in their own buckets; dismissed + shelved-by-failure
                // records are closed and counted in neither triage bucket.
                if punt::is_open(r) {
                    punted_count += 1;
                } else if r.resolution_path == "advisor-resolved"
                    || r.resolution_path == punt::RESOLUTION_HUMAN_RESOLVED
                    || r.decision.as_deref() == Some("resolved")
                {
                    resolved_count += 1;
                    total_judged += 1;
                } else if r.resolution_path == "escalated-to-human"
                    || r.decision.as_deref() == Some("escalated")
                {
                    escalated_count += 1;
                    total_judged += 1;
                }

                if let (Some(paused), Some(resolved)) = (r.paused_at, r.resolved_at) {
                    if resolved >= paused {
                        durations.push(resolved.signed_duration_since(paused));
                    }
                }
            }

            let escalation_rate = if total_judged > 0 {
                (escalated_count as f64 / total_judged as f64) * 100.0
            } else {
                0.0
            };

            println!("{}", "◆ AIDA Punt Ledger Analytics ◆".magenta().bold());
            println!(
                "{}",
                "──────────────────────────────────────────────────".dimmed()
            );
            println!(
                "{:<30} {}",
                "Total Recorded Punts:",
                total.to_string().bold()
            );
            println!(
                "{:<30} {}",
                "  Punted (Awaiting Triage):",
                punted_count.to_string().yellow()
            );
            println!(
                "{:<30} {}",
                "  Resolved by Advisor:",
                resolved_count.to_string().green()
            );
            println!(
                "{:<30} {}",
                "  Escalated to Human:",
                escalated_count.to_string().red()
            );
            println!(
                "{:<30} {:.1}% ({}/{})",
                "Escalation Rate (Advisor):", escalation_rate, escalated_count, total_judged
            );

            if !durations.is_empty() {
                let total_sec: i64 = durations.iter().map(|d| d.num_seconds()).sum();
                let avg_sec = total_sec / durations.len() as i64;
                let avg_duration = chrono::Duration::seconds(avg_sec);
                println!(
                    "{:<30} {}m {}s",
                    "Average Resolution Time:",
                    avg_duration.num_minutes(),
                    avg_duration.num_seconds() % 60
                );
            }

            println!("\n{}", "Category Breakdown:".bold());
            let mut categories: Vec<_> = category_counts.into_iter().collect();
            categories.sort_by(|a, b| b.1.cmp(&a.1));
            for (cat, count) in categories {
                println!(
                    "  {:<28} {}",
                    cat.to_string().blue(),
                    count.to_string().bold()
                );
            }

            println!(
                "\n{}",
                "Classification Triage Patterns (Question Shapes):".bold()
            );
            if classification_counts.is_empty() {
                println!("  No classifications recorded yet.");
            } else {
                let mut classes: Vec<_> = classification_counts.into_iter().collect();
                classes.sort_by(|a, b| b.1.cmp(&a.1));
                for (cls, count) in classes {
                    println!("  {:<28} {}", cls.cyan(), count.to_string().bold());
                }
            }
            println!(
                "{}",
                "──────────────────────────────────────────────────".dimmed()
            );
        }
        PuntsCommand::Resolve {
            id,
            answer,
            reasoning,
            classification,
        } => {
            // BUG-674: resolve via the shared core (parity with the
            // `resolve_punt` MCP tool) — writes the orchestrator resume
            // response AND closes the open ledger record(s). trace:BUG-674
            let reasoning =
                reasoning.unwrap_or_else(|| "resolved via `aida punts resolve`".to_string());
            let (path, closed) = punt::resolve_punt_core(
                &project_root,
                &id,
                &answer,
                &reasoning,
                classification,
                Some("cli"),
            )?;
            if closed == 0 {
                println!(
                    "No open punt found for {} — wrote resolution response to {}.",
                    id.cyan(),
                    path.display().to_string().dimmed()
                );
            } else {
                println!(
                    "{} Resolved {} and closed {} ledger record{}. A live drain resumes on the response.",
                    glyph(crate::glyphs::Glyph::Check).green(),
                    id.cyan(),
                    closed,
                    if closed == 1 { "" } else { "s" }
                );
            }
        }
        PuntsCommand::Dismiss { id, reason } => {
            // BUG-674: dismiss just closes the ledger record — no orchestrator
            // response (a dismissed punt does not resume an implementer).
            let closed = punt::dismiss_punt_core(&project_root, &id, Some("cli"))?;
            if closed == 0 {
                println!("No open punt found for {}.", id.cyan());
            } else {
                let note = reason.map(|r| format!(" ({r})")).unwrap_or_default();
                println!(
                    "{} Dismissed {} — closed {} ledger record{}{}.",
                    glyph(crate::glyphs::Glyph::Check).green(),
                    id.cyan(),
                    closed,
                    if closed == 1 { "" } else { "s" },
                    note
                );
            }
        }
        PuntsCommand::Escalate {
            id,
            reasoning,
            escalation_reason,
            classification,
        } => {
            // BUG-674: escalate via the shared core (parity with the
            // `escalate_punt` MCP tool) — writes the park response AND closes
            // the ledger record as escalated-to-human. trace:BUG-674
            let (path, closed) = punt::escalate_punt_core(
                &project_root,
                &id,
                &reasoning,
                escalation_reason,
                classification,
                Some("cli"),
            )?;
            if closed == 0 {
                println!(
                    "No open punt found for {} — wrote escalation response to {}.",
                    id.cyan(),
                    path.display().to_string().dimmed()
                );
            } else {
                println!(
                    "{} Escalated {} to a human and closed {} ledger record{}.",
                    glyph(crate::glyphs::Glyph::Warning).yellow(),
                    id.cyan(),
                    closed,
                    if closed == 1 { "" } else { "s" }
                );
            }
        }
        PuntsCommand::Read { id } => {
            // Most recent record for the spec is the live one (mirrors the
            // `read_punt` MCP tool). trace:BUG-674
            let record = records
                .iter()
                .rev()
                .find(|r| r.spec.eq_ignore_ascii_case(&id))
                .ok_or_else(|| {
                    anyhow::anyhow!("No punt record found in ledger for spec ID '{}'", id)
                })?;
            println!("{}", serde_json::to_string_pretty(record)?);
        }
        PuntsCommand::Promote { id, memory_name } => {
            let record = records
                .iter()
                .find(|r| r.spec.eq_ignore_ascii_case(&id))
                .ok_or_else(|| {
                    anyhow::anyhow!("No punt record found in ledger for spec ID '{}'", id)
                })?;

            let target_path = if memory_name.starts_with("docs/") {
                project_root.join(&memory_name)
            } else if memory_name.starts_with("discipline/") {
                project_root.join("docs").join(&memory_name)
            } else {
                project_root.join(&memory_name)
            };

            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let title = record.classification.as_deref().unwrap_or(&record.detail);
            let slug = memory_name
                .trim_end_matches(".md")
                .replace(['/', ' ', '-'], "_");
            let date_str = chrono::Utc::now().format("%Y-%m-%d").to_string();

            let mut content = String::new();
            if !target_path.exists() {
                content.push_str(&format!(
                    "# {}\n\n\
                     **Last updated**: {}\n\
                     **Principle Trace**: `feedback_{}` | `{}`\n\n\
                     ## Context & Design Fork\n\
                     - **Spec**: {}\n\
                     - **Category**: {}\n\
                     - **Raised By**: {}\n\
                     - **Resolution Path**: {}\n\n\
                     ## Decision / Principle\n\
                     {}\n",
                    title,
                    date_str,
                    slug,
                    record.spec,
                    record.spec,
                    record.category,
                    record.raised_by.as_deref().unwrap_or("unknown"),
                    record.resolution_path,
                    record
                        .answer
                        .as_deref()
                        .or(record.classification.as_deref())
                        .unwrap_or(&record.detail)
                ));
                std::fs::write(&target_path, &content)?;
                println!(
                    "Created new memory pack entry at: {}",
                    target_path.display().to_string().cyan()
                );
            } else {
                content.push_str(&format!(
                    "\n---\n\n\
                     ## Principle Trace: `feedback_{}` | `{}`\n\
                     - **Spec**: {}\n\
                     - **Category**: {}\n\
                     - **Decision**: {}\n\n\
                     ### Context\n\
                     {}\n\n\
                     ### Resolution\n\
                     {}\n",
                    slug,
                    record.spec,
                    record.spec,
                    record.category,
                    record.decision.as_deref().unwrap_or("-"),
                    record.detail,
                    record
                        .answer
                        .as_deref()
                        .or(record.classification.as_deref())
                        .unwrap_or(&record.detail)
                ));
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&target_path)?;
                file.write_all(content.as_bytes())?;
                println!(
                    "Appended design-fork pattern to existing memory pack entry at: {}",
                    target_path.display().to_string().cyan()
                );
            }

            // Update the record's principle_link in the ledger
            let mut updated_records = records.clone();
            let mut found = false;
            for r in &mut updated_records {
                if r.spec.eq_ignore_ascii_case(&id) {
                    r.principle_link = Some(memory_name.clone());
                    found = true;
                    break;
                }
            }

            if found {
                let path = punt::ledger_path(&project_root);
                let mut file = std::fs::File::create(&path)?;
                for r in &updated_records {
                    let serialized = serde_json::to_string(r)?;
                    use std::io::Write;
                    writeln!(file, "{}", serialized)?;
                }
                println!(
                    "Updated punt record '{}' with principle link to '{}'.",
                    id.cyan(),
                    memory_name.green()
                );
            }
        }
    }

    Ok(())
}
