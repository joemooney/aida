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

    // The spec in hand lost its binding too: its own full reads refuse, a
    // bounded number of times, and the wait still refuses.
    std::fs::remove_file(bindings.join(SPEC).join("PR-77")).unwrap();
    let (new, took) = batch(&root);
    eprintln!("bug_2629 own discovery refused: batch={new} in {took:?}");
    // One refused full read per resolution; `batch` resolves twice.
    assert!(
        new <= 3 * corpus,
        "a refused discovery read {new} of a {corpus} byte corpus"
    );
}

/// Finding 1 of the round-4 review: many fingerprinted reviews of the spec
/// in hand, one lost binding, a malformed ledger and no open wait, so the
/// recovery visits every one of its changes. The refused full read happens
/// once for the resolution, not once per change: each change's projection
/// read (what the recovery used to ask per key) repeats it.
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
    assert!(
        new < 2 * corpus,
        "the resolution read {new} of a {corpus} byte corpus"
    );
    reset_ledger_reads();
    refuse_waiting_review(&root, SPEC, SPEC, None).unwrap();
    assert!(ledger_reads() < 2 * corpus);
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
