//! BUG-2404: session-end / `aida pr` auto-queue read the new review story's
//! ID out of `aida add` output. The agreed-plus-local display
//! (`Added: STORY-2272 (STORY-HOME-288) - …`) was passed whole to `rel add`
//! and `queue add`, so the story was filed but never linked or queued. The
//! parser now takes exactly one stored ID or refuses.
// trace:BUG-2404 | ai:claude

use super::*;
use crate::pr_lookup::parse_spec_id_from_add_output as parse;
use aida_core::RequirementsStore;

const LOCAL: &str = "STORY-B6B1278890774EF993D2EBC327862470-288";

#[test]
fn agreed_plus_local_display_yields_the_stored_local_id() {
    assert_eq!(
        parse(&format!(
            "Added: STORY-2272 ({LOCAL}) - Review PR-3207: fix\n"
        )),
        Some(LOCAL.to_string())
    );
    // ANSI around either part, and a title holding ` - ` and ID-like text.
    assert_eq!(
        parse(&format!(
            "Added: \x1b[1mSTORY-2272\x1b[0m (\x1b[2m{LOCAL}\x1b[0m) - Review PR-9: TASK-1 - BUG-2 (STORY-3)\n"
        )),
        Some(LOCAL.to_string())
    );
}

#[test]
fn supported_single_id_forms_still_parse() {
    // No alias: agreed or canonical node-local only.
    assert_eq!(
        parse("Added: STORY-82 - title\nHint: link via STORY-1\n"),
        Some("STORY-82".into())
    );
    assert_eq!(
        parse(&format!("Added: {LOCAL} - title\n")),
        Some(LOCAL.into())
    );
    // Legacy backend, with a UUID line that must never be read as identity.
    assert_eq!(
        parse("Requirement added successfully!\nUUID: 019e1300-0000-7000-8000-000000000288\nID: \x1b[32mSTORY-77\x1b[0m\n"),
        Some("STORY-77".into())
    );
    // Sanitized intake prints the ID with no title.
    assert_eq!(parse("Added: TASK-5\n"), Some("TASK-5".into()));
    // Both shapes naming the same ID agree.
    assert_eq!(
        parse("Added: STORY-9 - t\nID: STORY-9\n"),
        Some("STORY-9".into())
    );
}

#[test]
fn malformed_ambiguous_or_missing_output_is_refused() {
    for out in [
        "",
        "something unrelated\nwarning: STORY-1 is stale\n",
        // A whole decorated string with no ` (` separator is not one ID.
        "Added: STORY-2272 STORY-HOME-288 - t\n",
        // Unclosed / multi-token alias.
        "Added: STORY-2272 (STORY-HOME-288 - t\n",
        "Added: STORY-2272 (STORY-1 STORY-2) - t\n",
        // A title, not an ID.
        "Added: Review the thing - t\n",
        "Added:  - t\n",
        "ID: not an id\n",
        // Two different IDs.
        "Added: STORY-1 - a\nAdded: STORY-2 - b\n",
        "Added: STORY-1 - a\nID: STORY-2\n",
    ] {
        assert_eq!(parse(out), None, "{out:?}");
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Forge {
    GitHub,
    GitLab,
}

struct Opts<'a> {
    forge: Forge,
    add_out: &'a str,
    queue_rc: u8,
    rel_rc: u8,
    /// Seed the store with an already-filed `Review <PR|MR>-42` story whose
    /// stored ID is [`LOCAL`] (an interrupted earlier run).
    existing_story: bool,
}

impl<'a> Opts<'a> {
    fn add(add_out: &'a str) -> Self {
        Opts {
            forge: Forge::GitHub,
            add_out,
            queue_rc: 0,
            rel_rc: 0,
            existing_story: false,
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    repo: std::path::PathBuf,
    log: std::path::PathBuf,
    _forge: crate::test_env::EnvVarsGuard,
    _aida: crate::kernel::test_aida_exe::Guard,
}

impl Fixture {
    /// A real `main..feat` range delivering TASK-77, a fake GitHub or GitLab
    /// forge for change 42, a home-fixture store, and a recording fake `aida`
    /// that prints `add_out` on `add` and exits `rel_rc` / `queue_rc` on
    /// `rel add` / `queue add`. The log keeps every argument intact: args end
    /// in US (0x1f), invocations in RS (0x1e), so a multi-line `--description`
    /// cannot split one call into several records.
    fn new(opts: Opts<'_>) -> Self {
        let Opts {
            forge,
            add_out,
            queue_rc,
            rel_rc,
            existing_story,
        } = opts;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let out = crate::test_git::git_command()
                .current_dir(&repo)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "base"]);
        git(&["checkout", "-q", "-b", "feat"]);
        git(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "[AI:claude] fix(x): thing (TASK-77)",
        ]);
        let (origin_url, noun) = match forge {
            Forge::GitHub => ("https://github.com/acme/widget.git", "PR"),
            Forge::GitLab => ("https://gitlab.com/acme/widget.git", "MR"),
        };
        git(&["remote", "add", "origin", origin_url]);
        let store_path = repo.join(".aida-store");
        std::fs::create_dir_all(repo.join(".aida")).unwrap();
        std::fs::write(
            repo.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        let mut store = RequirementsStore::default();
        if existing_story {
            let mut story =
                aida_core::Requirement::new(format!("Review {noun}-42: thing"), String::new());
            story.spec_id = Some(LOCAL.into());
            story.status = aida_core::RequirementStatus::Approved;
            store.requirements.push(story);
        }
        crate::test_env::home_fixture_backend(&store_path)
            .unwrap()
            .save(&store)
            .unwrap();

        // GitHub answers `pr list` / `pr view`; on GitLab `gh` must never be
        // the forge that resolves the change, so it only fails.
        let gh = dir.path().join("gh");
        crate::test_exec::write_executable(
            &gh,
            match forge {
                Forge::GitHub => "#!/bin/sh\n\
                     case \"$1 $2\" in\n\
                     'pr list') printf '42\\tthing\\thttps://github.com/acme/widget/pull/42\\tfeat\\n' ;;\n\
                     'pr view') printf '{\"state\":\"OPEN\",\"title\":\"thing\",\"baseRefName\":\"main\",\"headRefName\":\"feat\"}\\n' ;;\n\
                     *) exit 1 ;;\n\
                     esac\n",
                Forge::GitLab => "#!/bin/sh\nexit 1\n",
            },
        );
        // `glab api -X GET <path> …`: the list query returns an array, the
        // single-MR read an object.
        let glab = dir.path().join("glab");
        crate::test_exec::write_executable(
            &glab,
            "#!/bin/sh\n\
             mr='{\"iid\":42,\"web_url\":\"https://gitlab.com/acme/widget/-/merge_requests/42\",\"source_branch\":\"feat\",\"target_branch\":\"main\",\"title\":\"thing\",\"state\":\"opened\",\"sha\":\"deadbeef\"}'\n\
             case \"$4\" in\n\
             'projects/:id/merge_requests') printf '[%s]\\n' \"$mr\" ;;\n\
             'projects/:id/merge_requests/42') printf '%s\\n' \"$mr\" ;;\n\
             *) exit 1 ;;\n\
             esac\n",
        );
        let log = dir.path().join("aida.log");
        let add_out_path = dir.path().join("add.out");
        std::fs::write(&add_out_path, add_out).unwrap();
        let aida = dir.path().join("aida");
        crate::test_exec::write_executable(
            &aida,
            format!(
                "#!/bin/sh\n\
                 for a in \"$@\"; do printf '%s\\037' \"$a\"; done >> '{log}'\n\
                 printf '\\036' >> '{log}'\n\
                 case \"$1 $2\" in\n\
                 'rel add') exit {rel_rc} ;;\n\
                 'queue add') exit {queue_rc} ;;\n\
                 esac\n\
                 if [ \"$1\" = add ]; then cat '{out}'; fi\n",
                log = log.display(),
                out = add_out_path.display()
            ),
        );
        let _forge = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
            ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ]);
        let _aida = crate::kernel::test_aida_exe::set(aida);
        Fixture {
            _dir: dir,
            repo,
            log,
            _forge,
            _aida,
        }
    }

    /// Run session-end auto-queue; return the outcome and every recorded
    /// `aida` invocation as its exact argv.
    fn run(&self) -> (AutoQueueOutcome, Calls) {
        let outcome = try_auto_queue_pr_review_unbumped(
            &self.repo,
            "feat",
            "session-bug-2404",
            AutoQueueOrigin::SessionEnd,
        );
        let raw = std::fs::read_to_string(&self.log).unwrap_or_default();
        let calls = raw
            .split('\x1e')
            .filter(|rec| !rec.is_empty())
            .map(|rec| {
                rec.strip_suffix('\x1f')
                    .unwrap_or(rec)
                    .split('\x1f')
                    .map(str::to_string)
                    .collect()
            })
            .collect();
        (outcome, Calls(calls))
    }
}

#[derive(Debug)]
struct Calls(Vec<Vec<String>>);

impl Calls {
    fn with_prefix(&self, prefix: &[&str]) -> Vec<&Vec<String>> {
        self.0
            .iter()
            .filter(|argv| {
                argv.len() >= prefix.len() && argv.iter().zip(prefix).all(|(a, p)| a == p)
            })
            .collect()
    }
    fn adds(&self) -> Vec<&Vec<String>> {
        self.with_prefix(&["add"])
    }
    fn has(&self, prefix: &[&str]) -> bool {
        !self.with_prefix(prefix).is_empty()
    }
    fn mentions(&self, needle: &str) -> bool {
        self.0.iter().flatten().any(|a| a.contains(needle))
    }
}

/// The full successful handoff for `id`: one child add printing canonical
/// IDs, the `implements` link to TASK-77 (both directions), and the reviewer
/// queue insertion of exactly `id`.
fn assert_filed_linked_queued(outcome: &AutoQueueOutcome, calls: &Calls, id: &str, title: &str) {
    assert!(
        matches!(outcome.status, AutoQueueStatus::Filed),
        "{}\n{calls:?}",
        outcome.summary
    );
    assert_eq!(outcome.review_spec.as_deref(), Some(id), "{calls:?}");
    let adds = calls.adds();
    assert_eq!(adds.len(), 1, "{calls:?}");
    assert_eq!(
        adds[0].last().map(String::as_str),
        Some("--canonical-ids"),
        "child add must print canonical IDs: {calls:?}"
    );
    assert!(adds[0].iter().any(|a| a == title), "{calls:?}");
    assert!(
        calls.has(&["rel", "add", id, "TASK-77", "--type", "implements"]),
        "{calls:?}"
    );
    assert!(
        calls.has(&["rel", "add", "TASK-77", id, "--type", "implemented-by"]),
        "{calls:?}"
    );
    assert!(
        calls.has(&["queue", "add", id, "--for", "reviewer"]),
        "{calls:?}"
    );
}

#[test]
fn session_end_files_links_and_queues_the_exact_stored_id() {
    let fx = Fixture::new(Opts::add(&format!(
        "Added: STORY-2272 ({LOCAL}) - Review PR-42: thing\nHint: x\n"
    )));
    let (outcome, calls) = fx.run();
    assert_filed_linked_queued(&outcome, &calls, LOCAL, "Review PR-42: thing");
    assert!(!calls.mentions("STORY-2272"), "{calls:?}");
}

#[test]
fn session_end_links_and_queues_every_supported_add_output_form() {
    for (out, id) in [
        // No alias: the agreed ID alone.
        ("Added: STORY-82 - Review PR-42: thing\n".to_string(), "STORY-82"),
        // Canonical node-local ID alone.
        (format!("Added: {LOCAL} - Review PR-42: thing\n"), LOCAL),
        // Legacy backend `ID:` line, colored, with a UUID that is not identity.
        (
            "Requirement added successfully!\nUUID: 019e1300-0000-7000-8000-000000000288\nID: \x1b[32mSTORY-77\x1b[0m\n"
                .to_string(),
            "STORY-77",
        ),
        // ANSI around both parts of the agreed+local display, ID-like title.
        (
            format!("Added: \x1b[1mSTORY-2272\x1b[0m (\x1b[2m{LOCAL}\x1b[0m) - Review PR-42: TASK-1 (STORY-3)\n"),
            LOCAL,
        ),
    ] {
        let fx = Fixture::new(Opts::add(&out));
        let (outcome, calls) = fx.run();
        assert_filed_linked_queued(&outcome, &calls, id, "Review PR-42: thing");
    }
}

#[test]
fn gitlab_mr_files_a_review_mr_story_and_queues_the_exact_stored_id() {
    let fx = Fixture::new(Opts {
        forge: Forge::GitLab,
        ..Opts::add(&format!(
            "Added: STORY-2272 ({LOCAL}) - Review MR-42: thing\n"
        ))
    });
    let (outcome, calls) = fx.run();
    assert_filed_linked_queued(&outcome, &calls, LOCAL, "Review MR-42: thing");
    assert!(outcome.summary.contains("MR-42"), "{}", outcome.summary);
}

#[test]
fn existing_review_story_retry_reuses_its_exact_id_without_a_second_add() {
    for forge in [Forge::GitHub, Forge::GitLab] {
        let fx = Fixture::new(Opts {
            forge,
            existing_story: true,
            // Would surface as a wrong ID if the retry ever re-filed.
            ..Opts::add("Added: STORY-999 - duplicate\n")
        });
        let (outcome, calls) = fx.run();
        assert!(
            matches!(outcome.status, AutoQueueStatus::AlreadyExists),
            "{}\n{calls:?}",
            outcome.summary
        );
        assert_eq!(outcome.review_spec.as_deref(), Some(LOCAL), "{calls:?}");
        assert!(calls.adds().is_empty(), "no second story: {calls:?}");
        // The interrupted run's missing link is repaired, then queued.
        assert!(
            calls.has(&["rel", "add", LOCAL, "TASK-77", "--type", "implements"]),
            "{calls:?}"
        );
        assert!(
            calls.has(&["queue", "add", LOCAL, "--for", "reviewer"]),
            "{calls:?}"
        );
        assert!(!calls.mentions("STORY-999"), "{calls:?}");
    }
}

#[test]
fn queue_failure_is_a_durable_retry_not_a_claimed_queue_or_second_story() {
    let fx = Fixture::new(Opts {
        queue_rc: 1,
        ..Opts::add(&format!("Added: STORY-2272 ({LOCAL}) - r\n"))
    });
    let (outcome, calls) = fx.run();
    assert!(
        matches!(outcome.status, AutoQueueStatus::SkippedNeedsAttention),
        "{}",
        outcome.summary
    );
    assert_eq!(outcome.review_spec.as_deref(), Some(LOCAL));
    assert!(
        outcome.summary.contains("durable retry"),
        "{}",
        outcome.summary
    );
    assert_eq!(calls.adds().len(), 1, "{calls:?}");
}

#[test]
fn link_failure_is_a_durable_retry_and_never_claims_the_queue() {
    let fx = Fixture::new(Opts {
        rel_rc: 1,
        ..Opts::add(&format!("Added: STORY-2272 ({LOCAL}) - r\n"))
    });
    let (outcome, calls) = fx.run();
    assert!(
        matches!(outcome.status, AutoQueueStatus::SkippedNeedsAttention),
        "{}",
        outcome.summary
    );
    assert_eq!(outcome.review_spec.as_deref(), Some(LOCAL));
    assert!(
        outcome.summary.contains("linking it failed") && outcome.summary.contains("durable retry"),
        "{}",
        outcome.summary
    );
    assert_eq!(calls.adds().len(), 1, "{calls:?}");
    assert!(calls.has(&["rel", "add", LOCAL]), "{calls:?}");
    assert!(!calls.has(&["queue", "add"]), "{calls:?}");
}

#[test]
fn unreadable_add_output_refuses_without_linking_or_queueing_a_guess() {
    for out in [
        "Added: STORY-2272 STORY-HOME-288 - r\n",
        "Added: \x1b[1mSTORY-2272 STORY-HOME-288\x1b[0m - r\n",
        "Added: STORY-1 - a\nAdded: STORY-2 - b\n",
        "Added: STORY-1 - a\nID: STORY-2\n",
        "Added: Review the thing - r\n",
        "done\n",
    ] {
        let fx = Fixture::new(Opts::add(out));
        let (outcome, calls) = fx.run();
        assert!(
            matches!(outcome.status, AutoQueueStatus::SkippedNeedsAttention),
            "{out:?}: {}",
            outcome.summary
        );
        assert!(outcome.review_spec.is_none(), "{out:?}");
        assert!(
            outcome.summary.contains("refusing to guess"),
            "{}",
            outcome.summary
        );
        assert_eq!(calls.adds().len(), 1, "{calls:?}");
        assert!(
            !calls.has(&["rel", "add"]) && !calls.has(&["queue", "add"]),
            "{out:?}: {calls:?}"
        );
    }
}
