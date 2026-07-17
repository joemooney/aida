use super::*;

#[test]
fn trivial_doc_task_with_acceptance_is_not_under_specified() {
    // BUG-708 regression: a well-formed trivial doc task — real body plus a
    // `## Acceptance` section, but no EARS "THE SYSTEM SHALL" response
    // clause — must NOT be gated as under-specified. It trips only the
    // OPTIONAL missing-behavior clarity nit, which is advisory, not a drive
    // gate. This is the exact shape of TASK-1115 that got wrongly blocked.
    let req = Requirement::new(
        "Flow smoke-test: add a dated marker line".to_string(),
        "Append a line to docs/flow-smoke.md recording the date this smoke-test ran.\n\n\
             ## Acceptance\n- docs/flow-smoke.md exists and contains a dated marker line."
            .to_string(),
    );
    assert!(
        !spec_is_under_specified(&req),
        "a trivial doc task with a real body + acceptance must not be under-specified"
    );
}

#[test]
fn essentially_empty_spec_is_under_specified() {
    // The genuine under-spec signal (EmptyBody) still gates the drive.
    let req = Requirement::new("Stub".to_string(), "fix".to_string());
    assert!(
        spec_is_under_specified(&req),
        "an essentially-empty spec is genuinely under-specified"
    );
}
