//! Questions command: decision-request sweep, answer/clarify/remedy flows, research, headless-log question and outage detection (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuestionSweepScope {
    Backlog,
    Approved,
    Planned,
    InProgress,
    All,
}

impl QuestionSweepScope {
    pub(crate) fn parse(raw: Option<&str>) -> Result<Self> {
        match raw.unwrap_or("backlog").trim().to_ascii_lowercase().as_str() {
            "" | "backlog" | "near-term" | "near_term" | "workable" => Ok(Self::Backlog),
            "approved" => Ok(Self::Approved),
            "planned" => Ok(Self::Planned),
            "in-progress" | "in_progress" | "inprogress" => Ok(Self::InProgress),
            "all" => Ok(Self::All),
            other => anyhow::bail!(
                "unknown questions sweep scope `{other}` — expected backlog, approved, planned, in-progress, or all"
            ),
        }
    }

    pub(crate) fn includes_status(self, status: &RequirementStatus) -> bool {
        match self {
            Self::Backlog => matches!(
                status,
                RequirementStatus::Approved
                    | RequirementStatus::Planned
                    | RequirementStatus::InProgress
            ),
            Self::Approved => matches!(status, RequirementStatus::Approved),
            Self::Planned => matches!(status, RequirementStatus::Planned),
            Self::InProgress => matches!(status, RequirementStatus::InProgress),
            // BUG-596: `all` includes DRAFTS. The advisor's distill section
            // points at under-specified drafts (the groom worklist), but the
            // old `all` scope excluded Draft by status, so `aida questions
            // clarify` (which defaults to the swept `all` set) could never
            // reach the very specs the hint flagged — the hint and the sweep
            // contradicted each other. Drafts are exactly where acceptance
            // criteria are missing, so the broadest scope must reach them.
            // (`backlog`/`approved`/etc. still exclude drafts by design.)
            // trace:BUG-596 | ai:claude
            Self::All => matches!(
                status,
                RequirementStatus::Draft
                    | RequirementStatus::Approved
                    | RequirementStatus::Planned
                    | RequirementStatus::InProgress
                    | RequirementStatus::NeedsAttention
            ),
        }
    }
}

/// What KIND of decision a swept candidate needs — which shape of
/// DecisionRequest [`formulate_sweep_decision_request`] should attach.
/// trace:STORY-555 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SweepKind {
    /// Under-specified / design-fork text: the proceed-as-written vs
    /// park-for-clarification question (the original slice-1 sweep shape).
    Clarify,
    /// A spec held ONLY by a disposition parking tag, with no explicit
    /// question — synthesize an approve / reject / keep-parked disposition so
    /// the tag-park inbox converges on the decision inbox. Carries the gating
    /// tag that triggered it. trace:STORY-555 (R2)
    Disposition(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuestionSweepCandidate {
    pub(crate) reason: String,
    pub(crate) kind: SweepKind,
}

pub(crate) fn requirement_text(req: &Requirement) -> String {
    let mut text = format!("{}\n{}", req.title, req.description);
    for comment in &req.comments {
        text.push('\n');
        text.push_str(&comment.content);
    }
    text
}

pub(crate) fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

pub(crate) fn is_advisor_resolvable(req: &Requirement) -> bool {
    if req.tags.iter().any(|tag| {
        matches!(
            tag.as_str(),
            "advisor-resolvable" | "recorded-principle" | "recorded-preference"
        )
    }) {
        return true;
    }
    let text = requirement_text(req).to_ascii_lowercase();
    contains_any(
        &text,
        &[
            "advisor-resolvable",
            "recorded principle",
            "recorded preference",
            "advisor tier handles",
            "advisor tier can",
        ],
    )
}

pub(crate) fn has_open_decision_request(req: &Requirement) -> bool {
    req.decision_request
        .as_ref()
        .is_some_and(aida_core::DecisionRequest::is_pending)
}

pub(crate) fn is_in_questions_sweep_scope(req: &Requirement, scope: QuestionSweepScope) -> bool {
    !req.archived
        && !matches!(req.priority, RequirementPriority::Low)
        && !matches!(
            req.req_type,
            RequirementType::Sprint | RequirementType::Folder | RequirementType::Meta
        )
        && scope.includes_status(&req.status)
}

/// BUG-495: types an autonomous agent never "implements" — strategic
/// (`vision`), organizational (`folder`, `meta`), and knowledge-graph
/// (`principle`, `term`) entries. Flagging them for "missing acceptance
/// criteria" is noise, not signal: they will never be implementation
/// candidates, so the gap is moot.
// trace:BUG-495
pub(crate) fn is_implementable_type(req_type: &RequirementType) -> bool {
    !matches!(
        req_type,
        RequirementType::Vision
            | RequirementType::Folder
            | RequirementType::Meta
            | RequirementType::Principle
            | RequirementType::Term
    )
}

/// BUG-495: a spec whose work is already built or in-flight — at/past Done,
/// tagged `review:draft-only` (work done, awaiting human review), or holding
/// an active lease — has a moot acceptance gap. Excluded from the
/// missing-acceptance flag so the sweep fires only on genuinely-implementable,
/// not-yet-built specs. `in_flight_scopes` is the lowercased lease-scope set
/// from [`in_flight_lease_scopes`].
// trace:BUG-495
pub(crate) fn is_built_or_held(req: &Requirement, in_flight_scopes: &HashSet<String>) -> bool {
    let at_or_past_done = matches!(
        req.status,
        RequirementStatus::Done | RequirementStatus::Completed
    );
    let draft_only = req.tags.iter().any(|t| {
        t.trim()
            .eq_ignore_ascii_case(crate::pr_ship::DRAFT_ONLY_TAG)
    });
    let leased = req
        .spec_id
        .as_ref()
        .is_some_and(|id| in_flight_scopes.contains(&id.to_ascii_lowercase()));
    at_or_past_done || draft_only || leased
}

pub(crate) fn question_sweep_candidate(
    req: &Requirement,
    all: &[Requirement],
    scope: QuestionSweepScope,
    in_flight_scopes: &HashSet<String>,
) -> Option<QuestionSweepCandidate> {
    if !is_in_questions_sweep_scope(req, scope) || has_open_decision_request(req) {
        return None;
    }
    if is_advisor_resolvable(req) {
        return None;
    }

    // STORY-555 (R2): a spec held only by a disposition parking tag, with no
    // explicit question, needs a human DISPOSITION (approve / reject / keep) —
    // not an authored answer. Detect it first (the most explicit park signal)
    // so the tag-park and decision-park inboxes converge on `aida questions`.
    // trace:STORY-555 | ai:claude
    if let Some(tag) = disposition_gating_tag(&req.tags) {
        return Some(QuestionSweepCandidate {
            reason: format!("parked by `{tag}`"),
            kind: SweepKind::Disposition(tag),
        });
    }

    let text = requirement_text(req).to_ascii_lowercase();
    let has_acceptance = contains_any(&text, &["acceptance", "acceptance criteria", "acceptance:"]);
    // BUG-495: the missing-acceptance flag only makes sense for specs an agent
    // would actually implement and that aren't already built/in-flight.
    // Strategic/organizational/knowledge types and built-or-held specs have a
    // moot acceptance gap — excluding them strips false positives. trace:BUG-495
    // BUG-596: a SPIKE legitimately has no `## Acceptance` section — its
    // deliverable is an analysis + a decision under a timebox, not a mergeable
    // PR with pass/fail criteria. Flagging spikes for "missing acceptance" is
    // noise (BUG-495 cleaned up non-implementable handling but spikes escaped),
    // so exclude the Spike type from THIS check. trace:BUG-596 | ai:claude
    if !has_acceptance
        && is_implementable_type(&req.req_type)
        && !matches!(req.req_type, RequirementType::Spike)
        && !is_built_or_held(req, in_flight_scopes)
    {
        return Some(QuestionSweepCandidate {
            reason: "missing acceptance criteria".to_string(),
            kind: SweepKind::Clarify,
        });
    }

    if contains_any(
        &text,
        &[
            "design fork",
            "design-fork",
            "open question",
            "needs human",
            "human decision",
            "operator decision needed",
            "should this even be built",
            "should we build",
            "choose between",
            "ambiguous",
            "unclear",
            "contradictory",
            "contradiction",
            "disagree",
            "tbd",
        ],
    ) {
        return Some(QuestionSweepCandidate {
            reason: "decision-marker text".to_string(),
            kind: SweepKind::Clarify,
        });
    }

    for rel in req
        .relationships
        .iter()
        .filter(|rel| matches!(rel.rel_type, RelationshipType::BlockedBy))
    {
        if let Some(blocker) = all.iter().find(|candidate| candidate.id == rel.target_id) {
            let blocker_text = requirement_text(blocker).to_ascii_lowercase();
            if matches!(blocker.status, RequirementStatus::Rejected)
                || contains_any(
                    &blocker_text,
                    &["ambiguous", "unclear", "design fork", "design-fork"],
                )
            {
                return Some(QuestionSweepCandidate {
                    reason: format!("blocked by ambiguous {}", blocker.display_id()),
                    kind: SweepKind::Clarify,
                });
            }
        }
    }

    None
}

pub(crate) fn formulate_sweep_decision_request(
    req: &Requirement,
    candidate: &QuestionSweepCandidate,
) -> aida_core::DecisionRequest {
    let now = chrono::Utc::now();
    match &candidate.kind {
        // STORY-555 (R2): a tag-parked spec with no explicit question. Offer a
        // DISPOSITION — answering `Approve` clears the gating tag and unparks +
        // queues; `Reject` kills it; `Keep parked` records why and holds. No
        // recommended default: a disposition must be an explicit human call,
        // not an Enter-through. trace:STORY-555 | ai:claude
        SweepKind::Disposition(tag) => aida_core::DecisionRequest {
            question: format!(
                "{} is parked by `{tag}` with no recorded question — what's the disposition?",
                req.display_id(),
            ),
            choices: vec![
                aida_core::DecisionChoice {
                    label: "Approve → ready".to_string(),
                    consequence: format!(
                        "Clears the `{tag}` gate and queues the spec for the burndown ready set."
                    ),
                    resolution: "disposition:approve-to-ready".to_string(),
                },
                aida_core::DecisionChoice {
                    label: "Reject".to_string(),
                    consequence: "Marks the spec Rejected — it will not be implemented.".to_string(),
                    resolution: "disposition:reject".to_string(),
                },
                aida_core::DecisionChoice {
                    label: "Keep parked".to_string(),
                    consequence: "Leaves the spec parked and records a why-open note.".to_string(),
                    resolution: "disposition:keep-parked".to_string(),
                },
            ],
            recommended: None,
            rationale: Some(format!(
                "Held by the `{tag}` parking tag with no question attached; a human disposition unparks it (or confirms the hold).",
            )),
            answered: None,
            note: None,
            asked_at: Some(now),
            answered_at: None,
        },
        SweepKind::Clarify => aida_core::DecisionRequest {
            question: format!(
                "{} appears to need a human decision before implementation ({}) - should it proceed as written or be parked for clarification?",
                req.display_id(),
                candidate.reason
            ),
            choices: vec![
                aida_core::DecisionChoice {
                    label: "Proceed as written".to_string(),
                    consequence: "The spec stays in the near-term queue and implementers use the current text.".to_string(),
                    resolution: "noop".to_string(),
                },
                aida_core::DecisionChoice {
                    label: "Park for clarification".to_string(),
                    consequence: "The spec is deferred until the ambiguity is clarified outside the drain.".to_string(),
                    // A recognized parking tag (matches burndown::parking_tag),
                    // so applying this answer actually keeps the spec parked.
                    // trace:STORY-555 | ai:claude
                    resolution: "tag:+needs-decision".to_string(),
                },
            ],
            recommended: Some(1),
            rationale: Some(format!(
                "The deterministic sweep flagged this as `{}`; parking avoids a mid-drain implementation guess.",
                candidate.reason
            )),
            answered: None,
            note: None,
            asked_at: Some(now),
            answered_at: None,
        },
    }
}

/// Parse a `--choice` string of the form `label|consequence|resolution`
/// into a [`DecisionChoice`]. Each of the three fields is trimmed; all
/// three are required and non-empty. The `resolution` is a deterministic
/// action token (e.g. `status:rejected`, `tag:+deferred`, `noop`) — its
/// shape isn't validated here (slice 1 only records it), but it must be
/// present so the eventual auto-applier has something to act on.
// trace:STORY-522 | ai:claude
pub(crate) fn parse_decision_choice(spec: &str) -> Result<aida_core::DecisionChoice> {
    let parts: Vec<&str> = spec.splitn(3, '|').collect();
    if parts.len() != 3 {
        anyhow::bail!(
            "invalid --choice {spec:?}: expected `label|consequence|resolution` \
             (three fields separated by `|`)"
        );
    }
    let label = parts[0].trim();
    let consequence = parts[1].trim();
    let resolution = parts[2].trim();
    if label.is_empty() || consequence.is_empty() || resolution.is_empty() {
        anyhow::bail!(
            "invalid --choice {spec:?}: label, consequence, and resolution \
             must all be non-empty"
        );
    }
    Ok(aida_core::DecisionChoice {
        label: label.to_string(),
        consequence: consequence.to_string(),
        resolution: resolution.to_string(),
    })
}

/// Resolve a user-typed choice token (`"2"`, `"default"`, `"recommended"`)
/// to a 0-based choice index, validated against `request`.
// trace:STORY-522 | ai:claude
pub(crate) fn resolve_choice_index(
    request: &aida_core::DecisionRequest,
    choice: &str,
) -> Result<usize> {
    let trimmed = choice.trim();
    if trimmed.eq_ignore_ascii_case("default") || trimmed.eq_ignore_ascii_case("recommended") {
        return request.recommended.ok_or_else(|| {
            anyhow::anyhow!(
                "no recommended default on this question — pick a choice number (1-{})",
                request.choices.len()
            )
        });
    }
    let one_based: usize = trimmed.parse().map_err(|_| {
        anyhow::anyhow!(
            "invalid choice {choice:?}: expected a number 1-{}, `default`, or `recommended`",
            request.choices.len()
        )
    })?;
    if one_based == 0 || one_based > request.choices.len() {
        anyhow::bail!(
            "choice {one_based} out of range — this question has {} choices (1-{})",
            request.choices.len(),
            request.choices.len()
        );
    }
    Ok(one_based - 1)
}

/// Record an answer on a (pending) DecisionRequest: resolve the user-typed
/// `choice` to an index, set `answered` + `answered_at`, return the index.
/// Pure (no I/O) so the record-and-flip logic is unit-testable without a
/// backend. Errors if already answered or the choice is invalid.
// trace:STORY-522 | ai:claude
pub(crate) fn record_answer(
    request: &mut aida_core::DecisionRequest,
    choice: &str,
) -> Result<usize> {
    if !request.is_pending() {
        anyhow::bail!("already answered");
    }
    let idx = resolve_choice_index(request, choice)?;
    request.answered = Some(idx);
    request.answered_at = Some(chrono::Utc::now());
    Ok(idx)
}

/// Confirm the recommended default on a (pending) DecisionRequest. Returns
/// the chosen index, or `None` when there is no default to confirm or the
/// request is already answered. Pure (no I/O). trace:STORY-522 | ai:claude
pub(crate) fn confirm_default(request: &mut aida_core::DecisionRequest) -> Option<usize> {
    if !request.is_pending() {
        return None;
    }
    let idx = request.recommended?;
    request.answered = Some(idx);
    request.answered_at = Some(chrono::Utc::now());
    Some(idx)
}

/// The parking tags that mark a spec as held for a human DISPOSITION
/// (approve / reject / keep-parked) rather than for an authored design answer.
/// A deliberate SUBSET of [`burndown::parking_tag`]'s set — `needs-supervised-
/// build`, `deferred:*`, and `review:draft-only` carry their own resolution
/// path and are NOT a yes/no disposition, so the sweep does not synthesize a
/// disposition for them. trace:STORY-555 | ai:claude
pub(crate) const DISPOSITION_GATING_TAGS: &[&str] = &[
    "needs-design-signoff",
    "needs-design",
    "needs-human",
    "operator-action",
];

/// The first disposition gating tag present on `tags` (case-insensitive),
/// preserving the spec's own casing in the returned value. trace:STORY-555
pub(crate) fn disposition_gating_tag(tags: &std::collections::HashSet<String>) -> Option<String> {
    tags.iter()
        .find(|t| DISPOSITION_GATING_TAGS.contains(&t.trim().to_ascii_lowercase().as_str()))
        .map(|t| t.trim().to_string())
}

/// The effect of applying one answered choice's resolution token to a spec.
/// `effects` are human-readable lines for the answer printout; `is_disposition`
/// suppresses the R1(a) `## Acceptance` refinement (a disposition is a
/// hold/approve/reject, not a design decision that binds the implementer);
/// `keeps_parked` suppresses the R3 auto-queue (the resolution deliberately
/// left the spec held — rejected, kept-parked, or newly parking-tagged).
/// `binds_acceptance` is true only when a genuine design refinement was made
/// that should be written into `## Acceptance` — `noop`, an unrecognized token,
/// or a no-op edit (empty tag, tag-not-present) mutate nothing and so must NOT
/// claim to have bound a decision (else the printout contradicts itself:
/// "no spec change (noop)" + "bound the decision into ## Acceptance"). trace:TASK-884
// trace:STORY-555 | ai:claude
pub(crate) struct ResolutionApplied {
    pub(crate) effects: Vec<String>,
    /// TASK-884: superseded by `binds_acceptance` for the bind decision (a
    /// disposition AND a noop both must not bind), but retained as the explicit
    /// disposition-vs-refinement classification the tests assert on.
    #[allow(dead_code)]
    pub(crate) is_disposition: bool,
    pub(crate) keeps_parked: bool,
    /// Whether this resolution made a real design refinement worth binding into
    /// `## Acceptance`. trace:TASK-884
    pub(crate) binds_acceptance: bool,
}

/// Apply a DecisionChoice `resolution` token to `req` in place — the slice-2
/// auto-applier `record_answer` deferred. The token grammar mirrors what
/// `aida questions ask --choice label|consequence|<resolution>` and the sweep
/// emit:
///   * `noop`                          — no spec change
///   * `tag:+<tag>` / `tag:-<tag>`     — add / remove a tag
///   * `status:<status>`               — set status (e.g. `status:rejected`)
///   * `disposition:approve-to-ready`  — clear every disposition gating tag (R2)
///   * `disposition:reject`            — set status Rejected (R2)
///   * `disposition:keep-parked`       — record a why-open note, leave parked (R2)
///
/// An unrecognized token is recorded-only (no mutation) so a typo never
/// silently mangles a spec. trace:STORY-555 | ai:claude
pub(crate) fn apply_resolution_token(
    req: &mut Requirement,
    token: &str,
    label: &str,
    consequence: &str,
) -> ResolutionApplied {
    let t = token.trim();

    // BUG-726: accept BARE disposition verbs + common synonyms. Advisors and
    // operators naturally write `defer` / `reject` / `queue`, not the internal
    // `disposition:...` form. Before this, a bare verb fell through to the
    // auto-queue at the bottom — the exact OPPOSITE of a `defer`/`reject` intent
    // — and `defer` had no verb at all.
    let lower = t.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "defer" | "park" | "hold" | "passive" | "passive-only" | "shelve"
    ) {
        // trace:BUG-730 | ai:codex
        req.deferred = true;
        if req.deferred_at.is_none() {
            req.deferred_at = Some(chrono::Utc::now());
        }
        let trigger = consequence.trim();
        if !trigger.is_empty() {
            req.deferred_until = Some(trigger.to_string());
        }
        return ResolutionApplied {
            effects: vec!["deferred — parked out of the queue".to_string()],
            is_disposition: true,
            keeps_parked: true,
            binds_acceptance: false,
        };
    }
    let t = match lower.as_str() {
        "reject" | "archive" | "close" | "drop" | "reject/archive" => "disposition:reject",
        "approve" | "queue" | "ready" | "approve-to-ready" | "approve+queue" => {
            "disposition:approve-to-ready"
        }
        "keep-parked" | "keep" | "leave-parked" | "keep-open" => "disposition:keep-parked",
        _ => t,
    };

    // R2: synthesized-disposition tokens — a yes/no/keep disposition on a
    // tag-parked spec, NOT a design refinement.
    if let Some(rest) = t.strip_prefix("disposition:") {
        return match rest {
            "approve-to-ready" => {
                let cleared: Vec<String> = req
                    .tags
                    .iter()
                    .filter(|tag| {
                        DISPOSITION_GATING_TAGS.contains(&tag.trim().to_ascii_lowercase().as_str())
                    })
                    .cloned()
                    .collect();
                for tag in &cleared {
                    req.tags.remove(tag);
                }
                let effects = if cleared.is_empty() {
                    vec!["no disposition gating tag to clear".to_string()]
                } else {
                    vec![format!("cleared gating tag(s): {}", cleared.join(", "))]
                };
                ResolutionApplied {
                    effects,
                    is_disposition: true,
                    keeps_parked: false,
                    binds_acceptance: false,
                }
            }
            "reject" => {
                req.status = RequirementStatus::Rejected;
                ResolutionApplied {
                    effects: vec!["set status → Rejected".to_string()],
                    is_disposition: true,
                    keeps_parked: true,
                    binds_acceptance: false,
                }
            }
            "keep-parked" => {
                push_why_open(req, label, consequence);
                ResolutionApplied {
                    effects: vec!["recorded why-open; left parked".to_string()],
                    is_disposition: true,
                    keeps_parked: true,
                    binds_acceptance: false,
                }
            }
            other => ResolutionApplied {
                effects: vec![format!(
                    "unrecognized disposition `{other}` — recorded only"
                )],
                is_disposition: true,
                keeps_parked: true,
                binds_acceptance: false,
            },
        };
    }

    // tag:+X / tag:-X
    if let Some(rest) = t.strip_prefix("tag:") {
        if let Some(tag) = rest.strip_prefix('+') {
            let tag = tag.trim();
            if tag.is_empty() {
                return ResolutionApplied {
                    effects: vec!["empty tag in resolution — recorded only".to_string()],
                    is_disposition: false,
                    keeps_parked: false,
                    binds_acceptance: false,
                };
            }
            let existed = req.tags.iter().any(|x| x.eq_ignore_ascii_case(tag));
            if !existed {
                req.tags.insert(tag.to_string());
            }
            // Does the tag we just added still park the spec? Ask the same
            // burndown predicate the gate uses so the two never disagree.
            let parks = burndown::parking_tag(&[tag.to_string()]).is_some();
            return ResolutionApplied {
                effects: vec![format!("added tag `{tag}`")],
                is_disposition: false,
                keeps_parked: parks,
                // A genuine tag mutation is a refinement worth binding; a
                // no-op (tag already present) is not. trace:TASK-884
                binds_acceptance: !existed,
            };
        }
        if let Some(tag) = rest.strip_prefix('-') {
            let needle = tag.trim().to_ascii_lowercase();
            let removed: Vec<String> = req
                .tags
                .iter()
                .filter(|x| x.trim().to_ascii_lowercase() == needle)
                .cloned()
                .collect();
            for tag in &removed {
                req.tags.remove(tag);
            }
            return ResolutionApplied {
                effects: vec![if removed.is_empty() {
                    format!("tag `{needle}` not present")
                } else {
                    format!("removed tag `{needle}`")
                }],
                is_disposition: false,
                keeps_parked: false,
                // Removing a present tag is a refinement; "not present" is a
                // no-op and does not bind. trace:TASK-884
                binds_acceptance: !removed.is_empty(),
            };
        }
    }

    // status:<status>
    if let Some(rest) = t.strip_prefix("status:") {
        return match parse_status(rest) {
            Ok(st) => {
                // A terminal Rejected is not queue-worthy; any other status
                // change does not, by itself, keep the spec parked.
                let keeps = matches!(st, RequirementStatus::Rejected);
                req.status = st.clone();
                ResolutionApplied {
                    effects: vec![format!("set status → {st}")],
                    is_disposition: false,
                    keeps_parked: keeps,
                    // A status refinement (e.g. status:planned) is a real
                    // decision worth binding. trace:TASK-884
                    binds_acceptance: true,
                }
            }
            Err(_) => ResolutionApplied {
                effects: vec![format!("unrecognized status `{rest}` — recorded only")],
                is_disposition: false,
                keeps_parked: false,
                binds_acceptance: false,
            },
        };
    }

    if t.eq_ignore_ascii_case("noop") {
        return ResolutionApplied {
            effects: vec!["no spec change (noop)".to_string()],
            is_disposition: false,
            keeps_parked: false,
            // noop mutates nothing — it must NOT claim to bind a decision. trace:TASK-884
            binds_acceptance: false,
        };
    }

    // BUG-726: a freeform resolution (a design DIRECTIVE with no executable verb,
    // e.g. "context-gated") is recorded as guidance and the spec is LEFT PARKED —
    // never silently queued. Auto-queuing here was the bug that made a `defer`/
    // `reject` answer land the spec in the burndown, the opposite of intent.
    ResolutionApplied {
        effects: vec![format!(
            "recorded as directive `{t}` — left parked for advisor disposition"
        )],
        is_disposition: false,
        keeps_parked: true,
        binds_acceptance: false,
    }
}

/// Record a `why-open:` note (the keep-parked disposition's audit trail) as a
/// comment on the spec. trace:STORY-555 | ai:claude
pub(crate) fn push_why_open(req: &mut Requirement, label: &str, consequence: &str) {
    let now = chrono::Utc::now();
    req.comments.push(Comment {
        id: uuid::Uuid::now_v7(),
        author: current_user_id(None),
        content: format!(
            "why-open: kept parked by operator decision ({}) — {label}: {consequence}",
            now.format("%Y-%m-%d")
        ),
        created_at: now,
        modified_at: now,
        parent_id: None,
        replies: Vec::new(),
        reactions: Vec::new(),
        session_id: resolve_current_session_id(), // trace:TASK-330
        relayed_from: None,
    });
}

/// The `## Acceptance` refinement line that BINDS a chosen design decision into
/// the spec (R1(a) — a decision recorded only as a comment does not bind the
/// next implementer; an acceptance refinement does, per the
/// refinements-must-be-acceptance-criteria discipline). trace:STORY-555
pub(crate) fn resolved_acceptance_line(label: &str, consequence: &str) -> String {
    let date = chrono::Utc::now().format("%Y-%m-%d");
    format!(
        "- Resolved ({date}, operator decision via `aida questions answer`): {label} — {consequence}"
    )
}

/// Append `line` to the spec body's `## Acceptance` section, creating the
/// section if absent. Inserts after the section's last non-blank content line
/// (before the next heading) so the refinement reads as the newest acceptance
/// item rather than landing in the blank gap before the following section.
/// trace:STORY-555 | ai:claude
pub(crate) fn append_resolved_to_acceptance(description: &str, line: &str) -> String {
    let lines: Vec<&str> = description.lines().collect();
    let heading_idx = lines.iter().position(|l| {
        let t = l.trim();
        if !t.starts_with('#') {
            return false;
        }
        let h = t.trim_start_matches('#').trim();
        h.eq_ignore_ascii_case("acceptance") || h.eq_ignore_ascii_case("acceptance criteria")
    });

    match heading_idx {
        Some(start) => {
            // The section runs until the next heading (any level) or EOF.
            let mut end = lines.len();
            for (offset, l) in lines.iter().enumerate().skip(start + 1) {
                if l.trim_start().starts_with('#') {
                    end = offset;
                    break;
                }
            }
            let mut insert_at = end;
            while insert_at > start + 1 && lines[insert_at - 1].trim().is_empty() {
                insert_at -= 1;
            }
            let mut out: Vec<String> = lines[..insert_at].iter().map(|s| s.to_string()).collect();
            out.push(line.to_string());
            out.extend(lines[insert_at..].iter().map(|s| s.to_string()));
            out.join("\n")
        }
        None => {
            let mut out = description.trim_end().to_string();
            out.push_str("\n\n## Acceptance\n\n");
            out.push_str(line);
            out.push('\n');
            out
        }
    }
}

/// Re-run the burndown pickability gate against a spec AFTER its answer's
/// resolution has been applied — the honest "is it actually Ready now, or what
/// still holds it?" check (#2 round-trip + #4 honest partial-unpark). Pure over
/// the passed store snapshot. trace:STORY-555 | ai:claude
pub(crate) fn evaluate_unpark(req: &Requirement, all: &[Requirement]) -> burndown::Pickability {
    if req.deferred {
        // A structural defer is a view-state shelf, not a tag-only burndown
        // gate; it must stay parked until `aida undefer` clears it. trace:BUG-730
        let trigger = req
            .deferred_until
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("no trigger recorded");
        return burndown::Pickability::Parked(format!("deferred: {trigger}"));
    }

    let status_by_id: std::collections::HashMap<uuid::Uuid, RequirementStatus> =
        all.iter().map(|r| (r.id, r.status.clone())).collect();
    let has_unsatisfied_blocker = req.relationships.iter().any(|rel| {
        matches!(rel.rel_type, RelationshipType::BlockedBy)
            && status_by_id
                .get(&rel.target_id)
                .map(|s| *s != RequirementStatus::Completed)
                .unwrap_or(true)
    });
    let candidate = burndown::BurndownCandidate {
        id: req.display_id(),
        req_type: format!("{:?}", req.req_type).to_ascii_lowercase(),
        tags: req.tags.iter().cloned().collect(),
        has_unsatisfied_blocker,
        has_pending_decision: req
            .decision_request
            .as_ref()
            .map(|d| d.is_pending())
            .unwrap_or(false),
    };
    burndown::classify(&candidate)
}

/// Apply a just-recorded answer's resolution and, when it fully unparks the
/// spec, auto-queue it — the shared tail of every `aida questions answer` path
/// (single, interactive loop, `--all-defaults`). This is STORY-555's core: the
/// human's answer APPLIES (not just records) the resolution, binds a design
/// decision into `## Acceptance` (R1), clears a disposition gate (R2), and
/// hands the now-decision-free spec to the burndown ready set via the same
/// advisor-gated enqueue path a groom uses (R3). `req` arrives with the answer
/// already recorded on its DecisionRequest. trace:STORY-555 | ai:claude
pub(crate) fn finalize_answer(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    mut req: Requirement,
    idx: usize,
) -> Result<()> {
    let display_id = req.display_id();
    let title = req.title.clone();
    let choice = req
        .decision_request
        .as_ref()
        .and_then(|dr| dr.choices.get(idx).cloned())
        .ok_or_else(|| anyhow::anyhow!("{display_id}: answered choice {idx} is out of range"))?;

    // BUG-1637: the status before the answer; every status change the answer
    // makes (the resolution token and the Draft promotion below) is recorded
    // under the answering caller before the write. trace:BUG-1637 | ai:claude
    let status_before_answer = req.status.clone();

    // Apply the resolution token (mutates tags / status / comments in place).
    let applied = apply_resolution_token(
        &mut req,
        &choice.resolution,
        &choice.label,
        &choice.consequence,
    );

    // R1(a): bind a genuine design decision into ## Acceptance. A disposition
    // (approve/reject/keep) is NOT a design refinement; nor is a noop / no-op
    // edit — `binds_acceptance` is the single predicate so the printout below
    // and the actual mutation never disagree. trace:TASK-884
    if applied.binds_acceptance {
        req.description = append_resolved_to_acceptance(
            &req.description,
            &resolved_acceptance_line(&choice.label, &choice.consequence),
        );
    }

    // BUG-526: when the answerer holds advisor authority (the answer is itself
    // advisor-gated, TASK-647/ADR-3), promote a still-Draft spec to Approved as
    // part of applying the resolution, so the R3 auto-queue below can fire and
    // the ask-ahead/answer-async loop doesn't dead-end at "status is Draft".
    // The resolution must not deliberately keep the spec parked (rejected /
    // kept-parked stay where the chosen resolution left them), and a non-advisor
    // answerer keeps the gate — their answer leaves the spec Draft for triage.
    // trace:BUG-526
    let mut promoted_to_approved = false;
    if matches!(req.status, RequirementStatus::Draft)
        && !applied.keeps_parked
        && has_advisor_authority()
    {
        req.status = RequirementStatus::Approved;
        promoted_to_approved = true;
    }
    // trace:BUG-1637 | ai:claude
    record_caller_status_transition(&mut req, &status_before_answer);

    req.modified_at = chrono::Utc::now();
    backend.update_requirement(&req)?;

    println!(
        "{} {display_id} ({}) → {}",
        "Answered".green().bold(),
        title.dimmed(),
        choice.label.bold()
    );
    for effect in &applied.effects {
        println!(
            "  {} {effect}",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    }
    if applied.binds_acceptance {
        println!(
            "  {} bound the decision into ## Acceptance",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    }
    if promoted_to_approved {
        // trace:BUG-526
        println!(
            "  {} promoted Draft → Approved (advisor authority)",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    }

    // R3 + #2 + #4: re-check pickability and, if genuinely Ready, auto-queue.
    let all = backend.list_requirements(false)?;
    match evaluate_unpark(&req, &all) {
        burndown::Pickability::Ready if !applied.keeps_parked => {
            maybe_autoqueue(store_path, &req)?;
        }
        burndown::Pickability::Ready => {
            // keeps_parked — rejected / kept-parked: deliberately not queued.
            println!(
                "  {} not queued (left held by the chosen resolution)",
                crate::glyph(crate::glyphs::Glyph::InFlight).dimmed()
            );
        }
        burndown::Pickability::Parked(reason) => {
            // #4 honest partial-unpark: name what still holds it instead of
            // claiming Ready.
            println!(
                "  {} still parked: {} — not queued",
                crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                reason.dimmed()
            );
        }
    }
    Ok(())
}

/// Queue a fully-unparked spec via the dispatch-gated enqueue path a groom
/// uses (R3). The spec is already Approved, so this routes rather than disposes,
/// and only queues Approved work (the burndown
/// ready scope); reports honestly when it can't. Idempotent — skips a spec
/// already queued. trace:STORY-555 | ai:claude
pub(crate) fn maybe_autoqueue(store_path: &std::path::Path, req: &Requirement) -> Result<()> {
    if !matches!(req.status, RequirementStatus::Approved) {
        println!(
            "  {} unparked, but status is {} (not Approved) — not auto-queued",
            crate::glyph(crate::glyphs::Glyph::InFlight).dimmed(),
            req.status
        );
        return Ok(());
    }
    if !has_dispatch_authority() {
        println!(
            "  {} unparked, but this session lacks dispatch authority — ask product, advisor, or integrator to queue",
            crate::glyph(crate::glyphs::Glyph::InFlight).dimmed()
        );
        return Ok(());
    }
    let project_root = find_project_root().ok();
    let already_queued = project_root
        .as_deref()
        .map(all_queued_requirement_ids)
        .map(|ids| ids.contains(&req.id))
        .unwrap_or(false);
    if already_queued {
        println!("  {} already queued — burndown-ready", "→".green());
        return Ok(());
    }
    let storage = Storage::new(store_path);
    let user_id = current_user_id(None);
    backlog::enqueue_groomed(
        &storage,
        req,
        None,
        Some("unparked by `aida questions answer`"),
        &user_id,
    )?;
    println!("  {} queued — now burndown-ready", "→".green().bold());
    Ok(())
}

/// Render a single pending DecisionRequest to stdout — question, numbered
/// choices (recommended marked), and rationale. Shared by the list view
/// and the interactive answer loop.
// trace:STORY-522 | ai:claude
pub(crate) fn print_decision_request(
    display_id: &str,
    title: &str,
    request: &aida_core::DecisionRequest,
) {
    println!(
        "{} {}  {}",
        "Decision needed:".yellow().bold(),
        display_id.cyan().bold(),
        title.dimmed()
    );
    println!("  {}", request.question);
    for (i, choice) in request.choices.iter().enumerate() {
        let marker = if request.recommended == Some(i) {
            " (recommended)".green().to_string()
        } else {
            String::new()
        };
        println!(
            "    {}{} {} — {}",
            format!("{}.", i + 1).bold(),
            marker,
            choice.label.bold(),
            choice.consequence.dimmed()
        );
    }
    if let Some(rationale) = &request.rationale {
        println!("  {} {}", "Why:".dimmed(), rationale);
    }
}

/// Load every requirement carrying a DecisionRequest, partitioned into
/// (pending, answered). Reads full objects (the cache does not project the
/// `decision_request` field, mirroring history). trace:STORY-522 | ai:claude
pub(crate) fn collect_decision_requests(
    backend: &aida_core::CachedGitBackend,
) -> Result<(Vec<aida_core::Requirement>, Vec<aida_core::Requirement>)> {
    let mut pending = Vec::new();
    let mut answered = Vec::new();
    for req in backend.list_requirements(false)? {
        match &req.decision_request {
            Some(dr) if dr.is_pending() => pending.push(req),
            Some(_) => answered.push(req),
            None => {}
        }
    }
    Ok((pending, answered))
}

/// TASK-1061 → TASK-1065: the store-based pending-DecisionRequest count. This is
/// now the **reference oracle** for the cache-backed
/// `CachedGitBackend::pending_decision_count`: the `has_pending_decision` cache
/// column projects exactly this predicate (non-archived +
/// `decision_request.is_pending()`) per row, so `aida status --full`'s
/// decision-inbox line reads the cache column instead of a full store load. Kept
/// (test-only) as the equivalence anchor its test module asserts against.
// trace:TASK-1065 (supersedes TASK-1061)
#[cfg(test)]
pub(crate) fn pending_decision_request_count(
    store: &aida_core::models::RequirementsStore,
) -> usize {
    store
        .requirements
        .iter()
        // Match `list_requirements(false)` exactly: archived rows are excluded
        // (the predicate the previous `collect_decision_requests` path used).
        .filter(|req| !req.archived)
        .filter(|req| {
            req.decision_request
                .as_ref()
                .map(|dr| dr.is_pending())
                .unwrap_or(false)
        })
        .count()
}

// TASK-1061: the store-backed pending-decision count that replaced the
// `--full` presence section's redundant second full-store load must agree
// EXACTLY with the prior `collect_decision_requests` pending count — same
// predicate (`decision_request.is_pending()`), same archived-excluded scope
// (`list_requirements(false)`). trace:TASK-1061
#[cfg(test)]
#[path = "tests/pending_decision_request_count_tests.rs"]
mod pending_decision_request_count_tests;

/// `aida questions` / `aida questions list` / `ask` / `answer` dispatch.
// trace:STORY-522 | ai:claude
pub(crate) fn handle_questions_command(
    cmd: Option<&QuestionsCommand>,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        // Bare `aida questions` — list, then (TTY + pending) offer the loop.
        None => {
            let pending_count = questions_list(backend)?;
            let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
            if interactive && pending_count > 0 {
                print!("\nAnswer {pending_count} now? [Y/n] ");
                use std::io::Write;
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let answer = input.trim();
                if answer.is_empty() || answer.eq_ignore_ascii_case("y") {
                    questions_answer_loop(backend, store_path)?;
                }
            }
            Ok(())
        }
        Some(QuestionsCommand::List) => {
            questions_list(backend)?;
            Ok(())
        }
        Some(QuestionsCommand::Sweep { scope, apply }) => {
            questions_sweep(backend, scope.as_deref(), *apply)?;
            Ok(())
        }
        Some(QuestionsCommand::Clarify { specs, dry_run }) => {
            questions_clarify(backend, store_path, specs, *dry_run)
        }
        Some(QuestionsCommand::Remedy { spec, dry_run }) => {
            questions_remedy(backend, spec, *dry_run)
        }
        Some(QuestionsCommand::Ask {
            spec,
            question,
            choice,
            recommend,
            rationale,
            force,
        }) => questions_ask(
            backend,
            store_path,
            spec,
            question,
            choice,
            *recommend,
            rationale.as_deref(),
            *force,
        ),
        Some(QuestionsCommand::Answer {
            spec,
            choice,
            all_defaults,
            note,
        }) => {
            if *all_defaults {
                if spec.is_some() || choice.is_some() || note.is_some() {
                    anyhow::bail!(
                        "--all-defaults answers every recommended default in bulk; \
                         do not also pass <spec>/<choice>/--note"
                    );
                }
                return questions_answer_all_defaults(backend, store_path);
            }
            match (spec, choice) {
                // trace:TASK-791 | ai:claude
                (Some(spec), Some(choice)) => {
                    questions_answer_one(backend, store_path, spec, choice, note.as_deref())
                }
                (None, None) => {
                    if note.is_some() {
                        anyhow::bail!(
                            "--note attaches a counter-proposal to a specific answer; \
                             pass <spec> and <choice> too"
                        );
                    }
                    questions_answer_loop(backend, store_path)
                }
                _ => anyhow::bail!(
                    "answer takes either both <spec> and <choice> \
                     (non-interactive) or neither (interactive loop)"
                ),
            }
        }
    }
}

/// `aida decide <spec>` — the natural human-resolution entry point. A thin
/// dispatcher over the `aida questions` verbs: if the spec carries a PENDING
/// DecisionRequest (enumerated choices), route to the answer path; otherwise
/// the spec is under-specified, so route to the interactive clarifier. The
/// underlying logic stays in `aida questions` — this only picks the lane and
/// reuses the existing handlers.
// trace:TASK-779 | ai:claude
/// STORY-568: the research/spike lane. Dispatch a SPIKE to a headless research
/// agent (reusing the `deep-research` skill), capture the source-grounded
/// analysis as the deliverable (a dated artifact + a comment on the spike),
/// and ESCALATE any strategic decision to the questions inbox — never
/// auto-applied. The agent-able counterpart to the implementer drain; a
/// spike's "done" is deliverable-produced, not PR-merged.
///
/// `--dry-run` composes the prompt + classification + artifact paths and stops
/// (no spawn, no writes) so the advisor can inspect before paying for a run.
// trace:STORY-568 | ai:claude
pub(crate) fn handle_research_command(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    id: &str,
    dry_run: bool,
    artifact_dir: &str,
) -> Result<()> {
    let mut req = backend
        .get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.display_id();

    // Reclassify rather than flatly refuse: a genuinely human-only spike is
    // not agent-able; everything else is the research lane.
    if !matches!(req.req_type, aida_core::RequirementType::Spike) {
        eprintln!(
            "{} {display_id} is a {} — the research lane is for spikes. Proceeding anyway.",
            "Note:".yellow(),
            format!("{:?}", req.req_type).to_lowercase()
        );
    }
    let tags: Vec<String> = req.tags.iter().cloned().collect();
    if matches!(
        crate::burndown::classify_spike_lane(&tags),
        crate::burndown::SpikeLane::HumanOnly
    ) {
        anyhow::bail!(
            "{display_id} is tagged `human-only` — human analysis required; the \
             research lane does not dispatch it. Remove the tag to make it \
             agent-able."
        );
    }

    // Resolve artifact paths (dated). Body = description + the spike's own
    // acceptance text already lives in `description` for AIDA specs.
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let project_root = store_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let dir = project_root.join(artifact_dir);
    let (analysis_path, sidecar_path) =
        crate::research::research_artifact_paths(&dir, &display_id, &date);
    let analysis_rel = format!(
        "{}/{}",
        artifact_dir.trim_end_matches('/'),
        analysis_path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    );

    let prompt = crate::research::build_research_prompt(
        &display_id,
        &req.title,
        &req.description,
        &analysis_path,
        &sidecar_path,
    );

    if dry_run {
        println!(
            "{} {display_id} — {}",
            "Research dispatch:".cyan().bold(),
            req.title
        );
        println!("  lane:     research-lane (agent-able)");
        println!("  analysis: {}", analysis_path.display());
        println!("  sidecar:  {}", sidecar_path.display());
        println!("\n{}", "── research prompt ──".dimmed());
        println!("{prompt}");
        println!(
            "\n{}",
            "Dry run — nothing spawned or written. Drop --dry-run to dispatch.".dimmed()
        );
        return Ok(());
    }

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating artifact dir {}", dir.display()))?;

    // Dispatch the headless research agent. The spawn is the integration
    // boundary; the pure transforms above/below are unit-tested.
    println!(
        "{} {display_id} — dispatching research agent (deep-research, headless)…",
        "Research:".cyan().bold()
    );
    let session_id = Uuid::now_v7().to_string();
    let log_path = project_root.join(format!(".aida/research/{display_id}-{date}.log"));
    let tee = crate::headless_tee::TeeOptions::from_env_and_flag(false)
        .with_label(format!("research:{display_id}"));
    let status =
        crate::session::spawn_claude_headless(&prompt, &session_id, &log_path, &tee, false)
            .context("spawning headless research agent")?;
    if !status.success() {
        anyhow::bail!(
            "research agent exited with {} — see log {}",
            status.code().unwrap_or(-1),
            log_path.display()
        );
    }

    // Collect the deliverable. The analysis artifact is mandatory; the sidecar
    // is optional (informational spikes have no decision to escalate).
    if !analysis_path.exists() {
        anyhow::bail!(
            "research agent did not produce the analysis artifact at {} — see log {}",
            analysis_path.display(),
            log_path.display()
        );
    }
    let now = chrono::Utc::now();

    // Attach a provenance-tagged pointer comment so the deliverable is visible
    // from `aida show` while the full report lives in the dated file.
    let summary = std::fs::read_to_string(&sidecar_path)
        .ok()
        .and_then(|j| crate::research::parse_research_sidecar(&j).ok())
        .map(|s| s.summary)
        .unwrap_or_else(|| "Analysis produced (no machine-readable summary).".to_string());
    req.comments.push(aida_core::Comment {
        id: Uuid::now_v7(),
        content: crate::research::provenance_comment(&analysis_rel, &summary),
        author: "research-agent".to_string(),
        created_at: now,
        modified_at: now,
        parent_id: None,
        replies: Vec::new(),
        reactions: Vec::new(),
        session_id: resolve_current_session_id(), // trace:TASK-330
        relayed_from: None,
    });

    // Escalate the decision (if any) — never auto-apply it. A pending decision
    // parks the spike from any drain and surfaces in `aida questions`.
    let mut escalated: Option<aida_core::DecisionRequest> = None;
    if let Ok(json) = std::fs::read_to_string(&sidecar_path) {
        if let Ok(sidecar) = crate::research::parse_research_sidecar(&json) {
            if let Some(dr) = crate::research::sidecar_to_decision_request(&sidecar, now) {
                if req
                    .decision_request
                    .as_ref()
                    .map(|d| d.is_pending())
                    .unwrap_or(false)
                {
                    eprintln!(
                        "{} {display_id} already has a pending decision — leaving it; \
                         the new analysis is attached as a comment.",
                        "Note:".yellow()
                    );
                } else {
                    req.decision_request = Some(dr.clone());
                    escalated = Some(dr);
                }
            }
        }
    }

    // Deliverable-produced = spike "done" (NOT PR-merged). Tag it so the lane
    // is auditable and the spike reads as research-complete.
    req.tags.insert("spike:deliverable-produced".to_string());
    req.modified_at = now;
    backend.update_requirement(&req)?;

    println!(
        "{} analysis at {}",
        format!("{} deliverable:", crate::glyph(crate::glyphs::Glyph::Check))
            .green()
            .bold(),
        analysis_path.display()
    );
    println!("  pointer comment attached to {display_id}");
    if let Some(dr) = escalated {
        println!(
            "\n{}",
            "Decision escalated — answer in your own time:".bold()
        );
        print_decision_request(&display_id, &req.title, &dr);
        println!(
            "{}",
            "Answer with `aida questions answer` (or `aida questions` at a TTY).".dimmed()
        );
    } else {
        println!(
            "{}",
            "  no strategic decision surfaced — informational deliverable only.".dimmed()
        );
    }
    Ok(())
}

/// `aida questions sweep` - proactive producer for the async decision inbox.
/// The cheap pre-filter is deterministic; only flagged candidates receive a
/// formulated DecisionRequest, and specs with open requests are skipped for
/// idempotency. trace:STORY-523 | ai:codex
pub(crate) fn questions_sweep(
    backend: &aida_core::CachedGitBackend,
    raw_scope: Option<&str>,
    apply: bool,
) -> Result<usize> {
    let scope = QuestionSweepScope::parse(raw_scope)?;
    let all = backend.list_requirements(false)?;
    // BUG-495: exclude specs holding an active lease (work in-flight) from the
    // missing-acceptance flag. Reuse the same live-lease detection the queue +
    // session surfaces use, so the sweep and the rest of the system agree on
    // what's in-flight. trace:BUG-495
    let in_flight_scopes = find_project_root()
        .map(|root| in_flight_lease_scopes(&root))
        .unwrap_or_default();
    let mut candidates = Vec::new();

    for req in &all {
        if let Some(candidate) = question_sweep_candidate(req, &all, scope, &in_flight_scopes) {
            candidates.push((req.display_id(), candidate));
        }
    }

    if candidates.is_empty() {
        println!(
            "{}",
            "Question sweep found no new human-decision candidates.".dimmed()
        );
        return Ok(0);
    }

    // TASK-700: default to a read-only preview — the sweep is a coarse keyword
    // heuristic that MUTATES specs (attaches DecisionRequests), so it must not
    // write without an explicit --apply. Mirrors `integrate --dry-run`.
    // trace:TASK-700 | ai:claude
    let mut affected = 0usize;
    for (spec, candidate) in candidates {
        // trace:TASK-1468 | ai:claude
        let Some(mut req) = backend.get_requirement_unambiguous(&spec)? else {
            continue;
        };
        if has_open_decision_request(&req) {
            continue;
        }
        if apply {
            let request = formulate_sweep_decision_request(&req, &candidate);
            req.decision_request = Some(request);
            req.modified_at = chrono::Utc::now();
            backend.update_requirement(&req)?;
            println!(
                "  {} {} - {}",
                "Recorded".green(),
                spec.cyan(),
                candidate.reason.dimmed()
            );
        } else {
            println!(
                "  {} {} - {}",
                "[dry-run] would attach".dimmed(),
                spec.cyan(),
                candidate.reason.dimmed()
            );
        }
        affected += 1;
    }

    if apply {
        println!(
            "{} attached a DecisionRequest to {affected} spec{} — each is parked from the burndown until its question is answered (`aida questions answer`).",
            "Done.".green().bold(),
            if affected == 1 { "" } else { "s" }
        );
    } else {
        // BUG-495: name what --apply actually does. It attaches a DecisionRequest
        // that PARKS the spec from the burndown (records the gap); it does NOT
        // write or generate the missing acceptance criteria. Point at the real
        // fix for a missing-acceptance flag so the two are not conflated.
        // trace:BUG-495
        println!(
            "{} {affected} spec{} flagged. Re-run with {} to attach a DecisionRequest (parks each spec from the burndown until answered) — it does not write the missing acceptance criteria.",
            "Dry-run:".yellow().bold(),
            if affected == 1 { "" } else { "s" },
            "--apply".cyan()
        );
        println!(
            "  {} to resolve a `missing acceptance criteria` flag, add a {} section to the spec.",
            "Tip:".dimmed(),
            "## Acceptance".cyan()
        );
    }
    Ok(affected)
}

/// True when a spec must NOT be offered to the clarify loop, honouring the
/// BUG-495 exclusions: non-implementable types (vision/folder/meta/principle/
/// term), already-built/terminal status (Done/Completed/Rejected), and
/// held-for-review specs (`review:draft-only`). The active-lease exclusion is
/// applied separately by the caller (it needs the live lease set).
// trace:STORY-557 | ai:claude
pub(crate) fn is_clarify_excluded(req: &Requirement) -> bool {
    if matches!(
        req.req_type,
        RequirementType::Vision
            | RequirementType::Folder
            | RequirementType::Meta
            | RequirementType::Principle
            | RequirementType::Term
    ) {
        return true;
    }
    if matches!(
        req.status,
        RequirementStatus::Done | RequirementStatus::Completed | RequirementStatus::Rejected
    ) {
        return true;
    }
    if req.tags.iter().any(|t| t == "review:draft-only") {
        return true;
    }
    false
}

/// Resolve the default clarify set: specs the sweep would flag as
/// under-specified, minus the BUG-495 exclusions and any spec held by a live
/// lease. Returns display IDs in stable order. trace:STORY-557 | ai:claude
pub(crate) fn clarify_default_specs(backend: &aida_core::CachedGitBackend) -> Result<Vec<String>> {
    let all = backend.list_requirements(false)?;
    let project_root = find_project_root().ok();
    let in_flight: HashSet<String> = project_root
        .as_deref()
        .map(in_flight_lease_scopes)
        .unwrap_or_default();
    let mut out = Vec::new();
    for req in &all {
        if is_clarify_excluded(req) {
            continue;
        }
        // Active-lease exclusion: an implementer is mid-flight on this scope.
        let live = !in_flight.is_empty()
            && [req.agreed_id.as_deref(), req.spec_id.as_deref()]
                .into_iter()
                .flatten()
                .any(|s| in_flight.contains(&s.to_ascii_lowercase()));
        if live {
            continue;
        }
        // Reuse the sweep's under-specification detector across every workable
        // scope so clarify resolves exactly what sweep flags.
        if question_sweep_candidate(req, &all, QuestionSweepScope::All, &in_flight).is_some() {
            out.push(req.display_id());
        }
    }
    Ok(out)
}

/// `aida questions clarify [<spec>...]` — fire up an INTERACTIVE advisor
/// (`claude "/aida-clarify <specs>"`, never headless `claude -p`) that walks
/// the human through authoring acceptance criteria for under-specified specs.
/// The interactive counterpart to `aida burndown run` (headless): sweep
/// detects, clarify resolves. With no specs it defaults to the swept set.
// trace:STORY-557 | ai:claude
pub(crate) fn questions_clarify(
    backend: &aida_core::CachedGitBackend,
    _store_path: &std::path::Path,
    specs: &[String],
    dry_run: bool,
) -> Result<()> {
    // Resolve the target set: explicit args, or the default swept set.
    let targets: Vec<String> = if specs.is_empty() {
        let resolved = clarify_default_specs(backend)?;
        if resolved.is_empty() {
            println!(
                "{}",
                "No under-specified specs to clarify — the sweep is clean.".dimmed()
            );
            return Ok(());
        }
        resolved
    } else {
        // Explicit specs: drop any that are non-clarifiable, warn loudly.
        let mut kept = Vec::new();
        for raw in specs {
            // trace:TASK-1468 | ai:claude
            match backend.get_requirement_unambiguous(raw)? {
                Some(req) if is_clarify_excluded(&req) => {
                    println!(
                        "  {} {} skipped — not clarifiable (built/held/non-implementable type)",
                        "·".dimmed(),
                        req.display_id().yellow()
                    );
                }
                Some(req) => kept.push(req.display_id()),
                None => println!("  {} {} not found — skipped", "·".dimmed(), raw.yellow()),
            }
        }
        if kept.is_empty() {
            anyhow::bail!("no clarifiable specs in the given set");
        }
        kept
    };

    println!(
        "{} clarify",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    println!(
        "  {} {} spec(s): {}",
        "→".green(),
        targets.len(),
        targets.join(", ").cyan()
    );

    // Build the interactive invocation. Unlike the headless drain, clarify
    // needs a human at the keyboard, so it launches plain interactive
    // `claude "<prompt>"` — NOT `claude -p`.
    let prompt = format!("/aida-clarify {}", targets.join(" "));

    if dry_run {
        println!("\n{} dry run — not launching. Would run:", "·".dimmed());
        println!("  claude {prompt:?}");
        return Ok(());
    }

    println!(
        "  {} launching interactive `claude` — answer its questions to author acceptance",
        "→".green()
    );
    println!(
        "  {} {}",
        "→".green(),
        format!("claude {prompt:?}").dimmed()
    );
    println!();

    // Interactive: inherit the terminal so the human can converse. `claude` is
    // resolved off PATH (matching every other launch site); a missing binary
    // surfaces as a guided error, not a raw ENOENT. STORY-762: on a
    // codex-only machine the guidance names the alternatives instead of just
    // demanding an install the machine may not permit — the clarify loop
    // drives a Claude Code skill, so Codex parity arrives with the
    // skills-parity work, not by swapping the binary here.
    let project_root = find_project_root()?;
    let child_grant =
        seat_authority::issue_child(&project_root, "advisor", &current_user_id(None))?;
    let status_code = build_questions_clarify_command(&prompt, &child_grant.id)
        .status()
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to launch `claude` ({e}) — the acceptance-authoring loop drives a \
                 Claude Code skill and needs the Claude Code CLI on PATH.\n\
                 On a Codex-only machine, instead:\n\
                 - answer questions directly: aida questions list / aida questions answer\n\
                 - or edit acceptance by hand: aida edit <spec-id> --description ..."
            )
        })?;

    if status_code.success() {
        Ok(())
    } else {
        let code = status_code.code().unwrap_or(1);
        std::process::exit(code);
    }
}

// trace:BUG-2124 | ai:antigravity
pub(crate) fn build_questions_clarify_command(
    prompt: &str,
    child_grant_id: &str,
) -> std::process::Command {
    let mut cmd = std::process::Command::new("claude");
    cmd.arg(prompt)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env(seat_authority::GRANT_ENV, child_grant_id);
    if crate::session::containment_applied(crate::session::HeadlessVendor::Claude, false) {
        cmd.env("AIDA_CONTAINED", "1");
    } else {
        cmd.env_remove("AIDA_CONTAINED");
    }
    cmd
}

/// `aida questions remedy <spec>` — the headless, bounded acceptance-authoring
/// pass consumed by the TUI drive auto-remedy loop. Unlike
/// [`questions_clarify`], this path must not ask the human anything: it either
/// grounds minimal acceptance criteria from the existing substrate and writes
/// them, or exits non-zero so the caller can keep the gate held.
// trace:TASK-1075 | ai:codex
pub(crate) fn questions_remedy(
    backend: &aida_core::CachedGitBackend,
    spec: &str,
    dry_run: bool,
) -> Result<()> {
    let req = backend
        .get_requirement_unambiguous(spec)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| anyhow::anyhow!("{spec} not found"))?;
    if is_clarify_excluded(&req) {
        anyhow::bail!(
            "{} is not eligible for automatic acceptance remedy (built/held/non-implementable type)",
            req.display_id()
        );
    }

    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let vendor = session::resolve_headless_vendor(&project_root);
    let prompt = questions_remedy_prompt(&req.display_id());
    let log_path = questions_remedy_log_path(&project_root, &req.display_id());

    println!("remedying {} with headless advisor", req.display_id());
    println!("  vendor: {}", vendor.as_str());
    println!("  log: {}", log_path.display());

    if dry_run {
        println!("\ndry run — not launching. Prompt:\n\n{prompt}");
        return Ok(());
    }

    let tee = crate::headless_tee::TeeOptions::from_env_and_flag(false)
        .with_label(format!("remedy-{}", req.display_id().to_ascii_lowercase()));
    let session_id = uuid::Uuid::now_v7().to_string();
    let status = session::spawn_vendor_headless_with_seat(
        vendor,
        aida_core::agents_config::AgentSeat::Advisor,
        &prompt,
        &session_id,
        &log_path,
        &tee,
        false,
    );
    let status = status?;
    if status.success() {
        Ok(())
    } else {
        let code = status.code().unwrap_or(1);
        std::process::exit(code);
    }
}

// trace:TASK-1075 | ai:codex
pub(crate) fn questions_remedy_log_path(
    project_root: &std::path::Path,
    spec: &str,
) -> std::path::PathBuf {
    let safe: String = spec
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    project_root
        .join(".aida")
        .join("headless-logs")
        .join(format!("remedy-{safe}-{ts}.jsonl"))
}

// trace:TASK-1075 | ai:codex
pub(crate) fn questions_remedy_prompt(spec: &str) -> String {
    format!(
        r#"You are AIDA's bounded headless advisor remedy for {spec}.

Goal: make exactly one under-specified requirement drive-ready by adding a minimal, testable `## Acceptance` section when the current substrate is sufficient.

Rules:
- Do not ask the human questions and do not use AskUserQuestion.
- Read the requirement first: `aida show {spec}`.
- Read nearby context if needed: `aida graph tree {spec}` and `aida graph blocked-by {spec}`.
- If the current description, title, comments, or graph context are enough, replace the spec description with the same prose plus a `## Acceptance` section containing crisp observable bullets.
- Preserve all existing description content and existing headings. Do not change status, priority, type, relationships, queue state, code, or unrelated specs.
- Bind the edit with `aida edit {spec} --description "<full updated description>"`. For multi-line text, use a temp file and shell substitution.
- After editing, run `aida zen {spec} --json`. Exit 0 only if the verdict is `ready`.
- If grounded acceptance cannot be inferred, or the gate still holds for a reason you cannot fix by acceptance criteria, leave the spec unchanged if possible and exit non-zero.

This is an advisor-authoring remedy, not an implementation pass."#
    )
}

/// List the decision inbox. Returns the count of pending requests so the
/// bare-invocation caller can decide whether to offer the answer loop.
// trace:STORY-522 | ai:claude
pub(crate) fn questions_list(backend: &aida_core::CachedGitBackend) -> Result<usize> {
    let (pending, answered) = collect_decision_requests(backend)?;
    if pending.is_empty() && answered.is_empty() {
        println!(
            "{}",
            "Decision inbox empty — no questions recorded.".dimmed()
        );
        return Ok(0);
    }
    if !pending.is_empty() {
        println!(
            "{} ({})",
            "Pending decisions".yellow().bold(),
            pending.len()
        );
        for req in &pending {
            if let Some(dr) = &req.decision_request {
                println!();
                print_decision_request(&req.display_id(), &req.title, dr);
            }
        }
    }
    if !answered.is_empty() {
        println!();
        println!("{} ({})", "Answered".green().bold(), answered.len());
        for req in &answered {
            if let Some(dr) = &req.decision_request {
                if let Some(idx) = dr.answered {
                    let label = dr.choices.get(idx).map(|c| c.label.as_str()).unwrap_or("?");
                    println!(
                        "  {:<14} {} → {}",
                        req.display_id().cyan(),
                        req.title.dimmed(),
                        label.bold()
                    );
                    // TASK-791: surface the counter-proposal note so the
                    // implementer reads choice + note.
                    if let Some(note) = &dr.note {
                        println!("  {:<14} {} {}", "", "note:".dimmed(), note);
                    }
                }
            }
        }
    }
    Ok(pending.len())
}

/// `aida questions ask` — pose a DecisionRequest on a spec.
// trace:STORY-522 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn questions_ask(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    spec: &str,
    question: &str,
    choice_specs: &[String],
    recommend: Option<usize>,
    rationale: Option<&str>,
    force: bool,
) -> Result<()> {
    if choice_specs.len() < 2 {
        anyhow::bail!(
            "a decision needs at least two choices (got {}) — pass --choice \
             `label|consequence|resolution` twice or more",
            choice_specs.len()
        );
    }
    let choices: Vec<aida_core::DecisionChoice> = choice_specs
        .iter()
        .map(|c| parse_decision_choice(c))
        .collect::<Result<_>>()?;

    // recommend is 1-based on the CLI; store 0-based.
    let recommended = match recommend {
        Some(n) => {
            if n == 0 || n > choices.len() {
                anyhow::bail!(
                    "--recommend {n} out of range — there are {} choices (1-{})",
                    choices.len(),
                    choices.len()
                );
            }
            Some(n - 1)
        }
        None => None,
    };

    let mut req = backend
        .get_requirement_unambiguous(spec)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(spec, Some(store_path)))?;
    let display_id = req.display_id();

    if let Some(existing) = &req.decision_request {
        if existing.is_pending() && !force {
            anyhow::bail!(
                "{display_id} already has a pending decision — answer it first, \
                 or pass --force to overwrite it"
            );
        }
    }

    let now = chrono::Utc::now();
    let request = aida_core::DecisionRequest {
        question: question.to_string(),
        choices,
        recommended,
        rationale: rationale.map(|s| s.to_string()),
        answered: None,
        note: None,
        asked_at: Some(now),
        answered_at: None,
    };
    req.decision_request = Some(request.clone());
    req.modified_at = now;
    backend.update_requirement(&req)?;

    println!("{} {display_id}", "Decision recorded:".green().bold());
    print_decision_request(&display_id, &req.title, &request);
    println!(
        "{}",
        "Answer it with `aida questions answer` (or `aida questions` at a TTY).".dimmed()
    );
    Ok(())
}

/// Record one answer on a spec's DecisionRequest (the non-interactive,
/// agent-free data op). trace:STORY-522 | ai:claude
// trace:TASK-791 | ai:claude
pub(crate) fn questions_answer_one(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    spec: &str,
    choice: &str,
    note: Option<&str>,
) -> Result<()> {
    let mut req = backend
        .get_requirement_unambiguous(spec)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(spec, Some(store_path)))?;
    let display_id = req.display_id();

    let request = req
        .decision_request
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("{display_id} has no decision to answer"))?;
    if !request.is_pending() {
        anyhow::bail!("{display_id} is already answered");
    }
    let idx = record_answer(request, choice)?;
    // TASK-791: a free-text counter-proposal rides ALONGSIDE the picked option.
    // Pure data: recorded on the request, never interpreted at answer-time.
    if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
        request.note = Some(n.to_string());
    }

    // STORY-555: record was slice 1; now APPLY the resolution + (if it unparks)
    // auto-queue, so answering closes the questions -> burndown loop.
    finalize_answer(backend, store_path, req, idx)
}

/// What the operator chose at the interactive decision prompt. The pure-pick
/// path (`Pick`) stays the default; `PickWithNote` / `Chat` are the TASK-791
/// escapes that mirror /aida-clarify + the AskUserQuestion model.
// trace:TASK-791 | ai:claude
pub(crate) enum DecisionPromptAction {
    /// Skip this question (`s`/`skip`) — leave it pending.
    Skip,
    /// No input + no recommended default — re-prompt / move on without acting.
    NoOp,
    /// The pure-pick path: a 1-based number or `default`/`recommended`.
    Pick(String),
    /// (4) TYPE SOMETHING — a pick plus a free-text counter-proposal note
    /// recorded alongside it (pure data; no LLM at answer-time).
    PickWithNote { choice: String, note: String },
    /// (5) CHAT — drop into the interactive clarifier to discuss before
    /// deciding.
    Chat,
}

/// Render the decision prompt + read one operator action. Shared by the answer
/// loop and the `aida decide` single-spec path so the option list and the two
/// escapes stay identical. trace:TASK-791 | ai:claude
pub(crate) fn prompt_decision_action(
    request: &aida_core::DecisionRequest,
) -> Result<DecisionPromptAction> {
    use std::io::Write;
    let prompt = match request.recommended {
        Some(r) => format!(
            "  Choice [1-{}, Enter={}, s=skip, t=type a note, c=chat]: ",
            request.choices.len(),
            r + 1
        ),
        None => format!(
            "  Choice [1-{}, s=skip, t=type a note, c=chat]: ",
            request.choices.len()
        ),
    };
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let entered = input.trim();

    if entered.eq_ignore_ascii_case("s") || entered.eq_ignore_ascii_case("skip") {
        return Ok(DecisionPromptAction::Skip);
    }
    // (5) CHAT — drop into `aida questions clarify <spec>` to discuss first.
    if entered.eq_ignore_ascii_case("c") || entered.eq_ignore_ascii_case("chat") {
        return Ok(DecisionPromptAction::Chat);
    }
    // (4) TYPE SOMETHING — pick an option, then attach a counter-proposal note.
    // Pure data op: no LLM at answer-time; the implementer reads choice + note.
    if entered.eq_ignore_ascii_case("t")
        || entered.eq_ignore_ascii_case("type")
        || entered.eq_ignore_ascii_case("note")
    {
        let pick = read_line_prompt(&format!(
            "  Which option does the note refine? [1-{}, Enter={}]: ",
            request.choices.len(),
            request
                .recommended
                .map(|r| (r + 1).to_string())
                .unwrap_or_else(|| "?".to_string()),
        ))?;
        let pick = pick.trim();
        let choice = if pick.is_empty() {
            if request.recommended.is_none() {
                println!("  {}", "no default — note needs a choice number".dimmed());
                return Ok(DecisionPromptAction::NoOp);
            }
            "default".to_string()
        } else {
            pick.to_string()
        };
        let note = read_line_prompt("  Your note / counter-proposal: ")?;
        let note = note.trim();
        if note.is_empty() {
            println!("  {}", "empty note — nothing recorded".dimmed());
            return Ok(DecisionPromptAction::NoOp);
        }
        return Ok(DecisionPromptAction::PickWithNote {
            choice,
            note: note.to_string(),
        });
    }

    if entered.is_empty() {
        // Enter accepts the recommended default when one exists.
        if request.recommended.is_none() {
            println!(
                "  {}",
                "no default — type a choice number, t to add a note, c to chat, or s to skip"
                    .dimmed()
            );
            return Ok(DecisionPromptAction::NoOp);
        }
        return Ok(DecisionPromptAction::Pick("default".to_string()));
    }
    Ok(DecisionPromptAction::Pick(entered.to_string()))
}

/// Print a prompt and read one trimmed line. trace:TASK-791 | ai:claude
pub(crate) fn read_line_prompt(prompt: &str) -> Result<String> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input)
}

/// Interactive answer loop over every pending DecisionRequest. Reads a
/// choice number (or `s`/`skip`, `t`=type a note, `c`=chat) from stdin per
/// question. trace:STORY-522 | ai:claude
// trace:TASK-791 | ai:claude
pub(crate) fn questions_answer_loop(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let (pending, _) = collect_decision_requests(backend)?;
    if pending.is_empty() {
        println!("{}", "No pending decisions to answer.".dimmed());
        return Ok(());
    }
    let mut answered = 0usize;
    for req in pending {
        let spec = req.display_id();
        let dr = match &req.decision_request {
            Some(dr) if dr.is_pending() => dr,
            _ => continue,
        };
        println!();
        print_decision_request(&spec, &req.title, dr);
        let (choice, note) = match prompt_decision_action(dr)? {
            DecisionPromptAction::Skip => {
                println!("  {}", "skipped".dimmed());
                continue;
            }
            DecisionPromptAction::NoOp => continue,
            DecisionPromptAction::Chat => {
                // (5) CHAT — hand off to the interactive clarifier on this spec,
                // then move on (the discussion may resolve it out-of-band).
                println!(
                    "  {} dropping into clarify — discuss, then re-run `aida questions answer`",
                    "→".green()
                );
                if let Err(e) =
                    questions_clarify(backend, store_path, std::slice::from_ref(&spec), false)
                {
                    println!("  {} {e}", "error:".red());
                }
                continue;
            }
            DecisionPromptAction::Pick(c) => (c, None),
            DecisionPromptAction::PickWithNote { choice, note } => (choice, Some(note)),
        };
        match questions_answer_one(backend, store_path, &spec, &choice, note.as_deref()) {
            Ok(()) => answered += 1,
            Err(e) => println!("  {} {e}", "error:".red()),
        }
    }
    println!();
    println!("{} {answered} answered.", "Done.".green().bold());
    Ok(())
}

/// `aida questions answer --all-defaults` — confirm every recommended
/// default in one shot. trace:STORY-522 | ai:claude
pub(crate) fn questions_answer_all_defaults(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let (pending, _) = collect_decision_requests(backend)?;
    let mut answered = 0usize;
    let mut skipped = 0usize;
    for mut req in pending {
        let request = match req.decision_request.as_mut() {
            Some(r) if r.is_pending() => r,
            _ => continue,
        };
        let Some(idx) = confirm_default(request) else {
            skipped += 1;
            continue;
        };
        // STORY-555: apply the resolution + auto-queue, same as the single and
        // interactive paths.
        finalize_answer(backend, store_path, req, idx)?;
        answered += 1;
    }
    if answered == 0 && skipped == 0 {
        println!("{}", "No pending decisions to answer.".dimmed());
    } else {
        println!(
            "{} {answered} answered{}.",
            "Done.".green().bold(),
            if skipped > 0 {
                format!(", {skipped} skipped (no recommended default)")
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

// BUG-531: `aida search` mirrors `aida list`'s output-mode flags. These tests
// assert the clap surface (flag + aliases parse, mutual exclusion enforced) at
// parse level — no backend / CWD / HOME / git. trace:BUG-531 | ai:claude
#[cfg(test)]
#[path = "tests/search_output_flags_tests.rs"]
mod search_output_flags_tests;

// STORY-522: `aida questions` parser + record/confirm logic. These tests are
// fully isolated — pure functions over in-memory DecisionRequest values, no
// backend / CWD / HOME / git. trace:STORY-522 | ai:claude
#[cfg(test)]
#[path = "tests/questions_tests.rs"]
mod questions_tests;

/// BUG-266: classify a headless `claude -p` JSONL log as evidence the
/// upstream Anthropic API took the session out (a transient outage), not
/// that the implementer attempted the work and failed. The orchestrator
/// uses the returned reason to flip phase 1 from *failed* to *Inconclusive*
/// (the BUG-257 path), so a 529 / 5xx / stream-timeout no longer marks a
/// spec as failed-by-implementer.
///
/// Patterns are taken from real Anthropic error text plus the proxy /
/// upstream connectivity families that surface in `claude -p`'s output.
/// Match is anchored and conservative — `Overloaded` and `API Error: 5\d\d`
/// are Anthropic's own wording, `upstream connect error` is the Envoy /
/// proxy convention, `stream timeout` is the SSE-stream variant. Returns
/// the first matching diagnostic line so the orchestrator's epilogue can
/// echo what the substrate said. trace:BUG-266 | ai:claude
pub(crate) fn claude_log_indicates_api_outage(content: &str) -> Option<String> {
    // external-prose-classifier: claude_log_indicates_api_outage
    if content.is_empty() {
        return None;
    }
    for line in content.lines() {
        // The log is JSON stream-output; the human-readable phrase appears
        // verbatim inside an event payload, so substring matching on each
        // line works without parsing JSON. Skip lines that have no error
        // shape — keeps the false-positive surface small.
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        // Anthropic's own 5xx wording — `API Error: 5\d\d` covers 500-599.
        if let Some(idx) = lower.find(aida_core::external_tool_output::CLAUDE_API_5XX_PREFIX) {
            let tail = &lower[idx + aida_core::external_tool_output::CLAUDE_API_5XX_PREFIX.len()..];
            let digits_after = tail.chars().take(2).filter(|c| c.is_ascii_digit()).count();
            if digits_after == 2 {
                return Some(reason_excerpt(trimmed, idx));
            }
        }
        // Anthropic's explicit overload signal — emitted with 529s and the
        // capacity-shed envelope. Conservative: anchor on the full word.
        if let Some(idx) = aida_core::external_tool_output::CLAUDE_API_OUTAGE
            .iter()
            .find_map(|marker| lower.find(marker))
        {
            return Some(reason_excerpt(trimmed, idx));
        }
    }
    None
}

/// Capture a short diagnostic excerpt centered on the matched phrase — the
/// orchestrator's epilogue is one line, so we cap at ~160 chars rather
/// than echoing a full assistant turn. Pure helper for the classifier.
pub(crate) fn reason_excerpt(line: &str, match_start: usize) -> String {
    const MAX: usize = 160;
    if line.len() <= MAX {
        return line.to_string();
    }
    // Anchor the excerpt window around the match so the matched phrase is
    // always visible, then trim to char boundaries to avoid splitting UTF-8.
    let begin = match_start.saturating_sub(20);
    let mut end = (begin + MAX).min(line.len());
    while !line.is_char_boundary(end) && end > begin {
        end -= 1;
    }
    let mut start = begin;
    while !line.is_char_boundary(start) && start < end {
        start += 1;
    }
    format!("…{}…", &line[start..end])
}

/// BUG-266: locate the headless implementer's JSONL log by the session UUID
/// the orchestrator minted (the filename pattern is
/// `<branch>-<session-uuid>.jsonl` under `.aida/headless-logs/`). Glob by
/// suffix because the branch isn't known at the failure point in
/// `run_implementer` (it is discovered AFTER the implementer exits, via the
/// session lease — but a failed-to-spawn or early-crashed implementer may
/// not have a lease yet, so we can't depend on the branch). Returns the log
/// contents on success; `None` if the directory doesn't exist, no matching
/// file is found, or the file can't be read. Best-effort by design —
/// classification is a refinement, not a gate. trace:BUG-266 | ai:claude
pub(crate) fn read_headless_log_for_session(
    project_root: &std::path::Path,
    session_uuid: &str,
) -> Option<String> {
    let path = headless_log_path_for_session(project_root, session_uuid)?;
    std::fs::read_to_string(&path).ok()
}

// trace:STORY-998 | ai:codex
pub(crate) fn headless_log_path_for_session(
    project_root: &std::path::Path,
    session_uuid: &str,
) -> Option<std::path::PathBuf> {
    let dir = project_root.join(".aida").join("headless-logs");
    let suffix = format!("-{session_uuid}.jsonl");
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(&suffix))
            .unwrap_or(false)
        {
            return Some(path);
        }
    }
    None
}

/// BUG-453: byte length of the headless session's JSONL log (the newest file
/// matching `-<session_id>.jsonl` under `.aida/headless-logs/`), or `None`. A
/// *growing* log means the session is alive and emitting events — reading,
/// thinking, or editing — even when it hasn't yet changed a worktree file. The
/// phase watchdog folds this into its progress signal so a productive-but-
/// currently-reading implementer is not false-killed as "no progress" (the
/// TASK-673 symptom: it wrote 503 lines, then spent its final window reading
/// tests in a 20k-line file and the worktree-only signature went static).
/// trace:BUG-453 | ai:claude
pub(crate) fn headless_log_len(project_root: &std::path::Path, session_id: &str) -> Option<u64> {
    let dir = project_root.join(".aida").join("headless-logs");
    let suffix = format!("-{session_id}.jsonl");
    let mut newest_len: Option<u64> = None;
    let mut newest_mtime = std::time::UNIX_EPOCH;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let is_match = entry
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(&suffix))
            .unwrap_or(false);
        if !is_match {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            let m = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            if m >= newest_mtime {
                newest_mtime = m;
                newest_len = Some(meta.len());
            }
        }
    }
    newest_len
}

/// BUG-1716: the two log shapes that witness a headless vendor which emitted
/// nothing. `ZeroBytes` is BUG-826's original signature — the vendor started,
/// created its JSONL log, and died before writing a single stream event.
/// `Missing` is its never-started sibling: the vendor rejected its own launch
/// (an argv/usage error, e.g. the duplicated bypass flag that killed the
/// TASK-204 drive in 2.7s) and exited before even creating the log file. The
/// zero-byte check alone let the `Missing` shape fall through to the
/// work-failure path, where it was classified `NoPr`/`tool-exit`, burned the
/// whole STORY-975 transient budget, and parked a healthy spec
/// `NeedsAttention` behind the live empty lease.
// trace:BUG-1716 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmptyLaunchLog {
    /// No session log file exists at all.
    Missing,
    /// A session log file exists and is zero bytes.
    ZeroBytes,
}

/// BUG-1716: classify the session log's evidence about the launch. `None`
/// means the log has content — the vendor demonstrably ran.
// trace:BUG-1716 | ai:claude
pub(crate) fn headless_launch_log_evidence(
    project_root: &std::path::Path,
    session_id: &str,
) -> Option<EmptyLaunchLog> {
    match headless_log_len(project_root, session_id) {
        None => Some(EmptyLaunchLog::Missing),
        Some(0) => Some(EmptyLaunchLog::ZeroBytes),
        Some(_) => None,
    }
}

/// BUG-1716: is a non-zero headless vendor exit an *empty launch* (the agent
/// never started) rather than implementer work that failed?
///
/// A zero-byte log is taken at face value, exactly as BUG-826 shipped it. A
/// MISSING log claims only the TASK-204 shape — the child DID claim this
/// session's lease (`session_lease` is `Some`), so without this lane the
/// lease discovery below would succeed and the never-started vendor would be
/// classified as implementer work that failed. Two boundaries keep it narrow:
///
/// - `session_lease == None` (no lease for this session) is NOT an empty
///   launch: the child never even claimed, and the BUG-1629/BUG-1769
///   lost-child recovery is the authoritative owner of that shape — it reads
///   the child's own recorded refusal, substrate-verifies a spec that
///   advanced during the run, and spends exactly one pinned replacement
///   launch. Swallowing it here is what broke the whole
///   `bug_1629_phase1_recovery_tests` contract on the first round of this
///   spec (and BUG-1776 is that same misrouting seen from macOS).
/// - `session_lease == Some(true)` (the lease branch carries commits) is NOT
///   an empty launch: log routing could be broken while a real implementer
///   worked, and the failure must keep falling through to the substrate
///   verification (BUG-1140), never into the lease-releasing lane.
// trace:BUG-1716 | ai:claude
pub(crate) fn empty_launch_decision(
    evidence: Option<EmptyLaunchLog>,
    session_lease: Option<bool>,
) -> bool {
    match evidence {
        Some(EmptyLaunchLog::ZeroBytes) => true,
        Some(EmptyLaunchLog::Missing) => session_lease == Some(false),
        None => false,
    }
}

/// BUG-826: bounded launch-retry schedule for empty-log vendor death. Kept
/// separate from the generic transient-work retry: this retry happens before
/// phase 1 returns a failure and before the spec is parked.
// trace:BUG-826 | ai:codex
pub(crate) fn phase1_empty_launch_retry_delay(attempts_used: usize) -> Option<std::time::Duration> {
    [30_u64, 60]
        .get(attempts_used)
        .copied()
        .map(std::time::Duration::from_secs)
}

#[cfg(not(test))]
pub(crate) fn phase1_empty_launch_retry_sleep(delay: std::time::Duration) {
    std::thread::sleep(delay);
}

#[cfg(test)]
pub(crate) fn phase1_empty_launch_retry_sleep(_delay: std::time::Duration) {}

/// TASK-298: scan one headless `claude -p --output-format stream-json` JSONL
/// *line* for a hard "the session bailed at a gate" signal and, when present,
/// return a specific human-readable reason. SPIKE-7 found `claude -p`'s exit
/// code is unreliable (it exits 0 even when it abandoned the work at a
/// permission prompt), so the real completion signal lives in the stream:
///
///   - a non-empty `permission_denials` array → the model wanted a tool the
///     non-interactive run could not grant; in `--no-human` mode there is no
///     human to approve it, so the run is silently stuck. We surface the first
///     denied tool name.
///   - a top-level `is_error: true` (on any event, including the terminal
///     `result`) → the run reported an error envelope; we surface its
///     `result`/`subtype` detail.
///
/// Pure and FULLY ISOLATED: takes a single JSONL line, returns
/// `Some(reason)` when the line carries a stall signal, `None` otherwise.
/// Blank or non-JSON lines are `None` (the orchestrator's log is well-formed
/// JSONL, but a partially-flushed final line must never trip a false stall).
/// The watchdog folds this over the log so a `--no-human` drain wedged at a
/// permission gate is detected and failed instead of hanging until the
/// no-progress / ceiling timeout. trace:TASK-298 | ai:claude
pub(crate) fn headless_line_permission_stall(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;

    // Permission denials are the precise SPIKE-7 signal — surface them first
    // with the offending tool name(s).
    if let Some(denials) = parsed
        .get("permission_denials")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
    {
        let tool = denials
            .iter()
            .find_map(|d| {
                d.get("tool_name")
                    .or_else(|| d.get("tool"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("an unknown tool");
        let extra = if denials.len() > 1 {
            format!(" (+{} more)", denials.len() - 1)
        } else {
            String::new()
        };
        return Some(format!(
            "headless Claude bailed at the permission gate for tool {tool}{extra}"
        ));
    }

    // An error envelope — `is_error: true` on any event (the terminal
    // `result` event is the common carrier, but be liberal). Exit code 0
    // cannot be trusted, so this is the authoritative failure flag.
    if parsed.get("is_error").and_then(|v| v.as_bool()) == Some(true) {
        let detail = parsed
            .get("result")
            .and_then(|v| v.as_str())
            .map(first_nonempty_line)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                parsed
                    .get("subtype")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "error".to_string());
        return Some(format!("headless Claude reported is_error: {detail}"));
    }

    None
}

/// TASK-298: fold [`headless_line_permission_stall`] over a whole headless log
/// body, returning the first line's stall reason or `None`. Best-effort, like
/// the sibling [`claude_log_indicates_api_outage`]: the classification is a
/// refinement that lets the watchdog fail fast, not a gate.
/// trace:TASK-298 | ai:claude
pub(crate) fn headless_log_permission_stall(content: &str) -> Option<String> {
    content.lines().find_map(headless_line_permission_stall)
}

/// BUG-1930: every tool the headless stream reported in a
/// `permission_denials` array, de-duplicated in first-seen order. Empty when
/// the stream recorded no denial (or is not JSONL).
// trace:BUG-1930 | ai:claude
pub(crate) fn headless_log_permission_denied_tools(content: &str) -> Vec<String> {
    let mut tools: Vec<String> = Vec::new();
    for line in content.lines() {
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        let Some(denials) = parsed.get("permission_denials").and_then(|v| v.as_array()) else {
            continue;
        };
        for denial in denials {
            let tool = denial
                .get("tool_name")
                .or_else(|| denial.get("tool"))
                .and_then(|v| v.as_str())
                .unwrap_or("an unknown tool");
            if !tools.iter().any(|t| t == tool) {
                tools.push(tool.to_string());
            }
        }
    }
    tools
}

/// First non-empty trimmed line of a string, or the empty string. Small pure
/// helper so a multi-line `result` detail collapses to one log-friendly row.
/// trace:TASK-298 | ai:claude
pub(crate) fn first_nonempty_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

pub(crate) struct TextQuestionPunt {
    pub(crate) detail: String,
    pub(crate) lean: String,
}

/// BUG-354 / BUG-374: classify the headless implementer's final `result` text
/// as the plain-markdown variant of AskUserQuestion: the model asks the
/// operator to choose/confirm a path, calls no tool, exits success, and opens
/// no PR. We only scan the terminal result text and require both a question
/// mark and decision-fork phrasing so normal implementation summaries that
/// mention questions in passing keep the existing NoPr failure path.
pub(crate) fn pending_text_question_from_headless_log(content: &str) -> Option<TextQuestionPunt> {
    let result = reviewer_summary::parse_result_event(content)?;
    if result.is_error {
        return None;
    }
    let text = result.result_text?;
    pending_text_question_from_result_text(&text)
}

pub(crate) fn pending_text_question_from_result_text(text: &str) -> Option<TextQuestionPunt> {
    let trimmed = text.trim();
    if !trimmed.contains('?') {
        return None;
    }
    // BUG-462: scan EVERY question sentence (not just the first `?`) for
    // decision-fork phrasing. A confident headless implementer phrases its
    // "which path?" infinitely many ways and routinely buries the operative
    // question after an options block, so the first `?` in the text is often a
    // rhetorical aside. An exact-phrase allowlist checked against only the first
    // question is the wrong shape — it let "Which way do you want to go?" (the
    // TASK-457 drain) fall through to a misleading phase-1 NoPr failure instead
    // of advisor-tier routing. trace:BUG-462 | ai:claude
    let questions = all_question_sentences(trimmed);
    let question = questions
        .iter()
        .find(|q| question_has_fork_marker(&q.to_ascii_lowercase()))
        .cloned()
        // Options-block fallback: even when no phrase marker matches, an
        // enumerated multi-option block (`A)`/`B)`, `1.`/`2.`, `Option A/B`)
        // paired with a trailing question is an unambiguous decision fork. Bias
        // toward punt here: a spec parked and routed to the headless advisor
        // beats a hard phase-1 NoPr failure with a misleading `/aida-pr` hint —
        // "a paused spec beats a guessed one". trace:BUG-462 | ai:claude
        .or_else(|| {
            if has_options_block(trimmed) {
                questions.last().cloned()
            } else {
                None
            }
        })?;
    let recommendation = recommendation_line(trimmed).unwrap_or_else(|| {
        "review the punted analysis and choose the implementation path".to_string()
    });
    Some(TextQuestionPunt {
        detail: format!(
            "headless implementer exited without opening a PR after asking a plain-text decision question: {}",
            bounded_text(&question, 220),
        ),
        lean: bounded_text(&recommendation, 220),
    })
}

/// Decision-fork phrasing markers for a single (already lowercased) question
/// sentence. Broadened under BUG-462 beyond the original BUG-354/BUG-374 set so
/// novel phrasings ("which way", "do you want", "would you like") still route to
/// the advisor tier. Kept narrow enough that an ordinary summary question
/// ("does the parser handle aliases?") does not match. trace:BUG-462 | ai:claude
pub(crate) fn question_has_fork_marker(question_lower: &str) -> bool {
    const QUESTION_FORK_MARKERS: &[&str] = &[
        "which path",
        "which option",
        "which approach",
        "which one",
        "which way",
        "which direction",
        "do you want",
        "would you like",
        "want me to",
        "should i",
        "should we",
        "how would you like",
        "how should",
        "need you to choose",
        "go with",
        "do you prefer",
        "your call",
        "please confirm",
        "confirm and i'll proceed",
        "confirm and i’ll proceed",
        "confirm and i will proceed",
        "confirm this path",
        "confirm the path",
        "confirm option",
        "confirm approach",
    ];
    QUESTION_FORK_MARKERS
        .iter()
        .any(|marker| question_lower.contains(marker))
}

/// Every `?`-terminated sentence in `text`, whitespace-collapsed. Splits on
/// `.`/`!`/`?` so a fork question buried after rhetorical asides is still
/// surfaced (BUG-462 scans all of them, not just the first). trace:BUG-462
pub(crate) fn all_question_sentences(text: &str) -> Vec<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = Vec::new();
    let mut start = 0usize;
    // `?`/`.`/`!` are ASCII, so every index used here is a char boundary.
    for (i, b) in collapsed.bytes().enumerate() {
        match b {
            b'?' => {
                let sentence = collapsed[start..=i]
                    .trim_start_matches(['.', '!', '?', ' '])
                    .trim();
                if !sentence.is_empty() {
                    out.push(sentence.to_string());
                }
                start = i + 1;
            }
            b'.' | b'!' => start = i + 1,
            _ => {}
        }
    }
    out
}

/// True when `text` presents an enumerated *choice* — two or more lines that
/// start with a letter-labelled option (`Option A`, `A)`, `B:`). Deliberately
/// LETTER-only (not `1.`/`2.`): digit labels far more often enumerate completed
/// *steps* in a summary than mutually-exclusive choices, and the fallback fires
/// only when no phrase marker matched, so a numbered accomplishments list with a
/// trailing rhetorical question must not read as a fork. Line-anchored so inline
/// version strings like `v1.2.0` don't count. trace:BUG-462 | ai:claude
pub(crate) fn has_options_block(text: &str) -> bool {
    let mut enumerated = 0usize;
    for raw in text.lines() {
        // strip a leading markdown bold/bullet so "- **A)** ..." still counts
        let line = raw
            .trim_start()
            .trim_start_matches(['-', '*', ' '])
            .trim_start_matches("**");
        if line.to_ascii_lowercase().starts_with("option ") || starts_with_letter_label(line) {
            enumerated += 1;
        }
    }
    enumerated >= 2
}

/// A single letter (`A`–`H`, any case) followed by `)`, `.`, or `:` — the start
/// of a letter-labelled option line. trace:BUG-462 | ai:claude
pub(crate) fn starts_with_letter_label(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let Some(sep) = chars.next() else {
        return false;
    };
    matches!(first, 'A'..='H' | 'a'..='h') && matches!(sep, ')' | '.' | ':')
}

pub(crate) fn recommendation_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("recommend") || lower.starts_with("lean:") || lower.contains("my lean")
        })
        .map(str::to_string)
}

pub(crate) fn bounded_text(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() <= max {
        return text;
    }
    let mut end = max.saturating_sub(1).min(text.len());
    while !text.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
