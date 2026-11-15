#[cfg(test)]
mod tests {
    use super::*;

    // BUG-581: every test that reads or mutates os_wrap touches the
    // process-global `AIDA_OS_WRAP` env var (via `os_wrap_env_override`). cargo
    // runs tests in parallel threads sharing ONE process environment, so a
    // mutator test mid-flight (with AIDA_OS_WRAP set) leaks into reader tests
    // that assume a clean baseline and they fail intermittently in CI. This
    // BUG-697: the `OsWrapEnvGuard` RAII helper below acquires the ONE shared
    // process-global env lock (crate::test_env::env_lock) so os_wrap swaps
    // can't race a read/swap under any other test helper. trace:BUG-581

    /// RAII guard for the os_wrap env tests (BUG-581). On construction it locks
    /// the shared `OS_WRAP_ENV_LOCK` (recovering from a poisoned lock so one
    /// panicking test can't cascade), saves the ambient `AIDA_OS_WRAP`, and
    /// REMOVES it so the test starts from a clean baseline. On drop it restores
    /// the saved value, so the rest of the suite is unaffected. Every os_wrap
    /// test acquires this at its top; mutators still set/remove the var within
    /// their body — those changes are also undone by the drop restore.
    /// trace:BUG-581 | ai:claude
    // BUG-697 / TASK-1532: route through EnvVarGuard. trace:BUG-581 trace:TASK-1532 | ai:agy
    struct OsWrapEnvGuard {
        _guard: crate::test_env::EnvVarGuard,
    }

    impl OsWrapEnvGuard {
        fn acquire() -> Self {
            Self {
                _guard: crate::test_env::EnvVarGuard::unset("AIDA_OS_WRAP"),
            }
        }

        fn set(&mut self, val: impl AsRef<std::ffi::OsStr>) {
            self._guard.reset(val);
        }

        fn unset(&mut self) {
            self._guard.reset_unset();
        }
    }

    #[test]
    fn stamp_lease_active_pid_records_spawned_child_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join(".aida").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let lease = sessions.join("lease741.toml");
        std::fs::write(
            &lease,
            "id = \"lease741\"\nscope = \"BUG-741\"\ncreator_pid = 111\n",
        )
        .unwrap();

        stamp_lease_active_pid_at(tmp.path(), "lease741", 4242).unwrap();

        let doc = std::fs::read_to_string(lease)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(doc["creator_pid"].as_integer(), Some(111));
        assert_eq!(doc["active_pid"].as_integer(), Some(4242));
    }

    #[test]
    fn stamp_lease_active_pid_records_process_start_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join(".aida").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let lease = sessions.join("lease1284.toml");
        std::fs::write(&lease, "id = \"lease1284\"\nscope = \"TASK-1284\"\n").unwrap();

        let pid = std::process::id();
        stamp_lease_active_pid_at(tmp.path(), "lease1284", pid).unwrap();

        let doc = std::fs::read_to_string(lease)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(doc["active_pid"].as_integer(), Some(i64::from(pid)));
        assert_eq!(
            doc["active_pid_start_time"].as_str(),
            crate::process_probe::process_start_identity(pid).as_deref()
        );
    }

    /// RAII guard for the `AIDA_AGENT_CMD` resolver tests. It shares the
    /// `ENV_LOCK` so it is mutually exclusive with the os_wrap
    /// program-resolution tests — those call `claude_program_and_args`, which now
    /// reads `AIDA_AGENT_CMD`, so a concurrently-set override must never leak into
    /// their clean-baseline `program == "claude"` assertions. Saves + clears both
    /// `AIDA_AGENT_CMD` and `AIDA_OS_WRAP` on construct (a clean, os_wrap-off
    /// baseline), restores both on drop.
    // trace:TASK-1081 trace:TASK-1532 | ai:agy
    struct AgentCmdEnvGuard {
        _guard: crate::test_env::EnvVarsGuard,
    }

    impl AgentCmdEnvGuard {
        fn acquire() -> Self {
            Self {
                _guard: crate::test_env::EnvVarsGuard::apply(&[
                    ("AIDA_AGENT_CMD", None),
                    ("AIDA_CONTAINED", None),
                    ("AIDA_OS_WRAP", None),
                ]),
            }
        }

        fn set_cmd(&mut self, val: impl AsRef<std::ffi::OsStr>) {
            self._guard.set_key("AIDA_AGENT_CMD", val);
        }

        fn unset_cmd(&mut self) {
            self._guard.unset_key("AIDA_AGENT_CMD");
        }
    }

    /// With `AIDA_AGENT_CMD` unset the resolver yields the native vendor binary
    /// (faithful-launcher invariant); set at a trivial fake exe it yields the
    /// fake, and the spawn site passes the ORIGINAL argv through unchanged. No
    /// real claude/codex is spawned — the resolver never launches the program,
    /// it only names it.
    // trace:TASK-1081 | ai:claude
    #[test]
    fn agent_cmd_override_swaps_program_keeps_argv() {
        let mut _env = AgentCmdEnvGuard::acquire();

        // Unset → the native vendor binaries, byte-identical to today.
        assert_eq!(resolve_agent_program("claude"), "claude");
        assert_eq!(resolve_agent_program("codex"), "codex");

        // Set at a trivial fake exe → that program replaces every vendor binary.
        let fake = "/tmp/aida-fake-agent-task-1081";
        _env.set_cmd(fake);
        assert_eq!(resolve_agent_program("claude"), fake);
        assert_eq!(resolve_agent_program("codex"), fake);

        // The headless spawn site swaps only the PROGRAM; the argv (all flags +
        // the trailing prompt positional) is passed through unchanged. os_wrap is
        // off (guard cleared AIDA_OS_WRAP; the temp root has no config).
        let tmp = tempfile::tempdir().unwrap();
        let claude_args = claude_headless_args("/aida-review", "sid");
        let (program, args) = claude_program_and_args(None, tmp.path(), claude_args.clone())
            .expect("unwrapped launch never errors");
        assert_eq!(program, fake, "override swaps the vendor program");
        assert_eq!(args, claude_args, "argv is passed through unchanged");

        // Empty / whitespace override is treated as unset — fall back to native.
        _env.set_cmd("   ");
        assert_eq!(resolve_agent_program("claude"), "claude");
        _env.unset_cmd();
        assert_eq!(resolve_agent_program("claude"), "claude");
    }

    // TASK-865: the doctor/init bwrap row distils bwrap_preflight's long
    // fail-closed message down to a single clean remediation line — the first
    // concrete `sysctl` step, with the parenthetical aside and soft-wrap
    // whitespace stripped. trace:TASK-865 | ai:claude
    #[test]
    fn bwrap_remediation_hint_extracts_first_concrete_step() {
        let full = "[contained] os_wrap is enabled and `bwrap` is installed, but a sandbox \
                    self-test failed. Remediate with ONE of:\n  \
                    - sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0   (host-wide)\n  \
                    - install an AppArmor profile granting bwrap userns";
        let hint = bwrap_userns_remediation_hint(full);
        assert_eq!(
            hint,
            "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0"
        );
    }

    // STORY-665: the shared remediation constants are the single source of
    // truth for the doctor remediation, the `--fix-sandbox` printer, and the
    // docs. Pin their exact text so a copy-paste from any surface is correct
    // and they can't silently drift apart. trace:STORY-665 | ai:claude
    #[test]
    fn bwrap_remediation_constants_are_exact_and_copy_pasteable() {
        assert_eq!(
            BWRAP_USERNS_SYSCTL_RUNTIME,
            "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0"
        );
        assert_eq!(
            BWRAP_USERNS_SYSCTL_PERSIST,
            "echo 'kernel.apparmor_restrict_unprivileged_userns=0' \
             | sudo tee /etc/sysctl.d/99-aida-bwrap-userns.conf"
        );
        assert_eq!(BWRAP_INSTALL_DEBIAN, "sudo apt install bubblewrap");
        // The persist drop-in must land under /etc/sysctl.d so it survives a
        // reboot, and the runtime form must be the non-persisting `sysctl -w`.
        assert!(BWRAP_USERNS_SYSCTL_PERSIST.contains("/etc/sysctl.d/"));
        assert!(BWRAP_USERNS_SYSCTL_RUNTIME.contains("sysctl -w"));
    }

    // TASK-865: when the expected marker is absent, fall back to a complete
    // self-contained hint rather than returning an empty string.
    #[test]
    fn bwrap_remediation_hint_falls_back_without_marker() {
        let hint = bwrap_userns_remediation_hint("some unexpected error text");
        assert!(hint.contains("apparmor_restrict_unprivileged_userns"));
        assert!(!hint.is_empty());
    }

    // STORY-605: an EMPTY egress allowlist must leave the contained settings
    // unchanged — the network-egress restriction is strictly opt-in. The
    // parsed sandbox object must carry exactly the four pre-STORY-605 keys and
    // NO `network` key. (If this fails, the default contained posture changed.)
    // trace:STORY-605 | ai:claude
    #[test]
    fn empty_egress_allowlist_omits_network_key_unchanged() {
        let json = contained_settings_json(&[], false, &[]);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let sandbox = v.get("sandbox").and_then(|s| s.as_object()).unwrap();
        assert!(
            sandbox.get("network").is_none(),
            "empty allowlist must omit the network key (unchanged posture): {json}"
        );
        assert_eq!(sandbox.get("enabled"), Some(&serde_json::json!(true)));
        assert_eq!(
            sandbox.get("allowUnsandboxedCommands"),
            Some(&serde_json::json!(false))
        );
        // The unchanged set is exactly these four keys.
        let mut keys: Vec<&String> = sandbox.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "allowUnsandboxedCommands",
                "autoAllowBashIfSandboxed",
                "enabled",
                "failIfUnavailable",
            ]
        );
    }

    // STORY-615: managed_domains_only=true adds
    // `sandbox.network.allowManagedDomainsOnly`; composes with the allowlist;
    // default (false) + empty allowlist still omits the network key entirely.
    // trace:STORY-615 | ai:claude
    #[test]
    fn managed_domains_only_adds_allow_managed_flag() {
        // managed-only with NO allowlist → network carries only the flag.
        let json = contained_settings_json(&[], true, &[]);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.pointer("/sandbox/network/allowManagedDomainsOnly"),
            Some(&serde_json::json!(true))
        );
        assert!(
            v.pointer("/sandbox/network/allowedDomains").is_none(),
            "no allowlist → no allowedDomains key: {json}"
        );
        // composes with an allowlist (both keys present).
        let json = contained_settings_json(&["github.com".to_string()], true, &[]);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.pointer("/sandbox/network/allowManagedDomainsOnly"),
            Some(&serde_json::json!(true))
        );
        assert!(v.pointer("/sandbox/network/allowedDomains").is_some());
        // default off + empty allowlist → network omitted (unchanged posture).
        let json = contained_settings_json(&[], false, &[]);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.pointer("/sandbox/network").is_none());
    }

    // TASK-809: the MANAGED-settings doc carries the hard default-deny flag
    // `sandbox.network.allowManagedDomainsOnly = true`, and mirrors the operator
    // `allowed_hosts` into `sandbox.network.allowedDomains` (managed-only honors
    // only the managed allowedDomains). With an empty allowlist the
    // allowedDomains key is omitted but the hard-block flag is still present.
    // trace:TASK-809 | ai:claude
    #[test]
    fn managed_settings_json_hard_blocks_and_mirrors_hosts() {
        // empty allowlist → flag present, no allowedDomains key.
        let json = managed_settings_json(&[]);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.pointer("/sandbox/network/allowManagedDomainsOnly"),
            Some(&serde_json::json!(true)),
            "managed doc must hard-block (deny without prompt): {json}"
        );
        assert!(
            v.pointer("/sandbox/network/allowedDomains").is_none(),
            "no allowlist → no allowedDomains key: {json}"
        );
        // non-empty allowlist → mirrored into the MANAGED allowedDomains.
        let hosts = vec!["github.com".to_string(), "*.crates.io".to_string()];
        let json = managed_settings_json(&hosts);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.pointer("/sandbox/network/allowManagedDomainsOnly"),
            Some(&serde_json::json!(true))
        );
        let got: Vec<&str> = v
            .pointer("/sandbox/network/allowedDomains")
            .and_then(|d| d.as_array())
            .expect("managed allowedDomains present")
            .iter()
            .filter_map(|d| d.as_str())
            .collect();
        assert_eq!(got, vec!["github.com", "*.crates.io"]);
    }

    // trace:STORY-994 | ai:codex
    #[cfg(unix)]
    #[test]
    fn run_claude_session_restores_terminal_title_after_child_exit() {
        // trace:BUG-1994 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        use std::os::unix::fs::PermissionsExt;

        fn write_executable(path: &std::path::Path, body: &str) {
            std::fs::write(path, body).unwrap();
            let mut permissions = std::fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions).unwrap();
        }

        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = tmp.path().join("calls.log");
        write_executable(
            &bin.join("claude"),
            &format!("#!/bin/sh\nprintf 'child-exit\\n' >> '{}'\n", log.display()),
        );
        write_executable(
            &bin.join("tmux"),
            &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", log.display()),
        );
        let old_path = std::env::var("PATH").unwrap_or_default();
        let path = format!("{}:{old_path}", bin.display());
        let _env = crate::test_env::EnvVarsGuard::set(&[("PATH", &path)]);
        let terminal = crate::agent_registry::terminal_identity_from_env(
            Some("/dev/pts/4".to_string()),
            vec![("TMUX_PANE", "%3"), ("TMUX", "/tmp/tmux.sock,1,0")],
        )
        .unwrap();
        let restore = crate::agent_registry::TerminalTitleRestore {
            terminal: Some(terminal),
            previous_title: Some("before launch".to_string()),
        };

        run_claude_session(
            crate::seat_clone::launcher_main().as_deref(),
            None,
            None,
            None,
            None,
            false,
            None,
            Some(restore),
        )
        .unwrap();

        let calls = std::fs::read_to_string(log).unwrap();
        assert_eq!(calls.trim(), "child-exit\nrename-window before launch");
    }

    // TASK-809: the os_wrap launch binds the generated managed-settings doc over
    // `/etc/claude-code/managed-settings.json` (hard `--ro-bind`) ONLY when
    // `[contained] managed_domains_only = true`; absent the flag the launch has
    // no such bind (unchanged STORY-612 posture). trace:TASK-809 | ai:claude
    #[test]
    fn os_wrap_binds_managed_settings_only_when_opted_in() {
        // Clean, serialized os_wrap env baseline (BUG-581). trace:BUG-581
        let _env = OsWrapEnvGuard::acquire();
        // Helper: build a worktree with the given config, return the bwrap argv
        // (or None when the host can't create a userns / bwrap is absent — then
        // the launch fails closed and we can't inspect the argv, so we skip).
        fn wrapped_args(cfg: &str) -> Option<Vec<String>> {
            let tmp = tempfile::tempdir().unwrap();
            let wt = tmp.path();
            std::fs::create_dir_all(wt.join(".git")).unwrap();
            std::fs::create_dir_all(wt.join(".aida")).unwrap();
            std::fs::write(wt.join(".aida/config.toml"), cfg).unwrap();
            if which_on_path("bwrap").is_none() || bwrap_preflight().is_err() {
                return None;
            }
            let dummy = claude_headless_args("/aida-review", "sid");
            let (program, args) =
                claude_program_and_args(None, wt, dummy).expect("wrapped launch builds");
            assert_eq!(program, "bwrap");
            Some(args)
        }

        const MANAGED_DEST: &str = "/etc/claude-code/managed-settings.json";

        // managed_domains_only ON → a hard --ro-bind onto the managed dest.
        if let Some(args) =
            wrapped_args("[contained]\nos_wrap = true\nmanaged_domains_only = true\n")
        {
            let bound = args
                .windows(3)
                .any(|w| w[0] == "--ro-bind" && w[2] == MANAGED_DEST);
            assert!(
                bound,
                "managed_domains_only must --ro-bind the managed settings doc: {args:?}"
            );
        }

        // os_wrap on but managed_domains_only OFF → NO managed-settings bind.
        if let Some(args) = wrapped_args("[contained]\nos_wrap = true\n") {
            assert!(
                !args.iter().any(|a| a == MANAGED_DEST),
                "no managed bind without managed_domains_only (unchanged posture): {args:?}"
            );
        }
    }

    // STORY-617: with an EMPTY read_allowlist the confinement args are
    // byte-for-byte the pre-STORY-617 base — whole-fs `--ro-bind / /`, no
    // `--ro-bind-try` for system paths. (If this fails the default read posture
    // changed.) trace:STORY-617 | ai:claude
    #[test]
    fn empty_read_allowlist_keeps_ro_root_unchanged() {
        let wt = Path::new("/home/joe/ai/aida-story-617");
        let store = wt.join(".aida-store");
        let with_helper = bwrap_confinement_args(wt, std::slice::from_ref(&store));
        let with_inner = bwrap_confinement_args_inner(wt, std::slice::from_ref(&store), &[]);
        assert_eq!(
            with_helper, with_inner,
            "empty allowlist must equal the no-allowlist helper output"
        );
        // ro-root present, first.
        let ro = with_inner
            .windows(3)
            .position(|w| w == ["--ro-bind", "/", "/"])
            .expect("must --ro-bind / / when no read_allowlist");
        assert_eq!(ro, 0, "ro-root must come first: {with_inner:?}");
        // no enumerated essential-path binds in the unchanged posture.
        assert!(
            !with_inner.iter().any(|a| a == "--ro-bind-try"),
            "default posture must not enumerate system paths: {with_inner:?}"
        );
    }

    // STORY-617: a NON-EMPTY read_allowlist replaces `--ro-bind / /` with an
    // enumerated set — essential system paths + the allowlist (all ro via
    // --ro-bind-try) — while still rw-binding the worktree and NEVER unsharing
    // net. Host secrets outside the allowlist are simply absent. trace:STORY-617
    #[test]
    fn nonempty_read_allowlist_enumerates_and_drops_ro_root() {
        let wt = Path::new("/home/joe/ai/aida-story-617");
        let store = wt.join(".aida-store");
        let allow = vec![
            "/home/joe/.config/special".to_string(),
            "/data/shared".to_string(),
        ];
        let flags = bwrap_confinement_args_inner(wt, std::slice::from_ref(&store), &allow);

        // (a) the broad ro-root is GONE — strict default-absent filesystem.
        assert!(
            !flags.windows(3).any(|w| w == ["--ro-bind", "/", "/"]),
            "strict mode must NOT --ro-bind / /: {flags:?}"
        );

        // (b) each allowlist entry is bound ro via --ro-bind-try (skips if absent).
        for p in &allow {
            assert!(
                flags
                    .windows(3)
                    .any(|w| w[0] == "--ro-bind-try" && w[1] == *p && w[2] == *p),
                "allowlist path {p} must be ro-bind-try'd: {flags:?}"
            );
        }
        // (c) the essential toolchain paths are present (so claude/cargo run).
        for must in ["/usr", "/etc", "/lib"] {
            assert!(
                flags
                    .windows(3)
                    .any(|w| w[0] == "--ro-bind-try" && w[1] == must && w[2] == must),
                "essential path {must} must be bound: {flags:?}"
            );
        }
        // (d) a host-secret path NOT in the allowlist is absent entirely.
        assert!(
            !flags.iter().any(|a| a.contains(".ssh")),
            "non-allowlisted ~/.ssh must not appear: {flags:?}"
        );

        // worktree still rw-bound; net still shared.
        let wt_s = wt.to_string_lossy().to_string();
        assert!(
            flags
                .windows(3)
                .any(|w| w[0] == "--bind" && w[1] == wt_s && w[2] == wt_s),
            "worktree must stay rw --bind in strict mode: {flags:?}"
        );
        assert!(
            !flags.iter().any(|a| a == "--unshare-net"),
            "strict read-confinement must NOT unshare net: {flags:?}"
        );
    }

    // STORY-617: the config reader is opt-in — absent key => empty => unchanged.
    // trace:STORY-617 | ai:claude
    #[test]
    fn read_allowlist_config_is_opt_in() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path();
        // absent config => empty.
        assert!(contained_read_allowlist(wt).is_empty());
        // present array => parsed.
        std::fs::create_dir_all(wt.join(".aida")).unwrap();
        std::fs::write(
            wt.join(".aida/config.toml"),
            "[contained]\nread_allowlist = [\"/data/a\", \"/data/b\"]\n",
        )
        .unwrap();
        assert_eq!(
            contained_read_allowlist(wt),
            vec!["/data/a".to_string(), "/data/b".to_string()]
        );
    }

    // STORY-605: a non-empty allowlist adds `sandbox.network.allowedDomains`
    // (Claude Code's verified egress schema) with the given hosts, leaving the
    // base sandbox keys intact. trace:STORY-605 | ai:claude
    #[test]
    fn nonempty_egress_allowlist_adds_allowed_domains() {
        let hosts = vec!["github.com".to_string(), "*.crates.io".to_string()];
        let json = contained_settings_json(&hosts, false, &[]);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let domains = v
            .pointer("/sandbox/network/allowedDomains")
            .and_then(|d| d.as_array())
            .expect("network.allowedDomains present");
        let got: Vec<&str> = domains.iter().filter_map(|d| d.as_str()).collect();
        assert_eq!(got, vec!["github.com", "*.crates.io"]);
        // Base sandbox keys still intact.
        assert_eq!(
            v.pointer("/sandbox/enabled"),
            Some(&serde_json::json!(true))
        );
    }

    // TASK-402 (friction #2): a session JSONL that echoed the deprecated
    // `dialog` alias must report the canonical `advisor` role in
    // `--list-sessions`, so a recovery operator doesn't think the orchestrator
    // launched the wrong role. Other roles pass through unchanged.
    // trace:TASK-402 | ai:claude
    #[test]
    fn canonical_session_role_normalizes_dialog_alias() {
        assert_eq!(canonical_session_role("dialog"), "advisor");
        assert_eq!(canonical_session_role("Dialog"), "advisor");
        assert_eq!(canonical_session_role("DIALOG"), "advisor");
        // Real roles are untouched.
        assert_eq!(canonical_session_role("implementer"), "implementer");
        assert_eq!(canonical_session_role("advisor"), "advisor");
        assert_eq!(canonical_session_role("reviewer"), "reviewer");
    }

    fn parse_session_role_from_lines(lines: &[&str]) -> Option<String> {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session-id.jsonl");
        std::fs::write(&path, lines.join("\n")).unwrap();
        let now = SystemTime::now();
        parse_session_meta(&path, now, now).unwrap().role
    }

    // BUG-837: AIDA's own SessionStart/UserPromptSubmit hooks record role
    // markers in hook text formats that the session picker must understand.
    // trace:BUG-837 | ai:codex
    #[test]
    fn session_hook_status_role_marker_resolves_role() {
        assert_eq!(
            parse_session_role_from_lines(&[r#"{"message":"AIDA · role: product"}"#]),
            Some("product".to_string())
        );
    }

    // BUG-837: the additional-context header is enough role evidence by itself.
    // trace:BUG-837 | ai:codex
    #[test]
    fn session_hook_context_role_marker_resolves_role() {
        assert_eq!(
            parse_session_role_from_lines(&[r#"{"message":"AIDA active role: implementer"}"#]),
            Some("implementer".to_string())
        );
    }

    // BUG-837: `none` is an explicit no-role sentinel, not an absent parse.
    // Once seen, later stray role-shaped strings must not overwrite it.
    // trace:BUG-837 | ai:codex
    #[test]
    fn session_hook_none_role_marker_stops_scan() {
        assert_eq!(
            parse_session_role_from_lines(&[
                r#"{"message":"AIDA · role: none"}"#,
                r#"{"message":"AIDA_SESSION_ROLE=implementer"}"#,
            ]),
            None
        );
    }

    // BUG-1593 (rework, PROXY DECISION): a tier-1 structural marker wins
    // even when a later line echoes a foreign, unanchored bare `Role: `
    // string — e.g. a tool result quoting someone else's status text. The
    // launch line here uses ONLY `AIDA_SESSION_ROLE=implementer`, a form
    // that contains no bare `Role: ` substring at all, so this test fails
    // if the two tiers are collapsed into one first-match-of-any scan: a
    // collapsed scan finds no marker on line 1 and would incorrectly
    // resolve to the foreign "product" on line 2 instead of "implementer".
    // trace:BUG-1593 | ai:claude
    #[test]
    fn foreign_echoed_role_string_does_not_override_launch_role() {
        assert_eq!(
            parse_session_role_from_lines(&[
                r#"{"message":"AIDA_SESSION_ROLE=implementer"}"#,
                r#"{"message":"tool output quoting someone else's status: Role: product"}"#,
            ]),
            Some("implementer".to_string())
        );
    }

    // BUG-1593 (rework, PROXY DECISION): a transcript with NO tier-1
    // structural marker anywhere but a bare `Role: ` line — exactly what
    // `.claude/skills/aida-pickup.md`, `aida-queue.md`, and `aida role
    // show` echo — still resolves the role via the tier-2 fallback. This
    // is the regression guard: dropping the bare marker outright (rather
    // than demoting it to a fallback) is what triggered the review BLOCK
    // on the first rework of this bug.
    // trace:BUG-1593 | ai:claude
    #[test]
    fn bare_skill_echoed_role_resolves_via_tier2_fallback() {
        assert_eq!(
            parse_session_role_from_lines(&[r#"{"message":"Role: implementer"}"#,]),
            Some("implementer".to_string())
        );
    }

    // BUG-1593: a role marker that only appears past the MAX_LINES cap must
    // never be found — the scan honours the cap regardless of whether role
    // (or title/started_at) resolved by then, so the result is `unknown`
    // (None) rather than a late, possibly-stale match from a still-growing
    // transcript.
    // trace:BUG-1593 | ai:claude
    #[test]
    fn role_marker_past_the_line_cap_is_unknown() {
        // MAX_LINES is 400 (private to `parse_session_meta_for_agent`); pad
        // well past it before the marker to land the marker outside the cap.
        let mut lines: Vec<String> = (0..410)
            .map(|n| format!(r#"{{"message":"filler line {n}"}}"#))
            .collect();
        lines[405] = r#"{"message":"- Role: implementer"}"#.to_string();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        assert_eq!(parse_session_role_from_lines(&refs), None);
    }

    // trace:STORY-822 | ai:codex
    #[test]
    fn codex_rollout_session_meta_supplies_agent_id_and_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("rollout-2026-09-03T08-39-39-01a067ec-facc-71a0-9a69-1476568e1f1e.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":"2026-09-03T15:39:39.268Z","type":"session_meta","payload":{"session_id":"01a067ec-facc-71a0-9a69-1476568e1f1e","timestamp":"2026-09-03T15:39:39.093Z","cwd":"/home/joe/ai/aida-story-822"}}"#,
        )
        .unwrap();
        let now = SystemTime::now();
        let meta = parse_session_meta_for_agent(&path, now, now, "codex").unwrap();

        assert_eq!(meta.agent, "codex");
        assert_eq!(meta.id, "01a067ec-facc-71a0-9a69-1476568e1f1e");
        assert_eq!(
            meta.last_cwd.as_deref(),
            Some("/home/joe/ai/aida-story-822")
        );
        assert!(meta.started_at.is_some());
    }

    /// BUG-1789: write a Codex rollout whose head is `head` followed by one
    /// unterminated line padded (sparsely) to `total` bytes — the shape of a
    /// real multi-MB single-line rollout event, without the disk cost.
    fn write_bug1789_rollout(path: &Path, head: &str, total: u64) {
        use std::io::Write;
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(head.as_bytes()).unwrap();
        f.write_all(b"{\"type\":\"response_item\",\"payload\":\"")
            .unwrap();
        f.set_len(total).unwrap();
    }

    fn bug1789_session_meta_line(id: &str, cwd: &Path) -> String {
        format!(
            "{{\"timestamp\":\"2026-09-03T15:39:39.268Z\",\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{id}\",\"timestamp\":\"2026-09-03T15:39:39.093Z\",\"cwd\":\"{}\"}}}}\n",
            cwd.display()
        )
    }

    // trace:BUG-1789 | ai:antigravity
    #[test]
    fn bug1789_parse_reads_at_most_the_per_file_byte_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("rollout-2026-09-03T08-39-39-01a067ec-facc-71a0-9a69-1476568e1f1e.jsonl");
        let head = format!(
            "{}{{\"message\":\"- Role: implementer\"}}\n",
            bug1789_session_meta_line("01a067ec-facc-71a0-9a69-1476568e1f1e", Path::new("/w/p"))
        );
        write_bug1789_rollout(&path, &head, 8 * 1024 * 1024);
        let now = SystemTime::now();
        let (meta, bytes) =
            parse_session_meta_counted(&path, now, now, "codex", None, None).unwrap();
        assert!(
            bytes <= SESSION_META_MAX_BYTES,
            "read {bytes} bytes, cap is {SESSION_META_MAX_BYTES}"
        );
        assert_eq!(meta.id, "01a067ec-facc-71a0-9a69-1476568e1f1e");
        assert_eq!(meta.last_cwd.as_deref(), Some("/w/p"));
        assert_eq!(meta.role.as_deref(), Some("implementer"));
    }

    // trace:BUG-1789 | ai:antigravity
    #[test]
    fn bug1789_first_line_truncated_by_cap_still_yields_cwd_and_id() {
        // A session_meta line larger than the cap is cut mid-line (and here
        // mid-multibyte-char): serde fails, so cwd/timestamp must come from
        // the substring scan and the id from the rollout filename.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("rollout-2026-09-03T08-39-39-01a067ec-facc-71a0-9a69-1476568e1f1e.jsonl");
        let mut line = String::from(
            r#"{"timestamp":"2026-09-03T15:39:39.268Z","type":"session_meta","payload":{"cwd":"/w/p","instructions":""#,
        );
        while (line.len() as u64) < SESSION_META_MAX_BYTES + 1024 {
            line.push('é');
        }
        line.push_str("\"}}\n");
        std::fs::write(&path, line).unwrap();
        let now = SystemTime::now();
        let meta = parse_session_meta_for_agent(&path, now, now, "codex").unwrap();
        assert_eq!(meta.id, "01a067ec-facc-71a0-9a69-1476568e1f1e");
        assert_eq!(meta.last_cwd.as_deref(), Some("/w/p"));
        assert!(meta.started_at.is_some());
    }

    // trace:BUG-1789 | ai:antigravity
    #[test]
    fn bug1789_role_scan_over_1500_multi_mb_rollouts_is_byte_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        let foreign = tmp.path().join("other-project");
        std::fs::create_dir_all(&project).unwrap();
        let base = SystemTime::now() - std::time::Duration::from_secs(100_000);
        let mut entries = Vec::new();
        for i in 0..1500u64 {
            let id = format!("01a067ec-facc-71a0-9a69-{i:012x}");
            let path = tmp
                .path()
                .join(format!("rollout-2026-09-03T08-39-39-{id}.jsonl"));
            // Three in-project advisor sessions (newest, behind 39 foreign
            // multi-MB rollouts, and the very oldest); the rest are foreign
            // projects. Foreign logs must cost only their head, so the scan
            // reaches all three without blowing the byte budget.
            let head = if [0, 40, 1499].contains(&i) {
                format!(
                    "{}{{\"message\":\"AIDA_SESSION_ROLE=advisor\"}}\n{{\"type\":\"ai-title\",\"aiTitle\":\"title {i}\"}}\n",
                    bug1789_session_meta_line(&id, &project)
                )
            } else {
                bug1789_session_meta_line(&id, &foreign)
            };
            write_bug1789_rollout(&path, &head, 3 * 1024 * 1024);
            entries.push(SessionLogEntry {
                path,
                mtime: base - std::time::Duration::from_secs(i),
                agent: "codex",
            });
        }
        let want = canonical_session_role("advisor");
        let mut filter =
            |m: &SessionMeta| m.role.as_deref().map(canonical_session_role) == Some(want.clone());
        let (rows, stats) = select_recent_sessions(
            entries,
            8,
            Some(SESSION_SCAN_BYTE_BUDGET),
            Some(want.as_str()),
            SystemTime::now(),
            Some(&project),
            &[],
            &mut filter,
        );
        assert!(stats.bytes_read <= SESSION_SCAN_BYTE_BUDGET);
        assert!(
            stats.bytes_read < 50_000_000,
            "scan read {} bytes over {} files",
            stats.bytes_read,
            stats.files_opened
        );
        assert_eq!(stats.files_opened, 1500);
        assert_eq!(rows.len(), 3);
        for (i, row) in [0, 40, 1499].iter().zip(rows.iter()) {
            assert_eq!(row.role.as_deref(), Some("advisor"));
            assert_eq!(row.title.as_deref(), Some(format!("title {i}").as_str()));
            assert_eq!(row.last_cwd.as_deref(), Some(project.to_str().unwrap()));
        }
    }

    // trace:TASK-1578 | ai:antigravity
    #[test]
    fn task1578_in_project_non_matching_tier1_breaks_early_and_reaches_older() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let base = SystemTime::now();
        let mut entries = Vec::new();
        for i in 0..200u64 {
            let id = format!("01a067ec-facc-71a0-9a69-{i:012x}");
            let path = tmp
                .path()
                .join(format!("rollout-2026-09-03T08-39-39-{id}.jsonl"));
            let head = format!(
                "{}{{\"message\":\"AIDA_SESSION_ROLE=implementer\"}}\n",
                bug1789_session_meta_line(&id, &project)
            );
            write_bug1789_rollout(&path, &head, 3 * 1024 * 1024);
            entries.push(SessionLogEntry {
                path,
                mtime: base - std::time::Duration::from_secs(i),
                agent: "codex",
            });
        }
        // Add an older matching session at the end.
        let id = format!("01a067ec-facc-71a0-9a69-{:012x}", 200);
        let path = tmp
            .path()
            .join(format!("rollout-2026-09-03T08-39-39-{id}.jsonl"));
        let head = format!(
            "{}{{\"message\":\"AIDA_SESSION_ROLE=advisor\"}}\n",
            bug1789_session_meta_line(&id, &project)
        );
        write_bug1789_rollout(&path, &head, 3 * 1024 * 1024);
        entries.push(SessionLogEntry {
            path,
            mtime: base - std::time::Duration::from_secs(200),
            agent: "codex",
        });

        let (rows, stats) = select_recent_sessions(
            entries,
            8,
            Some(SESSION_SCAN_BYTE_BUDGET),
            Some("advisor"),
            SystemTime::now(),
            Some(&project),
            &[],
            &mut |m: &SessionMeta| m.role.as_deref() == Some("advisor"),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].role.as_deref(), Some("advisor"));
        assert!(stats.bytes_read < SESSION_SCAN_BYTE_BUDGET);
        assert_eq!(stats.files_opened, 201);
    }

    // trace:BUG-1789 | ai:antigravity
    #[test]
    fn bug1789_scan_stops_once_limit_matches_found() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let base = SystemTime::now();
        let mut entries = Vec::new();
        for i in 0..50u64 {
            let id = format!("01a067ec-facc-71a0-9a69-{i:012x}");
            let path = tmp
                .path()
                .join(format!("rollout-2026-09-03T08-39-39-{id}.jsonl"));
            std::fs::write(&path, bug1789_session_meta_line(&id, &project)).unwrap();
            entries.push(SessionLogEntry {
                path,
                mtime: base - std::time::Duration::from_secs(i),
                agent: "codex",
            });
        }
        let (rows, stats) = select_recent_sessions(
            entries,
            8,
            None,
            None,
            SystemTime::now(),
            Some(&project),
            &[],
            &mut |_: &SessionMeta| true,
        );
        assert_eq!(rows.len(), 8);
        assert_eq!(stats.files_opened, 8);
    }

    // trace:STORY-822 | ai:codex
    #[test]
    fn format_session_line_includes_agent_column() {
        let m = SessionMeta {
            agent: "codex".to_string(),
            id: "01a067ec-facc-71a0-9a69-1476568e1f1e".to_string(),
            age_seconds: 90,
            role: Some("implementer".to_string()),
            spec: Some("STORY-822".to_string()),
            title: Some("agent-aware sessions".to_string()),
            started_at: None,
            last_cwd: None,
            terminal: None,
            branch: None,
            recent_focus: None,
            path: None,
            process: Default::default(),
        };

        let line = format_session_line(&m);
        assert!(line.contains("01a067ec"));
        assert!(line.contains("codex"));
        assert!(line.contains("implementer"));
    }

    // trace:STORY-822 | ai:codex
    #[test]
    fn non_claude_resume_target_uses_native_command_shape() {
        let target = ResumeTarget {
            agent: "codex".to_string(),
            id: "01a067ec-facc-71a0-9a69-1476568e1f1e".to_string(),
            worktree: None,
        };

        let (program, args) = resume_command_for(&target);
        assert_eq!(program, "codex");
        assert_eq!(
            args,
            vec![
                "resume".to_string(),
                "01a067ec-facc-71a0-9a69-1476568e1f1e".to_string()
            ]
        );
    }

    // trace:STORY-821 | ai:codex
    #[test]
    fn role_resume_session_line_includes_enriched_columns() {
        let m = SessionMeta {
            agent: "claude".to_string(),
            id: "019e45cf-acea-73c1-9476-abcdef012345".to_string(),
            age_seconds: 3600,
            role: Some("implementer".to_string()),
            spec: Some("STORY-821".to_string()),
            title: Some("resume prompt work".to_string()),
            started_at: None,
            last_cwd: Some("/home/joe/ai/aida-story-821".to_string()),
            terminal: None,
            branch: Some("story-821".to_string()),
            recent_focus: None,
            path: None,
            process: Default::default(),
        };

        let line = format_role_resume_session_line(&m);
        assert!(line.contains("019e45cf"));
        assert!(line.contains("1h"));
        assert!(line.contains("implementer"));
        assert!(line.contains("STORY-821"));
        assert!(line.contains("aida-story-821 @ story"));
        assert!(line.contains("resume prompt work"));
    }

    #[test]
    fn session_cwd_scoping_separates_sibling_project_trees() {
        // BUG-447: the reported bleed — a TASK-007 session in the deleted
        // `~/ai` project must NOT match a `~/ai/dummy1` lookup.
        let root = Path::new("/home/joe/ai/dummy1");
        // main worktree (cwd == root) and a subdir of it are in scope
        assert!(session_cwd_in_project("/home/joe/ai/dummy1", root));
        assert!(session_cwd_in_project("/home/joe/ai/dummy1/src", root));
        // the stale sibling-tree session is OUT of scope
        assert!(!session_cwd_in_project("/home/joe/ai-task-007", root));
        assert!(!session_cwd_in_project("/home/joe/ai", root));
    }

    #[test]
    fn session_cwd_scoping_includes_sibling_worktree() {
        // `aida queue work` creates `<parent>/<repo_name>-<slug>` worktrees;
        // their sessions belong to this project and must stay in scope.
        let root = Path::new("/home/joe/ai/dummy1");
        assert!(session_cwd_in_project("/home/joe/ai/dummy1-task-007", root));
        assert!(session_cwd_in_project(
            "/home/joe/ai/dummy1-task-007-some-title",
            root
        ));
        // a genuinely different sibling project (no shared `<root>-` prefix)
        // is out of scope.
        assert!(!session_cwd_in_project("/home/joe/ai/other", root));
    }

    /// BUG-112: build a manifest with `(spec, position, started_at)` items
    /// for the RECENT FOCUS precedence tests.
    fn manifest_with(
        items: &[(&str, u32, Option<chrono::DateTime<chrono::Utc>>)],
    ) -> crate::session_manifest::SessionManifest {
        use crate::session_manifest::{ManifestItem, SessionManifest};
        SessionManifest {
            session_id: "lease01".to_string(),
            planned_at: chrono::Utc::now(),
            plan_source: "test".to_string(),
            claude_session_id: None,
            batch_name: None,
            agent_name: None,
            plan: None,
            seat: None,
            items: items
                .iter()
                .map(|(spec, pos, started)| ManifestItem {
                    spec_id: spec.to_string(),
                    position: *pos,
                    status_at_plan: "Approved".to_string(),
                    started_at: *started,
                    completed_at: None,
                    note: None,
                })
                .collect(),
        }
    }

    /// BUG-112: the activity log is the live signal — its most-recent
    /// spec wins over the manifest's planned items.
    #[test]
    fn recent_focus_prefers_activity_log_over_manifest() {
        let m = manifest_with(&[("STORY-1", 1, None)]);
        assert_eq!(
            recent_focus_from(&m, Some("BUG-9")).as_deref(),
            Some("BUG-9")
        );
    }

    /// BUG-112: with no activity log, fall back to the most recently
    /// picked-up planned item — latest `started_at`, else highest
    /// `position`, and a started item always outranks an unstarted one.
    #[test]
    fn recent_focus_falls_back_to_latest_manifest_item() {
        let early: chrono::DateTime<chrono::Utc> = "2026-05-17T10:00:00Z".parse().unwrap();
        let late: chrono::DateTime<chrono::Utc> = "2026-05-17T12:00:00Z".parse().unwrap();
        let by_time = manifest_with(&[("TASK-1", 1, Some(early)), ("TASK-2", 2, Some(late))]);
        assert_eq!(recent_focus_from(&by_time, None).as_deref(), Some("TASK-2"));

        let by_position = manifest_with(&[("TASK-3", 1, None), ("TASK-4", 2, None)]);
        assert_eq!(
            recent_focus_from(&by_position, None).as_deref(),
            Some("TASK-4")
        );

        let started_beats_unstarted =
            manifest_with(&[("TASK-5", 1, Some(early)), ("TASK-6", 9, None)]);
        assert_eq!(
            recent_focus_from(&started_beats_unstarted, None).as_deref(),
            Some("TASK-5")
        );
    }

    /// BUG-112: a manifest with no items yields no focus → renders `-`.
    #[test]
    fn recent_focus_none_for_empty_manifest() {
        let m = manifest_with(&[]);
        assert_eq!(recent_focus_from(&m, None), None);
    }

    #[test]
    fn extract_str_basic() {
        let line = r#"{"type":"ai-title","aiTitle":"Hello world","sessionId":"abc"}"#;
        assert_eq!(
            extract_str(line, "\"aiTitle\":\""),
            Some("Hello world".into())
        );
    }

    #[test]
    fn extract_str_handles_escaped_quotes() {
        let line = r#"{"aiTitle":"Said \"hi\" to bob"}"#;
        assert_eq!(
            extract_str(line, "\"aiTitle\":\""),
            Some(r#"Said "hi" to bob"#.into())
        );
    }

    #[test]
    fn first_spec_id_finds_node_aware_form() {
        assert_eq!(
            first_spec_id("Working on FR-1-042 today"),
            Some("FR-1-042".into())
        );
        assert_eq!(
            first_spec_id("BUG-1-017 is fixed"),
            Some("BUG-1-017".into())
        );
        assert_eq!(
            first_spec_id("see EPIC-2 and TASK-1"),
            Some("EPIC-2".into())
        );
    }

    #[test]
    fn first_spec_id_skips_non_specs() {
        assert_eq!(first_spec_id("version 1-2 compatible"), None);
        assert_eq!(first_spec_id("not-spec-1 here"), None);
        // X is too short / not in the prefix list
        assert_eq!(first_spec_id("X-1 isn't a spec"), None);
    }

    #[test]
    fn humanize_age_buckets() {
        assert_eq!(humanize_age(5), "5s");
        assert_eq!(humanize_age(120), "2m");
        assert_eq!(humanize_age(7200), "2h");
        assert_eq!(humanize_age(2 * 86_400), "2d");
        assert_eq!(humanize_age(2 * 7 * 86_400), "2w");
    }

    #[test]
    fn claude_project_dir_encodes_path() {
        // We can only assert the encoded suffix since the dir prefix
        // depends on $HOME at test time.
        let dir = claude_project_dir(Path::new("/home/joe/ai/aida")).unwrap();
        assert!(dir.to_string_lossy().ends_with("-home-joe-ai-aida"));
    }

    /// FR-1-044: launch-log correlation finds the closest record within
    /// the time window and ignores records outside it.
    #[test]
    fn match_launch_picks_closest_within_window() {
        let base: chrono::DateTime<chrono::Utc> = "2026-05-04T18:00:00Z".parse().unwrap();
        let mk = |secs_offset: i64, title: &str| LaunchRecord {
            ts: base + chrono::Duration::seconds(secs_offset),
            role: "implementer".into(),
            title: title.into(),
        };
        let launches = vec![
            mk(-120, "way-before"),     // 2min before — outside 60s window
            mk(-10, "ten-before"),      // 10s before — inside window
            mk(45, "forty-five-after"), // 45s after — inside window, but farther than -10
            mk(200, "way-after"),       // outside window
        ];
        let session_started = base; // exactly base
        let m = match_launch(&launches, session_started, 60).unwrap();
        assert_eq!(m.title, "ten-before");
    }

    #[test]
    fn match_launch_returns_none_when_all_outside_window() {
        let base: chrono::DateTime<chrono::Utc> = "2026-05-04T18:00:00Z".parse().unwrap();
        let launches = vec![LaunchRecord {
            ts: base + chrono::Duration::seconds(-200),
            role: "implementer".into(),
            title: "old".into(),
        }];
        assert!(match_launch(&launches, base, 60).is_none());
    }

    #[test]
    fn sanitize_for_tsv_drops_tabs_newlines() {
        assert_eq!(sanitize_for_tsv("a\tb\nc"), "a b c");
        assert_eq!(sanitize_for_tsv("plain"), "plain");
    }

    /// STORY-59: liveness indicator buckets — `●` live (<5min), a recent
    /// marker (<1h), space for idle. The widths are visual; the test
    /// guards the bucket boundaries. The recent marker routes through the
    /// glyph registry (TASK-840). trace:STORY-59 | ai:claude
    #[test]
    fn liveness_indicator_buckets() {
        let recent = crate::glyphs::Glyph::InFlight.render(crate::glyphs::active_profile(None));
        assert_eq!(liveness_indicator(0), "●");
        assert_eq!(liveness_indicator(4 * 60 + 59), "●");
        assert_eq!(liveness_indicator(5 * 60), recent);
        assert_eq!(liveness_indicator(59 * 60), recent);
        assert_eq!(liveness_indicator(60 * 60), " ");
        assert_eq!(liveness_indicator(86_400), " ");
    }

    /// STORY-59: worktree label = "<basename> @ <branch>", truncated
    /// with `…` when it overflows the column width. Empty cwd → "-".
    /// Missing branch → just the basename.
    /// trace:STORY-59 | ai:claude
    #[test]
    fn worktree_label_formatting() {
        assert_eq!(format_worktree_label(None, None, 28), "-");
        assert_eq!(
            format_worktree_label(Some("/home/joe/ai/aida"), Some("main"), 28),
            "aida @ main"
        );
        assert_eq!(
            format_worktree_label(Some("/home/joe/ai/aida-epic-20"), None, 28),
            "aida-epic-20"
        );
        let truncated = format_worktree_label(
            Some("/home/joe/ai/aida-epic-20"),
            Some("epic-20-batch3-followups-with-more"),
            28,
        );
        assert!(truncated.chars().count() <= 28);
        assert!(truncated.ends_with('…'));
    }

    /// STORY-59: extract_str pulls cwd out of a Claude Code event line.
    /// We piggyback on the existing `extract_str` helper; this test
    /// guards the marker we use.
    /// trace:STORY-59 | ai:claude
    #[test]
    fn extract_cwd_from_event_line() {
        let line = r#"{"type":"user","cwd":"/home/joe/ai/aida-epic-20","message":{"role":"user"}}"#;
        assert_eq!(
            extract_str(line, "\"cwd\":\""),
            Some("/home/joe/ai/aida-epic-20".into())
        );
    }

    /// TASK-31: reviewer sessions get `review@<scope>` regardless of branch.
    #[test]
    fn derive_session_name_reviewer_uses_review_prefix() {
        assert_eq!(
            derive_session_name("PR-10", "pr-10", "reviewer"),
            Some("review@PR-10".to_string())
        );
        assert_eq!(
            derive_session_name("MR-7", "mr-7-anything", "reviewer"),
            Some("review@MR-7".to_string())
        );
    }

    /// TASK-31: implementer on epic-batch shape collapses to EPIC-N:batchM.
    #[test]
    fn derive_session_name_epic_batch_shape() {
        assert_eq!(
            derive_session_name("EPIC-20", "epic-20-batch11", "implementer"),
            Some("EPIC-20:batch11".to_string())
        );
        // Case-insensitive scope; batch must be `batch<digits>`.
        assert_eq!(
            derive_session_name("epic-20", "epic-20-batch12", "implementer"),
            Some("EPIC-20:batch12".to_string())
        );
    }

    /// TASK-31: implementer with branch tail beyond the scope-slug uses
    /// `<role>@<scope>:<suffix>`.
    #[test]
    fn derive_session_name_implementer_with_suffix() {
        assert_eq!(
            derive_session_name("FR-42", "fr-42-spike", "implementer"),
            Some("impl@FR-42:spike".to_string())
        );
    }

    /// TASK-31: when the branch matches the scope-slug exactly, fall back
    /// to the no-suffix form.
    #[test]
    fn derive_session_name_implementer_no_suffix() {
        assert_eq!(
            derive_session_name("FR-42", "fr-42", "implementer"),
            Some("impl@FR-42".to_string())
        );
    }

    /// TASK-31: long names are truncated to 64 chars.
    #[test]
    fn derive_session_name_truncates() {
        let scope = "feature:".to_string() + &"x".repeat(80);
        let name = derive_session_name(&scope, "feature-x", "implementer").unwrap();
        assert!(name.len() <= 64, "got len {}: {}", name.len(), name);
    }

    /// TASK-31: empty/sentinel role falls back to `session@<scope>`.
    #[test]
    fn derive_session_name_empty_role_uses_session_label() {
        assert_eq!(
            derive_session_name("FR-42", "fr-42-misc", "-"),
            Some("session@FR-42:misc".to_string())
        );
    }

    /// TASK-31: free-form scopes (not TYPE-NUM) keep their original case.
    #[test]
    fn derive_session_name_free_form_scope_keeps_case() {
        assert_eq!(
            derive_session_name("feature:auth", "feature-auth-login", "implementer"),
            Some("impl@feature:auth".to_string())
        );
    }

    // --- Headless launch (STORY-263) --------------------------------------

    /// SPIKE-7's mandatory flag set must all be present — a headless run that
    /// is missing any of these silently does nothing or hangs.
    #[test]
    fn claude_headless_args_has_spike7_mandatory_flags() {
        let args = claude_headless_args(
            "/aida-review --pr 7",
            "019e0000-0000-7000-8000-000000000000",
        );
        assert!(args.contains(&"-p".to_string()), "print mode: {args:?}");
        assert!(
            args.contains(&"bypassPermissions".to_string()),
            "permission mode (spike Q2): {args:?}"
        );
        assert!(
            args.contains(&"stream-json".to_string()),
            "stream-json output (spike Q6): {args:?}"
        );
        assert!(args.contains(&"--verbose".to_string()), "verbose: {args:?}");
        assert!(
            args.contains(&"--session-id".to_string()),
            "session id (spike Q9): {args:?}"
        );
        // The prompt and the session id both survive into the argv.
        assert!(args.contains(&"/aida-review --pr 7".to_string()));
        assert!(args.contains(&"019e0000-0000-7000-8000-000000000000".to_string()));
    }

    /// STORY-495: a native interactive launch (`permission_mode = None`)
    /// injects NO `--permission-mode` — Claude uses its own default posture.
    /// trace:STORY-495 | ai:claude
    #[test]
    fn claude_session_args_native_omits_permission_mode() {
        let args = claude_session_args(
            crate::seat_clone::launcher_main().as_deref(),
            None,
            None,
            Some("/aida-pickup"),
            Some("sid"),
            false,
            None,
        )
        .unwrap();
        assert!(
            !args.iter().any(|a| a == "--permission-mode"),
            "native launch must not inject --permission-mode: {args:?}"
        );
        // The session-id and prompt still thread through.
        assert!(args.contains(&"--session-id".to_string()), "{args:?}");
        assert!(args.contains(&"/aida-pickup".to_string()), "{args:?}");
    }

    /// STORY-495: an explicit mode is injected as `--permission-mode <m>`.
    /// trace:STORY-495 | ai:claude
    #[test]
    fn claude_session_args_some_injects_permission_mode() {
        let args = claude_session_args(
            crate::seat_clone::launcher_main().as_deref(),
            Some("bypassPermissions"),
            None,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let pos = args
            .iter()
            .position(|a| a == "--permission-mode")
            .expect("--permission-mode present");
        assert_eq!(
            args.get(pos + 1).map(String::as_str),
            Some("bypassPermissions")
        );
    }

    // TASK-895: the native interactive Codex argv is just the prompt
    // positional — Codex has no caller-minted `--session-id` /
    // TUI-addressable `--resume`, and the faithful-launcher default injects no
    // approval/sandbox bypass.
    // trace:TASK-895 | ai:claude
    #[test]
    fn codex_session_args_native_is_just_the_prompt_positional() {
        let args = codex_session_args("/aida-pickup", false, None);
        assert_eq!(args, vec![codex_inline_prompt("/aida-pickup").unwrap()]);
        assert!(!args.iter().any(|a| a == "--session-id"), "{args:?}");
        assert!(!args.iter().any(|a| a == "--resume"), "{args:?}");
        assert!(
            !args
                .iter()
                .any(|a| a == "--dangerously-bypass-approvals-and-sandbox"),
            "interactive codex must not force the bypass: {args:?}"
        );
    }

    // trace:BUG-2387 | ai:codex
    #[test]
    fn interactive_codex_argv_expands_aida_workflows() {
        for command in ["pickup", "guided-implement", "plan", "derisk"] {
            let prompt = format!("/aida-{command} BUG-2387");
            let args = codex_session_args(&prompt, false, Some("gpt-test"));
            assert_eq!(&args[..2], &["--model", "gpt-test"]);
            assert_eq!(args.len(), 3);
            let body = &args[2];
            assert!(body.contains("BUG-2387"), "{command}: arguments lost");
            assert!(!body.starts_with('/'), "{command}: unexpanded invocation");
            assert!(body.contains("Codex prompt arguments:"), "{command}");
        }
    }

    // trace:BUG-2387 | ai:codex
    #[test]
    fn interactive_codex_prompt_preserves_prose_and_handles_missing_bodies() {
        for prompt in [
            "plain prose\nwith trailing newline\n",
            "/help",
            "$aida-pickup BUG-2387",
            "Discuss /aida-review",
        ] {
            assert_eq!(codex_interactive_prompt(prompt), prompt);
        }
        for prompt in ["/aida-unknown BUG-2387", "/aida-advise PUNT-1"] {
            let body = codex_interactive_prompt(prompt);
            assert!(body.starts_with(&prompt.replacen('/', "$", 1)));
            assert!(body.contains("If it is unavailable, stop"));
        }
    }

    // BUG-2387 review round 2: only the intended invocation line expands —
    // the first line, or the first `/aida-*` line inside the launch-context
    // preamble paragraph. Pasted usage lines later in free-form prompt text
    // (column-0 `/aida-review --pr 7 ...`) stay verbatim.
    // trace:BUG-2387 | ai:claude
    #[test]
    fn interactive_codex_prompt_expands_only_the_invocation_line() {
        let usage = "/aida-review --pr 7   Explicit PR number\n/aida-plan BUG-1";
        // Free-form prose first: nothing expands, even column-0 usage lines.
        let prose = format!("Please explain these usages:\n{usage}");
        assert_eq!(codex_interactive_prompt(&prose), prose);

        // Invocation first: only line 0 expands; the pasted block is kept.
        let first = format!("/aida-pickup BUG-2387\n\nNotes:\n{usage}");
        let body = codex_interactive_prompt(&first);
        assert!(!body.starts_with('/'), "{body}");
        assert!(body.ends_with(&format!("Notes:\n{usage}")), "{body}");
        assert!(
            !body.contains("# Plan Requirement Implementation"),
            "second workflow inlined: {body}"
        );

        // Preamble: the invocation inside the preamble paragraph expands;
        // `/aida-*` lines after the paragraph break do not.
        let preamble = format!(
            "{}\ncat \"$AIDA_AGENT_CONTEXT_FILE\"\n/aida-derisk BUG-2387\n\nUsage:\n{usage}",
            crate::agent_launch_prompt::LAUNCH_CONTEXT_PREAMBLE
        );
        let body = codex_interactive_prompt(&preamble);
        assert!(
            !body.contains("$AIDA_AGENT_CONTEXT_FILE\"\n/aida-derisk"),
            "{body}"
        );
        assert!(
            body.contains("Codex prompt arguments: `BUG-2387`"),
            "{body}"
        );
        assert!(body.ends_with(&format!("Usage:\n{usage}")), "{body}");
    }

    // BUG-743: when the queue/do resolver reports the uniform
    // `[agents] bypass` posture, interactive Codex must get Codex's actual
    // bypass flag too, not only headless `codex exec`.
    // trace:BUG-743 | ai:codex
    #[test]
    fn codex_session_args_bypass_maps_to_codex_bypass_flag() {
        let args = codex_session_args("/aida-pickup BUG-743", true, None);
        assert_eq!(
            args,
            vec![
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                codex_inline_prompt("/aida-pickup BUG-743").unwrap()
            ]
        );
        assert!(!args.iter().any(|a| a == "--session-id"), "{args:?}");
        assert!(!args.iter().any(|a| a == "--resume"), "{args:?}");
    }

    #[test]
    fn interactive_session_args_pass_model_before_prompt() {
        let claude = claude_session_args(
            crate::seat_clone::launcher_main().as_deref(),
            None,
            None,
            Some("/aida-pickup STORY-1003"),
            Some("sid"),
            false,
            Some("opus-alias"),
        )
        .unwrap();
        assert_eq!(
            claude
                .windows(2)
                .find(|w| w[0] == "--model")
                .map(|w| w[1].as_str()),
            Some("opus-alias")
        );
        assert_eq!(
            claude.last().map(String::as_str),
            Some("/aida-pickup STORY-1003")
        );

        let codex = codex_session_args("/aida-pickup STORY-1003", false, Some("gpt-alias"));
        assert_eq!(
            codex,
            vec![
                "--model".to_string(),
                "gpt-alias".to_string(),
                codex_inline_prompt("/aida-pickup STORY-1003").unwrap()
            ]
        );
    }

    /// STORY-495 safety invariant: the headless argv ALWAYS forces
    /// `bypassPermissions` regardless of the interactive faithful default —
    /// a prompting (`default`) headless child has no TTY to answer and would
    /// hang the unattended drain forever. This is structurally separate from
    /// `claude_session_args`, so flipping the interactive default can never
    /// reach it. trace:STORY-495 | ai:claude
    #[test]
    fn headless_args_force_bypass_regardless_of_interactive_default() {
        // The interactive builder is now native-by-default…
        let interactive = claude_session_args(
            crate::seat_clone::launcher_main().as_deref(),
            None,
            None,
            Some("/aida-review"),
            Some("sid"),
            false,
            None,
        )
        .unwrap();
        assert!(!interactive.iter().any(|a| a == "--permission-mode"));
        // …yet the headless builder still hard-forces bypass.
        let headless = claude_headless_args("/aida-review", "sid");
        let pos = headless
            .iter()
            .position(|a| a == "--permission-mode")
            .expect("headless must inject --permission-mode");
        assert_eq!(
            headless.get(pos + 1).map(String::as_str),
            Some("bypassPermissions"),
            "headless must force bypassPermissions: {headless:?}"
        );
    }

    // BUG-743 acceptance: the Codex exec path stays bypassed independently of
    // the interactive builder, so both postures are covered.
    // trace:BUG-743 | ai:codex
    #[test]
    fn codex_headless_args_force_bypass() {
        let args = codex_headless_args("/aida-pickup BUG-743");
        assert_eq!(args.first().map(String::as_str), Some("exec"));
        assert!(
            args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()),
            "headless codex must force bypass: {args:?}"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("/aida-pickup BUG-743")
        );
    }

    #[test]
    fn headless_vendor_args_pass_model_for_claude_and_codex_only() {
        let prompt = "/aida-pickup STORY-1003";
        let sid = "sid";
        let claude = headless_vendor_args(
            None,
            HeadlessVendor::Claude,
            prompt,
            sid,
            false,
            Some("opus-alias"),
            None,
        )
        .unwrap();
        assert_eq!(
            claude
                .windows(2)
                .find(|w| w[0] == "--model")
                .map(|w| w[1].as_str()),
            Some("opus-alias")
        );
        assert_eq!(claude.last().map(String::as_str), Some(prompt));

        let codex = headless_vendor_args(
            None,
            HeadlessVendor::Codex,
            prompt,
            sid,
            false,
            Some("gpt-alias"),
            None,
        )
        .unwrap();
        assert_eq!(
            codex
                .windows(2)
                .find(|w| w[0] == "--model")
                .map(|w| w[1].as_str()),
            Some("gpt-alias")
        );
        assert_eq!(codex.last().map(String::as_str), Some(prompt));

        let agy = headless_vendor_args(
            None,
            HeadlessVendor::Agy,
            prompt,
            sid,
            false,
            Some("ignored"),
            None,
        )
        .unwrap();
        assert!(!agy.iter().any(|arg| arg == "--model"), "{agy:?}");
    }

    #[test]
    fn headless_vendor_args_pass_effort_in_native_form() {
        let prompt = "/aida-pickup STORY-1033";
        let sid = "sid";

        let claude = headless_vendor_args(
            None,
            HeadlessVendor::Claude,
            prompt,
            sid,
            false,
            None,
            Some("high"),
        )
        .unwrap();
        assert_eq!(
            claude
                .windows(2)
                .find(|w| w[0] == "--effort")
                .map(|w| w[1].as_str()),
            Some("high")
        );
        assert_eq!(claude.last().map(String::as_str), Some(prompt));

        let codex = headless_vendor_args(
            None,
            HeadlessVendor::Codex,
            prompt,
            sid,
            false,
            None,
            Some("medium"),
        )
        .unwrap();
        assert_eq!(
            codex
                .windows(2)
                .find(|w| w[0] == "-c")
                .map(|w| w[1].as_str()),
            Some("model_reasoning_effort=medium")
        );
        assert_eq!(codex.last().map(String::as_str), Some(prompt));

        let agy = headless_vendor_args(
            None,
            HeadlessVendor::Agy,
            prompt,
            sid,
            false,
            None,
            Some("low"),
        )
        .unwrap();
        assert_eq!(
            agy.windows(2)
                .find(|w| w[0] == "--effort")
                .map(|w| w[1].as_str()),
            Some("low")
        );
        assert_eq!(agy.last().map(String::as_str), Some(prompt));
    }

    /// BUG-050: every headless claude argv (fresh and resume, contained and
    /// not) carries exactly one `--settings` document whose `env` turns off
    /// background tasks, and a contained one also denies `Monitor`. The
    /// interactive contained flags keep background tasks: a human turn follows.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-050.ac265abc | ai:claude
    #[test]
    fn headless_argv_disables_monitor_and_background_tasks() {
        let dir = std::env::temp_dir();
        for (label, contained, args) in [
            ("fresh", false, claude_headless_args("p", "sid")),
            (
                "fresh-contained",
                true,
                claude_headless_args_for_roots(
                    crate::seat_clone::launcher_main().as_deref(),
                    "p",
                    "sid",
                    Some(&[dir.clone()]),
                    None,
                    None,
                )
                .unwrap(),
            ),
            ("resume", false, claude_headless_resume_args("p", "sid")),
            (
                "resume-contained",
                true,
                claude_headless_resume_args_with_posture(
                    crate::seat_clone::launcher_main().as_deref(),
                    "p",
                    "sid",
                    Some(&dir),
                )
                .unwrap(),
            ),
        ] {
            assert_headless_posture(&args, contained, label);
            assert_eq!(args.last().map(String::as_str), Some("p"), "[{label}]");
        }
        let interactive =
            claude_contained_flags_for_roots(crate::seat_clone::launcher_main().as_deref(), &[dir])
                .unwrap();
        assert!(
            !interactive
                .iter()
                .any(|a| a.contains(DISABLE_BACKGROUND_TASKS_ENV)),
            "{interactive:?}"
        );
    }

    /// BUG-050: only a resumable-vendor implementer lane with a lease and a
    /// seat tree gets the drain resume loop; the queue lane launch wires the
    /// lease's worktree and branch into it. BUG-2386: Codex lanes resume too.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    // trace:BUG-2386 | ai:claude
    #[test]
    fn only_a_resumable_implementer_lane_gets_the_drain_resume_loop() {
        let tree = Path::new("/seat");
        let lane = Some((tree, "bug-1"));
        for vendor in [HeadlessVendor::Claude, HeadlessVendor::Codex] {
            assert_eq!(
                drain_resume_lane(vendor, "implementer", Some("L1"), lane),
                Some(("L1", (tree, "bug-1")))
            );
        }
        for (vendor, seat, lease, lane_tree) in [
            (HeadlessVendor::Claude, "reviewer", Some("L1"), lane),
            (HeadlessVendor::Claude, "advisor", Some("L1"), lane),
            (HeadlessVendor::Agy, "implementer", Some("L1"), lane),
            (HeadlessVendor::Claude, "implementer", None, lane),
            (HeadlessVendor::Claude, "implementer", Some("L1"), None),
        ] {
            assert_eq!(
                drain_resume_lane(vendor, seat, lease, lane_tree),
                None,
                "{vendor:?} {seat} {lease:?} {lane_tree:?}"
            );
        }
        // The one production caller passes the lane's seat, not None.
        let queue = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/queue_cmd/work.rs"),
        )
        .unwrap();
        let call = queue
            .find("session::exec_vendor_headless(")
            .expect("queue lane launch");
        let args = &queue[call..call + queue[call..].find(");").unwrap()];
        assert!(
            args.contains("Some(&lease.id)")
                && args.contains("Some((&lease.worktree_path, &lease.branch))"),
            "the queue lane must hand its lease and seat tree to the resume loop: {args}"
        );
    }

    /// BUG-050 ratchet: every non-test function that builds a `claude` argv
    /// with `-p` (print mode: a headless run with no later turn) takes its
    /// flags from [`headless_claude_posture`] or
    /// [`headless_claude_posture_for`]. A new headless claude builder that
    /// composes its own flags fails here; route it through the helper.
    /// Functions that pass `-p` to another program (cargo, git, systemd-run,
    /// tmux, ps, agy) and name no claude are not claude argv builders.
    /// Scans `aida-cli-lib/src` and `aida-core/src`; in `aida-core/src/ai`
    /// (the AI client, which only ever drives the claude CLI) every `-p`
    /// counts as a claude builder whether or not it names claude.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    #[test]
    fn every_headless_claude_argv_goes_through_the_posture_helper() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest.parent().unwrap();
        let src = workspace;
        let mut files = Vec::new();
        let mut dirs = vec![manifest.join("src"), workspace.join("aida-core/src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n != "tests") {
                        dirs.push(path);
                    }
                } else if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
        let is_fn_line = |line: &str| {
            let t = line.trim_start();
            let t = t
                .strip_prefix("pub(crate) ")
                .or_else(|| t.strip_prefix("pub(super) "))
                .or_else(|| t.strip_prefix("pub "))
                .unwrap_or(t);
            t.starts_with("fn ")
        };
        let mut offenders = Vec::new();
        let mut sites = 0;
        let mut core_sites = 0;
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            // Production code only: stop at the unit-test module.
            let lines: Vec<&str> = text
                .lines()
                .take_while(|l| !l.starts_with("mod tests"))
                .collect();
            for (i, line) in lines.iter().enumerate() {
                if !line.contains("\"-p\"") || line.trim_start().starts_with("//") {
                    continue;
                }
                let start = (0..=i).rev().find(|&j| is_fn_line(lines[j])).unwrap_or(0);
                let end = (i + 1..lines.len())
                    .find(|&j| is_fn_line(lines[j]))
                    .unwrap_or(lines.len());
                let body: String = lines[start..end]
                    .iter()
                    .filter(|l| !l.trim_start().starts_with("//"))
                    .map(|l| format!("{l}\n"))
                    .collect();
                let rel = file.strip_prefix(src).unwrap_or(&file);
                let ai_client = rel.starts_with("aida-core/src/ai");
                if !ai_client && !body.to_ascii_lowercase().contains("claude") {
                    continue;
                }
                sites += 1;
                if ai_client {
                    core_sites += 1;
                }
                // Argv templates, never launched as built: every compete arm
                // and judge launches through `compete_cmd::compete_arm_argv`,
                // which leads with the posture.
                if rel == std::path::Path::new("aida-cli-lib/src/compete.rs")
                    && ["fn vendor_adapter(", "fn judge_command("]
                        .iter()
                        .any(|f| lines[start].contains(f))
                {
                    continue;
                }
                if !body.contains("headless_claude_posture") {
                    offenders.push(format!(
                        "{}:{}: {}",
                        rel.display(),
                        i + 1,
                        lines[start].trim()
                    ));
                }
            }
        }
        assert!(
            sites >= 5,
            "the scan found too few claude -p builders ({sites})"
        );
        assert!(
            core_sites >= 1,
            "the scan missed the aida-core AI client's claude -p builder"
        );
        assert!(
            offenders.is_empty(),
            "headless claude argv built without session::headless_claude_posture \
             (BUG-050: Monitor and background tasks must be off):\n{}",
            offenders.join("\n")
        );
    }

    #[test]
    fn contained_headless_args_use_strict_sandbox_settings() {
        let args = claude_headless_args_with_posture(
            crate::seat_clone::launcher_main().as_deref(),
            "/aida-review",
            "sid",
            true,
        )
        .unwrap();
        let pos = args
            .iter()
            .position(|a| a == "--permission-mode")
            .expect("contained headless must set a permission mode");
        assert_eq!(args.get(pos + 1).map(String::as_str), Some("dontAsk"));
        assert!(args.contains(&"--setting-sources".to_string()));
        assert!(args.contains(&"project".to_string()));
        let settings_pos = args
            .iter()
            .position(|a| a == "--settings")
            .expect("contained launch must pass inline settings");
        let settings: serde_json::Value =
            serde_json::from_str(args.get(settings_pos + 1).unwrap()).unwrap();
        assert_eq!(settings["sandbox"]["enabled"], true);
        assert_eq!(settings["sandbox"]["failIfUnavailable"], true);
        assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
        // BUG-1928: the allow list is the absolute rule set for the worktree
        // this process runs in, not the project-relative `Edit(/**)`.
        // trace:BUG-1928 | ai:claude
        // BUG-2201: the baseline Bash/Read rules are not write rules.
        let allow: Vec<String> = settings["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r.as_str().map(str::to_string))
            .filter(|r| {
                crate::contained_boundary::WRITE_TOOLS
                    .iter()
                    .any(|tool| r.starts_with(&format!("{tool}(")))
            })
            .collect();
        assert!(!allow.contains(&"Edit(/**)".to_string()), "{allow:?}");
        assert!(
            !allow.is_empty(),
            "the worktree must be writable: {allow:?}"
        );
        assert!(
            allow
                .iter()
                .all(|r| r.contains("(//") && r.ends_with("/**)")),
            "every write rule must be absolute: {allow:?}"
        );
        assert_eq!(
            allow,
            crate::contained_boundary::write_allow_rules(&contained_session_write_roots())
        );
        assert!(settings["permissions"]["deny"]
            .as_array()
            .unwrap()
            .contains(&serde_json::Value::String(
                "Bash(git reset --hard *)".to_string()
            )));
    }

    // STORY-683 / BUG-697 / TASK-1532: route through EnvVarGuard. trace:TASK-1532 | ai:agy
    struct HeadlessVendorEnvGuard {
        _guard: crate::test_env::EnvVarGuard,
    }

    impl HeadlessVendorEnvGuard {
        fn acquire() -> Self {
            let guard = crate::test_env::EnvVarGuard::unset("AIDA_HEADLESS_VENDOR");
            // TASK-1116: the flag-override tier is a process-global too — clear
            // it under the same lock so a prior override-setting test can never
            // leak into an env/knob/default assertion.
            set_headless_vendor_override(None);
            Self { _guard: guard }
        }

        fn set(&mut self, val: impl AsRef<std::ffi::OsStr>) {
            self._guard.reset(val);
        }
    }

    impl Drop for HeadlessVendorEnvGuard {
        fn drop(&mut self) {
            // TASK-1116: reset the override tier so it does not survive the test.
            set_headless_vendor_override(None);
        }
    }

    /// STORY-683: the vendor selector builds the correct command for each vendor.
    /// Claude reuses the SPIKE-7 `claude -p` flag set; Codex builds `codex exec`.
    /// This is the core vendor-dispatch invariant. trace:STORY-683 | ai:claude
    #[test]
    fn headless_vendor_args_builds_correct_command_per_vendor() {
        let prompt = "/aida-review --pr 7";
        let sid = "019e0000-0000-7000-8000-000000000000";

        // Claude arm: -p print mode + the prompt survives, NOT a codex command.
        let claude =
            headless_vendor_args(None, HeadlessVendor::Claude, prompt, sid, false, None, None)
                .unwrap();
        assert!(claude.contains(&"-p".to_string()), "claude -p: {claude:?}");
        assert!(
            claude.contains(&"bypassPermissions".to_string()),
            "claude bypass: {claude:?}"
        );
        assert!(claude.contains(&prompt.to_string()), "{claude:?}");
        assert!(
            !claude.contains(&"exec".to_string()),
            "claude arm must not be a codex exec: {claude:?}"
        );
        // Identical to the dedicated claude builder — the default path is unchanged.
        assert_eq!(
            claude,
            claude_headless_args_with_posture(None, prompt, sid, false).unwrap()
        );

        // Codex arm: `codex exec --json --dangerously-bypass-approvals-and-sandbox
        // <prompt>`, with the prompt as the final positional and NO claude `-p`.
        let codex =
            headless_vendor_args(None, HeadlessVendor::Codex, prompt, sid, false, None, None)
                .unwrap();
        assert_eq!(codex.first().map(String::as_str), Some("exec"), "{codex:?}");
        assert!(
            codex.contains(&"--json".to_string()),
            "codex must stream JSONL so the drain watchdog sees live output: {codex:?}"
        );
        assert!(
            codex.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()),
            "codex bypass: {codex:?}"
        );
        assert_eq!(codex.last().map(String::as_str), Some(prompt), "{codex:?}");
        assert!(
            !codex.contains(&"-p".to_string()),
            "codex arm must not carry claude's -p: {codex:?}"
        );

        // TASK-1048: Agy arm: `agy --dangerously-skip-permissions -p <prompt>`,
        // with the prompt as `-p`'s value at the tail and NO codex `exec`.
        // BUG-1686: the permission flag leads; `-p` is value-taking on agy.
        // trace:BUG-1686 | ai:claude
        let agy = headless_vendor_args(None, HeadlessVendor::Agy, prompt, sid, false, None, None)
            .unwrap();
        assert_eq!(agy, agy_headless_args(prompt), "{agy:?}");
        assert_eq!(
            agy.first().map(String::as_str),
            Some("--dangerously-skip-permissions"),
            "{agy:?}"
        );
        assert_eq!(agy.last().map(String::as_str), Some(prompt), "{agy:?}");
        assert!(
            !agy.contains(&"exec".to_string()),
            "agy arm must not be a codex exec: {agy:?}"
        );
    }

    /// TASK-1162: the guided-mode launch honors the session vendor. Claude
    /// keeps the natively-expanded slash invocation; Codex gets the adapted
    /// prompt (BUG-731 rendering) with the spec substituted and no
    /// Claude-only mechanics left in; Agy is refused by dispatch policy with
    /// a message that names the fence and both usable vendors.
    // trace:TASK-1162 | ai:claude
    #[test]
    fn guided_session_launch_routes_per_vendor() {
        let claude = guided_session_launch(HeadlessVendor::Claude, "TASK-9").unwrap();
        assert_eq!(claude.program, "claude");
        assert_eq!(claude.prompt, "/aida-guided-implement TASK-9");

        let codex = guided_session_launch(HeadlessVendor::Codex, "TASK-9").unwrap();
        assert_eq!(codex.program, "codex");
        assert!(codex.prompt.contains("TASK-9"), "{}", codex.prompt);
        assert!(!codex.prompt.contains("$ARGUMENTS"), "{}", codex.prompt);
        assert!(
            !codex.prompt.contains("AskUserQuestion"),
            "{}",
            codex.prompt
        );
        assert!(!codex.prompt.contains("/aida-"), "{}", codex.prompt);
        // The fork-presentation adaptation survives into the launched prompt.
        assert!(
            codex.prompt.contains("plain-text numbered"),
            "{}",
            codex.prompt
        );

        let err = guided_session_launch(HeadlessVendor::Agy, "TASK-9")
            .unwrap_err()
            .to_string();
        assert!(err.contains("draft-for-review"), "{err}");
        assert!(err.contains("claude"), "{err}");
        assert!(err.contains("codex"), "{err}");
    }

    /// TASK-894: the advisor-tier spawn builds the correct command per vendor.
    /// Claude resumes the fork (`--resume <uuid> /aida-advise`) or cold-boots the
    /// seeded prompt; Codex always hosts a fresh `codex exec <seeded>` (no resume,
    /// `is_fork` ignored). The Claude arms are byte-identical to the dedicated
    /// builders so an un-configured drain is unchanged.
    // trace:TASK-894 | ai:claude
    #[test]
    fn advisor_tier_program_and_args_builds_correct_command_per_vendor() {
        // BUG-698 family: `advisor_tier_program_and_args` reads `AIDA_AGENT_CMD`
        // via `resolve_agent_program`; without the guard, a concurrently-running
        // guarded WRITER test flips the env between this test's two evaluations
        // and the byte-identical assertion flakes (seen on CI 2026-07-07).
        let _env = AgentCmdEnvGuard::acquire();
        let seeded = "advisor-context\n\n/aida-advise";
        let advisor_uuid = "019e0000-0000-7000-8000-000000000aaa";

        // Claude cold-boot: `claude` + the seeded prompt via the dedicated builder.
        let (prog, args) =
            advisor_tier_program_and_args(HeadlessVendor::Claude, false, seeded, advisor_uuid);
        assert_eq!(prog, "claude");
        assert_eq!(args, claude_headless_args(seeded, advisor_uuid));
        assert!(args.contains(&"-p".to_string()), "{args:?}");
        assert!(args.contains(&seeded.to_string()), "{args:?}");
        assert!(!args.contains(&"--resume".to_string()), "{args:?}");

        // Claude fork: `claude … --resume <uuid> /aida-advise`, NOT the seeded prompt.
        let (prog, args) =
            advisor_tier_program_and_args(HeadlessVendor::Claude, true, seeded, advisor_uuid);
        assert_eq!(prog, "claude");
        assert_eq!(
            args,
            claude_headless_resume_args("/aida-advise", advisor_uuid)
        );
        assert!(args.contains(&"--resume".to_string()), "{args:?}");
        assert!(args.contains(&advisor_uuid.to_string()), "{args:?}");
        assert!(args.contains(&"/aida-advise".to_string()), "{args:?}");

        // Codex: a fresh `codex exec <seeded>` per punt; with `is_fork`,
        // `codex exec fork <thread> <seeded>` where the caller passes the live
        // Codex thread as `advisor_uuid`. trace:TASK-894
        // trace:STORY-2255 | ai:claude
        for is_fork in [false, true] {
            let (prog, args) =
                advisor_tier_program_and_args(HeadlessVendor::Codex, is_fork, seeded, advisor_uuid);
            assert_eq!(prog, "codex", "is_fork={is_fork}");
            let expected = if is_fork {
                codex_exec_fork_args(codex_headless_args(seeded), advisor_uuid).unwrap()
            } else {
                codex_headless_args(seeded)
            };
            assert_eq!(args, expected, "is_fork={is_fork}");
            assert_eq!(args.first().map(String::as_str), Some("exec"), "{args:?}");
            assert_eq!(args.last().map(String::as_str), Some(seeded), "{args:?}");
            assert_eq!(args.get(1).map(String::as_str) == Some("fork"), is_fork);
            assert_eq!(
                args.contains(&advisor_uuid.to_string()),
                is_fork,
                "{args:?}"
            );
            // Never carries claude's resume / session flags.
            assert!(!args.contains(&"--resume".to_string()), "{args:?}");
            assert!(!args.contains(&"-p".to_string()), "{args:?}");
        }

        // TASK-1048: Agy, like Codex, has no resume model — a fresh `agy -p
        // <seeded>` per punt, `is_fork` ignored, never carrying claude's resume
        // flags. trace:TASK-1048
        for is_fork in [false, true] {
            let (prog, args) =
                advisor_tier_program_and_args(HeadlessVendor::Agy, is_fork, seeded, advisor_uuid);
            assert_eq!(prog, "agy", "is_fork={is_fork}");
            assert_eq!(args, agy_headless_args(seeded), "is_fork={is_fork}");
            // BUG-1686: permission flag first, prompt as `-p`'s trailing value.
            // trace:BUG-1686 | ai:claude
            assert_eq!(
                args.first().map(String::as_str),
                Some("--dangerously-skip-permissions"),
                "{args:?}"
            );
            assert_eq!(args.last().map(String::as_str), Some(seeded), "{args:?}");
            assert!(!args.contains(&"--resume".to_string()), "{args:?}");
            assert!(!args.contains(&advisor_uuid.to_string()), "{args:?}");
        }
    }

    /// STORY-683: the program a vendor spawns is its own binary.
    /// trace:STORY-683 | ai:claude
    #[test]
    fn headless_vendor_program_maps_to_binary() {
        assert_eq!(HeadlessVendor::Claude.program(), "claude");
        assert_eq!(HeadlessVendor::Codex.program(), "codex");
        // TASK-1048: AGY spawns the `agy` runtime binary.
        assert_eq!(HeadlessVendor::Agy.program(), "agy");
    }

    /// STORY-683: vendor-token parsing is case-insensitive, whitespace-tolerant,
    /// and rejects unknowns (so the caller falls through to the default).
    /// trace:STORY-683 | ai:claude
    #[test]
    fn headless_vendor_parse_is_lenient_and_rejects_unknown() {
        assert_eq!(
            HeadlessVendor::parse("claude"),
            Some(HeadlessVendor::Claude)
        );
        assert_eq!(
            HeadlessVendor::parse(" Codex "),
            Some(HeadlessVendor::Codex)
        );
        assert_eq!(HeadlessVendor::parse("CODEX"), Some(HeadlessVendor::Codex));
        // TASK-1048: `agy` is the canonical Antigravity token; `antigravity` is
        // an accepted alias; both are case-insensitive + whitespace-tolerant.
        assert_eq!(HeadlessVendor::parse("agy"), Some(HeadlessVendor::Agy));
        assert_eq!(HeadlessVendor::parse(" AGY "), Some(HeadlessVendor::Agy));
        assert_eq!(
            HeadlessVendor::parse("Antigravity"),
            Some(HeadlessVendor::Agy)
        );
        // Genuine unknowns still reject so the caller falls through to default.
        assert_eq!(HeadlessVendor::parse("gemini"), None);
        assert_eq!(HeadlessVendor::parse(""), None);
    }

    /// STORY-683: with no env override and no config, the resolver defaults to
    /// Claude — an un-configured drain is byte-identical to the pre-STORY-683
    /// behavior. trace:STORY-683 | ai:claude
    #[test]
    fn resolve_headless_vendor_defaults_to_claude() {
        let _env = HeadlessVendorEnvGuard::acquire();
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Claude);
    }

    /// STORY-683: `AIDA_HEADLESS_VENDOR=codex` selects Codex (env precedence);
    /// an unrecognized value is ignored and falls through to the Claude default.
    /// trace:STORY-683 | ai:claude
    #[test]
    fn resolve_headless_vendor_env_override() {
        let mut _env = HeadlessVendorEnvGuard::acquire();
        let tmp = tempfile::tempdir().unwrap();

        _env.set("codex");
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Codex);

        _env.set("claude");
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Claude);

        _env.set("nonsense");
        assert_eq!(
            resolve_headless_vendor(tmp.path()),
            HeadlessVendor::Claude,
            "unrecognized env value must fall through to the default"
        );
    }

    // BUG-898: a codex-only enabled profile must not let queue-work's launch
    // resolver fall through to the built-in Claude default. This preflight is
    // pure and runs before the queue-work worktree/lease mutation path.
    // trace:BUG-898 | ai:codex
    // trace:TASK-1748 | ai:antigravity
    #[test]
    fn resolve_enabled_headless_vendor_autopicks_single_enabled_profile() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(home.join(".aida")).unwrap();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        std::fs::write(
            project.join(".aida/config.toml"),
            "[agents]\nenabled = [\"codex\"]\n",
        )
        .unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HEADLESS_VENDOR", None),
            ("AIDA_HOME", Some(home.to_str().unwrap())),
            ("HOME", Some(home.to_str().unwrap())),
        ]);
        set_headless_vendor_override(None);

        assert_eq!(
            resolve_enabled_headless_vendor(&project).unwrap(),
            HeadlessVendor::Codex
        );
    }

    // BUG-898: an all-disabled profile is a launch configuration error and
    // must be reported before queue-work creates a worktree or lease.
    // trace:BUG-898 | ai:codex
    // trace:TASK-1748 | ai:antigravity
    #[test]
    fn resolve_enabled_headless_vendor_refuses_all_disabled_profile() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(home.join(".aida")).unwrap();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        std::fs::write(
            project.join(".aida/config.toml"),
            "[agents]\nenabled = []\n",
        )
        .unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HEADLESS_VENDOR", None),
            ("AIDA_HOME", Some(home.to_str().unwrap())),
            ("HOME", Some(home.to_str().unwrap())),
        ]);
        set_headless_vendor_override(None);

        let err = resolve_enabled_headless_vendor(&project)
            .expect_err("all-disabled profile must refuse launch");
        let msg = err.to_string();
        assert!(
            msg.contains("no agent launch profiles are enabled"),
            "{msg}"
        );
        assert!(msg.contains("--no-launch"), "{msg}");
    }

    // BUG-898: the allow-list gates explicit launch choices too. If the
    // operator explicitly asks for disabled Claude in a codex-only project, we
    // refuse instead of silently launching Claude or mutating queue-work state.
    // trace:BUG-898 | ai:codex
    // trace:TASK-1748 | ai:antigravity
    #[test]
    fn resolve_enabled_headless_vendor_refuses_explicit_disabled_vendor() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(home.join(".aida")).unwrap();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        std::fs::write(
            project.join(".aida/config.toml"),
            "[agents]\nenabled = [\"codex\"]\n",
        )
        .unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HEADLESS_VENDOR", Some("claude")),
            ("AIDA_HOME", Some(home.to_str().unwrap())),
            ("HOME", Some(home.to_str().unwrap())),
        ]);
        set_headless_vendor_override(None);

        let err = resolve_enabled_headless_vendor(&project)
            .expect_err("explicit disabled vendor must refuse launch");
        let msg = err.to_string();
        assert!(
            msg.contains("resolved launch vendor `claude` is disabled"),
            "{msg}"
        );
        assert!(msg.contains("--vendor codex"), "{msg}");
        assert!(msg.contains("--no-launch"), "{msg}");
    }

    /// BUG-1607: an explicit `--vendor` flag (the `install_headless_vendor_override`
    /// bridge every `--vendor` flag installs before resolution — the SAME bridge
    /// `aida queue work --role reviewer --vendor codex` uses) wins over the
    /// project `[agents] vendor` default, for the exact resolver
    /// (`resolve_enabled_headless_vendor`) both the standalone reviewer launch
    /// and the implementer launch share.
    // trace:BUG-1607 | ai:claude
    // trace:TASK-1748 | ai:antigravity
    #[test]
    fn reviewer_vendor_resolution_explicit_flag_wins_over_config_default() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(home.join(".aida")).unwrap();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        // Project config defaults to claude, with both profiles enabled.
        std::fs::write(
            project.join(".aida/config.toml"),
            "[agents]\nvendor = \"claude\"\nenabled = [\"claude\", \"codex\"]\n",
        )
        .unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HEADLESS_VENDOR", None),
            ("AIDA_HOME", Some(home.to_str().unwrap())),
            ("HOME", Some(home.to_str().unwrap())),
        ]);
        set_headless_vendor_override(None);

        // No flag: the configured default (claude) wins.
        assert_eq!(
            resolve_enabled_headless_vendor(&project).unwrap(),
            HeadlessVendor::Claude,
            "with no explicit vendor, the config default should resolve"
        );

        // An explicit `--vendor codex` wins over the config default.
        assert_eq!(
            install_headless_vendor_override("codex"),
            Some(HeadlessVendor::Codex)
        );
        assert_eq!(
            resolve_enabled_headless_vendor(&project).unwrap(),
            HeadlessVendor::Codex,
            "an explicit --vendor must win over `[agents] vendor`"
        );

        set_headless_vendor_override(None);
    }

    /// BUG-1607: [`preflight_vendor_binary`] must fail closed for a resolved
    /// vendor whose binary cannot be found — this is the check that must run
    /// BEFORE `session_start` mints a lease/worktree, so a
    /// resolved-but-uninstalled vendor never leaves orphaned state behind.
    /// Routes through `AIDA_AGENT_CMD` (the same mock seam the headless spawn
    /// paths already use) so no real `claude`/`codex` binary is ever spawned or
    /// even required to exist on the test machine.
    // trace:BUG-1607 | ai:claude
    #[test]
    fn preflight_vendor_binary_fails_closed_for_unreachable_binary() {
        let mut _env = AgentCmdEnvGuard::acquire();
        let missing = "/tmp/aida-bug-1607-missing-agent-binary-does-not-exist";
        let _ = std::fs::remove_file(missing);
        _env.set_cmd(missing);

        // A tempdir standing in for "nothing has been created yet" — the
        // state right before `session_start` would mint a lease/worktree.
        let tmp = tempfile::tempdir().unwrap();
        let before = std::fs::read_dir(tmp.path()).unwrap().count();

        let err = preflight_vendor_binary(HeadlessVendor::Codex)
            .expect_err("an unreachable binary must fail the preflight");
        let msg = err.to_string();
        assert!(msg.contains("codex"), "{msg}");
        assert!(msg.contains("not on PATH"), "{msg}");
        assert!(
            msg.contains("lease or worktree"),
            "the error must say WHY this runs early: {msg}"
        );

        // Purely a reachability check — it must not have touched the
        // filesystem at all (no lease, no worktree, nothing).
        let after = std::fs::read_dir(tmp.path()).unwrap().count();
        assert_eq!(before, after, "preflight must not create any state");
    }

    /// BUG-1607: the mirror of the above — once the mock binary actually
    /// exists (still never a real vendor CLI), the preflight passes.
    // trace:BUG-1607 | ai:claude
    #[test]
    fn preflight_vendor_binary_succeeds_for_reachable_binary() {
        let mut _env = AgentCmdEnvGuard::acquire();
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("fake-agent");
        std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
        _env.set_cmd(fake.to_str().unwrap());

        preflight_vendor_binary(HeadlessVendor::Codex)
            .expect("an existing (mock) binary must pass the preflight");
        preflight_vendor_binary(HeadlessVendor::Claude)
            .expect("the same mock override applies uniformly across vendors");
    }

    /// BUG-1607: the ordering-gap fix. Before `preflight_launch_vendor`
    /// existed, an interactive Agy launch could pass `preflight_vendor_binary`
    /// cleanly whenever `agy` happened to be reachable — so `session_start`
    /// minted the lease + worktree, and only the LATER
    /// `interactive_reviewer_launch_plan` call (or the implementer-side
    /// `launch_vendor == Agy && !no_human` check, both well after
    /// `session_start`) refused. That is the exact BUG-1607 shape: a refusal
    /// that arrives after state was already mutated. Prove the fix two ways:
    /// (1) an interactive Agy launch refuses EVEN WHEN the binary is
    /// reachable — vendor support is checked, not just presence; (2) the
    /// SAME resolved vendor, launched headlessly, now refuses on the separate
    /// all-tool admission gate before callers can create state.
    // trace:BUG-1607 | ai:claude
    #[test]
    fn preflight_launch_vendor_refuses_interactive_agy_even_when_binary_reachable() {
        let mut _env = AgentCmdEnvGuard::acquire();
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("fake-agy");
        std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
        _env.set_cmd(fake.to_str().unwrap());

        // Binary IS reachable (the mock exists) — the old
        // `preflight_vendor_binary`-only check would have passed here,
        // letting a caller mint a lease/worktree before the later refusal.
        preflight_vendor_binary(HeadlessVendor::Agy).expect("sanity: the mock binary is reachable");

        // The interactive preflight must still refuse — vendor SUPPORT is
        // checked, independent of binary reachability.
        let err = preflight_launch_vendor(HeadlessVendor::Agy, true)
            .expect_err("an interactive Agy launch must be refused before any state is created");
        let msg = err.to_string();
        assert!(msg.contains("agy"), "{msg}");
        assert!(msg.contains("--vendor claude"), "{msg}");
        assert!(msg.contains("--vendor codex"), "{msg}");

        // trace:TASK-3048 | ai:codex
        let error = preflight_launch_vendor(HeadlessVendor::Agy, false)
            .expect_err("legacy headless bypass must refuse before state creation");
        assert!(error.to_string().contains("fresh all-tool conformance"));
    }

    /// BUG-1607: vendor support is checked BEFORE binary reachability, so an
    /// interactive Agy refusal names the real reason ("does not support
    /// vendor `agy`") even when the binary is ALSO missing — an operator
    /// installing `agy` would not make an interactive reviewer/implementer
    /// launch work, so the error must not suggest otherwise.
    // trace:BUG-1607 | ai:claude
    #[test]
    fn preflight_launch_vendor_agy_support_check_runs_before_binary_check() {
        let _env = AgentCmdEnvGuard::acquire();
        // No override installed — `agy` is (almost certainly) not a real
        // binary on the test machine either, so BOTH checks could fire;
        // assert the vendor-support message wins.
        let err = preflight_launch_vendor(HeadlessVendor::Agy, true)
            .expect_err("interactive Agy must refuse regardless of binary presence");
        assert!(
            err.to_string().contains("does not support vendor `agy`"),
            "{err}"
        );
    }

    /// BUG-1607: the pure launch-plan resolver an interactive reviewer launch
    /// (`aida queue work --role reviewer` and `aida review` / `aida human
    /// review`) builds from — no process is spawned. With Codex resolved, the
    /// plan names the `codex` program and the exact argv
    /// [`codex_session_args`] builds, so "reviewer launch with codex resolves
    /// to the codex exec" is provable without spawning anything.
    // trace:BUG-1607 | ai:claude
    #[test]
    fn interactive_reviewer_launch_plan_codex_resolves_to_codex_exec() {
        // The plan's `program` is now routed through `resolve_agent_program`
        // (`AIDA_AGENT_CMD`), so guard against a concurrently-running mock
        // test leaking an override into this one.
        let _env = AgentCmdEnvGuard::acquire();
        let plan = interactive_reviewer_launch_plan(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Codex,
            None,
            Some("review-story-1"),
            "/aida-review --pr 42",
            "session-1",
            false,
        )
        .expect("codex is a supported interactive reviewer vendor");
        assert_eq!(plan.vendor, HeadlessVendor::Codex);
        assert_eq!(plan.program, "codex");
        assert_eq!(
            plan.args,
            codex_session_args("/aida-review --pr 42", false, None)
        );
        let body = plan.args.last().unwrap();
        assert!(body.contains(".aida/review-verdicts/PR-N.json"));
        assert!(body.contains("42"));
        assert!(!body.starts_with('/'));

        // `--permission-mode bypassPermissions` maps to Codex's own bypass
        // flag (mirrors the general interactive Codex-tab dispatch, BUG-743).
        let bypass_plan = interactive_reviewer_launch_plan(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Codex,
            Some("bypassPermissions"),
            Some("review-story-1"),
            "/aida-review --pr 42",
            "session-1",
            false,
        )
        .unwrap();
        assert_eq!(
            bypass_plan.args,
            codex_session_args("/aida-review --pr 42", true, None)
        );
    }

    /// BUG-1607: parity check — Claude's plan is byte-identical to the
    /// pre-existing `claude_session_args` shape, so routing the reviewer
    /// launch through the shared plan resolver never changes Claude's
    /// behavior.
    // trace:BUG-1607 | ai:claude
    #[test]
    fn interactive_reviewer_launch_plan_claude_matches_claude_session_args() {
        let _env = AgentCmdEnvGuard::acquire();
        let plan = interactive_reviewer_launch_plan(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Claude,
            Some("acceptEdits"),
            Some("review-story-1"),
            "/aida-review",
            "session-1",
            true,
        )
        .expect("claude is a supported interactive reviewer vendor");
        assert_eq!(plan.vendor, HeadlessVendor::Claude);
        assert_eq!(plan.program, "claude");
        assert_eq!(
            plan.args,
            claude_session_args(
                crate::seat_clone::launcher_main().as_deref(),
                Some("acceptEdits"),
                Some("review-story-1"),
                Some("/aida-review"),
                Some("session-1"),
                true,
                None,
            )
            .unwrap()
        );
    }

    /// BUG-1607: Agy has no interactive reviewer support yet — the plan
    /// resolver refuses with a recovery hint instead of silently launching
    /// nothing or falling back to another vendor.
    // trace:BUG-1607 | ai:claude
    #[test]
    fn interactive_reviewer_launch_plan_agy_refuses_with_recovery_hint() {
        let _env = AgentCmdEnvGuard::acquire();
        let err = interactive_reviewer_launch_plan(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Agy,
            None,
            None,
            "/aida-review",
            "session-1",
            false,
        )
        .expect_err("agy has no interactive reviewer support");
        let msg = err.to_string();
        assert!(msg.contains("agy"), "{msg}");
        assert!(msg.contains("--vendor claude"), "{msg}");
        assert!(msg.contains("--vendor codex"), "{msg}");
    }

    /// BUG-705: the shared composition routes per vendor — with Codex the
    /// program is `codex` and the argv is the exec form; with Claude it is the
    /// unchanged `claude -p` form. This is the helper BOTH the spawn path and
    /// the `--no-human` phase-1 exec path build from, so the exec site can
    /// never silently miss the vendor again.
    // trace:BUG-705 | ai:claude
    // ── BUG-799: codex slash-command inline rendering ──
    #[test]
    fn codex_review_slash_command_expands_to_the_handshake_bearing_body() {
        let p = codex_inline_prompt("/aida-review --pr 1612").expect("portable command expands");
        assert!(
            p.contains(".aida/review-verdicts/PR-N.json"),
            "the verdict-file handshake must reach the model"
        );
        assert!(
            p.contains("1612"),
            "the PR number is load-bearing and must survive expansion"
        );
        assert!(!p.starts_with('/'), "must be a body, not a slash string");
    }

    #[test]
    fn codex_pickup_slash_command_expands_with_its_spec_id() {
        let p = codex_inline_prompt("/aida-pickup BUG-790").expect("portable command expands");
        assert!(p.contains("BUG-790"), "the spec id must survive expansion");
    }

    #[test]
    fn non_slash_and_non_aida_prompts_pass_through_untouched() {
        assert_eq!(codex_inline_prompt("do a thing"), None);
        assert_eq!(codex_inline_prompt("/help"), None);
        assert_eq!(codex_inline_prompt("review the PR /aida-review"), None);
    }

    #[test]
    fn nonportable_commands_fall_through_raw() {
        // aida-advise is orchestrator-internal and excluded from the portable
        // set; expansion must decline rather than panic or invent a body.
        assert_eq!(codex_inline_prompt("/aida-advise PUNT-1"), None);
    }

    #[test]
    fn codex_phase_argv_carries_the_expanded_body_and_claude_is_unchanged() {
        let _env = AgentCmdEnvGuard::acquire();
        let (_, args) = compose_headless_command(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Codex,
            "/aida-review --pr 7",
            "sid",
            false,
            None,
            None,
        )
        .unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("review-verdicts"),
            "codex argv must carry the rendered body: {joined}"
        );
        assert!(
            !args.iter().any(|a| a == "/aida-review --pr 7"),
            "the literal slash string must be gone"
        );

        let (_, cargs) = compose_headless_command(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Claude,
            "/aida-review --pr 7",
            "sid",
            false,
            None,
            None,
        )
        .unwrap();
        assert!(
            cargs.contains(&"/aida-review --pr 7".to_string()),
            "claude expands its own skills — its path must be byte-identical"
        );
    }

    // trace:BUG-2389 | ai:codex
    #[test]
    fn bug_2389_coordinators_route_configured_vendor_and_keep_child_contracts() {
        for vendor in [HeadlessVendor::Claude, HeadlessVendor::Codex] {
            let root = tempfile::tempdir().unwrap();
            assert!(std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(root.path())
                .status()
                .unwrap()
                .success());
            let _ambient = crate::test_env::AmbientGuard::hermetic_and(
                root.path(),
                None,
                &[
                    ("AIDA_HEADLESS_VENDOR", None),
                    ("AIDA_AGENT_CMD", Some("/bin/true")),
                    ("AIDA_OS_WRAP", Some("0")),
                    ("AIDA_CONTAINED", None),
                    ("AIDA_SESSION_PURPOSE", Some("parent-purpose")),
                ],
            );
            let _cwd = contained_cwd_seam::pin(root.path());
            set_headless_vendor_override(None);
            std::fs::create_dir_all(root.path().join(".aida")).unwrap();
            std::fs::write(
                root.path().join(".aida/agents.toml"),
                format!("[agents]\nvendor = {:?}\n", vendor.as_str()),
            )
            .unwrap();
            // The handlers resolve once and hand the result to the builders.
            let resolved = resolve_headless_vendor(root.path());
            assert_eq!(resolved, vendor);
            assert_eq!(
                crate::session::analysis_headless_launch(root.path()).0,
                vendor
            );
            let intake = crate::intake::IntakeConfig::default();
            let commands = [
                crate::burndown_run::build_burndown_quiet_command(
                    resolved,
                    "/aida-burndown --max 2",
                    "bypassPermissions",
                    "child-grant",
                    "CEILING",
                    "10",
                    false,
                )
                .unwrap(),
                crate::burndown_run::build_burndown_verbose_command(
                    resolved,
                    "/aida-burndown --max 2",
                    "bypassPermissions",
                    "child-grant",
                    "CEILING",
                    "10",
                    false,
                )
                .unwrap(),
                crate::autopilot_intake::build_autopilot_intake_command(
                    resolved,
                    "/aida-assess",
                    "bypassPermissions",
                    "child-grant",
                    false,
                    &[],
                    &[],
                    &intake,
                    "stop",
                    crate::backlog::RiskLevel::Low,
                    None,
                    false,
                )
                .unwrap(),
                crate::questions::build_questions_clarify_command(
                    resolved,
                    "/aida-clarify BUG-2389",
                    "child-grant",
                    false,
                )
                .unwrap(),
            ];
            for (i, command) in commands.iter().enumerate() {
                let args: Vec<_> = command
                    .get_args()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect();
                assert_eq!(command.get_program(), "/bin/true");
                let env = |key: &str| {
                    command
                        .get_envs()
                        .find(|(k, _)| *k == key)
                        .map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
                };
                assert_eq!(
                    env(crate::gate::seat_authority::GRANT_ENV),
                    Some(Some("child-grant".into()))
                );
                assert_eq!(env("AIDA_SESSION_PURPOSE"), Some(None));
                assert_eq!(
                    env("AIDA_SESSION_ROLE"),
                    Some(Some(if i < 2 { "implementer" } else { "advisor" }.into()))
                );
                if i < 3 {
                    assert_eq!(env("AIDA_HEADLESS"), Some(Some("1".into())));
                    assert_eq!(
                        env("AIDA_HEADLESS_VENDOR"),
                        Some(Some(vendor.as_str().into()))
                    );
                }
                if vendor == HeadlessVendor::Codex {
                    assert!(!args.iter().any(|a| a == "-p" || a == "--permission-mode"));
                    assert_eq!(args.first().map(String::as_str) == Some("exec"), i < 3);
                    assert_eq!(args.iter().any(|a| a == "--json"), i == 1);
                    let prompt = args.last().unwrap();
                    assert!(!prompt.starts_with("/aida-"), "{prompt}");
                    if i < 2 {
                        assert!(prompt.contains("--max 2"));
                        assert!(!prompt.contains("--vendor claude"));
                    }
                } else if i < 3 {
                    assert!(args.iter().any(|a| a == "-p"));
                    assert_eq!(args.iter().any(|a| a == "stream-json"), i == 1);
                    assert_eq!(
                        args.iter().any(|a| a == "--include-partial-messages"),
                        i == 1
                    );
                }
            }
        }
    }

    /// `dryrun --ai`, `intent` and `questions research` launch on the
    /// configured vendor and keep the implementer seat they ran under before
    /// BUG-2389, so a launcher grant that delegates implementer still works.
    // trace:BUG-2389 | ai:claude
    #[test]
    fn bug_2389_analysis_passes_route_configured_vendor_on_implementer_seat() {
        for vendor in [HeadlessVendor::Claude, HeadlessVendor::Codex] {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir(root.path().join(".git")).unwrap();
            let _ambient = crate::test_env::AmbientGuard::hermetic_and(
                root.path(),
                None,
                &[("AIDA_HEADLESS_VENDOR", None)],
            );
            set_headless_vendor_override(None);
            std::fs::create_dir_all(root.path().join(".aida")).unwrap();
            std::fs::write(
                root.path().join(".aida/agents.toml"),
                format!("[agents]\nvendor = {:?}\n", vendor.as_str()),
            )
            .unwrap();
            assert_eq!(
                analysis_headless_launch(root.path()),
                (vendor, aida_core::agents_config::AgentSeat::Implementer)
            );
        }
        // Every analysis call site goes through the shared helper.
        for (file, src, sites) in [
            ("plan_intent.rs", include_str!("../plan_intent.rs"), 2),
            ("questions.rs", include_str!("../questions.rs"), 1),
        ] {
            assert_eq!(
                src.matches("spawn_analysis_headless(").count(),
                sites,
                "{file}: analysis passes must launch via spawn_analysis_headless"
            );
            assert!(
                !src.contains("spawn_claude_headless("),
                "{file}: no Claude-only analysis spawn"
            );
        }
    }

    /// `--permission-mode` is a Claude flag: burndown and intake refuse it on
    /// another vendor instead of silently dropping it.
    // trace:BUG-2389 | ai:claude
    #[test]
    fn bug_2389_coordinator_permission_mode_is_claude_only() {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
        let err = coordinator_permission_mode(HeadlessVendor::Codex, Some("acceptEdits"), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--permission-mode"), "{err}");
        assert!(err.contains("codex"), "{err}");
        assert_eq!(
            coordinator_permission_mode(HeadlessVendor::Codex, None, false).unwrap(),
            ""
        );
        assert_eq!(
            coordinator_permission_mode(HeadlessVendor::Claude, Some("acceptEdits"), false)
                .unwrap(),
            "acceptEdits"
        );
        assert_eq!(
            coordinator_permission_mode(HeadlessVendor::Claude, None, false).unwrap(),
            "bypassPermissions"
        );
    }

    /// The argv adapter never panics when `--verbose` is absent, keeps the
    /// prompt last, and leaves an existing partial-messages flag alone.
    // trace:BUG-2389 | ai:claude
    #[test]
    fn bug_2389_adapt_coordinator_args_tolerates_missing_verbose() {
        let mut args: Vec<String> = ["-p", "--permission-mode", "bypassPermissions", "prompt"]
            .map(String::from)
            .to_vec();
        adapt_coordinator_args(
            HeadlessVendor::Claude,
            &mut args,
            Some("acceptEdits"),
            false,
            true,
        );
        assert_eq!(args[2], "acceptEdits");
        assert!(args.iter().any(|a| a == "--include-partial-messages"));
        assert_eq!(args.last().unwrap(), "prompt");
        let before = args.clone();
        adapt_coordinator_args(
            HeadlessVendor::Claude,
            &mut args,
            Some("acceptEdits"),
            false,
            true,
        );
        assert_eq!(args, before);
        // A trailing `--permission-mode` with no value is left as is.
        let mut dangling: Vec<String> = vec!["--permission-mode".into()];
        adapt_coordinator_args(
            HeadlessVendor::Claude,
            &mut dangling,
            Some("x"),
            false,
            false,
        );
        assert_eq!(dangling, vec!["--permission-mode".to_string()]);
    }

    /// The dry-run preview is the launch argv (modulo the session id) and
    /// reserves no child handle.
    // trace:BUG-2389 | ai:claude
    #[test]
    fn bug_2389_coordinator_preview_matches_launch_without_reserving() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        let _ambient = crate::test_env::AmbientGuard::hermetic_and(
            root.path(),
            None,
            &[
                ("AIDA_AGENT_CMD", Some("/bin/true")),
                ("AIDA_OS_WRAP", Some("0")),
                ("AIDA_CONTAINED", None),
            ],
        );
        let _cwd = contained_cwd_seam::pin(root.path());
        let seat = aida_core::agents_config::AgentSeat::Implementer;
        let cases = [HeadlessVendor::Claude, HeadlessVendor::Codex]
            .into_iter()
            .flat_map(|vendor| [(vendor, false), (vendor, true)]);
        let previews: Vec<_> = cases
            .map(|(vendor, stream)| {
                let (program, preview) = coordinator_headless_preview(
                    None,
                    vendor,
                    "/aida-burndown --max 2",
                    "acceptEdits",
                    seat,
                    false,
                    stream,
                )
                .unwrap();
                assert_eq!(program, "/bin/true");
                (vendor, stream, preview)
            })
            .collect();
        let handles = crate::agent_handle::handles_dir(root.path());
        let reserved = std::fs::read_dir(&handles).map_or(0, |d| d.count());
        assert_eq!(reserved, 0, "a preview must not reserve a child handle");
        for (vendor, stream, preview) in previews {
            {
                let launched: Vec<String> = build_coordinator_headless_command(
                    None,
                    vendor,
                    "/aida-burndown --max 2",
                    "acceptEdits",
                    seat,
                    "",
                    false,
                    stream,
                )
                .unwrap()
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
                let without_session = |args: &[String]| -> Vec<String> {
                    let mut out = args.to_vec();
                    if let Some(i) = out.iter().position(|a| a == "--session-id") {
                        out.drain(i..i + 2);
                    }
                    out
                };
                assert_eq!(without_session(&preview), without_session(&launched));
                if vendor == HeadlessVendor::Claude {
                    assert!(preview.iter().any(|a| a == "acceptEdits"));
                    assert_eq!(preview.iter().any(|a| a == "stream-json"), stream);
                }
            }
        }
    }

    /// BUG-2386: a Codex headless resume is the launch's own argv rewritten to
    /// `exec resume <flags> <thread> <prompt>`: posture, model and effort kept.
    // trace:BUG-2386 | ai:claude
    #[test]
    fn codex_headless_resume_keeps_the_launch_flags() {
        let _env = AgentCmdEnvGuard::acquire();
        let (prog, args) = compose_headless_command_for(
            None,
            HeadlessVendor::Codex,
            "do a thing",
            "019a-thread",
            false,
            Some("gpt-x"),
            Some("high"),
            true,
        )
        .unwrap();
        assert!(prog.ends_with("codex"), "{prog}");
        assert_eq!(&args[..2], ["exec", "resume"], "{args:?}");
        let n = args.len();
        assert_eq!(&args[n - 2..], ["019a-thread", "do a thing"], "{args:?}");
        for flag in [
            "--json",
            "--dangerously-bypass-approvals-and-sandbox",
            "gpt-x",
            "model_reasoning_effort=high",
        ] {
            assert!(args.iter().any(|a| a == flag), "{flag}: {args:?}");
        }
        // A native-contained launch keeps its sandbox flags ahead of `exec`.
        let contained: Vec<String> = [
            "--sandbox",
            "workspace-write",
            "--ask-for-approval",
            "never",
            "exec",
            "--json",
            "p",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            codex_resume_exec_args(contained, "t-1").unwrap(),
            [
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "never",
                "exec",
                "resume",
                "--json",
                "t-1",
                "p"
            ]
            .map(String::from)
        );
        assert!(codex_resume_exec_args(vec!["exec".into(), "p".into()], "--last").is_err());
        assert!(codex_resume_exec_args(vec!["-p".into(), "p".into()], "t-1").is_err());
        let agy = compose_headless_command_for(
            None,
            HeadlessVendor::Agy,
            "p",
            "sid",
            false,
            None,
            None,
            true,
        )
        .unwrap_err();
        assert!(format!("{agy:#}").contains("cannot be resumed"), "{agy:#}");
    }

    /// BUG-2386: the interactive `codex resume` argv per posture.
    // trace:BUG-2386 | ai:claude
    #[test]
    fn codex_interactive_resume_argv_per_posture() {
        let tmp = tempfile::tempdir().unwrap();
        let lane = || {
            Some(CodexLane {
                tree: tmp.path(),
                role: None,
                host_main: None,
            })
        };
        assert_eq!(
            codex_interactive_resume_args("t-1", None, false, false).unwrap(),
            ["resume", "t-1"]
        );
        assert_eq!(
            codex_interactive_resume_args("t-1", None, true, false).unwrap(),
            [
                "--dangerously-bypass-approvals-and-sandbox",
                "resume",
                "t-1"
            ]
        );
        let native = codex_interactive_resume_args("t-1", lane(), true, false).unwrap();
        assert!(
            !native.iter().any(|a| a.contains("dangerously")),
            "a contained resume never carries the bypass: {native:?}"
        );
        assert_eq!(&native[native.len() - 2..], ["resume", "t-1"]);
        assert!(native.iter().any(|a| a == "workspace-write"), "{native:?}");
        assert!(codex_interactive_resume_args("-x", None, false, false).is_err());
        // BUG-2394: a plan-only resume keeps the read-only sandbox and never
        // the bypass, on either posture. trace:BUG-2394 | ai:claude
        assert_eq!(
            codex_interactive_resume_args("t-1", None, true, true).unwrap(),
            ["--sandbox", "read-only", "resume", "t-1"]
        );
        let plan_native = codex_interactive_resume_args("t-1", lane(), false, true).unwrap();
        assert_eq!(&plan_native[..2], ["--sandbox", "read-only"]);
        assert_eq!(
            plan_native.iter().filter(|a| *a == "--sandbox").count(),
            1,
            "{plan_native:?}"
        );
        assert!(!plan_native
            .iter()
            .any(|a| a.contains("dangerously") || a == "workspace-write"));
        assert!(plan_native.ends_with(&["resume".to_string(), "t-1".to_string()]));
        assert!(vendor_can_resume(HeadlessVendor::Codex));
        assert!(!vendor_can_resume(HeadlessVendor::Agy));
    }

    #[test]
    fn compose_headless_command_routes_per_vendor() {
        let _env = AgentCmdEnvGuard::acquire();
        let (prog, args) = compose_headless_command(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Codex,
            "do a thing",
            "sid",
            false,
            None,
            None,
        )
        .unwrap();
        assert!(prog.ends_with("codex"), "{prog}");
        assert_eq!(args.first().map(String::as_str), Some("exec"), "{args:?}");
        assert!(!args.contains(&"-p".to_string()), "{args:?}");

        let (prog, args) = compose_headless_command(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Claude,
            "do a thing",
            "sid",
            false,
            None,
            None,
        )
        .unwrap();
        assert!(prog.ends_with("claude"), "{prog}");
        assert!(args.contains(&"-p".to_string()), "{args:?}");
        assert_eq!(
            args,
            claude_headless_args("do a thing", "sid"),
            "claude arm must be byte-identical to the dedicated builder"
        );
    }

    /// STORY-761: the uniform `[agents] vendor` knob in the project
    /// `.aida/agents.toml` flips the headless vendor when neither the env
    /// override nor the per-surface `[orchestrator] headless_vendor` is set —
    /// and the per-surface config still wins over it.
    // trace:STORY-761 | ai:claude
    #[test]
    fn resolve_headless_vendor_agents_knob_fallback_and_precedence() {
        let _env = HeadlessVendorEnvGuard::acquire();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();

        // Knob alone → codex.
        std::fs::write(
            tmp.path().join(".aida/agents.toml"),
            "[agents]\nvendor = \"codex\"\n",
        )
        .unwrap();
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Codex);

        // Per-surface `[orchestrator] headless_vendor` beats the knob.
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            "[orchestrator]\nheadless_vendor = \"claude\"\n",
        )
        .unwrap();
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Claude);
    }

    /// TASK-1116: the per-invocation `--vendor`/`--agent` flag override is the
    /// TOP tier — it wins over BOTH the `AIDA_HEADLESS_VENDOR` env AND the
    /// config knobs. With no override installed, resolution is byte-identical
    /// to before (the env/knob/default tiers still apply).
    // trace:TASK-1116 | ai:claude
    #[test]
    fn resolve_headless_vendor_flag_override_wins_over_env_and_knob() {
        let mut _env = HeadlessVendorEnvGuard::acquire();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();

        // Set up the LOSING tiers: env says codex, the config knob says codex.
        _env.set("codex");
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            "[orchestrator]\nheadless_vendor = \"codex\"\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join(".aida/agents.toml"),
            "[agents]\nvendor = \"codex\"\n",
        )
        .unwrap();
        // Sanity: without the flag, the env (top of the pre-TASK-1116 order)
        // decides → codex.
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Codex);

        // The flag override wins over env + knob → claude.
        set_headless_vendor_override(Some(HeadlessVendor::Claude));
        assert_eq!(
            resolve_headless_vendor(tmp.path()),
            HeadlessVendor::Claude,
            "the per-invocation --vendor/--agent override must beat env + knob"
        );

        // Flipped: override codex still wins regardless of the lower tiers.
        _env.set("claude");
        set_headless_vendor_override(Some(HeadlessVendor::Codex));
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Codex);

        // TASK-1048: the agy override wins over the codex env + codex knobs too.
        _env.set("codex");
        set_headless_vendor_override(Some(HeadlessVendor::Agy));
        assert_eq!(
            resolve_headless_vendor(tmp.path()),
            HeadlessVendor::Agy,
            "the per-invocation --vendor agy override must beat env + knob"
        );

        // Clearing the override falls back to the env/knob tiers (now claude).
        _env.set("claude");
        set_headless_vendor_override(None);
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Claude);
    }

    /// TASK-1116: [`install_headless_vendor_override`] is the flag→override
    /// bridge the drive commands call. A recognized token sets the in-process
    /// tier (so an in-process spawn routes to it) AND exports
    /// `AIDA_HEADLESS_VENDOR` (so a child-subprocess implementer inherits it);
    /// an unrecognized token installs nothing and returns `None` so the caller
    /// can error or fall through — never a silent misroute.
    // trace:TASK-1116 | ai:claude
    #[test]
    fn install_headless_vendor_override_sets_tier_and_env() {
        let _env = HeadlessVendorEnvGuard::acquire();
        let tmp = tempfile::tempdir().unwrap();

        // Recognized token → override installed + env exported for children.
        assert_eq!(
            install_headless_vendor_override("codex"),
            Some(HeadlessVendor::Codex)
        );
        assert_eq!(headless_vendor_override(), Some(HeadlessVendor::Codex));
        assert_eq!(
            std::env::var("AIDA_HEADLESS_VENDOR").ok().as_deref(),
            Some("codex"),
            "the flag must export AIDA_HEADLESS_VENDOR so a child implementer inherits it"
        );
        // The headless implementer resolves to the flag value.
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Codex);

        // Case-insensitive alias-value parse (mirrors `HeadlessVendor::parse`).
        assert_eq!(
            install_headless_vendor_override(" Claude "),
            Some(HeadlessVendor::Claude)
        );
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Claude);

        // TASK-1048: the `antigravity` alias installs the Agy override and
        // exports the canonical `agy` token for a child implementer.
        assert_eq!(
            install_headless_vendor_override("antigravity"),
            Some(HeadlessVendor::Agy)
        );
        assert_eq!(headless_vendor_override(), Some(HeadlessVendor::Agy));
        assert_eq!(
            std::env::var("AIDA_HEADLESS_VENDOR").ok().as_deref(),
            Some("agy"),
            "the alias must export the canonical `agy` token, not `antigravity`"
        );
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Agy);

        // Unrecognized token → nothing installed, caller decides.
        set_headless_vendor_override(None);
        assert_eq!(install_headless_vendor_override("gemini"), None);
        assert_eq!(headless_vendor_override(), None);
    }

    /// `--bare` strips OAuth/keychain auth and breaks login (spike Q1) — the
    /// headless launch must never use it.
    #[test]
    fn claude_headless_args_never_uses_bare() {
        let args = claude_headless_args("/aida-review --pr 7", "sid");
        assert!(!args.iter().any(|a| a == "--bare"), "{args:?}");
        // Persistence stays ON — no `--no-session-persistence` either (Q9).
        assert!(
            !args.iter().any(|a| a == "--no-session-persistence"),
            "{args:?}"
        );
    }

    /// STORY-306: the headless-resume argv `--resume`s the punted session
    /// instead of minting a fresh `--session-id`, and keeps every SPIKE-7
    /// mandatory flag.
    #[test]
    fn claude_headless_resume_args_has_resume_and_no_session_id() {
        let sid = "019e0000-0000-7000-8000-000000000000";
        let args = claude_headless_resume_args("proceed with OAuth", sid);
        // Resumes the existing session — `--resume <id>`, not `--session-id`.
        assert!(args.contains(&"--resume".to_string()), "{args:?}");
        assert!(
            !args.iter().any(|a| a == "--session-id"),
            "resume must not mint a new session id: {args:?}"
        );
        assert!(args.contains(&sid.to_string()), "{args:?}");
        // SPIKE-7 mandatory flags intact.
        assert!(args.contains(&"-p".to_string()), "{args:?}");
        assert!(args.contains(&"bypassPermissions".to_string()), "{args:?}");
        assert!(args.contains(&"stream-json".to_string()), "{args:?}");
        assert!(args.contains(&"--verbose".to_string()), "{args:?}");
        assert!(!args.iter().any(|a| a == "--bare"), "{args:?}");
        // The resume prompt survives into the argv.
        assert!(args.contains(&"proceed with OAuth".to_string()), "{args:?}");
    }

    /// BUG-327: both headless argv builders must structurally disable
    /// `AskUserQuestion` so a headless reviewer / implementer / advisor
    /// cannot reason past the skill-template instruction added in BUG-280
    /// and bail without writing the verdict file. The flag is variadic on
    /// the claude side (`--disallowed-tools <tools...>`), so the *placement*
    /// matters: the next argv element after the value must start with `--`
    /// to terminate the value list — otherwise the prompt positional at
    /// the tail would be consumed as a second "disallowed tool". This test
    /// pins both the presence and the safe placement.
    /// trace:BUG-327 | ai:claude
    #[test]
    fn headless_argv_disables_askuserquestion_structurally() {
        for (label, args) in [
            ("fresh", claude_headless_args("/aida-review --pr 7", "sid")),
            ("resume", claude_headless_resume_args("proceed", "sid")),
        ] {
            let pos = args
                .iter()
                .position(|a| a == "--disallowed-tools")
                .unwrap_or_else(|| panic!("[{label}] --disallowed-tools missing: {args:?}"));
            assert_eq!(
                args.get(pos + 1).map(String::as_str),
                Some("AskUserQuestion"),
                "[{label}] AskUserQuestion must immediately follow --disallowed-tools: {args:?}",
            );
            // BUG-050: `Monitor` follows in the same value list.
            assert_eq!(
                args.get(pos + 2).map(String::as_str),
                Some("Monitor"),
                "[{label}] Monitor must be disallowed too: {args:?}",
            );
            // The variadic-terminator: the token after the tool list
            // must start with "--" or "-", otherwise the prompt at the
            // tail gets gobbled into the disallowed-tool list.
            let terminator = args
                .get(pos + 3)
                .unwrap_or_else(|| panic!("[{label}] value list runs off the end: {args:?}"));
            assert!(
                terminator.starts_with('-'),
                "[{label}] next argv element must be a flag (variadic terminator), \
                 got {terminator:?}: {args:?}",
            );
            // Prompt positional still lands at the tail.
            assert_eq!(
                args.last().map(String::as_str),
                if label == "fresh" {
                    Some("/aida-review --pr 7")
                } else {
                    Some("proceed")
                },
                "[{label}] prompt must remain the trailing positional: {args:?}",
            );
        }
    }

    // STORY-612: the write-confinement bwrap flags must (a) make the root
    // read-only, (b) rw-bind the worktree, (c) chdir into it, and crucially
    // (d) NEVER `--unshare-net` — `claude` needs api.anthropic.com.
    // trace:STORY-612 | ai:claude
    #[test]
    fn bwrap_confinement_is_ro_root_rw_worktree_shared_net() {
        let wt = Path::new("/home/joe/ai/aida-story-612");
        let store = wt.join(".aida-store");
        let flags = bwrap_confinement_args(wt, std::slice::from_ref(&store));

        // (a) read-only root: the tokens "--ro-bind", "/", "/" appear in order.
        let ro = flags
            .windows(3)
            .position(|w| w == ["--ro-bind", "/", "/"])
            .expect("must --ro-bind / / (read-only root)");
        assert_eq!(
            ro, 0,
            "ro-root bind must come first so later binds override"
        );

        // (b) the worktree is hard-bound rw.
        let wt_s = wt.to_string_lossy().to_string();
        assert!(
            flags
                .windows(3)
                .any(|w| w[0] == "--bind" && w[1] == wt_s && w[2] == wt_s),
            "worktree must be rw --bind: {flags:?}"
        );
        // store is rw via --bind-try (skipped if absent).
        let store_s = store.to_string_lossy().to_string();
        assert!(
            flags
                .windows(3)
                .any(|w| w[0] == "--bind-try" && w[1] == store_s && w[2] == store_s),
            "store must be rw --bind-try: {flags:?}"
        );

        // (c) chdir into the worktree.
        let chdir = flags.iter().position(|a| a == "--chdir").expect("--chdir");
        assert_eq!(
            flags.get(chdir + 1).map(String::as_str),
            Some(wt_s.as_str())
        );

        // (d) network is SHARED — no net unshare anywhere.
        assert!(
            !flags.iter().any(|a| a == "--unshare-net"),
            "must NOT unshare net (claude needs the API): {flags:?}"
        );
        // tears down with the parent.
        assert!(flags.iter().any(|a| a == "--die-with-parent"), "{flags:?}");
    }

    // STORY-612: when os_wrap is OFF (the default — no `[contained] os_wrap`
    // in this repo's config), the launch is the bare `claude` argv, byte-for-
    // byte unchanged. This is the "unchanged posture" invariant mirroring the
    // slice-1 empty-allowlist test. trace:STORY-612 | ai:claude
    #[test]
    fn os_wrap_off_leaves_launch_unwrapped() {
        // Clean, serialized os_wrap env baseline (BUG-581). trace:BUG-581
        let _env = OsWrapEnvGuard::acquire();
        // A project root with no `[contained] os_wrap` (here: an empty temp dir
        // with no config at all) must return ("claude", <args unchanged>).
        let tmp = tempfile::tempdir().unwrap();
        assert!(!os_wrap_enabled(tmp.path()), "absent config => os_wrap off");
        let claude_args = claude_headless_args("/aida-review", "sid");
        let (program, args) = claude_program_and_args(None, tmp.path(), claude_args.clone())
            .expect("unwrapped launch never errors");
        assert_eq!(
            program, "claude",
            "default posture must exec claude directly"
        );
        assert_eq!(
            args, claude_args,
            "argv must be unchanged when os_wrap is off"
        );
    }

    /// STORY-612 — the automated live-verify gate. Adapts to the host:
    ///
    /// - On a host where bubblewrap CAN create an unprivileged user namespace
    ///   (e.g. GitHub `ubuntu-latest` runners), this spawns `bwrap` with the
    ///   EXACT confinement flags the launcher emits and proves the write-
    ///   confinement property live: a write INSIDE the worktree succeeds and a
    ///   write OUTSIDE (the read-only root) is blocked. If `claude` is on PATH
    ///   it also runs `claude --version` inside the wrapper (best-effort — the
    ///   binary is absent on CI runners). This is the live gate the PR's merge
    ///   is blocked on.
    /// - On a userns-RESTRICTED host (this dev box; or any non-Linux where
    ///   `bwrap` is absent) it instead asserts the FAIL-CLOSED contract: an
    ///   os_wrap-enabled launch returns an Err carrying the remediation, and
    ///   never silently falls through to an unconfined run.
    ///
    /// trace:STORY-612 | ai:claude
    #[test]
    fn bwrap_write_confinement_live_or_fail_closed() {
        // Clean, serialized os_wrap env baseline (BUG-581) — a leaked
        // AIDA_OS_WRAP=0 would otherwise force this on-config launch off.
        // trace:BUG-581
        let _env = OsWrapEnvGuard::acquire();
        // A worktree with `[contained] os_wrap = true`.
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path();
        std::fs::create_dir_all(wt.join(".git")).unwrap();
        std::fs::create_dir_all(wt.join(".aida")).unwrap();
        std::fs::write(
            wt.join(".aida/config.toml"),
            "[contained]\nos_wrap = true\n",
        )
        .unwrap();
        assert!(os_wrap_enabled(wt), "config must enable os_wrap");

        let dummy = claude_headless_args("/aida-review", "sid");

        // Can bubblewrap actually set up a userns on this host?
        if bwrap_preflight().is_err() {
            // CI sets AIDA_REQUIRE_BWRAP_LIVE=1 to force the live arm so a green
            // check can never come from the fail-closed branch silently. If the
            // runner can't create a userns, that is a CI-environment failure, not
            // a pass. trace:STORY-612 | ai:claude
            assert!(
                std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none(),
                "AIDA_REQUIRE_BWRAP_LIVE=1 but bwrap cannot create an unprivileged user \
                 namespace here — the live-confinement gate could not run. Install bubblewrap \
                 and/or `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`."
            );
            // Restricted host (no force): the launch MUST fail closed with remediation.
            let err = claude_program_and_args(None, wt, dummy)
                .expect_err("userns-restricted host must NOT yield a runnable wrapped command");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("Refusing to launch the drain unconfined"),
                "fail-closed error must spell out it won't run unconfined: {msg}"
            );
            return;
        }

        // Live path: the launcher wraps with bwrap…
        let (program, _args) =
            claude_program_and_args(None, wt, dummy).expect("wrapped launch builds");
        assert_eq!(program, "bwrap", "userns-capable host must wrap with bwrap");

        // …and the EXACT flags it emits actually confine writes. Probe inside
        // the worktree (rw bind → succeeds) vs. a path on the read-only root
        // (blocked). Pass paths via env; bwrap forwards the environment.
        let inside = wt.join("probe_in");
        let outside = format!("/aida-oswrap-probe-out-{}", std::process::id());
        let flags = bwrap_confinement_args(wt, &[]);
        let probe = "touch \"$AIDA_PROBE_IN\" && \
             (touch \"$AIDA_PROBE_OUT\" 2>/dev/null && echo LEAK || echo BLOCKED)";
        let out = std::process::Command::new("bwrap")
            .args(&flags)
            .args(["sh", "-c", probe])
            .env("AIDA_PROBE_IN", &inside)
            .env("AIDA_PROBE_OUT", &outside)
            .output()
            .expect("spawn bwrap probe");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("BLOCKED") && !stdout.contains("LEAK"),
            "write to the read-only root must be blocked: stdout={stdout:?} stderr={:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            inside.exists(),
            "write inside the rw-bound worktree must persist to the host"
        );
        assert!(
            !Path::new(&outside).exists(),
            "the blocked outside write must not have leaked onto the host"
        );

        // Best-effort: if claude is installed, confirm it launches inside the
        // wrapper (auth dir bound, binary resolves). Skipped on CI runners.
        if which_on_path("claude").is_some() {
            let mut full = bwrap_confinement_args(wt, &[]);
            full.push("claude".into());
            full.push("--version".into());
            let st = std::process::Command::new("bwrap")
                .args(&full)
                .output()
                .expect("spawn claude --version under bwrap");
            assert!(
                st.status.success(),
                "claude --version must run inside the wrapper: {:?}",
                String::from_utf8_lossy(&st.stderr)
            );
        }
    }

    /// TASK-876: the `AIDA_OS_WRAP` per-host override takes PRECEDENCE over the
    /// tracked config value, in BOTH directions, and recognizes the documented
    /// truthy/falsey spellings (case-insensitive). An unrecognized value falls
    /// through to the config. These tests mutate a process-global env var, so
    /// they run serially under a shared mutex. trace:TASK-876 | ai:claude
    #[test]
    fn aida_os_wrap_env_overrides_config() {
        // Serialize env mutation across the os_wrap tests + start clean; the
        // guard's drop restores the ambient AIDA_OS_WRAP (BUG-581). trace:BUG-581
        let mut _env = OsWrapEnvGuard::acquire();

        // Config says OFF (no config at all).
        let off_dir = tempfile::tempdir().unwrap();
        // Config says ON.
        let on_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(on_dir.path().join(".aida")).unwrap();
        std::fs::write(
            on_dir.path().join(".aida/config.toml"),
            "[contained]\nos_wrap = true\n",
        )
        .unwrap();

        // Override ON forces true even when config is off.
        for truthy in ["1", "true", "TRUE", "Yes", " yes "] {
            _env.set(truthy);
            assert!(
                os_wrap_enabled(off_dir.path()),
                "AIDA_OS_WRAP={truthy:?} must force os_wrap ON over an off config"
            );
        }

        // Override OFF forces false even when config is on.
        for falsey in ["0", "false", "FALSE", "No", " no "] {
            _env.set(falsey);
            assert!(
                !os_wrap_enabled(on_dir.path()),
                "AIDA_OS_WRAP={falsey:?} must force os_wrap OFF over an on config"
            );
        }

        // Unrecognized value → ignored → config wins.
        _env.set("maybe");
        assert!(
            !os_wrap_enabled(off_dir.path()),
            "garbage AIDA_OS_WRAP must fall through to the (off) config"
        );
        assert!(
            os_wrap_enabled(on_dir.path()),
            "garbage AIDA_OS_WRAP must fall through to the (on) config"
        );

        // Unset → config wins in both directions.
        _env.unset();
        assert!(!os_wrap_enabled(off_dir.path()));
        assert!(os_wrap_enabled(on_dir.path()));
    }

    /// TASK-864: the INTERACTIVE launch path (which calls
    /// `os_wrapped_program_and_args` with the PATH-resolved claude binary) is
    /// byte-identical to a bare exec when os_wrap is OFF, and produces a
    /// `bwrap … <binary> …` wrap (or a fail-closed Err on a userns-restricted
    /// host) when os_wrap is ON. trace:TASK-864 | ai:claude
    #[test]
    fn interactive_path_wraps_when_enabled_unchanged_when_off() {
        // Serialize + clean env baseline; the guard restores it on drop
        // (BUG-581). trace:BUG-581
        let mut _env = OsWrapEnvGuard::acquire();

        let resolved_binary = "/usr/local/bin/claude";
        let args = vec![
            "--dangerously-skip-permissions".to_string(),
            "--settings".to_string(),
            "/tmp/x.json".to_string(),
        ];

        // OFF (no config, no env) → bare program + args unchanged.
        let off_dir = tempfile::tempdir().unwrap();
        let (program, out_args) = os_wrapped_program_and_args(
            crate::seat_clone::launcher_main().as_deref(),
            off_dir.path(),
            resolved_binary,
            args.clone(),
        )
        .expect("off path never errors");
        assert_eq!(
            program, resolved_binary,
            "os_wrap OFF must exec the resolved binary directly"
        );
        assert_eq!(out_args, args, "args unchanged when off");

        // ON via the env override.
        _env.set("1");
        let on_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(on_dir.path().join(".git")).unwrap();
        match os_wrapped_program_and_args(
            crate::seat_clone::launcher_main().as_deref(),
            on_dir.path(),
            resolved_binary,
            args.clone(),
        ) {
            Ok((program, out_args)) => {
                // userns-capable host: bwrap wrap with the resolved binary inside.
                assert_eq!(program, "bwrap", "enabled => wrap with bwrap");
                assert!(
                    out_args.iter().any(|a| a == resolved_binary),
                    "wrapped argv must invoke the resolved binary: {out_args:?}"
                );
                let bin_pos = out_args.iter().position(|a| a == resolved_binary).unwrap();
                assert_eq!(
                    &out_args[bin_pos + 1..],
                    &args[..],
                    "the claude args must follow the binary unchanged"
                );
                assert!(
                    out_args.iter().any(|a| a == "--die-with-parent"),
                    "confinement flags must be present: {out_args:?}"
                );
            }
            Err(e) => {
                // userns-restricted host: fail-closed, never a silent unconfined run.
                let msg = format!("{e:#}");
                assert!(
                    msg.contains("Refusing to launch the drain unconfined")
                        || msg.contains("os_wrap"),
                    "enabled-but-unrunnable must fail closed with remediation: {msg}"
                );
            }
        }
    }

    // trace:BUG-2001 | ai:antigravity
    #[test]
    fn os_wrap_outside_git_refuses_instead_of_binding_cwd_rw() {
        let mut _env = OsWrapEnvGuard::acquire();
        _env.set("1");

        let outside_git = tempfile::tempdir().unwrap();
        let resolved_binary = "/bin/echo";
        let args = vec!["--print".to_string(), "hi".to_string()];

        let err = os_wrapped_program_and_args(
            crate::seat_clone::launcher_main().as_deref(),
            outside_git.path(),
            resolved_binary,
            args.clone(),
        )
        .expect_err("os_wrap outside git must refuse before binding non-repo dir read-write");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not inside a git repository")
                && msg.contains("refusing to bind a non-repository directory read-write"),
            "refusal must explain non-repository os_wrap failure: {msg}"
        );

        let _cwd = contained_cwd_seam::pin(outside_git.path());
        assert_eq!(
            headless_worktree_root(),
            outside_git.path().to_path_buf(),
            "headless_worktree_root must honor contained_cwd_seam in unit tests"
        );
        let err2 = claude_program_and_args(None, &headless_worktree_root(), args)
            .expect_err("headless claude_program_and_args outside git must refuse os_wrap");
        assert!(
            format!("{err2:#}").contains("not inside a git repository"),
            "headless path must refuse outside-git os_wrap: {err2:#}"
        );
    }

    // trace:BUG-2006 | ai:antigravity
    #[test]
    fn story_2326_spawn_reviewer_launch_plan_contains_or_refuses_stale_uncontained_plan() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join(".aida-store")).unwrap();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        std::fs::write(
            project.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        std::env::set_current_dir(&project).unwrap();
        let fake = tmp.path().join("fake-agent");
        let env_out = tmp.path().join("env.txt");
        crate::test_exec::write_executable(
            &fake,
            format!("#!/bin/sh\nenv | sort > '{}'\nexit 0\n", env_out.display()),
        );

        // 1. A stale uncontained plan under a contained parent refuses before spawn.
        {
            let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
                &project,
                "advisor",
                &["reviewer"],
                &[("AIDA_CONTAINED", Some("1"))],
            );
            let plan = ReviewerLaunchPlan {
                vendor: HeadlessVendor::Claude,
                program: fake.to_string_lossy().to_string(),
                args: vec![],
                contained: false,
                host_main: None,
            };
            let err = spawn_reviewer_launch_plan(&plan, &project).unwrap_err();
            assert!(err
                .to_string()
                .contains("reviewer launch plan is uncontained"));
            assert!(!env_out.exists());
        }

        // 2. Contained = true without ambient AIDA_CONTAINED -> exported in child
        {
            let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
                &project,
                "advisor",
                &["reviewer"],
                &[("AIDA_CONTAINED", None)],
            );
            let plan = ReviewerLaunchPlan {
                vendor: HeadlessVendor::Claude,
                program: fake.to_string_lossy().to_string(),
                args: vec![],
                contained: true,
                host_main: crate::seat_clone::launcher_main(),
            };
            let status = spawn_reviewer_launch_plan(&plan, &project).unwrap();
            assert!(status.success());
            let env_contents = std::fs::read_to_string(&env_out).unwrap();
            assert!(
                env_contents.lines().any(|l| l == "AIDA_CONTAINED=1"),
                "contained reviewer launch must export AIDA_CONTAINED=1 to child: {env_contents}"
            );
        }
    }

    // trace:BUG-2006 | ai:antigravity
    #[test]
    fn spawn_claude_resume_inherits_or_explicitly_applies_containment() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("fake-claude");
        let env_out = tmp.path().join("env.txt");
        crate::test_exec::write_executable(
            &fake,
            format!("#!/bin/sh\nenv | sort > '{}'\nexit 0\n", env_out.display()),
        );

        // 1. Caller false with ambient containment -> contained child
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
                ("AIDA_CONTAINED", Some("1")),
            ]);
            let status = spawn_claude_resume(
                crate::seat_clone::launcher_main().as_deref(),
                "session-123",
                None,
                false,
            )
            .unwrap();
            assert!(status.success());
            let env_contents = std::fs::read_to_string(&env_out).unwrap();
            assert!(
                env_contents.lines().any(|l| l == "AIDA_CONTAINED=1"),
                "spawn_claude_resume must inherit containment: {env_contents}"
            );
        }

        // 2. Contained without ambient AIDA_CONTAINED -> exported in child
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
                ("AIDA_CONTAINED", None),
            ]);
            let status = spawn_claude_resume(
                crate::seat_clone::launcher_main().as_deref(),
                "session-123",
                None,
                true,
            )
            .unwrap();
            assert!(status.success());
            let env_contents = std::fs::read_to_string(&env_out).unwrap();
            assert!(
                env_contents.lines().any(|l| l == "AIDA_CONTAINED=1"),
                "contained spawn_claude_resume must export AIDA_CONTAINED=1: {env_contents}"
            );
        }
    }

    // trace:BUG-2172 | ai:codex
    #[test]
    #[cfg(unix)]
    fn interactive_reviewer_and_resume_inherit_parent_before_spawn() {
        // A contained parent refuses the `AIDA_AGENT_CMD` seam (STORY-2326),
        // so the fake is the `claude` on PATH, in its own process.
        // trace:STORY-2326 | ai:claude
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("fake-claude");
        let capture = tmp.path().join("capture");
        crate::test_exec::write_executable(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$AIDA_CONTAINED\" \"$@\" > '{}'\n",
                capture.display()
            ),
        );
        let path = path_with_fake_claude(tmp.path(), &fake);
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            tmp.path(),
            "advisor",
            &["reviewer"],
            &[
                ("AIDA_AGENT_CMD", None),
                ("PATH", path.to_str()),
                ("AIDA_CONTAINED", Some("1")),
                ("AIDA_OS_WRAP", Some("0")),
            ],
        );
        let plan = interactive_reviewer_launch_plan(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Claude,
            None,
            None,
            "review",
            "sid",
            false,
        )
        .unwrap();
        assert!(plan.contained);
        assert!(plan.args.iter().any(|a| a == "--settings"));
        let (status, grant) = spawn_reviewer_launch_plan_granted(&plan, tmp.path()).unwrap();
        assert!(status.success());
        revoke_headless_child_grant(&grant);
        let output = std::fs::read_to_string(&capture).unwrap();
        assert!(output.starts_with("1\n"), "{output}");
        assert!(output.lines().any(|l| l == "--settings"));
        std::fs::remove_file(&capture).unwrap();
        let host_main = crate::seat_clone::launcher_main();
        let status =
            spawn_claude_resume_with_grant(host_main.as_deref(), "sid", None, false, None, None)
                .unwrap();
        assert!(status.success());
        let output = std::fs::read_to_string(&capture).unwrap();
        assert!(output.starts_with("1\n--resume\nsid\n"), "{output}");
        assert!(output.lines().any(|l| l == "--settings"));
        std::fs::remove_file(&capture).unwrap();
        assert!(spawn_claude_resume_with_grant(
            host_main.as_deref(),
            "sid",
            Some("bypassPermissions"),
            false,
            None,
            None
        )
        .is_err());
        assert!(!capture.exists());
        assert!(interactive_reviewer_launch_plan(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Codex,
            None,
            None,
            "review",
            "sid",
            false,
        )
        .is_err());
    }

    /// A PATH whose first entry holds `fake` as `claude`.
    // trace:STORY-2326 | ai:claude
    fn path_with_fake_claude(dir: &std::path::Path, fake: &std::path::Path) -> std::ffi::OsString {
        let bin = dir.join("fake-bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::copy(fake, bin.join("claude")).unwrap();
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap()
    }

    // trace:BUG-2006 | ai:antigravity
    #[test]
    fn spawn_vendor_headless_with_seat_preserves_parent_containment() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join(".aida-store")).unwrap();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        std::fs::write(
            project.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        let fake = tmp.path().join("fake-claude");
        let env_out = tmp.path().join("env.txt");
        crate::test_exec::write_executable(
            &fake,
            format!("#!/bin/sh\nenv | sort > '{}'\nexit 0\n", env_out.display()),
        );
        let log_path = tmp.path().join("headless.log");
        let tee_opts = crate::headless_tee::TeeOptions {
            enabled: false,
            label: None,
        };

        // trace:BUG-2172 | ai:codex
        // 1. Caller passes false, but contained parent must win. A contained
        // parent refuses `AIDA_AGENT_CMD` (STORY-2326): the fake is on PATH.
        // trace:STORY-2326 | ai:claude
        {
            let path = path_with_fake_claude(tmp.path(), &fake);
            let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
                &project,
                "advisor",
                &["implementer"],
                &[
                    ("AIDA_AGENT_CMD", None),
                    ("PATH", path.to_str()),
                    ("AIDA_OS_WRAP", Some("0")),
                    ("AIDA_CONTAINED", Some("1")),
                ],
            );
            let status = spawn_vendor_headless_with_seat(
                HeadlessVendor::Claude,
                aida_core::agents_config::AgentSeat::Implementer,
                "prompt",
                "sid",
                &log_path,
                &tee_opts,
                false,
            )
            .unwrap();
            assert!(status.success());
            let env_contents = std::fs::read_to_string(&env_out).unwrap();
            assert!(
                env_contents.lines().any(|l| l == "AIDA_CONTAINED=1"),
                "contained parent must force AIDA_CONTAINED=1: {env_contents}"
            );
        }

        // 2. Contained without ambient AIDA_CONTAINED -> exported in child
        {
            let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
                &project,
                "advisor",
                &["implementer"],
                &[
                    ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
                    ("AIDA_OS_WRAP", Some("0")),
                    ("AIDA_CONTAINED", None),
                ],
            );
            let status = spawn_vendor_headless_with_seat(
                HeadlessVendor::Claude,
                aida_core::agents_config::AgentSeat::Implementer,
                "prompt",
                "sid",
                &log_path,
                &tee_opts,
                true,
            )
            .unwrap();
            assert!(status.success());
            let env_contents = std::fs::read_to_string(&env_out).unwrap();
            assert!(
                env_contents.lines().any(|l| l == "AIDA_CONTAINED=1"),
                "contained spawn_vendor_headless must export AIDA_CONTAINED=1: {env_contents}"
            );
        }
    }

    // trace:BUG-2006 | ai:antigravity
    #[test]
    fn spawn_claude_headless_resume_preserves_parent_containment() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join(".aida-store")).unwrap();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        std::fs::write(
            project.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        let fake = tmp.path().join("fake-claude");
        let env_out = tmp.path().join("env.txt");
        crate::test_exec::write_executable(
            &fake,
            format!("#!/bin/sh\nenv | sort > '{}'\nexit 0\n", env_out.display()),
        );
        let log_path = tmp.path().join("headless.log");
        let tee_opts = crate::headless_tee::TeeOptions {
            enabled: false,
            label: None,
        };

        // trace:BUG-2172 | ai:codex
        // 1. Literal false cannot override parent containment.
        {
            let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
                &project,
                "advisor",
                &["implementer"],
                &[
                    ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
                    ("AIDA_OS_WRAP", Some("0")),
                    ("AIDA_CONTAINED", Some("1")),
                ],
            );
            let status = spawn_claude_headless_resume(
                crate::seat_clone::launcher_main().as_deref(),
                "prompt",
                "sid",
                &log_path,
                &project,
                &tee_opts,
                false,
                "implementer",
            )
            .unwrap();
            assert!(status.success());
            let env_contents = std::fs::read_to_string(&env_out).unwrap();
            assert!(
                env_contents.lines().any(|l| l == "AIDA_CONTAINED=1"),
                "contained parent must force AIDA_CONTAINED=1: {env_contents}"
            );
        }

        // 2. Contained without ambient AIDA_CONTAINED -> exported in child
        {
            let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
                &project,
                "advisor",
                &["implementer"],
                &[
                    ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
                    ("AIDA_OS_WRAP", Some("0")),
                    ("AIDA_CONTAINED", None),
                ],
            );
            let status = spawn_claude_headless_resume(
                crate::seat_clone::launcher_main().as_deref(),
                "prompt",
                "sid",
                &log_path,
                &project,
                &tee_opts,
                true,
                "implementer",
            )
            .unwrap();
            assert!(status.success());
            let env_contents = std::fs::read_to_string(&env_out).unwrap();
            assert!(
                env_contents.lines().any(|l| l == "AIDA_CONTAINED=1"),
                "contained spawn_claude_headless_resume must export AIDA_CONTAINED=1: {env_contents}"
            );
        }
    }

    // trace:BUG-2006 | ai:antigravity
    #[test]
    fn containment_applied_reports_only_claude_as_contained() {
        assert!(containment_applied(HeadlessVendor::Claude, true, None));
        assert!(!containment_applied(HeadlessVendor::Claude, false, None));
        assert!(!containment_applied(HeadlessVendor::Codex, true, None));
        assert!(!containment_applied(HeadlessVendor::Codex, false, None));
        assert!(!containment_applied(HeadlessVendor::Agy, true, None));
        assert!(!containment_applied(HeadlessVendor::Agy, false, None));
    }

    /// M2: the one mint/preflight question. Claude credits as before; only
    /// Codex consults the posture, so Agy never fails here; contained Codex
    /// with no trusted opt-in fails closed instead of minting an unsandboxed
    /// grant. (The `aida` posture arm runs under the TASK-2708 policy tests.)
    // trace:TASK-2708 | ai:claude
    #[test]
    fn task_2708_sandboxed_child_grant_is_the_one_mint_question() {
        assert!(sandboxed_child_grant(HeadlessVendor::Claude, true).unwrap());
        assert!(!sandboxed_child_grant(HeadlessVendor::Claude, false).unwrap());
        assert!(!sandboxed_child_grant(HeadlessVendor::Codex, false).unwrap());
        assert!(!sandboxed_child_grant(HeadlessVendor::Agy, true).unwrap());
        assert!(!sandboxed_child_grant(HeadlessVendor::Agy, false).unwrap());
        assert!(sandboxed_child_grant(HeadlessVendor::Codex, true).is_err());
    }

    // trace:BUG-2006 | ai:antigravity
    // BUG-2144: this test used to pin the OPPOSITE behaviour — a contained
    // codex/agy launch silently ran uncontained (AIDA_CONTAINED stripped).
    // The BUG-2133 ruling (containment is monotonic, fail closed) reverses
    // that intent: a contained request on a vendor that cannot apply it is
    // REFUSED, and the vendor binary never runs. The uncontained control has
    // no ambient containment request. trace:BUG-2144 | ai:claude
    // trace:BUG-2172 | ai:codex
    #[test]
    fn spawn_vendor_headless_with_seat_codex_and_agy_refuse_contained_launch() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join(".aida-store")).unwrap();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        std::fs::write(
            project.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        let fake = tmp.path().join("fake-vendor");
        let env_out = tmp.path().join("env.txt");
        // Emit valid AGY terminal evidence for the uncontained control.
        // trace:TASK-2838 | ai:codex
        crate::test_exec::write_executable(
            &fake,
            format!("#!/bin/sh\ncase \" $* \" in *\" --probe-seat \"*) echo \"seat_grant_valid=true expected_seat=true\"; exit 0;; esac\nenv | sort > '{}'\nprintf '%s\\n' '{{\"event\":\"result\",\"result\":{{\"status\":\"SUCCESS\"}}}}'\nexit 0\n", env_out.display()),
        );
        let log_path = tmp.path().join("headless.log");
        let tee_opts = crate::headless_tee::TeeOptions {
            enabled: false,
            label: None,
        };

        for vendor in [HeadlessVendor::Codex, HeadlessVendor::Agy] {
            let _ = std::fs::remove_file(&env_out);
            {
                let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
                    &project,
                    "advisor",
                    &["implementer"],
                    &[
                        ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
                        ("AIDA_OS_WRAP", Some("0")),
                        ("AIDA_CONTAINED", Some("1")),
                    ],
                );
                // Both explicit and ambient-only containment refuse before spawn.
                for contained in [true, false] {
                    let err = spawn_vendor_headless_with_seat(
                        vendor,
                        aida_core::agents_config::AgentSeat::Implementer,
                        "prompt",
                        "sid",
                        &log_path,
                        &tee_opts,
                        contained,
                    )
                    .expect_err("explicit or ambient containment must refuse codex/agy");
                    let msg = format!("{err:#}");
                    assert!(
                        msg.contains("refusing to launch") && msg.contains(vendor.as_str()),
                        "{vendor:?}: {msg}"
                    );
                    assert!(
                        !env_out.exists(),
                        "{vendor:?}: the vendor binary must not run on a refused contained launch"
                    );
                }
            }

            // Uncontained control -> runs only with ambient containment unset.
            let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
                &project,
                "advisor",
                &["implementer"],
                &[
                    ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
                    ("AIDA_OS_WRAP", Some("0")),
                    ("AIDA_CONTAINED", None),
                ],
            );
            let result = spawn_vendor_headless_with_seat(
                vendor,
                aida_core::agents_config::AgentSeat::Implementer,
                "prompt",
                "sid",
                &log_path,
                &tee_opts,
                false,
            );
            // trace:TASK-3048 | ai:codex
            if vendor == HeadlessVendor::Agy {
                let error = result.expect_err("unreceipted AGY bypass must refuse");
                assert!(format!("{error:#}").contains("applied AIDA ContainmentReceipt"));
                assert!(!env_out.exists());
                continue;
            }
            let status = result.unwrap();
            assert!(status.success(), "{vendor:?}: {status}");
            let env_contents = std::fs::read_to_string(&env_out).unwrap();
            assert!(
                !env_contents
                    .lines()
                    .any(|l| l.starts_with("AIDA_CONTAINED=")),
                "{vendor:?}: an uncontained launch must strip AIDA_CONTAINED: {env_contents}"
            );
        }
    }

    // trace:BUG-2006 | ai:antigravity
    #[test]
    fn interactive_reviewer_launch_plan_for_codex_refuses_contained_and_stale_uncontained_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join(".aida-store")).unwrap();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        std::fs::write(
            project.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        let fake = tmp.path().join("fake-codex");
        let env_out = tmp.path().join("env.txt");
        crate::test_exec::write_executable(
            &fake,
            format!("#!/bin/sh\nenv | sort > '{}'\nexit 0\n", env_out.display()),
        );

        // BUG-2144: a contained Codex reviewer used to be planned uncontained;
        // per the BUG-2133 ruling it is refused instead. trace:BUG-2144 | ai:claude
        let err = interactive_reviewer_launch_plan(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Codex,
            Some("bypassPermissions"),
            Some("codex-reviewer"),
            "review prompt",
            "sid-123",
            true,
        )
        .expect_err("a contained Codex reviewer must be refused");
        assert!(
            format!("{err:#}").contains("refusing to launch the reviewer session"),
            "{err:#}"
        );

        let plan = {
            let _env = AgentCmdEnvGuard::acquire();
            interactive_reviewer_launch_plan(
                None,
                HeadlessVendor::Codex,
                Some("bypassPermissions"),
                Some("codex-reviewer"),
                "review prompt",
                "sid-123",
                false,
            )
            .unwrap()
        };
        assert!(
            !plan.contained,
            "Codex reviewer plan must never be marked contained"
        );

        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            &project,
            "advisor",
            &["reviewer"],
            &[
                ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
                ("AIDA_CONTAINED", Some("1")),
            ],
        );
        let mut plan_with_fake = plan;
        plan_with_fake.program = fake.to_string_lossy().to_string();
        let err = spawn_reviewer_launch_plan(&plan_with_fake, &project).unwrap_err();
        assert!(format!("{err:#}").contains("refusing to launch the reviewer session"));
        assert!(!env_out.exists(), "stale plan must refuse before spawning");
    }

    // trace:BUG-2006 | ai:antigravity
    #[test]
    fn build_claude_exec_command_exports_when_contained_and_strips_when_uncontained() {
        let cmd_contained = build_claude_exec_command(
            crate::seat_clone::launcher_main().as_deref(),
            None,
            None,
            Some("prompt"),
            Some("sid"),
            true,
            None,
            None,
        )
        .unwrap();
        let envs: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> =
            cmd_contained.get_envs().collect();
        assert!(
            envs.iter()
                .any(|(k, v)| *k == "AIDA_CONTAINED" && *v == Some(std::ffi::OsStr::new("1"))),
            "contained build_claude_exec_command must export AIDA_CONTAINED=1: {envs:?}"
        );

        let cmd_uncontained = build_claude_exec_command(
            crate::seat_clone::launcher_main().as_deref(),
            None,
            None,
            Some("prompt"),
            Some("sid"),
            false,
            None,
            None,
        )
        .unwrap();
        let envs_uncontained: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> =
            cmd_uncontained.get_envs().collect();
        assert!(
            envs_uncontained
                .iter()
                .all(|(k, v)| *k != "AIDA_CONTAINED" || v.is_none()),
            "uncontained build_claude_exec_command must strip AIDA_CONTAINED: {envs_uncontained:?}"
        );
    }

    // trace:BUG-2006 | ai:antigravity
    #[test]
    fn build_run_claude_session_command_exports_when_contained_and_strips_when_uncontained() {
        let cmd_contained = build_run_claude_session_command(
            crate::seat_clone::launcher_main().as_deref(),
            None,
            None,
            Some("prompt"),
            Some("sid"),
            true,
            None,
        )
        .unwrap();
        let envs: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> =
            cmd_contained.get_envs().collect();
        assert!(
            envs.iter()
                .any(|(k, v)| *k == "AIDA_CONTAINED" && *v == Some(std::ffi::OsStr::new("1"))),
            "contained build_run_claude_session_command must export AIDA_CONTAINED=1: {envs:?}"
        );

        let cmd_uncontained = build_run_claude_session_command(
            crate::seat_clone::launcher_main().as_deref(),
            None,
            None,
            Some("prompt"),
            Some("sid"),
            false,
            None,
        )
        .unwrap();
        let envs_uncontained: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> =
            cmd_uncontained.get_envs().collect();
        assert!(
            envs_uncontained
                .iter()
                .all(|(k, v)| *k != "AIDA_CONTAINED" || v.is_none()),
            "uncontained build_run_claude_session_command must strip AIDA_CONTAINED: {envs_uncontained:?}"
        );
    }

    // trace:BUG-2006 | ai:antigravity
    #[test]
    fn build_claude_resume_exec_command_exports_when_contained_and_strips_when_uncontained() {
        let cmd_contained = build_claude_resume_exec_command(
            crate::seat_clone::launcher_main().as_deref(),
            "session-123",
            None,
            true,
            None,
        )
        .unwrap();
        let envs: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> =
            cmd_contained.get_envs().collect();
        assert!(
            envs.iter()
                .any(|(k, v)| *k == "AIDA_CONTAINED" && *v == Some(std::ffi::OsStr::new("1"))),
            "contained build_claude_resume_exec_command must export AIDA_CONTAINED=1: {envs:?}"
        );

        let cmd_uncontained = build_claude_resume_exec_command(
            crate::seat_clone::launcher_main().as_deref(),
            "session-123",
            None,
            false,
            None,
        )
        .unwrap();
        let envs_uncontained: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> =
            cmd_uncontained.get_envs().collect();
        assert!(
            envs_uncontained
                .iter()
                .any(|(k, v)| *k == "AIDA_CONTAINED" && *v == None),
            "uncontained build_claude_resume_exec_command must strip AIDA_CONTAINED: {envs_uncontained:?}"
        );
    }

    // trace:BUG-2006 | ai:antigravity
    #[test]
    fn build_codex_session_command_strips_aida_contained() {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
        let cmd = build_codex_session_command("prompt", false, false, None, None).unwrap();
        let envs: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> = cmd.get_envs().collect();
        assert!(
            envs.iter()
                .any(|(k, v)| *k == "AIDA_CONTAINED" && *v == None),
            "build_codex_session_command must explicitly strip AIDA_CONTAINED: {envs:?}"
        );
    }

    /// BUG-2139: the child grant stamped on a headless lease is revoked when
    /// the lease ends or is reaped — the crash-leak window where the launcher
    /// died before it could revoke on the child's exit. An unstamped lease
    /// revokes nothing.
    // trace:BUG-2139 | ai:claude
    #[test]
    fn lease_removal_revokes_the_stamped_child_grant() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let _lock = crate::test_env::env_lock();
        let subject = "bug-2139-lease-user";
        let id = crate::gate::seat_authority::test_support::mint_grant_for(
            root,
            subject,
            "implementer",
            &[],
        );
        let other = crate::gate::seat_authority::test_support::mint_grant_for(
            root,
            subject,
            "implementer",
            &[],
        );
        let sessions = root.join(".aida").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        for lease in ["stamped", "plain"] {
            std::fs::write(
                sessions.join(format!("{lease}.toml")),
                format!("id = \"{lease}\"\nactive_pid = 1\n"),
            )
            .unwrap();
        }
        stamp_lease_child_grant_at(root, "stamped", &id).unwrap();
        let body = std::fs::read_to_string(sessions.join("stamped.toml")).unwrap();
        assert!(
            body.contains("active_pid = 1"),
            "the stamp keeps the lease: {body}"
        );
        revoke_lease_child_grant(&sessions.join("plain.toml"));
        revoke_lease_child_grant(&sessions.join("missing.toml"));
        let valid = |g: &str| {
            matches!(
                crate::gate::seat_authority::grant_check_for(root, g, subject),
                crate::gate::seat_authority::GrantCheck::Valid(_)
            )
        };
        assert!(valid(&id) && valid(&other), "nothing revoked yet");
        revoke_lease_child_grant(&sessions.join("stamped.toml"));
        assert!(!valid(&id), "the stamped child grant is revoked");
        assert!(valid(&other), "only the stamped grant is touched");
    }

    /// TASK-1798: the CI-fix child's grant is stamped under its own key, so it
    /// never overwrites the implementer's, and a lease end or reap revokes
    /// both. Clearing the key leaves the lease intact; a missing lease is an
    /// error, so the CI-fix launch refuses instead of running unstamped.
    // trace:TASK-1952 | trace:TASK-1798 | ai:claude
    #[test]
    fn task_1798_ci_fix_grant_is_stamped_beside_the_implementer_grant() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let _lock = crate::test_env::env_lock();
        let subject = "task-1798-lease-user";
        let implementer = crate::gate::seat_authority::test_support::mint_grant_for(
            root,
            subject,
            "implementer",
            &[],
        );
        let ci_fix = crate::gate::seat_authority::test_support::mint_grant_for(
            root,
            subject,
            "implementer",
            &[],
        );
        let sessions = root.join(".aida").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("l.toml"), "id = \"l\"\nactive_pid = 1\n").unwrap();
        stamp_lease_child_grant_at(root, "l", &implementer).unwrap();
        set_lease_grant_key_at(root, "l", LEASE_CI_FIX_CHILD_GRANT_KEY, Some(&ci_fix)).unwrap();
        let body = std::fs::read_to_string(sessions.join("l.toml")).unwrap();
        assert!(
            body.contains(&implementer) && body.contains(&ci_fix),
            "{body}"
        );
        set_lease_grant_key_at(root, "l", LEASE_CI_FIX_CHILD_GRANT_KEY, None).unwrap();
        let body = std::fs::read_to_string(sessions.join("l.toml")).unwrap();
        assert!(!body.contains(LEASE_CI_FIX_CHILD_GRANT_KEY), "{body}");
        assert!(body.contains(&implementer) && body.contains("active_pid = 1"));
        assert!(
            set_lease_grant_key_at(root, "gone", LEASE_CI_FIX_CHILD_GRANT_KEY, Some(&ci_fix))
                .is_err(),
            "a missing lease cannot be stamped"
        );

        set_lease_grant_key_at(root, "l", LEASE_CI_FIX_CHILD_GRANT_KEY, Some(&ci_fix)).unwrap();
        let valid = |g: &str| {
            matches!(
                crate::gate::seat_authority::grant_check_for(root, g, subject),
                crate::gate::seat_authority::GrantCheck::Valid(_)
            )
        };
        assert!(valid(&implementer) && valid(&ci_fix));
        revoke_lease_child_grant(&sessions.join("l.toml"));
        assert!(!valid(&implementer), "the implementer grant is revoked");
        assert!(!valid(&ci_fix), "the CI-fix grant is revoked");
    }

    /// BUG-2139: the fresh headless child's seat env is ALWAYS explicit — the
    /// grant minted for it (or `""` for a baseline child) and the routed seat
    /// — never the launcher's, even when the launcher exports a grant, a
    /// role and a purpose. Every vendor.
    // trace:BUG-2139 | ai:claude
    #[test]
    fn exec_headless_child_never_inherits_the_launcher_seat_env() {
        let mut env = crate::test_env::EnvVarsGuard::snapshot(&[
            crate::gate::seat_authority::GRANT_ENV,
            "AIDA_SESSION_ROLE",
            "AIDA_SESSION_PURPOSE",
            "AIDA_CONTAINED",
        ]);
        env.unset_key("AIDA_CONTAINED");
        env.set_key(
            crate::gate::seat_authority::GRANT_ENV,
            "11111111-1111-4111-8111-111111111111",
        );
        env.set_key("AIDA_SESSION_ROLE", "advisor");
        env.set_key("AIDA_SESSION_PURPOSE", "advisor relay");
        let child = "22222222-2222-4222-8222-222222222222";
        for vendor in [
            HeadlessVendor::Claude,
            HeadlessVendor::Codex,
            HeadlessVendor::Agy,
        ] {
            for (seat, grant) in [
                ("implementer", child),
                ("reviewer", child),
                ("implementer", ""),
            ] {
                let result = build_exec_vendor_headless_command(
                    None, vendor, "prompt", "sid", false, None, None, seat, grant,
                );
                // trace:TASK-3048 | ai:codex
                if vendor == HeadlessVendor::Agy {
                    let error =
                        result.expect_err("AGY bypass requires the receipt-bearing wrapper");
                    assert!(format!("{error:#}").contains("applied AIDA ContainmentReceipt"));
                    continue;
                }
                let cmd = result.unwrap();
                let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();
                let get = |k: &str| envs.get(std::ffi::OsStr::new(k)).copied();
                assert_eq!(
                    get(crate::gate::seat_authority::GRANT_ENV),
                    Some(Some(std::ffi::OsStr::new(grant))),
                    "{vendor:?} {seat}: the grant is set explicitly"
                );
                assert_eq!(
                    get("AIDA_SESSION_ROLE"),
                    Some(Some(std::ffi::OsStr::new(seat))),
                    "{vendor:?}: the routed seat, not the launcher's"
                );
                let cleared =
                    crate::vendors::profile::VendorProfile::for_vendor(vendor.launch_vendor())
                        .clears_environment();
                if cleared {
                    assert_eq!(get("AIDA_SESSION_PURPOSE").flatten(), None);
                } else {
                    assert_eq!(
                        get("AIDA_SESSION_PURPOSE"),
                        Some(None),
                        "{vendor:?}: inheriting commands must explicitly scrub the launcher's purpose"
                    );
                }
            }
        }
    }

    /// TASK-1804: a headless seat / drain phase child carries its own
    /// vendor's `--version` as AIDA_HARNESS_VERSION, never the launcher's;
    /// a mocked vendor (AIDA_AGENT_CMD) clears it instead of probing the mock.
    // trace:TASK-1955 | ai:claude (TASK-1804)
    #[cfg(unix)]
    #[test]
    fn headless_child_carries_its_own_vendor_harness_version() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let bin = tempfile::tempdir().unwrap();
        crate::test_exec::write_executable(
            &bin.path().join("codex"),
            "#!/bin/sh\n[ \"$1\" = --version ] && echo 'codex-cli 9.9.9'\nexit 0\n",
        );
        let path = format!(
            "{}:{}",
            bin.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let env_of = |cmd: &std::process::Command| {
            cmd.get_envs()
                .find(|(k, _)| *k == "AIDA_HARNESS_VERSION")
                .map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
        };
        let build = || {
            build_exec_vendor_headless_command(
                None,
                HeadlessVendor::Codex,
                "prompt",
                "sid",
                false,
                None,
                None,
                "implementer",
                "",
            )
            .unwrap()
        };
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("PATH", Some(path.as_str())),
                ("AIDA_CONTAINED", None),
                ("AIDA_OS_WRAP", None),
                ("AIDA_AGENT_CMD", None),
                ("AIDA_HARNESS_VERSION", Some("launcher-version")),
            ]);
            assert_eq!(env_of(&build()), Some(Some("codex-cli 9.9.9".into())));
        }
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("PATH", Some(path.as_str())),
            ("AIDA_CONTAINED", None),
            ("AIDA_OS_WRAP", None),
            ("AIDA_AGENT_CMD", Some("/bin/true")),
            ("AIDA_HARNESS_VERSION", Some("launcher-version")),
        ]);
        assert_eq!(
            env_of(&build()),
            Some(None),
            "a mocked vendor is not probed"
        );
    }

    /// TASK-2103: the two spawn paths that do not go through
    /// `build_exec_vendor_headless_command` stamp the child's own vendor
    /// version too. A fake vendor prints the `AIDA_HARNESS_VERSION` it was
    /// launched with into the session log; deleting either call leaves the
    /// launcher's value there.
    // trace:TASK-1955 | ai:claude (TASK-2103)
    #[cfg(unix)]
    #[test]
    fn spawn_and_resume_children_carry_their_own_vendor_harness_version() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        for vendor in ["claude", "codex"] {
            crate::test_exec::write_executable(
                &bin.path().join(vendor),
                crate::test_exec::codex_probe_script(format!(
                    "#!/bin/sh\n[ \"$1\" = --version ] && {{ echo '{vendor} 9.9.9'; exit 0; }}\nprintf 'HV=%s\\n' \"${{AIDA_HARNESS_VERSION-unset}}\"\n"
                )),
            );
        }
        let path = format!(
            "{}:{}",
            bin.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            root.path(),
            "advisor",
            &["implementer"],
            &[
                ("PATH", Some(path.as_str())),
                ("AIDA_CONTAINED", None),
                ("AIDA_OS_WRAP", None),
                ("AIDA_AGENT_CMD", None),
                ("AIDA_HARNESS_VERSION", Some("launcher-version")),
            ],
        );
        // The real child probe resolves the roster from cwd, as a real
        // queue pickup does. This test runs in its exact isolated process.
        std::env::set_current_dir(root.path()).unwrap();
        let tee = crate::headless_tee::TeeOptions {
            enabled: false,
            label: None,
        };
        let spawn_log = root.path().join("spawn.log");
        let (status, _) = spawn_vendor_headless_with_seat_granted(
            None,
            HeadlessVendor::Codex,
            aida_core::agents_config::AgentSeat::Implementer,
            "prompt",
            "sid",
            &spawn_log,
            &tee,
            false,
        )
        .unwrap();
        assert!(status.success());
        assert_eq!(
            std::fs::read_to_string(&spawn_log).unwrap().trim(),
            "HV=codex 9.9.9"
        );
        let resume_log = root.path().join("resume.log");
        let (status, _) = spawn_claude_headless_resume_granted(
            crate::seat_clone::launcher_main().as_deref(),
            "prompt",
            "sid",
            &resume_log,
            root.path(),
            &tee,
            false,
            "implementer",
        )
        .unwrap();
        assert!(status.success());
        assert_eq!(
            std::fs::read_to_string(&resume_log).unwrap().trim(),
            "HV=claude 9.9.9"
        );
    }

    // trace:TASK-3225 | ai:codex
    #[test]
    fn task_3225_forking_session_refuses_required_service_before_native_spawn() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("vendor-ran");
        let vendor = root.path().join("fake-vendor");
        crate::test_exec::write_executable(
            &vendor,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        );
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            root.path(),
            "advisor",
            &["implementer"],
            &[
                ("AIDA_CONTAINED", None),
                ("AIDA_OS_WRAP", None),
                ("AIDA_AGENT_CMD", vendor.to_str()),
            ],
        );
        std::env::set_current_dir(root.path()).unwrap();
        let tee = crate::headless_tee::TeeOptions {
            enabled: false,
            label: None,
        };
        for fork in [None, Some("existing-thread")] {
            let result = spawn_vendor_headless_forking(
                None,
                HeadlessVendor::Codex,
                aida_core::agents_config::AgentSeat::Implementer,
                "prompt",
                "sid",
                &root.path().join("spawn.log"),
                &tee,
                false,
                fork,
                Some(&|cmd| {
                    cmd.env("AIDA_STORE_MUTATION_REQUIRED", "1");
                }),
            );
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("required store service is unavailable"),
                "fresh and forked session launches must refuse before native execution"
            );
            assert!(
                !marker.exists(),
                "vendor must not run on a required-service refusal"
            );
        }
    }

    /// TASK-2066: every seat's child is labelled from its own vendor and
    /// seat; an implementer child never inherits the launcher's lane name.
    // trace:TASK-1955 | ai:claude (TASK-2066, TASK-1672)
    #[test]
    fn headless_child_identity_comes_from_vendor_and_seat_not_the_launcher() {
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_CONTAINED", None),
            ("AIDA_OS_WRAP", None),
            ("AIDA_AGENT_TYPE", Some("claude")),
            ("AIDA_AGENT_NAME", Some("fix-task-1784")),
        ]);
        for (vendor, seat, agent_type) in [
            (HeadlessVendor::Claude, "implementer", "claude"),
            (HeadlessVendor::Codex, "implementer", "codex"),
            (HeadlessVendor::Claude, "reviewer", "claude"),
        ] {
            let cmd = build_exec_vendor_headless_command(
                None, vendor, "prompt", "sid", false, None, None, seat, "",
            )
            .unwrap();
            let env = |key: &str| {
                cmd.get_envs()
                    .find(|(k, _)| *k == key)
                    .and_then(|(_, v)| v)
                    .map(|v| v.to_string_lossy().into_owned())
            };
            assert_eq!(env("AIDA_AGENT_TYPE").as_deref(), Some(agent_type));
            assert_eq!(
                env("AIDA_AGENT_NAME"),
                Some(format!("{agent_type}-{seat}")),
                "{vendor:?} {seat} child must not inherit the lane name"
            );
        }
    }

    /// A headless codex child (queue/dispatch/session spawn, not `agent new`)
    /// never inherits its parent's launch-decided mail wake, so its own
    /// SessionStart hook falls back to the `[agents.codex]` default.
    // trace:TASK-2737 | ai:claude
    #[test]
    fn headless_codex_child_never_inherits_the_parent_mail_wake() {
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_CONTAINED", None),
            ("AIDA_OS_WRAP", None),
            (crate::mail_wake::LAUNCH_WAKE_ENV, Some("off")),
        ]);
        let cmd = build_exec_vendor_headless_command(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Codex,
            "prompt",
            "sid",
            false,
            None,
            None,
            "implementer",
            "",
        )
        .unwrap();
        let wake = cmd
            .get_envs()
            .find(|(k, _)| *k == crate::mail_wake::LAUNCH_WAKE_ENV)
            .map(|(_, v)| v);
        assert_eq!(wake, Some(None), "the parent's wake must be removed");
    }

    // trace:BUG-2006 | ai:antigravity
    #[test]
    fn build_exec_vendor_headless_command_exports_only_for_claude() {
        // Keep this argv/env contract independent of a native seat's host
        // binding; it must not resolve the operator's checkout or store.
        // trace:STORY-2326 | ai:codex
        let root = contained_cwd_seam::hermetic_checkout();
        let _cwd = contained_cwd_seam::pin(root);
        let _main = crate::seat_clone::LauncherMainPin::new(root);
        // TASK-1794: also pin os_wrap off — an ambient or racing
        // AIDA_OS_WRAP=1 routes the contained launch through the bwrap
        // preflight, which fails closed on userns-restricted CI runners.
        // trace:TASK-1794 | ai:claude
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_CONTAINED", None),
            ("AIDA_OS_WRAP", None),
        ]);
        // Claude contained -> exported
        let cmd_claude_contained = build_exec_vendor_headless_command(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Claude,
            "prompt",
            "sid",
            true,
            None,
            None,
            "implementer",
            "",
        )
        .unwrap();
        let envs: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> =
            cmd_claude_contained.get_envs().collect();
        assert!(
            envs.iter()
                .any(|(k, v)| *k == "AIDA_CONTAINED" && *v == Some(std::ffi::OsStr::new("1"))),
            "contained Claude exec headless must export AIDA_CONTAINED=1: {envs:?}"
        );

        // Claude uncontained -> stripped
        let cmd_claude_uncontained = build_exec_vendor_headless_command(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Claude,
            "prompt",
            "sid",
            false,
            None,
            None,
            "implementer",
            "",
        )
        .unwrap();
        let envs: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> =
            cmd_claude_uncontained.get_envs().collect();
        assert!(
            envs.iter()
                .all(|(k, v)| *k != "AIDA_CONTAINED" || v.is_none()),
            "uncontained Claude exec headless must strip AIDA_CONTAINED: {envs:?}"
        );

        // BUG-2144: Codex with contained=true is REFUSED (this used to pin a
        // silent downgrade that stripped AIDA_CONTAINED); uncontained strips it.
        let err = build_exec_vendor_headless_command(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Codex,
            "prompt",
            "sid",
            true,
            None,
            None,
            "implementer",
            "",
        )
        .expect_err("a contained Codex exec headless launch must be refused");
        assert!(format!("{err:#}").contains("refusing to launch"), "{err:#}");
        let cmd_codex = build_exec_vendor_headless_command(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Codex,
            "prompt",
            "sid",
            false,
            None,
            None,
            "implementer",
            "",
        )
        .unwrap();
        let envs: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> =
            cmd_codex.get_envs().collect();
        assert!(
            envs.iter()
                .any(|(k, v)| *k == "AIDA_CONTAINED" && *v == None),
            "uncontained Codex exec headless must strip AIDA_CONTAINED: {envs:?}"
        );

        // BUG-2144: Agy with contained=true is REFUSED (this used to pin a
        // silent downgrade that stripped AIDA_CONTAINED); uncontained strips it.
        let err = build_exec_vendor_headless_command(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Agy,
            "prompt",
            "sid",
            true,
            None,
            None,
            "implementer",
            "",
        )
        .expect_err("a contained Agy exec headless launch must be refused");
        assert!(format!("{err:#}").contains("refusing to launch"), "{err:#}");
        let cmd_agy = build_exec_vendor_headless_command(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Agy,
            "prompt",
            "sid",
            false,
            None,
            None,
            "implementer",
            "",
        )
        .expect_err("uncontained AGY bypass requires a receipt too");
        // trace:TASK-3048 | ai:codex
        assert!(format!("{cmd_agy:#}").contains("applied AIDA ContainmentReceipt"));
    }
    // trace:STORY-1843 | ai:codex
    #[test]
    fn story_1843_pr3_headless_failure_releases_and_spawn_keeps_reservation() {
        let root = tempfile::tempdir().unwrap();
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            root.path(),
            "advisor",
            &["implementer"],
            &[
                ("AIDA_AGENT_CMD", Some("/missing/story-1843-vendor")),
                ("AIDA_OS_WRAP", Some("0")),
                ("AIDA_CONTAINED", None),
            ],
        );
        let _cwd = contained_cwd_seam::pin(root.path());
        let log = root.path().join("failed.log");
        let result = run_vendor_headless_child(
            None,
            None,
            HeadlessVendor::Claude,
            "prompt",
            "sid",
            &log,
            &crate::headless_tee::TeeOptions::default(),
            false,
            None,
            "implementer",
            "",
            &HeadlessChildHooks::default(),
        );
        assert!(result.is_err());
        assert!(crate::agent_handle::list_reservations(root.path()).is_empty());
        // The review-watch wrapper also releases on a failed spawn and
        // disarms immediately after a successful (even nonzero) child.
        for program in ["/missing/story-1843-vendor", "/bin/false"] {
            let mut guard = child_reservation_guard();
            let mut cmd = std::process::Command::new(program);
            apply_child_agent_identity(&mut cmd, HeadlessVendor::Claude, "reviewer");
            let handle = cmd
                .get_envs()
                .find(|(k, _)| *k == "AIDA_HANDLE")
                .and_then(|(_, v)| v)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let result = reviewer_status_with_handle_guard(&mut cmd, &mut guard);
            drop(guard);
            assert_eq!(
                crate::agent_handle::load_reservation(root.path(), &handle).is_some(),
                result.is_ok()
            );
        }
    }

    // Advisor ruling 2026-11-14: a headless child never inherits its
    // launcher's named seat. Every headless child command drops the
    // launcher's `AIDA_ROTATION_SEAT`, so the child's seat comes from its
    // own registry row (none, unless its own launch named one).
    // trace:STORY-2745 | ai:claude
    #[test]
    fn story2745_headless_child_does_not_inherit_the_launcher_seat() {
        let root = tempfile::tempdir().unwrap();
        let _cwd = contained_cwd_seam::pin(root.path());
        let _env = crate::test_env::EnvVarsGuard::apply(&[(
            crate::agent_rotation::SEAT_ENV,
            Some("integration"),
        )]);
        let seat_of = |cmd: &std::process::Command| {
            cmd.get_envs()
                .find(|(k, _)| *k == crate::agent_rotation::SEAT_ENV)
                .map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
        };
        for vendor in [
            HeadlessVendor::Claude,
            HeadlessVendor::Codex,
            HeadlessVendor::Agy,
        ] {
            for resume in [false, true] {
                let mut guard = child_reservation_guard();
                let mut cmd = std::process::Command::new("unused");
                if resume {
                    apply_resumed_child_agent_identity(&mut cmd, vendor, "reviewer", "own-session");
                } else {
                    apply_child_agent_identity(&mut cmd, vendor, "implementer");
                }
                // Explicitly removed: the spawned child sees no seat.
                assert_eq!(seat_of(&cmd), Some(None), "{vendor:?} resume={resume}");
                drop(cmd);
                drop(guard.take());
            }
        }
        // The seat env every headless child gets, and the launcher-session
        // scrub alone (phase drivers, autopilot intake), drop it too.
        let mut cmd = std::process::Command::new("unused");
        apply_headless_child_seat_env(&mut cmd, "implementer", "grant");
        assert_eq!(seat_of(&cmd), Some(None));
        let mut cmd = std::process::Command::new("unused");
        scrub_launcher_session_env(&mut cmd);
        assert_eq!(seat_of(&cmd), Some(None));
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn story_1843_pr3_headless_children_scrub_every_parent_session_key() {
        let root = tempfile::tempdir().unwrap();
        let _cwd = contained_cwd_seam::pin(root.path());
        let pairs: Vec<_> = crate::agent_handle::INHERITED_SESSION_ENV
            .iter()
            .map(|key| (*key, Some("parent-session")))
            .collect();
        let _env = crate::test_env::EnvVarsGuard::apply(&pairs);
        for vendor in [
            HeadlessVendor::Claude,
            HeadlessVendor::Codex,
            HeadlessVendor::Agy,
        ] {
            for resume in [false, true] {
                let mut guard = child_reservation_guard();
                let mut cmd = std::process::Command::new("unused");
                if resume {
                    apply_resumed_child_agent_identity(&mut cmd, vendor, "reviewer", "own-session");
                } else {
                    apply_child_agent_identity(&mut cmd, vendor, "implementer");
                }
                let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();
                for key in crate::agent_handle::INHERITED_SESSION_ENV {
                    let value = envs
                        .get(std::ffi::OsStr::new(key))
                        .expect("explicit scrub or child value");
                    assert_ne!(
                        *value,
                        Some(std::ffi::OsStr::new("parent-session")),
                        "{vendor:?} {key}"
                    );
                    if ![
                        crate::agent_handle::HANDLE_ENV,
                        crate::agent_handle::LAUNCH_TOKEN_ENV,
                    ]
                    .contains(key)
                    {
                        assert_eq!(*value, None, "{vendor:?} {key}");
                    }
                }
                // Retain the guard to cover the command's entire lifetime.
                drop(cmd);
                drop(guard.take());
            }
        }
    }
}
