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

// Derive the CLI from this exact test image, accepting both Cargo layouts.
// Retain the checkout/target ownership checks below for binary candidates.
// trace:BUG-1745 trace:BUG-1777 trace:BUG-2539 | ai:codex
fn built_aida_binary() -> std::path::PathBuf {
    let runner = crate::test_exec::current_test_runner();
    let profile_dir = crate::test_exec::test_profile_dir(&runner).unwrap();
    let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate directory sits inside the workspace root");
    let binary_name = format!("aida{}", std::env::consts::EXE_SUFFIX);
    // The cross-platform workflow prebuilds release artifacts before running
    // debug test binaries; local `cargo test` builds only the current profile.
    let mut candidates = vec![
        checkout.join("target/release").join(&binary_name),
        profile_dir.join(&binary_name),
    ];
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = std::path::PathBuf::from(target_dir);
        candidates.insert(0, target_dir.join("release").join(&binary_name));
        candidates.push(
            target_dir
                .join(profile_dir.file_name().unwrap())
                .join(&binary_name),
        );
    }
    let binary = candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| profile_dir.join(&binary_name));
    let is_in_checkout = binary.starts_with(checkout);
    let is_in_target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(|dir| binary.starts_with(std::path::PathBuf::from(dir)))
        .unwrap_or(false);
    assert!(
        is_in_checkout || is_in_target_dir,
        "resolved {} outside this checkout at {} \u{2014} the resolver returned a \
         foreign build (AIDA_BIN or AIDA_BUILD_PROFILE); refusing to measure it",
        binary.display(),
        checkout.display()
    );
    assert!(
        binary.is_file(),
        "expected the built aida binary at {} \u{2014} run `cargo build -p aida-cli` first",
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
