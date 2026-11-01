//! BUG-2379: `aida session start`'s `Next:` block for a private clone vs a
//! linked worktree, under the claude and codex default vendors.
// trace:BUG-2379 | ai:claude

use super::{session_next_lines, vendor_launch_line, NextLine};
use std::path::Path;

fn render(lines: &[NextLine]) -> Vec<String> {
    lines
        .iter()
        .map(|l| match l.comment {
            Some(c) => format!("{}    {}", l.command, c),
            None => l.command.clone(),
        })
        .collect()
}

const TREE: &str = "/work/aida-seat-aida-task-2631";

#[test]
fn linked_worktree_claude_output_is_unchanged() {
    // trace:BUG-b6b1278890774ef993d2ebc327862470-153.acf478b4 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-153.ac8a4f04 | ai:claude
    let lines = render(&session_next_lines(
        "TASK-2631",
        Path::new(TREE),
        false,
        "claude",
        true,
    ));
    assert_eq!(
        lines,
        vec![
            "claude    # then /aida-implement TASK-2631".to_string(),
            format!("cd {TREE}"),
            "aida worktree enter TASK-2631    # cd in + warm build cache (filtered session env)"
                .to_string(),
        ]
    );
    let no_cache = render(&session_next_lines(
        "TASK-2631",
        Path::new(TREE),
        false,
        "claude",
        false,
    ));
    assert_eq!(
        no_cache,
        vec![
            "claude    # then /aida-implement TASK-2631".to_string(),
            format!("cd {TREE}"),
        ]
    );
}

#[test]
fn linked_worktree_codex_uses_codex_launcher() {
    // trace:BUG-b6b1278890774ef993d2ebc327862470-153.ac4b70f0 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-153.ac8a4f04 | ai:claude
    let lines = render(&session_next_lines(
        "TASK-2631",
        Path::new(TREE),
        false,
        "codex",
        true,
    ));
    assert_eq!(lines[0], "codex    # then $aida-implement TASK-2631");
    assert!(lines.iter().all(|l| !l.starts_with("claude")));
    assert!(lines.iter().any(|l| l.starts_with("aida worktree enter")));
}

#[test]
fn clone_names_the_clone_and_never_suggests_worktree_enter() {
    // trace:BUG-b6b1278890774ef993d2ebc327862470-153.ac213f2e | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-153.ac4b70f0 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-153.ac8a4f04 | ai:claude
    for (vendor, launch) in [
        ("claude", "claude    # then /aida-implement TASK-2631"),
        ("codex", "codex    # then $aida-implement TASK-2631"),
    ] {
        for cache in [true, false] {
            let lines = render(&session_next_lines(
                "TASK-2631",
                Path::new(TREE),
                true,
                vendor,
                cache,
            ));
            assert!(
                lines[0].starts_with(&format!("cd {TREE}")),
                "clone Next: must cd into the clone first: {lines:?}"
            );
            assert_eq!(lines[1], launch);
            assert!(
                lines.iter().all(|l| !l.contains("worktree enter")),
                "clone Next: must never suggest `aida worktree enter`: {lines:?}"
            );
        }
    }
}

#[test]
fn vendor_launch_line_defaults_blank_to_claude() {
    assert_eq!(
        vendor_launch_line("  ", "BUG-1"),
        "claude    # then /aida-implement BUG-1"
    );
    assert_eq!(
        vendor_launch_line("Codex", "BUG-1"),
        "Codex    # then $aida-implement BUG-1"
    );
}
