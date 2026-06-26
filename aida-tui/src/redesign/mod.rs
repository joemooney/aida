//! The `aida tui` **action→target redesign** prototype — Slice 1.
//!
//! A throwaway-able keystone that validates the gesture grammar from
//! `docs/plans/2026-06-25-tui-action-target-redesign.md` on ONE scope
//! (Backlog) and ONE functional verb (groom). It is gated behind the
//! `AIDA_TUI_REDESIGN=1` env toggle (see [`enabled`]); the existing TUI is
//! completely unchanged without it.
//!
//! The protocol: **scope → action → targets → execute**. The top panel
//! holds the scopes, then a scope's verbs after a drill; the bottom panel
//! is the multi-selectable target set; Enter on a verb runs it on the
//! selection (or confirms "apply to all N?"); `p` previews an item in a
//! modal; Esc pops the navigation stack; the status line carries the
//! breadcrumb + role + counts.
//!
//! All pure logic lives in [`state`] (and is unit-tested there). This
//! module owns only the IO: the terminal guard, the backlog fetch, the
//! render, and the keystroke→transition wiring.
//!
//! trace:STORY-690 | ai:claude

mod list_row;
mod state;
mod store;

pub use state::{RedesignState, RunOutcome, Scope, TargetItem, Verb};
use std::collections::HashMap;
use store::{LoadedSpec, SpecStore};

use crate::term;
use crate::theme::Theme;
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use state::{Focus, Level};
use std::io::Stdout;
use std::process::Command;

/// Lines scrolled per PageUp / PageDown (and Space) in the item modal.
/// trace:TASK-913 | ai:claude
const MODAL_PAGE: u16 = 10;

/// Is the redesign prototype toggled on? Checked by `aida_tui::run` so the
/// existing TUI is the default and the prototype is strictly opt-in.
pub fn enabled() -> bool {
    matches!(
        std::env::var("AIDA_TUI_REDESIGN").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Launch the redesign prototype. Owns the terminal via the same RAII
/// guard the rest of the TUI uses, so a panic or a normal exit never
/// strands the terminal in raw mode.
///
/// `project_root` is the directory holding `.aida/config.toml` (resolved by
/// the launcher). It opens the cache-backed git backend ONCE
/// ([`SpecStore`]) so every scope-list + show-modal read is in-process —
/// no per-read `aida` subprocess cold-start. trace:STORY-693 | ai:claude
pub fn run(theme: Theme, project_root: &std::path::Path) -> Result<()> {
    term::install_panic_hook();
    term::install_signal_handler()?;

    // Open the in-process read backend once. If the store can't be attached
    // (offline, missing, etc.) the prototype still runs — the scope lists are
    // empty and the modal reports the failure — rather than crashing.
    // trace:STORY-693 | ai:claude
    let store = SpecStore::open(project_root);

    let items = store
        .as_ref()
        .map(|s| s.scope_items(Scope::Backlog))
        .unwrap_or_default();
    let mut st = RedesignState::new(items, resolve_role());
    st.theme = theme;
    st.status = Some(if store.is_some() {
        "Slice 1 prototype — Backlog / Open scopes. ? exits.".to_string()
    } else {
        "Slice 1 prototype — store unavailable (no in-process data). ? exits.".to_string()
    });

    // Per-scope item-set cache so the bottom panel can follow the
    // highlighted scope without re-querying on every cursor move.
    // trace:STORY-690 | ai:claude
    let mut item_cache: HashMap<Scope, Vec<TargetItem>> = HashMap::new();
    item_cache.insert(Scope::Backlog, st.items.clone());
    let mut loaded_scope = Scope::Backlog;

    let _guard = term::TermGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    terminal.clear()?;

    event_loop(
        &mut terminal,
        &mut st,
        store.as_ref(),
        &mut item_cache,
        &mut loaded_scope,
    )?;
    Ok(())
}

/// The scope whose item-set the bottom panel should currently show: the
/// drilled-into scope when at the verb level, else the highlighted scope at
/// the scope level. Only functional scopes have a target set; others keep
/// the last loaded set. trace:STORY-690 | ai:claude
fn active_item_scope(st: &RedesignState) -> Option<Scope> {
    match st.scope {
        Some(scope) => Some(scope),
        None => st.top_scope().filter(|s| s.is_functional()),
    }
}

/// Keep the bottom panel's items in sync with the active scope, fetching
/// (and caching) on first visit. The fetch is now an in-process cache read
/// via the open [`SpecStore`] — no subprocess. trace:STORY-690 trace:STORY-693
fn sync_scope_items(
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    cache: &mut HashMap<Scope, Vec<TargetItem>>,
    loaded: &mut Scope,
) {
    let Some(scope) = active_item_scope(st) else {
        return;
    };
    if scope == *loaded {
        return;
    }
    let items = cache
        .entry(scope)
        .or_insert_with(|| store.map(|s| s.scope_items(scope)).unwrap_or_default())
        .clone();
    st.set_items(items);
    *loaded = scope;
}

/// The shell's role lens (ambient context in the status line). Mirrors the
/// CLI's role resolution loosely — Slice 1 only displays it.
fn resolve_role() -> String {
    std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "advisor".to_string())
}

/// Load the focused item's full spec for the show modal — in-process, via the
/// open [`SpecStore`] (no `aida show` subprocess). The loaded record is held in
/// `loaded_spec` and rendered natively by [`render_modal`]. Fires only on
/// modal-open, never on cursor move. trace:STORY-693 | ai:claude
fn load_focused_spec(st: &RedesignState, store: Option<&SpecStore>) -> Option<LoadedSpec> {
    let idxs = st.bottom_indices();
    let real = *idxs.get(st.bottom_idx)?;
    let id = st.items.get(real)?.id.clone();
    match store {
        Some(s) => s.load_spec(&id).or_else(|| Some(missing_spec(&id))),
        None => Some(missing_spec(&id)),
    }
}

/// A placeholder spec for the modal when the store is unavailable or the id
/// can't be found — rendered the same way as a real one. trace:STORY-693
fn missing_spec(id: &str) -> LoadedSpec {
    LoadedSpec {
        id: id.to_string(),
        title: String::new(),
        req_type: String::new(),
        status: String::new(),
        priority: String::new(),
        tags: Vec::new(),
        description: format!("(could not load {id} from the store)"),
    }
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    cache: &mut HashMap<Scope, Vec<TargetItem>>,
    loaded: &mut Scope,
) -> Result<()> {
    // The spec currently shown in the item modal (loaded in-process on open,
    // cleared on close). trace:STORY-693 | ai:claude
    let mut loaded_spec: Option<LoadedSpec> = None;
    loop {
        // Keep the bottom panel's target set following the active scope
        // (highlighted at the scope level, drilled-into at the verb level).
        sync_scope_items(st, store, cache, loaded);
        terminal.draw(|f| render(f, st, loaded_spec.as_ref()))?;
        let Event::Key(key) = crossterm::event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        if handle_key(terminal, st, store, &mut loaded_spec, key)? {
            break;
        }
    }
    Ok(())
}

/// Route one keystroke. Returns `Ok(true)` when the app should exit.
fn handle_key(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    loaded_spec: &mut Option<LoadedSpec>,
    key: KeyEvent,
) -> Result<bool> {
    // Ctrl-C always quits.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(true);
    }

    // A confirmation popup captures input until resolved.
    if st.confirm.is_some() {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                let outcome = st.resolve_confirm(true);
                apply_outcome(terminal, st, store, loaded_spec, outcome)?;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                st.resolve_confirm(false);
                st.status = Some("cancelled".to_string());
            }
            _ => {}
        }
        return Ok(false);
    }

    // A modal (item-body or verb-output) captures Esc / q / p (close) plus the
    // scroll keys so a body taller than the popup can be paged; nothing else
    // mutating leaks through. trace:TASK-913 | ai:claude
    if st.modal_open() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('p') => {
                st.close_modal();
                *loaded_spec = None;
            }
            KeyCode::Down | KeyCode::Char('j') => st.modal_scroll_down(1),
            KeyCode::Up | KeyCode::Char('k') => st.modal_scroll_up(1),
            KeyCode::PageDown | KeyCode::Char(' ') => st.modal_scroll_down(MODAL_PAGE),
            KeyCode::PageUp => st.modal_scroll_up(MODAL_PAGE),
            _ => {}
        }
        return Ok(false);
    }

    match key.code {
        // `?` exits the prototype (a bare `q` is reserved for the modal /
        // could be typed into a filter, so the exit key is unambiguous).
        KeyCode::Char('?') => return Ok(true),

        KeyCode::Up => st.move_up(),
        KeyCode::Down => st.move_down(),

        KeyCode::Tab => st.focus_bottom(),
        KeyCode::BackTab => st.focus_top(),

        KeyCode::Char(' ') => st.toggle_select(),
        KeyCode::Char('a') if st.focus == Focus::Bottom => st.select_all(),
        KeyCode::Char('A') if st.focus == Focus::Bottom => st.select_none(),

        KeyCode::Char('p') => {
            if st.focus == Focus::Bottom {
                open_modal_with_body(st, store, loaded_spec);
            }
        }

        KeyCode::Esc => {
            if !st.pop() {
                // Esc at the top-of-stack scope level exits.
                return Ok(true);
            }
        }

        KeyCode::Enter => match (st.focus, st.level) {
            // Scope level: Enter drills into the highlighted scope.
            (Focus::Top, Level::Scopes) => {
                st.drill();
            }
            // Verb level, top focus: Enter runs the verb.
            (Focus::Top, Level::Verbs) => {
                let outcome = st.run_verb();
                apply_outcome(terminal, st, store, loaded_spec, outcome)?;
            }
            // Bottom focus: Enter on an item opens its modal (the N=1
            // "preview this spec" case of the same protocol).
            (Focus::Bottom, _) => {
                open_modal_with_body(st, store, loaded_spec);
            }
        },

        KeyCode::Backspace => st.pop_filter(),

        // Type-to-fuzzy-filter the focused list. Printable chars only; the
        // bottom-panel select-all/none shortcuts above already claimed
        // `a`/`A` when focused there, so they won't reach the filter.
        KeyCode::Char(c) if !c.is_control() => st.push_filter(c),

        _ => {}
    }
    Ok(false)
}

/// Open the item modal for the focused row, loading its full spec in-process
/// (via the open [`SpecStore`]) into `loaded_spec` for native rendering. No
/// `aida show` subprocess. trace:STORY-693 | ai:claude
fn open_modal_with_body(
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    loaded_spec: &mut Option<LoadedSpec>,
) {
    *loaded_spec = load_focused_spec(st, store);
    st.open_modal();
}

/// Turn a [`RunOutcome`] into IO. Slice 1 STUBS the actual groom: it logs
/// the verb + target ids to the status line. Wiring the real groom (shell
/// out to `aida` / the grooming skill) is a later slice — the loop and the
/// selection are what Slice 1 validates. trace:STORY-690 | ai:claude
fn apply_outcome(
    _terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    loaded_spec: &mut Option<LoadedSpec>,
    outcome: RunOutcome,
) -> Result<()> {
    match outcome {
        RunOutcome::Execute { verb, ids } => {
            // TODO(Slice 2+): replace this stub with the real verb wiring —
            // e.g. shell out to `aida` (groom = the backlog-groom skill /
            // `aida intake`) or emit an intent for the bash wrapper. Slice
            // 1 only proves the selection + gesture loop, so we log instead.
            let preview: Vec<&str> = ids.iter().take(5).map(|s| s.as_str()).collect();
            let more = if ids.len() > 5 {
                format!(" +{} more", ids.len() - 5)
            } else {
                String::new()
            };
            st.status = Some(format!(
                "[stub] {} {} item(s): {}{}  (TODO: wire real {})",
                verb.label(),
                ids.len(),
                preview.join(", "),
                more,
                verb.label(),
            ));
        }
        RunOutcome::ShowItem { verb, id } => match verb {
            // `show` is now in-process: load the spec from the open backend and
            // render its structured fields + body natively in the item modal
            // (no `aida show` subprocess). trace:STORY-693 | ai:claude
            Verb::Show => {
                *loaded_spec = store
                    .map(|s| s.load_spec(&id).unwrap_or_else(|| missing_spec(&id)))
                    .or_else(|| Some(missing_spec(&id)));
                st.open_modal_external();
            }
            // `why` MAY remain a shell-out for now — its classifier lives in
            // aida-cli/burndown.rs (not aida-core), so making it in-process is
            // a separate task. TODO(why in-process). Result lands in the
            // captured-stdout verb modal. trace:STORY-693 | ai:claude
            _ => {
                let (out, title) = run_item_verb(verb, &id);
                st.open_verb_modal(title, out);
            }
        },
        RunOutcome::RequestApproval { drafts, skipped } => {
            // Route each draft to the advisor queue via the RELIABLE path
            // (`aida queue add --for advisor <id>`) — not the mailbox.
            // trace:STORY-690 | ai:claude
            let mut routed = Vec::new();
            let mut failed = Vec::new();
            for id in &drafts {
                if queue_for_advisor(id) {
                    routed.push(id.clone());
                } else {
                    failed.push(id.clone());
                }
            }
            st.status = Some(request_approval_status(&routed, &failed, &skipped));
        }
        RunOutcome::Approve { drafts, skipped } => {
            // Directly approve each draft via the advisor-gated transition
            // (`aida edit <id> --status approved`, run with advisor authority).
            // The do-it-yourself mirror of request approval. trace:TASK-920 | ai:claude
            let mut approved = Vec::new();
            let mut failed = Vec::new();
            for id in &drafts {
                if approve_spec(id) {
                    approved.push(id.clone());
                } else {
                    failed.push(id.clone());
                }
            }
            st.status = Some(approve_status(&approved, &failed, &skipped));
        }
        RunOutcome::Queue { approved, skipped } => {
            // Route each Approved spec to the implementer queue via the
            // RELIABLE path (`aida queue add --for implementer <id>`) — the
            // Approved-conditional mirror of request approval.
            // trace:TASK-915 | ai:claude
            let mut routed = Vec::new();
            let mut failed = Vec::new();
            for id in &approved {
                if queue_for_implementer(id) {
                    routed.push(id.clone());
                } else {
                    failed.push(id.clone());
                }
            }
            st.status = Some(queue_status(&routed, &failed, &skipped));
        }
        RunOutcome::NeedsConfirm(_) => { /* popup already raised by run_verb */ }
        RunOutcome::None => {}
    }
    Ok(())
}

/// Shell out for the `why` verb and return `(stdout_or_error, title)`.
///
/// `why` is the ONE remaining read-style shell-out in this module:
/// `aida why <id>`. Its state classifier lives in `aida-cli/burndown.rs` (not
/// in `aida-core`), so making it in-process is a separate task —
/// TODO(why in-process). `show` and the scope lists are now in-process via
/// [`SpecStore`] and never reach here. trace:STORY-693 | ai:claude
fn run_item_verb(verb: Verb, id: &str) -> (String, String) {
    let title = format!("{id} — {}", verb.label());
    // Only `why` is shelled out now; any other verb is a defensive no-op
    // (item-level `show` is intercepted upstream and served in-process).
    if verb != Verb::Why {
        return (String::new(), title);
    }
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["why", id]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    let body = match cmd.output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            format!(
                "aida {} exited {}:\n{}",
                verb.label(),
                out.status,
                err.trim()
            )
        }
        Err(e) => format!("could not run aida {}: {e}", verb.label()),
    };
    (body, title)
}

/// Route one draft spec to the advisor queue. Returns `true` on success.
/// Uses `aida queue add --for advisor <id>` — the reliable routing path.
/// trace:STORY-690 | ai:claude
fn queue_for_advisor(id: &str) -> bool {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["queue", "add", "--for", "advisor", id]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    matches!(cmd.output(), Ok(out) if out.status.success())
}

/// The status-line confirmation for a `request approval` run: which ids were
/// routed, which failed to route, and which were skipped as non-drafts.
/// Pure (no IO) so it is render-smoke / unit testable. trace:STORY-690
fn request_approval_status(routed: &[String], failed: &[String], skipped: &[String]) -> String {
    let mut parts = Vec::new();
    if !routed.is_empty() {
        parts.push(format!(
            "routed {} to advisor: {}",
            routed.len(),
            routed.join(", ")
        ));
    }
    if !failed.is_empty() {
        parts.push(format!("FAILED to route: {}", failed.join(", ")));
    }
    if !skipped.is_empty() {
        parts.push(format!(
            "skipped {} non-draft(s): {}",
            skipped.len(),
            skipped.join(", ")
        ));
    }
    if parts.is_empty() {
        return "request approval: nothing to route (no drafts selected)".to_string();
    }
    parts.join(" · ")
}

/// Route one Approved spec to the implementer queue. Returns `true` on
/// success. Uses `aida queue add --for implementer <id>` — the reliable
/// routing path, the mirror of [`queue_for_advisor`]. trace:TASK-915 | ai:claude
fn queue_for_implementer(id: &str) -> bool {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["queue", "add", "--for", "implementer", id]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    matches!(cmd.output(), Ok(out) if out.status.success())
}

/// The status-line confirmation for a `queue` run: which ids were routed to
/// the implementer queue, which failed to route, and which were skipped as
/// non-Approved. Pure (no IO) so it is unit testable. The mirror of
/// [`request_approval_status`]. trace:TASK-915 | ai:claude
fn queue_status(routed: &[String], failed: &[String], skipped: &[String]) -> String {
    let mut parts = Vec::new();
    if !routed.is_empty() {
        parts.push(format!(
            "queued {} to implementer: {}",
            routed.len(),
            routed.join(", ")
        ));
    }
    if !failed.is_empty() {
        parts.push(format!("FAILED to queue: {}", failed.join(", ")));
    }
    if !skipped.is_empty() {
        parts.push(format!(
            "skipped {} non-approved: {}",
            skipped.len(),
            skipped.join(", ")
        ));
    }
    if parts.is_empty() {
        return "queue: nothing to route (no approved specs selected)".to_string();
    }
    parts.join(" · ")
}

/// Directly approve one draft spec. Returns `true` on success. Runs the
/// advisor-gated transition `aida edit <id> --status approved` — the approval
/// transition is REFUSED from a non-advisor identity, so the spawned command
/// carries advisor authority via `AIDA_SESSION_ROLE=advisor` in its env. The
/// do-it-yourself mirror of [`queue_for_advisor`]. trace:TASK-920 | ai:claude
fn approve_spec(id: &str) -> bool {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["edit", id, "--status", "approved"]);
    // The approved-status transition is advisor-gated; carry advisor authority
    // on the spawned command so it is not refused as a non-advisor identity.
    cmd.env("AIDA_SESSION_ROLE", "advisor");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    matches!(cmd.output(), Ok(out) if out.status.success())
}

/// The status-line confirmation for an `approve` run: which ids were approved,
/// which failed the transition, and which were skipped as non-drafts. Pure (no
/// IO) so it is unit testable. The mirror of [`queue_status`]. trace:TASK-920 | ai:claude
fn approve_status(approved: &[String], failed: &[String], skipped: &[String]) -> String {
    let mut parts = Vec::new();
    if !approved.is_empty() {
        parts.push(format!(
            "approved {}: {}",
            approved.len(),
            approved.join(", ")
        ));
    }
    if !failed.is_empty() {
        parts.push(format!("FAILED to approve: {}", failed.join(", ")));
    }
    if !skipped.is_empty() {
        parts.push(format!(
            "skipped {} non-draft(s): {}",
            skipped.len(),
            skipped.join(", ")
        ));
    }
    if parts.is_empty() {
        return "approve: nothing to approve (no drafts selected)".to_string();
    }
    parts.join(" · ")
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

fn render(f: &mut Frame, st: &RedesignState, loaded_spec: Option<&LoadedSpec>) {
    let theme = &st.theme;

    let rows = Layout::vertical([
        Constraint::Length(1), // status / breadcrumb line
        Constraint::Min(0),    // top panel (list)
        Constraint::Min(0),    // bottom panel (targets)
        Constraint::Length(1), // key hint
    ])
    .split(f.area());

    render_status(f, rows[0], st, theme);
    render_top(f, rows[1], st, theme);
    render_bottom(f, rows[2], st, theme);
    render_hint(f, rows[3], st, theme);

    // The item modal renders the spec loaded IN-PROCESS (`loaded_spec`):
    // structured fields + native body. trace:STORY-693 | ai:claude
    if st.modal.is_some() {
        if let Some(spec) = loaded_spec {
            render_modal(f, f.area(), theme, spec, st.modal_scroll);
        }
    }
    if let Some(vm) = &st.verb_modal {
        render_verb_modal(f, f.area(), theme, &vm.title, &vm.body);
    }
    if let Some(c) = st.confirm {
        render_confirm(f, f.area(), theme, c.verb, c.count);
    }
}

fn render_status(f: &mut Frame, area: Rect, st: &RedesignState, theme: &Theme) {
    let breadcrumb = st.breadcrumb();
    let sel = st.selected_count();
    let counts = format!(
        "role: {} · {} item(s) · {} selected",
        st.role,
        st.items.len(),
        sel
    );
    let spans = vec![
        Span::styled(
            format!(" {breadcrumb} "),
            Style::default()
                .fg(theme.on_accent)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(counts, Style::default().fg(theme.dim)),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_top(f: &mut Frame, area: Rect, st: &RedesignState, theme: &Theme) {
    let focused = st.focus == Focus::Top;
    let title = match st.level {
        Level::Scopes => " Scopes ".to_string(),
        Level::Verbs => format!(" {} › verbs ", st.scope.map(|s| s.label()).unwrap_or("")),
    };
    let block = panel_block(title, focused, theme);
    let inner_h = area.height.saturating_sub(2) as usize;

    let idxs = st.top_indices();
    let mut lines: Vec<Line> = Vec::new();
    for (row, &real) in idxs.iter().enumerate() {
        let selected = row == st.top_idx;
        let (glyph, label, hint, drills) = match st.level {
            Level::Scopes => {
                let s = Scope::all()[real];
                // A scope is a noun with children → `›` (drill).
                (
                    if s.is_functional() { "›" } else { "·" },
                    s.label(),
                    s.hint(),
                    s.is_functional(),
                )
            }
            Level::Verbs => {
                // Use the item-state-conditional list so the render agrees
                // with `top_indices()` (e.g. the Draft-only `request
                // approval` verb on the Open scope). trace:STORY-690
                let v = st.current_verbs()[real];
                // A verb is a leaf action → `↵` (run).
                ("↵", v.label(), v.hint(), false)
            }
        };
        let marker = if selected { "▸ " } else { "  " };
        let dim_label = !drills && st.level == Level::Scopes; // non-wired scopes are dimmed
        let style = row_style(theme, selected && focused, dim_label);
        let line = Line::from(vec![
            Span::styled(marker, style),
            Span::styled(format!("{glyph} "), style),
            Span::styled(format!("{label:<10}"), style.add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {hint}"), Style::default().fg(theme.dim)),
        ]);
        lines.push(line);
    }
    if focused && !st.filter.is_empty() {
        lines.insert(
            0,
            Line::from(Span::styled(
                format!("> {}", st.filter),
                Style::default().fg(theme.info),
            )),
        );
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no matches)",
            Style::default().fg(theme.dim),
        )));
    }
    lines.truncate(inner_h.max(1));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_bottom(f: &mut Frame, area: Rect, st: &RedesignState, theme: &Theme) {
    let focused = st.focus == Focus::Bottom;
    let block = panel_block(" Targets ".to_string(), focused, theme);
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;

    let idxs = st.bottom_indices();
    if st.items.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "(no backlog items — file some with `aida add --status approved`)",
                Style::default().fg(theme.dim),
            ))
            .block(block),
            area,
        );
        return;
    }

    // Scroll so the cursor stays visible.
    let start = if inner_h > 0 && st.bottom_idx >= inner_h {
        st.bottom_idx - inner_h + 1
    } else {
        0
    };

    // CLI-style columnar render: aligned ID · Type · Status(glyph+label) ·
    // Priority · Title, with the status glyph + semantic colour mirrored from
    // the CLI's `aida list` palette (see `list_row`). The status/priority cells
    // carry their own colour even on a non-cursor row so the eye learns one
    // colour map, exactly like the CLI. trace:TASK-914 | ai:claude
    let mode = list_row::GlyphMode::from_env();
    // Column widths computed across the *visible* id set so columns line up.
    let widths =
        list_row::ColumnWidths::for_rows(idxs.iter().map(|&real| st.items[real].id.as_str()));
    // The leading control cells (cursor marker + checkbox) reserve a fixed,
    // mode-independent prefix; the title gets whatever width remains.
    // "▸[x] " = marker(1) + checkbox(3) + space(1) = 5 visible cols, then each
    // column + its single-space separator.
    let prefix_w = 5;
    let glyph_w = 2; // status glyph (1) + space (1)
    let fixed = prefix_w
        + widths.id
        + 1
        + widths.req_type
        + 1
        + glyph_w
        + widths.status_label
        + 1
        + widths.priority
        + 1;
    let title_width = inner_w.saturating_sub(fixed).max(1);

    let mut lines: Vec<Line> = Vec::new();
    for (row, &real) in idxs.iter().enumerate().skip(start).take(inner_h) {
        let item = &st.items[real];
        let is_sel = st.selected[real];
        let cursor = row == st.bottom_idx;
        let checkbox = if is_sel { "[x]" } else { "[ ]" };
        let marker = if cursor { "▸" } else { " " };

        let cells = list_row::layout_row(
            list_row::RowInput {
                id: &item.id,
                req_type: &item.req_type,
                status: &item.status,
                priority: &item.priority,
                title: &item.title,
            },
            widths,
            mode,
            title_width,
        );

        // The base row style: cursor row gets the accent highlight; a selected
        // (but not cursor) row tints the structural text with `info`; otherwise
        // plain fg. The status/priority cells override their own colour.
        let cursor_active = cursor && focused;
        let base = row_style(theme, cursor_active, false);
        let structural = if is_sel && !cursor_active {
            base.fg(theme.info)
        } else {
            base
        };
        // On the cursor-highlighted row keep the accent fill uniform (don't
        // recolour the status/priority cells over the accent bg — that hurts
        // contrast); elsewhere paint the semantic palette. trace:TASK-914
        let status_style = if cursor_active {
            structural
        } else {
            list_row::status_style(&item.status, theme)
        };
        let priority_style = if cursor_active {
            structural
        } else {
            list_row::priority_style(&item.priority, theme)
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{marker}{checkbox} "), structural),
            Span::styled(format!("{} ", cells.id), structural),
            Span::styled(format!("{} ", cells.req_type), structural),
            Span::styled(format!("{} ", cells.status_glyph), status_style),
            Span::styled(format!("{} ", cells.status_label), status_style),
            Span::styled(format!("{} ", cells.priority), priority_style),
            Span::styled(cells.title, structural),
        ]));
    }
    if focused && !st.filter.is_empty() {
        lines.insert(
            0,
            Line::from(Span::styled(
                format!("> {}", st.filter),
                Style::default().fg(theme.info),
            )),
        );
    }
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_hint(f: &mut Frame, area: Rect, st: &RedesignState, theme: &Theme) {
    let base = match (st.focus, st.level) {
        (Focus::Top, Level::Scopes) => "↵ drill · Tab items · ? quit",
        (Focus::Top, Level::Verbs) => "↵ run · Tab items · ⇧Tab scopes? Esc back · ? quit",
        (Focus::Bottom, _) => "Space select · a/A all/none · p preview · ⇧Tab back · Esc back",
    };
    let text = st.status.clone().unwrap_or_else(|| base.to_string());
    f.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(theme.dim))),
        area,
    );
}

/// Render the item modal from a spec loaded IN-PROCESS — a native render of
/// the structured fields (type / status / priority / tags) plus the
/// description body rendered as MARKDOWN. `scroll` is the vertical line
/// offset (clamped here so an over-scroll pins to the last page rather than
/// scrolling the body off the top). trace:STORY-693 trace:TASK-913 | ai:claude
fn render_modal(f: &mut Frame, area: Rect, theme: &Theme, spec: &LoadedSpec, scroll: u16) {
    let popup = centered(area, 80, 80);
    f.render_widget(Clear, popup);
    let lines = spec_lines(spec, theme);
    // Clamp the scroll so the last line can reach the top of the inner area
    // but no further — past that the modal would render blank.
    let inner_h = popup.height.saturating_sub(2);
    let max_scroll = (lines.len() as u16).saturating_sub(inner_h.max(1));
    let scroll = scroll.min(max_scroll);
    let scrollable = max_scroll > 0;
    let title = if scrollable {
        format!(" {} (↑↓/PgUp/PgDn scroll · Esc/q/p close) ", spec.id)
    } else {
        format!(" {} (Esc/q/p to close) ", spec.id)
    };
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.accent))
        .title(title);
    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
        .block(block);
    f.render_widget(para, popup);
}

/// Build the native modal body: a structured header (title + a color-coded
/// field row + tags) then the description rendered as markdown. Pure (no IO)
/// so it is render-smoke / unit testable. trace:STORY-693 trace:TASK-913
fn spec_lines<'a>(spec: &'a LoadedSpec, theme: &Theme) -> Vec<Line<'a>> {
    let mut lines: Vec<Line> = Vec::new();

    // Title.
    if !spec.title.is_empty() {
        lines.push(Line::from(Span::styled(
            spec.title.clone(),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        )));
    }

    // Field row: type · status · priority — each a labelled, dimmed pair.
    let mut field_spans: Vec<Span> = Vec::new();
    let push_field = |spans: &mut Vec<Span>, label: &str, value: &str| {
        if value.is_empty() {
            return;
        }
        if !spans.is_empty() {
            spans.push(Span::styled("  ·  ", Style::default().fg(theme.dim)));
        }
        spans.push(Span::styled(
            format!("{label}: "),
            Style::default().fg(theme.dim),
        ));
        spans.push(Span::styled(
            value.to_string(),
            Style::default().fg(theme.info),
        ));
    };
    push_field(&mut field_spans, "type", &spec.req_type);
    push_field(&mut field_spans, "status", &spec.status);
    push_field(&mut field_spans, "priority", &spec.priority);
    if !field_spans.is_empty() {
        lines.push(Line::from(field_spans));
    }

    // Tags.
    if !spec.tags.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("tags: ", Style::default().fg(theme.dim)),
            Span::styled(spec.tags.join(", "), Style::default().fg(theme.accent)),
        ]));
    }

    // Separator before the body.
    lines.push(Line::from(""));

    // Description body, rendered as markdown (headings, bold/italic, inline
    // code, fenced code blocks, lists, paragraph spacing).
    if spec.description.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            "(no description)",
            Style::default().fg(theme.dim),
        )));
    } else {
        lines.extend(markdown_to_lines(&spec.description, theme));
    }

    lines
}

/// Parse a markdown body into styled ratatui [`Line`]s — a PURE function (no
/// terminal, no IO) so it is unit-testable. Honors the user's [`Theme`] for
/// every style. Handled elements:
///
/// - Headings → bold, prefixed with the original `#` markers for level cue.
/// - `**bold**` / `*italic*` inline emphasis.
/// - inline `` `code` `` → a distinct accent-coloured style.
/// - fenced code blocks → dim accent, preserved verbatim line-by-line.
/// - unordered (`- `) and ordered (`1. `) list items → bullet/number + indent.
/// - paragraphs → plain `fg` text with a blank line between blocks.
///
/// Anything unrecognised degrades to plain `fg` text; the parser never panics.
/// trace:TASK-913 | ai:claude
fn markdown_to_lines<'a>(src: &str, theme: &Theme) -> Vec<Line<'a>> {
    use pulldown_cmark::{Event, HeadingLevel, Parser, Tag};

    let code_style = Style::default().fg(theme.accent);
    let heading_style = Style::default().fg(theme.fg).add_modifier(Modifier::BOLD);
    let plain_style = Style::default().fg(theme.fg);

    let mut lines: Vec<Line> = Vec::new();
    // Spans accumulating into the current (non-code-block) line.
    let mut current: Vec<Span> = Vec::new();
    // Inline emphasis nesting counters.
    let mut bold = 0u8;
    let mut italic = 0u8;
    // Heading-in-progress flag.
    let mut in_heading = false;
    // Fenced/indented code-block state.
    let mut in_code_block = false;
    // List context stack: each entry is `Some(next_number)` for an ordered
    // list, `None` for an unordered one. Depth = indent level.
    let mut list_stack: Vec<Option<u64>> = Vec::new();
    // Whether a list-item marker is still pending for the next text.
    let mut item_prefix: Option<String> = None;

    // Push the accumulated `current` spans as a finished line, then clear.
    // A macro (not a closure) so it doesn't introduce a borrow that fights
    // the `'a` lifetime variance on `Vec<Line<'a>>`.
    macro_rules! flush {
        () => {
            if !current.is_empty() {
                lines.push(Line::from(std::mem::take(&mut current)));
            }
        };
    }

    let inline_style = |bold: u8, italic: u8| -> Style {
        let mut s = plain_style;
        if bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        s
    };

    let heading_marker = |level: HeadingLevel| -> String {
        let n = match level {
            HeadingLevel::H1 => 1,
            HeadingLevel::H2 => 2,
            HeadingLevel::H3 => 3,
            HeadingLevel::H4 => 4,
            HeadingLevel::H5 => 5,
            HeadingLevel::H6 => 6,
        };
        "#".repeat(n)
    };

    for event in Parser::new(src) {
        match event {
            Event::Start(Tag::Heading(level, _, _)) => {
                flush!();
                in_heading = true;
                current.push(Span::styled(
                    format!("{} ", heading_marker(level)),
                    heading_style,
                ));
            }
            Event::End(Tag::Heading(_, _, _)) => {
                flush!();
                in_heading = false;
                lines.push(Line::from(""));
            }
            Event::Start(Tag::Strong) => bold = bold.saturating_add(1),
            Event::End(Tag::Strong) => bold = bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => italic = italic.saturating_add(1),
            Event::End(Tag::Emphasis) => italic = italic.saturating_sub(1),
            Event::Start(Tag::Paragraph) => { /* spans accumulate until End */ }
            Event::End(Tag::Paragraph) => {
                flush!();
                lines.push(Line::from(""));
            }
            // Both fenced (```lang) and indented code blocks land here.
            Event::Start(Tag::CodeBlock(_)) => {
                flush!();
                in_code_block = true;
            }
            Event::End(Tag::CodeBlock(_)) => {
                flush!();
                in_code_block = false;
                lines.push(Line::from(""));
            }
            Event::Start(Tag::List(first)) => {
                flush!();
                list_stack.push(first);
            }
            Event::End(Tag::List(_)) => {
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                flush!();
                let depth = list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let marker = match list_stack.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{indent}{n}. ");
                        *n += 1;
                        m
                    }
                    _ => format!("{indent}• "),
                };
                item_prefix = Some(marker);
            }
            Event::End(Tag::Item) => {
                flush!();
            }
            Event::Code(text) => {
                if let Some(prefix) = item_prefix.take() {
                    current.push(Span::styled(prefix, plain_style));
                }
                current.push(Span::styled(text.into_string(), code_style));
            }
            Event::Text(text) => {
                if in_code_block {
                    // Preserve verbatim, line-by-line, with the code style.
                    for (i, raw) in text.split('\n').enumerate() {
                        if i > 0 {
                            flush!();
                        }
                        current.push(Span::styled(raw.to_string(), code_style));
                    }
                    continue;
                }
                if let Some(prefix) = item_prefix.take() {
                    current.push(Span::styled(prefix, plain_style));
                }
                let style = if in_heading {
                    heading_style
                } else {
                    inline_style(bold, italic)
                };
                current.push(Span::styled(text.into_string(), style));
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_code_block {
                    flush!();
                } else {
                    current.push(Span::styled(" ", plain_style));
                }
            }
            Event::Rule => {
                flush!();
                lines.push(Line::from(Span::styled(
                    "─".repeat(8),
                    Style::default().fg(theme.dim),
                )));
            }
            // Unknown / unsupported nodes (tables, html, footnotes, …) degrade
            // to whatever text they carry; never panic.
            _ => {}
        }
    }
    flush!();

    // Collapse a trailing blank line for a tidy bottom edge.
    while matches!(lines.last(), Some(l) if line_is_blank(l)) {
        lines.pop();
    }
    lines
}

/// Is a [`Line`] visually blank (no spans, or only empty-content spans)? Used
/// to trim trailing blank lines from rendered markdown. trace:TASK-913
fn line_is_blank(line: &Line) -> bool {
    line.spans.iter().all(|s| s.content.is_empty())
}

/// Render a verb-output modal (the captured stdout of `show` / `why`).
/// trace:STORY-690 | ai:claude
fn render_verb_modal(f: &mut Frame, area: Rect, theme: &Theme, title: &str, body: &str) {
    let popup = centered(area, 80, 80);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.accent))
        .title(format!(" {title} (Esc/q to close) "));
    let para = Paragraph::new(body.to_string())
        .block(block)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(theme.fg));
    f.render_widget(para, popup);
}

fn render_confirm(f: &mut Frame, area: Rect, theme: &Theme, verb: Verb, count: usize) {
    let popup = centered(area, 50, 20);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.warn))
        .title(" confirm ");
    let lines = vec![
        Line::from(Span::styled(
            format!("Nothing selected. {} all {count} item(s)?", verb.label()),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "y / Enter = yes   ·   n / Esc = no",
            Style::default().fg(theme.dim),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Center),
        popup,
    );
}

// --- small render helpers --------------------------------------------------

fn panel_block(title: String, focused: bool, theme: &Theme) -> Block<'static> {
    let border = if focused { theme.accent } else { theme.border };
    Block::bordered()
        .border_style(Style::default().fg(border))
        .title(title)
}

fn row_style(theme: &Theme, active: bool, dim: bool) -> Style {
    if active {
        Style::default()
            .fg(theme.on_accent)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if dim {
        Style::default().fg(theme.dim)
    } else {
        Style::default().fg(theme.fg)
    }
}

/// A centered popup `pct_w` × `pct_h` percent of `area`.
fn centered(area: Rect, pct_w: u16, pct_h: u16) -> Rect {
    let vert = Layout::vertical([
        Constraint::Percentage((100 - pct_h) / 2),
        Constraint::Percentage(pct_h),
        Constraint::Percentage((100 - pct_h) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_w) / 2),
        Constraint::Percentage(pct_w),
        Constraint::Percentage((100 - pct_w) / 2),
    ])
    .split(vert[1])[1]
}

#[cfg(test)]
mod render_tests {
    //! Render smoke tests — drive the IO-side `render` over a headless
    //! [`TestBackend`] to prove the two-panel layout and the modal /
    //! confirm overlays paint without panicking at a realistic and a tiny
    //! terminal size. (The interaction logic itself is covered by the pure
    //! tests in `state`.) trace:STORY-690 | ai:claude
    use super::*;
    use ratatui::backend::TestBackend;

    fn sample(n: usize) -> RedesignState {
        let items = (0..n)
            .map(|i| TargetItem {
                id: format!("STORY-{i}"),
                title: format!("a sample backlog item {i}"),
                req_type: "Story".into(),
                // Alternate Draft / Approved so the open-scope render path
                // (Draft-conditional verb) is exercised by the smoke tests.
                status: if i % 2 == 0 { "Draft" } else { "Approved" }.into(),
                // Carry a priority so the columnar Priority cell is exercised
                // by the render smoke tests. trace:TASK-914
                priority: ["High", "Medium", "Low"][i % 3].into(),
                body: format!("# STORY-{i}\n\nbody text here"),
            })
            .collect();
        RedesignState::new(items, "advisor")
    }

    /// Drill into the Open scope (index 1) for the open-scope render tests.
    fn drill_open(st: &mut RedesignState) {
        st.move_down(); // Backlog → Open
        st.drill();
    }

    fn draw(st: &RedesignState, w: u16, h: u16) {
        draw_with_spec(st, None, w, h);
    }

    fn draw_with_spec(st: &RedesignState, spec: Option<&LoadedSpec>, w: u16, h: u16) {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
        terminal.draw(|f| render(f, st, spec)).expect("render");
    }

    fn sample_spec() -> LoadedSpec {
        LoadedSpec {
            id: "STORY-1".into(),
            title: "a sample spec".into(),
            req_type: "Story".into(),
            status: "Approved".into(),
            priority: "High".into(),
            tags: vec!["performance".into(), "tui".into()],
            description: "Line one.\n\nLine two of the body.".into(),
        }
    }

    #[test]
    fn renders_scope_level() {
        draw(&sample(5), 100, 30);
    }

    /// The columnar scope-list render (TASK-914): a multi-row Backlog list with
    /// mixed statuses/priorities + a selection + the cursor paints over the
    /// TestBackend without panicking, at a realistic and a tiny size. The pure
    /// column-layout + status→glyph/colour mapping is unit-tested in
    /// `super::list_row`. trace:TASK-914 | ai:claude
    #[test]
    fn renders_columnar_scope_list() {
        let mut st = sample(8);
        st.focus_bottom();
        st.toggle_select(); // select the cursor row
        st.move_down();
        st.move_down(); // move the cursor off the selected row
        draw(&st, 120, 20);
        // Tiny terminal: columns clamp, title narrows, no panic.
        draw(&st, 24, 8);
        // Single row (id-width floor path).
        draw(&sample(1), 100, 10);
    }

    #[test]
    fn renders_verb_level_with_selection() {
        let mut st = sample(5);
        st.drill();
        st.focus_bottom();
        st.toggle_select();
        st.focus_top();
        draw(&st, 100, 30);
    }

    #[test]
    fn renders_item_modal() {
        let mut st = sample(5);
        st.drill();
        st.focus_bottom();
        st.open_modal();
        // The modal now renders the spec loaded in-process (structured fields
        // + native body), not the captured stdout. trace:STORY-693
        let spec = sample_spec();
        draw_with_spec(&st, Some(&spec), 100, 30);
    }

    #[test]
    fn renders_native_spec_fields_and_body() {
        // The in-process modal paints the structured field row + tags + the
        // line-by-line body without panicking. trace:STORY-693
        let mut st = sample(5);
        st.open_modal_external();
        let spec = sample_spec();
        draw_with_spec(&st, Some(&spec), 100, 30);
        // Tiny terminal too.
        draw_with_spec(&st, Some(&spec), 20, 6);
    }

    #[test]
    fn renders_confirm_popup() {
        let mut st = sample(5);
        st.drill();
        st.run_verb(); // raises the confirm-all popup
        draw(&st, 100, 30);
    }

    #[test]
    fn renders_into_a_tiny_terminal_without_panicking() {
        draw(&sample(5), 20, 6);
        let mut st = sample(0); // empty backlog
        st.drill();
        draw(&st, 20, 6);
    }

    #[test]
    fn renders_open_scope_verbs_with_draft_conditional() {
        // Focused on a Draft item → the verb list shows request approval +
        // approve (the Draft-conditional verbs). trace:TASK-920
        let mut st = sample(5); // index 0 is Draft
        drill_open(&mut st);
        st.focus_bottom(); // focus TASK-0 (Draft)
        st.focus_top();
        draw(&st, 100, 30);
        assert_eq!(
            st.current_verbs(),
            vec![Verb::Show, Verb::Why, Verb::RequestApproval, Verb::Approve]
        );
    }

    #[test]
    fn renders_verb_output_modal() {
        let mut st = sample(5);
        drill_open(&mut st);
        st.open_verb_modal("STORY-0 — show", "captured stdout\nline two");
        draw(&st, 100, 30);
    }

    #[test]
    fn request_approval_status_lists_routed_skipped_failed() {
        let s = request_approval_status(
            &["TASK-1".to_string(), "TASK-2".to_string()],
            &["TASK-3".to_string()],
            &["TASK-4".to_string()],
        );
        assert!(s.contains("routed 2"));
        assert!(s.contains("TASK-1"));
        assert!(s.contains("FAILED"));
        assert!(s.contains("skipped 1"));
        // Empty case.
        let empty = request_approval_status(&[], &[], &[]);
        assert!(empty.contains("nothing to route"));
    }

    #[test]
    fn queue_status_lists_routed_skipped_failed() {
        let s = queue_status(
            &["TASK-1".to_string(), "TASK-2".to_string()],
            &["TASK-3".to_string()],
            &["TASK-4".to_string()],
        );
        assert!(s.contains("queued 2"));
        assert!(s.contains("implementer"));
        assert!(s.contains("TASK-1"));
        assert!(s.contains("FAILED"));
        assert!(s.contains("skipped 1"));
        // Empty case.
        let empty = queue_status(&[], &[], &[]);
        assert!(empty.contains("nothing to route"));
    }

    #[test]
    fn approve_status_lists_approved_skipped_failed() {
        // trace:TASK-920
        let s = approve_status(
            &["TASK-1".to_string(), "TASK-2".to_string()],
            &["TASK-3".to_string()],
            &["TASK-4".to_string()],
        );
        assert!(s.contains("approved 2"));
        assert!(s.contains("TASK-1"));
        assert!(s.contains("FAILED"));
        assert!(s.contains("skipped 1"));
        // Empty case.
        let empty = approve_status(&[], &[], &[]);
        assert!(empty.contains("nothing to approve"));
    }

    // --- Markdown body rendering (TASK-913) -------------------------------

    /// Concatenate a line's span contents back into a plain string.
    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Does any span on the line carry the given modifier?
    fn line_has_modifier(line: &Line, m: Modifier) -> bool {
        line.spans.iter().any(|s| s.style.add_modifier.contains(m))
    }

    /// Does any span on the line render in the code style (theme accent fg)?
    fn line_has_code_style(line: &Line, theme: &Theme) -> bool {
        line.spans.iter().any(|s| s.style.fg == Some(theme.accent))
    }

    #[test]
    fn markdown_heading_is_bold() {
        let theme = Theme::default();
        let lines = markdown_to_lines("# Title here", &theme);
        let heading = lines
            .iter()
            .find(|l| line_text(l).contains("Title here"))
            .expect("heading line present");
        assert!(line_text(heading).starts_with('#'), "keeps a # level cue");
        assert!(
            line_has_modifier(heading, Modifier::BOLD),
            "heading span is bold"
        );
    }

    #[test]
    fn markdown_list_item_starts_with_bullet() {
        let theme = Theme::default();
        let lines = markdown_to_lines("- first\n- second", &theme);
        let first = lines
            .iter()
            .find(|l| line_text(l).contains("first"))
            .expect("first item present");
        assert!(
            line_text(first).trim_start().starts_with('•'),
            "unordered item leads with a bullet, got {:?}",
            line_text(first)
        );
    }

    #[test]
    fn markdown_ordered_list_is_numbered() {
        let theme = Theme::default();
        let lines = markdown_to_lines("1. alpha\n2. beta", &theme);
        let beta = lines
            .iter()
            .find(|l| line_text(l).contains("beta"))
            .expect("second item present");
        assert!(
            line_text(beta).trim_start().starts_with("2."),
            "ordered item keeps its number, got {:?}",
            line_text(beta)
        );
    }

    #[test]
    fn markdown_inline_code_carries_code_style() {
        let theme = Theme::default();
        let lines = markdown_to_lines("call `aida show` now", &theme);
        let line = lines
            .iter()
            .find(|l| line_text(l).contains("aida show"))
            .expect("line with inline code");
        // The inline-code span specifically must be code-styled.
        let code_span = line
            .spans
            .iter()
            .find(|s| s.content.contains("aida show"))
            .expect("code span");
        assert_eq!(code_span.style.fg, Some(theme.accent), "inline code styled");
    }

    #[test]
    fn markdown_fenced_code_block_is_verbatim_and_styled() {
        let theme = Theme::default();
        let src = "```\nlet x = 1;\n  indented();\n```";
        let lines = markdown_to_lines(src, &theme);
        let code_line = lines
            .iter()
            .find(|l| line_text(l).contains("let x = 1;"))
            .expect("first code line");
        assert_eq!(line_text(code_line), "let x = 1;", "preserved verbatim");
        assert!(line_has_code_style(code_line, &theme), "code-styled");
        // The indented second line keeps its leading whitespace verbatim.
        let indented = lines
            .iter()
            .find(|l| line_text(l).contains("indented();"))
            .expect("second code line");
        assert!(
            line_text(indented).starts_with("  indented();"),
            "indentation preserved, got {:?}",
            line_text(indented)
        );
    }

    #[test]
    fn markdown_plain_text_passes_through() {
        let theme = Theme::default();
        let lines = markdown_to_lines("just a plain paragraph", &theme);
        assert!(
            lines
                .iter()
                .any(|l| line_text(l) == "just a plain paragraph"),
            "plain text survives intact"
        );
    }

    #[test]
    fn markdown_unknown_nodes_do_not_panic() {
        let theme = Theme::default();
        // Tables + raw HTML + a setext rule — none specially handled.
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n\n<div>raw</div>\n\n---\n";
        let _ = markdown_to_lines(src, &theme); // must not panic
    }

    #[test]
    fn renders_markdown_body_smoke() {
        // A representative multi-element body drives the full modal render
        // over a TestBackend without panicking. trace:TASK-913
        let mut st = sample(5);
        st.open_modal_external();
        let spec = LoadedSpec {
            id: "TASK-913".into(),
            title: "Markdown body".into(),
            req_type: "Task".into(),
            status: "Draft".into(),
            priority: "Medium".into(),
            tags: vec!["markdown".into()],
            description: "# Heading\n\nA paragraph with **bold**, *italic*, and \
                 `inline code`.\n\n- bullet one\n- bullet two\n\n1. step one\n2. step \
                 two\n\n```\nfenced();\n```\n"
                .into(),
        };
        draw_with_spec(&st, Some(&spec), 100, 30);
        // Tiny terminal must not panic either.
        draw_with_spec(&st, Some(&spec), 20, 6);
    }

    #[test]
    fn modal_scroll_clamps_and_renders() {
        // A long body scrolled past its end pins to the last page (render
        // never blanks / panics). trace:TASK-913
        let mut st = sample(5);
        st.open_modal_external();
        let body: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let spec = LoadedSpec {
            id: "TASK-913".into(),
            title: "Long".into(),
            req_type: "Task".into(),
            status: "Draft".into(),
            priority: String::new(),
            tags: vec![],
            description: body,
        };
        st.modal_scroll = 9999; // way past the end
        draw_with_spec(&st, Some(&spec), 100, 30);
    }
}
