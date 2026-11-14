//! STORY-2828: `[mandate] max_ttl_hours`. Absent, a mandate is capped at 24h
//! as before; the global config raises the cap; a project config cannot; and
//! 168h is the ceiling whatever any config says.
// trace:STORY-2828 | ai:claude

use super::*;
use crate::gate::seat_authority::{resolve_hour_cap_from, HourCap, MANDATE_MAX_TTL_KEY};

fn t0() -> DateTime<Utc> {
    "2026-11-14T12:00:00Z".parse().unwrap()
}

fn human_ok() -> Result<()> {
    Ok(())
}

fn config_dir(body: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(dir.path().join(".aida").join("config.toml"), body).unwrap();
    dir
}

/// The cap the loader resolves from a global and a project config, pinned
/// for this test's thread until dropped.
struct Pinned;

impl Pinned {
    fn configs(home: &str, project: &str) -> Self {
        let (home, project) = (config_dir(home), config_dir(project));
        let cap: HourCap = resolve_hour_cap_from(
            Some(home.path()),
            Some(project.path()),
            MANDATE_MAX_TTL_KEY,
            MAX_TTL.num_hours() as u32,
        );
        TEST_MAX_TTL.with(|c| c.set(Some(cap)));
        Pinned
    }
}

impl Drop for Pinned {
    fn drop(&mut self) {
        TEST_MAX_TTL.with(|c| c.set(None));
    }
}

fn start_for(dir: &Path, hours: i64) -> Result<Mandate> {
    start(
        dir,
        &human_ok,
        "joe",
        Duration::hours(hours),
        None,
        false,
        t0(),
    )
}

fn err_of<T>(r: Result<T>) -> String {
    match r {
        Ok(_) => panic!("expected a refusal"),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn with_the_key_absent_25h_is_refused_as_before() {
    let _cap = Pinned::configs("", "");
    assert_eq!(parse_ttl("24h").unwrap(), Duration::hours(24));
    let err = err_of(parse_ttl("25h"));
    assert!(err.contains("--ttl may be at most 24h"), "{err}");
    assert!(err.contains("aida mandate renew"), "{err}");
    // It names where the cap comes from and the key that raises it.
    assert!(
        err.contains("[mandate] max_ttl_hours in ~/.aida/config.toml"),
        "{err}"
    );
    let err = err_of(parse_renew_for("25h"));
    assert!(err.contains("--for may be at most 24h"), "{err}");
    let tmp = tempfile::tempdir().unwrap();
    let err = err_of(start_for(tmp.path(), 25));
    assert!(err.contains("at most 24h"), "{err}");
    assert!(load_valid(tmp.path(), "joe", t0()).0.is_empty());
}

#[test]
fn a_global_72_lets_a_72h_mandate_start_and_stay_valid() {
    let tmp = tempfile::tempdir().unwrap();
    let cap = Pinned::configs("[mandate]\nmax_ttl_hours = 72\n", "");
    assert_eq!(parse_ttl("72h").unwrap(), Duration::hours(72));
    let m = start_for(tmp.path(), 72).unwrap();
    assert_eq!(m.expires_at, t0() + Duration::hours(72));
    // Reloaded two days in, the record is still the active mandate.
    let later = t0() + Duration::hours(48);
    let (valid, warnings) = load_valid(tmp.path(), "joe", later);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(state_at(&valid, later), MandateState::Active(m.clone()));
    // A renewal runs up to the same cap from now.
    assert_eq!(parse_renew_for("72h").unwrap(), Duration::hours(72));
    let (renewed, _) = renew(
        tmp.path(),
        &human_ok,
        "joe",
        Some(Duration::hours(72)),
        later,
    )
    .unwrap();
    assert_eq!(renewed.expires_at, later + Duration::hours(72));
    assert!(load_valid(tmp.path(), "joe", later).1.is_empty());
    // Past the configured cap is refused, naming the cap and its source.
    let err = err_of(parse_ttl("73h"));
    assert!(err.contains("--ttl may be at most 72h"), "{err}");
    assert!(
        err.contains("[mandate] max_ttl_hours in ~/.aida/config.toml"),
        "{err}"
    );
    // Fail closed: with the key removed again, the long record is ignored.
    drop(cap);
    let _cap = Pinned::configs("", "");
    let (valid, warnings) = load_valid(tmp.path(), "joe", later);
    assert!(valid.is_empty(), "{valid:?}");
    assert!(
        warnings.iter().any(|w| w.contains("24h cap")),
        "{warnings:?}"
    );
}

#[test]
fn hours_169_are_refused_whatever_the_config_says() {
    for home in [
        "",
        "[mandate]\nmax_ttl_hours = 168\n",
        "[mandate]\nmax_ttl_hours = 169\n",
        "[mandate]\nmax_ttl_hours = 100000\n",
    ] {
        let _cap = Pinned::configs(home, "[mandate]\nmax_ttl_hours = 100000\n");
        assert!(parse_ttl("169h").is_err(), "{home}");
        assert!(parse_renew_for("169h").is_err(), "{home}");
        let tmp = tempfile::tempdir().unwrap();
        assert!(start_for(tmp.path(), 169).is_err(), "{home}");
        assert!(load_valid(tmp.path(), "joe", t0()).0.is_empty(), "{home}");
    }
    let _cap = Pinned::configs("[mandate]\nmax_ttl_hours = 100000\n", "");
    assert_eq!(parse_ttl("168h").unwrap(), Duration::hours(168));
    let err = err_of(parse_ttl("169h"));
    assert!(err.contains("--ttl may be at most 168h"), "{err}");
    assert!(err.contains("not configurable"), "{err}");
}

#[test]
fn a_project_config_cannot_raise_the_cap() {
    {
        let _cap = Pinned::configs("", "[mandate]\nmax_ttl_hours = 72\n");
        assert!(parse_ttl("25h").is_err());
        let tmp = tempfile::tempdir().unwrap();
        assert!(start_for(tmp.path(), 72).is_err());
        assert!(start_for(tmp.path(), 24).is_ok());
    }
    {
        let _cap = Pinned::configs(
            "[mandate]\nmax_ttl_hours = 48\n",
            "[mandate]\nmax_ttl_hours = 72\n",
        );
        assert!(parse_ttl("48h").is_ok());
        assert!(parse_ttl("49h").is_err());
    }
    // It can lower it, and the refusal says the project did.
    let _cap = Pinned::configs(
        "[mandate]\nmax_ttl_hours = 72\n",
        "[mandate]\nmax_ttl_hours = 4\n",
    );
    assert!(parse_ttl("4h").is_ok());
    let err = err_of(parse_ttl("5h"));
    assert!(err.contains("--ttl may be at most 4h"), "{err}");
    assert!(err.contains("this project's .aida/config.toml"), "{err}");
}
