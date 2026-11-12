//! BUG-2403: a phase-3 handshake / drain stamp is the same act of review as
//! the record it overlays, so it keeps that record's record-time receipt;
//! retained receipt-less overlays answer with their proved ordinary origin;
//! nothing unproved collapses and no receipt is ever filled.
// trace:BUG-2403 | ai:claude
// mandate-gate
use crate::gate::ledger::project::{self, PHASE3_MODE};
use crate::gate::ledger::{store, ChangeKey, EventBody, GateEvent};
use crate::gate::review_verdict::{self as rv, JsonObj, ATTESTATION_KEY};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const HEAD: &str = "1111111111111111111111111111111111111111";
const OTHER: &str = "2222222222222222222222222222222222222222";
const SPEC: &str = "STORY-2204";

fn root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    (tmp, root)
}

fn uncontained() -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)])
}

/// The ordinary record `aida review record` writes: verdict, full head,
/// recorder, time, attestation and its record-time receipt.
fn origin(verdict: &str, head: &str, at: &str) -> JsonObj {
    let Value::Object(obj) = json!({
        "verdict": verdict,
        "reviewed_sha": head,
        "reviewed_branch": "story-2204-work",
        "summary": "reviewed",
        "recorded_by": "codex-reviewer-08",
        "recorded_at": at,
        ATTESTATION_KEY: {"sha": head, "seat": "reviewer", "grant_id": "g-1"},
        "release_grant": {
            "id": "g-1", "subject": "s", "session_id": "sess",
            "recorded_by": "codex-reviewer-08"
        },
    }) else {
        unreachable!()
    };
    obj
}

/// What the pre-fix `--pr` writer produced from `origin`: `mode` added,
/// receipt dropped, retained history possibly represented differently.
fn overlay_of(origin: &JsonObj) -> JsonObj {
    let mut o = origin.clone();
    o.remove("release_grant");
    o.insert("mode".into(), json!(PHASE3_MODE));
    o.insert(
        "rounds".into(),
        json!([{"verdict": "request_changes", "reviewed_sha": OTHER}]),
    );
    o
}

fn event(change: ChangeKey, spec: &str, rec: JsonObj) -> GateEvent {
    let mut e = GateEvent::new(change, EventBody::Verdict { record: rec });
    e.head = e
        .verdict_record()
        .and_then(|r| r.get("reviewed_sha"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    e.spec = Some(spec.to_string());
    e
}

fn append(root: &Path, change: ChangeKey, spec: &str, recs: Vec<JsonObj>) {
    let events = recs.into_iter().map(|r| event(change, spec, r)).collect();
    store::append(root, change, events).unwrap();
}

fn selected(root: &Path, key: &str) -> JsonObj {
    project::newest_verdict_event(root, key)
        .and_then(|e| e.verdict_record().cloned())
        .expect("a selected verdict")
}

fn ts(secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339()
}

/// Production shape PR-3185 #22/#23 (and #20/#21, whose rounds differ):
/// the change key and the spec label both select the receipt-bearing origin;
/// every event stays in the retained history.
#[test]
fn bug_2403_historical_phase3_duplicate_prefers_proved_origin() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let pr = ChangeKey::pr(3185);
    let o = origin("approved", HEAD, &ts(-5));
    append(&root, pr, SPEC, vec![o.clone(), overlay_of(&o)]);

    for key in ["PR-3185", SPEC] {
        let got = selected(&root, key);
        assert_eq!(got, o, "{key} selects the ordinary origin");
        assert!(got.get("release_grant").is_some());
    }
    assert_eq!(project::verdict_records(&root, "PR-3185").len(), 2);
    let rendered = rv::current_verdict(&root, "PR-3185").unwrap();
    assert!(rendered.body.contains("release_grant"), "{}", rendered.body);
}

type Mutation = Box<dyn Fn(&mut JsonObj, &mut JsonObj)>;

fn case(f: impl Fn(&mut JsonObj, &mut JsonObj) + 'static) -> Mutation {
    Box::new(f)
}

/// Every unproved pairing keeps the overlay as the answer: it never borrows
/// the origin's receipt or standing.
#[test]
fn bug_2403_unproved_overlays_do_not_collapse() {
    let base = origin("approved", HEAD, &ts(-5));
    let cases: Vec<(&str, Mutation)> = vec![
        (
            "different recorder",
            case(|_: &mut JsonObj, v: &mut JsonObj| {
                v.insert("recorded_by".into(), json!("other"));
            }),
        ),
        (
            "different time",
            case(|_: &mut JsonObj, v: &mut JsonObj| {
                v.insert("recorded_at".into(), json!(ts(-1)));
            }),
        ),
        (
            "different verdict",
            case(|_: &mut JsonObj, v: &mut JsonObj| {
                v.insert("verdict".into(), json!("request_changes"));
            }),
        ),
        (
            "different head",
            case(|_: &mut JsonObj, v: &mut JsonObj| {
                v.insert("reviewed_sha".into(), json!(OTHER));
            }),
        ),
        (
            "different findings",
            case(|_: &mut JsonObj, v: &mut JsonObj| {
                v.insert("findings".into(), json!(["x"]));
            }),
        ),
        (
            "different summary",
            case(|_: &mut JsonObj, v: &mut JsonObj| {
                v.insert("summary".into(), json!("other"));
            }),
        ),
        (
            "unknown field",
            case(|_: &mut JsonObj, v: &mut JsonObj| {
                v.insert("ci_terminal_green".into(), json!(true));
            }),
        ),
        (
            "present mismatched receipt",
            case(|_: &mut JsonObj, v: &mut JsonObj| {
                v.insert("release_grant".into(), json!({"id": "g-2"}));
            }),
        ),
        (
            "missing attestation",
            case(|o: &mut JsonObj, v: &mut JsonObj| {
                o.remove(ATTESTATION_KEY);
                v.remove(ATTESTATION_KEY);
            }),
        ),
        (
            "origin without receipt",
            case(|o: &mut JsonObj, _: &mut JsonObj| {
                o.remove("release_grant");
            }),
        ),
        (
            "origin is itself phase-3",
            case(|o: &mut JsonObj, _: &mut JsonObj| {
                o.insert("mode".into(), json!(PHASE3_MODE));
            }),
        ),
        (
            "overlay is not phase-3",
            case(|_: &mut JsonObj, v: &mut JsonObj| {
                v.insert("mode".into(), json!("deep"));
            }),
        ),
    ];
    for (name, mutate) in cases {
        let (_tmp, root) = root();
        let _env = uncontained();
        let (mut o, mut v) = (base.clone(), overlay_of(&base));
        mutate(&mut o, &mut v);
        append(&root, ChangeKey::pr(7), SPEC, vec![o, v.clone()]);
        assert_eq!(
            selected(&root, "PR-7"),
            v,
            "{name}: the overlay stays the answer"
        );
    }
}

/// A different spec label on the same PR, or an intervening/later refusal,
/// cannot collapse: a real refusal is never skipped.
#[test]
fn bug_2403_refusals_and_other_specs_are_never_skipped() {
    let o = origin("approved", HEAD, &ts(-10));
    let mut refusal = origin("request_changes", HEAD, &ts(-8));
    refusal.remove("release_grant");
    refusal.insert("findings".into(), json!(["still broken"]));

    // Intervening refusal between origin and overlay.
    let (_tmp, root) = root();
    let _env = uncontained();
    let pr = ChangeKey::pr(8);
    append(&root, pr, SPEC, vec![o.clone(), refusal.clone()]);
    append(&root, pr, SPEC, vec![overlay_of(&o)]);
    assert_eq!(selected(&root, "PR-8"), overlay_of(&o));

    // A refusal after the pair stays the answer.
    let (_tmp2, root) = self::root();
    let pr = ChangeKey::pr(9);
    append(
        &root,
        pr,
        SPEC,
        vec![o.clone(), overlay_of(&o), refusal.clone()],
    );
    assert_eq!(selected(&root, "PR-9"), refusal);
    assert_eq!(selected(&root, SPEC), refusal);

    // An unrelated spec's origin on the shared PR is not this overlay's.
    let (_tmp3, root) = self::root();
    let pr = ChangeKey::pr(10);
    append(&root, pr, "STORY-9999", vec![o.clone()]);
    append(&root, pr, SPEC, vec![overlay_of(&o)]);
    assert_eq!(selected(&root, "PR-10"), overlay_of(&o));

    // PR-N and MR-N stay distinct changes.
    let (_tmp4, root) = self::root();
    append(&root, ChangeKey::pr(11), SPEC, vec![o.clone()]);
    let mr = ChangeKey::parse("MR-11").unwrap();
    append(&root, mr, SPEC, vec![overlay_of(&o)]);
    assert_eq!(selected(&root, "MR-11"), overlay_of(&o));
    assert_eq!(selected(&root, "PR-11"), o);
}

#[test]
fn bug_2403_same_origin_receipt_requires_the_same_act() {
    let o = origin("approved", HEAD, "2026-11-01T08:53:00+00:00");
    let mut v = o.clone();
    v.remove("release_grant");
    v.insert("mode".into(), json!(PHASE3_MODE));
    assert_eq!(
        rv::same_origin_receipt(&o, &v),
        o.get("release_grant").cloned()
    );

    let mut short = o.clone();
    short.insert("reviewed_sha".into(), json!("1111111"));
    let mut vs = v.clone();
    vs.insert("reviewed_sha".into(), json!("1111111"));
    assert_eq!(
        rv::same_origin_receipt(&short, &vs),
        None,
        "abbreviated head"
    );

    for (k, val) in [
        ("reviewed_sha", json!(OTHER)),
        ("recorded_by", json!("aida drain reviewer")),
        ("recorded_at", json!("2026-11-01T09:00:00+00:00")),
        ("verdict", json!("request_changes")),
        (
            ATTESTATION_KEY,
            json!({"sha": HEAD, "seat": "reviewer", "grant_id": "g-9"}),
        ),
    ] {
        let mut changed = v.clone();
        changed.insert(k.into(), val);
        assert_eq!(rv::same_origin_receipt(&o, &changed), None, "{k}");
    }
    let mut no_receipt = o.clone();
    no_receipt.remove("release_grant");
    assert_eq!(
        rv::same_origin_receipt(&no_receipt, &v),
        None,
        "never filled"
    );
    let mut foreign = o.clone();
    foreign.insert(
        "release_grant".into(),
        json!({"id": "g-1", "recorded_by": "someone"}),
    );
    assert_eq!(
        rv::same_origin_receipt(&foreign, &v),
        None,
        "receipt names another recorder"
    );
}

/// The drain's provenance-only stamp at the same head keeps the reviewer's
/// receipt, recorder and time; at a moved head it blesses nothing.
#[test]
fn bug_2403_drain_stamp_keeps_the_same_head_receipt_only() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let path = rv::verdict_path(&root, "PR-2403");
    let o = origin("approved", HEAD, &ts(-5));
    rv::write_verdict_object(&path, &o).unwrap();

    crate::pr_merge_gate::stamp_pr_review_verdict(
        &root,
        2403,
        Some(HEAD),
        None,
        Some(HEAD),
        Some(true),
    )
    .unwrap();
    let got = selected(&root, "PR-2403");
    assert_eq!(got.get("release_grant"), o.get("release_grant"));
    assert_eq!(got.get("recorded_by"), o.get("recorded_by"));
    assert_eq!(got.get("recorded_at"), o.get("recorded_at"));
    assert_eq!(got.get("ci_terminal_green"), Some(&json!(true)));
    // Review finding 4: the kept origin is not also archived as a round of
    // its own, so one act of review is never counted twice.
    assert!(got.get("rounds").is_none(), "{got:?}");

    crate::pr_merge_gate::stamp_pr_review_verdict(&root, 2403, Some(OTHER), None, None, None)
        .unwrap();
    let moved = selected(&root, "PR-2403");
    assert_eq!(moved.get("reviewed_sha"), Some(&json!(OTHER)));
    assert!(
        moved.get("release_grant").is_none(),
        "a moved head keeps no receipt"
    );
    assert_eq!(
        moved.get("recorded_by"),
        Some(&json!("aida drain reviewer"))
    );
}

/// A git project root under a hermetic reviewer seat whose `--pr`
/// handshake anchor is the project's own `PR-N` file; `origin` (when given)
/// is set as the remote so the forge kind is detected from it. Runs one
/// granted `aida review record <spec> --pr N` approval, with `between` run
/// after the spec write and before the handshake is built.
fn record_pr(
    origin: Option<&str>,
    spec: &str,
    pr: u64,
    between: Option<crate::AfterSpecRecordHook>,
) -> (tempfile::TempDir, PathBuf, crate::test_env::AmbientGuard) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let root_str = root.to_str().unwrap().to_string();
    let ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &root,
        "reviewer",
        &[],
        &[
            ("AIDA_PROJECT_ROOT", Some(root_str.as_str())),
            ("AIDA_DRIVE_ROOT", Some(root_str.as_str())),
            ("AIDA_REVIEW_VERDICT_FILE", None),
            ("AIDA_CONTAINED", None),
            ("AIDA_SESSION_ID", Some("independent-2403")),
        ],
    );
    let head = crate::test_git::commit_fixture(&root);
    if let Some(url) = origin {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&root)
            .args(["remote", "add", "origin", url])
            .output()
            .unwrap();
        assert!(out.status.success());
    }
    let branch = format!("{}-work", spec.to_ascii_lowercase());
    let subject = crate::ReviewPrSubject {
        head_ref: branch.clone(),
        commit_subjects: vec![],
    };
    let reviewer = crate::gate::review_authority::IdentitySource {
        session_ids: vec!["independent-2403".into()],
        ..Default::default()
    };
    crate::AFTER_SPEC_RECORD_HOOK.with(|h| *h.borrow_mut() = between);
    let result = crate::with_review_pr_subject(subject, || {
        crate::gate::review_authority::with_identity_source(reviewer, || {
            crate::handle_review_record_at(
                root.clone(),
                spec,
                "approved",
                Some(&head),
                Some(&branch),
                Some("looks right"),
                &[],
                &[],
                Some(pr),
            )
        })
    });
    crate::AFTER_SPEC_RECORD_HOOK.with(|h| *h.borrow_mut() = None);
    result.expect("a granted approval records");
    (tmp, root, ambient)
}

fn handshake_of(root: &Path, pr_key: &str) -> JsonObj {
    project::verdict_records(root, pr_key)
        .into_iter()
        .rev()
        .find(|r| r.get("mode").and_then(|m| m.as_str()) == Some(PHASE3_MODE))
        .expect("the phase-3 handshake is on the change's ledger")
}

/// Production path: a granted `aida review record <spec> --pr N` writes the
/// handshake with the SAME receipt the spec record carries, so the PR key,
/// the spec key and auto-release's projected read all see it.
#[test]
fn bug_2403_record_pr_handshake_keeps_origin_release_receipt() {
    let (_tmp, root, _env) = record_pr(None, "BUG-24031", 24031, None);
    let spec_record = selected(&root, "BUG-24031");
    let receipt = spec_record
        .get("release_grant")
        .cloned()
        .expect("the granted record carries its record-time receipt");
    let handshake = handshake_of(&root, "PR-24031");
    assert_eq!(handshake.get("release_grant"), Some(&receipt));
    for k in [
        "reviewed_sha",
        "recorded_by",
        "recorded_at",
        ATTESTATION_KEY,
    ] {
        assert_eq!(handshake.get(k), spec_record.get(k), "{k}");
    }
    let pr_current: Value =
        serde_json::from_str(&rv::current_verdict(&root, "PR-24031").unwrap().body).unwrap();
    assert_eq!(pr_current.get("release_grant"), Some(&receipt));
}

/// Review finding 4b: on a GitLab-origin project the `--pr` handshake keeps
/// its existing mapping contract (the orchestrator's `PR-N` anchor; no
/// `MR-N` record is invented) and the same-origin receipt.
#[test]
fn bug_2403_gitlab_record_pr_handshake_keeps_mapping_and_receipt() {
    let (_tmp, root, _env) = record_pr(
        Some("https://gitlab.com/example/aida-fixture.git"),
        "BUG-24032",
        24032,
        None,
    );
    assert_eq!(
        crate::forge::resolve_open_change_forge_kind(&root),
        crate::forge::ForgeKind::GitLab
    );
    let spec_record = selected(&root, "BUG-24032");
    let receipt = spec_record.get("release_grant").cloned().unwrap();
    let handshake = handshake_of(&root, "PR-24032");
    assert_eq!(handshake.get("release_grant"), Some(&receipt));
    assert!(
        project::verdict_records(&root, "MR-24032").is_empty(),
        "PR-N and MR-N stay distinct changes"
    );
}

/// Review finding 4c: a concurrent reviewer replaces the spec record
/// between the handler's write and its handshake build. The handshake still
/// carries THIS handler's act of review (recorder, time, head, receipt), not
/// the replacement it would have re-read.
#[test]
fn bug_2403_record_pr_handshake_ignores_a_concurrent_origin_replacement() {
    let replaced: std::rc::Rc<std::cell::RefCell<Option<JsonObj>>> = Default::default();
    let seen = replaced.clone();
    let hook: crate::AfterSpecRecordHook = Box::new(move |root: &Path, spec: &str| {
        let path = rv::verdict_path(root, spec);
        let mut other = origin("request_changes", OTHER, &ts(0));
        other.remove("release_grant");
        other.insert("recorded_by".into(), json!("concurrent-reviewer"));
        rv::write_verdict_object(&path, &other).unwrap();
        *seen.borrow_mut() = Some(other);
    });
    let (_tmp, root, _env) = record_pr(None, "BUG-24033", 24033, Some(hook));
    assert!(replaced.borrow().is_some(), "the replacement ran");
    let handshake = handshake_of(&root, "PR-24033");
    assert_eq!(handshake.get("verdict"), Some(&json!("approved")));
    assert_ne!(
        handshake.get("recorded_by"),
        Some(&json!("concurrent-reviewer"))
    );
    assert_ne!(handshake.get("reviewed_sha"), Some(&json!(OTHER)));
    assert!(
        handshake.get("release_grant").is_some(),
        "the handler's own receipt, never the replacement's absence"
    );
    // The replacement really landed (it is retained in the spec's history);
    // the handshake did not borrow its provenance.
    assert!(project::verdict_records(&root, "BUG-24033")
        .iter()
        .any(|r| r.get("recorded_by") == Some(&json!("concurrent-reviewer"))));
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_phase3_effective_wait_closes_in_raw_and_effective_projections() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let pr = ChangeKey::pr(3185);
    let o = origin("waiting", HEAD, &ts(-5));
    append(&root, pr, SPEC, vec![o.clone(), overlay_of(&o)]);
    for key in [SPEC, "PR-3185"] {
        let source = rv::current_verdict(&root, key).unwrap();
        assert_eq!(serde_json::from_str::<JsonObj>(&source.body).unwrap(), o);
        assert!(rv::close_terminal_wait(&root, key, pr, &source).unwrap());
        assert!(rv::read_recorded_verdict(&root, key).unwrap().is_closed());
        assert!(selected(&root, key).contains_key("closed_by_merge"));
        crate::queue_cmd::refuse_waiting_review(&root, key, key, None).unwrap();
    }
    append(&root, pr, SPEC, vec![origin("waiting", HEAD, &ts(0))]);
    for key in [SPEC, "PR-3185"] {
        assert!(!rv::read_recorded_verdict(&root, key).unwrap().is_closed());
    }
}
