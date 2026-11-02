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

struct Fixture {
    _dir: tempfile::TempDir,
    repo: std::path::PathBuf,
    log: std::path::PathBuf,
    _gh: crate::test_env::EnvVarGuard,
    _aida: crate::kernel::test_aida_exe::Guard,
}

impl Fixture {
    /// A real `main..feat` range delivering TASK-77, a fake GitHub forge for
    /// PR-42, an empty home-fixture store, and a recording fake `aida` that
    /// prints `add_out` on `add` and exits `queue_rc` on `queue add`.
    fn new(add_out: &str, queue_rc: u8) -> Self {
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
        git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widget.git",
        ]);
        let store = repo.join(".aida-store");
        std::fs::create_dir_all(repo.join(".aida")).unwrap();
        std::fs::write(
            repo.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        crate::test_env::home_fixture_backend(&store)
            .unwrap()
            .save(&RequirementsStore::default())
            .unwrap();

        let gh = dir.path().join("gh");
        crate::test_exec::write_executable(
            &gh,
            "#!/bin/sh\n\
             case \"$1 $2\" in\n\
             'pr list') printf '42\\tthing\\thttps://github.com/acme/widget/pull/42\\tfeat\\n' ;;\n\
             'pr view') printf '{\"state\":\"OPEN\",\"title\":\"thing\",\"baseRefName\":\"main\",\"headRefName\":\"feat\"}\\n' ;;\n\
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
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n\
                 if [ \"$1\" = add ]; then cat '{}'; fi\n\
                 if [ \"$1 $2\" = 'queue add' ]; then exit {queue_rc}; fi\n",
                log.display(),
                add_out_path.display()
            ),
        );
        let _gh = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", &gh);
        let _aida = crate::kernel::test_aida_exe::set(aida);
        Fixture {
            _dir: dir,
            repo,
            log,
            _gh,
            _aida,
        }
    }

    fn run(&self) -> (AutoQueueOutcome, String) {
        let outcome = try_auto_queue_pr_review_unbumped(
            &self.repo,
            "feat",
            "session-bug-2404",
            AutoQueueOrigin::SessionEnd,
        );
        let calls = std::fs::read_to_string(&self.log).unwrap_or_default();
        (outcome, calls)
    }
}

fn adds(calls: &str) -> usize {
    calls.lines().filter(|l| l.starts_with("add ")).count()
}

#[test]
fn session_end_files_links_and_queues_the_exact_stored_id() {
    let fx = Fixture::new(
        &format!("Added: STORY-2272 ({LOCAL}) - Review PR-42: thing\nHint: x\n"),
        0,
    );
    let (outcome, calls) = fx.run();
    assert!(
        matches!(outcome.status, AutoQueueStatus::Filed),
        "{}\n{calls}",
        outcome.summary
    );
    assert_eq!(outcome.review_spec.as_deref(), Some(LOCAL), "{calls}");
    assert_eq!(adds(&calls), 1, "{calls}");
    assert!(
        calls
            .lines()
            .any(|l| l.starts_with("add ") && l.ends_with("--canonical-ids")),
        "child add must print canonical IDs:\n{calls}"
    );
    assert!(
        calls
            .lines()
            .any(|l| l == format!("rel add {LOCAL} TASK-77 --type implements")),
        "{calls}"
    );
    assert!(
        calls
            .lines()
            .any(|l| l.starts_with(&format!("queue add {LOCAL} --for reviewer"))),
        "{calls}"
    );
    assert!(!calls.contains("STORY-2272"), "{calls}");
}

#[test]
fn queue_failure_is_a_durable_retry_not_a_claimed_queue_or_second_story() {
    let fx = Fixture::new(&format!("Added: STORY-2272 ({LOCAL}) - r\n"), 1);
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
    assert_eq!(adds(&calls), 1, "{calls}");
}

#[test]
fn unreadable_add_output_refuses_without_linking_or_queueing_a_guess() {
    for out in [
        "Added: STORY-2272 STORY-HOME-288 - r\n",
        "Added: STORY-1 - a\nAdded: STORY-2 - b\n",
        "done\n",
    ] {
        let fx = Fixture::new(out, 0);
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
        assert_eq!(adds(&calls), 1, "{calls}");
        assert!(
            !calls.contains("rel add") && !calls.contains("queue add"),
            "{out:?}:\n{calls}"
        );
    }
}
