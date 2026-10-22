//! TASK-1752: the session grant env var is read in ONE module, `seat_authority`.
//! BUG-2011 happened because two functions validated a grant differently; a
//! module that names the variable itself can grow a third validator. Other
//! modules ask `grant_check` / `current_grant_id`, or set and strip a child's
//! grant through `seat_authority::GRANT_ENV`. This guard scans every crate's
//! production source, comments included.
// trace:TASK-1752 | ai:claude

use std::path::{Path, PathBuf};

const CRATES: &[&str] = &[
    "aida-core",
    "aida-cli-lib",
    "aida-cli",
    "aida-server",
    "aida-tui",
];

const GRANT_LITERAL: &str = "AIDA_SESSION_GRANT";

/// The one production file that may name the variable or read it.
const OWNER: &str = "aida-cli-lib/src/gate/seat_authority.rs";

/// (path suffix, exact line text, why) — the only other production lines
/// allowed to name the variable. Matched per line, so nothing else in the
/// same file is exempt.
const ALLOWED_LITERAL_LINES: &[(&str, &str, &str)] = &[(
    "aida-cli-lib/src/role_cmd_args.rs",
    "/// seat session; it also exits 3 when `AIDA_SESSION_GRANT` is set but its",
    "clap help text for `aida role expiry`: users set the variable by name",
)];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Unit-test and integration-test files set the variable freely.
            if path.file_name().is_some_and(|n| n == "tests") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The line index just past the `{ … }` block opened on line `open`, found
/// by a lexer that skips braces inside comments and string, raw-string and
/// char literals, whose state carries across lines. `None` when the block
/// never closes.
fn block_end(lines: &[&str], open: usize) -> Option<usize> {
    let text = lines[open..].join("\n");
    let chars: Vec<char> = text.chars().collect();
    let mut depth = 0i64;
    let mut line = open;
    let mut i = 0;
    let at = |i: usize| chars.get(i).copied();
    while i < chars.len() {
        match chars[i] {
            '\n' => line += 1,
            '/' if at(i + 1) == Some('/') => {
                while at(i + 1).is_some_and(|c| c != '\n') {
                    i += 1;
                }
            }
            '/' if at(i + 1) == Some('*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && at(i + 1) == Some('/')) {
                    line += usize::from(chars[i] == '\n');
                    i += 1;
                }
                i += 1;
            }
            'r' if at(i + 1) == Some('"') || at(i + 1) == Some('#') => {
                let mut j = i + 1;
                let mut hashes = 0;
                while at(j) == Some('#') {
                    hashes += 1;
                    j += 1;
                }
                if at(j) == Some('"') {
                    let close: String = std::iter::once('"')
                        .chain(std::iter::repeat_n('#', hashes))
                        .collect();
                    let close: Vec<char> = close.chars().collect();
                    i = j + 1;
                    while i < chars.len() && !chars[i..].starts_with(&close) {
                        line += usize::from(chars[i] == '\n');
                        i += 1;
                    }
                    i += close.len() - 1;
                } else {
                    i = j - 1;
                }
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    line += usize::from(at(i) == Some('\n'));
                    i += 1;
                }
            }
            '\'' if at(i + 2) == Some('\'') => i += 2,
            '\'' if at(i + 1) == Some('\\') => {
                while at(i + 1).is_some_and(|c| c != '\'') {
                    i += 1;
                }
                i += 1;
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(line + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Production lines `(line number, text)`: everything except the bodies of
/// inline `#[cfg(test)] mod name { … }` blocks, which may sit anywhere in a
/// file. `Err` when such a block never closes, so a brace the counter
/// misreads fails the guard instead of silently exempting the file's tail.
fn production_lines(source: &str) -> Result<Vec<(usize, &str)>, String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let code = lines[i].trim_start();
        if code.starts_with("#[cfg(test)]") {
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim_start().starts_with("#[") {
                j += 1;
            }
            let item = lines.get(j).map(|l| l.trim_start()).unwrap_or("");
            let item = item.strip_prefix("pub(crate) ").unwrap_or(item);
            let item = item.strip_prefix("pub ").unwrap_or(item);
            if item.starts_with("mod ") && item.contains('{') {
                let Some(k) = block_end(&lines, j) else {
                    return Err(format!("#[cfg(test)] mod at line {} never closes", j + 1));
                };
                i = k;
                continue;
            }
        }
        out.push((i + 1, lines[i]));
        i += 1;
    }
    Ok(out)
}

/// A direct read of the variable from this process's environment.
fn is_env_read(code: &str) -> bool {
    (code.contains("env::var(") || code.contains("env::var_os("))
        && (code.contains("GRANT_ENV") || code.contains(GRANT_LITERAL))
}

#[test]
fn only_seat_authority_names_or_reads_the_grant_env_var() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let mut files = Vec::new();
    for krate in CRATES {
        rust_files(&root.join(krate).join("src"), &mut files);
    }
    assert!(
        files.iter().any(|f| f.ends_with(OWNER)),
        "the scan must reach seat_authority.rs, or it is scanning nothing"
    );

    let mut offenders = Vec::new();
    let mut used_exemptions = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(root).unwrap_or(file);
        if rel.ends_with(OWNER) {
            continue;
        }
        let source = std::fs::read_to_string(file).expect("read source");
        let lines = production_lines(&source).unwrap_or_else(|e| panic!("{}: {e}", rel.display()));
        for (line, text) in lines {
            let code = text.trim();
            if is_env_read(code) {
                offenders.push(format!(
                    "{}:{line}: reads the grant env: {code}",
                    rel.display()
                ));
                continue;
            }
            if !code.contains(GRANT_LITERAL) {
                continue;
            }
            let exempt = ALLOWED_LITERAL_LINES
                .iter()
                .position(|(suffix, allowed, _)| rel.ends_with(suffix) && code == *allowed);
            match exempt {
                Some(at) => used_exemptions.push(at),
                None => offenders.push(format!("{}:{line}: {code}", rel.display())),
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the session grant env var is named or read outside seat_authority.rs; ask \
         seat_authority::grant_check (or current_grant_id for the raw handle), and set or \
         strip a child's grant with seat_authority::GRANT_ENV:\n{}",
        offenders.join("\n")
    );
    for (at, (suffix, allowed, _)) in ALLOWED_LITERAL_LINES.iter().enumerate() {
        assert!(
            used_exemptions.contains(&at),
            "stale exemption: {suffix} no longer contains `{allowed}`; remove it"
        );
    }
}

#[test]
fn scan_skips_inline_test_modules_anywhere_and_keeps_comments() {
    let source = "\
#[cfg(test)]
mod early_tests {
    fn t() { let s = \"}\"; let c = '{'; set(\"AIDA_SESSION_GRANT\"); }
    const M: &str = \"multi-line {
        string\";
    const R: &str = r#\"raw } \"quoted\" \"#; /* } */
}
/// doc naming AIDA_SESSION_GRANT
fn a() { std::env::var(crate::seat_authority::GRANT_ENV); }
#[cfg(test)]
#[path = \"tests/x.rs\"]
mod x;
fn b() {}
";
    let lines = production_lines(source).expect("balanced");
    let numbers: Vec<usize> = lines.iter().map(|(n, _)| *n).collect();
    assert_eq!(numbers, vec![8, 9, 10, 11, 12, 13], "{lines:?}");
    assert!(lines[0].1.contains(GRANT_LITERAL));
    assert!(is_env_read(lines[1].1));
}

#[test]
fn scan_fails_closed_on_an_unclosed_test_module() {
    let source = "#[cfg(test)]\nmod tests {\n    fn t() {}\n";
    assert!(production_lines(source).is_err());
}
