//! BUG-1633: `aida pull` / `aida push` follow-ups to BUG-1625 and BUG-1626 —
//! persisted reconcile scan start after a store failure, followup matching,
//! `--code-only` followup deferral, `push --dry-run` fetching, the
//! multi-URL push skip, and the behind-origin wording. Temp repos only.
// trace:BUG-1633 | ai:claude

use super::*;
use aida_core::db::DatabaseBackend;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} in {} failed: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str) {
    std::fs::write(dir.join(file), file).expect("write");
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", file]);
}

fn clone(src: &Path, dest: &Path, extra: &[&str]) {
    let mut args = vec!["clone", "-q"];
    args.extend_from_slice(extra);
    args.push(src.to_str().unwrap());
    args.push(dest.to_str().unwrap());
    let out = std::process::Command::new("git")
        .args(&args)
        .output()
        .expect("clone");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A bare `name.git` remote seeded with one commit on `branch`, plus a
/// working clone of it at `clone_dir`. Returns the bare path.
fn remote_with_clone(root: &Path, name: &str, branch: &str, clone_dir: &Path) -> PathBuf {
    let bare = root.join(format!("{name}.git"));
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", branch]);
    std::fs::create_dir_all(clone_dir).unwrap();
    git(clone_dir, &["init", "-q", "-b", branch]);
    git(
        clone_dir,
        &["remote", "add", "origin", bare.to_str().unwrap()],
    );
    git(clone_dir, &["config", "user.email", "t@example.invalid"]);
    git(clone_dir, &["config", "user.name", "t"]);
    commit(clone_dir, &format!("{name}-seed.txt"));
    git(clone_dir, &["push", "-q", "-u", "origin", branch]);
    bare
}

/// Code project at `root/proj` (branch `main`, `origin/HEAD` set so the
/// auto-bump sees `main` as the default branch). Returns (proj, code_bare).
fn code_project(root: &Path) -> (PathBuf, PathBuf) {
    let seed = root.join("seed");
    let code_bare = remote_with_clone(root, "code", "main", &seed);
    let proj = root.join("proj");
    clone(&code_bare, &proj, &[]);
    git(&proj, &["config", "user.email", "t@example.invalid"]);
    git(&proj, &["config", "user.name", "t"]);
    std::fs::write(proj.join(".git/info/exclude"), ".aida-store/\n.aida/\n").unwrap();
    (proj, code_bare)
}

/// Push a commit naming `spec_id` to `code_bare`, followed by `fillers`
/// unrelated commits, from a throwaway clone.
fn land_remote_commits(root: &Path, code_bare: &Path, spec_id: &str, fillers: usize) {
    let work = root.join(format!("landing-{spec_id}"));
    clone(code_bare, &work, &[]);
    std::fs::write(work.join("landed.txt"), spec_id).unwrap();
    git(&work, &["add", "landed.txt"]);
    git(
        &work,
        &[
            "commit",
            "-q",
            "-m",
            &format!("feat: teammate work ({spec_id})"),
        ],
    );
    for i in 0..fillers {
        git(
            &work,
            &[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                &format!("chore: filler {i}"),
            ],
        );
    }
    git(&work, &["push", "-q", "origin", "main"]);
}

fn spec(spec_id: &str, status: &str) -> aida_core::Requirement {
    let mut req = aida_core::Requirement::new(format!("test-{spec_id}"), String::new());
    req.spec_id = Some(spec_id.to_string());
    req.set_status_from_str(status);
    req
}

/// A git store at `path` (branch `aida-store`) holding `reqs`, committed.
fn init_store(path: &Path, reqs: Vec<aida_core::Requirement>) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q", "-b", "aida-store"]);
    git(path, &["config", "user.email", "t@example.invalid"]);
    git(path, &["config", "user.name", "t"]);
    let backend = aida_core::db::GitBackend::new(path).unwrap();
    let store = aida_core::RequirementsStore {
        requirements: reqs,
        ..Default::default()
    };
    backend.save(&store).unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-q", "--allow-empty", "-m", "seed store"]);
}

fn write_plan(project_root: &Path, file: &str, spec_id: &str, bullets: &[&str]) -> String {
    let dir = project_root.join("docs/plans");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = format!("# Plan for {spec_id}\n\nSpecs: {spec_id}\n\n## Followups\n\n");
    for b in bullets {
        body.push_str(&format!("- {b}\n"));
    }
    std::fs::write(dir.join(file), body).unwrap();
    format!("docs/plans/{file}")
}

/// Every `(parent, title)` the add hook saw.
type AddCalls = Rc<RefCell<Vec<(String, String)>>>;

/// Install a [`FOLLOWUP_ADD_HOOK`] for the current thread; the returned
/// guard removes it. The shared vec records every `(parent, title)` add.
struct HookGuard;
impl Drop for HookGuard {
    fn drop(&mut self) {
        FOLLOWUP_ADD_HOOK.with(|h| *h.borrow_mut() = None);
    }
}

fn install_add_hook(
    mut f: impl FnMut(&Path, &str, &str, Option<&str>) -> Option<String> + 'static,
) -> (HookGuard, AddCalls) {
    let calls: AddCalls = Rc::default();
    let rec = calls.clone();
    FOLLOWUP_ADD_HOOK.with(|h| {
        *h.borrow_mut() = Some(Box::new(move |root, parent, title, plan| {
            rec.borrow_mut()
                .push((parent.to_string(), title.to_string()));
            f(root, parent, title, plan)
        }))
    });
    (HookGuard, calls)
}

fn state_file(project_root: &Path, name: &str) -> PathBuf {
    reconcile_state_path(project_root, name).expect("git-path")
}

fn followups_marker(store_path: &Path, spec_id: &str) -> Option<String> {
    let store = Storage::new(store_path).load().unwrap();
    store
        .get_requirement_by_spec_id(spec_id)?
        .comments
        .iter()
        .find(|c| c.content.starts_with(FOLLOWUPS_MARKER))
        .map(|c| c.content.clone())
}

// ----- Item 1: the scan start survives a failed store pull -----

/// A pull whose store leg fails persists its scan start; the retry (a no-op
/// code pull) resumes from it instead of the last 50 commits, so a spec named
/// by a commit more than 50 commits back still auto-completes.
#[test]
fn bug_1633_pull_retry_after_store_failure_resumes_persisted_reconcile_scan() {
    let tmp = tempfile::tempdir().unwrap();
    let (proj, code_bare) = code_project(tmp.path());
    let spec_id = "BUG-9811";
    let store_path = proj.join(".aida-store");
    init_store(&store_path, vec![spec(spec_id, "Done")]);
    let hub = tmp.path().join("store-hub.git");
    git(
        &store_path,
        &["remote", "add", "origin", hub.to_str().unwrap()],
    );

    let pre_pull = git(&proj, &["rev-parse", "HEAD"]);
    // The completing commit lands first, then 55 more: outside a 50-wide scan.
    land_remote_commits(tmp.path(), &code_bare, spec_id, 55);

    // Store hub missing → store leg fails, reconcile is deferred.
    assert!(handle_pull_command(&store_path, false, false, true, true, false).is_err());
    let saved = std::fs::read_to_string(state_file(&proj, PENDING_RECONCILE_BASE_STATE))
        .expect("scan start persisted after the store failure");
    assert_eq!(saved.trim(), pre_pull);
    let req_status = |p: &Path| {
        Storage::new(p)
            .load()
            .unwrap()
            .get_requirement_by_spec_id(spec_id)
            .unwrap()
            .status
            .clone()
    };
    assert!(matches!(req_status(&store_path), RequirementStatus::Done));

    // Fix the store remote, then retry: the code pull is now a no-op.
    std::fs::create_dir_all(&hub).unwrap();
    git(&hub, &["init", "-q", "--bare", "-b", "aida-store"]);
    git(&store_path, &["push", "-q", "origin", "aida-store"]);
    handle_pull_command(&store_path, false, false, true, true, false).expect("retry pull");

    assert!(
        matches!(req_status(&store_path), RequirementStatus::Completed),
        "the retry must scan from the persisted start, not only the last 50 commits"
    );
    assert!(
        !state_file(&proj, PENDING_RECONCILE_BASE_STATE).exists(),
        "a completed reconcile clears the persisted scan start"
    );
}

/// Review fix: a pull that returns without scanning (HEAD on a feature
/// branch) must NOT clear the persisted scan start — the next pull on the
/// default branch still resumes from it.
#[test]
fn bug_1633_pull_reconcile_base_survives_a_feature_branch_pull() {
    let tmp = tempfile::tempdir().unwrap();
    let (proj, code_bare) = code_project(tmp.path());
    let spec_id = "BUG-9821";
    let store_path = proj.join(".aida-store");
    init_store(&store_path, vec![spec(spec_id, "Done")]);
    let hub = tmp.path().join("store-hub.git");
    git(
        &store_path,
        &["remote", "add", "origin", hub.to_str().unwrap()],
    );
    let pre_pull = git(&proj, &["rev-parse", "HEAD"]);
    land_remote_commits(tmp.path(), &code_bare, spec_id, 55);

    // Store fails on main → base persisted.
    assert!(handle_pull_command(&store_path, false, false, true, true, false).is_err());
    let base_file = state_file(&proj, PENDING_RECONCILE_BASE_STATE);
    assert_eq!(
        std::fs::read_to_string(&base_file).unwrap().trim(),
        pre_pull
    );

    // Store fixed; a successful pull on a feature branch does not scan.
    std::fs::create_dir_all(&hub).unwrap();
    git(&hub, &["init", "-q", "--bare", "-b", "aida-store"]);
    git(&store_path, &["push", "-q", "origin", "aida-store"]);
    git(&proj, &["checkout", "-q", "-b", "feature"]);
    git(&proj, &["push", "-q", "-u", "origin", "feature"]);
    handle_pull_command(&store_path, false, false, true, true, false).expect("feature pull");
    assert!(
        base_file.exists(),
        "no scan ran on the feature branch, so the base must be kept"
    );
    let status = |p: &Path| {
        Storage::new(p)
            .load()
            .unwrap()
            .get_requirement_by_spec_id(spec_id)
            .unwrap()
            .status
            .clone()
    };
    assert!(matches!(status(&store_path), RequirementStatus::Done));

    // Back on main: the no-op pull resumes from the kept base.
    git(&proj, &["checkout", "-q", "main"]);
    handle_pull_command(&store_path, false, false, true, true, false).expect("main pull");
    assert!(matches!(status(&store_path), RequirementStatus::Completed));
    assert!(!base_file.exists());
}

// ----- Item 2: followup filing -----

/// A failed `aida add` whose followup another clone filed meanwhile (tagged
/// with THIS plan) is recorded as already filed, not declined.
#[test]
fn bug_1633_followup_failed_add_counts_as_already_filed() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("proj");
    std::fs::create_dir_all(&project_root).unwrap();
    let parent = "BUG-9812";
    let bullet = "Harden the frobnicator";
    let plan = write_plan(&project_root, "2026-01-01-frob.md", parent, &[bullet]);
    let store_path = project_root.join(".aida-store");
    init_store(&store_path, vec![spec(parent, "Completed")]);

    let hook_store = store_path.clone();
    let (_guard, calls) = install_add_hook(move |_, _, title, plan| {
        // Another clone files it first; our add then fails.
        let backend = aida_core::db::GitBackend::new(&hook_store).unwrap();
        let mut store = backend.load().unwrap();
        let mut r = aida_core::Requirement::new(title.to_string(), String::new());
        r.spec_id = Some("TASK-9813".to_string());
        r.tags
            .insert(format!("{FOLLOWUP_SRC_TAG_PREFIX}{}", plan.unwrap()));
        store.requirements.push(r);
        backend.save(&store).unwrap();
        None
    });

    let storage = Storage::new(store_path.clone());
    extract_plan_followups(&storage, &project_root, parent, parent, false).unwrap();

    assert_eq!(calls.borrow().len(), 1);
    let marker = followups_marker(&store_path, parent).expect("marker written");
    assert!(marker.contains(&plan), "{marker}");
    assert!(marker.contains("skipped 1 already-filed"), "{marker}");
    assert!(!marker.contains("declined"), "{marker}");
}

/// A same-titled spec tagged with a DIFFERENT plan's followup tag does not
/// turn a failed add into "already filed".
#[test]
fn bug_1633_followup_failed_add_other_plan_tag_is_declined() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("proj");
    std::fs::create_dir_all(&project_root).unwrap();
    let parent = "BUG-9814";
    let bullet = "Polish the gizmo";
    write_plan(&project_root, "2026-01-02-gizmo.md", parent, &[bullet]);
    let store_path = project_root.join(".aida-store");
    let mut unrelated = aida_core::Requirement::new(bullet.to_string(), String::new());
    unrelated.spec_id = Some("TASK-9815".to_string());
    unrelated.tags.insert(format!(
        "{FOLLOWUP_SRC_TAG_PREFIX}docs/plans/some-other-plan.md"
    ));
    init_store(&store_path, vec![spec(parent, "Completed"), unrelated]);

    let (_guard, calls) = install_add_hook(|_, _, _, _| None);
    let storage = Storage::new(store_path.clone());
    extract_plan_followups(&storage, &project_root, parent, parent, false).unwrap();

    assert_eq!(calls.borrow().len(), 1);
    let marker = followups_marker(&store_path, parent).expect("marker written");
    assert!(marker.contains("declined 1"), "{marker}");
    assert!(!marker.contains("already-filed"), "{marker}");
}

/// End to end: the followup exists only in the REMOTE store (another clone
/// filed it). `aida pull` syncs the store first, so the auto-bump's followup
/// extraction sees it and files no duplicate.
#[test]
fn bug_1633_pull_followup_filed_only_remotely_is_not_refiled() {
    let tmp = tempfile::tempdir().unwrap();
    let (proj, code_bare) = code_project(tmp.path());
    let parent = "BUG-9816";
    let bullet = "Document the sprocket";
    let plan = write_plan(&proj, "2026-01-03-sprocket.md", parent, &[bullet]);

    // Store hub + the other machine's clone; both start with parent at Done.
    let other = tmp.path().join("other-store");
    let hub = remote_with_clone(tmp.path(), "store-hub", "aida-store", &other);
    {
        let backend = aida_core::db::GitBackend::new(&other).unwrap();
        let store = aida_core::RequirementsStore {
            requirements: vec![spec(parent, "Done")],
            ..Default::default()
        };
        backend.save(&store).unwrap();
        git(&other, &["add", "-A"]);
        git(
            &other,
            &["commit", "-q", "--allow-empty", "-m", "parent at Done"],
        );
        git(&other, &["push", "-q", "origin", "aida-store"]);
    }
    let store_path = proj.join(".aida-store");
    clone(&hub, &store_path, &["--branch", "aida-store"]);
    git(&store_path, &["config", "user.email", "t@example.invalid"]);
    git(&store_path, &["config", "user.name", "t"]);

    // The other machine files the followup as a child of the parent.
    {
        let backend = aida_core::db::GitBackend::new(&other).unwrap();
        let mut store = backend.load().unwrap();
        let mut child = aida_core::Requirement::new(bullet.to_string(), String::new());
        child.spec_id = Some("TASK-9817".to_string());
        child
            .tags
            .insert(format!("{FOLLOWUP_SRC_TAG_PREFIX}{plan}"));
        let child_id = child.id;
        store.requirements.push(child);
        let p = store
            .requirements
            .iter_mut()
            .find(|r| r.spec_id.as_deref() == Some(parent))
            .unwrap();
        p.relationships.push(aida_core::Relationship {
            rel_type: aida_core::RelationshipType::Parent,
            target_id: child_id,
            created_at: None,
            created_by: None,
        });
        backend.save(&store).unwrap();
        git(&other, &["add", "-A"]);
        git(
            &other,
            &["commit", "-q", "--allow-empty", "-m", "file followup"],
        );
        git(&other, &["push", "-q", "origin", "aida-store"]);
    }
    land_remote_commits(tmp.path(), &code_bare, parent, 0);

    let (_guard, calls) = install_add_hook(|_, _, _, _| Some("TASK-DUP".to_string()));
    handle_pull_command(&store_path, false, false, true, true, false).expect("pull");

    assert!(
        calls.borrow().is_empty(),
        "a followup already filed remotely must not be re-added: {:?}",
        calls.borrow()
    );
    let after = Storage::new(store_path.clone()).load().unwrap();
    assert!(matches!(
        after.get_requirement_by_spec_id(parent).unwrap().status,
        RequirementStatus::Completed
    ));
    let copies = after
        .requirements
        .iter()
        .filter(|r| r.title == bullet)
        .count();
    assert_eq!(copies, 1);
    let marker = followups_marker(&store_path, parent).expect("marker written");
    assert!(marker.contains("skipped 1 already-filed"), "{marker}");
}

// ----- Item 3: --code-only defers followup filing -----

/// `aida pull --code-only` against a store with a remote flips the spec but
/// defers its plan followups; the next full pull files them.
#[test]
fn bug_1633_pull_code_only_defers_followup_extraction_to_next_full_pull() {
    let tmp = tempfile::tempdir().unwrap();
    let (proj, code_bare) = code_project(tmp.path());
    let parent = "BUG-9818";
    write_plan(&proj, "2026-01-04-widget.md", parent, &["Tune the widget"]);
    let store_path = proj.join(".aida-store");
    init_store(&store_path, vec![spec(parent, "Done")]);
    let hub = tmp.path().join("store-hub.git");
    std::fs::create_dir_all(&hub).unwrap();
    git(&hub, &["init", "-q", "--bare", "-b", "aida-store"]);
    git(
        &store_path,
        &["remote", "add", "origin", hub.to_str().unwrap()],
    );
    git(&store_path, &["push", "-q", "origin", "aida-store"]);
    land_remote_commits(tmp.path(), &code_bare, parent, 0);

    let (_guard, calls) = install_add_hook(|_, _, _, _| Some("TASK-9819".to_string()));
    handle_pull_command(&store_path, true, false, true, true, false).expect("code-only pull");

    let after = Storage::new(store_path.clone()).load().unwrap();
    assert!(matches!(
        after.get_requirement_by_spec_id(parent).unwrap().status,
        RequirementStatus::Completed
    ));
    assert!(
        calls.borrow().is_empty(),
        "--code-only must not file followups"
    );
    assert!(followups_marker(&store_path, parent).is_none());
    assert_eq!(
        read_reconcile_state_lines(&proj, PENDING_FOLLOWUPS_STATE),
        vec![parent.to_string()]
    );

    // The next full pull syncs the store, then files the deferred followups.
    handle_pull_command(&store_path, false, false, true, true, false).expect("full pull");
    assert_eq!(calls.borrow().len(), 1, "{:?}", calls.borrow());
    assert!(followups_marker(&store_path, parent).is_some());
    assert!(!state_file(&proj, PENDING_FOLLOWUPS_STATE).exists());
}

/// Review fix: a deferred followup whose extraction errors stays pending;
/// only filed ids are removed, and an id appended concurrently (another
/// `--code-only` pull) while the drain runs is kept.
#[test]
fn bug_1633_followup_drain_keeps_failed_and_concurrent_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("proj");
    std::fs::create_dir_all(&project_root).unwrap();
    git(&project_root, &["init", "-q", "-b", "main"]);
    let parent = "BUG-9822";
    write_plan(
        &project_root,
        "2026-01-05-drain.md",
        parent,
        &["Retune the drain"],
    );
    let store_path = project_root.join(".aida-store");
    init_store(&store_path, vec![spec(parent, "Completed")]);
    write_reconcile_state(&project_root, PENDING_FOLLOWUPS_STATE, parent);

    // A store that cannot be read: extraction errors, the id is kept.
    let broken = tmp.path().join("broken.yaml");
    std::fs::write(&broken, "requirements: [ : : not yaml\n").unwrap();
    let broken_storage = Storage::new(&broken);
    assert!(broken_storage.load().is_err(), "fixture must fail to load");
    let kept = drain_pending_followups(&project_root, &broken_storage);
    assert_eq!(kept, vec![parent.to_string()]);
    assert_eq!(
        read_reconcile_state_lines(&project_root, PENDING_FOLLOWUPS_STATE),
        vec![parent.to_string()]
    );

    // Healthy store: the id is filed and removed; an id appended by a
    // concurrent `--code-only` pull mid-drain survives.
    let hook_root = project_root.clone();
    let (_guard, calls) = install_add_hook(move |_, _, _, _| {
        update_pending_followups(&hook_root, |ids| ids.push("BUG-9823".to_string()));
        Some("TASK-9824".to_string())
    });
    let kept = drain_pending_followups(&project_root, &Storage::new(&store_path));
    assert_eq!(calls.borrow().len(), 1);
    assert_eq!(kept, vec!["BUG-9823".to_string()]);
    assert_eq!(
        read_reconcile_state_lines(&project_root, PENDING_FOLLOWUPS_STATE),
        vec!["BUG-9823".to_string()]
    );
    assert!(followups_marker(&store_path, parent).is_some());
}

// ----- Items 4-6: aida push -----

/// Project with a code remote and an orphan-store remote, as `aida push` sees
/// it. Returns (proj, code_bare, store_bare).
fn push_project(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let proj = root.join("proj");
    let code_bare = remote_with_clone(root, "code", "main", &proj);
    std::fs::write(proj.join(".git/info/exclude"), ".aida-store/\n.aida/\n").unwrap();
    let store_bare = remote_with_clone(root, "store", "aida-store", &proj.join(".aida-store"));
    (proj, code_bare, store_bare)
}

fn advance_remote(root: &Path, bare: &Path, name: &str) {
    let other = root.join(format!("other-{name}"));
    clone(bare, &other, &[]);
    git(&other, &["config", "user.email", "t@example.invalid"]);
    git(&other, &["config", "user.name", "t"]);
    commit(&other, &format!("{name}-remote-advance.txt"));
    git(&other, &["push", "-q", "origin", "HEAD"]);
}

/// `aida push --dry-run` fetches first: a remote that advanced since the last
/// fetch shows up as a divergence instead of a clean "1 commit to push".
#[test]
fn bug_1633_push_dry_run_fetches_before_reporting() {
    let tmp = tempfile::tempdir().unwrap();
    let (proj, code_bare, store_bare) = push_project(tmp.path());
    advance_remote(tmp.path(), &code_bare, "code");
    advance_remote(tmp.path(), &store_bare, "store");
    commit(&proj, "local-only.txt");
    // The cached ref still says "1 ahead, 0 behind".
    assert_eq!(
        ahead_behind_vs_ref(&proj, "main", "origin/main"),
        Some((1, 0))
    );

    let legs = push_dry_run_legs(
        &proj.join(".aida-store"),
        false,
        false,
        PUSH_REMOTE_REFRESH_TIMEOUT,
    );
    let code = legs.iter().find(|l| l.label == "code").unwrap();
    assert!(code.summary.contains("DIVERGED"), "{}", code.summary);
    let store = legs.iter().find(|l| l.label == "store").unwrap();
    assert!(
        store.summary.contains("origin has 1 store commit"),
        "{}",
        store.summary
    );

    // An unreachable remote within the budget says the counts are cached.
    let gone = tmp.path().join("gone.git");
    git(
        &proj,
        &["remote", "set-url", "origin", gone.to_str().unwrap()],
    );
    let legs = push_dry_run_legs(
        &proj.join(".aida-store"),
        true,
        false,
        PUSH_REMOTE_REFRESH_TIMEOUT,
    );
    assert!(
        legs[0].summary.contains("remote state unknown"),
        "{}",
        legs[0].summary
    );
}

/// With two `remote.origin.url` entries, `git push` pushes to both, but the
/// fetch only refreshed one — the skip must not be taken, so a lagging
/// second URL is caught up.
#[test]
fn bug_1633_push_skip_requires_exactly_one_origin_url() {
    let tmp = tempfile::tempdir().unwrap();
    let (proj, code_bare, _) = push_project(tmp.path());
    let second = tmp.path().join("second.git");
    clone(&code_bare, &second, &["--bare"]);
    commit(&proj, "only-on-first-url.txt");
    git(&proj, &["push", "-q", "origin", "main"]);
    git(
        &proj,
        &[
            "remote",
            "set-url",
            "--add",
            "origin",
            second.to_str().unwrap(),
        ],
    );
    let head = git(&proj, &["rev-parse", "HEAD"]);

    assert!(!origin_push_url_matches_fetch(&proj));
    let plan = CodeLegPlan::gather(&proj, PUSH_REMOTE_REFRESH_TIMEOUT);
    assert_eq!(plan.ahead_behind, Some((0, 0)));
    assert!(!plan.nothing_to_push(), "two fetch URLs must not skip");

    handle_push_command(&proj.join(".aida-store"), true, false, None, true, true)
        .expect("push to both URLs");
    assert_eq!(git(&second, &["rev-parse", "refs/heads/main"]), head);
}

/// Behind origin with nothing to push: the summary says "nothing to push",
/// never "already up to date".
#[test]
fn bug_1633_push_behind_origin_summary_says_nothing_to_push() {
    let msg = push_failure_summary(
        &PushLegOutcome::NothingToPush,
        &PushLegOutcome::Failed("rejected".into()),
        "main",
    )
    .unwrap();
    assert!(msg.contains("code leg nothing to push"), "{msg}");
    assert!(!msg.contains("up to date"), "{msg}");

    // End to end: code behind origin, store diverged → the failure summary
    // names the code leg as "nothing to push".
    let tmp = tempfile::tempdir().unwrap();
    let (proj, code_bare, store_bare) = push_project(tmp.path());
    let store = proj.join(".aida-store");
    advance_remote(tmp.path(), &code_bare, "code");
    advance_remote(tmp.path(), &store_bare, "store");
    commit(&store, "store-local.txt");
    let err = handle_push_command(&store, false, false, None, true, true)
        .expect_err("the diverged store leg fails");
    let msg = err.to_string();
    assert!(msg.contains("code leg nothing to push"), "{msg}");
    assert!(!msg.contains("already up to date"), "{msg}");
}

/// BUG-1648: plan paths are recorded with `/` on every OS, and markers and
/// `followup-src:` tags that older Windows builds wrote with `\` still dedup
/// when read back.
// trace:BUG-1648 | ai:claude
#[test]
fn bug_1648_backslash_plan_paths_read_back_as_forward_slash() {
    let root = Path::new("proj");
    assert_eq!(
        plan_rel_path(&root.join("docs").join("plans").join("x.md"), root),
        "docs/plans/x.md"
    );

    let marker = format!(
        "{FOLLOWUPS_MARKER} extracted from docs/plans\\a.md, docs/plans/b.md\nfiled 0 task(s)"
    );
    assert_eq!(
        parse_extracted_plans_from_marker(&marker),
        vec!["docs/plans/a.md".to_string(), "docs/plans/b.md".to_string()]
    );

    let mut parent = spec("BUG-9900", "Completed");
    let mut child = aida_core::Requirement::new("Harden it".to_string(), String::new());
    child.spec_id = Some("TASK-9901".to_string());
    child
        .tags
        .insert(format!("{FOLLOWUP_SRC_TAG_PREFIX}docs/plans\\a.md"));
    parent.relationships.clear();
    let store = aida_core::RequirementsStore {
        requirements: vec![parent, child],
        ..Default::default()
    };
    assert!(followup_filed_in_store(
        &store,
        "BUG-9900",
        "harden it",
        "docs/plans/a.md"
    ));
    assert!(!followup_filed_in_store(
        &store,
        "BUG-9900",
        "harden it",
        "docs/plans/b.md"
    ));
}
