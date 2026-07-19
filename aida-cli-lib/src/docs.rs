//! Project the requirements graph as a layered docs tree.
//!
//! Reads the live store via the cached git backend and renders six layers
//! into `docs/aida/`:
//!
//! - `00-constitution.md`     — Principles
//! - `01-vision.md`           — Vision items
//! - `02-constraints.md`      — Constraints
//! - `05-decisions/ADR-*.md`  — Decisions (one file per ADR)
//! - `07-quality.md`          — Non-Functional requirements
//! - `10-glossary.md`         — Terms
//!
//! Plus a top-level `README.md` that links the layers.
//!
//! Each generated file wraps its body in `<!-- AIDA-AUTOGEN-BEGIN -->` /
//! `<!-- AIDA-AUTOGEN-END -->` markers. v1 regenerates the whole body on
//! every build; future iterations can preserve manual prose between the
//! markers using `extract_aida_block` from `aida_core::scaffolding`.
//!
//! trace:FR-1-077 | ai:claude

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use aida_core::models::{Requirement, RequirementType, RequirementsStore};

const AUTOGEN_BEGIN: &str = "<!-- AIDA-AUTOGEN-BEGIN -->";
const AUTOGEN_END: &str = "<!-- AIDA-AUTOGEN-END -->";

/// Result of a build pass — what was written, what stayed identical, and
/// (for `check` mode) what would have changed.
#[derive(Debug, Default)]
pub struct BuildReport {
    pub written: Vec<PathBuf>,
    pub unchanged: Vec<PathBuf>,
    pub drifted: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

impl BuildReport {
    pub fn has_drift(&self) -> bool {
        !self.drifted.is_empty()
    }
}

/// Build the docs tree at `output_dir`. When `dry_run` is true, files are
/// not written but the report still tracks what would change.
pub fn build(store: &RequirementsStore, output_dir: &Path, dry_run: bool) -> Result<BuildReport> {
    let mut report = BuildReport::default();
    let mut planned: Vec<(PathBuf, String)> = vec![
        (output_dir.join("README.md"), render_index(store)),
        (
            output_dir.join("00-constitution.md"),
            render_constitution(store),
        ),
        (output_dir.join("01-vision.md"), render_vision(store)),
        (
            output_dir.join("02-constraints.md"),
            render_constraints(store),
        ),
        (output_dir.join("07-quality.md"), render_quality(store)),
        (output_dir.join("10-glossary.md"), render_glossary(store)),
    ];

    // Decisions: one file per Decision req, plus an index — sorted by id
    // so ADR-1 lands before ADR-2 in the index. trace:BUG-20 | ai:claude
    let decisions_dir = output_dir.join("05-decisions");
    let decisions = filter_type_sorted(store, &RequirementType::Decision);
    planned.push((
        decisions_dir.join("README.md"),
        render_decisions_index(&decisions),
    ));
    for d in &decisions {
        let filename = decision_filename(d);
        planned.push((decisions_dir.join(filename), render_decision(d)));
    }

    // Apply: write only when content differs.
    for (path, body) in &planned {
        let body_with_marker = wrap_autogen(body);
        let existing = std::fs::read_to_string(path).ok();
        let unchanged = existing.as_deref() == Some(body_with_marker.as_str());
        if unchanged {
            report.unchanged.push(path.clone());
            continue;
        }

        if !dry_run {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create directory {}", parent.display()))?;
            }
            // Atomic write — uniform with the concurrent-writer paths. trace:TASK-331 | ai:claude
            aida_core::write_atomic(path, &body_with_marker)
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        if existing.is_some() {
            report.drifted.push(path.clone());
        } else {
            report.written.push(path.clone());
        }
    }

    // Stale ADR files (Decision was deleted) — surface but don't auto-delete
    // in v1; user can `aida docs build` after a manual cleanup.
    if let Ok(entries) = std::fs::read_dir(&decisions_dir) {
        let planned_paths: std::collections::HashSet<PathBuf> =
            planned.iter().map(|(p, _)| p.clone()).collect();
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            if !planned_paths.contains(&path) {
                report.deleted.push(path);
            }
        }
    }

    Ok(report)
}

fn wrap_autogen(body: &str) -> String {
    let trimmed = body.trim_end();
    format!("{}\n\n{}\n\n{}\n", AUTOGEN_BEGIN, trimmed, AUTOGEN_END)
}

fn display_id(req: &Requirement) -> String {
    req.spec_id
        .clone()
        .or_else(|| req.agreed_id.clone())
        .unwrap_or_else(|| req.id.to_string())
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(60)
        .collect()
}

fn decision_filename(d: &Requirement) -> String {
    let id = display_id(d);
    let slug = slugify(&d.title);
    if slug.is_empty() {
        format!("{}.md", id)
    } else {
        format!("{}-{}.md", id, slug)
    }
}

// ─── Layer renderers ────────────────────────────────────────────────────

fn render_index(store: &RequirementsStore) -> String {
    let project_name = if !store.title.is_empty() {
        &store.title
    } else if !store.name.is_empty() {
        &store.name
    } else {
        "AIDA project"
    };

    let count_principles = count_type(store, &RequirementType::Principle);
    let count_visions = count_type(store, &RequirementType::Vision);
    let count_constraints = count_type(store, &RequirementType::Constraint);
    let count_decisions = count_type(store, &RequirementType::Decision);
    let count_nfrs = count_type(store, &RequirementType::NonFunctional);
    let count_terms = count_type(store, &RequirementType::Term);

    let mut s = String::new();
    s.push_str(&format!("# {} — Living Documentation\n\n", project_name));
    s.push_str(
        "This tree is auto-generated by `aida docs build` from the project's \
         requirements graph. Each section projects a typed slice of the graph; \
         the graph is the source, this is the view.\n\n\
         To re-render after editing requirements: `aida docs build`.\n\n",
    );
    s.push_str("## Layers\n\n");
    s.push_str(&format!(
        "- [Constitution](00-constitution.md) — {} principles\n",
        count_principles
    ));
    s.push_str(&format!(
        "- [Vision](01-vision.md) — {} item(s)\n",
        count_visions
    ));
    s.push_str(&format!(
        "- [Constraints](02-constraints.md) — {} constraint(s)\n",
        count_constraints
    ));
    s.push_str(&format!(
        "- [Decisions](05-decisions/README.md) — {} ADR(s)\n",
        count_decisions
    ));
    s.push_str(&format!(
        "- [Quality requirements](07-quality.md) — {} NFR(s)\n",
        count_nfrs
    ));
    s.push_str(&format!(
        "- [Glossary](10-glossary.md) — {} term(s)\n",
        count_terms
    ));
    s
}

fn render_constitution(store: &RequirementsStore) -> String {
    let principles = filter_type_sorted(store, &RequirementType::Principle);
    let mut s = String::from("# Constitution\n\n");
    s.push_str(
        "Non-negotiable principles that govern how this project is built. \
         Sourced from requirements with `type=principle`. To add a new \
         principle: `aida add --type principle --title \"...\"`.\n\n",
    );
    if principles.is_empty() {
        s.push_str(
            "_No principles defined yet._ Run `aida add --type principle --title \"...\"` to add one.\n",
        );
        return s;
    }
    for p in &principles {
        s.push_str(&format!("## {} — {}\n\n", display_id(p), p.title));
        if !p.description.trim().is_empty() {
            s.push_str(p.description.trim());
            s.push_str("\n\n");
        }
    }
    s
}

fn render_vision(store: &RequirementsStore) -> String {
    let visions = filter_type_sorted(store, &RequirementType::Vision);
    let mut s = String::from("# Vision\n\n");
    s.push_str(
        "Target outcomes — what we're building, for whom, and what \"done\" \
         looks like. Sourced from requirements with `type=vision`.\n\n",
    );
    if visions.is_empty() {
        s.push_str("_No vision items defined yet._\n");
        return s;
    }
    for v in &visions {
        let status = v.effective_status().to_string();
        s.push_str(&format!(
            "## {} — {}  *(status: {})*\n\n",
            display_id(v),
            v.title,
            status
        ));
        if !v.description.trim().is_empty() {
            s.push_str(v.description.trim());
            s.push_str("\n\n");
        }
    }
    s
}

fn render_constraints(store: &RequirementsStore) -> String {
    let constraints = filter_type_sorted(store, &RequirementType::Constraint);
    let mut s = String::from("# Constraints\n\n");
    s.push_str(
        "External, technical, or organizational constraints the project \
         must respect. Sourced from `type=constraint` requirements.\n\n",
    );
    if constraints.is_empty() {
        s.push_str("_No constraints defined yet._\n");
        return s;
    }
    for c in &constraints {
        let status = c.effective_status().to_string();
        s.push_str(&format!(
            "## {} — {}  *(status: {})*\n\n",
            display_id(c),
            c.title,
            status
        ));
        if !c.description.trim().is_empty() {
            s.push_str(c.description.trim());
            s.push_str("\n\n");
        }
    }
    s
}

fn render_decisions_index(decisions: &[&Requirement]) -> String {
    let mut s = String::from("# Architecture Decisions\n\n");
    s.push_str(
        "Append-only log of architecture decisions (ADRs). Each decision \
         lives in its own file; this page indexes them by id and status.\n\n",
    );
    if decisions.is_empty() {
        s.push_str("_No decisions recorded yet._ Run `aida add --type decision --title \"...\"` to add one.\n");
        return s;
    }
    s.push_str("| ID | Title | Status |\n|---|---|---|\n");
    let mut sorted: Vec<&&Requirement> = decisions.iter().collect();
    sorted.sort_by_key(|d| display_id(d));
    for d in sorted {
        let id = display_id(d);
        let status = d.effective_status().to_string();
        let file = decision_filename(d);
        s.push_str(&format!(
            "| [{}]({}) | {} | {} |\n",
            id, file, d.title, status
        ));
    }
    s
}

fn render_decision(d: &Requirement) -> String {
    let id = display_id(d);
    let status = d.effective_status().to_string();
    let mut s = format!("# {} — {}\n\n", id, d.title);
    s.push_str(&format!("**Status:** {}\n\n", status));
    if !d.description.trim().is_empty() {
        s.push_str(d.description.trim());
        s.push_str("\n\n");
    } else {
        s.push_str("_No rationale yet._\n\n");
    }
    if !d.comments.is_empty() {
        s.push_str("## Discussion\n\n");
        for c in &d.comments {
            let ts = c.created_at.format("%Y-%m-%d");
            s.push_str(&format!(
                "**{}** — *{}*\n\n{}\n\n",
                c.author,
                ts,
                c.content.trim()
            ));
        }
    }
    s
}

fn render_quality(store: &RequirementsStore) -> String {
    let nfrs = filter_type_sorted(store, &RequirementType::NonFunctional);
    let mut s = String::from("# Quality Requirements\n\n");
    s.push_str(
        "Performance, reliability, security, and other non-functional \
         requirements. Sourced from `type=non-functional` requirements.\n\n",
    );
    if nfrs.is_empty() {
        s.push_str("_No non-functional requirements defined yet._\n");
        return s;
    }
    for r in &nfrs {
        let status = r.effective_status().to_string();
        s.push_str(&format!(
            "## {} — {}  *(status: {})*\n\n",
            display_id(r),
            r.title,
            status
        ));
        if !r.description.trim().is_empty() {
            s.push_str(r.description.trim());
            s.push_str("\n\n");
        }
    }
    s
}

fn render_glossary(store: &RequirementsStore) -> String {
    let terms = filter_type(store, &RequirementType::Term);
    let mut s = String::from("# Glossary\n\n");
    s.push_str(
        "Domain language — the project's ubiquitous terms. Sourced from \
         `type=term` requirements.\n\n",
    );
    if terms.is_empty() {
        s.push_str(
            "_No terms defined yet._ Run `aida add --type term --title \"...\"` to add one.\n",
        );
        return s;
    }
    let mut sorted = terms;
    sorted.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    for t in sorted {
        s.push_str(&format!("### {}", t.title));
        s.push_str(&format!("  *({})*\n\n", display_id(t)));
        if !t.description.trim().is_empty() {
            s.push_str(t.description.trim());
            s.push_str("\n\n");
        }
    }
    s
}

// ─── Helpers ────────────────────────────────────────────────────────────

fn filter_type<'a>(store: &'a RequirementsStore, t: &RequirementType) -> Vec<&'a Requirement> {
    store
        .requirements
        .iter()
        .filter(|r| &r.req_type == t)
        .collect()
}

/// Filter by type AND sort by spec_id ascending so the projected docs list
/// PRIN-1 before PRIN-2, ADR-1 before ADR-2, etc. Without this, the order
/// is `store.requirements` insertion order, which surprises readers.
/// trace:BUG-20 | ai:claude
fn filter_type_sorted<'a>(
    store: &'a RequirementsStore,
    t: &RequirementType,
) -> Vec<&'a Requirement> {
    let mut v = filter_type(store, t);
    v.sort_by(|a, b| {
        let id = |r: &&Requirement| {
            r.spec_id
                .as_deref()
                .or(r.agreed_id.as_deref())
                .unwrap_or("")
                .to_string()
        };
        id(a).cmp(&id(b))
    });
    v
}

fn count_type(store: &RequirementsStore, t: &RequirementType) -> usize {
    filter_type(store, t).len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::models::Requirement;

    fn req(t: RequirementType, spec_id: &str, title: &str, description: &str) -> Requirement {
        let mut r = Requirement::new(title.to_string(), description.to_string());
        r.req_type = t;
        r.spec_id = Some(spec_id.to_string());
        r
    }

    #[test]
    fn slugify_handles_punctuation_and_unicode() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(
            slugify("ADR: orphan-branch storage"),
            "adr-orphan-branch-storage"
        );
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn decision_filename_combines_id_and_slug() {
        let d = req(
            RequirementType::Decision,
            "ADR-1",
            "Use orphan-branch storage",
            "",
        );
        assert_eq!(decision_filename(&d), "ADR-1-use-orphan-branch-storage.md");
    }

    #[test]
    fn render_constitution_lists_principles() {
        let mut store = RequirementsStore::new();
        store.requirements.push(req(
            RequirementType::Principle,
            "PRIN-1",
            "Graph is canonical",
            "Documentation is the projection.",
        ));
        let body = render_constitution(&store);
        assert!(body.contains("# Constitution"));
        assert!(body.contains("PRIN-1 — Graph is canonical"));
        assert!(body.contains("Documentation is the projection."));
    }

    #[test]
    fn render_constitution_says_empty_when_no_principles() {
        let store = RequirementsStore::new();
        let body = render_constitution(&store);
        assert!(body.contains("No principles defined yet"));
    }

    #[test]
    fn build_writes_planned_files_and_skips_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = RequirementsStore::new();
        store
            .requirements
            .push(req(RequirementType::Principle, "PRIN-1", "P", "body"));

        let r1 = build(&store, dir.path(), false).unwrap();
        assert!(!r1.written.is_empty(), "first build must write files");

        // Second build with the same store: every file should be unchanged.
        let r2 = build(&store, dir.path(), false).unwrap();
        assert!(r2.written.is_empty(), "second build should not re-write");
        assert!(r2.drifted.is_empty(), "second build should not show drift");
        assert!(
            !r2.unchanged.is_empty(),
            "second build should report unchanged files"
        );
    }

    #[test]
    fn build_detects_drift_when_file_modified_externally() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = RequirementsStore::new();
        store
            .requirements
            .push(req(RequirementType::Principle, "PRIN-1", "Original", ""));
        build(&store, dir.path(), false).unwrap();

        // External edit
        std::fs::write(dir.path().join("00-constitution.md"), "# tampered\n").unwrap();

        let r = build(&store, dir.path(), true).unwrap();
        assert!(r.has_drift(), "external edit must surface as drift");
        assert!(r.drifted.iter().any(|p| p.ends_with("00-constitution.md")));
    }

    #[test]
    fn wrap_autogen_includes_markers() {
        let s = wrap_autogen("hello");
        assert!(s.starts_with(AUTOGEN_BEGIN));
        assert!(s.trim_end().ends_with(AUTOGEN_END));
        assert!(s.contains("hello"));
    }
}
