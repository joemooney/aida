use super::*;

fn write_config(dir: &std::path::Path, body: &str) {
    std::fs::create_dir_all(dir.join(".aida")).unwrap();
    std::fs::write(dir.join(".aida").join("config.toml"), body).unwrap();
}

#[test]
fn read_block_allocation_config_returns_defaults_when_file_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = read_block_allocation_config(tmp.path());
    assert!(cfg.is_enabled_for("BUG"));
    assert_eq!(cfg.threshold_for("BUG"), 20);
    assert_eq!(cfg.size_for("BUG"), 100);
}

#[test]
fn read_block_allocation_config_returns_defaults_when_section_missing() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[id_format]\npolicy = \"blocks-then-fallback\"\n",
    );
    let cfg = read_block_allocation_config(tmp.path());
    assert!(cfg.is_enabled_for("BUG"));
    assert_eq!(cfg.threshold_for("TASK"), 20);
}

#[test]
fn read_block_allocation_config_parses_global_opt_out() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[block_allocation]\nauto_claim = false\n");
    let cfg = read_block_allocation_config(tmp.path());
    assert!(!cfg.is_enabled_for("BUG"));
    assert!(!cfg.is_enabled_for("TASK"));
}

#[test]
fn read_block_allocation_config_parses_per_type_section() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[block_allocation]\nauto_claim = true\n\n\
             [block_allocation.bug]\nauto_claim_threshold = 50\nauto_claim_size = 200\n\n\
             [block_allocation.story]\nauto_claim = false\n",
    );
    let cfg = read_block_allocation_config(tmp.path());
    assert!(cfg.is_enabled_for("BUG"));
    assert_eq!(cfg.threshold_for("BUG"), 50);
    assert_eq!(cfg.size_for("BUG"), 200);
    assert!(!cfg.is_enabled_for("STORY"));
    // Untouched types still use built-in defaults.
    assert!(cfg.is_enabled_for("TASK"));
    assert_eq!(cfg.threshold_for("TASK"), 20);
}

#[test]
fn read_block_allocation_config_handles_malformed_toml_gracefully() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "not [valid toml at all\n");
    let cfg = read_block_allocation_config(tmp.path());
    // Should not panic — falls back to defaults.
    assert!(cfg.is_enabled_for("BUG"));
}

#[test]
fn read_block_allocation_config_ignores_negative_threshold() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[block_allocation.bug]\nauto_claim_threshold = -5\n",
    );
    let cfg = read_block_allocation_config(tmp.path());
    assert_eq!(cfg.threshold_for("BUG"), 20);
}

// trace:TASK-444 | ai:claude — the per-type continuation line under
// each row of `aida db block status` is derived from these summaries.
#[test]
fn auto_claim_summary_shows_built_in_defaults_when_no_config() {
    let cfg = aida_core::BlockAllocationConfig::default();
    assert_eq!(
        auto_claim_summary(&cfg, "BUG"),
        "auto-claim: threshold 20, size 100"
    );
}

#[test]
fn auto_claim_summary_marks_configured_when_threshold_overridden() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[block_allocation.bug]\nauto_claim_threshold = 50\n",
    );
    let cfg = read_block_allocation_config(tmp.path());
    assert_eq!(
        auto_claim_summary(&cfg, "BUG"),
        "auto-claim: threshold 50, size 100 (configured)"
    );
}

#[test]
fn auto_claim_summary_marks_configured_when_size_overridden() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[block_allocation.bug]\nauto_claim_size = 250\n",
    );
    let cfg = read_block_allocation_config(tmp.path());
    assert_eq!(
        auto_claim_summary(&cfg, "BUG"),
        "auto-claim: threshold 20, size 250 (configured)"
    );
}

#[test]
fn auto_claim_summary_no_configured_tag_when_only_auto_claim_bool_set() {
    // `auto_claim = true/false` is opt-out plumbing, not a threshold
    // override — don't mislead the reader by tagging it (configured).
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[block_allocation]\nauto_claim = true\n\n\
             [block_allocation.bug]\nauto_claim = true\n",
    );
    let cfg = read_block_allocation_config(tmp.path());
    assert_eq!(
        auto_claim_summary(&cfg, "BUG"),
        "auto-claim: threshold 20, size 100"
    );
}

#[test]
fn auto_claim_summary_reports_global_opt_out() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[block_allocation]\nauto_claim = false\n");
    let cfg = read_block_allocation_config(tmp.path());
    assert_eq!(
        auto_claim_summary(&cfg, "BUG"),
        "auto-claim: off (global opt-out)"
    );
    assert_eq!(
        auto_claim_summary(&cfg, "TASK"),
        "auto-claim: off (global opt-out)"
    );
}

#[test]
fn auto_claim_summary_reports_per_type_opt_out() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[block_allocation]\nauto_claim = true\n\n\
             [block_allocation.story]\nauto_claim = false\n",
    );
    let cfg = read_block_allocation_config(tmp.path());
    assert_eq!(
        auto_claim_summary(&cfg, "STORY"),
        "auto-claim: off (per-type opt-out)"
    );
    // Other types unaffected.
    assert_eq!(
        auto_claim_summary(&cfg, "BUG"),
        "auto-claim: threshold 20, size 100"
    );
}

// trace:TASK-449 | ai:claude — empty-blocks branch of `aida db block
// status` calls `global_auto_claim_summary` (no type prefix in scope
// yet); these cases cover what the user sees before their first claim.
#[test]
fn global_auto_claim_summary_shows_built_in_defaults_when_no_config() {
    let cfg = aida_core::BlockAllocationConfig::default();
    assert_eq!(
        global_auto_claim_summary(&cfg),
        "auto-claim: threshold 20, size 100"
    );
}

#[test]
fn global_auto_claim_summary_reports_global_opt_out() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[block_allocation]\nauto_claim = false\n");
    let cfg = read_block_allocation_config(tmp.path());
    assert_eq!(
        global_auto_claim_summary(&cfg),
        "auto-claim: off (global opt-out)"
    );
}

/// TASK-467: global off + a per-type re-enable must surface the per-type
/// wiring in the empty-blocks summary, not a flat "off (global opt-out)".
#[test]
fn global_auto_claim_summary_surfaces_per_type_re_enable() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[block_allocation]\nauto_claim = false\n\n[block_allocation.bug]\nauto_claim = true\n",
    );
    let cfg = read_block_allocation_config(tmp.path());
    assert_eq!(
        global_auto_claim_summary(&cfg),
        "auto-claim: off globally (re-enabled per-type)"
    );
}

#[test]
fn global_auto_claim_summary_picks_up_star_override() {
    // `*` isn't a bare TOML key, so users write `[block_allocation."*"]`
    // to override the catch-all defaults. The parser stores it under
    // the literal `*` per_type slot, which `cfg.threshold_for("*")` /
    // `cfg.size_for("*")` resolve.
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[block_allocation.\"*\"]\nauto_claim_threshold = 30\nauto_claim_size = 250\n",
    );
    let cfg = read_block_allocation_config(tmp.path());
    assert_eq!(
        global_auto_claim_summary(&cfg),
        "auto-claim: threshold 30, size 250 (configured)"
    );
}

#[test]
fn global_auto_claim_summary_tags_configured_for_any_per_type_section() {
    // User has configured BUG specifically but not `*`. The global
    // line still shows defaults but tags (configured) to signal that
    // `[block_allocation]` is wired up — per-type details surface
    // when that type's row appears in the populated branch.
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[block_allocation.bug]\nauto_claim_threshold = 50\n",
    );
    let cfg = read_block_allocation_config(tmp.path());
    assert_eq!(
        global_auto_claim_summary(&cfg),
        "auto-claim: threshold 20, size 100 (configured)"
    );
}

#[test]
fn auto_claim_summary_per_type_re_enable_wins_over_global_off() {
    // Global off, per-type explicitly on with overrides → summary
    // reflects the per-type re-enable + (configured) tag.
    let tmp = tempfile::tempdir().unwrap();
    write_config(
            tmp.path(),
            "[block_allocation]\nauto_claim = false\n\n\
             [block_allocation.bug]\nauto_claim = true\nauto_claim_threshold = 10\nauto_claim_size = 50\n",
        );
    let cfg = read_block_allocation_config(tmp.path());
    assert_eq!(
        auto_claim_summary(&cfg, "BUG"),
        "auto-claim: threshold 10, size 50 (configured)"
    );
    // Untouched type still inherits the global off.
    assert_eq!(
        auto_claim_summary(&cfg, "TASK"),
        "auto-claim: off (global opt-out)"
    );
}
