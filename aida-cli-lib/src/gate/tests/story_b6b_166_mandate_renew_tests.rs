//! `aida mandate renew`: extend the active mandate in place — one test per
//! acceptance bullet.
// trace:STORY-B6B1278890774EF993D2EBC327862470-166 | ai:claude

use super::*;

fn t0() -> DateTime<Utc> {
    "2026-10-31T12:00:00Z".parse().unwrap()
}

fn human_ok() -> Result<()> {
    Ok(())
}

fn no_tty() -> Result<()> {
    bail!("role grants require a human at an interactive TTY")
}

/// A drive mandate with every non-default flag set, started at `t0` for 8h.
fn started(dir: &Path) -> Mandate {
    start_with_scopes(
        dir,
        &human_ok,
        "joe",
        Duration::hours(8),
        Some("weekend fire".into()),
        true,
        true,
        BTreeSet::from([Scope::ShipHeld]),
        t0(),
    )
    .unwrap()
}

fn on_disk(dir: &Path, at: DateTime<Utc>) -> (Vec<Mandate>, Vec<String>) {
    load_valid(dir, "joe", at)
}

// ── AC1: human at an interactive terminal only ─────────────────────────────

#[test]
fn renew_refuses_without_a_human_at_a_terminal_and_leaves_the_record_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let m = started(tmp.path());
    let err = renew(tmp.path(), &no_tty, "joe", None, t0() + Duration::hours(1)).unwrap_err();
    assert!(err.to_string().contains("interactive terminal"), "{err}");
    let (valid, _) = on_disk(tmp.path(), t0());
    assert_eq!(valid, vec![m]);
}

// ── AC2: only an active, unexpired mandate ─────────────────────────────────

#[test]
fn renew_with_no_mandate_points_at_start() {
    let tmp = tempfile::tempdir().unwrap();
    let err = renew(tmp.path(), &human_ok, "joe", None, t0()).unwrap_err();
    assert!(err.to_string().contains("aida mandate start"), "{err}");
}

#[test]
fn renew_refuses_an_expired_or_stopped_mandate() {
    let tmp = tempfile::tempdir().unwrap();
    started(tmp.path());
    let late = t0() + Duration::hours(9);
    let err = renew(tmp.path(), &human_ok, "joe", None, late).unwrap_err();
    assert!(err.to_string().contains("aida mandate start"), "{err}");

    let tmp = tempfile::tempdir().unwrap();
    started(tmp.path());
    stop(tmp.path(), &human_ok, "joe", t0() + Duration::hours(1)).unwrap();
    let err = renew(
        tmp.path(),
        &human_ok,
        "joe",
        None,
        t0() + Duration::hours(2),
    )
    .unwrap_err();
    assert!(err.to_string().contains("aida mandate start"), "{err}");
}

// ── AC3: scope and flags kept; expiry = now + duration, capped ─────────────

#[test]
fn renew_keeps_scope_and_flags_and_defaults_to_the_original_duration() {
    let tmp = tempfile::tempdir().unwrap();
    let before = started(tmp.path());
    let at = t0() + Duration::hours(6);
    let (after, renewal) = renew(tmp.path(), &human_ok, "joe", None, at).unwrap();
    assert_eq!(after.expires_at, at + Duration::hours(8));
    assert_eq!(renewal.expires_at, after.expires_at);
    assert_eq!(after.id, before.id);
    assert_eq!(after.principal, before.principal);
    assert_eq!(after.seat, before.seat);
    assert_eq!(after.scope, before.scope);
    assert_eq!(after.scopes, before.scopes);
    assert_eq!(after.kind, before.kind);
    assert_eq!(after.include_supervised, before.include_supervised);
    assert_eq!(after.reason, before.reason);
    assert_eq!(after.issued_at, before.issued_at);
    let (valid, warnings) = on_disk(tmp.path(), at);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(valid, vec![after.clone()]);
    assert_eq!(after.effective_scopes(), before.effective_scopes());

    // A second default renewal still uses the ORIGINAL duration.
    let at2 = at + Duration::hours(1);
    let (again, _) = renew(tmp.path(), &human_ok, "joe", None, at2).unwrap();
    assert_eq!(again.expires_at, at2 + Duration::hours(8));
}

#[test]
fn renew_for_an_explicit_duration_is_capped_like_start() {
    let tmp = tempfile::tempdir().unwrap();
    started(tmp.path());
    let at = t0() + Duration::hours(2);
    let (m, _) = renew(
        tmp.path(),
        &human_ok,
        "joe",
        Some(Duration::minutes(90)),
        at,
    )
    .unwrap();
    assert_eq!(m.expires_at, at + Duration::minutes(90));
    // The CLI parses `--for` with start's parser: the same 24h maximum.
    assert!(parse_ttl("25h").is_err());
    assert_eq!(parse_ttl("24h").unwrap(), MAX_TTL);
    // The gate never writes more than the cap, whatever it is handed.
    let (m, _) = renew(tmp.path(), &human_ok, "joe", Some(Duration::hours(30)), at).unwrap();
    assert_eq!(m.expires_at, at + MAX_TTL);
}

// ── AC4: lineage record, renewal count in status ───────────────────────────

#[test]
fn renew_records_its_lineage_and_status_shows_the_count() {
    let tmp = tempfile::tempdir().unwrap();
    let before = started(tmp.path());
    // Renewed late enough that the whole lifetime exceeds 24h from issuance:
    // the cap runs from the latest renewal, so the record stays valid.
    let at = t0() + Duration::hours(7);
    let (_, first) = renew(tmp.path(), &human_ok, "joe", None, at).unwrap();
    let at2 = t0() + Duration::hours(14);
    let (m, second) = renew(tmp.path(), &human_ok, "joe", Some(Duration::hours(12)), at2).unwrap();
    assert!(m.expires_at - m.issued_at > MAX_TTL);
    assert_eq!(
        m.renewals,
        vec![
            Renewal {
                renewed_at: at,
                renewed_by: "joe".into(),
                previous_expires_at: before.expires_at,
                expires_at: at + Duration::hours(8),
            },
            Renewal {
                renewed_at: at2,
                renewed_by: "joe".into(),
                previous_expires_at: first.expires_at,
                expires_at: at2 + Duration::hours(12),
            },
        ]
    );
    assert_eq!(second, m.renewals[1]);
    let (valid, warnings) = on_disk(tmp.path(), at2 + Duration::hours(1));
    assert!(warnings.is_empty(), "{warnings:?}");
    let state = state_at(&valid, at2 + Duration::hours(1));
    assert_eq!(state, MandateState::Active(m.clone()));

    let text = crate::mandate::status_report(&state, at2, None, false);
    assert!(text.contains("renewals:  2 (last by joe"), "{text}");
    let json: serde_json::Value =
        serde_json::from_str(&crate::mandate::status_report(&state, at2, None, true)).unwrap();
    assert_eq!(json["renewal_count"], 2);

    // The events.jsonl audit copy names old -> new expiry and the renewer.
    crate::mandate::emit_renewed(tmp.path(), &m, &second);
    let renewed: Vec<_> = crate::events::read_all(tmp.path())
        .into_iter()
        .filter_map(|e| match e.kind {
            crate::events::EventKind::MandateChanged {
                id,
                principal,
                expires_at,
                renewed_from,
                ..
            } => Some((id, principal, expires_at, renewed_from)),
            _ => None,
        })
        .collect();
    assert_eq!(
        renewed,
        vec![(
            m.id.clone(),
            "joe".to_string(),
            second.expires_at.to_rfc3339(),
            Some(first.expires_at.to_rfc3339()),
        )]
    );
}

#[test]
fn a_status_with_no_renewals_says_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let m = started(tmp.path());
    let text = crate::mandate::status_report(&MandateState::Active(m), t0(), None, false);
    assert!(text.contains("renewals:  0"), "{text}");
}

#[test]
fn a_record_whose_lineage_does_not_add_up_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    started(tmp.path());
    let (mut m, _) = renew(
        tmp.path(),
        &human_ok,
        "joe",
        None,
        t0() + Duration::hours(7),
    )
    .unwrap();
    // Forged: an expiry pushed past what the lineage records.
    m.expires_at += Duration::hours(4);
    write_private(
        &tmp.path().join(format!("{}.json", m.id)),
        &serde_json::to_vec_pretty(&m).unwrap(),
    )
    .unwrap();
    let (valid, warnings) = on_disk(tmp.path(), t0() + Duration::hours(8));
    assert!(valid.is_empty());
    assert!(warnings[0].contains("renewal lineage"), "{warnings:?}");
}

#[test]
fn a_stop_after_a_renewal_still_wins() {
    let tmp = tempfile::tempdir().unwrap();
    started(tmp.path());
    let at = t0() + Duration::hours(7);
    renew(tmp.path(), &human_ok, "joe", None, at).unwrap();
    stop(tmp.path(), &human_ok, "joe", at + Duration::minutes(5)).unwrap();
    let later = at + Duration::hours(1);
    let (valid, _) = on_disk(tmp.path(), later);
    assert!(matches!(state_at(&valid, later), MandateState::Stopped(_)));
}

// ── AC5: grants derived from the mandate survive the renewal ──────────────

#[test]
fn grants_naming_the_mandate_stay_valid_across_the_renewal() {
    let tmp = tempfile::tempdir().unwrap();
    let m = started(tmp.path());
    // A grant derived from the mandate (a dispatch request, the drive tick)
    // carries its id and re-checks that exact id before acting.
    let grant_mandate_id = m.id.clone();
    let past_old_expiry = m.expires_at + Duration::hours(1);
    assert_eq!(
        recheck(tmp.path(), &grant_mandate_id, "joe", past_old_expiry),
        Err(MandateRefusal::Expired {
            id: grant_mandate_id.clone()
        })
    );

    renew(
        tmp.path(),
        &human_ok,
        "joe",
        None,
        m.expires_at - Duration::hours(1),
    )
    .unwrap();
    let still = recheck(tmp.path(), &grant_mandate_id, "joe", past_old_expiry).unwrap();
    assert_eq!(still.id, grant_mandate_id);
    assert!(still.covers(Scope::Dispatch));
    assert!(still.covers(Scope::ShipHeld));
}
