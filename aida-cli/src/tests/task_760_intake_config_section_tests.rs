use super::*;

/// The scaffolded `[intake]` block is fully commented-out: written to a
/// config.toml as-is it changes nothing (loads pure defaults).
// trace:TASK-760 | ai:claude
#[test]
fn scaffolded_intake_block_is_fully_commented() {
    let section = init_intake_config_section();
    assert!(section.contains("# [intake]"));
    // Every non-empty line is a comment — nothing takes effect as-is.
    for line in section.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            line.trim_start().starts_with('#'),
            "uncommented line in scaffolded [intake] block: {line:?}"
        );
    }
    assert_eq!(
        intake::IntakeConfig::from_toml_str(section),
        intake::IntakeConfig::default()
    );
}

/// Uncommenting the example lines yields exactly the built-in defaults —
/// the documented values match what the parser actually accepts.
// trace:TASK-760 | ai:claude
#[test]
fn uncommented_intake_example_matches_defaults() {
    let section = init_intake_config_section();
    // The example lines are the trailing group beginning at `# [intake]`.
    let start = section
        .lines()
        .position(|l| l.trim_start() == "# [intake]")
        .expect("scaffolded block carries a `# [intake]` example header");
    let uncommented: String = section
        .lines()
        .skip(start)
        .map(|l| l.trim_start().trim_start_matches("# ").to_string() + "\n")
        .collect();
    assert!(uncommented.starts_with("[intake]"));
    assert_eq!(
        intake::IntakeConfig::from_toml_str(&uncommented),
        intake::IntakeConfig::default()
    );
}
