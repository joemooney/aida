//! TASK-2325: the agent-scratch scan over fixture job dirs, a fixture TMPDIR
//! root, and fixture build caches. Every rail the acceptance names has a
//! witness: a live job (non-terminal, unreadable state, recent update) is
//! never listed; only the `tmp/` subdir is ever a row; live session dirs are
//! excluded; `--apply` deletes only the listed rows and the delete-site
//! re-check refuses anything else.
// trace:TASK-2325 | ai:claude

use super::*;

const DAY: Duration = Duration::from_secs(24 * 3600);

/// Set the mtime of every entry under `path` (and the path itself) to
/// `age_days` before `now`.
fn age(path: &Path, now: SystemTime, age_days: u64) {
    let when = now - DAY * (age_days as u32);
    let md = std::fs::symlink_metadata(path).unwrap();
    if md.is_dir() {
        for entry in std::fs::read_dir(path).unwrap().flatten() {
            age(&entry.path(), now, age_days);
        }
    }
    std::fs::File::open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

/// A fixture job dir: `<jobs>/<id>/{state.json, timeline.jsonl, tmp/out.o}`.
fn job(jobs_root: &Path, id: &str, state: Option<&str>) -> PathBuf {
    let dir = jobs_root.join(id);
    std::fs::create_dir_all(dir.join("tmp")).unwrap();
    std::fs::write(dir.join("tmp/out.o"), vec![0u8; 64]).unwrap();
    std::fs::write(dir.join("timeline.jsonl"), "{}\n").unwrap();
    if let Some(state) = state {
        std::fs::write(
            dir.join("state.json"),
            format!(r#"{{"state":"{state}","tokens":1}}"#),
        )
        .unwrap();
    }
    dir
}

fn survey(root: &Path, now: SystemTime) -> ScratchSurvey {
    ScratchSurvey {
        jobs_root: Some(root.join("jobs")),
        tmp_root: Some(root.join("tmp-root")),
        build_caches: vec![root.join("impl-target")],
        live_session_dirs: BTreeSet::new(),
        min_age: 3 * DAY,
        now,
    }
}

#[test]
fn a_terminal_old_job_lists_only_its_tmp_dir() {
    let fx = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let jobs = fx.path().join("jobs");
    for (id, state) in [("aaaa1111", "done"), ("bbbb2222", "failed")] {
        let dir = job(&jobs, id, Some(state));
        age(&dir, now, 5);
    }
    let (rows, held) = plan(&survey(fx.path(), now));
    assert_eq!(rows.len(), 2, "{rows:?} {held:?}");
    for row in &rows {
        assert_eq!(row.kind, ScratchKind::JobTmp);
        assert_eq!(row.path.file_name().unwrap(), "tmp");
        assert!(row.reason.contains("terminal job"), "{}", row.reason);
        assert!(row.bytes > 0);
    }
}

#[test]
fn a_live_job_is_never_listed() {
    let fx = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let jobs = fx.path().join("jobs");
    // Non-terminal states, a missing state, and a fresh terminal job.
    age(&job(&jobs, "work1111", Some("working")), now, 9);
    age(&job(&jobs, "blok2222", Some("blocked")), now, 9);
    age(&job(&jobs, "none3333", None), now, 9);
    job(&jobs, "done4444", Some("done")); // fresh: inside the age window
    let (rows, held) = plan(&survey(fx.path(), now));
    assert!(rows.is_empty(), "{rows:?}");
    assert_eq!(held.len(), 4, "{held:?}");
    let reasons: Vec<&str> = held.iter().map(|(_, r)| r.as_str()).collect();
    assert!(reasons.iter().any(|r| r.contains("not terminal")));
    assert!(reasons.iter().any(|r| r.contains("state unreadable")));
    assert!(reasons.iter().any(|r| r.contains("inside the age window")));
}

#[test]
fn session_scratch_ages_and_live_dirs_are_excluded() {
    let fx = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let tmp_root = fx.path().join("tmp-root");
    // A slug dir with two session dirs (one old, one live) and an old loose file.
    let slug = tmp_root.join("-home-user-proj");
    std::fs::create_dir_all(slug.join("old-session")).unwrap();
    std::fs::write(slug.join("old-session/x.txt"), b"x").unwrap();
    std::fs::create_dir_all(slug.join("live-session")).unwrap();
    std::fs::write(slug.join("live-session/x.txt"), b"x").unwrap();
    std::fs::write(tmp_root.join("loose.log"), b"y").unwrap();
    age(&tmp_root, now, 10);
    let mut sv = survey(fx.path(), now);
    sv.live_session_dirs.insert(slug.join("live-session"));
    let (rows, held) = plan(&sv);
    let paths: Vec<&Path> = rows.iter().map(|r| r.path.as_path()).collect();
    assert!(
        paths.contains(&slug.join("old-session").as_path()),
        "{rows:?}"
    );
    assert!(paths.contains(&tmp_root.join("loose.log").as_path()));
    assert!(
        !paths.iter().any(|p| p.ends_with("live-session")),
        "a live session dir is never listed: {rows:?}"
    );
    // The slug dir itself is never a row — only its children are.
    assert!(!paths.contains(&slug.as_path()));
    assert!(held
        .iter()
        .any(|(p, r)| p.ends_with("live-session") && r.contains("live session")));
}

#[test]
fn an_idle_build_cache_is_listed_and_a_warm_one_kept() {
    let fx = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let cache = fx.path().join("impl-target");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("lib.rlib"), vec![0u8; 32]).unwrap();
    age(&cache, now, 11);
    let (rows, _) = plan(&survey(fx.path(), now));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, ScratchKind::BuildCache);
    assert!(rows[0].reason.contains("unused"), "{}", rows[0].reason);

    // Warm again: a fresh write keeps it.
    std::fs::write(cache.join("new.rlib"), b"z").unwrap();
    let (rows, _) = plan(&survey(fx.path(), now));
    assert!(rows.is_empty(), "{rows:?}");
}

/// `--apply` deletes only the listed rows; the job's `state.json` and
/// `timeline.jsonl` survive because only `tmp/` was ever a row.
#[test]
fn apply_deletes_only_listed_rows_and_never_job_records() {
    let fx = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let jobs = fx.path().join("jobs");
    let done = job(&jobs, "aaaa1111", Some("done"));
    age(&done, now, 5);
    let live = job(&jobs, "work2222", Some("working"));
    age(&live, now, 5);
    let sv = survey(fx.path(), now);
    let (rows, _) = plan(&sv);
    assert_eq!(rows.len(), 1);
    let (deleted, failed) = apply(&sv, &rows, |p| std::fs::remove_dir_all(p)).unwrap();
    assert_eq!(deleted.len(), 1);
    assert!(failed.is_empty(), "{failed:?}");
    assert!(!done.join("tmp").exists(), "the listed tmp dir is gone");
    assert!(done.join("state.json").exists(), "job records survive");
    assert!(done.join("timeline.jsonl").exists());
    assert!(live.join("tmp").exists(), "the live job is untouched");
}

/// The delete-site re-check refuses a row outside the surveyed roots — a
/// wrongly built plan cannot reach `remove_dir_all`.
#[test]
fn the_delete_site_recheck_refuses_a_foreign_path() {
    let fx = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let sv = survey(fx.path(), now);
    for (kind, path) in [
        (ScratchKind::JobTmp, fx.path().join("jobs/aaaa1111")), // the job dir, not tmp/
        (ScratchKind::JobTmp, fx.path().join("elsewhere/tmp")),
        (ScratchKind::SessionScratch, fx.path().join("outside")),
        (ScratchKind::BuildCache, fx.path().join("not-a-cache")),
    ] {
        let row = ScratchRow {
            path,
            bytes: 1,
            kind,
            reason: "forged".into(),
            idle: Duration::ZERO,
        };
        let err = apply(&sv, std::slice::from_ref(&row), |_| {
            panic!("a refused row must never reach the delete")
        })
        .unwrap_err();
        assert!(err.to_string().contains("refusing to delete"), "{err}");
    }
}

#[test]
fn tmp_root_and_own_session_dir_are_derived_from_tmpdir() {
    let tmpdir = Path::new("/tmp/claude-1000/-home-user-proj/abcd-1234/scratchpad");
    let root = tmp_root_from(Some(tmpdir)).unwrap();
    assert_eq!(root, Path::new("/tmp/claude-1000"));
    assert_eq!(
        own_session_dir(&root, Some(tmpdir)).unwrap(),
        Path::new("/tmp/claude-1000/-home-user-proj/abcd-1234")
    );
    // A non-slug child of the root is its own entry.
    let loose = Path::new("/tmp/claude-1000/adhoc");
    assert_eq!(
        own_session_dir(Path::new("/tmp/claude-1000"), Some(loose)).unwrap(),
        Path::new("/tmp/claude-1000/adhoc")
    );
    assert!(tmp_root_from(Some(Path::new("/var/tmp/other"))).is_none());
    assert!(tmp_root_from(None).is_none());
}

#[test]
fn terminal_states_are_exactly_the_finished_ones() {
    for s in ["done", "failed", "stopped", "cancelled"] {
        assert!(job_state_terminal(s), "{s}");
    }
    for s in ["working", "blocked", "", "unknown", "Done"] {
        assert!(!job_state_terminal(s), "{s}");
    }
}
