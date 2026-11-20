// trace:TASK-3780 | ai:codex
use super::*;
use std::path::{Path, PathBuf};

fn untrusted() -> Vec<DoctorFinding> {
    worktree_trust_findings(Path::new("/home/op/wt"), true, Some(&[]), None)
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac891c92 | ai:claude
#[test]
fn off_names_the_opt_in_key_in_the_untrusted_finding() {
    let mut findings = untrusted();
    seat_trust_findings(&mut findings, false, &[]);
    assert_eq!(findings.len(), 1);
    assert!(findings[0].action.contains("trust_seat_worktrees = true"));
    assert!(findings[0]
        .summary
        .contains("a headless lane there ignores the project's permissions.allow list"));
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac891c92 | ai:claude
#[test]
fn on_reports_pre_trust_as_on_and_drops_the_headless_claim() {
    let mut findings = untrusted();
    seat_trust_findings(&mut findings, true, &[]);
    assert_eq!(findings.len(), 2);
    assert!(!findings[0].action.contains("trust_seat_worktrees = true"));
    assert!(!findings[0].summary.contains("ignores the project's"));
    assert_eq!(findings[1].id, "worktree-seat-pre-trust");
    assert!(findings[1].summary.contains("per-seat pre-trust is on"));
}

/// Ruling 3: on is reported even when the worktrees parent is already
/// trusted (no folder-trust finding to annotate); off reports nothing.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac891c92 | ai:claude
#[test]
fn on_is_reported_when_the_parent_is_already_trusted() {
    let mut findings = Vec::new();
    seat_trust_findings(&mut findings, true, &[]);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "worktree-seat-pre-trust");
    assert!(findings[0]
        .summary
        .contains("review trees are never trusted"));
    let mut off = Vec::new();
    seat_trust_findings(&mut off, false, &[]);
    assert!(off.is_empty());
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac891c92 | ai:claude
#[test]
fn stale_entries_are_reported() {
    let mut findings = Vec::new();
    seat_trust_findings(
        &mut findings,
        true,
        &[PathBuf::from("/home/op/wt/aida-seat-x")],
    );
    let stale = findings
        .iter()
        .find(|f| f.id == "worktree-seat-trust-stale")
        .unwrap();
    assert!(stale.summary.contains("1 Claude Code trust entry"));
    assert!(stale.action.contains("prunes them automatically"));
    assert!(!stale.safe_heal);
    let mut off = Vec::new();
    seat_trust_findings(&mut off, false, &[PathBuf::from("/home/op/wt/aida-seat-x")]);
    assert!(off[0].action.contains("by hand"));
}
