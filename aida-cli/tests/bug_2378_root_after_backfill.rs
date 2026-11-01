//! Candidate-binary coverage of BUG-2378, along the STORY-105 post-merge
//! runbook on a clone whose native node exists only as a label binding
//! (`registry/node_aliases.toml`), never a `registry/nodes.toml` row: after
//! `node enroll-root` and the agreed-ID backfill (which retires the legacy
//! node), `aida db block verify` is clean, `aida node show` names the root
//! node as this clone's identity, `.aida/node.toml` no longer names the
//! retired node, and `aida remote reconcile --execute` brings a second bare
//! hub still sitting before the backfill up to date.
// trace:BUG-2378 | ai:claude
mod support;
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
    hubs: [PathBuf; 2],
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
            hubs: [dir.path().join("origin.git"), dir.path().join("mirror.git")],
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
        for (name, hub) in ["origin", "mirror"].iter().zip(&f.hubs) {
            f.git(&["init", "-q", "--bare", hub.to_str().unwrap()]);
            f.git(&["remote", "add", name, hub.to_str().unwrap()]);
        }
        fs::create_dir_all(f.root.join(".aida")).unwrap();
        fs::write(
            f.root.join(".aida/config.toml"),
            "[store]\nmode='distributed'\nstore_path='.aida-store'\n",
        )
        .unwrap();
        fs::create_dir_all(f.store.join(".aida")).unwrap();
        // Like a real store, the per-clone identity and counter stay out of
        // the shared branch. Without this the pre-allocation sync of the
        // first `aida add` (an origin remote exists) commits them, the
        // backfill's identity re-point dirties the store worktree, and
        // `aida remote reconcile --execute` refuses the uncommitted change.
        // `registry/*.lock` is the dispense lock the backfill takes beside
        // `registry/blocks.yaml`; `aida doctor` writes it into a store's
        // runtime ignores (and `aida init` covers it with `*.lock`).
        fs::write(
            f.store.join(".gitignore"),
            ".aida/node.toml\n.aida/dispenser.toml\n.aida/*.lock\nregistry/*.lock\n",
        )
        .unwrap();
        // The store carries an older clone's legacy node, like node 1 on
        // the home store.
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
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().into()
    }

    fn store_git(&self, args: &[&str]) -> String {
        let mut full = vec!["-C", self.store.to_str().unwrap()];
        full.extend_from_slice(args);
        self.git(&full)
    }

    /// The store worktree has no uncommitted change, so a leak is reported at
    /// the step that leaks rather than as a later reconcile refusal.
    fn assert_store_clean(&self, step: &str) {
        let status = self.store_git(&["status", "--porcelain"]);
        assert!(status.is_empty(), "store dirty after {step}:\n{status}");
    }

    fn hub_tip(&self, i: usize) -> String {
        let hub = self.hubs[i].to_str().unwrap();
        self.git(&["--git-dir", hub, "rev-parse", "refs/heads/aida-store"])
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
}

#[test]
fn bug_2378_root_registered_by_label_verifies_identifies_and_syncs_after_backfill() {
    let f = Fixture::new();

    // The first add binds the clone to its native node.
    let out = f.aida(&[
        "add",
        "--type",
        "story",
        "--title",
        "before root",
        "--description",
        "fixture",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let node = aida_core::work_binding::load(&f.store)
        .unwrap()
        .expect("the first add binds the clone")
        .node_id;
    f.aida_ok(&["node", "enroll-root"]);

    // The native node is registered only by its label; the legacy node 7 is
    // a nodes.toml row and still names a spec.
    let registry = f.store.join("registry");
    fs::create_dir_all(&registry).unwrap();
    fs::write(
        registry.join("node_aliases.toml"),
        format!("[[binding]]\nlabel = \"HOME\"\nnode = \"{node}\"\n"),
    )
    .unwrap();
    fs::write(
        registry.join("nodes.toml"),
        "[[nodes]]\nid = \"7\"\nuser_id = 1\nhostname = \"\"\nregistered = \"2026-01-01T00:00:00Z\"\n",
    )
    .unwrap();
    let mut legacy = aida_core::Requirement::new("legacy node spec".into(), "fixture".into());
    legacy.spec_id = Some("STORY-7-001".into());
    legacy.req_type = aida_core::RequirementType::Story;
    aida_core::object_store::write_object(&f.store.join("objects"), &legacy).unwrap();
    f.store_git(&["add", "-A"]);
    f.store_git(&["commit", "-qm", "pre-backfill store"]);
    // Both hubs start at the pre-backfill store.
    f.store_git(&["push", "-q", "origin", "aida-store"]);
    f.store_git(&["push", "-q", "mirror", "aida-store"]);
    let stale = f.hub_tip(1);

    // The runbook: preview, then apply. The preview names the re-point.
    let dry = f.aida_ok(&["doctor", "--fix", "agreed-ids", "--dry-run"]);
    assert!(
        dry.contains(&format!(
            "This clone's .aida/node.toml identity: node 7 -> node {node}"
        )),
        "{dry}"
    );
    f.aida_ok(&["doctor", "--fix", "agreed-ids", "--yes"]);
    f.assert_store_clean("the backfill");
    let nodes = fs::read_to_string(registry.join("nodes.toml")).unwrap();
    assert!(nodes.contains("retired_at"), "{nodes}");
    assert!(
        !nodes.contains(&node),
        "the root never gets a nodes.toml row: {nodes}"
    );
    let config = aida_core::NodeConfig::load(&f.store.join(".aida/node.toml")).unwrap();
    assert_eq!(
        config.node_id, node,
        "the retirement re-points the identity"
    );
    let again = f.aida_ok(&["doctor", "--fix", "agreed-ids", "--dry-run"]);
    assert!(again.contains("nothing to backfill"), "{again}");

    // (3) A store an older binary already backfilled: renames, block claims
    // and the retirement mark are in, but `.aida/node.toml` still names the
    // retired node 7. The one-time repair previews the re-point, applies it,
    // and leaves nothing behind.
    fs::write(
        f.store.join(".aida/node.toml"),
        "node_id='7'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n",
    )
    .unwrap();
    let repair = f.aida_ok(&["doctor", "--fix", "agreed-ids", "--dry-run"]);
    assert!(
        repair.contains(&format!(
            "This clone's .aida/node.toml identity: node 7 -> node {node}"
        )),
        "{repair}"
    );
    assert!(!repair.contains("nothing to backfill"), "{repair}");
    let nodes_before = fs::read_to_string(registry.join("nodes.toml")).unwrap();
    f.aida_ok(&["doctor", "--fix", "agreed-ids", "--yes"]);
    f.assert_store_clean("the one-time repair");
    let config = aida_core::NodeConfig::load(&f.store.join(".aida/node.toml")).unwrap();
    assert_eq!(config.node_id, node, "the repair re-points the identity");
    assert_eq!(
        fs::read_to_string(registry.join("nodes.toml")).unwrap(),
        nodes_before,
        "the repair leaves the registry alone"
    );
    let again = f.aida_ok(&["doctor", "--fix", "agreed-ids", "--dry-run"]);
    assert!(again.contains("nothing to backfill"), "{again}");

    // (1) The root owns blocks and is registered by its label + root record.
    let verify = f.aida_ok(&["db", "block", "verify"]);
    assert!(!verify.contains("not registered"), "{verify}");
    assert!(!verify.contains(&format!("node `{node}` owns")), "{verify}");
    // The retired node 7 owns no block by design; under blocks-only it is
    // still not a "registered node with no claimed block".
    let config_path = f.root.join(".aida/config.toml");
    let config_before = fs::read_to_string(&config_path).unwrap();
    fs::write(
        &config_path,
        format!("{config_before}[id_format]\npolicy='blocks-only'\n"),
    )
    .unwrap();
    let strict = f.aida_ok(&["db", "block", "verify"]);
    assert!(strict.contains("blocks-only"), "{strict}");
    assert!(!strict.contains("with no claimed block"), "{strict}");
    fs::write(&config_path, config_before).unwrap();

    // (2) This clone's identity is the root node, never the retired node 7.
    let shown = f.aida_ok(&["node", "show"]);
    assert!(shown.contains(&format!("Node {node}")), "{shown}");
    assert!(shown.contains("Label: HOME"), "{shown}");
    assert!(shown.contains("Active on this clone: yes"), "{shown}");
    assert!(!shown.contains("Node 7"), "{shown}");
    assert!(!shown.contains("not in the shared registry"), "{shown}");
    let retired = f.aida_ok(&["node", "show", "7"]);
    assert!(retired.contains("Retired:    yes"), "{retired}");
    assert!(!retired.contains("Active on this clone"), "{retired}");

    // (1) One hub has the backfill, the other is still before it: the
    // reconcile pushes the union to both.
    f.store_git(&["push", "-q", "origin", "aida-store"]);
    assert_eq!(f.hub_tip(1), stale);
    let audit = f.aida_ok(&[
        "db",
        "audit-ids",
        "--against",
        "origin/aida-store",
        "--against",
        &stale,
    ]);
    assert!(audit.contains("clean"), "{audit}");
    f.assert_store_clean("verify, node show and audit-ids");
    let out = f.aida(&["remote", "reconcile", "--execute", "--yes"]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{said}");
    assert!(!said.contains("allocation claims check refused"), "{said}");
    let head = f.store_git(&["rev-parse", "HEAD"]);
    assert_eq!(f.hub_tip(0), head, "{said}");
    assert_eq!(f.hub_tip(1), head, "{said}");
}
