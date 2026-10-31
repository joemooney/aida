//! Candidate-binary coverage of STORY-HOME-105, along the documented
//! post-merge runbook with no hand-written blocks: a clone with no usable
//! block writes a local ID only and shows the agreed ID as pending (show and
//! the agent list); after `node enroll-root` a block claim is refused until
//! the agreed-ID backfill has run; the backfill assigns agreed IDs, renames
//! the legacy node's specs into the bound node and retires it, and claims
//! the root's blocks above every backfilled number; `aida add` then writes
//! the agreed ID from that block AND the local ID in one object; the merge
//! gate gives another node's pending spec its agreed ID; and a clone whose
//! local setting says root while the store records another root refuses
//! before dispensing.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
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
    // The agent list carries the pending marker in its local_id column.
    let listed = f.aida_ok(&["list"]);
    assert!(listed.contains("local_id"), "{listed}");
    assert!(listed.contains("(agreed id pending)"), "{listed}");

    // Runbook step 0: enroll as root.
    f.aida_ok(&["node", "enroll-root"]);
    // The store's legacy node 7 is registered and still names a spec, and a
    // flat STORY-40 raises the STORY floor.
    let registry = f.store.join("registry");
    fs::create_dir_all(&registry).unwrap();
    let nodes_toml = registry.join("nodes.toml");
    let mut nodes = fs::read_to_string(&nodes_toml).unwrap_or_default();
    nodes.push_str(
        "\n[[nodes]]\nid = \"7\"\nuser_id = 1\nhostname = \"\"\nregistered = \"2026-01-01T00:00:00Z\"\n",
    );
    fs::write(&nodes_toml, nodes).unwrap();
    let objects = f.store.join("objects");
    let mut legacy = aida_core::Requirement::new("legacy node spec".into(), "fixture".into());
    legacy.spec_id = Some("STORY-7-001".into());
    legacy.req_type = aida_core::RequirementType::Story;
    aida_core::object_store::write_object(&objects, &legacy).unwrap();
    let mut flat = aida_core::Requirement::new("flat spec".into(), "fixture".into());
    flat.spec_id = Some("STORY-40".into());
    flat.req_type = aida_core::RequirementType::Story;
    aida_core::object_store::write_object(&objects, &flat).unwrap();

    // A block claim before the backfill is refused, and writes nothing.
    let blocks = registry.join("blocks.yaml");
    let out = f.aida(&["db", "block", "claim", "--type", "STORY"]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("before the agreed-ID backfill"), "{said}");
    assert!(!blocks.exists(), "the refused claim wrote no ledger");

    // Runbook step 1: dry run lists both mappings and the claims.
    let dry = f.aida_ok(&["doctor", "--fix", "agreed-ids", "--dry-run"]);
    for needle in [
        "Local ID -> agreed ID:",
        "STORY-7-001",
        "Node 7 local ID -> node",
        "Node 7 is marked retired",
        "Blocks claimed for node",
        "STORY-43..142",
        "dry run: nothing written",
    ] {
        assert!(dry.contains(needle), "{needle}: {dry}");
    }
    assert!(!blocks.exists(), "the dry run wrote no ledger");

    // Runbook step 2: apply.
    f.aida_ok(&["doctor", "--fix", "agreed-ids", "--yes"]);
    let first = f.by_title("before root");
    let moved = f.by_title("legacy node spec");
    let mut agreed: Vec<String> = [&first, &moved]
        .iter()
        .map(|r| r.agreed_id.clone().unwrap())
        .collect();
    agreed.sort();
    assert_eq!(agreed, vec!["STORY-41", "STORY-42"]);
    assert_eq!(
        first.spec_id.as_deref(),
        Some(local.as_str()),
        "local ID kept"
    );
    let moved_local = moved.spec_id.clone().unwrap();
    assert!(
        moved_local
            .to_ascii_uppercase()
            .starts_with(&format!("STORY-{node}-")),
        "the legacy spec now has a bound-node local ID: {moved_local}"
    );
    assert_ne!(moved_local, local);
    let nodes = fs::read_to_string(registry.join("nodes.toml")).unwrap();
    assert!(nodes.contains("retired_at"), "{nodes}");
    // Runbook step 3 happened inside the backfill: the root's own STORY
    // block starts above every agreed and backfilled number.
    let ledger = aida_core::BlockRegistry::load(&blocks).unwrap();
    let own = ledger
        .blocks
        .iter()
        .find(|b| b.node_id == binding.node_id && b.type_prefix == "STORY" && !b.is_exhausted())
        .expect("the backfill claimed a STORY block for the bound node");
    assert_eq!((own.range_start, own.range_end), (43, 142));
    // A re-run is a no-op.
    let again = f.aida_ok(&["doctor", "--fix", "agreed-ids", "--dry-run"]);
    assert!(again.contains("nothing to backfill"), "{again}");

    // Runbook step 4 (verify): `aida add` mints an agreed ID, not pending.
    let out = f.add("after root");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let second = f.by_title("after root");
    assert_eq!(second.agreed_id.as_deref(), Some("STORY-43"), "{stdout}");
    let local = second.spec_id.clone().unwrap();
    assert!(
        local.to_ascii_uppercase().contains(&node),
        "the local ID is the bound node's, never the store's legacy node 7: {local}"
    );
    assert!(
        stdout.contains("STORY-43 ("),
        "agreed first, local beside: {stdout}"
    );
    // Both IDs open the same spec, and the agreed ID of the moved spec too.
    for (id, title) in [
        ("STORY-43", "after root"),
        (local.as_str(), "after root"),
        (moved.agreed_id.as_deref().unwrap(), "legacy node spec"),
        (moved_local.as_str(), "legacy node spec"),
    ] {
        let shown = f.aida_ok(&["show", id]);
        assert!(shown.contains(title), "{id}: {shown}");
    }
    // The agent list shows the local ID beside the agreed one.
    let listed = f.aida_ok(&["list"]);
    let row = listed
        .lines()
        .find(|l| l.contains("after root"))
        .unwrap_or_else(|| panic!("{listed}"));
    assert!(row.contains("STORY-43"), "{row}");
    assert!(row.to_ascii_uppercase().contains(&node), "{row}");

    // AC2: a sync reaching the block holder (merge-gate promotion) gives
    // another node's pending local-only spec its agreed ID, above the
    // root's own block, and keeps its local ID.
    let other = format!("STORY-{}-001", "c".repeat(32));
    let mut pending = aida_core::Requirement::new("other node spec".into(), "fixture".into());
    pending.spec_id = Some(other.clone());
    pending.req_type = aida_core::RequirementType::Story;
    aida_core::object_store::write_object(&objects, &pending).unwrap();
    let shown = f.aida_ok(&["show", &other]);
    assert!(shown.contains("pending"), "{shown}");
    // The merge gate is an advisor act: seat a grant the way `aida role
    // enter advisor` would at a human TTY.
    let grant = support::ensure_seat_for(&f.home, &f.root, "t105", "advisor", &[])
        .expect("the project is initialized");
    let out = f
        .command(false, &["db", "merge-gate"])
        .env("AIDA_USER", "t105")
        .env("AIDA_SESSION_GRANT", &grant)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let gated = f.by_title("other node spec");
    assert_eq!(
        gated.spec_id.as_deref(),
        Some(other.as_str()),
        "local ID kept"
    );
    let gated_agreed = gated.agreed_id.clone().expect("merge gate assigned one");
    let n: u32 = gated_agreed.rsplit('-').next().unwrap().parse().unwrap();
    assert!(n > 142, "outside the root's own block: {gated_agreed}");
    let shown = f.aida_ok(&["show", &other]);
    assert!(
        !shown.contains("agreed_id: pending") && !shown.contains("agreed id pending"),
        "{shown}"
    );
    assert!(shown.contains(&gated_agreed), "{shown}");

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
