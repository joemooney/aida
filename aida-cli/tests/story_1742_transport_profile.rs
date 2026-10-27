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
            .env("GIT_AUTHOR_EMAIL", "aida")
            .env("GIT_COMMITTER_EMAIL", "aida")
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
        let result = self.json(&[
            "config",
            "transport",
            "enable",
            "work",
            "--baseline",
            &self.store_sha,
            "--json",
        ]);
        self.enroll_policy();
        result
    }

    fn enroll_policy(&self) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.common.join("reviewed-policy-input");
        fs::write(&path, br#"{"version":1,"reviewed":true,"deny_tokens":["private-marker"],"git_name":"Fixture","git_identity":"aida"}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let output = self.aida(&[
            "config",
            "transport",
            "policy",
            "--file",
            path.to_str().unwrap(),
            "--reviewed",
        ]);
        assert!(output.status.success(), "{}", diagnostics(&output));
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

    #[cfg(unix)]
    fn symlink_parent_remotes(&self) -> (PathBuf, PathBuf) {
        let external = self.root.parent().unwrap().join("external");
        fs::create_dir_all(external.join("child")).unwrap();
        let intended = external.join("hub.git");
        let wrong = self.root.join("hub.git");
        for remote in [&intended, &wrong] {
            self.git(&["init", "--bare", "-q", remote.to_str().unwrap()]);
        }
        std::os::unix::fs::symlink(external.join("child"), self.root.join("link")).unwrap();
        self.git(&["remote", "set-url", "origin", "link/../hub.git"]);
        self.git(&[
            "worktree",
            "add",
            "-q",
            self.root.join(".aida-store").to_str().unwrap(),
            "aida-store",
        ]);
        (intended, wrong)
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
    assert_eq!(enabled["sanitization_available"], true);
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
        assert_eq!(plan["executed"], false);
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
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&profile, fs::Permissions::from_mode(0o600)).unwrap();
        }
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
    fixture.identity_policy("aida");
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
    let output = fixture.aida(&["push", "--store-only"]);
    assert!(output.status.success(), "{}", diagnostics(&output));
    assert!(diagnostics(&output).contains("--store-only: redundant"));
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
    assert!(!diagnostics(&output).contains("aida"));
    assert!(!remote.join("attempts").exists()); // identity refused before receive
    fixture.identity_policy("aida");
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
    let dirty = fixture.aida(&["push"]);
    assert!(!dirty.status.success());
    assert!(diagnostics(&dirty).contains("Store is dirty"));
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
        let refused = fixture.aida(&["push"]);
        assert!(!refused.status.success(), "{marker}");
        assert!(
            diagnostics(&refused).contains("in-progress"),
            "{}",
            diagnostics(&refused)
        );
        fs::remove_file(gitdir.join(marker)).unwrap();
    }
    for marker in ["rebase-merge", "rebase-apply", "sequencer"] {
        fs::create_dir(gitdir.join(marker)).unwrap();
        let refused = fixture.aida(&["push"]);
        assert!(!refused.status.success(), "{marker}");
        assert!(
            diagnostics(&refused).contains("in-progress"),
            "{}",
            diagnostics(&refused)
        );
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

// trace:STORY-1742 | ai:codex
#[test]
#[cfg(unix)]
fn credentialed_pinned_push_authenticates_without_exposing_or_redirecting_the_endpoint() {
    let fixture = Fixture::new();
    let (_store, remote) = fixture.attach_with_remote();
    let endpoint = "https://oauth2:aida-1742-secret@gitlab.fixture.invalid/group/store.git";
    let clean = "https://gitlab.fixture.invalid/group/store.git";
    fixture.json(&["config", "transport", "disable", "--json"]);
    fixture.git(&["remote", "set-url", "origin", endpoint]);
    fixture.enable();
    let profile = fs::read(fixture.common.join("aida-transport.json")).unwrap();
    let exec_dir = fixture.home.join("git-exec");
    let argv_log = fixture.home.join("transport-argv");
    let real_exec = fixture.git(&["--exec-path"]);
    hook(&exec_dir.join("git-remote-https"), &format!(
        "#!/bin/sh\nset -eu\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\ncred=$(printf 'protocol=https\\nhost=gitlab.fixture.invalid\\n\\n' | GIT_TERMINAL_PROMPT=0 git credential fill 2>/dev/null)\ncase \"$cred\" in *'password=aida-1742-secret'*) ;; *) exit 91 ;; esac\nexec env -u GIT_CONFIG_PARAMETERS -u GIT_CONFIG_COUNT git -c protocol.ext.allow=always remote-ext \"$1\" \"git %s {}\"\n",
        argv_log.display(), remote.display()
    ));
    hook(
        &remote.join("hooks/pre-receive"),
        "#!/bin/sh\nprintf 'attempt\\n' >> attempts\ncat >> received-refs\n",
    );
    let output = fixture
        .command(env!("CARGO_BIN_EXE_aida"), &fixture.root, &["push"])
        .env("GIT_EXEC_PATH", &exec_dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "PATH",
            format!("{real_exec}:{}", std::env::var("PATH").unwrap_or_default()),
        )
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", diagnostics(&output));
    let argv = fs::read_to_string(&argv_log).unwrap();
    assert!(argv.contains(clean));
    assert!(!argv.contains("aida-1742-secret"));
    assert!(!diagnostics(&output).contains("aida-1742-secret"));
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
    assert_eq!(
        fixture.git(&["config", "--get", "remote.origin.url"]),
        endpoint
    );
    assert_eq!(
        fs::read(fixture.common.join("aida-transport.json")).unwrap(),
        profile
    );

    // The credentialed pin itself does not match this rule. Its cleaned
    // destination does, so refuse before handing anything to a remote helper.
    fixture.git(&[
        "config",
        "url.https://other.fixture.invalid/.insteadOf",
        clean,
    ]);
    let refused = fixture.aida(&["push"]);
    assert!(!refused.status.success());
    assert!(
        diagnostics(&refused).contains("rewrites"),
        "{}",
        diagnostics(&refused)
    );
    assert!(!diagnostics(&refused).contains("aida-1742-secret"));
    assert_eq!(
        fs::read_to_string(remote.join("attempts")).unwrap(),
        "attempt\n"
    );
}

// trace:STORY-1742 | ai:codex
#[test]
fn relative_origin_executes_against_the_destination_pinned_from_the_caller() {
    let fixture = Fixture::new();
    let (_store, remote) = fixture.attach_with_remote();
    fixture.json(&["config", "transport", "disable", "--json"]);
    fixture.git(&["remote", "set-url", "origin", "../hub.git"]);
    fixture.enable();
    assert_eq!(fixture.json(&["push", "--json"])["executed"], false);
    assert_eq!(fixture.git_at(&remote, &["for-each-ref"]), "");
    let output = fixture.aida(&["push"]);
    assert!(output.status.success(), "{}", diagnostics(&output));
    assert_eq!(
        fixture.git_at(&remote, &["rev-parse", "aida-store"]),
        fixture.store_sha
    );
    assert!(!fixture.root.join("hub.git").exists());
}

// trace:STORY-1742 | ai:codex
#[test]
#[cfg(unix)]
fn busy_store_writer_is_reported_separately_from_dirty_or_active_store() {
    use std::io::BufRead;
    use std::process::Stdio;
    let fixture = Fixture::new();
    let (store, remote) = fixture.attach_with_remote();
    fs::create_dir_all(store.join(".aida")).unwrap();
    let mut holder = Command::new("python3")
        .args(["-u", "-c", "import fcntl,sys; f=open(sys.argv[1], 'a+'); fcntl.flock(f, fcntl.LOCK_EX); print('ready', flush=True); sys.stdin.read()"])
        .arg(store.join(".aida/store-write.lock"))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut ready = String::new();
    std::io::BufReader::new(holder.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready\n");
    let output = fixture
        .command(env!("CARGO_BIN_EXE_aida"), &fixture.root, &["push"])
        .env("AIDA_STORE_LOCK_TIMEOUT_SECS", "0")
        .output()
        .unwrap();
    drop(holder.stdin.take());
    assert!(holder.wait().unwrap().success());
    assert!(!output.status.success());
    assert!(
        diagnostics(&output).contains("writer lock"),
        "{}",
        diagnostics(&output)
    );
    assert!(!diagnostics(&output).contains("Store is dirty"));
    assert_eq!(fixture.git_at(&remote, &["for-each-ref"]), "");
}

// trace:STORY-1742 | ai:codex
#[test]
#[cfg(unix)]
fn symlink_then_parent_pushes_only_to_gits_actual_origin() {
    for absolute in [false, true] {
        let fixture = Fixture::new();
        let (intended, wrong) = fixture.symlink_parent_remotes();
        if absolute {
            fixture.git(&[
                "remote",
                "set-url",
                "origin",
                fixture.root.join("link/../hub.git").to_str().unwrap(),
            ]);
        }
        fixture.enable();
        let before = fixture.snapshot();
        let output = fixture.aida(&["push"]);
        assert!(output.status.success(), "{}", diagnostics(&output));
        assert_eq!(
            fixture.git_at(&wrong, &["for-each-ref"]),
            "",
            "wrong repository must receive nothing"
        );
        assert_eq!(
            fixture.git_at(
                &intended,
                &["for-each-ref", "--format=%(refname) %(objectname)"]
            ),
            format!("refs/heads/aida-store {}", fixture.store_sha)
        );
        assert_eq!(
            fixture.git(&["ls-remote", "origin"]),
            format!("{}\trefs/heads/aida-store", fixture.store_sha)
        );
        assert_eq!(fixture.snapshot(), before);
    }
}

// trace:STORY-1742 | ai:codex
#[test]
#[cfg(unix)]
fn changing_a_symlink_target_after_enrollment_refuses_both_remotes() {
    let fixture = Fixture::new();
    let (intended, wrong) = fixture.symlink_parent_remotes();
    fixture.enable();
    let enrolled = fs::read(fixture.common.join("aida-transport.json")).unwrap();
    let changed = fixture.root.parent().unwrap().join("changed");
    fs::create_dir_all(changed.join("child")).unwrap();
    let other = changed.join("hub.git");
    fixture.git(&["init", "--bare", "-q", other.to_str().unwrap()]);
    fs::remove_file(fixture.root.join("link")).unwrap();
    std::os::unix::fs::symlink(changed.join("child"), fixture.root.join("link")).unwrap();
    let output = fixture.aida(&["push"]);
    assert!(!output.status.success());
    assert!(
        diagnostics(&output).contains("Origin changed"),
        "{}",
        diagnostics(&output)
    );
    assert_eq!(fixture.git_at(&intended, &["for-each-ref"]), "");
    assert_eq!(fixture.git_at(&other, &["for-each-ref"]), "");
    assert_eq!(fixture.git_at(&wrong, &["for-each-ref"]), "");
    assert_eq!(
        fs::read(fixture.common.join("aida-transport.json")).unwrap(),
        enrolled
    );
}

// trace:STORY-1742 | ai:codex
#[test]
#[cfg(unix)]
fn an_old_lexical_destination_pin_cannot_confirm_the_wrong_repository() {
    let fixture = Fixture::new();
    let (intended, wrong) = fixture.symlink_parent_remotes();
    // Enroll the old normalizer's result, then restore the configured path
    // that the old code incorrectly considered the same destination.
    fixture.git(&["remote", "set-url", "origin", wrong.to_str().unwrap()]);
    fixture.enable();
    fixture.git(&["remote", "set-url", "origin", "link/../hub.git"]);
    assert_eq!(
        fixture.json(&["config", "transport", "show", "--json"])["origin_matches"],
        false
    );
    let output = fixture.aida(&["push"]);
    assert!(!output.status.success());
    assert!(diagnostics(&output).contains("Origin changed"));
    assert_eq!(fixture.git_at(&intended, &["for-each-ref"]), "");
    assert_eq!(fixture.git_at(&wrong, &["for-each-ref"]), "");
}

// trace:STORY-1742 | ai:codex
#[test]
#[cfg(unix)]
fn unresolved_filesystem_components_refuse_without_lexical_parent_collapse() {
    let fixture = Fixture::new();
    let (intended, wrong) = fixture.symlink_parent_remotes();
    fs::write(fixture.root.join("plain-file"), b"not a directory").unwrap();
    std::os::unix::fs::symlink(fixture.root.join("absent"), fixture.root.join("dangling")).unwrap();
    for path in [
        "missing/../hub.git",
        "plain-file/../hub.git",
        "dangling/../hub.git",
    ] {
        fixture.git(&["remote", "set-url", "origin", path]);
        let output = fixture.aida(&["config", "transport", "enable", "work"]);
        assert!(!output.status.success(), "{path}: {}", diagnostics(&output));
        assert!(!fixture.common.join("aida-transport.json").exists());
    }
    assert_eq!(fixture.git_at(&intended, &["for-each-ref"]), "");
    assert_eq!(fixture.git_at(&wrong, &["for-each-ref"]), "");
}

// trace:STORY-1742 | ai:codex
impl Fixture {
    fn store_commit(&self, store: &Path, path: &str, bytes: &[u8]) -> String {
        let target = store.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
        self.git_at(store, &["add", "--all"]);
        self.git_at(store, &["commit", "-qm", "Generic change"]);
        self.git_at(store, &["rev-parse", "HEAD"])
    }
    fn assert_scan_refusal(&self, remote: &Path) {
        assert_eq!(self.git_at(remote, &["for-each-ref"]), "");
        self.assert_scan_refusal_with(remote, &[]);
        assert!(!self.common.join("aida-transport-receipt.json").exists());
    }
    fn assert_scan_refusal_with(&self, remote: &Path, env: &[(&str, &str)]) -> String {
        let before = self.snapshot();
        let objects = self.git_at(remote, &["count-objects", "-v"]);
        let refs = self.git_at(remote, &["for-each-ref"]);
        let mut command = self.command(env!("CARGO_BIN_EXE_aida"), &self.root, &["push"]);
        command.envs(env.iter().copied());
        let mut child = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("scan refusal exceeded watchdog");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success(), "{}", diagnostics(&output));
        assert!(!diagnostics(&output).contains("private-marker"));
        assert!(!diagnostics(&output).contains("host.invalid"));
        assert_eq!(objects, self.git_at(remote, &["count-objects", "-v"]));
        assert_eq!(self.git_at(remote, &["for-each-ref"]), refs);
        assert_eq!(before, self.snapshot());
        diagnostics(&output)
    }

    /// Re-enroll the current store tip as the reviewed trust root.
    fn reenroll_at_store_tip(&self, store: &Path) -> String {
        let root = self.git_at(store, &["rev-parse", "HEAD"]);
        self.json(&["config", "transport", "disable", "--json"]);
        self.json(&[
            "config",
            "transport",
            "enable",
            "work",
            "--baseline",
            &root,
            "--json",
        ]);
        self.enroll_policy();
        root
    }

    /// A commit object built with Git plumbing, e.g. with an opaque header.
    fn raw_commit(&self, body: &str) -> String {
        let path = self.root.parent().unwrap().join("raw-commit");
        fs::write(&path, body).unwrap();
        self.git(&["hash-object", "-t", "commit", "-w", path.to_str().unwrap()])
    }
}

// trace:STORY-1742 | ai:claude
const OPLOG: &str = "operations:\n- id: 019deaa4-dcd0-7541-95f0-ae88f28fc0e6\n  target_id: 019deaa4-9b9d-7fd1-aec5-0aec24f4b255\n  author: aida\n  node_id: '1'\n  lamport: 1\n  timestamp: 2026-05-02T21:42:46.224238843Z\n  kind: !SetStatus\n    status: In-progress\nclock: 1\nnode_id: '1'\n";

/// Every path shape the real store writes, with synthetic content.
const LAYOUT: &[(&str, &str)] = &[
    ("metadata.yaml", "name: Generic\n"),
    ("oplog.yaml", OPLOG),
    (".gitignore", "*.sqlite3\n"),
    ("objects/.gitkeep", ""),
    ("timings/TASK/000/TASK-1.json", "{\"seconds\":12}"),
    (
        "mailbox/019deaa4-dcd0-7541-95f0-ae88f28fc0e6.json",
        "{\"subject\":\"Generic\"}",
    ),
    (
        "coordination/leases/pr-1-019f7dda.toml",
        "holder = \"aida\"\n",
    ),
    ("coordination/drain.lock.toml", "holder = \"aida\"\n"),
    ("registry/queues/role%3Aadvisor.yaml", "items: []\n"),
    ("registry/team.toml", "name = \"Generic\"\n"),
    ("registry/agreed_counters.toml", "TASK = 1\n"),
    ("registry/blocks.yaml", "blocks: []\n"),
    ("registry/blocks.lock", ""),
    ("schedule/night-shift.yaml", "every: daily\n"),
    (".aida/agents/generic-1154604.toml", "kind = \"generic\"\n"),
    (
        ".aida/burndown/20260720T044813Z-019f7dda.jsonl",
        "{\"event\":\"start\"}\n",
    ),
    (".aida/.format-hint-shown", ""),
];

#[test]
fn scanner_refuses_historical_deleted_content_and_merge_side_metadata_before_receive() {
    for merge in [false, true] {
        let fixture = Fixture::new();
        let (store, remote) = fixture.attach_with_remote();
        if merge {
            let side = fixture.git(&[
                "commit-tree",
                "aida-store^{tree}",
                "-p",
                &fixture.store_sha,
                "-m",
                "private-marker",
            ]);
            let tip = fixture.git(&[
                "commit-tree",
                "aida-store^{tree}",
                "-p",
                &fixture.store_sha,
                "-p",
                &side,
                "-m",
                "Generic merge",
            ]);
            fixture.git(&["update-ref", "refs/heads/aida-store", &tip]);
        } else {
            fixture.store_commit(&store, "README.md", b"private-marker\n");
            fixture.git_at(&store, &["rm", "README.md"]);
            fixture.git_at(&store, &["commit", "-qm", "Generic cleanup"]);
        }
        fixture.assert_scan_refusal(&remote);
    }
}

#[test]
fn scanner_checks_decoded_scalars_commit_names_paths_binary_and_signature_encodings() {
    for case in 0..8 {
        let fixture = Fixture::new();
        let (store, remote) = fixture.attach_with_remote();
        match case {
            0 => {
                fixture.store_commit(
                    &store,
                    "objects/BUG/000/BUG-1.yaml",
                    br#"title: "private\x2dmarker""#,
                );
            }
            1 => {
                fixture.store_commit(&store, "README.md", b"host.invalid\n");
            }
            2 => {
                fixture.store_commit(&store, "private-marker", b"Generic\n");
            }
            3 => {
                fixture.store_commit(&store, "README.md", b"\x00\xff");
            }
            4 => {
                fixture.store_commit(&store, "README.md", b"name@host.invalid\n");
            }
            5 => {
                let output = fixture
                    .command(
                        "git",
                        &store,
                        &["commit", "--allow-empty", "-qm", "Generic"],
                    )
                    .env("GIT_AUTHOR_NAME", "private-marker")
                    .output()
                    .unwrap();
                assert!(output.status.success());
            }
            6 => {
                fixture.store_commit(
                    &store,
                    "objects/BUG/000/BUG-1.yaml",
                    b"title: !!binary cHJpdmF0ZS1tYXJrZXI=\n",
                );
            }
            _ => {
                fixture.store_commit(
                    &store,
                    "objects/BUG/000/BUG-1.yaml",
                    b"encoding: base64\ndata: cHJpdmF0ZS1tYXJrZXI=\n",
                );
            }
        }
        fixture.assert_scan_refusal(&remote);
    }
}

#[test]
fn reviewed_non_email_identity_passes_real_store_hook_and_receipt_pins_exact_safe_history() {
    let fixture = Fixture::new();
    let (store, remote) = fixture.attach_with_remote();
    fixture.identity_policy("aida");
    let tip = fixture.store_commit(
        &store,
        "objects/BUG/000/BUG-1.yaml",
        b"title: Generic failure\ndescription: Synthetic reproduction\n",
    );
    let output = fixture.aida(&["push"]);
    assert!(output.status.success(), "{}", diagnostics(&output));
    assert_eq!(fixture.git_at(&remote, &["rev-parse", "aida-store"]), tip);
    let receipt: Value = serde_json::from_slice(
        &fs::read(fixture.common.join("aida-transport-receipt.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["accepted_tip"], tip);
    assert_eq!(receipt["trust_root"], fixture.store_sha);
    assert_eq!(receipt["reference"], "refs/heads/aida-store");
    assert!(!fixture
        .common
        .join("refs/remotes/origin/aida-store")
        .exists());
    assert!(!fixture.common.join("FETCH_HEAD").exists());
    // A stricter policy must scan old exported deltas from the original root.
    let policy_path = fixture.common.join("aida-transport-policy.json");
    let mut policy: Value = serde_json::from_slice(&fs::read(&policy_path).unwrap()).unwrap();
    policy["deny_tokens"] = serde_json::json!(["Synthetic"]);
    fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let before = fs::read(fixture.common.join("aida-transport-receipt.json")).unwrap();
    let output = fixture.aida(&["push"]);
    assert!(!output.status.success(), "{}", diagnostics(&output));
    assert_eq!(
        before,
        fs::read(fixture.common.join("aida-transport-receipt.json")).unwrap()
    );
    assert_eq!(fixture.git_at(&remote, &["rev-parse", "aida-store"]), tip);
}

#[test]
fn missing_policy_forged_tracking_and_unsafe_private_files_never_create_remote_objects() {
    use std::os::unix::fs::PermissionsExt;
    for case in 0..6 {
        let fixture = Fixture::new();
        let (_store, remote) = fixture.attach_with_remote();
        let path = fixture.common.join("aida-transport-policy.json");
        match case {
            0 => {
                fs::remove_file(&path).unwrap();
            }
            1 => {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            }
            2 => {
                let other = fixture.common.join("private-target");
                fs::rename(&path, &other).unwrap();
                std::os::unix::fs::symlink(other, &path).unwrap();
            }
            3 => {
                fs::hard_link(&path, fixture.common.join("second-link")).unwrap();
            }
            4 => {
                fs::remove_file(&path).unwrap();
                assert!(Command::new("mkfifo")
                    .arg("-m")
                    .arg("600")
                    .arg(&path)
                    .status()
                    .unwrap()
                    .success());
            }
            _ => {
                let mut profile: Value = serde_json::from_slice(
                    &fs::read(fixture.common.join("aida-transport.json")).unwrap(),
                )
                .unwrap();
                profile["baseline"] = Value::Null;
                fs::write(
                    fixture.common.join("aida-transport.json"),
                    serde_json::to_vec(&profile).unwrap(),
                )
                .unwrap();
                fixture.git(&[
                    "update-ref",
                    "refs/remotes/origin/aida-store",
                    &fixture.store_sha,
                ]);
            }
        }
        fixture.assert_scan_refusal(&remote);
    }
}

#[test]
fn policy_change_after_remote_acceptance_reports_partial_success_without_receipt() {
    let fixture = Fixture::new();
    let (store, remote) = fixture.attach_with_remote();
    let tip = fixture.store_commit(&store, "README.md", b"Generic reproduction\n");
    let policy = fixture.common.join("aida-transport-policy.json");
    hook(&remote.join("hooks/post-receive"),&format!(
        "#!/bin/sh\npython3 - '{}' <<'PY'\nimport json,sys\np=sys.argv[1]\nd=json.load(open(p))\nd['deny_tokens'].append('new-private-token')\nopen(p,'w').write(json.dumps(d))\nPY\n",policy.display()));
    let output = fixture.aida(&["push"]);
    assert!(!output.status.success(), "{}", diagnostics(&output));
    assert!(diagnostics(&output).contains("publication succeeded"));
    assert_eq!(fixture.git_at(&remote, &["rev-parse", "aida-store"]), tip);
    assert!(!fixture.common.join("aida-transport-receipt.json").exists());
}

#[test]
fn reused_baseline_blob_under_unsafe_path_is_refused() {
    let fixture = Fixture::new();
    let (store, remote) = fixture.attach_with_remote();
    fixture.store_commit(&store, "README.md", b"Generic baseline\n");
    let root = fixture.git_at(&store, &["rev-parse", "HEAD"]);
    fixture.json(&["config", "transport", "disable", "--json"]);
    fixture.json(&[
        "config",
        "transport",
        "enable",
        "work",
        "--baseline",
        &root,
        "--json",
    ]);
    fixture.enroll_policy();
    fixture.git_at(&store, &["mv", "README.md", "private-marker"]);
    fixture.git_at(&store, &["commit", "-qm", "Generic rename"]);
    fixture.assert_scan_refusal(&remote);
}

#[test]
fn incomplete_replaced_grafted_and_alternate_graphs_refuse_before_receive() {
    for case in 0..4 {
        let fixture = Fixture::new();
        let (_store, remote) = fixture.attach_with_remote();
        match case {
            0 => {
                fs::write(
                    fixture.common.join("shallow"),
                    format!("{}\n", fixture.store_sha),
                )
                .unwrap();
            }
            1 => {
                fixture.git(&[
                    "update-ref",
                    &format!("refs/replace/{}", fixture.store_sha),
                    &fixture.store_sha,
                ]);
            }
            2 => {
                fs::write(
                    fixture.common.join("info/grafts"),
                    format!("{}\n", fixture.store_sha),
                )
                .unwrap();
            }
            _ => {
                fs::write(fixture.common.join("objects/info/alternates"), b"\n").unwrap();
            }
        }
        fixture.assert_scan_refusal(&remote);
    }
}

#[test]
#[cfg(unix)]
fn ordinary_shared_clones_remain_ordinary_but_enrolled_shared_metadata_refuses() {
    use std::os::unix::fs::PermissionsExt;
    let ordinary = Fixture::new();
    fs::set_permissions(&ordinary.common, fs::Permissions::from_mode(0o775)).unwrap();
    assert_eq!(
        ordinary.json(&["config", "transport", "show", "--json"])["enabled"],
        false
    );
    let output = ordinary.aida(&["push", "--dry-run"]);
    assert!(
        !diagnostics(&output).contains("Transport private directory"),
        "{}",
        diagnostics(&output)
    );
    let enrolled = Fixture::new();
    let (_store, remote) = enrolled.attach_with_remote();
    fs::set_permissions(&enrolled.common, fs::Permissions::from_mode(0o775)).unwrap();
    enrolled.assert_scan_refusal(&remote);
}

#[test]
#[cfg(unix)]
fn real_store_layout_passes_at_scale_beyond_the_old_object_and_tree_caps() {
    // trace:STORY-1742 | ai:claude
    // Synthetic only: >10,000 trusted paths/objects in one root tree (the old
    // 10,000-object cap refused this), every real path shape, and native
    // oplog tags. New commits rewalk all paths but read only the new delta.
    let fixture = Fixture::new();
    let (store, remote) = fixture.attach_with_remote();
    fixture.identity_policy("aida");
    for n in 1..=10_500u32 {
        let path = store.join(format!("objects/TASK/{:03}/TASK-{n}.yaml", (n - 1) / 1000));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("title: Generic {n}\n")).unwrap();
    }
    for (path, body) in LAYOUT {
        let target = store.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, body).unwrap();
    }
    std::os::unix::fs::symlink("../aida-store-target", store.join(".aida-store")).unwrap();
    fixture.git_at(&store, &["add", "--all"]);
    fixture.git_at(&store, &["commit", "-qm", "Generic baseline"]);
    fixture.reenroll_at_store_tip(&store);
    // New content in every structured format is decoded and scanned.
    fixture.store_commit(
        &store,
        "oplog.yaml",
        format!("{}", OPLOG.replace("In-progress", "Completed")).as_bytes(),
    );
    fixture.store_commit(
        &store,
        "timings/TASK/002/TASK-2347.json",
        b"{\"seconds\":3}",
    );
    fixture.store_commit(
        &store,
        "mailbox/019deaa6-0682-7231-aead-d87744bcdf88.json",
        b"{\"subject\":\"Generic reply\"}",
    );
    fixture.store_commit(
        &store,
        "coordination/leases/task-2347-019f8240.toml",
        b"holder = \"aida\"\n",
    );
    fixture.store_commit(
        &store,
        "registry/queues/role%3Aimplementer.yaml",
        b"items: []\n",
    );
    let tip = fixture.store_commit(
        &store,
        "objects/TASK/010/TASK-10001.yaml",
        b"title: Generic change\n",
    );
    let started = std::time::Instant::now();
    let output = fixture.aida(&["push"]);
    assert!(output.status.success(), "{}", diagnostics(&output));
    assert!(started.elapsed() < std::time::Duration::from_secs(120));
    assert_eq!(fixture.git_at(&remote, &["rev-parse", "aida-store"]), tip);
    // A real-shaped path still scans its user-controlled component.
    fixture.store_commit(
        &store,
        "coordination/leases/private-marker.toml",
        b"holder = \"aida\"\n",
    );
    let receipt = fs::read(fixture.common.join("aida-transport-receipt.json")).unwrap();
    fixture.assert_scan_refusal_with(&remote, &[]);
    assert_eq!(
        receipt,
        fs::read(fixture.common.join("aida-transport-receipt.json")).unwrap()
    );
}

#[test]
fn scanner_refuses_signatures_identity_mismatch_structured_tokens_trailers_addresses_entities_and_limits(
) {
    // trace:STORY-1742 | ai:claude
    for case in 0..13 {
        let fixture = Fixture::new();
        let (store, remote) = fixture.attach_with_remote();
        let mut env = Vec::new();
        let mut expected = None;
        match case {
            0 => {
                // A valid commit object with an opaque signature header.
                let tree = fixture.git(&["rev-parse", "aida-store^{tree}"]);
                let commit = fixture.raw_commit(&format!(
                    "tree {tree}\nparent {}\nauthor Fixture <aida> 1700000000 +0000\ncommitter Fixture <aida> 1700000000 +0000\ngpgsig opaque\n\nGeneric\n",
                    fixture.store_sha
                ));
                fixture.git(&["update-ref", "refs/heads/aida-store", &commit]);
                expected = Some("Unsupported commit metadata");
            }
            1 => {
                let output = fixture
                    .command(
                        "git",
                        &store,
                        &["commit", "--allow-empty", "-qm", "Generic"],
                    )
                    .env("GIT_COMMITTER_NAME", "Other")
                    .output()
                    .unwrap();
                assert!(output.status.success());
                expected = Some("Commit identity differs");
            }
            2 => {
                fixture.store_commit(
                    &store,
                    "coordination/leases/pr-1.toml",
                    br#"title = "private\u002dmarker""#,
                );
            }
            3 => {
                fixture.store_commit(
                    &store,
                    "timings/BUG/000/BUG-1.json",
                    br#"{"title":"private\u002dmarker"}"#,
                );
            }
            4 => {
                fixture.store_commit(&store, "README.md", b"Generic\n");
                fixture.git_at(
                    &store,
                    &[
                        "commit",
                        "--allow-empty",
                        "-qm",
                        "Generic\n\nCo-authored-by: Other <name@host.invalid>",
                    ],
                );
            }
            5 => {
                fixture.store_commit(&store, "README.md", b"Generic 192.0.2.4\n");
            }
            6 => {
                fixture.store_commit(
                    &store,
                    "mailbox/019deaa4-dcd0-7541-95f0-ae88f28fc0e6.json",
                    br#"{"route":"2001:db8::1"}"#,
                );
            }
            7 => {
                fixture.store_commit(&store, "README.md", b"Generic private&#45;marker\n");
            }
            8 => {
                fixture.store_commit(
                    &store,
                    "timings/BUG/000/BUG-1.json",
                    br#"{"title":"private&#x2d;marker"}"#,
                );
            }
            9 => {
                fixture.store_commit(&store, "README.md", "Generic\n".repeat(150_000).as_bytes());
                expected = Some("limit");
            }
            10 => {
                let nested = format!("{}{}", "[".repeat(80), "]".repeat(80));
                fixture.store_commit(&store, "timings/BUG/000/BUG-1.json", nested.as_bytes());
                expected = Some("limit");
            }
            11 => {
                fixture.store_commit(&store, "README.md", b"Generic\n");
                env.push(("AIDA_TRANSPORT_SCAN_DEADLINE_MS", "0"));
                expected = Some("deadline");
            }
            _ => {
                fixture.store_commit(&store, "notes/arbitrary.yaml", b"title: Generic\n");
                expected = Some("unsupported grammar");
            }
        }
        assert_eq!(fixture.git_at(&remote, &["for-each-ref"]), "");
        let output = fixture.assert_scan_refusal_with(&remote, &env);
        assert!(!fixture.common.join("aida-transport-receipt.json").exists());
        if let Some(expected) = expected {
            assert!(output.contains(expected), "case {case}: {output}");
        }
    }
}

#[test]
#[cfg(unix)]
fn legacy_main_created_profile_and_lock_stay_usable_and_disable_clears_them() {
    // trace:STORY-1742 | ai:claude
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let fixture = Fixture::new();
    fixture.enable();
    let profile = fixture.common.join("aida-transport.json");
    let lock = fixture.common.join("aida-transport.lock");
    for path in [&profile, &lock] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    let lock_inode = fs::metadata(&lock).unwrap().ino();
    assert_eq!(
        fixture.json(&["config", "transport", "show", "--json"])["enabled"],
        true
    );
    // Show is read-only: no migration of either legacy file.
    assert_eq!(fs::metadata(&profile).unwrap().mode() & 0o777, 0o644);
    let output = fixture.aida(&["push", "--dry-run"]);
    assert!(output.status.success(), "{}", diagnostics(&output));
    assert_eq!(
        fixture.json(&["config", "transport", "disable", "--json"])["enabled"],
        false
    );
    assert!(!profile.exists());
    // The permanent lock is migrated in place, never unlinked.
    assert_eq!(fs::metadata(&lock).unwrap().ino(), lock_inode);
    assert_eq!(fs::metadata(&lock).unwrap().mode() & 0o777, 0o600);

    // A corrupt but safely stored profile is still cleared by disable.
    fs::write(&profile, b"{not json").unwrap();
    fs::set_permissions(&profile, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!fixture
        .aida(&["config", "transport", "show"])
        .status
        .success());
    assert_eq!(
        fixture.json(&["config", "transport", "disable", "--json"])["enabled"],
        false
    );
    assert!(!profile.exists());

    // Genuinely unsafe storage refuses with a hint and is left untouched;
    // the policy never gets the legacy tolerance.
    for case in 0..3 {
        let fixture = Fixture::new();
        fixture.attach_with_remote();
        let profile = fixture.common.join("aida-transport.json");
        match case {
            0 => fs::set_permissions(&profile, fs::Permissions::from_mode(0o666)).unwrap(),
            1 => {
                let target = fixture.common.join("profile-target");
                fs::rename(&profile, &target).unwrap();
                std::os::unix::fs::symlink(&target, &profile).unwrap();
            }
            _ => fs::set_permissions(
                fixture.common.join("aida-transport-policy.json"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap(),
        }
        let command = if case == 2 { "push" } else { "show" };
        let output = if case == 2 {
            fixture.aida(&[command])
        } else {
            fixture.aida(&["config", "transport", command])
        };
        assert!(!output.status.success(), "case {case}");
        assert!(
            diagnostics(&output).contains("chmod 600"),
            "{}",
            diagnostics(&output)
        );
        if case < 2 {
            let output = fixture.aida(&["config", "transport", "disable"]);
            assert!(!output.status.success(), "case {case}");
            assert!(fs::symlink_metadata(&profile).is_ok());
        }
    }
}

#[test]
// trace:STORY-1742 | ai:claude
fn sanitized_intake_template_and_preview_are_read_only_and_never_echo_refusals() {
    let fixture = Fixture::new();
    let initial = fixture.snapshot();
    let stdout = |output: &Output| String::from_utf8_lossy(&output.stdout).into_owned();

    let template = fixture.aida(&["add", "--sanitized", "--template"]);
    assert!(template.status.success(), "{}", diagnostics(&template));
    for heading in ["## Problem", "## Reproduction", "## Expected", "## Actual"] {
        assert!(stdout(&template).contains(heading), "{heading}");
    }

    let ordinary = fixture.aida(&["add", "--sanitized", "--preview", "Generic failure"]);
    assert!(ordinary.status.success(), "{}", diagnostics(&ordinary));
    assert!(stdout(&ordinary).contains("no enabled WORK transport profile"));

    fixture.enable();
    let clean = fixture.aida(&["add", "--sanitized", "--preview", "Generic failure"]);
    assert!(clean.status.success(), "{}", diagnostics(&clean));
    assert!(stdout(&clean).contains("passes this clone's reviewed policy"));
    assert!(stdout(&clean).contains("title: Generic failure"));

    let refused = fixture.aida(&[
        "add",
        "--sanitized",
        "--preview",
        "Generic failure",
        "--description",
        "seen on private-marker",
    ]);
    assert!(refused.status.success(), "{}", diagnostics(&refused));
    assert!(stdout(&refused).contains("refused (reviewed deny token)"));
    assert!(!diagnostics(&refused).contains("private-marker"));
    assert!(!diagnostics(&refused).contains("Generic failure"));

    for extra in [
        vec!["--owner", "someone"],
        vec!["--tags", "generic"],
        vec!["--status", "approved"],
    ] {
        let mut args = vec!["add", "--sanitized", "--preview", "Generic failure"];
        args.extend(extra.iter());
        let output = fixture.aida(&args);
        assert!(!output.status.success(), "{extra:?}");
        assert!(diagnostics(&output).contains("--sanitized"), "{extra:?}");
    }

    // Nothing above touched tracked state, refs or the clone-local counter.
    assert_eq!(initial, fixture.snapshot());
    for name in ["aida-work-dispenser.toml", "aida-work-node.toml"] {
        assert!(!fixture.common.join(name).exists(), "{name}");
    }
}

#[test]
// trace:STORY-1742 | ai:claude
fn sanitized_intake_write_files_one_generic_draft_only_when_enrolled() {
    let fixture = Fixture::new();
    let store = fixture.root.join(".aida-store");
    fixture.git(&[
        "worktree",
        "add",
        "-q",
        store.to_str().unwrap(),
        "aida-store",
    ]);
    fs::write(store.join("metadata.yaml"), "title: Generic\n").unwrap();
    // An attached store is recognised by its object tree.
    fs::create_dir(store.join("objects")).unwrap();
    fixture.git_at(&store, &["add", "metadata.yaml"]);
    fixture.git_at(&store, &["commit", "-qm", "seed store"]);
    let head = || fixture.git_at(&store, &["rev-parse", "HEAD"]);

    // Without a profile a sanitized write refuses before any change.
    let seeded = head();
    let refused = fixture.aida(&["add", "--sanitized", "Generic failure"]);
    assert!(!refused.status.success());
    assert!(
        diagnostics(&refused).contains("enabled WORK transport profile"),
        "{}",
        diagnostics(&refused)
    );
    assert_eq!(head(), seeded);

    let enabled = fixture.json(&[
        "config",
        "transport",
        "enable",
        "work",
        "--baseline",
        &seeded,
        "--json",
    ]);
    assert_eq!(enabled["enabled"], true);
    fixture.enroll_policy();

    // Preview reads the store a write would use, not the working directory:
    // from outside the project, an explicit store still previews enrolled.
    let outside = fixture.aida_at(
        &fixture.home,
        &[
            "--file",
            store.to_str().unwrap(),
            "add",
            "--sanitized",
            "--preview",
            "Generic failure",
        ],
    );
    assert!(outside.status.success(), "{}", diagnostics(&outside));
    assert!(
        String::from_utf8_lossy(&outside.stdout).contains("passes this clone's reviewed policy"),
        "{}",
        diagnostics(&outside)
    );

    // Unsupported flags and refused text change nothing and never echo.
    let flagged = fixture.aida(&[
        "add",
        "--sanitized",
        "Generic failure",
        "--owner",
        "someone",
    ]);
    assert!(!flagged.status.success());
    assert!(diagnostics(&flagged).contains("--owner is not available with --sanitized"));
    let unsafe_text = fixture.aida(&[
        "add",
        "--sanitized",
        "Generic failure",
        "--description",
        "seen on private-marker",
    ]);
    assert!(!unsafe_text.status.success());
    assert!(!diagnostics(&unsafe_text).contains("private-marker"));
    assert_eq!(head(), seeded);
    for name in ["aida-work-dispenser.toml", "aida-work-node.toml"] {
        assert!(!fixture.common.join(name).exists(), "{name}");
    }

    // An enrolled write files exactly one draft in one pseudonymous commit.
    let added = fixture.aida(&[
        "add",
        "--sanitized",
        "Generic failure",
        "--description",
        "Synthetic reproduction",
    ]);
    assert!(added.status.success(), "{}", diagnostics(&added));
    let stdout = String::from_utf8_lossy(&added.stdout).into_owned();
    let id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Added: "))
        .expect("the write reports its identifier")
        .trim()
        .to_owned();
    assert!(id.starts_with("BUG-") && id.ends_with("-001"), "{id}");
    assert_eq!(
        fixture.git_at(&store, &["rev-list", "--count", &format!("{seeded}..HEAD")]),
        "1"
    );
    assert_eq!(
        fixture.git_at(&store, &["log", "-1", "--format=%an %ae %cn %ce"]),
        "Fixture aida Fixture aida"
    );
    let changed = fixture.git_at(&store, &["show", "--name-only", "--format=", "HEAD"]);
    let object = changed
        .lines()
        .find(|path| path.starts_with("objects/"))
        .expect("one object written");
    let row = fs::read_to_string(store.join(object)).unwrap();
    assert!(row.contains("status: Draft"), "{row}");
    assert!(row.contains("filing_provenance_omission_reason"), "{row}");

    // The MCP route takes the same prepared path: a refused add changes
    // nothing, an admitted add allocates from this clone's own counter.
    let mcp = |title: &str| {
        use std::io::Write;
        let request = |id: u32, method: &str, params: Value| {
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
                .to_string()
        };
        let input = [
            request(1, "initialize", serde_json::json!({})),
            request(
                2,
                "tools/call",
                serde_json::json!({"name": "add_requirement", "arguments": {
                    "title": title, "description": "Synthetic reproduction", "type": "bug"}}),
            ),
        ]
        .join("\n");
        let mut child = fixture
            .command(env!("CARGO_BIN_EXE_aida"), &fixture.root, &["mcp-serve"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{input}\n").as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let reply = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|reply| reply["id"] == 2)
            .unwrap_or_else(|| panic!("no tools/call reply: {}", diagnostics(&output)));
        (reply["result"].to_string(), diagnostics(&output))
    };
    let accepted = head();
    let (refused, _) = mcp("private-marker failure");
    assert!(refused.contains("isError\":true"), "{refused}");
    assert!(!refused.contains("private-marker"), "{refused}");
    assert_eq!(head(), accepted);
    let (added, log) = mcp("Second generic failure");
    assert!(added.contains("Requirement added: BUG-"), "{added}\n{log}");
    assert!(added.contains("-002"), "{added}");
    assert_eq!(
        fixture.git_at(
            &store,
            &["rev-list", "--count", &format!("{accepted}..HEAD")]
        ),
        "1"
    );
}

#[test]
fn work_intake_and_push_scan_agree_on_what_a_write_adds() {
    // trace:STORY-1742 | ai:claude
    let fixture = Fixture::new();
    let (store, remote) = fixture.attach_with_remote();
    fixture.identity_policy("aida");
    // Trusted history today's policy would refuse, in both delta-scanned files.
    let record = |id: &str| {
        OPLOG["operations:\n".len()..OPLOG.find("clock: ").unwrap()]
            .replace("019deaa4-dcd0-7541-95f0-ae88f28fc0e6", id)
            .replace("In-progress", "private-marker")
    };
    let history = OPLOG.replace("In-progress", "private-marker");
    fs::write(store.join("oplog.yaml"), &history).unwrap();
    fixture.store_commit(
        &store,
        "metadata.yaml",
        b"title: Generic\ndescription: Seen on private-marker\n",
    );
    fs::create_dir_all(store.join("objects")).unwrap();
    fixture.reenroll_at_store_tip(&store);

    // An admitted WORK write rewrites both files around that history, and
    // the push gate applies the same delta rule, so the write publishes.
    let added = fixture.aida(&[
        "add",
        "--sanitized",
        "Generic failure",
        "--description",
        "Synthetic reproduction",
    ]);
    assert!(added.status.success(), "{}", diagnostics(&added));
    let changed = fixture.git_at(&store, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(
        changed.lines().any(|path| path == "oplog.yaml"),
        "{changed}"
    );
    let oplog = fs::read_to_string(store.join("oplog.yaml")).unwrap();
    assert!(oplog.starts_with(&history[..history.find("clock: ").unwrap()]));
    let output = fixture.aida(&["push"]);
    assert!(output.status.success(), "{}", diagnostics(&output));
    let tip = fixture.git_at(&store, &["rev-parse", "HEAD"]);
    assert_eq!(fixture.git_at(&remote, &["rev-parse", "aida-store"]), tip);

    // A write that adds refused prose is refused at intake ...
    let refused = fixture.aida(&[
        "add",
        "--sanitized",
        "Generic failure",
        "--description",
        "seen on private-marker",
    ]);
    assert!(!refused.status.success());
    assert!(!diagnostics(&refused).contains("private-marker"));
    assert_eq!(fixture.git_at(&store, &["rev-parse", "HEAD"]), tip);
    // ... and the same addition committed around intake is refused at push.
    let trailer = oplog.rfind("\nclock: ").unwrap() + 1;
    let appended = format!(
        "{}{}{}",
        &oplog[..trailer],
        record("019deaa4-dcd0-7541-95f0-ae88f28fc0e7"),
        &oplog[trailer..]
    );
    fixture.store_commit(&store, "oplog.yaml", appended.as_bytes());
    fixture.assert_scan_refusal_with(&remote, &[]);
    assert_eq!(fixture.git_at(&remote, &["rev-parse", "aida-store"]), tip);
}
