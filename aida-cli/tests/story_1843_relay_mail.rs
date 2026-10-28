#![cfg(target_os = "linux")]
//! The shipped `aida relay-mail` CLI against a stub relay controller: a shell
//! process holding its run lock the way relay.sh does. No model is launched.
// trace:STORY-1843 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let fx = Self {
            _tmp: tmp,
            root,
            home,
        };
        fx.checked(Command::new("git").args(["init", "-q", "-b", "main"]));
        std::fs::write(
            fx.root.join(".git/aida-allocation.toml"),
            "version=1\nauthoritative=true\n",
        )
        .unwrap();
        fx.aida_ok(&["init", "--no-skills", "--no-hooks"]);
        fx
    }

    fn env(&self, cmd: &mut Command) {
        for (key, _) in std::env::vars() {
            if [
                "AIDA_",
                "GIT_",
                "CLAUDE",
                "CODEX",
                "ANTIGRAVITY",
                "GEMINI",
                "XDG_",
            ]
            .iter()
            .any(|p| key.starts_with(p))
            {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(&self.root)
            .env("HOME", &self.home)
            .env("AIDA_HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_OS_WRAP", "0")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid");
    }

    fn checked(&self, cmd: &mut Command) -> Output {
        self.env(cmd);
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{cmd:?}: {}", text(&out));
        out
    }

    fn aida(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
        cmd.args(args);
        self.env(&mut cmd);
        cmd.output().unwrap()
    }

    fn aida_ok(&self, args: &[&str]) -> String {
        let out = self.aida(args);
        assert!(out.status.success(), "aida {args:?}: {}", text(&out));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_str(&self.aida_ok(args)).unwrap()
    }

    fn send(&self, to: &str, subject: &str) -> Output {
        self.aida(&[
            "mailbox",
            "send",
            "--to",
            to,
            "--from",
            "fixture-operator",
            "--subject",
            subject,
            "body",
        ])
    }
}

fn text(out: &Output) -> String {
    format!(
        "status={:?} stdout={} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A stub relay controller: takes an exclusive flock on its run lock, writes
/// its own pid on the first line, then idles holding the lock (relay.sh's
/// run-lock shape).
fn stub_controller(lock: &Path) -> Child {
    let child = Command::new("sh")
        .args([
            "-c",
            r#"exec 9>"$1"; flock -x 9; echo $$ >&9; exec sleep 60"#,
            "sh",
        ])
        .arg(lock)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::fs::read_to_string(lock).unwrap_or_default().trim() != child.id().to_string() {
        assert!(
            std::time::Instant::now() < deadline,
            "stub controller never locked"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child
}

#[test]
fn story_1843_relay_mail_lifetime_through_the_cli() {
    let fx = Fixture::new();
    let probe = fx.json(&["relay-mail", "probe", "--json"]);
    let protocol = probe["protocol"].as_str().unwrap().to_string();
    assert_eq!(protocol, "aida-relay-mail/1");

    let lock = fx.root.join("relay.lock");
    let mut controller = stub_controller(&lock);
    let pid = controller.id().to_string();
    let lock_arg = lock.to_str().unwrap();

    // A mismatched protocol refuses before anything is reserved.
    let refused = fx.aida(&[
        "relay-mail",
        "bind",
        "--label",
        "relay-fx",
        "--controller-pid",
        &pid,
        "--run-lock",
        lock_arg,
        "--protocol",
        "aida-relay-mail/0",
    ]);
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(!fx.root.join(".aida/agents/handles/relay-fx.toml").exists());

    // Unknown before bind; accepted in the gap after it.
    assert!(!fx.send("relay-fx", "before-bind").status.success());
    let bound = fx.json(&[
        "relay-mail",
        "bind",
        "--label",
        "relay-fx",
        "--controller-pid",
        &pid,
        "--run-lock",
        lock_arg,
        "--protocol",
        &protocol,
        "--json",
    ]);
    let run = bound["run"].as_str().unwrap().to_string();
    let gap = fx.send("relay-fx", "gap-mail");
    assert!(gap.status.success(), "{}", text(&gap));
    assert!(text(&gap).contains("between sessions"), "{}", text(&gap));

    // Two rotations, one run.
    for generation in [2, 3] {
        let child = fx.json(&["relay-mail", "child-bind", "--run", &run, "--json"]);
        assert_eq!(child["run"], run.as_str());
        assert_eq!(child["generation"], generation);
        assert!(child["launch_token"]
            .as_str()
            .is_some_and(|t| !t.is_empty()));
        assert_eq!(child["launch_token_env"], "AIDA_HANDLE_LAUNCH_TOKEN");
    }

    // End is recorded once; mail is refused afterwards.
    let end = |reason: &str| {
        fx.json(&[
            "relay-mail",
            "end",
            "--run",
            &run,
            "--controller-pid",
            &pid,
            "--reason",
            reason,
            "--json",
        ])
    };
    let first = end("exit");
    assert_eq!(first["recorded_now"], true);
    assert_eq!(first["terminal_generation"], 4);
    let again = end("stop");
    assert_eq!(again["recorded_now"], false);
    assert_eq!(again["reason"], "exit");
    assert!(!fx.send("relay-fx", "after-end").status.success());
    controller.kill().unwrap();
    controller.wait().unwrap();

    // Crash path: a controller killed without `end` is ended by maintenance.
    let lock2 = fx.root.join("relay2.lock");
    let mut crashed = stub_controller(&lock2);
    let pid2 = crashed.id().to_string();
    let run2 = fx.json(&[
        "relay-mail",
        "bind",
        "--label",
        "relay-crash",
        "--controller-pid",
        &pid2,
        "--run-lock",
        lock2.to_str().unwrap(),
        "--protocol",
        &protocol,
        "--json",
    ])["run"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(fx.send("relay-crash", "gap").status.success());
    crashed.kill().unwrap();
    crashed.wait().unwrap();
    assert!(!fx.send("relay-crash", "dead-controller").status.success());
    let gc = fx.aida_ok(&["agent", "gc"]);
    assert!(gc.contains("Relay runs: recorded the end of 1 run"), "{gc}");
    let after = fx.aida(&["relay-mail", "child-bind", "--run", &run2]);
    assert!(!after.status.success());
    assert!(text(&after).contains("ended"), "{}", text(&after));
}
