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

fn claim(dir: &Path, id: &str, epoch: &str, crossing: u8) -> Result<bool> {
    if crossing == 0 {
        return Ok(false);
    }
    // Permanent lock inode, never renamed or unlinked. Nonblocking contention
    // defers to the next tool call; the shell hard timeout is the outer bound.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(format!("{id}.notice.lock")))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    let path = dir.join(format!("{id}.notice.json"));
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

fn notice(root: &Path, now: DateTime<Utc>) -> Result<Option<String>> {
    let Some(grant) = crate::gate::seat_authority::notice_grant(root, now)? else {
        return Ok(None);
    };
    let crossing = threshold(grant.expiry.expires_at - now);
    // Expiry changes also reset the epoch for an in-place renewal.
    let epoch = format!("{}:{}", grant.id, grant.expiry.expires_at);
    if !claim(
        &crate::gate::seat_authority::grant_dir()?,
        &grant.id,
        &epoch,
        crossing,
    )? {
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
