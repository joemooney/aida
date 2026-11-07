//! One-time, lossless migration of legacy deferral prose and tags.
// trace:STORY-2402 | ai:codex

use aida_core::condition::Condition;
use aida_core::{CachedGitBackend, Comment, DatabaseBackend, Requirement};
use anyhow::Result;
use regex::Regex;
use serde::Serialize;
use std::path::Path;
use std::sync::LazyLock;

#[path = "defer_condition_migration_args.rs"]
pub(crate) mod args;

/// Only recognize a whole, simple trigger. A compound or qualified sentence
/// must not accidentally become permission to resurface on just one clause.
fn infer(raw: &str) -> Option<String> {
    if let Ok(condition) = raw.parse::<Condition>() {
        return Some(condition.to_string());
    }
    static SPEC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:(?:when|after)\s+)?(STORY-[a-z0-9]+(?:-[a-z0-9]+)*)\s+(?:completes|ships|lands)[.!]?$").unwrap()
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
        format!("spec:{}=completed", &c[1])
    } else if let Some(c) = PR.captures(raw) {
        format!("pr:{}=merged", &c[1])
    } else if let Some(c) = VERSION.captures(raw) {
        format!("release:>={}", &c[1])
    } else if let Some(c) = DATE.captures(raw) {
        format!("date:{}", &c[1])
    } else {
        return None;
    };
    candidate.parse::<Condition>().ok().map(|c| c.to_string())
}

#[derive(Debug, Serialize)]
pub(crate) struct SpecPlan {
    pub spec_id: String,
    pub original_until: Option<String>,
    pub legacy_tags: Vec<String>,
    pub condition: String,
    pub malformed_typed: bool,
}

fn plan_spec(req: &Requirement) -> Option<SpecPlan> {
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
    if legacy_tags.is_empty()
        && (typed.is_some() || (!req.deferred && req.deferred_until.is_none()))
    {
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
        let inferred: Vec<_> = sources.iter().map(|s| infer(s)).collect();
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
        // Use existing evidence rather than restarting the manual-age clock.
        req.deferred_at.get_or_insert(previous_modified_at);
        for tag in &plan.legacy_tags {
            req.tags.remove(tag);
        }
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
    let mut rows = snapshot.requirements;
    rows.sort_by_key(|r| r.id);
    let mut outcome = MigrationOutcome {
        dry_run: !apply,
        specs_scanned: rows.len(),
        ..Default::default()
    };
    for row in rows {
        let Some(plan) = plan_spec(&row) else {
            continue;
        };
        if !apply {
            outcome.specs.push(plan);
            continue;
        }
        if let Some(plan) = migrate_spec(backend, &row)? {
            outcome.specs_written += 1;
            outcome.specs.push(plan);
        }
    }
    Ok(outcome)
}

fn migrate_spec(backend: &CachedGitBackend, observed: &Requirement) -> Result<Option<SpecPlan>> {
    let mut result: Result<Option<SpecPlan>> = Ok(None);
    let updated = backend.update_spec_atomically(observed, |current| {
        result = (|| {
            let Some(plan) = plan_spec(current) else {
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
            "{}: {:?} + {:?} -> {}{}",
            plan.spec_id,
            plan.original_until,
            plan.legacy_tags,
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
