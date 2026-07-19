//! The `aida tui` **action→target redesign** cockpit (EPIC-54).
//!
//! Implements the gesture grammar from
//! `docs/plans/2026-06-25-tui-action-target-redesign.md` across the wired
//! scopes (Backlog / Open / Test / Queue) and their verbs. It is the default
//! `aida tui` surface; the legacy TUI is reachable via `AIDA_TUI_REDESIGN=0`
//! (see [`enabled`]).
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
mod liveness;
mod mail;
mod state;
mod store;

use state::PendingOp;
// trace:TASK-937 | ai:claude
use state::{batch_approve, BatchApproveOutcome};
pub use state::{RedesignState, RunOutcome, Scope, TargetItem, Verb};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;
use store::{LoadedComment, LoadedRelation, LoadedSpec, SpecGraph, SpecStore};

use crate::term;
use crate::theme::Theme;
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::{Frame, Terminal};
use state::{Focus, GateHold, Level};
use std::io::Stdout;
use std::process::Command;

/// Lines scrolled per PageUp / PageDown (and Space) in the item modal.
// trace:TASK-913 | ai:claude
const MODAL_PAGE: u16 = 10;

/// The trailing marker on a Test-scope row whose spec carries a `## Test Plan`
/// section. A small suffix glyph so the operator sees which shipped specs have
// verification steps. trace:STORY-699 | ai:claude
const TEST_PLAN_MARKER: &str = "🧪";

/// Is the action→target redesign selected? Checked by `aida_tui::run` and the
/// CLI launcher gate (via the `aida_tui::redesign_enabled` re-export). EPIC-54
/// is now the DEFAULT (TASK-1051): the redesign renders unless
/// `AIDA_TUI_REDESIGN` is an explicit opt-OUT (`0`/`false`/`no`/`off`), which
/// selects the legacy TUI. Unset, empty, or any other value keeps the default
/// (redesign on).
// trace:TASK-1051 | ai:claude
pub fn enabled() -> bool {
    !matches!(
        std::env::var("AIDA_TUI_REDESIGN")
            .ok()
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("no") | Some("off")
    )
}

/// The cockpit's opening status line — a confident, finished one-liner naming
/// the wired scopes (or the store-unavailable fallback). Deliberately carries
/// NO "prototype" / "Slice N" self-label: this is the default `aida tui`
/// surface that every user sees.
// trace:STORY-724 | ai:claude
fn startup_status(store_available: bool) -> String {
    if store_available {
        "Scopes: Backlog · Open · Test · Queue · Mail. ? help · q quits.".to_string()
    } else {
        "Store unavailable — no in-process data. ? help · q quits.".to_string()
    }
}

/// Launch the redesign cockpit. Owns the terminal via the same RAII
/// guard the rest of the TUI uses, so a panic or a normal exit never
/// strands the terminal in raw mode.
///
/// `project_root` is the directory holding `.aida/config.toml` (resolved by
/// the launcher). It opens the cache-backed git backend ONCE
/// ([`SpecStore`]) so every scope-list + show-modal read is in-process —
// no per-read `aida` subprocess cold-start. trace:STORY-693 | ai:claude
pub fn run(theme: Theme, project_root: &std::path::Path) -> Result<()> {
    term::install_panic_hook();
    term::install_signal_handler()?;

    // Open the in-process read backend once. If the store can't be attached
    // (offline, missing, etc.) the cockpit still runs — the scope lists are
    // empty and the modal reports the failure — rather than crashing.
    // trace:STORY-693 | ai:claude
    let store = SpecStore::open(project_root);

    // The optional EPIC focus lens (STORY-695 / STORY-697): the TUI can launch
    // scoped to an epic + its transitive children. The launch epic is resolved
    // with precedence `AIDA_TUI_EPIC` env > `.aida/tui-focus` marker > branch
    // inference (STORY-697). We compute the descendant closure ONCE here
    // (in-process) and thread it through every scope-list fetch; an
    // empty/unresolvable closure leaves the TUI unfocused.
    // trace:STORY-695 trace:STORY-697 | ai:claude
    let launch_epic = launch_focus_epic(project_root, store.as_ref());
    let mut focus_set = launch_epic.as_ref().and_then(|epic| {
        store.as_ref().and_then(|s| {
            let set = s.descendants_of(epic);
            if set.is_empty() {
                None
            } else {
                Some(set)
            }
        })
    });

    let items = store
        .as_ref()
        .map(|s| s.scope_items(Scope::Backlog, focus_set.as_ref()))
        .unwrap_or_default();
    let mut st = RedesignState::new(items, resolve_role());
    st.theme = theme;
    // Seed the focus context (epic id + progress summary) when launched focused.
    // trace:STORY-695 trace:STORY-697 | ai:claude
    if let (Some(set), Some(s)) = (focus_set.as_ref(), store.as_ref()) {
        st.focus_epic = launch_epic.clone();
        refresh_focus_summary(&mut st, s, set);
    }
    st.status = Some(startup_status(store.is_some()));

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
        &mut focus_set,
        project_root,
    )?;
    Ok(())
}

/// Resolve the launch-time EPIC to focus on, with precedence
/// **`AIDA_TUI_EPIC` env > `.aida/tui-focus` marker > branch inference**
/// (STORY-697). The pure precedence over env-vs-marker lives in
/// [`store::resolve_focus_epic`]; when both are absent we *infer* the epic from
/// the current branch's trailered specs' most-common parent epic (the stretch).
/// Returns the epic id (still to be closure-resolved by the caller), or `None`
// when nothing resolves. trace:STORY-697 | ai:claude
fn launch_focus_epic(project_root: &std::path::Path, store: Option<&SpecStore>) -> Option<String> {
    let env = std::env::var("AIDA_TUI_EPIC").ok();
    let marker = store::read_focus_marker(project_root);
    if let Some(epic) = store::resolve_focus_epic(env.as_deref(), marker.as_deref()) {
        return Some(epic);
    }
    // Stretch (STORY-697): neither env nor marker set — infer from the branch.
    infer_focus_from_branch(project_root, store)
}

/// Infer the launch focus epic from the current branch's commit trailers
/// (STORY-697 stretch): read the branch's `(SPEC-ID)` trailers, map each
/// trailered spec to its parent epic, and take the mode. Returns `None` when
/// the store is unavailable, git can't be read, or no trailered spec has an
// epic parent. trace:STORY-697 | ai:claude
fn infer_focus_from_branch(
    project_root: &std::path::Path,
    store: Option<&SpecStore>,
) -> Option<String> {
    let store = store?;
    let log = branch_commit_subjects(project_root)?;
    let trailered = store::parse_spec_trailers(&log);
    if trailered.is_empty() {
        return None;
    }
    let epics = store.parent_epics_of(&trailered);
    store::most_common(&epics)
}

/// The commit subjects unique to the current branch (`origin/main..HEAD`),
/// newline-joined — the input to the branch-inference trailer scan. Falls back
/// to the last 50 subjects when `origin/main` is unknown (a fresh clone with no
/// upstream). One bounded `git` shell-out, fired ONCE at launch only when env +
// marker are both unset. trace:STORY-697 | ai:claude
fn branch_commit_subjects(project_root: &std::path::Path) -> Option<String> {
    let run = |range: &str| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["log", "--pretty=%s", range])
            .output()
            .ok()?;
        if out.status.success() {
            String::from_utf8(out.stdout).ok()
        } else {
            None
        }
    };
    run("origin/main..HEAD")
        .filter(|s| !s.trim().is_empty())
        .or_else(|| run("-n50"))
}

/// Recompute and store the focus-line progress summary for the active focus set
/// (e.g. "EPIC-54: 6 done · 2 draft"). Stored on `st.focus_summary` for the
// status-line render. trace:STORY-695 | ai:claude
fn refresh_focus_summary(
    st: &mut RedesignState,
    store: &SpecStore,
    set: &std::collections::HashSet<String>,
) {
    let (progress, _total) = store.focus_progress(set);
    st.focus_summary = Some(progress.summary());
}

/// Apply a NEW focus epic at runtime (the change-focus key): recompute the
/// closure, and — when it resolves — set the focus context + invalidate the
/// per-scope item cache so the next sync re-fetches narrowed lists. A blank /
/// unresolvable epic clears the focus instead. The `loaded` sentinel is reset
/// so [`sync_scope_items`] re-fetches even when the active scope is unchanged.
// trace:STORY-695 | ai:claude
#[allow(clippy::too_many_arguments)]
fn apply_focus(
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    cache: &mut HashMap<Scope, Vec<TargetItem>>,
    loaded: &mut Scope,
    focus_set: &mut Option<std::collections::HashSet<String>>,
    project_root: &std::path::Path,
    epic: &str,
) {
    let set = store.map(|s| s.descendants_of(epic)).unwrap_or_default();
    if set.is_empty() {
        st.status = Some(format!(
            "focus: {epic} resolved to no specs — staying {}",
            if st.focused() {
                "on current focus"
            } else {
                "unfocused"
            }
        ));
        return;
    }
    st.focus_epic = Some(epic.to_string());
    *focus_set = Some(set);
    if let (Some(s), Some(set)) = (store, focus_set.as_ref()) {
        refresh_focus_summary(st, s, set);
    }
    invalidate_scope_cache(cache, loaded);
    // Persist the pick so re-launching this worktree auto-focuses on it.
    // trace:STORY-697 | ai:claude
    store::write_focus_marker(project_root, epic);
    st.status = Some(format!("focus set to {epic} (saved to .aida/tui-focus)"));
}

/// Clear the runtime focus (the clear-focus key): drop the lens + summary and
/// invalidate the item cache so every scope re-fetches unfiltered.
// trace:STORY-695 | ai:claude
fn clear_focus(
    st: &mut RedesignState,
    cache: &mut HashMap<Scope, Vec<TargetItem>>,
    loaded: &mut Scope,
    focus_set: &mut Option<std::collections::HashSet<String>>,
    project_root: &std::path::Path,
) {
    // Always remove the marker so a cleared worktree relaunches unfocused, even
    // if the in-memory lens was already empty. trace:STORY-697 | ai:claude
    store::clear_focus_marker(project_root);
    if !st.focused() {
        st.status = Some("no focus set".to_string());
        return;
    }
    st.clear_focus();
    *focus_set = None;
    invalidate_scope_cache(cache, loaded);
    st.status = Some("focus cleared (.aida/tui-focus removed)".to_string());
}

/// Live-refresh (the `r` key): re-read the store in-process so state changes
/// made OUTSIDE the TUI (a status flip via the CLI or another agent) appear
/// without relaunch.
///
/// Mechanics: [`invalidate_scope_cache`] drops the in-memory per-scope item
/// cache and resets the `loaded` sentinel, so the next [`sync_scope_items`]
/// (run at the top of the event loop, before the next draw) re-fetches the
/// active scope. The re-fetch goes through [`SpecStore::scope_items`] →
/// `CachedGitBackend::list_summaries`, which **stale-checks the SQLite cache
/// against the orphan-branch HEAD on every call and rebuilds when behind**. So
/// an external write — which advances the `.aida-store` worktree HEAD when its
/// targeted `update SPEC-ID` commit lands — is surfaced WITHOUT reopening the
/// backend: the backend reads HEAD fresh from disk per read and never memoizes
/// it. (The focus-progress + descendant reads use the same fresh `list_*`
/// paths.) Reopening the backend is therefore unnecessary.
///
/// When focused on an epic, the descendant closure + progress summary are also
/// recomputed so a child added or a status flipped inside the focus epic is
// reflected. trace:TASK-934 | ai:claude
fn refresh(
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    cache: &mut HashMap<Scope, Vec<TargetItem>>,
    loaded: &mut Scope,
    focus_set: &mut Option<std::collections::HashSet<String>>,
) {
    invalidate_scope_cache(cache, loaded);
    // Recompute the focus closure + progress so externally-added children and
    // status flips inside the focus epic surface too. A now-empty closure
    // leaves the existing focus untouched (mirrors apply_focus's guard).
    // trace:TASK-934 | ai:claude
    if let (Some(epic), Some(s)) = (st.focus_epic.clone(), store) {
        let set = s.descendants_of(&epic);
        if !set.is_empty() {
            *focus_set = Some(set);
            if let Some(set) = focus_set.as_ref() {
                refresh_focus_summary(st, s, set);
            }
        }
    }
    st.status = Some(if store.is_some() {
        "refreshed".to_string()
    } else {
        "refresh: store unavailable".to_string()
    });
}

/// Drop the per-scope item cache and reset the `loaded` sentinel to a value no
/// functional scope equals, so the next [`sync_scope_items`] re-fetches the
// active scope under the new focus. trace:STORY-695 | ai:claude
fn invalidate_scope_cache(cache: &mut HashMap<Scope, Vec<TargetItem>>, loaded: &mut Scope) {
    cache.clear();
    // Sessions is non-functional, so it can never equal the active functional
    // scope — forcing the sync to re-fetch.
    *loaded = Scope::Sessions;
}

/// The scope whose item-set the bottom panel should currently show: the
/// drilled-into scope when at the verb level, else the highlighted scope at
/// the scope level. Only functional scopes have a target set; others keep
// the last loaded set. trace:STORY-690 | ai:claude
fn active_item_scope(st: &RedesignState) -> Option<Scope> {
    match st.scope {
        Some(scope) => Some(scope),
        None => st.top_scope().filter(|s| s.is_functional()),
    }
}

/// Keep the bottom panel's items in sync with the active scope, fetching
/// (and caching) on first visit. The fetch is now an in-process cache read
// via the open [`SpecStore`] — no subprocess. trace:STORY-690 trace:STORY-693
fn sync_scope_items(
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    cache: &mut HashMap<Scope, Vec<TargetItem>>,
    loaded: &mut Scope,
    focus_set: Option<&std::collections::HashSet<String>>,
) {
    let Some(scope) = active_item_scope(st) else {
        return;
    };
    if scope == *loaded {
        return;
    }
    // The EPIC focus lens (STORY-695) narrows every scope fetch to the focus
    // set; `None` is the unfocused (all-items) behavior. trace:STORY-695
    let items = cache
        .entry(scope)
        .or_insert_with(|| {
            store
                .map(|s| s.scope_items(scope, focus_set))
                .unwrap_or_default()
        })
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
// modal-open, never on cursor move. trace:STORY-693 | ai:claude
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
// can't be found — rendered the same way as a real one. trace:STORY-693
fn missing_spec(id: &str) -> LoadedSpec {
    LoadedSpec {
        id: id.to_string(),
        title: String::new(),
        req_type: String::new(),
        status: String::new(),
        priority: String::new(),
        tags: Vec::new(),
        description: format!("(could not load {id} from the store)"),
        comments: Vec::new(),
        graph: SpecGraph::default(),
    }
}

// ---------------------------------------------------------------------------
// Async verb execution (BUG-633)
// ---------------------------------------------------------------------------

/// The outcome of a background verb run, sent back over the completion channel:
/// the final status-line message plus whether the affected scope cache must be
// invalidated so the new state shows. trace:BUG-633 | ai:claude
struct VerbResult {
    /// The status-line text to show on completion (the existing `*_status`
    /// message — e.g. "approved 2: TASK-1, TASK-2").
    status: String,
    /// Whether to drop the per-scope item cache on completion (a store WRITE
    /// changed the data; reads don't).
    invalidate: bool,
}

/// An in-flight background verb: the DISPLAY state ([`PendingOp`], pure) paired
/// with the completion channel the worker thread sends its [`VerbResult`] over.
/// Held as an event-loop local (like `loaded_spec`), never in the pure state.
// trace:BUG-633 | ai:claude
struct Pending {
    op: PendingOp,
    rx: Receiver<VerbResult>,
}

/// Kick off a verb on a background thread so the key handler returns
/// immediately and the event loop keeps ticking (spinner + completion poll).
///
/// POLICY: refuses to start a second verb while one is in flight — sets a
/// "busy — <label> in progress" status and leaves the existing op untouched
/// (returns `false`). Navigation stays live because only the verb-START paths
/// call this; movement keys never do. On success spawns the worker, installs
// the [`Pending`], and returns `true`. trace:BUG-633 | ai:claude
fn start_pending(
    pending: &mut Option<Pending>,
    st: &mut RedesignState,
    label: impl Into<String>,
    work: impl FnOnce() -> VerbResult + Send + 'static,
) -> bool {
    if let Some(p) = pending.as_ref() {
        st.status = Some(format!("busy — {} in progress", p.op.label));
        return false;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // The receiver may have been dropped if the TUI exited mid-run; the
        // send error is then expected and ignored.
        let _ = tx.send(work());
    });
    *pending = Some(Pending {
        op: PendingOp::new(label),
        rx,
    });
    true
}

/// Apply a finished verb's [`VerbResult`] to the state: write the final status
/// line and report whether the scope cache must be invalidated. Pure (no IO, no
// threads) so the completion transition is unit-testable. trace:BUG-633 | ai:claude
fn apply_verb_result(st: &mut RedesignState, result: &VerbResult) -> bool {
    st.status = Some(result.status.clone());
    result.invalidate
}

#[allow(clippy::too_many_arguments)]
fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    cache: &mut HashMap<Scope, Vec<TargetItem>>,
    loaded: &mut Scope,
    focus_set: &mut Option<std::collections::HashSet<String>>,
    project_root: &std::path::Path,
) -> Result<()> {
    // The spec currently shown in the item modal (loaded in-process on open,
    // cleared on close). trace:STORY-693 | ai:claude
    let mut loaded_spec: Option<LoadedSpec> = None;
    // The in-flight background verb, if any (BUG-633): a worker thread runs the
    // slow `aida` store-write and sends its result here; the loop polls it,
    // animates a spinner, and stays responsive meanwhile. trace:BUG-633
    let mut pending: Option<Pending> = None;
    loop {
        // Keep the bottom panel's target set following the active scope
        // (highlighted at the scope level, drilled-into at the verb level).
        sync_scope_items(st, store, cache, loaded, focus_set.as_ref());
        // Refresh the per-row liveness verdict (TASK-978) on a poll cadence: the
        // liveness read runs a real `/proc` process probe (~1.3s), so
        // `refresh_if_due` guards it three ways (BUG-676) — a long TTL, a
        // single-flight background thread, and this lazy-when-visible gate: probe
        // ONLY when the current scope actually surfaces the liveness glyph (the
        // running-work scopes). On Backlog / the scope panel of a glyph-less scope
        // we never pay for the probe. The render path only reads the cached map.
        //
        // BUG-677: this now computes liveness IN-PROCESS via
        // `aida_core::liveness` (same probe + classifiers `aida ps` uses) instead
        // of shelling out to `aida ps --json`. The spec projection is built
        // lazily — only on a frame that actually fires the probe — from the
        // already-open store. trace:TASK-978 trace:BUG-676 trace:BUG-677 | ai:claude
        let liveness_visible = st.scope.map(|s| s.shows_liveness()).unwrap_or(false);
        st.liveness
            .refresh_if_due(project_root, liveness_visible, || {
                store.map(|s| s.liveness_inputs()).unwrap_or_default()
            });
        terminal.draw(|f| render(f, st, loaded_spec.as_ref(), pending.as_ref().map(|p| &p.op)))?;

        // Drain a finished background verb (BUG-633): on completion set the
        // final status + invalidate the affected scope so the new state shows;
        // otherwise advance the spinner one frame. A disconnected channel means
        // the worker panicked without sending — clear the pending state rather
        // than spin forever. trace:BUG-633 | ai:claude
        if let Some(p) = pending.as_mut() {
            match p.rx.try_recv() {
                Ok(result) => {
                    if apply_verb_result(st, &result) {
                        invalidate_scope_cache(cache, loaded);
                    }
                    pending = None;
                }
                Err(TryRecvError::Empty) => p.op.tick(),
                Err(TryRecvError::Disconnected) => {
                    st.status = Some("operation ended unexpectedly".to_string());
                    pending = None;
                }
            }
        }

        // POLL the input with a timeout so the loop TICKS even with no key: a
        // short timeout while a verb is pending (smooth spinner + prompt
        // completion), a long idle timeout otherwise (negligible wakeups; any
        // keystroke wakes `poll` immediately). trace:BUG-633 | ai:claude
        let timeout = if pending.is_some() {
            Duration::from_millis(80)
        } else {
            Duration::from_millis(1000)
        };
        if !crossterm::event::poll(timeout)? {
            continue;
        }
        let Event::Key(key) = crossterm::event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        if handle_key(
            terminal,
            st,
            store,
            &mut loaded_spec,
            cache,
            loaded,
            focus_set,
            project_root,
            &mut pending,
            key,
        )? {
            break;
        }
    }
    Ok(())
}

/// Route one keystroke. Returns `Ok(true)` when the app should exit.
#[allow(clippy::too_many_arguments)]
fn handle_key(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    loaded_spec: &mut Option<LoadedSpec>,
    cache: &mut HashMap<Scope, Vec<TargetItem>>,
    loaded: &mut Scope,
    focus_set: &mut Option<std::collections::HashSet<String>>,
    project_root: &std::path::Path,
    pending: &mut Option<Pending>,
    key: KeyEvent,
) -> Result<bool> {
    // Ctrl-C always quits.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(true);
    }

    // The context-sensitive '?' help popup captures all keys while open: Esc or
    // '?' closes it; every other key is inert (it must not leak to the
    // underlying panel). trace:TASK-922 | ai:claude
    if st.help_open() {
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
            st.close_help();
        }
        return Ok(false);
    }

    // The defer revisit-trigger input modal captures all typing until the
    // operator confirms (Enter → run the defer with the typed trigger) or
    // cancels (Esc). Printable chars append; Backspace edits. trace:TASK-921
    if st.defer_input_open() {
        match key.code {
            KeyCode::Enter => {
                if let Some((ids, trigger)) = st.take_defer_input() {
                    apply_outcome(
                        terminal,
                        st,
                        store,
                        loaded_spec,
                        pending,
                        RunOutcome::Defer { ids, trigger },
                    )?;
                }
            }
            KeyCode::Esc => {
                st.cancel_defer_input();
                st.status = Some("defer cancelled".to_string());
            }
            KeyCode::Backspace => st.pop_defer_char(),
            KeyCode::Char(c) if !c.is_control() => st.push_defer_char(c),
            _ => {}
        }
        return Ok(false);
    }

    // The reply-body input modal (Mail scope, STORY-701) captures all typing
    // until the operator confirms (Enter → send via `aida mailbox send` with
    // the typed body) or cancels (Esc). An empty/whitespace body cancels
    // without sending, mirroring the new-spec title input. Printable chars
    // append; Backspace edits. trace:STORY-701 | ai:claude
    if st.reply_input_open() {
        match key.code {
            KeyCode::Enter => match st.take_reply_input() {
                Some((to, in_reply_to, body)) => {
                    apply_outcome(
                        terminal,
                        st,
                        store,
                        loaded_spec,
                        pending,
                        RunOutcome::Reply {
                            to,
                            in_reply_to,
                            body,
                        },
                    )?;
                }
                None => st.status = Some("reply: cancelled (empty body)".to_string()),
            },
            KeyCode::Esc => {
                st.cancel_reply_input();
                st.status = Some("reply cancelled".to_string());
            }
            KeyCode::Backspace => st.pop_reply_char(),
            KeyCode::Char(c) if !c.is_control() => st.push_reply_char(c),
            _ => {}
        }
        return Ok(false);
    }

    // The new-spec TITLE input modal captures all typing until the operator
    // confirms (Enter → create a Draft from the typed title) or cancels (Esc).
    // An empty/whitespace title cancels without creating. Printable chars
    // append; Backspace edits. trace:TASK-931 | ai:claude
    if st.new_input_open() {
        match key.code {
            KeyCode::Enter => match st.take_new_input() {
                Some(title) => create_new_spec(st, pending, &title),
                None => st.status = Some("new: cancelled (empty title)".to_string()),
            },
            KeyCode::Esc => {
                st.cancel_new_input();
                st.status = Some("new spec cancelled".to_string());
            }
            KeyCode::Backspace => st.pop_new_char(),
            KeyCode::Char(c) if !c.is_control() => st.push_new_char(c),
            _ => {}
        }
        return Ok(false);
    }

    // The EPIC focus picker captures all keys while open (STORY-697): ↑/↓
    // navigate the (fuzzy-filtered) list; printable chars fuzzy-filter it;
    // Backspace edits the filter; Enter focuses the highlighted epic (and saves
    // the marker); Esc cancels. trace:STORY-697 | ai:claude
    if st.epic_picker_open() {
        match key.code {
            KeyCode::Up => st.picker_move_up(),
            KeyCode::Down => st.picker_move_down(),
            KeyCode::Enter => match st.take_epic_selection() {
                Some(epic) => apply_focus(st, store, cache, loaded, focus_set, project_root, &epic),
                None => st.status = Some("no epic to focus on".to_string()),
            },
            KeyCode::Esc => {
                st.cancel_epic_picker();
                st.status = Some("focus picker cancelled".to_string());
            }
            KeyCode::Backspace => st.pop_picker_char(),
            KeyCode::Char(c) if !c.is_control() => st.push_picker_char(c),
            _ => {}
        }
        return Ok(false);
    }

    // The drive-gate HOLD popup captures input until resolved (STORY-744): `c`
    // clarifies (when the hold is under-specified), `f` forces the drive (when
    // the hold is soft), Esc / q dismiss. A distinct, exclusive popup raised by
    // the drive verb when the zen suitability gate holds the focused spec — so it
    // is handled before the confirm / modal overlays. trace:STORY-744 | ai:claude
    if let Some(hold) = st.gate_hold.clone() {
        match key.code {
            KeyCode::Char('c') | KeyCode::Char('C') if hold.offers_clarify() => {
                clarify_and_reoffer(terminal, st, store, loaded_spec, pending, &hold.id)?;
            }
            KeyCode::Char('f') | KeyCode::Char('F') if hold.offers_force() => {
                st.gate_hold = None;
                // Force overrides the SOFT hold; it keeps the ADR-6 default
                // route (solo = false), it does NOT split out. trace:TASK-1076
                if spawn_drive(&hold.id, true, false) {
                    st.status = Some(format!(
                        "drive FORCE-launched for {} — watch it with `aida drain status`",
                        hold.id
                    ));
                } else {
                    st.status = Some(format!(
                        "drive: FAILED to force-launch the drive for {}",
                        hold.id
                    ));
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                st.gate_hold = None;
                st.status = Some("drive hold dismissed".to_string());
            }
            _ => {}
        }
        return Ok(false);
    }

    // The drive-ROUTING popup captures input until resolved (TASK-1076): `s`
    // toggles the --solo route (split out vs route into the scope worktree),
    // Enter / d launches with the current toggle, Esc / q cancels. Raised by the
    // drive verb on a READY spec whose default ADR-6 route would carry it into a
    // scope worktree — so the operator sees WHERE it would run and can split it
    // out before launching. trace:TASK-1076 | ai:claude
    if let Some(routing) = st.drive_routing.clone() {
        match key.code {
            KeyCode::Char('s') | KeyCode::Char('S') => {
                if let Some(r) = st.drive_routing.as_mut() {
                    r.solo = !r.solo;
                    st.status = Some(if r.solo {
                        format!("drive {}: --solo (own worktree) — Enter to launch", r.id)
                    } else {
                        format!(
                            "drive {}: into {} worktree (ADR-6) — Enter to launch",
                            r.id, r.scope
                        )
                    });
                }
            }
            KeyCode::Enter | KeyCode::Char('d') | KeyCode::Char('D') => {
                st.drive_routing = None;
                // solo == false preserves the ADR-6 default route (no --solo).
                if spawn_drive(&routing.id, false, routing.solo) {
                    let how = if routing.solo {
                        "solo (own worktree)".to_string()
                    } else {
                        format!("into the {} scope worktree", routing.scope)
                    };
                    st.status = Some(format!(
                        "drive launched for {} {how} — watch it with `aida drain status`",
                        routing.id
                    ));
                } else {
                    st.status = Some(format!(
                        "drive: FAILED to launch the drive for {}",
                        routing.id
                    ));
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                st.drive_routing = None;
                st.status = Some("drive routing cancelled".to_string());
            }
            _ => {}
        }
        return Ok(false);
    }

    // A confirmation popup captures input until resolved.
    if st.confirm.is_some() {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                let outcome = st.resolve_confirm(true);
                apply_outcome(terminal, st, store, loaded_spec, pending, outcome)?;
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
    //
    // CAROUSEL (STORY-710 part A): while an item-body modal is open, Left/Right
    // move to the prev/next spec WITHOUT closing — re-loading that spec into the
    // modal in-process and resetting the scroll. Up/Down/j/k/PgUp/PgDn stay
    // SCROLL within the current spec; Esc/q/p still close. trace:STORY-710
    if st.modal_open() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('p') => {
                st.close_modal();
                *loaded_spec = None;
            }
            KeyCode::Right => carousel_modal(st, store, loaded_spec, 1),
            KeyCode::Left => carousel_modal(st, store, loaded_spec, -1),
            KeyCode::Down | KeyCode::Char('j') => st.modal_scroll_down(1),
            KeyCode::Up | KeyCode::Char('k') => st.modal_scroll_up(1),
            KeyCode::PageDown | KeyCode::Char(' ') => st.modal_scroll_down(MODAL_PAGE),
            KeyCode::PageUp => st.modal_scroll_up(MODAL_PAGE),
            _ => {}
        }
        return Ok(false);
    }

    // FIND mode (TASK-945): an explicit `/` query owns the keyboard for the
    // top-level LIST filter (scopes/verbs/items). Printable chars live-filter
    // the focused list; Backspace edits; Up/Down still move the highlight
    // within the filtered set; Enter CONFIRMS (keep the filter, back to normal
    // so hotkeys act on the filtered list); Esc CANCELS (clear the filter, back
    // to normal). This is what removes the need for the old per-hotkey
    // `if st.filter.is_empty()` guards. trace:TASK-945 | ai:claude
    if st.find_mode {
        match key.code {
            KeyCode::Enter => st.confirm_find(),
            KeyCode::Esc => {
                st.cancel_find();
                st.status = Some("find cleared".to_string());
            }
            KeyCode::Backspace => st.pop_filter(),
            KeyCode::Up => st.move_up(),
            KeyCode::Down => st.move_down(),
            KeyCode::Char(c) if !c.is_control() => {
                st.type_char(c);
            }
            _ => {}
        }
        return Ok(false);
    }

    match key.code {
        // `?` opens the context-sensitive help popup (it no longer quits —
        // quit moved to `q` / Esc-at-top / Ctrl-C). trace:TASK-922
        KeyCode::Char('?') => st.open_help(),

        // `F` opens the EPIC focus PICKER (a fuzzy-filterable list of open
        // epics to scope the whole TUI to + its transitive children); `C`
        // clears the focus back to all items. Capital letters so they don't
        // collide with the type-to-filter fallthrough (lowercase) or the
        // bottom-panel a/A. trace:STORY-697 | ai:claude
        KeyCode::Char('F') => match store {
            Some(s) => st.open_epic_picker(s.open_epics()),
            None => st.status = Some("store unavailable — cannot list epics".to_string()),
        },
        KeyCode::Char('C') => clear_focus(st, cache, loaded, focus_set, project_root),

        // `/` ENTERS find mode (TASK-945): the explicit, vim/less/fzf-style
        // gesture for live-filtering the top-level list. Until it is pressed,
        // every printable char below is a HOTKEY — which is why the old
        // `if st.filter.is_empty()` guards are gone. trace:TASK-945 | ai:claude
        KeyCode::Char('/') => st.enter_find_mode(),

        // `n` opens the new-spec TITLE input modal (create a Draft). In NORMAL
        // mode this is an unconditional hotkey — typing can no longer steal it
        // (filtering only happens in find mode). trace:TASK-931 | ai:claude
        KeyCode::Char('n') => st.open_new_input(),

        // `r` live-refreshes — re-read the store in-process so state changes
        // made OUTSIDE the TUI (a status flip via the CLI / another agent)
        // appear without relaunch. Unconditional hotkey in normal mode.
        // trace:TASK-934 | ai:claude
        KeyCode::Char('r') => {
            refresh(st, store, cache, loaded, focus_set);
            // Also force a fresh liveness read on the next visible tick — the
            // manual refresh should override the (now-long, BUG-676) probe TTL so
            // the glyphs re-sync on demand, not only every 20s. trace:BUG-676
            st.liveness.mark_stale();
        }

        // `q` quits. Unconditional hotkey in normal mode; Ctrl-C always quits
        // (handled above); the help popup documents this. trace:TASK-922
        KeyCode::Char('q') => return Ok(true),

        KeyCode::Up => st.move_up(),
        KeyCode::Down => st.move_down(),

        KeyCode::Tab => st.focus_bottom(),
        KeyCode::BackTab => st.focus_top(),

        // Directional navigation (TASK-944): Right = go deeper (toward the
        // verbs), Left = back a level. Up/Down own list movement, so the
        // horizontal arrows are free for depth. trace:TASK-944 | ai:claude
        KeyCode::Right => match (st.focus, st.level) {
            // Scopes panel, OR the items panel at the scope level → OPEN THE
            // VERBS (drill). From the items panel the verb list reflects the
            // focused item's status, because `drill` keeps `bottom_idx` and
            // `current_verbs` keys off the focused item. trace:TASK-944
            (_, Level::Scopes) => {
                st.drill();
            }
            // Items panel under a drilled scope → surface the verbs panel
            // (it already reflects the focused item). trace:TASK-944
            (Focus::Bottom, Level::Verbs) => st.focus_top(),
            // Already on the verbs panel → nothing deeper to open.
            (Focus::Top, Level::Verbs) => {}
        },
        // Left always goes BACK a level (items → scopes, verbs → scopes); it
        // never exits at the top of the stack — that stays Esc's job.
        // trace:TASK-944 | ai:claude
        KeyCode::Left => {
            st.pop();
        }

        KeyCode::Char(' ') => st.toggle_select(),
        KeyCode::Char('a') if st.focus == Focus::Bottom => st.select_all(),
        KeyCode::Char('A') if st.focus == Focus::Bottom => st.select_none(),

        KeyCode::Char('p') => {
            if st.focus == Focus::Bottom {
                open_modal_with_body(st, store, loaded_spec);
            }
        }

        // Esc layering (TASK-945): a confirmed filter is the innermost layer —
        // clear it FIRST (and consume the Esc); only with no filter applied
        // does Esc fall through to the pop-level / top-of-stack-exit behavior.
        KeyCode::Esc => {
            if st.esc_clears_filter() {
                st.status = Some("filter cleared".to_string());
            } else if !st.pop() {
                // Esc at the top-of-stack scope level (no filter) exits.
                return Ok(true);
            }
        }

        KeyCode::Enter => match (st.focus, st.level) {
            // Scope level: Enter DESCENDS to the items/Targets panel (the top
            // panel keeps showing the scopes). Drilling to the verbs is now the
            // Right-arrow gesture. trace:TASK-944 | ai:claude
            (Focus::Top, Level::Scopes) => {
                st.focus_bottom();
            }
            // Verb level, top focus: Enter runs the verb.
            (Focus::Top, Level::Verbs) => {
                let outcome = st.run_verb();
                apply_outcome(terminal, st, store, loaded_spec, pending, outcome)?;
            }
            // Bottom focus: Enter on an item opens its modal (the N=1
            // "preview this spec" case of the same protocol).
            (Focus::Bottom, _) => {
                open_modal_with_body(st, store, loaded_spec);
            }
        },

        // In NORMAL mode printable chars are HOTKEYS — anything not bound above
        // is inert (no type-to-filter fall-through; that lives in find mode).
        // trace:TASK-945 | ai:claude
        _ => {}
    }
    Ok(false)
}

/// Open the item modal for the focused row, loading its full spec in-process
/// (via the open [`SpecStore`]) into `loaded_spec` for native rendering. No
// `aida show` subprocess. trace:STORY-693 | ai:claude
fn open_modal_with_body(
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    loaded_spec: &mut Option<LoadedSpec>,
) {
    *loaded_spec = load_focused_spec(st, store).map(|spec| test_plan_view(st, spec));
    st.open_modal();
}

/// Pure carousel index math. Given the bottom-panel list `all` (real item
/// indices, in list order), the `selected` real indices, and the real index
/// `current` the modal is showing, return the real index of the next
/// (`dir > 0`) or previous (`dir < 0`) item — CLAMPED at the ends (no wrap,
/// chosen for predictability).
///
/// The set carouselled over is the SELECTED subset when anything is selected,
/// otherwise the full `all` list (the list the modal was opened from). Returns
/// `None` when the resolved set is empty or `current` isn't a member (e.g. an
/// externally-opened modal with no real list position).
// trace:STORY-710 | ai:claude
fn carousel_target(current: usize, all: &[usize], selected: &[usize], dir: i32) -> Option<usize> {
    let set: &[usize] = if selected.is_empty() { all } else { selected };
    let pos = set.iter().position(|&i| i == current)?;
    let new_pos = if dir > 0 {
        (pos + 1).min(set.len().saturating_sub(1))
    } else {
        pos.saturating_sub(1)
    };
    set.get(new_pos).copied()
}

/// Carousel the open item-body modal to the prev/next spec without closing it.
/// Moves through the SELECTED subset if anything is selected, else through the
/// whole bottom-panel list; clamps at the ends; re-loads the new item's spec
/// IN-PROCESS (same path as `p`/open_modal) and resets the scroll. No-op for the
/// externally-opened `show` modal (sentinel index, no real list position).
// trace:STORY-710 | ai:claude
fn carousel_modal(
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    loaded_spec: &mut Option<LoadedSpec>,
    dir: i32,
) {
    let Some(current) = st.modal else {
        return; // verb-output modal (show/why stdout) — nothing to carousel.
    };
    // TODO(STORY-710): carousel the external `show`-verb modal from the focused
    // item's list position. It carries a sentinel index (no real list slot), so
    // for now leave it unchanged rather than break it.
    if current == usize::MAX {
        return;
    }
    let all = st.bottom_indices();
    let selected: Vec<usize> = (0..st.items.len()).filter(|&i| st.selected[i]).collect();
    let Some(next) = carousel_target(current, &all, &selected, dir) else {
        return;
    };
    if next == current {
        return; // already at the clamped end — keep the current spec loaded.
    }
    st.modal = Some(next);
    st.modal_scroll = 0;
    let id = st.items[next].id.clone();
    let spec = match store {
        Some(s) => s.load_spec(&id).unwrap_or_else(|| missing_spec(&id)),
        None => missing_spec(&id),
    };
    *loaded_spec = Some(test_plan_view(st, spec));
}

/// For a Test-scope preview (STORY-699), swap the loaded spec's description for
/// its extracted `## Test Plan` section so the modal renders the do→expect steps
/// prominently; falls back to the full description when there is no test plan,
/// and leaves every other scope's spec untouched. The structured field header
// (type/status/priority/tags) is preserved either way. trace:STORY-699 | ai:claude
fn test_plan_view(st: &RedesignState, spec: LoadedSpec) -> LoadedSpec {
    if active_item_scope(st) != Some(Scope::Test) {
        return spec;
    }
    match store::extract_test_plan(&spec.description) {
        Some(plan) => LoadedSpec {
            description: plan,
            ..spec
        },
        None => spec,
    }
}

/// Turn a [`RunOutcome`] into IO. The generic set-level [`RunOutcome::Execute`]
/// path is dispatched by verb: `groom` runs the headless advisor disposition
/// pass in PROPOSE mode (`aida groom`, read-only) and shows the plan in a modal;
/// `archive` shells out to `aida archive <id>` for each selected target (a store
/// write, run async). Any other verb falls through to the latent status-line log
/// (defensive — no other verb reaches the set-level path today).
// trace:STORY-690 trace:STORY-703 | ai:claude
fn apply_outcome(
    _terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    loaded_spec: &mut Option<LoadedSpec>,
    pending: &mut Option<Pending>,
    outcome: RunOutcome,
) -> Result<()> {
    match outcome {
        RunOutcome::Execute {
            verb: Verb::Groom, ..
        } => {
            // `groom` is the marquee advisor gesture: run the headless
            // disposition pass in PROPOSE mode (`aida groom`, no `--apply`) and
            // surface the proposed approve/reject/park/queue plan in the verb
            // modal. PROPOSE is the SAFE default — it only READS the store and
            // prints what it WOULD do; it never mutates. The operator reviews the
            // plan, then acts (approve / queue / reject) via the other verbs or
            // the CLI's `aida groom --apply`. The set-level `ids` are ignored:
            // `aida groom` always weighs the whole open backlog, not a subset.
            // Captured synchronously (it is a deterministic, local store read —
            // no LLM in the propose path), mirroring the `show` / `why` read
            // verbs. trace:STORY-703 | ai:claude
            let (out, title) = run_groom_propose();
            st.open_verb_modal(title, out);
        }
        RunOutcome::Execute {
            verb: Verb::Archive,
            ids,
        } => {
            // `archive` marks each selected spec archived via `aida archive <id>`
            // — a store WRITE per id, so run the whole batch on a background
            // thread (BUG-633 pattern) and report on completion. The selection
            // gate (TASK-954) guarantees `ids` is non-empty here. trace:STORY-703
            let label = format!("archiving {} spec(s)…", ids.len());
            start_pending(pending, st, label, move || {
                let mut archived = Vec::new();
                let mut failed = Vec::new();
                for id in &ids {
                    if archive_spec(id) {
                        archived.push(id.clone());
                    } else {
                        failed.push(id.clone());
                    }
                }
                VerbResult {
                    status: archive_status(&archived, &failed),
                    invalidate: true,
                }
            });
        }
        RunOutcome::Execute { verb, ids } => {
            // Defensive: no other verb reaches the generic set-level path today
            // (the live verbs have dedicated outcome variants, and groom/archive
            // are handled above). Log instead of silently no-opping so a future
            // wiring gap is visible. trace:STORY-690
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
                let spec = store
                    .map(|s| s.load_spec(&id).unwrap_or_else(|| missing_spec(&id)))
                    .unwrap_or_else(|| missing_spec(&id));
                // In the Test scope, `show` surfaces the ## Test Plan section
                // (the same view as a `p` preview). trace:STORY-699
                *loaded_spec = Some(test_plan_view(st, spec));
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
            // (`aida queue add --for advisor <id>`) — not the mailbox. Each
            // route is a SLOW orphan-branch store write, so run the whole batch
            // on a background thread (BUG-633) and report on completion.
            // trace:STORY-690 trace:BUG-633 | ai:claude
            let label = format!("requesting approval for {} spec(s)…", drafts.len());
            start_pending(pending, st, label, move || {
                let mut routed = Vec::new();
                let mut failed = Vec::new();
                for id in &drafts {
                    if queue_for_advisor(id) {
                        routed.push(id.clone());
                    } else {
                        failed.push(id.clone());
                    }
                }
                VerbResult {
                    status: request_approval_status(&routed, &failed, &skipped),
                    invalidate: true,
                }
            });
        }
        RunOutcome::Approve { drafts, skipped } => {
            // Directly approve each draft via the advisor-gated transition
            // (`aida edit <id> --status approved`, run with advisor authority).
            // The do-it-yourself mirror of request approval; run async (BUG-633).
            // trace:TASK-920 trace:BUG-633 | ai:claude
            let label = format!("approving {} spec(s)…", drafts.len());
            start_pending(pending, st, label, move || {
                let mut approved = Vec::new();
                let mut failed = Vec::new();
                for id in &drafts {
                    if approve_spec(id) {
                        approved.push(id.clone());
                    } else {
                        failed.push(id.clone());
                    }
                }
                VerbResult {
                    status: approve_status(&approved, &failed, &skipped),
                    invalidate: true,
                }
            });
        }
        RunOutcome::BatchApprove { ids } => {
            // `approve all` confirmed: batch-approve the derived approvable set
            // — approve + queue every id in ONE call ([`batch_approve`] drives
            // the whole set; each spec gets the advisor-gated approve transition
            // then the implementer-queue route). Each per-spec half is a SLOW
            // orphan-branch store write, so the whole batch runs on a background
            // thread (BUG-633) and reports on completion.
            // trace:TASK-937 trace:BUG-633 | ai:claude
            let label = format!("approving + queueing {} spec(s)…", ids.len());
            start_pending(pending, st, label, move || {
                let outcome = batch_approve(&ids, approve_and_queue_spec);
                VerbResult {
                    status: batch_approve_status(&outcome),
                    invalidate: true,
                }
            });
        }
        RunOutcome::Reject { drafts, skipped } => {
            // Directly reject each draft via the advisor-gated transition
            // (`aida edit <id> --status rejected`, run with advisor authority).
            // The sibling of approve; run async (BUG-633).
            // trace:TASK-949 trace:BUG-633 | ai:claude
            let label = format!("rejecting {} spec(s)…", drafts.len());
            start_pending(pending, st, label, move || {
                let mut rejected = Vec::new();
                let mut failed = Vec::new();
                for id in &drafts {
                    if reject_spec(id) {
                        rejected.push(id.clone());
                    } else {
                        failed.push(id.clone());
                    }
                }
                VerbResult {
                    status: reject_status(&rejected, &failed, &skipped),
                    invalidate: true,
                }
            });
        }
        RunOutcome::Queue { approved, skipped } => {
            // Route each Approved spec to the implementer queue via the
            // RELIABLE path (`aida queue add --for implementer <id>`) — the
            // Approved-conditional mirror of request approval; run async (BUG-633).
            // trace:TASK-915 trace:BUG-633 | ai:claude
            let label = format!("queueing {} spec(s)…", approved.len());
            start_pending(pending, st, label, move || {
                let mut routed = Vec::new();
                let mut failed = Vec::new();
                for id in &approved {
                    if queue_for_implementer(id) {
                        routed.push(id.clone());
                    } else {
                        failed.push(id.clone());
                    }
                }
                VerbResult {
                    status: queue_status(&routed, &failed, &skipped),
                    invalidate: true,
                }
            });
        }
        RunOutcome::Accept { done, skipped } => {
            // The reviewer accepts each finished Done spec: run the
            // implementation-approval transition (`aida edit <id> --status
            // completed`, carrying reviewer authority) and record a reviewer-
            // acceptance comment. The Done-status mirror of Approve; run async
            // (BUG-633). trace:TASK-933 trace:BUG-633 | ai:claude
            let label = format!("accepting {} spec(s)…", done.len());
            start_pending(pending, st, label, move || {
                let mut accepted = Vec::new();
                let mut failed = Vec::new();
                for id in &done {
                    if accept_spec(id) {
                        accepted.push(id.clone());
                    } else {
                        failed.push(id.clone());
                    }
                }
                VerbResult {
                    status: accept_status(&accepted, &failed, &skipped),
                    invalidate: true,
                }
            });
        }
        RunOutcome::OpenDeferInput { ids } => {
            // `defer` needs the operator-supplied `--until` trigger before it
            // can run: open the single-line input modal over the targets. The
            // defer itself fires on Enter (the input-modal confirm path emits
            // RunOutcome::Defer). trace:TASK-921 | ai:claude
            if ids.is_empty() {
                st.status = Some("defer: nothing to park (no specs selected)".to_string());
            } else {
                st.open_defer_input(ids);
            }
        }
        RunOutcome::Defer { ids, trigger } => {
            // Park each spec off the active view with the captured revisit
            // trigger via `aida defer <id> --until "<trigger>"` — run async
            // (BUG-633). trace:TASK-921 trace:BUG-633 | ai:claude
            let label = format!("deferring {} spec(s)…", ids.len());
            start_pending(pending, st, label, move || {
                let mut deferred = Vec::new();
                let mut failed = Vec::new();
                for id in &ids {
                    if defer_spec(id, &trigger) {
                        deferred.push(id.clone());
                    } else {
                        failed.push(id.clone());
                    }
                }
                VerbResult {
                    status: defer_status(&deferred, &failed, &trigger),
                    invalidate: true,
                }
            });
        }
        RunOutcome::Drive { id } => {
            // Kick off the headline autonomous drive on the focused spec by
            // launching `aida zen <id>` as a DETACHED background drive. The
            // cockpit holds the terminal, so it can't host the long-running,
            // interactive drive inline (the prompt's read+dispose surface rule);
            // we spawn it detached with its stdio nulled and point the operator
            // at `aida drain status` to watch it — matching the existing
            // shell-out pattern rather than inventing a PTY host. Unlike the
            // queue/approve/defer verbs this does NOT use `start_pending`: that
            // captures output and blocks on completion, which is exactly wrong
            // for a drive that runs for minutes. trace:STORY-728 | ai:claude
            //
            // STORY-744: the detached drive nulls its stdio, so a zen
            // suitability-gate HOLD (e.g. under-specified) would die silently and
            // the TUI would report a false "drive launched". PROBE the gate first
            // (`aida zen <id> --json`) and only launch when it reports ready; a
            // hold opens the gate-hold popup (reason + clarify / force remedy)
            // instead of a launch confirmation. trace:STORY-744 | ai:claude
            match probe_drive_gate(&id) {
                Ok(v) if v.verdict == "ready" => {
                    // TASK-1076: when the DEFAULT drive would route into a scope
                    // (epic / focus) worktree, DON'T silently launch — surface the
                    // resolved routing + a --solo toggle first, so an epic-parented
                    // spec doesn't quietly join the epic worktree. A solo-by-default
                    // spec (no scope) has nothing to toggle, so it launches straight
                    // away, preserving the pre-TASK-1076 behavior. trace:TASK-1076
                    if v.routes_into_scope() {
                        st.status =
                            Some(format!("drive: confirm routing for {id} — see the popup"));
                        st.drive_routing = Some(state::DriveRouting {
                            id,
                            scope: v.scope,
                            solo: false,
                        });
                    } else if spawn_drive(&id, false, false) {
                        st.status = Some(format!(
                            "drive launched for {id} — watch it with `aida drain status`"
                        ));
                    } else {
                        st.status = Some(format!("drive: FAILED to launch the drive for {id}"));
                    }
                }
                Ok(v) => {
                    // The gate held the spec — surface the reason + remedies
                    // instead of a false launch. Under-specified → clarify;
                    // soft → force.
                    st.status = Some(format!("drive held for {id} — see the popup"));
                    st.gate_hold = Some(GateHold {
                        id,
                        reason: v.reason,
                        clarifiable: v.under_specified,
                        forceable: v.forceable,
                    });
                }
                Err(msg) => {
                    // Could not evaluate the gate — refuse to report a launch we
                    // cannot vouch for.
                    st.status = Some(format!(
                        "drive: could not evaluate the gate for {id} — {msg}"
                    ));
                }
            }
        }
        RunOutcome::OpenReplyInput { to, in_reply_to } => {
            // `reply` needs the operator-typed body before it can send: open
            // the single-line input modal addressed to the sender, threaded
            // onto the focused message. The send itself fires on Enter (the
            // input-modal confirm path emits RunOutcome::Reply).
            // trace:STORY-701 | ai:claude
            st.open_reply_input(to, in_reply_to);
        }
        RunOutcome::Reply {
            to,
            in_reply_to,
            body,
        } => {
            // Send the reply via `aida mailbox send --to <to> --in-reply-to
            // <in_reply_to> "<body>"` — run async (BUG-633 pattern).
            // trace:STORY-701 | ai:claude
            let label = format!("sending reply to {to}…");
            start_pending(pending, st, label, move || {
                let sent = reply_mail(&to, &body, &in_reply_to);
                VerbResult {
                    status: reply_status(&to, sent),
                    invalidate: true,
                }
            });
        }
        RunOutcome::NeedsConfirm(_) => { /* popup already raised by run_verb */ }
        RunOutcome::None => {}
    }
    Ok(())
}

/// Launch the autonomous drive on `id` as a DETACHED background process:
/// `aida zen <id>` (`--force` when `force`, `--solo` when `solo`). Returns
/// `true` if the child spawned. The cockpit can't host the long-running
/// interactive drive in-terminal, so the child's stdio is nulled and it is left
/// to run independently (the operator watches it with `aida drain status`).
/// Mirrors the other verbs' launchers but uses `spawn` (fire-and-forget) instead
/// of `output` (capture-and-wait), and carries advisor authority on the spawned
/// command so the drive isn't refused by the role gate. `--force` is the
/// operator's answer to a SOFT gate hold (STORY-744): it overrides the
/// under-specified / coupled warnings, never the hard refusals. `--solo` (when
/// `solo`) is the operator's answer to the ADR-6 routing affordance (TASK-1076):
/// split the drive out into its OWN worktree + PR instead of routing into the
/// parent-epic / focus scope worktree. `solo == false` preserves the ADR-6
/// default route — it does NOT force `--solo`.
// trace:STORY-728 | ai:claude
// trace:STORY-744 | ai:claude — the `force` path is the gate-hold override.
// trace:TASK-1076 | ai:claude — the `solo` path is the routing toggle.
fn spawn_drive(id: &str, force: bool, solo: bool) -> bool {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["zen", id]);
    if force {
        cmd.arg("--force");
    }
    if solo {
        cmd.arg("--solo");
    }
    // Kicking off the drive commits the team to autonomously execute the spec —
    // an advisor-authority act (like routing it onto the implementer queue), so
    // carry advisor authority on the spawned command.
    cmd.env("AIDA_SESSION_ROLE", "advisor");
    // Detach: the drive outlives this gesture and runs on its own. Null the
    // stdio so it never fights the TUI for the terminal.
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    cmd.spawn().is_ok()
}

/// Handle the `c` (clarify) affordance on a drive-gate hold (STORY-744): SUSPEND
/// the cockpit, launch the INTERACTIVE clarifier (`aida questions clarify <id>`,
/// which hosts `/aida-clarify`) so the operator authors acceptance criteria,
/// then RE-OFFER the drive — re-run the drive gesture, which re-probes the gate
/// and either launches (now ready) or re-opens the hold popup (still held). The
/// hold is cleared before suspending so the popup is gone while the child owns
/// the terminal; the cockpit repaints from scratch on return.
// trace:STORY-744 | ai:claude
fn clarify_and_reoffer(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    loaded_spec: &mut Option<LoadedSpec>,
    pending: &mut Option<Pending>,
    id: &str,
) -> Result<()> {
    st.gate_hold = None;
    let exe = crate::app::aida_exe();
    let id_owned = id.to_string();
    // Hand the terminal to the interactive clarifier while it runs.
    let status = term::suspend_for_child(|| {
        let mut cmd = Command::new(&exe);
        cmd.args(["questions", "clarify", &id_owned]);
        cmd.env("AIDA_SESSION_ROLE", "advisor");
        if let Ok(cwd) = std::env::current_dir() {
            cmd.current_dir(cwd);
        }
        cmd.status()
    });
    // The child scribbled over the alt screen — force a full repaint next frame.
    terminal.clear()?;
    match status {
        Ok(s) if s.success() => {
            // Acceptance authored — PRESENT THE DRIVE VERB AGAIN: re-run the
            // drive gesture so the (now hopefully clean) gate is re-evaluated.
            apply_outcome(
                terminal,
                st,
                store,
                loaded_spec,
                pending,
                RunOutcome::Drive { id: id.to_string() },
            )?;
        }
        Ok(_) => {
            st.status = Some(format!(
                "clarify exited without finishing for {id} — run drive again when it's ready"
            ));
        }
        Err(e) => {
            st.status = Some(format!("clarify: could not launch the clarifier ({e})"));
        }
    }
    Ok(())
}

/// The drive-gate verdict `aida zen <id> --json` emits (STORY-744) — the
/// deserialized mirror of `zen_drive::GateVerdict`. The TUI reads this to
/// evaluate the SAME suitability gate the drive runs, so a gate hold surfaces
/// honestly instead of a false "drive launched".
// trace:STORY-744 | ai:claude
#[derive(Debug, Clone, serde::Deserialize)]
struct DriveGateVerdict {
    /// `"ready"` (clear to drive) or `"hold"` (a gate held it).
    verdict: String,
    /// The operator-facing hold reason (empty when ready).
    reason: String,
    /// The hold is under-specified → a clarify remedy applies.
    under_specified: bool,
    /// The hold is soft → `--force` overrides it.
    forceable: bool,
    /// The DEFAULT (no `--solo`) ADR-6 scope route: `"solo"` or `"into-scope"`.
    /// `#[serde(default)]` keeps an older `aida` binary (which does not emit this
    /// field) parsing — it falls back to `"solo"`, preserving the pre-TASK-1076
    // behavior of launching straight away. trace:TASK-1076 | ai:claude
    #[serde(default)]
    route: String,
    /// The scope (parent epic / active focus) the default drive routes into,
    // when `route == "into-scope"`; empty for solo. trace:TASK-1076 | ai:claude
    #[serde(default)]
    scope: String,
}

impl DriveGateVerdict {
    /// True when the DEFAULT drive would route into a scope (epic / focus)
    /// worktree rather than a solo own-worktree drive — the case that warrants
    // showing the routing + a `--solo` toggle before launching. trace:TASK-1076
    fn routes_into_scope(&self) -> bool {
        self.route == "into-scope" && !self.scope.trim().is_empty()
    }
}

/// Probe the drive-suitability gate for `id` by shelling out to
/// `aida zen <id> --json` and parsing the verdict. Returns `Ok(verdict)` on a
/// clean probe, or `Err(message)` when the probe could not be run or parsed —
/// the caller then refuses to launch (an unverifiable gate must not report a
/// false "drive launched"). Synchronous: the probe is a fast local store read +
/// pure classification (no LLM, no network), like the `show` / `why` reads.
// trace:STORY-744 | ai:claude
fn probe_drive_gate(id: &str) -> std::result::Result<DriveGateVerdict, String> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["zen", id, "--json"]);
    // Advisor authority for parity with the drive it stands in for (the probe
    // itself only reads, but keeps the provenance consistent).
    cmd.env("AIDA_SESSION_ROLE", "advisor");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("could not run the gate probe ({e})"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    parse_gate_probe(out.status.success(), &stdout, &stderr)
}

/// Interpret an `aida zen <id> --json` probe's captured streams into a verdict
/// (or an error message). PURE — split out of [`probe_drive_gate`] so the
/// stream-selection + parse is unit-testable without spawning.
///
/// TASK-1079: the failure branch consults BOTH streams. The TUI captures the
/// child's stdio with pipes, so the child's stdout is NOT a TTY → it runs in
/// AGENT MODE, and TASK-972 makes agent-mode errors print as a structured block
/// on STDOUT, not stderr. Reading only stderr (the pre-TASK-1079 behavior) lost
/// the real reason and reported a generic "the gate probe failed". We prefer
/// stderr (the human-path channel) and fall back to stdout (the agent-error
/// block) so the operator sees the actual failure either way.
// trace:TASK-1079 | ai:claude
fn parse_gate_probe(
    success: bool,
    stdout: &str,
    stderr: &str,
) -> std::result::Result<DriveGateVerdict, String> {
    if !success {
        let err = stderr.trim();
        let out = stdout.trim();
        let msg = if !err.is_empty() {
            err
        } else if !out.is_empty() {
            out
        } else {
            "the gate probe failed"
        };
        return Err(msg.to_string());
    }
    serde_json::from_str::<DriveGateVerdict>(stdout.trim())
        .map_err(|e| format!("could not read the gate verdict ({e})"))
}

/// The argument vector for the cockpit's `groom` gesture: `aida groom` with NO
/// `--apply` — the PROPOSE pass. Propose is the safe default: it reads the open
/// backlog and prints the approve/reject/park/queue plan it WOULD apply, without
/// writing anything. Kept as a pure arg vector (passed to `Command::args`, never
/// a shell string) so the verb shape is unit-testable without spawning.
// trace:STORY-703 | ai:claude
fn groom_args() -> [&'static str; 1] {
    ["groom"]
}

/// Run the headless advisor disposition pass in PROPOSE mode and return
/// `(stdout_or_error, title)` for the verb modal. Captures stdout (colour codes
/// auto-disable on a non-TTY pipe, so the modal text is plain). A non-zero exit
/// or spawn failure yields an error body rather than a panic, so the cockpit
/// always shows *something*. Carries advisor authority for provenance (and so
/// any future gating on the read path isn't surprised); the propose pass itself
/// performs no writes.
// trace:STORY-703 | ai:claude
fn run_groom_propose() -> (String, String) {
    let title = "groom — proposed dispositions (propose-only, nothing applied)".to_string();
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(groom_args());
    cmd.env("AIDA_SESSION_ROLE", "advisor");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(out) => {
            let mut body = String::from_utf8_lossy(&out.stdout).into_owned();
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                body.push_str("\n\n[groom exited non-zero]\n");
                body.push_str(&err);
            }
            if body.trim().is_empty() {
                body = "groom produced no output.".to_string();
            }
            (body, title)
        }
        Err(e) => (format!("failed to run `aida groom`: {e}"), title),
    }
}

/// The argument vector for archiving one spec: `aida archive <id>`. Kept as a
/// pure arg vector (the id is a single arg-vector element, never shell-parsed)
/// so it is unit-testable without spawning.
// trace:STORY-703 | ai:claude
fn archive_args(id: &str) -> [&str; 2] {
    ["archive", id]
}

/// Archive one spec via `aida archive <id>`. Returns `true` on success. Carries
/// advisor authority on the spawned command (archival is an advisor-authority
/// disposition, mirroring the other backlog verbs).
// trace:STORY-703 | ai:claude
fn archive_spec(id: &str) -> bool {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(archive_args(id));
    cmd.env("AIDA_SESSION_ROLE", "advisor");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    matches!(cmd.output(), Ok(out) if out.status.success())
}

/// The status-line confirmation for an `archive` run: which ids were archived
/// and which failed. Pure (no IO) so it is unit testable.
// trace:STORY-703 | ai:claude
fn archive_status(archived: &[String], failed: &[String]) -> String {
    let mut parts = Vec::new();
    if !archived.is_empty() {
        parts.push(format!(
            "archived {}: {}",
            archived.len(),
            archived.join(", ")
        ));
    }
    if !failed.is_empty() {
        parts.push(format!("FAILED to archive: {}", failed.join(", ")));
    }
    if parts.is_empty() {
        return "archive: nothing to archive".to_string();
    }
    parts.join(" · ")
}

/// Shell out for the `why` verb and return `(stdout_or_error, title)`.
///
/// `why` is the ONE remaining read-style shell-out in this module:
/// `aida why <id>`. Its state classifier lives in `aida-cli/burndown.rs` (not
/// in `aida-core`), so making it in-process is a separate task —
/// TODO(why in-process). `show` and the scope lists are now in-process via
// [`SpecStore`] and never reach here. trace:STORY-693 | ai:claude
fn run_item_verb(verb: Verb, id: &str) -> (String, String) {
    let title = format!("{id} — {}", verb.label());
    // `why` and `status` shell out to the matching `aida` subcommand; any other
    // verb is a defensive no-op (item-level `show` is intercepted upstream and
    // served in-process). `status` reuses the per-spec liveness probe wholesale
    // (STORY-694's `aida status <spec>`): queued / In-Progress / live / STALE +
    // session / pid / started / elapsed — no reimplementation here.
    // trace:TASK-953 | ai:claude
    let subcommand = match verb {
        Verb::Why => "why",
        Verb::Status => "status",
        _ => return (String::new(), title),
    };
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args([subcommand, id]);
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
// trace:STORY-690 | ai:claude
fn queue_for_advisor(id: &str) -> bool {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["queue", "add", "--for", "advisor", id]);
    // BUG-630: carry advisor authority on the spawned command (like approve_spec
    // / accept), so `aida queue add` isn't refused by the TASK-647 gate. Once
    // BUG-631 lands (the gate exempts --for advisor), this becomes unnecessary
    // for the advisor-routing case. trace:BUG-630 | ai:claude
    cmd.env("AIDA_SESSION_ROLE", "advisor");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    matches!(cmd.output(), Ok(out) if out.status.success())
}

/// The status-line confirmation for a `request approval` run: which ids were
/// routed, which failed to route, and which were skipped as non-drafts.
// Pure (no IO) so it is render-smoke / unit testable. trace:STORY-690
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
// routing path, the mirror of [`queue_for_advisor`]. trace:TASK-915 | ai:claude
fn queue_for_implementer(id: &str) -> bool {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["queue", "add", "--for", "implementer", id]);
    // BUG-630: dispatching to the implementer queue needs advisor authority
    // (the TASK-647 gate, correctly — this commits the team to execute). Carry
    // it on the spawned command, mirroring approve_spec/accept. trace:BUG-630
    cmd.env("AIDA_SESSION_ROLE", "advisor");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    matches!(cmd.output(), Ok(out) if out.status.success())
}

/// The status-line confirmation for a `queue` run: which ids were routed to
/// the implementer queue, which failed to route, and which were skipped as
/// non-Approved. Pure (no IO) so it is unit testable. The mirror of
// [`request_approval_status`]. trace:TASK-915 | ai:claude
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
// do-it-yourself mirror of [`queue_for_advisor`]. trace:TASK-920 | ai:claude
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
// IO) so it is unit testable. The mirror of [`queue_status`]. trace:TASK-920 | ai:claude
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

/// The per-spec batch-approve operation: approve the spec (the advisor-gated
/// `aida edit <id> --status approved` transition) THEN route it to the
/// implementer queue (`aida queue add --for implementer <id>`). `true` only
/// when BOTH halves succeed — an approved-but-unqueued spec reports as failed
/// so the operator sees it needs a hand. Composes the existing
/// [`approve_spec`] + [`queue_for_implementer`] shell-outs; injected into the
// pure [`batch_approve`] driver by the BatchApprove outcome. trace:TASK-937 | ai:claude
fn approve_and_queue_spec(id: &str) -> bool {
    approve_spec(id) && queue_for_implementer(id)
}

/// The status-line confirmation for an `approve all` batch run: which ids were
/// approved + queued, and which failed either half. Pure (no IO) so it is unit
// testable, like its sibling formatters. trace:TASK-937 | ai:claude
fn batch_approve_status(outcome: &BatchApproveOutcome) -> String {
    let mut parts = Vec::new();
    if !outcome.approved.is_empty() {
        parts.push(format!(
            "approved + queued {}: {}",
            outcome.approved.len(),
            outcome.approved.join(", ")
        ));
    }
    if !outcome.failed.is_empty() {
        parts.push(format!(
            "FAILED to approve + queue: {}",
            outcome.failed.join(", ")
        ));
    }
    if parts.is_empty() {
        return "approve all: nothing approvable".to_string();
    }
    parts.join(" · ")
}

/// Directly reject one draft spec. Returns `true` on success. Runs the
/// advisor-gated transition `aida edit <id> --status rejected` — the rejection
/// transition is REFUSED from a non-advisor identity, so the spawned command
/// carries advisor authority via `AIDA_SESSION_ROLE=advisor` in its env. The
/// sibling of [`approve_spec`].
// trace:TASK-949 | ai:claude
fn reject_spec(id: &str) -> bool {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["edit", id, "--status", "rejected"]);
    // The rejected-status transition is advisor-gated; carry advisor authority
    // on the spawned command so it is not refused as a non-advisor identity.
    cmd.env("AIDA_SESSION_ROLE", "advisor");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    matches!(cmd.output(), Ok(out) if out.status.success())
}

/// The status-line confirmation for a `reject` run: which ids were rejected,
/// which failed the transition, and which were skipped as non-drafts. Pure (no
/// IO) so it is unit testable. The sibling of [`approve_status`].
// trace:TASK-949 | ai:claude
fn reject_status(rejected: &[String], failed: &[String], skipped: &[String]) -> String {
    let mut parts = Vec::new();
    if !rejected.is_empty() {
        parts.push(format!(
            "rejected {}: {}",
            rejected.len(),
            rejected.join(", ")
        ));
    }
    if !failed.is_empty() {
        parts.push(format!("FAILED to reject: {}", failed.join(", ")));
    }
    if !skipped.is_empty() {
        parts.push(format!(
            "skipped {} non-draft(s): {}",
            skipped.len(),
            skipped.join(", ")
        ));
    }
    if parts.is_empty() {
        return "reject: nothing to reject (no drafts selected)".to_string();
    }
    parts.join(" · ")
}

/// The argument vector for the reviewer's implementation-approval transition:
/// `aida edit <id> --status completed`. The Done-status counterpart to
/// `approve`'s `--status approved`. Kept as a pure arg vector (passed to
/// `Command::args`, never a shell string) so the id — and the verb shape — are
// unit-testable without spawning. trace:TASK-933 | ai:claude
fn accept_edit_args(id: &str) -> Vec<&str> {
    vec!["edit", id, "--status", "completed"]
}

/// The argument vector for the reviewer-acceptance comment recorded alongside
/// the accept transition: `aida comment add <id> "<note>"`. The note is a
/// SINGLE arg-vector element, so it is never shell-parsed (no command
// substitution, no globbing). Pure so it is unit-testable. trace:TASK-933 | ai:claude
fn accept_comment_args(id: &str) -> [&str; 4] {
    [
        "comment",
        "add",
        id,
        "accepted by reviewer: implementation reviewed and accepted (Done -> Completed)",
    ]
}

/// Accept one finished Done spec as the reviewer. Returns `true` when the
/// Done → Completed transition succeeded.
///
/// WRINKLE (investigated TASK-933): `completed` is documented as merge-driven —
/// auto-bumped by `aida pull` when a `(SPEC-ID)`-trailered commit lands on the
/// default branch. But a *manual* `Done → Completed` edit is NOT hard-gated:
/// the lifecycle `transition_guard` only requires advisor authority for
/// `from ∈ {Draft, NeedsAttention}` (un-triaged/punted intent), so a
/// `Done → Completed` flip is an un-gated, implementer-legitimate transition
/// (the same path `aida done` rides). So `accept` runs the real transition
/// rather than faking it. The spawned command carries `AIDA_SESSION_ROLE=reviewer`
/// to record reviewer provenance (and to mirror `approve_spec`'s authority env).
/// The reviewer-acceptance comment is best-effort (recorded after a successful
/// transition); the transition itself is the load-bearing result.
///
/// NUANCE: in the *full* multi-machine flow, final Completed still comes from
/// the merge auto-bump on `aida pull`; this in-TUI accept completes the spec for
/// the reviewer-at-the-keyboard walkthrough. The Done-status mirror of
// [`approve_spec`]. trace:TASK-933 | ai:claude
fn accept_spec(id: &str) -> bool {
    let exe = crate::app::aida_exe();
    let cwd = std::env::current_dir().ok();
    let mut edit = Command::new(&exe);
    edit.args(accept_edit_args(id));
    // The Done → Completed transition is reviewer work — carry reviewer
    // authority on the spawned command for provenance (and role activity).
    edit.env("AIDA_SESSION_ROLE", "reviewer");
    if let Some(cwd) = cwd.as_ref() {
        edit.current_dir(cwd);
    }
    let completed = matches!(edit.output(), Ok(out) if out.status.success());
    if !completed {
        return false;
    }
    // Best-effort: record the reviewer-acceptance comment. A failed comment
    // does NOT un-accept the spec — the transition above is the load-bearing
    // result — so the accept is still reported as succeeded.
    let mut note = Command::new(&exe);
    note.args(accept_comment_args(id));
    note.env("AIDA_SESSION_ROLE", "reviewer");
    if let Some(cwd) = cwd.as_ref() {
        note.current_dir(cwd);
    }
    let _ = note.output();
    true
}

/// The status-line confirmation for an `accept` run: which ids the reviewer
/// accepted (Done → Completed), which failed the transition, and which were
/// skipped as non-Done. Pure (no IO) so it is unit testable. The mirror of
// [`approve_status`]. trace:TASK-933 | ai:claude
fn accept_status(accepted: &[String], failed: &[String], skipped: &[String]) -> String {
    let mut parts = Vec::new();
    if !accepted.is_empty() {
        parts.push(format!(
            "accepted {} (Done → Completed): {}",
            accepted.len(),
            accepted.join(", ")
        ));
    }
    if !failed.is_empty() {
        parts.push(format!("FAILED to accept: {}", failed.join(", ")));
    }
    if !skipped.is_empty() {
        parts.push(format!(
            "skipped {} non-done: {}",
            skipped.len(),
            skipped.join(", ")
        ));
    }
    if parts.is_empty() {
        return "accept: nothing to accept (no Done specs selected)".to_string();
    }
    parts.join(" · ")
}

/// Park one spec off the active view with a revisit trigger. Returns `true` on
/// success. Runs `aida defer <id> --until "<trigger>"` — the trigger is passed
// as a single argument (no shell), so embedded spaces are safe. trace:TASK-921 | ai:claude
fn defer_spec(id: &str, trigger: &str) -> bool {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["defer", id, "--until", trigger]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    matches!(cmd.output(), Ok(out) if out.status.success())
}

/// The status-line confirmation for a `defer` run: which ids were parked (with
/// the revisit trigger) and which failed. Pure (no IO) so it is unit testable.
// The mirror of [`approve_status`]. trace:TASK-921 | ai:claude
fn defer_status(deferred: &[String], failed: &[String], trigger: &str) -> String {
    let mut parts = Vec::new();
    if !deferred.is_empty() {
        parts.push(format!(
            "deferred {} until \"{}\": {}",
            deferred.len(),
            trigger,
            deferred.join(", ")
        ));
    }
    if !failed.is_empty() {
        parts.push(format!("FAILED to defer: {}", failed.join(", ")));
    }
    if parts.is_empty() {
        return "defer: nothing parked".to_string();
    }
    parts.join(" · ")
}

/// Send a reply via `aida mailbox send`, built through the pure
/// [`mail::send_mail_argv`] (never a shell string — `body` is passed as an
/// OS-level argument, never shell-parsed). Mirrors [`defer_spec`]'s
/// shell-out shape. Role-agnostic: unlike `queue_for_advisor` / `accept_spec`,
/// no `AIDA_SESSION_ROLE` override — any role may send mail.
// trace:STORY-701 | ai:claude
fn reply_mail(to: &str, body: &str, in_reply_to: &str) -> bool {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(mail::send_mail_argv(to, body, Some(in_reply_to), false));
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    matches!(cmd.output(), Ok(out) if out.status.success())
}

/// The status-line confirmation for a `reply` run. Pure (no IO) so it is unit
/// testable.
// trace:STORY-701 | ai:claude
fn reply_status(to: &str, sent: bool) -> String {
    if sent {
        format!("reply sent to {to}")
    } else {
        format!("reply: FAILED to send to {to}")
    }
}

/// The argument vector for creating a Draft spec — passed to `Command::args`
/// so each element (notably the operator-typed `title`) is an OS-level argument
/// that is NEVER shell-parsed. A title with backticks, quotes, or `$` is inert
/// (no command substitution, no globbing) because there is no shell in the
/// pipeline. Pure (no IO) so the safe arg vector is unit-testable without
/// spawning.
///
/// When a focus epic is active (`parent` is `Some`), the new Draft is filed
/// under it via `--parent <epic>` so work created while focused on EPIC-X is
/// auto-linked to EPIC-X instead of orphaned — and so the focus lens does not
/// immediately hide the freshly-created spec. With no focus (`parent` is
/// `None`) the args are unchanged and the spec is created unparented as before.
// trace:TASK-931 trace:TASK-942 | ai:claude
fn new_spec_args<'a>(title: &'a str, parent: Option<&'a str>) -> Vec<&'a str> {
    let mut args = vec![
        "add", "--title", title, "--type", "task", "--status", "draft",
    ];
    if let Some(parent) = parent {
        args.push("--parent");
        args.push(parent);
    }
    args
}

/// Parse the spec id out of `aida add`'s success line (`Added: TASK-932 - …`).
/// Returns `None` when no such line is present (or the id is the `?` placeholder
/// the CLI prints when a spec_id wasn't assigned). Pure so it is unit-testable.
// trace:TASK-931 | ai:claude
fn parse_created_spec_id(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|l| {
        let rest = l.strip_prefix("Added: ")?;
        let id = rest.split(" - ").next()?.trim();
        if id.is_empty() || id == "?" {
            None
        } else {
            Some(id.to_string())
        }
    })
}

/// The status-line confirmation for a `new` create: the created spec id (when
/// the CLI reported one) plus a truncated title. When the create was filed under
/// an active focus epic (`parent` is `Some`), the confirmation names the parent
/// so the operator sees the new draft was linked (not orphaned) — the visible
/// counterpart to the silent focus-hiding the link prevents. Pure (no IO) so it
/// is unit testable.
// trace:TASK-931 trace:TASK-942 | ai:claude
fn create_status(created: Option<&str>, title: &str, parent: Option<&str>) -> String {
    let short: String = if title.chars().count() > 50 {
        format!("{}…", title.chars().take(50).collect::<String>())
    } else {
        title.to_string()
    };
    let base = match created {
        Some(id) => format!("created {id} (Draft): {short}"),
        None => format!("created Draft spec: {short}"),
    };
    match parent {
        Some(epic) => format!("{base} (under {epic})"),
        None => base,
    }
}

/// Run `aida add --title <title> --type task --status draft [--parent <epic>]`
/// and turn its result into a [`VerbResult`]. The title is passed as a SINGLE
/// arg-vector element (see [`new_spec_args`]) — never a shell string — so
/// embedded backticks / quotes / `$` are safe. When `parent` is `Some` (an
/// active focus epic), the new draft is filed under it so it is not orphaned and
/// the focus lens keeps it visible (TASK-942). A successful create invalidates
/// the scope cache so the new draft shows if it is in view (e.g. the Open scope,
/// or the focus subtree). Runs ON THE WORKER THREAD (no `st`/cache borrows), so
/// the slow store write never blocks the loop.
// trace:TASK-931 trace:TASK-942 trace:BUG-633 | ai:claude
fn run_create(title: &str, parent: Option<String>) -> VerbResult {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(new_spec_args(title, parent.as_deref()));
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let created = parse_created_spec_id(&stdout);
            VerbResult {
                status: create_status(created.as_deref(), title, parent.as_deref()),
                // Invalidate so the next sync_scope_items re-fetches in-process
                // and the new draft appears if it is in view.
                invalidate: true,
            }
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            VerbResult {
                status: format!("new: aida add failed: {}", err.trim()),
                invalidate: false,
            }
        }
        Err(e) => VerbResult {
            status: format!("new: could not run aida add: {e}"),
            invalidate: false,
        },
    }
}

/// Create a fresh Draft spec from the operator-typed `title`, running the slow
/// `aida add` store write on a background thread (BUG-633) so the TUI never
/// freezes. When a focus epic is active (`st.focus_epic`), the new draft is
/// filed under it (`--parent <epic>`) so work created while focused on EPIC-X is
/// auto-linked to EPIC-X instead of orphaned — otherwise the focus lens would
/// immediately hide the parentless spec ("I created a spec but it vanished").
/// With no active focus the draft is created unparented as before. The focus
/// notion reused is the redesign's own `focus_epic` (STORY-697: `AIDA_TUI_EPIC`
/// env > `.aida/tui-focus` marker > branch inference), the same source the
/// cockpit lens uses to decide what is in view — so the create and the lens
/// agree. The work + result-shaping live in [`run_create`]; this only kicks off
/// the pending op.
// trace:TASK-931 trace:TASK-942 trace:BUG-633 | ai:claude
fn create_new_spec(st: &mut RedesignState, pending: &mut Option<Pending>, title: &str) {
    let title = title.to_string();
    let parent = st.focus_epic.clone();
    let label = "creating spec…".to_string();
    start_pending(pending, st, label, move || run_create(&title, parent));
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

fn render(
    f: &mut Frame,
    st: &RedesignState,
    loaded_spec: Option<&LoadedSpec>,
    pending: Option<&PendingOp>,
) {
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
    render_hint(f, rows[3], st, theme, pending);

    // The item modal renders the spec loaded IN-PROCESS (`loaded_spec`):
    // structured fields + native body. trace:STORY-693 | ai:claude
    if st.modal.is_some() {
        if let Some(spec) = loaded_spec {
            // Carousel is offered only when the set has more than one item to
            // move through (the selected subset, else the whole bottom list),
            // and never for the externally-opened `show` modal. trace:STORY-710
            let carousel = st.modal != Some(usize::MAX) && {
                let set_len = if st.selected_count() > 0 {
                    st.selected_count()
                } else {
                    st.bottom_len()
                };
                set_len > 1
            };
            render_modal(f, f.area(), theme, spec, st.modal_scroll, carousel);
        }
    }
    if let Some(vm) = &st.verb_modal {
        render_verb_modal(f, f.area(), theme, &vm.title, &vm.body, st.modal_scroll);
    }
    if let Some(c) = st.confirm {
        render_confirm(f, f.area(), theme, c.verb, c.count);
    }
    // The drive-gate HOLD popup (STORY-744) overlays the panels — a distinct,
    // exclusive popup raised by the drive verb when the zen suitability gate
    // holds the focused spec. trace:STORY-744 | ai:claude
    if let Some(h) = &st.gate_hold {
        render_gate_hold(f, f.area(), theme, h);
    }
    // The drive-ROUTING popup (TASK-1076) overlays the panels — a distinct,
    // exclusive popup raised by the drive verb to surface the resolved ADR-6
    // route + a --solo toggle before launching. trace:TASK-1076 | ai:claude
    if let Some(r) = &st.drive_routing {
        render_drive_routing(f, f.area(), theme, r);
    }
    // The defer revisit-trigger input overlays everything else. trace:TASK-921
    if let Some(di) = &st.defer_input {
        render_defer_input(f, f.area(), theme, di);
    }
    // The reply-body input overlays everything else. trace:STORY-701
    if let Some(ri) = &st.reply_input {
        render_reply_input(f, f.area(), theme, ri);
    }
    // The new-spec title input overlays everything else. trace:TASK-931
    if let Some(ni) = &st.new_input {
        render_new_input(f, f.area(), theme, ni);
    }
    // The EPIC focus picker overlays everything else. trace:STORY-697
    if let Some(p) = &st.epic_picker {
        render_epic_picker(f, f.area(), theme, p);
    }
    // The '?' help popup overlays everything — it is the topmost layer.
    // trace:TASK-922 | ai:claude
    if st.help_open() {
        render_help(f, f.area(), theme, &st.help_content());
    }
}

/// Render the context-sensitive '?' help popup: a header (where you are /
/// what's selected), the focused element's help body, and a key legend for
/// the current context. Content comes from the pure [`state::help_for`] via
// `st.help_content()`, so this is render-only. trace:TASK-922 | ai:claude
fn render_help(f: &mut Frame, area: Rect, theme: &Theme, hc: &state::HelpContent) {
    let popup = centered(area, 70, 70);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.accent))
        .title(format!(" Help — {} (Esc / ? to close) ", hc.header));

    let mut lines: Vec<Line> = Vec::new();
    // Body, wrapped as a paragraph (split on the blank-line boundaries the
    // help strings don't carry — render as one wrapped block).
    lines.push(Line::from(Span::styled(
        hc.body.clone(),
        Style::default().fg(theme.fg),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Keys here:",
        Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
    )));
    for key in &hc.legend {
        lines.push(Line::from(vec![
            Span::styled("  • ", Style::default().fg(theme.dim)),
            Span::styled(key.clone(), Style::default().fg(theme.info)),
        ]));
    }

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(block);
    f.render_widget(para, popup);
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
    let mut spans = vec![
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
    // The EPIC focus lens (STORY-695): when set, show `focus: <EPIC>` plus the
    // filtered-set progress summary as a distinct, accent-coloured chip so the
    // operator always knows the whole TUI is narrowed. trace:STORY-695
    if let Some(epic) = &st.focus_epic {
        let label = match &st.focus_summary {
            Some(summary) => format!(" focus: {epic} — {summary} "),
            None => format!(" focus: {epic} "),
        };
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            label,
            Style::default()
                .fg(theme.on_accent)
                .bg(theme.info)
                .add_modifier(Modifier::BOLD),
        ));
    }
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
        // Grey out + relabel a verb that doesn't apply to the selected spec, on
        // any of four COMPOSING axes (the operator still SEES the full verb
        // vocabulary — quiet-depth discoverability — but inapplicable rows
        // render dimmed, non-selectable, with a reason instead of the normal
        // hint):
        //   * WIRED (STORY-724): a verb not yet wired -> "not yet available".
        //     STORY-703 wired the last two stubs (`groom`/`archive`), so no verb
        //     trips this axis today, but the gate stays as the structural home
        //     for any future not-yet-wired verb.
        //   * ROLE (BUG-638): an advisor-/reviewer-only verb the active role
        //     would be refused for -> "requires the <role> role".
        //   * STATUS (TASK-947): a status-conditional verb the FOCUSED item's
        //     status doesn't apply to (e.g. `approve` on a non-Draft, `accept`
        //     on a non-Done) -> "only for <Status> specs".
        //   * SELECTION (TASK-954): an UPDATE verb that acts on the explicit
        //     selection set when nothing is selected (none = all is safe for a
        //     read, a silent mutation for an update) -> "select item(s) first".
        // A verb greys if ANY axis disqualifies it; the hint follows a
        // most-fundamental-wins precedence — wired (act doesn't exist yet) >
        // role (seat mismatch) > status (lifecycle mismatch) > selection
        // (transient UI state) > keystone (focused spec stays human-supervised).
        // trace:STORY-724 trace:TASK-954 trace:TASK-947 trace:BUG-638 trace:STORY-728
        let mut hint = hint.to_string();
        let mut disabled = false;
        if st.level == Level::Verbs {
            let v = st.current_verbs()[real];
            // WIRED axis (STORY-724): a verb not yet wired greys with "not yet
            // available" — checked first because an unwired verb is inert
            // regardless of role / status / selection. STORY-703 wired the last
            // two stubs, so this currently disqualifies nothing, but the gate is
            // retained for any future not-yet-wired verb.
            if !v.is_functional() {
                disabled = true;
                hint = "not yet available".to_string();
            } else if !st.verb_role_permitted(v) {
                disabled = true;
                if let Some(req) = v.required_role() {
                    hint = format!("requires the {req} role");
                }
            } else if !st.verb_status_permitted(v) {
                disabled = true;
                if let Some(req) = st.verb_status_hint(v) {
                    hint = format!("only for {req} specs");
                }
            } else if !st.verb_selection_permitted(v) {
                disabled = true;
                hint = "select item(s) first".to_string();
            } else if !st.verb_keystone_permitted(v) {
                // KEYSTONE axis (STORY-728): `drive` is refused on a keystone /
                // architecture-class focused spec — that work stays human-
                // supervised rather than shipping on an autonomous default.
                disabled = true;
                hint = "keystone — stays human-supervised".to_string();
            }
        }
        let marker = if selected { "▸ " } else { "  " };
        // Non-wired scopes are dimmed; a disabled verb is dimmed too, even when
        // it is the cursor row, so the greyed state survives the highlight.
        let dim_label = (!drills && st.level == Level::Scopes) || disabled;
        let style = if disabled {
            Style::default().fg(theme.dim)
        } else {
            row_style(theme, selected && focused, dim_label)
        };
        // A disabled verb keeps the plain (dim) weight; only an enabled label is
        // bolded, so the disabled rows read as visibly inert.
        let label_style = if disabled {
            style
        } else {
            style.add_modifier(Modifier::BOLD)
        };
        let line = Line::from(vec![
            Span::styled(marker, style),
            Span::styled(format!("{glyph} "), style),
            Span::styled(format!("{label:<10}"), label_style),
            Span::styled(format!("  {hint}"), Style::default().fg(theme.dim)),
        ]);
        lines.push(line);
    }
    // The `/query` find prompt renders ONLY in find mode (TASK-945) — a
    // confirmed filter narrows the list silently; the prompt is the live
    // editing surface, not a persistent badge. trace:TASK-945 | ai:claude
    if focused && st.find_mode {
        lines.insert(
            0,
            Line::from(Span::styled(
                format!("/{}", st.filter),
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
        // Scope-appropriate empty state: the Queue scope's "nothing here" means
        // an empty queue, not an empty backlog; Mail's means a caught-up inbox.
        // trace:TASK-948 trace:STORY-701
        let empty_msg = match active_item_scope(st) {
            Some(Scope::Queue) => "(queue empty — route work with `aida queue add --for <role>`)",
            Some(Scope::Mail) => "(no unread mail — you're caught up)",
            _ => "(no backlog items — file some with `aida add --status approved`)",
        };
        f.render_widget(
            Paragraph::new(Span::styled(empty_msg, Style::default().fg(theme.dim))).block(block),
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
    // The leading control cells (liveness glyph + cursor marker + checkbox)
    // reserve a fixed, mode-independent prefix; the title gets whatever width
    // remains. "● ▸[x] " = liveness(1) + space(1) + marker(1) + checkbox(3) +
    // space(1) = 7 visible cols, then each column + its single-space separator.
    // trace:TASK-978 | ai:claude
    let live_w = 2; // leading liveness glyph (1) + space (1)
    let prefix_w = 5;
    let glyph_w = 2; // status glyph (1) + space (1)
    let fixed = live_w
        + prefix_w
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

        // Leading per-row liveness glyph (TASK-978): ● live / ⚠ stale / ◦ idle,
        // sourced from `aida ps --json` (the cached `st.liveness` probe), so an
        // operator watching a drive sees at a glance which targets are live vs
        // orphaned/stale. Rides the structural style over the cursor highlight
        // (contrast), else its own semantic colour. trace:TASK-978 | ai:claude
        let live = st.liveness.for_id(&item.id);
        let live_glyph = list_row::liveness_glyph(live, mode);
        let live_style = if cursor_active {
            structural
        } else {
            list_row::liveness_style(live, theme)
        };

        let mut row_spans = vec![
            Span::styled(format!("{live_glyph} "), live_style),
            Span::styled(format!("{marker}{checkbox} "), structural),
            Span::styled(format!("{} ", cells.id), structural),
            Span::styled(format!("{} ", cells.req_type), structural),
            Span::styled(format!("{} ", cells.status_glyph), status_style),
            Span::styled(format!("{} ", cells.status_label), status_style),
            Span::styled(format!("{} ", cells.priority), priority_style),
            Span::styled(cells.title, structural),
        ];
        // Test scope marker: a trailing glyph on rows whose description carries
        // a `## Test Plan` section, so the operator sees at a glance which
        // shipped specs have verification steps. trace:STORY-699 | ai:claude
        if item.has_test_plan {
            row_spans.push(Span::styled(
                format!(" {TEST_PLAN_MARKER}"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        // Queue scope routing badge (TASK-948): a trailing `->role` on rows that
        // sit on a role's queue, so a routed spec is visibly distinct from an
        // unrouted one (the "I routed it and it vanished" gap). Painted in the
        // accent colour like the test-plan marker; left off (not greyed) when
        // the entry is unrouted (`routed_role == None`). On the cursor-
        // highlighted row it rides the structural style so it stays legible over
        // the accent fill. trace:TASK-948 | ai:claude
        if let Some(role) = &item.routed_role {
            let badge_style = if cursor_active {
                structural
            } else {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            };
            row_spans.push(Span::styled(format!(" ->{role}"), badge_style));
        }
        lines.push(Line::from(row_spans));
    }
    // The `/query` find prompt renders ONLY in find mode (TASK-945).
    // trace:TASK-945 | ai:claude
    if focused && st.find_mode {
        lines.insert(
            0,
            Line::from(Span::styled(
                format!("/{}", st.filter),
                Style::default().fg(theme.info),
            )),
        );
    }
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_hint(
    f: &mut Frame,
    area: Rect,
    st: &RedesignState,
    theme: &Theme,
    pending: Option<&PendingOp>,
) {
    // A pending background verb (BUG-633) owns the status line: spinner glyph +
    // label (e.g. "⠙ approving TASK-930…"), painted in the accent colour so the
    // operator sees the TUI is alive + working. trace:BUG-633 | ai:claude
    if let Some(op) = pending {
        f.render_widget(
            Paragraph::new(Span::styled(
                op.status_line(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            area,
        );
        return;
    }
    // The focus-key hint (F set/change · C clear) is appended to the base hint
    // in every context so the EPIC lens is discoverable. trace:STORY-695
    let base = match (st.focus, st.level) {
        (Focus::Top, Level::Scopes) => {
            "↵ drill · Tab items · / find · F focus · C clear · ? help · q quit"
        }
        (Focus::Top, Level::Verbs) => {
            "↵ run · Tab items · Esc back · / find · F focus · C clear · ? help · q quit"
        }
        (Focus::Bottom, _) => {
            "Space select · a/A all/none · p preview · / find · F focus · C clear · ⇧Tab back · ? help · q quit"
        }
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
// scrolling the body off the top). trace:STORY-693 trace:TASK-913 | ai:claude
fn render_modal(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    spec: &LoadedSpec,
    scroll: u16,
    carousel: bool,
) {
    let popup = centered(area, 80, 80);
    f.render_widget(Clear, popup);
    let lines = spec_lines(spec, theme);
    // The inner text area is the popup minus its two border columns/rows. The
    // scrollbar overlays the right border, so it does NOT steal a text column.
    let inner_w = popup.width.saturating_sub(2);
    let inner_h = popup.height.saturating_sub(2);
    // Clamp against the WRAPPED row count, not the logical line count: the
    // Paragraph wraps each long line (Wrap { trim: false }) into many visual
    // rows, so `lines.len()` drastically under-counts the real height and
    // PageDown would stop short. `line_count` is the post-word-wrap row count.
    // trace:BUG-635 | ai:claude
    let max_scroll = modal_max_scroll(&lines, inner_w, inner_h);
    let scroll = scroll.min(max_scroll);
    let scrollable = max_scroll > 0;
    // The title hint advertises the carousel (←/→ prev/next) only when there is
    // more than one item to move through, the scroll keys only when the body
    // overflows, and always the close keys. trace:STORY-710 | ai:claude
    let title = match (carousel, scrollable) {
        (true, true) => format!(
            " {} (←/→ prev/next · ↑↓/PgUp/PgDn scroll · Esc/q/p close) ",
            spec.id
        ),
        (true, false) => format!(" {} (←/→ prev/next · Esc/q/p close) ", spec.id),
        (false, true) => format!(" {} (↑↓/PgUp/PgDn scroll · Esc/q/p close) ", spec.id),
        (false, false) => format!(" {} (Esc/q/p to close) ", spec.id),
    };
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.accent))
        .title(title);
    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
        .block(block);
    f.render_widget(para, popup);
    render_modal_scrollbar(f, popup, theme, max_scroll, scroll);
}

/// Compute the maximum vertical scroll offset for a modal whose `lines` are
/// rendered into a bordered, `Wrap { trim: false }` Paragraph of inner size
/// `inner_w` x `inner_h`. The clamp is against the WRAPPED row count (what the
/// Paragraph actually paints) rather than `lines.len()` (the logical count),
// because long lines wrap into many visual rows. trace:BUG-635 | ai:claude
fn modal_max_scroll(lines: &[Line], inner_w: u16, inner_h: u16) -> u16 {
    // `Paragraph::line_count(width)` returns the wrapped row count PLUS the
    // block's vertical space (top+bottom borders = 2). We pass the same
    // bordered Paragraph shape and subtract those 2 border rows to get the
    // pure text-row count, then subtract the visible height.
    let para = Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .block(Block::bordered());
    let wrapped_rows = (para.line_count(inner_w) as u16).saturating_sub(2);
    wrapped_rows.saturating_sub(inner_h.max(1))
}

/// Render the modal's vertical scrollbar on the right inner edge — only when
/// the content actually overflows (`max_scroll > 0`). `content_length` is the
/// scrollable range and `position` the current offset, so the thumb tracks
// where the body is. trace:BUG-635 | ai:claude
fn render_modal_scrollbar(f: &mut Frame, popup: Rect, theme: &Theme, max_scroll: u16, scroll: u16) {
    if max_scroll == 0 {
        return;
    }
    let mut sb_state = ScrollbarState::new(max_scroll as usize).position(scroll as usize);
    let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .style(Style::default().fg(theme.accent));
    // Inset vertically by one row so the thumb track sits between the top and
    // bottom borders (not over the corner glyphs); horizontal margin 0 keeps
    // it on the right border column.
    let sb_area = popup.inner(Margin {
        vertical: 1,
        horizontal: 0,
    });
    f.render_stateful_widget(sb, sb_area, &mut sb_state);
}

/// Build the native modal body: a structured header (title + a color-coded
/// field row + tags) then the description rendered as markdown. Pure (no IO)
// so it is render-smoke / unit testable. trace:STORY-693 trace:TASK-913
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

    // Relationship graph section (STORY-739): the spec's typed graph — parent
    // epic, children, blocked-by / blocks chains, references — surfaced right
    // after the body. This is AIDA's #1 differentiator (the requirement graph
    // `aida show` / `aida graph` print on the CLI) shown at the natural dig-in
    // gesture. Omitted entirely when the spec has no relationships, so an
    // unconnected spec shows no empty header.
    lines.extend(graph_lines(&spec.graph, theme));

    // Comments / advisor disposition section (TASK-932): surfaced below the
    // body so the human can READ "approved because X" inside the TUI. Always
    // present (even with no comments) so the disposition affordance is
    // discoverable.
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "── Comments / Disposition ──",
        Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
    )));
    lines.extend(comment_lines(&spec.comments, theme));

    lines
}

/// Render a spec's comments as styled modal lines: one `author · time` header
/// per comment followed by its markdown body, blank-line separated; an empty
/// list renders a single dim "No comments." line. A PURE function (no IO) so
/// the comment→lines mapping is render-smoke testable: N comments produce N
// headers, empty produces the empty-state message. trace:TASK-932 | ai:claude
fn comment_lines<'a>(comments: &'a [LoadedComment], theme: &Theme) -> Vec<Line<'a>> {
    if comments.is_empty() {
        return vec![Line::from(Span::styled(
            "No comments.",
            Style::default().fg(theme.dim),
        ))];
    }
    let mut lines: Vec<Line> = Vec::new();
    for (i, c) in comments.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        // Header: author (accent) · short time (dim).
        let mut header: Vec<Span> = vec![Span::styled(
            c.author.clone(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )];
        if !c.when.is_empty() {
            header.push(Span::styled(
                format!("  ·  {}", c.when),
                Style::default().fg(theme.dim),
            ));
        }
        lines.push(Line::from(header));
        // Body, rendered as markdown (so disposition prose / lists / code read
        // nicely); an empty body degrades to nothing.
        if c.content.trim().is_empty() {
            lines.push(Line::from(Span::styled(
                "(empty)",
                Style::default().fg(theme.dim),
            )));
        } else {
            lines.extend(markdown_to_lines(&c.content, theme));
        }
    }
    lines
}

/// Render a spec's relationship graph as styled modal lines: a `── Graph ──`
/// header followed by one labelled group per non-empty relation (Parent epic,
/// Children, Blocked by, Blocks, References), each row a status glyph + id +
/// title + `[status]`. Surfacing the typed graph here is AIDA's #1 differentiator
/// shown at the dig-in gesture — the same edges `aida show` / `aida graph` print.
/// Returns NO lines when the graph is empty, so an unconnected spec shows no
/// header (the caller appends nothing). A PURE function (no IO) so the
/// graph→lines mapping is render-smoke testable.
// trace:STORY-739 | ai:claude
fn graph_lines<'a>(graph: &'a SpecGraph, theme: &Theme) -> Vec<Line<'a>> {
    if graph.is_empty() {
        return Vec::new();
    }
    let glyph_mode = list_row::GlyphMode::from_env();
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "── Graph ──",
        Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
    )));
    // The groups, in a deliberate reading order: where the spec sits in the
    // hierarchy (parent → children) then its dependency edges (blockers → blocks)
    // then loose references.
    let groups: [(&str, &[LoadedRelation]); 5] = [
        ("Parent epic", &graph.parents),
        ("Children", &graph.children),
        ("Blocked by", &graph.blocked_by),
        ("Blocks", &graph.blocks),
        ("References", &graph.references),
    ];
    for (label, rels) in groups {
        if rels.is_empty() {
            continue;
        }
        lines.push(Line::from(Span::styled(
            format!("{label}:"),
            Style::default().fg(theme.dim),
        )));
        for rel in rels {
            lines.push(relation_row(rel, glyph_mode, theme));
        }
    }
    lines
}

/// One graph row: `  <status-glyph> <id> <title> [<status>]`. The glyph + the
/// `[status]` tag carry the status colour (the cockpit's `list_row` palette);
/// the id is accent, the title plain. A PURE helper so the row idiom is shared
/// across groups.
// trace:STORY-739 | ai:claude
fn relation_row<'a>(rel: &'a LoadedRelation, mode: list_row::GlyphMode, theme: &Theme) -> Line<'a> {
    let status_style = list_row::status_style(&rel.status, theme);
    let mut spans: Vec<Span> = vec![
        Span::styled(
            format!("  {} ", list_row::status_glyph(&rel.status, mode)),
            status_style,
        ),
        Span::styled(
            rel.id.clone(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if !rel.title.is_empty() {
        spans.push(Span::styled(
            format!(" {}", rel.title),
            Style::default().fg(theme.fg),
        ));
    }
    if !rel.status.is_empty() {
        spans.push(Span::styled(format!("  [{}]", rel.status), status_style));
    }
    Line::from(spans)
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
// trace:TASK-913 | ai:claude
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
// to trim trailing blank lines from rendered markdown. trace:TASK-913
fn line_is_blank(line: &Line) -> bool {
    line.spans.iter().all(|s| s.content.is_empty())
}

/// Render a verb-output modal (the captured stdout of `show` / `why`).
// trace:STORY-690 | ai:claude
fn render_verb_modal(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    title: &str,
    body: &str,
    scroll: u16,
) {
    let popup = centered(area, 80, 80);
    f.render_widget(Clear, popup);
    // Same wrapped-row clamp + scrollbar as render_modal so long `why` /
    // captured-output bodies scroll instead of truncating. trace:BUG-635
    let lines: Vec<Line> = body.lines().map(|l| Line::from(l.to_string())).collect();
    let inner_w = popup.width.saturating_sub(2);
    let inner_h = popup.height.saturating_sub(2);
    let max_scroll = modal_max_scroll(&lines, inner_w, inner_h);
    let scroll = scroll.min(max_scroll);
    let scrollable = max_scroll > 0;
    let hint = if scrollable {
        format!(" {title} (↑↓/PgUp/PgDn scroll · Esc/q to close) ")
    } else {
        format!(" {title} (Esc/q to close) ")
    };
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.accent))
        .title(hint);
    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
        .style(Style::default().fg(theme.fg));
    f.render_widget(para, popup);
    render_modal_scrollbar(f, popup, theme, max_scroll, scroll);
}

fn render_confirm(f: &mut Frame, area: Rect, theme: &Theme, verb: Verb, count: usize) {
    let popup = centered(area, 50, 20);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.warn))
        .title(" confirm ");
    // `approve all` confirms over the derived approvable set, not an empty
    // selection — its message names the batch action + the set, where the
    // generic line would read "approve all all N". trace:TASK-937 | ai:claude
    let headline = if verb == Verb::ApproveAll {
        format!("Approve + queue all {count} approvable spec(s)?")
    } else {
        format!("Nothing selected. {} all {count} item(s)?", verb.label())
    };
    let lines = vec![
        Line::from(Span::styled(
            headline,
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

/// Render the drive-gate HOLD popup (STORY-744): the held spec id in the title,
/// a "was NOT launched" headline, the wrapped gate reason, and the remedy
/// affordances the hold offers (clarify / force / dismiss). Warn-bordered like
/// the confirm popup — a drive-gate hold is a refusal, not a success.
// trace:STORY-744 | ai:claude
fn render_gate_hold(f: &mut Frame, area: Rect, theme: &Theme, hold: &GateHold) {
    let popup = centered(area, 64, 45);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.warn))
        .title(format!(" drive held — {} ", hold.id));
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "The suitability gate held this spec — it was NOT launched.",
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            hold.reason.clone(),
            Style::default().fg(theme.fg),
        )),
        Line::from(""),
    ];
    for aff in hold.affordances() {
        lines.push(Line::from(vec![
            Span::styled("  • ", Style::default().fg(theme.dim)),
            Span::styled(aff, Style::default().fg(theme.fg)),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        popup,
    );
}

/// Render the drive-ROUTING popup (TASK-1076): the spec id in the title, the
/// resolved ADR-6 route (into-scope vs solo — reflecting the current toggle),
/// and the toggle / launch / cancel affordances. Accent-bordered like the other
// input popups — this is a decision, not a refusal. trace:TASK-1076 | ai:claude
fn render_drive_routing(f: &mut Frame, area: Rect, theme: &Theme, routing: &state::DriveRouting) {
    let popup = centered(area, 64, 40);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.accent))
        .title(format!(" drive routing — {} ", routing.id));
    // Highlight the ACTIVE route line in accent; the toggle re-styles it live.
    let route_style = if routing.solo {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    };
    let lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "This drive would route:",
            Style::default().fg(theme.dim),
        )),
        Line::from(""),
        Line::from(Span::styled(routing.routing_line(), route_style)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  • ", Style::default().fg(theme.dim)),
            Span::styled(
                "s: toggle --solo (split out into own worktree + PR)",
                Style::default().fg(theme.fg),
            ),
        ]),
        Line::from(vec![
            Span::styled("  • ", Style::default().fg(theme.dim)),
            Span::styled(
                "Enter / d: launch with the route above",
                Style::default().fg(theme.fg),
            ),
        ]),
        Line::from(vec![
            Span::styled("  • ", Style::default().fg(theme.dim)),
            Span::styled("Esc / q: cancel", Style::default().fg(theme.fg)),
        ]),
    ];
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        popup,
    );
}

/// Render the single-line revisit-trigger input modal for the `defer` verb:
/// a prompt, the typed buffer with a block cursor, the target count, and the
// confirm/cancel keys. trace:TASK-921 | ai:claude
fn render_defer_input(f: &mut Frame, area: Rect, theme: &Theme, di: &state::DeferInput) {
    let popup = centered(area, 60, 25);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.accent))
        .title(format!(
            " defer {} spec(s) — revisit trigger ",
            di.targets.len()
        ));
    let lines = vec![
        Line::from(Span::styled(
            "Revisit when…  (the --until trigger)",
            Style::default().fg(theme.dim),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(
                di.buffer.clone(),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            // A block cursor at the end of the input.
            Span::styled("█", Style::default().fg(theme.accent)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Enter = defer   ·   Esc = cancel",
            Style::default().fg(theme.dim),
        )),
    ];
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Render the single-line reply-body input modal for the `reply` verb
/// (Mail scope): a prompt naming the recipient, the typed buffer with a block
/// cursor, and the confirm/cancel keys. Mirrors [`render_defer_input`]'s shape.
// trace:STORY-701 | ai:claude
fn render_reply_input(f: &mut Frame, area: Rect, theme: &Theme, ri: &state::ReplyInput) {
    let popup = centered(area, 60, 25);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.accent))
        .title(format!(" reply to {} ", ri.to));
    let lines = vec![
        Line::from(Span::styled("Message body", Style::default().fg(theme.dim))),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(
                ri.buffer.clone(),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            // A block cursor at the end of the input.
            Span::styled("█", Style::default().fg(theme.accent)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Enter = send   ·   Esc = cancel",
            Style::default().fg(theme.dim),
        )),
    ];
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Render the new-spec TITLE input modal (TASK-931): a single-line prompt with
/// a block cursor for the title of a fresh Draft spec. Mirrors the defer-input
/// modal's shape. Enter creates; Esc (or an empty title) cancels.
// trace:TASK-931 | ai:claude
fn render_new_input(f: &mut Frame, area: Rect, theme: &Theme, ni: &state::NewSpecInput) {
    let popup = centered(area, 60, 25);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.accent))
        .title(" new — create a Draft spec ");
    let lines = vec![
        Line::from(Span::styled(
            "Title for the new Draft spec",
            Style::default().fg(theme.dim),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(
                ni.buffer.clone(),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            // A block cursor at the end of the input.
            Span::styled("█", Style::default().fg(theme.accent)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Enter = create   ·   Esc = cancel",
            Style::default().fg(theme.dim),
        )),
    ];
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Render the EPIC focus PICKER modal (STORY-697): a fuzzy-filter prompt with a
/// block cursor, then the (filtered) open-epic list — one row per epic showing
/// its id + title + status, with the highlighted row reverse-styled. Mirrors
/// the selectable-list + fuzzy-filter patterns of the main panels.
// trace:STORY-697 | ai:claude
fn render_epic_picker(f: &mut Frame, area: Rect, theme: &Theme, p: &state::EpicPicker) {
    let popup = centered(area, 70, 70);
    f.render_widget(Clear, popup);
    let idxs = p.filtered_indices();
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.accent))
        .title(format!(
            " focus on an EPIC — {}/{} (type to filter · ↑↓ · ↵ focus · Esc) ",
            idxs.len(),
            p.epics.len()
        ));

    let mut lines: Vec<Line> = Vec::new();
    // The fuzzy-filter input line with a block cursor.
    lines.push(Line::from(vec![
        Span::styled("filter> ", Style::default().fg(theme.accent)),
        Span::styled(
            p.filter.clone(),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("█", Style::default().fg(theme.accent)),
    ]));
    lines.push(Line::from(""));

    if idxs.is_empty() {
        lines.push(Line::from(Span::styled(
            "No open epic matches the filter. Backspace to widen it.",
            Style::default().fg(theme.dim),
        )));
    } else {
        // Window the list so the highlighted row stays visible in a tall list.
        let inner_h = popup.height.saturating_sub(4) as usize; // borders + filter + blank
        let visible = inner_h.max(1);
        let start = p.selected.saturating_sub(visible.saturating_sub(1));
        for (row, &real) in idxs.iter().enumerate().skip(start).take(visible) {
            let epic = &p.epics[real];
            let active = row == p.selected;
            let style = row_style(theme, active, false);
            let marker = if active { "▸ " } else { "  " };
            lines.push(Line::from(vec![
                Span::styled(marker, style),
                Span::styled(format!("{}  ", epic.id), style.add_modifier(Modifier::BOLD)),
                Span::styled(epic.title.clone(), style),
                Span::styled(
                    format!("  ({})", epic.status),
                    if active {
                        style
                    } else {
                        Style::default().fg(theme.dim)
                    },
                ),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines).block(block), popup);
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
mod refresh_tests {
    //! Pure tests for the `r` live-refresh (TASK-934): the parts that don't
    //! need a terminal or an attached store. The store re-read itself rides
    //! `CachedGitBackend`'s per-call stale-check (covered by aida-core), so
    //! here we prove refresh drops the in-memory scope cache + resets the
    //! sentinel (forcing the next `sync_scope_items` to re-fetch) and reports
    //! status without a store. trace:TASK-934 | ai:claude
    use super::*;

    fn item(id: &str) -> TargetItem {
        TargetItem {
            id: id.to_string(),
            title: String::new(),
            req_type: "Task".into(),
            status: "Approved".into(),
            priority: String::new(),
            body: String::new(),
            has_test_plan: false,
            routed_role: None,
            tags: Vec::new(),
        }
    }

    #[test]
    fn startup_status_has_no_prototype_self_label() {
        // STORY-724: the default cockpit's opening line must read as finished —
        // no "prototype" / "Slice N" self-deprecation in either branch.
        for available in [true, false] {
            let line = startup_status(available);
            let lower = line.to_lowercase();
            assert!(
                !lower.contains("prototype"),
                "status line still says prototype: {line}"
            );
            assert!(
                !lower.contains("slice 1") && !lower.contains("slice 2"),
                "status line still references a slice: {line}"
            );
        }
        // The store-available line names the wired scopes.
        let ok = startup_status(true);
        assert!(ok.contains("Backlog"));
        assert!(ok.contains("Queue"));
        assert!(ok.contains("Mail"));
    }

    #[test]
    fn refresh_clears_the_scope_cache_and_resets_the_sentinel() {
        let mut st = RedesignState::new(vec![item("TASK-1")], "advisor");
        let mut cache: HashMap<Scope, Vec<TargetItem>> = HashMap::new();
        cache.insert(Scope::Backlog, vec![item("TASK-1")]);
        cache.insert(Scope::Open, vec![item("TASK-2")]);
        let mut loaded = Scope::Backlog;
        let mut focus_set: Option<std::collections::HashSet<String>> = None;

        refresh(&mut st, None, &mut cache, &mut loaded, &mut focus_set);

        // The in-memory scope rows are dropped so the next sync re-fetches…
        assert!(cache.is_empty(), "scope cache emptied");
        // …and the sentinel is reset to the non-functional Sessions scope, which
        // can never equal the active functional scope, forcing a re-fetch.
        assert_eq!(loaded, Scope::Sessions);
    }

    #[test]
    fn refresh_without_store_reports_unavailable() {
        let mut st = RedesignState::new(vec![], "advisor");
        let mut cache: HashMap<Scope, Vec<TargetItem>> = HashMap::new();
        let mut loaded = Scope::Backlog;
        let mut focus_set: Option<std::collections::HashSet<String>> = None;

        refresh(&mut st, None, &mut cache, &mut loaded, &mut focus_set);

        assert_eq!(st.status.as_deref(), Some("refresh: store unavailable"));
        // With no store the focus closure recompute is skipped, leaving the lens
        // untouched. trace:TASK-934
        assert!(focus_set.is_none());
    }

    #[test]
    fn invalidate_scope_cache_empties_and_resets() {
        let mut cache: HashMap<Scope, Vec<TargetItem>> = HashMap::new();
        cache.insert(Scope::Open, vec![item("TASK-9")]);
        let mut loaded = Scope::Open;
        invalidate_scope_cache(&mut cache, &mut loaded);
        assert!(cache.is_empty());
        assert_eq!(loaded, Scope::Sessions);
    }

    // --- Async verb execution (BUG-633) -----------------------------------

    #[test]
    fn start_pending_installs_a_pending_op() {
        let mut st = RedesignState::new(vec![item("TASK-1")], "advisor");
        let mut pending: Option<Pending> = None;
        let started = start_pending(&mut pending, &mut st, "approving 1 spec(s)…", || {
            VerbResult {
                status: "approved 1: TASK-1".to_string(),
                invalidate: true,
            }
        });
        assert!(started, "a verb starts when none is pending");
        let p = pending.as_ref().expect("pending installed");
        assert_eq!(p.op.label, "approving 1 spec(s)…");
        assert_eq!(p.op.frame, 0);
    }

    #[test]
    fn start_pending_rejects_a_second_verb_while_one_is_in_flight() {
        let mut st = RedesignState::new(vec![item("TASK-1")], "advisor");
        let mut pending: Option<Pending> = None;
        // First verb starts and installs the pending op.
        start_pending(&mut pending, &mut st, "approving 1 spec(s)…", || {
            VerbResult {
                status: "approved 1: TASK-1".to_string(),
                invalidate: true,
            }
        });
        // A SECOND verb is refused while the first is in flight: returns false,
        // sets a busy status, and leaves the existing op untouched.
        let started = start_pending(&mut pending, &mut st, "deferring 1 spec(s)…", || {
            VerbResult {
                status: "should not run".to_string(),
                invalidate: false,
            }
        });
        assert!(!started, "a second verb is rejected while one is pending");
        let busy = st.status.as_deref().unwrap_or_default();
        assert!(busy.contains("busy"), "busy status, got: {busy}");
        assert!(
            busy.contains("approving 1 spec(s)…"),
            "names the in-flight op"
        );
        // The original op is unchanged.
        assert_eq!(pending.as_ref().unwrap().op.label, "approving 1 spec(s)…");
    }

    #[test]
    fn apply_verb_result_sets_status_and_returns_invalidate_flag() {
        let mut st = RedesignState::new(vec![item("TASK-1")], "advisor");
        let invalidate = apply_verb_result(
            &mut st,
            &VerbResult {
                status: "approved 1: TASK-1".to_string(),
                invalidate: true,
            },
        );
        assert!(invalidate, "a store write asks for cache invalidation");
        assert_eq!(st.status.as_deref(), Some("approved 1: TASK-1"));

        let invalidate = apply_verb_result(
            &mut st,
            &VerbResult {
                status: "new: aida add failed: nope".to_string(),
                invalidate: false,
            },
        );
        assert!(!invalidate, "a failed write does not invalidate");
        assert_eq!(st.status.as_deref(), Some("new: aida add failed: nope"));
    }

    #[test]
    fn pending_op_completes_over_the_channel() {
        // The integration shim end-to-end: a worker thread runs the (here
        // trivial) work and the result arrives over the channel — proving the
        // completion plumbing without shelling out to `aida`. trace:BUG-633
        let mut st = RedesignState::new(vec![item("TASK-1")], "advisor");
        let mut pending: Option<Pending> = None;
        start_pending(&mut pending, &mut st, "approving 1 spec(s)…", || {
            VerbResult {
                status: "approved 1: TASK-1".to_string(),
                invalidate: true,
            }
        });
        let p = pending.take().expect("pending installed");
        // Block for the worker's result (the loop's try_recv is the non-blocking
        // variant; here we just prove the value crosses the channel).
        let result = p.rx.recv().expect("worker sends a result");
        assert_eq!(result.status, "approved 1: TASK-1");
        assert!(result.invalidate);
    }
}

#[cfg(test)]
mod carousel_tests {
    //! Pure tests for the modal carousel index math (STORY-710 part A): next/
    //! prev clamp at the ends, and the set is the SELECTED subset when anything
    //! is selected, else the whole bottom-panel list. trace:STORY-710 | ai:claude
    use super::carousel_target;

    #[test]
    fn next_and_prev_move_through_all_when_nothing_selected() {
        let all = [0usize, 1, 2, 3];
        let selected: [usize; 0] = [];
        // Forward steps one item at a time.
        assert_eq!(carousel_target(0, &all, &selected, 1), Some(1));
        assert_eq!(carousel_target(1, &all, &selected, 1), Some(2));
        // Backward likewise.
        assert_eq!(carousel_target(2, &all, &selected, -1), Some(1));
        assert_eq!(carousel_target(1, &all, &selected, -1), Some(0));
    }

    #[test]
    fn clamps_at_both_ends_no_wrap() {
        let all = [0usize, 1, 2];
        let selected: [usize; 0] = [];
        // Next at the last item stays put (clamp, not wrap).
        assert_eq!(carousel_target(2, &all, &selected, 1), Some(2));
        // Prev at the first item stays put.
        assert_eq!(carousel_target(0, &all, &selected, -1), Some(0));
    }

    #[test]
    fn moves_over_the_selection_when_items_are_selected() {
        // The bottom list is 0..5 but only 1, 3, 4 are selected; the carousel
        // walks the SELECTED subset (skipping 2) and clamps at its ends.
        let all = [0usize, 1, 2, 3, 4];
        let selected = [1usize, 3, 4];
        assert_eq!(carousel_target(1, &all, &selected, 1), Some(3));
        assert_eq!(carousel_target(3, &all, &selected, 1), Some(4));
        assert_eq!(carousel_target(4, &all, &selected, 1), Some(4)); // clamp
        assert_eq!(carousel_target(3, &all, &selected, -1), Some(1));
        assert_eq!(carousel_target(1, &all, &selected, -1), Some(1)); // clamp
    }

    #[test]
    fn current_outside_the_set_returns_none() {
        let all = [0usize, 1, 2];
        let selected: [usize; 0] = [];
        // A current index not in the set (e.g. the externally-opened sentinel
        // would never be passed here, but a stale index returns None safely).
        assert_eq!(carousel_target(9, &all, &selected, 1), None);
        // Selected set that does not contain `current`.
        assert_eq!(carousel_target(0, &all, &[1usize, 2], 1), None);
    }

    #[test]
    fn empty_set_returns_none() {
        let all: [usize; 0] = [];
        let selected: [usize; 0] = [];
        assert_eq!(carousel_target(0, &all, &selected, 1), None);
    }
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
                has_test_plan: false,
                routed_role: None,
                tags: Vec::new(),
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
        terminal
            .draw(|f| render(f, st, spec, None))
            .expect("render");
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
            comments: vec![],
            graph: SpecGraph::default(),
        }
    }

    #[test]
    fn renders_scope_level() {
        draw(&sample(5), 100, 30);
    }

    #[test]
    fn renders_pending_spinner_in_the_status_line() {
        // The status line shows the spinner + label while a verb is pending
        // (BUG-633); the render must not panic and must paint the label.
        let st = sample(5);
        let op = PendingOp::new("approving 1 spec(s)…");
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("test backend");
        terminal
            .draw(|f| render(f, &st, None, Some(&op)))
            .expect("render");
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for cell in buf.content() {
            out.push_str(cell.symbol());
        }
        assert!(out.contains("approving"), "spinner label is painted");
        assert!(
            out.contains(state::SPINNER_FRAMES[0]),
            "spinner glyph is painted"
        );
    }

    /// Flatten the painted backend into one string so a render test can assert
    // a glyph sequence is (or is not) present. trace:TASK-945 | ai:claude
    fn rendered_text(st: &RedesignState, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
        terminal
            .draw(|f| render(f, st, None, None))
            .expect("render");
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for cell in buf.content() {
            out.push_str(cell.symbol());
        }
        out
    }

    /// The `/query` find prompt renders ONLY in find mode (TASK-945): a
    /// confirmed-but-applied filter narrows silently (no prompt); entering find
    // mode shows the live `/…` prompt. trace:TASK-945 | ai:claude
    #[test]
    fn find_prompt_renders_only_in_find_mode() {
        // Normal mode with a confirmed filter applied → NO `/zz` prompt.
        let mut st = sample(6);
        st.focus_bottom();
        st.enter_find_mode();
        st.push_filter('z');
        st.push_filter('z');
        st.confirm_find();
        assert!(!st.find_mode);
        let normal = rendered_text(&st, 120, 30);
        assert!(
            !normal.contains("/zz"),
            "the find prompt must NOT show in normal mode"
        );

        // Re-enter find mode and type → the `/ab` prompt IS painted.
        let mut st2 = sample(6);
        st2.focus_bottom();
        st2.enter_find_mode();
        st2.push_filter('a');
        st2.push_filter('b');
        let finding = rendered_text(&st2, 120, 30);
        assert!(
            finding.contains("/ab"),
            "the find prompt must show in find mode"
        );
    }

    /// The columnar scope-list render (TASK-914): a multi-row Backlog list with
    /// mixed statuses/priorities + a selection + the cursor paints over the
    /// TestBackend without panicking, at a realistic and a tiny size. The pure
    /// column-layout + status→glyph/colour mapping is unit-tested in
    // `super::list_row`. trace:TASK-914 | ai:claude
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

    /// The Test scope (STORY-699): a row carrying a `## Test Plan` gets the
    /// trailing marker, and the render paints over the backend without
    // panicking. trace:STORY-699
    #[test]
    fn renders_test_scope_row_marker() {
        let mut st = sample(4);
        st.items[1].has_test_plan = true;
        st.items[3].has_test_plan = true;
        st.focus_bottom();
        draw(&st, 120, 20);
        draw(&st, 24, 8); // tiny terminal, no panic
    }

    /// `test_plan_view` (STORY-699): swaps the body to the extracted `## Test
    /// Plan` ONLY in the Test scope; falls back to the full description when the
    // section is absent; leaves other scopes' specs untouched. trace:STORY-699
    #[test]
    fn test_plan_view_extracts_only_in_test_scope() {
        let mut st = sample(3);
        let spec = LoadedSpec {
            id: "STORY-690".into(),
            title: "t".into(),
            req_type: "Story".into(),
            status: "Done".into(),
            priority: "High".into(),
            tags: vec![],
            description: "Intro paragraph.\n\n## Test Plan\n1. do X → expect Y".into(),
            comments: vec![],
            graph: SpecGraph::default(),
        };
        // Not in the Test scope → untouched (full description).
        let untouched = test_plan_view(&st, spec.clone());
        assert!(untouched.description.contains("Intro paragraph."));
        // In the Test scope → only the ## Test Plan section is rendered.
        st.scope = Some(Scope::Test);
        let view = test_plan_view(&st, spec.clone());
        assert!(view.description.starts_with("## Test Plan"));
        assert!(!view.description.contains("Intro paragraph."));
        // No test plan → falls back to the full description.
        let plain = LoadedSpec {
            description: "just a description".into(),
            ..spec
        };
        assert_eq!(test_plan_view(&st, plain).description, "just a description");
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
    fn modal_title_advertises_carousel_when_multiple_items() {
        // With >1 item in the bottom list and no selection, the modal title
        // advertises the ←/→ carousel hint. trace:STORY-710
        let mut st = sample(4);
        st.drill();
        st.focus_bottom();
        st.open_modal();
        let spec = sample_spec();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("test backend");
        terminal
            .draw(|f| render(f, &st, Some(&spec), None))
            .expect("render");
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for cell in buf.content() {
            out.push_str(cell.symbol());
        }
        assert!(
            out.contains("prev/next"),
            "the carousel hint is painted in the modal title, got: {out}"
        );
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
    fn renders_open_scope_verbs_with_status_greying() {
        // Post-TASK-947: the verb list is the FULL Open vocabulary regardless of
        // focused status — status-inapplicable verbs render GREYED, not hidden.
        // On a Draft focus the draft verbs apply and `queue`/`accept` grey.
        // trace:TASK-920 trace:TASK-949 trace:TASK-947
        let mut st = sample(5); // index 0 is Draft
        drill_open(&mut st);
        st.focus_bottom(); // focus TASK-0 (Draft)
        st.focus_top();
        draw(&st, 100, 30); // renders without panic, greyed rows included
        assert_eq!(
            st.current_verbs(),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::Status,
                Verb::RequestApproval,
                Verb::Approve,
                Verb::Reject,
                Verb::Queue,
                Verb::Accept,
                Verb::Defer,
                Verb::Drive,
                // trace:TASK-937
                Verb::ApproveAll,
            ]
        );
        // Draft focus: draft verbs apply; approved/done verbs grey.
        assert!(st.verb_status_permitted(Verb::Approve));
        assert!(!st.verb_status_permitted(Verb::Queue));
        assert!(!st.verb_status_permitted(Verb::Accept));
        // `drive` is Approved-gated too, so it greys on a Draft focus.
        assert!(!st.verb_status_permitted(Verb::Drive));
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

    #[test]
    fn batch_approve_status_lists_approved_and_failed() {
        // trace:TASK-937
        let s = batch_approve_status(&BatchApproveOutcome {
            approved: vec!["TASK-1".to_string(), "TASK-2".to_string()],
            failed: vec!["TASK-3".to_string()],
        });
        assert!(s.contains("approved + queued 2"));
        assert!(s.contains("TASK-1"));
        assert!(s.contains("TASK-2"));
        assert!(s.contains("FAILED"));
        assert!(s.contains("TASK-3"));
        // Empty case.
        let empty = batch_approve_status(&BatchApproveOutcome::default());
        assert!(empty.contains("nothing approvable"));
    }

    #[test]
    fn reject_status_lists_rejected_skipped_failed() {
        // trace:TASK-949
        let s = reject_status(
            &["TASK-1".to_string(), "TASK-2".to_string()],
            &["TASK-3".to_string()],
            &["TASK-4".to_string()],
        );
        assert!(s.contains("rejected 2"));
        assert!(s.contains("TASK-1"));
        assert!(s.contains("FAILED"));
        assert!(s.contains("skipped 1"));
        // Empty case.
        let empty = reject_status(&[], &[], &[]);
        assert!(empty.contains("nothing to reject"));
    }

    #[test]
    fn accept_status_lists_accepted_skipped_failed() {
        // trace:TASK-933
        let s = accept_status(
            &["TASK-1".to_string(), "TASK-2".to_string()],
            &["TASK-3".to_string()],
            &["TASK-4".to_string()],
        );
        assert!(s.contains("accepted 2"));
        assert!(s.contains("Completed"));
        assert!(s.contains("TASK-1"));
        assert!(s.contains("FAILED"));
        assert!(s.contains("skipped 1"));
        // Empty case.
        let empty = accept_status(&[], &[], &[]);
        assert!(empty.contains("nothing to accept"));
    }

    #[test]
    fn accept_edit_args_builds_completed_transition() {
        // accept runs the real Done → Completed transition (NOT a fake): the
        // arg vector is `edit <id> --status completed`, the Done-status mirror
        // of approve's `--status approved`. trace:TASK-933
        let args = accept_edit_args("TASK-7");
        assert_eq!(args, vec!["edit", "TASK-7", "--status", "completed"]);
    }

    #[test]
    fn accept_comment_args_is_single_safe_note_element() {
        // The reviewer-acceptance note is exactly one arg-vector element, so it
        // is never shell-parsed. trace:TASK-933
        let args = accept_comment_args("TASK-7");
        assert_eq!(args[0], "comment");
        assert_eq!(args[1], "add");
        assert_eq!(args[2], "TASK-7");
        assert!(args[3].contains("accepted by reviewer"));
        // No shell metacharacters that would be dangerous if (mistakenly) shelled.
        assert!(!args[3].contains('`'));
        assert!(!args[3].contains('$'));
    }

    #[test]
    fn groom_args_is_propose_only() {
        // STORY-703: the cockpit `groom` gesture runs `aida groom` with NO
        // `--apply` — the SAFE propose pass that reads + prints the plan but
        // never writes. Assert the verb shape (the spawn argv) so a future edit
        // can't silently add `--apply` and turn the read into a mutation.
        let args = groom_args();
        assert_eq!(args, ["groom"]);
        assert!(!args.contains(&"--apply"), "groom stays propose-only");
    }

    #[test]
    fn archive_args_builds_safe_arg_vector() {
        // STORY-703: `archive` shells out to `aida archive <id>`; the id is a
        // single arg-vector element (never shell-parsed). Assert the argv.
        let args = archive_args("BUG-42");
        assert_eq!(args, ["archive", "BUG-42"]);
    }

    #[test]
    fn archive_status_lists_archived_and_failed() {
        // STORY-703
        let s = archive_status(
            &["TASK-1".to_string(), "TASK-2".to_string()],
            &["TASK-3".to_string()],
        );
        assert!(s.contains("archived 2"));
        assert!(s.contains("TASK-1"));
        assert!(s.contains("FAILED to archive"));
        assert!(s.contains("TASK-3"));
        // All-success case omits the FAILED clause.
        let ok = archive_status(&["TASK-9".to_string()], &[]);
        assert!(ok.contains("archived 1"));
        assert!(!ok.contains("FAILED"));
        // Empty case.
        let empty = archive_status(&[], &[]);
        assert!(empty.contains("nothing to archive"));
    }

    #[test]
    fn defer_status_lists_deferred_failed_with_trigger() {
        // trace:TASK-921
        let s = defer_status(
            &["TASK-1".to_string(), "TASK-2".to_string()],
            &["TASK-3".to_string()],
            "when the shelf grows",
        );
        assert!(s.contains("deferred 2"));
        assert!(s.contains("when the shelf grows"));
        assert!(s.contains("TASK-1"));
        assert!(s.contains("FAILED"));
        assert!(s.contains("TASK-3"));
        // Empty case.
        let empty = defer_status(&[], &[], "x");
        assert!(empty.contains("nothing parked"));
    }

    // --- New-spec create (TASK-931) ---------------------------------------

    #[test]
    fn new_spec_args_builds_safe_arg_vector() {
        // The title is a SINGLE arg-vector element — even with backticks, quotes
        // and `$`, which would be shell-dangerous in a shell string but are
        // inert as an OS argument. trace:TASK-931
        let title = "fix the `rm -rf $HOME` \"quote\" bug";
        let args = new_spec_args(title, None);
        assert_eq!(
            args,
            vec!["add", "--title", title, "--type", "task", "--status", "draft"],
        );
        // The whole title is exactly one element (not split on spaces/specials).
        assert_eq!(args.iter().filter(|a| **a == title).count(), 1);
    }

    #[test]
    fn new_spec_args_appends_parent_when_focused() {
        // With an active focus epic, the new draft is filed under it via
        // `--parent <epic>` so the focus lens keeps it visible. trace:TASK-942
        let args = new_spec_args("a new task", Some("EPIC-54"));
        assert_eq!(
            args,
            vec![
                "add",
                "--title",
                "a new task",
                "--type",
                "task",
                "--status",
                "draft",
                "--parent",
                "EPIC-54",
            ],
        );
        // No focus → no --parent flag at all (unparented, unchanged behavior).
        assert!(!new_spec_args("a new task", None)
            .iter()
            .any(|a| *a == "--parent"));
    }

    #[test]
    fn parse_created_spec_id_extracts_from_added_line() {
        // The CLI prints `Added: <SPEC-ID> - <title>` on success.
        assert_eq!(
            parse_created_spec_id("Added: TASK-932 - my new title\n"),
            Some("TASK-932".to_string()),
        );
        // No Added line → None (e.g. unexpected output).
        assert_eq!(parse_created_spec_id("something else entirely"), None);
        // The `?` placeholder (no spec_id assigned) is not a real id.
        assert_eq!(parse_created_spec_id("Added: ? - title"), None);
    }

    #[test]
    fn create_status_reports_id_and_title() {
        let with_id = create_status(Some("TASK-9"), "do the thing", None);
        assert!(with_id.contains("TASK-9"));
        assert!(with_id.contains("do the thing"));
        // Without an id we still confirm a Draft was created.
        let no_id = create_status(None, "do the thing", None);
        assert!(no_id.contains("Draft"));
        // Long titles are truncated with an ellipsis.
        let long = "x".repeat(80);
        assert!(create_status(Some("TASK-1"), &long, None).contains('…'));
    }

    #[test]
    fn create_status_names_focus_parent_when_filed_under_epic() {
        // When filed under an active focus epic, the confirmation names the
        // parent so the operator sees the link (not orphaned). trace:TASK-942
        let s = create_status(Some("TASK-9"), "do the thing", Some("EPIC-54"));
        assert!(s.contains("TASK-9"));
        assert!(s.contains("EPIC-54"));
        // No focus → no parent mention.
        let none = create_status(Some("TASK-9"), "do the thing", None);
        assert!(!none.contains("under"));
    }

    #[test]
    fn renders_new_input_modal() {
        // The single-line title input modal paints over the backend (empty and
        // with typed text), at a realistic and a tiny size, no panic. trace:TASK-931
        let mut st = sample(5);
        drill_open(&mut st);
        st.open_new_input();
        draw(&st, 100, 30);
        // With typed title text.
        st.push_new_char('h');
        st.push_new_char('i');
        draw(&st, 100, 30);
        // Tiny terminal.
        draw(&st, 20, 6);
    }

    #[test]
    fn renders_defer_input_modal() {
        // The single-line revisit-trigger input modal paints over the backend
        // (with and without typed text), at a realistic and a tiny size, no
        // panic. trace:TASK-921
        let mut st = sample(5);
        drill_open(&mut st);
        st.open_defer_input(vec!["TASK-0".to_string(), "TASK-2".to_string()]);
        draw(&st, 100, 30);
        // With typed trigger text.
        st.push_defer_char('w');
        st.push_defer_char('e');
        st.push_defer_char('n');
        draw(&st, 100, 30);
        // Tiny terminal.
        draw(&st, 20, 6);
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
            comments: vec![LoadedComment {
                author: "advisor".into(),
                when: "2026-06-26 16:27".into(),
                content: "approved because the slice is well-bounded".into(),
            }],
            graph: SpecGraph::default(),
        };
        draw_with_spec(&st, Some(&spec), 100, 30);
        // Tiny terminal must not panic either.
        draw_with_spec(&st, Some(&spec), 20, 6);
    }

    #[test]
    fn renders_help_popup() {
        // The '?' help popup paints over the backend in each focus context, at
        // a realistic and a tiny size, no panic. trace:TASK-922
        // Scope context.
        let mut st = sample(5);
        st.open_help();
        draw(&st, 100, 30);
        draw(&st, 20, 6);
        // Verb context.
        let mut st = sample(5);
        st.drill();
        st.open_help();
        draw(&st, 100, 30);
        // Item context.
        let mut st = sample(5);
        st.drill();
        st.focus_bottom();
        st.open_help();
        draw(&st, 100, 30);
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
            comments: vec![],
            graph: SpecGraph::default(),
        };
        st.modal_scroll = 9999; // way past the end
        draw_with_spec(&st, Some(&spec), 100, 30);
    }

    /// The wrapped-height clamp (BUG-635): a body of long lines wraps into many
    /// visual rows, so the scroll range computed from the WRAPPED row count must
    /// be larger than the old buggy clamp computed from the LOGICAL line count —
    /// otherwise PageDown stops short and the body bottom is unreachable.
    // trace:BUG-635 | ai:claude
    #[test]
    fn modal_max_scroll_uses_wrapped_not_logical_count() {
        let theme = Theme::default();
        // Each body line is ~300 cols — far wider than the 40-col inner width —
        // so every logical line wraps into ~8 visual rows.
        let long = "word ".repeat(60);
        let body: String = (0..10).map(|i| format!("{long} {i}\n")).collect();
        let spec = LoadedSpec {
            id: "BUG-635".into(),
            title: "Long wrapping body".into(),
            req_type: "Bug".into(),
            status: "Draft".into(),
            priority: String::new(),
            tags: vec![],
            description: body,
            comments: vec![],
            graph: SpecGraph::default(),
        };
        let lines = spec_lines(&spec, &theme);
        let inner_w = 40u16;
        let inner_h = 20u16;
        // The old (buggy) clamp: logical line count minus the visible height.
        let logical_max = (lines.len() as u16).saturating_sub(inner_h.max(1));
        // The fixed clamp: wrapped row count minus the visible height.
        let wrapped_max = modal_max_scroll(&lines, inner_w, inner_h);
        assert!(
            wrapped_max > logical_max,
            "wrapped clamp {wrapped_max} must exceed logical clamp {logical_max}"
        );
        // The body genuinely overflows at this width/height → it is scrollable.
        assert!(wrapped_max > 0, "long wrapping body must be scrollable");
    }

    /// The verb-output modal now scrolls long captured output instead of
    /// truncating it — a long body scrolled past its end pins to the last page
    // and never panics. trace:BUG-635 | ai:claude
    #[test]
    fn verb_modal_scrolls_long_output() {
        let mut st = sample(5);
        let body: String = (0..200)
            .map(|i| format!("captured output line {i}\n"))
            .collect();
        st.open_verb_modal("TASK-0 — show", body);
        st.modal_scroll = 9999; // way past the end → clamps internally
        draw(&st, 100, 30);
        assert!(st.modal_open());
    }

    /// `comment_lines` (TASK-932): N comments → N author headers + bodies;
    // an empty list → the "No comments." empty-state line. trace:TASK-932
    #[test]
    fn comment_lines_maps_each_and_empty_state() {
        let theme = Theme::default();
        // Empty → the empty-state message.
        let empty = comment_lines(&[], &theme);
        assert_eq!(empty.len(), 1);
        assert_eq!(line_text(&empty[0]), "No comments.");

        // Two comments → both authors + both bodies appear.
        let comments = vec![
            LoadedComment {
                author: "advisor".into(),
                when: "2026-06-26 16:27".into(),
                content: "approved because the slice is bounded".into(),
            },
            LoadedComment {
                author: "claude".into(),
                when: "2026-06-26 17:00".into(),
                content: "implemented in-process".into(),
            },
        ];
        let lines = comment_lines(&comments, &theme);
        let blob: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(blob.contains("advisor"), "first author present");
        assert!(blob.contains("claude"), "second author present");
        assert!(blob.contains("approved because the slice is bounded"));
        assert!(blob.contains("implemented in-process"));
        // The short time stamp rides the header.
        assert!(blob.contains("2026-06-26 16:27"));
    }

    /// The disposition section is appended to the preview modal body (TASK-932):
    /// `spec_lines` includes the comment header below the description.
    #[test]
    fn spec_lines_appends_disposition_section() {
        let theme = Theme::default();
        let mut spec = sample_spec();
        spec.comments = vec![LoadedComment {
            author: "advisor".into(),
            when: "2026-06-26 16:27".into(),
            content: "approved because X".into(),
        }];
        let lines = spec_lines(&spec, &theme);
        let blob: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            blob.contains("Comments / Disposition"),
            "section header shown"
        );
        assert!(blob.contains("advisor"), "comment author shown");
        assert!(
            blob.contains("approved because X"),
            "disposition text shown"
        );
        // No comments → the empty-state still renders the section.
        let bare = sample_spec();
        let bare_blob: String = spec_lines(&bare, &theme)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(bare_blob.contains("No comments."));
    }

    /// STORY-739: `spec_lines` renders the relationship-graph section — the
    /// `── Graph ──` header, a labelled group per non-empty relation, and one
    /// `<id> <title> [<status>]` row per related spec — and OMITS empty groups
    /// (and the whole section when the spec has no relationships).
    #[test]
    fn spec_lines_renders_graph_section_grouped_and_omits_empty() {
        let theme = Theme::default();
        let mut spec = sample_spec();
        spec.graph = SpecGraph {
            parents: vec![LoadedRelation {
                id: "EPIC-7".into(),
                title: "the epic".into(),
                status: "InProgress".into(),
            }],
            children: vec![
                LoadedRelation {
                    id: "TASK-2".into(),
                    title: "child one".into(),
                    status: "Approved".into(),
                },
                LoadedRelation {
                    id: "TASK-3".into(),
                    title: "child two".into(),
                    status: "Done".into(),
                },
            ],
            blocked_by: vec![LoadedRelation {
                id: "BUG-9".into(),
                title: "a blocker".into(),
                status: "Draft".into(),
            }],
            blocks: vec![],
            references: vec![],
        };
        let blob: String = spec_lines(&spec, &theme)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(blob.contains("── Graph ──"), "graph header shown");
        assert!(blob.contains("Parent epic:"), "parent group label shown");
        assert!(blob.contains("EPIC-7"), "parent id shown");
        assert!(blob.contains("the epic"), "parent title shown");
        assert!(blob.contains("Children:"), "children group label shown");
        assert!(blob.contains("TASK-2"), "first child shown");
        assert!(blob.contains("TASK-3"), "second child shown");
        assert!(blob.contains("Blocked by:"), "blocked-by group label shown");
        assert!(blob.contains("BUG-9"), "blocker shown");
        assert!(blob.contains("[Approved]"), "status tag rendered on a row");
        // Empty groups produce no header.
        assert!(!blob.contains("Blocks:"), "empty blocks group omitted");
        assert!(
            !blob.contains("References:"),
            "empty references group omitted"
        );

        // A spec with NO relationships renders no graph section at all.
        let bare = sample_spec();
        assert!(bare.graph.is_empty());
        let bare_blob: String = spec_lines(&bare, &theme)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !bare_blob.contains("── Graph ──"),
            "no graph header for an unconnected spec"
        );
    }
}

#[cfg(test)]
mod gate_tests {
    //! The default-on / opt-out semantics of `AIDA_TUI_REDESIGN` (TASK-1051):
    //! EPIC-54 renders unless the env var is an explicit opt-OUT. trace:TASK-1051
    use super::enabled;
    use std::sync::Mutex;

    /// Serialize tests that mutate `AIDA_TUI_REDESIGN`. cargo runs tests in
    /// parallel within a process, so without this they trample each other.
    fn with_redesign_env<R>(val: Option<&str>, f: impl FnOnce() -> R) -> R {
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("AIDA_TUI_REDESIGN").ok();
        match val {
            Some(v) => std::env::set_var("AIDA_TUI_REDESIGN", v),
            None => std::env::remove_var("AIDA_TUI_REDESIGN"),
        }
        let result = f();
        match prev {
            Some(v) => std::env::set_var("AIDA_TUI_REDESIGN", v),
            None => std::env::remove_var("AIDA_TUI_REDESIGN"),
        }
        result
    }

    #[test]
    fn default_on_when_unset() {
        assert!(with_redesign_env(None, enabled));
    }

    #[test]
    fn opt_out_values_select_legacy() {
        for v in ["0", "false", "no", "off", "FALSE", "Off", " 0 "] {
            assert!(
                !with_redesign_env(Some(v), enabled),
                "expected opt-out for {v:?}"
            );
        }
    }

    #[test]
    fn redesign_stays_on_for_truthy_or_other_values() {
        for v in ["1", "true", "yes", "on", "", "anything"] {
            assert!(
                with_redesign_env(Some(v), enabled),
                "expected redesign-on for {v:?}"
            );
        }
    }
}

#[cfg(test)]
mod drive_gate_probe_tests {
    //! The drive-gate probe stream-selection (TASK-1079) + scope-route surfacing
    //! (TASK-1076): pure parsing, unit-testable without spawning `aida zen`.
    use super::{parse_gate_probe, DriveGateVerdict};

    const READY_SOLO: &str = r#"{"spec":"TASK-1","verdict":"ready","class":"ready","reason":"","under_specified":false,"forceable":false,"route":"solo","scope":""}"#;
    const READY_INTO_SCOPE: &str = r#"{"spec":"TASK-2","verdict":"ready","class":"ready","reason":"","under_specified":false,"forceable":false,"route":"into-scope","scope":"EPIC-54"}"#;
    const HOLD_SOFT: &str = r#"{"spec":"TASK-3","verdict":"hold","class":"soft-block","reason":"under-specified","under_specified":true,"forceable":true,"route":"solo","scope":""}"#;

    // A clean success parses the READY verdict off stdout. trace:TASK-1079
    #[test]
    fn success_parses_ready_verdict_from_stdout() {
        let v = parse_gate_probe(true, READY_SOLO, "").expect("verdict parses");
        assert_eq!(v.verdict, "ready");
        assert!(!v.routes_into_scope());
    }

    /// A HOLD verdict is exit-zero JSON on stdout → parsed like any verdict, so
    // the hold surfaces (not a false launch). trace:TASK-1079
    #[test]
    fn success_parses_hold_verdict_from_stdout() {
        let v = parse_gate_probe(true, HOLD_SOFT, "").expect("verdict parses");
        assert_eq!(v.verdict, "hold");
        assert!(v.forceable);
        assert!(v.under_specified);
    }

    /// TASK-1079: a non-zero exit whose error text landed on STDOUT (agent mode —
    /// TASK-972 routes agent-mode errors to stdout, and the TUI's captured child
    /// runs in agent mode) is surfaced, NOT swallowed as a generic failure.
    #[test]
    fn failure_reads_error_from_stdout_when_stderr_empty() {
        let err = parse_gate_probe(false, "error: no requirement matches `TASK-9`", "")
            .expect_err("a failed probe is an error");
        assert!(
            err.contains("no requirement matches"),
            "the stdout agent-error must surface, got: {err}"
        );
    }

    /// A non-zero exit with the error on stderr (human path) still surfaces.
    // trace:TASK-1079
    #[test]
    fn failure_reads_error_from_stderr_when_present() {
        let err = parse_gate_probe(false, "", "Error: boom").expect_err("error");
        assert!(err.contains("boom"), "got: {err}");
    }

    /// Both streams empty on failure → a non-empty generic message, never a
    // silent success. trace:TASK-1079
    #[test]
    fn failure_with_no_output_falls_back_to_generic_message() {
        let err = parse_gate_probe(false, "", "").expect_err("error");
        assert!(!err.trim().is_empty());
    }

    /// TASK-1076: a verdict emitting `route=into-scope` with a named scope is
    /// recognized as a scope drive (→ the TUI shows the routing + --solo toggle).
    #[test]
    fn into_scope_route_is_recognized() {
        let v = parse_gate_probe(true, READY_INTO_SCOPE, "").expect("verdict parses");
        assert!(v.routes_into_scope());
        assert_eq!(v.scope, "EPIC-54");
    }

    /// TASK-1076: back-compat — an OLDER `aida` binary omits `route`/`scope`;
    /// `#[serde(default)]` fills them so the verdict still parses and defaults to
    /// solo (launch straight away, pre-TASK-1076 behavior).
    #[test]
    fn missing_route_fields_default_to_solo() {
        let legacy = r#"{"spec":"TASK-4","verdict":"ready","class":"ready","reason":"","under_specified":false,"forceable":false}"#;
        let v: DriveGateVerdict = serde_json::from_str(legacy).expect("legacy verdict parses");
        assert!(!v.routes_into_scope(), "no route field → treated as solo");
        assert_eq!(v.route, "");
    }

    /// `route=into-scope` but an EMPTY scope is NOT a scope drive (nothing to
    // name / route into). trace:TASK-1076
    #[test]
    fn into_scope_with_empty_scope_is_not_a_scope_drive() {
        let s = r#"{"spec":"TASK-5","verdict":"ready","class":"ready","reason":"","under_specified":false,"forceable":false,"route":"into-scope","scope":"  "}"#;
        let v: DriveGateVerdict = serde_json::from_str(s).expect("parses");
        assert!(!v.routes_into_scope());
    }
}
