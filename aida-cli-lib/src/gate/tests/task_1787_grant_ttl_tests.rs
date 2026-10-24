//! Tests for TASK-1787: Operator-chosen grant TTL (`--ttl <dur>`),
//! capped by `[seat] max_grant_hours` (default 24, hard ceiling 168h).
// trace:TASK-1787 | ai:antigravity

use super::*;
use chrono::Duration;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_format_ttl() {
    assert_eq!(format_ttl(Duration::hours(72)), "72h");
    assert_eq!(format_ttl(Duration::hours(24)), "24h");
    assert_eq!(format_ttl(Duration::hours(1)), "1h");
    assert_eq!(format_ttl(Duration::minutes(30)), "30m");
    assert_eq!(format_ttl(Duration::minutes(90)), "1h30m");
    assert_eq!(format_ttl(Duration::minutes(65)), "1h05m");
    assert_eq!(format_ttl(Duration::zero()), "0m");
    assert_eq!(format_ttl(Duration::minutes(-5)), "0m");
}

#[test]
fn test_resolve_max_grant_hours_defaults_to_24() {
    let dir = tempdir().unwrap();
    let _env =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(dir.path().to_str().unwrap()))]);
    let cap = resolve_max_grant_hours(dir.path());
    assert_eq!(cap, DEFAULT_MAX_GRANT_HOURS);
    assert_eq!(cap, 24);
}

#[test]
fn test_resolve_max_grant_hours_project_lowers_default() {
    let dir = tempdir().unwrap();
    let empty_home = tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[(
        "AIDA_HOME",
        Some(empty_home.path().to_str().unwrap()),
    )]);
    let aida_dir = dir.path().join(".aida");
    fs::create_dir_all(&aida_dir).unwrap();
    fs::write(
        aida_dir.join("config.toml"),
        "[seat]\nmax_grant_hours = 12\n",
    )
    .unwrap();

    let cap = resolve_max_grant_hours(dir.path());
    assert_eq!(cap, 12);
}

#[test]
fn test_resolve_max_grant_hours_precedence() {
    let dir = tempdir().unwrap();
    let fake_home = tempdir().unwrap();
    let global_aida = fake_home.path().join(".aida");
    fs::create_dir_all(&global_aida).unwrap();
    let project_aida = dir.path().join(".aida");
    fs::create_dir_all(&project_aida).unwrap();

    let _env = crate::test_env::EnvVarsGuard::apply(&[(
        "AIDA_HOME",
        Some(fake_home.path().to_str().unwrap()),
    )]);

    // 1. Both set: global=72, project=48 -> min is 48
    fs::write(
        global_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 72\n",
    )
    .unwrap();
    fs::write(
        project_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 48\n",
    )
    .unwrap();
    assert_eq!(resolve_max_grant_hours(dir.path()), 48);

    // 2. Both set: global=48, project=72 -> min is 48 (project cannot raise global cap)
    fs::write(
        global_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 48\n",
    )
    .unwrap();
    fs::write(
        project_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 72\n",
    )
    .unwrap();
    assert_eq!(resolve_max_grant_hours(dir.path()), 48);

    // 3. Project alone: project=72, global=None -> min(24, 72) = 24 (project alone cannot raise 24h ceiling)
    fs::remove_file(global_aida.join("config.toml")).unwrap();
    fs::write(
        project_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 72\n",
    )
    .unwrap();
    assert_eq!(resolve_max_grant_hours(dir.path()), 24);

    // 4. Project alone: project=12, global=None -> min(24, 12) = 12 (project alone can lower 24h ceiling)
    fs::write(
        project_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 12\n",
    )
    .unwrap();
    assert_eq!(resolve_max_grant_hours(dir.path()), 12);
}

#[test]
fn test_worktree_config_cannot_raise_max_grant_hours() {
    let dir = tempdir().unwrap();
    let fake_home = tempdir().unwrap();
    let global_aida = fake_home.path().join(".aida");
    fs::create_dir_all(&global_aida).unwrap();
    let project_aida = dir.path().join(".aida");
    fs::create_dir_all(&project_aida).unwrap();

    let _env = crate::test_env::EnvVarsGuard::apply(&[(
        "AIDA_HOME",
        Some(fake_home.path().to_str().unwrap()),
    )]);

    // Case 1: No global config set (trusted default is 24h).
    // Worktree attempts to set max_grant_hours = 100.
    // It must NOT raise the cap above 24h.
    fs::write(
        project_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 100\n",
    )
    .unwrap();
    assert_eq!(
        resolve_max_grant_hours(dir.path()),
        DEFAULT_MAX_GRANT_HOURS,
        "worktree config alone must not raise max_grant_hours above the 24h default ceiling"
    );

    // Case 2: Global config sets trusted cap to 12h.
    // Worktree attempts to set max_grant_hours = 48.
    // Cap must remain 12h; worktree cannot raise trusted global cap.
    fs::write(
        global_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 12\n",
    )
    .unwrap();
    assert_eq!(
        resolve_max_grant_hours(dir.path()),
        12,
        "worktree config must not raise max_grant_hours above the global trusted cap"
    );

    // Case 3: Worktree may only LOWER the cap.
    // Global is 12h, worktree sets 6h -> cap becomes 6h.
    fs::write(
        project_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 6\n",
    )
    .unwrap();
    assert_eq!(
        resolve_max_grant_hours(dir.path()),
        6,
        "worktree config may only lower the cap"
    );
}

#[test]
fn test_parse_ttl_bare_number_without_unit_refused() {
    let dir = tempdir().unwrap();
    let _env =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(dir.path().to_str().unwrap()))]);
    let err = parse_ttl(dir.path(), Some("72")).unwrap_err().to_string();
    assert!(
        err.contains("--ttl needs a unit, e.g. 72h or 90m"),
        "error message `{err}` must mention unit requirement"
    );
    let err = parse_ttl(dir.path(), Some("24")).unwrap_err().to_string();
    assert!(err.contains("--ttl needs a unit, e.g. 72h or 90m"));
}

#[test]
fn test_parse_ttl_none_and_empty() {
    let dir = tempdir().unwrap();
    let _env =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(dir.path().to_str().unwrap()))]);
    assert_eq!(parse_ttl(dir.path(), None).unwrap(), None);
    assert_eq!(parse_ttl(dir.path(), Some("")).unwrap(), None);
    assert_eq!(parse_ttl(dir.path(), Some("   ")).unwrap(), None);
}

#[test]
fn test_parse_ttl_within_default_cap() {
    let dir = tempdir().unwrap();
    let _env =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(dir.path().to_str().unwrap()))]);
    // Default cap is 24h
    let dur = parse_ttl(dir.path(), Some("12h")).unwrap().unwrap();
    assert_eq!(dur, Duration::hours(12));

    let dur = parse_ttl(dir.path(), Some("24h")).unwrap().unwrap();
    assert_eq!(dur, Duration::hours(24));

    let dur = parse_ttl(dir.path(), Some("30m")).unwrap().unwrap();
    assert_eq!(dur, Duration::minutes(30));
}

#[test]
fn test_parse_ttl_within_custom_cap() {
    let dir = tempdir().unwrap();
    let fake_home = tempdir().unwrap();
    let global_aida = fake_home.path().join(".aida");
    fs::create_dir_all(&global_aida).unwrap();
    fs::write(
        global_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 72\n",
    )
    .unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[(
        "AIDA_HOME",
        Some(fake_home.path().to_str().unwrap()),
    )]);

    let dur = parse_ttl(dir.path(), Some("72h")).unwrap().unwrap();
    assert_eq!(dur, Duration::hours(72));

    let dur = parse_ttl(dir.path(), Some("48h")).unwrap().unwrap();
    assert_eq!(dur, Duration::hours(48));
}

#[test]
fn test_parse_ttl_above_cap_refused_with_setting_name() {
    let dir = tempdir().unwrap();
    let empty_home = tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[(
        "AIDA_HOME",
        Some(empty_home.path().to_str().unwrap()),
    )]);
    // Default cap is 24h; requesting 25h should fail naming [seat] max_grant_hours
    let err = parse_ttl(dir.path(), Some("25h")).unwrap_err().to_string();
    assert!(
        err.contains("exceeds the configured cap of 24h ([seat] max_grant_hours)"),
        "error message `{err}` must name `[seat] max_grant_hours`"
    );
    drop(_env);

    // Global cap 48h; requesting 50h should fail
    let fake_home = tempdir().unwrap();
    let global_aida = fake_home.path().join(".aida");
    fs::create_dir_all(&global_aida).unwrap();
    fs::write(
        global_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 48\n",
    )
    .unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[(
        "AIDA_HOME",
        Some(fake_home.path().to_str().unwrap()),
    )]);
    let err = parse_ttl(dir.path(), Some("50h")).unwrap_err().to_string();
    assert!(
        err.contains("exceeds the configured cap of 48h ([seat] max_grant_hours)"),
        "error message `{err}` must name `[seat] max_grant_hours`"
    );
}

#[test]
fn test_parse_ttl_above_hard_ceiling_refused_even_with_higher_config() {
    let dir = tempdir().unwrap();
    let fake_home = tempdir().unwrap();
    let global_aida = fake_home.path().join(".aida");
    fs::create_dir_all(&global_aida).unwrap();
    // Config attempts to set max_grant_hours = 200 (above hard ceiling 168h)
    fs::write(
        global_aida.join("config.toml"),
        "[seat]\nmax_grant_hours = 200\n",
    )
    .unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[(
        "AIDA_HOME",
        Some(fake_home.path().to_str().unwrap()),
    )]);

    // 168h is allowed
    let dur = parse_ttl(dir.path(), Some("168h")).unwrap().unwrap();
    assert_eq!(dur, Duration::hours(168));

    // 169h is refused naming the 168h maximum (not configurable)
    let err = parse_ttl(dir.path(), Some("169h")).unwrap_err().to_string();
    assert!(
        err.contains("exceeds the 168h maximum (not configurable)"),
        "error message `{err}` must mention 168h maximum (not configurable)"
    );

    let err = parse_ttl(dir.path(), Some("180h")).unwrap_err().to_string();
    assert!(
        err.contains("exceeds the 168h maximum (not configurable)"),
        "error message `{err}` must mention 168h maximum (not configurable)"
    );
}

#[test]
fn test_parse_ttl_rejects_negative_or_zero_or_invalid() {
    let dir = tempdir().unwrap();
    let _env =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(dir.path().to_str().unwrap()))]);
    let err = parse_ttl(dir.path(), Some("0m")).unwrap_err().to_string();
    assert!(err.contains("grant TTL must be positive"));

    let err = parse_ttl(dir.path(), Some("not-a-duration"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("invalid grant TTL `not-a-duration`"));
}

#[test]
fn test_default_ttl_unchanged_24h_even_with_higher_config() {
    let dir = tempdir().unwrap();
    let empty_home = tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[(
        "AIDA_HOME",
        Some(empty_home.path().to_str().unwrap()),
    )]);
    let aida_dir = dir.path().join(".aida");
    fs::create_dir_all(&aida_dir).unwrap();
    fs::write(
        aida_dir.join("config.toml"),
        "[seat]\nmax_grant_hours = 72\n",
    )
    .unwrap();

    let grant_id = test_support::mint_grant_for(dir.path(), "user1", "advisor", &[]);
    let grant = read_grant(&grant_id).unwrap();
    let ttl = grant.expires_at.signed_duration_since(grant.issued_at);
    assert_eq!(ttl, Duration::hours(24));
}

#[test]
fn test_custom_ttl_minted_correctly() {
    let dir = tempdir().unwrap();
    let empty_home = tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[(
        "AIDA_HOME",
        Some(empty_home.path().to_str().unwrap()),
    )]);
    let grant_id = test_support::mint_grant_for_with_ttl(
        dir.path(),
        "user1",
        "advisor",
        &[],
        Some(Duration::hours(72)),
    );
    let grant = read_grant(&grant_id).unwrap();
    let ttl = grant.expires_at.signed_duration_since(grant.issued_at);
    assert_eq!(ttl, Duration::hours(72));
}

#[test]
fn test_non_tty_mint_still_refused() {
    let dir = tempdir().unwrap();
    // Non-interactive / headless environment must refuse issuance
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HEADLESS", Some("1")),
        ("AIDA_AGENT_NAME", None),
        ("AIDA_AGENT_TYPE", None),
        ("AIDA_HOME", Some(dir.path().to_str().unwrap())),
    ]);

    let prev = crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
        project_root: dir.path().to_path_buf(),
        stdin_is_terminal: false,
        stdout_is_terminal: false,
        stderr_is_terminal: false,
    }));

    let res = issue_direct(
        dir.path(),
        "advisor",
        Vec::new(),
        Some(Duration::hours(12)),
        None,
    );
    crate::test_ambient::replace(prev);

    assert!(res.is_err(), "non-TTY minting must be refused");
    let err = res.unwrap_err().to_string();
    assert!(
        err.contains("role grants require a human at an interactive TTY"),
        "error `{err}` should mention interactive TTY"
    );
}

#[test]
fn test_managed_agent_mint_refused() {
    let dir = tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HEADLESS", None),
        ("AIDA_AGENT_NAME", Some("antigravity-1")),
        ("AIDA_HOME", Some(dir.path().to_str().unwrap())),
    ]);

    let res = issue_direct(dir.path(), "advisor", Vec::new(), None, None);
    assert!(res.is_err(), "managed agent minting must be refused");
    let err = res.unwrap_err().to_string();
    assert!(err.contains("AIDA-managed agent sessions cannot issue direct TTY grants"));
}

#[test]
fn test_task_1736_relay_refusal_with_long_grant() {
    let dir = tempdir().unwrap();
    let now = chrono::Utc::now();

    // 1. Long grant (72h) with lots of time left (>1h)
    let grant_id = test_support::mint_grant_for_with_ttl(
        dir.path(),
        "fixture-user",
        "advisor",
        &[],
        Some(Duration::hours(72)),
    );
    let grant = read_grant(&grant_id).unwrap();
    assert!(grant.expires_at > now + Duration::hours(71));

    // Expiry reports correctly
    let expiry = GrantExpiry {
        expires_at: grant.expires_at,
        seat: grant.seat.clone(),
    };
    // Statusline suffix when within 1h: None since >71h remaining
    assert_eq!(
        expiry_suffix(&expiry, now, Duration::hours(1)),
        None,
        "72h grant should not trigger 1h warning when fresh"
    );

    // 2. Grant near expiry (<1h left)
    let near_expiry = GrantExpiry {
        expires_at: now + Duration::minutes(45),
        seat: grant.seat.clone(),
    };
    let suffix = expiry_suffix(&near_expiry, now, Duration::hours(1));
    assert_eq!(suffix, Some("expires in 45m".to_string()));
}

#[test]
fn test_parse_ttl_boundary_values() {
    let dir = tempdir().unwrap();
    let _env =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(dir.path().to_str().unwrap()))]);
    // 1h valid
    let dur = parse_ttl(dir.path(), Some("1h")).unwrap().unwrap();
    assert_eq!(dur, Duration::hours(1));

    // 24h1m against default 24h cap -> refused
    let err = parse_ttl(dir.path(), Some("24h1m"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("exceeds the configured cap of 24h ([seat] max_grant_hours)"));

    // 168h1s -> refused naming 168h maximum (not configurable)
    let err = parse_ttl(dir.path(), Some("168h1s"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("exceeds the 168h maximum (not configurable)"));

    // "0s" -> refused as not positive
    let err = parse_ttl(dir.path(), Some("0s")).unwrap_err().to_string();
    assert!(err.contains("grant TTL must be positive"));

    // Oversized config clamped to exactly 168h
    assert_eq!(
        parse_max_grant_hours_from_str("[seat]\nmax_grant_hours = 500\n"),
        Some(168)
    );
}

#[test]
fn test_issue_direct_writes_requested_ttl() {
    let dir = tempdir().unwrap();
    let aida_dir = dir.path().join(".aida");
    fs::create_dir_all(&aida_dir).unwrap();
    fs::write(
        aida_dir.join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let store = dir.path().join(".aida-store");
    fs::create_dir_all(store.join("objects")).unwrap();
    let registry = store.join("registry");
    fs::create_dir_all(&registry).unwrap();
    fs::write(
        registry.join("team.toml"),
        "[members]\nhuman-operator = [\"advisor\"]\n",
    )
    .unwrap();

    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HEADLESS", None),
        ("AIDA_AGENT_NAME", None),
        ("AIDA_AGENT_TYPE", None),
        ("AIDA_USER", Some("human-operator")),
        ("AIDA_HOME", Some(dir.path().to_str().unwrap())),
    ]);
    let prev = crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
        project_root: dir.path().to_path_buf(),
        stdin_is_terminal: true,
        stdout_is_terminal: false,
        stderr_is_terminal: true,
    }));
    let grant = issue_direct(
        dir.path(),
        "advisor",
        Vec::new(),
        Some(Duration::hours(12)),
        None,
    )
    .unwrap();
    crate::test_ambient::replace(prev);
    let ttl = grant.expires_at.signed_duration_since(grant.issued_at);
    assert_eq!(ttl, Duration::hours(12));
}

#[test]
fn test_session_show_displays_grant_ttl() {
    let dir = tempdir().unwrap();
    let aida_dir = dir.path().join(".aida");
    fs::create_dir_all(&aida_dir).unwrap();

    let mut _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ID", Some("123456789012")),
        ("AIDA_HOME", Some(dir.path().to_str().unwrap())),
    ]);
    let grant_id = test_support::mint_grant_for_with_ttl(
        dir.path(),
        "fixture-user",
        "advisor",
        &[],
        Some(Duration::hours(72)),
    );
    _env.set_key(crate::seat_authority::GRANT_ENV, grant_id.as_str());

    let lease = crate::leases::SessionLease {
        tree_kind: Default::default(),
        id: "123456789012".to_string(),
        scope: "TASK-1787".to_string(),
        slug: "task-1787".to_string(),
        owner: "test-user".to_string(),
        worktree_path: dir.path().to_path_buf(),
        branch: "task-1787".to_string(),
        started_at: chrono::Utc::now(),
        hostname: "test-host".to_string(),
        role: Some("advisor".to_string()),
        creator_pid: None,
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: None,
        // trace:BUG-2165 | ai:codex
        child_grant_id: None,
    };
    let leases_dir = aida_dir.join("leases");
    fs::create_dir_all(&leases_dir).unwrap();
    fs::write(
        leases_dir.join(format!("{}.toml", lease.id)),
        toml::to_string(&lease).unwrap(),
    )
    .unwrap();
    let prev = crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
        project_root: dir.path().to_path_buf(),
        stdin_is_terminal: false,
        stdout_is_terminal: false,
        stderr_is_terminal: false,
    }));

    let resolved = crate::session_misc::resolve_session_grant(&lease);
    assert!(resolved.is_some(), "grant must resolve for current session");
    let grant = resolved.unwrap();
    let ttl = grant.expires_at.signed_duration_since(grant.issued_at);
    assert_eq!(crate::seat_authority::format_ttl(ttl), "72h");

    let ttl_line = crate::session_misc::format_session_grant_ttl_line(&lease);
    assert!(
        ttl_line.as_deref().unwrap_or("").contains("grant ttl")
            && ttl_line.as_deref().unwrap_or("").contains("72h"),
        "ttl line must format 72h: {ttl_line:?}"
    );

    // Another session lease does NOT resolve viewer's grant
    let mut other_lease = lease.clone();
    other_lease.id = "999999999999".to_string();
    assert!(
        crate::session_misc::resolve_session_grant(&other_lease).is_none(),
        "viewer grant must not resolve for another session"
    );

    let res = crate::session_misc::session_show(Some("123456789012"), false);
    crate::test_ambient::replace(prev);
    assert!(res.is_ok());
}
