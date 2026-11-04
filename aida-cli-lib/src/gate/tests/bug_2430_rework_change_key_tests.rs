//! BUG-2430: a review recorded under the change key (`PR-N`) with no spec
//! binding in the ledger still reaches the rework brief when that change is
//! the spec's open pull request; the newer of a spec-keyed and a change-keyed
//! record answers; a refusal that cannot be read refuses the launch.
// trace:BUG-2430 | ai:claude
use crate::gate::ledger::{store, ChangeKey, EventBody, GateEvent};
use crate::gate::review_verdict::JsonObj;
use crate::queue_cmd::resolve_rework_findings;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const SPEC: &str = "STORY-2325";
const HEAD: &str = "34031ff4a06c1d24c08846619f87aaa66b31dec7";

fn root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    (tmp, root)
}

fn uncontained() -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)])
}

fn ts(secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339()
}

fn record(verdict: &str, at: &str, findings: &[&str]) -> JsonObj {
    let Value::Object(obj) = json!({
        "verdict": verdict,
        "reviewed_sha": HEAD,
        "recorded_by": "claude-reviewer",
        "recorded_at": at,
        "findings": findings,
    }) else {
        unreachable!()
    };
    obj
}

/// A verdict event under `change`; `spec` is `None` for the PR-3291 shape.
fn append(root: &Path, change: ChangeKey, spec: Option<&str>, rec: JsonObj) {
    let mut e = GateEvent::new(change, EventBody::Verdict { record: rec });
    e.head = Some(HEAD.to_string());
    e.spec = spec.map(str::to_string);
    store::append(root, change, vec![e]).unwrap();
}

/// PR-3291 shape: CHANGES REQUESTED recorded under `PR-3291` with no spec
/// and no by-spec index. Without the open-PR binding the brief had nothing;
/// with it the brief carries the findings.
#[test]
fn bug_2430_change_keyed_refusal_of_open_pr_reaches_the_brief() {
    let (_tmp, root) = root();
    let _env = uncontained();
    append(
        &root,
        ChangeKey::pr(3291),
        None,
        record(
            "request_changes",
            &ts(-60),
            &["migrate.rs:658 adds a 7th literal"],
        ),
    );
    assert_eq!(
        resolve_rework_findings(&root, SPEC, SPEC, None),
        Ok(None),
        "an unbound change is still not attributed without the open PR"
    );
    let block = resolve_rework_findings(&root, SPEC, SPEC, Some("PR-3291"))
        .unwrap()
        .expect("the open PR's findings");
    assert!(
        block.contains("migrate.rs:658 adds a 7th literal"),
        "{block}"
    );
    assert!(block.contains("PR #3291"), "{block}");
    // A different open PR does not borrow this change's review.
    assert_eq!(
        resolve_rework_findings(&root, SPEC, SPEC, Some("PR-9999")),
        Ok(None)
    );
}

/// The newer record answers: an older spec-keyed approval loses to a newer
/// change-keyed refusal, and a newer spec-keyed refusal beats an older
/// change-keyed one.
#[test]
fn bug_2430_newer_of_spec_and_change_key_wins() {
    let (_tmp, root) = root();
    let _env = uncontained();
    append(
        &root,
        ChangeKey::pr(3100),
        Some(SPEC),
        record("approved", &ts(-600), &[]),
    );
    append(
        &root,
        ChangeKey::pr(3291),
        None,
        record("request_changes", &ts(-60), &["newer change finding"]),
    );
    let block = resolve_rework_findings(&root, SPEC, SPEC, Some("PR-3291"))
        .unwrap()
        .expect("newer change-keyed refusal");
    assert!(block.contains("newer change finding"), "{block}");

    let (_tmp2, root2) = self::root();
    append(
        &root2,
        ChangeKey::pr(3291),
        None,
        record("request_changes", &ts(-600), &["older change finding"]),
    );
    append(
        &root2,
        ChangeKey::pr(3100),
        Some(SPEC),
        record("request_changes", &ts(-60), &["newer spec finding"]),
    );
    let block = resolve_rework_findings(&root2, SPEC, SPEC, Some("PR-3291"))
        .unwrap()
        .expect("newer spec-keyed refusal");
    assert!(block.contains("newer spec finding"), "{block}");
    assert!(!block.contains("older change finding"), "{block}");
}

/// A newer approval on the open PR clears the older change-keyed refusal.
#[test]
fn bug_2430_newer_open_pr_approval_yields_no_findings() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let pr = ChangeKey::pr(3291);
    append(
        &root,
        pr,
        None,
        record("request_changes", &ts(-600), &["x"]),
    );
    append(&root, pr, None, record("approved", &ts(-60), &[]));
    assert_eq!(
        resolve_rework_findings(&root, SPEC, SPEC, Some("PR-3291")),
        Ok(None)
    );
}

/// The open PR's verdict exists but cannot be read: the launch refuses.
#[test]
fn bug_2430_unreadable_open_pr_review_refuses() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let dir = root.join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("PR-3291.json"), "{ not json").unwrap();
    let err = resolve_rework_findings(&root, SPEC, SPEC, Some("PR-3291"))
        .expect_err("an unreadable review of the open PR refuses");
    assert!(err.contains("PR-3291"), "{err}");
}

/// Two ledger-bound changes each holding a refusal, no spec-keyed record:
/// the findings cannot be chosen, so the launch refuses.
#[test]
fn bug_2430_two_bound_refusals_refuse() {
    let (_tmp, root) = root();
    let _env = uncontained();
    for n in [3288_u64, 3291] {
        let pr = ChangeKey::pr(n);
        append(&root, pr, None, record("request_changes", &ts(-60), &["f"]));
        crate::gate::ledger::project::index_spec(&root, SPEC, pr).unwrap();
    }
    let err = resolve_rework_findings(&root, SPEC, SPEC, None)
        .expect_err("two current refusals are ambiguous");
    assert!(err.contains("PR-3288") && err.contains("PR-3291"), "{err}");
}
