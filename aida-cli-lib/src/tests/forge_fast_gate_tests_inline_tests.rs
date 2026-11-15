#[test]
fn fast_gate_body_preserves_text_after_previous_annotation() {
    let body = super::fast_gate_body("original", "old", "fail") + "later reviewer notes";
    let updated = super::fast_gate_body(&body, "fixed", "pass");
    assert!(updated.starts_with("original"));
    assert!(updated.ends_with("later reviewer notes"));
    assert!(updated.contains("fast_gate=pass head=fixed"));
    assert!(!updated.contains("head=old"));
}
