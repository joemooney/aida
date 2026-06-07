//! Empty-state welcome panel — the centered card shown when `aida tui`
//! has no hosted session (BUG-109).
//!
//! Before this, an empty shell (no scope on launch, or every hosted
//! session ended) painted nothing but the bottom status strip — a black
//! screen with no hint that `Ctrl-A` is the prefix key. First-time users
//! (the TUI is default-on since STORY-137) reflexively pressed q /
//! Ctrl-C / arrows, none of which did anything, and killed the process
//! from another terminal.
//!
//! The panel coexists with the strip: it owns rows `0..H-1`, the strip
//! owns the last row. It is plain crossterm chrome — a static base layer
//! like the strip and the blitted child — whereas the `prefix o`
//! overlay, `prefix n` picker and `prefix ?` help are ratatui modals.
//!
//! ## Mission-control empty state (TASK-255)
//!
//! The thin slice of the mission-control vision: when nothing is hosted,
//! surface what's *actionable* — the role queue head (what to pick up
//! next) and the project-wide open-PR count — beneath the keybindings.
//! The queue head is read cache-only via the same `aida status --json`
//! projection the `prefix o` overlay uses (sub-millisecond); the open-PR
//! count is a best-effort `gh pr list` that degrades to "unavailable"
//! when `gh` is missing / offline. Deferred to follow-ups: active
//! session leases, the CI rollup, and suggested next actions.
//!
//! [`mission_section`] is a pure assembly over [`MissionData`], unit-
//! tested directly; [`fetch_mission`] is the impure gather around it.
//!
//! trace:BUG-109 | ai:claude
//! trace:TASK-255 | ai:claude

use crossterm::{
    cursor,
    style::Print,
    terminal::{Clear, ClearType},
    QueueableCommand,
};
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One queued requirement shown in the mission-control empty state —
/// the spec id, its title, and its status. trace:TASK-255 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissionQueueItem {
    pub spec_id: String,
    pub title: String,
    pub status: String,
}

/// The actionable snapshot surfaced beneath the keybindings when the
/// shell is empty: the role queue head (what to pick up next) and the
/// project-wide open-PR count. Both are best-effort — a stale cache, a
/// queue-less role, or a missing `gh` each degrade to a placeholder line
/// rather than blocking the empty-state paint. trace:TASK-255 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissionData {
    /// The role the queue head was read for (e.g. `implementer`).
    pub role: Option<String>,
    /// Total items queued for the role (the head is a truncation of it).
    pub queue_total: u64,
    /// The first few queued items — `mission_section` shows at most
    /// [`MISSION_QUEUE_LIMIT`].
    pub queue_head: Vec<MissionQueueItem>,
    /// Project-wide count of open PRs, or `None` when `gh` couldn't be
    /// reached (missing / offline / unauthenticated).
    pub open_prs: Option<usize>,
}

/// How many queue-head rows the empty state shows before the
/// "+N more" tail. Keeps the card compact on small terminals.
const MISSION_QUEUE_LIMIT: usize = 3;

/// Build the mission-control body lines from a [`MissionData`] snapshot.
/// Pure (no I/O) so it can be unit-tested directly; [`fetch_mission`]
/// gathers the data, [`panel`] frames these lines beneath the keys.
/// trace:TASK-255 | ai:claude
pub fn mission_section(data: &MissionData) -> Vec<String> {
    let mut out = Vec::new();
    let role = data.role.as_deref().unwrap_or("—");
    out.push(format!(
        "Up next — {role} queue ({} total)",
        data.queue_total
    ));
    if data.queue_head.is_empty() {
        out.push("  queue empty for this role".to_string());
    } else {
        for (i, item) in data.queue_head.iter().take(MISSION_QUEUE_LIMIT).enumerate() {
            // `STORY-123  short title  [Approved]`
            out.push(format!(
                "  {}. {}  {}  [{}]",
                i + 1,
                item.spec_id,
                item.title,
                item.status
            ));
        }
        let shown = data.queue_head.len().min(MISSION_QUEUE_LIMIT);
        if data.queue_head.len() > shown {
            out.push(format!("  +{} more", data.queue_head.len() - shown));
        }
    }
    out.push(String::new());
    let pr_line = match data.open_prs {
        Some(0) => "Open PRs — none".to_string(),
        Some(1) => "Open PRs — 1".to_string(),
        Some(n) => format!("Open PRs — {n}"),
        None => "Open PRs — unavailable (gh offline?)".to_string(),
    };
    out.push(pr_line);
    out
}

/// Gather a fresh [`MissionData`] snapshot. The queue head reuses the
/// overlay's cache-only `aida status --json` fetch (sub-millisecond); the
/// open-PR count shells out to `gh pr list --state open` with a short
/// wall-clock timeout so an offline shell can never stall the empty-state
/// paint. Both legs degrade independently. trace:TASK-255 | ai:claude
pub fn fetch_mission() -> MissionData {
    let mut data = MissionData::default();

    // Queue head — cache-only, the same projection the `prefix o` overlay
    // paints from, so the empty state and the overlay agree.
    if let Ok(model) = crate::overlay::fetch(true) {
        data.role = model
            .queue
            .as_ref()
            .and_then(|q| q.role.clone())
            .or(model.role);
        if let Some(q) = model.queue {
            data.queue_total = q.total;
            data.queue_head = q
                .head
                .into_iter()
                .map(|item| MissionQueueItem {
                    spec_id: item.spec_id,
                    title: item.title,
                    status: item.status,
                })
                .collect();
        }
    }

    data.open_prs = count_open_prs(Duration::from_secs(3));
    data
}

/// Count open PRs via `gh pr list --state open`. Best-effort: any failure
/// (gh missing, offline, unauthenticated, timeout, unparseable JSON)
/// returns `None`, which renders as "unavailable" rather than an error.
/// trace:TASK-255 | ai:claude
fn count_open_prs(timeout: Duration) -> Option<usize> {
    let mut child = Command::new("gh")
        .args(["pr", "list", "--state", "open", "--json", "number"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .ok()?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut buf = Vec::new();
                if let Some(mut s) = child.stdout.take() {
                    use std::io::Read;
                    let _ = s.read_to_end(&mut buf);
                }
                return parse_open_pr_count(&buf);
            }
            Ok(Some(_)) => return None, // gh ran but failed
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

/// Parse the length of `gh pr list --json number` output. Split out so the
/// count logic is testable without `gh`. trace:TASK-255 | ai:claude
fn parse_open_pr_count(json: &[u8]) -> Option<usize> {
    let parsed: serde_json::Value = serde_json::from_slice(json).ok()?;
    parsed.as_array().map(|a| a.len())
}

/// The empty-state card's body lines, before the box is drawn around
/// them. `prefix` is the human label for the configured prefix key
/// (`Ctrl-A` by default) so a reconfigured prefix stays accurate.
fn body(prefix: &str) -> Vec<String> {
    vec![
        "AIDA hosts your Claude Code sessions.".to_string(),
        "Nothing is running yet.".to_string(),
        String::new(),
        format!("{prefix}  N    new session"),
        format!("{prefix}  O    status overlay"),
        format!("{prefix}  ?    all keybindings"),
        format!("{prefix}  D    detach"),
        format!("{prefix}  Q    quit"),
        String::new(),
        format!("Press {prefix} then N to pick one — or"),
        // Generic `<SCOPE>` placeholder, never a real spec id: a
        // first-user has no EPIC-26 (that's AIDA's own TUI epic), and an
        // internal id with no context is noise on a welcome screen.
        // trace:TASK-268 | ai:claude
        "relaunch as  aida tui <SCOPE>.".to_string(),
    ]
}

/// Build the welcome card: a bordered box around [`body`], optionally
/// followed by a divider and the mission-control section (queue head +
/// open-PR count) when `mission` is supplied. Every returned line is the
/// same display width, so [`render`] can centre the block with one
/// offset. Pure (no I/O) — unit-tested directly. trace:TASK-255 | ai:claude
pub fn panel(prefix: &str, mission: Option<&MissionData>) -> Vec<String> {
    let body = full_body(prefix, mission);
    let title = " aida tui ";
    let text_w = body.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    // Inner width between the corner columns; never narrower than the
    // title bar.
    let inner = (text_w + 2).max(title.chars().count());

    let mut out = Vec::with_capacity(body.len() + 2);
    out.push(format!("╭{}╮", center_fill(title, inner)));
    for line in &body {
        out.push(format!("│ {:<width$} │", line, width = inner - 2));
    }
    out.push(format!("╰{}╯", "─".repeat(inner)));
    out
}

/// The full set of card body lines: the keybindings, plus — when a
/// mission snapshot is supplied — a blank-line gap and the mission-control
/// section. Shared by [`panel`] (framed) and [`render`]'s small-terminal
/// fallback (bare) so both surfaces show the same content.
/// trace:TASK-255 | ai:claude
fn full_body(prefix: &str, mission: Option<&MissionData>) -> Vec<String> {
    let mut body = body(prefix);
    if let Some(data) = mission {
        body.push(String::new());
        body.extend(mission_section(data));
    }
    body
}

/// Place `s` in a field of `width` columns padded with `─`, centred.
fn center_fill(s: &str, width: usize) -> String {
    let sw = s.chars().count();
    if sw >= width {
        return s.chars().take(width).collect();
    }
    let left = (width - sw) / 2;
    let right = width - sw - left;
    format!("{}{}{}", "─".repeat(left), s, "─".repeat(right))
}

/// Paint the welcome card centred in the `width × height` region (the
/// screen above the status strip). The caller has already cleared the
/// screen; on a terminal too small for the bordered card the bare body
/// lines are printed top-left, clipped, so the keys are still legible.
pub fn render(
    out: &mut impl Write,
    width: u16,
    height: u16,
    prefix: &str,
    mission: Option<&MissionData>,
) -> io::Result<()> {
    let card = panel(prefix, mission);
    let card_h = card.len() as u16;
    let card_w = card.first().map(|l| l.chars().count()).unwrap_or(0) as u16;

    if width < card_w || height < card_h {
        // Too small for the bordered card — fall back to the bare body.
        for (i, line) in full_body(prefix, mission).iter().enumerate() {
            if i as u16 >= height {
                break;
            }
            let clipped: String = line.chars().take(width as usize).collect();
            out.queue(cursor::MoveTo(0, i as u16))?;
            out.queue(Clear(ClearType::CurrentLine))?;
            out.queue(Print(clipped))?;
        }
        return Ok(());
    }

    let top = (height - card_h) / 2;
    let left = (width - card_w) / 2;
    for (i, line) in card.iter().enumerate() {
        out.queue(cursor::MoveTo(left, top + i as u16))?;
        out.queue(Print(line))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_mission() -> MissionData {
        MissionData {
            role: Some("implementer".to_string()),
            queue_total: 4,
            queue_head: vec![
                MissionQueueItem {
                    spec_id: "STORY-1".to_string(),
                    title: "first thing".to_string(),
                    status: "Approved".to_string(),
                },
                MissionQueueItem {
                    spec_id: "TASK-2".to_string(),
                    title: "second thing".to_string(),
                    status: "Planned".to_string(),
                },
            ],
            open_prs: Some(3),
        }
    }

    #[test]
    fn panel_lines_are_uniform_width() {
        let card = panel("Ctrl-A", None);
        let w = card[0].chars().count();
        assert!(w >= 30, "card should be wide enough to read");
        assert!(
            card.iter().all(|l| l.chars().count() == w),
            "every box row must be the same display width"
        );
    }

    #[test]
    fn panel_with_mission_lines_are_uniform_width() {
        // Adding the mission section must keep every box row the same
        // width — the centring offset in `render` relies on it.
        let m = sample_mission();
        let card = panel("Ctrl-A", Some(&m));
        let w = card[0].chars().count();
        assert!(
            card.iter().all(|l| l.chars().count() == w),
            "every box row must be the same display width with mission section"
        );
    }

    #[test]
    fn panel_shows_the_keys_and_relaunch_banner() {
        let text = panel("Ctrl-A", None).join("\n");
        // Title bar.
        assert!(text.contains("aida tui"));
        // The five empty-state keybindings.
        assert!(text.contains("Ctrl-A  N    new session"));
        assert!(text.contains("Ctrl-A  O    status overlay"));
        assert!(text.contains("Ctrl-A  ?    all keybindings"));
        assert!(text.contains("Ctrl-A  D    detach"));
        assert!(text.contains("Ctrl-A  Q    quit"));
        // The relaunch-with-scope banner — a generic placeholder, not a
        // real (internal) spec id. trace:TASK-268
        assert!(text.contains("Press Ctrl-A then N to pick one"));
        assert!(text.contains("aida tui <SCOPE>"));
    }

    #[test]
    fn panel_honours_a_reconfigured_prefix() {
        let text = panel("Ctrl-B", None).join("\n");
        assert!(text.contains("Ctrl-B  N    new session"));
        assert!(text.contains("Press Ctrl-B then N"));
        assert!(!text.contains("Ctrl-A"));
    }

    #[test]
    fn render_into_a_small_terminal_falls_back_without_panicking() {
        let mut buf: Vec<u8> = Vec::new();
        // 20×4 is far too small for the card — must not panic.
        render(&mut buf, 20, 4, "Ctrl-A", None).expect("render succeeds");
        assert!(!buf.is_empty());
    }

    #[test]
    fn render_with_mission_into_small_terminal_does_not_panic() {
        let m = sample_mission();
        let mut buf: Vec<u8> = Vec::new();
        render(&mut buf, 20, 4, "Ctrl-A", Some(&m)).expect("render succeeds");
        assert!(!buf.is_empty());
    }

    // --- mission_section: the pure empty-state assembly (TASK-255) ---

    #[test]
    fn mission_section_shows_role_total_and_queue_head() {
        let lines = mission_section(&sample_mission()).join("\n");
        assert!(lines.contains("Up next — implementer queue (4 total)"));
        assert!(lines.contains("1. STORY-1  first thing  [Approved]"));
        assert!(lines.contains("2. TASK-2  second thing  [Planned]"));
        assert!(lines.contains("Open PRs — 3"));
    }

    #[test]
    fn mission_section_truncates_queue_head_with_more_tail() {
        let mut m = sample_mission();
        m.queue_head = (0..5)
            .map(|i| MissionQueueItem {
                spec_id: format!("TASK-{i}"),
                title: format!("t{i}"),
                status: "Approved".to_string(),
            })
            .collect();
        let lines = mission_section(&m);
        let shown = lines.iter().filter(|l| l.contains("TASK-")).count();
        assert_eq!(shown, MISSION_QUEUE_LIMIT, "head is capped");
        assert!(
            lines.iter().any(|l| l.contains("+2 more")),
            "the remainder is summarised: {lines:?}"
        );
    }

    #[test]
    fn mission_section_empty_queue_shows_placeholder() {
        let m = MissionData {
            role: Some("reviewer".to_string()),
            queue_total: 0,
            queue_head: vec![],
            open_prs: Some(0),
        };
        let lines = mission_section(&m).join("\n");
        assert!(lines.contains("Up next — reviewer queue (0 total)"));
        assert!(lines.contains("queue empty for this role"));
        assert!(lines.contains("Open PRs — none"));
    }

    #[test]
    fn mission_section_unknown_role_and_unavailable_prs_degrade() {
        let m = MissionData::default();
        let lines = mission_section(&m).join("\n");
        // No role → em-dash placeholder, not a crash.
        assert!(lines.contains("Up next — — queue (0 total)"));
        assert!(lines.contains("queue empty for this role"));
        // `gh` couldn't be reached → explicit unavailable line.
        assert!(lines.contains("Open PRs — unavailable"));
    }

    #[test]
    fn mission_section_singular_pr_count() {
        let mut m = MissionData::default();
        m.open_prs = Some(1);
        assert!(mission_section(&m).join("\n").contains("Open PRs — 1"));
    }

    #[test]
    fn parse_open_pr_count_reads_array_length() {
        assert_eq!(parse_open_pr_count(br#"[]"#), Some(0));
        assert_eq!(
            parse_open_pr_count(br#"[{"number":1},{"number":2}]"#),
            Some(2)
        );
        // Non-array / garbage → None (degrades to "unavailable").
        assert_eq!(parse_open_pr_count(br#"{"oops":true}"#), None);
        assert_eq!(parse_open_pr_count(b"not json"), None);
    }
}
