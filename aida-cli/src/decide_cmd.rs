//! `aida decide` command (records an ADR decision / answers a pending
//! DecisionRequest).
//!
//! The operator-facing entry point that either answers a spec's pending
//! DecisionRequest (behaving like `aida questions answer`) or drops into the
//! clarify flow when there is nothing pending (behaving like
//! `aida questions clarify`). All the decision-prompt and answer/clarify
//! machinery is shared with the `questions` command and stays in `main.rs`;
//! this module holds only the thin dispatch handler. Extracted verbatim from
//! `main.rs` (SPIKE-78); no behavior change.

use anyhow::Result;
use colored::Colorize;
use std::io::IsTerminal;

use aida_core::DatabaseBackend;

use crate::not_found;
use crate::{
    has_open_decision_request, print_decision_request, prompt_decision_action,
    questions_answer_one, questions_clarify, DecisionPromptAction,
};

pub(crate) fn handle_decide_command(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    spec: &str,
) -> Result<()> {
    let req = backend
        .get_requirement_by_spec_id(spec)?
        .ok_or_else(|| not_found::requirement_not_found(spec, Some(store_path)))?;
    let display_id = req.display_id();

    if has_open_decision_request(&req) {
        // Pending DecisionRequest → behave like `aida questions answer <spec>`.
        // The request is pending (has_open_decision_request guaranteed it), so
        // the field is present and non-resolved.
        let dr = req
            .decision_request
            .as_ref()
            .expect("pending DecisionRequest present");
        let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        if !interactive {
            anyhow::bail!(
                "{display_id} has a pending decision — answer it with \
                 `aida questions answer {display_id} <choice>` (this needs a TTY)"
            );
        }
        println!();
        print_decision_request(&display_id, &req.title, dr);
        // trace:TASK-791 | ai:claude — shared prompt carries the type-note + chat escapes.
        let (choice, note) = match prompt_decision_action(dr)? {
            DecisionPromptAction::Skip => {
                println!("  {}", "skipped".dimmed());
                return Ok(());
            }
            DecisionPromptAction::NoOp => return Ok(()),
            DecisionPromptAction::Chat => {
                println!(
                    "  {} dropping into clarify — discuss, then re-run `aida decide`",
                    "→".green()
                );
                return questions_clarify(
                    backend,
                    store_path,
                    std::slice::from_ref(&display_id),
                    false,
                );
            }
            DecisionPromptAction::Pick(c) => (c, None),
            DecisionPromptAction::PickWithNote { choice, note } => (choice, Some(note)),
        };
        // Reuse the existing single-answer handler: it records + applies the
        // resolution and auto-queues the now-decision-free spec.
        questions_answer_one(backend, store_path, &display_id, &choice, note.as_deref())
    } else {
        // No pending decision → behave like `aida questions clarify <spec>`.
        questions_clarify(
            backend,
            store_path,
            std::slice::from_ref(&display_id),
            false,
        )
    }
}
