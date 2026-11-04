//! BUG-2434: a headless implementer lane defaults `[contained] os_wrap` on
//! when nobody chose a value and bubblewrap works on the host, so its lane
//! job runner is offered and its tests can run. An explicit choice wins, and
//! the reason a lane still gets no runner names the fix.
// trace:BUG-2434 | ai:claude

use crate::session::{
    default_os_wrap_for_implementer_lane, lane_os_wrap, os_wrap_explicit, BwrapAvailability,
    LaneOsWrap,
};
use crate::test_env::EnvVarGuard;

#[test]
fn unset_os_wrap_defaults_on_only_when_bwrap_is_live() {
    assert_eq!(lane_os_wrap(None, true), LaneOsWrap::DefaultOn);
    assert_eq!(lane_os_wrap(None, false), LaneOsWrap::Unavailable);
}

#[test]
fn an_explicit_os_wrap_wins_either_way() {
    for live in [true, false] {
        assert_eq!(lane_os_wrap(Some(false), live), LaneOsWrap::Explicit(false));
        assert_eq!(lane_os_wrap(Some(true), live), LaneOsWrap::Explicit(true));
    }
}

#[test]
fn the_no_runner_warning_names_the_fix() {
    let off = super::no_runner_reason(LaneOsWrap::Explicit(false));
    assert!(off.contains("untested=true"), "{off}");
    assert!(off.contains("set off"), "{off}");
    let missing = super::no_runner_reason(LaneOsWrap::Unavailable);
    assert!(missing.contains("untested=true"), "{missing}");
    assert!(missing.contains("aida doctor --fix-sandbox"), "{missing}");
}

#[test]
fn doctor_states_what_bwrap_means_for_lane_tests() {
    assert!(crate::doctor_cmd::doctor_lane_tests_note(true).contains("AIDA_OS_WRAP=0"));
    assert!(crate::doctor_cmd::doctor_lane_tests_note(false).contains("untested=true"));
}

#[test]
fn explicit_reads_env_then_config_and_is_none_when_unset() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    {
        let _env = EnvVarGuard::unset("AIDA_OS_WRAP");
        assert_eq!(os_wrap_explicit(root), None, "no env, no config");
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[contained]\nos_wrap = false\n",
        )
        .unwrap();
        assert_eq!(os_wrap_explicit(root), Some(false), "config says off");
    }
    let _env = EnvVarGuard::set("AIDA_OS_WRAP", "1");
    assert_eq!(os_wrap_explicit(root), Some(true), "env beats config");
}

#[test]
fn an_explicit_off_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set("AIDA_OS_WRAP", "0");
    assert_eq!(
        default_os_wrap_for_implementer_lane(tmp.path()),
        LaneOsWrap::Explicit(false)
    );
    assert_eq!(std::env::var("AIDA_OS_WRAP").as_deref(), Ok("0"));
}

#[test]
fn an_unset_os_wrap_is_turned_on_exactly_when_bwrap_works() {
    let tmp = tempfile::tempdir().unwrap();
    // The guard restores the unset value, whatever the launch sets.
    let _env = EnvVarGuard::unset("AIDA_OS_WRAP");
    let live = crate::session::bwrap_availability() == BwrapAvailability::Ok;
    let decision = default_os_wrap_for_implementer_lane(tmp.path());
    if live {
        assert_eq!(decision, LaneOsWrap::DefaultOn);
        assert_eq!(std::env::var("AIDA_OS_WRAP").as_deref(), Ok("1"));
        assert!(crate::session::os_wrap_enabled(tmp.path()));
    } else {
        assert_eq!(decision, LaneOsWrap::Unavailable);
        assert!(std::env::var_os("AIDA_OS_WRAP").is_none());
    }
}
