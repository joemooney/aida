//! Candidate-binary coverage of single-hub allocation: with one declared
//! hub and no enrolled root, any clone that reaches the hub claims blocks by
//! CAS push and mints agreed IDs; a claim not on the hub's ref is never
//! spendable; a second hub without a root refuses claims (held blocks keep
//! dispensing); enroll-root promotes the implicit pools without renumbering.
//! Every clone files its binding spec first, as a real clone does, so no
//! test hides the agreed-ID backfill gate. Temporary repositories and local
//! bare repositories only.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
#[path = "support/test_git.rs"]
mod test_git;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct World {
    dir: tempfile::TempDir,
    hub: PathBuf,
}

struct Clone {
    root: PathBuf,
    home: PathBuf,
    store: PathBuf,
}

fn command(cwd: &Path, home: &Path, git: bool, args: &[&str]) -> Command {
    let mut cmd = if git {
        test_git::git_command()
    } else {
        Command::new(env!("CARGO_BIN_EXE_aida"))
    };
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("AIDA_") || key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    cmd.current_dir(cwd)
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ID", "fixture-session")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid");
    cmd
}

fn run_git(cwd: &Path, home: &Path, args: &[&str]) -> String {
    let out = command(cwd, home, true, args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

impl World {
    /// A bare hub seeded with a project whose committed config declares
    /// that hub, and an empty store branch.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let hub = dir.path().join("hub.git");
        let seed = dir.path().join("seed");
        let home = dir.path().join("seed-home");
        fs::create_dir_all(&seed).unwrap();
        fs::create_dir_all(&home).unwrap();
        run_git(
            dir.path(),
            &home,
            &["init", "-q", "--bare", "-b", "main", hub.to_str().unwrap()],
        );
        run_git(&seed, &home, &["init", "-q", "-b", "main"]);
        let w = Self { dir, hub };
        fs::create_dir_all(seed.join(".aida")).unwrap();
        fs::write(seed.join(".aida/config.toml"), w.config(&[])).unwrap();
        fs::write(seed.join(".gitignore"), ".aida-store/\n").unwrap();
        run_git(&seed, &home, &["add", "."]);
        run_git(&seed, &home, &["commit", "-qm", "project"]);
        run_git(&seed, &home, &["checkout", "-q", "--orphan", "aida-store"]);
        run_git(&seed, &home, &["rm", "-rqf", "."]);
        run_git(&seed, &home, &["commit", "--allow-empty", "-qm", "store"]);
        run_git(
            &seed,
            &home,
            &["remote", "add", "origin", w.hub.to_str().unwrap()],
        );
        run_git(
            &seed,
            &home,
            &["push", "-q", "origin", "main", "aida-store"],
        );
        w
    }

    /// The project config declaring this hub plus any `extra` hubs.
    fn config(&self, extra: &[(&str, &str)]) -> String {
        let mut text = format!(
            "[store]\nmode='distributed'\nstore_path='.aida-store'\n[store.hubs]\nhub = '{}'\n",
            self.hub.display()
        );
        for (name, url) in extra {
            text.push_str(&format!("{name} = '{url}'\n"));
        }
        text
    }

    /// A clone of the hub with a store worktree, bound by filing its first
    /// spec (which has no agreed ID), as every real clone is.
    fn clone(&self, name: &str) -> Clone {
        let root = self.dir.path().join(name);
        let home = self.dir.path().join(format!("{name}-home"));
        fs::create_dir_all(&home).unwrap();
        run_git(
            self.dir.path(),
            &home,
            &[
                "clone",
                "-q",
                self.hub.to_str().unwrap(),
                root.to_str().unwrap(),
            ],
        );
        let store = root.join(".aida-store");
        run_git(
            &root,
            &home,
            &[
                "worktree",
                "add",
                "-q",
                store.to_str().unwrap(),
                "aida-store",
            ],
        );
        let n = if name == "a" { 7 } else { 8 };
        fs::create_dir_all(store.join(".aida")).unwrap();
        fs::write(
            store.join(".aida/node.toml"),
            format!(
                "node_id='{n}'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n"
            ),
        )
        .unwrap();
        fs::write(
            store.join(".aida/dispenser.toml"),
            format!("[mode.Distributed]\nnode_id='{n}'\n[sequences]\n"),
        )
        .unwrap();
        for f in ["node.toml", "dispenser.toml"] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                store.join(".aida").join(f),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        let c = Clone { root, home, store };
        // Bind the clone (its first add claims a node).
        let out = c.add("bind the clone", "story");
        assert!(out.status.success(), "{}", said(&out));
        c
    }

    fn hub_blocks(&self) -> Vec<aida_core::AgreedIdBlock> {
        let out = command(
            self.dir.path(),
            self.dir.path(),
            true,
            &[
                "--git-dir",
                self.hub.to_str().unwrap(),
                "show",
                "aida-store:registry/blocks.yaml",
            ],
        )
        .output()
        .unwrap();
        if !out.status.success() {
            return Vec::new();
        }
        let copy = self.dir.path().join("hub-blocks.yaml");
        fs::write(&copy, &out.stdout).unwrap();
        aida_core::BlockRegistry::load(&copy).unwrap().blocks
    }

    /// A bare store repository that is not a declared hub, seeded from
    /// `from`'s store: the source of an imported store.
    fn import_repo(&self, from: &Clone) -> PathBuf {
        let path = self.dir.path().join("import.git");
        from.git(
            self.dir.path(),
            &["init", "-q", "--bare", path.to_str().unwrap()],
        );
        from.git(
            &from.store,
            &["remote", "add", "import", path.to_str().unwrap()],
        );
        path
    }
}

impl Clone {
    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        run_git(cwd, &self.home, args)
    }
    fn aida(&self, args: &[&str]) -> Output {
        command(&self.root, &self.home, false, args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let out = self.aida(args);
        assert!(out.status.success(), "aida {args:?}: {}", said(&out));
        said(&out)
    }
    fn refused(&self, args: &[&str]) -> String {
        let out = self.aida(args);
        assert!(
            !out.status.success(),
            "aida {args:?} should refuse: {}",
            said(&out)
        );
        said(&out)
    }
    fn add(&self, title: &str, typ: &str) -> Output {
        self.aida(&[
            "add",
            "--type",
            typ,
            "--title",
            title,
            "--description",
            "fixture",
        ])
    }
    fn node(&self) -> String {
        aida_core::work_binding::load(&self.store)
            .unwrap()
            .expect("bound clone")
            .node_id
    }
    fn by_title(&self, title: &str) -> aida_core::Requirement {
        let objects = self.store.join("objects");
        aida_core::object_store::list_objects(&objects)
            .unwrap()
            .into_iter()
            .map(|(sid, _)| aida_core::object_store::read_object(&objects, &sid).unwrap())
            .find(|r| r.title == title)
            .unwrap_or_else(|| panic!("no object titled {title}"))
    }
    fn blocks(&self) -> Vec<aida_core::AgreedIdBlock> {
        aida_core::BlockRegistry::load(&self.store.join("registry/blocks.yaml"))
            .unwrap()
            .blocks
    }
    /// Commit `config` as the project config on main and publish it to the
    /// shared default branch (only a pushed declaration counts).
    fn commit_config(&self, config: &str) {
        fs::write(self.root.join(".aida/config.toml"), config).unwrap();
        self.git(&self.root, &["commit", "-qam", "config"]);
        self.git(&self.root, &["push", "-q", "origin", "HEAD:main"]);
    }
    /// Bring this clone's store up to the hub's.
    fn pull_store(&self) {
        self.git(
            &self.store,
            &["pull", "-q", "--rebase", "origin", "aida-store"],
        );
    }
}

fn bug_blocks(blocks: &[aida_core::AgreedIdBlock], node: &str) -> Vec<(u32, u32)> {
    blocks
        .iter()
        .filter(|b| b.node_id == node && b.type_prefix == "BUG")
        .map(|b| (b.range_start, b.range_end))
        .collect()
}

/// A hand-written hub-stamped BUG claim for `node` at 500..599.
fn hub_claim_yaml(node: &str) -> String {
    format!(
        "blocks:\n- node_id: '{node}'\n  owner: o\n  hostname: h\n  type_prefix: BUG\n  range_start: 500\n  range_end: 599\n  next: 500\n  allocated_at: 2026-10-31T00:00:00Z\n  pool_id: hub:hub:BUG\n"
    )
}

// AC1 + AC2: no root, two clones of one hub, each bound by filing a spec
// that has no agreed ID; the second clone's stale claim is rejected by the
// hub and retried, the ranges are disjoint, and `aida add` mints an agreed
// ID from the block.
#[test]
fn single_hub_clones_claim_by_cas_and_mint_without_a_root() {
    let w = World::new();
    let a = w.clone("a");
    let b = w.clone("b");
    let shown = a.ok(&["node", "show"]);
    assert!(
        shown.contains("Mode:      single-hub (hub hub → this clone reaches it as `origin`)"),
        "{shown}"
    );
    assert!(!a.store.join("registry/root.yaml").exists());

    a.ok(&["db", "block", "claim", "--type", "BUG", "--size", "10"]);
    // b has not pulled a's claim: its first push is rejected and retried.
    let out = b.ok(&["db", "block", "claim", "--type", "BUG", "--size", "10"]);
    assert!(out.contains("Push rejected"), "{out}");

    let on_hub = w.hub_blocks();
    let (na, nb) = (a.node(), b.node());
    let (ra, rb) = (bug_blocks(&on_hub, &na), bug_blocks(&on_hub, &nb));
    assert_eq!((ra.len(), rb.len()), (1, 1), "{on_hub:?}");
    assert!(
        ra[0].1 < rb[0].0 || rb[0].1 < ra[0].0,
        "disjoint: {ra:?} {rb:?}"
    );
    assert!(on_hub
        .iter()
        .filter(|b| b.type_prefix == "BUG")
        .all(|b| b.pool_id.as_deref() == Some("hub:hub:BUG")));

    let out = a.add("hub minted bug", "bug");
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(
        a.by_title("hub minted bug").agreed_id,
        Some(format!("BUG-{}", ra[0].0)),
        "{}",
        said(&out)
    );
    assert!(a.ok(&["node", "show"]).contains("spendable"));
}

// AC3: a claim committed locally but never pushed to the hub is not
// spendable; add falls back to a node-local ID and says why.
#[test]
fn a_claim_not_on_the_hub_ref_is_never_spendable() {
    let w = World::new();
    let a = w.clone("a");
    let node = a.node();
    let path = a.store.join("registry/blocks.yaml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, hub_claim_yaml(&node)).unwrap();
    a.git(&a.store, &["add", "registry/blocks.yaml"]);
    a.git(&a.store, &["commit", "-qm", "local-only claim"]);
    let shown = a.ok(&["node", "show"]);
    assert!(shown.contains("not spendable"), "{shown}");
    let out = a.add("unpushed claim bug", "bug");
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(a.by_title("unpushed claim bug").agreed_id, None);
    assert!(said(&out).contains("not on hub `hub`"), "{}", said(&out));
}

// AC3: a hub-stamped claim that only arrives through an imported store (a
// store repository that is not the hub, merged into this clone's store) is
// not spendable either.
#[test]
fn a_claim_only_in_an_imported_store_is_never_spendable() {
    let w = World::new();
    let a = w.clone("a");
    let node = a.node();
    let import = w.import_repo(&a);
    let hub_tip = a.git(&a.store, &["rev-parse", "HEAD"]);
    // The imported store carries a claim for this node the hub never saw.
    let path = a.store.join("registry/blocks.yaml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, hub_claim_yaml(&node)).unwrap();
    a.git(&a.store, &["add", "registry/blocks.yaml"]);
    a.git(&a.store, &["commit", "-qm", "claim in the other store"]);
    a.git(&a.store, &["push", "-q", "import", "HEAD:aida-store"]);
    a.git(&a.store, &["reset", "-q", "--hard", &hub_tip]);
    assert!(a.blocks().is_empty() || bug_blocks(&a.blocks(), &node).is_empty());
    // Import it: fetch the other store and merge it in.
    a.git(&a.store, &["fetch", "-q", "import"]);
    a.git(&a.store, &["merge", "-q", "--no-edit", "import/aida-store"]);
    assert_eq!(bug_blocks(&a.blocks(), &node), [(500, 599)]);
    assert!(w.hub_blocks().iter().all(|b| b.range_start != 500));
    let out = a.add("imported claim bug", "bug");
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(
        a.by_title("imported claim bug").agreed_id,
        None,
        "{}",
        said(&out)
    );
    assert!(said(&out).contains("not on hub `hub`"), "{}", said(&out));
    assert!(import.exists());
}

// AC3: a real `db block claim` whose push the hub rejects (a pre-receive
// hook) never yields a spendable block: nothing lands on the hub, and add
// mints no agreed ID from the refused range.
#[test]
fn a_claim_the_hub_rejected_is_never_spendable() {
    let w = World::new();
    let a = w.clone("a");
    let node = a.node();
    let hook = w.hub.join("hooks/pre-receive");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\necho refused by policy >&2\nexit 1\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _ = a.refused(&["db", "block", "claim", "--type", "BUG", "--size", "10"]);
    assert!(bug_blocks(&w.hub_blocks(), &node).is_empty());
    let out = a.add("rejected claim bug", "bug");
    if out.status.success() {
        assert_eq!(
            a.by_title("rejected claim bug").agreed_id,
            None,
            "{}",
            said(&out)
        );
    }
    assert!(bug_blocks(&w.hub_blocks(), &node).is_empty());
}

// AC3: a legacy v1 authoritative clone in single-hub mode gets the same
// on-hub check: its unpushed hub-stamped claim is not spent either.
#[test]
fn an_authoritative_clone_does_not_spend_an_unpushed_hub_claim() {
    let w = World::new();
    let a = w.clone("a");
    fs::write(
        a.root.join(".git/aida-allocation.toml"),
        "version = 1\nauthoritative = true\n",
    )
    .unwrap();
    let node = a.node();
    let path = a.store.join("registry/blocks.yaml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, hub_claim_yaml(&node)).unwrap();
    a.git(&a.store, &["add", "registry/blocks.yaml"]);
    a.git(&a.store, &["commit", "-qm", "local-only claim on the root"]);
    let out = a.add("authoritative unpushed bug", "bug");
    assert!(out.status.success(), "{}", said(&out));
    assert_ne!(
        a.by_title("authoritative unpushed bug").agreed_id.as_deref(),
        Some("BUG-500"),
        "{}",
        said(&out)
    );
    assert!(said(&out).contains("not on hub `hub`"), "{}", said(&out));
}

// AC4: a second non-mirror hub with no root, once on the shared default
// branch, refuses claims and names the enroll command; a block already held
// keeps dispensing.
#[test]
fn a_second_hub_without_a_root_refuses_claims_but_held_blocks_dispense() {
    let w = World::new();
    let a = w.clone("a");
    a.ok(&["db", "block", "claim", "--type", "BUG", "--size", "10"]);
    a.commit_config(&w.config(&[("airgap", "https://git.airgap.invalid/aida.git")]));
    let err = a.refused(&["db", "block", "claim", "--type", "TASK", "--size", "10"]);
    assert!(err.contains("aida node enroll-root"), "{err}");
    assert!(a.ok(&["node", "show"]).contains("multi-hub, NO ROOT"));
    let out = a.add("held block bug", "bug");
    assert!(out.status.success(), "{}", said(&out));
    assert!(
        a.by_title("held block bug").agreed_id.is_some(),
        "{}",
        said(&out)
    );
}

// AC1 / design Q2: a second hub committed only on a clone's local main
// (never pushed) does not switch that clone's mode: it still claims on the
// one shared hub.
#[test]
fn a_hub_declared_only_on_local_main_does_not_count() {
    let w = World::new();
    let a = w.clone("a");
    fs::write(
        a.root.join(".aida/config.toml"),
        w.config(&[("airgap", "https://git.airgap.invalid/aida.git")]),
    )
    .unwrap();
    a.git(&a.root, &["commit", "-qam", "local-only second hub"]);
    assert!(!a.ok(&["node", "show"]).contains("multi-hub"));
    a.ok(&["db", "block", "claim", "--type", "BUG", "--size", "10"]);
    assert_eq!(bug_blocks(&w.hub_blocks(), &a.node()).len(), 1);
}

// AC5: enroll-root on a single-hub store is dry-run first, then turns the
// implicit pools into root pools: same ids, nothing renumbered, every
// earlier ID still resolves and the lineage is conflict-free. Another
// clone's pre-enrollment block stays spendable through its own remote.
#[test]
fn enroll_root_promotes_the_implicit_pools_without_renumbering() {
    let w = World::new();
    let a = w.clone("a");
    let b = w.clone("b");
    // b names the hub `upstream`, not after the hub.
    b.git(&b.store, &["remote", "rename", "origin", "upstream"]);
    b.ok(&["db", "block", "claim", "--type", "BUG", "--size", "10"]);
    a.pull_store();
    a.ok(&["db", "block", "claim", "--type", "BUG", "--size", "10"]);
    let out = a.add("before enrollment", "bug");
    assert!(out.status.success(), "{}", said(&out));
    let agreed = a
        .by_title("before enrollment")
        .agreed_id
        .expect("agreed id");

    let dry = a.ok(&["node", "enroll-root", "--dry-run"]);
    assert!(dry.contains("Would record the single-hub pools"), "{dry}");
    assert!(dry.contains("hub:hub:BUG"), "{dry}");
    assert!(!a.store.join("registry/root.yaml").exists());
    assert!(!a.store.join("registry/pools.yaml").exists());

    let before = a.blocks();
    let done = a.ok(&["node", "enroll-root"]);
    assert!(done.contains("hub:hub:BUG"), "{done}");
    let pools = fs::read_to_string(a.store.join("registry/pools.yaml")).unwrap();
    assert!(pools.contains("pool_id: hub:hub:BUG"), "{pools}");
    assert_eq!(a.blocks(), before, "no block was renumbered");
    assert!(a.ok(&["show", &agreed]).contains("before enrollment"));
    let shown = a.ok(&["node", "show"]);
    assert!(!shown.contains("Conflicts"), "{shown}");
    assert!(shown.contains("rooted (root node"), "{shown}");

    // The other clone, which reaches the hub as `upstream`, still spends
    // its pre-enrollment block.
    b.git(
        &b.store,
        &["pull", "-q", "--rebase", "upstream", "aida-store"],
    );
    assert!(b.store.join("registry/root.yaml").exists());
    let rb = bug_blocks(&b.blocks(), &b.node());
    assert_eq!(rb.len(), 1, "{:?}", b.blocks());
    let shown = b.ok(&["node", "show"]);
    assert!(!shown.contains("not spendable"), "{shown}");
    let out = b.add("after enrollment", "bug");
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(
        b.by_title("after enrollment").agreed_id,
        Some(format!("BUG-{}", rb[0].0)),
        "{}",
        said(&out)
    );
}

// AC1: a mirror remote that is also a declared hub is reported.
#[test]
fn node_show_reports_a_remote_that_is_both_mirror_and_hub() {
    let w = World::new();
    let a = w.clone("a");
    let mut config = w.config(&[]);
    config.push_str("[store.sync]\nmirror_remotes = ['origin']\n");
    a.commit_config(&config);
    let shown = a.ok(&["node", "show"]);
    assert!(
        shown.contains("a remote cannot be both a mirror and a hub"),
        "{shown}"
    );
}
