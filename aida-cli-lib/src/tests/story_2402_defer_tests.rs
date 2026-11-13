//! S2: explicit typed deferrals and question parking preserve their evidence.
// trace:STORY-2402 | ai:codex
use super::*;
use aida_core::Requirement;

#[test]
fn story2402_s2_conditions_validate_and_canonicalize() {
    for (raw, canonical) in [
        ("spec:story-12=completed", "spec:STORY-12=completed"),
        ("pr:0012=merged", "pr:12=merged"),
        ("release:>=0.17", "release:>=0.17"),
        (" date:2026-12-01 ", "date:2026-12-01"),
        ("query:tag=ready count>=5", "query:tag=ready count>=5"),
    ] {
        assert_eq!(
            crate::condition_cmd::canonical_condition(Some(raw), None)
                .unwrap()
                .as_deref(),
            Some(canonical)
        );
    }
    for raw in [
        "wait for demand",
        "",
        "manual:",
        "pr:0=merged",
        "date:tomorrow",
    ] {
        assert!(
            crate::condition_cmd::canonical_condition(Some(raw), Some("explanation")).is_err(),
            "{raw}"
        );
    }
    for reason in [None, Some(""), Some("  ")] {
        assert!(
            crate::condition_cmd::canonical_condition(Some("manual:joe"), reason)
                .unwrap_err()
                .to_string()
                .contains("--reason")
        );
    }
    assert_eq!(
        crate::condition_cmd::canonical_condition(Some("manual:joe"), Some("decision pending"))
            .unwrap()
            .as_deref(),
        Some("manual:joe")
    );
    assert_eq!(
        crate::condition_cmd::canonical_condition(None, None).unwrap(),
        None
    );
}

#[test]
fn story2402_s2_refusal_precedes_writes_and_redefer_retains_evidence() {
    // Allocation metadata must stay private under a permissive host umask.
    // trace:STORY-2402 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    unsafe {
        libc::umask(0o077);
    }
    let dir = tempfile::tempdir().unwrap();
    let init = crate::test_exec::output(
        crate::test_git::git_command()
            .current_dir(dir.path())
            .args(["init", "--initial-branch=main", "--quiet"]),
    )
    .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(dir.path(), "implementer", &[]);
    let store_path = dir.path().join(".aida-store");
    std::fs::create_dir_all(&store_path).unwrap();
    aida_core::work_binding::bootstrap(&store_path)
        .unwrap()
        .unwrap();
    let backend =
        aida_core::CachedGitBackend::open(&store_path, &dir.path().join("cache.db")).unwrap();
    let mut req = Requirement::new("typed deferral fixture".into(), "".into());
    req.status = aida_core::RequirementStatus::Approved;
    let req = backend.add_requirement(req).unwrap();
    let id = req.id.to_string();
    let read = || backend.get_requirement(&req.id).unwrap().unwrap();
    let before = serde_yaml::to_string(&read()).unwrap();
    assert!(defer_single(&id, Some("when ready"), None, &backend, &store_path).is_err());
    assert!(defer_single(&id, Some("manual:joe"), None, &backend, &store_path).is_err());
    assert_eq!(serde_yaml::to_string(&read()).unwrap(), before);
    defer_single(
        &id,
        Some("spec:story-12=completed"),
        None,
        &backend,
        &store_path,
    )
    .unwrap();
    let deferred = read();
    assert!(deferred.deferred);
    assert_eq!(
        deferred.deferred_until.as_deref(),
        Some("spec:STORY-12=completed")
    );
    assert_eq!(deferred.status, req.status);
    defer_single(&id, None, None, &backend, &store_path).unwrap();
    assert_eq!(read().deferred_at, deferred.deferred_at);
    assert_eq!(read().deferred_until, deferred.deferred_until);
    // Legacy prose is retained by a bare re-deferral pending S3 migration.
    let mut legacy = read();
    legacy.deferred_until = Some("after the next release".into());
    backend.update_requirement(&legacy).unwrap();
    defer_single(&id, None, None, &backend, &store_path).unwrap();
    assert_eq!(read().deferred_until, legacy.deferred_until);
    assert_eq!(read().deferred_at, legacy.deferred_at);
}

#[test]
fn story2402_s2_question_parking_records_typed_or_manual_evidence() {
    let _identity = crate::test_env::EnvVarGuard::set("AIDA_USER", "question-resolver");
    for verb in ["defer", "park", "hold", "passive", "passive-only", "shelve"] {
        for consequence in [
            "spec:story-12=completed",
            "wait for demand",
            "",
            "pr:broken",
        ] {
            let mut req = Requirement::new("question fixture".into(), "".into());
            let timestamp = chrono::Utc::now() - chrono::Duration::days(2);
            req.deferred_at = Some(timestamp);
            req.deferred_until = Some("date:2026-01-01".into());
            let applied =
                crate::questions::apply_resolution_token(&mut req, verb, "Park", consequence);
            assert!(req.deferred && applied.keeps_parked && !applied.binds_acceptance);
            assert_eq!(req.deferred_at, Some(timestamp));
            let parsed = req
                .deferred_until
                .as_ref()
                .unwrap()
                .parse::<aida_core::condition::Condition>()
                .unwrap();
            if consequence.starts_with("spec:") {
                assert_eq!(parsed.to_string(), "spec:STORY-12=completed");
                assert!(req.comments.is_empty());
            } else if consequence.is_empty() {
                assert_eq!(parsed.to_string(), "date:2026-01-01");
                assert!(req.comments.is_empty());
            } else {
                assert_eq!(
                    parsed,
                    aida_core::condition::Condition::Manual {
                        who: crate::current_user_id(None)
                    }
                );
                assert!(req.comments[0].content.ends_with(consequence));
                assert_eq!(req.comments[0].author, crate::current_user_id(None));
            }
        }
    }
}

#[test]
fn story2402_s2_question_parking_invalid_identity_uses_parseable_manual() {
    for identity in ["Joe Mooney", ""] {
        let _identity = crate::test_env::EnvVarGuard::set("AIDA_USER", identity);
        for consequence in ["", "wait for demand"] {
            let mut req = Requirement::new("question fixture".into(), "".into());
            crate::questions::apply_resolution_token(&mut req, "park", "Park", consequence);
            let parsed = req
                .deferred_until
                .as_ref()
                .unwrap()
                .parse::<aida_core::condition::Condition>()
                .unwrap();
            assert_eq!(parsed.to_string(), "manual:operator");
            assert_eq!(req.comments.len(), usize::from(!consequence.is_empty()));
            if let Some(comment) = req.comments.first() {
                assert_eq!(comment.author, identity);
            }
        }
    }
}

#[test]
fn story2402_s2_display_handles_typed_and_legacy_triggers() {
    let mut req = Requirement::new("fixture".into(), "".into());
    for (raw, expected) in [
        ("spec:story-12=completed", "spec:STORY-12=completed"),
        ("when ready", "when ready"),
    ] {
        req.deferred_until = Some(raw.into());
        assert_eq!(
            crate::status_display::deferred_revisit_trigger(&req).as_deref(),
            Some(expected)
        );
    }
}

/// Pin the changed command help and catalog alongside the new condition gate.
#[test]
fn story2402_s2_defer_help_matches_snapshot() {
    let root = crate::TEST_CLI
        .built_command()
        .term_width(100)
        .color(clap::ColorChoice::Never);
    let help = root
        .try_get_matches_from(["aida", "defer", "--help"])
        .unwrap_err()
        .render()
        .to_string();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/golden/cli_help_snapshot.txt");
    let golden = std::fs::read_to_string(&path).unwrap();
    let (prefix, rest) = golden.split_once("===== aida defer\n").unwrap();
    let (expected, suffix) = rest.split_once("===== aida undefer\n").unwrap();
    if std::env::var_os("AIDA_UPDATE_SNAPSHOTS").is_some() {
        let nested = expected
            .find("===== aida defer resurface\n")
            .map(|at| &expected[at..])
            .unwrap_or("");
        let updated =
            format!("{prefix}===== aida defer\n{help}{nested}===== aida undefer\n{suffix}");
        let (body, _) = updated
            .split_once("===== catalog (hidden included)\n")
            .unwrap();
        let catalog =
            serde_json::to_string_pretty(&crate::help_catalog::catalog_rows(true)).unwrap();
        std::fs::write(
            path,
            format!("{body}===== catalog (hidden included)\n{catalog}\n"),
        )
        .unwrap();
    } else {
        // Nested subcommand help follows the parent in the full snapshot.
        assert_eq!(
            help,
            expected
                .split("===== aida defer resurface\n")
                .next()
                .unwrap()
        );
    }
}
