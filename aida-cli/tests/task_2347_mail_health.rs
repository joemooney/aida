//! Real doctor JSON -> substrate failure -> advisor due-route coverage.
// trace:TASK-2347 | ai:codex
#[path = "support/test_git.rs"]
mod test_git;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
const JOB: &str = "aida-codex-mail-fixture-health";
const THREAD: &str = "01a1700b-717d-71f2-b7e0-0569b05f3dee";
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    state: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let home = temp.path().join("home");
        let state = temp.path().join("state");
        for path in [&root, &home, &state] {
            fs::create_dir(path).unwrap();
        }
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let f = Self {
            _temp: temp,
            root,
            home,
            state,
        };
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["commit", "--allow-empty", "-qm", "project"],
            vec!["checkout", "-q", "--orphan", "aida-store"],
            vec!["commit", "--allow-empty", "-qm", "store"],
            vec!["checkout", "-q", "main"],
            vec!["worktree", "add", "-q", ".aida-store", "aida-store"],
        ] {
            assert!(f.command(true, &args).output().unwrap().status.success());
        }
        fs::create_dir(f.root.join(".aida")).unwrap();
        fs::write(f.root.join(".aida/config.toml"), format!(
            "[store]\nmode='distributed'\nstore_path='.aida-store'\n[schedule]\nmin_gap='1s'\n[[schedule.jobs]]\nname='{JOB}'\ncommand='doctor check codex-mail-wake --fail-on-findings'\nevery='15m'\nenabled=true\n[[schedule.jobs]]\nname='{JOB}-route'\nseats=['advisor']\non=['CronJobFailed:{JOB}']\nprompt='Inspect counts and ages'\nenabled=true\n")).unwrap();
        f.write(
            &f.root.join(".aida/codex-mail-health.json"),
            serde_json::json!({"version":1,"project":f.root,"thread":THREAD,
            "state_dir":f.state,"job":JOB,"max_age_seconds":3600,"snapshot_max_age_seconds":900}),
        );
        f.snapshot(false);
        f
    }
    fn command(&self, git: bool, args: &[&str]) -> Command {
        let mut c = if git {
            test_git::git_command()
        } else {
            Command::new(env!("CARGO_BIN_EXE_aida"))
        };
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("AIDA_")
                || key.to_string_lossy().starts_with("GIT_")
            {
                c.env_remove(key);
            }
        }
        c.current_dir(&self.root)
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join("gitconfig"))
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("AIDA_TELEMETRY", "0");
        c
    }
    fn aida(&self, args: &[&str]) -> Output {
        self.command(false, args).output().unwrap()
    }
    fn write(&self, path: &Path, value: serde_json::Value) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn snapshot(&self, stale: bool) {
        let now = chrono::Utc::now().timestamp() as f64;
        self.write(&self.state.join("health.json"), serde_json::json!({"version":1,"project":self.root,"thread":THREAD,
            "observed_at":now,"accepted_unacknowledged_count":u32::from(stale),"accepted_unknown_age_count":0,
            "oldest_known_notification_at":if stale { Some(now-86400.0) } else { None },"recovery_required":false}));
    }
}
#[test]
fn task_2347_real_health_routes_counts_once_and_recovers() {
    let f = Fixture::new();
    let seed = f.aida(&["schedule", "tick"]);
    assert!(
        seed.status.success(),
        "{}{}",
        String::from_utf8_lossy(&seed.stdout),
        String::from_utf8_lossy(&seed.stderr)
    );
    f.snapshot(true);
    let doctor = f.aida(&[
        "doctor",
        "check",
        "codex-mail-wake",
        "--json",
        "--fail-on-findings",
    ]);
    assert!(!doctor.status.success());
    let stderr = String::from_utf8_lossy(&doctor.stderr);
    assert!(
        stderr.contains("accepted_unacknowledged_count=1"),
        "{stderr}"
    );
    assert!(stderr.contains("recovery_required=false"));
    let run = f.aida(&["schedule", "run", JOB]);
    // schedule run reports child failure in the job ledger/stdout; the CLI
    // invocation itself succeeds after recording that outcome.
    assert!(run.status.success());
    assert!(String::from_utf8_lossy(&run.stdout).contains("failed"));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert!(f.aida(&["schedule", "tick"]).status.success());
    let due = f.aida(&["schedule", "due", "--seat", "advisor", "--json"]);
    assert!(due.status.success());
    let rows: serde_json::Value = serde_json::from_slice(&due.stdout).unwrap();
    let jobs = rows.as_array().unwrap();
    assert_eq!(jobs.len(), 1, "{rows}");
    let error = jobs[0]["failure"]["error"].as_str().unwrap();
    assert!(error.contains("accepted_unacknowledged_count=1"), "{error}");
    assert!(error.contains("oldest_known_notification_age_seconds="));
    assert!(error.contains("recovery_required=false"));
    assert!(!error.contains(THREAD));
    f.snapshot(false);
    assert!(f.aida(&["schedule", "run", JOB]).status.success());
    assert!(f
        .aida(&[
            "doctor",
            "check",
            "codex-mail-wake",
            "--json",
            "--fail-on-findings"
        ])
        .status
        .success());
}
