//! STORY-2828: the one loader behind every lifetime cap. The global config
//! sets a cap, a project config may only lower it, and nothing passes the
//! 168h hard ceiling. Covers `[mandate] max_ttl_hours` and
//! `[seat] inline_grant_max_hours`.
// trace:STORY-2828 | ai:claude

use super::*;

/// A directory whose `.aida/config.toml` holds `body` (a home or a project).
fn config_dir(body: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(dir.path().join(".aida").join("config.toml"), body).unwrap();
    dir
}

fn mandate_cap(home: &str, project: &str) -> HourCap {
    let (home, project) = (config_dir(home), config_dir(project));
    resolve_hour_cap_from(
        Some(home.path()),
        Some(project.path()),
        MANDATE_MAX_TTL_KEY,
        24,
    )
}

fn inline_hours(home: &str, project: &str) -> i64 {
    let (home, project) = (config_dir(home), config_dir(project));
    inline_parent_max_from(Some(home.path()), project.path()).num_hours()
}

#[test]
fn an_absent_key_is_the_default() {
    let cap = mandate_cap("", "");
    assert_eq!((cap.hours, cap.source), (24, CapSource::Default));
    // No home and no project at all reads the same.
    let cap = resolve_hour_cap_from(None, None, MANDATE_MAX_TTL_KEY, 24);
    assert_eq!((cap.hours, cap.source), (24, CapSource::Default));
    // Another section's key of the same name is not this key.
    let cap = mandate_cap("[seat]\nmax_ttl_hours = 72\n", "");
    assert_eq!(cap.hours, 24);
}

#[test]
fn the_global_config_raises_the_cap() {
    let cap = mandate_cap("[mandate]\nmax_ttl_hours = 72\n", "");
    assert_eq!((cap.hours, cap.source), (72, CapSource::Global));
}

#[test]
fn nothing_passes_the_hard_ceiling() {
    let cap = mandate_cap("[mandate]\nmax_ttl_hours = 200\n", "");
    assert_eq!(cap.hours, HARD_CEILING_GRANT_HOURS);
    let cap = mandate_cap(
        "[mandate]\nmax_ttl_hours = 200\n",
        "[mandate]\nmax_ttl_hours = 500\n",
    );
    assert_eq!(cap.hours, HARD_CEILING_GRANT_HOURS);
}

#[test]
fn a_project_config_lowers_and_never_raises() {
    // Alone, above the default: ignored.
    let cap = mandate_cap("", "[mandate]\nmax_ttl_hours = 72\n");
    assert_eq!((cap.hours, cap.source), (24, CapSource::Default));
    // Above the global value: ignored.
    let cap = mandate_cap(
        "[mandate]\nmax_ttl_hours = 48\n",
        "[mandate]\nmax_ttl_hours = 100\n",
    );
    assert_eq!((cap.hours, cap.source), (48, CapSource::Global));
    // Below the global value, or below the default: it lowers.
    let cap = mandate_cap(
        "[mandate]\nmax_ttl_hours = 72\n",
        "[mandate]\nmax_ttl_hours = 12\n",
    );
    assert_eq!((cap.hours, cap.source), (12, CapSource::Project));
    let cap = mandate_cap("", "[mandate]\nmax_ttl_hours = 12\n");
    assert_eq!((cap.hours, cap.source), (12, CapSource::Project));
}

#[test]
fn a_malformed_value_is_the_default() {
    for body in [
        "[mandate]\nmax_ttl_hours = \"72\"\n",
        "[mandate]\nmax_ttl_hours = -3\n",
        "[mandate\nmax_ttl_hours = 72\n",
    ] {
        assert_eq!(mandate_cap(body, "").hours, 24, "{body}");
    }
}

#[test]
fn the_refusal_origin_names_the_place_and_the_key() {
    let key = MANDATE_MAX_TTL_KEY;
    let default = mandate_cap("", "").origin(key);
    assert!(default.contains("the default"), "{default}");
    assert!(
        default.contains("[mandate] max_ttl_hours in ~/.aida/config.toml"),
        "{default}"
    );
    assert!(default.contains("168h"), "{default}");
    let global = mandate_cap("[mandate]\nmax_ttl_hours = 72\n", "").origin(key);
    assert!(
        global.starts_with("[mandate] max_ttl_hours in ~/.aida/config.toml"),
        "{global}"
    );
    let ceiling = mandate_cap("[mandate]\nmax_ttl_hours = 168\n", "").origin(key);
    assert!(ceiling.contains("not configurable"), "{ceiling}");
    let project = mandate_cap("", "[mandate]\nmax_ttl_hours = 4\n").origin(key);
    assert!(
        project.contains("this project's .aida/config.toml"),
        "{project}"
    );
    assert!(project.contains("only lower"), "{project}");
}

#[test]
fn the_inline_grant_is_8h_unless_the_global_config_says_otherwise() {
    assert_eq!(inline_hours("", ""), INLINE_PARENT_MAX.num_hours());
    assert_eq!(inline_hours("", ""), 8);
    // Raised by the global key only.
    assert_eq!(
        inline_hours("[seat]\ninline_grant_max_hours = 12\n", ""),
        12
    );
    assert_eq!(inline_hours("", "[seat]\ninline_grant_max_hours = 12\n"), 8);
    // Lowered by either layer.
    assert_eq!(inline_hours("[seat]\ninline_grant_max_hours = 4\n", ""), 4);
    assert_eq!(
        inline_hours(
            "[seat]\ninline_grant_max_hours = 12\n",
            "[seat]\ninline_grant_max_hours = 2\n"
        ),
        2
    );
}

#[test]
fn the_inline_grant_never_outlives_the_seat_maximum_or_the_ceiling() {
    // The seat maximum defaults to 24h.
    assert_eq!(
        inline_hours("[seat]\ninline_grant_max_hours = 100\n", ""),
        24
    );
    assert_eq!(
        inline_hours(
            "[seat]\ninline_grant_max_hours = 100\nmax_grant_hours = 72\n",
            ""
        ),
        72
    );
    // A project that lowers the seat maximum lowers the inline grant with it.
    assert_eq!(inline_hours("", "[seat]\nmax_grant_hours = 3\n"), 3);
    assert_eq!(
        inline_hours(
            "[seat]\ninline_grant_max_hours = 500\nmax_grant_hours = 500\n",
            ""
        ),
        HARD_CEILING_GRANT_HOURS as i64
    );
}

/// The production entry point reads the user's home through the hermetic
/// lookup. The value is the default, so a mandate test running beside this
/// one sees the cap it expects.
#[test]
fn the_global_layer_is_the_users_aida_home() {
    let home = config_dir("[mandate]\nmax_ttl_hours = 24\n");
    let _env =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(home.path().to_str().unwrap()))]);
    let cap = resolve_hour_cap(None, MANDATE_MAX_TTL_KEY, 24);
    assert_eq!((cap.hours, cap.source), (24, CapSource::Global));
    let project = config_dir("");
    assert_eq!(inline_parent_max(project.path()), INLINE_PARENT_MAX);
}
