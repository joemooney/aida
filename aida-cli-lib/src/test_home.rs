// trace:TASK-3784 | ai:codex
pub(crate) use aida_testkit::test_home::*;
#[cfg(test)]
mod tests {
    use super::*;
    /// The resolver that leaked in BUG-1642: with no override set, the global
    /// roles dir (where rework / queue-add record activity) is under the temp
    /// home.
    #[test]
    fn global_roles_dir_resolves_under_the_temp_home() {
        let _env =
            crate::test_env::EnvVarsGuard::apply(&[("AIDA_TEST_HOME", None), ("AIDA_HOME", None)]);
        let dir = crate::global_roles_dir().expect("roles dir");
        assert!(dir.starts_with(test_home()), "{}", dir.display());
        let presence = crate::presence::presence_path().expect("presence path");
        assert!(presence.starts_with(test_home()), "{}", presence.display());
        let clock = crate::presence::turn_clock_dir().expect("turn clock");
        assert!(clock.starts_with(test_home()), "{}", clock.display());
    }
}
