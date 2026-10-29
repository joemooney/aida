//! M1 slice 1: the gate ledger — append/lock/seq, the change as the only
//! key, contained submissions through the mailbox, legacy read-through, and
//! the legacy-path ratchet.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate
use super::event::{EventBody, GateEvent};
use super::*;
use crate::gate::review_verdict::{self as rv, JsonObj, RecorderAttestation, ATTESTATION_KEY};

const HEAD: &str = "1111111111111111111111111111111111111111";

fn root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    (tmp, root)
}

fn attestation(seat: &str) -> RecorderAttestation {
    RecorderAttestation {
        sha: HEAD.to_string(),
        seat: Some(seat.to_string()),
        grant_id: Some("g-1".to_string()),
        ..Default::default()
    }
}

/// A real reviewer seat grant for the hermetic test user, and an
/// attestation that presents it — what `aida review record` stores. The
/// guard holds the env lock; keep it alive for the test.
fn granted_reviewer(root: &Path) -> (crate::test_env::AmbientGuard, RecorderAttestation) {
    let guard = crate::test_env::AmbientGuard::hermetic(root, None);
    let id = crate::gate::seat_authority::test_support::mint_grant_for(
        root,
        crate::test_env::HERMETIC_TEST_USER,
        "reviewer",
        &[],
    );
    let binding = crate::gate::seat_authority::read_grant(&id)
        .unwrap()
        .binding;
    let actor = RecorderAttestation {
        grant_id: Some(id),
        grant_binding: binding,
        ..attestation("reviewer")
    };
    (guard, actor)
}

/// `now + secs` as RFC 3339: a grant minted now authorizes only verdicts
/// recorded after it was issued.
fn ts(secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339()
}

fn record(verdict: &str, at: &str, actor: Option<&RecorderAttestation>) -> JsonObj {
    let mut obj = JsonObj::new();
    for (k, v) in [
        ("verdict", verdict),
        ("reviewed_sha", HEAD),
        ("recorded_by", "reviewer"),
        ("recorded_at", at),
    ] {
        obj.insert(k.to_string(), serde_json::Value::String(v.to_string()));
    }
    if let Some(actor) = actor {
        obj.insert(
            ATTESTATION_KEY.to_string(),
            serde_json::to_value(actor).unwrap(),
        );
    }
    obj
}

fn verdict(change: ChangeKey, spec: Option<&str>, rec: JsonObj) -> GateEvent {
    let mut event = GateEvent::new(change, EventBody::Verdict { record: rec });
    event.head = Some(HEAD.to_string());
    event.spec = spec.map(str::to_string);
    event.actor = event
        .verdict_record()
        .and_then(|r| r.get(ATTESTATION_KEY).cloned())
        .and_then(|v| serde_json::from_value(v).ok());
    event
}

fn kinds(events: &[GateEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|e| match e.body {
            EventBody::Verdict { .. } => "verdict",
            EventBody::LegacyImported { .. } => "legacy-imported",
            EventBody::RejectedEvent { .. } => "rejected-event",
        })
        .collect()
}

#[test]
fn change_keys_parse_only_forge_changes() {
    assert_eq!(ChangeKey::parse("pr-12"), Some(ChangeKey::pr(12)));
    assert_eq!(
        ChangeKey::parse(" MR-3 ").map(|c| c.as_key()),
        Some("MR-3".to_string())
    );
    for not_a_change in ["STORY-12", "PR-", "PR-1a", "PR--1", ""] {
        assert_eq!(ChangeKey::parse(not_a_change), None, "{not_a_change}");
    }
}

#[test]
fn appends_round_trip_with_monotonic_seq_and_a_one_time_legacy_import() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(7);
    let a = attestation("reviewer");
    store::append(
        &root,
        change,
        vec![verdict(
            change,
            None,
            record("approved", "2026-10-29T10:00:00Z", Some(&a)),
        )],
    )
    .unwrap();
    store::append(
        &root,
        change,
        vec![verdict(
            change,
            None,
            record("request-changes", "2026-10-29T11:00:00Z", Some(&a)),
        )],
    )
    .unwrap();
    let events = store::read_file(&root, change);
    assert_eq!(kinds(&events), ["legacy-imported", "verdict", "verdict"]);
    assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(events
        .iter()
        .all(|e| e.change == "PR-7" && !e.at.is_empty()));
    assert!(events[1..].iter().all(|e| e.head.as_deref() == Some(HEAD)));
    assert_eq!(
        store::append(&root, change, Vec::new()).unwrap(),
        Vec::new()
    );
    assert_eq!(
        store::read_file(&root, change).len(),
        3,
        "nothing to append writes nothing"
    );
}

#[test]
fn concurrent_appenders_never_share_a_seq() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(8);
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let root = root.clone();
            std::thread::spawn(move || {
                for j in 0..5 {
                    let at = format!("2026-10-29T10:{i:02}:{j:02}Z");
                    store::append(
                        &root,
                        change,
                        vec![verdict(change, None, record("approved", &at, None))],
                    )
                    .unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let seqs: Vec<u64> = store::read_file(&root, change)
        .iter()
        .map(|e| e.seq)
        .collect();
    assert_eq!(
        seqs,
        (1..=41).collect::<Vec<_>>(),
        "one import + 40 verdicts, in order"
    );
}

#[test]
fn a_verdict_for_the_spec_of_a_pr_and_one_for_the_pr_are_the_same_record() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(9);
    let a = attestation("reviewer");
    let rec = record("approved", "2026-10-29T10:00:00Z", Some(&a));
    assert_eq!(
        record_event(&root, verdict(change, Some("STORY-42"), rec.clone()), false).unwrap(),
        Delivered::Appended
    );
    let by_pr = project::newest_verdict(&root, "PR-9").unwrap();
    let by_spec = project::newest_verdict(&root, "story-42").unwrap();
    assert_eq!(by_pr, rec);
    assert_eq!(by_spec, rec);
    assert_eq!(project::changes_for_spec(&root, "STORY-42"), [change]);
    assert_eq!(project::newest_verdict(&root, "STORY-43"), None);
}

#[test]
fn rewriting_the_same_record_does_not_append_twice() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(10);
    let rec = record("approved", "2026-10-29T10:00:00Z", None);
    assert_eq!(
        record_event(&root, verdict(change, None, rec.clone()), false).unwrap(),
        Delivered::Appended
    );
    assert_eq!(
        record_event(&root, verdict(change, None, rec), false).unwrap(),
        Delivered::Unchanged
    );
    assert_eq!(
        kinds(&store::read_file(&root, change)),
        ["legacy-imported", "verdict"]
    );
}

#[test]
fn a_contained_writer_submits_through_the_mailbox_and_the_next_read_ingests_it() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(11);
    let (_guard, a) = granted_reviewer(&root);
    let rec = record("approved", &ts(1), Some(&a));
    let Delivered::Spooled(file) =
        record_event(&root, verdict(change, Some("BUG-5"), rec.clone()), true).unwrap()
    else {
        panic!("a contained writer must not append");
    };
    assert!(file.starts_with(spool::spool_root(&root)));
    assert!(
        !store::ledger_file(&root, change).exists(),
        "the ledger is untouched until ingest"
    );
    assert_eq!(project::newest_verdict(&root, "PR-11"), Some(rec.clone()));
    assert!(!file.exists(), "an ingested submission is consumed");
    let events = store::read_file(&root, change);
    assert_eq!(kinds(&events), ["legacy-imported", "verdict"]);
    assert!(events[1].submission.is_some());
    assert_eq!(project::newest_verdict(&root, "BUG-5"), Some(rec));
}

#[test]
fn an_unauthorized_submission_is_rejected_and_kept_as_evidence() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(12);
    // An implementer seat cannot approve, and an unattested record proves nothing.
    let implementer = attestation("implementer");
    for rec in [
        record("approved", "2026-10-29T10:00:00Z", Some(&implementer)),
        record("approved", "2026-10-29T10:01:00Z", None),
    ] {
        spool::submit(&root, &verdict(change, None, rec)).unwrap();
    }
    // A submission naming another change is refused too.
    let a = attestation("reviewer");
    let mut stray = verdict(
        ChangeKey::pr(99),
        None,
        record("approved", "2026-10-29T10:02:00Z", Some(&a)),
    );
    let dir = spool::spool_dir(&root, change);
    stray.change = "PR-99".into();
    std::fs::write(
        dir.join("zz-stray.json"),
        serde_json::to_vec(&stray).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("zzz-garbage.json"), b"not json").unwrap();

    assert_eq!(project::newest_verdict(&root, "PR-12"), None);
    assert_eq!(rv::read_recorded_verdict(&root, "PR-12"), None);
    let events = project::events(&root, change);
    assert_eq!(
        kinds(&events),
        [
            "legacy-imported",
            "rejected-event",
            "rejected-event",
            "rejected-event",
            "rejected-event"
        ]
    );
    let reasons: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::RejectedEvent { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert!(reasons[0].contains("does not authorize"), "{reasons:?}");
    assert!(
        reasons[1].contains("no recorder attestation"),
        "{reasons:?}"
    );
    assert!(reasons[2].contains("PR-99"), "{reasons:?}");
    assert!(reasons[3].contains("not a gate event"), "{reasons:?}");
    assert!(!spool::has_pending(&root, change));
}

#[test]
fn legacy_files_are_read_through_and_imported_verbatim_on_the_first_write() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(13);
    let current = rv::verdict_path(&root, "PR-13");
    let archive = rv::verdict_archive_dir(&current).unwrap();
    std::fs::create_dir_all(&archive).unwrap();
    let old = "{\"verdict\":\"request-changes\",\"reviewed_sha\":\"aaaaaaa\"}\n";
    let cur = "{\"verdict\":\"approved\",\"reviewed_sha\":\"bbbbbbb\"}\n";
    std::fs::write(archive.join("aaaaaaa.json"), old).unwrap();
    std::fs::write(&current, cur).unwrap();

    assert_eq!(
        rv::read_recorded_verdict(&root, "PR-13").unwrap().kind,
        rv::VerdictKind::Approved,
        "read through before any ledger write"
    );
    record_event(
        &root,
        verdict(
            change,
            None,
            record("approved", "2026-10-29T12:00:00Z", None),
        ),
        false,
    )
    .unwrap();
    let events = store::read_file(&root, change);
    let EventBody::LegacyImported { legacy } = &events[0].body else {
        panic!("the first event is the legacy import");
    };
    let bodies: Vec<&str> = legacy.iter().map(|f| f.body.as_str()).collect();
    assert_eq!(bodies, [cur, old], "imported byte for byte");
    assert!(
        current.exists() && archive.join("aaaaaaa.json").exists(),
        "legacy files are never deleted"
    );
}

/// Finding 6: the first PR-keyed write imports the legacy files as they were
/// before it, so its own record lands on the ledger exactly once.
#[test]
fn the_first_verdict_write_is_recorded_once() {
    // An uncontained writer, as the primary side runs it.
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let (_tmp, root) = root();
    let change = ChangeKey::pr(17);
    let path = rv::verdict_path(&root, "PR-17");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let prior = "{\"verdict\":\"request-changes\",\"reviewed_sha\":\"aaaaaaa\"}\n";
    std::fs::write(&path, prior).unwrap();

    let rec = record("approved", "2026-10-29T12:00:00Z", None);
    rv::write_verdict_object(&path, &rec).unwrap();
    rv::write_verdict_object(&path, &rec).unwrap();

    let events = store::read_file(&root, change);
    assert_eq!(kinds(&events), ["legacy-imported", "verdict"]);
    let EventBody::LegacyImported { legacy } = &events[0].body else {
        panic!("the first event is the legacy import");
    };
    assert_eq!(
        legacy.iter().map(|f| f.body.as_str()).collect::<Vec<_>>(),
        [prior],
        "the import holds the file as it was before the write, not the new record"
    );
    assert_eq!(events[1].verdict_record(), Some(&rec));
}

#[test]
fn the_ledger_answers_a_read_only_where_the_file_knows_less() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(14);
    let (_guard, a) = granted_reviewer(&root);
    // A contained reviewer's verdict that never reached the file.
    let first = ts(1);
    let rec = record("approved", &first, Some(&a));
    record_event(&root, verdict(change, None, rec), true).unwrap();
    let got = rv::read_recorded_verdict(&root, "PR-14").unwrap();
    assert_eq!(got.kind, rv::VerdictKind::Approved);
    assert_eq!(
        rv::read_verdict_for_sha(&root, "PR-14", HEAD).unwrap().kind,
        rv::VerdictKind::Approved
    );

    // The same round on file, annotated by a later close: the file wins a tie.
    let mut closed = record("approved", &first, Some(&a));
    closed.insert("closed_by_merge".into(), "cccccccc".into());
    let file = rv::verdict_path(&root, "PR-14");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, serde_json::to_string(&closed).unwrap()).unwrap();
    assert!(rv::read_recorded_verdict(&root, "PR-14")
        .unwrap()
        .is_closed());

    // An older file round loses to a newer ledger round.
    let older = record("approved", &ts(-60), Some(&a));
    std::fs::write(&file, serde_json::to_string(&older).unwrap()).unwrap();
    record_event(
        &root,
        verdict(change, None, record("request-changes", &ts(2), Some(&a))),
        true,
    )
    .unwrap();
    assert_eq!(
        rv::read_recorded_verdict(&root, "PR-14").unwrap().kind,
        rv::VerdictKind::RequestChanges
    );
}

/// Finding 1: ingest applies the ship gate's authority checks. A contained
/// implementer that claims the reviewer seat — with no grant, or with a
/// grant that is not a reviewer grant — is recorded as a `rejected-event`,
/// never as a verdict.
#[test]
fn a_forged_reviewer_seat_submission_is_rejected() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(18);
    let _guard = crate::test_env::AmbientGuard::hermetic(&root, None);
    let implementer_grant = crate::gate::seat_authority::test_support::mint_grant_for(
        &root,
        crate::test_env::HERMETIC_TEST_USER,
        "implementer",
        &[],
    );
    let binding = crate::gate::seat_authority::read_grant(&implementer_grant)
        .unwrap()
        .binding;
    let no_grant = RecorderAttestation {
        grant_id: Some("00000000-0000-4000-8000-000000000000".into()),
        ..attestation("reviewer")
    };
    let wrong_seat = RecorderAttestation {
        grant_id: Some(implementer_grant),
        grant_binding: binding,
        ..attestation("reviewer")
    };
    for forged in [no_grant, wrong_seat] {
        spool::submit(
            &root,
            &verdict(change, None, record("approved", &ts(1), Some(&forged))),
        )
        .unwrap();
    }
    assert_eq!(project::newest_verdict(&root, "PR-18"), None);
    let events = store::read_file(&root, change);
    assert_eq!(
        kinds(&events),
        ["legacy-imported", "rejected-event", "rejected-event"]
    );
    assert!(events.iter().all(|e| match &e.body {
        EventBody::RejectedEvent { reason, .. } => reason.contains("did not re-validate"),
        _ => true,
    }));
}

/// Finding 2: a submission dated in the future is refused on ingest, and a
/// ledger verdict is ordered by primary-side time, so it cannot shadow a
/// later file verdict.
#[test]
fn a_future_dated_submission_cannot_shadow_a_later_file_verdict() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(19);
    let (_guard, a) = granted_reviewer(&root);
    spool::submit(
        &root,
        &verdict(change, None, record("approved", &ts(86_400), Some(&a))),
    )
    .unwrap();
    let file = rv::verdict_path(&root, "PR-19");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let later = record("request-changes", &ts(5), Some(&a));
    std::fs::write(&file, serde_json::to_string(&later).unwrap()).unwrap();

    assert_eq!(
        rv::read_recorded_verdict(&root, "PR-19").unwrap().kind,
        rv::VerdictKind::RequestChanges
    );
    let events = store::read_file(&root, change);
    assert_eq!(kinds(&events), ["legacy-imported", "rejected-event"]);

    // Even a future-dated record on the ledger counts only as of its `at`.
    let mut on_ledger = verdict(change, None, record("approved", &ts(86_400), Some(&a)));
    on_ledger.at = ts(0);
    let legacy = rv::parse_ledger_record(&later);
    assert_eq!(
        rv::newer_of(legacy, Some(&on_ledger)).unwrap().kind,
        rv::VerdictKind::RequestChanges
    );
}

/// Finding 3: a spool file that outlived its ingest is removed on the next
/// drain even with nothing new to append, and never re-appended.
#[test]
fn a_surviving_spool_file_is_removed_without_a_duplicate_event() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(20);
    let (_guard, a) = granted_reviewer(&root);
    let rec = record("approved", &ts(1), Some(&a));
    let Delivered::Spooled(file) =
        record_event(&root, verdict(change, None, rec.clone()), true).unwrap()
    else {
        panic!("a contained writer submits");
    };
    let body = std::fs::read(&file).unwrap();
    project::events(&root, change);
    // The removal failed: the same submission is still in the spool.
    std::fs::write(&file, &body).unwrap();
    assert!(spool::has_pending(&root, change));
    project::events(&root, change);
    assert!(!file.exists(), "consumed on the empty-pending path too");
    assert_eq!(
        record_event(&root, verdict(change, None, rec), false).unwrap(),
        Delivered::Unchanged
    );
    assert_eq!(
        kinds(&store::read_file(&root, change)),
        ["legacy-imported", "verdict"]
    );
}

/// Finding 4: a submitter's patch identity never reaches the ledger; it is
/// recomputed primary-side (here, outside a repository: none).
#[test]
fn a_submitted_patch_identity_is_recomputed_on_ingest() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(21);
    let (_guard, a) = granted_reviewer(&root);
    let mut event = verdict(change, None, record("approved", &ts(1), Some(&a)));
    event.patch_id = Some("forged".into());
    event.base_ref = Some("forged".into());
    event.base_tip = Some("forged".into());
    spool::submit(&root, &event).unwrap();
    let events = project::events(&root, change);
    assert_eq!(kinds(&events), ["legacy-imported", "verdict"]);
    assert_eq!(
        (
            &events[1].patch_id,
            &events[1].base_ref,
            &events[1].base_tip
        ),
        (&None, &None, &None)
    );
}

#[test]
fn a_pr_keyed_verdict_write_lands_on_the_ledger_and_a_spec_keyed_one_does_not() {
    // An uncontained writer, as the primary side runs it.
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let (_tmp, root) = root();
    let a = attestation("reviewer");
    let rec = record("approved", "2026-10-29T10:00:00Z", Some(&a));
    rv::write_verdict_object_for_spec(&rv::verdict_path(&root, "PR-15"), &rec, Some("STORY-7"))
        .unwrap();
    rv::write_verdict_object(&rv::verdict_path(&root, "STORY-8"), &rec).unwrap();
    assert_eq!(project::newest_verdict(&root, "PR-15"), Some(rec.clone()));
    assert_eq!(project::newest_verdict(&root, "STORY-7"), Some(rec));
    assert_eq!(project::newest_verdict(&root, "STORY-8"), None);
}

fn git(repo: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_linked_worktree_writes_the_primary_checkouts_ledger() {
    let (_tmp, base) = root();
    let primary = base.join("aida");
    std::fs::create_dir_all(&primary).unwrap();
    git(&primary, &["init", "--initial-branch=main", "--quiet"]);
    git(&primary, &["config", "user.email", "t@example.com"]);
    git(&primary, &["config", "user.name", "Test"]);
    std::fs::write(primary.join("README.md"), "init\n").unwrap();
    git(&primary, &["add", "."]);
    git(&primary, &["commit", "-q", "-m", "chore: init"]);
    let linked = base.join("aida-pr-16");
    git(
        &primary,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "pr-16",
            linked.to_str().unwrap(),
        ],
    );

    assert_eq!(store_root(&linked).unwrap(), primary);
    // Granted, so a contained write's submission is accepted on ingest.
    let (_guard, a) = granted_reviewer(&primary);
    let rec = record("approved", &ts(1), Some(&a));
    rv::write_verdict_object(&rv::verdict_path(&linked, "PR-16"), &rec).unwrap();
    // Contained or not, the event reaches the primary ledger (direct, or
    // through the primary mailbox and the next primary-side read).
    assert_eq!(project::newest_verdict(&primary, "PR-16"), Some(rec));
    assert!(store::ledger_file(&primary, ChangeKey::pr(16)).exists());
    assert!(
        !ledger_dir(&linked).exists(),
        "never a worktree-local ledger"
    );
}

fn non_test_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "tests" {
                    walk(&path, out);
                }
            } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files
}

/// AC6 ratchet (finding 5): non-test code may not add calls to the legacy
/// gate-state writers, which build their paths through helpers and so add
/// no directory literal. Counted per writer, definitions excluded. Slice 2
/// takes the verdict writers to 0 (advisor ruling); slice 3 the rest.
#[test]
fn legacy_gate_writers_do_not_gain_call_sites() {
    const BASELINE: &[(&str, usize)] = &[
        // Verdict files.
        ("write_verdict_atomic", 10),
        ("write_verdict_object", 3),
        ("write_verdict_object_for_spec", 3),
        ("record_verdict", 2),
        ("record_verdict_at_path", 2),
        ("adopt_direct_write", 7),
        ("collect_standalone_verdict", 1),
        ("close_verdict_on_merge", 3),
        ("stamp_pr_review_verdict", 3),
        // Merge holds and their clearances.
        ("write_hold", 4),
        ("write_typed_hold", 15),
        ("write_typed_hold_context", 3),
        ("write_marker", 9),
        ("write_marker_with_route", 1),
        ("place_hand_hold", 1),
        ("record_clearance", 2),
        ("record_clearance_with_verdict", 3),
        ("record_clearance_by", 2),
        ("clear_hold", 6),
        // Carry-over proofs.
        ("record_carry_over", 1),
    ];
    let sources: Vec<String> = non_test_sources()
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    for (writer, baseline) in BASELINE {
        let call = regex::Regex::new(&format!(r"(^|[^A-Za-z0-9_])(fn\s+)?{writer}\s*\(")).unwrap();
        let found: usize = sources
            .iter()
            .flat_map(|src| call.captures_iter(src))
            .filter(|c| c.get(2).is_none())
            .count();
        assert!(
            found <= *baseline,
            "{found} non-test call sites of {writer} (baseline {baseline}): new gate state goes on the gate ledger (gate::ledger), not a legacy file writer"
        );
    }
}

/// AC6 ratchet: non-test code may not grow its references to the legacy
/// gate-state directories. Each slice lowers a baseline; slice 3 ends at 0.
/// `gate/ledger/legacy.rs` is the one sanctioned reader.
#[test]
fn legacy_gate_paths_do_not_gain_new_references() {
    const BASELINE: &[(&str, usize)] = &[
        ("review-verdicts", 32),
        ("merge-holds", 3),
        ("merge-hold-clearances", 2),
        ("review-carryover", 1),
        ("review-in-progress", 6),
    ];
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let files = non_test_sources();
    let sanctioned = src.join("gate").join("ledger").join("legacy.rs");
    for (dir, baseline) in BASELINE {
        let needle = format!("\"{dir}\"");
        let found: usize = files
            .iter()
            .filter(|p| **p != sanctioned)
            .map(|p| std::fs::read_to_string(p).unwrap().matches(&needle).count())
            .sum();
        assert!(
            found <= *baseline,
            "{found} references to {needle} in non-test code (baseline {baseline}): new gate state goes on the gate ledger (gate::ledger), not the legacy directory"
        );
    }
}
