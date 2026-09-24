//! STORY-1418: guard the into-Completed seam (`crate::completion`).
//!
//! Every path that reaches `Completed` must stamp it through the seam so the
//! `SpecCompleted` ship record cannot be forgotten by a new path. These tests
//! scan the non-test source of aida-cli-lib and aida-core and fail when a
//! direct Completed assignment appears outside the seam, or when a file stamps
//! Completed through `mark_completed` without also emitting.
//!
//! The needles are split with `concat!` so this file can never match itself.
// trace:STORY-1418 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            // Out-of-line test modules are fixtures, not completion paths.
            if path.file_name().and_then(|n| n.to_str()) == Some("tests") {
                continue;
            }
            rust_sources(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn scanned_files() -> Vec<(String, String)> {
    let cli_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let core_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aida-core/src");
    let mut out = Vec::new();
    for (label, root) in [("aida-cli-lib", cli_src), ("aida-core", core_src)] {
        let mut files = Vec::new();
        rust_sources(&root, &mut files);
        for path in files {
            let rel = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((
                format!("{label}/{rel}"),
                std::fs::read_to_string(&path).unwrap(),
            ));
        }
    }
    out
}

/// Direct `... = RequirementStatus::Completed;` assignments and
/// `set_status_from_str("completed")` literals, counted per file. Inline
/// `#[cfg(test)] mod ... { }` blocks are stripped first (STORY-1418's
/// reviewer suggestion, done in TASK-1476): a test fixture that builds a
/// `Completed` record isn't a production path, so it shouldn't force an
/// allowlist bump — and stripping it out, rather than pinning fixture
/// counts, means the allowlist still catches a *new* production write hiding
/// behind a deleted fixture.
// trace:TASK-1476 | ai:claude
fn direct_completed_writes(source: &str) -> usize {
    let source = strip_cfg_test_mod_blocks(source);
    let assign = concat!("RequirementStatus::", "Completed;");
    let setter = concat!("set_status_from_str(\"", "completed\")");
    source.matches(assign).count() + source.to_ascii_lowercase().matches(setter).count()
}

/// Files allowed to write Completed directly, with the exact count. Anything
/// not listed must be zero. Raising a count here needs a reason: a new
/// production path that reaches Completed goes through `crate::completion`
/// instead, so it emits the ship record.
///
/// Inline `#[cfg(test)]` fixtures are stripped before counting (see
/// `strip_cfg_test_mod_blocks`), so this list only carries real production
/// allowances now — no more per-file fixture-count pins to keep in sync by
/// hand every time a test adds another `Completed` fixture.
fn allowed_direct_writes() -> BTreeMap<&'static str, usize> {
    BTreeMap::from([
        // The seam itself.
        ("aida-cli-lib/completion.rs", 1),
        // The pure in-memory setter the seam calls.
        ("aida-core/models.rs", 1),
        // GitHub import creates a record born Completed from a closed issue:
        // not a transition, so no ship record.
        ("aida-cli-lib/tracker_cmd.rs", 1),
    ])
}

/// Strip inline `#[cfg(test)] mod ... { ... }` blocks out of `source`,
/// brace-matched and aware of comments/strings/char literals so a brace
/// inside a test's JSON/YAML fixture string doesn't throw off the count.
/// Also normalises `\r\n` to `\n` first so the byte offsets this function
/// computes line up regardless of the source file's line endings.
///
/// Out-of-line `#[cfg(test)] #[path = "tests/..."] mod name;` declarations
/// have no `{ }` body here — the referenced file lives under a `tests/`
/// directory, which `rust_sources` already skips entirely — so they pass
/// through untouched.
// trace:TASK-1476 | ai:claude
fn strip_cfg_test_mod_blocks(source: &str) -> String {
    let source = source.replace("\r\n", "\n");
    let len = source.len();
    let mut out = String::with_capacity(source.len());
    let mut i = 0usize;
    while i < len {
        if source[i..].starts_with("#[cfg(test)]") {
            if let Some(brace_at) = mod_block_brace_after(&source, i + "#[cfg(test)]".len()) {
                if let Some(end) = matching_brace_end(&source, brace_at) {
                    i = end;
                    continue;
                }
            }
        }
        let ch = source[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// From `start` (just past `#[cfg(test)]`), skip whitespace/comments and any
/// further `#[...]` attributes, then require `mod <ident>` followed by `{`.
/// Returns the index of that `{`, or `None` for anything else in between —
/// notably `mod x;` (an out-of-line module, no body to strip here).
fn mod_block_brace_after(source: &str, start: usize) -> Option<usize> {
    let mut i = start;
    loop {
        i = skip_trivia(source, i);
        if source[i..].starts_with('#') {
            let mut j = skip_trivia(source, i + 1);
            if !source[j..].starts_with('[') {
                return None;
            }
            j = matching_bracket_end(source, j)?;
            i = j;
            continue;
        }
        break;
    }
    let rest = source[i..].strip_prefix("mod")?;
    if rest.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        // e.g. `module`, not the `mod` keyword.
        return None;
    }
    i += "mod".len();
    i = skip_trivia(source, i);
    let ident_start = i;
    while source[i..]
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        i += source[i..].chars().next().unwrap().len_utf8();
    }
    if i == ident_start {
        return None;
    }
    i = skip_trivia(source, i);
    source[i..].starts_with('{').then_some(i)
}

/// Skip whitespace, `//` line comments, and nested `/* */` block comments.
fn skip_trivia(source: &str, mut i: usize) -> usize {
    loop {
        let rest = &source[i..];
        if let Some(c) = rest.chars().next() {
            if c.is_whitespace() {
                i += c.len_utf8();
                continue;
            }
        }
        if rest.starts_with("//") {
            i = rest.find('\n').map_or(source.len(), |p| i + p);
            continue;
        }
        if rest.starts_with("/*") {
            if let Some(end) = skip_block_comment(source, i) {
                i = end;
                continue;
            }
        }
        break;
    }
    i
}

/// `source[i..]` starts with `/*`; returns the index just past the matching
/// (possibly nested) `*/`, or `None` if it never closes.
fn skip_block_comment(source: &str, i: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut j = i + 2;
    while j < source.len() {
        if source[j..].starts_with("/*") {
            depth += 1;
            j += 2;
        } else if source[j..].starts_with("*/") {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return Some(j);
            }
        } else {
            j += source[j..].chars().next().unwrap().len_utf8();
        }
    }
    None
}

/// `source[open..]` starts with `[`; returns the index just past its
/// matching `]`.
fn matching_bracket_end(source: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut j = open;
    while j < source.len() {
        let c = source[j..].chars().next().unwrap();
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j + 1);
                }
            }
            _ => {}
        }
        j += c.len_utf8();
    }
    None
}

/// From the index of an opening `{`, find the index just past its matching
/// `}`, skipping braces inside line/block comments, string literals (incl.
/// raw and byte strings), and char literals so they can't desync the count.
fn matching_brace_end(source: &str, open_brace_idx: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open_brace_idx;
    let len = source.len();
    while i < len {
        let rest = &source[i..];
        if rest.starts_with("//") {
            i = rest.find('\n').map_or(len, |p| i + p);
            continue;
        }
        if rest.starts_with("/*") {
            i = skip_block_comment(source, i)?;
            continue;
        }
        if let Some(end) = skip_raw_string(source, i) {
            i = end;
            continue;
        }
        let c = rest.chars().next().unwrap();
        if c == '"' {
            i = skip_normal_string(source, i);
            continue;
        }
        if c == '\'' {
            if let Some(end) = skip_char_literal(source, i) {
                i = end;
                continue;
            }
        }
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += c.len_utf8();
    }
    None
}

/// Recognise an optional `b` byte-string prefix plus a raw string
/// (`r"..."`, `r#"..."#`, `r##"..."##`, ...) starting at `source[i..]`.
/// Returns the index just past the closing quote+hashes, or `None` if this
/// isn't a raw string.
fn skip_raw_string(source: &str, i: usize) -> Option<usize> {
    let rest = &source[i..];
    let after_b = rest.strip_prefix('b').unwrap_or(rest);
    let b_len = rest.len() - after_b.len();
    let after_r = after_b.strip_prefix('r')?;
    let mut hashes = 0usize;
    let mut rest2 = after_r;
    let quote_rel = loop {
        let mut chars = rest2.char_indices();
        match chars.next() {
            Some((_, '#')) => {
                hashes += 1;
                rest2 = &rest2[1..];
            }
            Some((_, '"')) => break after_r.len() - rest2.len(),
            _ => return None,
        }
    };
    let content_start = i + b_len + 1 + quote_rel + 1;
    let closer = format!("\"{}", "#".repeat(hashes));
    let content = &source[content_start..];
    let end_rel = content.find(&closer)?;
    Some(content_start + end_rel + closer.len())
}

/// `source[i]` is an opening `"`. Returns the index just past the matching
/// closing `"`, honoring backslash escapes.
fn skip_normal_string(source: &str, i: usize) -> usize {
    let mut j = i + 1;
    let len = source.len();
    while j < len {
        let c = source[j..].chars().next().unwrap();
        if c == '\\' {
            j += c.len_utf8();
            if j < len {
                j += source[j..].chars().next().unwrap().len_utf8();
            }
            continue;
        }
        j += c.len_utf8();
        if c == '"' {
            break;
        }
    }
    j
}

/// Recognise a char literal (`'x'`, `'\n'`, `'\''`, `'\u{7B}'`) starting at
/// `source[i] == '\''`, distinguishing it from a lifetime (`'a`). Returns the
/// index just past the closing `'`, or `None` if this looks like a lifetime.
fn skip_char_literal(source: &str, i: usize) -> Option<usize> {
    let rest = &source[i + 1..];
    let mut chars = rest.char_indices();
    let (first_rel, first) = chars.next()?;
    if first == '\\' {
        let mut j = i + 1 + first_rel + first.len_utf8();
        // Bounded lookahead: real escapes (`\n`, `\'`, `\u{7B}`) close well
        // within this many bytes; a lifetime never contains a `\`.
        for _ in 0..16 {
            match source[j..].chars().next() {
                Some('\'') => return Some(j + 1),
                Some(c) => j += c.len_utf8(),
                None => return None,
            }
        }
        None
    } else {
        let after_first = i + 1 + first_rel + first.len_utf8();
        source[after_first..]
            .starts_with('\'')
            .then_some(after_first + 1)
    }
}

/// TASK-1476: the stripping helper removes an inline `#[cfg(test)] mod` test
/// fixture's `Completed` write, but keeps a production one right next to it
/// — including one hiding after a deleted/renamed fixture that used to sit
/// in the allowlist.
// trace:TASK-1476 | ai:claude
#[test]
fn strip_cfg_test_mod_blocks_drops_fixtures_but_keeps_production_code() {
    let source = r#"
fn ship(req: &mut Requirement) {
    req.status = RequirementStatus::Completed;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_marks_completed() {
        // A brace inside a string must not desync the matcher: { "a": "b" }
        let mut req = Requirement::default();
        req.status = RequirementStatus::Completed;
        assert_eq!(req.status, RequirementStatus::Completed);
    }
}
"#;
    let stripped = strip_cfg_test_mod_blocks(source);
    assert!(
        stripped.contains("req.status = RequirementStatus::Completed;"),
        "production write must survive stripping:\n{stripped}"
    );
    assert!(
        !stripped.contains("mod tests"),
        "the #[cfg(test)] mod block must be gone:\n{stripped}"
    );
    assert_eq!(
        direct_completed_writes(source),
        1,
        "only the production write should count once fixtures are stripped"
    );
}

/// A synthetic source with a new *production* write outside any test module
/// must still be caught — stripping fixtures must never blind the guard to
/// a real regression.
// trace:TASK-1476 | ai:claude
#[test]
fn a_new_production_write_outside_any_cfg_test_block_is_still_counted() {
    let source = r#"
fn sneaky_shortcut(req: &mut Requirement) {
    // Not routed through crate::completion — the guard must still see this.
    req.status = RequirementStatus::Completed;
}
"#;
    assert_eq!(
        direct_completed_writes(source),
        1,
        "a production Completed write outside #[cfg(test)] must still be counted"
    );
}

/// CRLF line endings must not change the count: `\r\n` is normalised before
/// the brace-matching pass runs.
// trace:TASK-1476 | ai:claude
#[test]
fn strip_cfg_test_mod_blocks_is_crlf_safe() {
    let unix = "fn a() {\n    req.status = RequirementStatus::Completed;\n}\n\n#[cfg(test)]\nmod tests {\n    fn b() {\n        req.status = RequirementStatus::Completed;\n    }\n}\n";
    let crlf = unix.replace('\n', "\r\n");
    assert_eq!(
        direct_completed_writes(unix),
        direct_completed_writes(&crlf)
    );
    assert_eq!(direct_completed_writes(&crlf), 1);
}

/// A nested block comment and a raw string containing an unbalanced brace
/// must not throw off the brace count that finds the end of the test
/// module.
// trace:TASK-1476 | ai:claude
#[test]
fn strip_cfg_test_mod_blocks_handles_nested_comments_and_raw_strings() {
    let source = r##"
fn keep(req: &mut Requirement) {
    req.status = RequirementStatus::Completed;
}

#[cfg(test)]
mod tests {
    /* outer /* nested { */ still a comment */
    #[test]
    fn fixture() {
        let json = r#"{ "status": "completed""#;
        let _ = json;
        req.status = RequirementStatus::Completed;
    }
}
"##;
    assert_eq!(direct_completed_writes(source), 1);
}

#[test]
fn no_direct_completed_write_outside_the_seam() {
    let allowed = allowed_direct_writes();
    let mut offenders = Vec::new();
    for (name, source) in scanned_files() {
        let count = direct_completed_writes(&source);
        let expected = allowed.get(name.as_str()).copied().unwrap_or(0);
        if count != expected {
            offenders.push(format!(
                "{name}: {count} direct Completed write(s), expected {expected}"
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "a path reaches Completed outside the into-Completed seam; route it through \
         crate::completion::transition_to_completed (or mark_completed + \
         emit_spec_completed) so it emits the ship record:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn every_file_that_stamps_completed_through_the_seam_also_emits() {
    let stamp = concat!("completion::mark_", "completed(");
    let emit = concat!("emit_spec_", "completed(");
    let mut offenders = Vec::new();
    for (name, source) in scanned_files() {
        if name == "aida-cli-lib/completion.rs" {
            continue;
        }
        if source.contains(stamp) && !source.contains(emit) {
            offenders.push(name);
        }
    }
    assert!(
        offenders.is_empty(),
        "these files stamp Completed via mark_completed but never emit the ship \
         record: {offenders:?}"
    );
}

/// The guard must actually see the seam and the known routed paths; an empty
/// scan (wrong root) would pass vacuously.
#[test]
fn the_scan_sees_the_routed_paths() {
    let files: BTreeMap<String, String> = scanned_files().into_iter().collect();
    let stamp = concat!("completion::mark_", "completed(");
    let transition = concat!("transition_to_", "completed(");
    assert!(files["aida-cli-lib/lib.rs"].contains(stamp));
    assert!(files["aida-cli-lib/lib.rs"].contains(transition));
    assert!(files["aida-cli-lib/queue_cmd.rs"].contains(transition));
    assert!(files["aida-cli-lib/git_backend_cmd.rs"].contains(stamp));
    assert!(files.contains_key("aida-core/models.rs"));
}

/// The seam emits exactly once on a real transition and not on a re-set.
#[test]
fn transition_to_completed_emits_only_on_a_real_transition() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();

    let mut req = aida_core::Requirement::new("seam".into(), String::new());
    req.set_status_from_str("Done");
    let mut persisted = 0;
    let into = crate::completion::transition_to_completed(
        &mut req,
        Some(root),
        "TASK-14180",
        "",
        "test",
        |r, prior| {
            assert!(matches!(r.status, aida_core::RequirementStatus::Completed));
            assert!(matches!(prior, aida_core::RequirementStatus::Done));
            persisted += 1;
            Ok(())
        },
    )
    .unwrap();
    assert!(into);
    assert_eq!(persisted, 1);

    // Re-setting an already-Completed spec is not a transition.
    let again = crate::completion::transition_to_completed(
        &mut req,
        Some(root),
        "TASK-14180",
        "",
        "test",
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(!again);

    let completed: Vec<_> = crate::events::read_all(root)
        .into_iter()
        .filter(|e| {
            e.spec.as_deref() == Some("TASK-14180")
                && matches!(e.kind, crate::events::EventKind::SpecCompleted { .. })
        })
        .collect();
    assert_eq!(completed.len(), 1, "exactly one ship record");
}

/// TASK-1477: a manual completion (this unit test drives `mark_completed`
/// directly, which is what `aida edit --status completed`, `aida done`,
/// queue close, and `aida promote` all route through via `transition_to_
/// completed`) stamps `implementation_info.completed_at` when it's absent —
/// before this, only the merge-driven auto-bump paths did, so a manual
/// completion fell back to `modified_at` and a later edit (tag/comment)
/// reordered it under `aida list --sort completed`.
// trace:TASK-1477 | ai:claude
#[test]
fn mark_completed_stamps_completed_at_when_absent() {
    let mut req = aida_core::Requirement::new("seam".into(), String::new());
    req.set_status_from_str("Done");
    assert!(req.implementation_info.is_none());

    crate::completion::mark_completed(&mut req);

    let stamped = req
        .implementation_info
        .as_ref()
        .and_then(|i| i.completed_at);
    assert!(
        stamped.is_some(),
        "a manual completion should stamp completed_at"
    );
}

/// A `completed_at` already on the requirement — e.g. the auto-bump paths'
/// own `info.completed_at.get_or_insert(now)`, called after `mark_completed`
/// returns — must never be overwritten by this stamp.
// trace:TASK-1477 | ai:claude
#[test]
fn mark_completed_never_overwrites_an_existing_completed_at() {
    let mut req = aida_core::Requirement::new("seam".into(), String::new());
    req.set_status_from_str("Done");
    let earlier = chrono::Utc::now() - chrono::Duration::days(3);
    req.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(earlier),
        ..Default::default()
    });

    crate::completion::mark_completed(&mut req);

    assert_eq!(
        req.implementation_info
            .as_ref()
            .and_then(|i| i.completed_at),
        Some(earlier),
        "an existing completed_at stamp must survive mark_completed"
    );
}

/// A failed persist must not emit.
#[test]
fn a_failed_persist_does_not_emit() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let mut req = aida_core::Requirement::new("seam".into(), String::new());
    req.set_status_from_str("Done");
    let result = crate::completion::transition_to_completed(
        &mut req,
        Some(root),
        "TASK-14181",
        "",
        "test",
        |_, _| Err(anyhow::anyhow!("write failed")),
    );
    assert!(result.is_err());
    assert!(crate::events::read_all(root)
        .iter()
        .all(|e| e.spec.as_deref() != Some("TASK-14181")));
}
