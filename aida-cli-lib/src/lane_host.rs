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

fn cache_check() -> Check {
    check("build-cache", false, false,
        "lane-job cache reachability unverified; host installation alone is insufficient",
        "install sccache and configure [build] rustc-wrapper in ~/.cargo/config.toml; lane cache transport and its in-sandbox probe are still required")
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

pub(crate) fn preflight(root: &Path, vendor: &str) -> Result<()> {
    let checks = containment_checks(root, vendor);
    if checks.iter().any(|c| c.containment && !c.pass) {
        render(&checks);
        anyhow::bail!("lane host containment checks failed; run `aida doctor --lane-host --fix` on the host before retrying");
    }
    Ok(())
}

fn render(checks: &[Check]) {
    for c in checks {
        println!(
            "{} {}: {}",
            if c.pass { "PASS" } else { "FAIL" },
            c.id,
            c.detail
        );
        if !c.pass {
            println!("  Fix: {}", c.fix);
        }
    }
}

pub(crate) fn inspect(root: &Path) -> Vec<Check> {
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
    let remote = git(
        &store,
        &[
            "ls-remote",
            "--exit-code",
            "origin",
            "refs/heads/aida-store",
        ],
    )
    .and_then(|s| s.split_whitespace().next().map(str::to_string));
    checks.push(check(
        "store-remote",
        matching_store(head.as_deref(), remote.as_deref()),
        false,
        "store HEAD compared with live origin aida-store (bounded read-only probe)",
        "run `aida pull` and resolve any store divergence; verify origin connectivity",
    ));
    // The separate lane cache transport must provide real in-sandbox evidence.
    // Host config or a successful host-side `sccache --show-stats` cannot prove it.
    checks.push(cache_check());
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
    render(&inspect(root));
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
    let checks = inspect(root);
    if json {
        println!("{}", serde_json::to_string_pretty(&checks)?);
    } else {
        render(&checks);
    }
    if checks.iter().any(|c| !c.pass) {
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
        let check = cache_check();
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
}
