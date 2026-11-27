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

fn deferred() -> Value {
    json!({"findings": ["P3 follow-up"], "finding_dispositions":
        [{"severity": "minor", "disposition": "deferred", "rationale": "later"}]})
}

fn run(root: &Path, pr: u64, aged: bool, ended: bool, finalize: Fin) -> SweepExpiry {
    let late = SWEEP_ABANDON_AFTER + chrono::Duration::minutes(1);
    let now = chrono::Utc::now() + if aged { late } else { chrono::Duration::zero() };
    expire_with(
        root,
        ChangeKey::pr(pr),
        now,
        SWEEP_ABANDON_AFTER,
        &|_| ended,
        finalize,
    )
    .unwrap()
}

fn newest(root: &Path, pr: u64) -> Value {
    Value::Object(project::newest_verdict(root, &format!("PR-{pr}")).unwrap())
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
fn exited_reviewer_with_complete_draft_is_finalized_and_a_failed_finalize_expires() {
    let (_tmp, root, _env) = setup(72, deferred());
    let seen = std::cell::Cell::new(0);
    let finalize = |_: &GateEvent, approval: &JsonObj| {
        assert!(rv::disposition_verdict(approval).approves());
        assert!(approval.get("sweep_pending").is_none());
        assert_eq!(approval["reviewed_sha"], HEAD);
        seen.set(seen.get() + 1);
        Ok(())
    };
    assert_eq!(run(&root, 72, true, true, &finalize).finalized, 1);
    assert_eq!(seen.get(), 1);

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
