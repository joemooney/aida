//! STORY-1662 PR2: lane identity from the worktree lease — the correction a
//! `claude --bg` lane's SessionStart hook and MCP server apply, and the role
//! `aida claim` records inside a lane.
// trace:STORY-1662 | ai:claude

use super::*;

const LANE: &str = "0123456789ab";

fn advisor_grant() -> InheritedGrant {
    InheritedGrant::Valid {
        id: "g-advisor".into(),
        seat: "advisor".into(),
    }
}

#[test]
fn an_advisor_env_in_an_implementer_lane_becomes_implementer_without_the_grant() {
    let fix = fix_for("implementer", None, Some("advisor"), &advisor_grant())
        .unwrap()
        .expect("a correction");
    assert_eq!(
        fix,
        LaneFix {
            role: "implementer".into(),
            grant: GrantFix::Unset,
            unset_purpose: true,
        }
    );
    assert_eq!(
        render(&fix),
        "export AIDA_SESSION_ROLE='implementer'\n\
         unset AIDA_SESSION_GRANT\n\
         unset AIDA_SESSION_PURPOSE\n"
    );
}

#[test]
fn the_lease_child_grant_is_exported_in_place_of_the_inherited_one() {
    let fix = fix_for(
        "implementer",
        Some("g-child"),
        Some("advisor"),
        &advisor_grant(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(fix.grant, GrantFix::Export("g-child".into()));
    assert!(render(&fix).contains("export AIDA_SESSION_GRANT='g-child'\n"));
}

/// Q-b: with no child grant, an inherited grant is kept only when its seat
/// is the lease's; an invalid one is always dropped.
#[test]
fn an_inherited_grant_is_kept_only_for_the_lease_seat() {
    let own = InheritedGrant::Valid {
        id: "g-impl".into(),
        seat: "implementer".into(),
    };
    assert_eq!(
        fix_for("implementer", None, Some("implementer"), &own).unwrap(),
        None,
        "the environment is already the lane's"
    );
    assert_eq!(
        fix_for(
            "implementer",
            None,
            Some("implementer"),
            &InheritedGrant::Absent
        )
        .unwrap(),
        None
    );
    let reviewer = InheritedGrant::Valid {
        id: "g-rev".into(),
        seat: "reviewer".into(),
    };
    let fix = fix_for("implementer", None, Some("implementer"), &reviewer)
        .unwrap()
        .unwrap();
    assert_eq!(fix.grant, GrantFix::Unset);
    assert!(!fix.unset_purpose, "the role already matched");
    let fix = fix_for("implementer", None, None, &InheritedGrant::Invalid)
        .unwrap()
        .unwrap();
    assert_eq!(fix.grant, GrantFix::Unset);
    // The child grant already in force is not re-exported.
    let child = InheritedGrant::Valid {
        id: "g-child".into(),
        seat: "implementer".into(),
    };
    assert_eq!(
        fix_for("implementer", Some("g-child"), Some("implementer"), &child).unwrap(),
        None
    );
}

/// C3: a role or grant carrying shell metacharacters is refused, never
/// quoted into a line.
#[test]
fn values_with_shell_metacharacters_are_refused() {
    for bad in [
        "implementer;touch pwned",
        "impl'ementer",
        "a b",
        "$(id)",
        "`id`",
        "x\nexport FOO=1",
        "",
        &"a".repeat(65),
    ] {
        assert!(validate_value("role", bad).is_err(), "{bad:?}");
        assert!(
            fix_for(bad, None, Some("advisor"), &InheritedGrant::Absent).is_err(),
            "{bad:?}"
        );
        if !bad.is_empty() {
            assert!(
                fix_for(
                    "implementer",
                    Some(bad),
                    Some("advisor"),
                    &InheritedGrant::Absent
                )
                .is_err(),
                "{bad:?}"
            );
        }
    }
    assert!(validate_value("grant", "0199a1b2-c3d4_7e5f").is_ok());
}

fn write_lease(root: &Path, id: &str, worktree: &Path, role: Option<&str>, claim: bool) {
    let mut table: toml::Table = toml::from_str(&format!(
        r#"
id = "{id}"
scope = "STORY-9"
slug = "story-9"
owner = "tester"
branch = "claude/story-9"
started_at = "2026-10-22T00:00:00Z"
hostname = "h"
claim_verb = {claim}
"#
    ))
    .unwrap();
    table.insert(
        "worktree_path".into(),
        toml::Value::String(worktree.to_string_lossy().into_owned()),
    );
    if let Some(role) = role {
        table.insert("role".into(), toml::Value::String(role.into()));
    }
    let path = crate::lease_path(root, id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, toml::to_string_pretty(&table).unwrap()).unwrap();
}

fn lane_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap().join("project");
    let lane = tmp.path().canonicalize().unwrap().join("lane");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::create_dir_all(lane.join("src")).unwrap();
    (tmp, root, lane)
}

#[test]
fn the_lane_lease_is_the_worktree_lease_covering_cwd_never_a_claim() {
    let (_tmp, root, lane) = lane_fixture();
    write_lease(&root, "claimclaim01", &lane, Some("advisor"), true);
    assert!(
        lane_lease_for_cwd(&root, &lane).is_none(),
        "a claim is not a lane"
    );
    write_lease(&root, LANE, &lane, Some("implementer"), false);
    let lease = lane_lease_for_cwd(&root, &lane.join("src")).expect("the lane lease");
    assert_eq!(lease.id, LANE);
    assert_eq!(lease_role(&lease).unwrap().as_deref(), Some("implementer"));
    assert!(
        lane_lease_for_cwd(&root, &root).is_none(),
        "outside the lane"
    );
}

/// The lease role is validated before it can reach a shell line.
#[test]
fn a_lease_role_with_metacharacters_is_an_error() {
    let (_tmp, root, lane) = lane_fixture();
    write_lease(&root, LANE, &lane, Some("implementer;touch x"), false);
    let lease = lane_lease_for_cwd(&root, &lane).unwrap();
    assert!(lease_role(&lease).is_err());
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("advisor")),
        (crate::seat_authority::GRANT_ENV, None),
    ]);
    assert!(fix_at(&root, &lane).is_err());
}

/// BUG-2207: `aida claim` inside a lane records the lease's role, not the
/// advisor seat the daemon env carries.
#[test]
fn claim_in_a_lane_keeps_the_lease_role() {
    let (_tmp, root, lane) = lane_fixture();
    write_lease(&root, LANE, &lane, Some("implementer"), false);
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("advisor")),
        (crate::seat_authority::GRANT_ENV, Some("g-unknown")),
    ]);
    assert_eq!(
        crate::claim::claim_role(&root, Some(&lane)).as_deref(),
        Some("implementer")
    );
    // Outside a lane the claim keeps the grant's seat (none valid here).
    assert_eq!(crate::claim::claim_role(&root, Some(&root)), None);
}

/// `aida internal lane-env`'s lines for an advisor env in a lane, and none
/// outside it.
#[test]
fn fix_at_reads_the_lane_and_the_inherited_env() {
    let (_tmp, root, lane) = lane_fixture();
    write_lease(&root, LANE, &lane, Some("implementer"), false);
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_SESSION_PURPOSE", Some("advise")),
        (crate::seat_authority::GRANT_ENV, Some("g-not-a-real-grant")),
    ]);
    let fix = fix_at(&root, &lane).unwrap().expect("a correction");
    assert_eq!(
        render(&fix),
        "export AIDA_SESSION_ROLE='implementer'\n\
         unset AIDA_SESSION_GRANT\n\
         unset AIDA_SESSION_PURPOSE\n"
    );
    assert_eq!(fix_at(&root, &root).unwrap(), None);
}

/// C2: the MCP server applies the same correction to its own env at start.
#[test]
fn the_mcp_server_takes_the_lane_identity_at_start() {
    let (_tmp, root, lane) = lane_fixture();
    write_lease(&root, LANE, &lane, Some("implementer"), false);
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_SESSION_PURPOSE", Some("advise")),
        (crate::seat_authority::GRANT_ENV, Some("g-daemon-advisor")),
    ]);
    apply_in_process(&root, &lane);
    assert_eq!(std::env::var("AIDA_SESSION_ROLE").unwrap(), "implementer");
    assert!(std::env::var(crate::seat_authority::GRANT_ENV).is_err());
    assert!(std::env::var("AIDA_SESSION_PURPOSE").is_err());
}

/// C2 outside a lane: the MCP server's env is left alone.
#[test]
fn the_mcp_server_outside_a_lane_keeps_its_env() {
    let (_tmp, root, _lane) = lane_fixture();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("advisor")),
        (crate::seat_authority::GRANT_ENV, Some("g-advisor")),
    ]);
    apply_in_process(&root, &root);
    assert_eq!(std::env::var("AIDA_SESSION_ROLE").unwrap(), "advisor");
    assert_eq!(
        std::env::var(crate::seat_authority::GRANT_ENV).unwrap(),
        "g-advisor"
    );
}

/// C4: `aida doctor` names a claude daemon whose environment holds a grant,
/// and only the daemon (not its hosted sessions, nor a clean daemon).
#[test]
fn doctor_finds_a_claude_daemon_holding_a_grant() {
    let proc_root = tempfile::tempdir().unwrap();
    let proc = |pid: u32, argv: &[&str], env: &[&str]| {
        let dir = proc_root.path().join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cmdline"), argv.join("\0") + "\0").unwrap();
        std::fs::write(dir.join("environ"), env.join("\0") + "\0").unwrap();
    };
    proc(
        10,
        &["/home/u/.local/bin/claude", "daemon", "run"],
        &["HOME=/home/u", "AIDA_SESSION_GRANT=g-advisor"],
    );
    proc(11, &["claude", "daemon", "run"], &["AIDA_SESSION_GRANT="]);
    proc(
        12,
        &["claude", "bg-pty-host", "--bg-pty-host", "x"],
        &["AIDA_SESSION_GRANT=g-advisor"],
    );
    proc(
        13,
        &["/usr/bin/node", "/opt/claude", "daemon"],
        &["AIDA_SESSION_GRANT=g-x"],
    );
    proc(14, &["claude-daemon"], &["AIDA_SESSION_GRANT=g-x"]);
    std::fs::create_dir_all(proc_root.path().join("self")).unwrap();
    assert_eq!(daemons_holding_a_grant(proc_root.path()), vec![10, 13]);
    let finding = crate::doctor_root::claude_daemon_grant_finding(10);
    assert_eq!(finding.category, "claude-daemon-grant");
    assert!(finding.action.contains("kill 10"));
    assert!(!finding.safe_heal);
    assert_eq!(
        crate::doctor_root::normalize_doctor_category("daemon-grant").unwrap(),
        "claude-daemon-grant"
    );
}

/// TASK-2213: a least-privilege stakeholder role is never widened to the
/// lane's seat, in the hook lines or in the MCP server's own env.
#[test]
fn a_guest_or_requester_in_a_lane_keeps_its_role() {
    for role in ["guest", "requester", "Requester"] {
        assert_eq!(
            fix_for("implementer", None, Some(role), &InheritedGrant::Absent).unwrap(),
            None,
            "{role}"
        );
        assert_eq!(
            fix_for("advisor", Some("g-child"), Some(role), &advisor_grant()).unwrap(),
            None,
            "{role}"
        );
    }
    let (_tmp, root, lane) = lane_fixture();
    write_lease(&root, LANE, &lane, Some("implementer"), false);
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("requester")),
        ("AIDA_SESSION_PURPOSE", Some("file")),
        (crate::seat_authority::GRANT_ENV, None),
    ]);
    apply_in_process(&root, &lane);
    assert_eq!(std::env::var("AIDA_SESSION_ROLE").unwrap(), "requester");
    assert_eq!(std::env::var("AIDA_SESSION_PURPOSE").unwrap(), "file");
}

/// A reviewer seat in a reviewer worktree keeps its grant: the lane is
/// already its own, so nothing is corrected (the PR-2844 reviewer that lost
/// its recording grant).
#[test]
fn a_reviewer_in_a_reviewer_lane_keeps_its_grant() {
    let reviewer = InheritedGrant::Valid {
        id: "g-rev".into(),
        seat: "reviewer".into(),
    };
    assert_eq!(
        fix_for("reviewer", None, Some("reviewer"), &reviewer).unwrap(),
        None
    );
}

/// TASK-2214: a lane lease whose role does not validate fails closed. The
/// claim records no role, never the inherited (valid) advisor seat.
#[test]
fn claim_in_a_lane_with_a_malformed_role_records_no_role() {
    let (_tmp, root, lane) = lane_fixture();
    write_lease(&root, LANE, &lane, Some("implementer;touch x"), false);
    let grant = crate::seat_authority::test_support::mint_grant_for(
        &root,
        &crate::current_user_id(None),
        "advisor",
        &[],
    );
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("advisor")),
        (crate::seat_authority::GRANT_ENV, Some(grant.as_str())),
    ]);
    assert_eq!(
        crate::seat_authority::current_seat(&root).as_deref(),
        Some("advisor"),
        "the inherited seat is valid, so a fallback would record it"
    );
    assert_eq!(crate::claim::claim_role(&root, Some(&lane)), None);
    assert_eq!(
        crate::claim::claim_role(&root, Some(&root)).as_deref(),
        Some("advisor"),
        "outside a lane the grant's seat is recorded"
    );
}

/// TASK-2217: the restamp refuses a lease a live session holds, and leaves
/// every other field of an idle lease intact.
#[test]
fn stamping_a_lane_role_refuses_a_live_lease_and_keeps_other_fields() {
    let (_tmp, root, lane) = lane_fixture();
    write_lease(&root, LANE, &lane, Some("advisor"), false);
    let err = stamp_lease_role_checked(&root, LANE, "implementer", |_| true).unwrap_err();
    assert!(format!("{err:#}").contains("live session"), "{err:#}");
    let lease = lane_lease_for_cwd(&root, &lane).unwrap();
    assert_eq!(
        lease.role.as_deref(),
        Some("advisor"),
        "a refusal writes nothing"
    );

    stamp_lease_role_checked(&root, LANE, "implementer", |_| false).unwrap();
    let lease = lane_lease_for_cwd(&root, &lane).unwrap();
    assert_eq!(lease.role.as_deref(), Some("implementer"));
    assert_eq!(lease.scope, "STORY-9");
    assert_eq!(lease.branch, "claude/story-9");
    assert!(stamp_lease_role_checked(&root, LANE, "x'; id; '", |_| false).is_err());
}

/// TASK-2217: the restamp waits for the lease-write lock, so it can never
/// interleave with another stamp's read-modify-write on the same lease.
#[test]
fn stamping_a_lane_role_takes_the_lease_write_lock() {
    let (_tmp, root, lane) = lane_fixture();
    write_lease(&root, LANE, &lane, Some("advisor"), false);
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let root_ref = &root;
        let holder = scope.spawn(move || {
            crate::session::with_lease_write_lock(root_ref, || {
                held_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
        });
        held_rx.recv().unwrap();
        let stamper =
            scope.spawn(|| stamp_lease_role_checked(&root, LANE, "implementer", |_| false));
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(
            lane_lease_for_cwd(&root, &lane).unwrap().role.as_deref(),
            Some("advisor"),
            "the stamp ran while another writer held the lock"
        );
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        stamper.join().unwrap().unwrap();
    });
    assert_eq!(
        lane_lease_for_cwd(&root, &lane).unwrap().role.as_deref(),
        Some("implementer")
    );
}

/// TASK-2216: the doctor scans an injected process table; under `cfg(test)`
/// the default never reads the real `/proc`.
#[test]
fn the_doctor_daemon_scan_reads_an_injected_proc_root() {
    let (_tmp, root, _lane) = lane_fixture();
    let proc_root = tempfile::tempdir().unwrap();
    let dir = proc_root.path().join("42");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cmdline"), "claude\0daemon\0").unwrap();
    std::fs::write(dir.join("environ"), "AIDA_SESSION_GRANT=g-advisor\0").unwrap();
    let store = aida_core::models::RequirementsStore::default();
    let found = crate::doctor_root::collect_doctor_findings_at(
        &root,
        &store,
        Some("claude-daemon-grant"),
        proc_root.path(),
    )
    .unwrap();
    assert_eq!(
        found.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        vec!["pid 42"]
    );
    assert_eq!(
        crate::doctor_root::doctor_proc_root(),
        std::path::PathBuf::new()
    );
    let default =
        crate::doctor_root::collect_doctor_findings(&root, &store, Some("claude-daemon-grant"))
            .unwrap();
    assert!(default.is_empty(), "{default:?}");
}
