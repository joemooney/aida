//! The one AIDA contained boundary (headless and interactive). Vendor
//! differences are rows of [`CONTAINED_VENDORS`] plus each vendor's argv
//! finisher; credentials exist only in the exact child env (API key) or as a
//! read-only credentials leaf (Claude and brokered Codex ChatGPT).
// trace:TASK-2707 | ai:codex
// trace:TASK-2708 | ai:claude
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[path = "contained_pty.rs"]
pub(crate) mod pty;
#[path = "../store_mutation/mod.rs"]
mod store_mutation;

#[path = "codex_auth.rs"]
pub(crate) mod codex_auth;

// trace:TASK-3219 | ai:codex
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchIo {
    Headless,
    Interactive,
}

const COMMON: &str = include_str!("../containment/egress_probe.py");
const LANE: &str = include_str!("../containment/contained_lane.py");
const HOST: &str = include_str!("../containment/contained_host.py");
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
    "AIDA_REVIEW_VERDICT_FILE",
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mutation_epoch: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    mutation_ready: bool,
}
impl ContainmentReceipt {
    pub(crate) fn is_accepted(&self) -> bool {
        self.accepted
    }
    pub(crate) fn requires_store_service(&self) -> bool {
        self.mutation_epoch.is_some()
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

/// Outer-only permission argv is available only for an explicitly shipped
/// descriptor and host-accepted evidence. Serialized receipts cannot grant it.
/// This does not admit a vendor or release the contained exec barrier.
// trace:TASK-3048 | ai:codex
pub(crate) fn require_outer_only_receipt(
    vendor: &str,
    receipt: Option<&ContainmentReceipt>,
) -> Result<()> {
    anyhow::ensure!(
        crate::containment::native_layer(vendor)? == crate::containment::NativeLayer::None,
        "{vendor} is not an approved outer-only vendor; refusing permission bypass"
    );
    anyhow::ensure!(
        receipt.is_some_and(ContainmentReceipt::is_accepted),
        "{vendor} outer-only permission bypass requires an applied AIDA ContainmentReceipt; refusing"
    );
    Ok(())
}

struct Mount {
    /// Anonymous content must use bwrap data delivery, not pathname resolution.
    data: bool,
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
    /// Seal after every ordinary grant, including seat-clone and cache grants.
    /// Pin resolved targets as well as protecting their writable ancestors:
    /// removing grants alone would leave primary-checkout launches writable.
    /// Returns a fresh cache location for explicitly routed derived readers;
    /// it is never mounted over the shared cache or populated from its bytes.
    // trace:TASK-3223 | ai:codex
    fn seal_mediated_store(&mut self, root: &Path, main: &Path, state: &Path) -> Result<PathBuf> {
        let mut protected = Vec::new();
        for base in [root, main] {
            for path in [
                base.join(".aida-store"),
                base.join(".aida"),
                aida_core::db::cache_dir_in(&base.join(".aida")),
                base.join(".git"),
            ] {
                // Absence is not safe under a writable ancestor: require the
                // host to provision mountpoints before admitting this profile.
                let path = std::fs::canonicalize(&path).with_context(|| {
                    format!("mediated protected path unavailable: {}", path.display())
                })?;
                if !protected.contains(&path) {
                    protected.push(path);
                }
            }
            let gitdir = aida_core::git_ops::worktree_gitdir_path(base);
            anyhow::ensure!(
                !base.join(".git").is_file() || gitdir.is_some(),
                "mediated Git metadata target unavailable"
            );
            if let Some(gitdir) = gitdir {
                let gitdir = std::fs::canonicalize(gitdir)?;
                if !protected.contains(&gitdir) {
                    protected.push(gitdir);
                }
            }
        }
        let state = std::fs::canonicalize(state)?;
        anyhow::ensure!(
            !protected.iter().any(|path| state.starts_with(path)),
            "private cache overlaps protected store state"
        );
        // Parent mounts first; otherwise rebinding a parent hides a previously
        // protected child mount (notably a cache symlink's external target).
        protected.sort_by_key(|path| path.components().count());
        for path in protected {
            self.add(&path, false)?;
        }
        let cache = state.join("derived-cache");
        std::fs::create_dir(&cache)?;
        self.add(&cache, true)?;
        Ok(cache.join(aida_core::db::CACHE_FILE_NAME))
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
            data: false,
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
                if m.data {
                    "--ro-bind-data"
                } else if m.writable {
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
/// The shared relay accepts exact DNS names only, whether from trusted project
/// config or shipped vendor inventory. No IP literals, globs or URL syntax.
// trace:TASK-3048 | ai:codex
pub(crate) fn validate_allowed_host(host: &str) -> Result<()> {
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
    Ok(())
}

/// Install a private writable runtime at the descriptor's conventional path.
/// The whole host vendor parent was masked first; only this runtime is reopened.
/// Credentials are bound separately and never copied into writable state.
// trace:TASK-3048 | ai:codex
fn install_private_runtime(
    profile: &mut WriteProfile,
    state: &Path,
    home: &Path,
    policy: &crate::containment::RuntimePolicy,
    workspace: &Path,
) -> Result<PathBuf> {
    policy.validate()?;
    let destination = home.join(&policy.relative);
    anyhow::ensure!(
        std::fs::canonicalize(&destination)? == destination,
        "private runtime destination must have no symlink components"
    );
    let source = state.join("vendor-runtime");
    ensure_dir(&source)?;
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o700))?;
    // Private settings inherit no host plugins, MCP or hooks.
    std::fs::write(
        source.join("settings.json"),
        serde_json::to_vec(&policy.settings(workspace)?)?,
    )?;
    std::fs::write(source.join(&policy.credential), "")?;
    profile.add(&source, true)?;
    profile
        .mounts
        .last_mut()
        .context("runtime mount missing")?
        .path = destination.clone();
    Ok(destination)
}

// trace:TASK-3048 | ai:codex
fn bind_runtime_credential(
    profile: &mut WriteProfile,
    runtime: &Path,
    source: &Path,
    leaf: &str,
    owner: u32,
) -> Result<()> {
    let file = open_credentials_leaf_for_owner(source, owner)
        .context("outer-only runtime credential refused")?;
    profile.mounts.push(Mount {
        data: false,
        file,
        path: runtime.join(leaf),
        writable: false,
    });
    Ok(())
}

/// Only the common wrapper may prepare argv before receipt acceptance; it
/// cannot release exec until spawn accepts a live receipt and checks the row.
// trace:TASK-3048 | ai:codex
fn wrapped_outer_only_argv(
    policy: &crate::containment::RuntimePolicy,
    runtime: &Path,
    prompt: &str,
) -> Result<Vec<String>> {
    policy.validate()?;
    anyhow::ensure!(
        crate::contained_boundary::root_rule_path(runtime).is_some(),
        "outer-only vendor requires an exact absolute private runtime path"
    );
    let mut args = policy.argv_prefix.clone();
    args.extend([
        "--settings".into(),
        runtime.join("settings.json").to_string_lossy().into(),
        "-p".into(),
        prompt.into(),
    ]);
    Ok(args)
}

/// Candidate argv outside the wrapper always requires already-applied evidence.
// trace:TASK-3048 | ai:codex
pub(crate) fn outer_only_argv(
    vendor: &str,
    runtime: &Path,
    prompt: &str,
    receipt: Option<&ContainmentReceipt>,
) -> Result<Vec<String>> {
    require_outer_only_receipt(vendor, receipt)?;
    let policy = crate::containment::runtime_policy(vendor)?;
    wrapped_outer_only_argv(&policy, runtime, prompt)
}

/// How a contained vendor authenticates. Each kind is one fixed, reviewed
/// mechanism; a vendor row picks one, it never supplies its own.
// trace:TASK-2708 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VendorAuth {
    /// An API key in the worker's own exact environment (Codex, P3 pending).
    OpenAiApiKey,
    /// The Claude OAuth credentials leaf, bound read-only (C7, 05:47 ruling).
    ClaudeCredentialsFile,
    /// A descriptor-selected opaque read-only leaf in a private runtime.
    // trace:TASK-3048 | ai:codex
    RuntimeCredentialsFile,
}

/// One vendor's data for the shared contained boundary (TASK-2708 AC1:
/// "vendor additions use data, not new containment code"). The mounts,
/// masks, filter, broker, relay, receipt and seat probe are common; a row
/// names only the vendor's program/policy key, where its private home and
/// tmp are pointed, fixed env, and which auth mechanism it uses.
// trace:TASK-2708 | ai:claude
#[derive(Debug)]
pub(crate) struct ContainedVendor {
    /// The program basename and its `[contained.vendors]` key.
    pub(crate) key: &'static str,
    label: &'static str,
    /// Env names pointed at the private vendor home inside the lane state.
    private_home_env: &'static [&'static str],
    /// Env names pointed at the lane tmp inside the state root (`TMPDIR`
    /// otherwise stays the lane's private `/tmp` tmpfs).
    private_tmp_env: &'static [&'static str],
    fixed_env: &'static [(&'static str, &'static str)],
    auth: VendorAuth,
    /// Probe support for a vendor that creates its own nested sandbox.
    // trace:TASK-2708 | ai:codex
    pub(crate) nested_userns: bool,
}

pub(crate) const CONTAINED_VENDORS: &[ContainedVendor] = &[
    ContainedVendor {
        key: "codex",
        label: "Codex",
        private_home_env: &["CODEX_HOME", "CODEX_SQLITE_HOME"],
        private_tmp_env: &[],
        fixed_env: &[],
        auth: VendorAuth::OpenAiApiKey,
        nested_userns: false,
    },
    // Ruling item 5 / C7: private writable CLAUDE_CONFIG_DIR, TMPDIR and
    // CLAUDE_CODE_TMPDIR inside the lane state, no self-update.
    ContainedVendor {
        key: "claude",
        label: "Claude",
        private_home_env: &["CLAUDE_CONFIG_DIR"],
        private_tmp_env: &["TMPDIR", "CLAUDE_CODE_TMPDIR"],
        fixed_env: &[("DISABLE_AUTOUPDATER", "1")],
        auth: VendorAuth::ClaudeCredentialsFile,
        nested_userns: true,
    },
    // Outer-only vendors use the same mounts, network broker and exec barrier.
    // Admission remains closed until the inventory and all-tool proof are complete.
    // trace:TASK-3048 | ai:codex
    ContainedVendor {
        key: "agy",
        label: "AGY",
        private_home_env: &[],
        private_tmp_env: &["TMPDIR"],
        fixed_env: &[],
        auth: VendorAuth::RuntimeCredentialsFile,
        nested_userns: false,
    },
];

impl ContainedVendor {
    /// The row for a launch child's program (by basename), or a named refusal.
    pub(crate) fn for_program(program: &std::ffi::OsStr) -> Result<&'static Self> {
        let name = Path::new(program)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        CONTAINED_VENDORS
            .iter()
            .find(|v| v.key == name)
            .with_context(|| format!("`{name}` has no AIDA contained launch profile"))
    }
}

enum Auth {
    Env(&'static str, String),
    File(PathBuf),
    RuntimeFile(PathBuf),
    ChatGpt(codex_auth::Credential),
}

/// The OAuth credentials leaf inside a Claude config dir.
pub(crate) const CLAUDE_CREDENTIALS: &str = ".credentials.json";

/// The named refusal when the RO credentials expire inside a lane (05:47).
pub(crate) const CLAUDE_EXPIRED_IN_LANE: &str =
    "claude credentials expired inside a contained lane; relaunch";

/// Seconds a baseline (grant-less) Claude lane must still have on its token.
const CLAUDE_BASELINE_LIFETIME_SECS: i64 = 3600;

/// Read the expiry from the exact leaf descriptor later bound into the lane.
// trace:TASK-2708 | ai:codex
fn claude_credentials_expiry_ms(file: &File) -> Result<i64> {
    use std::os::unix::fs::FileExt;
    let mut bytes = vec![0; 65537];
    let n = file
        .read_at(&mut bytes, 0)
        .context("CLAUDE_CREDENTIALS_UNREADABLE")?;
    anyhow::ensure!(n <= 65536, "CLAUDE_CREDENTIALS_INVALID: file exceeds bound");
    let value: serde_json::Value =
        serde_json::from_slice(&bytes[..n]).context("CLAUDE_CREDENTIALS_INVALID: not JSON")?;
    value
        .pointer("/claudeAiOauth/expiresAt")
        .and_then(serde_json::Value::as_i64)
        .context("CLAUDE_CREDENTIALS_INVALID: missing claudeAiOauth.expiresAt")
}

// No evidence establishes that `claude auth status` refreshes OAuth. Refuse
// immediately; only the operator's login may replace host credentials.
// trace:TASK-2708 | ai:codex
fn ensure_claude_credentials_outlive(expires_at: i64, ttl_secs: i64) -> Result<()> {
    let needed = ttl_secs.max(CLAUDE_BASELINE_LIFETIME_SECS);
    let left = expires_at.saturating_sub(chrono::Utc::now().timestamp_millis()) / 1000;
    anyhow::ensure!(left >= needed,
        "CLAUDE_EXPIRED: credentials expire in {left}s, before this lane's {needed}s grant; operator runs `claude /login` on the host, then relaunch");
    Ok(())
}

/// The host credentials leaf, pinned by descriptor for a read-only bind: a
/// regular, single-link file owned by this user, never a symlink.
// trace:TASK-2708 | ai:claude
#[cfg(test)]
fn open_credentials_leaf(credentials: &Path) -> Result<File> {
    open_credentials_leaf_for_owner(credentials, unsafe { libc::getuid() })
}
fn open_credentials_leaf_for_owner(credentials: &Path, owner: u32) -> Result<File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(credentials)
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::ELOOP) {
                anyhow::anyhow!("Claude credentials identity refused: symlink")
            } else {
                anyhow::Error::new(error)
                    .context("CLAUDE_CREDENTIALS_UNREADABLE: Claude credentials unavailable")
            }
        })?;
    let md = file.metadata()?;
    anyhow::ensure!(
        md.is_file() && md.nlink() == 1 && md.uid() == owner,
        "Claude credentials identity refused"
    );
    Ok(file)
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
) -> Result<Vec<PathBuf>> {
    let mut masks: Vec<PathBuf> = Vec::new();
    let candidates = [
        Some(host_home.join(".codex")),
        Some(host_home.join(".claude")),
        Some(host_home.join(".claude.json")),
        Some(host_home.join(".claude.json.backup")),
        Some(codex_home.to_path_buf()),
        sqlite_home.map(Path::to_path_buf),
    ];
    let candidates = candidates
        .into_iter()
        .flatten()
        .chain(crate::containment::runtime_host_parents(host_home)?);
    for p in candidates {
        let Ok(canonical) = std::fs::canonicalize(&p) else {
            continue;
        };
        if !masks.contains(&canonical) {
            masks.push(canonical);
        }
    }
    Ok(masks)
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

// trace:TASK-3225 | ai:codex
pub(crate) fn require_store_service_surface(child: &Command, wrapped: bool) -> Result<()> {
    anyhow::ensure!(
        wrapped
            || (explicit(child, "AIDA_STORE_MUTATION_REQUIRED").is_none()
                && explicit(child, "AIDA_STORE_MUTATION_PROFILE").is_none()),
        "required store service is unavailable outside the shared contained headless launcher"
    );
    Ok(())
}

/// An explicit child selection is input, not admission. Do not infer this
/// profile from the read-only S1 service requirement or inherited environment.
// trace:TASK-3223 | ai:codex
fn selected_store_profile(child: &Command) -> Result<crate::contained_boundary::StoreWriteProfile> {
    use crate::contained_boundary::StoreWriteProfile;
    match explicit(child, "AIDA_STORE_MUTATION_PROFILE").as_deref() {
        None | Some("compatibility") => Ok(StoreWriteProfile::Compatibility),
        Some("mediated") => anyhow::bail!(
            "mediated store profile is gated pending CLI/MCP routing, durable publication and boundary validation"
        ),
        Some(_) => anyhow::bail!("invalid explicit store mutation profile"),
    }
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
    /// Expiry captured from the exact read-only bound Claude leaf.
    credentials: Option<i64>,
    /// Absolute ChatGPT lane deadline in Unix seconds, shared with the supervisor.
    chatgpt_deadline: Option<i64>,
    /// Unlinked host-only evidence; never mounted or passed to the lane.
    oauth_denial: File,
    outer_only: Option<&'static str>,
    mutation: Option<store_mutation::Session>,
    io: LaunchIo,
}
impl PreparedLaunch {
    /// Takes finalized child assignments only. Never inherits ambient authority.
    /// `host_main` is the launcher's main checkout, resolved once at the
    /// launch entry point (TASK-2032); `None` recognises no seat clone.
    // trace:TASK-2708 TASK-2032 | ai:claude
    pub(crate) fn prepare(
        child: &mut Command,
        root: &Path,
        host_main: Option<&Path>,
    ) -> Result<Self> {
        Self::prepare_for_owner(child, root, host_main, unsafe { libc::getuid() })
    }
    fn prepare_for_owner(
        child: &mut Command,
        root: &Path,
        host_main: Option<&Path>,
        credential_owner: u32,
    ) -> Result<Self> {
        Self::prepare_with_io(
            child,
            root,
            host_main,
            credential_owner,
            LaunchIo::Headless,
            codex_auth::PROGRAM_SHA256,
        )
    }
    pub(crate) fn prepare_interactive(
        child: &mut Command,
        root: &Path,
        host_main: Option<&Path>,
    ) -> Result<Self> {
        Self::prepare_with_io(
            child,
            root,
            host_main,
            unsafe { libc::getuid() },
            LaunchIo::Interactive,
            codex_auth::PROGRAM_SHA256,
        )
    }
    // trace:STORY-2593 | ai:codex — inject the binary pin while keeping
    // ambient reads under their existing ExecContext ledger owner.
    fn prepare_with_io(
        child: &mut Command,
        root: &Path,
        host_main: Option<&Path>,
        credential_owner: u32,
        io: LaunchIo,
        codex_sha256: &str,
    ) -> Result<Self> {
        // Projection turns ambient values into explicit pairs, which this
        // wrapper would mistake for site decisions. All wrapper entrypoints
        // must receive an unprojected command, including future launch sites.
        // trace:STORY-2326 | ai:codex
        anyhow::ensure!(
            !super::profile::environment_is_cleared(child),
            "contained wrapper requires an unprojected command environment"
        );
        let store_profile = selected_store_profile(child)?;
        // trace:TASK-2708 | ai:claude — the vendor is data selected by the
        // child's program; everything below is the one shared boundary.
        let required = explicit(child, "AIDA_STORE_MUTATION_REQUIRED");
        anyhow::ensure!(
            required.is_none() || required.as_deref() == Some("1"),
            "invalid explicit store service selection"
        );
        let mutation = if required.is_some() {
            Some(store_mutation::Session::prepare(
                child,
                &std::fs::canonicalize(root)?,
            )?)
        } else {
            None
        };
        let vendor = ContainedVendor::for_program(child.get_program())?;
        let label = vendor.label;
        anyhow::ensure!(
            io == LaunchIo::Headless || vendor.key == "claude",
            "interactive wrapper supports Claude only"
        );

        // Reject widening/resume/daemon flags before private state or broker setup.
        // trace:TASK-3219 | ai:codex
        if io == LaunchIo::Interactive {
            let args = child
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            claude_argv(&args, 1, &[])?;
        }
        anyhow::ensure!(
            io == LaunchIo::Interactive || explicit(child, "AIDA_HEADLESS").as_deref() == Some("1"),
            "{label} AIDA posture currently supports foreground headless launches only (PR2 pending)"
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
            policy.pointer_vendor(vendor.key) == Some("aida"),
            "{label} AIDA posture requires trusted policy"
        );
        let mut allowed = policy
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
                "contained {label} strict read allowlist is not supported by this launch slice"
            );
        }
        for host in &allowed {
            validate_allowed_host(host)?;
        }
        // Vendor data never silently expands the trusted egress policy.
        // Refuse incomplete inventories before state creation or credentials.
        // trace:TASK-3048 | ai:codex
        let runtime_policy = if vendor.auth == VendorAuth::RuntimeCredentialsFile {
            let policy = crate::containment::runtime_policy(vendor.key)?;
            policy.require_auth_policy(&allowed)?;
            Some(policy)
        } else {
            None
        };
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
        let claude_home = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| host_home.join(".claude"));
        let grant = explicit(child, crate::gate::seat_authority::GRANT_ENV)
            .context("contained child requires explicit grant decision")?;
        let ttl = if grant.is_empty() {
            0
        } else {
            (crate::gate::seat_authority::read_grant(&grant)?.expires_at - chrono::Utc::now())
                .num_seconds()
                .max(1)
        };
        // Retain the checked path so verification and execution use one leaf.
        // trace:BUG-2484 | ai:codex
        let mut pinned_program = None;
        // Credentials resolve BEFORE any mount or state: a missing or
        // unusable one is a named refusal, never a lane without it.
        let auth = match vendor.auth {
            VendorAuth::OpenAiApiKey => {
                // trace:STORY-2593 | ai:codex — pin the probed executable;
                // never execute vendor code on the host to obtain a version.
                match std::fs::symlink_metadata(codex_home.join("auth.json")) {
                    Ok(_) => {
                        let program = resolve_program(child, &selected_environment(child))?;
                        let program = codex_auth::resolve_native_program(&program)?;
                        codex_auth::require_program(&program, codex_sha256)?;
                        pinned_program = Some(program);
                        let auth = codex_auth::prepare(&codex_home, credential_owner, ttl)?;
                        codex_auth::exclude_oauth_hosts(&mut allowed);
                        Auth::ChatGpt(auth)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        let key = std::env::var("OPENAI_API_KEY").map_err(|_| anyhow::anyhow!(
                            "contained Codex requires file ChatGPT login or OPENAI_API_KEY; keyring logins unsupported"))?;
                        anyhow::ensure!(!key.is_empty(), "contained Codex requires OPENAI_API_KEY");
                        Auth::Env("OPENAI_API_KEY", key)
                    }
                    Err(_) => anyhow::bail!("ChatGPT credentials unavailable"),
                }
            }
            VendorAuth::ClaudeCredentialsFile => {
                let credentials = claude_home.join(CLAUDE_CREDENTIALS);
                Auth::File(credentials)
            }
            VendorAuth::RuntimeCredentialsFile => {
                let policy = runtime_policy.as_ref().context("runtime policy missing")?;
                Auth::RuntimeFile(host_home.join(&policy.relative).join(&policy.credential))
            }
        };
        let mut profile = WriteProfile::new();
        profile.add(&root, true)?;
        let main = crate::main_worktree_root_from(&root);
        crate::session::verify_linked_worktree_main(host_main, &root, &main)?;
        for p in crate::contained_boundary::contained_store_write_paths_for(&main, store_profile) {
            let p =
                std::fs::canonicalize(p).context("required store/cache directory unavailable")?;
            profile.add(&p, runtime_policy.is_none())?;
        }
        if let Some(clone) = crate::seat_clone::recorded_clone_for(host_main, &root) {
            for path in crate::seat_clone::seat_extra_write_paths(&clone) {
                profile.add(&path, true)?;
            }
        }
        profile.add(state.path(), true)?;
        // Mask host vendor credentials/daemon endpoints with launch-owned empty RO views.
        let empty = state.path().join("empty");
        ensure_dir(&empty)?;
        // bwrap needs the nested runtime mountpoint inside the masked vendor
        // parent. Use a dedicated view so other vendor masks stay empty.
        // trace:TASK-3048 | ai:codex
        let runtime_parent_mask = if let Some(policy) = &runtime_policy {
            let view = state.path().join("runtime-parent-mask");
            let nested: PathBuf = Path::new(&policy.relative).components().skip(1).collect();
            ensure_dir(&view.join(nested))?;
            let parent = Path::new(&policy.relative)
                .components()
                .next()
                .context("runtime parent missing")?;
            Some((std::fs::canonicalize(host_home.join(parent))?, view))
        } else {
            None
        };
        // trace:TASK-2708 | ai:claude — including a relocated CODEX_HOME /
        // CODEX_SQLITE_HOME, which the default-path masks alone would leave
        // readable inside the sandbox.
        let sqlite_home = std::env::var_os("CODEX_SQLITE_HOME").map(PathBuf::from);
        let mut masks = host_vendor_state_masks(&host_home, &codex_home, sqlite_home.as_deref())?;
        // trace:TASK-2708 | ai:claude — and a relocated CLAUDE_CONFIG_DIR.
        if let Ok(relocated) = std::fs::canonicalize(&claude_home) {
            if !masks.contains(&relocated) {
                masks.push(relocated);
            }
        }
        let empty_file = state.path().join("empty-file");
        std::fs::write(&empty_file, "")?;
        for p in masks {
            let view = if p.is_dir() {
                runtime_parent_mask
                    .as_ref()
                    .filter(|(parent, _)| parent == &p)
                    .map_or(&empty, |(_, view)| view)
            } else {
                &empty_file
            };
            let file = File::open(view)?;
            profile.mounts.push(Mount {
                data: false,
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
                if base == &root
                    && runtime_policy.is_none()
                    && crate::seat_clone::recorded_clone_for(host_main, &root).is_some()
                {
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
        profile.add(&std::fs::canonicalize(cache)?, runtime_policy.is_none())?;
        if !grant.is_empty() {
            let notices = crate::grant_expiry_notice::notice_leaf(&grant)?;
            ensure_dir(&notices)?;
            profile.add(&notices, true)?;
        }
        if store_profile == crate::contained_boundary::StoreWriteProfile::Mediated {
            // Admission above remains closed until callers use this explicit
            // cache path and canonical verbs provide durable publication.
            let _cache = profile.seal_mediated_store(&root, &main, state.path())?;
        }
        // HOME remains the issuer's canonical AIDA home (read-only). Core
        // tool filtering retains HOME but not AIDA_HOME; relocating HOME would
        // silently redirect grant lookup. Vendor mutable state is separate:
        // a private vendor home inside the lane state (rw with the state).
        let private_home = if let Some(policy) = &runtime_policy {
            // The vendor resolves this conventional path itself; mount only a
            // launch-owned runtime there, never the host settings/plugins/logs.
            install_private_runtime(&mut profile, state.path(), &host_home, policy, &root)?
        } else {
            let path = state.path().join(vendor.key);
            ensure_dir(&path)?;
            path
        };
        let runtime = state.path().join("runtime");
        ensure_dir(&runtime)?;
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))?;
        // trace:TASK-2708 | ai:claude — a lane tmp inside the state root, so
        // a vendor's own bridge sockets there are private-endpoint connects.
        let lane_tmp = state.path().join("tmp");
        ensure_dir(&lane_tmp)?;
        std::fs::set_permissions(&lane_tmp, std::fs::Permissions::from_mode(0o700))?;
        let mut codex_deadline = None;
        let credentials = match &auth {
            Auth::ChatGpt(_) => None,
            Auth::Env(..) => {
                let config = private_home.join("config.toml");
                std::fs::write(&config, profile.native())?;
                profile.add(&config, false)?;
                None
            }
            // C7 (05:47 ruling): the credentials leaf is bound READ-ONLY into
            // the private config dir; host ~/.claude is masked above, and no
            // writable credential root exists in the lane.
            Auth::File(credentials) => {
                let file = open_credentials_leaf_for_owner(credentials, credential_owner)?;
                let expires_at = claude_credentials_expiry_ms(&file)?;
                ensure_claude_credentials_outlive(expires_at, ttl)?;
                let leaf = private_home.join(CLAUDE_CREDENTIALS);
                std::fs::write(&leaf, "")?;
                profile.mounts.push(Mount {
                    data: false,
                    file: file.try_clone()?,
                    path: leaf,
                    writable: false,
                });
                Some(expires_at)
            }
            Auth::RuntimeFile(credentials) => {
                let policy = runtime_policy.as_ref().context("runtime policy missing")?;
                bind_runtime_credential(
                    &mut profile,
                    &private_home,
                    credentials,
                    &policy.credential,
                    credential_owner,
                )?;
                None
            }
        };
        // Transfer the sealed leaf without a disk-backed secret copy.
        let auth = match auth {
            Auth::ChatGpt(auth) => {
                codex_deadline = Some(codex_auth::bind(&mut profile, &private_home, auth)?);
                None
            }
            other => Some(other),
        };
        let mut env = selected_environment(child);
        // A ChatGPT lane must not inherit an alternate API credential.
        env.remove("OPENAI_API_KEY");
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
        if let Some(clone) = crate::seat_clone::recorded_clone_for(host_main, &root) {
            for (name, value) in crate::seat_clone::seat_build_env(&clone) {
                env.insert(name.into(), value);
            }
        }
        for (name, value) in [
            ("HOME", authority_home.to_string_lossy().into_owned()),
            ("XDG_RUNTIME_DIR", runtime.to_string_lossy().into_owned()),
            ("TMPDIR", "/tmp".into()),
            ("LANG", "C.UTF-8".into()),
            (
                "PATH",
                explicit(child, "PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            ),
            ("AIDA_HOME", authority_home.to_string_lossy().into_owned()),
            ("AIDA_CONTAINED", "1".into()),
            (
                "AIDA_HEADLESS",
                if io == LaunchIo::Headless { "1" } else { "0" }.into(),
            ),
            ("AIDA_CONTAINMENT_POSTURE", "aida".into()),
        ] {
            env.insert(name.into(), value);
        }
        if io == LaunchIo::Interactive {
            env.insert("TERM".into(), "xterm-256color".into());
            // Only finalized assignments from this generation, never ambient rotation state.
            for name in [
                crate::agent_rotation::GENERATION_ENV,
                crate::agent_rotation::HANDOFF_ENV,
                crate::agent_rotation::PREVIOUS_SESSION_ENV,
                crate::agent_rotation::HANDOFF_STAMP_ENV,
                crate::agent_rotation::SEAT_ENV,
            ] {
                if let Some(value) = explicit(child, name) {
                    env.insert(name.into(), value);
                }
            }
        }
        // trace:TASK-2708 | ai:claude — the vendor's data rows.
        for name in vendor.private_home_env {
            env.insert((*name).into(), private_home.to_string_lossy().into_owned());
        }
        for name in vendor.private_tmp_env {
            env.insert((*name).into(), lane_tmp.to_string_lossy().into_owned());
        }
        for (name, value) in vendor.fixed_env {
            env.insert((*name).into(), (*value).into());
        }
        if let Some(Auth::Env(name, key)) = auth {
            env.insert(name.into(), key);
        }
        // Select an ephemeral number; the listener binds it only in the new
        // network namespace. No persistent host port is assigned.
        let reservation = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let port = reservation.local_addr()?.port();
        drop(reservation);
        env.extend(relay_environment(port));
        if let Some(service) = &mutation {
            env.insert(
                "AIDA_STORE_MUTATION_ENDPOINT".into(),
                service.endpoint.clone(),
            );
        }
        // API-key lanes share npm discovery, while only ChatGPT requires the pin.
        // trace:BUG-2484 | ai:codex
        let program = match pinned_program {
            Some(program) => program,
            None => {
                let program = resolve_program(child, &env)?;
                if vendor.auth == VendorAuth::OpenAiApiKey {
                    codex_auth::resolve_native_program(&program)?
                } else {
                    program
                }
            }
        };
        let original: Vec<_> = child
            .get_args()
            .map(|v| v.to_string_lossy().into_owned())
            .collect();
        // trace:TASK-2708 | ai:claude — the vendor finishes only its own argv;
        // the requested-seat probe that follows is shared.
        let (args, mut preflight) = match vendor.auth {
            VendorAuth::OpenAiApiKey => codex_argv(&program, &env, &original)?,
            VendorAuth::ClaudeCredentialsFile => {
                (claude_argv(&original, port, &allowed)?, Vec::new())
            }
            VendorAuth::RuntimeCredentialsFile => {
                let policy = runtime_policy.as_ref().context("runtime policy missing")?;
                (runtime_argv(policy, &private_home, &original)?, Vec::new())
            }
        };
        preflight.extend([
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
            "state":state.path(),"policy_sha":sha,"bwrap":profile.args(&root),"preflight":preflight,"nested_userns":vendor.nested_userns,"argv":argv});
        let mut cfg = cfg;
        if let Some(deadline) = codex_deadline {
            cfg["chatgpt_deadline"] = deadline.into();
            cfg["oauth_hosts"] = serde_json::json!(codex_auth::OAUTH_HOSTS);
        }
        if let Some(service) = &mutation {
            cfg["mutation"] = service.config.clone();
        }
        let mut prepared = Self::from_config(profile, state, sha, cfg, env, &root)?;
        prepared.mutation = mutation;
        prepared.credentials = credentials;
        prepared.chatgpt_deadline = codex_deadline;
        prepared.outer_only = runtime_policy.map(|_| vendor.key);
        prepared.io = io;
        Ok(prepared)
    }
}

/// Outer-only runtime argv accepts only a terminal prompt from the site.
/// Keep this adapter separate from the Claude settings adapter: session
/// options cannot smuggle settings, MCP servers or a host daemon.
// trace:TASK-3048 | ai:codex
fn runtime_argv(
    policy: &crate::containment::RuntimePolicy,
    runtime: &Path,
    original: &[String],
) -> Result<Vec<String>> {
    anyhow::ensure!(
        original.len() == 2 && original[0] == "-p",
        "outer-only runtime requires exactly -p <prompt>; extra options refused"
    );
    wrapped_outer_only_argv(policy, runtime, &original[1])
}

/// Codex's argv: the tool-env and exec-compatible profile config, then the site's
/// argv minus anything that would replace the generated profile. Its seat
/// probe runs through Codex's own sandbox as well (`codex sandbox ... --`).
// trace:TASK-2707 TASK-2708 | ai:claude
fn codex_argv(
    program: &Path,
    env: &BTreeMap<String, String>,
    original: &[String],
) -> Result<(Vec<String>, Vec<String>)> {
    let mut final_child = Command::new(program);
    final_child.env_clear().envs(env);
    let mut args = crate::codex_tool_env::codex_tool_env_overrides(&final_child)?;
    if let Some(endpoint) = env.get("AIDA_STORE_MUTATION_ENDPOINT") {
        args.extend([
            "-c".into(),
            format!(
                "shell_environment_policy.set.AIDA_STORE_MUTATION_ENDPOINT={}",
                toml::Value::String(endpoint.clone())
            ),
        ]);
    }
    for name in TOOL_NET_ENV.iter().chain(TOOL_BUILD_ENV) {
        args.extend([
            "-c".into(),
            format!(
                "shell_environment_policy.set.{name}={}",
                toml::Value::String(env[*name].clone())
            ),
        ]);
    }
    // trace:BUG-2487 | ai:codex — -P belongs only to `codex sandbox`.
    args.extend(["-c".into(), "default_permissions=\"aida\"".into()]);
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
            "contained Codex cannot override its generated native profile with {}",
            original[i]
        );
        args.push(original[i].clone());
        i += 1;
    }
    validate_codex_flags(&args, "exec")?;
    let mut sandbox = crate::codex_tool_env::sandbox_args_for_contained(&args)?;
    // The model-free seat preflight retains the same explicit profile.
    sandbox.extend([
        "-P".into(),
        "aida".into(),
        "--include-managed-config".into(),
    ]);
    validate_codex_flags(&sandbox, "sandbox")?;
    let mut preflight = vec![program.to_string_lossy().into_owned()];
    preflight.extend(sandbox);
    preflight.push("--".into());
    Ok((args, preflight))
}

/// Validate against recorded help from the exact auth pin, without executing
/// lane-selected vendor code on the host. Parse option declarations only;
/// option values and text within the terminal prompt are opaque. The recorded
/// help lives beside this production consumer, not under a tests directory.
// trace:BUG-2487 | ai:codex
fn validate_codex_flags(args: &[String], scope: &str) -> Result<()> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("codex_0166_help.json"))?;
    anyhow::ensure!(
        fixture["sha256"] == codex_auth::PROGRAM_SHA256,
        "contained Codex help fixture differs from executable pin; re-record help before launch"
    );
    let help = fixture[scope]
        .as_str()
        .context("missing pinned Codex help")?;
    let mut options = BTreeMap::new();
    for line in help
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('-'))
    {
        let declaration = line.split("  ").next().unwrap();
        let takes_value = declaration.contains('<');
        for flag in declaration
            .split_whitespace()
            .take_while(|word| !word.starts_with('<'))
        {
            options.insert(flag.trim_end_matches(','), takes_value);
        }
    }
    let mut command_seen = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            break;
        }
        if arg == scope && !command_seen {
            command_seen = true;
            i += 1;
            continue;
        }
        if !arg.starts_with('-') {
            anyhow::ensure!(
                command_seen && scope == "exec" && i + 1 == args.len(),
                "contained Codex requires a fresh exec with one terminal prompt"
            );
            break;
        }
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), false), |(f, _)| (f, true));
        let takes_value = options.get(flag).with_context(|| {
            format!(
                "contained Codex {scope} flag {flag} is unsupported by pinned codex-cli 0.166.0"
            )
        })?;
        anyhow::ensure!(
            !inline || *takes_value,
            "contained Codex {scope} flag {flag} takes no value"
        );
        if *takes_value && !inline {
            anyhow::ensure!(
                i + 1 < args.len(),
                "contained Codex {scope} flag {flag} requires a value"
            );
            i += 1;
        }
        i += 1;
    }
    anyhow::ensure!(command_seen, "contained Codex requires {scope}");
    Ok(())
}

/// Claude's argv: the site's argv with its one generated `--settings`
/// finished for the lane (ruling item 5): Claude's own sandbox on, required,
/// with no unsandboxed escape, its proxy chained to the lane relay
/// (`httpProxyPort`, no SOCKS port) and its domain list the trusted
/// `allowed_hosts`. A flag that would skip permissions, or a second or
/// missing `--settings`, refuses. Its seat probe runs directly under the lane
/// filter: the Bash sandbox wraps tool calls, not the vendor process.
// trace:TASK-2708 | ai:claude
fn claude_argv(original: &[String], port: u16, allowed: &[String]) -> Result<Vec<String>> {
    let mut args = Vec::with_capacity(original.len());
    let mut settings_seen = 0;
    let mut i = 0;
    while i < original.len() {
        let arg = original[i].as_str();
        anyhow::ensure!(
            !matches!(
                arg,
                "--dangerously-skip-permissions" | "--allow-dangerously-skip-permissions"
            ) && !(arg == "--permission-mode"
                && original.get(i + 1).map(String::as_str) == Some("bypassPermissions"))
                && arg != "--permission-mode=bypassPermissions",
            "contained Claude cannot skip its permission checks"
        );
        anyhow::ensure!(
            !arg.starts_with("--settings="),
            "contained Claude settings must be the one generated --settings value"
        );
        anyhow::ensure!(
            arg != "--bg",
            "contained Claude background launch requires an AIDA-owned supervisor"
        );
        // A resumed session lives in a config dir the lane's fresh private
        // one does not hold. trace:TASK-2708 | ai:claude
        anyhow::ensure!(
            !matches!(
                arg,
                "--resume" | "-r" | "--continue" | "-c" | "--fork-session"
            ) && !arg.starts_with("--resume="),
            "contained Claude resume waits for private-state lifecycle support"
        );
        if arg == "--settings" {
            let value = original
                .get(i + 1)
                .context("contained Claude --settings has no value")?;
            settings_seen += 1;
            args.push(original[i].clone());
            args.push(claude_lane_settings(value, port, allowed)?);
            i += 2;
            continue;
        }
        args.push(original[i].clone());
        i += 1;
    }
    anyhow::ensure!(
        settings_seen == 1,
        "contained Claude requires exactly one generated --settings value"
    );
    Ok(args)
}

/// The generated contained settings with the lane's sandbox block applied.
// trace:TASK-2708 | ai:claude
pub(crate) fn claude_lane_settings(json: &str, port: u16, allowed: &[String]) -> Result<String> {
    let mut settings: serde_json::Value =
        serde_json::from_str(json).context("contained Claude settings are not JSON")?;
    let object = settings
        .as_object_mut()
        .context("contained Claude settings must be an object")?;
    let sandbox = object
        .entry("sandbox")
        .or_insert_with(|| serde_json::json!({}));
    let sandbox = sandbox
        .as_object_mut()
        .context("contained Claude sandbox settings must be an object")?;
    sandbox.insert("enabled".into(), true.into());
    sandbox.insert("failIfUnavailable".into(), true.into());
    sandbox.insert("allowUnsandboxedCommands".into(), false.into());
    let network = sandbox
        .entry("network")
        .or_insert_with(|| serde_json::json!({}));
    let network = network
        .as_object_mut()
        .context("contained Claude sandbox network settings must be an object")?;
    network.insert("httpProxyPort".into(), port.into());
    network.remove("socksProxyPort");
    network.insert("allowedDomains".into(), serde_json::json!(allowed));
    Ok(settings.to_string())
}

impl PreparedLaunch {
    fn from_config(
        profile: WriteProfile,
        state: tempfile::TempDir,
        sha: String,
        mut cfg: serde_json::Value,
        mut env: BTreeMap<String, String>,
        root: &Path,
    ) -> Result<Self> {
        // bwrap --chdir sets PWD. Admit that exact name/value at the outer
        // boundary too, so helper, bwrap and shim have identical environments.
        env.insert("PWD".into(), root.to_string_lossy().into_owned());
        // trace:STORY-2593 | ai:codex
        let oauth_denial = tempfile::tempfile()?;
        oauth_denial.write_at(b"0", 0)?;
        if cfg.get("chatgpt_deadline").is_some() {
            cfg["oauth_denial_fd"] = oauth_denial.as_raw_fd().into();
        }
        let (reader, writer) = pipe()?;
        let (accept_reader, accept_writer) = pipe()?;
        let source = format!(
            "{COMMON}\n{LANE}\n{}",
            include_str!("../store_mutation/client.py")
        );
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-I","-S","-c"]).arg(format!("{COMMON}\n{LANE}\n{}\n{HOST}\nSOURCE = sys.argv[1]\ntry:\n    sys.exit(host_contained(json.loads(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])))\nexcept Exception as error:\n    print('contained launch refused: ' + str(error), file=sys.stderr)\n    sys.exit(1)", store_mutation::SOURCE))
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
            credentials: None,
            chatgpt_deadline: None,
            oauth_denial,
            outer_only: None,
            mutation: None,
            io: LaunchIo::Headless,
        })
    }
    pub(crate) fn spawn(self, stdout: Stdio) -> Result<ContainedChild> {
        anyhow::ensure!(
            self.io == LaunchIo::Headless,
            "interactive launch requires a private PTY"
        );
        self.spawn_io(Stdio::null(), stdout, Stdio::inherit())
    }
    pub(crate) fn spawn_interactive(self, pty: &pty::PrivatePty) -> Result<ContainedChild> {
        anyhow::ensure!(
            self.io == LaunchIo::Interactive,
            "headless launch cannot inherit terminal IO"
        );
        let [input, output, error] = pty.stdio()?;
        let result = self.spawn_io(input, output, error);
        if result.is_err() {
            pty.report_refusal();
        }
        result
    }
    fn spawn_io(mut self, stdin: Stdio, stdout: Stdio, stderr: Stdio) -> Result<ContainedChild> {
        let mut fds: Vec<_> = self
            .profile
            .mounts
            .iter()
            .map(|m| m.file.as_raw_fd())
            .collect();
        fds.extend([
            self.writer.as_raw_fd(),
            self.accept_reader.as_raw_fd(),
            self.oauth_denial.as_raw_fd(),
        ]);
        if let Some(service) = &self.mutation {
            fds.push(service.fd());
        }
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
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .context("contained host helper unavailable")?;
        let mut run = ContainedChild {
            child,
            acceptance: None,
            receipt: None,
            credentials: self.credentials,
            chatgpt_deadline: self.chatgpt_deadline,
            oauth_denial: self.oauth_denial,
            mutation: self.mutation,
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
        validate_service_receipt(&receipt, run.mutation.as_ref().map(|s| s.epoch.as_str()))?;
        receipt.accepted = true;
        if let Some(vendor) = self.outer_only {
            require_outer_only_receipt(vendor, Some(&receipt))?;
        }
        // Host-side evidence only: never export the private receipt or grant.
        // trace:TASK-3131 | ai:codex
        eprintln!("{}", receipt_diagnostic(&receipt, &self.policy_sha)?);
        run.receipt = Some(receipt);
        run.acceptance = Some(self.accept_writer);
        Ok(run)
    }
}
// trace:TASK-3225 | ai:codex
fn validate_service_receipt(receipt: &ContainmentReceipt, epoch: Option<&str>) -> Result<()> {
    anyhow::ensure!(
        match epoch {
            Some(expected) =>
                receipt.mutation_epoch.as_deref() == Some(expected) && receipt.mutation_ready,
            None => receipt.mutation_epoch.is_none() && !receipt.mutation_ready,
        },
        "store service receipt absent or mismatched"
    );
    Ok(())
}

// The conformance harness consumes this standalone stderr line.
// trace:TASK-3194 | ai:codex
fn receipt_diagnostic(receipt: &ContainmentReceipt, policy_sha: &str) -> Result<String> {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(serde_json::to_vec(receipt)?);
    Ok(format!(
        "aida-contained-receipt sha256={digest:x} policy_sha={policy_sha}"
    ))
}

trait PolicyVendor {
    fn pointer_vendor(&self, vendor: &str) -> Option<&str>;
}
impl PolicyVendor for toml::Value {
    fn pointer_vendor(&self, vendor: &str) -> Option<&str> {
        self.get("contained")?.get("vendors")?.get(vendor)?.as_str()
    }
}
pub(crate) struct ContainedChild {
    pub(crate) child: Child,
    receipt: Option<ContainmentReceipt>,
    acceptance: Option<File>,
    credentials: Option<i64>,
    /// Absolute ChatGPT lane deadline in Unix seconds, shared with the supervisor.
    chatgpt_deadline: Option<i64>,
    /// Unlinked host-only evidence; never mounted or passed to the lane.
    oauth_denial: File,
    mutation: Option<store_mutation::Session>,
    _state: tempfile::TempDir,
}
impl ContainedChild {
    /// Preserve vendor exits unless a failed ChatGPT lane expired or the host
    /// denied an OAuth CONNECT. Neither refusal permits a writable retry.
    // trace:TASK-2708 | ai:claude
    // trace:STORY-2593 | ai:codex
    pub(crate) fn vendor_exit(
        &self,
        status: std::process::ExitStatus,
    ) -> Result<std::process::ExitStatus> {
        if let Some(deadline) = self.chatgpt_deadline {
            let expired = chrono::Utc::now().timestamp() >= deadline;
            let mut denied = [0];
            self.oauth_denial.read_exact_at(&mut denied, 0)?;
            anyhow::ensure!(
                status.success() || !(expired || denied == *b"1"),
                codex_auth::RELAUNCH
            );
        }
        if let Some(credentials) = &self.credentials {
            let expired = *credentials <= chrono::Utc::now().timestamp_millis();
            anyhow::ensure!(status.success() || !expired, CLAUDE_EXPIRED_IN_LANE);
        }
        Ok(status)
    }
    /// Caller finishes host registry/lease binding while exec is still blocked.
    pub(crate) fn release(&mut self) -> Result<()> {
        // trace:TASK-3225 | ai:codex — bind through the existing authority
        // owner first; an absent/empty/foreign binding cannot release tools.
        if let Some(service) = &mut self.mutation {
            let receipt = self.receipt.as_ref().context("service receipt absent")?;
            service.bind_worker(receipt.worker_pid, &receipt.worker_start)?;
        }
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
#[path = "../containment/tests/launch_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../containment/tests/mediated_store_tests.rs"]
mod mediated_store_tests;
