//! BUG-053: `aida add` in a contained session.
//!
//! A contained session can write the store worktree but not the main
//! checkout's `.git`, which holds the clone's node-local ID counter. The add
//! mints the final ID under the store write lock and leaves the new object for
//! the host to commit; `aida doctor` reports one still waiting after
//! [`crate::doctor_root::QUEUED_SPEC_MAX_AGE_MINUTES`]; the host sweep
//! commits it under the same ID.
//!
//! trace:BUG-B6B1278890774EF993D2EBC327862470-053 | ai:claude

use aida_core::db::DatabaseBackend;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The sandbox's `denyWrite` on `<main>/.git`, applied to every directory a
/// store write or commit would create files in; restored on drop.
struct DeniedGit(Vec<(PathBuf, std::fs::Permissions)>);

impl DeniedGit {
    fn new(main: &Path) -> Self {
        let git = main.join(".git");
        let dirs = [
            git.clone(),
            git.join("objects"),
            git.join("refs"),
            git.join("worktrees/-aida-store"),
        ];
        DeniedGit(
            dirs.into_iter()
                .map(|d| {
                    let original = std::fs::metadata(&d).unwrap().permissions();
                    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
                    (d, original)
                })
                .collect(),
        )
    }
}

impl Drop for DeniedGit {
    fn drop(&mut self) {
        for (d, original) in &self.0 {
            let _ = std::fs::set_permissions(d, original.clone());
        }
    }
}

fn add_bug(store: &Path, title: &str) -> String {
    let backend = aida_core::GitBackend::new(store).unwrap();
    let mut req = aida_core::models::Requirement::new(title.into(), "fixture".into());
    req.req_type = aida_core::models::RequirementType::Bug;
    backend.add_requirement(req).unwrap().spec_id.unwrap()
}

#[test]
fn contained_add_queues_a_final_id_that_doctor_reports_and_the_host_commits() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipping: root ignores directory write bits");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let main = std::fs::canonicalize(tmp.path()).unwrap().join("main");
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::write(
        main.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git(
        &main,
        &["worktree", "add", "-q", "-b", "aida-store", ".aida-store"],
    );
    let store = main.join(".aida-store");
    std::fs::create_dir_all(store.join("objects")).unwrap();
    std::fs::write(store.join("metadata.yaml"), "name: test\n").unwrap();
    git(&store, &["add", "."]);
    git(&store, &["commit", "-q", "-m", "seed store"]);
    let node = aida_core::work_binding::bootstrap(&store)
        .unwrap()
        .unwrap()
        .node_id;
    // The host installs the store hooks once; a contained session only
    // verifies them.
    aida_core::git_ops::resolve_store_hooks_dir(&store).unwrap();

    let host_first = add_bug(&store, "filed on the host");
    assert_eq!(host_first, format!("BUG-{node}-001"));
    let head_before = git(&store, &["rev-parse", "HEAD"]);

    let (queued_id, queued, findings) = {
        let _denied = DeniedGit::new(&main);
        assert!(aida_core::git_ops::store_commit_deferred_to_host(&store));
        let id = add_bug(&store, "filed in a contained session");
        // Readable in the same session, before any host commit.
        let loaded = aida_core::GitBackend::new(&store).unwrap().load().unwrap();
        assert!(loaded
            .requirements
            .iter()
            .any(|r| r.spec_id.as_deref() == Some(id.as_str())));
        let queued =
            crate::child_store_commit::queued_spec_objects(&store, std::time::Duration::ZERO);
        // Age the object past the doctor threshold.
        let rel = aida_core::object_store::relative_object_path(&id).unwrap();
        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs(
                (crate::doctor_root::QUEUED_SPEC_MAX_AGE_MINUTES + 1) * 60,
            );
        std::fs::File::options()
            .write(true)
            .open(store.join(&rel))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let findings =
            crate::doctor_root::collect_doctor_findings(&main, &loaded, Some("store-worktree"))
                .unwrap();
        (id, queued, findings)
    };

    assert_eq!(queued_id, format!("BUG-{node}-002"));
    assert_eq!(
        git(&store, &["rev-parse", "HEAD"]),
        head_before,
        "a contained add must not commit"
    );
    let rel = aida_core::object_store::relative_object_path(&queued_id).unwrap();
    let stem = Path::new(&rel).file_stem().unwrap().to_str().unwrap();
    assert_eq!(
        queued.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        vec![stem]
    );
    assert!(
        findings
            .iter()
            .any(|f| f.id == "store-queued-specs" && f.summary.contains(stem)),
        "expected a store-queued-specs finding naming {stem}: {findings:?}"
    );

    // The host sweep commits it under the same ID.
    assert!(aida_core::git_ops::commit_pending_store_changes_on_host(&store).unwrap());
    let committed = git(&store, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(
        committed.lines().any(|l| l == rel),
        "host commit must include {rel}: {committed}"
    );
    assert!(
        crate::child_store_commit::queued_spec_objects(&store, std::time::Duration::ZERO)
            .is_empty()
    );
    let loaded = aida_core::GitBackend::new(&store).unwrap().load().unwrap();
    assert!(loaded
        .requirements
        .iter()
        .any(|r| r.spec_id.as_deref() == Some(queued_id.as_str())));
    // The host's counter (still at 1) jumps past the contained mint.
    assert_eq!(add_bug(&store, "host again"), format!("BUG-{node}-003"));
}
