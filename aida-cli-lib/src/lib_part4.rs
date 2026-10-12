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

// ── Worktree warm-pool CLI (STORY-714) ──────────────────────────────────────
//
// Dispatched before storage init (via handle_worktree_command): the pool
// operates purely on git + the `.aida/worktree-pool/` registry, no requirement
// store. trace:STORY-714 | ai:claude

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
