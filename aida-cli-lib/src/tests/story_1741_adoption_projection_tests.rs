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
        err.to_string()
            .contains("generated from the shared adoption plane"),
        "{err}"
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
