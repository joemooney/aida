//! One-time, lossless migration of legacy deferral prose and tags.
// trace:STORY-2402 | ai:codex

use aida_core::condition::Condition;
use aida_core::{
    CachedGitBackend, Comment, DatabaseBackend, Requirement, RequirementStatus, RequirementsStore,
};
use anyhow::Result;
use regex::Regex;
use serde::Serialize;
use std::path::Path;
use std::sync::LazyLock;

#[path = "defer_condition_migration_args.rs"]
pub(crate) mod args;

/// Only recognize a whole, simple trigger. A compound or qualified sentence
/// must not accidentally become permission to resurface on just one clause.
fn infer(raw: &str, snapshot: &RequirementsStore) -> Option<String> {
    if let Ok(condition) = raw.parse::<Condition>() {
        return resolved_condition(condition, snapshot);
    }
    static SPEC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^(?:(?:when|after)\s+)?([a-z]+-[0-9]+(?:-[0-9]+)*)\s+([a-z][a-z _-]*)[.!]?$",
        )
        .unwrap()
    });
    static PR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:(?:when|after)\s+)?#([0-9]+)\s+merges[.!]?$").unwrap()
    });
    static VERSION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:(?:when|after|release|version)\s+)?v?([0-9]+\.[0-9]+(?:\.[0-9]+)?)(?:\s+(?:ships|lands|releases))?[.!]?$").unwrap()
    });
    static DATE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:(?:on|after|until)\s+)?([0-9]{4}-[0-9]{2}-[0-9]{2})[.!]?$").unwrap()
    });
    let raw = raw.trim();
    let candidate = if let Some(c) = SPEC.captures(raw) {
        let status = match c[2].to_ascii_lowercase().as_str() {
            "completes" | "ships" | "lands" | "merges" | "shipped" | "landed" | "merged" => {
                RequirementStatus::Completed
            }
            _ => RequirementStatus::from_filter_str(&c[2])?,
        };
        format!("spec:{}={}", &c[1], status.cache_key())
    } else if let Some(c) = PR.captures(raw) {
        format!("pr:{}=merged", &c[1])
    } else if let Some(c) = VERSION.captures(raw) {
        format!("release:>={}", &c[1])
    } else if let Some(c) = DATE.captures(raw) {
        format!("date:{}", &c[1])
    } else {
        return None;
    };
    resolved_condition(candidate.parse::<Condition>().ok()?, snapshot)
}

// Inferred spec triggers must resolve just as the hourly evaluator resolves them.
// Missing or ambiguous targets retain a manual-review route.
// trace:STORY-2402 | ai:codex
fn resolved_condition(condition: Condition, snapshot: &RequirementsStore) -> Option<String> {
    if let Condition::Spec { id, .. } = &condition {
        snapshot.get_requirement_unambiguous(id).ok()??;
    }
    Some(condition.to_string())
}

#[derive(Debug, Serialize)]
pub(crate) struct SpecPlan {
    pub spec_id: String,
    pub original_until: Option<String>,
    pub legacy_tags: Vec<String>,
    pub condition: String,
    pub malformed_typed: bool,
    pub manual_overdue: bool,
}

fn plan_spec(req: &Requirement, snapshot: &RequirementsStore) -> Option<SpecPlan> {
    let mut legacy_tags: Vec<_> = req
        .tags
        .iter()
        .filter(|tag| tag.trim().to_ascii_lowercase().starts_with("deferred:"))
        .cloned()
        .collect();
    legacy_tags.sort();
    let typed = req
        .deferred_until
        .as_deref()
        .and_then(|s| s.parse::<Condition>().ok());
    if legacy_tags.is_empty() && (typed.is_some() || !req.deferred) {
        return None;
    }
    let sources: Vec<_> = req
        .deferred_until
        .iter()
        .map(String::as_str)
        .chain(
            legacy_tags
                .iter()
                .map(|tag| tag.trim().split_once(':').unwrap().1),
        )
        .collect();
    let manual = || {
        format!("manual:{}", req.owner.trim())
            .parse::<Condition>()
            .unwrap_or_else(|_| "manual:operator".parse().unwrap())
            .to_string()
    };
    // An explicit typed trigger wins. Otherwise every legacy source must agree;
    // conflicting tags or uninterpretable text require a person's decision.
    let condition = if typed.is_some() {
        // Even spelling/whitespace of an already valid typed trigger is retained.
        req.deferred_until.clone().unwrap()
    } else {
        let inferred: Vec<_> = sources.iter().map(|s| infer(s, snapshot)).collect();
        match inferred.first() {
            Some(Some(first)) if inferred.iter().all(|c| c.as_ref() == Some(first)) => {
                first.clone()
            }
            _ => manual(),
        }
    };
    let malformed_typed = sources.iter().any(|s| {
        s.split_once(':').is_some_and(|(kind, _)| {
            matches!(
                kind.trim().to_ascii_lowercase().as_str(),
                "spec" | "pr" | "release" | "date" | "query" | "manual"
            )
        }) && s.parse::<Condition>().is_err()
    });
    let manual_overdue = !req.archived
        && condition
            .parse::<Condition>()
            .is_ok_and(|c| matches!(c, Condition::Manual { .. }))
        && chrono::Utc::now().signed_duration_since(req.deferred_at.unwrap_or(req.modified_at))
            > chrono::Duration::days(14);
    Some(SpecPlan {
        spec_id: req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string()),
        original_until: req.deferred_until.clone(),
        legacy_tags,
        condition,
        malformed_typed,
        manual_overdue,
    })
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct MigrationOutcome {
    pub dry_run: bool,
    pub specs_scanned: usize,
    pub specs_written: usize,
    pub specs: Vec<SpecPlan>,
}

fn apply_plan(req: &mut Requirement, plan: &SpecPlan) -> Result<()> {
    let previous_modified_at = req.modified_at;
    // JSON preserves exact original strings, including newlines and whitespace.
    let original = serde_json::json!({
        "deferred_until": plan.original_until,
        "legacy_tags": plan.legacy_tags,
        "deferred": req.deferred,
        "deferred_at": req.deferred_at,
    });
    req.add_comment(Comment::new(
        "aida:defer-migration".into(),
        format!(
            "[aida:defer-migration]\n{}\nMigrated condition: {}",
            serde_json::to_string(&original)?,
            plan.condition
        ),
    ));
    req.deferred_until = Some(plan.condition.clone());
    if !plan.legacy_tags.is_empty() {
        req.deferred = true;
        for tag in &plan.legacy_tags {
            req.tags.remove(tag);
        }
    }
    if req.deferred {
        // Use existing evidence rather than restarting the manual-age clock.
        req.deferred_at.get_or_insert(previous_modified_at);
    }
    req.modified_at = chrono::Utc::now();
    Ok(())
}

/// Strict canonical snapshot; re-plan each row under the existing writer lock.
/// No whole-store save, stale-snapshot overwrite or trigger evaluation occurs.
pub(crate) fn run_migration(
    backend: &CachedGitBackend,
    store_path: &Path,
    apply: bool,
) -> Result<MigrationOutcome> {
    let snapshot = crate::condition_cmd::read_store(store_path, Some(store_path))?;
    let mut rows: Vec<_> = snapshot.requirements.iter().collect();
    rows.sort_by_key(|r| r.id);
    let mut outcome = MigrationOutcome {
        dry_run: !apply,
        specs_scanned: rows.len(),
        ..Default::default()
    };
    for row in rows {
        let Some(plan) = plan_spec(row, &snapshot) else {
            continue;
        };
        if !apply {
            outcome.specs.push(plan);
            continue;
        }
        if let Some(plan) = migrate_spec(backend, row, &snapshot)? {
            outcome.specs_written += 1;
            outcome.specs.push(plan);
        }
    }
    Ok(outcome)
}

fn migrate_spec(
    backend: &CachedGitBackend,
    observed: &Requirement,
    snapshot: &RequirementsStore,
) -> Result<Option<SpecPlan>> {
    let mut result: Result<Option<SpecPlan>> = Ok(None);
    let updated = backend.update_spec_atomically(observed, |current| {
        result = (|| {
            let Some(plan) = plan_spec(current, snapshot) else {
                return Ok(None);
            };
            apply_plan(current, &plan)?;
            Ok(Some(plan))
        })();
    })?;
    let updated =
        updated.ok_or_else(|| anyhow::anyhow!("{} disappeared during migration", observed.id))?;
    let plan = result?;
    if let Some(plan) = &plan {
        anyhow::ensure!(
            updated.deferred_until.as_ref() == Some(&plan.condition)
                && plan
                    .legacy_tags
                    .iter()
                    .all(|tag| !updated.tags.contains(tag)),
            "{} failed migration verification",
            plan.spec_id
        );
    }
    Ok(plan)
}

pub(crate) fn render_outcome(outcome: &MigrationOutcome) -> String {
    use std::fmt::Write;
    let mut text = String::new();
    for plan in &outcome.specs {
        let _ = writeln!(
            text,
            "{}: trigger={} tags={} -> {}{}",
            plan.spec_id,
            plan.original_until
                .as_deref()
                .map(|s| serde_json::to_string(s).unwrap())
                .unwrap_or_else(|| "(none)".into()),
            serde_json::to_string(&plan.legacy_tags).unwrap(),
            plan.condition,
            if plan.malformed_typed {
                " (malformed typed input)"
            } else {
                ""
            }
        );
    }
    let _ = writeln!(
        text,
        "{} specs scanned; {} migration candidate(s); {} written.",
        outcome.specs_scanned,
        outcome.specs.len(),
        outcome.specs_written
    );
    let _ = writeln!(
        text,
        "{} resulting manual condition(s) older than the default 14-day threshold; eligible for aida human on the next hourly run (configured threshold may differ).",
        outcome.specs.iter().filter(|plan| plan.manual_overdue).count()
    );
    if outcome.dry_run {
        let _ = writeln!(
            text,
            "Dry run: nothing written. Re-run with --apply to migrate."
        );
    }
    text
}

#[cfg(test)]
#[path = "tests/story2402_s3_migration.rs"]
mod tests;
