use super::*;

/// TASK-589: with no flags, both glossary sections render, machinery first.
#[test]
fn default_renders_both_glossary_sections_machinery_first() {
    let out = render_discipline_glossary(false, false).unwrap();
    let m = out.find("# Machinery glossary");
    let l = out.find("# Lifecycle vocabulary");
    assert!(m.is_some(), "machinery section present");
    assert!(l.is_some(), "lifecycle section present");
    assert!(
        m < l,
        "machinery glossary renders before lifecycle vocabulary"
    );
}

/// Each filter flag isolates its own section.
#[test]
fn filter_flags_isolate_their_section() {
    let machinery = render_discipline_glossary(true, false).unwrap();
    assert!(machinery.contains("# Machinery glossary"));
    assert!(
        !machinery.contains("# Lifecycle vocabulary"),
        "--machinery omits the lifecycle section"
    );

    let lifecycle = render_discipline_glossary(false, true).unwrap();
    assert!(lifecycle.contains("# Lifecycle vocabulary"));
    assert!(
        !lifecycle.contains("# Machinery glossary"),
        "--lifecycle omits the machinery section"
    );
}

/// Passing both flags is the same as passing neither — show everything.
#[test]
fn both_flags_render_both_sections() {
    let out = render_discipline_glossary(true, true).unwrap();
    assert!(out.contains("# Machinery glossary") && out.contains("# Lifecycle vocabulary"));
}
