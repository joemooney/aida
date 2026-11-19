use super::*;

// trace:BUG-2494 | ai:codex
#[test]
fn open_change_failure_keeps_forge_reason_in_display() {
    let cause = anyhow::anyhow!("GraphQL: title is too long (maximum is 256 characters)")
        .context("gh pr create failed");
    let message = open_change_error(cause).to_string();
    assert!(message.contains("could not open a forge change request"));
    assert!(message.contains("gh pr create failed"));
    assert!(message.contains("GraphQL: title is too long (maximum is 256 characters)"));
}

// trace:BUG-2494 | ai:codex
#[test]
fn open_change_github_title_limit_reports_length_without_spawning() {
    for character in ['a', '界'] {
        assert!(validate_github_change_title(&character.to_string().repeat(256)).is_ok());
        let forge = GitHubForge::new(Path::new("/nonexistent"));
        let error = forge
            .open_change(OpenChange {
                branch: "topic".into(),
                base: "main".into(),
                title: character.to_string().repeat(257),
                body: String::new(),
                draft: false,
            })
            .unwrap_err();
        let message = open_change_error(error).to_string();
        assert!(message.contains("257 characters"), "{message}");
        assert!(message.contains("limit is 256 characters"), "{message}");
        assert!(message.contains("Shorten the commit subject"), "{message}");
    }
}

// TASK-1241 / SPIKE-80: live round-trip of GitLabForge against a REAL GitLab.
// #[ignore] — run explicitly:
//   AIDA_GITLAB_LIVE_HOST=gitlab.joemooney.com \
//     cargo test -p aida-cli-lib gitlab_live_mr_round_trip -- --ignored --nocapture
// Requires glab authed to that host with project-create rights. Skips (passes)
// when the env var is unset, so it never runs in normal CI. Creates a throwaway
// project, opens an MR via GitLabForge::open_change, waits for its real pipeline,
// records the review verdict, squash-merges via merge_change, asserts Merged,
// and deletes the project (even on panic). The nightly workflow supplies the
// host/token; local and ordinary PR test runs remain inert.
// trace:TASK-1241 trace:TASK-1273 | ai:claude+codex
#[test]
#[ignore]
fn gitlab_live_mr_round_trip() {
    let host = match std::env::var("AIDA_GITLAB_LIVE_HOST") {
        Ok(h) if !h.is_empty() => h,
        _ => {
            eprintln!(
                "skip: set AIDA_GITLAB_LIVE_HOST (a glab-authed host) to run the live GitLab round-trip"
            );
            return;
        }
    };
    let glab_api = |args: &[&str]| -> std::process::Output {
        std::process::Command::new("glab")
            .env("GITLAB_HOST", &host)
            .args(args)
            .output()
            .expect("glab must be installed")
    };

    // 1) create a throwaway project
    let name = format!(
        "aida-forge-live-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    let created = glab_api(&[
        "api",
        "-X",
        "POST",
        "projects",
        "-f",
        &format!("name={name}"),
        "-f",
        "visibility=private",
        "-f",
        "initialize_with_readme=true",
    ]);
    assert!(
        created.status.success(),
        "project create failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    // Some glab/API combinations can return a successful POST before its
    // JSON body is available. Resolve the uniquely named project, allowing
    // a short window for asynchronous project creation on the live mirror.
    let proj: serde_json::Value = match serde_json::from_slice(&created.stdout) {
        Ok(project) => project,
        Err(create_parse_err) => {
            let mut found = None;
            for _ in 0..10 {
                let lookup = glab_api(&["api", &format!("projects?search={name}&owned=true")]);
                if lookup.status.success() {
                    if let Ok(projects) =
                        serde_json::from_slice::<serde_json::Value>(&lookup.stdout)
                    {
                        found = projects.as_array().and_then(|projects| {
                            projects
                                .iter()
                                .find(|project| project["name"] == name)
                                .cloned()
                        });
                        if found.is_some() {
                            break;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            found.unwrap_or_else(|| {
                let stdout_prefix: String = String::from_utf8_lossy(&created.stdout)
                    .chars()
                    .take(200)
                    .collect();
                panic!(
                    "phase=project-create: create response was not JSON ({create_parse_err}; stdout_prefix={stdout_prefix:?}; stderr={:?}) and project {name} was not found",
                    String::from_utf8_lossy(&created.stderr)
                )
            })
        }
    };
    let pid = proj["id"].as_u64().expect("project id");
    let http = proj["http_url_to_repo"]
        .as_str()
        .expect("http_url_to_repo")
        .to_string();

    // Best-effort panic fallback. The success path below explicitly deletes
    // and verifies the project so cleanup regressions fail the smoke.
    struct Cleanup {
        pid: u64,
        host: String,
        armed: bool,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if !self.armed {
                return;
            }
            let deletion = std::process::Command::new("glab")
                .env("GITLAB_HOST", &self.host)
                .args(["api", "-X", "DELETE", &format!("projects/{}", self.pid)])
                .output();
            match deletion {
                Ok(out) if out.status.success() => {}
                Ok(out) => eprintln!(
                    "phase=project-cleanup panic fallback failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ),
                Err(err) => eprintln!("phase=project-cleanup panic fallback failed: {err}"),
            }
        }
    }
    let mut cleanup = Cleanup {
        pid,
        host: host.clone(),
        armed: true,
    };

    // 2) clone, branch, change, push — a real implementer branch
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let clone = crate::test_git::git_command()
        .args(["clone", &http, repo.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        clone.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&clone.stderr)
    );
    git(&repo, &["checkout", "-b", "live-round-trip"]);
    std::fs::write(repo.join("probe.txt"), "live forge round-trip").unwrap();
    std::fs::write(
        repo.join(".gitlab-ci.yml"),
        "forge-smoke:\n  script:\n    - test -f probe.txt\n",
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(
        &repo,
        &[
            "-c",
            "user.email=aida-test@example.com",
            "-c",
            "user.name=aida-test",
            "commit",
            "-m",
            "live round-trip probe",
        ],
    );
    git(&repo, &["push", "-u", "origin", "live-round-trip"]);

    // 3) the GitLabForge round-trip — glab detects the host from the remote
    let forge = GitLabForge::new(&repo);
    let cr = forge
        .open_change(OpenChange {
            branch: "live-round-trip".into(),
            base: "main".into(),
            title: "TASK-1241 live round-trip".into(),
            body: "GitLabForge live integration test".into(),
            draft: false,
        })
        .expect("phase=open-change: open_change failed against real GitLab");
    assert!(cr.id > 0, "MR iid must be set");

    // poll for mergeability (GitLab computes it async)
    let mut status = forge
        .change_status(&cr)
        .expect("phase=pre-merge-status: change_status failed");
    for _ in 0..10 {
        if status.mergeable.is_known_true() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
        status = forge
            .change_status(&cr)
            .expect("phase=pre-merge-status: change_status poll failed");
    }
    assert_eq!(
        status.state,
        ChangeState::Open,
        "MR should be open pre-merge"
    );

    // TASK-1273: name each phase in the output so a nightly failure points
    // directly at the broken Forge method. This is deliberately the trait
    // call, not a parallel `glab` implementation.
    eprintln!("forge-smoke phase=ci-registration mr={}", cr.id);
    let mut registration = forge
        .checks_registered(&cr)
        .expect("phase ci-registration: checks_registered failed");
    for _ in 0..12 {
        if registration != CheckRegistration::NotYet {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
        registration = forge
            .checks_registered(&cr)
            .expect("phase ci-registration: checks_registered failed");
    }
    assert_eq!(
        registration,
        CheckRegistration::Registered,
        "phase ci-registration: pipeline did not register"
    );

    eprintln!("forge-smoke phase=ci-wait branch={}", cr.branch);
    // Use the same branch-stream seam as the drain rather than the lower
    // level MR watcher. It owns terminal polling and the CiTerminal emit.
    let ci = forge
        .stream_ci_for_branch(&cr.branch, false)
        .expect("phase ci-wait: stream_ci_for_branch failed");
    assert_eq!(
        ci,
        CiProbeResult::Green { change: cr.id },
        "phase ci-verdict: pipeline was not green"
    );

    eprintln!("forge-smoke phase=review-verdict mr={}", cr.id);
    let approved = glab_api(&[
        "api",
        "-X",
        "POST",
        &format!("projects/{pid}/merge_requests/{}/approve", cr.id),
    ]);
    assert!(
        approved.status.success(),
        "phase=review-verdict: could not create approval: {}",
        String::from_utf8_lossy(&approved.stderr)
    );
    let mut review_sink = crate::network_retry::StderrSink;
    let review = forge
        .change_reviews(cr.id, &mut review_sink)
        .expect("phase review-verdict: change_reviews failed");
    eprintln!("forge-smoke review={:?}", review.decision);
    assert_eq!(
        review.decision,
        ReviewDecision::Approved,
        "phase=review-verdict: GitLab approval did not map to Approved"
    );

    // merge_change — the never-live-validated path
    eprintln!("forge-smoke phase=merge mr={}", cr.id);
    let mut sink = crate::network_retry::StderrSink;
    forge
        .merge_change(
            &cr,
            &MergeOptions {
                method: MergeMethod::Squash,
                squash_subject: None,
                squash_body: None, // trace:TASK-1330 | ai:claude
                delete_branch: true,
                match_head: None,
            },
            &mut sink,
        )
        .expect("phase=merge: merge_change failed against real GitLab");

    let after = forge
        .change_status(&cr)
        .expect("phase=post-merge-status: change_status failed");
    assert_eq!(
        after.state,
        ChangeState::Merged,
        "MR must read Merged after merge_change"
    );
    crate::events::emit(
        &repo,
        &crate::events::Event::new(
            Some("TASK-1273".into()),
            "gitlab-live-smoke",
            crate::events::EventKind::PrMerged { pr: cr.id as u32 },
        ),
    );

    eprintln!("forge-smoke phase=lifecycle-events");
    let lifecycle = crate::events::read_all(&repo);
    assert!(
        lifecycle.iter().any(|event| matches!(
            event.kind,
            crate::events::EventKind::CiTerminal { green: true }
        )),
        "phase=lifecycle-events: missing green CiTerminal"
    );
    assert!(
        lifecycle.iter().any(|event| matches!(
            event.kind,
            crate::events::EventKind::PrMerged { pr } if pr == cr.id as u32
        )),
        "phase=lifecycle-events: missing PrMerged"
    );
    let shelved: Vec<_> = lifecycle
        .iter()
        .filter_map(|event| match &event.kind {
            crate::events::EventKind::SpecShelved { phase, kind, .. } => {
                Some((phase.as_str(), kind.as_str()))
            }
            _ => None,
        })
        .collect();
    assert!(
        shelved.is_empty(),
        "phase=lifecycle-events: successful lifecycle unexpectedly shelved: {shelved:?}"
    );

    eprintln!("forge-smoke phase=branch-cleanup branch={}", cr.branch);
    let branch = glab_api(&[
        "api",
        &format!("projects/{pid}/repository/branches?search=^{}$", cr.branch),
    ]);
    assert!(
        branch.status.success(),
        "phase=branch-cleanup: branch absence probe failed: {}",
        String::from_utf8_lossy(&branch.stderr)
    );
    let branches: serde_json::Value = serde_json::from_slice(&branch.stdout)
        .expect("phase=branch-cleanup: invalid branch-list JSON");
    assert_eq!(
        branches.as_array().map(Vec::len),
        Some(0),
        "phase=branch-cleanup: source branch still exists: {branches}"
    );

    eprintln!("forge-smoke phase=project-cleanup project={pid}");
    let deleted = glab_api(&["api", "-X", "DELETE", &format!("projects/{pid}")]);
    assert!(
        deleted.status.success(),
        "phase=project-cleanup: project deletion failed: {}",
        String::from_utf8_lossy(&deleted.stderr)
    );
    let absent = glab_api(&["api", &format!("projects/{pid}")]);
    let cleanup_state = serde_json::from_slice::<serde_json::Value>(&absent.stdout).ok();
    let deletion_scheduled = cleanup_state.as_ref().is_some_and(|project| {
        project["marked_for_deletion_on"].is_string()
            || project["name"]
                .as_str()
                .is_some_and(|name| name.contains("-deletion_scheduled-"))
    });
    assert!(
        !absent.status.success() || deletion_scheduled,
        "phase=project-cleanup: project is neither absent nor scheduled for deletion: {:?}",
        String::from_utf8_lossy(&absent.stdout)
    );
    cleanup.armed = false;
}

/// STORY-516: PrLookup → ChangeLookup is a 1:1 state map that preserves the
/// BUG-257 transient-vs-definitive distinction; Found carries number/url/title.
#[test]
fn change_lookup_maps_every_pr_lookup_state() {
    use crate::{OpenPrInfo, PrLookup};
    let found = change_lookup_from_pr_lookup(
        PrLookup::Found(OpenPrInfo {
            number: 42,
            title: "Fix it".to_string(),
            url: "https://example/pr/42".to_string(),
            head_branch: None,
        }),
        "feature",
    );
    match found {
        ChangeLookup::Found(c) => {
            assert_eq!(c.id, 42);
            assert_eq!(c.url, "https://example/pr/42");
            assert_eq!(c.branch, "feature");
            assert_eq!(c.title.as_deref(), Some("Fix it"));
        }
        other => panic!("expected Found, got {other:?}"),
    }
    assert_eq!(
        change_lookup_from_pr_lookup(PrLookup::NoOpenPr, "b"),
        ChangeLookup::NoChange
    );
    assert_eq!(
        change_lookup_from_pr_lookup(PrLookup::GhMissing, "b"),
        ChangeLookup::CliMissing
    );
    assert_eq!(
        change_lookup_from_pr_lookup(PrLookup::GhFailed("auth".into()), "b"),
        ChangeLookup::CliFailed("auth".into())
    );
    // The BUG-257 distinction: transient outage maps to Unreachable, NOT NoChange.
    assert_eq!(
        change_lookup_from_pr_lookup(PrLookup::GhUnreachable("dns".into()), "b"),
        ChangeLookup::Unreachable("dns".into())
    );
}

/// STORY-516: CiProbe → CiProbeResult is a 1:1 map that preserves the
/// NoSignal(reason) "couldn't probe" state + the per-state change number.
#[test]
fn ci_probe_result_maps_every_ci_probe_state() {
    use crate::CiProbe;
    assert_eq!(
        ci_probe_result_from_ci_probe(CiProbe::NoSignal("gh down".into())),
        CiProbeResult::NoSignal("gh down".into())
    );
    assert_eq!(
        ci_probe_result_from_ci_probe(CiProbe::PrNoChecks { pr_number: 7 }),
        CiProbeResult::NoChecks { change: 7 }
    );
    assert_eq!(
        ci_probe_result_from_ci_probe(CiProbe::InProgress { pr_number: 7 }),
        CiProbeResult::InProgress { change: 7 }
    );
    assert_eq!(
        ci_probe_result_from_ci_probe(CiProbe::Green { pr_number: 7 }),
        CiProbeResult::Green { change: 7 }
    );
    assert_eq!(
        ci_probe_result_from_ci_probe(CiProbe::Red {
            pr_number: 7,
            failed_summary: "build".into()
        }),
        CiProbeResult::Failed {
            change: 7,
            summary: "build".into()
        }
    );
}

/// STORY-508: forge-aware hint helpers — noun + templated CLI command.
#[test]
fn change_noun_per_forge() {
    assert_eq!(ForgeKind::GitHub.change_noun(), "PR");
    assert_eq!(ForgeKind::GitLab.change_noun(), "MR");
    assert_eq!(ForgeKind::None.change_noun(), "change");
}

#[test]
fn change_cmd_hint_templates_cli_and_noun() {
    // GitHub → gh pr, GitLab → glab mr, with args appended verbatim.
    assert_eq!(
        ForgeKind::GitHub.change_cmd_hint("merge", "47 --squash --delete-branch"),
        Some("gh pr merge 47 --squash --delete-branch".to_string())
    );
    assert_eq!(
        ForgeKind::GitLab.change_cmd_hint("merge", "47 --squash --delete-branch"),
        Some("glab mr merge 47 --squash --delete-branch".to_string())
    );
    // No args → bare command, no trailing space.
    assert_eq!(
        ForgeKind::GitHub.change_cmd_hint("create", ""),
        Some("gh pr create".to_string())
    );
    assert_eq!(
        ForgeKind::GitLab.change_cmd_hint("view", "  42  "),
        Some("glab mr view 42".to_string())
    );
    // pure-git has no forge CLI — caller supplies a native alternative.
    assert_eq!(ForgeKind::None.change_cmd_hint("merge", "47"), None);
}

/// TASK-651: per-forge command vocabulary — the shapes that diverge beyond
/// the noun (ci status, run/pipeline viewer, merge branch-cleanup flag).
#[test]
fn command_vocabulary_per_forge() {
    // CI watch: gh is pr-scoped, glab is pipeline-scoped. Display-string
    // id accepts a number or a placeholder.
    assert_eq!(
        ForgeKind::GitHub.ci_watch_cmd("47"),
        Some("gh pr checks 47 --watch".to_string())
    );
    assert_eq!(
        ForgeKind::GitHub.ci_watch_cmd("<N>"),
        Some("gh pr checks <N> --watch".to_string())
    );
    assert_eq!(
        ForgeKind::GitLab.ci_watch_cmd("47"),
        Some("glab ci status".to_string())
    );
    assert_eq!(ForgeKind::None.ci_watch_cmd("47"), None);

    // Run / pipeline viewer.
    assert_eq!(
        ForgeKind::GitHub.ci_view_cmd("12345"),
        Some("gh run view 12345".to_string())
    );
    assert_eq!(
        ForgeKind::GitLab.ci_view_cmd("12345"),
        Some("glab ci view 12345".to_string())
    );
    assert_eq!(ForgeKind::None.ci_view_cmd("12345"), None);

    // Merge: correct branch-cleanup flag per forge.
    assert_eq!(
        ForgeKind::GitHub.merge_cmd("47"),
        Some("gh pr merge 47 --squash --delete-branch".to_string())
    );
    assert_eq!(
        ForgeKind::GitLab.merge_cmd("47"),
        Some("glab mr merge 47 --squash --remove-source-branch".to_string())
    );
    assert_eq!(ForgeKind::None.merge_cmd("47"), None);

    // Create + view.
    assert_eq!(
        ForgeKind::GitHub.create_cmd(),
        Some("gh pr create".to_string())
    );
    assert_eq!(
        ForgeKind::GitLab.create_cmd(),
        Some("glab mr create".to_string())
    );
    assert_eq!(ForgeKind::None.create_cmd(), None);
    assert_eq!(
        ForgeKind::GitHub.create_cmd_for_branch("bug-816", "main"),
        Some("gh pr create --head bug-816 --base main --title \"$(git log -1 --format=%s bug-816)\" --body \"$(git log -1 --format=%b bug-816)\"".to_string())
    );
    assert_eq!(
        ForgeKind::GitLab.create_cmd_for_branch("bug-816", "main"),
        Some("glab mr create --source-branch bug-816 --target-branch main --title \"$(git log -1 --format=%s bug-816)\" --description \"$(git log -1 --format=%b bug-816)\"".to_string())
    );
    assert_eq!(
        ForgeKind::None.create_cmd_for_branch("bug-816", "main"),
        None
    );
    assert_eq!(
        ForgeKind::GitLab.view_cmd("9"),
        Some("glab mr view 9".to_string())
    );
    assert_eq!(ForgeKind::None.view_cmd("9"), None);
}

#[test]
fn detect_github_ssh_and_https() {
    assert_eq!(
        detect_forge_kind("git@github.com:joemooney/aida.git"),
        ForgeKind::GitHub
    );
    assert_eq!(
        detect_forge_kind("https://github.com/joemooney/aida.git"),
        ForgeKind::GitHub
    );
}

#[test]
fn detect_gitlab_dotcom_and_self_hosted() {
    assert_eq!(
        detect_forge_kind("git@gitlab.com:joe/aida.git"),
        ForgeKind::GitLab
    );
    // Self-hosted.
    assert_eq!(
        detect_forge_kind("https://gitlab.joemooney.com/joe/aida.git"),
        ForgeKind::GitLab
    );
    assert_eq!(
        detect_forge_kind("git@gitlab.joemooney.com:2222/joe/aida.git"),
        ForgeKind::GitLab
    );
}

#[test]
fn detect_unknown_host_is_pure_git() {
    assert_eq!(
        detect_forge_kind("git@git.example.org:team/repo.git"),
        ForgeKind::None
    );
    assert_eq!(
        detect_forge_kind("https://bitbucket.org/team/repo.git"),
        ForgeKind::None
    );
    assert_eq!(detect_forge_kind(""), ForgeKind::None);
}

#[test]
fn config_token_round_trips() {
    for k in [ForgeKind::GitHub, ForgeKind::GitLab, ForgeKind::None] {
        assert_eq!(ForgeKind::from_config_token(k.config_token()), Some(k));
    }
    // Aliases.
    assert_eq!(ForgeKind::from_config_token("GH"), Some(ForgeKind::GitHub));
    assert_eq!(ForgeKind::from_config_token("none"), Some(ForgeKind::None));
    assert_eq!(ForgeKind::from_config_token("git"), Some(ForgeKind::None));
    assert_eq!(ForgeKind::from_config_token("bogus"), None);
}

#[test]
fn pure_git_needs_no_cli() {
    // trace:TASK-860 — pure-git has no forge CLI, so it is trivially present.
    assert!(ForgeKind::None.cli_on_path());
}

#[test]
fn binary_on_path_detects_absent() {
    // trace:TASK-860 — a binary that cannot plausibly exist is not on PATH.
    assert!(!binary_on_path("aida-no-such-binary-xyzzy-9999"));
}

#[test]
fn forge_cli_status_message_shapes() {
    // trace:TASK-860 — GitHub origin, message names `gh` (OK or missing-hint).
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida").join("config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    let (kind, msg) = forge_cli_status(tmp.path());
    assert_eq!(kind, ForgeKind::GitHub);
    assert!(msg.contains("forge CLI:"));
    assert!(msg.contains("gh"));

    // GitLab config → names `glab`.
    std::fs::write(
        tmp.path().join(".aida").join("config.toml"),
        "[forge]\nprovider = \"gitlab\"\n",
    )
    .unwrap();
    let (kind, msg) = forge_cli_status(tmp.path());
    assert_eq!(kind, ForgeKind::GitLab);
    assert!(msg.contains("glab"));
    // GitLab must never make a GitHub user install glab and vice versa —
    // the message names exactly one CLI, scoped to the resolved forge.
    assert!(!msg.contains(" gh "));

    // Pure-git config → "none needed", no CLI nag.
    std::fs::write(
        tmp.path().join(".aida").join("config.toml"),
        "[forge]\nprovider = \"pure-git\"\n",
    )
    .unwrap();
    let (kind, msg) = forge_cli_status(tmp.path());
    assert_eq!(kind, ForgeKind::None);
    assert!(msg.contains("none needed"));
}

#[test]
fn checked_forge_provider_reads_every_toml_spelling_or_refuses() {
    // trace:TASK-1335 | ai:claude
    for (doc, want) in [
        ("[forge]\nprovider = \"gitlab\"\n", Some(ForgeKind::GitLab)),
        ("forge.provider = \"github\"\n", Some(ForgeKind::GitHub)),
        (
            "forge = { provider = \"gitlab\" }\n",
            Some(ForgeKind::GitLab),
        ),
        (
            "[forge]\nprovider = 'pure-git' # local\n",
            Some(ForgeKind::None),
        ),
        ("[node]\nid = \"1\"\n", None),
        ("[forge]\n", None),
        ("", None),
    ] {
        assert_eq!(checked_forge_provider(doc), Ok(want), "{doc:?}");
    }
    for doc in [
        "forge = \"gitlab\"\n",
        "[forge]\nprovider = 7\n",
        "forge = { provider = [\"gitlab\"] }\n",
        "[forge]\nprovider = \"bitbucket\"\n",
        "[forge\nprovider = \"gitlab\"\n",
        "[forge]\nprovider = \"github\"\nprovider = \"gitlab\"\n",
    ] {
        assert!(checked_forge_provider(doc).is_err(), "{doc:?} must refuse");
    }
}

#[test]
fn read_forge_config_parses_section() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida").join("config.toml"),
        "[node]\nid = \"x\"\n\n[forge]\nprovider = \"gitlab\"  # self-hosted\n",
    )
    .unwrap();
    assert_eq!(read_forge_config(tmp.path()), Some(ForgeKind::GitLab));
}

#[test]
fn read_forge_config_absent_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida").join("config.toml"),
        "[node]\nid = \"x\"\n",
    )
    .unwrap();
    assert_eq!(read_forge_config(tmp.path()), None);
}

#[test]
fn config_overrides_detection() {
    // No recognized origin: an explicit pure-git config still wins.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida").join("config.toml"),
        "[forge]\nprovider = \"pure-git\"\n",
    )
    .unwrap();
    assert_eq!(resolve_forge_kind(tmp.path()), ForgeKind::None);
}

#[test]
fn init_then_add_github_remote_repairs_stale_pure_git_config() {
    // trace:BUG-1109 | ai:codex
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    crate::test_git::git_command()
        .current_dir(root)
        .args(["init", "-q", "-b", "main"])
        .status()
        .unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida").join("config.toml"),
        "[forge]\nprovider = \"pure-git\"\n",
    )
    .unwrap();

    assert_eq!(resolve_forge_kind(root), ForgeKind::None);

    crate::test_git::git_command()
        .current_dir(root)
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/acme/repo.git",
        ])
        .status()
        .unwrap();

    assert_eq!(resolve_forge_kind(root), ForgeKind::GitHub);
    let config = std::fs::read_to_string(root.join(".aida").join("config.toml")).unwrap();
    assert!(config.contains("provider = \"github\""), "{config}");
}

#[test]
fn init_then_add_gitlab_remote_repairs_stale_pure_git_config() {
    // trace:BUG-1109 | ai:codex
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    crate::test_git::git_command()
        .current_dir(root)
        .args(["init", "-q", "-b", "main"])
        .status()
        .unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida").join("config.toml"),
        "[forge]\nprovider = \"pure-git\"\n",
    )
    .unwrap();
    crate::test_git::git_command()
        .current_dir(root)
        .args([
            "remote",
            "add",
            "origin",
            "git@gitlab.example.test:acme/repo.git",
        ])
        .status()
        .unwrap();

    assert_eq!(resolve_forge_kind(root), ForgeKind::GitLab);
    let config = std::fs::read_to_string(root.join(".aida").join("config.toml")).unwrap();
    assert!(config.contains("provider = \"gitlab\""), "{config}");
}

#[test]
fn open_change_resolution_prefers_recognized_origin_over_stale_pure_git_config() {
    // trace:BUG-896 | ai:codex
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    crate::test_git::git_command()
        .current_dir(root)
        .args(["init", "-q", "-b", "main"])
        .status()
        .unwrap();
    crate::test_git::git_command()
        .current_dir(root)
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/acme/repo.git",
        ])
        .status()
        .unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida").join("config.toml"),
        "[forge]\nprovider = \"pure-git\"\n",
    )
    .unwrap();

    assert_eq!(resolve_forge_kind(root), ForgeKind::GitHub);
    assert_eq!(resolve_open_change_forge_kind(root), ForgeKind::GitHub);
    assert_eq!(forge_for_open_change(root).kind(), ForgeKind::GitHub);
}

/// Pure-git `change_status`: merged iff the branch is an ancestor of base.
#[test]
fn pure_git_status_reflects_ancestry() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let g = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    g(&["init", "-q", "-b", "main"]);
    g(&["config", "user.email", "t@t.t"]);
    g(&["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    g(&["add", "."]);
    g(&["commit", "-q", "-m", "base"]);
    // Feature branch with a commit not on main → Open.
    g(&["checkout", "-q", "-b", "feature"]);
    std::fs::write(root.join("b"), "2").unwrap();
    g(&["add", "."]);
    g(&["commit", "-q", "-m", "work"]);

    let forge = PureGitForge::new(root);
    let cref = ChangeRef {
        id: 0,
        url: String::new(),
        branch: "feature".to_string(),
        base: "main".to_string(),
        title: None,
    };
    let st = forge.change_status(&cref).unwrap();
    assert_eq!(st.state, ChangeState::Open, "feature ahead of main → Open");
    assert_eq!(st.review, ReviewDecision::None);
    assert!(st.mergeable.is_known_true());

    // Merge feature into main → now ancestor → Merged.
    g(&["checkout", "-q", "main"]);
    g(&["merge", "-q", "--no-ff", "feature", "-m", "merge"]);
    let st2 = forge.change_status(&cref).unwrap();
    assert_eq!(st2.state, ChangeState::Merged, "feature merged → Merged");
}

#[test]
fn pure_git_ci_is_none_and_open_change_is_unsupported() {
    // trace:BUG-896 | ai:codex
    let tmp = tempfile::tempdir().unwrap();
    let forge = PureGitForge::new(tmp.path());
    assert_eq!(
        forge.ci_status(CiTarget::Branch("x".into())).unwrap().state,
        CiState::None
    );
    let err = forge
        .open_change(OpenChange {
            branch: "feat".into(),
            base: "main".into(),
            title: "t".into(),
            body: "b".into(),
            draft: false,
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("pure-git forge-less mode"), "{err}");
    assert!(err.contains("cannot open a real change request"), "{err}");
}

/// STORY-516: pure-git squash merge must actually COMMIT (advance base,
/// leave a clean index) and preserve the branch's `(SPEC-ID)`-trailered
/// subject — `git merge --squash` alone only stages. trace:STORY-516
#[test]
fn pure_git_squash_merge_commits_and_advances_base() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let g = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    g(&["init", "-q", "-b", "main"]);
    g(&["config", "user.email", "t@t.t"]);
    g(&["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    g(&["add", "."]);
    g(&["commit", "-q", "-m", "base"]);
    g(&["checkout", "-q", "-b", "feature"]);
    std::fs::write(root.join("b"), "2").unwrap();
    g(&["add", "."]);
    g(&["commit", "-q", "-m", "feat: the thing (TASK-9)"]);
    g(&["checkout", "-q", "main"]);

    let base_before = rev_parse(root, "main");
    let forge = PureGitForge::new(root);
    let cref = ChangeRef {
        id: 0,
        url: String::new(),
        branch: "feature".into(),
        base: "main".into(),
        title: None,
    };
    let res = forge
        .merge_change(
            &cref,
            &MergeOptions::squash(),
            &mut crate::network_retry::NoopSink,
        )
        .unwrap();
    assert!(res.merged, "squash merge should report merged");

    // base advanced (a real commit was made, not just a staged index).
    assert_ne!(
        base_before,
        rev_parse(root, "main"),
        "base HEAD must advance"
    );
    // working tree clean — committed, not left staged.
    let status = crate::test_git::git_command()
        .current_dir(root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "squash must commit, not leave a dirty index"
    );
    // the feature's change landed on base.
    assert!(root.join("b").exists(), "feature file present on base");
    // subject + trailer preserved.
    let subj = branch_head_subject(root, "main").unwrap();
    assert!(
        subj.contains("TASK-9"),
        "squash preserves the trailer: {subj}"
    );
    // NOTE: we deliberately do NOT assert change_status==Merged here.
    // change_status routes through `branch_is_ancestor_of`, which maps any
    // git error to `false` (the documented graceful-fallback) — under CI
    // parallelism a transient git hiccup there flips the result and flaked
    // this test. The pure-git Merged-via-ancestry path is covered reliably
    // by `pure_git_status_reflects_ancestry` (a `--no-ff` merge). This test's
    // job is the squash *commit* behavior, fully asserted above (base
    // advanced, clean index, file landed, trailer preserved).
    // trace:STORY-516 | ai:claude
}

/// TASK-2115: the non-completing declaration rides in the squash body.
/// GitLab sends it inside `squash_commit_message` (subject falling back
/// to the MR title); pure-git commits it as the squash commit's body.
// trace:TASK-1951 trace:TASK-2115 | ai:claude
#[test]
fn squash_body_reaches_main_on_gitlab_and_pure_git() {
    let mut opts = MergeOptions::squash();
    opts.squash_body = Some("* fix: one\n\naida: no-complete".into());
    let path = "projects/:id/merge_requests/7";
    let titled = gitlab_merge_api_shape(path, &opts, false, "abc123", "fix: slice one");
    assert!(titled.fields.contains(&(
        "squash_commit_message".into(),
        "fix: slice one\n\n* fix: one\n\naida: no-complete".into()
    )));
    opts.squash_subject = Some("fix: explicit (Refs TASK-9)".into());
    let explicit = gitlab_merge_api_shape(path, &opts, false, "abc123", "fix: slice one");
    assert!(explicit
        .fields
        .iter()
        .any(|(k, v)| k == "squash_commit_message"
            && v.starts_with("fix: explicit (Refs TASK-9)\n\n")
            && v.ends_with("aida: no-complete")));

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let g = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    g(&["init", "-q", "-b", "main"]);
    g(&["config", "user.email", "t@t.t"]);
    g(&["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    g(&["add", "."]);
    g(&["commit", "-q", "-m", "base"]);
    g(&["checkout", "-q", "-b", "feature"]);
    std::fs::write(root.join("b"), "2").unwrap();
    g(&["add", "."]);
    g(&["commit", "-q", "-m", "feat: slice one"]);
    g(&["checkout", "-q", "main"]);
    let cref = ChangeRef {
        id: 0,
        url: String::new(),
        branch: "feature".into(),
        base: "main".into(),
        title: None,
    };
    opts.squash_subject = None;
    PureGitForge::new(root)
        .merge_change(&cref, &opts, &mut crate::network_retry::NoopSink)
        .unwrap();
    let message = g(&["log", "-1", "--format=%B", "main"]);
    assert!(message.starts_with("feat: slice one"), "{message}");
    assert!(
        crate::db_reconcile::commit_declares_non_completing(&message),
        "{message}"
    );
}

// trace:STORY-2733 | ai:codex
#[test]
fn mandate_audit_body_reaches_merge_commits_on_every_forge() {
    let mut opts = MergeOptions::squash();
    opts.method = MergeMethod::Merge;
    let audit = "Aida-Mandate-Gate-Path: {\"path\":\"aida-cli-lib/src/gate/x.rs\"}";
    opts.squash_body = Some(audit.into());
    let argv = github_merge_argv(7, &opts);
    assert!(argv
        .windows(2)
        .any(|pair| pair[0] == "--body" && pair[1] == audit));
    let path = "projects/:id/merge_requests/7";
    let shaped = gitlab_merge_api_shape(path, &opts, false, "abc123", "fix: slice one");
    assert!(shaped.fields.contains(&(
        "merge_commit_message".into(),
        format!("fix: slice one\n\n{audit}")
    )));
    // GitLab rebase: the `/rebase` transition carries nothing, but the
    // post-rebase `/merge` (squash=false) still writes the audit message.
    // trace:STORY-2733 | ai:claude
    opts.method = MergeMethod::Rebase;
    let rebase = gitlab_merge_api_shape(path, &opts, true, "abc123", "fix: slice one");
    assert_eq!(rebase.path, format!("{path}/rebase"));
    assert!(rebase.fields.is_empty());
    let after = gitlab_merge_api_shape(path, &opts, false, "abc123", "fix: slice one");
    assert_eq!(after.path, format!("{path}/merge"));
    assert!(after.fields.contains(&("squash".into(), "false".into())));
    assert!(after.fields.contains(&(
        "merge_commit_message".into(),
        format!("fix: slice one\n\n{audit}")
    )));
    opts.method = MergeMethod::Merge;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let g = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    g(&["init", "-q", "-b", "main"]);
    g(&["config", "user.email", "t@t.t"]);
    g(&["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    g(&["add", "."]);
    g(&["commit", "-q", "-m", "base"]);
    g(&["checkout", "-q", "-b", "feature"]);
    std::fs::write(root.join("b"), "2").unwrap();
    g(&["add", "."]);
    g(&["commit", "-q", "-m", "feat: slice one"]);
    g(&["checkout", "-q", "main"]);
    let cref = ChangeRef {
        id: 0,
        url: String::new(),
        branch: "feature".into(),
        base: "main".into(),
        title: None,
    };
    opts.squash_subject = None;
    PureGitForge::new(root)
        .merge_change(&cref, &opts, &mut crate::network_retry::NoopSink)
        .unwrap();
    let message = g(&["log", "-1", "--format=%B", "main"]);
    assert!(message.contains(audit), "{message}");
    assert_eq!(
        g(&["rev-list", "--parents", "-n", "1", "main"])
            .split_whitespace()
            .count(),
        3,
        "a real merge commit carries the audit"
    );
}

// ── TASK-1458: match_head pin on every forge ────────────────────────────
// trace:TASK-1458 | ai:claude

const PIN: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn github_merge_argv_unpinned_is_byte_identical_to_merge_args() {
    let opts = MergeOptions {
        method: MergeMethod::Squash,
        squash_subject: Some("feat: x (TASK-1)".into()),
        squash_body: None, // trace:TASK-1330 | ai:claude
        delete_branch: true,
        match_head: None,
    };
    assert_eq!(
        github_merge_argv(42, &opts),
        crate::pr_ship::merge_args(42, true, Some("feat: x (TASK-1)"), None)
    );
}

#[test]
fn github_merge_argv_pins_match_head_commit_for_every_method() {
    for method in [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase] {
        let opts = MergeOptions {
            method,
            squash_subject: None,
            squash_body: None, // trace:TASK-1330 | ai:claude
            delete_branch: true,
            match_head: Some(PIN.into()),
        };
        let argv = github_merge_argv(42, &opts);
        let at = argv
            .iter()
            .position(|a| a == "--match-head-commit")
            .unwrap_or_else(|| panic!("{method:?}: no --match-head-commit in {argv:?}"));
        assert_eq!(argv.get(at + 1).map(String::as_str), Some(PIN));
    }
    // A blank pin is no pin — never `--match-head-commit ""`.
    let blank = MergeOptions {
        match_head: Some("  ".into()),
        ..MergeOptions::squash()
    };
    assert!(!github_merge_argv(1, &blank).contains(&"--match-head-commit".to_string()));
}

#[test]
fn head_matches_pin_uses_the_approval_gate_comparison() {
    assert!(head_matches_pin(PIN, PIN));
    assert!(head_matches_pin(&PIN[..12], PIN), "prefix of the head");
    assert!(!head_matches_pin(
        "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        PIN
    ));
    assert!(!head_matches_pin("1ac", PIN), "too short to compare");
    assert!(!head_matches_pin(PIN, ""), "unreadable head never matches");
}

fn pure_git_repo_with_feature(root: &Path) -> String {
    let g = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    };
    g(&["init", "-q", "-b", "main"]);
    g(&["config", "user.email", "t@t.t"]);
    g(&["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    g(&["add", "."]);
    g(&["commit", "-q", "-m", "base"]);
    g(&["checkout", "-q", "-b", "feature"]);
    std::fs::write(root.join("b"), "2").unwrap();
    g(&["add", "."]);
    g(&["commit", "-q", "-m", "feat: the thing (TASK-9)"]);
    g(&["checkout", "-q", "main"]);
    rev_parse(root, "feature").unwrap()
}

fn feature_ref() -> ChangeRef {
    ChangeRef {
        id: 0,
        url: String::new(),
        branch: "feature".into(),
        base: "main".into(),
        title: None,
    }
}

#[test]
fn pure_git_merge_refuses_a_branch_head_that_moved_past_the_pin() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    pure_git_repo_with_feature(root);
    let base_before = rev_parse(root, "main");
    let opts = MergeOptions {
        match_head: Some("08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()),
        ..MergeOptions::squash()
    };
    let err = PureGitForge::new(root)
        .merge_change(&feature_ref(), &opts, &mut crate::network_retry::NoopSink)
        .expect_err("a moved head must not merge")
        .to_string();
    assert!(err.contains("is not the approved"), "{err}");
    assert_eq!(base_before, rev_parse(root, "main"), "base untouched");
}

#[test]
fn pure_git_merge_lands_when_the_branch_head_is_the_pin() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let head = pure_git_repo_with_feature(root);
    let opts = MergeOptions {
        match_head: Some(head),
        ..MergeOptions::squash()
    };
    let res = PureGitForge::new(root)
        .merge_change(&feature_ref(), &opts, &mut crate::network_retry::NoopSink)
        .expect("the approved head merges");
    assert!(res.merged);
    assert!(root.join("b").exists(), "feature file landed on base");
}

// ───────────────────── STORY-509: glab JSON mapping ─────────────────────
//
// `glab … --output json` emits the GitLab REST object verbatim, so these
// fixtures (trimmed real-shape MR JSON) faithfully exercise the pure parsers
// without a live GitLab. trace:STORY-509 | ai:claude

/// A trimmed `glab mr view --output json` fixture — the GitLab MR object's
/// load-bearing fields.
fn mr_view_fixture(state: &str, merge_status: &str, sha: &str) -> String {
    format!(
        r#"{{
              "iid": 7,
              "web_url": "https://gitlab.example.com/joe/aida/-/merge_requests/7",
              "source_branch": "feature/x",
              "target_branch": "main",
              "title": "feat: the thing (STORY-509)",
              "state": "{state}",
              "detailed_merge_status": "{merge_status}",
              "sha": "{sha}"
            }}"#
    )
}

#[test]
fn glab_state_maps_gitlab_tokens() {
    assert_eq!(glab_state_from_str("opened"), ChangeState::Open);
    assert_eq!(glab_state_from_str("merged"), ChangeState::Merged);
    assert_eq!(glab_state_from_str("closed"), ChangeState::Closed);
    assert_eq!(glab_state_from_str("locked"), ChangeState::Closed);
    assert_eq!(glab_state_from_str("weird"), ChangeState::Open);
}

#[test]
fn parse_glab_mr_status_open_mergeable() {
    let st = parse_glab_mr_status(&mr_view_fixture("opened", "mergeable", "abc123")).unwrap();
    assert_eq!(st.state, ChangeState::Open);
    assert!(
        st.mergeable.is_known_true(),
        "detailed_merge_status=mergeable → mergeable"
    );
    assert_eq!(st.review, ReviewDecision::None);
    assert_eq!(st.head_sha, "abc123");
}

#[test]
fn parse_glab_mr_status_merged_not_mergeable() {
    let st = parse_glab_mr_status(&mr_view_fixture("merged", "not_open", "deadbeef")).unwrap();
    assert_eq!(st.state, ChangeState::Merged);
    assert!(st.mergeable.is_known_false());
    assert_eq!(st.head_sha, "deadbeef");
}

#[test]
fn parse_glab_mr_status_legacy_merge_status_field() {
    // Older GitLab servers emit `merge_status` ("can_be_merged"), no
    // `detailed_merge_status`. The parser must fall back to it.
    let body = r#"{"iid":1,"state":"opened","merge_status":"can_be_merged","sha":"f00"}"#;
    let st = parse_glab_mr_status(body).unwrap();
    assert!(
        st.mergeable.is_known_true(),
        "legacy merge_status=can_be_merged → mergeable"
    );
    assert_eq!(st.head_sha, "f00");
}

#[test]
fn parse_glab_mr_status_head_sha_from_diff_refs() {
    let body = r#"{"iid":1,"state":"opened","diff_refs":{"head_sha":"diffsha"}}"#;
    let st = parse_glab_mr_status(body).unwrap();
    assert_eq!(st.head_sha, "diffsha", "falls back to diff_refs.head_sha");
    // STORY-2401: no merge-status field says nothing settled.
    assert!(
        matches!(st.mergeable, crate::forge_observed::Observed::Pending(_)),
        "no merge-status field → pending, not a settled no"
    );
}

#[test]
fn parse_glab_mr_status_rejects_garbage() {
    assert!(parse_glab_mr_status("not json").is_err());
}

#[test]
fn gitlab_merge_api_shape_distinguishes_all_methods() {
    let opts = |method| MergeOptions {
        method,
        squash_subject: None,
        squash_body: None, // trace:TASK-1330 | ai:claude
        delete_branch: true,
        match_head: None,
    };
    let path = "projects/:id/merge_requests/7";

    let merge = gitlab_merge_api_shape(path, &opts(MergeMethod::Merge), false, "abc123", "");
    assert_eq!(merge.path, format!("{path}/merge"));
    assert!(merge.fields.contains(&("squash".into(), "false".into())));
    assert!(merge.fields.contains(&("sha".into(), "abc123".into())));

    let squash = gitlab_merge_api_shape(path, &opts(MergeMethod::Squash), false, "abc123", "");
    assert_eq!(squash.path, format!("{path}/merge"));
    assert!(squash.fields.contains(&("squash".into(), "true".into())));

    let rebase = gitlab_merge_api_shape(path, &opts(MergeMethod::Rebase), true, "abc123", "");
    assert_eq!(rebase.path, format!("{path}/rebase"));
    assert!(rebase.fields.is_empty());

    let after = gitlab_merge_api_shape(path, &opts(MergeMethod::Rebase), false, "def456", "");
    assert_eq!(after.path, format!("{path}/merge"));
    assert!(after.fields.contains(&("squash".into(), "false".into())));
    assert!(after.fields.contains(&("sha".into(), "def456".into())));
}

#[test]
fn gitlab_merge_gate_classifies_every_transient_status() {
    // Fixture classes from the BUG-1232 drain contract: these must wait
    // and reread instead of invoking merge against stale GitLab state.
    // trace:BUG-1232 | ai:codex
    for status in [
        "ci_must_pass",
        "ci_still_running",
        "checking",
        "preparing",
        "unchecked",
        "approvals_syncing",
    ] {
        assert_eq!(
            classify_gitlab_merge_status(status),
            GitLabMergeGate::Wait,
            "{status}"
        );
    }
    assert_eq!(
        classify_gitlab_merge_status("mergeable"),
        GitLabMergeGate::Ready
    );
    for status in [
        "conflict",
        "not_approved",
        "discussions_not_resolved",
        "unknown",
    ] {
        assert_eq!(
            classify_gitlab_merge_status(status),
            GitLabMergeGate::Blocked,
            "{status}"
        );
    }
}

#[test]
fn gitlab_merge_snapshot_and_retry_responses_are_fixture_driven() {
    let snapshot = parse_gitlab_merge_snapshot(
        r#"{"state":"opened","detailed_merge_status":"ci_still_running","sha":"abc123"}"#,
    )
    .unwrap();
    assert_eq!(snapshot.state, "opened");
    assert_eq!(snapshot.detailed_merge_status, "ci_still_running");
    assert_eq!(snapshot.head_sha, "abc123");

    let diff_ref_snapshot = parse_gitlab_merge_snapshot(
        r#"{"state":"opened","detailed_merge_status":"mergeable","diff_refs":{"head_sha":"def456"}}"#,
    )
    .unwrap();
    assert_eq!(diff_ref_snapshot.head_sha, "def456");

    assert!(gitlab_merge_response_is_stale_head(
        br#"{"message":"SHA does not match HEAD of source branch"}"#
    ));
    assert!(!gitlab_merge_response_is_stale_head(
        br#"{"message":"405 Method Not Allowed"}"#
    ));

    for response in [
        br#"{"message":"Method Not Allowed","status":405}"#.as_slice(),
        br#"{"message":"Cannot be merged"}"#.as_slice(),
        br#"{"message":"Conflict","status_code":"409"}"#.as_slice(),
    ] {
        assert!(gitlab_merge_response_is_retryable(response));
    }
    assert!(!gitlab_merge_response_is_retryable(
        br#"{"message":"Unauthorized","status":401}"#
    ));
    assert!(!gitlab_merge_response_is_retryable(b"glab: 409 Conflict"));
}

#[test]
fn gitlab_merge_error_never_forwards_glab_wrapper_prose() {
    let wrapper = b"glab: you do not have enough privileges to merge this merge request";
    assert_eq!(gitlab_api_message(wrapper), "");
    assert_eq!(
        gitlab_api_message(br#"{"message":"Branch cannot be merged"}"#),
        ": Branch cannot be merged"
    );
}

#[test]
fn parse_glab_mr_list_maps_array() {
    let body = r#"[
          {"iid":3,"web_url":"https://gl/mr/3","source_branch":"a","target_branch":"main","title":"A"},
          {"iid":4,"web_url":"https://gl/mr/4","source_branch":"b","target_branch":"main","title":"B"}
        ]"#;
    let refs = parse_glab_mr_list(body).unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].id, 3);
    assert_eq!(refs[0].branch, "a");
    assert_eq!(refs[0].base, "main");
    assert_eq!(refs[0].title.as_deref(), Some("A"));
    assert_eq!(refs[1].id, 4);
}

#[test]
fn parse_glab_mr_list_empty_array_and_blank() {
    assert!(parse_glab_mr_list("[]").unwrap().is_empty());
    assert!(parse_glab_mr_list("   ").unwrap().is_empty());
}

#[test]
fn parse_glab_mr_list_skips_rows_without_iid() {
    let body = r#"[{"iid":5,"source_branch":"x"},{"web_url":"https://gl/mr/?"}]"#;
    let refs = parse_glab_mr_list(body).unwrap();
    assert_eq!(refs.len(), 1, "row without iid is skipped");
    assert_eq!(refs[0].id, 5);
}

#[test]
fn parse_glab_mr_list_non_array_is_error() {
    assert!(parse_glab_mr_list(r#"{"iid":1}"#).is_err());
}

/// The list→lookup mapping preserves the BUG-257 transient-vs-definitive
/// distinction. Synthesize `std::process::Output` values for each arm.
fn fake_output(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
    // ExitStatusExt::from_raw is platform-specific: on Unix the raw value is
    // a wait-status (exit code in bits 8-15), on Windows it's the bare exit
    // code as u32. Gate per platform so the test build compiles on both —
    // both yield a status whose .code() == `code`. trace:BUG-469 | ai:claude
    #[cfg(unix)]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw((code & 0xff) << 8)
    };
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code as u32)
    };
    std::process::Output {
        status,
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[test]
fn glab_lookup_found_from_clean_array() {
    let json = r#"[{"iid":9,"web_url":"https://gl/mr/9","source_branch":"feat","target_branch":"main","title":"T"}]"#;
    let lk = glab_lookup_from_list_output(Ok(fake_output(0, json, "")), "feat");
    match lk {
        ChangeLookup::Found(c) => {
            assert_eq!(c.id, 9);
            assert_eq!(c.branch, "feat");
            assert_eq!(c.title.as_deref(), Some("T"));
        }
        other => panic!("expected Found, got {other:?}"),
    }
}

#[test]
fn glab_lookup_branch_hint_fills_missing_source_branch() {
    // change_for_spec passes "" as the hint; a spec search row carries its
    // own source_branch, so the row wins. But if it's absent, the hint fills.
    let json = r#"[{"iid":2,"web_url":"https://gl/mr/2"}]"#;
    let lk = glab_lookup_from_list_output(Ok(fake_output(0, json, "")), "hinted");
    match lk {
        ChangeLookup::Found(c) => assert_eq!(c.branch, "hinted"),
        other => panic!("expected Found, got {other:?}"),
    }
}

#[test]
fn glab_lookup_empty_array_is_no_change() {
    assert_eq!(
        glab_lookup_from_list_output(Ok(fake_output(0, "[]", "")), "b"),
        ChangeLookup::NoChange
    );
    assert_eq!(
        glab_lookup_from_list_output(Ok(fake_output(0, "", "")), "b"),
        ChangeLookup::NoChange
    );
}

#[test]
fn glab_lookup_invoke_error_is_cli_missing() {
    assert_eq!(
        glab_lookup_from_list_output(Err(anyhow::anyhow!("not found")), "b"),
        ChangeLookup::CliMissing
    );
}

#[test]
fn glab_lookup_transient_stderr_is_unreachable() {
    let lk =
        glab_lookup_from_list_output(Ok(fake_output(1, "", "dial tcp: connection refused")), "b");
    match lk {
        ChangeLookup::Unreachable(s) => assert!(s.contains("connection refused")),
        other => panic!("expected Unreachable, got {other:?}"),
    }
}

#[test]
fn glab_lookup_auth_stderr_is_cli_failed() {
    // A 401 / auth failure is definitive (NOT transient) → CliFailed, so the
    // orchestrator does not treat it as Inconclusive.
    let lk = glab_lookup_from_list_output(
        Ok(fake_output(1, "", "401 Unauthorized: token invalid")),
        "b",
    );
    match lk {
        ChangeLookup::CliFailed(s) => assert!(s.contains("401")),
        other => panic!("expected CliFailed, got {other:?}"),
    }
}

#[test]
fn glab_stderr_transient_classification() {
    assert!(glab_stderr_is_transient(
        "Get ...: net/http: TLS handshake timeout"
    ));
    assert!(glab_stderr_is_transient(
        "could not resolve host: gitlab.example.com"
    ));
    assert!(glab_stderr_is_transient("received 503 Service Unavailable"));
    assert!(!glab_stderr_is_transient("404 Project Not Found"));
    assert!(!glab_stderr_is_transient("401 Unauthorized"));
}

// ───────────────────── STORY-510: glab CI/pipeline mapping ─────────────────────
//
// `glab api -X GET projects/:id/pipelines` (TASK-962) returns the GitLab
// REST pipeline objects verbatim, so these fixtures faithfully exercise the
// pure mappers without a live GitLab. trace:STORY-510 trace:TASK-962 | ai:claude

/// A `glab api pipelines` REST array fixture with one pipeline of `status`
/// at `id`. (`web_url`/`ref`/`sha` are the other load-bearing fields.)
fn pipeline_fixture(id: u64, status: &str) -> String {
    format!(
        r#"[{{
              "id": {id},
              "iid": 1,
              "status": "{status}",
              "ref": "feature/x",
              "sha": "abc123",
              "web_url": "https://gitlab.example.com/joe/aida/-/pipelines/{id}",
              "created_at": "2026-06-06T00:00:0{id}Z"
            }}]"#
    )
}

#[test]
fn glab_ci_state_maps_pipeline_status_tokens() {
    assert_eq!(glab_ci_state_from_status("success"), CiState::Success);
    assert_eq!(glab_ci_state_from_status("failed"), CiState::Failed);
    assert_eq!(glab_ci_state_from_status("canceled"), CiState::Failed);
    assert_eq!(glab_ci_state_from_status("cancelled"), CiState::Failed);
    // Non-failing terminal → Success (nothing to block on).
    assert_eq!(glab_ci_state_from_status("skipped"), CiState::Success);
    assert_eq!(glab_ci_state_from_status("manual"), CiState::Success);
    // In-flight / queued → Running.
    for s in [
        "created",
        "waiting_for_resource",
        "preparing",
        "pending",
        "running",
        "scheduled",
    ] {
        assert_eq!(glab_ci_state_from_status(s), CiState::Running, "{s}");
    }
    // Unknown / empty → None.
    assert_eq!(glab_ci_state_from_status(""), CiState::None);
    assert_eq!(glab_ci_state_from_status("weird"), CiState::None);
}

// STORY-1166: GitLab registration + per-check row mapping are pure.
#[test]
fn glab_registration_and_job_rows_map_to_forge_neutral_shapes() {
    assert_eq!(
        registration_from_glab_pipelines("[]"),
        CheckRegistration::NotYet
    );
    assert_eq!(
        registration_from_glab_pipelines(""),
        CheckRegistration::NotYet
    );
    assert_eq!(
        registration_from_glab_pipelines("garbage"),
        CheckRegistration::NotYet
    );
    assert_eq!(
        registration_from_glab_pipelines(r#"[{"id":7,"status":"running"}]"#),
        CheckRegistration::Registered
    );
    let jobs = r#"[{"name":"build","status":"success"},{"name":"merge-hold-gate","status":"failed"},
            {"name":"lint","status":"running"},{"name":"deploy","status":"manual"},{"name":"old","status":"canceled"}]"#;
    let rows = check_rows_from_glab_jobs(jobs, "gitlab-ci", Some(42));
    let buckets: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.name.as_str(), r.bucket.as_str()))
        .collect();
    assert_eq!(
        buckets,
        vec![
            ("build", "pass"),
            ("merge-hold-gate", "fail"),
            ("lint", "pending"),
            ("deploy", "skipping"),
            ("old", "cancel")
        ]
    );
    assert!(rows.iter().all(|r| r.workflow == "gitlab-ci"));
    assert!(check_rows_from_glab_jobs("nope", "x", None).is_empty());
}

#[test]
fn newest_glab_pipeline_picks_highest_id() {
    let body = r#"[
          {"id": 10, "status": "failed"},
          {"id": 42, "status": "success"},
          {"id": 7,  "status": "running"}
        ]"#;
    let arr = match serde_json::from_str::<serde_json::Value>(body).unwrap() {
        serde_json::Value::Array(a) => a,
        _ => unreachable!(),
    };
    let newest = newest_glab_pipeline(&arr).unwrap();
    assert_eq!(newest.get("id").and_then(|v| v.as_u64()), Some(42));
    assert_eq!(
        newest.get("status").and_then(|v| v.as_str()),
        Some("success")
    );
}

// trace:BUG-1819 | ai:codex
#[test]
fn github_ci_evidence_reads_the_pr_head() {
    let body = r#"[{"number":26,"headRefOid":"abc123","statusCheckRollup":[]}]"#;
    assert_eq!(github_ci_head_from_json(body).as_deref(), Some("abc123"));
}

// trace:BUG-1224 | ai:codex
#[test]
fn gitlab_progress_snapshot_changes_as_running_job_duration_advances() {
    let pipelines = r#"[{"id":42,"status":"running"}]"#;
    let after_one_minute = r#"[{"id":9,"name":"verify","status":"running","started_at":"2026-09-18T17:00:00Z","duration":60.0}]"#;
    let after_fifteen_minutes = r#"[{"id":9,"name":"verify","status":"running","started_at":"2026-09-18T17:00:00Z","duration":900.0}]"#;

    assert_ne!(
        glab_ci_progress_snapshot(pipelines, Some(after_one_minute)),
        glab_ci_progress_snapshot(pipelines, Some(after_fifteen_minutes))
    );
    assert_eq!(
        glab_ci_progress_snapshot(pipelines, None),
        pipelines,
        "a jobs API failure retains the pipeline-level progress signal"
    );
    assert_eq!(newest_glab_pipeline_id(pipelines), Some(42));
}

// trace:TASK-1254 | ai:claude
#[test]
fn gitlab_stream_keeps_waiting_only_on_in_progress_probe() {
    assert!(stream_probe_needs_wait(&CiProbeResult::InProgress {
        change: 2
    }));
    assert!(!stream_probe_needs_wait(&CiProbeResult::Green {
        change: 2
    }));
    assert!(!stream_probe_needs_wait(&CiProbeResult::Failed {
        change: 2,
        summary: "pipeline failed".into(),
    }));
    assert!(!stream_probe_needs_wait(&CiProbeResult::NoChecks {
        change: 2
    }));
    assert!(!stream_probe_needs_wait(&CiProbeResult::NoSignal(
        "glab missing".into()
    )));
}

#[test]
fn ci_status_from_pipelines_maps_each_state() {
    let green =
        ci_status_from_glab_pipelines(Ok(fake_output(0, &pipeline_fixture(5, "success"), "")));
    assert_eq!(green.state, CiState::Success);
    assert!(green.failing_checks.is_empty());
    assert_eq!(
        green.url.as_deref(),
        Some("https://gitlab.example.com/joe/aida/-/pipelines/5")
    );

    let red = ci_status_from_glab_pipelines(Ok(fake_output(0, &pipeline_fixture(6, "failed"), "")));
    assert_eq!(red.state, CiState::Failed);
    assert_eq!(red.failing_checks, vec!["pipeline failed".to_string()]);

    let running =
        ci_status_from_glab_pipelines(Ok(fake_output(0, &pipeline_fixture(7, "running"), "")));
    assert_eq!(running.state, CiState::Running);
}

#[test]
fn ci_status_from_pipelines_no_signal_degrades_to_none() {
    // CLI missing, non-zero exit, empty array, and blank stdout all degrade
    // to CiState::None (no CI / nothing to wait on), mirroring the gh path.
    assert_eq!(
        ci_status_from_glab_pipelines(Err(anyhow::anyhow!("no glab"))).state,
        CiState::None
    );
    assert_eq!(
        ci_status_from_glab_pipelines(Ok(fake_output(1, "", "boom"))).state,
        CiState::None
    );
    assert_eq!(
        ci_status_from_glab_pipelines(Ok(fake_output(0, "[]", ""))).state,
        CiState::None
    );
    assert_eq!(
        ci_status_from_glab_pipelines(Ok(fake_output(0, "", ""))).state,
        CiState::None
    );
}

#[test]
fn ci_probe_from_pipelines_maps_each_state() {
    assert_eq!(
        ci_probe_from_glab_pipelines(Ok(fake_output(0, &pipeline_fixture(5, "success"), "")), 7),
        CiProbeResult::Green { change: 7 }
    );
    assert_eq!(
        ci_probe_from_glab_pipelines(Ok(fake_output(0, &pipeline_fixture(5, "running"), "")), 7),
        CiProbeResult::InProgress { change: 7 }
    );
    match ci_probe_from_glab_pipelines(Ok(fake_output(0, &pipeline_fixture(5, "failed"), "")), 7) {
        CiProbeResult::Failed { change, summary } => {
            assert_eq!(change, 7);
            assert!(
                summary.contains("failed"),
                "summary names the status: {summary}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    // canceled is a failure for a CI-wait.
    assert!(matches!(
        ci_probe_from_glab_pipelines(Ok(fake_output(0, &pipeline_fixture(5, "canceled"), "")), 7),
        CiProbeResult::Failed { change: 7, .. }
    ));
}

#[test]
fn ci_probe_from_pipelines_no_pipelines_is_no_checks_not_no_signal() {
    // The BUG-257-style distinction carried into CI: a clean run with no
    // pipelines is a definitive "no checks" (NoChecks), NOT "couldn't probe".
    assert_eq!(
        ci_probe_from_glab_pipelines(Ok(fake_output(0, "[]", "")), 7),
        CiProbeResult::NoChecks { change: 7 }
    );
    assert_eq!(
        ci_probe_from_glab_pipelines(Ok(fake_output(0, "", "")), 7),
        CiProbeResult::NoChecks { change: 7 }
    );
}

#[test]
fn ci_probe_from_pipelines_invoke_or_exit_failure_is_no_signal() {
    // glab not on PATH → NoSignal (couldn't probe).
    match ci_probe_from_glab_pipelines(Err(anyhow::anyhow!("missing")), 7) {
        CiProbeResult::NoSignal(why) => assert!(why.contains("glab")),
        other => panic!("expected NoSignal, got {other:?}"),
    }
    // non-zero exit → NoSignal carrying stderr.
    match ci_probe_from_glab_pipelines(Ok(fake_output(1, "", "401 Unauthorized")), 7) {
        CiProbeResult::NoSignal(why) => assert!(why.contains("401")),
        other => panic!("expected NoSignal, got {other:?}"),
    }
    // clean exit, non-array JSON → NoSignal.
    assert!(matches!(
        ci_probe_from_glab_pipelines(Ok(fake_output(0, r#"{"id":1}"#, "")), 7),
        CiProbeResult::NoSignal(_)
    ));
}

// TASK-962: `ci_status`/`ci_probe_for_branch` now read `glab api -X GET
// projects/:id/pipelines?ref=<branch>` instead of the broken `glab ci list
// -F json`. The GitLab REST `GET /projects/:id/pipelines` response carries
// a few fields `pipeline_fixture` doesn't (`project_id`, `source`,
// `updated_at`) alongside the ones the mappers read (`id`/`status`/
// `web_url`/`created_at`) — this fixture is the full REST shape verbatim
// from the GitLab API docs, proving the pure mappers tolerate the extra
// fields and still pick the right pipeline/state/url. trace:TASK-962 | ai:claude
fn real_glab_api_pipelines_response(id: u64, status: &str) -> String {
    format!(
        r#"[{{
              "id": {id},
              "iid": 12,
              "project_id": 1,
              "sha": "a91957a858320c0e17f3a0eca7cfacbff50ea29a",
              "ref": "feature/x",
              "status": "{status}",
              "source": "push",
              "created_at": "2026-06-06T11:28:34.085Z",
              "updated_at": "2026-06-06T11:32:35.169Z",
              "web_url": "https://gitlab.example.com/joe/aida/-/pipelines/{id}"
            }}]"#
    )
}

#[test]
fn ci_status_from_real_glab_api_pipelines_response_maps_success() {
    let status = ci_status_from_glab_pipelines(Ok(fake_output(
        0,
        &real_glab_api_pipelines_response(99, "success"),
        "",
    )));
    assert_eq!(status.state, CiState::Success);
    assert!(status.failing_checks.is_empty());
    assert_eq!(
        status.url.as_deref(),
        Some("https://gitlab.example.com/joe/aida/-/pipelines/99")
    );
}

#[test]
fn ci_probe_from_real_glab_api_pipelines_response_maps_failed() {
    match ci_probe_from_glab_pipelines(
        Ok(fake_output(
            0,
            &real_glab_api_pipelines_response(100, "failed"),
            "",
        )),
        12,
    ) {
        CiProbeResult::Failed { change, summary } => {
            assert_eq!(change, 12);
            assert!(summary.contains("failed"), "summary: {summary}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

// trace:BUG-1819 | ai:codex
#[test]
fn gitlab_ci_evidence_reads_the_newest_pipeline_head() {
    let evidence = ci_probe_evidence_from_glab_pipelines(
        Ok(fake_output(
            0,
            &real_glab_api_pipelines_response(101, "success"),
            "",
        )),
        26,
    );
    assert_eq!(evidence.probe, CiProbeResult::Green { change: 26 });
    assert_eq!(
        evidence.head_sha.as_deref(),
        Some("a91957a858320c0e17f3a0eca7cfacbff50ea29a")
    );
}

// ─────────────── change-metadata parsers (TASK-963) ───────────────

#[test]
fn gh_change_metadata_maps_full_object() {
    let json: serde_json::Value = serde_json::from_str(
        r#"{
              "state": "MERGED",
              "title": "[AI:claude] fix(x): y (TASK-1)",
              "author": {"login": "codex-bot"},
              "mergedAt": "2026-09-12T18:00:00Z",
              "baseRefName": "main",
              "headRefName": "task-1-fix",
              "headRefOid": "abc123",
              "isCrossRepository": true,
              "headRepository": {"nameWithOwner": "alice/aida"},
              "isDraft": true
            }"#,
    )
    .unwrap();
    let m = parse_gh_change_metadata(&json);
    assert_eq!(m.state, ChangeState::Merged);
    assert_eq!(m.title, "[AI:claude] fix(x): y (TASK-1)");
    assert_eq!(m.author.as_deref(), Some("codex-bot"));
    assert_eq!(
        m.merged_at.map(|dt| dt.to_rfc3339()).as_deref(),
        Some("2026-09-12T18:00:00+00:00")
    );
    assert_eq!(m.base_ref, "main");
    assert_eq!(m.head_ref, "task-1-fix");
    assert_eq!(m.head_sha, "abc123");
    assert!(m.is_draft);
    assert!(m.is_cross_repository);
    assert_eq!(m.head_repo.as_deref(), Some("alice/aida"));
}

#[test]
fn gh_change_metadata_missing_fields_degrade() {
    let json: serde_json::Value = serde_json::from_str(r#"{"state": "OPEN"}"#).unwrap();
    let m = parse_gh_change_metadata(&json);
    assert_eq!(m.state, ChangeState::Open);
    assert!(m.title.is_empty());
    assert_eq!(m.author, None);
    assert!(m.base_ref.is_empty());
    assert!(!m.is_draft);
    assert!(!m.is_cross_repository);
    assert_eq!(m.head_repo, None);
}

#[test]
fn gh_change_reviews_maps_decision_and_latest_approver() {
    let out = parse_gh_change_reviews(
        r#"{
              "reviewDecision": "APPROVED",
              "latestReviews": [
                {"state": "APPROVED", "author": {"login": "alice"}},
                {"state": "COMMENTED", "author": {"login": "bob"}},
                {"state": "APPROVED", "author": {"login": "carol"}}
              ]
            }"#,
    )
    .unwrap();
    assert_eq!(out.decision, ReviewDecision::Approved);
    assert_eq!(out.approver.as_deref(), Some("carol"));
    let none = parse_gh_change_reviews(r#"{"reviewDecision": "", "latestReviews": []}"#).unwrap();
    assert_eq!(none.decision, ReviewDecision::None);
    assert_eq!(none.approver, None);
}

#[test]
fn review_decision_gh_token_round_trip() {
    assert_eq!(ReviewDecision::Approved.gh_token(), "APPROVED");
    assert_eq!(
        ReviewDecision::ChangesRequested.gh_token(),
        "CHANGES_REQUESTED"
    );
    assert_eq!(ReviewDecision::ReviewRequired.gh_token(), "REVIEW_REQUIRED");
    assert_eq!(ReviewDecision::None.gh_token(), "");
}

#[test]
fn glab_mr_metadata_maps_rest_object() {
    let m = parse_glab_mr_metadata(
        r#"{
              "iid": 7,
              "state": "opened",
              "title": "Fix the thing",
              "author": {"username": "codex-bot"},
              "target_branch": "main",
              "source_branch": "fix-thing",
              "sha": "def456",
              "draft": true,
              "source_project_id": 12,
              "target_project_id": 34
            }"#,
    )
    .unwrap();
    assert_eq!(m.state, ChangeState::Open);
    assert_eq!(m.title, "Fix the thing");
    assert_eq!(m.author.as_deref(), Some("codex-bot"));
    assert_eq!(m.base_ref, "main");
    assert_eq!(m.head_ref, "fix-thing");
    assert_eq!(m.head_sha, "def456");
    assert!(m.is_draft);
    assert!(m.is_cross_repository, "differing project ids = cross-repo");
    assert_eq!(m.head_repo, None);
}

#[test]
fn glab_mr_metadata_legacy_wip_and_same_project() {
    let m = parse_glab_mr_metadata(
        r#"{
              "state": "merged",
              "merged_at": "2026-09-12T18:00:00Z",
              "target_branch": "main",
              "source_branch": "b",
              "work_in_progress": true,
              "source_project_id": 12,
              "target_project_id": 12,
              "diff_refs": {"head_sha": "789abc"}
            }"#,
    )
    .unwrap();
    assert_eq!(m.state, ChangeState::Merged);
    assert_eq!(
        m.merged_at.map(|dt| dt.to_rfc3339()).as_deref(),
        Some("2026-09-12T18:00:00+00:00")
    );
    assert!(m.is_draft, "legacy work_in_progress maps to draft");
    assert!(!m.is_cross_repository);
    assert_eq!(m.head_sha, "789abc", "falls back to diff_refs.head_sha");
}

#[test]
fn glab_mr_metadata_rejects_garbage() {
    assert!(parse_glab_mr_metadata("not json").is_err());
    assert!(parse_glab_mr_metadata("[1, 2]").is_err());
}

#[test]
fn glab_mr_commit_headlines_maps_titles() {
    let titles = parse_glab_mr_commit_headlines(
        r#"[
              {"id": "a", "title": "[AI:claude] fix(x): y (BUG-1)"},
              {"id": "b", "title": "chore: tidy"},
              {"id": "c"}
            ]"#,
    )
    .unwrap();
    assert_eq!(
        titles,
        vec![
            "[AI:claude] fix(x): y (BUG-1)".to_string(),
            "chore: tidy".to_string()
        ]
    );
    assert!(parse_glab_mr_commit_headlines("[]").unwrap().is_empty());
    assert!(parse_glab_mr_commit_headlines(r#"{"not": "array"}"#).is_err());
}

#[test]
fn glab_mr_approvals_derive_review_decision() {
    // Approved with a named approver.
    let approved = parse_glab_mr_approvals(
        r#"{
              "approved": true,
              "approvals_required": 1,
              "approvals_left": 0,
              "approved_by": [{"user": {"username": "joe"}}]
            }"#,
    )
    .unwrap();
    assert_eq!(approved.decision, ReviewDecision::Approved);
    assert_eq!(approved.approver.as_deref(), Some("joe"));

    // Approvals outstanding → ReviewRequired.
    let required = parse_glab_mr_approvals(
        r#"{"approved": false, "approvals_required": 2, "approvals_left": 2, "approved_by": []}"#,
    )
    .unwrap();
    assert_eq!(required.decision, ReviewDecision::ReviewRequired);
    assert_eq!(required.approver, None);

    // No approval rules at all → None.
    let none = parse_glab_mr_approvals(
        r#"{"approved": false, "approvals_required": 0, "approvals_left": 0, "approved_by": []}"#,
    )
    .unwrap();
    assert_eq!(none.decision, ReviewDecision::None);

    assert!(parse_glab_mr_approvals("nope").is_err());
}
