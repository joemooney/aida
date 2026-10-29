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

#[test]
fn a_pr_keyed_verdict_write_lands_on_the_ledger_and_a_spec_keyed_one_does_not() {
    // An uncontained writer, as the primary side runs it.
    let _env = uncontained();
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

    let mut closed = record("approved", &first, Some(&a));
    closed.insert("closed_by_merge".into(), "cccccccc".into());
    let file = rv::verdict_path(&root, "PR-14");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, serde_json::to_string(&closed).unwrap()).unwrap();
    assert!(rv::read_recorded_verdict(&root, "PR-14")
        .unwrap()
        .is_closed());

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
        rv::newer_of(rv::parse_ledger_record(&later), Some(&on_ledger))
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
        crate::gate::rework_hold_release::RetainedHistory::with_ledger(None, &root, "PR-43")
            .unwrap();
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

/// AC6 ratchet (finding 5): non-test code may not add calls to the legacy
/// gate-state writers, which build their paths through helpers and so add
/// no directory literal. Counted per writer, definitions excluded. Slice 2
/// takes the verdict writers to 0 (advisor ruling); slice 3 the rest.
/// `collect_standalone_verdict` writes the ledger only and is not listed;
/// `adopt_direct_write`'s archive is counted as `archive_round_by_sha`.
#[test]
fn legacy_gate_writers_do_not_gain_call_sites() {
    const BASELINE: &[(&str, usize)] = &[
        // Verdict files.
        ("write_verdict_atomic", 8),
        ("write_verdict_object", 3),
        ("write_verdict_object_for_spec", 3),
        ("record_verdict", 2),
        ("record_verdict_at_path", 2),
        ("close_verdict_on_merge", 3),
        ("stamp_pr_review_verdict", 3),
        ("archive_round_by_sha", 2),
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
    let sources = non_test_sources();
    for (writer, baseline) in BASELINE {
        let found: usize = sources.iter().map(|(_, src)| call_sites(src, writer)).sum();
        assert!(
            found <= *baseline,
            "{found} non-test call sites of {writer} (baseline {baseline}): new gate state goes on the gate ledger (gate::ledger), not a legacy file writer"
        );
    }
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
        ("merge-holds", 3),
        ("merge-hold-clearances", 2),
        ("review-carryover", 1),
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
