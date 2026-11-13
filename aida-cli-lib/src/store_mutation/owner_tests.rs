//! Real launch owner fixtures: no synthetic authority validator.
// trace:TASK-3225 | ai:codex
use super::*;
use crate::gate::seat_authority::{self, test_support::mint_grant_for, SeatGrant};

struct Fixture {
    _tmp: tempfile::TempDir,
    main: PathBuf,
    lane: PathBuf,
    grant: String,
    parent: String,
    lease: crate::leases::SessionLease,
}
impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let lane = tmp.path().join("lane");
        std::fs::create_dir(&main).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-qm",
                "fixture",
                "--allow-empty",
            ],
            vec!["worktree", "add", "-q", "--detach", lane.to_str().unwrap()],
        ] {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&main)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let parent = mint_grant_for(&main, "fixture", "advisor", &["implementer"]);
        let grant = mint_grant_for(&main, "fixture", "implementer", &[]);
        let mut requirement = aida_core::Requirement::new("fixture".into(), "owner test".into());
        requirement.spec_id = Some("TASK-3225".into());
        // Seed a committed-format object without exercising allocation or
        // writer admission: this fixture tests the read-only launch owner.
        let object =
            aida_core::object_store::object_path(&main.join(".aida-store/objects"), "TASK-3225")
                .unwrap();
        std::fs::create_dir_all(object.parent().unwrap()).unwrap();
        std::fs::write(object, serde_yaml::to_string(&requirement).unwrap()).unwrap();
        let lease = serde_json::from_value(serde_json::json!({
            "id":"0123456789ab", "scope":"TASK-3225", "slug":"fixture", "owner":"fixture",
            "worktree_path":lane, "branch":"fixture", "started_at":chrono::Utc::now(),
            "hostname":"fixture", "role":"implementer"
        }))
        .unwrap();
        let fixture = Self {
            _tmp: tmp,
            main,
            lane,
            grant,
            parent,
            lease,
        };
        fixture.edit_grant(&fixture.grant, |g| {
            g.parent_grant_id = Some(fixture.parent.clone())
        });
        fixture.write_lease(&fixture.lease);
        fixture
    }
    fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new("true");
        command
            .env(seat_authority::GRANT_ENV, &self.grant)
            .env("AIDA_USER", "fixture")
            .env("AIDA_SESSION_ROLE", "implementer");
        command
    }
    fn prepare(&self) -> Result<Session> {
        Session::prepare(
            &self.command(),
            &self.lane,
            crate::contained_boundary::StoreWriteProfile::Compatibility,
        )
    }
    fn write_lease(&self, lease: &crate::leases::SessionLease) {
        let path = crate::leases::lease_path(&self.main, &lease.id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, toml::to_string(lease).unwrap()).unwrap();
    }
    fn edit_grant(&self, id: &str, edit: impl FnOnce(&mut SeatGrant)) {
        let path = seat_authority::grant_dir()
            .unwrap()
            .join(format!("{id}.json"));
        let mut grant = seat_authority::read_grant(id).unwrap();
        edit(&mut grant);
        std::fs::write(path, serde_json::to_vec(&grant).unwrap()).unwrap();
    }
    fn query(session: &mut Session) -> u8 {
        session.helper.set_nonblocking(false).unwrap();
        session
            .helper
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        session.helper.write_all(b"?").unwrap();
        let mut reply = [0];
        session.helper.read_exact(&mut reply).unwrap();
        reply[0]
    }
}

// The capability is passed by the launch owner after profile admission. Child
// environment assignments cannot widen a compatibility authority session.
// trace:TASK-3223 | ai:codex
#[test]
fn host_selected_profile_controls_broker_configuration() {
    use crate::contained_boundary::StoreWriteProfile;
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path().to_str().unwrap());
    let f = Fixture::new();
    let mut child = f.command();
    child.env("AIDA_STORE_MUTATION_PROFILE", "mediated");
    let session = Session::prepare(&child, &f.lane, StoreWriteProfile::Compatibility).unwrap();
    assert_eq!(session.config["store_profile"], "compatibility");
    drop(session);
    child.env("AIDA_STORE_MUTATION_PROFILE", "compatibility");
    let session = Session::prepare(&child, &f.lane, StoreWriteProfile::Mediated).unwrap();
    assert_eq!(session.config["store_profile"], "mediated");
}

#[test]
fn task_3225_prepare_ignores_unrelated_unreadable_empty_and_deleted_leases() {
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path().to_str().unwrap());
    let f = Fixture::new();
    for (id, path) in [
        ("empty", PathBuf::new()),
        ("deleted", f.main.join("deleted")),
        ("sibling", f.main.clone()),
    ] {
        let mut lease = f.lease.clone();
        lease.id = id.into();
        lease.worktree_path = path;
        f.write_lease(&lease);
    }
    std::fs::write(
        crate::leases::lease_path(&f.main, "malformed"),
        "not toml [",
    )
    .unwrap();
    let mut session = f.prepare().unwrap();
    assert_eq!(Fixture::query(&mut session), b'Y');
}

#[test]
fn task_3225_prepare_refuses_missing_or_mismatched_launch_authority() {
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path().to_str().unwrap());
    let f = Fixture::new();
    for key in ["AIDA_USER", "AIDA_SESSION_ROLE", seat_authority::GRANT_ENV] {
        let mut command = f.command();
        command.env_remove(key);
        assert!(
            Session::prepare(
                &command,
                &f.lane,
                crate::contained_boundary::StoreWriteProfile::Compatibility
            )
            .is_err(),
            "missing {key}"
        );
    }
    for (key, value) in [
        ("AIDA_USER", "other"),
        ("AIDA_SESSION_ROLE", "reviewer"),
        (seat_authority::GRANT_ENV, "invalid"),
        (seat_authority::GRANT_ENV, ""),
    ] {
        let mut command = f.command();
        command.env(key, value);
        assert!(
            Session::prepare(
                &command,
                &f.lane,
                crate::contained_boundary::StoreWriteProfile::Compatibility
            )
            .is_err(),
            "{key}={value}"
        );
    }
    let other = tempfile::tempdir().unwrap();
    assert!(
        Session::prepare(
            &f.command(),
            other.path(),
            crate::contained_boundary::StoreWriteProfile::Compatibility
        )
        .is_err(),
        "wrong project"
    );
    assert!(
        Session::prepare(
            &f.command(),
            &f.main,
            crate::contained_boundary::StoreWriteProfile::Compatibility
        )
        .is_err(),
        "wrong worktree"
    );
    let mut lease = f.lease.clone();
    lease.id = "second".into();
    f.write_lease(&lease);
    assert!(f.prepare().is_err(), "ambiguous matching leases");
    std::fs::remove_file(crate::leases::lease_path(&f.main, "second")).unwrap();
    lease = f.lease.clone();
    lease.scope = "unresolved".into();
    f.write_lease(&lease);
    assert!(f.prepare().is_err(), "unresolved owned spec");
    std::fs::remove_file(crate::leases::lease_path(&f.main, &lease.id)).unwrap();
    assert!(f.prepare().is_err(), "no matching lease");
}

#[test]
fn task_3225_owner_revalidates_real_grant_roster_and_parent() {
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path().to_str().unwrap());
    for fault in [
        "expired",
        "revoked",
        "parent_expired",
        "parent_revoked",
        "parent_changed",
        "roster_removed",
        "subject",
        "seat",
        "binding",
        "worker",
    ] {
        let f = Fixture::new();
        let mut session = f.prepare().unwrap();
        assert_eq!(Fixture::query(&mut session), b'Y', "{fault} before");
        match fault {
            "roster_removed" => {
                std::fs::remove_file(f.main.join(".aida-store/registry/team.toml")).unwrap()
            }
            "parent_expired" | "parent_revoked" => f.edit_grant(&f.parent, |g| {
                if fault == "parent_expired" {
                    g.expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
                } else {
                    g.revoked_at = Some(chrono::Utc::now());
                }
            }),
            "worker" => {
                let pid = std::process::id();
                let start = aida_core::liveness::process_start_identity(pid).unwrap();
                let raw = start.strip_prefix("linux-starttime:").unwrap();
                let token = worker_binding_token(pid, raw).unwrap();
                f.edit_grant(&f.grant, |g| g.binding.push(token));
                // Model Python settimeout() on the shared endpoint. The
                // receipt-barrier owner must not spuriously read EAGAIN.
                session.helper.set_nonblocking(true).unwrap();
                session.bind_worker(pid, raw).unwrap();
                assert_ne!(
                    unsafe { libc::fcntl(session.fd(), libc::F_GETFL) } & libc::O_NONBLOCK,
                    0
                );
                assert_eq!(Fixture::query(&mut session), b'Y');
                // A still-valid grant cannot authorize the worker after its
                // exact receipt token disappears from the binding.
                f.edit_grant(&f.grant, |g| g.binding = vec!["other-worker".into()]);
                assert!(session.bind_worker(pid, raw).is_err());
            }
            _ => f.edit_grant(&f.grant, |g| match fault {
                "expired" => g.expires_at = chrono::Utc::now() - chrono::Duration::seconds(1),
                "revoked" => g.revoked_at = Some(chrono::Utc::now()),
                "parent_changed" => g.parent_grant_id = None,
                "subject" => g.subject = "other".into(),
                "seat" => g.seat = "advisor".into(),
                "binding" => g.binding.clear(),
                _ => unreachable!(),
            }),
        }
        let expected = match fault {
            "expired" | "parent_expired" => b'E',
            "revoked" | "parent_revoked" => b'R',
            _ => b'N',
        };
        assert_eq!(Fixture::query(&mut session), expected, "{fault} after");
        // Each invalid authority also refuses a fresh prepare (a changed but
        // still valid parent/binding is only a live-session identity refusal).
        if !matches!(fault, "parent_changed" | "worker") {
            assert!(f.prepare().is_err(), "{fault} prepare");
        }
    }
}

// Keep the fixture config write visibly test-only to the config trust scanner.
// trace:TASK-3225 | ai:codex
#[cfg(test)]
#[test]
fn task_3225_owner_revalidates_every_lease_and_root_identity() {
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path().to_str().unwrap());
    for fault in [
        "project",
        "worktree",
        "store",
        "roster_store",
        "removed",
        "id",
        "scope",
        "started_at",
        "role",
        "lease_worktree",
    ] {
        let f = Fixture::new();
        let mut session = f.prepare().unwrap();
        assert_eq!(Fixture::query(&mut session), b'Y', "{fault} before");
        match fault {
            "project" | "worktree" | "store" => {
                let path = match fault {
                    "project" => f.main.clone(),
                    "worktree" => f.lane.clone(),
                    _ => f.main.join(".aida-store"),
                };
                std::fs::rename(&path, path.with_extension("old")).unwrap();
                std::fs::create_dir(&path).unwrap();
            }
            "roster_store" => {
                // A new, valid roster store must not replace the pinned one.
                let other = f.main.join("other-store");
                std::fs::create_dir_all(other.join("objects")).unwrap();
                std::fs::create_dir_all(other.join("registry")).unwrap();
                std::fs::copy(
                    f.main.join(".aida-store/registry/team.toml"),
                    other.join("registry/team.toml"),
                )
                .unwrap();
                std::fs::write(
                    f.main.join(".aida/config.toml"),
                    "store_path = \"other-store\"\n",
                )
                .unwrap();
            }
            "removed" => {
                std::fs::remove_file(crate::leases::lease_path(&f.main, &f.lease.id)).unwrap()
            }
            _ => {
                let mut lease = f.lease.clone();
                match fault {
                    "id" => lease.id = "rewritten".into(),
                    "scope" => lease.scope = "other".into(),
                    "started_at" => lease.started_at += chrono::Duration::seconds(1),
                    "role" => lease.role = Some("reviewer".into()),
                    "lease_worktree" => lease.worktree_path = f.main.clone(),
                    _ => unreachable!(),
                }
                std::fs::write(
                    crate::leases::lease_path(&f.main, &f.lease.id),
                    toml::to_string(&lease).unwrap(),
                )
                .unwrap();
            }
        }
        assert_eq!(Fixture::query(&mut session), b'N', "{fault} after");
    }
}
