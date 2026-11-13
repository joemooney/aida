// trace:TASK-3780 | ai:codex
use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn unsafe_existing_slots_remain_visible_to_doctor() {
    let tmp = tempfile::tempdir().unwrap();
    let uid = unsafe { libc::geteuid() };
    let slot = tmp.path().join(format!("cargo-slots-{uid}"));
    std::fs::create_dir(&slot).unwrap();
    std::fs::set_permissions(&slot, std::fs::Permissions::from_mode(0o775)).unwrap();
    std::fs::create_dir(slot.join("debug")).unwrap();
    std::fs::write(slot.join("debug/artifact"), "target output").unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[("XDG_RUNTIME_DIR", tmp.path().to_str())]);
    assert_eq!(cargo_slot_dir(), Some(slot.clone()));
    let report =
        disk_headroom_light_report_probing(tmp.path(), |_| Some(100 << 30), cargo_slot_dir(), None);
    assert!(report
        .findings
        .iter()
        .any(|f| f.id == "cargo-slot-permissions"));
    assert!(!cargo_slot_target_artifacts(&slot).is_empty());
    assert!(report
        .findings
        .iter()
        .any(|f| f.id == "cargo-slot-target-dir"));
    assert_eq!(
        std::fs::metadata(&slot).unwrap().permissions().mode() & 0o777,
        0o775
    );
    std::fs::set_permissions(&slot, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(cargo_slot_permission_findings(&slot, uid).is_empty());
    assert!(!cargo_slot_permission_findings(&slot, uid.wrapping_add(1)).is_empty());
}
