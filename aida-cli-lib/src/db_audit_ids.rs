//! `aida db audit-ids`: a read-only audit of allocation claims.
//!
//! Runs the same collector every store sync runs before it merges
//! (`aida_core::allocation_conflicts`) over the store's `HEAD` plus any
//! `--against` refs, all already-local commits, and prints the conflict
//! classification, the sanitized report and a manual repair plan. It reads
//! committed trees only: it never fetches, never opens a `Storage` or
//! `GitBackend` (so no load-time migration runs) and writes nothing.
// trace:BUG-2017 | ai:claude

use aida_core::allocation_conflicts::{collect, ConflictKind, ConflictReport, GapKind};
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[path = "db_audit_ids_args.rs"]
mod db_audit_ids_args;
pub(crate) use db_audit_ids_args::AuditIdsArgs;

const KINDS: [ConflictKind; 4] = [
    ConflictKind::GlobalRange,
    ConflictKind::NodeBinding,
    ConflictKind::DisplayId,
    ConflictKind::Unissued,
];

/// The audit outcome: `clean` only when every claim was read and none
/// conflict. `incomplete` wins over `conflicts`: an audit that could not see
/// every claim is never a clean answer.
fn status(report: &ConflictReport) -> &'static str {
    if !report.is_complete() {
        "incomplete"
    } else if report.conflicts.is_empty() {
        "clean"
    } else {
        "conflicts"
    }
}

/// Conflicts per category, every category present.
fn conflict_counts(report: &ConflictReport) -> BTreeMap<ConflictKind, usize> {
    let mut counts: BTreeMap<ConflictKind, usize> = KINDS.iter().map(|k| (*k, 0)).collect();
    for c in &report.conflicts {
        *counts.entry(c.kind).or_default() += 1;
    }
    counts
}

fn gap_counts(report: &ConflictReport) -> BTreeMap<GapKind, usize> {
    let mut counts = BTreeMap::new();
    for g in &report.gaps {
        *counts.entry(g.kind).or_default() += 1;
    }
    counts
}

/// The manual repair steps for what the report found, in order. Empty for a
/// clean report. Fixed text only: nothing from the store is interpolated.
pub(crate) fn repair_plan(report: &ConflictReport) -> Vec<&'static str> {
    if report.is_clean() {
        return Vec::new();
    }
    let has = |kind: ConflictKind| report.conflicts.iter().any(|c| c.kind == kind);
    let mut plan = vec![
        "Do not sync the store until a re-audit is clean: every store sync refuses \
         these claims. This audit changed nothing.",
    ];
    if !report.is_complete() {
        plan.push(
            "Make every claim readable: fetch the missing history (or deepen a shallow \
             clone), check that each --against ref exists locally, and correct any \
             malformed registry or object file.",
        );
    }
    if has(ConflictKind::GlobalRange) || has(ConflictKind::DisplayId) || has(ConflictKind::Unissued)
    {
        plan.push(
            "On the authoritative clone, claim a fresh global block disjoint from every \
             recorded range (`aida db block claim`).",
        );
        plan.push(
            "Keep every object UUID. Give each conflicting object a new display ID from \
             that block, keep its old display ID as an alias, and record the old-to-new \
             mapping for review.",
        );
    }
    if has(ConflictKind::GlobalRange) {
        plan.push(
            "Retire the overlapping range rows by hand on the authoritative clone so no \
             two recorded ranges can dispense one ID.",
        );
    }
    if has(ConflictKind::NodeBinding) {
        plan.push(
            "For each conflicting node id, decide which registration it keeps and correct \
             the other registration by hand on the authoritative clone.",
        );
    }
    plan.push(
        "Re-run `aida db audit-ids` with the same --against refs until it reports clean, \
         then retry the sync.",
    );
    plan
}

#[derive(Serialize)]
struct AuditJson<'a> {
    status: &'static str,
    counts: Counts,
    conflicts: &'a [aida_core::allocation_conflicts::Conflict],
    gaps: &'a [aida_core::allocation_conflicts::Gap],
    repair_plan: Vec<&'static str>,
}

#[derive(Serialize)]
struct Counts {
    conflicts: BTreeMap<ConflictKind, usize>,
    gaps: BTreeMap<GapKind, usize>,
}

pub(crate) fn render_json(report: &ConflictReport) -> Result<String> {
    let doc = AuditJson {
        status: status(report),
        counts: Counts {
            conflicts: conflict_counts(report),
            gaps: gap_counts(report),
        },
        conflicts: &report.conflicts,
        gaps: &report.gaps,
        repair_plan: repair_plan(report),
    };
    Ok(crate::cache_output::json_pretty(&doc)?)
}

pub(crate) fn render_human(report: &ConflictReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Allocation claims audit: {} ({} conflict(s), {} gap(s))",
        status(report),
        report.conflicts.len(),
        report.gaps.len()
    );
    for (kind, n) in conflict_counts(report) {
        let _ = writeln!(out, "  {n:>4}  {}", kind.describe());
    }
    if !report.conflicts.is_empty() {
        let _ = writeln!(out, "\nConflicts:");
        for c in &report.conflicts {
            let refs: Vec<String> = c.refs.iter().map(ToString::to_string).collect();
            let _ = writeln!(out, "  {}: {}", c.kind.describe(), refs.join(", "));
        }
    }
    if !report.gaps.is_empty() {
        let _ = writeln!(out, "\nIncomplete:");
        for g in &report.gaps {
            let mut line = format!("  {}", g.kind.describe());
            if let Some(class) = g.class {
                let _ = write!(line, " in {}", class.as_str());
                if let Some(n) = g.ordinal {
                    let _ = write!(line, "#{n}");
                }
            }
            if let Some(commit) = &g.commit {
                let short: String = commit.chars().take(12).collect();
                let _ = write!(line, " @ {short}");
            }
            let _ = writeln!(out, "{line}");
        }
    }
    let plan = repair_plan(report);
    if plan.is_empty() {
        let _ = writeln!(
            out,
            "\nNo conflicting allocation claims among the inspected commits."
        );
    } else {
        let _ = writeln!(
            out,
            "\nManual repair plan (nothing is repaired automatically):"
        );
        for (i, step) in plan.iter().enumerate() {
            let _ = writeln!(out, "  {}. {step}", i + 1);
        }
    }
    out
}

/// Audit `store_path`'s `HEAD` and `against`, already-local commits only.
pub(crate) fn audit(store_path: &Path, against: &[String]) -> ConflictReport {
    collect(store_path, Some("HEAD"), against)
}

/// Print the audit; exit 1 unless it is complete and conflict-free.
pub(crate) fn run(store_path: &Path, args: &AuditIdsArgs) -> Result<()> {
    let report = audit(store_path, &args.against);
    if args.json {
        println!("{}", render_json(&report)?);
    } else {
        print!("{}", render_human(&report));
    }
    if !report.is_clean() {
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/bug_2017_audit_ids_tests.rs"]
mod tests;
