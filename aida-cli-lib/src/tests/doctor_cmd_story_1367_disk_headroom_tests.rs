// trace:TASK-3780 | ai:codex
use super::*;

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn healthy_headroom_is_silent() {
    assert!(disk_headroom_finding(100 * GIB, 60).is_none());
    assert!(disk_headroom_finding(60 * GIB, 60).is_none());
}

#[test]
fn low_headroom_reports_once_with_the_floor_and_measured_free_space() {
    let finding = disk_headroom_finding(16 * 1024, 60).expect("below-floor must report");
    assert_eq!(finding.category, "disk-headroom");
    assert!(finding.summary.contains("60 GiB"), "{}", finding.summary);
    assert!(!finding.safe_heal, "never auto-healed — operator decision");
}

#[test]
fn min_free_gib_reads_config_and_falls_back_to_the_default() {
    assert_eq!(
        disk_headroom_min_free_gib(None),
        DEFAULT_DISK_HEADROOM_MIN_FREE_GIB
    );
    let cfg: toml::Value = "[doctor.disk_headroom]\nmin_free_gib = 200\n"
        .parse()
        .unwrap();
    assert_eq!(disk_headroom_min_free_gib(Some(&cfg)), 200);
    // A non-positive override is ignored rather than manufacturing an
    // always-failing (0) or always-passing check.
    let zero: toml::Value = "[doctor.disk_headroom]\nmin_free_gib = 0\n"
        .parse()
        .unwrap();
    assert_eq!(
        disk_headroom_min_free_gib(Some(&zero)),
        DEFAULT_DISK_HEADROOM_MIN_FREE_GIB
    );
}

/// TASK-1527: the cache-refresh scan is silent with no request and with a
/// fresh one, reports the crash loop once suppression engages, and
/// reports a request nothing has retired within two tick cadences.
// trace:TASK-1527 | ai:claude
#[test]
fn scan_cache_refresh_reports_crash_loops_and_stuck_requests_only() {
    use aida_core::db::refresh_request;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let cache = aida_core::CachedGitBackend::default_cache_path(&root.join(".aida-store"));
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    // No request: silent.
    assert!(scan_cache_refresh(root).is_empty());
    // A fresh pending request is normal operation: silent.
    refresh_request::file_request(&cache, "abc123").unwrap();
    assert!(scan_cache_refresh(root).is_empty());
    // Three failures inside the window: the crash-loop finding, alone.
    for n in 0..3 {
        refresh_request::record_failed_attempt(&cache, &format!("exit {n}")).unwrap();
    }
    let findings = scan_cache_refresh(root);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "cache-refresh-crash-loop");
    assert!(findings[0].summary.contains("exit 2"));
    // An old request with no failures: the stuck finding.
    refresh_request::clear(&cache).unwrap();
    let mut request = refresh_request::file_request(&cache, "abc123").unwrap();
    request.requested_at = (chrono::Utc::now()
        - chrono::Duration::minutes(CACHE_REFRESH_REQUEST_STUCK_MINS + 1))
    .to_rfc3339();
    let body = serde_json::to_string(&request).unwrap();
    std::fs::write(refresh_request::refresh_request_path(&cache), body).unwrap();
    let findings = scan_cache_refresh(root);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "cache-refresh-request-stuck");
}

#[test]
fn scan_is_silent_against_a_floor_the_real_disk_always_clears() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(scan_disk_headroom(tmp.path(), 0).is_empty());
}

#[test]
fn scan_reports_against_a_floor_no_real_disk_clears() {
    let tmp = tempfile::tempdir().unwrap();
    let findings = scan_disk_headroom(tmp.path(), u64::MAX / (1024 * 1024 * 1024));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].category, "disk-headroom");
}
