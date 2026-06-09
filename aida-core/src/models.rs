use crate::ai::StoredAiEvaluation;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::fmt;
use ts_rs_forge::TS;
use uuid::Uuid;

/// Represents the status of a requirement
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
pub enum RequirementStatus {
    Draft,
    Approved,
    Planned,
    InProgress,
    /// Work finished on a branch; not yet merged to default branch.
    /// Auto-bumped to `Completed` by `aida pull` / `aida db sync --pull`
    /// when a commit referencing this spec lands on the default branch.
    /// trace:STORY-86 | ai:claude
    Done,
    Completed,
    Rejected,
    /// Work was in progress but is now paused — an autonomous agent hit a
    /// design-fork it could not safely resolve and punted (`aida punt` /
    /// `/aida-punt`) instead of guessing. A human or advisor must decide
    /// something before it can proceed. Reached only from `InProgress`;
    /// resolved out to `Approved` / `InProgress` / `Rejected`. The structured
    /// why lives in `Requirement::attention_reason`.
    /// trace:STORY-332 | ai:claude
    NeedsAttention,
}

impl fmt::Display for RequirementStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequirementStatus::Draft => write!(f, "Draft"),
            RequirementStatus::Approved => write!(f, "Approved"),
            RequirementStatus::Planned => write!(f, "Planned"),
            RequirementStatus::InProgress => write!(f, "In Progress"),
            RequirementStatus::Done => write!(f, "Done"),
            RequirementStatus::Completed => write!(f, "Completed"),
            RequirementStatus::Rejected => write!(f, "Rejected"),
            RequirementStatus::NeedsAttention => write!(f, "Needs Attention"),
        }
    }
}

impl RequirementStatus {
    /// Parse one of the eight canonical statuses from a user-typed string,
    /// tolerant of casing and `-`/`_`/space word-breaks (e.g. "in-progress",
    /// "In Progress", "InProgress" all map to `InProgress`). Returns `None`
    /// for anything that isn't a recognized status — the caller decides
    /// whether that's an error (positional shortcut) or a custom status.
    ///
    /// This is the single recognizer behind `aida list`'s status shortcuts and
    /// the `open`/`closed` alias expansion, so "what is a valid status" stays
    /// defined in one place. trace:TASK-0415
    pub fn from_filter_str(s: &str) -> Option<RequirementStatus> {
        match Self::normalize_token(s).as_str() {
            "draft" => Some(RequirementStatus::Draft),
            "approved" => Some(RequirementStatus::Approved),
            "planned" => Some(RequirementStatus::Planned),
            "inprogress" => Some(RequirementStatus::InProgress),
            "done" => Some(RequirementStatus::Done),
            "completed" => Some(RequirementStatus::Completed),
            "rejected" => Some(RequirementStatus::Rejected),
            "needsattention" => Some(RequirementStatus::NeedsAttention),
            _ => None,
        }
    }

    /// Lowercase + drop space/hyphen/underscore so casing and word-break
    /// variants collapse to one canonical comparison key. trace:TASK-0415
    fn normalize_token(s: &str) -> String {
        s.chars()
            .filter_map(|c| match c {
                ' ' | '-' | '_' => None,
                c if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
                c => Some(c),
            })
            .collect()
    }

    /// The canonical no-space variant name as stored in the cache (Debug form,
    /// e.g. "InProgress", "NeedsAttention"). This is what the `status` column
    /// holds, so it's the value a status filter must compare against.
    /// trace:TASK-0415
    pub fn cache_key(&self) -> &'static str {
        match self {
            RequirementStatus::Draft => "Draft",
            RequirementStatus::Approved => "Approved",
            RequirementStatus::Planned => "Planned",
            RequirementStatus::InProgress => "InProgress",
            RequirementStatus::Done => "Done",
            RequirementStatus::Completed => "Completed",
            RequirementStatus::Rejected => "Rejected",
            RequirementStatus::NeedsAttention => "NeedsAttention",
        }
    }

    /// The non-terminal ("open") lifecycle statuses: a spec still in flight.
    /// The `open` list alias expands to these. trace:TASK-0415
    pub fn open_statuses() -> [RequirementStatus; 5] {
        [
            RequirementStatus::Draft,
            RequirementStatus::Approved,
            RequirementStatus::Planned,
            RequirementStatus::InProgress,
            RequirementStatus::NeedsAttention,
        ]
    }

    /// The terminal ("closed") lifecycle statuses: a spec that is finished or
    /// abandoned. The `closed` list alias expands to these. `Done` is terminal
    /// here (work finished on a branch) — it sits with Completed / Rejected on
    /// the "no longer open" side. trace:TASK-0415
    pub fn closed_statuses() -> [RequirementStatus; 3] {
        [
            RequirementStatus::Done,
            RequirementStatus::Completed,
            RequirementStatus::Rejected,
        ]
    }

    /// Expand a single status filter token into one or more canonical status
    /// cache-keys. Recognizes the `open` / `closed` aliases (case/word-break
    /// tolerant) and the eight canonical statuses. Returns `None` for an
    /// unrecognized token so the caller can produce a clear error.
    /// trace:TASK-0415
    pub fn expand_filter_token(token: &str) -> Option<Vec<&'static str>> {
        match Self::normalize_token(token).as_str() {
            "open" => Some(
                RequirementStatus::open_statuses()
                    .iter()
                    .map(|s| s.cache_key())
                    .collect(),
            ),
            "closed" => Some(
                RequirementStatus::closed_statuses()
                    .iter()
                    .map(|s| s.cache_key())
                    .collect(),
            ),
            _ => RequirementStatus::from_filter_str(token).map(|s| vec![s.cache_key()]),
        }
    }

    /// Expand a comma-separated status filter spec (`"open"`, `"draft,approved"`,
    /// `"closed"`) into the deduplicated set of canonical status cache-keys it
    /// matches. Each comma-separated token is OR'd together; aliases expand
    /// in place. Returns `Err(token)` naming the first unrecognized token so
    /// the caller can produce a clear, actionable error. Empty/blank tokens
    /// are skipped. trace:TASK-0415
    pub fn expand_filter_spec(spec: &str) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = Vec::new();
        for raw in spec.split(',') {
            let token = raw.trim();
            if token.is_empty() {
                continue;
            }
            match Self::expand_filter_token(token) {
                Some(keys) => {
                    for k in keys {
                        let k = k.to_string();
                        if !out.contains(&k) {
                            out.push(k);
                        }
                    }
                }
                None => return Err(token.to_string()),
            }
        }
        Ok(out)
    }
}

/// The kind of obstacle that triggered a punt — the machine-readable category
/// `aida punt` / `/aida-punt` records on a [`RequirementStatus::NeedsAttention`]
/// spec (STORY-332).
///
/// This taxonomy is deliberately **obstacle-type**, not escalation-reason.
/// Punt-time categories must be *observable facts* an agent can pick
/// honestly at the moment it gets stuck — "what kind of obstacle is this" —
/// not competence self-judgments. The escalation-reason taxonomy
/// (`lack-of-synthesis`, `unrecorded-preference`, `needs-human-judgment`, …)
/// is STORY-325's *ledger-derived, advisor-reviewed* layer; it is computed
/// from punt history, never asserted at punt time. Do not "improve" this enum
/// into escalation-reason — the two live at different layers on purpose.
///
/// A `BlockedDependency` punt is a direct signal to file a blocked-by
/// relationship (STORY-333): the punt feeds the graph that prevents the next.
/// trace:STORY-332 | ai:claude
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "kebab-case")]
pub enum PuntCategory {
    /// Multiple genuinely-valid designs; the spec does not say which.
    DesignFork,
    /// The spec itself is unclear or self-contradictory.
    AmbiguousSpec,
    /// Needs information or context the agent does not have.
    MissingContext,
    /// Cannot proceed — depends on work that is not done.
    BlockedDependency,
    /// Catch-all for an obstacle that fits none of the above.
    Other,
}

impl fmt::Display for PuntCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PuntCategory::DesignFork => write!(f, "design-fork"),
            PuntCategory::AmbiguousSpec => write!(f, "ambiguous-spec"),
            PuntCategory::MissingContext => write!(f, "missing-context"),
            PuntCategory::BlockedDependency => write!(f, "blocked-dependency"),
            PuntCategory::Other => write!(f, "other"),
        }
    }
}

impl PuntCategory {
    /// Parse a category from its kebab-case CLI form. Tolerant of casing,
    /// surrounding whitespace, and `_`/` ` separators.
    // why: inherent parser returns Option<Self> (infallible-ish, no error type) — std::str::FromStr would force a Result + Err type we don't have.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        let normalized: String = s
            .trim()
            .chars()
            .map(|c| match c {
                ' ' | '-' | '_' => '-',
                c if c.is_ascii_alphabetic() => c.to_ascii_lowercase(),
                _ => c,
            })
            .collect();
        match normalized.as_str() {
            "design-fork" => Some(PuntCategory::DesignFork),
            "ambiguous-spec" => Some(PuntCategory::AmbiguousSpec),
            "missing-context" => Some(PuntCategory::MissingContext),
            "blocked-dependency" => Some(PuntCategory::BlockedDependency),
            "other" => Some(PuntCategory::Other),
            _ => None,
        }
    }

    /// Every category, in declaration order — for help text and validation.
    pub fn all() -> [PuntCategory; 5] {
        [
            PuntCategory::DesignFork,
            PuntCategory::AmbiguousSpec,
            PuntCategory::MissingContext,
            PuntCategory::BlockedDependency,
            PuntCategory::Other,
        ]
    }
}

/// Why a spec is paused in [`RequirementStatus::NeedsAttention`] — the
/// structured record `aida punt` writes onto the spec (STORY-332).
///
/// `detail` is named `detail`, not `comment`, to avoid overloading AIDA's
/// existing spec comments. `lean` is kept distinct from `detail` so the punt
/// ledger (STORY-325) can separate *the fork* (`detail`) from *the agent's
/// best guess if forced to choose* (`lean`) — the recoverable-punt signal.
/// Deliberately carries no `escalation_reason`: that is STORY-325's derived,
/// advisor-reviewed layer, not punt-time data.
/// trace:STORY-332 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct AttentionReason {
    /// The kind of obstacle that triggered the punt.
    pub category: PuntCategory,
    /// Human-readable description of the fork / obstacle the agent hit.
    pub detail: String,
    /// The raiser's best-guess answer if forced to pick — distinct from
    /// `detail` so the fork and the lean stay separable. `None` when the
    /// agent had no defensible lean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean: Option<String>,
    /// Role / agent that raised the punt (the active session role, or
    /// `None` when no role context was available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raised_by: Option<String>,
    /// When the punt was raised.
    pub raised_at: DateTime<Utc>,
}

/// Why a spec was shelved by the `--auto-complete` orchestrator after a
/// phase failure — the structured record that lets `aida findings list`
/// surface the failure with the same triage affordances as a punt.
///
/// A sibling of [`AttentionReason`]: both inhabit `NeedsAttention` and both
/// answer "why is this spec currently paused", but they record different
/// causes. `AttentionReason` is the agent-raised obstacle (a design-fork
/// punt); `FailureReason` is the orchestrator-raised phase failure. They
/// stay distinct because [`PuntCategory`] is documented as
/// **obstacle-shape, not failure-shape** — widening it to carry phase
/// failures would break that invariant and pollute STORY-325's punt
/// frequency analysis. trace:EPIC-28 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct FailureReason {
    /// Stable slug for the phase that failed — `implementer`, `ci`,
    /// `review`, `merge`, `pull`, or `build`. A string (not an enum)
    /// keeps `aida-core` free of orchestrator concepts: the source of
    /// truth for phase identities lives in `aida-cli::auto_complete`.
    pub phase: String,
    /// 1-based phase index (1..=6) for sorting / display.
    pub phase_index: u8,
    /// Stable slug for the failure kind — `no-pr`, `ci-red`,
    /// `request-changes`, `merge-conflict`, etc. Mirrors the orchestrator's
    /// existing `PhaseFailureKind::slug()`.
    pub kind: String,
    /// One-line human description of what went wrong.
    pub detail: String,
    /// Pre-rendered recovery hint — what the orchestrator would tell a
    /// human standing in front of the broken phase. Cached here so
    /// triage doesn't have to rebuild it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_hint: Option<String>,
    /// Role / agent that shelved the spec (the active session role,
    /// `None` when no role context was available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shelved_by: Option<String>,
    /// When the spec was shelved.
    pub shelved_at: DateTime<Utc>,
}

/// Validate a status transition against the STORY-332 NeedsAttention rules.
///
/// Returns `Some(error message)` when the transition is forbidden, `None`
/// when it is allowed. Only the two edges that *touch* `NeedsAttention` are
/// constrained — every other transition returns `None`, so AIDA's otherwise
/// free-form status edits are not regressed:
///   - **into** `NeedsAttention`: only from `InProgress` (via `aida punt`);
///   - **out of** `NeedsAttention`: only to `Approved` / `InProgress` /
///     `Rejected` (triage outcomes).
///     trace:STORY-332 | ai:claude
pub fn forbidden_attention_transition(
    from: &RequirementStatus,
    to: &RequirementStatus,
) -> Option<String> {
    use RequirementStatus::*;
    match (from, to) {
        // No-op stays allowed.
        (NeedsAttention, NeedsAttention) => None,
        // Entering NeedsAttention.
        (_, NeedsAttention) if !matches!(from, InProgress) => Some(
            "a spec can only enter Needs Attention from In Progress \
             (an autonomous agent hits a design-fork mid-work) — \
             use `aida punt` to do this"
                .to_string(),
        ),
        // Leaving NeedsAttention.
        (NeedsAttention, to) if !matches!(to, Approved | InProgress | Rejected) => Some(
            "a Needs Attention spec can only be triaged to Approved, \
             In Progress, or Rejected"
                .to_string(),
        ),
        _ => None,
    }
}

/// One enumerated answer to a [`DecisionRequest`].
///
/// Each choice carries a human-readable `label` and `consequence` plus a
/// machine-applicable `resolution` *token* — a deterministic action string
/// (e.g. `"status:rejected"`, `"status:approved;tag:+ready-to-implement"`,
/// `"tag:+deferred:post-stability"`, `"noop"`). The HARD RULE for STORY-522:
/// every choice must encode a deterministic resolution — never free-form;
/// un-reducible forks stay advisor-tier escalations, not questions.
///
/// Slice 1 (this code) RECORDS the resolution token. The loop-resume
/// auto-applier that parses + applies the token is DEFERRED per the operator
/// decision (it couples to the orchestrator and carries the design risk).
// trace:STORY-522 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct DecisionChoice {
    /// Short human label for the option (what the human picks).
    pub label: String,
    /// What happens / what this option means, in human terms.
    pub consequence: String,
    /// Deterministic action token the loop will eventually apply (NOT
    /// applied in slice 1). Never free-form prose — a parseable token.
    pub resolution: String,
}

/// A structured, persisted decision the human answers OUTSIDE any agent.
///
/// When the advisor can't resolve a fork, it distills the fork into a
/// self-contained question + enumerated choices and records it on the spec.
/// The human then batch-answers via plain CLI (`aida questions answer`) — a
/// pure data op, no LLM session — flipping the request from pending to
/// answered. A future drain pass reads the answered choice and applies its
/// resolution token (DEFERRED — slice 1 only records the answer).
// trace:STORY-522 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct DecisionRequest {
    /// The self-contained question (state / deciding factor distilled so the
    /// human needs no spec re-read to answer).
    pub question: String,
    /// Enumerated, actionable choices (≥2). Each maps to a resolution token.
    pub choices: Vec<DecisionChoice>,
    /// 0-based index of the recommended default choice, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<usize>,
    /// Why the recommended default is recommended (the advisor's reasoning).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// 0-based index of the chosen answer. `None` while the request is
    /// pending (unanswered).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered: Option<usize>,
    /// When the question was first posed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<DateTime<Utc>>,
    /// When the human answered it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<DateTime<Utc>>,
}

impl DecisionRequest {
    /// A request is pending until it has been answered.
    pub fn is_pending(&self) -> bool {
        self.answered.is_none()
    }
}

/// Represents the priority of a requirement
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
pub enum RequirementPriority {
    High,
    Medium,
    Low,
}

impl fmt::Display for RequirementPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequirementPriority::High => write!(f, "High"),
            RequirementPriority::Medium => write!(f, "Medium"),
            RequirementPriority::Low => write!(f, "Low"),
        }
    }
}

/// Represents the type of a requirement
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
pub enum RequirementType {
    // Traditional requirements types
    Functional,
    NonFunctional,
    System,
    User,
    ChangeRequest,
    Bug,
    // Agile types
    Epic,
    Story,
    Task,
    Spike,
    Sprint, // Time-boxed iteration for work planning
    // Organizational types (stateless)
    Folder,
    // Meta type for database configuration (prompts, skills, etc.)
    Meta,
    // Documentation-layer types (stateless or low-state, drive aida-docs
    // projection of the graph into a layered docs tree).
    // trace:FR-1-074 | ai:claude
    /// Constitution clause — non-negotiable principle that governs how the
    /// project is built. Stateless (always active until explicitly retired).
    Principle,
    /// Vision / target outcome — what we're building, for whom, by when.
    /// Stateful (active / achieved / abandoned).
    Vision,
    /// External or technical constraint — regulation, dependency, deadline.
    /// Stateful (active / lifted).
    Constraint,
    /// Architecture Decision Record (ADR) — a recorded decision + its
    /// rationale. Stateful (proposed / accepted / superseded / deprecated).
    Decision,
    /// Glossary term — domain language entry, ubiquitous-language anchor.
    /// Stateless.
    Term,
    /// Living-documentation entry — narrative captured during work (rationale,
    /// scenarios, recipes, gotchas) and linked back to the specs it explains
    /// via the `relationships` field. Distinct from `Decision` (a single
    /// recorded choice + its rationale) and `Term` (glossary definition):
    /// `Doc` is generic explanatory prose that powers the EPIC-24 book/tutorial
    /// projection. trace:STORY-104 | ai:claude
    Doc,
}

impl fmt::Display for RequirementType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequirementType::Functional => write!(f, "Functional"),
            RequirementType::NonFunctional => write!(f, "Non-Functional"),
            RequirementType::System => write!(f, "System"),
            RequirementType::User => write!(f, "User"),
            RequirementType::ChangeRequest => write!(f, "Change Request"),
            RequirementType::Bug => write!(f, "Bug"),
            RequirementType::Epic => write!(f, "Epic"),
            RequirementType::Story => write!(f, "Story"),
            RequirementType::Task => write!(f, "Task"),
            RequirementType::Spike => write!(f, "Spike"),
            RequirementType::Sprint => write!(f, "Sprint"),
            RequirementType::Folder => write!(f, "Folder"),
            RequirementType::Meta => write!(f, "Meta"),
            RequirementType::Principle => write!(f, "Principle"),
            RequirementType::Vision => write!(f, "Vision"),
            RequirementType::Constraint => write!(f, "Constraint"),
            RequirementType::Decision => write!(f, "Decision"),
            RequirementType::Term => write!(f, "Term"),
            RequirementType::Doc => write!(f, "Doc"),
        }
    }
}

impl RequirementType {
    /// The built-in short prefix for this type, used in agreed-id format
    /// (`<PREFIX>-<SEQ>`) and as the default block-allocation key. Stateless
    /// — does not consult any per-store override. For per-store config-aware
    /// resolution, use `RequirementsStore::get_type_prefix`.
    /// trace:FR-1-074 | ai:claude
    pub fn default_prefix(&self) -> &'static str {
        match self {
            RequirementType::Functional => "FR",
            RequirementType::NonFunctional => "NFR",
            RequirementType::System => "SR",
            RequirementType::User => "UR",
            RequirementType::ChangeRequest => "CR",
            RequirementType::Bug => "BUG",
            RequirementType::Epic => "EPIC",
            RequirementType::Story => "STORY",
            RequirementType::Task => "TASK",
            RequirementType::Spike => "SPIKE",
            RequirementType::Sprint => "SPRINT",
            RequirementType::Folder => "FOLDER",
            RequirementType::Meta => "META",
            RequirementType::Principle => "PRIN",
            RequirementType::Vision => "VIS",
            RequirementType::Constraint => "CON",
            RequirementType::Decision => "ADR",
            RequirementType::Term => "TERM",
            RequirementType::Doc => "DOC",
        }
    }
}

/// Represents the subtype for Meta requirements
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
pub enum MetaSubtype {
    /// AI prompts (evaluate, improve, etc.)
    Prompt,
    /// Skill definitions
    Skill,
    /// Slash commands
    Command,
    /// Other templates
    Template,
    /// Database configuration
    Config,
}

impl fmt::Display for MetaSubtype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetaSubtype::Prompt => write!(f, "Prompt"),
            MetaSubtype::Skill => write!(f, "Skill"),
            MetaSubtype::Command => write!(f, "Command"),
            MetaSubtype::Template => write!(f, "Template"),
            MetaSubtype::Config => write!(f, "Config"),
        }
    }
}

impl MetaSubtype {
    /// Parse a meta subtype from a string
    // why: inherent parser returns Option<Self>; std FromStr would force a Result + Err type.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "prompt" => Some(MetaSubtype::Prompt),
            "skill" => Some(MetaSubtype::Skill),
            "command" => Some(MetaSubtype::Command),
            "template" => Some(MetaSubtype::Template),
            "config" => Some(MetaSubtype::Config),
            _ => None,
        }
    }

    /// Get all meta subtypes
    pub fn all() -> Vec<MetaSubtype> {
        vec![
            MetaSubtype::Prompt,
            MetaSubtype::Skill,
            MetaSubtype::Command,
            MetaSubtype::Template,
            MetaSubtype::Config,
        ]
    }
}

/// Represents a relationship type between requirements
///
/// BUG-251: `Deserialize` is hand-written (not derived) for forward-compat
/// across binary version skew. The derived impl errors hard on an unknown
/// unit variant (e.g. an older binary reading a newer `BlockedBy`), which
/// produced noisy parse failures on every machine trailing a format change.
/// The manual impl routes any unknown variant to `Custom(name)` instead.
/// `Serialize` stays derived so the on-disk bytes are unchanged.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash, TS)]
pub enum RelationshipType {
    // TASK-679: the stored convention — verified empirically against `aida add
    // --parent` — is "the rel_type names the SOURCE's role relative to the
    // target". So a parent stores `Parent --> child` ("I am the parent of this
    // target") and the child stores the reciprocal `Child --> parent`. Both
    // `aida add --parent` and (since TASK-679) `aida rel add --type parent`
    // write the bidirectional pair. trace:TASK-679 | ai:claude
    /// This requirement is the parent of the target. Stored on the parent as
    /// `parent --Parent--> child`; its reciprocal is `Child` on the child. Walk
    /// OUTGOING `Parent` from an epic/folder to reach its children.
    Parent,
    /// This requirement is a child of the target. Stored on the child as
    /// `child --Child--> parent`; its reciprocal is `Parent` on the parent.
    Child,
    /// Duplicate relationship
    Duplicate,
    /// Verification relationship (this verifies target)
    Verifies,
    /// Verified-by relationship (this is verified by target)
    VerifiedBy,
    /// General reference relationship
    References,
    /// This requirement is blocked by the target — a hard dependency.
    /// The blocked spec is un-pickable until the blocker reaches
    /// `Completed`; if the blocker is `Rejected`, the block is permanent
    /// and needs re-scoping. Consumed by `pickability` for the pre-pickup
    /// gate. trace:STORY-333 | ai:claude
    BlockedBy,
    /// This requirement blocks the target (inverse of `BlockedBy`).
    /// trace:STORY-333 | ai:claude
    Blocks,
    /// Custom relationship type with user-defined name
    Custom(String),
}

impl fmt::Display for RelationshipType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RelationshipType::Parent => write!(f, "parent"),
            RelationshipType::Child => write!(f, "child"),
            RelationshipType::Duplicate => write!(f, "duplicate"),
            RelationshipType::Verifies => write!(f, "verifies"),
            RelationshipType::VerifiedBy => write!(f, "verified-by"),
            RelationshipType::References => write!(f, "references"),
            RelationshipType::BlockedBy => write!(f, "blocked-by"),
            RelationshipType::Blocks => write!(f, "blocks"),
            RelationshipType::Custom(name) => write!(f, "{}", name),
        }
    }
}

/// BUG-251: forward-compatible deserialization. An unknown variant — a newer
/// binary's addition read by an older one — lands in `Custom(name)` rather
/// than failing the whole spec parse. Handles all three wire shapes the
/// derived `Serialize` can emit (confirmed empirically):
///   - bare string `Parent` / `BlockedBy` / future names  → `visit_str`
///   - YAML externally-tagged `!Custom foo`                → `visit_enum`
///   - JSON externally-tagged `{"Custom":"foo"}`           → `visit_map`
///     `from_str` lowercases, so the stored PascalCase variant names round-trip,
///     and unknown names fall through to `Custom`. trace:BUG-251 | ai:claude
impl<'de> Deserialize<'de> for RelationshipType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RelTypeVisitor;

        impl<'de> serde::de::Visitor<'de> for RelTypeVisitor {
            type Value = RelationshipType;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "a relationship-type string, a `!Custom <name>` tag, or a `{Custom: <name>}` map",
                )
            }

            fn visit_str<E>(self, value: &str) -> Result<RelationshipType, E>
            where
                E: serde::de::Error,
            {
                Ok(RelationshipType::from_str(value))
            }

            fn visit_string<E>(self, value: String) -> Result<RelationshipType, E>
            where
                E: serde::de::Error,
            {
                Ok(RelationshipType::from_str(&value))
            }

            // YAML externally-tagged form: `!Custom foo`. `Custom` is the only
            // newtype variant; any newtype-shaped value carries its name in the
            // payload, so an unknown future newtype still preserves the payload.
            fn visit_enum<A>(self, data: A) -> Result<RelationshipType, A::Error>
            where
                A: serde::de::EnumAccess<'de>,
            {
                use serde::de::VariantAccess;
                let (_name, variant) = data.variant::<String>()?;
                let payload: String = variant.newtype_variant()?;
                Ok(RelationshipType::Custom(payload))
            }

            // JSON externally-tagged form: `{"Custom":"foo"}`.
            fn visit_map<A>(self, mut map: A) -> Result<RelationshipType, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let key: Option<String> = map.next_key()?;
                let Some(_key) = key else {
                    return Err(serde::de::Error::custom(
                        "empty map is not a valid relationship type",
                    ));
                };
                let value: String = map.next_value()?;
                Ok(RelationshipType::Custom(value))
            }
        }

        deserializer.deserialize_any(RelTypeVisitor)
    }
}

impl RelationshipType {
    /// Parse a relationship type from a string
    // why: inherent parser is infallible (returns Self with a default fallthrough); std FromStr requires a fallible Result signature.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "parent" => RelationshipType::Parent,
            "child" => RelationshipType::Child,
            "duplicate" => RelationshipType::Duplicate,
            "verifies" => RelationshipType::Verifies,
            "verified-by" | "verified_by" | "verifiedby" => RelationshipType::VerifiedBy,
            "references" => RelationshipType::References,
            "blocked-by" | "blocked_by" | "blockedby" => RelationshipType::BlockedBy,
            "blocks" => RelationshipType::Blocks,
            _ => RelationshipType::Custom(s.to_string()),
        }
    }

    /// Get the inverse relationship type (if applicable)
    pub fn inverse(&self) -> Option<Self> {
        match self {
            RelationshipType::Parent => Some(RelationshipType::Child),
            RelationshipType::Child => Some(RelationshipType::Parent),
            RelationshipType::Verifies => Some(RelationshipType::VerifiedBy),
            RelationshipType::VerifiedBy => Some(RelationshipType::Verifies),
            RelationshipType::Duplicate => Some(RelationshipType::Duplicate),
            RelationshipType::BlockedBy => Some(RelationshipType::Blocks),
            RelationshipType::Blocks => Some(RelationshipType::BlockedBy),
            RelationshipType::References => None,
            RelationshipType::Custom(_) => None,
        }
    }

    /// Get the canonical name for this relationship type
    pub fn name(&self) -> String {
        match self {
            RelationshipType::Parent => "parent".to_string(),
            RelationshipType::Child => "child".to_string(),
            RelationshipType::Duplicate => "duplicate".to_string(),
            RelationshipType::Verifies => "verifies".to_string(),
            RelationshipType::VerifiedBy => "verified_by".to_string(),
            RelationshipType::References => "references".to_string(),
            RelationshipType::BlockedBy => "blocked_by".to_string(),
            RelationshipType::Blocks => "blocks".to_string(),
            RelationshipType::Custom(name) => name.clone(),
        }
    }
}

// ============================================================================
// Custom Type Definition System
// ============================================================================

/// Field type for custom fields
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum CustomFieldType {
    /// Single-line text input
    #[default]
    Text,
    /// Multi-line text input
    TextArea,
    /// Selection from predefined options
    Select,
    /// Boolean checkbox
    Boolean,
    /// Date value
    Date,
    /// Reference to a user ($USER-XXX)
    User,
    /// Reference to another requirement (SPEC-XXX)
    Requirement,
    /// Numeric value
    Number,
}

impl fmt::Display for CustomFieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CustomFieldType::Text => write!(f, "Text"),
            CustomFieldType::TextArea => write!(f, "Text Area"),
            CustomFieldType::Select => write!(f, "Select"),
            CustomFieldType::Boolean => write!(f, "Boolean"),
            CustomFieldType::Date => write!(f, "Date"),
            CustomFieldType::User => write!(f, "User Reference"),
            CustomFieldType::Requirement => write!(f, "Requirement Reference"),
            CustomFieldType::Number => write!(f, "Number"),
        }
    }
}

/// Definition of a custom field for a requirement type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct CustomFieldDefinition {
    /// Field name (used as key in custom_fields map)
    pub name: String,

    /// Display label for the field
    pub label: String,

    /// Field type
    #[serde(default)]
    pub field_type: CustomFieldType,

    /// Whether this field is required
    #[serde(default)]
    pub required: bool,

    /// Options for Select field type
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,

    /// Default value (as string, converted based on field_type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,

    /// Help text / description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Display order (lower = first)
    #[serde(default)]
    pub order: i32,
}

impl CustomFieldDefinition {
    /// Creates a new text field definition
    pub fn text(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            field_type: CustomFieldType::Text,
            required: false,
            options: Vec::new(),
            default_value: None,
            description: None,
            order: 0,
        }
    }

    /// Creates a new select field definition
    pub fn select(name: impl Into<String>, label: impl Into<String>, options: Vec<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            field_type: CustomFieldType::Select,
            required: false,
            options,
            default_value: None,
            description: None,
            order: 0,
        }
    }

    /// Creates a new user reference field definition
    pub fn user_ref(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            field_type: CustomFieldType::User,
            required: false,
            options: Vec::new(),
            default_value: None,
            description: None,
            order: 0,
        }
    }

    /// Creates a new text area (multiline text) field definition
    pub fn textarea(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            field_type: CustomFieldType::TextArea,
            required: false,
            options: Vec::new(),
            default_value: None,
            description: None,
            order: 0,
        }
    }

    /// Creates a new number field definition
    pub fn number(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            field_type: CustomFieldType::Number,
            required: false,
            options: Vec::new(),
            default_value: None,
            description: None,
            order: 0,
        }
    }

    /// Sets the field as required
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Sets the description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Sets the display order
    pub fn with_order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// Sets a default value
    pub fn with_default(mut self, value: impl Into<String>) -> Self {
        self.default_value = Some(value.into());
        self
    }
}

/// Definition of a custom requirement type with its specific statuses and fields
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct CustomTypeDefinition {
    /// Internal name/key for the type (e.g., "ChangeRequest")
    pub name: String,

    /// Display label (e.g., "Change Request")
    pub display_name: String,

    /// Description of this type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Preferred ID prefix for this type (e.g., "CR")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    /// Custom statuses for this type (if empty, uses default statuses)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statuses: Vec<String>,

    /// Custom priorities for this type (if empty, uses default priorities)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub priorities: Vec<String>,

    /// Additional custom fields for this type
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_fields: Vec<CustomFieldDefinition>,

    /// Whether this is a built-in type (cannot be deleted)
    #[serde(default)]
    pub built_in: bool,

    /// Color for visual distinction (hex color code)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,

    /// Whether this type is stateless (no status/priority tracking)
    /// Stateless types are used for organizational purposes (e.g., Folders)
    /// They are excluded from status metrics and reports by default
    #[serde(default)]
    pub stateless: bool,
}

impl CustomTypeDefinition {
    /// Creates a new custom type definition
    pub fn new(name: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            display_name: display_name.into(),
            description: None,
            prefix: None,
            statuses: Vec::new(),
            priorities: Vec::new(),
            custom_fields: Vec::new(),
            built_in: false,
            color: None,
            stateless: false,
        }
    }

    /// Creates a built-in type definition
    pub fn built_in(name: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            display_name: display_name.into(),
            description: None,
            prefix: None,
            statuses: Vec::new(),
            priorities: Vec::new(),
            custom_fields: Vec::new(),
            built_in: true,
            color: None,
            stateless: false,
        }
    }

    /// Creates a built-in stateless type definition (no status/priority tracking)
    pub fn built_in_stateless(name: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            display_name: display_name.into(),
            description: None,
            prefix: None,
            statuses: Vec::new(),
            priorities: Vec::new(),
            custom_fields: Vec::new(),
            built_in: true,
            color: None,
            stateless: true,
        }
    }

    /// Sets the prefix
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Sets custom statuses
    pub fn with_statuses(mut self, statuses: Vec<&str>) -> Self {
        self.statuses = statuses.into_iter().map(String::from).collect();
        self
    }

    /// Sets custom priorities
    pub fn with_priorities(mut self, priorities: Vec<&str>) -> Self {
        self.priorities = priorities.into_iter().map(String::from).collect();
        self
    }

    /// Adds a custom field
    pub fn with_field(mut self, field: CustomFieldDefinition) -> Self {
        self.custom_fields.push(field);
        self
    }

    /// Sets the description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Sets the color
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Marks this type as stateless (no status/priority tracking)
    pub fn as_stateless(mut self) -> Self {
        self.stateless = true;
        self
    }

    /// Gets the statuses for this type, falling back to defaults if none specified
    pub fn get_statuses(&self) -> Vec<String> {
        if self.statuses.is_empty() {
            // Default statuses
            vec![
                "Draft".to_string(),
                "Approved".to_string(),
                "Completed".to_string(),
                "Rejected".to_string(),
            ]
        } else {
            self.statuses.clone()
        }
    }

    /// Gets the priorities for this type, falling back to defaults if none specified
    pub fn get_priorities(&self) -> Vec<String> {
        if self.priorities.is_empty() {
            // Default priorities
            vec!["High".to_string(), "Medium".to_string(), "Low".to_string()]
        } else {
            self.priorities.clone()
        }
    }
}

/// Returns the default type definitions
pub fn default_type_definitions() -> Vec<CustomTypeDefinition> {
    vec![
        CustomTypeDefinition::built_in("Functional", "Functional")
            .with_prefix("FR")
            .with_description("Functional requirements describing system behavior"),
        CustomTypeDefinition::built_in("NonFunctional", "Non-Functional")
            .with_prefix("NFR")
            .with_description("Non-functional requirements (performance, security, etc.)"),
        CustomTypeDefinition::built_in("System", "System")
            .with_prefix("SYS")
            .with_description("System-level requirements"),
        CustomTypeDefinition::built_in("User", "User Story")
            .with_prefix("US")
            .with_description("User stories and user requirements"),
        CustomTypeDefinition::built_in("ChangeRequest", "Change Request")
            .with_prefix("CR")
            .with_description("Change requests for existing functionality")
            .with_statuses(vec![
                "Draft",
                "Submitted",
                "Under Review",
                "Approved",
                "Rejected",
                "In Progress",
                "Implemented",
                "Verified",
                "Closed",
            ])
            .with_color("#9333ea")
            .with_field(
                CustomFieldDefinition::select(
                    "impact",
                    "Impact Level",
                    vec![
                        "Low".to_string(),
                        "Medium".to_string(),
                        "High".to_string(),
                        "Critical".to_string(),
                    ],
                )
                .with_description("Impact of this change on the system")
                .with_order(1),
            )
            .with_field(
                CustomFieldDefinition::user_ref("requested_by", "Requested By")
                    .with_description("User who requested this change")
                    .with_order(2),
            )
            .with_field(
                CustomFieldDefinition::text("target_release", "Target Release")
                    .with_description("Target release version for this change")
                    .with_order(3),
            )
            .with_field(
                CustomFieldDefinition::text("justification", "Justification")
                    .required()
                    .with_description("Business justification for the change")
                    .with_order(4),
            ),
        // Bug tracking
        CustomTypeDefinition::built_in("Bug", "Bug")
            .with_prefix("BUG")
            .with_description("Bug reports and defects")
            .with_statuses(vec![
                "New",
                "Confirmed",
                "In Progress",
                "Fixed",
                "Verified",
                "Closed",
                "Won't Fix",
            ])
            .with_color("#dc2626")
            .with_field(
                CustomFieldDefinition::select(
                    "severity",
                    "Severity",
                    vec![
                        "Critical".to_string(),
                        "Major".to_string(),
                        "Minor".to_string(),
                        "Trivial".to_string(),
                    ],
                )
                .with_description("Severity of the bug")
                .with_order(1),
            )
            .with_field(
                CustomFieldDefinition::text("steps_to_reproduce", "Steps to Reproduce")
                    .with_description("Steps to reproduce the bug")
                    .with_order(2),
            )
            .with_field(
                CustomFieldDefinition::text("expected_behavior", "Expected Behavior")
                    .with_description("What should happen")
                    .with_order(3),
            )
            .with_field(
                CustomFieldDefinition::text("actual_behavior", "Actual Behavior")
                    .with_description("What actually happens")
                    .with_order(4),
            ),
        // Agile types
        CustomTypeDefinition::built_in("Epic", "Epic")
            .with_prefix("EPIC")
            .with_description("Large feature or initiative spanning multiple stories")
            .with_statuses(vec!["Draft", "Ready", "In Progress", "Done"])
            .with_color("#7c3aed")
            .with_field(
                CustomFieldDefinition::text("business_value", "Business Value")
                    .with_description("Business value or benefit of this epic")
                    .with_order(1),
            )
            .with_field(
                CustomFieldDefinition::text("target_release", "Target Release")
                    .with_description("Target release or milestone")
                    .with_order(2),
            )
            .with_field(
                CustomFieldDefinition::number("story_points", "Story Points")
                    .with_description("Estimated story points")
                    .with_order(3),
            ),
        // trace:FR-0309 | ai:claude:high
        CustomTypeDefinition::built_in("Story", "Story")
            .with_prefix("STORY")
            .with_description("User story for agile development")
            .with_statuses(vec!["Draft", "Ready", "In Progress", "In Review", "Done"])
            .with_color("#10b981")
            .with_field(
                CustomFieldDefinition::textarea("acceptance_criteria", "Acceptance Criteria")
                    .with_description("Criteria that must be met for the story to be complete")
                    .with_order(1),
            )
            .with_field(
                CustomFieldDefinition::number("story_points", "Story Points")
                    .with_description("Estimated story points")
                    .with_order(2),
            )
            .with_field(
                CustomFieldDefinition::user_ref("assignee", "Assignee")
                    .with_description("Person assigned to this story")
                    .with_order(3),
            ),
        CustomTypeDefinition::built_in("Task", "Task")
            .with_prefix("TASK")
            .with_description("Implementation task or work item")
            .with_statuses(vec!["To Do", "In Progress", "In Review", "Done"])
            .with_color("#0891b2")
            .with_field(
                CustomFieldDefinition::number("estimate_hours", "Estimate (hours)")
                    .with_description("Estimated hours to complete")
                    .with_order(1),
            )
            .with_field(
                CustomFieldDefinition::user_ref("assignee", "Assignee")
                    .with_description("Person assigned to this task")
                    .with_order(2),
            ),
        CustomTypeDefinition::built_in("Spike", "Spike")
            .with_prefix("SPIKE")
            .with_description("Research or investigation task with time-boxed exploration")
            .with_statuses(vec!["Planned", "In Progress", "Completed"])
            .with_color("#ca8a04")
            .with_field(
                CustomFieldDefinition::text("research_question", "Research Question")
                    .required()
                    .with_description("The question or problem to investigate")
                    .with_order(1),
            )
            .with_field(
                CustomFieldDefinition::text("time_box", "Time Box")
                    .with_description("Time allocated for investigation (e.g., '2 days')")
                    .with_order(2),
            )
            .with_field(
                CustomFieldDefinition::textarea("findings", "Findings")
                    .with_description("Results and conclusions from the investigation")
                    .with_order(3),
            )
            .with_field(
                CustomFieldDefinition::text("recommendation", "Recommendation")
                    .with_description("Recommended next steps based on findings")
                    .with_order(4),
            ),
        // Sprint type for time-boxed iterations
        CustomTypeDefinition::built_in("Sprint", "Sprint")
            .with_prefix("SPRINT")
            .with_description("Time-boxed iteration for work planning")
            .with_statuses(vec!["Draft", "In Progress", "Completed", "Archived"])
            .with_color("#7c3aed")
            .with_field(
                CustomFieldDefinition::number("sprint_number", "Sprint Number")
                    .with_description("Sequential sprint number")
                    .with_order(1),
            )
            .with_field(
                CustomFieldDefinition::text("start_date", "Start Date")
                    .with_description("Sprint start date (YYYY-MM-DD)")
                    .with_order(2),
            )
            .with_field(
                CustomFieldDefinition::text("end_date", "End Date")
                    .with_description("Sprint end date (YYYY-MM-DD)")
                    .with_order(3),
            )
            .with_field(
                CustomFieldDefinition::text("sprint_goal", "Sprint Goal")
                    .with_description("Goal or theme for this sprint")
                    .with_order(4),
            )
            .with_field(
                CustomFieldDefinition::number("velocity", "Planned Velocity")
                    .with_description("Planned story points for this sprint")
                    .with_order(5),
            ),
        // Stateless organizational types
        CustomTypeDefinition::built_in_stateless("Folder", "Folder")
            .with_prefix("FLD")
            .with_description("Organizational container for grouping related requirements")
            .with_color("#6b7280"),
        // Meta type for database configuration
        CustomTypeDefinition::built_in_stateless("Meta", "Meta")
            .with_prefix("META")
            .with_description("Database configuration, prompts, skills, and templates")
            .with_color("#8b5cf6"),
    ]
}

// ============================================================================
// Relationship Definition System
// ============================================================================

/// Cardinality constraints for relationships
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
pub enum Cardinality {
    /// One source to one target (1:1)
    OneToOne,
    /// One source to many targets (1:N)
    OneToMany,
    /// Many sources to one target (N:1)
    ManyToOne,
    /// Many sources to many targets (N:N) - default
    #[default]
    ManyToMany,
}

impl fmt::Display for Cardinality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cardinality::OneToOne => write!(f, "1:1"),
            Cardinality::OneToMany => write!(f, "1:N"),
            Cardinality::ManyToOne => write!(f, "N:1"),
            Cardinality::ManyToMany => write!(f, "N:N"),
        }
    }
}

impl Cardinality {
    /// Parse cardinality from string
    // why: inherent parser is infallible (returns Self with a default fallthrough); std FromStr requires a fallible Result signature.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().replace(" ", "").as_str() {
            "1:1" | "one_to_one" | "onetoone" => Cardinality::OneToOne,
            "1:n" | "one_to_many" | "onetomany" => Cardinality::OneToMany,
            "n:1" | "many_to_one" | "manytoone" => Cardinality::ManyToOne,
            _ => Cardinality::ManyToMany,
        }
    }
}

/// Defines a relationship type and its constraints
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct RelationshipDefinition {
    /// Unique identifier for this relationship type (lowercase, no spaces)
    pub name: String,

    /// Human-readable display name
    pub display_name: String,

    /// Description of what this relationship means
    #[serde(default)]
    pub description: String,

    /// The inverse relationship name (if any)
    /// e.g., "parent" has inverse "child"
    #[serde(default)]
    pub inverse: Option<String>,

    /// Whether this relationship is symmetric (A->B implies B->A with same type)
    /// e.g., "duplicate" is symmetric
    #[serde(default)]
    pub symmetric: bool,

    /// Cardinality constraints
    #[serde(default)]
    pub cardinality: Cardinality,

    /// Source type constraints (which requirement types can be the source)
    /// Empty means all types allowed
    #[serde(default)]
    pub source_types: Vec<String>,

    /// Target type constraints (which requirement types can be the target)
    /// Empty means all types allowed
    #[serde(default)]
    pub target_types: Vec<String>,

    /// Whether this is a built-in relationship (cannot be deleted)
    #[serde(default)]
    pub built_in: bool,

    /// Color for visualization (optional, hex format e.g., "#ff6b6b")
    #[serde(default)]
    pub color: Option<String>,

    /// Icon/symbol for the relationship (optional)
    #[serde(default)]
    pub icon: Option<String>,
}

impl RelationshipDefinition {
    /// Create a new relationship definition
    pub fn new(name: &str, display_name: &str) -> Self {
        Self {
            name: name.to_lowercase(),
            display_name: display_name.to_string(),
            description: String::new(),
            inverse: None,
            symmetric: false,
            cardinality: Cardinality::ManyToMany,
            source_types: Vec::new(),
            target_types: Vec::new(),
            built_in: false,
            color: None,
            icon: None,
        }
    }

    /// Create a built-in relationship definition
    pub fn built_in(name: &str, display_name: &str, description: &str) -> Self {
        Self {
            name: name.to_lowercase(),
            display_name: display_name.to_string(),
            description: description.to_string(),
            inverse: None,
            symmetric: false,
            cardinality: Cardinality::ManyToMany,
            source_types: Vec::new(),
            target_types: Vec::new(),
            built_in: true,
            color: None,
            icon: None,
        }
    }

    /// Set the inverse relationship
    pub fn with_inverse(mut self, inverse: &str) -> Self {
        self.inverse = Some(inverse.to_lowercase());
        self
    }

    /// Set as symmetric
    pub fn with_symmetric(mut self, symmetric: bool) -> Self {
        self.symmetric = symmetric;
        self
    }

    /// Set the cardinality
    pub fn with_cardinality(mut self, cardinality: Cardinality) -> Self {
        self.cardinality = cardinality;
        self
    }

    /// Set source type constraints
    pub fn with_source_types(mut self, types: Vec<String>) -> Self {
        self.source_types = types;
        self
    }

    /// Set target type constraints
    pub fn with_target_types(mut self, types: Vec<String>) -> Self {
        self.target_types = types;
        self
    }

    /// Set the color
    pub fn with_color(mut self, color: &str) -> Self {
        self.color = Some(color.to_string());
        self
    }

    /// Get the default built-in relationship definitions
    pub fn defaults() -> Vec<RelationshipDefinition> {
        vec![
            // Requirement-to-requirement relationships
            RelationshipDefinition::built_in("parent", "Parent", "Hierarchical parent requirement")
                .with_inverse("child")
                .with_cardinality(Cardinality::OneToMany), // A parent can have many children
            RelationshipDefinition::built_in("child", "Child", "Hierarchical child requirement")
                .with_inverse("parent")
                .with_cardinality(Cardinality::ManyToOne), // Many children can share one parent
            RelationshipDefinition::built_in(
                "verifies",
                "Verifies",
                "Test or verification relationship",
            )
            .with_inverse("verified_by"),
            RelationshipDefinition::built_in(
                "verified_by",
                "Verified By",
                "Verified by a test requirement",
            )
            .with_inverse("verifies"),
            RelationshipDefinition::built_in(
                "duplicate",
                "Duplicate",
                "Marks requirements as duplicates",
            )
            .with_symmetric(true),
            RelationshipDefinition::built_in("references", "References", "General reference link"),
            RelationshipDefinition::built_in("depends_on", "Depends On", "Dependency relationship")
                .with_inverse("dependency_of"),
            RelationshipDefinition::built_in(
                "dependency_of",
                "Dependency Of",
                "Inverse dependency relationship",
            )
            .with_inverse("depends_on"),
            RelationshipDefinition::built_in(
                "implements",
                "Implements",
                "Implementation relationship",
            )
            .with_inverse("implemented_by"),
            RelationshipDefinition::built_in(
                "implemented_by",
                "Implemented By",
                "Inverse implementation relationship",
            )
            .with_inverse("implements"),
            // User-to-requirement relationships
            RelationshipDefinition::built_in(
                "created_by",
                "Created By",
                "User who created the requirement",
            )
            .with_cardinality(Cardinality::ManyToOne)
            .with_color("#4a9eff"),
            RelationshipDefinition::built_in(
                "assigned_to",
                "Assigned To",
                "User assigned to work on this requirement",
            )
            .with_cardinality(Cardinality::ManyToOne)
            .with_color("#22c55e"),
            RelationshipDefinition::built_in(
                "tested_by",
                "Tested By",
                "User who tested/verified the requirement",
            )
            .with_cardinality(Cardinality::ManyToMany)
            .with_color("#f59e0b"),
            RelationshipDefinition::built_in(
                "closed_by",
                "Closed By",
                "User who closed/completed the requirement",
            )
            .with_cardinality(Cardinality::ManyToOne)
            .with_color("#ef4444"),
            // Sprint planning relationships
            RelationshipDefinition::built_in(
                "sprint_assignment",
                "Assigned to Sprint",
                "Assigns a requirement to a Sprint for work planning",
            )
            .with_inverse("sprint_contains")
            .with_cardinality(Cardinality::ManyToOne) // Each item in one Sprint at a time
            .with_target_types(vec!["Sprint".to_string()])
            .with_color("#7c3aed"),
            RelationshipDefinition::built_in(
                "sprint_contains",
                "Sprint Contains",
                "Items assigned to this Sprint",
            )
            .with_inverse("sprint_assignment")
            .with_cardinality(Cardinality::OneToMany)
            .with_source_types(vec!["Sprint".to_string()])
            .with_color("#7c3aed"),
        ]
    }

    /// Check if a source requirement type is allowed
    pub fn allows_source_type(&self, req_type: &RequirementType) -> bool {
        if self.source_types.is_empty() {
            return true;
        }
        let type_str = req_type.to_string();
        self.source_types
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&type_str))
    }

    /// Check if a target requirement type is allowed
    pub fn allows_target_type(&self, req_type: &RequirementType) -> bool {
        if self.target_types.is_empty() {
            return true;
        }
        let type_str = req_type.to_string();
        self.target_types
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&type_str))
    }
}

/// Result of validating a relationship
#[derive(Debug, Clone, TS)]
pub struct RelationshipValidation {
    /// Whether the relationship is valid
    pub valid: bool,
    /// Error messages (if invalid)
    pub errors: Vec<String>,
    /// Warning messages (valid but may have issues)
    pub warnings: Vec<String>,
}

impl RelationshipValidation {
    pub fn ok() -> Self {
        Self {
            valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn error(msg: &str) -> Self {
        Self {
            valid: false,
            errors: vec![msg.to_string()],
            warnings: Vec::new(),
        }
    }

    pub fn with_warning(mut self, msg: &str) -> Self {
        self.warnings.push(msg.to_string());
        self
    }

    pub fn add_error(&mut self, msg: &str) {
        self.valid = false;
        self.errors.push(msg.to_string());
    }

    pub fn add_warning(&mut self, msg: &str) {
        self.warnings.push(msg.to_string());
    }
}

// ============================================================================
// Configurable ID System
// ============================================================================

/// ID format style for requirement identifiers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
pub enum IdFormat {
    /// Single-level format: PREFIX-NNN (e.g., AUTH-001, FR-002)
    /// Features and types share the same namespace
    #[default]
    SingleLevel,
    /// Two-level format: FEATURE-TYPE-NNN (e.g., AUTH-FR-001)
    /// Hierarchical with feature prefix, type prefix, and number
    TwoLevel,
}

/// Numbering strategy for requirement IDs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
pub enum NumberingStrategy {
    /// Global sequential numbering across all prefixes
    /// e.g., AUTH-001, FR-002, PAY-003
    #[default]
    Global,
    /// Per-prefix numbering (each prefix has its own counter)
    /// e.g., AUTH-001, FR-001, PAY-001
    PerPrefix,
    /// Per feature+type combination (only for TwoLevel format)
    /// e.g., AUTH-FR-001, AUTH-FR-002, AUTH-NFR-001
    PerFeatureType,
}

/// Configuration for a requirement type with its prefix
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct RequirementTypeDefinition {
    /// Display name for the type (e.g., "Functional")
    pub name: String,
    /// Prefix used in IDs (e.g., "FR")
    pub prefix: String,
    /// Optional description
    #[serde(default)]
    pub description: String,
}

impl RequirementTypeDefinition {
    pub fn new(name: &str, prefix: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            prefix: prefix.to_uppercase(),
            description: description.to_string(),
        }
    }
}

/// Default requirement types with prefixes
fn default_requirement_types() -> Vec<RequirementTypeDefinition> {
    vec![
        RequirementTypeDefinition::new("Functional", "FR", "Functional requirements"),
        RequirementTypeDefinition::new(
            "Non-Functional",
            "NFR",
            "Non-functional requirements (performance, security, etc.)",
        ),
        RequirementTypeDefinition::new("System", "SR", "System-level requirements"),
        RequirementTypeDefinition::new("User", "UR", "User story requirements"),
        RequirementTypeDefinition::new(
            "Change Request",
            "CR",
            "Change requests for modifications to existing functionality",
        ),
        RequirementTypeDefinition::new("Bug", "BUG", "Bug reports and defects"),
        RequirementTypeDefinition::new("Epic", "EPIC", "Large features spanning multiple stories"),
        RequirementTypeDefinition::new("Story", "STORY", "User stories for agile development"),
        RequirementTypeDefinition::new("Task", "TASK", "Individual work items"),
        RequirementTypeDefinition::new("Spike", "SPIKE", "Research and investigation tasks"),
        RequirementTypeDefinition::new("Sprint", "SPRINT", "Sprint planning containers"),
        RequirementTypeDefinition::new("Folder", "FOLDER", "Organizational folders"),
        RequirementTypeDefinition::new("Meta", "META", "Database configuration and templates"),
    ]
}

/// Configuration for a feature with its prefix
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct FeatureDefinition {
    /// Sequential number for ordering
    pub number: u32,
    /// Display name for the feature
    pub name: String,
    /// Prefix used in IDs (e.g., "AUTH" for Authentication)
    pub prefix: String,
    /// Optional description
    #[serde(default)]
    pub description: String,
}

impl FeatureDefinition {
    pub fn new(number: u32, name: &str, prefix: &str) -> Self {
        Self {
            number,
            name: name.to_string(),
            prefix: prefix.to_uppercase(),
            description: String::new(),
        }
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = description.to_string();
        self
    }
}

/// ID system configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct IdConfiguration {
    /// Format style for IDs
    #[serde(default)]
    pub format: IdFormat,
    /// Numbering strategy
    #[serde(default)]
    pub numbering: NumberingStrategy,
    /// Number of digits for the numeric portion (default 3 = 001)
    #[serde(default = "default_id_digits")]
    pub digits: u8,
    /// Configured requirement types
    #[serde(default = "default_requirement_types")]
    pub requirement_types: Vec<RequirementTypeDefinition>,
}

fn default_id_digits() -> u8 {
    3
}

impl Default for IdConfiguration {
    fn default() -> Self {
        Self {
            format: IdFormat::default(),
            numbering: NumberingStrategy::default(),
            digits: 3,
            requirement_types: default_requirement_types(),
        }
    }
}

impl IdConfiguration {
    /// Get all reserved prefixes (type prefixes that cannot be used as feature prefixes)
    pub fn reserved_prefixes(&self) -> Vec<String> {
        self.requirement_types
            .iter()
            .map(|t| t.prefix.clone())
            .collect()
    }

    /// Check if a prefix is reserved (used by a requirement type)
    pub fn is_prefix_reserved(&self, prefix: &str) -> bool {
        let upper = prefix.to_uppercase();
        self.requirement_types.iter().any(|t| t.prefix == upper)
    }

    /// Get a requirement type definition by name
    pub fn get_type_by_name(&self, name: &str) -> Option<&RequirementTypeDefinition> {
        let lower = name.to_lowercase();
        self.requirement_types
            .iter()
            .find(|t| t.name.to_lowercase() == lower)
    }

    /// Get a requirement type definition by prefix
    pub fn get_type_by_prefix(&self, prefix: &str) -> Option<&RequirementTypeDefinition> {
        let upper = prefix.to_uppercase();
        self.requirement_types.iter().find(|t| t.prefix == upper)
    }

    /// Format a number with the configured digit width
    pub fn format_number(&self, num: u32) -> String {
        format!("{:0>width$}", num, width = self.digits as usize)
    }
}

// ============================================================================
// Original structures continue below
// ============================================================================

/// Result of validating ID configuration changes
#[derive(Debug, Clone)]
pub struct IdConfigValidation {
    /// Whether the change is valid
    pub valid: bool,
    /// Error message if invalid
    pub error: Option<String>,
    /// Warning message (change is valid but has implications)
    pub warning: Option<String>,
    /// Whether migration is possible
    pub can_migrate: bool,
    /// Number of requirements that would be affected by migration
    pub affected_count: usize,
}

/// Represents a relationship between two requirements
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct Relationship {
    /// The type of relationship
    pub rel_type: RelationshipType,
    /// The target requirement ID
    pub target_id: Uuid,
    /// When this relationship was created
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// Who created this relationship
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

/// Represents a field change in a requirement's history
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct FieldChange {
    /// Name of the field that changed
    pub field_name: String,

    /// Value before the change
    pub old_value: String,

    /// Value after the change
    pub new_value: String,
}

/// Represents a history entry for a requirement update
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct HistoryEntry {
    /// Unique identifier for this history entry
    pub id: Uuid,

    /// Who made the change
    pub author: String,

    /// When the change was made
    pub timestamp: DateTime<Utc>,

    /// List of field changes in this update
    pub changes: Vec<FieldChange>,
}

impl HistoryEntry {
    /// Creates a new history entry
    pub fn new(author: String, changes: Vec<FieldChange>) -> Self {
        Self {
            id: Uuid::now_v7(),
            author,
            timestamp: Utc::now(),
            changes,
        }
    }
}

/// A snapshot of a requirement at a specific point in time (for baselines)
/// This is a full copy of the requirement state, not a reference
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct RequirementSnapshot {
    /// The original requirement's UUID (for linking back)
    pub original_id: Uuid,

    /// Spec ID at the time of snapshot
    pub spec_id: Option<String>,

    /// Title at snapshot time
    pub title: String,

    /// Description at snapshot time
    pub description: String,

    /// Status at snapshot time
    pub status: RequirementStatus,

    /// Priority at snapshot time
    pub priority: RequirementPriority,

    /// Owner at snapshot time
    pub owner: String,

    /// Feature at snapshot time
    pub feature: String,

    /// Type at snapshot time
    pub req_type: RequirementType,

    /// Tags at snapshot time
    #[serde(serialize_with = "crate::yaml_helpers::serialize_sorted_string_set")]
    pub tags: HashSet<String>,

    /// Relationships at snapshot time (storing IDs, not full objects)
    pub relationships: Vec<Relationship>,

    /// Custom status at snapshot time
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_status: Option<String>,

    /// Custom priority at snapshot time
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_priority: Option<String>,

    /// Custom fields at snapshot time
    #[serde(
        default,
        skip_serializing_if = "std::collections::HashMap::is_empty",
        serialize_with = "crate::yaml_helpers::serialize_sorted_string_map"
    )]
    pub custom_fields: std::collections::HashMap<String, String>,
}

impl RequirementSnapshot {
    /// Creates a snapshot from a requirement
    pub fn from_requirement(req: &Requirement) -> Self {
        Self {
            original_id: req.id,
            spec_id: req.spec_id.clone(),
            title: req.title.clone(),
            description: req.description.clone(),
            status: req.status.clone(),
            priority: req.priority.clone(),
            owner: req.owner.clone(),
            feature: req.feature.clone(),
            req_type: req.req_type.clone(),
            tags: req.tags.clone(),
            relationships: req.relationships.clone(),
            custom_status: req.custom_status.clone(),
            custom_priority: req.custom_priority.clone(),
            custom_fields: req.custom_fields.clone(),
        }
    }
}

/// Represents a baseline - a named snapshot of requirements at a point in time
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct Baseline {
    /// Unique identifier for this baseline
    pub id: Uuid,

    /// Human-readable name (e.g., "Release 1.0", "Sprint 5 End")
    pub name: String,

    /// Optional description of what this baseline represents
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// When the baseline was created
    pub created_at: DateTime<Utc>,

    /// Who created the baseline
    pub created_by: String,

    /// Git tag associated with this baseline (for YAML backend)
    /// Format: "baseline-{name-slug}" or custom
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_tag: Option<String>,

    /// Full snapshots of all requirements at baseline time
    /// For SQL backends, this is always populated
    /// For YAML backend, this may be empty if git_tag is used (can reconstruct from git)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<RequirementSnapshot>,

    /// Whether this baseline is locked (cannot be modified or deleted)
    #[serde(default)]
    pub locked: bool,
}

impl Baseline {
    /// Creates a new baseline with snapshots of the given requirements
    pub fn new(
        name: String,
        description: Option<String>,
        created_by: String,
        requirements: &[Requirement],
    ) -> Self {
        let snapshots: Vec<RequirementSnapshot> = requirements
            .iter()
            .filter(|r| !r.archived) // Don't include archived requirements in baselines
            .map(RequirementSnapshot::from_requirement)
            .collect();

        Self {
            id: Uuid::now_v7(),
            name,
            description,
            created_at: Utc::now(),
            created_by,
            git_tag: None,
            requirements: snapshots,
            locked: false,
        }
    }

    /// Creates a baseline name slug suitable for git tags
    pub fn name_slug(&self) -> String {
        self.name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    }

    /// Gets the git tag name for this baseline
    pub fn git_tag_name(&self) -> String {
        self.git_tag
            .clone()
            .unwrap_or_else(|| format!("baseline-{}", self.name_slug()))
    }
}

/// Summary of changes between two baselines or baseline and current state
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
pub struct BaselineComparison {
    /// Requirements added (in target but not in source)
    pub added: Vec<Uuid>,

    /// Requirements removed (in source but not in target)
    pub removed: Vec<Uuid>,

    /// Requirements modified (exist in both but changed)
    pub modified: Vec<BaselineRequirementDiff>,

    /// Requirements unchanged
    pub unchanged: Vec<Uuid>,
}

/// Represents changes to a single requirement between baselines
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BaselineRequirementDiff {
    /// The requirement's UUID
    pub id: Uuid,

    /// Spec ID (for display)
    pub spec_id: Option<String>,

    /// List of changed fields
    pub changes: Vec<FieldChange>,
}

/// Represents a reaction emoji definition
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct ReactionDefinition {
    /// Unique identifier/key for the reaction (e.g., "resolved", "rejected")
    pub name: String,

    /// The emoji character to display
    pub emoji: String,

    /// Human-readable label for the reaction
    pub label: String,

    /// Optional description of when to use this reaction
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Whether this is a built-in reaction (cannot be deleted)
    #[serde(default)]
    pub built_in: bool,
}

impl ReactionDefinition {
    /// Creates a new reaction definition
    pub fn new(
        name: impl Into<String>,
        emoji: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            emoji: emoji.into(),
            label: label.into(),
            description: None,
            built_in: false,
        }
    }

    /// Creates a built-in reaction definition
    pub fn builtin(
        name: impl Into<String>,
        emoji: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            emoji: emoji.into(),
            label: label.into(),
            description: Some(description.into()),
            built_in: true,
        }
    }
}

/// Returns the default set of reaction definitions
pub fn default_reaction_definitions() -> Vec<ReactionDefinition> {
    vec![
        ReactionDefinition::builtin(
            "resolved",
            "✅",
            "Resolved",
            "Mark comment as resolved/addressed",
        ),
        ReactionDefinition::builtin(
            "rejected",
            "❌",
            "Rejected",
            "Mark comment as rejected/declined",
        ),
        ReactionDefinition::builtin("thumbs_up", "👍", "Thumbs Up", "Agree or approve"),
        ReactionDefinition::builtin("thumbs_down", "👎", "Thumbs Down", "Disagree or disapprove"),
        ReactionDefinition::builtin("question", "❓", "Question", "Needs clarification"),
        ReactionDefinition::builtin(
            "important",
            "⚠️",
            "Important",
            "Mark as important/attention needed",
        ),
    ]
}

/// Represents a reaction on a comment
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct CommentReaction {
    /// The reaction type (references ReactionDefinition.name)
    pub reaction: String,

    /// Who added this reaction
    pub author: String,

    /// When the reaction was added
    pub added_at: DateTime<Utc>,
}

impl CommentReaction {
    /// Creates a new reaction
    pub fn new(reaction: impl Into<String>, author: impl Into<String>) -> Self {
        Self {
            reaction: reaction.into(),
            author: author.into(),
            added_at: Utc::now(),
        }
    }
}

/// How a URL should be opened when clicked
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum UrlOpenMode {
    /// Open in embedded iframe preview (default)
    #[default]
    Preview,
    /// Open in a new browser tab/window
    NewTab,
}

/// Represents an external URL link attached to a requirement
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct UrlLink {
    /// Unique identifier for the link
    pub id: Uuid,

    /// The URL
    pub url: String,

    /// Display title/label for the link
    pub title: String,

    /// Optional description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// How the URL should be opened (preview iframe or new tab)
    #[serde(default)]
    pub open_mode: UrlOpenMode,

    /// When the link was added
    pub added_at: DateTime<Utc>,

    /// Who added the link
    pub added_by: String,

    /// Last time the URL was verified as accessible
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_verified: Option<DateTime<Utc>>,

    /// Whether the last verification succeeded
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_verified_ok: Option<bool>,
}

impl UrlLink {
    /// Creates a new URL link
    pub fn new(
        url: impl Into<String>,
        title: impl Into<String>,
        added_by: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            url: url.into(),
            title: title.into(),
            description: None,
            open_mode: UrlOpenMode::default(),
            added_at: Utc::now(),
            added_by: added_by.into(),
            last_verified: None,
            last_verified_ok: None,
        }
    }

    /// Creates a new URL link with description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Sets the open mode for the URL link
    pub fn with_open_mode(mut self, mode: UrlOpenMode) -> Self {
        self.open_mode = mode;
        self
    }
}

/// Represents a file attachment on a requirement
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct Attachment {
    /// Unique identifier for the attachment
    pub id: Uuid,

    /// Original filename
    pub filename: String,

    /// Relative path to the stored file (e.g., "attachments/FR-0042/document.pdf")
    pub stored_path: String,

    /// Optional MIME type
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,

    /// File size in bytes
    pub size_bytes: u64,

    /// When the attachment was added
    pub added_at: DateTime<Utc>,

    /// Who added the attachment (user handle)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<String>,

    /// Optional description of the attachment
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Attachment {
    /// Creates a new attachment
    pub fn new(
        filename: impl Into<String>,
        stored_path: impl Into<String>,
        size_bytes: u64,
        added_by: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            filename: filename.into(),
            stored_path: stored_path.into(),
            mime_type: None,
            size_bytes,
            added_at: Utc::now(),
            added_by,
            description: None,
        }
    }

    /// Sets the MIME type
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    /// Sets the description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Formats the file size as a human-readable string
    pub fn format_size(&self) -> String {
        let bytes = self.size_bytes;
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }
}

// trace:REQ-0243 | ai:claude:high
/// Represents the type of artifact being traced
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
pub enum ArtifactType {
    /// Source code file
    SourceCode,
    /// Test code file
    TestCode,
    /// Configuration file
    Config,
    /// Documentation file
    Doc,
}

impl fmt::Display for ArtifactType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArtifactType::SourceCode => write!(f, "source"),
            ArtifactType::TestCode => write!(f, "test"),
            ArtifactType::Config => write!(f, "config"),
            ArtifactType::Doc => write!(f, "doc"),
        }
    }
}

impl ArtifactType {
    /// Parse an artifact type from a string
    // why: inherent parser returns Option<Self>; std FromStr would force a Result + Err type.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "source" | "sourcecode" | "src" | "code" => Some(ArtifactType::SourceCode),
            "test" | "testcode" | "tests" => Some(ArtifactType::TestCode),
            "config" | "configuration" | "cfg" => Some(ArtifactType::Config),
            "doc" | "docs" | "documentation" => Some(ArtifactType::Doc),
            _ => None,
        }
    }
}

// trace:REQ-0243 | ai:claude:high
/// Represents a trace link between a requirement and a code artifact
/// This enables bidirectional traceability: requirement -> code and code -> requirement
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct TraceLink {
    /// Unique identifier for the trace link
    pub id: Uuid,

    /// The type of artifact (source, test, config, doc)
    pub artifact_type: ArtifactType,

    /// Path to the file containing the artifact (relative to project root)
    pub file_path: String,

    /// Optional symbol name (function, struct, module, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,

    /// Optional line range (start, end) where the implementation exists
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,

    /// Optional notes about this trace link
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,

    /// When this trace link was created
    pub created_at: DateTime<Utc>,

    /// Who created this trace link
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,

    /// Git commit hash where this trace was identified (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,
}

impl TraceLink {
    /// Creates a new trace link
    pub fn new(artifact_type: ArtifactType, file_path: impl Into<String>) -> Self {
        Self {
            id: Uuid::now_v7(),
            artifact_type,
            file_path: file_path.into(),
            symbol: None,
            line_start: None,
            line_end: None,
            notes: None,
            created_at: Utc::now(),
            created_by: None,
            commit_hash: None,
        }
    }

    /// Sets the symbol name
    pub fn with_symbol(mut self, symbol: impl Into<String>) -> Self {
        self.symbol = Some(symbol.into());
        self
    }

    /// Sets the line range
    pub fn with_lines(mut self, start: u32, end: u32) -> Self {
        self.line_start = Some(start);
        self.line_end = Some(end);
        self
    }

    /// Sets notes
    pub fn with_notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = Some(notes.into());
        self
    }

    /// Sets the creator
    pub fn with_created_by(mut self, author: impl Into<String>) -> Self {
        self.created_by = Some(author.into());
        self
    }

    /// Sets the commit hash
    pub fn with_commit(mut self, hash: impl Into<String>) -> Self {
        self.commit_hash = Some(hash.into());
        self
    }

    /// Returns the line range as a tuple if both start and end are set
    pub fn line_range(&self) -> Option<(u32, u32)> {
        match (self.line_start, self.line_end) {
            (Some(start), Some(end)) => Some((start, end)),
            _ => None,
        }
    }
}

// trace:STORY-0323 | ai:claude
/// Represents a link between an AIDA requirement and a GitLab issue
/// Used to track traceability between specs and implementation work
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct GitLabIssueLink {
    /// Unique identifier for the link
    pub id: Uuid,

    /// GitLab issue IID (project-scoped issue number)
    pub issue_iid: u64,

    /// GitLab project ID (optional - uses default from config if not set)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<u64>,

    /// Display title for the issue (cached from GitLab)
    pub issue_title: String,

    /// Type of link between requirement and issue
    #[serde(default)]
    pub link_type: GitLabLinkType,

    /// Optional notes about this link
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,

    /// When this link was created
    pub created_at: DateTime<Utc>,

    /// Who created this link
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,

    /// Last time the issue data was synced from GitLab
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced: Option<DateTime<Utc>>,

    /// GitLab issue state when last synced (open/closed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_state: Option<String>,
}

impl GitLabIssueLink {
    /// Creates a new GitLab issue link
    pub fn new(issue_iid: u64, issue_title: impl Into<String>) -> Self {
        Self {
            id: Uuid::now_v7(),
            issue_iid,
            project_id: None,
            issue_title: issue_title.into(),
            link_type: GitLabLinkType::default(),
            notes: None,
            created_at: Utc::now(),
            created_by: None,
            last_synced: None,
            issue_state: None,
        }
    }

    /// Sets the project ID
    pub fn with_project(mut self, project_id: u64) -> Self {
        self.project_id = Some(project_id);
        self
    }

    /// Sets the link type
    pub fn with_link_type(mut self, link_type: GitLabLinkType) -> Self {
        self.link_type = link_type;
        self
    }

    /// Sets the creator
    pub fn with_creator(mut self, creator: impl Into<String>) -> Self {
        self.created_by = Some(creator.into());
        self
    }

    /// Sets notes
    pub fn with_notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = Some(notes.into());
        self
    }

    /// Update sync metadata from GitLab issue
    pub fn update_from_issue(&mut self, title: &str, state: &str) {
        self.issue_title = title.to_string();
        self.issue_state = Some(state.to_string());
        self.last_synced = Some(Utc::now());
    }

    /// Returns a display string like "GL-123"
    pub fn display_id(&self) -> String {
        format!("GL-{}", self.issue_iid)
    }
}

// trace:STORY-0323 | ai:claude
/// Type of link between AIDA requirement and GitLab issue
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
pub enum GitLabLinkType {
    /// Requirement is implemented by the GitLab issue
    #[default]
    ImplementedBy,
    /// Requirement traces to the GitLab issue (general traceability)
    TracesTo,
    /// GitLab issue is a bug related to this requirement
    RelatedBug,
    /// GitLab issue is a follow-up task
    FollowUp,
}

impl fmt::Display for GitLabLinkType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitLabLinkType::ImplementedBy => write!(f, "Implemented by"),
            GitLabLinkType::TracesTo => write!(f, "Traces to"),
            GitLabLinkType::RelatedBug => write!(f, "Related bug"),
            GitLabLinkType::FollowUp => write!(f, "Follow-up"),
        }
    }
}

// trace:STORY-0325 | ai:claude
/// How a GitLab link was originally created
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
pub enum LinkOrigin {
    /// Issue was created from AIDA via "Create GitLab Issue"
    #[default]
    CreatedFromAida,
    /// Issue was imported from GitLab (future feature)
    ImportedFromGitLab,
    /// User manually linked an existing issue
    ManualLink,
}

impl fmt::Display for LinkOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkOrigin::CreatedFromAida => write!(f, "Created from AIDA"),
            LinkOrigin::ImportedFromGitLab => write!(f, "Imported from GitLab"),
            LinkOrigin::ManualLink => write!(f, "Manual link"),
        }
    }
}

// trace:STORY-0325 | ai:claude
/// Current sync status between AIDA and GitLab
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
pub enum SyncStatus {
    /// Content matches between AIDA and GitLab
    #[default]
    InSync,
    /// AIDA requirement has changed since last sync
    AidaModified,
    /// GitLab issue has changed since last sync
    GitLabModified,
    /// Both AIDA and GitLab have changed (conflict)
    Conflict,
    /// A sync error occurred
    Error,
    /// Linked but not actively syncing
    Untracked,
}

impl fmt::Display for SyncStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncStatus::InSync => write!(f, "In Sync"),
            SyncStatus::AidaModified => write!(f, "AIDA Modified"),
            SyncStatus::GitLabModified => write!(f, "GitLab Modified"),
            SyncStatus::Conflict => write!(f, "Conflict"),
            SyncStatus::Error => write!(f, "Error"),
            SyncStatus::Untracked => write!(f, "Untracked"),
        }
    }
}

// trace:STORY-0325 | ai:claude
/// Tracks sync state between an AIDA requirement and a GitLab issue
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GitLabSyncState {
    /// AIDA requirement UUID
    pub requirement_id: Uuid,

    /// AIDA spec-id (for display)
    pub spec_id: String,

    /// GitLab project ID
    pub gitlab_project_id: u64,

    /// GitLab issue IID (project-scoped issue number)
    pub gitlab_issue_iid: u64,

    /// GitLab issue global ID (for API operations)
    pub gitlab_issue_id: u64,

    /// When the link was created
    pub linked_at: DateTime<Utc>,

    /// Last successful sync timestamp
    pub last_sync: DateTime<Utc>,

    /// Hash of AIDA content at last sync
    pub aida_content_hash: String,

    /// Hash of GitLab content at last sync
    pub gitlab_content_hash: String,

    /// How the link was created
    pub link_origin: LinkOrigin,

    /// Current sync status
    pub sync_status: SyncStatus,

    /// Last sync error message (if any)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl GitLabSyncState {
    /// Create a new sync state for a newly created link
    pub fn new(
        requirement_id: Uuid,
        spec_id: impl Into<String>,
        gitlab_project_id: u64,
        gitlab_issue_iid: u64,
        gitlab_issue_id: u64,
        link_origin: LinkOrigin,
    ) -> Self {
        let now = Utc::now();
        Self {
            requirement_id,
            spec_id: spec_id.into(),
            gitlab_project_id,
            gitlab_issue_iid,
            gitlab_issue_id,
            linked_at: now,
            last_sync: now,
            aida_content_hash: String::new(),
            gitlab_content_hash: String::new(),
            link_origin,
            sync_status: SyncStatus::Untracked,
            last_error: None,
        }
    }

    /// Update the sync state after a successful sync
    pub fn mark_synced(&mut self, aida_hash: String, gitlab_hash: String) {
        self.last_sync = Utc::now();
        self.aida_content_hash = aida_hash;
        self.gitlab_content_hash = gitlab_hash;
        self.sync_status = SyncStatus::InSync;
        self.last_error = None;
    }

    /// Mark the sync state as having an error
    pub fn mark_error(&mut self, error: impl Into<String>) {
        self.sync_status = SyncStatus::Error;
        self.last_error = Some(error.into());
    }

    /// Calculate content hash for an AIDA requirement
    /// Includes: title, description, status, priority, owner
    /// Excludes: timestamps, comments, history (too volatile)
    pub fn hash_requirement(req: &Requirement) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();

        // Include stable content fields
        hasher.update(req.title.as_bytes());
        hasher.update(req.description.as_bytes());
        hasher.update(req.status.to_string().as_bytes());
        hasher.update(req.priority.to_string().as_bytes());
        hasher.update(req.owner.as_bytes());
        hasher.update(req.req_type.to_string().as_bytes());

        // Include tags (sorted for consistency)
        let mut tags: Vec<_> = req.tags.iter().collect();
        tags.sort();
        for tag in tags {
            hasher.update(tag.as_bytes());
        }

        format!("{:x}", hasher.finalize())
    }

    /// Calculate content hash for a GitLab issue
    /// Includes: title, description, state, labels, assignees
    /// Excludes: timestamps, comment count, vote count (too volatile)
    #[cfg(feature = "gitlab")]
    pub fn hash_gitlab_issue(issue: &crate::integrations::gitlab::GitLabIssue) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();

        // Include stable content fields
        hasher.update(issue.title.as_bytes());
        if let Some(desc) = &issue.description {
            hasher.update(desc.as_bytes());
        }
        hasher.update(format!("{:?}", issue.state).as_bytes());

        // Include labels (sorted for consistency)
        let mut labels = issue.labels.clone();
        labels.sort();
        for label in labels {
            hasher.update(label.as_bytes());
        }

        // Include assignees (sorted by id for consistency)
        let mut assignee_ids: Vec<_> = issue.assignees.iter().map(|a| a.id).collect();
        assignee_ids.sort();
        for id in assignee_ids {
            hasher.update(id.to_string().as_bytes());
        }

        format!("{:x}", hasher.finalize())
    }
}

// trace:EPIC-0246 | ai:claude:high
/// Confidence level for AI-generated implementation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
pub enum ConfidenceLevel {
    /// >80% AI-generated
    High,
    /// 40-80% AI with modifications
    Medium,
    /// <40% AI, mostly human
    Low,
}

impl fmt::Display for ConfidenceLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfidenceLevel::High => write!(f, "high"),
            ConfidenceLevel::Medium => write!(f, "med"),
            ConfidenceLevel::Low => write!(f, "low"),
        }
    }
}

impl ConfidenceLevel {
    /// Parse a confidence level from a string
    // why: inherent parser returns Option<Self>; std FromStr would force a Result + Err type.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "high" | "h" => Some(ConfidenceLevel::High),
            "medium" | "med" | "m" => Some(ConfidenceLevel::Medium),
            "low" | "l" => Some(ConfidenceLevel::Low),
            _ => None,
        }
    }
}

// trace:EPIC-0246 | ai:claude:high
/// Tracks implementation metadata for a requirement
/// Stores information about how and when a requirement was implemented
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, Default)]
pub struct ImplementationInfo {
    /// Whether the requirement has been implemented
    pub implemented: bool,

    /// Summary of the implementation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// When the last AI agent run occurred for this requirement
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_agent_run: Option<DateTime<Utc>>,

    /// Risk notes identified during implementation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_notes: Option<String>,

    /// Notes about test coverage
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_coverage_notes: Option<String>,

    /// The AI tool used for implementation (e.g., "claude", "copilot")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_tool: Option<String>,

    /// Confidence level of the AI-generated implementation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<ConfidenceLevel>,

    /// When the implementation was completed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implemented_at: Option<DateTime<Utc>>,

    /// Who performed the implementation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implemented_by: Option<String>,

    /// When the auto-bump from `Done` → `Completed` fired (i.e. when a
    /// commit referencing this spec landed on the default branch).
    /// Stamped by the `aida pull` auto-bump path, not by `aida queue done`.
    /// trace:STORY-86 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,

    /// The default-branch commit SHA that triggered the auto-bump.
    /// Stamped alongside `completed_at`.
    /// trace:STORY-86 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_sha: Option<String>,
}

impl ImplementationInfo {
    /// Creates a new ImplementationInfo with implemented=false
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new ImplementationInfo marked as implemented
    pub fn implemented() -> Self {
        Self {
            implemented: true,
            implemented_at: Some(Utc::now()),
            ..Self::default()
        }
    }

    /// Sets the implementation summary
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Sets the source tool
    pub fn with_source_tool(mut self, tool: impl Into<String>) -> Self {
        self.source_tool = Some(tool.into());
        self
    }

    /// Sets the confidence level
    pub fn with_confidence(mut self, confidence: ConfidenceLevel) -> Self {
        self.confidence = Some(confidence);
        self
    }

    /// Sets the implementer
    pub fn with_implemented_by(mut self, author: impl Into<String>) -> Self {
        self.implemented_by = Some(author.into());
        self
    }

    /// Records an agent run
    pub fn record_agent_run(&mut self) {
        self.last_agent_run = Some(Utc::now());
    }
}

/// Parsed trace comment from source code
/// Format: `// trace:<SPEC-ID> - <title> | ai:<tool>:<confidence> | impl:<date> | by:<user>`
#[derive(Debug, Clone, PartialEq, Eq, TS)]
pub struct TraceComment {
    /// The requirement ID (e.g., FR-0042)
    pub spec_id: String,
    /// Brief title from the requirement (optional)
    pub title: Option<String>,
    /// AI tool used (e.g., "claude")
    pub ai_tool: Option<String>,
    /// Confidence level
    pub confidence: Option<ConfidenceLevel>,
    /// Implementation date
    pub impl_date: Option<String>,
    /// Implementer username
    pub implemented_by: Option<String>,
}

impl TraceComment {
    /// Parse a trace comment from a line of source code
    /// Supports both old format: `// trace:FR-0042 | ai:claude:high`
    /// And new format: `// trace:FR-0042 - Title | ai:claude:high | impl:2025-12-10 | by:joe`
    pub fn parse(line: &str) -> Option<Self> {
        // Strip comment prefix and find trace:
        let line = line.trim();
        let trace_start = line.find("trace:")?;
        let content = &line[trace_start + 6..];

        // Split by pipe to get segments
        let segments: Vec<&str> = content.split('|').map(|s| s.trim()).collect();
        if segments.is_empty() {
            return None;
        }

        // First segment: SPEC-ID optionally followed by " - title"
        let first = segments[0];
        let (spec_id, title) = if let Some(dash_pos) = first.find(" - ") {
            let id = first[..dash_pos].trim().to_string();
            let title = first[dash_pos + 3..].trim().to_string();
            (id, Some(title))
        } else {
            (first.trim().to_string(), None)
        };

        let mut result = TraceComment {
            spec_id,
            title,
            ai_tool: None,
            confidence: None,
            impl_date: None,
            implemented_by: None,
        };

        // Parse remaining segments
        for segment in segments.iter().skip(1) {
            let segment = segment.trim();
            if let Some(rest) = segment.strip_prefix("ai:") {
                // Format: ai:claude:high or ai:claude
                let parts: Vec<&str> = rest.split(':').collect();
                if !parts.is_empty() {
                    result.ai_tool = Some(parts[0].to_string());
                }
                if parts.len() > 1 {
                    result.confidence = ConfidenceLevel::from_str(parts[1]);
                }
            } else if let Some(rest) = segment.strip_prefix("impl:") {
                result.impl_date = Some(rest.trim().to_string());
            } else if let Some(rest) = segment.strip_prefix("by:") {
                result.implemented_by = Some(rest.trim().to_string());
            }
        }

        Some(result)
    }

    /// Format as a trace comment string (without the comment prefix)
    pub fn format(&self) -> String {
        let mut parts = Vec::new();

        // SPEC-ID with optional title
        let spec_part = if let Some(ref title) = self.title {
            format!("{} - {}", self.spec_id, title)
        } else {
            self.spec_id.clone()
        };
        parts.push(spec_part);

        // AI tool and confidence
        if let Some(ref tool) = self.ai_tool {
            let ai_part = if let Some(ref conf) = self.confidence {
                format!("ai:{}:{}", tool, conf)
            } else {
                format!("ai:{}", tool)
            };
            parts.push(ai_part);
        }

        // Implementation date
        if let Some(ref date) = self.impl_date {
            parts.push(format!("impl:{}", date));
        }

        // Implementer
        if let Some(ref by) = self.implemented_by {
            parts.push(format!("by:{}", by));
        }

        format!("trace:{}", parts.join(" | "))
    }
}

/// Represents a comment on a requirement with threading support
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct Comment {
    /// Unique identifier for the comment
    pub id: Uuid,

    /// Author of the comment
    pub author: String,

    /// Content of the comment
    pub content: String,

    /// When the comment was created
    pub created_at: DateTime<Utc>,

    /// When the comment was last modified
    pub modified_at: DateTime<Utc>,

    /// Parent comment ID (None for top-level comments)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,

    /// Nested replies to this comment
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Comment>,

    /// Reactions on this comment
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<CommentReaction>,
}

impl Comment {
    /// Creates a new top-level comment
    pub fn new(author: String, content: String) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::now_v7(),
            author,
            content,
            created_at: now,
            modified_at: now,
            parent_id: None,
            replies: Vec::new(),
            reactions: Vec::new(),
        }
    }

    /// Creates a new reply to an existing comment
    pub fn new_reply(author: String, content: String, parent_id: Uuid) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::now_v7(),
            author,
            content,
            created_at: now,
            modified_at: now,
            parent_id: Some(parent_id),
            replies: Vec::new(),
            reactions: Vec::new(),
        }
    }

    /// Adds a reaction to this comment
    /// Returns true if reaction was added, false if user already has this reaction
    pub fn add_reaction(&mut self, reaction: &str, author: &str) -> bool {
        // Check if user already has this reaction
        if self
            .reactions
            .iter()
            .any(|r| r.reaction == reaction && r.author == author)
        {
            return false;
        }
        self.reactions.push(CommentReaction::new(reaction, author));
        true
    }

    /// Removes a reaction from this comment
    /// Returns true if reaction was removed, false if not found
    pub fn remove_reaction(&mut self, reaction: &str, author: &str) -> bool {
        let initial_len = self.reactions.len();
        self.reactions
            .retain(|r| !(r.reaction == reaction && r.author == author));
        self.reactions.len() < initial_len
    }

    /// Toggles a reaction (adds if not present, removes if present)
    /// Returns true if reaction is now present, false if removed
    pub fn toggle_reaction(&mut self, reaction: &str, author: &str) -> bool {
        if self.remove_reaction(reaction, author) {
            false
        } else {
            self.add_reaction(reaction, author);
            true
        }
    }

    /// Gets counts of each reaction type
    pub fn reaction_counts(&self) -> std::collections::HashMap<String, usize> {
        let mut counts = std::collections::HashMap::new();
        for r in &self.reactions {
            *counts.entry(r.reaction.clone()).or_insert(0) += 1;
        }
        counts
    }

    /// Checks if a user has a specific reaction
    pub fn has_reaction(&self, reaction: &str, author: &str) -> bool {
        self.reactions
            .iter()
            .any(|r| r.reaction == reaction && r.author == author)
    }

    /// Adds a reply to this comment
    pub fn add_reply(&mut self, reply: Comment) {
        self.replies.push(reply);
    }

    /// Finds a comment by ID in this comment tree
    pub fn find_comment_mut(&mut self, id: &Uuid) -> Option<&mut Comment> {
        if &self.id == id {
            return Some(self);
        }
        for reply in &mut self.replies {
            if let Some(found) = reply.find_comment_mut(id) {
                return Some(found);
            }
        }
        None
    }

    /// Updates the modified timestamp
    pub fn touch(&mut self) {
        self.modified_at = Utc::now();
    }

    /// Recursively removes a reply from comment tree
    fn remove_reply_recursive(comment: &mut Comment, target_id: &Uuid) -> bool {
        if let Some(pos) = comment.replies.iter().position(|c| &c.id == target_id) {
            comment.replies.remove(pos);
            return true;
        }
        for reply in &mut comment.replies {
            if Comment::remove_reply_recursive(reply, target_id) {
                return true;
            }
        }
        false
    }
}

/// Represents a user in the system
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct User {
    /// Unique identifier for the user
    pub id: Uuid,

    /// Human-friendly spec ID (e.g., "$USER-001")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<String>,

    /// User's full name
    pub name: String,

    /// User's email address
    pub email: String,

    /// User's handle for @mentions (without the @)
    pub handle: String,

    /// Hashed PIN for simple authentication (SHA-256 hash stored as hex string)
    /// This is a basic authentication mechanism for web clients.
    /// For production use, consider a proper auth system (OAuth, JWT, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_hash: Option<String>,

    /// When the user was created
    pub created_at: DateTime<Utc>,

    /// Whether the user is archived
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,

    /// Version number for optimistic locking (SQLite only)
    #[serde(skip)]
    pub version: i64,
}

impl User {
    /// Creates a new user (without spec_id - use RequirementsStore::add_user for auto-generated ID)
    pub fn new(name: String, email: String, handle: String) -> Self {
        Self {
            id: Uuid::now_v7(),
            spec_id: None,
            name,
            email,
            handle,
            pin_hash: None,
            created_at: Utc::now(),
            archived: false,
            version: 1,
        }
    }

    /// Creates a new user with a spec_id
    pub fn new_with_spec_id(name: String, email: String, handle: String, spec_id: String) -> Self {
        Self {
            id: Uuid::now_v7(),
            spec_id: Some(spec_id),
            name,
            email,
            handle,
            pin_hash: None,
            created_at: Utc::now(),
            archived: false,
            version: 1,
        }
    }

    /// Returns display name: spec_id if available, otherwise name
    pub fn display_id(&self) -> &str {
        self.spec_id.as_deref().unwrap_or(&self.name)
    }

    /// Set the user's PIN (stores SHA-256 hash)
    pub fn set_pin(&mut self, pin: &str) {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(pin.as_bytes());
        let result = hasher.finalize();
        self.pin_hash = Some(format!("{:x}", result));
    }

    /// Verify a PIN against the stored hash
    /// Returns true if the PIN matches, false otherwise
    /// Returns false if no PIN is set
    pub fn verify_pin(&self, pin: &str) -> bool {
        if let Some(ref stored_hash) = self.pin_hash {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(pin.as_bytes());
            let result = hasher.finalize();
            let input_hash = format!("{:x}", result);
            stored_hash == &input_hash
        } else {
            false
        }
    }

    /// Check if the user has a PIN set
    pub fn has_pin(&self) -> bool {
        self.pin_hash.is_some()
    }

    /// Clear the user's PIN
    pub fn clear_pin(&mut self) {
        self.pin_hash = None;
    }
}

/// Represents a team in the system
/// Teams can contain users (members) and can be nested (parent/child teams)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Team {
    /// Unique identifier for the team
    pub id: Uuid,

    /// Human-friendly spec ID (e.g., "$TEAM-001")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<String>,

    /// Team name
    pub name: String,

    /// Team description (optional)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,

    /// Parent team ID for nested teams (None if top-level team)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_team_id: Option<Uuid>,

    /// User IDs of team members
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub member_ids: Vec<Uuid>,

    /// When the team was created
    pub created_at: DateTime<Utc>,

    /// When the team was last modified
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<DateTime<Utc>>,

    /// Whether the team is archived
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
}

impl Team {
    /// Creates a new team (without spec_id - use RequirementsStore::add_team_with_id for auto-generated ID)
    pub fn new(name: String, description: String, parent_team_id: Option<Uuid>) -> Self {
        Self {
            id: Uuid::now_v7(),
            spec_id: None,
            name,
            description,
            parent_team_id,
            member_ids: Vec::new(),
            created_at: Utc::now(),
            modified_at: None,
            archived: false,
        }
    }

    /// Creates a new team with a spec_id
    pub fn new_with_spec_id(
        name: String,
        description: String,
        parent_team_id: Option<Uuid>,
        spec_id: String,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            spec_id: Some(spec_id),
            name,
            description,
            parent_team_id,
            member_ids: Vec::new(),
            created_at: Utc::now(),
            modified_at: None,
            archived: false,
        }
    }

    /// Returns display name: spec_id if available, otherwise name
    pub fn display_id(&self) -> &str {
        self.spec_id.as_deref().unwrap_or(&self.name)
    }

    /// Adds a member to the team
    pub fn add_member(&mut self, user_id: Uuid) {
        if !self.member_ids.contains(&user_id) {
            self.member_ids.push(user_id);
            self.modified_at = Some(Utc::now());
        }
    }

    /// Removes a member from the team
    pub fn remove_member(&mut self, user_id: &Uuid) -> bool {
        if let Some(pos) = self.member_ids.iter().position(|id| id == user_id) {
            self.member_ids.remove(pos);
            self.modified_at = Some(Utc::now());
            true
        } else {
            false
        }
    }

    /// Checks if a user is a member of this team
    pub fn has_member(&self, user_id: &Uuid) -> bool {
        self.member_ids.contains(user_id)
    }

    /// Returns the number of members
    pub fn member_count(&self) -> usize {
        self.member_ids.len()
    }
}

/// Represents a single requirement in the system
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Requirement {
    /// Unique identifier for the requirement (UUID)
    pub id: Uuid,

    /// Human-friendly specification ID (e.g., "SPEC-001")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<String>,

    /// Short agreed ID assigned at merge-to-trunk (e.g., "FR-423").
    /// Only populated in distributed mode after the merge gate runs.
    /// In centralized mode, spec_id is already the short form so this is unused.
    /// Both spec_id and agreed_id permanently resolve to the same UUID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed_id: Option<String>,

    /// Optional prefix override for the spec_id (e.g., "SEC" for security requirements)
    /// If set, uses this prefix instead of deriving from feature/type
    /// Must be uppercase letters only (A-Z)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_override: Option<String>,

    /// Short title describing the requirement
    pub title: String,

    /// Detailed description of the requirement
    pub description: String,

    /// Current status of the requirement
    pub status: RequirementStatus,

    /// Priority level of the requirement
    pub priority: RequirementPriority,

    /// Person responsible for the requirement
    pub owner: String,

    /// The feature this requirement belongs to
    pub feature: String,

    /// When the requirement was created
    pub created_at: DateTime<Utc>,

    /// Who created this requirement
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,

    /// When the requirement was last modified
    pub modified_at: DateTime<Utc>,

    /// Type of the requirement
    pub req_type: RequirementType,

    /// Subtype for Meta requirements (prompts, skills, commands, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta_subtype: Option<MetaSubtype>,

    /// IDs of requirements this requirement depends on
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<Uuid>,

    /// Tags for categorizing the requirement
    #[serde(
        default,
        skip_serializing_if = "HashSet::is_empty",
        serialize_with = "crate::yaml_helpers::serialize_sorted_string_set"
    )]
    pub tags: HashSet<String>,

    /// Weight/effort estimate for the requirement (e.g., story points)
    /// Optional - only shown in UI when set
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f32>,

    /// Relationships to other requirements
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<Relationship>,

    /// Comments on this requirement (threaded)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,

    /// History of changes to this requirement
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<HistoryEntry>,

    /// Whether this requirement is archived
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,

    /// Timestamp when this requirement was archived (None when not archived).
    /// Cleared on unarchive. Used by `aida archive --older-than` sweeps and
    /// the auto-sweep on `aida pull` to compute spec age.
    /// trace:STORY-441 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<DateTime<Utc>>,

    /// Custom status string (for types with custom statuses)
    /// If set, this takes precedence over the `status` enum field
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_status: Option<String>,

    /// Custom priority string (for types with custom priorities)
    /// If set, this takes precedence over the `priority` enum field
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_priority: Option<String>,

    /// Custom field values (key = field name, value = field value as string)
    #[serde(
        default,
        skip_serializing_if = "std::collections::HashMap::is_empty",
        serialize_with = "crate::yaml_helpers::serialize_sorted_string_map"
    )]
    pub custom_fields: std::collections::HashMap<String, String>,

    /// External URL links attached to this requirement
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<UrlLink>,

    /// File attachments on this requirement
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,

    // trace:REQ-0243 | ai:claude:high
    /// Trace links to code artifacts implementing this requirement
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace_links: Vec<TraceLink>,

    // trace:STORY-0323 | ai:claude
    /// Links to GitLab issues related to this requirement
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gitlab_issues: Vec<GitLabIssueLink>,

    /// One-way external issue references composing this spec with PM systems
    /// (Linear / Jira / GitHub). Each entry is a validated `provider:id`
    /// string (e.g. `linear:LIN-123`, `jira:PROJ-456`,
    /// `github:owner/repo#123`). Rendered as clickable links via the
    /// `[external_refs]` base URLs in `.aida/config.toml`, and searchable.
    /// Deliberately one-way — AIDA records the ref, it does NOT sync state
    /// back to the external system. trace:STORY-476 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_refs: Vec<String>,

    // trace:EPIC-0246 | ai:claude:high
    /// Implementation metadata for this requirement
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation_info: Option<ImplementationInfo>,

    /// Cached AI evaluation results
    /// Automatically populated by background evaluator when requirement changes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_evaluation: Option<StoredAiEvaluation>,

    /// Why this spec is paused — set by `aida punt` when status flips to
    /// `NeedsAttention`, cleared when it is triaged back out. Answers "why is
    /// this *currently* paused"; the punt ledger keeps the durable history.
    /// trace:STORY-332 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_reason: Option<AttentionReason>,

    /// Set by the `--auto-complete` orchestrator when it shelves a spec
    /// after a phase failure (sibling to `attention_reason` — see
    /// [`FailureReason`]). Sticks until the spec is triaged out of
    /// `NeedsAttention`; the punt ledger keeps the durable history.
    /// trace:EPIC-28 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<FailureReason>,

    /// Marks this spec as work no agent can do — a person-in-the-room task,
    /// a sign-off, a physical activity. The pre-pickup gate
    /// (`crate::pickability`) skips any spec with this flag set so the
    /// orchestrator/queue never spawn a doomed phase-1 implementer on it.
    /// Distinct from `BlockedBy` (which clears when the blocker ships):
    /// `human_only` is a permanent property of the spec, cleared only by
    /// the human explicitly flipping it off. trace:STORY-333 | ai:claude
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub human_only: bool,

    /// A structured decision the human answers OUTSIDE any agent — the
    /// async decision-inbox artifact. `None` when no question is pending or
    /// recorded. Set by `aida questions ask`, answered by
    /// `aida questions answer`. trace:STORY-522 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_request: Option<DecisionRequest>,

    /// Version number for optimistic locking (SQLite only)
    /// Incremented on each update, used to detect concurrent modifications
    #[serde(skip)]
    pub version: i64,
}

impl Requirement {
    /// Creates a new requirement with the specified title and description
    pub fn new(title: String, description: String) -> Self {
        let now = Utc::now();

        // Get default feature name from environment variable
        let default_feature =
            env::var("REQ_FEATURE").unwrap_or_else(|_| String::from("Uncategorized"));

        Self {
            id: Uuid::now_v7(),
            spec_id: None, // Will be assigned when added to store
            agreed_id: None,
            prefix_override: None,
            title,
            description,
            status: RequirementStatus::Draft,
            priority: RequirementPriority::Medium,
            owner: String::new(),
            feature: default_feature,
            created_at: now,
            created_by: None,
            modified_at: now,
            req_type: RequirementType::Functional,
            meta_subtype: None,
            dependencies: Vec::new(),
            tags: HashSet::new(),
            weight: None,
            relationships: Vec::new(),
            comments: Vec::new(),
            history: Vec::new(),
            archived: false,
            archived_at: None,
            custom_status: None,
            custom_priority: None,
            custom_fields: std::collections::HashMap::new(),
            attention_reason: None,
            // trace:EPIC-28 | ai:claude
            failure_reason: None,
            // trace:STORY-333 | ai:claude
            human_only: false,
            // trace:STORY-522 | ai:claude
            decision_request: None,
            urls: Vec::new(),
            attachments: Vec::new(),
            trace_links: Vec::new(),
            gitlab_issues: Vec::new(),
            // trace:STORY-476 | ai:claude
            external_refs: Vec::new(),
            implementation_info: None,
            version: 1,
            ai_evaluation: None,
        }
    }

    /// Gets the best display ID: agreed_id if available, then spec_id, then UUID.
    pub fn display_id(&self) -> String {
        self.agreed_id
            .as_deref()
            .or(self.spec_id.as_deref())
            .unwrap_or("?")
            .to_string()
    }

    /// Check if this requirement matches a given ID string.
    /// Matches against spec_id, agreed_id, or UUID.
    pub fn matches_id(&self, id: &str) -> bool {
        self.spec_id.as_deref() == Some(id)
            || self.agreed_id.as_deref() == Some(id)
            || self.id.to_string() == id
    }

    /// Gets the effective status string, preferring custom_status if set
    pub fn effective_status(&self) -> String {
        self.custom_status
            .clone()
            .unwrap_or_else(|| self.status.to_string())
    }

    /// Sets the status from a string, using custom_status for non-standard values.
    ///
    /// Accepts every canonical variant a user is likely to type:
    /// - case-insensitive match
    /// - hyphens, underscores, and spaces between words are interchangeable
    /// - so "in-progress", "In Progress", "InProgress", "in_progress" all
    ///   map to RequirementStatus::InProgress.
    ///
    /// Truly non-standard strings (project-specific statuses) still fall
    /// through to custom_status. Whenever a canonical status IS recognized,
    /// custom_status is cleared so a previously-set custom value can't keep
    /// overriding effective_status() forever (BUG-1-025).
    /// trace:BUG-1-025 | ai:claude
    pub fn set_status_from_str(&mut self, status_str: &str) {
        // Normalize: lowercase, collapse whitespace/hyphen/underscore so the
        // match table only needs canonical forms.
        let normalized: String = status_str
            .chars()
            .filter_map(|c| match c {
                ' ' | '-' | '_' => None,
                c if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
                c => Some(c),
            })
            .collect();

        match normalized.as_str() {
            "draft" => {
                self.status = RequirementStatus::Draft;
                self.custom_status = None;
            }
            "approved" => {
                self.status = RequirementStatus::Approved;
                self.custom_status = None;
            }
            "planned" => {
                self.status = RequirementStatus::Planned;
                self.custom_status = None;
            }
            "inprogress" => {
                self.status = RequirementStatus::InProgress;
                self.custom_status = None;
            }
            "done" => {
                self.status = RequirementStatus::Done;
                self.custom_status = None;
            }
            "completed" => {
                self.status = RequirementStatus::Completed;
                self.custom_status = None;
            }
            "rejected" => {
                self.status = RequirementStatus::Rejected;
                self.custom_status = None;
            }
            "needsattention" => {
                self.status = RequirementStatus::NeedsAttention;
                self.custom_status = None;
            }
            _ => {
                // Custom status - preserve the original (un-normalized) form
                // so the user's chosen casing/spacing is what surfaces.
                self.custom_status = Some(status_str.to_string());
            }
        }
    }

    /// Gets the effective priority string, preferring custom_priority if set
    pub fn effective_priority(&self) -> String {
        self.custom_priority
            .clone()
            .unwrap_or_else(|| self.priority.to_string())
    }

    /// Sets the priority from a string, using custom_priority for non-standard values
    pub fn set_priority_from_str(&mut self, priority_str: &str) {
        match priority_str {
            "High" => {
                self.priority = RequirementPriority::High;
                self.custom_priority = None;
            }
            "Medium" => {
                self.priority = RequirementPriority::Medium;
                self.custom_priority = None;
            }
            "Low" => {
                self.priority = RequirementPriority::Low;
                self.custom_priority = None;
            }
            other => {
                // Custom priority - keep enum at Medium but store custom value
                self.custom_priority = Some(other.to_string());
            }
        }
    }

    /// Gets a custom field value
    pub fn get_custom_field(&self, name: &str) -> Option<&String> {
        self.custom_fields.get(name)
    }

    /// Sets a custom field value
    pub fn set_custom_field(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.custom_fields.insert(name.into(), value.into());
    }

    /// Removes a custom field
    pub fn remove_custom_field(&mut self, name: &str) -> Option<String> {
        self.custom_fields.remove(name)
    }

    /// Validates and normalizes a prefix string
    /// Returns Some(normalized_prefix) if valid, None if invalid
    /// Valid prefixes contain only uppercase letters A-Z
    pub fn validate_prefix(prefix: &str) -> Option<String> {
        let trimmed = prefix.trim();
        if trimmed.is_empty() {
            return None;
        }
        let upper = trimmed.to_uppercase();
        if upper.chars().all(|c| c.is_ascii_uppercase()) {
            Some(upper)
        } else {
            None
        }
    }

    /// Sets the prefix override with validation
    /// Returns Ok if valid or empty, Err with message if invalid
    pub fn set_prefix_override(&mut self, prefix: &str) -> Result<(), String> {
        let trimmed = prefix.trim();
        if trimmed.is_empty() {
            self.prefix_override = None;
            return Ok(());
        }
        match Self::validate_prefix(trimmed) {
            Some(valid) => {
                self.prefix_override = Some(valid);
                Ok(())
            }
            None => Err("Prefix must contain only uppercase letters (A-Z)".to_string()),
        }
    }

    /// Records a change to the requirement history
    pub fn record_change(&mut self, author: String, changes: Vec<FieldChange>) {
        if !changes.is_empty() {
            let entry = HistoryEntry::new(author, changes);
            self.history.push(entry);
            self.modified_at = Utc::now();
        }
    }

    /// Helper to create a field change
    pub fn field_change(field_name: &str, old_value: String, new_value: String) -> FieldChange {
        FieldChange {
            field_name: field_name.to_string(),
            old_value,
            new_value,
        }
    }

    /// Adds a top-level comment to this requirement
    pub fn add_comment(&mut self, comment: Comment) {
        self.comments.push(comment);
        self.modified_at = Utc::now();
    }

    /// Adds a reply to an existing comment
    pub fn add_reply(&mut self, parent_id: Uuid, reply: Comment) -> anyhow::Result<()> {
        for comment in &mut self.comments {
            if comment.id == parent_id {
                comment.add_reply(reply);
                self.modified_at = Utc::now();
                return Ok(());
            }
            if let Some(found) = comment.find_comment_mut(&parent_id) {
                found.add_reply(reply);
                self.modified_at = Utc::now();
                return Ok(());
            }
        }
        anyhow::bail!("Parent comment not found")
    }

    /// Finds a comment by ID (returns mutable reference)
    pub fn find_comment_mut(&mut self, comment_id: &Uuid) -> Option<&mut Comment> {
        for comment in &mut self.comments {
            if &comment.id == comment_id {
                return Some(comment);
            }
            if let Some(found) = comment.find_comment_mut(comment_id) {
                return Some(found);
            }
        }
        None
    }

    /// Deletes a comment by ID
    pub fn delete_comment(&mut self, comment_id: &Uuid) -> anyhow::Result<()> {
        // Try to find and remove from top-level
        if let Some(pos) = self.comments.iter().position(|c| &c.id == comment_id) {
            self.comments.remove(pos);
            self.modified_at = Utc::now();
            return Ok(());
        }

        // Search in nested replies
        for comment in &mut self.comments {
            if Comment::remove_reply_recursive(comment, comment_id) {
                self.modified_at = Utc::now();
                return Ok(());
            }
        }

        anyhow::bail!("Comment not found")
    }

    /// Compute a hash of the requirement content used for AI evaluation staleness detection
    /// The hash includes title, description, and type - fields that affect evaluation
    pub fn content_hash(&self) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        self.title.hash(&mut hasher);
        self.description.hash(&mut hasher);
        self.req_type.to_string().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Check if AI evaluation is needed (never evaluated or stale)
    pub fn needs_ai_evaluation(&self) -> bool {
        match &self.ai_evaluation {
            None => true,
            Some(eval) => eval.is_stale(&self.content_hash()),
        }
    }
}

// ============================================================================
// AI Prompt Configuration
// ============================================================================

/// Configuration for a single AI prompt action (evaluation, duplicates, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
pub struct AiActionPromptConfig {
    /// Custom template to replace the default prompt entirely.
    /// Use placeholders: {project_context}, {req_context}, {related_context}, {all_reqs}
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_template: Option<String>,

    /// Additional instructions appended to the default prompt.
    /// Used when custom_template is None.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub additional_instructions: String,
}

/// Per-type AI prompt customization
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
pub struct AiTypePromptConfig {
    /// The requirement type this config applies to (e.g., "Functional", "Epic", "Story")
    pub type_name: String,

    /// Extra instructions for evaluation prompts for this type
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evaluation_extra: String,

    /// Extra instructions for improve description prompts for this type
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub improve_extra: String,

    /// Extra instructions for generate children prompts for this type
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub generate_children_extra: String,
}

/// Complete AI prompt configuration for a project
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
pub struct AiPromptConfig {
    /// Global context prepended to ALL AI prompts.
    /// Use this to describe your project's methodology, terminology, or special rules.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub global_context: String,

    /// Configuration for the evaluation action
    #[serde(default, skip_serializing_if = "is_default_action_config")]
    pub evaluation: AiActionPromptConfig,

    /// Configuration for the find duplicates action
    #[serde(default, skip_serializing_if = "is_default_action_config")]
    pub duplicates: AiActionPromptConfig,

    /// Configuration for the suggest relationships action
    #[serde(default, skip_serializing_if = "is_default_action_config")]
    pub relationships: AiActionPromptConfig,

    /// Configuration for the improve description action
    #[serde(default, skip_serializing_if = "is_default_action_config")]
    pub improve: AiActionPromptConfig,

    /// Configuration for the generate children action
    #[serde(default, skip_serializing_if = "is_default_action_config")]
    pub generate_children: AiActionPromptConfig,

    /// Per-type customization
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_prompts: Vec<AiTypePromptConfig>,
}

/// Helper function for skip_serializing_if
fn is_default_action_config(config: &AiActionPromptConfig) -> bool {
    config.custom_template.is_none() && config.additional_instructions.is_empty()
}

impl AiPromptConfig {
    /// Get type-specific extra instructions for evaluation
    pub fn get_type_evaluation_extra(&self, type_name: &str) -> Option<&str> {
        self.type_prompts
            .iter()
            .find(|t| t.type_name == type_name)
            .filter(|t| !t.evaluation_extra.is_empty())
            .map(|t| t.evaluation_extra.as_str())
    }

    /// Get type-specific extra instructions for improve
    pub fn get_type_improve_extra(&self, type_name: &str) -> Option<&str> {
        self.type_prompts
            .iter()
            .find(|t| t.type_name == type_name)
            .filter(|t| !t.improve_extra.is_empty())
            .map(|t| t.improve_extra.as_str())
    }

    /// Get type-specific extra instructions for generate children
    pub fn get_type_generate_children_extra(&self, type_name: &str) -> Option<&str> {
        self.type_prompts
            .iter()
            .find(|t| t.type_name == type_name)
            .filter(|t| !t.generate_children_extra.is_empty())
            .map(|t| t.generate_children_extra.as_str())
    }
}

/// Collection of all requirements
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RequirementsStore {
    /// Database name (displayed in window title prefix)
    #[serde(default)]
    pub name: String,

    /// Database title (one-liner, displayed in window title)
    #[serde(default)]
    pub title: String,

    /// Database description (multi-line)
    #[serde(default)]
    pub description: String,

    pub requirements: Vec<Requirement>,

    /// Users in the system
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<User>,

    /// Teams in the system
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<Team>,

    /// ID system configuration
    #[serde(default)]
    pub id_config: IdConfiguration,

    /// Defined features with their prefixes
    #[serde(default)]
    pub features: Vec<FeatureDefinition>,

    /// Counter for feature numbers (used when creating new features)
    #[serde(default = "default_next_feature_number")]
    pub next_feature_number: u32,

    /// Global counter for requirement IDs (used with Global numbering strategy)
    #[serde(default = "default_next_spec_number")]
    pub next_spec_number: u32,

    /// Per-prefix counters for requirement IDs (used with PerPrefix numbering)
    /// Key is the prefix (e.g., "FR", "AUTH"), value is the next number
    #[serde(default)]
    pub prefix_counters: std::collections::HashMap<String, u32>,

    /// Relationship type definitions with constraints
    #[serde(default = "RelationshipDefinition::defaults")]
    pub relationship_definitions: Vec<RelationshipDefinition>,

    /// Reaction definitions for comments
    #[serde(default = "default_reaction_definitions")]
    pub reaction_definitions: Vec<ReactionDefinition>,

    /// Counter for meta-type IDs (users, views, etc.) - maps prefix to next number
    /// e.g., "$USER" -> 1 means next user will be $USER-001
    #[serde(default)]
    pub meta_counters: std::collections::HashMap<String, u32>,

    /// Custom type definitions with their statuses and fields
    #[serde(default = "default_type_definitions")]
    pub type_definitions: Vec<CustomTypeDefinition>,

    /// List of allowed/known ID prefixes for the project
    /// These are collected from usage and can be managed by admins
    #[serde(default)]
    pub allowed_prefixes: Vec<String>,

    /// Whether to restrict prefix selection to only allowed_prefixes
    /// When false, users can enter any valid prefix (which gets added to allowed_prefixes)
    /// When true, users must select from the allowed_prefixes list
    #[serde(default)]
    pub restrict_prefixes: bool,

    /// AI prompt configuration for customizing AI behavior
    #[serde(default, skip_serializing_if = "is_default_ai_prompt_config")]
    pub ai_prompts: AiPromptConfig,

    /// Baselines - named snapshots of requirements at specific points in time
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub baselines: Vec<Baseline>,

    /// Store version for detecting external modifications (SQLite only)
    /// Incremented on each save, used to detect if store was modified externally
    #[serde(skip)]
    pub store_version: i64,

    /// Migration marker - if set, indicates this YAML was migrated to another format
    /// Contains the path to the migrated database (e.g., "requirements.db")
    /// When this is set, opening the YAML should warn/redirect to the migrated database
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated_to: Option<String>,

    /// Optional external dispenser for ID generation (distributed mode).
    /// When set, ID generation delegates to this dispenser instead of using
    /// the internal next_spec_number / prefix_counters fields.
    /// Skipped in serialization — the dispenser has its own persistence.
    #[serde(skip)]
    #[ts(skip)]
    pub dispenser: Option<DispenserHandle>,
}

/// Wrapper for Arc<dyn Dispenser> that implements Debug and Clone.
/// This avoids requiring Debug on the Dispenser trait itself.
#[derive(Clone)]
pub struct DispenserHandle(pub std::sync::Arc<dyn crate::dispenser::Dispenser>);

impl std::fmt::Debug for DispenserHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DispenserHandle(<active>)")
    }
}

impl std::ops::Deref for DispenserHandle {
    type Target = dyn crate::dispenser::Dispenser;
    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

/// Helper function for skip_serializing_if on AiPromptConfig
fn is_default_ai_prompt_config(config: &AiPromptConfig) -> bool {
    config.global_context.is_empty()
        && is_default_action_config(&config.evaluation)
        && is_default_action_config(&config.duplicates)
        && is_default_action_config(&config.relationships)
        && is_default_action_config(&config.improve)
        && is_default_action_config(&config.generate_children)
        && config.type_prompts.is_empty()
}

/// Default value for next_feature_number
fn default_next_feature_number() -> u32 {
    1
}

/// Default value for next_spec_number
fn default_next_spec_number() -> u32 {
    1
}

/// Meta-type prefixes for special object types
pub const META_PREFIX_USER: &str = "$USER";
pub const META_PREFIX_VIEW: &str = "$VIEW";
pub const META_PREFIX_FEATURE: &str = "$FEAT";
pub const META_PREFIX_TEAM: &str = "$TEAM";

/// Per-requirement ID-generation snapshot: `(index, prefix, feature, spec_id)`.
/// Collected before mutation to avoid borrow conflicts during ID assignment.
type ReqIdData = Vec<(usize, Option<String>, Option<String>, Option<String>)>;

impl RequirementsStore {
    /// Creates an empty requirements store
    pub fn new() -> Self {
        Self {
            name: String::new(),
            title: String::new(),
            description: String::new(),
            requirements: Vec::new(),
            users: Vec::new(),
            teams: Vec::new(),
            id_config: IdConfiguration::default(),
            features: Vec::new(),
            next_feature_number: 1,
            next_spec_number: 1,
            prefix_counters: std::collections::HashMap::new(),
            relationship_definitions: RelationshipDefinition::defaults(),
            reaction_definitions: default_reaction_definitions(),
            meta_counters: std::collections::HashMap::new(),
            type_definitions: default_type_definitions(),
            allowed_prefixes: Vec::new(),
            restrict_prefixes: false,
            ai_prompts: AiPromptConfig::default(),
            baselines: Vec::new(),
            store_version: 1,
            migrated_to: None,
            dispenser: None,
        }
    }

    /// Gets the type definition for a requirement type
    pub fn get_type_definition(&self, req_type: &RequirementType) -> Option<&CustomTypeDefinition> {
        let type_name = match req_type {
            RequirementType::Functional => "Functional",
            RequirementType::NonFunctional => "NonFunctional",
            RequirementType::System => "System",
            RequirementType::User => "User",
            RequirementType::ChangeRequest => "ChangeRequest",
            RequirementType::Bug => "Bug",
            RequirementType::Epic => "Epic",
            RequirementType::Story => "Story",
            RequirementType::Task => "Task",
            RequirementType::Spike => "Spike",
            RequirementType::Sprint => "Sprint",
            RequirementType::Folder => "Folder",
            RequirementType::Meta => "Meta",
            RequirementType::Principle => "Principle",
            RequirementType::Vision => "Vision",
            RequirementType::Constraint => "Constraint",
            RequirementType::Decision => "Decision",
            RequirementType::Term => "Term",
            RequirementType::Doc => "Doc",
        };
        self.type_definitions.iter().find(|td| td.name == type_name)
    }

    /// Gets the available statuses for a requirement type
    pub fn get_statuses_for_type(&self, req_type: &RequirementType) -> Vec<String> {
        self.get_type_definition(req_type)
            .map(|td| td.get_statuses())
            .unwrap_or_else(|| {
                vec![
                    "Draft".to_string(),
                    "Approved".to_string(),
                    "Completed".to_string(),
                    "Rejected".to_string(),
                ]
            })
    }

    /// Gets the available priorities for a requirement type
    pub fn get_priorities_for_type(&self, req_type: &RequirementType) -> Vec<String> {
        self.get_type_definition(req_type)
            .map(|td| td.get_priorities())
            .unwrap_or_else(|| vec!["High".to_string(), "Medium".to_string(), "Low".to_string()])
    }

    /// Gets the custom field definitions for a requirement type
    pub fn get_custom_fields_for_type(
        &self,
        req_type: &RequirementType,
    ) -> Vec<CustomFieldDefinition> {
        self.get_type_definition(req_type)
            .map(|td| {
                let mut fields = td.custom_fields.clone();
                fields.sort_by_key(|f| f.order);
                fields
            })
            .unwrap_or_default()
    }

    /// Checks if a requirement type is stateless (no status/priority tracking)
    pub fn is_type_stateless(&self, req_type: &RequirementType) -> bool {
        self.get_type_definition(req_type)
            .map(|td| td.stateless)
            .unwrap_or(false)
    }

    // ========================================================================
    // Sprint Planning Methods
    // ========================================================================

    /// Get all Sprints, sorted by sprint_number custom field (if available)
    pub fn get_sprints(&self) -> Vec<&Requirement> {
        let mut sprints: Vec<&Requirement> = self
            .requirements
            .iter()
            .filter(|r| r.req_type == RequirementType::Sprint)
            .collect();

        // Sort by sprint_number if available, otherwise by created_at
        sprints.sort_by(|a, b| {
            let a_num = a
                .custom_fields
                .get("sprint_number")
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(i32::MAX);
            let b_num = b
                .custom_fields
                .get("sprint_number")
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(i32::MAX);
            a_num.cmp(&b_num)
        });

        sprints
    }

    /// Get items assigned to a specific Sprint via sprint_assignment relationship
    pub fn get_sprint_items(&self, sprint_id: &Uuid) -> Vec<&Requirement> {
        self.requirements
            .iter()
            .filter(|r| {
                r.relationships.iter().any(|rel| {
                    rel.rel_type == RelationshipType::Custom("sprint_assignment".to_string())
                        && rel.target_id == *sprint_id
                })
            })
            .collect()
    }

    /// Get the Sprint that a requirement is assigned to (if any)
    pub fn get_requirement_sprint(&self, req_id: &Uuid) -> Option<&Requirement> {
        let req = self.get_requirement_by_id(req_id)?;
        let sprint_rel = req.relationships.iter().find(|rel| {
            rel.rel_type == RelationshipType::Custom("sprint_assignment".to_string())
        })?;
        self.get_requirement_by_id(&sprint_rel.target_id)
    }

    /// Get backlog items (requirements without Sprint assignment, excluding Sprints and Folders)
    pub fn get_backlog(&self) -> Vec<&Requirement> {
        self.requirements
            .iter()
            .filter(|r| {
                // Exclude Sprint, Folder, and Meta types
                r.req_type != RequirementType::Sprint
                    && r.req_type != RequirementType::Folder
                    && r.req_type != RequirementType::Meta
                    // Has no sprint_assignment relationship
                    && !r.relationships.iter().any(|rel| {
                        rel.rel_type == RelationshipType::Custom("sprint_assignment".to_string())
                    })
            })
            .collect()
    }

    /// Assign a requirement to a Sprint
    /// This removes any existing sprint assignment and creates a new one
    pub fn assign_to_sprint(&mut self, req_id: Uuid, sprint_id: Uuid, username: &str) {
        // First, remove any existing sprint assignment
        if let Some(req) = self.requirements.iter_mut().find(|r| r.id == req_id) {
            req.relationships.retain(|rel| {
                rel.rel_type != RelationshipType::Custom("sprint_assignment".to_string())
            });

            // Add the new sprint assignment
            req.relationships.push(Relationship {
                rel_type: RelationshipType::Custom("sprint_assignment".to_string()),
                target_id: sprint_id,
                created_at: Some(Utc::now()),
                created_by: Some(username.to_string()),
            });

            // Update modified timestamp
            req.modified_at = Utc::now();

            // Add to history
            let sprint = self.requirements.iter().find(|r| r.id == sprint_id);
            let sprint_name = sprint
                .and_then(|s| s.spec_id.clone())
                .unwrap_or_else(|| sprint_id.to_string());

            let history_entry = HistoryEntry::new(
                username.to_string(),
                vec![FieldChange {
                    field_name: "sprint_assignment".to_string(),
                    old_value: String::new(),
                    new_value: sprint_name,
                }],
            );

            if let Some(req) = self.requirements.iter_mut().find(|r| r.id == req_id) {
                req.history.push(history_entry);
            }
        }

        // Also add the inverse relationship (sprint_contains) to the sprint
        if let Some(sprint) = self.requirements.iter_mut().find(|r| r.id == sprint_id) {
            // Check if this relationship already exists
            if !sprint.relationships.iter().any(|rel| {
                rel.rel_type == RelationshipType::Custom("sprint_contains".to_string())
                    && rel.target_id == req_id
            }) {
                sprint.relationships.push(Relationship {
                    rel_type: RelationshipType::Custom("sprint_contains".to_string()),
                    target_id: req_id,
                    created_at: Some(Utc::now()),
                    created_by: Some(username.to_string()),
                });
            }
        }
    }

    /// Remove a requirement from its Sprint assignment
    pub fn remove_from_sprint(&mut self, req_id: Uuid, username: &str) {
        // Get the current sprint assignment for history
        let current_sprint_id = self
            .requirements
            .iter()
            .find(|r| r.id == req_id)
            .and_then(|req| {
                req.relationships.iter().find_map(|rel| {
                    if rel.rel_type == RelationshipType::Custom("sprint_assignment".to_string()) {
                        Some(rel.target_id)
                    } else {
                        None
                    }
                })
            });

        if let Some(sprint_id) = current_sprint_id {
            // Remove from requirement
            if let Some(req) = self.requirements.iter_mut().find(|r| r.id == req_id) {
                req.relationships.retain(|rel| {
                    rel.rel_type != RelationshipType::Custom("sprint_assignment".to_string())
                });
                req.modified_at = Utc::now();

                // Add to history
                let history_entry = HistoryEntry::new(
                    username.to_string(),
                    vec![FieldChange {
                        field_name: "sprint_assignment".to_string(),
                        old_value: sprint_id.to_string(),
                        new_value: String::new(),
                    }],
                );
                req.history.push(history_entry);
            }

            // Remove inverse relationship from sprint
            if let Some(sprint) = self.requirements.iter_mut().find(|r| r.id == sprint_id) {
                sprint.relationships.retain(|rel| {
                    !(rel.rel_type == RelationshipType::Custom("sprint_contains".to_string())
                        && rel.target_id == req_id)
                });
            }
        }
    }

    /// Gets all unique prefixes currently in use from requirements
    pub fn get_used_prefixes(&self) -> Vec<String> {
        let mut prefixes: std::collections::HashSet<String> = std::collections::HashSet::new();

        for req in &self.requirements {
            if let Some(ref spec_id) = req.spec_id {
                // Extract prefix from spec_id (e.g., "SEC-001" -> "SEC")
                if let Some(prefix) = spec_id.split('-').next() {
                    // Skip meta-type prefixes like $USER, $VIEW
                    if !prefix.starts_with('$') {
                        prefixes.insert(prefix.to_string());
                    }
                }
            }
        }

        let mut result: Vec<String> = prefixes.into_iter().collect();
        result.sort();
        result
    }

    /// Gets all allowed prefixes (combines allowed_prefixes with used prefixes)
    pub fn get_all_prefixes(&self) -> Vec<String> {
        let mut prefixes: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Add explicitly allowed prefixes
        for p in &self.allowed_prefixes {
            prefixes.insert(p.clone());
        }

        // Add prefixes currently in use
        for p in self.get_used_prefixes() {
            prefixes.insert(p);
        }

        let mut result: Vec<String> = prefixes.into_iter().collect();
        result.sort();
        result
    }

    /// Adds a prefix to the allowed list if not already present
    pub fn add_allowed_prefix(&mut self, prefix: &str) {
        let prefix = prefix.to_uppercase();
        if !self.allowed_prefixes.contains(&prefix) {
            self.allowed_prefixes.push(prefix);
            self.allowed_prefixes.sort();
        }
    }

    /// Removes a prefix from the allowed list
    pub fn remove_allowed_prefix(&mut self, prefix: &str) {
        self.allowed_prefixes.retain(|p| p != prefix);
    }

    /// Checks if a prefix is allowed (always true if restrict_prefixes is false)
    pub fn is_prefix_allowed(&self, prefix: &str) -> bool {
        if !self.restrict_prefixes {
            return true;
        }
        self.allowed_prefixes
            .iter()
            .any(|p| p.eq_ignore_ascii_case(prefix))
    }

    /// Generates the next meta-type ID for a given prefix (e.g., "$USER" -> "$USER-001")
    pub fn next_meta_id(&mut self, prefix: &str) -> String {
        if let Some(ref dispenser) = self.dispenser {
            if let Ok(id) = dispenser.next_id(prefix) {
                return id;
            }
        }
        let counter = self.meta_counters.entry(prefix.to_string()).or_insert(1);
        let num = *counter;
        *counter += 1;
        format!("{}-{:03}", prefix, num)
    }

    /// Adds a requirement to the store
    pub fn add_requirement(&mut self, req: Requirement) {
        self.requirements.push(req);
    }

    /// Adds a user to the store (legacy - no spec_id)
    pub fn add_user(&mut self, user: User) {
        self.users.push(user);
    }

    /// Adds a user with auto-generated $USER-XXX spec_id
    pub fn add_user_with_id(&mut self, name: String, email: String, handle: String) -> String {
        let spec_id = self.next_meta_id(META_PREFIX_USER);
        let user = User::new_with_spec_id(name, email, handle, spec_id.clone());
        self.users.push(user);
        spec_id
    }

    /// Finds a user by spec_id (e.g., "$USER-001")
    pub fn find_user_by_spec_id(&self, spec_id: &str) -> Option<&User> {
        self.users
            .iter()
            .find(|u| u.spec_id.as_deref() == Some(spec_id))
    }

    /// Finds a user by spec_id (mutable)
    pub fn find_user_by_spec_id_mut(&mut self, spec_id: &str) -> Option<&mut User> {
        self.users
            .iter_mut()
            .find(|u| u.spec_id.as_deref() == Some(spec_id))
    }

    /// Finds a user by UUID
    pub fn find_user_by_id(&self, id: &Uuid) -> Option<&User> {
        self.users.iter().find(|u| u.id == *id)
    }

    /// Migrates existing users without spec_id to have $USER-XXX IDs
    pub fn migrate_users_to_spec_ids(&mut self) {
        for user in &mut self.users {
            if user.spec_id.is_none() {
                let counter = self
                    .meta_counters
                    .entry(META_PREFIX_USER.to_string())
                    .or_insert(1);
                let spec_id = format!("{}-{:03}", META_PREFIX_USER, *counter);
                *counter += 1;
                user.spec_id = Some(spec_id);
            }
        }
    }

    /// Gets a mutable reference to a user by ID
    pub fn get_user_by_id_mut(&mut self, id: &Uuid) -> Option<&mut User> {
        self.users.iter_mut().find(|u| &u.id == id)
    }

    /// Removes a user by ID
    pub fn remove_user(&mut self, id: &Uuid) -> bool {
        if let Some(pos) = self.users.iter().position(|u| &u.id == id) {
            self.users.remove(pos);
            true
        } else {
            false
        }
    }

    // ==================== Team Management ====================

    /// Adds a team to the store (legacy - no spec_id)
    pub fn add_team(&mut self, team: Team) {
        self.teams.push(team);
    }

    /// Adds a team with auto-generated $TEAM-XXX spec_id
    pub fn add_team_with_id(
        &mut self,
        name: String,
        description: String,
        parent_team_id: Option<Uuid>,
    ) -> String {
        let spec_id = self.next_meta_id(META_PREFIX_TEAM);
        let team = Team::new_with_spec_id(name, description, parent_team_id, spec_id.clone());
        self.teams.push(team);
        spec_id
    }

    /// Finds a team by spec_id (e.g., "$TEAM-001")
    pub fn find_team_by_spec_id(&self, spec_id: &str) -> Option<&Team> {
        self.teams
            .iter()
            .find(|t| t.spec_id.as_deref() == Some(spec_id))
    }

    /// Finds a team by spec_id (mutable)
    pub fn find_team_by_spec_id_mut(&mut self, spec_id: &str) -> Option<&mut Team> {
        self.teams
            .iter_mut()
            .find(|t| t.spec_id.as_deref() == Some(spec_id))
    }

    /// Finds a team by UUID
    pub fn find_team_by_id(&self, id: &Uuid) -> Option<&Team> {
        self.teams.iter().find(|t| t.id == *id)
    }

    /// Gets a mutable reference to a team by ID
    pub fn get_team_by_id_mut(&mut self, id: &Uuid) -> Option<&mut Team> {
        self.teams.iter_mut().find(|t| &t.id == id)
    }

    /// Removes a team by ID
    pub fn remove_team(&mut self, id: &Uuid) -> bool {
        if let Some(pos) = self.teams.iter().position(|t| &t.id == id) {
            self.teams.remove(pos);
            true
        } else {
            false
        }
    }

    /// Gets child teams for a given parent team
    pub fn get_child_teams(&self, parent_id: &Uuid) -> Vec<&Team> {
        self.teams
            .iter()
            .filter(|t| t.parent_team_id.as_ref() == Some(parent_id))
            .collect()
    }

    /// Gets top-level teams (teams without a parent)
    pub fn get_root_teams(&self) -> Vec<&Team> {
        self.teams
            .iter()
            .filter(|t| t.parent_team_id.is_none())
            .collect()
    }

    /// Gets all teams a user belongs to
    pub fn get_teams_for_user(&self, user_id: &Uuid) -> Vec<&Team> {
        self.teams
            .iter()
            .filter(|t| t.member_ids.contains(user_id))
            .collect()
    }

    /// Checks if setting a parent team would create a circular reference
    pub fn would_create_team_cycle(&self, team_id: &Uuid, proposed_parent_id: &Uuid) -> bool {
        // If team_id equals proposed_parent_id, it's a direct cycle
        if team_id == proposed_parent_id {
            return true;
        }

        // Walk up the parent chain from proposed_parent_id
        let mut current_id = Some(*proposed_parent_id);
        while let Some(id) = current_id {
            if &id == team_id {
                return true;
            }
            current_id = self.find_team_by_id(&id).and_then(|t| t.parent_team_id);
        }
        false
    }

    /// Migrates existing teams without spec_id to have $TEAM-XXX IDs
    pub fn migrate_teams_to_spec_ids(&mut self) {
        for team in &mut self.teams {
            if team.spec_id.is_none() {
                let counter = self
                    .meta_counters
                    .entry(META_PREFIX_TEAM.to_string())
                    .or_insert(1);
                let spec_id = format!("{}-{:03}", META_PREFIX_TEAM, *counter);
                *counter += 1;
                team.spec_id = Some(spec_id);
            }
        }
    }

    // ==================== End Team Management ====================

    /// Gets a requirement by ID
    pub fn get_requirement_by_id(&self, id: &Uuid) -> Option<&Requirement> {
        self.requirements.iter().find(|r| r.id == *id)
    }

    /// Gets a mutable reference to a requirement by ID
    pub fn get_requirement_by_id_mut(&mut self, id: &Uuid) -> Option<&mut Requirement> {
        self.requirements.iter_mut().find(|r| r.id == *id)
    }

    /// Gets the next feature number and increments the counter
    pub fn get_next_feature_number(&mut self) -> u32 {
        let current_number = self.next_feature_number;
        self.next_feature_number += 1;
        current_number
    }

    /// Formats a feature with number prefix
    pub fn format_feature_with_number(&self, feature_name: &str) -> String {
        format!("{}-{}", self.next_feature_number, feature_name)
    }

    /// Gets all unique feature names
    pub fn get_feature_names(&self) -> Vec<String> {
        let mut feature_names = Vec::new();

        for req in &self.requirements {
            // Skip feature if it's already in the list
            if feature_names.contains(&req.feature) {
                continue;
            }

            feature_names.push(req.feature.clone());
        }

        // Sort features by their prefix number if they have one
        feature_names.sort_by(|a, b| {
            let a_parts: Vec<&str> = a.splitn(2, '-').collect();
            let b_parts: Vec<&str> = b.splitn(2, '-').collect();

            // If both have prefix numbers, compare them numerically
            if a_parts.len() > 1 && b_parts.len() > 1 {
                if let (Ok(a_num), Ok(b_num)) =
                    (a_parts[0].parse::<u32>(), b_parts[0].parse::<u32>())
                {
                    return a_num.cmp(&b_num);
                }
            }

            // Otherwise, lexicographical comparison
            a.cmp(b)
        });

        feature_names
    }

    /// Updates an existing feature name
    pub fn update_feature_name(&mut self, old_name: &str, new_name: &str) {
        for req in &mut self.requirements {
            if req.feature == old_name {
                req.feature = new_name.to_string();
            }
        }
    }

    /// Migrate existing features to use numbered prefixes
    pub fn migrate_features(&mut self) {
        // First, collect all unique features
        let mut unique_features: Vec<String> = Vec::new();

        for req in &self.requirements {
            // Skip if already has a number prefix (format: "1-Feature")
            if req.feature.contains('-') {
                if let Some((prefix, _)) = req.feature.split_once('-') {
                    if prefix.parse::<u32>().is_ok() {
                        continue; // Already has a number prefix
                    }
                }
            }

            if !unique_features.contains(&req.feature) {
                unique_features.push(req.feature.clone());
            }
        }

        // Assign numbers to each unique feature
        for feature in unique_features {
            let number = self.get_next_feature_number();
            let new_name = format!("{}-{}", number, feature);

            // Update all requirements with this feature
            self.update_feature_name(&feature, &new_name);
        }
    }

    /// Gets a requirement by SPEC-ID. Match is case-insensitive on the
    /// spec_id and the agreed short id, so callers may pass user input
    /// (e.g. "fr-1") without canonicalizing first.
    pub fn get_requirement_by_spec_id(&self, spec_id: &str) -> Option<&Requirement> {
        self.requirements.iter().find(|r| {
            r.spec_id
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
                || r.agreed_id
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
        })
    }

    /// Gets a mutable reference to a requirement by SPEC-ID. Same matching
    /// rules as `get_requirement_by_spec_id`.
    pub fn get_requirement_by_spec_id_mut(&mut self, spec_id: &str) -> Option<&mut Requirement> {
        self.requirements.iter_mut().find(|r| {
            r.spec_id
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
                || r.agreed_id
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
        })
    }

    /// Assigns SPEC-IDs to requirements that don't have them
    pub fn assign_spec_ids(&mut self) {
        for req in &mut self.requirements {
            if req.spec_id.is_none() {
                req.spec_id = Some(format!("SPEC-{:03}", self.next_spec_number));
                self.next_spec_number += 1;
            }
        }
    }

    /// Gets the next SPEC-ID that would be assigned
    pub fn peek_next_spec_id(&self) -> String {
        format!("SPEC-{:03}", self.next_spec_number)
    }

    /// Validates that all SPEC-IDs are unique
    pub fn validate_unique_spec_ids(&self) -> anyhow::Result<()> {
        use std::collections::HashSet;
        let mut seen = HashSet::new();

        for req in &self.requirements {
            if let Some(spec_id) = &req.spec_id {
                if !seen.insert(spec_id) {
                    anyhow::bail!("Duplicate SPEC-ID found: {}", spec_id);
                }
            }
        }

        Ok(())
    }

    /// Repairs duplicate SPEC-IDs by assigning new unique IDs to duplicates
    /// Keeps the first occurrence of each SPEC-ID, reassigns duplicates
    /// Returns the number of duplicates that were repaired
    pub fn repair_duplicate_spec_ids(&mut self) -> usize {
        use std::collections::HashSet;
        let mut seen: HashSet<String> = HashSet::new();
        let mut duplicates: Vec<(usize, String)> = Vec::new();

        // First pass: find all duplicates (indices and their spec_id prefixes)
        for (idx, req) in self.requirements.iter().enumerate() {
            if let Some(spec_id) = &req.spec_id {
                if !seen.insert(spec_id.clone()) {
                    // This is a duplicate - extract the prefix
                    let prefix = Self::extract_prefix_from_spec_id(spec_id);
                    duplicates.push((idx, prefix));
                }
            }
        }

        if duplicates.is_empty() {
            return 0;
        }

        // Log duplicates found (for CLI output)
        eprintln!(
            "Found {} duplicate SPEC-ID(s), automatically repairing...",
            duplicates.len()
        );

        // Second pass: assign new unique IDs to duplicates
        // We need to collect these changes because we're modifying self
        let repairs: Vec<(usize, String)> = duplicates
            .iter()
            .map(|(idx, prefix)| {
                let new_id = self.generate_requirement_id_with_override(prefix);
                (*idx, new_id)
            })
            .collect();

        // Apply the repairs
        for (idx, new_id) in &repairs {
            if let Some(req) = self.requirements.get_mut(*idx) {
                let old_id = req.spec_id.clone().unwrap_or_default();
                eprintln!("  Repaired: {} -> {} ({})", old_id, new_id, req.title);
                req.spec_id = Some(new_id.clone());
                // Add the new ID to seen set so we don't create new duplicates
                seen.insert(new_id.clone());
            }
        }

        repairs.len()
    }

    /// Extract the prefix from a spec_id (e.g., "FR-0042" -> "FR", "AUTH-REQ-001" -> "AUTH-REQ")
    fn extract_prefix_from_spec_id(spec_id: &str) -> String {
        // Find the last '-' followed by digits
        if let Some(last_dash_pos) = spec_id.rfind('-') {
            let after_dash = &spec_id[last_dash_pos + 1..];
            if after_dash.chars().all(|c| c.is_ascii_digit()) {
                // Return everything before the last dash as the prefix
                return spec_id[..last_dash_pos].to_string();
            }
        }
        // Fallback: use the whole thing as-is or default to "REQ"
        if spec_id.is_empty() {
            "REQ".to_string()
        } else {
            spec_id.to_string()
        }
    }

    /// Adds a requirement and assigns it a SPEC-ID (legacy method for backward compatibility)
    pub fn add_requirement_with_spec_id(&mut self, mut req: Requirement) {
        if req.spec_id.is_none() {
            req.spec_id = Some(format!("SPEC-{:03}", self.next_spec_number));
            self.next_spec_number += 1;
        }
        self.requirements.push(req);
    }

    /// Migrates type definitions by adding any missing built-in types
    /// Returns true if any types were added
    pub fn migrate_type_definitions(&mut self) -> bool {
        let defaults = default_type_definitions();
        let mut added = false;

        for default_type in defaults {
            // Only add built-in types that are missing
            if default_type.built_in {
                let exists = self
                    .type_definitions
                    .iter()
                    .any(|t| t.name == default_type.name);
                if !exists {
                    self.type_definitions.push(default_type);
                    added = true;
                }
            }
        }

        added
    }

    /// Migrates id_config.requirement_types by adding any missing built-in types
    /// This ensures CLI type list shows all built-in types
    /// Returns true if any types were added
    // trace:FR-0309 | ai:claude:high
    pub fn migrate_id_config_types(&mut self) -> bool {
        let defaults = default_requirement_types();
        let mut added = false;

        for default_type in defaults {
            let exists = self.id_config.requirement_types.iter().any(|t| {
                t.name.to_lowercase() == default_type.name.to_lowercase()
                    || t.prefix == default_type.prefix
            });
            if !exists {
                self.id_config.requirement_types.push(default_type);
                added = true;
            }
        }

        added
    }

    // ========================================================================
    // New ID System Methods
    // ========================================================================

    /// Add a new feature definition
    /// Returns error if the prefix is reserved or already in use
    pub fn add_feature(&mut self, name: &str, prefix: &str) -> anyhow::Result<FeatureDefinition> {
        let prefix_upper = prefix.to_uppercase();

        // Check if prefix is reserved by a requirement type
        if self.id_config.is_prefix_reserved(&prefix_upper) {
            anyhow::bail!(
                "Prefix '{}' is reserved for requirement type '{}'",
                prefix_upper,
                self.id_config
                    .get_type_by_prefix(&prefix_upper)
                    .map(|t| t.name.as_str())
                    .unwrap_or("unknown")
            );
        }

        // Check if prefix is already used by another feature
        if self.features.iter().any(|f| f.prefix == prefix_upper) {
            anyhow::bail!(
                "Prefix '{}' is already used by another feature",
                prefix_upper
            );
        }

        let feature = FeatureDefinition::new(self.next_feature_number, name, &prefix_upper);
        self.next_feature_number += 1;
        self.features.push(feature.clone());
        Ok(feature)
    }

    /// Get a feature by name
    pub fn get_feature_by_name(&self, name: &str) -> Option<&FeatureDefinition> {
        let lower = name.to_lowercase();
        self.features
            .iter()
            .find(|f| f.name.to_lowercase() == lower)
    }

    /// Get a feature by prefix
    pub fn get_feature_by_prefix(&self, prefix: &str) -> Option<&FeatureDefinition> {
        let upper = prefix.to_uppercase();
        self.features.iter().find(|f| f.prefix == upper)
    }

    /// Get the next counter value for a given prefix.
    /// If a dispenser is set, delegates to it. Otherwise uses internal counters.
    fn get_next_counter_for_prefix(&mut self, prefix: &str) -> u32 {
        if let Some(ref dispenser) = self.dispenser {
            return dispenser.next(prefix).unwrap_or_else(|_| {
                // Fallback to internal counter if dispenser fails
                let upper = prefix.to_uppercase();
                let counter = self.prefix_counters.entry(upper).or_insert(1);
                let current = *counter;
                *counter += 1;
                current
            });
        }
        let upper = prefix.to_uppercase();
        let counter = self.prefix_counters.entry(upper).or_insert(1);
        let current = *counter;
        *counter += 1;
        current
    }

    /// Get the next global sequence number.
    /// If a dispenser is set, delegates to it (using "GLOBAL" as the type key).
    /// Otherwise uses internal next_spec_number counter.
    fn get_next_global_number(&mut self) -> u32 {
        if let Some(ref dispenser) = self.dispenser {
            return dispenser.next("GLOBAL").unwrap_or_else(|_| {
                let n = self.next_spec_number;
                self.next_spec_number += 1;
                n
            });
        }
        let n = self.next_spec_number;
        self.next_spec_number += 1;
        n
    }

    /// Generate a new requirement ID based on configuration
    /// - feature_prefix: Optional feature prefix (e.g., "AUTH")
    /// - type_prefix: Optional type prefix (e.g., "FR")
    pub fn generate_requirement_id(
        &mut self,
        feature_prefix: Option<&str>,
        type_prefix: Option<&str>,
    ) -> String {
        let digits = self.id_config.digits;

        match self.id_config.format {
            IdFormat::SingleLevel => {
                // Use either feature or type prefix, type takes precedence
                let prefix = type_prefix
                    .or(feature_prefix)
                    .map(|s| s.to_uppercase())
                    .unwrap_or_else(|| "REQ".to_string());

                let number = match self.id_config.numbering {
                    NumberingStrategy::Global => self.get_next_global_number(),
                    NumberingStrategy::PerPrefix | NumberingStrategy::PerFeatureType => {
                        self.get_next_counter_for_prefix(&prefix)
                    }
                };

                // If we have a dispenser in distributed mode, use its formatting
                if let Some(ref dispenser) = self.dispenser {
                    if let Ok(id) = dispenser.format_id(&prefix, number) {
                        return id;
                    }
                }

                format!("{}-{:0>width$}", prefix, number, width = digits as usize)
            }
            IdFormat::TwoLevel => {
                let feat = feature_prefix
                    .map(|s| s.to_uppercase())
                    .unwrap_or_else(|| "GEN".to_string()); // GEN = General
                let typ = type_prefix
                    .map(|s| s.to_uppercase())
                    .unwrap_or_else(|| "REQ".to_string());

                let number = match self.id_config.numbering {
                    NumberingStrategy::Global => self.get_next_global_number(),
                    NumberingStrategy::PerPrefix => {
                        // Per feature prefix only
                        self.get_next_counter_for_prefix(&feat)
                    }
                    NumberingStrategy::PerFeatureType => {
                        // Per feature+type combination
                        let combo_key = format!("{}-{}", feat, typ);
                        self.get_next_counter_for_prefix(&combo_key)
                    }
                };

                format!(
                    "{}-{}-{:0>width$}",
                    feat,
                    typ,
                    number,
                    width = digits as usize
                )
            }
        }
    }

    /// Add a requirement with the new ID system
    /// If spec_id is already set, uses that; otherwise generates one
    /// If prefix_override is set on the requirement, uses that prefix instead of feature/type
    pub fn add_requirement_with_id(
        &mut self,
        mut req: Requirement,
        feature_prefix: Option<&str>,
        type_prefix: Option<&str>,
    ) {
        if req.spec_id.is_none() {
            // Check if requirement has a prefix override
            if let Some(ref override_prefix) = req.prefix_override {
                req.spec_id = Some(self.generate_requirement_id_with_override(override_prefix));
            } else {
                req.spec_id = Some(self.generate_requirement_id(feature_prefix, type_prefix));
            }
        }
        self.requirements.push(req);
    }

    /// Generate a requirement ID using an explicit prefix override
    /// Uses SingleLevel format with the override prefix, respects numbering strategy
    fn generate_requirement_id_with_override(&mut self, prefix: &str) -> String {
        let prefix_upper = prefix.to_uppercase();
        let digits = self.id_config.digits;

        let number = match self.id_config.numbering {
            NumberingStrategy::Global => self.get_next_global_number(),
            NumberingStrategy::PerPrefix | NumberingStrategy::PerFeatureType => {
                // Treat the override prefix as its own counter
                self.get_next_counter_for_prefix(&prefix_upper)
            }
        };

        // If we have a dispenser in distributed mode, use its formatting
        if let Some(ref dispenser) = self.dispenser {
            if let Ok(id) = dispenser.format_id(&prefix_upper, number) {
                return id;
            }
        }

        format!(
            "{}-{:0>width$}",
            prefix_upper,
            number,
            width = digits as usize
        )
    }

    /// Get the type prefix for a RequirementType enum value
    /// Falls back to built-in defaults if the type is not in the database
    // trace:BUG-0308 | ai:claude:high
    pub fn get_type_prefix(&self, req_type: &RequirementType) -> Option<String> {
        // Map enum to type name and fallback prefix
        let (type_name, fallback_prefix) = match req_type {
            RequirementType::Functional => ("Functional", "FR"),
            RequirementType::NonFunctional => ("Non-Functional", "NFR"),
            RequirementType::System => ("System", "SR"),
            RequirementType::User => ("User", "UR"),
            RequirementType::ChangeRequest => ("Change Request", "CR"),
            RequirementType::Bug => ("Bug", "BUG"),
            RequirementType::Epic => ("Epic", "EPIC"),
            RequirementType::Story => ("Story", "STORY"),
            RequirementType::Task => ("Task", "TASK"),
            RequirementType::Spike => ("Spike", "SPIKE"),
            RequirementType::Sprint => ("Sprint", "SPRINT"),
            RequirementType::Folder => ("Folder", "FOLDER"),
            RequirementType::Meta => ("Meta", "META"),
            // trace:FR-1-074 | ai:claude
            RequirementType::Principle => ("Principle", "PRIN"),
            RequirementType::Vision => ("Vision", "VIS"),
            RequirementType::Constraint => ("Constraint", "CON"),
            RequirementType::Decision => ("Decision", "ADR"),
            RequirementType::Term => ("Term", "TERM"),
            RequirementType::Doc => ("Doc", "DOC"),
        };
        // Try database first, fall back to built-in prefix
        self.id_config
            .get_type_by_name(type_name)
            .map(|t| t.prefix.clone())
            .or_else(|| Some(fallback_prefix.to_string()))
    }

    /// Generate a new spec_id for a requirement with a new prefix override
    /// Returns Ok(new_spec_id) if successful, Err if the new ID would conflict
    pub fn regenerate_spec_id_for_prefix_change(
        &mut self,
        req_uuid: &Uuid,
        new_prefix: Option<&str>,
        feature_prefix: Option<&str>,
        type_prefix: Option<&str>,
    ) -> Result<String, String> {
        // Generate the new ID
        let new_spec_id = if let Some(prefix) = new_prefix {
            self.generate_requirement_id_with_override(prefix)
        } else {
            self.generate_requirement_id(feature_prefix, type_prefix)
        };

        // Check if this ID is already taken by another requirement
        let conflicts = self
            .requirements
            .iter()
            .any(|r| r.id != *req_uuid && r.spec_id.as_deref() == Some(&new_spec_id));

        if conflicts {
            Err(format!(
                "ID '{}' is already in use by another requirement",
                new_spec_id
            ))
        } else {
            Ok(new_spec_id)
        }
    }

    /// Check if a spec_id is available (not used by any requirement, or only by the given UUID)
    pub fn is_spec_id_available(&self, spec_id: &str, exclude_uuid: Option<&Uuid>) -> bool {
        !self.requirements.iter().any(|r| {
            r.spec_id.as_deref() == Some(spec_id) && exclude_uuid.is_none_or(|uuid| r.id != *uuid)
        })
    }

    /// Update a requirement's spec_id when its type changes
    /// Replaces the type prefix portion while keeping the number
    pub fn update_spec_id_for_type_change(
        &self,
        current_spec_id: Option<&str>,
        new_type: &RequirementType,
    ) -> Option<String> {
        let spec_id = current_spec_id?;
        let new_prefix = self.get_type_prefix(new_type)?;

        // Parse the current spec_id to extract the number
        // Formats: "PREFIX-NNN" (SingleLevel) or "FEATURE-TYPE-NNN" (TwoLevel)
        let parts: Vec<&str> = spec_id.split('-').collect();

        match self.id_config.format {
            IdFormat::SingleLevel => {
                // Format: PREFIX-NNN
                if parts.len() >= 2 {
                    let number = parts.last()?;
                    Some(format!("{}-{}", new_prefix, number))
                } else {
                    None
                }
            }
            IdFormat::TwoLevel => {
                // Format: FEATURE-TYPE-NNN
                if parts.len() >= 3 {
                    let feature = parts[0];
                    let number = parts.last()?;
                    Some(format!("{}-{}-{}", feature, new_prefix, number))
                } else {
                    None
                }
            }
        }
    }

    /// Migrate all existing SPEC-XXX IDs to the new format
    /// This will regenerate all IDs based on the current configuration
    /// Requirements with prefix_override will use their override prefix
    pub fn migrate_to_new_id_format(&mut self) {
        // Reset counters
        self.next_spec_number = 1;
        self.prefix_counters.clear();

        // Clear all spec_ids first
        for req in &mut self.requirements {
            req.spec_id = None;
        }

        // Collect data needed for ID generation (to avoid borrow issues)
        let req_data: ReqIdData = self
            .requirements
            .iter()
            .enumerate()
            .map(|(i, req)| {
                // Check for prefix_override first
                let prefix_override = req.prefix_override.clone();

                let feature_prefix = self
                    .features
                    .iter()
                    .find(|f| req.feature.contains(&f.name))
                    .map(|f| f.prefix.clone());
                let type_prefix = match req.req_type {
                    RequirementType::Functional => Some("FR".to_string()),
                    RequirementType::NonFunctional => Some("NFR".to_string()),
                    RequirementType::System => Some("SR".to_string()),
                    RequirementType::User => Some("UR".to_string()),
                    RequirementType::ChangeRequest => Some("CR".to_string()),
                    RequirementType::Bug => Some("BUG".to_string()),
                    RequirementType::Epic => Some("EPIC".to_string()),
                    RequirementType::Story => Some("STORY".to_string()),
                    RequirementType::Task => Some("TASK".to_string()),
                    RequirementType::Spike => Some("SPIKE".to_string()),
                    RequirementType::Sprint => Some("SPRINT".to_string()),
                    RequirementType::Folder => Some("FLD".to_string()),
                    RequirementType::Meta => Some("META".to_string()),
                    // trace:FR-1-074 | ai:claude
                    RequirementType::Principle => Some("PRIN".to_string()),
                    RequirementType::Vision => Some("VIS".to_string()),
                    RequirementType::Constraint => Some("CON".to_string()),
                    RequirementType::Decision => Some("ADR".to_string()),
                    RequirementType::Term => Some("TERM".to_string()),
                    RequirementType::Doc => Some("DOC".to_string()),
                };
                (i, prefix_override, feature_prefix, type_prefix)
            })
            .collect();

        // Now assign new IDs
        for (i, prefix_override, feature_prefix, type_prefix) in req_data {
            let new_id = if let Some(ref override_prefix) = prefix_override {
                // Use the override prefix
                self.generate_requirement_id_with_override(override_prefix)
            } else {
                // Use standard feature/type prefix logic
                self.generate_requirement_id(feature_prefix.as_deref(), type_prefix.as_deref())
            };
            self.requirements[i].spec_id = Some(new_id);
        }
    }

    /// Validate proposed changes to ID configuration
    /// Returns validation result with error/warning messages
    pub fn validate_id_config_change(
        &self,
        new_format: &IdFormat,
        new_numbering: &NumberingStrategy,
        new_digits: u8,
    ) -> IdConfigValidation {
        let mut result = IdConfigValidation {
            valid: true,
            error: None,
            warning: None,
            can_migrate: true,
            affected_count: 0,
        };

        // Check if anything actually changed
        let format_changed = &self.id_config.format != new_format;
        let numbering_changed = &self.id_config.numbering != new_numbering;
        let digits_changed = self.id_config.digits != new_digits;

        if !format_changed && !numbering_changed && !digits_changed {
            result.can_migrate = false;
            return result;
        }

        // Find the maximum number of digits currently in use
        let max_digits_in_use = self.get_max_digits_in_use();

        // Validate digit reduction
        if new_digits < max_digits_in_use {
            result.valid = false;
            result.can_migrate = false;
            result.error = Some(format!(
                "Cannot reduce digits to {} - existing requirements use up to {} digits",
                new_digits, max_digits_in_use
            ));
            return result;
        }

        // Check format change constraints
        if format_changed {
            // For format changes, we require Global numbering for safe migration
            if self.id_config.numbering != NumberingStrategy::Global
                && *new_numbering != NumberingStrategy::Global
            {
                result.valid = false;
                result.can_migrate = false;
                result.error = Some(
                    "Format changes require Global numbering strategy. \
                     Please switch to Global numbering first."
                        .to_string(),
                );
                return result;
            }

            // Count affected requirements
            result.affected_count = self
                .requirements
                .iter()
                .filter(|r| r.spec_id.is_some())
                .count();

            if result.affected_count > 0 {
                result.warning = Some(format!(
                    "{} requirement(s) will have their IDs updated to the new format.",
                    result.affected_count
                ));
            }
        } else if numbering_changed || digits_changed {
            // For numbering/digit changes only, count affected
            result.affected_count = self
                .requirements
                .iter()
                .filter(|r| r.spec_id.is_some())
                .count();

            if digits_changed && result.affected_count > 0 {
                result.warning = Some(format!(
                    "{} requirement(s) will have their ID numbers reformatted.",
                    result.affected_count
                ));
            }
        }

        result
    }

    /// Get the maximum number of digits currently used in requirement IDs
    pub fn get_max_digits_in_use(&self) -> u8 {
        let mut max_digits: u8 = 0;

        for req in &self.requirements {
            if let Some(spec_id) = &req.spec_id {
                // Extract the numeric portion from the ID
                // Formats: "PREFIX-NNN" or "FEATURE-TYPE-NNN"
                let parts: Vec<&str> = spec_id.split('-').collect();
                if let Some(last) = parts.last() {
                    // Check if it's numeric
                    if last.chars().all(|c| c.is_ascii_digit()) {
                        let digits = last.len() as u8;
                        if digits > max_digits {
                            max_digits = digits;
                        }
                    }
                }
            }
        }

        max_digits
    }

    /// Migrate requirement IDs to new format/numbering/digits configuration
    /// Returns the number of requirements migrated
    pub fn migrate_ids_to_config(
        &mut self,
        new_format: IdFormat,
        new_numbering: NumberingStrategy,
        new_digits: u8,
    ) -> usize {
        // Update the configuration first
        self.id_config.format = new_format;
        self.id_config.numbering = new_numbering;
        self.id_config.digits = new_digits;

        // Reset counters for fresh numbering
        self.next_spec_number = 1;
        self.prefix_counters.clear();

        // Collect requirement data for migration (to avoid borrow issues)
        let req_data: ReqIdData = self
            .requirements
            .iter()
            .enumerate()
            .map(|(i, req)| {
                // Check for prefix_override first
                let prefix_override = req.prefix_override.clone();

                let feature_prefix = self
                    .features
                    .iter()
                    .find(|f| req.feature.contains(&f.name))
                    .map(|f| f.prefix.clone());
                let type_prefix = match req.req_type {
                    RequirementType::Functional => Some("FR".to_string()),
                    RequirementType::NonFunctional => Some("NFR".to_string()),
                    RequirementType::System => Some("SR".to_string()),
                    RequirementType::User => Some("UR".to_string()),
                    RequirementType::ChangeRequest => Some("CR".to_string()),
                    RequirementType::Bug => Some("BUG".to_string()),
                    RequirementType::Epic => Some("EPIC".to_string()),
                    RequirementType::Story => Some("STORY".to_string()),
                    RequirementType::Task => Some("TASK".to_string()),
                    RequirementType::Spike => Some("SPIKE".to_string()),
                    RequirementType::Sprint => Some("SPRINT".to_string()),
                    RequirementType::Folder => Some("FLD".to_string()),
                    RequirementType::Meta => Some("META".to_string()),
                    // trace:FR-1-074 | ai:claude
                    RequirementType::Principle => Some("PRIN".to_string()),
                    RequirementType::Vision => Some("VIS".to_string()),
                    RequirementType::Constraint => Some("CON".to_string()),
                    RequirementType::Decision => Some("ADR".to_string()),
                    RequirementType::Term => Some("TERM".to_string()),
                    RequirementType::Doc => Some("DOC".to_string()),
                };
                (i, prefix_override, feature_prefix, type_prefix)
            })
            .collect();

        let mut migrated_count = 0;

        // Generate new IDs for all requirements
        for (i, prefix_override, feature_prefix, type_prefix) in req_data {
            let new_id = if let Some(ref override_prefix) = prefix_override {
                // Use the override prefix
                self.generate_requirement_id_with_override(override_prefix)
            } else {
                // Use standard feature/type prefix logic
                self.generate_requirement_id(feature_prefix.as_deref(), type_prefix.as_deref())
            };
            self.requirements[i].spec_id = Some(new_id);
            migrated_count += 1;
        }

        migrated_count
    }

    /// Add a new requirement type definition
    pub fn add_requirement_type(
        &mut self,
        name: &str,
        prefix: &str,
        description: &str,
    ) -> anyhow::Result<()> {
        let prefix_upper = prefix.to_uppercase();

        // Check if prefix is already used
        if self.id_config.get_type_by_prefix(&prefix_upper).is_some() {
            anyhow::bail!(
                "Prefix '{}' is already used by another requirement type",
                prefix_upper
            );
        }

        // Check if it conflicts with a feature prefix
        if self.get_feature_by_prefix(&prefix_upper).is_some() {
            anyhow::bail!("Prefix '{}' is already used by a feature", prefix_upper);
        }

        self.id_config
            .requirement_types
            .push(RequirementTypeDefinition::new(
                name,
                &prefix_upper,
                description,
            ));
        Ok(())
    }

    /// Add a relationship between two requirements
    pub fn add_relationship(
        &mut self,
        source_id: &Uuid,
        rel_type: RelationshipType,
        target_id: &Uuid,
        bidirectional: bool,
    ) -> anyhow::Result<()> {
        self.add_relationship_with_creator(source_id, rel_type, target_id, bidirectional, None)
    }

    /// Add a relationship between two requirements with optional creator info
    pub fn add_relationship_with_creator(
        &mut self,
        source_id: &Uuid,
        rel_type: RelationshipType,
        target_id: &Uuid,
        bidirectional: bool,
        created_by: Option<String>,
    ) -> anyhow::Result<()> {
        // Validate both requirements exist
        if !self.requirements.iter().any(|r| r.id == *source_id) {
            anyhow::bail!("Source requirement not found: {}", source_id);
        }
        if !self.requirements.iter().any(|r| r.id == *target_id) {
            anyhow::bail!("Target requirement not found: {}", target_id);
        }

        // Don't allow self-relationships
        if source_id == target_id {
            anyhow::bail!("Cannot create relationship to self");
        }

        // Add the relationship to source
        let source_req = self
            .get_requirement_by_id_mut(source_id)
            .ok_or_else(|| anyhow::anyhow!("Source requirement not found"))?;

        // Check if relationship already exists
        if source_req
            .relationships
            .iter()
            .any(|r| r.target_id == *target_id && r.rel_type == rel_type)
        {
            anyhow::bail!(
                "Relationship '{}' to {} already exists",
                rel_type,
                target_id
            );
        }

        let now = Utc::now();
        source_req.relationships.push(Relationship {
            rel_type: rel_type.clone(),
            target_id: *target_id,
            created_at: Some(now),
            created_by: created_by.clone(),
        });

        // Add inverse relationship if bidirectional and inverse exists
        if bidirectional {
            if let Some(inverse_type) = rel_type.inverse() {
                let target_req = self
                    .get_requirement_by_id_mut(target_id)
                    .ok_or_else(|| anyhow::anyhow!("Target requirement not found"))?;

                // Only add if it doesn't already exist
                if !target_req
                    .relationships
                    .iter()
                    .any(|r| r.target_id == *source_id && r.rel_type == inverse_type)
                {
                    target_req.relationships.push(Relationship {
                        rel_type: inverse_type,
                        target_id: *source_id,
                        created_at: Some(now),
                        created_by: created_by.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Set a unique relationship, removing any existing relationship of the same type first
    /// This is useful for Parent relationships where a requirement can only have one parent
    pub fn set_relationship(
        &mut self,
        source_id: &Uuid,
        rel_type: RelationshipType,
        target_id: &Uuid,
        bidirectional: bool,
    ) -> anyhow::Result<()> {
        self.set_relationship_with_creator(source_id, rel_type, target_id, bidirectional, None)
    }

    /// Set a unique relationship with creator info, removing any existing relationship of the same type first
    pub fn set_relationship_with_creator(
        &mut self,
        source_id: &Uuid,
        rel_type: RelationshipType,
        target_id: &Uuid,
        bidirectional: bool,
        created_by: Option<String>,
    ) -> anyhow::Result<()> {
        // Validate both requirements exist
        if !self.requirements.iter().any(|r| r.id == *source_id) {
            anyhow::bail!("Source requirement not found: {}", source_id);
        }
        if !self.requirements.iter().any(|r| r.id == *target_id) {
            anyhow::bail!("Target requirement not found: {}", target_id);
        }

        // Don't allow self-relationships
        if source_id == target_id {
            anyhow::bail!("Cannot create relationship to self");
        }

        // Remove any existing relationships of this type from the source
        // For Parent relationships, this ensures a child can only have one parent
        {
            let source_req = self
                .get_requirement_by_id_mut(source_id)
                .ok_or_else(|| anyhow::anyhow!("Source requirement not found"))?;

            // Find and remove existing relationships of this type
            let old_targets: Vec<Uuid> = source_req
                .relationships
                .iter()
                .filter(|r| r.rel_type == rel_type)
                .map(|r| r.target_id)
                .collect();

            source_req.relationships.retain(|r| r.rel_type != rel_type);

            // Remove inverse relationships from old targets
            if bidirectional {
                if let Some(inverse_type) = rel_type.inverse() {
                    for old_target in old_targets {
                        if let Some(old_target_req) = self.get_requirement_by_id_mut(&old_target) {
                            old_target_req.relationships.retain(|r| {
                                !(r.target_id == *source_id && r.rel_type == inverse_type)
                            });
                        }
                    }
                }
            }
        }

        // Now add the new relationship
        self.add_relationship_with_creator(
            source_id,
            rel_type,
            target_id,
            bidirectional,
            created_by,
        )
    }

    /// Remove a relationship between two requirements
    pub fn remove_relationship(
        &mut self,
        source_id: &Uuid,
        rel_type: &RelationshipType,
        target_id: &Uuid,
        bidirectional: bool,
    ) -> anyhow::Result<()> {
        // Remove relationship from source
        let source_req = self
            .get_requirement_by_id_mut(source_id)
            .ok_or_else(|| anyhow::anyhow!("Source requirement not found: {}", source_id))?;

        let original_len = source_req.relationships.len();
        source_req
            .relationships
            .retain(|r| !(r.target_id == *target_id && r.rel_type == *rel_type));

        if source_req.relationships.len() == original_len {
            anyhow::bail!("Relationship '{}' to {} not found", rel_type, target_id);
        }

        // Remove inverse relationship if bidirectional
        if bidirectional {
            if let Some(inverse_type) = rel_type.inverse() {
                if let Some(target_req) = self.get_requirement_by_id_mut(target_id) {
                    target_req
                        .relationships
                        .retain(|r| !(r.target_id == *source_id && r.rel_type == inverse_type));
                }
            }
        }

        Ok(())
    }

    /// Get all relationships for a requirement
    pub fn get_relationships(&self, id: &Uuid) -> Vec<(RelationshipType, Uuid)> {
        self.get_requirement_by_id(id)
            .map(|req| {
                req.relationships
                    .iter()
                    .map(|r| (r.rel_type.clone(), r.target_id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all relationships of a specific type for a requirement
    pub fn get_relationships_by_type(&self, id: &Uuid, rel_type: &RelationshipType) -> Vec<Uuid> {
        self.get_requirement_by_id(id)
            .map(|req| {
                req.relationships
                    .iter()
                    .filter(|r| r.rel_type == *rel_type)
                    .map(|r| r.target_id)
                    .collect()
            })
            .unwrap_or_default()
    }

    // ========================================================================
    // Relationship Definition Management
    // ========================================================================

    /// Get a relationship definition by name
    pub fn get_relationship_definition(&self, name: &str) -> Option<&RelationshipDefinition> {
        let name_lower = name.to_lowercase();
        self.relationship_definitions
            .iter()
            .find(|d| d.name == name_lower)
    }

    /// Get a relationship definition for a RelationshipType
    pub fn get_definition_for_type(
        &self,
        rel_type: &RelationshipType,
    ) -> Option<&RelationshipDefinition> {
        self.get_relationship_definition(&rel_type.name())
    }

    /// Get all relationship definitions
    pub fn get_relationship_definitions(&self) -> &[RelationshipDefinition] {
        &self.relationship_definitions
    }

    /// Add a new relationship definition
    pub fn add_relationship_definition(
        &mut self,
        definition: RelationshipDefinition,
    ) -> anyhow::Result<()> {
        let name_lower = definition.name.to_lowercase();

        // Check if name already exists
        if self
            .relationship_definitions
            .iter()
            .any(|d| d.name == name_lower)
        {
            anyhow::bail!("Relationship definition '{}' already exists", name_lower);
        }

        // If it has an inverse, verify the inverse exists or will be created
        if let Some(ref inverse) = definition.inverse {
            let inverse_lower = inverse.to_lowercase();
            // Only warn if the inverse doesn't exist - it might be added later
            if !self
                .relationship_definitions
                .iter()
                .any(|d| d.name == inverse_lower)
            {
                // This is okay - the inverse might be defined later
            }
        }

        self.relationship_definitions.push(RelationshipDefinition {
            name: name_lower,
            ..definition
        });
        Ok(())
    }

    /// Update an existing relationship definition
    pub fn update_relationship_definition(
        &mut self,
        name: &str,
        definition: RelationshipDefinition,
    ) -> anyhow::Result<()> {
        let name_lower = name.to_lowercase();

        let def = self
            .relationship_definitions
            .iter_mut()
            .find(|d| d.name == name_lower)
            .ok_or_else(|| anyhow::anyhow!("Relationship definition '{}' not found", name_lower))?;

        // Can't change built_in status
        if def.built_in {
            // Allow updates to non-critical fields for built-ins
            def.display_name = definition.display_name;
            def.description = definition.description;
            def.color = definition.color;
            def.icon = definition.icon;
            def.source_types = definition.source_types;
            def.target_types = definition.target_types;
            // Don't change: name, inverse, symmetric, cardinality, built_in
        } else {
            *def = RelationshipDefinition {
                name: name_lower,
                built_in: false,
                ..definition
            };
        }

        Ok(())
    }

    /// Remove a relationship definition (only non-built-in)
    pub fn remove_relationship_definition(&mut self, name: &str) -> anyhow::Result<()> {
        let name_lower = name.to_lowercase();

        let def = self
            .relationship_definitions
            .iter()
            .find(|d| d.name == name_lower)
            .ok_or_else(|| anyhow::anyhow!("Relationship definition '{}' not found", name_lower))?;

        if def.built_in {
            anyhow::bail!(
                "Cannot remove built-in relationship definition '{}'",
                name_lower
            );
        }

        self.relationship_definitions
            .retain(|d| d.name != name_lower);
        Ok(())
    }

    /// Ensure built-in relationship definitions exist (call after loading)
    pub fn ensure_builtin_relationships(&mut self) {
        let defaults = RelationshipDefinition::defaults();
        for default_def in defaults {
            if !self
                .relationship_definitions
                .iter()
                .any(|d| d.name == default_def.name)
            {
                self.relationship_definitions.push(default_def);
            }
        }
    }

    /// Validate a proposed relationship
    pub fn validate_relationship(
        &self,
        source_id: &Uuid,
        rel_type: &RelationshipType,
        target_id: &Uuid,
    ) -> RelationshipValidation {
        let mut validation = RelationshipValidation::ok();

        // Check self-reference
        if source_id == target_id {
            return RelationshipValidation::error("Cannot create relationship to self");
        }

        // Get source and target requirements
        let source = match self.get_requirement_by_id(source_id) {
            Some(r) => r,
            None => return RelationshipValidation::error("Source requirement not found"),
        };
        let target = match self.get_requirement_by_id(target_id) {
            Some(r) => r,
            None => return RelationshipValidation::error("Target requirement not found"),
        };

        // Check if relationship already exists
        if source
            .relationships
            .iter()
            .any(|r| r.target_id == *target_id && r.rel_type == *rel_type)
        {
            return RelationshipValidation::error(&format!(
                "Relationship '{}' to {} already exists",
                rel_type, target_id
            ));
        }

        // Get the relationship definition
        let definition = match self.get_definition_for_type(rel_type) {
            Some(d) => d,
            None => {
                // Custom relationship without definition - allow but warn
                validation.add_warning(&format!(
                    "No definition found for relationship type '{}'. Consider creating one.",
                    rel_type.name()
                ));
                return validation;
            }
        };

        // Check source type constraint
        if !definition.allows_source_type(&source.req_type) {
            validation.add_error(&format!(
                "Source requirement type '{}' is not allowed for '{}' relationships. Allowed: {:?}",
                source.req_type, definition.display_name, definition.source_types
            ));
        }

        // Check target type constraint
        if !definition.allows_target_type(&target.req_type) {
            validation.add_error(&format!(
                "Target requirement type '{}' is not allowed for '{}' relationships. Allowed: {:?}",
                target.req_type, definition.display_name, definition.target_types
            ));
        }

        // Check cardinality constraints
        match definition.cardinality {
            Cardinality::OneToOne => {
                // Source can only have one outgoing relationship of this type
                let existing_outgoing = source
                    .relationships
                    .iter()
                    .filter(|r| r.rel_type == *rel_type)
                    .count();
                if existing_outgoing > 0 {
                    validation.add_warning(&format!(
                        "Source already has a '{}' relationship (cardinality is 1:1)",
                        definition.display_name
                    ));
                }
                // Target can only have one incoming relationship of this type
                let existing_incoming = self
                    .requirements
                    .iter()
                    .filter(|r| r.id != *source_id)
                    .flat_map(|r| r.relationships.iter())
                    .filter(|r| r.target_id == *target_id && r.rel_type == *rel_type)
                    .count();
                if existing_incoming > 0 {
                    validation.add_warning(&format!(
                        "Target already has an incoming '{}' relationship (cardinality is 1:1)",
                        definition.display_name
                    ));
                }
            }
            Cardinality::ManyToOne => {
                // Source can only have one outgoing relationship of this type
                let existing_outgoing = source
                    .relationships
                    .iter()
                    .filter(|r| r.rel_type == *rel_type)
                    .count();
                if existing_outgoing > 0 {
                    validation.add_warning(&format!(
                        "Source already has a '{}' relationship (cardinality is N:1, only one allowed per source)",
                        definition.display_name
                    ));
                }
            }
            Cardinality::OneToMany => {
                // Target can only have one incoming relationship of this type
                let existing_incoming = self
                    .requirements
                    .iter()
                    .filter(|r| r.id != *source_id)
                    .flat_map(|r| r.relationships.iter())
                    .filter(|r| r.target_id == *target_id && r.rel_type == *rel_type)
                    .count();
                if existing_incoming > 0 {
                    validation.add_warning(&format!(
                        "Target already has an incoming '{}' relationship (cardinality is 1:N)",
                        definition.display_name
                    ));
                }
            }
            Cardinality::ManyToMany => {
                // No cardinality constraints
            }
        }

        // Check for cycles in hierarchical relationships (parent/child)
        if (rel_type.name() == "parent" || rel_type.name() == "child")
            && self.would_create_cycle(source_id, target_id, rel_type)
        {
            validation.add_error("This relationship would create a cycle in the hierarchy");
        }

        validation
    }

    /// Check if adding a relationship would create a cycle
    fn would_create_cycle(
        &self,
        source_id: &Uuid,
        target_id: &Uuid,
        rel_type: &RelationshipType,
    ) -> bool {
        // For parent relationships, check if target is already an ancestor of source
        // For child relationships, check if target is already a descendant of source
        let check_type = if rel_type.name() == "parent" {
            RelationshipType::Parent
        } else if rel_type.name() == "child" {
            RelationshipType::Child
        } else {
            return false;
        };

        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![*target_id];

        while let Some(current) = stack.pop() {
            if current == *source_id {
                return true; // Found a cycle
            }
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current);

            // Follow the relationship chain
            if let Some(req) = self.get_requirement_by_id(&current) {
                for rel in &req.relationships {
                    if rel.rel_type == check_type {
                        stack.push(rel.target_id);
                    }
                }
            }
        }

        false
    }

    /// Get the inverse relationship type from definitions
    pub fn get_inverse_type(&self, rel_type: &RelationshipType) -> Option<RelationshipType> {
        // First check built-in inverse
        if let Some(inverse) = rel_type.inverse() {
            return Some(inverse);
        }

        // Then check definition
        if let Some(def) = self.get_definition_for_type(rel_type) {
            if let Some(ref inverse_name) = def.inverse {
                return Some(RelationshipType::from_str(inverse_name));
            }
            if def.symmetric {
                return Some(rel_type.clone());
            }
        }

        None
    }

    // =========================================================================
    // Baseline Operations
    // =========================================================================

    /// Creates a new baseline from current requirements
    pub fn create_baseline(
        &mut self,
        name: String,
        description: Option<String>,
        created_by: String,
    ) -> &Baseline {
        let baseline = Baseline::new(name, description, created_by, &self.requirements);
        self.baselines.push(baseline);
        self.baselines.last().unwrap()
    }

    /// Gets a baseline by ID
    pub fn get_baseline(&self, id: &Uuid) -> Option<&Baseline> {
        self.baselines.iter().find(|b| &b.id == id)
    }

    /// Gets a baseline by name
    pub fn get_baseline_by_name(&self, name: &str) -> Option<&Baseline> {
        self.baselines.iter().find(|b| b.name == name)
    }

    /// Deletes a baseline by ID (if not locked)
    pub fn delete_baseline(&mut self, id: &Uuid) -> bool {
        if let Some(idx) = self.baselines.iter().position(|b| &b.id == id) {
            if !self.baselines[idx].locked {
                self.baselines.remove(idx);
                return true;
            }
        }
        false
    }

    /// Compares current requirements against a baseline
    pub fn compare_with_baseline(&self, baseline_id: &Uuid) -> Option<BaselineComparison> {
        let baseline = self.get_baseline(baseline_id)?;
        Some(self.compare_snapshots_to_current(&baseline.requirements))
    }

    /// Compares two baselines
    pub fn compare_baselines(
        &self,
        source_id: &Uuid,
        target_id: &Uuid,
    ) -> Option<BaselineComparison> {
        let source = self.get_baseline(source_id)?;
        let target = self.get_baseline(target_id)?;
        Some(Self::compare_snapshot_sets(
            &source.requirements,
            &target.requirements,
        ))
    }

    /// Helper: compare snapshots to current requirements
    fn compare_snapshots_to_current(
        &self,
        snapshots: &[RequirementSnapshot],
    ) -> BaselineComparison {
        use std::collections::HashMap;

        let snapshot_map: HashMap<Uuid, &RequirementSnapshot> =
            snapshots.iter().map(|s| (s.original_id, s)).collect();

        let current_map: HashMap<Uuid, &Requirement> = self
            .requirements
            .iter()
            .filter(|r| !r.archived)
            .map(|r| (r.id, r))
            .collect();

        let mut comparison = BaselineComparison::default();

        // Find added (in current but not in baseline)
        for id in current_map.keys() {
            if !snapshot_map.contains_key(id) {
                comparison.added.push(*id);
            }
        }

        // Find removed (in baseline but not in current)
        for id in snapshot_map.keys() {
            if !current_map.contains_key(id) {
                comparison.removed.push(*id);
            }
        }

        // Find modified and unchanged
        for (id, snapshot) in &snapshot_map {
            if let Some(current) = current_map.get(id) {
                let changes = Self::diff_snapshot_to_requirement(snapshot, current);
                if changes.is_empty() {
                    comparison.unchanged.push(*id);
                } else {
                    comparison.modified.push(BaselineRequirementDiff {
                        id: *id,
                        spec_id: current.spec_id.clone(),
                        changes,
                    });
                }
            }
        }

        comparison
    }

    /// Helper: compare two sets of snapshots
    fn compare_snapshot_sets(
        source: &[RequirementSnapshot],
        target: &[RequirementSnapshot],
    ) -> BaselineComparison {
        use std::collections::HashMap;

        let source_map: HashMap<Uuid, &RequirementSnapshot> =
            source.iter().map(|s| (s.original_id, s)).collect();

        let target_map: HashMap<Uuid, &RequirementSnapshot> =
            target.iter().map(|s| (s.original_id, s)).collect();

        let mut comparison = BaselineComparison::default();

        // Find added (in target but not in source)
        for id in target_map.keys() {
            if !source_map.contains_key(id) {
                comparison.added.push(*id);
            }
        }

        // Find removed (in source but not in target)
        for id in source_map.keys() {
            if !target_map.contains_key(id) {
                comparison.removed.push(*id);
            }
        }

        // Find modified and unchanged
        for (id, source_snap) in &source_map {
            if let Some(target_snap) = target_map.get(id) {
                let changes = Self::diff_snapshots(source_snap, target_snap);
                if changes.is_empty() {
                    comparison.unchanged.push(*id);
                } else {
                    comparison.modified.push(BaselineRequirementDiff {
                        id: *id,
                        spec_id: target_snap.spec_id.clone(),
                        changes,
                    });
                }
            }
        }

        comparison
    }

    /// Helper: diff a snapshot against current requirement
    fn diff_snapshot_to_requirement(
        snapshot: &RequirementSnapshot,
        current: &Requirement,
    ) -> Vec<FieldChange> {
        let mut changes = Vec::new();

        if snapshot.title != current.title {
            changes.push(FieldChange {
                field_name: "title".to_string(),
                old_value: snapshot.title.clone(),
                new_value: current.title.clone(),
            });
        }
        if snapshot.description != current.description {
            changes.push(FieldChange {
                field_name: "description".to_string(),
                old_value: snapshot.description.clone(),
                new_value: current.description.clone(),
            });
        }
        if snapshot.status != current.status {
            changes.push(FieldChange {
                field_name: "status".to_string(),
                old_value: snapshot.status.to_string(),
                new_value: current.status.to_string(),
            });
        }
        if snapshot.priority != current.priority {
            changes.push(FieldChange {
                field_name: "priority".to_string(),
                old_value: snapshot.priority.to_string(),
                new_value: current.priority.to_string(),
            });
        }
        if snapshot.owner != current.owner {
            changes.push(FieldChange {
                field_name: "owner".to_string(),
                old_value: snapshot.owner.clone(),
                new_value: current.owner.clone(),
            });
        }
        if snapshot.feature != current.feature {
            changes.push(FieldChange {
                field_name: "feature".to_string(),
                old_value: snapshot.feature.clone(),
                new_value: current.feature.clone(),
            });
        }
        if snapshot.req_type != current.req_type {
            changes.push(FieldChange {
                field_name: "type".to_string(),
                old_value: format!("{:?}", snapshot.req_type),
                new_value: format!("{:?}", current.req_type),
            });
        }

        changes
    }

    /// Helper: diff two snapshots
    fn diff_snapshots(
        source: &RequirementSnapshot,
        target: &RequirementSnapshot,
    ) -> Vec<FieldChange> {
        let mut changes = Vec::new();

        if source.title != target.title {
            changes.push(FieldChange {
                field_name: "title".to_string(),
                old_value: source.title.clone(),
                new_value: target.title.clone(),
            });
        }
        if source.description != target.description {
            changes.push(FieldChange {
                field_name: "description".to_string(),
                old_value: source.description.clone(),
                new_value: target.description.clone(),
            });
        }
        if source.status != target.status {
            changes.push(FieldChange {
                field_name: "status".to_string(),
                old_value: source.status.to_string(),
                new_value: target.status.to_string(),
            });
        }
        if source.priority != target.priority {
            changes.push(FieldChange {
                field_name: "priority".to_string(),
                old_value: source.priority.to_string(),
                new_value: target.priority.to_string(),
            });
        }
        if source.owner != target.owner {
            changes.push(FieldChange {
                field_name: "owner".to_string(),
                old_value: source.owner.clone(),
                new_value: target.owner.clone(),
            });
        }
        if source.feature != target.feature {
            changes.push(FieldChange {
                field_name: "feature".to_string(),
                old_value: source.feature.clone(),
                new_value: target.feature.clone(),
            });
        }
        if source.req_type != target.req_type {
            changes.push(FieldChange {
                field_name: "type".to_string(),
                old_value: format!("{:?}", source.req_type),
                new_value: format!("{:?}", target.req_type),
            });
        }

        changes
    }
}

impl Default for RequirementsStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BUG-251: forward-compat — an unknown `RelationshipType` variant (a newer
    /// binary's addition, read by an older one) deserializes to `Custom(name)`
    /// instead of failing the whole spec parse. Covers all three wire shapes
    /// plus serialize-byte stability. trace:BUG-251 | ai:claude
    #[test]
    fn relationship_type_deserialize_is_forward_compatible() {
        // The bug: an unknown unit variant used to hard-error. Now → Custom.
        let future: RelationshipType = serde_yaml::from_str("FutureVariant\n").unwrap();
        assert_eq!(
            future,
            RelationshipType::Custom("FutureVariant".to_string())
        );

        // Existing YAML forms still parse to the right typed variants.
        assert_eq!(
            serde_yaml::from_str::<RelationshipType>("Parent\n").unwrap(),
            RelationshipType::Parent
        );
        assert_eq!(
            serde_yaml::from_str::<RelationshipType>("BlockedBy\n").unwrap(),
            RelationshipType::BlockedBy
        );
        // YAML externally-tagged Custom (`!Custom foo`).
        assert_eq!(
            serde_yaml::from_str::<RelationshipType>("!Custom foo\n").unwrap(),
            RelationshipType::Custom("foo".to_string())
        );

        // JSON forms: bare-string unit variant + externally-tagged Custom.
        assert_eq!(
            serde_json::from_str::<RelationshipType>("\"Parent\"").unwrap(),
            RelationshipType::Parent
        );
        assert_eq!(
            serde_json::from_str::<RelationshipType>("{\"Custom\":\"foo\"}").unwrap(),
            RelationshipType::Custom("foo".to_string())
        );

        // Serialize bytes are unchanged (derived Serialize preserved) — older
        // and newer binaries must keep producing identical on-disk content.
        assert_eq!(
            serde_yaml::to_string(&RelationshipType::Parent)
                .unwrap()
                .trim(),
            "Parent"
        );
        assert_eq!(
            serde_yaml::to_string(&RelationshipType::BlockedBy)
                .unwrap()
                .trim(),
            "BlockedBy"
        );
        assert_eq!(
            serde_yaml::to_string(&RelationshipType::Custom("foo".to_string()))
                .unwrap()
                .trim(),
            "!Custom foo"
        );

        // Full round-trip: serialize a future-unknown variant we modeled as
        // Custom, read it back identically.
        let rt = RelationshipType::Custom("future-edge".to_string());
        let yaml = serde_yaml::to_string(&rt).unwrap();
        assert_eq!(serde_yaml::from_str::<RelationshipType>(&yaml).unwrap(), rt);
    }

    /// STORY-333: typed `BlockedBy` round-trips cleanly through the canonical
    /// string form. A regression here would silently downgrade typed edges
    /// to `Custom("blocked-by")` and break the pickability gate.
    /// trace:STORY-333 | ai:claude
    #[test]
    fn relationship_type_blocked_by_round_trips_through_string() {
        let parsed = RelationshipType::from_str("blocked-by");
        assert_eq!(parsed, RelationshipType::BlockedBy);
        assert_eq!(parsed.to_string(), "blocked-by");
        // Aliases all land on the typed variant.
        assert_eq!(
            RelationshipType::from_str("blocked_by"),
            RelationshipType::BlockedBy
        );
        assert_eq!(
            RelationshipType::from_str("blockedby"),
            RelationshipType::BlockedBy
        );
        assert_eq!(
            RelationshipType::from_str("BLOCKED-BY"),
            RelationshipType::BlockedBy
        );
    }

    /// STORY-333: `BlockedBy` has a known inverse `Blocks` so bidirectional
    /// rels stay consistent and the doctor check can walk either direction.
    /// trace:STORY-333 | ai:claude
    #[test]
    fn relationship_type_blocked_by_inverse_is_blocks_and_back() {
        assert_eq!(
            RelationshipType::BlockedBy.inverse(),
            Some(RelationshipType::Blocks)
        );
        assert_eq!(
            RelationshipType::Blocks.inverse(),
            Some(RelationshipType::BlockedBy)
        );
    }

    /// STORY-333: `human_only` defaults to `false` on a fresh Requirement and
    /// serializes absent (skip_serializing_if), so existing specs round-trip
    /// unchanged through yaml.
    /// trace:STORY-333 | ai:claude
    #[test]
    fn requirement_default_human_only_is_false_and_serializes_absent() {
        let r = Requirement::new("t".into(), "d".into());
        assert!(!r.human_only);
        let yaml = serde_yaml::to_string(&r).unwrap();
        assert!(
            !yaml.contains("human_only"),
            "default human_only=false must not appear in yaml; got:\n{}",
            yaml
        );

        // When set, it must appear.
        let mut r2 = r.clone();
        r2.human_only = true;
        let yaml2 = serde_yaml::to_string(&r2).unwrap();
        assert!(yaml2.contains("human_only: true"));
    }

    /// `RequirementType::Doc` is the EPIC-24 living-documentation type. Lock
    /// in its prefix + display string so a careless rename doesn't silently
    /// orphan existing DOC-N spec ids on disk. trace:STORY-104 | ai:claude
    #[test]
    fn doc_type_has_stable_prefix_and_display() {
        let t = RequirementType::Doc;
        assert_eq!(t.default_prefix(), "DOC");
        assert_eq!(t.to_string(), "Doc");
    }

    /// `get_type_prefix` is the dynamic dispatcher used everywhere a spec_id
    /// gets generated — it must know about the Doc variant or `aida doc add`
    /// silently falls back to the wrong prefix. trace:STORY-104 | ai:claude
    #[test]
    fn doc_type_resolves_through_get_type_prefix() {
        let store = RequirementsStore::new();
        assert_eq!(
            store.get_type_prefix(&RequirementType::Doc).as_deref(),
            Some("DOC")
        );
    }

    #[test]
    fn test_add_requirement_with_spec_id() {
        let mut store = RequirementsStore::new();
        let req = Requirement::new("Test".into(), "Description".into());

        assert_eq!(store.next_spec_number, 1);
        assert!(req.spec_id.is_none());

        store.add_requirement_with_spec_id(req);

        assert_eq!(store.requirements.len(), 1);
        assert_eq!(store.requirements[0].spec_id, Some("SPEC-001".into()));
        assert_eq!(store.next_spec_number, 2);
    }

    #[test]
    fn test_get_requirement_by_spec_id() {
        let mut store = RequirementsStore::new();
        let req = Requirement::new("Test".into(), "Description".into());
        store.add_requirement_with_spec_id(req);

        let found = store.get_requirement_by_spec_id("SPEC-001");
        assert!(found.is_some());
        assert_eq!(found.unwrap().title, "Test");

        let not_found = store.get_requirement_by_spec_id("SPEC-999");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_assign_spec_ids() {
        let mut store = RequirementsStore::new();

        let req1 = Requirement::new("R1".into(), "D1".into());
        let req2 = Requirement::new("R2".into(), "D2".into());

        // Manually add without SPEC-IDs
        store.requirements.push(req1);
        store.requirements.push(req2);

        assert!(store.requirements[0].spec_id.is_none());
        assert!(store.requirements[1].spec_id.is_none());

        store.assign_spec_ids();

        assert_eq!(store.requirements[0].spec_id, Some("SPEC-001".into()));
        assert_eq!(store.requirements[1].spec_id, Some("SPEC-002".into()));
        assert_eq!(store.next_spec_number, 3);
    }

    #[test]
    fn test_assign_spec_ids_skips_existing() {
        let mut store = RequirementsStore::new();

        let mut req1 = Requirement::new("R1".into(), "D1".into());
        req1.spec_id = Some("SPEC-001".into());
        let req2 = Requirement::new("R2".into(), "D2".into());

        store.requirements.push(req1);
        store.requirements.push(req2);
        store.next_spec_number = 2; // Start at 2 since SPEC-001 exists

        store.assign_spec_ids();

        assert_eq!(store.requirements[0].spec_id, Some("SPEC-001".into()));
        assert_eq!(store.requirements[1].spec_id, Some("SPEC-002".into()));
        assert_eq!(store.next_spec_number, 3);
    }

    #[test]
    fn test_validate_unique_spec_ids_success() {
        let mut store = RequirementsStore::new();
        let req1 = Requirement::new("R1".into(), "D1".into());
        let req2 = Requirement::new("R2".into(), "D2".into());

        store.add_requirement_with_spec_id(req1);
        store.add_requirement_with_spec_id(req2);

        assert!(store.validate_unique_spec_ids().is_ok());
    }

    #[test]
    fn test_validate_unique_spec_ids_duplicate() {
        let mut store = RequirementsStore::new();

        let mut req1 = Requirement::new("R1".into(), "D1".into());
        req1.spec_id = Some("SPEC-001".into());
        let mut req2 = Requirement::new("R2".into(), "D2".into());
        req2.spec_id = Some("SPEC-001".into()); // Duplicate!

        store.requirements.push(req1);
        store.requirements.push(req2);

        let result = store.validate_unique_spec_ids();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Duplicate SPEC-ID"));
    }

    #[test]
    fn test_repair_duplicate_spec_ids() {
        let mut store = RequirementsStore::new();

        let mut req1 = Requirement::new("R1".into(), "D1".into());
        req1.spec_id = Some("FR-0001".into());
        let mut req2 = Requirement::new("R2".into(), "D2".into());
        req2.spec_id = Some("FR-0001".into()); // Duplicate!
        let mut req3 = Requirement::new("R3".into(), "D3".into());
        req3.spec_id = Some("FR-0002".into()); // Unique

        store.requirements.push(req1);
        store.requirements.push(req2);
        store.requirements.push(req3);

        // Verify duplicates exist
        assert!(store.validate_unique_spec_ids().is_err());

        // Repair duplicates
        let repaired = store.repair_duplicate_spec_ids();
        assert_eq!(repaired, 1);

        // Verify no more duplicates
        assert!(store.validate_unique_spec_ids().is_ok());

        // First requirement should keep its original ID
        assert_eq!(store.requirements[0].spec_id.as_deref(), Some("FR-0001"));

        // Second requirement should have a new ID with same prefix
        let new_id = store.requirements[1].spec_id.as_deref().unwrap();
        assert!(new_id.starts_with("FR-"));
        assert_ne!(new_id, "FR-0001");

        // Third requirement should be unchanged
        assert_eq!(store.requirements[2].spec_id.as_deref(), Some("FR-0002"));
    }

    /// trace:BUG-1-025 | ai:claude
    #[test]
    fn set_status_from_str_accepts_canonical_variants() {
        let mut req = Requirement::new("t".into(), "d".into());

        // All these forms should map to InProgress with custom_status=None.
        for s in &[
            "InProgress",
            "in-progress",
            "In Progress",
            "in_progress",
            "IN-PROGRESS",
            "InProgress  ",
        ] {
            req.custom_status = Some("stale".into()); // pretend a previous bad value
            req.set_status_from_str(s);
            assert_eq!(
                req.status,
                RequirementStatus::InProgress,
                "{:?} should map to InProgress",
                s
            );
            assert_eq!(
                req.custom_status, None,
                "{:?} should clear custom_status",
                s
            );
        }

        // Same for Planned.
        for s in &["Planned", "planned", "PLANNED"] {
            req.custom_status = Some("stale".into());
            req.set_status_from_str(s);
            assert_eq!(req.status, RequirementStatus::Planned);
            assert_eq!(req.custom_status, None);
        }

        // Truly non-standard strings still flow into custom_status, preserving
        // the original casing.
        req.set_status_from_str("Awaiting Review");
        assert_eq!(req.custom_status.as_deref(), Some("Awaiting Review"));
    }

    /// trace:TASK-0415 — the `open` / `closed` aliases must expand to exactly
    /// the non-terminal / terminal status sets, regardless of casing or
    /// word-break punctuation.
    #[test]
    fn expand_filter_token_handles_aliases() {
        for alias in &["open", "OPEN", " Open "] {
            assert_eq!(
                RequirementStatus::expand_filter_token(alias),
                Some(vec![
                    "Draft",
                    "Approved",
                    "Planned",
                    "InProgress",
                    "NeedsAttention",
                ]),
                "{:?} should expand to the open set",
                alias
            );
        }
        for alias in &["closed", "CLOSED", "Closed"] {
            assert_eq!(
                RequirementStatus::expand_filter_token(alias),
                Some(vec!["Done", "Completed", "Rejected"]),
                "{:?} should expand to the closed set",
                alias
            );
        }
    }

    /// trace:TASK-0415 — a single canonical status token expands to itself
    /// (cache-key form), tolerant of casing and word-break punctuation.
    #[test]
    fn expand_filter_token_handles_single_status() {
        assert_eq!(
            RequirementStatus::expand_filter_token("draft"),
            Some(vec!["Draft"])
        );
        assert_eq!(
            RequirementStatus::expand_filter_token("in-progress"),
            Some(vec!["InProgress"])
        );
        assert_eq!(
            RequirementStatus::expand_filter_token("Needs Attention"),
            Some(vec!["NeedsAttention"])
        );
        // Unknown token → None (caller turns this into a clear error).
        assert_eq!(RequirementStatus::expand_filter_token("bogus"), None);
        assert_eq!(RequirementStatus::expand_filter_token("inflight"), None);
    }

    /// trace:TASK-0415 — a comma-separated spec is OR'd; aliases expand in
    /// place; duplicate cache-keys collapse; blank tokens are skipped.
    #[test]
    fn expand_filter_spec_ors_and_dedups() {
        assert_eq!(
            RequirementStatus::expand_filter_spec("draft,approved"),
            Ok(vec!["Draft".to_string(), "Approved".to_string()])
        );
        // alias + explicit status overlapping → deduped (Draft once).
        assert_eq!(
            RequirementStatus::expand_filter_spec("open,draft"),
            Ok(vec![
                "Draft".to_string(),
                "Approved".to_string(),
                "Planned".to_string(),
                "InProgress".to_string(),
                "NeedsAttention".to_string(),
            ])
        );
        // whitespace + blank tokens tolerated.
        assert_eq!(
            RequirementStatus::expand_filter_spec(" done , , completed "),
            Ok(vec!["Done".to_string(), "Completed".to_string()])
        );
    }

    /// trace:TASK-0415 — an unrecognized token surfaces as Err naming the
    /// offending token so the CLI can point the user at the real filters.
    #[test]
    fn expand_filter_spec_errors_on_unknown_token() {
        assert_eq!(
            RequirementStatus::expand_filter_spec("draft,wat"),
            Err("wat".to_string())
        );
        assert_eq!(
            RequirementStatus::expand_filter_spec("nonsense"),
            Err("nonsense".to_string())
        );
    }

    /// trace:BUG-1-025 | ai:claude — regression: setting Completed must clear
    /// a previously-set custom_status, otherwise effective_status() returns the
    /// stale string forever.
    #[test]
    fn set_status_from_str_clears_stale_custom_status() {
        let mut req = Requirement::new("t".into(), "d".into());
        req.set_status_from_str("in-progress"); // bug-era data: would have set custom_status
        assert_eq!(req.custom_status, None); // post-fix: enum value, no custom
        assert_eq!(req.effective_status(), "In Progress");

        req.custom_status = Some("In-progress".into()); // simulate already-corrupted record
        req.status = RequirementStatus::Draft;
        req.set_status_from_str("Completed");
        assert_eq!(req.status, RequirementStatus::Completed);
        assert_eq!(req.custom_status, None);
        assert_eq!(req.effective_status(), "Completed");
    }

    /// STORY-332: the NeedsAttention status round-trips through Display and
    /// every spelling `set_status_from_str` should canonicalise.
    #[test]
    fn needs_attention_status_round_trips() {
        assert_eq!(
            RequirementStatus::NeedsAttention.to_string(),
            "Needs Attention"
        );
        let mut req = Requirement::new("t".into(), "d".into());
        for s in &[
            "NeedsAttention",
            "needs-attention",
            "Needs Attention",
            "needs_attention",
            "NEEDS-ATTENTION",
        ] {
            req.custom_status = Some("stale".into());
            req.set_status_from_str(s);
            assert_eq!(
                req.status,
                RequirementStatus::NeedsAttention,
                "{s:?} should map to NeedsAttention"
            );
            assert_eq!(req.custom_status, None, "{s:?} should clear custom_status");
        }
    }

    /// STORY-332: a punt can enter NeedsAttention only from In Progress.
    #[test]
    fn forbidden_attention_transition_into_only_from_in_progress() {
        use RequirementStatus::*;
        // The one allowed entry.
        assert!(forbidden_attention_transition(&InProgress, &NeedsAttention).is_none());
        // Every other source is forbidden.
        for from in [Draft, Approved, Planned, Done, Completed, Rejected] {
            assert!(
                forbidden_attention_transition(&from, &NeedsAttention).is_some(),
                "{from} → NeedsAttention should be forbidden"
            );
        }
        // Idempotent no-op stays allowed.
        assert!(forbidden_attention_transition(&NeedsAttention, &NeedsAttention).is_none());
    }

    /// STORY-332: a NeedsAttention spec resolves only to Approved /
    /// In Progress / Rejected.
    #[test]
    fn forbidden_attention_transition_out_only_to_triage_outcomes() {
        use RequirementStatus::*;
        for to in [Approved, InProgress, Rejected] {
            assert!(
                forbidden_attention_transition(&NeedsAttention, &to).is_none(),
                "NeedsAttention → {to} should be allowed"
            );
        }
        for to in [Draft, Planned, Done, Completed] {
            assert!(
                forbidden_attention_transition(&NeedsAttention, &to).is_some(),
                "NeedsAttention → {to} should be forbidden"
            );
        }
    }

    /// STORY-332: transitions that do not touch NeedsAttention are never
    /// constrained — the rule must not regress AIDA's free-form status edits.
    #[test]
    fn forbidden_attention_transition_none_for_unrelated_edges() {
        use RequirementStatus::*;
        let states = [
            Draft, Approved, Planned, InProgress, Done, Completed, Rejected,
        ];
        for from in &states {
            for to in &states {
                assert!(
                    forbidden_attention_transition(from, to).is_none(),
                    "{from} → {to} touches no NeedsAttention edge — must stay allowed"
                );
            }
        }
    }

    /// EPIC-28: a fresh requirement has no failure_reason.
    /// trace:EPIC-28 | ai:claude
    #[test]
    fn failure_reason_absent_on_fresh_requirement() {
        let r = Requirement::new("t".into(), "d".into());
        assert!(r.failure_reason.is_none());
    }

    /// EPIC-28: FailureReason round-trips through serde JSON cleanly so
    /// the cache + git-canonical store agree on its shape.
    /// trace:EPIC-28 | ai:claude
    #[test]
    fn failure_reason_round_trips() {
        let now = chrono::Utc::now();
        let fr = FailureReason {
            phase: "ci".into(),
            phase_index: 2,
            kind: "ci-red".into(),
            detail: "CI run 12345 failed — 3 tests panicked".into(),
            recovery_hint: Some("gh run view 12345".into()),
            shelved_by: Some("implementer".into()),
            shelved_at: now,
        };
        let j = serde_json::to_string(&fr).unwrap();
        let back: FailureReason = serde_json::from_str(&j).unwrap();
        assert_eq!(back, fr);
    }

    /// EPIC-28: optional fields drop out when None so the on-disk YAML
    /// stays minimal for the common case.
    /// trace:EPIC-28 | ai:claude
    #[test]
    fn failure_reason_skip_serialise_when_none() {
        let fr = FailureReason {
            phase: "build".into(),
            phase_index: 6,
            kind: "build-failed".into(),
            detail: "cargo build --release exit 101".into(),
            recovery_hint: None,
            shelved_by: None,
            shelved_at: chrono::Utc::now(),
        };
        let j = serde_json::to_string(&fr).unwrap();
        assert!(!j.contains("recovery_hint"));
        assert!(!j.contains("shelved_by"));
    }

    /// STORY-332: PuntCategory parses its kebab form and is tolerant of
    /// casing / separator drift.
    #[test]
    fn punt_category_parse_round_trips() {
        for cat in PuntCategory::all() {
            assert_eq!(PuntCategory::from_str(&cat.to_string()), Some(cat));
        }
        assert_eq!(
            PuntCategory::from_str("Design_Fork"),
            Some(PuntCategory::DesignFork)
        );
        assert_eq!(PuntCategory::from_str("nonsense"), None);
    }

    #[test]
    fn test_extract_prefix_from_spec_id() {
        assert_eq!(
            RequirementsStore::extract_prefix_from_spec_id("FR-0042"),
            "FR"
        );
        assert_eq!(
            RequirementsStore::extract_prefix_from_spec_id("AUTH-REQ-001"),
            "AUTH-REQ"
        );
        assert_eq!(
            RequirementsStore::extract_prefix_from_spec_id("SPEC-123"),
            "SPEC"
        );
        assert_eq!(
            RequirementsStore::extract_prefix_from_spec_id("IMPL-0001"),
            "IMPL"
        );
        // Edge cases
        assert_eq!(RequirementsStore::extract_prefix_from_spec_id(""), "REQ");
        assert_eq!(
            RequirementsStore::extract_prefix_from_spec_id("no-numbers"),
            "no-numbers"
        );
    }

    #[test]
    fn test_peek_next_spec_id() {
        let store = RequirementsStore::new();
        assert_eq!(store.peek_next_spec_id(), "SPEC-001");

        let mut store2 = RequirementsStore::new();
        store2.next_spec_number = 42;
        assert_eq!(store2.peek_next_spec_id(), "SPEC-042");
    }

    #[test]
    fn test_add_relationship() {
        let mut store = RequirementsStore::new();
        let req1 = Requirement::new("Req1".into(), "Description 1".into());
        let req2 = Requirement::new("Req2".into(), "Description 2".into());

        let id1 = req1.id;
        let id2 = req2.id;

        store.add_requirement_with_spec_id(req1);
        store.add_requirement_with_spec_id(req2);

        // Add parent relationship
        let result = store.add_relationship(&id1, RelationshipType::Parent, &id2, false);
        assert!(result.is_ok());

        // Verify relationship was added
        let req1_updated = store.get_requirement_by_id(&id1).unwrap();
        assert_eq!(req1_updated.relationships.len(), 1);
        assert_eq!(
            req1_updated.relationships[0].rel_type,
            RelationshipType::Parent
        );
        assert_eq!(req1_updated.relationships[0].target_id, id2);
    }

    #[test]
    fn test_add_relationship_bidirectional() {
        let mut store = RequirementsStore::new();
        let req1 = Requirement::new("Req1".into(), "Description 1".into());
        let req2 = Requirement::new("Req2".into(), "Description 2".into());

        let id1 = req1.id;
        let id2 = req2.id;

        store.add_requirement_with_spec_id(req1);
        store.add_requirement_with_spec_id(req2);

        // Add bidirectional parent-child relationship
        let result = store.add_relationship(&id1, RelationshipType::Parent, &id2, true);
        assert!(result.is_ok());

        // Verify forward relationship
        let req1_updated = store.get_requirement_by_id(&id1).unwrap();
        assert_eq!(req1_updated.relationships.len(), 1);
        assert_eq!(
            req1_updated.relationships[0].rel_type,
            RelationshipType::Parent
        );

        // Verify inverse relationship
        let req2_updated = store.get_requirement_by_id(&id2).unwrap();
        assert_eq!(req2_updated.relationships.len(), 1);
        assert_eq!(
            req2_updated.relationships[0].rel_type,
            RelationshipType::Child
        );
        assert_eq!(req2_updated.relationships[0].target_id, id1);
    }

    #[test]
    fn test_add_relationship_self_error() {
        let mut store = RequirementsStore::new();
        let req = Requirement::new("Req".into(), "Description".into());
        let id = req.id;

        store.add_requirement_with_spec_id(req);

        // Try to add self-relationship
        let result = store.add_relationship(&id, RelationshipType::Parent, &id, false);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot create relationship to self"));
    }

    #[test]
    fn test_add_relationship_duplicate_error() {
        let mut store = RequirementsStore::new();
        let req1 = Requirement::new("Req1".into(), "Description 1".into());
        let req2 = Requirement::new("Req2".into(), "Description 2".into());

        let id1 = req1.id;
        let id2 = req2.id;

        store.add_requirement_with_spec_id(req1);
        store.add_requirement_with_spec_id(req2);

        // Add relationship
        store
            .add_relationship(&id1, RelationshipType::Parent, &id2, false)
            .unwrap();

        // Try to add duplicate
        let result = store.add_relationship(&id1, RelationshipType::Parent, &id2, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn test_remove_relationship() {
        let mut store = RequirementsStore::new();
        let req1 = Requirement::new("Req1".into(), "Description 1".into());
        let req2 = Requirement::new("Req2".into(), "Description 2".into());

        let id1 = req1.id;
        let id2 = req2.id;

        store.add_requirement_with_spec_id(req1);
        store.add_requirement_with_spec_id(req2);
        store
            .add_relationship(&id1, RelationshipType::Parent, &id2, false)
            .unwrap();

        // Remove relationship
        let result = store.remove_relationship(&id1, &RelationshipType::Parent, &id2, false);
        assert!(result.is_ok());

        // Verify it was removed
        let req1_updated = store.get_requirement_by_id(&id1).unwrap();
        assert_eq!(req1_updated.relationships.len(), 0);
    }

    #[test]
    fn test_remove_relationship_bidirectional() {
        let mut store = RequirementsStore::new();
        let req1 = Requirement::new("Req1".into(), "Description 1".into());
        let req2 = Requirement::new("Req2".into(), "Description 2".into());

        let id1 = req1.id;
        let id2 = req2.id;

        store.add_requirement_with_spec_id(req1);
        store.add_requirement_with_spec_id(req2);
        store
            .add_relationship(&id1, RelationshipType::Parent, &id2, true)
            .unwrap();

        // Remove bidirectional relationship
        let result = store.remove_relationship(&id1, &RelationshipType::Parent, &id2, true);
        assert!(result.is_ok());

        // Verify both sides were removed
        let req1_updated = store.get_requirement_by_id(&id1).unwrap();
        assert_eq!(req1_updated.relationships.len(), 0);

        let req2_updated = store.get_requirement_by_id(&id2).unwrap();
        assert_eq!(req2_updated.relationships.len(), 0);
    }

    #[test]
    fn test_relationship_type_from_str() {
        assert_eq!(
            RelationshipType::from_str("parent"),
            RelationshipType::Parent
        );
        assert_eq!(RelationshipType::from_str("child"), RelationshipType::Child);
        assert_eq!(
            RelationshipType::from_str("duplicate"),
            RelationshipType::Duplicate
        );
        assert_eq!(
            RelationshipType::from_str("verifies"),
            RelationshipType::Verifies
        );
        assert_eq!(
            RelationshipType::from_str("verified-by"),
            RelationshipType::VerifiedBy
        );
        assert_eq!(
            RelationshipType::from_str("references"),
            RelationshipType::References
        );

        // Test custom type
        if let RelationshipType::Custom(name) = RelationshipType::from_str("implements") {
            assert_eq!(name, "implements");
        } else {
            panic!("Expected Custom variant");
        }
    }

    #[test]
    fn test_relationship_type_inverse() {
        assert_eq!(
            RelationshipType::Parent.inverse(),
            Some(RelationshipType::Child)
        );
        assert_eq!(
            RelationshipType::Child.inverse(),
            Some(RelationshipType::Parent)
        );
        assert_eq!(
            RelationshipType::Verifies.inverse(),
            Some(RelationshipType::VerifiedBy)
        );
        assert_eq!(
            RelationshipType::VerifiedBy.inverse(),
            Some(RelationshipType::Verifies)
        );
        assert_eq!(
            RelationshipType::Duplicate.inverse(),
            Some(RelationshipType::Duplicate)
        );
        assert_eq!(RelationshipType::References.inverse(), None);
        assert_eq!(RelationshipType::Custom("test".to_string()).inverse(), None);
    }

    // STORY-522: the async decision-inbox artifact. trace:STORY-522 | ai:claude

    fn sample_decision_request() -> DecisionRequest {
        DecisionRequest {
            question: "Promote STORY-X to an EPIC, or ship as one story?".to_string(),
            choices: vec![
                DecisionChoice {
                    label: "Promote to EPIC".to_string(),
                    consequence: "decompose into child stories first".to_string(),
                    resolution: "tag:+epic-candidate".to_string(),
                },
                DecisionChoice {
                    label: "Ship as one story".to_string(),
                    consequence: "implement directly".to_string(),
                    resolution: "status:approved;tag:+ready-to-implement".to_string(),
                },
            ],
            recommended: Some(1),
            rationale: Some("the read/answer slice is bounded".to_string()),
            answered: None,
            asked_at: Some(Utc::now()),
            answered_at: None,
        }
    }

    #[test]
    fn decision_request_is_pending_logic() {
        let mut dr = sample_decision_request();
        assert!(dr.is_pending(), "unanswered request is pending");
        dr.answered = Some(0);
        assert!(!dr.is_pending(), "answered request is no longer pending");
    }

    #[test]
    fn requirement_with_decision_request_round_trips() {
        let mut req = Requirement::new("Async decision protocol".to_string(), String::new());
        req.decision_request = Some(sample_decision_request());

        let yaml = serde_yaml::to_string(&req).unwrap();
        // The field serializes under its snake_case name.
        assert!(yaml.contains("decision_request:"), "yaml: {yaml}");
        assert!(yaml.contains("Promote to EPIC"));

        let parsed: Requirement = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.decision_request, req.decision_request);
        let dr = parsed.decision_request.unwrap();
        assert_eq!(dr.choices.len(), 2);
        assert_eq!(dr.recommended, Some(1));
        assert!(dr.is_pending());
        assert_eq!(
            dr.choices[1].resolution,
            "status:approved;tag:+ready-to-implement"
        );
    }

    #[test]
    fn requirement_without_decision_request_serializes_clean() {
        // Backward-compat invariant: a spec with no decision skips the field
        // entirely (skip_serializing_if), so existing YAML stays untouched.
        let req = Requirement::new("plain".to_string(), String::new());
        assert!(req.decision_request.is_none());
        let yaml = serde_yaml::to_string(&req).unwrap();
        assert!(
            !yaml.contains("decision_request"),
            "absent field must not serialize: {yaml}"
        );
    }

    #[test]
    fn legacy_requirement_yaml_without_field_deserializes() {
        // CRITICAL: a Requirement YAML written before STORY-522 (no
        // `decision_request` key) must still deserialize, with the field
        // defaulting to None via #[serde(default)].
        let mut req = Requirement::new("legacy".to_string(), "older spec".to_string());
        req.spec_id = Some("STORY-001".to_string());
        let yaml = serde_yaml::to_string(&req).unwrap();
        // Sanity: the serialized legacy form genuinely lacks the field.
        assert!(!yaml.contains("decision_request"));

        let parsed: Requirement = serde_yaml::from_str(&yaml).unwrap();
        assert!(
            parsed.decision_request.is_none(),
            "missing field must default to None"
        );
        assert_eq!(parsed.spec_id.as_deref(), Some("STORY-001"));
    }
}

// =========================================================================
// Queue Entry (STORY-0366)
// =========================================================================

/// Represents an entry in a user's personal work queue
// trace:STORY-0366 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueEntry {
    /// The user whose queue this entry belongs to
    pub user_id: String,
    /// The requirement in the queue
    pub requirement_id: Uuid,
    /// Position in the queue (lower = higher priority)
    pub position: i64,
    /// Who added this entry (may differ from user_id for assigned items)
    pub added_by: String,
    /// Optional note explaining why this was queued
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// When the entry was added
    pub added_at: DateTime<Utc>,
    /// Routing tag: this item is meant for whoever wears this role
    /// (e.g., "implementer", "architect"). Used by `aida queue add --for X`
    /// and surfaced via `aida queue list --role X`. None = unrouted /
    /// general queue.
    /// trace:EPIC-1-001 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_role: Option<String>,
    /// Routing tag: this item is meant for sessions whose lease scope
    /// matches this string (e.g. "EPIC-20"). Filters items so a session
    /// sees only what's targeted at its scope (or unrouted). Default-
    /// populated when `aida queue add` runs inside a session worktree
    /// without `--no-scope`. trace:STORY-57 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_scope: Option<String>,
    /// Routing tag: this item is meant for one specific session,
    /// identified by its lease id (12-char prefix). Mutually exclusive
    /// with `for_scope` in the typical workflow but the queue layer is
    /// permissive — both filters apply if both are set.
    /// trace:STORY-57 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_session: Option<String>,
    /// Machine fingerprint (hostname) of the clone that added this entry.
    /// Stamped by the `aida queue add` CLI path. Used to detect the silent
    /// cross-machine collision hazard when two clones share the BUG-89
    /// "default" user_id and therefore write the SAME
    /// `registry/queues/default.yaml` (concurrent commits → orphan-branch
    /// merge conflict on sync). Optional + serde-default so entries written
    /// before this field, and entries added through non-CLI paths, round-trip
    /// cleanly as `None`.
    // trace:TASK-618 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by_machine: Option<String>,
}
