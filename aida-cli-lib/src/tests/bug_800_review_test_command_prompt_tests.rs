use super::*;

#[test]
fn review_prompt_names_focused_tests_as_commands() {
    let section = review_prompt_test_commands_section();

    assert!(section.contains("cargo test -p <crate> <test_name>"));
    assert!(section.contains("not a bare test identifier"));
    assert!(!section.contains("cargo test <test_name>"));
}
