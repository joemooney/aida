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
    if md.file_type().is_symlink() {
        return;
    }
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
    let done_reclaim = apply(&sv, &rows, None, |_| None, |p| std::fs::remove_dir_all(p)).unwrap();
    assert_eq!(done_reclaim.deleted.len(), 1);
    assert!(done_reclaim.failed.is_empty(), "{:?}", done_reclaim.failed);
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
        (ScratchKind::JobTmp, fx.path().join("elsewhere").join("tmp")),
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
        let err = apply(
            &sv,
            std::slice::from_ref(&row),
            None,
            |_| None,
            |_| panic!("a refused row must never reach the delete"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("refusing to delete"), "{err}");
    }
}

/// A loose file under the TMPDIR root prices as its own length — the tree
/// walk alone sums only directory entries (PR-2906 round 1).
#[test]
fn a_loose_file_row_reports_its_own_size() {
    let fx = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let tmp_root = fx.path().join("tmp-root");
    std::fs::create_dir_all(&tmp_root).unwrap();
    std::fs::write(tmp_root.join("loose.log"), vec![0u8; 4096]).unwrap();
    age(&tmp_root, now, 10);
    let (rows, _) = plan(&survey(fx.path(), now));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].path, tmp_root.join("loose.log"));
    assert_eq!(rows[0].bytes, 4096, "a file prices as its own length");
}

#[test]
fn tmp_root_and_own_session_dir_are_derived_from_tmpdir() {
    // Pure path derivation; every fixture path is built from a tempdir so
    // nothing names a real system location (PR-2906 round 1).
    let base = tempfile::tempdir().unwrap();
    let claude_root = base.path().join("claude-1000");
    let tmpdir = claude_root.join("-home-user-proj/abcd-1234/scratchpad");
    let root = tmp_root_from(Some(&tmpdir)).unwrap();
    assert_eq!(root, claude_root);
    assert_eq!(
        own_session_dir(&root, Some(&tmpdir)).unwrap(),
        claude_root.join("-home-user-proj/abcd-1234")
    );
    // A non-slug child of the root is its own entry.
    let loose = claude_root.join("adhoc");
    assert_eq!(
        own_session_dir(&claude_root, Some(&loose)).unwrap(),
        claude_root.join("adhoc")
    );
    assert!(tmp_root_from(Some(&base.path().join("var-tmp/other"))).is_none());
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

/// Requirement 2: `plan_tmp` and `plan_build_caches` filter symlinks before
/// aging/pricing, including slug children and dangling links, so `apply`
/// never aborts mid-run on a static link and external link targets are never
/// priced or touched.
// trace:TASK-2325 | ai:agy
#[test]
fn task_2325_symlinks_under_tmp_and_build_caches_are_filtered_at_plan_time() {
    let fx = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let tmp_root = fx.path().join("tmp-root");
    let slug = tmp_root.join("-home-user-proj");
    std::fs::create_dir_all(slug.join("real-session")).unwrap();
    std::fs::write(slug.join("real-session/data.bin"), vec![0u8; 128]).unwrap();

    // External target tree that must not be priced or deleted.
    let ext = fx.path().join("external-target");
    std::fs::create_dir_all(&ext).unwrap();
    std::fs::write(ext.join("precious.bin"), vec![0u8; 8192]).unwrap();
    age(&ext, now, 10);

    // Symlinks: top-level pointing to ext, top-level dangling, slug-child
    // pointing to ext, slug-child dangling, slug-named symlink, and build-cache
    // symlink.
    std::os::unix::fs::symlink(&ext, tmp_root.join("top-link")).unwrap();
    std::os::unix::fs::symlink(fx.path().join("missing-1"), tmp_root.join("top-dangling")).unwrap();
    std::os::unix::fs::symlink(&ext, slug.join("slug-link")).unwrap();
    std::os::unix::fs::symlink(fx.path().join("missing-2"), slug.join("slug-dangling")).unwrap();
    std::os::unix::fs::symlink(&ext, tmp_root.join("-slug-symlink")).unwrap();
    let cache_link = fx.path().join("impl-target");
    std::os::unix::fs::symlink(&ext, &cache_link).unwrap();

    age(&tmp_root, now, 10);

    let sv = survey(fx.path(), now);
    let (rows, _) = plan(&sv);
    assert_eq!(
        rows.len(),
        1,
        "only real-session should be planned: {rows:?}"
    );
    assert_eq!(rows[0].path, slug.join("real-session"));
    assert_eq!(rows[0].bytes, 128);

    let res = apply(&sv, &rows, None, |_| None, |p| std::fs::remove_dir_all(p)).unwrap();
    assert_eq!(res.deleted.len(), 1);
    assert!(res.failed.is_empty());
    assert!(!slug.join("real-session").exists());
    assert!(
        ext.join("precious.bin").exists(),
        "external target untouched"
    );
}

/// Requirement 3: `apply` honors `--target-pct` across scratch rows, probing
/// before each deletion and stopping when the filesystem reaches or is
/// already at the target percentage.
// trace:TASK-2325 | ai:agy
#[test]
fn task_2325_apply_honors_target_pct_and_leaves_remaining_scratch_intact() {
    use std::cell::Cell;
    let fx = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let tmp_root = fx.path().join("tmp-root");
    let slug = tmp_root.join("-home-user-proj");
    std::fs::create_dir_all(slug.join("sess-a")).unwrap();
    std::fs::write(slug.join("sess-a/a.bin"), vec![0u8; 500]).unwrap();
    std::fs::create_dir_all(slug.join("sess-b")).unwrap();
    std::fs::write(slug.join("sess-b/b.bin"), vec![0u8; 300]).unwrap();
    std::fs::create_dir_all(slug.join("sess-c")).unwrap();
    std::fs::write(slug.join("sess-c/c.bin"), vec![0u8; 100]).unwrap();
    age(&tmp_root, now, 10);

    let sv = survey(fx.path(), now);
    let (rows, _) = plan(&sv);
    assert_eq!(rows.len(), 3);

    // Case 1: already at target (80% <= 80%) -> deletes nothing, all 3 stopped_early.
    let already = apply(
        &sv,
        &rows,
        Some(80),
        |_| Some(80),
        |p| std::fs::remove_dir_all(p),
    )
    .unwrap();
    assert!(already.deleted.is_empty());
    assert_eq!(already.stopped_early.len(), 3);
    assert!(slug.join("sess-a").exists());

    // Case 2: reaches target after first deletion (85% -> 80% -> 80%) -> deletes
    // largest row (sess-a) and leaves sess-b and sess-c intact.
    let pct = Cell::new(85u8);
    let partial = apply(
        &sv,
        &rows,
        Some(80),
        |_| Some(pct.get()),
        |p| {
            std::fs::remove_dir_all(p)?;
            pct.set(pct.get() - 5);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(partial.deleted.len(), 1);
    assert_eq!(partial.deleted[0].0, slug.join("sess-a"));
    assert_eq!(partial.stopped_early.len(), 2);
    assert!(!slug.join("sess-a").exists());
    assert!(slug.join("sess-b").exists(), "remaining scratch left warm");
    assert!(slug.join("sess-c").exists(), "remaining scratch left warm");
}

/// Requirement 7: `aged` / `walk_cache` uses lstat semantics so a nested
/// symlink inside a scratch directory to a large, recently modified external
/// tree neither inflates reported bytes nor pins the scratch directory's age.
// trace:TASK-2325 | ai:agy
#[test]
fn task_2325_nested_symlink_to_external_tree_does_not_inflate_bytes_or_pin_age() {
    let fx = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let jobs = fx.path().join("jobs");
    let done = job(&jobs, "aaaa1111", Some("done"));

    // External tree with a large, freshly written file.
    let ext = fx.path().join("external-fresh");
    std::fs::create_dir_all(&ext).unwrap();
    std::fs::write(ext.join("huge.bin"), vec![0u8; 65536]).unwrap();

    // Symlink inside the job's tmp/ pointing to the fresh external tree.
    std::os::unix::fs::symlink(&ext, done.join("tmp/ext-link")).unwrap();
    age(&done, now, 6);
    // Keep external tree fresh (now).
    std::fs::write(ext.join("huge.bin"), vec![0u8; 65536]).unwrap();

    let sv = survey(fx.path(), now);
    let (rows, held) = plan(&sv);
    assert_eq!(
        rows.len(),
        1,
        "fresh external symlink target must not pin job age: {rows:?} {held:?}"
    );
    assert_eq!(
        rows[0].bytes, 64,
        "external symlink target bytes must not be counted in tmp/ size"
    );
}
