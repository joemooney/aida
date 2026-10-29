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
    let a = attestation("reviewer");
    let rec = record("approved", "2026-10-29T10:00:00Z", Some(&a));
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

    let before = project::verdict_history(&root, change);
    assert_eq!(before.len(), 2, "read through before any ledger write");
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
    let after = project::verdict_history(&root, change);
    assert_eq!(after[..2], before[..], "no legacy round is lost");
    assert_eq!(after.len(), 3);
    assert!(
        current.exists() && archive.join("aaaaaaa.json").exists(),
        "legacy files are never deleted"
    );
}

#[test]
fn the_ledger_answers_a_read_only_where_the_file_knows_less() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(14);
    let a = attestation("reviewer");
    // A contained reviewer's verdict that never reached the file.
    let rec = record("approved", "2026-10-29T10:00:00Z", Some(&a));
    record_event(&root, verdict(change, None, rec), true).unwrap();
    let got = rv::read_recorded_verdict(&root, "PR-14").unwrap();
    assert_eq!(got.kind, rv::VerdictKind::Approved);
    assert_eq!(
        rv::read_verdict_for_sha(&root, "PR-14", HEAD).unwrap().kind,
        rv::VerdictKind::Approved
    );

    // The same round on file, annotated by a later close: the file wins a tie.
    let mut closed = record("approved", "2026-10-29T10:00:00Z", Some(&a));
    closed.insert("closed_by_merge".into(), "cccccccc".into());
    let file = rv::verdict_path(&root, "PR-14");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, serde_json::to_string(&closed).unwrap()).unwrap();
    assert!(rv::read_recorded_verdict(&root, "PR-14")
        .unwrap()
        .is_closed());

    // A strictly newer ledger round wins over an older file.
    record_event(
        &root,
        verdict(
            change,
            None,
            record("request-changes", "2026-10-29T11:00:00Z", Some(&a)),
        ),
        true,
    )
    .unwrap();
    assert_eq!(
        rv::read_recorded_verdict(&root, "PR-14").unwrap().kind,
        rv::VerdictKind::RequestChanges
    );
}

#[test]
fn a_pr_keyed_verdict_write_lands_on_the_ledger_and_a_spec_keyed_one_does_not() {
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
    // Attested, so a contained write's submission is accepted on ingest.
    let rec = record(
        "approved",
        "2026-10-29T10:00:00Z",
        Some(&attestation("reviewer")),
    );
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
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
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
