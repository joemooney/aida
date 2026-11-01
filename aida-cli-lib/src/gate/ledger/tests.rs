//! M1 slice 1: the gate ledger — append/lock/seq, the change as the only
//! key, uncontained writers only, the launcher's collection as a contained
//! seat's one route to the ledger, legacy read-through, and the legacy-path
//! ratchets.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate
use super::event::{EventBody, GateEvent};
use super::*;
use crate::gate::review_verdict::{self as rv, JsonObj, RecorderAttestation, ATTESTATION_KEY};

const HEAD: &str = "1111111111111111111111111111111111111111";
const OTHER_HEAD: &str = "2222222222222222222222222222222222222222";

fn root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    (tmp, root)
}

/// An uncontained process, as the primary side runs, whatever the suite's
/// own shell is.
fn uncontained() -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)])
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
/// guard holds the env lock (and an uncontained env); keep it alive for the
/// test.
fn granted_reviewer(root: &Path) -> (crate::test_env::AmbientGuard, RecorderAttestation) {
    let guard =
        crate::test_env::AmbientGuard::hermetic_and(root, None, &[("AIDA_CONTAINED", None)]);
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
            EventBody::Hold { .. } => "hold",
            EventBody::HoldRelease { .. } => "hold-release",
            EventBody::ReleaseAuthorized { .. } => "release-authorized",
            EventBody::ReleaseWithdrawn { .. } => "release-withdrawn",
            EventBody::LabelSync { .. } => "label-sync",
            EventBody::CarryOverProof { .. } => "carry-over-proof",
            EventBody::CarryOverRefused { .. } => "carry-over-refused",
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
    let _env = uncontained();
    let (_tmp, root) = root();
    let change = ChangeKey::pr(9);
    let a = attestation("reviewer");
    let rec = record("approved", "2026-10-29T10:00:00Z", Some(&a));
    assert_eq!(
        record_event(&root, verdict(change, Some("STORY-42"), rec.clone())).unwrap(),
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
    let _env = uncontained();
    let (_tmp, root) = root();
    let change = ChangeKey::pr(10);
    let rec = record("approved", "2026-10-29T10:00:00Z", None);
    assert_eq!(
        record_event(&root, verdict(change, None, rec.clone())).unwrap(),
        Delivered::Appended
    );
    assert_eq!(
        record_event(&root, verdict(change, None, rec)).unwrap(),
        Delivered::Unchanged
    );
    assert_eq!(
        kinds(&store::read_file(&root, change)),
        ["legacy-imported", "verdict"]
    );
}

#[test]
fn legacy_files_are_read_through_and_imported_verbatim_on_the_first_write() {
    let _env = uncontained();
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
    let _env = uncontained();
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

/// M1 slice 2 (brief item 3): a spec-only verdict goes onto the ledger too,
/// on the spec's unbound entry while no change is indexed for it; slice 1
/// left it in the legacy file only.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn a_pr_keyed_verdict_write_lands_on_the_change_and_a_spec_keyed_one_on_the_spec() {
    // An uncontained writer, as the primary side runs it.
    let _env = uncontained();
    let (_tmp, root) = root();
    let a = attestation("reviewer");
    let rec = record("approved", "2026-10-29T10:00:00Z", Some(&a));
    rv::write_verdict_object_for_spec(&rv::verdict_path(&root, "PR-15"), &rec, Some("STORY-7"))
        .unwrap();
    rv::write_verdict_object(&rv::verdict_path(&root, "STORY-8"), &rec).unwrap();
    assert_eq!(project::newest_verdict(&root, "PR-15"), Some(rec.clone()));
    assert_eq!(project::newest_verdict(&root, "STORY-7"), Some(rec.clone()));
    assert_eq!(project::newest_verdict(&root, "STORY-8"), Some(rec));
    assert!(
        !rv::verdict_path(&root, "STORY-8").exists(),
        "no legacy file is written for a spec-only verdict"
    );
    assert!(store::unbound_file(&root, "STORY-8").is_file());
}

/// STORY-080 AC2 (slice 3): a spec-only verdict recorded before any change
/// existed binds to the change when it opens. It then reads under the change
/// and, once, under the spec; the unbound ledger keeps its line (binding
/// never deletes evidence); a later spec record lands on the change after
/// it; and binding twice adds nothing.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn a_spec_only_verdict_binds_to_the_change_when_it_opens() {
    let _env = uncontained();
    let (_tmp, root) = root();
    let a = attestation("reviewer");
    let early = record("request-changes", "2026-10-29T09:00:00Z", Some(&a));
    rv::write_verdict_object(&rv::verdict_path(&root, "STORY-9"), &early).unwrap();
    assert_eq!(project::newest_verdict(&root, "PR-21"), None);

    super::bind_specs(&root, ChangeKey::pr(21), &["story-9".to_string()]);
    assert_eq!(project::newest_verdict(&root, "PR-21"), Some(early.clone()));
    assert_eq!(
        project::changes_for_spec(&root, "STORY-9"),
        vec![ChangeKey::pr(21)]
    );
    assert_eq!(
        project::verdict_records(&root, "STORY-9"),
        vec![early.clone()],
        "the bound record reads once under the spec"
    );
    assert_eq!(store::read_unbound(&root, "STORY-9").len(), 1);

    super::bind_specs(&root, ChangeKey::pr(21), &["STORY-9".to_string()]);
    let bound = store::read_file(&root, ChangeKey::pr(21))
        .into_iter()
        .filter(|e| e.verdict_record().is_some())
        .count();
    assert_eq!(bound, 1, "binding again adds nothing");

    let later = record("approved", "2026-10-29T11:00:00Z", Some(&a));
    rv::write_verdict_object(&rv::verdict_path(&root, "STORY-9"), &later).unwrap();
    assert_eq!(project::newest_verdict(&root, "PR-21"), Some(later.clone()));
    assert_eq!(project::newest_verdict(&root, "STORY-9"), Some(later));
}

/// STORY-080 AC2 (round-2 review finding 2): a pre-PR spec verdict binds to
/// the first change opened for the spec only. A second change for the same
/// spec does not inherit it: the spec reads the record once, and the second
/// change answers with nothing until a verdict is recorded on it.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn a_bound_spec_verdict_does_not_bind_again_to_a_second_change() {
    let _env = uncontained();
    let (_tmp, root) = root();
    let a = attestation("reviewer");
    let early = record("approved", "2026-10-29T09:00:00Z", Some(&a));
    rv::write_verdict_object(&rv::verdict_path(&root, "STORY-9"), &early).unwrap();

    super::bind_specs(&root, ChangeKey::pr(21), &["STORY-9".to_string()]);
    super::bind_specs(&root, ChangeKey::pr(22), &["STORY-9".to_string()]);

    assert_eq!(project::newest_verdict(&root, "PR-21"), Some(early.clone()));
    assert_eq!(
        project::newest_verdict(&root, "PR-22"),
        None,
        "PR-22 does not answer with a verdict that predates it and belongs to PR-21"
    );
    assert_eq!(
        project::verdict_records(&root, "STORY-9"),
        vec![early],
        "the spec reads the record once, not once per change"
    );

    let later = record("request-changes", "2026-10-29T11:00:00Z", Some(&a));
    rv::write_verdict_object_for_spec(&rv::verdict_path(&root, "PR-22"), &later, Some("STORY-9"))
        .unwrap();
    assert_eq!(project::newest_verdict(&root, "PR-22"), Some(later.clone()));
    assert_eq!(project::verdict_records(&root, "STORY-9").len(), 2);
}

/// STORY-080 AC2 (round-2 review finding 3): every PR-open site goes through
/// [`super::open_change_bound`], which binds the credited specs' pre-PR
/// verdicts to the opened change. Driven with a stub forge.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn opening_a_change_through_the_hook_binds_the_credited_specs() {
    let opens = |id| {
        let mut forge = crate::forge::fake::RecordingForge::new();
        forge.opens_as = Some(id);
        forge
    };
    let _env = uncontained();
    let (_tmp, root) = root();
    let a = attestation("reviewer");
    let early = record("approved", "2026-10-29T09:00:00Z", Some(&a));
    rv::write_verdict_object(&rv::verdict_path(&root, "STORY-9"), &early).unwrap();
    let req = crate::forge::OpenChange {
        branch: "feat".into(),
        base: "main".into(),
        title: "t".into(),
        body: String::new(),
        draft: false,
    };

    let change =
        super::open_change_bound(&opens(31), &root, req.clone(), &["STORY-9".to_string()]).unwrap();
    assert_eq!(change.id, 31);
    assert_eq!(project::newest_verdict(&root, "PR-31"), Some(early));

    super::open_change_bound(&opens(0), &root, req, &["STORY-9".to_string()]).unwrap();
    assert_eq!(
        project::changes_for_spec(&root, "STORY-9"),
        vec![ChangeKey::pr(31)],
        "an open with no change number binds nothing"
    );
}

/// STORY-080 AC2 ratchet: production code opens a change only through
/// [`super::open_change_bound`], so no PR-open path (ship, the orchestrator,
/// the drain cluster, the train, queue recover) skips the binding.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn every_pr_open_site_binds_through_the_hook() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "tests") {
                    stack.push(path);
                }
                continue;
            }
            let rel = path
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if path.extension().is_none_or(|e| e != "rs")
                || rel.ends_with("tests.rs")
                || rel == "gate/ledger/mod.rs"
            {
                continue;
            }
            let body = std::fs::read_to_string(&path).unwrap();
            // Unit-test modules sit at the end of their file.
            let live = body.split("#[cfg(test)]\nmod ").next().unwrap_or("");
            if live.contains(".open_change(") {
                offenders.push(rel);
            }
        }
    }
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "these files open a change without `gate::ledger::open_change_bound`, so pre-PR \
         spec verdicts never bind to it: {offenders:?}"
    );
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

/// Round 2: only uncontained primary-side code writes the ledger. A
/// contained seat's direct record is refused, and its verdict write leaves
/// the ledger untouched (the file stays for its launcher to collect).
#[test]
fn a_contained_writer_cannot_write_the_ledger() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);
    let (_tmp, root) = root();
    let change = ChangeKey::pr(11);
    let rec = record(
        "approved",
        "2026-10-29T10:00:00Z",
        Some(&attestation("reviewer")),
    );
    let err = record_event(&root, verdict(change, None, rec.clone())).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    let path = rv::verdict_path(&root, "PR-11");
    rv::write_verdict_object(&path, &rec).unwrap();
    rv::adopt_direct_write(&path).unwrap();
    assert!(path.exists(), "the seat's own file is still written");
    assert!(!store::ledger_file(&root, change).exists());
    assert!(
        !ledger_dir(&root).exists(),
        "nothing under the ledger at all"
    );
}

/// A verdict that only the ledger holds (collected from a contained
/// reviewer) answers reads; a newer file round wins over an older ledger one
/// and the same round on file (a later annotation) wins a tie.
#[test]
fn the_ledger_answers_a_read_only_where_the_file_knows_less() {
    let (_tmp, root) = root();
    let change = ChangeKey::pr(14);
    let (_guard, a) = granted_reviewer(&root);
    let first = ts(1);
    record_event(
        &root,
        verdict(change, None, record("approved", &first, Some(&a))),
    )
    .unwrap();
    assert_eq!(
        rv::read_recorded_verdict(&root, "PR-14").unwrap().kind,
        rv::VerdictKind::Approved
    );
    assert_eq!(
        rv::read_verdict_for_sha(&root, "PR-14", HEAD).unwrap().kind,
        rv::VerdictKind::Approved
    );

    // Slice 2: the merge-close annotation is a ledger event of the same
    // round, and the same round on both sides is the ledger's.
    let mut closed = record("approved", &first, Some(&a));
    closed.insert("closed_by_merge".into(), "cccccccc".into());
    record_event(&root, verdict(change, None, closed)).unwrap();
    assert!(rv::read_recorded_verdict(&root, "PR-14")
        .unwrap()
        .is_closed());
    let file = rv::verdict_path(&root, "PR-14");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        serde_json::to_string(&record("approved", &first, Some(&a))).unwrap(),
    )
    .unwrap();
    assert!(
        rv::read_recorded_verdict(&root, "PR-14")
            .unwrap()
            .is_closed(),
        "a same-round legacy file does not outrank the ledger's annotation"
    );

    let older = record("approved", &ts(-60), Some(&a));
    std::fs::write(&file, serde_json::to_string(&older).unwrap()).unwrap();
    record_event(
        &root,
        verdict(change, None, record("request-changes", &ts(2), Some(&a))),
    )
    .unwrap();
    assert_eq!(
        rv::read_recorded_verdict(&root, "PR-14").unwrap().kind,
        rv::VerdictKind::RequestChanges
    );
}

/// A ledger verdict counts as of the ledger's own `at`, so a future-dated
/// record cannot shadow a later file verdict.
#[test]
fn a_future_dated_ledger_record_cannot_shadow_a_later_file_verdict() {
    let later = record("request-changes", &ts(5), None);
    let mut on_ledger = verdict(
        ChangeKey::pr(19),
        None,
        record("approved", &ts(86_400), None),
    );
    on_ledger.at = ts(0);
    assert_eq!(
        rv::newer_of(
            Path::new("/nonexistent/PR-19.json"),
            rv::parse_ledger_record(&later),
            Some(&on_ledger)
        )
        .unwrap()
        .kind,
        rv::VerdictKind::RequestChanges
    );
}

/// The launcher's collection checks, as `aida review launch` builds them.
fn checks<'a>(
    root: &'a Path,
    pr: u64,
    child: &str,
) -> crate::queue_cmd::StandaloneVerdictChecks<'a> {
    crate::queue_cmd::StandaloneVerdictChecks {
        project_root: root,
        pr,
        head_sha: Some(HEAD.to_string()),
        launcher_grant: None,
        child_grant: Some(child.to_string()),
    }
}

/// A launcher checkout and a reviewer worktree with one fresh verdict file.
fn reviewer_wrote(worktree: &Path, name: &str, rec: &JsonObj) {
    let dir = worktree.join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{name}.json")),
        serde_json::to_string(rec).unwrap(),
    )
    .unwrap();
}

fn since() -> std::time::SystemTime {
    std::time::SystemTime::now() - std::time::Duration::from_secs(5)
}

/// Round 2, item 2: collection is a contained reviewer's one route to the
/// ledger. It appends the verdict (and the credited spec's record) from the
/// launcher's side, writes no legacy file, and every gate reader sees it.
#[test]
fn a_collected_verdict_goes_on_the_ledger_and_no_legacy_file_is_written() {
    let (_tmp, root) = root();
    let launcher = root.join("primary");
    let worktree = root.join("seat");
    std::fs::create_dir_all(&launcher).unwrap();
    let (_guard, a) = granted_reviewer(&launcher);
    let rec = record("approved", &ts(1), Some(&a));
    reviewer_wrote(&worktree, "STORY-1", &rec);
    let canonical = rv::verdict_path(&launcher, "PR-30");
    let child = a.grant_id.clone().unwrap();

    let got = rv::collect_standalone_verdict(
        &launcher,
        &worktree,
        &canonical,
        &["STORY-1".to_string()],
        since(),
        &checks(&launcher, 30, &child),
    )
    .unwrap();
    assert!(got.collected_from_worktree);
    assert_eq!(got.spec_records, ["STORY-1"]);
    assert!(!canonical.exists(), "no PR-keyed legacy file");
    assert!(
        !rv::verdict_path(&launcher, "STORY-1").exists(),
        "no spec-keyed one"
    );
    assert_eq!(
        project::newest_verdict(&launcher, "PR-30"),
        Some(rec.clone())
    );
    assert_eq!(project::newest_verdict(&launcher, "STORY-1"), Some(rec));
    assert_eq!(
        rv::read_recorded_verdict(&launcher, "PR-30").unwrap().kind,
        rv::VerdictKind::Approved
    );
    let candidates = crate::gate::ship::merge_gate_verdict_candidates(
        &[launcher.as_path()],
        30,
        &["STORY-1".to_string()],
        Some(HEAD),
    );
    assert!(candidates
        .iter()
        .any(|v| v.kind == rv::VerdictKind::Approved));
    let bodies =
        crate::gate::merge_hold_core::corpus_gate_bodies(&[launcher.as_path()], 30, &[]).unwrap();
    assert_eq!(
        bodies.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        ["PR-30"]
    );

    // Collecting again appends nothing new.
    let before = store::read_file(&launcher, ChangeKey::pr(30)).len();
    rv::collect_standalone_verdict(
        &launcher,
        &worktree,
        &canonical,
        &["STORY-1".to_string()],
        since(),
        &checks(&launcher, 30, &child),
    )
    .unwrap();
    assert_eq!(store::read_file(&launcher, ChangeKey::pr(30)).len(), before);
}

/// BUG-HOME-045 AC ace2889c: a CONTAINED reviewer records from its own
/// review worktree under its OWN child grant, and the launcher collects it.
/// The contained launch issuer gives that child a session-keyring key (the
/// only binding a sandboxed `aida review record` can present), and a verdict
/// attested by exactly that grant goes on the PR's gate ledger.
// trace:BUG-b6b1278890774ef993d2ebc327862470-045.ace2889c | ai:claude
// trace:TASK-b6b1278890774ef993d2ebc327862470-144 | ai:claude
#[test]
fn a_contained_reviewers_own_grant_verdict_is_collected_from_its_worktree() {
    if !crate::grant_keyring::available() {
        eprintln!("SKIP: the session keyring is unavailable here");
        return;
    }
    let (_tmp, root) = root();
    let launcher = root.join("primary");
    let worktree = root.join("seat");
    std::fs::create_dir_all(&launcher).unwrap();
    let _guard = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &launcher,
        "advisor",
        &["reviewer"],
        &[("AIDA_CONTAINED", None)],
    );
    let child = crate::gate::seat_authority::issue_launch_child(
        &launcher,
        "reviewer",
        crate::test_env::HERMETIC_TEST_USER,
        true,
    )
    .expect("a contained reviewer child is minted while the keyring holds its key");
    assert!(
        child.binding.iter().any(|b| !b.starts_with("proc:")),
        "a contained child carries a key its sandbox can present: {:?}",
        child.binding
    );
    let own = RecorderAttestation {
        grant_id: Some(child.id.clone()),
        grant_binding: child.binding.clone(),
        ..attestation("reviewer")
    };
    let rec = record("approved", &ts(1), Some(&own));
    reviewer_wrote(&worktree, "STORY-1", &rec);
    let canonical = rv::verdict_path(&launcher, "PR-31");

    let got = rv::collect_standalone_verdict(
        &launcher,
        &worktree,
        &canonical,
        &["STORY-1".to_string()],
        since(),
        &checks(&launcher, 31, &child.id),
    )
    .expect("the contained reviewer's own-grant verdict is collected");
    assert!(got.collected_from_worktree);
    assert_eq!(
        rv::read_recorded_verdict(&launcher, "PR-31").unwrap().kind,
        rv::VerdictKind::Approved
    );
}

/// Round 2, item 2: a verdict for PR-B is never collected from PR-A's
/// reviewer seat — neither a record naming PR-B, nor one reviewed at PR-B's
/// head, nor a `PR-B.json` file in the seat.
#[test]
fn a_verdict_for_another_pr_is_refused_at_collection() {
    let (_tmp, root) = root();
    let launcher = root.join("primary");
    std::fs::create_dir_all(&launcher).unwrap();
    let (_guard, a) = granted_reviewer(&launcher);
    let child = a.grant_id.clone().unwrap();
    let canonical = rv::verdict_path(&launcher, "PR-30");

    let mut names_b = record("approved", &ts(1), Some(&a));
    names_b.insert("pr".into(), serde_json::json!(31));
    let mut at_b_head = record("approved", &ts(1), Some(&a));
    at_b_head.insert("reviewed_sha".into(), OTHER_HEAD.into());
    for (n, (name, rec, why)) in [
        ("STORY-1", names_b, "not PR-30"),
        ("STORY-1", at_b_head, "head is now"),
        (
            "PR-31",
            record("approved", &ts(1), Some(&a)),
            "wrote no verdict",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let worktree = root.join(format!("seat-{n}"));
        reviewer_wrote(&worktree, name, &rec);
        let err = rv::collect_standalone_verdict(
            &launcher,
            &worktree,
            &canonical,
            &["STORY-1".to_string()],
            since(),
            &checks(&launcher, 30, &child),
        )
        .unwrap_err();
        assert!(err.contains(why), "{name}: {err}");
    }
    assert!(!store::ledger_file(&launcher, ChangeKey::pr(30)).exists());
    assert!(!store::ledger_file(&launcher, ChangeKey::pr(31)).exists());
}

/// Round 2, item 2: the recording grant must be valid when the launcher
/// collects, not only when the reviewer recorded: a grant revoked in
/// between collects nothing, and neither does another grant than this
/// launch's child grant.
#[test]
fn a_revoked_or_foreign_grant_is_refused_at_collection() {
    let (_tmp, root) = root();
    let launcher = root.join("primary");
    let worktree = root.join("seat");
    std::fs::create_dir_all(&launcher).unwrap();
    let (_guard, a) = granted_reviewer(&launcher);
    let child = a.grant_id.clone().unwrap();
    reviewer_wrote(&worktree, "STORY-1", &record("approved", &ts(1), Some(&a)));
    let canonical = rv::verdict_path(&launcher, "PR-30");
    let collect = |child: &str| {
        rv::collect_standalone_verdict(
            &launcher,
            &worktree,
            &canonical,
            &["STORY-1".to_string()],
            since(),
            &checks(&launcher, 30, child),
        )
    };

    let foreign = collect("00000000-0000-4000-8000-000000000000").unwrap_err();
    assert!(
        foreign.contains("not this launch's reviewer grant"),
        "{foreign}"
    );
    assert!(crate::gate::seat_authority::revoke_by_id(&child).unwrap());
    let revoked = collect(&child).unwrap_err();
    assert!(revoked.contains("is not valid"), "{revoked}");
    assert!(!store::ledger_file(&launcher, ChangeKey::pr(30)).exists());
}

/// A heredoc-written PR-keyed verdict is adopted onto the ledger once,
/// however often adopted (beside the slice-1 archive dual write).
#[test]
fn a_direct_write_is_adopted_onto_the_ledger_once() {
    let _env = uncontained();
    let (_tmp, root) = root();
    let path = rv::verdict_path(&root, "PR-32");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let rec = record("approved", "2026-10-29T10:00:00Z", None);
    std::fs::write(&path, serde_json::to_string(&rec).unwrap()).unwrap();
    rv::adopt_direct_write(&path).unwrap();
    rv::adopt_direct_write(&path).unwrap();
    assert_eq!(
        kinds(&store::read_file(&root, ChangeKey::pr(32))),
        ["legacy-imported", "verdict"]
    );
}

/// Round 3, finding 5: a grant that expired between the review and the
/// collection collects nothing, like a revoked one.
#[test]
fn an_expired_grant_is_refused_at_collection() {
    let (_tmp, root) = root();
    let launcher = root.join("primary");
    let worktree = root.join("seat");
    std::fs::create_dir_all(&launcher).unwrap();
    let (_guard, a) = granted_reviewer(&launcher);
    let child = a.grant_id.clone().unwrap();
    reviewer_wrote(&worktree, "STORY-1", &record("approved", &ts(1), Some(&a)));
    let mut grant = crate::gate::seat_authority::read_grant(&child).unwrap();
    grant.expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
    std::fs::write(
        crate::gate::seat_authority::grant_dir()
            .unwrap()
            .join(format!("{child}.json")),
        serde_json::to_vec_pretty(&grant).unwrap(),
    )
    .unwrap();
    let canonical = rv::verdict_path(&launcher, "PR-30");

    let expired = rv::collect_standalone_verdict(
        &launcher,
        &worktree,
        &canonical,
        &["STORY-1".to_string()],
        since(),
        &checks(&launcher, 30, &child),
    )
    .unwrap_err();
    assert!(expired.contains("is not valid"), "{expired}");
    assert!(!store::ledger_file(&launcher, ChangeKey::pr(30)).exists());
}

/// A legacy file at `key` holding `rec`.
fn file_holds(root: &Path, key: &str, rec: &JsonObj) {
    let path = rv::verdict_path(root, key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_string(rec).unwrap()).unwrap();
}

/// A ledger-only verdict for `change`, as collection leaves it.
fn ledger_holds(root: &Path, change: ChangeKey, rec: JsonObj) {
    store::append(root, change, vec![verdict(change, None, rec)]).unwrap();
}

/// Round 3, finding 1: a same-head re-review collected onto the ledger
/// (request-changes) outranks the previous round's approval still in the
/// file, at both merge gates; an older ledger record does not outrank a
/// newer file.
#[test]
fn a_newer_ledger_verdict_outranks_the_file_at_both_merge_gates() {
    let _env = uncontained();
    let (_tmp, root) = root();
    let change = ChangeKey::pr(40);
    file_holds(&root, "PR-40", &record("approved", &ts(-3600), None));
    ledger_holds(&root, change, record("request-changes", &ts(-60), None));

    let candidates =
        crate::gate::ship::merge_gate_verdict_candidates(&[&root], 40, &[], Some(HEAD));
    let kinds: Vec<_> = candidates.iter().map(|v| v.kind).collect();
    assert_eq!(kinds, [rv::VerdictKind::RequestChanges], "{candidates:?}");

    let bodies = crate::gate::merge_hold_core::corpus_gate_bodies(&[&root], 40, &[]).unwrap();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].1.contains("request-changes"), "{bodies:?}");
    assert!(!bodies[0].1.contains("\"approved\""), "{bodies:?}");

    // The file re-recorded after the ledger round answers again.
    file_holds(&root, "PR-40", &record("approved", &ts(0), None));
    let candidates =
        crate::gate::ship::merge_gate_verdict_candidates(&[&root], 40, &[], Some(HEAD));
    let kinds: Vec<_> = candidates.iter().map(|v| v.kind).collect();
    assert_eq!(kinds, [rv::VerdictKind::Approved], "{candidates:?}");
    let bodies = crate::gate::merge_hold_core::corpus_gate_bodies(&[&root], 40, &[]).unwrap();
    assert!(bodies[0].1.contains("\"approved\""), "{bodies:?}");
}

/// Round 3, finding 1: a ledger record is weighed once, even when two roots
/// sharing the ledger each hold a file it outranks.
#[test]
fn a_ledger_verdict_counts_once_across_roots() {
    let _env = uncontained();
    let (_tmp, root) = root();
    let other = root.join("wt");
    std::fs::create_dir_all(&other).unwrap();
    file_holds(&root, "PR-41", &record("approved", &ts(-3600), None));
    ledger_holds(
        &root,
        ChangeKey::pr(41),
        record("request-changes", &ts(-60), None),
    );
    let bodies =
        crate::gate::merge_hold_core::corpus_gate_bodies(&[&root, &root], 41, &[]).unwrap();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
}

/// Round 3, findings 2-4: single-key readers (launcher summary, calibration,
/// awaiting) get the newer of file and ledger; an unparseable file is kept
/// as the blocking evidence it is.
#[test]
fn single_key_readers_get_the_newer_of_file_and_ledger() {
    let _env = uncontained();
    let (_tmp, root) = root();
    let path = rv::verdict_path(&root, "PR-42");
    assert_eq!(rv::current_verdict_body_at(&path), None);

    ledger_holds(
        &root,
        ChangeKey::pr(42),
        record("request-changes", &ts(-60), None),
    );
    let only_ledger = rv::current_verdict_body_at(&path).unwrap();
    assert!(only_ledger.contains("request-changes"), "{only_ledger}");

    file_holds(&root, "PR-42", &record("approved", &ts(-3600), None));
    let stale_file = rv::current_verdict_body_at(&path).unwrap();
    assert!(stale_file.contains("request-changes"), "{stale_file}");

    file_holds(&root, "PR-42", &record("approved", &ts(0), None));
    let newer_file = rv::current_verdict_body_at(&path).unwrap();
    assert!(newer_file.contains("\"approved\""), "{newer_file}");

    std::fs::write(&path, "{not json").unwrap();
    assert_eq!(
        rv::current_verdict_body_at(&path).as_deref(),
        Some("{not json")
    );
}

/// Round 3, finding 4 (rework auto-release): the ledger's rounds join the
/// retained history, so a ledger-only arming refusal is found with no file.
#[test]
fn retained_history_includes_the_ledger_rounds() {
    let _env = uncontained();
    let (_tmp, root) = root();
    ledger_holds(
        &root,
        ChangeKey::pr(43),
        record("request-changes", &ts(-120), None),
    );
    ledger_holds(&root, ChangeKey::pr(43), record("approved", &ts(-60), None));
    let history =
        crate::gate::rework_hold_release::RetainedHistory::projected(&root, "PR-43").unwrap();
    let verdicts: Vec<_> = history
        .archived
        .iter()
        .map(|r| r["verdict"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(verdicts, ["request-changes", "approved"]);
    assert!(
        history.location.contains("gate ledger"),
        "{}",
        history.location
    );
}

/// Round 4, finding 1: `aida review verdict <KEY> --json` prints the record
/// the gates weigh and names its source: the ledger's record when the key is
/// ledger-only or the file is a previous round, the file when it is newer.
#[test]
fn review_verdict_json_prints_the_selected_record_and_its_source() {
    let _env = uncontained();
    let (_tmp, root) = root();
    let show = |root: &Path| {
        crate::review_verdict_show_body(root, "PR-44", |_| rv::TipRelation::AtReviewedSha)
            .map(|(_, value, _)| value)
    };
    assert!(show(&root).is_none());

    let mut collected = record("request-changes", &ts(-60), None);
    collected.insert("summary".into(), "collected round".into());
    ledger_holds(&root, ChangeKey::pr(44), collected);
    let only_ledger = show(&root).unwrap();
    assert_eq!(only_ledger["verdict"], "request-changes", "{only_ledger}");
    assert_eq!(only_ledger["summary"], "collected round");
    assert_eq!(only_ledger["record_source"]["kind"], "gate-ledger");
    assert!(only_ledger["record_source"]["location"]
        .as_str()
        .unwrap()
        .contains("gate-ledger"));
    assert!(only_ledger["actionability"].is_string());

    file_holds(&root, "PR-44", &record("approved", &ts(-3600), None));
    let stale_file = show(&root).unwrap();
    assert_eq!(stale_file["verdict"], "request-changes", "{stale_file}");
    assert_eq!(stale_file["record_source"]["kind"], "gate-ledger");

    file_holds(&root, "PR-44", &record("approved", &ts(0), None));
    let newer_file = show(&root).unwrap();
    assert_eq!(newer_file["verdict"], "approved", "{newer_file}");
    assert_eq!(newer_file["record_source"]["kind"], "file");
}

/// Round 4, finding 3 (regression): a contained review's spec record is
/// collected onto the ledger only; its non-blocking findings are still read
/// for filing as successors when the spec completes.
#[test]
fn a_ledger_only_spec_record_still_files_its_findings_on_completion() {
    let _env = uncontained();
    let (_tmp, root) = root();
    assert!(crate::findings_gate::completion_verdict(&root, "STORY-45").is_none());
    let mut rec = record("approved", &ts(-60), None);
    rec.insert(
        "findings".into(),
        serde_json::json!(["[minor] rename the helper"]),
    );
    store::append(
        &root,
        ChangeKey::pr(45),
        vec![verdict(ChangeKey::pr(45), Some("STORY-45"), rec)],
    )
    .unwrap();
    assert!(!rv::verdict_path(&root, "STORY-45").exists());
    let v = crate::findings_gate::completion_verdict(&root, "STORY-45")
        .expect("the ledger's spec record is read");
    let todo =
        crate::findings_gate::findings_needing_a_successor(&v, &std::collections::HashSet::new());
    assert_eq!(todo.len(), 1, "{todo:?}");
    assert_eq!(todo[0].0, "[minor] rename the helper");
}

/// The PR and spec records `aida human` weighs for PR-46 / STORY-46.
fn human_approved(root: &Path) -> bool {
    let req = aida_core::Requirement::new("t".into(), "d".into());
    crate::human::spec_review_approved(root, &req, "STORY-46", 46)
}

/// Round 4, finding 4: where the ledger's record supersedes the file, the
/// file is not a candidate at all, so a stale request-changes file with a
/// later mtime cannot block a newer ledger approval; a newer file still
/// answers.
#[test]
fn awaiting_you_drops_a_file_the_ledger_supersedes() {
    let _env = uncontained();
    let (_tmp, root) = root();
    ledger_holds(&root, ChangeKey::pr(46), record("approved", &ts(-60), None));
    // Written after the ledger record (later mtime), but an older round.
    file_holds(&root, "PR-46", &record("request-changes", &ts(-3600), None));
    assert!(
        !human_approved(&root),
        "ledger parsing alone is not positive authority"
    );

    file_holds(&root, "PR-46", &record("request-changes", &ts(0), None));
    assert!(!human_approved(&root));
}

/// Round 4, finding 2: directory-scanning readers walk the ledger's keys
/// too, and the phase-3 handshake reads a collected verdict written during
/// the review but not an earlier round left on the ledger.
#[test]
fn scanning_and_handshake_readers_see_ledger_only_verdicts() {
    let _env = uncontained();
    let (_tmp, root) = root();
    store::append(
        &root,
        ChangeKey::pr(47),
        vec![
            verdict(
                ChangeKey::pr(47),
                None,
                record("request-changes", &ts(-120), None),
            ),
            verdict(
                ChangeKey::pr(47),
                Some("STORY-47"),
                record("request-changes", &ts(-120), None),
            ),
        ],
    )
    .unwrap();
    let listed: Vec<_> = rv::list_active_verdicts(&root)
        .into_iter()
        .map(|(k, v)| (k, v.kind))
        .collect();
    assert_eq!(
        listed,
        [
            ("PR-47".to_string(), rv::VerdictKind::RequestChanges),
            ("STORY-47".to_string(), rv::VerdictKind::RequestChanges)
        ]
    );

    let path = rv::verdict_path(&root, "PR-47");
    let before = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
    let after = std::time::SystemTime::now() + std::time::Duration::from_secs(600);
    assert!(matches!(
        crate::phase_gates::read_verdict_file_since(&path, Some(before)),
        Ok(crate::auto_complete::ReviewerOutcome::Verdict(
            crate::auto_complete::Verdict::RequestChanges
        ))
    ));
    assert!(
        crate::phase_gates::read_verdict_file_since(&path, Some(after)).is_err(),
        "a ledger round older than the review session is not its answer"
    );
    assert!(crate::human::pr_has_local_verdict(&root, 47));
    assert!(crate::db_reconcile::read_review_verdict_for_record(&root, "STORY-47").is_some());
}

/// Round 5, finding 1: `aida review <SPEC>` clears the spec's verdict file
/// before it launches the reviewer, but not the ledger. A ledger record for
/// the spec from before the launch (an earlier round, or another PR's review
/// of this spec) is not this review's verdict and must not be re-recorded at
/// the current head; a verdict the reviewer writes after the launch is.
#[test]
fn review_spec_ignores_a_ledger_verdict_from_before_the_launch() {
    let _env = uncontained();
    let (_tmp, root) = root();
    store::append(
        &root,
        ChangeKey::pr(48),
        vec![verdict(
            ChangeKey::pr(48),
            Some("STORY-48"),
            record("approved", &ts(-120), None),
        )],
    )
    .unwrap();
    let path = rv::verdict_path(&root, "STORY-48");
    let launched_at = std::time::SystemTime::now() - std::time::Duration::from_secs(30);
    assert!(
        crate::review_spec_verdict(&path, launched_at).is_none(),
        "an earlier round on the ledger is not this review's verdict"
    );
    let earlier_launch = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
    assert_eq!(
        crate::review_spec_verdict(&path, earlier_launch).map(|v| v.verdict),
        Some("approved".to_string()),
        "a ledger record written after the launch is the review's answer"
    );

    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_string(&record("request-changes", &ts(0), None)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        crate::review_spec_verdict(&path, launched_at).map(|v| v.verdict),
        Some("request-changes".to_string()),
        "the verdict the reviewer wrote after the launch answers"
    );
}

/// Round 5, finding 2: a current verdict file the anchored reader refuses
/// (TASK-1801) is not evidence, so the phase-3 archive fallback stays closed
/// even when the ledger holds a record that would otherwise let it answer.
#[cfg(unix)]
#[test]
fn a_refused_current_file_keeps_the_phase3_archive_fallback_closed() {
    let _env = uncontained();
    let (_tmp, root) = root();
    let path = rv::verdict_path(&root, "PR-49");
    let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    // Two rounds as an older binary left them: the current file at
    // OTHER_HEAD and a per-commit archive entry for each head (M1 slice 2
    // writes neither; the phase-3 fallback reads them through).
    let archive = rv::verdict_archive_dir(&path).unwrap();
    std::fs::create_dir_all(&archive).unwrap();
    for sha in [HEAD, OTHER_HEAD] {
        let body = format!(
            r#"{{"verdict":"approved","reviewed_sha":"{sha}","recorded_by":"reviewer","recorded_at":"{}"}}"#,
            chrono::Utc::now().to_rfc3339()
        );
        std::fs::write(archive.join(format!("{sha}.json")), &body).unwrap();
        std::fs::write(&path, &body).unwrap();
    }
    // The ledger's current round is at OTHER_HEAD, like the file's.
    let mut rec = record("approved", &ts(0), None);
    rec.insert(
        "reviewed_sha".to_string(),
        serde_json::Value::String(OTHER_HEAD.to_string()),
    );
    let mut event = verdict(ChangeKey::pr(49), None, rec);
    event.head = Some(OTHER_HEAD.to_string());
    store::append(&root, ChangeKey::pr(49), vec![event]).unwrap();
    assert!(
        crate::phase_gates::phase3_head_archive_fallback(&path, Some(HEAD), started).is_some(),
        "precondition: the archived round answers for HEAD while the file is readable"
    );
    let real = root.join("elsewhere.json");
    std::fs::rename(&path, &real).unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();
    assert!(rv::verdict_file_refused(&path));
    assert!(
        crate::phase_gates::phase3_head_archive_fallback(&path, Some(HEAD), started).is_none(),
        "a refused current file must not let the archive answer"
    );
}

/// Round 8, finding 1: a current file that is refused (a link, TASK-1801) or
/// does not parse keeps the ledger out of the single-key readers, so neither
/// `aida pr ship`'s refusal release nor the dispatch-flow fast path takes a
/// ledger approval in its place.
#[cfg(unix)]
#[test]
fn a_refused_or_unparseable_file_keeps_the_ledger_out_of_every_reader() {
    use crate::gate::merge_hold_core::{typed_hold, HoldReasonKind, RefusalRelease};
    let _env = uncontained();
    let (_tmp, root) = root();
    ledger_holds(&root, ChangeKey::pr(50), record("approved", &ts(-60), None));
    let hold = typed_hold(50, HoldReasonKind::Rework, "CHANGES REQUESTED", None);
    let approved_by_every_reader = || {
        let release = crate::pr_cmd::ship_refusal_release(&[&root], &hold, Some(HEAD));
        let dispatch = crate::gate::dispatch_authority::verdict_at_head(&root, 50, HEAD);
        (
            rv::read_recorded_verdict(&root, "PR-50").is_some(),
            rv::read_verdict_for_sha(&root, "PR-50", HEAD).is_some(),
            matches!(release, RefusalRelease::Released(_)),
            dispatch == Some(rv::VerdictKind::Approved),
        )
    };
    assert_eq!(
        approved_by_every_reader(),
        (true, true, true, false),
        "legacy readers still see the ledger artifact; dispatch needs durable authority"
    );

    let path = rv::verdict_path(&root, "PR-50");
    let real = root.join("elsewhere.json");
    std::fs::write(
        &real,
        serde_json::to_string(&record("approved", &ts(0), None)).unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();
    assert!(rv::verdict_file_refused(&path));
    assert_eq!(approved_by_every_reader(), (false, false, false, false));

    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "{not json").unwrap();
    assert_eq!(approved_by_every_reader(), (false, false, false, false));
}

/// Round 8, finding 2: a file with no `recorded_at` is dated by its mtime
/// at both merge gates and in `newer_of`, so an older ledger approval does
/// not replace a newer hand-written request-changes file; a ledger round
/// newer than the file's mtime still answers.
#[test]
fn an_undated_file_is_dated_by_its_mtime_at_both_merge_gates() {
    let _env = uncontained();
    let (_tmp, root) = root();
    ledger_holds(
        &root,
        ChangeKey::pr(51),
        record("approved", &ts(-3600), None),
    );
    let mut undated = record("request-changes", &ts(0), None);
    undated.remove("recorded_at");
    file_holds(&root, "PR-51", &undated);
    let path = rv::verdict_path(&root, "PR-51");

    let gates = || {
        let candidates =
            crate::gate::ship::merge_gate_verdict_candidates(&[&root], 51, &[], Some(HEAD));
        let bodies = crate::gate::merge_hold_core::corpus_gate_bodies(&[&root], 51, &[]).unwrap();
        assert_eq!(candidates.len(), 1, "{candidates:?}");
        assert_eq!(bodies.len(), 1, "{bodies:?}");
        (
            candidates[0].kind,
            bodies[0].1.contains("request-changes"),
            rv::read_recorded_verdict(&root, "PR-51").unwrap().kind,
        )
    };
    assert_eq!(
        gates(),
        (
            rv::VerdictKind::RequestChanges,
            true,
            rv::VerdictKind::RequestChanges
        ),
        "a newer undated file keeps the older ledger approval out"
    );

    let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(two_hours_ago)
        .unwrap();
    assert_eq!(
        gates(),
        (rv::VerdictKind::Approved, false, rv::VerdictKind::Approved),
        "a ledger round newer than the undated file's mtime answers"
    );
}

/// Round 3 ruling: no reader opens a legacy verdict file directly. Non-test
/// calls of the file-reading primitives are allowed in the storage owner
/// (`gate/review_verdict.rs`: the read-through itself, the writers' own
/// read-modify-write, collection from a seat clone) and the ledger's legacy
/// importer; anywhere else only as listed, each with its reason. Every other
/// reader goes through `current_verdict` / `read_recorded_verdict` /
/// `ledger::project`.
#[test]
fn legacy_verdict_files_are_read_only_through_the_ledger_projection() {
    const PRIMITIVES: &[&str] = &[
        "read_verdict_body",
        "archived_recordings",
        "archived_verdict_file_for_sha",
        "verdict_file_for_sha",
        "verdict_file_modified",
    ];
    const SANCTIONED: &[&str] = &["gate/review_verdict.rs", "gate/ledger/legacy.rs"];
    const ALLOWED: &[(&str, &str, usize, &str)] = &[
        (
            "gate/ship.rs",
            "read_verdict_body",
            1,
            "merge-gate corpus: weighs each file against the ledger (ledger_supersedes)",
        ),
        (
            "gate/merge_hold_core.rs",
            "read_verdict_body",
            1,
            "merge-chokepoint corpus: weighs each file against the ledger (ledger_supersedes)",
        ),
        (
            "gate/rework_hold_release.rs",
            "read_verdict_body",
            1,
            "an unreadable arming file must fail closed; the body then goes through current_verdict_body",
        ),
        (
            "gate/rework_hold_release.rs",
            "archived_recordings",
            1,
            "retained history: the archive plus the ledger's rounds (RetainedHistory::with_ledger)",
        ),
        (
            "review_classes.rs",
            "read_verdict_body",
            1,
            "finding-class corpus over every legacy round; the ledger's records are added beside them",
        ),
        (
            "phase_gates.rs",
            "read_verdict_body",
            2,
            "BUG-809 sibling sweep: evidence files in other checkouts, reconciled before publishing",
        ),
        (
            "phase_gates.rs",
            "verdict_file_modified",
            2,
            "freshness of a sibling checkout's file and of a per-commit archive",
        ),
        (
            "phase_gates.rs",
            "archived_verdict_file_for_sha",
            1,
            "phase-3 per-commit fallback; the ledger's round at the head answers when there is no archive",
        ),
    ];
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found: Vec<(String, &str, usize)> = Vec::new();
    for (path, body) in non_test_sources() {
        let rel = path
            .strip_prefix(&src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if SANCTIONED.contains(&rel.as_str()) {
            continue;
        }
        let body = without_test_modules(body);
        for primitive in PRIMITIVES {
            let n = call_sites(&body, primitive);
            if n > 0 {
                found.push((rel.clone(), primitive, n));
            }
        }
    }
    for (file, primitive, n) in &found {
        let allowed = ALLOWED
            .iter()
            .find(|(f, p, _, _)| f == file && p == primitive)
            .map_or(0, |(_, _, n, _)| *n);
        assert!(
            *n <= allowed,
            "{file} calls {primitive} {n} time(s) (allowed {allowed}): read verdicts through current_verdict / read_recorded_verdict / gate::ledger::project, not the legacy file"
        );
    }
}

/// `src` with every `#[cfg(test)] mod … { … }` block removed (braces
/// counted naively), so inline test modules do not count as callers.
fn without_test_modules(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(at) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "#[cfg(test)]".len()..];
        let head = after.trim_start();
        let is_mod = head.starts_with("mod ")
            || head.starts_with("pub mod ")
            || head.starts_with("pub(crate) mod ")
            || head.starts_with("#[");
        let open = after.find(['{', ';']);
        match open {
            Some(i) if is_mod && after.as_bytes()[i] == b'{' => {
                let mut depth = 0usize;
                let mut end = after.len();
                for (j, b) in after.bytes().enumerate().skip(i) {
                    match b {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = j + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                rest = &after[end..];
            }
            _ => {
                out.push_str("#[cfg(test)]");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn test_modules_are_stripped_but_code_after_them_is_kept() {
    let src = "fn a() { read_verdict_body(x); }\n#[cfg(test)]\nmod t { fn b() { read_verdict_body(y); } }\nfn c() { read_verdict_body(z); }\n#[cfg(test)]\nuse foo;\n";
    let kept = without_test_modules(src);
    assert_eq!(call_sites(&kept, "read_verdict_body"), 2, "{kept}");
    assert!(kept.contains("use foo;"));
}

/// Every non-test source file under `src/` with its body, read once per
/// test process and shared by both ratchets (they scan ~1,000 files).
fn non_test_sources() -> &'static [(PathBuf, String)] {
    static SOURCES: std::sync::OnceLock<Vec<(PathBuf, String)>> = std::sync::OnceLock::new();
    SOURCES.get_or_init(|| {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if path.is_dir() {
                    if name != "tests" {
                        walk(&path, out);
                    }
                } else if name.ends_with(".rs")
                    && name != "tests.rs"
                    && !name.ends_with("_tests.rs")
                {
                    let body = std::fs::read_to_string(&path).unwrap();
                    out.push((path, body));
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        files
    })
}

/// Calls of `name(` in `src` — the name preceded by a non-identifier byte
/// and followed by optional whitespace and `(` — that are not its
/// definition (`fn name(`).
fn call_sites(src: &str, name: &str) -> usize {
    let bytes = src.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    src.match_indices(name)
        .filter(|(at, _)| {
            if *at > 0 && ident(bytes[at - 1]) {
                return false;
            }
            let after = src[at + name.len()..].trim_start();
            if !after.starts_with('(') {
                return false;
            }
            let is_definition = src[..*at]
                .trim_end()
                .strip_suffix("fn")
                .is_some_and(|before| !before.bytes().last().is_some_and(ident));
            !is_definition
        })
        .count()
}

/// AC6 ratchet (finding 5), M1 slice 2: every legacy verdict and hold file
/// writer is gone, so each baseline is 0 (advisor ruling: the story cannot
/// complete with dual writes). The record funnels — `record_verdict`,
/// `record_verdict_at_path`, `write_verdict_object(_for_spec)`,
/// `close_verdict_on_merge`, `stamp_pr_review_verdict`, `place_hold`,
/// `release_hold*` — now append the gate ledger only and are not legacy
/// writers ([`verdict_and_hold_writers_write_no_legacy_file`] proves it).
/// Inline `#[cfg(test)]` modules are stripped before counting.
///
/// `write_verdict_atomic` is the one atomic file publisher left, and only at
/// the sites below, each with its reason. Slice 3 moved carry-over proofs
/// onto the ledger, so `record_carry_over` is gone too.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn legacy_gate_writers_do_not_gain_call_sites() {
    const GONE: &[&str] = &[
        // Verdict files and their per-commit archive.
        "write_verdict_file",
        "archive_round_by_sha",
        "prepare_archive_round",
        "migrate_sidecars_to_archive",
        "ensure_sidecars_archived",
        // Merge-hold markers and their clearances.
        "write_typed_hold",
        "write_typed_hold_context",
        "write_typed_hold_replacing",
        "write_marker_with_route",
        "write_marker_files",
        "place_hand_hold",
        "record_clearance",
        "record_clearance_with_verdict",
        "record_clearance_by",
        "record_clearance_full",
        "record_mandate_clearance",
        "clear_hold",
        "supersede_clearance",
        "local_hold_path",
        "local_clearance_path",
        "superseded_clearance_path",
        // Carry-over proof records (slice 3: `carry-over-proof` events).
        "record_carry_over",
    ];
    const ATOMIC_ALLOWED: &[(&str, usize, &str)] = &[(
        "gate/review_verdict.rs",
        2,
        "a contained seat's hand-off file in its own seat clone (the launcher collects it), \
             and the operator's one-shot `aida review backfill-shas` over legacy files",
    )];
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let stripped: Vec<(String, String)> = non_test_sources()
        .iter()
        .map(|(p, body)| {
            let rel = p
                .strip_prefix(&src_root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/");
            (rel, without_test_modules(body))
        })
        .collect();
    for writer in GONE {
        let found: Vec<&str> = stripped
            .iter()
            .filter(|(_, src)| call_sites(src, writer) > 0)
            .map(|(p, _)| p.as_str())
            .collect();
        assert!(
            found.is_empty(),
            "non-test calls of the legacy writer {writer} in {found:?}: gate state goes on the gate ledger (gate::ledger)"
        );
    }
    for (file, src) in &stripped {
        let n = call_sites(src, "write_verdict_atomic");
        let allowed = ATOMIC_ALLOWED
            .iter()
            .find(|(f, _, _)| f == file)
            .map_or(0, |(_, n, _)| *n);
        assert!(
            n <= allowed,
            "{n} non-test calls of write_verdict_atomic in {file} (allowed {allowed}): verdicts go on the gate ledger"
        );
    }
}

/// The record funnels and every hold writer leave no legacy file behind:
/// recording, closing, placing, label-syncing and releasing all land on the
/// ledger, and `.aida/review-verdicts/` and `.aida/merge-holds/` stay absent.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn verdict_and_hold_writers_write_no_legacy_file() {
    use crate::gate::merge_hold_core as mh;
    let _env = uncontained();
    let (_tmp, root) = root();
    rv::record_verdict(
        &root,
        "PR-61",
        Some("request-changes"),
        Some("a1b2c3d4e5f6a7b8c9d0a1b2c3d4e5f6a7b8c9d0"),
        None,
        Some("fix it"),
        &["one".to_string()],
        "reviewer-x",
    )
    .unwrap();
    rv::record_verdict(
        &root,
        "STORY-61",
        Some("approved"),
        None,
        None,
        None,
        &[],
        "r",
    )
    .unwrap();
    assert!(rv::close_verdict_on_merge(&root, "PR-61", "deadbeef").unwrap());
    let hold = mh::typed_hold(61, mh::HoldReasonKind::Supervision, "held", None);
    mh::place_hold(&root, &hold).unwrap();
    mh::record_label_state(&root, 61, &mh::LabelState::Synced).unwrap();
    mh::release_hold(&root, &hold, &mh::PrincipalIdentity::human("joe"), None).unwrap();
    for legacy in ["review-verdicts", "merge-holds", "merge-hold-clearances"] {
        assert!(
            !root.join(".aida").join(legacy).exists(),
            "{legacy} must not be written"
        );
    }
    let closed = rv::read_recorded_verdict(&root, "PR-61").unwrap();
    assert_eq!(closed.closed_by_merge.as_deref(), Some("deadbeef"));
    assert!(rv::read_recorded_verdict(&root, "STORY-61").is_some());
}

#[test]
fn call_sites_skip_definitions_and_longer_names() {
    let src = "fn write_hold(x) {}\nwrite_hold (1); a.write_hold(2); my_write_hold(3); write_holds(4); write_hold_x(5);";
    assert_eq!(call_sites(src, "write_hold"), 2);
}

/// AC6 ratchet: non-test code may not grow its references to the legacy
/// gate-state directories. Each slice lowers a baseline; slice 3 ends at 0.
/// `gate/ledger/legacy.rs` is the one sanctioned reader.
#[test]
fn legacy_gate_paths_do_not_gain_new_references() {
    const BASELINE: &[(&str, usize)] = &[
        ("review-verdicts", 32),
        ("merge-holds", 0),
        ("merge-hold-clearances", 0),
        ("review-carryover", 0),
        ("review-in-progress", 6),
    ];
    let sanctioned = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("gate")
        .join("ledger")
        .join("legacy.rs");
    for (dir, baseline) in BASELINE {
        let needle = format!("\"{dir}\"");
        let found: usize = non_test_sources()
            .iter()
            .filter(|(p, _)| *p != sanctioned)
            .map(|(_, src)| src.matches(&needle).count())
            .sum();
        assert!(
            found <= *baseline,
            "{found} references to {needle} in non-test code (baseline {baseline}): new gate state goes on the gate ledger (gate::ledger), not the legacy directory"
        );
    }
}

// ---- M1 slice 2: holds on the ledger (AC3) ----

/// AC3: a release appends a `hold-release` event; the `hold` it answers
/// stays on the ledger, so "a hold existed and was cleared" is evidence,
/// not an absence.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn a_release_appends_and_keeps_the_hold_as_evidence() {
    use crate::gate::merge_hold_core as mh;
    let _env = uncontained();
    let (_tmp, root) = root();
    let hold = mh::typed_hold(71, mh::HoldReasonKind::Supervision, "supervised", None);
    mh::place_hold(&root, &hold).unwrap();
    assert!(mh::hold_is_open(&root, 71));
    mh::release_hold(&root, &hold, &mh::PrincipalIdentity::human("joe"), None).unwrap();
    assert!(mh::read_hold_record(&root, 71).is_none());
    let events = store::read_file(&root, ChangeKey::pr(71));
    assert_eq!(
        kinds(&events),
        ["legacy-imported", "hold", "hold-release"],
        "the release is appended after the hold it answers"
    );
    let cleared = mh::human_recorded_clearance(&root, 71).expect("a human release");
    assert_eq!(cleared.cleared_by, "human:joe");
    // A new placement closes the release: it no longer answers.
    mh::place_hold(&root, &hold).unwrap();
    assert!(mh::recorded_clearance(&root, 71).is_none());
    assert!(mh::human_recorded_clearance(&root, 71).is_none());
}

/// AC3: the ship-held mandate's "a hold existed and was cleared" and the
/// awaiting surface read the ledger alone; no marker or clearance file is
/// involved at any point.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn a_cleared_hold_is_read_from_the_ledger_without_a_marker() {
    use crate::gate::merge_hold_core as mh;
    let _env = uncontained();
    let (_tmp, root) = root();
    let hold = mh::typed_hold(72, mh::HoldReasonKind::Supervision, "supervised", None);
    mh::place_hold(&root, &hold).unwrap();
    assert_eq!(mh::list_holds(&root), vec![(72, "supervised".to_string())]);
    mh::release_hold(&root, &hold, &mh::PrincipalIdentity::human("joe"), None).unwrap();
    assert!(mh::list_holds(&root).is_empty());
    assert!(mh::human_recorded_clearance(&root, 72).is_some());
    assert!(mh::unrecorded_marker_removals(&root).is_empty());
    assert!(!root.join(".aida/merge-holds").exists());
    assert!(!root.join(".aida/merge-hold-clearances").exists());
}

/// AC6 for holds: a legacy marker is read through while the ledger has no
/// placement or release for the PR; an unreadable one still holds
/// (TASK-1238); a marker older than the ledger's last release does not
/// reopen it, and a newer one (an older binary placed it) does.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn legacy_markers_are_read_through_until_the_ledger_answers() {
    use crate::gate::merge_hold_core as mh;
    let _env = uncontained();
    let (_tmp, root) = root();
    let marker = legacy::hold_marker_path(&root, 73);
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, "legacy supervised hold\n").unwrap();
    let legacy_hold = mh::read_hold_record(&root, 73).expect("read through");
    assert!(legacy_hold.legacy);

    // Releasing imports the marker-held hold first, then releases it.
    mh::release_hold(
        &root,
        &legacy_hold,
        &mh::PrincipalIdentity::human("joe"),
        None,
    )
    .unwrap();
    assert!(
        mh::read_hold_record(&root, 73).is_none(),
        "the old marker no longer holds"
    );
    let events = store::read_file(&root, ChangeKey::pr(73));
    assert_eq!(kinds(&events), ["legacy-imported", "hold", "hold-release"]);
    let EventBody::LegacyImported { legacy } = &events[0].body else {
        panic!("first event is the import");
    };
    assert!(
        legacy.iter().any(|f| f.body == "legacy supervised hold\n"),
        "the marker body is preserved verbatim"
    );

    // An older binary writes a marker after the release: it holds again.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&marker, "re-placed by an older binary\n").unwrap();
    assert_eq!(
        mh::read_hold(&root, 73).as_deref(),
        Some("re-placed by an older binary")
    );

    // An unreadable marker on a PR with no ledger events holds (fail closed).
    #[cfg(unix)]
    {
        let dir_marker = legacy::hold_marker_path(&root, 74);
        std::fs::create_dir_all(&dir_marker).unwrap();
        let held = mh::read_hold_record(&root, 74).expect("unreadable marker holds");
        assert_eq!(held.reason_kind, mh::HoldReasonKind::Unknown);
    }
}

/// A label sync and an auto-release authorization are recorded as events
/// on the open hold; a withdrawn authorization no longer counts.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn label_sync_and_release_authorization_are_ledger_events() {
    use crate::gate::merge_hold_core as mh;
    let _env = uncontained();
    let (_tmp, root) = root();
    let hold = mh::typed_hold(75, mh::HoldReasonKind::Rework, "changes requested", None);
    mh::place_hold(&root, &hold).unwrap();
    mh::record_label_state(&root, 75, &mh::LabelState::Synced).unwrap();
    assert_eq!(mh::read_label_state(&root, 75), mh::LabelState::Synced);
    let clearance = mh::clearance_for(&hold, "auto-release", None, None).unwrap();
    hold::append(
        &root,
        75,
        EventBody::ReleaseAuthorized { clearance },
        None,
        None,
    )
    .unwrap();
    assert!(hold::state(&root, 75).authorized.is_some());
    hold::append(
        &root,
        75,
        EventBody::ReleaseWithdrawn {
            reason: "head moved".into(),
        },
        None,
        None,
    )
    .unwrap();
    let st = hold::state(&root, 75);
    assert!(st.authorized.is_none());
    assert!(
        st.open.is_some(),
        "a withdrawn authorization leaves the hold armed"
    );
}

/// A worktree marker collected onto the ledger is no longer a legacy marker:
/// an untyped one is recorded as the typed Rework hold it stood for, so the
/// ship gate and rework auto-release treat it like any reviewer refusal. A
/// typed marker is collected as written.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn a_collected_untyped_marker_becomes_a_typed_rework_hold() {
    use crate::gate::merge_hold_core as mh;
    let _env = uncontained();
    let (_tmp, root) = root();
    let untyped = mh::parse_hold_marker(Ok("changes requested by the reviewer\n".into()), 76)
        .expect("an untyped marker parses");
    assert!(untyped.legacy);
    mh::collect_legacy_hold(&root, &untyped).unwrap();
    let held = mh::read_hold_record(&root, 76).expect("the collected hold is open");
    assert!(
        !held.legacy,
        "a collected hold is a ledger hold, not a legacy one"
    );
    assert_eq!(held.schema_version, 2);
    assert_eq!(held.reason_kind, mh::HoldReasonKind::Rework);
    assert!(held.is_refusal(), "it is still read as a reviewer refusal");
    assert_eq!(held.detail, "changes requested by the reviewer");
    assert!(held.release_condition.is_some());

    let typed = mh::typed_hold(77, mh::HoldReasonKind::Supervision, "supervised", None);
    mh::collect_legacy_hold(&root, &typed).unwrap();
    let held = mh::read_hold_record(&root, 77).expect("open");
    assert_eq!(held.reason_kind, mh::HoldReasonKind::Supervision);
    assert!(!held.legacy);
}

/// PR-3106 round 1, item 2: an uncontained reviewer's `aida review record`
/// writes the ledger and no file, so the launcher collects that ledger record
/// (under the same PR, head and grant checks) and arms the refusal's hold
/// from it. A ledger round from before the launch is not this run's verdict.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn an_uncontained_reviewers_ledger_only_refusal_is_collected_and_holds_the_pr() {
    use crate::gate::merge_hold_core as mh;
    let (_tmp, root) = root();
    let launcher = root.join("primary");
    let worktree = root.join("seat");
    std::fs::create_dir_all(&launcher).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    let (_guard, a) = granted_reviewer(&launcher);
    let canonical = rv::verdict_path(&launcher, "PR-40");
    let child = a.grant_id.clone().unwrap();

    // Before the reviewer writes anything there is nothing to collect.
    let err = rv::collect_standalone_verdict(
        &launcher,
        &worktree,
        &canonical,
        &[],
        since(),
        &checks(&launcher, 40, &child),
    )
    .unwrap_err();
    assert!(err.contains("no verdict"), "{err}");

    let rec = record("request-changes", &ts(1), Some(&a));
    record_verdict_at(&canonical, &rec, None).unwrap();
    assert!(!canonical.exists(), "the reviewer wrote the ledger only");

    // A launch that started after that record does not collect it.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    assert!(rv::collect_standalone_verdict(
        &launcher,
        &worktree,
        &canonical,
        &[],
        later,
        &checks(&launcher, 40, &child),
    )
    .is_err());

    let before = store::read_file(&launcher, ChangeKey::pr(40)).len();
    let got = rv::collect_standalone_verdict(
        &launcher,
        &worktree,
        &canonical,
        &[],
        since(),
        &checks(&launcher, 40, &child),
    )
    .expect("the ledger-only verdict is collected");
    assert_eq!(got.verdict.kind, rv::VerdictKind::RequestChanges);
    assert!(!got.collected_from_worktree);
    assert_eq!(
        store::read_file(&launcher, ChangeKey::pr(40)).len(),
        before,
        "the record is already on the ledger: nothing is appended twice"
    );

    // The same grant check applies: another launch's grant is refused.
    assert!(rv::collect_standalone_verdict(
        &launcher,
        &worktree,
        &canonical,
        &[],
        since(),
        &checks(&launcher, 40, "some-other-grant"),
    )
    .is_err());

    assert!(crate::merge_hold::place_collected_refusal(&launcher, 40, &got).unwrap());
    let held = mh::read_hold_record(&launcher, 40).expect("the refusal holds the PR");
    assert_eq!(held.reason_kind, mh::HoldReasonKind::Rework);
    assert!(held.is_refusal());
}

/// PR-3106 round 1, items 3 and 9: a contained reviewer cannot write hold
/// state, so the launcher arms the refusal's Rework hold from the verdict it
/// collected; the seat's hand-off file does not outlive the collection. An
/// approval arms nothing.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn a_contained_reviewers_refusal_is_armed_by_the_launcher_and_its_hand_off_removed() {
    use crate::gate::merge_hold_core as mh;
    let (_tmp, root) = root();
    let launcher = root.join("primary");
    let worktree = root.join("seat");
    std::fs::create_dir_all(&launcher).unwrap();
    let (_guard, a) = granted_reviewer(&launcher);
    let child = a.grant_id.clone().unwrap();
    let canonical = rv::verdict_path(&launcher, "PR-41");
    let rec = record("request-changes", &ts(1), Some(&a));
    reviewer_wrote(&worktree, "PR-41", &rec);
    let hand_off = worktree.join(".aida/review-verdicts/PR-41.json");

    let got = rv::collect_standalone_verdict(
        &launcher,
        &worktree,
        &canonical,
        &[],
        since(),
        &checks(&launcher, 41, &child),
    )
    .unwrap();
    assert!(got.collected_from_worktree);
    assert!(
        !hand_off.exists(),
        "no verdict state persists in the seat after collection"
    );
    assert!(mh::read_hold_record(&launcher, 41).is_none());
    assert!(crate::merge_hold::place_collected_refusal(&launcher, 41, &got).unwrap());
    let held = mh::read_hold_record(&launcher, 41).expect("armed by the launcher");
    assert_eq!(held.reason_kind, mh::HoldReasonKind::Rework);
    assert!(kinds(&store::read_file(&launcher, ChangeKey::pr(41))).contains(&"hold"));

    let approved = rv::CollectedVerdict {
        verdict: rv::parse_recorded_verdict(
            &serde_json::to_string(&record("approved", &ts(1), Some(&a))).unwrap(),
        )
        .unwrap(),
        spec_records: Vec::new(),
        collected_from_worktree: true,
    };
    assert!(!crate::merge_hold::place_collected_refusal(&launcher, 42, &approved).unwrap());
    assert!(mh::read_hold_record(&launcher, 42).is_none());
}

/// PR-3106 round 1, item 6: `aida queue work --role reviewer` resolves
/// containment from the `[agents] contained` knob and exports
/// `AIDA_CONTAINED=1` for the reviewer it launches. That export must not
/// contain the launcher itself: run from a shell with no `AIDA_CONTAINED`,
/// its collection still writes the ledger. A contained parent stays
/// contained, and a malformed value reads as contained (BUG-2146).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn the_launchers_export_for_its_reviewer_does_not_contain_its_own_collection() {
    let (_tmp, root) = root();
    let launcher = root.join("primary");
    let worktree = root.join("seat");
    std::fs::create_dir_all(&launcher).unwrap();
    let (_guard, a) = granted_reviewer(&launcher);
    let child = a.grant_id.clone().unwrap();
    let canonical = rv::verdict_path(&launcher, "PR-43");
    reviewer_wrote(&worktree, "PR-43", &record("approved", &ts(1), Some(&a)));

    crate::queue_cmd::apply_queue_work_contained_env(true);
    assert!(
        crate::session::aida_contained_env().unwrap(),
        "the reviewer the launcher spawns inherits AIDA_CONTAINED=1"
    );
    assert!(!crate::session::process_contained());
    let got = rv::collect_standalone_verdict(
        &launcher,
        &worktree,
        &canonical,
        &[],
        since(),
        &checks(&launcher, 43, &child),
    );
    crate::queue_cmd::apply_queue_work_contained_env(false);
    assert!(got.is_ok(), "{got:?}");
    assert_eq!(
        project::newest_verdict(&launcher, "PR-43")
            .and_then(|r| r.get("verdict").cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .as_deref(),
        Some("approved")
    );

    // The guard above holds the env lock and restores AIDA_CONTAINED.
    std::env::set_var("AIDA_CONTAINED", "1");
    crate::queue_cmd::apply_queue_work_contained_env(true);
    assert!(crate::session::process_contained(), "a contained parent");
    std::env::set_var("AIDA_CONTAINED", "ture");
    assert!(crate::session::process_contained(), "fail closed");
    let err = record_verdict_at(&canonical, &record("approved", &ts(1), None), None).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    std::env::remove_var("AIDA_CONTAINED");
    crate::queue_cmd::apply_queue_work_contained_env(false);
}

/// PR-3106 round 1, item 5: one spec's records on two changes' ledgers are
/// weighed in the ledger's own order, not the order the files are walked in.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn verdicts_at_a_commit_keep_the_newest_record_per_key_in_ledger_order() {
    let _env = uncontained();
    let (_tmp, root) = root();
    store::append_one(
        &root,
        verdict(
            ChangeKey::pr(51),
            Some("STORY-9"),
            record("approved", &ts(0), None),
        ),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    store::append_one(
        &root,
        verdict(
            ChangeKey::pr(50),
            Some("STORY-9"),
            record("request-changes", &ts(0), None),
        ),
    )
    .unwrap();
    let got = rv::verdicts_for_sha(&root, HEAD);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].kind, rv::VerdictKind::RequestChanges);
}
