//! BUG-2434: a headless implementer lane defaults `[contained] os_wrap` on
//! for its own job runner when nobody chose a value and bubblewrap works on
//! the host, so its tests can run. The default lives on that runner only:
//! it never reaches this process's environment, so no later launch inherits
//! it. An explicit choice wins, and a lane that ran without os_wrap says
//! `os_wrap=off` where the advisor reads it.
// trace:BUG-2434 | ai:claude

use super::{
    lane_ran_without_os_wrap, offer_with, phase_test_warning, record_untested, with_os_wrap_off,
    Sandbox,
};
use crate::session::{implementer_lane_os_wrap, lane_os_wrap, os_wrap_explicit, LaneOsWrap};
use crate::test_env::EnvVarGuard;

#[test]
fn unset_os_wrap_defaults_on_only_when_bwrap_is_live() {
    assert_eq!(lane_os_wrap(None, true), LaneOsWrap::DefaultOn);
    assert_eq!(lane_os_wrap(None, false), LaneOsWrap::Unavailable);
    assert!(LaneOsWrap::DefaultOn.on());
    assert!(LaneOsWrap::Explicit(true).on());
    assert!(!LaneOsWrap::Explicit(false).on());
    assert!(!LaneOsWrap::Unavailable.on());
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
fn an_explicit_off_is_left_alone_and_the_probe_is_not_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set("AIDA_OS_WRAP", "0");
    assert_eq!(
        implementer_lane_os_wrap(tmp.path(), || panic!("an explicit value skips the probe")),
        LaneOsWrap::Explicit(false)
    );
    assert_eq!(std::env::var("AIDA_OS_WRAP").as_deref(), Ok("0"));
}

// Review finding 4: offer end to end, with the bubblewrap probe injected so
// the DefaultOn path runs on any host. Finding 2: nothing leaks to a later
// launch in the same process.
#[test]
fn offer_with_bwrap_live_and_os_wrap_unset_gives_the_lane_a_runner_and_leaks_nothing() {
    let main = tempfile::tempdir().unwrap();
    let seat = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::unset("AIDA_OS_WRAP");
    // A stale marker from an earlier round without os_wrap is cleared.
    super::record_os_wrap_off(main.path(), "lease-on", true);

    let jobs = offer_with(main.path(), seat.path(), "lease-on", || true)
        .expect("a runner is offered when bwrap works and os_wrap is unset");
    assert_eq!(jobs.sandbox, Sandbox::BwrapDefaultOn);
    assert!(!lane_ran_without_os_wrap(main.path(), "lease-on"));

    // The default stayed on the runner: the process environment is untouched,
    // so the seat's own launch and every later launch resolve os_wrap as
    // before (off here), and a later lane still reads the host as unset.
    assert!(std::env::var_os("AIDA_OS_WRAP").is_none());
    assert!(!crate::session::os_wrap_enabled(seat.path()));
    assert_eq!(os_wrap_explicit(seat.path()), None);
    let (program, args) =
        crate::session::os_wrapped_program_and_args(seat.path(), "claude", vec!["-p".into()])
            .unwrap();
    assert_eq!(
        (program.as_str(), args),
        ("claude", vec!["-p".to_string()]),
        "a launch after the lane is not wrapped"
    );
    assert_eq!(
        implementer_lane_os_wrap(seat.path(), || true),
        LaneOsWrap::DefaultOn,
        "a second lane in the same process still gets the default, not a leaked Explicit(true)"
    );
}

// The runner's wrap decision is the lane's, passed in: a DefaultOn lane is not
// refused for os_wrap, while a seat whose os_wrap is off still is.
#[test]
fn the_job_wrapper_takes_the_lanes_decision_not_the_env() {
    let seat = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::unset("AIDA_OS_WRAP");
    let off = crate::session::lane_job_wrapped_program_and_args(
        seat.path(),
        false,
        &[],
        "cargo",
        vec!["test".into()],
    )
    .unwrap_err()
    .to_string();
    assert!(off.contains("os_wrap` is off"), "{off}");
    // On: past the os_wrap gate (this tempdir then fails the git-root check).
    let on = crate::session::lane_job_wrapped_program_and_args(
        seat.path(),
        true,
        &[],
        "cargo",
        vec!["test".into()],
    );
    if let Err(e) = on {
        assert!(!e.to_string().contains("os_wrap` is off"), "{e}");
    }
}

// Spec ask (advisor 2026-11-04): the untested signal reaches the advisor, not
// only the lane log: the phase line and the result line carry `os_wrap=off`.
#[test]
fn a_lane_without_os_wrap_is_marked_where_the_advisor_reads_it() {
    let main = tempfile::tempdir().unwrap();
    let seat = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::unset("AIDA_OS_WRAP");

    assert!(offer_with(main.path(), seat.path(), "lease-off", || false).is_none());
    assert!(lane_ran_without_os_wrap(main.path(), "lease-off"));
    record_untested(main.path(), "lease-off", true);
    let line = phase_test_warning(main.path(), Some("lease-off")).unwrap();
    assert!(line.contains("untested=true"), "{line}");
    assert!(line.ends_with(" os_wrap=off"), "{line}");

    // A lane that had its runner but ran no test is untested, not os_wrap=off.
    assert!(offer_with(main.path(), seat.path(), "lease-on", || true).is_some());
    record_untested(main.path(), "lease-on", true);
    let line = phase_test_warning(main.path(), Some("lease-on")).unwrap();
    assert!(!line.contains("os_wrap=off"), "{line}");

    assert_eq!(
        with_os_wrap_off("implementer: pr-opened pr=7 untested=true".into(), true),
        "implementer: pr-opened pr=7 untested=true os_wrap=off"
    );
    assert_eq!(with_os_wrap_off("x".into(), false), "x");
}
