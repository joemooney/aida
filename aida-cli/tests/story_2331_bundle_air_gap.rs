//! Candidate-binary regression of the supported air-gap workflow (SPIKE-708):
//! the allocation root grants a pool to a hub on the far side of an air gap,
//! a `git bundle` carries the grant in, clones of the inside hub claim and
//! spend blocks ordered by that hub alone, and a second bundle carries their
//! store back out, where `aida remote reconcile` unions it with the root's
//! divergent claims and refuses an overlapping claim without moving anything.
//!
//! Two local bare repositories stand in for the outside and inside hubs; the
//! outside hub is moved away for the whole inside phase, so nothing inside
//! can reach it. Only direct root-to-hub grants are covered: recursive
//! delegation and pool replenishment are not supported.
// trace:STORY-B6B1278890774EF993D2EBC327862470-329 | ai:claude
#[path = "support/test_git.rs"]
mod test_git;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

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

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

struct Clone {
    root: PathBuf,
    home: PathBuf,
    store: PathBuf,
}

impl Clone {
    fn at(dir: &Path, name: &str) -> Self {
        let root = dir.join(name);
        let home = dir.join(format!("{name}-home"));
        fs::create_dir_all(&home).unwrap();
        Self {
            store: root.join(".aida-store"),
            root,
            home,
        }
    }
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
    fn add(&self, title: &str, typ: &str) -> String {
        self.ok(&[
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
    fn agreed(&self, title: &str) -> Option<String> {
        let objects = self.store.join("objects");
        aida_core::object_store::list_objects(&objects)
            .unwrap()
            .into_iter()
            .map(|(sid, _)| aida_core::object_store::read_object(&objects, &sid).unwrap())
            .find(|r| r.title == title)
            .unwrap_or_else(|| panic!("no object titled {title}"))
            .agreed_id
    }
    fn blocks(&self) -> Vec<aida_core::AgreedIdBlock> {
        aida_core::BlockRegistry::load(&self.store.join("registry/blocks.yaml"))
            .unwrap()
            .blocks
    }
    /// The store worktree's `git status --porcelain` (untracked included).
    fn store_status(&self) -> String {
        self.git(
            &self.store,
            &["status", "--porcelain", "--untracked-files=all"],
        )
    }
    /// Rebase this clone's store onto its hub and publish it there.
    fn publish_store(&self) {
        self.git(
            &self.store,
            &["pull", "-q", "--rebase", "origin", "aida-store"],
        );
        self.git(&self.store, &["push", "-q", "origin", "HEAD:aida-store"]);
    }
}

/// A clone of bare repository `hub` with a store worktree and a hand-written
/// node identity, bound by filing its first spec, as every real clone is.
fn inside_clone(dir: &Path, name: &str, node: u32, hub: &Path) -> Clone {
    let c = Clone::at(dir, name);
    c.git(dir, &["clone", "-q", s(hub), s(&c.root)]);
    c.git(
        &c.root,
        &["worktree", "add", "-q", s(&c.store), "aida-store"],
    );
    fs::create_dir_all(c.store.join(".aida")).unwrap();
    fs::write(
        c.store.join(".aida/node.toml"),
        format!("node_id='{node}'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n"),
    )
    .unwrap();
    fs::write(
        c.store.join(".aida/dispenser.toml"),
        format!("[mode.Distributed]\nnode_id='{node}'\n[sequences]\n"),
    )
    .unwrap();
    for f in ["node.toml", "dispenser.toml"] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            c.store.join(".aida").join(f),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    c.add("bind the clone", "story");
    c
}

/// A store file at `aida-store` on bare repository `hub`, if any.
fn hub_file(home: &Path, hub: &Path, file: &str) -> Option<String> {
    let out = command(
        home,
        home,
        true,
        &["--git-dir", s(hub), "show", &format!("aida-store:{file}")],
    )
    .output()
    .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).unwrap())
}

/// The block claims on bare repository `hub`'s store branch.
fn hub_blocks(home: &Path, hub: &Path) -> Vec<aida_core::AgreedIdBlock> {
    let Some(text) = hub_file(home, hub, "registry/blocks.yaml") else {
        return Vec::new();
    };
    let copy = home.join("hub-blocks.yaml");
    fs::write(&copy, text).unwrap();
    aida_core::BlockRegistry::load(&copy).unwrap().blocks
}

fn ranges(blocks: &[aida_core::AgreedIdBlock], node: &str, typ: &str) -> Vec<(u32, u32)> {
    let mut r: Vec<(u32, u32)> = blocks
        .iter()
        .filter(|b| b.node_id == node && b.type_prefix == typ)
        .map(|b| (b.range_start, b.range_end))
        .collect();
    r.sort();
    r
}

fn hub_ref(home: &Path, hub: &Path) -> String {
    run_git(
        home,
        home,
        &["--git-dir", s(hub), "rev-parse", "aida-store"],
    )
}

// The whole SPIKE-708 round trip, in order: grant in by bundle, inside
// claims (one observed collision, retried into a disjoint range), final-slot
// and exhaustion behaviour, a divergent outside claim, the return bundle's
// disjoint union, the overlap refusal that changes nothing, and an ordinary
// clean pull from the return bundle. Along the way the root's block claim
// must leave its store clean enough to reconcile (SPIKE-708 Q4).
// trace:STORY-B6B1278890774EF993D2EBC327862470-329 | ai:claude
#[test]
fn a_pool_grant_round_trips_an_air_gap_by_git_bundle() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let outside = dir.join("outside.git");
    let inside = dir.join("inside.git");
    let away = dir.join("outside-unreachable.git");

    // ── The allocation root, set up the real way: `aida init`, one declared
    // hub, a pushed declaration.
    let r = Clone::at(dir, "root");
    fs::create_dir_all(&r.root).unwrap();
    r.git(dir, &["init", "-q", "--bare", "-b", "main", s(&outside)]);
    r.git(&r.root, &["init", "-q", "-b", "main"]);
    r.ok(&["init", "--no-skills", "--no-hooks", "--no-agent-config"]);
    let config_path = r.root.join(".aida/config.toml");
    let mut config = fs::read_to_string(&config_path)
        .unwrap()
        .replace("counter_scope = \"global\"", "counter_scope = \"per-type\"");
    config.push_str(&format!(
        "\n[store.hubs]\noutside = '{}'\n",
        outside.display()
    ));
    fs::write(&config_path, config).unwrap();
    r.git(&r.root, &["add", ".aida/config.toml"]);
    r.git(&r.root, &["commit", "-qm", "declare the outside hub"]);
    r.git(&r.root, &["remote", "add", "origin", s(&outside)]);
    r.git(&r.root, &["push", "-q", "origin", "main", "aida-store"]);

    r.ok(&["db", "block", "claim", "--type", "BUG", "--size", "10"]);
    r.ok(&["db", "block", "claim", "--type", "TASK", "--size", "10"]);
    r.ok(&["node", "enroll-root"]);
    let granted = r.ok(&[
        "node",
        "hub",
        "add",
        "inside",
        "--url",
        s(&inside),
        "--size",
        "4",
    ]);
    assert!(granted.contains("Granted hub `inside` TASK"), "{granted}");
    r.ok(&["doctor", "--fix", "agreed-ids", "--yes"]);
    r.git(&r.root, &["commit", "-qam", "declare the inside hub"]);
    r.git(&r.root, &["push", "-q", "origin", "main", "aida-store"]);
    let pools = {
        let copy = dir.join("granted");
        fs::create_dir_all(copy.join("registry")).unwrap();
        fs::write(
            copy.join("registry/pools.yaml"),
            hub_file(&r.home, &outside, "registry/pools.yaml").expect("pools on the outside hub"),
        )
        .unwrap();
        aida_core::allocation_lineage::PoolRegistry::load(&copy).unwrap()
    };
    let task_pool = pools
        .pools
        .iter()
        .find(|p| p.pool_id == "hub:inside:TASK")
        .expect("a TASK pool granted to the inside hub")
        .clone();
    assert_eq!(task_pool.range_end - task_pool.range_start + 1, 4);

    // ── Grant in: one bundle, fetched into the inside hub. Nothing inside
    // names or reaches the outside hub from here on.
    let grant = dir.join("grant.bundle");
    r.git(
        &r.root,
        &["bundle", "create", "-q", s(&grant), "main", "aida-store"],
    );
    r.git(dir, &["init", "-q", "--bare", "-b", "main", s(&inside)]);
    r.git(
        dir,
        &[
            "--git-dir",
            s(&inside),
            "fetch",
            "-q",
            s(&grant),
            "refs/heads/main:refs/heads/main",
            "refs/heads/aida-store:refs/heads/aida-store",
        ],
    );
    fs::rename(&outside, &away).unwrap();

    let c1 = inside_clone(dir, "c1", 7, &inside);
    let c2 = inside_clone(dir, "c2", 8, &inside);
    for c in [&c1, &c2] {
        c.ok(&["node", "upstream", "origin"]);
    }

    // ── Claims inside: c2 has not seen c1's claim, so the inside hub rejects
    // its first push and it retries into a disjoint range.
    c1.ok(&["db", "block", "claim", "--type", "TASK", "--size", "2"]);
    let raced = c2.ok(&["db", "block", "claim", "--type", "TASK", "--size", "2"]);
    assert!(raced.contains("Push rejected"), "{raced}");
    let on_inside = hub_blocks(&c1.home, &inside);
    let (n1, n2) = (c1.node(), c2.node());
    let (r1, r2) = (
        ranges(&on_inside, &n1, "TASK"),
        ranges(&on_inside, &n2, "TASK"),
    );
    assert_eq!((r1.len(), r2.len()), (1, 1), "{on_inside:?}");
    let ((s1, e1), (s2, e2)) = (r1[0], r2[0]);
    assert!(e1 < s2 || e2 < s1, "disjoint: {r1:?} {r2:?}");
    for (start, end) in [(s1, e1), (s2, e2)] {
        assert!(
            task_pool.range_start <= start && end <= task_pool.range_end,
            "{start}..{end} outside {task_pool:?}"
        );
    }
    assert!(
        on_inside
            .iter()
            .filter(|b| b.node_id == n1 || b.node_id == n2)
            .all(|b| b.pool_id.as_deref() == Some("hub:inside:TASK")),
        "{on_inside:?}"
    );

    // ── Exhaustion and the final slot: the pool is spent, so a further
    // claim is refused; c1's block still dispenses both IDs, the last one
    // included, and the add after that falls back to a node-local ID.
    let spent = c1.refused(&["db", "block", "claim", "--type", "TASK", "--size", "1"]);
    assert!(spent.contains("is exhausted"), "{spent}");
    c1.add("first inside task", "task");
    let last = c1.add("final-slot inside task", "task");
    assert_eq!(c1.agreed("first inside task"), Some(format!("TASK-{s1}")));
    assert_eq!(
        c1.agreed("final-slot inside task"),
        Some(format!("TASK-{e1}")),
        "{last}"
    );
    let fell_back = c1.add("fallback inside task", "task");
    assert_eq!(c1.agreed("fallback inside task"), None, "{fell_back}");
    assert!(fell_back.contains("node-local ID"), "{fell_back}");
    // (The fixture clones skip `aida init`, so their stores carry no oplog
    // merge driver: c2 catches up before filing rather than rebasing.)
    c2.git(
        &c2.store,
        &["pull", "-q", "--rebase", "origin", "aida-store"],
    );
    c2.add("c2 inside task", "task");
    assert_eq!(c2.agreed("c2 inside task"), Some(format!("TASK-{s2}")));
    c1.publish_store();
    c2.publish_store();

    // ── Back outside: the root claims again while the inside store is away.
    fs::rename(&away, &outside).unwrap();
    r.ok(&["db", "block", "claim", "--type", "BUG", "--size", "5"]);
    let root_bug = ranges(&r.blocks(), &r.node(), "BUG");
    // SPIKE-708 Q4: the claim leaves no untracked dispense lock (or anything
    // else) that would make reconcile refuse a dirty store.
    let status = r.store_status();
    assert!(!status.contains("blocks.lock"), "{status}");
    assert!(
        status.is_empty(),
        "the root's store is dirty after a claim: {status}"
    );

    // ── Bring out: one bundle of the inside store, reconciled on the root.
    let back = dir.join("return.bundle");
    r.git(
        dir,
        &[
            "--git-dir",
            s(&inside),
            "bundle",
            "create",
            "-q",
            s(&back),
            "aida-store",
        ],
    );
    r.git(&r.root, &["remote", "add", "arrival", s(&back)]);
    r.ok(&["remote", "push-hubs", "origin"]);
    let union = r.ok(&["remote", "reconcile", "--execute", "--yes"]);
    assert!(union.contains("ranges disjoint"), "{union}");
    let merged = r.blocks();
    assert_eq!(ranges(&merged, &n1, "TASK"), r1, "{union}");
    assert_eq!(ranges(&merged, &n2, "TASK"), r2, "{union}");
    assert_eq!(ranges(&merged, &r.node(), "BUG"), root_bug, "{union}");
    assert_eq!(
        r.agreed("final-slot inside task"),
        Some(format!("TASK-{e1}"))
    );
    assert_eq!(r.agreed("c2 inside task"), Some(format!("TASK-{s2}")));
    let tip = r.git(&r.store, &["rev-parse", "HEAD"]);
    assert_eq!(hub_ref(&r.home, &outside), tip, "the union reached the hub");
    assert!(r.store_status().is_empty(), "{}", r.store_status());

    // ── An overlapping claim carried out is refused and changes nothing:
    // c2's store gains a forged claim on c1's TASK range.
    c2.git(
        &c2.store,
        &["pull", "-q", "--rebase", "origin", "aida-store"],
    );
    // Written by hand, as a tampered store would be: the registry's own
    // writers refuse to save it.
    let blocks_path = c2.store.join("registry/blocks.yaml");
    let twin = format!(
        "blocks:\n- node_id: '{n2}'\n  owner: o\n  hostname: h\n  type_prefix: TASK\n  range_start: {s1}\n  range_end: {e1}\n  next: {s1}\n  allocated_at: 2026-11-03T00:00:00Z\n  pool_id: hub:inside:TASK\n"
    );
    let text = fs::read_to_string(&blocks_path).unwrap();
    assert!(text.starts_with("blocks:\n"), "{text}");
    fs::write(&blocks_path, text.replacen("blocks:\n", &twin, 1)).unwrap();
    assert_eq!(
        ranges(&c2.blocks(), &n2, "TASK"),
        [(s1, e1), (s2, e2)],
        "the forged claim parses"
    );
    c2.git(&c2.store, &["add", "registry/blocks.yaml"]);
    c2.git(&c2.store, &["commit", "-qm", "forged overlapping claim"]);
    let overlap = dir.join("overlap.bundle");
    c2.git(
        &c2.root,
        &["bundle", "create", "-q", s(&overlap), "aida-store"],
    );
    r.git(&r.root, &["remote", "set-url", "arrival", s(&overlap)]);
    let before = (
        r.git(&r.store, &["rev-parse", "HEAD"]),
        r.git(&r.root, &["rev-parse", "HEAD"]),
        r.store_status(),
        hub_ref(&r.home, &outside),
    );
    let err = r.refused(&["remote", "reconcile", "--execute", "--yes"]);
    assert!(err.contains("allocation claims check refused"), "{err}");
    let after = (
        r.git(&r.store, &["rev-parse", "HEAD"]),
        r.git(&r.root, &["rev-parse", "HEAD"]),
        r.store_status(),
        hub_ref(&r.home, &outside),
    );
    assert_eq!(before, after, "a refused reconcile moves nothing");

    // ── An ordinary clean pull from the return bundle: a clone made from
    // the grant bundle reaches exactly the returned inside tip.
    let p = Clone::at(dir, "puller");
    p.git(
        dir,
        &["clone", "-q", "--branch", "main", s(&grant), s(&p.root)],
    );
    p.ok(&[
        "init",
        "--footprint",
        "minimal",
        "--no-skills",
        "--no-hooks",
        "--no-post-hooks",
        "--no-roles",
        "--no-agent-config",
        "--no-schedule",
    ]);
    p.git(&p.root, &["remote", "set-url", "origin", s(&back)]);
    let pulled = p.ok(&["db", "sync", "--pull"]);
    let returned = p.git(&p.root, &["ls-remote", s(&back), "refs/heads/aida-store"]);
    let returned = returned.split_whitespace().next().unwrap().to_string();
    assert_eq!(
        p.git(&p.store, &["rev-parse", "HEAD"]),
        returned,
        "{pulled}"
    );
}
