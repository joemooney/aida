use super::{looks_like_code_arg, resolve_spec_from_markdown, why_first_sentence};

#[test]
fn markdown_fallback_resolves_intent_from_a_bare_folder() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("specs")).unwrap();
    // Frontmatter id + title + status.
    std::fs::write(
            dir.path().join("specs/STORY-1.md"),
            "---\nid: STORY-1\ntitle: Rate-limit login\nstatus: completed\n---\nWe saw stuffing attacks. Throttle it.\n",
        )
        .unwrap();
    let it = resolve_spec_from_markdown(dir.path(), "story-1").expect("case-insensitive id");
    assert_eq!(it.title, "Rate-limit login");
    assert_eq!(it.status.as_deref(), Some("completed"));
    assert_eq!(it.why, "We saw stuffing attacks.");
    assert!(it.markdown.is_some());

    // By filename, title from the first `#` heading, no frontmatter.
    std::fs::write(
        dir.path().join("STORY-2.md"),
        "# Second thing\n\nBecause reasons.\n",
    )
    .unwrap();
    let it2 = resolve_spec_from_markdown(dir.path(), "STORY-2").expect("by filename");
    assert_eq!(it2.title, "Second thing");
    assert_eq!(it2.why, "Because reasons.");

    // Unknown id → None (no false positive).
    assert!(resolve_spec_from_markdown(dir.path(), "NOPE-9").is_none());
}

#[test]
fn code_args_vs_spec_ids_are_distinguished() {
    // SPEC-IDs → spec path (false).
    for id in ["STORY-750", "BUG-1", "FR-1-042", "TASK-1088"] {
        assert!(!looks_like_code_arg(id), "{id} should read as a SPEC-ID");
    }
    // Code locations → code path (true).
    for loc in [
        "src/main.rs",
        "aida-cli/src/main.rs:40408",
        "foo.rs",
        "path/to/thing",
    ] {
        assert!(looks_like_code_arg(loc), "{loc} should read as code");
    }
}

#[test]
fn first_sentence_strips_markdown_headers_and_truncates() {
    // Leading `## Why` header is dropped; prose starts clean.
    let s = why_first_sentence("## Why\n\nThe cache was stale. More detail here.");
    assert_eq!(s, "The cache was stale.");
    // No sentence break → char-safe truncation with an ellipsis.
    let long = "x".repeat(300);
    let out = why_first_sentence(&long);
    assert!(out.ends_with('…'));
    assert!(out.len() <= 161 + "…".len());
    // Empty / header-only → empty.
    assert_eq!(why_first_sentence("### Heading only"), "");
}
