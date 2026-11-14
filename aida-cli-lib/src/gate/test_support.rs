// trace:TASK-3784 | ai:codex
use crate::test_env::EnvVarsGuard;
use std::ffi::OsStr;

/// BUG-1618: the fixed identity `AmbientGuard::hermetic` pins `AIDA_USER` to,
/// chosen to be absent from any real roster.
// trace:BUG-1618 | ai:claude
// trace:BUG-2209 | ai:claude — one constant, shared with aida-core's roster-write guard.
pub(crate) const HERMETIC_TEST_USER: &str = aida_core::team::HERMETIC_TEST_USER;

/// BUG-1618: a hermetic ambient context for tests that exercise the
/// advisor-authority checks (`has_advisor_authority` and friends). Those checks
/// read ambient process state — the project root discovered from cwd (whose
/// `.aida-store` roster and drain state then decide the role and orchestrator
/// carve-outs), stdin TTY-ness, and the role / identity / orchestrator env
/// vars. Inside a leased `aida worktree add` checkout the cwd walk reaches a
/// `.aida-store` symlink to the live store, whose roster can make the
/// invoking user an advisor, so refusal tests silently gained authority.
///
/// This guard pins every one of those inputs for its lifetime:
/// - the project root to `root` (a per-thread override, no global `chdir`),
/// - stdin and stdout to "not a terminal",
/// - `AIDA_USER` to a fixed test identity absent from any roster,
/// - `AIDA_ROLE_INSTANCE`, `AIDA_AUTO_COMPLETE`, `AIDA_AUTO_COMPLETE_TOKEN`
///   cleared, and `AIDA_SESSION_ROLE` set to `role` (or cleared on `None`).
///
/// Holds `ENV_LOCK` like the other guards: do not nest another env guard
/// under it; drop it before constructing the next one.
// trace:BUG-1618 | ai:claude
pub(crate) struct AmbientGuard {
    prev: Option<crate::test_ambient::Ambient>,
    // Field order: the thread-local is restored in `Drop`, then `_env`
    // restores the env vars and releases ENV_LOCK last.
    _env: EnvVarsGuard,
}

impl AmbientGuard {
    /// Hermetic ambient plus a validated, session-bound `seat` grant for the
    /// hermetic test user (ADR-66: authority comes only from a grant — the
    /// role env stays a display/routing hint, so this mints the real records
    /// `aida role enter` would and exports the handle). `delegable` lists the
    /// child seats this session may issue (empty by default in production;
    /// pass the child's seat for launcher tests).
    // trace:STORY-1473 | ai:claude
    pub(crate) fn hermetic_with_seat(
        root: &std::path::Path,
        seat: &str,
        delegable: &[&str],
    ) -> Self {
        Self::hermetic_with_seat_and(root, seat, delegable, &[])
    }

    /// [`Self::hermetic_with_seat`] plus extra env pins applied under the SAME
    /// guard (the env lock is not reentrant, so a test cannot stack a second
    /// guard for them).
    // trace:STORY-1473 | ai:claude
    pub(crate) fn hermetic_with_seat_and(
        root: &std::path::Path,
        seat: &str,
        delegable: &[&str],
        extra: &[(&'static str, Option<&str>)],
    ) -> Self {
        let grant_id = crate::gate::seat_authority::test_support::mint_grant_for(
            root,
            HERMETIC_TEST_USER,
            seat,
            delegable,
        );
        let mut pairs: Vec<(&'static str, Option<&str>)> = vec![
            ("AIDA_SESSION_ROLE", Some(seat)),
            (
                crate::gate::seat_authority::GRANT_ENV,
                Some(grant_id.as_str()),
            ),
            ("AIDA_USER", Some(HERMETIC_TEST_USER)),
            ("AIDA_ROLE_INSTANCE", None),
            ("AIDA_AUTO_COMPLETE", None),
            ("AIDA_AUTO_COMPLETE_TOKEN", None),
        ];
        pairs.extend_from_slice(extra);
        let env = EnvVarsGuard::apply(&pairs);
        let prev = crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
            project_root: root.to_path_buf(),
            stdin_is_terminal: false,
            stdout_is_terminal: false,
            stderr_is_terminal: false,
        }));
        Self { prev, _env: env }
    }

    pub(crate) fn hermetic(root: &std::path::Path, role: Option<&str>) -> Self {
        Self::hermetic_and(root, role, &[])
    }

    /// [`Self::hermetic`] plus extra env pins applied under the SAME guard
    /// (the env lock is not reentrant).
    // trace:TASK-2053 | ai:claude
    // trace:TASK-1955 | ai:claude (TASK-1808)
    pub(crate) fn hermetic_and(
        root: &std::path::Path,
        role: Option<&str>,
        extra: &[(&'static str, Option<&str>)],
    ) -> Self {
        let mut pairs: Vec<(&'static str, Option<&str>)> = vec![
            ("AIDA_SESSION_ROLE", role),
            // ADR-66: a seat grant is ambient authority too — a hermetic
            // context must shed any leased grant along with the role env.
            // trace:STORY-1473 | ai:claude
            (crate::gate::seat_authority::GRANT_ENV, None),
            ("AIDA_USER", Some(HERMETIC_TEST_USER)),
            ("AIDA_ROLE_INSTANCE", None),
            ("AIDA_AUTO_COMPLETE", None),
            ("AIDA_AUTO_COMPLETE_TOKEN", None),
        ];
        pairs.extend_from_slice(extra);
        let env = EnvVarsGuard::apply(&pairs);
        let prev = crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
            project_root: root.to_path_buf(),
            stdin_is_terminal: false,
            stdout_is_terminal: false, // trace:BUG-1667 | ai:claude
            stderr_is_terminal: false,
        }));
        Self { prev, _env: env }
    }

    /// [`Self::hermetic`] that exports `grant` as the session grant handle
    /// instead of shedding it, without minting or validating anything: the
    /// caller prepares the grant record (valid, expired, revoked, or none at
    /// all) and this guard only seats the handle, as a relay launcher would.
    // trace:BUG-1983 | ai:claude
    pub(crate) fn hermetic_with_grant(
        root: &std::path::Path,
        role: Option<&str>,
        grant: &str,
    ) -> Self {
        let env = EnvVarsGuard::apply(&[
            ("AIDA_SESSION_ROLE", role),
            (crate::gate::seat_authority::GRANT_ENV, Some(grant)),
            ("AIDA_USER", Some(HERMETIC_TEST_USER)),
            ("AIDA_ROLE_INSTANCE", None),
            ("AIDA_AUTO_COMPLETE", None),
            ("AIDA_AUTO_COMPLETE_TOKEN", None),
        ]);
        let prev = crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
            project_root: root.to_path_buf(),
            stdin_is_terminal: false,
            stdout_is_terminal: false,
            stderr_is_terminal: false,
        }));
        Self { prev, _env: env }
    }

    /// Re-pin one env key under this guard's held lock; restored on drop like
    /// the keys applied at construction (the lock is not reentrant, so a
    /// fixture that must reissue a value after its pins cannot stack a guard).
    // trace:TASK-1993 | ai:claude
    pub(crate) fn set_key(&mut self, key: &'static str, value: impl AsRef<OsStr>) {
        self._env.set_key(key, value);
    }

    /// [`Self::set_key`]'s removal counterpart, restored on drop.
    // trace:TASK-1993 | ai:claude
    pub(crate) fn unset_key(&mut self, key: &'static str) {
        self._env.unset_key(key);
    }
}

impl Drop for AmbientGuard {
    fn drop(&mut self) {
        crate::test_ambient::replace(self.prev.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// BUG-1618: the leased-worktree case, simulated without touching any real
    /// store, and independent of the machine it runs on. A temp project root
    /// carries a roster naming THIS process's resolved ambient identity (the
    /// same `current_user_id` the authority check uses) as advisor, which is
    /// the shape a leased worktree's `.aida-store` symlink exposes.
    ///
    /// - Positive control: with the seam pinned to that root, the roster grants
    ///   authority, so the seam really reaches the roster on any machine.
    /// - Guarded: `AmbientGuard::hermetic` must pin every input it promises
    ///   (asserted directly) and must refuse authority, although the seam still
    ///   points at a roster that would grant it to the ambient identity.
    ///
    /// If the guard regresses, both the postcondition asserts and the refusal
    /// fail, whatever the ambient role, roster or `$USER`.
    // trace:BUG-1618 | ai:claude
    #[test]
    fn hermetic_guard_defeats_a_leased_advisor_roster() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        std::fs::create_dir_all(store.join("registry")).unwrap();

        // Pin the seam to the fake root for the whole test (thread-local, so no
        // lock is needed). The guard below re-pins it; without the guard, the
        // refusal check would still resolve against this advisor roster.
        let outer = crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
            project_root: tmp.path().to_path_buf(),
            stdin_is_terminal: false,
            stdout_is_terminal: false, // trace:BUG-1667 | ai:claude
            stderr_is_terminal: false,
        }));

        // Positive control: clear the role inputs (not the identity), resolve
        // the ambient identity exactly as the authority check does, and seat
        // it through a validated advisor grant (ADR-66: the roster is only
        // the grant's ceiling — a roster row alone seats nobody).
        // trace:STORY-1473 | ai:claude
        // TASK-1743: pin the identity under ENV_LOCK instead of reading the
        // process-wide AIDA_USER unlocked; a concurrent hermetic test could
        // otherwise set it to the hermetic id mid-read and flake this test.
        // trace:TASK-1743 | ai:claude
        const AMBIENT_ADVISOR: &str = "task-1743-ambient-advisor";
        let ambient_user = {
            let user = AMBIENT_ADVISOR.to_string();
            assert_ne!(
                user, HERMETIC_TEST_USER,
                "ambient identity is the hermetic id"
            );
            let grant_id = crate::gate::seat_authority::test_support::mint_grant_for(
                tmp.path(),
                &user,
                "advisor",
                &[],
            );
            let _env = EnvVarsGuard::apply(&[
                ("AIDA_USER", Some(AMBIENT_ADVISOR)),
                ("AIDA_SESSION_ROLE", None),
                (
                    crate::gate::seat_authority::GRANT_ENV,
                    Some(grant_id.as_str()),
                ),
                ("AIDA_ROLE_INSTANCE", None),
                ("AIDA_AUTO_COMPLETE", None),
                ("AIDA_AUTO_COMPLETE_TOKEN", None),
            ]);
            assert!(
                crate::gate::role_authority::has_advisor_authority(),
                "the granted seat must give the ambient identity {user:?} advisor authority"
            );
            user
        };

        {
            let _ambient = AmbientGuard::hermetic(tmp.path(), None);
            // The guard's postconditions, asserted directly.
            assert_eq!(
                std::env::var("AIDA_USER").ok().as_deref(),
                Some(HERMETIC_TEST_USER)
            );
            for key in [
                "AIDA_SESSION_ROLE",
                crate::gate::seat_authority::GRANT_ENV, // trace:STORY-1473 | ai:claude
                "AIDA_ROLE_INSTANCE",
                "AIDA_AUTO_COMPLETE",
                "AIDA_AUTO_COMPLETE_TOKEN",
            ] {
                assert!(std::env::var_os(key).is_none(), "{key} must be unset");
            }
            assert_eq!(crate::test_ambient::stdin_is_terminal(), Some(false));
            assert_eq!(crate::test_ambient::stdout_is_terminal(), Some(false)); // trace:BUG-1667 | ai:claude
            assert_eq!(
                crate::test_ambient::project_root().as_deref(),
                Some(tmp.path())
            );
            assert!(
                !crate::gate::role_authority::has_advisor_authority(),
                "the hermetic guard must not inherit the granted advisor seat of {ambient_user:?}"
            );
        }

        crate::test_ambient::replace(outer);
    }
}
