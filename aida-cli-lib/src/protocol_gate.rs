//! Typed protocols slice 2: the protocol check at done.
//!
//! STORY-1221 stores an editable protocol per requirement type as META and
//! injects it at pickup. This module is the *gate* half: `aida queue done`
//! and PR creation (`aida pr ship`'s create step) evaluate the
//! machine-checkable items of the spec type's protocol and report each one as
//! met / missing / unknown, with the layer it came from.
//!
//! Machine-checkable items (the prose protocol body stays advisory):
//!   - spike    — a `docs/spikes/<date>-<slug>.md` deliverable is in the diff
//!   - decision — the ADR is accepted and carries a `references` edge
//!   - bug      — the PR touches at least one test file
//!
//! Posture follows `[protocol] enforce` (shared with the TASK-1290 criteria
//! gate via [`crate::criteria_gate::read_enforce_mode`]): `warn` (default)
//! prints the missing items, ledgers them, and proceeds; `refuse` blocks with
//! the item named; `--force` overrides with a ledger comment naming who
//! forced it. An item whose evidence cannot be read (no default branch, git
//! failure, on the default branch) is reported UNKNOWN — never counted as
//! met (PRIN-5) and never used to refuse on a guess.
//!
//! The deliverable-in-diff scan reuses the harvest loop's branch-diff helper
//! ([`crate::harvest::changed_files_base_to_head`]) and the stale-check's
//! test-path classifier ([`crate::is_test_path`]); no second implementation.
// trace:TASK-1277 | ai:claude

use aida_core::{RelationshipType, Requirement, RequirementStatus, RequirementType};
use std::path::Path;

use crate::criteria_gate::EnforceMode;

/// Marker text carried by a `--force` override ledger comment. PR creation
/// honours an override recorded at `aida queue done` by finding it.
pub(crate) const FORCE_LEDGER_MARKER: &str = "protocol --force override";

/// The layer an item came from. Precedence: type < lane < spec acceptance.
/// Only the type layer carries machine-checkable items today.
pub(crate) const LAYER_TYPE: &str = "type";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemKind {
    SpikeDeliverable,
    AdrAcceptedWithReferences,
    BugTestChange,
}

impl ItemKind {
    /// The checklist item, verbatim — the same words appear in the done
    /// report, the PR body, and the reviewer prompt.
    pub(crate) fn checklist_text(self) -> &'static str {
        match self {
            ItemKind::SpikeDeliverable => "deliverable docs/spikes/<date>-<slug>.md is in the diff",
            ItemKind::AdrAcceptedWithReferences => {
                "ADR status is accepted and it has a references edge"
            }
            ItemKind::BugTestChange => "the PR touches at least one test file",
        }
    }

    fn missing_text(self) -> &'static str {
        match self {
            ItemKind::SpikeDeliverable => "deliverable docs/spikes/<date>-<slug>.md",
            ItemKind::AdrAcceptedWithReferences => "accepted ADR with a references edge",
            ItemKind::BugTestChange => "test-file change",
        }
    }

    fn needs_diff(self) -> bool {
        matches!(self, ItemKind::SpikeDeliverable | ItemKind::BugTestChange)
    }
}

/// The protocol slug and machine-checkable items for a requirement type.
/// Types with no machine-checkable item return an empty list — the gate is
/// then silent.
// trace:TASK-1277 | ai:claude
pub(crate) fn machine_items(req_type: &RequirementType) -> (&'static str, &'static [ItemKind]) {
    match req_type {
        RequirementType::Spike => ("spike", &[ItemKind::SpikeDeliverable]),
        RequirementType::Decision => ("decision", &[ItemKind::AdrAcceptedWithReferences]),
        RequirementType::Bug => ("bug", &[ItemKind::BugTestChange]),
        _ => ("", &[]),
    }
}

/// The branch's changed files, or why they could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChangedFiles {
    Known(BranchDiff),
    Unknown(String),
}

/// What the branch diff says: every changed path, plus the Rust files whose
/// hunks touch inline test code (`#[cfg(test)]`, `#[test]`, `mod tests`) —
/// a bug fix whose regression test lives in the source file still counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BranchDiff {
    pub(crate) files: Vec<String>,
    pub(crate) test_hunk_files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ItemState {
    Met(String),
    Missing(String),
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ItemResult {
    pub(crate) kind: ItemKind,
    pub(crate) layer: &'static str,
    pub(crate) state: ItemState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtocolReport {
    pub(crate) protocol: &'static str,
    pub(crate) items: Vec<ItemResult>,
}

impl ProtocolReport {
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// One line per missing item, e.g.
    /// `protocol: spike — missing deliverable docs/spikes/<date>-<slug>.md (layer: type; …)`.
    pub(crate) fn missing_lines(&self) -> Vec<String> {
        self.items
            .iter()
            .filter_map(|item| match &item.state {
                ItemState::Missing(detail) => Some(format!(
                    "protocol: {} — missing {} (layer: {}; {})",
                    self.protocol,
                    item.kind.missing_text(),
                    item.layer,
                    detail
                )),
                _ => None,
            })
            .collect()
    }

    /// One line per item that could not be evaluated. Unknown is reported,
    /// never passed.
    pub(crate) fn unknown_lines(&self) -> Vec<String> {
        self.items
            .iter()
            .filter_map(|item| match &item.state {
                ItemState::Unknown(reason) => Some(format!(
                    "protocol: {} — unknown {} (layer: {}; could not evaluate: {})",
                    self.protocol,
                    item.kind.missing_text(),
                    item.layer,
                    reason
                )),
                _ => None,
            })
            .collect()
    }
}

/// `docs/spikes/<YYYY-MM-DD>-<slug>.md`, directly under `docs/spikes/`.
fn is_spike_deliverable(path: &str) -> bool {
    let Some(name) = path.strip_prefix("docs/spikes/") else {
        return false;
    };
    if name.contains('/') || !name.ends_with(".md") {
        return false;
    }
    let b = name.as_bytes();
    b.len() > "YYYY-MM-DD-.md".len()
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
        && b[10] == b'-'
}

/// A test file in any of the languages the criteria tracer scans. Extends the
/// stale-check's `is_test_path` (shared) with the common non-Rust shapes.
fn is_test_file(path: &str) -> bool {
    if crate::is_test_path(path) {
        return true;
    }
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    name == "tests.rs"
        || lower.starts_with("test/")
        || lower.contains("/test/")
        || (name.starts_with("test_") && name.ends_with(".py"))
        || name.ends_with("_test.go")
        || name.contains(".test.")
        || name.contains(".spec.")
}

fn diff_item(changed: &ChangedFiles, is_hit: fn(&str) -> bool, none_detail: &str) -> ItemState {
    match changed {
        ChangedFiles::Unknown(reason) => ItemState::Unknown(reason.clone()),
        ChangedFiles::Known(diff) => match diff.files.iter().find(|f| is_hit(f)) {
            Some(hit) => ItemState::Met(hit.clone()),
            None => ItemState::Missing(none_detail.to_string()),
        },
    }
}

/// A test-file change, or a source file whose diff hunks touch inline tests.
fn bug_test_item(changed: &ChangedFiles) -> ItemState {
    match diff_item(changed, is_test_file, "no test file in the diff") {
        ItemState::Missing(detail) => match changed {
            ChangedFiles::Known(diff) => match diff.test_hunk_files.first() {
                Some(file) => ItemState::Met(format!("{file} (inline #[cfg(test)] change)")),
                None => ItemState::Missing(detail),
            },
            ChangedFiles::Unknown(_) => ItemState::Missing(detail),
        },
        other => other,
    }
}

/// Pure scan of a `git diff -U0` patch: the files whose added/removed lines or
/// hunk headers mention inline Rust test code.
// trace:TASK-1277 | ai:claude
pub(crate) fn files_with_test_hunks(patch: &str) -> Vec<String> {
    const MARKERS: &[&str] = &["#[cfg(test)]", "#[test]", "mod tests"];
    let mut out: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            current = Some(path.to_string());
            continue;
        }
        if line.starts_with("+++ ") || line.starts_with("--- ") {
            continue;
        }
        let relevant = line.starts_with("@@") || line.starts_with('+') || line.starts_with('-');
        if relevant && MARKERS.iter().any(|m| line.contains(m)) {
            if let Some(path) = &current {
                if !out.contains(path) {
                    out.push(path.clone());
                }
            }
        }
    }
    out
}

/// The status an ADR is judged by. At `queue done` a leased ADR reads In
/// Progress (the lease flipped it), so the pre-lease status is recovered from
/// the spec's recorded history; with no such record the item is UNKNOWN
/// rather than a guaranteed false "not accepted".
// trace:TASK-1277 | ai:claude
fn adr_judged_status(req: &Requirement) -> Result<RequirementStatus, String> {
    if !matches!(
        req.status,
        RequirementStatus::InProgress | RequirementStatus::NeedsAttention
    ) {
        return Ok(req.status.clone());
    }
    req.history
        .iter()
        .rev()
        .flat_map(|entry| entry.changes.iter().rev())
        .filter(|c| c.field_name.eq_ignore_ascii_case("status"))
        .find_map(|c| {
            let new = RequirementStatus::from_filter_str(&c.new_value)?;
            let old = RequirementStatus::from_filter_str(&c.old_value)?;
            (new == RequirementStatus::InProgress
                && !matches!(
                    old,
                    RequirementStatus::InProgress | RequirementStatus::NeedsAttention
                ))
            .then_some(old)
        })
        .ok_or_else(|| {
            format!(
                "ADR is {} under its lease and no pre-lease status is recorded",
                req.status
            )
        })
}

/// Evaluate every machine-checkable item for `req`'s type. `changed` is only
/// consulted by the diff-based items.
// trace:TASK-1277 | ai:claude
pub(crate) fn evaluate_items(req: &Requirement, changed: &ChangedFiles) -> ProtocolReport {
    let (protocol, kinds) = machine_items(&req.req_type);
    let items = kinds
        .iter()
        .map(|&kind| {
            let state = match kind {
                ItemKind::SpikeDeliverable => diff_item(
                    changed,
                    is_spike_deliverable,
                    "no docs/spikes/<date>-*.md file in the diff",
                ),
                ItemKind::BugTestChange => bug_test_item(changed),
                ItemKind::AdrAcceptedWithReferences => {
                    let referenced = req
                        .relationships
                        .iter()
                        .any(|r| r.rel_type == RelationshipType::References);
                    match adr_judged_status(req) {
                        Err(reason) => ItemState::Unknown(reason),
                        Ok(status) => {
                            let accepted = matches!(
                                status,
                                RequirementStatus::Approved
                                    | RequirementStatus::Done
                                    | RequirementStatus::Completed
                            );
                            match (accepted, referenced) {
                                (true, true) => {
                                    ItemState::Met("accepted, references edge present".into())
                                }
                                (false, true) => {
                                    ItemState::Missing(format!("status is {status}, not accepted"))
                                }
                                (true, false) => ItemState::Missing("no references edge".into()),
                                (false, false) => ItemState::Missing(format!(
                                    "status is {status}, not accepted; no references edge"
                                )),
                            }
                        }
                    }
                }
            };
            ItemResult {
                kind,
                layer: LAYER_TYPE,
                state,
            }
        })
        .collect();
    ProtocolReport { protocol, items }
}

/// Resolve the branch's changed files against the default branch. Returns
/// `Unknown` (never an empty `Known`) when the comparison is impossible, so
/// a diff-based item cannot be marked missing — or met — on a guess.
// trace:TASK-1277 | ai:claude
pub(crate) fn changed_files_for_branch(repo: &Path) -> ChangedFiles {
    let Some(base) = crate::resolve_default_branch_ref(repo) else {
        return ChangedFiles::Unknown("no default branch ref could be resolved".into());
    };
    let branch = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let base_name = base.strip_prefix("origin/").unwrap_or(&base);
    match branch.as_deref() {
        None => return ChangedFiles::Unknown("current branch could not be read".into()),
        Some(b) if b == "HEAD" || b == base_name => {
            return ChangedFiles::Unknown(format!(
                "on the default branch ({base_name}); no branch diff to inspect"
            ));
        }
        _ => {}
    }
    let files = match crate::harvest::changed_files_base_to_head(repo, &base) {
        Ok(files) => files,
        Err(e) => return ChangedFiles::Unknown(format!("{e:#}")),
    };
    // Cheap inline-test grep over the Rust hunks; a failure here only loses
    // the inline-test signal, never the file list.
    let test_hunk_files =
        crate::harvest::git_diff_base_to_head(repo, &base, &["-U0", "--", "*.rs"])
            .map(|patch| files_with_test_hunks(&patch))
            .unwrap_or_default();
    ChangedFiles::Known(BranchDiff {
        files,
        test_hunk_files,
    })
}

/// Evaluate `req` against the repo at `repo`, reading the diff only when one
/// of the type's items needs it. Empty report for types without items.
// trace:TASK-1277 | ai:claude
pub(crate) fn evaluate_for_repo(req: &Requirement, repo: &Path) -> ProtocolReport {
    let (_, kinds) = machine_items(&req.req_type);
    let changed = if kinds.iter().any(|k| k.needs_diff()) {
        changed_files_for_branch(repo)
    } else {
        ChangedFiles::Known(BranchDiff::default())
    };
    evaluate_items(req, &changed)
}

/// The done/PR-creation decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GateOutcome {
    /// Nothing missing. `unknown` lines (if any) are still printed.
    Proceed { unknown: Vec<String> },
    /// Missing items under `enforce = warn`, or under `refuse` with `--force`.
    Warn {
        missing: Vec<String>,
        unknown: Vec<String>,
        forced: bool,
    },
    /// Missing items under `enforce = refuse` without `--force`.
    Refuse {
        missing: Vec<String>,
        unknown: Vec<String>,
    },
}

/// Pure policy: the warn / refuse / force matrix.
// trace:TASK-1277 | ai:claude
pub(crate) fn decide(report: &ProtocolReport, enforce: EnforceMode, force: bool) -> GateOutcome {
    let missing = report.missing_lines();
    let unknown = report.unknown_lines();
    if missing.is_empty() {
        return GateOutcome::Proceed { unknown };
    }
    match (enforce, force) {
        (EnforceMode::Warn, _) => GateOutcome::Warn {
            missing,
            unknown,
            forced: false,
        },
        (EnforceMode::Refuse, true) => GateOutcome::Warn {
            missing,
            unknown,
            forced: true,
        },
        (EnforceMode::Refuse, false) => GateOutcome::Refuse { missing, unknown },
    }
}

/// Ledger comment recording the missing items. `forced_by` names who forced
/// a `refuse` override; `None` is the plain `warn` record.
// trace:TASK-1277 | ai:claude
pub(crate) fn ledger_comment(
    surface: &str,
    display_id: &str,
    missing: &[String],
    forced_by: Option<&str>,
) -> String {
    let mut out = match forced_by {
        Some(who) => format!(
            "{FORCE_LEDGER_MARKER} at `{surface}` by {who}: {display_id} proceeded with missing \
             protocol items although [protocol] enforce = \"refuse\" (why: operator passed \
             --force to accept the gap).\n"
        ),
        None => format!(
            "protocol check at `{surface}` ([protocol] enforce = \"warn\"): {display_id} \
             proceeded with missing protocol items:\n"
        ),
    };
    for line in missing {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// True when an identical protocol ledger comment is already on the spec, so
/// a drain retry or rework round re-running `queue done` records it once.
// trace:TASK-1277 | ai:claude
pub(crate) fn ledger_already_recorded(req: &Requirement, body: &str) -> bool {
    req.comments.iter().any(|c| c.content == body)
}

/// True when the spec already carries a `--force` protocol override ledger —
/// PR creation honours an override made at `aida queue done`.
pub(crate) fn has_force_override(req: &Requirement) -> bool {
    req.comments
        .iter()
        .any(|c| c.content.contains(FORCE_LEDGER_MARKER))
}

/// The PR-body section: every item for every covered spec, under a
/// `## Protocol` heading. `None` when no covered spec has a machine-checkable
/// item.
// trace:TASK-1277 | ai:claude
pub(crate) fn pr_body_section(reports: &[(String, ProtocolReport)]) -> Option<String> {
    let mut lines = Vec::new();
    for (display, report) in reports {
        for item in &report.items {
            let (mark, detail) = match &item.state {
                ItemState::Met(d) => ("met", d.as_str()),
                ItemState::Missing(d) => ("MISSING", d.as_str()),
                ItemState::Unknown(d) => ("UNKNOWN", d.as_str()),
            };
            lines.push(format!(
                "- {display} ({} protocol, {} layer): {} — **{mark}** ({detail})",
                report.protocol,
                item.layer,
                item.kind.checklist_text()
            ));
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!("## Protocol\n\n{}\n", lines.join("\n")))
}

/// The reviewer-prompt block: each covered spec's machine-checkable
/// checklist items, verbatim, so review rounds cite them by name.
// trace:TASK-1277 | ai:claude
pub(crate) fn reviewer_prompt_block(specs: &[(String, RequirementType)]) -> Option<String> {
    let mut lines = Vec::new();
    for (display, req_type) in specs {
        let (protocol, kinds) = machine_items(req_type);
        for kind in kinds {
            lines.push(format!(
                "- {display} ({protocol} protocol): {}",
                kind.checklist_text()
            ));
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "\n\nType protocol checklist (machine-checkable items; cite each by name in your \
         verdict as met, missing, or unknown):\n{}\n",
        lines.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::Relationship;

    fn req(kind: RequirementType, status: RequirementStatus) -> Requirement {
        let mut r = Requirement::new("t".into(), "d".into());
        r.req_type = kind;
        r.status = status;
        r
    }

    fn known(files: &[&str]) -> ChangedFiles {
        ChangedFiles::Known(BranchDiff {
            files: files.iter().map(|s| s.to_string()).collect(),
            test_hunk_files: Vec::new(),
        })
    }

    #[test]
    fn spike_deliverable_missing_prints_the_named_line() {
        let r = req(RequirementType::Spike, RequirementStatus::InProgress);
        let report = evaluate_items(&r, &known(&["src/lib.rs", "docs/spikes/notes.md"]));
        let missing = report.missing_lines();
        assert_eq!(missing.len(), 1);
        assert!(missing[0]
            .starts_with("protocol: spike — missing deliverable docs/spikes/<date>-<slug>.md"));
    }

    #[test]
    fn spike_deliverable_met_by_dated_report() {
        let r = req(RequirementType::Spike, RequirementStatus::InProgress);
        let report = evaluate_items(&r, &known(&["docs/spikes/2026-09-23-cache-probe.md"]));
        assert!(matches!(report.items[0].state, ItemState::Met(_)));
        assert!(report.missing_lines().is_empty());
        assert_eq!(report.items[0].layer, "type");
    }

    #[test]
    fn bug_without_test_change_is_missing_and_with_one_is_met() {
        let r = req(RequirementType::Bug, RequirementStatus::InProgress);
        let miss = evaluate_items(&r, &known(&["aida-cli-lib/src/queue_cmd.rs"]));
        assert_eq!(
            miss.missing_lines()[0].split(" (").next().unwrap(),
            "protocol: bug — missing test-file change"
        );
        for file in [
            "aida-cli/tests/bug_1_x.rs",
            "aida-cli-lib/src/tests/foo_tests.rs",
            "py/test_widget.py",
            "web/src/app.test.ts",
        ] {
            let met = evaluate_items(&r, &known(&[file]));
            assert!(
                matches!(met.items[0].state, ItemState::Met(_)),
                "{file} should count as a test file"
            );
        }
    }

    #[test]
    fn adr_needs_accepted_status_and_references_edge() {
        let mut r = req(RequirementType::Decision, RequirementStatus::Draft);
        let report = evaluate_items(&r, &known(&[]));
        assert!(report.missing_lines()[0].contains("not accepted; no references edge"));
        r.status = RequirementStatus::Approved;
        assert!(evaluate_items(&r, &known(&[])).missing_lines()[0].contains("no references edge"));
        r.relationships.push(Relationship {
            rel_type: RelationshipType::References,
            target_id: uuid::Uuid::new_v4(),
            created_at: None,
            created_by: None,
        });
        let met = evaluate_items(&r, &known(&[]));
        assert!(matches!(met.items[0].state, ItemState::Met(_)));
    }

    fn status_change(old: &str, new: &str) -> aida_core::HistoryEntry {
        aida_core::HistoryEntry::new(
            "joe".into(),
            vec![aida_core::FieldChange {
                field_name: "status".into(),
                old_value: old.into(),
                new_value: new.into(),
            }],
        )
    }

    #[test]
    fn leased_adr_is_judged_by_pre_lease_status_or_unknown() {
        let mut r = req(RequirementType::Decision, RequirementStatus::InProgress);
        r.relationships.push(Relationship {
            rel_type: RelationshipType::References,
            target_id: uuid::Uuid::new_v4(),
            created_at: None,
            created_by: None,
        });
        // No recorded pre-lease status: UNKNOWN, never a false "not accepted".
        let report = evaluate_items(&r, &known(&[]));
        assert!(matches!(report.items[0].state, ItemState::Unknown(_)));
        assert!(report.missing_lines().is_empty());
        // Accepted before the lease flipped it In Progress: met.
        r.history.push(status_change("Draft", "Approved"));
        r.history.push(status_change("Approved", "In Progress"));
        let met = evaluate_items(&r, &known(&[]));
        assert!(matches!(met.items[0].state, ItemState::Met(_)), "{met:?}");
        // Leased straight from Draft: missing, judged by the Draft status.
        r.history.clear();
        r.history.push(status_change("Draft", "In Progress"));
        let miss = evaluate_items(&r, &known(&[]));
        assert!(miss.missing_lines()[0].contains("status is Draft, not accepted"));
    }

    #[test]
    fn warn_ledger_is_recorded_once_across_reruns() {
        let missing = vec!["protocol: bug — missing test-file change".to_string()];
        let body = ledger_comment("aida queue done", "BUG-7", &missing, None);
        let mut r = req(RequirementType::Bug, RequirementStatus::InProgress);
        assert!(!ledger_already_recorded(&r, &body));
        r.add_comment(aida_core::Comment::new("joe".into(), body.clone()));
        // A drain retry / rework round produces the identical body: skipped.
        let rerun = ledger_comment("aida queue done", "BUG-7", &missing, None);
        assert!(ledger_already_recorded(&r, &rerun));
        // A different gap is a new record.
        let other = ledger_comment("aida queue done", "BUG-7", &["x".to_string()], None);
        assert!(!ledger_already_recorded(&r, &other));
    }

    #[test]
    fn bug_test_item_counts_tests_rs_and_inline_cfg_test_hunks() {
        let r = req(RequirementType::Bug, RequirementStatus::InProgress);
        let met = evaluate_items(&r, &known(&["aida-core/src/db/tests.rs"]));
        assert!(matches!(met.items[0].state, ItemState::Met(_)));

        let patch = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                     @@ -10,0 +11,2 @@ fn helper() {\n+    let x = 1;\n\
                     diff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n\
                     @@ -40,0 +41,4 @@ mod tests {\n+    #[test]\n+    fn regress() {}\n";
        assert_eq!(files_with_test_hunks(patch), vec!["src/b.rs".to_string()]);

        let inline = ChangedFiles::Known(BranchDiff {
            files: vec!["src/a.rs".into(), "src/b.rs".into()],
            test_hunk_files: files_with_test_hunks(patch),
        });
        let report = evaluate_items(&r, &inline);
        match &report.items[0].state {
            ItemState::Met(detail) => assert!(detail.contains("src/b.rs")),
            other => panic!("inline test change should count: {other:?}"),
        }
    }

    #[test]
    fn unreadable_diff_is_unknown_not_met_and_never_refuses() {
        let r = req(RequirementType::Spike, RequirementStatus::InProgress);
        let report = evaluate_items(&r, &ChangedFiles::Unknown("git failed".into()));
        assert!(matches!(report.items[0].state, ItemState::Unknown(_)));
        assert!(report.missing_lines().is_empty());
        assert!(report.unknown_lines()[0].contains("could not evaluate: git failed"));
        match decide(&report, EnforceMode::Refuse, false) {
            GateOutcome::Proceed { unknown } => assert_eq!(unknown.len(), 1),
            other => panic!("unknown must not refuse or pass silently: {other:?}"),
        }
    }

    #[test]
    fn types_without_items_are_silent() {
        let r = req(RequirementType::Task, RequirementStatus::InProgress);
        let report = evaluate_items(&r, &known(&[]));
        assert!(report.is_empty());
        assert_eq!(
            decide(&report, EnforceMode::Refuse, false),
            GateOutcome::Proceed { unknown: vec![] }
        );
        assert_eq!(pr_body_section(&[("TASK-1".into(), report)]), None);
    }

    #[test]
    fn warn_refuse_force_matrix() {
        let r = req(RequirementType::Spike, RequirementStatus::InProgress);
        let report = evaluate_items(&r, &known(&[]));
        assert!(matches!(
            decide(&report, EnforceMode::Warn, false),
            GateOutcome::Warn { forced: false, .. }
        ));
        assert!(matches!(
            decide(&report, EnforceMode::Warn, true),
            GateOutcome::Warn { forced: false, .. }
        ));
        assert!(matches!(
            decide(&report, EnforceMode::Refuse, false),
            GateOutcome::Refuse { .. }
        ));
        assert!(matches!(
            decide(&report, EnforceMode::Refuse, true),
            GateOutcome::Warn { forced: true, .. }
        ));
    }

    #[test]
    fn force_ledger_names_who_and_is_detectable_at_pr_create() {
        let missing = vec!["protocol: spike — missing deliverable x".to_string()];
        let comment = ledger_comment("aida queue done", "SPIKE-9", &missing, Some("joe"));
        assert!(comment.contains(FORCE_LEDGER_MARKER));
        assert!(comment.contains("by joe"));
        assert!(comment.contains("--force"));
        assert!(comment.contains("missing deliverable x"));
        let mut r = req(RequirementType::Spike, RequirementStatus::Done);
        assert!(!has_force_override(&r));
        r.add_comment(aida_core::Comment::new("joe".into(), comment));
        assert!(has_force_override(&r));
        let warn = ledger_comment("aida queue done", "SPIKE-9", &missing, None);
        assert!(!warn.contains(FORCE_LEDGER_MARKER));
    }

    #[test]
    fn pr_body_has_protocol_heading_with_each_item_state() {
        let spike = req(RequirementType::Spike, RequirementStatus::InProgress);
        let bug = req(RequirementType::Bug, RequirementStatus::InProgress);
        let section = pr_body_section(&[
            ("SPIKE-1".into(), evaluate_items(&spike, &known(&[]))),
            (
                "BUG-2".into(),
                evaluate_items(&bug, &ChangedFiles::Unknown("no base".into())),
            ),
        ])
        .unwrap();
        assert!(section.starts_with("## Protocol\n"));
        assert!(section.contains("SPIKE-1 (spike protocol, type layer)"));
        assert!(section.contains("**MISSING**"));
        assert!(section.contains("BUG-2 (bug protocol, type layer)"));
        assert!(section.contains("**UNKNOWN**"));
    }

    #[test]
    fn reviewer_block_carries_checklist_items_verbatim() {
        let block = reviewer_prompt_block(&[
            ("SPIKE-1".into(), RequirementType::Spike),
            ("TASK-2".into(), RequirementType::Task),
            ("ADR-3".into(), RequirementType::Decision),
        ])
        .unwrap();
        assert!(block.contains(ItemKind::SpikeDeliverable.checklist_text()));
        assert!(block.contains(ItemKind::AdrAcceptedWithReferences.checklist_text()));
        assert!(!block.contains("TASK-2"));
        assert_eq!(
            reviewer_prompt_block(&[("TASK-2".into(), RequirementType::Task)]),
            None
        );
    }

    #[test]
    fn spike_deliverable_path_shape() {
        assert!(is_spike_deliverable("docs/spikes/2026-01-02-x.md"));
        assert!(!is_spike_deliverable("docs/spikes/x.md"));
        assert!(!is_spike_deliverable("docs/spikes/2026-01-02-x.txt"));
        assert!(!is_spike_deliverable("docs/spikes/sub/2026-01-02-x.md"));
        assert!(!is_spike_deliverable("other/docs/spikes/2026-01-02-x.md"));
    }
}
