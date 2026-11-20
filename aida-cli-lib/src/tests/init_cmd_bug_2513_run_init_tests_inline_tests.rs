use super::*;
use clap::CommandFactory;

fn invoke(agent: &str) -> Result<()> {
    let matches = InitArgs::command()
        .try_get_matches_from(["init", "--agent", agent])
        .unwrap();
    run_init(
        &matches,
        &crate::cmdkit::DispatchCtx {
            globals: None,
            store: None,
        },
    )
}

fn git(root: &std::path::Path, args: &[&str]) {
    let output = crate::test_git::git_command()
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

// Exercise the outer dispatcher, not just distributed_init_footprint.
// An invalid local agent is a tripwire for reaching vendor resolution.
// trace:BUG-2513 | ai:codex
#[test]
fn run_init_adoption_precedes_local_choices() {
    if let Some(output) = crate::test_env::run_path_isolated_test_output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("Setup style:"), "{stdout}");
        assert!(!stdout.contains("Scaffold agent support for:"), "{stdout}");
        return;
    }
    let previous = std::env::current_dir().unwrap();
    // Plane discovery deliberately excludes /tmp; use a real project root.
    let tmp = tempfile::tempdir_in(&previous).unwrap();
    let root = tmp.path();
    git(root, &["init"]);
    git(root, &["commit", "--allow-empty", "-m", "product"]);
    git(root, &["checkout", "--orphan", "aida-store"]);
    std::fs::create_dir_all(root.join("adoption")).unwrap();
    // Adoption evidence is sufficient for routing even with invalid settings.
    std::fs::write(root.join("adoption/settings.toml"), "invalid [").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "plane"]);
    git(root, &["checkout", "main"]);
    std::env::set_current_dir(root).unwrap();
    for projected in [false, true] {
        if projected {
            let manifest = root.join(aida_core::adoption_projection::MANIFEST);
            std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
            std::fs::write(manifest, "").unwrap();
        }
        assert!(adopted_init_route(root).unwrap());
        let error = format!("{:#}", invoke("invalid-local-agent").unwrap_err());
        assert!(!error.contains("Invalid --agent"), "{error}");
        assert!(!root.join(".aida/config.toml").exists());
        assert!(!root.join("AGENTS.md").exists());
    }
    std::env::set_current_dir(previous).unwrap();
}

// A parent footprint must not suppress full setup (and its vendor choice)
// in a fresh nested repository. The invalid agent proves run_init reached
// the child's full-setup selection before any storage writes.
// trace:BUG-2513 | ai:codex
#[test]
fn run_init_nested_repo_ignores_parent_choices() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let previous = std::env::current_dir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let child = tmp.path().join("child");
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::create_dir_all(&child).unwrap();
    git(tmp.path(), &["init"]);
    git(&child, &["init"]);
    std::env::set_current_dir(&child).unwrap();
    for footprint in ["minimal", "memory-lane"] {
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            format!("[scaffold]\nfootprint = '{footprint}'\n[agents]\nvendor = 'codex'\nenabled = ['claude']\n"),
        ).unwrap();
        let error = format!("{:#}", invoke("invalid-local-agent").unwrap_err());
        assert!(error.contains("Invalid --agent"), "{error}");
        assert!(!child.join(".aida/config.toml").exists());
    }
    std::env::set_current_dir(previous).unwrap();
}
