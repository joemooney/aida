//! `aida manual <command>` (a.k.a. `--doc`) — print the CLI-manual rationale
//! section for a command inline, next to `--help`.
//!
//! The smallest valuable slice of EPIC-40: `--help` says *what* a command does;
//! the manual chapters under `docs/cli/*.md` say *when / why / when-not*. This
//! command greps those chapters for the `### \`aida <cmd>\`` header that matches
//! and prints that one section (paged when a pager is available, else plain
//! stdout). `--help` stays the fact source; `manual` is the rationale.
//!
//! Extraction key: a level-3 header `### \`aida <cmd>\`` opens a command's
//! entry; the next header of the same-or-higher level (`###`, `##`, `#`) or a
//! horizontal-rule separator (`---`) closes it. A single header may name
//! several commands (`### \`aida away\` · \`aida home\` · \`aida presence\``);
//! we match if any backticked `aida <cmd>` token in the header equals the
//! requested command.
//!
//! trace:STORY-600 | ai:claude

use anyhow::Result;
use std::io::Write;
use std::path::Path;

/// Run `aida manual <command>`. Returns `Ok(())` when a section was found and
/// printed; errors (non-zero exit) when no manual entry matches `command` or
/// the manual chapters can't be located.
pub fn run(command: &str, project_root: &Path) -> Result<()> {
    // Normalise: accept either `aida foo` or just `foo`, and trim surrounding
    // whitespace. We match on the command token(s) after `aida `.
    let wanted = command.trim().trim_start_matches("aida ").trim();
    if wanted.is_empty() {
        anyhow::bail!("usage: aida manual <command>  (e.g. `aida manual graph`)");
    }

    let chapters_dir = project_root.join("docs").join("cli");
    if !chapters_dir.is_dir() {
        anyhow::bail!(
            "no CLI manual found at {} — the manual chapters (docs/cli/*.md) are not present in this project",
            chapters_dir.display()
        );
    }

    let mut md_files: Vec<std::path::PathBuf> = std::fs::read_dir(&chapters_dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("md")
                // The index/SUMMARY pages carry no per-command sections.
                && !matches!(
                    p.file_name().and_then(|s| s.to_str()),
                    Some("README.md") | Some("SUMMARY.md")
                )
        })
        .collect();
    md_files.sort();

    for path in &md_files {
        let body = match std::fs::read_to_string(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if let Some(section) = extract_section(&body, wanted) {
            let chapter = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("docs/cli");
            return present(&section, wanted, chapter);
        }
    }

    anyhow::bail!(
        "no manual entry for `aida {wanted}`. \
         The manual covers the rationale (when/why/when-not) for documented commands; \
         for the exact flags and defaults, run `aida {wanted} --help`."
    );
}

/// Pull the manual section for `wanted` out of one chapter's markdown body.
///
/// A section starts at the level-3 header whose backticked command tokens
/// include `aida <wanted>`, and runs up to (but not including) the next
/// boundary: another `###`/`##`/`#` header, or a `---` horizontal rule.
fn extract_section(body: &str, wanted: &str) -> Option<String> {
    let lines: Vec<&str> = body.lines().collect();
    let mut start: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        if let Some(rest) = line.strip_prefix("### ") {
            if header_matches(rest, wanted) {
                start = Some(i);
                break;
            }
        }
    }

    let start = start?;

    // Find the end: the first boundary after the header line.
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim();
        if line.starts_with("# ")
            || line.starts_with("## ")
            || line.starts_with("### ")
            || trimmed == "---"
        {
            end = i;
            break;
        }
    }

    let section = lines[start..end].join("\n");
    Some(section.trim_end().to_string())
}

/// Does a level-3 header (text after `### `) name `aida <wanted>` as one of its
/// commands? Headers may list several commands separated by `·`, e.g.
/// `` `aida away` · `aida home` · `aida presence` ``. We extract each
/// backtick-delimited token and compare the part after `aida `.
fn header_matches(header_text: &str, wanted: &str) -> bool {
    let mut rest = header_text;
    while let Some(open) = rest.find('`') {
        let after_open = &rest[open + 1..];
        if let Some(close) = after_open.find('`') {
            let token = after_open[..close].trim();
            if let Some(cmd) = token.strip_prefix("aida ") {
                if cmd.trim() == wanted {
                    return true;
                }
            }
            rest = &after_open[close + 1..];
        } else {
            break;
        }
    }
    false
}

/// Print the section, paged through `$PAGER`/`less` when stdout is a TTY and a
/// pager is available; otherwise plain stdout. Falls back to plain print if the
/// pager can't be spawned.
fn present(section: &str, wanted: &str, chapter: &str) -> Result<()> {
    let header = format!(
        "Manual — `aida {wanted}`  (docs/cli/{chapter})\n\
         The when/why/when-not. For exact flags + defaults: `aida {wanted} --help`.\n\n"
    );
    let full = format!("{header}{section}\n");

    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    if is_tty {
        if let Some(pager) = resolve_pager() {
            if page_through(&pager, &full).is_ok() {
                return Ok(());
            }
        }
    }

    print!("{full}");
    let _ = std::io::stdout().flush();
    Ok(())
}

/// The pager to use: `$PAGER` if set and non-empty, else `less` if present on
/// PATH, else `more`, else none. `$PAGER` may be a command line (e.g.
/// `less -R`); we split on whitespace.
fn resolve_pager() -> Option<Vec<String>> {
    if let Ok(p) = std::env::var("PAGER") {
        let p = p.trim();
        if !p.is_empty() {
            return Some(p.split_whitespace().map(|s| s.to_string()).collect());
        }
    }
    for candidate in ["less", "more"] {
        if which_on_path(candidate) {
            // `-R` so markdown's occasional ANSI/escape passes through cleanly;
            // `-F` so short sections that fit one screen don't trap the user.
            if candidate == "less" {
                return Some(vec!["less".into(), "-RF".into()]);
            }
            return Some(vec![candidate.into()]);
        }
    }
    None
}

/// Is `name` an executable on `$PATH`? Cheap PATH scan; avoids a dependency.
fn which_on_path(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path) {
        if dir.join(name).is_file() {
            return true;
        }
    }
    false
}

/// Spawn the pager and feed it `content` on stdin. Returns `Err` if the pager
/// can't be spawned (caller falls back to plain print).
fn page_through(pager: &[String], content: &str) -> Result<()> {
    use std::process::{Command, Stdio};
    let (prog, args) = pager
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty pager"))?;
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        // Ignore a broken pipe (user quit the pager early).
        let _ = stdin.write_all(content.as_bytes());
    }
    let _ = child.wait()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Chapter X

Intro prose.

---

### `aida init`

**One line** — bootstrap.

Body of init.

---

### `aida away` · `aida home` · `aida presence`

**One line** — presence trio.

Body of presence.

### `aida graph`

**One line** — graph queries.

Body of graph.
";

    #[test]
    fn extracts_a_simple_section() {
        let s = extract_section(SAMPLE, "init").expect("init section");
        assert!(s.starts_with("### `aida init`"));
        assert!(s.contains("Body of init."));
        // Must stop before the next entry.
        assert!(!s.contains("presence trio"));
    }

    #[test]
    fn extracts_last_section_to_eof() {
        let s = extract_section(SAMPLE, "graph").expect("graph section");
        assert!(s.contains("Body of graph."));
        assert!(!s.contains("presence trio"));
    }

    #[test]
    fn matches_a_command_listed_in_a_multi_command_header() {
        let s = extract_section(SAMPLE, "home").expect("home matches the trio header");
        assert!(s.starts_with("### `aida away` · `aida home` · `aida presence`"));
        assert!(s.contains("presence trio"));
        assert!(!s.contains("Body of graph."));
    }

    #[test]
    fn unknown_command_returns_none() {
        assert!(extract_section(SAMPLE, "nonesuch").is_none());
    }

    #[test]
    fn header_matches_exact_only() {
        assert!(header_matches("`aida init`", "init"));
        assert!(header_matches("`aida away` · `aida home`", "home"));
        // `aida list` must not match a request for `lis`.
        assert!(!header_matches("`aida list`", "lis"));
        assert!(!header_matches("`aida list`", "list-extra"));
    }
}
