// trace:BUG-2452 | ai:codex
use super::*;

fn clean() -> PrState {
    PrState {
        state: "OPEN".into(),
        head_ref_oid: "a".repeat(40),
        mergeable: "MERGEABLE".into(),
    }
}

#[test]
fn bug_2452_conflict_is_an_empty_rework_trigger() {
    let mut state = clean();
    state.mergeable = "CONFLICTING".into();
    assert!(reasons(&state, &[], &[])[0].contains("CONFLICTING"));
    state.mergeable = "UNKNOWN".into();
    assert!(reasons(&state, &[], &[]).is_empty());
}

#[test]
fn bug_2452_failing_required_checks_are_named_but_pending_is_not_failure() {
    let rows = crate::ci_gate::parse_check_rows(
        r#"[
        {"name":"Build (ubuntu-latest)","bucket":"fail"},
        {"name":"cancelled-required","bucket":"cancel"},
        {"name":"waiting","bucket":"pending"},
        {"name":"green","bucket":"pass"}]"#,
    )
    .unwrap();
    let reasons = reasons(&clean(), &rows, &[]);
    assert_eq!(reasons.len(), 2);
    assert!(reasons[0].contains("Build (ubuntu-latest)"));
    assert!(reasons[1].contains("cancelled-required"));
}

#[test]
fn bug_2452_current_head_findings_only() {
    let state = clean();
    let mut verdict = crate::gate::review_verdict::parse_recorded_verdict(&format!(
        r#"{{"verdict":"request-changes","reviewed_sha":"{}","summary":"leak remains","findings":["F1: close the handle"]}}"#, state.head_ref_oid
    )).unwrap();
    let got = reasons(&state, &[], &[verdict.clone()]);
    assert!(got[0].contains("F1: close the handle"));
    verdict.reviewed_sha = Some("a".repeat(12));
    assert_eq!(reasons(&state, &[], &[verdict.clone()]).len(), 1);
    verdict.reviewed_sha = Some("b".repeat(40));
    assert!(reasons(&state, &[], &[verdict]).is_empty());
}

#[test]
fn bug_2452_missing_summary_uses_verdict_kind_without_raw_json() {
    let state = clean();
    for (raw, kind) in [
        ("CHANGES REQUESTED", "request-changes"),
        ("FAIL", "rejected"),
        ("unrecognized verdict with verbose context", "unknown"),
    ] {
        let verdict = crate::gate::review_verdict::parse_recorded_verdict(
            &serde_json::json!({
                "verdict": raw, "reviewed_sha": state.head_ref_oid,
                "findings": ["F1: close the handle"]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            reasons(&state, &[], &[verdict]),
            vec![format!("blocking review: {kind}\nF1: close the handle")]
        );
    }
}

#[test]
fn bug_2452_unknown_local_push_skips_even_if_forge_outcome_has_old_head() {
    let head = "a".repeat(40);
    let launch_pr = (7, Some(head.clone()));
    let launch = super::super::LaunchSeat {
        pr: Some(&launch_pr),
        head: Some(&head),
    };
    let outcome = super::super::Outcome::PrOpened {
        pr: 7,
        head: Some(head.clone()),
    };
    for seat in [
        super::super::SeatState {
            head: Some("b".repeat(40)),
            origin: crate::headless_background::OriginHead::Unknown,
            uncommitted: Some(0),
        },
        super::super::SeatState {
            head: None,
            origin: crate::headless_background::OriginHead::At(head.clone()),
            uncommitted: Some(0),
        },
    ] {
        assert_eq!(seat.pushed_since(launch.head), None);
        // No forge I/O is needed: an old forge head cannot disprove a push.
        assert!(inspect(
            Path::new("/no-such-root"),
            "BUG-2452",
            &launch,
            &outcome,
            &seat
        )
        .unwrap()
        .is_none());
    }
}

#[test]
fn bug_2452_conflicting_pr_without_registered_checks_is_classified() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let head = "a".repeat(40);
    std::fs::write(
        root.join("view.json"),
        serde_json::json!({"state":"OPEN","headRefOid":head,"mergeable":"CONFLICTING"}).to_string(),
    )
    .unwrap();
    let path = format!("{}:/usr/bin:/bin", root.display());
    let _env = crate::test_env::EnvVarsGuard::apply(&[("PATH", Some(&path))]);
    let launch_pr = (7, Some(head.clone()));
    let launch = super::super::LaunchSeat {
        pr: Some(&launch_pr),
        head: Some(&head),
    };
    let seat = super::super::SeatState {
        head: Some(head.clone()),
        origin: crate::headless_background::OriginHead::At(head.clone()),
        uncommitted: Some(0),
    };
    for configured in [false, true] {
        std::fs::write(
            root.join(".aida/config.toml"),
            format!(
                "[forge]\nprovider = \"github\"\n{}",
                if configured {
                    "[review_watch]\nrequired_checks = [\"Build\"]\n"
                } else {
                    ""
                }
            ),
        )
        .unwrap();
        for redirect in ["", " >&2"] {
            crate::test_exec::write_executable(
                &root.join("gh"),
                &format!(
                    "#!/bin/sh\ncase \"$2\" in\nview) /bin/cat view.json ;;\nchecks) printf 'no checks reported on the rework branch\\n'{redirect}; exit 1 ;;\nesac\n"
                ),
            );
            let result = inspect(
                root,
                "BUG-2452",
                &launch,
                &super::super::Outcome::NoPr,
                &seat,
            )
            .unwrap()
            .expect("conflicts must trigger routing even when CI never registered");
            assert_eq!(result.0, 7);
            assert_eq!(result.1, head);
            assert!(result.2.contains("mergeable=CONFLICTING"));
        }
    }
}

#[test]
fn bug_2452_explicit_disputed_finding_punt_precedes_forge_io() {
    let outcome = super::super::Outcome::Punted {
        reason: "dispute finding F1: intended behavior".into(),
    };
    let launch_pr = (1, Some("a".repeat(40)));
    let launch = super::super::LaunchSeat {
        pr: Some(&launch_pr),
        head: Some("a"),
    };
    let seat = super::super::SeatState {
        head: None,
        origin: crate::headless_background::OriginHead::Unknown,
        uncommitted: None,
    };
    assert!(inspect(
        Path::new("/no-such-root"),
        "BUG-2452",
        &launch,
        &outcome,
        &seat
    )
    .unwrap()
    .is_none());
}

#[test]
fn bug_2452_first_exit_queues_brief_second_exit_requests_advisor_and_new_head_resets() {
    let (_dir, root, store_root, req) =
        crate::bug_1638_race_seam_tests::project_with(RequirementStatus::Done);
    let _env = crate::test_env::pin_queue_user();
    let spec = crate::bug_1638_race_seam_tests::SPEC;
    assert!(!route(
        &root,
        spec,
        "codex",
        7,
        "aaaa",
        "required check failed: Build"
    )
    .unwrap());
    let storage = Storage::new(&store_root);
    let rows = storage.queue_list("role:implementer", true).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].requirement_id, req.id);
    assert!(rows[0].note.as_deref().unwrap().contains("Build"));
    assert!(crate::brief::pending_brief_exists(&root, "codex", spec));
    assert_eq!(
        storage
            .load()
            .unwrap()
            .get_requirement_unambiguous(spec)
            .unwrap()
            .unwrap()
            .status,
        RequirementStatus::InProgress
    );
    assert!(route(&root, spec, "codex", 7, "aaaa", "mergeable=CONFLICTING").unwrap());
    assert!(storage
        .queue_list("role:implementer", true)
        .unwrap()
        .is_empty());
    let mail: aida_core::mailbox::Message = serde_json::from_slice(
        &std::fs::read(root.join(format!(".aida/mailbox/empty-rework-{}-7-aaaa.json", req.id)))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(mail.intent, aida_core::mailbox::Intent::Request);
    assert_eq!(
        mail.to,
        aida_core::mailbox::Recipient::Agent("advisor".into())
    );
    assert!(mail.body.contains("CONFLICTING"));
    assert!(!route(
        &root,
        spec,
        "codex",
        7,
        "bbbb",
        "required check failed: Build"
    )
    .unwrap());
}

#[test]
fn bug_2452_launcher_probe_covers_old_pr_no_pr_head_movement_and_unknown_evidence() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida/review-verdicts")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    crate::test_exec::write_executable(
        &root.join("gh"),
        r#"#!/bin/sh
case "$2" in
  view)
    if [ -f unknown ]; then exit 1; fi
    if [ -f moved ]; then /bin/cat next.json; else /bin/cat view.json; fi ;;
  checks)
    if [ -f move ]; then : > moved; fi
    /bin/cat checks.json; exit 1 ;;
esac
"#,
    );
    let path = format!("{}:/usr/bin:/bin", root.display());
    let _env = crate::test_env::EnvVarsGuard::apply(&[("PATH", Some(&path))]);
    let head = "a".repeat(40);
    let launch_pr = (7, Some(head.clone()));
    let launch = super::super::LaunchSeat {
        pr: Some(&launch_pr),
        head: Some(&head),
    };
    let seat = super::super::SeatState {
        head: Some(head.clone()),
        origin: crate::headless_background::OriginHead::At(head.clone()),
        uncommitted: Some(0),
    };
    let opened = super::super::Outcome::PrOpened {
        pr: 7,
        head: Some(head.clone()),
    };
    let write_view = |head: &str, mergeable: &str| {
        std::fs::write(
            root.join("view.json"),
            serde_json::json!({"state":"OPEN","headRefOid": head,"mergeable":mergeable})
                .to_string(),
        )
        .unwrap();
    };
    write_view(&head, "CONFLICTING");
    std::fs::write(root.join("checks.json"), "[]").unwrap();
    assert!(inspect(root, "BUG-2452", &launch, &opened, &seat)
        .unwrap()
        .unwrap()
        .2
        .contains("CONFLICTING"));
    // A transient PR lookup failure must not evade the check using NoPr.
    assert!(inspect(
        root,
        "BUG-2452",
        &launch,
        &super::super::Outcome::NoPr,
        &seat
    )
    .unwrap()
    .is_some());
    write_view(&head, "MERGEABLE");
    std::fs::write(
        root.join("checks.json"),
        r#"[{"name":"Build","bucket":"fail"}]"#,
    )
    .unwrap();
    assert!(inspect(root, "BUG-2452", &launch, &opened, &seat)
        .unwrap()
        .unwrap()
        .2
        .contains("Build"));
    std::fs::write(root.join("checks.json"), "[]").unwrap();
    assert!(inspect(root, "BUG-2452", &launch, &opened, &seat)
        .unwrap()
        .is_none());
    std::fs::write(
        root.join(".aida/review-verdicts/PR-7.json"),
        serde_json::json!({
            "verdict":"request-changes", "reviewed_sha":head, "findings":["F1: resource leak"]
        })
        .to_string(),
    )
    .unwrap();
    assert!(inspect(root, "BUG-2452", &launch, &opened, &seat)
        .unwrap()
        .unwrap()
        .2
        .contains("F1: resource leak"));
    // Project-required checks count even when branch protection declares none;
    // unrelated failures are informational for this required-check alarm.
    std::fs::remove_file(root.join(".aida/review-verdicts/PR-7.json")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n[review_watch]\nrequired_checks = [\"Build\"]\n",
    )
    .unwrap();
    crate::test_exec::write_executable(
        &root.join("gh"),
        r#"#!/bin/sh
case "$2" in
  view) /bin/cat view.json ;;
  checks)
    if [ "$4" = "--required" ]; then printf '[]'; else printf '[{"name":"Build","bucket":"fail"},{"name":"informational","bucket":"fail"}]'; fi; exit 1 ;;
esac
"#,
    );
    let diagnostic = inspect(root, "BUG-2452", &launch, &opened, &seat)
        .unwrap()
        .unwrap()
        .2;
    assert!(diagnostic.contains("Build"));
    assert!(!diagnostic.contains("informational"));
    std::fs::write(
        root.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    crate::test_exec::write_executable(
        &root.join("gh"),
        r#"#!/bin/sh
case "$2" in
  view)
    if [ -f unknown ]; then exit 1; fi
    if [ -f moved ]; then /bin/cat next.json; else /bin/cat view.json; fi ;;
  checks)
    if [ -f move ]; then : > moved; fi
    /bin/cat checks.json; exit 1 ;;
esac
"#,
    );
    // Either movement before the probe or during collection is progress.
    write_view(&"b".repeat(40), "CONFLICTING");
    assert!(inspect(root, "BUG-2452", &launch, &opened, &seat)
        .unwrap()
        .is_none());
    write_view(&head, "CONFLICTING");
    std::fs::write(
        root.join("next.json"),
        serde_json::json!({"state":"OPEN","headRefOid":"b".repeat(40),"mergeable":"CONFLICTING"})
            .to_string(),
    )
    .unwrap();
    std::fs::write(root.join("move"), "").unwrap();
    assert!(inspect(root, "BUG-2452", &launch, &opened, &seat)
        .unwrap()
        .is_none());
    std::fs::write(root.join("unknown"), "").unwrap();
    assert!(inspect(root, "BUG-2452", &launch, &opened, &seat).is_err());
}
