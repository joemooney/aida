// Binary-driving e2e suite; Linux-only like its siblings (BUG-688).
#![cfg(target_os = "linux")]
//! BUG-1934 SKETCH v2: a REAL bubblewrap-namespaced presenter. An agent
//! harness's sandbox runs each tool call in its own PID namespace, where the
//! process that a grant is bound to is never visible. The grant's session
//! key, adopted by the shell that entered the role (`aida role adopt-key`,
//! what the `role enter` eval text runs), is inherited into the namespace and
//! satisfies the binding; the same command without the key is refused with
//! the sandbox sentence. Skipped when bwrap is absent or cannot create user
//! and PID namespaces on this host.
// trace:BUG-1934 | ai:claude

use std::path::PathBuf;
use std::process::{Command, Output};

mod support;

const SECRET: &str = "4f1d9c0b7a6e5d4c3b2a19087f6e5d4c3b2a19087f6e5d4c3b2a19087f6e5d4c";

/// `key:<sha256(SECRET)>`, as `seat_authority::key_token` writes it.
fn key_token() -> String {
    let out = Command::new("sha256sum")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(SECRET.as_bytes())?;
            child.wait_with_output()
        })
        .expect("sha256sum runs");
    let hex = String::from_utf8(out.stdout).unwrap();
    format!("key:{}", hex.split_whitespace().next().unwrap())
}

fn bwrap_usable() -> bool {
    Command::new("bwrap")
        .args(BWRAP_ARGS)
        .arg("true")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The namespaces Claude Code's sandbox unshares (user, pid, ipc, uts, net,
/// cgroup) and its `--new-session --die-with-parent`.
const BWRAP_ARGS: &[&str] = &[
    "--unshare-all",
    "--new-session",
    "--die-with-parent",
    "--ro-bind",
    "/",
    "/",
    "--dev",
    "/dev",
    "--proc",
    "/proc",
    "--tmpfs",
    "/dev/shm",
];

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    grant: String,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(repo.join(".aida")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        repo.join(".aida").join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let grant = support::grant_seat_for(&home, &repo, "bwrap-fixture", "reviewer", &[]);
    // Bound to the session key only: no process of the namespace can match.
    let path = home
        .join(".aida")
        .join("session-grants")
        .join(format!("{grant}.json"));
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["binding"] = serde_json::json!([key_token()]);
    std::fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    Fixture {
        _tmp: tmp,
        repo,
        home,
        grant,
    }
}

/// Run `script` under `sh -c` with the fixture's env; `$AIDA` is the binary.
fn sh(fx: &Fixture, script: &str) -> Output {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script).current_dir(&fx.repo);
    for key in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_PURPOSE",
        "AIDA_AGENT_NAME",
        "AIDA_AGENT_TYPE",
        "AIDA_SESSION_ID",
    ] {
        cmd.env_remove(key);
    }
    cmd.env("AIDA", env!("CARGO_BIN_EXE_aida"))
        .env("HOME", &fx.home)
        .env("AIDA_HOME", &fx.home)
        .env("AIDA_USER", "bwrap-fixture")
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_GRANT", &fx.grant)
        .output()
        .expect("run sh")
}

fn status_in_bwrap(script_prefix: &str) -> String {
    let bwrap: Vec<String> = BWRAP_ARGS.iter().map(|a| format!("'{a}'")).collect();
    format!(
        "{script_prefix}\nexec bwrap {} --bind \"$HOME/..\" \"$HOME/..\" \"$AIDA\" role status --json",
        bwrap.join(" ")
    )
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "role status --json: {e}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn bwrap_presenter_holding_the_adopted_key_is_valid() {
    if !bwrap_usable() {
        eprintln!("SKIP: bwrap is absent or cannot unshare namespaces here");
        return;
    }
    let fx = fixture();
    // The shell adopts the key exactly as the role-enter eval text does,
    // then a namespaced child presents the grant.
    let adopt = format!(
        "{{ set +x; }} 2>/dev/null\n\"$AIDA\" role adopt-key '{}' <<'AIDA_GRANT_KEY'\n{SECRET}\nAIDA_GRANT_KEY",
        fx.grant
    );
    let out = sh(&fx, &status_in_bwrap(&adopt));
    let status = json(&out);
    assert_eq!(status["grant"]["valid"], true, "{status}");
    assert_eq!(status["grant"]["via"], "key", "{status}");
    assert!(
        status["harness_pid"].is_null(),
        "no stable pid in a sandbox: {status}"
    );
}

#[test]
fn bwrap_presenter_without_the_key_is_refused_with_the_sandbox_sentence() {
    if !bwrap_usable() {
        eprintln!("SKIP: bwrap is absent or cannot unshare namespaces here");
        return;
    }
    let fx = fixture();
    let out = sh(&fx, &status_in_bwrap(":"));
    let status = json(&out);
    assert_eq!(status["grant"]["valid"], false, "{status}");
    assert_eq!(status["grant"]["via"], "none", "{status}");
    let why = status["grant"]["rejection"].as_str().unwrap_or_default();
    assert!(why.contains("sandboxed process namespace"), "{why}");
    assert!(why.contains("aida role enter reviewer"), "{why}");
}

#[test]
fn adopt_key_refuses_a_wrong_secret() {
    let fx = fixture();
    let out = sh(
        &fx,
        &format!(
            "\"$AIDA\" role adopt-key '{}' <<'AIDA_GRANT_KEY'\nnot-the-secret\nAIDA_GRANT_KEY",
            fx.grant
        ),
    );
    assert!(!out.status.success());
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(err.contains("does not belong"), "{err}");
}
