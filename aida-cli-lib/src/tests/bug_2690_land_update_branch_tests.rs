// BUG-2690 round 2: the lander's update-branch request driven through the
// REAL path (`Live::update_ci_base` -> `forge::landing_update_branch`)
// against a fake `gh` on an isolated PATH. The fake logs each call's exact
// argv, so every refusal is proven by the absence of the PUT, and the happy
// path by exactly one repository-bound PUT carrying `expected_head_sha`.
// trace:BUG-2690 | ai:claude
use super::*;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MOVED: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const BRANCH: &str = "feature-7";
const GET: &str = "github.com api repos/owner/repo/pulls/7";

/// One repository per scenario: `<tmp>/repo` with a GitHub origin, and the
/// fake gh's fixture (`pr.json`) and call log (`calls.log`) beside it. The
/// fake runs with the repository as its cwd, so it finds them by `../`.
struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        );
        let root = root.canonicalize().unwrap();
        let fixture = Self { temp, root };
        fixture.live_pr(|_| {});
        fixture
    }

    /// The live PR the forge reports, edited from an open, unheld PR at HEAD.
    fn live_pr(&self, edit: impl FnOnce(&mut serde_json::Value)) {
        let mut live = serde_json::json!({
            "state": "open",
            "draft": false,
            "head": {"sha": HEAD, "ref": BRANCH},
            "base": {"ref": "main"},
            "labels": [{"name": "enhancement"}],
        });
        edit(&mut live);
        std::fs::write(self.temp.path().join("pr.json"), live.to_string()).unwrap();
    }

    fn live(&self) -> Live {
        Live {
            root: self.root.clone(),
            repo: crate::review_watch::forge::Repository::load(&self.root).unwrap(),
            program: PathBuf::from("/nonexistent/aida"),
            vendor: None,
            gitlab: false,
            handles: Vec::new(),
            other_run: false,
            hygiene: None,
            salvaged: std::collections::BTreeSet::new(),
            ci_updates: BTreeMap::new(),
            noted: Vec::new(),
        }
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.temp.path().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn puts(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| call.contains("PUT"))
            .collect()
    }

    /// A session lease on `branch` held by this live test process.
    fn live_lease(&self, branch: &str) {
        let me = std::process::id();
        let lease: crate::leases::SessionLease = serde_json::from_value(serde_json::json!({
            "id": "lane-lease",
            "pid_namespace": crate::agent_registry::current_pid_namespace(),
            "scope": "BUG-1",
            "slug": "bug-1",
            "owner": "test",
            "worktree_path": self.root,
            "branch": branch,
            "started_at": "2026-11-03T00:00:00Z",
            "hostname": crate::coordination::hostname(),
            "active_pid": me,
            "active_pid_start_time": crate::process_probe::process_start_identity(me),
        }))
        .unwrap();
        std::fs::create_dir_all(crate::leases::leases_dir(&self.root)).unwrap();
        std::fs::write(
            crate::leases::lease_path(&self.root, &lease.id),
            toml::to_string_pretty(&lease).unwrap(),
        )
        .unwrap();
    }

    fn protect_heads(&self) {
        let config = crate::config_path_for_project(&self.root);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(config, "[pr-head]\nprotect = true\n").unwrap();
    }

    fn approve_at(&self, head: &str) {
        let dir = self.root.join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("PR-7.json"),
            format!(
                r#"{{"verdict":"APPROVED","reviewed_sha":"{head}","recorded_by":"reviewer-a"}}"#
            ),
        )
        .unwrap();
    }
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn stale_pr() -> Pr {
    Pr {
        id: 7,
        head: HEAD.into(),
        base: "main".into(),
        branch: Some(BRANCH.into()),
        title: "fix(x): a change (BUG-7)".into(),
        updated_at: None,
        // Discovery read the label and found none: the hold read stays local.
        hold_label: Some(false),
        ci: None,
    }
}

/// The scenario must be refused before any PUT reaches the forge.
fn assert_refused(fixture: &Fixture, live: &mut Live, pr: &Pr, why: &str) {
    let error = live
        .update_ci_base(pr)
        .expect_err(&format!("{why}: must refuse"));
    assert!(
        fixture.puts().is_empty(),
        "{why}: no PUT may be sent, got {:?} ({error:#})",
        fixture.calls()
    );
    assert!(
        live.ci_updates.is_empty(),
        "{why}: nothing recorded as requested"
    );
}

#[test]
fn bug_2690_real_update_branch_path_puts_once_and_refuses_every_guard() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let bin = tempfile::tempdir().unwrap();
    let gh = bin.path().join("gh");
    crate::test_exec::write_executable(
        &gh,
        r#"#!/bin/sh
[ "$*" = 'api --help' ] && exit 0
printf '%s %s\n' "$GH_HOST" "$*" >> ../calls.log
case "$*" in
  'api repos/owner/repo/pulls/7'|'api --allow-escape-sequences repos/owner/repo/pulls/7') cat ../pr.json ;;
  'api --method PUT repos/owner/repo/pulls/7/update-branch -f expected_head_sha='*) printf '{}' ;;
  *) exit 99 ;;
esac
"#,
    );
    // Only the fake gh and fixed system tools: no host gh can be reached.
    let path = format!("{}:/usr/bin:/bin", bin.path().display());
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("PATH", Some(path.as_str())),
        ("GH_REPO", None),
        ("GH_HOST", None),
    ]);

    // (a) Positive control: the live PR is at the stale head, nothing guards
    // it, so exactly one PUT goes to the origin's repository with the head
    // pinned. A second pass at the same head does not ask again.
    let f = Fixture::new();
    let mut live = f.live();
    let note = live.update_ci_base(&stale_pr()).unwrap();
    assert!(note.contains("requested update-branch"), "{note}");
    assert_eq!(
        f.calls(),
        [
            GET.to_string(),
            format!(
                "github.com api --method PUT repos/owner/repo/pulls/7/update-branch -f expected_head_sha={HEAD}"
            ),
        ]
    );
    let note = live.update_ci_base(&stale_pr()).unwrap();
    assert!(note.contains("already requested"), "{note}");
    assert_eq!(f.puts().len(), 1, "one request per head");

    // A live lease owns the branch.
    let f = Fixture::new();
    f.live_lease(BRANCH);
    assert_refused(&f, &mut f.live(), &stale_pr(), "live lease on the branch");
    assert!(f.calls().is_empty(), "refused before any forge read");
    // A lease on another branch does not protect this one.
    let f = Fixture::new();
    f.live_lease("other-branch");
    f.live().update_ci_base(&stale_pr()).unwrap();
    assert_eq!(f.puts().len(), 1, "an unrelated lease does not block");

    // The lease set cannot be read (a torn lease file could be the owner's).
    let f = Fixture::new();
    std::fs::create_dir_all(crate::leases::leases_dir(&f.root)).unwrap();
    std::fs::write(
        crate::leases::lease_path(&f.root, "torn-lease"),
        "id = \"torn-lease\"\nscope = \"BUG-1\"\nbranch = \"feat",
    )
    .unwrap();
    assert_refused(&f, &mut f.live(), &stale_pr(), "unreadable lease set");

    // The origin changed after the pass pinned the repository.
    let f = Fixture::new();
    let mut live = f.live();
    git(
        &f.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/someone/else.git",
        ],
    );
    assert_refused(&f, &mut live, &stale_pr(), "changed repository");
    assert!(f.calls().is_empty(), "no forge call on a foreign origin");

    // A local merge hold.
    let f = Fixture::new();
    let marker = crate::gate::merge_hold_core::hold_path(&f.root, 7);
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, "operator hold").unwrap();
    assert_refused(&f, &mut f.live(), &stale_pr(), "local merge hold");

    // The hold label discovery read.
    let f = Fixture::new();
    let labelled = Pr {
        hold_label: Some(true),
        ..stale_pr()
    };
    assert_refused(&f, &mut f.live(), &labelled, "discovered hold label");

    // The live PR gained the hold label after discovery.
    let f = Fixture::new();
    f.live_pr(|pr| {
        pr["labels"] = serde_json::json!([{"name": crate::merge_hold::HOLD_LABEL}]);
    });
    assert_refused(&f, &mut f.live(), &stale_pr(), "live hold label");

    // The live PR moved or changed since the read.
    for (why, edit) in [
        (
            "moved head",
            Box::new(|pr: &mut serde_json::Value| pr["head"]["sha"] = MOVED.into())
                as Box<dyn Fn(&mut serde_json::Value)>,
        ),
        (
            "renamed branch",
            Box::new(|pr: &mut serde_json::Value| pr["head"]["ref"] = "other".into()),
        ),
        (
            "retargeted base",
            Box::new(|pr: &mut serde_json::Value| pr["base"]["ref"] = "integration".into()),
        ),
        (
            "closed",
            Box::new(|pr: &mut serde_json::Value| pr["state"] = "closed".into()),
        ),
        (
            "draft",
            Box::new(|pr: &mut serde_json::Value| pr["draft"] = true.into()),
        ),
        (
            "unreadable labels",
            Box::new(|pr: &mut serde_json::Value| pr["labels"] = serde_json::Value::Null),
        ),
    ] {
        let f = Fixture::new();
        f.live_pr(edit);
        assert_refused(&f, &mut f.live(), &stale_pr(), why);
        assert_eq!(f.calls(), [GET.to_string()], "{why}: read, never written");
    }

    // An approved head under head protection: the caller runs the shared
    // head guard, which refuses after the live read and before the PUT.
    let f = Fixture::new();
    f.protect_heads();
    f.approve_at(HEAD);
    let error = f.live().update_ci_base(&stale_pr()).unwrap_err();
    assert!(
        format!("{error:#}").contains("recorded approval"),
        "{error:#}"
    );
    assert!(f.puts().is_empty(), "approved head: no PUT");
    // Control: protection on, approval at another head, so this head moves.
    let f = Fixture::new();
    f.protect_heads();
    f.approve_at(MOVED);
    f.live().update_ci_base(&stale_pr()).unwrap();
    assert_eq!(f.puts().len(), 1, "an approval elsewhere does not protect");
}

// trace:BUG-2711 | ai:codex
#[test]
fn bug_2711_old_gh_reads_raw_bytes_and_probes_once() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new();
    crate::test_exec::write_executable(
        &f.temp.path().join("gh"),
        r#"#!/bin/sh
case "$*" in
  'api --help') echo probe >> ../probes; echo 'Usage: gh api PATH' ;;
  'api logs') printf '\033\377' ;;
  *) exit 99 ;;
esac
"#,
    );
    let path = format!("{}:/usr/bin:/bin", f.temp.path().display());
    let live = f.live();
    let _env = crate::test_env::EnvVarsGuard::apply(&[("PATH", Some(&path))]);
    for _ in 0..2 {
        assert_eq!(
            crate::forge::landing_ci_api(&f.root, &live.repo, "logs").unwrap(),
            [27, 255]
        );
    }
    assert_eq!(
        std::fs::read_to_string(f.temp.path().join("probes")).unwrap(),
        "probe\n"
    );
}

// trace:BUG-2711 | ai:codex
#[test]
fn bug_2711_installed_gh_supports_composed_argv() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let help = match std::process::Command::new("gh")
        .args(["api", "--help"])
        .output()
    {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("Skipping installed gh argv check: gh is absent");
            return;
        }
        result => result.unwrap(),
    };
    assert!(help.status.success());
    let f = Fixture::new();
    let args = crate::forge::landing_ci_args(&f.root, &f.live().repo, "logs");
    let help = String::from_utf8(help.stdout).unwrap();
    let expected = if help
        .lines()
        .any(|line| line.trim_start().starts_with("--allow-escape-sequences "))
    {
        vec!["api", "--allow-escape-sequences", "logs"]
    } else {
        vec!["api", "logs"]
    };
    assert_eq!(args, expected);
}
