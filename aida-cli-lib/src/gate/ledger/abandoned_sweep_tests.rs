//! BUG-2786 AC1/AC4: an exited reviewer's pending sweep is finalized or
//! expired to "needs review"; a live or recent one is left alone.
// trace:BUG-2786 | ai:claude
use super::*;
use crate::gate::ledger::{project, EventBody};
use serde_json::{json, Value};

const HEAD: &str = "1111111111111111111111111111111111111111";
type Fin<'a> = &'a dyn Fn(&GateEvent, &JsonObj) -> std::io::Result<()>;
fn no_finalize(_: &GateEvent, _: &JsonObj) -> std::io::Result<()> {
    panic!("must not finalize")
}
const NO_FINALIZE: Fin<'static> = &no_finalize;

/// A change ledger holding the barrier `record_verdict_at` writes for an
/// approval whose findings were not yet delivered.
fn setup(
    pr: u64,
    extra: Value,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    crate::test_env::EnvVarsGuard,
) {
    let env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let mut record = json!({
        "verdict": "request-changes", "sweep_pending": true, "reviewed_sha": HEAD,
        "recorded_by": "reviewer:gone-review-session", "findings": [],
        "recorded_at": chrono::Utc::now().to_rfc3339(),
    });
    record
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let change = ChangeKey::pr(pr);
    let record = record.as_object().unwrap().clone();
    let mut event = GateEvent::new(change, EventBody::Verdict { record });
    event.head = Some(HEAD.to_string());
    store::append(&store_root(&root).unwrap(), change, vec![event]).unwrap();
    (tmp, root, env)
}

/// The barrier of an approval whose only finding is deferred.
fn deferred() -> Value {
    deferred_of(Some("approved"))
}

/// A deferred-finding barrier standing in for `original` (`None` = a barrier
/// that names no original verdict).
fn deferred_of(original: Option<&str>) -> Value {
    let mut body = json!({"findings": ["P3 follow-up"], "finding_dispositions":
        [{"severity": "minor", "disposition": "deferred", "rationale": "later"}]});
    if let Some(word) = original {
        body[rv::BARRIER_OF] = word.into();
    }
    body
}

/// Review finding on #4752: the collected-verdict writer barriers a rejected
/// record with a deferred finding too. Expiry must never turn that rejection
/// into an approval, nor finalize a request-changes or unlabeled barrier.
#[test]
fn a_non_approval_barrier_is_abandoned_never_finalized() {
    for (pr, original) in [
        (81, Some("rejected")),
        (82, Some("request-changes")),
        (83, None),
        (84, Some("waiting")),
    ] {
        let (_tmp, root, _env) = setup(pr, deferred_of(original));
        let done = run(&root, pr, true, true, NO_FINALIZE);
        assert_eq!(
            done,
            SweepExpiry {
                finalized: 0,
                abandoned: 1
            },
            "{original:?}"
        );
        let after = rv::read_recorded_verdict(&root, &format!("PR-{pr}")).unwrap();
        assert!(!after.kind.approves(), "{original:?} never approves");
        assert!(rv::sweep_abandoned(&newest(&root, pr)), "{original:?}");
    }
}

fn run(root: &Path, pr: u64, aged: bool, ended: bool, finalize: Fin) -> SweepExpiry {
    run_under(root, pr, aged, ended, finalize, "")
}

/// [`run`] under the blocking policy `config` holds now ("" = unset).
fn run_under(
    root: &Path,
    pr: u64,
    aged: bool,
    ended: bool,
    finalize: Fin,
    config: &str,
) -> SweepExpiry {
    let late = SWEEP_ABANDON_AFTER + chrono::Duration::minutes(1);
    let now = chrono::Utc::now() + if aged { late } else { chrono::Duration::zero() };
    expire_with(
        root,
        ChangeKey::pr(pr),
        now,
        SWEEP_ABANDON_AFTER,
        &blocking::from_config_text(config),
        &|_| ended,
        finalize,
    )
    .unwrap()
}

fn newest(root: &Path, pr: u64) -> Value {
    Value::Object(project::newest_verdict(root, &format!("PR-{pr}")).unwrap())
}

/// The writer half: under tier policy the collected-verdict writer barriers a
/// rejected record with a deferred finding, and the barrier keeps the word
/// `rejected`. When delivery dies there, expiry abandons it.
#[test]
fn a_collected_rejection_barrier_records_its_verdict_and_is_never_approved() {
    let (_tmp, root, _env) = setup(85, json!({}));
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[review]\nblocking_tiers = [\"p1\"]\n",
    )
    .unwrap();
    crate::review_blocking::commit_config_for_test(&root);
    let mut rejected = json!({"verdict": "rejected", "reviewed_sha": HEAD,
        "recorded_by": "reviewer:gone-review-session",
        "recorded_at": chrono::Utc::now().to_rfc3339()});
    rejected
        .as_object_mut()
        .unwrap()
        .extend(deferred_of(None).as_object().unwrap().clone());
    let rejected = rejected.as_object().unwrap().clone();
    // No canonical store here: delivery fails after the barrier is durable.
    assert!(
        super::super::record_collected_verdict(&root, ChangeKey::pr(85), &rejected, &[]).is_err()
    );
    let barrier = newest(&root, 85);
    assert!(rv::delivery_pending(&barrier));
    assert_eq!(barrier[rv::BARRIER_OF], "rejected");
    assert_eq!(run(&root, 85, true, true, NO_FINALIZE).abandoned, 1);
    let after = rv::read_recorded_verdict(&root, "PR-85").unwrap();
    assert!(!after.kind.approves());
}

#[test]
fn exited_reviewer_with_incomplete_draft_is_marked_sweep_abandoned_and_needs_review() {
    let (_tmp, root, _env) = setup(71, json!({}));
    let before = rv::read_recorded_verdict(&root, "PR-71").unwrap();
    assert_eq!(
        before.kind,
        rv::VerdictKind::RequestChanges,
        "barrier reads fail-closed"
    );
    assert!(crate::gate::review_marker::refuse_conclusive_head(&root, 71, Some(HEAD)).is_err());

    let done = run(&root, 71, true, true, NO_FINALIZE);
    assert_eq!(
        done,
        SweepExpiry {
            finalized: 0,
            abandoned: 1
        }
    );
    assert!(rv::sweep_abandoned(&newest(&root, 71)));
    assert!(!rv::delivery_pending(&newest(&root, 71)));
    let after = rv::read_recorded_verdict(&root, "PR-71").unwrap();
    assert!(!after.kind.approves(), "never approval");
    assert!(!after.kind.requires_changes(), "never request-changes");
    // Needs review: a fresh review is admitted at the head again.
    crate::gate::review_marker::refuse_conclusive_head(&root, 71, Some(HEAD)).unwrap();
    // A second pass leaves the expired record alone.
    assert_eq!(
        run(&root, 71, true, true, NO_FINALIZE),
        SweepExpiry::default()
    );
}

#[test]
fn exited_reviewer_with_complete_draft_is_finalized() {
    let (_tmp, root, _env) = setup(72, deferred());
    let seen = std::cell::Cell::new(0);
    let finalize = |_: &GateEvent, approval: &JsonObj| {
        assert!(rv::disposition_verdict(approval).approves());
        assert!(approval.get("sweep_pending").is_none());
        assert!(approval.get(rv::BARRIER_OF).is_none());
        assert_eq!(approval["reviewed_sha"], HEAD);
        seen.set(seen.get() + 1);
        Ok(())
    };
    assert_eq!(run(&root, 72, true, true, &finalize).finalized, 1);
    assert_eq!(seen.get(), 1);
}

#[test]
fn a_failed_finalize_expires_the_barrier_instead_of_leaving_it_standing() {
    let (_tmp, root, _env) = setup(73, deferred());
    let fail: Fin = &|_, _| Err(std::io::Error::other("store unavailable"));
    let done = run(&root, 73, true, true, fail);
    assert_eq!(
        done,
        SweepExpiry {
            finalized: 0,
            abandoned: 1
        }
    );
}

#[test]
fn live_or_recent_reviewer_sweeps_are_untouched() {
    let (_tmp, root, _env) = setup(74, json!({}));
    assert_eq!(
        run(&root, 74, true, false, NO_FINALIZE),
        SweepExpiry::default(),
        "live"
    );
    assert_eq!(
        run(&root, 74, false, true, NO_FINALIZE),
        SweepExpiry::default(),
        "young"
    );
    assert!(rv::delivery_pending(&newest(&root, 74)));
}

#[test]
fn session_registry_decides_whether_the_reviewer_ended() {
    let (_tmp, root, _env) = setup(75, json!({}));
    let mut named = project::newest_verdict(&root, "PR-75").unwrap();
    assert!(
        reviewer_session_ended(&root, &named),
        "no lease: session ended"
    );
    let sessions = root.join(".aida/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("gone-review-session.toml"), "not a lease").unwrap();
    assert!(!reviewer_session_ended(&root, &named), "unreadable lease");
    named.insert("recorded_by".into(), "joe".into());
    assert!(!reviewer_session_ended(&root, &named), "no session named");
}

/// A complete barrier whose deferral the policy held now would block is not
/// finalized from the barrier: it is abandoned for a fresh review.
#[test]
fn a_policy_that_now_blocks_the_deferral_abandons_instead_of_finalizing() {
    let (_tmp, root, _env) = setup(76, deferred());
    let invalid = "[review]\nblocking_tiers = 5\n";
    let done = run_under(&root, 76, true, true, NO_FINALIZE, invalid);
    assert_eq!(
        done,
        SweepExpiry {
            finalized: 0,
            abandoned: 1
        }
    );
}

/// A worktree-less review lease whose liveness is its creator process:
/// `live` names this test process, otherwise no process (stale).
fn register(root: &Path, id: &str, live: bool) {
    let lease: crate::leases::SessionLease = serde_json::from_value(json!({
        "id": id, "scope": "PR-75", "slug": "pr-75", "owner": "test",
        "worktree_path": "", "branch": "main", "review_verb": true,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "hostname": crate::coordination::hostname(),
        "pid_namespace": aida_core::pid_observer::current_pid_namespace(),
        "creator_pid": live.then(std::process::id),
    }))
    .unwrap();
    let path = crate::lease_path(root, id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, toml::to_string_pretty(&lease).unwrap()).unwrap();
}

#[test]
fn registered_leases_decide_liveness_for_every_named_session() {
    let (_tmp, root, _env) = setup(77, json!({}));
    let mut named = project::newest_verdict(&root, "PR-77").unwrap();
    register(&root, "gone-review-session", true);
    assert!(!reviewer_session_ended(&root, &named), "live lease");
    register(&root, "gone-review-session", false);
    assert!(reviewer_session_ended(&root, &named), "stale lease");
    // Two named sessions: one stale, one live; the reviewer has not ended.
    register(&root, "live-attest-session", true);
    named.insert(
        rv::ATTESTATION_KEY.into(),
        json!({"identity": ["session:live-attest-session"]}),
    );
    assert!(!reviewer_session_ended(&root, &named), "one live session");
    register(&root, "live-attest-session", false);
    assert!(reviewer_session_ended(&root, &named), "both stale");
}

/// The merge gate over the change's ledger history at `head`.
fn gate(root: &Path, pr: u64, head: &str) -> crate::gate::merge_hold_core::CorpusMergeGate {
    let event = project::newest_verdict_event(root, &format!("PR-{pr}")).unwrap();
    let bodies = rv::bodies_with_ledger_history(&store_root(root).unwrap(), &event).unwrap();
    let keyed: Vec<_> = bodies
        .into_iter()
        .map(|b| (format!("PR-{pr}"), b))
        .collect();
    crate::gate::merge_hold_core::corpus_merge_gate(pr, &keyed, Some(head))
}

fn approve(root: &Path, pr: u64, head: &str) {
    let record = json!({"verdict": "approved", "reviewed_sha": head, "findings": [],
        "recorded_by": "reviewer:fresh-review-session",
        "recorded_at": chrono::Utc::now().to_rfc3339()});
    let change = ChangeKey::pr(pr);
    let record = record.as_object().unwrap().clone();
    let mut event = GateEvent::new(change, EventBody::Verdict { record });
    event.head = Some(head.to_string());
    store::append(&store_root(root).unwrap(), change, vec![event]).unwrap();
}

/// BUG-2786 AC1: an abandoned barrier reads as "needs review" in every
/// reconciliation, and a fresh approval at that head or a later one clears.
#[test]
fn an_abandoned_sweep_never_wedges_the_merge_gate() {
    use crate::gate::merge_hold_core::CorpusMergeGate;
    const LATER: &str = "2222222222222222222222222222222222222222";
    for (pr, head) in [(78, HEAD), (79, LATER)] {
        let (_tmp, root, _env) = setup(pr, json!({}));
        run(&root, pr, true, true, NO_FINALIZE);
        match gate(&root, pr, HEAD) {
            CorpusMergeGate::Refuse { message, .. } => {
                assert!(!message.contains("cannot be reconciled"), "{message}");
                assert!(message.contains("do not approve"), "{message}");
            }
            other => panic!("abandoned alone must need review: {other:?}"),
        }
        approve(&root, pr, head);
        let event = project::newest_verdict_event(&root, &format!("PR-{pr}")).unwrap();
        let bodies = rv::bodies_with_ledger_history(&store_root(&root).unwrap(), &event).unwrap();
        assert_eq!(
            rv::reconcile_artifacts_for_sha(bodies.iter().map(String::as_str), head),
            Ok(Some(rv::VerdictKind::Approved))
        );
        assert!(
            matches!(gate(&root, pr, head), CorpusMergeGate::Clear),
            "{:?}",
            gate(&root, pr, head)
        );
    }
}
