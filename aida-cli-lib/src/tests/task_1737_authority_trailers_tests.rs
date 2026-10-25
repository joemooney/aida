//! TASK-1737: advisor-gated status advances record `Aida-Authority`,
//! `Aida-Seat`, and `Aida-Grant` (8-character prefix only) git trailers on the
//! store commit from the single store-commit path; the history parser and
//! index tolerate missing and unknown trailers; store hooks accept them; and
//! `aida history events` surfaces the authority on `status_change` rows.
// trace:TASK-1737 | ai:antigravity

// trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
use crate::gate::seat_authority::test_support::mint_grant_for;
use crate::history::{
    collect_filtered_events_git, collect_filtered_events_sourced, events_json,
    parse_authority_trailers, render_events_toon, EventAuthority, EventKind, HistoryOpts,
    HistorySource,
};
use crate::history_cache;
use crate::test_env::{AmbientGuard, HERMETIC_TEST_USER};
use aida_core::db::DatabaseBackend;
use aida_core::models::{Requirement, RequirementStatus};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

fn setup_store() -> (tempfile::TempDir, PathBuf, aida_core::CachedGitBackend) {
    crate::gate::seat_authority::ensure_commit_authority_resolver_registered();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &[]);
    let store = crate::detect_distributed_store_from(root).unwrap();
    aida_core::git_ops::init(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "fixture", "fixture@localhost").unwrap();
    let backend = crate::test_env::home_fixture_cached_backend(
        &store,
        &aida_core::CachedGitBackend::default_cache_path(&store),
    )
    .unwrap();
    backend.save(&aida_core::RequirementsStore::new()).unwrap();
    (tmp, store, backend)
}

fn head_commit_message(store: &Path) -> String {
    let out = aida_core::git_ops::host_git_command(store)
        .args(["log", "-1", "--format=%B"])
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn default_history_opts() -> HistoryOpts {
    HistoryOpts {
        since: None,
        until: None,
        author_filter: None,
        id_filter: None,
        type_filter: None,
        status_changes_only: false,
        comments_only: false,
        shipped_only: false,
        opened_only: false,
        to_status: None,
        from_status: None,
        events_mode: true,
        limit: 50,
        max_commits: 500,
        max_commits_explicit: false,
        oneline: false,
        exclude_meta: false,
        archived_specs: Default::default(),
        archived_only_specs: None,
        deferred_specs: Default::default(),
        deferred_only_specs: None,
    }
}

/// Condition 1: every advisor-gated status advance (`aida edit`,
/// `update_spec_atomically`, and `update_atomically` / `save`) emits
/// `Aida-Authority`, `Aida-Seat`, and `Aida-Grant` (8-char prefix only) from
/// the single store-commit path, for both `grant` and `orchestrator`
/// authority, while ungated status changes do not.
// trace:TASK-1737 | ai:antigravity
#[test]
fn task_1737_store_commit_path_emits_authority_trailers_for_every_advisor_gated_advance() {
    let (tmp, store, backend) = setup_store();
    let root = tmp.path();

    for (spec_id, status) in [
        ("TASK-1", RequirementStatus::Draft),
        ("TASK-2", RequirementStatus::NeedsAttention),
        ("TASK-3", RequirementStatus::Rejected),
        ("TASK-4", RequirementStatus::Draft),
        ("TASK-5", RequirementStatus::Draft),
        ("TASK-6", RequirementStatus::Draft),
    ] {
        let mut req = Requirement::new(format!("title {spec_id}"), "desc".into());
        req.spec_id = Some(spec_id.into());
        req.status = status;
        backend.add_requirement(req).unwrap();
    }

    // 1. `aida edit TASK-1 --status approved` under a valid advisor grant.
    let grant_id = mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &[]);
    let prefix = &grant_id[..8];
    {
        let _ambient = AmbientGuard::hermetic_with_grant(root, Some("advisor"), &grant_id);
        let cli = crate::command_registry::try_parse_from([
            "aida", "edit", "TASK-1", "--status", "approved",
        ])
        .unwrap();
        crate::git_backend_cmd::handle_git_backend_command(&store, &cli.command).unwrap();
    }
    let msg1 = head_commit_message(&store);
    assert!(
        msg1.contains("Aida-Authority: grant"),
        "expected grant authority trailer in:\n{msg1}"
    );
    assert!(
        msg1.contains("Aida-Seat: advisor"),
        "expected advisor seat trailer in:\n{msg1}"
    );
    assert!(
        msg1.contains(&format!("Aida-Grant: {prefix}")),
        "expected 8-char grant prefix trailer in:\n{msg1}"
    );
    assert!(
        !msg1.contains(&grant_id),
        "full grant UUID must never appear in commit message (C2):\n{msg1}"
    );

    // 2. Non-edit path (`update_spec_atomically`: NeedsAttention -> InProgress)
    // under the same advisor grant also emits the trailers from `GitBackend`.
    {
        let _ambient = AmbientGuard::hermetic_with_grant(root, Some("advisor"), &grant_id);
        let target = backend
            .get_requirement_by_spec_id("TASK-2")
            .unwrap()
            .unwrap();
        backend
            .update_spec_atomically(&target, |r| {
                r.status = RequirementStatus::InProgress;
            })
            .unwrap();
    }
    let msg2 = head_commit_message(&store);
    assert!(msg2.contains("Aida-Authority: grant"), "{msg2}");
    assert!(msg2.contains("Aida-Seat: advisor"), "{msg2}");
    assert!(msg2.contains(&format!("Aida-Grant: {prefix}")), "{msg2}");
    assert!(!msg2.contains(&grant_id), "{msg2}");

    // 3. Ungated transition (`Approved -> InProgress` on TASK-1) does NOT emit
    // authority trailers even when the advisor grant is active.
    {
        let _ambient = AmbientGuard::hermetic_with_grant(root, Some("advisor"), &grant_id);
        let target = backend
            .get_requirement_by_spec_id("TASK-1")
            .unwrap()
            .unwrap();
        backend
            .update_spec_atomically(&target, |r| {
                r.status = RequirementStatus::InProgress;
            })
            .unwrap();
    }
    let msg_ungated = head_commit_message(&store);
    assert!(
        !msg_ungated.contains("Aida-Authority:"),
        "ungated status change must not emit authority trailers:\n{msg_ungated}"
    );

    // 4. Orchestrator-authorized advance (`Rejected -> Approved` on TASK-3 via
    // `update_atomically`) emits `Aida-Authority: orchestrator`.
    let run_token = uuid::Uuid::new_v4().to_string();
    let pid = std::process::id();
    let mut state = crate::drain_state::DrainState::new_single("TASK-3", &run_token, false);
    state.orchestrator_pid = pid;
    state.write(root).unwrap();
    crate::orchestrator::publish_run(
        root,
        &run_token,
        &crate::orchestrator::RunMarker {
            pid,
            spec: "TASK-3".into(),
            started_at: state.started_at,
            zen: false,
        },
    )
    .unwrap();
    {
        let _ambient = AmbientGuard::hermetic_with_seat_and(
            root,
            "implementer",
            &[],
            &[
                ("AIDA_AUTO_COMPLETE", Some("1")),
                ("AIDA_AUTO_COMPLETE_TOKEN", Some(run_token.as_str())),
            ],
        );
        backend
            .update_atomically(|s| {
                let req = s
                    .requirements
                    .iter_mut()
                    .find(|r| r.spec_id.as_deref() == Some("TASK-3"))
                    .unwrap();
                req.status = RequirementStatus::Approved;
            })
            .unwrap();
    }
    crate::orchestrator::remove_run(root, &run_token);
    let msg3 = head_commit_message(&store);
    assert!(
        msg3.contains("Aida-Authority: orchestrator"),
        "expected orchestrator authority trailer in:\n{msg3}"
    );
    assert!(
        !msg3.contains("Aida-Seat:"),
        "orchestrator authority must not carry Aida-Seat:\n{msg3}"
    );

    // 5. No-grant advance (e.g. human at a TTY with no grant): `aida edit`
    // without a grant is refused in non-TTY hermetic context, and a direct
    // store advance emits NO authority trailer.
    {
        let _ambient = AmbientGuard::hermetic(root, None);
        assert_eq!(
            crate::gate::seat_authority::resolve_commit_authority(&store),
            None
        );
        let cli = crate::command_registry::try_parse_from([
            "aida", "edit", "TASK-4", "--status", "approved",
        ])
        .unwrap();
        assert!(
            crate::git_backend_cmd::handle_git_backend_command(&store, &cli.command).is_err(),
            "roleless non-TTY edit must be refused"
        );
        let target = backend
            .get_requirement_by_spec_id("TASK-4")
            .unwrap()
            .unwrap();
        backend
            .update_spec_atomically(&target, |r| {
                r.status = RequirementStatus::Approved;
            })
            .unwrap();
    }
    let msg_no_grant = head_commit_message(&store);
    assert!(
        !msg_no_grant.contains("Aida-Authority:"),
        "no-grant advance must not emit authority trailers:\n{msg_no_grant}"
    );

    // 6. Non-advisor grant (`reviewer`): `aida edit --status approved` is
    // refused by the advisor gate, `resolve_commit_authority` returns `None`,
    // and a store advance never misattributes `Aida-Seat: reviewer`.
    let reviewer_grant = mint_grant_for(root, HERMETIC_TEST_USER, "reviewer", &["advisor"]);
    {
        let _ambient = AmbientGuard::hermetic_with_grant(root, Some("reviewer"), &reviewer_grant);
        assert_eq!(
            crate::gate::seat_authority::resolve_commit_authority(&store),
            None
        );
        let cli = crate::command_registry::try_parse_from([
            "aida", "edit", "TASK-5", "--status", "approved",
        ])
        .unwrap();
        assert!(
            crate::git_backend_cmd::handle_git_backend_command(&store, &cli.command).is_err(),
            "reviewer grant must not satisfy advisor gate"
        );
        let target = backend
            .get_requirement_by_spec_id("TASK-5")
            .unwrap()
            .unwrap();
        backend
            .update_spec_atomically(&target, |r| {
                r.status = RequirementStatus::Approved;
            })
            .unwrap();
    }
    let msg_reviewer = head_commit_message(&store);
    assert!(
        !msg_reviewer.contains("Aida-Authority:") && !msg_reviewer.contains("Aida-Seat:"),
        "non-advisor grant must never emit authority trailers:\n{msg_reviewer}"
    );

    // 7. Companion (`AIDA_ROLE_INSTANCE=companion`) even with an advisor grant:
    // refused by the advisor gate and emits no authority trailer.
    {
        let _ambient = AmbientGuard::hermetic_with_seat_and(
            root,
            "advisor",
            &[],
            &[("AIDA_ROLE_INSTANCE", Some("companion"))],
        );
        assert_eq!(
            crate::gate::seat_authority::resolve_commit_authority(&store),
            None
        );
        let cli = crate::command_registry::try_parse_from([
            "aida", "edit", "TASK-6", "--status", "approved",
        ])
        .unwrap();
        assert!(
            crate::git_backend_cmd::handle_git_backend_command(&store, &cli.command).is_err(),
            "companion instance must not satisfy advisor gate"
        );
        let target = backend
            .get_requirement_by_spec_id("TASK-6")
            .unwrap()
            .unwrap();
        backend
            .update_spec_atomically(&target, |r| {
                r.status = RequirementStatus::Approved;
            })
            .unwrap();
    }
    let msg_companion = head_commit_message(&store);
    assert!(
        !msg_companion.contains("Aida-Authority:"),
        "companion advance must never emit authority trailers:\n{msg_companion}"
    );
}

/// Condition 2: the history parser and index tolerate missing trailers and
/// unknown trailer keys, and `aida history events` surfaces `authority` on
/// `status_change` rows that carry the trailers while leaving old commits
/// unchanged.
// trace:TASK-1737 | ai:antigravity
#[test]
fn task_1737_history_parser_tolerates_missing_and_unknown_trailers_and_surfaces_authority() {
    // Unit-level trailer parser checks.
    assert_eq!(parse_authority_trailers(""), None);
    assert_eq!(
        parse_authority_trailers(
            "Signed-off-by: Alice <a@b>\x1fAida-Store: deadbeef\x1fFuture-Key: v"
        ),
        None
    );
    assert_eq!(
        parse_authority_trailers(
            "Future-Key: ignored\x1fAida-Authority: grant\x1fAida-Seat: advisor\x1fAida-Grant: 0123456789abcdef\x1fAnother-Unknown: 42"
        ),
        Some(EventAuthority {
            kind: "grant".into(),
            seat: Some("advisor".into()),
            grant: Some("01234567".into()),
        })
    );
    assert_eq!(
        parse_authority_trailers("Aida-Authority: orchestrator\nUnknown-Trailer: value"),
        Some(EventAuthority {
            kind: "orchestrator".into(),
            seat: None,
            grant: None,
        })
    );

    // End-to-end git walk + history_cache parity with:
    // - old commit (no trailers)
    // - commit with unknown trailer keys only
    // - commit with advisor grant trailers + unknown trailer key
    // - commit with orchestrator trailer
    let (tmp, store, backend) = setup_store();
    let root = tmp.path();
    let mut req = Requirement::new("History trailer test".into(), "desc".into());
    req.spec_id = Some("TASK-10".into());
    req.status = RequirementStatus::Draft;
    backend.add_requirement(req).unwrap();

    // Commit 1: advisor grant status advance (Draft -> Approved).
    let grant_id = mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &[]);
    let prefix = grant_id[..8].to_string();
    {
        let _ambient = AmbientGuard::hermetic_with_grant(root, Some("advisor"), &grant_id);
        let target = backend
            .get_requirement_by_spec_id("TASK-10")
            .unwrap()
            .unwrap();
        backend
            .update_spec_atomically(&target, |r| {
                r.status = RequirementStatus::Approved;
            })
            .unwrap();
    }

    // Commit 2: manual commit with unknown + folded multi-line trailers only (Approved -> Rejected).
    let yaml_path = store.join("objects/TASK/000/TASK-10.yaml");
    let body = std::fs::read_to_string(&yaml_path)
        .unwrap()
        .replace("status: Approved", "status: Rejected");
    std::fs::write(&yaml_path, body).unwrap();
    let out = aida_core::git_ops::host_git_command(&store)
        .args([
            "commit",
            "-am",
            "update TASK-10\n\nSigned-off-by: Fixture <fixture@localhost>\nAida-Unknown-Key: custom-value\n \tcontinued-on-next-line",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());

    // Commit 3: manual commit with folded orchestrator trailer + folded unknown
    // trailer on a gated reopen (Rejected -> Done).
    let body = std::fs::read_to_string(&yaml_path)
        .unwrap()
        .replace("status: Rejected", "status: Done");
    std::fs::write(&yaml_path, body).unwrap();
    let out = aida_core::git_ops::host_git_command(&store)
        .args([
            "commit",
            "-am",
            "update TASK-10\n\nAida-Future: 1\n \tfolded\nAida-Authority:\n orchestrator",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());

    let mut opts = default_history_opts();
    opts.id_filter = Some("TASK-10".into());
    opts.status_changes_only = true;

    let (git_events, _, _) = collect_filtered_events_git(&store, &opts).unwrap();
    let db_path = root.join(".aida/history.sqlite");
    history_cache::rebuild_full_at(&store, &db_path).unwrap();
    let (cached_events, _, _, _) = collect_filtered_events_sourced(&store, &opts).unwrap();
    assert_eq!(
        git_events, cached_events,
        "indexed history events must match raw git walk byte-for-byte"
    );
    assert_eq!(git_events.len(), 3);

    // Newest first:
    // [0] Rejected -> Done (orchestrator)
    // [1] Approved -> Rejected (no authority; unknown trailers ignored)
    // [2] Draft -> Approved (grant: advisor, prefix)
    assert_eq!(
        git_events[0].kind,
        EventKind::StatusChange {
            from: "Rejected".into(),
            to: "Done".into(),
            authority: Some(EventAuthority {
                kind: "orchestrator".into(),
                seat: None,
                grant: None,
            }),
        }
    );
    assert_eq!(
        git_events[1].kind,
        EventKind::StatusChange {
            from: "Approved".into(),
            to: "Rejected".into(),
            authority: None,
        }
    );
    assert_eq!(
        git_events[2].kind,
        EventKind::StatusChange {
            from: "Draft".into(),
            to: "Approved".into(),
            authority: Some(EventAuthority {
                kind: "grant".into(),
                seat: Some("advisor".into()),
                grant: Some(prefix.clone()),
            }),
        }
    );

    // Verify `aida history events` TOON and JSON projections include authority on
    // rows with trailers and omit it on rows without trailers.
    let toon = render_events_toon(&git_events);
    assert!(
        toon.contains(&format!("authority: grant (advisor, {prefix})")),
        "TOON must show grant authority: {toon}"
    );
    assert!(
        toon.contains("authority: orchestrator"),
        "TOON must show orchestrator authority: {toon}"
    );

    let json = events_json(
        &git_events,
        false,
        &HistorySource::GitWalk { fallback: false },
    );
    let rows = json["events"].as_array().unwrap();
    assert_eq!(rows[0]["authority"]["kind"], "orchestrator");
    assert!(rows[1].get("authority").is_none());
    assert!(rows[1]["detail"].get("authority").is_none());
    assert_eq!(rows[2]["authority"]["kind"], "grant");
    assert_eq!(rows[2]["authority"]["seat"], "advisor");
    assert_eq!(rows[2]["authority"]["grant"], prefix);
    assert_eq!(rows[2]["detail"]["authority"]["grant"], prefix);
}

/// A commit's trailers describe the whole commit, so in a batch commit that
/// makes one gated advance and one ungated flip, only the gated row carries
/// the authority; and a tab inside a trailer value does not shift into the
/// indexed subject.
// trace:TASK-1737 | ai:claude
#[test]
fn task_1737_batch_commit_attributes_authority_only_to_the_gated_row() {
    let (tmp, store, backend) = setup_store();
    let root = tmp.path();
    for (spec_id, status) in [
        ("TASK-20", RequirementStatus::Draft),
        ("TASK-21", RequirementStatus::InProgress),
    ] {
        let mut req = Requirement::new(format!("title {spec_id}"), "desc".into());
        req.spec_id = Some(spec_id.into());
        req.status = status;
        backend.add_requirement(req).unwrap();
    }

    let grant_id = mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &[]);
    {
        let _ambient = AmbientGuard::hermetic_with_grant(root, Some("advisor"), &grant_id);
        let mut store_data = backend.load().unwrap();
        for req in store_data.requirements.iter_mut() {
            match req.spec_id.as_deref() {
                Some("TASK-20") => req.status = RequirementStatus::Approved,
                Some("TASK-21") => req.status = RequirementStatus::Done,
                _ => {}
            }
        }
        backend.save(&store_data).unwrap();
    }
    assert!(head_commit_message(&store).contains("Aida-Authority: grant"));

    let mut opts = default_history_opts();
    opts.status_changes_only = true;
    let (git_events, _, _) = collect_filtered_events_git(&store, &opts).unwrap();
    let db_path = root.join(".aida/history.sqlite");
    history_cache::rebuild_full_at(&store, &db_path).unwrap();
    let (cached_events, _, _, _) = collect_filtered_events_sourced(&store, &opts).unwrap();
    assert_eq!(git_events, cached_events);

    let authority_of = |spec: &str| {
        git_events
            .iter()
            .find(|e| e.spec_id == spec)
            .map(|e| match &e.kind {
                EventKind::StatusChange { authority, .. } => authority.clone(),
                other => panic!("unexpected event {other:?}"),
            })
            .unwrap_or_else(|| panic!("no status change for {spec}"))
    };
    assert_eq!(
        authority_of("TASK-20").map(|a| a.kind),
        Some("grant".to_string())
    );
    assert_eq!(authority_of("TASK-21"), None);

    // A literal tab inside a trailer value stays in the trailer field on both
    // the raw walk and the index (gated reopen Rejected -> Approved).
    let yaml_path = store.join("objects/TASK/000/TASK-21.yaml");
    for (from, to, msg) in [
        ("status: Done", "status: Rejected", "reject TASK-21"),
        (
            "status: Rejected",
            "status: Approved",
            "reopen TASK-21\n\nAida-Authority: grant\nAida-Seat: adv\tisor",
        ),
    ] {
        let body = std::fs::read_to_string(&yaml_path)
            .unwrap()
            .replace(from, to);
        std::fs::write(&yaml_path, body).unwrap();
        let out = aida_core::git_ops::host_git_command(&store)
            .args(["commit", "-am", msg])
            .output()
            .unwrap();
        assert!(out.status.success());
    }
    history_cache::rebuild_full_at(&store, &db_path).unwrap();
    let (git_events, _, _) = collect_filtered_events_git(&store, &opts).unwrap();
    let (cached_events, _, _, _) = collect_filtered_events_sourced(&store, &opts).unwrap();
    assert_eq!(git_events, cached_events);
    assert_eq!(
        git_events[0].kind,
        EventKind::StatusChange {
            from: "Rejected".into(),
            to: "Approved".into(),
            authority: Some(EventAuthority {
                kind: "grant".into(),
                seat: Some("adv\tisor".into()),
                grant: None,
            }),
        }
    );
}

/// Condition 3: confirm the store's `commit-msg` and `prepare-commit-msg`
/// (provenance/store-pair) hooks accept commits carrying `Aida-Authority`,
/// `Aida-Seat`, and `Aida-Grant` trailers without error or corruption.
// trace:TASK-1737 | ai:antigravity
// The store hooks are shell scripts run through `bash`.
#[cfg(unix)]
#[test]
fn task_1737_store_commit_msg_and_provenance_hooks_accept_authority_trailers() {
    let (tmp, store, _backend) = setup_store();
    let root = tmp.path();

    // 1. Run `prepare-commit-msg` (`hooks/aida-store-pair.sh`) and `commit-msg`
    // (`hooks/aida-commit-msg`) directly against a commit message carrying the
    // authority trailers.
    let commit_msg_hook = *aida_core::templates::EMBEDDED_TEMPLATES
        .get("hooks/aida-commit-msg")
        .unwrap();
    let store_pair_hook = *aida_core::templates::EMBEDDED_TEMPLATES
        .get("hooks/aida-store-pair.sh")
        .unwrap();

    let hook_dir = root.join("hook-bin");
    std::fs::create_dir_all(&hook_dir).unwrap();
    let commit_msg_path = hook_dir.join("commit-msg");
    let store_pair_path = hook_dir.join("prepare-commit-msg");
    crate::test_exec::write_executable(&commit_msg_path, commit_msg_hook);
    crate::test_exec::write_executable(&store_pair_path, store_pair_hook);

    // Initialise the project root as a git repo so `aida-store-pair.sh` also
    // exercises its `Aida-Store` trailer-insertion path alongside our trailers.
    aida_core::git_ops::init(root).unwrap();
    aida_core::git_ops::configure_user(root, "fixture", "fixture@localhost").unwrap();

    let store_msg_file = root.join("STORE_COMMIT_EDITMSG");
    std::fs::write(
        &store_msg_file,
        "update TASK-1\n\nAida-Authority: grant\nAida-Seat: advisor\nAida-Grant: 01234567\n",
    )
    .unwrap();

    let prep_out = ProcessCommand::new("bash")
        .arg(&store_pair_path)
        .arg(&store_msg_file)
        .arg("message")
        .current_dir(&store)
        .env("AIDA_STORE_WRITE_GUARD", "1")
        .output()
        .unwrap();
    assert!(
        prep_out.status.success(),
        "prepare-commit-msg failed: {}",
        String::from_utf8_lossy(&prep_out.stderr)
    );

    let msg_out = ProcessCommand::new("bash")
        .arg(&commit_msg_path)
        .arg(&store_msg_file)
        .current_dir(&store)
        .env("AIDA_STORE_WRITE_GUARD", "1")
        .output()
        .unwrap();
    assert!(
        msg_out.status.success(),
        "commit-msg hook rejected store commit with authority trailers: {}{}",
        String::from_utf8_lossy(&msg_out.stdout),
        String::from_utf8_lossy(&msg_out.stderr)
    );

    // Also run both hooks in strict mode on a conventional commit carrying the
    // authority trailers in the project root (where `aida-store-pair.sh` appends
    // `Aida-Store: <sha>` via `git interpret-trailers`).
    let code_msg_file = root.join("CODE_COMMIT_EDITMSG");
    std::fs::write(
        &code_msg_file,
        "[AI:codex] chore(store): advance status (TASK-1)\n\nAida-Authority: grant\nAida-Seat: advisor\nAida-Grant: 01234567\n",
    )
    .unwrap();
    let prep_code = ProcessCommand::new("bash")
        .arg(&store_pair_path)
        .arg(&code_msg_file)
        .arg("message")
        .current_dir(root)
        .output()
        .unwrap();
    assert!(prep_code.status.success());

    let strict_code = ProcessCommand::new("bash")
        .arg(&commit_msg_path)
        .arg(&code_msg_file)
        .current_dir(root)
        .env("AIDA_COMMIT_STRICT", "true")
        .output()
        .unwrap();
    assert!(
        strict_code.status.success(),
        "strict commit-msg hook rejected commit with authority trailers: {}{}",
        String::from_utf8_lossy(&strict_code.stdout),
        String::from_utf8_lossy(&strict_code.stderr)
    );

    let resulting_msg = std::fs::read_to_string(&code_msg_file).unwrap();
    assert!(
        resulting_msg.contains("Aida-Authority: grant"),
        "{resulting_msg}"
    );
    assert!(
        resulting_msg.contains("Aida-Seat: advisor"),
        "{resulting_msg}"
    );
    assert!(
        resulting_msg.contains("Aida-Grant: 01234567"),
        "{resulting_msg}"
    );
    assert!(resulting_msg.contains("Aida-Store: "), "{resulting_msg}");
}
