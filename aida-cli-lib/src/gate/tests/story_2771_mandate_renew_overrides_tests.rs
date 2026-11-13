//! `aida mandate renew` overrides: `--ttl`, and the start flags applied to
//! the renewed mandate with the change recorded in its lineage.
// trace:STORY-2771 | ai:claude

use super::*;

fn t0() -> DateTime<Utc> {
    "2026-11-13T12:00:00Z".parse().unwrap()
}

fn human_ok() -> Result<()> {
    Ok(())
}

fn no_tty() -> Result<()> {
    bail!("role grants require a human at an interactive TTY")
}

/// A plain ship-held mandate with a reason, started at `t0` for 8h.
fn ship_held(dir: &Path) -> Mandate {
    start_with_scopes(
        dir,
        &human_ok,
        "joe",
        Duration::hours(8),
        Some("morning queue".into()),
        false,
        false,
        BTreeSet::from([Scope::ShipHeld]),
        t0(),
    )
    .unwrap()
}

fn stored(dir: &Path, at: DateTime<Utc>) -> Mandate {
    let (valid, warnings) = load_valid(dir, "joe", at);
    assert!(warnings.is_empty(), "{warnings:?}");
    match state_at(&valid, at) {
        MandateState::Active(m) => m,
        other => panic!("expected an active mandate, got {other:?}"),
    }
}

fn parsed_renew(args: &[&str]) -> crate::cli::MandateAction {
    let mut argv = vec!["aida", "mandate", "renew"];
    argv.extend_from_slice(args);
    crate::command_registry::parse_command(argv)
        .registered::<crate::mandate::MandateArgs>()
        .action
}

#[test]
fn ttl_is_an_alias_of_for() {
    let duration = |args: &[&str]| match parsed_renew(args) {
        crate::cli::MandateAction::Renew { duration, .. } => duration,
        other => panic!("{other:?}"),
    };
    assert_eq!(duration(&["--ttl", "90m"]), Some("90m".to_string()));
    assert_eq!(duration(&["--ttl", "90m"]), duration(&["--for", "90m"]));
    assert_eq!(duration(&[]), None);
}

#[test]
fn renewal_without_overrides_keeps_every_flag_and_records_no_change() {
    let tmp = tempfile::tempdir().unwrap();
    let before = start_with_scopes(
        tmp.path(),
        &human_ok,
        "joe",
        Duration::hours(8),
        Some("weekend fire".into()),
        true,
        true,
        BTreeSet::from([Scope::ShipHeld]),
        t0(),
    )
    .unwrap();
    // Protected paths opted in at start stay opted in.
    let mut opted = before.clone();
    opted.include_gate_paths = true;
    write_private(
        &tmp.path().join(format!("{}.json", opted.id)),
        &serde_json::to_vec_pretty(&opted).unwrap(),
    )
    .unwrap();
    let at = t0() + Duration::hours(2);
    let (after, renewal, revived) = renew_with(
        tmp.path(),
        &human_ok,
        "joe",
        None,
        &RenewOverrides::default(),
        at,
    )
    .unwrap();
    assert_eq!(revived, None);
    assert!(renewal.changes.is_empty());
    assert_eq!(after.expires_at, at + Duration::hours(8));
    assert_eq!(
        (
            &after.id,
            &after.scopes,
            &after.kind,
            after.include_supervised,
            after.include_gate_paths,
            &after.reason
        ),
        (
            &opted.id,
            &opted.scopes,
            &opted.kind,
            true,
            true,
            &opted.reason
        )
    );
    assert_eq!(stored(tmp.path(), at), after);
}

#[test]
fn an_override_changes_the_mandate_and_is_recorded_in_its_lineage() {
    let tmp = tempfile::tempdir().unwrap();
    let before = ship_held(tmp.path());
    let at = t0() + Duration::hours(1);
    let overrides = RenewOverrides {
        include_gate_paths: Some(true),
        include_supervised: Some(true),
        add_scopes: BTreeSet::from([Scope::Dispatch]),
        reason: Some("gate-path train".into()),
        ..RenewOverrides::default()
    };
    let (after, renewal, _) = renew_with(
        tmp.path(),
        &human_ok,
        "joe",
        Some(Duration::hours(4)),
        &overrides,
        at,
    )
    .unwrap();
    assert_eq!(after.id, before.id);
    assert!(after.include_gate_paths && after.include_supervised);
    assert_eq!(
        after.effective_scopes(),
        BTreeSet::from([Scope::ShipHeld, Scope::Dispatch])
    );
    assert_eq!(after.reason.as_deref(), Some("gate-path train"));
    assert_eq!(
        renewal.changes,
        vec![
            "scopes: ship-held -> ship-held, dispatch".to_string(),
            "include-supervised: off -> on".to_string(),
            "include-gate-paths: off -> on".to_string(),
            "reason: morning queue -> gate-path train".to_string(),
        ]
    );
    // The lineage on disk carries the change and still validates.
    let on_disk = stored(tmp.path(), at);
    assert_eq!(on_disk.renewals, vec![renewal]);
    assert_eq!(on_disk, after);

    // Naming the value it already has is no change; turning one off is.
    let at2 = at + Duration::hours(1);
    let overrides = RenewOverrides {
        include_supervised: Some(true),
        include_gate_paths: Some(false),
        add_scopes: BTreeSet::from([Scope::ShipHeld]),
        ..RenewOverrides::default()
    };
    let (again, second, _) =
        renew_with(tmp.path(), &human_ok, "joe", None, &overrides, at2).unwrap();
    assert_eq!(
        second.changes,
        vec!["include-gate-paths: on -> off".to_string()]
    );
    assert!(!again.include_gate_paths && again.include_supervised);
    assert_eq!(again.renewals.len(), 2);
}

#[test]
fn drive_makes_it_a_drive_mandate_and_keeps_the_supervised_flag() {
    let tmp = tempfile::tempdir().unwrap();
    ship_held(tmp.path());
    let at = t0() + Duration::hours(1);
    let overrides = RenewOverrides {
        drive: true,
        ..RenewOverrides::default()
    };
    let (after, renewal, _) =
        renew_with(tmp.path(), &human_ok, "joe", None, &overrides, at).unwrap();
    assert!(after.is_drive());
    assert!(!after.include_supervised);
    for scope in Scope::ALL {
        assert!(after.covers(scope), "{scope:?}");
    }
    assert_eq!(renewal.changes[0], "drive: off -> on");
    assert_eq!(stored(tmp.path(), at), after);
}

#[test]
fn a_plain_renewal_leaves_an_old_drive_records_scopes_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let mut old = start_with_scopes(
        tmp.path(),
        &human_ok,
        "joe",
        Duration::hours(8),
        None,
        true,
        true,
        BTreeSet::from([Scope::ShipHeld]),
        t0(),
    )
    .unwrap();
    // A drive record from before relay-restart existed.
    old.scopes.remove(&Scope::RelayRestart);
    write_private(
        &tmp.path().join(format!("{}.json", old.id)),
        &serde_json::to_vec_pretty(&old).unwrap(),
    )
    .unwrap();
    let (after, renewal, _) = renew_with(
        tmp.path(),
        &human_ok,
        "joe",
        None,
        &RenewOverrides::default(),
        t0() + Duration::hours(1),
    )
    .unwrap();
    assert_eq!(after.scopes, old.scopes);
    assert!(!after.covers(Scope::RelayRestart));
    assert!(renewal.changes.is_empty());
}

#[test]
fn overrides_need_a_human_at_a_terminal_like_any_renewal() {
    let tmp = tempfile::tempdir().unwrap();
    let before = ship_held(tmp.path());
    let overrides = RenewOverrides {
        include_gate_paths: Some(true),
        ..RenewOverrides::default()
    };
    let at = t0() + Duration::hours(1);
    let err = renew_with(tmp.path(), &no_tty, "joe", None, &overrides, at).unwrap_err();
    assert!(err.to_string().contains("human at an interactive terminal"));
    assert_eq!(stored(tmp.path(), at), before);
}

#[test]
fn a_revival_applies_the_overrides_to_the_fresh_mandate() {
    let tmp = tempfile::tempdir().unwrap();
    let before = ship_held(tmp.path());
    let late = before.expires_at + Duration::hours(1);
    let overrides = RenewOverrides {
        include_gate_paths: Some(true),
        ..RenewOverrides::default()
    };
    let (revived, receipt, old_id) =
        renew_with(tmp.path(), &human_ok, "joe", None, &overrides, late).unwrap();
    assert_eq!(old_id.as_deref(), Some(before.id.as_str()));
    assert_ne!(revived.id, before.id);
    assert!(revived.include_gate_paths && revived.renewals.is_empty());
    assert_eq!(
        receipt.changes,
        vec!["include-gate-paths: off -> on".to_string()]
    );
}

#[test]
fn the_flags_map_to_overrides_and_absent_flags_ask_for_nothing() {
    let overrides = |args: &[&str]| match parsed_renew(args) {
        crate::cli::MandateAction::Renew {
            ship_held,
            scopes,
            drive,
            include_supervised,
            exclude_supervised,
            include_gate_paths,
            exclude_gate_paths,
            reason,
            ..
        } => crate::mandate::renew_overrides(
            ship_held,
            &scopes,
            drive,
            include_supervised,
            exclude_supervised,
            include_gate_paths,
            exclude_gate_paths,
            reason,
        )
        .unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(overrides(&[]), RenewOverrides::default());
    assert_eq!(
        overrides(&[
            "--ship-held",
            "--scope",
            "dispatch",
            "--drive",
            "--exclude-supervised",
            "--include-gate-paths",
            "--reason",
            "why"
        ]),
        RenewOverrides {
            drive: true,
            add_scopes: BTreeSet::from([Scope::ShipHeld, Scope::Dispatch]),
            include_supervised: Some(false),
            include_gate_paths: Some(true),
            reason: Some("why".into()),
        }
    );
    assert_eq!(
        overrides(&["--include-supervised", "--exclude-gate-paths"]),
        RenewOverrides {
            include_supervised: Some(true),
            include_gate_paths: Some(false),
            ..RenewOverrides::default()
        }
    );
}

#[test]
fn a_kept_protected_path_opt_in_is_printed_and_a_changed_or_absent_one_is_not() {
    let tmp = tempfile::tempdir().unwrap();
    ship_held(tmp.path());
    let gate_paths = |include| RenewOverrides {
        include_gate_paths: Some(include),
        ..RenewOverrides::default()
    };
    let at = |hours| t0() + Duration::hours(hours);
    let renewed = |overrides: &RenewOverrides, hours| {
        let (m, renewal, _) =
            renew_with(tmp.path(), &human_ok, "joe", None, overrides, at(hours)).unwrap();
        crate::mandate::kept_gate_paths_line(&m, &renewal)
    };
    let keep = RenewOverrides::default();
    assert_eq!(renewed(&keep, 1), None);
    assert_eq!(renewed(&gate_paths(true), 2), None);
    let kept = Some("includes protected paths: KEPT (pass --exclude-gate-paths to drop)".into());
    assert_eq!(renewed(&keep, 3), kept);
    assert_eq!(renewed(&gate_paths(true), 4), kept);
    // A revival keeps it too, and says so.
    assert_eq!(renewed(&keep, 20), kept);
    assert_eq!(renewed(&gate_paths(false), 21), None);
}
