use super::{is_toml_kv_line, parse_role_lenient, write_atomic, RoleActivity, RoleState};

fn sample_role() -> RoleState {
    RoleState {
        name: "implementer".to_string(),
        purpose: Some("Heads-down coding.".to_string()),
        created_at: "2026-05-04T04:14:39.836786244Z".parse().unwrap(),
        last_active_at: "2026-05-17T15:40:08.879179372Z".parse().unwrap(),
        working_directory: None,
        notes: None,
        global: true,
        activity: Vec::new(),
        scope_tags: Vec::new(),
        scope_status: None,
        system_prompt: None,
    }
}

// AC5: a string field containing `"` must survive serialize → parse.
// The proper TOML serializer escapes it; this guards against any
// regression to hand-built TOML for activity-log appends.
#[test]
fn activity_string_with_quotes_round_trips() {
    let mut state = sample_role();
    state.activity.push(RoleActivity {
        spec_id: "STORY-9704".to_string(),
        action: "noted \"auto-completed\" status".to_string(),
        at: "2026-05-17T15:40:08.610383127Z".parse().unwrap(),
    });
    let serialized = toml::to_string_pretty(&state).unwrap();
    let back: RoleState = toml::from_str(&serialized).unwrap();
    assert_eq!(back.activity.len(), 1);
    assert_eq!(back.activity[0].action, "noted \"auto-completed\" status");
}

// A clean file takes the strict fast path: no warnings, no salvage.
#[test]
fn lenient_parse_fast_path_on_clean_file() {
    let mut state = sample_role();
    state.activity.push(RoleActivity {
        spec_id: "STORY-1".to_string(),
        action: "edit".to_string(),
        at: "2026-05-17T15:40:08.610383127Z".parse().unwrap(),
    });
    let content = toml::to_string_pretty(&state).unwrap();
    let (parsed, warnings) = parse_role_lenient(&content).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(parsed.name, "implementer");
    assert_eq!(parsed.activity.len(), 1);
}

// AC6: the exact BUG-228 corruption — a complete file plus a trailing
// stray `"` from a torn concurrent write. The salvage path keeps every
// valid entry, drops the junk line, and reports it.
#[test]
fn lenient_parse_salvages_stray_quote() {
    let content = "name = \"implementer\"\n\
            created_at = \"2026-05-04T04:14:39.836786244Z\"\n\
            last_active_at = \"2026-05-17T15:40:08.879179372Z\"\n\
            global = true\n\
            \n\
            [[activity]]\n\
            spec_id = \"STORY-9201\"\n\
            action = \"auto-completed\"\n\
            at = \"2026-05-17T15:40:08.879176602Z\"\n\
            \n\
            [[activity]]\n\
            spec_id = \"STORY-9704\"\n\
            action = \"auto-completed\"\n\
            at = \"2026-05-17T15:40:08.610383127Z\"\n\
            \"\n";
    // The fixture must actually reproduce the bug: strict parse fails.
    assert!(toml::from_str::<RoleState>(content).is_err());
    let (state, warnings) = parse_role_lenient(content).expect("header is salvageable");
    assert_eq!(state.name, "implementer");
    // Both valid entries survive — the stray quote is dropped, not the
    // entry it landed against.
    assert_eq!(state.activity.len(), 2);
    assert_eq!(state.activity[0].spec_id, "STORY-9201");
    assert_eq!(state.activity[1].spec_id, "STORY-9704");
    assert!(!warnings.is_empty(), "the dropped line must be reported");
}

// An [[activity]] block missing a required field can't parse as a
// RoleActivity — it is quarantined, the well-formed entry survives.
#[test]
fn lenient_parse_skips_incomplete_entry_keeps_rest() {
    let content = "name = \"implementer\"\n\
            created_at = \"2026-05-04T04:14:39.836786244Z\"\n\
            last_active_at = \"2026-05-17T15:40:08.879179372Z\"\n\
            global = true\n\
            \n\
            [[activity]]\n\
            spec_id = \"STORY-1\"\n\
            action = \"edit\"\n\
            \n\
            [[activity]]\n\
            spec_id = \"STORY-2\"\n\
            action = \"show\"\n\
            at = \"2026-05-17T15:40:08.610383127Z\"\n";
    let (state, warnings) = parse_role_lenient(content).unwrap();
    assert_eq!(state.activity.len(), 1);
    assert_eq!(state.activity[0].spec_id, "STORY-2");
    assert_eq!(warnings.len(), 1);
}

// Header corruption can't be salvaged — it must surface as an error
// rather than silently returning a half-empty role.
#[test]
fn lenient_parse_errors_on_unparseable_header() {
    let content = "name = \"implementer\nbroken = oops";
    assert!(parse_role_lenient(content).is_err());
}

#[test]
fn kv_line_detection() {
    assert!(is_toml_kv_line("spec_id = \"STORY-1\""));
    assert!(is_toml_kv_line("at=\"x\""));
    assert!(!is_toml_kv_line("\""));
    assert!(!is_toml_kv_line("[[activity]]"));
    assert!(!is_toml_kv_line("just some prose"));
}

// write_atomic replaces content cleanly and leaves no temp file behind.
#[test]
fn write_atomic_replaces_content_no_leftovers() {
    let dir = tempfile::tempdir().unwrap();
    let role_dir = dir.path().join("roles");
    std::fs::create_dir_all(&role_dir).unwrap();
    let path = role_dir.join("implementer.toml");
    write_atomic(&path, "first").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
    write_atomic(&path, "second").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    let leftovers: Vec<_> = std::fs::read_dir(&role_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
        .collect();
    assert!(leftovers.is_empty(), "atomic write left a temp file behind");
}
