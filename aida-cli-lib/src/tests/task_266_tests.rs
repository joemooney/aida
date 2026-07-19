use super::{auto_complete_queue_add_args, parse_added_spec_id};

#[test]
fn parses_spec_id_from_aida_add_output() {
    let stdout = "Requirement added successfully!\n\
                      UUID: 019e31cc-26c3-70f3-adb6-3b20cb6d32a9\n\
                      ID: BUG-220\n";
    assert_eq!(parse_added_spec_id(stdout), Some("BUG-220".to_string()));
}

#[test]
fn returns_none_when_no_id_line() {
    let stdout = "Requirement added successfully!\nUUID: abc\n";
    assert_eq!(parse_added_spec_id(stdout), None);
}

#[test]
fn returns_none_for_empty_id_value() {
    assert_eq!(parse_added_spec_id("ID:   \n"), None);
}

#[test]
fn auto_complete_preflight_queue_add_disables_cwd_scope_derivation() {
    // BUG-352: `aida queue work <SPEC> --auto-complete` auto-queues
    // an explicit spec as standalone work. It must not inherit a
    // stale/misattributed cwd lease via queue-add's normal scope
    // derivation path.
    assert_eq!(
        auto_complete_queue_add_args("TASK-488"),
        vec![
            "queue",
            "add",
            "TASK-488",
            "--for",
            "implementer",
            "--no-scope"
        ]
    );
}
