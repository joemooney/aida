#![cfg(target_os = "linux")]
//! Black-box adoption lifecycle through the real binary: clone a project
//! whose aida-store branch carries an adoption plane, run `aida init`, and
//! the clone is set up from the plane without a single product-file write;
//! then status, the dry-run inventory, uninstall and the CI bootstrap.
// trace:STORY-1741 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn aida(home: &Path, dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(dir)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", "human")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env_remove("AIDA_STORE")
        .env_remove("REQ_DB_NAME")
        .env_remove("AIDA_REGISTRY_PATH")
        .env_remove("REQ_REGISTRY_PATH")
        .env_remove("AIDA_AGENT_OUTPUT")
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::null())
        .args(args)
        .output()
        .expect("run aida")
}

fn text(out: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const SETTINGS: &str = "schema = 1\n\n[settings.agents]\nvendor = \"claude\"\n\n\
                        [guidance]\nreview-norms = \"Review every PR against its spec.\\n\"\n";

/// An origin whose `main` holds only product code and whose orphan
/// `aida-store` branch holds a store and the adoption plane, cloned.
/// Returns `(home, clone, plane commit)`.
fn cloned_adopted_project(base: &Path) -> (PathBuf, PathBuf, String) {
    let home = base.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let seed = base.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    std::fs::write(seed.join("README.md"), "product\n").unwrap();
    git(&seed, &["add", "README.md"]);
    git(&seed, &["commit", "-q", "-m", "product"]);
    git(&seed, &["checkout", "-q", "--orphan", "aida-store"]);
    git(&seed, &["rm", "-rfq", "--cached", "."]);
    std::fs::remove_file(seed.join("README.md")).unwrap();
    std::fs::create_dir_all(seed.join("objects")).unwrap();
    std::fs::write(seed.join("objects/.gitkeep"), "").unwrap();
    std::fs::create_dir_all(seed.join("adoption")).unwrap();
    std::fs::write(seed.join("adoption/settings.toml"), SETTINGS).unwrap();
    git(&seed, &["add", "objects", "adoption"]);
    git(&seed, &["commit", "-q", "-m", "plane"]);
    let plane = git(&seed, &["rev-parse", "HEAD"]);
    git(&seed, &["checkout", "-q", "main"]);
    let origin = base.join("origin.git");
    git(
        base,
        &[
            "clone",
            "-q",
            "--bare",
            seed.to_str().unwrap(),
            "origin.git",
        ],
    );
    git(base, &["clone", "-q", origin.to_str().unwrap(), "clone"]);
    let clone = base.join("clone").canonicalize().unwrap();
    test_git::enroll_synthetic_home(&clone);
    (home, clone, plane)
}

/// The product side adoption must never change: tracked bytes, the index,
/// `main`, the remotes and the status.
fn product_snapshot(top: &Path) -> String {
    let tracked = git(top, &["ls-files", "-s"]);
    let bytes: Vec<String> = tracked
        .lines()
        .filter_map(|l| l.split('\t').nth(1))
        .map(|p| format!("{p}:{:?}", std::fs::read(top.join(p)).ok()))
        .collect();
    format!(
        "{tracked}\n{}\n{}\n{}\n{}",
        git(top, &["rev-parse", "refs/heads/main"]),
        git(top, &["remote", "-v"]),
        git(top, &["status", "--porcelain", "--untracked-files=all"]),
        bytes.join("\n")
    )
}

#[test]
fn init_in_a_fresh_clone_projects_the_plane_and_writes_no_product_file() {
    // trace:STORY-1741.ac752fd0 | ai:claude
    // trace:STORY-1741.ac6b1be3 | ai:claude
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (home, clone, plane) = cloned_adopted_project(&base);
    // Pre-dirty: the tree stays exactly this dirty.
    std::fs::write(clone.join("README.md"), "local edit\n").unwrap();
    let before = product_snapshot(&clone);

    let out = aida(&home, &clone, &["init"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(product_snapshot(&clone), before, "{}", text(&out));
    assert!(!clone.join(".gitignore").exists());
    assert!(!clone.join("CLAUDE.md").exists() && !clone.join("AGENTS.md").exists());
    assert!(clone.join(".aida-store/objects").is_dir(), "{}", text(&out));

    // The config init leaves is the projection, so the plane's settings
    // apply: no kept local config, no warning on later launches. Node setup
    // commits to the store branch, so the plane moved past the seed commit.
    let seeded = plane;
    let plane = git(&clone, &["rev-parse", "refs/heads/aida-store"]);
    assert_eq!(
        git(&clone, &["merge-base", "--is-ancestor", &seeded, &plane]),
        ""
    );
    let config = std::fs::read_to_string(clone.join(".aida/config.toml")).unwrap();
    assert!(config.contains(&plane), "{config}");
    assert!(config.contains("vendor = \"claude\""), "{config}");
    let status = aida(&home, &clone, &["adoption", "status"]);
    let said = String::from_utf8_lossy(&status.stdout);
    assert!(
        said.contains(&format!(
            "Current: the projection holds plane revision {plane}"
        )),
        "{}",
        text(&status)
    );
    assert!(!said.contains("Note:"), "{}", text(&status));
}

#[test]
fn inventory_uninstall_and_ci_bootstrap_leave_the_product_untouched() {
    // trace:STORY-1741.ac676d62 | ai:claude
    // trace:STORY-1741.ac7e5256 | ai:claude
    // trace:STORY-1741.ac591cd7 | ai:claude
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (home, clone, plane) = cloned_adopted_project(&base);
    let before = product_snapshot(&clone);

    let inventory = aida(&home, &clone, &["adoption", "inventory"]);
    assert!(inventory.status.success(), "{}", text(&inventory));
    assert!(
        String::from_utf8_lossy(&inventory.stdout).contains("No AIDA files are tracked"),
        "{}",
        text(&inventory)
    );
    assert_eq!(product_snapshot(&clone), before);

    // CI: a revision that is not the plane's refuses before writing.
    let wrong = aida(
        &home,
        &clone,
        &["adoption", "bootstrap", "--revision", &"0".repeat(40)],
    );
    assert!(!wrong.status.success(), "{}", text(&wrong));
    assert!(!clone.join(".aida/adoption/manifest.toml").exists());
    let ok = aida(
        &home,
        &clone,
        &["adoption", "bootstrap", "--revision", &plane],
    );
    assert!(ok.status.success(), "{}", text(&ok));
    assert!(String::from_utf8_lossy(&ok.stdout).contains(&format!("revision {plane}")));
    assert!(clone.join(".aida/config.toml").exists());

    let dry = aida(&home, &clone, &["adoption", "uninstall", "--dry-run"]);
    assert!(dry.status.success(), "{}", text(&dry));
    assert!(
        clone.join(".aida/config.toml").exists(),
        "a dry run removes nothing"
    );
    let gone = aida(&home, &clone, &["adoption", "uninstall"]);
    assert!(gone.status.success(), "{}", text(&gone));
    assert!(!clone.join(".aida/config.toml").exists(), "{}", text(&gone));
    assert!(!clone.join(".aida/adoption").exists(), "{}", text(&gone));
    assert_eq!(product_snapshot(&clone), before);
    assert_eq!(
        git(&clone, &["rev-parse", "refs/remotes/origin/aida-store"]),
        plane
    );

    let status = aida(&home, &clone, &["adoption", "status"]);
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("not yet projected"),
        "{}",
        text(&status)
    );
}

#[test]
fn init_refuses_and_writes_nothing_when_the_plane_cannot_be_read() {
    // trace:STORY-1741.ac1ebcce | ai:claude
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (home, clone, _) = cloned_adopted_project(&base);
    let locator = clone.join(".git/aida-adoption-locator.toml");
    std::fs::write(&locator, "not a locator").unwrap();
    std::fs::set_permissions(&locator, std::fs::Permissions::from_mode(0o600)).unwrap();
    let before = product_snapshot(&clone);

    let out = aida(&home, &clone, &["init"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("nothing was written"),
        "{}",
        text(&out)
    );
    assert_eq!(product_snapshot(&clone), before);
    assert!(!clone.join(".aida").exists() && !clone.join(".aida-store").exists());
}

#[test]
fn product_writing_init_modes_refuse_through_the_cli_in_an_adopted_clone() {
    // trace:STORY-1741.ac752fd0 | ai:claude
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (home, clone, _) = cloned_adopted_project(&base);
    let before = product_snapshot(&clone);

    for flag in ["--minimal", "--centralized"] {
        let out = aida(&home, &clone, &["init", flag]);
        assert!(!out.status.success(), "{flag}: {}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(&format!("aida init {flag}")),
            "{flag}: {}",
            text(&out)
        );
        assert_eq!(product_snapshot(&clone), before, "{flag}: {}", text(&out));
        assert!(!clone.join(".aida").exists(), "{flag}: {}", text(&out));
    }
}
