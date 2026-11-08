//! Independent intake transport against local remotes; no real store or network.
// trace:TASK-3096 | ai:codex
mod support;
#[path = "support/test_git.rs"]
mod test_git;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    home: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&home).unwrap();
        let f = Self {
            _temp: temp,
            root,
            home,
        };
        f.git(&["init", "-q", "-b", "main"]);
        f.git(&["commit", "-q", "--allow-empty", "-m", "product"]);
        f.git(&["branch", "aida-store"]);
        f
    }
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("AIDA_")
                || key.to_string_lossy().starts_with("GIT_")
                || key.to_string_lossy().starts_with("REQ_")
            {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(&self.root)
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_USER", "intake-test")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        if self.root.join(".aida/config.toml").exists() {
            let grant =
                support::ensure_seat_for(&self.home, &self.root, "intake-test", "advisor", &[])
                    .unwrap();
            let grant_path = self.home.join(format!(".aida/session-grants/{grant}.json"));
            let mut record: Value =
                serde_json::from_slice(&fs::read(&grant_path).unwrap()).unwrap();
            let pid = std::process::id();
            let start = aida_core::liveness::process_start_identity(pid).unwrap();
            let ns = aida_core::local_pid_ns().unwrap();
            let start = if ns.trim().parse::<u64>().unwrap()
                == aida_core::process_probe::INITIAL_PID_NAMESPACE_INODE
            {
                start
            } else {
                format!("pidns-{}/{start}", ns.trim())
            };
            record["binding"] = json!([format!("proc:{pid}@{start}")]);
            fs::write(grant_path, serde_json::to_vec(&record).unwrap()).unwrap();
            cmd.env("AIDA_SESSION_GRANT", grant)
                .env("AIDA_SESSION_ROLE", "advisor");
        }
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }
    fn git(&self, args: &[&str]) -> String {
        git(&self.root, args)
    }
    fn intake(&self) -> PathBuf {
        self.root.join(".git/aida-intake/worktree")
    }
    fn enroll(&self, remote: &Path) {
        good(&self.run(&[
            "intent",
            "enroll",
            "--remote",
            remote.to_str().unwrap(),
            "--reviewed",
        ]));
    }
    fn note(&self, title: &str) -> String {
        fs::write(
            self.root.join("body.md"),
            "Untrusted text: $(touch NEVER)\n",
        )
        .unwrap();
        let title_arg = format!("--title={title}");
        let out = self.run(&[
            "intent",
            "note",
            "--category",
            "observation",
            &title_arg,
            "--file",
            "body.md",
        ]);
        good(&out);
        fs::write(self.root.join("envelope.json"), &out.stdout).unwrap();
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["checksum"]
            .as_str()
            .unwrap()
            .into()
    }
    fn publish(&self) -> Output {
        self.run(&["intent", "publish", "--file", "envelope.json", "--reviewed"])
    }
    fn snapshot(&self) -> (String, Option<Vec<u8>>) {
        (
            self.git(&["show-ref"]),
            fs::read(self.root.join(".git/index")).ok(),
        )
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let name = if root.file_name().is_some_and(|name| name == "worktree") {
        "AIDA Intake"
    } else {
        "Fixture"
    };
    let email = if name == "AIDA Intake" {
        "intake@invalid"
    } else {
        "fixture@invalid"
    };
    let out = test_git::git_command()
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", name)
        .env("GIT_AUTHOR_EMAIL", email)
        .env("GIT_COMMITTER_NAME", name)
        .env("GIT_COMMITTER_EMAIL", email)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn good(out: &Output) {
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
fn bad(out: &Output, reason: &str) {
    assert!(!out.status.success(), "unexpected success");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(reason),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn remote(temp: &TempDir) -> PathBuf {
    let path = temp.path().join("remote.git");
    git(
        temp.path(),
        &["init", "--bare", "-q", path.to_str().unwrap()],
    );
    path
}

#[test]
fn round_trip_receipts_duplicates_and_product_store_index_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let sender = Fixture::new();
    let receiver = Fixture::new();
    let before = sender.snapshot();
    let other_before = receiver.snapshot();
    bad(&sender.run(&["intent", "publish"]), "not enrolled");
    sender.enroll(&remote);
    receiver.enroll(&remote);
    let checksum = sender.note("Review this");
    bad(
        &sender.run(&["intent", "publish", "--file", "envelope.json"]),
        "--reviewed",
    );
    good(&sender.publish());
    let tip = git(&sender.intake(), &["rev-parse", "HEAD"]);
    good(&sender.publish());
    assert_eq!(tip, git(&sender.intake(), &["rev-parse", "HEAD"]));
    assert_eq!(
        git(&remote, &["for-each-ref", "--format=%(refname)"]),
        "refs/heads/aida-intent"
    );
    good(&receiver.run(&["intent", "pull"]));
    let out = receiver.run(&["--format", "json", "intent", "list"]);
    good(&out);
    let rows: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(rows[0]["state"], "pending");
    assert_eq!(rows[0]["checksum"], checksum);
    good(&receiver.run(&["intent", "process", &checksum, "--action", "pending"]));
    assert!(!receiver
        .intake()
        .join(format!("receipts/{checksum}.json"))
        .exists());
    good(&receiver.run(&["intent", "process", &checksum, "--action", "dismiss"]));
    good(&receiver.run(&["intent", "process", &checksum, "--action", "dismiss"]));
    bad(
        &receiver.run(&["intent", "process", &checksum, "--action", "create"]),
        "already processed",
    );
    let out = receiver.run(&["intent", "list", "--json"]);
    good(&out);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!([])
    );
    let out = receiver.run(&["intent", "list", "--all", "--json"]);
    good(&out);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()[0]["state"],
        "processed"
    );
    good(&receiver.run(&["intent", "publish"]));
    good(&sender.run(&["intent", "pull"]));
    assert_eq!(sender.snapshot(), before);
    assert_eq!(receiver.snapshot(), other_before);
    assert!(!sender.root.join(".aida").exists());
    assert!(!receiver.root.join("NEVER").exists());
    assert!(!sender.root.join(".git/FETCH_HEAD").exists());
    assert!(sender
        .intake()
        .join(format!("receipts/{checksum}.json"))
        .is_file());
}

#[test]
fn concurrent_duplicate_publication_serializes_interleaves_and_rewrite_refuses() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let a = Fixture::new();
    let b = Fixture::new();
    a.enroll(&remote);
    b.enroll(&remote);
    a.note("one");
    let mut children: Vec<_> = (0..3)
        .map(|_| {
            a.cmd(&["intent", "publish", "--file", "envelope.json", "--reviewed"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children.drain(..) {
        good(&child.wait_with_output().unwrap());
    }
    assert_eq!(git(&a.intake(), &["rev-list", "--count", "HEAD"]), "1");
    good(&b.run(&["intent", "pull"]));
    a.note("two");
    good(&a.publish());
    // b never pulled "two": publication syncs first and appends on the new tip.
    // trace:TASK-3096 | ai:claude
    b.note("three");
    good(&b.publish());
    good(&a.run(&["intent", "pull"]));
    assert_eq!(git(&a.intake(), &["rev-list", "--count", "HEAD"]), "3");
    assert_eq!(
        git(&a.intake(), &["rev-parse", "HEAD"]),
        git(&b.intake(), &["rev-parse", "HEAD"])
    );
    // Divergence now only arises from a rewrite outside AIDA (force push).
    let four = a.note("four");
    git(&a.intake(), &["reset", "-q", "--hard", "HEAD~1"]);
    fs::copy(
        a.root.join("envelope.json"),
        a.intake().join(format!("observations/{four}.json")),
    )
    .unwrap();
    git(&a.intake(), &["add", "."]);
    git(
        &a.intake(),
        &["commit", "-qm", "Record reviewed intake data"],
    );
    git(
        &a.intake(),
        &[
            "push",
            "--force",
            remote.to_str().unwrap(),
            "HEAD:refs/heads/aida-intent",
        ],
    );
    let tip = git(&b.intake(), &["rev-parse", "HEAD"]);
    bad(&b.run(&["intent", "pull"]), "diverged");
    b.note("five");
    bad(&b.publish(), "diverged");
    assert_eq!(tip, git(&b.intake(), &["rev-parse", "HEAD"]));
}

/// Review round 2 finding 1: the receiver processes while the sender keeps
/// publishing without pulling. No step diverges or needs manual recovery.
// trace:TASK-3096 | ai:claude
#[test]
fn two_site_process_and_publication_interleave_without_divergence() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let work = Fixture::new();
    let home = Fixture::new();
    work.enroll(&remote);
    home.enroll(&remote);
    home.git(&["branch", "-D", "aida-store"]);
    good(&home.run(&[
        "init",
        "--no-skills",
        "--no-hooks",
        "--no-agent-config",
        "--no-roles",
    ]));
    let first = work.note("First observation");
    good(&work.publish());
    // Home never ran pull: process synchronizes, writes, and publishes the receipt.
    good(&home.run(&["intent", "process", &first, "--action", "create"]));
    // Work is stale (no receipt) and publishes between home's process and publish.
    let second = work.note("Second observation");
    good(&work.publish());
    good(&home.run(&["intent", "publish"]));
    good(&home.run(&["intent", "pull"]));
    let out = home.run(&["intent", "list", "--json"]);
    good(&out);
    let rows: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["checksum"], second);
    good(&home.run(&["intent", "process", &second, "--action", "dismiss"]));
    good(&work.run(&["intent", "pull"]));
    for checksum in [&first, &second] {
        assert!(work
            .intake()
            .join(format!("receipts/{checksum}.json"))
            .is_file());
    }
    let published = git(
        &remote,
        &["ls-tree", "-r", "--name-only", "refs/heads/aida-intent"],
    );
    assert!(published.contains(&format!("receipts/{first}.json")));
    assert!(published.contains(&format!("receipts/{second}.json")));
    // Repeating the create on the synchronized home reuses the receipt.
    good(&home.run(&["intent", "process", &first, "--action", "create"]));
    let store_path = aida_core::store_locate::detect_distributed_store_from(&home.root).unwrap();
    let backend = aida_core::GitBackend::new(&store_path).unwrap();
    let store = aida_core::DatabaseBackend::load(&backend).unwrap();
    assert_eq!(
        store
            .requirements
            .iter()
            .filter(|r| r.title == "First observation")
            .count(),
        1
    );
}

/// A receipt that cannot be published after a confirmed spec write keeps the
/// journal; rerunning the same command publishes it without a second write.
// trace:TASK-3096 | ai:claude
#[test]
fn unpublishable_receipt_retries_without_a_second_spec_write() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let work = Fixture::new();
    let home = Fixture::new();
    work.enroll(&remote);
    home.enroll(&remote);
    home.git(&["branch", "-D", "aida-store"]);
    good(&home.run(&[
        "init",
        "--no-skills",
        "--no-hooks",
        "--no-agent-config",
        "--no-roles",
    ]));
    let checksum = work.note("Receipt retry");
    good(&work.publish());
    // Fetch still works from a read-only object database; push cannot write.
    let objects = remote.join("objects");
    fs::set_permissions(&objects, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(objects.join("probe"), b"").is_ok() {
        // Running as a superuser: permissions cannot simulate a refused push.
        fs::remove_file(objects.join("probe")).unwrap();
        fs::set_permissions(&objects, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    good(&home.run(&["intent", "pull"]));
    let checkpoint = home.root.join(".git/aida-intake/validated-tip");
    let accepted = fs::read(&checkpoint).unwrap();
    let failed = home.run(&["intent", "process", &checksum, "--action", "create"]);
    assert_eq!(fs::read(&checkpoint).unwrap(), accepted);
    fs::set_permissions(&objects, fs::Permissions::from_mode(0o755)).unwrap();
    bad(&failed, "rerun the same process command");
    bad(&failed, "Intake push refused:");
    assert!(!String::from_utf8_lossy(&failed.stderr).contains("lost the race"));
    assert!(home
        .root
        .join(format!(".git/aida-intake/operation-{checksum}.json"))
        .exists());
    assert!(!home
        .intake()
        .join(format!("receipts/{checksum}.json"))
        .exists());
    let store_path = aida_core::store_locate::detect_distributed_store_from(&home.root).unwrap();
    let count = || {
        let backend = aida_core::GitBackend::new(&store_path).unwrap();
        aida_core::DatabaseBackend::load(&backend)
            .unwrap()
            .requirements
            .iter()
            .filter(|r| r.title == "Receipt retry")
            .count()
    };
    assert_eq!(count(), 1);
    good(&home.run(&["intent", "process", &checksum, "--action", "create"]));
    assert_eq!(count(), 1);
    assert!(!home
        .root
        .join(format!(".git/aida-intake/operation-{checksum}.json"))
        .exists());
    good(&work.run(&["intent", "pull"]));
    assert!(work
        .intake()
        .join(format!("receipts/{checksum}.json"))
        .is_file());
}

#[test]
fn configured_sensitive_content_conflicting_bytes_and_unknown_schema_refuse() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let f = Fixture::new();
    good(&f.run(&[
        "intent",
        "enroll",
        "--remote",
        remote.to_str().unwrap(),
        "--reviewed",
        "--deny-pattern",
        "SECRET",
    ]));
    f.note("SECRET");
    bad(&f.publish(), "Sensitive content");
    f.note("clean");
    good(&f.publish());
    let raw = fs::read(f.root.join("envelope.json")).unwrap();
    let value: Value = serde_json::from_slice(&raw).unwrap();
    fs::write(
        f.root.join("envelope.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
    bad(&f.publish(), "Conflicting bytes");
    let mut value = value;
    value["unexpected"] = json!(true);
    fs::write(
        f.root.join("envelope.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    bad(&f.publish(), "Invalid envelope schema");
    bad(
        &f.run(&["intent", "enroll", "--remote", "/different", "--reviewed"]),
        "already enrolled",
    );
}

#[test]
fn hostile_intermediate_history_never_moves_accepted_branch() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let sender = Fixture::new();
    let receiver = Fixture::new();
    sender.enroll(&remote);
    receiver.enroll(&remote);
    sender.note("safe");
    good(&sender.publish());
    good(&receiver.run(&["intent", "pull"]));
    let accepted = git(&receiver.intake(), &["rev-parse", "HEAD"]);
    // A hostile file is introduced then deleted: validating only the tip would miss it.
    fs::write(sender.intake().join("leak.txt"), "private").unwrap();
    git(&sender.intake(), &["add", "leak.txt"]);
    git(
        &sender.intake(),
        &["commit", "-qm", "Record reviewed intake data"],
    );
    git(&sender.intake(), &["rm", "-q", "leak.txt"]);
    git(
        &sender.intake(),
        &["commit", "-qm", "Record reviewed intake data"],
    );
    git(
        &sender.intake(),
        &[
            "push",
            remote.to_str().unwrap(),
            "HEAD:refs/heads/aida-intent",
        ],
    );
    bad(&receiver.run(&["intent", "pull"]), "Unexpected intake path");
    assert_eq!(accepted, git(&receiver.intake(), &["rev-parse", "HEAD"]));
}

#[test]
fn incoming_policy_violation_and_symlink_modes_refuse_before_acceptance() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let sender = Fixture::new();
    let receiver = Fixture::new();
    sender.enroll(&remote);
    good(&receiver.run(&[
        "intent",
        "enroll",
        "--remote",
        remote.to_str().unwrap(),
        "--reviewed",
        "--deny-pattern",
        "SECRET",
    ]));
    sender.note("SECRET");
    good(&sender.publish());
    bad(&receiver.run(&["intent", "pull"]), "Sensitive content");
    assert!(receiver.run(&["intent", "list", "--json"]).status.success());
    // Refuse a replaced private directory, before any Git helper can use it.
    let third = Fixture::new();
    std::os::unix::fs::symlink(temp.path(), third.root.join(".git/aida-intake")).unwrap();
    bad(
        &third.run(&[
            "intent",
            "enroll",
            "--remote",
            remote.to_str().unwrap(),
            "--reviewed",
        ]),
        "private directory unavailable",
    );
}

#[test]
fn explicit_create_comment_and_recovery_do_not_repeat_spec_writes() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let f = Fixture::new();
    f.enroll(&remote);
    f.git(&["branch", "-D", "aida-store"]);
    good(&f.run(&[
        "init",
        "--no-skills",
        "--no-hooks",
        "--no-agent-config",
        "--no-roles",
    ]));
    let store_before_publication = f.snapshot();
    let checksum = f.note("--an-untrusted-title");
    good(&f.publish());
    assert_eq!(f.snapshot(), store_before_publication);
    let out = f.run(&[
        "intent", "process", &checksum, "--action", "create", "--json",
    ]);
    good(&out);
    let receipt: Value = serde_json::from_slice(&out.stdout).unwrap();
    let receipt_bytes = fs::read(f.intake().join(format!("receipts/{checksum}.json"))).unwrap();
    let wire_receipt: Value = serde_json::from_slice(&receipt_bytes).unwrap();
    let destination = wire_receipt["destination"].as_str().unwrap();
    let store_path = aida_core::store_locate::detect_distributed_store_from(&f.root).unwrap();
    let load = || {
        let backend = aida_core::GitBackend::new(&store_path).unwrap();
        aida_core::DatabaseBackend::load(&backend).unwrap()
    };
    let first = load();
    let req = first
        .get_requirement_unambiguous(destination)
        .unwrap()
        .unwrap();
    assert_eq!(receipt["destination"], req.friendly_id());
    assert_eq!(req.title, "--an-untrusted-title");
    assert_eq!(req.status, aida_core::RequirementStatus::Draft);
    let count = first.requirements.len();
    good(&f.run(&["intent", "process", &checksum, "--action", "create"]));
    assert_eq!(load().requirements.len(), count);
    let comment_checksum = f.note("Comment title");
    good(&f.publish());
    let out = f.run(&[
        "intent",
        "process",
        &comment_checksum,
        "--action",
        "comment",
        "--target",
        destination,
    ]);
    good(&out);
    let comments = load()
        .get_requirement_unambiguous(destination)
        .unwrap()
        .unwrap()
        .comments
        .len();
    good(&f.run(&[
        "intent",
        "process",
        &comment_checksum,
        "--action",
        "comment",
        "--target",
        destination,
    ]));
    assert_eq!(
        load()
            .get_requirement_unambiguous(destination)
            .unwrap()
            .unwrap()
            .comments
            .len(),
        comments
    );

    // Simulate crash after a canonical comment write, before receipt persistence:
    // a durable journal plus exact operation marker recovers without re-execution.
    let recover = f.note("Recover a confirmed operation");
    good(&f.publish());
    let operation = uuid::Uuid::new_v4().to_string();
    let journal = json!({"version":1,"checksum":recover,"action":"comment","target":destination,"operation":operation,"store":store_path.canonicalize().unwrap()});
    let private =
        aida_core::transport_policy::Private::open(&f.root.join(".git/aida-intake")).unwrap();
    private
        .write(
            &format!("operation-{recover}.json"),
            &serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
    let backend = aida_core::GitBackend::new(&store_path).unwrap();
    aida_core::DatabaseBackend::update_atomically(&backend, |store| {
        store
            .get_requirement_unambiguous_mut(destination)
            .unwrap()
            .unwrap()
            .add_comment(aida_core::Comment::new(
                "fixture".into(),
                format!("confirmed\n\n[intake-op:{operation}]"),
            ));
    })
    .unwrap();
    let before = load()
        .get_requirement_unambiguous(destination)
        .unwrap()
        .unwrap()
        .comments
        .len();
    good(&f.run(&[
        "intent",
        "process",
        &recover,
        "--action",
        "comment",
        "--target",
        destination,
    ]));
    assert_eq!(
        load()
            .get_requirement_unambiguous(destination)
            .unwrap()
            .unwrap()
            .comments
            .len(),
        before
    );
    assert!(private
        .read(&format!("operation-{recover}.json"))
        .unwrap()
        .is_none());

    // Crash after a successful receipt push, before journal cleanup. Reinstall
    // the same durable journal: retry verifies evidence and removes it.
    // trace:TASK-3096 | ai:codex
    let recovered_journal = json!({"version":1,"checksum":recover,"action":"comment","target":destination,"operation":operation,"store":store_path.canonicalize().unwrap()});
    private
        .write(
            &format!("operation-{recover}.json"),
            &serde_json::to_vec(&recovered_journal).unwrap(),
        )
        .unwrap();
    good(&f.run(&[
        "intent",
        "process",
        &recover,
        "--action",
        "comment",
        "--target",
        destination,
    ]));
    assert!(private
        .read(&format!("operation-{recover}.json"))
        .unwrap()
        .is_none());
    assert_eq!(
        load()
            .get_requirement_unambiguous(destination)
            .unwrap()
            .unwrap()
            .comments
            .len(),
        before
    );

    // Another clone won receipt publication after our separate create write.
    // Retrying must identify the orphan instead of silently returning its receipt.
    let duplicate = f.note("Concurrent duplicate create");
    good(&f.publish());
    good(&f.run(&["intent", "process", &duplicate, "--action", "create"]));
    let out = f.run(&[
        "add",
        "--title=Orphaned local duplicate",
        "--type",
        "task",
        "--status",
        "draft",
        "--tags",
        &format!("intake-op:{operation}"),
    ]);
    good(&out);
    let duplicate_spec = load()
        .requirements
        .into_iter()
        .find(|r| r.title == "Orphaned local duplicate")
        .unwrap();
    let duplicate_journal = json!({"version":1,"checksum":duplicate,"action":"create","target":null,"operation":operation,"store":store_path.canonicalize().unwrap()});
    private
        .write(
            &format!("operation-{duplicate}.json"),
            &serde_json::to_vec(&duplicate_journal).unwrap(),
        )
        .unwrap();
    let out = f.run(&["intent", "process", &duplicate, "--action", "create"]);
    bad(&out, "Orphaned duplicate spec write");
    bad(&out, &duplicate_spec.friendly_id());
    assert!(private
        .read(&format!("operation-{duplicate}.json"))
        .unwrap()
        .is_some());
    let count = load().requirements.len();

    // Crash-before-write is indistinguishable from lost evidence: retain the
    // journal and refuse rather than issuing a second potentially duplicate write.
    let ambiguous = f.note("Ambiguous operation");
    good(&f.publish());
    let journal = json!({"version":1,"checksum":ambiguous,"action":"create","target":null,"operation":uuid::Uuid::new_v4().to_string(),"store":store_path.canonicalize().unwrap()});
    private
        .write(
            &format!("operation-{ambiguous}.json"),
            &serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
    bad(
        &f.run(&["intent", "process", &ambiguous, "--action", "create"]),
        "Refusing automatic replay",
    );
    assert_eq!(load().requirements.len(), count);
    assert!(private
        .read(&format!("operation-{ambiguous}.json"))
        .unwrap()
        .is_some());
    good(&f.run(&[
        "intent",
        "process",
        &ambiguous,
        "--action",
        "pending",
        "--reconcile",
    ]));
    assert!(private
        .read(&format!("operation-{ambiguous}.json"))
        .unwrap()
        .is_none());
    assert_eq!(load().requirements.len(), count);
}

#[test]
fn linked_product_worktrees_share_only_intake_state_and_preserve_both_indexes() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let f = Fixture::new();
    f.enroll(&remote);
    f.note("linked worktree");
    let sibling = f._temp.path().join("sibling");
    f.git(&[
        "worktree",
        "add",
        "--detach",
        sibling.to_str().unwrap(),
        "HEAD",
    ]);
    let before = f.snapshot();
    let sibling_index_path = f.root.join(".git/worktrees/sibling/index");
    let sibling_index = fs::read(&sibling_index_path).unwrap();
    good(
        &f.cmd(&[
            "intent",
            "publish",
            "--file",
            f.root.join("envelope.json").to_str().unwrap(),
            "--reviewed",
        ])
        .current_dir(&sibling)
        .output()
        .unwrap(),
    );
    good(
        &f.cmd(&["intent", "list", "--json"])
            .current_dir(&sibling)
            .output()
            .unwrap(),
    );
    assert_eq!(f.snapshot(), before);
    assert_eq!(fs::read(sibling_index_path).unwrap(), sibling_index);
    assert!(!sibling.join(".git/aida-intake").exists());
}

#[test]
fn hostile_file_modes_and_orphan_receipts_are_not_accepted() {
    for attack in ["symlink", "executable", "receipt", "receipt-unknown"] {
        let temp = tempfile::tempdir().unwrap();
        let remote = remote(&temp);
        let sender = Fixture::new();
        let receiver = Fixture::new();
        sender.enroll(&remote);
        receiver.enroll(&remote);
        let checksum = sender.note("safe");
        good(&sender.publish());
        good(&receiver.run(&["intent", "pull"]));
        let accepted = git(&receiver.intake(), &["rev-parse", "HEAD"]);
        let path = sender
            .intake()
            .join(format!("observations/{checksum}.json"));
        match attack {
            "symlink" => {
                fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink("/etc/passwd", &path).unwrap();
            }
            "executable" => {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            "receipt" | "receipt-unknown" => {
                fs::create_dir_all(sender.intake().join("receipts")).unwrap();
                let fake = if attack == "receipt" {
                    "0".repeat(64)
                } else {
                    checksum.clone()
                };
                let mut receipt = json!({"version":1,"checksum":fake,"action":"dismiss","operation":uuid::Uuid::new_v4().to_string()});
                if attack == "receipt-unknown" {
                    receipt["SECRET-RECEIPT-KEY"] = json!("sensitive");
                }
                fs::write(
                    sender.intake().join(format!("receipts/{fake}.json")),
                    serde_json::to_vec(&receipt).unwrap(),
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        git(&sender.intake(), &["add", "."]);
        git(
            &sender.intake(),
            &["commit", "-qm", "Record reviewed intake data"],
        );
        git(
            &sender.intake(),
            &[
                "push",
                remote.to_str().unwrap(),
                "HEAD:refs/heads/aida-intent",
            ],
        );
        let refused = receiver.run(&["intent", "pull"]);
        assert!(!String::from_utf8_lossy(&refused.stderr).contains("SECRET-RECEIPT-KEY"));
        bad(
            &refused,
            if attack == "receipt" {
                "Receipt without observation"
            } else if attack == "receipt-unknown" {
                "Invalid intake receipt schema or JSON"
            } else {
                "ordinary non-executable"
            },
        );
        assert_eq!(accepted, git(&receiver.intake(), &["rev-parse", "HEAD"]));
    }
}

#[test]
fn intake_human_and_json_renderings_are_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let f = Fixture::new();
    f.enroll(&remote);
    let enrolled = f.run(&[
        "intent",
        "enroll",
        "--remote",
        remote.to_str().unwrap(),
        "--reviewed",
        "--json",
    ]);
    good(&enrolled);
    assert_eq!(
        serde_json::from_slice::<Value>(&enrolled.stdout).unwrap()["enrolled"],
        true
    );
    let checksum = f.note("Title with spaces");
    let published = f.publish();
    good(&published);
    assert!(String::from_utf8_lossy(&published.stdout).starts_with("Published "));
    let published = f.run(&["intent", "publish", "--json"]);
    good(&published);
    assert_eq!(
        serde_json::from_slice::<Value>(&published.stdout).unwrap()["published"],
        true
    );
    let pulled = f.run(&["intent", "pull"]);
    good(&pulled);
    assert!(String::from_utf8_lossy(&pulled.stdout).starts_with("Pulled "));
    let pulled = f.run(&["--format", "json", "intent", "pull"]);
    good(&pulled);
    assert_eq!(
        serde_json::from_slice::<Value>(&pulled.stdout).unwrap()["pulled"],
        true
    );
    let listed = f.run(&["--format", "human", "intent", "list"]);
    good(&listed);
    assert!(String::from_utf8_lossy(&listed.stdout).ends_with("pending Title with spaces\n"));
    let pending = f.run(&[
        "intent", "process", &checksum, "--action", "pending", "--json",
    ]);
    good(&pending);
    assert_eq!(
        serde_json::from_slice::<Value>(&pending.stdout).unwrap()["state"],
        "pending"
    );
    let dismissed = f.run(&["intent", "process", &checksum, "--action", "dismiss"]);
    good(&dismissed);
    assert_eq!(
        String::from_utf8_lossy(&dismissed.stdout),
        format!("{checksum} dismiss\n")
    );
}

// trace:STORY-2480 | ai:codex
#[test]
fn validated_checkpoint_skips_old_trees_but_checks_new_intermediate_commits() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let sender = Fixture::new();
    let receiver = Fixture::new();
    sender.enroll(&remote);
    receiver.enroll(&remote);
    sender.note("first");
    good(&sender.publish());
    sender.note("second");
    good(&sender.publish());
    good(&receiver.run(&["intent", "pull"]));
    let old = git(&receiver.intake(), &["rev-parse", "HEAD"]);
    let checkpoint = receiver.root.join(".git/aida-intake/validated-tip");
    let cached = fs::read_to_string(&checkpoint).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&cached).unwrap()["tip"], old);
    // Removing an old object proves a second command uses the validated prefix.
    // The tip's live blobs still get validated, so a missing live blob refuses.
    let root_commit = git(&receiver.intake(), &["rev-list", "--max-parents=0", "HEAD"]);
    let root_tree = git(
        &receiver.intake(),
        &["rev-parse", &format!("{root_commit}^{{tree}}")],
    );
    let object = receiver
        .intake()
        .join(".git/objects")
        .join(&root_tree[..2])
        .join(&root_tree[2..]);
    let saved = fs::read(&object).unwrap();
    fs::remove_file(&object).unwrap();
    good(&receiver.run(&["intent", "list", "--json"]));
    sender.note("third, after receiver checkpoint");
    good(&sender.publish());
    good(&receiver.run(&["intent", "pull"]));
    let old = git(&receiver.intake(), &["rev-parse", "HEAD"]);
    let cached = fs::read_to_string(&checkpoint).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&cached).unwrap()["tip"], old);
    let mut stale: Value = serde_json::from_str(&cached).unwrap();
    stale["validator"] = Value::String("stale-validator".to_owned());
    fs::write(&checkpoint, serde_json::to_vec(&stale).unwrap()).unwrap();
    bad(
        &receiver.run(&["intent", "list", "--json"]),
        "Git operation refused",
    );
    fs::write(object, saved).unwrap();
    good(&receiver.run(&["intent", "list", "--json"]));
    assert_eq!(fs::read_to_string(&checkpoint).unwrap(), cached);
    // The checkpoint cannot hide a hostile addition and removal after its tip.
    fs::write(sender.intake().join("leak.txt"), "private").unwrap();
    git(&sender.intake(), &["add", "."]);
    git(
        &sender.intake(),
        &["commit", "-qm", "Record reviewed intake data"],
    );
    git(&sender.intake(), &["rm", "-q", "leak.txt"]);
    git(
        &sender.intake(),
        &["commit", "-qm", "Record reviewed intake data"],
    );
    git(
        &sender.intake(),
        &[
            "push",
            remote.to_str().unwrap(),
            "HEAD:refs/heads/aida-intent",
        ],
    );
    bad(&receiver.run(&["intent", "pull"]), "Unexpected intake path");
    assert_eq!(fs::read_to_string(checkpoint).unwrap(), cached);
    assert_eq!(git(&receiver.intake(), &["rev-parse", "HEAD"]), old);
}

#[test]
fn writer_refusal_is_diagnostic_and_successful_retry_does_not_duplicate() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let f = Fixture::new();
    f.enroll(&remote);
    f.git(&["branch", "-D", "aida-store"]);
    good(&f.run(&[
        "init",
        "--no-skills",
        "--no-hooks",
        "--no-agent-config",
        "--no-roles",
    ]));
    let refused = f.note("Retry after a refused writer");
    good(&f.publish());
    // A definite ordinary-writer policy refusal; local intake reads stay available.
    let common = aida_core::transport_policy::Private::open(&f.root.join(".git")).unwrap();
    common.write("aida-transport.json", b"{}").unwrap();
    let failure = f.run(&["intent", "process", &refused, "--action", "create"]);
    bad(&failure, "retry is available");
    assert!(
        String::from_utf8_lossy(&failure.stderr).contains("transport profile"),
        "writer stderr must survive: {}",
        String::from_utf8_lossy(&failure.stderr)
    );
    assert!(!f
        .root
        .join(format!(".git/aida-intake/operation-{refused}.json"))
        .exists());
    common.remove("aida-transport.json").unwrap();
    good(&f.run(&["intent", "process", &refused, "--action", "create"]));
    good(&f.run(&["intent", "publish"]));
    let out = f.run(&["intent", "list", "--all", "--json"]);
    good(&out);
    let rows: Value = serde_json::from_slice(&out.stdout).unwrap();
    let destination = rows[0]["receipt"]["destination"].as_str().unwrap();
    assert!(destination.starts_with("TASK-"));
    let store_path = aida_core::store_locate::detect_distributed_store_from(&f.root).unwrap();
    let backend = aida_core::GitBackend::new(&store_path).unwrap();
    let store = aida_core::DatabaseBackend::load(&backend).unwrap();
    assert_eq!(
        store
            .requirements
            .iter()
            .filter(|r| r.title == "Retry after a refused writer")
            .count(),
        1
    );
}

#[test]
fn escaped_nul_spawn_failure_leaves_pending_and_dismiss_available() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let f = Fixture::new();
    f.enroll(&remote);
    f.git(&["branch", "-D", "aida-store"]);
    good(&f.run(&[
        "init",
        "--no-skills",
        "--no-hooks",
        "--no-agent-config",
        "--no-roles",
    ]));
    // A valid escaped NUL is data in an envelope, but cannot be an argv value.
    // A spawn failure must leave pending/dismiss and subsequent work usable.
    let nul = "34663d4588bf11840f31087f115be131de11a7a27dfdf71e8de08ce456a748ce";
    let wire = json!({"version":1,"kind":"observation","category":"observation","title":"NUL\0title","body":"body","checksum":nul});
    fs::write(
        f.root.join("envelope.json"),
        serde_json::to_vec(&wire).unwrap(),
    )
    .unwrap();
    good(&f.publish());
    bad(
        &f.run(&["intent", "process", nul, "--action", "create"]),
        "retry is available",
    );
    assert!(!f
        .root
        .join(format!(".git/aida-intake/operation-{nul}.json"))
        .exists());
    good(&f.run(&["intent", "process", nul, "--action", "pending"]));
    good(&f.run(&["intent", "process", nul, "--action", "dismiss"]));
    good(&f.run(&["intent", "publish"]));
}

// trace:TASK-3096 | ai:codex
#[test]
fn https_userinfo_refuses_before_persistence_and_ssh_is_supported() {
    for remote in [
        "https://token@example.org/intake.git",
        "https://user:token@example.org/intake.git",
    ] {
        let f = Fixture::new();
        let out = f.run(&["intent", "enroll", "--remote", remote, "--reviewed"]);
        bad(&out, "HTTPS userinfo is refused");
        assert!(!f.root.join(".git/aida-intake/profile.json").exists());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("token@"));
    }
    for remote in [
        "https://example.org/public.git",
        "ssh://git@example.org/private.git",
    ] {
        let f = Fixture::new();
        good(&f.run(&["intent", "enroll", "--remote", remote, "--reviewed"]));
    }
    // A previously persisted credential-bearing profile must also refuse.
    let f = Fixture::new();
    let temp = tempfile::tempdir().unwrap();
    f.enroll(&remote(&temp));
    let private =
        aida_core::transport_policy::Private::open(&f.root.join(".git/aida-intake")).unwrap();
    private.write("profile.json", &serde_json::to_vec(&json!({"version":1,"remote":"https://token@example.org/intake.git","deny_patterns":[]})).unwrap()).unwrap();
    bad(&f.run(&["intent", "list"]), "HTTPS userinfo is refused");
}

// trace:TASK-3096 | ai:codex
#[test]
fn push_hook_refusal_reports_diagnostic_once_and_keeps_accepted_checkpoint() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let f = Fixture::new();
    f.enroll(&remote);
    f.note("accepted");
    good(&f.publish());
    let checkpoint = f.root.join(".git/aida-intake/validated-tip");
    let accepted = fs::read(&checkpoint).unwrap();
    let tip = git(&f.intake(), &["rev-parse", "HEAD"]);
    let hook = remote.join("hooks/pre-receive");
    fs::write(&hook, "#!/bin/sh\necho attempted >> push-attempts\necho 'fixture policy denied this push' >&2\nexit 1\n").unwrap();
    fs::set_permissions(hook, fs::Permissions::from_mode(0o700)).unwrap();
    f.note("refused candidate");
    let out = f.publish();
    bad(&out, "fixture policy denied this push");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("lost the race"));
    assert_eq!(
        fs::read_to_string(remote.join("push-attempts"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(git(&f.intake(), &["rev-parse", "HEAD"]), tip);
    assert_eq!(fs::read(&checkpoint).unwrap(), accepted);
}

// trace:TASK-3096 | ai:codex
#[test]
fn moved_remote_tip_retries_publication_after_an_actual_push_race() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let first = Fixture::new();
    let second = Fixture::new();
    first.enroll(&remote);
    second.enroll(&remote);
    first.note("initial tip");
    good(&first.publish());
    // Pause only the first push after both candidates have synchronized. The
    // second publisher wins, making the first's ref update a real lost race.
    let hook = remote.join("hooks/pre-receive");
    fs::write(&hook, "#!/bin/sh\nif mkdir race-gate 2>/dev/null; then\n  i=0\n  while [ ! -f race-release ]; do\n    i=$((i + 1))\n    [ \"$i\" -le 100 ] || exit 1\n    sleep 0.1\n  done\nfi\n").unwrap();
    fs::set_permissions(hook, fs::Permissions::from_mode(0o700)).unwrap();
    first.note("first candidate loses push race");
    let child = first
        .cmd(&["intent", "publish", "--file", "envelope.json", "--reviewed"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !remote.join("race-gate").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let reached_hook = remote.join("race-gate").exists();
    second.note("second candidate wins push race");
    let winner = second.publish();
    fs::write(remote.join("race-release"), b"").unwrap();
    let retried = child.wait_with_output().unwrap();
    assert!(
        reached_hook,
        "first publisher never reached the receive hook"
    );
    good(&winner);
    good(&retried);
    good(&second.run(&["intent", "pull"]));
    assert_eq!(
        git(&first.intake(), &["rev-parse", "HEAD"]),
        git(&second.intake(), &["rev-parse", "HEAD"])
    );
    let out = first.run(&["intent", "list", "--json"]);
    good(&out);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

// trace:TASK-3096 | ai:codex
#[test]
fn published_receipt_reconciles_a_crash_before_the_local_writer() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let f = Fixture::new();
    let other = Fixture::new();
    f.enroll(&remote);
    other.enroll(&remote);
    f.git(&["branch", "-D", "aida-store"]);
    good(&f.run(&[
        "init",
        "--no-skills",
        "--no-hooks",
        "--no-agent-config",
        "--no-roles",
    ]));
    let checksum = f.note("Crash before writer");
    good(&f.publish());
    let store_path = aida_core::store_locate::detect_distributed_store_from(&f.root).unwrap();
    let backend = aida_core::GitBackend::new(&store_path).unwrap();
    let before = aida_core::DatabaseBackend::load(&backend)
        .unwrap()
        .requirements
        .len();
    let private =
        aida_core::transport_policy::Private::open(&f.root.join(".git/aida-intake")).unwrap();
    let name = format!("operation-{checksum}.json");
    private.write(&name, &serde_json::to_vec(&json!({"version":1,"checksum":checksum,"action":"create","target":null,"operation":uuid::Uuid::new_v4().to_string(),"store":store_path.canonicalize().unwrap()})).unwrap()).unwrap();
    good(&other.run(&["intent", "process", &checksum, "--action", "dismiss"]));
    bad(
        &f.run(&["intent", "process", &checksum, "--action", "dismiss"]),
        "journal retained",
    );
    good(&f.run(&[
        "intent",
        "process",
        &checksum,
        "--action",
        "pending",
        "--reconcile",
    ]));
    assert!(private.read(&name).unwrap().is_none());
    good(&f.run(&["intent", "process", &checksum, "--action", "dismiss"]));
    assert_eq!(
        aida_core::DatabaseBackend::load(&backend)
            .unwrap()
            .requirements
            .len(),
        before
    );
}

// trace:TASK-3096 | ai:codex
#[test]
fn concurrent_comments_on_one_target_report_the_losing_operation() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let a = Fixture::new();
    let b = Fixture::new();
    for f in [&a, &b] {
        f.enroll(&remote);
        f.git(&["branch", "-D", "aida-store"]);
        good(&f.run(&[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ]));
    }
    good(&a.run(&[
        "add",
        "--title=Shared target",
        "--type",
        "task",
        "--status",
        "draft",
    ]));
    let a_path = aida_core::store_locate::detect_distributed_store_from(&a.root).unwrap();
    let a_backend = aida_core::GitBackend::new(&a_path).unwrap();
    let target_req = aida_core::DatabaseBackend::load(&a_backend)
        .unwrap()
        .requirements
        .into_iter()
        .find(|r| r.title == "Shared target")
        .unwrap();
    let target = target_req.id.to_string();
    let b_path = aida_core::store_locate::detect_distributed_store_from(&b.root).unwrap();
    let b_backend = aida_core::GitBackend::new(&b_path).unwrap();
    // Model two home clones with the same canonical target history. Importing
    // via Git preserves allocation provenance instead of minting a foreign ID.
    git(&b_path, &["fetch", a_path.to_str().unwrap(), "HEAD"]);
    git(&b_path, &["reset", "--hard", "FETCH_HEAD"]);
    let checksum = a.note("Concurrent comment");
    good(&a.publish());
    good(&b.run(&["intent", "pull"]));
    let hook = remote.join("hooks/pre-receive");
    fs::write(&hook, "#!/bin/sh\nif mkdir comment-first 2>/dev/null; then\n i=0\n while [ ! -f comment-release ]; do\n  i=$((i+1)); [ \"$i\" -le 200 ] || exit 1; sleep 0.1\n done\nfi\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    let args = [
        "intent", "process", &checksum, "--action", "comment", "--target", &target,
    ];
    let first = a
        .cmd(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !remote.join("comment-first").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let reached_hook = remote.join("comment-first").exists();
    let second = b
        .cmd(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let second_wrote = loop {
        let store = aida_core::DatabaseBackend::load(&b_backend).unwrap();
        let wrote = store
            .get_requirement_unambiguous(&target)
            .unwrap()
            .unwrap()
            .comments
            .iter()
            .any(|c| c.content.contains("[intake-op:"));
        if wrote || Instant::now() >= deadline {
            break wrote;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // Git can refuse the losing update before invoking pre-receive. Store
    // evidence is the barrier: both writers must commit before the first push
    // is released, without requiring the loser to reach a server-side hook.
    fs::write(remote.join("comment-release"), b"").unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();
    assert!(
        reached_hook && second_wrote,
        "both comments must precede receipt publication: {first:?} {second:?}"
    );
    assert_ne!(first.status.success(), second.status.success());
    let (winner, loser, loser_backend) = if first.status.success() {
        (&a, &b, &b_backend)
    } else {
        (&b, &a, &a_backend)
    };
    let receipt: Value = serde_json::from_slice(
        &fs::read(winner.intake().join(format!("receipts/{checksum}.json"))).unwrap(),
    )
    .unwrap();
    let name = format!("operation-{checksum}.json");
    let private =
        aida_core::transport_policy::Private::open(&loser.root.join(".git/aida-intake")).unwrap();
    let journal: Value = serde_json::from_slice(&private.read(&name).unwrap().unwrap()).unwrap();
    assert_ne!(receipt["operation"], journal["operation"]);
    bad(&loser.run(&args), "Orphaned duplicate spec write");
    bad(
        &loser.run(&[
            "intent",
            "process",
            &checksum,
            "--action",
            "comment",
            "--target",
            &target,
            "--reconcile",
        ]),
        "Orphaned duplicate spec write",
    );
    assert!(private.read(&name).unwrap().is_some());
    let store = aida_core::DatabaseBackend::load(loser_backend).unwrap();
    let comments = &store
        .get_requirement_unambiguous(&target)
        .unwrap()
        .unwrap()
        .comments;
    assert_eq!(
        comments
            .iter()
            .filter(|c| c.content.contains("[intake-op:"))
            .count(),
        1
    );
    good(&winner.run(&args));
}

// trace:TASK-3096 | ai:codex
#[test]
fn sync_failures_preserve_git_stderr_and_local_ahead_push_keeps_policy_error() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let unavailable = temp.path().join("missing.git");
    let f = Fixture::new();
    f.enroll(&unavailable);
    let out = f.run(&["intent", "pull"]);
    bad(&out, "Intake Git operation refused (ls-remote)");
    bad(&out, "does not appear to be a git repository");
    let remote = remote(&temp);
    let f = Fixture::new();
    f.enroll(&remote);
    f.note("Initial");
    good(&f.publish());
    let shim = temp.path().join("shim");
    fs::create_dir(&shim).unwrap();
    // Resolve before adding the shim so its delegation cannot recurse.
    let git_path = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("git"))
        .find(|path| path.is_file())
        .unwrap()
        .canonicalize()
        .unwrap();
    let wrapper = shim.join("git");
    fs::write(&wrapper, format!("#!/bin/sh\nfor arg do\n if [ \"$arg\" = fetch ]; then echo 'fixture fetch authentication denied' >&2; exit 1; fi\ndone\nexec '{}' \"$@\"\n", git_path.display())).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!("{}:{}", shim.display(), std::env::var("PATH").unwrap());
    let out = f
        .cmd(&["intent", "pull"])
        .env("PATH", &path)
        .output()
        .unwrap();
    bad(&out, "Intake Git operation refused (fetch)");
    bad(&out, "fixture fetch authentication denied");
    fs::remove_file(wrapper).unwrap();
    let ssh = shim.join("ssh");
    fs::write(
        &ssh,
        "#!/bin/sh\necho 'Permission denied (publickey).' >&2\nexit 255\n",
    )
    .unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    let ssh_site = Fixture::new();
    good(&ssh_site.run(&[
        "intent",
        "enroll",
        "--remote",
        "ssh://git@example.invalid/intake.git",
        "--reviewed",
    ]));
    let out = ssh_site
        .cmd(&["intent", "pull"])
        .env("PATH", &path)
        .output()
        .unwrap();
    bad(&out, "Intake Git operation refused (ls-remote)");
    bad(&out, "Permission denied (publickey)");
    let hook = remote.join("hooks/pre-receive");
    fs::write(&hook, "#!/bin/sh\necho 'ahead policy denied' >&2\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    let checksum = f.note("Retained ahead commit");
    fs::copy(
        f.root.join("envelope.json"),
        f.intake().join(format!("observations/{checksum}.json")),
    )
    .unwrap();
    git(&f.intake(), &["add", "."]);
    git(
        &f.intake(),
        &["commit", "-qm", "Record reviewed intake data"],
    );
    let tip = git(&f.intake(), &["rev-parse", "HEAD"]);
    let out = f.run(&["intent", "publish"]);
    bad(&out, "ahead policy denied");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("Retry;"));
    assert_eq!(tip, git(&f.intake(), &["rev-parse", "HEAD"]));
}

// trace:STORY-2652 | ai:codex
const PRODUCER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PROCESSOR: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn enroll_writer(f: &Fixture, remote: &Path, processor: bool) -> Output {
    let mut args = vec![
        "intent",
        "enroll",
        "--remote",
        remote.to_str().unwrap(),
        "--reviewed",
        "--role",
        if processor { "processor" } else { "producer" },
        "--writer-id",
        if processor { PROCESSOR } else { PRODUCER },
        "--producer-id",
        PRODUCER,
        "--json",
    ];
    if !processor {
        args.extend(["--receipt-writer", PROCESSOR]);
    }
    f.run(&args)
}

#[test]
fn per_writer_round_trip_separates_orphans_and_preserves_product_store_refs() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let work = Fixture::new();
    let home = Fixture::new();
    work.git(&["push", remote.to_str().unwrap(), "main", "aida-store"]);
    let remote_product = git(&remote, &["rev-parse", "refs/heads/main"]);
    let remote_store = git(&remote, &["rev-parse", "refs/heads/aida-store"]);
    let work_before = work.snapshot();
    let home_before = home.snapshot();
    let enrollment = enroll_writer(&work, &remote, false);
    good(&enrollment);
    let profile: Value = serde_json::from_slice(&enrollment.stdout).unwrap();
    assert_eq!(profile["profile"]["version"], 2);
    assert_eq!(profile["protection"], "unverified");
    good(&enroll_writer(&work, &remote, false));
    good(&enroll_writer(&home, &remote, true));
    let checksum = work.note("Separated round trip");
    good(&work.publish());
    let producer_ref = format!("refs/heads/aida-feedback-{PRODUCER}");
    let receipt_ref = format!("refs/heads/aida-feedback-receipts-{PROCESSOR}");
    let observation = format!("observations/{checksum}.json");
    assert_eq!(
        git(&remote, &["ls-tree", "-r", "--name-only", &producer_ref]),
        observation
    );
    good(&home.run(&["intent", "pull"]));
    good(&home.run(&["intent", "process", &checksum, "--action", "dismiss"]));
    good(&home.run(&["intent", "process", &checksum, "--action", "dismiss"]));
    assert_eq!(git(&remote, &["rev-list", "--count", &receipt_ref]), "1");
    assert_eq!(
        git(&remote, &["ls-tree", "-r", "--name-only", &receipt_ref]),
        format!("receipts/{checksum}.json")
    );
    for branch in [&producer_ref, &receipt_ref] {
        assert_eq!(
            git(&remote, &["rev-list", "--parents", branch])
                .split_whitespace()
                .count(),
            1
        );
    }
    good(&work.run(&["intent", "pull"]));
    let list = work.run(&["intent", "list", "--all", "--json"]);
    good(&list);
    let rows: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(rows[0]["checksum"], checksum);
    assert_eq!(rows[0]["state"], "processed");
    assert_eq!(rows[0]["receipt"]["action"], "dismiss");
    bad(
        &work.run(&["intent", "process", &checksum, "--action", "create"]),
        "Only an enrolled processor",
    );
    bad(&home.publish(), "Only an enrolled producer");
    assert_eq!(
        git(&remote, &["rev-parse", "refs/heads/main"]),
        remote_product
    );
    assert_eq!(
        git(&remote, &["rev-parse", "refs/heads/aida-store"]),
        remote_store
    );
    assert_eq!(work.snapshot(), work_before);
    assert_eq!(home.snapshot(), home_before);
    assert_eq!(
        git(&remote, &["for-each-ref", "--format=%(refname)"])
            .lines()
            .count(),
        4
    );
}

#[test]
fn per_writer_enrollment_pins_identity_and_defaults_without_store() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let f = Fixture::new();
    let before = f.snapshot();
    let args = [
        "intent",
        "enroll",
        "--remote",
        remote.to_str().unwrap(),
        "--reviewed",
        "--role",
        "producer",
        "--json",
    ];
    let first = f.run(&args);
    good(&first);
    let second = f.run(&args);
    good(&second);
    assert_eq!(first.stdout, second.stdout);
    let profile: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(profile["profile"]["writer_id"].as_str().unwrap().len(), 32);
    bad(&enroll_writer(&f, &remote, false), "already enrolled");
    assert_eq!(f.snapshot(), before);
    let g = Fixture::new();
    bad(
        &g.run(&[
            "intent",
            "enroll",
            "--remote",
            remote.to_str().unwrap(),
            "--reviewed",
            "--role",
            "producer",
            "--writer-id",
            "../main",
        ]),
        "Invalid writer identity",
    );
    assert!(!g.root.join(".git/aida-intake/profile.json").exists());
    // WORK identity is read without allocating a store node or changing binding.
    use std::os::unix::fs::PermissionsExt;
    let binding = g.root.join(".git/aida-work-node.toml");
    let bytes = format!("version = 1\nnode_id = \"{PRODUCER}\"\n");
    fs::write(&binding, &bytes).unwrap();
    fs::set_permissions(&binding, fs::Permissions::from_mode(0o600)).unwrap();
    let inherited = g.run(&args);
    good(&inherited);
    assert_eq!(
        serde_json::from_slice::<Value>(&inherited.stdout).unwrap()["profile"]["writer_id"],
        PRODUCER
    );
    assert_eq!(fs::read_to_string(&binding).unwrap(), bytes);
}

#[test]
fn per_writer_receipt_retry_after_store_write_does_not_duplicate() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let work = Fixture::new();
    let home = Fixture::new();
    good(&enroll_writer(&work, &remote, false));
    good(&enroll_writer(&home, &remote, true));
    home.git(&["branch", "-D", "aida-store"]);
    good(&home.run(&[
        "init",
        "--no-skills",
        "--no-hooks",
        "--no-agent-config",
        "--no-roles",
    ]));
    let checksum = work.note("Separated receipt retry");
    good(&work.publish());
    let hook = remote.join("hooks/pre-receive");
    fs::write(
        &hook,
        "#!/bin/sh\necho receipt-publication-denied >&2\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let failed = home.run(&["intent", "process", &checksum, "--action", "create"]);
    bad(
        &failed,
        "Spec write confirmed but its receipt is not published",
    );
    let journal = home
        .root
        .join(format!(".git/aida-intake/operation-{checksum}.json"));
    assert!(journal.exists());
    let store_path = aida_core::store_locate::detect_distributed_store_from(&home.root).unwrap();
    let count = || {
        let backend = aida_core::GitBackend::new(&store_path).unwrap();
        aida_core::DatabaseBackend::load(&backend)
            .unwrap()
            .requirements
            .iter()
            .filter(|r| r.title == "Separated receipt retry")
            .count()
    };
    assert_eq!(count(), 1);
    fs::remove_file(hook).unwrap();
    good(&home.run(&["intent", "process", &checksum, "--action", "create"]));
    assert_eq!(count(), 1);
    assert!(!journal.exists());
    good(&work.run(&["intent", "pull"]));
    let rows = work.run(&["intent", "list", "--all", "--json"]);
    good(&rows);
    assert_eq!(
        serde_json::from_slice::<Value>(&rows.stdout).unwrap()[0]["state"],
        "processed"
    );
}

#[test]
fn per_writer_foreign_paths_refuse_and_standalone_receipts_are_readable() {
    let temp = tempfile::tempdir().unwrap();
    let remote = remote(&temp);
    let work = Fixture::new();
    let home = Fixture::new();
    good(&enroll_writer(&work, &remote, false));
    good(&enroll_writer(&home, &remote, true));
    let checksum = work.note("Standalone receipt");
    good(&work.publish());
    let producer_ref = format!("refs/heads/aida-feedback-{PRODUCER}");
    let receipt_ref = format!("refs/heads/aida-feedback-receipts-{PROCESSOR}");
    // A valid observation history is invalid on a receipt branch, even with the
    // validator checkpoint from the same commit on the observation branch.
    let producer_tip = git(&remote, &["rev-parse", &producer_ref]);
    git(&remote, &["update-ref", &receipt_ref, &producer_tip]);
    bad(
        &work.run(&["intent", "pull"]),
        "forbidden for this intake branch role",
    );
    assert!(!work.intake().join(format!(".git/{receipt_ref}")).exists());
    git(&remote, &["update-ref", "-d", &receipt_ref]);
    good(&home.run(&["intent", "process", &checksum, "--action", "dismiss"]));
    git(&remote, &["update-ref", "-d", &producer_ref]);
    // A fresh reader can validate receipt-only history without its observation.
    let reader = Fixture::new();
    good(&enroll_writer(&reader, &remote, false));
    let pulled = reader.run(&["intent", "pull", "--json"]);
    good(&pulled);
    assert_eq!(
        serde_json::from_slice::<Value>(&pulled.stdout).unwrap()
            ["receipts_for_unknown_observations"],
        json!([checksum])
    );
    assert_eq!(
        git(&reader.intake(), &["rev-parse", &receipt_ref]),
        git(&remote, &["rev-parse", &receipt_ref])
    );
}
