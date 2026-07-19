use super::*;

#[test]
fn redact_secrets_collapses_known_token_shapes() {
    let cases = [
        "token is ghp_abcdEFGH1234567890abcdEFGH1234567890",
        "key sk-ABCdef0123456789ABCdef0123456789",
        "aws AKIAIOSFODNN7EXAMPLE here",
        "Authorization: Bearer eyJsupersecretvalue",
        "password=hunter2 in the log",
        "api_key = s3cr3tvalue trailing",
    ];
    for c in cases {
        let out = redact_secrets(c);
        assert!(
            out.contains("[REDACTED]"),
            "expected redaction in {c:?} → {out:?}"
        );
    }
    // A normal sentence + a short SHA is untouched.
    let clean = "Completed via merge a1b2c3d4 after reviewer approved";
    assert_eq!(redact_secrets(clean), clean);
}

#[test]
fn parse_pr_number_from_url_github_and_gitlab() {
    assert_eq!(
        parse_pr_number_from_url("https://github.com/o/r/pull/123#issuecomment-9"),
        Some(123)
    );
    assert_eq!(
        parse_pr_number_from_url("https://gitlab.com/o/r/-/merge_requests/45"),
        Some(45)
    );
    assert_eq!(
        parse_pr_number_from_url("https://github.com/o/r/issues/7"),
        None
    );
    assert_eq!(parse_pr_number_from_url("not a url"), None);
}

#[test]
fn add_processing_record_is_idempotent_on_commit_sha() {
    let mut req = aida_core::Requirement::new("Test".to_string(), "desc".to_string());
    let mut rec = aida_core::ProcessingRecord::new("claude".into(), "did the thing".into());
    rec.commit_sha = Some("deadbeef".into());
    assert!(req.add_processing_record(rec.clone()));
    // Same SHA → not duplicated.
    assert!(!req.add_processing_record(rec.clone()));
    assert_eq!(req.processing_record.len(), 1);
    // A different SHA → appended.
    let mut rec2 = rec.clone();
    rec2.commit_sha = Some("cafef00d".into());
    assert!(req.add_processing_record(rec2));
    assert_eq!(req.processing_record.len(), 2);
    // An empty/None SHA always appends (no idempotency key).
    let rec3 = aida_core::ProcessingRecord::new("claude".into(), "manual".into());
    assert!(req.add_processing_record(rec3.clone()));
    assert!(req.add_processing_record(rec3));
    assert_eq!(req.processing_record.len(), 4);
}

#[test]
fn build_processing_record_falls_back_with_no_artifacts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // No brief, no verdict, no punts → agent "aida", summary from sha.
    let rec = build_processing_record(root, "STORY-582", "abcdef1234567890");
    assert_eq!(rec.agent, "aida");
    assert_eq!(rec.commit_sha.as_deref(), Some("abcdef1234567890"));
    assert!(rec.summary.contains("abcdef12"), "{}", rec.summary);
    assert!(rec.brief_ref.is_none());
    assert!(rec.review_verdict.is_none());
}

#[test]
fn build_processing_record_promotes_verdict() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let vdir = root.join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&vdir).unwrap();
    std::fs::write(
            vdir.join("STORY-582.json"),
            r#"{"verdict":"Approved","summary":"clean diff, good tests","comment_url":"https://github.com/o/r/pull/811#c1"}"#,
        )
        .unwrap();
    let rec = build_processing_record(root, "STORY-582", "abcdef1234567890");
    assert_eq!(rec.review_verdict.as_deref(), Some("Approved"));
    assert_eq!(rec.summary, "clean diff, good tests");
    assert_eq!(rec.pr, Some(811));
}

#[test]
fn latest_brief_for_spec_prefers_newest_and_reads_generated_by() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dir = root.join(".aida").join("agent-briefs").join("advisor");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("STORY-582-20260101T000000Z.md"),
        "---\ngenerated_by: codex\n---\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("STORY-582-20260612T120000Z.md"),
        "---\ngenerated_by: claude\n---\n",
    )
    .unwrap();
    let (rel, agent, gb) = latest_brief_for_spec(root, "STORY-582").unwrap();
    assert!(rel.ends_with("20260612T120000Z.md"), "{rel}");
    assert_eq!(agent, "advisor");
    assert_eq!(gb.as_deref(), Some("claude"));
}
