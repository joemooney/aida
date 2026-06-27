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
use store::{LoadedComment, LoadedSpec, SpecStore};

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

/// The trailing marker on a Test-scope row whose spec carries a `## Test Plan`
/// section. A small suffix glyph so the operator sees which shipped specs have
/// verification steps. trace:STORY-699 | ai:claude
const TEST_PLAN_MARKER: &str = "🧪";

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
    st.status = Some(if store.is_some() {
        "Slice 1 prototype — Backlog / Open scopes. ? help · q quits.".to_string()
    } else {
        "Slice 1 prototype — store unavailable (no in-process data). ? help · q quits.".to_string()
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
/// when nothing resolves. trace:STORY-697 | ai:claude
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
/// epic parent. trace:STORY-697 | ai:claude
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
/// marker are both unset. trace:STORY-697 | ai:claude
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
/// status-line render. trace:STORY-695 | ai:claude
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
/// trace:STORY-695 | ai:claude
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
/// trace:STORY-695 | ai:claude
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
/// reflected. trace:TASK-934 | ai:claude
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
/// active scope under the new focus. trace:STORY-695 | ai:claude
fn invalidate_scope_cache(cache: &mut HashMap<Scope, Vec<TargetItem>>, loaded: &mut Scope) {
    cache.clear();
    // Sessions is non-functional, so it can never equal the active functional
    // scope — forcing the sync to re-fetch.
    *loaded = Scope::Sessions;
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
        comments: Vec::new(),
    }
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
    loop {
        // Keep the bottom panel's target set following the active scope
        // (highlighted at the scope level, drilled-into at the verb level).
        sync_scope_items(st, store, cache, loaded, focus_set.as_ref());
        terminal.draw(|f| render(f, st, loaded_spec.as_ref()))?;
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

    // The new-spec TITLE input modal captures all typing until the operator
    // confirms (Enter → create a Draft from the typed title) or cancels (Esc).
    // An empty/whitespace title cancels without creating. Printable chars
    // append; Backspace edits. trace:TASK-931 | ai:claude
    if st.new_input_open() {
        match key.code {
            KeyCode::Enter => match st.take_new_input() {
                Some(title) => create_new_spec(st, store, cache, loaded, &title),
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
                apply_outcome(terminal, st, store, loaded_spec, outcome)?;
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
/// `aida show` subprocess. trace:STORY-693 | ai:claude
fn open_modal_with_body(
    st: &mut RedesignState,
    store: Option<&SpecStore>,
    loaded_spec: &mut Option<LoadedSpec>,
) {
    *loaded_spec = load_focused_spec(st, store).map(|spec| test_plan_view(st, spec));
    st.open_modal();
}

/// For a Test-scope preview (STORY-699), swap the loaded spec's description for
/// its extracted `## Test Plan` section so the modal renders the do→expect steps
/// prominently; falls back to the full description when there is no test plan,
/// and leaves every other scope's spec untouched. The structured field header
/// (type/status/priority/tags) is preserved either way. trace:STORY-699 | ai:claude
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
        RunOutcome::Accept { done, skipped } => {
            // The reviewer accepts each finished Done spec: run the
            // implementation-approval transition (`aida edit <id> --status
            // completed`, carrying reviewer authority) and record a reviewer-
            // acceptance comment. The Done-status mirror of Approve.
            // trace:TASK-933 | ai:claude
            let mut accepted = Vec::new();
            let mut failed = Vec::new();
            for id in &done {
                if accept_spec(id) {
                    accepted.push(id.clone());
                } else {
                    failed.push(id.clone());
                }
            }
            st.status = Some(accept_status(&accepted, &failed, &skipped));
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
            // trigger via `aida defer <id> --until "<trigger>"`. trace:TASK-921
            let mut deferred = Vec::new();
            let mut failed = Vec::new();
            for id in &ids {
                if defer_spec(id, &trigger) {
                    deferred.push(id.clone());
                } else {
                    failed.push(id.clone());
                }
            }
            st.status = Some(defer_status(&deferred, &failed, &trigger));
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

/// The argument vector for the reviewer's implementation-approval transition:
/// `aida edit <id> --status completed`. The Done-status counterpart to
/// `approve`'s `--status approved`. Kept as a pure arg vector (passed to
/// `Command::args`, never a shell string) so the id — and the verb shape — are
/// unit-testable without spawning. trace:TASK-933 | ai:claude
fn accept_edit_args(id: &str) -> Vec<&str> {
    vec!["edit", id, "--status", "completed"]
}

/// The argument vector for the reviewer-acceptance comment recorded alongside
/// the accept transition: `aida comment add <id> "<note>"`. The note is a
/// SINGLE arg-vector element, so it is never shell-parsed (no command
/// substitution, no globbing). Pure so it is unit-testable. trace:TASK-933 | ai:claude
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
/// [`approve_spec`]. trace:TASK-933 | ai:claude
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
/// [`approve_status`]. trace:TASK-933 | ai:claude
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
/// as a single argument (no shell), so embedded spaces are safe. trace:TASK-921 | ai:claude
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
/// The mirror of [`approve_status`]. trace:TASK-921 | ai:claude
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

/// The argument vector for creating a Draft spec — passed to `Command::args`
/// so each element (notably the operator-typed `title`) is an OS-level argument
/// that is NEVER shell-parsed. A title with backticks, quotes, or `$` is inert
/// (no command substitution, no globbing) because there is no shell in the
/// pipeline. Pure (no IO) so the safe arg vector is unit-testable without
/// spawning. trace:TASK-931 | ai:claude
fn new_spec_args(title: &str) -> Vec<&str> {
    vec![
        "add", "--title", title, "--type", "task", "--status", "draft",
    ]
}

/// Parse the spec id out of `aida add`'s success line (`Added: TASK-932 - …`).
/// Returns `None` when no such line is present (or the id is the `?` placeholder
/// the CLI prints when a spec_id wasn't assigned). Pure so it is unit-testable.
/// trace:TASK-931 | ai:claude
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
/// the CLI reported one) plus a truncated title. Pure (no IO) so it is unit
/// testable. trace:TASK-931 | ai:claude
fn create_status(created: Option<&str>, title: &str) -> String {
    let short: String = if title.chars().count() > 50 {
        format!("{}…", title.chars().take(50).collect::<String>())
    } else {
        title.to_string()
    };
    match created {
        Some(id) => format!("created {id} (Draft): {short}"),
        None => format!("created Draft spec: {short}"),
    }
}

/// Create a fresh Draft spec from the operator-typed `title` via
/// `aida add --title <title> --type task --status draft`. The title is passed as
/// a SINGLE arg-vector element (see [`new_spec_args`]) — never a shell string —
/// so embedded backticks/quotes/`$` are safe. On success the active scope's
/// item cache is invalidated so the next [`sync_scope_items`] re-fetches
/// in-process and the new draft appears if it is in view; the created spec id is
/// reported on the status line. trace:TASK-931 | ai:claude
fn create_new_spec(
    st: &mut RedesignState,
    _store: Option<&SpecStore>,
    cache: &mut HashMap<Scope, Vec<TargetItem>>,
    loaded: &mut Scope,
    title: &str,
) {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(new_spec_args(title));
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let created = parse_created_spec_id(&stdout);
            // Refresh the active scope so the new draft shows if it is in view
            // (e.g. the Open scope, which includes drafts). The fetch is the
            // same in-process cache read the rest of the TUI uses; clearing the
            // per-scope cache + resetting the `loaded` sentinel forces it on the
            // next loop iteration. trace:TASK-931 | ai:claude
            invalidate_scope_cache(cache, loaded);
            st.status = Some(create_status(created.as_deref(), title));
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            st.status = Some(format!("new: aida add failed: {}", err.trim()));
        }
        Err(e) => {
            st.status = Some(format!("new: could not run aida add: {e}"));
        }
    }
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
    // The defer revisit-trigger input overlays everything else. trace:TASK-921
    if let Some(di) = &st.defer_input {
        render_defer_input(f, f.area(), theme, di);
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
/// `st.help_content()`, so this is render-only. trace:TASK-922 | ai:claude
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

        let mut row_spans = vec![
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

fn render_hint(f: &mut Frame, area: Rect, st: &RedesignState, theme: &Theme) {
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
/// headers, empty produces the empty-state message. trace:TASK-932 | ai:claude
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

/// Render the single-line revisit-trigger input modal for the `defer` verb:
/// a prompt, the typed buffer with a block cursor, the target count, and the
/// confirm/cancel keys. trace:TASK-921 | ai:claude
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

/// Render the new-spec TITLE input modal (TASK-931): a single-line prompt with
/// a block cursor for the title of a fresh Draft spec. Mirrors the defer-input
/// modal's shape. Enter creates; Esc (or an empty title) cancels.
/// trace:TASK-931 | ai:claude
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
/// trace:STORY-697 | ai:claude
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
        }
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
            comments: vec![],
        }
    }

    #[test]
    fn renders_scope_level() {
        draw(&sample(5), 100, 30);
    }

    /// Flatten the painted backend into one string so a render test can assert
    /// a glyph sequence is (or is not) present. trace:TASK-945 | ai:claude
    fn rendered_text(st: &RedesignState, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
        terminal.draw(|f| render(f, st, None)).expect("render");
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for cell in buf.content() {
            out.push_str(cell.symbol());
        }
        out
    }

    /// The `/query` find prompt renders ONLY in find mode (TASK-945): a
    /// confirmed-but-applied filter narrows silently (no prompt); entering find
    /// mode shows the live `/…` prompt. trace:TASK-945 | ai:claude
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

    /// The Test scope (STORY-699): a row carrying a `## Test Plan` gets the
    /// trailing marker, and the render paints over the backend without
    /// panicking. trace:STORY-699
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
    /// section is absent; leaves other scopes' specs untouched. trace:STORY-699
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
            vec![
                Verb::Show,
                Verb::Why,
                Verb::RequestApproval,
                Verb::Approve,
                Verb::Defer
            ]
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
        let args = new_spec_args(title);
        assert_eq!(
            args,
            vec!["add", "--title", title, "--type", "task", "--status", "draft"],
        );
        // The whole title is exactly one element (not split on spaces/specials).
        assert_eq!(args.iter().filter(|a| **a == title).count(), 1);
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
        let with_id = create_status(Some("TASK-9"), "do the thing");
        assert!(with_id.contains("TASK-9"));
        assert!(with_id.contains("do the thing"));
        // Without an id we still confirm a Draft was created.
        let no_id = create_status(None, "do the thing");
        assert!(no_id.contains("Draft"));
        // Long titles are truncated with an ellipsis.
        let long = "x".repeat(80);
        assert!(create_status(Some("TASK-1"), &long).contains('…'));
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
        };
        st.modal_scroll = 9999; // way past the end
        draw_with_spec(&st, Some(&spec), 100, 30);
    }

    /// `comment_lines` (TASK-932): N comments → N author headers + bodies;
    /// an empty list → the "No comments." empty-state line. trace:TASK-932
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
}
