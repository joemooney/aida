#![cfg(target_os = "linux")]
//! Black-box coverage for friendly node aliases: `aida node alias` management,
//! alias input resolving to the canonical requirement, the JSON `display_id`
//! contract, `--canonical-ids`, and the stored object never carrying a label.
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
#[allow(dead_code)]
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

    /// [`Self::ok`] under an advisor seat grant, which holds queue dispatch
    /// authority.
    fn ok_as_advisor(&self, args: &[&str]) -> String {
        let grant = support::ensure_seat(&self.home, &self.root, "advisor", &["implementer"])
            .expect("fixture has .aida/config.toml");
        let out = self
            .command(false, args)
            .env("AIDA_SESSION_ROLE", "advisor")
            .env("AIDA_SESSION_GRANT", grant)
            .output()
            .unwrap();
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

/// A running `aida mcp-serve`, driven one JSON-RPC request at a time.
struct Mcp {
    child: std::process::Child,
    out: std::io::BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn spawn(f: &Fixture) -> Self {
        let mut child = f
            .command(false, &["mcp-serve"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let out = std::io::BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            out,
            next_id: 0,
        }
    }

    /// Call `tool` and return its text content (error results included).
    fn call(&mut self, tool: &str, arguments: serde_json::Value) -> String {
        use std::io::{BufRead, Write};
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "tools/call",
            "params": {"name": tool, "arguments": arguments},
        });
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(
                self.out.read_line(&mut line).unwrap() > 0,
                "mcp-serve exited"
            );
            let Ok(response) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if response["id"] == self.next_id {
                return response["result"]["content"][0]["text"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| response.to_string());
            }
        }
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

/// Alias input works through CLI flags (`--parent`, `rel add --from/--to`),
/// relationship targets and MCP tool arguments — including a label set while
/// the MCP server is already running — and everything written stays
/// canonical: relationship edges, stored objects and generated commit
/// trailers. Read surfaces (show human/TOON relationship rows, MCP rows)
/// carry the friendly form; JSON keeps the canonical id plus `display_id`.
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
#[test]
fn node_alias_input_through_flags_relationships_and_mcp_writes_canonical() {
    let f = Fixture::new();
    f.ok(&["add", "--title", "parent story", "--type", "story"]);
    f.ok(&["add", "--title", "aliased defect", "--type", "bug"]);
    let node = aida_core::work_binding::load(&f.store)
        .unwrap()
        .unwrap()
        .node_id
        .to_ascii_uppercase();
    let story = format!("STORY-{node}-001");
    let bug = format!("BUG-{node}-001");
    let task = format!("TASK-{node}-001");
    let uuid_of = |id: &str| f.show_json(&[id])["uuid"].as_str().unwrap().to_string();
    let story_uuid = uuid_of(&story);

    // The MCP server starts BEFORE the label exists and still resolves it.
    let mut mcp = Mcp::spawn(&f);
    f.ok(&["node", "alias", "set", "HOME"]);
    let shown = mcp.call(
        "show_requirement",
        serde_json::json!({"id": "bug-home-001"}),
    );
    assert!(
        shown.to_ascii_uppercase().contains(&bug) && shown.contains("BUG-HOME-001"),
        "{shown}"
    );
    let listed = mcp.call("list_requirements", serde_json::json!({}));
    assert!(
        listed
            .to_ascii_uppercase()
            .contains(&format!("{bug} (DISPLAY_ID: BUG-HOME-001)")),
        "{listed}"
    );
    let added = mcp.call(
        "add_relationship",
        serde_json::json!({
            "spec_id": "BUG-HOME-001",
            "relationship_type": "references",
            "target_spec_id": "story-home-001",
        }),
    );
    assert!(!added.to_lowercase().contains("not found"), "{added}");
    drop(mcp);

    // CLI flags: `--parent` and `rel add --from/--to` take the alias form.
    f.ok(&[
        "add",
        "--title",
        "child task",
        "--type",
        "task",
        "--parent",
        "STORY-HOME-001",
    ]);
    f.ok(&[
        "rel",
        "add",
        "--from",
        "task-home-001",
        "--to",
        "BUG-HOME-001",
        "--type",
        "references",
    ]);

    // Every edge stores the target's uuid; no object carries the label.
    for (id, target) in [(&bug, &story_uuid), (&task, &story_uuid)] {
        let path = f.store.join(format!(
            "objects/{}/000/{id}.yaml",
            id.split('-').next().unwrap()
        ));
        let yaml = fs::read_to_string(&path).unwrap();
        assert!(yaml.contains(target.as_str()), "{id} edge missing:\n{yaml}");
        assert!(!yaml.contains("HOME"), "{id} stored a label:\n{yaml}");
    }

    // Relationship rows show the friendly form on human and TOON surfaces;
    // JSON keeps the canonical id and adds display_id.
    let human = f.ok(&["show", "TASK-HOME-001", "--no-git", "--format", "human"]);
    assert!(
        human.contains("STORY-HOME-001") && human.contains("BUG-HOME-001"),
        "{human}"
    );
    let toon = f.ok(&["show", "TASK-HOME-001", "--no-git", "--format", "toon"]);
    assert!(toon.contains("STORY-HOME-001"), "{toon}");
    let json = f.show_json(&["TASK-HOME-001"]);
    let rels = json["relationships"].as_array().unwrap();
    assert!(
        rels.iter().any(|r| r["id"]
            .as_str()
            .is_some_and(|id| id.eq_ignore_ascii_case(&story))
            && r["display_id"] == "STORY-HOME-001"),
        "{json}"
    );

    // A generated commit trailer is canonical even when --spec uses the label.
    let message = f.ok(&[
        "commit",
        "--dry-run",
        "--type",
        "fix",
        "--message",
        "aliased spec",
        "--spec",
        "task-home-001",
    ]);
    assert!(message.trim().ends_with(&format!("({task})")), "{message}");

    // Inferred from staged traces (no --spec): the label form expands too, and
    // the label and canonical forms of one spec count as ONE spec (not an
    // ambiguous pair that silently drops the trailer).
    fs::write(
        f.root.join("lib.rs"),
        format!("// trace:TASK-HOME-001 | ai:claude\n// trace:{task} | ai:claude\n"),
    )
    .unwrap();
    f.git(&["add", "lib.rs"]);
    let message = f.ok(&[
        "commit",
        "--dry-run",
        "--type",
        "fix",
        "--message",
        "inferred spec",
    ]);
    assert!(message.trim().ends_with(&format!("({task})")), "{message}");
    assert!(!message.contains("HOME"), "{message}");

    // A label equal to a registered node id is refused on rename as on set.
    let nodes = f.store.join("registry/nodes.toml");
    let mut roster = fs::read_to_string(&nodes).unwrap_or_default();
    roster.push_str(
        "\n[[nodes]]\nid = \"w2\"\nuser_id = 9\nhostname = \"h\"\nregistered = \"2026-01-01T00:00:00Z\"\n",
    );
    fs::create_dir_all(nodes.parent().unwrap()).unwrap();
    fs::write(&nodes, roster).unwrap();
    for args in [
        &["node", "alias", "rename", "HOME", "W2"][..],
        &[
            "node",
            "alias",
            "set",
            "W2",
            "--node",
            "0123456789abcdef0123456789abcdef",
        ][..],
    ] {
        let refused = f.aida(args);
        assert!(!refused.status.success(), "{args:?} must refuse");
        let err = String::from_utf8_lossy(&refused.stderr).into_owned()
            + &String::from_utf8_lossy(&refused.stdout);
        assert!(err.contains("registered node id"), "{err}");
    }
}

/// A label is refused (on set and rename alike) when a stored spec_id or
/// agreed_id already has it as its middle segment, naming one example: the
/// labelled node's `BUG-<LABEL>-001` would otherwise display as an id that
/// parses back to the literal spec. Hand-written text that names the literal
/// id (a `--spec` trailer) therefore keeps naming the literal spec.
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
#[test]
fn node_alias_label_refused_when_a_stored_id_uses_it() {
    let f = Fixture::new();
    f.ok(&["add", "--title", "aliased defect", "--type", "bug"]);
    // Stored objects that arrived from elsewhere (an import, another clone):
    // written as files, since this clone may not mint global ids itself.
    let node = aida_core::work_binding::load(&f.store)
        .unwrap()
        .unwrap()
        .node_id
        .to_ascii_uppercase();
    let canonical = format!("BUG-{node}-001");
    let uuid = f.show_json(&[&canonical])["uuid"]
        .as_str()
        .unwrap()
        .to_string();
    let template = fs::read_to_string(
        aida_core::object_store::object_path(&f.store.join("objects"), &canonical).unwrap(),
    )
    .unwrap();
    for (spec_id, other_uuid, extra) in [
        ("BUG-JM-001", "00000000-0000-4000-8000-0000000000a1", ""),
        (
            "BUG-777",
            "00000000-0000-4000-8000-0000000000a2",
            "agreed_id: TASK-AK-007\n",
        ),
    ] {
        let path = aida_core::object_store::object_path(&f.store.join("objects"), spec_id).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = template
            .replace(&canonical, spec_id)
            .replace(&uuid, other_uuid)
            + extra;
        fs::write(&path, text).unwrap();
    }

    f.ok(&["node", "alias", "set", "HOME"]);
    for (args, example) in [
        (
            &[
                "node",
                "alias",
                "set",
                "jm",
                "--node",
                "0123456789abcdef0123456789abcdef",
            ][..],
            "BUG-JM-001",
        ),
        (&["node", "alias", "rename", "HOME", "JM"][..], "BUG-JM-001"),
        (
            &[
                "node",
                "alias",
                "set",
                "AK",
                "--node",
                "0123456789abcdef0123456789abcdef",
            ][..],
            "TASK-AK-007",
        ),
        (
            &["node", "alias", "rename", "HOME", "ak"][..],
            "TASK-AK-007",
        ),
    ] {
        let refused = f.aida(args);
        assert!(!refused.status.success(), "{args:?} must refuse");
        let err = String::from_utf8_lossy(&refused.stderr).into_owned()
            + &String::from_utf8_lossy(&refused.stdout);
        assert!(err.contains(example), "{args:?}: {err}");
    }
    let list = f.ok(&["node", "alias", "list"]);
    assert!(!list.contains("JM") && !list.contains("AK"), "{list}");

    // The literal id keeps naming the literal spec in hand-written text.
    let message = f.ok(&[
        "commit",
        "--dry-run",
        "--type",
        "fix",
        "--message",
        "literal spec",
        "--spec",
        "BUG-JM-001",
    ]);
    assert!(message.trim().ends_with("(BUG-JM-001)"), "{message}");
}

/// Spec-scope commands resolve an alias argument to the canonical id and act
/// on it: queue entries, claim leases, worktree branches and the `aida add`
/// confirmation. An unknown label stays a not-found that names it.
// trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude
#[test]
fn node_alias_spec_scope_commands_act_on_the_canonical_id() {
    let f = Fixture::new();
    f.ok(&["add", "--title", "aliased story", "--type", "story"]);
    let node = aida_core::work_binding::load(&f.store)
        .unwrap()
        .unwrap()
        .node_id
        .to_ascii_uppercase();
    let canonical = format!("STORY-{node}-001");
    f.ok(&["node", "alias", "set", "HOME"]);

    // `aida add` confirms with the display form; --canonical-ids keeps it raw.
    let added = f.ok(&["add", "--title", "second story", "--type", "story"]);
    assert!(added.starts_with("Added: STORY-HOME-002 - "), "{added}");
    let raw = f.ok(&[
        "--canonical-ids",
        "add",
        "--title",
        "third",
        "--type",
        "story",
    ]);
    assert!(
        raw.starts_with(&format!("Added: STORY-{node}-003 - ")),
        "{raw}"
    );

    // Queue: the alias and canonical forms name one entry, stored canonical.
    let queued = |f: &Fixture| -> Vec<String> {
        let list: serde_json::Value =
            serde_json::from_str(&f.ok(&["queue", "list", "--json", "--all"])).unwrap();
        list.as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["spec_id"].as_str().map(str::to_ascii_uppercase))
            .collect()
    };
    f.ok(&["queue", "add", "story-home-001", "--for", "advisor"]);
    assert_eq!(queued(&f), vec![canonical.clone()]);
    f.ok(&["queue", "add", &canonical, "--for", "advisor"]);
    assert_eq!(queued(&f), vec![canonical.clone()]);
    // The scope matcher resolves the alias: the dry-run plan for the alias is
    // the plan for the canonical id, and names that spec.
    let plan = |id: &str| {
        let out = f.aida(&["queue", "work", id, "--role", "implementer", "--dry-run"]);
        let text = String::from_utf8_lossy(&out.stderr).into_owned()
            + &String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains("doesn't match any queued entry"), "{text}");
        // The lines naming the spec (the session id is fresh per run).
        text.lines()
            .map(str::trim)
            .filter(|l| {
                [
                    "anchor:",
                    "scope:",
                    "skill:",
                    "branch:",
                    "worktree:",
                    "lease:",
                ]
                .iter()
                .any(|k| l.starts_with(k))
            })
            .map(str::to_ascii_uppercase)
            .collect::<Vec<_>>()
    };
    let alias_plan = plan("STORY-HOME-001");
    assert!(
        alias_plan.contains(&format!("SCOPE:   {canonical}")),
        "{alias_plan:?}"
    );
    assert_eq!(alias_plan, plan(&canonical));

    // Remove and done by alias act on the canonical entry.
    f.ok_as_advisor(&["queue", "remove", "story-home-001", "--for", "advisor"]);
    assert!(queued(&f).is_empty(), "{:?}", queued(&f));
    f.ok(&["queue", "add", "STORY-HOME-001", "--for", "advisor"]);
    assert_eq!(queued(&f), vec![canonical.clone()]);
    f.ok_as_advisor(&["queue", "done", "STORY-HOME-001"]);
    assert!(queued(&f).is_empty(), "{:?}", queued(&f));

    // Claim: the lease scope is the canonical id; unclaim by alias releases it.
    let claimed = f.ok(&["claim", "STORY-HOME-001"]);
    assert!(
        claimed.to_ascii_uppercase().contains(&canonical),
        "{claimed}"
    );
    let leases_dir = f.root.join(".aida/sessions");
    let scopes = || -> Vec<String> {
        fs::read_dir(&leases_dir)
            .map(|d| {
                d.flatten()
                    .filter_map(|e| fs::read_to_string(e.path()).ok())
                    .filter_map(|t| {
                        t.lines()
                            .find_map(|l| l.strip_prefix("scope = "))
                            .map(|s| s.trim_matches('"').to_ascii_uppercase())
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(scopes(), vec![canonical.clone()]);
    f.ok(&["unclaim", "story-home-001"]);
    assert!(scopes().is_empty(), "{:?}", scopes());

    // Unknown label: still a not-found that names what was typed.
    for args in [
        &["claim", "STORY-NOPE-001"][..],
        &[
            "queue",
            "work",
            "STORY-NOPE-001",
            "--role",
            "implementer",
            "--dry-run",
        ][..],
    ] {
        let out = f.aida(args);
        assert!(!out.status.success(), "{args:?} must refuse");
        let err = String::from_utf8_lossy(&out.stderr).into_owned()
            + &String::from_utf8_lossy(&out.stdout);
        assert!(err.contains("STORY-NOPE-001"), "{args:?}: {err}");
    }
}

/// Every file under `dir`, recursively.
fn files_under(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// `aida review record STORY-<LABEL>-NNN` keys the verdict file and the PR
/// ledger entry under the canonical id; the alias is never written.
// trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude
#[test]
fn node_alias_review_record_keys_the_verdict_under_the_canonical_id() {
    let f = Fixture::new();
    f.ok(&["add", "--title", "reviewed story", "--type", "story"]);
    let node = aida_core::work_binding::load(&f.store)
        .unwrap()
        .unwrap()
        .node_id
        .to_ascii_uppercase();
    let canonical = format!("STORY-{node}-001");
    f.ok(&["node", "alias", "set", "HOME"]);

    // A refusing verdict insists on `--pr`; pin the sha to HEAD so no forge
    // lookup is needed.
    let head = f.git(&["rev-parse", "HEAD"]);
    let out = f.ok(&[
        "review",
        "record",
        "story-home-001",
        "--verdict",
        "request-changes",
        "--pr",
        "1",
        "--sha",
        &head,
        "--summary",
        "fixture refusal",
    ]);
    assert!(out.to_ascii_uppercase().contains(&canonical), "{out}");

    let verdicts = f.root.join(".aida/review-verdicts");
    let verdict_file = verdicts.join(format!("{canonical}.json"));
    assert!(
        verdict_file.is_file(),
        "no verdict under the canonical id: {:?}",
        files_under(&verdicts)
    );
    // The PR ledger and every other record name the canonical id, never the
    // alias.
    let written = files_under(&f.root.join(".aida"));
    let ledger_hits: Vec<_> = written
        .iter()
        .filter(|p| {
            fs::read_to_string(p)
                .map(|t| t.to_ascii_uppercase().contains(&canonical))
                .unwrap_or(false)
        })
        .filter(|p| **p != verdict_file)
        .collect();
    assert!(
        !ledger_hits.is_empty(),
        "no PR ledger entry names {canonical}: {written:?}"
    );
    for path in &written {
        let text = fs::read_to_string(path).unwrap_or_default();
        assert!(
            !text.to_ascii_uppercase().contains("STORY-HOME-001"),
            "{} records the alias:\n{text}",
            path.display()
        );
    }
    // Reading back by alias finds the same verdict.
    let shown = f.ok(&["review", "verdict", "STORY-HOME-001", "--json"]);
    assert!(shown.to_ascii_uppercase().contains(&canonical), "{shown}");
}
