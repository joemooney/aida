/// TASK-804 (facet a of STORY-604): build the argv (after the `claude` program
/// name) for the `--verbose` burndown drain. Identical base to the quiet launch
/// (`-p <prompt> --permission-mode <mode>`) PLUS the stream-json flags so the
/// drain's events can be teed and rendered live. The permission mode stays
/// CALLER-controlled (not the headless helper's baked-in `bypassPermissions`)
/// because `--verbose` must only add visibility — it must never change the
/// drain's permission posture or control flow. The prompt stays the trailing
/// positional. Pure + unit-tested. trace:TASK-804 | ai:claude
pub(crate) fn burndown_verbose_claude_args(prompt: &str, mode: &str) -> Vec<String> {
    vec![
        "-p".to_string(),
        "--permission-mode".to_string(),
        mode.to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
        prompt.to_string(),
    ]
}

/// TASK-804: the discoverable JSONL log path for a `--verbose` burndown drain:
/// `.aida/burndown/<drain-id>.jsonl`. Gitignored by the deny-by-default
/// `.aida/*` rule — pure per-clone runtime state. This is the path future
/// drain-status tooling (TASK-806) will read. trace:TASK-804 | ai:claude
pub(crate) fn burndown_drain_log_path(
    project_root: &std::path::Path,
    drain_id: &str,
) -> std::path::PathBuf {
    project_root
        .join(".aida")
        .join("burndown")
        .join(format!("{drain_id}.jsonl"))
}

/// TASK-806: the most recent burndown event log under `.aida/burndown/`, if any.
/// Drain ids are `%Y%m%dT%H%M%SZ-<uuid8>` (see [`run_burndown_verbose`]), so a
/// lexicographic sort of the `*.jsonl` file names is chronological — the last
/// entry is the newest run. The pointer is best-effort: a `--verbose` drain
/// writes one of these, a quiet drain does not, so absence is normal.
/// trace:TASK-806 | ai:claude
pub(crate) fn latest_burndown_log(project_root: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = project_root.join(".aida").join("burndown");
    let mut logs: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .collect();
    logs.sort();
    logs.pop()
}

/// TASK-806: one in-flight leased worktree the drain has fanned an implementer
/// into — the read-side answer to "what specs are being worked right now".
/// Derived from the live session leases (`.aida/sessions/*.toml`).
/// trace:TASK-806 | ai:claude
pub(crate) struct InFlightLease {
    pub(crate) scope: String,
    pub(crate) branch: String,
    pub(crate) role: String,
    pub(crate) worktree: String,
    /// TASK-834: most-recent activity in this lease's worktree (newest of the
    /// last commit time and the newest non-`.git`/non-`target` file mtime).
    /// `None` when the worktree is gone or git/fs probing failed — render the
    /// row without the activity suffix rather than error. trace:TASK-834
    pub(crate) last_activity: Option<chrono::DateTime<chrono::Utc>>,
}

/// TASK-805: spec ids with a live (non-stale) session lease — "actively being
/// worked" (the InProgress half of the running-drain marking). Reuses the same
/// lease-liveness classification `aida session leases` uses. A lease's `scope`
/// is the spec id for a single-spec session; epic/batch scopes simply won't
/// match a spec id, which is the correct (no-mark) outcome. trace:TASK-805
pub(crate) fn leased_spec_ids(project_root: &std::path::Path) -> std::collections::HashSet<String> {
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    list_leases(project_root)
        .into_iter()
        .filter(|l| !matches!(lease_state_for(l, &live, now), LeaseState::Stale))
        .map(|l| l.scope)
        .collect()
}

/// TASK-805: a LIVE-drain overlay for the read views (`queue list`, `burndown
/// plan`). When a `burndown run` / `queue work --auto-complete` drain holds the
/// BUG-538 lock, these views mark the specs it owns: **in-flight** (an
/// implementer is actively leased on it) vs **scheduled** (claimed by the
/// drain's blessed/queued set, not yet picked up). Reads the SAME
/// `.aida/drain.lock` (`drain_lock::probe_lock`) the drain writes — never a
/// parallel liveness probe (advisor note 2026-06-14). trace:TASK-805
pub(crate) struct DrainOverlay {
    pub(crate) pid: u32,
    /// Spec ids with a live session lease — actively being worked.
    pub(crate) in_flight: std::collections::HashSet<String>,
}

impl DrainOverlay {
    /// Probe the lock + live leases. `Some` only when a drain is ACTIVELY
    /// running (lock present + pid alive); `None` for no-drain or a stale lock
    /// — the marking is about a live drain, so a crashed lock adds no overlay
    /// (a reader who wants the crash signal uses `burndown status`). trace:TASK-805
    pub(crate) fn probe(project_root: &std::path::Path) -> Option<DrainOverlay> {
        match drain_lock::probe_lock(project_root) {
            drain_lock::LockStatus::Running(l) => Some(DrainOverlay {
                pid: l.pid,
                in_flight: leased_spec_ids(project_root),
            }),
            _ => None,
        }
    }

    /// Partition `specs` (a set the caller is displaying) into (in_flight,
    /// scheduled): in_flight = has a live lease, scheduled = the rest. Both
    /// sorted for stable output. trace:TASK-805
    pub(crate) fn partition(&self, specs: &[String]) -> (Vec<String>, Vec<String>) {
        let (mut in_flight, mut scheduled): (Vec<String>, Vec<String>) = specs
            .iter()
            .cloned()
            .partition(|id| self.in_flight.contains(id));
        in_flight.sort();
        scheduled.sort();
        (in_flight, scheduled)
    }
}

/// TASK-805: the "a drain is running" banner shared by `queue list` and
/// `burndown plan`. Pure over its inputs (the spec ids already partitioned by
/// the caller from the set it displays) so it renders identically in tests and
/// at the terminal. trace:TASK-805
pub(crate) fn drain_running_banner(pid: u32, in_flight: &[String], scheduled: &[String]) -> String {
    let cell = |label: &str, ids: &[String]| -> String {
        if ids.is_empty() {
            format!("{label}: none")
        } else {
            format!("{label}: {}", ids.join(", "))
        }
    };
    format!(
        "{} a drain is running (pid {pid}) — {}; {}",
        "⚡".yellow().bold(),
        cell("in-flight", in_flight),
        cell("scheduled", scheduled),
    )
}

/// TASK-806: `aida burndown status` — the read-side companion to `burndown
/// run`. Answers, from the substrate alone: is a drain running (the BUG-538
/// global drain lock + a PID-liveness probe), what is it working (the live
/// session leases — the fanned-out implementer worktrees), and where is the
/// live event log (`.aida/burndown/<drain-id>.jsonl`, TASK-804) to tail. Pure
/// read; exits 0 whether or not a drain is running. trace:TASK-806 | ai:claude
pub(crate) fn handle_burndown_status(json: bool) -> Result<()> {
    // Resolve the shared `.aida/` root from any worktree so a child in a sibling
    // worktree reads the *orchestrator's* lock / leases — mirrors `aida drain
    // status`. Fails safe to "no drain" on a resolution error.
    let project_root = find_main_worktree_root()
        .or_else(|_| std::env::current_dir())
        .unwrap_or_else(|_| std::path::PathBuf::from("."));

    let lock = drain_lock::probe_lock(&project_root);

    // In-flight = the live (non-stale) leases — the implementer worktrees the
    // drain fanned out. Same liveness classification `aida session leases` uses.
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    let in_flight: Vec<InFlightLease> = list_leases(&project_root)
        .into_iter()
        .filter(|l| !matches!(lease_state_for(l, &live, now), LeaseState::Stale))
        .map(|l| InFlightLease {
            scope: l.scope.clone(),
            branch: l.branch.clone(),
            role: l.role.clone().unwrap_or_else(|| "-".to_string()),
            worktree: l.worktree_path.display().to_string(),
            // TASK-834: best-effort per-agent activity probe from the worktree.
            last_activity: worktree_last_activity(&l.worktree_path),
        })
        .collect();

    let log = latest_burndown_log(&project_root);

    if json {
        println!(
            "{}",
            render_burndown_status_json(&lock, &in_flight, log.as_deref())
        );
    } else {
        print!(
            "{}",
            render_burndown_status_human(&lock, &in_flight, log.as_deref())
        );
        // TASK-833: open-PR section — the universal "work in flight, any source"
        // signal that leases miss (keyboard agents, Agent-tool fan-out, manual
        // PRs never register an AIDA lease, so they're invisible mid-flight until
        // the merge heartbeat). Best-effort: empty (forge-aware degrade) → skip.
        // trace:TASK-833 | ai:claude
        let mut open_prs: Vec<status_cleanup::OpenPrItem> = collect_open_prs(&project_root)
            .by_branch
            .into_values()
            .collect();
        if !open_prs.is_empty() {
            const PR_CAP: usize = 15;
            // Stable order: lowest PR number (oldest) first.
            open_prs.sort_by_key(|pr| pr.number);
            println!(
                "\n  {} Open PRs ({}) — awaiting merge:",
                "◆".cyan().bold(),
                open_prs.len()
            );
            for pr in open_prs.iter().take(PR_CAP) {
                println!(
                    "    {} {}  {}",
                    format!("#{}", pr.number).yellow(),
                    pr.title,
                    pr.head_branch.dimmed()
                );
            }
            if open_prs.len() > PR_CAP {
                println!(
                    "    {}",
                    format!("+{} more", open_prs.len() - PR_CAP).dimmed()
                );
            }
        }
        // TASK-829: recent-activity heartbeat. Without this, "no drain running"
        // reads as "nothing is happening" even when an advisor agent is clearing
        // specs at the keyboard (worktree → PR → merge, which never registers a
        // lease). Surfacing recent completions makes the view a liveness signal,
        // not just a drain-state probe. trace:TASK-829 | ai:claude
        if let Some(line) = recent_activity_line(&project_root, now) {
            println!("{line}");
        }
    }
    Ok(())
}

/// TASK-829: a one-line recent-activity heartbeat for `aida burndown status` /
/// `aida list inflight` — specs that reached Completed in the last 24h, read
/// from the store (`modified_at` is stamped on the status flip, incl. the
/// auto-bump on merge). Proves the system is making progress even when no drain
/// or lease is live. `None` only when the store can't be loaded.
/// trace:TASK-829 | ai:claude
pub(crate) fn recent_activity_line(
    project_root: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let store = load_store_for_lookup(project_root)?;
    let cutoff = now - chrono::Duration::hours(24);
    let mut done: Vec<(String, chrono::DateTime<chrono::Utc>)> = store
        .requirements
        .iter()
        .filter(|r| matches!(r.status, aida_core::RequirementStatus::Completed))
        .filter(|r| r.modified_at >= cutoff)
        .map(|r| {
            let id = r
                .agreed_id
                .clone()
                .or_else(|| r.spec_id.clone())
                .unwrap_or_else(|| r.id.to_string());
            (id, r.modified_at)
        })
        .collect();
    if done.is_empty() {
        return Some(format!(
            "\n  {} no completions in the last 24h · {} for the full feed",
            "○".dimmed(),
            "aida history".cyan()
        ));
    }
    done.sort_by(|a, b| b.1.cmp(&a.1));
    let (last_id, last_t) = &done[0];
    Some(format!(
        "\n  {} recent: {} spec{} completed in last 24h — last: {} ({}) · {}",
        "●".green(),
        done.len(),
        if done.len() == 1 { "" } else { "s" },
        last_id.cyan(),
        humanize_relative(*last_t),
        "aida history".cyan()
    ))
}

/// TASK-834: idle threshold past which a still-alive lease is flagged "possibly
/// stuck" — the process exists but nothing has moved in the worktree for a
/// while. trace:TASK-834
pub(crate) const ACTIVITY_STUCK_THRESHOLD_SECS: i64 = 10 * 60;

/// TASK-834: best-effort last-activity timestamp for a lease's worktree. Returns
/// the newest of (the worktree's `git log -1` commit time) and (the newest
/// non-`.git`/non-`target` file mtime). `None` when the worktree is missing or
/// every probe failed — callers render the row without an activity suffix rather
/// than erroring. trace:TASK-834 | ai:claude
pub(crate) fn worktree_last_activity(
    worktree: &std::path::Path,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if worktree.as_os_str().is_empty() || !worktree.exists() {
        return None;
    }

    let mut newest: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut consider = |t: chrono::DateTime<chrono::Utc>| {
        newest = Some(match newest {
            Some(cur) if cur >= t => cur,
            _ => t,
        });
    };

    // Leg 1: last commit time (epoch seconds) via `git log -1 --format=%ct`.
    if let Ok(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["log", "-1", "--format=%ct"])
        .output()
    {
        if output.status.success() {
            if let Ok(epoch) = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<i64>()
            {
                if let Some(dt) = chrono::DateTime::from_timestamp(epoch, 0) {
                    consider(dt);
                }
            }
        }
    }

    // Leg 2: newest non-`.git`/non-`target` file mtime, shallow walk of the
    // worktree's top entries (best-effort; depth-bounded to stay cheap).
    if let Some(dt) = newest_file_mtime(worktree, 3) {
        consider(dt);
    }

    newest
}

/// TASK-834: newest file mtime under `dir`, skipping `.git` and `target`,
/// bounded to `max_depth` levels. Best-effort: unreadable entries are skipped.
/// trace:TASK-834 | ai:claude
pub(crate) fn newest_file_mtime(
    dir: &std::path::Path,
    max_depth: usize,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let mut newest: Option<chrono::DateTime<chrono::Utc>> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == ".git" || name == "target" {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if max_depth > 0 {
                if let Some(dt) = newest_file_mtime(&path, max_depth - 1) {
                    if newest.map(|cur| dt > cur).unwrap_or(true) {
                        newest = Some(dt);
                    }
                }
            }
        } else if let Ok(mtime) = meta.modified() {
            let dt: chrono::DateTime<chrono::Utc> = mtime.into();
            if newest.map(|cur| dt > cur).unwrap_or(true) {
                newest = Some(dt);
            }
        }
    }
    newest
}

/// TASK-834: format the per-agent activity label from a last-activity timestamp
/// and the current time. Pure — the unit-tested core of the in-flight activity
/// signal. `… active 15s ago` when recent; `… idle 15m possibly stuck` (with a warning) once
/// the gap exceeds [`ACTIVITY_STUCK_THRESHOLD_SECS`]. trace:TASK-834 | ai:claude
pub(crate) fn format_activity_label(
    last_activity: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let secs = now
        .signed_duration_since(last_activity)
        .num_seconds()
        .max(0);
    let rel = humanize_idle(secs);
    if secs >= ACTIVITY_STUCK_THRESHOLD_SECS {
        format!(
            "idle {rel} {} possibly stuck",
            crate::glyph(crate::glyphs::Glyph::Warning)
        )
    } else {
        format!("active {rel} ago")
    }
}

/// TASK-834: compact `Ns`/`Nm`/`Nh`/`Nd` for an elapsed-seconds gap (no " ago"
/// suffix — callers add their own framing). trace:TASK-834 | ai:claude
pub(crate) fn humanize_idle(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// TASK-806: human-readable `burndown status` summary. Pure over its inputs so
/// the three lock states render identically in tests and at the terminal.
/// trace:TASK-806 | ai:claude
pub(crate) fn render_burndown_status_human(
    lock: &drain_lock::LockStatus,
    in_flight: &[InFlightLease],
    log: Option<&std::path::Path>,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} burndown status",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    let _ = writeln!(out);

    match lock {
        drain_lock::LockStatus::None => {
            let _ = writeln!(out, "  {} no drain running", "○".dimmed());
            let _ = writeln!(out, "  {}", "start one with: aida burndown run".dimmed());
        }
        drain_lock::LockStatus::Running(l) => {
            let when = burndown_lock_when(&l.started_at_utc);
            let _ = writeln!(
                out,
                "  {} drain running — pid {}, started {}",
                "●".green().bold(),
                l.pid,
                when
            );
            let _ = writeln!(out, "    {}", l.command.dimmed());
            if !l.host.is_empty() {
                let _ = writeln!(out, "    {}", format!("host: {}", l.host).dimmed());
            }
        }
        drain_lock::LockStatus::Stale(l) => {
            let when = burndown_lock_when(&l.started_at_utc);
            let _ = writeln!(
                out,
                "  {} stale drain lock — pid {} (started {}) is no longer running",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                l.pid,
                when
            );
            let _ = writeln!(out, "    {}", l.command.dimmed());
            let _ = writeln!(
                out,
                "    {}",
                "the drain crashed or exited without releasing the lock; the next `aida burndown run` reclaims it"
                    .dimmed()
            );
        }
    }

    // In-flight leased worktrees — what the drain is working right now.
    if !in_flight.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  {}",
            format!("In-flight ({} leased):", in_flight.len()).bold()
        );
        // TASK-834: per-agent activity suffix so a committing agent reads
        // differently from one idle/stuck 15m. Computed against now from each
        // lease's captured last-activity; absent (worktree gone / probe failed)
        // → no suffix, the row still renders. trace:TASK-834
        let now = chrono::Utc::now();
        for f in in_flight {
            let _ = write!(
                out,
                "    {:<20} {:<18} {:<14} {}",
                truncate(&f.scope, 20),
                truncate(&f.branch, 18),
                truncate(&f.role, 14),
                f.worktree.dimmed()
            );
            if let Some(t) = f.last_activity {
                let label = format_activity_label(t, now);
                let secs = now.signed_duration_since(t).num_seconds().max(0);
                let painted = if secs >= ACTIVITY_STUCK_THRESHOLD_SECS {
                    label.yellow().to_string()
                } else {
                    label.green().to_string()
                };
                let _ = write!(out, "  {painted}");
            }
            let _ = writeln!(out);
        }
    }

    // Live log pointer — the TASK-804 event stream to tail.
    if let Some(path) = log {
        let _ = writeln!(out);
        let _ = writeln!(out, "  Live log: {}", path.display().to_string().cyan());
        let _ = writeln!(
            out,
            "    {}",
            format!("tail -f {}", path.display()).dimmed()
        );
    }

    out
}

/// TASK-806: render the start time as `local-time (Nm ago)`, falling back to the
/// raw RFC-3339 string if it does not parse. trace:TASK-806 | ai:claude
pub(crate) fn burndown_lock_when(started_at_utc: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(started_at_utc) {
        Ok(dt) => {
            let utc = dt.with_timezone(&chrono::Utc);
            let local = dt
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string();
            format!("{local} ({})", humanize_relative(utc))
        }
        Err(_) => started_at_utc.to_string(),
    }
}

/// TASK-806: machine-readable `burndown status`. trace:TASK-806 | ai:claude
pub(crate) fn render_burndown_status_json(
    lock: &drain_lock::LockStatus,
    in_flight: &[InFlightLease],
    log: Option<&std::path::Path>,
) -> String {
    let drain = match lock {
        drain_lock::LockStatus::None => serde_json::json!({ "running": false }),
        drain_lock::LockStatus::Running(l) => serde_json::json!({
            "running": true,
            "stale": false,
            "pid": l.pid,
            "started_at": l.started_at_utc,
            "command": l.command,
            "host": l.host,
        }),
        drain_lock::LockStatus::Stale(l) => serde_json::json!({
            "running": false,
            "stale": true,
            "pid": l.pid,
            "started_at": l.started_at_utc,
            "command": l.command,
            "host": l.host,
        }),
    };
    let in_flight_json: Vec<serde_json::Value> = in_flight
        .iter()
        .map(|f| {
            // TASK-834: surface the activity signal to machine consumers too —
            // `last_activity` (RFC-3339) plus a derived `stuck` flag. null when
            // the worktree probe found nothing. trace:TASK-834
            let now = chrono::Utc::now();
            let stuck = f.last_activity.map(|t| {
                now.signed_duration_since(t).num_seconds() >= ACTIVITY_STUCK_THRESHOLD_SECS
            });
            serde_json::json!({
                "spec": f.scope,
                "branch": f.branch,
                "role": f.role,
                "worktree": f.worktree,
                "last_activity": f.last_activity.map(|t| t.to_rfc3339()),
                "stuck": stuck,
            })
        })
        .collect();
    let value = serde_json::json!({
        "drain": drain,
        "in_flight": in_flight_json,
        "log": log.map(|p| p.display().to_string()),
    });
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
}

/// TASK-804: launch the `--verbose` burndown drain. Same headless `claude -p`
/// drain as the quiet path (same auto-proceed, same exit-code semantics), but
/// the stream-json stdout is redirected to `.aida/burndown/<drain-id>.jsonl`
/// and a background tee renders a human-readable progress line per event to
/// stderr — so a long drain shows live progress instead of a silent terminal.
/// Reuses the proven `headless_tee` machinery (TASK-307) rather than rolling a
/// new renderer. trace:TASK-804 | ai:claude
pub(crate) fn run_burndown_verbose(prompt: &str, mode: &str) -> Result<std::process::ExitStatus> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    // TASK-1169 / ADR-22: same bounded ceiling the quiet launch sets.
    // trace:TASK-1169 | ai:claude
    let (ceiling_key, ceiling_value) = bg_wait_ceiling_env(Some(&project_root));
    // Time-prefixed id so logs sort chronologically and are easy to tail; the
    // short uuid suffix disambiguates two drains started in the same second.
    let drain_id = format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        &uuid::Uuid::now_v7().to_string()[..8]
    );
    let log_path = burndown_drain_log_path(&project_root, &drain_id);
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            anyhow::anyhow!(
                "could not create the burndown log dir {} ({e})",
                dir.display()
            )
        })?;
    }
    let log = std::fs::File::create(&log_path).map_err(|e| {
        anyhow::anyhow!(
            "could not create the burndown log {} ({e})",
            log_path.display()
        )
    })?;

    println!(
        "  {} live progress → {} (tail it live, or read it after)",
        "→".green(),
        log_path.display().to_string().cyan()
    );
    println!();

    // Tee the JSONL to stderr as human-readable lines. `from_env_and_flag(false)`
    // keeps the routine chatter on by default (the operator asked for `--verbose`),
    // while `AIDA_TEE_HEADLESS=0` can quiet it — errors/denials still surface loud.
    let tee_opts = headless_tee::TeeOptions::from_env_and_flag(false).with_label("burndown");
    let tee_handle = headless_tee::start_tee(&log_path, &tee_opts);

    let child_grant =
        seat_authority::issue_child(&project_root, "implementer", &current_user_id(None))?;
    let status_code = std::process::Command::new("claude")
        .args(burndown_verbose_claude_args(prompt, mode))
        .stdout(std::process::Stdio::from(log))
        // BUG-607: the launcher holds the exclusive drain lock — the agent must
        // trust it and not self-detect a "competing" drain (see the quiet path).
        .env("AIDA_BURNDOWN_LOCK_HELD", "1")
        .env("AIDA_SESSION_ROLE", "implementer")
        .env(seat_authority::GRANT_ENV, child_grant.id)
        // TASK-1169 / ADR-22: identical bounded ceiling to the quiet path —
        // `--verbose` adds visibility, never a behaviour fork.
        // trace:TASK-1169 | ai:claude
        .env(ceiling_key, ceiling_value)
        .status()
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to launch `claude -p` ({e}) — the headless drain needs the Claude Code \
                 CLI on PATH. Install it, or use `aida queue work --auto-complete` (the \
                 orchestrator drain) instead."
            )
        });
    // Always stop the tee, even on spawn failure, so the background thread exits.
    tee_handle.stop();
    status_code
}

/// STORY-547: build the [`burndown::OpenFacts`] for every OPEN spec in the store
/// (non-archived, status not Completed/Rejected). Reasons are derived purely
/// from store signals + the live lease set — no new stored field. Shared by
/// `burndown explain` and `aida why`. trace:STORY-547 | ai:claude
pub(crate) fn collect_open_facts(
    store: &aida_core::RequirementsStore,
    // BUG-511: lowercased live-lease scope → role, so the explainer can say
    // "being reviewed" when the holder is a reviewer. From
    // [`in_flight_lease_role_map`].
    in_flight_scopes: &std::collections::HashMap<String, Option<String>>,
) -> Vec<burndown::OpenFacts> {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    };
    // TASK-723 (source #2): index FINDINGS by the spec they were filed against,
    // so each open spec can link its attempt-outcome findings without
    // recomputing them. A finding is a draft requirement carrying a
    // `from-implementer:<SPEC>` / `from-advisor:<SPEC>` origin tag, plus any
    // extra `linked:<SPEC>` tags. We reuse the existing finding cohort — single
    // source of truth for attempt outcomes. trace:TASK-723 | ai:claude
    let findings_by_spec = collect_findings_by_spec(store);
    let mut facts = Vec::new();
    for req in &store.requirements {
        if req.archived {
            continue;
        }
        // BUG-556: deferred is a view-flag orthogonal to status (STORY-584) —
        // `aida defer` parks a spec as primed/conditional work, hidden from
        // default views. The human worklist and `burndown explain` ARE default
        // views, so a deferred spec must drop out here exactly like an archived
        // one. (`explain_open` only recognized the legacy `deferred:` tag, not
        // the structural flag, so deferred specs leaked onto `aida human` as
        // ungroomed/umbrella.) trace:BUG-556 | ai:claude
        if req.deferred {
            continue;
        }
        // BUG-593: standing-artifact types (meta / folder / vision / principle /
        // term / constraint) are perpetual reference rows, NOT backlog work —
        // the same set `aida list` and `aida status` (BUG-464 / TASK-773) hide
        // from open-work views. Without this, a fresh `aida init`'s 6 seeded
        // META prompts surfaced on the advisor worklist's GROOM bucket (and on
        // `aida human`) as if an advisor should approve/reject AI-prompt
        // templates. trace:BUG-593 | ai:claude
        if is_standing_artifact_type(&format!("{:?}", req.req_type)) {
            continue;
        }
        // "Open" = not yet terminal. Completed/Rejected are done with.
        if matches!(
            req.status,
            aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
        ) {
            continue;
        }
        let id = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        let in_flight_entry = [req.agreed_id.as_deref(), req.spec_id.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|s| in_flight_scopes.get(&s.to_ascii_lowercase()));
        let in_flight = in_flight_entry.is_some();
        let in_flight_role = in_flight_entry.cloned().flatten();
        // TASK-723 (source #2): findings linked to this spec (deduped, stable
        // order). Don't link a finding to itself.
        let mut findings: Vec<String> = findings_by_spec
            .get(&id.to_ascii_uppercase())
            .cloned()
            .unwrap_or_default();
        findings.retain(|fid| !fid.eq_ignore_ascii_case(&id));
        // TASK-723 (source #3): residual `why-open:` notes recorded on this
        // spec's comments. Derivable state is never written here — these are
        // the non-derivable tail only.
        let residual_notes: Vec<String> = req
            .comments
            .iter()
            .filter_map(|c| burndown::parse_why_open_comment(&c.content))
            .collect();
        // BUG-543: for an epic, compute its child rollup (the same walk + tally
        // `aida graph tree` prints) so `explain_open` can surface a fully-
        // delivered epic ("N/N children Completed") as ready-to-close rather
        // than the generic umbrella. `None` for non-epics and childless epics.
        // TASK-884: also note whether any child is actually in motion
        // (Completed / Done / InProgress). An epic with children that are all
        // un-started reads as `Umbrella` (decompose lane), not the invisible
        // InProgress bucket. trace:TASK-884
        let (epic_rollup, epic_children_in_motion) =
            if matches!(req.req_type, aida_core::RequirementType::Epic) {
                let r = aida_core::graph_walk::child_status_rollup(store, req.id);
                let in_motion = r.completed + r.done + r.in_progress > 0;
                ((r.total > 0).then_some((r.completed, r.total)), in_motion)
            } else {
                (None, false)
            };
        facts.push(burndown::OpenFacts {
            id,
            req_type: format!("{:?}", req.req_type).to_ascii_lowercase(),
            status: norm(&format!("{:?}", req.status)),
            tags: req.tags.iter().cloned().collect(),
            has_unsatisfied_blocker: aida_core::pickability::blocked_by_incomplete(req, store),
            has_pending_decision: req
                .decision_request
                .as_ref()
                .map(|d| d.is_pending())
                .unwrap_or(false),
            in_flight,
            in_flight_role,
            findings,
            residual_notes,
            epic_rollup,
            // BUG-564: carry the spec's orthogonal human-only marker (the same
            // `req.human_only` the queue's "Blocked — human-only" bucket and the
            // spec-card `[human-only]` chip read) so the human worklist folds it
            // into `human_required` instead of a hardcoded `false`. A spec that
            // is human-required PURELY via this marker now surfaces in
            // `aida list human` exactly as it does in the queue.
            // trace:BUG-564 | ai:claude
            human_only: req.human_only,
            epic_children_in_motion,
        });
    }
    facts
}

/// TASK-723 (source #2): map UPPERCASE origin SPEC-ID → display-ids of the
/// findings filed against it. A finding is a draft requirement carrying a
/// `from-implementer:<SPEC>` or `from-advisor:<SPEC>` origin tag (and possibly
/// extra `linked:<SPEC>` tags). Reuses the existing finding cohort so the
/// explainer links — never recomputes — attempt outcomes. trace:TASK-723 | ai:claude
pub(crate) fn collect_findings_by_spec(
    store: &aida_core::RequirementsStore,
) -> std::collections::HashMap<String, Vec<String>> {
    let mut by_spec: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for req in &store.requirements {
        if req.archived {
            continue;
        }
        // Only findings still AWAITING TRIAGE count as open. `aida findings
        // list` defines that set as draft requirements carrying a finding tag
        // (`ListFilter { status: draft }` + `is_finding`); a finding that's
        // been resolved/escalated/promoted/dismissed leaves draft but may keep
        // its `from-*` tag. Gating on draft here keeps `aida human`'s open-
        // findings count equal to `aida findings list`'s awaiting count rather
        // than counting every finding row ever filed. trace:BUG-544
        if !matches!(req.status, RequirementStatus::Draft) {
            continue;
        }
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        if !findings::is_finding(&tags) {
            continue;
        }
        let finding_id = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        // Every SPEC-ID this finding points at: the origin tag's value plus any
        // `linked:<SPEC>` tags. `general` / non-spec origins are ignored.
        for tag in &tags {
            let t = tag.trim();
            let target = t
                .strip_prefix(findings::FROM_IMPLEMENTER_PREFIX)
                .or_else(|| t.strip_prefix(findings::FROM_ADVISOR_PREFIX))
                .or_else(|| t.strip_prefix(findings::LINKED_PREFIX));
            if let Some(spec) = target {
                let spec = spec.trim();
                if spec.is_empty() || spec.eq_ignore_ascii_case("general") {
                    continue;
                }
                let entry = by_spec.entry(spec.to_ascii_uppercase()).or_default();
                if !entry.iter().any(|e| e.eq_ignore_ascii_case(&finding_id)) {
                    entry.push(finding_id.clone());
                }
            }
        }
    }
    by_spec
}

/// STORY-547: `aida burndown explain` — classify every open spec into its
/// "why still open" bucket + one-line reason. The post-burndown companion to
/// `plan`: where `plan` answers "what can I fan out?", `explain` answers "why
/// is everything that's left still here?". trace:STORY-547 | ai:claude
pub(crate) fn handle_burndown_explain(json: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;
    let in_flight_scopes = in_flight_lease_role_map(&project_root);
    let facts = collect_open_facts(&store, &in_flight_scopes);

    // Classify, preserving store order within each bucket. TASK-723: each spec
    // now carries the FULL reason set (derived + finding-links + residual
    // notes), most-fundamental-first; the derived reason still drives the
    // bucket. trace:TASK-723
    let classified: Vec<(
        burndown::OpenFacts,
        burndown::OpenBucket,
        Vec<burndown::Reason>,
    )> = facts
        .into_iter()
        .map(|f| {
            let (bucket, reasons) = burndown::explain_reasons(&f);
            (f, bucket, reasons)
        })
        .collect();

    if json {
        let payload = classified
            .iter()
            .map(|(f, bucket, reasons)| {
                serde_json::json!({
                    "spec": f.id,
                    "bucket": bucket.key(),
                    // The primary (derived) reason — first, most-fundamental.
                    "reason": reasons.first().map(|r| r.text.as_str()).unwrap_or_default(),
                    // TASK-723: the full reason set, most-fundamental-first.
                    "reasons": reasons
                        .iter()
                        .map(|r| serde_json::json!({"source": r.source.key(), "text": r.text}))
                        .collect::<Vec<_>>(),
                    "findings": f.findings,
                    "needs_human": bucket.needs_human(),
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!(
        "{} why each open spec is still open",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    if classified.is_empty() {
        println!("  {}", "Nothing open — the backlog is clear.".dimmed());
        return Ok(());
    }
    let needs = classified
        .iter()
        .filter(|(_, b, _)| b.needs_human())
        .count();
    println!(
        "  {} {} open · {} need a human nudge, {} self-resolve through normal flow",
        "→".green(),
        classified.len(),
        needs,
        classified.len() - needs
    );

    // Group by bucket for a scannable read; ordering matches the precedence in
    // explain_open so the "needs you" buckets surface first.
    use burndown::OpenBucket::*;
    let order = [
        HeldForReview,
        AwaitingDecision,
        ReadyToClose,
        BuildSupervised,
        Ungroomed,
        Umbrella,
        Blocked,
        Deferred,
        InFlight,
        InProgress,
        AwaitingMerge,
        LongLived,
        Actionable,
    ];
    for bucket in order {
        let rows: Vec<&(
            burndown::OpenFacts,
            burndown::OpenBucket,
            Vec<burndown::Reason>,
        )> = classified.iter().filter(|(_, b, _)| *b == bucket).collect();
        if rows.is_empty() {
            continue;
        }
        let marker = if bucket.needs_human() {
            "●".yellow()
        } else {
            "·".dimmed()
        };
        // The bucket header glosses the derived reason — the first reason of the
        // first row (every row in this bucket shares the same bucket).
        let sample_reason = rows[0]
            .2
            .first()
            .map(|r| r.text.clone())
            .unwrap_or_default();
        println!(
            "\n{} {} — {}",
            marker,
            bucket.key().bold(),
            sample_reason.dimmed()
        );
        for (f, _, reasons) in rows {
            // The derived (first) reason: only repeat it when it differs from the
            // bucket header (e.g. `deferred:<why>` / decision tags vary per spec);
            // for uniform buckets just list the ID to keep the view scannable.
            let derived = reasons.first().map(|r| r.text.clone()).unwrap_or_default();
            if derived == sample_reason {
                println!("    {}", f.id.cyan());
            } else {
                println!("    {} {}", f.id.cyan(), format!("({derived})").dimmed());
            }
            // TASK-723: the additional reasons (finding-links + residual notes)
            // hang under the spec, most-fundamental-first, so the operator sees
            // every reason it's still open — not just the derived one.
            for r in reasons.iter().skip(1) {
                let tag = match r.source {
                    burndown::ReasonSource::Finding => "finding".magenta(),
                    burndown::ReasonSource::Residual => "note".yellow(),
                    burndown::ReasonSource::Derived => continue,
                };
                println!("      {} {}", tag, r.text.dimmed());
            }
        }
    }
    Ok(())
}

// TASK-773: perpetual standing-artifact types — the docs-layer / structural
// types that are NOT work and should be hidden from the default `aida list`
// open-work view. They surface via an explicit `--type <T>` filter or `--all`.
// `req_type` here is the cache summary's Display string (e.g. "Vision").
// trace:TASK-773
pub(crate) const STANDING_ARTIFACT_TYPES: [&str; 6] = [
    "vision",
    "principle",
    "term",
    "constraint",
    "folder",
    "meta",
];

// pub(crate) so the MCP status surface can mirror this exact exclusion (BUG-717).
pub(crate) fn is_standing_artifact_type(req_type: &str) -> bool {
    STANDING_ARTIFACT_TYPES
        .iter()
        .any(|s| req_type.eq_ignore_ascii_case(s))
}

#[cfg(test)]
#[path = "tests/task_773_standing_artifact_tests.rs"]
mod task_773_standing_artifact_tests;

#[cfg(test)]
#[path = "tests/bug_544_open_findings_count_tests.rs"]
mod bug_544_open_findings_count_tests;

#[cfg(test)]
#[path = "tests/bug_556_deferred_excluded_from_open_facts_tests.rs"]
mod bug_556_deferred_excluded_from_open_facts_tests;

/// STORY-611: one row in the "Reviews awaiting you" bucket — a spec with a
/// code-review surface that the human still has to act on.
// trace:STORY-611 | ai:claude
pub(crate) struct ReviewAwaiting {
    pub(crate) spec_id: String,
    /// Short human-readable surface label (e.g. "PR-42" / "branch foo").
    pub(crate) surface: String,
    /// True when an Approved reviewer verdict already exists for the PR — the
    /// spec is in the awaiting-MERGE micro-state, NOT awaiting review.
    pub(crate) reviewed: bool,
    /// BUG-722: true when this is a loose draft WIP branch (a `Draft` spec on a
    /// pushed branch with commits but no open PR). Rendered under `wip-branches`
    /// but NOT a review gate — nothing awaits a human until there's a PR or a
    /// Done claim.
    // trace:BUG-722 | ai:claude
    pub(crate) wip: bool,
}

/// STORY-611: classify whether an open PR already carries an Approved reviewer
/// verdict (the State-3 "reviewed + approved → awaiting merge" signal). Reuses
/// the same verdict-file convention the orchestrator-resume probe reads:
/// `.aida/review-verdicts/PR-{n}.json`. trace:STORY-611 | ai:claude
pub(crate) fn pr_has_approved_verdict(project_root: &std::path::Path, pr_number: u64) -> bool {
    let path = project_root
        .join(".aida")
        .join("review-verdicts")
        .join(format!("PR-{pr_number}.json"));
    matches!(
        read_verdict_file(&path),
        Ok(auto_complete::ReviewerOutcome::Verdict(
            auto_complete::Verdict::Approved
        ))
    )
}

/// BUG-1672: the verdict a review COMMENT on the spec carries, when the
/// comment is one. Reviewers that only post prose (a fresh reviewer subagent,
/// a human at the keyboard) write no verdict file at all, so the comment is
/// the only record of the decision.
///
/// Only a comment that is plainly a review counts: its first line starts with
/// `VERDICT:`, or its head (the text before the first `:`) is `Review` or
/// begins `Review ` / `Review(` — `Review:`, `Review (fresh Opus reviewer):`,
/// `Review round 4 (...):`. Anything else (a proxy decision quoting a review,
/// an orchestrator note, the `REVIEW FINDINGS TO ADDRESS (` rework block) is
/// not a review. The verdict is read from the first line only: a
/// request-changes / reject word blocks regardless of anything else on the
/// line; `PARTIAL` counts as request-changes (as `VerdictKind::parse` does);
/// an approval word approves unless the line qualifies it (`WITHHELD`,
/// `PENDING`, `NOT APPROVED`), in which case it is `Unknown`, which never
/// approves. `APPROVE WITH NITS` is an approval — the wording the review
/// skill's accepted-with-nits path posts.
///
/// Returns `None` for a comment that is not a review at all.
// trace:BUG-1672 | ai:claude
pub(crate) fn review_comment_verdict(content: &str) -> Option<review_verdict::VerdictKind> {
    if review_verdict::is_findings_block(content) {
        return None;
    }
    let first = content.lines().find(|l| !l.trim().is_empty())?.trim();
    let lower = first.to_ascii_lowercase();
    let head = lower.split(':').next().unwrap_or(&lower).trim();
    let is_review = lower.starts_with("verdict:")
        || head == "review"
        || head.starts_with("review ")
        || head.starts_with("review(");
    if !is_review {
        return None;
    }
    // Word-level scan of the upper-cased first line: verdict words are written
    // in capitals by every writer, and the whole-word match keeps a prose
    // "reviewer" or "approved-by" from counting.
    let upper = first.to_ascii_uppercase();
    let words: Vec<&str> = upper
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .map(|w| w.trim_matches('_'))
        .filter(|w| !w.is_empty())
        .collect();
    let has = |w: &str| words.contains(&w);
    let has_seq = |a: &str, b: &str| words.windows(2).any(|p| p[0] == a && p[1] == b);
    let request_changes = has("REQUEST_CHANGES")
        || has("REQUESTCHANGES")
        || has_seq("REQUEST", "CHANGES")
        || has_seq("CHANGES", "REQUESTED")
        || has_seq("NEEDS", "CHANGES");
    if request_changes || has("PARTIAL") {
        return Some(review_verdict::VerdictKind::RequestChanges);
    }
    if has("REJECT") || has("REJECTED") {
        return Some(review_verdict::VerdictKind::Rejected);
    }
    if has("APPROVE") || has("APPROVED") || has("LGTM") {
        let qualified = has("WITHHELD")
            || has("PENDING")
            || has_seq("NOT", "APPROVED")
            || has_seq("NOT", "APPROVE");
        return Some(if qualified {
            review_verdict::VerdictKind::Unknown
        } else {
            review_verdict::VerdictKind::Approved
        });
    }
    Some(review_verdict::VerdictKind::Unknown)
}

/// BUG-1672: the LATEST review comment on the spec and the verdict it
/// carries. Top-level comments only, newest by `created_at`; the newest
/// comment that is a review decides, so a round-4 APPROVE after a round-3
/// REQUEST_CHANGES approves and a REQUEST_CHANGES after an earlier APPROVE
/// blocks.
///
/// Every review comment counts, whoever posted it: the author field is the
/// operator's name on reviewer and implementer comments alike, and the
/// session that moved the spec to done is not recorded in its history, so
/// there is no reliable way to tell the implementer's own "looks good" apart.
/// The strict first-line shape `review_comment_verdict` requires is the
/// guard instead.
// trace:BUG-1672 | ai:claude
pub(crate) fn latest_review_comment_verdict(
    comments: &[aida_core::Comment],
) -> Option<(review_verdict::VerdictKind, chrono::DateTime<chrono::Utc>)> {
    comments
        .iter()
        .filter_map(|c| review_comment_verdict(&c.content).map(|k| (k, c.created_at)))
        // trace:BUG-1672 | ai:codex — equal timestamps cannot clear a blocker.
        .max_by_key(|(kind, at)| (*at, !kind.approves()))
}

/// BUG-1672: when a verdict file was last written, as UTC. `None` when the
/// file is missing or the filesystem gives no modification time.
// trace:BUG-1672 | ai:claude
pub(crate) fn verdict_file_written_at(
    path: &std::path::Path,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(chrono::DateTime::<chrono::Utc>::from(modified))
}

/// BUG-1672: has an open PR for `req` already been approved by an independent
/// review? The one predicate `reviews_awaiting_human` uses to split
/// awaiting-review from awaiting-merge. Three records can approve:
///  1. the drain's PR-keyed verdict file (`PR-<n>.json`, the original signal);
///  2. the spec-keyed verdict file `aida review <SPEC>` records (not one a
///     merge has already closed out);
///  3. the latest review comment on the spec (the fresh-reviewer flow posts
///     only a comment).
///
/// The newest review wins across all eligible sources. An approval must be
/// strictly newer than every blocking candidate; ties or missing timestamps
/// cannot clear a blocker. File times remain filesystem mtimes (BUG-1681 owns
/// switching to recorded_at).
// trace:BUG-1672 | ai:codex
pub(crate) fn spec_review_approved(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
    spec_id: &str,
    pr_number: u64,
) -> bool {
    let mut candidates = Vec::new();
    if let Some((kind, at)) = latest_review_comment_verdict(&req.comments) {
        candidates.push((kind.approves(), Some(at)));
    }

    let pr_path = review_verdict::verdict_path(project_root, &format!("PR-{pr_number}"));
    if pr_path.is_file() {
        // Preserve the drain reader's conflict/escalation checks. An unknown
        // or unreadable verdict is blocking evidence, never an approval.
        candidates.push((
            pr_has_approved_verdict(project_root, pr_number),
            verdict_file_written_at(&pr_path),
        ));
    }

    let ids = [
        Some(spec_id),
        req.agreed_id.as_deref(),
        req.spec_id.as_deref(),
    ];
    for path in ids
        .into_iter()
        .flatten()
        .filter(|id| !id.trim().is_empty() && *id != "???")
        .map(|id| review_verdict::verdict_path(project_root, id))
        .filter(|p| p.is_file())
    {
        let record = std::fs::read_to_string(&path)
            .ok()
            .and_then(|body| review_verdict::parse_recorded_verdict(&body));
        if record.as_ref().is_some_and(|v| v.is_closed()) {
            continue;
        }
        candidates.push((
            record.is_some_and(|v| v.kind.approves()),
            verdict_file_written_at(&path),
        ));
    }
    review_candidates_approved(&candidates)
}

// trace:BUG-1672 | ai:codex
pub(crate) fn review_candidates_approved(
    candidates: &[(bool, Option<chrono::DateTime<chrono::Utc>>)],
) -> bool {
    candidates.iter().any(|(approves, at)| {
        *approves
            && candidates.iter().all(|(other_approves, other_at)| {
                *other_approves
                    || matches!((at, other_at), (Some(at), Some(other_at)) if at > other_at)
            })
    })
}

/// BUG-1291: any valid local reviewer decision means this PR has already
/// been reviewed. The orphan sweep must not turn RequestChanges or Rejected
/// back into fresh reviewer work merely because GitHub has no decision.
// trace:BUG-1291 | ai:codex
pub(crate) fn pr_has_local_verdict(project_root: &std::path::Path, pr_number: u64) -> bool {
    let path = project_root
        .join(".aida")
        .join("review-verdicts")
        .join(format!("PR-{pr_number}.json"));
    matches!(
        read_verdict_file(&path),
        Ok(auto_complete::ReviewerOutcome::Verdict(_))
            | Ok(auto_complete::ReviewerOutcome::EscalatedToHuman { .. })
    )
}

/// BUG-1549: gather every locally-recorded verdict candidate for `pr` — every
/// spec id parsed from the PR title (ALL of them, in title order, each read
/// directly rather than stopping at the first file on disk the way the old
/// `review_verdict::read_recorded_verdict_any` did), plus the PR-keyed
/// record. The ONLY verdict read on the awaiting-you review path: its
/// result feeds `awaiting_you::classify_pr_review` via `pr_review_decision`.
// trace:BUG-1549 | ai:claude
pub(crate) fn verdict_candidates_for_pr(
    project_root: &std::path::Path,
    pr: &status_cleanup::OpenPrItem,
) -> Vec<review_verdict::RecordedVerdict> {
    let mut candidates: Vec<review_verdict::RecordedVerdict> =
        pr_ship::extract_spec_ids_from_text(&pr.title)
            .iter()
            .filter(|id| !id.trim().is_empty())
            .filter_map(|id| review_verdict::read_recorded_verdict(project_root, id))
            .collect();
    if let Some(pr_verdict) =
        review_verdict::read_recorded_verdict(project_root, &format!("PR-{}", pr.number))
    {
        candidates.push(pr_verdict);
    }
    candidates
}

/// BUG-1549: the ONE review decision for `pr` — `awaiting_you::classify_pr_review`
/// over `verdict_candidates_for_pr`. Both the mergeable suppression set
/// (`local_suppressed_prs`) and the review rows (`pr_review_rows`) derive
/// from this and nothing else, so "suppressed" and "has a row explaining it"
/// cannot disagree.
// trace:BUG-1549 | ai:claude
pub(crate) fn pr_review_decision(
    project_root: &std::path::Path,
    pr: &status_cleanup::OpenPrItem,
) -> awaiting_you::PrReviewDecision {
    let candidates = verdict_candidates_for_pr(project_root, pr);
    awaiting_you::classify_pr_review(&candidates, pr.head_sha.as_deref())
}

/// BUG-1549: the set of PR numbers a LOCAL (no-network) verdict record
/// removes from the mergeable set — exactly the PRs whose
/// `pr_review_decision` is `suppressed`.
// trace:BUG-1549 | ai:claude
pub(crate) fn local_suppressed_prs(
    project_root: &std::path::Path,
    prs: &[status_cleanup::OpenPrItem],
) -> std::collections::HashSet<u64> {
    prs.iter()
        .filter(|pr| pr_review_decision(project_root, pr).suppressed)
        .map(|pr| pr.number)
        .collect()
}

/// BUG-1549: the report's review rows (blocked / stale-approval /
/// rework-ready) for `prs`, each rendered from that PR's
/// `pr_review_decision`. A head-less PR is not skipped: its decision can
/// suppress, so its row must exist.
// trace:BUG-1549 | ai:claude
// STORY-1420: production uses `pr_review_rows_routed`; this keeps the
// STORY-1419 every-recorder-live contract for the plumbing tests.
#[cfg(test)]
pub(crate) fn pr_review_rows<'a>(
    project_root: &std::path::Path,
    prs: impl IntoIterator<Item = &'a status_cleanup::OpenPrItem>,
    seat: Option<&str>,
) -> awaiting_you::PrReviewRows {
    let mut rows = awaiting_you::PrReviewRows::default();
    for pr in prs {
        let decision = pr_review_decision(project_root, pr);
        rows.add(
            pr.number,
            pr.head_sha.as_deref(),
            &pr.head_branch,
            &decision,
            seat,
        );
    }
    rows
}

/// STORY-1420: [`pr_review_rows`] with the rework-ready row routed by the
/// recorder's liveness (agent registry + pid, never the network).
// trace:STORY-1420 | ai:claude
pub(crate) fn pr_review_rows_routed<'a>(
    project_root: &std::path::Path,
    prs: impl IntoIterator<Item = &'a status_cleanup::OpenPrItem>,
    reader: awaiting_you::ReworkReader<'_>,
) -> awaiting_you::PrReviewRows {
    let mut rows = awaiting_you::PrReviewRows::default();
    let liveness = |who: &str| {
        awaiting_you::classify_recorder(who, |name| {
            agent_registry::named_agent_liveness(project_root, name)
        })
    };
    for pr in prs {
        let decision = pr_review_decision(project_root, pr);
        rows.add_routed(
            pr.number,
            pr.head_sha.as_deref(),
            &pr.head_branch,
            &decision,
            reader,
            liveness,
        );
    }
    rows
}

/// BUG-550: the set of SPEC-IDs referenced by commits that exist on some ref
/// but are NOT yet reachable from the default branch — i.e. specs with an
/// in-flight (unmerged) review surface. This is the cheap prefilter that lets
/// the reviews bucket widen its candidate set past the Done specs WITHOUT a
/// regression: one `git log --all --not <default>` pass replaces N per-spec
/// `git log --all --grep` history scans, and the full per-spec linkage probe
/// then runs only for this small set (the handful with open work) rather than
/// every active spec. A spec whose commits are all on the default branch is
/// merged (Shipped) and correctly absent — no false negatives for a genuinely
/// open surface; the at-worst false positive (local default behind origin)
/// is harmless, since the per-spec classifier re-checks and drops it. Ids are
/// resolved through the same trailer / `trace:` parsers `collect_git_linkage`
/// uses, so membership matches what the classifier later confirms.
/// trace:BUG-550 | ai:claude
pub(crate) fn specs_with_unmerged_commits(
    project_root: &std::path::Path,
) -> std::collections::HashSet<String> {
    use std::process::Command as PCmd;
    let mut out = std::collections::HashSet::new();
    let Some(default_ref) = resolve_default_branch_ref(project_root) else {
        return out;
    };
    let log = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "log",
            "--all",
            "--not",
            &default_ref,
            "--pretty=format:%B%x1e",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    for record in log.split('\u{1e}') {
        let body = record.trim();
        if body.is_empty() {
            continue;
        }
        let mut resolved = extract_spec_ids_from_commit(body);
        resolved.extend(extract_referenced_spec_ids_from_commit(body));
        resolved.extend(extract_trace_line_spec_ids(body));
        for r in resolved {
            out.insert(r.to_ascii_uppercase());
        }
    }
    out
}

/// STORY-611: enumerate the specs that have a code-review surface awaiting the
/// human — branch / open-PR specs not yet merged, the SAME surface `aida
/// review <SPEC>` locates. Candidate set (BUG-550) = every non-archived spec
/// that isn't `Completed` (merged) or `Rejected` (abandoned) — work that could
/// still carry an open PR, regardless of whether someone ran `aida queue
/// done`. For each we run the exact surface-detection `handle_review_spec`
/// uses (`collect_git_linkage_opts` + `change_lookup_for_branch` +
/// `classify_review_surface`) and keep only the `OpenChange` / `BranchNoChange`
/// surfaces; `Shipped` (already merged) and `Local` (never pushed) are dropped,
/// so the open-PR linkage — not the Done flag — is the real gate. Each row is
/// tagged `reviewed` so the caller can split awaiting-review from the
/// awaiting-MERGE micro-state (the REFINEMENT comment's case 3).
/// trace:STORY-611 trace:BUG-550 | ai:claude
pub(crate) fn reviews_awaiting_human(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
) -> Vec<ReviewAwaiting> {
    // BUG-550: the candidate set is "specs that could have an OPEN review
    // surface", NOT "specs someone remembered to mark Done". Gating on
    // `status == Done` made an open PR invisible whenever the PR was opened at
    // the keyboard without a `queue done` (observed: PR #879 / BUG-549 stayed
    // Approved). Substrate-as-bouncer: let the open-PR linkage be the gate
    // (`classify_review_surface` drops merged/never-pushed specs), not a
    // manually-maintained status flag.
    //
    // We still exclude Completed (merged ⇒ classifies as `Shipped` ⇒ dropped
    // anyway) and Rejected (abandoned, not headed to merge) purely to bound
    // cost — those are the bulk of a mature store. The remaining set
    // (Draft/Approved/Planned/InProgress/Done/NeedsAttention) is "active,
    // not-yet-shipped" work.
    //
    // To keep this hot, offline command fast even with the wider set, a single
    // `git log` (`specs_with_unmerged_commits`) names the specs that actually
    // have unmerged commits; the per-spec linkage probe — and its skip of the
    // source-tree walk via `collect_git_linkage_opts(.., false)` — then runs
    // ONLY for those. Active specs with nothing in flight cost nothing beyond
    // the one shared log pass. trace:BUG-550 | ai:claude
    let candidates: Vec<aida_core::Requirement> = match backend.list_requirements(false) {
        Ok(reqs) => reqs
            .into_iter()
            .filter(|r| {
                !r.archived
                    && !matches!(
                        r.status,
                        aida_core::RequirementStatus::Completed
                            | aida_core::RequirementStatus::Rejected
                    )
            })
            .collect(),
        Err(_) => return Vec::new(),
    };
    let unmerged = specs_with_unmerged_commits(project_root);
    let open_prs = collect_open_prs(project_root);
    let mut out = Vec::new();
    for req in &candidates {
        let Some(spec_id) = req.spec_id.clone().or_else(|| req.agreed_id.clone()) else {
            continue;
        };
        // BUG-550: skip the per-spec history scan for specs with no unmerged
        // commit — they can only classify as Shipped/Local and be dropped, so
        // probing them is pure cost. This is what keeps the widened candidate
        // set from regressing `aida human`'s latency. trace:BUG-550 | ai:claude
        if !unmerged.contains(&spec_id.to_ascii_uppercase()) {
            continue;
        }
        let ids = vec![spec_id.clone()];
        let linkage = collect_git_linkage_opts(project_root, &ids, false);
        let change = linkage
            .branch
            .as_deref()
            .map(|b| change_lookup_for_branch(project_root, b));
        let surface = classify_review_surface(&linkage, change);
        // BUG-582: the load-bearing invariant — a Completed/Rejected spec, or a
        // merged/never-pushed surface, is NEVER reviews-awaiting. This is the
        // gate the count and the render both flow through (the single `out`
        // vec), so they stay consistent (BUG-579). The candidate pre-filter
        // above already drops most of these for cost, but it reads the cache;
        // this re-checks the live status so a stale cache row left over from a
        // lingering Agent-tool worktree branch can't slip a finished spec back
        // onto the operator's seat. trace:BUG-582 | ai:claude
        // BUG-722: the single classification gate. Layers view-state (deferred/
        // archived hidden exactly as `aida list` hides them), branch-type (the
        // `aida-store` orphan store branch is never a code-review target), and
        // the draft-WIP split on top of the BUG-582 status/surface invariant.
        // Pure ⇒ the false-positive classes are unit-tested without a repo.
        // trace:BUG-722 | ai:claude
        let bucket = classify_human_review_bucket(
            req.status.clone(),
            req.archived,
            requirement_is_deferred(req),
            &surface,
        );
        if bucket == HumanReviewBucket::Excluded {
            continue;
        }
        let wip = bucket == HumanReviewBucket::WipBranch;
        match surface {
            ReviewSurface::OpenChange {
                number, ref branch, ..
            } => {
                if open_prs.by_branch.get(branch).is_some_and(|pr| pr.is_draft) {
                    continue;
                }
                let forge = crate::forge::resolve_forge_kind(project_root);
                // BUG-1672: the PR-keyed file is only the drain's record; a
                // keyboard `aida review` writes the spec-keyed file and a
                // fresh-reviewer subagent posts only a comment. Read all three
                // so an already-approved spec lands under awaiting-merge, not
                // back on the operator's review seat. trace:BUG-1672 | ai:claude
                let reviewed = spec_review_approved(project_root, req, &spec_id, number);
                out.push(ReviewAwaiting {
                    spec_id,
                    surface: format!("{}-{}", forge.change_noun(), number),
                    reviewed,
                    wip,
                });
            }
            ReviewSurface::BranchNoChange { branch, .. } => {
                out.push(ReviewAwaiting {
                    spec_id,
                    surface: format!("branch {branch}"),
                    // No PR ⇒ no verdict file ⇒ never the awaiting-merge state.
                    reviewed: false,
                    wip,
                });
            }
            // Already merged or never pushed — nothing for the human to review.
            // (Unreachable: `classify_human_review_bucket` already dropped these
            // above; kept exhaustive for the match.)
            ReviewSurface::Shipped { .. } | ReviewSurface::Local => {}
        }
    }
    out.sort_by(|a, b| a.spec_id.cmp(&b.spec_id));
    out
}

pub(crate) fn handle_list_human(short: bool, backend: &aida_core::CachedGitBackend) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;
    let in_flight_scopes = in_flight_lease_role_map(&project_root);
    let facts = collect_open_facts(&store, &in_flight_scopes);
    // STORY-620: the operator/advisor seat split is a resolved policy now — the
    // configurable buckets (to-groom/decompose/ready-to-close/triage) show here
    // only when assigned to the operator. trace:STORY-620
    let seat_policy = seats::SeatPolicy::load(&project_root);

    // trace:BUG-535 — display-id → title, built from the SAME store load the
    // facts came from (no extra scan, no N+1), so every bucket can render the
    // spec's title beside its bare id via the shared `spec_title_cell` helper
    // (the same id+title rendering BUG-532 added for `burndown plan`).
    let titles: std::collections::HashMap<String, String> = store
        .requirements
        .iter()
        .filter_map(|req| {
            let id = req
                .agreed_id
                .clone()
                .or_else(|| req.spec_id.clone())
                .unwrap_or_else(|| req.id.to_string());
            (!req.title.is_empty()).then(|| (id, req.title.clone()))
        })
        .collect();

    // TASK-747: specs explicitly routed `--for human` are part of the
    // human-attention set even when the status/tag classifier wouldn't derive
    // them. We UNION the explicitly-routed members with the derived membership
    // (the route is additive, never subtractive — Risk #6 in the SPIKE-57
    // design doc). trace:TASK-747 | ai:claude
    let routed_ids = human_routed_spec_ids(&project_root, &store);

    // Classify with the SAME classifier as `burndown explain`, then keep only
    // the specs the canonical `human_required` predicate (SPIKE-57/TASK-746)
    // classifies as needing a human — the rest self-resolve. Routing the filter
    // through the named predicate keeps this view and the predicate from
    // drifting. BUG-564: the orthogonal `human_only` marker is now folded in at
    // the view layer — `collect_open_facts` enriches each fact with
    // `req.human_only`, so a spec that is human-required PURELY via the marker
    // (e.g. init's High "Commit AIDA scaffolding" task, a non-human bucket)
    // surfaces here exactly as it does in the queue's human-only bucket, which
    // reads the SAME `req.human_only`. Before this fix the marker was fed as a
    // hardcoded `false`, so the two human views contradicted each other on a new
    // user's first interaction. trace:STORY-562 trace:TASK-746 trace:BUG-564
    let classified: Vec<(
        burndown::OpenFacts,
        burndown::OpenBucket,
        Vec<burndown::Reason>,
    )> = facts
        .into_iter()
        .map(|f| {
            let (bucket, reasons) = burndown::explain_reasons(&f);
            (f, bucket, reasons)
        })
        // STORY-618/620: keep the derived buckets assigned to the OPERATOR
        // seat. The fixed-operator buckets (held-for-review, build-supervised)
        // always qualify; the configurable buckets (to-groom / decompose /
        // ready-to-close) qualify only when the seat policy assigns them to the
        // operator (default: advisor → they show on `aida advisor`, not here).
        // `operator_seat` also excludes `AwaitingDecision`, rendered richer by
        // the first-class "decisions-awaiting" bucket below. trace:STORY-620
        .filter(|(f, bucket, _)| {
            // BUG-564: a spec carrying the `human_only` marker is human-required
            // regardless of its derived bucket — the canonical `human_required`
            // predicate ORs the marker in, and so must this view. Without this
            // clause a human-only spec whose bucket is NOT a needs-human bucket
            // (the init scaffolding-commit task is High/Approved → a non-human
            // bucket) was invisible here while the queue showed it.
            // trace:BUG-564 | ai:claude
            //
            // BUG-572: but `human_only` must NOT override the to-groom→advisor
            // seat routing. An ungroomed Draft (the `Ungroomed` / to-groom
            // bucket) carrying `human_only=true` is still advisor grooming work —
            // you cannot make a human decision on a draft that has no formulated
            // question. Excluding `Ungroomed` here keeps it off the operator seat;
            // `handle_advisor_worklist` still surfaces it under to-groom (its
            // filter gates on `advisor_seat()`, not `human_only`). A GROOMED
            // human_only spec (any non-`Ungroomed` bucket — e.g. BUG-564's
            // High/Approved scaffolding task) still reaches the operator.
            // trace:BUG-572 | ai:claude
            (f.human_only && !matches!(bucket, burndown::OpenBucket::Ungroomed))
                || bucket.operator_seat()
                || (seats::CONFIGURABLE_KEYS.contains(&bucket.key())
                    && seat_policy.seat_of(bucket.key()) == seats::Seat::Operator)
        })
        .collect();

    // TASK-747: the explicitly-routed members the derived classification did
    // NOT already surface — shown as their own "routed to you" group so the
    // operator sees push-routed work alongside the derived bottleneck. De-dupe
    // by SPEC-ID against the derived set (Risk #5). trace:TASK-747 | ai:claude
    let derived_ids: std::collections::HashSet<String> = classified
        .iter()
        .map(|(f, _, _)| f.id.to_ascii_uppercase())
        .collect();
    let mut routed_only: Vec<String> = routed_ids
        .iter()
        .filter(|id| !derived_ids.contains(*id))
        .cloned()
        .collect();
    routed_only.sort();

    // STORY-611: the THREE first-class pending-for-human buckets, each → its
    // drain verb. The banner already promises "a decision, review, or triage";
    // deliver all three as real lists, not pointers. trace:STORY-611
    //  1. Decisions awaiting you  → `aida questions answer <spec> <choice>`
    //  2. Reviews awaiting you    → `aida review <SPEC>` (the must-have)
    //  3. Triage (findings)       → `aida findings list`
    let pending_decisions = collect_decision_requests(backend)
        .map(|(pending, _)| pending)
        .unwrap_or_default();
    let reviews_all = reviews_awaiting_human(&project_root, backend);
    // BUG-722: loose draft WIP branches (a `Draft` spec on a pushed branch with
    // no PR) are visible but NOT a review gate — split them out first so they
    // never masquerade as reviews-awaiting. trace:BUG-722
    let wip_branches: Vec<&ReviewAwaiting> = reviews_all.iter().filter(|r| r.wip).collect();
    // Split case 1 (code-review-needed) from case 3 of the REFINEMENT comment
    // (already reviewed → awaiting MERGE, a distinct micro-state, NOT a review).
    let (awaiting_merge, awaiting_review): (Vec<&ReviewAwaiting>, Vec<&ReviewAwaiting>) =
        reviews_all
            .iter()
            .filter(|r| !r.wip)
            .partition(|r| r.reviewed);
    // Findings stay a pointer + live count (cheap, no recompute): open findings
    // targeting an open spec. STORY-620: triage is a configurable seat; it only
    // counts toward (and renders on) the operator's list when the policy
    // assigns `triage` to the operator (default: advisor → `aida advisor`).
    // trace:STORY-611 trace:STORY-620
    let open_findings = if seat_policy.seat_of("triage") == seats::Seat::Operator {
        collect_findings_by_spec(&store)
            .values()
            .flat_map(|ids| ids.iter())
            .collect::<std::collections::HashSet<_>>()
            .len()
    } else {
        0
    };

    // `--short`: bare IDs, one per line — usable in `$(...)` / xargs. No
    // header, footer, color, or grouping (mirrors `aida list --short`).
    // Derived members first, then the explicitly-routed-only tail (TASK-747).
    if short {
        for (f, _, _) in &classified {
            println!("{}", f.id);
        }
        for id in &routed_only {
            println!("{id}");
        }
        return Ok(());
    }

    println!(
        "{} what needs a human",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    let nothing = classified.is_empty()
        && routed_only.is_empty()
        && pending_decisions.is_empty()
        && awaiting_review.is_empty()
        && awaiting_merge.is_empty()
        && wip_branches.is_empty()
        && open_findings == 0;
    if nothing {
        println!(
            "  {}",
            "Nothing needs you right now — everything open is in flight, deferred, \
             or self-resolving."
                .dimmed()
        );
        return Ok(());
    }
    let total = classified.len()
        + routed_only.len()
        + pending_decisions.len()
        + awaiting_review.len()
        + open_findings;
    // BUG-722: the header counts true gates (decision/review/triage). When the
    // only open items are informational (awaiting-merge or loose wip-branches),
    // skip the "0 items need …" line and let those sections speak for
    // themselves. trace:BUG-722
    if total > 0 {
        println!(
            "  {} {} open {} need a decision, review, or triage from you",
            "→".green(),
            total,
            if total == 1 { "item" } else { "items" },
        );
    }

    // STORY-611 — bucket 1: Decisions awaiting you. Upgrade from the old
    // one-line pointer to a real list of the pending DecisionRequests; each
    // resolvable inline with `aida human answer <spec> <choice>`.
    // trace:STORY-611
    if !pending_decisions.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            "decisions-awaiting".bold(),
            "answer with `aida human answer <spec> <choice>`".dimmed()
        );
        for req in &pending_decisions {
            let display_id = req.display_id();
            let title = spec_title_cell(&titles, &display_id);
            println!("    {}{}", display_id.cyan(), title);
            if let Some(dr) = &req.decision_request {
                println!("      {} {}", "?".magenta(), dr.question.dimmed());
                for (i, choice) in dr.choices.iter().enumerate() {
                    let rec = if dr.recommended == Some(i) {
                        " (recommended)".green().to_string()
                    } else {
                        String::new()
                    };
                    println!(
                        "        {}{} {}",
                        format!("{}.", i + 1).bold(),
                        rec,
                        choice.label.dimmed()
                    );
                }
            }
        }
    }

    // STORY-611 — bucket 2: Reviews awaiting you (the must-have). Specs with a
    // code-review surface (open PR / branch-with-commits) not yet merged — the
    // SAME surface `aida review <SPEC>` locates. Each line → `aida review
    // <SPEC>` (or the `aida human review <SPEC>` alias). trace:STORY-611
    if !awaiting_review.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            "reviews-awaiting".bold(),
            "review with `aida human review <spec>` (or `aida review <spec>`)".dimmed()
        );
        for r in &awaiting_review {
            let title = spec_title_cell(&titles, &r.spec_id);
            println!(
                "    {}{} {}",
                r.spec_id.cyan(),
                title,
                format!("[{}]", r.surface).dimmed()
            );
        }
    }

    // STORY-611 — the awaiting-MERGE micro-state (REFINEMENT case 3): already
    // reviewed + approved, just needs the merge button. A distinct indicator,
    // NOT part of the reviews bucket. trace:STORY-611
    if !awaiting_merge.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".green(),
            "awaiting-merge".bold(),
            "reviewed + approved — just needs merging".dimmed()
        );
        for r in &awaiting_merge {
            let title = spec_title_cell(&titles, &r.spec_id);
            println!(
                "    {}{} {}",
                r.spec_id.cyan(),
                title,
                format!("[{}]", r.surface).dimmed()
            );
        }
    }

    // BUG-722 — loose work-in-progress: a draft spec sitting on a pushed WIP
    // branch with no PR. Surfaced so it isn't invisible, but explicitly NOT a
    // review gate — nothing awaits the human until there's a PR or a Done claim
    // (operator decision 2026-07-12). A hollow marker distinguishes it from the
    // solid gate buckets above. trace:BUG-722
    if !wip_branches.is_empty() {
        println!(
            "\n{} {} — {}",
            "○".dimmed(),
            "wip-branches".bold(),
            "loose draft work-in-progress — not awaiting review".dimmed()
        );
        for r in &wip_branches {
            let title = spec_title_cell(&titles, &r.spec_id);
            println!(
                "    {}{} {}",
                r.spec_id.cyan(),
                title,
                format!("[{}]", r.surface).dimmed()
            );
        }
    }

    // Group by bucket, ordered most-actionable-first (matches the precedence in
    // `explain_open` / the explain view). trace:STORY-562
    //
    // STORY-611: `AwaitingDecision` is intentionally DROPPED from this derived
    // set — the first-class "decisions-awaiting" bucket above supersedes it
    // (same specs, richer output: the actual choices). Keeping both would
    // double-list every pending decision, exactly the conflation the
    // REFINEMENT comment warns against. trace:STORY-611
    // STORY-618/620: render order for the derived buckets. The fixed-operator
    // buckets (held-for-review, build-supervised) always belong here; the
    // configurable ones (ready-to-close / to-groom / decompose) appear only when
    // the seat policy moved them to the operator — the `classified` filter above
    // already dropped the rest, so listing them here just sets their order.
    // trace:STORY-620 (was BUG-543's ReadyToClose-leads order)
    // BUG-579: the render order is the shared `HUMAN_DERIVED_BUCKET_ORDER`
    // constant so the catch-all below (and its test) check against the SAME
    // source of truth — count and render can never silently disagree.
    // trace:BUG-579 | ai:claude
    let order = burndown::HUMAN_DERIVED_BUCKET_ORDER;
    for bucket in order {
        let rows: Vec<&(
            burndown::OpenFacts,
            burndown::OpenBucket,
            Vec<burndown::Reason>,
        )> = classified.iter().filter(|(_, b, _)| *b == bucket).collect();
        if rows.is_empty() {
            continue;
        }
        // The bucket header glosses the derived reason — shared by every row in
        // the bucket (the first reason of the first row).
        let sample_reason = rows[0]
            .2
            .first()
            .map(|r| r.text.clone())
            .unwrap_or_default();
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            bucket.key().bold(),
            sample_reason.dimmed()
        );
        for (f, _, reasons) in rows {
            // Repeat the derived reason only when it differs from the header
            // (e.g. per-spec decision tags); otherwise just the ID + title stays
            // scannable. trace:BUG-535 — append the (truncated) title beside the
            // id via the shared `spec_title_cell` helper so the bucket is a
            // readable wall of titles, not bare ids.
            let derived = reasons.first().map(|r| r.text.clone()).unwrap_or_default();
            let title = spec_title_cell(&titles, &f.id);
            if derived == sample_reason {
                println!("    {}{}", f.id.cyan(), title);
            } else {
                println!(
                    "    {}{} {}",
                    f.id.cyan(),
                    title,
                    format!("({derived})").dimmed()
                );
            }
            // Fold in the additional reasons (finding-links + residual notes)
            // so the operator sees every reason it's still open — same as
            // `burndown explain`. trace:STORY-562
            for r in reasons.iter().skip(1) {
                let tag = match r.source {
                    burndown::ReasonSource::Finding => "finding".magenta(),
                    burndown::ReasonSource::Residual => "note".yellow(),
                    burndown::ReasonSource::Derived => continue,
                };
                println!("      {} {}", tag, r.text.dimmed());
            }
        }
    }

    // BUG-579: catch-all — render EVERY classified row that the ordered loop and
    // the first-class decisions-awaiting bucket did NOT already show. Without
    // this, any classified spec whose bucket isn't in `order` and which lacks a
    // posed `DecisionRequest` (the canonical case: an `AwaitingDecision` Spike
    // with no formal question) is COUNTED in `total` but never RENDERED — the
    // phantom "N items need a human" with no bucket to act on. This restores the
    // count==render invariant and future-proofs against any new OpenBucket
    // variant. The `has_pending_decision` rows are deliberately NOT re-rendered
    // here (the decisions-awaiting bucket already lists them, richer).
    // trace:BUG-579 | ai:claude
    let needs_attention: Vec<&(
        burndown::OpenFacts,
        burndown::OpenBucket,
        Vec<burndown::Reason>,
    )> = classified
        .iter()
        .filter(|(f, b, _)| !burndown::classified_row_is_rendered(*b, f.has_pending_decision))
        .collect();
    if !needs_attention.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            "needs-attention".bold(),
            "counted above — surface or groom so nothing is invisible".dimmed()
        );
        for (f, bucket, reasons) in needs_attention {
            let title = spec_title_cell(&titles, &f.id);
            println!("    {}{}", f.id.cyan(), title);
            // Lead with the bucket-specific hint (AwaitingDecision-without-a-
            // posed-question gets the actionable "pose a question / groom" line);
            // then fold in any derived reason for context.
            println!(
                "      {} {}",
                "?".magenta(),
                burndown::needs_attention_hint(*bucket).dimmed()
            );
            if let Some(r) = reasons.first() {
                if !r.text.is_empty() {
                    println!("      {} {}", "note".yellow(), r.text.dimmed());
                }
            }
        }
    }

    // TASK-747: the explicitly-routed-only group — specs pushed `--for human`
    // that the derived classifier didn't already surface. Listed last because
    // the derived buckets are most-actionable-first; an explicit route is a
    // deliberate "this needs you" the operator filed, shown so push-routed work
    // never gets lost behind derived membership. trace:TASK-747 | ai:claude
    if !routed_only.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            "routed-to-human".bold(),
            "explicitly routed to you (`queue add --for human`)".dimmed()
        );
        for id in &routed_only {
            // trace:BUG-535 — title beside the id, same as the derived buckets.
            println!("    {}{}", id.cyan(), spec_title_cell(&titles, id));
        }
    }

    // STORY-611 — bucket 3: Triage (findings). A pointer + live count is the
    // accepted shape (cheap; the full list lives in `aida findings list`).
    // trace:STORY-562 trace:STORY-611
    if open_findings > 0 {
        println!(
            "\n{} {} — {}",
            "●".magenta(),
            "triage-findings".bold(),
            format!(
                "{} finding{} awaiting triage — `aida findings list`",
                open_findings,
                if open_findings == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }

    // TASK-823: only print pointers that lead somewhere actionable — an
    // unconditional "see `aida questions list` / `aida findings list`" footer is
    // noise when those views are empty (and `questions list` shows only ANSWERED
    // decisions, so pointing at it as an "inbox" misleads). Build the footer from
    // the parts that actually have content. trace:TASK-823 | ai:claude
    let mut hints: Vec<String> = Vec::new();
    // Pending decisions already render as their own bucket above; the pointer is
    // only useful as "the rest of the inbox, incl. answered" — show it only when
    // there's a decision history to browse.
    if !pending_decisions.is_empty() {
        hints.push("answered decisions: `aida questions list`".to_string());
    }
    if open_findings > 0 {
        hints.push("all findings: `aida findings list`".to_string());
    }
    // STORY-618/622: cross-link the advisor worklist only when it has work — an
    // ungroomed draft to groom, or a finding to triage. (Cheap store scan; the
    // full set is `aida advisor`.) A solo operator wearing both hats then knows
    // the second list to check; when there's no advisor work, no noise.
    let has_ungroomed_draft = store.requirements.iter().any(|r| {
        !r.archived
            && !r.deferred
            && matches!(r.status, RequirementStatus::Draft)
            && !findings::is_finding(&r.tags.iter().cloned().collect::<Vec<_>>())
    });
    let has_triage = collect_findings_by_spec(&store)
        .values()
        .any(|ids| !ids.is_empty());
    let advisor_has_work = has_ungroomed_draft || has_triage;
    if advisor_has_work {
        hints.push("advisor work: `aida advisor`".to_string());
    }
    if !hints.is_empty() {
        println!(
            "\n  {} {}",
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
            hints.join(" · ").dimmed()
        );
    }
    Ok(())
}

/// STORY-618: bare `aida advisor` — the advisor's actionable worklist, the
/// mirror of bare `aida human` but grouped by ADVISOR action. The seat
/// complement of [`handle_list_human`]: the routine dispositions that need
/// cross-spec awareness + product judgment (not the operator's strategic
/// sign-off). Five buckets, each pointing at its canonical verb:
///   GROOM    — ungroomed drafts awaiting approve/defer/reject
///   DECOMPOSE/CLOSE — umbrella epics to decompose or close (advisor_seat)
///   DISTILL  — under-specified specs to turn into pre-recorded questions
///   TRIAGE   — findings awaiting triage
///   BLESS    — approved-but-unqueued backlog ready to groom onto the queue
/// Read-only; writes nothing. trace:STORY-618 | ai:claude
pub(crate) fn handle_advisor_worklist(
    short: bool,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;

    // Shared id→title cell (same construction as `aida human`). trace:BUG-535
    let titles: std::collections::HashMap<String, String> = store
        .requirements
        .iter()
        .filter_map(|req| {
            let id = req
                .agreed_id
                .clone()
                .or_else(|| req.spec_id.clone())
                .unwrap_or_else(|| req.id.to_string());
            (!req.title.is_empty()).then(|| (id, req.title.clone()))
        })
        .collect();

    // STORY-620: the configurable buckets (to-groom/decompose/ready-to-close/
    // triage) appear here only when the seat policy assigns them to the advisor
    // (the default).
    let seat_policy = seats::SeatPolicy::load(&project_root);

    // GROOM / DECOMPOSE / CLOSE come from the SAME open-facts classifier the
    // human view uses — keep the advisor_seat buckets the policy assigns to the
    // advisor. trace:STORY-618 trace:STORY-620
    let in_flight_role_map = in_flight_lease_role_map(&project_root);
    let advisor_rows: Vec<(
        burndown::OpenFacts,
        burndown::OpenBucket,
        Vec<burndown::Reason>,
    )> = collect_open_facts(&store, &in_flight_role_map)
        .into_iter()
        .map(|f| {
            let (bucket, reasons) = burndown::explain_reasons(&f);
            (f, bucket, reasons)
        })
        .filter(|(_, bucket, _)| {
            bucket.advisor_seat() && seat_policy.seat_of(bucket.key()) == seats::Seat::Advisor
        })
        .collect();

    // DISTILL — under-specified specs the sweep would flag (minus live leases /
    // BUG-495 exclusions). Reuses the exact set `aida clarify` resolves.
    let distill = clarify_default_specs(backend).unwrap_or_default();

    // TRIAGE — findings still awaiting triage (draft + finding tag), distinct.
    // STORY-620: only when `triage` is assigned to the advisor (the default).
    let triage = if seat_policy.seat_of("triage") == seats::Seat::Advisor {
        collect_findings_by_spec(&store)
            .values()
            .flat_map(|ids| ids.iter())
            .collect::<std::collections::HashSet<_>>()
            .len()
    } else {
        0
    };

    // BLESS — approved, unblocked, unqueued, not-in-flight specs the advisor
    // could groom onto the queue (queueing IS the ADR-3 sign-off).
    let queued_ids = all_queued_requirement_ids(&project_root);
    let in_flight_scopes = in_flight_lease_scopes(&project_root);
    let bless: Vec<String> = store
        .requirements
        .iter()
        .filter(|r| !r.archived && !r.deferred)
        .filter(|r| matches!(r.status, RequirementStatus::Approved))
        .filter(|r| !queued_ids.contains(&r.id))
        .filter(|r| !findings::is_finding(&r.tags.iter().cloned().collect::<Vec<_>>()))
        .filter(|r| !aida_core::pickability::blocked_by_incomplete(r, &store))
        .filter(|r| {
            let live = !in_flight_scopes.is_empty()
                && [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|s| in_flight_scopes.contains(&s.to_ascii_lowercase()));
            !live
        })
        .map(|r| r.display_id())
        .collect();

    // `--short`: bare IDs of the actionable groom/decompose/close specs, one
    // per line (mirrors `aida human --short`). The pointer buckets
    // (distill/triage/bless) are summaries, not part of the id stream.
    if short {
        for (f, _, _) in &advisor_rows {
            println!("{}", f.id);
        }
        for id in &bless {
            println!("{id}");
        }
        return Ok(());
    }

    println!(
        "{} the advisor's worklist",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    let nothing = advisor_rows.is_empty() && distill.is_empty() && triage == 0 && bless.is_empty();
    if nothing {
        println!(
            "  {}",
            "Nothing on the advisor's plate — no ungroomed drafts, no findings to \
             triage, no backlog to bless, nothing to close."
                .dimmed()
        );
        return Ok(());
    }

    // GROOM / DECOMPOSE / CLOSE buckets — full id+title lists (the real work),
    // most-actionable-first (CLOSE leads: a delivered epic is the cheapest win).
    use burndown::OpenBucket::*;
    let groups = [
        (
            ReadyToClose,
            "close",
            "mark the delivered epic Completed (`aida edit <id> --status completed`)",
        ),
        (
            Ungroomed,
            "groom",
            "weigh against the graph (overlap / staleness / scope), then dispose: \
             `aida edit <id> --status approved|rejected` · `aida defer <id> --until <trigger>` · or refine",
        ),
        (
            Umbrella,
            "decompose",
            "decompose into children or complete them",
        ),
    ];
    for (bucket, key, action) in groups {
        let rows: Vec<&(
            burndown::OpenFacts,
            burndown::OpenBucket,
            Vec<burndown::Reason>,
        )> = advisor_rows
            .iter()
            .filter(|(_, b, _)| *b == bucket)
            .collect();
        if rows.is_empty() {
            continue;
        }
        println!("\n{} {} — {}", "●".yellow(), key.bold(), action.dimmed());
        for (f, _, _) in rows {
            println!("    {}{}", f.id.cyan(), spec_title_cell(&titles, &f.id));
        }
    }

    // DISTILL / TRIAGE / BLESS — pointer + live count, each with its verb (the
    // accepted shape for "go act over there" buckets; full lists live in their
    // own commands). trace:STORY-618
    if !distill.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            "distill".bold(),
            // BUG-596: the hint must print a RUNNABLE command — `aida clarify`
            // does not exist (errors as an unrecognized subcommand). The real
            // verbs are `aida questions clarify` (interactive, defaults to the
            // swept set) and `aida decide <SPEC>` (smart-routes per spec).
            // trace:BUG-596 | ai:claude
            format!(
                "{} spec{} under-specified — author acceptance: `aida questions clarify` (or `aida decide <SPEC>`)",
                distill.len(),
                if distill.len() == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }
    if triage > 0 {
        println!(
            "\n{} {} — {}",
            "●".magenta(),
            "triage".bold(),
            format!(
                "{} finding{} awaiting triage — `aida findings list`",
                triage,
                if triage == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }
    if !bless.is_empty() {
        // STORY-622: LIST the bless candidates (not just a count) — these are the
        // approved-but-unqueued specs the advisor drives next, the actionable
        // backlog. A bare count hid the work and made the worklist read "empty"
        // next to `aida list open`. trace:STORY-622 | ai:claude
        println!(
            "\n{} {} — {}",
            "●".green(),
            "bless".bold(),
            "approved but not queued — `aida backlog groom --pickable` to queue (advisor sign-off)"
                .dimmed()
        );
        for id in &bless {
            println!("    {}{}", id.cyan(), spec_title_cell(&titles, id));
        }
    }

    println!(
        "\n  {} operator decisions / reviews live on `aida human` · full dashboard: `aida advisor status`",
        crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
    );
    Ok(())
}

/// STORY-563: build [`burndown::UnblockFacts`] for every open spec — the input
/// to the `aida human unblock` classifier. Mirrors [`collect_open_facts`] but
/// adds the three signals the unblock lens needs that the explain lens doesn't:
/// queue membership (advisor sign-off, ADR-3), acceptance-criteria presence,
/// and implementable-type. Read-only. trace:STORY-563 | ai:claude
pub(crate) fn collect_unblock_facts(
    store: &aida_core::RequirementsStore,
    in_flight_scopes: &std::collections::HashSet<String>,
    queued_ids: &std::collections::HashSet<uuid::Uuid>,
) -> Vec<burndown::UnblockFacts> {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let mut facts = Vec::new();
    for req in &store.requirements {
        if req.archived {
            continue;
        }
        // "Open" = not yet terminal. Completed/Rejected are done with.
        if matches!(
            req.status,
            aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
        ) {
            continue;
        }
        let id = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        let in_flight = !in_flight_scopes.is_empty()
            && [req.agreed_id.as_deref(), req.spec_id.as_deref()]
                .into_iter()
                .flatten()
                .any(|s| in_flight_scopes.contains(&s.to_ascii_lowercase()));
        // Acceptance presence — same text scan the questions-sweep uses (BUG-495).
        let text = requirement_text(req).to_ascii_lowercase();
        let has_acceptance =
            contains_any(&text, &["acceptance", "acceptance criteria", "acceptance:"]);
        facts.push(burndown::UnblockFacts {
            id,
            title: req.title.clone(),
            req_type: format!("{:?}", req.req_type).to_ascii_lowercase(),
            status: norm(&format!("{:?}", req.status)),
            tags: req.tags.iter().cloned().collect(),
            has_unsatisfied_blocker: aida_core::pickability::blocked_by_incomplete(req, store),
            has_pending_decision: req
                .decision_request
                .as_ref()
                .map(|d| d.is_pending())
                .unwrap_or(false),
            in_flight,
            queued: queued_ids.contains(&req.id),
            has_acceptance,
            implementable: is_implementable_type(&req.req_type),
        });
    }
    facts
}

/// STORY-563: `aida human unblock` — the deterministic prompt-assembler that
/// ends the recurring "how do I get open items into the burndown?" question by
/// GENERATING the grooming question. Read-only + no LLM (the SPIKE-55
/// deterministic-slice pattern, like `aida ultraplan` / `aida goal`): classify
/// every open spec by what keeps it out of the burndown ready set, then assemble
/// a paste-ready advisor prompt that routes each to queue / clarify-first /
/// leave-parked. The advisor (the grooming skill / live session) is the actor
/// the prompt drives. trace:STORY-563 | ai:claude
pub(crate) fn handle_human_subcommand(cmd: &cli::HumanCommand) -> Result<()> {
    match cmd {
        // TASK-770: namespaced aliases over the existing top-level presence
        // verbs; no duplicate state path or output shape.
        cli::HumanCommand::Away => presence_cmd::handle_away_command(),
        cli::HumanCommand::Home => presence_cmd::handle_home_command(),
        cli::HumanCommand::Presence | cli::HumanCommand::Status => {
            presence_cmd::handle_presence_command()
        }
        cli::HumanCommand::Unblock {
            copy,
            stdout,
            json,
            interactive,
            then_drain,
        } => handle_human_unblock(*copy, *stdout, *json, *interactive, *then_drain),
        // STORY-768: fire the /aida-human-audit pass at the advisor — enqueue a
        // durable directive (default) and optionally tmux-inject it now.
        // trace:STORY-768 | ai:claude
        cli::HumanCommand::Audit { inject } => {
            let project_root =
                find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
            handle_human_audit(&project_root, *inject)
        }
        // STORY-611: the action aliases need a storage handle, so they are
        // dispatched in the main `run()` body (where `backend`/`store_path`
        // are in scope), not here. This arm is unreachable from the early
        // pre-storage dispatch, which gates on `human_subcommand_needs_no_storage`.
        // trace:STORY-611 | ai:claude
        cli::HumanCommand::Answer { .. }
        | cli::HumanCommand::Decide { .. }
        | cli::HumanCommand::Review { .. } => {
            unreachable!("STORY-611 action aliases are dispatched after store init")
        }
    }
}

/// STORY-768: `aida human audit` — fire the `/aida-human-audit` reconcile pass
/// at the advisor session. Default is a durable enqueue onto the worker-
/// directive channel that the polling advisor picks up; `--inject` additionally
/// `tmux send-keys` the slash command into the advisor's registered pane for an
/// immediate run, falling back to enqueue-only when no pane is registered.
// trace:STORY-768 | ai:claude
pub(crate) fn handle_human_audit(project_root: &std::path::Path, inject: bool) -> Result<()> {
    // Always enqueue the durable directive first — it is the reliable,
    // headless/cross-vendor path. The optional inject is a best-effort nudge on
    // top of it, so an idle advisor runs the pass without waiting a poll cycle.
    human_audit::post_directive_line_enqueue(project_root)?;
    println!(
        "{} Enqueued a human-audit request for the advisor.",
        glyph(crate::glyphs::Glyph::Check).green()
    );
    println!(
        "  {}",
        "The polling advisor picks it up next cycle (aida worker directives) and runs the pass."
            .dimmed()
    );

    if !inject {
        return Ok(());
    }

    // --inject: try to send the slash command straight into the advisor's
    // registered tmux pane. Never hard-fail — fall back to the enqueue already
    // done above and explain why.
    match human_audit::plan_inject(human_audit::read_pane(project_root)) {
        human_audit::InjectPlan::Inject(argv) => {
            let (program, rest) = argv.split_first().expect("send-keys argv is non-empty");
            match std::process::Command::new(program)
                .args(rest)
                .status_retrying_etxtbsy()
            {
                Ok(status) if status.success() => {
                    println!(
                        "{} Injected the audit command into the advisor's tmux pane.",
                        glyph(crate::glyphs::Glyph::Check).green()
                    );
                }
                Ok(status) => {
                    println!(
                        "{} tmux send-keys exited with {} — the enqueued request still stands.",
                        glyph(crate::glyphs::Glyph::Warning).yellow(),
                        status
                    );
                }
                Err(e) => {
                    println!(
                        "{} Could not run tmux ({}) — the enqueued request still stands.",
                        glyph(crate::glyphs::Glyph::Warning).yellow(),
                        e
                    );
                }
            }
        }
        human_audit::InjectPlan::Fallback => {
            println!(
                "{} No advisor tmux pane is registered (not in tmux, or the advisor session did \
                 not start under tmux) — kept the enqueued request only.",
                glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
    }
    Ok(())
}

/// STORY-611: which `aida human <sub>` verbs run WITHOUT a storage handle. The
/// presence + unblock verbs do; the action aliases (`answer`/`review`/`decide`)
/// delegate to backend-needing canonical verbs, so they fall through to the
/// main dispatch instead of the early pre-storage return.
/// trace:STORY-611 | ai:claude
pub(crate) fn human_subcommand_needs_no_storage(cmd: &cli::HumanCommand) -> bool {
    matches!(
        cmd,
        cli::HumanCommand::Away
            | cli::HumanCommand::Home
            | cli::HumanCommand::Presence
            | cli::HumanCommand::Status
            | cli::HumanCommand::Unblock { .. }
            | cli::HumanCommand::Audit { .. }
    )
}

// trace:STORY-611 | ai:claude
#[cfg(test)]
#[path = "tests/story_611_human_alias_tests.rs"]
mod story_611_human_alias_tests;

/// STORY-563: the `aida human unblock` body. trace:STORY-563 | ai:claude
pub(crate) fn handle_human_unblock(
    copy: bool,
    stdout: bool,
    json: bool,
    interactive: bool,
    then_drain: bool,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;
    let in_flight_scopes = in_flight_lease_scopes(&project_root);
    let queued_ids = all_queued_requirement_ids(&project_root);
    let facts = collect_unblock_facts(&store, &in_flight_scopes, &queued_ids);

    // Classify each open spec, dropping the ones already in the burndown ready
    // set (nothing for the human to do). Preserve store order. trace:STORY-563
    let lines: Vec<burndown::UnblockLine> = facts
        .iter()
        .filter_map(|f| {
            burndown::classify_unblock(f).map(|class| burndown::UnblockLine {
                id: f.id.clone(),
                title: f.title.clone(),
                class,
                reason: burndown::unblock_reason(class).to_string(),
            })
        })
        .collect();

    // STORY-750: --interactive walks the human-blocked set and resolves each
    // hurdle inline (self-invoking the existing `aida` verbs) instead of emitting
    // the paste-ready advisor prompt. trace:STORY-750 | ai:claude
    if interactive {
        return run_interactive_unblock_sweep(lines, then_drain);
    }

    // --json: the classification, for machine consumers / the TUI.
    if json {
        let payload = lines
            .iter()
            .map(|l| {
                let action = match l.class.action() {
                    burndown::UnblockAction::Queue => "queue",
                    burndown::UnblockAction::Clarify => "clarify",
                    // trace:BUG-502
                    burndown::UnblockAction::Review => "review",
                    burndown::UnblockAction::Leave => "leave",
                };
                serde_json::json!({
                    "spec": l.id,
                    "class": l.class.key(),
                    "action": action,
                    "reason": l.reason,
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let prompt = burndown::assemble_unblock_prompt(&lines);

    // --stdout: the bare prompt only, for piping / command substitution.
    if stdout {
        print!("{prompt}");
        return Ok(());
    }

    // --copy: prompt to the clipboard, falling back to stdout.
    if copy {
        if copy_to_clipboard(&prompt) {
            println!(
                "{} copied the grooming prompt to the clipboard — paste it to the advisor",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            println!(
                "{} no clipboard tool found (wl-copy/xclip/xsel/pbcopy/clip) — printing instead",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            println!();
            print!("{prompt}");
        }
        return Ok(());
    }

    // Default: framed terminal output. Headline the count + the three actions,
    // then the assembled prompt set apart so the operator knows exactly what to
    // paste. trace:STORY-563
    let counts = |want: burndown::UnblockAction| -> usize {
        lines.iter().filter(|l| l.class.action() == want).count()
    };
    let n_queue = counts(burndown::UnblockAction::Queue);
    let n_clarify = counts(burndown::UnblockAction::Clarify);
    let n_leave = counts(burndown::UnblockAction::Leave);

    println!(
        "{} grooming prompt for the advisor",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    if lines.is_empty() {
        println!(
            "  {}",
            "Nothing to groom — every open spec is either already in the burndown \
             or self-resolving."
                .dimmed()
        );
        return Ok(());
    }
    println!(
        "  {} {} to queue · {} to clarify first · {} to leave parked",
        "→".green(),
        n_queue,
        n_clarify,
        n_leave
    );
    println!(
        "  {}",
        "Paste the block below to the advisor (or --copy it / --stdout to pipe):".dimmed()
    );
    println!();
    for line in prompt.lines() {
        println!("  {}", line.dimmed());
    }
    Ok(())
}

/// STORY-750: the `aida human unblock --interactive` sweep — walk the
/// human-blocked specs CHEAPEST-first and resolve each inline. Impure (TTY
/// prompts + self-invoked `aida` verbs); the menus + walk order are the pure,
/// unit-tested `burndown::sweep_*` helpers.
pub(crate) fn run_interactive_unblock_sweep(
    lines: Vec<burndown::UnblockLine>,
    then_drain: bool,
) -> Result<()> {
    use burndown::SweepChoice;

    // Needs a human at the keyboard: `inquire` reads stdin. In a non-TTY / agent
    // context, degrade with clear guidance rather than hang or silently no-op.
    if non_interactive_confirm() {
        anyhow::bail!(
            "`aida human unblock --interactive` needs a terminal. Run `aida human unblock` \
             (no flag) for the paste-ready advisor prompt, or `--json` for the machine view."
        );
    }

    if lines.is_empty() {
        println!(
            "{} nothing blocked on you — every open spec is drive-ready or self-resolving.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    let ordered = burndown::sweep_walk_order(lines);
    // Partition into: advisor-groomable (bulk-delegate, not the human's call),
    // genuinely-human decisions (walk), and leave-parked info rows. trace:STORY-750
    let (groomable, rest): (Vec<_>, Vec<_>) = ordered
        .into_iter()
        .partition(|l| burndown::sweep_is_advisor_groomable(l.class));
    let (mut human, info): (Vec<_>, Vec<_>) = rest
        .into_iter()
        .partition(|l| burndown::sweep_is_actionable(l.class));

    // Everything the operator hands off (the bulk groomable set + per-spec
    // "Ask the advisor" picks) → one advisor-delegate prompt at the end.
    let mut delegated: Vec<burndown::UnblockLine> = Vec::new();

    // --- Bulk advisor-grooming step: don't dump 100 draft-approvals on the human.
    // "Hand them to the advisor" used to be one vague option that merely printed a
    // paste-prompt. TASK-1087: make it legible — offer the two concrete groom
    // actions as top-level picks (propose vs apply), keep the paste-prompt as an
    // explicit option, and add a re-prompting "?" item that explains each with
    // simple examples (inquire 0.9.1 has no native '?' keybinding).
    // trace:STORY-750 trace:TASK-1087 | ai:claude
    if !groomable.is_empty() {
        const GROOM_PROPOSE: &str =
            "Groom them — the advisor proposes approve/queue/reject; you review";
        const GROOM_APPLY: &str = "Groom + approve — the advisor decides AND applies now";
        const PASTE: &str = "Emit a paste-prompt for a live advisor session";
        const WALK: &str = "Walk them myself anyway";
        const SKIP: &str = "Skip them";
        const HELP: &str = "? — what do these mean? (examples)";

        let heading = format!(
            "{} spec(s) are advisor grooming (drafts to approve / approved to queue) — the advisor's call, not yours",
            groomable.len()
        );

        // A tiny action enum so the re-prompting menu (the '?' item loops back to
        // the same prompt) is decoupled from the code that consumes `groomable`.
        enum GroomAction {
            Propose,
            Apply,
            Paste,
            Walk,
            Skip,
        }

        let action = loop {
            let opts = vec![GROOM_PROPOSE, GROOM_APPLY, PASTE, WALK, SKIP, HELP];
            let pick = match inquire::Select::new(&heading, opts)
                .with_help_message("Up/Down to move, Enter to select, or pick '?' for examples")
                .prompt()
            {
                Ok(p) => p,
                Err(inquire::InquireError::OperationCanceled)
                | Err(inquire::InquireError::OperationInterrupted) => {
                    println!(
                        "  {} sweep stopped.",
                        crate::glyph(crate::glyphs::Glyph::Cross).yellow()
                    );
                    return Ok(());
                }
                Err(e) => anyhow::bail!("prompt failed: {e}"),
            };
            match pick {
                GROOM_PROPOSE => break GroomAction::Propose,
                GROOM_APPLY => break GroomAction::Apply,
                PASTE => break GroomAction::Paste,
                WALK => break GroomAction::Walk,
                SKIP => break GroomAction::Skip,
                HELP => {
                    print_groom_handoff_help();
                    continue;
                }
                _ => continue,
            }
        };

        match action {
            // "Groom them" / "Groom + approve" fire the advisor's own disposition
            // pass via the SAME `aida groom` verb the advisor runs by hand — no
            // re-implementation. Propose writes nothing; apply executes.
            GroomAction::Propose => {
                println!(
                    "  {} firing the advisor's groom pass (propose-only — writes nothing until you apply)…",
                    crate::glyph(crate::glyphs::Glyph::Robot).cyan()
                );
                self_invoke_aida(&["groom"])?;
            }
            GroomAction::Apply => {
                println!(
                    "  {} firing the advisor's groom pass with --apply — it will approve/queue/reject…",
                    crate::glyph(crate::glyphs::Glyph::Robot).cyan()
                );
                self_invoke_aida(&["groom", "--apply"])?;
            }
            // Relay to a warm advisor session as a paste-prompt (the old behavior).
            GroomAction::Paste => delegated.extend(groomable),
            GroomAction::Walk => {
                let mut merged = groomable;
                merged.extend(human);
                human = merged;
            }
            GroomAction::Skip => {}
        }
    }

    // --- Interactive human walk (cheapest-first; title shown so you can decide).
    println!();
    if human.is_empty() {
        println!(
            "{} no genuinely-human decisions to walk.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        println!(
            "{} {} spec(s) need your judgment — walking cheapest-first. Ctrl-C to stop.",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
            human.len()
        );
        println!();
    }

    let (mut resolved, mut skipped) = (0usize, 0usize);
    'walk: for line in &human {
        // Per-spec loop so "Show details" re-prompts the same spec.
        loop {
            let menu = burndown::sweep_menu(line.class);
            let labels: Vec<&str> = menu.iter().map(|c| c.label()).collect();
            let heading = format!("{} — {} — {}", line.id, line.title, line.reason);
            let picked = match inquire::Select::new(&heading, labels).prompt() {
                Ok(label) => menu
                    .iter()
                    .copied()
                    .find(|c| c.label() == label)
                    .unwrap_or(SweepChoice::Skip),
                Err(inquire::InquireError::OperationCanceled)
                | Err(inquire::InquireError::OperationInterrupted) => {
                    println!(
                        "  {} sweep stopped.",
                        crate::glyph(crate::glyphs::Glyph::Cross).yellow()
                    );
                    break 'walk;
                }
                Err(e) => anyhow::bail!("prompt failed: {e}"),
            };
            match picked {
                // Show the full spec, then re-prompt the SAME one.
                SweepChoice::ShowDetails => {
                    let _ = self_invoke_aida(&["show", &line.id, "--no-git"]);
                    continue;
                }
                // Hand this one to the advisor instead of deciding it.
                SweepChoice::AskAdvisor => {
                    delegated.push(line.clone());
                    println!(
                        "    {} {} handed to the advisor.",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        line.id
                    );
                    skipped += 1;
                    break;
                }
                other => {
                    if apply_sweep_choice(&line.id, other)? {
                        resolved += 1;
                    } else {
                        skipped += 1;
                    }
                    break;
                }
            }
        }
    }

    // --- Leave-parked info rows: list, no prompt.
    if !info.is_empty() {
        println!();
        println!(
            "{} left parked (nothing to resolve inline):",
            crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
        );
        for line in &info {
            println!(
                "  {} {} — {}",
                crate::glyph(crate::glyphs::Glyph::Bullet).dimmed(),
                line.id.dimmed(),
                line.reason.dimmed()
            );
        }
    }

    println!();
    println!(
        "{} sweep done — {} resolved, {} skipped, {} handed to the advisor.",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        resolved,
        skipped,
        delegated.len()
    );

    // --- Emit ONE advisor-delegate prompt for everything handed off.
    if !delegated.is_empty() {
        let prompt = burndown::assemble_unblock_prompt(&delegated);
        println!();
        println!(
            "{} paste this to the advisor to groom the {} handed-off spec(s) (or `aida human unblock --copy`):",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
            delegated.len()
        );
        println!();
        for l in prompt.lines() {
            println!("  {}", l.dimmed());
        }
    }

    if then_drain {
        report_drain_readiness();
    }
    Ok(())
}

/// TASK-1087: the '?' item on the bulk advisor-grooming menu — explain each
/// choice in plain terms with concrete examples, then the caller re-prompts.
// trace:TASK-1087 | ai:claude
pub(crate) fn print_groom_handoff_help() {
    let arrow = crate::glyph(crate::glyphs::Glyph::Arrow);
    println!();
    println!("  {}", "What each choice does".bold());
    println!();
    println!("  {}", "Groom them (propose)".cyan());
    println!("    Fires the advisor — a fresh `claude -p` that reads the open drafts /");
    println!("    approved-unqueued specs and proposes a fate for each. Writes NOTHING");
    println!("    until you apply. For example:");
    println!("      DRAFT-812  {arrow} approve + queue   (clear, in scope)");
    println!("      DRAFT-820  {arrow} reject            (duplicate of an existing story)");
    println!("      SPIKE-9    {arrow} park              (needs a demand signal first)");
    println!("    You review, then run `aida groom --apply` to execute.");
    println!();
    println!("  {}", "Groom + approve (apply)".cyan());
    println!("    The same advisor pass, but it EXECUTES its calls immediately —");
    println!("    approvals get queued, rejects rejected, parks parked. For example:");
    println!("      \"11 approved & queued, 3 rejected, 2 parked\" — nothing left for you.");
    println!("    Use when you trust the advisor on these low-stakes grooming calls.");
    println!();
    println!("  {}", "Emit a paste-prompt for a live advisor".cyan());
    println!("    Changes nothing. Prints a ready-made prompt (grouped by action) for");
    println!("    you to paste into your WARM advisor session — richer context than a");
    println!("    cold-boot agent.");
    println!();
    println!("  {}", "Walk them myself".cyan());
    println!("    Adds them back to your one-by-one review; you decide each.");
    println!();
    println!("  {}", "Skip them".cyan());
    println!("    Leave them parked and untouched this pass.");
    println!();
    println!(
        "  {}",
        "Note: groom considers the advisor's full open-spec candidate set (these included), not only the rows shown here."
            .dimmed()
    );
    println!();
}

/// The parking tag the interactive sweep clears when the operator captures a
/// design decision on a spec — see [`tags_to_clear_on_note`].
// trace:TASK-1086 | ai:claude
pub(crate) const NEEDS_DESIGN_TAG: &str = "needs-design";

/// TASK-1086: the pure core of the sweep's "add a decision note" action — decide
/// which tags to clear given the operator's typed note and the spec's current
/// tags. Capturing a real decision on a `needs-design`-parked spec unparks it, so
/// `needs-design` (case-insensitive) is cleared and the spec becomes drive-ready
/// in the same action; a blank/cancelled note changes nothing. Every other tag is
/// preserved, and a spec that doesn't carry `needs-design` yields an empty list
/// (idempotent no-op). Pure, so exhaustively unit-testable; the runtime reuses the
/// `aida edit --remove-tag` write path for the actual clear.
// trace:TASK-1086 | ai:claude
pub(crate) fn tags_to_clear_on_note(note: &str, tags: &HashSet<String>) -> Vec<String> {
    if note.trim().is_empty() {
        return Vec::new();
    }
    tags.iter()
        .filter(|t| t.trim().eq_ignore_ascii_case(NEEDS_DESIGN_TAG))
        .cloned()
        .collect()
}

/// TASK-1086: best-effort read of a spec's current tag set for the interactive
/// sweep. Returns an empty set if the store can't be reached — the caller then
/// clears nothing, which is the safe (leave-parked) default.
// trace:TASK-1086 | ai:claude
pub(crate) fn load_spec_tags(id: &str) -> HashSet<String> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let Some(store) = load_store_for_lookup(&project_root) else {
        return HashSet::new();
    };
    store
        .get_requirement_by_spec_id(id.trim())
        .map(|r| r.tags.clone())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "tests/task_1086_unblock_clears_needs_design.rs"]
mod task_1086_unblock_clears_needs_design;

/// STORY-750: perform ONE sweep resolution by self-invoking the existing `aida`
/// verb — so the write path is the SAME one `aida edit` / `aida queue add` use,
/// never a re-implementation. Returns `true` if it mutated the spec toward
/// drive-ready, `false` if it only surfaced guidance / the operator skipped.
// trace:STORY-750 | ai:claude
pub(crate) fn apply_sweep_choice(id: &str, choice: burndown::SweepChoice) -> Result<bool> {
    use burndown::SweepChoice;
    match choice {
        SweepChoice::ApproveQueue => {
            self_invoke_aida(&["edit", id, "--status", "approved"])?;
            self_invoke_aida(&["queue", "add", id])?;
            println!(
                "    {} {id} approved & queued.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
            Ok(true)
        }
        SweepChoice::Reject => {
            self_invoke_aida(&["edit", id, "--status", "rejected"])?;
            println!(
                "    {} {id} rejected.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
            Ok(true)
        }
        SweepChoice::Queue => {
            self_invoke_aida(&["queue", "add", id])?;
            println!(
                "    {} {id} queued.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
            Ok(true)
        }
        SweepChoice::LaunchGuided => {
            // The operator chose to resolve the design NOW — hand off to the real
            // guided resolver (its own focused session), then return to the sweep.
            self_invoke_aida(&["queue", "work", id, "--guided"])?;
            Ok(true)
        }
        SweepChoice::AddNote => {
            let note = inquire::Text::new("Decision note:")
                .prompt()
                .unwrap_or_default();
            if note.trim().is_empty() {
                println!(
                    "    {} no note entered — left parked.",
                    crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
                );
                return Ok(false);
            }
            self_invoke_aida(&["comment", "add", id, note.trim()])?;
            // TASK-1086: capturing a design decision unparks the spec — clear the
            // `needs-design` tag so it becomes drive-ready in the SAME action,
            // instead of leaving the operator to strip the tag by hand. Reuses the
            // `aida edit --remove-tag` write path; idempotent (a spec without the
            // tag clears nothing). Only fires on a real capture — a blank note
            // returned above with the tag intact.
            let to_clear = tags_to_clear_on_note(&note, &load_spec_tags(id));
            for tag in &to_clear {
                self_invoke_aida(&["edit", id, "--remove-tag", tag])?;
            }
            if to_clear.is_empty() {
                println!(
                    "    {} note recorded on {id} (left parked).",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
                Ok(false)
            } else {
                println!(
                    "    {} decision recorded on {id} — cleared `{NEEDS_DESIGN_TAG}`, now drive-ready.",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
                Ok(true)
            }
        }
        SweepChoice::Clarify => {
            println!(
                "    {} clarify its acceptance, then it becomes queueable:",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan()
            );
            println!("      aida edit {id}   (add a `## Acceptance` section)");
            Ok(false)
        }
        SweepChoice::ShowReview => {
            println!(
                "    {} it's built — review it (don't re-queue): aida review {id}  (or reopen its draft PR)",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan()
            );
            Ok(false)
        }
        SweepChoice::ShowBlocker => {
            // `aida why` surfaces the dependency chain in detail; best-effort.
            let _ = self_invoke_aida(&["why", id]);
            Ok(false)
        }
        // Handled by the walk loop before reaching here (they re-prompt / delegate),
        // but the match must be exhaustive.
        SweepChoice::ShowDetails | SweepChoice::AskAdvisor | SweepChoice::Skip => Ok(false),
    }
}

// BUG-677: `SpecLiveness` + `classify_spec_liveness` (the operator's "is a LIVE
// process working this In-Progress spec?" verdict) moved to
// `aida_core::liveness`, re-exported at the crate root, so `aida-tui` shares the
// exact same classifier. trace:BUG-677 | ai:claude

/// Find the LOCAL session lease whose scope is this spec — the happy-path link
/// for AIDA-launched work (`aida queue work`, `aida agent new`) where the lease
/// scope IS the spec id. Matches the spec's agreed id OR its raw spec id,
/// case-insensitively, against each lease's raw `--owns` scope. Returns `None`
/// for advisor Agent-tool fan-outs (generic `harness-worktree` scopes) — that
/// absence is the honest `flag-only` signal.
// trace:STORY-694 | ai:claude
pub(crate) fn spec_scoped_lease<'a>(
    leases: &'a [SessionLease],
    spec_ids: &[&str],
) -> Option<&'a SessionLease> {
    leases.iter().find(|l| {
        spec_ids
            .iter()
            .any(|id| !id.is_empty() && l.scope.eq_ignore_ascii_case(id))
    })
}

/// `aida status <spec>` — the per-spec liveness view. Shows the spec's
/// lifecycle status and a LIVENESS section answering "is a live session
/// actually working this, or is the In-Progress flag orphaned?". Reuses the
/// session-lease store ([`list_leases`]), the lease-state classifier
/// ([`lease_state_for`], which already folds in pid/worktree liveness), and the
/// elapsed-from-started helper ([`humanize_duration_secs`]).
// trace:STORY-694 | ai:claude
/// The outcome of [`ensure_epic_worktree`] / [`ensure_spec_worktree`]: where
/// the worktree lives, the branch it's on, the focus label written into it, and
/// whether THIS call created it (vs. found it already registered). Lets `add`
/// print "created" vs "already exists", and `enter` re-affirm the focus on an
/// existing tree.
// trace:STORY-716 trace:STORY-742 | ai:claude
pub(crate) struct WorktreeOutcome {
    pub(crate) path: std::path::PathBuf,
    pub(crate) branch: String,
    pub(crate) focus: String,
    pub(crate) created: bool,
    pub(crate) lease_id: Option<String>,
    pub(crate) has_session_env: bool,
}

// trace:TASK-1156 | ai:codex
pub(crate) struct ScopedEnvVar {
    pub(crate) key: &'static str,
    pub(crate) previous: Option<std::ffi::OsString>,
}

impl ScopedEnvVar {
    pub(crate) fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

/// Resolve the path + branch for an epic worktree (honoring `--path`/`--branch`
/// overrides), create the git worktree off the default branch (origin/main) if
/// it isn't already registered, and write the `aida focus <epic>` marker INSIDE
/// it. Idempotent: an already-registered worktree is left as-is and its focus
/// re-affirmed. Prints nothing — the caller routes human/eval output.
// trace:STORY-716 | ai:claude
pub(crate) fn ensure_epic_worktree(
    epic: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
) -> Result<WorktreeOutcome> {
    let main_root = find_main_worktree_root()?;
    let home =
        crate::home_dir().context("cannot resolve home directory for the default worktree path")?;
    ensure_epic_worktree_core(&main_root, &home, epic, path_override, branch_override)
}

/// The testable core of [`ensure_epic_worktree`] — takes the main worktree root
/// and the home dir explicitly so a test can drive it against a throwaway git
/// repo + temp home without depending on cwd discovery.
// trace:STORY-716 | ai:claude
pub(crate) fn ensure_epic_worktree_core(
    main_root: &std::path::Path,
    home: &std::path::Path,
    epic: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
) -> Result<WorktreeOutcome> {
    let focus_label = crate::worktree::normalize_epic_label(epic);

    let path: std::path::PathBuf = match path_override {
        Some(p) => std::path::PathBuf::from(p),
        None => crate::worktree::default_worktree_path(home, epic),
    };
    let branch = branch_override
        .map(|b| b.to_string())
        .unwrap_or_else(|| crate::worktree::default_branch(epic));

    // Idempotency: if git already tracks a worktree at this path, re-affirm the
    // focus and report it rather than erroring (mirrors a re-run of `add`).
    let porcelain = std::process::Command::new("git")
        .arg("-C")
        .arg(main_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();

    if crate::worktree::is_registered(&porcelain, &path) {
        if aida_core::git_ops::is_git_repo(&path) {
            aida_core::git_ops::ensure_aida_runtime_excluded(&path)
                .with_context(|| format!("exclude AIDA runtime files in {}", path.display()))?;
        }
        crate::focus::write_focus_marker(&path, &focus_label)?;
        return Ok(WorktreeOutcome {
            path,
            branch,
            focus: focus_label,
            created: false,
            lease_id: None,
            has_session_env: false,
        });
    }

    // Fork the new branch from origin's mainline regardless of the current
    // worktree's HEAD (BUG-76's detect helper), falling back to origin/main.
    let base_ref = detect_default_branch_ref(main_root).unwrap_or_else(|| "origin/main".into());

    // If the branch already exists, attach it (drop `-b`); else create it off
    // the base ref. `git worktree add` refuses to recreate an existing branch.
    let branch_exists = std::process::Command::new("git")
        .arg("-C")
        .arg(main_root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{}", branch))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let path_str = path
        .to_str()
        .context("worktree path is not valid UTF-8")?
        .to_string();

    // Ensure the parent dir exists (e.g. `~/ai` on a fresh machine) — older git
    // won't create missing intermediate directories for the worktree path.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // `--` ends options: the path and branch/base that follow are
    // positional even when dash-led. trace:BUG-1622 | ai:claude
    let mut args: Vec<String> = vec!["worktree".into(), "add".into()];
    if branch_exists {
        args.push("--".into());
        args.push(path_str.clone());
        args.push(branch.clone());
    } else {
        args.push("-b".into());
        args.push(branch.clone());
        args.push("--".into());
        args.push(path_str.clone());
        args.push(base_ref.clone());
    }

    let res = std::process::Command::new("git")
        .arg("-C")
        .arg(main_root)
        .args(&args)
        .output()
        .context("failed to invoke `git worktree add`")?;
    if !res.status.success() {
        anyhow::bail!(
            "`git worktree add` failed: {}",
            String::from_utf8_lossy(&res.stderr).trim()
        );
    }
    aida_core::git_ops::ensure_aida_runtime_excluded(&path)
        .with_context(|| format!("exclude AIDA runtime files in {}", path.display()))?;
    aida_core::git_ops::warn_worktree_container_gitdir(main_root, &path);
    aida_core::git_ops::init_submodules_or_warn(&path, worktree_config_init_submodules(main_root))
        .with_context(|| format!("prepare submodules in worktree {}", path.display()))?;

    // Auto-scope the new tree to the epic (STORY-706 focus). Uses the low-level
    // marker writer, not the validating `aida focus` path — the fresh worktree
    // has no cache attached yet, and validation already happened (or doesn't
    // apply) at the operator's keyboard.
    crate::focus::write_focus_marker(&path, &focus_label)?;

    Ok(WorktreeOutcome {
        path,
        branch,
        focus: focus_label,
        created: true,
        lease_id: None,
        has_session_env: false,
    })
}

/// Whether a `aida worktree add|enter <arg>` argument names an epic (the
/// STORY-716 legacy path — a scoping-only worktree, no lease) or a single
/// non-epic spec (create the worktree AND take the implementer lease so the
/// spec flips InProgress and shows live). Unresolvable args and epics both take
/// the epic path; only a resolved non-epic spec takes the lease path.
// trace:STORY-742 | ai:claude
pub(crate) enum WorktreeTarget {
    /// An epic (or an arg that didn't resolve to a stored spec): the STORY-716
    /// epic-scoped worktree, unchanged.
    Epic,
    /// A single non-epic spec: `display` is its canonical id, `focus` the label
    /// the new worktree is scoped to (the spec itself).
    Spec { display: String, focus: String },
}

/// Pure classification half of [`classify_worktree_arg`]: given the requirement
/// the arg resolved to (or `None`), decide epic-vs-spec. A non-epic spec routes
/// to the lease path; an epic or an unresolved arg keeps the legacy epic path.
// trace:STORY-742 | ai:claude
pub(crate) fn classify_worktree_target(
    req: Option<&aida_core::Requirement>,
    arg: &str,
) -> WorktreeTarget {
    match req {
        Some(r) if r.req_type != RequirementType::Epic => {
            let display = r
                .agreed_id
                .as_deref()
                .or(r.spec_id.as_deref())
                .unwrap_or(arg)
                .to_string();
            WorktreeTarget::Spec {
                focus: display.clone(),
                display,
            }
        }
        _ => WorktreeTarget::Epic,
    }
}

/// Resolve a `aida worktree add|enter` argument against the store to decide
/// whether it names an epic or a single spec. Best-effort: any store-load
/// failure falls back to the legacy epic path so the command never regresses on
/// a fresh clone / offline store.
// trace:STORY-742 | ai:claude
pub(crate) fn classify_worktree_arg(arg: &str) -> WorktreeTarget {
    let Ok(main_root) = find_main_worktree_root() else {
        return WorktreeTarget::Epic;
    };
    let Ok(store) = Storage::new(main_root.join(".aida-store")).load() else {
        return WorktreeTarget::Epic;
    };
    let req = store.requirements.iter().find(|r| spec_matches(r, arg));
    classify_worktree_target(req, arg)
}

/// Testable core of [`ensure_spec_worktree`]: decide create-vs-re-enter for a
/// single-spec worktree. Inputs: the resolved home dir + spec, whether a lease
/// already covers the spec (`existing` = its worktree path + branch), and
/// whether the default path is already a registered git worktree. The `mint`
/// closure performs the real worktree+lease setup (production wires it to
/// `session_start` — the same primitive `aida queue work --no-launch` rides)
/// and returns the created worktree path + branch. Idempotent: an existing
/// lease OR a registered worktree short-circuits to re-affirming the focus
/// marker without minting.
// trace:STORY-742 | ai:claude
pub(crate) fn ensure_spec_worktree_core(
    home: &std::path::Path,
    spec_display: &str,
    focus_label: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
    existing: Option<(std::path::PathBuf, String, String, bool)>,
    registered: bool,
    mint: impl FnOnce(&std::path::Path, &str) -> Result<(std::path::PathBuf, String)>,
) -> Result<WorktreeOutcome> {
    let path = match path_override {
        Some(p) => std::path::PathBuf::from(p),
        None => crate::worktree::default_worktree_path(home, spec_display),
    };
    let branch = branch_override
        .map(|b| b.to_string())
        .unwrap_or_else(|| crate::worktree::default_branch(spec_display));

    // Idempotent re-entry: a lease already covers this spec — re-affirm focus at
    // that worktree and report it, don't mint a second one.
    if let Some((worktree_path, lease_branch, lease_id, has_session_env)) = existing {
        if aida_core::git_ops::is_git_repo(&worktree_path) {
            aida_core::git_ops::ensure_aida_runtime_excluded(&worktree_path).with_context(
                || format!("exclude AIDA runtime files in {}", worktree_path.display()),
            )?;
        }
        crate::focus::write_focus_marker(&worktree_path, focus_label)?;
        return Ok(WorktreeOutcome {
            path: worktree_path,
            branch: lease_branch,
            focus: focus_label.to_string(),
            created: false,
            lease_id: Some(lease_id),
            has_session_env,
        });
    }

    // A worktree is registered at the default path but carries no lease — treat
    // it as already-there (re-affirm focus) rather than recreating it.
    if registered {
        if aida_core::git_ops::is_git_repo(&path) {
            aida_core::git_ops::ensure_aida_runtime_excluded(&path)
                .with_context(|| format!("exclude AIDA runtime files in {}", path.display()))?;
        }
        crate::focus::write_focus_marker(&path, focus_label)?;
        return Ok(WorktreeOutcome {
            path,
            branch,
            focus: focus_label.to_string(),
            created: false,
            lease_id: None,
            has_session_env: false,
        });
    }

    // Fresh: mint the worktree + lease, then scope focus to the spec.
    let (worktree_path, minted_branch) = mint(&path, &branch)?;
    crate::focus::write_focus_marker(&worktree_path, focus_label)?;
    Ok(WorktreeOutcome {
        path: worktree_path,
        branch: minted_branch,
        focus: focus_label.to_string(),
        created: true,
        lease_id: None,
        has_session_env: false,
    })
}

/// `<spec>` variant of [`ensure_epic_worktree`]: create-if-missing a worktree
/// for a single non-epic spec and take the implementer lease, reusing the same
/// `session_start` worktree+lease primitive that `aida queue work --no-launch`
/// rides — NO agent is launched. Idempotent: an existing lease/worktree for the
/// spec is re-entered, not recreated. The worktree forks off origin/main and is
/// auto-scoped (focus) to the spec.
// trace:STORY-742 | ai:claude
pub(crate) fn ensure_spec_worktree(
    spec_display: &str,
    focus_label: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
    worktree_context: &str,
) -> Result<WorktreeOutcome> {
    let main_root = find_main_worktree_root()?;
    let home =
        crate::home_dir().context("cannot resolve home directory for the default worktree path")?;

    // Existing lease for this spec → re-enter it (freshest wins).
    let existing = list_leases(&main_root)
        .into_iter()
        .filter(|l| l.scope.eq_ignore_ascii_case(spec_display))
        .max_by_key(|l| l.started_at)
        .map(|l| {
            (
                l.worktree_path.clone(),
                l.branch.clone(),
                l.id.clone(),
                l.cargo_target_dir.is_some()
                    || l.worktree_path
                        .join(".aida")
                        .join("session-env.sh")
                        .is_file(),
            )
        });

    // Is the default path already a registered (lease-less) git worktree?
    let default_path = match path_override {
        Some(p) => std::path::PathBuf::from(p),
        None => crate::worktree::default_worktree_path(&home, spec_display),
    };
    let porcelain = std::process::Command::new("git")
        .arg("-C")
        .arg(&main_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let registered = crate::worktree::is_registered(&porcelain, &default_path);

    ensure_spec_worktree_core(
        &home,
        spec_display,
        focus_label,
        path_override,
        branch_override,
        existing,
        registered,
        |path, branch| {
            // Reuse the queue-work `--no-launch` setup primitive: session_start
            // mints the worktree + lease + Approved→InProgress bump off
            // origin/main. use_pool=Some(false) keeps the deterministic
            // per-spec path (no warm-pool tree); launch=false = no agent.
            let _context = ScopedEnvVar::set("AIDA_WORKTREE_CONTEXT", worktree_context);
            let _suppress_next = ScopedEnvVar::set("AIDA_SUPPRESS_SESSION_NEXT", "1");
            session_start(
                spec_display,
                Some(branch),
                /* base */ None,
                /* reuse_branch */ false,
                path.to_str(),
                /* forge_override */ None,
                /* branch_style */ "auto",
                /* launch */ false,
                /* launch_title */ None,
                /* launch_set_title */ false,
                /* launch_name */ None,
                /* permission_mode */ None,
                /* launch_contained */ false,
                /* role */ Some("implementer".to_string()),
                /* force_claim */ false,
                /* use_pool */ Some(false),
            )?;
            let lease = list_leases(&main_root)
                .into_iter()
                .filter(|l| l.scope.eq_ignore_ascii_case(spec_display))
                .max_by_key(|l| l.started_at)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "worktree + lease setup completed but no lease for `{}` is visible \
                         — try `aida session leases`",
                        spec_display
                    )
                })?;
            Ok((lease.worktree_path.clone(), lease.branch.clone()))
        },
    )
    .and_then(|mut out| {
        if out.lease_id.is_some() && out.has_session_env {
            return Ok(out);
        }
        if let Some(lease) = list_leases(&main_root)
            .into_iter()
            .filter(|l| l.scope.eq_ignore_ascii_case(spec_display))
            .max_by_key(|l| l.started_at)
        {
            out.lease_id = Some(lease.id.clone());
            out.has_session_env = lease.cargo_target_dir.is_some()
                || lease
                    .worktree_path
                    .join(".aida")
                    .join("session-env.sh")
                    .is_file();
        }
        Ok(out)
    })
    .map(|out| {
        // BUG-778: both verbs that land here (`worktree enter` / `worktree
        // add`) hand the worktree to a HUMAN and launch no agent. Stamp that
        // provenance on the lease so `aida ps` reads the gap before the
        // operator's launch as "awaiting agent", not "process dead — resume
        // with `aida queue work <spec>`" (which would dispatch a competing
        // session onto a spec someone is working by hand). Best-effort: a
        // failed stamp costs a cosmetic misread, never the worktree the
        // operator asked for. trace:BUG-778 | ai:claude
        if let Some(id) = out.lease_id.as_deref() {
            let _ = mark_lease_manual_enter(&main_root, id);
        }
        // BUG-1944: taking the worktree starts work on every outcome (fresh,
        // re-entered lease, or lease-less registered worktree), so an Approved
        // spec moves to In Progress here too, not only inside session_start.
        // No driver is recorded: AIDA has no driver field (derived, STORY-1653).
        // trace:BUG-1944 | ai:claude
        crate::live_work::bump_spec_in_progress(
            &main_root,
            spec_display,
            aida_core::conflict::LEASE_TAKE_AUTHOR,
        );
        out
    })
}

/// Dispatch `aida worktree <subcommand>` (STORY-716, EPIC-55 workspace layer).
// trace:STORY-716 | ai:claude
pub(crate) fn handle_worktree_command(cmd: &WorktreeCommand) -> Result<()> {
    match cmd {
        WorktreeCommand::Add {
            target,
            path,
            branch,
        } => handle_worktree_add(target, path.as_deref(), branch.as_deref()),
        WorktreeCommand::Enter {
            target,
            path,
            branch,
        } => handle_worktree_enter(target, path.as_deref(), branch.as_deref()),
        WorktreeCommand::Exit => handle_worktree_exit(),
        WorktreeCommand::List { json } => handle_worktree_list(*json),
        WorktreeCommand::Gc { yes, force, json } => {
            doctor_cmd::run_merged_agent_worktree_gc(*yes, *force, *json)
        }
        WorktreeCommand::Dismiss { path, category } => {
            worktree_dismiss::handle_dismiss(path.as_deref(), category.as_deref())
        }
        // STORY-714 warm-pool surface.
        WorktreeCommand::Reclaim {
            apply,
            min_age_mins,
            target_pct,
            include_live,
            include_open_prs,
            json,
        } => worktree_reclaim::run(&worktree_reclaim::ReclaimOptions {
            apply: *apply,
            min_age_mins: *min_age_mins,
            target_pct: *target_pct,
            include_live: *include_live,
            include_open_prs: *include_open_prs,
            json: *json,
        }),
        WorktreeCommand::Pool(pool) => handle_worktree_pool_command(pool),
    }
}

// ── Worktree warm-pool CLI (STORY-714) ──────────────────────────────────────
//
// Dispatched before storage init (via handle_worktree_command): the pool
// operates purely on git + the `.aida/worktree-pool/` registry, no requirement
// store. trace:STORY-714 | ai:claude

/// `[worktree_pool] max_trees` from the repo-level config (clamped ≥1), or None
/// to fall back to the pool's default cap.
pub(crate) fn worktree_pool_config_max_trees(project_root: &std::path::Path) -> Option<usize> {
    let body = std::fs::read_to_string(project_root.join(".aida").join("config.toml")).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    value
        .get("worktree_pool")?
        .get("max_trees")?
        .as_integer()
        .map(|n| n.max(1) as usize)
}

/// Read `[worktree_pool] worktree_parent` from `.aida/config.toml`: the opt-in
/// single parent directory that AIDA-created worktrees are nested under, so one
/// editor/agent folder-trust grant on that directory covers every worktree AIDA
/// mints (folder trust inherits from a parent — BUG-1700). Unset (the default)
/// keeps the historical sibling layout, so nothing moves under a live fleet. A
/// blank value is treated as unset. `~` is expanded; a relative path is left
/// relative and resolved against the project root by the pool.
// trace:BUG-1700 | ai:claude
pub(crate) fn worktree_pool_config_worktree_parent(
    project_root: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let body = std::fs::read_to_string(project_root.join(".aida").join("config.toml")).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let raw = value
        .get("worktree_pool")?
        .get("worktree_parent")?
        .as_str()?
        .trim();
    if raw.is_empty() {
        return None;
    }
    Some(expand_worktree_parent_tilde(raw))
}

/// Expand a leading `~` / `~/` in a configured worktree parent against `$HOME`.
// trace:BUG-1700 | ai:claude
pub(crate) fn expand_worktree_parent_tilde(raw: &str) -> std::path::PathBuf {
    // `home_dir()`, not a direct HOME read: it is the crate's single home
    // resolver and carries the cfg(test) redirect to a temp home, which the
    // `no_direct_home_resolution_in_crate` guard enforces (TASK-1513).
    expand_tilde_against(raw, home_dir())
}

/// Pure core of [`expand_worktree_parent_tilde`], with `$HOME` injected so it is
/// testable without mutating process environment. A path is left EXACTLY as
/// written when home is unknown, so the caller sees the literal configured value
/// rather than a silently wrong one.
// trace:BUG-1700 | ai:claude
pub(crate) fn expand_tilde_against(
    raw: &str,
    home: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    match (raw, home) {
        ("~", Some(h)) => h,
        (r, Some(h)) if r.starts_with("~/") => h.join(&r[2..]),
        _ => std::path::PathBuf::from(raw),
    }
}

/// Read `[worktree_pool] lease_ttl_secs` (seconds) from `.aida/config.toml`,
/// falling back to the core default when unset/invalid. Drives the
/// reservation-leak backstop: a lease older than this with no live owner is
/// reclaimable (and shows as `expired` in `pool status`).
// trace:TASK-1008 | ai:claude
pub(crate) fn worktree_pool_config_lease_ttl_secs(project_root: &std::path::Path) -> i64 {
    std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        .ok()
        .and_then(|body| toml::from_str::<toml::Value>(&body).ok())
        .and_then(|value| {
            value
                .get("worktree_pool")?
                .get("lease_ttl_secs")?
                .as_integer()
        })
        .unwrap_or(aida_core::worktree_pool::DEFAULT_LEASE_TTL_SECS)
}

/// Read `[worktree] init_submodules` from `.aida/config.toml`. Missing or
/// invalid values default to true so AIDA-created worktrees are build-ready for
/// repos with vendored git submodules.
// trace:BUG-899 | ai:codex
pub(crate) fn worktree_config_init_submodules(project_root: &std::path::Path) -> bool {
    std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        .ok()
        .and_then(|body| toml::from_str::<toml::Value>(&body).ok())
        .and_then(|value| value.get("worktree")?.get("init_submodules")?.as_bool())
        .unwrap_or(true)
}

/// Hook commands for `key` (`post_create` / `pre_destroy`), sourced ONLY from
/// the machine-global `~/.aida/config.toml`. Repo-level config is deliberately
/// ignored — cloning a repo must never run arbitrary shell on your machine
/// (treehouse's stance).
///
/// For `post_create`, the opt-in `[worktree_pool] prewarm_build = true` knob
/// (also global-only, since it triggers a build) appends the canonical
/// backgrounded `cargo build` pre-warm so a freshly-created pool tree starts
/// warm on first use (TASK-1010).
// trace:STORY-714 trace:TASK-1010 | ai:claude
pub(crate) fn worktree_pool_global_hooks(key: &str) -> Vec<String> {
    let Some(home) = crate::home_dir() else {
        return Vec::new();
    };
    let Ok(body) = std::fs::read_to_string(home.join(".aida").join("config.toml")) else {
        return Vec::new();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return Vec::new();
    };
    worktree_pool_hooks_from_config(&value, key)
}

/// Pure parse of the `[worktree_pool]` hook list for `key`, plus the TASK-1010
/// `prewarm_build` injection. Split out so the sourcing rule (global-config
/// only, in `worktree_pool_global_hooks`) stays where it belongs and this
/// decision — which commands run for a phase — is unit-testable.
// trace:TASK-1010 | ai:claude
pub(crate) fn worktree_pool_hooks_from_config(value: &toml::Value, key: &str) -> Vec<String> {
    let pool = value.get("worktree_pool");
    let mut hooks: Vec<String> = pool
        .and_then(|w| w.get(key))
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    // Opt-in cargo pre-warm — turn cold-create into warm-on-first-use. The
    // command is backgrounded (see PREWARM_BUILD_COMMAND) so it never delays the
    // handout; appended after any explicit hooks so a user's own post_create
    // still runs first.
    if key == "post_create"
        && pool
            .and_then(|w| w.get("prewarm_build"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        hooks.push(aida_core::worktree_hooks::PREWARM_BUILD_COMMAND.to_string());
    }
    hooks
}

#[cfg(test)]
#[path = "tests/task_1010_prewarm_tests.rs"]
mod task_1010_prewarm_tests;

/// Trust policy for spec-authored acceptance commands, sourced ONLY from the
/// machine-global `~/.aida/config.toml` `[review]` table — the same sourcing
/// rule as [`worktree_pool_global_hooks`]. Repo config (branch-local AND the
/// trusted default-branch copy), the store, env vars and CLI flags are
/// deliberately not trust sources: an unattended-drain PR could otherwise
/// merge the opt-in through the very review it switches on. Every failure
/// (no home, no file, unreadable, malformed, wrong types) resolves to the
/// denied default and is reported once on stderr.
// trace:STORY-1476 | ai:claude
pub(crate) fn acceptance_command_policy_global() -> graded_review::AcceptanceCommandPolicy {
    match acceptance_command_policy_global_quiet() {
        Ok(policy) => policy,
        Err(reason) => {
            eprintln!(
                "  {} graded review: spec-authored acceptance commands will not run ({reason}); \
                 criteria that need them go to the reviewer seat as manual checks",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            graded_review::AcceptanceCommandPolicy::default()
        }
    }
}

/// [`acceptance_command_policy_global`] without the stderr notice: `Err` names
/// why the policy is denied. Used by `aida config show` to render the value.
// trace:STORY-1476 | ai:claude
pub(crate) fn acceptance_command_policy_global_quiet(
) -> std::result::Result<graded_review::AcceptanceCommandPolicy, String> {
    let Some(home) = crate::home_dir() else {
        return Err("home directory could not be resolved".to_string());
    };
    let path = home.join(".aida").join("config.toml");
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("no [review] opt-in in ~/.aida/config.toml".to_string());
        }
        Err(e) => return Err(format!("could not read ~/.aida/config.toml: {e}")),
    };
    let value = toml::from_str::<toml::Value>(&body)
        .map_err(|e| format!("~/.aida/config.toml did not parse: {e}"))?;
    acceptance_command_policy_from_toml(&value)
}

/// Pure parse of `[review] run_acceptance_commands` +
/// `acceptance_command_allow`. Fail-closed: anything other than a well-typed,
/// enabled, non-empty allowlist of clean entries is `Err` (denied), never a
/// partially-honoured policy — a bad element does not get skipped. Refused
/// characters are checked on the entry as written, before trimming, so a
/// malformed entry cannot be normalised into an accepted one.
// trace:STORY-1476 | ai:claude
pub(crate) fn acceptance_command_policy_from_toml(
    value: &toml::Value,
) -> std::result::Result<graded_review::AcceptanceCommandPolicy, String> {
    let Some(review) = value.get("review") else {
        return Err("no [review] table in ~/.aida/config.toml".to_string());
    };
    if !review.is_table() {
        return Err("[review] is not a table".to_string());
    }
    let enabled = match review.get("run_acceptance_commands") {
        None => return Err("[review] run_acceptance_commands is not set".to_string()),
        Some(v) => v.as_bool().ok_or_else(|| {
            "[review] run_acceptance_commands must be a boolean (true/false)".to_string()
        })?,
    };
    // Type-check the allowlist even when disabled, so a wrong type anywhere
    // in the pair is reported rather than silently tolerated.
    let allow: Vec<String> = match review.get("acceptance_command_allow") {
        None => Vec::new(),
        Some(v) => {
            let arr = v
                .as_array()
                .ok_or_else(|| "[review] acceptance_command_allow must be an array".to_string())?;
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let entry = item.as_str().ok_or_else(|| {
                    "[review] acceptance_command_allow entries must all be strings".to_string()
                })?;
                // Fail closed on the RAW entry, before any trimming: `*\r`
                // would otherwise normalise to `*` and be accepted as full
                // trust even though CR is refused.
                // trace:STORY-1476 trace:TASK-1545 | ai:claude
                if graded_review::contains_refused_chars(entry) {
                    return Err(format!(
                        "[review] acceptance_command_allow entry `{}` contains a shell \
                         metacharacter or control character",
                        entry.escape_debug()
                    ));
                }
                let trimmed = entry.trim();
                if trimmed.is_empty() {
                    return Err("[review] acceptance_command_allow has an empty entry".to_string());
                }
                // `*` is either the whole entry (full trust) or the last word
                // (any trailing arguments); anywhere else it is a typo.
                let words: Vec<&str> = trimmed.split_whitespace().collect();
                if words
                    .iter()
                    .enumerate()
                    .any(|(i, w)| *w == "*" && i + 1 != words.len())
                {
                    return Err(format!(
                        "[review] acceptance_command_allow entry `{trimmed}`: `*` may only be the \
                         last word"
                    ));
                }
                out.push(trimmed.to_string());
            }
            out
        }
    };
    if !enabled {
        return Err("[review] run_acceptance_commands = false".to_string());
    }
    if allow.is_empty() {
        return Err(
            "[review] run_acceptance_commands = true but acceptance_command_allow is missing or \
             empty"
                .to_string(),
        );
    }
    Ok(graded_review::AcceptanceCommandPolicy {
        enabled: true,
        allow,
        repo_optin_ignored: false,
    })
}

/// Display-only detection of a repo-level `[review]` opt-in — the branch-local
/// `.aida/config.toml` or its trusted default-branch copy naming
/// `run_acceptance_commands` / `acceptance_command_allow`. The result feeds a
/// one-line notice in the verdict summary and NEVER the policy itself.
// trace:STORY-1476 | ai:claude
pub(crate) fn repo_review_optin_present(project_root: &std::path::Path) -> bool {
    let names_optin = |body: &str| {
        toml::from_str::<toml::Value>(body)
            .ok()
            .and_then(|v| v.get("review").cloned())
            .is_some_and(|review| {
                review.get("run_acceptance_commands").is_some()
                    || review.get("acceptance_command_allow").is_some()
            })
    };
    let local = std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        .ok()
        .is_some_and(|body| names_optin(&body));
    local
        || crate::trusted_config::read_trusted_config_toml_local(project_root)
            .is_some_and(|body| names_optin(&body))
}

#[cfg(test)]
#[path = "tests/story_1476_acceptance_trust_tests.rs"]
mod story_1476_acceptance_trust_tests;

#[cfg(test)]
#[path = "tests/task_1558_agents_mcp_type_tests.rs"]
mod task_1558_agents_mcp_type_tests;

#[cfg(test)]
#[path = "tests/bug_1700_worktree_parent_tests.rs"]
mod bug_1700_worktree_parent_tests;

#[cfg(test)]
#[path = "tests/task_1561_pickup_worktree_parent_tests.rs"]
mod task_1561_pickup_worktree_parent_tests;

#[cfg(test)]
#[path = "tests/bug_1701_drain_routing_tests.rs"]
mod bug_1701_drain_routing_tests;

#[cfg(test)]
#[path = "tests/bug_1761_preclaim_collision_tests.rs"]
mod bug_1761_preclaim_collision_tests;

#[cfg(test)]
#[path = "tests/bug_1763_merged_branch_preclaim_tests.rs"]
mod bug_1763_merged_branch_preclaim_tests;

pub(crate) fn handle_worktree_pool_command(cmd: &WorktreePoolCommand) -> Result<()> {
    let project_root = find_project_root()?;
    match cmd {
        WorktreePoolCommand::Status { json } => worktree_pool_status(&project_root, *json),
        WorktreePoolCommand::Acquire { lease_holder, json } => {
            let opts = aida_core::worktree_pool::AcquireOptions {
                lease_holder: lease_holder.clone(),
                max_trees: worktree_pool_config_max_trees(&project_root),
                lease_ttl_secs: Some(worktree_pool_config_lease_ttl_secs(&project_root)),
                post_create_hooks: worktree_pool_global_hooks("post_create"),
                init_submodules: worktree_config_init_submodules(&project_root),
                parent_dir: worktree_pool_config_worktree_parent(&project_root),
            };
            let path = aida_core::worktree_pool::acquire(&project_root, &opts)?;
            if *json {
                println!("{}", serde_json::json!({ "path": path }));
            } else {
                println!("{}", path.display());
            }
            Ok(())
        }
        WorktreePoolCommand::Return { path } => {
            let target = match path {
                Some(p) => std::path::PathBuf::from(p),
                None => std::env::current_dir()?,
            };
            aida_core::worktree_pool::return_to_pool(&project_root, &target)?;
            println!(
                "{} returned {} to the warm-pool (reset to a clean base, marked idle — directory + build cache kept)",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                target.display()
            );
            Ok(())
        }
        WorktreePoolCommand::Adopt {
            paths,
            all,
            no_dry_run,
            include_unlanded,
            json,
        } => worktree_pool_adopt(
            &project_root,
            paths,
            *all,
            !*no_dry_run,
            *include_unlanded,
            *json,
        ),
        WorktreePoolCommand::Destroy {
            paths,
            all,
            no_dry_run,
            include_unlanded,
            include_in_use,
            include_leased,
            json,
        } => worktree_pool_destroy(
            &project_root,
            paths,
            *all,
            !*no_dry_run,
            *include_unlanded,
            *include_in_use,
            *include_leased,
            *json,
        ),
    }
}

pub(crate) fn worktree_pool_status(project_root: &std::path::Path, json: bool) -> Result<()> {
    let cwd = std::env::current_dir().ok();
    let lease_ttl = worktree_pool_config_lease_ttl_secs(project_root);
    let rows = aida_core::worktree_pool::list(project_root, cwd.as_deref(), lease_ttl)?;
    // Warm-pool HIT-RATE telemetry (reuse vs create) — proves the warm-cache
    // payoff empirically. Counts live in the pool state (counts only, no
    // paths/content). trace:TASK-1012 | ai:claude
    let pool_state = aida_core::worktree_pool::read_state(project_root).unwrap_or_default();

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.entry.name,
                    "path": r.entry.path,
                    "state": r.state.label(),
                    "leased": r.entry.leased,
                    "lease_holder": r.entry.lease_holder,
                    "leased_at": r.entry.leased_at,
                    "owner_pid": r.entry.owner_pid,
                    "head": r.head,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "pool": arr,
                "telemetry": {
                    "reuse_count": pool_state.reuse_count,
                    "create_count": pool_state.create_count,
                    "total_acquires": pool_state.total_acquires(),
                    "hit_rate": pool_state.hit_rate(),
                },
            }))?
        );
        return Ok(());
    }

    if rows.is_empty() {
        println!("warm-pool is empty — `aida worktree pool acquire` creates the first tree");
        print_worktree_pool_hit_rate(&pool_state);
        return Ok(());
    }

    use crate::glyphs::Glyph;
    use aida_core::worktree_pool::PoolState;
    println!("{}", "WORKTREE WARM-POOL".bold());
    for r in &rows {
        // Registry-managed glyphs route through `crate::glyph` so they get the
        // ASCII fallback on terminals that can't draw them (glyph-lint). The
        // filled-circle/diamond/ellipsis markers below have no registry variant
        // and stay raw. trace:STORY-714 trace:TASK-835
        let dot = match r.state {
            PoolState::Available => "●".green(),
            PoolState::InUse => crate::glyph(Glyph::InFlight).yellow(),
            PoolState::Leased => "◆".cyan(),
            PoolState::Expired => crate::glyph(Glyph::Warning).yellow(),
            PoolState::Dirty => crate::glyph(Glyph::Cross).red(),
            PoolState::Destroying => "…".dimmed(),
            PoolState::Here => crate::glyph(Glyph::FlowActive).bold(),
        };
        let head = r
            .head
            .as_deref()
            .map(|h| h[..h.len().min(9)].to_string())
            .unwrap_or_else(|| "-".to_string());
        let holder = r
            .entry
            .lease_holder
            .as_deref()
            .map(|h| format!("  lease:{h}"))
            .unwrap_or_default();
        println!(
            "  {} {:<14} {:<11} {}  {}{}",
            dot,
            r.entry.name,
            r.state.label(),
            head.dimmed(),
            r.entry.path.display(),
            holder.cyan()
        );
    }
    print_worktree_pool_hit_rate(&pool_state);
    Ok(())
}

/// Render the warm-pool HIT-RATE line — reuse (warm-cache hit) vs create (cold
/// mint), and the ratio that proves the warm-cache payoff empirically. Silent
/// until the first acquire is observed (nothing to report yet). Counts only, no
/// paths/content.
// trace:TASK-1012 | ai:claude
pub(crate) fn print_worktree_pool_hit_rate(pool: &aida_core::worktree_pool::Pool) {
    match pool.hit_rate() {
        Some(rate) => {
            let pct = format!("{:.0}%", rate * 100.0);
            println!(
                "  hit-rate {}  ({} reuse / {} total, {} fresh mint)",
                pct.bold(),
                pool.reuse_count,
                pool.total_acquires(),
                pool.create_count,
            );
        }
        None => println!("{}", "  hit-rate n/a  (no acquires observed yet)".dimmed()),
    }
}

/// `aida worktree pool adopt` — register pre-pool sibling worktrees into the
/// warm-pool registry. Preview by default; the only side effect when applied is
/// new entries in the registry (no worktree is ever reset, checked out, or
/// removed). Exits non-zero only on a hard error — an all-skipped pass is a
/// legitimate "nothing to migrate" outcome.
// trace:TASK-1009 | ai:claude
pub(crate) fn worktree_pool_adopt(
    project_root: &std::path::Path,
    paths: &[String],
    all: bool,
    dry_run: bool,
    include_unlanded: bool,
    json: bool,
) -> Result<()> {
    use aida_core::worktree_pool_adopt::{self as wa, AdoptAction, AdoptOptions, AdoptSelector};

    let selector = if all {
        AdoptSelector::All
    } else if !paths.is_empty() {
        AdoptSelector::Paths(paths.iter().map(std::path::PathBuf::from).collect())
    } else {
        anyhow::bail!(
            "specify worktree path(s) to adopt, or pass --all to consider every worktree of this repository"
        );
    };

    let opts = AdoptOptions {
        dry_run,
        include_unlanded,
        max_trees: worktree_pool_config_max_trees(project_root),
        ..Default::default()
    };
    // A worktree a live session is working in is never adopted — the pool must
    // not start accounting for a tree somebody is inside.
    let live = wa::live_lease_worktrees(project_root);
    let report = wa::adopt(project_root, &selector, &opts, &live)?;

    if json {
        let arr: Vec<_> = report
            .targets
            .iter()
            .map(|t| {
                serde_json::json!({
                    "path": t.path,
                    "name": t.name,
                    "class": t.class.label(),
                    "action": match t.action {
                        AdoptAction::WouldAdopt => "would-adopt",
                        AdoptAction::Adopted => "adopted",
                        AdoptAction::Skipped => "skipped",
                    },
                    "reserved": t.reserved,
                    "reason": t.reason,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "dry_run": report.dry_run,
                "adopted": report.adopted_count(),
                "would_adopt": report.would_adopt_count(),
                "skipped": report.skipped_count(),
                "targets": arr,
            }))?
        );
        return Ok(());
    }

    if report.targets.is_empty() {
        println!("no candidate worktrees to adopt");
        return Ok(());
    }

    if report.dry_run {
        println!(
            "{} dry-run — registry unchanged (pass --no-dry-run to apply)",
            "→".dimmed()
        );
    }
    for t in &report.targets {
        let mark = match t.action {
            AdoptAction::WouldAdopt => "would adopt".yellow(),
            AdoptAction::Adopted => "adopted".green(),
            AdoptAction::Skipped => "skipped".dimmed(),
        };
        println!("  {}  {}  {}", mark, t.path.display(), t.reason.dimmed());
    }
    if !report.dry_run {
        println!(
            "{} {} worktree(s) adopted into the warm-pool",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            report.adopted_count()
        );
        if report.targets.iter().any(|t| t.reserved) {
            println!(
                "  reserved trees keep their unlanded work and are never handed out — release one with `aida worktree pool return <path>`"
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn worktree_pool_destroy(
    project_root: &std::path::Path,
    paths: &[String],
    all: bool,
    dry_run: bool,
    include_unlanded: bool,
    include_in_use: bool,
    include_leased: bool,
    json: bool,
) -> Result<()> {
    use aida_core::worktree_pool_destroy::{
        self as wd, DestroyAction, DestroyOptions, DestroySelector,
    };

    let selector = if all {
        DestroySelector::All
    } else if !paths.is_empty() {
        DestroySelector::Paths(paths.iter().map(std::path::PathBuf::from).collect())
    } else {
        anyhow::bail!("specify worktree path(s) to destroy, or pass --all for a bulk sweep");
    };

    let opts = DestroyOptions {
        dry_run,
        include_unlanded,
        include_in_use,
        include_leased,
        pre_destroy_hooks: worktree_pool_global_hooks("pre_destroy"),
    };

    let mut salvage = |p: &std::path::Path| {
        let _ = salvage_worktree_patch(project_root, "worktree-pool", Some("pool"), p);
    };
    let report = wd::destroy(project_root, &selector, &opts, &mut salvage)?;

    if json {
        let arr: Vec<_> = report
            .targets
            .iter()
            .map(|t| {
                serde_json::json!({
                    "path": t.entry.path,
                    "name": t.entry.name,
                    "class": t.class.label(),
                    "action": match t.action {
                        DestroyAction::WouldRemove => "would-remove",
                        DestroyAction::Removed => "removed",
                        DestroyAction::Skipped => "skipped",
                    },
                    "reason": t.reason,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "dry_run": report.dry_run,
                "removed": report.removed_count(),
                "targets": arr,
            }))?
        );
        return Ok(());
    }

    if report.targets.is_empty() {
        println!("no matching pool worktrees");
        return Ok(());
    }

    if report.dry_run {
        println!(
            "{} dry-run — nothing removed (pass --no-dry-run to apply)",
            "→".dimmed()
        );
    }
    for t in &report.targets {
        let mark = match t.action {
            DestroyAction::WouldRemove => "would remove".yellow(),
            DestroyAction::Removed => "removed".green(),
            DestroyAction::Skipped => "skipped".dimmed(),
        };
        println!(
            "  {}  {}  {}",
            mark,
            t.entry.path.display(),
            t.reason.dimmed()
        );
    }
    if !report.dry_run {
        println!(
            "{} {} worktree(s) removed",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            report.removed_count()
        );
    }
    Ok(())
}

/// `aida worktree add <epic|spec>` — create (or re-affirm) the worktree and
/// print its path. Mirrors `git worktree add`: prints the path, does NOT cd.
/// An epic arg keeps the STORY-716 scoping-only behavior; a single non-epic
/// spec additionally takes the implementer lease (STORY-742).
// trace:STORY-716 trace:STORY-742 | ai:claude
pub(crate) fn handle_worktree_add(
    arg: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
) -> Result<()> {
    let out = match classify_worktree_arg(arg) {
        WorktreeTarget::Epic => ensure_epic_worktree(arg, path_override, branch_override)?,
        WorktreeTarget::Spec { display, focus } => {
            ensure_spec_worktree(&display, &focus, path_override, branch_override, "add")?
        }
    };
    let verb = if out.created {
        "Created"
    } else {
        "Already exists"
    };
    println!(
        "{} {} worktree for {} at {}",
        crate::glyph(crate::glyphs::Glyph::Check),
        verb,
        out.focus.cyan().bold(),
        out.path.display().to_string().cyan(),
    );
    println!("  branch: {}  ·  focus: {}", out.branch, out.focus);
    print_worktree_next("add", arg, &out, false);
    Ok(())
}

/// `aida worktree enter <epic|spec>` — create-if-missing, then emit `cd
/// '<path>'` on stdout for the `aida()` shell wrapper to auto-eval. Human-facing
/// status goes to STDERR so it never contaminates the eval'd stdout. An epic arg
/// keeps the STORY-716 scoping-only behavior; a single non-epic spec
/// additionally takes the implementer lease (STORY-742).
// trace:STORY-716 trace:STORY-742 | ai:claude
pub(crate) fn handle_worktree_enter(
    arg: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
) -> Result<()> {
    let out = match classify_worktree_arg(arg) {
        WorktreeTarget::Epic => ensure_epic_worktree(arg, path_override, branch_override)?,
        WorktreeTarget::Spec { display, focus } => {
            ensure_spec_worktree(&display, &focus, path_override, branch_override, "enter")?
        }
    };
    let verb = if out.created {
        "Created and entering"
    } else {
        "Entering"
    };
    // Status to stderr — the eval-wrapper captures stdout only.
    eprintln!(
        "{} {} worktree for {} ({} · focus {})",
        crate::glyph(crate::glyphs::Glyph::Check),
        verb,
        out.focus,
        out.branch,
        out.focus,
    );
    // `worktree enter` is itself a pickup surface. Keep stdout pure shell code
    // and put the same capped, META-citing protocol block on stderr.
    // trace:STORY-1221 | ai:codex
    if let Ok(main_root) = find_main_worktree_root() {
        if let Ok(store) = Storage::new(main_root.join(".aida-store")).load() {
            if let Some(req) = store.requirements.iter().find(|r| spec_matches(r, arg)) {
                if let Some(block) = protocol_cmd::pickup_block_for_requirement(&store, req) {
                    eprintln!("\n{}\n", block);
                }
            }
        }
    }
    // BUG-654: warn (to STDERR, never stdout) when the installed shell wrapper
    // can't auto-eval this `cd`, so the silent no-op is explained instead of
    // leaving the operator wondering why `aida focus` shows nothing afterward.
    if !wrapper_can_eval_worktree_enter() {
        eprintln!(
            "  {} shell wrapper looks stale or missing — the `cd` below won't auto-apply. \
             Run `aida dev shell-init --install` to enable auto-cd, or \
             `eval \"$(aida worktree enter {})\"`.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            arg,
        );
    }
    let shell_payload_evaled = wrapper_can_eval_worktree_enter();
    print_worktree_next("enter", arg, &out, shell_payload_evaled);
    // BUG-780: record the lease file backing the marker so the wrapper's prompt
    // hook can tell a live session from one ended elsewhere.
    // trace:TASK-1160 trace:BUG-780 | ai:claude
    let lease_file = out.lease_id.as_deref().and_then(|id| {
        find_main_worktree_root()
            .ok()
            .map(|root| lease_path(&root, id))
    });
    // trace:TASK-1171 | ai:claude — everything printed while this guard is
    // alive is SHELL CODE, marked as such for the wrapper.
    {
        let _eval = crate::shell_eval::EvalBlock::open();
        print!(
            "{}",
            enter_shell_payload(&out.path, &out.focus, lease_file.as_deref())
        );
    }
    Ok(())
}

/// BUG-780: the worktree session a shell CARRIES — the `AIDA_SESSION_ID` export
/// and the `(wt:...) ` PS1 segment `enter` spliced in. Both live in the shell,
/// not in cwd, so a bare `cd` out (or a `session end` run from another shell)
/// leaves them behind. `worktree exit` clears whichever of the two is present.
// trace:BUG-780 | ai:claude
pub(crate) struct CarriedSession {
    pub(crate) session_id: Option<String>,
    pub(crate) marker: Option<String>,
}

/// Read `key` from the env, trimmed, treating blank as unset.
// trace:BUG-780 | ai:claude
pub(crate) fn carried_env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// BUG-780: is this shell carrying a worktree session? `None` means there is
/// genuinely nothing to step out of, wherever the shell stands.
// trace:BUG-780 | ai:claude
pub(crate) fn carried_worktree_session() -> Option<CarriedSession> {
    let session_id = carried_env_value("AIDA_SESSION_ID");
    let marker = carried_env_value("AIDA_WT_PS1_PREFIX");
    if session_id.is_none() && marker.is_none() {
        return None;
    }
    Some(CarriedSession { session_id, marker })
}

/// Does `lease_id` name the session the shell carries? Exact match, or the
/// short (>= 8 char) prefix the session verbs print — so an id copied off a
/// status line still resolves. A carried id that matches NO lease is dangling:
/// the session was ended from another shell and the marker is pointing at work
/// that no longer exists. Pure so the decision is unit-testable.
// trace:BUG-780 | ai:claude
pub(crate) fn lease_id_matches(lease_id: &str, carried: &str) -> bool {
    let carried = carried.trim().to_ascii_lowercase();
    if carried.is_empty() {
        return false;
    }
    let lease_id = lease_id.trim().to_ascii_lowercase();
    lease_id == carried || (carried.len() >= 8 && lease_id.starts_with(&carried))
}

/// The env names a worktree's generated `session-env.sh` exports — the extra
/// unsets `worktree exit` emits beyond the well-known base set, derived from
/// the same file `enter` sources so the two never drift.
// trace:TASK-1160 trace:BUG-780 | ai:claude
pub(crate) fn session_env_unset_names(worktree: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(worktree.join(".aida").join("session-env.sh"))
        .map(|body| {
            // Allowlisted names only: a branch-committed file must not make
            // `worktree exit` unset PATH. trace:BUG-1624 | ai:claude
            parse_session_env(&body)
                .into_iter()
                .map(|(n, _)| n)
                .filter(|n| SESSION_ENV_NAMES.contains(&n.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

/// Pure decision: is the shell STANDING in a scoped worktree? `toplevel` is the
/// checkout containing cwd (`None` when cwd is outside any git checkout), and a
/// toplevel equal to the main checkout — or an unresolvable main root — means
/// there is no worktree to step out of. This is only half the exit question:
/// BUG-780's other half is what the shell CARRIES, which is cwd-independent.
// trace:BUG-780 | ai:claude
pub(crate) fn scoped_worktree_for_exit(
    toplevel: Option<&std::path::Path>,
    main_root: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    let toplevel = toplevel?;
    let main_root = main_root?;
    (crate::worktree::canonical_or_self(toplevel) != crate::worktree::canonical_or_self(main_root))
        .then(|| toplevel.to_path_buf())
}

/// `aida worktree exit` — the symmetric step-out: emit `cd '<main root>'`,
/// unset the session env exports `enter` applied, and strip the `(wt:...)`
/// PS1 segment, all via the same auto-evaled stdout payload. The session
/// lease and the spec's status are untouched — the worktree stays live for
/// re-enter; `aida session end` is the verb that finishes the work.
///
/// BUG-780: the step-out is keyed on what the shell CARRIES, not only on cwd.
/// Standing outside a scoped worktree while still carrying its session env (a
/// bare `cd` out, or a session ended from another shell) used to be a no-op
/// that left the `(wt:)` marker stuck with no verb to clear it — now the
/// session env + marker are cleared wherever you stand, minus the `cd` (moving
/// the shell out of an unrelated directory would be the surprise). Only a shell
/// carrying nothing gets the friendly no-op.
// trace:TASK-1160 | ai:claude
// trace:BUG-780 | ai:claude
pub(crate) fn handle_worktree_exit() -> Result<()> {
    let carried = carried_worktree_session();
    // Outside an AIDA project entirely we can still clear what the shell
    // carries — the env is the shell's, not the project's.
    let main_root = match find_main_worktree_root() {
        Ok(root) => Some(root),
        Err(e) => {
            if carried.is_none() {
                return Err(e);
            }
            None
        }
    };

    // Where is the shell standing? The toplevel of the checkout containing
    // cwd, or None when cwd isn't inside any git checkout.
    let toplevel = std::env::current_dir()
        .ok()
        .and_then(|cwd| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(["rev-parse", "--show-toplevel"])
                .output()
                .ok()
        })
        .filter(|o| o.status.success())
        .map(|o| std::path::PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string()));
    let toplevel = scoped_worktree_for_exit(toplevel.as_deref(), main_root.as_deref());

    let Some(toplevel) = toplevel else {
        return worktree_exit_from_outside(main_root.as_deref(), carried);
    };

    // Session env exports to clear: always the well-known base set, plus
    // whatever else this worktree's session-env.sh exports.
    let extra_unsets = session_env_unset_names(&toplevel);
    let main_root = main_root.expect("a scoped worktree implies a resolvable main root");

    // The lease (if any) survives the step-out — say so, with the way back.
    let toplevel_canon = crate::worktree::canonical_or_self(&toplevel);
    let lease = list_leases(&main_root)
        .into_iter()
        .filter(|l| crate::worktree::canonical_or_self(&l.worktree_path) == toplevel_canon)
        .max_by_key(|l| l.started_at);

    // Human-facing status to STDERR — the eval-wrapper captures stdout only.
    eprintln!(
        "{} Stepping out of {} back to {}",
        crate::glyph(crate::glyphs::Glyph::Check),
        toplevel.display().to_string().cyan(),
        main_root.display().to_string().cyan(),
    );
    match &lease {
        Some(l) => {
            let short = &l.id[..l.id.len().min(8)];
            eprintln!(
                "  stepped out — session {} ({}) still holds the worktree; re-enter with \
                 `aida worktree enter {}`, finish with `aida session end {}`",
                short.cyan(),
                l.scope.cyan().bold(),
                l.scope,
                short,
            );
        }
        None => {
            if let Some(focus) = crate::focus::read_focus_marker(&toplevel) {
                eprintln!(
                    "  the worktree stays put — re-enter with `aida worktree enter {}`",
                    focus
                );
            }
        }
    }
    // Warn (STDERR) when the installed wrapper predates the exit verb — its
    // case statement won't auto-eval this payload, so the cd would silently
    // no-op (same staleness UX as enter).
    if !wrapper_can_eval_worktree_exit() {
        eprintln!(
            "  {} shell wrapper looks stale or missing — the `cd` below won't auto-apply. \
             Run `aida dev shell-init --install` to refresh it, or \
             `eval \"$(aida worktree exit)\"`.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        );
    }
    // trace:TASK-1171 | ai:claude
    {
        let _eval = crate::shell_eval::EvalBlock::open();
        print!(
            "{}",
            crate::worktree::exit_shell_payload(&main_root, &extra_unsets)
        );
    }
    Ok(())
}

/// BUG-780: `worktree exit` run from OUTSIDE a scoped worktree. Carrying
/// nothing is the old friendly no-op. Carrying a session (id and/or the `(wt:)`
/// PS1 marker) clears it right here — the shell's carrying is the thing exit
/// ends, and it outlives the cwd that created it. No `cd` is emitted: the shell
/// is already out of the worktree, and relocating it from wherever it stands
/// would be the surprise.
// trace:BUG-780 | ai:claude
pub(crate) fn worktree_exit_from_outside(
    main_root: Option<&std::path::Path>,
    carried: Option<CarriedSession>,
) -> Result<()> {
    let Some(carried) = carried else {
        let where_main = main_root
            .map(|m| format!(" — the main checkout is {}", m.display().to_string().cyan()))
            .unwrap_or_default();
        eprintln!(
            "{} Not inside a scoped worktree{}. Nothing to step out of.",
            crate::glyph(crate::glyphs::Glyph::Check),
            where_main,
        );
        return Ok(());
    };

    // Resolve the carried id against the live leases so the report can say
    // whether the session is still running or was ended elsewhere, and so the
    // unset list covers that worktree's own session-env exports.
    let leases = main_root.map(list_leases).unwrap_or_default();
    let lease = carried
        .session_id
        .as_deref()
        .and_then(|id| leases.iter().find(|l| lease_id_matches(&l.id, id)));
    let extra_unsets = lease
        .map(|l| session_env_unset_names(&l.worktree_path))
        .unwrap_or_default();

    let label = lease
        .map(|l| l.scope.clone())
        .or_else(|| {
            carried
                .marker
                .as_deref()
                .and_then(|m| m.trim().strip_prefix("(wt:"))
                .and_then(|m| m.strip_suffix(')'))
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "this shell's session".to_string());

    eprintln!(
        "{} Stepped out of {} — this shell carried it but was standing elsewhere.",
        crate::glyph(crate::glyphs::Glyph::Check),
        label.cyan().bold(),
    );
    match lease {
        Some(l) => {
            let short = &l.id[..l.id.len().min(8)];
            eprintln!(
                "  the session is still live — re-enter with `aida worktree enter {}`, \
                 finish with `aida session end {}`",
                l.scope, short,
            );
        }
        None => {
            eprintln!(
                "  the session it pointed at is gone (ended elsewhere) — the marker was stale."
            );
        }
    }
    if !wrapper_can_eval_worktree_exit() {
        eprintln!(
            "  {} shell wrapper looks stale or missing — the clearing below won't auto-apply. \
             Run `aida dev shell-init --install` to refresh it, or \
             `eval \"$(aida worktree exit)\"`.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        );
    } else if !wrapper_can_self_heal_wt_marker() {
        eprintln!(
            "  {} refresh the shell wrapper (`aida dev shell-init --install`) so the marker \
             also clears itself when a session ends elsewhere.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        );
    }
    // trace:TASK-1171 | ai:claude
    {
        let _eval = crate::shell_eval::EvalBlock::open();
        print!("{}", crate::worktree::session_clear_payload(&extra_unsets));
    }
    Ok(())
}

// trace:TASK-1156 | ai:codex
pub(crate) fn print_worktree_next(
    command: &str,
    arg: &str,
    out: &WorktreeOutcome,
    shell_payload_evaled: bool,
) {
    eprintln!();
    eprintln!("Next:");
    match (command, out.lease_id.as_deref(), shell_payload_evaled) {
        ("enter", Some(_), true) => {
            eprintln!(
                "  {}",
                format!("claude    # then /aida-implement {}", out.focus).cyan()
            );
        }
        ("enter", Some(_), false) => {
            eprintln!(
                "  {}",
                format!("eval \"$(aida worktree enter {})\"    # cd in", arg).cyan()
            );
            eprintln!(
                "  {}",
                format!("claude    # then /aida-implement {}", out.focus).cyan()
            );
        }
        ("add", Some(_), _) => {
            eprintln!(
                "  {}",
                format!(
                    "aida worktree enter {}    # cd in, then /aida-implement {}",
                    arg, out.focus
                )
                .cyan()
            );
        }
        ("enter", None, true) => {
            eprintln!("  {}", "work in this shell; the cd already happened".cyan());
        }
        ("enter", None, false) => {
            eprintln!(
                "  {}",
                format!("eval \"$(aida worktree enter {})\"    # cd in", arg).cyan()
            );
        }
        _ => {
            eprintln!(
                "  {}",
                format!("aida worktree enter {}    # cd in", arg).cyan()
            );
        }
    }
    // Never suggest a raw `source` of the worktree's session-env file: a
    // branch can commit it. Run enter through the eval so the filtered
    // payload applies. trace:BUG-1624 | ai:claude
    if command == "enter" && out.has_session_env && !shell_payload_evaled {
        eprintln!(
            "  {}    {}",
            "cat .aida/session-env.sh".cyan(),
            "# review it; the eval'd enter applies only CARGO_TARGET_DIR, AIDA_AGENT_TYPE, AIDA_BIN"
                .dimmed()
        );
    }
    if let Some(id) = out.lease_id.as_deref() {
        eprintln!();
        eprintln!("When finished:");
        // trace:TASK-1160 | ai:claude
        if command == "enter" {
            eprintln!(
                "  {}",
                "aida worktree exit    # step back out (session stays live)".dimmed()
            );
        }
        eprintln!(
            "  {}",
            format!("aida session end {}", &id[..id.len().min(8)]).dimmed()
        );
    }
}

/// BUG-654: does the installed `aida()` shell wrapper auto-eval `worktree
/// enter`? The wrapper advertises which verb groups it auto-evals via the
/// comma-separated `AIDA_SHELL_WRAPPER` marker (exported from `SHELL_HELPERS`).
/// A wrapper installed before STORY-716 added `worktree enter` to its eval case
/// carries a marker WITHOUT the `worktree` capability; a shell with no wrapper
/// at all leaves the marker unset. Either way the `cd` line `worktree enter`
/// prints is never eval'd, so the cd silently no-ops. Returns true only when the
/// marker is present AND lists the `worktree` capability — the one wrapper shape
/// that will actually apply the cd.
// trace:BUG-654 | ai:claude
pub(crate) fn wrapper_can_eval_worktree_enter() -> bool {
    wrapper_marker_has_worktree_cap(std::env::var("AIDA_SHELL_WRAPPER").ok().as_deref())
}

/// Does the installed wrapper auto-eval `worktree exit`? A wrapper installed
/// before the exit verb existed advertises `worktree` (it evals `enter`) but
/// not `worktree-exit` — its case statement matches `"worktree enter"` only,
/// so the exit payload would print without being eval'd. Same staleness logic
/// as [`wrapper_can_eval_worktree_enter`], keyed on the newer capability.
// trace:TASK-1160 | ai:claude
pub(crate) fn wrapper_can_eval_worktree_exit() -> bool {
    wrapper_marker_has_cap(
        std::env::var("AIDA_SHELL_WRAPPER").ok().as_deref(),
        "worktree-exit",
    )
}

/// BUG-780: does the installed wrapper carry the prompt hook that drops the
/// `(wt:)` marker when the session it names has ended elsewhere? Only wrappers
/// advertising the `worktree-stale` capability self-heal; older ones need
/// `aida worktree exit` (or a hand-rolled `unset`) to clear a dangling marker.
// trace:BUG-780 | ai:claude
pub(crate) fn wrapper_can_self_heal_wt_marker() -> bool {
    wrapper_marker_has_cap(
        std::env::var("AIDA_SHELL_WRAPPER").ok().as_deref(),
        "worktree-stale",
    )
}

/// BUG-654: pure decision half of [`wrapper_can_eval_worktree_enter`] — given
/// the `AIDA_SHELL_WRAPPER` marker value (`None` when unset), is the `worktree`
/// capability advertised? Pure so the parsing is unit-testable without mutating
/// the process-global env var.
// trace:BUG-654 | ai:claude
pub(crate) fn wrapper_marker_has_worktree_cap(marker: Option<&str>) -> bool {
    wrapper_marker_has_cap(marker, "worktree")
}

/// Generalized capability probe over the comma-separated `AIDA_SHELL_WRAPPER`
/// marker: is `cap` advertised as an auto-evaled verb group? Whole-token,
/// case-insensitive compare — a substring of another token never counts.
// trace:BUG-654 trace:TASK-1160 trace:TASK-1171 | ai:claude
pub(crate) fn wrapper_marker_has_cap(marker: Option<&str>, cap: &str) -> bool {
    crate::shell_eval::marker_has_cap(marker, cap)
}

/// The single shell line `aida worktree enter` emits on stdout for the `aida()`
/// wrapper to auto-eval: `cd '<single-quote-escaped path>'`. Pure so the
/// emitted-shell contract is unit-testable without git.
// trace:STORY-716 | ai:claude
pub(crate) fn enter_cd_line(path: &std::path::Path) -> String {
    format!("cd '{}'", sh_single_quote(&path.display().to_string()))
}

/// TASK-1156: the wrapper-evaled worktree-enter payload carries the shell
/// mutations: cd into the worktree and source the generated session env exports
/// so CARGO_TARGET_DIR is live with no extra manual step. It also splices the
/// ambient `(wt:<focus>) ` segment into PS1 (recorded in `AIDA_WT_PS1_PREFIX`)
/// so the shell always shows where it is standing; `worktree exit` strips it.
// trace:TASK-1156 | ai:codex
// trace:TASK-1160 | ai:claude
pub(crate) fn enter_shell_payload(
    path: &std::path::Path,
    focus: &str,
    lease_file: Option<&std::path::Path>,
) -> String {
    let mut payload = format!("{}\n", enter_cd_line(path));
    let env_path = path.join(".aida").join("session-env.sh");
    if let Ok(body) = std::fs::read_to_string(env_path) {
        payload.push_str(&session_env_eval_lines(&body, &resolve_aida_exe()));
    }
    payload.push_str(&crate::worktree::ps1_wt_splice_block(focus, lease_file));
    payload
}

/// Re-render `.aida/session-env.sh` for the eval'd `worktree enter` payload.
///
/// The file lives in the worktree, so a branch can commit its own copy; its
/// body is never eval'd verbatim. Only the allowlisted names survive
/// ([`trusted_session_env`]), each value re-quoted. The PATH prepend comes
/// from `running_exe`, the binary doing the enter, never from the file.
// trace:BUG-1624 | ai:claude
pub(crate) fn session_env_eval_lines(body: &str, running_exe: &std::path::Path) -> String {
    let mut out = String::new();
    for (name, value) in trusted_session_env(body, running_exe) {
        out.push_str(&format!("export {name}={}\n", shell_single_quote(&value)));
        if name == "AIDA_BIN" {
            if let Some(dir) = running_exe.parent() {
                out.push_str(&format!(
                    "PATH={}:\"$PATH\"\n",
                    shell_single_quote(&dir.display().to_string())
                ));
            }
        }
    }
    out
}

/// `aida worktree list` — every registered git worktree annotated with its
/// `.aida/focus` (the epic it's scoped to, or `—` when unscoped).
// trace:STORY-716 | ai:claude
pub(crate) fn handle_worktree_list(json: bool) -> Result<()> {
    let main_root = find_main_worktree_root()?;
    let porcelain = std::process::Command::new("git")
        .arg("-C")
        .arg(&main_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .context("failed to invoke `git worktree list`")?;
    if !porcelain.status.success() {
        anyhow::bail!(
            "`git worktree list` failed: {}",
            String::from_utf8_lossy(&porcelain.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&porcelain.stdout);
    let paths = crate::worktree::parse_worktree_paths(&text);

    // Pair each worktree with its focus marker + current branch (re-parse the
    // porcelain for the branch line that follows each `worktree` record).
    let branches = parse_worktree_branches(&text);
    let rows: Vec<(std::path::PathBuf, Option<String>, Option<String>)> = paths
        .iter()
        .map(|p| {
            let focus = crate::focus::read_focus_marker(p);
            let branch = branches.get(p).cloned();
            (p.clone(), branch, focus)
        })
        .collect();

    if json {
        let arr: Vec<serde_json::Value> = rows
            .iter()
            .map(|(p, b, f)| {
                serde_json::json!({
                    "path": p.display().to_string(),
                    "branch": b,
                    "focus": f,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::json!(arr))?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No git worktrees found.");
        return Ok(());
    }
    println!("{}", "AIDA worktrees".bold());
    for (p, b, f) in &rows {
        let focus = f
            .as_deref()
            .map(|s| s.cyan().bold().to_string())
            .unwrap_or_else(|| "\u{2014}".dimmed().to_string());
        let branch = b.as_deref().unwrap_or("(detached)");
        println!(
            "  {}  [{}]  focus: {}",
            p.display().to_string().cyan(),
            branch,
            focus,
        );
    }
    Ok(())
}

/// Parse `git worktree list --porcelain` into a path -> branch-shortname map.
/// Each record is `worktree <path>` followed (when on a branch) by
/// `branch refs/heads/<name>`; a detached record has no branch line. Pure.
// trace:STORY-716 | ai:claude
pub(crate) fn parse_worktree_branches(
    porcelain: &str,
) -> std::collections::HashMap<std::path::PathBuf, String> {
    let mut map = std::collections::HashMap::new();
    let mut current: Option<std::path::PathBuf> = None;
    for line in porcelain.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current = Some(std::path::PathBuf::from(p.trim()));
        } else if let Some(b) = line.strip_prefix("branch ") {
            if let Some(p) = &current {
                let short = b.trim().strip_prefix("refs/heads/").unwrap_or(b.trim());
                map.insert(p.clone(), short.to_string());
            }
        }
    }
    map
}

/// The indented session-detail block shared by the live + stale renderings of
/// `aida status <spec>`.
// trace:STORY-694 | ai:claude
pub(crate) fn print_lease_detail(l: &SessionLease, elapsed_secs: u64) {
    println!("  {:<12} {}", "session:".dimmed(), l.id);
    println!(
        "  {:<12} {}",
        "role:".dimmed(),
        l.role.as_deref().unwrap_or("-")
    );
    println!(
        "  {:<12} {}",
        "worktree:".dimmed(),
        l.worktree_path.display()
    );
    println!(
        "  {:<12} {}",
        "started:".dimmed(),
        l.started_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
    );
    println!(
        "  {:<12} {}",
        "elapsed:".dimmed(),
        humanize_duration_secs(elapsed_secs)
    );
}

/// The shortest unambiguous prefix of a lease id, for the `aida session end
/// <id>` hint. Reuses the same disambiguation helper the `aida session leases`
/// table uses.
// trace:STORY-694 | ai:claude
pub(crate) fn short_lease_id(l: &SessionLease, all: &[SessionLease]) -> String {
    let ids: Vec<&str> = all.iter().map(|x| x.id.as_str()).collect();
    let n = unique_prefix_len(&l.id, &ids, 8);
    l.id[..n.min(l.id.len())].to_string()
}

/// One row of the `aida ps` running-work table: a classified session lease plus
/// the spec it is linked to (if any). Built off the SAME machinery the per-spec
/// `aida status <spec>` and `aida session leases` use ([`list_leases`],
/// [`lease_state_for`], the pid probe), so the global view and the per-spec view
/// can never disagree about what is live.
// trace:STORY-696 | ai:claude
pub(crate) struct PsRow {
    pub(crate) lease: SessionLease,
    pub(crate) state: LeaseState,
    /// Display role for `aida ps`: the live transcript role wins when present
    /// because it names the session's current behavior; the raw lease role is
    /// retained separately for JSON provenance.
    // trace:TASK-152 | ai:codex
    pub(crate) role: Option<String>,
    /// Raw role recorded on the lease before any live-transcript override.
    // trace:TASK-152 | ai:codex
    pub(crate) lease_role: Option<String>,
    /// PID of the live claude inside the worktree (or the review-verb creator
    /// pid). `None` when no live process backs the lease.
    pub(crate) pid: Option<u32>,
    /// BUG-763: start time of that pid, when the probe can say. Lets the
    /// renderer surface an adopted persistent lease (pid younger than the
    /// lease) instead of silently mixing lease-age and process-age.
    pub(crate) pid_started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// BUG-769: the WORK-SESSION age behind the `elapsed` column — the backing
    /// process's uptime on an adopted lease (pid younger than the lease
    /// record), the lease's own age on every other row. See
    /// [`ps_elapsed_secs`]; lease-age still shows in the adopted annotation.
    // trace:BUG-769 | ai:claude
    pub(crate) elapsed_secs: u64,
    /// The display id of the spec this lease is scoped to, when the lease scope
    /// resolves to a known spec id (the AIDA-launched happy path). `None` for
    /// generic `harness-worktree` advisor fan-out leases — surfaced honestly as
    /// "scope unknown" rather than guessed.
    // trace:STORY-696
    pub(crate) spec: Option<String>,
    /// TASK-1090: the dispatch-health classification for this row (MOVING /
    /// STALLED / SALVAGEABLE) plus the exact next command to unstick it.
    /// `None` for worktree-less advisory leases (`aida review` / `aida claim`
    /// locks) or a lease whose worktree no longer exists — there is no git
    /// state to classify.
    // trace:TASK-1090 | ai:claude
    pub(crate) dispatch: Option<PsDispatch>,
    /// TASK-1143: the advisor holding the STORY-711 worktree lock on this row's
    /// worktree, if any — the `authorized_by` value on the covering session
    /// lease, read via `worktree_lock::read_authorized_by` (the same source the
    /// `aida lock` CLI writes and `locking_gate` reads). `None` when the
    /// worktree carries no lock, which is the common case since `[locking]` is
    /// opt-in — so the locked-by column stays blank until a lock exists.
    // trace:TASK-1143 | ai:claude
    pub(crate) locked_by: Option<String>,
    /// TASK-1451: the live seat's resolved mail-sender identity source —
    /// `None` when there is no live pid backing this row (nothing to probe).
    // trace:TASK-1451 | ai:claude
    pub(crate) mail_identity: Option<MailIdentityStatus>,
    /// BUG-1553: is this LIVE seat actively working, blocked on a human
    /// approval gate, or in a state the probe cannot resolve? `None` when
    /// the row has no live pid at all (Dormant/Stale rows — already
    /// unambiguous; nothing to inspect).
    // trace:BUG-1553 | ai:claude
    pub(crate) activity: Option<SeatActivity>,
}

/// The TASK-1090 dispatch-health payload for one [`PsRow`].
// trace:TASK-1090 | ai:claude
pub(crate) struct PsDispatch {
    pub(crate) state: dispatch_health_ps::DispatchState,
    /// `None` only for `Moving` — nothing to unstick.
    pub(crate) hint: Option<String>,
    pub(crate) dirty: bool,
    pub(crate) ahead_of_main: u32,
    pub(crate) untracked_only: bool,
}

/// TASK-1451: whether a live seat's resolved mail identity is a stable seat
/// value or the ambiguous shell-user fallback BUG-1533 flagged in the
/// envelope — surfaced here BEFORE that seat sends any mail, so the gap is
/// visible up front instead of discovered 200 messages later.
// trace:TASK-1451 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MailIdentityStatus {
    /// `AIDA_AGENT_NAME` / `AIDA_USER` / `AIDA_SESSION_ROLE` resolves a
    /// stable seat identity — mail sent from this process is attributable.
    Attributed,
    /// None of those three are set in the process environment — mail sent
    /// from this seat would collapse to the shell-user fallback.
    Unattributed,
    /// The process environment could not be read — another user's process,
    /// the process already exited, or a non-Linux host. Reported as
    /// "identity unknown" (PRIN-5) — never silently treated as fine.
    Unknown,
}

impl MailIdentityStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            MailIdentityStatus::Attributed => "attributed",
            MailIdentityStatus::Unattributed => "unattributed",
            MailIdentityStatus::Unknown => "unknown",
        }
    }
}

/// Pure classification given an already-read environment snapshot — the
/// TASK-1451 test seam. Reuses the BUG-1533 `resolve_sender` precedence
/// wholesale rather than a second, drifting copy of it. `explicit` has no
/// meaning here (`aida ps` is not sending a message), so it is always
/// absent.
// trace:TASK-1451 | ai:claude
pub(crate) fn mail_identity_status_from_env(
    env: &std::collections::HashMap<String, String>,
) -> MailIdentityStatus {
    let get = |k: &str| env.get(k).map(String::as_str);
    let (_, source) = aida_core::mailbox::resolve_sender(
        None,
        get("AIDA_AGENT_NAME"),
        get("AIDA_USER"),
        get("AIDA_SESSION_ROLE"),
        get("USER"),
    );
    if source.is_attributed() {
        MailIdentityStatus::Attributed
    } else {
        MailIdentityStatus::Unattributed
    }
}

/// Parse `/proc/<pid>/environ` (NUL-separated `KEY=VALUE` records) into a
/// map. `None` when the file can't be read — permission denied (another
/// user's process) or the process has already exited. Linux-only: that file
/// has no equivalent on other platforms.
// trace:TASK-1451 | ai:claude
#[cfg(target_os = "linux")]
pub(crate) fn read_pid_environ_vars(pid: u32) -> Option<std::collections::HashMap<String, String>> {
    let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let mut map = std::collections::HashMap::new();
    for entry in raw.split(|&b| b == 0) {
        if entry.is_empty() {
            continue;
        }
        if let Ok(s) = std::str::from_utf8(entry) {
            if let Some((k, v)) = s.split_once('=') {
                map.insert(k.to_string(), v.to_string());
            }
        }
    }
    Some(map)
}

/// The real `/proc/<pid>/environ` probe — the same "one seam per real I/O
/// source" pattern as `pid_start_time`, injected into `build_running_work`
/// so the row-building logic stays filesystem-free and unit-testable with
/// an injected environment map instead of a real `/proc` read.
// trace:TASK-1451 | ai:claude
#[cfg(target_os = "linux")]
pub(crate) fn probe_mail_identity(pid: u32) -> MailIdentityStatus {
    match read_pid_environ_vars(pid) {
        Some(env) => mail_identity_status_from_env(&env),
        None => MailIdentityStatus::Unknown,
    }
}

/// Non-Linux hosts have no `/proc/<pid>/environ` to read — always "unknown",
/// never guessed as fine.
// trace:TASK-1451 | ai:claude
#[cfg(not(target_os = "linux"))]
pub(crate) fn probe_mail_identity(_pid: u32) -> MailIdentityStatus {
    MailIdentityStatus::Unknown
}

/// BUG-1553: is a LIVE seat (a lease whose pid exists) actively working, on
/// a long-running tool call, suspended (a stopped process), or in a state
/// this probe cannot resolve? `LeaseState::Live` alone only answers "does a
/// process exist" — the 2026-09-21 incident (see BUG-1553) showed that
/// answer collapses several different situations into one row. PRIN-5:
/// `Working` is returned only when the transcript positively supports it —
/// anything the probe can't read renders `Unknown`, never silently
/// `Working`. **None of these states asserts "blocked on your approval"** —
/// per the 2026-09-23 strict-review PROXY DECISION, a purely time-based or
/// process-state heuristic cannot distinguish a long-running tool call (or a
/// deliberately paused process) from a genuine unanswered permission prompt,
/// so both render neutral/warning, never red, and never claim to know a
/// human is needed. A true "blocked on approval" verdict needs a recorded
/// marker from Claude Code's Notification hook (the `permission_prompt`
/// type) and is a follow-up task.
// trace:BUG-1553 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeatActivity {
    /// Recent transcript activity, or the last turn completed cleanly (no
    /// tool call left unresolved past the stall threshold).
    Working,
    /// The transcript's last assistant turn called a tool that has had no
    /// resolving result for longer than [`LONG_TOOL_CALL_THRESHOLD_SECS`].
    /// This is NOT evidence of a blocked approval prompt — a long Bash or
    /// Task call is working, not blocked — so it renders as a neutral
    /// informational note. `tool` names the pending tool when the
    /// transcript names one; `secs` is how long it has been outstanding.
    LongToolCall { tool: Option<String>, secs: i64 },
    /// The process itself is job-control-stopped (`T`/`t` state in
    /// `/proc/<pid>/stat`) — e.g. Ctrl-Z'd or paused under a debugger.
    /// Neutral-to-warning, not "blocked on approval".
    Suspended,
    /// No session transcript could be resolved/read for this pid at all —
    /// honestly "don't know", never guessed as Working.
    Unknown,
    /// TASK-1454: a REAL `Notification(permission_prompt)` hook marker is
    /// present (and not stale) for this seat's Claude Code session — ground
    /// truth from Claude Code itself, not an inference from process/
    /// transcript state. Unlike `LongToolCall`/`Suspended`, this DOES assert
    /// "blocked on your approval", because it is sourced from the exact
    /// signal the 2026-09-23 PROXY DECISION said would justify that claim.
    /// `tool` names the pending tool when the notification message named
    /// one; `secs` is how long the marker has stood.
    // trace:TASK-1454 | ai:claude
    Blocked { tool: Option<String>, secs: i64 },
}

impl SeatActivity {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            SeatActivity::Working => "working",
            SeatActivity::LongToolCall { .. } => "long_tool_call",
            SeatActivity::Suspended => "suspended",
            SeatActivity::Unknown => "unknown",
            SeatActivity::Blocked { .. } => "blocked",
        }
    }
}

/// BUG-1553: how long a pending tool call (an assistant turn's tool_use with
/// no resolving tool_result yet, per the transcript tail) must sit unresolved
/// before this reads as a [`SeatActivity::LongToolCall`] rather than "still
/// executing". Wide enough that an ordinary slow tool call rarely trips it on
/// its own; the row frames the verdict as an informational note ("long tool
/// call"), never an alarm, because a handful of legitimately long-running
/// tools (a full test suite) routinely cross it.
// trace:BUG-1553 | ai:claude
pub(crate) const LONG_TOOL_CALL_THRESHOLD_SECS: i64 = 240;

/// BUG-1553: pure scan of a session transcript's TAIL lines (most-recent
/// last, as a JSONL tail naturally reads) for a pending tool call — the last
/// assistant-turn tool_use block(s) with no later matching tool_result.
/// Returns the pending tool's name (when the transcript names one) and the
/// age of that assistant message in seconds. `None` means the tail parsed
/// cleanly and nothing is pending (the last turn resolved, or no tool was
/// called) — distinct from "the tail could not be read/parsed at all",
/// which this function cannot express (see `classify_seat_activity`, which
/// takes `tail: Option<&[String]>` precisely to keep that distinction).
// trace:BUG-1553 | ai:claude
pub(crate) fn pending_tool_from_tail(
    tail_lines: &[String],
    now: chrono::DateTime<chrono::Utc>,
) -> Option<(Option<String>, i64)> {
    // (tool name, tool_use id, assistant message timestamp)
    let mut pending: Option<(Option<String>, String, chrono::DateTime<chrono::Utc>)> = None;
    for line in tail_lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let content = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array());
        let Some(content) = content else { continue };
        if msg_type == "assistant" {
            let mut turn_pending: Option<(Option<String>, String)> = None;
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    let id = block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string());
                    turn_pending = Some((name, id));
                }
            }
            if let Some((name, id)) = turn_pending {
                let ts = v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|t| t.with_timezone(&chrono::Utc));
                if let Some(ts) = ts {
                    pending = Some((name, id, ts));
                }
            }
        } else if msg_type == "user" {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                    let result_id = block.get("tool_use_id").and_then(|i| i.as_str());
                    match (result_id, pending.as_ref()) {
                        // Matches the outstanding call by id — resolved.
                        (Some(rid), Some((_, pid, _))) if rid == pid => pending = None,
                        // No id on either side to compare — conservatively
                        // treat any tool_result as resolving the outstanding
                        // call, since we can't match ids.
                        // trace:BUG-1553 | ai:claude
                        (None, _) => pending = None,
                        _ => {}
                    }
                }
            }
        }
    }
    let (name, _id, ts) = pending?;
    let age = now.signed_duration_since(ts).num_seconds().max(0);
    Some((name, age))
}

/// BUG-1553: the pure verdict — given (optionally) a transcript tail and
/// whether the process is job-control-stopped, classify seat activity.
/// `tail: None` means "could not be read at all" (no jsonl resolved, or the
/// read failed) → `Unknown`, never guessed as `Working`. Per the
/// 2026-09-23 strict-review PROXY DECISION, neither branch below asserts
/// "blocked on approval" — a job-control-stopped process reads `Suspended`
/// and a long-outstanding tool call reads `LongToolCall`, both neutral.
// trace:BUG-1553 | ai:claude
pub(crate) fn classify_seat_activity(
    tail: Option<&[String]>,
    proc_stopped: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> SeatActivity {
    if proc_stopped {
        return SeatActivity::Suspended;
    }
    let Some(lines) = tail else {
        return SeatActivity::Unknown;
    };
    match pending_tool_from_tail(lines, now) {
        Some((name, age)) if age >= LONG_TOOL_CALL_THRESHOLD_SECS => SeatActivity::LongToolCall {
            tool: name,
            secs: age,
        },
        _ => SeatActivity::Working,
    }
}

/// TASK-1454: the marker-first wrapper around [`classify_seat_activity`]. A
/// present (already-filtered-non-stale) pending-approval marker is ground
/// truth and ALWAYS wins outright over the transcript/proc-state heuristic —
/// pure and separated out specifically so that priority is unit-testable
/// without a lease/session-manifest fixture.
// trace:TASK-1454 | ai:claude
pub(crate) fn seat_activity_with_marker(
    marker: Option<&pending_approval::PendingApprovalMarker>,
    tail: Option<&[String]>,
    proc_stopped: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> SeatActivity {
    if let Some(marker) = marker {
        let secs = now.signed_duration_since(marker.since).num_seconds().max(0);
        return SeatActivity::Blocked {
            tool: marker.tool.clone(),
            secs,
        };
    }
    classify_seat_activity(tail, proc_stopped, now)
}

/// BUG-1553: read only the TAIL of a transcript file — never the whole
/// file — capped at `max_bytes` from the end. Keeps `aida ps` fast even
/// against a long-running session's multi-MB transcript, per the surface's
/// own speed constraint (STORY-707's "cache-fast, no full scan" discipline
/// applied to this probe too). Returns whole lines only (a partial first
/// line from the seek point is dropped). `None` when the file can't be
/// opened/read at all. Reads to the end of the file and decodes lossily
/// (`from_utf8_lossy`) rather than `read_to_string`, because the seek point
/// (`max_bytes` from the end) can land mid multi-byte UTF-8 character —
/// `read_to_string` would hard-error on that instead of tolerating it.
// trace:BUG-1553 | ai:claude
pub(crate) fn read_transcript_tail(path: &std::path::Path, max_bytes: u64) -> Option<Vec<String>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).ok()?;
    let buf = String::from_utf8_lossy(&raw);
    let mut lines: Vec<String> = buf.lines().map(|l| l.to_string()).collect();
    // Drop a partial first line when we didn't start at byte 0.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    Some(lines)
}

/// BUG-1553: bytes read from the tail of a transcript — generous enough to
/// span several recent turns (a pending tool call plus the assistant text
/// leading up to it) without ever reading a whole multi-MB file.
// trace:BUG-1553 | ai:claude
pub(crate) const TRANSCRIPT_TAIL_BYTES: u64 = 64 * 1024;

/// BUG-1553: is `pid` currently job-control-stopped (`T`/`t` in
/// `/proc/<pid>/stat` field 3)? A process parked at a blocking read is
/// normally `S` (sleeping) — the SAME state as idle-between-turns — so this
/// is a narrow, unambiguous supplementary signal, not the primary one (the
/// transcript tail is); it costs one small file read. Non-Linux hosts have
/// no `/proc` to read — always `false`, folding into the transcript-only
/// verdict rather than a platform-specific guess.
// trace:BUG-1553 | ai:claude
#[cfg(target_os = "linux")]
pub(crate) fn proc_is_stopped(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // Field 3 (process state) follows the `(comm)` field, which may itself
    // contain spaces/parens — split on the LAST ')' to find it reliably.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .is_some_and(|state| state == "T" || state == "t")
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn proc_is_stopped(_pid: u32) -> bool {
    false
}

/// An In-Progress spec with NO live spec-scoped session backing it — the
/// flag-only / orphaned case. Surfaced by `aida ps` alongside the live table so
/// a crashed or never-started session can't hide behind a status flag.
// trace:STORY-696 | ai:claude
#[derive(Debug)]
pub(crate) struct PsOrphan {
    pub(crate) spec: String,
    pub(crate) title: String,
    /// `true` when a spec-scoped lease EXISTS but its holder process is dead
    /// (STALE) — distinct from no lease at all (pure flag-only). Either way the
    /// In-Progress flag is not liveness-backed.
    pub(crate) stale_lease: bool,
    /// TASK-1064: `true` when this flag-only spec (no spec-scoped lease) is most
    /// likely being built by a live advisor Agent-tool fan-out — a fan-out takes
    /// a generic non-spec-linked `harness-worktree` lease, so its specs read as
    /// having no spec lease. Surfaced informationally instead of under the
    /// alarming "Orphaned" header. Never set for a stale (crashed) spec-scoped
    /// lease — that is a genuine orphan.
    // trace:TASK-1064 | ai:claude
    pub(crate) likely_fanout: bool,
    /// BUG-1656: `true` when this spec-scoped lease is STALE (its recorded pid
    /// is dead) but a LIVE generic `harness-worktree` lease exists in this repo
    /// — the shape an Agent-tool subagent dispatched into the spec's leased
    /// worktree leaves behind. The spec may well be being worked right now;
    /// the surface says "possibly worked by a subagent" instead of a flat
    /// "abandoned".
    // trace:BUG-1656 | ai:claude
    pub(crate) possibly_subagent: bool,
}

/// The flag-only / orphaned verdict for one In-Progress spec, given the
/// classified state of its spec-scoped lease (if any). A `Live` lease means the
/// spec is genuinely running (NOT orphaned → `None`). A dead/dormant lease →
/// orphaned with `stale_lease = true` (crashed session). No lease → orphaned
/// flag-only. Pure so the orphan matrix is unit-testable without a store or a
/// lease dir.
///
/// BUG-778: `awaiting_agent` short-circuits to "not orphaned". A spec whose
/// worktree a human just entered by hand has no live process backing it *yet*
/// — that is the launch-lag, not an orphan, and listing it under "no live
/// session backing the flag" pointed the operator at a re-dispatch that would
/// race them. The row still shows in the running-work table, framed honestly.
// trace:STORY-696 | ai:claude
// trace:BUG-778 | ai:claude
pub(crate) fn ps_orphan_verdict(
    lease_state: Option<LeaseState>,
    awaiting_agent: bool,
) -> Option<bool> {
    if awaiting_agent {
        return None;
    }
    match lease_state {
        Some(LeaseState::Live) => None,
        Some(_) => Some(true),
        None => Some(false),
    }
}

/// BUG-1656: [`ps_orphan_verdict`] plus the dirty-movement liveness signal. A
/// spec-scoped lease whose worktree's dirty files are still being written is
/// NOT orphaned — something is editing there — however dead its recorded pid
/// looks (an Agent-tool subagent runs inside the parent claude process and
/// is invisible to the pid/cwd probes). Pure so the matrix stays testable.
// trace:BUG-1656 | ai:claude
pub(crate) fn ps_orphan_verdict_with_movement(
    lease_state: Option<LeaseState>,
    awaiting_agent: bool,
    dirty_movement_fresh: bool,
) -> Option<bool> {
    if lease_state.is_some() && dirty_movement_fresh {
        return None;
    }
    ps_orphan_verdict(lease_state, awaiting_agent)
}

/// BUG-1740: is this `harness-worktree` lease pinned to the project ROOT
/// itself — the main checkout — rather than to a worktree of its own?
///
/// THE RULE, and why (AC1). A root-pinned harness lease is **never**, on its
/// own, evidence that an Agent-tool fan-out is running. The project root is
/// where the operator's own interactive session sits and where every `aida`
/// invocation runs, so a lease there is the ambient session, not a dispatched
/// subagent. Worse, `lease_state_for`'s `has_live_claude` arm matches on
/// `cwd.starts_with(worktree_path)`, so a root-pinned lease absorbs the
/// liveness of every claude process anywhere under the checkout — including
/// the one running this command. A genuine fan-out does not need this lease:
/// the harness gives each Agent-tool subagent its own worktree
/// (`.claude/worktrees/agent-*`), and the subagent takes its own
/// `harness-worktree` lease pinned there, which this filter keeps.
///
/// Decided from the lease alone — `worktree_path == parent_project_root` —
/// so the predicate stays pure and needs no ambient project-root lookup.
// trace:BUG-1740 | ai:claude
pub(crate) fn ps_lease_is_project_root_pinned(l: &SessionLease) -> bool {
    !l.worktree_path.as_os_str().is_empty()
        && l.parent_project_root.as_deref() == Some(l.worktree_path.as_path())
}

/// BUG-1740: the pid chain of the process running this command, innermost
/// first and including itself. The reporting session must never be its own
/// corroboration for a fan-out (AC2), and the session that invoked `aida` is
/// an ANCESTOR of this process, not this process — so the whole chain is
/// excluded, not just `std::process::id()`.
// trace:BUG-1740 | ai:claude
pub(crate) fn ps_caller_pid_chain() -> Vec<u32> {
    process_probe::walk_ancestor_pids(std::process::id())
}

/// BUG-1740 (rework): does the IDENTITY evidence this lease would present to
/// `lease_state_for` — the pid arm that short-circuits `Live` before the
/// presence list is consulted — name the caller or one of its ancestors?
///
/// Mirrors `lease_state_for`'s arm order exactly, so the pid examined here is
/// the pid that would have decided liveness there:
///
///  - the advisory-lock arm (`review_verb`/`claim_verb`, no worktree of its
///    own) reads `creator_pid`;
///  - otherwise an explicit `active_pid` wins.
///
/// The mirroring matters for AC4: a lease the caller MINTED (`creator_pid` in
/// the caller's chain) but whose liveness is attested by a distinct worker's
/// `active_pid` is a genuine fan-out, and an unconditional creator-pid check
/// would delete it. Only the pid `lease_state_for` would actually consult is
/// compared against the caller's chain.
///
/// This is deliberately NOT inside `lease_state_for`: the exclusion is about
/// fan-out CORROBORATION (the observer must not testify for itself), not
/// about lease liveness generally, and every other caller of
/// `lease_state_for` keeps its behavior.
// trace:BUG-1740 | ai:claude
pub(crate) fn ps_lease_identity_is_caller(l: &SessionLease, caller_pids: &[u32]) -> bool {
    if (l.review_verb || l.claim_verb) && l.worktree_path.as_os_str().is_empty() {
        return l.creator_pid.is_some_and(|pid| caller_pids.contains(&pid));
    }
    l.active_pid.is_some_and(|pid| caller_pids.contains(&pid))
}

/// TASK-1064: is an advisor Agent-tool fan-out currently running? Detected as a
/// LIVE lease whose scope is the generic `harness-worktree` fallback — the lease
/// an Agent-tool subagent (a `general-purpose` fan-out whose branch carries no
/// SPEC-ID) takes. Such a lease is deliberately NOT spec-linked, so the specs it
/// is building read as having no spec-scoped lease. Its liveness means those
/// flag-only In-Progress specs are most likely being worked by the fan-out, not
/// genuinely orphaned. Pure over the lease set + the shared liveness machinery.
///
/// BUG-1740: two leases that used to qualify no longer do.
///
///  1. A lease pinned to the project root — see [`ps_lease_is_project_root_pinned`].
///  2. A lease whose only live backing is rooted in the caller's own pid
///     chain. THE FULL RULE (AC2): neither presence (cwd) nor identity
///     (`active_pid`/`creator_pid`) evidence rooted in the caller's own pid
///     chain corroborates a fan-out. The reporting session must never be its
///     own corroboration, whichever door the evidence comes through:
///
///     - PRESENCE: the `has_live_claude` arm of `lease_state_for` infers a
///       worker from a claude process whose cwd is inside the lease's
///       worktree — and the reporting session is such a process. So
///       `caller_pids` is subtracted from the live set before liveness is
///       classified.
///     - IDENTITY: `lease_state_for` short-circuits `Live` on a live
///       `active_pid` (and on `creator_pid` in its advisory-lock arm) BEFORE
///       the presence list is consulted. An Agent-tool subagent executes
///       INSIDE the parent claude process, so a harness lease whose
///       `active_pid` is the caller or one of its ancestors says only that
///       the long-lived session that minted it still exists — not that a
///       fan-out is running (BUG-1772's unreaped harness leases make exactly
///       this shape persistent). Such leases are excluded here via
///       [`ps_lease_identity_is_caller`], which mirrors `lease_state_for`'s
///       arm order.
///
/// WHY THIS DOES NOT DELETE TASK-1064's CASE (AC4). AC4 protects a lease
/// "backed by a live process that is NOT the caller". A fan-out observed from
/// any OTHER session (operator window, monitor seat) still corroborates: the
/// orchestrator's pid is not in that caller's ancestor chain. Only the
/// orchestrating session's own self-report loses the softening — the price
/// AC2 explicitly accepts ("`possibly_subagent` must be false when the only
/// live process backing the harness lease is the process running the command
/// (or its ancestors)").
///
/// The exclusion lives HERE, in the fan-out corroboration path, and not in
/// `lease_state_for`: liveness for every other caller (`aida ps` display,
/// drain, the orphan pass's lease display) is unchanged.
///
/// Still pure: `caller_pids` is an input, so every corner is fixture-testable.
// trace:TASK-1064 | ai:claude
// trace:BUG-1740 | ai:claude
pub(crate) fn ps_live_fanout_leases<'a>(
    leases: &'a [SessionLease],
    live: &[process_probe::LiveSession],
    now: chrono::DateTime<chrono::Utc>,
    caller_pids: &[u32],
) -> Vec<&'a SessionLease> {
    // The observer removed from the evidence, once for the whole pass.
    // trace:BUG-1740 | ai:claude
    let corroborating: Vec<process_probe::LiveSession> = live
        .iter()
        .filter(|s| !caller_pids.contains(&s.pid))
        .cloned()
        .collect();
    leases
        .iter()
        .filter(|l| {
            l.scope
                .eq_ignore_ascii_case(worktree_lease::HARNESS_WORKTREE_SCOPE)
                // trace:BUG-1740 | ai:claude
                && !ps_lease_is_project_root_pinned(l)
                // Identity evidence rooted in the caller's own pid chain is
                // the observer testifying for itself — see the doc comment.
                // trace:BUG-1740 | ai:claude
                && !ps_lease_identity_is_caller(l, caller_pids)
                && matches!(lease_state_for(l, &corroborating, now), LeaseState::Live)
        })
        .collect()
}

/// BUG-1681: does `text` — a branch name or a worktree path — NAME one of
/// `ids`? Case-insensitive containment with id boundaries on both sides, so
/// `claude/bug-168` never matches `BUG-1681` and `wt-bug-1681` does. Pure, so
/// the matching rule is fixture-testable.
// trace:BUG-1681 | ai:claude
pub(crate) fn ps_text_names_spec(text: &str, ids: &[&str]) -> bool {
    let hay = text.to_ascii_lowercase();
    ids.iter().any(|id| {
        let needle = id.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return false;
        }
        hay.match_indices(&needle).any(|(at, _)| {
            let before = hay[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric());
            let after = hay[at + needle.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric());
            before && after
        })
    })
}

/// BUG-1681: is this LIVE fan-out lease plausibly working the spec `ids` name?
/// True only when the subagent's OWN branch or worktree names that spec. The
/// previous rule was "any fan-out is alive anywhere in this repo", which
/// credited a live subagent with every flag-only In-Progress spec in the store
/// — including specs no subagent had ever touched.
// trace:BUG-1681 | ai:claude
pub(crate) fn ps_fanout_names_spec(fanout: &SessionLease, ids: &[&str]) -> bool {
    ps_text_names_spec(&fanout.branch, ids)
        || ps_text_names_spec(&fanout.worktree_path.to_string_lossy(), ids)
}

/// BUG-1681: is this LIVE fan-out lease working inside `spec_lease`'s
/// worktree? That is the BUG-1656 shape — an Agent-tool subagent dispatched
/// INTO a spec's leased worktree, whose work the spec lease's own pid probe
/// structurally cannot see — and it is the only case where a dead spec lease
/// may be explained by a subagent. Matched on the worktree the subagent
/// recorded, or on the branch it is on; never on mere coexistence in the repo.
// trace:BUG-1681 | ai:claude
pub(crate) fn ps_fanout_holds_lease(fanout: &SessionLease, spec_lease: &SessionLease) -> bool {
    let same_worktree = !spec_lease.worktree_path.as_os_str().is_empty()
        && fanout.worktree_path == spec_lease.worktree_path;
    let same_branch = !spec_lease.branch.trim().is_empty()
        && fanout
            .branch
            .trim()
            .eq_ignore_ascii_case(spec_lease.branch.trim());
    same_worktree || same_branch
}

/// BUG-1681: the generic role label a fan-out row shows when the harness
/// recorded only its placeholder agent type. It says what the row IS — a
/// subagent — instead of borrowing the identity of the session hosting it.
// trace:BUG-1681 | ai:claude
pub(crate) const PS_SUBAGENT_ROLE: &str = "subagent";

/// The role an `aida ps` row displays, from the three signals available.
///
/// BUG-1521: a REAL recorded lease role is authoritative — the row shows what
/// the session recorded about itself, not what a heuristic inferred.
///
/// BUG-1681: when the recorded role is the harness's generic Agent-tool
/// placeholder, the row is an Agent-tool subagent, and its lease's pid is the
/// PARENT claude process (a subagent executes inside it — see BUG-752). The
/// transcript scan therefore resolves the HOST session's role, which is how
/// fan-out rows came to display `advisor`: the parent's identity, not their
/// own. So the placeholder falls back to the manifest join (TASK-153's stable
/// per-lease `claude_session_id` link, which is about THIS row) and then to the
/// generic subagent label — never to the ambiguous host-transcript scan.
///
/// A lease with no recorded role at all keeps the pre-existing derivation: the
/// manifest join, else the transcript scan.
// trace:BUG-1521 trace:TASK-153 trace:BUG-1681 | ai:claude
pub(crate) fn ps_display_role(
    lease_role: Option<&str>,
    manifest_role: Option<&str>,
    jsonl_role: Option<&str>,
) -> Option<String> {
    match lease_role.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) if !r.eq_ignore_ascii_case(tail_cmd::HARNESS_AGENT_TYPE) => Some(r.to_string()),
        Some(_) => Some(
            manifest_role
                .map(str::to_string)
                .unwrap_or_else(|| PS_SUBAGENT_ROLE.to_string()),
        ),
        None => manifest_role.or(jsonl_role).map(str::to_string),
    }
}

/// BUG-1681: re-read one row's dispatch verdict in the light of its
/// integration standing, replacing the stalled/stopped reading (and its
/// resume hint) with the actionless `awaiting-integration` note. Returns
/// whether the row changed. Pure over the row + the standing, so the
/// reframing is fixture-testable without a store or a forge.
// trace:BUG-1681 | ai:claude
pub(crate) fn ps_apply_integration_standing(
    row: &mut PsRow,
    standing: Option<&dispatch_health_ps::IntegrationStanding>,
) -> bool {
    let Some(dispatch) = row.dispatch.as_mut() else {
        return false;
    };
    let next = dispatch_health_ps::apply_integration(dispatch.state, standing);
    if next == dispatch.state {
        return false;
    }
    dispatch.state = next;
    dispatch.hint = Some(dispatch_health_ps::integration_hint(standing));
    true
}

/// BUG-1681: ask the substrate whether a stalled-looking row's spec has in
/// fact FINISHED before `aida ps` advertises a resume for it. The observed
/// failure was ~20 rows whose specs were already Completed and landed in
/// integration batches, each advertised as `stalled — resume/rebrief`; an
/// agent that follows that hint redoes shipped work.
///
/// Cost-gated: nothing is read unless some row actually classified
/// stalled/stopped (the quiet case pays zero), and then only the cached
/// requirement lookup for those rows' specs plus their local review-verdict
/// files. No forge call.
// trace:BUG-1681 | ai:claude
pub(crate) fn ps_overlay_integration_standing(project_root: &std::path::Path, rows: &mut [PsRow]) {
    let candidates: Vec<String> = rows
        .iter()
        .filter(|r| {
            r.dispatch.as_ref().is_some_and(|d| {
                matches!(
                    d.state,
                    dispatch_health_ps::DispatchState::Stalled
                        | dispatch_health_ps::DispatchState::Stopped
                )
            })
        })
        .filter_map(|r| r.spec.clone())
        .collect();
    if candidates.is_empty() {
        return;
    }
    let finished = session_reap::finished_scopes(project_root, &candidates);
    for row in rows.iter_mut() {
        let Some(spec) = row.spec.clone() else {
            continue;
        };
        if !candidates.iter().any(|c| c.eq_ignore_ascii_case(&spec)) {
            continue;
        }
        let branch = row.lease.branch.clone();
        let verdict = review_verdict::read_recorded_verdict_any(project_root, &[spec.as_str()]);
        let standing = dispatch_health_ps::integration_standing(
            finished.contains(&spec.to_ascii_uppercase()),
            verdict.as_ref().is_some_and(|v| v.kind.approves()),
            verdict.as_ref().and_then(|v| v.reviewed_branch.as_deref()),
            &branch,
            verdict.as_ref().and_then(|v| v.comment_url.as_deref()),
        );
        ps_apply_integration_standing(row, standing.as_ref());
    }
}

/// TASK-1064: should a flag-only In-Progress spec (no spec-scoped lease) be
/// surfaced as "likely worked by a fan-out" rather than "orphaned"? True only
/// when there is NO spec-scoped lease at all (`stale_lease == false` — a crashed
/// spec-scoped lease is a genuine orphan regardless) AND a live fan-out harness
/// lease exists (a plausible live worker). Pure so the three-way framing is
/// unit-testable without a store or a lease dir.
// trace:TASK-1064 | ai:claude
pub(crate) fn ps_orphan_likely_fanout(stale_lease: bool, fanout_names_spec: bool) -> bool {
    !stale_lease && fanout_names_spec
}

/// BUG-752: does this lease carry NO process-liveness signal at all while
/// being one of the harness (Agent-tool) isolation worktrees? Such a worker
/// runs inside the parent claude process (cwd = parent project root), so the
/// cwd-based worktree probe structurally cannot see it — "no live claude in
/// the worktree" is NOT evidence of death there. True only for the legacy /
/// degraded case: a harness-worktree lease minted without an `active_pid`
/// stamp (pre-fix binary, or no claude ancestor found at register time).
/// Pure so the predicate is unit-testable on lease fixtures.
// trace:BUG-752 | ai:claude
pub(crate) fn harness_lease_without_pid_signal(l: &SessionLease) -> bool {
    l.active_pid.is_none()
        && l.creator_pid.is_none()
        && is_agent_managed_worktree(&l.worktree_path, Some(&l.branch))
}

/// BUG-752: the tri-state pid-liveness input to the dispatch-health
/// classifier. `Some(true)` for a Live lease (a real process demonstrably
/// backs it), `None` — liveness undeterminable — for a harness lease with no
/// pid signal (see [`harness_lease_without_pid_signal`]), `Some(false)`
/// otherwise (the probe looked and found nothing alive). Keeping this pure
/// keeps the "salvage hint only on genuinely determined death" rule
/// unit-testable without a lease dir or `/proc`.
// trace:BUG-752 | ai:claude
pub(crate) fn ps_pid_liveness(state: LeaseState, harness_without_pid_signal: bool) -> Option<bool> {
    match state {
        LeaseState::Live => Some(true),
        _ if harness_without_pid_signal => None,
        _ => Some(false),
    }
}

/// BUG-778: seconds since `aida worktree enter|add` handed this worktree to a
/// human, or `None` when it never did (every orchestrator-spawned lease). The
/// "was an agent ever expected here?" input to the dispatch-health classifier.
/// A clock-skewed future stamp clamps to 0 (freshly handed over) rather than
/// underflowing. Pure so the provenance read is unit-testable on fixtures.
// trace:BUG-778 | ai:claude
pub(crate) fn ps_manual_enter_secs(
    l: &SessionLease,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<u64> {
    l.manual_enter_at
        .map(|at| now.signed_duration_since(at).num_seconds().max(0) as u64)
}

/// BUG-778: is this lease's row the hand-entered, launch-pending shape? Read
/// straight off the row's already-computed dispatch state so the orphan
/// section and the running-work table can never disagree about the same lease
/// (the reported bug listed ONE hand-entered spec in both, under two
/// contradictory framings). Pure over the built rows.
// trace:BUG-778 | ai:claude
pub(crate) fn ps_row_awaiting_agent(rows: &[PsRow], lease_id: &str) -> bool {
    rows.iter().any(|r| {
        r.lease.id == lease_id
            && r.dispatch
                .as_ref()
                .is_some_and(|d| d.state == dispatch_health_ps::DispatchState::AwaitingAgent)
    })
}

/// TASK-1143: the `locked-by` cell for one `aida ps` row — the advisor holding
/// the STORY-711 worktree lock, or an empty string when the worktree carries no
/// lock. Blank (not `-`) for the unlocked common case so the column stays quiet
/// until a lock actually exists — `[locking]` is opt-in, so most rows have no
/// lock. An empty `authorized_by` (defensive: should never be written) is
/// treated the same as absent, mirroring `verify_worktree_lock`. Pure so the
/// locked/blank split is unit-testable without a lease dir.
// trace:TASK-1143 | ai:claude
pub(crate) fn ps_locked_by_cell(locked_by: Option<&str>) -> String {
    locked_by
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Floor / ceiling for the auto-sized `spec` column of the `aida ps` table. The
/// floor is the historical fixed width (so a table of ordinary SPEC-IDs looks
/// exactly as before); the ceiling bounds a pathological scope value so one long
/// row can't blow the table past a normal terminal width.
// trace:TASK-1168 | ai:claude
pub(crate) const PS_SPEC_MIN_WIDTH: usize = 14;
pub(crate) const PS_SPEC_MAX_WIDTH: usize = 20;

/// Floor / ceiling for the auto-sized `role` column. The ceiling is sized so the
/// whole closed set of role names — `implementer`, `advisor`, `reviewer`,
/// `general-purpose`, `harness-worktree` (16 chars, the longest) — renders whole.
// trace:TASK-1168 | ai:claude
pub(crate) const PS_ROLE_MIN_WIDTH: usize = 10;
pub(crate) const PS_ROLE_MAX_WIDTH: usize = 16;

/// Auto-size one `aida ps` column to its widest cell, clamped to `[min, max]`.
/// The previous fixed widths pre-truncated the `role` and `spec` cells to ~13
/// visible chars, so bounded closed-set identifiers (`harness-worktree`,
/// `general-purpose`) rendered ellipsized even though nothing else needed the
/// space. Sizing to content within a bound renders them whole while still
/// capping a pathological scope. Pure so the width math is unit-testable.
// trace:TASK-1168 | ai:claude
pub(crate) fn ps_column_width<S: AsRef<str>>(cells: &[S], min: usize, max: usize) -> usize {
    let widest = cells
        .iter()
        .map(|c| c.as_ref().chars().count())
        .max()
        .unwrap_or(0);
    widest.clamp(min, max.max(min))
}

/// Split an over-wide `aida ps` cell into `(cell, continuation)` — a WRAP, not an
/// ellipsis: every character survives, the remainder moves to an indented
/// continuation line alongside the existing per-row worktree / adopted-lease
/// lines. Prefers breaking after a separator (`-`, `_`, `/`, `.`, space) inside
/// the column so a hyphenated identifier wraps at a segment boundary rather than
/// mid-word; falls back to a hard split when the first `width` chars hold no
/// separator. Pure so the wrap points are unit-testable.
// trace:TASK-1168 | ai:claude
pub(crate) fn ps_wrap_cell(value: &str, width: usize) -> (String, Option<String>) {
    let chars: Vec<char> = value.chars().collect();
    if width == 0 || chars.len() <= width {
        return (value.to_string(), None);
    }
    // Break AFTER the last separator that still fits in the column, so the
    // delimiter stays with the head and the tail starts on a segment.
    let split = chars[..width]
        .iter()
        .rposition(|c| matches!(c, '-' | '_' | '/' | '.' | ' '))
        .map(|i| i + 1)
        .filter(|i| *i > 0 && *i < chars.len())
        .unwrap_or(width);
    let head: String = chars[..split].iter().collect();
    let tail: String = chars[split..].iter().collect();
    (head, Some(tail))
}

/// The `started` cell for one `aida ps` row — time-of-day only when the lease
/// started today (local time), date-qualified ("Jun-26 11:55") for anything
/// older, so a weeks-old persistent lease can't read as if it started this
/// morning next to a 500h elapsed. Pure over already-localized values so the
/// today/not-today split is unit-testable without freezing the clock or the
/// host timezone.
// trace:BUG-763 | ai:claude
pub(crate) fn ps_started_cell(
    started_local: chrono::NaiveDateTime,
    today_local: chrono::NaiveDate,
) -> String {
    if started_local.date() == today_local {
        started_local.format("%H:%M").to_string()
    } else {
        started_local.format("%b-%d %H:%M").to_string()
    }
}

/// Minimum a live pid must postdate its lease before the row is annotated as
/// adopted. Small enough to catch any real re-adoption, large enough to absorb
/// ordinary start-up ordering (a lease is written moments around the process
/// it records, in either order).
pub(crate) const PS_ADOPTED_SLACK_SECS: i64 = 300;

/// Is this row's lease ADOPTED — i.e. did the live pid backing it start
/// meaningfully (more than [`PS_ADOPTED_SLACK_SECS`]) after the lease record
/// itself was written? The single definition of adoption, shared by the
/// annotation ([`ps_adopted_note`]) and the elapsed column
/// ([`ps_elapsed_secs`]) so the two can never disagree about which rows are
/// adopted.
// trace:BUG-769 | ai:claude
pub(crate) fn ps_lease_is_adopted(
    lease_started: chrono::DateTime<chrono::Utc>,
    pid_started: chrono::DateTime<chrono::Utc>,
) -> bool {
    pid_started
        .signed_duration_since(lease_started)
        .num_seconds()
        >= PS_ADOPTED_SLACK_SECS
}

/// The `elapsed` cell for one `aida ps` row, in seconds — the age of the
/// WORK SESSION, not unconditionally the age of the lease record.
///
/// BUG-769: for an adopted lease (see [`ps_lease_is_adopted`]) this is the
/// backing process's uptime. A persistent harness lease is born once and then
/// re-adopted by every later session, so `now - lease.started_at` printed
/// "583h" next to a process that had been alive three minutes — the headline
/// number contradicted the BUG-763 annotation directly under it. Lease-age
/// keeps its place in that annotation, which still names BOTH ages; the column
/// now carries the actionable one ("how long has this actual worker been
/// going"). Every other row — no live pid, no resolvable start time, or a pid
/// born with its lease — is unchanged: `now - lease.started_at`.
///
/// Pure so both branches are unit-testable without a lease dir or `/proc`.
// trace:BUG-769 | ai:claude
pub(crate) fn ps_elapsed_secs(
    lease_started: chrono::DateTime<chrono::Utc>,
    pid_started: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> u64 {
    let anchor = match pid_started {
        Some(p) if ps_lease_is_adopted(lease_started, p) => p,
        _ => lease_started,
    };
    now.signed_duration_since(anchor).num_seconds().max(0) as u64
}

/// The adopted-lease annotation for one `aida ps` row — `Some` (naming BOTH
/// ages) when the live pid backing the row started meaningfully later than the
/// lease itself. That is the persistent harness-worktree pattern: the lease is
/// born once, then adopted and pid-restamped by every subsequent session, so
/// the row otherwise mixes provenance silently — pid/liveness from today's
/// process, started/elapsed from the lease record. Pure so the threshold and
/// wording are unit-testable.
// trace:BUG-763 | ai:claude
pub(crate) fn ps_adopted_note(
    lease_started: chrono::DateTime<chrono::Utc>,
    pid_started: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let pid_started = pid_started?;
    if !ps_lease_is_adopted(lease_started, pid_started) {
        return None;
    }
    let lease_age = now
        .signed_duration_since(lease_started)
        .num_seconds()
        .max(0) as u64;
    let pid_age = now.signed_duration_since(pid_started).num_seconds().max(0) as u64;
    Some(format!(
        "adopted — lease born {} ago, current pid up {}",
        humanize_duration_secs(lease_age),
        humanize_duration_secs(pid_age)
    ))
}

/// Best-effort start time of a single live pid, via a pid-scoped sysinfo
/// refresh (the BUG-613 single-pid discipline — never a full process-table
/// walk). `None` when the process is gone or the platform can't say.
// trace:BUG-763 | ai:claude
pub(crate) fn pid_start_time(pid: u32) -> Option<chrono::DateTime<chrono::Utc>> {
    use sysinfo::{Pid, ProcessRefreshKind, System};
    let target = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_pids_specifics(&[target], ProcessRefreshKind::new());
    let start = sys.process(target)?.start_time();
    if start == 0 {
        return None;
    }
    chrono::DateTime::<chrono::Utc>::from_timestamp(start as i64, 0)
}

// Rollup / stateless requirement types never carry a session of their own, so
// flagging them in the orphan pass is pure noise. Since BUG-626 made an epic's
// status a child-rollup, In-Progress epics are common — and would otherwise
// *all* read as "orphaned" (an epic never holds a spec-scoped lease). Folder
// and Meta are stateless containers/prompts that also never get worked
// directly. Skip these in the orphan walk so only real work-item In-Progress
// specs without a live session surface.
// trace:TASK-940 | ai:claude
pub(crate) fn ps_orphan_excluded_type(req_type: &aida_core::RequirementType) -> bool {
    matches!(
        req_type,
        aida_core::RequirementType::Epic
            | aida_core::RequirementType::Folder
            | aida_core::RequirementType::Meta
    )
}

/// `aida ps` — the GLOBAL running-work table. One row per active session/agent
/// across the project (the project-wide companion to `aida status <spec>`), plus
/// a pass over In-Progress specs with no live spec-scoped lease so orphaned
/// flags surface. Reuses the per-spec liveness machinery wholesale.
// trace:STORY-696 trace:STORY-694 | ai:claude
// trace:STORY-1024 | ai:codex
pub(crate) struct IntegrateCommandOpts {
    pub(crate) json: bool,
    pub(crate) run: bool,
    pub(crate) dry_run: bool,
    pub(crate) watch: bool,
    pub(crate) interval: u64,
    pub(crate) max: usize,
    pub(crate) wait_ci: bool,
    pub(crate) rebase: bool,
    pub(crate) strategy: Option<integrate::IntegrateStrategy>,
    pub(crate) focus: Option<String>,
    pub(crate) idle_minutes: Option<u64>,
    pub(crate) force: bool,
    pub(crate) user: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct MergeQueueRow {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) state: String,
    pub(crate) position: Option<usize>,
}

// trace:STORY-1024 | ai:codex
pub(crate) fn merge_queue_state_label(
    position: Option<usize>,
    ready: bool,
    has_open_pr: bool,
) -> String {
    match (position, ready, has_open_pr) {
        (Some(n), _, _) => format!("In merge queue (position {n})"),
        (None, true, true) => "In merge queue".to_string(),
        (None, false, true) => "Not ready for merge queue".to_string(),
        (None, _, false) => "Done without open PR".to_string(),
    }
}

/// `aida integrate` (bare) — the read-only integrator throughput view
/// (TASK-1034). One screen, scoped to the active focus: the focus-scoped queue,
/// the Done-with-PR merge queue, live throughput, and active fan-out. `--run`
/// hands off to the serialized merge engine.
// trace:TASK-1034 trace:STORY-718 trace:STORY-1024 | ai:claude codex
pub(crate) fn handle_integrate(opts: IntegrateCommandOpts) -> Result<()> {
    if opts.run || opts.watch || opts.dry_run {
        enforce_team_gate(permissions::GatedOp::Integrate, opts.force)?;
        let store_path = detect_distributed_store().ok_or_else(|| {
            anyhow::anyhow!("could not locate AIDA store for `aida integrate --run`")
        })?;
        let storage = Storage::new(store_path);
        let user_id = current_user_id(opts.user.as_deref());
        return queue_cmd::handle_queue_integrate(
            &storage,
            &user_id,
            opts.dry_run,
            opts.watch,
            opts.interval,
            opts.max,
            opts.wait_ci,
            opts.rebase,
            opts.strategy,
            opts.focus,
            opts.idle_minutes,
        );
    }

    use std::collections::HashSet;

    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let now = chrono::Utc::now();

    // --- Focus scope ---------------------------------------------------------
    let focus_label = focus::resolve_focus(&project_root);

    // --- Focus-scoped queue (reuse the queue read + cache summaries) ---------
    // The store is resolved the same way every store-bearing command resolves
    // it; a fresh / un-attached clone degrades to an empty queue rather than an
    // error (the throughput + running-work sections still render).
    let store_path = detect_distributed_store();
    let user_id = current_user_id(None);
    let mut queue_rows: Vec<(String, String, String, String)> = Vec::new(); // (id, title, status, role)
    let mut merge_rows: Vec<MergeQueueRow> = Vec::new();
    if let Some(sp) = store_path.as_deref() {
        let storage = Storage::new(sp);
        if let Ok(backend) = advance_backend(sp) {
            let summaries = backend
                .list_summaries(&aida_core::ListFilter::default())
                .unwrap_or_default();
            let by_id: std::collections::HashMap<Uuid, &aida_core::RequirementSummary> =
                summaries.iter().map(|s| (s.id, s)).collect();

            // Resolve the focus label to its subtree (cache-fast descendant
            // closure, the same `backend.descendant_ids` the focused `aida list`
            // / `aida queue list` use). None => no focus, show the whole queue.
            let subtree: Option<HashSet<Uuid>> = focus_label.as_deref().and_then(|label| {
                summaries
                    .iter()
                    .find(|s| {
                        [s.agreed_id.as_deref(), s.spec_id.as_deref()]
                            .into_iter()
                            .flatten()
                            .any(|id| id.eq_ignore_ascii_case(label))
                    })
                    .and_then(|s| backend.descendant_ids(&s.id).ok())
            });

            let raw = storage.queue_list(&user_id, false).unwrap_or_default();
            for e in &raw {
                if let Some(set) = subtree.as_ref() {
                    if !set.contains(&e.requirement_id) {
                        continue;
                    }
                }
                let Some(s) = by_id.get(&e.requirement_id) else {
                    continue;
                };
                // Mirror the default queue/list view: hide archived + terminal
                // (Completed / Rejected) so the queue reads as live work.
                if s.archived {
                    continue;
                }
                let st = s.status.to_ascii_lowercase();
                if st == "completed" || st == "rejected" {
                    continue;
                }
                let id = s
                    .agreed_id
                    .as_deref()
                    .or(s.spec_id.as_deref())
                    .unwrap_or("")
                    .to_string();
                queue_rows.push((
                    id,
                    s.title.clone(),
                    s.status.clone(),
                    e.for_role.clone().unwrap_or_default(),
                ));
            }

            let store = storage.load().ok();
            let mut ready_position = 0usize;
            for req in store
                .as_ref()
                .map(|s| s.requirements.as_slice())
                .unwrap_or(&[])
            {
                if req.status != aida_core::RequirementStatus::Done || req.archived {
                    continue;
                }
                let id = req.display_id();
                if let Some(set) = subtree.as_ref() {
                    if !set.contains(&req.id) {
                        continue;
                    }
                }
                let lookup = detect_open_pr_for_spec_via_forge(&project_root, &id);
                let inconclusive = matches!(
                    lookup,
                    PrLookup::GhMissing | PrLookup::GhFailed(_) | PrLookup::GhUnreachable(_)
                );
                let (facts, _branch, pr) = probe_resume_facts(&project_root, &storage, &id, None);
                let candidate = integrate::IntegrationCandidate {
                    id: id.clone(),
                    is_done: true,
                    has_open_pr: pr.is_some(),
                    pr_merged: facts.pr_merged,
                    pr_lookup_inconclusive: inconclusive,
                    held_for_human: req.tags.iter().any(|t| {
                        let t = t.trim();
                        t.eq_ignore_ascii_case("supervised")
                            || t.eq_ignore_ascii_case("review:draft-only")
                    }),
                };
                let ready = integrate::classify_candidate(&candidate)
                    == integrate::CandidateVerdict::Integrate;
                let position = if ready {
                    ready_position += 1;
                    Some(ready_position)
                } else {
                    None
                };
                merge_rows.push(MergeQueueRow {
                    id,
                    title: req.title.clone(),
                    state: merge_queue_state_label(position, ready, candidate.has_open_pr),
                    position,
                });
            }
        }
    }

    // --- Throughput (git log origin/main, falling back to the event stream) --
    let git_merges =
        integrate_view::read_git_merge_times(&project_root, integrate_view::GIT_LOG_SCAN_LIMIT);
    let events = integrate_view::read_events(&project_root);
    let merge_times = if git_merges.is_empty() {
        integrate_view::merge_times_from_events(&events)
    } else {
        git_merges
    };
    let throughput = integrate_view::summarize_throughput(&merge_times, now);
    let idle = integrate_view::main_idle_verdict(
        throughput.last_merge,
        now,
        integrate_view::DEFAULT_IDLE_THRESHOLD_MINS,
    );
    let drain_times = integrate_view::drain_times_from_events(&events);
    let drains_last_day = drain_times
        .iter()
        .filter(|t| **t >= now - chrono::Duration::hours(24) && **t <= now)
        .count();

    // --- Active fan-out work (reuse the `aida ps` running-work table) --------
    let (mut run_rows, _orphans) = gather_running_work(&project_root);
    // When a focus is active, keep rows whose spec is in the focus subtree; a
    // generic harness-worktree lease (spec unknown) stays visible since it
    // cannot be excluded honestly. With no focus, every row shows.
    if let (Some(_label), Some(sp)) = (focus_label.as_deref(), store_path.as_deref()) {
        if let Ok(backend) = advance_backend(sp) {
            let summaries = backend
                .list_summaries(&aida_core::ListFilter::default())
                .unwrap_or_default();
            if let Some(froot) = summaries.iter().find(|s| {
                [s.agreed_id.as_deref(), s.spec_id.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|id| id.eq_ignore_ascii_case(focus_label.as_deref().unwrap_or("")))
            }) {
                if let Ok(subtree) = backend.descendant_ids(&froot.id) {
                    let in_scope: HashSet<String> = summaries
                        .iter()
                        .filter(|s| subtree.contains(&s.id))
                        .flat_map(|s| {
                            [s.agreed_id.clone(), s.spec_id.clone()]
                                .into_iter()
                                .flatten()
                        })
                        .map(|id| id.to_ascii_lowercase())
                        .collect();
                    run_rows.retain(|r| match &r.spec {
                        Some(sp) => in_scope.contains(&sp.to_ascii_lowercase()),
                        None => true,
                    });
                }
            }
        }
    }

    // ---------------------------------------------------------------- JSON ----
    if opts.json {
        let queue_json: Vec<serde_json::Value> = queue_rows
            .iter()
            .map(|(id, title, status, role)| {
                serde_json::json!({ "id": id, "title": title, "status": status, "role": role })
            })
            .collect();
        let merge_queue_json: Vec<serde_json::Value> = merge_rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.id,
                    "title": row.title,
                    "state": row.state,
                    "position": row.position,
                })
            })
            .collect();
        let running_json: Vec<serde_json::Value> = run_rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "spec": row.spec,
                    "role": row.role,
                    "lease_role": row.lease_role,
                    "pid": row.pid,
                    "elapsed_secs": row.elapsed_secs,
                    "live": matches!(row.state, LeaseState::Live),
                    "liveness": row.state.label(),
                })
            })
            .collect();
        println!(
            "{}",
            crate::cache_output::json_pretty(&serde_json::json!({
                "focus": focus_label,
                "queue_depth": queue_rows.len(),
                "queue": queue_json,
                "merge_queue_depth": merge_rows.iter().filter(|r| r.position.is_some()).count(),
                "merge_queue": merge_queue_json,
                "throughput": {
                    "last_merge": throughput.last_merge.map(|t| t.to_rfc3339()),
                    "merges_last_hour": throughput.merges_last_hour,
                    "merges_last_day": throughput.merges_last_day,
                    "drains_last_day": drains_last_day,
                    "main_idle": idle.idle,
                    "minutes_since_last_merge": idle.minutes_since_last_merge,
                },
                "running": running_json,
            }))?
        );
        return Ok(());
    }

    // Hide STALE running rows (dead leases: worktree gone OR no live process
    // and >24h old) behind a footer count in the two consumption-facing renders
    // below, mirroring `aida ps`, which hides stale rows unless `--all`. The
    // JSON block above deliberately keeps every row so structured consumers can
    // filter on `live`/`liveness` themselves. Without this, a long-dead lease
    // (e.g. a Completed spec whose worktree was removed) clutters the
    // integrator's throughput view. trace:BUG-693 | ai:claude
    let stale_hidden = run_rows
        .iter()
        .filter(|r| matches!(r.state, LeaseState::Stale))
        .count();
    run_rows.retain(|r| !matches!(r.state, LeaseState::Stale));

    // --------------------------------------------------- AGENT (TOON) mode ----
    // Token-efficient rendering for non-TTY / AIDA_AGENT_OUTPUT consumers, the
    // TASK-964 AXI pattern: flat scalars + uniform TOON tables, no glyphs/color.
    if agent_output_mode() {
        println!("view: integrate");
        println!("focus: {}", focus_label.as_deref().unwrap_or("none"));
        println!("main_idle: {}", idle.idle);
        println!(
            "minutes_since_last_merge: {}",
            idle.minutes_since_last_merge
                .map(|m| m.to_string())
                .unwrap_or_else(|| "none".to_string())
        );
        println!("merges_last_hour: {}", throughput.merges_last_hour);
        println!("merges_last_day: {}", throughput.merges_last_day);
        println!("drains_last_day: {drains_last_day}");
        println!("queue_depth: {}", queue_rows.len());
        println!(
            "merge_queue_depth: {}",
            merge_rows.iter().filter(|r| r.position.is_some()).count()
        );
        println!("running: {}", run_rows.len());
        println!("stale_hidden: {stale_hidden}");
        let q: Vec<Vec<String>> = queue_rows
            .iter()
            .map(|(id, title, status, role)| {
                vec![
                    id.clone(),
                    title.clone(),
                    toon_status_token(status),
                    role.clone(),
                ]
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw("queue", &["id", "title", "status", "role"], &q)
        );
        let mq: Vec<Vec<String>> = merge_rows
            .iter()
            .map(|row| vec![row.id.clone(), row.title.clone(), row.state.clone()])
            .collect();
        println!(
            "{}",
            crate::toon::table_raw("merge_queue", &["id", "title", "state"], &mq)
        );
        let r: Vec<Vec<String>> = run_rows
            .iter()
            .map(|row| {
                vec![
                    row.spec.clone().unwrap_or_else(|| "-".to_string()),
                    row.role.clone().unwrap_or_else(|| "-".to_string()),
                    humanize_duration_secs(row.elapsed_secs),
                    row.state.label().to_string(),
                ]
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw("running", &["spec", "role", "elapsed", "live"], &r)
        );
        return Ok(());
    }

    // --------------------------------------------------------- HUMAN mode ----
    let arrow = crate::glyph(crate::glyphs::Glyph::Arrow);
    let queued_g = crate::glyph(crate::glyphs::Glyph::Queued);
    let robot_g = crate::glyph(crate::glyphs::Glyph::Robot);
    let hourglass = crate::glyph(crate::glyphs::Glyph::Hourglass);
    let check_g = crate::glyph(crate::glyphs::Glyph::Check);
    let warn_g = crate::glyph(crate::glyphs::Glyph::Warning);

    println!("{}", "Integrator".bold());
    match focus_label.as_deref() {
        Some(f) => println!("{} {}", arrow.cyan(), format!("focus: {f}").cyan()),
        None => println!("{}", "scope: all (no focus set)".dimmed()),
    }
    println!();

    // Queue ------------------------------------------------------------------
    println!(
        "{}  {}",
        "Queue".bold(),
        format!("{} routed", queue_rows.len()).dimmed()
    );
    if queue_rows.is_empty() {
        println!("  {}", "(queue empty)".dimmed());
    } else {
        for (id, title, _status, role) in queue_rows.iter().take(15) {
            let role_col = if role.is_empty() {
                "unrouted".dimmed().to_string()
            } else {
                role.cyan().to_string()
            };
            println!(
                "  {} {:<14} {}  {}",
                queued_g.cyan(),
                id.yellow(),
                truncate(title, 48),
                role_col,
            );
        }
        if queue_rows.len() > 15 {
            println!(
                "  {}",
                format!("(+{} more)", queue_rows.len() - 15).dimmed()
            );
        }
    }
    println!();

    // Merge queue ------------------------------------------------------------
    let merge_queue_depth = merge_rows.iter().filter(|r| r.position.is_some()).count();
    println!(
        "{}  {}",
        "Merge queue".bold(),
        format!("{} ready", merge_queue_depth).dimmed()
    );
    if merge_rows.is_empty() {
        println!("  {}", "(no Done specs awaiting integration)".dimmed());
    } else {
        for row in merge_rows.iter().take(15) {
            let marker = if row.position.is_some() {
                arrow.green()
            } else {
                "·".dimmed()
            };
            println!(
                "  {} {:<14} {}  {}",
                marker,
                row.id.yellow(),
                truncate(&row.title, 48),
                row.state.cyan(),
            );
        }
        if merge_rows.len() > 15 {
            println!(
                "  {}",
                format!("(+{} more)", merge_rows.len() - 15).dimmed()
            );
        }
    }
    println!();

    // Throughput -------------------------------------------------------------
    println!("{}", "Throughput".bold());
    let last_merge_str = match throughput.last_merge {
        Some(ts) => {
            let secs = (now - ts).num_seconds().max(0) as u64;
            format!("{} ago", humanize_duration_secs(secs))
        }
        None => "no merges on record".to_string(),
    };
    let idle_badge = if idle.idle {
        format!("{} idle", warn_g).yellow().to_string()
    } else {
        format!("{} moving", check_g).green().to_string()
    };
    println!("  Last merge to main:  {last_merge_str}   {idle_badge}");
    if idle.idle {
        if let Some(mins) = idle.minutes_since_last_merge {
            println!(
                "  {}",
                format!(
                    "(no merge in {mins}m — threshold {}m)",
                    integrate_view::DEFAULT_IDLE_THRESHOLD_MINS
                )
                .dimmed()
            );
        }
    }
    println!(
        "  Merges to main:      {} in last 1h {} {} in last 24h",
        throughput.merges_last_hour,
        "·".dimmed(),
        throughput.merges_last_day,
    );
    if !drain_times.is_empty() {
        println!("  Drains completed:    {drains_last_day} in last 24h");
    }
    println!();

    // Active fan-out ---------------------------------------------------------
    println!(
        "{}  {}",
        "Running work".bold(),
        format!("{} active", run_rows.len()).dimmed()
    );
    if run_rows.is_empty() {
        println!("  {}", "(no active sessions)".dimmed());
    } else {
        for row in &run_rows {
            let spec_col = row
                .spec
                .clone()
                .unwrap_or_else(|| truncate(&row.lease.scope, 14));
            let role_col = row.role.as_deref().unwrap_or("-");
            let live_label = format!("{} {}", row.state.glyph(), row.state.label());
            let live_col = match row.state {
                LeaseState::Live => live_label.green(),
                LeaseState::Dormant => live_label.cyan(),
                LeaseState::Stale => live_label.yellow(),
            };
            println!(
                "  {} {:<14} {:<10} {:<8} {}",
                robot_g.cyan(),
                truncate(&spec_col, 14),
                truncate(role_col, 10),
                humanize_duration_secs(row.elapsed_secs),
                live_col,
            );
        }
    }
    if stale_hidden > 0 {
        println!(
            "  {}",
            format!(
                "({stale_hidden} stale session{} hidden — `aida ps --all` for detail)",
                if stale_hidden == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }
    println!();
    println!(
        "{}",
        format!("{hourglass} read-only view — `aida integrate --run` to drain the merge queue, `aida ps` for full session detail").dimmed()
    );

    Ok(())
}

/// TASK-1072: the minimal per-spec projection `gather_running_work` needs — a
/// lease scope's display id (row spec resolution) plus the In-Progress /
/// excluded-type flags for the orphan pass. Deliberately tiny so it can be built
/// from the cache-fast SQLite summaries (the STORY-707 pattern) instead of a
/// full `backend.load()` (which parses every YAML — ~1.0s of `aida ps`'s ~1.3s).
// trace:TASK-1072 | ai:claude
pub(crate) struct RunningWorkSpec {
    /// agreed_id || spec_id || uuid — what a resolved row / orphan displays.
    pub(crate) disp: String,
    pub(crate) agreed_id: Option<String>,
    pub(crate) spec_id: Option<String>,
    pub(crate) title: String,
    /// The spec is currently In Progress (orphan-pass candidate).
    pub(crate) in_progress: bool,
    /// Rollup / stateless type (epic / folder / meta) — excluded from the
    /// orphan pass because it never holds a spec-scoped lease.
    // trace:TASK-940 | ai:claude
    pub(crate) orphan_excluded_type: bool,
}

/// TASK-1072: string form of [`ps_orphan_excluded_type`], for the cache-fast
/// path where the type arrives as the cache's Debug-form string ("Epic",
/// "Folder", "Meta") rather than a parsed [`aida_core::RequirementType`]. Kept
/// in lockstep with the enum variant so the two paths agree.
// trace:TASK-1072 trace:TASK-940 | ai:claude
pub(crate) fn ps_orphan_excluded_type_str(req_type: &str) -> bool {
    req_type.eq_ignore_ascii_case("Epic")
        || req_type.eq_ignore_ascii_case("Folder")
        || req_type.eq_ignore_ascii_case("Meta")
}

/// TASK-1072: pure mapping from a cache summary's raw fields to a
/// [`RunningWorkSpec`] — the disp-id precedence (agreed_id → spec_id → uuid) and
/// the two derived flags (In-Progress candidacy, orphan-excluded type). Extracted
/// so the cache-fast projection is unit-testable without a live backend, and so
/// the string-form status/type decoding stays in one audited place.
// trace:TASK-1072 | ai:claude
pub(crate) fn running_work_spec_from_summary(s: aida_core::RequirementSummary) -> RunningWorkSpec {
    let disp = s
        .agreed_id
        .clone()
        .or_else(|| s.spec_id.clone())
        .unwrap_or_else(|| s.id.to_string());
    RunningWorkSpec {
        disp,
        in_progress: aida_core::RequirementStatus::from_filter_str(&s.status)
            == Some(aida_core::RequirementStatus::InProgress),
        orphan_excluded_type: ps_orphan_excluded_type_str(&s.req_type),
        agreed_id: s.agreed_id,
        spec_id: s.spec_id,
        title: s.title,
    }
}

/// TASK-1072: build the [`RunningWorkSpec`] index cache-fast. Reads the SQLite
/// summary projection (`list_summaries`, which triggers a stale-check +
/// auto-rebuild first — the same freshness contract bare `aida status` /
/// `aida list` ride) instead of a full `backend.load()`. The summaries carry
/// exactly the fields the running-work picture needs (ids, title, status,
/// req_type), so no YAML parse is required. Uses `ArchiveFilter::Both` +
/// `DeferFilter::Both` to match the prior full-store scan, which iterated every
/// requirement regardless of the archive/defer view flags. Falls back to the
/// full store load only when the cached backend is unreachable (legacy /
/// un-attached clones), preserving `load_store_for_lookup`'s degrade path.
/// Read-only; returns an empty index when no store is reachable.
// trace:TASK-1072 | ai:claude
pub(crate) fn running_work_spec_index(project_root: &std::path::Path) -> Vec<RunningWorkSpec> {
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        if let Ok(backend) = advance_backend(&store_path) {
            let filter = aida_core::ListFilter {
                archive: aida_core::ArchiveFilter::Both,
                defer: aida_core::DeferFilter::Both,
                ..Default::default()
            };
            if let Ok(summaries) = backend.list_summaries(&filter) {
                return summaries
                    .into_iter()
                    .map(running_work_spec_from_summary)
                    .collect();
            }
        }
    }
    // Legacy / un-attached fallback: the full store load (same degrade path as
    // `load_store_for_lookup`). trace:TASK-1072 | ai:claude
    load_store_for_lookup(project_root)
        .map(|store| {
            store
                .requirements
                .iter()
                .map(|r| {
                    let disp = r
                        .agreed_id
                        .clone()
                        .or_else(|| r.spec_id.clone())
                        .unwrap_or_else(|| r.id.to_string());
                    RunningWorkSpec {
                        disp,
                        agreed_id: r.agreed_id.clone(),
                        spec_id: r.spec_id.clone(),
                        title: r.title.clone(),
                        in_progress: matches!(r.status, aida_core::RequirementStatus::InProgress),
                        orphan_excluded_type: ps_orphan_excluded_type(&r.req_type),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build the GLOBAL running-work picture: one [`PsRow`] per active session
/// lease (with liveness/pid/elapsed/spec resolved) plus the orphaned
/// In-Progress specs with no live spec-scoped lease. Extracted from `handle_ps`
/// so the `aida ps` table and the `aida integrate` throughput view share ONE
/// definition of "what's running right now" and can never disagree.
///
/// One pid-probe + one lease read feed both passes, the same single-probe
/// discipline `aida session leases` uses. Read-only; degrades gracefully when
/// the store is unreachable (rows still resolve, specs read as scope-unknown).
// trace:TASK-1034 trace:STORY-696 | ai:claude
pub(crate) fn gather_running_work(project_root: &std::path::Path) -> (Vec<PsRow>, Vec<PsOrphan>) {
    let now = chrono::Utc::now();
    // ONE /proc pass, reused for every liveness check below (memoized by
    // `probe_live_claude_sessions`'s OnceLock — BUG-613). The row + orphan
    // passes look each session up in this in-memory slice; they never re-probe
    // per lease. trace:TASK-1072 | ai:claude
    let live = process_probe::probe_live_claude_sessions();
    let leases = list_leases(project_root);
    let manifests = session_manifest::list_all(project_root);
    let manifest_roles: std::collections::HashMap<String, String> = manifests
        .iter()
        .filter_map(|m| {
            let role = m
                .claude_session_id
                .as_deref()
                .and_then(session::role_from_claude_session_id)?;
            Some((m.session_id.clone(), role))
        })
        .collect();
    // BUG-1553: lease id -> the `claude` conversation's own session id
    // (STORY-153's join, reused here) — lets the activity probe resolve
    // this session's transcript path even long after `probe_live_claude_sessions`'s
    // own recent-jsonl window (60s) has lapsed, which a 20-minute permission
    // block always exceeds. trace:BUG-1553 | ai:claude
    let manifest_claude_session_ids: std::collections::HashMap<String, String> = manifests
        .into_iter()
        .filter_map(|m| m.claude_session_id.map(|csid| (m.session_id, csid)))
        .collect();
    // TASK-1454: active (non-stale) pending-approval markers, keyed by the
    // Claude Code session id they were recorded for. `list_active` is cheap
    // when nothing is blocked (the overwhelming common case — a single
    // `read_dir` on a directory that's usually empty), so building this map
    // up front costs nothing extra on a quiet `aida ps`.
    // trace:TASK-1454 | ai:claude
    let pending_approvals: std::collections::HashMap<
        String,
        pending_approval::PendingApprovalMarker,
    > = pending_approval::list_active(project_root, now)
        .into_iter()
        .map(|m| (m.session_id.clone(), m))
        .collect();
    // BUG-1553 / TASK-1454: the real (marker-first, then transcript-tail +
    // /proc-reading) seat-activity probe — injected into `build_running_work`
    // the same way every other real I/O source here is, so the row-building
    // logic stays testable on fixtures. A recorded pending-approval marker is
    // ground truth and wins outright; only when none exists does this fall
    // back to reading the TAIL of the resolved transcript
    // (`read_transcript_tail`, capped at `TRANSCRIPT_TAIL_BYTES`, never the
    // whole file). trace:BUG-1553 trace:TASK-1454 | ai:claude
    let seat_activity_probe = |l: &SessionLease, pid: u32| -> SeatActivity {
        let marker = manifest_claude_session_ids
            .get(&l.id)
            .and_then(|csid| pending_approvals.get(csid));
        let tail = manifest_claude_session_ids.get(&l.id).and_then(|csid| {
            aida_core::liveness::claude_projects_dir_for_cwd(&l.worktree_path)
                .map(|dir| dir.join(format!("{csid}.jsonl")))
        });
        let tail_lines = tail.and_then(|path| read_transcript_tail(&path, TRANSCRIPT_TAIL_BYTES));
        seat_activity_with_marker(marker, tail_lines.as_deref(), proc_is_stopped(pid), now)
    };

    // The store gives us (a) the set of known spec ids (so a lease scope can be
    // resolved to a spec vs. a generic harness scope) and (b) the In-Progress
    // specs for the orphan pass. TASK-1072: this is the cache-fast summary
    // projection (the STORY-707 pattern), NOT a full `backend.load()` — profiling
    // showed the full YAML parse was ~1.0s of `aida ps`'s ~1.3s. Read-only;
    // degrades gracefully (empty index) when the store is unreachable.
    // trace:TASK-1072 | ai:claude
    let specs = running_work_spec_index(project_root);

    // trace:TASK-1090 | ai:claude — the real (git-probing) dispatch seam.
    // trace:TASK-1143 | ai:claude — the real (lease-reading) lock-owner seam:
    // `worktree_lock::read_authorized_by` reads the `authorized_by` value off the
    // session lease covering each worktree — the same STORY-711 slice-1 source
    // `aida lock` writes and the commit gate reads. Injected (not called inline)
    // so `build_running_work` stays filesystem-free and unit-testable.
    // trace:BUG-763 | ai:claude — the real (single-pid /proc-reading) start-time
    // seam, injected the same way as the git + lock probes so
    // `build_running_work` stays filesystem-free and unit-testable.
    let (mut rows, mut orphans) = build_running_work(
        &specs,
        &leases,
        &live,
        now,
        dispatch_health_ps::probe_worktree,
        |wt| worktree_lock::read_authorized_by(project_root, wt),
        pid_start_time,
        |jsonl| session::role_from_jsonl(jsonl, "claude").ok().flatten(),
        |lease_id| manifest_roles.get(lease_id).cloned(),
        probe_mail_identity,
        seat_activity_probe,
    );
    // TASK-163: a dead phase child does not make its lease stale while the
    // drain orchestrator owns that spec. Overlay the authoritative drain PID
    // and suppress the contradictory orphan row.
    for row in &mut rows {
        let Some(spec) = row.spec.as_deref() else {
            continue;
        };
        if let Some(drain) = drain_state::live_drain_spec(project_root, spec) {
            row.state = LeaseState::Live;
            row.pid = Some(drain.pid);
            row.pid_started_at = pid_start_time(drain.pid);
            row.role = Some(format!("drain {}", drain.phase));
            row.dispatch = None;
            // TASK-1451: re-probe under the authoritative drain pid, not the
            // (possibly stale/absent) pid this row resolved before the
            // overlay.
            row.mail_identity = Some(probe_mail_identity(drain.pid));
            // BUG-1553: a drain phase is orchestrator-driven, not a human
            // sitting at a permission prompt, and the overlay pid isn't the
            // one the lease's own transcript join resolves — leave activity
            // unclassified rather than risk a misattributed tail read.
            row.activity = None;
        }
    }
    // BUG-1681: a row whose spec has already FINISHED (done + approved, or
    // riding an integration PR) is waiting on the integrator — reframe it
    // before anything prints a resume hint for work that already landed.
    // trace:BUG-1681 | ai:claude
    ps_overlay_integration_standing(project_root, &mut rows);
    orphans.retain(|orphan| drain_state::live_drain_spec(project_root, &orphan.spec).is_none());
    (rows, orphans)
}

/// BUG-1523 (AC3): the pure mapping from `aida ps`'s orphan-detection output
/// ([`PsOrphan`], produced by [`build_running_work`] / [`gather_running_work`])
/// to the `aida awaiting` surface item ([`awaiting_you::OrphanedInProgressItem`]).
/// Extracted from `collect_awaiting_report_inner` so the "does a genuinely
/// orphaned In-Progress spec reach the report" question is testable end to end
/// from detection through emission, not just against a hand-built item (what
/// the pre-existing rendering test covered). A fan-out-worked flag-only spec
/// (TASK-1064) is filtered out here, same as `aida ps`'s own framing — it is
/// informational, not a genuine anomaly. `since_label_for` is injected so this
/// stays free of the cache/summary lookup and `Utc::now()` the real caller
/// wires in.
// trace:BUG-1523 | ai:claude
pub(crate) fn orphaned_in_progress_items(
    orphans: Vec<PsOrphan>,
    since_label_for: impl Fn(&str) -> String,
) -> Vec<awaiting_you::OrphanedInProgressItem> {
    orphans
        .into_iter()
        .filter(|o| !o.likely_fanout)
        .map(|o| awaiting_you::OrphanedInProgressItem {
            since_label: since_label_for(&o.spec),
            spec_id: o.spec,
            title: o.title,
            abandoned: o.stale_lease,
            // trace:BUG-1656 | ai:claude
            possibly_subagent: o.possibly_subagent,
        })
        .collect()
}

/// TASK-1454: `aida awaiting`'s view of every LIVE seat `aida ps` classifies
/// `SeatActivity::Blocked` — reuses `gather_running_work` verbatim (the same
/// marker-first classifier `seat_activity_probe` applies), so there is
/// exactly ONE place that decides "is this seat blocked," not a second
/// implementation drifting from the first.
///
/// Cheap on EVERY caller, including the per-turn notice-fast path: it starts
/// with [`pending_approval::list_active`], a single local directory read
/// that is empty on the overwhelming majority of turns (nothing is ever
/// blocked most of the time) — that empty case returns immediately with no
/// lease scan, no `/proc` probe, no store read at all. Only when a marker
/// genuinely exists does this pay `gather_running_work`'s heavier
/// lease-resolution cost, and that is exactly the rare, actionable moment
/// where paying it is worth it.
// trace:TASK-1454 | ai:claude
pub(crate) fn collect_blocked_seat_items(
    project_root: &std::path::Path,
) -> Vec<awaiting_you::BlockedSeatItem> {
    if pending_approval::list_active(project_root, chrono::Utc::now()).is_empty() {
        return Vec::new();
    }
    let (rows, _orphans) = gather_running_work(project_root);
    rows.into_iter()
        .filter_map(|row| match row.activity {
            Some(SeatActivity::Blocked { tool, secs }) => Some(awaiting_you::BlockedSeatItem {
                session_id: row.lease.id.clone(),
                spec: row.spec.clone().or(Some(row.lease.scope.clone())),
                tool,
                since_label: humanize_duration_secs(secs.max(0) as u64),
            }),
            _ => None,
        })
        .collect()
}

/// TASK-1072: the pure core of [`gather_running_work`] — given the resolved spec
/// index, the session leases, and the ONE already-computed live-session slice,
/// build the row + orphan picture. Extracted from the store/proc/lease I/O so
/// the running-work logic (scope→spec resolution, the orphan verdict, the
/// fan-out framing, the sort) is unit-testable on fixtures without touching the
/// filesystem or `/proc`. Takes `live` by reference — the single-probe
/// discipline (`aida ps` probes `/proc` once, not once per lease) is a caller
/// invariant this signature enforces.
///
/// TASK-1090: `dispatch_probe` is the ONE seam that touches git — injected
/// (rather than called directly) so this function stays exercisable on pure
/// fixtures in tests (a no-op stub) while the real caller
/// ([`gather_running_work`]) wires in [`dispatch_health_ps::probe_worktree`].
///
/// TASK-1143: `lock_probe` is a second injected seam — given a worktree path it
/// returns the advisor holding the STORY-711 worktree lock (the lease's
/// `authorized_by`), or `None`. Injected the same way as `dispatch_probe` so the
/// lock-owner resolution stays exercisable on pure fixtures; the real caller
/// wires in `worktree_lock::read_authorized_by`.
///
/// BUG-763: `pid_start_probe` is a third injected seam — given a live pid it
/// returns that process's start time, or `None`. Feeds the adopted-lease
/// annotation (a persistent lease whose pid is younger than the lease record);
/// the real caller wires in [`pid_start_time`].
///
/// TASK-153: `manifest_role_probe` resolves the transcript role through the
/// lease-manifest `claude_session_id` join, so long-lived resumed sessions do
/// not fall back to the stale harness lease role when the live `/proc` JSONL
/// association is absent.
// trace:TASK-1072 trace:STORY-696 trace:TASK-1090 trace:TASK-1143 trace:BUG-763 trace:TASK-153 | ai:claude
pub(crate) fn build_running_work(
    specs: &[RunningWorkSpec],
    leases: &[SessionLease],
    live: &[process_probe::LiveSession],
    now: chrono::DateTime<chrono::Utc>,
    dispatch_probe: impl Fn(&std::path::Path) -> dispatch_health_ps::WorktreeGitProbe,
    lock_probe: impl Fn(&std::path::Path) -> Option<String>,
    pid_start_probe: impl Fn(u32) -> Option<chrono::DateTime<chrono::Utc>>,
    role_probe: impl Fn(&std::path::Path) -> Option<String>,
    manifest_role_probe: impl Fn(&str) -> Option<String>,
    mail_identity_probe: impl Fn(u32) -> MailIdentityStatus,
    // BUG-1553: given the lease and its live pid, classify Working /
    // LongToolCall / Suspended / Unknown. Only called for a row with a
    // resolved live pid — a Dormant/Stale row has no process to inspect and
    // stays `None`.
    seat_activity_probe: impl Fn(&SessionLease, u32) -> SeatActivity,
) -> (Vec<PsRow>, Vec<PsOrphan>) {
    let rows: Vec<PsRow> = leases
        .iter()
        .map(|l| {
            let state = lease_state_for(l, live, now);
            let live_in_worktree = live.iter().find(|s| {
                !s.stale_cwd && (s.cwd == l.worktree_path || s.cwd.starts_with(&l.worktree_path))
            });
            let pid = l
                .active_pid
                .filter(|p| process_probe::pid_is_alive(*p))
                .or_else(|| live_in_worktree.map(|s| s.pid))
                .or(if l.review_verb || l.claim_verb {
                    l.creator_pid
                } else {
                    None
                });
            let live_by_pid = pid.and_then(|p| live.iter().find(|s| s.pid == p));
            let jsonl_role = live_by_pid
                .or(live_in_worktree)
                .and_then(|s| s.jsonl.as_deref())
                .and_then(&role_probe);
            let manifest_role = manifest_role_probe(&l.id);
            let lease_role = l.role.clone();
            // BUG-1521: the role shown must be READ from the session's own
            // record (the lease's stored `role`) whenever that record is a
            // REAL role — not the harness's generic Agent-tool placeholder
            // (`tail_cmd::HARNESS_AGENT_TYPE`, "general-purpose"). The
            // jsonl/manifest heuristics match on ambiguous signals (a shared
            // cwd, a text marker anywhere in the log) that can resolve to a
            // DIFFERENT live session's transcript — and can resolve
            // differently between two invocations of the same command
            // seconds apart, since each re-scans independently — so a real
            // recorded lease role is authoritative and wins first. But the
            // placeholder itself carries no information (every harness
            // fan-out subagent gets it, regardless of actual role), so a
            // lease stuck with it falls through to the derived signals
            // instead of masking them: manifest_role (TASK-153's stable
            // claude_session_id join) next, then jsonl_role (the ambiguous
            // transcript scan), and only the placeholder itself as the last
            // resort when nothing else resolved. trace:BUG-1521 | ai:claude
            let role = ps_display_role(
                lease_role.as_deref(),
                manifest_role.as_deref(),
                jsonl_role.as_deref(),
            );
            // BUG-763: resolve the backing pid's own start time so an adopted
            // persistent lease (pid younger than the lease record) can name
            // both ages instead of mixing provenance silently.
            let pid_started_at = pid.and_then(&pid_start_probe);
            // BUG-769: the displayed `elapsed` is the WORK-SESSION age — the
            // process's uptime on an adopted lease, the lease's age otherwise.
            // trace:BUG-769 | ai:claude
            let elapsed_secs = ps_elapsed_secs(l.started_at, pid_started_at, now);
            // The lease record's own age stays the input to the dispatch-health
            // stalled threshold (TASK-1090 semantics, deliberately unchanged by
            // BUG-769 — that classifier is not the elapsed column).
            let lease_elapsed_secs =
                now.signed_duration_since(l.started_at).num_seconds().max(0) as u64;
            let spec = specs
                .iter()
                .find(|s| {
                    [s.agreed_id.as_deref(), s.spec_id.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|id| l.scope.eq_ignore_ascii_case(id))
                })
                .map(|s| s.disp.clone());
            // TASK-1090: worktree-less advisory leases (review/claim locks)
            // have no git state to classify — dispatch stays None for them.
            // A dead-worktree lease (removed dir) also has nothing to probe;
            // `dispatch_health_ps::probe_worktree` degrades to the zero
            // default in that case rather than erroring.
            let dispatch = if l.review_verb || l.claim_verb {
                None
            } else {
                let probe = dispatch_probe(&l.worktree_path);
                // BUG-752: tri-state — a harness (Agent-tool) lease with no
                // recorded pid has UNDETERMINABLE liveness (the worker runs
                // inside the parent claude process, invisible to the
                // cwd-based worktree probe), so it must not be treated as a
                // dead process and offered the salvage-commit hint.
                let pid_alive = ps_pid_liveness(state, harness_lease_without_pid_signal(l));
                // BUG-778: how long ago `aida worktree enter|add` handed this
                // worktree to a human, when it did. `None` on every
                // orchestrator-spawned lease — those DO expect an agent, so
                // their crash detection is untouched. trace:BUG-778 | ai:claude
                let manual_enter_secs = ps_manual_enter_secs(l, now);
                // BUG-1656: a dirty tree still being written is liveness in
                // its own right — never offer the salvage-commit while the
                // files are changing under someone. trace:BUG-1656 | ai:claude
                let dirty_movement_fresh = dispatch_health_ps::dirty_movement_is_fresh(
                    probe.dirty_newest_mtime_age_secs,
                    dispatch_health_ps::DEFAULT_DIRTY_MOVEMENT_FRESH_SECS,
                );
                let ds = dispatch_health_ps::dispatch_state_with_movement(
                    pid_alive,
                    probe.dirty,
                    probe.ahead_of_main,
                    lease_elapsed_secs,
                    dispatch_health_ps::DEFAULT_STALLED_THRESHOLD_SECS,
                    manual_enter_secs,
                    dispatch_health_ps::DEFAULT_AWAITING_AGENT_GRACE_SECS,
                    dirty_movement_fresh,
                );
                // TASK-1518: a lease the drain's SIGTERM handler stamped on
                // its way out is a stopped wave, not a crashed agent — the
                // dead-process/clean-tree arm reads `stopped`.
                // trace:TASK-1518 | ai:claude
                let ds = dispatch_health_ps::apply_interruption(
                    ds,
                    pid_alive,
                    l.interrupted_at.is_some(),
                );
                let hint = dispatch_health_ps::next_command_hint_with_untracked(
                    ds,
                    &l.worktree_path,
                    &l.branch,
                    probe.last_commit_subject.as_deref(),
                    spec.as_deref(),
                    manual_enter_secs.is_some(),
                    probe.untracked_only,
                );
                Some(PsDispatch {
                    state: ds,
                    hint,
                    dirty: probe.dirty,
                    ahead_of_main: probe.ahead_of_main,
                    untracked_only: probe.untracked_only,
                })
            };
            // TASK-1143: the worktree lock owner (if any) for this row, read
            // from the same lease source the lock CLI writes. A worktree-less
            // advisory lease (review/claim) has an empty path that matches no
            // lease → `None`.
            let locked_by = lock_probe(&l.worktree_path).filter(|s| !s.is_empty());
            // TASK-1451: only probe a pid that actually backs this row — no
            // live process, nothing to read an environment from.
            let mail_identity = pid.map(&mail_identity_probe);
            // BUG-1553: same gate as mail_identity — only a row with a live
            // pid has a process worth classifying at all.
            let activity = pid.map(|p| seat_activity_probe(l, p));
            PsRow {
                lease: l.clone(),
                state,
                role,
                lease_role,
                pid,
                pid_started_at,
                elapsed_secs,
                spec,
                dispatch,
                locked_by,
                mail_identity,
                activity,
            }
        })
        .collect();

    // Orphan pass: every In-Progress spec with no LIVE spec-scoped lease. A
    // dead-pid (STALE) lease still counts as orphaned — the flag is not
    // liveness-backed — but we mark it so the operator sees "crashed session"
    // vs "never started". trace:STORY-696
    let mut orphans: Vec<PsOrphan> = Vec::new();
    // TASK-1064: is a live advisor Agent-tool fan-out running? If so, a flag-only
    // In-Progress spec (no spec-scoped lease) is most likely being built by it
    // (the fan-out takes a generic non-spec-linked harness lease) — surface it
    // informationally instead of as a genuine orphan. trace:TASK-1064 | ai:claude
    // BUG-1681: the live fan-out leases THEMSELVES, so the framing below can
    // be matched per spec instead of applied to every flag-only row in the
    // store the moment any subagent is alive. trace:BUG-1681 | ai:claude
    // BUG-1740: the caller is excluded from fan-out corroboration. Resolved
    // ONCE here (one /proc walk per `aida ps`, not one per lease) and passed
    // into the pure predicate. trace:BUG-1740 | ai:claude
    let caller_pids = ps_caller_pid_chain();
    let fanouts = ps_live_fanout_leases(leases, live, now, &caller_pids);
    for s in specs {
        if !s.in_progress {
            continue;
        }
        // Rollup / stateless types (epic, folder, meta) never hold a
        // spec-scoped lease, so they'd all read as orphaned — skip them so
        // only real work-item In-Progress specs surface. trace:TASK-940
        if s.orphan_excluded_type {
            continue;
        }
        let mut id_owned: Vec<String> = Vec::new();
        if let Some(a) = s.agreed_id.as_deref() {
            id_owned.push(a.to_string());
        }
        if let Some(sp) = s.spec_id.as_deref() {
            id_owned.push(sp.to_string());
        }
        let id_refs: Vec<&str> = id_owned.iter().map(|s| s.as_str()).collect();
        let lease = spec_scoped_lease(leases, &id_refs);
        let lease_state = lease.map(|l| lease_state_for(l, live, now));
        // trace:BUG-778 | ai:claude
        let awaiting_agent = lease.is_some_and(|l| ps_row_awaiting_agent(&rows, &l.id));
        // BUG-1656: a non-live spec lease whose worktree is still being
        // written (fresh dirty mtimes) is being worked — by an Agent-tool
        // subagent the pid probe cannot see — not orphaned. Only probed for
        // the few non-live spec leases, never for every row.
        // trace:BUG-1656 | ai:claude
        let dirty_movement_fresh = lease
            .filter(|_| !matches!(lease_state, Some(LeaseState::Live)))
            .filter(|l| !l.review_verb && !l.claim_verb)
            .map(|l| dispatch_probe(&l.worktree_path))
            .is_some_and(|p| {
                p.dirty
                    && dispatch_health_ps::dirty_movement_is_fresh(
                        p.dirty_newest_mtime_age_secs,
                        dispatch_health_ps::DEFAULT_DIRTY_MOVEMENT_FRESH_SECS,
                    )
            });
        // BUG-1681: attribution must MATCH. A live subagent counts for this
        // spec only when its own branch/worktree names the spec (the flag-only
        // framing), or when it is working inside this spec's own leased
        // worktree (the stale-lease framing). trace:BUG-1681 | ai:claude
        let fanout_names_this_spec = fanouts.iter().any(|f| ps_fanout_names_spec(f, &id_refs));
        let subagent_in_this_worktree =
            lease.is_some_and(|l| fanouts.iter().any(|f| ps_fanout_holds_lease(f, l)));
        if let Some(stale_lease) =
            ps_orphan_verdict_with_movement(lease_state, awaiting_agent, dirty_movement_fresh)
        {
            orphans.push(PsOrphan {
                spec: s.disp.clone(),
                title: s.title.clone(),
                stale_lease,
                // trace:TASK-1064 | ai:claude
                // trace:BUG-1681 | ai:claude
                likely_fanout: ps_orphan_likely_fanout(stale_lease, fanout_names_this_spec),
                // trace:BUG-1656 | ai:claude
                // trace:BUG-1681 | ai:claude
                possibly_subagent: stale_lease && subagent_in_this_worktree,
            });
        }
    }
    orphans.sort_by(|a, b| a.spec.cmp(&b.spec));

    (rows, orphans)
}

/// BUG-1680: Group salvageable rows by worktree path to collapse duplicates.
/// Rows for the same worktree path are shown once with a count, preserving all spec IDs.
// trace:BUG-1680 | ai:antigravity
pub(crate) struct CollapsedSalvageRow<'a> {
    pub(crate) row: &'a PsRow,
    pub(crate) count: usize,
    pub(crate) spec_ids: Vec<String>,
    pub(crate) untracked_only: bool,
}

impl<'a> CollapsedSalvageRow<'a> {
    pub(crate) fn display_spec(&self) -> String {
        if !self.spec_ids.is_empty() {
            self.spec_ids.join(", ")
        } else {
            self.row
                .spec
                .clone()
                .unwrap_or_else(|| self.row.lease.scope.clone())
        }
    }

    pub(crate) fn display_spec_with_count(&self) -> String {
        let spec = self.display_spec();
        if self.count > 1 {
            format!("{spec} ({} sessions)", self.count)
        } else {
            spec
        }
    }
}

// trace:BUG-1680 | ai:antigravity
pub(crate) fn collapse_salvageable_by_worktree<'a>(
    rows: &[&'a PsRow],
) -> Vec<CollapsedSalvageRow<'a>> {
    let mut collapsed: Vec<CollapsedSalvageRow<'a>> = Vec::new();
    for row in rows {
        let wt = &row.lease.worktree_path;
        if wt.as_os_str().is_empty() {
            let mut spec_ids = Vec::new();
            if let Some(ref s) = row.spec {
                spec_ids.push(s.clone());
            }
            let untracked_only = row
                .dispatch
                .as_ref()
                .map(|d| d.untracked_only)
                .unwrap_or(false);
            collapsed.push(CollapsedSalvageRow {
                row,
                count: 1,
                spec_ids,
                untracked_only,
            });
            continue;
        }

        if let Some(pos) = collapsed
            .iter()
            .position(|c| &c.row.lease.worktree_path == wt)
        {
            collapsed[pos].count += 1;
            if matches!(row.state, LeaseState::Live)
                && !matches!(collapsed[pos].row.state, LeaseState::Live)
            {
                collapsed[pos].row = row;
            } else if collapsed[pos].row.dispatch.is_none() && row.dispatch.is_some() {
                collapsed[pos].row = row;
            }
            if let Some(ref s) = row.spec {
                if !collapsed[pos].spec_ids.contains(s) {
                    collapsed[pos].spec_ids.push(s.clone());
                }
            }
            if let Some(d) = &row.dispatch {
                if collapsed[pos].row.dispatch.is_none() {
                    collapsed[pos].untracked_only = d.untracked_only;
                } else if !d.untracked_only {
                    collapsed[pos].untracked_only = false;
                }
            }
        } else {
            let mut spec_ids = Vec::new();
            if let Some(ref s) = row.spec {
                spec_ids.push(s.clone());
            }
            let untracked_only = row
                .dispatch
                .as_ref()
                .map(|d| d.untracked_only)
                .unwrap_or(false);
            collapsed.push(CollapsedSalvageRow {
                row,
                count: 1,
                spec_ids,
                untracked_only,
            });
        }
    }
    collapsed
}

pub(crate) fn handle_ps(json: bool, all: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());

    let (rows, orphans) = gather_running_work(&project_root);
    // trace:TASK-1285 | ai:codex
    let live_wave = drain_lock::read_pid_live_lock(&project_root);

    // STORY-769: the last-human-input presence oracle — "operator last seen Nm
    // ago" + an active/idle/stale verdict, from the per-turn `aida awaiting
    // --notice` stamp. A fan-out overseer reading `aida ps` sees at a glance
    // whether the operator is around to answer an interactive ask.
    // trace:STORY-769 | ai:claude
    let now = chrono::Utc::now();
    let thresholds = presence::read_presence_thresholds(&config_path_for_project(&project_root));
    let operator_last_seen = presence::latest_human_input();

    if json {
        let operator = match operator_last_seen {
            Some(last) => serde_json::json!({
                "last_seen": last.to_rfc3339(),
                "ago": presence::since_label(last, now),
                "verdict": presence::human_presence(now, last, thresholds).word(),
            }),
            None => serde_json::Value::Null,
        };
        let row_refs: Vec<&PsRow> = rows.iter().collect();
        let collapsed_rows = collapse_salvageable_by_worktree(&row_refs);
        let sessions: Vec<serde_json::Value> = collapsed_rows
            .iter()
            .map(|item| {
                let row = item.row;
                let spec_val = if item.spec_ids.is_empty() {
                    row.spec.clone()
                } else {
                    Some(item.spec_ids.join(", "))
                };
                serde_json::json!({
                    "session_id": row.lease.id,
                    "scope": row.lease.scope,
                    "spec": spec_val,
                    "specs": item.spec_ids,
                    "count": item.count,
                    "untracked_only": item.untracked_only,
                    "role": row.role,
                    "lease_role": row.lease_role,
                    "worktree": row.lease.worktree_path.display().to_string(),
                    "branch": row.lease.branch,
                    "pid": row.pid,
                    "started_at": row.lease.started_at.to_rfc3339(),
                    // BUG-763: the backing pid's OWN start time (null when
                    // unresolvable) — an adopted persistent lease has a pid
                    // younger than the lease record, and both ages matter.
                    "pid_started_at": row.pid_started_at.map(|t| t.to_rfc3339()),
                    "elapsed_secs": row.elapsed_secs,
                    "liveness": row.state.label(),
                    "live": matches!(row.state, LeaseState::Live),
                    // TASK-1090: dispatch-health — null for worktree-less
                    // advisory leases (no git state to classify).
                    "dispatch_state": row.dispatch.as_ref().map(|d| d.state.label()),
                    "dispatch_hint": row.dispatch.as_ref().and_then(|d| d.hint.clone()),
                    "worktree_dirty": row.dispatch.as_ref().map(|d| d.dirty),
                    "branch_ahead_of_main": row.dispatch.as_ref().map(|d| d.ahead_of_main),
                    // TASK-1143: the advisor holding the STORY-711 worktree lock
                    // on this row's worktree — null when unlocked (the common
                    // case; `[locking]` is opt-in).
                    "locked_by": row.locked_by,
                    // TASK-1451: null when no live pid backs the row (nothing
                    // to probe); otherwise "attributed" / "unattributed" /
                    // "unknown" — never collapsed to a boolean "fine".
                    "mail_identity": row.mail_identity.map(MailIdentityStatus::as_str),
                    // BUG-1553 / TASK-1454: "working" / "long_tool_call" /
                    // "suspended" / "unknown" / "blocked", null when no live
                    // pid backs the row (nothing to classify). "blocked" is
                    // the only value sourced from ground truth (the
                    // Notification hook marker) rather than a heuristic.
                    "activity": row.activity.as_ref().map(SeatActivity::label),
                    "activity_pending_tool": match &row.activity {
                        Some(SeatActivity::LongToolCall { tool, .. })
                        | Some(SeatActivity::Blocked { tool, .. }) => tool.clone(),
                        _ => None,
                    },
                    "activity_secs": match &row.activity {
                        Some(SeatActivity::LongToolCall { secs, .. })
                        | Some(SeatActivity::Blocked { secs, .. }) => Some(*secs),
                        _ => None,
                    },
                })
            })
            .collect();
        let orphaned: Vec<serde_json::Value> = orphans
            .iter()
            .map(|o| {
                serde_json::json!({
                    "spec": o.spec,
                    "title": o.title,
                    "liveness": if o.stale_lease { "stale" } else { "flag-only" },
                    // TASK-1064: a flag-only spec while a fan-out is live is most
                    // likely being built by it, not genuinely orphaned.
                    "likely_fanout": o.likely_fanout,
                    // trace:BUG-1656 | ai:claude
                    "possibly_subagent": o.possibly_subagent,
                    "live": false,
                    // BUG-1553: an orphan has no lease/worktree to probe a
                    // process against at all, so whether it's blocked or
                    // truly exited is never determinable here — say so
                    // explicitly rather than let "flag-only" imply "just
                    // not started". A stale (crashed) lease IS unambiguous
                    // (the process is confirmed gone), so it keeps its own
                    // "exited" reading instead of "unknown".
                    "activity": if o.stale_lease { "exited" } else { "unknown" },
                })
            })
            .collect();
        println!(
            "{}",
            crate::cache_output::json_pretty(&serde_json::json!({
                "sessions": sessions,
                "orphaned": orphaned,
                // STORY-769: last-human-input oracle, null when never stamped.
                "operator": operator,
                "wave": live_wave.as_ref().map(|l| serde_json::json!({
                    "id": if l.wave_id.is_empty() { format!("pid-{}", l.pid) } else { l.wave_id.clone() },
                    "pid": l.pid,
                    "binary_sha": l.binary_sha,
                    "binary_mtime_secs": l.binary_mtime_secs,
                    "binary_path": l.binary_path,
                })),
            }))?
        );
        return Ok(());
    }

    // Agent mode: token-efficient TOON, mirroring `aida integrate` / `awaiting`
    // so agents orienting via `aida ps` get flat scalars + uniform tables, not
    // the human column table. `--json` above still wins for structured
    // consumers; stale rows are hidden behind a count as in the human view.
    // trace:STORY-753 | ai:claude
    if agent_output_mode() {
        let (shown, hidden_stale): (Vec<&PsRow>, Vec<&PsRow>) = if all {
            (rows.iter().collect(), Vec::new())
        } else {
            rows.iter()
                .partition(|r| !matches!(r.state, LeaseState::Stale))
        };
        // TASK-1090: dead-and-dirty rows are the single highest-value signal
        // this report adds — never let them hide behind the stale-hidden
        // footer just because `--all` wasn't passed. Computed from the FULL
        // row set (not `shown`) so a machine consumer sees them regardless.
        let salvageable_hidden: Vec<&PsRow> = hidden_stale
            .iter()
            .filter(|r| {
                r.dispatch
                    .as_ref()
                    .is_some_and(|d| d.state == dispatch_health_ps::DispatchState::Salvageable)
            })
            .copied()
            .collect();

        println!("view: ps");
        // BUG-1521 (proxy decision): `running:` stays a bare integer — the
        // shape every other agent-mode view (`aida integrate`, `aida
        // awaiting`) uses — so a machine consumer can parse it without
        // branching on whether a parenthetical got appended. The hidden
        // count lives in its own `stale_hidden:` scalar right below, exactly
        // like `aida integrate`'s `running:`/`stale_hidden:` pair.
        println!("running: {}", shown.len());
        println!("stale_hidden: {}", hidden_stale.len());
        println!("orphaned: {}", orphans.len());
        if let Some(lock) = &live_wave {
            println!(
                "wave_id: {}",
                if lock.wave_id.is_empty() {
                    format!("pid-{}", lock.pid)
                } else {
                    lock.wave_id.clone()
                }
            );
            println!("wave_binary_sha: {}", lock.binary_sha);
            println!(
                "wave_binary_mtime_secs: {}",
                lock.binary_mtime_secs
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into())
            );
        }
        // STORY-769: last-human-input oracle as flat scalars for agent consumers.
        match operator_last_seen {
            Some(last) => {
                println!("operator_last_seen: {}", presence::since_label(last, now));
                println!(
                    "operator_presence: {}",
                    presence::human_presence(now, last, thresholds).word()
                );
            }
            None => {
                println!("operator_last_seen: unknown");
                println!("operator_presence: unknown");
            }
        }
        let collapsed_shown = collapse_salvageable_by_worktree(&shown);
        let run: Vec<Vec<String>> = collapsed_shown
            .iter()
            .map(|item| {
                let r = item.row;
                vec![
                    r.lease.id.clone(),
                    item.display_spec_with_count(),
                    r.role.clone().unwrap_or_else(|| "-".to_string()),
                    r.pid
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    humanize_duration_secs(r.elapsed_secs),
                    r.state.label().to_string(),
                    // TASK-1143: the worktree lock owner, blank when unlocked.
                    ps_locked_by_cell(r.locked_by.as_deref()),
                    // TASK-1090: dispatch-health state + the exact next
                    // command, blank for Moving / worktree-less rows.
                    r.dispatch
                        .as_ref()
                        .map(|d| d.state.label().to_string())
                        .unwrap_or_default(),
                    r.dispatch
                        .as_ref()
                        .and_then(|d| d.hint.clone())
                        .unwrap_or_default(),
                    // TASK-1451: blank when no live pid backs the row;
                    // otherwise "attributed" / "unattributed" / "unknown".
                    r.mail_identity
                        .map(MailIdentityStatus::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    // BUG-1553: blank when no live pid backs the row;
                    // otherwise "working" / "long_tool_call" / "suspended" /
                    // "unknown".
                    r.activity
                        .as_ref()
                        .map(SeatActivity::label)
                        .unwrap_or_default()
                        .to_string(),
                ]
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw(
                "running",
                &[
                    "session",
                    "spec",
                    "role",
                    "pid",
                    "elapsed",
                    "live",
                    "locked_by",
                    "dispatch_state",
                    "dispatch_hint",
                    "mail_identity",
                    "activity"
                ],
                &run
            )
        );
        let orph: Vec<Vec<String>> = orphans
            .iter()
            .map(|o| {
                vec![
                    o.spec.clone(),
                    o.title.clone(),
                    if o.stale_lease {
                        "stale".to_string()
                    } else {
                        "flag-only".to_string()
                    },
                    o.likely_fanout.to_string(),
                ]
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw(
                "orphaned",
                &["spec", "title", "liveness", "likely_fanout"],
                &orph
            )
        );
        // TASK-1090: always-shown (not gated by --all) — dead process +
        // uncommitted work hidden behind the stale-session footer.
        // BUG-1680: collapse duplicate rows for the same worktree path with a count.
        // trace:BUG-1680 | ai:antigravity
        let collapsed = collapse_salvageable_by_worktree(&salvageable_hidden);
        let salv: Vec<Vec<String>> = collapsed
            .iter()
            .map(|item| {
                let r = item.row;
                vec![
                    r.lease.id.clone(),
                    item.display_spec_with_count(),
                    r.lease.worktree_path.display().to_string(),
                    r.dispatch
                        .as_ref()
                        .and_then(|d| d.hint.clone())
                        .unwrap_or_default(),
                ]
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw(
                "salvageable",
                &["session", "spec", "worktree", "hint"],
                &salv
            )
        );
        return Ok(());
    }

    // Hide STALE session rows behind a footer count unless --all, mirroring
    // `aida session leases`. trace:STORY-696
    let (shown, hidden_stale): (Vec<&PsRow>, Vec<&PsRow>) = if all {
        (rows.iter().collect(), Vec::new())
    } else {
        rows.iter()
            .partition(|r| !matches!(r.state, LeaseState::Stale))
    };

    // TASK-965: stranded-primary alarm — loud banner ABOVE the running-work table
    // when the primary checkout is parked on a feature branch with in-flight
    // leases. trace:TASK-965 | ai:claude
    if let Some(stranded) = detect_stranded_primary(&project_root) {
        print_stranded_primary_banner(&stranded);
    }

    println!("{}", "Running work".bold());
    if let Some(lock) = &live_wave {
        let wave = if lock.wave_id.is_empty() {
            format!("pid-{}", lock.pid)
        } else {
            lock.wave_id.clone()
        };
        println!(
            "live wave {wave} · binary {} · mtime {}",
            truncate(&lock.binary_sha, 8),
            lock.binary_mtime_secs
                .and_then(|secs| {
                    std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs))
                })
                .map(|time| {
                    let local: chrono::DateTime<chrono::Local> = time.into();
                    local.format("%Y-%m-%d %H:%M:%S").to_string()
                })
                .unwrap_or_else(|| "?".into())
        );
    }
    // STORY-769: last-human-input oracle line — "operator last seen Nm ago —
    // active/idle/stale". Quiet ("unknown") until the per-turn notice stamps.
    match presence::last_seen_line(now, thresholds) {
        Some(line) => println!("{}", line.dimmed()),
        None => println!("{}", "operator last seen — unknown".dimmed()),
    }
    println!();

    if shown.is_empty() && hidden_stale.is_empty() {
        println!("{}", "(no active sessions)".dimmed());
    } else {
        let all_ids: Vec<&str> = rows.iter().map(|r| r.lease.id.as_str()).collect();
        // TASK-1168: size the spec + role columns to their widest cell (bounded)
        // instead of pre-truncating every cell to a fixed ~13 visible chars —
        // `harness-worktree` / `general-purpose` are short, bounded identifiers
        // and must render whole. trace:TASK-1168 | ai:claude
        let collapsed_shown = collapse_salvageable_by_worktree(&shown);
        let spec_cells: Vec<String> = collapsed_shown
            .iter()
            .map(|item| item.display_spec_with_count())
            .collect();
        let role_cells: Vec<String> = collapsed_shown
            .iter()
            .map(|item| item.row.role.clone().unwrap_or_else(|| "-".to_string()))
            .collect();
        let spec_w = ps_column_width(&spec_cells, PS_SPEC_MIN_WIDTH, PS_SPEC_MAX_WIDTH);
        let role_w = ps_column_width(&role_cells, PS_ROLE_MIN_WIDTH, PS_ROLE_MAX_WIDTH);
        // Column start offsets, so an overflow wrap continues under its column.
        let spec_offset = 11;
        let role_offset = spec_offset + spec_w + 1;
        // BUG-763: the started column is wide enough for a date-qualified
        // "Jun-26 11:55" — a non-today start always shows its date.
        let header = format!(
            "{:<10} {:<specw$} {:<rolew$} {:<8} {:<12} {:<11} {:<12} {}",
            "session",
            "spec",
            "role",
            "pid",
            "started",
            "elapsed",
            "locked-by",
            "live",
            specw = spec_w,
            rolew = role_w,
        );
        println!("{}", header.dimmed());
        for item in &collapsed_shown {
            let row = item.row;
            let l = &row.lease;
            let prefix_len = unique_prefix_len(&l.id, &all_ids, 8);
            let spec_col = item.display_spec_with_count();
            let pid_col = row.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
            // BUG-763: time-of-day for today's leases, "Jun-26 11:55" for
            // anything older — a June birth must never read as this morning
            // next to a 500h elapsed.
            let started = ps_started_cell(
                l.started_at.with_timezone(&chrono::Local).naive_local(),
                now.with_timezone(&chrono::Local).date_naive(),
            );
            let live_label = format!("{} {}", row.state.glyph(), row.state.label());
            // BUG-1553 (2026-09-23 PROXY DECISION): neither `LongToolCall`
            // nor `Suspended` overrides this cell's color/text — a purely
            // time-based or process-state heuristic cannot assert "blocked
            // on your approval", so the `live` cell keeps its ordinary
            // Live/Dormant/Stale coloring for those; the neutral/warning
            // activity note prints as an extra line below instead.
            // `SeatActivity::Blocked` is different: it is a REAL
            // Notification-hook marker, not a heuristic, so THIS is the
            // ground-truth case the proxy decision deferred — it DOES
            // override the cell, loudly, naming the state "Blocked"
            // instead of "Live" so it reads as visually distinct, not a
            // footnote (BUG-1553 AC1). trace:BUG-1553 trace:TASK-1454 | ai:claude
            let live_col = match &row.activity {
                Some(SeatActivity::Blocked { .. }) => {
                    format!("{} Blocked", crate::glyph(crate::glyphs::Glyph::Blocked)).red()
                }
                _ => match row.state {
                    LeaseState::Live => live_label.green(),
                    LeaseState::Dormant => live_label.cyan(),
                    LeaseState::Stale => live_label.yellow(),
                },
            };
            // TASK-1143: the worktree lock owner, blank when unlocked. Plain
            // text (paddable), so it slots into the fixed-width table before the
            // colored `live` column without breaking alignment.
            let locked_col = ps_locked_by_cell(row.locked_by.as_deref());
            // TASK-1168: an over-wide cell WRAPS onto an indented continuation
            // line (nothing is lost) instead of being ellipsized mid-word.
            let (spec_head, spec_rest) = ps_wrap_cell(&spec_col, spec_w);
            let (role_head, role_rest) = ps_wrap_cell(row.role.as_deref().unwrap_or("-"), role_w);
            println!(
                "{:<10} {:<specw$} {:<rolew$} {:<8} {:<12} {:<11} {:<12} {}",
                (&l.id[..prefix_len]).yellow(),
                spec_head,
                role_head,
                pid_col,
                started,
                humanize_duration_secs(row.elapsed_secs),
                truncate(&locked_col, 12),
                live_col,
                specw = spec_w,
                rolew = role_w,
            );
            if let Some(rest) = spec_rest {
                println!("{}{}", " ".repeat(spec_offset), rest.dimmed());
            }
            if let Some(rest) = role_rest {
                println!("{}{}", " ".repeat(role_offset), rest.dimmed());
            }
            // A second dimmed line carries the worktree so the wide path
            // doesn't blow out the table's column alignment.
            println!("{}{}", " ".repeat(11), l.worktree_path.display());
            // BUG-763: an adopted persistent lease — the backing pid started
            // later than the lease record — names both ages explicitly, so
            // lease-age vs process-age never mix silently in one row.
            if let Some(note) = ps_adopted_note(l.started_at, row.pid_started_at, now) {
                println!("{}{}", " ".repeat(11), note.dimmed());
            }
            // TASK-1090: a third line names the dispatch-health hint —
            // nothing printed for Moving (it's fine, per the acceptance).
            if let Some(d) = &row.dispatch {
                if let Some(hint) = &d.hint {
                    let (glyph, colored_label) = match d.state {
                        dispatch_health_ps::DispatchState::Salvageable => (
                            crate::glyph(crate::glyphs::Glyph::Warning),
                            d.state.label().red().bold(),
                        ),
                        dispatch_health_ps::DispatchState::Stalled => (
                            crate::glyph(crate::glyphs::Glyph::Warning),
                            d.state.label().yellow().bold(),
                        ),
                        // BUG-752: undeterminable liveness — informational,
                        // not an alarm (the agent may well be working).
                        // trace:BUG-752 | ai:claude
                        dispatch_health_ps::DispatchState::Unknown => (
                            crate::glyph(crate::glyphs::Glyph::Neutral),
                            d.state.label().dimmed(),
                        ),
                        // BUG-778: the operator entered this worktree by hand
                        // and hasn't launched an agent in it yet — an
                        // informational nudge (start one), never an alarm.
                        // trace:BUG-778 | ai:claude
                        dispatch_health_ps::DispatchState::AwaitingAgent => (
                            crate::glyph(crate::glyphs::Glyph::Info),
                            d.state.label().cyan(),
                        ),
                        // TASK-1518: the wave was stopped and the lease marked
                        // on the way out — informational (resume as normal),
                        // not the dead-agent alarm.
                        // trace:TASK-1518 | ai:claude
                        dispatch_health_ps::DispatchState::Stopped => (
                            crate::glyph(crate::glyphs::Glyph::Info),
                            d.state.label().cyan(),
                        ),
                        // BUG-1681: finished work waiting on the integrator —
                        // informational, and deliberately actionless.
                        // trace:BUG-1681 | ai:claude
                        dispatch_health_ps::DispatchState::AwaitingIntegration => (
                            crate::glyph(crate::glyphs::Glyph::Info),
                            d.state.label().cyan(),
                        ),
                        dispatch_health_ps::DispatchState::Moving => unreachable!(
                            "hint is None for Moving — see dispatch_health_ps::next_command_hint"
                        ),
                    };
                    println!(
                        "{}{} {}: {}",
                        " ".repeat(11),
                        glyph,
                        colored_label,
                        hint.dimmed()
                    );
                }
            }
            // BUG-1553 (2026-09-23 PROXY DECISION): a neutral informational
            // note for a long-outstanding tool call — a long Bash or Task
            // call is working, not blocked, so this is dim/neutral, never
            // red and never framed as "waiting on your approval". A
            // job-control-stopped process gets its own neutral-to-warning
            // "suspended" note. `Unknown` gets its own honest, quieter note
            // (say so rather than guess); silent for `Working` (nothing to
            // flag) and `None` (no live pid to probe at all).
            match &row.activity {
                Some(SeatActivity::LongToolCall { tool, secs }) => {
                    let what = tool.as_deref().unwrap_or("a tool call");
                    println!(
                        "{}{} {}: {what} ({})",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Neutral),
                        "long tool call".dimmed(),
                        humanize_duration_secs(*secs as u64),
                    );
                }
                Some(SeatActivity::Suspended) => {
                    println!(
                        "{}{} {}",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Warning),
                        "suspended (stopped process)".yellow(),
                    );
                }
                Some(SeatActivity::Unknown) => {
                    println!(
                        "{}{} {}: could not read this session's transcript — activity unknown",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Neutral),
                        "activity".dimmed()
                    );
                }
                // TASK-1454: ground truth from the Notification hook — loud
                // and named, since a human is the only one who can unblock
                // this seat right now (BUG-1553's whole motivation).
                Some(SeatActivity::Blocked { tool, secs }) => {
                    let what = tool.as_deref().unwrap_or("a tool call");
                    println!(
                        "{}{} {}: {what} — waiting {}",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Blocked),
                        "blocked on your approval".red().bold(),
                        humanize_duration_secs(*secs as u64),
                    );
                }
                Some(SeatActivity::Working) | None => {}
            }
            // TASK-1451: flag a live seat whose mail identity would fall
            // back to the shell user — visible BEFORE it sends unattributable
            // mail, not discovered after the fact in the envelope. Silent
            // for `Attributed` (nothing to flag) and for `None` (no live pid
            // to probe).
            match row.mail_identity {
                Some(MailIdentityStatus::Unattributed) => {
                    println!(
                        "{}{} {}: no AIDA_AGENT_NAME / AIDA_USER / AIDA_SESSION_ROLE in this process's environment — mail would go out unattributed",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Warning),
                        "mail identity".yellow().bold()
                    );
                }
                Some(MailIdentityStatus::Unknown) => {
                    println!(
                        "{}{} {}: could not read this process's environment — identity unknown",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Neutral),
                        "mail identity".dimmed()
                    );
                }
                Some(MailIdentityStatus::Attributed) | None => {}
            }
        }
        if !hidden_stale.is_empty() {
            println!();
            println!(
                "{}",
                format!(
                    "({} stale session{} hidden — pass --all to show)",
                    hidden_stale.len(),
                    if hidden_stale.len() == 1 { "" } else { "s" }
                )
                .dimmed()
            );
        }
    }

    // TASK-1090: dead process + uncommitted work is the single highest-value
    // signal this report adds — never let it hide silently behind the
    // stale-session footer just because `--all` wasn't passed. Always shown
    // (naturally empty once `--all` folds `hidden_stale` into the main
    // table, where its hint line already printed above).
    let salvageable_hidden: Vec<&PsRow> = hidden_stale
        .iter()
        .filter(|r| {
            r.dispatch
                .as_ref()
                .is_some_and(|d| d.state == dispatch_health_ps::DispatchState::Salvageable)
        })
        .copied()
        .collect();
    if !salvageable_hidden.is_empty() {
        let warn = crate::glyph(crate::glyphs::Glyph::Warning);
        println!();
        // BUG-1680: collapse duplicate rows for the same worktree path with a count.
        // trace:BUG-1680 | ai:antigravity
        let collapsed = collapse_salvageable_by_worktree(&salvageable_hidden);
        let work_desc = if collapsed.iter().all(|c| c.untracked_only) {
            "untracked files only"
        } else {
            "uncommitted work"
        };
        println!(
            "{}",
            format!(
                "Salvageable (dead process, {work_desc} — hidden behind the stale-session count above)"
            )
            .bold()
            .red()
        );
        for item in &collapsed {
            let row = item.row;
            let header = item.display_spec_with_count();
            println!("  {} {}", warn.red(), header.red().bold());
            println!(
                "      {}",
                row.lease.worktree_path.display().to_string().dimmed()
            );
            if let Some(hint) = row.dispatch.as_ref().and_then(|d| d.hint.as_deref()) {
                println!("      {}", hint.dimmed());
            }
        }
    }

    // TASK-1064: split the no-live-spec-lease specs into two honestly-framed
    // groups. A flag-only spec while a fan-out is live is most likely being
    // BUILT by that fan-out (which takes a generic non-spec-linked harness
    // lease) — informational, not alarming. Only the rest — crashed (stale)
    // spec-scoped leases, or flag-only with NO live worker — are genuine
    // orphans. trace:TASK-1064 | ai:claude
    let (fanout_worked, genuine): (Vec<&PsOrphan>, Vec<&PsOrphan>) =
        orphans.iter().partition(|o| o.likely_fanout);

    // Likely worked by a fan-out — informational (a live advisor Agent-tool
    // subagent is plausibly building these; its lease just isn't spec-linked).
    if !fanout_worked.is_empty() {
        let info = crate::glyph(crate::glyphs::Glyph::Info);
        println!();
        println!(
            "{}",
            "Likely worked by a fan-out (live advisor Agent-tool subagent — generic harness lease, spec not linked)"
                .bold()
                .cyan()
        );
        for o in &fanout_worked {
            println!(
                "  {} {}  {}",
                info.cyan(),
                o.spec.cyan().bold(),
                truncate(&o.title, 48).dimmed(),
            );
        }
    }

    // Orphaned In-Progress specs — the genuine flag-only / crashed-session case.
    if !genuine.is_empty() {
        let warn = crate::glyph(crate::glyphs::Glyph::Warning);
        println!();
        println!(
            "{}",
            "Orphaned In-Progress specs (no live session backing the flag)"
                .bold()
                .yellow()
        );
        for o in &genuine {
            // BUG-1553 (acceptance #3): a stale lease is unambiguous (the
            // process is confirmed gone). A pure flag-only spec has no
            // lease/worktree to probe a process against at all — whether
            // the (possibly still-working, possibly gone) seat behind it is
            // blocked or exited is genuinely undeterminable from here, so
            // say that plainly instead of a bare "flag-only" that reads as
            // "nothing has started". trace:BUG-1553 | ai:claude
            let why = if o.possibly_subagent {
                // trace:BUG-1656 | ai:claude
                "stale lease — recorded pid dead, but a live harness lease is in this repo: \
                 possibly worked by a subagent; verify before any cleanup"
            } else if o.stale_lease {
                "stale lease — process dead"
            } else {
                "flag-only — cannot determine whether this seat is blocked or exited"
            };
            println!(
                "  {} {}  {}  {}",
                warn.yellow(),
                o.spec.yellow().bold(),
                truncate(&o.title, 48).dimmed(),
                format!("({why})").dimmed(),
            );
        }
        println!(
            "{}",
            "  (advisor Agent-tool fan-outs take generic harness-worktree leases — \
             not spec-linked — so their specs read flag-only)"
                .dimmed()
        );
    }

    Ok(())
}

/// STORY-545: resolve a selector to `(ready, awaiting_signoff, parked)` via the
/// pickability gate + the STORY-546 queue gate. Shared by `burndown plan` (which
/// renders it) and `burndown run` (which preflights + drains it) so the two can
/// NEVER disagree on what's drainable. trace:STORY-527 trace:STORY-546 trace:STORY-545
#[allow(clippy::type_complexity)]
// trace:BUG-532 — the title map (display-id → title) is built from the SAME
// single store load the set resolution already does, so `burndown plan`'s text
// output can show id+title with no extra store scan (no N+1 lookup).
// (ready, awaiting_signoff, serialize_held, parked, supervised, titles)
// `supervised` (STORY-610): specs tagged `supervised` — signed off for KEYBOARD
// pickup but excluded from the unattended drain. Surfaced as its own section in
// `burndown plan` so the route per spec (`queue work` vs drain) is visible.
// `serialize_held` (TASK-149): queued + blessed specs a sibling's
// `serialize:<group>` claim deferred to a LATER wave. Kept out of
// `awaiting_signoff` — nothing human is pending on them.
pub(crate) type BurndownSets = (
    Vec<String>,
    Vec<String>,
    Vec<burndown::SerializeHold>,
    Vec<(String, String)>,
    Vec<String>,
    std::collections::HashMap<String, String>,
);

pub(crate) fn resolve_burndown_sets(
    status: &str,
    tag: Option<&str>,
    batch: Option<&str>,
    type_filter: Option<&str>,
) -> Result<BurndownSets> {
    // BUG-784: normalize an explicit `--type` to the same lowercased `Debug`
    // form the per-spec `req_type` carries, so `--type non-functional` and
    // `--type adr` both compare cleanly. With no `--type`, the selector applies
    // the default work-item type-class filter instead (knowledge-class records —
    // decision / vision / term / principle — are authored, not implemented, and
    // must never be offered as drain candidates). trace:BUG-784 | ai:claude
    let type_filter_norm: Option<String> = match type_filter {
        Some(raw) => Some(format!("{:?}", parse_requirement_type(raw)?).to_ascii_lowercase()),
        None => None,
    };
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;

    // Normalize a status label to an alphanumeric-only lowercase key so
    // "in-progress" / "In Progress" / "InProgress" all compare equal.
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let want_status = norm(status);
    let batch_tag = batch.map(|b| format!("batch:{}", b.to_ascii_lowercase()));

    // uuid → status, so a BlockedBy edge can be checked for satisfaction.
    let status_by_id: std::collections::HashMap<uuid::Uuid, aida_core::RequirementStatus> = store
        .requirements
        .iter()
        .map(|r| (r.id, r.status.clone()))
        .collect();

    // STORY-546: queue membership = advisor sign-off. Union of every user's
    // queue (matches `aida list --queued`). A spec is drainable only if the
    // advisor deliberately queued it.
    let queued_ids = all_queued_requirement_ids(&project_root);
    // TASK-1175: the queue-insertion timestamps that order the ready set.
    // trace:TASK-1175 | ai:claude
    let queued_added_at = all_queued_added_at(&project_root);

    let mut candidates: Vec<burndown::BurndownCandidate> = Vec::new();
    // Display-ids of queued specs in the selector scope, so the pickable set can
    // be split into the blessed (queued) drain set vs awaiting-sign-off.
    // trace:STORY-546
    let mut queued_disp: std::collections::HashSet<String> = std::collections::HashSet::new();
    // trace:BUG-532 — display-id → title, populated from this same scan so the
    // text plan output can show id+title without re-loading the store.
    let mut titles: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // STORY-610: supervised specs collected for the keyboard-pickup section.
    let mut supervised: Vec<String> = Vec::new();
    // TASK-1172: display-id → priority label, populated from this same scan so
    // the ready set can be ordered by priority without a second store read.
    let mut priority_by_id: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // TASK-1175: display-id → queue-entry `added_at`, joined here (the only
    // place display id and requirement UUID are both in hand) so the ordering
    // sort downstream stays a pure function of its arguments.
    // trace:TASK-1175 | ai:claude
    let mut added_at_by_id: std::collections::HashMap<String, chrono::DateTime<chrono::Utc>> =
        std::collections::HashMap::new();
    for req in &store.requirements {
        // TASK-803: the per-spec filter/bucket decision now lives in the pure,
        // unit-tested `burndown::classify_spec`. The caller computes the probed
        // inputs (display id, normalized status, blocker satisfaction, pending
        // decision) and the helper applies the load-bearing filter order
        // (archived → deferred → status → tag → batch → supervised → candidate).
        // Inputs are computed eagerly for every spec; behavior is identical to
        // the prior inline loop (the skipped specs just never reach the title /
        // queued / candidate collections). trace:TASK-803 | ai:claude
        let status_norm = norm(&req.status.to_string());
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        let disp = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        // A BlockedBy edge is unsatisfied unless its target is Completed; a
        // dangling target (not in the store) is treated as unsatisfied so we
        // never fan out a spec whose blocker we can't verify.
        let has_unsatisfied_blocker = req.relationships.iter().any(|rel| {
            matches!(rel.rel_type, aida_core::RelationshipType::BlockedBy)
                && status_by_id
                    .get(&rel.target_id)
                    .map(|s| *s != aida_core::RequirementStatus::Completed)
                    .unwrap_or(true)
        });
        let has_pending_decision = req
            .decision_request
            .as_ref()
            .map(|d| d.is_pending())
            .unwrap_or(false);
        let req_type = format!("{:?}", req.req_type).to_ascii_lowercase();
        let input = burndown::SpecClassifyInput {
            archived: req.archived,
            deferred: req.deferred,
            status_norm: &status_norm,
            want_status: &want_status,
            tags: &tags,
            tag_filter: tag,
            batch_tag: batch_tag.as_deref(),
            // trace:BUG-784 | ai:claude
            type_filter: type_filter_norm.as_deref(),
            disp: &disp,
            req_type: &req_type,
            has_unsatisfied_blocker,
            has_pending_decision,
            // trace:BUG-1717 | ai:claude
            execution_mode: req.execution_mode,
        };
        match burndown::classify_spec(&input) {
            burndown::SpecDisposition::Skip => {}
            burndown::SpecDisposition::Supervised => {
                titles.insert(disp.clone(), req.title.clone());
                supervised.push(disp);
            }
            burndown::SpecDisposition::Candidate(candidate) => {
                if queued_ids.contains(&req.id) {
                    queued_disp.insert(disp.clone());
                }
                titles.insert(disp.clone(), req.title.clone());
                // trace:TASK-1172 | ai:claude
                priority_by_id.insert(disp.clone(), req.priority.to_string());
                // trace:TASK-1175 | ai:claude
                if let Some(at) = queued_added_at.get(&req.id) {
                    added_at_by_id.insert(disp.clone(), *at);
                }
                candidates.push(candidate);
            }
        }
    }

    // STORY-614: id → serialize:<group> lookup, built from the same scan so the
    // wave-builder can substrate-enforce the serialize convention without a
    // re-read. trace:STORY-614
    let groups_by_id: std::collections::HashMap<String, Vec<String>> = candidates
        .iter()
        .map(|c| (c.id.clone(), burndown::serialize_groups(&c.tags)))
        .collect();

    let (pickable, parked) = burndown::partition(&candidates);
    // STORY-546: split the pickable set by advisor sign-off (queue membership).
    // `ready` = blessed + drainable; `awaiting_signoff` = pickable but the
    // advisor hasn't queued it yet (the `--candidates` curation list).
    let (mut ready, awaiting_signoff) = burndown::split_by_signoff(pickable, &queued_disp);

    // TASK-1172: order the wave BEFORE it is fanned, so a high-priority spec
    // queued after a low-priority one still drains first. Dependencies are
    // unaffected — a blocked spec never entered this set.
    //
    // TASK-1175: both orders are anchored on the queue-entry `added_at`.
    // `[burndown] order = queue` (or `--order queue`) drains strictly oldest-
    // queued first; the default priority order uses that same insertion time as
    // the tiebreak within a band. The sort is TOTAL (final tiebreak = display
    // id), which is why no pre-sort is needed here for the serialize claim
    // below to stay stable across runs.
    //
    // STORY-614: substrate-enforce the `serialize:<group>` convention — the
    // fan-out set must never carry more than one spec per group, so a drain that
    // ignores the skill text still can't co-fan a collision group. The
    // deterministic order above is what makes the "first claims the group" pick
    // reproducible.
    //
    // TASK-149: the held members are their OWN bucket, not part of
    // `awaiting_signoff`. They are queued and blessed — only a sibling's group
    // claim defers them to a later wave — so labelling them "awaiting advisor
    // sign-off" told both the drain runner and the operator that a human gate
    // was missing when nothing was pending.
    // trace:STORY-614 trace:TASK-149 trace:TASK-1172 trace:TASK-1175 | ai:claude
    let ready_order = resolved_burndown_ready_order(&project_root);
    ready = burndown::sort_ready(ready, &priority_by_id, &added_at_by_id, ready_order);

    let (kept_ready, serialize_held) = burndown::collapse_serialize_groups(ready, &groups_by_id);
    ready = kept_ready;
    supervised.sort();
    Ok((
        ready,
        awaiting_signoff,
        serialize_held,
        parked,
        supervised,
        titles,
    ))
}

/// STORY-527 slice 1: resolve a selector to the ready + parked sets via the
/// pure pickability gate. Read-only — the deterministic input the
/// `/aida-burndown` skill fans out.
///
/// STORY-546: the ready set now ALSO requires queue membership — queueing a
/// spec IS the advisor sign-off (`queue add` is advisor-authority-gated, ADR-3
/// / TASK-647), so burndown can never drain a spec the advisor didn't bless.
/// Pickable-but-unqueued specs surface as "awaiting sign-off", and
/// `--candidates` shows exactly that set (the advisor's "what to bless next"
/// aid). trace:STORY-527 trace:STORY-546 | ai:claude
pub(crate) fn handle_burndown_plan(
    status: &str,
    tag: Option<&str>,
    batch: Option<&str>,
    // trace:BUG-784 | ai:claude
    type_filter: Option<&str>,
    candidates_view: bool,
    json: bool,
) -> Result<()> {
    let (ready, awaiting_signoff, serialize_held, parked, supervised, titles) =
        resolve_burndown_sets(status, tag, batch, type_filter)?;

    // trace:BUG-532 — render the spec's (truncated) title beside its id so the
    // plan reads as a scannable decision surface, via the shared
    // `spec_title_cell` helper (also used by `aida human`, BUG-535).
    let title_cell = |id: &str| -> String { spec_title_cell(&titles, id) };

    // TASK-805: the running-drain overlay (live lock + leases) also feeds the
    // JSON payload, so machine consumers polling `--json` see the same
    // in-flight/scheduled partition the human view shows. Resolve the SHARED
    // main-worktree root — the drain writes its lock there (see
    // `handle_burndown_run` → `find_main_worktree_root`), so a sibling worktree
    // must read the orchestrator's lock, not its own. trace:TASK-805
    let overlay = find_main_worktree_root()
        .ok()
        .and_then(|r| DrainOverlay::probe(&r));
    let (in_flight_ready, scheduled_ready) = match &overlay {
        Some(o) => o.partition(&ready),
        None => (Vec::new(), ready.clone()),
    };

    if json {
        let drain = match &overlay {
            Some(o) => serde_json::json!({
                "running": true,
                "pid": o.pid,
                "in_flight": in_flight_ready,
                "scheduled": scheduled_ready,
            }),
            None => serde_json::json!({ "running": false }),
        };
        let payload = serde_json::json!({
            "selector": { "status": status, "tag": tag, "batch": batch },
            "ready": ready,
            "awaiting_signoff": awaiting_signoff,
            // trace:TASK-149 — its own key, NOT folded into awaiting_signoff:
            // these are queued + blessed, deferred to a later wave by a
            // sibling's serialize-group claim. Each entry names the claiming
            // spec + group so a consumer can explain the hold.
            "serialize_held": serialize_held
                .iter()
                .map(|h| serde_json::json!({
                    "spec": h.spec,
                    "held_by": h.held_by,
                    "group": h.group,
                }))
                .collect::<Vec<_>>(),
            "supervised": supervised,
            "parked": parked.iter().map(|(id, reason)| serde_json::json!({ "spec": id, "reason": reason })).collect::<Vec<_>>(),
            "drain": drain,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    // `--candidates`: the advisor's curation view — approved + pickable + NOT
    // yet queued. Read-only; the answer to "what could I bless next?".
    // trace:STORY-546
    if candidates_view {
        println!(
            "{} burndown candidates",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
        );
        println!(
            "  {}",
            burndown::selector_summary(status, tag, batch).dimmed()
        );
        println!(
            "  {} {} pickable + unqueued — bless with `aida queue add <id>` to make them drainable",
            "→".green(),
            awaiting_signoff.len()
        );
        if awaiting_signoff.is_empty() {
            println!(
                "\n{} Nothing to bless — every pickable {} spec is already queued (or none are pickable).",
                "·".dimmed(),
                status
            );
        } else {
            println!(
                "\n{}",
                "Candidates to bless (pickable, not yet queued):".bold()
            );
            for id in &awaiting_signoff {
                println!("  {} {}", "+".yellow(), id.cyan());
            }
        }
        return Ok(());
    }

    // Plain-language header: glosses "selector" so a new user understands what
    // is shown and how to narrow it (no bare jargon). trace:STORY-544
    println!(
        "{} burndown plan",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    println!(
        "  {}",
        burndown::selector_summary(status, tag, batch).dimmed()
    );
    // trace:TASK-149 — serialize-held is counted separately from awaiting
    // sign-off (and only mentioned when non-empty, so the common line is
    // unchanged): nothing human is pending on a held spec.
    let held_summary = if serialize_held.is_empty() {
        String::new()
    } else {
        format!(", {} held for a later wave", serialize_held.len())
    };
    println!(
        "  {} {} ready to fan out, {} awaiting sign-off, {} supervised, {} parked{}",
        "→".green(),
        ready.len(),
        awaiting_signoff.len(),
        supervised.len(),
        parked.len(),
        held_summary
    );

    // TASK-805: if a drain is actively running, surface it as a banner +
    // partition the Ready set into in-flight (leased) vs scheduled (claimed,
    // not yet picked up) so the plan reflects what the drain is mid-way through
    // rather than reading as a fresh, untouched ready set. The overlay +
    // partition were computed above (shared with the JSON path). trace:TASK-805
    if let Some(o) = &overlay {
        println!(
            "\n  {}",
            drain_running_banner(o.pid, &in_flight_ready, &scheduled_ready)
        );
    }

    if !ready.is_empty() {
        println!(
            "\n{}",
            "Ready (queued + bounded + unblocked + decision-free):".bold()
        );
        for id in &ready {
            // TASK-805: when a drain owns this spec, mark its state: in-flight
            // (an implementer is leased on it now) / ◷ scheduled (claimed, not
            // yet picked up). Falls back to the plain check mark when no drain is live.
            let (glyph, suffix) = match &overlay {
                Some(o) if o.in_flight.contains(id) => (
                    crate::glyph(crate::glyphs::Glyph::FlowActive).cyan(),
                    format!("  {}", "in-flight".cyan()),
                ),
                Some(_) => ("◷".yellow(), format!("  {}", "scheduled".yellow())),
                None => (
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    String::new(),
                ),
            };
            println!("  {} {}{}{}", glyph, id.cyan(), title_cell(id), suffix);
        }
    }
    // STORY-546: pickable but not blessed — show them so the advisor knows what
    // they could queue, but they are NOT in the drain set.
    // trace:BUG-499 — wording: queueing IS the advisor sign-off (role-gated via
    // TASK-647), not a casual human bypass; the hint must say so explicitly.
    if !awaiting_signoff.is_empty() {
        println!(
            "\n{}",
            "Awaiting advisor sign-off (mechanical gate passed; the advisor blesses these into the drain with `aida queue add <id>` — advisor authority required):".bold()
        );
        for id in &awaiting_signoff {
            println!("  {} {}{}", "+".yellow(), id, title_cell(id));
        }
    }
    // TASK-149: serialize-held — queued AND blessed, just not this wave. Its own
    // section (never the awaiting-sign-off list) so neither the drain runner nor
    // a reading operator concludes a human gate is missing; each row names the
    // spec that claimed the group. trace:TASK-149 | ai:claude
    if !serialize_held.is_empty() {
        println!(
            "\n{}",
            "Held for a later wave (queued + blessed — one spec per `serialize:<group>` drains at a time; no action needed):"
                .bold()
        );
        for h in &serialize_held {
            println!(
                "  {} {}{} — {}",
                "◷".yellow(),
                h.spec.cyan(),
                title_cell(&h.spec),
                format!("held behind {} in serialize group `{}`", h.held_by, h.group).dimmed()
            );
        }
    }
    // STORY-610: the supervised section — the structural answer to the recurring
    // "how do I make progress on these?" question. These specs are signed off
    // for keyboard pickup but excluded from the unattended drain; the route per
    // spec is `aida queue work <id>` (advisor-watched), NOT `burndown run`.
    if !supervised.is_empty() {
        println!(
            "\n{}",
            "Supervised — work these at the keyboard (excluded from the drain; `aida queue work <id>`):"
                .bold()
        );
        for id in &supervised {
            println!(
                "  {} {}{}",
                crate::glyph(crate::glyphs::Glyph::Arrow).magenta(),
                id.cyan(),
                title_cell(id)
            );
        }
    }
    if !parked.is_empty() {
        println!("\n{}", "Parked:".bold());
        for (id, reason) in &parked {
            // trace:BUG-532 — keep the parked annotation (`reason`, e.g.
            // "tagged `deferred:design`") AND add the title between id and reason.
            println!(
                "  {} {}{} — {}",
                "·".dimmed(),
                id,
                title_cell(id),
                reason.dimmed()
            );
        }
    }

    // Next-step footer (AIDA house style): point the user at the runner —
    // `aida burndown run` (STORY-545), with /aida-burndown as the in-Claude
    // alternative. Suppressed under --json (handled above) and when nothing is
    // ready.
    // trace:STORY-544 trace:BUG-494
    if ready.is_empty() {
        if !awaiting_signoff.is_empty() {
            println!(
                "\n{} Nothing blessed yet — queue a candidate above (`aida queue add <id>`) to make it drainable.",
                "·".dimmed()
            );
        } else if !supervised.is_empty() {
            // STORY-610: nothing to drain, but supervised work IS actionable —
            // route the operator to the keyboard path rather than a dead end.
            println!(
                "\n{} Nothing for the drain, but {} supervised spec(s) above are ready to work at the keyboard — `aida queue work <id>`.",
                "·".dimmed(),
                supervised.len()
            );
        } else {
            println!(
                "\n{} Nothing ready to fan out — adjust the selector above or unblock parked specs.",
                "·".dimmed()
            );
        }
    } else {
        println!("\n{}", burndown::next_step_footer().bold());
    }
    Ok(())
}

pub(crate) fn handle_release(
    patch: bool,
    minor: bool,
    major: bool,
    check: bool,
    after_pr: Option<u64>,
    skip_xplat_check: bool,
) -> Result<()> {
    let bump = resolve_release_bump(patch, minor, major).map_err(|e| anyhow::anyhow!(e))?;

    if check {
        let cur = current_version();
        let target = preview_next_version(cur, bump).unwrap_or_else(|| "?".to_string());
        let repo = std::env::current_dir().ok().and_then(|cwd| {
            find_aida_repo_above(&cwd).or_else(|| {
                std::env::var("AIDA_DEV_REPO")
                    .ok()
                    .map(std::path::PathBuf::from)
                    .filter(|p| is_aida_repo(p))
            })
        });
        println!(
            "{} would run a {} release: {} → {}",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
            bump.bold(),
            cur,
            target.green()
        );
        match &repo {
            Some(r) => {
                let branch = current_branch_at(r).unwrap_or_else(|| "?".to_string());
                let dirty = std::process::Command::new("git")
                    .arg("-C")
                    .arg(r)
                    .args(["status", "--porcelain"])
                    .output()
                    .ok()
                    .map(|o| !o.stdout.is_empty())
                    .unwrap_or(false);
                println!("  repo:    {}", r.display());
                println!("  branch:  {branch}");
                println!(
                    "  tree:    {}",
                    if dirty {
                        "DIRTY — release.sh requires a clean tree"
                            .yellow()
                            .to_string()
                    } else {
                        "clean".to_string()
                    }
                );
            }
            None => println!(
                "  {} not in an aida repo (cd into the checkout or set AIDA_DEV_REPO)",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            ),
        }
        if let Some(n) = after_pr {
            println!(
                "  after-pr: would wait for PR #{n}'s checks, merge it (--squash --delete-branch), and sync main FIRST"
            );
        }
        // ADR-72: no cross-platform pre-release gate; --skip-xplat-check is a
        // no-op kept for compatibility. trace:TASK-1723 | ai:claude
        println!("  steps:   1) aida-store pull  2) scripts/release.sh {bump} (tag + push)  3) wait for tarballs  4) upgrade sibling installs");
        println!("  cross-platform gate: none (Linux-only releases)");
        println!("\n  Re-run without --check to execute.");
        return Ok(());
    }

    // TASK-693: land an in-flight PR before releasing — wait for its checks,
    // merge it, and sync local main so scripts/release.sh tags a main that
    // actually includes the merged work. Refuse if the checks don't pass.
    // trace:STORY-472 | ai:claude
    if let Some(n) = after_pr {
        let repo = std::env::current_dir().ok().and_then(|cwd| {
            find_aida_repo_above(&cwd).or_else(|| {
                std::env::var("AIDA_DEV_REPO")
                    .ok()
                    .map(std::path::PathBuf::from)
                    .filter(|p| is_aida_repo(p))
            })
        });
        let merge_root = repo
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| {
                anyhow::anyhow!("could not resolve repository root for --after-pr merge")
            })?;
        let forge = crate::forge::forge_for(&merge_root);
        let mut metadata_sink = crate::network_retry::NoopSink;
        let metadata = forge.change_metadata(n, &mut metadata_sink).ok();
        let change_ref = crate::forge::ChangeRef {
            id: n,
            url: String::new(),
            branch: metadata
                .as_ref()
                .map(|m| m.head_ref.clone())
                .unwrap_or_default(),
            base: metadata
                .as_ref()
                .map(|m| m.base_ref.clone())
                .unwrap_or_default(),
            title: metadata.as_ref().map(|m| m.title.clone()),
        };
        let change_noun = crate::forge::resolve_forge_kind(&merge_root).change_noun();

        println!(
            "{} waiting for {change_noun} #{n}'s checks…",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
        );
        match forge.watch_ci(&change_ref) {
            Ok(crate::forge::CiState::Success | crate::forge::CiState::None) => println!(
                "  {} {change_noun} #{n} checks passed",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            ),
            Ok(state) => anyhow::bail!(
                "{change_noun} #{n}'s checks did not pass ({state:?}) — not merging or releasing."
            ),
            Err(e) => anyhow::bail!(
                "could not watch {change_noun} #{n}'s checks via forge provider: {e:#}"
            ),
        }

        println!(
            "{} merging {change_noun} #{n}…",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
        );
        let merge_opts = crate::forge::MergeOptions {
            method: crate::forge::MergeMethod::Squash,
            squash_subject: None,
            squash_body: None, // trace:TASK-1330 | ai:claude
            delete_branch: true,
            match_head: None,
        };
        let mut sink = crate::network_retry::StderrSink;
        if let Err(e) =
            crate::forge::forge_for(&merge_root).merge_change(&change_ref, &merge_opts, &mut sink)
        {
            let hint = crate::forge::resolve_forge_kind(&merge_root)
                .merge_cmd(&n.to_string())
                .unwrap_or_else(|| format!("merge change {n}"));
            anyhow::bail!(
                "`{hint}` failed ({e:#}) — merge it manually, then re-run `aida release` without --after-pr."
            );
        }
        println!(
            "  {} {change_noun} #{n} merged",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );

        // Sync local main so release.sh tags the merged commit.
        if let Some(r) = &repo {
            println!(
                "{} syncing local main…",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
            );
            let pull = std::process::Command::new("git")
                .arg("-C")
                .arg(r)
                .args(["pull", "--ff-only"])
                .status();
            if !matches!(pull, Ok(s) if s.success()) {
                anyhow::bail!(
                    "`git pull --ff-only` failed after merging PR #{n} — sync main manually, then re-run `aida release` without --after-pr."
                );
            }
        }
    }

    if skip_xplat_check {
        // scripts/release.sh / pre-release-check.sh honor this env. trace:STORY-472
        std::env::set_var("AIDA_SKIP_XPLAT_CHECK", "1");
    }
    handle_dev_release(bump)
}

#[cfg(test)]
#[path = "tests/release_verb_tests.rs"]
mod release_verb_tests;

/// Stream a child's stdout/stderr to the parent's stderr with a prefix.
/// `aida dev release [bump]` — the one-command release flow:
/// 1. run scripts/release.sh (bumps version, tags, pushes, interactive)
/// 2. wait for the GitHub Actions workflow to publish the binary tarballs
///    (HEAD-poll the asset URL with timeout)
/// 3. upgrade sibling installs to the new version (auto-yes)
///
/// `aida dev patch` is a thin alias that calls this with bump = "patch".
/// trace:EPIC-1-001 | ai:claude
// trace:FR-2-004 | ai:claude
pub(crate) fn handle_dev_release(bump: &str) -> Result<()> {
    // Locate the aida repo. Prefer PWD walk, then $AIDA_DEV_REPO.
    let cwd = std::env::current_dir()?;
    let repo = find_aida_repo_above(&cwd)
        .or_else(|| {
            std::env::var("AIDA_DEV_REPO")
                .ok()
                .map(std::path::PathBuf::from)
                .filter(|p| is_aida_repo(p))
        })
        .ok_or_else(|| {
            // trace:TASK-667 — wrapper-correct activate form.
            anyhow::anyhow!(
                "Not in an aida repo. cd into the aida checkout, set AIDA_DEV_REPO, \
                 or run `{}` first.",
                eval_subcommand_hint("dev activate")
            )
        })?;

    let script = repo.join("scripts/release.sh");
    if !script.is_file() {
        anyhow::bail!(
            "scripts/release.sh not found at {} — is this checkout up to date?",
            script.display()
        );
    }

    // Detect if this repo has a git-canonical aida store to sync.
    let store_path = detect_store_path(&repo);

    // ── Step 1/5: sync store (pull) ──────────────────────────────────────
    if let Some(ref sp) = store_path {
        println!("{}", "─── Step 1/5: syncing aida-store (pull) ───".bold());
        let branch =
            aida_core::git_ops::current_branch(sp).unwrap_or_else(|_| "aida-store".to_string());
        // Use rebase: bare `git pull` fails on divergent branches when
        // the user has no pull.rebase/pull.ff config; the orphan-store
        // model wants linear history. trace:BUG-1-051 | ai:claude
        match aida_core::git_ops::pull_rebase(sp, "origin", &branch) {
            Ok(()) => println!("  Store pull complete."),
            Err(e) => {
                anyhow::bail!(
                    "aida-store pull failed: {}\n\
                     Resolve store conflicts first: aida db sync --pull\n\
                     Then re-run `aida dev release {}`.",
                    e,
                    bump
                );
            }
        }
        println!();
    }

    // STORY-127 detector (3): warn (don't block) when there are open PRs the
    // release tag will not include. A single batched `gh pr list` snapshot.
    // trace:STORY-127 | ai:claude
    {
        let open_numbers: Vec<u64> = collect_open_prs(&repo)
            .by_branch
            .values()
            .map(|p| p.number)
            .collect();
        if let Some(msg) = release_unmerged_pr_warning(&open_numbers) {
            eprintln!("  {} {}", "Warning:".yellow().bold(), msg);
            println!();
        }
    }

    // ── Step 2/5: run release.sh ─────────────────────────────────────────
    println!(
        "{}",
        format!("─── Step 2/5: ./scripts/release.sh {} ───", bump).bold()
    );
    println!("Working in {}", repo.display());
    println!();

    // Run release.sh interactively — it prints the version-bump diff and
    // asks for confirmation. Inheriting stdio means the user sees and
    // responds to the prompts directly.
    let status = std::process::Command::new(&script)
        .arg(bump)
        .current_dir(&repo)
        .status_retrying_etxtbsy()
        .with_context(|| format!("Failed to invoke {}", script.display()))?;
    if !status.success() {
        anyhow::bail!(
            "release.sh exited non-zero (likely cancelled at the confirmation \
             prompt). Aborting upgrade phase — your tree may have a pending \
             version bump; resolve manually if needed."
        );
    }

    // After release.sh completes, the new tag is the latest one in the repo.
    let new_tag = git_describe_latest_tag(&repo).ok_or_else(|| {
        anyhow::anyhow!("release.sh succeeded but no tag is reachable — confused state, please check `git tag --list` manually.")
    })?;

    // ── Step 3/5: wait for GitHub release artifacts ───────────────────────
    println!();
    println!(
        "{}",
        format!(
            "─── Step 3/5: waiting for {} release artifacts ───",
            new_tag
        )
        .bold()
    );
    let target = release_target().ok_or_else(|| {
        anyhow::anyhow!(
            "Unsupported platform — can't auto-poll for tarballs. Wait for the \
             release.yml workflow to finish and run `aida upgrade` manually."
        )
    })?;
    let asset_url = format!(
        "https://github.com/joemooney/aida/releases/download/{}/aida-{}.tar.gz",
        new_tag, target
    );
    println!("Polling {} ...", asset_url);
    poll_until_published(&asset_url, std::time::Duration::from_secs(600))?;

    // ── Step 4/5: upgrade sibling installs ───────────────────────────────
    println!();
    println!(
        "{}",
        format!(
            "─── Step 4/5: upgrading sibling installs to {} ───",
            new_tag
        )
        .bold()
    );
    let bare_version = strip_v(&new_tag).to_string();
    upgrade_cmd::upgrade_dev_mode_sibling_scan(false, Some(&bare_version), true, false)?;

    // ── Step 5/5: sync store (push) ───────────────────────────────────────
    if let Some(ref sp) = store_path {
        println!();
        println!(
            "{}",
            format!(
                "─── Step 5/5: syncing aida-store (push) for {} ───",
                new_tag
            )
            .bold()
        );
        let branch =
            aida_core::git_ops::current_branch(sp).unwrap_or_else(|_| "aida-store".to_string());

        // Commit any pending store changes (e.g., block pointer updates)
        if aida_core::git_ops::has_changes(sp).unwrap_or(false) {
            let msg = format!("chore: sync store for release {}", new_tag);
            let _ = aida_core::git_ops::add_all(sp, "objects");
            let _ = aida_core::git_ops::add_all(sp, "registry");
            if sp.join("metadata.yaml").exists() {
                let _ = aida_core::git_ops::add(sp, &["metadata.yaml"]);
            }
            if let Err(e) = aida_core::git_ops::commit(sp, &msg) {
                eprintln!("  Warning: could not commit store changes: {}", e);
            } else {
                println!("  Committed store changes: {}", msg);
            }
        }

        match aida_core::git_ops::push(sp, "origin", &branch) {
            Ok(true) => println!("  Store push complete."),
            Ok(false) => {
                println!("  Push rejected. Pulling and retrying...");
                if let Err(e) = aida_core::git_ops::pull_rebase(sp, "origin", &branch) {
                    eprintln!("  Warning: store pull-rebase failed: {}", e);
                } else {
                    match aida_core::git_ops::push(sp, "origin", &branch) {
                        Ok(_) => println!("  Store push complete after rebase."),
                        Err(e) => eprintln!("  Warning: store push failed after rebase: {}", e),
                    }
                }
            }
            Err(e) => eprintln!("  Warning: store push failed: {}", e),
        }
    }

    println!();
    println!(
        "{}: shipped {} and refreshed sibling installs.",
        "DONE".green().bold(),
        new_tag
    );
    Ok(())
}

// ----------------------------------------------------------------------------
// `aida usage` — inspect locally-recorded CLI invocation telemetry.
// trace:STORY-122 | ai:claude
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// `aida metrics agent-lift` — dogfood agent-lift report (STORY-477).
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// `aida push` — push code AND the orphan store in one shot.
// trace:FR-264 | ai:claude
// ----------------------------------------------------------------------------

// trace:BUG-1796 | ai:codex
pub(crate) fn pull_skip_warning(message: &str) -> String {
    format!("{} {message}", "Warning:".yellow().bold())
}

pub(crate) fn pull_code_start_line(branch: &str) -> String {
    format!("{} {} ← origin", "Pulling code".cyan().bold(), branch)
}

pub(crate) fn pull_store_start_line() -> String {
    format!("{} aida-store ← origin", "Pulling store".cyan().bold())
}

/// TASK-75: a status transition extracted from a commit's YAML diff,
/// annotated with where it came from. trace:TASK-75 | ai:claude
pub(crate) struct StatusTransition {
    pub(crate) spec_id: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) source: String,
}

/// TASK-75: classify a commit subject as a "PR-N merge" (squash via
/// `(#N)` suffix), an explicit commit referencing a REQ-ID, or "manual".
/// trace:TASK-75 | ai:claude
pub(crate) fn classify_commit_source(subject: &str) -> String {
    let trimmed = subject.trim();
    // PR-N merge: trailing `(#N)`.
    if let Some(open) = trimmed.rfind('(') {
        let inner = &trimmed[open + 1..];
        if let Some(close) = inner.find(')') {
            let body = &inner[..close];
            if let Some(n) = body.strip_prefix('#') {
                if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
                    return format!("PR-{} merge", n);
                }
            }
        }
    }
    // commit-with-REQ-ID: subject extractor finds at least one SPEC-ID
    // in the trailing parens.
    let ids = extract_spec_ids_from_commit(subject);
    if !ids.is_empty() {
        return format!("commit ({})", ids.join(", "));
    }
    "manual".to_string()
}

/// TASK-75: parse each commit's diff to find status transitions in the
/// modified YAML files. Returns one StatusTransition per (commit, spec)
/// pair that actually changed status. trace:TASK-75 | ai:claude
pub(crate) fn extract_status_changes_from_commits(
    store_path: &std::path::Path,
    commits: &[(String, String, String)],
    modified_specs_per_commit: &std::collections::BTreeMap<String, Vec<String>>,
) -> Vec<StatusTransition> {
    use std::process::Command as ProcessCommand;
    let mut out = Vec::new();
    for (sha, _subj, source) in commits {
        let Some(specs) = modified_specs_per_commit.get(sha) else {
            continue;
        };
        if specs.is_empty() {
            continue;
        }
        let diff = match ProcessCommand::new("git")
            .arg("-C")
            .arg(store_path)
            .args(["show", "--no-color", "--unified=0", sha, "--"])
            .args(specs.iter().map(|s| format!("objects/*/{}.yaml", s)))
            .output()
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => continue,
        };
        for tr in parse_status_transitions_from_diff(&diff) {
            out.push(StatusTransition {
                spec_id: tr.0,
                from: tr.1,
                to: tr.2,
                source: source.clone(),
            });
        }
    }
    out
}

/// TASK-75: scan unified-diff output for status transitions. Each YAML
/// status change appears as a `-status: X` / `+status: Y` pair within a
/// single file's hunk. We walk file-by-file: track which spec the hunk
/// belongs to (via `diff --git a/objects/.../X.yaml`), then pair the
/// `-status` and `+status` lines. trace:TASK-75 | ai:claude
pub(crate) fn parse_status_transitions_from_diff(diff: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut current_spec: Option<String> = None;
    let mut prev_status: Option<String> = None;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            // rest: `objects/TYPE/000/SPEC-ID.yaml b/objects/...`
            current_spec = rest
                .split_whitespace()
                .next()
                .and_then(|p| spec_id_from_orphan_path(p).into())
                .filter(|s| s != "?");
            prev_status = None;
            continue;
        }
        let Some(spec) = current_spec.as_deref() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("-status:") {
            prev_status = Some(rest.trim().trim_matches('"').to_string());
        } else if let Some(rest) = line.strip_prefix("+status:") {
            let to = rest.trim().trim_matches('"').to_string();
            if let Some(from) = prev_status.take() {
                if from != to {
                    out.push((spec.to_string(), from, to));
                }
            }
        }
    }
    out
}

pub(crate) fn spec_id_from_orphan_path(path: &str) -> String {
    // objects/TYPE/000/SPEC-ID.yaml
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}

pub(crate) fn req_type_from_orphan_path(path: &str) -> String {
    path.split('/').nth(1).unwrap_or("?").to_string()
}

#[cfg(test)]
#[path = "tests/pull_summary_status_change_tests.rs"]
mod pull_summary_status_change_tests;

#[cfg(test)]
#[path = "tests/queue_work_tests.rs"]
mod queue_work_tests;

#[cfg(test)]
#[path = "tests/bug_1607_reviewer_vendor_tests.rs"]
mod bug_1607_reviewer_vendor_tests;

// trace:BUG-1686 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1686_agy_headless_argv_tests.rs"]
mod bug_1686_agy_headless_argv_tests;

#[cfg(test)]
#[path = "tests/bug_1609_gitlab_reviewer_preflight_tests.rs"]
mod bug_1609_gitlab_reviewer_preflight_tests;

#[cfg(test)]
#[path = "tests/queue_rework_tests.rs"]
mod queue_rework_tests;

// trace:BUG-1632 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1632_status_writer_tests.rs"]
mod bug_1632_status_writer_tests;

// BUG-1637: every status writer records its transition through the one shared
// helper with the right author class. trace:BUG-1637 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1637_status_writer_tests.rs"]
mod bug_1637_status_writer_tests;

// BUG-1638: race seams for the self-reading status writers, the phase-1
// restore outcome, zen's deleted-spec message, and the atomic findings
// promote. trace:BUG-1638 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1638_race_seam_tests.rs"]
mod bug_1638_race_seam_tests;

// BUG-1647: atomic findings dismiss and the findings promote edge cases.
// trace:BUG-1647 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1647_findings_atomic_tests.rs"]
mod bug_1647_findings_atomic_tests;

// BUG-1651: compare-and-swap queue rollback on a failed findings promote.
// trace:BUG-1651 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1651_promote_queue_cas_tests.rs"]
mod bug_1651_promote_queue_cas_tests;

// Shared stale-cache fixture for the BUG-1664 / BUG-1670 / BUG-1606 suites.
// trace:TASK-1526 | ai:claude
#[cfg(test)]
#[path = "tests/stale_cache_fixture.rs"]
mod stale_cache_fixture;

// trace:BUG-1664 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1664_stale_sweep_tests.rs"]
mod bug_1664_stale_sweep_tests;

// BUG-1671: the sweeps re-check each candidate INSIDE the store write lock, so
// a reopen landing between the re-check and the write is not overwritten.
// trace:BUG-1671 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1671_sweep_lock_window_tests.rs"]
mod bug_1671_sweep_lock_window_tests;

// BUG-1672: `aida human` reviews-awaiting must not list already-approved
// specs. trace:BUG-1672 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1672_reviews_awaiting_approved_tests.rs"]
mod bug_1672_reviews_awaiting_approved_tests;

// BUG-1633: pull/push follow-ups to BUG-1625 and BUG-1626. trace:BUG-1633 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1633_pull_push_followups_tests.rs"]
mod bug_1633_pull_push_followups_tests;

#[cfg(test)]
#[path = "tests/eval_subcommand_hint_tests.rs"]
mod eval_subcommand_hint_tests;

#[cfg(test)]
#[path = "tests/worktree_enter_wrapper_staleness_tests.rs"]
mod worktree_enter_wrapper_staleness_tests;

#[cfg(test)]
#[path = "tests/epic_agent_new_refusal_tests.rs"]
mod epic_agent_new_refusal_tests;

#[cfg(test)]
#[path = "tests/binary_selection_tests.rs"]
mod binary_selection_tests;

#[cfg(test)]
#[path = "tests/ci_action_tests.rs"]
mod ci_action_tests;

// ----------------------------------------------------------------------------
// `aida status` — comprehensive project overview, with extra sections when
// the current project is the aida repo itself.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

/// Distributed-mode status: read from CachedGitBackend so we get cache-backed
/// counts, sync state, and recent activity.
/// TASK-220: gathered facts for the unified `aida status` view. Each
/// optional field's `None` means "section absent" (no session covering
/// cwd, no PR open, gh missing) — call sites graceful-degrade per
/// section without bailing on the whole command. trace:TASK-220
#[derive(Debug)]
pub(crate) struct UserStatusContext {
    pub(crate) session: Option<SessionLease>,
    pub(crate) role: Option<String>,
    pub(crate) branch: Option<BranchFacts>,
    pub(crate) pr: Option<PrFacts>,
    pub(crate) queue_head: Vec<QueueRow>,
    pub(crate) queue_total: usize,
    pub(crate) agents: Vec<agent_registry::AgentRegistryView>,
}

#[derive(Debug, Clone)]
pub(crate) struct BranchFacts {
    pub(crate) name: String,
    pub(crate) dirty: bool,
    pub(crate) ahead_main: Option<u32>,
    pub(crate) behind_main: Option<u32>,
    pub(crate) ahead_upstream: Option<u32>,
    pub(crate) behind_upstream: Option<u32>,
    pub(crate) has_upstream: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct PrFacts {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) state: String,
    pub(crate) ci_rollup: Option<String>,
    pub(crate) gh_status: GhStatus,
}

#[derive(Debug, Clone)]
pub(crate) enum GhStatus {
    Ok,
    Missing,
    Failed(String),
    Skipped,
    /// BUG-560: the remote isn't GitHub, so `gh` is the wrong tool — we never
    /// spawned it. Carries the detected forge so the section can name the right
    /// CLI (`glab`) / degrade cleanly instead of leaking gh's "known GitHub
    /// host" auth error to a GitLab/pure-git user. trace:BUG-560 | ai:claude
    NotGitHub(forge::ForgeKind),
}

#[derive(Debug, Clone)]
pub(crate) struct QueueRow {
    pub(crate) spec_id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) for_role: Option<String>,
    pub(crate) in_progress: bool,
    pub(crate) lease_id: Option<String>,
    pub(crate) lease_started_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub(crate) fn collect_user_context(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
    backend: &aida_core::CachedGitBackend,
    no_ci: bool,
) -> UserStatusContext {
    let leases = list_leases(project_root);
    let session = std::env::current_dir().ok().and_then(|cwd| {
        let canon = cwd.canonicalize().unwrap_or(cwd);
        leases
            .iter()
            .find(|&l| lease_covers_cwd(l, &canon))
            .cloned()
    });
    let role = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| session.as_ref().and_then(|l| l.role.clone()));

    let branch = collect_branch_facts(project_root);

    let pr = if no_ci {
        Some(PrFacts {
            number: 0,
            title: String::new(),
            url: String::new(),
            state: String::new(),
            ci_rollup: None,
            gh_status: GhStatus::Skipped,
        })
    } else {
        branch
            .as_ref()
            .map(|b| collect_pr_facts(project_root, &b.name))
    };

    let (queue_head, queue_total) =
        collect_queue_snapshot(backend, store, role.as_deref(), &leases);
    let agent_ctx = build_agent_classify_context(project_root, &leases);
    let registry_agents = agent_registry::list_agent_views(project_root, &agent_ctx);
    let agents =
        merge_agent_views_with_lease_fallback(project_root, &leases, registry_agents, &agent_ctx);

    UserStatusContext {
        session,
        role,
        branch,
        pr,
        queue_head,
        queue_total,
        agents,
    }
}

/// STORY-435: assemble the `AgentClassifyContext` `list_agent_views` needs
/// to compute busy/idle. The live-lease snapshot reuses the same
/// `probe_live_claude_sessions` that `aida session leases` shows under the
/// ● live glyph so the two views agree about which scopes are "being
/// worked." trace:STORY-435 | ai:claude
pub(crate) fn build_agent_classify_context(
    project_root: &std::path::Path,
    leases: &[SessionLease],
) -> agent_registry::AgentClassifyContext {
    let now = chrono::Utc::now();
    let cfg = agent_registry::Config::load(project_root);
    // BUG-613/TASK-1061: ONE memoized live-session probe drives liveness for
    // every lease — the mapping below is a pure fold over this single snapshot,
    // never an O(leases) fan-out of separate `/proc` probes. trace:TASK-1061
    let live_sessions = process_probe::probe_live_claude_sessions();
    let live_lease_worktrees = live_lease_worktrees(now, leases, &live_sessions);
    agent_registry::AgentClassifyContext::new(now, cfg.busy_threshold_secs, live_lease_worktrees)
        .with_work_probe(
            aida_core::liveness::probe_process_tree(),
            cfg.work_grace_secs,
        )
}

/// TASK-1061: compute the set of live-lease worktree paths from a SINGLE shared
/// `live_sessions` snapshot. Pulled out of `build_agent_classify_context` as a
/// pure fold over the already-probed slice so the live-session `/proc` probe
/// stays bounded to the one memoized walk (BUG-613) regardless of how many
/// leases are open — adding leases never adds probes. Taking `live_sessions` by
/// reference is the structural guarantee: this function CANNOT re-probe per
/// lease.
// trace:TASK-1061
pub(crate) fn live_lease_worktrees(
    now: chrono::DateTime<chrono::Utc>,
    leases: &[SessionLease],
    live_sessions: &[process_probe::LiveSession],
) -> Vec<std::path::PathBuf> {
    leases
        .iter()
        .filter_map(|l| {
            if l.worktree_path.as_os_str().is_empty() {
                return None;
            }
            let worktree_exists = l.worktree_path.exists();
            let live_in_worktree = live_sessions.iter().any(|s| {
                !s.stale_cwd && (s.cwd == l.worktree_path || s.cwd.starts_with(&l.worktree_path))
            });
            let age_hours = now.signed_duration_since(l.started_at).num_hours();
            match classify_lease_state(worktree_exists, live_in_worktree, age_hours) {
                LeaseState::Live => Some(l.worktree_path.clone()),
                _ => None,
            }
        })
        .collect()
}

// TASK-1061: the live-session `/proc` probe must stay bounded — one memoized
// walk drives liveness for EVERY lease, never an O(leases) fan-out of separate
// probes. `live_lease_worktrees` enforces this structurally by taking the
// already-probed `live_sessions` slice; these tests lock that contract: N
// leases are classified entirely from a SINGLE injected snapshot, with zero
// probes performed by the function itself. trace:TASK-1061
#[cfg(test)]
#[path = "tests/live_lease_worktrees_tests.rs"]
mod live_lease_worktrees_tests;

/// TASK-515: launcher/MCP registry rows are richer and win. Raw agent launches
/// still create session leases, though, so synthesize lease-backed rows for any
/// active lease not already represented by registry metadata.
pub(crate) fn merge_agent_views_with_lease_fallback(
    project_root: &std::path::Path,
    leases: &[SessionLease],
    mut registry_agents: Vec<agent_registry::AgentRegistryView>,
    ctx: &agent_registry::AgentClassifyContext,
) -> Vec<agent_registry::AgentRegistryView> {
    let live_sessions = process_probe::probe_live_claude_sessions();
    for lease in leases {
        if registry_agents
            .iter()
            .any(|agent| agent_view_covers_lease(agent, lease))
        {
            continue;
        }
        registry_agents.push(lease_agent_view(project_root, lease, ctx, &live_sessions));
    }
    registry_agents.sort_by(|a, b| {
        a.agent_type
            .cmp(&b.agent_type)
            .then_with(|| a.current_spec.cmp(&b.current_spec))
            .then_with(|| a.id.cmp(&b.id))
    });
    registry_agents
}

pub(crate) fn agent_view_covers_lease(
    agent: &agent_registry::AgentRegistryView,
    lease: &SessionLease,
) -> bool {
    let same_scope = agent
        .current_spec
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case(&lease.scope))
        .unwrap_or(false);
    let same_worktree = !lease.worktree_path.as_os_str().is_empty()
        && (agent.worktree_path == lease.worktree_path
            || agent.worktree_path.starts_with(&lease.worktree_path)
            || lease.worktree_path.starts_with(&agent.worktree_path));
    same_scope || same_worktree
}

pub(crate) fn lease_agent_view(
    project_root: &std::path::Path,
    lease: &SessionLease,
    ctx: &agent_registry::AgentClassifyContext,
    live_sessions: &[process_probe::LiveSession],
) -> agent_registry::AgentRegistryView {
    // BUG-511: review-verb leases classify by creator PID, not worktree.
    let lease_state = lease_state_for(lease, live_sessions, ctx.now);
    let status = match lease_state {
        LeaseState::Stale => agent_registry::AgentStatus::Stale,
        LeaseState::Live | LeaseState::Dormant => agent_registry::AgentStatus::Busy,
    };

    agent_registry::AgentRegistryView {
        id: format!("lease-{}", lease.id),
        agent_type: lease_agent_type(lease),
        pid: lease.creator_pid.unwrap_or(0),
        name: None,
        description: None,
        tty: None,
        terminal: None,
        started_at: lease.started_at,
        last_active_at: lease_activity_timestamp(project_root, lease).unwrap_or(lease.started_at),
        role: lease.role.clone(),
        current_spec: Some(lease.scope.clone()),
        worktree_path: lease.worktree_path.clone(),
        source: "lease".to_string(),
        binary_version: None,
        build_sha: None,
        status,
        // STORY-528: lease-derived views have no availability state of their
        // own — only registry-backed agents can be paused.
        availability: agent_registry::Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        ended_at: None,
        resumed_from: None,
    }
}

pub(crate) fn lease_agent_type(lease: &SessionLease) -> String {
    let env_path = lease.worktree_path.join(".aida").join("session-env.sh");
    std::fs::read_to_string(&env_path)
        .ok()
        .and_then(|body| {
            parse_session_env(&body)
                .into_iter()
                .find(|(name, _)| name == "AIDA_AGENT_TYPE")
                .map(|(_, value)| value)
        })
        .map(agent_registry::normalize_agent_type)
        .unwrap_or_else(|| "unknown".to_string())
}

pub(crate) fn lease_activity_timestamp(
    project_root: &std::path::Path,
    lease: &SessionLease,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let path = session_activity_path(project_root, &lease.id);
    let body = std::fs::read_to_string(path).ok()?;
    let record = toml::from_str::<SessionActivityLog>(&body).ok()?;
    record.entries.into_iter().map(|entry| entry.at).max()
}

pub(crate) fn collect_branch_facts(project_root: &std::path::Path) -> Option<BranchFacts> {
    let name = current_branch_at(project_root)?;
    let dirty = !working_tree_clean(project_root).unwrap_or(true);
    let (ahead_main, behind_main) = ahead_behind_vs_ref(project_root, &name, "origin/main")
        .or_else(|| ahead_behind_vs_ref(project_root, &name, "main"))
        .map(|(a, b)| (Some(a), Some(b)))
        .unwrap_or((None, None));
    let upstream = upstream_ref_for(project_root, &name);
    let (ahead_upstream, behind_upstream, has_upstream) = match upstream {
        Some(u) => {
            let (a, b) = ahead_behind_vs_ref(project_root, &name, &u).unwrap_or((0, 0));
            (Some(a), Some(b), true)
        }
        None => (None, None, false),
    };
    Some(BranchFacts {
        name,
        dirty,
        ahead_main,
        behind_main,
        ahead_upstream,
        behind_upstream,
        has_upstream,
    })
}

pub(crate) fn working_tree_clean(project_root: &std::path::Path) -> Option<bool> {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    Some(o.stdout.iter().all(|b| b.is_ascii_whitespace()))
}

pub(crate) fn ahead_behind_vs_ref(
    project_root: &std::path::Path,
    branch: &str,
    target: &str,
) -> Option<(u32, u32)> {
    let exists = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "--quiet", target])
        .output()
        .ok()?
        .status
        .success();
    if !exists {
        return None;
    }
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-list",
            "--left-right",
            "--count",
            // trace:BUG-1622 | ai:claude
            git_arg_guard::END_OF_OPTIONS,
            &format!("{}...{}", branch, target),
        ])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&o.stdout);
    let mut parts = s.split_whitespace();
    let a: u32 = parts.next()?.parse().ok()?;
    let b: u32 = parts.next()?.parse().ok()?;
    Some((a, b))
}

pub(crate) fn upstream_ref_for(project_root: &std::path::Path, _branch: &str) -> Option<String> {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Process-lifetime memo over [`collect_pr_facts_uncached`]. The rich
/// `aida status` view resolves the current-branch PR/CI facts from more than
/// one section (the user-context gather + the awaiting-you report), and a
/// `--full` run warms this concurrently with the other gh probes up front
/// (TASK-1055). PR state does not change within a single short-lived `status`
/// run, so memoize by (canonical root, branch) — same justification as the
/// BUG-613 open-PR memo.
// trace:TASK-1055
pub(crate) fn collect_pr_facts(project_root: &std::path::Path, branch: &str) -> PrFacts {
    use std::sync::Mutex;
    use std::sync::OnceLock;
    static CACHE: OnceLock<
        Mutex<std::collections::HashMap<(std::path::PathBuf, String), PrFacts>>,
    > = OnceLock::new();
    let key = (
        project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf()),
        branch.to_string(),
    );
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(hit) = guard.get(&key) {
            return hit.clone();
        }
    }
    let facts = collect_pr_facts_uncached(project_root, branch);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, facts.clone());
    }
    facts
}

pub(crate) fn collect_pr_facts_uncached(project_root: &std::path::Path, branch: &str) -> PrFacts {
    // external-prose-classifier: status_context::collect_pr_facts_uncached
    // BUG-560: `gh` is GitHub-only. On a GitLab / pure-git remote it fails with
    // a raw "none of the git remotes ... point to a known GitHub host" auth
    // error that we used to surface verbatim — telling a corporate GitLab
    // first-user to `gh auth login` a CLI they don't use. Detect the forge and
    // skip the spawn entirely for anything but GitHub; the section then renders
    // a clean forge-aware line. trace:BUG-560 | ai:claude
    let forge_kind = forge::resolve_forge_kind(project_root);
    if forge_kind != forge::ForgeKind::GitHub {
        return PrFacts {
            number: 0,
            title: String::new(),
            url: String::new(),
            state: String::new(),
            ci_rollup: None,
            gh_status: GhStatus::NotGitHub(forge_kind),
        };
    }
    let gh_bin = match resolve_gh_binary() {
        Some(p) => p,
        None => {
            return PrFacts {
                number: 0,
                title: String::new(),
                url: String::new(),
                state: String::new(),
                ci_rollup: None,
                gh_status: GhStatus::Missing,
            };
        }
    };
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "view",
            branch,
            "--json",
            "number,title,url,state,statusCheckRollup",
            "-q",
            r#"[(.number|tostring), .title, .url, .state, ([.statusCheckRollup[]?.conclusion] | unique | join(","))] | @tsv"#,
        ])
        .output_retrying_etxtbsy();
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            return PrFacts {
                number: 0,
                title: String::new(),
                url: String::new(),
                state: String::new(),
                ci_rollup: None,
                gh_status: GhStatus::Failed(format!("{}", e)),
            };
        }
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // "no pull requests found" is the common no-PR-for-this-branch
        // path; treat it as a clean "no PR" rather than a failure.
        if aida_core::external_tool_output::contains_any_case_insensitive(
            &stderr,
            aida_core::external_tool_output::GH_NO_PULL_REQUEST,
        ) {
            return PrFacts {
                number: 0,
                title: String::new(),
                url: String::new(),
                state: "none".to_string(),
                ci_rollup: None,
                gh_status: GhStatus::Ok,
            };
        }
        return PrFacts {
            number: 0,
            title: String::new(),
            url: String::new(),
            state: String::new(),
            ci_rollup: None,
            gh_status: GhStatus::Failed(stderr),
        };
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let line = raw.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return PrFacts {
            number: 0,
            title: String::new(),
            url: String::new(),
            state: "none".to_string(),
            ci_rollup: None,
            gh_status: GhStatus::Ok,
        };
    }
    let mut fields = line.split('\t');
    let number: u64 = fields.next().unwrap_or("0").parse().unwrap_or(0);
    let title = fields.next().unwrap_or("").to_string();
    let url = fields.next().unwrap_or("").to_string();
    let state = fields.next().unwrap_or("").to_string();
    let rollup = fields.next().unwrap_or("").to_string();
    let ci_rollup = if rollup.is_empty() {
        None
    } else {
        Some(rollup)
    };
    PrFacts {
        number,
        title,
        url,
        state,
        ci_rollup,
        gh_status: GhStatus::Ok,
    }
}

pub(crate) fn collect_queue_snapshot(
    backend: &aida_core::CachedGitBackend,
    store: &aida_core::models::RequirementsStore,
    role: Option<&str>,
    leases: &[SessionLease],
) -> (Vec<QueueRow>, usize) {
    use aida_core::DatabaseBackend;
    let user_id = current_user_id(None);
    // Include the work peers routed to this role — it lives in THEIR queue
    // file, so an own-file-only read made the snapshot disagree with what is
    // actually routed to us. trace:BUG-774 | ai:claude
    let entries = match role {
        Some(r) => {
            queue_role_fallback::queue_list_with_role_fallback(backend, &user_id, Some(r), false)
                .unwrap_or_default()
        }
        None => backend.queue_list(&user_id, false).unwrap_or_default(),
    };

    // TASK-490: an at-keyboard operator's first question is "what is being
    // worked on right now?" — surface in-progress queue items unconditionally,
    // then fill the next-up display budget with the FIFO head of the rest.
    // The "X more" counter in the renderer is computed against next-up only
    // so the two sections sum to the visible totals.
    // trace:TASK-490 | ai:claude
    const NEXT_UP_BUDGET: usize = 5;
    let mut in_progress_rows: Vec<QueueRow> = Vec::new();
    let mut next_up_rows: Vec<QueueRow> = Vec::new();
    let mut total: usize = 0;
    for entry in entries {
        if let Some(r) = role {
            let mismatch = entry
                .for_role
                .as_deref()
                .map(|er: &str| !er.eq_ignore_ascii_case(r))
                .unwrap_or(false);
            if mismatch {
                continue;
            }
        }
        let Some(req) = store
            .requirements
            .iter()
            .find(|rq| rq.id == entry.requirement_id)
        else {
            continue;
        };
        if matches!(
            req.status,
            aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
        ) {
            continue;
        }
        total += 1;
        let is_in_progress = matches!(req.status, aida_core::RequirementStatus::InProgress);
        let spec_id = req.display_id();
        let (lease_id, lease_started_at) = if is_in_progress {
            leases
                .iter()
                .find(|l| l.scope.eq_ignore_ascii_case(&spec_id))
                .map(|l| (Some(l.id.clone()), Some(l.started_at)))
                .unwrap_or((None, None))
        } else {
            (None, None)
        };
        let row = QueueRow {
            spec_id,
            title: req.title.clone(),
            status: format!("{}", req.status),
            for_role: entry.for_role.clone(),
            in_progress: is_in_progress,
            lease_id,
            lease_started_at,
        };
        if is_in_progress {
            in_progress_rows.push(row);
        } else if next_up_rows.len() < NEXT_UP_BUDGET {
            next_up_rows.push(row);
        }
    }
    // In-progress first (all of them — typically few), then the FIFO head of
    // the remaining queue up to the display budget.
    in_progress_rows.extend(next_up_rows);
    (in_progress_rows, total)
}

/// TASK-756: a small additive presence line in `aida status`. Read-only —
/// surfaces the effective operator presence (home/away) without changing any
/// other section. Only the `away` state is loud enough to print; `home` is the
/// boring default and stays quiet so the line appearing IS the signal.
/// trace:TASK-756 | ai:claude
pub(crate) fn print_status_presence_line(project_root: &std::path::Path) {
    let now = chrono::Utc::now();
    if !matches!(presence::current_presence(now), presence::Presence::Away) {
        return;
    }
    let Some(file) = presence::read_presence_file() else {
        return;
    };
    let set_at = chrono::DateTime::parse_from_rfc3339(&file.set_at)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or(now);
    let _ = project_root; // reserved: per-project presence would key off this
    println!(
        "  {} {} (set {}, {})",
        crate::glyph(crate::glyphs::Glyph::Away).to_string(),
        "away".yellow().bold(),
        presence::since_label(set_at, now),
        presence::ttl_remaining_label(set_at, file.ttl_secs, now)
    );
    println!();
}

/// The keystone parking tag — work that is clear to build but reserved for the
/// at-keyboard `--zen` lane (the supervised-build cohort). Mirrors the
/// `needs-supervised-build` parking tag the burndown gate already excludes from
/// the autonomous ready set. trace:STORY-561 | ai:claude
pub(crate) const KEYSTONE_TAG: &str = "needs-supervised-build";

/// STORY-561 consumers (c) + (d) home-side: presence-gated surfacing in
/// `aida status`. When the operator is HOME and `[presence] consumers = on`,
/// surface (c) the decision inbox depth (`aida questions`) and — when
/// `home_offer = surface` — (d) the keystone set as "ready for `--zen`"
/// (presence is useful in BOTH directions). When AWAY these accumulate quietly
/// (the point of `away` is to NOT nag; the away line above already prints).
/// Display-only and advisory — changes no execution path. The away-side safety
/// floor (keystone NOT offered for autonomous pickup) is already structural:
/// `needs-supervised-build` is a parking tag, so the drain gate never picks it
/// up regardless of presence. trace:STORY-561 | ai:claude
pub(crate) fn print_status_presence_consumers(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
) {
    let cfg = presence::read_presence_config(&config_path_for_project(project_root));
    if cfg.consumers == presence::ConsumersMode::Off {
        return;
    }
    // Quiet accumulation when away — the away line is the only presence noise.
    if matches!(
        presence::current_presence(chrono::Utc::now()),
        presence::Presence::Away
    ) {
        return;
    }

    // (c) decision inbox — pending DecisionRequests awaiting the operator.
    // TASK-1065: read the count from the `has_pending_decision` cache column so
    // the rich `aida status` path no longer needs a full `backend.load()` to
    // surface it. Matches the prior store-based predicate exactly (non-archived +
    // `decision_request.is_pending()`); see `pending_decision_request_count` for
    // the reference oracle. trace:TASK-1065 (supersedes TASK-1061)
    let pending = backend.pending_decision_count().unwrap_or(0);
    if pending > 0 {
        println!(
            "  {} {} decision{} await your call — {}",
            "Decisions:".bold().yellow(),
            pending,
            if pending == 1 { "" } else { "s" },
            "aida questions".cyan()
        );
        println!();
    }

    // (d) home_offer = surface: the keystone cohort, ready for the at-keyboard
    // `--zen` lane. Open (non-terminal, triaged) specs carrying the keystone
    // parking tag. `dont-block` keeps home quiet here.
    if cfg.home_offer == presence::HomeOffer::Surface {
        if let Ok(rows) = backend.list_summaries(&aida_core::ListFilter {
            tags: vec![KEYSTONE_TAG.to_string()],
            ..Default::default()
        }) {
            let ready: Vec<String> = rows
                .iter()
                .filter(|r| {
                    matches!(
                        r.status.to_ascii_lowercase().as_str(),
                        "approved" | "planned" | "in-progress" | "inprogress"
                    )
                })
                .filter_map(|r| r.agreed_id.clone().or_else(|| r.spec_id.clone()))
                .collect();
            if !ready.is_empty() {
                const SHOW: usize = 5;
                let shown = ready
                    .iter()
                    .take(SHOW)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                let overflow = ready.len().saturating_sub(SHOW);
                let tail = if overflow > 0 {
                    format!(" (+{overflow} more)")
                } else {
                    String::new()
                };
                println!(
                    "  {} {} keystone spec{} ready for {} — {}{}",
                    "At keyboard:".bold().green(),
                    ready.len(),
                    if ready.len() == 1 { "" } else { "s" },
                    "--zen".cyan(),
                    shown,
                    tail
                );
                println!();
            }
        }
    }
}

pub(crate) fn print_status_session_section(ctx: &UserStatusContext) {
    println!("{}", "─── Session ───".bold());
    match &ctx.session {
        Some(l) => {
            let short_id = &l.id[..l.id.len().min(8)];
            println!(
                "  {} {}  · scope: {}",
                "id:".dimmed(),
                short_id.yellow(),
                l.scope.cyan()
            );
            println!("  {} {}", "branch:".dimmed(), l.branch.cyan());
            println!(
                "  {} {}",
                "worktree:".dimmed(),
                l.worktree_path.display().to_string().dimmed()
            );
            if let Some(role) = &l.role {
                println!("  {} {}", "role:".dimmed(), role.cyan());
            }
            println!(
                "  {} {}",
                "age:".dimmed(),
                humanize_relative(l.started_at).dimmed()
            );
        }
        None => {
            let extra = match &ctx.role {
                Some(r) => format!(" (active shell role: {})", r.cyan()),
                None => String::new(),
            };
            println!("  (no lease covers cwd){}", extra);
        }
    }
    println!();
}

pub(crate) fn print_status_branch_section(ctx: &UserStatusContext) {
    println!("{}", "─── Branch ───".bold());
    match &ctx.branch {
        Some(b) => {
            let dirty_chip = if b.dirty {
                " (dirty)".yellow().to_string()
            } else {
                " (clean)".green().to_string()
            };
            println!("  {} {}{}", "name:".dimmed(), b.name.cyan(), dirty_chip);
            if let (Some(a), Some(beh)) = (b.ahead_main, b.behind_main) {
                println!(
                    "  {} {} ahead, {} behind {}",
                    "vs main:".dimmed(),
                    a.to_string().green(),
                    if beh > 0 {
                        beh.to_string().yellow()
                    } else {
                        beh.to_string().normal()
                    },
                    "origin/main".dimmed()
                );
            }
            if b.has_upstream {
                if let (Some(a), Some(beh)) = (b.ahead_upstream, b.behind_upstream) {
                    println!(
                        "  {} {} ahead, {} behind",
                        "vs upstream:".dimmed(),
                        a.to_string().green(),
                        if beh > 0 {
                            beh.to_string().yellow()
                        } else {
                            beh.to_string().normal()
                        }
                    );
                }
            } else {
                println!("  {} (no upstream tracked)", "vs upstream:".dimmed());
            }
        }
        None => println!("  (not in a git repository)"),
    }
    println!();
}

pub(crate) fn print_status_pr_section(ctx: &UserStatusContext, focused: bool) {
    println!("{}", "─── PR / CI ───".bold());
    let Some(pr) = &ctx.pr else {
        println!("  (no branch context — can't query gh)");
        if focused {
            println!();
        }
        return;
    };
    match &pr.gh_status {
        GhStatus::Missing => {
            println!(
                "  (gh unavailable — install + auth `gh` to see CI status; \
                 `aida status --no-ci` to suppress this section)"
            );
        }
        GhStatus::Failed(reason) => {
            println!("  (gh failed: {})", reason.dimmed());
        }
        GhStatus::Skipped => {
            println!("  (skipped via --no-ci)");
        }
        // BUG-560: non-GitHub remote — name the right forge, never `gh`.
        GhStatus::NotGitHub(forge::ForgeKind::GitLab) => {
            println!(
                "  {}",
                "(GitLab remote — PR/CI status via `glab` is pending forge \
                 integration; the requirement store works fully)"
                    .dimmed()
            );
        }
        GhStatus::NotGitHub(_) => {
            println!(
                "  {}",
                "(no GitHub remote — the PR/CI section is GitHub-only today; \
                 the requirement store works fully)"
                    .dimmed()
            );
        }
        GhStatus::Ok if pr.state == "none" || pr.number == 0 => {
            println!("  (no open PR for this branch)");
        }
        GhStatus::Ok => {
            println!(
                "  {} {} {} {}",
                "PR:".dimmed(),
                format!("#{}", pr.number).cyan(),
                pr.state.dimmed(),
                pr.title
            );
            println!("  {} {}", "url:".dimmed(), pr.url.dimmed());
            match pr.ci_rollup.as_deref() {
                None => println!("  {} (no checks reported)", "ci:".dimmed()),
                Some("SUCCESS") => {
                    println!(
                        "  {} {}",
                        "ci:".dimmed(),
                        format!("{} SUCCESS", crate::glyph(crate::glyphs::Glyph::Check)).green()
                    )
                }
                Some(r) if r.contains("FAILURE") || r.contains("CANCELLED") => {
                    println!(
                        "  {} {}",
                        "ci:".dimmed(),
                        format!("{} {}", crate::glyph(crate::glyphs::Glyph::Cross), r).red()
                    )
                }
                Some(r) if r.contains("PENDING") || r.contains("IN_PROGRESS") => {
                    println!("  {} {}", "ci:".dimmed(), format!("⏱ {}", r).yellow())
                }
                Some(r) => println!("  {} {}", "ci:".dimmed(), r),
            }
        }
    }
    println!();
}

/// TASK-490: pure split of the queue head into the two display groups, with
/// the counter math the renderer needs ("X more" reflects only not-in-progress
/// items). Lifted out of the renderer so the layout invariants can be tested
/// without stdout capture. trace:TASK-490 | ai:claude
pub(crate) struct QueueViewSplit<'a> {
    pub(crate) in_progress_rows: Vec<&'a QueueRow>,
    pub(crate) next_up_rows: Vec<&'a QueueRow>,
    pub(crate) in_progress_total: usize,
    pub(crate) next_up_total: usize,
}

pub(crate) fn split_queue_view<'a>(head: &'a [QueueRow], queue_total: usize) -> QueueViewSplit<'a> {
    let (in_progress_rows, next_up_rows): (Vec<&QueueRow>, Vec<&QueueRow>) =
        head.iter().partition(|r| r.in_progress);
    let in_progress_total = in_progress_rows.len();
    let next_up_total = queue_total.saturating_sub(in_progress_total);
    QueueViewSplit {
        in_progress_rows,
        next_up_rows,
        in_progress_total,
        next_up_total,
    }
}

pub(crate) fn print_status_queue_section(ctx: &UserStatusContext, _focused: bool) {
    println!("{}", "─── Queue ───".bold());
    let role_label = ctx.role.as_deref().unwrap_or("(no active role)");
    if ctx.queue_total == 0 {
        println!("  (empty for role:{})", role_label.cyan());
    } else {
        println!(
            "  {} routed to role:{}",
            format!("{} item(s)", ctx.queue_total).cyan(),
            role_label.cyan()
        );
        // TASK-490: split queue display into "In progress" + "Next up". The
        // "what's being worked on right now?" signal is the highest-value
        // piece of queue info for an at-keyboard operator — surface it first
        // so it isn't buried in the FIFO tail. trace:TASK-490 | ai:claude
        let QueueViewSplit {
            in_progress_rows,
            next_up_rows,
            in_progress_total,
            next_up_total,
        } = split_queue_view(&ctx.queue_head, ctx.queue_total);

        if !in_progress_rows.is_empty() {
            println!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::InFlight).magenta(),
                format!("In progress ({}):", in_progress_total).magenta()
            );
            for row in &in_progress_rows {
                let lease_chip = match (&row.lease_id, &row.lease_started_at) {
                    (Some(id), Some(started)) => {
                        let short = &id[..id.len().min(8)];
                        format!(
                            "  [{}: {}, {}]",
                            "lease".dimmed(),
                            short.yellow(),
                            humanize_relative(*started).dimmed()
                        )
                    }
                    _ => String::new(),
                };
                println!(
                    "      {} [{}] {}{}",
                    row.spec_id.bold(),
                    status_display::status_badge(&row.status),
                    row.title,
                    lease_chip
                );
            }
        }

        if !next_up_rows.is_empty() {
            println!("  {}", "Next up:".dimmed());
            for (i, row) in next_up_rows.iter().enumerate() {
                // TASK-269: shared status badge. trace:TASK-269 | ai:claude
                println!(
                    "  {:>2}. {} [{}] {}",
                    i + 1,
                    row.spec_id.bold(),
                    status_display::status_badge(&row.status),
                    row.title
                );
            }
            if next_up_total > next_up_rows.len() {
                println!(
                    "    {} {} more (run `aida queue list`)",
                    "…".dimmed(),
                    (next_up_total - next_up_rows.len()).to_string().dimmed()
                );
            }
        }
    }
    println!();
}

/// BUG-609: partition an agent roster into (live, stale). An agent is "stale"
/// when its classified status is `Stale` — which `agent_registry::classify_status`
/// sets iff the registry pid failed the `process_probe::pid_is_alive` liveness
/// probe (or a lease-derived view's lease itself went stale). The headline
/// "Active agents (N)" must count only the live partition so dead-PID corpses
/// (observed 89 registrations where ~9 were live, the rest stale 8-16 days)
/// stop inflating the fleet roster. Reuses the already-probed verdict carried on
/// the view — no second per-agent scan, so the BUG-613 status-timing win holds.
// trace:BUG-609 | ai:claude
pub(crate) fn partition_agents_by_liveness(
    agents: &[agent_registry::AgentRegistryView],
) -> (
    Vec<agent_registry::AgentRegistryView>,
    Vec<agent_registry::AgentRegistryView>,
) {
    agents
        .iter()
        .cloned()
        .partition(|a| a.status != agent_registry::AgentStatus::Stale)
}

/// `show_stale` (from `aida status --all`/`--stale`) reveals the dead-PID
/// corpses the default view hides behind a footer count.
// trace:BUG-609 | ai:claude
pub(crate) fn print_status_agents_section(ctx: &UserStatusContext, show_stale: bool) {
    if ctx.agents.is_empty() {
        return;
    }
    let (live, stale) = partition_agents_by_liveness(&ctx.agents);
    // Even when every agent is stale, render the section (headline "(0)" plus
    // the footer) so the operator sees that the roster is all corpses rather
    // than the section silently vanishing.
    println!(
        "{}",
        format!("─── Active agents ({}) ───", live.len()).bold()
    );
    let shown: &[agent_registry::AgentRegistryView] = if show_stale { &ctx.agents } else { &live };
    for line in agent_registry::format_agent_status_lines(shown) {
        println!("{line}");
    }
    for line in agent_registry::mcp_authority_status_lines(shown) {
        println!("{line}");
    }
    if !stale.is_empty() && !show_stale {
        println!(
            "  {}",
            format!(
                "+{} stale (dead PID) — `aida status --all` to show, `aida doctor heal` to reap",
                stale.len()
            )
            .dimmed()
        );
    }
    println!();
}

/// SPIKE-30: query `claude agents --json` and cross-reference live Claude
/// Code sessions against AIDA's lease registry. Surfaces three signals the
/// existing hygiene scan can't:
///   - Linked: AIDA lease ↔ live Claude session pair (the healthy case)
///   - Untracked: Claude session running in a project worktree with no
///     matching AIDA lease (interactive shell, or a rogue launch)
///   - Section-wide counts: cross-substrate one-glance view
///
/// Skips silently when `claude` isn't on PATH (cross-tool, optional dep).
/// Dormant leases (lease present, no Claude session) are intentionally
/// NOT re-reported here — the Hygiene section already covers them via
/// process_probe.rs; duplicating would dilute both signals.
/// trace:SPIKE-30 | ai:claude
pub(crate) fn print_status_claude_code_section(project_root: &std::path::Path) {
    let Some(entries) = claude_agents::list_agents() else {
        return;
    };
    if entries.is_empty() {
        return;
    }
    let leases = list_leases(project_root);
    let worktree_paths: Vec<std::path::PathBuf> =
        leases.iter().map(|l| l.worktree_path.clone()).collect();
    let (in_scope, elsewhere) =
        claude_agents::partition_by_project(&entries, project_root, &worktree_paths);

    let manifests = session_manifest::list_all(project_root);

    // Build claude_session_id → lease lookup via manifest join, with cwd
    // fallback when the manifest doesn't carry the join key (legacy leases
    // pre-dating TASK-112's claude_session_id recording).
    let mut linked: Vec<(claude_agents::ClaudeAgentEntry, SessionLease)> = Vec::new();
    let mut untracked: Vec<claude_agents::ClaudeAgentEntry> = Vec::new();
    for entry in &in_scope {
        let manifest = manifests
            .iter()
            .find(|m| m.claude_session_id.as_deref() == Some(entry.session_id.as_str()));
        let lease = manifest
            .and_then(|m| {
                leases
                    .iter()
                    .find(|l| l.id.starts_with(&m.session_id) || m.session_id.starts_with(&l.id))
            })
            .or_else(|| leases.iter().find(|l| l.worktree_path == entry.cwd));
        match lease {
            Some(l) => linked.push((entry.clone(), l.clone())),
            None => untracked.push(entry.clone()),
        }
    }

    println!("{}", "─── Claude Code ───".bold());
    println!(
        "  Live: {} in project, {} elsewhere",
        in_scope.len().to_string().cyan(),
        elsewhere.len().to_string().dimmed(),
    );
    if !linked.is_empty() {
        println!(
            "  Linked (lease ↔ Claude session): {}",
            linked.len().to_string().green()
        );
        for (entry, lease) in &linked {
            let role = lease.role.as_deref().unwrap_or("?");
            println!(
                "    {}  {}  ({})  {}",
                entry.short_session_id().yellow(),
                lease.scope.cyan(),
                role.dimmed(),
                claude_status_chip(entry),
            );
        }
    }
    // Split untracked into:
    //   - cwd == project_root → interactive shells (the user's terminals);
    //     collapsed to a count because these are expected and high-volume
    //   - cwd in a worktree but no matching lease → drift (rogue launches,
    //     lease cleaned up while Claude still alive); always shown in full
    let (shell_count, drift): (usize, Vec<_>) =
        untracked
            .iter()
            .fold((0, Vec::new()), |(mut shells, mut drift), entry| {
                if entry.cwd == project_root {
                    shells += 1;
                } else {
                    drift.push(entry.clone());
                }
                (shells, drift)
            });
    if shell_count > 0 {
        println!(
            "  Interactive shells in project root: {}",
            shell_count.to_string().dimmed(),
        );
    }
    if !drift.is_empty() {
        println!(
            "  In worktree without lease ({}): {}",
            drift.len().to_string().yellow(),
            "rogue Claude or stale lease cleanup".dimmed(),
        );
        for entry in drift.iter().take(5) {
            println!(
                "    {}  {}  {}",
                entry.short_session_id().yellow(),
                entry.cwd.display().to_string().dimmed(),
                claude_status_chip(entry),
            );
        }
        if drift.len() > 5 {
            println!(
                "    {}",
                format!("... and {} more", drift.len() - 5).dimmed()
            );
        }
    }
    println!();
}
