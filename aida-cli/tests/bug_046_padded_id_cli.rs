#![cfg(target_os = "linux")]
//! Black-box coverage for numeric spec-ID identity at the command level:
//! IDs that differ only by leading zeros (`TASK-0001`, `TASK-1`,
//! `TASK-00001`) name ONE spec. `show`, `edit`, `comment add` and `rel add`
//! find an object stored under either padding by any other padding, a
//! `trace:` comment written with a different padding links to it, and a
//! commit trailer naming it under a different padding completes it in the
//! merge-time completion replay (`aida db reconcile-status`).
// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = test_git::git_command()
        .current_dir(dir)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
        .env("GIT_AUTHOR_DATE", "2026-09-10T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-09-10T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn text(out: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        git_in(
            &repo,
            &["commit", "-q", "--allow-empty", "-m", "chore: base"],
        );
        test_git::enroll_synthetic_home(&repo);
        let fx = Fx {
            _tmp: tmp,
            root,
            repo,
        };
        fx.ok(&[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-post-hooks",
            "--no-agent-config",
            "--no-roles",
        ]);
        fx
    }

    fn aida(&self, args: &[&str]) -> Output {
        let home = self.root.join("home");
        Command::new(env!("CARGO_BIN_EXE_aida"))
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .env("USER", "fixture-human")
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("XDG_RUNTIME_DIR", home.join("run"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_OUTPUT_FORMAT", "human")
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::null())
            .args(args)
            .output()
            .expect("run aida")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.aida(args);
        assert!(out.status.success(), "aida {args:?}\n{}", text(&out));
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn store(&self) -> PathBuf {
        self.repo.join(".aida-store")
    }

    fn object_path(&self, spec_id: &str) -> PathBuf {
        self.store()
            .join("objects/TASK/000")
            .join(format!("{spec_id}.yaml"))
    }

    fn object(&self, spec_id: &str) -> String {
        std::fs::read_to_string(self.object_path(spec_id)).unwrap()
    }

    /// Seed one TASK object filed under the literal `spec_id` (its padding is
    /// kept on disk) and commit it to the store branch.
    fn seed(&self, spec_id: &str, title: &str, status: &str) -> String {
        let uuid = uuid::Uuid::new_v4().to_string();
        let path = self.object_path(spec_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "id: {uuid}\nspec_id: {spec_id}\nagreed_id: {spec_id}\ntitle: {title}\n\
                 description: fixture\nstatus: {status}\npriority: Medium\nowner: ''\n\
                 feature: Uncategorized\ncreated_at: 2026-09-01T00:00:00Z\n\
                 modified_at: 2026-09-01T00:00:00Z\nreq_type: Task\n"
            ),
        )
        .unwrap();
        git_in(&self.store(), &["add", "-A"]);
        git_in(
            &self.store(),
            &["commit", "-q", "-m", &format!("seed {spec_id}")],
        );
        uuid
    }

    fn status(&self, spec_id: &str) -> String {
        self.object(spec_id)
            .lines()
            .find_map(|l| l.strip_prefix("status: "))
            .unwrap()
            .trim()
            .to_string()
    }

    /// Only the seeded files exist: no write created a re-padded twin.
    fn assert_no_twin_files(&self, expected: &[&str]) {
        let mut files: Vec<String> = std::fs::read_dir(self.store().join("objects/TASK/000"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        files.sort();
        let mut want: Vec<String> = expected.iter().map(|s| format!("{s}.yaml")).collect();
        want.sort();
        assert_eq!(files, want);
    }
}

/// `TASK-0001` is stored padded, `TASK-2` unpadded; each is read and written
/// through other paddings and cases.
fn mixed_fixture() -> (Fx, String, String) {
    let fx = Fx::new();
    let padded = fx.seed("TASK-0001", "padded original", "Approved");
    let plain = fx.seed("TASK-2", "plain original", "Approved");
    fx.ok(&["cache", "rebuild"]);
    (fx, padded, plain)
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-046.ac8d44cf | ai:claude
#[test]
fn show_finds_a_spec_under_any_padding() {
    let (fx, padded, plain) = mixed_fixture();
    for id in ["TASK-1", "task-001", "TASK-00001", "TASK-0001"] {
        let out = fx.ok(&["show", id, "--no-git"]);
        assert!(out.contains("padded original"), "show {id}:\n{out}");
        assert!(!out.contains("plain original"), "show {id}:\n{out}");
    }
    for id in ["TASK-0002", "task-02", "TASK-2"] {
        let out = fx.ok(&["show", id, "--no-git"]);
        assert!(out.contains("plain original"), "show {id}:\n{out}");
    }
    let json = fx.ok(&["show", "TASK-00002", "--no-git", "--format", "json"]);
    assert!(json.contains(&plain), "{json}");
    let json = fx.ok(&["show", "TASK-1", "--no-git", "--format", "json"]);
    assert!(json.contains(&padded), "{json}");
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-046.ac8d44cf | ai:claude
#[test]
fn edit_comment_and_link_write_the_stored_object_under_any_padding() {
    let (fx, _padded, plain) = mixed_fixture();
    fx.ok(&["edit", "TASK-1", "--title", "padded renamed"]);
    assert!(fx.object("TASK-0001").contains("padded renamed"));
    fx.ok(&["edit", "TASK-0002", "--title", "plain renamed"]);
    assert!(fx.object("TASK-2").contains("plain renamed"));

    fx.ok(&["comment", "add", "task-00001", "note on the padded one"]);
    assert!(fx.object("TASK-0001").contains("note on the padded one"));
    fx.ok(&["comment", "add", "TASK-002", "note on the plain one"]);
    assert!(fx.object("TASK-2").contains("note on the plain one"));

    fx.ok(&["rel", "add", "TASK-01", "TASK-0002", "--type", "related"]);
    let linked = fx.object("TASK-0001");
    assert!(
        linked.contains(&plain),
        "relationship targets TASK-2:\n{linked}"
    );

    // Every write landed on the stored file; no re-padded twin was created.
    fx.assert_no_twin_files(&["TASK-0001", "TASK-2"]);
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-046.ac8d44cf | ai:claude
#[test]
fn trace_comment_under_another_padding_links_the_spec() {
    let (fx, _padded, _plain) = mixed_fixture();
    let src = fx.repo.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.rs"),
        "// trace:TASK-1 | ai:claude\nfn a() {}\n// trace:TASK-0002 | ai:claude\nfn b() {}\n",
    )
    .unwrap();
    let out = fx.ok(&["trace", "scan", "src", "--update"]);
    assert!(!out.contains("not found"), "{out}");
    assert!(fx.object("TASK-0001").contains("src/lib.rs"), "{out}");
    assert!(fx.object("TASK-2").contains("src/lib.rs"), "{out}");
    fx.assert_no_twin_files(&["TASK-0001", "TASK-2"]);
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-046.ac8d44cf | ai:claude
#[test]
fn commit_trailer_under_another_padding_completes_the_spec_at_merge() {
    let fx = Fx::new();
    fx.seed("TASK-0003", "padded done", "Done");
    fx.seed("TASK-4", "plain done", "Done");
    fx.seed("TASK-5", "untouched", "Done");
    fx.ok(&["cache", "rebuild"]);
    git_in(
        &fx.repo,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fix: padded trailer (TASK-00003)",
        ],
    );
    git_in(
        &fx.repo,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fix: plain trailer (TASK-0004) (#12)",
        ],
    );
    let out = fx.aida(&["db", "reconcile-status"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(fx.status("TASK-0003"), "Completed", "{}", text(&out));
    assert_eq!(fx.status("TASK-4"), "Completed", "{}", text(&out));
    assert_eq!(fx.status("TASK-5"), "Done", "{}", text(&out));
    fx.assert_no_twin_files(&["TASK-0003", "TASK-4", "TASK-5"]);
}
