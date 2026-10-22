// Binary-driving e2e suite; Linux-only like its siblings (BUG-688).
#![cfg(target_os = "linux")]
//! STORY-1662 PR2 (BUG-2207's `claude --bg` path): a `--bg` session runs with
//! the claude daemon's environment, often the advisor's. The lane's identity
//! comes from its worktree lease instead: `aida worktree add <SPEC> --role
//! implementer` records it, `aida claim` inside the lane keeps it,
//! `aida internal lane-env` prints the correction, and the SessionStart hook
//! writes it to $CLAUDE_ENV_FILE — failing closed when lane-env cannot answer.
// trace:STORY-1662 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

fn aida(repo: &Path, home: &Path, bin_dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    let path = std::env::join_paths(
        std::iter::once(bin_dir.to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    cmd.env("PATH", path);
    for key in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_HEADLESS_VENDOR",
        "AIDA_AGENT_MODEL",
        "AIDA_AGENT_CMD",
        "AIDA_REVIEW_VERDICT_FILE",
        "AIDA_AGENT_NAME",
        "AIDA_AGENT_TYPE",
        "AIDA_SESSION_ID",
    ] {
        cmd.env_remove(key);
    }
    cmd
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = test_git::git_command()
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    advisor: String,
    spec: String,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let repo = base.join("repo");
    let home = base.join("home");
    let bin = base.join("bin");
    for dir in [&repo, &home, &bin] {
        std::fs::create_dir_all(dir).unwrap();
    }
    // The hook runs `aida` from PATH: the binary under test.
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_aida"), bin.join("aida")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(&repo, &home, &bin)
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
    let advisor = support::ensure_seat(&home, &repo, "advisor", &["implementer", "reviewer"])
        .expect("initialized");
    let add = as_advisor(aida(&repo, &home, &bin), &advisor)
        .args([
            "add",
            "--type",
            "story",
            "--status",
            "approved",
            "--title",
            "bg lane identity",
        ])
        .output()
        .unwrap();
    assert!(add.status.success(), "{add:?}");
    let stdout = String::from_utf8_lossy(&add.stdout);
    let spec = stdout
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| t.starts_with("STORY-"))
        .unwrap_or_else(|| panic!("no spec id in {stdout}"))
        .to_string();
    Fixture {
        _tmp: tmp,
        base,
        repo,
        home,
        bin,
        advisor,
        spec,
    }
}

/// The environment a `claude --bg` lane inherits from a daemon the advisor
/// started.
fn as_advisor(mut cmd: Command, grant: &str) -> Command {
    cmd.env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_SESSION_GRANT", grant)
        .env("AIDA_SESSION_PURPOSE", "advise");
    cmd
}

fn hook_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aida-core/templates/hooks/aida-session-context.sh")
}

/// Run the SessionStart hook in `cwd` with `bin` first on PATH and the
/// advisor's env; return what it appended to $CLAUDE_ENV_FILE.
fn run_hook(fx: &Fixture, bin: &Path, cwd: &Path) -> String {
    let env_file = fx.base.join(format!(
        "claude-env-{}",
        cwd.file_name().unwrap().to_string_lossy()
    ));
    let _ = std::fs::remove_file(&env_file);
    let mut hook = Command::new("bash");
    hook.arg(hook_path())
        .current_dir(cwd)
        .env("HOME", &fx.home)
        .env("AIDA_HOME", &fx.home)
        .env("AIDA_TELEMETRY", "0")
        .env(
            "PATH",
            std::env::join_paths(
                std::iter::once(bin.to_path_buf())
                    .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
            )
            .unwrap(),
        )
        .env("CLAUDE_ENV_FILE", &env_file);
    for key in [
        "CLAUDE_PROJECT_DIR",
        "CODEX_PROJECT_DIR",
        "AIDA_SESSION_PROJECT",
        "AIDA_SESSION_ID",
        "AIDA_AGENT_OUTPUT",
    ] {
        hook.env_remove(key);
    }
    let out = as_advisor(hook, &fx.advisor).output().unwrap();
    assert!(out.status.success(), "hook failed: {out:?}");
    std::fs::read_to_string(&env_file).unwrap_or_default()
}

const LANE_LINES: &str = "export AIDA_SESSION_ROLE='implementer'\n\
                          unset AIDA_SESSION_GRANT\n\
                          unset AIDA_SESSION_PURPOSE\n";

#[test]
fn a_bg_lane_takes_its_identity_from_the_worktree_lease() {
    let fx = fixture();
    let lane = fx.base.join("lane");
    let add = as_advisor(aida(&fx.repo, &fx.home, &fx.bin), &fx.advisor)
        .args([
            "worktree",
            "add",
            &fx.spec,
            "--role",
            "implementer",
            "--path",
            lane.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(add.status.success(), "{add:?}");
    let lane = lane.canonicalize().unwrap();

    // The lane's lease names the implementer, whatever the env said.
    let leases = std::fs::read_dir(fx.repo.join(".aida/sessions"))
        .unwrap()
        .flatten()
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        leases
            .iter()
            .any(|l| l.contains("role = \"implementer\"") && l.contains(&fx.spec)),
        "{leases:?}"
    );

    // `aida claim` inside the lane, under the advisor env, keeps the role.
    let claim = as_advisor(aida(&lane, &fx.home, &fx.bin), &fx.advisor)
        .args(["claim", &fx.spec])
        .output()
        .unwrap();
    assert!(claim.status.success(), "{claim:?}");
    let leases = std::fs::read_dir(fx.repo.join(".aida/sessions"))
        .unwrap()
        .flatten()
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        leases.iter().all(|l| !l.contains("role = \"advisor\"")),
        "no advisor-role lease for the lane: {leases:?}"
    );

    // lane-env prints the correction in the lane and nothing outside it.
    let lane_env = |cwd: &Path| {
        as_advisor(aida(cwd, &fx.home, &fx.bin), &fx.advisor)
            .args(["internal", "lane-env"])
            .output()
            .unwrap()
    };
    let inside = lane_env(&lane);
    assert!(inside.status.success(), "{inside:?}");
    assert_eq!(String::from_utf8_lossy(&inside.stdout), LANE_LINES);
    let outside = lane_env(&fx.repo);
    assert!(outside.status.success(), "{outside:?}");
    assert_eq!(String::from_utf8_lossy(&outside.stdout), "");

    // The hook writes exactly those lines in the lane, nothing elsewhere.
    assert_eq!(run_hook(&fx, &fx.bin, &lane), LANE_LINES);
    assert_eq!(run_hook(&fx, &fx.bin, &fx.repo), "");

    // C3: a lease role carrying shell metacharacters is refused by lane-env,
    // and the hook fails closed instead of quoting it.
    for entry in std::fs::read_dir(fx.repo.join(".aida/sessions"))
        .unwrap()
        .flatten()
    {
        let body = std::fs::read_to_string(entry.path()).unwrap();
        if body.contains("role = \"implementer\"") && !body.contains("claim_verb = true") {
            std::fs::write(
                entry.path(),
                body.replace(
                    "role = \"implementer\"",
                    "role = \"implementer'; touch pwned; '\"",
                ),
            )
            .unwrap();
        }
    }
    let refused = lane_env(&lane);
    assert!(!refused.status.success(), "{refused:?}");
    assert_eq!(String::from_utf8_lossy(&refused.stdout), "");
    assert_eq!(
        run_hook(&fx, &fx.bin, &lane),
        "unset AIDA_SESSION_GRANT\nunset AIDA_SESSION_PURPOSE\n"
    );
    assert!(!lane.join("pwned").exists());
}

/// C1 and C3 at the hook: a lane-env that fails, or prints anything but the
/// accepted lines, leaves no grant or purpose inside a linked worktree and
/// sets no role; in the primary checkout it changes nothing.
#[test]
fn the_hook_fails_closed_when_lane_env_cannot_answer() {
    use std::os::unix::fs::PermissionsExt;
    let fx = fixture();
    let lane = fx.base.join("plain-wt");
    git(
        &fx.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "plain",
            lane.to_str().unwrap(),
        ],
    );
    for (name, lane_env) in [
        ("fails", "exit 1".to_string()),
        (
            "injects",
            format!(
                "printf '%s\\n' \"export AIDA_SESSION_ROLE='implementer'\" \
                 \"touch {}\"",
                fx.base.join("pwned").display()
            ),
        ),
        (
            "foreign",
            "printf '%s\\n' \"export PATH='/tmp'\"".to_string(),
        ),
    ] {
        let bin = fx.base.join(format!("stub-{name}"));
        std::fs::create_dir_all(&bin).unwrap();
        let stub = bin.join("aida");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nif [ \"$1 $2\" = \"internal lane-env\" ]; then {lane_env}; fi\nexit 0\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            run_hook(&fx, &bin, &lane),
            "unset AIDA_SESSION_GRANT\nunset AIDA_SESSION_PURPOSE\n",
            "{name}"
        );
        assert_eq!(run_hook(&fx, &bin, &fx.repo), "", "{name}");
    }
    assert!(!fx.base.join("pwned").exists());
}
