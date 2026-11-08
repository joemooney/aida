//! Host readiness is diagnostic evidence, never vendor admission authority.
// trace:STORY-2642 | ai:codex
use aida_core::exec_context::Environment;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, serde::Serialize)]
pub(crate) struct Check {
    id: String,
    pub(crate) pass: bool,
    containment: bool,
    required: bool,
    detail: String,
    fix: String,
}

pub(crate) fn check(
    id: &str,
    pass: bool,
    containment: bool,
    detail: impl Into<String>,
    fix: &str,
) -> Check {
    Check {
        id: id.into(),
        pass,
        containment,
        required: true,
        detail: detail.into(),
        fix: fix.into(),
    }
}

fn matching_revision(built: &str, expected: Option<&str>) -> bool {
    !built.is_empty()
        && built != "unknown"
        && expected.is_some_and(|sha| sha == built || sha.starts_with(built))
}

fn matching_store(head: Option<&str>, remote: Option<&str>) -> bool {
    head.is_some_and(|sha| !sha.is_empty()) && head == remote
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=5",
        );
    let (status, bytes) = aida_core::transport_policy::bounded(
        cmd,
        &[],
        16384,
        Instant::now() + Duration::from_secs(8),
    )
    .ok()?;
    status
        .success()
        .then(|| String::from_utf8_lossy(&bytes).trim().to_owned())
}

fn wrap_check(explicit: Option<bool>) -> Check {
    check(
        "os-wrap",
        explicit != Some(false),
        true,
        if explicit == Some(false) {
            "OS wrapping explicitly disabled"
        } else {
            "OS wrapping enabled or left to the lane default"
        },
        "unset AIDA_OS_WRAP; remove `os_wrap = false` from [contained] to use the lane default",
    )
}

fn bwrap_check(availability: crate::session::BwrapAvailability) -> Check {
    let pass = availability == crate::session::BwrapAvailability::Ok;
    check("bubblewrap", pass, true, format!("{availability:?}"),
        "run `aida doctor --fix-sandbox` for the exact install/sysctl/AppArmor steps; run its sudo commands yourself")
}

fn posture_check(posture: Result<crate::session::ContainmentPosture>) -> Check {
    let (pass, detail) = match posture {
        Ok(p) => (
            p != crate::session::ContainmentPosture::None,
            p.label().to_string(),
        ),
        Err(_) => (
            false,
            "containment policy missing, invalid or unsupported for this vendor".into(),
        ),
    };
    check("agent-containment", pass, true, detail,
        "aida doctor --lane-host --fix; configure trusted [contained.vendors] for non-Claude vendors")
}

// Advisory evidence remains explicit in text/JSON without making readiness
// permanently fail for a capability the doctor cannot yet verify.
pub(crate) fn advisory(mut check: Check) -> Check {
    check.required = false;
    check
}

fn cache_check(home: Option<&Path>, env: &Environment) -> Check {
    use std::os::unix::fs::PermissionsExt;
    let wrapper = home.and_then(|home| {
        let legacy = home.join(".cargo/config");
        let path = if legacy.exists() {
            legacy
        } else {
            home.join(".cargo/config.toml")
        };
        let body = std::fs::read_to_string(path).ok()?;
        let doc = body.parse::<toml::Value>().ok()?;
        let wrapper = doc.get("build")?.get("rustc-wrapper")?.as_str()?;
        if wrapper.is_empty() {
            return None;
        }
        let path = Path::new(wrapper);
        if path.is_absolute() {
            Some(path.to_path_buf())
        } else if path.components().count() == 1 {
            env.get("PATH").and_then(|paths| {
                std::env::split_paths(paths)
                    .map(|dir| dir.join(path))
                    .find(|candidate| {
                        std::fs::metadata(candidate)
                            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    })
            })
        } else {
            None // Relative paths depend on the launch cwd; not host evidence.
        }
    });
    let pass = wrapper.is_some_and(|p| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    });
    check("build-cache", pass, false,
        "user Cargo build.rustc-wrapper configuration and executable presence (not lane reachability)",
        "install sccache and configure [build] rustc-wrapper in ~/.cargo/config.toml; aida doctor --lane-host --fix configures an installed ~/.cargo/bin/sccache")
}

fn cache_reachability_note() -> Check {
    advisory(check(
        "lane-cache-reachability",
        false,
        false,
        "unverified: host wrapper configuration does not prove in-sandbox cache reachability",
        "verify cache transport with a lane job; the doctor has no in-sandbox cache probe yet",
    ))
}

pub(crate) fn containment_checks(root: &Path, vendor: &str) -> Vec<Check> {
    let requested = crate::agent_config::load_agents_contained(root);
    let posture = requested.and_then(|on| crate::session::containment_posture_in(root, vendor, on));
    let mut checks = vec![
        posture_check(posture),
        wrap_check(crate::session::os_wrap_explicit(root)),
        bwrap_check(crate::session::bwrap_availability()),
    ];
    if let Some(check) = crate::vendors::host_diagnostics::workspace_check(vendor) {
        checks.push(check);
    }
    checks
}

pub(crate) fn needs_preflight(
    no_human: bool,
    dry_run: bool,
    no_launch: bool,
    list_sessions: bool,
) -> bool {
    no_human && !dry_run && !no_launch && !list_sessions
}

pub(crate) fn preflight(root: &Path, vendor: &str) -> Result<()> {
    crate::session::aida_contained_env()?;
    preflight_with(crate::session::process_contained(), || {
        containment_checks(root, vendor)
    })
}

fn preflight_with(contained_parent: bool, inspect: impl FnOnce() -> Vec<Check>) -> Result<()> {
    // A contained parent has already crossed host admission. Its filesystem
    // view and nested bwrap probe cannot establish host readiness. Ordinary
    // child admission still enforces monotonic containment and vendor policy.
    if contained_parent {
        return Ok(());
    }
    validate_containment(&inspect())
}

fn validate_containment(checks: &[Check]) -> Result<()> {
    if checks.iter().any(|c| c.containment && !c.pass) {
        render(checks);
        anyhow::bail!("lane host containment checks failed; run `aida doctor --lane-host --fix` on the host before retrying");
    }
    Ok(())
}

fn render(checks: &[Check]) {
    for c in checks {
        println!(
            "{} {}: {}",
            if !c.required {
                "NOTE"
            } else if c.pass {
                "PASS"
            } else {
                "FAIL"
            },
            c.id,
            c.detail
        );
        if !c.pass {
            println!("  Fix: {}", c.fix);
        }
    }
}

fn inspect(root: &Path, live_store: bool) -> Vec<Check> {
    let vendors = crate::init_cmd::read_enabled_agent_selection(root)
        .map(crate::session::enabled_headless_vendors)
        .unwrap_or_else(|| vec![crate::session::resolve_headless_vendor(root)]);
    let env = Environment::capture();
    let home = crate::aida_home_dir();
    let mut checks = Vec::new();
    for vendor in &vendors {
        let mut vendor_checks = containment_checks(root, vendor.as_str());
        vendor_checks.push(crate::vendors::host_diagnostics::auth_check(
            vendor.as_str(),
            home.as_deref(),
            &env,
        ));
        for check in &mut vendor_checks {
            check.id = format!("{}:{}", vendor.as_str(), check.id);
        }
        checks.extend(vendor_checks);
    }
    if vendors.is_empty() {
        checks.push(check(
            "enabled-agents",
            false,
            true,
            "no lane vendors are enabled",
            "enable at least one [agents] profile before launching lanes",
        ));
    }
    let main = git(root, &["rev-parse", "refs/remotes/origin/HEAD"]);
    let built = crate::upgrade_release::build_git_sha();
    checks.push(check("binary", matching_revision(built, main.as_deref()) && !crate::upgrade_release::build_git_dirty(), false,
        "dev binary compared with locally known origin default branch; release freshness is unverified",
        "fetch the project default branch, then run `aida dev build` from current clean main; release installs: `aida upgrade`"));
    let store = root.join(".aida-store");
    let head = git(&store, &["rev-parse", "HEAD"]);
    let remote = live_store
        .then(|| {
            git(
                &store,
                &[
                    "ls-remote",
                    "--exit-code",
                    "origin",
                    "refs/heads/aida-store",
                ],
            )
        })
        .flatten()
        .and_then(|s| s.split_whitespace().next().map(str::to_string));
    if live_store {
        checks.push(check(
            "store-remote",
            matching_store(head.as_deref(), remote.as_deref()),
            false,
            "store HEAD compared with live origin aida-store (bounded read-only probe)",
            "run `aida pull` and resolve any store divergence; verify origin connectivity",
        ));
    }
    // The separate lane cache transport must provide real in-sandbox evidence.
    // Host config or a successful host-side `sccache --show-stats` cannot prove it.
    checks.push(cache_check(home.as_deref(), &env));
    checks.push(cache_reachability_note());
    checks
}

// Only point Cargo at an already-installed user executable. This is not a
// cache transport or an in-lane reachability assertion. Preserve other settings.
fn fix_cargo_cache(home: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let wrapper = home.join(".cargo/bin/sccache");
    if !std::fs::metadata(&wrapper)
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    {
        return Ok(());
    }
    let path = home.join(".cargo/config.toml");
    // Cargo's legacy extensionless file wins; leave that manual configuration alone.
    if home.join(".cargo/config").exists() {
        return Ok(());
    }
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).context("reading user Cargo config"),
    };
    let mut doc = body
        .parse::<toml_edit::DocumentMut>()
        .context("parsing user Cargo config")?;
    if doc
        .get("build")
        .is_some_and(|b| !b.is_table() && !b.is_inline_table())
    {
        anyhow::bail!("user Cargo [build] must be a table; repair it manually");
    }
    if doc
        .get("build")
        .and_then(|b| b.get("rustc-wrapper"))
        .is_some()
    {
        return Ok(());
    }
    let wrapper = wrapper.to_str().context("sccache path is not Unicode")?;
    doc["build"]["rustc-wrapper"] = toml_edit::value(wrapper);
    aida_core::write_config_atomic(&path, doc.to_string())?;
    Ok(())
}

pub(crate) fn report(root: &Path) {
    render(&inspect(root, false));
}

pub(crate) fn run(root: &Path, fix: bool, json: bool) -> Result<()> {
    let home = crate::aida_home_dir().context("cannot resolve user home")?;
    if fix && !crate::vendors::host_diagnostics::user_preset_matches(&home) {
        crate::config_cmd::apply_permission_posture(
            root,
            crate::cli::ConfigPermissionTier::Contained,
            crate::config_cmd::PermissionPostureScope::User,
        )
        .context("applying user permission preset")?;
    }
    if fix {
        fix_cargo_cache(&home)?;
    }
    let checks = inspect(root, true);
    if json {
        println!("{}", serde_json::to_string_pretty(&checks)?);
    } else {
        render(&checks);
    }
    if checks.iter().any(|c| c.required && !c.pass) {
        if json {
            return Err(crate::TypedPayloadEmitted.into());
        }
        anyhow::bail!("lane host is not fully verified; see failed checks above");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendors::host_diagnostics::{auth_check, codex_config_check};
    #[test]
    fn policy_errors_and_uncontained_posture_refuse() {
        use crate::session::ContainmentPosture::*;
        assert!(!posture_check(Ok(None)).pass);
        assert!(!posture_check(Err(anyhow::anyhow!("invalid"))).pass);
        assert!(posture_check(Ok(Aida)).pass);
        let native = posture_check(Ok(Native));
        assert!(native.pass);
        assert!(native.detail.contains("not AIDA-contained"));
    }
    #[test]
    fn cache_never_claims_host_installation_proves_lane_reachability() {
        let check = cache_reachability_note();
        assert!(!check.required);
        assert!(!check.pass);
        assert!(!check.containment);
        assert!(check.detail.contains("unverified"));
    }
    #[test]
    fn explicit_off_refuses_and_unset_uses_lane_default() {
        assert!(!wrap_check(Some(false)).pass);
        assert!(wrap_check(None).pass);
        assert!(wrap_check(Some(true)).pass);
    }
    #[test]
    fn missing_or_blocked_bwrap_refuses() {
        use crate::session::BwrapAvailability::*;
        assert!(!bwrap_check(NotInstalled).pass);
        assert!(
            !bwrap_check(UsernsBlocked {
                hint: "blocked".into()
            })
            .pass
        );
        assert!(bwrap_check(Ok).pass);
    }
    #[test]
    fn codex_requires_explicit_current_preset() {
        let valid = "sandbox_mode='workspace-write'\napproval_policy='never'\n[sandbox_workspace_write]\nnetwork_access=true\nwritable_roots=[]\n";
        assert!(codex_config_check(Some(valid)).pass);
        for invalid in [
            None,
            Some("bad ["),
            Some(""),
            Some("sandbox_mode='danger-full-access'"),
        ] {
            assert!(!codex_config_check(invalid).pass);
        }
        assert!(!codex_config_check(Some(&valid.replace("[]", "['~/.cargo']"))).pass);
        assert!(
            !codex_config_check(Some(
                &valid.replace("network_access=true", "network_access=false")
            ))
            .pass
        );
    }
    #[test]
    fn credential_presence_is_not_assumed() {
        let env = Environment::default();
        for vendor in ["codex", "claude", "agy"] {
            assert!(!auth_check(vendor, None, &env).pass);
        }
        let env = Environment::from_pairs([("OPENAI_API_KEY".into(), "fixture".into())]);
        assert!(auth_check("codex", None, &env).pass);
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".codex")).unwrap();
        std::fs::write(home.path().join(".codex/auth.json"), "fixture").unwrap();
        assert!(auth_check("codex", Some(home.path()), &Environment::default()).pass);
    }
    #[test]
    fn unknown_or_stale_binary_is_not_current() {
        assert!(!matching_revision("unknown", Some("abc123")));
        assert!(!matching_revision("", Some("abc123")));
        assert!(!matching_revision("abc123", None));
        assert!(!matching_revision("abc123", Some("def456")));
        assert!(matching_revision("abc123", Some("abc123456")));
    }
    #[test]
    fn store_needs_both_live_and_local_evidence() {
        assert!(!matching_store(None, None));
        assert!(!matching_store(Some("abc"), None));
        assert!(!matching_store(Some("abc"), Some("def")));
        assert!(matching_store(Some("abc"), Some("abc")));
    }
    #[test]
    fn cargo_fix_needs_installed_wrapper_and_preserves_custom_config() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        fix_cargo_cache(home.path()).unwrap();
        let config = home.path().join(".cargo/config.toml");
        assert!(!config.exists());
        let bin = home.path().join(".cargo/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let wrapper = bin.join("sccache");
        std::fs::write(&wrapper, "fixture").unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&config, "[net]\noffline = true\n").unwrap();
        fix_cargo_cache(home.path()).unwrap();
        let once = std::fs::read_to_string(&config).unwrap();
        let parsed: toml::Value = once.parse().unwrap();
        assert_eq!(parsed["build"]["rustc-wrapper"].as_str(), wrapper.to_str());
        assert_eq!(parsed["net"]["offline"].as_bool(), Some(true));
        fix_cargo_cache(home.path()).unwrap();
        assert_eq!(once, std::fs::read_to_string(&config).unwrap());
        std::fs::write(&config, "[build]\nrustc-wrapper = '/custom/wrapper'\n").unwrap();
        fix_cargo_cache(home.path()).unwrap();
        assert!(std::fs::read_to_string(&config)
            .unwrap()
            .contains("/custom/wrapper"));
    }
    #[test]
    fn lane_host_flag_preserves_store_fix_syntax() {
        use clap::Parser;
        let args =
            crate::doctor_cmd::DoctorArgs::try_parse_from(["doctor", "--lane-host", "--fix"])
                .unwrap();
        assert!(args.lane_host);
        assert_eq!(args.fix.as_deref(), Some("lane-host"));
        let args = crate::doctor_cmd::DoctorArgs::try_parse_from([
            "doctor",
            "--fix",
            "id-collisions",
            "--dry-run",
        ])
        .unwrap();
        assert_eq!(args.fix.as_deref(), Some("id-collisions"));
    }
    #[test]
    fn configured_cache_can_pass_without_claiming_lane_reachability() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let env = Environment::default();
        assert!(!cache_check(Some(home.path()), &env).pass);
        std::fs::create_dir_all(home.path().join(".cargo/bin")).unwrap();
        let wrapper = home.path().join(".cargo/bin/sccache");
        std::fs::write(&wrapper, "fixture").unwrap();
        let config = home.path().join(".cargo/config.toml");
        std::fs::write(
            &config,
            format!("[build]\nrustc-wrapper = {:?}\n", wrapper.to_str().unwrap()),
        )
        .unwrap();
        assert!(!cache_check(Some(home.path()), &env).pass);
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(cache_check(Some(home.path()), &env).pass);
        std::fs::write(
            home.path().join(".cargo/config"),
            "[build]\nrustc-wrapper = '/missing'\n",
        )
        .unwrap();
        assert!(
            !cache_check(Some(home.path()), &env).pass,
            "Cargo's legacy config wins"
        );
        assert!(!cache_reachability_note().required);
    }

    #[test]
    fn unsupported_auth_is_explicitly_advisory() {
        let check = auth_check("agy", None, &Environment::default());
        assert!(!check.pass);
        assert!(!check.required);
        assert!(check.detail.contains("unverifiable"));
    }

    #[test]
    fn containment_refusals_stop_before_following_mutation() {
        use crate::session::{BwrapAvailability, ContainmentPosture};
        for failed in [
            posture_check(Ok(ContainmentPosture::None)),
            wrap_check(Some(false)),
            bwrap_check(BwrapAvailability::NotInstalled),
            codex_config_check(None),
        ] {
            let mut mutated = false;
            let result = (|| -> Result<()> {
                preflight_with(false, || vec![failed])?;
                mutated = true;
                Ok(())
            })();
            assert!(!mutated);
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("aida doctor --lane-host --fix"));
        }
        assert!(
            preflight_with(false, || vec![posture_check(Ok(ContainmentPosture::Aida))]).is_ok()
        );
    }

    #[test]
    fn previews_skip_host_gate_and_contained_parent_skips_host_probes() {
        assert!(needs_preflight(true, false, false, false));
        for (headless, dry, no_launch, list) in [
            (false, false, false, false),
            (true, true, false, false),
            (true, false, true, false),
            (true, false, false, true),
        ] {
            assert!(!needs_preflight(headless, dry, no_launch, list));
        }
        preflight_with(true, || {
            panic!("nested launches must not probe host files or bwrap")
        })
        .unwrap();
    }

    #[test]
    fn lane_host_rejects_ignored_doctor_options() {
        use clap::Parser;
        for flag in [
            "--yes",
            "--force",
            "--all",
            "--heal",
            "--fix-sandbox",
            "--contradictions",
        ] {
            assert!(
                crate::doctor_cmd::DoctorArgs::try_parse_from(["doctor", "--lane-host", flag])
                    .is_err(),
                "{flag}"
            );
        }
        assert!(crate::doctor_cmd::DoctorArgs::try_parse_from([
            "doctor",
            "--lane-host",
            "--category",
            "foo"
        ])
        .is_err());
    }
    #[test]
    fn relocated_codex_home_is_checked_and_fixed_consistently() {
        let home = tempfile::tempdir().unwrap();
        let relocated = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_HOME", home.path().to_str().unwrap()),
            ("CODEX_HOME", relocated.path().to_str().unwrap()),
        ]);
        use crate::vendors::host_diagnostics::{codex_home, user_preset_matches, workspace_check};
        assert!(!workspace_check("codex").unwrap().pass);
        crate::config_cmd::apply_permission_posture(
            project.path(),
            crate::cli::ConfigPermissionTier::Contained,
            crate::config_cmd::PermissionPostureScope::User,
        )
        .unwrap();
        assert!(relocated.path().join("config.toml").is_file());
        assert!(!home.path().join(".codex/config.toml").exists());
        assert!(workspace_check("codex").unwrap().pass);
        assert!(user_preset_matches(home.path()));
        let once = std::fs::read(relocated.path().join("config.toml")).unwrap();
        // The same predicate used by --fix now sees the effective config.
        assert!(user_preset_matches(home.path()));
        assert_eq!(
            once,
            std::fs::read(relocated.path().join("config.toml")).unwrap()
        );
        std::fs::write(relocated.path().join("auth.json"), "fixture").unwrap();
        assert!(auth_check("codex", Some(home.path()), &Environment::capture()).pass);
        assert_eq!(
            codex_home(
                home.path(),
                &Environment::from_pairs([("CODEX_HOME".into(), "".into())])
            ),
            home.path().join(".codex")
        );
    }

    #[test]
    fn real_preflight_refuses_missing_policy_and_nested_parent_defers_to_child_admission() {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_HOME", home.path().to_str().unwrap()),
            ("AIDA_CONTAINED", "0"),
        ]);
        let result = preflight(root.path(), "claude");
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("aida doctor --lane-host --fix"));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
        env.set_key("AIDA_CONTAINED", "1");
        preflight(root.path(), "claude").unwrap();
        env.set_key("AIDA_CONTAINED", "typo");
        assert!(preflight(root.path(), "claude").is_err());
    }
    #[test]
    fn queue_work_no_human_refuses_before_queue_lease_or_worktree_writes() {
        if let Some(output) = crate::test_env::run_path_isolated_test_output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("FAIL agent-containment"), "{stdout}");
            assert!(stdout.contains("FAIL os-wrap"), "{stdout}");
            assert!(stdout.contains("FAIL bubblewrap"), "{stdout}");
            assert!(stdout.contains("FAIL codex-config"), "{stdout}");
            assert!(
                stdout.contains("Fix: aida doctor --lane-host --fix"),
                "{stdout}"
            );
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut ambient = crate::test_env::AmbientGuard::hermetic_and(
            root.path(),
            None,
            &[
                ("AIDA_HOME", Some(home.path().to_str().unwrap())),
                ("HOME", Some(home.path().to_str().unwrap())),
                ("AIDA_CONTAINED", Some("0")),
                ("AIDA_HEADLESS_VENDOR", Some("claude")),
                ("AIDA_OS_WRAP", Some("0")),
            ],
        );
        let output = crate::test_git::git_command()
            .current_dir(root.path())
            .args(["init", "-q"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let storage = aida_core::Storage::new(root.path().join(".aida-store"));
        // Deliberately no queued spec: the host gate must win even over queue
        // lookup and implicit insertion, and it must leave the fixture untouched.
        fn snapshot(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
            fn walk(root: &Path, path: &Path, out: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
                for entry in std::fs::read_dir(path).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        walk(root, &path, out);
                    } else {
                        out.push((
                            path.strip_prefix(root).unwrap().into(),
                            std::fs::read(path).unwrap(),
                        ));
                    }
                }
            }
            let mut out = vec![];
            walk(root, root, &mut out);
            out.sort();
            out
        }
        // Commit only fixture policy so the Codex case passes its independent
        // trusted-default-branch posture check. No real vendor is launched.
        std::fs::create_dir(root.path().join(".aida")).unwrap();
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "[contained.vendors]\ncodex = 'native'\n",
        )
        .unwrap();
        for args in [
            vec!["add", ".aida/config.toml"],
            vec![
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.com",
                "commit",
                "-qm",
                "fixture policy",
            ],
            vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
            vec![
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        ] {
            let output = crate::test_git::git_command()
                .current_dir(root.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let bwrap = bin.join("bwrap");
        std::fs::write(&bwrap, "#!/bin/sh\nexit 0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bwrap, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(home.path().join(".aida")).unwrap();
        let env = Environment::capture();
        let git = std::env::split_paths(env.get("PATH").unwrap())
            .map(|dir| dir.join("git"))
            .find(|path| path.is_file())
            .expect("fixture requires git");
        std::os::unix::fs::symlink(git, bin.join("git")).unwrap();
        ambient.set_key("PATH", &bin);
        ambient.set_key("CODEX_HOME", home.path().join(".codex"));
        let before = snapshot(root.path());
        // Exercise each failure alone, through the actual fresh and drain
        // dispatchers. Positive controls ensure another failed row cannot
        // accidentally make the refusal test pass. trace:STORY-2642 | ai:codex
        for (vendor, contained, wrap, have_bwrap, failed_id) in [
            ("claude", false, "1", true, "agent-containment"),
            ("claude", true, "0", true, "os-wrap"),
            ("claude", true, "1", false, "bubblewrap"),
            ("codex", true, "1", true, "codex-config"),
        ] {
            ambient.set_key("AIDA_HEADLESS_VENDOR", vendor);
            ambient.set_key("AIDA_OS_WRAP", wrap);
            std::fs::write(
                home.path().join(".aida/agents.toml"),
                format!("[agents]\ncontained = {contained}\n"),
            )
            .unwrap();
            let hidden = bin.join("disabled-bwrap");
            if !have_bwrap {
                std::fs::rename(&bwrap, &hidden).unwrap();
            }
            let checks = containment_checks(root.path(), vendor);
            let failures: Vec<_> = checks
                .iter()
                .filter(|c| c.containment && !c.pass)
                .map(|c| c.id.as_str())
                .collect();
            assert_eq!(failures, vec![failed_id], "{vendor}: {checks:?}");
            let home_before = snapshot(home.path());
            for route in [
                vec!["TASK-2642", "--no-human"],
                vec!["--drain", "--no-human"],
            ] {
                let mut argv = vec!["aida", "queue", "work"];
                argv.extend(route);
                let cli = crate::command_registry::try_parse_from(argv).unwrap();
                let args = cli.command.registered::<crate::queue_cmd::QueueArgs>();
                let err = crate::queue_cmd::handle_queue_command(
                    &args.inner,
                    &storage,
                    &root.path().join(".aida-store"),
                )
                .unwrap_err();
                assert!(
                    err.to_string()
                        .contains("lane host containment checks failed"),
                    "{err:#}"
                );
                assert_eq!(before, snapshot(root.path()));
                assert_eq!(home_before, snapshot(home.path()));
                assert!(!root.path().join(".aida-store").exists());
            }
            if !have_bwrap {
                std::fs::rename(&hidden, &bwrap).unwrap();
            }
        }
        for preview in ["--dry-run", "--no-launch", "--list-sessions"] {
            let cli = crate::command_registry::try_parse_from([
                "aida",
                "queue",
                "work",
                "TASK-2642",
                "--no-human",
                preview,
            ])
            .unwrap();
            let args = cli.command.registered::<crate::queue_cmd::QueueArgs>();
            if let Err(err) = crate::queue_cmd::handle_queue_command(
                &args.inner,
                &storage,
                &root.path().join(".aida-store"),
            ) {
                assert!(
                    !err.to_string()
                        .contains("lane host containment checks failed"),
                    "{preview}: {err:#}"
                );
            }
        }
    }
}
