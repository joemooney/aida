//! BUG-2629: rework admission reads only the spec's own gate ledgers, and a
//! failed ownership discovery preserves a known wait instead of admitting.
// trace:BUG-2629 | ai:claude
use crate::gate::ledger::checked::discovery_work_tests as work;
use crate::gate::ledger::{self, event, project, store, ChangeKey};
use crate::gate::review_verdict::{self as rv, JsonObj, RecorderAttestation, ATTESTATION_KEY};
use crate::queue_cmd::refuse_waiting_review;
use crate::queue_cmd::work::{change_verdict_bound_to, resolve_rework_review, ReworkReviewError};
use aida_core::DatabaseBackend;
use std::cell::Cell;
use std::path::{Path, PathBuf};

const HEAD: &str = "1111111111111111111111111111111111111111";
const SPEC: &str = "BUG-2629";
const SUMMARY: &str = "Host: operator must run the host probe";

fn fixture() -> (tempfile::TempDir, PathBuf, crate::test_env::EnvVarsGuard) {
    let env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    (tmp, root, env)
}

/// A review record an ownership witness could disqualify (it carries every
/// field of the review fingerprint), recorded `secs` from now.
fn record(verdict: &str, secs: i64) -> JsonObj {
    let at = (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339();
    let actor = RecorderAttestation {
        sha: HEAD.to_string(),
        seat: Some("reviewer".to_string()),
        grant_id: Some("g-1".to_string()),
        ..Default::default()
    };
    let mut obj: JsonObj = serde_json::from_value(serde_json::json!({
        "verdict": verdict,
        "reviewed_sha": HEAD,
        "recorded_by": "reviewer",
        "recorded_at": at,
        "summary": SUMMARY,
    }))
    .unwrap();
    if verdict == "request-changes" {
        obj.insert("findings".into(), serde_json::json!(["repair the defect"]));
    }
    obj.insert(
        ATTESTATION_KEY.to_string(),
        serde_json::to_value(actor).unwrap(),
    );
    obj
}

/// Record `rec` for `spec` on change `pr`, as a ledger writer does (binding,
/// event, by-spec hint, completeness marker).
fn append(root: &Path, pr: u64, spec: &str, rec: &JsonObj) {
    let spec = ledger::GateSpec::with_key(spec, None);
    let change = ChangeKey::pr(pr);
    store::append_one(root, event::verdict_event(root, change, rec, Some(&spec))).unwrap();
}

/// The by-spec hint still names `pr`, its binding is gone, and an unrelated
/// ledger is malformed: the full read the lost binding calls for refuses.
fn lose_binding_beside_corrupt_ledger(root: &Path, spec: &str, pr: u64) {
    let binding = ledger::bindings::bindings_dir(root)
        .join(spec)
        .join(format!("PR-{pr}"));
    std::fs::remove_file(binding).unwrap();
    std::fs::write(
        ledger::ledger_dir(root).join("PR-999999.jsonl"),
        b"invalid\n",
    )
    .unwrap();
}

/// A second change bound to `spec` whose ledger is malformed.
fn corrupt_bound_sibling(root: &Path, spec: &str) {
    std::fs::write(
        ledger::bindings::bindings_dir(root)
            .join(spec)
            .join("PR-9999"),
        b"",
    )
    .unwrap();
    project::index_spec(root, spec, ChangeKey::pr(9999)).unwrap();
    std::fs::write(ledger::ledger_dir(root).join("PR-9999.jsonl"), b"invalid\n").unwrap();
}

fn review(root: &Path) -> Result<(), ReworkReviewError> {
    resolve_rework_review(root, SPEC, SPEC, None).map(|_| ())
}

fn refused_waiting(root: &Path) -> bool {
    let early = refuse_waiting_review(root, SPEC, SPEC, None);
    let resolved = review(root);
    match (&early, &resolved) {
        (Err(why), Err(ReworkReviewError::Waiting(_))) => {
            assert!(why.contains("WAITING") && why.contains("Host:"), "{why}");
            true
        }
        (Ok(()), Ok(()) | Err(ReworkReviewError::Unavailable(_))) => false,
        _ => panic!("admission and resolution disagree: {early:?} / {resolved:?}"),
    }
}

fn unavailable(root: &Path) -> String {
    match review(root) {
        Err(ReworkReviewError::Unavailable(why)) => why,
        other => panic!("expected unavailable evidence, got {other:?}"),
    }
}

/// The round-3 P1, ledger shape: the wait lives only on the bound change's
/// ledger, and the failed discovery withholds its projection under both the
/// change key and the spec key. Admission must still refuse.
#[test]
fn bug_2629_ledger_wait_survives_failed_ownership_discovery() {
    fn lost(root: &Path) {
        lose_binding_beside_corrupt_ledger(root, SPEC, 3698);
    }
    fn sibling(root: &Path) {
        corrupt_bound_sibling(root, SPEC);
    }
    let breakages: [(&str, fn(&Path)); 2] = [
        ("lost binding beside a malformed ledger", lost),
        ("malformed bound sibling", sibling),
    ];
    for (name, breakage) in breakages {
        let (_tmp, root, _env) = fixture();
        append(&root, 3698, SPEC, &record("waiting", -30));
        assert!(refused_waiting(&root), "{name}: healthy ledger");
        breakage(&root);
        assert!(
            rv::current_verdict(&root, "PR-3698").is_none(),
            "{name}: the change-keyed projection is withheld"
        );
        assert!(
            rv::current_verdict(&root, SPEC).is_none(),
            "{name}: the spec-keyed projection is withheld"
        );
        assert!(refused_waiting(&root), "{name}");
    }
}

/// The round-3 P1, legacy shape: the by-spec index names the change, the
/// wait is its legacy verdict file, no open change is known, and an
/// unrelated ledger is malformed.
#[test]
fn bug_2629_indexed_legacy_wait_survives_failed_ownership_discovery() {
    let (_tmp, root, _env) = fixture();
    let dir = root.join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("PR-3698.json"),
        serde_json::json!({"verdict": "waiting", "reviewed_sha": HEAD, "summary": SUMMARY})
            .to_string(),
    )
    .unwrap();
    project::index_spec(&root, SPEC, ChangeKey::pr(3698)).unwrap();
    std::fs::write(
        ledger::ledger_dir(&root).join("PR-999999.jsonl"),
        b"invalid\n",
    )
    .unwrap();
    assert!(refused_waiting(&root));
}

/// A failed discovery with no wait behind it is unavailable evidence, named
/// as such: early admission proceeds (its callers keep their own policy for
/// unavailable evidence) and the resolver never answers "no review".
#[test]
fn bug_2629_failed_discovery_without_a_wait_is_unavailable_not_empty() {
    let (_tmp, root, _env) = fixture();
    append(&root, 3698, SPEC, &record("request-changes", -30));
    assert!(matches!(
        resolve_rework_review(&root, SPEC, SPEC, None),
        Ok(crate::rework_direction::ReviewResult::Available(_))
    ));
    lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
    assert!(!refused_waiting(&root));
    let why = unavailable(&root);
    assert!(why.contains("ownership discovery failed"), "{why}");
    assert!(
        why.contains("PR-999999"),
        "names the unreadable file: {why}"
    );
}

/// BUG-2646 (1): a later round whose ownership cannot be verified is no
/// clearance of a wait; a round no witness could disqualify still is.
#[test]
fn bug_2629_unverified_later_round_does_not_clear_a_withheld_wait() {
    for (phase3, clears) in [(false, false), (true, true)] {
        let (_tmp, root, _env) = fixture();
        append(&root, 3698, SPEC, &record("waiting", -30));
        let mut approval = record("approved", -10);
        if phase3 {
            approval.insert("mode".into(), project::PHASE3_MODE.into());
        }
        append(&root, 3698, SPEC, &approval);
        assert!(
            !refused_waiting(&root),
            "a healthy ledger reads the approval"
        );
        lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
        assert_eq!(refused_waiting(&root), !clears, "phase3={phase3}");
        if clears {
            unavailable(&root);
        }
    }
}

/// BUG-2646 (2): a wait its terminal annotation closed is not revived by
/// reading the raw ledger. A closure names one key, as in the projection:
/// the wait stays open until both the change's and the spec's are closed.
#[test]
fn bug_2629_closed_terminal_wait_stays_closed_when_discovery_fails() {
    let close = |root: &Path, key: &str| {
        let current = rv::current_verdict(root, key).unwrap();
        assert!(rv::close_terminal_wait(root, key, ChangeKey::pr(3698), &current).unwrap());
    };
    for keys in [&["PR-3698", SPEC][..], &["PR-3698"][..], &[SPEC][..]] {
        let (_tmp, root, _env) = fixture();
        append(&root, 3698, SPEC, &record("waiting", -30));
        assert!(refused_waiting(&root));
        for key in keys {
            close(&root, key);
        }
        let open = keys.len() < 2;
        assert_eq!(
            refused_waiting(&root),
            open,
            "healthy ledger, closed {keys:?}"
        );
        lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
        assert_eq!(
            refused_waiting(&root),
            open,
            "failed discovery, closed {keys:?}"
        );
        if !open {
            unavailable(&root);
        }
    }
}

/// The batch binding answers exactly what the per-key check answered.
#[test]
fn bug_2629_batch_binding_matches_the_per_key_check() {
    let (_tmp, root, _env) = fixture();
    let dir = root.join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    // Indexed for a label of its own, legacy file only.
    std::fs::write(
        dir.join("PR-10.json"),
        serde_json::json!({"verdict": "approved", "reviewed_sha": HEAD}).to_string(),
    )
    .unwrap();
    project::index_spec(&root, "BUG-LEGACY", ChangeKey::pr(10)).unwrap();
    // Recorded for the spec on the ledger.
    append(&root, 11, SPEC, &record("request-changes", -50));
    // Another spec's change.
    append(&root, 12, "BUG-9000", &record("request-changes", -40));
    // Recorded for the spec, then for another spec.
    append(&root, 13, SPEC, &record("request-changes", -30));
    append(&root, 13, "BUG-9000", &record("approved", -20));
    // A legacy file nothing binds.
    std::fs::write(
        dir.join("PR-14.json"),
        serde_json::json!({"verdict": "approved", "reviewed_sha": HEAD}).to_string(),
    )
    .unwrap();
    for labels in [
        vec![SPEC.to_string()],
        vec!["BUG-9000".to_string()],
        vec!["BUG-LEGACY".to_string()],
        vec![SPEC.to_string(), "BUG-NATIVE-1".to_string()],
        vec!["BUG-NEVER-RECORDED".to_string()],
    ] {
        let bindings = super::discover(&root, &labels, None, false);
        assert_eq!(bindings.failure(), None, "{labels:?}");
        let keys = rv::verdict_keys(&root);
        assert!(keys.iter().any(|k| k == "PR-14"), "{keys:?}");
        for key in keys {
            assert_eq!(
                bindings.binds(&key),
                change_verdict_bound_to(&root, &key, &labels),
                "{key} for {labels:?}"
            );
        }
    }
    assert!(super::discover(&root, &[SPEC.to_string()], None, false).binds("PR-11"));
    assert!(super::discover(&root, &["BUG-LEGACY".to_string()], None, false).binds("PR-10"));
    // Without its hint a label is bound by what its ledgers record: the
    // changes whose newest verdict names it, found from one full read.
    std::fs::remove_file(ledger::ledger_dir(&root).join("by-spec/BUG-9000.json")).unwrap();
    let labels = vec!["BUG-9000".to_string()];
    let bindings = super::discover(&root, &labels, None, false);
    assert_eq!(bindings.failure(), None);
    assert!(bindings.binds("PR-12") && bindings.binds("PR-13") && !bindings.binds("PR-11"));
    for key in rv::verdict_keys(&root) {
        assert_eq!(
            bindings.binds(&key),
            change_verdict_bound_to(&root, &key, &labels),
            "{key} without a hint"
        );
    }
    // A selected change replaces the spec-bound ones.
    let selected = super::discover(&root, &[SPEC.to_string()], Some("PR-12"), true);
    assert!(!selected.binds("PR-11") && !selected.binds("PR-12"));
}

fn ledger_reads() -> u64 {
    work::LOSSY_READ_BYTES.with(Cell::get) + work::READ_BYTES.with(Cell::get)
}

fn reset_ledger_reads() {
    work::LOSSY_READ_BYTES.with(|n| n.set(0));
    work::READ_BYTES.with(|n| n.set(0));
    work::DIRECTORY_ENTRIES.with(|n| n.set(0));
}

/// How many unrelated change ledgers [`bench_corpus`] writes.
const CHANGES: u64 = 1100;

/// Add [`CHANGES`] bound and indexed change ledgers of other specs
/// (`PR-10000..`, `BENCH-n`), each one review of about 2 KiB. Returns the
/// bytes every change ledger under `root` then holds together.
fn bench_corpus(root: &Path) -> u64 {
    let mut seed = record("request-changes", -60);
    seed.insert("findings".into(), serde_json::json!(["x".repeat(2048)]));
    append(root, 10_000, "BENCH-0", &seed);
    let line = std::fs::read_to_string(store::ledger_file(root, ChangeKey::pr(10_000))).unwrap();
    let seed: serde_json::Value = serde_json::from_str(line.lines().last().unwrap()).unwrap();
    assert_eq!(seed["kind"], "verdict");
    let dir = ledger::ledger_dir(root);
    let bindings = ledger::bindings::bindings_dir(root);
    for n in 1..CHANGES {
        let (key, spec) = (format!("PR-{}", 10_000 + n), format!("BENCH-{n}"));
        let mut event = seed.clone();
        event["change"] = key.clone().into();
        event["spec"] = spec.clone().into();
        std::fs::write(dir.join(format!("{key}.jsonl")), format!("{event}\n")).unwrap();
        std::fs::create_dir_all(bindings.join(&spec)).unwrap();
        std::fs::write(bindings.join(&spec).join(&key), b"").unwrap();
        std::fs::write(
            dir.join("by-spec").join(format!("{spec}.json")),
            serde_json::json!({ "changes": [key] }).to_string(),
        )
        .unwrap();
    }
    std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .map(|e| e.metadata().unwrap().len())
        .sum()
}

/// Acceptance 3 (and the reproduction of the reported spin): on a ledger of
/// more than 1000 change ledgers, rework admission reads the spec's own
/// ledgers, not the corpus. The per-key check it replaces read the whole
/// corpus once per command, and once per KEY whose ownership discovery had
/// to fall back to a full read that refused.
// trace:BUG-b6b1278890774ef993d2ebc327862470-393.ac915e51 | ai:claude
#[test]
fn bug_2629_rework_scan_is_bounded_on_a_ledger_of_over_1000_changes() {
    const LOST: u64 = 48;
    let (_tmp, root, _env) = fixture();
    append(&root, 77, SPEC, &record("waiting", -30));
    let corpus = bench_corpus(&root);
    let bindings = ledger::bindings::bindings_dir(&root);
    let dir = ledger::ledger_dir(&root);
    let labels = vec![SPEC.to_string()];
    let per_key = |root: &Path| {
        reset_ledger_reads();
        let bound: Vec<String> = rv::verdict_keys(root)
            .into_iter()
            .filter(|key| change_verdict_bound_to(root, key, &labels))
            .collect();
        assert_eq!(bound, ["PR-77"]);
        ledger_reads()
    };
    let batch = |root: &Path| {
        reset_ledger_reads();
        let started = std::time::Instant::now();
        assert!(refused_waiting(root));
        (ledger_reads(), started.elapsed())
    };

    // A healthy ledger.
    let old = per_key(&root);
    let (new, took) = batch(&root);
    eprintln!("bug_2629 healthy: corpus={corpus} per-key={old} batch={new} in {took:?}");
    assert!(old >= corpus, "the per-key check read the corpus: {old}");
    assert!(
        new * 20 < corpus,
        "batch read {new} of a {corpus} byte corpus"
    );

    // Other specs lost a binding and one ledger is malformed: each of their
    // keys repeated the refused full read. The spec in hand is unaffected.
    for n in 1..=LOST {
        std::fs::remove_file(
            bindings
                .join(format!("BENCH-{n}"))
                .join(format!("PR-{}", 10_000 + n)),
        )
        .unwrap();
    }
    std::fs::write(dir.join("PR-99999999.jsonl"), b"invalid\n").unwrap();
    let old = per_key(&root);
    let (new, took) = batch(&root);
    eprintln!("bug_2629 refused full reads: per-key={old} batch={new} in {took:?}");
    assert!(
        old >= (LOST - 8) * corpus,
        "the per-key check repeated the full read per key: {old}"
    );
    assert!(
        new * 20 < corpus,
        "batch read {new} of a {corpus} byte corpus"
    );

    // The spec in hand lost its binding too: its own full read refuses and
    // one label pass stands in for it, a bounded number of times, and the
    // wait still refuses.
    std::fs::remove_file(bindings.join(SPEC).join("PR-77")).unwrap();
    let (new, took) = batch(&root);
    eprintln!("bug_2629 own discovery refused: batch={new} in {took:?}");
    // One refused full read and one label pass per resolution; `batch`
    // resolves twice.
    assert!(
        new <= 5 * corpus,
        "a refused discovery read {new} of a {corpus} byte corpus"
    );
}

/// Finding 1 of the round-4 review: many fingerprinted reviews of the spec
/// in hand, one lost binding, a malformed ledger and no open wait, so the
/// recovery visits every one of its changes. The refused full read happens
/// once for the resolution, not once per change (each change's projection
/// read, what the recovery used to ask per key, repeats it), followed by the
/// one label pass that finds the change whose binding was lost.
// trace:BUG-b6b1278890774ef993d2ebc327862470-393.ac915e51 | ai:claude
#[test]
fn bug_2629_failed_discovery_reads_the_corpus_once_for_many_target_changes() {
    const TARGETS: u64 = 40;
    let (_tmp, root, _env) = fixture();
    for n in 0..TARGETS {
        append(
            &root,
            500 + n,
            SPEC,
            &record("request-changes", -600 + n as i64),
        );
    }
    let corpus = bench_corpus(&root);
    lose_binding_beside_corrupt_ledger(&root, SPEC, 500);

    reset_ledger_reads();
    for n in 0..TARGETS {
        assert!(rv::current_verdict(&root, &format!("PR-{}", 500 + n)).is_none());
    }
    let per_key = ledger_reads();
    assert!(
        per_key >= (TARGETS / 2) * corpus,
        "each withheld projection read repeats the refused full read: {per_key}"
    );

    reset_ledger_reads();
    let started = std::time::Instant::now();
    let why = unavailable(&root);
    let (new, took) = (ledger_reads(), started.elapsed());
    eprintln!("bug_2629 many targets: corpus={corpus} per-key={per_key} batch={new} in {took:?}");
    assert!(why.contains("ownership discovery failed"), "{why}");
    // The refused full read, then the one label pass that stands in for it.
    assert!(
        new < 3 * corpus,
        "the resolution read {new} of a {corpus} byte corpus"
    );
    reset_ledger_reads();
    refuse_waiting_review(&root, SPEC, SPEC, None).unwrap();
    assert!(ledger_reads() < 3 * corpus);
}

/// A review no ownership witness could disqualify: the `--pr` handshake.
fn phase3(mut rec: JsonObj) -> JsonObj {
    rec.insert("mode".into(), project::PHASE3_MODE.into());
    rec
}

fn legacy_file(root: &Path, key: &str, body: serde_json::Value) {
    let dir = root.join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{key}.json")), body.to_string()).unwrap();
}

/// Sets the read budget of this thread's bounded passes; restores it on drop.
struct ReadBudgetGuard;

impl ReadBudgetGuard {
    fn set(bytes: u64) -> Self {
        super::READ_BUDGET.with(|b| b.set(Some(bytes)));
        Self
    }
}

impl Drop for ReadBudgetGuard {
    fn drop(&mut self) {
        super::READ_BUDGET.with(|b| b.set(None));
    }
}

/// Admission and resolution both refuse, without a waiting review in hand,
/// because one cannot be ruled out; returns the refusal.
fn refused_uncovered(root: &Path) -> String {
    let early = refuse_waiting_review(root, SPEC, SPEC, None).unwrap_err();
    match review(root) {
        Err(ReworkReviewError::Waiting(why)) => {
            assert_eq!(why, early);
            assert!(why.contains("cannot be ruled out"), "{why}");
            why
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Finding 2: the recovery reads under an aggregate byte budget. When the
/// budget ends before the ledger that holds the wait, the search is
/// incomplete and admission is refused, naming the file; it is never
/// answered from the part that was read.
#[test]
fn bug_2629_recovery_past_its_read_budget_refuses_by_name() {
    let (_tmp, root, _env) = fixture();
    append(&root, 3698, SPEC, &record("request-changes", -40));
    append(&root, 3699, SPEC, &record("waiting", -30));
    lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
    let first = std::fs::metadata(store::ledger_file(&root, ChangeKey::pr(3698)))
        .unwrap()
        .len();
    {
        let _budget = ReadBudgetGuard::set(first);
        let why = refused_uncovered(&root);
        assert!(why.contains("PR-3699.jsonl"), "{why}");
        assert!(why.contains("read budget"), "{why}");
    }
    assert!(refused_waiting(&root), "the full budget finds the wait");
}

/// Finding 3: a terminal annotation closes a wait only in the view of the
/// key it names. One that names the change key but sits on another change's
/// ledger is never seen by the change-keyed reader, and one that matches key
/// and source without being a closed record closes nothing.
#[test]
fn bug_2629_only_a_closure_in_the_keys_own_view_closes_a_withheld_wait() {
    let annotate = |root: &Path, key: &str, closed: bool| {
        let current = rv::current_verdict(root, "PR-3698").unwrap();
        let source: JsonObj = serde_json::from_str(&current.body).unwrap();
        let mut note = source.clone();
        note.insert("terminal_wait_key".into(), key.into());
        note.insert("terminal_wait_source".into(), source.into());
        if closed {
            note.insert("closed_by_merge".into(), "PR-4000".into());
            note.insert("closed_at".into(), chrono::Utc::now().to_rfc3339().into());
        }
        note
    };
    {
        // The spec-keyed closure is genuine; the change-keyed one is foreign.
        let (_tmp, root, _env) = fixture();
        append(&root, 3698, SPEC, &record("waiting", -30));
        let foreign = annotate(&root, "PR-3698", true);
        let current = rv::current_verdict(&root, SPEC).unwrap();
        assert!(rv::close_terminal_wait(&root, SPEC, ChangeKey::pr(3698), &current).unwrap());
        append(&root, 4000, SPEC, &foreign);
        assert!(refused_waiting(&root), "healthy ledger, foreign closure");
        lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
        assert!(refused_waiting(&root), "failed discovery, foreign closure");
    }

    // Both keys annotated on the wait's own ledger, neither record closed.
    let (_tmp, root, _env) = fixture();
    append(&root, 3698, SPEC, &record("waiting", -30));
    for open in ["PR-3698", SPEC].map(|key| annotate(&root, key, false)) {
        append(&root, 3698, SPEC, &open);
    }
    assert!(
        refused_waiting(&root),
        "healthy ledger, unclosed annotation"
    );
    lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
    assert!(
        refused_waiting(&root),
        "failed discovery, unclosed annotation"
    );
}

/// Finding 4: a write-ahead binding lists a change for the spec before any
/// review of it is appended there. Its current wait, recorded for another
/// spec, does not refuse this one; a wait recorded for this spec does.
#[test]
fn bug_2629_a_listed_changes_wait_for_another_spec_does_not_refuse() {
    for (recorded_for, refuses) in [("BUG-9000", false), (SPEC, true)] {
        let (_tmp, root, _env) = fixture();
        append(&root, 77, SPEC, &record("request-changes", -60));
        append(&root, 50, recorded_for, &phase3(record("waiting", -30)));
        lose_binding_beside_corrupt_ledger(&root, SPEC, 77);
        // Listed by a binding only: the hint names PR-77 alone.
        std::fs::write(
            ledger::bindings::bindings_dir(&root)
                .join(SPEC)
                .join("PR-50"),
            b"",
        )
        .unwrap();
        std::fs::write(
            ledger::ledger_dir(&root).join(format!("by-spec/{SPEC}.json")),
            serde_json::json!({ "changes": ["PR-77"] }).to_string(),
        )
        .unwrap();
        let readable = rv::current_verdict(&root, "PR-50")
            .and_then(|current| rv::parse_recorded_verdict(&current.body))
            .unwrap();
        assert_eq!(readable.kind, rv::VerdictKind::Waiting);
        assert_eq!(
            refused_waiting(&root),
            refuses,
            "recorded for {recorded_for}"
        );
        if !refuses {
            unavailable(&root);
        }
    }
}

/// Finding 6: a wait on one bound change does not keep back the
/// request-changes findings that are readable as current on another, as in
/// a healthy ledger. Findings a withheld later round may have superseded
/// are not imported.
#[test]
fn bug_2629_readable_findings_beside_a_withheld_wait_are_still_imported() {
    use crate::rework_direction::ReviewResult;
    let imported = |root: &Path, finding: &str| {
        refuse_waiting_review(root, SPEC, SPEC, None).unwrap();
        match resolve_rework_review(root, SPEC, SPEC, None) {
            Ok(ReviewResult::Available(block)) => {
                assert_eq!(block.matches(finding).count(), 1, "{block}");
            }
            other => panic!("expected findings, got {other:?}"),
        }
    };
    {
        // A ledger round no witness could disqualify.
        let (_tmp, root, _env) = fixture();
        append(&root, 3698, SPEC, &record("waiting", -30));
        append(&root, 4000, SPEC, &phase3(record("request-changes", -10)));
        assert!(matches!(
            resolve_rework_review(&root, SPEC, SPEC, None),
            Ok(ReviewResult::Available(_))
        ));
        lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
        assert!(rv::current_verdict(&root, "PR-3698").is_none());
        imported(&root, "repair the defect");
    }

    {
        // A legacy verdict file of an indexed change.
        let (_tmp, root, _env) = fixture();
        append(&root, 3698, SPEC, &record("waiting", -30));
        lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
        legacy_file(
            &root,
            "PR-4001",
            serde_json::json!({
                "verdict": "request-changes", "reviewed_sha": HEAD,
                "findings": ["repair the legacy defect"],
            }),
        );
        project::index_spec(&root, SPEC, ChangeKey::pr(4001)).unwrap();
        imported(&root, "repair the legacy defect");
    }

    // The readable refusal is followed by a round whose ownership cannot be
    // verified: it is not known to be current, and the wait refuses.
    let (_tmp, root, _env) = fixture();
    append(&root, 3698, SPEC, &record("waiting", -30));
    append(&root, 4000, SPEC, &phase3(record("request-changes", -20)));
    append(&root, 4000, SPEC, &record("approved", -10));
    lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
    assert!(refused_waiting(&root));
}

/// Finding 7: a ledger whose bindings certify nothing (written before the
/// bindings existed: no marker, no binding, no hint) can still hold a
/// waiting review that names the spec. The per-key check found it by its
/// event's label; so does the bounded pass that replaces it, and when that
/// pass cannot cover the ledger, admission is refused.
#[test]
fn bug_2629_an_unbootstrapped_ledger_is_searched_by_label() {
    let (_tmp, root, _env) = fixture();
    append(&root, 3698, SPEC, &phase3(record("waiting", -30)));
    std::fs::remove_file(ledger::bindings::marker_file(&root)).unwrap();
    std::fs::remove_dir_all(ledger::bindings::bindings_dir(&root)).unwrap();
    std::fs::remove_dir_all(ledger::ledger_dir(&root).join("by-spec")).unwrap();
    let labels = vec![SPEC.to_string()];
    assert!(change_verdict_bound_to(&root, "PR-3698", &labels));
    assert!(super::discover(&root, &labels, None, false).binds("PR-3698"));
    assert!(refused_waiting(&root));
    // Another spec's reviews stay its own.
    assert!(!super::discover(&root, &["BUG-9000".to_string()], None, false).binds("PR-3698"));

    let _budget = ReadBudgetGuard::set(1);
    let why = refused_uncovered(&root);
    assert!(why.contains("could not be enumerated"), "{why}");
    assert!(why.contains("PR-3698.jsonl"), "{why}");
}

struct StoreFixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    backend: aida_core::GitBackend,
    storage: aida_core::Storage,
    cwd: PathBuf,
}

impl Drop for StoreFixture {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.cwd);
    }
}

/// A project with one stored spec, entered as the working directory.
fn store_fixture(req: aida_core::Requirement) -> StoreFixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::create_dir(root.join(".aida")).unwrap();
    let store_root = root.join(".aida-store");
    let backend = crate::test_env::home_fixture_backend(&store_root).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let common = aida_core::allocation_authority::common_dir(&store_root).unwrap();
    std::fs::set_permissions(
        common.join(aida_core::allocation_authority::CONFIG_FILE),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let storage = aida_core::Storage::new(&store_root);
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();
    let cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&root).unwrap();
    StoreFixture {
        _tmp: tmp,
        root,
        backend,
        storage,
        cwd,
    }
}

fn rework(storage: &aida_core::Storage) -> anyhow::Result<()> {
    crate::queue_cmd::handle_queue_rework(
        storage,
        SPEC,
        false,
        Some("implementer"),
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        Some("codex"),
    )
}

/// The command itself: an Approved spec whose only wait is withheld by the
/// failed discovery is refused before any status, comment or queue write.
#[test]
fn bug_2629_queue_rework_refuses_a_withheld_wait_without_mutation() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let mut req = aida_core::Requirement::new("external wait".into(), String::new());
    req.spec_id = Some(SPEC.into());
    req.status = aida_core::RequirementStatus::Approved;
    let fx = store_fixture(req);
    let ledger_root = ledger::store_root(&fx.root).unwrap();
    append(&ledger_root, 3698, SPEC, &record("waiting", -30));
    lose_binding_beside_corrupt_ledger(&ledger_root, SPEC, 3698);
    assert!(rv::current_verdict(&fx.root, "PR-3698").is_none());
    let lock = aida_core::db::store_write_lock_path(fx.storage.path());
    let observations = std::rc::Rc::new(Cell::new(0));
    struct ResetProbe;
    impl Drop for ResetProbe {
        fn drop(&mut self) {
            super::ON_LEDGER_STEP.with(|probe| *probe.borrow_mut() = None);
        }
    }
    let _reset = ResetProbe;
    super::ON_LEDGER_STEP.with(|probe| {
        let observations = observations.clone();
        *probe.borrow_mut() = Some(Box::new(move || {
            use fs2::FileExt as _;
            assert!(!aida_core::db::any_store_write_lock_held());
            let writer = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock)
                .unwrap();
            writer
                .try_lock_exclusive()
                .expect("review discovery holds no store writer lock");
            observations.set(observations.get() + 1);
        }));
    });
    let before = serde_json::to_value(fx.backend.load().unwrap()).unwrap();
    for _ in 0..2 {
        let err = rework(&fx.storage).unwrap_err().to_string();
        assert!(err.contains("WAITING") && err.contains("Host:"), "{err}");
        assert_eq!(
            serde_json::to_value(fx.backend.load().unwrap()).unwrap(),
            before
        );
        assert!(fx.storage.queue_users().unwrap().is_empty());
        assert!(!fx.root.join(".aida/merge-holds").exists());
    }
    assert!(observations.get() > 0);
}

/// Acceptance 2: a spec on another track is refused before the gate ledger
/// is listed or read at all.
// trace:BUG-b6b1278890774ef993d2ebc327862470-393.aca2b718 | ai:claude
#[test]
fn bug_2629_track_refusal_precedes_any_gate_ledger_read() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let _env =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None), ("AIDA_TRACK", None)]);
    let mut req = aida_core::Requirement::new("another track".into(), String::new());
    req.spec_id = Some(SPEC.into());
    req.status = aida_core::RequirementStatus::Approved;
    req.tags.insert("track:elsewhere".into());
    let fx = store_fixture(req);
    let ledger_root = ledger::store_root(&fx.root).unwrap();
    append(&ledger_root, 3698, SPEC, &record("waiting", -30));
    let before = serde_json::to_value(fx.backend.load().unwrap()).unwrap();
    reset_ledger_reads();
    let err = rework(&fx.storage).unwrap_err().to_string();
    assert!(err.contains("reason=other-track"), "{err}");
    assert_eq!(
        (ledger_reads(), work::DIRECTORY_ENTRIES.with(Cell::get)),
        (0, 0),
        "the gate ledger was read before the track refusal"
    );
    assert_eq!(
        serde_json::to_value(fx.backend.load().unwrap()).unwrap(),
        before
    );
    assert!(fx.storage.queue_users().unwrap().is_empty());
}

/// Acceptance 1: a phase that outlives the notice interval names itself and
/// its elapsed time; a quick one says nothing.
// trace:BUG-b6b1278890774ef993d2ebc327862470-393.ac56b111 | ai:claude
#[test]
fn bug_2629_a_slow_rework_phase_names_itself() {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = said.clone();
    let out = super::announce_slow_phase(
        "reading the reviews of BUG-2629 from the gate ledger",
        Duration::from_millis(20),
        move |line| sink.lock().unwrap().push(line),
        || {
            std::thread::sleep(Duration::from_millis(300));
            7
        },
    );
    assert_eq!(out, 7);
    let lines = said.lock().unwrap().clone();
    assert!(!lines.is_empty(), "a slow phase must report");
    assert!(
        lines.iter().all(|line| line
            .starts_with("still reading the reviews of BUG-2629 from the gate ledger (")
            && line.ends_with("s elapsed)")),
        "{lines:?}"
    );

    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = said.clone();
    super::announce_slow_phase(
        "quick",
        Duration::from_secs(60),
        move |line| sink.lock().unwrap().push(line),
        || (),
    );
    assert!(said.lock().unwrap().is_empty());
}

// Round 5: a source that could not be listed, read, parsed or verified is
// unknown, never absent, and an unknown source refuses rework by name.

/// Finding 1: a legacy file that is malformed or refused (a link) is not an
/// absent record. The phase-3 refusal behind it is not known to be current,
/// so its findings are not imported beside the wait on another change, and
/// with no wait in hand admission is still refused, naming the file.
#[test]
fn bug_2629_a_refused_legacy_file_is_unknown_not_absent() {
    fn malformed(root: &Path) {
        let dir = root.join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("PR-4000.json"), b"{ not a verdict").unwrap();
    }
    fn linked(root: &Path) {
        let target = root.join("elsewhere.json");
        std::fs::write(
            &target,
            serde_json::json!({"verdict": "approved", "reviewed_sha": HEAD}).to_string(),
        )
        .unwrap();
        let dir = root.join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(target, dir.join("PR-4000.json")).unwrap();
    }
    let refusals: [(&str, fn(&Path)); 2] = [("malformed", malformed), ("linked", linked)];
    for (name, refuse) in refusals {
        {
            let (_tmp, root, _env) = fixture();
            append(&root, 3698, SPEC, &record("waiting", -30));
            append(&root, 4000, SPEC, &phase3(record("request-changes", -10)));
            lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
            assert!(
                matches!(
                    resolve_rework_review(&root, SPEC, SPEC, None),
                    Ok(crate::rework_direction::ReviewResult::Available(_))
                ),
                "{name}: readable findings beside the wait are imported"
            );
            refuse(&root);
            assert!(refused_waiting(&root), "{name}: the wait refuses");
        }

        // No wait in hand: the unknown file alone refuses.
        let (_tmp, root, _env) = fixture();
        append(&root, 4000, SPEC, &phase3(record("request-changes", -10)));
        lose_binding_beside_corrupt_ledger(&root, SPEC, 4000);
        unavailable(&root);
        refuse(&root);
        let why = refused_uncovered(&root);
        assert!(why.contains("PR-4000.json"), "{name}: {why}");
    }
}

/// Finding 2: the hint and the binding of the spec's only waiting change
/// are gone and its bindings directory remains, so a strict full read is
/// called for, and a malformed unrelated ledger refuses it. That read
/// enumerated nothing: the change is found by label instead, and when the
/// label pass cannot cover the ledger admission is refused.
#[test]
fn bug_2629_a_refused_lost_binding_read_still_finds_the_waiting_change() {
    let (_tmp, root, _env) = fixture();
    append(&root, 3698, SPEC, &record("waiting", -30));
    assert!(refused_waiting(&root));
    lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
    std::fs::remove_file(ledger::ledger_dir(&root).join(format!("by-spec/{SPEC}.json"))).unwrap();
    assert!(ledger::bindings::bindings_dir(&root).join(SPEC).is_dir());
    let found = super::discover(&root, &[SPEC.to_string()], None, false);
    assert!(found.failure().is_some() && found.refusal().is_none());
    assert!(refused_waiting(&root));

    let _budget = ReadBudgetGuard::set(1);
    let why = refused_uncovered(&root);
    assert!(why.contains("could not be enumerated"), "{why}");
}

/// Finding 3: the label pass tells a ledger directory that does not exist
/// (empty) from one it could not inspect (refused, by name).
#[test]
fn bug_2629_the_label_pass_refuses_what_it_cannot_inspect() {
    let labels = vec![SPEC.to_string()];
    let (_tmp, root, _env) = fixture();
    assert_eq!(super::labelled_changes(&root, &labels), Ok(Vec::new()));
    append(&root, 3698, SPEC, &phase3(record("waiting", -30)));
    assert_eq!(
        super::labelled_changes(&root, &labels),
        Ok(vec![ChangeKey::pr(3698)])
    );
    let dir = ledger::ledger_dir(&root);

    // A line that is not an event: passed over, unless it carries the label.
    let torn = dir.join("PR-7.jsonl");
    std::fs::write(&torn, b"{\"kind\":\"verdict\",\"spec\":\"BUG-9000\",\n").unwrap();
    assert_eq!(
        super::labelled_changes(&root, &labels),
        Ok(vec![ChangeKey::pr(3698)])
    );
    std::fs::write(&torn, b"{\"kind\":\"verdict\",\"spec\":\"bug-2629\",\n").unwrap();
    let why = super::labelled_changes(&root, &labels).unwrap_err();
    assert!(
        why.contains("PR-7.jsonl") && why.contains("cannot be parsed"),
        "{why}"
    );

    // A ledger that is not a readable regular text file.
    std::fs::write(&torn, [0xff, 0xfe, b'\n']).unwrap();
    let why = super::labelled_changes(&root, &labels).unwrap_err();
    assert!(why.contains("PR-7.jsonl"), "{why}");
    std::fs::remove_file(&torn).unwrap();
    std::fs::create_dir(&torn).unwrap();
    let why = super::labelled_changes(&root, &labels).unwrap_err();
    assert!(
        why.contains("PR-7.jsonl") && why.contains("not a regular file"),
        "{why}"
    );

    // The command refuses on it once the bindings certify nothing.
    std::fs::remove_file(ledger::bindings::marker_file(&root)).unwrap();
    std::fs::remove_dir_all(ledger::bindings::bindings_dir(&root)).unwrap();
    std::fs::remove_dir_all(dir.join("by-spec")).unwrap();
    let why = refused_uncovered(&root);
    assert!(why.contains("could not be enumerated"), "{why}");
    assert!(why.contains("PR-7.jsonl"), "{why}");
    std::fs::remove_dir(&torn).unwrap();
    assert!(refused_waiting(&root));

    // A ledger directory that cannot be listed.
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::write(&dir, b"").unwrap();
    let why = super::labelled_changes(&root, &labels).unwrap_err();
    assert!(why.contains("cannot be listed"), "{why}");
}

/// Finding 3, recovery side: an inventoried ledger that exists and cannot
/// be read, or holds a line that is not an event, is unknown.
#[test]
fn bug_2629_an_unreadable_ledger_of_the_spec_refuses() {
    for torn in [&b"{\"kind\":\"verdict\",\n"[..], &[0xff, 0xfe, b'\n'][..]] {
        let (_tmp, root, _env) = fixture();
        append(&root, 4000, SPEC, &phase3(record("request-changes", -10)));
        append(&root, 4001, SPEC, &phase3(record("request-changes", -5)));
        lose_binding_beside_corrupt_ledger(&root, SPEC, 4000);
        unavailable(&root);
        std::fs::write(store::ledger_file(&root, ChangeKey::pr(4001)), torn).unwrap();
        let why = refused_uncovered(&root);
        assert!(why.contains("PR-4001.jsonl"), "{why}");
    }
}

/// Finding 4: the legacy files are read within the same aggregate budget as
/// the ledgers, and the findings kept across keys are charged to it. Small
/// ledgers beside large legacy findings exhaust it at a legacy file.
#[test]
fn bug_2629_legacy_records_and_kept_findings_spend_the_recovery_budget() {
    use crate::rework_direction::ReviewResult;
    let (_tmp, root, _env) = fixture();
    append(&root, 3698, SPEC, &record("waiting", -30));
    for pr in 4001..=4003u64 {
        legacy_file(
            &root,
            &format!("PR-{pr}"),
            serde_json::json!({
                "verdict": "request-changes", "reviewed_sha": HEAD,
                "findings": [format!("defect {pr} {}", "x".repeat(1000))],
            }),
        );
        project::index_spec(&root, SPEC, ChangeKey::pr(pr)).unwrap();
    }
    lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
    let size = |path: PathBuf| std::fs::metadata(path).unwrap().len();
    let ledger_bytes = size(store::ledger_file(&root, ChangeKey::pr(3698)));
    let legacy_bytes = size(root.join(".aida/review-verdicts/PR-4001.json"));
    {
        // The ledger, one legacy record read and kept (body and parsed
        // findings), and half of the next.
        let _budget = ReadBudgetGuard::set(ledger_bytes + 3 * legacy_bytes + legacy_bytes / 2);
        let why = refused_uncovered(&root);
        assert!(why.contains("PR-4002.json"), "{why}");
        assert!(why.contains("read budget"), "{why}");
    }
    {
        // Every byte read fits; the findings it would keep do not.
        let _budget = ReadBudgetGuard::set(ledger_bytes + legacy_bytes + 16);
        let why = refused_uncovered(&root);
        assert!(why.contains("the findings recorded for PR-4001"), "{why}");
        assert!(why.contains("read budget"), "{why}");
    }
    match resolve_rework_review(&root, SPEC, SPEC, None) {
        Ok(ReviewResult::Available(block)) => {
            for pr in 4001..=4003 {
                assert!(block.contains(&format!("defect {pr}")), "{block}");
            }
        }
        other => panic!("the full budget imports the findings, got {other:?}"),
    }
}

/// Finding 4: the bound holds on the open handle. A ledger that grows after
/// it was listed is refused at the budget, not read past it.
#[test]
fn bug_2629_a_read_is_capped_at_the_remaining_budget() {
    let (_tmp, root, _env) = fixture();
    let dir = ledger::ledger_dir(&root);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("PR-1.jsonl");
    std::fs::write(&file, vec![b'x'; 64]).unwrap();
    let pinned = ledger::checked::pin(&root).unwrap().unwrap();
    let _budget = ReadBudgetGuard::set(100);
    let mut budget = super::ReadBudget::new(u64::MAX);
    let mut read = |budget: &mut super::ReadBudget, name: &str| {
        budget.read(&pinned, false, name, &dir.join(name))
    };
    assert_eq!(
        read(&mut budget, "PR-1.jsonl")
            .ok()
            .flatten()
            .map(|b| b.len()),
        Some(64)
    );
    assert_eq!(budget.left, 36);
    assert!(matches!(
        read(&mut budget, "PR-1.jsonl"),
        Err(super::Unread::Exhausted(why)) if why.contains("PR-1.jsonl")
    ));
    assert_eq!(budget.left, 36, "a refused read spends nothing");
    assert!(matches!(read(&mut budget, "absent"), Ok(None)));
    // No by-spec directory: no unbound ledger.
    assert!(matches!(
        budget.read(
            &pinned,
            true,
            "BUG-1.jsonl",
            &dir.join("by-spec/BUG-1.jsonl")
        ),
        Ok(None)
    ));
    assert!(budget.charge(&"kept", 37).is_err());
    budget.charge(&"kept", 36).unwrap();
    // The legacy reader stops at its allowance as well.
    legacy_file(
        &root,
        "PR-1",
        serde_json::json!({"verdict": "approved", "reviewed_sha": HEAD}),
    );
    let mut consumed = 0;
    let over = rv::checked_legacy_record(&root, "PR-1", 8, &mut consumed).unwrap_err();
    assert!(over.get_ref().is_some_and(|e| e.is::<rv::OverReadLimit>()));
    assert_eq!(consumed, 0, "a file over the allowance is not read");
    let (body, _) = rv::checked_legacy_record(&root, "PR-1", 4096, &mut consumed)
        .unwrap()
        .unwrap();
    assert_eq!(consumed, body.len() as u64);
    assert!(
        rv::checked_legacy_record(&root, "PR-2", 4096, &mut consumed)
            .unwrap()
            .is_none()
    );
}

/// Answers for the signed publication of every key; restored on drop.
struct PublishedGuard;

impl PublishedGuard {
    fn set(answer: impl Fn(&str) -> anyhow::Result<Option<rv::CurrentVerdict>> + 'static) -> Self {
        super::PUBLISHED.with(|stub| *stub.borrow_mut() = Some(Box::new(answer)));
        Self
    }
}

impl Drop for PublishedGuard {
    fn drop(&mut self) {
        super::PUBLISHED.with(|stub| *stub.borrow_mut() = None);
    }
}

fn signed(root: &Path, verdict: &str) -> rv::CurrentVerdict {
    rv::CurrentVerdict {
        body: serde_json::json!({
            "verdict": verdict, "reviewed_sha": HEAD,
            "summary": "review publication refused: bad signature",
        })
        .to_string(),
        source: rv::VerdictSource::File(root.join(".aida-store/reviews/v1/changes")),
        written_at: None,
    }
}

/// Findings 5 and 7. A verified signed approval answers for its keys, so
/// the raw wait it superseded no longer refuses, and the evidence is still
/// named unavailable rather than empty (the signing fixture's half of this
/// lives with the publication tests). A publication that cannot be verified
/// is no signed record: it supersedes nothing and supplies no findings.
#[test]
fn bug_2629_only_a_verified_publication_takes_signed_precedence() {
    {
        let (_tmp, root, _env) = fixture();
        append(&root, 3698, SPEC, &record("waiting", -30));
        lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
        assert!(refused_waiting(&root));
        {
            let at = root.clone();
            let _signed = PublishedGuard::set(move |_| Ok(Some(signed(&at, "approved"))));
            assert!(!refused_waiting(&root));
            let why = unavailable(&root);
            assert!(why.contains("ownership discovery failed"), "{why}");
        }
        let _signed = PublishedGuard::set(|_| Err(anyhow::anyhow!("bad signature")));
        assert!(refused_waiting(&root), "the raw wait still refuses");
    }
    {
        // Beside a wait on another change, the diagnostic is not findings,
        // and the findings it may supersede are not imported.
        let (_tmp, root, _env) = fixture();
        append(&root, 3698, SPEC, &record("waiting", -30));
        append(&root, 4000, SPEC, &phase3(record("request-changes", -10)));
        lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
        assert!(matches!(
            resolve_rework_review(&root, SPEC, SPEC, None),
            Ok(crate::rework_direction::ReviewResult::Available(_))
        ));
        let _signed = PublishedGuard::set(|key| match key {
            "PR-4000" => Err(anyhow::anyhow!("bad signature")),
            _ => Ok(None),
        });
        assert!(refused_waiting(&root));
    }

    // No wait in hand: the unverifiable publication alone refuses, by name.
    let (_tmp, root, _env) = fixture();
    append(&root, 4000, SPEC, &phase3(record("request-changes", -10)));
    lose_binding_beside_corrupt_ledger(&root, SPEC, 4000);
    unavailable(&root);
    let _signed = PublishedGuard::set(|_| Err(anyhow::anyhow!("bad signature")));
    let why = refused_uncovered(&root);
    assert!(
        why.contains("cannot be verified") && why.contains("bad signature"),
        "{why}"
    );
}

/// Finding 6: an unbound review is read from its bound copy only when that
/// copy can answer. A copy whose ownership is unverified answers nothing,
/// and the unbound record, which no foreign change can own, still does: an
/// approval keeps clearing the older wait, and a refusal keeps its findings.
#[test]
fn bug_2629_an_unbound_review_outlives_its_withheld_bound_copy() {
    use crate::rework_direction::ReviewResult;
    let spec = ledger::GateSpec::with_key(SPEC, None);
    {
        let (_tmp, root, _env) = fixture();
        store::append_unbound(&root, &spec, &record("waiting", -30)).unwrap();
        assert!(refused_waiting(&root));
        let approval = record("approved", -10);
        store::append_unbound(&root, &spec, &approval).unwrap();
        append(&root, 3698, SPEC, &approval);
        assert!(!refused_waiting(&root), "healthy ledger");
        lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
        assert!(rv::current_verdict(&root, "PR-3698").is_none());
        assert!(!refused_waiting(&root), "the unbound approval answers");
        unavailable(&root);
    }

    let (_tmp, root, _env) = fixture();
    append(&root, 3698, SPEC, &record("waiting", -30));
    let refusal = record("request-changes", -10);
    store::append_unbound(&root, &spec, &refusal).unwrap();
    append(&root, 4000, SPEC, &refusal);
    lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
    assert!(rv::current_verdict(&root, "PR-4000").is_none());
    match resolve_rework_review(&root, SPEC, SPEC, None) {
        Ok(ReviewResult::Available(block)) => {
            assert_eq!(block.matches("repair the defect").count(), 1, "{block}");
        }
        other => panic!("expected the unbound findings, got {other:?}"),
    }
}

/// Round-6 finding 1 (acceptance 3 on a healthy ledger), in the reported
/// shape: two changes of the spec with six fingerprinted review rounds each
/// (BUG-2706's two pull requests), beside `targets` more bound changes of
/// the spec. Returns how many change ledgers the spec has and their bytes.
fn healthy_many_target_spec(root: &Path, targets: u64) -> (u64, u64) {
    for (pr, base) in [(4401, -2000), (4402, -1500)] {
        for round in 0..6 {
            append(
                root,
                pr,
                SPEC,
                &record("request-changes", base + 60 * round),
            );
        }
    }
    for n in 0..targets {
        append(
            root,
            500 + n,
            SPEC,
            &record("request-changes", -900 + n as i64),
        );
    }
    let bytes = (500..500 + targets)
        .chain([4401, 4402])
        .map(|pr| {
            std::fs::metadata(store::ledger_file(root, ChangeKey::pr(pr)))
                .unwrap()
                .len()
        })
        .sum();
    (targets + 2, bytes)
}

/// Filesystem work of one measured call: discoveries opened, shared
/// inventory re-validations, directory entries listed, ledger bytes read.
#[derive(Debug, Clone, Copy)]
struct Work {
    opened: usize,
    checks: usize,
    entries: usize,
    binding_lists: usize,
    ledger_opens: usize,
    index_reads: usize,
    membership: usize,
    bytes: u64,
    took: std::time::Duration,
}

fn measured<T>(run: impl FnOnce() -> T) -> (T, Work) {
    reset_ledger_reads();
    work::INVENTORY_CHECKS.with(|n| n.set(0));
    work::DISCOVERIES_OPENED.with(|n| n.set(0));
    work::BINDING_LISTS.with(|n| n.set(0));
    work::LEDGER_OPENS.with(|n| n.set(0));
    work::INDEX_READS.with(|n| n.set(0));
    work::MEMBERSHIP_INPUTS.with(|n| n.set(0));
    let started = std::time::Instant::now();
    let out = run();
    let spent = Work {
        opened: work::DISCOVERIES_OPENED.with(Cell::get),
        checks: work::INVENTORY_CHECKS.with(Cell::get),
        entries: work::DIRECTORY_ENTRIES.with(Cell::get),
        binding_lists: work::BINDING_LISTS.with(Cell::get),
        ledger_opens: work::LEDGER_OPENS.with(Cell::get),
        index_reads: work::INDEX_READS.with(Cell::get),
        membership: work::MEMBERSHIP_INPUTS.with(Cell::get),
        bytes: ledger_reads(),
        took: started.elapsed(),
    };
    (out, spent)
}

/// Round-6 finding 1: the ownership of every change of a healthy spec is
/// checked under ONE discovery. A discovery per change re-validated the
/// shared inventory each time (a listing of the spec's bindings and a stat
/// of every folded ledger), which is quadratic filesystem work over the
/// spec's changes although hardly a byte is read. The inventory pass opens
/// one discovery, re-validates nothing, lists a number of directory entries
/// linear in the spec's changes and reads each ledger a bounded number of
/// times; the whole resolution stays linear and finishes in seconds.
// trace:BUG-b6b1278890774ef993d2ebc327862470-393.ac915e51 | ai:claude
#[test]
fn bug_2629_one_discovery_checks_every_change_of_a_healthy_spec() {
    let labels = vec![SPEC.to_string()];
    let limit = std::time::Duration::from_secs(5);
    let run = |targets: u64| {
        let (_tmp, root, _env) = fixture();
        let (ledgers, bytes) = healthy_many_target_spec(&root, targets);
        let (bindings, pass) = measured(|| super::discover(&root, &labels, None, false));
        eprintln!("bug_2629 healthy {ledgers} ledgers of {bytes} bytes: inventory pass {pass:?}");
        assert_eq!(bindings.failure(), None);
        assert_eq!(bindings.refusal(), None);
        for pr in (500..500 + targets).chain([4401, 4402]) {
            assert!(bindings.binds(&format!("PR-{pr}")), "PR-{pr}");
        }
        assert_eq!(pass.opened, 1, "one discovery for {ledgers} changes");
        assert_eq!(pass.checks, 0, "no inventory is re-validated per change");
        assert!(
            pass.entries as u64 <= 3 * ledgers + 16,
            "{} directory entries listed for {ledgers} changes",
            pass.entries
        );
        // Each ledger once for its events and once for the ownership index.
        assert!(
            pass.bytes < 3 * bytes,
            "{} bytes read of {bytes} in {ledgers} ledgers",
            pass.bytes
        );
        assert!(pass.took < limit, "the inventory pass took {:?}", pass.took);
        let mut cache = super::DiscoveryCache::default();
        let (_, whole) = measured(|| {
            // Admission and findings selection use the same command context.
            for _ in 0..2 {
                crate::queue_cmd::work::resolve_rework_review_cached(
                    &mut cache, &root, SPEC, SPEC, None,
                )
                .unwrap();
            }
        });
        eprintln!("bug_2629 healthy {ledgers} ledgers: whole resolution {whole:?}");
        assert_eq!(whole.opened, 1, "{whole:?}");
        assert_eq!(whole.binding_lists, 1, "{whole:?}");
        assert_eq!(whole.index_reads, 1, "{whole:?}");
        // One set build/probe per binding and hint for the inventory and
        // ownership checks, plus one set insertion per item in their union.
        assert_eq!(whole.membership as u64, 6 * ledgers, "{whole:?}");
        assert_eq!(whole.checks, 0, "{whole:?}");
        assert!(whole.ledger_opens as u64 <= ledgers + 2, "{whole:?}");
        assert!(whole.took < limit, "the resolution took {:?}", whole.took);
        (ledgers, bytes, pass, whole)
    };
    let (few, few_bytes, few_pass, few_whole) = run(6);
    let (many, many_bytes, many_pass, many_whole) = run(48);
    {
        let (_tmp, root, _env) = fixture();
        healthy_many_target_spec(&root, 48);
        std::fs::remove_file(
            ledger::bindings::bindings_dir(&root)
                .join(SPEC)
                .join("PR-547"),
        )
        .unwrap();
        let (bindings, pass) = measured(|| super::discover(&root, &labels, None, false));
        assert!(bindings.binds("PR-547"));
        assert_eq!(bindings.failure(), None);
        assert_eq!(pass.opened, 1, "{pass:?}");
        assert_eq!(pass.binding_lists, 1, "{pass:?}");
        assert_eq!(pass.index_reads, 1, "{pass:?}");
        assert_eq!(pass.ledger_opens, 50, "{pass:?}");
        assert!(pass.membership <= 9 * 50, "{pass:?}");
        eprintln!("bug_2629 50 ledgers with a lost binding: {pass:?}");
    }

    // The pass does not open more discoveries for more changes, and what
    // the whole resolution reads grows with the spec's ledgers, not with
    // their square.
    assert_eq!(few_pass.opened, many_pass.opened);
    let growth = many.div_ceil(few);
    assert!(
        many_pass.entries <= few_pass.entries * growth as usize + 16,
        "directory entries grew from {} to {} for {few} -> {many} changes",
        few_pass.entries,
        many_pass.entries
    );
    assert!(
        many_whole.bytes * few_bytes <= 2 * few_whole.bytes.max(few_bytes) * many_bytes,
        "the resolution read {} of {many_bytes} bytes against {} of {few_bytes}",
        many_whole.bytes,
        few_whole.bytes
    );
    assert!(
        many_whole.opened <= 2 * growth as usize * few_whole.opened.max(1),
        "the resolution opened {} discoveries against {}",
        many_whole.opened,
        few_whole.opened
    );
}

/// Rewrite the last event of `pr`'s ledger in place.
fn rewrite_last_event(root: &Path, pr: u64, edit: impl Fn(&mut serde_json::Value)) {
    let file = store::ledger_file(root, ChangeKey::pr(pr));
    let body = std::fs::read_to_string(&file).unwrap();
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    let last = lines.last_mut().unwrap();
    let mut event: serde_json::Value = serde_json::from_str(last).unwrap();
    edit(&mut event);
    *last = event.to_string();
    std::fs::write(&file, lines.join("\n") + "\n").unwrap();
}

/// Round-6 finding 2: the recovery holds each record to the envelope the
/// strict reader requires of the ledger it is read from. A later phase-3
/// record that parses but does not belong there (another change's key, an
/// unsupported version, a sequence number that does not advance) is what
/// strict discovery refused; it must not answer for this change, clear its
/// wait or supply findings. The wait stays refused.
#[test]
fn bug_2629_a_misplaced_record_answers_nothing_in_recovery() {
    fn foreign(event: &mut serde_json::Value) {
        event["change"] = "PR-1".into();
    }
    fn unsupported(event: &mut serde_json::Value) {
        event["v"] = (event::EVENT_VERSION + 1).into();
    }
    fn replayed(event: &mut serde_json::Value) {
        event["seq"] = 0.into();
    }
    let misplacements: [(&str, fn(&mut serde_json::Value)); 3] = [
        ("another change's record", foreign),
        ("an unsupported version", unsupported),
        ("a sequence that does not advance", replayed),
    ];
    for later in ["approved", "request-changes"] {
        // In its place the later round answers: the wait is superseded.
        {
            let (_tmp, root, _env) = fixture();
            append(&root, 3698, SPEC, &record("waiting", -30));
            append(&root, 3698, SPEC, &phase3(record(later, -10)));
            lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
            assert!(!refused_waiting(&root), "a {later} round in its place");
        }
        for (name, misplace) in misplacements {
            let (_tmp, root, _env) = fixture();
            append(&root, 3698, SPEC, &record("waiting", -30));
            append(&root, 3698, SPEC, &phase3(record(later, -10)));
            rewrite_last_event(&root, 3698, misplace);
            lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
            assert!(refused_waiting(&root), "{name} ({later}) cleared the wait");
        }
    }
    // With no wait to name, the misplaced record is unknown evidence, by name.
    let (_tmp, root, _env) = fixture();
    append(&root, 3698, SPEC, &phase3(record("request-changes", -30)));
    append(&root, 3698, SPEC, &phase3(record("approved", -10)));
    rewrite_last_event(&root, 3698, foreign);
    lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
    let why = refused_uncovered(&root);
    assert!(
        why.contains("PR-3698.jsonl") && why.contains("PR-1"),
        "{why}"
    );
}

/// Round-6 finding 3: the recovery and the label pass open a ledger beneath
/// the pinned ledger directory, without following a link and without
/// blocking, and read the handle they validated. A link to a well-formed
/// ledger, a hard link and a FIFO are each refused by name, promptly.
#[test]
fn bug_2629_a_linked_or_special_ledger_is_refused_not_read() {
    fn linked(root: &Path, file: &Path) {
        let elsewhere = root.join("elsewhere.jsonl");
        std::fs::rename(file, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, file).unwrap();
    }
    fn hard_linked(root: &Path, file: &Path) {
        std::fs::hard_link(file, root.join("second-name.jsonl")).unwrap();
    }
    fn fifo(_: &Path, file: &Path) {
        use std::os::unix::ffi::OsStrExt;
        std::fs::remove_file(file).unwrap();
        let path = std::ffi::CString::new(file.as_os_str().as_bytes()).unwrap();
        // SAFETY: `path` is a NUL-terminated path owned for the call.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    }
    let replacements: [(&str, fn(&Path, &Path)); 3] = [
        ("a link", linked),
        ("a hard link", hard_linked),
        ("a FIFO", fifo),
    ];
    let labels = vec![SPEC.to_string()];
    for (name, replace) in replacements {
        let (_tmp, root, _env) = fixture();
        append(&root, 4000, SPEC, &phase3(record("request-changes", -10)));
        append(&root, 4001, SPEC, &phase3(record("request-changes", -5)));
        lose_binding_beside_corrupt_ledger(&root, SPEC, 4000);
        unavailable(&root);
        replace(&root, &store::ledger_file(&root, ChangeKey::pr(4001)));
        let started = std::time::Instant::now();
        let why = refused_uncovered(&root);
        assert!(why.contains("PR-4001.jsonl"), "{name}: {why}");
        let why = super::labelled_changes(&root, &labels).unwrap_err();
        assert!(why.contains("PR-4001.jsonl"), "{name}: {why}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{name} stalled the command for {:?}",
            started.elapsed()
        );
    }
}

/// Round-6 finding 4: a verdict event whose record has no verdict field is
/// a round nobody can read. It is unknown, never "no review": as the newest
/// round it must not discard the wait recorded before it.
#[test]
fn bug_2629_a_round_without_a_verdict_keeps_the_wait_before_it() {
    {
        let (_tmp, root, _env) = fixture();
        append(&root, 3698, SPEC, &record("waiting", -30));
        append(&root, 3698, SPEC, &phase3(record("approved", -10)));
        rewrite_last_event(&root, 3698, |event| {
            event["record"].as_object_mut().unwrap().remove("verdict");
        });
        lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
        assert!(refused_waiting(&root));
    }

    // With no wait before it, the unreadable round still refuses, by name.
    let (_tmp, root, _env) = fixture();
    append(&root, 3698, SPEC, &phase3(record("request-changes", -30)));
    append(&root, 3698, SPEC, &phase3(record("approved", -10)));
    rewrite_last_event(&root, 3698, |event| {
        event["record"].as_object_mut().unwrap().remove("verdict");
    });
    lose_binding_beside_corrupt_ledger(&root, SPEC, 3698);
    let why = refused_uncovered(&root);
    assert!(why.contains("no readable verdict"), "{why}");
}

/// Round-6 finding 5: a legacy file that does not parse, or is not UTF-8,
/// was read all the same, and the recovery's budget pays for it. Two such
/// files, each fitting the allowance alone, exhaust it together.
#[test]
fn bug_2629_a_malformed_legacy_file_spends_the_recovery_budget() {
    const LEGACY: usize = 4096;
    for body in [vec![b'x'; LEGACY], vec![0xff; LEGACY]] {
        let (_tmp, root, _env) = fixture();
        append(&root, 4000, SPEC, &phase3(record("request-changes", -10)));
        append(&root, 4001, SPEC, &phase3(record("request-changes", -5)));
        lose_binding_beside_corrupt_ledger(&root, SPEC, 4000);
        let dir = root.join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        for key in ["PR-4000", "PR-4001"] {
            std::fs::write(dir.join(format!("{key}.json")), &body).unwrap();
        }
        // Unbounded, both files are read and are unknown evidence.
        let why = refused_uncovered(&root);
        assert!(why.contains("PR-4000.json"), "{why}");
        // Every ledger of the label pass, then one legacy file and a half.
        let ledgers: u64 = std::fs::read_dir(ledger::ledger_dir(&root))
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
            .map(|e| e.metadata().unwrap().len())
            .sum();
        let _budget = ReadBudgetGuard::set(ledgers + (LEGACY + LEGACY / 2) as u64);
        let why = refused_uncovered(&root);
        assert!(
            why.contains("recovery read budget") && why.contains("PR-4001.json"),
            "{why}"
        );
    }
}

/// Sets the parse ceiling of this thread's bounded passes; restores it on
/// drop.
struct ParseBudgetGuard;

impl ParseBudgetGuard {
    fn set(lines: usize) -> Self {
        super::PARSE_BUDGET.with(|b| b.set(Some(lines)));
        Self
    }
}

impl Drop for ParseBudgetGuard {
    fn drop(&mut self) {
        super::PARSE_BUDGET.with(|b| b.set(None));
    }
}

/// Round-6 finding 6: the fallback passes count every line they try to
/// parse against one ceiling shared by the whole pass, and keep none of the
/// lines that fail. Many short malformed lines, well inside the byte budget,
/// stop the pass at the ceiling, by name.
#[test]
fn bug_2629_the_fallback_passes_stop_at_the_parse_ceiling() {
    let labels = vec![SPEC.to_string()];
    let (_tmp, root, _env) = fixture();
    append(&root, 4000, SPEC, &phase3(record("request-changes", -10)));
    append(&root, 4001, SPEC, &phase3(record("request-changes", -5)));
    let dir = ledger::ledger_dir(&root);
    // Sorted between the spec's ledgers: the ceiling is shared across them.
    std::fs::write(dir.join("PR-4000999.jsonl"), "x\n".repeat(1000)).unwrap();
    assert_eq!(
        super::labelled_changes(&root, &labels),
        Ok(vec![ChangeKey::pr(4000), ChangeKey::pr(4001)])
    );
    {
        let _lines = ParseBudgetGuard::set(100);
        work::PARSE_ATTEMPTS.with(|n| n.set(0));
        let why = super::labelled_changes(&root, &labels).unwrap_err();
        assert!(
            why.contains("PR-4000999.jsonl") && why.contains("parse budget"),
            "{why}"
        );
        assert_eq!(work::PARSE_ATTEMPTS.with(Cell::get), 100);
    }
    // The recovery shares the ceiling across its ledgers as well.
    std::fs::remove_file(dir.join("PR-4000999.jsonl")).unwrap();
    lose_binding_beside_corrupt_ledger(&root, SPEC, 4000);
    unavailable(&root);
    let _lines = ParseBudgetGuard::set(1);
    let why = refused_uncovered(&root);
    assert!(why.contains("parse budget"), "{why}");
}

// trace:BUG-2629 | ai:codex
#[test]
fn bug_2629_selected_ownership_failure_checks_other_spec_changes() {
    let (_tmp, root, _env) = fixture();
    append(&root, 41, SPEC, &record("request-changes", -100));
    let other = "BUG-17";
    append(&root, 41, other, &record("approved", -90));
    corrupt_bound_sibling(&root, other);
    let mut wait = phase3(record("waiting", -30));
    wait.insert("summary".into(), "newer evidence on PR-42".into());
    append(&root, 42, SPEC, &wait);
    let mut cache = super::DiscoveryCache::default();
    let result =
        crate::queue_cmd::work::pickup_rework_review(&mut cache, &root, SPEC, SPEC, Some("PR-41"));
    assert!(
        matches!(result, Err(ReworkReviewError::Waiting(ref why))
            if why.contains("newer evidence on PR-42")),
        "{result:?}"
    );
}

// trace:BUG-2629 | ai:codex
#[test]
fn bug_2629_selected_malformed_ledger_is_unknown_even_without_fingerprints() {
    let (_tmp, root, _env) = fixture();
    append(&root, 41, SPEC, &phase3(record("waiting", -100)));
    for invalid in [b"invalid\n".as_slice(), &[0xff, 0xfe, b'\n']] {
        std::fs::write(store::ledger_file(&root, ChangeKey::pr(41)), invalid).unwrap();
        let mut cache = super::DiscoveryCache::default();
        let result = crate::queue_cmd::work::pickup_rework_review(
            &mut cache,
            &root,
            SPEC,
            SPEC,
            Some("PR-41"),
        );
        assert!(
            matches!(result, Err(ReworkReviewError::Waiting(ref why))
            if why.contains("PR-41.jsonl")),
            "{result:?}"
        );
    }
}

// trace:BUG-2629 | ai:codex
#[test]
fn bug_2629_pickup_terminal_lookup_does_not_close_superseded_wait() {
    use crate::forge::ChangeState;
    use crate::queue_cmd::work::{pickup_rework_review, resolve_rework_review_with};
    use crate::rework_direction::ReviewResult;

    for new_change in [None, Some(41), Some(42)] {
        let (_tmp, root, _env) = fixture();
        append(&root, 41, SPEC, &record("waiting", -60));
        let index = ledger::ledger_dir(&root)
            .join("by-spec")
            .join(format!("{SPEC}.json"));
        let before_index = std::fs::read(&index).unwrap();
        let mut cache = super::DiscoveryCache::default();
        let mut observations = 0;
        let (result, pass) = measured(|| {
            resolve_rework_review_with(
                &mut cache,
                &root,
                SPEC,
                SPEC,
                None,
                false,
                |root, keys, reviews| {
                    crate::review_waiting::reconcile_waiting_with(root, keys, reviews, |change| {
                        assert_eq!(change, ChangeKey::pr(41));
                        observations += 1;
                        if let Some(pr) = new_change {
                            let mut newer = record("waiting", -10);
                            newer.insert("summary".into(), "new independent evidence".into());
                            append(root, pr, SPEC, &newer);
                        }
                        Some((ChangeState::Closed, HEAD.into()))
                    });
                },
            )
        });
        assert_eq!(observations, 1, "aliases share forge observations");
        assert_eq!(pass.opened, 1, "{pass:?}");
        assert_eq!(pass.binding_lists, 1, "{pass:?}");
        assert_eq!(pass.index_reads, 1, "{pass:?}");
        let terminal_records = store::read_file(&root, ChangeKey::pr(41))
            .into_iter()
            .filter(|event| {
                event
                    .verdict_record()
                    .is_some_and(|r| r.contains_key("terminal_wait_source"))
            })
            .count();
        let current = rv::read_recorded_verdict(&root, SPEC).unwrap();
        if new_change.is_some() {
            assert!(
                matches!(&result, Err(ReworkReviewError::Waiting(why)) if why.contains("changed during terminal lookup")),
                "{result:?}"
            );
            assert_eq!(terminal_records, 0);
            assert_eq!(current.summary.as_deref(), Some("new independent evidence"));
            assert!(!current.is_closed());
        } else {
            result.unwrap();
            assert_eq!(
                terminal_records, 2,
                "both the spec and change wait are closed"
            );
            assert!(current.is_closed());

            // Preparation and both pickup checks share the same discovery.
            let (pickups, reused) = measured(|| {
                [0, 1].map(|_| pickup_rework_review(&mut cache, &root, SPEC, SPEC, Some("PR-41")))
            });
            for pickup in pickups {
                assert!(
                    matches!(pickup, Ok(ReviewResult::Unavailable)),
                    "{pickup:?}"
                );
            }
            assert_eq!(reused.opened, 0, "{reused:?}");
            assert_eq!(reused.ledger_opens, 0, "{reused:?}");
            assert_eq!(reused.index_reads, 0, "{reused:?}");
            assert_eq!(std::fs::read(&index).unwrap(), before_index);
            assert_eq!(store::read_file(&root, ChangeKey::pr(41)).len(), 4);

            // A different writer changing the hint still invalidates reuse.
            std::fs::write(&index, b"{}").unwrap();
            let drift = pickup_rework_review(&mut cache, &root, SPEC, SPEC, Some("PR-41"));
            assert!(
                matches!(&drift, Err(ReworkReviewError::Waiting(why)) if why.contains("by-spec")),
                "{drift:?}"
            );
        }
    }
}

// trace:BUG-2629 | ai:codex
#[test]
fn bug_2629_same_scope_pickup_refuses_new_local_wait() {
    use crate::queue_cmd::work::pickup_rework_review;
    use crate::rework_direction::ReviewResult;

    for verdict in ["approved", "request-changes"] {
        let (_tmp, root, _env) = fixture();
        append(&root, 41, SPEC, &record(verdict, -60));
        let mut cache = super::DiscoveryCache::default();
        let first = pickup_rework_review(&mut cache, &root, SPEC, SPEC, Some("PR-41"));
        assert!(
            matches!(
                first,
                Ok(ReviewResult::ClearedByApproval | ReviewResult::Available(_))
            ),
            "{first:?}"
        );

        append(&root, 41, SPEC, &record("waiting", -10));
        let (second, reused) =
            measured(|| pickup_rework_review(&mut cache, &root, SPEC, SPEC, Some("PR-41")));
        assert!(
            matches!(&second, Err(ReworkReviewError::Waiting(why)) if why.contains("PR-41.jsonl")),
            "{second:?}"
        );
        assert_eq!(reused.opened, 0, "{reused:?}");
        assert_eq!(reused.ledger_opens, 0, "{reused:?}");
        assert_eq!(reused.index_reads, 0, "{reused:?}");
    }
}

// trace:BUG-2629 | ai:codex
#[test]
fn bug_2629_pickup_revalidates_local_evidence_without_terminal_annotations() {
    use crate::forge::ChangeState;
    use crate::queue_cmd::work::resolve_rework_review_with;
    use crate::rework_direction::ReviewResult;

    for superseded in [false, true] {
        let (_tmp, root, _env) = fixture();
        append(&root, 41, SPEC, &record("waiting", -60));
        append(&root, 42, SPEC, &record("approved", -30));
        let mut cache = super::DiscoveryCache::default();
        let mut observed = Vec::new();
        let result = resolve_rework_review_with(
            &mut cache,
            &root,
            SPEC,
            SPEC,
            Some("PR-42"),
            false,
            |root, keys, reviews| {
                crate::review_waiting::reconcile_waiting_with(root, keys, reviews, |change| {
                    observed.push(change);
                    assert_eq!(change, ChangeKey::pr(41));
                    if superseded {
                        append(root, 42, SPEC, &record("waiting", -10));
                    }
                    Some((ChangeState::Open, HEAD.into()))
                });
                assert!(reviews.terminal.is_empty());
            },
        );
        assert_eq!(observed, [ChangeKey::pr(41)]);
        if superseded {
            assert!(
                matches!(&result, Err(ReworkReviewError::Waiting(why))
                    if why.contains("changed during terminal lookup")),
                "{result:?}"
            );
        } else {
            assert!(
                matches!(result, Ok(ReviewResult::ClearedByApproval)),
                "{result:?}"
            );
        }
    }
}

// trace:BUG-2629 | ai:codex
#[test]
fn bug_2629_pickup_refuses_unsafe_index_for_ownership_free_records() {
    use crate::gate::ledger::checked::MAX_INDEX_BYTES;
    for kind in [
        "malformed",
        "invalid key",
        "oversized",
        "symlink",
        "hard link",
    ] {
        let (_tmp, root, _env) = fixture();
        append(&root, 41, SPEC, &phase3(record("approved", -60)));
        let hint = ledger::ledger_dir(&root)
            .join("by-spec")
            .join(format!("{SPEC}.json"));
        // Even with an unambiguous approval in the ledger, a sound hint can
        // discover a legacy wait with no physical change ledger or binding.
        assert!(!refused_waiting(&root));
        legacy_file(
            &root,
            "PR-42",
            serde_json::json!({
                "verdict": "waiting",
                "reviewed_sha": HEAD,
                "summary": SUMMARY,
            }),
        );
        project::index_spec(&root, SPEC, ChangeKey::pr(42)).unwrap();
        assert!(refused_waiting(&root));
        match kind {
            "malformed" => std::fs::write(&hint, b"{malformed").unwrap(),
            "invalid key" => {
                std::fs::write(&hint, br#"{"changes":["PR-41","not a key"]}"#).unwrap()
            }
            "oversized" => std::fs::OpenOptions::new()
                .write(true)
                .open(&hint)
                .unwrap()
                .set_len(MAX_INDEX_BYTES + 1)
                .unwrap(),
            "symlink" | "hard link" => {
                let target = root.join("other-index.json");
                std::fs::rename(&hint, &target).unwrap();
                if kind == "symlink" {
                    std::os::unix::fs::symlink(target, &hint).unwrap();
                } else {
                    std::fs::hard_link(target, &hint).unwrap();
                }
            }
            _ => unreachable!(),
        }
        let ((first, second), pass) = measured(|| {
            let mut cache = super::DiscoveryCache::default();
            let mut resolve = || {
                crate::queue_cmd::work::resolve_rework_review_cached(
                    &mut cache, &root, SPEC, SPEC, None,
                )
            };
            (resolve(), resolve())
        });
        for result in [first, second] {
            assert!(
                matches!(&result, Err(ReworkReviewError::Waiting(why)) if why.contains("BUG-2629.json")),
                "{kind}: {result:?}"
            );
        }
        assert_eq!(pass.opened, 1, "{kind}: {pass:?}");
        assert_eq!(
            pass.index_reads,
            usize::from(matches!(kind, "malformed" | "invalid key")),
            "{kind}: {pass:?}"
        );
    }
}

// trace:BUG-2629 | ai:codex
#[test]
fn bug_2629_queue_hint_spends_the_shared_checked_read_budget() {
    use crate::gate::ledger::checked::Budget;
    let (_tmp, root, _env) = fixture();
    let labels = [SPEC.to_string()];
    let key = ChangeKey::pr(41);
    append(&root, 41, SPEC, &phase3(record("approved", -60)));
    let ledger_bytes = std::fs::metadata(store::ledger_file(&root, key))
        .unwrap()
        .len();
    let index = ledger::ledger_dir(&root)
        .join("by-spec")
        .join(format!("{SPEC}.json"));
    let index_bytes = std::fs::metadata(index).unwrap().len();
    for extra in [0, 1] {
        let mut cache = super::DiscoveryCache {
            budget: Budget::with_limits(ledger_bytes + index_bytes - extra, 10),
            ..Default::default()
        };
        let bindings = super::discover_with_cache(&mut cache, &root, &labels, None, false);
        if extra == 0 {
            assert!(bindings.binds("PR-41"));
            assert_eq!(bindings.failure(), None);
            assert_eq!(bindings.refusal(), None);
        } else {
            assert!(bindings
                .failure()
                .is_some_and(|why| why.contains("read budget")));
        }
    }
}

// trace:BUG-2629 | ai:codex
#[test]
fn bug_2629_uncertified_labels_reuse_the_checked_hint() {
    let (_tmp, root, _env) = fixture();
    append(&root, 41, SPEC, &phase3(record("approved", -60)));
    std::fs::remove_file(ledger::bindings::marker_file(&root)).unwrap();
    let mut cache = super::DiscoveryCache::default();
    let (_, pass) = measured(|| {
        for labels in [
            vec![SPEC.to_string()],
            vec![SPEC.to_string(), "BUG-17".to_string()],
        ] {
            let inventory = cache.inventory(&root, &labels);
            assert_eq!(inventory.refusal(), None);
            assert_eq!(inventory.hints[SPEC], [ChangeKey::pr(41)]);
        }
    });
    assert_eq!(pass.opened, 1, "{pass:?}");
    assert_eq!(pass.index_reads, 1, "{pass:?}");
}
