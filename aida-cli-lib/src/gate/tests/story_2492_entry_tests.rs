//! Two-node command-entry regression. No decision, gate or train rail is mocked.
//! The reviewer substitute synthesizes local human-at-TTY receipt evidence from
//! a non-TTY child; delegated reviewer-grant attestation is not exercised here.
// trace:STORY-2492.ac540936 | ai:codex
use super::tests::{available, write, Fixture};
use super::*;
use clap::Parser;

fn git_ok(root: &Path, args: &[&str]) -> String {
    let output = crate::test_git::git_command()
        .current_dir(root)
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

const TEST: &str =
    "gate::review_publication::entry_tests::two_node_ship_watch_rework_and_train_entry_points";

#[derive(Parser)]
struct Watch {
    #[command(flatten)]
    args: crate::review_watch::ReviewWatchArgs,
}

fn watch_args(extra: &[&str]) -> crate::review_watch::ReviewWatchArgs {
    Watch::parse_from(std::iter::once("watch").chain(extra.iter().copied())).args
}

fn wait(child: &mut std::process::Child, root: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            if !status.success() {
                for entry in std::fs::read_dir(root.join(".aida/review-watch/logs")).unwrap() {
                    eprintln!(
                        "{}",
                        std::fs::read_to_string(entry.unwrap().path()).unwrap()
                    );
                }
            }
            assert!(status.success(), "dispatcher: {status}");
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("review dispatcher exceeded 25 seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// Model substitute: this non-TTY child synthesizes a human-at-TTY receipt via
// write_human_receipt and human_at_tty=true. The ordinary host collector validates
// it, but this does not exercise the real queue-work child's delegated reviewer
// grant attestation. This test covers transport of locally validated human
// authority; it must not claim end-to-end delegated-grant coverage.
fn collect(root: &Path) {
    let sha = std::env::var("AIDA_REVIEW_WATCH_HEAD").unwrap();
    let decision = std::fs::read_to_string(root.parent().unwrap().join("decision")).unwrap();
    let identity = vec!["session:reviewer-b".into(), "proc:123@start".into()];
    let id = authority::write_human_receipt(&sha, "STORY-1", &identity, false, true).unwrap();
    let att = rv::RecorderAttestation {
        sha: sha.clone(),
        human_at_tty: true,
        receipt_id: Some(id),
        identity,
        ..Default::default()
    };
    let mut record = serde_json::json!({"verdict":decision.trim(), "reviewed_sha":sha,
        "reviewed_branch":"feature", "recorded_by":"reviewer B",
        "recorded_at":chrono::Utc::now().to_rfc3339(), "recorder_attestation":att})
    .as_object()
    .unwrap()
    .clone();
    if decision.trim() != "approved" {
        record.insert("findings".into(), serde_json::json!(["fix the regression"]));
        record.insert(
            "blocking_finding_origin_ids".into(),
            serde_json::json!(["TASK-NODEB-2"]),
        );
    }
    let specs = ledger::GateSpec::resolve_all(root, &["STORY-1"]);
    assert_eq!(specs.len(), 1);
    ledger::record_collected_verdict(
        root,
        ChangeKey::pr(7),
        &record,
        &[(specs[0].clone(), record.clone())],
    )
    .unwrap();
}

// trace:STORY-2492.ac540936 | ai:codex
// trace:STORY-b6b1278890774ef993d2ebc327862470-418.ac540936 | ai:codex
// trace:STORY-b6b1278890774ef993d2ebc327862470-418.Docs | ai:codex
// Verifies the operator sequence described in docs/agents/second-review-node.md.
// trace:STORY-2492.Docs | ai:codex
// trace:STORY-2492.ac8cff0f | ai:codex
#[test]
fn two_node_ship_watch_rework_and_train_entry_points() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let _transport = crate::review_pr_transport_fixture();
    if let Ok(stage) = std::env::var("STORY2492_STAGE") {
        let root = std::env::current_dir().unwrap();
        if stage == "collect" {
            collect(&root);
            return;
        }
        let ticket = std::env::var("STORY2492_TICKET").unwrap();
        crate::review_watch::run_with_program(
            &watch_args(&["--launch-reservation", &ticket]),
            &root,
            Some(&root.parent().unwrap().join("review-cli")),
        )
        .unwrap();
        return;
    }
    assert!(available(), "OpenSSH is required for the end-to-end test");
    let f = Fixture::new_with_forge(true);
    let bin = f.temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let hub = f.temp.path().join("code.git");
    write(
        &bin,
        "forge.json",
        serde_json::to_vec(&serde_json::json!({"hub":hub,"prs":{},
            "git": crate::find_executable_on_path("git").expect("Git must be on PATH")}))
        .unwrap(),
    );
    let forge_script = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/gate/tests/fixtures/story_2492_forge.py"),
    )
    .unwrap();
    crate::test_exec::write_executable(&bin.join("gh"), &forge_script);
    crate::test_exec::write_executable(&bin.join("git"), &forge_script);
    // Pin a normal forge URL while all Git transport stays in the bare fixture hub.
    for root in [&f.a, &f.b] {
        git_ok(root, &["config", "user.name", "Fixture"]);
        git_ok(root, &["config", "user.email", "fixture@example.invalid"]);
        git_ok(
            root,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/fixture/review.git",
            ],
        );
        git_ok(
            root,
            &[
                "config",
                &format!("url.{}.insteadOf", hub.display()),
                "https://github.com/fixture/review.git",
            ],
        );
        git_ok(
            root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
    }
    git_ok(&f.a, &["checkout", "-qb", "feature"]);
    write(&f.a, "code", "first PR head\n");
    git_ok(&f.a, &["commit", "-qam", "feat: fixture (STORY-1)"]);
    git_ok(&f.a, &["push", "-qu", "origin", "feature"]);
    let h1 = head(&f.a).unwrap();
    let author = authority::AuthorshipRecord {
        scope: f.spec.spec_id.clone().unwrap(),
        lease_id: "author-lease".into(),
        branch: "feature".into(),
        tokens: vec!["session:author-session".into()],
        recorded_at: chrono::Utc::now().to_rfc3339(),
        launch: None,
    };
    write(
        &f.a,
        ".aida/review-authorship.jsonl",
        format!("{}\n", serde_json::to_string(&author).unwrap()),
    );
    let gh = bin.join("gh");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let home = f.temp.path().join("home-a");
    let home_b = f.temp.path().join("home-b");
    std::fs::create_dir(&home_b).unwrap();
    std::fs::create_dir(&home).unwrap();
    let script = format!(
        r#"#!/usr/bin/env python3
import os, sys
args = sys.argv[1:]
os.environ['AIDA_TEST_GH_BINARY'] = os.path.join(os.path.dirname(sys.argv[0]), 'bin', 'gh')
if args[:2] == ['review', 'watch']:
    os.environ['STORY2492_STAGE'] = 'dispatch'
    os.environ['STORY2492_TICKET'] = args[3]
else:
    assert args[:3] == ['queue', 'work', 'PR-7'], args
    os.environ['STORY2492_STAGE'] = 'collect'
os.execv({exe:?}, [{exe:?}, '--exact', '--nocapture', '--test-threads=1', {test:?}])
"#,
        exe = crate::aida_bin::current_executable_path()
            .unwrap()
            .to_str()
            .unwrap(),
        test = TEST
    );
    let program = f.temp.path().join("review-cli");
    crate::test_exec::write_executable(&program, &script);
    // The ship entry creates the PR before refusing its still-unreviewed head.
    {
        let _env = crate::test_env::AmbientGuard::hermetic_and(
            &f.a,
            None,
            &[
                ("PATH", Some(&path)),
                ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
                ("AIDA_HOME", Some(home.to_str().unwrap())),
                ("AIDA_CONTAINED", None),
                ("AIDA_HEADLESS", None),
            ],
        );
        let result = crate::pr_cmd::pr_ship_handler(
            None,
            true,
            true,
            false,
            false,
            None,
            None,
            false,
            false,
            false,
            Some(1),
            false,
        );
        assert!(
            format!("{:#}", result.as_ref().unwrap_err()).contains("no approval recorded"),
            "ship must reach its approval gate: {result:?}"
        );
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bin.join("forge.json")).unwrap()).unwrap();
        assert_eq!(
            state["prs"]["7"]["headRefName"], "feature",
            "ship must open the PR: {result:?}"
        );
        assert_eq!(
            git_ok(&f.a, &["ls-remote", "origin", "main"])
                .split_whitespace()
                .next(),
            Some(f.sha.as_str())
        );
    }
    for (decision, expected_head) in [("request-changes", h1.clone()), ("approved", String::new())]
    {
        let h = if expected_head.is_empty() {
            write(&f.a, "code", "re-reviewed correction\n");
            git_ok(&f.a, &["commit", "-qam", "fix: correction (STORY-1)"]);
            git_ok(&f.a, &["push", "-q", "origin", "feature"]);
            let h = head(&f.a).unwrap();
            let _env = crate::test_env::AmbientGuard::hermetic_and(
                &f.a,
                None,
                &[
                    ("PATH", Some(&path)),
                    ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
                    ("AIDA_HOME", Some(home.to_str().unwrap())),
                    ("AIDA_CONTAINED", None),
                    ("AIDA_HEADLESS", None),
                ],
            );
            publish_change_authorship(&f.a, ChangeKey::pr(7), "feature", &["STORY-1".into()]);
            h
        } else {
            expected_head
        };
        write(f.temp.path(), "decision", decision);
        f.sync(&f.b);
        {
            let mut env = crate::test_env::AmbientGuard::hermetic_with_seat_and(
                &f.b,
                "advisor",
                &["reviewer"],
                &[
                    ("PATH", Some(&path)),
                    ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
                    ("AIDA_HOME", Some(home_b.to_str().unwrap())),
                    ("AIDA_CONTAINED", None),
                    ("AIDA_HEADLESS", None),
                ],
            );
            // Mint after the final AIDA_HOME is pinned, as the child validates
            // this local parent through that home's keyring.
            let grant = crate::gate::seat_authority::test_support::mint_grant_for(
                &f.b,
                crate::test_env::HERMETIC_TEST_USER,
                "advisor",
                &["reviewer"],
            );
            env.set_key(crate::gate::seat_authority::GRANT_ENV, grant);
            let mut children = crate::review_watch::run_with_program(
                &watch_args(&["--once"]),
                &f.b,
                Some(&program),
            )
            .unwrap();
            assert_eq!(
                children.len(),
                1,
                "watcher must discover and dispatch the shipped head"
            );
            for child in &mut children {
                wait(child, &f.b);
            }
            assert!(f
                .receipts(&f.b, "PR-7")
                .iter()
                .any(|(r, ..)| r.head == h && r.decision == decision));
            let idle = crate::review_watch::run_with_program(
                &watch_args(&["--once"]),
                &f.b,
                Some(&program),
            )
            .unwrap();
            assert!(
                idle.is_empty(),
                "a recorded head must not be reviewed twice"
            );
            let reports = crate::review_watch::readiness_reports(&f.b);
            assert_eq!(reports.len(), 1);
            if decision == "approved" {
                assert!(reports[0].1.contains(": ready ("), "{reports:?}");
            } else {
                assert!(reports[0].1.contains("not ready:"), "{reports:?}");
            }
        }
        f.sync(&f.a);
        assert!(home_b.join(".aida/review-receipts").is_dir());
        assert!(
            !home.join(".aida/review-receipts").exists(),
            "node A must consume publications without B's local receipt evidence"
        );
        let _env = crate::test_env::AmbientGuard::hermetic_and(
            &f.a,
            None,
            &[
                ("PATH", Some(&path)),
                ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
                ("AIDA_HOME", Some(home.to_str().unwrap())),
                ("AIDA_CONTAINED", None),
                ("AIDA_HEADLESS", None),
            ],
        );
        let result = crate::pr_train::pr_train_handler(&crate::pr_train::TrainOptions {
            prs: &[7],
            all_ready: false,
            dry_run: false,
            local_ci: None,
            max: 2,
            no_pull: true,
        });
        if decision == "request-changes" {
            let selected = f.candidates(&f.a, &h);
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].kind, rv::VerdictKind::RequestChanges);
            let receipt = f.receipts(&f.a, "PR-7").pop().unwrap().0;
            assert_eq!(receipt.blocking_finding_origin_ids, vec!["TASK-NODEB-2"]);
            // The train's approval gate reports a request-changes head as
            // lacking approval. Pin that exact PR/head refusal alongside the
            // synced blocking verdict above, excluding forge/transport errors.
            let error = format!("{:#}", result.unwrap_err());
            assert!(
                error.contains("#7: PR-7 has no approval recorded at its head"),
                "train must refuse B's blocking verdict: {error}"
            );
            assert_eq!(
                git_ok(&f.a, &["ls-remote", "origin", "main"])
                    .split_whitespace()
                    .next(),
                Some(f.sha.as_str())
            );
        } else {
            result.unwrap();
            git_ok(&f.a, &["fetch", "-q", "origin", "main"]);
            let landed = git_ok(&f.a, &["rev-parse", "origin/main"]);
            assert_ne!(landed, f.sha);
            assert_eq!(
                git_ok(&f.a, &["show", &format!("{landed}:code")]),
                "re-reviewed correction"
            );
            assert!(git_ok(&f.a, &["log", "-1", "--format=%B", &landed])
                .contains(&format!("Aida-Train-Member: #7 at {h}")));
        }
    }
}
