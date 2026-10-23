//! TASK-1945 S3a: the report-only ship-readiness verdict and its once-only
//! advisor wake. Forge, CI and base evidence are injected through
//! `ready::Evidence`; the approval and carry-over tests run the production
//! gate on real git fixtures. No LLM, no cargo, no network.
// trace:TASK-1945 | ai:claude
use super::ready::{Approval, Evidence, Readiness};
use super::*;
use crate::forge::{CheckOutcome, CommitCheck, CommitChecks, CI_AGGREGATOR_CHECK};
use std::os::unix::fs::PermissionsExt;

const HEAD: u64 = 0xb;

fn fixture() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), "github:github.com/owner/repo").unwrap();
    (temp, store)
}

fn change(behind: u64, overlap: bool) -> forge::Change {
    forge::Change {
        id: 7,
        head: format!("{HEAD:040x}"),
        base: "main".into(),
        author: "writer".into(),
        gitlab: false,
        overlap: Some(overlap),
        behind: Some(behind),
    }
}

fn check(name: &str, outcome: CheckOutcome) -> CommitCheck {
    CommitCheck {
        name: name.into(),
        outcome,
    }
}

fn green(sha: &str) -> CommitChecks {
    CommitChecks {
        sha: sha.into(),
        checks: vec![
            check("CI (lib-0)", CheckOutcome::Passed),
            check(CI_AGGREGATOR_CHECK, CheckOutcome::Passed),
        ],
    }
}

fn approval() -> Approval {
    Approval {
        reviewed: format!("{HEAD:040x}"),
        identity: "reviewer seat at 2026-10-22".into(),
        carried: None,
    }
}

/// Evidence snapshots; every reader is a value or an error.
struct Fake {
    approval: std::result::Result<Approval, String>,
    review: Option<String>,
    hold: std::result::Result<Option<String>, String>,
    ci: std::result::Result<CommitChecks, String>,
    head: std::result::Result<String, String>,
}

impl Fake {
    fn ready() -> Self {
        let head = format!("{HEAD:040x}");
        Self {
            approval: Ok(approval()),
            review: None,
            hold: Ok(None),
            ci: Ok(green(&head)),
            head: Ok(head),
        }
    }
}

impl Evidence for Fake {
    fn approval(&self, _: &forge::Change) -> std::result::Result<Approval, String> {
        self.approval.clone()
    }
    fn under_review(&self, _: &forge::Change) -> Option<String> {
        self.review.clone()
    }
    fn hold(&self, _: &forge::Change) -> Result<Option<String>> {
        self.hold.clone().map_err(|e| anyhow::anyhow!(e))
    }
    fn ci(&self, _: &str) -> Result<CommitChecks> {
        self.ci.clone().map_err(|e| anyhow::anyhow!(e))
    }
    fn required_checks(&self) -> Result<Vec<String>> {
        Ok(vec![CI_AGGREGATOR_CHECK.to_string()])
    }
    fn base(&self, change: &forge::Change) -> Result<(u64, bool)> {
        Ok((change.behind.unwrap(), change.overlap.unwrap()))
    }
    fn head(&self, _: &forge::Change) -> Result<String> {
        self.head.clone().map_err(|e| anyhow::anyhow!(e))
    }
}

/// A recorded verdict for the change, so it is reported on.
fn record_verdict(root: &Path, verdict: &str) {
    let dir = root.join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("PR-7.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "reviewed_sha": format!("{HEAD:040x}"),
            "recorded_by": "reviewer seat",
            "verdict": verdict,
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn mailbox(root: &Path) -> Vec<aida_core::mailbox::Message> {
    let Ok(entries) = std::fs::read_dir(root.join(".aida/mailbox")) else {
        return Vec::new();
    };
    entries
        .map(|e| serde_json::from_slice(&std::fs::read(e.unwrap().path()).unwrap()).unwrap())
        .collect()
}

fn ready_events(root: &Path) -> Vec<crate::events::EventKind> {
    crate::events::read_all(root)
        .into_iter()
        .map(|e| e.kind)
        .filter(|k| matches!(k, crate::events::EventKind::ReviewWatchReady { .. }))
        .collect()
}

fn reported(store: &Store) -> String {
    store.load().unwrap().entries["PR-7"]
        .readiness
        .as_ref()
        .unwrap()
        .line
        .clone()
}

fn not_ready(evidence: &Fake, c: &forge::Change) -> String {
    match ready::evaluate(c, evidence) {
        (Readiness::NotReady(reason), None) => reason,
        other => panic!("expected not ready: {other:?}"),
    }
}

// (1)
#[test]
fn ready_needs_authorized_approval_green_ci_on_head_zero_behind_no_hold() {
    let (temp, store) = fixture();
    record_verdict(temp.path(), "approved");
    let c = change(0, false);
    report(temp.path(), &store, &c, &Fake::ready()).unwrap();
    assert_eq!(reported(&store), "ready");
    let mail = mailbox(temp.path());
    assert_eq!(mail.len(), 1, "one advisor wake");
    assert_eq!(
        mail[0].to,
        aida_core::mailbox::Recipient::Agent("advisor".into())
    );
    assert!(
        mail[0].body.contains("`aida pr ship 7`"),
        "{}",
        mail[0].body
    );
    // C4: the wake never suggests an override.
    assert!(!mail[0].body.contains("--override"), "{}", mail[0].body);
    let events = ready_events(temp.path());
    assert_eq!(
        events,
        vec![crate::events::EventKind::ReviewWatchReady {
            pr: 7,
            head: c.head.clone(),
            after_update_branch: false,
        }]
    );
    assert!(events[0].is_actionable(), "the wake line fires");
    let entry = &store.load().unwrap().entries["PR-7"];
    assert!(entry.wakes.values().all(|w| w.sent));
}

// (2)
#[test]
fn pending_failed_and_absent_ci_is_not_ready() {
    let c = change(0, false);
    let head = c.head.clone();
    let with = |checks: Vec<CommitCheck>| Fake {
        ci: Ok(CommitChecks {
            sha: head.clone(),
            checks,
        }),
        ..Fake::ready()
    };
    let pending = not_ready(
        &with(vec![
            check("CI (lib-0)", CheckOutcome::Pending),
            check(CI_AGGREGATOR_CHECK, CheckOutcome::Passed),
        ]),
        &c,
    );
    assert!(pending.contains("CI pending: CI (lib-0)"), "{pending}");
    let failed = not_ready(
        &with(vec![
            check("CI (lib-0)", CheckOutcome::Failed),
            check(CI_AGGREGATOR_CHECK, CheckOutcome::Pending),
        ]),
        &c,
    );
    assert!(failed.contains("CI failed: CI (lib-0)"), "{failed}");
    assert!(not_ready(&with(vec![]), &c).contains("no CI checks"));
    // Every lane green but the aggregator absent: unknown, not ready.
    let missing = not_ready(&with(vec![check("CI (lib-0)", CheckOutcome::Passed)]), &c);
    assert!(missing.contains(CI_AGGREGATOR_CHECK), "{missing}");
}

// (3)
#[test]
fn ci_on_another_sha_is_not_ready() {
    let c = change(0, false);
    let evidence = Fake {
        ci: Ok(green(&format!("{:040x}", 0xc))),
        ..Fake::ready()
    };
    assert!(not_ready(&evidence, &c).contains("not the head"));
}

// (4)
#[test]
fn ci_reader_error_refuses_with_the_error_and_sends_no_wake() {
    let (temp, store) = fixture();
    record_verdict(temp.path(), "approved");
    let evidence = Fake {
        ci: Err("gh CI read failed (exit status: 1): HTTP 502".into()),
        ..Fake::ready()
    };
    report(temp.path(), &store, &change(0, false), &evidence).unwrap();
    let line = reported(&store);
    assert!(
        line.contains("CI evidence unknown") && line.contains("HTTP 502"),
        "{line}"
    );
    assert!(mailbox(temp.path()).is_empty());
    assert!(ready_events(temp.path()).is_empty());
}

// (5)
#[test]
fn hold_marker_label_or_unreadable_hold_is_not_ready() {
    use crate::merge_hold::HoldReasonKind;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let no_label = || Ok(false);
    assert_eq!(ready::hold_at(root, 7, no_label).unwrap(), None);
    let path = crate::merge_hold::hold_path(root, 7);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for kind in [
        HoldReasonKind::Supervision,
        HoldReasonKind::Recusal,
        HoldReasonKind::Rework,
        HoldReasonKind::Decision,
    ] {
        let record = crate::merge_hold::typed_hold(7, kind, "held for a test", None);
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        let hold = ready::hold_at(root, 7, no_label).unwrap().expect("held");
        let name = format!("{kind:?}").to_lowercase();
        assert!(hold.starts_with(&format!("{name} hold")), "{hold}");
    }
    // A malformed marker is an unknown-kind hold, never clean.
    std::fs::write(&path, "{ not json").unwrap();
    assert!(ready::hold_at(root, 7, no_label)
        .unwrap()
        .unwrap()
        .starts_with("unknown hold"));
    std::fs::remove_file(&path).unwrap();
    // The label alone holds; an unreadable label refuses.
    assert!(ready::hold_at(root, 7, || Ok(true)).unwrap().is_some());
    assert!(ready::hold_at(root, 7, || Err("gh failed".into())).is_err());

    let c = change(0, false);
    let held = Fake {
        hold: Ok(Some("decision hold: needs a human".into())),
        ..Fake::ready()
    };
    assert!(not_ready(&held, &c).contains("held: decision hold"));
    let unreadable = Fake {
        hold: Err("merge-hold label unreadable: gh failed".into()),
        ..Fake::ready()
    };
    assert!(not_ready(&unreadable, &c).contains("unreadable"));
}

// (6)
#[test]
fn under_review_marker_is_not_ready() {
    let evidence = Fake {
        review: Some("claude reviewing since 07:00".into()),
        ..Fake::ready()
    };
    assert!(not_ready(&evidence, &change(0, false)).contains("under review"));
}

// (7)
#[test]
fn stale_non_overlap_reports_ready_after_update_branch() {
    let (temp, store) = fixture();
    record_verdict(temp.path(), "approved");
    let c = change(3, false);
    report(temp.path(), &store, &c, &Fake::ready()).unwrap();
    let line = reported(&store);
    assert!(
        line.starts_with("ready after its branch is updated")
            && line.contains("`gh pr update-branch 7`"),
        "{line}"
    );
    let mail = mailbox(temp.path());
    assert_eq!(mail.len(), 1);
    assert!(
        mail[0].body.contains("`gh pr update-branch 7`"),
        "{}",
        mail[0].body
    );
    assert!(!mail[0].body.contains("--override"));
    assert_eq!(
        ready_events(temp.path()),
        vec![crate::events::EventKind::ReviewWatchReady {
            pr: 7,
            head: c.head.clone(),
            after_update_branch: true,
        }]
    );
    // Behind with overlapping paths is not ready at all.
    let reason = not_ready(&Fake::ready(), &change(3, true));
    assert!(reason.contains("overlapping paths"), "{reason}");
}

mod gate {
    //! The production approval gate, decided read-only, on real git
    //! fixtures (TASK-1960's carry-over repository).
    use super::*;
    use crate::pr_ship::ShipApprovalPolicy;
    use crate::review_carryover::tests::{commit, fixture, head, write};
    use crate::review_carryover::CarryOverSite;
    use crate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};

    fn verdict(sha: &str) -> RecordedVerdict {
        RecordedVerdict {
            kind: VerdictKind::Approved,
            raw: "approved".into(),
            reviewed_sha: Some(sha.into()),
            recorded_at: Some("2026-09-23T00:00:00Z".into()),
            recorded_by: Some("independent reviewer".into()),
            attestation: Some(RecorderAttestation {
                sha: sha.into(),
                human_at_tty: true,
                receipt_id: Some("rcpt".into()),
                identity: vec!["session:independent-reviewer".into()],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// A policy under which `verdicts`' attestations re-validated, shipped
    /// by an independent session.
    fn policy(verdicts: &[RecordedVerdict]) -> ShipApprovalPolicy {
        let mut p = ShipApprovalPolicy {
            reviewed_committed_at: Some("2026-09-22T00:00:00Z".parse().unwrap()),
            shipper: vec!["session:independent-shipper".into()],
            ..Default::default()
        };
        for v in verdicts {
            if let Some(a) = v.attestation.as_ref() {
                if let Some(k) = a.authority_key(v.recorded_at.as_deref()) {
                    p.verified_authority.insert(k, a.identity.clone());
                }
            }
        }
        p
    }

    /// Merge `main` into the PR branch, through the maintenance-safe helper.
    fn merge_main(repo: &Path) {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(repo)
            .args(["merge", "--quiet", "--no-edit", "main"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn untouched(root: &Path) {
        for side_effect in [
            ".aida/advisor-activity.jsonl",
            ".aida/events.jsonl",
            ".aida/review-carryover",
        ] {
            assert!(!root.join(side_effect).exists(), "{side_effect} written");
        }
    }

    // (8)
    #[test]
    fn clean_base_merge_carries_the_approval() {
        let (repo, approved) = fixture();
        merge_main(repo.path());
        let merged = head(repo.path());
        let site = CarryOverSite::quiet(repo.path(), Some("main"));
        let candidates = [verdict(&approved)];
        let approval = ready::approval_at(42, &candidates, &merged, &policy(&candidates), &site)
            .expect("a clean main merge carries the approval");
        assert_eq!(approval.reviewed, approved);
        assert!(
            approval
                .carried
                .as_deref()
                .is_some_and(|c| c.contains("`main`")),
            "{approval:?}"
        );
        // Green CI on the merge commit makes the change ready there.
        let c = forge::Change {
            head: merged.clone(),
            ..change(0, false)
        };
        let evidence = Fake {
            approval: Ok(approval),
            ci: Ok(green(&merged)),
            head: Ok(merged),
            ..Fake::ready()
        };
        assert!(matches!(
            ready::evaluate(&c, &evidence),
            (Readiness::Ready, Some(_))
        ));
        untouched(repo.path());
    }

    // (9)
    #[test]
    fn own_diff_change_refuses_carry_over() {
        let (repo, approved) = fixture();
        merge_main(repo.path());
        write(repo.path(), "c.txt", "an unreviewed change\n");
        let moved = commit(repo.path(), "pr: unreviewed");
        let site = CarryOverSite::quiet(repo.path(), Some("main"));
        let candidates = [verdict(&approved)];
        let refusal = ready::approval_at(42, &candidates, &moved, &policy(&candidates), &site)
            .expect_err("an own-diff change needs a new review");
        assert!(refusal.contains("did not carry over"), "{refusal}");
        untouched(repo.path());
    }

    // (10)
    #[test]
    fn self_or_unauthorized_approval_is_not_ready() {
        let temp = tempfile::tempdir().unwrap();
        let site = CarryOverSite::quiet(temp.path(), Some("main"));
        let sha = format!("{HEAD:040x}");
        let candidates = [verdict(&sha)];
        // No re-validated authority: unattested.
        let refusal =
            ready::approval_at(7, &candidates, &sha, &ShipApprovalPolicy::default(), &site)
                .expect_err("an unattested approval is not ready");
        assert!(!refusal.is_empty());
        // Recorded by the work's own author session: a self-approval.
        let mut own = policy(&candidates);
        own.author_sessions = vec![crate::pr_ship::AuthorSession {
            label: "implementer lease".into(),
            tokens: vec!["session:independent-reviewer".into()],
        }];
        let refusal = ready::approval_at(7, &candidates, &sha, &own, &site)
            .expect_err("a self-approval is not ready");
        assert!(!refusal.is_empty());
        // No approval at all.
        assert!(ready::approval_at(7, &[], &sha, &policy(&[]), &site).is_err());
        // The authorized approval at the head passes.
        let approval = ready::approval_at(7, &candidates, &sha, &policy(&candidates), &site)
            .expect("an independent approval at the head");
        assert_eq!(approval.carried, None);
        untouched(temp.path());
    }

    // (17)
    #[test]
    fn approval_decision_has_no_side_effects() {
        let (repo, approved) = fixture();
        merge_main(repo.path());
        let merged = head(repo.path());
        let site = CarryOverSite::quiet(repo.path(), Some("main"));
        let candidates = [verdict(&approved)];
        for (candidates, policy) in [
            (&candidates[..], policy(&candidates)),
            (&candidates[..], ShipApprovalPolicy::default()),
            (&[][..], policy(&[])),
        ] {
            let _ = crate::pr_cmd::approval_decision(
                42,
                candidates,
                Some(&merged),
                &policy,
                Some(&site),
            );
        }
        let _ = crate::pr_cmd::approval_decision(
            42,
            &candidates,
            Some(&merged),
            &policy(&candidates),
            None,
        );
        untouched(repo.path());
        assert!(site.proven_base().is_some(), "the quiet proof was applied");
    }
}

// (11)
#[test]
fn head_moving_before_emit_sends_nothing() {
    let (temp, store) = fixture();
    record_verdict(temp.path(), "approved");
    let evidence = Fake {
        head: Ok(format!("{:040x}", 0xd)),
        ..Fake::ready()
    };
    report(temp.path(), &store, &change(0, false), &evidence).unwrap();
    assert!(
        reported(&store).contains("head moved"),
        "{}",
        reported(&store)
    );
    assert!(mailbox(temp.path()).is_empty());
    assert!(ready_events(temp.path()).is_empty());
}

// (12)
#[test]
fn wake_is_once_only_across_passes_and_restart() {
    let (temp, store) = fixture();
    record_verdict(temp.path(), "approved");
    let c = change(0, false);
    // The mail lands, then the process dies before its receipt: the event
    // append fails here, which leaves the same state.
    std::fs::create_dir_all(crate::events::events_path(temp.path())).unwrap();
    assert!(report(temp.path(), &store, &c, &Fake::ready()).is_err());
    assert_eq!(mailbox(temp.path()).len(), 1);
    assert!(store.load().unwrap().entries["PR-7"]
        .wakes
        .values()
        .all(|w| !w.sent));
    std::fs::remove_dir(crate::events::events_path(temp.path())).unwrap();
    // A restarted watcher re-delivers under the same ID: still one mail.
    let restarted = Store::open(temp.path(), "github:github.com/owner/repo").unwrap();
    for _ in 0..3 {
        report(temp.path(), &restarted, &c, &Fake::ready()).unwrap();
    }
    assert_eq!(mailbox(temp.path()).len(), 1);
    assert_eq!(ready_events(temp.path()).len(), 1);
    assert!(store.load().unwrap().entries["PR-7"]
        .wakes
        .values()
        .all(|w| w.sent));
    // A new head is a new wake; the old head's receipt is dropped.
    let moved = forge::Change {
        head: format!("{:040x}", 0xe),
        ..c
    };
    let evidence = Fake {
        ci: Ok(green(&moved.head)),
        head: Ok(moved.head.clone()),
        ..Fake::ready()
    };
    report(temp.path(), &store, &moved, &evidence).unwrap();
    assert_eq!(mailbox(temp.path()).len(), 2);
    let entry = &store.load().unwrap().entries["PR-7"];
    assert_eq!(entry.wakes.len(), 1);
    assert!(entry.wakes.keys().all(|k| k.starts_with(&moved.head)));
}

// (13)
#[test]
fn failed_delivery_is_retried_and_sent_once() {
    let (temp, store) = fixture();
    record_verdict(temp.path(), "approved");
    let c = change(0, false);
    let mailbox_dir = temp.path().join(".aida/mailbox");
    std::fs::write(&mailbox_dir, "not a directory").unwrap();
    assert!(report(temp.path(), &store, &c, &Fake::ready()).is_err());
    assert!(
        ready_events(temp.path()).is_empty(),
        "no event before the mail"
    );
    std::fs::remove_file(&mailbox_dir).unwrap();
    report(temp.path(), &store, &c, &Fake::ready()).unwrap();
    report(temp.path(), &store, &c, &Fake::ready()).unwrap();
    assert_eq!(mailbox(temp.path()).len(), 1);
    assert_eq!(ready_events(temp.path()).len(), 1);
}

// (16)
#[test]
fn corrupt_verdict_is_not_ready() {
    let (temp, store) = fixture();
    record_verdict(temp.path(), "approved");
    let path = temp.path().join(".aida/review-verdicts/PR-7.json");
    std::fs::write(&path, "{ truncated").unwrap();
    report(temp.path(), &store, &change(0, false), &Fake::ready()).unwrap();
    assert!(
        reported(&store).contains("verdict history unreadable"),
        "{}",
        reported(&store)
    );
    assert!(mailbox(temp.path()).is_empty());
}

/// A change with an active review, or no recorded verdict, is not judged.
#[test]
fn unreviewed_or_active_change_is_not_reported() {
    let (temp, store) = fixture();
    let c = change(0, false);
    report(temp.path(), &store, &c, &Fake::ready()).unwrap();
    assert!(!store.load().unwrap().entries.contains_key("PR-7"));
    record_verdict(temp.path(), "approved");
    let next = forge::Change {
        head: format!("{:040x}", 0xf),
        ..c.clone()
    };
    let attempt = match select(&store, &next, None, 2).unwrap() {
        Action::Dispatch(attempt) => attempt,
        other => panic!("{other:?}"),
    };
    report(temp.path(), &store, &next, &Fake::ready()).unwrap();
    let entry = &store.load().unwrap().entries["PR-7"];
    assert!(entry.readiness.is_none() && entry.wakes.is_empty());
    assert_eq!(entry.attempts[0].token, attempt.token);
    assert!(mailbox(temp.path()).is_empty());
}

// (19)
#[test]
fn legacy_state_without_wakes_loads() {
    let (temp, store) = fixture();
    let legacy = serde_json::json!({
        "version": 1,
        "repository": "github:github.com/owner/repo",
        "max_concurrent": 2,
        "entries": {"PR-3": {
            "attempts": [],
            "notices": {format!("{:040x}", 1): {
                "message": stale_message(
                    "github:github.com/owner/repo",
                    "PR-3",
                    &forge::Change { id: 3, head: format!("{:040x}", 1), ..change(1, true) },
                    "writer",
                ),
                "sent": true,
            }},
        }},
    });
    let dir = temp.path().join(".aida/review-watch");
    std::fs::write(dir.join(STATE), serde_json::to_vec(&legacy).unwrap()).unwrap();
    std::fs::set_permissions(dir.join(STATE), std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(dir.join("initialized"), "").unwrap();
    std::fs::set_permissions(
        dir.join("initialized"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let state = store.load().unwrap();
    let entry = &state.entries["PR-3"];
    assert!(entry.wakes.is_empty() && entry.readiness.is_none());
    // Written back, an entry with no wakes or readiness adds no new fields.
    store.transaction("PR-3", |_| Ok(())).unwrap();
    let text = std::fs::read_to_string(dir.join(STATE)).unwrap();
    assert!(
        !text.contains("wakes") && !text.contains("readiness"),
        "{text}"
    );
}

/// `aida ps` reads the last readiness without creating watcher state.
// trace:TASK-1945 | ai:claude
#[test]
fn readiness_reports_read_only() {
    let empty = tempfile::tempdir().unwrap();
    assert!(readiness_reports(empty.path()).is_empty());
    assert!(!empty.path().join(".aida").exists());
    let (temp, store) = fixture();
    record_verdict(temp.path(), "approved");
    report(temp.path(), &store, &change(0, false), &Fake::ready()).unwrap();
    let reports = readiness_reports(temp.path());
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0, "PR-7");
    assert!(reports[0].1.starts_with(&format!(
        "at {}: ready (checked ",
        &format!("{HEAD:040x}")[..12]
    )));
}

// TASK-2150: a reservation superseded before its dispatcher was recorded
// frees its slot once the dispatcher is reaped or never spawned.
// trace:TASK-1945 | ai:claude (TASK-2150)
#[test]
fn superseded_unclaimed_dispatch_frees_its_slot() {
    let (_temp, store) = fixture();
    let first = forge::Change {
        head: format!("{:040x}", 1),
        ..change(0, false)
    };
    for (n, spawned) in [(1u64, true), (2, false)] {
        let c = forge::Change {
            id: n,
            ..first.clone()
        };
        let attempt = match select(&store, &c, None, 2).unwrap() {
            Action::Dispatch(attempt) => attempt,
            other => panic!("{other:?}"),
        };
        let key = c.key();
        // A concurrent watcher supersedes the reservation first.
        store
            .transaction(&key, |state| {
                let a = attempt_mut(state, &key, &attempt.token).unwrap();
                a.phase = Phase::Superseding;
                a.supersede = Some(Supersede {
                    by: format!("{:040x}", 2),
                    requested_at: chrono::Utc::now(),
                    term_at: None,
                    kill_at: None,
                    diagnostic: None,
                });
                Ok(())
            })
            .unwrap();
        if spawned {
            mark_unclaimed_exit(&store, &key, &attempt.token, "exit status: 1").unwrap();
        } else {
            mark_spawn_failed(&store, &key, &attempt.token).unwrap();
        }
        let state = store.load().unwrap();
        let a = &state.entries[&key].attempts[0];
        assert_eq!(a.phase, Phase::Superseded, "{key}");
        assert!(!active(a));
    }
    assert_eq!(capacity_holders(&store.load().unwrap()), "");
}

// TASK-2200: the wake key names the approval, as its message does: a new
// approval of the same head wakes the advisor again; the same one never does.
// trace:TASK-1945 | ai:claude (TASK-2200)
#[test]
fn new_approval_of_the_same_head_wakes_again() {
    let (temp, store) = fixture();
    record_verdict(temp.path(), "approved");
    let c = change(0, false);
    report(temp.path(), &store, &c, &Fake::ready()).unwrap();
    report(temp.path(), &store, &c, &Fake::ready()).unwrap();
    assert_eq!(mailbox(temp.path()).len(), 1);
    let again = Fake {
        approval: Ok(Approval {
            identity: "second reviewer seat at 2026-10-23".into(),
            ..approval()
        }),
        ..Fake::ready()
    };
    report(temp.path(), &store, &c, &again).unwrap();
    report(temp.path(), &store, &c, &again).unwrap();
    assert_eq!(mailbox(temp.path()).len(), 2);
    let entry = &store.load().unwrap().entries["PR-7"];
    assert_eq!(entry.wakes.len(), 2, "{:?}", entry.wakes.keys());
    assert!(entry.wakes.values().all(|w| w.sent));
}

// TASK-2200: a wake receipt recorded before the key named the approval
// (`<head>:<kind>`) still loads.
#[test]
fn legacy_wake_key_without_approval_loads() {
    let (temp, store) = fixture();
    record_verdict(temp.path(), "approved");
    let c = change(0, false);
    report(temp.path(), &store, &c, &Fake::ready()).unwrap();
    let path = temp.path().join(".aida/review-watch").join(STATE);
    let text = std::fs::read_to_string(&path).unwrap();
    let key = store.load().unwrap().entries["PR-7"]
        .wakes
        .keys()
        .next()
        .unwrap()
        .clone();
    let legacy = key.rsplit_once(':').unwrap().0.to_string();
    std::fs::write(&path, text.replace(&key, &legacy)).unwrap();
    assert!(store.load().unwrap().entries["PR-7"]
        .wakes
        .contains_key(&legacy));
}

/// Evidence whose required checks are injected.
struct Required(Fake, std::result::Result<Vec<String>, String>);
impl Evidence for Required {
    fn approval(&self, c: &forge::Change) -> std::result::Result<Approval, String> {
        self.0.approval(c)
    }
    fn under_review(&self, c: &forge::Change) -> Option<String> {
        self.0.under_review(c)
    }
    fn hold(&self, c: &forge::Change) -> Result<Option<String>> {
        self.0.hold(c)
    }
    fn ci(&self, sha: &str) -> Result<CommitChecks> {
        self.0.ci(sha)
    }
    fn required_checks(&self) -> Result<Vec<String>> {
        self.1.clone().map_err(|e| anyhow::anyhow!(e))
    }
    fn base(&self, c: &forge::Change) -> Result<(u64, bool)> {
        self.0.base(c)
    }
    fn head(&self, c: &forge::Change) -> Result<String> {
        self.0.head(c)
    }
}

// TASK-2201: the required checks are the project's own, from its config.
// trace:TASK-1945 | ai:claude (TASK-2201)
#[test]
fn required_checks_come_from_the_project_config() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    assert!(
        ready::required_checks(root).unwrap().is_empty(),
        "no config"
    );
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let config = root.join(".aida/config.toml");
    std::fs::write(&config, "[drain]\nretry_transient = 2\n").unwrap();
    assert!(ready::required_checks(root).unwrap().is_empty(), "unset");
    std::fs::write(
        &config,
        "[review_watch]\nrequired_checks = [\"test (stable)\", \"lint\"]\n",
    )
    .unwrap();
    assert_eq!(
        ready::required_checks(root).unwrap(),
        vec!["test (stable)".to_string(), "lint".to_string()]
    );
    for broken in [
        "[review_watch]\nrequired_checks = \"lint\"\n",
        "[review_watch]\nrequired_checks = [1]\n",
        "[review_watch\n",
    ] {
        std::fs::write(&config, broken).unwrap();
        assert!(ready::required_checks(root).is_err(), "{broken}");
    }

    // Another project's checks: green with none named, or with its own.
    let c = change(0, false);
    let theirs = CommitChecks {
        sha: c.head.clone(),
        checks: vec![check("test (stable)", CheckOutcome::Passed)],
    };
    let fake = || Fake {
        ci: Ok(theirs.clone()),
        ..Fake::ready()
    };
    for required in [vec![], vec!["test (stable)".to_string()]] {
        let (readiness, _) = ready::evaluate(&c, &Required(fake(), Ok(required)));
        assert_eq!(readiness, Readiness::Ready);
    }
    let missing = not_ready_with(&Required(fake(), Ok(vec!["lint".into()])), &c);
    assert!(missing.contains("`lint`"), "{missing}");
    let unknown = not_ready_with(&Required(fake(), Err("parse .aida/config.toml".into())), &c);
    assert!(unknown.contains("required CI checks unknown"), "{unknown}");
}

fn not_ready_with(evidence: &dyn Evidence, c: &forge::Change) -> String {
    match ready::evaluate(c, evidence) {
        (Readiness::NotReady(reason), None) => reason,
        other => panic!("expected not ready: {other:?}"),
    }
}

// TASK-2202: the GitLab advice is a merge of the target branch, which the
// carry-over accepts, never a rebase.
// trace:TASK-1945 | ai:claude (TASK-2202)
#[test]
fn update_advice_is_a_merge_on_both_forges() {
    let github = change(2, false);
    let gitlab = forge::Change {
        gitlab: true,
        ..change(2, false)
    };
    assert_eq!(ready::update_command(&github), "`gh pr update-branch 7`");
    let advice = ready::update_command(&gitlab);
    assert!(advice.contains("`git merge origin/main`"), "{advice}");
    let line = Readiness::ReadyAfterUpdateBranch { behind: 2 }.line(&gitlab);
    assert!(!line.contains("rebase"), "{line}");
    let message = ready::wake_message(
        "gitlab:gitlab.com/owner/repo",
        &gitlab,
        &Readiness::ReadyAfterUpdateBranch { behind: 2 },
        &approval(),
    );
    assert!(!message.body.contains("rebase"), "{}", message.body);
    assert!(
        message.body.contains("git merge origin/main"),
        "{}",
        message.body
    );
}

// TASK-2203: the Live adapters read the real marker, corpus and hold files.
// trace:TASK-1945 | ai:claude (TASK-2203)
#[test]
fn live_review_marker_is_read_for_the_head() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let c = change(0, false);
    assert_eq!(ready::live_under_review(root, &c), None);
    let other = format!("{:040x}", 0xe);
    let marker =
        crate::review_marker::Marker::for_this_process(7, Some(&other), None, "reviewer seat");
    crate::review_marker::write(root, &marker).unwrap();
    assert_eq!(
        ready::live_under_review(root, &c),
        None,
        "a marker for another head does not cover this one"
    );
    let marker =
        crate::review_marker::Marker::for_this_process(7, Some(&c.head), None, "reviewer seat");
    crate::review_marker::write(root, &marker).unwrap();
    let described = ready::live_under_review(root, &c).expect("live marker at the head");
    assert!(described.contains("reviewer seat"), "{described}");
}

#[test]
fn live_corpus_gate_refuses_a_blocking_verdict_at_the_head() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let c = change(0, false);
    assert_eq!(ready::corpus_at(root, &c, &[]), Ok(()));
    record_verdict(root, "request-changes");
    let refusal = ready::corpus_at(root, &c, &[]).unwrap_err();
    assert!(refusal.contains("request-changes"), "{refusal}");
    assert!(
        crate::merge_hold::read_hold_record(root, 7).is_none(),
        "the read-only gate arms no hold"
    );
    record_verdict(root, "approved");
    assert_eq!(ready::corpus_at(root, &c, &[]), Ok(()));
}

#[test]
fn live_hold_reads_the_marker_then_the_label() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let c = change(0, false);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    // No forge: no label to read, so no hold.
    std::fs::write(
        root.join(".aida/config.toml"),
        "[forge]\nprovider = \"pure-git\"\n",
    )
    .unwrap();
    assert_eq!(ready::live_hold(root, &c).unwrap(), None);
    // A GitHub project whose label cannot be read pinned (no origin): not
    // clean, an error.
    std::fs::write(
        root.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    let error = ready::live_hold(root, &c).unwrap_err();
    assert!(
        format!("{error:#}").contains("merge-hold label unreadable"),
        "{error:#}"
    );
    // A marker is reported before the label is read.
    let hold = crate::merge_hold::typed_hold(
        7,
        crate::merge_hold::HoldReasonKind::Rework,
        "findings outstanding",
        Some(c.head.clone()),
    );
    crate::merge_hold::write_typed_hold(root, &hold).unwrap();
    let held = ready::live_hold(root, &c).unwrap().expect("marker hold");
    assert!(held.contains("rework hold"), "{held}");
}

// TASK-2204: a pending wake is retried on the next pass, not an interval
// later; a sent one, or a judged head with none, waits out the interval.
// trace:TASK-1945 | ai:claude (TASK-2204)
#[test]
fn pending_wake_is_never_throttled() {
    let head = format!("{HEAD:040x}");
    let now = chrono::Utc::now();
    let interval = chrono::Duration::seconds(120);
    let mut entry = Entry {
        readiness: Some(Reported {
            head: head.clone(),
            line: "ready".into(),
            checked_at: now,
        }),
        ..Entry::default()
    };
    assert!(throttled(&entry, &head, now, interval));
    assert!(!throttled(&entry, &format!("{:040x}", 0xe), now, interval));
    assert!(!throttled(&entry, &head, now + interval, interval));
    let wake = ready::wake_message(
        "github:github.com/owner/repo",
        &change(0, false),
        &Readiness::Ready,
        &approval(),
    );
    entry.wakes.insert(
        format!("{head}:{}:{}", ready::WAKE_READY, notice::digest("a")),
        Notice {
            message: wake,
            sent: false,
        },
    );
    assert!(!throttled(&entry, &head, now, interval), "pending wake");
    entry.wakes.values_mut().for_each(|w| w.sent = true);
    assert!(throttled(&entry, &head, now, interval));
}
