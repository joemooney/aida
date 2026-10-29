//! Candidate-binary coverage of hierarchical block delegation, slice 1:
//! root enrollment, the declared upstream, `aida node show`, and the
//! once-per-session downgrade notice. Temporary repositories only.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
#[path = "support/test_git.rs"]
mod test_git;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    store: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        let home = dir.path().join("home");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&home).unwrap();
        let f = Self {
            store: root.join(".aida-store"),
            root,
            home,
            _dir: dir,
        };
        f.git(&["init", "-q", "-b", "main"]);
        f.git(&["commit", "--allow-empty", "-qm", "project"]);
        f.git(&["checkout", "-q", "--orphan", "aida-store"]);
        f.git(&["commit", "--allow-empty", "-qm", "store"]);
        f.git(&["checkout", "-q", "main"]);
        f.git(&[
            "worktree",
            "add",
            "-q",
            f.store.to_str().unwrap(),
            "aida-store",
        ]);
        f.config("");
        fs::create_dir_all(f.store.join(".aida")).unwrap();
        fs::write(
            f.store.join(".aida/node.toml"),
            "node_id='7'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n",
        )
        .unwrap();
        // A complete legacy node/provider pair, as a registered clone has.
        fs::write(
            f.store.join(".aida/dispenser.toml"),
            "[mode.Distributed]\nnode_id='7'\n[sequences]\n",
        )
        .unwrap();
        f
    }
    fn config(&self, extra: &str) {
        fs::create_dir_all(self.root.join(".aida")).unwrap();
        fs::write(
            self.root.join(".aida/config.toml"),
            format!("[store]\nmode='distributed'\nstore_path='.aida-store'\n{extra}"),
        )
        .unwrap();
    }
    fn command(&self, git: bool, args: &[&str]) -> Command {
        let mut cmd = if git {
            test_git::git_command()
        } else {
            Command::new(env!("CARGO_BIN_EXE_aida"))
        };
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("AIDA_")
                || key.to_string_lossy().starts_with("GIT_")
            {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(&self.root)
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_SESSION_ID", "fixture-session")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join("gitconfig"))
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid");
        cmd
    }
    fn git(&self, args: &[&str]) -> String {
        let out = self.command(true, args).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().into()
    }
    fn aida(&self, args: &[&str]) -> Output {
        self.command(false, args).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let out = self.aida(args);
        assert!(
            out.status.success(),
            "aida {args:?}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
    fn refused(&self, args: &[&str]) -> String {
        let out = self.aida(args);
        assert!(!out.status.success(), "aida {args:?} should refuse");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }
    fn setting(&self) -> Option<String> {
        fs::read_to_string(self.root.join(".git/aida-allocation.toml")).ok()
    }
}

#[test]
fn upstream_is_one_declared_non_mirror_remote() {
    let f = Fixture::new();
    assert!(f.ok(&["node", "upstream"]).contains("none"));
    let err = f.refused(&["node", "upstream", "gitlab"]);
    assert!(err.contains("not a git remote"), "{err}");
    assert!(f.setting().is_none());

    f.git(&["remote", "add", "gitlab", "https://example.invalid/hub.git"]);
    f.git(&[
        "remote",
        "add",
        "mirror",
        "https://example.invalid/mirror.git",
    ]);
    f.config("[store.sync]\nmirror_remotes=['mirror']\n");
    let err = f.refused(&["node", "upstream", "mirror"]);
    assert!(err.contains("store mirror"), "{err}");
    assert!(f.setting().is_none());

    f.ok(&["node", "upstream", "gitlab"]);
    let setting = f.setting().unwrap();
    assert!(setting.contains("role = \"node\"") && setting.contains("upstream = \"gitlab\""));
    assert_eq!(f.ok(&["node", "upstream"]).trim(), "gitlab");
    let show = f.ok(&["node", "show"]);
    assert!(show.contains("Upstream:  gitlab"), "{show}");
    assert!(show.contains("Role:      node"), "{show}");

    f.ok(&["node", "upstream", "--clear"]);
    assert!(!f.setting().unwrap().contains("upstream"));
}

#[test]
fn enroll_root_is_dry_run_first_single_and_refuses_an_upstream() {
    let f = Fixture::new();
    let head = f.git(&["rev-parse", "aida-store"]);
    let dry = f.ok(&["node", "enroll-root", "--dry-run"]);
    assert!(dry.contains("Nothing written"), "{dry}");
    assert_eq!(f.git(&["rev-parse", "aida-store"]), head);
    assert!(f.setting().is_none());
    assert!(!f.store.join("registry/root.yaml").exists());

    f.ok(&["node", "enroll-root"]);
    let root = fs::read_to_string(f.store.join("registry/root.yaml")).unwrap();
    assert!(
        root.contains("holder_node: '7'") || root.contains("holder_node: \"7\""),
        "{root}"
    );
    assert!(root.contains("epoch: 1"), "{root}");
    assert!(f.setting().unwrap().contains("role = \"root\""));
    assert_ne!(f.git(&["rev-parse", "aida-store"]), head);
    // Idempotent for the holder: no second commit.
    let enrolled = f.git(&["rev-parse", "aida-store"]);
    f.ok(&["node", "enroll-root"]);
    assert_eq!(f.git(&["rev-parse", "aida-store"]), enrolled);

    f.git(&["remote", "add", "gitlab", "https://example.invalid/hub.git"]);
    let err = f.refused(&["node", "upstream", "gitlab"]);
    assert!(err.contains("root has no upstream"), "{err}");
    let show = f.ok(&["node", "show"]);
    assert!(show.contains("Role:      root"), "{show}");
    assert!(show.contains("Root:      node 7 (epoch 1)"), "{show}");

    // Another clone's node cannot enroll while this one holds the role.
    fs::write(
        f.store.join("registry/root.yaml"),
        "holder_node: '9'\nepoch: 1\n",
    )
    .unwrap();
    let err = f.refused(&["node", "enroll-root"]);
    assert!(err.contains("already the allocation root"), "{err}");
}

#[test]
fn add_says_once_per_session_why_blocks_are_unusable() {
    let f = Fixture::new();
    let first = f.aida(&["add", "--title", "generic fixture defect", "--type", "bug"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stderr = String::from_utf8_lossy(&first.stderr);
    assert!(
        stderr.contains("Issuing a node-local ID: no upstream is declared"),
        "{stderr}"
    );
    let second = f.aida(&["add", "--title", "second fixture defect", "--type", "bug"]);
    assert!(second.status.success());
    assert!(!String::from_utf8_lossy(&second.stderr).contains("Issuing a node-local ID"));
    // A changed reason is news again.
    f.git(&["remote", "add", "gitlab", "https://example.invalid/hub.git"]);
    f.ok(&["node", "upstream", "gitlab"]);
    let third = f.aida(&["add", "--title", "third fixture defect", "--type", "bug"]);
    assert!(third.status.success());
    let stderr = String::from_utf8_lossy(&third.stderr);
    assert!(stderr.contains("holds no BUG block"), "{stderr}");
}
