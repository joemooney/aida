//! `aida spec dryrun <SPEC> [--json] [--ai]` — an implementer-readiness
//! pre-check run BEFORE work is queued.
//!
//! EPIC-48 L1 spec-quality gate. The verb answers "is this spec ready for an
//! implementer to pick up, or will it generate questions and punts?" *without*
//! implementing anything. Two layers:
//!
//! 1. A DETERMINISTIC pre-check (this module) — a pure, fully unit-tested
//!    function over a spec's own fields. It computes a 0-100 readiness score
//!    from a fixed set of weighted dimensions and reports each dimension's
//!    pass/fail plus a one-line reason. This is the core: no I/O, no LLM, no
//!    clock — same spec in, same score out, so it is cheap to run on every
//!    spec and trivially testable.
//! 2. An OPTIONAL AI pass (gated behind `--ai`, lives in `main.rs`, mirrors how
//!    `aida intent` fences its `claude -p` spawn) that lists the questions an
//!    implementer would need answered, the assumptions they'd make, and the
//!    ambiguities / missing acceptance — again WITHOUT implementing. The
//!    deterministic pre-check ALWAYS runs; `--ai` only appends the report.
//!
//! The integration boundary (store load + the headless spawn) lives in
//! `main.rs`, mirroring how `intent.rs` pairs with `handle_intent`. This module
//! is deliberately dependency-light so the scorer stays pure and testable.
//!
//! trace:STORY-656 | ai:claude

use aida_core::{Requirement, RequirementType};
use serde::{Deserialize, Serialize};

/// A scorer view of a spec: just the fields the deterministic pre-check reads.
///
/// We extract this instead of scoring [`Requirement`] directly so the pure core
/// has a tiny, fully-constructable input — unit tests build a `SpecSnapshot`
/// literal without standing up a whole `Requirement`/store. `from_requirement`
/// is the (also pure) adapter used by the live path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecSnapshot {
    /// The spec's description body. The scorer trims where it matters.
    pub description: String,
    /// Lowercased requirement type token (`"story"`, `"folder"`, `"epic"`, …).
    pub req_type: String,
    /// Whether a priority is set. Priority is non-optional on `Requirement`, so
    /// the live adapter always reports `true`; the field exists so tests can
    /// exercise the "priority missing" dimension and so a future optional
    /// priority does not silently pass.
    pub has_priority: bool,
    /// Whether the spec has a linked parent (an outgoing `Child` relationship,
    /// i.e. "I am a child of <target>").
    pub has_parent: bool,
    /// Whether the spec is a top-level container that legitimately has no
    /// parent (`epic` / `vision`). Such specs PASS the parent dimension.
    pub is_top_level: bool,
}

impl SpecSnapshot {
    /// Pure adapter: project a live [`Requirement`] onto the scorer's input.
    ///
    /// `has_parent` is true when the spec carries an outgoing `Child`
    /// relationship (the stored convention per `RelationshipType::Child`:
    /// "I am a child of this target"). `is_top_level` is true for `Epic` /
    /// `Vision`, the container types that legitimately sit at the root of the
    /// graph. The type token is the lowercased `Display` form with spaces
    /// stripped (`"non-functional"` etc.), which matches the tokens the scorer
    /// compares against.
    pub fn from_requirement(req: &Requirement) -> Self {
        let has_parent = req
            .relationships
            .iter()
            .any(|r| matches!(r.rel_type, aida_core::RelationshipType::Child));
        SpecSnapshot {
            description: req.description.clone(),
            req_type: req
                .req_type
                .to_string()
                .to_ascii_lowercase()
                .replace(' ', ""),
            has_priority: true,
            has_parent,
            is_top_level: matches!(
                req.req_type,
                RequirementType::Epic | RequirementType::Vision
            ),
        }
    }
}

/// One scored dimension of readiness: did it pass, and the one-line why.
///
/// Serialize-only: the `name` is a `&'static str` (a compile-time constant, never
/// allocated per score), which cannot be the target of a `Deserialize`. These
/// verdicts are only ever *produced* here and rendered to JSON/human output; no
/// code path parses them back, so dropping `Deserialize` is non-behavioral.
/// trace:TASK-849 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dimension {
    /// Stable machine name (`"description"`, `"acceptance"`, …) — usable as a
    /// JSON key / filter token; never localized. A compile-time constant: these
    /// names are fixed in source, so `&'static str` avoids a per-score
    /// allocation. trace:TASK-849 | ai:claude
    pub name: &'static str,
    /// Whether this dimension is satisfied.
    pub pass: bool,
    /// A single human-readable line explaining the verdict.
    pub reason: String,
    /// The points this dimension contributes to the 0-100 score when it passes.
    pub weight: u32,
}

/// The full deterministic readiness verdict for one spec.
///
/// Serialize-only for the same reason as [`Dimension`]: it owns `Dimension`s
/// whose `&'static str` names rule out `Deserialize`, and nothing parses a
/// verdict back. trace:TASK-849 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Readiness {
    /// 0-100 readiness score: sum of passing dimensions' weights (weights sum
    /// to exactly 100).
    pub score: u32,
    /// Every scored dimension, in a stable order.
    pub dimensions: Vec<Dimension>,
}

impl Readiness {
    /// The dimensions that failed — the actionable "fix these before queuing"
    /// list.
    pub fn failing(&self) -> impl Iterator<Item = &Dimension> {
        self.dimensions.iter().filter(|d| !d.pass)
    }
}

/// Minimum description length (chars, trimmed) below which a description is a
/// trivial stub rather than a real spec body. One short sentence clears this.
const DESCRIPTION_FLOOR: usize = 40;

/// Description length (chars, trimmed) below which the not-too-vague dimension
/// treats the body as too thin to carry concrete detail, independent of the
/// placeholder check.
const VAGUE_BODY_FLOOR: usize = 60;

/// Dimension weights. They sum to exactly 100 so a fully-ready spec scores 100.
/// Description + acceptance carry the most weight because they are what an
/// implementer actually reads to begin work; type/priority/parent are cheaper
/// metadata gates; not-too-vague is a heuristic backstop.
const W_DESCRIPTION: u32 = 25;
const W_ACCEPTANCE: u32 = 25;
const W_TYPE: u32 = 15;
const W_PRIORITY: u32 = 10;
const W_PARENT: u32 = 10;
const W_NOT_VAGUE: u32 = 15;

/// Compute the deterministic readiness verdict for a spec snapshot.
///
/// Pure: no I/O, no clock, no randomness. The score is the sum of the weights
/// of the passing dimensions (weights total 100). Dimension order is stable so
/// JSON output and the human view are deterministic.
pub fn score(spec: &SpecSnapshot) -> Readiness {
    let desc = spec.description.trim();
    let mut dimensions = Vec::with_capacity(6);

    // 1. Non-empty description beyond a trivial floor.
    let desc_len = desc.chars().count();
    let desc_pass = desc_len >= DESCRIPTION_FLOOR;
    dimensions.push(Dimension {
        name: "description",
        pass: desc_pass,
        reason: if desc.is_empty() {
            "description is empty".to_string()
        } else if desc_pass {
            format!("description has {desc_len} chars (>= {DESCRIPTION_FLOOR} floor)")
        } else {
            format!("description is only {desc_len} chars (< {DESCRIPTION_FLOOR} floor) — a stub")
        },
        weight: W_DESCRIPTION,
    });

    // 2. Description contains an acceptance section / criteria.
    let accept_pass = has_acceptance(desc);
    dimensions.push(Dimension {
        name: "acceptance",
        pass: accept_pass,
        reason: if accept_pass {
            "found an acceptance section / criteria".to_string()
        } else {
            "no acceptance section found (expected `## Acceptance`, `Acceptance:`, or criteria \
             bullets)"
                .to_string()
        },
        weight: W_ACCEPTANCE,
    });

    // 3. Type set + implementable (not folder/meta — those are organizational,
    //    not work an implementer picks up).
    let type_pass = is_implementable_type(&spec.req_type);
    dimensions.push(Dimension {
        name: "type",
        pass: type_pass,
        reason: if type_pass {
            format!("type `{}` is implementable", spec.req_type)
        } else {
            format!(
                "type `{}` is organizational (folder/meta), not implementable work",
                spec.req_type
            )
        },
        weight: W_TYPE,
    });

    // 4. Priority set.
    dimensions.push(Dimension {
        name: "priority",
        pass: spec.has_priority,
        reason: if spec.has_priority {
            "priority is set".to_string()
        } else {
            "priority is not set".to_string()
        },
        weight: W_PRIORITY,
    });

    // 5. Has a linked parent OR is a top-level epic/vision.
    let parent_pass = spec.has_parent || spec.is_top_level;
    dimensions.push(Dimension {
        name: "parent",
        pass: parent_pass,
        reason: if spec.has_parent {
            "linked to a parent spec".to_string()
        } else if spec.is_top_level {
            "top-level epic/vision — no parent required".to_string()
        } else {
            "no parent link and not a top-level epic/vision (orphaned work)".to_string()
        },
        weight: W_PARENT,
    });

    // 6. Not-too-vague heuristic: mostly placeholder text, or too thin to
    //    carry concrete nouns/verbs.
    let (vague_pass, vague_reason) = not_too_vague(desc);
    dimensions.push(Dimension {
        name: "not_vague",
        pass: vague_pass,
        reason: vague_reason,
        weight: W_NOT_VAGUE,
    });

    let score = dimensions.iter().filter(|d| d.pass).map(|d| d.weight).sum();
    Readiness { score, dimensions }
}

/// Whether the description names an acceptance section or criteria.
///
/// Accepts the common shapes a spec uses for "definition of done":
/// `## Acceptance` (any heading level), an `Acceptance:` / `Acceptance criteria`
/// label, or a `Definition of Done` / `DoD` marker. Case-insensitive.
fn has_acceptance(desc: &str) -> bool {
    let lower = desc.to_ascii_lowercase();
    lower.contains("acceptance")
        || lower.contains("definition of done")
        // A standalone `dod` token (avoid matching inside other words).
        || lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w == "dod")
}

/// Whether a (lowercased) type token is implementable work as opposed to an
/// organizational container. `folder` and `meta` are stateless organizational
/// types; everything else is pickable.
fn is_implementable_type(req_type: &str) -> bool {
    !matches!(req_type, "folder" | "meta")
}

/// The not-too-vague heuristic. Returns (pass, reason).
///
/// A description fails when it is dominated by placeholder/TODO text, or when it
/// is too thin to carry the concrete nouns and verbs an implementer needs. The
/// check is deliberately conservative — it is a backstop, not a style linter —
/// so a real, modest description passes.
fn not_too_vague(desc: &str) -> (bool, String) {
    let trimmed = desc.trim();
    if trimmed.is_empty() {
        return (false, "description is empty".to_string());
    }
    let lower = trimmed.to_ascii_lowercase();

    // Placeholder dominance: count tokens that are pure placeholder markers.
    let placeholder_markers = [
        "todo",
        "tbd",
        "tba",
        "fixme",
        "xxx",
        "placeholder",
        "wip",
        "n/a",
    ];
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '/')
        .filter(|w| !w.is_empty())
        .collect();
    let total = words.len().max(1);
    let placeholder_hits = words
        .iter()
        .filter(|w| placeholder_markers.contains(&w.trim_matches('/')))
        .count();
    // If the body is short AND a placeholder marker is a meaningful fraction of
    // it, treat it as a placeholder stub.
    if placeholder_hits > 0 && (total <= 8 || placeholder_hits * 4 >= total) {
        return (
            false,
            format!("description is mostly placeholder text ({placeholder_hits}/{total} tokens)"),
        );
    }

    // Too thin to carry concrete detail.
    if trimmed.chars().count() < VAGUE_BODY_FLOOR {
        return (
            false,
            format!(
                "description is too thin ({} chars < {VAGUE_BODY_FLOOR}) to carry concrete detail",
                trimmed.chars().count()
            ),
        );
    }

    // Needs at least a handful of distinct words — a wall of one repeated word
    // is not concrete.
    let distinct: std::collections::HashSet<&&str> = words.iter().collect();
    if distinct.len() < 6 {
        return (
            false,
            format!(
                "description has only {} distinct words — not concrete enough",
                distinct.len()
            ),
        );
    }

    (
        true,
        "description reads as concrete (no placeholder dominance, enough distinct content)"
            .to_string(),
    )
}

/// The optional AI report layered on top of the deterministic verdict. Produced
/// by the headless `claude -p` pass in `main.rs`; this is the parsed shape and
/// the `--json` representation. Never produced in tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiReport {
    /// Questions an implementer would need answered before starting.
    pub questions: Vec<String>,
    /// Assumptions an implementer would have to make to proceed.
    pub assumptions: Vec<String>,
    /// Ambiguities / missing acceptance the AI flagged.
    pub ambiguities: Vec<String>,
    /// The model id that produced the report.
    pub model: String,
}

/// Parse the AI sidecar JSON the headless `/aida-dryrun` pass writes.
///
/// Pure + unit-tested so the parse contract is locked without spawning a model.
/// Tolerates missing arrays (treated as empty) so a partial report still folds
/// in.
pub fn parse_ai_sidecar(raw: &str) -> anyhow::Result<AiReport> {
    #[derive(Deserialize)]
    struct Sidecar {
        #[serde(default)]
        questions: Vec<String>,
        #[serde(default)]
        assumptions: Vec<String>,
        #[serde(default)]
        ambiguities: Vec<String>,
        #[serde(default)]
        model: String,
    }
    let s: Sidecar = serde_json::from_str(raw.trim())
        .map_err(|e| anyhow::anyhow!("dryrun AI sidecar was not valid JSON: {e}"))?;
    Ok(AiReport {
        questions: s.questions,
        assumptions: s.assumptions,
        ambiguities: s.ambiguities,
        model: s.model,
    })
}

/// The full `--json` payload for `aida spec dryrun`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DryrunJson<'a> {
    pub spec: &'a str,
    pub readiness_score: u32,
    pub dimensions: &'a [Dimension],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_report: Option<&'a AiReport>,
}

/// Assemble the `--json` payload from a readiness verdict + optional AI report.
pub fn dryrun_json<'a>(
    spec: &'a str,
    readiness: &'a Readiness,
    ai: Option<&'a AiReport>,
) -> DryrunJson<'a> {
    DryrunJson {
        spec,
        readiness_score: readiness.score,
        dimensions: &readiness.dimensions,
        ai_report: ai,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fully-ready spec: rich description with an acceptance section,
    /// implementable type, priority set, a parent, concrete content.
    fn ready_snapshot() -> SpecSnapshot {
        SpecSnapshot {
            description: "Add a readiness pre-check command that scores a spec across several \
                          dimensions before it is queued for implementation.\n\n\
                          ## Acceptance\n\
                          - Deterministic scorer returns a 0-100 score\n\
                          - Each dimension reports pass/fail with a reason"
                .to_string(),
            req_type: "story".to_string(),
            has_priority: true,
            has_parent: true,
            is_top_level: false,
        }
    }

    #[test]
    fn fully_specified_scores_100() {
        let r = score(&ready_snapshot());
        assert_eq!(r.score, 100, "fully-ready spec should score 100: {r:?}");
        assert!(r.dimensions.iter().all(|d| d.pass));
        assert_eq!(r.failing().count(), 0);
    }

    #[test]
    fn weights_sum_to_100() {
        let r = score(&ready_snapshot());
        let total: u32 = r.dimensions.iter().map(|d| d.weight).sum();
        assert_eq!(total, 100, "dimension weights must total 100");
    }

    #[test]
    fn empty_description_fails_description_and_vague() {
        let mut s = ready_snapshot();
        s.description = String::new();
        let r = score(&s);
        assert!(failed(&r, "description"));
        assert!(failed(&r, "not_vague"));
        // No acceptance either, since the body is gone.
        assert!(failed(&r, "acceptance"));
        assert!(r.score < 100);
    }

    #[test]
    fn stub_description_fails_floor() {
        let mut s = ready_snapshot();
        s.description = "Fix the thing.".to_string(); // < 40 chars
        let r = score(&s);
        assert!(failed(&r, "description"), "short stub should fail floor");
        // Missing acceptance too.
        assert!(failed(&r, "acceptance"));
    }

    #[test]
    fn missing_acceptance_fails_only_acceptance() {
        let mut s = ready_snapshot();
        s.description = "Implement a thorough readiness scorer over the spec's own fields, \
                         covering description quality, type, priority and parent linkage."
            .to_string();
        let r = score(&s);
        assert!(failed(&r, "acceptance"), "no acceptance section present");
        assert!(passed(&r, "description"));
        assert!(passed(&r, "not_vague"));
        // Exactly the acceptance weight is missing.
        assert_eq!(r.score, 100 - W_ACCEPTANCE);
    }

    #[test]
    fn acceptance_variants_all_detected() {
        for body in [
            "Long enough body text here.\n## Acceptance\n- a\n- b",
            "Long enough body text here.\nAcceptance: it works as described in detail.",
            "Long enough body text here.\nAcceptance criteria: the scorer returns a number.",
            "Long enough body text here.\nDefinition of Done: the command ships.",
            "Long enough body text here.\nDoD: tests pass and it builds clean.",
        ] {
            assert!(
                has_acceptance(body),
                "should detect acceptance in: {body:?}"
            );
        }
        assert!(!has_acceptance(
            "A description with no readiness criteria at all, just plain prose about the work."
        ));
    }

    #[test]
    fn missing_priority_fails_priority() {
        let mut s = ready_snapshot();
        s.has_priority = false;
        let r = score(&s);
        assert!(failed(&r, "priority"));
        assert_eq!(r.score, 100 - W_PRIORITY);
    }

    #[test]
    fn orphan_without_parent_fails_parent() {
        let mut s = ready_snapshot();
        s.has_parent = false;
        s.is_top_level = false;
        let r = score(&s);
        assert!(failed(&r, "parent"), "orphaned non-top-level should fail");
        assert_eq!(r.score, 100 - W_PARENT);
    }

    #[test]
    fn top_level_epic_passes_parent_without_link() {
        let mut s = ready_snapshot();
        s.has_parent = false;
        s.is_top_level = true;
        s.req_type = "epic".to_string();
        let r = score(&s);
        assert!(passed(&r, "parent"), "top-level epic needs no parent");
        assert_eq!(r.score, 100);
    }

    #[test]
    fn folder_type_fails_type() {
        let mut s = ready_snapshot();
        s.req_type = "folder".to_string();
        let r = score(&s);
        assert!(failed(&r, "type"), "folder is not implementable");
        assert_eq!(r.score, 100 - W_TYPE);
    }

    #[test]
    fn meta_type_fails_type() {
        let mut s = ready_snapshot();
        s.req_type = "meta".to_string();
        assert!(failed(&score(&s), "type"));
    }

    #[test]
    fn placeholder_description_fails_vague() {
        let mut s = ready_snapshot();
        // Keep an acceptance marker so we isolate the vague dimension, but make
        // the body placeholder-dominated and short.
        s.description = "TODO TBD acceptance FIXME".to_string();
        let r = score(&s);
        assert!(failed(&r, "not_vague"), "placeholder-heavy body is vague");
    }

    #[test]
    fn thin_body_fails_vague() {
        let mut s = ready_snapshot();
        // Has an acceptance keyword and clears the 40-char floor, but under the
        // 60-char vague floor.
        s.description = "Add it. Acceptance: done when merged ok now.".to_string();
        let r = score(&s);
        assert!(passed(&r, "description"), "clears the 40-char floor");
        assert!(failed(&r, "not_vague"), "under the 60-char vague floor");
    }

    #[test]
    fn worst_case_scores_zero() {
        let s = SpecSnapshot {
            description: String::new(),
            req_type: "folder".to_string(),
            has_priority: false,
            has_parent: false,
            is_top_level: false,
        };
        let r = score(&s);
        assert_eq!(r.score, 0, "everything failing should score 0: {r:?}");
        assert_eq!(r.failing().count(), r.dimensions.len());
    }

    #[test]
    fn score_is_deterministic() {
        let s = ready_snapshot();
        assert_eq!(score(&s), score(&s), "scorer must be pure/deterministic");
    }

    #[test]
    fn every_dimension_has_a_reason() {
        let r = score(&ready_snapshot());
        assert_eq!(r.dimensions.len(), 6, "expected six dimensions");
        for d in &r.dimensions {
            assert!(!d.reason.trim().is_empty(), "{} has no reason", d.name);
        }
    }

    #[test]
    fn ai_sidecar_parses_full() {
        let raw = r#"{
            "questions": ["Which backend?", "Sync or async?"],
            "assumptions": ["Assume Postgres off by default"],
            "ambiguities": ["No acceptance for the error path"],
            "model": "claude-x"
        }"#;
        let r = parse_ai_sidecar(raw).expect("parses");
        assert_eq!(r.questions.len(), 2);
        assert_eq!(r.assumptions.len(), 1);
        assert_eq!(r.ambiguities.len(), 1);
        assert_eq!(r.model, "claude-x");
    }

    #[test]
    fn ai_sidecar_tolerates_missing_arrays() {
        let r = parse_ai_sidecar(r#"{"model":"m"}"#).expect("parses");
        assert!(r.questions.is_empty());
        assert!(r.assumptions.is_empty());
        assert!(r.ambiguities.is_empty());
        assert_eq!(r.model, "m");
    }

    #[test]
    fn ai_sidecar_rejects_garbage() {
        assert!(parse_ai_sidecar("not json").is_err());
    }

    #[test]
    fn json_payload_shape() {
        let r = score(&ready_snapshot());
        let payload = dryrun_json("STORY-656", &r, None);
        let v = serde_json::to_value(&payload).unwrap();
        assert_eq!(v["spec"], "STORY-656");
        assert_eq!(v["readiness_score"], 100);
        assert!(v["dimensions"].is_array());
        assert_eq!(v["dimensions"].as_array().unwrap().len(), 6);
        // ai_report omitted when None.
        assert!(v.get("ai_report").is_none());
    }

    #[test]
    fn json_payload_includes_ai_when_present() {
        let r = score(&ready_snapshot());
        let ai = AiReport {
            questions: vec!["q?".to_string()],
            assumptions: vec![],
            ambiguities: vec![],
            model: "m".to_string(),
        };
        let payload = dryrun_json("STORY-656", &r, Some(&ai));
        let v = serde_json::to_value(&payload).unwrap();
        assert_eq!(v["ai_report"]["questions"][0], "q?");
    }

    // ── helpers ──────────────────────────────────────────────────────
    fn dim<'a>(r: &'a Readiness, name: &str) -> &'a Dimension {
        r.dimensions
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("no dimension {name}"))
    }
    fn passed(r: &Readiness, name: &str) -> bool {
        dim(r, name).pass
    }
    fn failed(r: &Readiness, name: &str) -> bool {
        !dim(r, name).pass
    }
}
