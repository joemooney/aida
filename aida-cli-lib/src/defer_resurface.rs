//! Timer-only deferral evaluation. Observations grant no hold-lifting authority.
// trace:STORY-2402 | ai:codex
use std::path::Path;

use aida_core::condition::{Condition, ConditionFacts, Verdict};
use aida_core::{CachedGitBackend, Comment, DatabaseBackend, QueueEntry, Requirement};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Resurface,
    Human,
    Keep,
}

fn action(
    req: &Requirement,
    facts: &dyn ConditionFacts,
    now: DateTime<Utc>,
    manual_age_days: u32,
) -> Result<(Action, String)> {
    if !req.deferred || req.archived {
        return Ok((Action::Keep, "not an active deferral".into()));
    }
    let Some(raw) = req.deferred_until.as_deref() else {
        return Ok((Action::Keep, "no condition recorded".into()));
    };
    let condition: Condition = raw.parse()?;
    let evaluation = condition.evaluate(facts)?;
    let action = match evaluation.verdict {
        Verdict::Met if req.deferred_reason.is_some() || req.tags.contains("operator-held") => {
            Action::Human
        }
        Verdict::Met => Action::Resurface,
        Verdict::Manual
            if req.deferred_at.is_some_and(|at| {
                now.signed_duration_since(at) > chrono::Duration::days(i64::from(manual_age_days))
            }) =>
        {
            Action::Human
        }
        _ => Action::Keep,
    };
    let evidence = format!(
        "{condition}: {} ({})",
        evaluation.verdict.as_str(),
        evaluation.observed
    );
    let evidence = match (&action, evaluation.verdict) {
        (Action::Human, Verdict::Met) => format!("operator hold retained; {evidence}"),
        (Action::Human, Verdict::Manual) => {
            format!("manual condition older than {manual_age_days} days; {evidence}")
        }
        _ => evidence,
    };
    Ok((action, evidence))
}

/// Compare the complete observed row under the writer lock. An edited trigger,
/// newly placed hold, archive or concurrent sweep must never be overwritten.
fn clear_if_unchanged(
    current: &mut Requirement,
    observed: &Requirement,
    evidence: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    if serde_yaml::to_value(&*current)? != serde_yaml::to_value(observed)? {
        return Ok(false);
    }
    crate::defer_cmd::clear_deferral(current, now);
    current.add_comment(Comment::new(
        "aida:defer-resurface".into(),
        format!("Automatically resurfaced deferred work: {evidence}"),
    ));
    current.modified_at = now;
    Ok(true)
}

pub(crate) fn run(
    project_root: &Path,
    store_path: &Path,
    backend: &CachedGitBackend,
    dry_run: bool,
    manual_age_days: u32,
) -> Result<()> {
    // Strict canonical reads: corruption cannot silently shrink the sweep.
    let snapshot = crate::condition_cmd::read_store(project_root, Some(store_path))?;
    let file = store_path.to_str().context("store path is not UTF-8")?;
    let facts = crate::condition_cmd::CliConditionFacts::new(project_root, Some(file));
    sweep(
        backend,
        &snapshot.requirements,
        &facts,
        Utc::now(),
        dry_run,
        manual_age_days,
    )
}

fn sweep(
    backend: &CachedGitBackend,
    rows: &[Requirement],
    facts: &dyn ConditionFacts,
    now: DateTime<Utc>,
    dry_run: bool,
    manual_age_days: u32,
) -> Result<()> {
    let mut errors = Vec::new();
    for req in rows.iter().filter(|r| r.deferred && !r.archived) {
        let id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
        let result = (|| -> Result<()> {
            let (action, evidence) = action(req, facts, now, manual_age_days)?;
            println!(
                "{id}: {action:?} — {evidence}{}",
                if dry_run { " (dry run)" } else { "" }
            );
            if dry_run || action == Action::Keep {
                return Ok(());
            }
            match action {
                Action::Resurface => {
                    let mut changed = Ok(false);
                    backend.update_atomically(|store| {
                        if let Some(current) =
                            store.requirements.iter_mut().find(|r| r.id == req.id)
                        {
                            changed = clear_if_unchanged(current, req, &evidence, now);
                        }
                    })?;
                    if !changed? {
                        println!("{id}: changed during evaluation; retained for the next sweep");
                    }
                }
                Action::Human => {
                    // A stable shared queue identity makes timer runs independent
                    // of the invoking seat. Existing rows retain order and notes.
                    let user = "role:human";
                    if backend
                        .queue_list(user, true)?
                        .iter()
                        .any(|e| e.requirement_id == req.id)
                    {
                        return Ok(());
                    }
                    // Do not route a row whose deferral changed during probing.
                    let current = backend.get_requirement(&req.id)?;
                    if current.as_ref().map(serde_yaml::to_value).transpose()?
                        != Some(serde_yaml::to_value(req)?)
                    {
                        println!("{id}: changed during evaluation; retained for the next sweep");
                        return Ok(());
                    }
                    backend.queue_add(QueueEntry {
                        user_id: user.into(),
                        requirement_id: req.id,
                        position: i64::MAX,
                        added_by: "aida:defer-resurface".into(),
                        note: Some(evidence),
                        added_at: now,
                        for_role: Some("human".into()),
                        for_scope: None,
                        for_session: None,
                        added_by_machine: None,
                    })?;
                }
                Action::Keep => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("{id}: {error:#}");
            errors.push(format!("{id}: {error:#}"));
        }
    }
    anyhow::ensure!(
        errors.is_empty(),
        "deferral sweep errors: {}",
        errors.join("; ")
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_2402_resurface_tests.rs"]
mod tests;
