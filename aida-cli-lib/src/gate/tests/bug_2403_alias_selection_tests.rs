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
    let mut reqs = vec![real, other];
    let maps = SnapshotMaps::default();
    let labels = |reqs: &[aida_core::Requirement], key: &str| {
        let mut got = resolve_labels(reqs, &maps, key).unwrap();
        got.sort();
        got
    };

    let mut both = vec![FRIENDLY.to_string(), NATIVE.to_string()];
    both.sort();
    assert_eq!(labels(&reqs, FRIENDLY), both);
    assert_eq!(labels(&reqs, NATIVE), both);
    assert_eq!(labels(&reqs, "STORY-9999"), vec!["STORY-9999"]);
    assert_eq!(
        labels(&reqs, "STORY-B6B1278890774EF993D2EBC327862470"),
        vec!["STORY-B6B1278890774EF993D2EBC327862470"],
        "a node prefix bridges nothing"
    );
    assert_eq!(
        labels(&reqs, "PR-3194"),
        vec!["PR-3194"],
        "a change key is its own ledger"
    );

    // A friendly label two requirements claim is ambiguous: the key itself
    // refuses, and reached from the native side it is dropped, not bridged.
    let mut clash = aida_core::Requirement::new("clash".into(), "d".into());
    clash.spec_id = Some(FRIENDLY.into());
    reqs.push(clash);
    assert!(resolve_labels(&reqs, &maps, FRIENDLY).is_err());
    assert_eq!(labels(&reqs, NATIVE), vec![NATIVE.to_string()]);

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

/// Finding 2 (amendment 6): the `by-spec` index is a hint. A newer native
/// refusal appended to a change ledger whose index entry never landed (the
/// writer appends first and only warns when indexing fails) is still
/// discovered, so the older friendly approval cannot be selected over it;
/// with the index deleted outright the same holds.
#[test]
fn bug_2403_stale_or_missing_index_after_append_still_discovers_the_refusal() {
    let _env = uncontained();
    let (_tmp, root) = root();
    append(
        &root,
        ChangeKey::pr(3185),
        FRIENDLY,
        record("approved", OLD_HEAD, &ts(-600), &[]),
    );
    append(
        &root,
        ChangeKey::pr(3186),
        NATIVE,
        record("request_changes", NEW_HEAD, &ts(-60), &["newer"]),
    );
    let by_spec = crate::gate::ledger::ledger_dir(&root).join("by-spec");
    // The index lags: the native label's entry is gone.
    std::fs::remove_file(by_spec.join(format!("{NATIVE}.json"))).unwrap();
    let v = selected(select_verdict(&root, &[FRIENDLY, NATIVE]));
    assert_eq!(
        v.raw, "request_changes",
        "index lag must not hide the refusal"
    );
    assert_eq!(v.findings, vec!["newer".to_string()]);
    // No index at all.
    std::fs::remove_dir_all(&by_spec).unwrap();
    let v = selected(select_verdict(&root, &[NATIVE, FRIENDLY]));
    assert_eq!(v.raw, "request_changes");
    // The Option API and the queue-done gate agree.
    assert_eq!(
        crate::gate::review_verdict::read_recorded_verdict_any(&root, &[FRIENDLY, NATIVE])
            .map(|v| v.raw),
        Some("request_changes".to_string())
    );
}

/// Finding 5: a linked or unreadable legacy verdict FILE under one label is
/// refused evidence (never "absent"), so the other label's older approval is
/// not selected past it; the same refusal holds for a linked directory.
#[test]
fn bug_2403_linked_legacy_verdict_file_refuses_selection() {
    let _env = uncontained();
    let (_tmp, root) = root();
    append(
        &root,
        ChangeKey::pr(3185),
        FRIENDLY,
        record("approved", OLD_HEAD, &ts(-600), &[]),
    );
    let path = crate::gate::review_verdict::verdict_path(&root, NATIVE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let elsewhere = root.join("elsewhere.json");
    std::fs::write(
        &elsewhere,
        serde_json::to_string(&record("request_changes", NEW_HEAD, &ts(-5), &["linked"])).unwrap(),
    )
    .unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    assert_unsafe_no_approval(&root, "symlinked legacy verdict file");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_unsafe_no_approval(&root, "a directory in place of the verdict file");
}

/// A git-canonical store fixture: `<root>/.aida/config.toml` naming
/// `<root>/.aida-store`, a git-initialised store holding the one requirement
/// with both labels, committed.
fn snapshot_store() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (tmp, root) = root();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida").join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let store = root.join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    aida_core::git_ops::init(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "fixture", "fixture@localhost").unwrap();
    let backend = crate::test_env::home_fixture_cached_backend(
        &store,
        &aida_core::CachedGitBackend::default_cache_path(&store),
    )
    .unwrap();
    let mut req = aida_core::Requirement::new("real".into(), "d".into());
    req.spec_id = Some(NATIVE.into());
    req.agreed_id = Some(FRIENDLY.into());
    aida_core::db::DatabaseBackend::add_requirement(&backend, req).unwrap();
    commit_store(&store, "fixture");
    (tmp, root, store)
}

fn commit_store(store: &Path, msg: &str) {
    for args in [
        vec!["add", "-A"],
        vec!["commit", "-q", "--allow-empty", "-m", msg],
    ] {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(store)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// Finding 1 (amendment 1): identity is proved against ONE committed store
/// snapshot. Friendly, native and committed node-label forms bridge to the
/// same requirement; a node-label mapping CHANGED in the working tree (the
/// objects clean) refuses, as do dirty or untracked objects; a committed
/// remap that stops resolving the label bridges nothing.
#[test]
fn bug_2403_snapshot_identity_is_bound_to_the_committed_mappings() {
    let _env = uncontained();
    let (_tmp, root, store) = snapshot_store();
    let both = sorted(vec![FRIENDLY.to_string(), NATIVE.to_string()]);
    assert_eq!(sorted(snapshot_labels(&root, FRIENDLY).unwrap()), both);
    assert_eq!(sorted(snapshot_labels(&root, NATIVE).unwrap()), both);
    assert_eq!(snapshot_labels(&root, "PR-3194").unwrap(), vec!["PR-3194"]);
    assert_eq!(
        snapshot_labels(&root, "STORY-9999").unwrap(),
        vec!["STORY-9999"]
    );

    // A committed node label bridges its alias form too.
    let registry = store.join("registry");
    std::fs::create_dir_all(&registry).unwrap();
    let aliases = registry.join("node_aliases.toml");
    std::fs::write(
        &aliases,
        "[[binding]]\nlabel = \"HOME\"\nnode = \"b6b1278890774ef993d2ebc327862470\"\n",
    )
    .unwrap();
    commit_store(&store, "alias");
    let with_alias = sorted(vec![
        FRIENDLY.to_string(),
        NATIVE.to_string(),
        "STORY-HOME-262".to_string(),
    ]);
    assert_eq!(
        sorted(snapshot_labels(&root, "STORY-HOME-262").unwrap()),
        with_alias
    );
    assert_eq!(
        sorted(snapshot_labels(&root, FRIENDLY).unwrap()),
        with_alias
    );

    // The mapping changes in the working tree while every object is clean:
    // the snapshot no longer proves identity.
    std::fs::write(
        &aliases,
        "[[binding]]\nlabel = \"AWAY\"\nnode = \"b6b1278890774ef993d2ebc327862470\"\n",
    )
    .unwrap();
    let err = snapshot_labels(&root, FRIENDLY).unwrap_err();
    assert!(err.0.contains("uncommitted"), "{err}");

    // Committed, the remap drops the old label rather than bridging it.
    commit_store(&store, "remap");
    assert_eq!(
        snapshot_labels(&root, "STORY-HOME-262").unwrap(),
        vec!["STORY-HOME-262"]
    );

    // An untracked object refuses too.
    std::fs::create_dir_all(store.join("objects")).unwrap();
    std::fs::write(store.join("objects").join("stray.yaml"), "x: 1\n").unwrap();
    assert!(snapshot_labels(&root, FRIENDLY).is_err());
}

/// The pure resolver: retired IDs bridge only with the recorded UUID; a
/// node label naming no stored requirement bridges nothing.
#[test]
fn bug_2403_retired_ids_bridge_only_with_the_same_uuid() {
    let mut real = aida_core::Requirement::new("real".into(), "d".into());
    real.spec_id = Some(NATIVE.into());
    real.agreed_id = Some(FRIENDLY.into());
    let uuid = real.id.to_string();
    let reqs = vec![real];
    let good = SnapshotMaps::parse(
        None,
        Some(&format!(
            "[[retired]]\nold = \"STORY-1-418\"\nnew = \"{NATIVE}\"\nuuid = \"{uuid}\"\n"
        )),
    )
    .unwrap();
    assert_eq!(
        sorted(resolve_labels(&reqs, &good, "STORY-1-418").unwrap()),
        sorted(vec![
            FRIENDLY.to_string(),
            NATIVE.to_string(),
            "STORY-1-418".to_string()
        ])
    );
    let wrong = SnapshotMaps::parse(
        None,
        Some(&format!(
            "[[retired]]\nold = \"STORY-1-418\"\nnew = \"{NATIVE}\"\nuuid = \"00000000-0000-0000-0000-000000000000\"\n"
        )),
    )
    .unwrap();
    assert_eq!(
        resolve_labels(&reqs, &wrong, "STORY-1-418").unwrap(),
        vec!["STORY-1-418"]
    );
    assert!(SnapshotMaps::parse(Some("not = [toml"), None).is_err());
}

// trace:BUG-2507 | ai:codex
#[test]
fn bug_2507_rework_imports_large_ledger_findings_and_refuses_over_limit() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let pr = ChangeKey::pr(3708);
    let large = format!("{}end of finding", "review finding ".repeat(100_000));
    assert!(large.len() > 1024 * 1024);
    append(
        &root,
        pr,
        FRIENDLY,
        record("request_changes", NEW_HEAD, &ts(-60), &[&large]),
    );
    let block = crate::queue_cmd::resolve_rework_findings(&root, FRIENDLY, NATIVE, None)
        .expect("large ledger remains readable")
        .expect("findings imported");
    assert!(block.contains(&large));

    let too_large = "x".repeat(crate::gate::ledger::checked::MAX_LINE_BYTES + 1);
    append(
        &root,
        pr,
        FRIENDLY,
        record("request_changes", NEW_HEAD, &ts(-30), &[&too_large]),
    );
    assert!(
        crate::queue_cmd::resolve_rework_findings(&root, FRIENDLY, NATIVE, None)
            .unwrap()
            .is_none(),
        "must not import older findings"
    );
    let why = crate::gate::ledger::checked::read_change_events(
        &root,
        pr,
        &mut crate::gate::ledger::checked::Budget::default(),
    )
    .expect_err("oversized current evidence must refuse")
    .to_string();
    assert!(why.contains("PR-3708.jsonl line 3"), "{why}");
    assert!(why.contains("oversized"), "{why}");
    assert!(why.contains("limit 8388608"), "{why}");
}

// trace:BUG-2531 | ai:codex
#[test]
fn bug_2531_unrelated_closed_history_refuses_at_discovery_limit() {
    let (_tmp, root, _) = two_label_fixture();
    let _env = uncontained();
    // Ten valid files below the per-file/line caps, together above 64 MiB.
    // Neither the index nor closed_by_merge is trusted to skip discovery.
    let large = "x".repeat(7 * 1024 * 1024);
    let mut total = 0;
    for n in 1..=10 {
        let change = ChangeKey::pr(n);
        let mut rec = record("approved", OLD_HEAD, &ts(-600), &[&large]);
        rec.insert("closed_by_merge".into(), json!(OLD_HEAD));
        let mut event = GateEvent::new(change, EventBody::Verdict { record: rec });
        event.spec = Some("STORY-UNRELATED-1".into());
        event.seq = 1;
        let body = format!("{}\n", serde_json::to_string(&event).unwrap());
        total += body.len() as u64;
        std::fs::write(store::ledger_file(&root, change), body).unwrap();
    }
    assert!(total > crate::gate::ledger::checked::MAX_SELECTION_BYTES);
    // Missing hints cannot authorize skipping evidence. Until a complete
    // binding index exists, oversized discovery must refuse rather than do
    // unbounded unrelated work or return an older approval.
    let index = crate::gate::ledger::ledger_dir(&root).join("by-spec");
    std::fs::remove_file(index.join(format!("{NATIVE}.json"))).unwrap();
    let sel = select_verdict(&root, &[FRIENDLY, NATIVE]);
    assert!(matches!(sel, AliasSelection::Unsafe { .. }), "{sel:?}");
}

// trace:BUG-2531 | ai:codex
#[test]
fn bug_2531_selected_payload_is_charged_once_across_labels() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let change = ChangeKey::pr(1);
    append(
        &root,
        change,
        FRIENDLY,
        record("approved", OLD_HEAD, &ts(-600), &[]),
    );
    append(
        &root,
        change,
        NATIVE,
        record("request_changes", NEW_HEAD, &ts(-60), &[]),
    );
    let path = store::ledger_file(&root, change);
    let ledger_bytes = std::fs::metadata(&path).unwrap().len();
    let by_spec = crate::gate::ledger::ledger_dir(&root).join("by-spec");
    let index_bytes = [FRIENDLY, NATIVE]
        .iter()
        .map(|label| {
            std::fs::metadata(by_spec.join(format!("{label}.json")))
                .unwrap()
                .len()
        })
        .sum::<u64>();
    let labels = [FRIENDLY.to_string(), NATIVE.to_string()];
    let mut budget = Budget::with_limits(ledger_bytes + index_bytes, 3);
    assert_eq!(
        selected(across(&root, &labels, &mut budget)).raw,
        "request_changes"
    );
    let mut budget = Budget::with_limits(ledger_bytes + index_bytes - 1, 3);
    assert!(matches!(
        across(&root, &labels, &mut budget),
        AliasSelection::Unsafe { .. }
    ));
}
