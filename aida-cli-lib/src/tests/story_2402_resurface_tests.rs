//! Hourly trigger sweep: holds are review work, never automatic authority.
// trace:STORY-2402 | ai:codex
use super::*;
use aida_core::condition::{QueryFilter, Version};
use aida_core::RequirementStatus;

struct Facts;
impl ConditionFacts for Facts {
    fn spec_status(&self, id: &str) -> Result<Option<RequirementStatus>, String> {
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
    assert!(action(&parked("spec:TASK-404=completed"), &Facts, now, 14).is_err());
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
        .add_requirement(parked("spec:TASK-404=completed"))
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
        crate::command_registry::try_parse_from(args).unwrap();
    }
    assert!(crate::command_registry::try_parse_from(["aida", "defer"]).is_err());
    assert!(crate::command_registry::try_parse_from([
        "aida",
        "defer",
        "resurface",
        "--reason",
        "bypass"
    ])
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
        3
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
