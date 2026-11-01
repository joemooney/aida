//! Codex-only transport of an already-selected child seat into tool shells.
//! Selectors are public; grant key payloads stay in the existing keyring.
// trace:BUG-2398 | ai:codex
use crate::gate::seat_authority::GRANT_ENV;
use anyhow::{Context, Result};
use std::process::{Command, Stdio};

const TOOL_ENV: &[&str] = &[
    GRANT_ENV,
    "AIDA_SESSION_ROLE",
    "AIDA_ROLE_INSTANCE",
    "AIDA_USER",
    "AIDA_AGENT_NAME",
    "AIDA_AGENT_TYPE",
    "AIDA_SESSION_ID",
    "AIDA_SESSION_SCOPE",
    "AIDA_PROJECT_ROOT",
    "AIDA_AGENT_CONTEXT_FILE",
    "AIDA_CONTAINMENT_POSTURE",
    "AIDA_HEADLESS",
];

/// A transport integrity checksum, never authority evidence. Encode presence
/// separately from empty and use a fixed order and length-delimited values.
/// Only the public metadata whitelist enters this digest (no bearer/key data).
// trace:BUG-2398 | ai:codex
pub(crate) fn context_digest(mut value: impl FnMut(&str) -> Option<std::ffi::OsString>) -> String {
    use sha2::{Digest, Sha256};
    use std::os::unix::ffi::OsStrExt;
    let mut hash = Sha256::new();
    hash.update(b"aida-codex-child-context-v1");
    for key in TOOL_ENV {
        match value(key) {
            None => hash.update([0]),
            Some(value) => {
                hash.update([1]);
                let bytes = value.as_os_str().as_bytes();
                hash.update((bytes.len() as u64).to_be_bytes());
                hash.update(bytes);
            }
        }
    }
    format!("{:x}", hash.finalize())
}

/// Only explicit child assignments qualify. Never consult the parent env.
/// Removed/absent entries stay absent; an explicit empty grant stays empty.
pub(crate) fn codex_tool_env_overrides(child: &Command) -> Result<Vec<String>> {
    let mut args = Vec::new();
    for (key, value) in child.get_envs() {
        let Some(key) = key.to_str().filter(|key| TOOL_ENV.contains(key)) else {
            continue;
        };
        let Some(value) = value else { continue };
        let value = value
            .to_str()
            .with_context(|| format!("Codex child metadata {key} is not UTF-8"))?;
        anyhow::ensure!(
            !value.contains(['\0', '\n', '\r']),
            "Codex child metadata {key} contains NUL or a newline"
        );
        if key == "AIDA_ROLE_INSTANCE" {
            anyhow::ensure!(
                matches!(value, "driver" | "companion"),
                "Codex child role instance must be driver or companion"
            );
        }
        if key == GRANT_ENV && !value.is_empty() {
            anyhow::ensure!(
                uuid::Uuid::parse_str(value).is_ok(),
                "Codex child grant selector is not a UUID"
            );
        }
        args.extend([
            "-c".into(),
            format!(
                "shell_environment_policy.set.{key}={}",
                toml::Value::String(value.into())
            ),
        ]);
    }
    Ok(args)
}

/// Call once, after the child grant, identity and customization are final,
/// immediately before spawn/exec. Other vendors never enter this adapter.
pub(crate) fn prepare_launch(child: &mut Command) -> Result<()> {
    let overrides = codex_tool_env_overrides(child)?;
    let grant = child
        .get_envs()
        .find(|(key, _)| *key == GRANT_ENV)
        .and_then(|(_, value)| value);
    anyhow::ensure!(grant.is_some(), "Codex child has no explicit grant decision; provide its delegated grant or an explicit empty baseline");
    if let Some(seat) = child
        .get_envs()
        .find(|(key, _)| *key == "AIDA_SESSION_ROLE")
        .and_then(|(_, value)| value)
        .and_then(|value| value.to_str())
    {
        anyhow::ensure!(
            crate::gate::seat_authority::is_baseline_seat(seat) || !grant.unwrap().is_empty(),
            "Codex requested seat requires its own nonempty delegated grant"
        );
    }
    if grant.unwrap().is_empty() {
        refuse_baseline_conflicts(child)?;
    }
    preflight(child, &overrides)?;
    let prefix = codex_argv_start(child)?;
    let original: Vec<_> = child.get_args().map(|arg| arg.to_owned()).collect();
    let mut replacement = Command::new(child.get_program());
    if let Some(cwd) = child.get_current_dir() {
        replacement.current_dir(cwd);
    }
    for (key, value) in child.get_envs() {
        match value {
            Some(value) => {
                replacement.env(key, value);
            }
            None => {
                replacement.env_remove(key);
            }
        }
    }
    replacement
        .args(&original[..prefix])
        .args(overrides)
        .args(&original[prefix..]);
    // Managed constructors use default env inheritance and no process hooks.
    // Every managed caller finalizes before configuring stdio or process
    // hooks. Rebuilding preserves the explicit env, cwd and wrapper argv.
    *child = replacement;
    Ok(())
}

/// bwrap's explicit command separator pins the wrapped executable. Never
/// search arbitrary argv (especially prompts) for a token named codex.
fn codex_argv_start(child: &Command) -> Result<usize> {
    if std::path::Path::new(child.get_program()).file_name() != Some(std::ffi::OsStr::new("bwrap"))
    {
        return Ok(0);
    }
    let args: Vec<_> = child.get_args().collect();
    let at = args
        .iter()
        .position(|arg| *arg == "--")
        .context("Codex bwrap launch has no command separator")?;
    anyhow::ensure!(
        at + 1 < args.len(),
        "Codex bwrap launch has no wrapped executable"
    );
    Ok(at + 2)
}

/// Translate only sandbox/config options, never model/prompt flags. Current
/// Codex sandbox uses permission profiles rather than the model CLI's -s.
fn sandbox_args(child: &Command) -> Result<Vec<std::ffi::OsString>> {
    use std::ffi::OsString;
    let all_args: Vec<_> = child.get_args().collect();
    let args = &all_args[codex_argv_start(child)?..];
    let mut out = vec![OsString::from("sandbox")];
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].to_string_lossy();
        match arg.as_ref() {
            // The target tool argv (or positional prompt after --) is not
            // Codex configuration, even if it contains a shell's -c flag.
            "--" => break,
            "-c" | "--config" | "-p" | "--profile" | "-P" | "--permission-profile" | "-C" | "--cd" | "--enable" | "--disable" => {
                anyhow::ensure!(i + 1 < args.len(), "incomplete Codex configuration option");
                out.extend([args[i].to_owned(), args[i + 1].to_owned()]);
                i += 1;
            }
            "-s" | "--sandbox" => {
                anyhow::ensure!(i + 1 < args.len(), "incomplete Codex sandbox option");
                let mode = args[i + 1].to_str();
                if mode == Some("danger-full-access") {
                    out.extend(["-c".into(), "sandbox_mode=\"danger-full-access\"".into()]);
                    i += 2;
                    continue;
                }
                let profile = match mode {
                    Some("workspace-write") => ":workspace",
                    Some("read-only") => ":read-only",
                    _ => anyhow::bail!("unsupported Codex sandbox profile for seat preflight"),
                };
                out.extend(["-P".into(), profile.into()]);
                i += 1;
            }
            "--full-auto" => { out.extend(["-P".into(), ":workspace".into()]); }
            "--dangerously-bypass-approvals-and-sandbox" => {
                out.extend(["-c".into(), "sandbox_mode=\"danger-full-access\"".into()]);
            }
            _ if arg.starts_with("-p") && !arg.starts_with("--") && arg.len() > 2 => {
                out.extend(["--profile".into(), arg[2..].into()]);
            }
            _ if arg.starts_with("-P") && arg.len() > 2 => {
                out.extend(["--permission-profile".into(), arg[2..].into()]);
            }
            _ if arg.starts_with("-C") && arg.len() > 2 => {
                out.extend(["--cd".into(), arg[2..].into()]);
            }
            _ if arg.starts_with("-c") && !arg.starts_with("--") && arg.len() > 2 => {
                out.extend(["-c".into(), arg[2..].into()]);
            }
            _ if arg.starts_with("--config=") => out.extend(["-c".into(), arg[9..].into()]),
            _ if arg.starts_with("--profile=") || arg.starts_with("--permission-profile=") || arg.starts_with("--cd=") => out.push(args[i].to_owned()),
            _ if arg.starts_with("--sandbox=") || (arg.starts_with("-s") && arg.len() > 2) => anyhow::bail!("unsupported Codex sandbox option spelling for seat preflight; use --sandbox <mode>"),
            _ => {}
        }
        i += 1;
    }
    if out.iter().any(|arg| {
        arg == "-P"
            || arg == "--permission-profile"
            || arg.to_string_lossy().starts_with("--permission-profile=")
    }) {
        // Explicit sandbox profiles otherwise omit managed requirements.
        out.push("--include-managed-config".into());
    }
    Ok(out)
}

/// Baseline launches have no grant to validate. Still refuse later policy
/// replacements that could unmask ambient authority or replace their explicit
/// empty selector. Profiles remain below the exact-key CLI prefix precedence.
// trace:BUG-2398 | ai:codex
fn refuse_baseline_conflicts(child: &Command) -> Result<()> {
    let args = sandbox_args(child)?;
    for pair in args
        .windows(2)
        .filter(|pair| pair[0] == "-c" || pair[0] == "--config")
    {
        let Some(raw) = pair[1].to_str() else {
            continue;
        };
        let table: toml::Value = match toml::from_str(raw) {
            Ok(table) => table,
            Err(_) => {
                anyhow::ensure!(
                    !raw.contains("shell_environment_policy"),
                    "Codex baseline context conflict: invalid shell environment override"
                );
                continue;
            }
        };
        let Some(policy) = table.get("shell_environment_policy") else {
            continue;
        };
        let key: String = raw
            .split('=')
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_whitespace() && !matches!(c, '\'' | '"'))
            .collect();
        anyhow::ensure!(
            key != "shell_environment_policy" && key != "shell_environment_policy.set",
            "Codex baseline context conflict: shell environment table replacement"
        );
        let Some(set) = policy.get("set").and_then(toml::Value::as_table) else {
            continue;
        };
        for (key, value) in set
            .iter()
            .filter(|(key, _)| TOOL_ENV.contains(&key.as_str()))
        {
            let intended = child
                .get_envs()
                .find(|(k, _)| *k == key.as_str())
                .and_then(|(_, v)| v);
            anyhow::ensure!(
                value
                    .as_str()
                    .is_some_and(|value| intended.and_then(|v| v.to_str()) == Some(value)),
                "Codex baseline context conflict: protected child metadata override"
            );
        }
    }
    Ok(())
}

fn preflight(child: &Command, overrides: &[String]) -> Result<()> {
    use std::ffi::OsStr;
    let explicit = |key: &str| {
        child
            .get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v)
    };
    let Some(_) = explicit(GRANT_ENV).filter(|v| !v.is_empty()) else {
        return Ok(());
    };
    let seat = explicit("AIDA_SESSION_ROLE").context("Codex granted child has no explicit seat")?;
    let mut probe = Command::new(child.get_program());
    if let Some(cwd) = child.get_current_dir() {
        probe.current_dir(cwd);
    }
    for (k, v) in child.get_envs() {
        match v {
            Some(v) => {
                probe.env(k, v);
            }
            None => {
                probe.env_remove(k);
            }
        }
    }
    let original: Vec<_> = child.get_args().collect();
    probe
        .args(&original[..codex_argv_start(child)?])
        .args(overrides)
        .args(sandbox_args(child)?)
        .args([
            OsStr::new("--"),
            crate::aida_exe_path().as_os_str(),
            OsStr::new("role"),
            OsStr::new("current"),
            OsStr::new("--probe-seat"),
            OsStr::new("--expect"),
            seat,
        ]);
    probe
        .arg("--expect-context-sha256")
        .arg(context_digest(|key| explicit(key).map(|v| v.to_owned())));
    if let Some(instance) = explicit("AIDA_ROLE_INSTANCE") {
        probe.arg("--expect-role-instance").arg(instance);
    }
    let mut log = tempfile::tempfile()?;
    probe
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?);
    let status = bounded_status(&mut probe, std::time::Duration::from_secs(10))
        .context("Codex requested-seat preflight could not run")?;
    use std::io::{Read, Seek, SeekFrom};
    log.seek(SeekFrom::Start(0))?;
    let mut message = String::new();
    log.take(4096).read_to_string(&mut message)?;
    if !status.success() {
        anyhow::bail!(
            "Codex requested-seat preflight refused ({status}): {}",
            safe_probe_failure(&message)
        );
    }
    anyhow::ensure!(
        message
            .lines()
            .any(|line| line == "seat_grant_valid=true expected_seat=true"),
        "Codex requested-seat preflight returned no validated seat evidence"
    );
    Ok(())
}

// Only fixed diagnostics escape the private, unlinked probe capture. In
// particular a config parser may echo user -c values: never relay that output.
// trace:BUG-2398 | ai:codex
fn safe_probe_failure(message: &str) -> &'static str {
    if message.lines().any(|line| {
        line.trim().strip_prefix("Error: ").unwrap_or(line.trim())
            == "seat_grant_valid=true expected_seat=false"
    }) {
        return "ExpectedSeatMismatch";
    }
    for cause in [
        "ChildContextMismatch",
        "RoleInstanceMismatch",
        "Absent",
        "BadHandle",
        "Unreadable",
        "Malformed",
        "Revoked",
        "Expired",
        "SubjectMismatch",
        "RosterDenied",
        "ParentPrincipalMismatch",
        "ParentNotDelegating",
        "Parent",
        "Unbound",
        "PresenterNotBound",
        "ForeignNamespace",
        "KeyringUnavailable",
    ] {
        if message.lines().any(|line| {
            line.trim().strip_prefix("Error: ").unwrap_or(line.trim())
                == format!("seat_grant_valid=false cause={cause}")
        }) {
            return cause;
        }
    }
    for cause in [
        "No permissions to create new namespace",
        "app-server socket directory must be a user-owned directory with mode 0700",
        "app-server socket directory must be user-owned",
        "Operation not permitted",
        "Permission denied",
    ] {
        if message.contains(cause) {
            return cause;
        }
    }
    "sandbox/configuration or seat validation failed (diagnostic values suppressed)"
}

/// Regular-file diagnostics avoid pipe deadlocks. Kill the probe group on
/// timeout, so a stuck sandbox cannot hold a managed launch indefinitely.
fn bounded_status(
    cmd: &mut Command,
    timeout: std::time::Duration,
) -> Result<std::process::ExitStatus> {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    let mut child = cmd.spawn()?;
    let start = std::time::Instant::now();
    loop {
        match crate::notify::poll_exit(&mut child) {
            Ok(crate::notify::ChildPoll::Running) => {}
            Ok(crate::notify::ChildPoll::ExitedUnreaped) => return Ok(child.wait()?),
            Ok(crate::notify::ChildPoll::Reaped(status)) => return Ok(status),
            Err(error) => {
                crate::kill_process_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
        if start.elapsed() >= timeout {
            crate::kill_process_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "Codex requested-seat preflight timed out after {}s",
                timeout.as_secs()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(test)]
#[path = "tests/bug_2398_codex_tool_env_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn tool_env_keys_for_test() -> &'static [&'static str] {
    TOOL_ENV
}
