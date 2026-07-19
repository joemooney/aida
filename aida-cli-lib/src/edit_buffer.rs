//! Editor-buffer flow for `aida edit <spec>` with no field flags — open the
//! user's editor (git-commit style) on a markdown buffer of the editable
//! fields and parse the saved result back.
//!
//! Buffer shape mirrors `git commit --cleanup=scissors`: the first line is the
//! title, everything after the first blank line is the description (markdown —
//! an `## Acceptance` section survives as-is), and everything at or below the
//! scissors line is guidance that is stripped on parse. Scissors-based
//! stripping (rather than a `#`-comment prefix) is deliberate: spec bodies are
//! markdown, so `#`-prefixed lines are headings, not comments.
//!
//! Pure render/parse helpers live here with unit tests; the integration point
//! (TTY gate, targeted write-back) is the `Command::Edit` arm in
//! `git_backend_cmd.rs`.

use anyhow::{Context, Result};
use std::path::Path;

/// The scissors marker separating editable content (above) from guidance
/// (below). Matched as an exact line, so markdown `#` headings in the
/// description are never mistaken for comments.
// trace:TASK-1117 | ai:claude
pub const SCISSORS: &str = "# ------------------------ >8 ------------------------";

/// Render the editable buffer for a spec: title, blank line, description,
/// then the scissors line and commented guidance.
// trace:TASK-1117 | ai:claude
pub fn render_buffer(spec_id: &str, title: &str, description: &str) -> String {
    format!(
        "{title}\n\n{description}\n\n{SCISSORS}\n\
         # Editing {spec_id}. Everything at or below the scissors line above\n\
         # is ignored when the buffer is parsed.\n\
         #\n\
         # First line: the title. Everything after the first blank line: the\n\
         # description (markdown — an '## Acceptance' section is kept as-is).\n\
         # Empty the buffer above the scissors line, or leave it unchanged,\n\
         # to finish without changing anything.\n"
    )
}

/// Parse a saved buffer back into `(title, description)`.
///
/// Returns `None` when the content above the scissors line is empty or
/// whitespace-only (the "abort without changes" signal). Leading/trailing
/// blank lines are trimmed; interior blank lines in the description are
/// preserved.
// trace:TASK-1117 | ai:claude
pub fn parse_buffer(content: &str) -> Option<(String, String)> {
    let mut kept: Vec<&str> = Vec::new();
    for line in content.lines() {
        if line.trim_end() == SCISSORS {
            break;
        }
        kept.push(line);
    }
    while kept.first().is_some_and(|l| l.trim().is_empty()) {
        kept.remove(0);
    }
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let (first, rest) = kept.split_first()?;
    let title = first.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let mut body = rest;
    while body.first().is_some_and(|l| l.trim().is_empty()) {
        body = &body[1..];
    }
    let description = body.join("\n").trim_end().to_string();
    Some((title, description))
}

/// Resolve which editor to launch: `AIDA_EDITOR` → `VISUAL` → `EDITOR` →
/// a sane platform default (`vi` on unix, `notepad` on Windows).
// trace:TASK-1117 | ai:claude
pub fn resolve_editor() -> String {
    for var in ["AIDA_EDITOR", "VISUAL", "EDITOR"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    if cfg!(windows) {
        "notepad".to_string()
    } else {
        "vi".to_string()
    }
}

/// Open the resolved editor on a rendered buffer for the spec and parse the
/// result. `Ok(Some((title, description)))` on a saved buffer with content;
/// `Ok(None)` when the buffer was emptied; `Err` when the editor exits
/// non-zero or cannot be launched (the spec is left untouched either way).
// trace:TASK-1117 | ai:claude
pub fn edit_spec_in_editor(
    spec_id: &str,
    title: &str,
    description: &str,
) -> Result<Option<(String, String)>> {
    let path =
        std::env::temp_dir().join(format!("aida-edit-{}-{}.md", spec_id, std::process::id()));
    std::fs::write(&path, render_buffer(spec_id, title, description))
        .with_context(|| format!("could not write edit buffer at {}", path.display()))?;
    let editor = resolve_editor();
    let outcome = match launch_editor(&editor, &path) {
        Ok(status) if status.success() => std::fs::read_to_string(&path)
            .with_context(|| format!("could not read edit buffer at {}", path.display()))
            .map(|content| parse_buffer(&content)),
        Ok(status) => Err(anyhow::anyhow!(
            "editor `{}` exited with {} — nothing was changed",
            editor,
            status
        )),
        Err(e) => Err(anyhow::anyhow!(
            "could not launch editor `{}` ({}) — set AIDA_EDITOR, VISUAL, or EDITOR",
            editor,
            e
        )),
    };
    let _ = std::fs::remove_file(&path);
    outcome
}

/// Launch the editor through the platform shell so multi-word editor values
/// (e.g. `code --wait`) work, matching git's behavior.
// trace:TASK-1117 | ai:claude
fn launch_editor(editor: &str, path: &Path) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg(format!("{} \"{}\"", editor, path.display()))
            .status()
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{} '{}'", editor, path.display()))
            .status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_title_and_description() {
        let desc = "Body line one.\n\n## Acceptance\n- criterion A\n- criterion B";
        let buffer = render_buffer("TASK-1", "My title", desc);
        let (title, description) = parse_buffer(&buffer).expect("content parses");
        assert_eq!(title, "My title");
        assert_eq!(description, desc);
    }

    #[test]
    fn markdown_headings_survive_scissors_stripping() {
        let buffer = format!("Title\n\n# Heading\n## Acceptance\n- x\n\n{SCISSORS}\n# guidance\n");
        let (_, description) = parse_buffer(&buffer).expect("content parses");
        assert_eq!(description, "# Heading\n## Acceptance\n- x");
    }

    #[test]
    fn empty_buffer_above_scissors_is_none() {
        let buffer = format!("\n  \n\n{SCISSORS}\n# guidance\n");
        assert!(parse_buffer(&buffer).is_none());
        assert!(parse_buffer("").is_none());
        assert!(parse_buffer("   \n\n").is_none());
    }

    #[test]
    fn title_only_buffer_yields_empty_description() {
        let buffer = format!("Just a title\n\n{SCISSORS}\n# guidance\n");
        let (title, description) = parse_buffer(&buffer).expect("content parses");
        assert_eq!(title, "Just a title");
        assert_eq!(description, "");
    }

    #[test]
    fn missing_scissors_line_still_parses() {
        let (title, description) = parse_buffer("Title\n\nBody").expect("content parses");
        assert_eq!(title, "Title");
        assert_eq!(description, "Body");
    }

    #[test]
    fn leading_and_trailing_blanks_are_trimmed() {
        let buffer = format!("\nTitle\n\n\nBody\n\n\n{SCISSORS}\n");
        let (title, description) = parse_buffer(&buffer).expect("content parses");
        assert_eq!(title, "Title");
        assert_eq!(description, "Body");
    }
}
