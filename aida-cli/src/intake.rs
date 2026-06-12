//! STORY-560: the headless advisor INTAKE pass.
//!
//! `aida intake` is the advisor-side analog of `aida burndown run`: where the
//! burndown launcher fires headless IMPLEMENTER agents over the blessed ready
//! set, `intake` fires a headless ADVISOR agent that reads all open specs,
//! applies worth-doing judgment, and proposes approve/reject/park/queue
//! dispositions — propose-by-default, `--apply` executes.
//!
//! This module is the side-effect-free heart: the `[intake]` policy
//! (P1/P2/P3), the pure candidate-FENCE computation (the bounded set the agent
//! may act on — the do-not-approve classes are excluded HERE, programmatically,
//! so the agent never sees them as actionable), and the skill-prompt builder.
//! The launcher itself (store load + headless `claude -p` spawn) lives in
//! `main.rs`, mirroring `handle_burndown_run`. trace:STORY-560 | ai:claude

use std::path::Path;

/// P1 — `intake.disposition_bias`: the cold-boot worth-doing posture. The
/// headless advisor is a COLD-BOOT `claude -p`, not the operator's live
/// session, so the bias tunes how aggressively it blesses autonomy-eligible
/// work. Propose-mode-by-default remains the ultimate gate regardless.
/// trace:STORY-560 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispositionBias {
    /// DEFAULT: propose approve for every autonomy-eligible spec; the
    /// propose-mode review is the worth-doing filter.
    ApproveEligible,
    /// Feed the agent a strategic-frame input (project phase / priorities) and
    /// propose approve only when a spec is BOTH eligible AND clearly aligned,
    /// else park-for-human.
    ParkAligned,
    /// Park-when-unsure bias, no strategic-frame input fed.
    ParkConservative,
}

impl DispositionBias {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "approve-eligible" | "approve_eligible" | "approve" => Some(Self::ApproveEligible),
            "park-aligned" | "park_aligned" | "aligned" => Some(Self::ParkAligned),
            "park-conservative" | "park_conservative" | "conservative" => {
                Some(Self::ParkConservative)
            }
            _ => None,
        }
    }

    /// The token passed to the skill (env + reporting). Stable, kebab-case.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApproveEligible => "approve-eligible",
            Self::ParkAligned => "park-aligned",
            Self::ParkConservative => "park-conservative",
        }
    }
}

/// P3 — `intake.on_apply`: what `--apply` does after queuing. Bounds the
/// compounding unattended authority (cold-boot approve + implementer drain in
/// one shot). trace:STORY-560 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnApply {
    /// DEFAULT: stop at queuing; draining is a separate explicit
    /// `aida burndown run`.
    Queue,
    /// Chain straight into a burndown drain after queuing.
    Drain,
}

impl OnApply {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "queue" | "stop" => Some(Self::Queue),
            "drain" => Some(Self::Drain),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Drain => "drain",
        }
    }
}

/// The default P2 do-not-approve classes: strategic + knowledge-graph types
/// (advisor-authored, not implementer work). The agent can NEVER propose
/// approve for these, even with `--apply` — they are fenced out of the
/// candidate set by the launcher. trace:STORY-560 | ai:claude
pub const DEFAULT_DO_NOT_APPROVE_CLASSES: &[&str] = &[
    "vision",
    "epic",
    "principle",
    "constraint",
    "decision",
    "term",
];

/// The always-on tag exclusions: a spec tagged `needs-human` or `strategic` is
/// never the agent's to bless, regardless of the configurable class list.
/// trace:STORY-560 | ai:claude
pub const ALWAYS_EXCLUDE_TAGS: &[&str] = &["needs-human", "strategic"];

/// `[intake]` section in `.aida/config.toml`. Each field has a safe default so
/// `aida intake` works out-of-the-box with no config (propose-mode is always
/// the gate). trace:STORY-560 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntakeConfig {
    /// P1 — the cold-boot worth-doing posture.
    pub disposition_bias: DispositionBias,
    /// P2 — the HARD authority bound: requirement types the agent can never
    /// propose approve for. Lowercased.
    pub do_not_approve_classes: Vec<String>,
    /// P3 — what `--apply` does after queuing.
    pub on_apply: OnApply,
}

impl Default for IntakeConfig {
    fn default() -> Self {
        Self {
            disposition_bias: DispositionBias::ApproveEligible,
            do_not_approve_classes: DEFAULT_DO_NOT_APPROVE_CLASSES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            on_apply: OnApply::Queue,
        }
    }
}

impl IntakeConfig {
    /// Load `[intake]` from `<project_root>/.aida/config.toml`. Missing file /
    /// section / keys all fall through to defaults — a config error never
    /// blocks intake. trace:STORY-560
    pub fn load(project_root: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        else {
            return Self::default();
        };
        Self::from_toml_str(&content)
    }

    /// Build from a raw TOML string — also used by tests so they don't touch
    /// the filesystem.
    pub fn from_toml_str(content: &str) -> Self {
        let mut cfg = Self::default();
        for (key, val) in scan_intake_section(content) {
            apply_key(&mut cfg, &key, &val);
        }
        cfg
    }

    /// True iff `req_type` (already lowercased) is a do-not-approve class.
    pub fn is_do_not_approve_class(&self, req_type: &str) -> bool {
        self.do_not_approve_classes
            .iter()
            .any(|c| c.eq_ignore_ascii_case(req_type))
    }
}

fn apply_key(cfg: &mut IntakeConfig, key: &str, val: &str) {
    // Scalar values keep their surrounding quotes (the scanner preserves them
    // so the `do_not_approve_classes` array literal survives) — strip them here.
    let scalar = val.trim().trim_matches('"').trim_matches('\'').trim();
    match key {
        "disposition_bias" => {
            if let Some(b) = DispositionBias::parse(scalar) {
                cfg.disposition_bias = b;
            }
        }
        "on_apply" => {
            if let Some(o) = OnApply::parse(scalar) {
                cfg.on_apply = o;
            }
        }
        "do_not_approve_classes" => {
            // Accept a TOML array literal (`["vision", "epic"]`) or a bare
            // comma-separated list. Empty list = the operator deliberately
            // opened the gate (allowed — the propose-mode review still gates).
            let inner = val.trim().trim_start_matches('[').trim_end_matches(']');
            let classes: Vec<String> = inner
                .split(',')
                .map(|s| s.trim().trim_matches('"').trim_matches('\'').trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_ascii_lowercase())
                .collect();
            cfg.do_not_approve_classes = classes;
        }
        _ => {}
    }
}

/// Extract `key = value` pairs from `[intake]`. Section-aware; stops at the
/// next `[section]`. Mirrors the hand-rolled scanner used by `[advisor]` /
/// `workflow_hints` so we don't pull a serde-toml dependency. trace:STORY-560
fn scan_intake_section(content: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut in_intake = false;
    for raw in content.lines() {
        let line = strip_inline_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_intake = stripped.trim_end_matches(']').trim() == "intake";
            continue;
        }
        if in_intake {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim();
                pairs.push((k.trim().to_string(), v.to_string()));
            }
        }
    }
    pairs
}

/// Strip a `#` inline comment that is not inside quotes. Shared shape with
/// `advisor::strip_inline_comment`. trace:STORY-560
fn strip_inline_comment(s: &str) -> &str {
    let (mut dq, mut sq) = (false, false);
    for (i, c) in s.char_indices() {
        match c {
            '"' if !sq => dq = !dq,
            '\'' if !dq => sq = !sq,
            '#' if !dq && !sq => return &s[..i],
            _ => {}
        }
    }
    s
}

/// One open spec's already-probed facts, built in `main.rs` from the store.
/// Consumed by the pure [`select_intake_candidates`]. trace:STORY-560
#[derive(Debug, Clone)]
pub struct IntakeSpec {
    /// Display SPEC-ID (e.g. `STORY-560`).
    pub id: String,
    /// Lowercased requirement type (`story`, `task`, `vision`, …).
    pub req_type: String,
    /// The spec's tags (case-insensitive checks).
    pub tags: Vec<String>,
    /// Advisory risk token (`low` / `medium` / `high` / `unknown`) — the same
    /// chip `aida backlog list` shows. Used for the `--risk` ceiling.
    pub risk: crate::backlog::RiskLevel,
}

/// Why a spec was fenced OUT of the agent's actionable set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FenceReason {
    /// A P2 do-not-approve requirement class (strategic / knowledge-graph).
    DoNotApproveClass(String),
    /// Carries an always-on exclusion tag (`needs-human` / `strategic`) or a
    /// `--exclude-tag` match.
    ExcludedTag(String),
    /// `--only-tag` is set and this spec doesn't carry it.
    NotOnlyTag(String),
    /// Risk above the `--risk` ceiling.
    RiskCeiling(crate::backlog::RiskLevel),
}

impl FenceReason {
    pub fn describe(&self) -> String {
        match self {
            FenceReason::DoNotApproveClass(t) => {
                format!("do-not-approve class `{t}` (advisor-authored)")
            }
            FenceReason::ExcludedTag(t) => format!("excluded tag `{t}`"),
            FenceReason::NotOnlyTag(t) => format!("missing required tag `{t}`"),
            FenceReason::RiskCeiling(r) => format!("risk above ceiling ({})", r.token()),
        }
    }
}

/// Per-run guardrail filters layered on top of the `[intake]` config. Flags
/// override config for a single run (acceptance #3). trace:STORY-560
#[derive(Debug, Clone)]
pub struct IntakeFilters {
    /// Only consider specs carrying this tag.
    pub only_tag: Option<String>,
    /// Never consider specs carrying this tag.
    pub exclude_tag: Option<String>,
    /// Exclude candidates riskier than this ceiling.
    pub max_risk: crate::backlog::RiskLevel,
}

impl Default for IntakeFilters {
    fn default() -> Self {
        // The `--risk` default mirrors `backlog groom --pickable`: medium.
        Self {
            only_tag: None,
            exclude_tag: None,
            max_risk: crate::backlog::RiskLevel::Medium,
        }
    }
}

fn tags_contain(tags: &[String], name: &str) -> bool {
    tags.iter().any(|t| t.trim().eq_ignore_ascii_case(name))
}

/// Pure candidate FENCE. Partition the open specs into `(eligible, fenced)`:
/// the eligible set is what the headless advisor may act on; the fenced set is
/// excluded with a reason. The P2 do-not-approve classes and the always-on tag
/// exclusions are applied HERE — the agent never sees a fenced spec as
/// actionable (substrate-as-bouncer for the HARD authority bound).
/// Order-preserving + side-effect-free so it is exhaustively unit-testable.
/// trace:STORY-560 | ai:claude
pub fn select_intake_candidates(
    specs: &[IntakeSpec],
    cfg: &IntakeConfig,
    filters: &IntakeFilters,
) -> (Vec<String>, Vec<(String, FenceReason)>) {
    let mut eligible: Vec<String> = Vec::new();
    let mut fenced: Vec<(String, FenceReason)> = Vec::new();

    for spec in specs {
        // P2 — HARD authority bound: the do-not-approve classes, fenced first.
        if cfg.is_do_not_approve_class(&spec.req_type) {
            fenced.push((
                spec.id.clone(),
                FenceReason::DoNotApproveClass(spec.req_type.clone()),
            ));
            continue;
        }
        // P2 always-on tag exclusions + the per-run --exclude-tag.
        if let Some(t) = ALWAYS_EXCLUDE_TAGS
            .iter()
            .find(|t| tags_contain(&spec.tags, t))
        {
            fenced.push((spec.id.clone(), FenceReason::ExcludedTag((*t).to_string())));
            continue;
        }
        if let Some(ex) = &filters.exclude_tag {
            if tags_contain(&spec.tags, ex) {
                fenced.push((spec.id.clone(), FenceReason::ExcludedTag(ex.clone())));
                continue;
            }
        }
        // Per-run --only-tag narrowing.
        if let Some(only) = &filters.only_tag {
            if !tags_contain(&spec.tags, only) {
                fenced.push((spec.id.clone(), FenceReason::NotOnlyTag(only.clone())));
                continue;
            }
        }
        // Risk ceiling.
        if !spec.risk.within_ceiling(filters.max_risk) {
            fenced.push((spec.id.clone(), FenceReason::RiskCeiling(spec.risk)));
            continue;
        }
        eligible.push(spec.id.clone());
    }

    (eligible, fenced)
}

/// Build the `/aida-intake` slash-command string the headless session runs.
/// Propose-by-default; `--apply` executes. Pure + unit-testable. The policy and
/// the bounded candidate fence are passed via env (`AIDA_INTAKE_*`), not the
/// prompt, so the prompt stays the human-facing surface. trace:STORY-560
pub fn intake_skill_prompt(apply: bool) -> String {
    if apply {
        "/aida-intake --apply".to_string()
    } else {
        "/aida-intake".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog::RiskLevel;

    fn spec(id: &str, ty: &str, tags: &[&str], risk: RiskLevel) -> IntakeSpec {
        IntakeSpec {
            id: id.to_string(),
            req_type: ty.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            risk,
        }
    }

    #[test]
    fn config_defaults_are_safe() {
        let cfg = IntakeConfig::default();
        assert_eq!(cfg.disposition_bias, DispositionBias::ApproveEligible);
        assert_eq!(cfg.on_apply, OnApply::Queue);
        assert!(cfg.is_do_not_approve_class("vision"));
        assert!(cfg.is_do_not_approve_class("epic"));
        assert!(cfg.is_do_not_approve_class("decision"));
        assert!(!cfg.is_do_not_approve_class("story"));
        assert!(!cfg.is_do_not_approve_class("task"));
    }

    #[test]
    fn config_parses_intake_block() {
        let toml = r#"
[deployment]
mode = "distributed"

[intake]
disposition_bias = "park-aligned"
on_apply = "drain"
do_not_approve_classes = ["vision", "epic"]

[hints]
workflow_hints = true
"#;
        let cfg = IntakeConfig::from_toml_str(toml);
        assert_eq!(cfg.disposition_bias, DispositionBias::ParkAligned);
        assert_eq!(cfg.on_apply, OnApply::Drain);
        assert_eq!(cfg.do_not_approve_classes, vec!["vision", "epic"]);
        assert!(!cfg.is_do_not_approve_class("principle")); // removed from default
    }

    #[test]
    fn config_missing_section_is_default() {
        let cfg = IntakeConfig::from_toml_str("[deployment]\nmode = \"distributed\"\n");
        assert_eq!(cfg, IntakeConfig::default());
    }

    #[test]
    fn config_bare_comma_list_for_classes() {
        let cfg = IntakeConfig::from_toml_str("[intake]\ndo_not_approve_classes = vision, epic\n");
        assert_eq!(cfg.do_not_approve_classes, vec!["vision", "epic"]);
    }

    #[test]
    fn select_excludes_do_not_approve_classes() {
        let cfg = IntakeConfig::default();
        let filters = IntakeFilters {
            max_risk: RiskLevel::High,
            ..Default::default()
        };
        let specs = vec![
            spec("STORY-1", "story", &[], RiskLevel::Low),
            spec("VIS-1", "vision", &[], RiskLevel::Low),
            spec("EPIC-1", "epic", &[], RiskLevel::Low),
            spec("TASK-1", "task", &[], RiskLevel::Low),
        ];
        let (eligible, fenced) = select_intake_candidates(&specs, &cfg, &filters);
        assert_eq!(eligible, vec!["STORY-1", "TASK-1"]);
        assert_eq!(fenced.len(), 2);
        assert!(matches!(fenced[0].1, FenceReason::DoNotApproveClass(_)));
    }

    #[test]
    fn select_excludes_needs_human_and_strategic_tags() {
        let cfg = IntakeConfig::default();
        let filters = IntakeFilters {
            max_risk: RiskLevel::High,
            ..Default::default()
        };
        let specs = vec![
            spec("STORY-1", "story", &["needs-human"], RiskLevel::Low),
            spec("STORY-2", "story", &["strategic"], RiskLevel::Low),
            spec("STORY-3", "story", &["ok"], RiskLevel::Low),
        ];
        let (eligible, fenced) = select_intake_candidates(&specs, &cfg, &filters);
        assert_eq!(eligible, vec!["STORY-3"]);
        assert_eq!(fenced.len(), 2);
    }

    #[test]
    fn select_applies_only_and_exclude_tag() {
        let cfg = IntakeConfig::default();
        let filters = IntakeFilters {
            only_tag: Some("papercut".to_string()),
            exclude_tag: Some("wip".to_string()),
            max_risk: RiskLevel::High,
        };
        let specs = vec![
            spec("STORY-1", "story", &["papercut"], RiskLevel::Low),
            spec("STORY-2", "story", &["papercut", "wip"], RiskLevel::Low),
            spec("STORY-3", "story", &["other"], RiskLevel::Low),
        ];
        let (eligible, fenced) = select_intake_candidates(&specs, &cfg, &filters);
        assert_eq!(eligible, vec!["STORY-1"]);
        // STORY-2 fenced by exclude-tag (checked before only-tag), STORY-3 by only-tag.
        assert_eq!(fenced.len(), 2);
    }

    #[test]
    fn select_applies_risk_ceiling() {
        let cfg = IntakeConfig::default();
        let filters = IntakeFilters {
            max_risk: RiskLevel::Low,
            ..Default::default()
        };
        let specs = vec![
            spec("STORY-1", "story", &[], RiskLevel::Low),
            spec("STORY-2", "story", &[], RiskLevel::High),
            spec("STORY-3", "story", &[], RiskLevel::Medium),
        ];
        let (eligible, fenced) = select_intake_candidates(&specs, &cfg, &filters);
        assert_eq!(eligible, vec!["STORY-1"]);
        assert_eq!(fenced.len(), 2);
        assert!(matches!(fenced[0].1, FenceReason::RiskCeiling(_)));
    }

    #[test]
    fn skill_prompt_propose_vs_apply() {
        assert_eq!(intake_skill_prompt(false), "/aida-intake");
        assert_eq!(intake_skill_prompt(true), "/aida-intake --apply");
    }
}
