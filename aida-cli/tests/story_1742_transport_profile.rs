//! Pure plans precede store discovery; execution contacts only isolated bare
//! fixture remotes and preserves store-only hook policy.
// trace:STORY-1742 | ai:codex

#[path = "support/test_git.rs"]
mod test_git;

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn diagnostics(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    home: PathBuf,
    common: PathBuf,
    store_sha: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&home).unwrap();
        let mut fixture = Self {
            _temp: temp,
            common: root.join(".git"),
            root,
            home,
            store_sha: String::new(),
        };
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.git(&["commit", "--allow-empty", "-qm", "product"]);
        fixture.store_sha = fixture.git(&["commit-tree", "HEAD^{tree}", "-m", "store"]);
        fixture.git(&["update-ref", "refs/heads/aida-store", &fixture.store_sha]);
        // Intentionally missing remote: even pull planning must not contact it.
        fixture.git(&[
            "remote",
            "add",
            "origin",
            fixture
                .root
                .parent()
                .unwrap()
                .join("nonexistent-hub.git")
                .to_str()
                .unwrap(),
        ]);
        fixture
    }

    fn command(&self, executable: &str, root: &Path, args: &[&str]) -> Command {
        let mut command = if executable == "git" {
            test_git::git_command()
        } else {
            Command::new(executable)
        };
        for (name, _) in std::env::vars_os() {
            let key = name.to_string_lossy();
            if key.starts_with("AIDA_") || key.starts_with("GIT_") {
                command.env_remove(name);
            }
        }
        command
            .current_dir(root)
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join(".gitconfig"))
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("AIDA_TELEMETRY", "0");
        command
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.command("git", &self.root, args).output().unwrap();
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn aida_at(&self, root: &Path, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_aida"), root, args)
            .output()
            .unwrap()
    }

    fn aida(&self, args: &[&str]) -> Output {
        self.aida_at(&self.root, args)
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.aida(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn enable(&self) -> Value {
        self.json(&["config", "transport", "enable", "work", "--json"])
    }

    fn attach_with_remote(&self) -> (PathBuf, PathBuf) {
        let remote = self.root.parent().unwrap().join("hub.git");
        self.git(&["init", "--bare", "-q", remote.to_str().unwrap()]);
        self.git(&["remote", "set-url", "origin", remote.to_str().unwrap()]);
        let store = self.root.join(".aida-store");
        self.git(&[
            "worktree",
            "add",
            "-q",
            store.to_str().unwrap(),
            "aida-store",
        ]);
        self.enable();
        (store, remote)
    }

    fn git_at(&self, root: &Path, args: &[&str]) -> String {
        let output = self.command("git", root, args).output().unwrap();
        assert!(output.status.success(), "{}", diagnostics(&output));
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn identity_policy(&self, email: &str) {
        fs::create_dir_all(self.root.join(".aida")).unwrap();
        fs::write(
            self.root.join(".aida/config.toml"),
            format!("[identity]\nallowed_emails = [\"{email}\"]\n"),
        )
        .unwrap();
    }

    fn snapshot(&self) -> Vec<String> {
        vec![
            self.git(&["status", "--porcelain=v1"]),
            self.git(&["ls-files", "--stage"]),
            self.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
            self.git(&["rev-parse", "HEAD"]),
        ]
    }
}

#[test]
// trace:STORY-1742.acda93d1 | ai:codex
// trace:STORY-1742.ac80f8ef | ai:codex
// trace:STORY-1742.ac7443a3 | ai:codex
fn profile_is_clone_local_shared_by_worktrees_and_disable_restores_default() {
    let fixture = Fixture::new();
    let initial = fixture.snapshot();
    assert_eq!(
        fixture.json(&["config", "transport", "show", "--format", "json"])["enabled"],
        false
    );
    let enabled = fixture.enable();
    assert_eq!(enabled["enabled"], true);
    assert_eq!(enabled["profile"]["name"], "work");
    assert_eq!(enabled["origin_matches"], true);
    assert_eq!(enabled["execution_available"], true);
    assert_eq!(enabled["pull_execution_available"], false);
    assert_eq!(enabled["sanitization_available"], false);
    assert!(enabled["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap().contains("raw Git")));
    assert_eq!(initial, fixture.snapshot());
    assert!(!fixture.root.join(".aida").exists());
    let sibling = fixture.root.parent().unwrap().join("sibling");
    fixture.git(&[
        "worktree",
        "add",
        "-qb",
        "sibling",
        sibling.to_str().unwrap(),
    ]);
    let shown = fixture.aida_at(&sibling, &["config", "transport", "show", "--json"]);
    assert!(shown.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&shown.stdout).unwrap()["profile"],
        enabled["profile"]
    );
    let disabled = fixture.aida_at(&sibling, &["config", "transport", "disable", "--json"]);
    assert!(disabled.status.success());
    assert_eq!(
        fixture.json(&["config", "transport", "show", "--json"])["enabled"],
        false
    );
    assert!(!fixture.common.join("aida-transport.json").exists());
    assert!(fixture.common.join("aida-transport.lock").is_file());
    // With the profile disabled this store-less clone takes ordinary discovery.
    let ordinary = fixture.aida(&["push", "--dry-run"]);
    assert!(!diagnostics(&ordinary).contains("not available in this slice"));
}

#[test]
// trace:STORY-1742.ac58fd1a | ai:codex
// trace:STORY-1742.acf57804 | ai:codex
// trace:STORY-1742.acc84d5d | ai:codex
fn plans_are_read_only_and_execution_cannot_fall_through() {
    let fixture = Fixture::new();
    fixture.enable();
    fs::write(fixture.root.join("untracked-product"), "leave this alone").unwrap();
    let before = fixture.snapshot();
    for operation in ["push", "pull"] {
        let plan = fixture.json(&[operation, "--json", "--store-only"]);
        assert_eq!(plan["profile"], "work");
        assert_eq!(plan["code_leg"], "forbidden");
        assert_eq!(plan["flags"]["--store-only"], "redundant");
        assert_eq!(plan["execution_available"], operation == "push");
        assert_eq!(plan["remote"], "origin");
        assert_eq!(
            plan["refs_that_may_move"].as_array().unwrap().len(),
            if operation == "push" { 1 } else { 2 }
        );
        if operation == "push" {
            assert_eq!(plan["source_sha"], fixture.store_sha);
            assert_eq!(
                plan["refspec"],
                "refs/heads/aida-store:refs/heads/aida-store"
            );
        } else {
            assert!(plan["source_sha"].is_null());
            assert_eq!(
                plan["refspec"],
                "refs/heads/aida-store:refs/remotes/origin/aida-store"
            );
        }
        let refused = fixture.aida(&[operation]);
        assert!(!refused.status.success());
        assert!(
            diagnostics(&refused).contains(if operation == "push" {
                "attached aida-store worktree"
            } else {
                "not available in this slice"
            }),
            "{}",
            diagnostics(&refused)
        );
        let forbidden = fixture.aida(&[operation, "--json", "--code-only"]);
        assert!(!forbidden.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&forbidden.stdout).unwrap()["flags"]["--code-only"],
            "forbidden"
        );
    }
    // The profile wins over an ordinary code-only default.
    let output = fixture
        .command(
            env!("CARGO_BIN_EXE_aida"),
            &fixture.root,
            &["push", "--format", "json"],
        )
        .env("AIDA_PUSH_DEFAULT", "code")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["code_leg"],
        "forbidden"
    );
    assert_eq!(before, fixture.snapshot());
    assert!(!fixture.root.join(".aida").exists());
    assert!(!fixture.root.join(".aida-store").exists());
    assert!(!fixture.common.join("FETCH_HEAD").exists());
}

#[test]
fn planning_from_store_worktree_does_not_install_hooks_or_projection() {
    let fixture = Fixture::new();
    fixture.enable();
    let store = fixture.root.join(".aida-store");
    fixture.git(&[
        "worktree",
        "add",
        "-q",
        store.to_str().unwrap(),
        "aida-store",
    ]);
    let before = fixture.snapshot();
    let entries = || {
        let mut names = fs::read_dir(&fixture.common)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        names.sort();
        names
    };
    let before_entries = entries();
    let output = fixture.aida_at(&store, &["push", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["source_sha"],
        fixture.store_sha
    );
    assert_eq!(entries(), before_entries);
    assert_eq!(fixture.snapshot(), before);
    assert!(!fixture.root.join(".aida").exists());
    assert!(!store.join(".aida").exists());
}

#[test]
fn dangerous_flags_refuse_and_do_not_echo_message() {
    let fixture = Fixture::new();
    fixture.enable();
    for args in [
        vec!["push", "--dry-run", "--message", "sensitive-marker"],
        vec!["pull", "--dry-run", "--auto"],
    ] {
        let output = fixture.aida(&args);
        assert!(!output.status.success());
        assert!(
            diagnostics(&output).contains("Flag forbidden"),
            "{}",
            diagnostics(&output)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("sensitive-marker"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("sensitive-marker"));
    }
}

#[test]
fn explicit_store_overrides_cannot_bypass_a_profile_in_either_clone() {
    let fixture = Fixture::new();
    fixture.enable();
    for operation in ["push", "pull"] {
        let args = [
            "--file",
            fixture.root.to_str().unwrap(),
            operation,
            "--json",
        ];
        for root in [&fixture.root, &fixture.home] {
            let output = fixture.aida_at(root, &args);
            assert!(!output.status.success());
            assert!(
                diagnostics(&output).contains("refuses --file"),
                "{}",
                diagnostics(&output)
            );
        }
    }
    assert!(!fixture.root.join(".aida").exists());
    assert!(!fixture.home.join(".aida").exists());
}

#[test]
fn origin_is_pinned_and_diagnostics_do_not_expose_endpoints_or_credentials() {
    let fixture = Fixture::new();
    let endpoint = "https://fixture:secret-marker@hub.example.invalid/store.git";
    fixture.git(&["remote", "set-url", "origin", endpoint]);
    let output = fixture.aida(&["config", "transport", "enable", "work", "--json"]);
    assert!(output.status.success());
    for bytes in [
        output.stdout,
        fs::read(fixture.common.join("aida-transport.json")).unwrap(),
    ] {
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("secret-marker"));
        assert!(!text.contains("hub.example.invalid"));
    }
    fixture.git(&["remote", "set-url", "origin", "../changed-origin.git"]);
    assert_eq!(
        fixture.json(&["config", "transport", "show", "--json"])["origin_matches"],
        false
    );
    let refused = fixture.aida(&["push", "--json"]);
    assert!(!refused.status.success());
    assert!(
        diagnostics(&refused).contains("Origin changed"),
        "{}",
        diagnostics(&refused)
    );
    assert!(!fixture
        .aida(&["config", "transport", "enable", "work"])
        .status
        .success());
    assert!(fixture
        .aida(&["config", "transport", "disable"])
        .status
        .success());
    fixture.enable();
    fixture.git(&["remote", "set-url", "--add", "--push", "origin", endpoint]);
    let refused = fixture.aida(&["push", "--json"]);
    assert!(!refused.status.success());
    assert!(!diagnostics(&refused).contains("secret-marker"));
}

#[test]
fn relative_origin_pin_uses_worktree_root_and_refuses_a_different_store_endpoint() {
    let fixture = Fixture::new();
    fixture.git(&["remote", "set-url", "origin", "../nonexistent-hub.git"]);
    fixture.enable();
    let sub = fixture.root.join("src");
    fs::create_dir(&sub).unwrap();
    assert!(fixture.aida_at(&sub, &["push", "--json"]).status.success());
    let store = fixture.root.join(".aida-store");
    fixture.git(&[
        "worktree",
        "add",
        "-q",
        store.to_str().unwrap(),
        "aida-store",
    ]);
    let output = fixture.aida_at(&store, &["push", "--json"]);
    assert!(!output.status.success());
    assert!(
        diagnostics(&output).contains("Origin changed"),
        "{}",
        diagnostics(&output)
    );
}

#[test]
fn malformed_or_future_profiles_and_mirror_origins_fail_closed() {
    let fixture = Fixture::new();
    let profile = fixture.common.join("aida-transport.json");
    for text in [
        "not-json sensitive-marker",
        r#"{"version":99,"name":"work","origin_sha256":"sensitive-marker"}"#,
    ] {
        fs::write(&profile, text).unwrap();
        let output = fixture.aida(&["pull", "--json"]);
        assert!(!output.status.success());
        assert!(
            diagnostics(&output).contains("Invalid"),
            "{}",
            diagnostics(&output)
        );
        assert!(!diagnostics(&output).contains("sensitive-marker"));
    }
    fs::remove_file(profile).unwrap();
    fixture.git(&["config", "remote.origin.mirror", "true"]);
    assert!(!fixture
        .aida(&["config", "transport", "enable", "work"])
        .status
        .success());
    assert!(!fixture.common.join("aida-transport.json").exists());
}

#[test]
fn baseline_enrollment_is_explicit_and_must_belong_to_store_history() {
    let fixture = Fixture::new();
    let baseline = fixture.json(&[
        "config",
        "transport",
        "enable",
        "work",
        "--baseline",
        &fixture.store_sha,
        "--json",
    ]);
    assert_eq!(baseline["profile"]["baseline"], fixture.store_sha);
    assert_eq!(
        fixture.json(&["push", "--json"])["blockers"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    fixture.json(&["config", "transport", "disable", "--json"]);
    for value in ["HEAD", &fixture.git(&["rev-parse", "HEAD"])] {
        let output = fixture.aida(&["config", "transport", "enable", "work", "--baseline", value]);
        assert!(!output.status.success());
        assert!(!fixture.common.join("aida-transport.json").exists());
    }
}

// Real hooks exercise the shipped profile and a single receive, rather than a
// mocked push that could conceal extra refs or lifecycle actions.
// trace:STORY-1742 | ai:codex
#[cfg(unix)]
fn hook(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
#[cfg(unix)]
// trace:STORY-1742.ac37eb1b | ai:codex
// trace:STORY-1742.ac804464 | ai:codex
fn executor_pushes_one_exact_store_ref_with_identity_hook_and_no_code_hooks() {
    let fixture = Fixture::new();
    let (store, remote) = fixture.attach_with_remote();
    fixture.identity_policy("fixture@example.invalid");
    let bait = fixture.common.join("project-hooks");
    hook(&bait.join("pre-push"), "#!/bin/sh\nexit 93\n");
    fixture.git(&["config", "core.hooksPath", bait.to_str().unwrap()]);
    fixture.git(&["config", "push.followTags", "true"]);
    fixture.git(&["config", "push.negotiate", "true"]);
    fixture.git(&["config", "push.recurseSubmodules", "on-demand"]);
    fixture.git(&[
        "config",
        "remote.origin.push",
        "refs/heads/main:refs/heads/main",
    ]);
    fixture.git(&[
        "tag",
        "-a",
        "store-tag",
        &fixture.store_sha,
        "-m",
        "not for export",
    ]);
    hook(
        &remote.join("hooks/pre-receive"),
        "#!/bin/sh\nprintf 'attempt\n' >> attempts\ncat >> received-refs\n",
    );
    fs::write(fixture.root.join("untracked-product"), "not committed").unwrap();
    let before = fixture.snapshot();
    let profile_before = fs::read(fixture.common.join("aida-transport.json")).unwrap();
    let output = fixture.aida(&["push"]);
    assert!(output.status.success(), "{}", diagnostics(&output));
    assert_eq!(
        fixture.git_at(
            &remote,
            &["for-each-ref", "--format=%(refname) %(objectname)"]
        ),
        format!("refs/heads/aida-store {}", fixture.store_sha)
    );
    assert_eq!(
        fs::read_to_string(remote.join("attempts")).unwrap(),
        "attempt\n"
    );
    let received = fs::read_to_string(remote.join("received-refs")).unwrap();
    assert_eq!(received.lines().count(), 1);
    assert!(received.ends_with(&format!("{} refs/heads/aida-store\n", fixture.store_sha)));
    assert_eq!(before, fixture.snapshot());
    assert_eq!(
        profile_before,
        fs::read(fixture.common.join("aida-transport.json")).unwrap()
    );
    assert!(!fixture.common.join("FETCH_HEAD").exists());
    assert!(!fixture.root.join(".aida/cache.db").exists());
    assert!(!store.join(".aida/cache.db").exists());
    assert_eq!(fixture.git_at(&store, &["status", "--porcelain=v1"]), "");
}

#[test]
#[cfg(unix)]
fn identity_refusal_and_remote_rejection_never_retry_or_echo_private_diagnostics() {
    let fixture = Fixture::new();
    let (_store, remote) = fixture.attach_with_remote();
    fixture.identity_policy("other@example.invalid");
    hook(&remote.join("hooks/pre-receive"),
        "#!/bin/sh\nprintf 'attempt\n' >> attempts\ncat >/dev/null\necho secret-host.example.invalid >&2\nexit 1\n");
    let before = fixture.snapshot();
    let output = fixture.aida(&["push"]);
    assert!(!output.status.success(), "{}", diagnostics(&output));
    assert!(diagnostics(&output).contains("no retry"));
    assert!(!diagnostics(&output).contains("fixture@example.invalid"));
    assert!(!remote.join("attempts").exists()); // identity refused before receive
    fixture.identity_policy("fixture@example.invalid");
    let output = fixture.aida(&["push"]);
    assert!(!output.status.success(), "{}", diagnostics(&output));
    assert!(!diagnostics(&output).contains("secret-host"));
    assert!(!diagnostics(&output).contains(remote.to_str().unwrap()));
    assert_eq!(
        fs::read_to_string(remote.join("attempts")).unwrap(),
        "attempt\n"
    );
    assert_eq!(fixture.git_at(&remote, &["for-each-ref"]), "");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
#[cfg(unix)]
// trace:STORY-1742.acf9e200 | ai:codex
fn dirty_in_progress_and_tampered_store_hooks_refuse_without_receiving() {
    let fixture = Fixture::new();
    let (store, remote) = fixture.attach_with_remote();
    hook(
        &remote.join("hooks/pre-receive"),
        "#!/bin/sh\necho received >> attempts\ncat >/dev/null\n",
    );
    let before = fixture.git(&["rev-parse", "aida-store"]);
    fixture.git(&["config", "remote.origin.promisor", "true"]);
    let output = fixture.aida(&["push"]);
    assert!(!output.status.success());
    assert!(diagnostics(&output).contains("hydration"));
    fixture.git(&["config", "--unset", "remote.origin.promisor"]);
    fs::write(store.join("uncommitted"), "must not be auto-committed").unwrap();
    assert!(!fixture.aida(&["push"]).status.success());
    fs::remove_file(store.join("uncommitted")).unwrap();
    let gitdir = PathBuf::from(fixture.git_at(&store, &["rev-parse", "--absolute-git-dir"]));
    for marker in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "index.lock",
    ] {
        fs::write(gitdir.join(marker), &fixture.store_sha).unwrap();
        assert!(!fixture.aida(&["push"]).status.success(), "{marker}");
        fs::remove_file(gitdir.join(marker)).unwrap();
    }
    for marker in ["rebase-merge", "rebase-apply", "sequencer"] {
        fs::create_dir(gitdir.join(marker)).unwrap();
        assert!(!fixture.aida(&["push"]).status.success(), "{marker}");
        fs::remove_dir(gitdir.join(marker)).unwrap();
    }
    let hooks = fs::read_dir(&fixture.common)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("aida-store-hooks-")
        })
        .unwrap();
    fs::remove_file(hooks.join("pre-push")).unwrap();
    hook(&hooks.join("pre-push"), "#!/bin/sh\nexit 0 # tampered\n");
    assert!(!fixture.aida(&["push"]).status.success());
    assert!(!remote.join("attempts").exists());
    assert_eq!(fixture.git(&["rev-parse", "aida-store"]), before);
    assert_eq!(fixture.git_at(&remote, &["for-each-ref"]), "");
}

#[test]
fn non_fast_forward_is_not_reconciled_or_force_pushed() {
    let fixture = Fixture::new();
    let (_store, remote) = fixture.attach_with_remote();
    fixture.git(&[
        "push",
        remote.to_str().unwrap(),
        "main:refs/heads/aida-store",
    ]);
    let remote_before = fixture.git_at(&remote, &["rev-parse", "aida-store"]);
    let local_before = fixture.snapshot();
    let output = fixture.aida(&["push"]);
    assert!(!output.status.success(), "{}", diagnostics(&output));
    assert_eq!(
        fixture.git_at(&remote, &["rev-parse", "aida-store"]),
        remote_before
    );
    assert_eq!(fixture.snapshot(), local_before);
    assert!(!fixture.common.join("FETCH_HEAD").exists());
}

#[test]
#[cfg(unix)]
fn writer_lock_is_released_before_receive_and_later_commits_stay_local() {
    let fixture = Fixture::new();
    let (store, remote) = fixture.attach_with_remote();
    let later = fixture.git(&[
        "commit-tree",
        "aida-store^{tree}",
        "-p",
        &fixture.store_sha,
        "-m",
        "later local work",
    ]);
    // A real receiver takes the same lock non-blockingly and advances only the
    // local branch after the executor has chosen its SHA. No sleeps or timing race.
    hook(&remote.join("hooks/pre-receive"), &format!(
        "#!/bin/sh\nset -eu\npython3 - '{}' '{}' '{}' <<'PY'\nimport fcntl, subprocess, sys\nwith open(sys.argv[1], 'r+') as lock:\n fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)\n subprocess.run(['git', '-c', 'maintenance.auto=false', '-c', 'gc.auto=0', '--git-dir=' + sys.argv[2], 'update-ref', 'refs/heads/aida-store', sys.argv[3]], check=True, env={{k: v for k, v in __import__('os').environ.items() if not k.startswith('GIT_')}})\nPY\ncat >/dev/null\n",
        store.join(".aida/store-write.lock").display(), fixture.common.display(), later
    ));
    let output = fixture.aida(&["push"]);
    assert!(output.status.success(), "{}", diagnostics(&output));
    assert_eq!(
        fixture.git_at(&remote, &["rev-parse", "aida-store"]),
        fixture.store_sha
    );
    assert_eq!(fixture.git(&["rev-parse", "aida-store"]), later);
}

#[test]
fn chained_url_rewrite_is_refused_without_contacting_either_destination() {
    let fixture = Fixture::new();
    let (_store, remote) = fixture.attach_with_remote();
    let alias = "transport-alias:fixture";
    fixture.git(&[
        "config",
        &format!("url.{}.insteadOf", remote.display()),
        alias,
    ]);
    fixture.git(&["remote", "set-url", "origin", alias]);
    // The first expansion remains the enrolled endpoint; passing it as an
    // explicit destination would expand again to this unintended endpoint.
    let other = fixture.root.parent().unwrap().join("other.git");
    fixture.git(&["init", "--bare", "-q", other.to_str().unwrap()]);
    fixture.git(&[
        "config",
        &format!("url.{}.insteadOf", other.display()),
        remote.to_str().unwrap(),
    ]);
    let output = fixture.aida(&["push"]);
    assert!(!output.status.success(), "{}", diagnostics(&output));
    assert!(
        diagnostics(&output).contains("rewrites"),
        "{}",
        diagnostics(&output)
    );
    assert_eq!(fixture.git_at(&remote, &["for-each-ref"]), "");
    assert_eq!(fixture.git_at(&other, &["for-each-ref"]), "");
    fixture.git(&[
        "config",
        "--unset",
        &format!("url.{}.insteadOf", other.display()),
    ]);
    fixture.git(&[
        "config",
        &format!("url.{}.pushInsteadOf", other.display()),
        remote.to_str().unwrap(),
    ]);
    let output = fixture.aida(&["push"]);
    assert!(!output.status.success(), "{}", diagnostics(&output));
    assert!(
        diagnostics(&output).contains("rewrites"),
        "{}",
        diagnostics(&output)
    );
    assert_eq!(fixture.git_at(&other, &["for-each-ref"]), "");
}

#[test]
fn endpoint_whitespace_is_preserved_and_multiline_output_is_refused() {
    let fixture = Fixture::new();
    let (_store, remote) = fixture.attach_with_remote();
    let spaced = remote.with_file_name("hub.git ");
    fs::rename(&remote, &spaced).unwrap();
    fixture.json(&["config", "transport", "disable", "--json"]);
    fixture.git(&["remote", "set-url", "origin", spaced.to_str().unwrap()]);
    fixture.enable();
    let output = fixture.aida(&["push"]);
    assert!(output.status.success(), "{}", diagnostics(&output));
    assert_eq!(
        fixture.git_at(&spaced, &["rev-parse", "aida-store"]),
        fixture.store_sha
    );
    assert!(!remote.exists());
    fixture.git(&[
        "remote",
        "set-url",
        "origin",
        &format!("{}\n", spaced.display()),
    ]);
    let output = fixture.aida(&["push"]);
    assert!(!output.status.success());
    assert!(diagnostics(&output).contains("unambiguous"));
}
