//! Spec dry-run/interview, `aida intent` and `aida why` handlers (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// STORY-750: run `aida <args>` as a child, INHERITING stdio so nested prompts /
/// sessions work, and surface a non-zero exit as an error.
pub(crate) fn self_invoke_aida(args: &[&str]) -> Result<()> {
    let exe = aida_exe_path();
    let status = std::process::Command::new(exe)
        .args(args)
        .status_retrying_etxtbsy()
        .map_err(|e| anyhow::anyhow!("run `aida {}`: {e}", args.join(" ")))?;
    if !status.success() {
        anyhow::bail!("`aida {}` exited with {status}", args.join(" "));
    }
    Ok(())
}

/// STORY-750: after a sweep, re-classify and report how many specs still need the
/// human, naming the drain command for the now-ready set. Read-only + best-effort
/// (never fails the sweep).
pub(crate) fn report_drain_readiness() {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let Some(store) = load_store_for_lookup(&project_root) else {
        return;
    };
    let in_flight_scopes = in_flight_lease_scopes(&project_root);
    let queued_ids = all_queued_requirement_ids(&project_root);
    let facts = collect_unblock_facts(&store, &in_flight_scopes, &queued_ids);
    let still_blocked = facts
        .iter()
        .filter(|f| burndown::classify_unblock(f).is_some())
        .count();
    println!();
    println!(
        "{} {} spec(s) still need you · {} queued and drive-ready.",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
        still_blocked,
        queued_ids.len()
    );
    if !queued_ids.is_empty() {
        println!(
            "  {}",
            "Drain the ready set: `aida burndown` (or `aida queue work --auto-complete`).".dimmed()
        );
    }
}

/// STORY-631: `aida intent <ID>` — the cached, drift-stamped, AI-generated
/// plain-terms comprehension of WHY a spec exists. Distinct from `aida why`
/// (the deterministic state classifier): this is an LLM synthesis over the spec
/// + its graph neighborhood, generated on first call (or `--refresh`), printed
/// `aida spec dryrun <ID>` — the implementer-readiness pre-check.
///
/// Loads the spec, ALWAYS runs the deterministic [`dryrun::score`] pre-check,
/// and (with `--ai`) appends a headless AI gap report. The pure scorer +
/// JSON/parse contracts are unit-tested in `dryrun.rs`; this is the integration
/// boundary (store load + the gated `claude -p` spawn), mirroring how
/// `handle_intent` pairs with `intent.rs`. trace:STORY-656 | ai:claude
pub(crate) fn handle_spec_dryrun(id: &str, ai: bool, json: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;

    // trace:TASK-849 | ai:claude — reuse the canonical case-insensitive resolver
    // (matches spec_id + agreed_id) instead of hand-rolling the same match here.
    let req = store
        .get_requirement_by_spec_id(id.trim())
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!("no spec found matching `{id}` — check the ID with `aida list`.")
        })?;
    let disp = req.display_id();

    // The deterministic pre-check ALWAYS runs. This is the pure core.
    let snapshot = dryrun::SpecSnapshot::from_requirement(&req);
    let readiness = dryrun::score(&snapshot);

    // Optional AI gap report. Gated behind --ai AND an interactive context,
    // exactly like `aida intent` fences its spawn.
    let ai_report = if ai {
        Some(run_dryrun_ai_pass(&project_root, &disp)?)
    } else {
        None
    };

    if json {
        let payload = dryrun::dryrun_json(&disp, &readiness, ai_report.as_ref());
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    print_dryrun_human(&disp, &req.title, &readiness, ai_report.as_ref());
    Ok(())
}

/// Render the human view of a dryrun verdict: the headline score, a pass/fail
/// line per dimension with its reason, the failing-dimension callout, and (when
/// present) the AI gap report. trace:STORY-656 | ai:claude
pub(crate) fn print_dryrun_human(
    disp: &str,
    title: &str,
    readiness: &dryrun::Readiness,
    ai: Option<&dryrun::AiReport>,
) {
    use colored::Colorize;

    let score = readiness.score;
    let colored_score = match score {
        s if s >= 80 => format!("{s}/100").green().bold(),
        s if s >= 50 => format!("{s}/100").yellow().bold(),
        s => format!("{s}/100").red().bold(),
    };
    println!(
        "{} {} — readiness {}",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        disp.cyan().bold(),
        colored_score
    );
    println!("  {}", title.dimmed());
    println!();

    for d in &readiness.dimensions {
        let (mark, name) = if d.pass {
            (
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                d.name.green(),
            )
        } else {
            (
                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                d.name.red(),
            )
        };
        println!("  {mark} {name} — {}", d.reason.dimmed());
    }

    let failing: Vec<&dryrun::Dimension> = readiness.failing().collect();
    if failing.is_empty() {
        println!();
        println!(
            "  {} ready for an implementer to pick up.",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold()
        );
    } else {
        println!();
        // trace:TASK-849 | ai:claude — dimension names are now `&'static str`.
        let names: Vec<&str> = failing.iter().map(|d| d.name).collect();
        println!(
            "  {} fix before queuing: {}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
            names.join(", ").yellow()
        );
    }

    if let Some(ai) = ai {
        let section = |label: &str, items: &[String]| {
            if items.is_empty() {
                return;
            }
            println!();
            println!("  {}", label.cyan().bold());
            for it in items {
                println!(
                    "    {} {it}",
                    crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
                );
            }
        };
        println!();
        println!(
            "  {}",
            format!("AI gap report · model={}", ai.model).dimmed()
        );
        section("Questions an implementer would ask", &ai.questions);
        section("Assumptions they'd make", &ai.assumptions);
        section("Ambiguities / missing acceptance", &ai.ambiguities);
    }
}

/// Run the gated headless `/aida-dryrun` AI pass: spawn `claude -p`, read the
/// JSON sidecar it writes, and parse it into a [`dryrun::AiReport`]. The spawn
/// is fenced behind a TTY / non-headless context exactly like `generate_intent`
/// — without a human to authorize tools the pass has no value. The parse
/// contract is unit-tested in `dryrun.rs`; tests never reach this function.
/// trace:STORY-656 | ai:claude
pub(crate) fn run_dryrun_ai_pass(
    project_root: &std::path::Path,
    disp: &str,
) -> Result<dryrun::AiReport> {
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let headless = std::env::var("AIDA_HEADLESS").as_deref() == Ok("1");
    if !interactive || headless {
        anyhow::bail!(
            "the --ai dryrun report shells out to `claude -p`, which needs an interactive (TTY) \
             context — run it from your terminal, or drop --ai for the deterministic pre-check \
             alone."
        );
    }

    let dir = project_root.join(".aida").join("dryrun");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating dryrun artifact dir {}", dir.display()))?;
    let sidecar_path = dir.join(format!("{disp}.json"));
    let _ = std::fs::remove_file(&sidecar_path);

    let prompt = format!(
        "/aida-dryrun {disp}\n\n\
         You are pre-checking spec {disp} for implementer-readiness. DO NOT implement anything \
         and DO NOT modify the spec. Read the spec and its immediate graph neighborhood, then \
         write a JSON object to `{}` with keys `questions` (string array — what an implementer \
         would need answered before starting), `assumptions` (string array — what they'd have to \
         assume to proceed), `ambiguities` (string array — ambiguities or missing acceptance \
         criteria), and `model` (your model id).",
        sidecar_path.display()
    );

    println!(
        "{} {disp} — running AI gap report (headless /aida-dryrun)…",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );

    let session_id = Uuid::now_v7().to_string();
    let date = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let log_path = dir.join(format!("{disp}-{date}.log"));
    let tee = crate::headless_tee::TeeOptions::from_env_and_flag(false)
        .with_label(format!("dryrun:{disp}"));
    let status =
        crate::session::spawn_claude_headless(&prompt, &session_id, &log_path, &tee, false)
            .context("spawning headless /aida-dryrun agent")?;
    if !status.success() {
        anyhow::bail!(
            "dryrun AI agent exited with {} — see log {}",
            status.code().unwrap_or(-1),
            log_path.display()
        );
    }

    let raw = std::fs::read_to_string(&sidecar_path).with_context(|| {
        format!(
            "dryrun AI agent did not produce the sidecar at {} — see log {}",
            sidecar_path.display(),
            log_path.display()
        )
    })?;
    let report = dryrun::parse_ai_sidecar(&raw)?;
    let _ = std::fs::remove_file(&sidecar_path);
    Ok(report)
}

/// `aida spec interview <ID>` — resolve a spec's `dryrun` readiness gaps INTO
/// the spec via clarifying questions.
///
/// Closes the L1 intent-quality loop dryrun opens: dryrun *surfaces* the gaps,
/// interview *resolves* them. The pure core lives in `interview.rs` (gap →
/// question mapping + answer → spec-edit folding, fully unit-tested); this is
/// the integration boundary — store load, the propose/apply split, TTY
/// prompting, the headless JSON emit, and the actual write. trace:STORY-657
pub(crate) fn handle_spec_interview(
    id: &str,
    apply: bool,
    ai: bool,
    answers_file: Option<&str>,
    json: bool,
) -> Result<()> {
    use colored::Colorize;

    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;

    let want = id.trim().to_ascii_uppercase();
    let req = store
        .requirements
        .iter()
        .find(|r| {
            [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                .into_iter()
                .flatten()
                .any(|s| s.eq_ignore_ascii_case(&want))
        })
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!("no spec found matching `{id}` — check the ID with `aida list`.")
        })?;
    let disp = req.display_id();

    // Score the spec (same deterministic check dryrun runs) and derive the
    // questions from the FAILING dimensions (+ optional AI gap report).
    let snapshot = dryrun::SpecSnapshot::from_requirement(&req);
    let readiness = dryrun::score(&snapshot);
    let ai_report = if ai {
        Some(run_dryrun_ai_pass(&project_root, &disp)?)
    } else {
        None
    };
    let questions = interview::questions_for(&readiness, ai_report.as_ref());

    if questions.is_empty() {
        if json {
            let payload = interview::interview_json(&disp, &readiness, &questions);
            println!("{}", serde_json::to_string_pretty(&payload)?);
        } else {
            println!(
                "{} {} — readiness {} — no gaps to interview; spec is ready.",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                disp.cyan().bold(),
                format!("{}/100", readiness.score).green().bold()
            );
        }
        return Ok(());
    }

    // Gather answers: from a --answers file (headless feedback), interactively
    // from a TTY, or none (headless propose — just emit the questions).
    let answers: Vec<interview::Answer> = if let Some(path) = answers_file {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading --answers file {path}"))?;
        interview::parse_answers(&raw)?
    } else {
        let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        let headless = std::env::var("AIDA_HEADLESS").as_deref() == Ok("1");
        if interactive && !headless {
            prompt_interview_answers(&disp, &req.title, &readiness, &questions)?
        } else {
            // Headless with no answers supplied: emit the structured question
            // list and exit WITHOUT blocking on stdin. The agent/advisor seat.
            let payload = interview::interview_json(&disp, &readiness, &questions);
            if json {
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!(
                    "{} {} — readiness {} — {} open question(s). \
                     No terminal: answer them and feed back with \
                     `aida spec interview {} --answers <file> --apply`.",
                    crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
                    disp.cyan().bold(),
                    format!("{}/100", readiness.score).yellow().bold(),
                    questions.len(),
                    disp,
                );
                for q in &questions {
                    println!(
                        "  {} [{}] {}",
                        crate::glyph(crate::glyphs::Glyph::Bullet).dimmed(),
                        q.dimension.dimmed(),
                        q.prompt
                    );
                }
            }
            return Ok(());
        }
    };

    // Fold the answers into a concrete edit (pure). The same computation drives
    // both the propose preview and the --apply write — this is the heart of the
    // non-destructive-by-default split.
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let edit = interview::apply_answers(&req.description, &questions, &answers, &date);

    if edit.is_noop() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "spec": disp,
                    "applied": false,
                    "changes": [],
                }))?
            );
        } else {
            println!(
                "  {} no answers folded in — nothing to apply.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
        return Ok(());
    }

    if !apply {
        // Propose-by-default: show what WOULD change, write nothing.
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "spec": disp,
                    "applied": false,
                    "changes": edit.changes,
                    "new_description": edit.new_description,
                    "parent_spec_id": edit.parent_spec_id,
                    "priority": edit.priority.as_ref().map(|p| p.to_string()),
                }))?
            );
        } else {
            println!(
                "{} {} — proposed edits (run again with {} to write):",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
                disp.cyan().bold(),
                "--apply".cyan()
            );
            for c in &edit.changes {
                println!(
                    "  {} {c}",
                    crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
                );
            }
        }
        return Ok(());
    }

    // --apply: write the resolved spec. Resolve the parent (if named) up front
    // so a bad id fails before any write.
    let Some(store_path) = detect_distributed_store_from(&project_root) else {
        anyhow::bail!(
            "aida spec interview --apply writes to the git-canonical store, but no distributed \
             store was found — run `aida init` (this verb is not supported on the deprecated \
             centralized backend)."
        );
    };
    let parent_req = match &edit.parent_spec_id {
        Some(pid) if pid.eq_ignore_ascii_case(&disp) => {
            anyhow::bail!("a spec cannot be its own parent ({disp}).");
        }
        Some(pid) => Some(
            store
                .requirements
                .iter()
                .find(|r| {
                    [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|s| s.eq_ignore_ascii_case(pid))
                })
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "named parent `{pid}` not found — check the ID with `aida list`."
                    )
                })?,
        ),
        None => None,
    };

    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    let mut to_save = req.clone();
    to_save.description = edit.new_description.clone();
    if let Some(p) = &edit.priority {
        to_save.priority = p.clone();
    }
    // Link the parent bidirectionally (same convention as `aida add --parent`).
    if let Some(parent) = &parent_req {
        use aida_core::models::{Relationship, RelationshipType};
        let now = chrono::Utc::now();
        let already_linked = to_save
            .relationships
            .iter()
            .any(|r| r.target_id == parent.id && r.rel_type == RelationshipType::Child);
        if !already_linked {
            to_save.relationships.push(Relationship {
                target_id: parent.id,
                rel_type: RelationshipType::Child,
                created_at: Some(now),
                created_by: None,
            });
            let mut parent_mut = parent.clone();
            parent_mut.relationships.push(Relationship {
                target_id: to_save.id,
                rel_type: RelationshipType::Parent,
                created_at: Some(now),
                created_by: None,
            });
            backend.update_requirement(&parent_mut)?;
        }
    }
    to_save.modified_at = chrono::Utc::now();
    backend.update_requirement(&to_save)?;

    // Re-score the now-resolved spec so the readiness improvement is visible.
    let after_snapshot = dryrun::SpecSnapshot::from_requirement(&to_save);
    let after = dryrun::score(&after_snapshot);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "spec": disp,
                "applied": true,
                "changes": edit.changes,
                "readiness_before": readiness.score,
                "readiness_after": after.score,
            }))?
        );
    } else {
        println!(
            "{} {} — applied {} edit(s):",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            disp.cyan().bold(),
            edit.changes.len()
        );
        for c in &edit.changes {
            println!(
                "  {} {c}",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        }
        println!(
            "  readiness {} {} {}",
            format!("{}/100", readiness.score).yellow(),
            crate::glyph(crate::glyphs::Glyph::Arrow).dimmed(),
            format!("{}/100", after.score).green().bold()
        );
    }
    Ok(())
}

/// Interactive (TTY) interview: prompt for each gap in turn, reading one line of
/// free-text per question from stdin. A blank line is a skip. Mirrors the
/// line-read pattern used elsewhere (`read_line`); kept out of the pure core so
/// tests never touch stdin. trace:STORY-657 | ai:claude
pub(crate) fn prompt_interview_answers(
    disp: &str,
    title: &str,
    readiness: &dryrun::Readiness,
    questions: &[interview::InterviewQuestion],
) -> Result<Vec<interview::Answer>> {
    use colored::Colorize;
    use std::io::Write;

    println!(
        "{} {} — readiness {} — {} gap(s) to resolve",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        disp.cyan().bold(),
        format!("{}/100", readiness.score).yellow().bold(),
        questions.len()
    );
    println!("  {}", title.dimmed());
    println!(
        "  {}",
        "Answer each (blank line = skip). Answers fold into the spec on --apply.".dimmed()
    );
    println!();

    let mut answers = Vec::with_capacity(questions.len());
    for (i, q) in questions.iter().enumerate() {
        println!(
            "  {} {}",
            format!("{}/{}", i + 1, questions.len()).cyan().bold(),
            q.prompt
        );
        print!("  > ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        let n = std::io::stdin().read_line(&mut line)?;
        if n == 0 {
            // EOF mid-interview — stop reading, keep what we have.
            println!();
            break;
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            answers.push(interview::Answer {
                dimension: q.dimension.clone(),
                answer: trimmed.to_string(),
            });
        }
        println!();
    }
    Ok(answers)
}

/// from cache otherwise, and marked STALE when the neighborhood drifted.
/// trace:STORY-631 | ai:claude
pub(crate) fn handle_intent(id: &str, audience: &str, refresh: bool, json: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;

    let want = id.trim().to_ascii_uppercase();
    let req = store
        .requirements
        .iter()
        .find(|r| {
            [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                .into_iter()
                .flatten()
                .any(|s| s.eq_ignore_ascii_case(&want))
        })
        .cloned();
    let Some(req) = req else {
        anyhow::bail!("no spec found matching `{id}` — check the ID with `aida list`.");
    };
    let disp = req.display_id();

    // Compute the fresh neighborhood hash (also the drift comparator).
    let inputs = build_intent_neighborhood(&req, &store.requirements);
    let fresh_hash = inputs.source_hash();

    // Decide whether to (re)generate. Slice 1: generate on absent OR --refresh;
    // a stale cache is REPORTED but not auto-regenerated (manual via --refresh).
    let need_generate = refresh || req.intent.is_none();

    if need_generate {
        let intent = generate_intent(&project_root, &req, &disp, &inputs, &fresh_hash)?;
        // Persist to the canonical store. This is a substrate WRITE — gated on
        // generation (drift / --refresh), never per-read. trace:STORY-631
        let Some(store_path) = detect_distributed_store_from(&project_root) else {
            anyhow::bail!(
                "aida intent writes the comprehension to the git-canonical store, but no \
                 distributed store was found — run `aida init` (this verb is not supported on \
                 the deprecated centralized backend)."
            );
        };
        let dispenser = load_dispenser(&store_path)?;
        let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
        let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
        let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;
        let mut to_save = req.clone();
        to_save.intent = Some(intent.clone());
        // NOTE: deliberately do NOT bump modified_at or touch any field in the
        // diff_snapshots allow-list — regeneration must generate ZERO history
        // rows. trace:STORY-631 | ai:claude
        backend.update_requirement(&to_save)?;

        // Fresh generation is never stale (hash == fresh_hash by construction).
        return print_intent(&disp, audience, &intent, false, json);
    }

    // Cache hit — print without regenerating; mark stale on drift.
    let intent = req
        .intent
        .as_ref()
        .expect("intent present on cache-hit path");
    let stale = intent::is_stale(&intent.source_hash, &fresh_hash);
    print_intent(&disp, audience, intent, stale, json)
}

/// Assemble the [`intent::NeighborhoodInputs`] for a spec: its own
/// title/description/status, each immediate neighbor's id+title+status, and the
/// comment count. trace:STORY-631 | ai:claude
pub(crate) fn build_intent_neighborhood(
    req: &Requirement,
    all: &[Requirement],
) -> intent::NeighborhoodInputs {
    let mut neighbors = Vec::new();
    for rel in &req.relationships {
        if let Some(n) = all.iter().find(|r| r.id == rel.target_id) {
            neighbors.push(intent::NeighborFact {
                id: n.display_id(),
                title: n.title.clone(),
                status: n.status.to_string(),
            });
        }
    }
    intent::NeighborhoodInputs {
        spec_id: req.display_id(),
        title: req.title.clone(),
        description: req.description.clone(),
        status: req.status.to_string(),
        neighbors,
        comment_count: intent::key_comment_count(req),
    }
}

/// Render the intent comprehension — JSON shape or the labelled human view.
/// The human view ALWAYS labels the prose AI-generated so no reader mistakes it
/// for hand-authored ground truth. trace:STORY-631 | ai:claude
pub(crate) fn print_intent(
    disp: &str,
    audience: &str,
    intent: &aida_core::SpecIntent,
    stale: bool,
    json: bool,
) -> Result<()> {
    if json {
        let payload = intent::intent_json(disp, audience, intent, stale);
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    let body = match audience {
        "llm" => &intent.llm,
        _ => &intent.layman,
    };
    println!(
        "{} {} — intent ({})",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        disp.cyan(),
        audience
    );
    println!(
        "  {}",
        format!(
            "AI-generated comprehension · model={} · generated {}{}",
            intent.model,
            intent.generated_at,
            if stale { " · STALE" } else { "" }
        )
        .dimmed()
    );
    if stale {
        println!(
            "  {} the spec or its neighbors changed since this was generated — \
             re-run with {} to regenerate.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            "--refresh".yellow().bold()
        );
    }
    println!();
    println!("{body}");
    Ok(())
}

/// Run the headless `/aida-intent` skill via `claude -p` over the spec + its
/// graph neighborhood, parse the sidecar, and fold it into a
/// [`aida_core::SpecIntent`] stamped with generated_at + source_hash. The spawn
/// is the integration boundary; the pure transforms are unit-tested in
/// `intent.rs`. trace:STORY-631 | ai:claude
pub(crate) fn generate_intent(
    project_root: &std::path::Path,
    _req: &Requirement,
    disp: &str,
    inputs: &intent::NeighborhoodInputs,
    source_hash: &str,
) -> Result<aida_core::SpecIntent> {
    // The sidecar the skill writes; the launcher reads it back. Lives under
    // .aida/ (gitignored runtime state). trace:STORY-631
    let dir = project_root.join(".aida").join("intent");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating intent artifact dir {}", dir.display()))?;
    let sidecar_path = dir.join(format!("{disp}.json"));
    let _ = std::fs::remove_file(&sidecar_path);

    // The skill reads the spec + graph from the substrate itself (it has the
    // MCP/CLI surface); the prompt names the spec + the exact output contract.
    let prompt = format!(
        "/aida-intent {disp}\n\n\
         Write the comprehension as a JSON object to `{}` with keys \
         `layman` (plain prose for a human skimmer), `llm` (denser/structured, \
         for an agent loading the spec), and `model` (your model id). Read the \
         spec and its immediate graph neighborhood (parents, children, blockers, \
         referenced specs, decisions, key comments) before writing.",
        sidecar_path.display()
    );

    // Gate the LIVE spawn behind a TTY / non-headless context, mirroring how
    // intake fences its `claude -p` launch. Under a non-interactive or already-
    // headless context the spawn would have no human to authorize tools and no
    // value, so we refuse with guidance rather than launching a doomed pass.
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let headless = std::env::var("AIDA_HEADLESS").as_deref() == Ok("1");
    if !interactive || headless {
        anyhow::bail!(
            "aida intent generation shells out to `claude -p /aida-intent`, which needs an \
             interactive (TTY) context — run it from your terminal. (Batch/headless regeneration \
             is a separate follow-up.)"
        );
    }

    println!(
        "{} {disp} — generating intent comprehension (headless /aida-intent)…",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );

    let session_id = Uuid::now_v7().to_string();
    let date = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let log_path = dir.join(format!("{disp}-{date}.log"));
    let tee = crate::headless_tee::TeeOptions::from_env_and_flag(false)
        .with_label(format!("intent:{disp}"));
    let status =
        crate::session::spawn_claude_headless(&prompt, &session_id, &log_path, &tee, false)
            .context("spawning headless /aida-intent agent")?;
    if !status.success() {
        anyhow::bail!(
            "intent agent exited with {} — see log {}",
            status.code().unwrap_or(-1),
            log_path.display()
        );
    }

    let raw = std::fs::read_to_string(&sidecar_path).with_context(|| {
        format!(
            "intent agent did not produce the sidecar at {} — see log {}",
            sidecar_path.display(),
            log_path.display()
        )
    })?;
    let sidecar = intent::parse_intent_sidecar(&raw)?;
    // Clean up the runtime sidecar now that we have folded it in.
    let _ = std::fs::remove_file(&sidecar_path);

    let model = if sidecar.model.trim().is_empty() {
        std::env::var("AIDA_INTENT_MODEL").unwrap_or_else(|_| "claude".to_string())
    } else {
        sidecar.model.clone()
    };

    // source_hash is over the same `inputs` the caller already hashed, so a
    // later read with an unchanged neighborhood reports fresh.
    let _ = inputs;
    Ok(aida_core::SpecIntent {
        layman: sidecar.layman,
        llm: sidecar.llm,
        generated_at: chrono::Utc::now().to_rfc3339(),
        source_hash: source_hash.to_string(),
        model,
    })
}

/// STORY-723: the leading arrow marker for an `aida why` headline. Stripped on
/// the AGENT output path — the agent convention is no decorative leading glyphs,
/// and `AIDA_AGENT_OUTPUT=1 aida why <spec>` was leaking the arrow glyph — and
/// kept byte-identical on the human TTY path.
pub(crate) fn why_headline_prefix() -> String {
    why_headline_prefix_for(agent_output_mode())
}

/// Pure form of [`why_headline_prefix`] — empty in agent mode, the colored
/// arrow + trailing space otherwise — so the "no leading glyph on the agent
/// path" invariant is unit-testable without touching the process env / TTY.
pub(crate) fn why_headline_prefix_for(agent: bool) -> String {
    if agent {
        String::new()
    } else {
        format!(
            "{} ",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
        )
    }
}

/// STORY-729 (FIX 7): the plain-language reason + headline for a TERMINAL spec
/// (Completed or Rejected) in `aida why`. Names the branch-vs-merged reality so
/// a user can tell Completed ("merged to the default branch") from Done
/// ("finished on a branch, awaiting merge") by the WORDS, not just the colour —
/// the old single "it's done, nothing keeping it open" read identically for
/// both terminal statuses and never said "merged". Returns `(machine_reason,
/// human_clause)`; `status` is the already-formatted `{:?}` status label.
/// Pure so the wording is unit-testable without a store fixture.
// trace:STORY-729 | ai:claude
pub(crate) fn terminal_why_text(
    status: aida_core::RequirementStatus,
    status_label: &str,
) -> (String, String) {
    match status {
        aida_core::RequirementStatus::Completed => (
            format!("{status_label} — merged to the default branch"),
            "it's merged to the default branch — nothing keeping it open.".to_string(),
        ),
        // Rejected (the only other terminal status this path is called for).
        _ => (
            format!("{status_label} — not open"),
            "it was rejected (dropped) — nothing keeping it open.".to_string(),
        ),
    }
}

/// STORY-547: `aida why <ID>` — single-spec drill-down using the same
/// classifier as `burndown explain`. trace:STORY-547 | ai:claude
/// STORY-732 (FIX 2): render an orchestrator [`FailureReason`] as the same
/// phase + detail + recovery-hint lines `aida findings list` shows, so `aida why`
/// and `aida status <spec>` answer "what failed?" inline instead of redirecting
/// to `aida findings list`. Pure (plain strings; colour applied at the call
/// site) so the inlining is unit-testable without a store.
// trace:STORY-732 | ai:claude
pub(crate) fn failure_reason_lines(fr: &aida_core::FailureReason) -> Vec<String> {
    let cause = auto_complete_telemetry::failure_cause_label(Some(&fr.kind));
    let detail = auto_complete_telemetry::failure_detail_first_line(Some(&fr.detail));
    let mut out = vec![format!("failure: {} at {} — {}", cause, fr.phase, detail)];
    if let Some(hint) = fr.recovery_hint.as_deref() {
        out.push(format!(
            "{} hint: {hint}",
            crate::glyph(crate::glyphs::Glyph::SubArrow)
        ));
    }
    out
}

/// Prefer the append-only stream's newest shelving payload over the mutable
/// requirement snapshot. A supervised re-drive can shelve a spec again while
/// another writer leaves the cache-backed snapshot describing the first run.
// trace:BUG-1227 | ai:codex
pub(crate) fn latest_shelved_failure(
    project_root: &std::path::Path,
    spec: &str,
    snapshot: &aida_core::FailureReason,
) -> aida_core::FailureReason {
    let events = events::read_all(project_root);
    let Some(event) = events::latest_spec_shelved(&events, spec) else {
        return snapshot.clone();
    };
    let events::EventKind::SpecShelved {
        phase,
        kind,
        detail,
        recovery_hint,
    } = &event.kind
    else {
        unreachable!("latest_spec_shelved returned a non-shelving event")
    };
    aida_core::FailureReason {
        phase: phase.clone(),
        phase_index: snapshot.phase_index,
        kind: kind.clone(),
        detail: detail.clone().unwrap_or_else(|| snapshot.detail.clone()),
        recovery_hint: recovery_hint.clone().or_else(|| {
            (phase == &snapshot.phase && kind == &snapshot.kind)
                .then(|| snapshot.recovery_hint.clone())
                .flatten()
        }),
        shelved_by: snapshot.shelved_by.clone(),
        shelved_at: event.ts,
    }
}

/// STORY-732 recovery-legibility tests: the three audit fixes are pure-helper
/// shaped so the "tell the truth" guarantee is checkable without a store, a
/// lease, or a live process.
// trace:STORY-732 | ai:claude
#[cfg(test)]
#[path = "tests/story_732_recovery_legibility_tests.rs"]
mod story_732_recovery_legibility_tests;

/// STORY-732 (FIX 3): the cleanup-framed suffix `aida status <spec>` appends to a
/// TERMINAL spec's status badge when a dormant lease is still attached. Says
/// "still attached … to clear" — housekeeping, NOT "the In-Progress flag is
/// orphaned" (which contradicts a Completed status). Pure so the no-contradiction
/// wording is unit-testable.
// trace:STORY-732 | ai:claude
pub(crate) fn stale_cleanup_suffix(end_hint: &str) -> String {
    format!("(a dormant lease/worktree is still attached — `aida session end {end_hint}` to clear)")
}

/// STORY-732 (FIX 3): the orphaned-flag warning line for a NON-terminal STALE
/// spec — preserved verbatim from the pre-STORY-732 behaviour, extracted only so
/// the terminal-vs-non-terminal split is testable.
// trace:STORY-732 | ai:claude
pub(crate) fn stale_orphaned_line(why: &str, elapsed: &str) -> String {
    format!("STALE — {why}, {elapsed} elapsed; the In-Progress flag is orphaned")
}

/// STORY-754: does `arg` name a code location (`file` or `file:line`) rather
/// than a SPEC-ID? A SPEC-ID is `LETTERS-DIGITS` (STORY-750, FR-1-042) — never
/// contains `/` or `.`. Anything with a path separator, a dotted extension, or
/// that names an existing file (optionally with a trailing `:<line>`) is code.
// trace:STORY-754 | ai:claude
pub(crate) fn looks_like_code_arg(arg: &str) -> bool {
    let stripped = match arg.rsplit_once(':') {
        Some((p, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => p,
        _ => arg,
    };
    stripped.contains('/') || stripped.contains('.') || std::path::Path::new(stripped).exists()
}

/// First sentence (or first ~160 chars) of a spec description, single-lined —
/// the one-breath "why" for the code-location answer.
// trace:STORY-754 | ai:claude
pub(crate) fn why_first_sentence(desc: &str) -> String {
    // Drop leading markdown header lines (`## Why`, `# Context`, …) and blank
    // lines so the one-breath summary starts at the actual prose.
    let body: String = desc
        .lines()
        .skip_while(|l| {
            let t = l.trim();
            t.is_empty() || t.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join(" ");
    let d = body.trim();
    if d.is_empty() {
        return String::new();
    }
    // Prefer a clean sentence boundary (no ellipsis); else hard-cap at ~160
    // chars on a char boundary and mark the truncation with an ellipsis.
    let (slice, truncated) = match d.find(". ") {
        Some(i) => (&d[..=i], false),
        None => {
            let mut cut = d.len().min(160);
            while cut < d.len() && !d.is_char_boundary(cut) {
                cut += 1;
            }
            (&d[..cut], cut < d.len())
        }
    };
    let s = slice.trim();
    if truncated {
        format!("{s}…")
    } else {
        s.to_string()
    }
}

/// STORY-755: a spec's intent, resolved from EITHER the git-canonical store or
/// a plain markdown file — so `aida why <file:line>` works with the machine off.
pub(crate) struct ResolvedIntent {
    pub(crate) title: String,
    pub(crate) status: Option<String>,
    pub(crate) why: String,
    /// `None` = store; `Some(path)` = a plain markdown spec file.
    pub(crate) markdown: Option<std::path::PathBuf>,
}

/// Read a single frontmatter scalar (`key: value`) from a leading `---` block.
// trace:STORY-755 | ai:claude
pub(crate) fn markdown_frontmatter_field(content: &str, key: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let t = line.trim();
        if t == "---" {
            break;
        }
        if let Some((k, v)) = t.split_once(':') {
            if k.trim().eq_ignore_ascii_case(key) {
                return Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }
    None
}

/// Body after a leading `---` frontmatter block (or the whole content).
pub(crate) fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim_start();
        }
    }
    content
}

/// STORY-755: resolve a SPEC-ID's intent from a plain markdown file when the
/// git-canonical store isn't attached — so `aida why <file:line>` delivers the
/// full title + why on a BARE folder of markdown + trace comments, zero setup.
/// Matches a file named `<ID>.md` (case-insensitive) or any `.md` whose
/// frontmatter carries `id: <ID>`. Bounded walk; skips vcs/build dirs.
// trace:STORY-755 | ai:claude
pub(crate) fn resolve_spec_from_markdown(
    root: &std::path::Path,
    id: &str,
) -> Option<ResolvedIntent> {
    let want = id.to_ascii_uppercase();
    let name_target = format!("{want}.MD");
    let mut stack = vec![root.to_path_buf()];
    let mut budget: usize = 5000;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            if budget == 0 {
                return None;
            }
            budget -= 1;
            let path = entry.path();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                let n = entry.file_name();
                let n = n.to_string_lossy();
                if matches!(
                    n.as_ref(),
                    ".git" | "target" | "node_modules" | ".aida-store" | ".claude"
                ) {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path
                .extension()
                .and_then(|x| x.to_str())
                .map(|e| e.eq_ignore_ascii_case("md"))
                != Some(true)
            {
                continue;
            }
            let by_name = path
                .file_name()
                .map(|f| f.to_string_lossy().to_ascii_uppercase() == name_target)
                .unwrap_or(false);
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let by_fm = markdown_frontmatter_field(&content, "id")
                .map(|v| v.eq_ignore_ascii_case(&want))
                .unwrap_or(false);
            if by_name || by_fm {
                let title = markdown_frontmatter_field(&content, "title").unwrap_or_else(|| {
                    content
                        .lines()
                        .find_map(|l| {
                            l.trim()
                                .strip_prefix('#')
                                .map(|h| h.trim_start_matches('#').trim().to_string())
                        })
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| {
                            path.file_stem()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default()
                        })
                });
                return Some(ResolvedIntent {
                    title,
                    status: markdown_frontmatter_field(&content, "status"),
                    why: why_first_sentence(strip_frontmatter(&content)),
                    markdown: Some(path),
                });
            }
        }
    }
    None
}

/// STORY-785: what `git blame` knows about the last commit to touch a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlameOrigin {
    pub(crate) short_sha: String,
    /// Committer date, `YYYY-MM-DD`.
    pub(crate) date: String,
    pub(crate) subject: String,
    /// Full message, for trailer/trace extraction.
    pub(crate) message: String,
}

/// STORY-785: extract SPEC-IDs from a commit message — the `(ABC-123)` trailer
/// form the commit convention requires, plus `trace:ABC-123` markers in the
/// body. Uppercase-id-with-digits only, so a lowercase `(scope)` and a
/// `(#123)` PR suffix can never false-positive. Pure, so the scope-vs-id
/// boundary is unit-testable.
// trace:STORY-785 | ai:claude
pub(crate) fn spec_ids_from_commit_message(msg: &str) -> Vec<String> {
    let re_trailer = regex::Regex::new(r"\(([A-Z]{2,}-[0-9][0-9-]*)\)").expect("valid regex");
    let re_trace = regex::Regex::new(r"trace:([A-Za-z]{2,}-[0-9][0-9-]*)").expect("valid regex");
    let mut ids: Vec<String> = Vec::new();
    for c in re_trailer.captures_iter(msg) {
        let id = c[1].to_ascii_uppercase();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    for c in re_trace.captures_iter(msg) {
        let id = c[1].to_ascii_uppercase();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// STORY-785: blame one line and return its last-touch commit.
///
/// `cwd` is where git runs (the directory the user invoked from, so relative
/// paths resolve exactly as the `fs::read` that preceded this did). Returns
/// `None` for an uncommitted line (all-zero sha), a file outside a repo, or
/// any git error — every miss falls back to the existing "not linked yet"
/// message, so this can only ever ADD answers, never new failure modes.
// trace:STORY-785 | ai:claude
pub(crate) fn blame_line_origin(
    cwd: &std::path::Path,
    path_str: &str,
    line: usize,
) -> Option<BlameOrigin> {
    let out = std::process::Command::new("git")
        .current_dir(cwd)
        .args([
            "blame",
            "-L",
            &format!("{line},{line}"),
            "--porcelain",
            "--",
            path_str,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let sha = text.split_whitespace().next()?.to_string();
    if sha.is_empty() || sha.bytes().all(|b| b == b'0') {
        return None; // uncommitted line
    }
    let show = std::process::Command::new("git")
        .current_dir(cwd)
        .args(["show", "-s", "--format=%h%x00%cs%x00%s%x00%B", &sha])
        .output()
        .ok()?;
    if !show.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&show.stdout);
    let mut parts = s.splitn(4, '\0');
    Some(BlameOrigin {
        short_sha: parts.next()?.trim().to_string(),
        date: parts.next()?.trim().to_string(),
        subject: parts.next()?.trim().to_string(),
        message: parts.next().unwrap_or("").to_string(),
    })
}

/// STORY-754: `aida why <file>[:<line>]` — answer "why does this CODE exist?"
/// from the nearest `trace:SPEC-ID` comment, resolved to the spec's intent.
/// AIDA's one genuine edge over a plain LLM wiki: code↔decision linkage, felt
/// instantly, with zero setup beyond the trace comments already in the tree.
// trace:STORY-754 | ai:claude
pub(crate) fn handle_why_code(arg: &str, json: bool) -> Result<()> {
    // Parse `file[:line]` — a trailing `:<digits>` is the line number.
    let (path_str, line_no): (&str, Option<usize>) = match arg.rsplit_once(':') {
        Some((p, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
            (p, n.parse::<usize>().ok())
        }
        _ => (arg, None),
    };
    let path = std::path::Path::new(path_str);
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read `{}`", path.display()))?;
    let lines: Vec<&str> = content.lines().collect();

    // Find trace ids: the NEAREST at/above the line, else every id in the file.
    let re = regex::Regex::new(r"trace:([A-Za-z]{2,}-[0-9][0-9-]*)").expect("valid regex");
    let mut ids: Vec<String> = Vec::new();
    let mut anchor: Option<usize> = line_no;
    if let Some(ln) = line_no {
        let start = ln.saturating_sub(1).min(lines.len().saturating_sub(1));
        for i in (0..=start).rev() {
            let hits: Vec<String> = re
                .captures_iter(lines[i])
                .map(|c| c[1].to_ascii_uppercase())
                .collect();
            if !hits.is_empty() {
                ids = hits;
                anchor = Some(i + 1);
                break;
            }
        }
    } else {
        for l in &lines {
            for c in re.captures_iter(l) {
                let id = c[1].to_ascii_uppercase();
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }

    // STORY-785: no trace comment at/above the line — fall back to git blame.
    // The last commit to touch the line carries the `(SPEC-ID)` trailer the
    // commit convention requires, so the repository usually knows the answer
    // even where nobody wrote an annotation. Trace comments stay authoritative
    // when present: this runs ONLY on a miss, and the provenance difference
    // (intentional statement vs circumstantial last-touch) is surfaced in both
    // output shapes rather than blended away.
    let mut via_blame: Option<BlameOrigin> = None;
    if ids.is_empty() {
        if let (Some(ln), Ok(cwd)) = (line_no, std::env::current_dir()) {
            if let Some(origin) = blame_line_origin(&cwd, path_str, ln) {
                let blame_ids = spec_ids_from_commit_message(&origin.message);
                if !blame_ids.is_empty() {
                    ids = blame_ids;
                    anchor = Some(ln);
                    via_blame = Some(origin);
                }
            }
        }
    }

    let loc = match anchor {
        Some(l) => format!("{path_str}:{l}"),
        None => path_str.to_string(),
    };

    if ids.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "file": path_str, "line": line_no, "traces": [],
                }))?
            );
        } else {
            let where_ = match line_no {
                Some(l) => format!("at or above {path_str}:{l}"),
                None => format!("in {path_str}"),
            };
            println!(
                "  {} no trace comment {where_} — this code isn't linked to a spec yet.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            println!(
                "  {}",
                "add `// trace:SPEC-ID` above it to record why it exists.".dimmed()
            );
        }
        return Ok(());
    }

    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root);
    let lookup = |id: &str| {
        store.as_ref().and_then(|s| {
            s.requirements.iter().find(|r| {
                [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|x| x.eq_ignore_ascii_case(id))
            })
        })
    };
    // STORY-755: the git-canonical store first, then the plain-markdown fallback
    // so the answer still lands with the machine switched off.
    let resolve = |id: &str| -> Option<ResolvedIntent> {
        if let Some(r) = lookup(id) {
            return Some(ResolvedIntent {
                title: r.title.clone(),
                status: Some(format!("{:?}", r.status).to_ascii_lowercase()),
                why: why_first_sentence(&r.description),
                markdown: None,
            });
        }
        resolve_spec_from_markdown(&project_root, id)
    };

    if json {
        let traces: Vec<serde_json::Value> = ids
            .iter()
            .map(|id| {
                let it = resolve(id);
                serde_json::json!({
                    "id": id,
                    "found": it.is_some(),
                    "title": it.as_ref().map(|i| i.title.clone()),
                    "status": it.as_ref().and_then(|i| i.status.clone()),
                    "why": it.as_ref().map(|i| i.why.clone()),
                    "source": it.as_ref().map(|i| if i.markdown.is_some() { "markdown" } else { "store" }),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "file": path_str, "line": anchor, "traces": traces,
                // STORY-785: how the ids were found — an intentional trace
                // comment, or the line's last-touch commit.
                "resolved_via": if via_blame.is_some() { "blame" } else { "trace" },
                "commit": via_blame.as_ref().map(|o| serde_json::json!({
                    "sha": o.short_sha, "date": o.date, "subject": o.subject,
                })),
            }))?
        );
        return Ok(());
    }

    // Human: the magic answer.
    println!(
        "{} {}",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        loc.bold()
    );
    println!();
    // STORY-785: blame provenance is circumstantial, not an intentional trace
    // comment — say so, and name the commit, so the reader can judge it.
    if let Some(o) = &via_blame {
        println!(
            "  {} commit {} {} — {}",
            "via".dimmed(),
            o.short_sha.yellow(),
            format!("({})", o.date).dimmed(),
            o.subject.dimmed()
        );
        println!(
            "  {}",
            "no trace comment here; answered from the last commit to touch this line".dimmed()
        );
        println!();
    }
    for id in &ids {
        match resolve(id) {
            Some(it) => {
                // STORY-758 polish: only append the status badge when present,
                // so a spec without a `status:` doesn't render a trailing space.
                match it.status.as_deref() {
                    Some(s) => println!(
                        "  this code exists because of {} {}",
                        id.cyan(),
                        format!("({s})").dimmed()
                    ),
                    None => println!("  this code exists because of {}", id.cyan()),
                }
                println!("    {}", it.title.bold());
                if !it.why.is_empty() {
                    println!("    {} {}", "why:".dimmed(), it.why);
                }
                match &it.markdown {
                    Some(p) => println!("    {} {}", "spec:".dimmed(), p.display()),
                    None => println!(
                        "    {} aida show {id}  |  aida graph impact {id}",
                        "more:".dimmed()
                    ),
                }
            }
            None => {
                println!(
                    "  traces to {} — {}",
                    id.cyan(),
                    "no matching spec (add a `<id>.md`, or attach the store)".yellow()
                );
            }
        }
        println!();
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/why_code_tests.rs"]
mod why_code_tests;

pub(crate) fn handle_why(id: &str, plain: bool, json: bool) -> Result<()> {
    // STORY-754: `aida why <file>[:<line>]` answers "why does this CODE exist?"
    // from the nearest trace comment — AIDA's code↔decision edge. A bare SPEC-ID
    // keeps the existing spec-liveness explanation. trace:STORY-754 | ai:claude
    if looks_like_code_arg(id) {
        if plain {
            anyhow::bail!("`aida why --plain` is only available for SPEC-IDs, not code locations.");
        }
        return handle_why_code(id, json);
    }
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;

    // Resolve the spec by agreed/spec id (case-insensitive).
    let want = id.trim().to_ascii_uppercase();
    let req = store.requirements.iter().find(|r| {
        [r.agreed_id.as_deref(), r.spec_id.as_deref()]
            .into_iter()
            .flatten()
            .any(|s| s.eq_ignore_ascii_case(&want))
    });
    let Some(req) = req else {
        anyhow::bail!("no spec found matching `{id}` — check the ID with `aida list`.");
    };

    // Display id for the resolved spec (agreed > spec > internal).
    let disp = req
        .agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| req.id.to_string());

    // STORY-1158: `aida why <spec> --plain` is a Tier-2 surplus layer: it
    // materializes a separate markdown artifact under docs/plain keyed to the
    // spec's modified_at, and never writes back to the spec object itself.
    if plain {
        return handle_why_plain(&project_root, &store, req, &disp, json);
    }

    // BUG-503: an archived (shelved) spec is excluded from the open set, so it
    // would otherwise fall through to the jargon "not in the open set" error.
    // Explain plainly instead — archiving is exactly the kind of "why isn't this
    // moving?" answer `aida why` exists to give. trace:BUG-503
    if req.archived {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "spec": disp,
                    "bucket": "archived",
                    "reason": "archived (shelved)",
                    "needs_human": false,
                }))?
            );
        } else {
            println!(
                "{}{} is archived (shelved) — run `aida unarchive {}` to reactivate it.",
                why_headline_prefix(),
                disp.cyan(),
                disp
            );
        }
        return Ok(());
    }

    // BUG-626: for an epic, classify by its derived rollup status so `aida why`
    // agrees with the rest of the surface (a fully-completed epic is closed; a
    // childless one reads Draft, handled by the open-facts path below).
    // trace:BUG-626 | ai:claude
    let eff_status = effective_display_status(&store, req);

    // TASK-163: terminal requirement status can arrive before the drain has
    // finished its review/merge lifecycle. While the corroborated orchestrator
    // still owns this spec, its live phase is more current than the store's
    // terminal classification and must win.
    // trace:TASK-163 | ai:codex
    if let Some(drain) = drain_state::live_drain_spec(&project_root, &disp) {
        let reason = format!(
            "in-flight — drain phase {}, orchestrator pid {}",
            drain.phase, drain.pid
        );
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "spec": disp,
                    "bucket": "in-flight",
                    "reason": reason,
                    "needs_human": false,
                    "drain": { "phase": drain.phase, "orchestrator_pid": drain.pid },
                }))?
            );
        } else {
            println!(
                "{}{} is {}",
                why_headline_prefix(),
                disp.cyan(),
                reason.green()
            );
        }
        return Ok(());
    }

    // Terminal specs aren't "open" — answer plainly rather than forcing a bucket.
    if matches!(
        eff_status,
        aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
    ) {
        let status = format!("{:?}", eff_status);
        // STORY-729 (FIX 7): name the REALITY behind the terminal status, not
        // just its colour. Pure helper so the Completed-vs-Rejected wording is
        // unit-testable without a store fixture. trace:STORY-729
        let (reason, human) = terminal_why_text(eff_status, &status);
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "spec": disp,
                    "bucket": "closed",
                    "reason": reason,
                    "needs_human": false,
                }))?
            );
        } else {
            println!(
                "{}{} is {} — {}",
                why_headline_prefix(),
                disp.cyan(),
                status.green(),
                human
            );
        }
        return Ok(());
    }

    // trace:STORY-1023 | ai:codex
    if matches!(eff_status, aida_core::RequirementStatus::NeedsAttention) {
        let (reason_req, lens) =
            effective_needs_attention_lens_with_source(&store, req, &eff_status).unwrap_or((
                req,
                status_display::NeedsAttentionLens::NeedsDecision { reason: None },
            ));
        if let status_display::NeedsAttentionLens::Shelved { .. } = lens {
            let Some(fr) = reason_req.failure_reason.as_ref() else {
                anyhow::bail!(
                    "{} has an effective shelved lens but no failure reason source",
                    disp
                );
            };
            let fr = latest_shelved_failure(&project_root, &disp, fr);
            let cause = auto_complete_telemetry::failure_cause_label(Some(&fr.kind));
            let detail = auto_complete_telemetry::failure_detail_first_line(Some(&fr.detail));
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "spec": disp,
                        "bucket": "shelved",
                        "reason": format!("Shelved ({cause}) — {detail}"),
                        "needs_human": false,
                        "failure_reason": {
                            "phase": fr.phase,
                            "kind": fr.kind,
                            "detail": fr.detail,
                            "hint": fr.recovery_hint,
                        },
                    }))?
                );
            } else {
                println!("{}{}", why_headline_prefix(), disp.cyan().bold());
                println!(
                    "  {} {} — {}",
                    crate::glyph(crate::glyphs::Glyph::Pause).blue(),
                    format!("Shelved ({cause})").bold(),
                    detail
                );
                if let Some(hint) = fr.recovery_hint.as_deref() {
                    println!(
                        "    {} hint: {}",
                        crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                        hint.dimmed()
                    );
                }
            }
            return Ok(());
        }
        let reason = reason_req
            .attention_reason
            .as_ref()
            .map(|a| a.category.to_string())
            .unwrap_or_else(|| "no recorded reason".to_string());
        let detail = reason_req
            .attention_reason
            .as_ref()
            .map(|a| a.detail.as_str())
            .unwrap_or("parked for a human decision");
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "spec": disp,
                    "bucket": "needs-decision",
                    "reason": format!("Needs Decision ({reason}) — {detail}"),
                    "needs_human": true,
                }))?
            );
        } else {
            println!("{}{}", why_headline_prefix(), disp.cyan().bold());
            println!(
                "  {} {} — {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                format!("Needs Decision ({reason})").bold(),
                detail
            );
        }
        return Ok(());
    }

    // BUG-623 (subsumed by STORY-694): a spec with a spec-scoped lease whose
    // holder process is NOT live (pid dead, or idle past threshold) is in-flight
    // but STALLED — a hung/abandoned session reads as active otherwise. The Live
    // case still falls through to `collect_open_facts` → `OpenBucket::InFlight`
    // ("being worked now"); only the stale case is rewritten here, so a genuine
    // live session is never mislabeled. trace:BUG-623 | ai:claude
    {
        let leases = list_leases(&project_root);
        let mut ids: Vec<String> = Vec::new();
        if let Some(a) = req.agreed_id.as_deref() {
            ids.push(a.to_string());
        }
        if let Some(s) = req.spec_id.as_deref() {
            ids.push(s.to_string());
        }
        let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        if let Some(lease) = spec_scoped_lease(&leases, &id_refs) {
            let now = chrono::Utc::now();
            let live = process_probe::probe_live_claude_sessions();
            if !matches!(lease_state_for(lease, &live, now), LeaseState::Live) {
                let elapsed = now
                    .signed_duration_since(lease.started_at)
                    .num_seconds()
                    .max(0) as u64;
                let reason = format!(
                    "in-flight but STALLED — {} elapsed, no live process working it (`aida session end {}` to release)",
                    humanize_duration_secs(elapsed),
                    short_lease_id(lease, &leases)
                );
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "spec": disp,
                            "bucket": "stalled",
                            "reason": reason,
                            "needs_human": true,
                        }))?
                    );
                } else {
                    println!("{}{}", why_headline_prefix(), disp.cyan().bold());
                    // The bucket label already says "stalled"; the text reason
                    // drops the redundant "in-flight but STALLED — " prefix the
                    // JSON keeps for a self-contained machine string.
                    let text_reason = reason
                        .strip_prefix("in-flight but STALLED — ")
                        .unwrap_or(&reason);
                    println!(
                        "  {} {} — in-flight but {}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        "stalled".bold(),
                        text_reason
                    );
                }
                return Ok(());
            }
        }
    }

    let in_flight_scopes = in_flight_lease_role_map(&project_root);
    let facts = collect_open_facts(&store, &in_flight_scopes);
    let Some(f) = facts.iter().find(|f| f.id.eq_ignore_ascii_case(&want)) else {
        // BUG-503: archived + terminal specs are answered plainly above, so a
        // resolved spec should always be in the open set here. Keep a plain
        // fallback rather than leaking the "open set" jargon if that ever
        // changes. trace:BUG-503
        anyhow::bail!("{disp} isn't currently open — nothing keeping it back.");
    };
    // TASK-723: the FULL reason set (derived + finding-links + residual notes),
    // most-fundamental-first. trace:TASK-723
    let (bucket, mut reasons) = burndown::explain_reasons(f);

    // BUG-493: the derived HeldForReview reason is computed purely from the
    // `review:draft-only` tag — it asserts a draft PR is held for human review
    // without checking real forge state. For a single-spec drill-down a forge
    // probe is cheap, so reconcile the derived reason against the actual open-PR
    // state: a closed/absent draft PR must not be reported as "held as a draft
    // PR for review".
    // trace:BUG-493 | ai:claude
    if bucket == burndown::OpenBucket::HeldForReview {
        let obs = match detect_open_pr_for_spec(&project_root, &f.id) {
            PrLookup::Found(pr) => burndown::DraftPrObservation::Open(pr.number),
            PrLookup::NoOpenPr => burndown::DraftPrObservation::NoOpenPr,
            // gh missing / failed / unreachable — can't confirm or deny.
            PrLookup::GhMissing | PrLookup::GhFailed(_) | PrLookup::GhUnreachable(_) => {
                burndown::DraftPrObservation::Unverifiable
            }
        };
        let reconciled = burndown::reconcile_held_for_review(&obs);
        // The derived reason is always first (see `explain_reasons`); overwrite
        // its text with the forge-reconciled story, keeping finding-links and
        // residual notes intact.
        if let Some(first) = reasons
            .iter_mut()
            .find(|r| r.source == burndown::ReasonSource::Derived)
        {
            first.text = reconciled;
        }
    }
    let reasons = reasons;
    // BUG-1551: a Done spec with an unresolved BlockedBy predecessor will not
    // auto-complete on merge — say so, naming the blocker, so the hold is
    // visible where "why hasn't this closed?" gets asked. trace:BUG-1551 | ai:claude
    let closure_hold = closure_hold_line(req, &store);
    // TASK-1475 (CR-8 acceptance 6 follow-up): the filing-drift hint. Skip
    // the git-linkage scan entirely when there's no `code_sha` to anchor
    // on — the common case for specs filed before CR-8 — so `aida why`
    // pays for this only when it can actually say something.
    // trace:TASK-1475 | ai:claude
    let drift_hint = req
        .filed_at
        .as_ref()
        .filter(|p| p.code_sha.is_some())
        .and_then(|_| {
            let mut ids: Vec<String> = Vec::new();
            if let Some(a) = req.agreed_id.as_deref() {
                ids.push(a.to_string());
            }
            if let Some(s) = req.spec_id.as_deref() {
                ids.push(s.to_string());
            }
            if ids.is_empty() {
                ids.push(f.id.clone());
            }
            let linkage = collect_git_linkage(&project_root, &ids);
            filing_drift_hint(
                &project_root,
                req.filed_at.as_ref(),
                &linkage.files,
                &linkage.commits,
            )
        });

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "spec": f.id,
                "closure_held_by": closure_hold.as_ref().map(|(ids, _)| ids.clone()),
                // STORY-1430: the spec's own unmet declared closure criteria.
                "closure_unmet_criteria": closure_hold
                    .as_ref()
                    .map(|_| aida_core::pickability::unmet_declared_closure_criteria(req)),
                "bucket": bucket.key(),
                "reason": reasons.first().map(|r| r.text.as_str()).unwrap_or_default(),
                "reasons": reasons
                    .iter()
                    .map(|r| serde_json::json!({"source": r.source.key(), "text": r.text}))
                    .collect::<Vec<_>>(),
                "findings": f.findings,
                // STORY-732: surface the orchestrator failure inline for machine
                // consumers too, not just the human render. trace:STORY-732
                "failure_reason": req.failure_reason.as_ref().map(|fr| serde_json::json!({
                    "phase": fr.phase,
                    "detail": fr.detail,
                    "hint": fr.recovery_hint,
                })),
                "needs_human": bucket.needs_human(),
                // TASK-1475: null when there's nothing to say (no
                // provenance, unresolvable sha, no traced files, no drift).
                "drift_since_filing": drift_hint,
            }))?
        );
        return Ok(());
    }
    let marker = if bucket.needs_human() {
        "●".yellow()
    } else {
        "·".dimmed()
    };
    println!("{}{}", why_headline_prefix(), f.id.cyan().bold());
    // The derived reason gets the bucket header; finding-links + residual notes
    // each get their own labelled line beneath it.
    let mut iter = reasons.iter();
    if let Some(first) = iter.next() {
        println!("  {} {} — {}", marker, bucket.key().bold(), first.text);
    }
    for r in iter {
        let tag = match r.source {
            burndown::ReasonSource::Finding => "finding".magenta(),
            burndown::ReasonSource::Residual => "note".yellow(),
            burndown::ReasonSource::Derived => continue,
        };
        println!("    {} {}", tag, r.text.dimmed());
    }
    if let Some((_, line)) = &closure_hold {
        println!(
            "    {} {}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            line
        );
    }
    // STORY-732 (FIX 2): a NeedsAttention spec the orchestrator shelved carries a
    // FailureReason (phase + detail + recovery hint). The derived reason only
    // redirects to `aida findings list`; inline WHAT failed here so "why is this
    // stuck?" answers in one command. trace:STORY-732 | ai:claude
    if let Some(fr) = &req.failure_reason {
        for line in failure_reason_lines(fr) {
            println!("    {}", line.magenta());
        }
    }
    // TASK-1475 (CR-8 acceptance 6 follow-up): the filing-drift hint.
    // trace:TASK-1475 | ai:claude
    if let Some(hint) = &drift_hint {
        println!("    {}", hint.dimmed());
    }
    // STORY-727: `aida why` previously named no command. The reason text now
    // names it generically (`<id>`); fill in the CONCRETE templated command as a
    // `Next:` block, leading with `aida zen <id>` for an Approved/Planned spec.
    // Gated to the buckets where a forward command makes sense — a Blocked /
    // AwaitingDecision spec's reason already names its own action, so a `zen`
    // suggestion there would contradict it. trace:STORY-727 | ai:claude
    if matches!(
        bucket,
        burndown::OpenBucket::Actionable | burndown::OpenBucket::InProgress
    ) {
        let next = crate::help_next::why_spec_next(&eff_status.to_string(), &f.id, &f.req_type);
        if let Some(block) = crate::help_next::render_human(&next) {
            println!("{block}");
        }
    }
    Ok(())
}

/// BUG-1551: when `req` is Done and an unresolved `BlockedBy` predecessor
/// holds its completion, the blocker ids plus the human line `aida why`
/// prints. `None` when nothing holds closure.
// trace:BUG-1551 | ai:claude
pub(crate) fn closure_hold_line(
    req: &aida_core::Requirement,
    store: &aida_core::RequirementsStore,
) -> Option<(Vec<String>, String)> {
    if !matches!(req.status, RequirementStatus::Done) {
        return None;
    }
    let blockers = aida_core::pickability::unresolved_closure_blockers(req, store);
    // STORY-1430: the spec's own declared-unmet closure criteria hold it too.
    // trace:STORY-1430 | ai:claude
    let criteria = aida_core::pickability::unmet_declared_closure_criteria(req);
    if blockers.is_empty() && criteria.is_empty() {
        return None;
    }
    let ids = blockers.iter().map(|b| b.id.clone()).collect();
    let mut parts = Vec::new();
    if !blockers.is_empty() {
        parts.push(format!(
            "blocked by {} (until every blocker is Completed, Rejected or Superseded)",
            aida_core::pickability::closure_blockers_label(&blockers)
        ));
    }
    if !criteria.is_empty() {
        parts.push(format!(
            "its own closure criteria are unmet: {} (resolve by checking the box in its \
             Closure section or removing the `{}` tag)",
            closure_criteria_label(&criteria),
            aida_core::pickability::CLOSURE_PENDING_TAG
        ));
    }
    Some((
        ids,
        format!(
            "completion held — {}; stays Done after merge until resolved",
            parts.join("; ")
        ),
    ))
}

/// STORY-1430: one-line rendering of unmet declared closure criteria.
// trace:STORY-1430 | ai:claude
pub(crate) fn closure_criteria_label(criteria: &[String]) -> String {
    criteria
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) const PLAIN_WHY_CACHE_PREFIX: &str = "<!-- aida-plain";

pub(crate) fn plain_why_cache_path(
    project_root: &std::path::Path,
    disp: &str,
) -> std::path::PathBuf {
    project_root
        .join("docs")
        .join("plain")
        .join(format!("{disp}.md"))
}

pub(crate) fn plain_why_cache_is_fresh(raw: &str, source_modified_at: &str) -> bool {
    raw.lines()
        .next()
        .map(|line| {
            line.contains(PLAIN_WHY_CACHE_PREFIX)
                && line.contains(&format!("source_modified_at=\"{source_modified_at}\""))
        })
        .unwrap_or(false)
}

pub(crate) fn plain_why_first_sentence(text: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let compact = compact.trim();
    if compact.is_empty() {
        return "No detailed description is recorded yet.".to_string();
    }
    for sep in [". ", "! ", "? "] {
        if let Some(idx) = compact.find(sep) {
            return compact[..idx + 1].to_string();
        }
    }
    compact.chars().take(260).collect()
}

pub(crate) fn plain_why_acceptance_lines(description: &str) -> Vec<String> {
    let mut in_acceptance = false;
    let mut out = Vec::new();
    for line in description.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") && in_acceptance {
            break;
        }
        if trimmed.eq_ignore_ascii_case("Acceptance:")
            || trimmed.eq_ignore_ascii_case("## Acceptance")
            || trimmed.eq_ignore_ascii_case("### Acceptance")
        {
            in_acceptance = true;
            continue;
        }
        if in_acceptance {
            let item = trimmed
                .trim_start_matches("- ")
                .trim_start_matches("* ")
                .trim();
            if !item.is_empty() {
                out.push(item.to_string());
            }
        }
    }
    out
}

pub(crate) fn plain_why_neighbors(
    store: &aida_core::RequirementsStore,
    req: &aida_core::Requirement,
) -> Vec<String> {
    let mut rows = Vec::new();
    for rel in &req.relationships {
        if let Some(target) = store.requirements.iter().find(|r| r.id == rel.target_id) {
            let rel_label = match rel.rel_type {
                aida_core::RelationshipType::Child => "child of".to_string(),
                aida_core::RelationshipType::Parent => "parent of".to_string(),
                _ => rel.rel_type.to_string(),
            };
            rows.push(format!(
                "{}: {} — {} ({:?})",
                rel_label,
                target.display_id(),
                target.title,
                target.status
            ));
        }
    }
    rows.sort();
    rows.dedup();
    rows
}

pub(crate) fn plain_why_linked_adrs(
    store: &aida_core::RequirementsStore,
    req: &aida_core::Requirement,
) -> Vec<String> {
    req.relationships
        .iter()
        .filter_map(|rel| store.requirements.iter().find(|r| r.id == rel.target_id))
        .filter(|target| {
            let id = target.display_id();
            id.starts_with("ADR-")
                || matches!(target.req_type, aida_core::RequirementType::Decision)
        })
        .map(|target| format!("{} — {}", target.display_id(), target.title))
        .collect()
}

pub(crate) fn render_plain_why(
    store: &aida_core::RequirementsStore,
    req: &aida_core::Requirement,
    disp: &str,
    generated_at: &str,
) -> String {
    let source_modified_at = req.modified_at.to_rfc3339();
    let status = format!("{:?}", req.status);
    let summary = plain_why_first_sentence(&req.description);
    let acceptance = plain_why_acceptance_lines(&req.description);
    let neighbors = plain_why_neighbors(store, req);
    let adrs = plain_why_linked_adrs(store, req);
    let first_acceptance = acceptance
        .first()
        .map(|s| s.trim_end_matches(['.', '!', '?']).to_string())
        .unwrap_or_else(|| "the behavior described by the spec works end to end".to_string());

    let mut out = String::new();
    out.push_str(&format!(
        "{PLAIN_WHY_CACHE_PREFIX} spec=\"{disp}\" source_modified_at=\"{source_modified_at}\" generated_at=\"{generated_at}\" -->\n"
    ));
    out.push_str(&format!("# Plain-English Why: {disp}\n\n"));
    out.push_str(&format!("## Rationale\n\n{summary}\n\n"));
    out.push_str(
        "In plain terms: this work exists so someone can understand the reason for the spec without first knowing AIDA's internal vocabulary. ",
    );
    out.push_str("It keeps the formal spec unchanged, then adds a separate explanation layer that a human can ask for when the terse contract is not enough.\n\n");
    out.push_str("## Jargon In Plain English\n\n");
    out.push_str("- Spec: the tracked requirement or work item.\n");
    out.push_str("- Graph context: the nearby parent, child, blocker, and reference links that explain how this work fits with other work.\n");
    out.push_str("- Cache: a saved copy that is reused until the spec changes.\n");
    out.push_str("- Surplus context: useful background that is available on demand but not loaded into every agent prompt by default.\n\n");
    out.push_str("## Concrete Novice Example\n\n");
    out.push_str(&format!(
        "Imagine a new contributor runs `aida why {disp} --plain` before touching code. They should learn that the current goal is: {first_acceptance}. They can then inspect the linked context below, make the change, and know that this explanatory note will be reused until the spec is edited again.\n\n"
    ));
    out.push_str("## Spec Snapshot\n\n");
    out.push_str(&format!("- Title: {}\n", req.title));
    out.push_str(&format!("- Status: {status}\n"));
    out.push_str(&format!("- Type: {:?}\n", req.req_type));
    out.push_str(&format!("- Source modified_at: {source_modified_at}\n\n"));

    if !acceptance.is_empty() {
        out.push_str("## Acceptance In Everyday Words\n\n");
        for item in acceptance.iter().take(6) {
            out.push_str(&format!("- {item}\n"));
        }
        out.push('\n');
    }

    out.push_str("## Graph Context\n\n");
    if neighbors.is_empty() {
        out.push_str("- No immediate relationships are recorded on this spec.\n\n");
    } else {
        for row in neighbors {
            out.push_str(&format!("- {row}\n"));
        }
        out.push('\n');
    }

    out.push_str("## Linked Decisions\n\n");
    if adrs.is_empty() {
        out.push_str("- No linked ADR/decision records were found in the immediate graph.\n");
    } else {
        for adr in adrs {
            out.push_str(&format!("- {adr}\n"));
        }
    }
    out
}

pub(crate) fn handle_why_plain(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
    req: &aida_core::Requirement,
    disp: &str,
    json: bool,
) -> Result<()> {
    let cache_path = plain_why_cache_path(project_root, disp);
    let source_modified_at = req.modified_at.to_rfc3339();
    let cached = std::fs::read_to_string(&cache_path).ok();
    let (body, cache_status) = match cached {
        Some(raw) if plain_why_cache_is_fresh(&raw, &source_modified_at) => (raw, "hit"),
        _ => {
            let generated_at = chrono::Utc::now().to_rfc3339();
            let body = render_plain_why(store, req, disp, &generated_at);
            if let Some(parent) = cache_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&cache_path, &body)
                .with_context(|| format!("writing {}", cache_path.display()))?;
            (body, "regenerated")
        }
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "spec": disp,
                "plain_artifact": cache_path,
                "cache": cache_status,
                "source_modified_at": source_modified_at,
                "content": body,
            }))?
        );
    } else {
        println!("{body}");
        println!();
        println!(
            "Plain layer: {} ({cache_status}; separate surplus artifact)",
            cache_path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod why_plain_tests {
    use super::*;

    fn sample_req() -> aida_core::Requirement {
        let mut req = aida_core::Requirement::new(
            "Plain why".to_string(),
            "Make specs easier to understand.\n\nAcceptance:\n- Print a plain rationale.\n- Cache separately.".to_string(),
        );
        req.agreed_id = Some("STORY-1158".to_string());
        req
    }

    #[test]
    fn plain_cache_key_uses_source_modified_at() {
        let req = sample_req();
        let ts = req.modified_at.to_rfc3339();
        let raw = format!(
            "<!-- aida-plain spec=\"STORY-1158\" source_modified_at=\"{ts}\" generated_at=\"now\" -->\nbody"
        );
        assert!(plain_why_cache_is_fresh(&raw, &ts));
        assert!(!plain_why_cache_is_fresh(&raw, "2026-01-01T00:00:00+00:00"));
    }

    #[test]
    fn plain_render_is_separate_markdown_with_example() {
        let req = sample_req();
        let mut store = aida_core::RequirementsStore::new();
        store.requirements.push(req.clone());
        let body = render_plain_why(&store, &req, "STORY-1158", "2026-09-15T00:00:00Z");
        assert!(body.contains("Plain-English Why: STORY-1158"));
        assert!(body.contains("Concrete Novice Example"));
        assert!(body.contains("Print a plain rationale"));
        assert!(body.contains("source_modified_at="));
    }
}
