//! Human-centric spec exposition sidecar data model, SHA-256 bounded drift hashing,
//! deterministic offline extraction, advisory Jev auditing, and storage (EPIC-72, TASK-1435, TASK-1436, TASK-1438).
//
// trace:EPIC-72 trace:TASK-1435 trace:TASK-1436 trace:TASK-1438 | ai:antigravity

use aida_core::Requirement;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const EXPOSITION_SCHEMA_VERSION: u32 = 1;

/// Audience persona for the exposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExpositionAudience {
    Operator,
    Executive,
    Implementer,
    Contributor,
}

impl ExpositionAudience {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::Executive => "executive",
            Self::Implementer => "implementer",
            Self::Contributor => "contributor",
        }
    }
}

impl std::fmt::Display for ExpositionAudience {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for ExpositionAudience {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "operator" => Ok(Self::Operator),
            "executive" | "exec" => Ok(Self::Executive),
            "implementer" | "dev" => Ok(Self::Implementer),
            "contributor" => Ok(Self::Contributor),
            other => Err(format!(
                "unknown audience '{other}'; valid: operator, executive, implementer, contributor"
            )),
        }
    }
}

/// Traceable link to an exact acceptance criterion or invariant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CriterionCitation {
    pub criterion_id: String,
    pub plain_summary: String,
}

/// Provenance metadata recording how, when, and from what exact source hash the exposition was derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpositionProvenance {
    pub generator: String,
    pub generated_at: String,
    pub source_hash: String,
    pub closure_spec_ids: Vec<String>,
}

/// Advisory quality audit metadata evaluating readability and constraint preservation (TASK-1438).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpositionAudit {
    pub evaluator: String,
    pub evaluated_at: String,
    pub readability_score: f64,
    pub jargon_saturation_score: f64,
    pub constraint_preservation_score: f64,
    pub deterministic_checks_passed: bool,
    pub needs_revision: bool,
    pub findings: Vec<String>,
}

/// Human review and override history protecting manual edits from automated overwrite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanEdits {
    pub reviewed_by: String,
    pub reviewed_at: String,
    pub notes: String,
    pub override_summary: Option<String>,
    pub human_reviewed: bool,
}

/// Versioned exposition sidecar stored at `.aida-store/expositions/<SPEC-ID>/<audience>.yaml` (TASK-1435).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpositionSidecar {
    pub schema_version: u32,
    pub spec_id: String,
    pub audience: ExpositionAudience,
    pub title: String,
    pub summary: String,
    pub rationale: String,
    pub key_constraints: Vec<String>,
    pub tradeoffs: Vec<String>,
    pub open_questions: Vec<String>,
    pub acceptance_criteria_citations: Vec<CriterionCitation>,
    pub provenance: ExpositionProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit: Option<ExpositionAudit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_edits: Option<HumanEdits>,
}

impl ExpositionSidecar {
    /// Returns whether this exposition has drifted from the current canonical closure hash.
    pub fn is_stale(&self, current_closure_hash: &str) -> bool {
        self.provenance.source_hash != current_closure_hash
    }

    /// Returns whether this exposition is protected by a human review.
    pub fn is_human_reviewed(&self) -> bool {
        self.human_edits.as_ref().is_some_and(|h| h.human_reviewed)
    }

    /// Effective summary: uses human override summary if present, otherwise generated summary.
    pub fn effective_summary(&self) -> &str {
        if let Some(h) = &self.human_edits {
            if let Some(s) = &h.override_summary {
                if !s.trim().is_empty() {
                    return s.trim();
                }
            }
        }
        &self.summary
    }
}

/// Immediate depth-1 neighbor participating in the bounded drift hash (TASK-1435).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Depth1Neighbor {
    pub id: String,
    pub req_type: String,
    pub status: String,
    pub relationship: String,
}

/// Bounded depth-1 closure inputs for deterministic SHA-256 drift detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedClosureInputs {
    pub spec_id: String,
    pub req_type: String,
    pub status: String,
    pub title: String,
    pub description: String,
    pub neighbors: Vec<Depth1Neighbor>,
}

impl BoundedClosureInputs {
    /// Canonical, deterministic serialization for SHA-256 hashing.
    pub fn canonical_text(&self) -> String {
        let mut neighbors = self.neighbors.clone();
        neighbors.sort_by(|a, b| a.id.cmp(&b.id));

        let mut out = String::new();
        out.push_str("spec:");
        out.push_str(&self.spec_id);
        out.push('\n');
        out.push_str("type:");
        out.push_str(&self.req_type);
        out.push('\n');
        out.push_str("status:");
        out.push_str(&self.status);
        out.push('\n');
        out.push_str("title:");
        out.push_str(&self.title);
        out.push('\n');
        out.push_str("description:\n");
        out.push_str(self.description.trim());
        out.push('\n');
        out.push_str("closure:\n");
        for n in &neighbors {
            out.push_str(&n.id);
            out.push('|');
            out.push_str(&n.req_type);
            out.push('|');
            out.push_str(&n.status);
            out.push('|');
            out.push_str(&n.relationship);
            out.push('\n');
        }
        out
    }

    /// Computes deterministic SHA-256 digest over the canonical serialization.
    pub fn compute_sha256(&self) -> String {
        let text = self.canonical_text();
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        format!("sha256:{:x}", hasher.finalize())
    }

    /// Returns the sorted list of participating spec IDs.
    pub fn closure_spec_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.neighbors.iter().map(|n| n.id.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    }
}

/// Assembles bounded depth-1 closure inputs for a spec from the store requirements.
/// Includes strictly immediate parents, blockers, decisions (ADR references), and children.
// trace:TASK-1435 | ai:antigravity
pub fn build_bounded_closure(req: &Requirement, all: &[Requirement]) -> BoundedClosureInputs {
    let mut neighbors = Vec::new();

    for rel in &req.relationships {
        if let Some(target) = all.iter().find(|r| r.id == rel.target_id) {
            let rel_str = rel.rel_type.to_string();
            neighbors.push(Depth1Neighbor {
                id: target.display_id(),
                req_type: format!("{:?}", target.req_type),
                status: target.status.to_string(),
                relationship: rel_str,
            });
        }
    }

    // Also look up inverse incoming blockers or children that point to this spec
    for other in all {
        if other.id == req.id {
            continue;
        }
        for rel in &other.relationships {
            if rel.target_id == req.id {
                let inverse_rel = match rel.rel_type {
                    aida_core::RelationshipType::BlockedBy => "Blocks",
                    aida_core::RelationshipType::Child => "Parent",
                    _ => continue, // only capture inverse blockers and children to stay strictly depth-1
                };
                let id = other.display_id();
                if !neighbors.iter().any(|n| n.id == id) {
                    neighbors.push(Depth1Neighbor {
                        id,
                        req_type: format!("{:?}", other.req_type),
                        status: other.status.to_string(),
                        relationship: inverse_rel.to_string(),
                    });
                }
            }
        }
    }

    BoundedClosureInputs {
        spec_id: req.display_id(),
        req_type: format!("{:?}", req.req_type),
        status: req.status.to_string(),
        title: req.title.clone(),
        description: req.description.clone(),
        neighbors,
    }
}

/// Deterministic offline extractor that creates an exposition from requirement markdown without network/LLM dependencies (TASK-1436).
// trace:TASK-1436 | ai:antigravity
pub fn extract_offline_exposition(
    req: &Requirement,
    audience: ExpositionAudience,
    closure: &BoundedClosureInputs,
) -> ExpositionSidecar {
    let desc = req.description.trim();

    // 1. Extract summary: first non-heading paragraph
    let mut summary = String::new();
    for p in desc.split("\n\n") {
        let p_trimmed = p.trim();
        if !p_trimmed.is_empty() && !p_trimmed.starts_with('#') {
            summary = p_trimmed.to_string();
            break;
        }
    }
    if summary.is_empty() {
        summary = format!("Requirement {} specifies: {}", req.display_id(), req.title);
    }

    // 2. Extract rationale
    let mut rationale = String::new();
    if let Some(pos) = desc.to_lowercase().find("context") {
        let after = &desc[pos..];
        if let Some(start) = after.find('\n') {
            let section = &after[start..];
            let end = section.find("\n#").unwrap_or(section.len());
            rationale = section[..end].trim().to_string();
        }
    }
    if rationale.is_empty() {
        rationale = format!(
            "Provides implementation and governance for '{}'.",
            req.title
        );
    }

    // 3. Extract key constraints and criteria
    let mut key_constraints = Vec::new();
    let mut criteria_citations = Vec::new();
    let mut tradeoffs = Vec::new();
    let mut open_questions = Vec::new();

    let mut ac_counter = 1;
    for line in desc.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let item = trimmed[2..].trim().to_string();
            if item.to_lowercase().contains("tradeoff") || item.to_lowercase().contains("trade-off")
            {
                tradeoffs.push(item);
            } else if item.ends_with('?') || item.to_lowercase().contains("open fork") {
                open_questions.push(item);
            } else {
                key_constraints.push(item.clone());
                criteria_citations.push(CriterionCitation {
                    criterion_id: format!("AC-{}", ac_counter),
                    plain_summary: item,
                });
                ac_counter += 1;
            }
        }
    }

    if key_constraints.is_empty() {
        key_constraints.push("Implement as specified in canonical requirement body.".to_string());
    }

    ExpositionSidecar {
        schema_version: EXPOSITION_SCHEMA_VERSION,
        spec_id: req.display_id(),
        audience,
        title: req.title.clone(),
        summary,
        rationale,
        key_constraints,
        tradeoffs,
        open_questions,
        acceptance_criteria_citations: criteria_citations,
        provenance: ExpositionProvenance {
            generator: "template-extract".to_string(),
            generated_at: Utc::now().to_rfc3339(),
            source_hash: closure.compute_sha256(),
            closure_spec_ids: closure.closure_spec_ids(),
        },
        audit: None,
        human_edits: None,
    }
}

/// Deterministic invariant audit paired with optional advisory Jev System One evaluation (TASK-1438).
// trace:TASK-1438 | ai:antigravity
pub fn audit_exposition(
    exposition: &ExpositionSidecar,
    req: &Requirement,
    evaluator: Option<&dyn crate::evaluator::EvaluatorEngine>,
) -> ExpositionAudit {
    let mut findings = Vec::new();
    let mut deterministic_passed = true;

    // 1. Deterministic Invariant Checks
    // Check 1: Summary must not be empty
    if exposition.summary.trim().is_empty() {
        deterministic_passed = false;
        findings.push("Summary is empty".to_string());
    }

    // Check 2: Key constraints must be present
    if exposition.key_constraints.is_empty() {
        deterministic_passed = false;
        findings.push("No key constraints or acceptance criteria cited".to_string());
    }

    // Check 3: Acceptance criteria coverage
    let desc_lower = req.description.to_lowercase();
    let mentions_fail_closed =
        desc_lower.contains("fail closed") || desc_lower.contains("fail-closed");
    let mentions_ci = desc_lower.contains("ci") || desc_lower.contains("green ci");

    let exposition_body = format!(
        "{} {} {}",
        exposition.summary,
        exposition.rationale,
        exposition.key_constraints.join(" ")
    )
    .to_lowercase();

    if mentions_fail_closed
        && !exposition_body.contains("fail closed")
        && !exposition_body.contains("fail-closed")
    {
        deterministic_passed = false;
        findings.push(
            "Dropped critical invariant: canonical spec specifies fail-closed behavior".to_string(),
        );
    }
    if mentions_ci && !exposition_body.contains("ci") {
        findings.push(
            "Advisory: canonical spec emphasizes CI invariants not mentioned in exposition"
                .to_string(),
        );
    }

    // Check 4: Acronym and jargon density
    let jargon_terms = [
        "ast",
        "serde",
        "fnv1a",
        "uuid",
        "sha256",
        "reconstitute",
        "ts-rs",
    ];
    let mut jargon_count = 0usize;
    for term in &jargon_terms {
        if exposition_body.contains(term) {
            jargon_count += 1;
        }
    }
    let jargon_saturation = (jargon_count as f64 / 10.0).min(1.0);

    // 2. Advisory Evaluation
    let mut readability = 0.88;
    let mut constraint_preservation = if deterministic_passed { 0.95 } else { 0.60 };
    let mut evaluator_name;

    if let Some(engine) = evaluator {
        evaluator_name = "jev-system-one".to_string();
        // Run advisory noul check on constraint preservation
        let context = format!(
            "Canonical Spec:\n{}\n\nExposition:\n{}",
            req.description, exposition.summary
        );
        let instruction = "Does the exposition preserve the core safety constraints and acceptance requirements of the canonical spec?";
        match engine.evaluate_noul_sync(&context, instruction) {
            Ok(resp) => {
                constraint_preservation = resp.noul;
                if resp.noul < 0.75 {
                    findings.push(format!(
                        "Jev advisory: Low constraint preservation score ({:.2})",
                        resp.noul
                    ));
                }
            }
            Err(err) => {
                // Fail closed: a failed remote audit must not read as a pass.
                // Report only the error class (never the request or key) and
                // fall back to the offline mechanical audit.
                // trace:TASK-1470 | ai:claude
                evaluator_name = "mechanical-audit".to_string();
                findings.push(format!(
                    "Jev advisory unavailable ({}); offline mechanical audit used instead",
                    evaluator_error_class(&err)
                ));
                apply_mechanical_readability(jargon_saturation, &mut readability, &mut findings);
            }
        }
    } else {
        evaluator_name = "mechanical-audit".to_string();
        apply_mechanical_readability(jargon_saturation, &mut readability, &mut findings);
    }

    let needs_revision =
        !deterministic_passed || readability < 0.70 || constraint_preservation < 0.80;

    ExpositionAudit {
        evaluator: evaluator_name,
        evaluated_at: Utc::now().to_rfc3339(),
        readability_score: readability,
        jargon_saturation_score: jargon_saturation,
        constraint_preservation_score: constraint_preservation,
        deterministic_checks_passed: deterministic_passed,
        needs_revision,
        findings,
    }
}

/// Environment variable holding the TypeSafe AI (Jev) API key for the optional
/// advisory audit in `aida explain`. Setting it opts in to network egress to
/// `api.typesafe.ai` (or `AIDA_JEV_ENDPOINT`); leaving it unset keeps
/// `aida explain` fully offline.
// trace:TASK-1470 | ai:claude
pub const JEV_API_KEY_ENV: &str = "AIDA_JEV_API_KEY";

/// Deadline for the advisory remote audit so it can never stall `aida explain`.
pub const JEV_ADVISORY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Resolve the advisory Jev API key from `AIDA_JEV_API_KEY` only. The legacy
/// unprefixed `JEV_API_KEY` is deliberately NOT consulted, and neither is
/// `~/.env`: egress must be an explicit, documented opt-in. `lookup` is the
/// environment accessor (injected so tests never mutate process env).
// trace:TASK-1470 | ai:claude
pub fn resolve_jev_api_key(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    lookup(JEV_API_KEY_ENV)
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

/// Operator-facing notice printed (to stderr) when the advisory key is unset.
// trace:TASK-1470 | ai:claude
pub fn jev_key_unset_notice() -> String {
    format!(
        "Note: {JEV_API_KEY_ENV} is not set, so the remote advisory audit was skipped. \
         Used the offline mechanical audit; nothing was sent over the network."
    )
}

/// A short, secret-free class name for an evaluator failure.
fn evaluator_error_class(err: &crate::evaluator::EvaluatorError) -> String {
    use crate::evaluator::EvaluatorError as E;
    match err {
        E::MissingApiKey(_) => "missing API key".to_string(),
        E::Network(_) => "network error".to_string(),
        E::ApiError { status, .. } => format!("HTTP {status}"),
        E::ParseError(_) => "unreadable response".to_string(),
        E::Timeout(_) => "timed out".to_string(),
        E::Other(_) => "evaluator error".to_string(),
    }
}

fn apply_mechanical_readability(
    jargon_saturation: f64,
    readability: &mut f64,
    findings: &mut Vec<String>,
) {
    if jargon_saturation > 0.40 {
        *readability = (1.0 - jargon_saturation).max(0.40);
        findings.push(format!(
            "Mechanical audit: High technical jargon density ({:.0}%)",
            jargon_saturation * 100.0
        ));
    }
}

/// Resolves the filesystem path for an exposition sidecar file in `.aida-store` (TASK-1435).
pub fn exposition_path(
    project_root: &Path,
    spec_id: &str,
    audience: ExpositionAudience,
) -> PathBuf {
    project_root
        .join(".aida-store")
        .join("expositions")
        .join(spec_id)
        .join(format!("{}.yaml", audience.as_str()))
}

/// Reads an exposition sidecar from disk.
pub fn load_exposition(
    project_root: &Path,
    spec_id: &str,
    audience: ExpositionAudience,
) -> Option<ExpositionSidecar> {
    let path = exposition_path(project_root, spec_id, audience);
    if path.exists() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(sidecar) = serde_yaml::from_str::<ExpositionSidecar>(&content) {
                return Some(sidecar);
            }
        }
    }
    None
}

/// Writes an exposition sidecar to disk, ensuring directory existence.
pub fn save_exposition(
    project_root: &Path,
    sidecar: &ExpositionSidecar,
) -> Result<PathBuf, std::io::Error> {
    let path = exposition_path(project_root, &sidecar.spec_id, sidecar.audience);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let yaml = serde_yaml::to_string(sidecar).map_err(std::io::Error::other)?;
    std::fs::write(&path, yaml)?;
    Ok(path)
}

/// Handles the `aida explain` CLI command (TASK-1436).
// trace:TASK-1436 | ai:antigravity
pub fn handle_explain_command(
    spec_arg: &str,
    audience_arg: &str,
    refresh: bool,
    force: bool,
    json: bool,
) -> anyhow::Result<()> {
    let audience = audience_arg
        .parse::<ExpositionAudience>()
        .map_err(|e| anyhow::anyhow!(e))?;

    let project_root =
        crate::find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());

    let store = crate::load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached",
            project_root.display()
        )
    })?;

    let want = spec_arg.trim();
    let req = store.requirements.iter().find(|r| {
        r.display_id().eq_ignore_ascii_case(want)
            || r.id.to_string().eq_ignore_ascii_case(want)
            || r.spec_id
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(want))
            || r.agreed_id
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(want))
    });

    let Some(req) = req else {
        anyhow::bail!(
            "no requirement found matching `{spec_arg}` — check the ID with `aida list`."
        );
    };

    let spec_id = req.display_id();
    let closure = build_bounded_closure(req, &store.requirements);
    let current_closure_hash = closure.compute_sha256();

    let existing = load_exposition(&project_root, &spec_id, audience);

    let sidecar = match existing {
        Some(sidecar) if !refresh && !force => {
            // Re-use existing sidecar, but check drift
            sidecar
        }
        Some(sidecar) if sidecar.is_human_reviewed() && !force => {
            eprintln!(
                "Note: exposition for {} ({}) has human_reviewed: true; skipping automated overwrite (use --force to overwrite).",
                spec_id,
                audience
            );
            sidecar
        }
        _ => {
            // Generate or regenerate
            let mut sidecar = extract_offline_exposition(req, audience, &closure);

            // Run advisory audit (TASK-1438). The remote Jev audit is network
            // egress to api.typesafe.ai and runs ONLY when AIDA_JEV_API_KEY is
            // set; otherwise it fails closed to the offline mechanical audit.
            // trace:TASK-1470 | ai:claude
            let evaluator_opt: Option<Box<dyn crate::evaluator::EvaluatorEngine>> =
                match resolve_jev_api_key(|name| std::env::var(name).ok()) {
                    Some(key) => Some(Box::new(
                        crate::evaluator::JevEvaluator::new(key).with_timeout(JEV_ADVISORY_TIMEOUT),
                    )),
                    None => {
                        eprintln!("{}", jev_key_unset_notice());
                        None
                    }
                };

            let audit = audit_exposition(&sidecar, req, evaluator_opt.as_deref());
            sidecar.audit = Some(audit);

            save_exposition(&project_root, &sidecar)
                .map_err(|e| anyhow::anyhow!("failed to save exposition sidecar: {e}"))?;

            sidecar
        }
    };

    let is_stale = sidecar.is_stale(&current_closure_hash);

    if json {
        let payload = serde_json::json!({
            "spec_id": spec_id,
            "audience": audience.as_str(),
            "stale": is_stale,
            "current_closure_hash": current_closure_hash,
            "human_reviewed": sidecar.is_human_reviewed(),
            "sidecar": sidecar,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        render_human_exposition(&sidecar, is_stale, &current_closure_hash);
    }

    Ok(())
}

fn render_human_exposition(sidecar: &ExpositionSidecar, is_stale: bool, current_hash: &str) {
    use colored::Colorize;

    println!();
    println!(
        "{} {} ({})",
        "EXPOSITION:".bold().cyan(),
        sidecar.spec_id.bold(),
        sidecar.audience.as_str().yellow()
    );
    println!("Title: {}", sidecar.title);

    let status_str = if is_stale {
        format!(
            "STALE (source inputs changed; current: {}, recorded: {})",
            &current_hash[..16.min(current_hash.len())],
            &sidecar.provenance.source_hash[..16.min(sidecar.provenance.source_hash.len())]
        )
        .red()
        .bold()
    } else {
        "FRESH (matches canonical graph closure)".green().bold()
    };
    println!("Freshness:      {}", status_str);

    if let Some(h) = &sidecar.human_edits {
        if h.human_reviewed {
            println!(
                "Human Review:   {} by {} at {}",
                "PROTECTED".yellow().bold(),
                h.reviewed_by,
                h.reviewed_at
            );
            if !h.notes.is_empty() {
                println!("Notes:          {}", h.notes);
            }
        }
    }

    println!(
        "Generator:      {} ({})",
        sidecar.provenance.generator, sidecar.provenance.generated_at
    );
    println!();
    println!("{}", "Summary:".bold());
    println!("  {}", sidecar.effective_summary());
    println!();
    println!("{}", "Rationale:".bold());
    println!("  {}", sidecar.rationale);
    println!();

    if !sidecar.key_constraints.is_empty() {
        println!("{}", "Key Constraints:".bold());
        for c in &sidecar.key_constraints {
            println!("  • {c}");
        }
        println!();
    }

    if !sidecar.tradeoffs.is_empty() {
        println!("{}", "Trade-offs:".bold());
        for t in &sidecar.tradeoffs {
            println!("  • {t}");
        }
        println!();
    }

    if !sidecar.open_questions.is_empty() {
        println!("{}", "Open Questions:".bold());
        for q in &sidecar.open_questions {
            println!("  ? {q}");
        }
        println!();
    }

    if let Some(audit) = &sidecar.audit {
        println!("{}", "Quality Audit:".bold().magenta());
        println!("  Evaluator:               {}", audit.evaluator);
        println!(
            "  Readability:             {:.1}%",
            audit.readability_score * 100.0
        );
        println!(
            "  Jargon Saturation:       {:.1}%",
            audit.jargon_saturation_score * 100.0
        );
        println!(
            "  Constraint Preservation: {:.1}%",
            audit.constraint_preservation_score * 100.0
        );
        let verdict = if audit.needs_revision {
            "NEEDS REVISION".red().bold()
        } else {
            "PASSED".green().bold()
        };
        println!("  Status:                  {}", verdict);
        if !audit.findings.is_empty() {
            println!("  Findings:");
            for f in &audit.findings {
                println!("    - {f}");
            }
        }
        println!();
    }
}
