//! The shipped gate library — named discipline checklists invoked on demand.
//!
//! A gate is *called*, not *carried*: its checklist costs context only at the
//! moment it fires, instead of living as ambient prose in every session's
//! CLAUDE.md. Gate definitions are markdown files under
//! `aida-core/templates/gates/*.md`, embedded at compile time by `build.rs`.
//!
//! Each gate is two-tier (PRIN-8, deterministic first):
//! - **Tier 1 (deterministic)** — an optional programmatic pass (today the EARS
//!   lint from [`crate::ears_lint`]), reproducible, no model call.
//! - **Tier 2 (heuristic, rung 4)** — the markdown checklist an agent follows
//!   for what tier 1 cannot decide. Always advisory; never blocks at intake.
//!
//! Every verdict carries the gate's name and version (`well-formed@v1`) so a
//! later reader knows what judged the spec and on what.
//!
//! trace:STORY-1427 | ai:claude

use crate::ears_lint::{lint_text, Category, Finding};
use crate::templates::EMBEDDED_TEMPLATES;

/// Key prefix of gate definitions inside the embedded template map.
const GATE_PREFIX: &str = "gates/";

/// The deterministic (tier-1) pass a gate runs before its agent checklist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeterministicPass {
    /// Run the EARS lint; an empty filter keeps every category.
    EarsLint { categories: Vec<Category> },
}

/// One parsed gate definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateDef {
    /// Stable name, e.g. `well-formed`.
    pub name: String,
    /// Monotonic version, bumped whenever the checklist changes meaning.
    pub version: u32,
    /// One-line description shown by `aida gate list`.
    pub summary: String,
    /// Lifecycle moments this gate is a DEFAULT for (e.g. `groom`).
    pub binds_to: Vec<String>,
    /// Optional tier-1 pass.
    pub deterministic: Option<DeterministicPass>,
    /// The tier-2 checklist body (markdown, frontmatter stripped).
    pub checklist: String,
}

impl GateDef {
    /// `name@vN` — the label every verdict records.
    pub fn label(&self) -> String {
        format!("{}@v{}", self.name, self.version)
    }
}

fn category_from_slug(slug: &str) -> Option<Category> {
    [
        Category::EmptyBody,
        Category::VagueTrigger,
        Category::MissingBehavior,
        Category::ConflictingConstraint,
        Category::LowTestability,
    ]
    .into_iter()
    .find(|c| c.slug() == slug)
}

fn parse_deterministic(value: &str) -> Option<DeterministicPass> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (kind, filter) = match value.split_once(':') {
        Some((k, f)) => (k.trim(), f),
        None => (value, ""),
    };
    match kind {
        "ears-lint" => Some(DeterministicPass::EarsLint {
            categories: filter
                .split(',')
                .filter_map(|s| category_from_slug(s.trim()))
                .collect(),
        }),
        _ => None,
    }
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse one gate markdown file (`---` frontmatter + body). Returns `None` when
/// the frontmatter is missing or carries no `name`.
pub fn parse_gate(content: &str) -> Option<GateDef> {
    let rest = content.strip_prefix("---")?;
    let (front, body) = rest.split_once("\n---")?;
    let mut name = None;
    let mut version = 1;
    let mut summary = String::new();
    let mut binds_to = Vec::new();
    let mut deterministic = None;
    for line in front.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "name" if !value.is_empty() => name = Some(value.to_string()),
            "version" => version = value.parse().unwrap_or(1),
            "summary" => summary = value.to_string(),
            "binds_to" => binds_to = split_csv(value),
            "deterministic" => deterministic = parse_deterministic(value),
            _ => {}
        }
    }
    Some(GateDef {
        name: name?,
        version,
        summary,
        binds_to,
        deterministic,
        checklist: body.trim_start_matches('-').trim().to_string(),
    })
}

/// Every shipped gate, sorted by name.
pub fn library() -> Vec<GateDef> {
    let mut gates: Vec<GateDef> = EMBEDDED_TEMPLATES
        .iter()
        .filter(|(key, _)| key.starts_with(GATE_PREFIX) && key.ends_with(".md"))
        .filter_map(|(_, content)| parse_gate(content))
        .collect();
    gates.sort_by(|a, b| a.name.cmp(&b.name));
    gates
}

/// Look up one gate by name (case-insensitive).
pub fn find(name: &str) -> Option<GateDef> {
    library()
        .into_iter()
        .find(|g| g.name.eq_ignore_ascii_case(name.trim()))
}

/// The gates that are DEFAULTS at a lifecycle moment (e.g. `groom`).
pub fn defaults_for(moment: &str) -> Vec<GateDef> {
    library()
        .into_iter()
        .filter(|g| g.binds_to.iter().any(|m| m.eq_ignore_ascii_case(moment)))
        .collect()
}

/// Resolve a list of gate names, erroring with the valid set on an unknown one.
pub fn resolve(names: &[String]) -> Result<Vec<GateDef>, String> {
    names
        .iter()
        .map(|n| {
            find(n).ok_or_else(|| {
                let valid: Vec<String> = library().into_iter().map(|g| g.name).collect();
                format!("unknown gate \"{n}\" — valid gates: {}", valid.join(", "))
            })
        })
        .collect()
}

/// The result of running a gate over one piece of spec text.
#[derive(Debug, Clone)]
pub struct GateVerdict {
    /// `name@vN` of the gate that produced this verdict.
    pub gate: String,
    /// Tier-1 findings, or `None` when the gate has no deterministic pass.
    pub deterministic: Option<Vec<Finding>>,
    /// The tier-2 checklist the agent must still answer (heuristic, advisory).
    pub checklist: String,
}

impl GateVerdict {
    /// Tier-1 outcome token: `pass`, `warn`, or `n/a` (no deterministic tier).
    pub fn tier1_outcome(&self) -> &'static str {
        match &self.deterministic {
            None => "n/a",
            Some(f) if f.is_empty() => "pass",
            Some(_) => "warn",
        }
    }
}

/// Run a gate's deterministic tier over `text` and package its checklist.
pub fn run_gate(gate: &GateDef, text: &str) -> GateVerdict {
    let deterministic = gate.deterministic.as_ref().map(|pass| match pass {
        DeterministicPass::EarsLint { categories } => lint_text(text)
            .findings
            .into_iter()
            .filter(|f| categories.is_empty() || categories.contains(&f.category))
            .collect(),
    });
    GateVerdict {
        gate: gate.label(),
        deterministic,
        checklist: gate.checklist.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_ships_named_gates() {
        let names: Vec<String> = library().into_iter().map(|g| g.name).collect();
        for want in ["well-formed", "testable-acceptance", "single-deliverable"] {
            assert!(names.iter().any(|n| n == want), "missing {want}: {names:?}");
        }
    }

    #[test]
    fn well_formed_is_default_at_groom_with_lint_tier() {
        let groom: Vec<String> = defaults_for("groom").into_iter().map(|g| g.name).collect();
        assert!(groom.contains(&"well-formed".to_string()), "{groom:?}");
        let g = find("WELL-FORMED").expect("case-insensitive lookup");
        assert_eq!(
            g.deterministic,
            Some(DeterministicPass::EarsLint { categories: vec![] })
        );
        assert_eq!(g.label(), "well-formed@v1");
        assert!(!g.checklist.starts_with("---"));
        assert!(g.checklist.contains("Tier 2"));
    }

    #[test]
    fn parse_gate_reads_filter_and_empty_fields() {
        let g = parse_gate(
            "---\nname: x\nversion: 3\nsummary: s\nbinds_to:\ndeterministic: ears-lint:low-testability\n---\n# body\n",
        )
        .unwrap();
        assert_eq!(g.version, 3);
        assert!(g.binds_to.is_empty());
        assert_eq!(
            g.deterministic,
            Some(DeterministicPass::EarsLint {
                categories: vec![Category::LowTestability]
            })
        );
        assert_eq!(g.checklist, "# body");
        assert!(parse_gate("no frontmatter").is_none());
        assert!(parse_gate("---\nversion: 1\n---\nbody").is_none());
    }

    #[test]
    fn run_gate_reports_tier1_and_records_version() {
        let g = find("well-formed").unwrap();
        let v = run_gate(&g, "");
        assert_eq!(v.gate, "well-formed@v1");
        assert_eq!(v.tier1_outcome(), "warn", "empty body must trip the lint");

        let heuristic_only = find("single-deliverable").unwrap();
        let v = run_gate(&heuristic_only, "");
        assert_eq!(v.tier1_outcome(), "n/a");
    }

    #[test]
    fn resolve_rejects_unknown_with_valid_set() {
        let err = resolve(&["nope".to_string()]).unwrap_err();
        assert!(err.contains("well-formed"), "{err}");
        assert_eq!(resolve(&["well-formed".to_string()]).unwrap().len(), 1);
    }
}
