//! `aida spec interview <SPEC> [--apply] [--json]` — the L1 intent-quality
//! resolution loop that closes the gap `aida spec dryrun` (STORY-656) opens.
//!
//! EPIC-48 L1. Where `dryrun` *surfaces* a spec's readiness gaps (missing
//! acceptance, a stub description, no parent, a too-vague body), interview
//! *resolves* them: it turns each failing dimension into a clarifying QUESTION,
//! collects the human's answers, and FOLDS those answers back into the spec —
//! into binding `## Acceptance` criteria and structured fields (e.g. a parent
//! link), never into comments (refinements must be binding, per the memory
//! discipline). Re-running dryrun afterward shows the readiness score improved.
//!
//! Two layers, mirroring `dryrun.rs`:
//!
//! 1. A DETERMINISTIC core (this module) — pure functions, fully unit-tested,
//!    no I/O / no LLM / no clock-as-input:
//!    - [`questions_for`] maps a [`dryrun::Readiness`] (plus the optional `--ai`
//!      gap report) onto an ordered list of [`InterviewQuestion`]s. A gap maps
//!      to exactly the right question; a passing dimension produces none.
//!    - [`apply_answers`] folds a set of [`Answer`]s into a spec: acceptance
//!      answers append to `## Acceptance` (reusing the same section-aware writer
//!      the `aida questions answer` path uses), a parent answer is parsed into a
//!      target spec-id for the caller to link, a priority answer parses into a
//!      [`Priority`]. It returns a [`SpecEdit`] describing exactly what changed,
//!      so the `--apply` vs propose split is a property of the data, not a side
//!      effect.
//! 2. The INTEGRATION boundary (store load, TTY prompting, the optional headless
//!    `claude -p` for AI questions, the actual write) lives in `main.rs`,
//!    exactly like `dryrun.rs` pairs with `handle_spec_dryrun`. Tests never
//!    reach that boundary.
//!
//! **Interactive vs headless.** With a TTY, the integration layer prompts for
//! each question in turn and applies the answers. Without a TTY (the advisor /
//! agent seat), it emits the structured question list as JSON and exits WITHOUT
//! blocking on stdin — an agent answers and feeds the answers back via
//! `--answers <file>`. **`--apply` vs default.** The default is non-destructive:
//! it PROPOSES the edits (prints the diff / emits the question list). `--apply`
//! is the only path that writes the spec — folding answers and re-scoring. This
//! keeps an interview from mutating a spec by surprise.
//!
//! trace:STORY-657 | ai:claude

use crate::dryrun::{AiReport, Readiness};
use aida_core::RequirementPriority as Priority;
use serde::{Deserialize, Serialize};

/// What kind of spec field an answer resolves — so the application step knows
/// whether to fold the answer into `## Acceptance`, set the parent, or set the
/// priority. Stable machine tokens (used as the JSON `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// The answer becomes an acceptance criterion (the `description`,
    /// `acceptance`, and `not_vague` dryrun dimensions all resolve here — they
    /// are all "the spec body doesn't say enough about what done looks like").
    Acceptance,
    /// The answer names a parent spec-id to link.
    Parent,
    /// The answer names a priority (`high` / `medium` / `low`).
    Priority,
}

/// One clarifying question posed to the human, derived from a failing readiness
/// dimension (or an `--ai` ambiguity). Carries the originating dimension name so
/// the caller can pair an answer back to its gap deterministically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterviewQuestion {
    /// The dryrun dimension this question resolves (`"acceptance"`, `"parent"`,
    /// `"priority"`, `"description"`, `"not_vague"`). Stable; used as the answer
    /// key.
    pub dimension: String,
    /// What kind of spec edit the answer drives.
    pub kind: QuestionKind,
    /// The clarifying prompt shown to the human.
    pub prompt: String,
}

/// A human's answer to one [`InterviewQuestion`], keyed by the question's
/// `dimension`. The integration layer collects these (from TTY prompts or a
/// `--answers` JSON file) and hands them to [`apply_answers`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    /// The `dimension` of the question being answered.
    pub dimension: String,
    /// The human's free-text answer.
    pub answer: String,
}

/// The concrete edit [`apply_answers`] computed from a spec + a set of answers.
/// The integration layer turns this into store writes. Expressing the result as
/// data (not in-place mutation) is what makes the propose-vs-apply split clean:
/// the same computation feeds both the dry-run preview and the `--apply` write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecEdit {
    /// The new description body (with any acceptance answers folded in). Equal
    /// to the input description when no acceptance/body answer was given.
    pub new_description: String,
    /// A parent spec-id to link, if a parent answer named one (already trimmed
    /// and uppercased). `None` when no parent answer was given (or it was blank).
    pub parent_spec_id: Option<String>,
    /// A priority to set, if a priority answer parsed to one. `None` when no
    /// priority answer was given (or it didn't parse).
    pub priority: Option<Priority>,
    /// Human-readable one-liners describing each applied change, for the
    /// propose/apply output. Stable order: acceptance lines, then parent, then
    /// priority.
    pub changes: Vec<String>,
}

impl SpecEdit {
    /// Whether this edit changes anything at all.
    pub fn is_noop(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Map a readiness verdict (and optional AI gap report) onto the ordered list of
/// clarifying questions an interview should ask.
///
/// Pure: same inputs → same questions, in a stable order. Only FAILING
/// dimensions yield questions (a spec already at 100 yields none). The
/// dimension→question mapping:
///
/// - `description` / `acceptance` / `not_vague` → an [`Acceptance`] question
///   (they all mean "the body doesn't pin down what done looks like"). We ask
///   ONE acceptance question per failing dimension so the human's answers
///   accumulate as distinct criteria, but they all fold into `## Acceptance`.
/// - `parent` → a [`Parent`] question.
/// - `priority` → a [`Priority`] question.
/// - `type` is intentionally NOT asked: changing a folder/meta to an
///   implementable type is a re-classification, not a clarification — out of
///   scope for an interview (the caller should `aida edit --type` deliberately).
///
/// When an [`AiReport`] is supplied, its `ambiguities` + `questions` are
/// appended as additional acceptance-shaped questions, so the `--ai` dryrun gap
/// report drives the interview too.
///
/// [`Acceptance`]: QuestionKind::Acceptance
/// [`Parent`]: QuestionKind::Parent
/// [`Priority`]: QuestionKind::Priority
pub fn questions_for(readiness: &Readiness, ai: Option<&AiReport>) -> Vec<InterviewQuestion> {
    let mut out = Vec::new();
    for d in readiness.dimensions.iter().filter(|d| !d.pass) {
        match d.name {
            // trace:TASK-849 | ai:claude — dryrun dimension names are `&'static str`.
            "description" => out.push(InterviewQuestion {
                dimension: "description".to_string(),
                kind: QuestionKind::Acceptance,
                prompt: "The description is too thin to start from. In a sentence or two, what is \
                         the concrete behavior or outcome this spec must deliver?"
                    .to_string(),
            }),
            "acceptance" => out.push(InterviewQuestion {
                dimension: "acceptance".to_string(),
                kind: QuestionKind::Acceptance,
                prompt: "No acceptance criteria found — what does 'done' look like? Give one \
                         concrete, checkable criterion."
                    .to_string(),
            }),
            "not_vague" => out.push(InterviewQuestion {
                dimension: "not_vague".to_string(),
                kind: QuestionKind::Acceptance,
                prompt: "The description reads as vague/placeholder. Name a specific input, \
                         output, or edge case the implementer must handle."
                    .to_string(),
            }),
            "parent" => out.push(InterviewQuestion {
                dimension: "parent".to_string(),
                kind: QuestionKind::Parent,
                prompt: "This spec has no parent. Which spec-id (e.g. EPIC-48, STORY-12) is it a \
                         child of? (Enter a spec-id, or leave blank to skip.)"
                    .to_string(),
            }),
            "priority" => out.push(InterviewQuestion {
                dimension: "priority".to_string(),
                kind: QuestionKind::Priority,
                prompt: "No priority set. What priority is this — high, medium, or low?"
                    .to_string(),
            }),
            // `type` is a re-classification, not a clarification — out of scope.
            _ => {}
        }
    }

    if let Some(ai) = ai {
        for (i, ambiguity) in ai.ambiguities.iter().enumerate() {
            out.push(InterviewQuestion {
                dimension: format!("ai_ambiguity_{i}"),
                kind: QuestionKind::Acceptance,
                prompt: format!(
                    "AI flagged an ambiguity — resolve it into a criterion: {ambiguity}"
                ),
            });
        }
        for (i, question) in ai.questions.iter().enumerate() {
            out.push(InterviewQuestion {
                dimension: format!("ai_question_{i}"),
                kind: QuestionKind::Acceptance,
                prompt: format!("AI question an implementer would ask — answer it: {question}"),
            });
        }
    }

    out
}

/// Fold a set of answers into a spec, producing the concrete [`SpecEdit`].
///
/// Pure: takes the current description + the questions that were asked + the
/// answers, returns the edit. No I/O. The questions are passed (not just the
/// answers) so the kind/dimension pairing is authoritative — an answer is only
/// applied if it matches a question that was actually asked.
///
/// Application rules:
/// - An [`Acceptance`] answer with non-blank text appends a dated, attributed
///   acceptance bullet via [`append_to_acceptance`] (creating the section if
///   absent), so multiple answers accumulate as distinct criteria.
/// - A [`Parent`] answer with non-blank text yields a trimmed, uppercased
///   `parent_spec_id` for the caller to link (a self-reference is rejected by
///   the caller, which knows the spec's own id).
/// - A [`Priority`] answer is parsed case-insensitively; an unparseable value is
///   silently ignored (no `priority` change) rather than erroring, so a typo'd
///   priority answer doesn't abort the whole interview.
/// - A blank answer to any question is a skip (no change for that gap).
///
/// `date` is passed in (not read from the clock) so the acceptance lines are
/// deterministic and the function stays pure/testable.
///
/// [`Acceptance`]: QuestionKind::Acceptance
/// [`Parent`]: QuestionKind::Parent
/// [`Priority`]: QuestionKind::Priority
pub fn apply_answers(
    description: &str,
    questions: &[InterviewQuestion],
    answers: &[Answer],
    date: &str,
) -> SpecEdit {
    let mut new_description = description.to_string();
    let mut parent_spec_id = None;
    let mut priority = None;
    let mut changes = Vec::new();

    // Acceptance answers first (stable order: by question order).
    for q in questions
        .iter()
        .filter(|q| q.kind == QuestionKind::Acceptance)
    {
        if let Some(ans) = answer_for(answers, &q.dimension) {
            let line = acceptance_line(ans, date);
            new_description = append_to_acceptance(&new_description, &line);
            changes.push(format!("+ acceptance: {}", truncate(ans, 72)));
        }
    }

    // Parent.
    if let Some(q) = questions.iter().find(|q| q.kind == QuestionKind::Parent) {
        if let Some(ans) = answer_for(answers, &q.dimension) {
            let id = ans.trim().to_ascii_uppercase();
            if !id.is_empty() {
                changes.push(format!("-> parent: {id}"));
                parent_spec_id = Some(id);
            }
        }
    }

    // Priority.
    if let Some(q) = questions.iter().find(|q| q.kind == QuestionKind::Priority) {
        if let Some(ans) = answer_for(answers, &q.dimension) {
            if let Some(p) = parse_priority(ans) {
                changes.push(format!("priority: {}", priority_token(&p)));
                priority = Some(p);
            }
        }
    }

    SpecEdit {
        new_description,
        parent_spec_id,
        priority,
        changes,
    }
}

/// Find the non-blank answer text for a dimension, if any. Trims; treats a
/// whitespace-only answer as absent (a skip).
fn answer_for<'a>(answers: &'a [Answer], dimension: &str) -> Option<&'a str> {
    answers
        .iter()
        .find(|a| a.dimension == dimension)
        .map(|a| a.answer.trim())
        .filter(|s| !s.is_empty())
}

/// Build the dated, attributed acceptance bullet for a folded interview answer.
/// Mirrors `resolved_acceptance_line` in `main.rs` (the `aida questions answer`
/// path) so an interview-folded criterion reads the same as a decision-folded
/// one.
fn acceptance_line(answer: &str, date: &str) -> String {
    format!("- From interview ({date}): {}", answer.trim())
}

/// Parse a free-text priority answer into a [`Priority`]. Case-insensitive;
/// accepts the canonical tokens plus the common one-letter shorthands. Returns
/// `None` for anything else (the caller treats `None` as "leave priority
/// alone").
fn parse_priority(answer: &str) -> Option<Priority> {
    match answer.trim().to_ascii_lowercase().as_str() {
        "high" | "h" => Some(Priority::High),
        "medium" | "med" | "m" => Some(Priority::Medium),
        "low" | "l" => Some(Priority::Low),
        _ => None,
    }
}

/// The stable lowercase token for a priority (for change descriptions).
fn priority_token(p: &Priority) -> &'static str {
    match p {
        Priority::High => "high",
        Priority::Medium => "medium",
        Priority::Low => "low",
    }
}

/// Truncate a string to at most `max` chars, appending an ellipsis when cut.
fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}...")
}

/// Append `line` to a description body's `## Acceptance` section, creating the
/// section if absent. Section-aware: inserts after the section's last non-blank
/// content line (before the next heading) so the new criterion reads as the
/// newest acceptance item.
///
/// This duplicates the section-walk logic of
/// `main.rs::append_resolved_to_acceptance` deliberately, to keep this module
/// dependency-light and fully unit-testable without pulling in `main.rs`. The
/// two are exercised against the same shapes. trace:STORY-657 | ai:claude
fn append_to_acceptance(description: &str, line: &str) -> String {
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
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str("## Acceptance\n\n");
            out.push_str(line);
            out.push('\n');
            out
        }
    }
}

/// The `--json` payload for `aida spec interview` (the non-TTY / propose path):
/// the spec, the readiness score it scored at, and the ordered questions to
/// answer. An agent answers these and feeds them back via `--answers <file>` +
/// `--apply`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterviewJson<'a> {
    pub spec: &'a str,
    pub readiness_score: u32,
    pub questions: &'a [InterviewQuestion],
}

/// Assemble the question-list `--json` payload.
pub fn interview_json<'a>(
    spec: &'a str,
    readiness: &'a Readiness,
    questions: &'a [InterviewQuestion],
) -> InterviewJson<'a> {
    InterviewJson {
        spec,
        readiness_score: readiness.score,
        questions,
    }
}

/// Parse a `--answers <file>` payload: a JSON array of `{dimension, answer}`
/// objects, or an object `{ "answers": [ ... ] }`. Pure + unit-tested so the
/// agent-feedback contract is locked without any I/O.
pub fn parse_answers(raw: &str) -> anyhow::Result<Vec<Answer>> {
    let raw = raw.trim();
    // Accept either a bare array or a wrapper object with an `answers` key.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Shape {
        Array(Vec<Answer>),
        Wrapped { answers: Vec<Answer> },
    }
    let shape: Shape = serde_json::from_str(raw)
        .map_err(|e| anyhow::anyhow!("--answers payload was not valid JSON: {e}"))?;
    Ok(match shape {
        Shape::Array(v) => v,
        Shape::Wrapped { answers } => answers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dryrun::{score, SpecSnapshot};

    /// A snapshot that fails everything an interview can ask about: no
    /// acceptance, thin description, no parent, no priority.
    fn gappy_snapshot() -> SpecSnapshot {
        SpecSnapshot {
            description: "Fix it.".to_string(),
            req_type: "story".to_string(),
            has_priority: false,
            has_parent: false,
            is_top_level: false,
        }
    }

    fn ready_snapshot() -> SpecSnapshot {
        SpecSnapshot {
            description: "Add a readiness pre-check command that scores a spec across several \
                          dimensions before it is queued for implementation.\n\n\
                          ## Acceptance\n\
                          - Deterministic scorer returns a 0-100 score"
                .to_string(),
            req_type: "story".to_string(),
            has_priority: true,
            has_parent: true,
            is_top_level: false,
        }
    }

    #[test]
    fn ready_spec_yields_no_questions() {
        let r = score(&ready_snapshot());
        assert_eq!(r.score, 100);
        assert!(questions_for(&r, None).is_empty());
    }

    #[test]
    fn gappy_spec_yields_questions_for_each_gap() {
        let r = score(&gappy_snapshot());
        let qs = questions_for(&r, None);
        let dims: Vec<&str> = qs.iter().map(|q| q.dimension.as_str()).collect();
        // description, acceptance, not_vague, priority, parent all failed.
        assert!(dims.contains(&"description"));
        assert!(dims.contains(&"acceptance"));
        assert!(dims.contains(&"not_vague"));
        assert!(dims.contains(&"priority"));
        assert!(dims.contains(&"parent"));
        // type is NOT asked even when it fails.
        assert!(!dims.contains(&"type"));
    }

    #[test]
    fn type_gap_is_never_asked() {
        let mut s = ready_snapshot();
        s.req_type = "folder".to_string();
        let r = score(&s);
        // type fails, but an interview does not ask about re-classification.
        let qs = questions_for(&r, None);
        assert!(qs.iter().all(|q| q.dimension != "type"));
    }

    #[test]
    fn missing_acceptance_maps_to_acceptance_kind() {
        let mut s = ready_snapshot();
        s.description = "A reasonably long and concrete description of work to do, but with no \
                         checkable criteria at all in the body text here right now."
            .to_string();
        let r = score(&s);
        let qs = questions_for(&r, None);
        let q = qs
            .iter()
            .find(|q| q.dimension == "acceptance")
            .expect("acceptance question present");
        assert_eq!(q.kind, QuestionKind::Acceptance);
        assert!(q.prompt.to_lowercase().contains("done"));
    }

    #[test]
    fn parent_gap_maps_to_parent_kind() {
        let mut s = ready_snapshot();
        s.has_parent = false;
        s.is_top_level = false;
        let r = score(&s);
        let q = questions_for(&r, None)
            .into_iter()
            .find(|q| q.dimension == "parent")
            .expect("parent question present");
        assert_eq!(q.kind, QuestionKind::Parent);
    }

    #[test]
    fn priority_gap_maps_to_priority_kind() {
        let mut s = ready_snapshot();
        s.has_priority = false;
        let r = score(&s);
        let q = questions_for(&r, None)
            .into_iter()
            .find(|q| q.dimension == "priority")
            .expect("priority question present");
        assert_eq!(q.kind, QuestionKind::Priority);
    }

    #[test]
    fn ai_ambiguities_and_questions_become_acceptance_questions() {
        let r = score(&ready_snapshot()); // no deterministic gaps
        let ai = AiReport {
            questions: vec!["Sync or async?".to_string()],
            assumptions: vec![],
            ambiguities: vec!["No error-path acceptance".to_string()],
            model: "m".to_string(),
        };
        let qs = questions_for(&r, Some(&ai));
        assert_eq!(qs.len(), 2);
        assert!(qs.iter().all(|q| q.kind == QuestionKind::Acceptance));
        assert!(qs.iter().any(|q| q.prompt.contains("No error-path")));
        assert!(qs.iter().any(|q| q.prompt.contains("Sync or async")));
    }

    #[test]
    fn questions_are_deterministic() {
        let r = score(&gappy_snapshot());
        assert_eq!(questions_for(&r, None), questions_for(&r, None));
    }

    #[test]
    fn apply_acceptance_answer_creates_section_and_folds_in() {
        let qs = vec![InterviewQuestion {
            dimension: "acceptance".to_string(),
            kind: QuestionKind::Acceptance,
            prompt: "what does done look like?".to_string(),
        }];
        let answers = vec![Answer {
            dimension: "acceptance".to_string(),
            answer: "The command exits 0 and prints the new score".to_string(),
        }];
        let edit = apply_answers("A bare description.", &qs, &answers, "2026-06-18");
        assert!(edit.new_description.contains("## Acceptance"));
        assert!(edit
            .new_description
            .contains("From interview (2026-06-18): The command exits 0"));
        assert_eq!(edit.changes.len(), 1);
        assert!(!edit.is_noop());
    }

    #[test]
    fn apply_acceptance_answer_appends_to_existing_section() {
        let desc = "Body.\n\n## Acceptance\n- existing criterion\n\n## Notes\nstuff";
        let qs = vec![InterviewQuestion {
            dimension: "acceptance".to_string(),
            kind: QuestionKind::Acceptance,
            prompt: "x".to_string(),
        }];
        let answers = vec![Answer {
            dimension: "acceptance".to_string(),
            answer: "second criterion".to_string(),
        }];
        let edit = apply_answers(desc, &qs, &answers, "2026-06-18");
        // The new line lands inside the Acceptance section, before ## Notes, and
        // after the existing criterion.
        let body = &edit.new_description;
        let existing = body.find("existing criterion").unwrap();
        let added = body.find("second criterion").unwrap();
        let notes = body.find("## Notes").unwrap();
        assert!(existing < added, "new line after existing criterion");
        assert!(
            added < notes,
            "new line stays inside the Acceptance section"
        );
    }

    #[test]
    fn multiple_acceptance_answers_all_fold_in() {
        let qs = vec![
            InterviewQuestion {
                dimension: "description".to_string(),
                kind: QuestionKind::Acceptance,
                prompt: "a".to_string(),
            },
            InterviewQuestion {
                dimension: "acceptance".to_string(),
                kind: QuestionKind::Acceptance,
                prompt: "b".to_string(),
            },
        ];
        let answers = vec![
            Answer {
                dimension: "description".to_string(),
                answer: "behavior one".to_string(),
            },
            Answer {
                dimension: "acceptance".to_string(),
                answer: "criterion two".to_string(),
            },
        ];
        let edit = apply_answers("desc", &qs, &answers, "2026-06-18");
        assert!(edit.new_description.contains("behavior one"));
        assert!(edit.new_description.contains("criterion two"));
        assert_eq!(edit.changes.len(), 2);
    }

    #[test]
    fn apply_parent_answer_sets_parent_uppercased() {
        let qs = vec![InterviewQuestion {
            dimension: "parent".to_string(),
            kind: QuestionKind::Parent,
            prompt: "which parent?".to_string(),
        }];
        let answers = vec![Answer {
            dimension: "parent".to_string(),
            answer: "  epic-48  ".to_string(),
        }];
        let edit = apply_answers("desc", &qs, &answers, "2026-06-18");
        assert_eq!(edit.parent_spec_id.as_deref(), Some("EPIC-48"));
        assert!(edit.changes.iter().any(|c| c.contains("EPIC-48")));
        // No acceptance change from a parent-only answer.
        assert_eq!(edit.new_description, "desc");
    }

    #[test]
    fn apply_priority_answer_parses() {
        for (input, want) in [
            ("high", Priority::High),
            ("Medium", Priority::Medium),
            ("LOW", Priority::Low),
            ("h", Priority::High),
            ("med", Priority::Medium),
        ] {
            let qs = vec![InterviewQuestion {
                dimension: "priority".to_string(),
                kind: QuestionKind::Priority,
                prompt: "p?".to_string(),
            }];
            let answers = vec![Answer {
                dimension: "priority".to_string(),
                answer: input.to_string(),
            }];
            let edit = apply_answers("desc", &qs, &answers, "2026-06-18");
            assert_eq!(edit.priority, Some(want), "input {input:?}");
        }
    }

    #[test]
    fn unparseable_priority_is_ignored_not_error() {
        let qs = vec![InterviewQuestion {
            dimension: "priority".to_string(),
            kind: QuestionKind::Priority,
            prompt: "p?".to_string(),
        }];
        let answers = vec![Answer {
            dimension: "priority".to_string(),
            answer: "urgent-ish".to_string(),
        }];
        let edit = apply_answers("desc", &qs, &answers, "2026-06-18");
        assert_eq!(edit.priority, None);
        assert!(edit.is_noop());
    }

    #[test]
    fn blank_answers_are_skips() {
        let qs = vec![
            InterviewQuestion {
                dimension: "acceptance".to_string(),
                kind: QuestionKind::Acceptance,
                prompt: "a".to_string(),
            },
            InterviewQuestion {
                dimension: "parent".to_string(),
                kind: QuestionKind::Parent,
                prompt: "p".to_string(),
            },
        ];
        let answers = vec![
            Answer {
                dimension: "acceptance".to_string(),
                answer: "   ".to_string(),
            },
            Answer {
                dimension: "parent".to_string(),
                answer: "".to_string(),
            },
        ];
        let edit = apply_answers("desc", &qs, &answers, "2026-06-18");
        assert!(edit.is_noop(), "blank answers change nothing");
        assert_eq!(edit.new_description, "desc");
        assert!(edit.parent_spec_id.is_none());
    }

    #[test]
    fn answer_without_matching_question_is_ignored() {
        let qs = vec![InterviewQuestion {
            dimension: "acceptance".to_string(),
            kind: QuestionKind::Acceptance,
            prompt: "a".to_string(),
        }];
        // Answer keyed to a dimension that was never asked.
        let answers = vec![Answer {
            dimension: "priority".to_string(),
            answer: "high".to_string(),
        }];
        let edit = apply_answers("desc", &qs, &answers, "2026-06-18");
        assert!(edit.is_noop());
        assert!(edit.priority.is_none());
    }

    #[test]
    fn full_round_trip_raises_readiness() {
        // Start gappy, gather answers for every question, apply, re-score the
        // resulting body — the score must strictly improve.
        let before = score(&gappy_snapshot());
        let qs = questions_for(&before, None);
        let answers: Vec<Answer> = qs
            .iter()
            .map(|q| Answer {
                dimension: q.dimension.clone(),
                answer: match q.kind {
                    QuestionKind::Acceptance => {
                        "The implementer must validate input and return a typed error on failure"
                            .to_string()
                    }
                    QuestionKind::Parent => "EPIC-48".to_string(),
                    QuestionKind::Priority => "high".to_string(),
                },
            })
            .collect();
        let edit = apply_answers(&gappy_snapshot().description, &qs, &answers, "2026-06-18");

        // Re-score the new body with parent/priority now satisfied.
        let after = score(&SpecSnapshot {
            description: edit.new_description.clone(),
            req_type: "story".to_string(),
            has_priority: edit.priority.is_some(),
            has_parent: edit.parent_spec_id.is_some(),
            is_top_level: false,
        });
        assert!(
            after.score > before.score,
            "interview should raise readiness: {} -> {}",
            before.score,
            after.score
        );
        assert!(after.score >= 80, "answered spec should be near-ready");
        assert_eq!(edit.parent_spec_id.as_deref(), Some("EPIC-48"));
        assert_eq!(edit.priority, Some(Priority::High));
    }

    #[test]
    fn parse_answers_accepts_bare_array() {
        let raw = r#"[{"dimension":"acceptance","answer":"done = merged"}]"#;
        let v = parse_answers(raw).expect("parses");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].dimension, "acceptance");
        assert_eq!(v[0].answer, "done = merged");
    }

    #[test]
    fn parse_answers_accepts_wrapped_object() {
        let raw = r#"{"answers":[{"dimension":"parent","answer":"EPIC-48"}]}"#;
        let v = parse_answers(raw).expect("parses");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].dimension, "parent");
    }

    #[test]
    fn parse_answers_rejects_garbage() {
        assert!(parse_answers("not json").is_err());
    }

    #[test]
    fn interview_json_shape() {
        let r = score(&gappy_snapshot());
        let qs = questions_for(&r, None);
        let payload = interview_json("STORY-657", &r, &qs);
        let v = serde_json::to_value(&payload).unwrap();
        assert_eq!(v["spec"], "STORY-657");
        assert!(v["readiness_score"].is_number());
        assert!(v["questions"].is_array());
        assert!(!v["questions"].as_array().unwrap().is_empty());
        // Each question carries dimension/kind/prompt.
        let q0 = &v["questions"][0];
        assert!(q0["dimension"].is_string());
        assert!(q0["kind"].is_string());
        assert!(q0["prompt"].is_string());
    }

    // End-to-end smoke test for the `aida spec interview --apply` WRITE path.
    //
    // The unit tests above lock the pure core (questions_for / apply_answers).
    // This one closes the loop the integration boundary (handle_spec_interview)
    // actually runs: fold answers, write the resolved spec back to a real
    // git-canonical store, RELOAD it, and assert against the persisted spec.
    // It mirrors handle_spec_interview's --apply branch without reaching into
    // main.rs (private fn), exercising the same DatabaseBackend write.
    //
    // Three assertions, one test (TASK-854):
    //   1. --apply WRITES the spec — the answers land in the persisted body.
    //   2. the spec re-scores HIGHER after applying.
    //   3. a BLANK answer is SKIPPED — it does not overwrite/clobber.
    #[test]
    fn apply_write_path_persists_rescores_and_skips_blanks() {
        use aida_core::{DatabaseBackend, GitBackend, Requirement, RequirementType};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Seed a gappy story spec: thin body, no acceptance, no parent. It maps
        // to acceptance/description/not_vague/parent/priority questions.
        let mut req = Requirement::new("interview apply smoke".into(), "Fix it.".into());
        req.spec_id = Some("TASK-854".into());
        req.req_type = RequirementType::Task;
        let added = backend.add_requirement(req).unwrap();

        // Score BEFORE (the same scorer handle_spec_interview runs).
        let before = score(&SpecSnapshot::from_requirement(&added));

        // Derive the questions, then answer ONLY the acceptance/parent gaps;
        // leave the `parent` answer BLANK to prove blank == skip end to end.
        let questions = questions_for(&before, None);
        let answers: Vec<Answer> = questions
            .iter()
            .map(|q| Answer {
                dimension: q.dimension.clone(),
                answer: match q.kind {
                    QuestionKind::Acceptance => {
                        "The command exits 0 and the written body re-scores higher".to_string()
                    }
                    // BLANK parent answer — must be skipped, not written.
                    QuestionKind::Parent => "   ".to_string(),
                    QuestionKind::Priority => "high".to_string(),
                },
            })
            .collect();

        let edit = apply_answers(&added.description, &questions, &answers, "2026-06-18");
        // The blank parent answer folded into no parent link (skip, not write).
        assert!(
            edit.parent_spec_id.is_none(),
            "blank parent answer must be skipped, not written"
        );

        // Mirror handle_spec_interview's --apply write: fold body + priority,
        // then persist via the backend.
        let mut to_save = added.clone();
        to_save.description = edit.new_description.clone();
        if let Some(p) = &edit.priority {
            to_save.priority = p.clone();
        }
        backend.update_requirement(&to_save).unwrap();

        // (1) Reload from the store and confirm the answer text PERSISTED.
        let reloaded = backend
            .get_requirement_by_spec_id("TASK-854")
            .unwrap()
            .expect("spec persisted");
        assert!(
            reloaded.description.contains("re-scores higher"),
            "interview answer must be written into the persisted spec body"
        );
        assert!(
            reloaded.description.contains("## Acceptance"),
            "the acceptance section must be present in the persisted body"
        );

        // (2) Re-score the RELOADED spec — readiness must strictly improve.
        let after = score(&SpecSnapshot::from_requirement(&reloaded));
        assert!(
            after.score > before.score,
            "applying the interview must raise readiness: {} -> {}",
            before.score,
            after.score
        );

        // (3) The skipped blank parent answer left no parent relationship behind.
        assert!(
            !reloaded
                .relationships
                .iter()
                .any(|r| matches!(r.rel_type, aida_core::RelationshipType::Child)),
            "blank parent answer must not have written a parent link"
        );
    }
}
