//! `aida statusbar` — the ambient, read-only terminal-title meter.
//!
//! Emits the OSC-2 set-window-title escape on an interval carrying a compact
//! live project meter — queue depth, live/STALE session counts, and how many
//! items await you — so any OSC-capable terminal (tmux or not) shows the
//! project's pulse while you are NOT in the TUI. Strictly an ambient signal:
//! it dispatches nothing, launches nothing, and writes nothing.
//!
//! Sources are cache/local-fast only (the same no-network discipline as the
//! per-turn awaiting notice): the orphan-store queue YAML, the session-lease
//! liveness probe, and the local coordination channels. The gh-backed PR
//! channel is deliberately omitted — a background loop must never poll the
//! network.
//!
//! trace:STORY-715 | ai:claude

use anyhow::Result;

use crate::*;

/// Save the current terminal title (XTerm title-stack push). Paired with
/// [`RESTORE_TITLE`]; opt-in because not every terminal implements the stack.
pub(crate) const SAVE_TITLE: &str = "\x1b[22;0t";
/// Restore the previously saved terminal title (XTerm title-stack pop).
pub(crate) const RESTORE_TITLE: &str = "\x1b[23;0t";

/// Floor for `--interval` — a title refresh is cheap but not free (a lease
/// probe + a couple of local file reads), so don't let a typo busy-loop it.
const MIN_INTERVAL_SECS: u64 = 2;

/// The gathered meter counts. Pure data so the renderer is unit-testable
/// without a store, a lease dir, or a terminal.
// trace:STORY-715 | ai:claude
#[derive(Debug, Default, Clone)]
pub(crate) struct MeterCounts {
    /// Depth of the queue routed to the effective role (implementer default).
    pub queue_depth: usize,
    /// Sessions with a live process backing the lease.
    pub live: usize,
    /// Leaked leases — dead pid / missing worktree. Hidden when zero.
    pub stale: usize,
    /// Needs-you breakdown: `(count, channel label)` per non-empty channel,
    /// in render order. Labels arrive pre-pluralized.
    pub you: Vec<(usize, String)>,
}

impl MeterCounts {
    /// Total items awaiting the operator — the sum over every channel
    /// (each unread mail counts, unlike the awaiting report's line total).
    pub fn you_total(&self) -> usize {
        self.you.iter().map(|(n, _)| n).sum()
    }
}

/// Max needs-you channels named inside the parenthetical breakdown; a title
/// bar is a narrow surface, so the long tail collapses to `…`.
const YOU_BREAKDOWN_CAP: usize = 3;

/// Render the meter line, e.g.
/// `aida · q:5 · live:2 · STALE:1 · you:3 (2 mail · 1 punt)`.
/// Always shows `q:` and `live:` (the "all quiet" reading is itself signal);
/// `STALE:` and `you:` appear only when non-zero so a healthy project stays
/// short. Pure — unit-tested without any I/O.
// trace:STORY-715 | ai:claude
pub(crate) fn render_meter(c: &MeterCounts) -> String {
    let mut parts = vec![
        "aida".to_string(),
        format!("q:{}", c.queue_depth),
        format!("live:{}", c.live),
    ];
    if c.stale > 0 {
        parts.push(format!("STALE:{}", c.stale));
    }
    let total = c.you_total();
    if total > 0 {
        let named: Vec<String> = c
            .you
            .iter()
            .filter(|(n, _)| *n > 0)
            .take(YOU_BREAKDOWN_CAP)
            .map(|(n, label)| format!("{n} {label}"))
            .collect();
        let overflow = c.you.iter().filter(|(n, _)| *n > 0).count() > YOU_BREAKDOWN_CAP;
        let inner = if overflow {
            format!("{} …", named.join(" · "))
        } else {
            named.join(" · ")
        };
        parts.push(format!("you:{total} ({inner})"));
    }
    parts.join(" · ")
}

/// Flatten the awaiting-you report into the meter's needs-you channels:
/// `(count, pre-pluralized label)` per non-empty channel. Mail counts per
/// message (the meter is a tally, not a line count). PRs are included for
/// completeness but are always empty on the meter's no-network path.
// trace:STORY-715 | ai:claude
pub(crate) fn you_channels(report: &awaiting_you::AwaitingReport) -> Vec<(usize, String)> {
    fn label(n: usize, singular: &str, plural: &str) -> String {
        if n == 1 { singular } else { plural }.to_string()
    }
    let mut v: Vec<(usize, String)> = Vec::new();
    let prs = report.mergeable_prs.len();
    if prs > 0 {
        v.push((prs, label(prs, "PR", "PRs")));
    }
    let briefs = report.pending_briefs.len();
    if briefs > 0 {
        v.push((briefs, label(briefs, "brief", "briefs")));
    }
    if report.findings_total > 0 {
        v.push((
            report.findings_total,
            label(report.findings_total, "finding", "findings"),
        ));
    }
    if report.mail.unread > 0 {
        v.push((report.mail.unread, "mail".to_string()));
    }
    let directives = report.worker_directives.pending;
    if directives > 0 {
        v.push((directives, label(directives, "directive", "directives")));
    }
    let verdicts = report.reviewer_queue_items.len();
    if verdicts > 0 {
        // Invariant label — "2 approve" reads as "2 awaiting your approval".
        v.push((verdicts, "approve".to_string()));
    }
    let punts = report.escalations.len();
    if punts > 0 {
        v.push((punts, label(punts, "punt", "punts")));
    }
    v
}

/// Gather the meter counts from cache/local-fast sources only. `backend` is
/// optional so an unattached / offline clone still renders the queue + lease
/// half of the meter (the needs-you channels degrade to empty).
// trace:STORY-715 | ai:claude
fn collect_meter(
    project_root: &std::path::Path,
    backend: Option<&aida_core::CachedGitBackend>,
) -> MeterCounts {
    // Queue depth for the effective role — the same resolution the
    // statusline's `q:` segment uses (implementer when no role is set).
    let raw_role = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let (effective_role, _role_is_default) = resolve_effective_role(raw_role.as_deref());
    let queue_depth = read_queue_depth(project_root, Some(effective_role.as_str())).unwrap_or(0);

    // Live / STALE — the same running-work gather `aida ps` renders, so the
    // meter and the table can never disagree about what is genuinely running.
    let (rows, _orphans) = gather_running_work(project_root);
    let live = rows
        .iter()
        .filter(|r| matches!(r.state, LeaseState::Live))
        .count();
    let stale = rows
        .iter()
        .filter(|r| matches!(r.state, LeaseState::Stale))
        .count();

    // Needs-you — the cheap (no_ci) awaiting report, mirroring the per-turn
    // notice path: a lightweight context (role from env, empty queue head),
    // so no full-store load and no gh call ever rides the refresh loop.
    let you = backend
        .map(|b| {
            let ctx = UserStatusContext {
                session: None,
                role: raw_role.clone(),
                branch: None,
                pr: None,
                queue_head: Vec::new(),
                queue_total: 0,
                agents: Vec::new(),
            };
            you_channels(&collect_awaiting_report(project_root, b, &ctx, true))
        })
        .unwrap_or_default();

    MeterCounts {
        queue_depth,
        live,
        stale,
        you,
    }
}

/// Emit one OSC-2 title update (no trailing newline) and flush so the
/// terminal sees it immediately.
fn emit_title(line: &str) {
    use std::io::Write;
    print!("{}", statusline_cmd::osc_terminal_title(line));
    let _ = std::io::stdout().flush();
}

/// Install a SIGINT/SIGTERM stop flag so the refresh loop can exit cleanly
/// (and restore the saved title when asked). Non-Unix: no handler — the loop
/// still runs; a hard kill simply skips the title restore.
#[cfg(unix)]
fn install_stop_flag() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        let _ = signal_hook::flag::register(sig, std::sync::Arc::clone(&stop));
    }
    stop
}

#[cfg(not(unix))]
fn install_stop_flag() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
}

/// `aida statusbar` — render the ambient meter. Three shapes:
///   - default: refresh the terminal title every `interval` seconds until
///     Ctrl+C (with opt-in save/restore of the previous title);
///   - `--once`: one title update, then exit (shell prompt hooks);
///   - `--plain`: print the meter as a plain text line (tmux status-right).
// trace:STORY-715 | ai:claude
pub(crate) fn handle_statusbar_command(
    interval: u64,
    once: bool,
    plain: bool,
    restore_title: bool,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    // Self-resolve the store like `aida integrate`: an unattached or offline
    // clone degrades to the queue + lease half of the meter, never an error —
    // an ambient meter that crashes the prompt/status bar is worse than one
    // that under-reports.
    let backend = detect_distributed_store().and_then(|sp| advance_backend(&sp).ok());

    if plain {
        println!(
            "{}",
            render_meter(&collect_meter(&project_root, backend.as_ref()))
        );
        return Ok(());
    }
    if once {
        emit_title(&render_meter(&collect_meter(
            &project_root,
            backend.as_ref(),
        )));
        return Ok(());
    }

    // Loop mode. The stop flag makes Ctrl+C a clean exit so the opt-in title
    // restore actually runs.
    let interval = interval.max(MIN_INTERVAL_SECS);
    let stop = install_stop_flag();
    if restore_title {
        use std::io::Write;
        print!("{SAVE_TITLE}");
        let _ = std::io::stdout().flush();
    }
    while !stop.load(std::sync::atomic::Ordering::SeqCst) {
        emit_title(&render_meter(&collect_meter(
            &project_root,
            backend.as_ref(),
        )));
        // Sleep in short slices so a stop signal is honored promptly.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(interval);
        while std::time::Instant::now() < deadline
            && !stop.load(std::sync::atomic::Ordering::SeqCst)
        {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
    if restore_title {
        use std::io::Write;
        print!("{RESTORE_TITLE}");
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awaiting_you::{
        AwaitingReport, DirectivesChannel, EscalationItem, MailChannel, MergeablePrItem,
        PendingBriefItem, ReviewerQueueItem,
    };

    #[test]
    fn quiet_meter_shows_queue_and_live_only() {
        let c = MeterCounts::default();
        assert_eq!(render_meter(&c), "aida · q:0 · live:0");
    }

    #[test]
    fn stale_and_you_segments_appear_only_when_nonzero() {
        let c = MeterCounts {
            queue_depth: 5,
            live: 2,
            stale: 1,
            you: vec![(2, "mail".to_string()), (1, "punt".to_string())],
        };
        assert_eq!(
            render_meter(&c),
            "aida · q:5 · live:2 · STALE:1 · you:3 (2 mail · 1 punt)"
        );

        let no_stale = MeterCounts {
            stale: 0,
            ..c.clone()
        };
        assert!(!render_meter(&no_stale).contains("STALE"));
    }

    #[test]
    fn you_breakdown_caps_at_three_channels_with_ellipsis() {
        let c = MeterCounts {
            you: vec![
                (2, "briefs".to_string()),
                (1, "finding".to_string()),
                (3, "mail".to_string()),
                (1, "punt".to_string()),
            ],
            ..Default::default()
        };
        let line = render_meter(&c);
        assert!(
            line.contains("you:7 (2 briefs · 1 finding · 3 mail …)"),
            "{line}"
        );
        assert!(
            !line.contains("punt"),
            "fourth channel must collapse: {line}"
        );
    }

    #[test]
    fn you_channels_flattens_every_populated_channel_with_counts() {
        let report = AwaitingReport {
            mergeable_prs: vec![MergeablePrItem {
                number: 7,
                title: "t".into(),
                head_branch: "b".into(),
                ci_rollup: Some("pass".into()),
            }],
            pending_briefs: vec![PendingBriefItem {
                agent: "claude".into(),
                spec_id: "".into(),
                path: std::path::PathBuf::from("/b/1"),
            }],
            findings_total: 2,
            mail: MailChannel {
                unread: 3,
                urgent: 1,
            },
            worker_directives: DirectivesChannel {
                pending: 1,
                next: None,
            },
            reviewer_queue_items: vec![ReviewerQueueItem {
                spec_id: "".into(),
                title: "".into(),
            }],
            escalations: vec![
                EscalationItem {
                    spec_id: "".into(),
                    title: "".into(),
                },
                EscalationItem {
                    spec_id: "".into(),
                    title: "".into(),
                },
            ],
        };
        let channels = you_channels(&report);
        let rendered: Vec<String> = channels.iter().map(|(n, l)| format!("{n} {l}")).collect();
        assert_eq!(
            rendered,
            vec![
                "1 PR",
                "1 brief",
                "2 findings",
                "3 mail",
                "1 directive",
                "1 approve",
                "2 punts"
            ]
        );
        // The meter total is the SUM over channels (each mail counts), not
        // the awaiting report's collapsed line total.
        let c = MeterCounts {
            you: channels,
            ..Default::default()
        };
        assert_eq!(c.you_total(), 11);
    }

    #[test]
    fn empty_report_yields_no_you_channels() {
        assert!(you_channels(&AwaitingReport::default()).is_empty());
    }

    #[test]
    fn meter_line_carries_no_control_chars_for_the_osc_title() {
        let c = MeterCounts {
            queue_depth: 9,
            live: 3,
            stale: 2,
            you: vec![(1, "mail".to_string())],
        };
        let line = render_meter(&c);
        assert!(line.chars().all(|ch| !ch.is_control()), "{line:?}");
        // Wrapped form is a single well-terminated OSC-2 sequence.
        let osc = statusline_cmd::osc_terminal_title(&line);
        assert!(osc.starts_with("\x1b]2;"), "{osc:?}");
        assert!(osc.ends_with('\x07'), "{osc:?}");
    }

    #[test]
    fn title_stack_escapes_are_the_xterm_pair() {
        assert_eq!(SAVE_TITLE, "\x1b[22;0t");
        assert_eq!(RESTORE_TITLE, "\x1b[23;0t");
    }
}
