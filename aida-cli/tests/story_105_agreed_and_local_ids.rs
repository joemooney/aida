//! Candidate-binary coverage of STORY-HOME-105: a bound root clone writes the
//! agreed ID from its block AND the local ID in one object; a clone with no
//! usable block writes a local ID only and shows the agreed ID as pending; a
//! clone whose local setting says root while the store records another root
//! refuses before dispensing.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
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
        fs::create_dir_all(f.root.join(".aida")).unwrap();
        fs::write(
            f.root.join(".aida/config.toml"),
            "[store]\nmode='distributed'\nstore_path='.aida-store'\n",
        )
        .unwrap();
        fs::create_dir_all(f.store.join(".aida")).unwrap();
        // The store still carries an older clone's legacy node (like node 1
        // on the home store); the clone's work binding must win over it.
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

    fn aida_ok(&self, args: &[&str]) -> String {
        let out = self.aida(args);
        assert!(
            out.status.success(),
            "aida {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn objects(&self) -> Vec<aida_core::Requirement> {
        let objects = self.store.join("objects");
        aida_core::object_store::list_objects(&objects)
            .unwrap()
            .into_iter()
            .map(|(sid, _)| aida_core::object_store::read_object(&objects, &sid).unwrap())
            .collect()
    }

    fn by_title(&self, title: &str) -> aida_core::Requirement {
        self.objects()
            .into_iter()
            .find(|r| r.title == title)
            .unwrap_or_else(|| panic!("no object titled {title}"))
    }

    fn add(&self, title: &str) -> Output {
        self.aida(&[
            "add",
            "--type",
            "story",
            "--title",
            title,
            "--description",
            "fixture",
        ])
    }
}

// trace:STORY-B6B1278890774EF993D2EBC327862470-105.ac6be633 | ai:claude
// trace:STORY-B6B1278890774EF993D2EBC327862470-105.acdb02e1 | ai:claude
// trace:STORY-B6B1278890774EF993D2EBC327862470-105.ac5e9c08 | ai:claude
#[test]
fn story_105_root_writes_agreed_and_local_ids_in_one_object() {
    let f = Fixture::new();

    // A clone with no usable block: local ID only, agreed ID pending.
    let out = f.add("before root");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let first = f.by_title("before root");
    let binding = aida_core::work_binding::load(&f.store)
        .unwrap()
        .expect("the first add binds the clone");
    let node = binding.node_id.to_ascii_uppercase();
    let local = first.spec_id.clone().unwrap();
    assert!(local.to_ascii_uppercase().contains(&node), "{local}");
    assert_eq!(first.agreed_id, None);
    let shown = f.aida_ok(&["show", &local]);
    assert!(shown.contains("pending"), "{shown}");

    // Enroll as root and hold a STORY block for this clone's bound node.
    f.aida_ok(&["node", "enroll-root"]);
    let blocks = f.store.join("registry/blocks.yaml");
    fs::write(
        &blocks,
        format!(
            "blocks:\n- node_id: '{}'\n  owner: o\n  hostname: h\n  type_prefix: STORY\n  range_start: 5801\n  range_end: 5900\n  next: 5801\n  allocated_at: 2026-10-31T00:00:00Z\n",
            binding.node_id
        ),
    )
    .unwrap();

    let out = f.add("after root");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let second = f.by_title("after root");
    assert_eq!(second.agreed_id.as_deref(), Some("STORY-5801"), "{stdout}");
    let local = second.spec_id.clone().unwrap();
    assert!(
        local.to_ascii_uppercase().contains(&node),
        "the local ID is the bound node's, never the store's legacy node 7: {local}"
    );
    assert!(
        stdout.contains("STORY-5801 ("),
        "agreed first, local beside: {stdout}"
    );
    // Both IDs open the same spec.
    for id in ["STORY-5801", local.as_str()] {
        let shown = f.aida_ok(&["show", id]);
        assert!(shown.contains("after root"), "{id}: {shown}");
    }

    // The local setting says root but the store now records another node.
    fs::write(
        f.store.join("registry/root.yaml"),
        "holder_node: someone-else\nepoch: 2\n",
    )
    .unwrap();
    let out = f.add("refused");
    assert!(!out.status.success());
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(said.contains("records node someone-else"), "{said}");
    assert!(f.objects().iter().all(|r| r.title != "refused"));
}
