//! Native two-clone coverage of the store sync evidence boundary.
// trace:BUG-2162 | ai:claude
use super::super::{
    fast_forward_store, head_sha, merge_union_auto, pull_rebase, pull_rebase_auto_merge,
    rebase_in_progress,
};
use super::FAIL_FINALIZE_AT;
use crate::git_ops::is_evidence_refusal;
use crate::models::{
    HypothesisLock, Relationship, RelationshipType, Requirement, RequirementStatus, RequirementType,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn sha(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn lock(desc: &str, secs: i64) -> HypothesisLock {
    HypothesisLock {
        locked_at: chrono::DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap(),
        description_sha256: sha(desc),
    }
}

/// A hypothesis SPIKE-1 and an experiment TASK-2, unlinked and open.
fn rows() -> (Requirement, Requirement) {
    let mut h = Requirement::new("prediction".into(), "before the result".into());
    h.spec_id = Some("SPIKE-1".into());
    h.req_type = RequirementType::Spike;
    h.tags.insert("hypothesis".into());
    let mut r = Requirement::new("experiment".into(), "run it".into());
    r.spec_id = Some("TASK-2".into());
    (h, r)
}

fn link(h: &mut Requirement, r: &Requirement) {
    h.relationships.push(Relationship {
        rel_type: RelationshipType::References,
        target_id: r.id,
        created_at: None,
        created_by: None,
    });
}

struct Clones {
    _tmp: tempfile::TempDir,
    tmp: PathBuf,
    a: PathBuf,
    b: PathBuf,
}

/// Seed a hub with `seed` and two store clones of it, `a` and `b`.
fn clones(seed: &[Requirement]) -> Clones {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let hub = root.join("hub.git");
    run(&root, &["init", "-q", "--bare", hub.to_str().unwrap()]);
    run(&hub, &["symbolic-ref", "HEAD", "refs/heads/aida-store"]);
    let a = root.join("a");
    run(
        &root,
        &["init", "-q", "-b", "aida-store", a.to_str().unwrap()],
    );
    for (k, v) in [("user.email", "t@example.invalid"), ("user.name", "T")] {
        run(&a, &["config", k, v]);
    }
    // trace:BUG-2017 | ai:codex — evidence union fixtures model established HOME registries.
    crate::allocation_authority::home_fixture_backend(&a).unwrap();
    std::fs::write(a.join("metadata.yaml"), "name: test\n").unwrap();
    for row in seed {
        write(&a, row);
    }
    commit(&a, "seed");
    run(&a, &["remote", "add", "origin", hub.to_str().unwrap()]);
    run(&a, &["push", "-q", "-u", "origin", "aida-store"]);
    let b = root.join("b");
    run(
        &root,
        &["clone", "-q", hub.to_str().unwrap(), b.to_str().unwrap()],
    );
    for (k, v) in [("user.email", "t@example.invalid"), ("user.name", "T")] {
        run(&b, &["config", k, v]);
    }
    crate::allocation_authority::home_fixture_backend(&b).unwrap();
    Clones {
        _tmp: tmp,
        tmp: root,
        a,
        b,
    }
}

fn write(repo: &Path, row: &Requirement) {
    crate::object_store::write_object(&repo.join("objects"), row).unwrap();
}

fn commit(repo: &Path, msg: &str) {
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", msg]);
}

fn publish(repo: &Path, row: &Requirement, msg: &str) {
    write(repo, row);
    commit(repo, msg);
    run(repo, &["push", "-q", "origin", "aida-store"]);
}

fn read(repo: &Path, spec: &str) -> Requirement {
    crate::object_store::read_object(&repo.join("objects"), spec).unwrap()
}

fn subject(repo: &Path) -> String {
    run(repo, &["log", "-1", "--format=%s"])
}

fn tracked_clean(repo: &Path) -> bool {
    run(repo, &["status", "--porcelain", "--untracked-files=no"]).is_empty()
}

type Sync = fn(&Path) -> anyhow::Result<()>;

fn via_rebase(repo: &Path) -> anyhow::Result<()> {
    pull_rebase(repo, "origin", "aida-store")
}

fn via_auto_merge(repo: &Path) -> anyhow::Result<()> {
    pull_rebase_auto_merge(repo, "origin", "aida-store").map(|_| ())
}

fn via_union(repo: &Path) -> anyhow::Result<()> {
    run(repo, &["fetch", "-q", "origin"]);
    merge_union_auto(repo, "origin/aida-store", "union").map(|_| ())
}

const SYNCS: [(&str, Sync); 3] = [
    ("rebase", via_rebase),
    ("auto-merge", via_auto_merge),
    ("union", via_union),
];

/// The STORY-1675 self-check reproduction: one clone completes the linked
/// result (stamping the hypothesis), the other rewrites only the description.
fn offline_body_change() -> (Clones, Requirement) {
    let (mut h, mut r) = rows();
    link(&mut h, &r);
    let c = clones(&[h.clone(), r.clone()]);
    r.status = RequirementStatus::Completed;
    write(&c.a, &r);
    let mut locked = h.clone();
    locked.hypothesis_lock = Some(lock(&h.description, 0));
    publish(&c.a, &locked, "result");
    let mut edited = h.clone();
    edited.description = "rewritten after the fact".into();
    write(&c.b, &edited);
    commit(&c.b, "offline edit");
    (c, edited)
}

#[test]
fn bug_2162_clean_text_rebase_refuses_offline_body_change_before_git() {
    // The fixture really is a clean textual merge: raw Git combines the new
    // body with the old hash in a disposable copy.
    let (c, _) = offline_body_change();
    let copy = c.tmp.join("copy");
    run(
        &c.tmp,
        &["clone", "-q", c.b.to_str().unwrap(), copy.to_str().unwrap()],
    );
    for (k, v) in [("user.email", "t@example.invalid"), ("user.name", "T")] {
        run(&copy, &["config", k, v]);
    }
    let hub = c.tmp.join("hub.git");
    run(&copy, &["fetch", "-q", hub.to_str().unwrap(), "aida-store"]);
    run(&copy, &["rebase", "-q", "FETCH_HEAD"]);
    let raw = read(&copy, "SPIKE-1");
    assert_eq!(raw.description, "rewritten after the fact");
    assert_ne!(
        raw.hypothesis_lock.unwrap().description_sha256,
        sha(&raw.description)
    );

    for (name, sync) in SYNCS {
        let (c, edited) = offline_body_change();
        let before = head_sha(&c.b).unwrap();
        let err = sync(&c.b).unwrap_err();
        assert!(is_evidence_refusal(&err), "{name}: {err:#}");
        assert!(format!("{err:#}").contains("SPIKE-1"), "{name}: {err:#}");
        assert_eq!(head_sha(&c.b).unwrap(), before, "{name}");
        assert!(!rebase_in_progress(&c.b), "{name}");
        assert!(tracked_clean(&c.b), "{name}");
        assert_eq!(read(&c.b, "SPIKE-1").description, edited.description);
    }
}

#[test]
fn bug_2162_clean_sync_preserves_exact_existing_lock() {
    for (name, sync) in SYNCS {
        let (mut h, mut r) = rows();
        link(&mut h, &r);
        let c = clones(&[h.clone(), r.clone()]);
        r.status = RequirementStatus::Completed;
        write(&c.a, &r);
        let mut locked = h.clone();
        locked.hypothesis_lock = Some(lock(&h.description, 7));
        publish(&c.a, &locked, "result");
        let mut retitled = h.clone();
        retitled.title = "sharper title".into();
        write(&c.b, &retitled);
        commit(&c.b, "retitle");
        sync(&c.b).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let got = read(&c.b, "SPIKE-1");
        assert_eq!(got.hypothesis_lock, Some(lock(&h.description, 7)), "{name}");
        assert_eq!(got.description, h.description);
        assert_eq!(got.title, "sharper title");
        assert_ne!(
            subject(&c.b),
            "aida: lock hypothesis evidence after store sync"
        );
    }
}

#[test]
fn bug_2162_conflicted_sync_converges_same_body_and_refuses_divergent_stamps() {
    // Both clones locked the same body independently: the earliest stamp wins.
    let (mut h, r) = rows();
    link(&mut h, &r);
    let c = clones(&[h.clone(), r.clone()]);
    let mut done = r.clone();
    done.status = RequirementStatus::Completed;
    write(&c.a, &done);
    let mut late = h.clone();
    late.hypothesis_lock = Some(lock(&h.description, 50));
    publish(&c.a, &late, "late lock");
    write(&c.b, &done);
    let mut early = h.clone();
    early.hypothesis_lock = Some(lock(&h.description, 5));
    write(&c.b, &early);
    commit(&c.b, "early lock");
    pull_rebase_auto_merge(&c.b, "origin", "aida-store").unwrap();
    assert_eq!(
        read(&c.b, "SPIKE-1").hypothesis_lock,
        Some(lock(&h.description, 5))
    );
    assert!(tracked_clean(&c.b));

    // Stamps over different bodies never converge, conflict or not.
    let c = clones(&[h.clone(), r.clone()]);
    publish(&c.a, &late, "lock");
    let mut other = h.clone();
    other.description = "a different prediction".into();
    other.hypothesis_lock = Some(lock(&other.description, 1));
    write(&c.b, &other);
    commit(&c.b, "other lock");
    let before = head_sha(&c.b).unwrap();
    let err = pull_rebase_auto_merge(&c.b, "origin", "aida-store").unwrap_err();
    assert!(is_evidence_refusal(&err), "{err:#}");
    assert_eq!(head_sha(&c.b).unwrap(), before);
}

#[test]
fn bug_2162_cross_clone_first_result_materializes_lock() {
    for (name, sync) in SYNCS {
        // The edge arrives from one clone, the completion from the other.
        let (h, r) = rows();
        let c = clones(&[h.clone(), r.clone()]);
        let mut linked = h.clone();
        link(&mut linked, &r);
        publish(&c.a, &linked, "link");
        let mut done = r.clone();
        done.status = RequirementStatus::Completed;
        write(&c.b, &done);
        commit(&c.b, "complete");
        sync(&c.b).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let got = read(&c.b, "SPIKE-1");
        let stamp = got.hypothesis_lock.clone().expect(name);
        assert_eq!(stamp.description_sha256, sha(&h.description), "{name}");
        assert_eq!(
            subject(&c.b),
            "aida: lock hypothesis evidence after store sync",
            "{name}"
        );
        assert!(tracked_clean(&c.b), "{name}");
        let head = head_sha(&c.b).unwrap();
        sync(&c.b).unwrap();
        assert_eq!(
            head_sha(&c.b).unwrap(),
            head,
            "{name}: repeat sync is a no-op"
        );
        assert_eq!(read(&c.b, "SPIKE-1").hypothesis_lock, Some(stamp));

        // The canonical mutation path now refuses a description rewrite.
        use crate::db::DatabaseBackend;
        let backend = crate::allocation_authority::home_fixture_backend(&c.b)
            .unwrap()
            .with_auto_commit(false)
            .with_oplog(false);
        let mut rewrite = read(&c.b, "SPIKE-1");
        rewrite.description = "moved goalposts".into();
        assert!(backend.update_requirement(&rewrite).is_err(), "{name}");
    }
}

#[test]
fn bug_2162_fast_forward_materializes_lock_from_older_writer() {
    // An older binary pushed the link and the completion without stamping.
    let (h, r) = rows();
    let c = clones(&[h.clone(), r.clone()]);
    let mut linked = h.clone();
    link(&mut linked, &r);
    write(&c.a, &linked);
    let mut done = r.clone();
    done.status = RequirementStatus::Completed;
    publish(&c.a, &done, "link and complete");
    run(&c.b, &["fetch", "-q", "origin"]);
    let tip = run(&c.b, &["rev-parse", "origin/aida-store"]);
    fast_forward_store(&c.b, "origin/aida-store").unwrap();
    assert_eq!(
        super::super::first_parent_sha(&c.b, &head_sha(&c.b).unwrap()),
        Some(tip)
    );
    assert_eq!(
        read(&c.b, "SPIKE-1")
            .hypothesis_lock
            .unwrap()
            .description_sha256,
        sha(&h.description)
    );
}

#[test]
fn bug_2162_boundary_cancellation_stays_sticky() {
    for (name, sync) in SYNCS {
        // One clone completes the result while the other cancels the
        // hypothesis by dropping its tag: the result boundary still locks it.
        let (mut h, r) = rows();
        link(&mut h, &r);
        let c = clones(&[h.clone(), r.clone()]);
        let mut done = r.clone();
        done.status = RequirementStatus::Completed;
        publish(&c.a, &done, "complete");
        let mut cancelled = h.clone();
        cancelled.tags.clear();
        cancelled.relationships.clear();
        write(&c.b, &cancelled);
        commit(&c.b, "cancel");
        sync(&c.b).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let got = read(&c.b, "SPIKE-1");
        assert!(got.tags.is_empty(), "{name}");
        assert_eq!(
            got.hypothesis_lock.map(|l| l.description_sha256),
            Some(sha(&h.description)),
            "{name}"
        );

        // A stamped base survives both tips dropping the stamp, and a
        // reopened result does not unlock it.
        let mut stamped = h.clone();
        stamped.hypothesis_lock = Some(lock(&h.description, 3));
        let c = clones(&[stamped.clone(), done.clone()]);
        let mut stripped = stamped.clone();
        stripped.hypothesis_lock = None;
        publish(&c.a, &stripped, "strip");
        let mut reopened = done.clone();
        reopened.status = RequirementStatus::Approved;
        write(&c.b, &reopened);
        write(&c.b, &stripped);
        commit(&c.b, "strip and reopen");
        sync(&c.b).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(
            read(&c.b, "SPIKE-1").hypothesis_lock,
            Some(lock(&h.description, 3)),
            "{name}"
        );
    }
}

#[test]
fn bug_2162_newly_eligible_divergent_body_refuses() {
    for (name, sync) in SYNCS {
        let (mut h, r) = rows();
        link(&mut h, &r);
        let c = clones(&[h.clone(), r.clone()]);
        let mut done = r.clone();
        done.status = RequirementStatus::Completed;
        publish(&c.a, &done, "complete without stamping");
        let mut edited = h.clone();
        edited.description = "hedged prediction".into();
        write(&c.b, &edited);
        commit(&c.b, "edit");
        let before = head_sha(&c.b).unwrap();
        let err = sync(&c.b).unwrap_err();
        assert!(is_evidence_refusal(&err), "{name}: {err:#}");
        assert!(format!("{err:#}").contains("differs"), "{name}: {err:#}");
        assert_eq!(head_sha(&c.b).unwrap(), before, "{name}");
    }
}

#[test]
fn bug_2162_protected_identity_or_deletion_refuses() {
    let (mut h, r) = rows();
    link(&mut h, &r);
    h.hypothesis_lock = Some(lock(&h.description, 0));
    let mut done = r.clone();
    done.status = RequirementStatus::Completed;
    type Mutate = fn(&Path, &Requirement);
    let cases: [(&str, Mutate); 3] = [
        ("delete", |b, _| {
            std::fs::remove_file(b.join("objects/SPIKE/000/SPIKE-1.yaml")).unwrap();
        }),
        ("replace uuid", |b, h| {
            let mut other = h.clone();
            other.id = uuid::Uuid::new_v4();
            write(b, &other);
        }),
        ("rename", |b, h| {
            std::fs::remove_file(b.join("objects/SPIKE/000/SPIKE-1.yaml")).unwrap();
            let mut renamed = h.clone();
            renamed.spec_id = Some("SPIKE-9".into());
            write(b, &renamed);
        }),
    ];
    for (name, mutate) in cases {
        let c = clones(&[h.clone(), done.clone()]);
        let mut retitled = h.clone();
        retitled.title = "remote title".into();
        publish(&c.a, &retitled, "retitle");
        mutate(&c.b, &h);
        commit(&c.b, name);
        let before = head_sha(&c.b).unwrap();
        let err = pull_rebase(&c.b, "origin", "aida-store").unwrap_err();
        assert!(is_evidence_refusal(&err), "{name}: {err:#}");
        assert_eq!(head_sha(&c.b).unwrap(), before, "{name}");
    }
}

#[test]
fn bug_2162_strict_snapshot_read_failures_refuse() {
    let (h, r) = rows();
    type Plant = fn(&Path);
    let cases: [(&str, Plant); 4] = [
        ("bad yaml", |a| {
            std::fs::create_dir_all(a.join("objects/TASK/000")).unwrap();
            std::fs::write(a.join("objects/TASK/000/TASK-9.yaml"), "{ not: [yaml").unwrap();
        }),
        ("misnamed", |a| {
            let (_, mut r) = rows();
            r.spec_id = Some("TASK-8".into());
            let text = serde_yaml::to_string(&r).unwrap();
            std::fs::write(a.join("objects/TASK/000/TASK-7.yaml"), text).unwrap();
        }),
        ("bad stamp", |a| {
            let (mut h, _) = rows();
            h.spec_id = Some("SPIKE-5".into());
            h.hypothesis_lock = Some(lock("some other body", 0));
            write(a, &h);
        }),
        ("symlink", |a| {
            #[cfg(unix)]
            std::os::unix::fs::symlink("TASK-2.yaml", a.join("objects/TASK/000/TASK-3.yaml"))
                .unwrap();
            #[cfg(not(unix))]
            std::fs::write(a.join("objects/TASK/000/TASK-3.yaml"), "{").unwrap();
        }),
    ];
    for (name, plant) in cases {
        let c = clones(&[h.clone(), r.clone()]);
        plant(&c.a);
        commit(&c.a, name);
        run(&c.a, &["push", "-q", "origin", "aida-store"]);
        let before = head_sha(&c.b).unwrap();
        let err = pull_rebase(&c.b, "origin", "aida-store").unwrap_err();
        assert!(is_evidence_refusal(&err), "{name}: {err:#}");
        assert_eq!(head_sha(&c.b).unwrap(), before, "{name}");
    }

    // Unrelated histories may join only while neither side holds evidence.
    let c = clones(&[h.clone(), r.clone()]);
    run(&c.b, &["checkout", "-q", "--orphan", "fresh"]);
    run(&c.b, &["rm", "-rqf", "."]);
    std::fs::write(c.b.join("metadata.yaml"), "name: test\n").unwrap();
    let (mut other, _) = rows();
    other.spec_id = Some("SPIKE-4".into());
    write(&c.b, &other);
    commit(&c.b, "unrelated");
    run(&c.b, &["branch", "-q", "-M", "aida-store"]);
    let before = head_sha(&c.b).unwrap();
    let err = pull_rebase(&c.b, "origin", "aida-store").unwrap_err();
    assert!(is_evidence_refusal(&err), "{err:#}");
    assert!(format!("{err:#}").contains("share no history"), "{err:#}");
    assert_eq!(head_sha(&c.b).unwrap(), before, "unrelated histories");
    assert!(!rebase_in_progress(&c.b), "unrelated histories");
    assert!(tracked_clean(&c.b), "unrelated histories");
}

#[test]
fn bug_2162_finalization_failure_never_claims_success_or_resets() {
    for step in [1u8, 2, 3] {
        let (h, r) = rows();
        let c = clones(&[h.clone(), r.clone()]);
        let mut linked = h.clone();
        link(&mut linked, &r);
        publish(&c.a, &linked, "link");
        let mut done = r.clone();
        done.status = RequirementStatus::Completed;
        write(&c.b, &done);
        commit(&c.b, "complete");
        FAIL_FINALIZE_AT.with(|f| f.set(step));
        let err = pull_rebase(&c.b, "origin", "aida-store").unwrap_err();
        FAIL_FINALIZE_AT.with(|f| f.set(0));
        let text = format!("{err:#}");
        assert!(is_evidence_refusal(&err), "step {step}: {text}");
        assert!(text.contains("after Git finished"), "step {step}: {text}");
        assert!(text.contains("Nothing was reset"), "step {step}: {text}");
        assert!(!text.contains("rebase --abort"), "step {step}: {text}");
        // History is kept as Git left it: the rebased commit, no lock commit.
        assert_eq!(subject(&c.b), "complete", "step {step}");
        assert!(!rebase_in_progress(&c.b));
        assert!(read(&c.b, "SPIKE-1").hypothesis_lock.is_none() || step == 3);
    }
}

#[test]
fn bug_2162_sync_lock_serializes_and_reenters() {
    let (h, r) = rows();
    let c = clones(&[h.clone(), r.clone()]);
    let mut retitled = r.clone();
    retitled.title = "remote".into();
    publish(&c.a, &retitled, "remote");
    crate::db::with_store_write_lock(&c.b, || pull_rebase(&c.b, "origin", "aida-store")).unwrap();
    assert_eq!(read(&c.b, "TASK-2").title, "remote");

    // A second writer waits for the sync to release the lock.
    let lock_path = crate::db::store_write_lock_path(&c.b);
    let b = c.b.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let held = std::thread::spawn(move || {
        crate::db::with_store_write_lock(&b, || {
            tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            Ok(std::time::Instant::now())
        })
        .unwrap()
    });
    rx.recv().unwrap();
    assert!(lock_path.exists());
    pull_rebase(&c.b, "origin", "aida-store").unwrap();
    let after = std::time::Instant::now();
    assert!(after >= held.join().unwrap());
}

#[test]
fn bug_2162_untagged_sync_unchanged() {
    let (mut h, r) = rows();
    h.tags.clear();
    link(&mut h, &r);
    let c = clones(&[h.clone(), r.clone()]);
    let mut done = r.clone();
    done.status = RequirementStatus::Completed;
    publish(&c.a, &done, "complete");
    let mut edited = h.clone();
    edited.description = "free to change".into();
    write(&c.b, &edited);
    commit(&c.b, "edit");
    pull_rebase(&c.b, "origin", "aida-store").unwrap();
    assert_eq!(subject(&c.b), "edit");
    let got = read(&c.b, "SPIKE-1");
    assert!(got.hypothesis_lock.is_none());
    assert_eq!(got.description, "free to change");
}

#[test]
fn bug_2162_unchanged_result_row_still_joins_eligibility() {
    // The completed result, byte-identical everywhere and never mentioning a
    // hypothesis, carries the only edge; one clone then tags the target.
    let (mut h, mut r) = rows();
    h.tags.clear();
    r.status = RequirementStatus::Completed;
    r.relationships.push(Relationship {
        rel_type: RelationshipType::VerifiedBy,
        target_id: h.id,
        created_at: None,
        created_by: None,
    });
    let c = clones(&[h.clone(), r.clone()]);
    let mut tagged = h.clone();
    tagged.tags.insert("hypothesis".into());
    publish(&c.a, &tagged, "tag");
    let (_, mut unrelated) = rows();
    unrelated.spec_id = Some("TASK-3".into());
    write(&c.b, &unrelated);
    commit(&c.b, "unrelated");
    pull_rebase(&c.b, "origin", "aida-store").unwrap();
    assert_eq!(
        read(&c.b, "SPIKE-1")
            .hypothesis_lock
            .map(|l| l.description_sha256),
        Some(sha(&h.description))
    );
}

#[test]
fn bug_2162_refusal_survives_context() {
    let err = anyhow::Error::new(crate::git_ops::EvidenceRefusal("x".into()))
        .context("pull")
        .context("coordination");
    assert!(is_evidence_refusal(&err));
    assert!(!is_evidence_refusal(&anyhow::anyhow!("offline")));
}

/// Two clones that each materialize the same newly eligible lock before
/// either pushes write byte-equal lock commits: the stamp comes from the
/// eligibility source, so the second clone's plain rebase drops its own lock
/// commit as already applied instead of conflicting on `locked_at`.
// trace:BUG-2162 | ai:claude
#[test]
fn bug_2162_racing_lock_materialization_converges_on_plain_rebase() {
    let (h, r) = rows();
    let c = clones(&[h.clone(), r.clone()]);
    let third = c.tmp.join("c");
    let hub = c.tmp.join("hub.git");
    run(
        &c.tmp,
        &[
            "clone",
            "-q",
            hub.to_str().unwrap(),
            third.to_str().unwrap(),
        ],
    );
    for (k, v) in [("user.email", "t@example.invalid"), ("user.name", "T")] {
        run(&third, &["config", k, v]);
    }
    // An older writer pushes the link and the completion without a stamp.
    let mut linked = h.clone();
    link(&mut linked, &r);
    write(&c.a, &linked);
    let mut done = r.clone();
    done.status = RequirementStatus::Completed;
    publish(&c.a, &done, "link and complete");

    for clone in [&c.b, &third] {
        pull_rebase(clone, "origin", "aida-store").unwrap();
        assert_eq!(
            subject(clone),
            "aida: lock hypothesis evidence after store sync"
        );
    }
    let stamp = read(&c.b, "SPIKE-1").hypothesis_lock.unwrap();
    assert_eq!(read(&third, "SPIKE-1").hypothesis_lock, Some(stamp.clone()));
    assert_eq!(
        stamp.locked_at,
        linked.modified_at.max(done.modified_at),
        "the stamp is the latest change among the hypothesis and its result"
    );
    run(&c.b, &["push", "-q", "origin", "aida-store"]);
    let published = run(&c.b, &["rev-parse", "HEAD"]);

    pull_rebase(&third, "origin", "aida-store").unwrap();
    assert_eq!(head_sha(&third).unwrap(), published);
    assert!(!rebase_in_progress(&third));
    assert!(tracked_clean(&third));
}

/// A working-tree file whose bytes differ from its blob (an index flag, a
/// clean/smudge filter or eol conversion can make `git status` call it clean)
/// is never read in place of the blob: here the on-disk copy carries a stamp
/// that no longer matches its description, the blob is intact, and the sync
/// succeeds from the blob.
// trace:BUG-2162 | ai:claude
#[test]
fn bug_2162_worktree_text_is_used_only_when_it_hashes_to_the_blob() {
    let (mut h, mut r) = rows();
    link(&mut h, &r);
    r.status = RequirementStatus::Completed;
    h.hypothesis_lock = Some(lock(&h.description, 3));
    let c = clones(&[h.clone(), r.clone()]);
    let mut retitled = r.clone();
    retitled.title = "renamed experiment".into();
    publish(&c.a, &retitled, "retitle");

    let rel = crate::object_store::relative_object_path("SPIKE-1").unwrap();
    let mut tampered = h.clone();
    tampered.description = "not what the blob says".into();
    write(&c.b, &tampered);
    run(&c.b, &["update-index", "--assume-unchanged", &rel]);
    assert!(tracked_clean(&c.b), "git must report the tree clean");

    pull_rebase(&c.b, "origin", "aida-store").unwrap();
    assert_eq!(read(&c.b, "TASK-2").title, "renamed experiment");
}
