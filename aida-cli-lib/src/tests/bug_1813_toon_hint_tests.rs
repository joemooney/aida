use super::*;

// BUG-1813: the STORY-764 hint fires ONLY on the real auto-switch path — no
// `--format`/`AIDA_OUTPUT_FORMAT` pin, no `AIDA_AGENT_OUTPUT`, stdout piped —
// and only when stderr is a terminal (a human is watching). Everything else
// is the caller's explicit choice or a capture nobody reads, so: no hint.
// trace:BUG-1813 | ai:claude
#[test]
fn hint_applies_only_on_real_auto_switch() {
    // The interactive-human-piping case STORY-764 was built for.
    assert!(toon_switch_hint_applies(None, None, false, true));
    // A pinned format is an explicit choice — nothing to hint about, whatever
    // the pin selects.
    assert!(!toon_switch_hint_applies(
        Some(OutputFormat::Toon),
        None,
        false,
        true
    ));
    assert!(!toon_switch_hint_applies(
        Some(OutputFormat::Json),
        None,
        false,
        true
    ));
    assert!(!toon_switch_hint_applies(
        Some(OutputFormat::Human),
        None,
        false,
        true
    ));
    // An explicit AIDA_AGENT_OUTPUT (either way) is also an explicit choice.
    assert!(!toon_switch_hint_applies(None, Some("1"), false, true));
    assert!(!toon_switch_hint_applies(None, Some("0"), false, true));
    // Stdout at a terminal: no switch happened.
    assert!(!toon_switch_hint_applies(None, None, true, true));
    // Stderr not a terminal: nobody human is watching (cron / CI / MCP /
    // `2>&1` capture) — the hint would only pollute the capture.
    assert!(!toon_switch_hint_applies(None, None, false, false));
}

// BUG-1813: the once-marker lives in the project's `.aida/` when there is a
// project, falls back to `~/.aida/` outside one, and is `None` only when
// there is nowhere at all to persist it. trace:BUG-1813 | ai:claude
#[test]
fn marker_prefers_project_then_home_then_none() {
    let project = std::path::Path::new("/proj");
    let home = std::path::Path::new("/home/u");
    assert_eq!(
        toon_switch_hint_marker(Some(project), Some(home)),
        Some(project.join(".aida").join(TOON_SWITCH_HINT_MARKER))
    );
    assert_eq!(
        toon_switch_hint_marker(None, Some(home)),
        Some(home.join(".aida").join(TOON_SWITCH_HINT_MARKER))
    );
    assert_eq!(toon_switch_hint_marker(None, None), None);
}

// BUG-1813: inside a project the hint is written exactly once — the first
// call emits the text and persists the marker under `.aida/`, the second call
// (a fresh process in real life) is silent. trace:BUG-1813 | ai:claude
#[test]
fn hint_emitted_at_most_once_with_project_marker() {
    let project = tempfile::tempdir().unwrap();
    let marker = toon_switch_hint_marker(Some(project.path()), None).unwrap();
    assert!(!marker.exists());

    let mut first = Vec::new();
    assert!(emit_toon_switch_hint_once(Some(&marker), &mut first));
    let first = String::from_utf8(first).unwrap();
    assert_eq!(first.trim_end(), TOON_SWITCH_HINT);
    assert!(first.contains("--format human|toon|json"));
    assert!(
        marker.exists(),
        "marker must be persisted under the project .aida/"
    );

    let mut second = Vec::new();
    assert!(!emit_toon_switch_hint_once(Some(&marker), &mut second));
    assert!(second.is_empty(), "second invocation must be silent");
}

// BUG-1813: outside any project the marker persists under `$HOME/.aida/`, so
// a shell running `aida` from an unrelated directory sees the hint once, not
// on every invocation (the original bug: the project-only marker could not be
// written, so the once-guard never tripped). trace:BUG-1813 | ai:claude
#[test]
fn hint_emitted_at_most_once_outside_project_via_home() {
    let home = tempfile::tempdir().unwrap();
    let marker = toon_switch_hint_marker(None, Some(home.path())).unwrap();
    assert!(marker.starts_with(home.path()));

    let mut first = Vec::new();
    assert!(emit_toon_switch_hint_once(Some(&marker), &mut first));
    assert_eq!(
        String::from_utf8(first).unwrap().trim_end(),
        TOON_SWITCH_HINT
    );
    assert!(home
        .path()
        .join(".aida")
        .join(TOON_SWITCH_HINT_MARKER)
        .exists());

    for _ in 0..3 {
        let mut again = Vec::new();
        assert!(!emit_toon_switch_hint_once(Some(&marker), &mut again));
        assert!(again.is_empty(), "repeat invocations must stay silent");
    }
}

// BUG-1813: nowhere to persist the marker (no project, no home) => suppress
// the hint rather than print it on every invocation. trace:BUG-1813 | ai:claude
#[test]
fn no_marker_location_suppresses_hint() {
    let mut out = Vec::new();
    assert!(!emit_toon_switch_hint_once(None, &mut out));
    assert!(out.is_empty());
}

// BUG-1813: a marker that cannot be written (here: its parent "directory" is
// a regular file) must also suppress the hint — persist first, hint second,
// so an unpersistable hint never becomes a per-invocation one.
// trace:BUG-1813 | ai:claude
#[test]
fn unwritable_marker_suppresses_hint() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();
    let marker = toon_switch_hint_marker(Some(&blocker), None).unwrap();

    let mut out = Vec::new();
    assert!(!emit_toon_switch_hint_once(Some(&marker), &mut out));
    assert!(out.is_empty(), "an unpersistable hint must not be printed");
    assert!(!marker.exists());
}
