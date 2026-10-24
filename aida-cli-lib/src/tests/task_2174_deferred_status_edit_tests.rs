//! TASK-2174 (TASK-1955): `edit_requirement_cli` itself refuses a status edit
//! on a deferred spec — the flag or a legacy `deferred:*` tag — and leaves the
//! stored status alone, while an autopilot reversal
//! ([`DeferredStatus::Restore`]) still restores the status it overwrote.
// trace:TASK-1955 | ai:claude (TASK-2174)

use super::*;

fn project() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let ok = crate::test_git::git_command()
        .current_dir(&root)
        .args(["init", "--initial-branch=main", "--quiet"])
        .status()
        .unwrap()
        .success();
    assert!(ok, "git init");
    let store_path = root.join("requirements.yaml");
    Storage::new(&store_path)
        .save(&aida_core::RequirementsStore::default())
        .unwrap();
    (tmp, store_path)
}

fn seed(store_path: &std::path::Path, spec_id: &str, edit: impl Fn(&mut Requirement)) {
    let storage = Storage::new(store_path);
    let mut store = storage.load().unwrap();
    let mut req = Requirement::new(format!("test-{spec_id}"), String::new());
    req.spec_id = Some(spec_id.to_string());
    req.status = RequirementStatus::Approved;
    edit(&mut req);
    store.requirements.push(req);
    storage.save(&store).unwrap();
}

fn status_of(store_path: &std::path::Path, spec_id: &str) -> RequirementStatus {
    let store = Storage::new(store_path).load().unwrap();
    store
        .get_requirement_by_spec_id(spec_id)
        .unwrap()
        .status
        .clone()
}

fn edit_status(storage: &Storage, id: &str, status: &str) -> anyhow::Result<()> {
    edit_requirement_cli(
        storage,
        id,
        &None,
        &None,
        &Some(status.to_string()),
        &None,
        &None,
        &None,
        &None,
        &None,
        &[],
        &[],
    )
}

#[test]
fn edit_requirement_cli_refuses_a_status_edit_on_a_deferred_spec() {
    let (_tmp, store_path) = project();
    let storage = Storage::new(&store_path);
    seed(&store_path, "TASK-90001", |r| r.deferred = true);
    seed(&store_path, "TASK-90002", |r| {
        r.tags.insert("deferred:after-release".to_string());
    });

    let err = edit_status(&storage, "TASK-90001", "completed").unwrap_err();
    assert!(err.to_string().contains("aida undefer TASK-90001"), "{err}");
    assert_eq!(
        status_of(&store_path, "TASK-90001"),
        RequirementStatus::Approved
    );

    let err = edit_status(&storage, "TASK-90002", "completed").unwrap_err();
    assert!(
        err.to_string()
            .contains("--remove-tag deferred:after-release"),
        "{err}"
    );
    assert_eq!(
        status_of(&store_path, "TASK-90002"),
        RequirementStatus::Approved
    );
}

#[test]
fn an_autopilot_restore_writes_the_stored_status_of_a_deferred_spec() {
    let (_tmp, store_path) = project();
    let storage = Storage::new(&store_path);
    seed(&store_path, "TASK-90003", |r| r.deferred = true);
    edit_requirement_cli_with(
        &storage,
        "TASK-90003",
        &EditFields {
            title: &None,
            description: &None,
            status: &Some("planned".to_string()),
            priority: &None,
            req_type: &None,
            owner: &None,
            feature: &None,
            tags: &None,
            add_tag: &[],
            remove_tag: &[],
        },
        DeferredStatus::Restore,
    )
    .unwrap();
    assert_eq!(
        status_of(&store_path, "TASK-90003"),
        RequirementStatus::Planned
    );
}
