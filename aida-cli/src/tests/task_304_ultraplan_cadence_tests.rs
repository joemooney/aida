use super::*;

fn write_config(root: &std::path::Path, body: &str) {
    let config_dir = root.join(".aida");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), body).unwrap();
}

fn req_with(spec_id: &str, description: &str) -> aida_core::Requirement {
    let mut r = aida_core::Requirement::new("chunky spec".to_string(), description.to_string());
    r.spec_id = Some(spec_id.to_string());
    r
}

const NINE_BULLETS: &str = "## Why\n\nsome prose\n\n## Acceptance\n\n\
        - [ ] one\n- [ ] two\n- [ ] three\n- [ ] four\n- [ ] five\n\
        - [ ] six\n- [ ] seven\n- [ ] eight\n- [ ] nine\n\n## Out of scope\n\n- [ ] not counted\n";

// ── mode token parsing ──────────────────────────────────────────────
// trace:TASK-304 | ai:claude
#[test]
fn mode_tokens_parse() {
    assert_eq!(
        UltraplanMode::from_token("never"),
        Some(UltraplanMode::Never)
    );
    assert_eq!(
        UltraplanMode::from_token("on-demand"),
        Some(UltraplanMode::OnDemand)
    );
    assert_eq!(
        UltraplanMode::from_token(" Suggested "),
        Some(UltraplanMode::Suggested)
    );
    assert_eq!(UltraplanMode::from_token("frequently"), None);
}

// trace:TASK-304 | ai:claude
#[test]
fn threshold_token_parses_and_rejects() {
    assert_eq!(
        parse_acceptance_bullet_threshold("acceptance-bullets>8"),
        Some(8)
    );
    assert_eq!(
        parse_acceptance_bullet_threshold("ACCEPTANCE-BULLETS>3"),
        Some(3)
    );
    assert_eq!(
        parse_acceptance_bullet_threshold("story-with-design-forks"),
        None
    );
    assert_eq!(
        parse_acceptance_bullet_threshold("acceptance-bullets>abc"),
        None
    );
}

// ── bullet counting ─────────────────────────────────────────────────
/// Counts only `## Acceptance` checkbox bullets, both checked and
// unchecked; ignores bullets in other sections. trace:TASK-304 | ai:claude
#[test]
fn counts_acceptance_bullets_only() {
    assert_eq!(count_acceptance_bullets(NINE_BULLETS), 9);
}

// trace:TASK-304 | ai:claude
#[test]
fn counts_checked_and_unchecked() {
    let d = "## Acceptance\n- [ ] todo\n- [x] done\n- [X] also done\n";
    assert_eq!(count_acceptance_bullets(d), 3);
}

// trace:TASK-304 | ai:claude
#[test]
fn counts_zero_without_acceptance_section() {
    let d = "## Why\n- [ ] not acceptance\n- [ ] still not\n";
    assert_eq!(count_acceptance_bullets(d), 0);
}

// Tolerates `## Acceptance Criteria` heading variants. trace:TASK-304
#[test]
fn tolerates_acceptance_criteria_heading() {
    let d = "## Acceptance Criteria\n- [ ] a\n- [ ] b\n";
    assert_eq!(count_acceptance_bullets(d), 2);
}

// ── read_ultraplan_config: each mode + threshold ────────────────────
/// Absent config → on-demand default, threshold = thinness (TASK-697).
// trace:TASK-697 | ai:claude
#[test]
fn config_absent_defaults_on_demand() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = read_ultraplan_config(tmp.path());
    assert_eq!(cfg.mode, UltraplanMode::OnDemand);
    assert_eq!(cfg.threshold, SuggestThreshold::Thinness);
}

// trace:TASK-304 | ai:claude
#[test]
fn config_never_mode_parses() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[ultraplan]\nmode = \"never\"\n");
    assert_eq!(read_ultraplan_config(tmp.path()).mode, UltraplanMode::Never);
}

// trace:TASK-304 | ai:claude
#[test]
fn config_suggested_mode_with_custom_threshold() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[ultraplan]\nmode = \"suggested\"\nsuggest_threshold = \"acceptance-bullets>3\"\n",
    );
    let cfg = read_ultraplan_config(tmp.path());
    assert_eq!(cfg.mode, UltraplanMode::Suggested);
    assert_eq!(cfg.threshold, SuggestThreshold::BulletCount(3));
}

/// Unknown mode token falls back to the on-demand default rather than
// erroring. trace:TASK-304 | ai:claude
#[test]
fn config_unknown_mode_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[ultraplan]\nmode = \"frequently\"\n");
    assert_eq!(
        read_ultraplan_config(tmp.path()).mode,
        UltraplanMode::OnDemand
    );
}

// ── ultraplan_suggestion_hint: mode × threshold matrix ──────────────
// on-demand never hints, even for a chunky spec. trace:TASK-304
#[test]
fn hint_silent_on_demand() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[ultraplan]\nmode = \"on-demand\"\n");
    let r = req_with("STORY-9", NINE_BULLETS);
    assert!(ultraplan_suggestion_hint(tmp.path(), &r).is_none());
}

// never never hints. trace:TASK-304 | ai:claude
#[test]
fn hint_silent_never() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[ultraplan]\nmode = \"never\"\n");
    let r = req_with("STORY-9", NINE_BULLETS);
    assert!(ultraplan_suggestion_hint(tmp.path(), &r).is_none());
}

/// suggested + legacy bullet threshold, over it → hint with the documented
// format. trace:TASK-304 | ai:claude
#[test]
fn hint_fires_suggested_over_threshold() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[ultraplan]\nmode = \"suggested\"\nsuggest_threshold = \"acceptance-bullets>8\"\n",
    );
    let r = req_with("STORY-9", NINE_BULLETS);
    let hint = ultraplan_suggestion_hint(tmp.path(), &r).expect("expected a hint");
    assert_eq!(
        hint,
        "STORY-9 has 9 acceptance bullets — `aida ultraplan STORY-9` to \
             assemble a planning prompt before implementing."
    );
}

/// suggested + legacy bullet threshold, at/under it (8 is not > 8) →
// silent. trace:TASK-304 | ai:claude
#[test]
fn hint_silent_at_threshold_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[ultraplan]\nmode = \"suggested\"\nsuggest_threshold = \"acceptance-bullets>8\"\n",
    );
    let eight = "## Acceptance\n- [ ] 1\n- [ ] 2\n- [ ] 3\n- [ ] 4\n\
            - [ ] 5\n- [ ] 6\n- [ ] 7\n- [ ] 8\n";
    let r = req_with("STORY-8", eight);
    assert!(ultraplan_suggestion_hint(tmp.path(), &r).is_none());
}

/// A custom lower threshold makes a smaller checklist trip the hint.
// trace:TASK-304 | ai:claude
#[test]
fn hint_respects_custom_threshold() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[ultraplan]\nmode = \"suggested\"\nsuggest_threshold = \"acceptance-bullets>2\"\n",
    );
    let three = "## Acceptance\n- [ ] a\n- [ ] b\n- [ ] c\n";
    let r = req_with("TASK-3", three);
    assert!(ultraplan_suggestion_hint(tmp.path(), &r).is_some());
}

/// The scaffolded `[ultraplan]` block ships mode = on-demand and the
/// spec-thinness threshold, and parses back to those defaults.
// trace:TASK-697 | ai:claude
#[test]
fn scaffolded_block_is_on_demand() {
    let section = init_ultraplan_config_section();
    assert!(section.contains("[ultraplan]"));
    assert!(section.contains("mode = \"on-demand\""));
    assert!(section.contains("suggest_threshold = \"spec-thinness\""));
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), section);
    let cfg = read_ultraplan_config(tmp.path());
    assert_eq!(cfg.mode, UltraplanMode::OnDemand);
    assert_eq!(cfg.threshold, SuggestThreshold::Thinness);
}

// ── TASK-697: spec-thinness threshold ───────────────────────────────
/// A well-specified spec carrying a `## Proposed shape` block (the
// SPIKE-8 example of where planning does NOT help). trace:TASK-697
const WELL_SPECIFIED: &str = "## Problem\n\nThe gate logic lives inline in \
        the handler, mixing pure decision logic with I/O. This makes the \
        decision tree hard to test in isolation, so regressions slip through.\n\n\
        ## Proposed shape\n\nExtract `foo_diagnose` returning an enum with the \
        Refuse / Proceed / Skip variants, then have the caller match on it.\n\n\
        ## Acceptance\n\n- [ ] pure function\n- [ ] caller matches\n";

/// A thin spec: substantive problem statement, an acceptance list, but no
// design section — where planning plausibly helps. trace:TASK-697
const THIN_SPEC: &str = "## Problem\n\nThe error message printed when a \
        pull hits a divergence is terse and does not tell the user which of \
        the two legs (code vs store) actually diverged, so they cannot tell \
        what to reconcile. We should make it actionable.\n\n## Acceptance\n\n\
        - [ ] message names the diverged leg\n- [ ] suggests the recovery cmd\n";

// trace:TASK-697 | ai:claude
#[test]
fn suggest_threshold_token_parsing() {
    assert_eq!(
        parse_suggest_threshold("acceptance-bullets>8"),
        Some(SuggestThreshold::BulletCount(8))
    );
    assert_eq!(
        parse_suggest_threshold("spec-thinness"),
        Some(SuggestThreshold::Thinness)
    );
    assert_eq!(
        parse_suggest_threshold(" THINNESS "),
        Some(SuggestThreshold::Thinness)
    );
    assert_eq!(
        parse_suggest_threshold("under-specified"),
        Some(SuggestThreshold::Thinness)
    );
    assert_eq!(parse_suggest_threshold("story-with-design-forks"), None);
}

/// `has_design_section` matches any design-marker heading at any level,
// case-insensitively. trace:TASK-697 | ai:claude
#[test]
fn detects_design_sections() {
    assert!(has_design_section("## Proposed shape\n\nfoo"));
    assert!(has_design_section("# Approach\n\nbar"));
    assert!(has_design_section("### design\n\nbaz"));
    assert!(has_design_section("## Implementation\n\nqux"));
    // Acceptance is not a design section.
    assert!(!has_design_section("## Acceptance\n- [ ] a\n"));
    assert!(!has_design_section("## Problem\n\njust prose, no design\n"));
}

/// Thin = no design section AND a substantive body. Well-specified specs
// and trivial one-liners are both not-thin. trace:TASK-697 | ai:claude
#[test]
fn is_spec_thin_classification() {
    assert!(is_spec_thin(THIN_SPEC));
    // Has a `## Proposed shape` → not thin even though it's substantive.
    assert!(!is_spec_thin(WELL_SPECIFIED));
    // Too short to be worth planning even without a design section.
    assert!(!is_spec_thin("fix the typo in the README header"));
}

/// suggested + thinness default fires on a design-less spec with the
// thinness-flavored message. trace:TASK-697 | ai:claude
#[test]
fn hint_fires_on_thin_spec() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[ultraplan]\nmode = \"suggested\"\n");
    let r = req_with("TASK-12", THIN_SPEC);
    let hint = ultraplan_suggestion_hint(tmp.path(), &r).expect("expected a hint");
    assert_eq!(
        hint,
        "TASK-12 has no design section yet — `aida ultraplan TASK-12` to \
             assemble a planning prompt before implementing."
    );
}

/// suggested + thinness default stays SILENT on a well-specified spec —
/// the core SPIKE-8 correction: bullet count would have fired here, but the
// spec already carries its plan. trace:TASK-697 | ai:claude
#[test]
fn hint_silent_on_well_specified_spec() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[ultraplan]\nmode = \"suggested\"\n");
    let r = req_with("TASK-500", WELL_SPECIFIED);
    assert!(ultraplan_suggestion_hint(tmp.path(), &r).is_none());
}
