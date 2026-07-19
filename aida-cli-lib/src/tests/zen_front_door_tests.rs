use super::*;
use clap::CommandFactory;
use tempfile::TempDir;

/// IMPROVEMENT 1: a free-text thought files a Draft spec, and the new id is
/// what the drive (mocked here via `drive_args`, not actually spawned) would
/// be handed. Forces the fallback path (a fabricated `DraftedThought`) so no
/// AI transport runs.
#[test]
fn free_text_thought_files_a_draft_then_would_drive_it() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join(".aida").join("cache.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let storage = Storage::new(db_path);

    let drafted =
        zen_drive::compose_draft_from_thought("make the tree header show the parent title", None);
    let (spec_id, source) = file_drafted_thought(&storage, drafted).unwrap();
    assert_eq!(source, zen_drive::DraftSource::Fallback);

    // The draft landed: exactly one spec, born Draft, titled from the thought.
    let store = storage.load().unwrap();
    assert_eq!(store.requirements.len(), 1);
    let req = &store.requirements[0];
    assert_eq!(req.status, RequirementStatus::Draft);
    assert_eq!(req.title, "make the tree header show the parent title");
    assert!(req.tags.contains("from-thought"));

    // The drive (mocked) would be handed exactly the new id — the same argv
    // `run_zen_drive` self-invokes.
    let args = zen_drive::drive_args(&spec_id, None, false, false);
    assert_eq!(args[..3], ["queue", "work", spec_id.as_str()]);
    assert!(args.contains(&"--auto-complete".to_string()));
}

/// IMPROVEMENT 1: an AI draft yields genuine acceptance criteria in the
/// filed body.
#[test]
fn ai_drafted_thought_files_acceptance_criteria() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join(".aida").join("cache.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let storage = Storage::new(db_path);

    let ai = aida_core::DraftSpecResponse {
        title: "Show the parent title in the tree header".to_string(),
        description: "Render the parent spec's title in the tree view header.".to_string(),
        acceptance_criteria: vec!["The header shows the parent's title".to_string()],
    };
    let drafted = zen_drive::compose_draft_from_thought("tree header", Some(ai));
    let (_id, source) = file_drafted_thought(&storage, drafted).unwrap();
    assert_eq!(source, zen_drive::DraftSource::Ai);

    let store = storage.load().unwrap();
    let req = &store.requirements[0];
    assert!(req.description.contains("## Acceptance"));
    assert!(req
        .description
        .contains("The header shows the parent's title"));
}

/// STORY-736: `aida zen "<thought>" --dry-run` renders the spec it WOULD
/// draft — title + description + acceptance — instead of echoing the thought
/// back. The renderer is pure (composes + formats, persists nothing), so we
/// exercise both the AI-drafted and the offline-fallback shapes here, and
/// confirm a real spec-id is still routed to the per-spec preview path.
#[test]
fn zen_dry_run_renders_drafted_spec() {
    // ── AI-drafted thought: title + description + acceptance bullets ──────
    let ai = aida_core::DraftSpecResponse {
        title: "Add a dark mode toggle".to_string(),
        description: "Let the user switch the UI between light and dark themes.".to_string(),
        acceptance_criteria: vec![
            "A toggle in settings switches the theme".to_string(),
            "The chosen theme persists across restarts".to_string(),
        ],
    };
    let drafted = zen_drive::compose_draft_from_thought("add a dark mode toggle", Some(ai));
    assert_eq!(drafted.source, zen_drive::DraftSource::Ai);
    let rendered = render_drafted_thought_preview(&drafted);

    // Title is rendered, not the raw flat echo.
    assert!(rendered.contains("Add a dark mode toggle"));
    assert!(!rendered.contains("would draft + file + drive: add a dark mode toggle"));
    // Structured blocks: description prose + an Acceptance section + each bullet.
    assert!(rendered.contains("Description:"));
    assert!(rendered.contains("Let the user switch the UI"));
    assert!(rendered.contains("Acceptance"));
    assert!(rendered.contains("A toggle in settings switches the theme"));
    assert!(rendered.contains("The chosen theme persists across restarts"));
    // The lifecycle line closes the preview.
    assert!(rendered.contains("Run without --dry-run to drive it."));
    // The raw `## Acceptance` markdown heading is NOT shown verbatim (it is
    // re-rendered as a labelled block) and neither is the provenance footer.
    assert!(!rendered.contains("## Acceptance"));
    assert!(!rendered.contains("_Drafted by"));

    // ── Offline fallback: no AI → still a rendered draft, flagged as such ─
    let fallback = zen_drive::compose_draft_from_thought("add a dark mode toggle", None);
    assert_eq!(fallback.source, zen_drive::DraftSource::Fallback);
    let rendered_fb = render_drafted_thought_preview(&fallback);
    assert!(rendered_fb.contains("add a dark mode toggle"));
    assert!(rendered_fb.contains("no AI was reachable"));
    assert!(rendered_fb.contains("Run without --dry-run to drive it."));

    // ── A real spec-id is NOT free text → the free-text renderer is skipped,
    // so the existing per-spec dry-run preview path still applies.
    assert!(zen_drive::looks_like_spec_id("STORY-736"));
    assert!(!zen_drive::looks_like_spec_id("add a dark mode toggle"));
}

/// IMPROVEMENT 2: `aida zen --help` no longer lists the introspection
/// subcommands (status / finish / needs-human) at the top level — yet they
/// still PARSE.
#[test]
fn zen_help_hides_introspection_subcommands_but_keeps_them_runnable() {
    let mut cli = crate::cli::Cli::command();
    let zen = cli
        .get_subcommands_mut()
        .find(|c| c.get_name() == "zen")
        .expect("zen subcommand exists");
    let help = zen.render_help().to_string();
    assert!(
        !help.contains("needs-human"),
        "zen --help must not list the introspection subcommands:\n{help}"
    );
    // The visible-subcommand list must be empty (all three are hidden).
    assert!(
        crate::cli::Cli::command()
            .get_subcommands()
            .find(|c| c.get_name() == "zen")
            .unwrap()
            .get_subcommands()
            .all(|s| s.is_hide_set()),
        "every zen subcommand must be hidden from --help"
    );

    // Still runnable: the parser accepts `aida zen status`.
    let parsed = crate::cli::Cli::try_parse_from(["aida", "zen", "status"]);
    assert!(parsed.is_ok(), "aida zen status must still parse");
}

/// IMPROVEMENT 1 (detection): a real spec id keeps existing behavior; free
/// text routes to the front door.
#[test]
fn detection_splits_spec_id_from_thought() {
    assert!(zen_drive::looks_like_spec_id("TASK-123"));
    assert!(zen_drive::looks_like_spec_id("FR-1-042"));
    assert!(!zen_drive::looks_like_spec_id(
        "make the tree header show the parent title"
    ));
}
