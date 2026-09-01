//! STORY-781: the checked-in project manifest, `.aida/project.toml`.
//!
//! These cover the CLI-side behaviour — scaffolding, the gitignore allow-line,
//! the init self-commit allow-list and the doctor category. The format itself
//! (parsing, malformed handling, fact derivation) is tested in
//! `aida_core::project_manifest`.
//!
//! trace:STORY-781 | ai:claude

use super::*;
use aida_core::project_manifest as pm;
use aida_core::{Requirement, RequirementStatus, RequirementType};

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "aida-story781-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn empty_store() -> aida_core::RequirementsStore {
    aida_core::RequirementsStore::new()
}

fn store_with_task() -> aida_core::RequirementsStore {
    let mut store = empty_store();
    let mut req = Requirement::new(
        "A real project task".to_string(),
        "ship something".to_string(),
    );
    req.req_type = RequirementType::Task;
    req.status = RequirementStatus::Approved;
    store.add_requirement_with_id(req, None, Some("task"));
    store
}

fn store_with_vision() -> aida_core::RequirementsStore {
    let mut store = store_with_task();
    let mut req = Requirement::new(
        "Project thesis".to_string(),
        "What bet is this project making?\n\nThe bet.".to_string(),
    );
    req.req_type = RequirementType::Vision;
    store.add_requirement_with_id(req, None, Some("vision"));
    store
}

// ── scaffolding ──────────────────────────────────────────────────────────────

#[test]
fn scaffold_writes_a_manifest_that_parses() {
    let root = tmp("write");
    assert!(ensure_project_manifest_scaffold(&root, false).unwrap());
    let state = pm::load(&root);
    assert!(
        matches!(state, pm::ManifestState::Present(_)),
        "scaffolded manifest must parse: {state:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn scaffold_is_prefilled_not_a_blank_form() {
    // The whole design rests on this: an empty form is how a metadata standard
    // goes stale in a week.
    let root = tmp("prefill");
    std::fs::write(
        root.join("README.md"),
        "# thing\n\nA tool that does things.\n",
    )
    .unwrap();
    ensure_project_manifest_scaffold(&root, false).unwrap();

    let m = match pm::load(&root) {
        pm::ManifestState::Present(m) => m,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        m.project.description.as_deref(),
        Some("A tool that does things.")
    );
    assert!(
        m.project.name.is_some(),
        "the directory name is always knowable"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn scaffold_never_overwrites_a_human_edit() {
    // This is the `aida init --refresh` guarantee. The manifest has no
    // canonical master to overlay, so the honest behaviour is to leave it be.
    let root = tmp("preserve");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let hand_written = "[project]\nwhy = \"because I kept forgetting\"\n";
    std::fs::write(root.join(pm::MANIFEST_REL_PATH), hand_written).unwrap();

    assert!(
        !ensure_project_manifest_scaffold(&root, false).unwrap(),
        "must report that it wrote nothing"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(pm::MANIFEST_REL_PATH)).unwrap(),
        hand_written,
        "an existing manifest must be byte-identical afterwards"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn repeated_scaffolding_is_idempotent() {
    let root = tmp("idem");
    assert!(ensure_project_manifest_scaffold(&root, false).unwrap());
    let first = std::fs::read_to_string(root.join(pm::MANIFEST_REL_PATH)).unwrap();
    assert!(!ensure_project_manifest_scaffold(&root, false).unwrap());
    assert!(!ensure_project_manifest_scaffold(&root, false).unwrap());
    assert_eq!(
        std::fs::read_to_string(root.join(pm::MANIFEST_REL_PATH)).unwrap(),
        first
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn force_overwrites_matching_the_other_scaffolds() {
    let root = tmp("force");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(pm::MANIFEST_REL_PATH),
        "[project]\nwhy = \"mine\"\n",
    )
    .unwrap();
    assert!(ensure_project_manifest_scaffold(&root, true).unwrap());
    let after = std::fs::read_to_string(root.join(pm::MANIFEST_REL_PATH)).unwrap();
    assert!(
        after.contains("AIDA project manifest"),
        "--force should rewrite"
    );
    std::fs::remove_dir_all(&root).ok();
}

// ── the gitignore allow-line ─────────────────────────────────────────────────
//
// Without it the manifest is silently ignored and never checked in, which
// defeats the entire purpose of a manifest that travels with the repository.

#[test]
fn a_fresh_gitignore_allow_lists_the_manifest() {
    let root = tmp("gi-fresh");
    add_aida_gitignore_entries(&root, ".aida-store").unwrap();
    let content = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(
        content.contains("!.aida/project.toml"),
        "deny-by-default `.aida/*` needs an explicit allow-line:\n{content}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_already_initialized_project_still_gets_the_allow_line() {
    // THE case that would silently break this feature. The block carrying
    // `!.aida/config.toml` is only appended when `.aida/*` is ABSENT, so a repo
    // initialized before this change would never receive a newly-added
    // allow-line — and its manifest would be invisible to git forever.
    let root = tmp("gi-existing");
    std::fs::write(
        root.join(".gitignore"),
        "target/\n.aida-store/\n.aida-store\n.aida/*\n!.aida/config.toml\nCLAUDE.local.md\n\
         .claude/rules/aida-specs/\ndocs/plans/_draft/\n.claude/settings.local.json\n",
    )
    .unwrap();

    let wrote = add_aida_gitignore_entries(&root, ".aida-store").unwrap();
    assert!(wrote, "must append something to an existing gitignore");
    let content = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(
        content.contains("!.aida/project.toml"),
        "an existing project must still get the allow-line:\n{content}"
    );
    // And it must not have duplicated the deny block while doing so.
    assert_eq!(
        content.matches(".aida/*").count(),
        1,
        "the deny-by-default block must not be duplicated:\n{content}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn the_allow_line_is_not_appended_twice() {
    let root = tmp("gi-idem");
    add_aida_gitignore_entries(&root, ".aida-store").unwrap();
    add_aida_gitignore_entries(&root, ".aida-store").unwrap();
    add_aida_gitignore_entries(&root, ".aida-store").unwrap();
    let content = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert_eq!(
        content.matches("!.aida/project.toml").count(),
        1,
        "idempotent append:\n{content}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_operator_added_allow_line_is_respected() {
    let root = tmp("gi-operator");
    std::fs::write(
        root.join(".gitignore"),
        ".aida/*\n!.aida/config.toml\n!.aida/project.toml\n",
    )
    .unwrap();
    add_aida_gitignore_entries(&root, ".aida-store").unwrap();
    let content = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert_eq!(
        content.matches("!.aida/project.toml").count(),
        1,
        "must not duplicate an operator's own line:\n{content}"
    );
    std::fs::remove_dir_all(&root).ok();
}

// ── init self-commit ─────────────────────────────────────────────────────────

#[test]
fn the_manifest_is_in_the_init_commit_allow_list() {
    // init stages an explicit allow-list, never `git add .`. A tracked file
    // missing from it is written but never committed.
    assert!(
        crate::init_cmd::init_scaffold_candidate_paths().contains(&".aida/project.toml"),
        "the manifest must be staged by init's self-commit"
    );
}

// ── doctor ───────────────────────────────────────────────────────────────────

#[test]
fn doctor_category_accepts_the_documented_aliases() {
    for alias in [
        "project-manifest",
        "project-manifests",
        "manifest",
        "manifests",
        "project-metadata",
        "PROJECT_MANIFEST",
    ] {
        assert_eq!(
            normalize_doctor_category(alias).unwrap(),
            "project-manifest",
            "alias {alias}"
        );
    }
}

#[test]
fn doctor_says_nothing_when_there_is_no_manifest() {
    // Load-bearing: absence is not a warning, an error, or a low score. Every
    // pre-existing repository must stay clean.
    let root = tmp("doc-absent");
    assert!(
        crate::doctor_cmd::scan_project_manifest(&root, &empty_store()).is_empty(),
        "a project without a manifest must produce no finding"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn doctor_reports_a_malformed_manifest_without_breaking() {
    let root = tmp("doc-bad");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(root.join(pm::MANIFEST_REL_PATH), "[project\nwhy = ").unwrap();

    let f = crate::doctor_cmd::scan_project_manifest(&root, &empty_store());
    assert_eq!(f.len(), 1, "expected exactly one finding, got {f:?}");
    assert_eq!(f[0].id, "project-manifest-malformed");
    assert!(!f[0].safe_heal, "never auto-heal a human's file");
    assert!(
        f[0].action.contains("Nothing else is affected"),
        "must reassure that other commands keep working: {}",
        f[0].action
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn doctor_is_quiet_for_a_fresh_scaffolded_but_unfilled_manifest() {
    let root = tmp("doc-unfilled");
    ensure_project_manifest_scaffold(&root, false).unwrap();
    let f = crate::doctor_cmd::scan_project_manifest(&root, &empty_store());
    assert!(
        f.is_empty(),
        "a fresh project must not be nagged before it proves it is real: {f:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn doctor_reports_unfilled_manifest_once_the_project_has_real_specs() {
    let root = tmp("doc-unfilled-real");
    ensure_project_manifest_scaffold(&root, false).unwrap();
    let f = crate::doctor_cmd::scan_project_manifest(&root, &store_with_task());
    assert!(
        f.iter().any(|x| x.id == "project-manifest-unfilled"),
        "a blank form is exactly the staleness this checks for: {f:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn doctor_is_quiet_once_the_manifest_says_something() {
    let root = tmp("doc-filled");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(pm::MANIFEST_REL_PATH),
        "[project]\nwhy = \"because I kept forgetting my projects\"\n",
    )
    .unwrap();
    assert!(
        crate::doctor_cmd::scan_project_manifest(&root, &store_with_vision()).is_empty(),
        "a filled-in manifest with no remote recorded is clean"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn doctor_reports_an_unrecognised_value_without_rejecting_the_file() {
    // The file still parses — degrade, not reject — but the author is told,
    // because otherwise they believe they said something and nothing read it.
    let root = tmp("doc-unknown");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(pm::MANIFEST_REL_PATH),
        "[project]\nwhy = \"x\"\nliveness = \"sort-of\"\n",
    )
    .unwrap();

    let f = crate::doctor_cmd::scan_project_manifest(&root, &empty_store());
    let hit = f
        .iter()
        .find(|x| x.id == "project-manifest-unrecognised-value")
        .unwrap_or_else(|| panic!("expected an unrecognised-value finding, got {f:?}"));
    assert!(
        hit.summary.contains("sort-of"),
        "must quote the value: {}",
        hit.summary
    );
    assert!(
        hit.summary.contains("alive"),
        "must list what IS accepted: {}",
        hit.summary
    );
    assert!(
        hit.action.contains("upgrade"),
        "a newer-schema manifest is a real cause: {}",
        hit.action
    );
    assert!(!f.iter().any(|x| x.id == "project-manifest-malformed"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn doctor_nudges_for_missing_thesis_only_after_real_project_signals() {
    let root = tmp("doc-thesis-real");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(pm::MANIFEST_REL_PATH),
        "[project]\nwhy = \"because the existing workflow hurts\"\n",
    )
    .unwrap();

    let fresh = crate::doctor_cmd::scan_project_manifest(&root, &empty_store());
    assert!(
        !fresh.iter().any(|x| x.id == "project-thesis-missing"),
        "fresh/dormant directories should not be nagged: {fresh:?}"
    );

    let real = crate::doctor_cmd::scan_project_manifest(&root, &store_with_task());
    assert!(
        real.iter().any(|x| x.id == "project-thesis-missing"),
        "real project with no VISION should get a thesis nudge: {real:?}"
    );

    let with_vision = crate::doctor_cmd::scan_project_manifest(&root, &store_with_vision());
    assert!(
        !with_vision.iter().any(|x| x.id == "project-thesis-missing"),
        "an existing VISION is the thesis home: {with_vision:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn doctor_does_not_nudge_parked_or_abandoned_projects_for_thesis() {
    for liveness in ["parked", "abandoned"] {
        let root = tmp(&format!("doc-thesis-{liveness}"));
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(pm::MANIFEST_REL_PATH),
            format!("[project]\nwhy = \"x\"\nliveness = \"{liveness}\"\n"),
        )
        .unwrap();
        let f = crate::doctor_cmd::scan_project_manifest(&root, &store_with_task());
        assert!(
            !f.iter().any(|x| x.id == "project-thesis-missing"),
            "dormant {liveness} project should stay quiet: {f:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

#[test]
fn equivalent_remote_url_forms_are_not_reported_as_drift() {
    // Comparing raw strings would flag every project whose manifest was
    // written from one URL form while its remote uses another.
    for (a, b) in [
        (
            "https://github.com/joemooney/aida.git",
            "git@github.com:joemooney/aida.git",
        ),
        (
            "https://github.com/joemooney/aida",
            "https://github.com/joemooney/aida.git",
        ),
        (
            "ssh://git@github.com/joemooney/aida.git",
            "https://github.com/joemooney/aida/",
        ),
    ] {
        assert!(
            crate::doctor_cmd::same_remote(a, b),
            "{a} and {b} name the same repository"
        );
    }
}

#[test]
fn genuinely_different_remotes_are_drift() {
    assert!(!crate::doctor_cmd::same_remote(
        "https://github.com/joemooney/aida.git",
        "https://gitlab.com/joemooney/aida.git"
    ));
    assert!(!crate::doctor_cmd::same_remote(
        "https://github.com/joemooney/aida.git",
        "https://github.com/someone/aida.git"
    ));
}
