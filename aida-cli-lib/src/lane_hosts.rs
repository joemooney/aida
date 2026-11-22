//! Shared claim-to-lane observation used by review and throughput readers.
// trace:BUG-2706 TASK-3827 | ai:codex
use crate::coordination::Claim;
use chrono::{DateTime, Duration, Utc};
pub(crate) const CLOCK_SKEW_SECS: i64 = 300;
/// What a host's record says about one of its lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    /// The heartbeat is inside its expiry.
    Reporting,
    /// Unreleased, heartbeat past its expiry: state unknown.
    Silent,
}

/// One lane on another host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lane {
    pub claim: Claim,
    pub state: State,
    /// The host's last report of this lane.
    pub heartbeat: DateTime<Utc>,
    /// When that report stops counting as live (heartbeat + expiry).
    pub expires: DateTime<Utc>,
}

impl Lane {
    /// Minutes since the host last reported this lane; never negative.
    pub(crate) fn age_mins(&self, now: DateTime<Utc>) -> i64 {
        now.signed_duration_since(self.heartbeat)
            .num_minutes()
            .max(0)
    }

    /// The lease or claim this lane runs under, for a message.
    pub(crate) fn lease(&self) -> &str {
        self.claim
            .lease_id
            .as_deref()
            .or(self.claim.claim_id.as_deref())
            .unwrap_or("no lease id")
    }
}

/// What one parsed record says about another host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Judged {
    /// Not a record of a lane on another host: this host's own claim, or a
    /// process lock. Nothing is concluded from it here.
    NotForeign,
    Lane(Lane),
    /// A record of another host that cannot be used as a report, and why.
    Invalid(String),
}

/// What `claim` says about a lane on another host at `now`.
pub(crate) fn judge(claim: &Claim, now: DateTime<Utc>, our_host: &str) -> Judged {
    if claim.process_backed {
        return Judged::NotForeign;
    }
    if claim.host.trim().is_empty() {
        return Judged::Invalid("it names no host".into());
    }
    if claim.host == our_host {
        return Judged::NotForeign;
    }
    let Ok(heartbeat) = DateTime::parse_from_rfc3339(claim.heartbeat_at.as_str()) else {
        return Judged::Invalid("its heartbeat is not a time".into());
    };
    let heartbeat = heartbeat.with_timezone(&Utc);
    let ahead = now.checked_add_signed(Duration::seconds(CLOCK_SKEW_SECS));
    if ahead.is_none_or(|limit| heartbeat > limit) {
        return Judged::Invalid(format!(
            "its heartbeat is more than {CLOCK_SKEW_SECS}s ahead of this host's clock"
        ));
    }
    let expires = i64::try_from(claim.ttl_secs)
        .ok()
        .and_then(Duration::try_seconds)
        .and_then(|ttl| heartbeat.checked_add_signed(ttl));
    let Some(expires) = expires else {
        return Judged::Invalid("its expiry is not a time".into());
    };
    // The same expiry rule every other reader of a claim applies. A lane on
    // another host has no process this host could look at.
    let state =
        if crate::coordination::claim_staleness(claim, now, our_host, |_, _| false, |_| false)
            .is_none()
        {
            State::Reporting
        } else {
            State::Silent
        };
    Judged::Lane(Lane {
        claim: claim.clone(),
        state,
        heartbeat,
        expires,
    })
}

#[derive(Default)]
pub(crate) struct Inventory {
    pub claims: Vec<Claim>,
    pub incomplete: Option<String>,
}

/// Read the claims at the commit advertised by the publisher. A watcher never
/// has to pull or reset its own working tree to observe a new lane.
pub(crate) fn published_claims(store: &std::path::Path) -> anyhow::Result<Inventory> {
    use aida_core::git_ops::{host_git_command, host_remote_git_command};
    use anyhow::{ensure, Context};
    use std::io::{Seek, Write};
    fn run(cmd: std::process::Command, cap: u64) -> anyhow::Result<Vec<u8>> {
        use crate::bounded_process::{command_output_with_timeout_capped, BoundedCommandOutput};
        let BoundedCommandOutput::Completed(out) =
            command_output_with_timeout_capped(cmd, std::time::Duration::from_secs(8), cap)
        else {
            anyhow::bail!("claim snapshot command failed or timed out");
        };
        anyhow::ensure!(
            out.status.success() && out.stdout.len() as u64 <= cap,
            "claim snapshot unavailable"
        );
        Ok(out.stdout)
    }
    let branch = aida_core::git_ops::current_branch(store)?;
    ensure!(branch != "HEAD", "detached claim store");
    let reference = format!("refs/heads/{branch}");
    let remote = |args: &[&str]| {
        let mut cmd = host_remote_git_command(store, "origin");
        cmd.args(args).env("GIT_TERMINAL_PROMPT", "0");
        run(cmd, 4096)
    };
    let advertised = remote(&["ls-remote", "--exit-code", "origin", &reference])?;
    let advertised = std::str::from_utf8(&advertised)?.trim_end();
    let (sha, name) = advertised
        .split_once('\t')
        .context("invalid claim origin response")?;
    ensure!(
        name == reference
            && matches!(sha.len(), 40 | 64)
            && sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid claim origin ref"
    );
    if aida_core::git_ops::head_sha(store)? != sha {
        remote(&[
            "fetch",
            "--no-write-fetch-head",
            "--no-tags",
            "--refmap=",
            "origin",
            &reference,
        ])?;
    }
    // A link at either parent is unreadable evidence, not an empty tree.
    for path in ["coordination", "coordination/leases"] {
        let mut parent = host_git_command(store);
        parent.args(["ls-tree", sha, "--", path]);
        let row = run(parent, 4096)?;
        if row.is_empty() {
            return Ok(Inventory::default());
        }
        ensure!(row.starts_with(b"040000 tree "), "invalid claim directory");
    }
    let mut list = host_git_command(store);
    list.args(["ls-tree", "-z", "-l", &format!("{sha}:coordination/leases")]);
    let rows = run(list, 4 << 20)?;
    let mut input = tempfile::tempfile()?;
    let mut sizes = Vec::new();
    let mut total = 0usize;
    let mut inventory = Inventory::default();
    for row in rows.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let row = std::str::from_utf8(row)?;
        let (header, path) = row.split_once('\t').context("invalid claim tree")?;
        if !path.ends_with(".toml") {
            continue;
        }
        let fields: Vec<_> = header.split_whitespace().collect();
        if fields.len() != 4 || !matches!(fields[0], "100644" | "100755") || fields[1] != "blob" {
            inventory.incomplete = Some("nonregular claim record".into());
            continue;
        }
        let size: usize = fields[3].parse()?;
        ensure!(
            size <= 65536 && sizes.len() < 20000,
            "claim inventory limit exceeded"
        );
        total += size + 1;
        ensure!(total < 32 << 20, "claim snapshot too large");
        sizes.push(size);
        writeln!(input, "{}", fields[2])?;
    }
    if sizes.is_empty() {
        return Ok(inventory);
    }
    input.rewind()?;
    let mut blobs = host_git_command(store);
    blobs.args(["cat-file", "--batch"]).stdin(input);
    let output = run(blobs, (total + sizes.len() * 100) as u64)?;
    let mut rest = output.as_slice();
    for size in sizes {
        let newline = rest
            .iter()
            .position(|&b| b == b'\n')
            .context("missing claim blob")?;
        rest = &rest[newline + 1..];
        ensure!(rest.get(size) == Some(&b'\n'), "incomplete claim blob");
        match std::str::from_utf8(&rest[..size])
            .ok()
            .and_then(|raw| toml::from_str(raw).ok())
        {
            Some(claim) => inventory.claims.push(claim),
            None => inventory.incomplete = Some("unreadable claim record".into()),
        }
        rest = &rest[size + 1..];
    }
    ensure!(rest.is_empty(), "unexpected claim blob");
    Ok(inventory)
}
