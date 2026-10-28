//! `aida db audit-ids`: a read-only audit of allocation claims.
//!
//! Runs the collector every store sync runs before it merges
//! (`aida_core::allocation_conflicts`) over the store's `HEAD` joined with
//! any `--against` refs, exactly as a sync would, so each conflict it finds
//! there is one a sync refuses. It also judges `HEAD`'s own tree against an
//! empty baseline, which no sync does: conflicts already inside `HEAD` that a
//! sync admits are reported too, marked as such. All commits are
//! already local. It prints the conflict classification, the sanitized report
//! and a manual repair plan. It reads committed trees only: it never fetches,
//! never opens a `Storage` or `GitBackend` (so no load-time migration runs)
//! and writes nothing.
// trace:BUG-2017 | ai:claude

use aida_core::allocation_conflicts::{ConflictKind, ConflictReport, ConflictScope, GapKind};
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

/// One audit: the report, and whether any `--against` ref was joined (only
/// then can it tell which conflicts a sync refuses).
#[derive(Debug)]
pub(crate) struct Audit {
    pub(crate) report: ConflictReport,
    pub(crate) against: bool,
}

impl std::ops::Deref for Audit {
    type Target = ConflictReport;
    fn deref(&self) -> &ConflictReport {
        &self.report
    }
}

impl std::fmt::Display for Audit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.report.fmt(f)
    }
}

impl Audit {
    fn count(&self, scope: ConflictScope) -> usize {
        self.conflicts.iter().filter(|c| c.scope == scope).count()
    }

    /// Whether a sync with the audited refs refuses: a conflict it would
    /// introduce, or a claim this audit could not read.
    pub(crate) fn blocks_sync(&self) -> bool {
        !self.is_complete() || self.count(ConflictScope::Introduced) > 0
    }
}

/// The audit outcome: `clean` only when every claim was read and none
/// conflict. `incomplete` wins over `conflicts` (a sync refuses at least one)
/// and `conflicts_in_head` (every conflict is already inside `HEAD`): an
/// audit that could not see every claim is never a clean answer.
fn status(audit: &Audit) -> &'static str {
    if !audit.is_complete() {
        "incomplete"
    } else if audit.count(ConflictScope::Introduced) > 0 {
        "conflicts"
    } else if !audit.conflicts.is_empty() {
        "conflicts_in_head"
    } else {
        "clean"
    }
}

/// The fixed label for a conflict's scope.
fn scope_label(scope: ConflictScope) -> &'static str {
    match scope {
        ConflictScope::Introduced => "sync refuses",
        ConflictScope::InHead => "already in HEAD",
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

/// The manual repair steps for what the audit found, in order. Empty for a
/// clean report. Fixed text only: nothing from the store is interpolated.
pub(crate) fn repair_plan(audit: &Audit) -> Vec<&'static str> {
    if audit.is_clean() {
        return Vec::new();
    }
    let refused = |kind: ConflictKind| {
        audit
            .conflicts
            .iter()
            .any(|c| c.kind == kind && c.scope == ConflictScope::Introduced)
    };
    let has = |kind: ConflictKind| audit.conflicts.iter().any(|c| c.kind == kind);
    let in_head = audit.count(ConflictScope::InHead) > 0;
    let mut plan = Vec::new();
    if audit.blocks_sync() {
        plan.push(
            "Do not sync the store with these refs until a re-audit is complete and \
             lists no conflict marked `sync refuses`: a store sync refuses those. This \
             audit changed nothing.",
        );
    } else {
        plan.push("This audit changed nothing.");
    }
    if in_head {
        plan.push(
            "Conflicts marked `already in HEAD` do not block a sync. Syncs refuse only \
             newly introduced conflicts (claims that differ between the local tip, the \
             incoming tip and their merge bases); conflicts already inside HEAD are \
             admitted by syncs and surfaced only by `aida db audit-ids`.",
        );
        if !audit.against {
            plan.push(
                "Without --against this audit cannot tell whether a conflict inside HEAD \
                 is also new since the last sync: re-run it with `--against` naming the \
                 incoming ref to see which ones a sync refuses.",
            );
        }
    }
    if !audit.is_complete() {
        plan.push(
            "Make every claim readable: fetch the missing history (or deepen a shallow \
             clone), check that each --against ref exists locally, and correct any \
             malformed registry or object file.",
        );
    }
    if refused(ConflictKind::GlobalRange)
        || refused(ConflictKind::DisplayId)
        || has(ConflictKind::Unissued)
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
    if audit
        .conflicts
        .iter()
        .any(|c| c.kind == ConflictKind::DisplayId && c.scope == ConflictScope::InHead)
    {
        plan.push(
            "For display IDs already claimed twice inside HEAD, `aida db check --collisions` \
             lists every claimant (matching IDs case-insensitively, as this audit does): \
             keep one and give each other claimant a new display ID by hand.",
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
    if audit.blocks_sync() {
        plan.push(
            "Re-run `aida db audit-ids` with the same --against refs until it lists no \
             conflict marked `sync refuses` and is complete, then retry the sync.",
        );
    } else {
        plan.push("Re-run `aida db audit-ids` after repairing to confirm HEAD is clean.");
    }
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
    scopes: BTreeMap<ConflictScope, usize>,
    gaps: BTreeMap<GapKind, usize>,
}

pub(crate) fn render_json(report: &Audit) -> Result<String> {
    let doc = AuditJson {
        status: status(report),
        counts: Counts {
            conflicts: conflict_counts(report),
            scopes: [ConflictScope::Introduced, ConflictScope::InHead]
                .into_iter()
                .map(|s| (s, report.count(s)))
                .collect(),
            gaps: gap_counts(report),
        },
        conflicts: &report.conflicts,
        gaps: &report.gaps,
        repair_plan: repair_plan(report),
    };
    Ok(crate::cache_output::json_pretty(&doc)?)
}

pub(crate) fn render_human(report: &Audit) -> String {
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
        let _ = writeln!(
            out,
            "  ({} a sync refuses, {} already in HEAD)",
            report.count(ConflictScope::Introduced),
            report.count(ConflictScope::InHead)
        );
        let _ = writeln!(out, "\nConflicts:");
        for c in &report.conflicts {
            let refs: Vec<String> = c.refs.iter().map(ToString::to_string).collect();
            let _ = writeln!(
                out,
                "  [{}] {}: {}",
                scope_label(c.scope),
                c.kind.describe(),
                refs.join(", ")
            );
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

/// Audit `store_path`'s `HEAD` (its own tree, and joined with each of
/// `against`), already-local commits only.
pub(crate) fn audit(store_path: &Path, against: &[String]) -> Audit {
    Audit {
        report: aida_core::allocation_conflicts::audit(store_path, against),
        against: !against.is_empty(),
    }
}

/// The exit code: 0 clean; 1 incomplete or a conflict a sync refuses; 2 only
/// conflicts already inside `HEAD`, which a sync admits.
pub(crate) fn exit_code(audit: &Audit) -> i32 {
    if audit.blocks_sync() {
        1
    } else if audit.conflicts.is_empty() {
        0
    } else {
        2
    }
}

/// Print the audit and exit with [`exit_code`].
pub(crate) fn run(store_path: &Path, args: &AuditIdsArgs) -> Result<()> {
    let report = audit(store_path, &args.against);
    if args.json {
        println!("{}", render_json(&report)?);
    } else {
        print!("{}", render_human(&report));
    }
    let code = exit_code(&report);
    if code != 0 {
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/bug_2017_audit_ids_tests.rs"]
mod tests;
