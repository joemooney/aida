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
    /// Whether the spec is in the operator's DEFERRED tier (STORY-584): either
    /// the deferred view-flag is set OR it carries a legacy `deferred:*` parking
    /// tag. Deferred specs are fenced out — re-blessing the operator's deferral
    /// shelf would undo the curation the deferred tier exists to protect.
    /// Computed by [`is_deferred`] in the launcher (mirrors the `aida list`
    /// honor-both predicate). trace:BUG-561 | ai:claude
    pub deferred: bool,
    /// Advisory risk token (`low` / `medium` / `high` / `unknown`) — the same
    /// chip `aida backlog list` shows. Used for the `--risk` ceiling.
    pub risk: crate::backlog::RiskLevel,
    /// BUG-595: the one-line reason the risk level was assigned, so a fenced
    /// spec's disposition is legible ("risk above ceiling (unknown) —
    /// under-specified …") rather than an opaque chip. trace:BUG-595 | ai:claude
    pub risk_reason: String,
}

/// The canonical "is this spec in the deferred tier?" predicate, shared so the
/// intake fence matches `aida list` exactly (STORY-584 honor-both): the deferred
/// view-flag is set, OR the spec carries any legacy `deferred:*` parking tag.
/// The launcher computes [`IntakeSpec::deferred`] through this. trace:BUG-561 | ai:claude
pub fn is_deferred(deferred_flag: bool, tags: &[String]) -> bool {
    deferred_flag
        || tags
            .iter()
            .any(|t| t.trim().to_ascii_lowercase().starts_with("deferred:"))
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
    /// In the operator's DEFERRED tier (STORY-584): deferred view-flag set or a
    /// legacy `deferred:*` parking tag. Re-blessing it would undo the operator's
    /// deferral curation. trace:BUG-561
    Deferred,
    /// BUG-594: keystone / supervised work the operator explicitly reserved for
    /// human judgment (`keystone` / `supervised` / `architecture` / `security` /
    /// epic, etc.). Fenced INDEPENDENT of the risk ceiling — the same invariant
    /// `aida queue integrate` enforces (TASK-813). Carries the matched marker.
    /// trace:BUG-594 | ai:claude
    Keystone(String),
    /// Risk above the `--risk` ceiling. BUG-595: carries the per-spec risk
    /// reason so the disposition is legible, not just the chip.
    RiskCeiling(crate::backlog::RiskLevel, String),
}

impl FenceReason {
    pub fn describe(&self) -> String {
        match self {
            FenceReason::DoNotApproveClass(t) => {
                format!("do-not-approve class `{t}` (advisor-authored)")
            }
            FenceReason::ExcludedTag(t) => format!("excluded tag `{t}`"),
            FenceReason::NotOnlyTag(t) => format!("missing required tag `{t}`"),
            FenceReason::Deferred => "deferred".to_string(),
            FenceReason::Keystone(marker) => {
                format!("keystone/supervised (`{marker}`) — reserved for human review")
            }
            FenceReason::RiskCeiling(r, reason) => {
                format!("risk above ceiling ({}) — {reason}", r.token())
            }
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

/// BUG-594: is this spec keystone / supervised — work the operator reserved for
/// human judgment? Reuses the canonical [`crate::presence::is_keystone_class`]
/// detector (epic type, or any `keystone` / `architecture` / `security` /
/// `supervised` / `needs-supervised-build` / `blast-radius:high` / `risk:high`
/// tag) so assess can never disagree with the drain / `queue integrate` on what
/// keystone means. Returns the matched marker for the fence reason.
/// trace:BUG-594 | ai:claude
fn keystone_marker(req_type: &str, tags: &[String]) -> Option<String> {
    if !crate::presence::is_keystone_class(req_type, tags.iter().map(|s| s.as_str())) {
        return None;
    }
    if req_type.trim().eq_ignore_ascii_case("epic") {
        return Some("epic".to_string());
    }
    // Surface the specific tag that triggered the fence (most informative).
    const KEYSTONE_TAGS: &[&str] = &[
        "keystone",
        "supervised",
        "architecture",
        "security",
        "needs-supervised-build",
        "blast-radius:high",
        "risk:high",
    ];
    tags.iter()
        .find(|t| {
            let lo = t.trim().to_ascii_lowercase();
            KEYSTONE_TAGS.contains(&lo.as_str())
        })
        .map(|t| t.trim().to_string())
        .or_else(|| Some("keystone".to_string()))
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
        // BUG-561: the operator's DEFERRED tier (STORY-584) is fenced out the
        // SAME way `aida list` hides it — deferred view-flag OR a `deferred:*`
        // parking tag. A headless `--apply` must NOT re-bless / re-queue the
        // operator's deferral shelf. trace:BUG-561 | ai:claude
        if spec.deferred {
            fenced.push((spec.id.clone(), FenceReason::Deferred));
            continue;
        }
        // BUG-594: keystone / supervised work is fenced INDEPENDENT of the risk
        // ceiling — the same human-judgment invariant `aida queue integrate`
        // enforces (TASK-813). Without this, a `keystone,supervised` spec whose
        // inferred risk happens to sit under the ceiling (or `--risk high`)
        // becomes an approve/queue candidate the cold-boot advisor can bless —
        // exactly the work the operator reserved for themselves. Reuses the
        // canonical `presence::is_keystone_class` detector. trace:BUG-594
        if let Some(marker) = keystone_marker(&spec.req_type, &spec.tags) {
            fenced.push((spec.id.clone(), FenceReason::Keystone(marker)));
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
            fenced.push((
                spec.id.clone(),
                FenceReason::RiskCeiling(spec.risk, spec.risk_reason.clone()),
            ));
            continue;
        }
        eligible.push(spec.id.clone());
    }

    (eligible, fenced)
}

/// Build the `/aida-assess` slash-command string the headless session runs.
/// Propose-by-default; `--apply` executes. Pure + unit-testable. The policy and
/// the bounded candidate fence are passed via env (`AIDA_INTAKE_*`), not the
/// prompt, so the prompt stays the human-facing surface. trace:STORY-560
pub fn intake_skill_prompt(apply: bool) -> String {
    if apply {
        "/aida-assess --apply".to_string()
    } else {
        "/aida-assess".to_string()
    }
}

/// Relative path (under the project root) of the live advisor's context-seed
/// file. The LIVE advisor maintains this file directly; the cold-boot launcher
/// reads it and prepends it to the `/aida-assess` prompt. `.aida/*` is
/// gitignored deny-by-default, so this is per-clone runtime state.
/// trace:STORY-626 | ai:claude
pub const ADVISOR_CONTEXT_SEED_REL: &str = ".aida/advisor-context.md";

/// Build the cold-boot `/aida-assess` prompt, seeded with the live advisor's
/// context file when one is present and non-empty. The headless cold-boot
/// advisor otherwise starts context-poor and re-derives priorities every run;
/// prepending the seed lets unattended assess decisions match the live session.
///
/// When `.aida/advisor-context.md` exists and has non-whitespace content the
/// returned prompt is the seed wrapped in a `## Live advisor context (seed …)`
/// heading, a `---` rule, then the bare `/aida-assess [--apply]`. With no seed
/// (or an empty file) it returns the bare `intake_skill_prompt(apply)` form.
/// trace:STORY-626 | ai:claude
pub fn seeded_assess_prompt(project_root: &std::path::Path, apply: bool) -> String {
    seed_skill_prompt(project_root, &intake_skill_prompt(apply))
}

/// Wrap ANY bare cold-boot skill invocation with the live advisor context seed —
/// the shared core behind both the assess and the burndown `/aida-advise`
/// cold-boots (STORY-626). Returns `bare` unchanged when
/// `.aida/advisor-context.md` is absent or empty. trace:STORY-626 | ai:claude
pub fn seed_skill_prompt(project_root: &std::path::Path, bare: &str) -> String {
    let seed_path = project_root.join(ADVISOR_CONTEXT_SEED_REL);
    let seed = match std::fs::read_to_string(&seed_path) {
        Ok(contents) if !contents.trim().is_empty() => contents,
        _ => return bare.to_string(),
    };
    format!(
        "## Live advisor context (seed — current ground-truth from the live advisor)\n\n\
         {}\n\n---\n\n{}",
        seed.trim_end(),
        bare
    )
}

/// Vendor-aware cold-boot `/aida-advise` prompt, seeded like the assess prompt.
/// The burndown advisor tier launches this on a punt cold-boot; the Claude fork
/// branch gets live context via `--resume`, so only the cold-boot needs the
/// seed.
///
/// Claude Code expands `/aida-advise` from `.claude/skills/aida-advise.md` on
/// demand, so for Claude the bare slash invocation is kept (cheap — the body
/// loads lazily). A non-Claude vendor (Codex, Gemini) never reads
/// `.claude/skills/`, so the slash token is inert noise to it; for those we
/// inline the embedded skill body so the launched `codex exec` (etc.) actually
/// receives the advisor prompt. This is the `seed_skill_prompt` hook that makes
/// AIDA's skill guidance reachable by a non-Claude drain agent.
// trace:STORY-626 trace:TASK-1045 | ai:claude
pub fn seeded_advise_prompt_for_vendor(project_root: &std::path::Path, is_claude: bool) -> String {
    seed_skill_prompt(
        project_root,
        &materialize_skill_invocation("/aida-advise", is_claude),
    )
}

/// Materialize a bare `/aida-<skill>` slash invocation into a form the target
/// vendor can actually consume.
///
/// `.claude/skills/*.md` are Claude-Code-native prompts: Claude expands the
/// `/aida-<skill>` slash command from that directory. A non-Claude vendor never
/// reads `.claude/skills/`, so the slash token is invisible to it. For a
/// non-Claude vendor we therefore inline the embedded skill body (YAML
/// frontmatter — a Claude Code convention — stripped) so the prompt is
/// self-contained. Trailing invocation arguments (`/aida-assess --apply`) are
/// appended as an explicit note. Claude keeps the slash form. Unknown skills
/// and non-slash input pass through unchanged.
// trace:TASK-1045 | ai:claude
pub fn materialize_skill_invocation(bare: &str, is_claude: bool) -> String {
    if is_claude {
        return bare.to_string();
    }
    let trimmed = bare.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return bare.to_string();
    };
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("");
    let args = parts.next().map(str::trim).unwrap_or("");
    let key = format!("skills/{name}.md");
    let Some(body) = aida_core::templates::EMBEDDED_TEMPLATES
        .get(key.as_str())
        .map(|s| s.to_string())
    else {
        // Unknown skill — pass the slash form through rather than swallow it.
        return bare.to_string();
    };
    let body = aida_core::scaffolding::codex_prompts::strip_frontmatter(&body)
        .trim()
        .to_string();
    if args.is_empty() {
        body
    } else {
        format!("{body}\n\n(Invocation arguments: {args})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog::RiskLevel;

    fn spec(id: &str, ty: &str, tags: &[&str], risk: RiskLevel) -> IntakeSpec {
        let tag_vec: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        // Mirror the launcher: a spec carrying a `deferred:*` tag is deferred
        // even without the flag (the helper-under-test's honor-both rule). The
        // flag-only case is set explicitly in the deferred test. trace:BUG-561
        let deferred = is_deferred(false, &tag_vec);
        IntakeSpec {
            id: id.to_string(),
            req_type: ty.to_string(),
            tags: tag_vec,
            deferred,
            risk,
            risk_reason: format!("{} (test)", risk.token()),
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

    /// BUG-561: the operator's DEFERRED tier (STORY-584) must be fenced out the
    /// same way `aida list` hides it — by the deferred view-flag AND by a legacy
    /// `deferred:*` parking tag. Without the fix these specs leak into the
    /// eligible set and a headless `--apply` would re-bless the deferral shelf.
    #[test]
    fn select_excludes_deferred_specs() {
        let cfg = IntakeConfig::default();
        let filters = IntakeFilters {
            max_risk: RiskLevel::High,
            ..Default::default()
        };
        // STORY-1 deferred via the view-flag; STORY-2 via a legacy parking tag;
        // STORY-3 is genuinely active.
        let mut flagged = spec("STORY-1", "story", &[], RiskLevel::Low);
        flagged.deferred = true;
        let specs = vec![
            flagged,
            spec(
                "STORY-2",
                "story",
                &["deferred:stabilization-first"],
                RiskLevel::Low,
            ),
            spec("STORY-3", "story", &["ok"], RiskLevel::Low),
        ];
        let (eligible, fenced) = select_intake_candidates(&specs, &cfg, &filters);
        // Only the genuinely-active draft is weighed.
        assert_eq!(eligible, vec!["STORY-3"]);
        // Both deferred specs are fenced with reason `deferred`.
        assert_eq!(fenced.len(), 2);
        assert!(fenced
            .iter()
            .all(|(_, r)| matches!(r, FenceReason::Deferred)));
        assert!(fenced.iter().any(|(id, _)| id == "STORY-1"));
        assert!(fenced.iter().any(|(id, _)| id == "STORY-2"));
        // The acceptance string: reason renders as `deferred`.
        assert_eq!(fenced[0].1.describe(), "deferred");
    }

    /// BUG-561: the shared honor-both predicate matches `aida list` (STORY-584).
    #[test]
    fn is_deferred_honors_flag_and_tag() {
        assert!(is_deferred(true, &[]));
        assert!(is_deferred(false, &["deferred:post-stability".to_string()]));
        assert!(is_deferred(false, &["DEFERRED:Foo".to_string()])); // case-insensitive
        assert!(!is_deferred(false, &["deferral".to_string()])); // not the prefix
        assert!(!is_deferred(false, &["ok".to_string()]));
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
        assert!(matches!(fenced[0].1, FenceReason::RiskCeiling(_, _)));
    }

    // BUG-594: keystone / supervised work is fenced INDEPENDENT of the risk
    // ceiling — the same human-judgment invariant `queue integrate` enforces.
    #[test]
    fn keystone_is_fenced_even_under_permissive_risk_ceiling() {
        let cfg = IntakeConfig::default();
        let filters = IntakeFilters {
            max_risk: RiskLevel::High, // permissive: admits everything by risk
            ..Default::default()
        };
        let specs = vec![
            spec(
                "STORY-11",
                "story",
                &["keystone", "supervised"],
                RiskLevel::Low,
            ),
            spec("TASK-1", "task", &[], RiskLevel::Low),
        ];
        let (eligible, fenced) = select_intake_candidates(&specs, &cfg, &filters);
        // The keystone spec is fenced even though its risk is under the ceiling.
        assert_eq!(eligible, vec!["TASK-1"]);
        assert!(
            fenced
                .iter()
                .any(|(id, r)| id == "STORY-11" && matches!(r, FenceReason::Keystone(_))),
            "keystone spec must be fenced, got: {fenced:?}"
        );
    }

    // BUG-594: a bare `supervised` tag (no `keystone`) is also fenced.
    #[test]
    fn supervised_alone_is_fenced() {
        let cfg = IntakeConfig::default();
        let filters = IntakeFilters {
            max_risk: RiskLevel::High,
            ..Default::default()
        };
        let specs = vec![spec("TASK-9", "task", &["supervised"], RiskLevel::Low)];
        let (eligible, fenced) = select_intake_candidates(&specs, &cfg, &filters);
        assert!(eligible.is_empty());
        assert!(matches!(fenced[0].1, FenceReason::Keystone(_)));
    }

    #[test]
    fn skill_prompt_propose_vs_apply() {
        assert_eq!(intake_skill_prompt(false), "/aida-assess");
        assert_eq!(intake_skill_prompt(true), "/aida-assess --apply");
    }

    #[test]
    fn seeded_prompt_no_file_is_bare() {
        let dir = std::env::temp_dir().join(format!("aida-seed-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // No .aida/advisor-context.md present.
        assert_eq!(seeded_assess_prompt(&dir, false), "/aida-assess");
        assert_eq!(seeded_assess_prompt(&dir, true), "/aida-assess --apply");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seeded_prompt_empty_file_is_bare() {
        let dir = std::env::temp_dir().join(format!("aida-seed-empty-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".aida")).unwrap();
        std::fs::write(dir.join(ADVISOR_CONTEXT_SEED_REL), "   \n\n").unwrap();
        assert_eq!(seeded_assess_prompt(&dir, false), "/aida-assess");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seeded_prompt_prepends_content() {
        let dir = std::env::temp_dir().join(format!("aida-seed-full-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".aida")).unwrap();
        let body = "Phase: stabilization-first.\nPriority: clear bugs before features.";
        std::fs::write(dir.join(ADVISOR_CONTEXT_SEED_REL), body).unwrap();

        let out = seeded_assess_prompt(&dir, false);
        assert!(out.starts_with(
            "## Live advisor context (seed — current ground-truth from the live advisor)"
        ));
        assert!(out.contains(body));
        assert!(out.contains("\n---\n"));
        assert!(out.ends_with("/aida-assess"));

        let out_apply = seeded_assess_prompt(&dir, true);
        assert!(out_apply.ends_with("/aida-assess --apply"));
        assert!(out_apply.contains(body));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// STORY-626: the burndown advisor-tier cold-boot is seeded the same way —
    /// bare `/aida-advise` with no file, prepended when the seed is present.
    #[test]
    fn seeded_advise_prompt_seeds_like_assess() {
        let dir = std::env::temp_dir().join(format!("aida-seed-advise-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".aida")).unwrap();

        // No seed file → bare (Claude form).
        assert_eq!(seeded_advise_prompt_for_vendor(&dir, true), "/aida-advise");

        // Seed present → prepended, ends with the bare /aida-advise.
        let body = "Phase: clear open items. Lane: advisor = merge gate.";
        std::fs::write(dir.join(ADVISOR_CONTEXT_SEED_REL), body).unwrap();
        let out = seeded_advise_prompt_for_vendor(&dir, true);
        assert!(out.starts_with(
            "## Live advisor context (seed — current ground-truth from the live advisor)"
        ));
        assert!(out.contains(body));
        assert!(out.ends_with("/aida-advise"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// TASK-1045: Claude keeps the bare slash form (expanded on demand); a
    /// non-Claude vendor gets the embedded skill body inlined so the prompt is
    /// self-contained.
    #[test]
    fn materialize_skill_invocation_inlines_body_only_for_non_claude() {
        // Claude → unchanged slash form.
        assert_eq!(
            materialize_skill_invocation("/aida-advise", true),
            "/aida-advise"
        );

        // Non-Claude → the actual embedded skill body, not the slash token.
        let inlined = materialize_skill_invocation("/aida-advise", false);
        assert_ne!(inlined, "/aida-advise");
        assert!(
            !inlined.starts_with('/'),
            "should be a body, not a slash invocation: {inlined}"
        );
        // Frontmatter (a Claude Code convention) is stripped.
        assert!(
            !inlined.starts_with("---"),
            "YAML frontmatter must be stripped: {inlined}"
        );
        // The body carries the advisor skill's actual guidance — a durable
        // phrase from `skills/aida-advise.md`, keyed off the embedded master.
        let master = aida_core::templates::EMBEDDED_TEMPLATES
            .get("skills/aida-advise.md")
            .map(|s| s.to_string())
            .expect("aida-advise skill is embedded");
        let master_body = aida_core::scaffolding::codex_prompts::strip_frontmatter(&master).trim();
        assert!(!master_body.is_empty());
        assert_eq!(inlined, master_body);
    }

    /// TASK-1045: trailing arguments survive as an explicit note, and unknown /
    /// non-slash input passes through unchanged.
    #[test]
    fn materialize_skill_invocation_handles_args_and_passthrough() {
        // Trailing args are preserved for a non-Claude vendor.
        let with_args = materialize_skill_invocation("/aida-assess --apply", false);
        assert!(
            with_args.contains("(Invocation arguments: --apply)"),
            "{with_args}"
        );

        // Unknown skill → slash form passes through (never swallowed).
        assert_eq!(
            materialize_skill_invocation("/aida-nonexistent-skill", false),
            "/aida-nonexistent-skill"
        );
        // Non-slash input passes through.
        assert_eq!(
            materialize_skill_invocation("plain prompt text", false),
            "plain prompt text"
        );
    }

    /// TASK-1045: the vendor-aware advise prompt keeps Claude on the slash form
    /// and inlines the skill body for a non-Claude vendor, in both cases still
    /// carrying the STORY-626 advisor-context seed when present.
    #[test]
    fn seeded_advise_prompt_for_vendor_materializes_for_non_claude() {
        let dir =
            std::env::temp_dir().join(format!("aida-seed-advise-vendor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".aida")).unwrap();

        // No seed file: Claude → bare slash; non-Claude → inlined body.
        assert_eq!(seeded_advise_prompt_for_vendor(&dir, true), "/aida-advise");
        let non_claude = seeded_advise_prompt_for_vendor(&dir, false);
        assert_ne!(non_claude, "/aida-advise");
        assert!(!non_claude.ends_with("/aida-advise"));

        // Seed present: prepended for both; the non-Claude tail is the body, not
        // the inert slash token.
        let body = "Phase: clear open items. Lane: advisor = merge gate.";
        std::fs::write(dir.join(ADVISOR_CONTEXT_SEED_REL), body).unwrap();
        let seeded_non_claude = seeded_advise_prompt_for_vendor(&dir, false);
        assert!(seeded_non_claude.starts_with(
            "## Live advisor context (seed — current ground-truth from the live advisor)"
        ));
        assert!(seeded_non_claude.contains(body));
        assert!(!seeded_non_claude.ends_with("/aida-advise"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
