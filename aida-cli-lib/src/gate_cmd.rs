//! `aida gate` — the shipped gate library as an invocable surface, plus the
//! `aida add --gates` intake hook.
//!
//! A gate is discipline that is *called* rather than *carried*: its checklist
//! enters an agent's context only when the gate fires. The definitions and the
//! two-tier run logic live in `aida_core::gates`; this module is presentation.
//!
//! trace:STORY-1427 | ai:claude

use anyhow::Result;
use colored::Colorize;

use aida_core::gates::{self, GateDef, GateVerdict};

use crate::cli::GateCommand;
use crate::{find_project_root, load_store_for_lookup};

pub(crate) fn handle_gate_command(cmd: &GateCommand) -> Result<()> {
    match cmd {
        GateCommand::List { moment, json } => {
            let list = match moment {
                Some(m) => gates::defaults_for(m),
                None => gates::library(),
            };
            if *json {
                let rows: Vec<serde_json::Value> = list
                    .iter()
                    .map(|g| {
                        serde_json::json!({
                            "name": g.name,
                            "version": g.version,
                            "summary": g.summary,
                            "default_at": g.binds_to,
                            "deterministic_tier": g.deterministic.is_some(),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                print!("{}", render_list(&list));
            }
            Ok(())
        }
        GateCommand::Show { name } => {
            let gate = gates::resolve(std::slice::from_ref(name)).map_err(anyhow::Error::msg)?;
            let gate = &gate[0];
            println!("{}  {}\n", gate.label().bold(), gate.summary);
            println!("{}", gate.checklist);
            Ok(())
        }
        GateCommand::Run { name, spec, json } => {
            let gate = gates::resolve(std::slice::from_ref(name)).map_err(anyhow::Error::msg)?;
            let project_root = find_project_root()?;
            let store = load_store_for_lookup(&project_root)
                .ok_or_else(|| anyhow::anyhow!("could not load the AIDA requirements store"))?;
            let req = store
                .requirements
                .iter()
                .find(|r| {
                    r.display_id().eq_ignore_ascii_case(spec)
                        || r.spec_id
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case(spec))
                        || r.agreed_id
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case(spec))
                })
                .ok_or_else(|| anyhow::anyhow!("no requirement found for {spec}"))?;
            let mut text = format!("{}\n{}", req.title, req.description);
            if let Some(acc) = req.custom_fields.get("acceptance_criteria") {
                text.push('\n');
                text.push_str(acc);
            }
            let verdict = gates::run_gate(&gate[0], &text);
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&verdict_json(&req.display_id(), &verdict))?
                );
            } else {
                print!("{}", render_verdict(&req.display_id(), &verdict, true));
            }
            Ok(())
        }
    }
}

/// `aida add --gates a,b` — run the named gates over the not-yet-filed spec
/// text. Advisory: prints each verdict and never blocks the filing. Errors only
/// on an unknown gate name (before anything is written).
pub(crate) fn run_intake_gates(names: &[String], title: &str, description: &str) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let resolved = gates::resolve(names).map_err(anyhow::Error::msg)?;
    let text = format!("{title}\n{description}");
    for gate in &resolved {
        let verdict = gates::run_gate(gate, &text);
        eprint!("{}", render_verdict("new spec", &verdict, false));
    }
    Ok(())
}

fn render_list(list: &[GateDef]) -> String {
    let mut out = String::new();
    if list.is_empty() {
        out.push_str("No gates match.\n");
        return out;
    }
    for g in list {
        let default_at = if g.binds_to.is_empty() {
            "on demand".to_string()
        } else {
            format!("default at: {}", g.binds_to.join(", "))
        };
        let tiers = if g.deterministic.is_some() {
            "deterministic + checklist"
        } else {
            "checklist only"
        };
        out.push_str(&format!(
            "{:<22} v{}  [{}; {}]\n    {}\n",
            g.name, g.version, tiers, default_at, g.summary
        ));
    }
    out.push_str("\nRun one: aida gate run <name> <SPEC>   Read one: aida gate show <name>\n");
    out
}

/// Render a verdict. `with_checklist` prints the tier-2 body inline (for
/// `aida gate run`); intake prints only a pointer to keep `aida add` terse.
fn render_verdict(target: &str, v: &GateVerdict, with_checklist: bool) -> String {
    let mut out = format!(
        "gate {} on {}: tier 1 (deterministic) {}\n",
        v.gate,
        target,
        v.tier1_outcome()
    );
    if let Some(findings) = &v.deterministic {
        for f in findings {
            out.push_str(&format!("  [tier 1] {}: {}\n", f.category, f.message));
        }
    }
    let name = v.gate.split('@').next().unwrap_or(&v.gate);
    if with_checklist {
        out.push_str("tier 2 (heuristic, advisory) — answer this checklist:\n\n");
        out.push_str(&v.checklist);
        out.push('\n');
    } else {
        out.push_str(&format!(
            "  [tier 2] heuristic checklist pending: aida gate show {name}\n"
        ));
    }
    out
}

fn verdict_json(target: &str, v: &GateVerdict) -> serde_json::Value {
    serde_json::json!({
        "gate": v.gate,
        "spec": target,
        "tier1": {
            "outcome": v.tier1_outcome(),
            "findings": v.deterministic.as_ref().map(|fs| fs.iter().map(|f| serde_json::json!({
                "category": f.category.slug(),
                "message": f.message,
                "evidence": f.evidence,
                "suggestion": f.suggestion,
            })).collect::<Vec<_>>()),
        },
        "tier2": { "rung": 4, "advisory": true, "checklist": v.checklist },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_shows_default_binding_without_a_catalogue_read() {
        let out = render_list(&gates::library());
        assert!(out.contains("well-formed"), "{out}");
        assert!(out.contains("default at: groom"), "{out}");
        assert!(out.contains("on demand"), "{out}");
    }

    #[test]
    fn verdict_labels_tiers_and_version() {
        let g = gates::find("well-formed").unwrap();
        let v = gates::run_gate(&g, "");
        let terse = render_verdict("new spec", &v, false);
        assert!(terse.contains("gate well-formed@v1"), "{terse}");
        assert!(terse.contains("[tier 1] empty-body"), "{terse}");
        assert!(terse.contains("aida gate show well-formed"), "{terse}");
        assert!(!terse.contains("Tier 2 (heuristic"), "intake stays terse");
        let full = render_verdict("STORY-1", &v, true);
        assert!(full.contains("Tier 2 (heuristic"), "{full}");
        let j = verdict_json("STORY-1", &v);
        assert_eq!(j["gate"], "well-formed@v1");
        assert_eq!(j["tier1"]["outcome"], "warn");
        assert_eq!(j["tier2"]["rung"], 4);
    }

    #[test]
    fn intake_gates_noop_without_flag_and_reject_unknown() {
        assert!(run_intake_gates(&[], "t", "d").is_ok());
        let err = run_intake_gates(&["nope".into()], "t", "d").unwrap_err();
        assert!(err.to_string().contains("unknown gate"), "{err}");
    }

    #[test]
    fn add_parses_gates_flag() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "aida",
            "add",
            "--title",
            "x",
            "--gates",
            "well-formed,testable-acceptance",
        ])
        .unwrap();
        match cli.command {
            crate::cli::Command::Add { gates, .. } => {
                assert_eq!(gates, vec!["well-formed", "testable-acceptance"])
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
