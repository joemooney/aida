//! Pure state machine for the action→target redesign prototype (Slice 1).
//!
//! This module is deliberately **IO-free**: no terminal, no shell-out, no
//! ratatui. It models the whole gesture grammar — which panel owns the
//! keyboard, the scope→verb navigation stack (and the breadcrumb it
//! implies), the multi-select target set, the fuzzy filter, and the modal
//! flag — as plain data with pure transition methods. Everything that
//! touches a terminal lives in the parent `redesign` module; everything
//! testable-without-a-TTY lives here, and is unit-tested at the bottom.
//!
//! The design doc this implements:
//! `docs/plans/2026-06-25-tui-action-target-redesign.md` §1–§3.
//!
//! trace:STORY-690 | ai:claude

/// One row in the bottom (target) panel — a backlog item. Kept minimal:
/// the loop validates selection + navigation, not spec rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetItem {
    pub id: String,
    pub title: String,
    pub status: String,
    /// Full body text for the item modal (Slice 1 renders it as a plain
    /// paragraph; STORY-689 makes it markdown later).
    pub body: String,
}

/// A scope is a noun with children (its verbs). At launch the top panel
/// holds the scopes; drilling into one replaces the top panel with that
/// scope's verbs. Only [`Scope::Backlog`] is functional in Slice 1; the
/// rest are non-functional labels that prove the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Backlog,
    Queue,
    Prs,
    History,
    Findings,
    Sessions,
}

impl Scope {
    /// All scopes in display order. Backlog leads — it is the only one
    /// wired in Slice 1.
    pub fn all() -> &'static [Scope] {
        &[
            Scope::Backlog,
            Scope::Queue,
            Scope::Prs,
            Scope::History,
            Scope::Findings,
            Scope::Sessions,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::Backlog => "Backlog",
            Scope::Queue => "Queue",
            Scope::Prs => "PRs",
            Scope::History => "History",
            Scope::Findings => "Findings",
            Scope::Sessions => "Sessions",
        }
    }

    /// A short noun-phrase describing what the scope holds — shown next to
    /// the label in the top panel.
    pub fn hint(self) -> &'static str {
        match self {
            Scope::Backlog => "approved + planned specs",
            Scope::Queue => "your routed work",
            Scope::Prs => "open pull requests",
            Scope::History => "completed specs",
            Scope::Findings => "triage items",
            Scope::Sessions => "recorded conversations",
        }
    }

    /// Is this scope wired for real in Slice 1? Only Backlog drills.
    pub fn is_functional(self) -> bool {
        matches!(self, Scope::Backlog)
    }

    /// The verbs this scope exposes. Slice 1 hardcodes Backlog's set (the
    /// §5 "lean registry" fork is deferred). `groom` is the only verb that
    /// executes; the rest are stubs that prove the verb list + breadcrumb.
    pub fn verbs(self) -> Vec<Verb> {
        match self {
            Scope::Backlog => vec![Verb::Groom, Verb::Approve, Verb::Archive],
            _ => Vec::new(),
        }
    }
}

/// A verb (leaf action) applied to the current target selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Groom,
    Approve,
    Archive,
}

impl Verb {
    pub fn label(self) -> &'static str {
        match self {
            Verb::Groom => "groom",
            Verb::Approve => "approve",
            Verb::Archive => "archive",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Verb::Groom => "cross-spec grooming + disposition",
            Verb::Approve => "advisor-only: draft → approved",
            Verb::Archive => "mark non-core specs archived",
        }
    }

    /// Is this verb wired in Slice 1? Only `groom` executes (stubbed);
    /// `approve` / `archive` are present to prove the verb list renders and
    /// the breadcrumb tracks the highlighted verb.
    pub fn is_functional(self) -> bool {
        matches!(self, Verb::Groom)
    }
}

/// Which panel owns the keyboard. Top = the current list (scopes, or verbs
/// after a drill); Bottom = the target set (multi-selectable items).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Top,
    Bottom,
}

/// The navigation depth. `Scopes` is the cold-open top panel; `Verbs`
/// is the top panel after drilling into a scope. The breadcrumb reads off
/// this plus the highlighted entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    #[default]
    Scopes,
    Verbs,
}

/// A pending confirmation popup — set when the user runs a verb with an
/// empty selection ("groom all N?"). The parent module renders it; the
/// state machine only tracks that one is open and which verb it gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmAll {
    pub verb: Verb,
    pub count: usize,
}

/// The full pure UI state for the redesign prototype.
#[derive(Debug, Clone)]
pub struct RedesignState {
    /// Which panel has the keyboard.
    pub focus: Focus,
    /// Scopes or verbs in the top panel.
    pub level: Level,
    /// The drilled-into scope, set when `level == Verbs`. `None` at the
    /// scope level.
    pub scope: Option<Scope>,
    /// Highlighted index within the top panel (scope or verb list).
    pub top_idx: usize,
    /// Highlighted index within the bottom (target) panel.
    pub bottom_idx: usize,
    /// The target items (backlog rows). Loaded once by the parent module.
    pub items: Vec<TargetItem>,
    /// Selected item indices (multi-select). A `BTreeSet`-free `Vec<bool>`
    /// parallel to `items` keeps toggle/clear trivial and order-stable.
    pub selected: Vec<bool>,
    /// The fuzzy filter buffer for the *focused* list. Cleared on focus
    /// change and on level change so a stale filter never hides a list.
    pub filter: String,
    /// Open item modal (full body preview), if any — holds the item index.
    pub modal: Option<usize>,
    /// Pending "apply to all?" confirmation, if any.
    pub confirm: Option<ConfirmAll>,
    /// Ambient context shown in the status line.
    pub role: String,
    /// Transient status message (last executed action / stub notice).
    pub status: Option<String>,
    /// The active palette. Defaults to the reference Catppuccin Mocha; the
    /// launcher overrides it from `[tui] theme`. Carried here so the pure
    /// state owns no render code yet the parent can paint in the user's
    /// palette. trace:STORY-690 | ai:claude
    pub theme: crate::theme::Theme,
}

impl RedesignState {
    /// Build a fresh state at the scope level with the given target items.
    pub fn new(items: Vec<TargetItem>, role: impl Into<String>) -> Self {
        let selected = vec![false; items.len()];
        RedesignState {
            focus: Focus::Top,
            level: Level::Scopes,
            scope: None,
            top_idx: 0,
            bottom_idx: 0,
            items,
            selected,
            filter: String::new(),
            modal: None,
            confirm: None,
            role: role.into(),
            status: None,
            theme: crate::theme::Theme::default(),
        }
    }

    // --- Breadcrumb -------------------------------------------------------

    /// The breadcrumb string, e.g. "Backlog" or "Backlog › groom". Tracks
    /// depth and the highlighted verb so the user always knows where they
    /// are (the §3 non-negotiable).
    pub fn breadcrumb(&self) -> String {
        match (self.level, self.scope) {
            (Level::Scopes, _) => self
                .top_scope()
                .map(|s| s.label().to_string())
                .unwrap_or_else(|| "—".to_string()),
            (Level::Verbs, Some(scope)) => {
                let verb = self.top_verb().map(|v| v.label()).unwrap_or("—");
                format!("{} › {}", scope.label(), verb)
            }
            (Level::Verbs, None) => "—".to_string(),
        }
    }

    // --- Top-panel accessors ---------------------------------------------

    /// The top panel's entry count after applying the filter (when the top
    /// panel is focused).
    pub fn top_len(&self) -> usize {
        self.top_indices().len()
    }

    /// The filtered indices of the top panel, in display order. The filter
    /// only applies when the top panel is focused.
    pub fn top_indices(&self) -> Vec<usize> {
        let total = match self.level {
            Level::Scopes => Scope::all().len(),
            Level::Verbs => self.scope.map(|s| s.verbs().len()).unwrap_or(0),
        };
        if self.focus != Focus::Top || self.filter.trim().is_empty() {
            return (0..total).collect();
        }
        (0..total)
            .filter(|&i| {
                let label = self.top_label_at(i);
                crate::cmd_palette::fuzzy_score(&self.filter, &label).is_some()
            })
            .collect()
    }

    fn top_label_at(&self, i: usize) -> String {
        match self.level {
            Level::Scopes => Scope::all()
                .get(i)
                .map(|s| s.label().to_string())
                .unwrap_or_default(),
            Level::Verbs => self
                .scope
                .and_then(|s| s.verbs().get(i).map(|v| v.label().to_string()))
                .unwrap_or_default(),
        }
    }

    /// The currently-highlighted scope (scope level only).
    pub fn top_scope(&self) -> Option<Scope> {
        if self.level != Level::Scopes {
            return None;
        }
        let idxs = self.top_indices();
        idxs.get(self.top_idx)
            .and_then(|&i| Scope::all().get(i).copied())
    }

    /// The currently-highlighted verb (verb level only).
    pub fn top_verb(&self) -> Option<Verb> {
        if self.level != Level::Verbs {
            return None;
        }
        let verbs = self.scope?.verbs();
        let idxs = self.top_indices();
        idxs.get(self.top_idx).and_then(|&i| verbs.get(i).copied())
    }

    // --- Bottom-panel accessors ------------------------------------------

    /// The filtered indices of the bottom (target) panel, in display order.
    /// The filter only applies when the bottom panel is focused.
    pub fn bottom_indices(&self) -> Vec<usize> {
        if self.focus != Focus::Bottom || self.filter.trim().is_empty() {
            return (0..self.items.len()).collect();
        }
        (0..self.items.len())
            .filter(|&i| {
                let item = &self.items[i];
                let hay = format!("{} {}", item.id, item.title);
                crate::cmd_palette::fuzzy_score(&self.filter, &hay).is_some()
            })
            .collect()
    }

    pub fn bottom_len(&self) -> usize {
        self.bottom_indices().len()
    }

    /// Number of selected items (across the whole set, not just filtered).
    pub fn selected_count(&self) -> usize {
        self.selected.iter().filter(|&&s| s).count()
    }

    // --- Transitions ------------------------------------------------------

    /// Move the highlight down within the focused list (saturating at the
    /// bottom). Filter-aware via the focused list's length.
    pub fn move_down(&mut self) {
        let len = self.focused_len();
        if len == 0 {
            return;
        }
        let idx = self.focused_idx_mut();
        if *idx + 1 < len {
            *idx += 1;
        }
    }

    /// Move the highlight up within the focused list (saturating at 0).
    pub fn move_up(&mut self) {
        let idx = self.focused_idx_mut();
        if *idx > 0 {
            *idx -= 1;
        }
    }

    fn focused_len(&self) -> usize {
        match self.focus {
            Focus::Top => self.top_len(),
            Focus::Bottom => self.bottom_len(),
        }
    }

    fn focused_idx_mut(&mut self) -> &mut usize {
        match self.focus {
            Focus::Top => &mut self.top_idx,
            Focus::Bottom => &mut self.bottom_idx,
        }
    }

    /// Tab → focus the bottom (target) panel. Clears the filter so a
    /// top-panel filter doesn't leak into the item list.
    pub fn focus_bottom(&mut self) {
        if self.focus != Focus::Bottom {
            self.focus = Focus::Bottom;
            self.filter.clear();
            self.clamp_indices();
        }
    }

    /// Shift-Tab → focus back to the top (action) panel.
    pub fn focus_top(&mut self) {
        if self.focus != Focus::Top {
            self.focus = Focus::Top;
            self.filter.clear();
            self.clamp_indices();
        }
    }

    /// Enter at the scope level → drill into the highlighted scope's verbs
    /// (only functional scopes drill). Returns `true` if a drill happened.
    pub fn drill(&mut self) -> bool {
        if self.level != Level::Scopes {
            return false;
        }
        let Some(scope) = self.top_scope() else {
            return false;
        };
        if !scope.is_functional() {
            self.status = Some(format!("{} is not wired yet (Slice 1)", scope.label()));
            return false;
        }
        self.level = Level::Verbs;
        self.scope = Some(scope);
        self.top_idx = 0;
        self.focus = Focus::Top;
        self.filter.clear();
        true
    }

    /// Esc → pop one level (verbs → scopes), or, if a modal/confirm is
    /// open, the parent closes that first. Returns `true` if a level was
    /// popped (the parent uses the return to decide whether Esc should
    /// also exit at the top level).
    pub fn pop(&mut self) -> bool {
        // Bottom focus pops back to the top panel first.
        if self.focus == Focus::Bottom {
            self.focus_top();
            return true;
        }
        if self.level == Level::Verbs {
            self.level = Level::Scopes;
            // Restore the scope highlight to the scope we came from.
            if let Some(scope) = self.scope {
                if let Some(pos) = Scope::all().iter().position(|&s| s == scope) {
                    self.top_idx = pos;
                }
            }
            self.scope = None;
            self.filter.clear();
            return true;
        }
        false
    }

    /// Space → toggle-select the item under the bottom cursor. No-op when
    /// the bottom panel isn't focused or there's no item.
    pub fn toggle_select(&mut self) {
        if self.focus != Focus::Bottom {
            return;
        }
        let idxs = self.bottom_indices();
        if let Some(&real) = idxs.get(self.bottom_idx) {
            self.selected[real] = !self.selected[real];
        }
    }

    /// `a` → select all (filtered). `A` → select none.
    pub fn select_all(&mut self) {
        for &i in &self.bottom_indices() {
            self.selected[i] = true;
        }
    }

    pub fn select_none(&mut self) {
        for s in &mut self.selected {
            *s = false;
        }
    }

    /// The ids of the currently-selected items (selection order = item
    /// order). Used by the (stubbed) executor.
    pub fn selected_ids(&self) -> Vec<String> {
        self.items
            .iter()
            .zip(self.selected.iter())
            .filter(|(_, &s)| s)
            .map(|(item, _)| item.id.clone())
            .collect()
    }

    /// Enter on a verb → either execute on the current selection, or, if
    /// nothing is selected, raise the "apply to all N?" confirmation.
    /// Returns the [`RunOutcome`] so the parent module performs the IO
    /// (shell-out / status line) — the pure machine only decides.
    pub fn run_verb(&mut self) -> RunOutcome {
        let Some(verb) = self.top_verb() else {
            return RunOutcome::None;
        };
        if !verb.is_functional() {
            self.status = Some(format!("{} is not wired yet (Slice 1)", verb.label()));
            return RunOutcome::None;
        }
        let count = self.selected_count();
        if count == 0 {
            // None selected → confirm "apply to all N?".
            let all = self.items.len();
            self.confirm = Some(ConfirmAll { verb, count: all });
            return RunOutcome::NeedsConfirm(ConfirmAll { verb, count: all });
        }
        RunOutcome::Execute {
            verb,
            ids: self.selected_ids(),
        }
    }

    /// Resolve an open "apply to all?" confirmation. `accept == true`
    /// executes the verb on *all* items; either way the confirm clears.
    pub fn resolve_confirm(&mut self, accept: bool) -> RunOutcome {
        let Some(c) = self.confirm.take() else {
            return RunOutcome::None;
        };
        if !accept {
            return RunOutcome::None;
        }
        RunOutcome::Execute {
            verb: c.verb,
            ids: self.items.iter().map(|i| i.id.clone()).collect(),
        }
    }

    /// `p` / Enter on a focused item → open the item modal for the item
    /// under the bottom cursor. No-op unless the bottom panel is focused.
    pub fn open_modal(&mut self) {
        if self.focus != Focus::Bottom {
            return;
        }
        let idxs = self.bottom_indices();
        if let Some(&real) = idxs.get(self.bottom_idx) {
            self.modal = Some(real);
        }
    }

    pub fn close_modal(&mut self) {
        self.modal = None;
    }

    /// Append a char to the focused list's fuzzy filter and clamp the
    /// cursor into the new filtered range.
    pub fn push_filter(&mut self, c: char) {
        self.filter.push(c);
        self.clamp_indices();
    }

    /// Backspace the filter.
    pub fn pop_filter(&mut self) {
        self.filter.pop();
        self.clamp_indices();
    }

    /// Keep both cursors inside their (possibly newly-filtered) ranges.
    fn clamp_indices(&mut self) {
        let top = self.top_len();
        if top == 0 {
            self.top_idx = 0;
        } else if self.top_idx >= top {
            self.top_idx = top - 1;
        }
        let bot = self.bottom_len();
        if bot == 0 {
            self.bottom_idx = 0;
        } else if self.bottom_idx >= bot {
            self.bottom_idx = bot - 1;
        }
    }
}

/// What [`RedesignState::run_verb`] / [`RedesignState::resolve_confirm`]
/// decided. The parent module turns this into IO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// Nothing to do (no verb, non-functional verb, declined confirm).
    None,
    /// Raise the "apply to all N?" popup.
    NeedsConfirm(ConfirmAll),
    /// Execute `verb` on these ids.
    Execute { verb: Verb, ids: Vec<String> },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize) -> Vec<TargetItem> {
        // Titles are deliberately digit-free so a digit-only fuzzy query
        // exercises the id field alone (the fuzzy core is a subsequence
        // matcher, so a stray digit in the title would broaden the match).
        (0..n)
            .map(|i| TargetItem {
                id: format!("STORY-{i}"),
                title: format!("item title number {}", word_for(i)),
                status: "Approved".into(),
                body: format!("body of item {}", word_for(i)),
            })
            .collect()
    }

    fn word_for(i: usize) -> &'static str {
        const W: [&str; 12] = [
            "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
            "eleven",
        ];
        W.get(i).copied().unwrap_or("many")
    }

    fn state(n: usize) -> RedesignState {
        RedesignState::new(items(n), "advisor")
    }

    #[test]
    fn cold_open_is_scopes_top_focus() {
        let s = state(3);
        assert_eq!(s.level, Level::Scopes);
        assert_eq!(s.focus, Focus::Top);
        assert_eq!(s.top_scope(), Some(Scope::Backlog));
        assert_eq!(s.breadcrumb(), "Backlog");
    }

    #[test]
    fn drill_into_backlog_shows_verbs() {
        let mut s = state(3);
        assert!(s.drill());
        assert_eq!(s.level, Level::Verbs);
        assert_eq!(s.scope, Some(Scope::Backlog));
        assert_eq!(s.top_verb(), Some(Verb::Groom));
        assert_eq!(s.breadcrumb(), "Backlog › groom");
    }

    #[test]
    fn non_functional_scope_does_not_drill() {
        let mut s = state(3);
        // Move highlight off Backlog onto Queue (index 1).
        s.move_down();
        assert_eq!(s.top_scope(), Some(Scope::Queue));
        assert!(!s.drill());
        assert_eq!(s.level, Level::Scopes);
        assert!(s.status.is_some());
    }

    #[test]
    fn breadcrumb_tracks_highlighted_verb() {
        let mut s = state(3);
        s.drill();
        assert_eq!(s.breadcrumb(), "Backlog › groom");
        s.move_down(); // → approve
        assert_eq!(s.breadcrumb(), "Backlog › approve");
        s.move_down(); // → archive
        assert_eq!(s.breadcrumb(), "Backlog › archive");
    }

    #[test]
    fn esc_pops_verbs_back_to_scopes_restoring_highlight() {
        let mut s = state(3);
        s.drill();
        assert_eq!(s.level, Level::Verbs);
        assert!(s.pop());
        assert_eq!(s.level, Level::Scopes);
        // Highlight is restored to Backlog (the scope we drilled from).
        assert_eq!(s.top_scope(), Some(Scope::Backlog));
        // A second pop at the top level returns false (parent may exit).
        assert!(!s.pop());
    }

    #[test]
    fn tab_focuses_bottom_shift_tab_returns_top() {
        let mut s = state(3);
        s.drill();
        s.focus_bottom();
        assert_eq!(s.focus, Focus::Bottom);
        s.focus_top();
        assert_eq!(s.focus, Focus::Top);
    }

    #[test]
    fn esc_from_bottom_focus_pops_to_top_before_level() {
        let mut s = state(3);
        s.drill();
        s.focus_bottom();
        assert!(s.pop());
        assert_eq!(s.focus, Focus::Top);
        // Still on the verb level — the first Esc only changed focus.
        assert_eq!(s.level, Level::Verbs);
    }

    #[test]
    fn space_toggles_multi_select() {
        let mut s = state(3);
        s.drill();
        s.focus_bottom();
        assert_eq!(s.selected_count(), 0);
        s.toggle_select(); // select item 0
        assert_eq!(s.selected_count(), 1);
        s.move_down();
        s.toggle_select(); // select item 1
        assert_eq!(s.selected_count(), 2);
        assert_eq!(s.selected_ids(), vec!["STORY-0", "STORY-1"]);
        s.move_up();
        s.toggle_select(); // deselect item 0
        assert_eq!(s.selected_count(), 1);
        assert_eq!(s.selected_ids(), vec!["STORY-1"]);
    }

    #[test]
    fn space_is_noop_when_top_focused() {
        let mut s = state(3);
        s.drill();
        s.toggle_select();
        assert_eq!(s.selected_count(), 0);
    }

    #[test]
    fn run_verb_with_selection_executes_on_selected() {
        let mut s = state(3);
        s.drill();
        s.focus_bottom();
        s.toggle_select(); // STORY-0
        s.focus_top();
        let outcome = s.run_verb();
        assert_eq!(
            outcome,
            RunOutcome::Execute {
                verb: Verb::Groom,
                ids: vec!["STORY-0".to_string()],
            }
        );
    }

    #[test]
    fn run_verb_with_no_selection_asks_to_confirm_all() {
        let mut s = state(3);
        s.drill();
        let outcome = s.run_verb();
        assert_eq!(
            outcome,
            RunOutcome::NeedsConfirm(ConfirmAll {
                verb: Verb::Groom,
                count: 3,
            })
        );
        assert!(s.confirm.is_some());
    }

    #[test]
    fn confirm_all_accept_executes_on_every_item() {
        let mut s = state(3);
        s.drill();
        s.run_verb(); // raises confirm
        let outcome = s.resolve_confirm(true);
        assert_eq!(
            outcome,
            RunOutcome::Execute {
                verb: Verb::Groom,
                ids: vec![
                    "STORY-0".to_string(),
                    "STORY-1".to_string(),
                    "STORY-2".to_string(),
                ],
            }
        );
        assert!(s.confirm.is_none());
    }

    #[test]
    fn confirm_all_decline_does_nothing() {
        let mut s = state(3);
        s.drill();
        s.run_verb();
        assert_eq!(s.resolve_confirm(false), RunOutcome::None);
        assert!(s.confirm.is_none());
    }

    #[test]
    fn non_functional_verb_does_not_execute() {
        let mut s = state(3);
        s.drill();
        s.move_down(); // → approve (stub)
        assert_eq!(s.run_verb(), RunOutcome::None);
        assert!(s.confirm.is_none());
        assert!(s.status.is_some());
    }

    #[test]
    fn select_all_then_none() {
        let mut s = state(4);
        s.drill();
        s.focus_bottom();
        s.select_all();
        assert_eq!(s.selected_count(), 4);
        s.select_none();
        assert_eq!(s.selected_count(), 0);
    }

    #[test]
    fn modal_opens_on_focused_item_and_closes() {
        let mut s = state(3);
        s.drill();
        s.focus_bottom();
        s.move_down(); // item 1
        s.open_modal();
        assert_eq!(s.modal, Some(1));
        s.close_modal();
        assert_eq!(s.modal, None);
    }

    #[test]
    fn modal_noop_when_top_focused() {
        let mut s = state(3);
        s.drill();
        s.open_modal();
        assert_eq!(s.modal, None);
    }

    #[test]
    fn fuzzy_filter_narrows_top_verbs() {
        let mut s = state(3);
        s.drill();
        // "ar" matches "archive" (and not "groom"); "approve" lacks a
        // subsequence 'a''r' contiguous? it has a..r? approve = a-p-p-r-o-v-e
        // contains a then r → also matches. So filter to "arch".
        s.push_filter('a');
        s.push_filter('r');
        s.push_filter('c');
        s.push_filter('h');
        let idxs = s.top_indices();
        // Only "archive" survives.
        assert_eq!(idxs, vec![2]);
        assert_eq!(s.top_verb(), Some(Verb::Archive));
    }

    #[test]
    fn fuzzy_filter_narrows_bottom_items_by_id() {
        let mut s = state(12);
        s.drill();
        s.focus_bottom();
        // Titles are digit-free (see `items`), so "11" can only be a
        // subsequence of an id with two 1s — STORY-11 alone.
        s.push_filter('1');
        s.push_filter('1');
        let idxs = s.bottom_indices();
        assert_eq!(idxs.len(), 1);
        assert_eq!(s.items[idxs[0]].id, "STORY-11");
    }

    #[test]
    fn focus_change_clears_filter() {
        let mut s = state(3);
        s.drill();
        s.push_filter('g');
        assert!(!s.filter.is_empty());
        s.focus_bottom();
        assert!(s.filter.is_empty());
    }

    #[test]
    fn move_down_saturates_at_list_end() {
        let mut s = state(3);
        // 6 scopes; move past the end and confirm it clamps.
        for _ in 0..20 {
            s.move_down();
        }
        assert_eq!(s.top_idx, Scope::all().len() - 1);
    }
}
