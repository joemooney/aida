use super::*;

// trace:STORY-556 | ai:claude
#[test]
fn getting_started_leads_with_the_why_magic() {
    let names: Vec<&str> = GETTING_STARTED.iter().map(|(n, _)| *n).collect();
    // STORY-758 (Strategy B): the first screen leads with the magic —
    // `why` first, then the spec core. The autonomy verbs (zen/ship/solo)
    // moved to their grouped depth (still in `aida help --all`); the first
    // impression is the 60-second magic, not the machine.
    assert_eq!(names[0], "why", "the magic leads the first screen");
    assert_eq!(
        names,
        ["why", "init", "show", "add", "list", "search", "graph"]
    );
}

// trace:STORY-556 | ai:claude
#[test]
fn command_groups_cover_the_acceptance_headings() {
    let headings: Vec<&str> = command_groups().iter().map(|(g, _)| *g).collect();
    for required in [
        "Getting started",
        "Specs",
        "Work & autonomy",
        "Git & lifecycle",
        "Planning",
        "Roles & sessions",
        "Project setup & maintenance",
        "Reporting",
    ] {
        assert!(
            headings.contains(&required),
            "missing required help heading: {required}"
        );
    }
}

// trace:BUG-520 | ai:claude — real top-level commands that were previously
// absent from every help-all group must now each appear exactly once.
#[test]
fn previously_ungrouped_commands_appear_exactly_once() {
    let mut counts = std::collections::HashMap::new();
    for (_group, cmds) in command_groups() {
        for (name, _desc) in *cmds {
            *counts.entry(*name).or_insert(0) += 1;
        }
    }
    for cmd in ["rules", "doctor", "defer", "undefer", "remote", "release"] {
        assert_eq!(
            counts.get(cmd).copied().unwrap_or(0),
            1,
            "command `{cmd}` must appear in exactly one help-all group"
        );
    }
}

// trace:STORY-556 | ai:claude — dev/internal stays out of the novice view.
#[test]
fn dev_commands_are_not_in_getting_started() {
    let names: Vec<&str> = GETTING_STARTED.iter().map(|(n, _)| *n).collect();
    assert!(!names.contains(&"dev"));
    assert!(!names.contains(&"help-all"));
    assert!(!names.contains(&"orchestrator"));
}

// trace:STORY-556 | ai:claude — no SPEC-IDs leak into any help row text.
#[test]
fn no_spec_ids_in_help_text() {
    let leak = |s: &str| {
        ["STORY-", "TASK-", "BUG-", "SPIKE-", "EPIC-", "FR-"]
            .iter()
            .any(|p| s.contains(p))
    };
    for (group, cmds) in command_groups() {
        assert!(!leak(group), "spec-id in group heading: {group}");
        for (name, desc) in *cmds {
            assert!(!leak(name) && !leak(desc), "spec-id in help row: {name}");
        }
    }
    for (name, desc) in GETTING_STARTED {
        assert!(!leak(name) && !leak(desc));
    }
}

// trace:TASK-861 | ai:claude — `aida help <topic>` resolves a group by exact
// (case-insensitive) name.
#[test]
fn help_topic_resolves_exact_name_case_insensitive() {
    let (g, cmds) = resolve_help_topic("Planning").expect("exact match");
    assert_eq!(g, "Planning");
    assert!(cmds.iter().any(|(n, _)| *n == "ultraplan"));

    let (g2, _) = resolve_help_topic("planning").expect("lowercased match");
    assert_eq!(g2, "Planning");
}

// trace:TASK-861 | ai:claude — a unique case-insensitive prefix resolves.
#[test]
fn help_topic_resolves_unique_prefix() {
    let (g, _) = resolve_help_topic("git").expect("unique prefix");
    assert_eq!(g, "Git & lifecycle");
    let (g2, _) = resolve_help_topic("report").expect("unique prefix");
    assert_eq!(g2, "Reporting");
}

// trace:TASK-861 | ai:claude — unknown topics error with the full topic list.
#[test]
fn help_topic_unknown_errors_with_topic_list() {
    let err = resolve_help_topic("bogus").expect_err("unknown topic");
    assert!(err.contains(&"Getting started"));
    assert!(err.contains(&"Planning"));
    // The error list is exactly the group names.
    let names: Vec<&str> = command_groups().iter().map(|(g, _)| *g).collect();
    assert_eq!(err, names);
}
