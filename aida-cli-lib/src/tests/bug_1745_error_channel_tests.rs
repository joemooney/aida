// trace:BUG-1745 | ai:codex
#[test]
fn typed_payload_error_is_recognised() {
    let err = anyhow::Error::new(crate::TypedPayloadEmitted);
    assert!(err.downcast_ref::<crate::TypedPayloadEmitted>().is_some());
}

// trace:BUG-1745 | ai:codex
#[test]
fn doctor_envelope_carries_error_and_performance_audits() {
    let encoded = crate::doctor_cmd::bug_1745_test_envelope();
    let decoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert!(decoded["error"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(decoded["performance_audits"].as_array().unwrap().len(), 1);
}

// Locate the built `aida` binary through the process-wide hardened resolver
// rather than a raw OS executable lookup: TASK-1262's architecture guard keeps every
// such lookup inside the resolver, and its allowlist is exact — the guard greps for
// the literal call, so naming it here would trip it too. Under
// `cargo test` the resolver returns target/<profile>/deps/<runner>, so the
// binary sits two levels up.
// trace:BUG-1745 | ai:codex
fn built_aida_binary() -> std::path::PathBuf {
    let runner = crate::resolve_aida_exe();
    let binary = runner
        .parent()
        .and_then(|parent| parent.parent())
        .map(|target| target.join("aida"))
        .expect("test runner path has a target/<profile> ancestor");
    assert!(
        binary.is_file(),
        "expected the built aida binary at {} \u{2014} run `cargo build -p aida-cli` first, \
         and unset AIDA_BIN, which redirects the resolver away from this build",
        binary.display()
    );
    binary
}

// trace:BUG-1745 | ai:codex
#[test]
fn performance_command_stdout_round_trips_into_failure_trip() {
    let argv = crate::maintenance_schedule::bug_1745_performance_argv();
    let output = std::process::Command::new(built_aida_binary())
        .args(argv)
        .output()
        .expect("run built aida binary with captured stdout");
    assert_ne!(
        output.status.code(),
        Some(0),
        "fixture must exercise failure output"
    );
    let trip = crate::maintenance_schedule::bug_1745_failure_trip(
        String::from_utf8(output.stdout).unwrap(),
        output.status.code().unwrap_or(1),
    )
    .unwrap();
    assert_eq!(
        trip.audit_error, None,
        "trip should parse the emitted audits"
    );
    assert!(
        !trip.performance.is_empty(),
        "trip should retain performance audits"
    );
}

// trace:BUG-1745 | ai:codex
#[test]
fn failure_trip_malformed_stdout_sets_audit_error() {
    let trip = crate::maintenance_schedule::bug_1745_failure_trip("{broken".into(), 1).unwrap();
    assert!(trip.audit_error.is_some());
}

// trace:BUG-1745 | ai:codex
#[test]
fn failure_trip_missing_audits_sets_audit_error() {
    let trip = crate::maintenance_schedule::bug_1745_failure_trip("{}".into(), 1).unwrap();
    assert!(trip.audit_error.is_some());
}
