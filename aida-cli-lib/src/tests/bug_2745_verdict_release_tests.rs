//! BUG-2745: a supervised-by-mode PR whose rework hold the gate ledger
//! records as auto-released by a reviewer verdict is a recorded clearance,
//! not a vanished marker. The train used to refuse it as "missing without a
//! recorded clearance (tampering suspected)".
// trace:BUG-2745 | ai:claude

use crate::gate::ledger::hold;
use crate::gate::mandate::{HoldFacts, MandateRefusal};
use crate::gate::mandate_ship::unheld_supervised_facts;
use crate::gate::merge_hold_core::{
    clearance_for, read_hold, typed_hold, HoldReasonKind, VerdictRef,
};
use crate::gate::rework_hold_release::PRINCIPAL;
use crate::merge_hold::{ChangeState, PinnedChange};
use std::path::{Path, PathBuf};

const PR: u64 = 4487;
const HEAD: &str = "05a17b1e0000000000000000000000000000beef";

fn uncontained() -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)])
}

fn labels(hold: bool, recorded: bool, cleared: bool) -> PinnedChange {
    PinnedChange {
        merged: false,
        state: ChangeState::Open,
        head_sha: Some(HEAD.into()),
        hold_label: hold,
        recorded_label: recorded,
        cleared_label: cleared,
    }
}

fn git(repo: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A primary checkout and a linked reviewer worktree sharing its ledger.
fn primary_and_reviewer() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let primary = base.join("aida");
    std::fs::create_dir_all(&primary).unwrap();
    git(&primary, &["init", "--initial-branch=main", "--quiet"]);
    git(&primary, &["config", "user.email", "t@example.com"]);
    git(&primary, &["config", "user.name", "Test"]);
    std::fs::write(primary.join("README.md"), "init\n").unwrap();
    git(&primary, &["add", "."]);
    git(&primary, &["commit", "-q", "-m", "chore: init"]);
    let reviewer = base.join("aida-review-4487");
    git(
        &primary,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "review",
            reviewer.to_str().unwrap(),
        ],
    );
    (tmp, primary, reviewer)
}

/// Place a `kind` hold and release it as `principal` from `writer`.
fn place_and_release(writer: &Path, kind: HoldReasonKind, principal: &str, verdict: bool) {
    let record = typed_hold(PR, kind, "reviewer refused", Some(HEAD.into()));
    hold::place(writer, &record, false).unwrap();
    let verdict = verdict.then(|| {
        VerdictRef::new(
            "BUG-2742",
            Some(PR),
            Some(HEAD.into()),
            Some("reviewer".into()),
        )
    });
    let clearance = clearance_for(&record, principal, verdict, None).unwrap();
    hold::release(writer, PR, clearance).unwrap();
}

fn facts(root: &Path, change: PinnedChange) -> Result<HoldFacts, MandateRefusal> {
    let reason = read_hold(root, PR);
    unheld_supervised_facts(root, PR, reason.as_deref(), || Ok(Some(change)))
}

/// AC1: a rework hold auto-released from a reviewer worktree is a recorded
/// clearance on the primary checkout's ledger; the train admits the PR as
/// supervised-by-mode (the mandate and approval checks still apply).
#[test]
fn a_reviewer_worktree_verdict_auto_release_counts_as_a_recorded_clearance() {
    let _env = uncontained();
    let (_tmp, primary, reviewer) = primary_and_reviewer();
    place_and_release(&reviewer, HoldReasonKind::Rework, PRINCIPAL, true);

    assert_eq!(
        read_hold(&primary, PR),
        None,
        "no open or missing-marker hold"
    );
    assert_eq!(
        facts(&primary, labels(false, true, true)),
        Ok(HoldFacts::SupervisedByMode)
    );
    // Labels that have not caught up with the hold at all agree too.
    assert_eq!(
        facts(&primary, labels(false, false, false)),
        Ok(HoldFacts::SupervisedByMode)
    );
}

/// The forge must agree the hold is cleared, and the PR must be open.
#[test]
fn a_verdict_auto_release_still_needs_agreeing_labels() {
    let _env = uncontained();
    let (_tmp, primary, reviewer) = primary_and_reviewer();
    place_and_release(&reviewer, HoldReasonKind::Rework, PRINCIPAL, true);

    for change in [labels(true, true, true), labels(false, true, false)] {
        assert_eq!(facts(&primary, change), Err(MandateRefusal::MarkerMissing));
    }
    let mut closed = labels(false, true, true);
    closed.state = ChangeState::ClosedUnmerged;
    assert_eq!(facts(&primary, closed), Err(MandateRefusal::MarkerMissing));
    assert!(matches!(
        unheld_supervised_facts(&primary, PR, None, || Err("forge down".into())),
        Err(MandateRefusal::LabelsUnreadable(_))
    ));
}

/// Only a verdict auto-release of a rework hold qualifies: another kind,
/// another principal or a release without its verdict stays refused.
#[test]
fn other_recorded_releases_stay_refused() {
    let _env = uncontained();
    for (kind, principal, verdict) in [
        (HoldReasonKind::Supervision, PRINCIPAL, true),
        (HoldReasonKind::Rework, "agent:someone", true),
        (HoldReasonKind::Rework, PRINCIPAL, false),
    ] {
        let (_tmp, primary, _reviewer) = primary_and_reviewer();
        place_and_release(&primary, kind, principal, verdict);
        assert_eq!(
            facts(&primary, labels(false, true, true)),
            Err(MandateRefusal::MarkerMissing),
            "{kind:?} released by {principal} (verdict: {verdict})"
        );
    }

    // A clearance that claims to release a rework hold over a supervision
    // hold is judged by the hold the ledger released, not by its claim.
    let (_tmp, primary, _reviewer) = primary_and_reviewer();
    let held = typed_hold(PR, HoldReasonKind::Supervision, "supervised", None);
    hold::place(&primary, &held, false).unwrap();
    let claimed = typed_hold(PR, HoldReasonKind::Rework, "supervised", None);
    let verdict = VerdictRef::new("BUG-2742", Some(PR), Some(HEAD.into()), None);
    let forged = clearance_for(&claimed, PRINCIPAL, Some(verdict), None).unwrap();
    hold::release(&primary, PR, forged).unwrap();
    assert_eq!(
        facts(&primary, labels(false, true, true)),
        Err(MandateRefusal::MarkerMissing)
    );
}

/// A missing-marker reason refuses even beside a verdict release, and a PR
/// never held needs clean labels (BUG-2663 unchanged).
#[test]
fn missing_marker_and_never_held_shapes_are_unchanged() {
    let _env = uncontained();
    let (_tmp, primary, reviewer) = primary_and_reviewer();
    assert_eq!(
        facts(&primary, labels(false, false, false)),
        Ok(HoldFacts::SupervisedByMode)
    );
    assert_eq!(
        facts(&primary, labels(false, true, true)),
        Err(MandateRefusal::MarkerMissing)
    );
    place_and_release(&reviewer, HoldReasonKind::Rework, PRINCIPAL, true);
    assert_eq!(
        unheld_supervised_facts(
            &primary,
            PR,
            Some(crate::gate::merge_hold_core::MARKER_MISSING_REASON),
            || Ok(Some(labels(false, true, true)))
        ),
        Err(MandateRefusal::MarkerMissing)
    );
}
