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
const OWNER: &str = "aida-cli-lib/src/seat_authority.rs";

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

/// TASK-1976: the reads [`is_env_read`] misses, found in one file's
/// production text with its whitespace collapsed: a call rustfmt split over
/// lines, a read through an alias (`use ...GRANT_ENV as G;`,
/// `let k = ...GRANT_ENV;`) and a scan of `env::vars()` that names the
/// variable or an alias in the same statement. Returns each offending
/// statement.
// trace:TASK-1952 | trace:TASK-1976 | ai:claude
fn split_or_aliased_env_reads(production: &str) -> Vec<String> {
    // Most files never read the environment: skip them before any regex
    // work, so the guard stays inside its time budget.
    if !production.contains("env::var") {
        return Vec::new();
    }
    static ALIAS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\bGRANT_ENV as (\w+)|\blet (\w+)\b[^=;]*= &?[\w:]*\bGRANT_ENV ?;")
            .unwrap()
    });
    let text = production.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut names = vec!["GRANT_ENV".to_string(), GRANT_LITERAL.to_string()];
    for capture in ALIAS.captures_iter(&text) {
        let name = capture.get(1).or(capture.get(2)).unwrap().as_str();
        names.push(name.to_string());
    }
    let names = names
        .iter()
        .map(|n| regex::escape(n))
        .collect::<Vec<_>>()
        .join("|");
    let read = regex::Regex::new(&format!(
        r"env::var(?:_os)? ?\( ?&? ?(?:[\w:]*::)?(?:{names})\b[^;]*"
    ))
    .unwrap();
    let scan = regex::Regex::new(&format!(
        r"env::vars(?:_os)? ?\( ?\)[^;]*\b(?:{names})\b[^;]*"
    ))
    .unwrap();
    read.find_iter(&text)
        .chain(scan.find_iter(&text))
        .map(|m| m.as_str().to_string())
        .collect()
}

/// TASK-1977: the `tests` directories the scan skips hold test code only if
/// every module declaration that reaches into one is `#[cfg(test)]`. Returns
/// each production `mod` declaration (`mod tests;`, or `#[path = "...tests/..."]`)
/// that lacks that attribute, as `(line number, declaration)`.
// trace:TASK-1952 | trace:TASK-1977 | ai:claude
fn ungated_test_dir_modules(lines: &[(usize, &str)]) -> Vec<(usize, String)> {
    static MODULE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^(pub(\([^)]*\))? )?mod (\w+);").unwrap());
    static ALL_TEST: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"[(,]\s*test\s*[,)]").unwrap());
    let module = &*MODULE;
    let mut found = Vec::new();
    for (at, (line, text)) in lines.iter().enumerate() {
        let Some(capture) = module.captures(text.trim()) else {
            continue;
        };
        let attrs: Vec<&str> = lines[..at]
            .iter()
            .rev()
            .map(|(_, l)| l.trim())
            .take_while(|l| l.starts_with("#[") || l.starts_with("//"))
            .collect();
        let into_tests = &capture[3] == "tests"
            || attrs
                .iter()
                .any(|a| a.starts_with("#[path") && a.contains("tests/"));
        // `cfg(test)` or `cfg(all(test, ...))`; never `any(...)`, which also
        // compiles without test.
        let test_only = |a: &&str| {
            a.starts_with("#[cfg(test)]") || (a.starts_with("#[cfg(all(") && ALL_TEST.is_match(a))
        };
        if into_tests && !attrs.iter().any(test_only) {
            found.push((*line, text.trim().to_string()));
        }
    }
    found
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
        // trace:TASK-1952 | trace:TASK-1976 | ai:claude
        let production = lines.iter().map(|(_, t)| *t).collect::<Vec<_>>().join("\n");
        for read in split_or_aliased_env_reads(&production) {
            offenders.push(format!("{}: reads the grant env: {read}", rel.display()));
        }
        // trace:TASK-1952 | trace:TASK-1977 | ai:claude
        for (line, decl) in ungated_test_dir_modules(&lines) {
            offenders.push(format!(
                "{}:{line}: `{decl}` reaches a skipped tests/ directory without #[cfg(test)]",
                rel.display()
            ));
        }
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

// TASK-1976: the split, aliased and vars() reads the line check misses.
// trace:TASK-1952 | trace:TASK-1976 | ai:claude
#[test]
fn scan_catches_split_aliased_and_iterated_reads() {
    for source in [
        "fn a() {\n    let _ = std::env::var(\n        crate::seat_authority::GRANT_ENV,\n    );\n}\n",
        "use crate::seat_authority::GRANT_ENV as G;\nfn a() { let _ = std::env::var(G); }\n",
        "fn a() {\n    let k = seat_authority::GRANT_ENV;\n    let _ = std::env::var_os(k);\n}\n",
        "fn a() { let _ = std::env::vars().find(|(k, _)| k == GRANT_ENV); }\n",
    ] {
        assert_eq!(
            split_or_aliased_env_reads(source).len(),
            1,
            "missed: {source}"
        );
    }
    for allowed in [
        "fn a(c: &mut Command) { c.env(crate::seat_authority::GRANT_ENV, \"x\"); }\n",
        "fn a() { let n = crate::seat_authority::GRANT_ENV; println!(\"{n}\"); }\n",
        "fn a() { let _ = std::env::var(\"AIDA_SESSION_ID\"); }\n",
    ] {
        assert!(
            split_or_aliased_env_reads(allowed).is_empty(),
            "flagged: {allowed}"
        );
    }
}

// TASK-1977: a module reaching a skipped tests/ directory must be cfg(test).
// trace:TASK-1952 | trace:TASK-1977 | ai:claude
#[test]
fn scan_requires_cfg_test_on_modules_in_skipped_test_dirs() {
    let source = "\
#[cfg(test)]
#[path = \"tests/a_tests.rs\"]
mod a_tests;
#[path = \"tests/b.rs\"]
mod b;
#[cfg(test)]
mod tests;
pub(crate) mod tests;
mod plain;
#[cfg(all(test, feature = \"native\"))]
#[path = \"tests/c.rs\"]
mod c;
#[cfg(any(test, feature = \"x\"))]
#[path = \"tests/d.rs\"]
mod d;
";
    let lines = production_lines(source).unwrap();
    let found: Vec<usize> = ungated_test_dir_modules(&lines)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(found, vec![5, 8, 15]);
}
