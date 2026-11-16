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
    work::FULL_READ_ANSWERS.with(|n| n.set(0));
    let small = expected * 2;
    assert!(small < unrelated);
    let mut d = checked::discover_with_budget(&root, Budget::with_limits(small, 100))
        .unwrap()
        .unwrap();
    // The normal two-label selection: the spec's other alias was never
    // recorded, so it has neither bindings nor a hint. That is no evidence
    // of a lost binding, and it reads nothing.
    let got = d
        .events_for(
            &["BUG-7".into(), "STORY-ALIAS-7".into()],
            &mut Budget::with_limits(small, 100),
        )
        .unwrap();
    assert_eq!(
        newest_verdict(&got[0].1).as_deref(),
        Some("request-changes")
    );
    assert!(got[1].1.is_empty());
    // Exactly the marker, the recorded label's hint and its one bound
    // ledger were read: no other change ledger, for either label.
    assert_eq!(work::READ_BYTES.with(Cell::get), expected);
    assert_eq!(work::PARSE_ATTEMPTS.with(Cell::get), lines);
    assert_eq!(work::FULL_READ_ANSWERS.with(Cell::get), 0);
    // The spec-keyed reader takes the same path for both labels.
    assert!(project::newest_verdict(&root, "BUG-7").is_some());
    assert_eq!(project::newest_verdict(&root, "STORY-ALIAS-7"), None);
    assert_eq!(work::FULL_READ_ANSWERS.with(Cell::get), 0);

    // The bootstrap leaves history it bound as a writer would: the label's
    // hint names its bound changes, so it is selected from its bindings.
    let hint = ledger_dir(&root).join("by-spec/STORY-UNRELATED-1.json");
    let hint: serde_json::Value = serde_json::from_slice(&std::fs::read(hint).unwrap()).unwrap();
    assert_eq!(hint["changes"].as_array().unwrap().len(), 40);
    let mut d = checked::discover(&root).unwrap().unwrap();
    let got = d
        .events_for(&["STORY-UNRELATED-1".into()], &mut Budget::default())
        .unwrap();
    assert_eq!(got[0].1.len(), 40);
    assert_eq!(work::FULL_READ_ANSWERS.with(Cell::get), 0);
}

/// The evidence of a lost binding, case by case.
#[test]
fn bug_2531_only_evidence_of_a_lost_binding_asks_for_a_full_read() {
    use checked::binding_lost;
    let none: Option<&[&str]> = None;
    // Never recorded: no bindings directory, no hint.
    assert!(!binding_lost(none, none));
    // As a writer leaves them, and a hint that lost an entry.
    assert!(!binding_lost(Some(&["PR-1"][..]), Some(&["PR-1"][..])));
    assert!(!binding_lost(
        Some(&["PR-1"][..]),
        Some(&["PR-1", "PR-2"][..])
    ));
    assert!(!binding_lost(Some(&[][..]), none));
    // The hint names a change the bindings do not.
    assert!(binding_lost(
        Some(&["PR-1", "PR-2"][..]),
        Some(&["PR-1"][..])
    ));
    assert!(binding_lost(Some(&["PR-1"][..]), Some(&[][..])));
    assert!(binding_lost(Some(&["PR-1"][..]), none));
    // The bindings directory outlived the hint: emptied or not.
    assert!(binding_lost(none, Some(&[][..])));
    assert!(binding_lost(none, Some(&["PR-1"][..])));
}

/// The fix removes a label directory it emptied, so the label reads as
/// never recorded rather than as a deletion on every selection.
#[test]
fn bug_2531_bootstrap_leaves_no_empty_label_directory() {
    let (_tmp, root) = root();
    let _env = uncontained();
    append(&root, ChangeKey::pr(1), "BUG-1", &record("approved", 0));
    std::fs::create_dir(bindings_dir(&root).join("BUG-2")).unwrap();
    std::fs::write(bindings_dir(&root).join("BUG-2/PR-77"), b"").unwrap();
    bootstrap(&root).unwrap();
    assert!(!bindings_dir(&root).join("BUG-2").exists());
    assert!(bindings_dir(&root).join("BUG-1/PR-1").is_file());
    work::FULL_READ_ANSWERS.with(|n| n.set(0));
    let mut d = checked::discover(&root).unwrap().unwrap();
    let got = d
        .events_for(&["BUG-2".into()], &mut Budget::default())
        .unwrap();
    assert!(got[0].1.is_empty());
    assert_eq!(work::FULL_READ_ANSWERS.with(Cell::get), 0);
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

    let findings = crate::doctor_cmd::scan_gate_ledger_bindings(&root, true);
    assert_eq!(findings.len(), 1);
    assert!(findings[0].action.contains(FIX_COMMAND));
    assert!(findings[0].safe_heal);
    bootstrap(&root).unwrap();
    assert!(crate::doctor_cmd::scan_gate_ledger_bindings(&root, true).is_empty());
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
    // A deleted binding does not hide the refusal either. Each state holds
    // evidence of the deletion: the stale hint names a change the bindings
    // do not, and with the hint missing the label's bindings directory
    // stands without one. Selection then reads every change ledger and
    // finds the one that records the spec.
    std::fs::remove_file(bindings_dir(&root).join("BUG-9/PR-2")).unwrap();
    for hint in [Some(r#"{"changes":["PR-1","PR-2"]}"#), None] {
        match hint {
            Some(body) => std::fs::write(&index, body).unwrap(),
            None => std::fs::remove_file(&index).unwrap(),
        }
        work::FULL_READ_ANSWERS.with(|n| n.set(0));
        let mut d = checked::discover(&root).unwrap().unwrap();
        let got = d
            .events_for(&["BUG-9".into()], &mut Budget::default())
            .unwrap();
        assert_eq!(
            newest_verdict(&got[0].1).as_deref(),
            Some("request-changes"),
            "{hint:?}"
        );
        assert_eq!(work::FULL_READ_ANSWERS.with(Cell::get), 1, "{hint:?}");
        let (_, changes) = d.spec_entries("BUG-9", &mut Budget::default()).unwrap();
        assert!(changes.contains(&ChangeKey::pr(2)), "{hint:?}");
        assert_eq!(
            project::newest_verdict(&root, "BUG-9").and_then(|r| r.get("verdict").cloned()),
            Some(serde_json::json!("request-changes")),
            "{hint:?}"
        );
    }
    // The full read is strict: a change ledger it cannot read refuses the
    // selection, naming the file, and the lossy reader withholds the reviews.
    let bad = store::ledger_file(&root, ChangeKey::pr(9999));
    std::fs::write(&bad, b"invalid\n").unwrap();
    let mut d = checked::discover(&root).unwrap().unwrap();
    let err = d
        .events_for(&["BUG-9".into()], &mut Budget::default())
        .unwrap_err();
    assert!(err.0.contains("PR-9999.jsonl"), "{err}");
    assert_eq!(project::newest_verdict(&root, "BUG-9"), None);
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

/// A ledger holding events of a newer format than this binary writes is
/// not appended to: the newer format's write rules are unknown here.
#[test]
fn bug_2531_writer_refuses_a_ledger_of_a_newer_format() {
    let (_tmp, root) = root();
    let _env = uncontained();
    append(&root, ChangeKey::pr(1), "BUG-1", &record("approved", -60));
    let file = store::ledger_file(&root, ChangeKey::pr(1));
    let body = std::fs::read_to_string(&file).unwrap();
    let mut last: serde_json::Value = serde_json::from_str(body.lines().last().unwrap()).unwrap();
    last["v"] = serde_json::json!(event::EVENT_VERSION + 1);
    last["seq"] = serde_json::json!(last["seq"].as_u64().unwrap() + 1);
    std::fs::write(&file, format!("{body}{last}\n")).unwrap();
    let before = len(&file);
    let spec = GateSpec::with_key("BUG-1", None);
    let err = store::append_one(
        &root,
        event::verdict_event(&root, ChangeKey::pr(1), &record("rejected", 0), Some(&spec)),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("newer than the format"), "{err}");
    assert!(err.contains("upgrade aida"), "{err}");
    assert_eq!(len(&file), before, "nothing was appended");
    // Another change's ledger is unaffected.
    append(&root, ChangeKey::pr(2), "BUG-1", &record("approved", 0));
}

/// A discovery shared across a listing is not reused once a spec it listed
/// gains a binding: the new change's refusal is read, as fresh discovery
/// reads it, and an unrelated unsafe ledger still blocks nothing.
// trace:BUG-2543 | ai:claude
#[test]
fn bug_2531_shared_discovery_sees_a_binding_added_after_it_listed() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let own = record("request_changes", -60);
    append(&root, ChangeKey::pr(1), "BUG-2517", &own);
    let mut foreign = record("approved", -30);
    append(&root, ChangeKey::pr(1), "BUG-2517", &foreign);
    std::fs::write(store::ledger_file(&root, ChangeKey::pr(9999)), b"invalid\n").unwrap();
    let verdict =
        |root: &Path| project::newest_verdict(root, "PR-1").and_then(|r| r.get("verdict").cloned());
    let _scope = checked::DiscoveryScope::enter();
    project::OWNERSHIP_WORK.with(|w| w.set((0, 0)));
    assert_eq!(verdict(&root), Some(serde_json::json!("approved")));
    assert_eq!(verdict(&root), Some(serde_json::json!("approved")));
    assert_eq!(
        project::OWNERSHIP_WORK.with(|w| w.get()).0,
        1,
        "the second read reuses the shared discovery"
    );
    // The approval's true owner records its phase-3 witness on another
    // change: a new binding under the listed spec, no change to PR-1.
    foreign.insert("mode".into(), serde_json::json!(project::PHASE3_MODE));
    append(&root, ChangeKey::pr(2), "BUG-2517", &foreign);
    assert_eq!(verdict(&root), Some(serde_json::json!("request_changes")));
    assert_eq!(
        project::OWNERSHIP_WORK.with(|w| w.get()).0,
        2,
        "the new binding forces a rediscovery"
    );
}

/// A first append that carries no spec still certifies the empty ledger:
/// a new store must never need the bootstrap before its first selection.
#[test]
fn bug_2531_first_append_without_a_spec_certifies_the_ledger() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let event = GateEvent::new(
        ChangeKey::pr(7),
        EventBody::Verdict {
            record: record("approved", 0),
        },
    );
    store::append_one(&root, event).unwrap();
    assert_eq!(marker_state(&root).unwrap(), MarkerState::Complete);
    let events =
        checked::read_change_events(&root, ChangeKey::pr(7), &mut Budget::default()).unwrap();
    assert_eq!(newest_verdict(&events).as_deref(), Some("approved"));
    // A ledger that already holds unbound change ledgers is never certified
    // by a writer: only the bootstrap can.
    let (_tmp, legacy) = self::root();
    legacy_ledger(&legacy, 1, "BUG-1", 16);
    append(&legacy, ChangeKey::pr(2), "BUG-2", &record("approved", 0));
    assert_eq!(marker_state(&legacy).unwrap(), MarkerState::Missing);
}

/// A ledger line as a binary older than format 2 appends it: version 1, no
/// binding, no index (its best-effort index update failed).
fn older_writer_appends(root: &Path, n: u64, spec: &str, verdict: &str) {
    use std::io::Write as _;
    let mut e = GateEvent::new(
        ChangeKey::pr(n),
        EventBody::Verdict {
            record: record(verdict, 0),
        },
    );
    e.v = 1;
    e.spec = Some(spec.into());
    e.head = Some(HEAD.into());
    e.seq = 1_000;
    e.at = ts(0);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(store::ledger_file(root, ChangeKey::pr(n)))
        .unwrap();
    writeln!(file, "{}", serde_json::to_string(&e).unwrap()).unwrap();
}

/// A binary older than format 2 cannot write the ledger once a format 2
/// writer has touched it: its ledger-wide lock no longer opens or creates.
/// A ledger whose fence was taken down (so such a binary could write) is
/// not trusted: selection refuses, the next writer withdraws the marker,
/// and the fix finds what the older binary appended.
#[test]
fn bug_2531_older_writers_are_fenced_out_and_an_open_fence_fails_closed() {
    let (_tmp, root) = root();
    let _env = uncontained();
    append(&root, ChangeKey::pr(1), "BUG-1", &record("approved", -60));
    let legacy = store::legacy_lock_file(&root);
    assert!(store::writer_fenced(&root));
    // What every older binary does first, in both of its lock paths.
    assert!(std::fs::File::open(&legacy).is_err());
    assert!(std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&legacy)
        .is_err());
    assert!(!legacy.exists(), "the refused open created nothing");

    // An operator takes the fence down and an older binary records a
    // refusal on a new change: no binding, no index.
    std::fs::remove_file(&legacy).unwrap();
    std::fs::write(&legacy, b"").unwrap();
    older_writer_appends(&root, 2, "BUG-1", "request-changes");
    assert_eq!(marker_state(&root).unwrap(), MarkerState::Complete);
    let err = checked::discover(&root).err().unwrap();
    assert!(err.0.contains("not fenced out"), "{err}");
    assert!(err.0.contains(FIX_COMMAND), "{err}");
    let findings = crate::doctor_cmd::scan_gate_ledger_bindings(&root, false);
    assert_eq!(findings.len(), 1);
    assert!(
        findings[0].summary.contains("not fenced out"),
        "{findings:?}"
    );
    assert!(findings[0].safe_heal);

    // The next format 2 write fences again and withdraws the marker: it
    // cannot know what was appended while the fence was down.
    append(&root, ChangeKey::pr(3), "BUG-3", &record("approved", 0));
    assert!(store::writer_fenced(&root));
    assert_eq!(marker_state(&root).unwrap(), MarkerState::Missing);
    assert!(checked::discover(&root).is_err());
    bootstrap(&root).unwrap();
    assert!(crate::doctor_cmd::scan_gate_ledger_bindings(&root, true).is_empty());
    let mut d = checked::discover(&root).unwrap().unwrap();
    let got = d
        .events_for(&["BUG-1".into()], &mut Budget::default())
        .unwrap();
    assert_eq!(
        newest_verdict(&got[0].1).as_deref(),
        Some("request-changes")
    );
}

/// The fence waits out an older writer that already holds the old lock, so
/// its append is on disk before anything is certified.
#[test]
fn bug_2531_fence_waits_for_an_older_writer_in_flight() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (_tmp, root) = root();
    let _env = uncontained();
    let legacy = store::legacy_lock_file(&root);
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, b"").unwrap();
    let older = std::fs::File::open(&legacy).unwrap();
    fs2::FileExt::lock_shared(&older).unwrap();
    let done = std::sync::Arc::new(AtomicBool::new(false));
    let waiter = {
        let (root, done) = (root.clone(), done.clone());
        std::thread::spawn(move || {
            store::fence_older_writers(&root).unwrap();
            done.store(true, Ordering::SeqCst);
        })
    };
    // The link goes in at once (no new older writer can start) ...
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !store::writer_fenced(&root) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(store::writer_fenced(&root));
    // ... but the writer behind it does not proceed while the older one
    // still holds its lock.
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(!done.load(Ordering::SeqCst));
    fs2::FileExt::unlock(&older).unwrap();
    waiter.join().unwrap();
    assert!(done.load(Ordering::SeqCst));
}

/// The fix command repairs what selection refuses on (or would miss) while
/// the marker is valid: a change ledger carrying a spec no binding names, a
/// binding whose ledger is gone, a bindings tree that is not plain
/// directories of regular files. Doctor reports each.
#[test]
fn bug_2531_fix_repairs_inconsistencies_under_a_valid_marker() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let scan = |deep: bool| crate::doctor_cmd::scan_gate_ledger_bindings(&root, deep);
    let newest = |spec: &str| {
        let mut d = checked::discover(&root).unwrap().unwrap();
        d.events_for(&[spec.to_string()], &mut Budget::default())
            .map(|got| newest_verdict(&got[0].1))
    };
    append(
        &root,
        ChangeKey::pr(1),
        "BUG-1",
        &record("request-changes", 0),
    );
    assert!(scan(true).is_empty());

    // A ledger records BUG-1 with neither binding nor hint, under a valid
    // marker. The label's emptied bindings directory is the evidence of the
    // deletion and sends selection to a full read of the change ledgers, so
    // the refusal is still seen; the named, deep check reports the lost
    // binding and the fix restores it.
    std::fs::remove_file(bindings_dir(&root).join("BUG-1/PR-1")).unwrap();
    std::fs::remove_file(ledger_dir(&root).join("by-spec/BUG-1.json")).unwrap();
    assert_eq!(marker_state(&root).unwrap(), MarkerState::Complete);
    assert_eq!(newest("BUG-1").unwrap().as_deref(), Some("request-changes"));
    assert_eq!(
        project::newest_verdict(&root, "BUG-1").and_then(|r| r.get("verdict").cloned()),
        Some(serde_json::json!("request-changes"))
    );
    assert!(scan(false).is_empty());
    let findings = scan(true);
    assert_eq!(findings.len(), 1);
    assert!(
        findings[0]
            .summary
            .contains("PR-1 records BUG-1 events without a binding"),
        "{findings:?}"
    );
    assert!(findings[0].action.contains(FIX_COMMAND));
    assert!(findings[0].safe_heal);
    assert_eq!(bootstrap(&root).unwrap().bindings, 1);
    assert!(scan(true).is_empty());
    assert_eq!(newest("BUG-1").unwrap().as_deref(), Some("request-changes"));

    // A binding whose ledger is gone, a non-directory spec entry and a
    // linked binding: selection refuses on each; the cheap check sees them.
    std::fs::write(bindings_dir(&root).join("BUG-1/PR-405"), b"").unwrap();
    std::fs::write(bindings_dir(&root).join("BUG-9"), b"").unwrap();
    std::fs::create_dir(bindings_dir(&root).join("BUG-8")).unwrap();
    std::os::unix::fs::symlink("/dev/null", bindings_dir(&root).join("BUG-8/PR-1")).unwrap();
    assert!(newest("BUG-1").unwrap_err().0.contains(FIX_COMMAND));
    assert!(newest("BUG-9").unwrap_err().0.contains(FIX_COMMAND));
    assert!(newest("BUG-8").unwrap_err().0.contains(FIX_COMMAND));
    let findings = scan(false);
    assert_eq!(findings.len(), 1);
    let problems = audit(&root, false).unwrap();
    for what in [
        "BUG-1 is bound to PR-405, whose ledger is missing",
        "bindings entry BUG-9 is not a directory",
        "binding BUG-8/PR-1 is not a regular file named by a change",
    ] {
        assert!(problems.iter().any(|p| p == what), "{what}: {problems:?}");
    }
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert_eq!(bootstrap(&root).unwrap().pruned, 3);
    assert!(scan(true).is_empty());
    assert_eq!(newest("BUG-1").unwrap().as_deref(), Some("request-changes"));
    assert_eq!(newest("BUG-9").unwrap(), None);
    assert_eq!(newest("BUG-8").unwrap(), None);
}

/// A review recorded under a label that is not a label key (a separator in
/// it) is bound like any other. The ownership filter therefore finds its
/// witnesses and keeps the refusal, instead of withholding every such
/// review for want of a key; a foreign copy is still dropped.
#[test]
fn bug_2531_a_refusal_under_an_unkeyable_label_is_bound_not_withheld() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let label = "BUG 7/odd";
    assert!(project::spec_key(label).is_none());
    let key = binding_key(label);
    assert!(key.starts_with('~'), "{key}");
    assert_eq!(key, binding_key(" bug 7/ODD "));

    append(
        &root,
        ChangeKey::pr(1),
        label,
        &record("request-changes", -60),
    );
    assert!(bindings_dir(&root).join(&key).join("PR-1").is_file());
    let read = |n: u64| {
        newest_verdict(
            &checked::read_change_events(&root, ChangeKey::pr(n), &mut Budget::default()).unwrap(),
        )
    };
    assert_eq!(read(1).as_deref(), Some("request-changes"));
    let raw = store::read_file(&root, ChangeKey::pr(1));
    let kept = checked::filter_miskeyed_or_fail_closed(&root, raw);
    assert_eq!(newest_verdict(&kept).as_deref(), Some("request-changes"));

    // A newer approval on PR-1 that PR-2's phase-3 record proves foreign is
    // still dropped: the witness is found through the same binding key.
    let mut foreign = record("approved", 0);
    append(&root, ChangeKey::pr(1), label, &foreign);
    assert_eq!(read(1).as_deref(), Some("approved"));
    foreign.insert("mode".into(), serde_json::json!(project::PHASE3_MODE));
    append(&root, ChangeKey::pr(2), label, &foreign);
    assert!(bindings_dir(&root).join(&key).join("PR-2").is_file());
    assert_eq!(read(1).as_deref(), Some("request-changes"));

    // A ledger written before bindings existed is bound the same way.
    let (_tmp, old) = self::root();
    legacy_ledger(&old, 5, label, 16);
    bootstrap(&old).unwrap();
    assert!(bindings_dir(&old).join(&key).join("PR-5").is_file());
    assert!(crate::doctor_cmd::scan_gate_ledger_bindings(&old, true).is_empty());
}
