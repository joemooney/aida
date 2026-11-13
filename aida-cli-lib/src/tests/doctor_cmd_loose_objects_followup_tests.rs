// trace:TASK-3780 | ai:codex
use super::*;

// trace:TASK-2197 | ai:codex
#[test]
fn loose_object_check_aliases_run_without_a_loadable_store() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(tmp.path())
        .status()
        .unwrap()
        .success());
    std::fs::create_dir(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        "[doctor.loose_objects]\nmax = \"bad\"\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join(".aida-store"), "unloadable store").unwrap();
    assert!(Storage::new(&tmp.path().join(".aida-store"))
        .load()
        .is_err());
    std::env::set_current_dir(tmp.path()).unwrap();
    for alias in ["loose-objects", "loose-object", "git-objects", "repack"] {
        assert_eq!(
            normalize_doctor_category(alias).unwrap(),
            crate::loose_objects::CATEGORY
        );
        let check = cli::DoctorCommand::Check {
            category: alias.into(),
            all: false,
            json: true,
            fail_on_findings: false,
        };
        handle_doctor_command(false, false, None, true, false, false, None, Some(&check)).unwrap();
        let check = cli::DoctorCommand::Check {
            category: alias.into(),
            all: false,
            json: true,
            fail_on_findings: true,
        };
        let error =
            handle_doctor_command(false, false, None, true, false, false, None, Some(&check))
                .unwrap_err();
        // JSON output owns the diagnostic; the dispatcher receives its
        // typed nonzero-exit marker. trace:TASK-2197 | ai:codex
        assert!(error.downcast_ref::<crate::TypedPayloadEmitted>().is_some());
    }
}

// Pin the opt-in append call separately from the light dispatch. The
// scan's actual config/count behavior is covered by loose_objects tests.
// trace:TASK-2053 trace:TASK-2198 | ai:codex
#[test]
fn full_doctor_keeps_the_loose_object_scan_and_trust_comment_together() {
    let source = include_str!("../doctor_cmd.rs");
    let full = source
        .split_once("fn doctor_multi_agent(opts:")
        .unwrap()
        .1
        .split_once("fn doctor_check_store_free_light(")
        .unwrap()
        .0;
    assert!(full.contains("if doctor_category_selected(opts.category.as_deref(), crate::loose_objects::CATEGORY)? {\n        findings.extend(crate::loose_objects::scan(&project_root));"));
    let trust = full
        .split_once("// BUG-1700: the folder-trust gate")
        .unwrap()
        .1;
    assert!(trust
        .split_once("scan_agent_worktree_trust")
        .unwrap()
        .0
        .contains("\"agent-launch\""));
    assert!(!trust
        .split_once("scan_agent_worktree_trust")
        .unwrap()
        .0
        .contains("loose_objects"));
}
