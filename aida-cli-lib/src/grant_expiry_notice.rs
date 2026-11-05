//! Informational Claude, Codex, and Antigravity tool-hook notices; no graph/cache/storage handle.
// trace:BUG-2014 | ai:codex
// trace:TASK-1744 | ai:antigravity
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeClient {
    Claude,
    Codex,
    Antigravity,
}

impl NoticeClient {
    pub(crate) fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "claude" | "claude-code" => Ok(Self::Claude),
            "codex" | "codex-cli" => Ok(Self::Codex),
            "agy" | "antigravity" | "antigravity-cli" => Ok(Self::Antigravity),
            other => anyhow::bail!(
                "unsupported expiry-notice client `{other}` (expected claude, codex, or agy)"
            ),
        }
    }

    pub(crate) fn render(self, message: Option<String>) -> Option<serde_json::Value> {
        match (self, message) {
            (Self::Claude | Self::Codex, Some(message)) => Some(serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "additionalContext": message
                }
            })),
            (Self::Claude | Self::Codex, None) => None,
            // Antigravity CLI 1.4.2 PreInvocation hook contract (documented in
            // `~/.gemini/antigravity-cli/builtin/skills/agy-customizations/docs/hooks.md`,
            // sections "2. `PostToolUse` Contract" and "4. `PreInvocation` Contract"):
            // `injectSteps` injects an ephemeral message before the model call,
            // and `{}` is an explicit no-op JSON object on stdout.
            (Self::Antigravity, Some(message)) => Some(serde_json::json!({
                "injectSteps": [
                    { "ephemeralMessage": message }
                ]
            })),
            (Self::Antigravity, None) => Some(serde_json::json!({})),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Ledger {
    epoch: String,
    // Highest observed threshold: 1 = 2h, 2 = 30m, 3 = expired.
    crossed: u8,
}

// trace:BUG-2451 | ai:codex
struct NoticeLock(std::fs::File);

impl Drop for NoticeLock {
    fn drop(&mut self) {
        // A concurrent fork can retain this open file description until exec.
        // Explicitly unlock on every exit, including deduplication and errors,
        // rather than waiting for the last inherited descriptor to close.
        let _ = FileExt::unlock(&self.0);
    }
}

fn threshold(left: Duration) -> u8 {
    if left <= Duration::zero() {
        3
    } else if left <= Duration::minutes(30) {
        2
    } else if left <= Duration::hours(2) {
        1
    } else {
        0
    }
}

fn claim(base: &Path, id: &str, epoch: &str, crossing: u8) -> Result<bool> {
    if crossing == 0 {
        return Ok(false);
    }
    // trace:TASK-2707 | ai:codex — informational state has its own directory
    // leaf; the lane never writes canonical grant records or their locks.
    let dir = base.join(id);
    std::fs::create_dir_all(&dir)?;
    // Permanent lock inode, never renamed or unlinked. Nonblocking contention
    // defers to the next tool call; the shell hard timeout is the outer bound.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("notice.lock"))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    let _lock = NoticeLock(lock);
    let path = dir.join("notice.json");
    let mut ledger: Ledger = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("invalid expiry notice ledger")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ledger::default(),
        Err(error) => return Err(error.into()),
    };
    if ledger.epoch != epoch {
        ledger = Ledger {
            epoch: epoch.into(),
            crossed: 0,
        };
    }
    if crossing <= ledger.crossed {
        return Ok(false);
    }
    ledger.crossed = crossing;
    aida_core::write_atomic(&path, serde_json::to_vec(&ledger)?)?;
    Ok(true)
}

// trace:TASK-2707 | ai:codex
pub(crate) fn notice_base() -> Result<std::path::PathBuf> {
    Ok(crate::gate::seat_authority::grant_dir()?
        .parent()
        .context("grant directory has no parent")?
        .join("grant-notices"))
}

pub(crate) fn notice_leaf(id: &str) -> Result<std::path::PathBuf> {
    anyhow::ensure!(
        uuid::Uuid::parse_str(id).is_ok(),
        "invalid notice grant selector"
    );
    Ok(notice_base()?.join(id))
}

/// Marker in the grant-notices base: the one-time legacy sweep has run.
// trace:TASK-2708 | ai:claude
const LEGACY_SWEPT_MARKER: &str = ".legacy-notices-swept-v1";

/// Host-only surgery, no migration or compatibility reads. Never unlink the
/// new permanent lock. A contained caller cannot write session-grants.
/// ONE-TIME (TASK-2708 ruling 4): the hook runs per tool call, so after the
/// first complete sweep a marker in the grant-notices base turns every later
/// call into a single `stat`, never a readdir of session-grants.
// trace:TASK-2708 | ai:claude
fn sweep_legacy_notices() {
    if std::env::var_os("AIDA_CONTAINED").is_some() {
        return;
    }
    if let (Ok(grants), Ok(base)) = (crate::gate::seat_authority::grant_dir(), notice_base()) {
        sweep_legacy_notices_once(&grants, &base);
    }
}

pub(crate) fn sweep_legacy_notices_once(grants: &Path, base: &Path) {
    let marker = base.join(LEGACY_SWEPT_MARKER);
    if marker.exists() {
        return;
    }
    match std::fs::read_dir(grants) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.ends_with(".notice.json") || name.ends_with(".notice.lock") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        // Unreadable now: retry on a later call rather than marking it done.
        Err(_) => return,
    }
    if std::fs::create_dir_all(base).is_ok() {
        let _ = std::fs::write(&marker, b"");
    }
}

fn notice(root: &Path, now: DateTime<Utc>) -> Result<Option<String>> {
    let Some(grant) = crate::gate::seat_authority::notice_grant(root, now)? else {
        return Ok(None);
    };
    let crossing = threshold(grant.expiry.expires_at - now);
    // Expiry changes also reset the epoch for an in-place renewal.
    let epoch = format!("{}:{}", grant.id, grant.expiry.expires_at);
    if !claim(&notice_base()?, &grant.id, &epoch, crossing)? {
        return Ok(None);
    }
    let expiry = grant.expiry;
    // Existing warning's strict '<' boundary needs one millisecond at 2h.
    Ok(crate::gate::seat_authority::expiry_warning(
        &expiry,
        now,
        Duration::hours(2) + Duration::milliseconds(1),
    ))
}

pub(crate) fn handle(raw_client: &str) -> Result<()> {
    sweep_legacy_notices();
    let client = match NoticeClient::parse(raw_client) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("Expiry notice unavailable: {error:#}");
            return Ok(());
        }
    };
    let root = crate::find_project_root().unwrap_or_else(|_| ".".into());
    match notice(&root, Utc::now()) {
        Ok(message) => {
            if let Some(payload) = client.render(message) {
                println!("{payload}");
            }
        }
        // No permission decision or refusal on ledger failure; Antigravity still
        // receives an empty JSON object so its hook parser stays fail-open.
        Err(error) => {
            eprintln!("Expiry notice unavailable: {error:#}");
            if let Some(payload) = client.render(None) {
                println!("{payload}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/bug_2014_grant_notice_tests.rs"]
mod tests;
