//! TASK-2707 (STORY-2238 decision 3, threat T7): the shared SQLite cache is a
//! rebuildable read projection that contained lanes may write, so it must never
//! decide authority. Grants, roles, seats, review records, merge holds and
//! mandates are read from the store and the grant records only.
//!
//! A source ratchet: no production line in an authority module names the
//! cache. A new need for cached data in these modules goes through the store
//! (`GitBackend`, `load_store_for_lookup`), never the cache.
// trace:TASK-2707 | ai:claude

use std::path::{Path, PathBuf};

/// Spellings that reach the cache: its types, path helpers, the cached
/// backend, the generic opener that wraps every git store in the cache, and
/// raw SQLite.
const CACHE_TOKENS: &[&str] = &[
    "CachedGitBackend",
    "Cache::open",
    "aida_core::Cache",
    "db::Cache",
    "project_cache_path",
    "default_cache_path",
    "ensure_project_cache_dir",
    "cache_dir_in",
    "CACHE_DIR_NAME",
    "CACHE_FILE_NAME",
    "cache.db",
    "cache-db",
    ".cache()",
    "aida_core::open_or_create",
    "db::open_or_create",
    "aida_core::create_backend",
    "db::create_backend",
    "rusqlite",
    "requirements_cache",
];

/// Modules that decide authority: everything under `gate/` (dispatch, seat,
/// role, review and approval authority, review records and verdicts, merge
/// holds, mandates, the ledger), plus the grant keyring and the role model.
fn authority_sources(src: &Path) -> Vec<PathBuf> {
    let mut out = vec![
        src.join("grant_keyring.rs"),
        src.join("role.rs"),
        src.join("merge_hold.rs"),
        src.join("mandate.rs"),
    ];
    let mut stack = vec![src.join("gate")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "tests" {
                    stack.push(path);
                }
            } else if name.ends_with(".rs") && !name.ends_with("tests.rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The production part of a source file: everything before its first
/// `#[cfg(test)]`, minus `//` comments (prose may name the cache).
fn production_lines(src: &str) -> Vec<(usize, &str)> {
    src.lines()
        .enumerate()
        .take_while(|(_, line)| line.trim() != "#[cfg(test)]")
        .map(|(n, line)| (n + 1, line.split("//").next().unwrap_or("")))
        .collect()
}

fn cache_offenders(src: &str) -> Vec<(usize, &'static str)> {
    production_lines(src)
        .into_iter()
        .flat_map(|(n, code)| {
            CACHE_TOKENS
                .iter()
                .filter(move |token| code.contains(**token))
                .map(move |token| (n, *token))
        })
        .collect()
}

#[test]
fn cache_rows_never_grant_authority() {
    // Positive control: the scanner sees a planted cache read and ignores
    // comments and test code.
    let planted = "fn grant_ok(root: &Path) -> bool {\n    \
                   let c = aida_core::Cache::open(aida_core::db::project_cache_path(root));\n    \
                   c.is_ok()\n}\n\
                   // the cache.db is never authority\n\
                   #[cfg(test)]\nmod t { fn x() { CachedGitBackend::default_cache_path(p); } }\n";
    let found = cache_offenders(planted);
    assert_eq!(
        found.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        vec![2, 2, 2],
        "{found:?}"
    );

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = authority_sources(&src);
    assert!(
        sources.len() > 30,
        "the authority module set shrank unexpectedly: {sources:?}"
    );
    let mut offenders = Vec::new();
    for path in &sources {
        let text =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for (line, token) in cache_offenders(&text) {
            offenders.push(format!(
                "{}:{line} names `{token}`",
                path.strip_prefix(&src).unwrap().display()
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "authority code must read the store and grant records, never the shared \
         cache (STORY-2238 T7):\n{}",
        offenders.join("\n")
    );
}
