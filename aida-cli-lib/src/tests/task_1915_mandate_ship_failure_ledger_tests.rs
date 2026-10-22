//! TASK-1915 (F3): a mandate-released ship that fails before or at its merge
//! writes a `refused` MandateDriveAction row under the mandate.
//
// trace:TASK-1915 | ai:claude

use super::*;
use crate::mandate_drive::{ledger_rows, Outcome};

#[test]
fn a_failed_mandate_ship_is_ledgered_as_refused() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    let cover = MandateCover {
        id: "m-1".into(),
        principal: "joe".into(),
    };
    ledger_mandate_ship_failure(
        root.path(),
        Some(&cover),
        42,
        "`gh pr merge` failed: base branch was modified",
    );
    let rows = ledger_rows(root.path(), "m-1");
    assert_eq!(rows.len(), 1, "{rows:?}");
    let (_, actor, row) = &rows[0];
    assert_eq!(actor, "session");
    assert_eq!(row.lane, "pr:42");
    assert_eq!(row.action, "ship");
    assert_eq!(row.outcome, Outcome::Refused);
    assert!(row
        .why
        .as_deref()
        .is_some_and(|w| w.contains("base branch was modified")));
}

#[test]
fn a_ship_no_mandate_released_writes_no_row() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    ledger_mandate_ship_failure(root.path(), None, 42, "`gh pr merge` failed");
    assert!(crate::events::read_all_with_archive(root.path()).is_empty());
}

// Wiring pin (behavioural seam impractical: these exits sit inside
// `pr_ship_with_method` behind live forge, CI and merge-lease calls). Every
// early exit between the mandate release decision and the merge must ledger
// its refusal: each exit (`return`, `?`, `bail!(`, `ensure!(`, found in the
// code with string literals and comments blanked) needs its own
// `ledger_mandate_ship_failure(` call, or a `ledger-exempt:` comment saying
// why there is no row, since the previous exit. Deleting a call site, or
// adding an exit of any kind without one, turns this red.
// trace:TASK-1915 | ai:claude (TASK-2124)
#[test]
fn every_exit_between_the_mandate_release_and_the_merge_ledgers_a_refusal() {
    let src = include_str!("../pr_cmd.rs");
    let body = &src[src
        .find(concat!("pub(crate) fn pr_ship_", "with_method("))
        .expect("pr_ship_with_method exists")..];
    let start = body
        .find(concat!("let mandate_release = (!floor_", "authority)"))
        .expect("the mandate release decision");
    let end = body
        .find(concat!(
            "STORY-1700: a ship under a mandate is a ",
            "mandate-driven action"
        ))
        .expect("the post-merge success ledger");
    let (exits, unledgered) = early_exits(&body[start..end]);
    assert!(
        unledgered.is_empty(),
        "exits after the release decision that write no MandateDriveAction row: {unledgered:#?}"
    );
    // The release floor, the refusal hold, the refusal pin, the rebase pin,
    // both hold-gate timeouts, the mandate ending, the proven base, and the
    // merge failure with and without a hold to re-arm.
    assert_eq!(exits.len(), 10, "{exits:#?}");
}

/// The pin's scanner catches every kind of early exit, and ignores a `?` or
/// `return` inside a string literal or a comment.
#[test]
fn the_exit_scanner_sees_every_kind_of_early_exit() {
    let window = r#"
        let x = load()?;
        eprintln!("unknown {}", s.unwrap_or("?")); // a comment? return
        ledger_mandate_ship_failure(root, cover, 1, "r");
        return Err(e);
        anyhow::ensure!(ok, "no");
        // ledger-exempt: nothing to ledger here
        anyhow::bail!("no mandate");
        let c = '?';
        if done { return Ok(()); }
    "#;
    let (exits, unledgered) = early_exits(window);
    assert_eq!(exits.len(), 5, "{exits:#?}");
    assert_eq!(
        unledgered,
        vec![
            "let x = load()?;".to_string(),
            "anyhow::ensure!(ok, \"no\");".to_string(),
            "if done { return Ok(()); }".to_string(),
        ]
    );
}

/// Returns (every exit line, the exit lines with no ledger call or
/// `ledger-exempt:` comment since the previous exit).
fn early_exits(window: &str) -> (Vec<String>, Vec<String>) {
    let code = blank_strings_and_comments(window);
    let original: Vec<&str> = window.lines().collect();
    let mut segment = 0;
    let (mut exits, mut unledgered) = (Vec::new(), Vec::new());
    for (n, line) in code.lines().enumerate() {
        let is_exit = line.contains('?')
            || line.contains("bail!(")
            || line.contains("ensure!(")
            || line
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|w| w == "return");
        if !is_exit {
            continue;
        }
        let covered = original[segment..=n].iter().any(|l| {
            l.contains(concat!("ledger_mandate_ship_", "failure("))
                || l.contains(concat!("ledger-", "exempt:"))
        });
        let shown = original[n].trim().to_string();
        if !covered {
            unledgered.push(shown.clone());
        }
        exits.push(shown);
        segment = n + 1;
    }
    (exits, unledgered)
}

/// Replaces the contents of string and char literals and comments with
/// spaces, keeping line breaks so line numbers still match the source.
fn blank_strings_and_comments(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == 'r' && matches!(chars.get(i + 1), Some('"') | Some('#')) {
            let hashes = chars[i + 1..].iter().take_while(|&&h| h == '#').count();
            if chars.get(i + 1 + hashes) != Some(&'"') {
                out.push(c);
                i += 1;
                continue;
            }
            let close: String = std::iter::once('"')
                .chain(std::iter::repeat('#').take(hashes))
                .collect();
            let from = i + 2 + hashes;
            let rest: String = chars[from..].iter().collect();
            let len = rest.find(&close).map_or(rest.len(), |e| e + close.len());
            let taken = rest[..len].chars().count();
            out.extend(chars[i..from + taken].iter().map(|&c| blank(c)));
            i = from + taken;
        } else if c == '"' {
            out.push(' ');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                let step = if chars[i] == '\\' { 2 } else { 1 };
                for k in 0..step {
                    if let Some(&ch) = chars.get(i + k) {
                        out.push(blank(ch));
                    }
                }
                i += step;
            }
            out.push(' ');
            i += 1;
        } else if c == '\'' && chars.get(i + 2) == Some(&'\'') {
            out.push_str("   ");
            i += 3;
        } else if c == '\'' && chars.get(i + 1) == Some(&'\\') && chars.get(i + 3) == Some(&'\'') {
            out.push_str("    ");
            i += 4;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}
