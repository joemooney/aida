//! BUG-2403: one requirement's friendly and native labels select the SAME
//! current review verdict; a stale alias never overrides a later record;
//! rework imports the current findings only; unsafe or ambiguous evidence
//! refuses instead of falling back to an older approval.
// trace:BUG-2403 | ai:claude
// mandate-gate
use super::*;
use crate::gate::ledger::{store, ChangeKey, EventBody, GateEvent};
use crate::gate::review_verdict::JsonObj;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const FRIENDLY: &str = "STORY-2220";
const NATIVE: &str = "STORY-B6B1278890774EF993D2EBC327862470-262";
const OLD_HEAD: &str = "1a9bb9a000000000000000000000000000000000";
const NEW_HEAD: &str = "6b1fd1a000000000000000000000000000000000";

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

fn record(verdict: &str, head: &str, at: &str, findings: &[&str]) -> JsonObj {
    let Value::Object(obj) = json!({
        "verdict": verdict,
        "reviewed_sha": head,
        "recorded_by": "codex-reviewer-08",
        "recorded_at": at,
        "findings": findings,
    }) else {
        unreachable!()
    };
    obj
}

fn append(root: &Path, change: ChangeKey, spec: &str, rec: JsonObj) {
    let mut e = GateEvent::new(change, EventBody::Verdict { record: rec });
    e.head = e
        .verdict_record()
        .and_then(|r| r.get("reviewed_sha"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    e.spec = Some(spec.to_string());
    store::append(root, change, vec![e]).unwrap();
}

fn selected(sel: AliasSelection) -> crate::gate::review_verdict::RecordedVerdict {
    match sel {
        AliasSelection::Selected(s) => s.verdict,
        other => panic!("expected a selection, got {other:?}"),
    }
}

/// PR3194 shape: friendly refusal at the old head (#7), then the native
/// refusal at the new head (#12). Both label orders, the `Option` API and
/// a single native query agree on the newer record.
#[test]
fn bug_2403_friendly_native_select_same_review() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let pr = ChangeKey::pr(3194);
    append(
        &root,
        pr,
        FRIENDLY,
        record(
            "request_changes",
            OLD_HEAD,
            &ts(-600),
            &["portability", "combined cap"],
        ),
    );
    append(
        &root,
        pr,
        NATIVE,
        record(
            "request_changes",
            NEW_HEAD,
            &ts(-60),
            &["empty local YAML queue"],
        ),
    );
    for ids in [[FRIENDLY, NATIVE], [NATIVE, FRIENDLY]] {
        let v = selected(select_verdict(&root, &ids));
        assert_eq!(v.reviewed_sha.as_deref(), Some(NEW_HEAD), "{ids:?}");
        let any = crate::gate::review_verdict::read_recorded_verdict_any(&root, &ids).unwrap();
        assert_eq!(any, v, "{ids:?}");
    }
    // Lower-case and duplicate labels are the same labels.
    let v = selected(select_verdict(
        &root,
        &[&FRIENDLY.to_ascii_lowercase(), NATIVE, FRIENDLY],
    ));
    assert_eq!(v.reviewed_sha.as_deref(), Some(NEW_HEAD));
    // Every event stays in the retained history of its label.
    assert_eq!(
        crate::gate::ledger::project::verdict_records(&root, FRIENDLY).len(),
        1
    );
}

/// PR3185 shape: an old friendly APPROVAL and a later native refusal — and
/// the reverse — always answer with the later record.
#[test]
fn bug_2403_later_record_wins_in_both_directions() {
    for (old, new) in [
        ("approved", "request_changes"),
        ("request_changes", "approved"),
    ] {
        let (_tmp, root) = root();
        let _env = uncontained();
        let pr = ChangeKey::pr(3185);
        append(
            &root,
            pr,
            FRIENDLY,
            record(old, OLD_HEAD, &ts(-600), &["a"]),
        );
        append(&root, pr, NATIVE, record(new, NEW_HEAD, &ts(-60), &[]));
        let v = selected(select_verdict(&root, &[FRIENDLY, NATIVE]));
        assert_eq!(v.raw, new, "{old} -> {new}");
        // And a later friendly record overrides an older native one.
        append(&root, pr, FRIENDLY, record(old, NEW_HEAD, &ts(-5), &["b"]));
        let v = selected(select_verdict(&root, &[NATIVE, FRIENDLY]));
        assert_eq!(v.raw, old, "later friendly {old}");
    }
}

/// Rework imports the CURRENT findings only (not the obsolete friendly
/// round), an authoritative current approval yields no findings, and the
/// recurrence guard keeps its policy on the selected block: an immediate
/// genuine repeat recurs, A → B → A does not.
#[test]
fn bug_2403_rework_uses_current_canonical_findings() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let pr = ChangeKey::pr(3194);
    append(
        &root,
        pr,
        FRIENDLY,
        record(
            "request_changes",
            OLD_HEAD,
            &ts(-600),
            &["portability", "combined cap"],
        ),
    );
    append(
        &root,
        pr,
        NATIVE,
        record(
            "request_changes",
            NEW_HEAD,
            &ts(-60),
            &["empty local YAML queue"],
        ),
    );
    let block = crate::queue_cmd::rework_findings_block_for_spec(&root, FRIENDLY, NATIVE)
        .expect("the current refusal's findings");
    assert!(block.contains("empty local YAML queue"), "{block}");
    assert!(!block.contains("portability"), "{block}");
    assert!(!block.contains("combined cap"), "{block}");
    assert_eq!(
        crate::queue_cmd::rework_findings_block_for_spec(&root, NATIVE, FRIENDLY)
            .map(|b| b.contains("empty local YAML queue")),
        Some(true)
    );

    let stale = {
        let (_t, other) = self::root();
        append(
            &other,
            pr,
            FRIENDLY,
            record(
                "request_changes",
                OLD_HEAD,
                &ts(-600),
                &["portability", "combined cap"],
            ),
        );
        crate::queue_cmd::rework_findings_block_for_spec(&other, FRIENDLY, NATIVE).unwrap()
    };
    let comment = |s: &str| aida_core::Comment::new("a".into(), s.to_string());
    // The previous round was the obsolete block: the current one is progress.
    assert!(!crate::queue_cmd::findings_recur_consecutively(
        &[comment(&stale)],
        &block
    ));
    // A genuine immediate repeat of the current findings still recurs.
    assert!(crate::queue_cmd::findings_recur_consecutively(
        &[comment(&stale), comment(&block)],
        &block
    ));
    // A → B → A is not a loop.
    assert!(!crate::queue_cmd::findings_recur_consecutively(
        &[comment(&block), comment(&stale)],
        &block
    ));

    // A later approval at the new head: no findings, no obsolete fallback.
    append(
        &root,
        pr,
        NATIVE,
        record("approved", NEW_HEAD, &ts(-5), &[]),
    );
    assert_eq!(
        crate::queue_cmd::rework_findings_block_for_spec(&root, FRIENDLY, NATIVE),
        None
    );
}

/// Labels are bridged only through one unambiguously resolved requirement:
/// unknown, colliding, prefix and change-key labels stay literal.
#[test]
fn bug_2403_alias_cannot_borrow_authority() {
    let mut real = aida_core::Requirement::new("real".into(), "d".into());
    real.spec_id = Some(NATIVE.into());
    real.agreed_id = Some(FRIENDLY.into());
    let mut other = aida_core::Requirement::new("other".into(), "d".into());
    other.spec_id = Some("STORY-B6B1278890774EF993D2EBC327862470-263".into());
    other.agreed_id = Some("STORY-2221".into());
    let mut store = aida_core::RequirementsStore::new();
    store.requirements = vec![real, other];

    let mut both = vec![FRIENDLY.to_string(), NATIVE.to_string()];
    both.sort();
    let mut got = labels_for(&store, FRIENDLY);
    got.sort();
    assert_eq!(got, both);
    let mut got = labels_for(&store, NATIVE);
    got.sort();
    assert_eq!(got, both);
    assert_eq!(labels_for(&store, "STORY-9999"), vec!["STORY-9999"]);
    assert_eq!(
        labels_for(&store, "STORY-B6B1278890774EF993D2EBC327862470"),
        vec!["STORY-B6B1278890774EF993D2EBC327862470"],
        "a node prefix bridges nothing"
    );
    assert_eq!(labels_for(&store, "PR-3194"), vec!["PR-3194"]);

    // A friendly label two requirements claim is ambiguous: literal only.
    let mut clash = aida_core::Requirement::new("clash".into(), "d".into());
    clash.spec_id = Some(FRIENDLY.into());
    store.requirements.push(clash);
    assert_eq!(labels_for(&store, FRIENDLY), vec![FRIENDLY]);

    // An unrelated spec on the shared PR is not one of this spec's labels.
    let (_tmp, root) = root();
    let _env = uncontained();
    let pr = ChangeKey::pr(3194);
    append(
        &root,
        pr,
        "STORY-2221",
        record("approved", NEW_HEAD, &ts(-5), &[]),
    );
    append(
        &root,
        pr,
        FRIENDLY,
        record("request_changes", OLD_HEAD, &ts(-600), &["x"]),
    );
    let v = selected(select_verdict(&root, &[FRIENDLY, NATIVE]));
    assert_eq!(v.raw, "request_changes");
    assert_eq!(
        spec_labels(&["", "???", "PR-1", "MR-2"]),
        Vec::<String>::new()
    );
}

/// Equal-time records that differ are ambiguous: only a refusal surfaces,
/// never an approval; the queue-done gate refuses rather than proceeding.
#[test]
fn bug_2403_equal_time_records_are_ambiguous() {
    let at = ts(-60);
    let (_tmp, root) = root();
    let _env = uncontained();
    let pr = ChangeKey::pr(3185);
    append(&root, pr, FRIENDLY, record("approved", NEW_HEAD, &at, &[]));
    append(
        &root,
        pr,
        NATIVE,
        record("request_changes", NEW_HEAD, &at, &["y"]),
    );
    let sel = select_verdict(&root, &[FRIENDLY, NATIVE]);
    assert!(matches!(sel, AliasSelection::Ambiguous { .. }), "{sel:?}");
    assert_eq!(
        sel.fail_closed().map(|v| v.raw),
        Some("request_changes".into())
    );

    let (_tmp2, root) = self::root();
    let mut a = record("approved", NEW_HEAD, &at, &[]);
    let b = record("approved", NEW_HEAD, &at, &[]);
    a.insert("recorded_by".into(), json!("someone-else"));
    append(&root, pr, FRIENDLY, a);
    append(&root, pr, NATIVE, b);
    let sel = select_verdict(&root, &[FRIENDLY, NATIVE]);
    assert!(matches!(sel, AliasSelection::Ambiguous { .. }), "{sel:?}");
    assert_eq!(sel.fail_closed(), None, "no approval is elected");
    let (gate, _) =
        crate::gate::review_verdict::evaluate_review_verdict_gate(&root, FRIENDLY, NATIVE);
    assert!(
        matches!(gate, crate::gate::review_verdict::VerdictGate::Refuse(_)),
        "{gate:?}"
    );
}

fn two_label_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (tmp, root) = root();
    let pr = ChangeKey::pr(3185);
    append(
        &root,
        pr,
        FRIENDLY,
        record("approved", OLD_HEAD, &ts(-600), &[]),
    );
    let native_pr = ChangeKey::pr(3186);
    append(
        &root,
        native_pr,
        NATIVE,
        record("request_changes", NEW_HEAD, &ts(-60), &["z"]),
    );
    let ledger = store::ledger_file(&root, native_pr);
    (tmp, root, ledger)
}

fn assert_unsafe_no_approval(root: &Path, what: &str) {
    let sel = select_verdict(root, &[FRIENDLY, NATIVE]);
    assert!(
        matches!(sel, AliasSelection::Unsafe { .. }),
        "{what}: {sel:?}"
    );
    assert_eq!(
        sel.fail_closed(),
        None,
        "{what}: no fallback to the older approval"
    );
}

/// Torn, malformed, version-invalid, misbound, linked, hard-linked, FIFO and
/// over-budget evidence refuses selection without hanging, and never falls
/// back to the older approval on the other label.
#[test]
fn bug_2403_selection_unsafe_evidence_refuses() {
    let _env = uncontained();

    let (_t, root, ledger) = two_label_fixture();
    let body = std::fs::read_to_string(&ledger).unwrap();
    std::fs::write(&ledger, format!("{body}{{\"v\":1,\"seq\":9")).unwrap();
    assert_unsafe_no_approval(&root, "torn final line");

    let (_t, root, ledger) = two_label_fixture();
    std::fs::write(&ledger, format!("{body}not json\n")).unwrap();
    assert_unsafe_no_approval(&root, "malformed line");

    let (_t, root, ledger) = two_label_fixture();
    let bumped = body.replacen("\"v\":1", "\"v\":99", 1);
    std::fs::write(&ledger, bumped).unwrap();
    assert_unsafe_no_approval(&root, "unsupported version");

    let (_t, root, ledger) = two_label_fixture();
    let misbound = body.replacen("\"change\":\"PR-3186\"", "\"change\":\"PR-1\"", 1);
    std::fs::write(&ledger, misbound).unwrap();
    assert_unsafe_no_approval(&root, "event bound to another change");

    let (_t, root, ledger) = two_label_fixture();
    let real = ledger.with_extension("real");
    std::fs::rename(&ledger, &real).unwrap();
    std::os::unix::fs::symlink(&real, &ledger).unwrap();
    assert_unsafe_no_approval(&root, "symlinked ledger");

    let (_t, root, ledger) = two_label_fixture();
    std::fs::hard_link(&ledger, ledger.with_extension("twin")).unwrap();
    assert_unsafe_no_approval(&root, "hard-linked ledger");

    let (_t, root, ledger) = two_label_fixture();
    std::fs::remove_file(&ledger).unwrap();
    let c = std::ffi::CString::new(ledger.to_str().unwrap()).unwrap();
    // SAFETY: a NUL-terminated path; mkfifo has no other preconditions.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert_unsafe_no_approval(&root, "FIFO in place of the ledger");

    let (_t, root, ledger) = two_label_fixture();
    std::fs::remove_file(&ledger).unwrap();
    assert_unsafe_no_approval(&root, "index names a missing ledger");

    let (_t, root, _ledger) = two_label_fixture();
    let by_spec = crate::gate::ledger::ledger_dir(&root).join("by-spec");
    let moved = by_spec.with_extension("real");
    std::fs::rename(&by_spec, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &by_spec).unwrap();
    assert_unsafe_no_approval(&root, "linked by-spec directory");

    let (_t, root, _ledger) = two_label_fixture();
    let labels = [FRIENDLY.to_string(), NATIVE.to_string()];
    let sel = across(&root, &labels, &mut Budget::with_limits(64, 1));
    assert!(matches!(sel, AliasSelection::Unsafe { .. }), "{sel:?}");

    // The same fixture, intact, selects the newer native refusal.
    let (_t, root, _ledger) = two_label_fixture();
    let v = selected(select_verdict(&root, &[FRIENDLY, NATIVE]));
    assert_eq!(v.raw, "request_changes");
}
