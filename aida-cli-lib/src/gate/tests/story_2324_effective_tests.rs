//! STORY-2324 PR-A: the pure `effective_gate_state` (test plan section 8:
//! the table tests and the re-spelling / late-approval properties). The pure
//! cases use event vectors; the rebound-label fixture also checks load_view.
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
            proof_version: crate::gate::review_carryover::PROOF_VERSION,
            carry_eligible: true,
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
            proof_version: crate::gate::review_carryover::PROOF_VERSION,
            carry_eligible: true,
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

/// Advisor evidence 2026-11-03 15:35 (train #3321, PR #3306): a round-1
/// RequestChanges recorded at a superseded head under one label of a member,
/// then an approval at the current head keyed by the change itself. Once
/// PR-C reads by canonical key plus head, the refusal at a head the change
/// no longer has must not count. Production readers remain unchanged until
/// the PR-C cutover; this is the pure core's regression.
// trace:STORY-2324 | ai:claude
#[test]
fn a_refusal_at_a_superseded_head_under_another_label_does_not_block_the_current_approval() {
    let r = subject();
    let superseded = "a2b504db00000000000000000000000000000000";
    let current = "3d1d467300000000000000000000000000000000";
    let mut stale_refusal = verdict_ev(5, Some("BUG-2403"), "changes_requested", superseded);
    stale_refusal.spec_key = Some(SpecKey(r.id));
    let approval = verdict_ev(9, None, "approved", current);
    let s = state(
        &[r],
        &["BUG-2403"],
        &view(vec![stale_refusal, approval]),
        current,
    );
    assert!(
        !refusals(&s).contains(&Refusal::ChangesRequested),
        "a refusal at a superseded head still blocks: {s:?}"
    );
    assert!(
        matches!(s.verdict, Verdict::Approved { seq: 9, .. }),
        "{s:?}"
    );
}

// trace:STORY-2324 | ai:codex
#[test]
fn canonical_stamp_overrides_an_obsolete_or_missing_audit_label() {
    let r = subject();
    for label in [Some("TASK-404"), None] {
        let mut approval = verdict_ev(9, label, "approved", HEAD_A);
        approval.spec_key = Some(SpecKey(r.id));
        let s = state(&[r.clone()], &["BUG-2403"], &view(vec![approval]), HEAD_A);
        assert!(
            matches!(s.members[0].verdict, Verdict::Approved { seq: 9, .. }),
            "{s:?}"
        );
        assert!(s.foreign_labels.is_empty());
    }
}

// trace:STORY-2324 | ai:codex
#[test]
fn foreign_stamp_cannot_borrow_a_members_audit_label() {
    let r = subject();
    let foreign = SpecKey(uuid::Uuid::new_v4());
    let mut approval = verdict_ev(9, Some("BUG-2403"), "approved", HEAD_A);
    approval.spec_key = Some(foreign);
    let s = state(&[r.clone()], &["BUG-2403"], &view(vec![approval]), HEAD_A);
    assert_eq!(s.members[0].verdict, Verdict::Absent);
    assert!(refusals(&s).contains(&Refusal::NoApproval));
    let mut refusal = verdict_ev(10, None, "request-changes", HEAD_A);
    refusal.spec_key = Some(foreign);
    let s = state(&[r], &["BUG-2403"], &view(vec![refusal]), HEAD_A);
    assert!(refusals(&s).contains(&Refusal::ChangesRequested));
}

// trace:STORY-2324 | ai:codex
#[test]
fn a_current_approval_cannot_discard_current_unknown_or_late_refusals() {
    let r = subject();
    for (sha, refusal_at, approval_at) in [
        (HEAD_A, 5, 9),    // same head: the refusal still answers
        ("", 5, 9),        // unknown head: never prove supersession
        ("ccccccc", 5, 9), // abbreviated head: no proof of a different full commit
        (HEAD_B, 10, 9),   // late collection: the refusal is actually newer
    ] {
        let refusal = verdict_ev_at(
            5,
            Some("BUG-2403"),
            "request-changes",
            sha,
            &at(refusal_at),
            &[],
        );
        let approval = verdict_ev_at(9, None, "approved", HEAD_A, &at(approval_at), &[]);
        let s = state(
            &[r.clone()],
            &["BUG-2403"],
            &view(vec![refusal, approval]),
            HEAD_A,
        );
        assert!(refusals(&s).contains(&Refusal::ChangesRequested), "{s:?}");
    }
}

// trace:STORY-2324 | ai:codex
#[test]
fn an_unrelated_members_approval_cannot_answer_a_stale_refusal() {
    let a = subject();
    let b = req("TASK-10", None, Some(ExecutionMode::Drain));
    let v = view(vec![
        verdict_ev(5, Some("BUG-2403"), "request-changes", HEAD_B),
        verdict_ev(9, Some("TASK-10"), "approved", HEAD_A),
    ]);
    let s = state(&[a, b], &["BUG-2403", "TASK-10"], &v, HEAD_A);
    assert!(refusals(&s).contains(&Refusal::ChangesRequested), "{s:?}");
}

// trace:STORY-2324 | ai:codex
#[test]
fn a_rebound_labels_unbound_events_do_not_belong_to_the_new_owner() {
    let r = subject();
    for word in ["approved", "request-changes"] {
        let mut event = verdict_ev(9, Some("BUG-2403"), word, HEAD_A);
        event.spec_key = Some(SpecKey(uuid::Uuid::new_v4()));
        let mut v = view(vec![verdict_ev(10, None, "approved", HEAD_A)]);
        v.unbound.insert(SpecKey(r.id), vec![event]);
        let s = state(&[r.clone()], &["BUG-2403"], &v, HEAD_A);
        assert_eq!(s.members[0].verdict, Verdict::Absent, "{s:?}");
        assert!(matches!(s.decision, Decision::Mergeable { .. }), "{s:?}");
    }
}

// trace:STORY-2324 | ai:codex
#[test]
fn load_view_rebound_label_keeps_old_owner_events_out_of_the_member_sequence() {
    let root = tempfile::tempdir().unwrap();
    let r = subject();
    let set = snapshot(&[r.clone()]).resolve(&["BUG-2403"]);
    let dir = root.path().join(".aida/gate-ledger");
    std::fs::create_dir_all(dir.join("by-spec")).unwrap();
    let mut old = verdict_ev(1, Some("BUG-2403"), "request-changes", HEAD_A);
    old.change = "BUG-2403".into();
    old.spec_key = Some(SpecKey(uuid::Uuid::new_v4()));
    let mut own = verdict_ev(2, Some("BUG-2403"), "approved", HEAD_A);
    own.change = "BUG-2403".into();
    own.spec_key = Some(SpecKey(r.id));
    let line = |e: &GateEvent| format!("{}\n", serde_json::to_string(e).unwrap());
    std::fs::write(dir.join("by-spec/BUG-2403.jsonl"), line(&old) + &line(&own)).unwrap();
    let v = load_view(root.path(), ChangeKey::pr(3185), &set).unwrap();
    // This is the actual load shape: one member bucket contains both UUIDs.
    assert_eq!(v.unbound[&SpecKey(r.id)], vec![old, own]);
    let s = effective_gate_state(HEAD_A, &set, &v, &drain());
    assert!(matches!(s.decision, Decision::Mergeable { .. }), "{s:?}");
}

// trace:STORY-2324 | ai:codex
#[test]
fn unbound_old_head_refusal_is_answered_by_change_approval_regardless_of_file_seq() {
    let r = subject();
    for seq in [1, 9, 50] {
        let mut old = verdict_ev_at(
            seq,
            Some("BUG-2403"),
            "request-changes",
            HEAD_B,
            &at(5),
            &[],
        );
        old.change = "BUG-2403".into();
        old.spec_key = Some(SpecKey(r.id));
        let mut v = view(vec![verdict_ev_at(
            9,
            None,
            "approved",
            HEAD_A,
            &at(9),
            &[],
        )]);
        v.unbound.insert(SpecKey(r.id), vec![old]);
        let s = state(&[r.clone()], &["BUG-2403"], &v, HEAD_A);
        assert!(matches!(s.decision, Decision::Mergeable { .. }), "{s:?}");
    }
}

// trace:STORY-2324 | ai:codex
#[test]
fn change_ledger_supersession_still_requires_later_seq_as_well_as_order_time() {
    let v = view(vec![
        verdict_ev_at(1, None, "approved", HEAD_A, &at(9), &[]),
        verdict_ev_at(2, Some("BUG-2403"), "request-changes", HEAD_B, &at(5), &[]),
    ]);
    let s = state(&[subject()], &["BUG-2403"], &v, HEAD_A);
    assert!(refusals(&s).contains(&Refusal::ChangesRequested), "{s:?}");
}

// trace:STORY-2324 | ai:codex
#[test]
fn same_seq_in_separate_files_does_not_hide_a_late_current_head_refusal() {
    let r = subject();
    let mut v = view(vec![verdict_ev_at(
        1,
        Some("BUG-2403"),
        "approved",
        HEAD_A,
        &at(5),
        &[],
    )]);
    let mut late = verdict_ev_at(1, Some("BUG-2403"), "request-changes", HEAD_A, &at(9), &[]);
    late.change = "BUG-2403".into();
    v.unbound.insert(SpecKey(r.id), vec![late]);
    let s = state(&[r], &["BUG-2403"], &v, HEAD_A);
    assert!(refusals(&s).contains(&Refusal::ChangesRequested), "{s:?}");
}

// trace:STORY-2324 | ai:codex
#[test]
fn foreign_uuid_sequences_are_separate_and_display_labels_are_normalized() {
    let r = subject();
    let foreign = SpecKey(uuid::Uuid::new_v4());
    let other = SpecKey(uuid::Uuid::new_v4());
    let mut refusal = verdict_ev(1, Some(" bug-404 "), "request-changes", HEAD_A);
    refusal.spec_key = Some(foreign);
    let mut approval = verdict_ev(2, Some("BUG-404"), "approved", HEAD_A);
    approval.spec_key = Some(other);
    let mut events = vec![refusal, approval];
    // Stamped and unstamped display labels share normalized diagnostics.
    events.push(verdict_ev(3, Some("bug-404"), "approved", HEAD_A));
    events.push(verdict_ev(4, Some("BUG-2403"), "approved", HEAD_A));
    let s = state(&[r.clone()], &["BUG-2403"], &view(events.clone()), HEAD_A);
    assert_eq!(s.foreign_labels, vec!["BUG-404"]);
    assert!(refusals(&s).contains(&Refusal::ChangesRequested), "{s:?}");
    // Conversely, the same foreign UUID is one sequence across audit labels.
    events[1].spec_key = Some(foreign);
    events[1].spec = Some("TASK-405".into());
    let s = state(&[r], &["BUG-2403"], &view(events), HEAD_A);
    assert!(matches!(s.decision, Decision::Mergeable { .. }), "{s:?}");
}

/// trace:BUG-2500 | ai:codex
#[test]
fn bug_2500_ledger_waiting_blocks_both_heads_until_fresh_review() {
    let r = subject();
    for label in [Some("BUG-2403"), None] {
        let mut waiting = verdict_ev(1, label, "waiting", HEAD_A);
        if let EventBody::Verdict { record } = &mut waiting.body {
            record.insert(
                "summary".into(),
                "CI: required Build pending; reviewer resumes at this head after settlement".into(),
            );
        }
        for head in [HEAD_A, HEAD_B] {
            let s = state(
                &[r.clone()],
                &["BUG-2403"],
                &view(vec![waiting.clone()]),
                head,
            );
            assert!(matches!(s.verdict, Verdict::Waiting { .. }));
            assert!(refusals(&s).contains(&Refusal::Waiting));
            assert_eq!(s.hold, Hold::None);
        }
        let fresh = verdict_ev(2, label, "approved", HEAD_B);
        let s = state(
            &[r.clone()],
            &["BUG-2403"],
            &view(vec![waiting, fresh]),
            HEAD_B,
        );
        assert!(matches!(s.decision, Decision::Mergeable { .. }));
    }
}

// trace:BUG-2517 | ai:codex
#[test]
fn bug_2517_spec_only_never_binds_or_projects_and_related_changes_scan_once() {
    let root = tempfile::tempdir().unwrap();
    let r = subject();
    let set = snapshot(&[r.clone()]).resolve(&["BUG-2403"]);
    let spec = crate::gate::canon::GateSpec::with_key("BUG-2403", Some(SpecKey(r.id)));
    for (i, word) in ["waiting", "approved"].iter().enumerate() {
        let mut local = record(word, HEAD_A, &at(i as u64 + 1), &[]);
        local.insert("change_binding".into(), "unbound".into());
        crate::gate::ledger::store::append_unbound(root.path(), &spec, &local).unwrap();
    }
    for n in [3185, 3194] {
        assert_eq!(
            crate::gate::ledger::store::bind_unbound(root.path(), &spec, ChangeKey::pr(n)).unwrap(),
            0
        );
        let mut e = verdict_ev(n, Some("BUG-2403"), "approved", HEAD_A);
        e.change = format!("PR-{n}");
        e.spec_key = Some(SpecKey(r.id));
        crate::gate::ledger::store::append_one(root.path(), e).unwrap();
        assert_eq!(
            crate::gate::ledger::store::bind_unbound(root.path(), &spec, ChangeKey::pr(n)).unwrap(),
            0
        );
    }
    // Discovery must include unrelated corpus evidence once, too. Keep it
    // unindexed so the hints budget below is exactly this member's reads.
    let mut unrelated = verdict_ev(1, None, "waiting", HEAD_B);
    unrelated.change = "PR-9000".into();
    crate::gate::ledger::store::append_one(root.path(), unrelated).unwrap();
    let dir = root.path().join(".aida/gate-ledger");
    let corpus: u64 = [3185, 3194, 9000]
        .iter()
        .map(|n| {
            std::fs::metadata(dir.join(format!("PR-{n}.jsonl")))
                .unwrap()
                .len()
        })
        .sum();
    let hints: u64 = std::fs::read_dir(dir.join("by-spec"))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    for n in [3185, 3194] {
        let view = load_view_with_budget(
            root.path(),
            ChangeKey::pr(n),
            &set,
            Budget::with_limits(corpus + hints, 8),
        )
        .unwrap();
        assert!(view.unbound.is_empty());
        assert!(view
            .change_events
            .iter()
            .all(|e| e.change == format!("PR-{n}")));
        assert_eq!(
            view.change_events
                .iter()
                .filter(|e| e.verdict_record().is_some())
                .count(),
            1
        );
    }
    // Reusing discovery must not relax the operation's aggregate byte cap.
    assert!(load_view_with_budget(
        root.path(),
        ChangeKey::pr(3185),
        &set,
        Budget::with_limits(corpus + hints - 1, 8),
    )
    .is_err());
    // The one discovery also retains the aggregate event bound.
    assert!(load_view_with_budget(
        root.path(),
        ChangeKey::pr(3185),
        &set,
        Budget::with_limits(corpus + hints, 7),
    )
    .is_err());
    assert_eq!(
        crate::gate::ledger::store::read_unbound(root.path(), "BUG-2403").len(),
        2
    );
}

// trace:BUG-2517 | ai:codex
#[test]
fn bug_2517_direct_gate_views_ignore_explicit_spec_only_records() {
    let r = subject();
    for word in ["waiting", "approved"] {
        let mut local = verdict_ev(1, Some("BUG-2403"), word, HEAD_A);
        if let EventBody::Verdict { record } = &mut local.body {
            record.insert("change_binding".into(), "unbound".into());
        }
        // Both the original spec sequence and a copy made by an older binder
        // must have no effect on the named change, even at its exact head.
        for copied in [false, true] {
            let mut v = view(Vec::new());
            if copied {
                v.change_events.push(local.clone());
            } else {
                v.unbound.insert(SpecKey(r.id), vec![local.clone()]);
            }
            let s = state(&[r.clone()], &["BUG-2403"], &v, HEAD_A);
            assert_eq!(s.verdict, Verdict::Absent, "{word}, copied={copied}");
            assert!(refusals(&s).contains(&Refusal::NoApproval));
        }
    }
}

// trace:BUG-2517 | ai:codex
#[test]
fn bug_2517_cached_discovery_refuses_a_missing_related_change() {
    let root = tempfile::tempdir().unwrap();
    let r = subject();
    let set = snapshot(&[r.clone()]).resolve(&["BUG-2403"]);
    for n in [3185, 3194] {
        let mut own = verdict_ev(1, Some("BUG-2403"), "approved", HEAD_A);
        own.change = format!("PR-{n}");
        own.spec_key = Some(SpecKey(r.id));
        crate::gate::ledger::store::append_one(root.path(), own).unwrap();
    }
    // Keep the index's related change but remove its evidence. Reusing a
    // discovery must refuse this just as the checked spec selector does.
    std::fs::remove_file(crate::gate::ledger::store::ledger_file(
        root.path(),
        ChangeKey::pr(3194),
    ))
    .unwrap();
    let err = load_view(root.path(), ChangeKey::pr(3185), &set).unwrap_err();
    assert!(err.0.contains("PR-3194, whose ledger is missing"), "{err}");
}

// trace:BUG-2517 | ai:codex
#[test]
fn bug_2517_binding_keeps_legacy_prechange_reviews_distinct_from_local_reviews() {
    use crate::gate::ledger::store;
    let root = tempfile::tempdir().unwrap();
    let r = subject();
    let set = snapshot(&[r.clone()]).resolve(&["BUG-2403"]);
    let spec = crate::gate::canon::GateSpec::with_key("BUG-2403", Some(SpecKey(r.id)));
    let legacy = record("approved", HEAD_A, &at(1), &[]);
    store::append_unbound(root.path(), &spec, &legacy).unwrap();
    let mut local = record("waiting", HEAD_A, &at(2), &[]);
    local.insert("change_binding".into(), "unbound".into());
    store::append_unbound(root.path(), &spec, &local).unwrap();
    let unbound = root.path().join(".aida/gate-ledger/by-spec/BUG-2403.jsonl");
    let original = std::fs::read(&unbound).unwrap();
    let change = ChangeKey::pr(3185);
    let v = load_view(root.path(), change, &set).unwrap();
    assert!(matches!(
        effective_gate_state(HEAD_A, &set, &v, &drain()).verdict,
        Verdict::Approved { .. }
    ));
    assert_eq!(store::bind_unbound(root.path(), &spec, change).unwrap(), 1);
    let bound: Vec<_> = store::read_file(root.path(), change)
        .into_iter()
        .filter_map(|e| e.verdict_record().cloned())
        .collect();
    assert_eq!(bound, vec![legacy]);
    let v = load_view(root.path(), change, &set).unwrap();
    assert!(matches!(
        effective_gate_state(HEAD_A, &set, &v, &drain()).verdict,
        Verdict::Approved { .. }
    ));
    assert_eq!(
        store::bind_unbound(root.path(), &spec, ChangeKey::pr(3194)).unwrap(),
        0
    );
    let other = load_view(root.path(), ChangeKey::pr(3194), &set).unwrap();
    assert_eq!(
        effective_gate_state(HEAD_A, &set, &other, &drain()).verdict,
        Verdict::Absent
    );
    assert_eq!(std::fs::read(unbound).unwrap(), original);
}

// trace:BUG-2517 | ai:codex
#[test]
fn bug_2517_local_records_stay_unbound_through_open_record_and_collect() {
    use crate::gate::ledger::{self, store};
    for word in ["waiting", "approved"] {
        let root = tempfile::tempdir().unwrap();
        let _ambient = crate::test_env::AmbientGuard::hermetic_and(
            root.path(),
            None,
            &[("AIDA_CONTAINED", None)],
        );
        let r = subject();
        let set = snapshot(&[r.clone()]).resolve(&["BUG-2403"]);
        let spec = crate::gate::canon::GateSpec::with_key("BUG-2403", Some(SpecKey(r.id)));
        let local_path = crate::gate::review_verdict::verdict_path(root.path(), "BUG-2403");
        let mut local = record(word, HEAD_A, &at(1), &[]);
        local.insert("change_binding".into(), "unbound".into());
        ledger::record_verdict_at(&local_path, &local, Some(&spec)).unwrap();
        let unbound_path = root.path().join(".aida/gate-ledger/by-spec/BUG-2403.jsonl");
        let original = std::fs::read(&unbound_path).unwrap();
        let mut previous_ledger = None;
        for n in [3185, 3194] {
            let change = ChangeKey::pr(n);
            ledger::bind_specs(root.path(), change, &[spec.clone()]);
            assert!(!store::ledger_file(root.path(), change).exists());
            let v = load_view(root.path(), change, &set).unwrap();
            assert!(v.unbound.is_empty());
            assert_eq!(
                effective_gate_state(HEAD_A, &set, &v, &drain()).verdict,
                Verdict::Absent
            );

            let own = record("request-changes", HEAD_A, &at(2), &["own defect"]);
            let change_path =
                crate::gate::review_verdict::verdict_path(root.path(), &change.as_key());
            ledger::record_verdict_at(&change_path, &own, Some(&spec)).unwrap();
            let collected = record("waiting", HEAD_A, &at(3), &[]);
            ledger::record_collected_verdict(
                root.path(),
                change,
                &collected,
                &[(spec.clone(), collected.clone())],
            )
            .unwrap();
            let events = store::read_file(root.path(), change);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.verdict_record().is_some())
                    .count(),
                3
            );
            assert!(events.iter().all(|e| !e.is_spec_only()));
            let v = load_view(root.path(), change, &set).unwrap();
            assert!(v.unbound.is_empty());
            assert!(matches!(
                effective_gate_state(HEAD_A, &set, &v, &drain()).verdict,
                Verdict::Waiting { .. }
            ));
            if let Some(ref bytes) = previous_ledger {
                assert_eq!(
                    &std::fs::read(store::ledger_file(root.path(), ChangeKey::pr(3185))).unwrap(),
                    bytes,
                    "recording and collecting the second PR must preserve the first ledger"
                );
            } else {
                previous_ledger =
                    Some(std::fs::read(store::ledger_file(root.path(), change)).unwrap());
            }
            assert_eq!(std::fs::read(&unbound_path).unwrap(), original);
        }
    }
}

// trace:STORY-2696 | ai:codex
#[test]
fn story_2696_effective_policy_refuses_only_the_moved_head() {
    let mut obj = record("approved", HEAD_A, &at(1), &[]);
    obj.insert(
        rv::ATTESTATION_KEY.into(),
        serde_json::json!({"sha":HEAD_A,"seat":"reviewer","carry_eligible":false}),
    );
    let v = rv::parse_recorded_verdict(&serde_json::to_string(&obj).unwrap()).unwrap();
    assert!(matches!(
        approval_at_head(1, &v, HEAD_A, &[]),
        Verdict::Approved {
            carried_over: false,
            ..
        }
    ));
    assert!(
        matches!(approval_at_head(1, &v, HEAD_B, &[]), Verdict::Stale { reason, .. } if reason.contains("not carry-eligible"))
    );
}
