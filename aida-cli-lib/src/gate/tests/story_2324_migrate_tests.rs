//! STORY-2324 PR-B: `aida gate migrate` (test plan section 8, "Migration
//! tests") and the writers' `spec_key`.
// trace:STORY-2324 | ai:claude

use super::*;
use crate::gate::canon::{GateSpec, Snapshot};
use crate::gate::ledger::{store, ChangeKey, EventBody, GateEvent};
use crate::gate::review_verdict::JsonObj;

const NODE: &str = "b6b1278890774ef993d2ebc327862470";
const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn req(native: &str, friendly: Option<&str>) -> aida_core::Requirement {
    let mut r = aida_core::Requirement::new(format!("fixture {native}"), "fixture".into());
    r.spec_id = Some(native.to_string());
    r.agreed_id = friendly.map(str::to_string);
    r
}

struct Fixture {
    dir: tempfile::TempDir,
    a: aida_core::Requirement,
    b: aida_core::Requirement,
    snap: Snapshot,
}

impl Fixture {
    /// `a` answers to its native ID (any case) and `BUG-2403`; `BUG-2999`
    /// is claimed by both `b` (friendly) and `c` (friendly): ambiguous.
    fn new() -> Self {
        let a = req(&format!("BUG-{NODE}-020"), Some("BUG-2403"));
        let b = req(&format!("STORY-{NODE}-007"), Some("BUG-2999"));
        let c = req(&format!("TASK-{NODE}-008"), Some("BUG-2999"));
        let snap =
            Snapshot::from_parts("store-head", &[a.clone(), b.clone(), c], None, None).unwrap();
        Self {
            dir: tempfile::tempdir().unwrap(),
            a,
            b,
            snap,
        }
    }

    fn aida(&self) -> PathBuf {
        self.dir.path().join(".aida")
    }

    fn ledger(&self) -> PathBuf {
        self.aida().join("gate-ledger")
    }

    fn plan(&self) -> Plan {
        let snap = &self.snap;
        let resolve = |l: &str| snap.resolve_one(l);
        plan(&self.ledger(), &self.aida(), &resolve).unwrap()
    }

    fn write(&self, rel: &str, body: &str) {
        let p = self.aida().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.aida().join(rel)).unwrap()
    }
}

fn record(word: &str, n: u64) -> JsonObj {
    let mut r = JsonObj::new();
    r.insert("verdict".into(), word.into());
    r.insert("reviewed_sha".into(), HEAD.into());
    r.insert(
        "recorded_at".into(),
        format!("2026-11-02T10:{n:02}:00+00:00").into(),
    );
    r.insert("recorded_by".into(), "reviewer seat".into());
    r
}

fn line(change: &str, seq: u64, spec: Option<&str>, word: &str) -> String {
    let mut e = GateEvent::new(
        ChangeKey::parse(change).unwrap_or(ChangeKey::pr(0)),
        EventBody::Verdict {
            record: record(word, seq),
        },
    );
    e.change = change.to_string();
    e.seq = seq;
    e.at = format!("2026-11-02T11:{seq:02}:00+00:00");
    e.head = Some(HEAD.into());
    e.spec = spec.map(str::to_string);
    serde_json::to_string(&e).unwrap()
}

/// A v1 tree: a change ledger mixing every line kind, an unbound entry and
/// its index, a lock file, and one file of each legacy store.
fn seed(f: &Fixture) -> String {
    let native_up = format!("BUG-{}-020", NODE.to_ascii_uppercase());
    let pr = [
        line("PR-7", 1, Some("BUG-2403"), "changes_requested"),
        line("PR-7", 2, None, "approved"),
        line("PR-7", 3, Some(&native_up), "approved"),
        line("PR-7", 4, Some("BUG-9999"), "changes_requested"),
        line("PR-7", 5, Some("BUG-2999"), "approved"),
    ]
    .join("\n")
        + "\n{\"v\":1,\"seq\":6,\"torn";
    f.write("gate-ledger/PR-7.jsonl", &pr);
    f.write("gate-ledger/PR-7.lock", "");
    f.write(
        "gate-ledger/by-spec/BUG-2403.jsonl",
        &(line("BUG-2403", 1, Some("BUG-2403"), "approved") + "\n"),
    );
    f.write(
        "gate-ledger/by-spec/BUG-2403.json",
        "{\"changes\":[\"PR-7\"]}",
    );
    f.write("review-verdicts/BUG-2403.json", "{}");
    f.write("review-verdicts/BUG-9999.json", "{}");
    f.write("review-verdicts/PR-7.json", "{}");
    f.write("merge-holds/PR-7", "legacy marker");
    f.write("pr-holds/BUG-2403.json", "{}");
    pr
}

fn lines_of(body: &str) -> Vec<&str> {
    body.lines().collect()
}

// trace:STORY-2324 | ai:claude
#[test]
fn a_stamped_line_is_the_v1_line_plus_its_key_and_reads_as_the_same_event() {
    let f = Fixture::new();
    let v1 = line("PR-7", 3, Some("bug-2403"), "approved");
    let snap = &f.snap;
    let (v2, fate) = stamp_line(&v1, &|l| snap.resolve_one(l));
    let key = SpecKey(f.a.id);
    assert_eq!(fate, LineFate::Stamped(key));
    assert_eq!(unstamp(&v2), Some((key, v1.clone())));
    let old: GateEvent = serde_json::from_str(&v1).unwrap();
    let new: GateEvent = serde_json::from_str(&v2).unwrap();
    assert_eq!(new.spec_key, Some(key));
    assert_eq!(
        GateEvent {
            spec_key: None,
            ..new
        },
        old
    );
}

// trace:STORY-2324 | ai:claude
#[test]
fn the_migration_preserves_every_body_and_the_seq_order_and_keeps_unresolved_labels() {
    let f = Fixture::new();
    let v1 = seed(&f);
    let legacy_before = f.read("merge-holds/PR-7");
    let p = f.plan();
    assert_eq!(
        p.stamped, 3,
        "BUG-2403, the upper-cased native ID, the unbound entry"
    );
    assert_eq!(p.unparsed, 1, "the torn final write");
    assert!(p.unresolved.contains_key("BUG-9999"));
    assert!(p.ambiguous.contains_key("BUG-2999"));
    let rv = p
        .legacy
        .iter()
        .find(|d| d.dir == "review-verdicts")
        .unwrap();
    assert_eq!((rv.files, rv.change_keyed, rv.resolved), (3, 1, 1));
    assert_eq!(rv.unresolved, vec!["BUG-9999".to_string()]);

    let applied = execute(&f.aida(), &p, "20261103T000000Z", "store-head").unwrap();
    assert!(applied.swapped);
    let v2 = f.read("gate-ledger/PR-7.jsonl");
    // A1: same line count, same order; each body hash-verifies against v1.
    verify_file(v1.as_bytes(), v2.as_bytes()).unwrap();
    let (old, new) = (lines_of(&v1), lines_of(&v2));
    assert_eq!(old.len(), new.len());
    let seqs = |ls: &[&str]| -> Vec<u64> {
        ls.iter()
            .filter_map(|l| serde_json::from_str::<GateEvent>(l).ok())
            .map(|e| e.seq)
            .collect()
    };
    assert_eq!(seqs(&old), seqs(&new));
    assert_eq!(seqs(&new), vec![1, 2, 3, 4, 5]);
    // Unresolved and ambiguous labels, the no-spec line and the torn line are
    // kept exactly; the resolved ones carry the key.
    for i in [1, 3, 4, 5] {
        assert_eq!(old[i], new[i], "line {i} copied as is");
    }
    let key = SpecKey(f.a.id);
    for i in [0, 2] {
        let e: GateEvent = serde_json::from_str(new[i]).unwrap();
        assert_eq!(e.spec_key, Some(key), "line {i}");
    }
    // The archive is v1, byte for byte; the index, lock and legacy files are
    // untouched; nothing became an approval that was not one.
    assert_eq!(
        std::fs::read_to_string(f.aida().join("gate-ledger.v1-20261103T000000Z/PR-7.jsonl"))
            .unwrap(),
        v1
    );
    assert_eq!(
        f.read("gate-ledger/by-spec/BUG-2403.json"),
        "{\"changes\":[\"PR-7\"]}"
    );
    assert!(f.ledger().join("PR-7.lock").exists());
    assert_eq!(f.read("merge-holds/PR-7"), legacy_before);
    let approvals = |b: &str| b.matches("\"approved\"").count();
    assert_eq!(approvals(&v1), approvals(&v2));
    // Today's readers see the same records through the same labels.
    let events = store::read_file(f.dir.path(), ChangeKey::pr(7));
    assert_eq!(events.len(), 5);
    assert_eq!(
        store::read_unbound(f.dir.path(), "BUG-2403")[0].spec_key,
        Some(key)
    );
}

// trace:STORY-2324 | ai:claude
#[test]
fn the_migration_is_idempotent() {
    let f = Fixture::new();
    seed(&f);
    execute(&f.aida(), &f.plan(), "20261103T000000Z", "store-head").unwrap();
    let after_first = f.read("gate-ledger/PR-7.jsonl");
    let again = f.plan();
    assert_eq!(again.stamped, 0);
    assert_eq!(again.already_keyed, 3);
    for file in &again.files {
        assert_eq!(file.v1, file.v2, "{} unchanged", file.rel.display());
    }
    let applied = execute(&f.aida(), &again, "20261103T000001Z", "store-head").unwrap();
    assert!(!applied.swapped);
    assert_eq!(f.read("gate-ledger/PR-7.jsonl"), after_first);
    assert!(!f.aida().join("gate-ledger.v1-20261103T000001Z").exists());
}

// trace:STORY-2324 | ai:claude
#[test]
fn verification_rejects_a_reordered_dropped_or_edited_line() {
    let a = "{\"v\":1,\"seq\":1}\n{\"v\":1,\"seq\":2}\n";
    assert!(verify_file(a.as_bytes(), a.as_bytes()).is_ok());
    let reordered = "{\"v\":1,\"seq\":2}\n{\"v\":1,\"seq\":1}\n";
    assert!(verify_file(a.as_bytes(), reordered.as_bytes()).is_err());
    assert!(verify_file(a.as_bytes(), b"{\"v\":1,\"seq\":1}\n").is_err());
    let key = uuid::Uuid::nil();
    let stamped_edit =
        format!("{{\"spec_key\":\"{key}\",\"v\":2,\"seq\":1}}\n{{\"v\":1,\"seq\":2}}\n");
    assert!(verify_file(a.as_bytes(), stamped_edit.as_bytes()).is_err());
    let stamped = format!("{{\"spec_key\":\"{key}\",\"v\":1,\"seq\":1}}\n{{\"v\":1,\"seq\":2}}\n");
    assert!(verify_file(a.as_bytes(), stamped.as_bytes()).is_ok());
}

// trace:STORY-2324 | ai:claude
#[test]
fn execute_refuses_off_a_terminal_from_a_seat_outside_the_host_or_under_a_drain() {
    let host = Some(aida_core::process_probe::INITIAL_PID_NAMESPACE_INODE);
    assert!(execute_refusal(false, host, false, false).is_some());
    assert!(execute_refusal(true, host, true, false)
        .unwrap()
        .contains("contained"));
    assert!(execute_refusal(true, Some(1), false, false).is_some());
    assert!(execute_refusal(true, None, false, false).is_some());
    assert!(execute_refusal(true, host, false, true)
        .unwrap()
        .contains("drain"));
    assert_eq!(execute_refusal(true, host, false, false), None);
}

// trace:STORY-2324 | ai:claude
#[test]
fn an_unresolved_label_on_an_open_pull_request_refuses_execute() {
    let f = Fixture::new();
    seed(&f);
    let p = f.plan();
    let on = p.changes_with_unresolved();
    assert_eq!(
        on.get(&7).map(|s| s.iter().cloned().collect::<Vec<_>>()),
        Some(vec!["BUG-2999".to_string(), "BUG-9999".to_string()])
    );
    assert!(open_pr_refusal(&p, &Ok([7].into()))
        .unwrap()
        .contains("PR-7"));
    assert_eq!(open_pr_refusal(&p, &Ok([8].into())), None);
    assert!(open_pr_refusal(&p, &Err("no forge".into())).is_some());
    let clean = Fixture::new();
    clean.write(
        "gate-ledger/PR-1.jsonl",
        &(line("PR-1", 1, Some("BUG-2403"), "approved") + "\n"),
    );
    assert_eq!(
        open_pr_refusal(&clean.plan(), &Err("no forge".into())),
        None
    );
}

// trace:STORY-2324 | ai:claude
#[test]
fn rollback_restores_v1_and_refuses_after_a_later_write() {
    let f = Fixture::new();
    let v1 = seed(&f);
    execute(&f.aida(), &f.plan(), "20261103T000000Z", "store-head").unwrap();
    assert_ne!(f.read("gate-ledger/PR-7.jsonl"), v1);
    let done = rollback(&f.aida()).unwrap();
    assert_eq!(done.restored, "gate-ledger.v1-20261103T000000Z");
    assert_eq!(f.read("gate-ledger/PR-7.jsonl"), v1);
    assert!(rollback(&f.aida()).is_err(), "one run, one rollback");

    let g = Fixture::new();
    seed(&g);
    execute(&g.aida(), &g.plan(), "20261103T000000Z", "store-head").unwrap();
    let migrated = g.read("gate-ledger/PR-7.jsonl");
    g.write(
        "gate-ledger/PR-8.jsonl",
        &(line("PR-8", 1, None, "approved") + "\n"),
    );
    let err = rollback(&g.aida()).unwrap_err().to_string();
    assert!(err.contains("PR-8.jsonl"), "{err}");
    assert_eq!(
        g.read("gate-ledger/PR-7.jsonl"),
        migrated,
        "nothing changed"
    );
}

// trace:STORY-2324 | ai:claude
#[test]
fn a_write_between_plan_and_swap_undoes_the_swap() {
    let f = Fixture::new();
    seed(&f);
    let p = f.plan();
    let raced = f.read("gate-ledger/PR-7.jsonl") + "\n" + &line("PR-7", 7, None, "approved");
    f.write("gate-ledger/PR-7.jsonl", &raced);
    let err = execute(&f.aida(), &p, "20261103T000000Z", "store-head")
        .unwrap_err()
        .to_string();
    assert!(err.contains("changed while"), "{err}");
    assert_eq!(
        f.read("gate-ledger/PR-7.jsonl"),
        raced,
        "the racing event kept"
    );
    assert!(!f.aida().join("gate-ledger.v1-20261103T000000Z").exists());
    assert!(!f.aida().join("gate-ledger.v2").exists());
}

// ── Writers (E2/E3) ─────────────────────────────────────────────────────────

// trace:STORY-2324 | ai:claude
#[test]
fn verdict_writers_stamp_the_resolved_key_and_an_unkeyed_event_serializes_as_before() {
    let f = Fixture::new();
    let key = SpecKey(f.b.id);
    let spec = GateSpec::with_key("story-x", Some(key));
    let e = crate::gate::ledger::event::verdict_event(
        f.dir.path(),
        ChangeKey::pr(3),
        &record("approved", 1),
        Some(&spec),
    );
    assert_eq!(
        (e.spec.as_deref(), e.spec_key),
        (Some("STORY-X"), Some(key))
    );
    let unkeyed = crate::gate::ledger::event::verdict_event(
        f.dir.path(),
        ChangeKey::pr(3),
        &record("approved", 1),
        Some(&GateSpec::with_key("BUG-1", None)),
    );
    assert!(!serde_json::to_string(&unkeyed)
        .unwrap()
        .contains("spec_key"));

    // An unbound record carries the key, and binding it keeps it.
    store::append_unbound(f.dir.path(), &spec, &record("approved", 2)).unwrap();
    assert_eq!(
        store::read_unbound(f.dir.path(), "STORY-X")[0].spec_key,
        Some(key)
    );
    store::bind_unbound(f.dir.path(), &spec, ChangeKey::pr(4)).unwrap();
    let bound: Vec<GateEvent> = store::read_file(f.dir.path(), ChangeKey::pr(4))
        .into_iter()
        .filter(|e| e.verdict_record().is_some())
        .collect();
    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0].spec_key, Some(key));
}

// trace:STORY-2324 | ai:claude
#[test]
fn a_label_outside_any_store_is_written_without_a_key() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        GateSpec::resolve(dir.path(), " bug-12 "),
        GateSpec {
            label: "BUG-12".into(),
            key: None
        }
    );
}

// trace:STORY-2324 | ai:claude
#[test]
fn hold_records_and_signals_read_without_a_key() {
    let hold = crate::gate::merge_hold_core::typed_hold(
        7,
        crate::gate::merge_hold_core::HoldReasonKind::Rework,
        "x",
        None,
    )
    .with_spec("BUG-2403");
    let body = serde_json::to_string(&hold).unwrap();
    assert!(!body.contains("spec_key"));
    let back: crate::gate::merge_hold_core::MergeHoldRecord = serde_json::from_str(&body).unwrap();
    assert_eq!(back.spec_key, None);
    let mut keyed = hold.clone();
    keyed.spec_key = Some(SpecKey(uuid::Uuid::nil()));
    let back: crate::gate::merge_hold_core::MergeHoldRecord =
        serde_json::from_str(&serde_json::to_string(&keyed).unwrap()).unwrap();
    assert_eq!(back, keyed);
    let signal: crate::punt::HoldSignal =
        serde_json::from_str("{\"spec\":\"BUG-2403\",\"branch\":\"b\"}").unwrap();
    assert_eq!(signal.spec_key, None);
}
