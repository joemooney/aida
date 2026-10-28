//! An adopted worktree's launch context carries the plane's guidance from
//! the verified projection, and config edits and scaffolding refuse to
//! write there, so the product tree stays untouched.
// trace:STORY-1741 | ai:claude

use super::{
    describe_adoption_state, refresh_adoption_projection, render_adoption_guidance_section,
};
use aida_core::adoption_projection::{CONFIG, GUIDANCE_DIR};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

fn git(dir: &Path, args: &[&str], stdin: Option<&str>) -> String {
    let mut child = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(stdin.unwrap_or("").as_bytes()).unwrap();
    drop(input);
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A product repo with one commit and a plane carrying `settings`.
fn adopted(dir: &Path, settings: &str) -> (PathBuf, String) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"], None);
    git(dir, &["config", "user.email", "t@example.invalid"], None);
    git(dir, &["config", "user.name", "t"], None);
    std::fs::write(dir.join("README.md"), "product\n").unwrap();
    git(dir, &["add", "README.md"], None);
    git(dir, &["commit", "-q", "-m", "product"], None);
    let blob = git(dir, &["hash-object", "-w", "--stdin"], Some(settings));
    let sub = git(
        dir,
        &["mktree"],
        Some(&format!("100644 blob {blob}\tsettings.toml\n")),
    );
    let tree = git(
        dir,
        &["mktree"],
        Some(&format!("040000 tree {sub}\tadoption\n")),
    );
    let commit = git(dir, &["commit-tree", &tree, "-m", "plane"], None);
    git(dir, &["update-ref", "refs/heads/aida-store", &commit], None);
    (dir.canonicalize().unwrap(), commit)
}

const SETTINGS: &str = "schema = 1\n[settings.agents]\nvendor = \"claude\"\n\
                        [guidance]\nreview-norms = \"Review every PR against its spec.\\n\"\n";

#[test]
fn a_launch_delivers_verified_plane_guidance_and_reports_how() {
    let tmp = tempfile::tempdir().unwrap();
    let (top, commit) = adopted(&tmp.path().join("p"), SETTINGS);
    assert!(describe_adoption_state(&top).contains("not yet projected"));

    refresh_adoption_projection(&top);
    let section = render_adoption_guidance_section(&top);
    assert!(section.contains(&format!("revision {commit}")), "{section}");
    assert!(section.contains("### review-norms\n\nReview every PR against its spec."));
    let state = describe_adoption_state(&top);
    assert!(
        state.contains(&format!("current at {commit}"))
            && state.contains("outside `aida agent` do not receive"),
        "{state}"
    );
    assert!(git(&top, &["status", "--porcelain"], None).is_empty());
    assert!(!top.join("CLAUDE.md").exists() && !top.join("AGENTS.md").exists());

    std::fs::write(top.join(GUIDANCE_DIR).join("review-norms.md"), "edited\n").unwrap();
    let section = render_adoption_guidance_section(&top);
    assert!(
        section.contains("No shared guidance was delivered"),
        "{section}"
    );
    assert!(!section.contains("### review-norms"));
    assert!(describe_adoption_state(&top).contains("damaged"));
}

#[test]
fn a_launch_outside_an_adopted_clone_adds_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("p")).unwrap();
    git(&tmp.path().join("p"), &["init", "-q"], None);
    refresh_adoption_projection(&tmp.path().join("p"));
    assert!(render_adoption_guidance_section(&tmp.path().join("p")).is_empty());
    assert_eq!(
        describe_adoption_state(&tmp.path().join("p")),
        "not adopted"
    );
    assert!(!tmp.path().join("p/.aida").exists());
}

#[test]
fn config_edits_and_scaffolding_refuse_in_an_adopted_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let (top, _) = adopted(&tmp.path().join("p"), SETTINGS);
    refresh_adoption_projection(&top);
    let config = top.join(CONFIG);
    let before = std::fs::read(&config).unwrap();

    let err =
        crate::config_edit::set_kv(&config, "agents", "vendor", toml_edit::Value::from("codex"))
            .unwrap_err();
    assert!(
        format!("{err:#}").contains("generated from the shared adoption plane"),
        "{err:#}"
    );
    assert_eq!(std::fs::read(&config).unwrap(), before);

    assert!(aida_core::scaffolding::refuse_in_adopted_worktree(&top).is_err());
    assert!(aida_core::scaffolding::codex_hooks::scaffold_codex_hooks(&top, true).is_err());
    assert!(aida_core::scaffolding::codex_hooks::scaffold_antigravity_hooks(&top, true).is_err());
    assert!(!top.join(".codex").exists() && !top.join(".agents").exists());
    assert!(git(&top, &["status", "--porcelain"], None).is_empty());
}

/// Point the plane at a new commit carrying `settings`.
fn republish(dir: &Path, settings: &str) -> String {
    let blob = git(dir, &["hash-object", "-w", "--stdin"], Some(settings));
    let sub = git(
        dir,
        &["mktree"],
        Some(&format!("100644 blob {blob}\tsettings.toml\n")),
    );
    let tree = git(
        dir,
        &["mktree"],
        Some(&format!("040000 tree {sub}\tadoption\n")),
    );
    let commit = git(dir, &["commit-tree", &tree, "-m", "plane"], None);
    git(dir, &["update-ref", "refs/heads/aida-store", &commit], None);
    commit
}

#[test]
fn a_failed_refresh_is_reported_and_delivers_nothing_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let (top, first) = adopted(&tmp.path().join("p"), SETTINGS);
    assert_eq!(refresh_adoption_projection(&top), None);
    let second = republish(
        &top,
        "schema = 1\n[guidance]\nreview-norms = \"Second revision.\\n\"\n",
    );
    std::fs::write(top.join(CONFIG), "edited = 1\n").unwrap();

    let warning = refresh_adoption_projection(&top).expect("the failure is reported");
    assert!(warning.contains("none is delivered"), "{warning}");
    let section = render_adoption_guidance_section(&top);
    assert!(
        section.contains("No shared guidance was delivered")
            && !section.contains(&format!("revision {first}"))
            && !section.contains(&format!("revision {second}"))
            && !section.contains("### review-norms"),
        "{section}"
    );
    assert!(describe_adoption_state(&top).contains("no shared guidance is delivered"));
}

#[test]
fn an_edited_projected_config_reads_the_same_in_preview_and_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let (top, _) = adopted(&tmp.path().join("p"), SETTINGS);
    refresh_adoption_projection(&top);
    std::fs::write(top.join(CONFIG), "edited = 1\n").unwrap();

    let preview = describe_adoption_state(&top);
    let section = render_adoption_guidance_section(&top);
    assert!(
        preview.contains("damaged (.aida/config.toml); no shared guidance is delivered"),
        "{preview}"
    );
    assert!(
        section.contains("No shared guidance was delivered")
            && section.contains(".aida/config.toml")
            && !section.contains("### review-norms"),
        "{section}"
    );
}

#[test]
fn a_tracked_product_config_skip_is_reported_in_preview_and_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let (top, commit) = adopted(&tmp.path().join("p"), SETTINGS);
    std::fs::create_dir_all(top.join(".aida")).unwrap();
    std::fs::write(top.join(CONFIG), "[agents]\nvendor = \"legacy\"\n").unwrap();
    git(&top, &["add", "-f", CONFIG], None);
    git(&top, &["commit", "-q", "-m", "legacy scaffolding"], None);

    let warning = refresh_adoption_projection(&top).expect("the skip is reported");
    assert!(warning.contains("tracks .aida/config.toml"), "{warning}");
    let section = render_adoption_guidance_section(&top);
    assert!(
        section.contains(&format!("revision {commit}"))
            && section.contains("settings are not applied here")
            && section.contains("### review-norms"),
        "{section}"
    );
    let preview = describe_adoption_state(&top);
    assert!(
        preview.contains(&format!("current at {commit}"))
            && preview.contains("tracks .aida/config.toml"),
        "{preview}"
    );
    assert_eq!(
        std::fs::read_to_string(top.join(CONFIG)).unwrap(),
        "[agents]\nvendor = \"legacy\"\n"
    );
}

/// An adopted worktree whose product branch tracks its own AGENTS.md and
/// .gitignore, projected; returns the bytes every scaffold writer must keep.
fn adopted_with_tracked_agents_md(dir: &Path) -> (PathBuf, Vec<(PathBuf, Vec<u8>)>) {
    let (top, _) = adopted(dir, SETTINGS);
    std::fs::write(top.join("AGENTS.md"), "# Product agents\n").unwrap();
    std::fs::write(top.join(".gitignore"), "target/\n").unwrap();
    git(&top, &["add", "AGENTS.md", ".gitignore"], None);
    git(&top, &["commit", "-q", "-m", "product guidance"], None);
    assert_eq!(refresh_adoption_projection(&top), None);
    let kept = ["AGENTS.md", ".gitignore", CONFIG]
        .iter()
        .map(|rel| (top.join(rel), std::fs::read(top.join(rel)).unwrap()))
        .collect();
    (top, kept)
}

fn assert_untouched(top: &Path, kept: &[(PathBuf, Vec<u8>)], path: &str) {
    assert_eq!(
        git(top, &["status", "--porcelain"], None),
        "",
        "{path} changed the product tree"
    );
    for (file, bytes) in kept {
        assert_eq!(
            &std::fs::read(file).unwrap(),
            bytes,
            "{path}: {}",
            file.display()
        );
    }
    for dir in [".claude", ".agents", ".codex", ".aida/discipline"] {
        assert!(!top.join(dir).exists(), "{path} created {dir}");
    }
}

fn assert_refused(path: &str, result: anyhow::Result<impl std::fmt::Debug>) {
    let err = result.expect_err(path);
    assert!(
        format!("{err:#}").contains("adopted from a shared AIDA plane"),
        "{path}: {err:#}"
    );
}

#[test]
fn every_scaffold_write_path_refuses_in_an_adopted_worktree() {
    use crate::cli::ScaffoldCommand;
    let tmp = tempfile::tempdir().unwrap();
    let (top, kept) = adopted_with_tracked_agents_md(&tmp.path().join("p"));
    let storage = crate::Storage::new(tmp.path().join("store.yaml"));
    let db = tmp.path().join("cache.db");

    let apply = ScaffoldCommand::Apply {
        project_root: Some(top.clone()),
        force: true,
        dry_run: false,
        prune: false,
    };
    assert_refused(
        "scaffold apply",
        crate::scaffold_cmd::handle_scaffold_command(&apply, &storage, &db),
    );
    assert_untouched(&top, &kept, "scaffold apply");

    let upgrade = ScaffoldCommand::Upgrade {
        project_root: Some(top.clone()),
        dry_run: false,
        force: true,
        prune: false,
        only: None,
    };
    assert_refused(
        "scaffold upgrade",
        crate::scaffold_cmd::handle_scaffold_command(&upgrade, &storage, &db),
    );
    assert_untouched(&top, &kept, "scaffold upgrade");

    // A caller that built its own preview is refused by the upgrade itself.
    let preview = aida_core::scaffolding::Scaffolder::new(
        top.clone(),
        aida_core::scaffolding::ScaffoldConfig::default(),
    )
    .preview(&aida_core::RequirementsStore::default());
    assert_refused(
        "run_scaffold_upgrade",
        crate::scaffold_cmd::run_scaffold_upgrade(&top, &preview, false, true, true),
    );
    assert_untouched(&top, &kept, "run_scaffold_upgrade");
    assert!(aida_core::scaffolding::Scaffolder::new(
        top.clone(),
        aida_core::scaffolding::ScaffoldConfig::default(),
    )
    .apply(&preview)
    .is_err());
    assert_untouched(&top, &kept, "Scaffolder::apply");

    // `scaffold refresh` and `init --refresh` share this entry.
    let refresh = ScaffoldCommand::Refresh {
        project_root: Some(top.clone()),
        dest: Some(tmp.path().join("prompts")),
    };
    assert_refused(
        "scaffold refresh",
        crate::scaffold_cmd::handle_scaffold_command(&refresh, &storage, &db),
    );
    assert_refused(
        "refresh_agent_packs",
        crate::scaffold_refresh::refresh_agent_packs_at(&top, None, None).map(|p| p.len()),
    );
    assert_untouched(&top, &kept, "scaffold refresh");

    assert_refused(
        "discipline pack",
        crate::ensure_discipline_pack_scaffold(&top, true).map(|r| r.written),
    );
    assert_refused(
        "discipline .gitignore",
        crate::ensure_discipline_pack_gitignore_allow_list(&top),
    );
    #[cfg(feature = "tui")]
    assert_refused(
        "agent profile enable",
        crate::config_cmd::scaffold_enabled_agent_profile(&top, "codex"),
    );
    assert_untouched(&top, &kept, "discipline and profile writers");

    // A writer rooted in a subdirectory is still inside the adopted worktree.
    std::fs::create_dir_all(top.join("src")).unwrap();
    assert!(aida_core::scaffolding::refuse_in_adopted_worktree(&top.join("src")).is_err());
}

#[test]
fn a_projected_config_refusal_names_where_local_keys_belong() {
    let tmp = tempfile::tempdir().unwrap();
    let (top, _) = adopted(&tmp.path().join("p"), SETTINGS);
    refresh_adoption_projection(&top);
    let err = crate::config_edit::set_kv(
        &top.join(CONFIG),
        "paths",
        "scratch",
        toml_edit::Value::from("scratch-dir"),
    )
    .unwrap_err();
    let err = format!("{err:#}");
    assert!(
        err.contains("adoption/settings.toml") && err.contains("~/.aida/config.toml"),
        "{err}"
    );
}

/// Plane settings that every project-config writer below would edit.
const WRITER_SETTINGS: &str = "schema = 1\n[settings.ui]\ntheme = \"ascii\"\n\
                               [settings.forge]\nprovider = \"gitlab\"\n\
                               [settings.id_format]\ncounter_scope = \"per_type\"\n\
                               [settings.hints]\nworkflow_hints = true\n\
                               [guidance]\nreview-norms = \"Review.\\n\"\n";

/// Every writer of the project `.aida/config.toml` refuses the projected
/// copy through the one guarded writer, and leaves it byte for byte.
#[test]
fn every_project_config_writer_refuses_the_projected_config() {
    let tmp = tempfile::tempdir().unwrap();
    let (top, _) = adopted(&tmp.path().join("p"), WRITER_SETTINGS);
    assert_eq!(refresh_adoption_projection(&top), None);
    let config = top.join(CONFIG);
    let before = std::fs::read(&config).unwrap();
    let refused = |writer: &str, result: anyhow::Result<()>| {
        let err = result.expect_err(writer);
        assert!(
            format!("{err:#}").contains("generated from the shared adoption plane"),
            "{writer}: {err:#}"
        );
        assert_eq!(std::fs::read(&config).unwrap(), before, "{writer} wrote");
        assert!(
            !describe_adoption_state(&top).contains("damaged"),
            "{writer} damaged the projection"
        );
    };

    refused(
        "config_edit::save_doc",
        crate::config_edit::set_kv(&config, "ui", "theme", toml_edit::Value::from("x")),
    );
    refused(
        "glyph_config::save_doc",
        crate::glyph_config::set_theme(&config, "unicode"),
    );
    refused(
        "config glyph theme unicode",
        crate::config_cmd::clear_theme_reference(&config),
    );
    refused(
        "doctor counter-scope migration",
        crate::doctor_cmd::update_config_counter_scope(&config, "global"),
    );
    refused(
        "forge provider",
        crate::forge::write_forge_config_provider(&top, crate::forge::ForgeKind::GitHub).map(drop),
    );
    refused(
        "workflow hints",
        crate::workflow_hints::persist_setting(&top, false).map(drop),
    );
    refused(
        "project capabilities",
        crate::project_capabilities::write_capabilities(
            &top,
            &crate::project_capabilities::classify(
                &top,
                None,
                &crate::project_capabilities::Probes {
                    cli_on_path: &|_| false,
                    gh_authed: &|_| None,
                },
            ),
        )
        .map(drop),
    );
    assert!(describe_adoption_state(&top).contains("current at"));
}

/// An adopted clone refuses scaffolding before its first launch has
/// projected anything, and the refusal writes nothing.
#[test]
fn scaffolding_refuses_in_an_adopted_worktree_before_its_first_projection() {
    use crate::cli::ScaffoldCommand;
    let tmp = tempfile::tempdir().unwrap();
    let (top, _) = adopted(&tmp.path().join("p"), SETTINGS);
    std::fs::write(top.join(".gitignore"), "target/\n").unwrap();
    git(&top, &["add", ".gitignore"], None);
    git(&top, &["commit", "-q", "-m", "ignore"], None);
    assert!(!aida_core::adoption_projection::is_projected(&top));

    assert!(aida_core::scaffolding::refuse_in_adopted_worktree(&top).is_err());
    let storage = crate::Storage::new(tmp.path().join("store.yaml"));
    let db = tmp.path().join("cache.db");
    let apply = ScaffoldCommand::Apply {
        project_root: Some(top.clone()),
        force: true,
        dry_run: false,
        prune: false,
    };
    assert_refused(
        "scaffold apply",
        crate::scaffold_cmd::handle_scaffold_command(&apply, &storage, &db),
    );
    assert_refused(
        "refresh_agent_packs",
        crate::scaffold_refresh::refresh_agent_packs_at(&top, None, None).map(|p| p.len()),
    );
    assert_refused(
        "discipline .gitignore",
        crate::ensure_discipline_pack_gitignore_allow_list(&top),
    );
    assert!(aida_core::scaffolding::codex_hooks::scaffold_codex_hooks(&top, true).is_err());
    assert!(git(&top, &["status", "--porcelain"], None).is_empty());
    assert!(!top.join("AGENTS.md").exists() && !top.join(".aida").exists());
}

/// A config an `aida config set` wrote before the first projection is
/// kept: the launch warns with both destinations and still delivers the
/// plane's guidance.
#[test]
fn a_local_config_written_before_the_first_projection_is_kept_and_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let (top, commit) = adopted(&tmp.path().join("p"), SETTINGS);
    let config = top.join(CONFIG);
    crate::config_edit::set_kv(&config, "paths", "scratch", toml_edit::Value::from("s")).unwrap();
    let local = std::fs::read(&config).unwrap();

    let warning = refresh_adoption_projection(&top).expect("the kept config is reported");
    assert!(
        warning.contains("adoption/settings.toml") && warning.contains("~/.aida/config.toml"),
        "{warning}"
    );
    let section = render_adoption_guidance_section(&top);
    assert!(
        section.contains(&format!("revision {commit}")) && section.contains("### review-norms"),
        "{section}"
    );
    assert!(describe_adoption_state(&top).contains(&format!("current at {commit}")));
    assert_eq!(std::fs::read(&config).unwrap(), local);
    assert_eq!(refresh_adoption_projection(&top), Some(warning));
}
