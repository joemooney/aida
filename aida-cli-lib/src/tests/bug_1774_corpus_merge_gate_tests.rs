//! BUG-1774: the ENFORCEMENT half of BUG-1705 — `forge::merge_change` refuses
//! a PR with an outstanding blocking verdict at its current head, whatever
//! producer wrote the artifact, and the marker + label mirrors are armed from
//! the gate's own predicate result.
//!
//! The decision core (`merge_hold::corpus_merge_gate`) is pure over
//! `(key, body)` pairs and a head sha; the enforcement wrapper and the
//! pure-git chokepoint are driven against real temp filesystems/repos.
// trace:BUG-1774 | ai:claude

use crate::merge_hold::{self, CorpusMergeGate};

const HEAD: &str = "9cd65761d7aa4f3b2c1e0d9a8b7c6d5e4f3a2b1c";
const OLD: &str = "1111111111111111111111111111111111111111";

fn body_by(verdict: &str, sha: &str, recorded_by: &str, closed: Option<&str>) -> String {
    let closed = match closed {
        Some(sha) => format!(r#","closed_by_merge":"{sha}""#),
        None => String::new(),
    };
    format!(
        r#"{{"verdict":"{verdict}","reviewed_sha":"{sha}","recorded_by":"{recorded_by}","findings":["f1"]{closed}}}"#
    )
}

fn body(verdict: &str, sha: &str, closed: Option<&str>) -> String {
    body_by(verdict, sha, "reviewer-a", closed)
}

fn keyed(key: &str, body: String) -> (String, String) {
    (key.to_string(), body)
}

fn refusal_of(gate: CorpusMergeGate) -> (String, Option<merge_hold::CorpusArm>) {
    match gate {
        CorpusMergeGate::Refuse { message, arm } => (message, arm),
        CorpusMergeGate::Clear => panic!("expected the gate to refuse"),
    }
}

// ── the pure decision ────────────────────────────────────────────────────────

// No verdict evidence at all: the gate stays out of the way (a PR merged
// without delegated review is governed by the other merge gates).
#[test]
fn no_artifacts_is_clear() {
    assert_eq!(
        merge_hold::corpus_merge_gate(7, &[], Some(HEAD)),
        CorpusMergeGate::Clear
    );
}

// AC1 — a blocking verdict at the PR's exact current head refuses, and the
// refusal carries the definite arm the mirrors are refreshed from (AC4: the
// arm's sha IS the head the gate judged, not a second derivation).
#[test]
fn a_refusal_at_the_current_head_refuses_and_arms() {
    let (message, arm) = refusal_of(merge_hold::corpus_merge_gate(
        7,
        &[keyed("BUG-9", body("request-changes", HEAD, None))],
        Some(HEAD),
    ));
    let arm = arm.expect("a definite at-head blocker must arm the mirrors");
    assert_eq!(arm.sha, HEAD);
    assert_eq!(arm.key, "BUG-9");
    assert_eq!(arm.verdict_raw, "request-changes");
    assert_eq!(arm.recorded_by.as_deref(), Some("reviewer-a"));
    // The refusal speaks the caller's release syntax, not `merge-hold clear`
    // (clearing a marker cannot release a corpus-derived hold).
    assert!(message.contains("aida review record"), "{message}");
    assert!(!message.contains("merge-hold clear"), "{message}");
}

// AC3 — a head move alone does not lift the hold: the refusal now sits at an
// older sha, nothing approves the new head, so the gate still refuses (with no
// arm — there is no definite blocker AT this head to mirror).
#[test]
fn a_head_move_alone_does_not_lift_the_hold() {
    let (message, arm) = refusal_of(merge_hold::corpus_merge_gate(
        7,
        &[keyed("BUG-9", body("request-changes", OLD, None))],
        Some(HEAD),
    ));
    assert!(
        arm.is_none(),
        "a stale refusal must not arm a mirror at the new head"
    );
    assert!(message.contains("head move alone"), "{message}");
}

// AC3 — an `approved` recorded at the OLD rejected sha does not lift it either.
#[test]
fn an_approval_at_the_old_rejected_sha_does_not_lift_the_hold() {
    let (_, arm) = refusal_of(merge_hold::corpus_merge_gate(
        7,
        &[
            keyed("BUG-9", body("request-changes", OLD, None)),
            keyed("PR-7", body_by("approved", OLD, "reviewer-b", None)),
        ],
        Some(HEAD),
    ));
    assert!(arm.is_none());
}

// AC3 — the one release: the head moved past the rejected sha AND a later
// `approved` verdict is recorded at the NEW head.
#[test]
fn an_approval_at_the_new_head_after_a_move_releases() {
    assert_eq!(
        merge_hold::corpus_merge_gate(
            7,
            &[
                keyed("BUG-9", body("request-changes", OLD, None)),
                keyed("PR-7", body_by("approved", HEAD, "reviewer-b", None)),
            ],
            Some(HEAD),
        ),
        CorpusMergeGate::Clear
    );
}

// BUG-1529: a verdict closed by a merge is history, not evidence about this
// head — a follow-up PR of the same spec is not refused by it.
#[test]
fn a_closed_refusal_is_history_and_clears() {
    assert_eq!(
        merge_hold::corpus_merge_gate(
            7,
            &[keyed("BUG-9", body("request-changes", OLD, Some(OLD)))],
            Some(HEAD),
        ),
        CorpusMergeGate::Clear
    );
}

// AC2 — the gate fails CLOSED on an unknown head once an artifact exists (the
// BUG-1773 view fails OPEN on the same input; the two must not collapse).
#[test]
fn an_unknown_head_fails_closed_with_an_artifact_on_file() {
    let (message, arm) = refusal_of(merge_hold::corpus_merge_gate(
        7,
        &[keyed("BUG-9", body("request-changes", HEAD, None))],
        None,
    ));
    assert!(
        arm.is_none(),
        "an unknown head is uncertainty, not a mirrorable hold"
    );
    assert!(message.contains("fails closed"), "{message}");
}

// Two reviewers disagreeing at one commit is an integrity state a human must
// resolve — the gate refuses rather than picking a winner.
#[test]
fn a_conflicting_corpus_at_the_head_refuses() {
    let (message, arm) = refusal_of(merge_hold::corpus_merge_gate(
        7,
        &[
            keyed(
                "BUG-9",
                body_by("request-changes", HEAD, "reviewer-a", None),
            ),
            keyed("PR-7", body_by("approved", HEAD, "reviewer-b", None)),
        ],
        Some(HEAD),
    ));
    assert!(arm.is_none());
    assert!(message.contains("cannot be reconciled"), "{message}");
}

// ── the enforcement wrapper (filesystem) ─────────────────────────────────────

fn write_artifact(root: &std::path::Path, key: &str, body: &str) {
    let path = root
        .join(".aida/review-verdicts")
        .join(format!("{key}.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

// AC1 at the wrapper: a hand-written artifact (the producer `adopt_direct_write`
// exists to accommodate) refuses the merge and arms the marker mirror with the
// gate's own evidence (AC4).
#[test]
fn enforce_refuses_and_arms_the_marker_from_the_gates_own_predicate() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_artifact(root, "PR-7", &body("request-changes", HEAD, None));
    let err = merge_hold::enforce_corpus_gate_before_merge(root, 7, &[], || Some(HEAD.to_string()))
        .expect_err("a refusal at the head must refuse the merge");
    assert!(err.to_string().contains("request-changes"), "{err:#}");
    let record = merge_hold::read_hold_record(root, 7).expect("the marker mirror must be armed");
    assert_eq!(record.reason_kind, merge_hold::HoldReasonKind::Rework);
    assert_eq!(record.target_head_sha.as_deref(), Some(HEAD));
    let verdict_ref = record.verdict_ref.expect("the hold references the verdict");
    assert_eq!(verdict_ref.key, "PR-7");
    assert_eq!(verdict_ref.reviewed_sha.as_deref(), Some(HEAD));
}

// AC6 — the programmatic Rework arm composes with `write_typed_hold`'s
// BUG-1691 precedence: it must NOT displace a standing Recusal hold.
#[test]
fn the_arm_does_not_displace_a_recusal_hold() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut recusal = merge_hold::typed_hold(
        7,
        merge_hold::HoldReasonKind::Recusal,
        "reviewer-a is recused from PR-7",
        Some(HEAD.to_string()),
    );
    recusal
        .recused_principals
        .push(merge_hold::PrincipalIdentity::registered_agent(
            "reviewer-a",
        ));
    merge_hold::write_typed_hold(root, &recusal).unwrap();
    write_artifact(root, "PR-7", &body("request-changes", HEAD, None));
    merge_hold::enforce_corpus_gate_before_merge(root, 7, &[], || Some(HEAD.to_string()))
        .expect_err("still refused");
    let record = merge_hold::read_hold_record(root, 7).expect("marker present");
    assert_eq!(
        record.reason_kind,
        merge_hold::HoldReasonKind::Recusal,
        "a programmatic Rework write must not displace a Recusal hold"
    );
    assert!(
        !record.recused_principals.is_empty(),
        "the recusal's principals survive the compose"
    );
}

// AC2 at the wrapper — an artifact that exists but cannot be read (a directory
// in its place) fails the merge closed.
#[test]
fn an_unreadable_artifact_fails_the_merge_closed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".aida/review-verdicts/PR-7.json")).unwrap();
    let err = merge_hold::enforce_corpus_gate_before_merge(root, 7, &[], || Some(HEAD.to_string()))
        .expect_err("an unreadable artifact must fail closed");
    assert!(err.to_string().contains("could not be read"), "{err:#}");
}

// A PR with no verdict artifacts pays no head resolution (the live-head read is
// a forge call on the real providers) and merges untouched.
#[test]
fn no_artifacts_skips_head_resolution_entirely() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = std::cell::Cell::new(false);
    merge_hold::enforce_corpus_gate_before_merge(dir.path(), 7, &[], || {
        resolved.set(true);
        Some(HEAD.to_string())
    })
    .expect("nothing on file, nothing refused");
    assert!(!resolved.get(), "the head must be resolved lazily");
}

// AC5-shape: a refusing verdict keyed to a spec this change does not name
// holds nothing here — the gate weighs only the PR's own keys.
#[test]
fn a_refusal_for_an_unrelated_spec_does_not_gate_this_pr() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_artifact(root, "BUG-999", &body("request-changes", HEAD, None));
    merge_hold::enforce_corpus_gate_before_merge(root, 7, &["BUG-9".to_string()], || {
        Some(HEAD.to_string())
    })
    .expect("an unrelated spec's refusal is not this PR's hold");
}

// ── the chokepoint itself ────────────────────────────────────────────────────

fn git(root: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn temp_repo_with_branch(branch: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.email", "t@t"]);
    git(root, &["config", "user.name", "t"]);
    std::fs::write(root.join("a.txt"), "base\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "base"]);
    git(root, &["checkout", "-b", branch]);
    std::fs::write(root.join("b.txt"), "change\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "change"]);
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    git(root, &["checkout", "main"]);
    (dir, head)
}

fn merge_via_forge(
    root: &std::path::Path,
    branch: &str,
) -> anyhow::Result<crate::forge::MergeResult> {
    use crate::forge::{ChangeRef, Forge, MergeMethod, MergeOptions, PureGitForge};
    PureGitForge::new(root).merge_change(
        &ChangeRef {
            id: 0,
            url: String::new(),
            branch: branch.to_string(),
            base: "main".to_string(),
            title: None,
        },
        &MergeOptions {
            method: MergeMethod::Squash,
            squash_subject: None,
            squash_body: None, // trace:TASK-1330 | ai:claude
            delete_branch: false,
            match_head: None,
        },
        &mut crate::network_retry::NoopSink,
    )
}

// AC1 end-to-end — `forge::merge_change` refuses on a HAND-WRITTEN refusal
// artifact (no marker, no label, no recording handler anywhere), and the AC3
// release (a fresh approval at the same head) lets the same merge proceed.
#[test]
fn merge_change_refuses_a_hand_written_refusal_then_merges_on_release() {
    let (dir, head) = temp_repo_with_branch("bug-9-work");
    let root = dir.path();
    write_artifact(root, "BUG-9", &body("request-changes", &head, None));
    let err = merge_via_forge(root, "bug-9-work").expect_err("the chokepoint must refuse");
    assert!(err.to_string().contains("request-changes"), "{err:#}");
    // The base is untouched by the refused merge.
    let main_tip = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "--oneline", "main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&main_tip.stdout).lines().count(), 1);
    // AC3 release: a fresh APPROVED verdict at the head it judges.
    write_artifact(
        root,
        "BUG-9",
        &body_by("approved", &head, "reviewer-b", None),
    );
    let merged = merge_via_forge(root, "bug-9-work").expect("released by the fresh approval");
    assert!(merged.merged);
}

// The GitHub provider runs the same gate BEFORE any `gh` invocation: with a
// pinned head (`match_head`) and a refusal at that pin, the refusal arrives
// with no `gh` on PATH and no network.
#[test]
fn github_merge_change_runs_the_corpus_gate_before_gh() {
    use crate::forge::{ChangeRef, Forge, GitHubForge, MergeMethod, MergeOptions};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_artifact(root, "PR-7", &body("request-changes", HEAD, None));
    let err = GitHubForge::new(root)
        .merge_change(
            &ChangeRef {
                id: 7,
                url: String::new(),
                branch: "bug-9-work".to_string(),
                base: String::new(),
                title: None,
            },
            &MergeOptions {
                method: MergeMethod::Squash,
                squash_subject: None,
                squash_body: None, // trace:TASK-1330 | ai:claude
                delete_branch: false,
                match_head: Some(HEAD.to_string()),
            },
            &mut crate::network_retry::NoopSink,
        )
        .expect_err("the corpus gate refuses before gh is ever invoked");
    assert!(err.to_string().contains("request-changes"), "{err:#}");
}

// Wiring guard: every real provider's `merge_change` calls the corpus gate.
// Deleting any one call (the mutation that re-opens this bug for that forge)
// fails here; the needle is split so this file cannot match itself.
#[test]
fn every_merge_provider_is_wired_into_the_corpus_gate() {
    let src = include_str!("../forge.rs");
    let needle = concat!("enforce_corpus_gate_", "before_merge(");
    let calls = src.matches(needle).count();
    assert!(
        calls >= 3,
        "expected the GitHub, GitLab and pure-git merge paths to call the corpus gate; found {calls}"
    );
}
