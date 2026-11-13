//! Write-ahead bindings: bound-only selection, the completeness marker, the
//! bootstrap and the binding/append boundary.
// trace:BUG-2531 | ai:claude
// mandate-gate

use super::super::checked::{self, discovery_work_tests as work, Budget};
use super::super::event::{self, EventBody, GateEvent};
use super::super::{ledger_dir, project, store, ChangeKey, GateSpec};
use super::*;
use crate::gate::review_verdict::{JsonObj, RecorderAttestation, ATTESTATION_KEY};
use std::cell::Cell;

const HEAD: &str = "1111111111111111111111111111111111111111";

fn root() -> (tempfile::TempDir, std::path::PathBuf) {
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

/// A fingerprintable review record (every provenance field present).
fn record(verdict: &str, secs: i64) -> JsonObj {
    let mut obj = JsonObj::new();
    for (k, v) in [
        ("verdict", verdict.to_string()),
        ("reviewed_sha", HEAD.to_string()),
        ("recorded_by", "reviewer".to_string()),
        ("recorded_at", ts(secs)),
    ] {
        obj.insert(k.to_string(), serde_json::Value::String(v));
    }
    let actor = RecorderAttestation {
        sha: HEAD.to_string(),
        seat: Some("reviewer".to_string()),
        grant_id: Some("g-1".to_string()),
        ..Default::default()
    };
    obj.insert(
        ATTESTATION_KEY.to_string(),
        serde_json::to_value(actor).unwrap(),
    );
    obj
}

fn append(root: &Path, change: ChangeKey, spec: &str, rec: &JsonObj) {
    let spec = GateSpec::with_key(spec, None);
    store::append_one(root, event::verdict_event(root, change, rec, Some(&spec))).unwrap();
}

/// A ledger written before bindings existed: raw lines, no binding.
fn legacy_ledger(root: &Path, n: u64, spec: &str, payload: usize) -> u64 {
    let mut rec = record("approved", -600);
    rec.insert("summary".into(), serde_json::json!("x".repeat(payload)));
    let mut e = GateEvent::new(ChangeKey::pr(n), EventBody::Verdict { record: rec });
    e.spec = Some(spec.into());
    e.head = Some(HEAD.into());
    e.seq = 1;
    e.at = ts(-600);
    let body = format!("{}\n", serde_json::to_string(&e).unwrap());
    std::fs::create_dir_all(ledger_dir(root)).unwrap();
    std::fs::write(store::ledger_file(root, ChangeKey::pr(n)), &body).unwrap();
    body.len() as u64
}

fn newest_verdict(events: &[GateEvent]) -> Option<String> {
    project::newest_of(events.to_vec())
        .and_then(|e| e.verdict_record().cloned())
        .and_then(|r| r.get("verdict").and_then(|v| v.as_str()).map(String::from))
}

fn len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

#[test]
fn bug_2531_fresh_writer_binds_and_certifies_an_empty_ledger() {
    let (_tmp, root) = root();
    let _env = uncontained();
    assert_eq!(marker_state(&root).unwrap(), MarkerState::NoLedger);
    append(&root, ChangeKey::pr(1), "BUG-1", &record("approved", 0));
    assert_eq!(marker_state(&root).unwrap(), MarkerState::Complete);
    assert!(bindings_dir(&root).join("BUG-1/PR-1").is_file());
    let mut d = checked::discover(&root).unwrap().unwrap();
    let got = d
        .events_for(&["bug-1".into()], &mut Budget::default())
        .unwrap();
    assert_eq!(newest_verdict(&got[0].1).as_deref(), Some("approved"));
}

/// (a) after bootstrap, a selection reads the marker, the label's index and
/// its one bound ledger: unrelated history, however large, is not read or
/// parsed, and a tiny work/selection budget suffices.
#[test]
fn bug_2531_selection_after_bootstrap_reads_only_bound_ledgers() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let mut unrelated = 0;
    for n in 100..140 {
        unrelated += legacy_ledger(&root, n, "STORY-UNRELATED-1", 64 * 1024);
    }
    append(
        &root,
        ChangeKey::pr(7),
        "BUG-7",
        &record("request-changes", 0),
    );
    assert_eq!(marker_state(&root).unwrap(), MarkerState::Missing);
    let done = bootstrap(&root).unwrap();
    assert_eq!(done.ledgers, 41);
    assert_eq!(marker_state(&root).unwrap(), MarkerState::Complete);

    let bound = store::ledger_file(&root, ChangeKey::pr(7));
    let expected =
        len(&bound) + len(&marker_file(&root)) + len(&ledger_dir(&root).join("by-spec/BUG-7.json"));
    let lines = std::fs::read_to_string(&bound).unwrap().lines().count();
    work::READ_BYTES.with(|n| n.set(0));
    work::PARSE_ATTEMPTS.with(|n| n.set(0));
    let small = expected * 2;
    assert!(small < unrelated);
    let mut d = checked::discover_with_budget(&root, Budget::with_limits(small, 100))
        .unwrap()
        .unwrap();
    let got = d
        .events_for(&["BUG-7".into()], &mut Budget::with_limits(small, 100))
        .unwrap();
    assert_eq!(
        newest_verdict(&got[0].1).as_deref(),
        Some("request-changes")
    );
    assert_eq!(work::READ_BYTES.with(Cell::get), expected);
    assert_eq!(work::PARSE_ATTEMPTS.with(Cell::get), lines);
}

/// (b) a ledger with change history and no marker, or an unsupported one,
/// fails closed and names the fix; the fix makes selection work.
#[test]
fn bug_2531_missing_or_unsupported_marker_fails_closed_naming_the_fix() {
    let (_tmp, root) = root();
    let _env = uncontained();
    legacy_ledger(&root, 5, "BUG-5", 16);
    let err = checked::discover(&root).err().unwrap();
    assert!(err.0.contains(FIX_COMMAND), "{err}");
    let mut events = store::read_file(&root, ChangeKey::pr(5));
    assert!(checked::filter_miskeyed(&root, events.clone(), &mut Budget::default()).is_err());
    // The lossy readers withhold every review copy rather than answer.
    events = checked::filter_miskeyed_or_fail_closed(&root, events);
    assert!(events.iter().all(|e| e.verdict_record().is_none()));

    let findings = crate::doctor_cmd::scan_gate_ledger_bindings(&root);
    assert_eq!(findings.len(), 1);
    assert!(findings[0].action.contains(FIX_COMMAND));
    assert!(findings[0].safe_heal);
    bootstrap(&root).unwrap();
    assert!(crate::doctor_cmd::scan_gate_ledger_bindings(&root).is_empty());
    let mut d = checked::discover(&root).unwrap().unwrap();
    let got = d
        .events_for(&["BUG-5".into()], &mut Budget::default())
        .unwrap();
    assert_eq!(newest_verdict(&got[0].1).as_deref(), Some("approved"));

    std::fs::write(marker_file(&root), br#"{"format":99}"#).unwrap();
    let err = checked::discover(&root).err().unwrap();
    assert!(err.0.contains("unsupported format 99"), "{err}");
    assert!(err.0.contains(FIX_COMMAND), "{err}");
    assert!(matches!(
        marker_state(&root).unwrap(),
        MarkerState::Unsupported(_)
    ));
    bootstrap(&root).unwrap();
    assert!(checked::discover(&root).is_ok());
}

/// (c) a missing or stale by-spec hint cannot hide a newer refusal: the
/// bindings name every change recorded against the spec.
#[test]
fn bug_2531_stale_or_missing_hint_still_finds_the_newer_refusal() {
    let (_tmp, root) = root();
    let _env = uncontained();
    append(&root, ChangeKey::pr(1), "BUG-9", &record("approved", -60));
    append(
        &root,
        ChangeKey::pr(2),
        "BUG-9",
        &record("request-changes", 0),
    );
    let index = ledger_dir(&root).join("by-spec/BUG-9.json");
    for hint in [Some(r#"{"changes":["PR-1"]}"#), None] {
        match hint {
            Some(body) => std::fs::write(&index, body).unwrap(),
            None => std::fs::remove_file(&index).unwrap(),
        }
        let mut d = checked::discover(&root).unwrap().unwrap();
        let got = d
            .events_for(&["BUG-9".into()], &mut Budget::default())
            .unwrap();
        assert_eq!(
            newest_verdict(&got[0].1).as_deref(),
            Some("request-changes"),
            "{hint:?}"
        );
        let (_, changes) = d.spec_entries("BUG-9", &mut Budget::default()).unwrap();
        assert!(changes.contains(&ChangeKey::pr(2)), "{hint:?}");
        // The single-label reader sees it through the bindings, too.
        assert_eq!(
            project::newest_verdict(&root, "BUG-9").and_then(|r| r.get("verdict").cloned()),
            Some(serde_json::json!("request-changes")),
            "{hint:?}"
        );
    }
    // A deleted binding is an inconsistency once its ledger is read.
    std::fs::write(&index, r#"{"changes":["PR-1","PR-2"]}"#).unwrap();
    std::fs::remove_file(bindings_dir(&root).join("BUG-9/PR-2")).unwrap();
    let mut d = checked::discover(&root).unwrap().unwrap();
    let err = d
        .events_for(&["BUG-9".into()], &mut Budget::default())
        .unwrap_err();
    assert!(err.0.contains("without a binding"), "{err}");
    assert!(err.0.contains(FIX_COMMAND), "{err}");
}

/// (d) a binding that cannot be written fails the append, and nothing is
/// appended.
#[test]
fn bug_2531_failed_binding_write_blocks_the_append() {
    let (_tmp, root) = root();
    let _env = uncontained();
    append(&root, ChangeKey::pr(1), "BUG-1", &record("approved", -60));
    // A non-directory where the spec's binding directory belongs.
    std::fs::write(bindings_dir(&root).join("BUG-2"), b"").unwrap();
    let change = ChangeKey::pr(3);
    let spec = GateSpec::with_key("BUG-2", None);
    let event = event::verdict_event(&root, change, &record("request-changes", 0), Some(&spec));
    let err = store::append_one(&root, event).unwrap_err();
    assert!(err.to_string().contains("not a plain directory"), "{err}");
    assert!(!store::ledger_file(&root, change).exists());
    // A linked binding refuses as well; the existing ledger is unchanged.
    std::fs::remove_file(bindings_dir(&root).join("BUG-2")).unwrap();
    std::fs::create_dir(bindings_dir(&root).join("BUG-2")).unwrap();
    std::os::unix::fs::symlink("/dev/null", bindings_dir(&root).join("BUG-2/PR-1")).unwrap();
    let before = std::fs::read(store::ledger_file(&root, ChangeKey::pr(1))).unwrap();
    let event = event::verdict_event(
        &root,
        ChangeKey::pr(1),
        &record("request-changes", 0),
        Some(&spec),
    );
    assert!(store::append_one(&root, event).is_err());
    assert_eq!(
        std::fs::read(store::ledger_file(&root, ChangeKey::pr(1))).unwrap(),
        before
    );
}

/// (e) an interrupted bootstrap publishes no marker, so selection refuses;
/// a rerun completes and removes bindings whose ledger is gone.
#[test]
fn bug_2531_interrupted_bootstrap_refuses_and_a_rerun_completes() {
    let (_tmp, root) = root();
    let _env = uncontained();
    legacy_ledger(&root, 1, "BUG-1", 16);
    legacy_ledger(&root, 2, "BUG-1", 16);
    std::fs::create_dir_all(bindings_dir(&root).join("BUG-1")).unwrap();
    std::fs::write(bindings_dir(&root).join("BUG-1/PR-404"), b"").unwrap();
    STOP_BEFORE_MARKER.with(|s| s.set(true));
    assert!(bootstrap(&root).is_err());
    STOP_BEFORE_MARKER.with(|s| s.set(false));
    assert!(bindings_dir(&root).join("BUG-1/PR-1").is_file());
    assert_eq!(marker_state(&root).unwrap(), MarkerState::Missing);
    let err = checked::discover(&root).err().unwrap();
    assert!(err.0.contains(FIX_COMMAND), "{err}");
    let done = bootstrap(&root).unwrap();
    assert_eq!((done.ledgers, done.bindings, done.pruned), (2, 2, 0));
    assert!(!bindings_dir(&root).join("BUG-1/PR-404").exists());
    let mut d = checked::discover(&root).unwrap().unwrap();
    let got = d
        .events_for(&["BUG-1".into()], &mut Budget::default())
        .unwrap();
    assert_eq!(got[0].1.len(), 2);
    // A binding whose ledger is missing refuses until the bootstrap reruns.
    std::fs::write(bindings_dir(&root).join("BUG-1/PR-405"), b"").unwrap();
    let mut d = checked::discover(&root).unwrap().unwrap();
    let err = d
        .events_for(&["BUG-1".into()], &mut Budget::default())
        .unwrap_err();
    assert!(err.0.contains("whose ledger is missing"), "{err}");
    assert_eq!(bootstrap(&root).unwrap().pruned, 1);
}

/// (f) mis-key filtering reads only the changes bound to the candidate's
/// spec: a foreign copy is still dropped, while an unrelated malformed
/// ledger is never read and cannot make the filter fail closed.
#[test]
fn bug_2531_miskey_filtering_uses_only_bound_changes() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let own = record("approved", -60);
    append(&root, ChangeKey::pr(1), "BUG-2517", &own);
    let mut foreign = record("waiting", 0);
    append(&root, ChangeKey::pr(1), "BUG-2517", &foreign);
    foreign.insert("mode".into(), serde_json::json!(project::PHASE3_MODE));
    append(&root, ChangeKey::pr(2), "BUG-2517", &foreign);
    append(&root, ChangeKey::pr(3), "BUG-OTHER", &record("approved", 0));
    std::fs::write(store::ledger_file(&root, ChangeKey::pr(9999)), b"invalid\n").unwrap();

    project::OWNERSHIP_WORK.with(|w| w.set((0, 0)));
    let events =
        checked::read_change_events(&root, ChangeKey::pr(1), &mut Budget::default()).unwrap();
    assert_eq!(newest_verdict(&events).as_deref(), Some("approved"));
    // Visited: the two ledgers bound to BUG-2517 (one legacy import each,
    // two verdicts on PR-1, one on PR-2), never PR-3 or PR-9999.
    assert_eq!(project::OWNERSHIP_WORK.with(|w| w.get()), (1, 5));
    let raw = store::read_file(&root, ChangeKey::pr(1));
    let kept = checked::filter_miskeyed_or_fail_closed(&root, raw);
    assert_eq!(
        kept.iter().filter(|e| e.verdict_record().is_some()).count(),
        1
    );
    assert_eq!(
        project::newest_verdict(&root, "PR-1").and_then(|r| r.get("verdict").cloned()),
        Some(serde_json::json!("approved"))
    );
    // The spec's own selection answers with the owner's phase-3 record.
    let mut d = checked::discover(&root).unwrap().unwrap();
    let got = d
        .events_for(&["BUG-2517".into()], &mut Budget::default())
        .unwrap();
    let newest = project::newest_of(got[0].1.clone()).unwrap();
    assert_eq!(newest.change, "PR-2");
}
