// BUG-688: gated to Linux like every other binary-driving e2e suite here.
// trace:BUG-688 | ai:claude
#![cfg(target_os = "linux")]
//! BUG-1952: `aida session end` run with its cwd INSIDE the worktree it ends
//! must resolve every runtime-state path against the MAIN checkout. Before the
//! fix, `find_project_root()` returned the worktree, the `.aida` runtime
//! symlinks were stripped, and the mcp-claim reap, manifest delete, BUG-483
//! peer guard and pool checks then ran against dead worktree paths: the claim
//! and manifest leaked, and a peer's shared worktree was force-removed.
//!
//! Driven through the real `aida` binary against a throwaway repo with an
//! isolated HOME, because the defect lives in the process-cwd resolution and
//! only an on-disk assertion after a real end shows which root was used.
// trace:BUG-1952 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

/// Pin every ambient input a spawned `git` (directly or under `aida`) reads:
/// the temp HOME, no global/system git config, and no inherited `GIT_DIR` /
/// `GIT_WORK_TREE` / `GIT_INDEX_FILE` (set when tests run under a git hook),
/// which would otherwise point the fixture's git at the developer's checkout.
fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home);
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    for k in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        cmd.env_remove(k);
    }
}

/// The isolated HOME `project` creates beside the fixture repo.
fn home_of(repo: &Path) -> PathBuf {
    repo.parent()
        .expect("fixture repo has a parent")
        .join("home")
}

fn aida(cwd: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(cwd);
    isolate(&mut cmd, home);
    cmd.env("AIDA_TELEMETRY", "0");
    for k in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_HEADLESS_VENDOR",
        "AIDA_AGENT_MODEL",
        "AIDA_AGENT_CMD",
        "AIDA_SESSION_ID",
        "AIDA_STORE_PATH",
        "AIDA_PROJECT_ROOT",
    ] {
        cmd.env_remove(k);
    }
    cmd
}

fn git(repo: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    isolate(&mut cmd, &home_of(repo));
    let status = cmd.current_dir(repo).args(args).status().expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn show(o: &Output) -> String {
    format!(
        "exit {:?}\n--- stderr ---\n{}\n--- stdout ---\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stderr),
        String::from_utf8_lossy(&o.stdout)
    )
}

fn is_spec_id(token: &str) -> bool {
    token.split_once('-').is_some_and(|(kind, num)| {
        !kind.is_empty()
            && !num.is_empty()
            && kind.chars().all(|c| c.is_ascii_uppercase())
            && num.chars().all(|c| c.is_ascii_digit())
    })
}

/// An initialized AIDA project plus an isolated HOME. With `pool` false every
/// worktree is a fresh non-pool tree; with it true `queue work` hands out a
/// warm-pool tree.
fn project(base_dir: &Path, pool: bool) -> (PathBuf, PathBuf) {
    let repo = base_dir.join("repo");
    let home = base_dir.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let init = aida(&repo, &home)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .expect("run aida init");
    assert!(init.status.success(), "aida init failed: {}", show(&init));

    let cfg = repo.join(".aida").join("config.toml");
    let body = std::fs::read_to_string(&cfg).expect("read scaffolded config");
    const SCAFFOLD: &str = "[worktree_pool]\nenabled = true\n";
    assert!(body.contains(SCAFFOLD), "init scaffold changed:\n{body}");
    std::fs::write(
        &cfg,
        body.replace(SCAFFOLD, &format!("[worktree_pool]\nenabled = {pool}\n")),
    )
    .unwrap();
    (repo, home)
}

fn add_spec(repo: &Path, home: &Path, title: &str) -> String {
    let add = aida(repo, home)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env(
            "AIDA_SESSION_GRANT",
            support::ensure_seat(home, repo, "advisor", &[]).expect("fixture repo is initialized"),
        )
        .args([
            "add", "--type", "task", "--status", "approved", "--title", title,
        ])
        .output()
        .expect("run aida add");
    assert!(add.status.success(), "aida add failed: {}", show(&add));
    let out = String::from_utf8_lossy(&add.stdout);
    out.split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| is_spec_id(t))
        .unwrap_or_else(|| panic!("no spec id in:\n{out}"))
        .to_string()
}

struct Lease {
    id: String,
    worktree: PathBuf,
}

fn sessions_dir(repo: &Path) -> PathBuf {
    repo.join(".aida").join("sessions")
}

/// The session lease (`<id>.toml`, not a manifest or mcp-claim) owning `spec`.
fn lease_for(repo: &Path, spec: &str) -> Lease {
    for entry in std::fs::read_dir(sessions_dir(repo)).expect("sessions dir") {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".toml")
            || name.ends_with(".manifest.toml")
            || name.starts_with("mcp-claim.")
        {
            continue;
        }
        let Ok(v) = std::fs::read_to_string(&path)
            .unwrap()
            .parse::<toml::Value>()
        else {
            continue;
        };
        if v.get("scope").and_then(|s| s.as_str()) == Some(spec) {
            return Lease {
                id: v["id"].as_str().unwrap().to_string(),
                worktree: PathBuf::from(v["worktree_path"].as_str().unwrap()),
            };
        }
    }
    panic!("no lease for {spec} under {}", sessions_dir(repo).display());
}

/// Start a worktree session for `spec`, then plant the per-session runtime
/// state `session end` must reap from the MAIN root: an MCP work-claim for the
/// scope and a planned-cluster manifest for the session.
fn start_session(repo: &Path, home: &Path, spec: &str) -> Lease {
    let run = aida(repo, home)
        .args(["queue", "work", spec, "--no-pull", "--no-launch"])
        .output()
        .expect("run queue work");
    assert!(run.status.success(), "queue work failed: {}", show(&run));
    let lease = lease_for(repo, spec);
    assert!(lease.worktree.is_dir(), "worktree not created");
    // The worktree reaches the main checkout's sessions dir only via the
    // runtime symlink session start installs — the state BUG-1952 strips.
    assert!(
        lease.worktree.join(".aida").join("sessions").is_symlink(),
        "fixture precondition: the worktree's .aida/sessions must be a symlink"
    );
    std::fs::write(mcp_claim(repo, spec), "spec_id = \"x\"\n").unwrap();
    let manifest = manifest(repo, &lease.id);
    if !manifest.exists() {
        std::fs::write(&manifest, "").unwrap();
    }
    lease
}

fn mcp_claim(repo: &Path, spec: &str) -> PathBuf {
    sessions_dir(repo).join(format!("mcp-claim.{}.toml", spec.to_ascii_lowercase()))
}

fn manifest(repo: &Path, id: &str) -> PathBuf {
    sessions_dir(repo).join(format!("{id}.manifest.toml"))
}

fn lease_file(repo: &Path, id: &str) -> PathBuf {
    sessions_dir(repo).join(format!("{id}.toml"))
}

fn session_end(cwd: &Path, home: &Path, id: &str) -> Output {
    // stdout is piped (non-TTY), so `session end` emits its shell-eval block —
    // whose `cd` names the root it resolved.
    aida(cwd, home)
        .args(["session", "end", id, "--yes", "--skip-ci"])
        .output()
        .expect("run session end")
}

fn registered_worktrees(repo: &Path) -> Vec<PathBuf> {
    let mut cmd = Command::new("git");
    isolate(&mut cmd, &home_of(repo));
    let out = cmd
        .current_dir(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("git worktree list");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect()
}

/// What an ended session leaves behind, as seen from the main checkout.
#[derive(Debug, PartialEq)]
struct EndState {
    lease_present: bool,
    mcp_claim_present: bool,
    manifest_present: bool,
    worktree_dir_present: bool,
    worktree_registered: bool,
}

fn end_state(repo: &Path, spec: &str, lease: &Lease) -> EndState {
    EndState {
        lease_present: lease_file(repo, &lease.id).exists(),
        mcp_claim_present: mcp_claim(repo, spec).exists(),
        manifest_present: manifest(repo, &lease.id).exists(),
        worktree_dir_present: lease.worktree.exists(),
        worktree_registered: registered_worktrees(repo).contains(&lease.worktree),
    }
}

const FULLY_ENDED: EndState = EndState {
    lease_present: false,
    mcp_claim_present: false,
    manifest_present: false,
    worktree_dir_present: false,
    worktree_registered: false,
};

// Acceptance 1 + 2: ending from INSIDE the worktree reaps the mcp-claim and the
// manifest from the main root and removes the worktree, and the result is
// identical to ending a sibling session from the main checkout.
#[test]
fn session_end_inside_worktree_resolves_main_root_and_matches_main_checkout() {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize");
    let (repo, home) = project(&base_dir, false);

    let spec_in = add_spec(&repo, &home, "end me from inside");
    let spec_main = add_spec(&repo, &home, "end me from main");
    let in_wt = start_session(&repo, &home, &spec_in);
    let from_main = start_session(&repo, &home, &spec_main);

    let out = session_end(&in_wt.worktree, &home, &in_wt.id);
    assert!(
        out.status.success(),
        "end from worktree failed: {}",
        show(&out)
    );
    assert_eq!(
        end_state(&repo, &spec_in, &in_wt),
        FULLY_ENDED,
        "ending from inside the worktree left state behind: {}",
        show(&out)
    );
    // The shell-eval `cd` names the resolved root: the main checkout, not the
    // worktree that was just removed.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("cd '{}'", repo.display())),
        "expected a cd back to the main root {}: {}",
        repo.display(),
        show(&out)
    );

    let out = session_end(&repo, &home, &from_main.id);
    assert!(out.status.success(), "end from main failed: {}", show(&out));
    assert_eq!(
        end_state(&repo, &spec_main, &from_main),
        end_state(&repo, &spec_in, &in_wt),
        "ending from inside the worktree must match ending from the main checkout"
    );
}

// Acceptance 2: the BUG-483 shared-worktree guard still fires when the end runs
// from inside the worktree — the peer's lease is found under the main root, so
// the tree is left standing and the peer lease survives.
#[test]
fn bug_483_peer_guard_fires_from_inside_worktree() {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize");
    let (repo, home) = project(&base_dir, false);

    let spec = add_spec(&repo, &home, "shared tree");
    let lease = start_session(&repo, &home, &spec);

    // A peer lease on the same worktree (the `aida agent new` shape, BUG-416).
    let peer_id = "feedfacecafe";
    let body = std::fs::read_to_string(lease_file(&repo, &lease.id)).unwrap();
    let mut peer: toml::Value = body.parse().unwrap();
    peer["id"] = toml::Value::String(peer_id.to_string());
    std::fs::write(lease_file(&repo, peer_id), toml::to_string(&peer).unwrap()).unwrap();

    let out = session_end(&lease.worktree, &home, &lease.id);
    assert!(out.status.success(), "end failed: {}", show(&out));
    assert!(
        !lease_file(&repo, &lease.id).exists(),
        "the ended lease must be deleted: {}",
        show(&out)
    );
    assert!(
        lease_file(&repo, peer_id).exists(),
        "the peer lease must survive: {}",
        show(&out)
    );
    assert!(
        lease.worktree.is_dir() && registered_worktrees(&repo).contains(&lease.worktree),
        "the shared worktree was removed out from under its peer: {}",
        show(&out)
    );
}

// Acceptance 1 (pool checks): a pooled tree ended from INSIDE itself is
// recognised as a pool tree against the main root's `.aida/worktree-pool`
// registry (never linked into the worktree) and returned to the pool — the
// directory persists and the entry goes idle — instead of being deleted. The
// pool evidence is asserted first so it stands on its own.
#[test]
fn pooled_tree_ended_from_inside_is_returned_not_removed() {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize");
    let (repo, home) = project(&base_dir, true);

    let spec = add_spec(&repo, &home, "pooled");
    let lease = start_session(&repo, &home, &spec);
    assert!(
        aida_core::worktree_pool::is_pool_worktree(&repo, &lease.worktree),
        "fixture precondition: queue work must hand out a pool tree"
    );

    let out = session_end(&lease.worktree, &home, &lease.id);
    assert!(out.status.success(), "end failed: {}", show(&out));
    assert!(
        lease.worktree.is_dir(),
        "the pooled tree was deleted instead of returned: {}",
        show(&out)
    );
    let pool = aida_core::worktree_pool::read_state(&repo).expect("pool registry");
    let target = lease.worktree.canonicalize().unwrap();
    let entry = pool
        .entries
        .iter()
        .find(|e| e.path.canonicalize().ok().as_ref() == Some(&target))
        .unwrap_or_else(|| panic!("pool entry gone: {}", show(&out)));
    assert!(
        !entry.leased && entry.owner_pid.is_none(),
        "the returned tree must be idle in the pool: {entry:?}\n{}",
        show(&out)
    );
    assert!(
        !lease_file(&repo, &lease.id).exists(),
        "lease not deleted: {}",
        show(&out)
    );
    assert!(
        !mcp_claim(&repo, &spec).exists(),
        "mcp-claim leaked: {}",
        show(&out)
    );
    assert!(
        !manifest(&repo, &lease.id).exists(),
        "manifest leaked: {}",
        show(&out)
    );
}
