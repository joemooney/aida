//! `aida lint` — the opt-in EARS-style quality lens (TASK-0417).
//!
//! Extracted verbatim from `main.rs` (SPIKE-78, pure movement). The EARS
//! analysis engine lives in `aida_core::ears_lint`; this CLI handler is the
//! thin presentation layer that selects specs, scores them, and renders the
//! findings (human table + `--json`).

use anyhow::Result;

use crate::{find_project_root, load_store_for_lookup};

/// `aida lint [<SPEC>|--scope feature|task|story] [--json]` — opt-in EARS-style
/// quality lint over requirement text. AIDA stays a graph-first substrate;
/// EARS is offered here as an optional clarity lens. The pass is read-only and
/// deterministic (heuristic, no LLM): it scores each spec's description plus
/// acceptance criteria for vague triggers, missing expected behavior,
/// conflicting constraints, and low-testability wording, and prints suggested
/// rewrites as drafts only — it never mutates a spec. Exits non-zero when any
/// finding is reported so it can gate a pre-commit hook or a drain step.
// trace:TASK-0417 | ai:claude
pub(crate) fn handle_lint_command(
    spec: Option<&str>,
    scope: Option<&str>,
    json: bool,
) -> Result<()> {
    use aida_core::ears_lint::lint_text;
    use colored::Colorize;

    let project_root = find_project_root()?;
    let store = load_store_for_lookup(&project_root)
        .ok_or_else(|| anyhow::anyhow!("could not load the AIDA requirements store"))?;

    // Select the specs to lint: a single SPEC-ID, or every spec of a scope kind.
    let targets: Vec<&aida_core::Requirement> = if let Some(id) = spec {
        let id_lower = id.to_ascii_lowercase();
        let req = store
            .requirements
            .iter()
            .find(|r| {
                r.display_id().eq_ignore_ascii_case(id)
                    || r.spec_id.as_deref().map(|s| s.to_ascii_lowercase())
                        == Some(id_lower.clone())
                    || r.agreed_id.as_deref().map(|s| s.to_ascii_lowercase())
                        == Some(id_lower.clone())
            })
            .ok_or_else(|| anyhow::anyhow!("no requirement found for {id}"))?;
        vec![req]
    } else if let Some(kind) = scope {
        let wanted = match kind.to_ascii_lowercase().as_str() {
            "feature" => vec![
                aida_core::RequirementType::Functional,
                aida_core::RequirementType::NonFunctional,
                aida_core::RequirementType::System,
                aida_core::RequirementType::User,
            ],
            "task" => vec![aida_core::RequirementType::Task],
            "story" => vec![aida_core::RequirementType::Story],
            other => {
                anyhow::bail!("unknown scope \"{other}\" — expected one of: feature, task, story");
            }
        };
        store
            .requirements
            .iter()
            .filter(|r| !r.archived && wanted.contains(&r.req_type))
            .collect()
    } else {
        anyhow::bail!("provide a SPEC-ID or --scope feature|task|story");
    };

    // Score each target on its description + acceptance criteria.
    struct SpecLint<'a> {
        req: &'a aida_core::Requirement,
        report: aida_core::ears_lint::LintReport,
    }
    let scored: Vec<SpecLint> = targets
        .into_iter()
        .map(|req| {
            let mut text = req.description.clone();
            // Acceptance criteria most often live inside the description under a
            // `## Acceptance` heading; fold in the custom field too when present.
            if let Some(acc) = req.custom_fields.get("acceptance_criteria") {
                text.push('\n');
                text.push_str(acc);
            }
            let mut report = lint_text(&text);
            // Don't over-flag legitimately-terse stateless types: a folder, a
            // meta prompt holder, or a glossary term is allowed to carry little
            // body. The other EARS heuristics never fire on these (they have no
            // behavior clause to mis-read), so we only suppress the empty-body
            // finding for them. trace:TASK-884
            if matches!(
                req.req_type,
                aida_core::RequirementType::Folder
                    | aida_core::RequirementType::Meta
                    | aida_core::RequirementType::Term
            ) {
                report
                    .findings
                    .retain(|f| f.category != aida_core::ears_lint::Category::EmptyBody);
            }
            SpecLint { req, report }
        })
        .collect();

    let total_findings: usize = scored.iter().map(|s| s.report.findings.len()).sum();

    if json {
        let arr: Vec<serde_json::Value> = scored
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.req.display_id(),
                    "title": s.req.title,
                    "clean": s.report.is_clean(),
                    "findings": s.report.findings.iter().map(|f| serde_json::json!({
                        "category": f.category.slug(),
                        "message": f.message,
                        "evidence": f.evidence,
                        "suggestion": f.suggestion,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "lens": "ears",
                "graph_first_substrate": true,
                "total_findings": total_findings,
                "specs": arr,
            }))?
        );
        return if total_findings == 0 {
            Ok(())
        } else {
            std::process::exit(1);
        };
    }

    println!(
        "{}",
        "EARS lint — optional clarity lens (AIDA stays a graph-first substrate; \
         suggestions are drafts, never auto-applied)"
            .dimmed()
    );
    println!();

    for s in &scored {
        let header = format!("{}  {}", s.req.display_id(), s.req.title);
        if s.report.is_clean() {
            println!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                header
            );
            continue;
        }
        println!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            header.bold()
        );
        for f in &s.report.findings {
            println!(
                "    {} {}",
                format!("[{}]", f.category.slug()).yellow(),
                f.message
            );
            println!("      {} {}", "suggest:".dimmed(), f.suggestion);
        }
        println!();
    }

    if total_findings == 0 {
        println!("{}", "All specs passed the EARS lens.".green());
        Ok(())
    } else {
        println!(
            "{}",
            format!("{total_findings} finding(s) — review the suggested rewrites above.").yellow()
        );
        std::process::exit(1);
    }
}
