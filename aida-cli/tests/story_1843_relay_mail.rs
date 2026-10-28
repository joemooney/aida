#![cfg(target_os = "linux")]
//! The shipped `aida relay-mail` CLI against a stub relay controller: an
//! idle child process whose run lock this test holds, with the child's pid on
//! its first line (relay.sh's run-lock shape). No model is launched, and
//! nothing waits on another process to reach a state.
// trace:STORY-1843 | ai:claude

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

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

/// A stub relay controller: an idle child process, plus its run lock held
/// exclusively (by this test process) with the child's pid on the first
/// line. Both are in place before this returns, so nothing polls.
struct Controller {
    child: Child,
    _lock: std::fs::File,
}

impl Controller {
    fn start(lock: &Path) -> Self {
        use fs2::FileExt;
        let child = Command::new("sleep").arg("300").spawn().unwrap();
        let mut file = std::fs::File::create(lock).unwrap();
        writeln!(file, "{}", child.id()).unwrap();
        file.lock_exclusive().unwrap();
        Self { child, _lock: file }
    }

    fn pid(&self) -> String {
        self.child.id().to_string()
    }

    /// SIGKILL and reap: the pid is provably free afterwards.
    fn kill(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Fixture {
    /// Run aida with `input` on stdin.
    fn aida_stdin(&self, args: &[&str], input: &str) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        self.env(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn json_stdin(&self, args: &[&str], input: &str) -> serde_json::Value {
        let out = self.aida_stdin(args, input);
        assert!(out.status.success(), "aida {args:?}: {}", text(&out));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn file(&self, path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap()
    }
}

#[test]
fn story_1843_relay_mail_lifetime_through_the_cli() {
    let fx = Fixture::new();
    let probe = fx.json(&["relay-mail", "probe", "--json"]);
    let protocol = probe["protocol"].as_str().unwrap().to_string();
    assert_eq!(protocol, "aida-relay-mail/1");

    let lock = fx.root.join("relay.lock");
    let controller = Controller::start(&lock);
    let pid = controller.pid();
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
    let capability = bound["capability"].as_str().unwrap().to_string();
    assert_eq!(capability.len(), 64);
    let record = fx.root.join(format!(".aida/agents/relay-runs/{run}.json"));
    assert!(!String::from_utf8_lossy(&fx.file(&record)).contains(&capability));
    let gap = fx.send("relay-fx", "gap-mail");
    assert!(gap.status.success(), "{}", text(&gap));
    assert!(text(&gap).contains("between sessions"), "{}", text(&gap));

    // Two rotations, one run: the capability on stdin, then from a 0600 file.
    let cap_file = fx.root.join("relay.cap");
    std::fs::write(&cap_file, format!("{capability}\n")).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cap_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let cap_arg = cap_file.to_str().unwrap();
    let rotations = [
        fx.json_stdin(
            &["relay-mail", "child-bind", "--run", &run, "--json"],
            &capability,
        ),
        fx.json(&[
            "relay-mail",
            "child-bind",
            "--run",
            &run,
            "--cap-file",
            cap_arg,
            "--json",
        ]),
    ];
    for (child, generation) in rotations.iter().zip([2, 3]) {
        assert_eq!(child["run"], run.as_str());
        assert_eq!(child["generation"], generation);
        assert!(child["launch_token"]
            .as_str()
            .is_some_and(|t| !t.is_empty()));
        assert_eq!(child["launch_token_env"], "AIDA_HANDLE_LAUNCH_TOKEN");
    }

    // A caller without the capability is refused and writes nothing: a
    // token child-bind, a native-session child-bind, and an end.
    let wrong = "0".repeat(64);
    let handle = fx.root.join(".aida/agents/handles/relay-fx.toml");
    let (record_before, handle_before) = (fx.file(&record), fx.file(&handle));
    for args in [
        vec!["relay-mail", "child-bind", "--run", &run],
        vec![
            "relay-mail",
            "child-bind",
            "--run",
            &run,
            "--native-session",
            "intruder",
        ],
        vec!["relay-mail", "end", "--run", &run],
    ] {
        for input in [wrong.as_str(), ""] {
            let out = fx.aida_stdin(&args, input);
            assert!(!out.status.success(), "{args:?}: {}", text(&out));
        }
    }
    assert_eq!(fx.file(&record), record_before);
    assert_eq!(fx.file(&handle), handle_before);

    // End is recorded once; mail is refused afterwards.
    let end = |reason: &str| {
        fx.json_stdin(
            &[
                "relay-mail",
                "end",
                "--run",
                &run,
                "--reason",
                reason,
                "--json",
            ],
            &capability,
        )
    };
    let first = end("exit");
    assert_eq!(first["recorded_now"], true);
    assert_eq!(first["terminal_generation"], 4);
    let again = end("stop");
    assert_eq!(again["recorded_now"], false);
    assert_eq!(again["reason"], "exit");
    assert!(!fx.send("relay-fx", "after-end").status.success());
    // An end notice to the advisor is still owed (this project has no
    // advisor reader), so the ended run stays a live record until it is
    // delivered; only then is it archived.
    assert!(fx
        .root
        .join(format!(".aida/agents/relay-runs/{run}.json"))
        .exists());

    // Reusing the label: the ended run still has its gap mail unread, so a
    // bind without a choice refuses with the exact count; --handover binds.
    let reuse_args = |extra: &[&'static str]| {
        let mut args = vec![
            "relay-mail",
            "bind",
            "--label",
            "relay-fx",
            "--controller-pid",
            pid.as_str(),
            "--run-lock",
            lock_arg,
            "--protocol",
            protocol.as_str(),
            "--json",
        ];
        args.extend_from_slice(extra);
        args.into_iter().map(str::to_string).collect::<Vec<_>>()
    };
    let refused = fx.aida(
        &reuse_args(&[])
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(
        text(&refused).contains("which has 1 unread message;"),
        "{}",
        text(&refused)
    );
    let reused = fx.json(
        &reuse_args(&["--handover"])
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    assert_ne!(reused["run"], run.as_str());
    assert_eq!(reused["reuse"]["previous_run"], run.as_str());
    assert_eq!(reused["reuse"]["handed_over"], 1);
    controller.kill();

    // Crash path: a controller killed without `end` is ended by maintenance.
    let lock2 = fx.root.join("relay2.lock");
    let crashed = Controller::start(&lock2);
    let pid2 = crashed.pid();
    let bound2 = fx.json(&[
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
    ]);
    let run2 = bound2["run"].as_str().unwrap().to_string();
    let capability2 = bound2["capability"].as_str().unwrap().to_string();
    assert!(fx.send("relay-crash", "gap").status.success());
    crashed.kill();
    // The send path reconciles before it resolves: the refused send records
    // the crash, and maintenance afterwards has nothing left to end.
    assert!(!fx.send("relay-crash", "dead-controller").status.success());
    let gc = fx.aida_ok(&["agent", "gc"]);
    assert!(!gc.contains("Relay runs: recorded"), "{gc}");
    let after = fx.aida_stdin(&["relay-mail", "child-bind", "--run", &run2], &capability2);
    assert!(!after.status.success());
    assert!(text(&after).contains("ended"), "{}", text(&after));
}
