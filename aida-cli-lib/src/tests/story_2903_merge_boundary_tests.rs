//! STORY-2903 S1c: one production merge boundary. No production code calls
//! `Forge::merge_change` except `landing_merge::merge`, so a new merge path
//! cannot skip the landing checks.
// trace:STORY-2903 | ai:claude

/// Production sources with a `.merge_change(` call, outside test files and
/// after-`#[cfg(test)]` modules.
fn merge_change_callers() -> Vec<String> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "tests") {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // Inline test modules sit at the end of a file by convention.
            let production = text.split("#[cfg(test)]").next().unwrap_or("");
            for (n, line) in production.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if code.contains(".merge_change(") && !code.contains("concat!") {
                    found.push(format!(
                        "{}:{}",
                        path.strip_prefix(&src).unwrap().display(),
                        n + 1
                    ));
                }
            }
        }
    }
    found.sort();
    found
}

#[test]
fn only_the_landing_boundary_calls_merge_change() {
    let callers = merge_change_callers();
    assert_eq!(callers.len(), 1, "{callers:?}");
    assert!(
        callers[0].starts_with("landing_merge.rs:"),
        "every merge path goes through landing_merge::merge: {callers:?}"
    );
}
