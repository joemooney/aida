// Binary-driving e2e suite; Linux-only like its siblings (BUG-688).
#![cfg(target_os = "linux")]
//! BUG-2201: a contained headless reviewer launched for a project other than
//! aida records its verdict.
//!
//! In aida-relay the reviewer's `aida review record` never ran: the contained
//! launch drops user-level allows (`--setting-sources project`), the project's
//! own `.claude/settings.json` had no `permissions`, and the child had no
//! agent identity, so the record refused and its `AIDA_AGENT_TYPE=… aida …`
//! retry was denied. The fixture here is a fresh temp project with a
//! hooks-only `.claude/settings.json`. The fake `claude` stands in for
//! `dontAsk`: it runs `aida review record` only when the launch's `--settings`
//! allow `Bash(aida *)`, and otherwise reports the call as a permission denial
//! the way Claude Code's stream-json `result` event does.
// trace:BUG-2201 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

const FAKE_CLAUDE: &str = r#"#!/bin/sh
env -0 > "$BUG2201_ENV_DUMP"
for a in "$@"; do printf '%s\n' "$a"; done > "$BUG2201_ARGV_DUMP"
if [ -n "$BUG2201_FORCE_DENY" ] || ! grep -qF '"Bash(aida *)"' "$BUG2201_ARGV_DUMP"; then
  printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"permission_denials":[{"tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"aida review record --pr 7 --verdict approved"}},{"tool_name":"Read","tool_use_id":"t2","tool_input":{"file_path":"/primary/.aida-store/objects/x.yaml"}}]}'
  exit 0
fi
aida review record "$BUG2201_SPEC" --pr 7 --sha "$BUG2201_SHA" --verdict request-changes --summary "fixture review" --finding "fixture finding" >&2
rc=$?
printf '{"type":"result","subtype":"success","is_error":false,"permission_denials":[],"result":"record rc=%s"}\n' "$rc"
exit 0
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    spec: String,
    advisor: String,
    sha: String,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = test_git::git_command()
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn aida(fx: &Fixture) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(&fx.repo);
    cmd.env("HOME", &fx.home);
    cmd.env("AIDA_HOME", &fx.home);
    cmd.env("AIDA_TELEMETRY", "0");
    let path = std::env::join_paths(
        std::iter::once(fx.bin.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    cmd.env("PATH", path);
    for key in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_SESSION_PURPOSE",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_HEADLESS_VENDOR",
        "AIDA_AGENT_MODEL",
        "AIDA_AGENT_CMD",
        "AIDA_REVIEW_VERDICT_FILE",
        "AIDA_AGENT_NAME",
        "AIDA_AGENT_TYPE",
        "AIDA_SESSION_ID",
        "AIDA_CONTAINED",
        "AIDA_AUTO_COMPLETE_PHASE",
        "AIDA_DRIVE_ROOT",
        "AIDA_PROJECT_ROOT",
    ] {
        cmd.env_remove(key);
    }
    cmd.env("BUG2201_ENV_DUMP", fx.base.join("child-env"))
        .env("BUG2201_ARGV_DUMP", fx.base.join("child-argv"))
        .env("BUG2201_SPEC", &fx.spec)
        .env("BUG2201_SHA", &fx.sha);
    cmd
}

fn as_advisor(fx: &Fixture) -> Command {
    let mut cmd = aida(fx);
    cmd.env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_SESSION_GRANT", &fx.advisor);
    cmd
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn spec_id(stdout: &str, prefix: &str) -> String {
    stdout
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| {
            t.split_once('-').is_some_and(|(p, n)| {
                p == prefix && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
            })
        })
        .unwrap_or_else(|| panic!("no {prefix} id in {stdout}"))
        .to_string()
}

/// A project outside the aida repo with a hooks-only `.claude/settings.json`,
/// a spec, a queued `Review PR-7` story, and a local `origin` carrying
/// `refs/pull/7/head`.
fn fixture() -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let repo = base.join("relay");
    let home = base.join("home");
    let bin = base.join("bin");
    for dir in [&repo, &home, &bin] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let claude = bin.join("claude");
    std::fs::write(&claude, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    let gh = bin.join("gh");
    std::fs::write(&gh, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    // trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
    test_git::enroll_synthetic_home(&repo);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    std::fs::write(
        repo.join(".claude/settings.json"),
        "{\"hooks\":{\"PreToolUse\":[]}}\n",
    )
    .unwrap();
    git(&repo, &["add", ".claude/settings.json"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    let mut fx = Fixture {
        _tmp: tmp,
        base,
        repo,
        home,
        bin,
        spec: String::new(),
        advisor: String::new(),
        sha,
    };
    let init = aida(&fx)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    std::fs::write(
        fx.repo.join(".aida/config.toml"),
        "[agents]\nenabled = [\"claude\"]\n",
    )
    .unwrap();
    fx.advisor = support::ensure_seat(&fx.home, &fx.repo, "advisor", &["implementer", "reviewer"])
        .expect("initialized");
    let add = as_advisor(&fx)
        .args([
            "add",
            "--type",
            "task",
            "--status",
            "approved",
            "--title",
            "relay version probe",
        ])
        .output()
        .unwrap();
    assert!(add.status.success(), "{add:?}");
    fx.spec = spec_id(&String::from_utf8_lossy(&add.stdout), "TASK");

    let origin = fx.base.join("origin.git");
    git(
        &fx.base,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(
        &fx.repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(
        &fx.repo,
        &["push", "-q", "origin", "main", "HEAD:refs/pull/7/head"],
    );
    let review = as_advisor(&fx)
        .args([
            "add",
            "--type",
            "story",
            "--status",
            "approved",
            "--title",
            &format!("Review PR-7: relay version probe ({})", fx.spec),
        ])
        .output()
        .unwrap();
    assert!(review.status.success(), "{review:?}");
    let story = spec_id(&String::from_utf8_lossy(&review.stdout), "STORY");
    let queued = as_advisor(&fx)
        .args(["queue", "add", &story, "--for", "reviewer"])
        .output()
        .unwrap();
    assert!(queued.status.success(), "{queued:?}");
    let settle = aida(&fx).args(["queue", "list", "--all"]).output().unwrap();
    assert!(settle.status.success(), "{settle:?}");
    fx
}

fn contained_review(fx: &Fixture, force_deny: bool) -> Output {
    let mut cmd = as_advisor(fx);
    cmd.env("AIDA_CONTAINED", "1");
    if force_deny {
        cmd.env("BUG2201_FORCE_DENY", "1");
    }
    cmd.args([
        "queue",
        "work",
        "PR-7",
        "--role",
        "reviewer",
        "--no-human",
        "--fresh",
        "--no-pull",
        "--vendor",
        "claude",
    ])
    .output()
    .unwrap()
}

fn child_env(fx: &Fixture) -> Vec<(String, String)> {
    let raw = std::fs::read(fx.base.join("child-env")).expect("the fake vendor ran");
    raw.split(|b| *b == 0)
        .filter_map(|kv| {
            let kv = String::from_utf8_lossy(kv);
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

#[test]
fn contained_reviewer_in_outside_project_records_its_verdict() {
    let fx = fixture();
    let out = contained_review(&fx, false);
    let log = text(&out);
    let argv = std::fs::read_to_string(fx.base.join("child-argv")).expect("the fake vendor ran");
    assert!(argv.contains("\"Bash(aida *)\""), "{argv}\n{log}");
    let lines: Vec<&str> = argv.lines().collect();
    let settings_at = lines
        .iter()
        .position(|a| *a == "--settings")
        .expect("contained launch passes --settings");
    let settings: serde_json::Value = serde_json::from_str(lines[settings_at + 1]).unwrap();
    let allow: Vec<&str> = settings["permissions"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.as_str())
        .collect();
    assert!(
        !allow.iter().any(|r| r.starts_with("Bash(git")),
        "no git allow rule: {allow:?}"
    );
    assert!(
        child_env(&fx)
            .iter()
            .any(|(k, v)| k == "AIDA_AGENT_TYPE" && v == "claude"),
        "the reviewer child carries its agent identity: {log}"
    );
    // The reviewer's own record, in its review worktree.
    let record = fx
        .base
        .join("relay-pr-7/.aida/review-verdicts")
        .join(format!("{}.json", fx.spec));
    assert!(record.is_file(), "the reviewer recorded its verdict: {log}");
    // The phase-3 handshake at the launcher's anchor (the fake vendor runs
    // outside Claude Code's OS sandbox, so the write lands in place).
    let verdict = fx.repo.join(".aida/review-verdicts/PR-7.json");
    assert!(
        verdict.is_file(),
        "the reviewer's verdict reaches the launcher: {log}"
    );
    let body: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&verdict).unwrap()).unwrap();
    // A request-changes verdict: an approval would also need forge metadata
    // for its author gate, which this pure-git fixture has none of.
    assert!(
        body["verdict"]
            .as_str()
            .is_some_and(|v| v.to_ascii_lowercase().contains("request")),
        "{body}"
    );
    // Not the exit status: BUG-2149's collection then checks the verdict
    // against the PR's live head through a pinned forge read, which a
    // pure-git fixture cannot answer. What BUG-2201 broke is upstream of it.
    assert!(!log.contains("permission layer denied"), "{log}");
    assert!(!log.contains("no agent identity is set"), "{log}");
}

#[test]
fn denied_reviewer_error_names_the_denied_calls() {
    let fx = fixture();
    let out = contained_review(&fx, true);
    let log = text(&out);
    assert_eq!(out.status.code(), Some(30), "no verdict exit: {log}");
    assert!(log.contains("permission layer denied 2"), "{log}");
    assert!(
        log.contains("Bash `aida review record --pr 7 --verdict approved`"),
        "{log}"
    );
    assert!(
        log.contains("Read `/primary/.aida-store/objects/x.yaml`"),
        "{log}"
    );
}
