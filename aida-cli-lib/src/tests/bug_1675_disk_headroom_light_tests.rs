//! Tests for BUG-1675: `aida doctor check disk-headroom` takes a light entry
//! path that never loads the AIDA store.
//
// trace:BUG-1675 | ai:claude

use super::*;

const GIB: u64 = 1024 * 1024 * 1024;

/// A project root whose `.aida-store` is a regular file, so any
/// `Storage::load()` of it fails. The light path must not care.
fn project_with_unloadable_store(min_free_gib: u64) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida").join("config.toml"),
        format!("[doctor.disk_headroom]\nmin_free_gib = {min_free_gib}\n"),
    )
    .unwrap();
    std::fs::write(tmp.path().join(".aida-store"), b"not a store\n").unwrap();
    tmp
}

/// The fixture really is unloadable through the full path's loader.
#[test]
fn bug_1675_fixture_store_is_unloadable() {
    let tmp = project_with_unloadable_store(1);
    assert!(
        Storage::new(&tmp.path().join(".aida-store"))
            .load()
            .is_err(),
        "fixture must be a store the full doctor path cannot load"
    );
}

/// Healthy floor: no finding, and the store was never needed. The free-space
/// reading is injected so the verdict does not depend on the host's disk.
#[test]
fn bug_1675_light_path_reports_healthy_without_loading_store() {
    let tmp = project_with_unloadable_store(60);
    let report = disk_headroom_light_report_with(tmp.path(), |_| Some(100 * GIB));
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert_eq!(report.total, 0);
    assert!(report.healed.is_empty());
    assert!(report.runaway_seats.is_none());
}

/// Below the configured floor: exactly one disk-headroom finding, still
/// without the store, and the same finding the full path's scanner builds
/// from the same reading.
#[test]
fn bug_1675_light_path_reports_below_floor_without_loading_store() {
    let tmp = project_with_unloadable_store(60);
    let report = disk_headroom_light_report_with(tmp.path(), |_| Some(16 * 1024));
    assert_eq!(report.total, 1);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].category, DISK_HEADROOM_CATEGORY);
    assert_eq!(report.findings[0].id, DISK_HEADROOM_CATEGORY);
    assert!(!report.findings[0].safe_heal);
    assert!(
        report.findings[0].summary.contains("60 GiB"),
        "{}",
        report.findings[0].summary
    );
    let full = disk_headroom_findings(Some(16 * 1024), 60);
    assert_eq!(full.len(), 1);
    assert_eq!(report.findings[0].category, full[0].category);
    assert_eq!(report.findings[0].id, full[0].id);
    assert_eq!(
        report.findings[0].summary, full[0].summary,
        "light path and full-path scanner must agree"
    );
}

/// The configured floor, not the default, decides: 50 GiB free clears a
/// 40 GiB floor but not the 60 GiB default.
#[test]
fn bug_1675_light_path_honors_configured_floor() {
    let tuned = project_with_unloadable_store(40);
    assert!(
        disk_headroom_light_report_with(tuned.path(), |_| Some(50 * GIB))
            .findings
            .is_empty()
    );
    let untuned = tempfile::tempdir().unwrap();
    std::fs::write(untuned.path().join(".aida-store"), b"not a store\n").unwrap();
    assert_eq!(
        disk_headroom_light_report_with(untuned.path(), |_| Some(50 * GIB))
            .findings
            .len(),
        1
    );
}

/// An unresolvable filesystem stays silent (no false disk trip), matching the
/// full path.
#[test]
fn bug_1675_light_path_unresolved_disk_is_silent() {
    let tmp = project_with_unloadable_store(60);
    let report = disk_headroom_light_report_with(tmp.path(), |_| None);
    assert!(report.findings.is_empty());
    assert_eq!(report.total, 0);
}

/// No config at all: the default floor applies (60 GiB), the report still
/// renders, and JSON keeps the full path's field names.
#[test]
fn bug_1675_light_path_without_config_uses_default_and_serializes() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".aida-store"), b"not a store\n").unwrap();
    let report = disk_headroom_light_report_with(tmp.path(), |_| Some(59 * GIB));
    assert_eq!(report.findings.len(), 1, "default 60 GiB floor applies");
    let json = serde_json::to_value(&report).unwrap();
    assert!(json.get("total").is_some());
    assert!(json.get("findings").is_some());
    assert_eq!(
        json["total"].as_u64().unwrap() as usize,
        report.findings.len()
    );
}

/// The production entry wires the real probe: an impossible floor against the
/// real filesystem reports (proves path -> mount -> free-space wiring).
#[test]
fn bug_1675_light_report_wires_the_real_disk_probe() {
    let tmp = project_with_unloadable_store(u64::MAX / GIB);
    let report = disk_headroom_light_report(tmp.path());
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].category, DISK_HEADROOM_CATEGORY);
}
