//! Headless Codex boundary. Credentials exist only in the exact child env.
// trace:TASK-2707 | ai:codex
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const COMMON: &str = include_str!("egress_probe.py");
const LANE: &str = include_str!("contained_lane.py");
const HOST: &str = include_str!("contained_host.py");
// TOOL_ENV is deliberately independent of this transport list. Never add
// registry/launch tokens, arbitrary proxy values, or parent grant context.
const ENV: &[&str] = &[
    "PWD",
    "PATH",
    "HOME",
    "LANG",
    "AIDA_HOME",
    "CODEX_HOME",
    "CODEX_SQLITE_HOME",
    "XDG_RUNTIME_DIR",
    "TMPDIR",
    "OPENAI_API_KEY",
    crate::gate::seat_authority::GRANT_ENV,
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
    "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS",
    "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS",
    "AIDA_CONTAINED",
    "AIDA_DRIVE_ROOT",
    "AIDA_HARNESS_VERSION",
];
const TOOL_NET_ENV: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "NO_PROXY",
];

// Public build paths, never secret-bearing tool environment. Keep explicit
// toolchain/target selection under the tool shell's inherit=core policy.
const TOOL_BUILD_ENV: &[&str] = &[
    "CARGO_HOME",
    "RUSTUP_HOME",
    "CARGO_TARGET_DIR",
    "RUSTC_WRAPPER",
];

/// Constructed only by accepting host evidence while the attested shim is
/// blocked before exec. Fields are private: callers cannot manufacture credit.
#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContainmentReceipt {
    #[serde(skip)]
    accepted: bool,
    version: u32,
    posture: String,
    worker_pid: u32,
    worker_start: String,
    wrapper_pid: u32,
    namespaces: BTreeMap<String, PathBuf>,
    uid_map_identity: bool,
    seat_preflight: bool,
    broker: String,
    relay_port: u16,
    policy_sha: String,
    keyring: String,
}
impl ContainmentReceipt {
    pub(crate) fn is_accepted(&self) -> bool {
        self.accepted
    }
    pub(crate) fn worker_pid(&self) -> u32 {
        self.worker_pid
    }
    pub(crate) fn validate(&self, sha: &str) -> Result<()> {
        anyhow::ensure!(
            self.version == 1
                && self.posture == "aida"
                && self.policy_sha == sha
                && self.uid_map_identity
                && self.seat_preflight
                && self.relay_port > 0
                && self.broker == "seccomp_notify_addfd"
                && self.keyring == "isolated_child_only",
            "contained launch receipt refused"
        );
        anyhow::ensure!(
            self.worker_pid > 0 && self.wrapper_pid > 0,
            "receipt PID missing"
        );
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", self.worker_pid))?;
        anyhow::ensure!(
            stat.rsplit_once(')')
                .context("worker stat invalid")?
                .1
                .split_whitespace()
                .nth(19)
                == Some(self.worker_start.as_str()),
            "receipt worker changed"
        );
        let identity = format!("{} {} 1", unsafe { libc::getuid() }, unsafe {
            libc::getuid()
        });
        let mapping = std::fs::read_to_string(format!("/proc/{}/uid_map", self.worker_pid))?
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        anyhow::ensure!(mapping == identity, "receipt uid mapping refused");
        anyhow::ensure!(self.namespaces.len() == 7, "receipt namespaces incomplete");
        for name in ["pid", "net", "user", "mnt", "ipc", "uts", "cgroup"] {
            let expected = self
                .namespaces
                .get(name)
                .context("receipt namespace missing")?;
            anyhow::ensure!(
                std::fs::read_link(format!("/proc/{}/ns/{name}", self.worker_pid))? == *expected
                    && std::fs::read_link(format!("/proc/self/ns/{name}"))? != *expected,
                "receipt namespace identity refused"
            );
        }
        Ok(())
    }
}

struct Mount {
    file: File,
    path: PathBuf,
    writable: bool,
}
struct WriteProfile {
    mounts: Vec<Mount>,
}
impl WriteProfile {
    fn new() -> Self {
        Self { mounts: Vec::new() }
    }
    fn add(&mut self, path: &Path, writable: bool) -> Result<()> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let md = file.metadata()?;
        anyhow::ensure!(
            !md.file_type().is_symlink() && (md.is_dir() || md.is_file()),
            "unsafe mount identity"
        );
        anyhow::ensure!(
            !writable || md.is_dir(),
            "contained write requires a directory leaf"
        );
        anyhow::ensure!(
            !md.is_file() || md.nlink() == 1,
            "hard-linked mount refused"
        );
        if writable {
            reject_hardlink_aliases(path)?;
            anyhow::ensure!(
                !path
                    .components()
                    .any(|p| p.as_os_str() == "session-grants"
                        || p.as_os_str() == "session-grant-locks"),
                "canonical grants are host-only; directory-leaf refusal"
            );
        }
        self.mounts.push(Mount {
            file,
            path: path.into(),
            writable,
        });
        Ok(())
    }
    fn args(&self, cwd: &Path) -> Vec<String> {
        let mut args: Vec<String> = [
            "--unshare-all",
            "--die-with-parent",
            "--cap-drop",
            "ALL",
            "--new-session",
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--tmpfs",
            "/run",
            "--chdir",
        ]
        .map(str::to_string)
        .into();
        args.push(cwd.to_string_lossy().into_owned());
        for m in &self.mounts {
            args.extend([
                if m.writable {
                    "--bind-fd"
                } else {
                    "--ro-bind-fd"
                }
                .into(),
                m.file.as_raw_fd().to_string(),
                m.path.to_string_lossy().into_owned(),
            ]);
        }
        args
    }
    fn native(&self) -> String {
        let mut fs = toml::map::Map::new();
        fs.insert("/".into(), toml::Value::String("read".into()));
        for m in &self.mounts {
            fs.insert(
                m.path.to_string_lossy().into_owned(),
                toml::Value::String(if m.writable { "write" } else { "read" }.into()),
            );
        }
        format!(
            "[permissions.aida.filesystem]\n{}\n[permissions.aida.network]\nenabled = true\n",
            toml::to_string(&fs).unwrap()
        )
    }
}

// Existing hard links inside a writable bind could alias protected host files.
// Symlinks are not traversed: their targets retain the outer mount permissions.
fn reject_hardlink_aliases(root: &Path) -> Result<()> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let md = std::fs::symlink_metadata(entry.path())?;
            if md.is_dir() {
                pending.push(entry.path());
            } else if md.is_file() {
                anyhow::ensure!(md.nlink() == 1, "hard-linked writable alias refused");
            }
        }
    }
    Ok(())
}
fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    let md = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        md.is_dir() && !md.file_type().is_symlink() && md.uid() == unsafe { libc::getuid() },
        "unsafe required directory"
    );
    Ok(())
}
fn explicit(child: &Command, key: &str) -> Option<String> {
    child
        .get_envs()
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| v)
        .and_then(|v| v.to_str())
        .map(str::to_owned)
}
fn relay_environment(port: u16) -> BTreeMap<String, String> {
    TOOL_NET_ENV
        .iter()
        .map(|name| {
            (
                (*name).into(),
                if *name == "NO_PROXY" {
                    String::new()
                } else {
                    format!("http://127.0.0.1:{port}")
                },
            )
        })
        .collect()
}
fn selected_environment(child: &Command) -> BTreeMap<String, String> {
    ENV.iter()
        .filter_map(|name| explicit(child, name).map(|v| ((*name).into(), v)))
        .collect()
}
fn resolve_program(child: &Command, env: &BTreeMap<String, String>) -> Result<PathBuf> {
    let program = Path::new(child.get_program());
    if program.is_absolute() {
        return Ok(std::fs::canonicalize(program)?);
    }
    anyhow::ensure!(
        program.components().count() == 1,
        "contained vendor must use an absolute path or a PATH basename"
    );
    for dir in std::env::split_paths(&std::ffi::OsString::from(
        env.get("PATH").context("contained PATH missing")?,
    )) {
        if !dir.is_absolute() {
            continue;
        }
        let p = dir.join(program);
        if let Ok(md) = std::fs::metadata(&p) {
            if md.is_file() && md.mode() & 0o111 != 0 {
                return Ok(std::fs::canonicalize(p)?);
            }
        }
    }
    anyhow::bail!("contained vendor executable unavailable")
}
fn refuse_chatgpt(home: &Path) -> Result<()> {
    anyhow::ensure!(
        !home.join("auth.json").exists(),
        "contained ChatGPT auth waits for P3 (PR2)"
    );
    Ok(())
}
/// Host vendor state the sandbox must see as an empty read-only view: the
/// default `~/.codex` and `~/.claude`, plus the resolved Codex home and
/// `CODEX_SQLITE_HOME` when an operator relocated them (else the relocated
/// credentials and thread database stay readable to the contained worker).
/// Only existing paths (bwrap needs a mount target), each once, compared by
/// canonical path so a symlinked relocation onto the default is not doubled.
// trace:TASK-2708 | ai:claude
fn host_vendor_state_masks(
    host_home: &Path,
    codex_home: &Path,
    sqlite_home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut masks: Vec<PathBuf> = Vec::new();
    let candidates = [
        Some(host_home.join(".codex")),
        Some(host_home.join(".claude")),
        Some(codex_home.to_path_buf()),
        sqlite_home.map(Path::to_path_buf),
    ];
    for p in candidates.into_iter().flatten() {
        let Ok(canonical) = std::fs::canonicalize(&p) else {
            continue;
        };
        if !masks.contains(&canonical) {
            masks.push(canonical);
        }
    }
    masks
}
/// The host kernel prerequisites `contained_host.py::host_contained` checks
/// first (x86_64 broker, Landlock ABI 1..=8, seccomp user notification),
/// checked here BEFORE any lane state exists so an unsupported host gets the
/// typed cause up front, and the live tests skip on exactly the same gate.
// trace:TASK-2708 | ai:claude
pub(crate) fn host_prerequisites() -> Result<()> {
    anyhow::ensure!(
        std::env::consts::ARCH == "x86_64",
        "socket broker architecture unsupported"
    );
    // landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)
    let abi =
        unsafe { libc::syscall(444, 0 as libc::c_long, 0 as libc::c_long, 1 as libc::c_long) };
    anyhow::ensure!(
        abi > 0,
        "Landlock capability unavailable; socket-policy selection refused"
    );
    anyhow::ensure!(
        abi < 9,
        "Landlock Unix-connect backend required on ABI>=9; broker not selected"
    );
    // seccomp(SECCOMP_GET_NOTIF_SIZES, 0, &sizes)
    let mut sizes = [0u16; 3];
    anyhow::ensure!(
        unsafe {
            libc::syscall(
                317,
                3 as libc::c_long,
                0 as libc::c_long,
                sizes.as_mut_ptr(),
            )
        } == 0,
        "seccomp notification size probe unavailable"
    );
    Ok(())
}
fn pipe() -> Result<(File, File)> {
    let mut fds = [-1; 2];
    anyhow::ensure!(
        unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } == 0,
        "contained readiness pipe unavailable"
    );
    Ok(unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) })
}

pub(crate) struct PreparedLaunch {
    command: Command,
    profile: WriteProfile,
    _state: tempfile::TempDir,
    policy_sha: String,
    reader: File,
    writer: File,
    accept_reader: File,
    accept_writer: File,
}
impl PreparedLaunch {
    /// Takes finalized child assignments only. Never inherits ambient authority.
    pub(crate) fn prepare(child: &Command, root: &Path) -> Result<Self> {
        anyhow::ensure!(
            explicit(child, "AIDA_HEADLESS").as_deref() == Some("1"),
            "Codex AIDA posture currently supports foreground headless launches only (PR2 pending)"
        );
        anyhow::ensure!(
            Path::new("/usr/bin/python3").is_file(),
            "contained launch requires /usr/bin/python3"
        );
        anyhow::ensure!(
            Path::new("/usr/bin/bwrap").is_file(),
            "contained launch requires /usr/bin/bwrap"
        );
        host_prerequisites()?;
        let sha = crate::trusted_config::local_trusted_default_branch_sha(root)
            .context("trusted containment policy unavailable")?;
        let body = crate::trusted_config::read_config_at_sha(root, &sha)
            .context("pinned policy unreadable")?;
        let policy: toml::Value = toml::from_str(&body)?;
        anyhow::ensure!(
            policy.pointer_vendor() == Some("aida"),
            "Codex AIDA posture requires trusted policy"
        );
        let allowed = policy
            .get("contained")
            .and_then(|c| c.get("allowed_hosts"))
            .map(|v| -> Result<Vec<String>> {
                v.as_array()
                    .context("allowed_hosts must be an array")?
                    .iter()
                    .map(|h| {
                        Ok(h.as_str()
                            .context("allowed host must be a string")?
                            .to_owned())
                    })
                    .collect()
            })
            .transpose()?
            .unwrap_or_default();
        if policy
            .get("contained")
            .and_then(|c| c.get("read_allowlist"))
            .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
        {
            anyhow::bail!(
                "contained Codex strict read allowlist is not supported by this launch slice"
            );
        }
        for host in &allowed {
            anyhow::ensure!(
                host.len() <= 253
                    && host.parse::<std::net::IpAddr>().is_err()
                    && host.split('.').all(|label| !label.is_empty()
                        && label.len() <= 63
                        && label
                            .as_bytes()
                            .first()
                            .is_some_and(u8::is_ascii_alphanumeric)
                        && label
                            .as_bytes()
                            .last()
                            .is_some_and(u8::is_ascii_alphanumeric)
                        && label
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')),
                "contained allowed_hosts requires exact lowercase DNS hostnames"
            );
        }
        let root = std::fs::canonicalize(root)?;
        let state = tempfile::Builder::new()
            .prefix("aida-contained-")
            .tempdir()?;
        std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o700))?;
        // trace:TASK-2708 | ai:claude — one home resolution for the crate.
        let host_home = crate::home_dir().context("host HOME unavailable")?;
        let codex_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| host_home.join(".codex"));
        refuse_chatgpt(&codex_home)?;
        let key = std::env::var("OPENAI_API_KEY").map_err(|_| {
            anyhow::anyhow!(
                "contained Codex requires OPENAI_API_KEY; ChatGPT auth waits for P3 (PR2)"
            )
        })?;
        anyhow::ensure!(!key.is_empty(), "contained Codex requires OPENAI_API_KEY");
        let grant = explicit(child, crate::gate::seat_authority::GRANT_ENV)
            .context("contained child requires explicit grant decision")?;
        let ttl = if grant.is_empty() {
            0
        } else {
            (crate::gate::seat_authority::read_grant(&grant)?.expires_at - chrono::Utc::now())
                .num_seconds()
                .max(1)
        };
        let mut profile = WriteProfile::new();
        profile.add(&root, true)?;
        let main = crate::main_worktree_root_from(&root);
        crate::session::verify_linked_worktree_main(&root, &main)?;
        for p in crate::contained_boundary::contained_store_write_paths(&main) {
            let p =
                std::fs::canonicalize(p).context("required store/cache directory unavailable")?;
            profile.add(&p, true)?;
        }
        if let Some(clone) = crate::seat_clone::host_recorded_clone(&root) {
            for path in crate::seat_clone::seat_extra_write_paths(&clone) {
                profile.add(&path, true)?;
            }
        }
        profile.add(state.path(), true)?;
        // Mask host vendor credentials/daemon endpoints with launch-owned empty RO views.
        let empty = state.path().join("empty");
        ensure_dir(&empty)?;
        // trace:TASK-2708 | ai:claude — including a relocated CODEX_HOME /
        // CODEX_SQLITE_HOME, which the default-path masks alone would leave
        // readable inside the sandbox.
        let sqlite_home = std::env::var_os("CODEX_SQLITE_HOME").map(PathBuf::from);
        for p in host_vendor_state_masks(&host_home, &codex_home, sqlite_home.as_deref()) {
            let file = File::open(&empty)?;
            profile.mounts.push(Mount {
                file,
                path: p,
                writable: false,
            });
        }
        // Read-only parents prevent absent-name creation as well as overwrites.
        for base in [&root, &main] {
            for name in [".aida", ".agents", ".codex", ".claude"] {
                let p = base.join(name);
                ensure_dir(&p)?;
                profile.add(&p, false)?;
            }
            let git = base.join(".git");
            if git.exists() {
                if base == &root && crate::seat_clone::host_recorded_clone(&root).is_some() {
                    // Explicit native carve-out: the native workspace default
                    // protects .git even when its parent is writable.
                    profile.add(&git, true)?;
                    for name in ["config", "config.worktree", "hooks", "info", "objects/info"] {
                        let p = git.join(name);
                        if !p.exists() {
                            if name.contains("config") {
                                std::fs::write(&p, "")?;
                            } else {
                                ensure_dir(&p)?;
                            }
                        }
                        profile.add(&p, false)?;
                    }
                } else {
                    profile.add(&git, false)?;
                    if let Some(dir) = aida_core::git_ops::worktree_gitdir_path(base) {
                        profile.add(&std::fs::canonicalize(dir)?, false)?;
                    }
                }
            }
            for name in ["AGENTS.md", "CLAUDE.md", ".mcp.json"] {
                let p = base.join(name);
                if !p.exists() {
                    std::fs::write(&p, "")?;
                }
                profile.add(&p, false)?;
            }
        }
        for p in crate::contained_boundary::contained_store_deny_write_paths(
            &crate::contained_boundary::contained_store_write_paths(&main),
        ) {
            if p.exists() {
                profile.add(&std::fs::canonicalize(p)?, false)?;
            }
        }
        // Protect global authority even when an operator's configured AIDA
        // home happens to overlap a writable project/cache ancestor. Only the
        // explicit cache and own informational notice leaves below are rw.
        let authority_home =
            crate::gate::kernel_deps::aida_home_dir().context("AIDA home unavailable")?;
        let authority_state = authority_home.join(".aida");
        ensure_dir(&authority_state)?;
        profile.add(&authority_state, false)?;
        // Cache is an explicit exception below the protected .aida parent.
        let cache = aida_core::db::cache_dir_in(&main.join(".aida"));
        profile.add(&std::fs::canonicalize(cache)?, true)?;
        if !grant.is_empty() {
            let notices = crate::grant_expiry_notice::notice_leaf(&grant)?;
            ensure_dir(&notices)?;
            profile.add(&notices, true)?;
        }
        // HOME remains the issuer's canonical AIDA home (read-only). Core
        // tool filtering retains HOME but not AIDA_HOME; relocating HOME would
        // silently redirect grant lookup. Vendor mutable state is separate.
        let private_codex = state.path().join("codex");
        ensure_dir(&private_codex)?;
        let runtime = state.path().join("runtime");
        ensure_dir(&runtime)?;
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))?;
        let config = private_codex.join("config.toml");
        std::fs::write(&config, profile.native())?;
        profile.add(&config, false)?;
        let mut env = selected_environment(child);
        env.insert(
            "CARGO_HOME".into(),
            std::env::var_os("CARGO_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| host_home.join(".cargo"))
                .to_string_lossy()
                .into_owned(),
        );
        env.insert(
            "RUSTUP_HOME".into(),
            std::env::var_os("RUSTUP_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| host_home.join(".rustup"))
                .to_string_lossy()
                .into_owned(),
        );
        env.insert(
            "CARGO_TARGET_DIR".into(),
            root.join("target").to_string_lossy().into_owned(),
        );
        env.insert("RUSTC_WRAPPER".into(), String::new());
        if let Some(clone) = crate::seat_clone::host_recorded_clone(&root) {
            for (name, value) in crate::seat_clone::seat_build_env(&clone) {
                env.insert(name.into(), value);
            }
        }
        for (name, value) in [
            ("HOME", authority_home.to_string_lossy().into_owned()),
            ("CODEX_HOME", private_codex.to_string_lossy().into_owned()),
            (
                "CODEX_SQLITE_HOME",
                private_codex.to_string_lossy().into_owned(),
            ),
            ("XDG_RUNTIME_DIR", runtime.to_string_lossy().into_owned()),
            ("TMPDIR", "/tmp".into()),
            ("LANG", "C.UTF-8".into()),
            (
                "PATH",
                explicit(child, "PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            ),
            ("AIDA_HOME", authority_home.to_string_lossy().into_owned()),
            ("AIDA_CONTAINED", "1".into()),
            ("AIDA_HEADLESS", "1".into()),
            ("AIDA_CONTAINMENT_POSTURE", "aida".into()),
            ("OPENAI_API_KEY", key),
        ] {
            env.insert(name.into(), value);
        }
        // Select an ephemeral number; the listener binds it only in the new
        // network namespace. No persistent host port is assigned.
        let reservation = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let port = reservation.local_addr()?.port();
        drop(reservation);
        env.extend(relay_environment(port));
        let program = resolve_program(child, &env)?;
        let mut final_child = Command::new(&program);
        final_child.env_clear().envs(&env);
        let mut args = crate::codex_tool_env::codex_tool_env_overrides(&final_child)?;
        for name in TOOL_NET_ENV.iter().chain(TOOL_BUILD_ENV) {
            args.extend([
                "-c".into(),
                format!(
                    "shell_environment_policy.set.{name}={}",
                    toml::Value::String(env[*name].clone())
                ),
            ]);
        }
        args.extend(["-P".into(), "aida".into()]);
        let original: Vec<_> = child
            .get_args()
            .map(|v| v.to_string_lossy().into_owned())
            .collect();
        if let Some(at) = original.iter().position(|a| a == "exec") {
            anyhow::ensure!(
                !original
                    .get(at + 1)
                    .is_some_and(|a| matches!(a.as_str(), "resume" | "fork")),
                "contained Codex resume/fork waits for private-state lifecycle support (PR2)"
            );
        }
        let mut i = 0;
        while i < original.len() {
            if original[i] == "--sandbox"
                && original.get(i + 1).map(String::as_str) == Some("workspace-write")
            {
                i += 2;
                continue;
            }
            anyhow::ensure!(
                !matches!(
                    original[i].as_str(),
                    "--dangerously-bypass-approvals-and-sandbox"
                        | "--full-auto"
                        | "--sandbox"
                        | "-s"
                        | "-P"
                        | "--permission-profile"
                ),
                "contained Codex cannot override its generated native profile"
            );
            args.push(original[i].clone());
            i += 1;
        }
        let mut preflight = vec![program.to_string_lossy().into_owned()];
        preflight.extend(crate::codex_tool_env::sandbox_args_for_contained(&args)?);
        preflight.extend([
            "--".into(),
            crate::aida_exe_path().to_string_lossy().into_owned(),
            "role".into(),
            "current".into(),
            "--probe-seat".into(),
            "--expect".into(),
            env.get("AIDA_SESSION_ROLE")
                .context("contained seat missing")?
                .clone(),
            "--expect-context-sha256".into(),
            crate::codex_tool_env::context_digest(|k| env.get(k).map(Into::into)),
        ]);
        let mut argv = vec![program.to_string_lossy().into_owned()];
        argv.extend(args);
        let cfg = serde_json::json!({"grant":grant,"ttl":ttl,"mode":"normal","port":port,"allowed":allowed,
            "state":state.path(),"policy_sha":sha,"bwrap":profile.args(&root),"preflight":preflight,"argv":argv});
        Self::from_config(profile, state, sha, cfg, env, &root)
    }
    fn from_config(
        profile: WriteProfile,
        state: tempfile::TempDir,
        sha: String,
        cfg: serde_json::Value,
        mut env: BTreeMap<String, String>,
        root: &Path,
    ) -> Result<Self> {
        // bwrap --chdir sets PWD. Admit that exact name/value at the outer
        // boundary too, so helper, bwrap and shim have identical environments.
        env.insert("PWD".into(), root.to_string_lossy().into_owned());
        let (reader, writer) = pipe()?;
        let (accept_reader, accept_writer) = pipe()?;
        let source = format!("{COMMON}\n{LANE}");
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-I","-S","-c"]).arg(format!("{COMMON}\n{LANE}\n{HOST}\nSOURCE = sys.argv[1]\ntry:\n    sys.exit(host_contained(json.loads(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])))\nexcept Exception:\n    print('contained launch refused (namespace, broker, or requested-seat preflight)', file=sys.stderr)\n    sys.exit(1)"))
            .arg(source).arg(serde_json::to_string(&cfg)?).arg(writer.as_raw_fd().to_string())
            .arg(accept_reader.as_raw_fd().to_string()).env_clear().envs(env).current_dir(&root);
        Ok(Self {
            command,
            profile,
            _state: state,
            policy_sha: sha,
            reader,
            writer,
            accept_reader,
            accept_writer,
        })
    }
    pub(crate) fn spawn(mut self, stdout: Stdio) -> Result<ContainedChild> {
        let mut fds: Vec<_> = self
            .profile
            .mounts
            .iter()
            .map(|m| m.file.as_raw_fd())
            .collect();
        fds.extend([self.writer.as_raw_fd(), self.accept_reader.as_raw_fd()]);
        let parent = std::process::id();
        unsafe {
            self.command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0
                    || libc::getppid() != parent as i32
                {
                    return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
                }
                if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 4u32) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for fd in &fds {
                    if libc::fcntl(*fd, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let child = self
            .command
            .stdin(Stdio::null())
            .stdout(stdout)
            .spawn()
            .context("contained host helper unavailable")?;
        let mut run = ContainedChild {
            child,
            acceptance: None,
            receipt: None,
            _state: self._state,
        };
        drop((self.writer, self.accept_reader));
        unsafe {
            libc::fcntl(self.reader.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
        }
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            match self.reader.read(&mut chunk) {
                Ok(0) => anyhow::bail!("contained helper refused before receipt"),
                Ok(n) => {
                    bytes.extend_from_slice(&chunk[..n]);
                    anyhow::ensure!(bytes.len() <= 8192, "oversized receipt");
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
            if bytes.ends_with(b"\n") {
                break;
            }
            anyhow::ensure!(Instant::now() < deadline, "contained receipt deadline");
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut receipt: ContainmentReceipt =
            serde_json::from_slice(&bytes).context("invalid containment receipt")?;
        receipt.validate(&self.policy_sha)?;
        receipt.accepted = true;
        run.receipt = Some(receipt);
        run.acceptance = Some(self.accept_writer);
        Ok(run)
    }
}
trait PolicyVendor {
    fn pointer_vendor(&self) -> Option<&str>;
}
impl PolicyVendor for toml::Value {
    fn pointer_vendor(&self) -> Option<&str> {
        self.get("contained")?
            .get("vendors")?
            .get("codex")?
            .as_str()
    }
}
pub(crate) struct ContainedChild {
    pub(crate) child: Child,
    receipt: Option<ContainmentReceipt>,
    acceptance: Option<File>,
    _state: tempfile::TempDir,
}
impl ContainedChild {
    /// Caller finishes host registry/lease binding while exec is still blocked.
    pub(crate) fn release(&mut self) -> Result<()> {
        self.acceptance
            .take()
            .context("contained exec barrier already released")?
            .write_all(b"R")?;
        Ok(())
    }
    pub(crate) fn receipt(&self) -> &ContainmentReceipt {
        self.receipt.as_ref().expect("accepted receipt")
    }
}
impl Drop for ContainedChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// trace:TASK-2708 | ai:claude — under tests/, which the TASK-1752 grant-env
// ratchet skips; the #[cfg(test)] gate is what keeps that skip honest.
#[cfg(test)]
#[path = "tests/launch_tests.rs"]
mod tests;
