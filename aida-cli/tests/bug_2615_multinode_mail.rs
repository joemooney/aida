#![cfg(target_os = "linux")]
//! Cross-node recipient admission and db sync mailbox publication.
// trace:BUG-2615 | ai:codex
use std::path::PathBuf;
use std::process::{Command, Output};

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

impl Fixture {
    fn store(&self) -> PathBuf {
        self.root.join(".aida-store")
    }

    fn lease(&self, node: &str, age_secs: i64) {
        let dir = self.store().join("coordination/leases");
        std::fs::create_dir_all(&dir).unwrap();
        let heartbeat = chrono::Utc::now() - chrono::Duration::seconds(age_secs);
        std::fs::write(
            dir.join(format!("{node}.toml")),
            format!(
                "scope = 'TASK-peer'\nnode_id = '{node}'\nheartbeat_at = '{}'\nttl_secs = 1800\n",
                heartbeat.to_rfc3339()
            ),
        )
        .unwrap();
    }

    fn without_roster() -> Self {
        let fx = Self::new();
        let roster = fx.store().join("registry/team.toml");
        if roster.exists() {
            std::fs::remove_file(roster).unwrap();
        }
        fx
    }
}

#[test]
fn bug_2615_live_node_lease_delivers_without_roster_and_expiry_refuses() {
    let fx = Fixture::without_roster();
    fx.lease("node-b", 0);
    let sent = fx.send("node-b", "live peer");
    assert!(sent.status.success(), "{}", text(&sent));
    let messages = aida_core::mailbox::read_local_messages(&fx.root).unwrap();
    let inbox = aida_core::mailbox::inbox_for("node-b", &messages);
    assert!(inbox
        .iter()
        .any(|m| m.subject.as_deref() == Some("live peer")));
    fx.lease("node-b", 3600);
    let refused = fx.send("node-b", "expired peer");
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(
        text(&refused).contains("aida team add"),
        "{}",
        text(&refused)
    );
}

#[test]
fn bug_2615_author_only_identity_refuses() {
    let fx = Fixture::without_roster();
    // A canonical object's author is not enrollment evidence.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.args(["add", "--type", "bug", "--title", "Authored by peer"]);
    fx.env(&mut cmd);
    cmd.env("AIDA_USER", "author-only");
    let added = cmd.output().unwrap();
    assert!(added.status.success(), "{}", text(&added));
    let refused = fx.send("author-only", "not enrolled");
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(
        text(&refused).contains("aida team add"),
        "{}",
        text(&refused)
    );
    assert!(
        text(&refused).contains("live node lease"),
        "{}",
        text(&refused)
    );
}

#[test]
fn bug_2615_db_sync_push_publishes_mailbox_to_remote() {
    let fx = Fixture::without_roster();
    fx.lease("node-b", 0);
    let sent = fx.send("node-b", "publish this");
    assert!(sent.status.success(), "{}", text(&sent));
    let remote = fx._tmp.path().join("remote.git");
    fx.checked(
        Command::new("git")
            .args(["init", "--bare", "--quiet"])
            .arg(&remote),
    );
    fx.checked(
        Command::new("git")
            .arg("-C")
            .arg(fx.store())
            .args(["remote", "add", "origin"])
            .arg(&remote),
    );
    fx.aida_ok(&["db", "sync", "--push"]);
    let branch = fx.checked(
        Command::new("git")
            .arg("-C")
            .arg(fx.store())
            .args(["branch", "--show-current"]),
    );
    let branch = String::from_utf8(branch.stdout).unwrap();
    let tree = fx.checked(Command::new("git").arg("--git-dir").arg(&remote).args([
        "ls-tree",
        "-r",
        "--name-only",
        branch.trim(),
        "mailbox/",
    ]));
    assert!(
        !tree.stdout.is_empty(),
        "mailbox missing from pushed commit"
    );
    let paths = String::from_utf8(tree.stdout).unwrap();
    let mut found = false;
    for path in paths.lines() {
        let blob = fx.checked(
            Command::new("git")
                .arg("--git-dir")
                .arg(&remote)
                .args(["show", &format!("{}:{path}", branch.trim())]),
        );
        found |= String::from_utf8_lossy(&blob.stdout).contains("publish this");
    }
    assert!(found, "pushed mailbox must contain the sent payload");
}
