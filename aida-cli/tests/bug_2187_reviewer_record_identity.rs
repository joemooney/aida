#![cfg(target_os = "linux")]
//! A real headless launch must identify its reviewer before the real recorder
//! runs. Labels alone must never make an ordinary implementer shell a reviewer.
// trace:BUG-2187 | ai:codex
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
mod support;
#[path = "support/test_git.rs"]
mod test_git;

fn checked(mut command: Command) -> Output {
    let out = command.output().expect("execute fixture command");
    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}
fn git(repo: &Path, args: &[&str]) -> Output {
    let mut cmd = test_git::git_command();
    cmd.current_dir(repo).args(args);
    checked(cmd)
}
fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}
impl Fixture {
    fn command(&self, args: &[&str], grant: Option<&str>) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
        cmd.current_dir(&self.repo)
            .args(args)
            .env("HOME", &self.home)
            .env("AIDA_HOME", &self.home)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("AIDA_OS_WRAP", "0")
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_TEE_HEADLESS", "0")
            .env("AIDA_AGENT_CMD", self.bin.join("claude"));
        for key in [
            "AIDA_AGENT_TYPE",
            "AIDA_AGENT_NAME",
            "AIDA_CONTAINED",
            "AIDA_SESSION_GRANT",
            "AIDA_SESSION_PURPOSE",
            // A standalone implementer child refuses `review record`.
            // trace:STORY-1662 | ai:claude
            "AIDA_STANDALONE_IMPLEMENTER",
            "AIDA_SESSION_ROLE",
            "AIDA_SESSION_PROJECT",
            "AIDA_DRIVE_ROOT",
            "AIDA_REVIEW_VERDICT_FILE",
            "AIDA_SESSION_ID",
            "CLAUDE_SESSION_ID",
            "CODEX_THREAD_ID",
        ] {
            cmd.env_remove(key);
        }
        if let Some(grant) = grant {
            cmd.env("AIDA_SESSION_GRANT", grant);
        }
        cmd
    }
}
// trace:BUG-2187.ac2db2c6 | ai:claude
// trace:BUG-2187.ac63597b | ai:claude
#[test]
fn scrubbed_headless_reviewer_records_but_implementer_shell_refuses() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = Fixture {
        repo: temp.path().join("repo"),
        home: temp.path().join("home"),
        bin: temp.path().join("bin"),
        _temp: temp,
    };
    for path in [&fixture.repo, &fixture.home, &fixture.bin] {
        std::fs::create_dir_all(path).unwrap();
    }
    git(&fixture.repo, &["init", "-q", "-b", "main"]);
    // trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
    test_git::enroll_synthetic_home(&fixture.repo);
    git(&fixture.repo, &["config", "user.name", "Reviewer fixture"]);
    git(
        &fixture.repo,
        &["config", "user.email", "reviewer@example.test"],
    );
    git(
        &fixture.repo,
        &["commit", "-q", "--allow-empty", "-m", "init"],
    );
    checked(fixture.command(
        &[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
        None,
    ));
    let grant = support::grant_seat(&fixture.home, &fixture.repo, "advisor", &["reviewer"]);
    let added = checked(fixture.command(
        &[
            "add",
            "--type",
            "task",
            "--title",
            "review subject",
            "--status",
            "approved",
        ],
        Some(&grant),
    ));
    let stdout = String::from_utf8_lossy(&added.stdout);
    let spec = stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|s| s.starts_with("TASK-") && s[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap();
    // Fetch the PR ref from this fixture, never the network. Forge metadata
    // remains GitHub-shaped so the actual reviewer setup path is exercised.
    git(
        &fixture.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/fixture/repo.git",
        ],
    );
    let config = fixture.repo.join(".aida/config.toml");
    let mut config_value: toml::Value =
        toml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    config_value["agents"] = toml::Value::Table(toml::Table::from_iter([(
        "enabled".to_string(),
        toml::Value::Array(vec![toml::Value::String("claude".into())]),
    )]));
    config_value["worktree_pool"]["enabled"] = toml::Value::Boolean(false);
    config_value["forge"] = toml::Value::Table(toml::Table::from_iter([(
        "provider".to_string(),
        toml::Value::String("github".into()),
    )]));
    std::fs::write(&config, toml::to_string(&config_value).unwrap()).unwrap();
    git(&fixture.repo, &["add", ".aida/config.toml"]);
    git(
        &fixture.repo,
        &["commit", "-q", "-m", "fixture forge configuration"],
    );
    let head = String::from_utf8(git(&fixture.repo, &["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_string();
    git(&fixture.repo, &["update-ref", "refs/pull/59/head", &head]);
    git(
        &fixture.repo,
        &["update-ref", "refs/remotes/origin/main", &head],
    );

    // Redirect only fetches, keeping origin discovery pinned to GitHub.
    // Every other command delegates to the real local Git unchanged.
    executable(&fixture.bin.join("git"), &format!("#!/bin/sh\nif [ \"$1\" = -C ]; then repo=$2; shift 2; else repo=$PWD; fi\nif [ \"$1\" = fetch ]; then\n shift\n if [ \"$1\" = origin ]; then shift; exec /usr/bin/git -C \"$repo\" -c maintenance.auto=false -c gc.auto=0 fetch '{}' \"$@\"; fi\n echo 'unexpected fixture fetch; network refused' >&2; exit 97\nfi\nexec /usr/bin/git -C \"$repo\" \"$@\"\n", fixture.repo.display()));
    let metadata = serde_json::json!({"number":59,"title":format!("Fix ({spec})"),"body":"","headRefName":"fixture-head","baseRefName":"main","headRefOid":head,"baseRefOid":head,"labels":[],"url":"https://github.com/fixture/repo/pull/59","state":"OPEN","isDraft":false,"files":[{"path":"src.rs"}]});
    executable(&fixture.bin.join("gh"), &format!("#!/bin/sh\ncase \"$*\" in\n*--jq*) printf '%s\\n' '{head}';;\n*\"repo view\"*) printf '%s\\n' '{{\"nameWithOwner\":\"fixture/repo\"}}';;\n*\"pr diff\"*) printf '%s\\n' src.rs;;\n*) printf '%s\\n' '{metadata}';;\nesac\n"));
    executable(&fixture.bin.join("claude"), &format!("#!/bin/sh\nset -eu\n[ \"$AIDA_AGENT_TYPE\" = claude ]\n[ \"$AIDA_AGENT_NAME\" = claude-reviewer ]\n'{}' review record {spec} --pr 59 --verdict approved --sha {head} --summary 'fixture independent review'\nprintf '%s\\n' '{{\"type\":\"result\",\"result\":\"Recorded approval\"}}'\n", env!("CARGO_BIN_EXE_aida").replace('\'', "'\\''")));
    let worktree = fixture._temp.path().join("reviewer");
    let output = checked(fixture.command(
        &[
            "queue",
            "work",
            "PR-59",
            "--role",
            "reviewer",
            "--no-human",
            "--vendor",
            "claude",
            "--no-pull",
            "--base",
            "main",
            "--path",
            worktree.to_str().unwrap(),
        ],
        Some(&grant),
    ));
    let path = fixture.repo.join(".aida/review-verdicts/PR-59.json");
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr))),
    )
    .unwrap();
    assert_eq!(record["reviewed_sha"], head);
    assert_eq!(record["recorder_attestation"]["seat"], "reviewer");
    assert!(record["recorded_by"]
        .as_str()
        .unwrap()
        .contains("claude-reviewer"));
    let before = std::fs::read(&path).unwrap();
    let implementer = support::grant_seat(&fixture.home, &fixture.repo, "implementer", &[]);
    let mut ordinary = fixture.command(
        &[
            "review",
            "record",
            "PR-59",
            "--verdict",
            "approved",
            "--sha",
            &head,
            "--summary",
            "unauthorized",
        ],
        Some(&implementer),
    );
    // A non-reviewer grant cannot gain recording authority through labels.
    ordinary
        .env("AIDA_SESSION_ROLE", "reviewer")
        .env("AIDA_AGENT_TYPE", "claude")
        .env("AIDA_AGENT_NAME", "unattested-reviewer");
    let refused = ordinary.output().unwrap();
    assert!(!refused.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
