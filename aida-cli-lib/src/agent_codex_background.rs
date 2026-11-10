//! STORY-2254: `aida agent new codex --bg` — a detached Codex implementer or
//! reviewer under the same lease, grant delegation and containment posture
//! as a foreground launch.
//!
//! Three processes, one attempt:
//!
//! 1. the **dispatcher** (this `aida agent new` invocation) runs every
//!    ordinary preflight, admits the scope, prepares the lease/worktree, has
//!    the gate mint the worker's OWN child grant with a one-use capability,
//!    builds the final worker command (BUG-2398 finalization included) and
//!    starts the supervisor;
//! 2. the **supervisor** (`aida internal codex-bg-supervise`, setsid,
//!    headless) receives the capability once over a private inherited socket,
//!    proves it is the issuer's child, spawns the exec barrier, takes
//!    ownership of the attempt by a durable compare-update, binds the grant
//!    through the gate, stamps lease/authorship/registry strictly, releases
//!    the barrier, corroborates the vendor exec, then owns wait/reap and
//!    terminal cleanup;
//! 3. the **exec barrier** (`aida internal codex-bg-barrier`) is the worker
//!    PID: it blocks on its private channel, and only after release sets its
//!    parent-death signal, rechecks its parent, and `execve`s Codex in place.
//!
//! No step mints a grant outside the gate, binds one outside
//! `gate/seat_authority.rs`, inherits the full parent environment, or puts a
//! token in argv. Receipts are evidence, never authority.
// trace:STORY-2254 | ai:claude

use super::codex_bg_receipt::{
    self as receipt, LaunchReceipt, Owner, Phase, ProcId, ReceiptRead, ScopeKey, TerminalRecord,
};
use super::*;
use std::io::{BufRead, BufReader, Write};

/// The private descriptor the dispatcher→supervisor and supervisor→barrier
/// channels are installed at in the child.
const CONTROL_FD: i32 = 3;
/// Bounded waits: an exceeded bound is UNCERTAINTY, never a guessed outcome.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const ACCEPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
const EXEC_CORROBORATION: std::time::Duration = std::time::Duration::from_secs(10);

/// Ambient (non-AIDA) variables the detached worker keeps. Everything else
/// from the launching shell is dropped: no inherit-all, no parent session,
/// handle, registry, context or grant identity.
const WORKER_AMBIENT_ENV: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "PATH",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "SSH_AUTH_SOCK",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "CODEX_HOME",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "NVM_DIR",
    "NVM_BIN",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "AIDA_HOME",
    "AIDA_BIN",
    "NO_COLOR",
];
/// Prefix families the worker keeps (locale only).
const WORKER_AMBIENT_PREFIXES: &[&str] = &["LC_"];
/// The supervisor and barrier need even less: they hold no model authority.
const HELPER_AMBIENT_ENV: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "LANG",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "AIDA_HOME",
    "AIDA_BIN",
    "NO_COLOR",
];

/// Arguments of a `--bg` Codex launch, as parsed.
pub(crate) struct CodexBgRequest {
    pub role: Option<String>,
    pub spec: Option<String>,
    pub force: bool,
    pub cwd: Option<std::path::PathBuf>,
    pub bypass_sandbox: bool,
    pub context: AgentContextOptions,
    pub noexec: bool,
    pub show_prompt: bool,
    pub verbose: bool,
    pub prompt: AgentPromptOptions,
    pub resume: AgentResumeOptions,
    pub flag_options: AgentDefaultFlagOptions,
    pub name: Option<String>,
    pub description: Option<String>,
    pub no_mail_wake: bool,
    /// Branch for the lane's fresh worktree (`--branch`); `None` derives it.
    // trace:BUG-2390 | ai:claude
    pub branch: Option<String>,
    /// Fork point of a new `--branch` (`--base`).
    pub base: Option<String>,
}

/// Refuse option combinations `--bg` does not support, before ANY
/// allocation, log, probe or preview side effect.
pub(crate) fn validate_request(req: &CodexBgRequest) -> Result<()> {
    if req.flag_options.rotate {
        anyhow::bail!("`--bg` cannot be combined with `--rotate`: a detached Codex lane is one generation; background rotation is not supported");
    }
    if req.resume.direct_target.is_some() {
        anyhow::bail!("`--bg` cannot be combined with `--resume`: a detached Codex lane starts a fresh session");
    }
    if !req.prompt.auto_prompt && req.prompt.explicit_prompt.is_none() {
        anyhow::bail!("`--bg` needs a prompt: a detached `codex exec` lane cannot be given one later (drop `--no-prompt`)");
    }
    if req.bypass_sandbox {
        anyhow::bail!("`--bg` cannot be combined with `--bypass-sandbox`: a detached Codex lane runs only in its trusted native sandbox posture");
    }
    let Some(spec) = req.spec.as_deref() else {
        anyhow::bail!(
            "`--bg` needs a spec: `aida agent new codex --role <implementer|reviewer> --bg <SPEC>`"
        );
    };
    let role = req
        .role
        .as_deref()
        .map(aida_core::team::canonical_role)
        .unwrap_or_else(|| agent_new_scope_role(spec).to_string());
    anyhow::ensure!(
        matches!(role.as_str(), "implementer" | "reviewer"),
        "`--bg` launches only an implementer or a reviewer; `{role}` needs a foreground launch"
    );
    Ok(())
}

/// Bypass, raw sandbox or approval overrides are refused outright, never
/// silently dropped: they would weaken the requested native posture.
pub(crate) fn refuse_posture_overrides(args: &[String]) -> Result<()> {
    // TASK-2712: a write root outside AIDA's own `--add-dir` run (e.g. a
    // passthrough `--add-dir /`) widens the native sandbox.
    // trace:TASK-2712 | ai:claude
    crate::codex_native_sandbox::refuse_foreign_add_dir(args)
        .map_err(|e| anyhow::anyhow!("`--bg` {e:#}"))?;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let value = args.get(i + 1).map(String::as_str).unwrap_or("");
        let refused = match arg {
            "--dangerously-bypass-approvals-and-sandbox" | "--yolo" | "--full-auto" => true,
            "--sandbox" | "-s" => value != "workspace-write",
            "--ask-for-approval" | "-a" => value != "never",
            "-c" | "--config" => {
                let key = value.split('=').next().unwrap_or("").trim();
                key.starts_with("sandbox")
                    || key.starts_with("approval")
                    || key.starts_with("shell_environment_policy")
                    || key.starts_with("permissions")
                    // trace:TASK-3736 | ai:codex
                    || key.starts_with("profile")
            }
            _ => {
                // trace:TASK-3736 | ai:codex
                arg == "--profile"
                    || arg.starts_with("--profile=")
                    || arg.starts_with("-p")
                    || arg.contains("danger-full-access")
                    || arg.starts_with("--sandbox=")
                    || arg.starts_with("--ask-for-approval=")
                    || arg.starts_with("--config=")
                    || (arg.starts_with("-c") && arg.len() > 2)
            }
        };
        if refused {
            anyhow::bail!("`--bg` refuses posture override `{arg}`: a detached Codex lane runs only as `native sandbox, not AIDA-contained` (workspace-write, approvals never)");
        }
        i += 1;
    }
    Ok(())
}

/// The trusted native posture, or a refusal. `--bg` never runs as `none`:
/// it requires `[agents] contained = true` plus the reviewed default-branch
/// `[contained.vendors] codex = "native"` opt-in (TASK-2631).
pub(crate) fn resolve_native_posture(
    config: &AgentLaunchConfig,
    project_root: &std::path::Path,
    use_config_defaults: bool,
) -> Result<session::ContainmentPosture> {
    anyhow::ensure!(
        use_config_defaults,
        "`--bg` cannot use `--no-default-flags`: the detached lane's posture comes from the trusted `[agents] contained` configuration"
    );
    anyhow::ensure!(
        crate::agent_config::load_agents_contained(project_root)?,
        "`--bg` needs a contained posture and `[agents] contained` is off; set `[agents] contained = true` (Codex then runs in its native sandbox, not AIDA-contained). Refusing to launch it uncontained"
    );
    let posture = session::containment_posture_in(project_root, "codex", true)?;
    // trace:TASK-2708 | ai:claude — the detached lane is not wired for AIDA.
    anyhow::ensure!(
        posture != session::ContainmentPosture::Aida,
        "{}",
        crate::codex_tool_env::UNWIRED_AIDA_POSTURE
    );
    anyhow::ensure!(
        posture == session::ContainmentPosture::Native,
        "`--bg` Codex requires the native sandbox posture; resolved {}",
        posture.label()
    );
    refuse_posture_overrides(&config.default_args)?;
    anyhow::ensure!(
        agent_containment_posture(config) == "native",
        "`--bg` Codex launch argv does not carry the native sandbox posture (workspace-write, approvals never); refusing"
    );
    Ok(posture)
}

/// The detached exec argv after the program: posture/config flags, `exec
/// --json`, the model after `exec`, then `--` and the prompt.
pub(crate) fn exec_args(default_args: &[String], prompt_args: &[String]) -> Vec<String> {
    let mut head = Vec::new();
    let mut model = Vec::new();
    let mut i = 0;
    while i < default_args.len() {
        let arg = &default_args[i];
        if arg == CODEX_NO_DAEMON_FLAG {
            i += 1;
            continue;
        }
        if (arg == "--model" || arg == "-m") && i + 1 < default_args.len() {
            model = vec!["--model".to_string(), default_args[i + 1].clone()];
            i += 2;
            continue;
        }
        head.push(arg.clone());
        i += 1;
    }
    let mut args = head;
    args.extend(["exec".to_string(), "--json".to_string()]);
    args.extend(model);
    args.push("--".to_string());
    args.extend(prompt_args.iter().cloned());
    args
}

/// How the worker's exec is corroborated after release.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum ExpectedExec {
    /// A native executable: the worker's `/proc/<pid>/exe` must be it.
    Native { path: std::path::PathBuf },
    /// The npm shim, which `execve`s its native binary at the SAME pid
    /// (`process.execve`): the worker must become an executable named
    /// `codex` that is neither the interpreter nor aida.
    ExecveShim { interpreter: std::path::PathBuf },
}

/// Prove, before anything is allocated, how the worker PID maps to the
/// native Codex process. An executable or shim whose PID-preserving
/// transition cannot be established is refused before model work.
pub(crate) fn resolve_expected_exec(binary: &std::path::Path) -> Result<ExpectedExec> {
    use std::io::Read;
    let path = binary
        .canonicalize()
        .with_context(|| format!("resolving {}", binary.display()))?;
    let mut head = [0u8; 256];
    let read = std::fs::File::open(&path)
        .and_then(|mut f| f.read(&mut head))
        .with_context(|| format!("reading {}", path.display()))?;
    let head = &head[..read];
    if head.starts_with(b"\x7fELF") {
        return Ok(ExpectedExec::Native { path });
    }
    anyhow::ensure!(
        head.starts_with(b"#!"),
        "cannot prove how {} maps to the Codex worker process (not a native executable or a script shim); refusing the detached launch",
        path.display()
    );
    let shebang = String::from_utf8_lossy(head)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    anyhow::ensure!(
        shebang.ends_with("env node") || shebang.ends_with("/node"),
        "the Codex shim {} is not a node script ({shebang}); its worker PID mapping cannot be proven",
        path.display()
    );
    let source =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    anyhow::ensure!(
        source.contains("process.execve("),
        "the Codex shim {} does not replace itself with the native binary (no process.execve); the worker PID mapping cannot be proven",
        path.display()
    );
    let interpreter = if shebang.ends_with("env node") {
        find_executable_on_path("node").context("the Codex shim needs `node` on PATH")?
    } else {
        std::path::PathBuf::from(shebang.trim_start_matches("#!").trim())
    };
    let interpreter = interpreter.canonicalize().unwrap_or(interpreter);
    // The shim takes the execve path only when node exposes it.
    let mut probe = std::process::Command::new(&interpreter);
    probe
        .args([
            "-e",
            "process.exit(process.platform !== 'win32' && typeof process.execve === 'function' ? 0 : 3)",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match crate::command_output_with_timeout_detail(probe, std::time::Duration::from_secs(10)) {
        crate::BoundedCommandOutput::Completed(out) if out.status.success() => {}
        _ => anyhow::bail!(
            "{} has no process.execve, so the Codex shim would spawn its native binary as a separate child; the worker PID mapping cannot be proven. Upgrade node (>= 23.11) or put the native codex binary first on PATH",
            interpreter.display()
        ),
    }
    Ok(ExpectedExec::ExecveShim { interpreter })
}

/// Whether `exe` (a worker's `/proc/<pid>/exe`) is the corroborated native
/// Codex executable `expected` names.
pub(crate) fn exec_matches(expected: &ExpectedExec, exe: &std::path::Path) -> bool {
    match expected {
        ExpectedExec::Native { path } => exe == path,
        ExpectedExec::ExecveShim { interpreter } => {
            exe.file_name().and_then(|n| n.to_str()) == Some("codex")
                && exe != interpreter
                && exe != crate::kernel::resolve_aida_exe().as_path()
        }
    }
}

/// The ambient variables a worker or helper keeps from `vars`.
pub(crate) fn ambient_env(
    vars: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    names: &[&str],
    prefixes: &[&str],
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    vars.into_iter()
        .filter(|(key, _)| {
            key.to_str().is_some_and(|key| {
                names.contains(&key) || prefixes.iter().any(|p| key.starts_with(p))
            })
        })
        .collect()
}

// trace:BUG-2510 | ai:codex
fn worker_ambient_env(
    vars: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    configured: &[String],
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let mut names = WORKER_AMBIENT_ENV.to_vec();
    names.extend(configured.iter().map(String::as_str));
    ambient_env(vars, &names, WORKER_AMBIENT_PREFIXES)
}

#[cfg(test)]
mod env_passthrough_tests {
    // trace:BUG-2510 | ai:codex
    #[test]
    fn env_passthrough_worker_projection() {
        let vars = [
            "XAIAA_API_KEY",
            "UNCONFIGURED_KEY",
            "OPENAI_API_KEY",
            "LC_TIME",
            "AIDA_ROLE",
        ]
        .map(|key| (key.into(), "secret".into()));
        let kept = super::worker_ambient_env(vars, &["XAIAA_API_KEY".into()]);
        let names: Vec<_> = kept.iter().map(|(key, _)| key.to_str().unwrap()).collect();
        assert_eq!(names, ["XAIAA_API_KEY", "OPENAI_API_KEY", "LC_TIME"]);
    }
}

/// The exact program, argv, environment and cwd the barrier `execve`s.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorkerExec {
    pub program: std::path::PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: std::path::PathBuf,
}

/// Everything the supervisor needs, sent once over its private channel.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct SupervisorInit {
    pub attempt: String,
    pub project_root: std::path::PathBuf,
    /// Taken exactly once by the supervisor.
    pub capability: Option<crate::gate::seat_authority::DetachedLaunchTransfer>,
    pub worker: WorkerExec,
    pub expected: ExpectedExec,
    pub agent_name: String,
    pub role: String,
    pub spec: String,
    pub lease_id: String,
    pub grant_id: String,
    /// Explicit launcher narrowing; empty means inherit on a later resume.
    #[serde(default)]
    pub delegate_seats: Vec<String>,
    pub description: Option<String>,
    pub mail_wake: String,
    pub context_path: Option<std::path::PathBuf>,
}

/// Control messages (newline-delimited JSON on the private channels).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "msg", rename_all = "kebab-case")]
pub(crate) enum Control {
    /// supervisor → dispatcher: the gated worker exists.
    Spawned { supervisor: ProcId, worker: ProcId },
    /// dispatcher → supervisor: ownership moved durably.
    Transferred,
    /// supervisor → dispatcher: vendor exec corroborated.
    Accepted { worker: ProcId, exe: String },
    /// Either direction: refused/failed. `executed` = model work may have run.
    Failed { reason: String, executed: bool },
    /// supervisor → barrier: the exec plan.
    Exec { worker: WorkerExec, parent: u32 },
    /// supervisor → barrier: go.
    Release,
}

fn send(stream: &mut std::os::unix::net::UnixStream, message: &Control) -> Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.flush()?;
    Ok(())
}

/// `Ok(None)` on EOF; an error on a timeout or a malformed line.
fn recv(reader: &mut BufReader<std::os::unix::net::UnixStream>) -> Result<Option<Control>> {
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(
        serde_json::from_str(line.trim_end()).context("malformed control message")?,
    ))
}

/// Install `fd` at [`CONTROL_FD`] in the child (dup2 clears close-on-exec
/// on the new descriptor; every other descriptor keeps it).
#[cfg(unix)]
fn install_control_fd(command: &mut std::process::Command, fd: i32, new_session: bool) {
    use std::os::unix::process::CommandExt;
    // SAFETY: only async-signal-safe calls (setsid, dup2) run in the child
    // between fork and exec; no allocation, no locks.
    unsafe {
        command.pre_exec(move || {
            if new_session && libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if fd != CONTROL_FD && libc::dup2(fd, CONTROL_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if fd == CONTROL_FD {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            }
            Ok(())
        });
    }
}

fn private_log(
    dir: &std::path::Path,
    attempt: &str,
) -> Result<(std::path::PathBuf, std::fs::File)> {
    use std::os::unix::fs::OpenOptionsExt;
    let logs = dir.join("logs");
    if std::fs::symlink_metadata(&logs).is_err() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&logs)?;
    }
    let path = logs.join(format!("{attempt}.log"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    Ok((path, file))
}

/// Dispatcher-side rollback for what this invocation alone owns: the
/// admission reservation and the newly issued grant. Each acts only while
/// the durable owner record still names this dispatcher in `Prepared`.
struct DispatcherGuard {
    dir: std::path::PathBuf,
    project_root: std::path::PathBuf,
    attempt: String,
    me: ProcId,
    grant_id: Option<String>,
    armed: bool,
}

impl DispatcherGuard {
    fn still_owner(&self) -> bool {
        receipt::dispatcher_still_owns(&self.dir, &self.project_root, &self.attempt, &self.me)
    }
}

impl Drop for DispatcherGuard {
    fn drop(&mut self) {
        if !self.armed || !self.still_owner() {
            return;
        }
        if let Some(id) = &self.grant_id {
            if let Err(error) = crate::gate::seat_authority::revoke_by_id(id) {
                eprintln!("could not revoke the refused detached launch grant: {error:#}");
            }
        }
        let me = self.me.clone();
        let _ = receipt::compare_update(
            &self.dir,
            &self.project_root,
            &self.attempt,
            |r| {
                anyhow::ensure!(
                    r.owner == Owner::Dispatcher(me.clone()) && r.phase == Phase::Prepared,
                    "not owned"
                );
                Ok(())
            },
            |r| {
                r.phase = Phase::RefusedBeforeExecution;
                r.owner = Owner::Released;
                r.reason
                    .get_or_insert_with(|| "refused before the worker was started".into());
            },
        );
    }
}

/// `aida agent new codex --bg`.
pub(crate) fn agent_new_codex_bg(mut req: CodexBgRequest) -> Result<()> {
    validate_request(&req)?;
    // A contained parent refuses codex outright (inherited containment is
    // sticky; native is never relabelled as AIDA containment).
    refuse_uncontained_child_of_contained_parent(&AgentLaunchConfig {
        delegate_seats: Vec::new(),
        agent_type: "codex",
        binary: "codex",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    })?;
    let cwd = req.cwd.clone();
    let base = cwd.clone().map(Ok).unwrap_or_else(std::env::current_dir)?;
    let project_root = agent_launch_project_root_from(&find_aida_project_root_from(&base)?);
    enforce_agent_type_enabled(&project_root, "codex")?;
    let binary = find_executable_on_path("codex").ok_or_else(|| {
        anyhow::anyhow!(
            "`codex` is not on PATH — install Codex, then retry `aida agent new codex --bg`"
        )
    })?;
    let mut config = AgentLaunchConfig {
        delegate_seats: Vec::new(),
        agent_type: "codex",
        binary: "codex",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    let use_config_defaults = req.flag_options.use_config_defaults;
    apply_agent_default_flags(&mut config, &project_root, req.flag_options.clone(), false)?;
    refuse_uncontained_child_of_contained_parent(&config)?;
    let posture = resolve_native_posture(&config, &project_root, use_config_defaults)?;
    let role = req.role.clone();
    let spec = req.spec.clone();
    let name = req.name.clone();
    let cwd_ref = cwd.as_deref();

    // Pure previews: no session, lease, status change, grant, log or probe.
    if req.context.show {
        return print_dry_launch_context_at(&project_root, role, spec, &config, name, cwd_ref);
    }
    if req.show_prompt || req.noexec {
        let plan = prepare_agent_launch_dry_at(&project_root, role, spec, "codex", name, cwd_ref)?;
        let prompt_args = agent_initial_prompt_args(&config, &plan, &req.prompt);
        if req.show_prompt {
            return print_agent_show_prompt(&req.prompt, &prompt_args);
        }
        // trace:STORY-1486 | ai:codex
        crate::launch_permission_profile::print_git_policy(
            &project_root,
            plan.role.as_deref(),
            req.flag_options.declared_git_read_only,
            true,
            req.verbose,
            false,
        );
        let mut preview = config.clone();
        preview.default_args = exec_args(&config.default_args, &[]);
        return print_agent_launch_noexec(
            &binary,
            &preview,
            &plan,
            &req.prompt,
            &prompt_args,
            false,
            req.context.enabled,
        );
    }

    let existing_owner =
        preflight_agent_launch_lease(&project_root, role.as_deref(), spec.as_deref(), cwd_ref)?;
    let spec_id = spec.clone().expect("validated");
    let expected_role = role
        .clone()
        .or_else(|| existing_owner.as_ref().and_then(|lease| lease.role.clone()))
        .unwrap_or_else(|| agent_new_scope_role(&spec_id).to_string());
    // trace:STORY-1486 | ai:codex
    crate::launch_permission_profile::print_git_policy(
        &project_root,
        Some(&expected_role),
        req.flag_options.declared_git_read_only,
        false,
        req.verbose,
        false,
    );
    preflight_agent_new_delegation(&project_root, Some(&expected_role))?;
    crate::gate::seat_authority::validate_launch_delegable_seats(
        &project_root,
        Some(&expected_role),
        &config.delegate_seats,
    )?;
    let early_instance = resolve_role_instance_for_launch(
        &project_root,
        Some(&expected_role),
        Some(&spec_id),
        cwd_ref.unwrap_or(&project_root),
    );
    anyhow::ensure!(
        early_instance == RoleInstanceKind::Driver,
        "`--bg` launches only a driver: a `{expected_role}` driver is already live on this scope, and a detached companion is not supported"
    );
    let spec = scope_preflight_before_consent(&project_root, role.as_deref(), spec, cwd_ref)?;
    let spec_id = spec.clone().expect("scoped");
    let _resolved_aida = aida_bin::process()?;
    gate_agent_bypass(
        &project_root,
        "codex",
        &mut config.default_args,
        true,
        false,
    )?;
    refuse_posture_overrides(&config.default_args)?;
    epic_agent_new_guard(&project_root, &spec_id)?;
    drain_mode_agent_new_guard(&project_root, &spec_id, req.force, false)?;
    focus_scope_guard_for_spec(&project_root, &spec_id, req.force)?;
    // Duplicate safety is not optional here: --force, --allow-duplicate and
    // --no-duplicate-check never bypass the detached ownership gate.
    enforce_agent_singleton_preflight(
        &project_root,
        Some(&expected_role),
        Some(&spec_id),
        cwd_ref.unwrap_or(&project_root),
    )?;
    let expected = resolve_expected_exec(&binary)?;

    // Admission: reserve the scope before any allocation.
    let dir = receipt::runtime_dir(&project_root)?;
    let me = ProcId::current().context("this launcher's start identity could not be read")?;
    let attempt = uuid::Uuid::new_v4().to_string();
    let scope = ScopeKey::new(&expected_role, &spec_id);
    let log_path = dir.join("logs").join(format!("{attempt}.log"));
    receipt::admit(
        &dir,
        &LaunchReceipt::new(
            &project_root,
            &attempt,
            scope.clone(),
            me.clone(),
            "",
            &project_root,
            &log_path,
        ),
        // Inside the same lock hold: no other route's live lease holds it.
        || refuse_live_scope_lease(&project_root, &scope),
    )?;
    let mut guard = DispatcherGuard {
        dir: dir.clone(),
        project_root: project_root.clone(),
        attempt: attempt.clone(),
        me: me.clone(),
        grant_id: None,
        armed: true,
    };
    let (_, log) = private_log(&dir, &attempt)?;

    req.resume.duplicate_check = true;
    let mut handle_guard = crate::agent_handle::LaunchReservationGuard::new(&project_root);
    let (plan, setup_receipt) = {
        // This invocation's own lease write is not refused by its own,
        // already admitted, attempt.
        let _own = receipt::OwnAttemptScope::enter(&attempt);
        // trace:BUG-2390 | ai:claude — dispatch take/impl name the lane branch.
        // TASK-2712: a native Codex lane runs in a seat clone (`true`), whose
        // pending work the host commits and publishes.
        // trace:TASK-2712 | ai:claude
        prepare_agent_launch_attempt_on_branch(
            &project_root,
            role,
            spec,
            "codex",
            name,
            cwd_ref,
            true,
            LaneBranch {
                branch: req.branch.clone(),
                base: req.base.clone(),
            },
        )?
    };
    // TASK-2712: the shared write set, scoped to the lane's own tree.
    // trace:TASK-2712 | ai:claude
    crate::agent_launch::rescope_contained_settings(
        &mut config,
        &plan.launch_cwd,
        plan.role.as_deref(),
    )?;
    refuse_posture_overrides(&config.default_args)?;
    let mut setup_receipt = setup_receipt;
    if let Some(setup) = setup_receipt.as_mut() {
        let (dir, root, attempt, me) = (
            dir.clone(),
            project_root.clone(),
            attempt.clone(),
            me.clone(),
        );
        setup.require_owner(move || receipt::dispatcher_still_owns(&dir, &root, &attempt, &me));
    }
    anyhow::ensure!(
        plan.role_instance == RoleInstanceKind::Driver,
        "`--bg` launches only a driver; this scope already has a live driver"
    );
    enforce_agent_singleton(&project_root, &plan, true)?;
    assert_no_worktree_tangle(&plan, &project_root)?;
    let lease_id = plan
        .lease_id
        .clone()
        .context("a detached launch needs its own lease; none was created")?;
    let plan_role = plan.role.clone().unwrap_or(expected_role.clone());
    anyhow::ensure!(
        ScopeKey::new(&plan_role, &spec_id) == scope,
        "the prepared lease's role `{plan_role}` differs from the admitted `{}`",
        scope.role
    );
    let description = resolve_agent_description(plan.role.as_deref(), req.description.take())?;
    let prompt_args = agent_initial_prompt_args(&config, &plan, &req.prompt);
    let launch_context = prepare_agent_launch_context(&config, &plan, req.context)?;

    eprintln!(
        "{} launching detached codex (stable name: {}) in {}",
        crate::glyph(crate::glyphs::Glyph::FlowActive)
            .green()
            .bold(),
        plan.name.cyan(),
        plan.launch_cwd.display().to_string().cyan()
    );
    eprintln!("  posture: {}", posture.label());
    eprintln!(
        "  {}: {}  {}: {}",
        "spec".bold(),
        spec_id.cyan(),
        "role".bold(),
        plan_role.cyan()
    );

    // The worker's OWN child grant, with its one-use capability.
    let seat = canonical_role_name(&plan_role);
    let (grant, capability) = crate::gate::seat_authority::issue_detached_launch_child(
        &project_root,
        &seat,
        &plan.name,
        true,
        &attempt,
        &format!("{}:{}", scope.role, scope.spec),
        &config.delegate_seats,
    )?;
    guard.grant_id = Some(grant.id.clone());
    session::stamp_lease_child_grant_at(&project_root, &lease_id, &grant.id)?;

    let mail_wake = if req.no_mail_wake {
        agent_registry::MailWake::Off
    } else {
        agent_registry::MailWake::Unsupported
    };
    let worker = build_worker_exec(
        &binary,
        &config,
        &plan,
        &prompt_args,
        launch_context.as_ref(),
        &plan_role,
        &grant.id,
        mail_wake,
    )?;
    let context_path = launch_context.as_ref().map(|c| c.path.clone());
    {
        let me = me.clone();
        let (name, worktree, lease, gid, log_path, ctx) = (
            plan.name.clone(),
            plan.launch_cwd.clone(),
            lease_id.clone(),
            grant.id.clone(),
            log_path.clone(),
            context_path.clone(),
        );
        receipt::compare_update(
            &dir,
            &project_root,
            &attempt,
            |r| {
                anyhow::ensure!(
                    r.owner == Owner::Dispatcher(me) && r.phase == Phase::Prepared,
                    "attempt is no longer this launcher's"
                );
                Ok(())
            },
            |r| {
                r.agent_name = name;
                r.worktree = worktree;
                r.lease_id = Some(lease);
                r.grant_id = Some(gid);
                r.log_path = log_path;
                r.context_path = ctx;
            },
        )?;
    }

    // Start the supervisor: setsid, headless, null stdin, private log, its
    // own minimal environment, the control socket at fd 3.
    let (mut ours, theirs) = std::os::unix::net::UnixStream::pair()?;
    let mut supervisor = spawn_helper(
        &crate::aida_exe_path(),
        &["internal", "codex-bg-supervise", "--attempt", &attempt],
        &project_root,
        &project_root,
        &log,
        &theirs,
        true,
    )?;
    drop(theirs);
    let abort = |supervisor: &mut std::process::Child, why: String| -> anyhow::Error {
        abort_supervisor(supervisor, None, why)
    };
    let init = SupervisorInit {
        attempt: attempt.clone(),
        project_root: project_root.clone(),
        capability: Some(capability.into_transfer()),
        worker,
        expected,
        agent_name: plan.name.clone(),
        role: plan_role.clone(),
        spec: spec_id.clone(),
        lease_id: lease_id.clone(),
        grant_id: grant.id.clone(),
        delegate_seats: config.delegate_seats.clone(),
        description,
        mail_wake: mail_wake.as_str().to_string(),
        context_path: context_path.clone(),
    };
    let mut line = serde_json::to_vec(&init)?;
    line.push(b'\n');
    if let Err(error) = ours.write_all(&line) {
        return Err(abort(
            &mut supervisor,
            format!("could not hand the launch to its supervisor: {error}"),
        ));
    }
    drop(init);
    ours.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let mut reader = BufReader::new(ours.try_clone()?);
    let (sup, worker) = match recv(&mut reader) {
        Ok(Some(Control::Spawned { supervisor: s, worker })) => (s, worker),
        Ok(Some(Control::Failed { reason, .. })) => {
            return Err(abort(&mut supervisor, format!("detached launch refused before the worker started: {reason}")))
        }
        other => {
            return Err(abort(
                &mut supervisor,
                format!("detached launch supervisor did not report its gated worker ({other:?}); nothing ran"),
            ))
        }
    };
    // trace:STORY-2254 | ai:claude — a mismatch is an abort, never a bare
    // return that would leave the supervisor and its gated worker running.
    if sup.pid != supervisor.id()
        || aida_core::liveness::process_start_identity(sup.pid).as_deref()
            != Some(sup.start.as_str())
    {
        return Err(abort_supervisor(
            &mut supervisor,
            Some(&worker),
            "detached launch supervisor reported a foreign identity; it and its gated worker were stopped unrun".into(),
        ));
    }

    // Ownership transfer: durable first, then local disarm, then ack.
    let transfer = {
        let (me, sup, worker) = (me.clone(), sup.clone(), worker.clone());
        receipt::compare_update(
            &dir,
            &project_root,
            &attempt,
            |r| {
                anyhow::ensure!(
                    r.owner == Owner::Dispatcher(me) && r.phase == Phase::Prepared,
                    "attempt is no longer this launcher's"
                );
                Ok(())
            },
            |r| {
                r.owner = Owner::Supervisor(sup.clone());
                r.phase = Phase::SupervisorOwned;
                r.supervisor = Some(sup);
                r.worker = Some(worker);
            },
        )
    };
    if let Err(error) = transfer {
        return Err(abort_supervisor(
            &mut supervisor,
            Some(&worker),
            format!("could not transfer the launch to its supervisor: {error:#}"),
        ));
    }
    guard.armed = false;
    handle_guard.disarm();
    if let Some(setup) = setup_receipt.as_mut() {
        setup.disarm();
    }
    // A lost ack is recovered by the supervisor from the owner record.
    let _ = send(&mut ours, &Control::Transferred);

    ours.set_read_timeout(Some(ACCEPT_TIMEOUT))?;
    let outcome = recv(&mut reader);
    // The supervisor is detached (its own session); this launcher leaves it
    // running (dropping a Child neither kills nor waits).
    drop(supervisor);
    let status_hint = format!("aida agent launch-status {attempt}");
    match outcome {
        Ok(Some(Control::Accepted { worker, exe })) => {
            println!("accepted: detached Codex {} (attempt {attempt})", plan_role);
            println!("  worker:  pid {} ({exe})", worker.pid);
            println!("  lease:   {lease_id}  spec: {spec_id}");
            println!("  posture: {}", posture.label());
            println!("  log:     {}", log_path.display());
            println!("  status:  {status_hint}");
            println!(
                "  mail wake: {}",
                if req.no_mail_wake {
                    "off"
                } else {
                    "unsupported for a detached exec lane (mail is read on its next turn)"
                }
            );
            println!("  accepted means the worker started; it does not mean the work completed");
            Ok(())
        }
        Ok(Some(Control::Failed { reason, executed })) => {
            if executed {
                anyhow::bail!("detached launch failed after the worker may have run: {reason}. Its worktree, branch and log are kept; inspect `{status_hint}`")
            }
            anyhow::bail!(
                "detached launch refused before model work: {reason}. Inspect `{status_hint}`"
            )
        }
        other => {
            // Lost or late report: the durable record, not EOF, decides.
            let phase = match receipt::read_receipt(&dir, &project_root, &attempt) {
                ReceiptRead::Known(r) => Some(r.phase),
                ReceiptRead::Unknown(_) => None,
            };
            if phase == Some(Phase::AcceptedRunning) {
                println!("accepted: detached Codex {plan_role} (attempt {attempt}); the supervisor's report was lost");
                println!("  status:  {status_hint}");
                return Ok(());
            }
            anyhow::bail!(
                "detached launch state is UNKNOWN ({}; receipt phase {}); it was not retried and its resources are kept. Inspect `{status_hint}` and, if needed, `aida agent recover-launch {attempt}`",
                match other {
                    Err(e) => format!("{e:#}"),
                    Ok(m) => format!("{m:?}"),
                },
                phase.map(Phase::as_str).unwrap_or("unknown")
            )
        }
    }
}

/// Refuse admission while another route's live (or unprovably dead) lease
/// holds `scope`. Runs inside the admission lock; routes that write leases
/// take the same lock ([`receipt::with_scope_entry`]).
// trace:STORY-2254 | ai:claude
pub(crate) fn refuse_live_scope_lease(
    project_root: &std::path::Path,
    scope: &ScopeKey,
) -> Result<()> {
    let main = main_worktree_root_from(project_root);
    for lease in list_leases(&main) {
        let role = lease
            .role
            .clone()
            .unwrap_or_else(|| agent_new_scope_role(&lease.scope).to_string());
        if ScopeKey::new(&role, &lease.scope) != *scope {
            continue;
        }
        match crate::stale_lease_recovery_for_lease(&lease).base_verdict {
            aida_core::liveness::StaleLeaseRecovery::Live => anyhow::bail!(
                "refusing a detached driver: live session {} already holds {} {}",
                &lease.id[..lease.id.len().min(8)],
                scope.role,
                scope.spec
            ),
            aida_core::liveness::StaleLeaseRecovery::UnknownLiveness => anyhow::bail!(
                "refusing a detached driver: session {} holds {} {} and its owner cannot be proven gone",
                &lease.id[..lease.id.len().min(8)],
                scope.role,
                scope.spec
            ),
            _ => {}
        }
    }
    Ok(())
}

/// Dispatcher-side abort before ownership moved: stop the gated worker (only
/// when it is provably the supervisor's own child, in its own group, with the
/// reported start identity), then kill and reap the supervisor, then wait a
/// bounded time for the orphaned barrier to be reaped by its new parent. The
/// worker never executed: the barrier releases only after the transfer this
/// invocation still owns.
// trace:STORY-2254 | ai:claude
pub(crate) fn abort_supervisor(
    supervisor: &mut std::process::Child,
    worker: Option<&ProcId>,
    why: String,
) -> anyhow::Error {
    let owned_worker = worker.filter(|w| {
        w.is_live()
            && agent_registry::parent_pid_of(w.pid) == Some(supervisor.id())
            // SAFETY: getpgid only reads another process's group id.
            && unsafe { libc::getpgid(w.pid as libc::pid_t) } == w.pid as libc::pid_t
    });
    if let Some(w) = owned_worker {
        // SAFETY: plain signal delivery to the verified barrier's own group,
        // whose leader is still the supervisor's unreaped child.
        unsafe {
            libc::killpg(w.pid as libc::pid_t, libc::SIGKILL);
        }
    }
    let _ = supervisor.kill();
    let _ = supervisor.wait();
    let mut why = why;
    if let Some(w) = owned_worker {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while w.is_live() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if w.is_live() {
            why.push_str(&format!(
                " (the stopped gated worker pid {} is not reaped yet)",
                w.pid
            ));
        }
    }
    anyhow::anyhow!(why)
}

/// Spawn an aida helper (supervisor or barrier) with a minimal explicit
/// environment, headless wait bounds, null stdin, the private log on
/// stdout/stderr and `control` at fd 3.
fn spawn_helper(
    aida: &std::path::Path,
    args: &[&str],
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    log: &std::fs::File,
    control: &std::os::unix::net::UnixStream,
    new_session: bool,
) -> Result<std::process::Child> {
    spawn_helper_with(
        aida,
        args,
        project_root,
        cwd,
        log,
        control,
        new_session,
        &[],
    )
    .map(|(child, _)| child)
}

#[allow(clippy::too_many_arguments)]
fn spawn_helper_with(
    aida: &std::path::Path,
    args: &[&str],
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    log: &std::fs::File,
    control: &std::os::unix::net::UnixStream,
    new_session: bool,
    extra_env: &[(String, String)],
) -> Result<(std::process::Child, aida_core::proc_identity::HolderRecord)> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let (ceiling_key, ceiling_value) = crate::burndown_run::bg_wait_ceiling_env(Some(project_root));
    let mut command = std::process::Command::new(aida);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(ambient_env(std::env::vars_os(), HELPER_AMBIENT_ENV, &[]))
        .envs(extra_env.iter().map(|(k, v)| (k, v)))
        .env("AIDA_HEADLESS", "1")
        .envs(crate::burndown_run::headless_wait_envs(
            ceiling_key,
            &ceiling_value,
        ))
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?);
    if !new_session {
        command.process_group(0);
    }
    install_control_fd(&mut command, control.as_raw_fd(), new_session);
    // The barrier exec preserves this child identity through worker exec.
    // trace:STORY-2537 trace:BUG-2458 | ai:codex
    let capture = aida_core::proc_identity::prepare_command(&mut command)?;
    let mut child = command
        .spawn()
        .with_context(|| format!("starting {}", aida.display()))?;
    let holder = capture.finish(&mut child);
    Ok((child, holder))
}

/// The worker's final command: explicit environment only (ambient
/// allowlist + its own identity), its own grant, headless bounds, native
/// posture argv, then the BUG-2398 finalizer exactly once.
#[allow(clippy::too_many_arguments)]
fn build_worker_exec(
    binary: &std::path::Path,
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    prompt_args: &[String],
    launch_context: Option<&AgentLaunchContext>,
    role: &str,
    grant_id: &str,
    mail_wake: agent_registry::MailWake,
) -> Result<WorkerExec> {
    let vendor = "codex";
    let (ceiling_key, ceiling_value) =
        crate::burndown_run::bg_wait_ceiling_env(Some(&plan.project_root));
    let passthrough = crate::agent_config::load_vendor_env_passthrough(&plan.project_root, vendor)?;
    let mut command = std::process::Command::new(binary);
    command
        .current_dir(&plan.launch_cwd)
        .args(exec_args(&config.default_args, prompt_args))
        .env_clear()
        .envs(worker_ambient_env(std::env::vars_os(), &passthrough))
        .env("AIDA_HEADLESS", "1")
        .envs(crate::burndown_run::headless_wait_envs(
            ceiling_key,
            &ceiling_value,
        ))
        .env(
            "AIDA_CONTAINMENT_POSTURE",
            agent_containment_posture(config),
        )
        .env("AIDA_AGENT_TYPE", vendor)
        .env("AIDA_AGENT_NAME", &plan.name)
        .env("AIDA_USER", &plan.name)
        .env("AIDA_PROJECT_ROOT", &plan.project_root)
        .env("AIDA_ROLE_INSTANCE", plan.role_instance.as_str())
        .env(crate::mail_wake::LAUNCH_WAKE_ENV, mail_wake.as_str());
    crate::agent_handle::apply_launch_env(&mut command, &plan.project_root, &plan.name);
    apply_harness_version(
        &mut command,
        harness_version_probe_program_and_args(binary, config, plan, false)?,
    );
    if let Some(spec) = &plan.current_spec {
        command.env("AIDA_SESSION_SCOPE", spec);
    }
    apply_agent_lease_identity(&mut command, plan.lease_id.as_deref());
    // trace:TASK-2902 | ai:codex
    if super::native_post_exit_spec(config, plan).is_some() {
        crate::native_publication::prepare(&plan.project_root, &plan.launch_cwd)?;
    }

    if let Some(ctx) = launch_context {
        command
            .env("AIDA_AGENT_CONTEXT_FILE", &ctx.path)
            .env("AIDA_AGENT_REGISTRY_TOKEN", &ctx.token);
    }
    command
        .env("AIDA_SESSION_ROLE", role)
        .env(crate::gate::seat_authority::GRANT_ENV, grant_id);
    crate::launch::profile::finalize_codex(&mut command)?;
    // The finalizer rebuilds the Command (default inheritance); the barrier
    // execs ONLY the explicit entries, never the launcher's environment.
    let env = command
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
        .collect();
    Ok(WorkerExec {
        program: binary.to_path_buf(),
        args: command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
        env,
        cwd: plan.launch_cwd.clone(),
    })
}

// ---------------------------------------------------------------- supervisor

/// A supervisor state transition, for deterministic fault injection in
/// tests ([`SupervisorEnv::at`]). Production injects nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// The gated worker exists; the dispatcher has not been told yet.
    Spawned,
    /// Ownership is confirmed; nothing is bound yet.
    BeforeBind,
    BeforeLease,
    BeforeAuthorship,
    BeforeRegistry,
    /// Registry, lease, authorship and grant all match; phase not yet
    /// `registered`.
    BeforeRegistered,
    /// Everything is recorded; the barrier is still closed.
    BeforeRelease,
    /// Acceptance is durable and reported.
    Accepted,
}

/// How the supervisor starts its exec barrier, and the injection point.
pub(crate) struct SupervisorEnv {
    pub barrier_program: std::path::PathBuf,
    pub barrier_args: Vec<String>,
    /// Extra explicit environment for the barrier helper (tests only).
    pub barrier_env: Vec<(String, String)>,
    /// Called at each [`Step`] with the gated worker's identity; an error is
    /// the failure of that step.
    pub at: Box<dyn Fn(Step, &ProcId) -> Result<()> + Send>,
    pub exec_corroboration: std::time::Duration,
}

impl SupervisorEnv {
    fn production() -> Result<Self> {
        Ok(Self {
            // trace:STORY-2254 | ai:claude — the hardened resolver (TASK-1262).
            barrier_program: crate::kernel::resolve_aida_exe(),
            barrier_args: vec!["internal".into(), "codex-bg-barrier".into()],
            barrier_env: Vec::new(),
            at: Box::new(|_, _| Ok(())),
            exec_corroboration: EXEC_CORROBORATION,
        })
    }
}

/// `aida internal codex-bg-supervise --attempt <ID>`.
pub(crate) fn supervise_main(attempt: &str) -> Result<()> {
    use std::os::fd::FromRawFd;
    anyhow::ensure!(uuid::Uuid::parse_str(attempt).is_ok(), "invalid attempt id");
    // The launcher may exit at any point after transfer: a write to its
    // closed channel must fail, not kill the owner of the attempt.
    // SAFETY: setting a signal disposition for this process.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    // SAFETY: fd 3 was installed by the dispatcher for this process alone.
    let control = unsafe { std::os::unix::net::UnixStream::from_raw_fd(CONTROL_FD) };
    set_cloexec(CONTROL_FD);
    control.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let mut to_parent = control.try_clone()?;
    let mut reader = BufReader::new(control);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut init: SupervisorInit =
        serde_json::from_str(line.trim_end()).context("malformed launch handoff")?;
    drop(line);
    anyhow::ensure!(
        init.attempt == attempt,
        "launch handoff names another attempt"
    );
    let fail = |to: &mut std::os::unix::net::UnixStream, reason: String| {
        let _ = send(
            to,
            &Control::Failed {
                reason: reason.clone(),
                executed: false,
            },
        );
        anyhow::anyhow!(reason)
    };
    let capability = match crate::gate::seat_authority::DetachedLaunchCapability::receive(
        init.capability
            .take()
            .context("launch handoff carried no capability")?,
    ) {
        Ok(c) => c,
        Err(e) => return Err(fail(&mut to_parent, format!("{e:#}"))),
    };
    supervise_attempt(
        attempt,
        init,
        capability,
        to_parent,
        reader,
        SupervisorEnv::production()?,
    )
}

/// The supervisor's whole lifecycle for one attempt, from a received
/// capability to terminal cleanup. `to_parent`/`reader` are the dispatcher
/// channel.
// trace:STORY-2254 | ai:claude
pub(crate) fn supervise_attempt(
    attempt: &str,
    init: SupervisorInit,
    capability: crate::gate::seat_authority::DetachedLaunchCapability,
    mut to_parent: std::os::unix::net::UnixStream,
    mut reader: BufReader<std::os::unix::net::UnixStream>,
    env: SupervisorEnv,
) -> Result<()> {
    let fail = |to: &mut std::os::unix::net::UnixStream, reason: String| {
        let _ = send(
            to,
            &Control::Failed {
                reason: reason.clone(),
                executed: false,
            },
        );
        anyhow::anyhow!(reason)
    };
    let me = ProcId::current().context("supervisor start identity unreadable")?;
    let dir = receipt::runtime_dir(&init.project_root)?;
    let (_, log) = private_log(&dir, attempt)?;

    // The gated worker: an exec barrier in its own process group.
    let (mut barrier_ctl, theirs) = std::os::unix::net::UnixStream::pair()?;
    let barrier_args: Vec<&str> = env.barrier_args.iter().map(String::as_str).collect();
    let (mut child, holder) = spawn_helper_with(
        &env.barrier_program,
        &barrier_args,
        &init.project_root,
        &init.worker.cwd,
        &log,
        &theirs,
        false,
        &env.barrier_env,
    )
    .map_err(|e| {
        fail(
            &mut to_parent,
            format!("could not start the gated worker: {e:#}"),
        )
    })?;
    drop(theirs);
    let stop = |child: &mut std::process::Child| {
        crate::kill_process_group(child.id());
        let _ = child.kill();
        let _ = child.wait();
    };
    let Some(start) = aida_core::liveness::process_start_identity(child.id()) else {
        stop(&mut child);
        return Err(fail(
            &mut to_parent,
            "the gated worker's start identity could not be read".into(),
        ));
    };
    let worker = ProcId {
        pid: child.id(),
        start,
    };
    if let Err(e) = send(
        &mut barrier_ctl,
        &Control::Exec {
            worker: init.worker.clone(),
            parent: me.pid,
        },
    ) {
        stop(&mut child);
        return Err(fail(
            &mut to_parent,
            format!("could not hand the exec plan to the gated worker: {e}"),
        ));
    }
    if let Err(e) = (env.at)(Step::Spawned, &worker) {
        stop(&mut child);
        return Err(fail(&mut to_parent, format!("{e:#}")));
    }
    if send(
        &mut to_parent,
        &Control::Spawned {
            supervisor: me.clone(),
            worker: worker.clone(),
        },
    )
    .is_err()
    {
        // Parent gone before transfer: it never committed; abort.
        stop(&mut child);
        anyhow::bail!(
            "launcher disappeared before ownership transfer; the gated worker was stopped unrun"
        );
    }
    // Wait for the ack; a lost ack is recovered from the owner record.
    let _ = recv(&mut reader);
    let owned = matches!(
        receipt::read_receipt(&dir, &init.project_root, attempt),
        ReceiptRead::Known(r) if r.owner == Owner::Supervisor(me.clone()) && r.phase == Phase::SupervisorOwned
    );
    if !owned {
        stop(&mut child);
        return Err(fail(
            &mut to_parent,
            "ownership was not transferred to the supervisor; the gated worker was stopped unrun"
                .into(),
        ));
    }

    // From here this supervisor alone owns rollback for the attempt.
    let refuse = |child: &mut std::process::Child,
                  to: &mut std::os::unix::net::UnixStream,
                  reason: String|
     -> anyhow::Error {
        stop(child);
        let _ = crate::gate::seat_authority::revoke_by_id(&init.grant_id);
        let _ = agent_registry::mark_detached_worker_ended(
            &init.project_root,
            "codex",
            worker.pid,
            &worker.start,
        );
        if let Some(ctx) = &init.context_path {
            let _ = std::fs::remove_file(ctx);
        }
        let me = me.clone();
        let reason_text = format!(
            "{reason}; the worker never executed. Its worktree and lease {} are kept: re-enter with `aida agent new codex --spec {} --cwd {}` or end the lease",
            init.lease_id,
            init.spec,
            init.worker.cwd.display()
        );
        let recorded = reason_text.clone();
        let _ = receipt::compare_update(
            &dir,
            &init.project_root,
            attempt,
            |r| {
                anyhow::ensure!(r.owner == Owner::Supervisor(me), "not owned");
                Ok(())
            },
            |r| {
                r.phase = Phase::RefusedBeforeExecution;
                r.owner = Owner::Released;
                r.reason = Some(recorded);
            },
        );
        let _ = send(
            to,
            &Control::Failed {
                reason: reason_text.clone(),
                executed: false,
            },
        );
        anyhow::anyhow!(reason_text)
    };
    let root = init.project_root.clone();
    let step = |step: Step| (env.at)(step, &worker);
    if let Err(e) = step(Step::BeforeBind).and_then(|_| {
        crate::gate::seat_authority::accept_detached_launch_child(
            capability,
            &root,
            worker.pid,
            &worker.start,
        )
    }) {
        return Err(refuse(
            &mut child,
            &mut to_parent,
            format!("grant binding refused: {e:#}"),
        ));
    }
    if let Err(e) = step(Step::BeforeLease).and_then(|_| {
        holder
            .validate()
            .map_err(|reason| anyhow::anyhow!("worker holder refused: {reason:?}"))?;
        session::stamp_lease_detached_worker_with_holder_at(
            &root,
            &init.lease_id,
            &init.grant_id,
            worker.pid,
            &worker.start,
            &holder,
        )
    }) {
        return Err(refuse(
            &mut child,
            &mut to_parent,
            format!("lease stamp refused: {e:#}"),
        ));
    }
    if let Err(e) = step(Step::BeforeAuthorship).and_then(|_| {
        crate::gate::review_authority::record_detached_worker_strict(
            &root,
            &init.lease_id,
            worker.pid,
            &worker.start,
        )
    }) {
        return Err(refuse(
            &mut child,
            &mut to_parent,
            format!("worker authorship was not recorded: {e:#}"),
        ));
    }
    let binary_id = agent_registry::AgentBinaryIdentity::new(
        env!("CARGO_PKG_VERSION").to_string(),
        env!("AIDA_BUILD_GIT_SHA").to_string(),
    );
    let row = step(Step::BeforeRegistry).and_then(|_| {
        agent_registry::register_detached_worker(
            &root,
            "codex",
            worker.pid,
            &worker.start,
            agent_registry::SpawnedRow {
                // trace:BUG-2467 | ai:codex
                delegate_seats: init.delegate_seats.clone(),
                role: Some(init.role.clone()),
                current_spec: Some(init.spec.clone()),
                worktree_path: init.worker.cwd.clone(),
                binary: Some(&binary_id),
                name: Some(init.agent_name.clone()),
                native_session_id: None,
                resumed_from: None,
                description: Some(
                    init.description
                        .clone()
                        .map(|d| format!("{d} (detached attempt {attempt})"))
                        .unwrap_or_else(|| format!("detached codex attempt {attempt}")),
                ),
            },
        )
    });
    let row = match row {
        Ok(row) => row,
        Err(e) => {
            return Err(refuse(
                &mut child,
                &mut to_parent,
                format!("registry registration refused: {e:#}"),
            ))
        }
    };
    let wake = if init.mail_wake == "off" {
        agent_registry::MailWake::Off
    } else {
        agent_registry::MailWake::Unsupported
    };
    stamp_launch_mail_wake(&root, &row, wake);
    let registered = step(Step::BeforeRegistered).and_then(|_| {
        let me = me.clone();
        receipt::compare_update(
            &dir,
            &root,
            attempt,
            |r| {
                anyhow::ensure!(
                    r.owner == Owner::Supervisor(me) && r.phase == Phase::SupervisorOwned,
                    "not owned"
                );
                Ok(())
            },
            |r| r.phase = Phase::Registered,
        )
    });
    if let Err(e) = registered {
        return Err(refuse(
            &mut child,
            &mut to_parent,
            format!("the registered phase was not recorded: {e:#}"),
        ));
    }

    // Release, then corroborate the vendor exec (EOF alone is not proof).
    if let Err(e) =
        step(Step::BeforeRelease).and_then(|_| send(&mut barrier_ctl, &Control::Release))
    {
        return Err(refuse(
            &mut child,
            &mut to_parent,
            format!("could not release the gated worker: {e}"),
        ));
    }
    barrier_ctl.set_read_timeout(Some(env.exec_corroboration))?;
    let mut barrier_reader = BufReader::new(barrier_ctl);
    let exec_report = recv(&mut barrier_reader);
    if let Ok(Some(Control::Failed { reason, .. })) = exec_report {
        return Err(refuse(
            &mut child,
            &mut to_parent,
            format!("the Codex exec failed: {reason}"),
        ));
    }
    let accepted = corroborate_exec(&mut child, &init.expected, env.exec_corroboration);
    let outcome = match accepted {
        Ok(exe) => {
            let me2 = me.clone();
            let recorded = receipt::compare_update(
                &dir,
                &root,
                attempt,
                |r| {
                    anyhow::ensure!(
                        r.owner == Owner::Supervisor(me2) && r.phase == Phase::Registered,
                        "not owned"
                    );
                    Ok(())
                },
                |r| r.phase = Phase::AcceptedRunning,
            );
            match recorded {
                Ok(_) => {
                    let _ = send(
                        &mut to_parent,
                        &Control::Accepted {
                            worker: worker.clone(),
                            exe: exe.display().to_string(),
                        },
                    );
                    let _ = step(Step::Accepted);
                    None
                }
                Err(e) => Some(format!("acceptance could not be recorded: {e:#}")),
            }
        }
        Err(why) => Some(why),
    };
    if let Some(why) = &outcome {
        // Model work may have begun: stop owned work, keep the workspace.
        crate::kill_process_group(worker.pid);
        let _ = send(
            &mut to_parent,
            &Control::Failed {
                reason: why.clone(),
                executed: true,
            },
        );
    }
    drop(to_parent);
    drop(reader);
    supervise_until_exit(&dir, attempt, &init, &me, &worker, child, outcome)
}

/// Poll the worker until its executable is the corroborated Codex binary.
pub(crate) fn corroborate_exec(
    child: &mut std::process::Child,
    expected: &ExpectedExec,
    within: std::time::Duration,
) -> std::result::Result<std::path::PathBuf, String> {
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!(
                    "the worker exited ({status}) before its Codex exec could be corroborated"
                ))
            }
            Ok(None) => {}
            Err(e) => return Err(format!("the worker could not be observed: {e}")),
        }
        if let Ok(exe) = std::fs::read_link(format!("/proc/{}/exe", child.id())) {
            if exec_matches(expected, &exe) {
                return Ok(exe);
            }
        }
        if started.elapsed() >= within {
            return Err("the worker did not become the native Codex executable in time; its process mapping cannot be proven, so it was stopped".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// Own wait/reap and exact-generation terminal cleanup.
pub(crate) fn supervise_until_exit(
    dir: &std::path::Path,
    attempt: &str,
    init: &SupervisorInit,
    me: &ProcId,
    worker: &ProcId,
    mut child: std::process::Child,
    failure: Option<String>,
) -> Result<()> {
    // Observe the exit WITHOUT reaping: while the leader is an unreaped
    // zombie its pid (and so its process group id) cannot be reused, so the
    // group signal below reaches only owned work, never an unrelated group.
    // trace:STORY-2254 | ai:claude
    let observed = wait_exit_unreaped(child.id());
    if observed.is_ok() {
        // Stop owned work left in the worker's group (a native descendant
        // that escaped the group is not claimed to be stopped).
        // SAFETY: the group's leader is this process's unreaped child.
        unsafe {
            libc::killpg(worker.pid as libc::pid_t, libc::SIGTERM);
        }
    }
    let terminal = match observed.and_then(|_| child.wait()) {
        Ok(status) => {
            use std::os::unix::process::ExitStatusExt;
            TerminalRecord {
                code: status.code(),
                signal: status.signal(),
                unknown: None,
            }
        }
        Err(e) => TerminalRecord {
            unknown: Some(format!("wait failed: {e}")),
            ..TerminalRecord::default()
        },
    };
    let mut problems = Vec::new();
    if let Err(e) = agent_registry::mark_detached_worker_ended(
        &init.project_root,
        "codex",
        worker.pid,
        &worker.start,
    ) {
        problems.push(format!("registry row not ended: {e:#}"));
    }
    match crate::gate::seat_authority::revoke_by_id(&init.grant_id) {
        Ok(_) => {}
        Err(e) => problems.push(format!("child grant not revoked: {e:#}")),
    }
    if let Some(ctx) = &init.context_path {
        if let Err(e) = std::fs::remove_file(ctx) {
            if e.kind() != std::io::ErrorKind::NotFound {
                problems.push(format!("context file not removed: {e}"));
            }
        }
    }
    // The scope is released only once the same occupancy scan
    // `recover-launch` runs proves the attempt over; anything left (or an
    // unreadable process) keeps the scope held for recovery.
    // trace:STORY-2254 | ai:claude
    if let Some(why) = exit_occupancy(dir, &init.project_root, attempt) {
        problems.push(why);
    }
    // TASK-2712 (advisor ruling F1): the native child cannot commit (Codex
    // keeps `<clone>/.git` read-only), so this host-side post-exit step
    // commits what an implementer lane left pending in its seat clone. Only
    // once the exit scan proved nothing of the lane still runs (no writer
    // races the commit). A refused or failed commit leaves the work
    // uncommitted and is reported; it does not hold the scope.
    // trace:TASK-2712 | ai:claude
    let clean_exit = terminal.unknown.is_none() && terminal.code == Some(0);
    let failure = match post_exit_seat_commit(init, problems.is_empty(), clean_exit) {
        Some(note) => Some(match failure {
            Some(why) => format!("{why}; {note}"),
            None => note,
        }),
        None => failure,
    };
    let uncertain = terminal.unknown.is_some() || !problems.is_empty();
    let me = me.clone();
    receipt::compare_update(
        dir,
        &init.project_root,
        attempt,
        |r| {
            anyhow::ensure!(
                r.owner == Owner::Supervisor(me),
                "attempt is owned elsewhere; leaving it"
            );
            Ok(())
        },
        |r| {
            r.terminal = Some(terminal);
            if uncertain {
                r.phase = Phase::RecoveryRequired;
                r.reason = Some(problems.join("; "));
            } else {
                r.phase = Phase::Exited;
                r.owner = Owner::Released;
                r.reason = failure;
            }
        },
    )?;
    Ok(())
}

/// TASK-2712: the detached lane's post-exit commit of its seat's pending
/// work ([`crate::ship_from_seat::post_exit_commit_reported`]), for an
/// implementer lane whose exit scan came back clear. A note for the
/// receipt when the commit was refused or failed; `None` otherwise.
/// `clean_exit` (status 0) decides a completing vs `(Refs SPEC)` trailer.
// trace:TASK-2712 | ai:claude
fn post_exit_seat_commit(
    init: &SupervisorInit,
    exit_clear: bool,
    clean_exit: bool,
) -> Option<String> {
    if aida_core::team::canonical_role(&init.role) != "implementer" {
        return None;
    }
    if !exit_clear {
        // TASK-2032: anchored on the lane's recorded host project, never the
        // supervisor's cwd. trace:TASK-2032 | ai:claude
        let main = crate::main_worktree_root_from(&init.project_root);
        return crate::seat_clone::host_recorded_clone(&main, &init.worker.cwd).map(|_| {
            "the lane's pending seat work was not committed: the exit scan could not prove \
             the lane over"
                .to_string()
        });
    }
    match crate::ship_from_seat::post_exit_commit_reported(
        &init.project_root,
        &init.worker.cwd,
        &init.spec,
        &init.agent_name,
        clean_exit,
    ) {
        crate::ship_from_seat::PostExitCommit::Failed(why) => Some(format!(
            "the lane's pending seat work was not committed: {why}"
        )),
        _ => None,
    }
}

/// How long the normal exit waits for signalled work to leave before it
/// concludes the attempt is not provably over.
const EXIT_OCCUPANCY_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// `None` when a fresh scan proves the attempt over; otherwise why the
/// scope must stay held. Re-scans briefly: work just signalled may still be
/// exiting. Fails closed: the last non-clear result is the answer.
// trace:STORY-2254 | ai:claude
pub(crate) fn exit_occupancy(
    dir: &std::path::Path,
    root: &std::path::Path,
    attempt: &str,
) -> Option<String> {
    let r = match receipt::read_receipt(dir, root, attempt) {
        ReceiptRead::Known(r) => r,
        ReceiptRead::Unknown(why) => {
            return Some(format!("receipt unreadable for the exit scan: {why}"))
        }
    };
    let own = own_process_chain();
    let deadline = std::time::Instant::now() + EXIT_OCCUPANCY_GRACE;
    loop {
        let why = match attempt_occupancy(&r, &own) {
            receipt::Occupancy::Clear => return None,
            receipt::Occupancy::Occupied(what) => format!("attempt not provably over: {what}"),
            receipt::Occupancy::Unknown(why) => format!("attempt cannot be proven over ({why})"),
        };
        if std::time::Instant::now() >= deadline {
            return Some(why);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Block until child `pid` has exited, leaving it unreaped (`WNOWAIT`).
// trace:STORY-2254 | ai:claude
pub(crate) fn wait_exit_unreaped(pid: u32) -> std::io::Result<()> {
    loop {
        // SAFETY: waitid writes only into the zeroed siginfo we own.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn set_cloexec(fd: i32) {
    // SAFETY: fcntl on a descriptor this process owns.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
    }
}

// ------------------------------------------------------------------ barrier

/// `aida internal codex-bg-barrier`: block until released, then become Codex.
pub(crate) fn barrier_main() -> Result<()> {
    use std::os::fd::FromRawFd;
    use std::os::unix::process::CommandExt;
    // SAFETY: fd 3 was installed by the supervisor for this process alone.
    let control = unsafe { std::os::unix::net::UnixStream::from_raw_fd(CONTROL_FD) };
    set_cloexec(CONTROL_FD);
    let mut out = control.try_clone()?;
    let mut reader = BufReader::new(control);
    let Some(Control::Exec { worker, parent }) = recv(&mut reader)? else {
        anyhow::bail!("gated worker got no exec plan; exiting unrun");
    };
    match recv(&mut reader) {
        Ok(Some(Control::Release)) => {}
        // EOF: the supervisor died or aborted. Exit without executing.
        _ => std::process::exit(0),
    }
    // Tie this worker to the supervisor (never the dispatcher), then recheck
    // that the supervisor did not exit before the signal was armed.
    // SAFETY: prctl/getppid only affect/read this process.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
        if libc::getppid() as u32 != parent {
            std::process::exit(0);
        }
    }
    let error = std::process::Command::new(&worker.program)
        .args(&worker.args)
        .env_clear()
        .envs(worker.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&worker.cwd)
        .exec();
    let _ = send(
        &mut out,
        &Control::Failed {
            reason: format!("exec {}: {error}", worker.program.display()),
            executed: false,
        },
    );
    std::process::exit(127);
}

// --------------------------------------------------- status and recovery

/// `aida agent launch-status <ATTEMPT>`: strictly read-only.
pub(crate) fn launch_status(attempt: &str) -> Result<()> {
    let root = main_worktree_root_from(&find_aida_project_root_from(&std::env::current_dir()?)?);
    let Some(dir) = receipt::existing_runtime_dir(&root) else {
        anyhow::bail!("no detached launch receipts exist for this project");
    };
    print!("{}", launch_status_at(&dir, &root, attempt));
    Ok(())
}

/// The read-only status text for one attempt.
// trace:STORY-2254 | ai:claude
pub(crate) fn launch_status_at(
    dir: &std::path::Path,
    root: &std::path::Path,
    attempt: &str,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    match receipt::read_receipt(dir, root, attempt) {
        ReceiptRead::Unknown(why) => {
            let _ = writeln!(out, "attempt {attempt}: UNKNOWN ({why})");
            match receipt::unknown_receipt_scope(dir, attempt) {
                Some(scope) => {
                    let _ = writeln!(
                        out,
                        "  holds:   {} {} (read leniently)",
                        scope.role, scope.spec
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "  holds:   any scope while something may still run for it"
                    );
                }
            }
            let _ = writeln!(out, "  an unknown receipt never permits cleanup; once nothing runs for it: `aida agent recover-launch {attempt} --retire-unknown`");
        }
        ReceiptRead::Known(r) => {
            let _ = writeln!(out, "attempt {}: {}", r.attempt, r.phase.as_str());
            let _ = writeln!(out, "  scope:   {} {}", r.scope.role, r.scope.spec);
            let _ = writeln!(out, "  name:    {}", r.agent_name);
            let _ = writeln!(
                out,
                "  owner:   {}",
                match &r.owner {
                    Owner::Dispatcher(p) => format!("launcher pid {}{}", p.pid, live_label(p)),
                    Owner::Supervisor(p) => format!("supervisor pid {}{}", p.pid, live_label(p)),
                    Owner::Released => "released".into(),
                }
            );
            if let Some(w) = &r.worker {
                let _ = writeln!(out, "  worker:  pid {}{}", w.pid, live_label(w));
            }
            if let Some(lease) = &r.lease_id {
                let _ = writeln!(out, "  lease:   {lease}");
            }
            let _ = writeln!(out, "  worktree: {}", r.worktree.display());
            let _ = writeln!(out, "  log:     {}", r.log_path.display());
            if let Some(t) = &r.terminal {
                let line = match (&t.unknown, t.code, t.signal) {
                    (Some(why), _, _) => format!("unknown ({why})"),
                    (None, Some(code), _) => {
                        format!("code {code} (exit status is not task success)")
                    }
                    (None, None, Some(sig)) => format!("signal {sig}"),
                    _ => "unknown".into(),
                };
                let _ = writeln!(out, "  exit:    {line}");
            }
            if let Some(reason) = &r.reason {
                let _ = writeln!(out, "  reason:  {reason}");
            }
        }
    }
    out
}

fn live_label(p: &ProcId) -> &'static str {
    match p.state() {
        receipt::ProcState::Live => " (live)",
        receipt::ProcState::Gone => " (not running)",
        receipt::ProcState::Unknown(_) => " (liveness unknown)",
    }
}

/// `aida agent recover-launch <ATTEMPT> [--retire-unknown]`.
pub(crate) fn recover_launch(attempt: &str, retire_unknown: bool) -> Result<()> {
    let root = main_worktree_root_from(&find_aida_project_root_from(&std::env::current_dir()?)?);
    let dir = receipt::existing_runtime_dir(&root)
        .context("no detached launch receipts exist for this project")?;
    let own = own_process_chain();
    if retire_unknown {
        let kept = receipt::retire_unknown(&dir, &root, attempt)?;
        println!(
            "retired unknown attempt {attempt}: no process runs for it; its receipt is kept as evidence at {}",
            kept.display()
        );
        return Ok(());
    }
    let message = recover_launch_at(&dir, &root, attempt, &own)?;
    println!("{message}");
    Ok(())
}

/// The ONE occupancy scan both the normal exit and `recover-launch` run
/// before releasing an attempt's scope: every process in the worktree, in
/// the worker's group or the supervisor's session, or naming the attempt.
// trace:STORY-2254 | ai:claude
pub(crate) fn attempt_occupancy(
    r: &LaunchReceipt,
    own: &std::collections::HashSet<u32>,
) -> receipt::Occupancy {
    let launcher = match &r.owner {
        Owner::Dispatcher(p) => Some(p),
        _ => None,
    };
    let probe = receipt::OccupancyProbe {
        worktree: Some(&r.worktree),
        group: r.worker.as_ref().map(|w| w.pid),
        session: r.supervisor.as_ref().map(|s| s.pid),
        attempt: Some(&r.attempt),
        // The launcher, supervisor and worker are the attempt's first
        // processes; nothing started before all of them is its work.
        not_before: [launcher, r.supervisor.as_ref(), r.worker.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(ProcId::start_tick)
            .min(),
    };
    receipt::scan_occupancy(&probe, own)
}

/// This process and its ancestors: the recovering shell is not an occupant.
pub(crate) fn own_process_chain() -> std::collections::HashSet<u32> {
    let mut own = std::collections::HashSet::new();
    let mut pid = Some(std::process::id());
    while let Some(p) = pid.filter(|p| *p > 1 && own.insert(*p)) {
        pid = agent_registry::parent_pid_of(p);
    }
    own
}

/// Reconcile exactly `attempt`. Never spawns, resumes, mints, binds or
/// kills; repairs only exact owned bookkeeping, and only once a FRESH scan
/// corroborates that the worker, the supervisor, the launcher, every
/// process in the worktree and every process left in the worker's group or
/// the supervisor's session are gone. Fails CLOSED: an unreadable `/proc`,
/// cwd or start identity, a missing recorded start, or a possibly escaped
/// descendant means UNKNOWN: nothing is changed and nothing is revoked.
// trace:STORY-2254 | ai:claude
pub(crate) fn recover_launch_at(
    dir: &std::path::Path,
    root: &std::path::Path,
    attempt: &str,
    own: &std::collections::HashSet<u32>,
) -> Result<String> {
    let r = match receipt::read_receipt(dir, root, attempt) {
        ReceiptRead::Known(r) => *r,
        ReceiptRead::Unknown(why) => anyhow::bail!(
            "attempt {attempt} is UNKNOWN ({why}); nothing was changed and its scope stays held. Once nothing runs for it, retire it with `aida agent recover-launch {attempt} --retire-unknown`"
        ),
    };
    if !r.phase.holds_scope() {
        return Ok(format!(
            "attempt {attempt} is already {}; nothing to recover",
            r.phase.as_str()
        ));
    }
    let mut owned: Vec<(&str, &ProcId)> = Vec::new();
    if let Owner::Dispatcher(p) = &r.owner {
        owned.push(("launcher", p));
    }
    if let Some(p) = &r.supervisor {
        owned.push(("supervisor", p));
    }
    if let Some(p) = &r.worker {
        owned.push(("worker", p));
    }
    anyhow::ensure!(
        r.worker.is_some() || r.phase == Phase::Prepared,
        "attempt {attempt} is {} but records no worker identity; its state is UNKNOWN and nothing was changed",
        r.phase.as_str()
    );
    for (what, p) in &owned {
        match p.state() {
            receipt::ProcState::Gone => {}
            receipt::ProcState::Live => anyhow::bail!(
                "attempt {attempt} still has its live {what} (pid {}); nothing was changed",
                p.pid
            ),
            receipt::ProcState::Unknown(why) => anyhow::bail!(
                "attempt {attempt}'s {what} cannot be proven gone ({why}); its state is UNKNOWN and nothing was changed"
            ),
        }
    }
    match attempt_occupancy(&r, own) {
        receipt::Occupancy::Clear => {}
        receipt::Occupancy::Occupied(what) => anyhow::bail!(
            "attempt {attempt} is not provably over: {what}; nothing was changed"
        ),
        receipt::Occupancy::Unknown(why) => anyhow::bail!(
            "attempt {attempt} cannot be proven over ({why}); its state is UNKNOWN and nothing was changed"
        ),
    }
    if let Some(w) = &r.worker {
        agent_registry::mark_detached_worker_ended(root, "codex", w.pid, &w.start)?;
    }
    if let Some(grant) = &r.grant_id {
        crate::gate::seat_authority::revoke_by_id(grant)?;
    }
    if let Some(ctx) = &r.context_path {
        let _ = std::fs::remove_file(ctx);
    }
    let generation = r.generation;
    receipt::compare_update(
        dir,
        root,
        attempt,
        |now| {
            anyhow::ensure!(
                now.generation == generation,
                "the attempt changed while recovering; nothing more was changed"
            );
            Ok(())
        },
        |now| {
            now.phase = Phase::Exited;
            now.owner = Owner::Released;
            if now.terminal.is_none() {
                now.terminal = Some(TerminalRecord {
                    unknown: Some("recovered after its processes were gone".into()),
                    ..TerminalRecord::default()
                });
            }
            now.reason = Some(
                "reconciled by `aida agent recover-launch`; worktree, branch and lease kept".into(),
            );
        },
    )?;
    Ok(format!("recovered attempt {attempt}: its scope reservation is released; worktree, branch and lease are kept"))
}

#[cfg(all(test, target_os = "linux"))]
#[path = "tests/story_2254_codex_background_tests.rs"]
mod story_2254_codex_background_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "tests/story_2254_codex_bg_supervisor_tests.rs"]
mod story_2254_codex_bg_supervisor_tests;

// Exercise the real detached-worker builder without spawning a supervisor.
// trace:TASK-2902 | ai:codex
#[cfg(test)]
pub(crate) fn publication_test_worker_exec(
    binary: &std::path::Path,
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
) -> Result<()> {
    build_worker_exec(
        binary,
        config,
        plan,
        &[],
        None,
        "implementer",
        "test",
        agent_registry::MailWake::Off,
    )
    .map(|_| ())
}
