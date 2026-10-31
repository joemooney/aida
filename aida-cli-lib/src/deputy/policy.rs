//! The deputy's TTY policy: which argv prefixes may run on a terminal the
//! deputy owns. Edited only at the operator's human TTY (`aida deputy
//! --allow-tty`, `aida deputy allow|deny`). An entry is an argv prefix with an
//! expiry, optionally single-use, and never outlives the operator's grant.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;

use super::{JobBody, Spool, BYPASS_FLAGS};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct PolicyEntry {
    pub id: String,
    pub prefix: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub single_use: bool,
    #[serde(default)]
    pub consumed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub consumed_by: Option<String>,
}

impl PolicyEntry {
    pub(crate) fn live(&self, now: DateTime<Utc>) -> bool {
        self.expires_at > now && self.consumed_at.is_none()
    }

    pub(crate) fn matches(&self, argv: &[String]) -> bool {
        !self.prefix.is_empty()
            && argv.len() >= self.prefix.len()
            && argv[..self.prefix.len()] == self.prefix[..]
    }

    pub(crate) fn label(&self) -> String {
        self.prefix.join(" ")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Policy {
    #[serde(default)]
    pub entries: Vec<PolicyEntry>,
}

/// The policy's answer for one job.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TtyDecision {
    /// No terminal; the job runs detached.
    NoTty,
    /// Run on a deputy-owned pty under `entry` until `expires_at`.
    Tty {
        entry: String,
        expires_at: DateTime<Utc>,
        /// The entry is used up by this job; the claim must record that
        /// before the job may have the terminal ([`consume_single_use`]).
        single_use: bool,
    },
    /// A policy entry matched but the argv carries a gate-bypass flag.
    RefuseBypass(String),
}

impl Policy {
    /// Decide whether `body` runs on a terminal. Script-form jobs never do.
    /// `grant_expires_at` caps every entry: a TTL never outlives the grant.
    pub(crate) fn decide(
        &self,
        body: &JobBody,
        now: DateTime<Utc>,
        grant_expires_at: Option<DateTime<Utc>>,
    ) -> TtyDecision {
        let JobBody::Argv(argv) = body else {
            return TtyDecision::NoTty;
        };
        let Some(entry) = self
            .entries
            .iter()
            .filter(|e| e.live(now) && e.matches(argv))
            .max_by_key(|e| e.prefix.len())
        else {
            return TtyDecision::NoTty;
        };
        if let Some(flag) = argv.iter().find(|a| is_bypass_flag(a)) {
            return TtyDecision::RefuseBypass(flag.clone());
        }
        let expires_at = match grant_expires_at {
            Some(g) if g < entry.expires_at => g,
            _ => entry.expires_at,
        };
        if expires_at <= now {
            return TtyDecision::NoTty;
        }
        TtyDecision::Tty {
            entry: entry.id.clone(),
            expires_at,
            single_use: entry.single_use,
        }
    }

    /// Mark a live single-use entry consumed by `job`. Returns whether
    /// anything changed.
    pub(crate) fn consume(&mut self, entry: &str, job: &str, now: DateTime<Utc>) -> bool {
        match self.entries.iter_mut().find(|e| e.id == entry) {
            Some(e) if e.single_use && e.live(now) => {
                e.consumed_at = Some(now);
                e.consumed_by = Some(job.to_string());
                true
            }
            _ => false,
        }
    }

    pub(crate) fn live_entries(&self, now: DateTime<Utc>) -> Vec<&PolicyEntry> {
        self.entries.iter().filter(|e| e.live(now)).collect()
    }

    /// Add an entry. Refuses an empty prefix and a prefix carrying a bypass
    /// flag (that would authorize the bypass itself).
    pub(crate) fn allow(
        &mut self,
        prefix: Vec<String>,
        ttl_secs: u64,
        single_use: bool,
        now: DateTime<Utc>,
    ) -> Result<PolicyEntry> {
        if prefix.is_empty() {
            bail!("a TTY policy entry needs a command prefix");
        }
        if let Some(flag) = prefix.iter().find(|a| is_bypass_flag(a)) {
            bail!("a TTY policy entry cannot include the gate-bypass flag {flag}");
        }
        if ttl_secs == 0 {
            bail!("a TTY policy entry needs a TTL greater than zero");
        }
        let entry = PolicyEntry {
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            prefix,
            created_at: now,
            expires_at: now + chrono::Duration::seconds(ttl_secs.min(i64::MAX as u64) as i64),
            single_use,
            consumed_at: None,
            consumed_by: None,
        };
        self.entries.push(entry.clone());
        Ok(entry)
    }

    /// Remove entries whose id equals `key` or whose prefix is `key`.
    pub(crate) fn deny(&mut self, key: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != key && e.label() != key);
        before - self.entries.len()
    }

    /// Drop entries that expired or were consumed more than a day ago.
    pub(crate) fn prune(&mut self, now: DateTime<Utc>) {
        let horizon = now - chrono::Duration::days(1);
        self.entries
            .retain(|e| e.expires_at > horizon && e.consumed_at.is_none_or(|c| c > horizon));
    }
}

pub(crate) fn is_bypass_flag(arg: &str) -> bool {
    BYPASS_FLAGS
        .iter()
        .any(|f| arg == *f || arg.starts_with(&format!("{f}=")))
}

/// Split a prefix the operator typed (`aida merge-hold clear`) into argv.
pub(crate) fn parse_prefix(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_string).collect()
}

/// Use up single-use entry `entry` for `job`, failing CLOSED: the policy is
/// reloaded, the entry must still be live and unused, and the consumed state
/// must be saved. Any failure means the job gets no terminal and the entry
/// stays unused (it was never recorded as consumed).
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
pub(crate) fn consume_single_use(
    spool: &Spool,
    entry: &str,
    job: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    let mut p = load(spool).context("reload the TTY policy")?;
    if !p.consume(entry, job, now) {
        bail!("single-use TTY entry {entry} is no longer live");
    }
    save(spool, &p).context("record the single-use TTY entry as used")
}

pub(crate) fn load(spool: &Spool) -> Result<Policy> {
    let p = spool.policy_path();
    let md = match std::fs::symlink_metadata(&p) {
        Ok(md) => md,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Policy::default()),
        Err(e) => return Err(e).context("stat deputy policy"),
    };
    if !md.file_type().is_file() {
        bail!("{} is not a regular file; refusing it", p.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if md.uid() != super::current_uid() || md.mode() & 0o077 != 0 {
            bail!(
                "{} must be owned by you and mode 0600; refusing it",
                p.display()
            );
        }
    }
    let bytes = std::fs::read(&p)?;
    serde_json::from_slice(&bytes).context("parse deputy policy")
}

pub(crate) fn save(spool: &Spool, policy: &Policy) -> Result<()> {
    let tmp = spool.root.join(".deputy.policy.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).context("write deputy policy")?;
    f.write_all(&serde_json::to_vec_pretty(policy)?)?;
    f.sync_all()?;
    std::fs::rename(&tmp, spool.policy_path())?;
    Ok(())
}
