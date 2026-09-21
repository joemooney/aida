use super::*;
use aida_core::models::Comment;

fn comment(content: &str) -> Comment {
    Comment::new("codex".to_string(), content.to_string())
}

#[test]
fn review_prompt_extracts_newest_stable_marked_implementer_approach() {
    let comments = vec![
        comment("## Implementer approach\n\n### Expected files\n- old.rs"),
        comment("I mentioned the implementer approach, but this is not the marker."),
        comment(
            "## Implementer approach\n\n### Acceptance criteria as tests\n- **Unit:** regression\n\n### Expected files\n- new.rs\n\n### Ambiguities / conflicts as questions\n- None",
        ),
    ];

    let approach = extract_implementer_approach(&comments).expect("marked approach");
    assert!(approach.contains("**Unit:** regression"));
    assert!(approach.contains("new.rs"));
    assert!(!approach.contains("old.rs"));

    let mut prompt = String::new();
    append_implementer_approach(&mut prompt, &comments);
    assert!(prompt.contains("#### Recorded implementer approach"));
    assert!(prompt.contains("## Implementer approach"));
    assert!(prompt.contains("**Unit:** regression"));
    assert!(prompt.contains("new.rs"));
}

#[test]
fn review_prompt_ignores_unmarked_approach_discussion() {
    let comments = vec![comment(
        "Reviewer note: the implementer approach should have covered the caller.",
    )];

    assert_eq!(extract_implementer_approach(&comments), None);
}

/// THE COUPLING TEST. The marker is a handshake between a TEMPLATE and a
/// PARSER with nothing in the type system connecting them: the pickup skill
/// writes the heading, `extract_implementer_approach` reads it. Editing the
/// template alone would silently stop every review from carrying an approach,
/// with no error anywhere — the failure is invisible precisely because both
/// sides still "work".
///
/// Reading the embedded master rather than the scaffolded copy is deliberate:
/// the master is what ships, and a project's local copy can legitimately lag.
// trace:STORY-1350 | ai:claude
#[test]
fn pickup_template_and_reader_share_one_marker() {
    let template = include_str!("../../../aida-core/templates/skills/aida-pickup.md");
    assert!(
        template.contains(IMPLEMENTER_APPROACH_MARKER),
        "the pickup skill no longer writes `{IMPLEMENTER_APPROACH_MARKER}`, so \
         `extract_implementer_approach` can never match it again. Change both \
         sides or neither."
    );
}

/// Drive the REAL prompt generator, not just the helpers it calls. The helpers
/// being correct does not prove the prompt carries their output — that is the
/// seam, and a helper-only test cannot see it.
// trace:STORY-1350 | ai:claude
#[test]
fn generate_review_prompt_carries_the_approach_and_says_so_when_absent() {
    use aida_core::Storage;

    fn prompt_for(comments: Vec<Comment>) -> String {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join("requirements.yaml");
        let storage = Storage::new(&store_path);

        let mut req = aida_core::Requirement::new("Widget".to_string(), "desc".to_string());
        req.spec_id = Some("STORY-13500".to_string());
        req.comments = comments;
        let mut store = aida_core::RequirementsStore::default();
        store.requirements.push(req);
        storage.save(&store).unwrap();

        let out_path = tmp.path().join("prompt.md");
        generate_review_prompt(
            &storage,
            Some("STORY-13500"),
            None,
            None,
            Some(out_path.to_str().unwrap()),
        )
        .expect("prompt generates");
        std::fs::read_to_string(&out_path).unwrap()
    }

    // PRESENT: the marked approach reaches the prompt through the generator.
    let present = prompt_for(vec![comment(
        "## Implementer approach\n\n### Expected files\n- widget.rs",
    )]);
    assert!(
        present.contains("#### Recorded implementer approach"),
        "{present}"
    );
    assert!(present.contains("widget.rs"), "{present}");
    assert!(!present.contains("_None recorded._"), "{present}");

    // ABSENT: the prompt SAYS there is none rather than omitting the section.
    // Silence would be indistinguishable from the section having been dropped.
    let absent = prompt_for(vec![comment("just a normal comment")]);
    assert!(
        absent.contains("#### Recorded implementer approach"),
        "the section must exist even when empty: {absent}"
    );
    assert!(absent.contains("_None recorded._"), "{absent}");
    assert!(!absent.contains("widget.rs"), "{absent}");
    // The absence PROSE must name the same marker the parser matches. It is
    // the string that tells a human what to write; if it drifts from the
    // constant, the system parses one heading while instructing people to
    // write another — and that instruction is the only place most readers
    // ever learn the heading.
    assert!(
        absent.contains(IMPLEMENTER_APPROACH_MARKER),
        "the none-recorded note must name the marker the reader matches: {absent}"
    );
}
