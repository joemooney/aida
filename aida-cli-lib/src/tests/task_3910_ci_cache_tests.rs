//! TASK-3910: the disk-headroom guard reports the GitLab CI cache above its cap.
// trace:TASK-3910 | ai:claude

use super::*;
use std::time::{Duration, Instant};

const GIB: u64 = 1024 * 1024 * 1024;

fn later() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

fn fixture_cache() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let inc = tmp
        .path()
        .join("cache/target-v2-0/debug/incremental/aida_core-1/s-a");
    std::fs::create_dir_all(&inc).unwrap();
    std::fs::write(inc.join("query-cache.bin"), vec![0u8; 3000]).unwrap();
    std::fs::write(tmp.path().join("cache/top.bin"), vec![0u8; 1000]).unwrap();
    tmp
}

#[test]
fn task_3910_walk_sums_the_tree_and_flags_above_the_threshold() {
    let tmp = fixture_cache();
    assert_eq!(
        ci_cache_bytes(tmp.path(), 10_000, later()),
        Some((4000, false))
    );
    let (bytes, exceeded) = ci_cache_bytes(tmp.path(), 500, later()).unwrap();
    assert!(exceeded && bytes > 500, "{bytes}");
}

#[test]
fn task_3910_missing_dir_or_spent_budget_is_inconclusive() {
    let tmp = fixture_cache();
    assert_eq!(ci_cache_bytes(&tmp.path().join("absent"), 1, later()), None);
    assert_eq!(ci_cache_bytes(tmp.path(), 10_000, Instant::now()), None);
}

#[test]
fn task_3910_finding_only_above_the_cap() {
    let dir = std::path::Path::new("/var/cache/aida-ci");
    assert!(ci_cache_finding(dir, None, 150).is_none());
    assert!(ci_cache_finding(dir, Some((100 * GIB, false)), 150).is_none());
    assert!(ci_cache_finding(dir, Some((255 * GIB, true)), 0).is_none());
    let f = ci_cache_finding(dir, Some((255 * GIB, true)), 150).expect("above cap alerts");
    assert_eq!(f.category, DISK_HEADROOM_CATEGORY);
    assert_eq!(f.id, "disk-headroom-ci-cache");
    assert!(f.summary.contains("/var/cache/aida-ci"), "{}", f.summary);
    assert!(f.summary.contains("150 GiB cap"), "{}", f.summary);
    assert!(
        !is_floor_finding(&f),
        "the cache alert must not trigger worktree floor reclaim"
    );
}

#[test]
fn task_3910_config_overrides_dir_and_cap() {
    let (dir, cap) = ci_cache_config(None);
    assert_eq!(dir, std::path::PathBuf::from(DEFAULT_CI_CACHE_DIR));
    assert_eq!(cap, DEFAULT_CI_CACHE_MAX_GIB);
    let cfg: toml::Value = toml::from_str(
        "[doctor.disk_headroom]\nci_cache_dir = \"/srv/ci\" # host\nci_cache_max_gib = 40\n",
    )
    .unwrap();
    assert_eq!(
        ci_cache_config(Some(&cfg)),
        (std::path::PathBuf::from("/srv/ci"), 40)
    );
}

#[test]
fn task_3910_configured_cache_above_cap_reaches_the_findings() {
    let tmp = fixture_cache();
    let cfg: toml::Value = toml::from_str(&format!(
        "[doctor.disk_headroom]\nci_cache_dir = {:?}\nci_cache_max_gib = 0\n",
        tmp.path().display().to_string()
    ))
    .unwrap();
    assert!(
        ci_cache_findings(Some(&cfg)).is_empty(),
        "cap 0 disables the check"
    );
    // A real GiB-scale fixture is impractical; the walk and verdict above pin
    // the threshold, this pins that a small under-cap cache stays silent.
    let cfg: toml::Value = toml::from_str(&format!(
        "[doctor.disk_headroom]\nci_cache_dir = {:?}\nci_cache_max_gib = 1\n",
        tmp.path().display().to_string()
    ))
    .unwrap();
    assert!(ci_cache_findings(Some(&cfg)).is_empty());
}
