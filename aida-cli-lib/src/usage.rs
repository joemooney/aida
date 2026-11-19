// trace:TASK-3783 | ai:codex
pub use aida_term::usage::*;

#[cfg(test)]
mod task3112_tests {
    use super::*;
    // Derive expected arity from clap so new GlobalArgs cannot silently drift.
    // trace:TASK-3112 | ai:codex
    #[test]
    fn task3112_root_flag_table_matches_clap() {
        let command =
            <crate::cli::GlobalArgs as clap::Args>::augment_args(clap::Command::new("aida"));
        for arg in command.get_arguments() {
            if arg.get_id() == "path" {
                continue;
            }
            let takes_value = arg.get_action().takes_values();
            let mut flags = Vec::new();
            if let Some(long) = arg.get_long() {
                flags.push(format!("--{long}"));
            }
            if let Some(short) = arg.get_short() {
                flags.push(format!("-{short}"));
            }
            for flag in flags {
                let mut argv = vec!["aida".into(), flag.clone()];
                if takes_value {
                    argv.push("private".into());
                }
                argv.extend(["show".into(), "TASK-1".into()]);
                assert_eq!(derive_cmd_shape(&argv), "show", "{flag}");
                if takes_value {
                    let attached = if flag.starts_with("--") {
                        format!("{flag}=private")
                    } else {
                        format!("{flag}private")
                    };
                    let argv = vec!["aida".into(), attached, "show".into(), "TASK-1".into()];
                    assert_eq!(derive_cmd_shape(&argv), "show", "{flag}");
                }
            }
        }
    }
}

#[cfg(test)]
mod env_tests {
    use super::*;
    #[test]
    fn schedule_invocation_source_none_for_other_commands() {
        let _guard = crate::test_env::EnvVarGuard::unset("AIDA_SCHEDULE_INVOKER");
        let argv = vec!["aida".to_string(), "queue".to_string(), "list".to_string()];
        assert_eq!(schedule_invocation_source(&argv), None);
    }
    #[test]
    fn schedule_invocation_source_hook_flag() {
        let _guard = crate::test_env::EnvVarGuard::unset("AIDA_SCHEDULE_INVOKER");
        let argv = vec![
            "aida".to_string(),
            "schedule".to_string(),
            "tick".to_string(),
            "--hook".to_string(),
        ];
        assert_eq!(schedule_invocation_source(&argv), Some("hook".to_string()));
    }
    #[test]
    fn schedule_invocation_source_manual_without_hook_or_env() {
        let _guard = crate::test_env::EnvVarGuard::unset("AIDA_SCHEDULE_INVOKER");
        let argv = vec![
            "aida".to_string(),
            "schedule".to_string(),
            "tick".to_string(),
        ];
        assert_eq!(
            schedule_invocation_source(&argv),
            Some("manual".to_string())
        );
    }
    #[test]
    fn schedule_invocation_source_env_wins_and_names_cron() {
        let _guard = crate::test_env::EnvVarGuard::set("AIDA_SCHEDULE_INVOKER", "cron");
        let argv = vec![
            "aida".to_string(),
            "schedule".to_string(),
            "tick".to_string(),
        ];
        assert_eq!(schedule_invocation_source(&argv), Some("cron".to_string()));
    }
    #[test]
    fn is_enabled_respects_env_kill_switch() {
        // TASK-521: serialise AIDA_TELEMETRY swaps under the shared
        // ENV_LOCK so a parallel test that emits telemetry doesn't
        // observe the kill-switch we set here. The guard restores the
        // prior value on drop. trace:TASK-521 | ai:claude
        let mut guard = crate::test_env::EnvVarGuard::set("AIDA_TELEMETRY", "0");
        assert!(!is_enabled(None));
        guard.reset("off");
        assert!(!is_enabled(None));
        guard.reset_unset();
        // No env, no project dir → default enabled.
        assert!(is_enabled(None));
    }
}

// trace:TASK-3783 | ai:codex
pub fn log_path() -> Option<std::path::PathBuf> {
    aida_term::usage::log_path(crate::aida_home_dir().as_deref())
}
pub fn append_event(event: &UsageEvent) {
    aida_term::usage::append_event(event, crate::aida_home_dir().as_deref());
}
pub fn append_value(value: &serde_json::Value) {
    aida_term::usage::append_value(value, crate::aida_home_dir().as_deref());
}
pub fn read_events() -> Vec<UsageEvent> {
    aida_term::usage::read_events(crate::aida_home_dir().as_deref())
}
