//! Ratchet: `Requirement::display_id()` is the CANONICAL id. Human and TOON
//! output shows `friendly_id()` / `node_alias::display_spec_id()` so a node's
//! friendly label (`TASK-HOME-095`) appears instead of the 32-hex node id.
//! Every remaining unmarked `.display_id()` in non-test source is counted;
//! the count may only go down.
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude

use std::path::Path;

/// Unmarked `.display_id()` occurrences in non-test `aida-cli-lib/src` code
/// after the STORY-078 output migration. Lower it when you migrate more.
const BASELINE: usize = 172;

const MARKER: &str = "canonical-id";

/// Count unmarked `.display_id()` calls in `src`, skipping comment lines,
/// marked lines, and test code (`#[cfg(test)]` followed by an inline module
/// ends the scan; a one-line `#[path] mod x;` declaration is skipped).
fn unmarked_display_id_calls(src: &str) -> usize {
    let mut count = 0;
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed == "#[cfg(test)]" {
            // Skip attribute lines up to the item, then decide.
            while let Some(next) = lines.peek() {
                if next.trim().starts_with("#[") || next.trim().starts_with("//") {
                    lines.next();
                } else {
                    break;
                }
            }
            match lines.next() {
                Some(item) if item.trim().ends_with(';') => continue,
                Some(item) if item.contains("mod ") => break,
                _ => continue,
            }
        }
        if trimmed.starts_with("//") || line.contains(MARKER) {
            continue;
        }
        count += line.matches(".display_id()").count();
    }
    count
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, usize)>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" {
                walk(root, &path, out);
            }
            continue;
        }
        if !name.ends_with(".rs") || name.ends_with("_tests.rs") || name == "tests.rs" {
            continue;
        }
        let n = unmarked_display_id_calls(&std::fs::read_to_string(&path).unwrap());
        if n > 0 {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            out.push((rel.display().to_string(), n));
        }
    }
}

#[test]
fn scanner_skips_comments_markers_and_test_modules() {
    let src = "fn a(r: &R) { println!(\"{}\", r.display_id()); }\n\
               // r.display_id() in a comment\n\
               let k = r.display_id(); // canonical-id: map key\n\
               #[cfg(test)]\n#[path = \"tests/x.rs\"]\nmod x;\n\
               fn b(r: &R) -> String { r.display_id() }\n\
               #[cfg(test)]\nmod tests {\n    fn c(r: &R) { r.display_id(); }\n}\n";
    assert_eq!(unmarked_display_id_calls(src), 2);
}

#[test]
fn unmigrated_display_id_print_sites_only_go_down() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    walk(&src_root, &src_root, &mut hits);
    hits.sort();
    let total: usize = hits.iter().map(|(_, n)| n).sum();
    let listing = hits
        .iter()
        .map(|(p, n)| format!("  {n:>3}  {p}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        total <= BASELINE,
        "{total} unmarked `.display_id()` calls (baseline {BASELINE}).\n\
         Human/TOON output must use `friendly_id()` / `node_alias::display_spec_id()` \
         so friendly node labels show. Stored data, trailers, map keys and JSON \
         identity fields keep `display_id()` — mark such a new line with \
         `// canonical-id`.\n{listing}"
    );
    assert!(
        total == BASELINE,
        "only {total} unmarked `.display_id()` calls remain — lower BASELINE in \
         {} from {BASELINE} to {total} so the ratchet holds.",
        file!()
    );
}
