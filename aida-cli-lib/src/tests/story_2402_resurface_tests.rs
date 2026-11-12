//! Hourly trigger sweep: holds are review work, never automatic authority.
// trace:STORY-2402 | ai:codex
use super::*;
use aida_core::condition::{QueryFilter, Version};
use aida_core::RequirementStatus;

struct Facts;
impl ConditionFacts for Facts {
    fn spec_status(&self, id: &str) -> Result<Option<RequirementStatus>, String> {
        if id == "TASK-999" {
            return Err("spec store read failed".into());
        }
        Ok((id == "TASK-1").then_some(RequirementStatus::Completed))
    }
    fn pr_merged(&self, _: u64) -> Option<bool> {
        None
    }
    fn latest_release(&self) -> Result<Option<Version>, String> {
        Err("offline".into())
    }
    fn today(&self) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 11, 6).unwrap()
    }
    fn count(&self, _: &[QueryFilter]) -> Result<u64, String> {
        Ok(3)
    }
}
fn parked(trigger: &str) -> Requirement {
    let mut row = Requirement::new("parked".into(), "".into());
    row.status = RequirementStatus::Approved;
    row.deferred = true;
    row.deferred_at = Some(Utc::now() - chrono::Duration::days(15));
    row.deferred_until = Some(trigger.into());
    row
}

#[test]
fn story2402_s4_verdicts_age_and_holds() {
    let now = Utc::now();
    for (trigger, expected) in [
        ("spec:TASK-1=completed", Action::Resurface),
        ("spec:TASK-1=approved", Action::Keep),
        ("date:2026-11-06", Action::Resurface),
        ("date:2026-11-07", Action::Keep),
        ("query:tag=ready count>=3", Action::Resurface),
        ("manual:joe", Action::Human),
        ("pr:42=merged", Action::Keep),
        ("release:>=0.17", Action::Keep),
    ] {
        assert_eq!(
            action(&parked(trigger), &Facts, now, 14).unwrap().0,
            expected,
            "{trigger}"
        );
    }
    let mut row = parked("manual:joe");
    for at in [None, Some(now), Some(now - chrono::Duration::days(14))] {
        row.deferred_at = at;
        assert_eq!(action(&row, &Facts, now, 14).unwrap().0, Action::Keep);
    }
    row.deferred_at = Some(now - chrono::Duration::days(15));
    assert_eq!(action(&row, &Facts, now, 16).unwrap().0, Action::Keep);
    row.deferred_until = Some("date:2026-01-01".into());
    row.deferred_reason = Some("operator decision".into());
    assert_eq!(action(&row, &Facts, now, 14).unwrap().0, Action::Human);
    row.deferred_reason = None;
    row.tags.insert("operator-held".into());
    assert_eq!(action(&row, &Facts, now, 14).unwrap().0, Action::Human);
    row.archived = true;
    row.deferred_until = Some("invalid".into());
    assert_eq!(action(&row, &Facts, now, 14).unwrap().0, Action::Keep);
    for trigger in [
        "invalid",
        "Codex ships command-backed status lines",
        "spec:",
        "date:bad",
    ] {
        let (outcome, evidence, migration) = action(&parked(trigger), &Facts, now, 14).unwrap();
        assert_eq!(outcome, Action::Keep);
        assert!(evidence.contains("unknown"));
        assert!(migration);
    }
    assert!(action(&parked("spec:TASK-999=completed"), &Facts, now, 14).is_err());
}

#[test]
fn story2402_s4_changed_rows_are_not_overwritten() {
    let row = parked("date:2026-01-01");
    let mut changed = row.clone();
    changed.deferred_reason = Some("a newly placed hold".into());
    assert!(!clear_if_unchanged(&mut changed, &row, "met", Utc::now()).unwrap());
    assert!(changed.deferred && changed.comments.is_empty());
}

#[test]
fn story2402_s4_persistence_dry_run_routing_and_repeat() {
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
    assert!(init.status.success());
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(dir.path(), "implementer", &[]);
    let path = dir.path().join(".aida-store");
    std::fs::create_dir_all(&path).unwrap();
    aida_core::work_binding::bootstrap(&path).unwrap().unwrap();
    let backend = CachedGitBackend::open(&path, &dir.path().join("cache.db")).unwrap();
    let met = backend.add_requirement(parked("date:2026-01-01")).unwrap();
    let manual = backend.add_requirement(parked("manual:joe")).unwrap();
    let unknown = backend.add_requirement(parked("pr:42=merged")).unwrap();
    let mut hold = parked("date:2026-01-01");
    hold.deferred_reason = Some("human must decide".into());
    hold.tags.insert("operator-held".into());
    let hold = backend.add_requirement(hold).unwrap();
    let rows = vec![met.clone(), manual.clone(), unknown.clone(), hold.clone()];
    let now = Utc::now();
    sweep(&backend, &rows, &Facts, now, true, 14).unwrap();
    assert!(backend.get_requirement(&met.id).unwrap().unwrap().deferred);
    assert!(backend.queue_list("role:human", true).unwrap().is_empty());
    sweep(&backend, &rows, &Facts, now, false, 14).unwrap();
    let result = backend.get_requirement(&met.id).unwrap().unwrap();
    assert!(!result.deferred);
    assert!(result.deferred_at.is_none() && result.deferred_until.is_none());
    assert_eq!(result.status, met.status);
    assert_eq!(result.comments.len(), 1);
    assert!(result.comments[0].content.contains("date:2026-01-01: met"));
    for row in [&manual, &hold, &unknown] {
        assert_eq!(
            serde_yaml::to_string(&backend.get_requirement(&row.id).unwrap().unwrap()).unwrap(),
            serde_yaml::to_string(row).unwrap()
        );
    }
    let queue = backend.queue_list("role:human", true).unwrap();
    assert_eq!(queue.len(), 2);
    let human = crate::human_routed_spec_ids(dir.path(), &backend.load().unwrap());
    for row in [&manual, &hold] {
        assert!(human.contains(&row.spec_id.as_ref().unwrap().to_ascii_uppercase()));
    }
    assert!(queue.iter().all(|e| e.for_role.as_deref() == Some("human")));
    sweep(&backend, &rows, &Facts, now, false, 14).unwrap();
    assert_eq!(backend.queue_list("role:human", true).unwrap(), queue);
    assert_eq!(
        backend
            .get_requirement(&met.id)
            .unwrap()
            .unwrap()
            .comments
            .len(),
        1
    );
    // Queue removal is not hold clearance: unchanged held work is routed again.
    backend.queue_remove("role:human", &hold.id).unwrap();
    sweep(&backend, &[hold.clone()], &Facts, now, false, 14).unwrap();
    assert!(backend
        .queue_list("role:human", true)
        .unwrap()
        .iter()
        .any(|entry| entry.requirement_id == hold.id));
    assert!(backend.get_requirement(&hold.id).unwrap().unwrap().deferred);
    // Real evaluation errors fail while independent valid rows still progress.
    let invalid = backend
        .add_requirement(parked("spec:TASK-999=completed"))
        .unwrap();
    let next = backend.add_requirement(parked("date:2026-01-01")).unwrap();
    assert!(sweep(&backend, &[invalid, next.clone()], &Facts, now, false, 14).is_err());
    assert!(!backend.get_requirement(&next.id).unwrap().unwrap().deferred);
}

#[test]
fn story2402_s4_cli_and_schedule() {
    for args in [
        vec!["aida", "defer", "resurface"],
        vec![
            "aida",
            "defer",
            "resurface",
            "--dry-run",
            "--manual-age-days",
            "7",
        ],
        vec!["aida", "defer", "TASK-1", "--until", "date:2026-12-01"],
    ] {
        crate::TEST_CLI.try_parse_from(args).unwrap();
    }
    assert!(crate::TEST_CLI.try_parse_from(["aida", "defer"]).is_err());
    assert!(crate::TEST_CLI
        .try_parse_from(["aida", "defer", "resurface", "--reason", "bypass"])
        .is_err());
    let value: toml::Value = toml::from_str(include_str!("../../../.aida/config.toml")).unwrap();
    let jobs = value["schedule"]["jobs"].as_array().unwrap();
    let job = jobs
        .iter()
        .find(|j| j["name"].as_str() == Some("defer-resurface"))
        .unwrap();
    assert_eq!(job["every"].as_str(), Some("1h"));
    assert_eq!(job["command"].as_str(), Some("defer resurface"));
    assert_eq!(job["enabled"].as_bool(), Some(true));
    let route = jobs
        .iter()
        .find(|j| j["name"].as_str() == Some("defer-resurface-route"))
        .unwrap();
    assert_eq!(
        route["on"][0].as_str(),
        Some("CronJobFailed:defer-resurface")
    );
    assert_eq!(route["seats"][0].as_str(), Some("advisor"));
    assert_eq!(route["enabled"].as_bool(), Some(true));
}

// Exercise production's raw canonical snapshot against the targeted backend reader.
#[test]
fn story2402_s4_canonical_snapshot_and_legacy_only_run() {
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
    assert!(init.status.success());
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(dir.path(), "implementer", &[]);
    let path = dir.path().join(".aida-store");
    std::fs::create_dir_all(&path).unwrap();
    aida_core::work_binding::bootstrap(&path).unwrap().unwrap();
    let backend = CachedGitBackend::open(&path, &dir.path().join("cache.db")).unwrap();
    for trigger in [
        "Codex ships command-backed status lines",
        "upstream release ships",
        "advisor decides",
        "spec:",
        "date:bad",
    ] {
        backend.add_requirement(parked(trigger)).unwrap();
    }
    let before = crate::condition_cmd::read_store(dir.path(), Some(&path)).unwrap();
    let bytes_before: Vec<_> = before
        .requirements
        .iter()
        .map(|r| {
            let object = aida_core::object_store::object_path(
                &path.join("objects"),
                r.spec_id.as_deref().unwrap(),
            )
            .unwrap();
            (object.clone(), std::fs::read(object).unwrap())
        })
        .collect();
    // Only unparseable deferrals: success, exact count, no object/queue writes.
    assert_eq!(
        sweep(
            &backend,
            &before.requirements,
            &Facts,
            Utc::now(),
            false,
            14
        )
        .unwrap(),
        5
    );
    run(dir.path(), &path, &backend, false, 14).unwrap();
    for (object, bytes) in bytes_before {
        assert_eq!(std::fs::read(object).unwrap(), bytes);
    }
    assert!(backend.queue_list("role:human", true).unwrap().is_empty());
    let met = backend.add_requirement(parked("date:2000-01-01")).unwrap();
    run(dir.path(), &path, &backend, false, 14).unwrap();
    let result = backend.get_requirement(&met.id).unwrap().unwrap();
    assert!(!result.deferred);
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.status, met.status);
}

// The production snapshot and per-spec writer must preserve concurrent holds,
// while an unknown dependency stays parked and independent rows progress.
// trace:STORY-2402 | ai:codex
#[test]
fn story2402_s4_canonical_stale_snapshot_and_evaluation_error() {
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
    assert!(init.status.success());
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(dir.path(), "implementer", &[]);
    let path = dir.path().join(".aida-store");
    std::fs::create_dir_all(&path).unwrap();
    aida_core::work_binding::bootstrap(&path).unwrap().unwrap();
    let backend = CachedGitBackend::open(&path, &dir.path().join("cache.db")).unwrap();
    let held = backend.add_requirement(parked("date:2000-01-01")).unwrap();
    let snapshot = crate::condition_cmd::read_store(dir.path(), Some(&path)).unwrap();
    backend
        .update_spec_atomically(&held, |current| {
            current.deferred_reason = Some("concurrent operator hold".into());
            current.tags.insert("operator-held".into());
        })
        .unwrap();
    let before = backend.get_requirement(&held.id).unwrap().unwrap();
    sweep(
        &backend,
        &snapshot.requirements,
        &Facts,
        Utc::now(),
        false,
        14,
    )
    .unwrap();
    assert_eq!(
        serde_yaml::to_value(backend.get_requirement(&held.id).unwrap().unwrap()).unwrap(),
        serde_yaml::to_value(&before).unwrap()
    );
    assert!(backend.queue_list("role:human", true).unwrap().is_empty());

    let unknown = backend
        .add_requirement(parked("spec:TASK-404=completed"))
        .unwrap();
    let met = backend.add_requirement(parked("date:2000-01-01")).unwrap();
    run(dir.path(), &path, &backend, false, 14).unwrap();
    assert!(
        backend
            .get_requirement(&unknown.id)
            .unwrap()
            .unwrap()
            .deferred
    );
    assert!(!backend.get_requirement(&met.id).unwrap().unwrap().deferred);
    assert!(backend.get_requirement(&held.id).unwrap().unwrap().deferred);
    assert!(backend
        .queue_list("role:human", true)
        .unwrap()
        .iter()
        .any(|entry| entry.requirement_id == held.id));
}

// Handoff/recurrence writers share the same persisted deferrals and timer.
#[test]
fn story2402_s5_recur_conditions_resurface_and_manual_routes() {
    // Allocation rewrites its authority files, so a private umask must cover
    // every write. Run in the shared exact-test child before changing it.
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
    assert!(init.status.success());
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(dir.path(), "advisor", &[]);
    let path = dir.path().join(".aida-store");
    std::fs::create_dir_all(&path).unwrap();
    aida_core::work_binding::bootstrap(&path).unwrap().unwrap();
    let backend = CachedGitBackend::open(&path, &dir.path().join("cache.db")).unwrap();
    let mut row = Requirement::new("finding".into(), "".into());
    row.tags.insert("from-advisor:STORY-1".into());
    let row = backend.add_requirement(row).unwrap();
    let id = row.spec_id.as_ref().unwrap();
    for (until, reason) in [
        ("when ready", None),
        ("manual:joe", None),
        ("manual:joe", Some("  ")),
    ] {
        assert!(crate::findings_gate::handle_findings_recur(
            &backend,
            &path,
            id,
            None,
            Some(until),
            reason
        )
        .is_err());
        let current = backend.get_requirement(&row.id).unwrap().unwrap();
        assert!(!current.deferred);
        assert_eq!(
            crate::findings::finding_recurrence(&current.tags.iter().cloned().collect::<Vec<_>>()),
            1
        );
    }
    let recurrence_pushes = crate::store_push_pull::RECURRENCE_PUSH_CALLS.with(|calls| calls.get());
    crate::findings_gate::handle_findings_recur(
        &backend,
        &path,
        id,
        Some("seen again"),
        Some("date:2026-01-01"),
        None,
    )
    .unwrap();
    assert_eq!(
        crate::store_push_pull::RECURRENCE_PUSH_CALLS.with(|calls| calls.get()),
        recurrence_pushes + 1,
        "saved recurrence deferral must reach auto-push before the recurrence update"
    );
    let current = backend.get_requirement(&row.id).unwrap().unwrap();
    assert!(current.deferred);
    assert_eq!(
        crate::findings::finding_recurrence(&current.tags.iter().cloned().collect::<Vec<_>>()),
        2
    );
    sweep(&backend, &[current], &Facts, Utc::now(), false, 14).unwrap();
    assert!(!backend.get_requirement(&row.id).unwrap().unwrap().deferred);
    // Exercise every machine condition through the actual recurrence writer,
    // not just the parser. Unknown forge/release observations must retain work;
    // a later met condition must still be able to resurface the same finding.
    // trace:STORY-2402 | ai:codex
    for (trigger, canonical, retained) in [
        ("pr:#42=MERGED", "pr:42=merged", true),
        ("release:v0.17", "release:>=0.17", true),
        ("spec:TASK-1=approved", "spec:TASK-1=approved", true),
        ("spec:TASK-1=completed", "spec:TASK-1=completed", false),
        (
            "query:tag=ready count>=3",
            "query:tag=ready count>=3",
            false,
        ),
    ] {
        crate::findings_gate::handle_findings_recur(&backend, &path, id, None, Some(trigger), None)
            .unwrap();
        let current = backend.get_requirement(&row.id).unwrap().unwrap();
        assert_eq!(current.deferred_until.as_deref(), Some(canonical));
        let recurrence =
            crate::findings::finding_recurrence(&current.tags.iter().cloned().collect::<Vec<_>>());
        sweep(&backend, &[current], &Facts, Utc::now(), false, 14).unwrap();
        let current = backend.get_requirement(&row.id).unwrap().unwrap();
        assert_eq!(current.deferred, retained, "{trigger}");
        assert_eq!(
            crate::findings::finding_recurrence(&current.tags.iter().cloned().collect::<Vec<_>>(),),
            recurrence,
            "sweeping {trigger} must preserve recurrence evidence",
        );
        assert!(backend.queue_list("role:human", true).unwrap().is_empty());
    }
    let note = dir.path().join("handoff.txt");
    std::fs::write(&note, "Review the dependency after merge.").unwrap();
    let before = backend.get_requirement(&row.id).unwrap().unwrap();
    // Manual handoffs must reject missing/blank reasons before touching either
    // the note or the canonical spec, just like free-text triggers.
    // trace:STORY-2402 | ai:codex
    for (trigger, reason) in [
        ("if it stalls", None),
        ("manual:joe", None),
        ("manual:joe", Some("  ")),
    ] {
        assert!(crate::session_misc::session_handoff_seat(
            Some(note.to_str().unwrap()),
            false,
            Some("advisor"),
            false,
            false,
            Some(trigger),
            Some(id),
            reason,
            false,
            false,
        )
        .is_err());
        assert_eq!(
            serde_yaml::to_value(&before).unwrap(),
            serde_yaml::to_value(backend.get_requirement(&row.id).unwrap().unwrap()).unwrap()
        );
        assert!(!dir.path().join(".aida/handoff/advisor/latest.md").exists());
    }
    // Plain handoff failures must retain the actionable first-line diagnostic
    // shown by agent mode. trace:STORY-2402 | ai:codex
    std::fs::write(
        &note,
        "x".repeat(crate::seat_rotation::HANDOFF_MAX_BYTES + 1),
    )
    .unwrap();
    let error = crate::session_misc::session_handoff_seat(
        Some(note.to_str().unwrap()),
        false,
        Some("advisor"),
        false,
        false,
        None,
        None,
        None,
        false,
        false,
    )
    .unwrap_err();
    let rendered = format!("{error:?}");
    let (summary, _) = crate::agent_error_summary_help(&rendered);
    assert!(
        summary.starts_with("handoff note is 16001 bytes; the cap is 16000"),
        "{rendered}"
    );
    std::fs::write(&note, "x".repeat(crate::seat_rotation::HANDOFF_MAX_BYTES)).unwrap();
    assert!(crate::session_misc::session_handoff_seat(
        Some(note.to_str().unwrap()),
        false,
        Some("advisor"),
        false,
        false,
        Some("date:2026-01-01"),
        Some(id),
        None,
        false,
        false,
    )
    .is_err());
    assert!(!backend.get_requirement(&row.id).unwrap().unwrap().deferred);
    // A note I/O failure must report the already-persisted deferral.
    // trace:STORY-2402 | ai:codex
    std::fs::write(&note, "Review the dependency after merge.").unwrap();
    let handoff_dir = dir.path().join(".aida/handoff/advisor");
    std::fs::create_dir_all(handoff_dir.parent().unwrap()).unwrap();
    std::fs::write(&handoff_dir, "block directory creation").unwrap();
    let push_calls = crate::store_push_pull::HANDOFF_PUSH_CALLS.with(|calls| calls.get());
    let error = crate::session_misc::session_handoff_seat(
        Some(note.to_str().unwrap()),
        false,
        Some("advisor"),
        false,
        false,
        Some("date:2026-01-01"),
        Some(id),
        None,
        false,
        false,
    )
    .unwrap_err();
    assert_eq!(
        crate::store_push_pull::HANDOFF_PUSH_CALLS.with(|calls| calls.get()),
        push_calls + 1,
        "saved deferral must reach auto-push even when note I/O fails"
    );
    let message = format!("{error:#}");
    assert!(message.contains(id), "{message}");
    assert!(message.contains("date:2026-01-01"), "{message}");
    assert!(message.contains("deferral remains saved"), "{message}");
    assert!(backend.get_requirement(&row.id).unwrap().unwrap().deferred);
    std::fs::remove_file(&handoff_dir).unwrap();
    crate::session_misc::session_handoff_seat(
        Some(note.to_str().unwrap()),
        false,
        Some("advisor"),
        false,
        false,
        Some("date:2026-01-01"),
        Some(id),
        None,
        false,
        false,
    )
    .unwrap();
    assert_eq!(
        crate::store_push_pull::HANDOFF_PUSH_CALLS.with(|calls| calls.get()),
        push_calls + 2,
        "successful handoff must also reach auto-push"
    );
    let handoff = crate::seat_rotation::read_latest_handoff(dir.path(), "advisor")
        .unwrap()
        .unwrap();
    assert!(handoff.contains("date:2026-01-01"));
    let current = backend.get_requirement(&row.id).unwrap().unwrap();
    assert!(current.deferred);
    sweep(&backend, &[current], &Facts, Utc::now(), false, 14).unwrap();
    assert!(!backend.get_requirement(&row.id).unwrap().unwrap().deferred);
    let original_age = Utc::now() - chrono::Duration::days(15);
    let mut aged = backend.get_requirement(&row.id).unwrap().unwrap();
    aged.deferred = true;
    aged.deferred_until = Some("date:2099-01-01".into());
    aged.deferred_at = Some(original_age);
    backend.update_requirement(&aged).unwrap();
    crate::findings_gate::handle_findings_recur(
        &backend,
        &path,
        id,
        None,
        Some("manual:joe"),
        Some("needs judgement"),
    )
    .unwrap();
    // A valid manual handoff persists the same hold as recurrence parking;
    // the timer routes it to a human without clearing it or bumping recurrence.
    // trace:STORY-2402 | ai:codex
    let recurrence = crate::findings::finding_recurrence(
        &backend
            .get_requirement(&row.id)
            .unwrap()
            .unwrap()
            .tags
            .into_iter()
            .collect::<Vec<_>>(),
    );
    crate::session_misc::session_handoff_seat(
        Some(note.to_str().unwrap()),
        false,
        Some("advisor"),
        false,
        false,
        Some("manual:joe"),
        Some(id),
        Some("handoff needs judgement"),
        false,
        false,
    )
    .unwrap();
    let current = backend.get_requirement(&row.id).unwrap().unwrap();
    assert_eq!(current.deferred_until.as_deref(), Some("manual:joe"));
    assert_eq!(
        current.deferred_reason.as_deref(),
        Some("handoff needs judgement")
    );
    assert_eq!(
        crate::findings::finding_recurrence(&current.tags.iter().cloned().collect::<Vec<_>>()),
        recurrence,
    );
    assert_eq!(current.deferred_at, Some(original_age));
    sweep(&backend, &[current], &Facts, Utc::now(), false, 14).unwrap();
    assert!(backend.get_requirement(&row.id).unwrap().unwrap().deferred);
    assert_eq!(backend.queue_list("role:human", true).unwrap().len(), 1);
}

#[test]
fn story2402_s5_handoff_cli_requires_write_and_spec() {
    for args in [
        vec!["aida", "session", "handoff", "--until", "date:2026-01-01"],
        vec![
            "aida",
            "session",
            "handoff",
            "--write",
            "-",
            "--until",
            "date:2026-01-01",
        ],
        vec!["aida", "session", "handoff", "--show", "--reason", "prose"],
        vec![
            "aida",
            "session",
            "handoff",
            "--snapshot",
            "--write",
            "-",
            "--spec",
            "TASK-1",
            "--until",
            "date:2026-01-01",
        ],
    ] {
        assert!(crate::TEST_CLI.try_parse_from(args).is_err());
    }
    assert!(crate::TEST_CLI
        .try_parse_from([
            "aida",
            "session",
            "handoff",
            "--seat",
            "advisor",
            "--write",
            "-",
            "--spec",
            "TASK-1",
            "--until",
            "manual:joe",
            "--reason",
            "review needed",
        ])
        .is_ok());
    assert!(crate::condition_cmd::canonical_condition(Some("if it stalls"), None).is_err());
}

// trace:STORY-2402 | ai:codex
#[test]
fn story2402_s5_conditional_handoff_auto_pushes() {
    for (args, expected) in [
        (vec!["--show"], false),
        (vec!["--write", "note.md"], false),
        (
            vec![
                "--write",
                "note.md",
                "--spec",
                "TASK-1",
                "--until",
                "date:2026-01-01",
            ],
            true,
        ),
    ] {
        let mut argv = vec!["aida", "session", "handoff"];
        argv.extend(args);
        let cli = crate::TEST_CLI.try_parse_from(argv).unwrap();
        assert_eq!(
            crate::command_triggers_per_write_auto_push(&cli.command),
            expected
        );
    }
}

// Both typed writers must refuse holds before changing a row or handoff.
// trace:STORY-2402 | ai:codex
#[test]
fn story2402_s5_implementer_cannot_set_holds() {
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
    assert!(init.status.success());
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(dir.path(), "implementer", &[]);
    let path = dir.path().join(".aida-store");
    std::fs::create_dir_all(&path).unwrap();
    aida_core::work_binding::bootstrap(&path).unwrap().unwrap();
    let backend = CachedGitBackend::open(&path, &dir.path().join("cache.db")).unwrap();
    let mut row = Requirement::new("finding".into(), "".into());
    row.tags.insert("from-advisor:STORY-1".into());
    let row = backend.add_requirement(row).unwrap();
    let id = row.spec_id.as_deref().unwrap();
    let note = dir.path().join("note.md");
    std::fs::write(&note, "Revisit after human review.").unwrap();
    let before = serde_yaml::to_value(&row).unwrap();
    for result in [
        crate::findings_gate::handle_findings_recur(
            &backend,
            &path,
            id,
            None,
            Some("manual:joe"),
            Some("review required"),
        ),
        crate::session_misc::session_handoff_seat(
            Some(note.to_str().unwrap()),
            false,
            Some("implementer"),
            false,
            false,
            Some("manual:joe"),
            Some(id),
            Some("review required"),
            false,
            false,
        ),
    ] {
        assert!(format!("{:#}", result.unwrap_err()).contains("may set an operator hold"));
        assert_eq!(
            serde_yaml::to_value(backend.get_requirement(&row.id).unwrap().unwrap()).unwrap(),
            before
        );
    }
    assert!(!dir
        .path()
        .join(".aida/handoff/implementer/latest.md")
        .exists());
    assert_eq!(
        crate::store_push_pull::HANDOFF_PUSH_CALLS.with(|calls| calls.get()),
        0
    );

    // Early session dispatch must refuse default team identity before parking
    // even for a machine trigger that does not need hold-setting authority.
    // trace:STORY-2402 | ai:codex
    std::fs::create_dir_all(path.join("registry")).unwrap();
    std::fs::write(path.join("registry/nodes.toml"),
        "[[nodes]]\nid = 'remote'\nuser_id = 1\nhostname = 'other'\nregistered = '2026-01-01T00:00:00Z'\n").unwrap();
    ambient.set_key("AIDA_USER", "default");
    ambient.set_key("AIDA_TEAM_REQUIRE_USER", "1");
    let error = crate::session_misc::session_handoff_seat(
        Some(note.to_str().unwrap()),
        false,
        Some("implementer"),
        false,
        false,
        Some("date:2026-01-01"),
        Some(id),
        None,
        false,
        false,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("AIDA_TEAM_REQUIRE_USER=1"));
    assert_eq!(
        serde_yaml::to_value(backend.get_requirement(&row.id).unwrap().unwrap()).unwrap(),
        before
    );
    assert!(!dir
        .path()
        .join(".aida/handoff/implementer/latest.md")
        .exists());
    // Exercise the registered policy through the same guard as store dispatch.
    // Refusal must precede both the recurrence increment and deferral write.
    // trace:STORY-2402 | ai:codex
    let cli = crate::TEST_CLI
        .try_parse_from([
            "aida",
            "findings",
            "recur",
            id,
            "--until",
            "date:2026-01-01",
        ])
        .unwrap();
    let recur = || {
        crate::status_awaiting::maybe_team_identity_guard(&path, &cli.command)?;
        crate::findings_gate::handle_findings_recur(
            &backend,
            &path,
            id,
            None,
            Some("date:2026-01-01"),
            None,
        )
    };
    let error = recur().unwrap_err();
    assert!(format!("{error:#}").contains("AIDA_TEAM_REQUIRE_USER=1"));
    assert_eq!(
        serde_yaml::to_value(backend.get_requirement(&row.id).unwrap().unwrap()).unwrap(),
        before
    );
    ambient.set_key("AIDA_USER", "distinct");
    recur().unwrap();
    let current = backend.get_requirement(&row.id).unwrap().unwrap();
    assert!(current.deferred);
    assert_eq!(current.deferred_until.as_deref(), Some("date:2026-01-01"));
    assert_eq!(
        crate::findings::finding_recurrence(&current.tags.iter().cloned().collect::<Vec<_>>()),
        2
    );
}

// Unknown dependency IDs keep work parked without failing the hourly sweep.
// trace:STORY-2402 | ai:codex
#[test]
fn story2402_s5_missing_spec_stays_parked() {
    let row = parked("spec:TYPO-1=completed");
    let (next, evidence, migration) = action(&row, &Facts, Utc::now(), 14).unwrap();
    assert_eq!(next, Action::Keep);
    assert!(evidence.contains("unknown"));
    assert!(!migration);
}
