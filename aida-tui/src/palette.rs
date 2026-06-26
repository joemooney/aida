//! Deterministic AIDA action palette — EPIC-51 slice 2 (STORY-679).
//!
//! EPIC-51's vision: from a live hosted chat you press a key to SUSPEND it
//! (Slice 1 / STORY-678 SIGSTOPs the focused child), act on AIDA state
//! INSTANTLY through a deterministic TUI surface — no LLM round-trip — then
//! resume the conversation. This module is that deterministic surface: a
//! fuzzy-filtered command list of curated AIDA actions that each run a fixed
//! `aida …` subprocess (with `--json` where the command supports it) and
//! render the result inline.
//!
//! **Deterministic** is the whole point. Where the status overlay's drains
//! (STORY-136) type a slash command into a Claude session — an LLM
//! round-trip — every palette action here maps to an explicit argv via
//! [`PaletteAction::dispatch`] / [`dispatch_query`] and runs as a captured
//! subprocess (`crate::actions::run_argv`). No `claude` is ever spawned, no
//! prompt is sent: a selection is a command, full stop.
//!
//! Three layers, all pure + unit-testable here (the terminal wiring lives in
//! `app.rs`'s `Mode::Paused` handling):
//!
//! 1. [`PaletteAction`] — the curated verb set the spec names
//!    (`queue / punts / findings / status / show`), each with a label, an
//!    `about` blurb, and a fixed argv.
//! 2. [`PaletteState`] — the input buffer + the fuzzy-ranked, selectable
//!    list. Typing filters; arrows move; Enter dispatches the selection (or
//!    a parametric `spec <ID>` / `run <cmd>` query).
//! 3. [`dispatch_query`] — turns the typed line into an argv, honouring the
//!    parametric forms and gating `run`/`spec` payloads through the same
//!    [`crate::intent::is_safe_payload`] allow-list the launcher uses, so no
//!    shell metacharacter ever reaches a spawned child.
//!
//! trace:STORY-679 | ai:claude

use crate::actions::ActivityEntry;
use crate::cmd_palette::fuzzy_score;
use crate::intent::is_safe_payload;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

/// One curated, deterministic AIDA action invocable from the suspended-chat
/// palette. Each maps to a fixed `aida …` argv (with `--json` where the
/// command supports it) — never an LLM prompt. Keep this set small +
/// high-signal; it is a quick-action menu, not the whole CLI surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    /// `aida queue list --json` — the active role's work queue.
    Queue,
    /// `aida punts list` — open punts awaiting a decision.
    Punts,
    /// `aida findings list` — shelved/triage findings.
    Findings,
    /// `aida status --json` — the project snapshot.
    Status,
    /// `aida list --json` — open requirements.
    List,
    /// `aida history` — recent activity.
    History,
}

impl PaletteAction {
    /// Every action, in menu order. The empty-query palette shows these in
    /// this order; a non-empty query fuzzy-ranks over them.
    pub const ALL: [PaletteAction; 6] = [
        Self::Queue,
        Self::Punts,
        Self::Findings,
        Self::Status,
        Self::List,
        Self::History,
    ];

    /// The keyword a user types to match this action — also the activity-log
    /// heading and the fuzzy-match target.
    pub fn keyword(self) -> &'static str {
        match self {
            PaletteAction::Queue => "queue",
            PaletteAction::Punts => "punts",
            PaletteAction::Findings => "findings",
            PaletteAction::Status => "status",
            PaletteAction::List => "list",
            PaletteAction::History => "history",
        }
    }

    /// One-line description shown beside the keyword in the list.
    pub fn about(self) -> &'static str {
        match self {
            PaletteAction::Queue => "the active role's work queue",
            PaletteAction::Punts => "open punts awaiting a decision",
            PaletteAction::Findings => "shelved / triage findings",
            PaletteAction::Status => "project snapshot",
            PaletteAction::List => "open requirements",
            PaletteAction::History => "recent activity",
        }
    }

    /// The deterministic argv this action runs, given the running `aida`
    /// binary path. Always a plain `aida …` invocation — no LLM, no
    /// `claude`. Commands that support a machine-stable `--json` projection
    /// (`queue list`, `status`, `list`) request it; the others
    /// (`punts list`, `findings list`, `history`) have no `--json` flag and
    /// run in their default human-readable form, which the palette renders
    /// inline just the same. (Adding `--json` where it isn't a real flag
    /// would make the action fail with an arg error — so the surface is
    /// kept honest against the actual CLI.)
    pub fn dispatch(self, aida_exe: &str) -> Vec<String> {
        let s = |v: &str| v.to_string();
        match self {
            PaletteAction::Queue => vec![s(aida_exe), s("queue"), s("list"), s("--json")],
            PaletteAction::Status => vec![s(aida_exe), s("status"), s("--json")],
            PaletteAction::List => vec![s(aida_exe), s("list"), s("--json")],
            PaletteAction::Punts => vec![s(aida_exe), s("punts"), s("list")],
            PaletteAction::Findings => vec![s(aida_exe), s("findings"), s("list")],
            PaletteAction::History => vec![s(aida_exe), s("history")],
        }
    }

    /// Whether this action requests a `--json` projection (true only for the
    /// commands whose CLI actually accepts it). Used by tests to keep the
    /// argv set honest against the real surface.
    #[cfg(test)]
    pub fn requests_json(self) -> bool {
        matches!(self, Self::Queue | Self::Status | Self::List)
    }
}

/// A [`PaletteAction`] paired with its fuzzy [`score`](Ranked::score) for the
/// current query. Higher score = better match; comparable only within one
/// [`PaletteState::rank`] pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ranked {
    /// The matched action.
    pub action: PaletteAction,
    /// Fuzzy score (higher is better).
    pub score: i32,
}

/// What a dispatched palette line resolves to — the deterministic command to
/// run, or a reason it was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatched {
    /// Run this argv as a captured subprocess (`crate::actions::run_argv`).
    /// `label` is the activity-log heading.
    Run { label: String, argv: Vec<String> },
    /// The line could not be dispatched — show this note, run nothing.
    Refused(String),
}

/// Turn a typed palette line into a deterministic dispatch. Three shapes:
///
/// - `spec <ID>`  → `aida show <ID> --json` (the spec's detail).
/// - `run <cmd>`  → the raw `<cmd>` tokenised as argv (an escape hatch for
///   arbitrary read-only `aida …` / `gh …`); spawned directly, no shell.
/// - anything else → the top fuzzy-ranked [`PaletteAction`]'s dispatch, so a
///   bare `q` runs the queue action.
///
/// Parametric `run`/`spec` payloads are gated through
/// [`crate::intent::is_safe_payload`] (the launcher's allow-list — no shell
/// metacharacters), so a malformed line is [`Dispatched::Refused`] rather
/// than reaching a spawned child.
///
/// `aida_exe` is the running binary path (mirrors `crate::app::aida_exe`).
pub fn dispatch_query(query: &str, aida_exe: &str) -> Dispatched {
    let q = query.trim();

    // Parametric: `spec <ID>` → aida show <ID> --json.
    if let Some(rest) = q.strip_prefix("spec ").or_else(|| q.strip_prefix("show ")) {
        let id = rest.trim();
        if id.is_empty() {
            return Dispatched::Refused("spec: missing a spec id (try `spec STORY-1`)".to_string());
        }
        if !is_safe_payload(id) {
            return Dispatched::Refused(format!("spec: refused unsafe id {id:?}"));
        }
        return Dispatched::Run {
            label: format!("show {id}"),
            argv: vec![
                aida_exe.to_string(),
                "show".to_string(),
                id.to_string(),
                "--json".to_string(),
            ],
        };
    }

    // Parametric: `run <cmd>` → the raw command, tokenised, spawned directly.
    if let Some(rest) = q.strip_prefix("run ") {
        let cmd = rest.trim();
        if cmd.is_empty() {
            return Dispatched::Refused(
                "run: missing a command (try `run aida cache status`)".to_string(),
            );
        }
        if !is_safe_payload(cmd) {
            return Dispatched::Refused(format!(
                "run: refused — the command contains shell metacharacters: {cmd:?}"
            ));
        }
        let argv: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
        return Dispatched::Run {
            label: format!("run {cmd}"),
            argv,
        };
    }

    // Otherwise: dispatch the top fuzzy-ranked curated action.
    match rank(q).first() {
        Some(top) => Dispatched::Run {
            label: top.action.keyword().to_string(),
            argv: top.action.dispatch(aida_exe),
        },
        None => Dispatched::Refused(format!("no action matches {q:?}")),
    }
}

/// Fuzzy-rank the curated [`PaletteAction::ALL`] against `query`, best-first.
///
/// An empty (or whitespace-only) query returns every action in
/// [`PaletteAction::ALL`] order — the sensible cold-open list. A non-empty
/// query keeps only the actions whose keyword subsequence-matches it
/// (via [`crate::cmd_palette::fuzzy_score`]), sorted by score then keyword
/// for determinism.
///
/// Parametric prefixes (`spec …`, `run …`) are NOT ranked here — they are
/// handled by [`dispatch_query`]; `rank` is for the visible filter list.
pub fn rank(query: &str) -> Vec<Ranked> {
    let q = query.trim();
    if q.is_empty() {
        return PaletteAction::ALL
            .iter()
            .map(|&action| Ranked { action, score: 0 })
            .collect();
    }
    let mut scored: Vec<Ranked> = PaletteAction::ALL
        .iter()
        .filter_map(|&action| {
            fuzzy_score(q, action.keyword()).map(|score| Ranked { action, score })
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.action.keyword().cmp(b.action.keyword()))
    });
    scored
}

/// The interactive state of the suspended-chat palette: the typed query, the
/// fuzzy-filtered candidate list, and the highlighted index. Pure — `app.rs`
/// drives it from key events and renders it; this carries no terminal.
#[derive(Debug, Clone)]
pub struct PaletteState {
    /// What the user has typed after the `:` prompt.
    pub query: String,
    /// Index of the highlighted candidate in [`PaletteState::ranked`].
    pub selected: usize,
}

impl Default for PaletteState {
    fn default() -> Self {
        PaletteState::new()
    }
}

impl PaletteState {
    /// A fresh palette: empty query, selection at the top.
    pub fn new() -> Self {
        PaletteState {
            query: String::new(),
            selected: 0,
        }
    }

    /// The current fuzzy-ranked candidate list for the typed query.
    pub fn ranked(&self) -> Vec<Ranked> {
        rank(&self.query)
    }

    /// Append a typed character and reset the selection to the top (the
    /// filtered list just changed, so the prior index may be stale).
    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.selected = 0;
    }

    /// Delete the last typed character; reset the selection to the top.
    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    /// Move the highlight down one, wrapping. No-op on an empty list.
    pub fn select_next(&mut self) {
        let n = self.ranked().len();
        if n == 0 {
            self.selected = 0;
        } else {
            self.selected = (self.selected + 1) % n;
        }
    }

    /// Move the highlight up one, wrapping. No-op on an empty list.
    pub fn select_prev(&mut self) {
        let n = self.ranked().len();
        if n == 0 {
            self.selected = 0;
        } else {
            self.selected = (self.selected + n - 1) % n;
        }
    }

    /// Dispatch the current line to a deterministic command.
    ///
    /// When the query is a bare filter, the *highlighted* candidate wins (so
    /// arrowing to `findings` and pressing Enter runs findings, even though
    /// `queue` might fuzzy-rank first). Parametric `spec …` / `run …` lines
    /// always go through [`dispatch_query`]. `aida_exe` is the running binary.
    pub fn dispatch(&self, aida_exe: &str) -> Dispatched {
        let q = self.query.trim();
        // Parametric forms are not part of the highlightable list — route
        // them straight through.
        if q.starts_with("spec ") || q.starts_with("show ") || q.starts_with("run ") || q.is_empty()
        {
            // Empty query: run the highlighted cold-open action below.
            if !q.is_empty() {
                return dispatch_query(q, aida_exe);
            }
        }
        // Bare filter: dispatch the highlighted candidate.
        let ranked = self.ranked();
        match ranked.get(self.selected).or_else(|| ranked.first()) {
            Some(top) => Dispatched::Run {
                label: top.action.keyword().to_string(),
                argv: top.action.dispatch(aida_exe),
            },
            None => dispatch_query(q, aida_exe),
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering — the `ratatui` view of the suspended-chat palette.
// ---------------------------------------------------------------------------

/// Render the deterministic action palette while the chat is suspended.
///
/// Four stacked panels: a `[paused]` header + the `:` query line, the
/// fuzzy-ranked candidate list (the highlighted row marked), the most recent
/// action result, and a key-hint footer. `last` is the newest activity-log
/// entry (the result of the previously-run action), or `None` before any
/// action has run this pause.
//
// trace:STORY-679 | ai:claude
pub fn render(frame: &mut Frame, state: &PaletteState, last: Option<&ActivityEntry>) {
    let area = frame.area();
    let rows = Layout::vertical([
        Constraint::Length(2), // header + query line
        Constraint::Min(3),    // candidate list
        Constraint::Min(3),    // result pane
        Constraint::Length(1), // hints
    ])
    .split(area);

    render_query(frame, rows[0], state);
    render_candidates(frame, rows[1], state);
    render_result(frame, rows[2], last);
    render_hints(frame, rows[3]);
}

/// The `[paused]` banner and the live `:`-prefixed query the user is typing.
fn render_query(frame: &mut Frame, area: Rect, state: &PaletteState) {
    let lines = vec![
        Line::from(vec![
            Span::styled(
                "[paused] ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("AIDA action palette -- deterministic, no LLM", dim()),
        ]),
        Line::from(vec![
            Span::styled(":", Style::default().fg(Color::Cyan)),
            Span::styled(
                state.query.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            // A block caret so the input line reads as active.
            Span::styled("_", Style::default().fg(Color::Cyan)),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

/// The fuzzy-ranked candidate list; the highlighted row is reverse-styled.
fn render_candidates(frame: &mut Frame, area: Rect, state: &PaletteState) {
    let block = Block::bordered().title(" Actions ");
    let ranked = state.ranked();
    let inner_h = area.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();

    if ranked.is_empty() {
        // A bare filter matched nothing — but `spec <ID>` / `run <cmd>` are
        // still dispatchable, so say so rather than implying a dead end.
        let q = state.query.trim();
        let note = if q.starts_with("spec ") || q.starts_with("show ") || q.starts_with("run ") {
            "press enter to run this command"
        } else {
            "no action matches -- try `spec <ID>` or `run <cmd>`"
        };
        lines.push(Line::from(Span::styled(note, dim())));
    } else {
        for (i, r) in ranked.iter().enumerate().take(inner_h.max(1)) {
            let selected = i == state.selected;
            let row_style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let marker = if selected { "> " } else { "  " };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker}{:<9}", r.action.keyword()), row_style),
                Span::raw(" "),
                Span::styled(r.action.about().to_string(), dim()),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The most recent action's captured output, tailed + per-line clipped.
fn render_result(frame: &mut Frame, area: Rect, last: Option<&ActivityEntry>) {
    let block = Block::bordered().title(" Result ");
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();

    match last {
        None => {
            lines.push(Line::from(Span::styled(
                "type to filter, enter to run -- the result lands here",
                dim(),
            )));
        }
        Some(entry) => {
            let (tag, tag_style) = if entry.ok {
                ("ok ", Style::default().fg(Color::Green))
            } else {
                ("x  ", Style::default().fg(Color::Red))
            };
            let mut header = vec![
                Span::styled(format!("{} ", entry.when), dim()),
                Span::styled(tag, tag_style.add_modifier(Modifier::BOLD)),
                Span::styled(
                    entry.label.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ];
            if !entry.command.is_empty() {
                header.push(Span::styled(format!("   $ {}", entry.command), dim()));
            }
            lines.push(Line::from(header));
            for out_line in &entry.lines {
                lines.push(Line::from(format!(
                    "  {}",
                    clip(out_line, inner_w.saturating_sub(2))
                )));
            }
        }
    }

    // Tail — the newest output stays visible if it overflows the pane.
    if inner_h > 0 && lines.len() > inner_h {
        lines = lines.split_off(lines.len() - inner_h);
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// The key-hint footer.
fn render_hints(frame: &mut Frame, area: Rect) {
    let hint = Line::from(vec![
        Span::styled(" type ", Style::default().fg(Color::Cyan)),
        Span::styled("filter   ", dim()),
        Span::styled("up/down ", Style::default().fg(Color::Cyan)),
        Span::styled("select   ", dim()),
        Span::styled("enter ", Style::default().fg(Color::Cyan)),
        Span::styled("run   ", dim()),
        Span::styled("esc ", Style::default().fg(Color::Cyan)),
        Span::styled("resume the conversation", dim()),
    ]);
    frame.render_widget(Paragraph::new(hint), area);
}

/// Dim style for secondary text (mirrors `overlay::dim`).
fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Clip `s` to `max` columns, adding an ellipsis only when actually cut
/// (mirrors `overlay::clip`).
fn clip(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let take = max.saturating_sub(1);
    let mut out: String = s.chars().take(take).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keywords(ranked: &[Ranked]) -> Vec<&'static str> {
        ranked.iter().map(|r| r.action.keyword()).collect()
    }

    // --- ranking / filtering --------------------------------------------

    #[test]
    fn empty_query_shows_all_actions_in_order() {
        let r = rank("");
        assert_eq!(
            keywords(&r),
            vec!["queue", "punts", "findings", "status", "list", "history"]
        );
        // whitespace-only is treated as empty
        assert_eq!(keywords(&rank("   ")), keywords(&r));
    }

    #[test]
    fn fuzzy_filters_actions_by_query() {
        // "find" subsequence-matches only "findings".
        let r = rank("find");
        assert_eq!(keywords(&r), vec!["findings"]);
        // "que" → queue (prefix); nothing else has q…u…e in order.
        assert_eq!(rank("que").first().unwrap().action, PaletteAction::Queue);
    }

    #[test]
    fn non_matching_query_filters_everything_out() {
        assert!(rank("zzzzz").is_empty());
    }

    #[test]
    fn ranking_is_score_descending() {
        let r = rank("s");
        for w in r.windows(2) {
            assert!(w[0].score >= w[1].score, "scores must be non-increasing");
        }
    }

    // --- deterministic dispatch (no LLM) --------------------------------

    #[test]
    fn dispatch_queue_action_is_aida_queue_list_json() {
        assert_eq!(
            PaletteAction::Queue.dispatch("/opt/aida"),
            vec!["/opt/aida", "queue", "list", "--json"]
        );
    }

    #[test]
    fn dispatch_argvs_match_the_real_cli_surface() {
        // The `--json`-supporting commands request it…
        assert_eq!(
            PaletteAction::Status.dispatch("aida"),
            vec!["aida", "status", "--json"]
        );
        assert_eq!(
            PaletteAction::List.dispatch("aida"),
            vec!["aida", "list", "--json"]
        );
        // …and the ones whose CLI has no `--json` flag run plain, so the
        // action can't fail with an arg error.
        assert_eq!(
            PaletteAction::Punts.dispatch("aida"),
            vec!["aida", "punts", "list"]
        );
        assert_eq!(
            PaletteAction::Findings.dispatch("aida"),
            vec!["aida", "findings", "list"]
        );
        assert_eq!(
            PaletteAction::History.dispatch("aida"),
            vec!["aida", "history"]
        );
    }

    #[test]
    fn dispatch_is_deterministic_and_never_spawns_an_llm() {
        // The defining property of EPIC-51 slice 2: zero LLM round-trip.
        // Every curated action's argv is a plain `aida …` invocation —
        // never `claude`, never a prompt. `--json` is requested exactly
        // when the command actually supports it (kept honest by
        // `requests_json`).
        for action in PaletteAction::ALL {
            let argv = action.dispatch("aida");
            assert_eq!(argv[0], "aida", "must invoke the aida binary, not an LLM");
            assert_eq!(
                argv.iter().any(|a| a == "--json"),
                action.requests_json(),
                "{:?} --json presence must match the real CLI surface",
                action
            );
            assert!(
                !argv.iter().any(|a| a == "claude" || a.contains("--resume")),
                "no palette action may spawn an LLM session: {argv:?}"
            );
        }
    }

    #[test]
    fn dispatch_spec_param_builds_show_json() {
        match dispatch_query("spec STORY-679", "aida") {
            Dispatched::Run { label, argv } => {
                assert_eq!(label, "show STORY-679");
                assert_eq!(argv, vec!["aida", "show", "STORY-679", "--json"]);
            }
            other => panic!("expected Run, got {other:?}"),
        }
        // `show <ID>` is an accepted alias for `spec <ID>`.
        assert!(matches!(
            dispatch_query("show TASK-1", "aida"),
            Dispatched::Run { .. }
        ));
    }

    #[test]
    fn dispatch_spec_rejects_empty_or_unsafe_id() {
        assert!(matches!(
            dispatch_query("spec ", "aida"),
            Dispatched::Refused(_)
        ));
        // shell metacharacters never reach a child
        assert!(matches!(
            dispatch_query("spec STORY-1;rm", "aida"),
            Dispatched::Refused(_)
        ));
    }

    #[test]
    fn dispatch_run_param_passes_through_tokenised() {
        match dispatch_query("run aida cache status", "aida") {
            Dispatched::Run { label, argv } => {
                assert_eq!(label, "run aida cache status");
                assert_eq!(argv, vec!["aida", "cache", "status"]);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_run_rejects_metacharacters() {
        for bad in [
            "run aida; rm -rf /",
            "run $(whoami)",
            "run aida | grep x",
            "run aida && touch /tmp/x",
            "run `whoami`",
        ] {
            assert!(
                matches!(dispatch_query(bad, "aida"), Dispatched::Refused(_)),
                "must refuse {bad:?}"
            );
        }
    }

    #[test]
    fn dispatch_bare_filter_runs_top_action() {
        // A bare `q` with no parametric prefix dispatches the top-ranked
        // curated action (queue).
        match dispatch_query("queue", "aida") {
            Dispatched::Run { label, argv } => {
                assert_eq!(label, "queue");
                assert_eq!(argv, vec!["aida", "queue", "list", "--json"]);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_bare_unknown_is_refused() {
        assert!(matches!(
            dispatch_query("zzzzz", "aida"),
            Dispatched::Refused(_)
        ));
    }

    // --- PaletteState interaction ---------------------------------------

    #[test]
    fn typing_filters_and_resets_selection() {
        let mut p = PaletteState::new();
        assert_eq!(p.ranked().len(), PaletteAction::ALL.len());
        p.select_next();
        assert_eq!(p.selected, 1);
        // Typing narrows the list and snaps the selection back to the top.
        for c in "find".chars() {
            p.push_char(c);
        }
        assert_eq!(p.selected, 0);
        assert_eq!(keywords(&p.ranked()), vec!["findings"]);
    }

    #[test]
    fn backspace_widens_the_list() {
        let mut p = PaletteState::new();
        for c in "find".chars() {
            p.push_char(c);
        }
        assert_eq!(p.ranked().len(), 1);
        p.backspace(); // "fin" still only matches findings
        p.backspace(); // "fi"
        p.backspace(); // "f"
        p.backspace(); // "" → all
        assert!(p.query.is_empty());
        assert_eq!(p.ranked().len(), PaletteAction::ALL.len());
    }

    #[test]
    fn selection_wraps_both_ways() {
        let mut p = PaletteState::new();
        let n = p.ranked().len();
        p.select_prev(); // wrap from 0 to n-1
        assert_eq!(p.selected, n - 1);
        p.select_next(); // wrap back to 0
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn selection_is_safe_on_empty_filtered_list() {
        let mut p = PaletteState::new();
        for c in "zzzzz".chars() {
            p.push_char(c);
        }
        assert!(p.ranked().is_empty());
        // No panic / underflow on an empty list.
        p.select_next();
        p.select_prev();
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn state_dispatch_runs_the_highlighted_candidate() {
        let mut p = PaletteState::new();
        // empty query: highlight punts (index 1) and dispatch it.
        p.select_next();
        assert_eq!(p.ranked()[p.selected].action, PaletteAction::Punts);
        match p.dispatch("aida") {
            Dispatched::Run { label, argv } => {
                assert_eq!(label, "punts");
                assert_eq!(argv, vec!["aida", "punts", "list"]);
            }
            other => panic!("expected Run(punts), got {other:?}"),
        }
    }

    #[test]
    fn state_dispatch_routes_parametric_lines() {
        let mut p = PaletteState::new();
        for c in "spec STORY-1".chars() {
            p.push_char(c);
        }
        match p.dispatch("aida") {
            Dispatched::Run { argv, .. } => {
                assert_eq!(argv, vec!["aida", "show", "STORY-1", "--json"]);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }
}
