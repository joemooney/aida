//! Shared usage-telemetry aggregation and MCP server launch/publish helpers (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Find the aida-store path for a given repo, if configured.
pub(crate) fn detect_store_path(repo: &std::path::Path) -> Option<std::path::PathBuf> {
    let config_path = repo.join(".aida").join("config.toml");
    if !config_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&config_path).ok()?;
    // First candidate that is an existing git dir wins, as the old per-line
    // loop did. trace:BUG-1650 | ai:claude
    aida_core::store_locate::store_path_candidates(&content)
        .into_iter()
        .map(|val| repo.join(val))
        .find(|sp| sp.exists() && sp.is_dir() && aida_core::git_ops::is_git_repo(sp))
}

/// HEAD-poll an URL until it returns 200 or `timeout` elapses. Used by
/// `aida dev release` to wait for the GitHub Actions release workflow to
/// publish its tarballs after we push the tag.
pub(crate) fn poll_until_published(url: &str, timeout: std::time::Duration) -> Result<()> {
    let start = std::time::Instant::now();
    let mut tick: u32 = 0;
    loop {
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Timed out after {} seconds. The release workflow may have failed.\n\
                 Check status: https://github.com/joemooney/aida/actions\n\
                 Once tarballs are published, run `aida upgrade --yes`.",
                timeout.as_secs()
            );
        }
        let out = std::process::Command::new("curl")
            .args(["-sIL", "-o", "/dev/null", "-w", "%{http_code}"])
            .arg(url)
            .output()
            .context("Failed to invoke curl")?;
        let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if code == "200" {
            println!(
                "\n  {} artifact available ({}s)",
                crate::glyph(crate::glyphs::Glyph::Check),
                start.elapsed().as_secs()
            );
            return Ok(());
        }
        // Animated dots so the user sees we're not stuck.
        let dots = ".".repeat(((tick % 4) + 1) as usize);
        print!(
            "\r  ... waiting for tarball ({:>3}s elapsed, http {}, last poll {}){}    ",
            start.elapsed().as_secs(),
            code,
            tick,
            dots
        );
        std::io::Write::flush(&mut std::io::stdout()).ok();
        tick += 1;
        std::thread::sleep(std::time::Duration::from_secs(15));
    }
}

pub(crate) fn spawn_log_pump<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    prefix: &'static str,
    reader: Option<R>,
    _done: tokio::sync::mpsc::UnboundedSender<()>,
) {
    let Some(reader) = reader else { return };
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("[{}] {}", prefix, line);
        }
    });
}

/// Resolve the aida-server binary: prefer the in-repo target/{release,debug}
/// build, fall back to whatever's on PATH.
pub(crate) fn locate_aida_server_binary(cwd: &std::path::Path) -> Result<std::path::PathBuf> {
    // If we're in (or under) the aida repo, use its built binary —
    // whichever of target/release vs target/debug is more recently
    // built. Mirrors `pick_dev_binary_dir`'s mtime-based choice for the
    // CLI binary, so an old `target/release/aida-server` from a stale
    // `cargo build --release` doesn't shadow a current debug build.
    // (Bug surfaced 2026-05-05: a Feb-22 release binary at v0.1.0 was
    // beating the May-4 debug binary at v0.4.3, and v0.1.0 lacked
    // git-backend support so `dev serve` failed with a YAML parse
    // error against the orphan store.) trace:BUG-1-049 | ai:claude
    let mut probe = cwd.to_path_buf();
    for _ in 0..4 {
        if is_aida_repo(&probe) {
            let release = probe.join("target/release/aida-server");
            let debug = probe.join("target/debug/aida-server");
            let release_mtime = std::fs::metadata(&release).and_then(|m| m.modified()).ok();
            let debug_mtime = std::fs::metadata(&debug).and_then(|m| m.modified()).ok();
            match (release_mtime, debug_mtime) {
                (Some(rm), Some(dm)) => return Ok(if rm >= dm { release } else { debug }),
                (Some(_), None) => return Ok(release),
                (None, Some(_)) => return Ok(debug),
                (None, None) => anyhow::bail!(
                    "Found aida repo at {} but no aida-server binary in target/. Run `cargo build` first.",
                    probe.display()
                ),
            }
        }
        match probe.parent() {
            Some(p) => probe = p.to_path_buf(),
            None => break,
        }
    }
    // Fall back to PATH.
    if let Ok(out) = std::process::Command::new("which")
        .arg("aida-server")
        .output()
    {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !p.is_empty() {
            return Ok(std::path::PathBuf::from(p));
        }
    }
    anyhow::bail!("aida-server not found on PATH and no in-repo build available")
}

/// Split a non-empty string into `(prefix, last_char_str)` on a valid
/// UTF-8 char boundary. Cheaper than `chars().last()` for the parsing
/// path because it avoids an extra allocation. The empty-string case is
/// already guarded at every call site, so this returns `("", "")` for
/// empty input rather than panicking. trace:BUG-100 | ai:claude
pub(crate) fn split_last_char(s: &str) -> (&str, &str) {
    match s.char_indices().next_back() {
        Some((idx, _)) => s.split_at(idx),
        None => ("", ""),
    }
}

// UsageRow + aggregate_events stay in main.rs: `crate::usage_cmd` reads them via
// `crate::`, and `mcp.rs` (the MCP `usage` tool) also calls `aggregate_events`,
// so they are shared, not single-consumer. trace:STORY-122 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct UsageRow {
    pub(crate) cmd: String,
    pub(crate) count: u32,
    pub(crate) errors: u32,
    pub(crate) total_ms: u64,
    // BUG-699: the same stats restricted to a recent window, so a since-resolved
    // historical batch (e.g. `rel list` errors, a slow pre-cache `status`) can't
    // masquerade as current in the `since`-window aggregate.
    pub(crate) recent_count: u32,
    pub(crate) recent_errors: u32,
    pub(crate) recent_total_ms: u64,
}

impl UsageRow {
    pub(crate) fn avg_ms(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            self.total_ms / u64::from(self.count)
        }
    }
    pub(crate) fn recent_avg_ms(&self) -> u64 {
        if self.recent_count == 0 {
            0
        } else {
            self.recent_total_ms / u64::from(self.recent_count)
        }
    }
    pub(crate) fn error_rate(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            f64::from(self.errors) / f64::from(self.count)
        }
    }
}

pub(crate) fn aggregate_events(
    events: &[usage::UsageEvent],
    since: chrono::DateTime<chrono::Utc>,
    recent_since: chrono::DateTime<chrono::Utc>,
) -> std::collections::HashMap<String, UsageRow> {
    let mut by_cmd: std::collections::HashMap<String, UsageRow> = std::collections::HashMap::new();
    for ev in events {
        let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&ev.ts) else {
            continue;
        };
        let ts = ts.with_timezone(&chrono::Utc);
        if ts < since {
            continue;
        }
        let row = by_cmd.entry(ev.cmd.clone()).or_insert_with(|| UsageRow {
            cmd: ev.cmd.clone(),
            count: 0,
            errors: 0,
            total_ms: 0,
            recent_count: 0,
            recent_errors: 0,
            recent_total_ms: 0,
        });
        let is_err = ev.exit_code != 0;
        row.count = row.count.saturating_add(1);
        if is_err {
            row.errors = row.errors.saturating_add(1);
        }
        row.total_ms = row.total_ms.saturating_add(ev.duration_ms);
        // BUG-699: the recent sub-window.
        if ts >= recent_since {
            row.recent_count = row.recent_count.saturating_add(1);
            if is_err {
                row.recent_errors = row.recent_errors.saturating_add(1);
            }
            row.recent_total_ms = row.recent_total_ms.saturating_add(ev.duration_ms);
        }
    }
    by_cmd
}

/// Look up the current status label of a requirement by spec-id — used to
/// annotate the drafted BUG in the `--auto-complete` failure list.
/// trace:TASK-266 | ai:claude
pub(crate) fn bug_status(store: &RequirementsStore, spec_id: &str) -> Option<String> {
    store
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some(spec_id))
        .map(|r| r.status.to_string())
}
