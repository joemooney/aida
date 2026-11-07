//! STORY-2325 PR-B: the store, the transitions, the dual-write adapters, the
//! one-time migration and the `--compare` classification.
// trace:STORY-2325 | ai:claude

use super::compare::{compare_one, expected, orphan_outcome, Outcome};
use super::dual::{self, WriterFacts};
use super::migrate::{self, Class, LeaseFacts, OpenPr, TrainLeaseFacts};
use super::ops;
use super::store::{Expect, OwnershipStore, Step};
use super::*;
use crate::gate::canon::SpecKey;
use aida_core::liveness::LeaseState;
use aida_core::process_probe::{ProcessIdentity, INITIAL_PID_NAMESPACE_INODE};
use chrono::{DateTime, TimeZone, Utc};
use std::path::Path;

const BOOT: &str = "11111111-1111-4111-8111-111111111111";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 3, 8, 0, 0).unwrap()
}

fn spec_key(n: u128) -> SpecKey {
    SpecKey(uuid::Uuid::from_u128(n))
}

fn host_writer() -> WriterFacts {
    WriterFacts {
        host: "host-a".to_string(),
        boot: Some(BOOT.to_string()),
        pid_ns: Some(INITIAL_PID_NAMESPACE_INODE),
    }
}

// trace:STORY-2537 trace:BUG-2458 | ai:codex
fn admitted_fixture() -> Witness {
    let id = ProcessIdentity {
        pid: 42,
        start: format!("linux:{BOOT}:9"),
    };
    let identity =
        aida_core::proc_identity::ProcIdentity::from_record(&id, Some(INITIAL_PID_NAMESPACE_INODE))
            .unwrap();
    let record = serde_json::from_value(serde_json::json!({"version": 1, "node_id": aida_core::proc_identity::local_node_id().unwrap(), "identity": identity, "generation": "test-worker", "refusal": null})).unwrap();
    Witness {
        admission: Some(record),
        kind: WitnessKind::Process {
            host: "host-a".into(),
            id,
            pid_ns: Some(INITIAL_PID_NAMESPACE_INODE),
        },
        recorded_at: now(),
    }
}

fn sandboxed_writer() -> WriterFacts {
    WriterFacts {
        pid_ns: Some(4_026_532_999),
        ..host_writer()
    }
}

fn lease(id: &str, scope: &str, extra: &str) -> crate::SessionLease {
    let raw = format!(
        "id = {id:?}\nscope = {scope:?}\nslug = \"s\"\nowner = \"t@example.com\"\n\
         worktree_path = \"/seats/aida-{id}\"\nbranch = \"b-{id}\"\n\
         started_at = \"2026-11-01T00:00:00Z\"\nhostname = \"host-a\"\n{extra}"
    );
    toml::from_str(&raw).expect("valid lease")
}

fn holder(session: &str) -> Holder {
    Holder {
        seat: format!("aida-{session}"),
        role: Role::Implementer,
        session: session.to_string(),
        tree: None,
        witnesses: vec![],
    }
}

fn rec(key: WorkKey, holder: Option<Holder>, phase: Phase) -> Ownership {
    Ownership {
        key,
        generation: 0,
        holder,
        phase,
    }
}

fn spec(n: u128) -> WorkKey {
    WorkKey::Spec { spec: spec_key(n) }
}

fn store() -> (tempfile::TempDir, OwnershipStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = OwnershipStore::at(dir.path().join("ownership"));
    (dir, store)
}

fn acquire_step(key: WorkKey, session: &str, expect: Expect) -> Step<'static> {
    let h = holder(session);
    let k = key.clone();
    Step {
        key,
        op: "acquire".to_string(),
        expect,
        transition: Box::new(move |cur| ops::acquire(cur, &k, h, Phase::Leased)),
    }
}

// ---- store -------------------------------------------------------------

#[test]
fn store_bumps_generation_and_journals_every_write() {
    let (_d, store) = store();
    let key = spec(1);
    let first = store
        .update_all(vec![acquire_step(key.clone(), "s1", Expect::Absent)])
        .unwrap();
    assert_eq!(first[0].as_ref().unwrap().record.generation, 1);
    let second = store
        .update_all(vec![acquire_step(key.clone(), "s2", Expect::Generation(1))])
        .unwrap();
    let second = second[0].as_ref().unwrap();
    assert_eq!(second.record.generation, 2);
    assert!(second
        .note
        .as_deref()
        .unwrap()
        .contains("replaced holder session s1"));
    assert_eq!(store.load(&key).unwrap().unwrap().generation, 2);
    let journal = store.journal();
    assert_eq!(journal.len(), 2);
    assert_eq!(
        (journal[0].from_generation, journal[0].to_generation),
        (None, 1)
    );
    assert_eq!(
        (journal[1].from_generation, journal[1].to_generation),
        (Some(1), 2)
    );
}

#[test]
fn store_cas_refuses_and_never_overwrites() {
    let (_d, store) = store();
    let key = spec(1);
    store
        .update_all(vec![acquire_step(key.clone(), "s1", Expect::Absent)])
        .unwrap();
    let err = store
        .update_all(vec![acquire_step(key.clone(), "s2", Expect::Absent)])
        .unwrap_err();
    assert!(format!("{err:#}").contains("already exists"), "{err:#}");
    let err = store
        .update_all(vec![acquire_step(key.clone(), "s2", Expect::Generation(7))])
        .unwrap_err();
    assert!(format!("{err:#}").contains("changed"), "{err:#}");
    let kept = store.load(&key).unwrap().unwrap();
    assert_eq!(kept.holder.unwrap().session, "s1");
    assert_eq!(store.journal().len(), 1);
}

#[test]
fn store_multi_key_write_is_all_or_nothing() {
    let (_d, store) = store();
    store
        .update_all(vec![acquire_step(spec(2), "s1", Expect::Absent)])
        .unwrap();
    // The second step's expectation fails, so the first is not written.
    let err = store.update_all(vec![
        acquire_step(spec(1), "s9", Expect::Absent),
        acquire_step(spec(2), "s9", Expect::Absent),
    ]);
    assert!(err.is_err());
    assert!(store.load(&spec(1)).unwrap().is_none());
    assert_eq!(
        store
            .load(&spec(2))
            .unwrap()
            .unwrap()
            .holder
            .unwrap()
            .session,
        "s1"
    );
}

#[test]
fn store_malformed_record_fails_closed() {
    let (_d, store) = store();
    let path = store.path_for(&spec(3));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{not json").unwrap();
    assert!(store.load(&spec(3)).is_err());
    let (good, bad) = store.list();
    assert!(good.is_empty());
    assert_eq!(bad.len(), 1);
    assert!(store
        .update_all(vec![acquire_step(spec(3), "s", Expect::Any)])
        .is_err());
}

#[test]
fn store_file_names_cannot_escape_their_directory() {
    let (_d, store) = store();
    let key = WorkKey::Review {
        change: ChangeId {
            forge: "../../etc".to_string(),
            n: 1,
        },
        head: "../../passwd".to_string(),
    };
    let path = store.path_for(&key);
    assert!(
        path.starts_with(store.dir().join("review")),
        "{}",
        path.display()
    );
    assert!(!path.to_string_lossy().contains(".."));
}

// ---- transitions ------------------------------------------------------

#[test]
fn acquire_is_idempotent_and_names_a_replaced_holder() {
    let key = spec(1);
    let cur = rec(key.clone(), Some(holder("s1")), Phase::Leased);
    assert!(ops::acquire(Some(&cur), &key, holder("s1"), Phase::Leased).is_none());
    let (next, note) = ops::acquire(Some(&cur), &key, holder("s2"), Phase::Leased).unwrap();
    assert_eq!(next.holder.unwrap().session, "s2");
    assert!(note.unwrap().contains("s1"));
    // Taking a key that is in a PR replaces nobody.
    let in_pr = rec(
        key.clone(),
        None,
        Phase::InPr {
            change: dual::change(5),
            handed_off_at: now(),
            head: "abc".into(),
        },
    );
    assert!(
        ops::acquire(Some(&in_pr), &key, holder("s3"), Phase::Leased)
            .unwrap()
            .1
            .is_none()
    );
}

#[test]
fn release_only_by_the_holding_session() {
    let key = spec(1);
    let cur = rec(key.clone(), Some(holder("s1")), Phase::Leased);
    assert!(ops::release(Some(&cur), "other", "x").is_none());
    assert!(ops::release(None, "s1", "x").is_none());
    let (next, _) = ops::release(Some(&cur), "s1", "ended").unwrap();
    assert!(next.holder.is_none());
    assert_eq!(next.phase, Phase::Leased);
    let review = rec(
        key.clone(),
        Some(holder("r1")),
        Phase::Reviewing { head: "h".into() },
    );
    let (next, _) = ops::release(Some(&review), "r1", "review ended").unwrap();
    assert!(matches!(next.phase, Phase::Closed { .. }));
}

#[test]
fn handoff_moves_spec_and_change_to_in_pr_in_one_write() {
    let (_d, store) = store();
    store
        .update_all(vec![acquire_step(spec(1), "s1", Expect::Absent)])
        .unwrap();
    let facts = dual::HandoffFacts {
        spec: Some(spec_key(1)),
        change: dual::change(42),
        head: "f00d".to_string(),
    };
    let out = store
        .update_all(dual::handoff_steps(&facts, now()))
        .unwrap();
    assert_eq!(out.len(), 2);
    for key in [
        spec(1),
        WorkKey::Change {
            change: dual::change(42),
        },
    ] {
        let r = store.load(&key).unwrap().unwrap();
        assert!(r.holder.is_none(), "{key:?}");
        assert!(
            matches!(&r.phase, Phase::InPr { change, head, .. } if change.n == 42 && head == "f00d")
        );
    }
    // The judge frees an in-PR key without touching the tree (A7).
    let r = store.load(&spec(1)).unwrap().unwrap();
    let probe = |_: &ProcessIdentity| aida_core::process_probe::Probe::Alive;
    let host = HostFacts {
        node_id: aida_core::proc_identity::local_node_id(),
        host: "host-a".into(),
        boot: Some(BOOT.into()),
        pid_ns: Some(INITIAL_PID_NAMESPACE_INODE),
        probe: &probe,
    };
    assert_eq!(
        judge(
            &r,
            &host,
            &TreeReading::NotAGitTree,
            None,
            &|_, _| false,
            now()
        )
        .reclaim,
        Reclaim::Free
    );
    // Re-running the same handoff writes nothing.
    let again = store
        .update_all(dual::handoff_steps(&facts, now()))
        .unwrap();
    assert!(again.iter().all(Option::is_none));
}

#[test]
fn a_rider_names_its_train() {
    let key = WorkKey::Change {
        change: dual::change(7),
    };
    let (next, _) = ops::ride(None, &key, &dual::change(99)).unwrap();
    assert_eq!(
        next.phase,
        Phase::Training {
            train: dual::change(99)
        }
    );
    assert!(ops::ride(Some(&next), &key, &dual::change(99)).is_none());
}

// ---- dual-write adapters ----------------------------------------------

#[test]
fn lease_start_identity_converts_only_from_the_exact_scheme() {
    let id = dual::identity_from_record(42, "linux-starttime:1234", BOOT).unwrap();
    assert_eq!(id.start, format!("linux:{BOOT}:1234"));
    assert!(dual::identity_from_record(42, "2026-11-01T00:00:00+00:00", BOOT).is_none());
    assert!(dual::identity_from_record(42, "sysinfo-start:2026", BOOT).is_none());
    assert!(dual::identity_from_record(0, "linux-starttime:1", BOOT).is_none());
}

#[test]
fn a1_witnesses_are_recorded_from_the_host_view_only() {
    let w = host_writer().witness(42, Some("linux-starttime:9"), now());
    assert!(
        w.is_none(),
        "an arbitrary target has no cooperative handoff"
    );
    // A sandboxed CLI call (another pid namespace) records no process witness.
    assert!(sandboxed_writer()
        .witness(42, Some("linux-starttime:9"), now())
        .is_none());
    // A4: no start identity, no witness.
    assert!(host_writer().witness(42, None, now()).is_none());
    // A sandboxed writer's lease holder has no witnesses at all.
    let l = lease(
        "l1",
        "STORY-1",
        "creator_pid = 42\ncreator_pid_start_time = \"linux-starttime:9\"\n",
    );
    assert!(dual::holder_for_lease(&l, &sandboxed_writer(), now())
        .witnesses
        .is_empty());
    assert_eq!(
        dual::holder_for_lease(&l, &host_writer(), now())
            .witnesses
            .len(),
        0
    );
}

#[test]
fn a_lease_maps_to_its_work_key() {
    let none = |_: &str| None;
    let known = |_: &str| Some(spec_key(7));
    let review = lease("r", "PR-12", "review_verb = true\npr_head_sha = \"abc\"\n");
    assert_eq!(
        dual::key_for_lease(&review, &none),
        Some((
            WorkKey::Review {
                change: dual::change(12),
                head: "abc".into()
            },
            Phase::Reviewing { head: "abc".into() }
        ))
    );
    let rework = lease("w", "pr-12", "");
    assert_eq!(
        dual::key_for_lease(&rework, &none),
        Some((
            WorkKey::Change {
                change: dual::change(12)
            },
            Phase::Leased
        ))
    );
    let s = lease("s", "STORY-7", "");
    assert_eq!(
        dual::key_for_lease(&s, &known),
        Some((spec(7), Phase::Leased))
    );
    assert_eq!(dual::key_for_lease(&s, &none), None);
    // Round 3, finding 3: a review lease with no recorded head (what `aida
    // review` writes) or one scoped to a spec stands for no key, so it can
    // never take the change or the spec.
    let headless_review = lease("r2", "PR-12", "review_verb = true\n");
    assert_eq!(dual::key_for_lease(&headless_review, &none), None);
    let spec_review = lease(
        "r3",
        "STORY-7",
        "review_verb = true\npr_head_sha = \"abc\"\n",
    );
    assert_eq!(dual::key_for_lease(&spec_review, &known), None);
    assert_eq!(dual::change_scope("PR-0"), None);
    assert_eq!(dual::change_scope("PRX-3"), None);
    assert_eq!(dual::change_scope("MR-3"), Some(3));
    let h = dual::holder_for_lease(&review, &host_writer(), now());
    assert_eq!(h.role, Role::Reviewer);
    assert_eq!(h.session, "r");
}

fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("f"), "1").unwrap();
    git(dir.path(), &["add", "f"]);
    git(
        dir.path(),
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qm",
            "one",
        ],
    );
    dir
}

#[test]
fn lease_written_and_released_round_trip_in_a_repository() {
    dual::enable_for_test();
    let dir = repo();
    let id = uuid::Uuid::from_u128(77);
    let l = lease("sess1", &id.to_string(), "");
    dual::lease_written(dir.path(), &l);
    let store = OwnershipStore::at(dir.path().join(".aida").join(store::DIR));
    let key = WorkKey::Spec { spec: SpecKey(id) };
    let r = store.load(&key).unwrap().expect("acquired");
    assert_eq!(r.holder.as_ref().unwrap().session, "sess1");
    assert_eq!(r.generation, 1);
    // Another session's release changes nothing.
    dual::lease_released(dir.path(), "other", "x");
    assert_eq!(store.load(&key).unwrap().unwrap().generation, 1);
    dual::lease_released(dir.path(), "sess1", "session ended");
    let r = store.load(&key).unwrap().unwrap();
    assert!(r.holder.is_none());
    assert_eq!(r.generation, 2);
}

#[test]
fn dual_write_is_off_unless_the_test_thread_opts_in() {
    let dir = repo();
    let l = lease("sess1", &uuid::Uuid::from_u128(5).to_string(), "");
    dual::lease_written(dir.path(), &l);
    assert!(!dir.path().join(".aida").join(store::DIR).exists());
}

#[test]
fn q2_handoff_check_refuses_a_dirty_or_unpushed_seat() {
    let dir = repo();
    let p = dir.path();
    git(p, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    assert!(dual::verify_seat_at_pr_head(p, "main").is_ok());
    std::fs::write(p.join("f"), "2").unwrap();
    let err = dual::verify_seat_at_pr_head(p, "main").unwrap_err();
    assert!(err.contains("uncommitted"), "{err}");
    git(
        p,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qam",
            "two",
        ],
    );
    let err = dual::verify_seat_at_pr_head(p, "main").unwrap_err();
    assert!(err.contains("is not the head"), "{err}");
    // A seat that fails the check records nothing.
    dual::enable_for_test();
    assert!(dual::handed_off(p, p, "main", "", 9).is_err());
    assert!(!p.join(".aida").join(store::DIR).exists());
}

// ---- migration ---------------------------------------------------------

fn facts(l: crate::SessionLease, key: Option<(WorkKey, Phase)>) -> LeaseFacts {
    LeaseFacts {
        lease: l,
        key,
        spec_closed: false,
        pr: None,
        tree: TreeReading::Read {
            dirty: vec![],
            unpushed: vec![],
            head: Some("h1".into()),
        },
        live_witnesses: vec![],
    }
}

fn classify(f: &LeaseFacts) -> migrate::LeasePlan {
    migrate::classify(f, &host_writer(), &|a, b| a == "h0" && b == "h1", now())
}

#[test]
fn migrate_classifies_each_lease_by_the_sketch() {
    let leased = Some((spec(1), Phase::Leased));
    let base = facts(lease("a", "STORY-1", ""), leased.clone());

    let unresolved = classify(&facts(lease("a", "NOPE", ""), None));
    assert_eq!(unresolved.class, Class::Unresolved);
    assert!(unresolved.records.is_empty());

    let in_pr = classify(&LeaseFacts {
        pr: Some(OpenPr {
            n: 5,
            head: "h1".into(),
        }),
        ..base.clone()
    });
    assert_eq!(in_pr.class, Class::InPr);
    assert_eq!(in_pr.records.len(), 2);
    assert!(in_pr.records.iter().all(|r| r.holder.is_none()));

    // HEAD an ancestor of the PR head is in the PR too.
    let behind = classify(&LeaseFacts {
        pr: Some(OpenPr {
            n: 5,
            head: "h1".into(),
        }),
        tree: TreeReading::Read {
            dirty: vec![],
            unpushed: vec![],
            head: Some("h0".into()),
        },
        ..base.clone()
    });
    assert_eq!(behind.class, Class::InPr);

    let ahead = classify(&LeaseFacts {
        pr: Some(OpenPr {
            n: 5,
            head: "h9".into(),
        }),
        ..base.clone()
    });
    assert_eq!(ahead.class, Class::Unknown);

    let dirty = classify(&LeaseFacts {
        pr: Some(OpenPr {
            n: 5,
            head: "h1".into(),
        }),
        tree: TreeReading::Read {
            dirty: vec![" M f".into()],
            unpushed: vec![],
            head: Some("h1".into()),
        },
        ..base.clone()
    });
    assert_eq!(dirty.class, Class::Salvage);
    assert_eq!(dirty.records[0].phase, Phase::Leased);

    let unpushed = classify(&LeaseFacts {
        tree: TreeReading::Read {
            dirty: vec![],
            unpushed: vec!["c1".into()],
            head: Some("c1".into()),
        },
        spec_closed: true,
        ..base.clone()
    });
    assert_eq!(unpushed.class, Class::Salvage);

    let not_git = classify(&LeaseFacts {
        tree: TreeReading::NotAGitTree,
        ..base.clone()
    });
    assert_eq!(not_git.class, Class::NotAGitTree);

    let closed = classify(&LeaseFacts {
        spec_closed: true,
        tree: TreeReading::Missing,
        ..base.clone()
    });
    assert_eq!(closed.class, Class::Closed);
    assert!(closed.records[0].holder.is_none());

    let unknown = classify(&base);
    assert_eq!(unknown.class, Class::Unknown);
    assert!(unknown.records[0]
        .holder
        .as_ref()
        .unwrap()
        .witnesses
        .is_empty());
}

#[test]
fn a4_migration_never_records_alive_without_a_live_probe() {
    // The lease records a pid with a start identity, but the collector found
    // it not running: no witness, so the holder is Unknown, never Alive.
    let l = lease(
        "a",
        "STORY-1",
        "creator_pid = 42\ncreator_pid_start_time = \"linux-starttime:9\"\n",
    );
    let f = facts(l.clone(), Some((spec(1), Phase::Leased)));
    let p = classify(&f);
    assert_eq!(p.class, Class::Unknown);
    assert!(p.records[0].holder.as_ref().unwrap().witnesses.is_empty());
    // With the probe's live witness it is Live, carrying exactly that witness.
    let w = admitted_fixture();
    let p = classify(&LeaseFacts {
        live_witnesses: vec![w.clone()],
        ..f
    });
    assert_eq!(p.class, Class::Live);
    assert_eq!(p.records[0].holder.as_ref().unwrap().witnesses, vec![w]);
    // A pid without a start identity can never become a witness.
    let bare = lease("b", "STORY-1", "creator_pid = 42\n");
    assert!(dual::holder_for_lease(&bare, &host_writer(), now())
        .witnesses
        .is_empty());
}

#[test]
fn migrate_trains_have_a_witness_only_when_alive() {
    let dead = migrate::classify_train(&TrainLeaseFacts {
        train: 3,
        pid: 50,
        live_witness: None,
    });
    assert!(dead.holder.as_ref().unwrap().witnesses.is_empty());
    assert_eq!(
        dead.phase,
        Phase::Training {
            train: dual::change(3)
        }
    );
}

#[test]
fn migrate_apply_is_idempotent_and_keeps_newer_records() {
    let (_d, store) = store();
    let a = facts(lease("a", "STORY-1", ""), Some((spec(1), Phase::Leased)));
    let b = facts(lease("b", "STORY-2", ""), Some((spec(2), Phase::Leased)));
    // A dual-write already recorded spec 2.
    store
        .update_all(vec![acquire_step(spec(2), "newer", Expect::Absent)])
        .unwrap();
    let plan = migrate::plan(
        &[a, b],
        &[],
        Default::default(),
        vec![],
        &host_writer(),
        &|_, _| false,
        now(),
    );
    let first = migrate::apply(&store, &plan).unwrap();
    assert_eq!((first.written, first.kept), (1, 1));
    assert_eq!(
        store
            .load(&spec(2))
            .unwrap()
            .unwrap()
            .holder
            .unwrap()
            .session,
        "newer"
    );
    let again = migrate::apply(&store, &plan).unwrap();
    assert_eq!((again.written, again.kept), (0, 2));
}

#[test]
fn migrate_reports_two_leases_for_one_key() {
    let a = facts(lease("a", "STORY-1", ""), Some((spec(1), Phase::Leased)));
    let b = facts(lease("b", "STORY-1", ""), Some((spec(1), Phase::Leased)));
    let plan = migrate::plan(
        &[a, b],
        &[],
        Default::default(),
        vec![],
        &host_writer(),
        &|_, _| false,
        now(),
    );
    assert_eq!(plan.records().len(), 1);
    assert_eq!(plan.duplicates.len(), 1);
    assert_eq!(plan.records()[0].holder.as_ref().unwrap().session, "a");
}

#[test]
fn migrate_prefers_a_live_lease_over_an_unknown_one_for_one_key() {
    // Round 3, finding 7: the dead lease is first in list order, but the
    // running holder's lease wins the key.
    let dead = facts(lease("dead", "STORY-1", ""), Some((spec(1), Phase::Leased)));
    let l = lease(
        "live",
        "STORY-1",
        "creator_pid = 42\ncreator_pid_start_time = \"linux-starttime:9\"\n",
    );
    let w = admitted_fixture();
    let live = LeaseFacts {
        live_witnesses: vec![w],
        ..facts(l, Some((spec(1), Phase::Leased)))
    };
    let plan = migrate::plan(
        &[dead, live],
        &[],
        Default::default(),
        vec![],
        &host_writer(),
        &|_, _| false,
        now(),
    );
    assert_eq!(plan.records().len(), 1);
    assert_eq!(plan.records()[0].holder.as_ref().unwrap().session, "live");
    assert_eq!(plan.duplicates.len(), 1);
    assert!(
        plan.duplicates[0].1.contains("lease dead (kept live)"),
        "{:?}",
        plan.duplicates
    );
}

#[test]
fn migrate_records_nothing_for_a_review_lease_without_a_head() {
    let l = lease("r", "PR-12", "review_verb = true\n");
    let p = classify(&facts(l.clone(), dual::key_for_lease(&l, &|_| None)));
    assert_eq!(p.class, Class::Unresolved);
    assert!(p.records.is_empty());
    assert!(p.detail.contains("review"), "{}", p.detail);
}

// ---- round 3: reviews, containment, train riders ------------------------

fn repo_store(dir: &Path) -> OwnershipStore {
    OwnershipStore::at(dir.join(".aida").join(store::DIR))
}

#[test]
fn a_review_lease_never_takes_the_change_or_the_spec() {
    dual::enable_for_test();
    let dir = repo();
    let id = uuid::Uuid::from_u128(91);
    // The implementer holds the spec; PR-12 was handed off.
    dual::lease_written(dir.path(), &lease("impl", &id.to_string(), ""));
    let store = repo_store(dir.path());
    store
        .update_all(dual::handoff_steps(
            &dual::HandoffFacts {
                spec: None,
                change: dual::change(12),
                head: "h12".into(),
            },
            now(),
        ))
        .unwrap();
    let change_key = WorkKey::Change {
        change: dual::change(12),
    };
    let spec_key = WorkKey::Spec { spec: SpecKey(id) };
    let before_change = store.load(&change_key).unwrap().unwrap();
    let before_spec = store.load(&spec_key).unwrap().unwrap();
    // What `aida review` writes: a PR-N or a spec scope, no head.
    dual::lease_written(dir.path(), &lease("rv1", "PR-12", "review_verb = true\n"));
    dual::lease_written(
        dir.path(),
        &lease("rv2", &id.to_string(), "review_verb = true\n"),
    );
    assert_eq!(store.load(&change_key).unwrap().unwrap(), before_change);
    assert_eq!(store.load(&spec_key).unwrap().unwrap(), before_spec);
    assert_eq!(before_spec.holder.as_ref().unwrap().session, "impl");
}

#[test]
fn the_review_lease_guard_releases_its_record_on_drop() {
    dual::enable_for_test();
    let dir = repo();
    let store = repo_store(dir.path());
    // A review recorded with its head, held by the review session.
    dual::lease_written(
        dir.path(),
        &lease(
            "rv1",
            "PR-12",
            "review_verb = true\npr_head_sha = \"h12\"\n",
        ),
    );
    let key = WorkKey::Review {
        change: dual::change(12),
        head: "h12".into(),
    };
    assert_eq!(
        store.load(&key).unwrap().unwrap().holder.unwrap().session,
        "rv1"
    );
    let lease_file = dir.path().join("rv1.toml");
    std::fs::write(&lease_file, "").unwrap();
    drop(crate::ReviewLeaseGuard {
        path: lease_file.clone(),
        project_root: dir.path().to_path_buf(),
        lease_id: "rv1".to_string(),
    });
    assert!(!lease_file.exists());
    let r = store.load(&key).unwrap().unwrap();
    assert!(r.holder.is_none(), "{r:?}");
    assert!(matches!(r.phase, Phase::Closed { .. }), "{r:?}");
}

#[test]
fn a_contained_process_writes_no_ownership_record() {
    dual::enable_for_test();
    let dir = repo();
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);
    let id = uuid::Uuid::from_u128(92);
    dual::lease_written(dir.path(), &lease("s", &id.to_string(), ""));
    dual::train_lease_taken(dir.path(), 5);
    dual::handed_off_from_seat(dir.path(), &id.to_string(), "tip", 7);
    let own = dir.path().join(".aida").join(store::DIR);
    assert!(
        !own.exists(),
        "a contained process created {}",
        own.display()
    );
    // The store itself refuses, before it creates or locks anything.
    let store = repo_store(dir.path());
    let err = store
        .update(&spec(92), "acquire", Expect::Any, |cur| {
            ops::acquire(cur, &spec(92), holder("s"), Phase::Leased)
        })
        .unwrap_err();
    assert!(format!("{err:#}").contains("contained"), "{err:#}");
    assert!(!own.exists());
}

#[test]
fn releasing_a_train_moves_its_riders_out_of_training() {
    let (_d, store) = store();
    let session = dual::merge_lease_session(4242);
    let train = dual::change(50);
    let train_key = WorkKey::Train {
        train: train.clone(),
    };
    let t_holder = Holder {
        seat: "train".into(),
        role: Role::Train,
        session: session.clone(),
        tree: None,
        witnesses: vec![],
    };
    let k = train_key.clone();
    let t = train.clone();
    store
        .update(&train_key, "begin-train", Expect::Any, |cur| {
            ops::acquire(cur, &k, t_holder, Phase::Training { train: t })
        })
        .unwrap();
    let rider = |n| WorkKey::Change {
        change: dual::change(n),
    };
    for (n, on) in [(11, 50), (12, 50), (13, 60)] {
        let key = rider(n);
        let k = key.clone();
        store
            .update(&key, "ride", Expect::Any, |cur| {
                ops::ride(cur, &k, &dual::change(on))
            })
            .unwrap();
    }
    let (records, _) = store.list();
    store
        .update_all(dual::train_release_steps(&records, &session, "merged"))
        .unwrap();
    let t = store.load(&train_key).unwrap().unwrap();
    assert!(t.holder.is_none());
    assert!(matches!(t.phase, Phase::Closed { .. }), "{t:?}");
    for n in [11, 12] {
        let r = store.load(&rider(n)).unwrap().unwrap();
        assert_eq!(r.phase, Phase::Leased, "rider {n}: {r:?}");
    }
    // A rider on another train is untouched.
    assert_eq!(
        store.load(&rider(13)).unwrap().unwrap().phase,
        Phase::Training {
            train: dual::change(60)
        }
    );
    // Released again: nothing left to do.
    let (records, _) = store.list();
    let again = store
        .update_all(dual::train_release_steps(&records, &session, "merged"))
        .unwrap();
    assert!(again.iter().all(Option::is_none));
}

#[test]
fn a4_execute_refuses_off_the_host() {
    let host = Some(INITIAL_PID_NAMESPACE_INODE);
    assert!(migrate::execute_refusal(true, host, false, false).is_none());
    assert!(migrate::execute_refusal(false, host, false, false)
        .unwrap()
        .contains("terminal"));
    assert!(
        migrate::execute_refusal(true, Some(4_026_532_999), false, false)
            .unwrap()
            .contains("pid namespace")
    );
    assert!(migrate::execute_refusal(true, None, false, false).is_some());
    assert!(migrate::execute_refusal(true, host, true, false)
        .unwrap()
        .contains("contained"));
    assert!(migrate::execute_refusal(true, host, false, true)
        .unwrap()
        .contains("drain"));
}

// ---- compare -----------------------------------------------------------

#[test]
fn compare_names_every_expected_difference_and_flags_the_rest() {
    let alive = OwnerVerdict::Alive {
        witness: "pid 1".into(),
    };
    let dead = OwnerVerdict::Dead {
        proof: "gone".into(),
    };
    let unknown = OwnerVerdict::Unknown {
        why: "no witness".into(),
    };
    let held = rec(spec(1), Some(holder("l1")), Phase::Leased);
    let c = |today, owner: &OwnerVerdict| compare_one("l1", today, Some(&held), Some(owner));
    assert_eq!(c(LeaseState::Live, &alive), Outcome::Agree);
    assert_eq!(c(LeaseState::Stale, &dead), Outcome::Agree);
    assert_eq!(c(LeaseState::Dormant, &unknown), Outcome::Agree);
    assert_eq!(
        c(LeaseState::Live, &unknown),
        Outcome::Expected(expected::LIVE_UNPROVEN.into())
    );
    assert_eq!(
        c(LeaseState::Stale, &unknown),
        Outcome::Expected(expected::STALE_UNPROVEN.into())
    );
    assert_eq!(
        c(LeaseState::Dormant, &alive),
        Outcome::Expected(expected::DORMANT_ALIVE.into())
    );
    assert_eq!(
        c(LeaseState::Dormant, &dead),
        Outcome::Expected(expected::DORMANT_DEAD.into())
    );
    assert!(matches!(
        c(LeaseState::Live, &dead),
        Outcome::Unexplained(_)
    ));
    assert!(matches!(
        c(LeaseState::Stale, &alive),
        Outcome::Unexplained(_)
    ));

    assert_eq!(
        compare_one("l1", LeaseState::Live, None, None),
        Outcome::Expected(expected::UNRECORDED.into())
    );
    let other = rec(spec(1), Some(holder("l2")), Phase::Leased);
    assert!(matches!(
        compare_one("l1", LeaseState::Live, Some(&other), Some(&alive)),
        Outcome::Unexplained(_)
    ));
    let in_pr = rec(
        spec(1),
        None,
        Phase::InPr {
            change: dual::change(1),
            handed_off_at: now(),
            head: "h".into(),
        },
    );
    assert_eq!(
        compare_one("l1", LeaseState::Live, Some(&in_pr), None),
        Outcome::Expected(expected::HANDED_OFF.into())
    );
    assert!(orphan_outcome(&held, &alive).is_some());
    let train = rec(
        WorkKey::Train {
            train: dual::change(3),
        },
        Some(Holder {
            session: dual::merge_lease_session(9),
            ..holder("x")
        }),
        Phase::Training {
            train: dual::change(3),
        },
    );
    assert!(orphan_outcome(&train, &dead).is_none());
}

// trace:STORY-2537 trace:BUG-2458 | ai:codex
#[test]
fn proc_identity_migration_is_pinned_and_preserves_legacy_evidence() {
    let (_d, store) = store();
    let mut old_holder = holder("worker-session");
    let mut legacy = admitted_fixture();
    legacy.admission = None;
    old_holder.witnesses.push(legacy);
    old_holder.tree = Some(SeatTree {
        path: "/kept-tree".into(),
        kind: TreeKind::Worktree,
        branch: Some("kept-branch".into()),
    });
    let key = spec(999);
    store
        .update(&key, "test", Expect::Absent, |cur| {
            ops::acquire(
                cur,
                &key,
                old_holder.clone(),
                Phase::Reviewing {
                    head: "kept-head".into(),
                },
            )
        })
        .unwrap();
    let original = store.load(&key).unwrap().unwrap();
    let unresolved = facts(lease("worker-session", "unresolved", ""), None);
    let mut plan = migrate::plan(
        &[unresolved],
        &[],
        Default::default(),
        vec![],
        &host_writer(),
        &|_, _| false,
        now(),
    );
    plan.existing = vec![original.clone()];
    assert_eq!(migrate::apply(&store, &plan).unwrap().written, 0);
    let repaired = store.load(&key).unwrap().unwrap();
    assert_eq!(repaired.phase, original.phase);
    assert_eq!(
        repaired.holder.as_ref().unwrap().tree,
        original.holder.as_ref().unwrap().tree
    );
    assert_eq!(repaired.holder.as_ref().unwrap().session, "worker-session");
    assert_eq!(
        repaired, original,
        "migration preserves legacy liveness evidence"
    );
    plan.existing = vec![repaired.clone()];
    assert_eq!(migrate::apply(&store, &plan).unwrap().written, 0);
    store
        .update(
            &key,
            "race",
            Expect::Generation(repaired.generation),
            |cur| ops::release(cur, "worker-session", "independent newer event"),
        )
        .unwrap();
    assert!(migrate::apply(&store, &plan).is_err());
    assert!(store.load(&key).unwrap().unwrap().holder.is_none());
}

// trace:STORY-2537 trace:BUG-2458 | ai:codex
#[test]
fn proc_identity_lease_selects_worker_never_creator() {
    let worker = admitted_fixture();
    let raw = serde_json::to_string(worker.admission.as_ref().unwrap()).unwrap();
    let l = lease("lane", "STORY-1", &format!(
        "active_pid = 42\nactive_pid_start_time = 'linux-starttime:9'\ncreator_pid = 2013\ncreator_pid_start_time = 'linux-starttime:1'\npid_namespace = {INITIAL_PID_NAMESPACE_INODE}\nholder_record = {raw:?}\n"
    ));
    let holder = dual::holder_for_lease(&l, &host_writer(), now());
    assert_eq!(holder.witnesses.len(), 1);
    let probe = |id: &ProcessIdentity| {
        assert_eq!(id.pid, 42, "creator/manager must never be probed as holder");
        aida_core::process_probe::Probe::Dead("worker exited".into())
    };
    let facts = HostFacts {
        node_id: aida_core::proc_identity::local_node_id(),
        host: "host-a".into(),
        boot: Some(BOOT.into()),
        pid_ns: Some(INITIAL_PID_NAMESPACE_INODE),
        probe: &probe,
    };
    assert!(matches!(
        judge_owner(&holder, &facts, now()),
        OwnerVerdict::Dead { .. }
    ));
    let mut replaced = l.clone();
    replaced.active_pid = Some(2013);
    let refused = dual::holder_for_lease(&replaced, &host_writer(), now());
    assert!(matches!(
        judge_owner(&refused, &facts, now()),
        OwnerVerdict::Unknown { .. }
    ));
}

// trace:STORY-2325 | ai:codex
#[test]
fn prc_staging_failure_does_not_publish_any_record_or_journal() {
    let (_dir, store) = store();
    store
        .update(&spec(1), "seed", Expect::Absent, |_| {
            Some((rec(spec(1), Some(holder("owner")), Phase::Leased), None))
        })
        .unwrap();
    let journal = std::fs::read(store.journal_path()).unwrap();
    let error = store
        .update_all_checked(
            vec![Step {
                key: spec(1),
                op: "handoff".into(),
                expect: Expect::Generation(1),
                transition: Box::new(|current| ops::release(current, "owner", "ended")),
            }],
            || anyhow::bail!("injected staging refusal"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("staging refusal"));
    assert_eq!(
        store
            .load(&spec(1))
            .unwrap()
            .unwrap()
            .holder
            .unwrap()
            .session,
        "owner"
    );
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), journal);
    assert!(!store.dir().join(".pending.json").exists());
}

// trace:STORY-2325 | ai:codex
#[test]
fn prc_pending_batch_recovery_is_complete_and_idempotent() {
    let (_dir, store) = store();
    store
        .update(&spec(1), "seed", Expect::Absent, |_| {
            Some((rec(spec(1), None, Phase::Leased), None))
        })
        .unwrap();
    let mut first = rec(spec(1), Some(holder("new")), Phase::Leased);
    first.generation = 2;
    let mut second = rec(spec(2), Some(holder("new")), Phase::Leased);
    second.generation = 1;
    // Crash after the first record was applied, before the second and journal.
    std::fs::write(
        store.path_for(&spec(1)),
        serde_json::to_vec(&first).unwrap(),
    )
    .unwrap();
    let journal = std::fs::read_to_string(store.journal_path()).unwrap();
    std::fs::write(
        store.dir().join(".pending.json"),
        serde_json::to_vec(&serde_json::json!({
            "records": [first, second], "journal": journal,
        }))
        .unwrap(),
    )
    .unwrap();
    let all = store.list();
    assert!(all.1.is_empty());
    assert_eq!(all.0.len(), 2);
    assert!(all
        .0
        .iter()
        .all(|r| r.holder.as_ref().unwrap().session == "new"));
    assert!(!store.dir().join(".pending.json").exists());
    assert_eq!(store.load(&spec(1)).unwrap().unwrap().generation, 2);
    assert_eq!(store.load(&spec(2)).unwrap().unwrap().generation, 1);
}

// trace:STORY-2325 | ai:codex
#[test]
fn prc_unrecoverable_batch_refuses_all_readers() {
    let (_dir, store) = store();
    store
        .update(&spec(1), "seed", Expect::Absent, |_| {
            Some((rec(spec(1), None, Phase::Leased), None))
        })
        .unwrap();
    let record = rec(spec(2), None, Phase::Leased);
    std::fs::create_dir_all(store.path_for(&spec(2))).unwrap();
    std::fs::write(
        store.dir().join(".pending.json"),
        serde_json::to_vec(&serde_json::json!({
            "records": [record], "journal": "",
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(store.load(&spec(1)).is_err());
    let (records, errors) = store.list();
    assert!(records.is_empty());
    assert_eq!(errors.len(), 1);
    assert!(store.dir().join(".pending.json").exists());
}

// trace:STORY-2325 | ai:codex
#[test]
fn prc_handoff_detaches_only_the_owner_at_the_pushed_head() {
    let dir = repo();
    let p = dir.path();
    git(p, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    std::fs::write(p.join(".git/info/exclude"), ".aida/\n").unwrap();
    let store = OwnershipStore::at(p.join(".aida/ownership"));
    let mut h = holder("owner");
    h.tree = Some(SeatTree {
        path: p.to_path_buf(),
        kind: TreeKind::Worktree,
        branch: Some("main".into()),
    });
    store
        .update(&spec(1), "seed", Expect::Absent, |_| {
            Some((rec(spec(1), Some(h), Phase::Leased), None))
        })
        .unwrap();
    assert!(dual::handoff_seat(&store, p, "main", Some(spec_key(1)), 9, "intruder").is_err());
    let output = crate::test_git::git_command()
        .arg("-C")
        .arg(p)
        .args(["symbolic-ref", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "refs/heads/main"
    );
    std::fs::write(p.join("untracked"), "salvage").unwrap();
    assert!(dual::handoff_seat(&store, p, "main", Some(spec_key(1)), 9, "owner").is_err());
    std::fs::remove_file(p.join("untracked")).unwrap();
    dual::handoff_seat(&store, p, "main", Some(spec_key(1)), 9, "owner").unwrap();
    let output = crate::test_git::git_command()
        .arg("-C")
        .arg(p)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "the handed-off branch must be free"
    );
    for key in [
        spec(1),
        WorkKey::Change {
            change: dual::change(9),
        },
    ] {
        let record = store.load(&key).unwrap().unwrap();
        assert!(record.holder.is_none());
        assert!(matches!(record.phase, Phase::InPr { .. }));
    }
}

// trace:STORY-2325 | ai:codex
#[test]
fn prc_readers_do_not_infer_death_from_absence_or_elapsed_time() {
    let dir = repo();
    let key = WorkKey::Train {
        train: dual::change(9),
    };
    assert!(reader::ended(dir.path(), &key).unwrap().is_none());
    let store = OwnershipStore::for_project(dir.path()).unwrap().unwrap();
    store
        .update(&key, "seed", Expect::Absent, |_| {
            Some((
                rec(
                    key.clone(),
                    Some(holder("unknown")),
                    Phase::Training {
                        train: dual::change(9),
                    },
                ),
                None,
            ))
        })
        .unwrap();
    assert!(reader::ended(dir.path(), &key).unwrap().is_none());
    store
        .update(&key, "release", Expect::Generation(1), |current| {
            ops::release(current, "unknown", "explicit release")
        })
        .unwrap();
    assert_eq!(
        reader::ended(dir.path(), &key).unwrap(),
        Some("explicit release".into())
    );
}
