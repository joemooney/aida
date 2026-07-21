use super::*;

/// The scaffolded `[autopilot]` block is fully commented-out: written to a
/// config.toml as-is it changes nothing (loads pure defaults).
// trace:TASK-1021 | ai:claude
#[test]
fn scaffolded_autopilot_block_is_fully_commented() {
    let section = init_autopilot_config_section();
    assert!(section.contains("# [autopilot]"));
    // Every non-empty line is a comment at column 0 — nothing takes effect as
    // written, and the block sits flush with the other scaffolded sections
    // (the `\` line continuations must eat the source indentation).
    for line in section.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            line.starts_with('#'),
            "uncommented or indented line in scaffolded [autopilot] block: {line:?}"
        );
    }
    assert!(
        autopilot::parse_authority_overrides(section).is_empty(),
        "the commented block must parse to zero overrides"
    );
}

/// Uncommenting the example lines yields exactly the built-in default
/// authority table — the documented values match what the parser accepts.
// trace:TASK-1021 | ai:claude
#[test]
fn uncommented_autopilot_example_matches_defaults() {
    let section = init_autopilot_config_section();
    // The example lines are the trailing group beginning at `# [autopilot]`.
    let start = section
        .lines()
        .position(|l| l.trim_start() == "# [autopilot]")
        .expect("scaffolded block carries a `# [autopilot]` example header");
    let uncommented: String = section
        .lines()
        .skip(start)
        .map(|l| l.trim_start().trim_start_matches("# ").to_string() + "\n")
        .collect();
    assert!(uncommented.starts_with("[autopilot]"));

    let overrides = autopilot::parse_authority_overrides(&uncommented);
    let defaults = autopilot::AutopilotEnvelope::default();
    // Every documented key parses (nothing silently skipped) …
    assert_eq!(
        overrides.len(),
        defaults.authorities.len(),
        "every action class in the default table must appear in the example block"
    );
    // … and every documented value equals the built-in default.
    assert_eq!(overrides, defaults.authorities);
}

/// The scaffolded block documents ONLY keys the parser actually honors today —
/// no aspirational knobs. Guards against the doc drifting ahead of the code.
// trace:TASK-1021 | ai:claude
#[test]
fn scaffolded_autopilot_block_documents_no_unparsed_keys() {
    let section = init_autopilot_config_section();
    let start = section
        .lines()
        .position(|l| l.trim_start() == "# [autopilot]")
        .expect("scaffolded block carries a `# [autopilot]` example header");
    for line in section.lines().skip(start + 1) {
        let body = line.trim_start().trim_start_matches("# ").trim();
        if body.is_empty() {
            continue;
        }
        let (key, _) = body
            .split_once('=')
            .unwrap_or_else(|| panic!("example line is not a key = value pair: {line:?}"));
        assert!(
            autopilot::ActionClass::parse(key.trim()).is_some(),
            "scaffolded [autopilot] documents a key the parser does not accept: {key:?}"
        );
    }
}
