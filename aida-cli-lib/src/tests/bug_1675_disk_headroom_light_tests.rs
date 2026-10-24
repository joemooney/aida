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
    let report = disk_headroom_light_report_with(tmp.path(), |_| Some(100 * GIB), None);
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
    let report = disk_headroom_light_report_with(tmp.path(), |_| Some(16 * 1024), None);
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
        disk_headroom_light_report_with(tuned.path(), |_| Some(50 * GIB), None)
            .findings
            .is_empty()
    );
    let untuned = tempfile::tempdir().unwrap();
    std::fs::write(untuned.path().join(".aida-store"), b"not a store\n").unwrap();
    assert_eq!(
        disk_headroom_light_report_with(untuned.path(), |_| Some(50 * GIB), None)
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
    let report = disk_headroom_light_report_with(tmp.path(), |_| None, None);
    assert!(report.findings.is_empty());
    assert_eq!(report.total, 0);
}

/// No config at all: the default floor applies (60 GiB), the report still
/// renders, and JSON keeps the full path's field names.
#[test]
fn bug_1675_light_path_without_config_uses_default_and_serializes() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".aida-store"), b"not a store\n").unwrap();
    let report = disk_headroom_light_report_with(tmp.path(), |_| Some(59 * GIB), None);
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
    // BUG-1702 added host-dependent Cargo-slot findings to this same category, so assert the
    // project-root verdict by id rather than by a total count the host can change.
    // trace:BUG-1702 | ai:claude
    let headroom = report
        .findings
        .iter()
        .find(|f| f.id == "disk-headroom")
        .expect("the real disk probe must produce the project-root headroom finding");
    assert_eq!(headroom.category, DISK_HEADROOM_CATEGORY);
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.category == DISK_HEADROOM_CATEGORY),
        "every finding from this entry belongs to the category: {:?}",
        report.findings
    );
}

/// BUG-1702: the runtime tmpfs holding the Cargo slot locks filled to 0 bytes free because a
/// Cargo target directory was pointed at it. A seat's build then died at link time with
/// "Disk full?" and a bus error, which is not a compile failure and must not be read as one.
/// The artifacts and the free space are separate findings because they need separate actions.
// trace:BUG-1702 | ai:claude
#[test]
fn cargo_slot_findings_separate_the_cause_from_the_symptom() {
    const GIB: u64 = 1024 * 1024 * 1024;

    // Healthy: room to link, nothing but lock files.
    assert!(cargo_slot_findings(Some(6 * GIB), &[], 1).is_empty());

    // The observed state: 0 bytes free AND a target directory sitting in the slot dir.
    let artifacts = vec!["debug/".to_string(), ".rustc_info.json".to_string()];
    let findings = cargo_slot_findings(Some(0), &artifacts, 1);
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["cargo-slot-target-dir", "cargo-slot-free"]);
    assert!(findings.iter().all(|f| f.category == "disk-headroom"));
    assert!(
        findings.iter().all(|f| !f.safe_heal),
        "deleting build state is never an automatic heal"
    );

    // The cause names the artifacts and warns off CARGO_TARGET_DIR; the symptom explains the
    // failure mode so an orchestrator does not retry it as if the code were wrong.
    assert!(findings[0].summary.contains("debug/"), "{:?}", findings[0]);
    assert!(findings[0].action.contains("CARGO_TARGET_DIR"));
    assert!(findings[0].action.contains(".lock"), "keep the lock files");
    assert!(
        findings[1].summary.contains("Disk full?"),
        "{:?}",
        findings[1]
    );
    assert!(findings[1].action.contains("not failing on"));

    // Artifacts present but plenty of room: still the cause, not yet the symptom.
    let findings = cargo_slot_findings(Some(6 * GIB), &artifacts, 1);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "cargo-slot-target-dir");

    // Low space with a clean slot dir: the symptom alone (something else filled the tmpfs).
    let findings = cargo_slot_findings(Some(100 * 1024 * 1024), &[], 1);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "cargo-slot-free");

    // An unresolved filesystem stays silent rather than risking a false positive, matching
    // the BUG-1675 contract above.
    assert!(cargo_slot_findings(None, &[], 1).is_empty());
}

/// BUG-1702: only a POPULATED profile directory or a real target marker counts. An empty `tmp/`
/// is ordinary scratch, and a slot directory holding just its lock files must stay silent.
// trace:BUG-1702 | ai:claude
#[test]
fn cargo_slot_target_artifacts_ignores_lock_files_and_empty_scratch() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();

    // The legitimate contents: advisory lock files only.
    for lock in ["slot-0.lock", "slot-1.lock", "queue.lock"] {
        std::fs::write(dir.join(lock), "").unwrap();
    }
    std::fs::create_dir_all(dir.join("tmp")).unwrap();
    assert!(
        cargo_slot_target_artifacts(dir).is_empty(),
        "lock files and an empty tmp/ are not build artifacts"
    );

    // A populated profile directory is.
    std::fs::create_dir_all(dir.join("debug")).unwrap();
    std::fs::write(dir.join("debug/aida"), "binary").unwrap();
    std::fs::write(dir.join(".rustc_info.json"), "{}").unwrap();
    let found = cargo_slot_target_artifacts(dir);
    assert!(found.contains(&"debug/".to_string()), "{found:?}");
    assert!(found.contains(&".rustc_info.json".to_string()), "{found:?}");
}

/// A floor no real filesystem meets, so the tempdir is always below it.
const HUGE_FLOOR_GIB: u64 = 1_000_000_000;

/// TASK-2264 (folded into STORY-1916): a manual light check below the floor
/// reports the breach and the status block, and never runs the emergency
/// tier or the stuck scan (a reclaim dry run: `gh`, the offline merge scan,
/// a walk of every cache).
// trace:STORY-1916.Visibility | ai:claude
#[test]
fn story_1916_manual_light_check_makes_no_gh_call_and_no_cache_walk() {
    let tmp = project_with_unloadable_store(HUGE_FLOOR_GIB);
    let cfg = crate::read_project_config_value(tmp.path());
    let report = disk_headroom_light_report_finish(
        tmp.path(),
        cfg.as_ref(),
        false,
        || panic!("a manual check must not run the emergency tier"),
        || disk_headroom_light_report_with(tmp.path(), |_| Some(0), None),
        |_, _| panic!("a manual check must not run the reclaim scan (gh, cache walk)"),
    );
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].id, "disk-headroom");
    assert!(!report.findings[0].summary.contains("STUCK"));
    let status = report.disk_status.expect("status block");
    assert_eq!(status.floor_gib, HUGE_FLOOR_GIB);
}

/// The substrate run does run both, and a scan that cannot cover the
/// deficit marks the finding STUCK.
// trace:STORY-1916.ac641c6e | ai:claude
#[test]
fn story_1916_substrate_run_scans_and_marks_a_stuck_floor() {
    let tmp = project_with_unloadable_store(HUGE_FLOOR_GIB);
    let cfg = crate::read_project_config_value(tmp.path());
    let ran = std::cell::Cell::new(false);
    let scans = std::cell::Cell::new(0);
    let report = disk_headroom_light_report_finish(
        tmp.path(),
        cfg.as_ref(),
        true,
        || ran.set(true),
        || disk_headroom_light_report_with(tmp.path(), |_| Some(0), None),
        |_, _| {
            scans.set(scans.get() + 1);
            (GIB, Vec::new())
        },
    );
    assert!(ran.get(), "the substrate run drives the emergency tier");
    assert_eq!(scans.get(), 1);
    assert!(
        report.findings[0].summary.contains("STUCK"),
        "{}",
        report.findings[0].summary
    );
}

/// AC3: the `--fail-on-findings` reason is one line that leads with the
/// STUCK sentence, so the route's last-stderr-line evidence carries it.
// trace:STORY-1916.ac641c6e | ai:claude
#[test]
fn story_1916_fail_reason_leads_with_the_stuck_sentence() {
    assert_eq!(disk_headroom_fail_reason(&[]), None);
    let mut finding = disk_headroom_finding(GIB, 60).unwrap();
    let plain = disk_headroom_fail_reason(std::slice::from_ref(&finding)).unwrap();
    assert!(
        plain.starts_with("1 finding(s) in disk-headroom"),
        "{plain}"
    );
    finding.summary = format!(
        "{}. {}",
        finding.summary,
        crate::disk_emergency::stuck_summary(
            GIB,
            59 * GIB,
            &[(std::path::PathBuf::from("/t/worktrees/take-2845"), 39 * GIB)]
        )
    );
    let reason = disk_headroom_fail_reason(&[finding]).unwrap();
    assert!(reason.starts_with("STUCK:"), "{reason}");
    assert!(
        reason.contains("39.0 GiB /t/worktrees/take-2845"),
        "{reason}"
    );
    assert!(
        reason.contains("failing because --fail-on-findings"),
        "{reason}"
    );
    assert_eq!(reason.lines().count(), 1, "one stderr line: {reason}");
}

/// AC4: the default agent (TOON) rendering carries the status block too.
// trace:STORY-1916.Visibility | ai:claude
#[test]
fn story_1916_toon_report_carries_the_disk_status() {
    let tmp = project_with_unloadable_store(60);
    let cfg = crate::read_project_config_value(tmp.path());
    let report = disk_headroom_light_report_finish(
        tmp.path(),
        cfg.as_ref(),
        false,
        || {},
        || disk_headroom_light_report_with(tmp.path(), |_| Some(100 * GIB), None),
        |_, _| (0, Vec::new()),
    );
    let toon = render_doctor_report_toon(&report);
    for want in [
        "disk_floor_gib",
        "disk_ceiling_pct",
        "disk_last_guard_run",
        "disk_last_reclaim",
    ] {
        assert!(toon.contains(want), "missing {want}: {toon}");
    }
}
