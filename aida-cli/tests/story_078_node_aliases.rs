#![cfg(target_os = "linux")]
//! Black-box coverage for friendly node aliases: `aida node alias` management,
//! alias input resolving to the canonical requirement, the JSON `display_id`
//! contract, `--canonical-ids`, and the stored object never carrying a label.
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
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
    /// A non-authoritative clone: `aida add` files node-local IDs.
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
        fs::create_dir_all(f.root.join(".aida")).unwrap();
        fs::write(
            f.root.join(".aida/config.toml"),
            "[store]\nmode='distributed'\nstore_path='.aida-store'\n",
        )
        .unwrap();
        fs::create_dir_all(f.store.join(".aida")).unwrap();
        fs::write(
            f.store.join(".aida/node.toml"),
            "node_id='7'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n",
        )
        .unwrap();
        fs::write(
            f.store.join(".aida/dispenser.toml"),
            "[mode.Distributed]\nnode_id='7'\n[sequences]\n",
        )
        .unwrap();
        f
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
            "aida {args:?}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn show_json(&self, args: &[&str]) -> serde_json::Value {
        let mut full = vec!["show"];
        full.extend_from_slice(args);
        full.extend_from_slice(&["--format", "json", "--no-git"]);
        serde_json::from_str(&self.ok(&full)).unwrap()
    }
}

#[test]
fn node_alias_set_resolve_display_and_rename() {
    let f = Fixture::new();
    f.ok(&["add", "--title", "aliased defect", "--type", "bug"]);
    let node = aida_core::work_binding::load(&f.store)
        .unwrap()
        .unwrap()
        .node_id;
    let canonical = format!("BUG-{node}-001").to_ascii_uppercase();
    let object_path = f.store.join(format!("objects/BUG/000/{canonical}.yaml"));
    let object_before = fs::read_to_string(&object_path).unwrap();

    // No label yet: display is the canonical form, upper-cased.
    let json = f.show_json(&[&canonical]);
    assert_eq!(json["display_id"], canonical.as_str());

    // Set (defaults to this clone's node); no TTY needed.
    f.ok(&["node", "alias", "set", "home"]);
    let list: serde_json::Value =
        serde_json::from_str(&f.ok(&["node", "alias", "list", "--json"])).unwrap();
    assert_eq!(list[0]["label"], "HOME");
    assert_eq!(list[0]["node"], node.to_ascii_lowercase().as_str());
    assert!(f
        .ok(&["node", "alias", "show", "HOME"])
        .contains(&node.to_ascii_lowercase()));

    // A label is never pointed at another node; refusal names both.
    let other = "0123456789abcdef0123456789abcdef";
    let refused = f.aida(&["node", "alias", "set", "HOME", "--node", other]);
    assert!(!refused.status.success());
    let err = String::from_utf8_lossy(&refused.stderr).into_owned()
        + &String::from_utf8_lossy(&refused.stdout);
    assert!(
        err.contains(&node.to_ascii_lowercase()) && err.contains(other),
        "{err}"
    );

    // Input: the alias form, any case, resolves to the canonical requirement;
    // JSON keeps the canonical spec_id and adds the friendly display_id.
    for input in ["BUG-HOME-001", "bug-home-001"] {
        let json = f.show_json(&[input]);
        assert!(json["spec_id"]
            .as_str()
            .unwrap()
            .eq_ignore_ascii_case(&canonical));
        assert_eq!(json["display_id"], "BUG-HOME-001");
    }
    let human = f.ok(&["show", "bug-home-001", "--no-git", "--format", "human"]);
    assert!(human.contains("BUG-HOME-001"), "{human}");

    // Scripts can turn the substitution off.
    let json = f.show_json(&["BUG-HOME-001", "--canonical-ids"]);
    assert_eq!(json["display_id"], canonical.as_str());
    let out = f
        .command(
            false,
            &["show", "BUG-HOME-001", "--format", "json", "--no-git"],
        )
        .env("AIDA_CANONICAL_IDS", "1")
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["display_id"], canonical.as_str());

    // An unknown label is a not-found that names the label, never a guess.
    let missing = f.aida(&["show", "BUG-NOPE-001", "--no-git"]);
    assert!(!missing.status.success());
    let err = String::from_utf8_lossy(&missing.stderr).into_owned()
        + &String::from_utf8_lossy(&missing.stdout);
    assert!(err.contains("BUG-NOPE-001"), "{err}");

    // Rename retires HOME: it still resolves, display uses the new label.
    f.ok(&["node", "alias", "rename", "HOME", "DESK"]);
    let json = f.show_json(&["BUG-HOME-001"]);
    assert_eq!(json["display_id"], "BUG-DESK-001");
    let list = f.ok(&["node", "alias", "list"]);
    assert!(list.contains("HOME") && list.contains("retired"), "{list}");
    let refused = f.aida(&["node", "alias", "set", "HOME", "--node", other]);
    assert!(!refused.status.success());

    // Persistence: the stored object never carries a label.
    assert_eq!(fs::read_to_string(&object_path).unwrap(), object_before);
}
