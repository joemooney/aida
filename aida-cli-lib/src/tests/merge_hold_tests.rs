//! Tests of the merge-hold plumbing: marker writes, forge labels and the
//! pinned-change reads (STORY-1889, moved from `merge_hold.rs`).
// trace:STORY-1889 | ai:claude
use super::*;

#[cfg(unix)]
fn shell_args(script: String) -> Vec<String> {
    vec!["-c".into(), script]
}

#[cfg(unix)]
fn kill_if_alive(pid: libc::pid_t) {
    // SAFETY: signal 0 only probes the child pid; SIGKILL is limited to
    // the pid started by this test.
    if unsafe { libc::kill(pid, 0) } == 0 {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

#[cfg(unix)]
struct ChildPidGuard(libc::pid_t);

#[cfg(unix)]
impl Drop for ChildPidGuard {
    fn drop(&mut self) {
        kill_if_alive(self.0);
    }
}

// STORY-1165: the label mirror must route to the right forge CLI — gh for
// GitHub, glab for GitLab (mr update --label/--unlabel), nothing for pure-git.
fn pin(kind: crate::forge::ForgeKind, origin: &str) -> PinnedRepo {
    PinnedRepo::from_origin(kind, origin).expect("origin must pin")
}

fn recused_record() -> MergeHoldRecord {
    let mut record = typed_hold(
        42,
        HoldReasonKind::Recusal,
        "author recused",
        Some("head-a".into()),
    );
    record.recused_principals = vec![PrincipalIdentity::parse("agent:author")];
    record
}

// The definition probe intentionally has a distinct bounded runner from
// hold placement; timeout coverage keeps the doctor call from hanging.
// trace:BUG-1747 | ai:codex
#[cfg(unix)]
#[test]
fn forge_definition_probe_times_out_near_its_injected_bound() {
    let dir = tempfile::tempdir().unwrap();
    let timeout = std::time::Duration::from_millis(200);
    let started = std::time::Instant::now();
    let result = run_forge_cli_stdout_bounded_with(
        dir.path(),
        "sh",
        &shell_args("sleep 30".into()),
        timeout,
    );
    let elapsed = started.elapsed();
    assert!(result.is_err(), "expected timeout, got {result:?}");
    assert_eq!(
        result.unwrap_err(),
        format!("timed out after {}s", timeout.as_secs())
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "call took {elapsed:?}"
    );
}

// trace:BUG-1747 | ai:codex
#[cfg(unix)]
#[test]
fn forge_definition_probe_timeout_kills_the_whole_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("grandchild.pid");
    let script = format!("sleep 30 & echo $! > '{}' ; wait", pidfile.display());
    let timeout = std::time::Duration::from_millis(200);
    let result = run_forge_cli_stdout_bounded_with(dir.path(), "sh", &shell_args(script), timeout);
    assert!(result.is_err(), "expected timeout, got {result:?}");
    let pid: libc::pid_t = std::fs::read_to_string(pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _child_guard = ChildPidGuard(pid);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        // SAFETY: signal 0 only probes whether the test grandchild exists.
        if unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "grandchild {pid} survived timeout"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

// trace:BUG-1747 | ai:codex
#[cfg(unix)]
#[test]
fn forge_definition_probe_returns_stdout_without_waiting_for_bound() {
    let dir = tempfile::tempdir().unwrap();
    let timeout = std::time::Duration::from_secs(5);
    let started = std::time::Instant::now();
    let result = run_forge_cli_stdout_bounded_with(
        dir.path(),
        "sh",
        &shell_args("printf hello".into()),
        timeout,
    );
    let elapsed = started.elapsed();
    assert_eq!(result.unwrap(), (true, "hello".into()));
    assert!(elapsed < timeout / 2, "call took {elapsed:?}");
}

// Unlike run_forge_cli_stdout (used for hold placement), this diagnostic
// probe returns stderr on failure so LabelDefinitions::Unknown keeps gh's
// actionable error text instead of becoming Unknown("").
// trace:BUG-1747 | ai:codex
#[cfg(unix)]
#[test]
fn forge_definition_probe_returns_stderr_for_failing_child() {
    let dir = tempfile::tempdir().unwrap();
    let result = run_forge_cli_stdout_bounded_with(
        dir.path(),
        "sh",
        &shell_args("echo OUT; echo ERR >&2; exit 3".into()),
        std::time::Duration::from_secs(5),
    )
    .unwrap();
    assert!(!result.0);
    assert!(result.1.contains("ERR"), "{}", result.1);
    assert!(!result.1.contains("OUT"), "{}", result.1);
}

// STORY-1165: the label mirror must route to the right forge CLI — gh for
// GitHub, glab for GitLab (mr update --label/--unlabel), nothing for pure-git.
// TASK-1455: and every routed call carries the pinned `-R <repo>`.
#[test]
fn sync_label_command_routes_per_forge() {
    use crate::forge::ForgeKind;
    let gh = pin(ForgeKind::GitHub, "git@github.com:o/r.git");
    let (cli, args) = sync_label_command(&gh, 42, true, ClearedLabel::Remove).unwrap();
    assert_eq!(cli, "gh");
    assert!(
        args.contains(&"--add-label".to_string())
            && args.contains(&format!("{HOLD_LABEL},{HOLD_RECORDED_LABEL}"))
    );
    assert!(args
        .windows(2)
        .any(|w| w[0] == "--remove-label" && w[1] == HOLD_CLEARED_LABEL));
    assert!(
        args.windows(2).any(|w| w[0] == "-R" && w[1] == "o/r"),
        "{args:?}"
    );

    let gl = pin(ForgeKind::GitLab, "https://gitlab.com/g/sub/p.git");
    let (cli, args) = sync_label_command(&gl, 42, true, ClearedLabel::Remove).unwrap();
    assert_eq!(cli, "glab");
    assert!(args.contains(&"mr".to_string()) && args.contains(&"update".to_string()));
    assert!(
        args.contains(&"--label".to_string())
            && args.contains(&format!("{HOLD_LABEL},{HOLD_RECORDED_LABEL}"))
    );
    assert!(args
        .windows(2)
        .any(|w| w[0] == "--unlabel" && w[1] == HOLD_CLEARED_LABEL));
    assert!(
        args.windows(2).any(|w| w[0] == "-R" && w[1] == "g/sub/p"),
        "{args:?}"
    );

    let (_, args) = sync_label_command(&gl, 42, false, ClearedLabel::Add).unwrap();
    assert!(
        args.contains(&"--unlabel".to_string()),
        "unheld → remove the label"
    );
    assert!(args.contains(&HOLD_LABEL.to_string()));
    assert!(args.contains(&"--label".to_string()));
    assert!(args.contains(&HOLD_CLEARED_LABEL.to_string()));

    let none = PinnedRepo {
        kind: ForgeKind::None,
        host: "example.org".into(),
        path: "o/r".into(),
    };
    assert!(
        sync_label_command(&none, 42, true, ClearedLabel::Remove).is_none(),
        "pure-git has no forge to label"
    );
}

// trace:BUG-1747 | ai:codex
#[test]
fn held_github_label_update_is_one_atomic_comma_list() {
    let repo = pin(crate::forge::ForgeKind::GitHub, "git@github.com:o/r.git");
    let (_, args) = sync_label_command(&repo, 9, true, ClearedLabel::Remove).unwrap();
    // An undefined member of the comma list exits 1 and applies neither label,
    // so provisioning must cover all three definitions before any hold is placed.
    assert_eq!(
        args[args.iter().position(|arg| arg == "--add-label").unwrap() + 1],
        format!("{HOLD_LABEL},{HOLD_RECORDED_LABEL}")
    );
}

// trace:BUG-1747 | ai:codex
#[test]
fn label_sync_failure_classification_matches_forge_diagnostics() {
    assert_eq!(
        classify_label_sync_failure(
            "'aida:merge-hold-cleared' not found",
            &[HOLD_LABEL, HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL]
        ),
        LabelSyncError::DefinitionMissing {
            names: vec![HOLD_CLEARED_LABEL.into()],
            detail: "'aida:merge-hold-cleared' not found".into()
        }
    );
    assert!(matches!(
        classify_label_sync_failure("HTTP 502", &[HOLD_LABEL]),
        LabelSyncError::Other(_)
    ));
    assert_eq!(
        classify_label_sync_failure(
            "'aida:merge-hold' not found and 'aida:merge-hold-recorded' not found",
            &[HOLD_LABEL, HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL]
        ),
        LabelSyncError::DefinitionMissing {
            names: vec![HOLD_LABEL.into(), HOLD_RECORDED_LABEL.into()],
            detail: "'aida:merge-hold' not found and 'aida:merge-hold-recorded' not found".into()
        }
    );
}

// trace:BUG-1747 | ai:codex
#[test]
fn label_definition_failure_skips_retry_but_transient_retries() {
    let dir = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["init", "-q"])
        .status()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["remote", "add", "origin", "git@github.com:o/r.git"])
        .status()
        .unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    let definition_calls = std::cell::Cell::new(0);
    let err = sync_label_with(dir.path(), 42, true, |_, _, _| {
        definition_calls.set(definition_calls.get() + 1);
        Ok((false, "'aida:merge-hold-cleared' not found".into()))
    })
    .unwrap_err();
    assert_eq!(definition_calls.get(), 1);
    assert!(err.is_definition_missing());
    let transient_calls = std::cell::Cell::new(0);
    let _ = sync_label_with(dir.path(), 43, false, |_, _, _| {
        transient_calls.set(transient_calls.get() + 1);
        Ok((false, "HTTP 502".into()))
    });
    assert_eq!(transient_calls.get(), 2);
}

// trace:BUG-1747 | ai:codex
#[test]
fn merge_hold_label_create_argv_covers_registry_with_force_and_metadata() {
    let repo = pin(crate::forge::ForgeKind::GitHub, "git@github.com:o/r.git");
    for label in MERGE_HOLD_LABELS {
        let (cli, args) = create_label_command(&repo, label);
        assert_eq!(cli, "gh");
        assert_eq!(args[0..3], ["label", "create", label.name]);
        assert!(args
            .windows(2)
            .any(|w| w == ["-R", repo.repo_arg().as_str()]));
        assert!(args.windows(2).any(|w| w == ["--color", label.color]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--description", label.description]));
        assert!(args.contains(&"--force".into()));
    }
}

// trace:BUG-1747 | ai:codex
#[test]
fn repo_label_definition_probe_maps_output_without_treating_failures_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    let repo = pin(crate::forge::ForgeKind::GitHub, "git@github.com:o/r.git");
    let all = serde_json::to_string(
        &MERGE_HOLD_LABELS
            .iter()
            .map(|l| serde_json::json!({"name": l.name}))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let LabelDefinitions::Read { missing, .. } =
        label_definitions_with(dir.path(), &repo, |_, _, _| Ok((true, all.clone())))
    else {
        panic!("expected read")
    };
    assert!(missing.is_empty());
    let partial = format!(r#"[{{"name":"{}"}}]"#, HOLD_LABEL);
    let LabelDefinitions::Read { missing, .. } =
        label_definitions_with(dir.path(), &repo, |_, _, _| Ok((true, partial.clone())))
    else {
        panic!("expected read")
    };
    assert_eq!(missing, vec![HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL]);
    assert!(matches!(
        label_definitions_with(dir.path(), &repo, |_, _, _| Ok((false, String::new()))),
        LabelDefinitions::Unknown(_)
    ));
    assert!(matches!(
        label_definitions_with(dir.path(), &repo, |_, _, _| Ok((true, "not json".into()))),
        LabelDefinitions::Unknown(_)
    ));
}

// trace:BUG-1747 | ai:codex
#[test]
fn github_probe_does_not_treat_longer_labels_as_the_base_label() {
    let dir = tempfile::tempdir().unwrap();
    let repo = pin(crate::forge::ForgeKind::GitHub, "git@github.com:o/r.git");
    let only_long_names =
        r#"[{"name":"aida:merge-hold-recorded"},{"name":"aida:merge-hold-cleared"}]"#;
    let LabelDefinitions::Read { present, missing } =
        label_definitions_with(dir.path(), &repo, |_, _, _| {
            Ok((true, only_long_names.into()))
        })
    else {
        panic!("expected read")
    };
    assert_eq!(missing, vec![HOLD_LABEL]);
    assert_eq!(present, vec![HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL]);
}

// trace:BUG-1747 | ai:codex
#[test]
fn gitlab_probe_does_not_treat_longer_labels_as_the_base_label() {
    let dir = tempfile::tempdir().unwrap();
    let repo = pin(crate::forge::ForgeKind::GitLab, "git@gitlab.com:o/r.git");
    let only_long_names = "aida:merge-hold-recorded\naida:merge-hold-cleared\n";
    let LabelDefinitions::Read { present, missing } =
        label_definitions_with(dir.path(), &repo, |_, _, _| {
            Ok((true, only_long_names.into()))
        })
    else {
        panic!("expected read")
    };
    assert_eq!(missing, vec![HOLD_LABEL]);
    assert_eq!(present, vec![HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL]);
}

// TASK-1455: the pin comes from origin; enterprise / self-managed hosts
// are host-qualified so a number never resolves against the public forge.
#[test]
fn pinned_repo_arg_is_host_qualified_off_the_public_forge() {
    use crate::forge::ForgeKind;
    assert_eq!(
        pin(ForgeKind::GitHub, "https://github.com/Joe/aida.git").repo_arg(),
        "Joe/aida"
    );
    assert_eq!(
        pin(ForgeKind::GitHub, "git@ghe.corp.example:team/svc.git").repo_arg(),
        "ghe.corp.example/team/svc"
    );
    // An ssh alias is resolved by the CLI itself; pin the path.
    assert_eq!(
        pin(ForgeKind::GitHub, "git@github-work:team/svc.git").repo_arg(),
        "team/svc"
    );
    assert_eq!(
        pin(
            ForgeKind::GitLab,
            "ssh://git@gitlab.corp.example:2222/g/p.git"
        )
        .repo_arg(),
        "https://gitlab.corp.example/g/p"
    );
}

// TASK-1455 / PRIN-5: no resolvable repo means REFUSE, never call unpinned.
#[test]
fn unresolvable_origin_refuses_instead_of_calling_unpinned() {
    use crate::forge::ForgeKind;
    for origin in [
        None,
        Some(""),
        Some("/srv/git/repo"),
        Some("https://github.com/"),
    ] {
        let err = pin_from_origin(ForgeKind::GitHub, origin).unwrap_err();
        assert!(
            err.contains("refusing to call `gh` unpinned"),
            "{origin:?}: {err}"
        );
    }
    assert!(pin_from_origin(ForgeKind::GitHub, Some("git@github.com:o/r.git")).is_ok());
}

// TASK-1455 acceptance 2: a forge answer naming a DIFFERENT repo (or a
// different number) is refused, and the pinned argv names the repo.
#[test]
fn pinned_change_refuses_an_answer_from_a_mismatched_repo() {
    use crate::forge::ForgeKind;
    let dir = tempfile::tempdir().unwrap();
    let gh = pin(ForgeKind::GitHub, "https://github.com/o/r.git");
    let seen = std::cell::RefCell::new(Vec::new());
    let answer = |url: &'static str| {
        let seen = &seen;
        move |_: &Path, cli: &str, args: &[String]| {
            seen.borrow_mut().push((cli.to_string(), args.to_vec()));
            Ok((
                true,
                format!(
                    r#"{{"url":"{url}","state":"OPEN","headRefOid":"abc","labels":[{{"name":"aida:merge-hold"}}]}}"#
                ),
            ))
        }
    };
    let ok = fetch_pinned_change_with(dir.path(), &gh, 7, answer("https://github.com/o/r/pull/7"))
        .unwrap();
    assert_eq!(
        ok,
        PinnedChange {
            merged: false,
            state: ChangeState::Open,
            head_sha: Some("abc".into()),
            hold_label: true,
            recorded_label: false,
            cleared_label: false,
        }
    );
    let (cli, args) = seen.borrow()[0].clone();
    assert_eq!(cli, "gh");
    assert!(
        args.windows(2).any(|w| w[0] == "-R" && w[1] == "o/r"),
        "{args:?}"
    );

    for wrong in [
        "https://github.com/other/r/pull/7",
        "https://github.com/o/r/pull/8",
        "https://ghe.example.com/o/r/pull/7",
    ] {
        let err = fetch_pinned_change_with(dir.path(), &gh, 7, answer(wrong)).unwrap_err();
        assert!(err.contains("refusing forge answer"), "{wrong}: {err}");
    }
    let err = fetch_pinned_change_with(dir.path(), &gh, 7, |_: &Path, _: &str, _: &[String]| {
        Ok((true, r#"{"state":"OPEN","labels":[]}"#.to_string()))
    })
    .unwrap_err();
    assert!(err.contains("<no url>"), "{err}");

    let gl = pin(ForgeKind::GitLab, "https://gitlab.com/g/p.git");
    let merged = fetch_pinned_change_with(dir.path(), &gl, 3, |_: &Path, cli: &str, args: &[String]| {
        assert_eq!(cli, "glab");
        assert!(args.contains(&"projects/g%2Fp/merge_requests/3".to_string()), "{args:?}");
        Ok((
            true,
            r#"{"web_url":"https://gitlab.com/g/p/-/merge_requests/3","state":"merged","sha":"d1","labels":["bug"]}"#
                .to_string(),
        ))
    })
    .unwrap();
    assert!(merged.merged && !merged.hold_label);
}

// TASK-1455: a label sync on a repo that cannot be pinned refuses AND
// records the marker unsynced — it never shells out unpinned.
#[test]
fn sync_label_refuses_and_records_when_the_repo_cannot_be_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["init", "-q"])
        .output()
        .unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    write_hold(root, 5, "drive").unwrap();
    let err = sync_label_with(root, 5, true, |_, _, _| {
        panic!("an unpinned forge call must never be made")
    })
    .unwrap_err();
    assert!(format!("{err}").contains("unpinned"), "{err}");
    assert!(matches!(read_label_state(root, 5), LabelState::Unsynced(_)));
}

/// TASK-1951: an unhold adds `aida:merge-hold-cleared` only when the forge
/// shows a recorded hold. A never-recorded PR has an orphaned cleared label
/// removed, and an unreadable forge gets neither: the gate fails on a
/// cleared label with no recorded one.
// trace:TASK-1951 | ai:claude
#[test]
fn unhold_adds_the_cleared_label_only_for_a_recorded_hold() {
    let dir = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["init", "-q"])
        .status()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["remote", "add", "origin", "git@github.com:o/r.git"])
        .status()
        .unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    let view = |labels: &'static str| {
        move |_: &Path, _: &str, _: &[String]| {
            Ok((
                true,
                format!(
                    r#"{{"url":"https://github.com/o/r/pull/42","state":"OPEN","headRefOid":"abc","labels":[{labels}]}}"#
                ),
            ))
        }
    };
    // The production path: `sync_label_via` reads the pinned change with
    // the injected reader, then writes with the injected runner.
    let cleared_ops = |read: &dyn Fn(&Path, &str, &[String]) -> Result<(bool, String), String>| {
        let writes = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let result = sync_label_via(
            dir.path(),
            42,
            false,
            |_, _, args| {
                writes.borrow_mut().push(args.to_vec());
                Ok((true, String::new()))
            },
            read,
        );
        let ops: Vec<String> = writes
            .borrow()
            .iter()
            .flat_map(|args| {
                args.windows(2)
                    .filter(|w| w[1] == HOLD_CLEARED_LABEL)
                    .map(|w| w[0].clone())
                    .collect::<Vec<_>>()
            })
            .collect();
        let count = writes.borrow().len();
        (result, count, ops)
    };
    let (ok, _, ops) = cleared_ops(&view(r#"{"name":"aida:merge-hold-recorded"}"#));
    assert!(ok.is_ok());
    assert_eq!(ops, ["--add-label"]);
    let (ok, _, ops) = cleared_ops(&view(r#"{"name":"aida:merge-hold-cleared"}"#));
    assert!(ok.is_ok());
    assert_eq!(ops, ["--remove-label"]);
    // An unreadable forge changes no label and reports the failure.
    let (err, writes, _) = cleared_ops(&|_, _, _| Err("HTTP 502".into()));
    let err = err.unwrap_err().to_string();
    assert!(
        // The caller names the retry (TASK-1951). trace:TASK-1951 | ai:claude
        err.contains("HTTP 502") && err.contains("no label was changed"),
        "{err}"
    );
    assert_eq!(writes, 0, "an unknown probe must not touch labels");

    let removed = std::cell::RefCell::new(Vec::new());
    let runner = |_: &Path, _: &str, args: &[String]| {
        removed.borrow_mut().push(args.to_vec());
        Ok((true, String::new()))
    };
    assert!(drop_orphaned_cleared_label_with(
        dir.path(),
        42,
        view(r#"{"name":"aida:merge-hold-cleared"}"#),
        runner
    )
    .unwrap());
    assert!(removed.borrow()[0]
        .windows(2)
        .any(|w| w[0] == "--remove-label" && w[1] == HOLD_CLEARED_LABEL));
    let untouched = |_: &Path, _: &str, _: &[String]| -> Result<(bool, String), String> {
        panic!("no label write expected")
    };
    assert!(!drop_orphaned_cleared_label_with(
        dir.path(),
        42,
        view(r#"{"name":"aida:merge-hold-recorded"},{"name":"aida:merge-hold-cleared"}"#),
        untouched,
    )
    .unwrap());
    assert!(
        !drop_orphaned_cleared_label_with(dir.path(), 42, view(""), untouched).unwrap(),
        "a never-held PR without the label gets no write"
    );
}

// TASK-189: the forge label is its own state; an unreadable forge is
// divergence-UNKNOWN, never agreement and never the marker's claim.
#[test]
fn forge_label_state_and_divergence() {
    let change = |hold_label| {
        Ok(Some(PinnedChange {
            merged: false,
            state: ChangeState::Open,
            head_sha: None,
            hold_label,
            recorded_label: false,
            cleared_label: false,
        }))
    };
    assert_eq!(ForgeLabel::from_fetch(&change(true)), ForgeLabel::Present);
    assert_eq!(ForgeLabel::from_fetch(&change(false)), ForgeLabel::Absent);
    assert_eq!(ForgeLabel::from_fetch(&Ok(None)), ForgeLabel::NoForge);
    let unknown = ForgeLabel::from_fetch(&Err("gh: offline".into()));
    assert_eq!(unknown.as_str(), "unknown");
    assert_eq!(label_diverged(&ForgeLabel::Present), Some(false));
    assert_eq!(label_diverged(&ForgeLabel::Absent), Some(true));
    assert_eq!(label_diverged(&ForgeLabel::NoForge), Some(false));
    assert_eq!(label_diverged(&unknown), None);
}

// BUG-1469: the label scan is ONE pinned query; every entry must belong
// to the pinned repo and carry the label, and a full page is reported as
// possibly truncated rather than trusted.
// trace:BUG-1469 | ai:claude
#[test]
fn labeled_change_scan_is_pinned_and_refuses_unattributable_answers() {
    use crate::forge::ForgeKind;
    let dir = tempfile::tempdir().unwrap();
    let gh = pin(ForgeKind::GitHub, "https://github.com/o/r.git");
    let (cli, args) = labeled_changes_command(&gh).unwrap();
    assert_eq!(cli, "gh");
    assert!(
        args.windows(2).any(|w| w[0] == "-R" && w[1] == "o/r"),
        "{args:?}"
    );
    assert!(args
        .windows(2)
        .any(|w| w[0] == "--label" && w[1] == HOLD_LABEL));
    assert!(args.windows(2).any(|w| w[0] == "--state" && w[1] == "open"));

    let row = |n: u64, repo: &str, label: &str| {
        format!(
            r#"{{"number":{n},"url":"https://github.com/{repo}/pull/{n}","labels":[{{"name":"{label}"}}]}}"#
        )
    };
    let answer = |body: String| move |_: &Path, _: &str, _: &[String]| Ok((true, body.clone()));
    let ok = list_labeled_open_changes_with(
        dir.path(),
        &gh,
        answer(format!(
            "[{},{}]",
            row(1978, "o/r", HOLD_LABEL),
            row(1974, "o/r", HOLD_LABEL)
        )),
    )
    .unwrap();
    assert_eq!(ok, vec![1974, 1978]);

    let foreign = format!("[{}]", row(7, "other/r", HOLD_LABEL));
    let err = list_labeled_open_changes_with(dir.path(), &gh, answer(foreign)).unwrap_err();
    assert!(err.contains("pinned repo"), "{err}");
    let unlabeled = format!("[{}]", row(7, "o/r", "bug"));
    let err = list_labeled_open_changes_with(dir.path(), &gh, answer(unlabeled)).unwrap_err();
    assert!(err.contains("does not carry"), "{err}");
    let full = format!(
        "[{}]",
        (1..=LABELED_QUERY_LIMIT as u64)
            .map(|n| row(n, "o/r", HOLD_LABEL))
            .collect::<Vec<_>>()
            .join(",")
    );
    let err = list_labeled_open_changes_with(dir.path(), &gh, answer(full)).unwrap_err();
    assert!(err.contains("truncated"), "{err}");
    let err = list_labeled_open_changes_with(dir.path(), &gh, |_: &Path, _: &str, _: &[String]| {
        Ok((false, String::new()))
    })
    .unwrap_err();
    assert!(err.contains("failed"), "{err}");

    let gl = pin(ForgeKind::GitLab, "https://gitlab.com/g/p.git");
    let (cli, args) = labeled_changes_command(&gl).unwrap();
    assert_eq!(cli, "glab");
    assert!(
        args.iter().any(|a| a
            .starts_with("projects/g%2Fp/merge_requests?state=opened&labels=aida%3Amerge-hold")),
        "{args:?}"
    );
    let got = list_labeled_open_changes_with(dir.path(), &gl, |_: &Path, _: &str, _: &[String]| {
        Ok((
            true,
            r#"[{"iid":3,"web_url":"https://gitlab.com/g/p/-/merge_requests/3","labels":["aida:merge-hold"]}]"#
                .to_string(),
        ))
    })
    .unwrap();
    assert_eq!(got, vec![3]);
}

// BUG-1541: state words parse per forge; anything unrecognised is Unknown
// and never terminal (fail closed).
// trace:BUG-1541 | ai:claude
#[test]
fn change_state_parses_both_forges_and_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let gh = pin(
        crate::forge::ForgeKind::GitHub,
        "https://github.com/o/r.git",
    );
    let closed = fetch_pinned_change_with(dir.path(), &gh, 7, |_: &Path, _: &str, _: &[String]| {
        Ok((
            true,
            r#"{"url":"https://github.com/o/r/pull/7","state":"CLOSED","labels":[]}"#.to_string(),
        ))
    })
    .unwrap();
    assert_eq!(closed.state, ChangeState::ClosedUnmerged);
    assert!(!closed.merged);
    assert_eq!(closed.state.terminal(), Some(TerminalState::ClosedUnmerged));
    let missing =
        fetch_pinned_change_with(dir.path(), &gh, 7, |_: &Path, _: &str, _: &[String]| {
            Ok((
                true,
                r#"{"url":"https://github.com/o/r/pull/7","labels":[]}"#.to_string(),
            ))
        })
        .unwrap();
    assert_eq!(missing.state, ChangeState::Unknown);
    assert_eq!(missing.state.terminal(), None);
    assert_eq!(ChangeState::parse(Some("locked")).terminal(), None);
}

// BUG-1562, red-first: a marker citing a sha is REPORTED once the PR head
// moves past it, and not while the head still matches. A rework hold whose
// verdict is now approved (or closed by a merge) is reported too. Nothing
// is cleared — the marker is untouched.
// trace:BUG-1562 | ai:claude
#[test]
fn premise_stale_fires_when_the_world_moves_and_never_clears() {
    use crate::review_verdict::{RecordedVerdict, VerdictKind};
    let dir = tempfile::tempdir().unwrap();
    write_hold(
        dir.path(),
        2049,
        "CHANGES REQUESTED for STORY-1422 at 3acf3671fd7a",
    )
    .unwrap();
    let before = std::fs::read(hold_path(dir.path(), 2049)).unwrap();
    let legacy = read_hold_record(dir.path(), 2049).unwrap();
    let none = |_: &str| None;
    let ns = |_: &str| -> Option<String> { None };
    // Current: head still at the cited sha → not stale.
    assert_eq!(
        premise_stale(&legacy, Some("3acf3671fd7a0000"), none, ns),
        None
    );
    // Head unknown → unknown is never stale.
    assert_eq!(premise_stale(&legacy, None, none, ns), None);
    // Advance the head past it → reported.
    let why = premise_stale(&legacy, Some("cd21a1dc0a9e1111"), none, ns).expect("stale");
    assert!(
        why.contains("3acf3671fd7a") && why.contains("cd21a1dc0a9e"),
        "{why}"
    );
    assert_eq!(std::fs::read(hold_path(dir.path(), 2049)).unwrap(), before);

    // A PR number or plain word is never mistaken for a cited sha.
    let plain = typed_hold(
        5,
        HoldReasonKind::Supervision,
        "PR 20490123 is marked drive",
        None,
    );
    assert_eq!(premise_stale(&plain, Some("abcdef1234567"), none, ns), None);

    // Typed target_head_sha wins over prose.
    let rework = typed_hold(
        2001,
        HoldReasonKind::Rework,
        "stranded refusal recovered for BUG-1291 at 64e4e5755b",
        Some("64e4e5755b".into()),
    );
    assert_eq!(premise_stale(&rework, Some("64e4e5755b99"), none, ns), None);
    let approved = |spec: &str| {
        (spec == "BUG-1291").then(|| RecordedVerdict {
            kind: VerdictKind::Approved,
            raw: "approved".into(),
            reviewed_sha: Some("af49b12b83fc".into()),
            ..Default::default()
        })
    };
    let why = premise_stale(&rework, Some("af49b12b83fc"), approved, ns).expect("stale");
    assert!(
        why.contains("BUG-1291's verdict is now APPROVED at af49b12b83fc"),
        "{why}"
    );
    assert!(why.contains("PR head is now af49b12b83fc"), "{why}");
    let closed = |_: &str| {
        Some(RecordedVerdict {
            kind: VerdictKind::RequestChanges,
            closed_by_merge: Some("deadbeef1234".into()),
            ..Default::default()
        })
    };
    let why = premise_stale(&rework, Some("64e4e5755b"), closed, ns).expect("closed");
    assert!(why.contains("closed by merge deadbeef1234"), "{why}");
    // A supervision hold's premise is not the verdict — an approval does
    // not make "awaiting a human merge" stale.
    let supervised = typed_hold(
        9,
        HoldReasonKind::Supervision,
        "BUG-1291 is marked drive",
        None,
    );
    assert_eq!(
        premise_stale(&supervised, Some("af49b12b83fc"), approved, ns),
        None
    );
}

// BUG-1562: "spec no longer running" — a hold whose spec is now terminal
// in the local store is flagged; a live one is not. Flag only.
// trace:BUG-1562 | ai:claude
#[test]
fn premise_flags_a_hold_whose_spec_is_no_longer_running() {
    let none = |_: &str| None;
    let mut hold = typed_hold(9, HoldReasonKind::Supervision, "is marked drive", None);
    hold.spec = Some("STORY-1".into());
    let running = |_: &str| Some("In Progress".to_string());
    assert_eq!(premise_stale(&hold, None, none, running), None);
    let done_on_branch = |_: &str| Some("Done".to_string());
    assert_eq!(premise_stale(&hold, None, none, done_on_branch), None);
    let completed = |s: &str| (s == "STORY-1").then(|| "Completed".to_string());
    let why = premise_stale(&hold, None, none, completed).expect("stale");
    assert!(
        why.contains("STORY-1 is no longer running (status completed)"),
        "{why}"
    );
    // Untyped: the spec named in the prose is checked (flag only).
    let prose = typed_hold(
        9,
        HoldReasonKind::Supervision,
        "STORY-1 is marked drive",
        None,
    );
    assert!(premise_stale(&prose, None, none, completed).is_some());
    let unknown = |_: &str| None;
    assert_eq!(premise_stale(&prose, None, none, unknown), None);
}

#[test]
fn parses_gitlab_label_shapes() {
    assert!(labels_json_contains(r#"{"labels":["bug","aida:merge-hold"]}"#, HOLD_LABEL).unwrap());
    assert!(
        labels_json_contains(r#"{"labels":[{"name":"aida:merge-hold"}]}"#, HOLD_LABEL).unwrap()
    );
    assert!(!labels_json_contains(r#"{"labels":["bug"]}"#, HOLD_LABEL).unwrap());
}

#[test]
fn live_head_projection_marks_stale_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let mut record = recused_record();
    record.routing_state = HoldRoutingState::Routed;
    record.routed_to = vec![PrincipalIdentity::parse("agent:reader")];
    std::fs::create_dir_all(holds_dir(dir.path())).unwrap();
    let body = serde_json::to_vec_pretty(&record).unwrap();
    std::fs::write(hold_path(dir.path(), 42), &body).unwrap();

    let view = project_live_head(&record, Some("head-b"));
    assert_eq!(view.routing_state, HoldRoutingState::StaleHead);
    assert!(view.routed_to.is_empty());
    assert_eq!(view.target_head_sha.as_deref(), Some("head-b"));
    // Same head: unchanged. No head known: unchanged.
    assert_eq!(project_live_head(&record, Some("head-a")), record);
    assert_eq!(project_live_head(&record, None), record);
    // The durable marker is untouched.
    assert_eq!(std::fs::read(hold_path(dir.path(), 42)).unwrap(), body);
}

#[test]
fn recusal_routing_is_idempotent_and_excludes_recused_agent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida/agents")).unwrap();
    for (file, id) in [("author.toml", "author"), ("reader.toml", "reader")] {
        std::fs::write(dir.path().join(".aida/agents").join(file), format!("id = \"{id}\"\nagent_type = \"codex\"\npid = {}\nrole = \"reviewer\"\navailability = \"available\"\n", std::process::id())).unwrap();
    }
    let mut record = typed_hold(43, HoldReasonKind::Recusal, "recused", Some("abc".into()));
    record.recused_principals = vec![PrincipalIdentity::parse("agent:author")];
    write_typed_hold(dir.path(), &record).unwrap();
    write_typed_hold(dir.path(), &record).unwrap();
    let reread = read_hold_record(dir.path(), 43).unwrap();
    assert_eq!(
        reread.routed_to,
        vec![PrincipalIdentity::parse("agent:reader")]
    );
    let briefs = std::fs::read_dir(dir.path().join(".aida/agent-briefs/codex"))
        .unwrap()
        .count();
    assert_eq!(briefs, 1);

    std::fs::remove_file(dir.path().join(".aida/agents/reader.toml")).unwrap();
    write_typed_hold(dir.path(), &reread).unwrap();
    let unrouted = read_hold_record(dir.path(), 43).unwrap();
    assert_eq!(
        unrouted.routing_state,
        HoldRoutingState::NoIndependentReader
    );
    assert!(dir
        .path()
        .join(".aida/agent-briefs/codex/PR-43-abc.md.stale")
        .exists());

    std::fs::write(dir.path().join(".aida/agents/new-reader.toml"), format!("id = \"new-reader\"\nagent_type = \"codex\"\npid = {}\nrole = \"reviewer\"\navailability = \"available\"\n", std::process::id())).unwrap();
    let mut moved = unrouted;
    moved.target_head_sha = Some("def".into());
    moved.routing_state = HoldRoutingState::StaleHead;
    write_typed_hold(dir.path(), &moved).unwrap();
    let rerouted = read_hold_record(dir.path(), 43).unwrap();
    assert_eq!(
        rerouted.routed_to,
        vec![PrincipalIdentity::parse("agent:new-reader")]
    );
    assert!(dir
        .path()
        .join(".aida/agent-briefs/codex/PR-43-def.md")
        .exists());
}
