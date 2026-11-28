//! Vendor credential and sandbox configuration diagnostics. No admission authority.
// trace:STORY-2642 | ai:codex
use crate::lane_host::{check, Check};
use aida_core::exec_context::Environment;
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

pub(crate) fn codex_config_check(body: Option<&str>) -> Check {
    let doc = body.and_then(|s| s.parse::<toml::Value>().ok());
    let pass = doc.as_ref().is_some_and(|d| {
        d.get("sandbox_mode").and_then(|v| v.as_str()) == Some("workspace-write")
            && d.get("approval_policy").and_then(|v| v.as_str()) == Some("never")
            && d.get("sandbox_workspace_write").is_some_and(|s| {
                s.get("network_access").and_then(|v| v.as_bool()) == Some(true)
                    && s.get("writable_roots")
                        .and_then(|v| v.as_array())
                        .is_some_and(Vec::is_empty)
            })
    });
    check(
        "codex-config",
        pass,
        true,
        "user Codex workspace-write preset (network on, no extra writable roots)",
        "aida doctor --lane-host --fix; project overrides are checked by ordinary launch admission",
    )
}

pub(crate) fn auth_check(vendor: &str, home: Option<&Path>, env: &Environment) -> Check {
    // Metadata only. Presence does not establish validity, refreshability or admission.
    if !matches!(vendor, "codex" | "claude" | "agy" | "antigravity") {
        return crate::lane_host::advisory(check(
            &format!("auth-{vendor}"),
            false,
            false,
            "unverifiable: this vendor has no supported credential-source probe",
            "log in with the vendor CLI and verify authentication manually",
        ));
    }
    let (key, file, login) = match vendor {
        "codex" => ("OPENAI_API_KEY", ".codex/auth.json", "codex login"),
        "claude" => (
            "ANTHROPIC_API_KEY",
            ".claude/.credentials.json",
            "claude auth login",
        ),
        "agy" | "antigravity" => (
            "",
            ".gemini/antigravity-cli/antigravity-oauth-token",
            "agy (complete OAuth login), then retry aida doctor --lane-host",
        ),
        _ => (
            "",
            "",
            "log in with the vendor CLI and verify its supported auth transport",
        ),
    };
    let (override_key, credential_file) = if vendor == "codex" {
        ("CODEX_HOME", "auth.json")
    } else if matches!(vendor, "agy" | "antigravity") {
        ("", "")
    } else {
        ("CLAUDE_CONFIG_DIR", ".credentials.json")
    };
    let credential = env
        .get(override_key)
        .filter(|p| !p.is_empty())
        .map(|p| std::path::PathBuf::from(p).join(credential_file))
        .or_else(|| home.map(|h| h.join(file)));
    let present = (vendor == "claude"
        && env
            .get("CLAUDE_CODE_OAUTH_TOKEN")
            .is_some_and(|v| !v.is_empty()))
        || (!key.is_empty() && env.get(key).is_some_and(|v| !v.is_empty()))
        || (!file.is_empty()
            && credential
                .is_some_and(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0)));
    check(
        &format!("auth-{vendor}"),
        present,
        false,
        if present {
            "credential source present; validity and refresh are not verified"
        } else {
            "no recognized credential source; auth readiness unverified"
        },
        login,
    )
}

/// Why a non-interactive agy can or cannot authenticate.
// trace:STORY-3202 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AgyAuth {
    Ready,
    /// Signed out for headless use with no file-backed token: the sign-in, if
    /// any, lives only in the desktop keyring, which a lane cannot reach.
    KeyringOnly,
    /// A file-backed token exists but agy still asks for sign-in.
    Rejected,
    Timeout,
    Failed(String),
}

const AGY_TOKEN: &str = ".gemini/antigravity-cli/antigravity-oauth-token";

/// Run `agy models` (needs sign-in, spends no tokens) through the review
/// runner's hardened spawn, the way a lane runs agy: a new session with no
/// TTY, an empty cwd and the runner env allowlist, so no session bus and hence
/// no keyring. `None` when agy is not on `parent_env`'s PATH.
pub(crate) fn agy_auth_probe(
    parent_env: &[(OsString, OsString)],
    home: Option<&Path>,
    deadline: Duration,
) -> Option<AgyAuth> {
    use crate::launch::review_runner::{DeadlineExceeded, RunnerCall, STDERR_CAP};
    use crate::vendors::review_runner::{AuthRequired, AGY_AUTH_PROBE};
    let checkout = tempfile::tempdir().ok()?;
    let call = RunnerCall {
        agent: Some(&AGY_AUTH_PROBE),
        checkout: checkout.path(),
        parent_env,
        pass: "auth-probe",
        deadline,
        stdout_cap: STDERR_CAP,
        stderr_cap: STDERR_CAP,
        tuning: Default::default(),
    };
    call.on_path(&AGY_AUTH_PROBE)?;
    let token = home.is_some_and(|h| {
        std::fs::metadata(h.join(AGY_TOKEN)).is_ok_and(|m| m.is_file() && m.len() > 0)
    });
    Some(match call.run("") {
        Ok(_) => AgyAuth::Ready,
        Err(e) if e.downcast_ref::<AuthRequired>().is_some() && token => AgyAuth::Rejected,
        Err(e) if e.downcast_ref::<AuthRequired>().is_some() => AgyAuth::KeyringOnly,
        Err(e) if e.downcast_ref::<DeadlineExceeded>().is_some() => AgyAuth::Timeout,
        Err(e) => AgyAuth::Failed(format!("{e:#}").chars().take(300).collect()),
    })
}

/// The lane-host check for a probe result; `required` when agy is a lane vendor.
pub(crate) fn agy_auth_probe_check(cause: &AgyAuth, required: bool) -> Check {
    let (detail, fix) = match cause {
        AgyAuth::Ready => ("non-interactive agy is signed in".into(), ""),
        AgyAuth::KeyringOnly => (
            "non-interactive agy is not signed in and has no file-backed token: a sign-in \
             kept only in the desktop keyring (Secret Service on the session bus) is \
             unreachable from a lane, which has no session bus or TTY"
                .into(),
            "sign in once from an ssh session (`ssh -t <host> agy`), which writes \
             ~/.gemini/antigravity-cli/antigravity-oauth-token; lanes run over ssh and \
             read that file (or set GEMINI_API_KEY in the launching environment)",
        ),
        AgyAuth::Rejected => (
            "non-interactive agy asks for sign-in despite its file-backed token".into(),
            "the token is expired or revoked: run agy interactively to sign in again",
        ),
        AgyAuth::Timeout => (
            "non-interactive agy did not answer within the probe deadline".into(),
            "check agy's proxy route and run `agy models` by hand",
        ),
        AgyAuth::Failed(why) => (
            format!("non-interactive agy probe failed: {why}"),
            "run `agy models` by hand to see the cause",
        ),
    };
    let found = check(
        "auth-probe-agy",
        *cause == AgyAuth::Ready,
        false,
        detail,
        fix,
    );
    if required {
        found
    } else {
        crate::lane_host::advisory(found)
    }
}

/// The lane-host agy auth probe when agy is on PATH: required when agy is an
/// enabled lane vendor, a note when it is only installed (a review challenger
/// candidate). The probe calls the vendor over the network, so only
/// `aida doctor --lane-host` (`networked`) runs it; `aida init` never does.
// trace:STORY-3202 | ai:claude
pub(crate) fn agy_lane_probe(
    enabled: &[&str],
    env: &Environment,
    home: Option<&Path>,
    networked: bool,
) -> Option<Check> {
    if !networked {
        return None;
    }
    let cause = agy_auth_probe(&env.to_pairs(), home, Duration::from_secs(30))?;
    Some(agy_auth_probe_check(&cause, enabled.contains(&"agy")))
}

// Shared by credential/config diagnostics and the user-scope preset writer.
// Empty CODEX_HOME follows the ordinary default, matching Codex launch reads.
pub(crate) fn codex_home(home: &Path, env: &Environment) -> std::path::PathBuf {
    env.get("CODEX_HOME")
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
}

pub(crate) fn user_preset_matches(home: &Path) -> bool {
    let agents = std::fs::read_to_string(home.join(".aida/agents.toml"))
        .ok()
        .and_then(|s| s.parse::<toml::Value>().ok());
    let codex =
        std::fs::read_to_string(codex_home(home, &Environment::capture()).join("config.toml")).ok();
    agents
        .as_ref()
        .and_then(|d| d.get("agents"))
        .is_some_and(|a| {
            a.get("contained").and_then(|v| v.as_bool()) == Some(true)
                && a.get("bypass").and_then(|v| v.as_bool()) == Some(false)
                && a.get("codex")
                    .and_then(|v| v.get("default_flags"))
                    .and_then(|v| v.as_array())
                    .is_some_and(|flags| {
                        flags.iter().filter_map(|v| v.as_str()).eq([
                            "--sandbox",
                            "workspace-write",
                            "--ask-for-approval",
                            "never",
                        ]) && flags.len() == 4
                    })
        })
        && codex_config_check(codex.as_deref()).pass
}

pub(crate) fn workspace_check(vendor: &str) -> Option<Check> {
    if vendor != "codex" {
        return None;
    }
    let body = crate::aida_home_dir().and_then(|h| {
        std::fs::read_to_string(codex_home(&h, &Environment::capture()).join("config.toml")).ok()
    });
    Some(codex_config_check(body.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe runs a fake agy without TTY or session bus and types its
    /// answer; the keyring-only cause names the headless fix.
    // trace:STORY-3202 | ai:claude
    #[test]
    fn story3202_agy_auth_probe_classifies_without_hanging() {
        let (bin, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let env = [("PATH".into(), bin.path().as_os_str().to_owned())];
        let probe = |body: &str, home, secs| {
            let src = bin.path().join("src");
            std::fs::write(&src, format!("#!/bin/sh\n{body}\n")).unwrap();
            let cp = std::process::Command::new("/usr/bin/install")
                .arg("-m755")
                .arg(&src)
                .arg(bin.path().join("agy"))
                .status();
            assert!(cp.unwrap().success());
            let started = std::time::Instant::now();
            let cause = agy_auth_probe(&env, home, Duration::from_secs(secs)).unwrap();
            assert!(started.elapsed() < Duration::from_secs(10), "{cause:?}");
            cause
        };
        let keyring = "[ \"$1\" = models ] && [ ! -t 0 ] && [ -z \"$DBUS_SESSION_BUS_ADDRESS\" ] \
            || exit 7\necho 'Error: Please sign in to view available models.' >&2; exit 1";
        let cause = probe(keyring, Some(home.path()), 20);
        assert_eq!(cause, AgyAuth::KeyringOnly);
        let json = serde_json::to_value(agy_auth_probe_check(&cause, true)).unwrap();
        let (fix, detail) = (json["fix"].as_str().unwrap(), json["detail"].to_string());
        assert!(
            fix.contains("ssh -t <host> agy") && fix.contains("lanes run over ssh"),
            "{fix}"
        );
        assert!(
            fix.contains("antigravity-oauth-token") && !fix.contains("DBUS"),
            "{fix}"
        );
        assert!(detail.contains("keyring") && json["required"] == true && json["pass"] == false);
        let token = home.path().join(AGY_TOKEN);
        std::fs::create_dir_all(token.parent().unwrap()).unwrap();
        std::fs::write(&token, "fixture").unwrap();
        assert_eq!(probe(keyring, Some(home.path()), 20), AgyAuth::Rejected);
        assert_eq!(probe("echo gemini-3-pro", None, 20), AgyAuth::Ready);
        assert_eq!(probe("exec /bin/sleep 30", None, 1), AgyAuth::Timeout);
        let odd = probe("echo 'quota exceeded' >&2; exit 1", None, 20);
        assert!(matches!(odd, AgyAuth::Failed(why) if why.contains("quota exceeded")));
        // Timeout comes from the typed deadline, never from the text of stderr.
        let text = probe("echo 'upstream deadline exceeded' >&2; exit 1", None, 20);
        assert!(matches!(text, AgyAuth::Failed(why) if why.contains("deadline")));
        let note = serde_json::to_value(agy_auth_probe_check(&AgyAuth::Ready, false)).unwrap();
        assert!(note["pass"] == true && note["required"] == false);
    }

    /// `aida init` (lane_host::report) passes `networked = false`: a fake agy on
    /// PATH that records each call is never spawned; the doctor path spawns it.
    // trace:STORY-3202 | ai:claude
    #[test]
    fn story3202_init_report_makes_no_agy_call() {
        let bin = tempfile::tempdir().unwrap();
        let mark = bin.path().join("called");
        let src = bin.path().join("src");
        std::fs::write(
            &src,
            format!("#!/bin/sh\n: > '{}'\necho m\n", mark.display()),
        )
        .unwrap();
        let cp = std::process::Command::new("/usr/bin/install")
            .arg("-m755")
            .arg(&src)
            .arg(bin.path().join("agy"))
            .status();
        assert!(cp.unwrap().success());
        let env = Environment::from_pairs([("PATH".into(), bin.path().as_os_str().to_owned())]);
        assert!(agy_lane_probe(&["agy"], &env, None, false).is_none());
        assert!(!mark.exists(), "init's report path spawned agy");
        assert!(agy_lane_probe(&["agy"], &env, None, true).is_some_and(|c| c.pass));
        assert!(mark.exists(), "the doctor path runs the probe");
    }

    #[test]
    fn agy_requires_nonempty_conventional_oauth_source() {
        let home = tempfile::tempdir().unwrap();
        let env = Environment::default();
        assert!(!auth_check("agy", Some(home.path()), &env).pass);
        let path = home
            .path()
            .join(".gemini/antigravity-cli/antigravity-oauth-token");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(!auth_check("antigravity", Some(home.path()), &env).pass);
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "").unwrap();
        assert!(!auth_check("agy", Some(home.path()), &env).pass);
        std::fs::write(path, "fixture-token").unwrap();
        let finding = auth_check("agy", Some(home.path()), &env);
        assert!(finding.pass);
        let json = serde_json::to_value(&finding).unwrap();
        assert_eq!(json["required"], true);
        assert!(json["fix"].as_str().unwrap().contains("OAuth login"));
        assert!(
            auth_check(
                aida_core::vendors::LaunchVendor::Agy.as_str(),
                Some(home.path()),
                &env
            )
            .pass
        );
    }
}
