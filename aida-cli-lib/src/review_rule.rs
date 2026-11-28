//! `aida review rule <PR> --fix-first --reason TEXT`: an advisor's FIX-FIRST
//! ruling held at the PR's current head in one step, so that head leaves train
//! candidacy at once. Only a new head clears it; normal review applies to it.
// trace:TASK-3923 | ai:claude

use anyhow::{bail, ensure, Result};

/// `4702`, `#4702`, `PR-4702` or `MR-4702`.
pub(crate) fn pr_ref(raw: &str) -> std::result::Result<u64, String> {
    crate::gate::ledger::ChangeKey::parse(raw)
        .map(|c| c.number)
        .or_else(|| raw.trim().trim_start_matches('#').parse().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("`{raw}` is not a pull request (use 4702, #4702 or PR-4702)"))
}

/// Hold `pr` at `head` and note the ruling on each covered spec. Only the
/// advisor seat or an operator (no seat) rules. `false`: already ruled here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rule_fix_first_at(
    train_root: &std::path::Path,
    pr: u64,
    head: &str,
    seat: Option<&str>,
    by: &str,
    reason: &str,
    specs: &[String],
    mut note: impl FnMut(&str, &str) -> Result<()>,
) -> Result<bool> {
    ensure!(
        matches!(seat, None | Some("advisor")),
        "refused: only the advisor seat or an operator records a fix-first ruling \
         (this is the {} seat); nothing was recorded",
        seat.unwrap_or("unknown")
    );
    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(
        !reason.is_empty(),
        "refused: --reason must say what to fix; nothing was recorded"
    );
    if !crate::pr_train::culprit::record_fix_first(train_root, pr, head, by, &reason)? {
        return Ok(false);
    }
    let body = format!(
        "## FIX-FIRST ruling\n\nPR #{pr} at head {}, ruled by {by}:\n\n{reason}\n\n\
         This head is held out of train candidacy; a new head clears the hold and \
         normal review applies to it.",
        crate::pr_train::short(head)
    );
    for spec in specs {
        if let Err(e) = note(spec, &body) {
            eprintln!("  warning: could not note the ruling on {spec}: {e:#}");
        }
    }
    Ok(true)
}

/// `aida review rule`.
pub(crate) fn handle(storage: &crate::Storage, pr: u64, reason: &str) -> Result<()> {
    crate::standalone_implementer::refuse_under_marker("review rule")?;
    let root = crate::drive_root_or_project_root()?;
    if let Some(rejection) = crate::gate::seat_authority::grant_check(&root).rejection_sentence() {
        bail!("refused: review rule. {rejection}.");
    }
    let seat = crate::gate::seat_authority::current_seat(&root);
    let forge = crate::forge::resolve_open_change_forge_kind(&root);
    let change = match crate::merge_hold::fetch_pinned_change(&root, forge, pr) {
        Ok(Some(c)) if c.state == crate::merge_hold::ChangeState::Open => c,
        Ok(_) => bail!("refused: PR #{pr} is not an open pull request; nothing was recorded"),
        Err(e) => bail!("refused: could not read PR #{pr} ({e}); nothing was recorded"),
    };
    let Some(head) = change.head_sha else {
        bail!("refused: the forge returned no head for PR #{pr}; nothing was recorded");
    };
    let env = aida_core::exec_context::Environment::capture();
    let by = crate::review_recorded_by_from(
        env.get("AIDA_AGENT_NAME").and_then(|v| v.to_str()),
        env.get("AIDA_AGENT_TYPE").and_then(|v| v.to_str()),
        Some(seat.as_deref().unwrap_or("operator")),
    );
    let resolve = |n| crate::review_pr_subject_from_forge(&root, n);
    let specs = match crate::review_subject(&format!("PR-{pr}"), Some(pr), None, resolve) {
        Ok((ids, _)) => ids,
        Err(e) => {
            eprintln!("  warning: the ruling is not noted on a spec: {e:#}");
            Vec::new()
        }
    };
    let train_root = crate::main_worktree_root_from(&root);
    let note = |s: &str, b: &str| crate::comment_cmd::add_comment_cli(storage, s, b, None, None);
    let short = crate::pr_train::short(&head);
    if rule_fix_first_at(
        &train_root,
        pr,
        &head,
        seat.as_deref(),
        &by,
        reason,
        &specs,
        note,
    )? {
        println!("PR #{pr} is held at {short}: FIX-FIRST, ruled by {by}. A new head clears it.");
    } else {
        println!("PR #{pr} already carries a FIX-FIRST ruling at {short}; nothing changed.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task3923_ruling_holds_the_head_notes_the_spec_once_and_refuses_other_seats() {
        assert_eq!((pr_ref("#4702"), pr_ref("PR-4702")), (Ok(4702), Ok(4702)));
        assert!(pr_ref("TASK-1").is_err() && pr_ref("0").is_err());
        let dir = tempfile::tempdir().unwrap();
        let (root, head) = (dir.path(), "1e0b7323aa");
        let specs = vec!["TASK-3922".to_string()];
        let mut notes = Vec::new();
        let mut rule = |seat: Option<&str>, reason: &str| {
            rule_fix_first_at(
                root,
                4702,
                head,
                seat,
                "advisor-122",
                reason,
                &specs,
                |s, b| {
                    notes.push(format!("{s}: {b}"));
                    Ok(())
                },
            )
        };
        for seat in [Some("implementer"), Some("reviewer")] {
            assert!(rule(seat, "leak").is_err(), "{seat:?} is refused");
        }
        assert!(
            rule(Some("advisor"), "  ").is_err(),
            "an empty reason is refused"
        );
        assert_eq!(crate::pr_train::culprit::hold(root, 4702, head), None);
        assert!(rule(Some("advisor"), "restart_step echoes\nthe raw argv").unwrap());
        assert!(!rule(None, "again").unwrap(), "idempotent per head");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].starts_with("TASK-3922: ## FIX-FIRST ruling"),
            "{notes:?}"
        );
        assert!(notes[0].contains("ruled by advisor-122:\n\nrestart_step echoes the raw argv"));
        let why = crate::pr_train::culprit::hold(root, 4702, head).unwrap();
        assert!(
            why.ends_with("advisor-122: restart_step echoes the raw argv"),
            "{why}"
        );
        assert!(!crate::pr_train::culprit::clear(root, 4702, head, "op").unwrap());
        crate::pr_train::culprit::record(root, 4702, head, Some(4707)).unwrap();
        let why = crate::pr_train::culprit::hold(root, 4702, head).unwrap();
        assert!(
            why.starts_with(crate::pr_train::culprit::FIX_FIRST),
            "a ruling outranks a red"
        );
        assert_eq!(
            crate::pr_train::culprit::hold(root, 4702, "0123456789"),
            None
        );
    }
}
