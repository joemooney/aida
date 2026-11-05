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
fn concurrent_duplicate_publication_serializes_and_divergence_refuses() {
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
    b.note("three");
    bad(&b.publish(), "push refused");
    let tip = git(&b.intake(), &["rev-parse", "HEAD"]);
    bad(&b.run(&["intent", "pull"]), "diverged");
    assert_eq!(tip, git(&b.intake(), &["rev-parse", "HEAD"]));
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
    assert_eq!(receipt["destination"], req.display_id());
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
                let mut receipt = json!({"version":1,"checksum":fake,"action":"dismiss"});
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
    assert!(cached.ends_with(&format!(":{old}")));
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
    fs::write(&checkpoint, format!("v0:stale:{old}")).unwrap();
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
