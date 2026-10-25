//! Candidate-binary coverage of clone-local allocation, without real HOME writes.
// trace:BUG-2017 | ai:codex
#[path = "support/test_git.rs"]
mod test_git;
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
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
        fs::write(
            f.store.join(".aida/node.toml"),
            "node_id='7'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n",
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
    fn objects(&self) -> Vec<aida_core::Requirement> {
        aida_core::object_store::list_objects(&self.store.join("objects"))
            .unwrap()
            .into_iter()
            .map(|(sid, _)| {
                aida_core::object_store::read_object(&self.store.join("objects"), &sid).unwrap()
            })
            .collect()
    }
}
#[test]
fn bug_2017_cli_local_and_global_refusals() {
    let f = Fixture::new();
    // Simulate a cached/imported global block: WORK must ignore it, and an
    // explicit claim refusal must not advance or rewrite its consumption.
    let blocks_path = f.store.join("registry/blocks.yaml");
    fs::create_dir_all(blocks_path.parent().unwrap()).unwrap();
    let mut blocks = aida_core::BlockRegistry::default();
    blocks.blocks.push(aida_core::AgreedIdBlock {
        node_id: "7".into(),
        owner: "fixture".into(),
        hostname: "fixture".into(),
        type_prefix: "BUG".into(),
        range_start: 500,
        range_end: 599,
        next: 500,
        allocated_at: chrono::Utc::now(),
    });
    let incoming = f._dir.path().join("incoming-blocks.yaml");
    blocks.save(&incoming).unwrap();
    let cached = fs::read_to_string(incoming).unwrap();
    fs::write(&blocks_path, &cached).unwrap();
    f.git(&[
        "-C",
        f.store.to_str().unwrap(),
        "add",
        "registry/blocks.yaml",
    ]);
    f.git(&[
        "-C",
        f.store.to_str().unwrap(),
        "commit",
        "-qm",
        "imported cached block",
    ]);
    let status = f.aida(&["node", "team", "--format", "json"]);
    assert!(
        status.status.success(),
        "{}",
        format!(
            "{}{}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        )
    );
    let json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(json["allocation"]["authoritative"], false);
    let add = f.aida(&["add", "--title", "generic fixture defect", "--type", "bug"]);
    assert!(
        add.status.success(),
        "{}{}",
        String::from_utf8_lossy(&add.stdout),
        String::from_utf8_lossy(&add.stderr)
    );
    let reqs = f.objects();
    assert_eq!(reqs.len(), 1);
    // trace:BUG-2017 | ai:codex — registered legacy CLI state migrates once.
    let binding = aida_core::work_binding::load(&f.store).unwrap().unwrap();
    assert_eq!(binding.node_id.len(), 32);
    assert_eq!(
        reqs[0].spec_id.as_deref(),
        Some(format!("BUG-{}-001", binding.node_id).as_str())
    );
    assert!(reqs[0].agreed_id.is_none());
    let counter_path = aida_core::work_binding::counter_path(&f.store).unwrap();
    let counter = fs::read(&counter_path).unwrap();
    let head = f.git(&["rev-parse", "aida-store"]);
    let block = f.aida(&["db", "block", "claim", "--type", "BUG", "--size", "10"]);
    assert!(!block.status.success());
    assert_eq!(fs::read(&counter_path).unwrap(), counter);
    assert_eq!(f.git(&["rev-parse", "aida-store"]), head);
    assert_eq!(fs::read_to_string(blocks_path).unwrap(), cached);
}
#[test]
fn bug_2017_cli_blocks_only_refuses_before_counter_or_object_write() {
    let f = Fixture::new();
    fs::write(f.root.join(".aida/config.toml"), "[store]\nmode='distributed'\nstore_path='.aida-store'\n[id_format]\npolicy='blocks-only'\n").unwrap();
    let out = f.aida(&["add", "--title", "must refuse", "--type", "bug"]);
    assert!(!out.status.success());
    assert!(f.objects().is_empty());
    assert!(!f.store.join(".aida/dispenser.toml").exists());
    assert!(aida_core::work_binding::load(&f.store).unwrap().is_none());
    assert!(!aida_core::work_binding::counter_path(&f.store)
        .unwrap()
        .exists());
}
#[test]
fn bug_2017_mcp_uses_same_node_local_writer() {
    let f = Fixture::new();
    let mut child = f
        .command(false, &["mcp-serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for request in [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"add_requirement","arguments":{"title":"generic MCP fixture defect","description":"fixture","type":"bug","status":"draft"}}}),
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"add_requirement","arguments":{"title":"MCP generic requirement","description":"fixture","type":"functional","status":"draft"}}}),
    ] {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "MCP fixture timed out: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reqs = f.objects();
    assert_eq!(reqs.len(), 2, "{}", String::from_utf8_lossy(&out.stdout));
    let binding = aida_core::work_binding::load(&f.store).unwrap().unwrap();
    assert_eq!(binding.node_id.len(), 32);
    let ids: std::collections::HashSet<_> =
        reqs.iter().map(|r| r.spec_id.clone().unwrap()).collect();
    assert_eq!(
        ids,
        std::collections::HashSet::from([
            format!("BUG-{}-001", binding.node_id),
            format!("FR-{}-001", binding.node_id),
        ])
    );
    assert!(reqs.iter().all(|r| r.agreed_id.is_none()));
}

#[test]
fn bug_2017_authority_is_shared_only_by_linked_worktrees_not_ordinary_clones() {
    let f = Fixture::new();
    fs::write(
        f.root.join(".git/aida-allocation.toml"),
        "version=1\nauthoritative=true\n",
    )
    .unwrap();
    let linked = f._dir.path().join("linked");
    f.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        linked.to_str().unwrap(),
        "HEAD",
    ]);
    assert!(
        aida_core::allocation_authority::resolve(&linked)
            .unwrap()
            .authoritative
    );
    let cloned = f._dir.path().join("ordinary");
    f.git(&[
        "clone",
        "-q",
        "--no-local",
        f.root.to_str().unwrap(),
        cloned.to_str().unwrap(),
    ]);
    assert!(
        !aida_core::allocation_authority::resolve(&cloned)
            .unwrap()
            .authoritative
    );
}
