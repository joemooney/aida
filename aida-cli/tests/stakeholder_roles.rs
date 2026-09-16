#![cfg(target_os = "linux")]
//! STORY-1110: guest/requester are least-privilege stakeholder roles.

use std::path::Path;
use std::process::Command;

fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    cmd.env("AIDA_AGENT_OUTPUT", "0");
    cmd.env_remove("AIDA_SESSION_ROLE");
    cmd
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn init_repo() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize tempdir");
    let repo = base_dir.join("repo");
    let home = base_dir.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
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
    assert!(
        init.status.success(),
        "aida init failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );
    (base, repo, home)
}

fn is_spec_id(t: &str) -> bool {
    let mut parts = t.splitn(2, '-');
    match (parts.next(), parts.next()) {
        (Some(prefix), Some(num)) => {
            !prefix.is_empty()
                && prefix.chars().all(|c| c.is_ascii_uppercase())
                && !num.is_empty()
                && num.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

fn parse_spec_id(out: &str) -> String {
    out.split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| is_spec_id(t))
        .unwrap_or_else(|| panic!("could not parse spec id from:\n{out}"))
        .to_string()
}

#[test]
fn guest_refuses_write_commands() {
    let (_base, repo, home) = init_repo();

    let add = aida(&repo, &home)
        .env("AIDA_SESSION_ROLE", "guest")
        .args(["add", "--title", "please build this", "--type", "bug"])
        .output()
        .expect("run guest add");
    assert!(!add.status.success(), "guest add must be refused");
    let err = String::from_utf8_lossy(&add.stderr);
    assert!(err.contains("AIDA_SESSION_ROLE=guest"), "{err}");
    assert!(err.contains("least-privilege"), "{err}");
}

#[test]
fn role_list_surfaces_stakeholder_personas_without_role_files() {
    let (_base, repo, home) = init_repo();

    let list = aida(&repo, &home)
        .args(["role", "list"])
        .output()
        .expect("run role list");
    assert!(
        list.status.success(),
        "role list failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&list.stdout),
        String::from_utf8_lossy(&list.stderr)
    );
    let out = String::from_utf8_lossy(&list.stdout);

    // trace:TASK-1237 | ai:codex
    assert!(out.contains("Stakeholder personas:"), "{out}");
    assert!(out.contains("guest"), "{out}");
    assert!(out.contains("requester"), "{out}");
    assert!(out.contains("least-privilege"), "{out}");
    assert!(out.contains("not a build seat"), "{out}");
    assert!(out.contains("AIDA_SESSION_ROLE=guest"), "{out}");
    assert!(out.contains("AIDA_SESSION_ROLE=requester"), "{out}");
}

#[test]
fn companion_refuses_to_drive_a_drain_via_burndown_run() {
    // STORY-1133 (reviewer gap): a companion is a NON-authoritative instance of
    // a driver role — it may read/converse/draft/advise but must NEVER drive a
    // drain. `burndown run` is a drain-start path and was missing the companion
    // gate that `queue work --auto-complete` already had. Even WITH advisor
    // authority (AIDA_SESSION_ROLE=advisor), the companion INSTANCE is refused —
    // and even the safe `--dry-run` gate-probe. trace:STORY-1133 | ai:claude
    let (_base, repo, home) = init_repo();
    let run = aida(&repo, &home)
        .env("AIDA_ROLE_INSTANCE", "companion")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args(["burndown", "run", "--dry-run"])
        .output()
        .expect("run companion burndown");
    assert!(
        !run.status.success(),
        "companion burndown run must be refused"
    );
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(
        err.contains("companion sessions cannot drive drains"),
        "{err}"
    );
}

#[test]
fn requester_adds_allowed_intake_as_draft_with_tag() {
    let (_base, repo, home) = init_repo();

    let add = aida(&repo, &home)
        .env("AIDA_SESSION_ROLE", "requester")
        .args([
            "add",
            "--title",
            "external request",
            "--type",
            "bug",
            "--status",
            "approved",
        ])
        .output()
        .expect("run requester add");
    assert!(
        add.status.success(),
        "requester add failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&add.stdout),
        String::from_utf8_lossy(&add.stderr)
    );
    let spec = parse_spec_id(&String::from_utf8_lossy(&add.stdout));

    let shown = aida(&repo, &home)
        .args(["show", &spec, "--full"])
        .output()
        .expect("show requester intake");
    assert!(shown.status.success(), "show failed");
    let out = String::from_utf8_lossy(&shown.stdout);
    assert!(out.contains("Status:") && out.contains("Draft"), "{out}");
    assert!(out.contains("Type: Bug"), "{out}");
    assert!(out.contains("intake:requester"), "{out}");
}

#[test]
fn requester_refuses_non_add_writes_and_build_loop_routing() {
    let (_base, repo, home) = init_repo();

    let add = aida(&repo, &home)
        .env("AIDA_SESSION_ROLE", "requester")
        .args(["add", "--title", "route me", "--type", "change-request"])
        .output()
        .expect("run requester add");
    assert!(add.status.success(), "requester add should succeed");
    let spec = parse_spec_id(&String::from_utf8_lossy(&add.stdout));

    let edit = aida(&repo, &home)
        .env("AIDA_SESSION_ROLE", "requester")
        .args(["edit", &spec, "--status", "approved"])
        .output()
        .expect("run requester edit");
    assert!(!edit.status.success(), "requester edit must be refused");
    let edit_err = String::from_utf8_lossy(&edit.stderr);
    assert!(
        edit_err.contains("AIDA_SESSION_ROLE=requester"),
        "{edit_err}"
    );

    let queue = aida(&repo, &home)
        .env("AIDA_SESSION_ROLE", "advisor")
        .args(["queue", "add", &spec, "--for", "requester"])
        .output()
        .expect("run queue add requester target");
    assert!(
        !queue.status.success(),
        "requester must not be a queue target"
    );
    let queue_err = String::from_utf8_lossy(&queue.stderr);
    assert!(
        queue_err.contains("not a build-loop queue target"),
        "{queue_err}"
    );
}
