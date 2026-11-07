//! Host-only, non-renewing ChatGPT credential delivery. Never serialize the
//! host cache or refresh material into the lane, argv, environment or receipts.
// trace:STORY-2593 | ai:codex
use super::{Mount, WriteProfile};
use crate::vendors::codex_auth_hints as hints;
use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::fs::File;
use std::io::{Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

pub(super) const RELAUNCH: &str =
    "contained ChatGPT authentication unavailable or expired; reauthenticate on the host, then relaunch";
const MARGIN: i64 = 300;
const BASELINE: i64 = 3600;
pub(crate) const DEFAULT_MIN_GRANT_SECS: i64 = 1800;
const BOUND: usize = 65536;
pub(super) const PROGRAM_SHA256: &str =
    "b8a11fedf24b2dfb59a70c9e52b030f9a508dcd49cd2dd0d41956f4c785156b8";

/// Pin the probed interface without executing lane-selected code on the host.
/// The actual vendor execution still happens only within the contained lane.
pub(super) fn require_program(program: &Path, expected_sha256: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(program)
        .context("contained ChatGPT executable unavailable")?;
    let md = file.metadata()?;
    // The recorded musl executable is approximately 305 MiB.
    let limit = 512 * 1024 * 1024;
    anyhow::ensure!(
        md.is_file() && md.len() <= limit,
        "contained ChatGPT executable refused"
    );
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    let mut total = 0;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        anyhow::ensure!(total <= limit, "contained ChatGPT executable exceeds bound");
        digest.update(&buffer[..n]);
    }
    anyhow::ensure!(
        format!("{:x}", digest.finalize()) == expected_sha256,
        "contained ChatGPT auth refused: re-probe this codex binary (supported: codex-cli 0.166.0 SHA-256 {PROGRAM_SHA256}); put the probed native codex binary first on PATH"
    );
    Ok(())
}
pub(super) const OAUTH_HOSTS: [&str; 4] = [
    "auth.openai.com",
    "auth0.openai.com",
    "openai.auth0.com",
    "auth.chatgpt.com",
];

// Intentionally no Debug/Serialize: only the sealed reduced leaf can cross.
pub(super) struct Credential {
    pub file: File,
    pub deadline: i64,
    pub expires_at: i64,
}

/// No endpoint-level TLS inspection exists in the CONNECT broker. Deny the
/// entire OAuth hosts, even if trusted policy allows them for other vendors.
pub(super) fn exclude_oauth_hosts(allowed: &mut Vec<String>) {
    allowed.retain(|h| !OAUTH_HOSTS.contains(&h.as_str()));
}

// This reads lifetime/routing metadata from the trusted host cache, not a
// signature check or a new source of grant authority. Upstream verifies tokens.
fn claims(token: &str) -> Result<serde_json::Value> {
    let parts: Vec<_> = token.split('.').collect();
    anyhow::ensure!(parts.len() == 3, "ChatGPT token claims invalid");
    let bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| anyhow::anyhow!("ChatGPT token claims invalid"))?;
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("ChatGPT token claims invalid"))
}

// Launch callers enforce executable compatibility before this reader; doctor
// reads the same bounded metadata without executing or admitting the vendor.
fn read_credentials(home: &Path, owner: u32) -> Result<serde_json::Value> {
    let file = std::fs::OpenOptions::new().read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(home.join("auth.json"))
        .map_err(|_| anyhow::anyhow!("ChatGPT file credentials unavailable; keyring logins unsupported; relaunch after host login"))?;
    let md = file.metadata()?;
    anyhow::ensure!(
        md.is_file()
            && md.nlink() == 1
            && md.uid() == owner
            && md.mode() & 0o7777 == 0o600
            && md.len() <= BOUND as u64,
        "ChatGPT credential identity or permissions refused (requires private mode 0600)"
    );
    let mut bytes = vec![0; BOUND + 1];
    let n = file
        .read_at(&mut bytes, 0)
        .context("ChatGPT credentials unreadable")?;
    anyhow::ensure!(n <= BOUND, "ChatGPT credentials exceed bound");
    // Do not propagate serde errors: a type mismatch can echo secret values.
    let value: serde_json::Value = serde_json::from_slice(&bytes[..n])
        .map_err(|_| anyhow::anyhow!("ChatGPT credentials invalid"))?;
    anyhow::ensure!(
        value.get("auth_mode").and_then(|v| v.as_str()) == Some("chatgpt"),
        "ChatGPT file login required; keyring and other auth modes unsupported"
    );
    Ok(value)
}

// trace:BUG-2486 | ai:codex
pub(crate) fn token_expiry(home: &Path, owner: u32) -> Result<i64> {
    access_expiry(&read_credentials(home, owner)?)
}

fn access_expiry(value: &serde_json::Value) -> Result<i64> {
    let access = value
        .pointer("/tokens/access_token")
        .and_then(|v| v.as_str())
        .context("ChatGPT access token missing")?;
    claims(access)?
        .get("exp")
        .and_then(|v| v.as_i64())
        .context("ChatGPT access-token expiry missing")
}

// trace:BUG-2486 | ai:codex
pub(super) fn grant_deadline(now: i64, exp: i64, ttl: i64, floor: i64) -> Result<i64> {
    anyhow::ensure!(floor > 0, "{}", hints::MIN_GRANT_INVALID);
    let requested = now
        .checked_add(if ttl > 0 { ttl } else { BASELINE })
        .context("ChatGPT deadline invalid")?;
    let deadline = requested.min(exp.saturating_sub(MARGIN));
    anyhow::ensure!(
        deadline.saturating_sub(now) >= floor,
        "{}",
        hints::floor_refusal(exp, exp.saturating_sub(now), floor)
    );
    Ok(deadline)
}

pub(super) fn prepare(home: &Path, owner: u32, ttl: i64, floor: i64) -> Result<Credential> {
    let value = read_credentials(home, owner)?;
    let tokens = value.get("tokens").context("ChatGPT tokens missing")?;
    let access = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .context("ChatGPT access token missing")?;
    let id = tokens
        .get("id_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .context("ChatGPT identity token missing")?;
    let identity = claims(id)?;
    let account = identity
        .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .context("ChatGPT identity account routing missing")?;
    // Account ID is derived from the identity claim the approved CLI uses.
    // Refuse conflicting host routing rather than silently switching account.
    anyhow::ensure!(
        tokens
            .get("account_id")
            .and_then(|v| v.as_str())
            .is_none_or(|v| v == account),
        "ChatGPT account routing mismatch"
    );
    let exp = access_expiry(&value)?;
    let now = chrono::Utc::now().timestamp();
    let deadline = grant_deadline(now, exp, ttl, floor)?;
    // No other host-cache fields are copied. Codex needs explicit account
    // routing and a recent cache timestamp to avoid its refresh path. Derive
    // routing from the approved ID token and stamp this leaf's delivery time;
    // this neither refreshes the bearer nor extends its checked expiry.
    let reduced = serde_json::json!({"auth_mode":"chatgpt", "tokens": {
        "access_token":access, "id_token":id, "refresh_token":"", "account_id":account
    }, "last_refresh":chrono::Utc::now().to_rfc3339()});
    let fd = unsafe {
        libc::memfd_create(
            c"aida-codex-auth".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    anyhow::ensure!(fd >= 0, "ChatGPT memory leaf unavailable");
    let mut leaf = unsafe { File::from_raw_fd(fd) };
    use std::os::unix::fs::PermissionsExt;
    leaf.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    leaf.write_all(&serde_json::to_vec(&reduced)?)?;
    anyhow::ensure!(
        unsafe {
            libc::fcntl(
                leaf.as_raw_fd(),
                libc::F_ADD_SEALS,
                libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL,
            )
        } == 0,
        "ChatGPT memory leaf sealing failed"
    );
    leaf.rewind()?;
    Ok(Credential {
        file: leaf,
        deadline,
        expires_at: exp,
    })
}

pub(super) fn bind(
    profile: &mut WriteProfile,
    private_home: &Path,
    auth: Credential,
) -> Result<i64> {
    let config = private_home.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "cli_auth_credentials_store = \"file\"\n{}",
            profile.native()
        ),
    )?;
    profile.add(&config, false)?;
    let path = private_home.join("auth.json");
    std::fs::write(&path, "")?;
    profile.mounts.push(Mount {
        data: true,
        file: auth.file,
        path,
        writable: false,
    });
    Ok(auth.deadline)
}
