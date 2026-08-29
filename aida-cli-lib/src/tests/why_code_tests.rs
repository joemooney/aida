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

// ── STORY-785: git-blame trailer fallback ────────────────────────────────────

#[test]
fn commit_trailers_yield_spec_ids_but_scopes_and_pr_numbers_do_not() {
    use super::spec_ids_from_commit_message;
    // The exact shapes this repo's convention produces.
    assert_eq!(
        spec_ids_from_commit_message("[AI:claude] feat(auth): add login validation (FR-0042)"),
        vec!["FR-0042"],
        "the trailer resolves; the lowercase (auth) scope must not"
    );
    // A squash-merge subject carries both the trailer and the PR number.
    assert_eq!(
        spec_ids_from_commit_message("feat(scan): walk roots (STORY-3) (#4)"),
        vec!["STORY-3"],
        "(#4) must not false-positive"
    );
    // Node-aware distributed ids keep their internal dashes.
    assert_eq!(
        spec_ids_from_commit_message("fix: reconcile (TASK-1-097)"),
        vec!["TASK-1-097"]
    );
    assert!(spec_ids_from_commit_message("chore(deps): update dependencies").is_empty());
}

#[test]
fn trace_markers_in_a_commit_body_also_resolve_and_dedupe() {
    use super::spec_ids_from_commit_message;
    let msg = "feat(x): thing (BUG-793)\n\nbody line\ntrace:BUG-793 | ai:claude\ntrace:TASK-1181\n";
    assert_eq!(
        spec_ids_from_commit_message(msg),
        vec!["BUG-793", "TASK-1181"],
        "trailer + body traces, deduped, trailer first"
    );
}

/// Build a throwaway repo whose one commit carries a `(SPEC-ID)` trailer, and
/// check blame resolves the line to that commit — the whole point of the
/// fallback: an UNANNOTATED line still answers, from the commit convention.
#[test]
fn blame_resolves_an_unannotated_line_to_its_trailered_commit() {
    use super::{blame_line_origin, spec_ids_from_commit_message};
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("code.rs"), "fn a() {}\nfn b() {}\n").unwrap();
    run(&["add", "-A"]);
    run(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "-m",
        "[AI:claude] feat(core): add b (STORY-999)\n\ntrace:STORY-999 | ai:claude",
    ]);

    let origin = blame_line_origin(dir.path(), "code.rs", 2).expect("committed line blames");
    assert_eq!(origin.subject, "[AI:claude] feat(core): add b (STORY-999)");
    assert!(!origin.short_sha.is_empty());
    assert_eq!(
        spec_ids_from_commit_message(&origin.message),
        vec!["STORY-999"]
    );
}

#[test]
fn an_uncommitted_line_yields_none_not_a_wrong_commit() {
    use super::blame_line_origin;
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .args(args)
            .output()
            .expect("git runs");
    };
    run(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("f.rs"), "fn a() {}\n").unwrap();
    run(&["add", "-A"]);
    run(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "-m",
        "init (TASK-1)",
    ]);
    // Append an uncommitted line — blame reports the all-zero sha for it.
    std::fs::write(
        dir.path().join("f.rs"),
        "fn a() {}\nfn new_uncommitted() {}\n",
    )
    .unwrap();

    assert_eq!(
        blame_line_origin(dir.path(), "f.rs", 2),
        None,
        "an uncommitted line must fall through to the not-linked-yet message"
    );
}

#[test]
fn outside_a_repo_blame_is_a_clean_none() {
    use super::blame_line_origin;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.rs"), "x\n").unwrap();
    assert_eq!(blame_line_origin(dir.path(), "f.rs", 1), None);
}
