//! STORY-2324 PR-A: the pure `effective_gate_state` (test plan section 8:
//! the table tests and the re-spelling / late-approval properties). No file,
//! git, clock or environment input: every case is an event vector.
// trace:STORY-2324 | ai:claude

use super::*;
use crate::gate::canon::Snapshot;
use crate::gate::merge_hold_core::{typed_hold, HoldReasonKind};
use aida_core::ExecutionMode;

const NODE: &str = "b6b1278890774ef993d2ebc327862470";
const HEAD_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEAD_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn req(
    native: &str,
    friendly: Option<&str>,
    mode: Option<ExecutionMode>,
) -> aida_core::Requirement {
    let mut r = aida_core::Requirement::new(format!("fixture {native}"), "fixture".into());
    r.spec_id = Some(native.to_string());
    r.agreed_id = friendly.map(str::to_string);
    r.execution_mode = mode;
    r
}

fn supersede(r: &mut aida_core::Requirement, carrier: &aida_core::Requirement) {
    r.relationships.push(aida_core::Relationship {
        rel_type: aida_core::RelationshipType::SupersededBy,
        target_id: carrier.id,
        created_at: None,
        created_by: None,
    });
}

fn snapshot(reqs: &[aida_core::Requirement]) -> Snapshot {
    let aliases = format!("[[binding]]\nlabel = \"HOME\"\nnode = \"{NODE}\"\n");
    let retired = format!(
        "[[retired]]\nold = \"BUG-1-020\"\nnew = \"x\"\nuuid = \"{}\"\n",
        reqs[0].id
    );
    Snapshot::from_parts("store", reqs, Some(&aliases), Some(&retired)).unwrap()
}

/// The PR3185/PR3194 requirement: native, friendly, node and retired labels.
fn subject() -> aida_core::Requirement {
    req(
        &format!("BUG-{NODE}-020"),
        Some("BUG-2403"),
        Some(ExecutionMode::Drain),
    )
}

fn at(n: u64) -> String {
    format!("2026-11-02T10:{:02}:00+00:00", n % 60)
}

fn record(word: &str, sha: &str, recorded_at: &str, findings: &[&str]) -> JsonObj {
    let mut r = JsonObj::new();
    r.insert("verdict".into(), word.into());
    r.insert("reviewed_sha".into(), sha.into());
    r.insert("recorded_at".into(), recorded_at.into());
    r.insert("recorded_by".into(), "reviewer seat".into());
    r.insert(
        crate::gate::review_verdict::ATTESTATION_KEY.into(),
        serde_json::json!({"seat": "reviewer"}),
    );
    if !findings.is_empty() {
        r.insert("findings".into(), serde_json::json!(findings));
    }
    r
}

fn event(seq: u64, spec: Option<&str>, body: EventBody) -> GateEvent {
    let mut e = GateEvent::new(ChangeKey::pr(3185), body);
    e.seq = seq;
    // Appended in the hour after every fixture review was recorded, so a
    // record's order time is its `recorded_at` (as for a real append).
    e.at = format!("2026-11-02T11:{:02}:00+00:00", seq % 60);
    e.spec = spec.map(str::to_string);
    e
}

fn verdict_ev(seq: u64, spec: Option<&str>, word: &str, sha: &str) -> GateEvent {
    verdict_ev_at(seq, spec, word, sha, &at(seq), &[])
}

fn verdict_ev_at(
    seq: u64,
    spec: Option<&str>,
    word: &str,
    sha: &str,
    recorded_at: &str,
    findings: &[&str],
) -> GateEvent {
    let mut e = event(
        seq,
        spec,
        EventBody::Verdict {
            record: record(word, sha, recorded_at, findings),
        },
    );
    e.head = Some(sha.to_string());
    e
}

fn view(change_events: Vec<GateEvent>) -> LedgerView {
    LedgerView {
        change_events,
        unbound: Default::default(),
    }
}

fn drain() -> GatePolicy {
    GatePolicy::default()
}

fn state(
    reqs: &[aida_core::Requirement],
    labels: &[&str],
    v: &LedgerView,
    head: &str,
) -> GateState {
    let set = snapshot(reqs).resolve(labels);
    effective_gate_state(head, &set, v, &drain())
}

fn refusals(s: &GateState) -> Vec<Refusal> {
    match &s.decision {
        Decision::Refused { refusals } => refusals.clone(),
        Decision::Mergeable { .. } => Vec::new(),
    }
}

#[test]
fn an_approval_at_head_on_a_drain_member_is_mergeable_and_pinned() {
    let r = subject();
    let v = view(vec![verdict_ev(1, Some("BUG-2403"), "approved", HEAD_A)]);
    let s = state(&[r], &["BUG-2403"], &v, HEAD_A);
    assert_eq!(
        s.decision,
        Decision::Mergeable {
            pin: HEAD_A.to_string()
        }
    );
    assert_eq!(s.mode, Mode::Drain);
    assert!(matches!(s.verdict, Verdict::Approved { seq: 1, .. }));
}

/// PR3185: #19 approved under the friendly label; #20 refused and #21
/// approved under the canonical label. One partition, seq order.
#[test]
fn pr3185_friendly_approval_does_not_answer_over_later_canonical_records() {
    let r = subject();
    let native = format!("BUG-{}-020", NODE.to_ascii_uppercase());
    let refused = vec![
        verdict_ev(19, Some("BUG-2403"), "approved", HEAD_A),
        verdict_ev(20, Some(&native), "changes_requested", HEAD_A),
    ];
    let s = state(&[r.clone()], &["BUG-2403"], &view(refused.clone()), HEAD_A);
    assert!(matches!(
        s.verdict,
        Verdict::ChangesRequested { seq: 20, .. }
    ));
    assert!(refusals(&s).contains(&Refusal::ChangesRequested));

    let mut fixed = refused;
    fixed.push(verdict_ev(21, Some(&native), "approved", HEAD_B));
    let s = state(&[r], &["BUG-2403"], &view(fixed), HEAD_B);
    assert!(
        matches!(s.verdict, Verdict::Approved { seq: 21, .. }),
        "{s:?}"
    );
}

/// PR3185 #22/#23, PR3194 #14-#16: a receipt-less phase-3 overlay answers
/// with the receipt-bearing record it duplicates, whatever label each used.
#[test]
fn a_phase3_overlay_answers_with_its_origin_across_spellings() {
    let r = subject();
    let mut origin_rec = record("approved", HEAD_A, &at(1), &[]);
    origin_rec.insert("release_grant".into(), serde_json::json!({"grant": "g"}));
    let mut overlay_rec = origin_rec.clone();
    overlay_rec.remove("release_grant");
    overlay_rec.insert(
        "mode".into(),
        crate::gate::ledger::project::PHASE3_MODE.into(),
    );
    let mut origin = event(
        22,
        Some("BUG-2403"),
        EventBody::Verdict { record: origin_rec },
    );
    origin.head = Some(HEAD_A.into());
    let mut overlay = event(
        23,
        Some(&format!("BUG-{NODE}-020")),
        EventBody::Verdict {
            record: overlay_rec,
        },
    );
    overlay.head = Some(HEAD_A.into());
    let s = state(&[r], &["BUG-2403"], &view(vec![origin, overlay]), HEAD_A);
    assert!(
        matches!(s.verdict, Verdict::Approved { seq: 22, .. }),
        "{s:?}"
    );
}

/// PR3194 #7: obsolete findings do not replay over a later approval; a
/// fresh refusal after it does answer, with its own findings.
#[test]
fn pr3194_obsolete_findings_vs_a_fresh_refusal() {
    let r = subject();
    let base = vec![
        verdict_ev_at(
            5,
            Some("BUG-2403"),
            "changes_requested",
            HEAD_A,
            &at(5),
            &["old"],
        ),
        verdict_ev(7, Some(&format!("BUG-{NODE}-020")), "approved", HEAD_B),
    ];
    let s = state(&[r.clone()], &["BUG-2403"], &view(base.clone()), HEAD_B);
    assert!(
        matches!(s.verdict, Verdict::Approved { seq: 7, .. }),
        "{s:?}"
    );

    let mut fresh = base;
    fresh.push(verdict_ev_at(
        8,
        Some("BUG-HOME-020"),
        "changes_requested",
        HEAD_B,
        &at(8),
        &["new"],
    ));
    let s = state(&[r], &["BUG-2403"], &view(fresh), HEAD_B);
    assert_eq!(
        s.verdict,
        Verdict::ChangesRequested {
            seq: 8,
            findings: vec!["new".into()]
        }
    );
}

#[test]
fn a_late_collected_older_approval_does_not_answer_a_newer_refusal() {
    let r = subject();
    let v = view(vec![
        verdict_ev(10, Some("BUG-2403"), "changes_requested", HEAD_A),
        // Appended later, but reviewed (recorded) before the refusal.
        verdict_ev_at(11, Some("BUG-2403"), "approved", HEAD_A, &at(1), &[]),
    ]);
    let s = state(&[r], &["BUG-2403"], &v, HEAD_A);
    assert!(matches!(
        s.verdict,
        Verdict::ChangesRequested { seq: 10, .. }
    ));
}

#[test]
fn superseded_inheritance_needs_the_carrier_in_the_same_set() {
    let carrier = req("STORY-50", None, Some(ExecutionMode::Drain));
    let mut bundled = req("TASK-51", None, None);
    supersede(&mut bundled, &carrier);
    let reqs = [subject(), carrier.clone(), bundled.clone()];
    let v = view(vec![]);

    let s = state(&reqs, &["STORY-50", "TASK-51"], &v, HEAD_A);
    assert_eq!(s.mode, Mode::Drain);

    let s = state(&reqs, &["TASK-51"], &v, HEAD_A);
    assert_eq!(
        s.mode,
        Mode::Supervised {
            members: vec![("TASK-51".into(), "unset (supervised)".into())]
        }
    );
}

#[test]
fn a_superseded_cycle_is_supervised() {
    let mut a = req("TASK-60", None, None);
    let mut b = req("TASK-61", None, None);
    supersede(&mut a, &b);
    supersede(&mut b, &a);
    let s = state(
        &[subject(), a, b],
        &["TASK-60", "TASK-61"],
        &view(vec![]),
        HEAD_A,
    );
    assert!(matches!(s.mode, Mode::Supervised { ref members } if members.len() == 2));
}

#[test]
fn an_unset_mode_is_supervised_except_for_an_interactive_human() {
    let r = req("STORY-70", None, None);
    let v = view(vec![verdict_ev(1, Some("STORY-70"), "approved", HEAD_A)]);
    let set = snapshot(&[subject(), r]).resolve(&["STORY-70"]);
    let s = effective_gate_state(HEAD_A, &set, &v, &drain());
    assert!(matches!(refusals(&s)[..], [Refusal::Supervised { .. }]));
    let tty = GatePolicy {
        interactive_tty: true,
    };
    let s = effective_gate_state(HEAD_A, &set, &v, &tty);
    assert!(matches!(s.decision, Decision::Mergeable { .. }));
}

#[test]
fn an_unresolved_label_refuses() {
    let v = view(vec![verdict_ev(1, Some("BUG-2403"), "approved", HEAD_A)]);
    let s = state(&[subject()], &["BUG-2403", "TASK-404"], &v, HEAD_A);
    assert_eq!(
        refusals(&s),
        vec![Refusal::Unresolved {
            labels: vec!["TASK-404".into()]
        }]
    );
}

#[test]
fn a_stale_head_carry_over_proof_and_carry_over_refusal() {
    let r = subject();
    let approval = verdict_ev(1, Some("BUG-2403"), "approved", HEAD_A);
    let s = state(
        &[r.clone()],
        &["BUG-2403"],
        &view(vec![approval.clone()]),
        HEAD_B,
    );
    assert!(matches!(s.verdict, Verdict::Stale { .. }));
    assert!(refusals(&s).contains(&Refusal::StaleApproval));

    let proof = crate::gate::review_carryover::CarryOver {
        pr: 3185,
        approved_sha: HEAD_A.into(),
        new_head: HEAD_B.into(),
        base_ref: "origin/main".into(),
        base_tip: "c".repeat(40),
        approved_range: String::new(),
        head_range: String::new(),
        approved_patch_id: "p".into(),
        head_patch_id: "p".into(),
        merge_commits: vec![HEAD_B.into()],
        recorded_at: at(2),
    };
    let carried = event(
        2,
        None,
        EventBody::CarryOverProof {
            proof,
            proof_version: 1,
        },
    );
    let s = state(
        &[r.clone()],
        &["BUG-2403"],
        &view(vec![approval.clone(), carried]),
        HEAD_B,
    );
    assert!(matches!(
        s.verdict,
        Verdict::Approved {
            carried_over: true,
            ..
        }
    ));

    let refused = event(
        2,
        None,
        EventBody::CarryOverRefused {
            approved_sha: HEAD_A.into(),
            new_head: HEAD_B.into(),
            refusal: crate::gate::review_carryover::CarryOverRefusal::NotDescendant,
            reason: "rewritten history".into(),
            proof_version: 1,
        },
    );
    let s = state(&[r], &["BUG-2403"], &view(vec![approval, refused]), HEAD_B);
    assert!(
        matches!(&s.verdict, Verdict::Stale { reason, .. } if reason.contains("rewritten history"))
    );
}

#[test]
fn hold_open_released_authorized_withdrawn_and_label_sync() {
    let r = subject();
    let approval = verdict_ev(1, Some("BUG-2403"), "approved", HEAD_A);
    let hold = event(
        2,
        None,
        EventBody::Hold {
            hold: typed_hold(3185, HoldReasonKind::Rework, "rework", Some(HEAD_A.into())),
            rearmed: false,
        },
    );
    let mut evs = vec![approval.clone(), hold.clone()];
    let s = state(&[r.clone()], &["BUG-2403"], &view(evs.clone()), HEAD_A);
    assert!(matches!(
        s.hold,
        Hold::Open {
            authorized: false,
            ..
        }
    ));
    assert_eq!(refusals(&s), vec![Refusal::HoldOpen]);

    evs.push(event(
        3,
        None,
        EventBody::ReleaseAuthorized {
            clearance: JsonObj::new(),
        },
    ));
    evs.push(event(
        4,
        None,
        EventBody::LabelSync {
            label_state: "synced".into(),
        },
    ));
    let s = state(&[r.clone()], &["BUG-2403"], &view(evs.clone()), HEAD_A);
    assert!(matches!(
        &s.hold,
        Hold::Open { authorized: true, label_state: Some(l), .. } if l == "synced"
    ));

    let mut withdrawn = evs.clone();
    withdrawn.push(event(
        5,
        None,
        EventBody::ReleaseWithdrawn {
            reason: "head moved".into(),
        },
    ));
    let s = state(&[r.clone()], &["BUG-2403"], &view(withdrawn), HEAD_A);
    assert!(matches!(
        s.hold,
        Hold::Open {
            authorized: false,
            ..
        }
    ));

    evs.push(event(
        5,
        None,
        EventBody::HoldRelease {
            clearance: JsonObj::new(),
        },
    ));
    let s = state(&[r], &["BUG-2403"], &view(evs), HEAD_A);
    assert_eq!(s.hold, Hold::Released);
    assert!(matches!(s.decision, Decision::Mergeable { .. }));
}

#[test]
fn a_draft_only_member_refuses() {
    let mut r = req("STORY-80", None, Some(ExecutionMode::Drain));
    r.tags.insert(crate::gate::ship::DRAFT_ONLY_TAG.to_string());
    let v = view(vec![verdict_ev(1, Some("STORY-80"), "approved", HEAD_A)]);
    let s = state(&[subject(), r], &["STORY-80"], &v, HEAD_A);
    assert_eq!(
        refusals(&s),
        vec![Refusal::DraftOnly {
            members: vec!["STORY-80".into()]
        }]
    );
}

#[test]
fn no_verdict_refuses_and_an_unrecognized_word_never_approves() {
    let s = state(&[subject()], &["BUG-2403"], &view(vec![]), HEAD_A);
    assert_eq!(refusals(&s), vec![Refusal::NoApproval]);
    let v = view(vec![verdict_ev(1, Some("BUG-2403"), "lgtm-ish", HEAD_A)]);
    let s = state(&[subject()], &["BUG-2403"], &v, HEAD_A);
    assert_eq!(refusals(&s), vec![Refusal::UnrecognizedVerdict]);
}

#[test]
fn unbound_records_count_once() {
    let r = subject();
    let unbound = verdict_ev(1, Some("BUG-2403"), "approved", HEAD_A);
    let mut v = view(vec![unbound.clone()]);
    v.unbound
        .insert(crate::gate::canon::SpecKey(r.id), vec![unbound]);
    let s = state(&[r.clone()], &["BUG-2403"], &v, HEAD_A);
    assert!(matches!(s.verdict, Verdict::Approved { seq: 1, .. }));
    assert!(matches!(s.decision, Decision::Mergeable { .. }));

    // An unbound refusal recorded after the change's approval still blocks.
    let mut lagged = view(vec![verdict_ev(3, Some("BUG-2403"), "approved", HEAD_A)]);
    lagged.unbound.insert(
        crate::gate::canon::SpecKey(r.id),
        vec![verdict_ev(9, Some("BUG-HOME-020"), "rejected", HEAD_A)],
    );
    let s = state(&[r], &["BUG-2403"], &lagged, HEAD_A);
    assert!(matches!(
        s.verdict,
        Verdict::ChangesRequested { seq: 9, .. }
    ));
}

#[test]
fn an_open_refusal_under_a_label_naming_no_member_blocks() {
    // Today's change-keyed read (project.rs verdict_events for PR-N) answers
    // with the change's newest verdict whatever spec it names, so an open
    // refusal under an unknown, foreign or ambiguous label blocks. Dropping
    // it into foreign_labels would be a loosening.
    for foreign in ["TASK-999", "bug-home-404"] {
        let v = view(vec![
            verdict_ev(1, Some("BUG-2403"), "approved", HEAD_A),
            verdict_ev(2, Some(foreign), "changes_requested", HEAD_A),
        ]);
        let s = state(&[subject()], &["BUG-2403"], &v, HEAD_A);
        assert_eq!(s.foreign_labels, vec![foreign.to_ascii_uppercase()]);
        assert!(matches!(
            s.foreign_verdict,
            Verdict::ChangesRequested { seq: 2, .. }
        ));
        assert!(matches!(
            s.verdict,
            Verdict::ChangesRequested { seq: 2, .. }
        ));
        assert!(matches!(s.decision, Decision::Refused { .. }));
        assert!(refusals(&s).contains(&Refusal::ChangesRequested));
    }
    // An unrecognized word under a foreign label never approves either.
    let v = view(vec![
        verdict_ev(1, Some("BUG-2403"), "approved", HEAD_A),
        verdict_ev(2, Some("TASK-999"), "lgtm-ish", HEAD_A),
    ]);
    let s = state(&[subject()], &["BUG-2403"], &v, HEAD_A);
    assert!(refusals(&s).contains(&Refusal::UnrecognizedVerdict));
    // A foreign approval is not a member's approval and grants nothing.
    let v = view(vec![verdict_ev(1, Some("TASK-999"), "approved", HEAD_A)]);
    let s = state(&[subject()], &["BUG-2403"], &v, HEAD_A);
    assert_eq!(s.foreign_verdict, Verdict::Absent);
    assert_eq!(refusals(&s), vec![Refusal::NoApproval]);
}

#[test]
fn an_open_refusal_under_an_ambiguous_label_blocks() {
    let a = req("TASK-5", None, Some(ExecutionMode::Drain));
    let b = req("TASK-6", None, Some(ExecutionMode::Drain));
    let retired = format!(
        "[[retired]]\nold = \"TASK-5\"\nnew = \"x\"\nuuid = \"{}\"\n",
        b.id
    );
    let snap = Snapshot::from_parts("h", &[a.clone(), b.clone()], None, Some(&retired)).unwrap();
    let set = snap.resolve(&["TASK-6"]);
    let v = view(vec![
        verdict_ev(1, Some("TASK-6"), "approved", HEAD_A),
        verdict_ev(2, Some("TASK-5"), "rejected", HEAD_A),
    ]);
    let s = effective_gate_state(HEAD_A, &set, &v, &drain());
    assert_eq!(s.foreign_labels, vec!["TASK-5".to_string()]);
    assert!(matches!(s.decision, Decision::Refused { .. }));
    assert!(refusals(&s).contains(&Refusal::ChangesRequested));
}

#[test]
fn a_pr_level_refusal_blocks_a_member_approval() {
    let v = view(vec![
        verdict_ev(1, Some("BUG-2403"), "approved", HEAD_A),
        verdict_ev(2, None, "changes_requested", HEAD_A),
    ]);
    let s = state(&[subject()], &["BUG-2403"], &v, HEAD_A);
    assert!(matches!(s.change_verdict, Verdict::ChangesRequested { .. }));
    assert!(refusals(&s).contains(&Refusal::ChangesRequested));
}

// ── Properties ─────────────────────────────────────────────────────────────

use proptest::prelude::*;

/// Every spelling the fixture requirement answers to.
fn spellings() -> Vec<String> {
    vec![
        format!("BUG-{NODE}-020"),
        format!("BUG-{}-020", NODE.to_ascii_uppercase()),
        "BUG-2403".into(),
        "bug-2403".into(),
        "BUG-HOME-020".into(),
        "Bug-Home-020".into(),
        "BUG-1-020".into(),
    ]
}

const WORDS: [&str; 3] = ["approved", "changes_requested", "rejected"];

fn arb_events() -> impl Strategy<Value = Vec<(usize, usize, bool, u64)>> {
    // (word, spelling, at head A?, recorded-at offset)
    proptest::collection::vec((0..3usize, 0..7usize, any::<bool>(), 0..50u64), 0..8)
}

fn build(spec: &[(usize, usize, bool, u64)], spelled: bool) -> Vec<GateEvent> {
    let labels = spellings();
    spec.iter()
        .enumerate()
        .map(|(i, (w, l, a, t))| {
            let label = if spelled { &labels[*l] } else { &labels[0] };
            verdict_ev_at(
                i as u64 + 1,
                Some(label),
                WORDS[*w],
                if *a { HEAD_A } else { HEAD_B },
                &at(*t),
                &[],
            )
        })
        .collect()
}

proptest! {
    /// Re-spelling a member's labels (friendly, native, node, retired, case)
    /// never changes the gate state.
    #[test]
    fn respelling_labels_never_changes_the_state(
        spec in arb_events(),
        subject_label in 0..7usize,
    ) {
        let r = subject();
        let labels = spellings();
        let canonical = state(&[r.clone()], &[labels[0].as_str()], &view(build(&spec, false)), HEAD_A);
        let respelled = state(
            &[r],
            &[labels[subject_label].as_str()],
            &view(build(&spec, true)),
            HEAD_A,
        );
        prop_assert_eq!(canonical, respelled);
    }

    /// Appending an approval recorded before an existing open refusal never
    /// yields a mergeable state.
    #[test]
    fn an_older_approval_never_unblocks_a_refusal(
        spec in arb_events(),
        refusal_word in 1..3usize,
        older_by in 1..30u64,
        label in 0..7usize,
    ) {
        let r = subject();
        let labels = spellings();
        let mut evs = build(&spec, true);
        let n = evs.len() as u64;
        evs.push(verdict_ev_at(n + 1, Some(&labels[label]), WORDS[refusal_word], HEAD_A, &at(55), &[]));
        evs.push(verdict_ev_at(n + 2, Some(&labels[0]), "approved", HEAD_A, &at(55 - older_by), &[]));
        let s = state(&[r], &["BUG-2403"], &view(evs), HEAD_A);
        prop_assert!(!matches!(s.decision, Decision::Mergeable { .. }), "{:?}", s);
    }
}
