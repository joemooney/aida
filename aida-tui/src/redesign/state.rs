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
    /// The requirement type (e.g. "Task", "Story", "Bug"), carried so the
    /// Open scope's rows render id / type / status / title. trace:STORY-690
    pub req_type: String,
    pub status: String,
    /// Priority, when the data path supplies it (the cache-fast
    /// `aida list --json` does not today; left empty then). trace:STORY-690
    pub priority: String,
    /// Full body text for the item modal (Slice 1 renders it as a plain
    /// paragraph; STORY-689 makes it markdown later).
    pub body: String,
}

impl TargetItem {
    /// Is this item in the Draft state? The item-state-conditional verb logic
    /// keys off this — `request approval` only applies to drafts. Matched
    /// case-insensitively so a "Draft" / "draft" status both qualify.
    /// trace:STORY-690 | ai:claude
    pub fn is_draft(&self) -> bool {
        self.status.eq_ignore_ascii_case("draft")
    }

    /// Is this item in the Approved state? The mirror of [`Self::is_draft`]
    /// for the `queue` verb, which only routes Approved specs to the
    /// implementer queue. Matched case-insensitively.
    /// trace:TASK-915 | ai:claude
    pub fn is_approved(&self) -> bool {
        self.status.eq_ignore_ascii_case("approved")
    }
}

/// A scope is a noun with children (its verbs). At launch the top panel
/// holds the scopes; drilling into one replaces the top panel with that
/// scope's verbs. Only [`Scope::Backlog`] is functional in Slice 1; the
/// rest are non-functional labels that prove the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Backlog,
    Open,
    Queue,
    Prs,
    History,
    Findings,
    Sessions,
}

impl Scope {
    /// All scopes in display order. Backlog leads; Open sits beside it —
    /// both are wired functional scopes.
    pub fn all() -> &'static [Scope] {
        &[
            Scope::Backlog,
            Scope::Open,
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
            Scope::Open => "Open",
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
            Scope::Open => "the open backlog (all unfinished specs)",
            Scope::Queue => "your routed work",
            Scope::Prs => "open pull requests",
            Scope::History => "completed specs",
            Scope::Findings => "triage items",
            Scope::Sessions => "recorded conversations",
        }
    }

    /// Is this scope wired for real? Backlog and Open both drill.
    pub fn is_functional(self) -> bool {
        matches!(self, Scope::Backlog | Scope::Open)
    }

    /// The *static* verbs this scope exposes — those that do not depend on
    /// the focused item's state. For the Open scope this is the always-on
    /// pair (`show` / `why`); item-state-conditional verbs (`request
    /// approval`, only for Draft specs) are layered on by
    /// [`verb_list_for`]. Slice 1 hardcodes the sets (the §5 "lean registry"
    /// fork is deferred). trace:STORY-690 | ai:claude
    pub fn verbs(self) -> Vec<Verb> {
        match self {
            Scope::Backlog => vec![Verb::Groom, Verb::Approve, Verb::Archive],
            Scope::Open => vec![Verb::Show, Verb::Why],
            _ => Vec::new(),
        }
    }
}

/// The verb list a scope exposes *given the focused item's status*. This is
/// the item-state-conditional logic, kept pure so it is unit-testable: for
/// the Open scope, `request approval` is appended only when the focused
/// item is a Draft (it routes drafts to the advisor queue). All other
/// scopes ignore `focused_status` and return their static [`Scope::verbs`].
/// trace:STORY-690 | ai:claude
pub fn verb_list_for(scope: Scope, focused_status: Option<&str>) -> Vec<Verb> {
    let mut verbs = scope.verbs();
    if scope == Scope::Open {
        let focused_is_draft = focused_status
            .map(|s| s.eq_ignore_ascii_case("draft"))
            .unwrap_or(false);
        if focused_is_draft {
            verbs.push(Verb::RequestApproval);
            // `approve` is the advisor's DIRECT draft → approved transition,
            // the Draft-conditional set-level counterpart to `request
            // approval` (route-to-advisor vs do-it-when-you-ARE-the-advisor).
            // Ordered after `request approval` so the existing draft-verb
            // index navigation is undisturbed. trace:TASK-920 | ai:claude
            verbs.push(Verb::Approve);
        }
        // `queue` is the Approved-conditional mirror of `request approval`:
        // it routes the focused/selected Approved specs to the implementer
        // queue, closing the lifecycle loop. trace:TASK-915 | ai:claude
        let focused_is_approved = focused_status
            .map(|s| s.eq_ignore_ascii_case("approved"))
            .unwrap_or(false);
        if focused_is_approved {
            verbs.push(Verb::Queue);
        }
        // `defer` is NOT status-conditional — it parks ANY open spec off the
        // active view with a revisit trigger, so it is appended for every
        // Open-scope focus (drafts, approved, or no focus). Ordered last so the
        // existing draft/approved verb indices are undisturbed. trace:TASK-921
        verbs.push(Verb::Defer);
    }
    verbs
}

/// A verb (leaf action) applied to the current target selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Groom,
    /// Open scope, Draft-only: the advisor's DIRECT approve — run the
    /// `aida edit <id> --status approved` transition (advisor-gated, so the
    /// spawned command carries advisor authority) on the selected drafts.
    /// The do-it-yourself counterpart to [`Verb::RequestApproval`].
    /// trace:TASK-920
    Approve,
    Archive,
    /// Open scope: `aida show <id> --no-git` on the focused item → modal.
    Show,
    /// Open scope: `aida why <id>` on the focused item → modal.
    Why,
    /// Open scope, Draft-only: route the selected drafts to the advisor
    /// queue via `aida queue add --for advisor`. trace:STORY-690
    RequestApproval,
    /// Open scope, Approved-only: route the selected Approved specs to the
    /// implementer queue via `aida queue add --for implementer`. The mirror
    /// of [`Verb::RequestApproval`]. trace:TASK-915
    Queue,
    /// Open scope, any open spec (NOT status-conditional): park the selected
    /// specs off the active view with a revisit trigger via
    /// `aida defer <id> --until "<trigger>"`. Set-level; the trigger is
    /// captured by a single-line input modal before execution. trace:TASK-921
    Defer,
}

impl Verb {
    pub fn label(self) -> &'static str {
        match self {
            Verb::Groom => "groom",
            Verb::Approve => "approve",
            Verb::Archive => "archive",
            Verb::Show => "show",
            Verb::Why => "why",
            Verb::RequestApproval => "request approval",
            Verb::Queue => "queue",
            Verb::Defer => "defer",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Verb::Groom => "cross-spec grooming + disposition",
            Verb::Approve => "advisor-only: draft → approved",
            Verb::Archive => "mark non-core specs archived",
            Verb::Show => "show this spec (aida show --no-git)",
            Verb::Why => "why is this spec still open? (aida why)",
            Verb::RequestApproval => "route selected drafts to the advisor queue",
            Verb::Queue => "route selected Approved specs to the implementer queue",
            Verb::Defer => "park selected specs off the active view with a revisit trigger",
        }
    }

    /// Does this verb operate on the single focused item (N=1), rather than
    /// the multi-select target set? `show` / `why` are item-level; they
    /// open a modal on the focused row. `request approval` is set-level.
    /// trace:STORY-690 | ai:claude
    pub fn is_item_level(self) -> bool {
        matches!(self, Verb::Show | Verb::Why)
    }

    /// Is this verb wired to do real work? `groom` (stubbed), `show`, `why`,
    /// `request approval`, `queue`, `approve`, and `defer` execute; `archive`
    /// is still present-but-stubbed to prove the verb list + breadcrumb.
    /// trace:TASK-920 trace:TASK-921
    pub fn is_functional(self) -> bool {
        matches!(
            self,
            Verb::Groom
                | Verb::Approve
                | Verb::Show
                | Verb::Why
                | Verb::RequestApproval
                | Verb::Queue
                | Verb::Defer
        )
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

/// A modal showing the captured stdout of a one-shot item verb (`show` /
/// `why`). The title is the breadcrumb-style header; `body` is the raw
/// command output. trace:STORY-690 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbModal {
    pub title: String,
    pub body: String,
}

/// A pending single-line text-input modal for the `defer` verb's revisit
/// trigger. Holds the typed-so-far `buffer` and the `targets` the trigger
/// will apply to (captured at the moment `defer` was run, before the input
/// opened). Enter confirms (runs the defer on `targets` with `buffer`); Esc
/// cancels. Kept pure (push_char / backspace / take) so it is unit-testable
/// without a terminal. trace:TASK-921 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferInput {
    /// The revisit-trigger text typed so far.
    pub buffer: String,
    /// The spec ids the trigger will be applied to on confirm.
    pub targets: Vec<String>,
}

impl DeferInput {
    /// Open a fresh input over the given target ids with an empty buffer.
    pub fn new(targets: Vec<String>) -> Self {
        DeferInput {
            buffer: String::new(),
            targets,
        }
    }

    /// Append a typed char to the trigger buffer.
    pub fn push_char(&mut self, c: char) {
        self.buffer.push(c);
    }

    /// Backspace the trigger buffer (no-op when empty).
    pub fn backspace(&mut self) {
        self.buffer.pop();
    }

    /// The trigger to use on confirm: the typed buffer, or — when the operator
    /// confirmed without typing anything — a sensible default so the defer
    /// still records a revisit condition rather than an empty string.
    pub fn trigger(&self) -> String {
        let t = self.buffer.trim();
        if t.is_empty() {
            "revisit later".to_string()
        } else {
            t.to_string()
        }
    }
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
    /// Vertical scroll offset (in rendered lines) for the open item modal, so
    /// a body taller than the popup can be paged with Up/Down/PageUp/PageDown.
    /// Reset to 0 whenever a modal opens or closes. trace:TASK-913 | ai:claude
    pub modal_scroll: u16,
    /// Verb-output modal content, if any — the captured stdout of a deliberate
    /// one-shot item verb (`show` / `why`) plus a title. Distinct from
    /// [`Self::modal`] (which previews an item's cached body); this carries
    /// command output. trace:STORY-690 | ai:claude
    pub verb_modal: Option<VerbModal>,
    /// Pending "apply to all?" confirmation, if any.
    pub confirm: Option<ConfirmAll>,
    /// Pending revisit-trigger input for the `defer` verb, if open. Holds the
    /// typed buffer + the target ids captured when `defer` was run.
    /// trace:TASK-921 | ai:claude
    pub defer_input: Option<DeferInput>,
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
            modal_scroll: 0,
            verb_modal: None,
            confirm: None,
            defer_input: None,
            role: role.into(),
            status: None,
            theme: crate::theme::Theme::default(),
        }
    }

    /// Replace the target set (e.g. when the highlighted scope changes from
    /// Backlog to Open). Resets the selection and the bottom cursor; clears
    /// any open modal that pointed into the old set. trace:STORY-690
    pub fn set_items(&mut self, items: Vec<TargetItem>) {
        self.selected = vec![false; items.len()];
        self.items = items;
        self.bottom_idx = 0;
        self.modal = None;
        self.modal_scroll = 0;
        self.verb_modal = None;
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

    // --- Focused item + conditional verbs --------------------------------

    /// The bottom-panel item under the cursor, if any (filter-aware). The
    /// item-state-conditional verb list keys off this item's status.
    /// trace:STORY-690 | ai:claude
    pub fn focused_item(&self) -> Option<&TargetItem> {
        let idxs = self.bottom_indices();
        idxs.get(self.bottom_idx).and_then(|&i| self.items.get(i))
    }

    /// The verb list the drilled-into scope currently exposes, accounting
    /// for the focused item's status (item-state-conditional verbs). At the
    /// scope level this is empty. Every verb-list accessor routes through
    /// this so the rendered list, the breadcrumb, and execution agree.
    /// trace:STORY-690 | ai:claude
    pub fn current_verbs(&self) -> Vec<Verb> {
        let Some(scope) = self.scope else {
            return Vec::new();
        };
        let focused_status = self.focused_item().map(|i| i.status.as_str());
        verb_list_for(scope, focused_status)
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
            Level::Verbs => self.current_verbs().len(),
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
                .current_verbs()
                .get(i)
                .map(|v| v.label().to_string())
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
        let verbs = self.current_verbs();
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

    /// The ids of the currently-selected items that are Draft, with the ids
    /// of any selected non-drafts that were skipped. If nothing is selected,
    /// the focused item stands in (the N=1 default) when it is itself a
    /// Draft. Used by `request approval`, which only routes drafts.
    /// Returns `(draft_ids, skipped_non_draft_ids)`. trace:STORY-690
    pub fn draft_selection(&self) -> (Vec<String>, Vec<String>) {
        let selected: Vec<&TargetItem> = self
            .items
            .iter()
            .zip(self.selected.iter())
            .filter(|(_, &s)| s)
            .map(|(item, _)| item)
            .collect();
        let targets: Vec<&TargetItem> = if selected.is_empty() {
            // None selected → the focused item is the N=1 default.
            self.focused_item().into_iter().collect()
        } else {
            selected
        };
        let mut drafts = Vec::new();
        let mut skipped = Vec::new();
        for item in targets {
            if item.is_draft() {
                drafts.push(item.id.clone());
            } else {
                skipped.push(item.id.clone());
            }
        }
        (drafts, skipped)
    }

    /// The ids of the currently-selected items that are Approved, with the
    /// ids of any selected non-Approved that were skipped. If nothing is
    /// selected, the focused item stands in (the N=1 default) when it is
    /// itself Approved. The mirror of [`Self::draft_selection`], used by the
    /// `queue` verb, which only routes Approved specs.
    /// Returns `(approved_ids, skipped_non_approved_ids)`. trace:TASK-915
    pub fn approved_selection(&self) -> (Vec<String>, Vec<String>) {
        let selected: Vec<&TargetItem> = self
            .items
            .iter()
            .zip(self.selected.iter())
            .filter(|(_, &s)| s)
            .map(|(item, _)| item)
            .collect();
        let targets: Vec<&TargetItem> = if selected.is_empty() {
            // None selected → the focused item is the N=1 default.
            self.focused_item().into_iter().collect()
        } else {
            selected
        };
        let mut approved = Vec::new();
        let mut skipped = Vec::new();
        for item in targets {
            if item.is_approved() {
                approved.push(item.id.clone());
            } else {
                skipped.push(item.id.clone());
            }
        }
        (approved, skipped)
    }

    /// The ids `defer` will target: the marked selection, or — when nothing is
    /// selected — the focused item (the N=1 default). Unlike
    /// [`Self::draft_selection`] / [`Self::approved_selection`], `defer` is NOT
    /// status-conditional, so every target is kept (no skip set). trace:TASK-921
    pub fn defer_selection(&self) -> Vec<String> {
        let selected = self.selected_ids();
        if !selected.is_empty() {
            return selected;
        }
        // None selected → the focused item is the N=1 default.
        self.focused_item()
            .map(|i| i.id.clone())
            .into_iter()
            .collect()
    }

    // --- Defer input modal ------------------------------------------------

    /// Open the revisit-trigger input modal over the given target ids. The
    /// parent calls this when `defer` is run; the buffer starts empty and the
    /// operator types the `--until` trigger. trace:TASK-921 | ai:claude
    pub fn open_defer_input(&mut self, targets: Vec<String>) {
        self.defer_input = Some(DeferInput::new(targets));
    }

    /// Is the defer-trigger input modal open? trace:TASK-921
    pub fn defer_input_open(&self) -> bool {
        self.defer_input.is_some()
    }

    /// Append a char to the open defer-trigger buffer (no-op when closed).
    pub fn push_defer_char(&mut self, c: char) {
        if let Some(di) = &mut self.defer_input {
            di.push_char(c);
        }
    }

    /// Backspace the open defer-trigger buffer (no-op when closed).
    pub fn pop_defer_char(&mut self) {
        if let Some(di) = &mut self.defer_input {
            di.backspace();
        }
    }

    /// Cancel the defer input (Esc) — discards the buffer and targets.
    pub fn cancel_defer_input(&mut self) {
        self.defer_input = None;
    }

    /// Confirm the defer input (Enter) — take the pending input out and return
    /// the `(targets, trigger)` for the parent to execute, closing the modal.
    /// `None` when no input is open. trace:TASK-921 | ai:claude
    pub fn take_defer_input(&mut self) -> Option<(Vec<String>, String)> {
        self.defer_input
            .take()
            .map(|di| (di.targets.clone(), di.trigger()))
    }

    /// Enter on a verb → decide what IO the parent should perform.
    ///
    /// Three shapes:
    ///   * item-level (`show` / `why`) → operate on the FOCUSED item (N=1),
    ///     result lands in the modal;
    ///   * `request approval` → operate on the marked drafts (skipping
    ///     non-drafts), or the focused item if nothing is selected;
    ///   * everything else (`groom`, …) → operate on the multi-select, with
    ///     none-selected raising the "apply to all N?" confirmation.
    ///
    /// The pure machine only decides; the parent shells out. trace:STORY-690
    pub fn run_verb(&mut self) -> RunOutcome {
        let Some(verb) = self.top_verb() else {
            return RunOutcome::None;
        };
        if !verb.is_functional() {
            self.status = Some(format!("{} is not wired yet (Slice 1)", verb.label()));
            return RunOutcome::None;
        }

        // Item-level verbs target the focused item, regardless of selection.
        if verb.is_item_level() {
            let Some(item) = self.focused_item() else {
                self.status = Some("no item focused".to_string());
                return RunOutcome::None;
            };
            return RunOutcome::ShowItem {
                verb,
                id: item.id.clone(),
            };
        }

        // request approval: route the marked drafts (or focused draft).
        if verb == Verb::RequestApproval {
            let (drafts, skipped) = self.draft_selection();
            return RunOutcome::RequestApproval { drafts, skipped };
        }

        // approve: directly approve the marked drafts (or focused draft),
        // skipping non-drafts. Same Draft selection as `request approval`,
        // but runs the advisor-gated approved-status transition rather than
        // routing. trace:TASK-920
        if verb == Verb::Approve {
            let (drafts, skipped) = self.draft_selection();
            return RunOutcome::Approve { drafts, skipped };
        }

        // queue: route the marked Approved specs (or focused Approved item)
        // to the implementer queue. trace:TASK-915
        if verb == Verb::Queue {
            let (approved, skipped) = self.approved_selection();
            return RunOutcome::Queue { approved, skipped };
        }

        // defer: park the marked specs (or focused item) — but first capture
        // the revisit trigger. The pure machine only decides WHO to defer; the
        // parent opens the input modal and shells out on confirm. Any open spec
        // qualifies (not status-conditional). trace:TASK-921
        if verb == Verb::Defer {
            let ids = self.defer_selection();
            return RunOutcome::OpenDeferInput { ids };
        }

        // Set-level verbs (groom, …) operate on the selection.
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
            self.modal_scroll = 0;
        }
    }

    /// Open the item modal independent of which panel has focus — used by the
    /// Open scope's `show` verb (run from top focus), whose loaded spec lives
    /// in the parent module's `loaded_spec`, not in `self.items`. The stored
    /// index is a sentinel ("a modal is open"); the parent renders from the
    /// loaded spec, not from this index. trace:STORY-693 | ai:claude
    pub fn open_modal_external(&mut self) {
        self.modal = Some(usize::MAX);
        self.modal_scroll = 0;
    }

    pub fn close_modal(&mut self) {
        self.modal = None;
        self.modal_scroll = 0;
        self.verb_modal = None;
    }

    /// Scroll the open item modal down by `n` lines. The render clamps the
    /// offset to the body height, so an over-scroll simply pins to the last
    /// page rather than going blank. trace:TASK-913 | ai:claude
    pub fn modal_scroll_down(&mut self, n: u16) {
        self.modal_scroll = self.modal_scroll.saturating_add(n);
    }

    /// Scroll the open item modal up by `n` lines (floored at the top).
    /// trace:TASK-913 | ai:claude
    pub fn modal_scroll_up(&mut self, n: u16) {
        self.modal_scroll = self.modal_scroll.saturating_sub(n);
    }

    /// Show a verb's captured stdout in the modal (`show` / `why` output).
    /// trace:STORY-690 | ai:claude
    pub fn open_verb_modal(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.verb_modal = Some(VerbModal {
            title: title.into(),
            body: body.into(),
        });
    }

    /// Is any modal (item-body or verb-output) open? The key router uses
    /// this to know it should capture close keys. trace:STORY-690
    pub fn modal_open(&self) -> bool {
        self.modal.is_some() || self.verb_modal.is_some()
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
    /// Execute a set-level `verb` on these ids.
    Execute { verb: Verb, ids: Vec<String> },
    /// An item-level verb (`show` / `why`) on the focused item — the parent
    /// shells out and shows the command's stdout in the item modal.
    /// trace:STORY-690
    ShowItem { verb: Verb, id: String },
    /// `request approval` on the Draft selection: route `drafts` to the
    /// advisor queue, report `skipped` non-drafts. trace:STORY-690
    RequestApproval {
        drafts: Vec<String>,
        skipped: Vec<String>,
    },
    /// `approve` on the Draft selection: directly approve `drafts` (the
    /// advisor-gated `aida edit <id> --status approved` transition), report
    /// `skipped` non-drafts. The do-it-yourself mirror of
    /// [`Self::RequestApproval`]. trace:TASK-920
    Approve {
        drafts: Vec<String>,
        skipped: Vec<String>,
    },
    /// `queue` on the Approved selection: route `approved` specs to the
    /// implementer queue, report `skipped` non-Approved. The mirror of
    /// [`Self::RequestApproval`]. trace:TASK-915
    Queue {
        approved: Vec<String>,
        skipped: Vec<String>,
    },
    /// `defer` on the selection: the parent should OPEN the revisit-trigger
    /// input modal over these `ids` (the defer itself runs on Enter, via
    /// [`Self::Defer`]). Two-step because `defer` needs the operator-supplied
    /// `--until` trigger before it can run. trace:TASK-921
    OpenDeferInput { ids: Vec<String> },
    /// Confirmed `defer`: park each id in `ids` with the captured `trigger`
    /// via `aida defer <id> --until "<trigger>"`. Emitted by the parent's
    /// input-modal confirm path, not by `run_verb`. trace:TASK-921
    Defer { ids: Vec<String>, trigger: String },
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
                req_type: "Story".into(),
                status: "Approved".into(),
                priority: "medium".into(),
                body: format!("body of item {}", word_for(i)),
            })
            .collect()
    }

    /// Items for the Open-scope tests: mixed statuses so the Draft-conditional
    /// verb + the draft-selection filtering can be exercised. Index 0 + 2 are
    /// Draft; 1 + 3 are Approved.
    fn open_items() -> Vec<TargetItem> {
        ["Draft", "Approved", "Draft", "Approved"]
            .iter()
            .enumerate()
            .map(|(i, status)| TargetItem {
                id: format!("TASK-{i}"),
                title: format!("open item {i}"),
                req_type: "Task".into(),
                status: (*status).into(),
                priority: "high".into(),
                body: String::new(),
            })
            .collect()
    }

    /// Drill into the Open scope (it sits at index 1 in `Scope::all`).
    fn drill_open(s: &mut RedesignState) {
        s.move_down(); // Backlog → Open
        assert_eq!(s.top_scope(), Some(Scope::Open));
        assert!(s.drill());
        assert_eq!(s.scope, Some(Scope::Open));
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
        // Move highlight onto Queue (index 2 — Backlog, Open, Queue, …).
        s.move_down(); // → Open
        s.move_down(); // → Queue
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
        // `approve` is now functional (trace:TASK-920); `archive` (Backlog
        // verb idx 2) remains the present-but-stubbed verb.
        let mut s = state(3);
        s.drill();
        s.move_down(); // → approve
        s.move_down(); // → archive (stub)
        assert_eq!(s.top_verb(), Some(Verb::Archive));
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
    fn modal_scroll_floors_at_top_and_resets_on_open() {
        // trace:TASK-913
        let mut s = state(3);
        s.drill();
        s.focus_bottom();
        s.modal_scroll = 5;
        s.open_modal(); // opening resets the offset
        assert_eq!(s.modal_scroll, 0);
        s.modal_scroll_up(3); // already at top → floored, no underflow
        assert_eq!(s.modal_scroll, 0);
        s.modal_scroll_down(4);
        assert_eq!(s.modal_scroll, 4);
        s.modal_scroll_up(1);
        assert_eq!(s.modal_scroll, 3);
        s.close_modal(); // closing also resets
        assert_eq!(s.modal_scroll, 0);
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
        // All scopes; move past the end and confirm it clamps.
        for _ in 0..20 {
            s.move_down();
        }
        assert_eq!(s.top_idx, Scope::all().len() - 1);
    }

    // --- Open scope (STORY-690) ------------------------------------------

    #[test]
    fn open_scope_is_functional_and_sits_beside_backlog() {
        assert!(Scope::Open.is_functional());
        assert!(Scope::Backlog.is_functional());
        // Backlog still leads; Open is the second entry.
        assert_eq!(Scope::all()[0], Scope::Backlog);
        assert_eq!(Scope::all()[1], Scope::Open);
    }

    #[test]
    fn open_scope_static_verbs_are_show_and_why() {
        assert_eq!(Scope::Open.verbs(), vec![Verb::Show, Verb::Why]);
    }

    #[test]
    fn verb_list_for_open_adds_request_approval_only_on_draft() {
        // Focused on a Draft → request approval + approve are present (the
        // Draft-conditional verbs); approve is ordered after request approval.
        // `defer` (TASK-921, status-unconditional) is appended last on every
        // Open focus. trace:TASK-920
        assert_eq!(
            verb_list_for(Scope::Open, Some("Draft")),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::RequestApproval,
                Verb::Approve,
                Verb::Defer
            ]
        );
        // Case-insensitive.
        assert_eq!(
            verb_list_for(Scope::Open, Some("draft")),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::RequestApproval,
                Verb::Approve,
                Verb::Defer
            ]
        );
        // Focused on Approved → queue (not request approval); see
        // `verb_list_for_adds_queue_only_on_approved`. trace:TASK-915
        assert_eq!(
            verb_list_for(Scope::Open, Some("Approved")),
            vec![Verb::Show, Verb::Why, Verb::Queue, Verb::Defer]
        );
        // No focused item → show / why / defer.
        assert_eq!(
            verb_list_for(Scope::Open, None),
            vec![Verb::Show, Verb::Why, Verb::Defer]
        );
        // Other scopes ignore the status argument.
        assert_eq!(
            verb_list_for(Scope::Backlog, Some("Draft")),
            vec![Verb::Groom, Verb::Approve, Verb::Archive]
        );
    }

    #[test]
    fn current_verbs_tracks_focused_item_status() {
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        // bottom_idx 0 = TASK-0 (Draft) → request approval + approve + defer.
        assert_eq!(
            s.current_verbs(),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::RequestApproval,
                Verb::Approve,
                Verb::Defer
            ]
        );
        s.move_down(); // → TASK-1 (Approved) → queue + defer (trace:TASK-915)
        assert_eq!(
            s.current_verbs(),
            vec![Verb::Show, Verb::Why, Verb::Queue, Verb::Defer]
        );
        s.move_down(); // → TASK-2 (Draft)
        assert_eq!(
            s.current_verbs(),
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
    fn show_verb_targets_focused_item() {
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.move_down(); // focus TASK-1
        s.focus_top(); // top_verb() = show (idx 0)
        assert_eq!(s.top_verb(), Some(Verb::Show));
        assert_eq!(
            s.run_verb(),
            RunOutcome::ShowItem {
                verb: Verb::Show,
                id: "TASK-1".to_string(),
            }
        );
    }

    #[test]
    fn why_verb_targets_focused_item() {
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.move_down();
        s.move_down(); // focus TASK-2
        s.focus_top();
        s.move_down(); // top_verb() = why (idx 1)
        assert_eq!(s.top_verb(), Some(Verb::Why));
        assert_eq!(
            s.run_verb(),
            RunOutcome::ShowItem {
                verb: Verb::Why,
                id: "TASK-2".to_string(),
            }
        );
    }

    #[test]
    fn request_approval_targets_selected_drafts_skips_non_drafts() {
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.toggle_select(); // TASK-0 (Draft)
        s.move_down();
        s.toggle_select(); // TASK-1 (Approved — should be skipped)
        s.move_down();
        s.toggle_select(); // TASK-2 (Draft)
                           // Focus back to TASK-0 (Draft) so the verb list includes the verb,
                           // then move the top highlight onto `request approval` (idx 2).
        s.focus_top();
        s.move_down();
        s.move_down();
        assert_eq!(s.top_verb(), Some(Verb::RequestApproval));
        assert_eq!(
            s.run_verb(),
            RunOutcome::RequestApproval {
                drafts: vec!["TASK-0".to_string(), "TASK-2".to_string()],
                skipped: vec!["TASK-1".to_string()],
            }
        );
    }

    #[test]
    fn request_approval_with_no_selection_uses_focused_draft() {
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom(); // focus TASK-0 (Draft), nothing selected
        s.focus_top();
        s.move_down();
        s.move_down(); // → request approval
        assert_eq!(s.top_verb(), Some(Verb::RequestApproval));
        assert_eq!(
            s.run_verb(),
            RunOutcome::RequestApproval {
                drafts: vec!["TASK-0".to_string()],
                skipped: vec![],
            }
        );
    }

    #[test]
    fn verb_list_for_adds_queue_only_on_approved() {
        // Focused on an Approved → queue is present (third verb); defer last.
        assert_eq!(
            verb_list_for(Scope::Open, Some("Approved")),
            vec![Verb::Show, Verb::Why, Verb::Queue, Verb::Defer]
        );
        // Case-insensitive.
        assert_eq!(
            verb_list_for(Scope::Open, Some("approved")),
            vec![Verb::Show, Verb::Why, Verb::Queue, Verb::Defer]
        );
        // Focused on a Draft → request approval + approve, not queue.
        assert_eq!(
            verb_list_for(Scope::Open, Some("Draft")),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::RequestApproval,
                Verb::Approve,
                Verb::Defer
            ]
        );
        // No focused item → show / why / defer.
        assert_eq!(
            verb_list_for(Scope::Open, None),
            vec![Verb::Show, Verb::Why, Verb::Defer]
        );
        // Other scopes ignore the status argument.
        assert_eq!(
            verb_list_for(Scope::Backlog, Some("Approved")),
            vec![Verb::Groom, Verb::Approve, Verb::Archive]
        );
    }

    #[test]
    fn queue_targets_selected_approved_skips_non_approved() {
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.move_down();
        s.toggle_select(); // TASK-1 (Approved)
        s.move_down();
        s.toggle_select(); // TASK-2 (Draft — should be skipped)
        s.move_down();
        s.toggle_select(); // TASK-3 (Approved)
                           // Focus back to TASK-1 (Approved) so the verb list includes the
                           // verb, then move the top highlight onto `queue` (idx 2).
        s.focus_bottom();
        s.move_down(); // → TASK-1 (Approved)
        s.focus_top();
        s.move_down();
        s.move_down();
        assert_eq!(s.top_verb(), Some(Verb::Queue));
        assert_eq!(
            s.run_verb(),
            RunOutcome::Queue {
                approved: vec!["TASK-1".to_string(), "TASK-3".to_string()],
                skipped: vec!["TASK-2".to_string()],
            }
        );
    }

    #[test]
    fn queue_with_no_selection_uses_focused_approved() {
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.move_down(); // focus TASK-1 (Approved), nothing selected
        s.focus_top();
        s.move_down();
        s.move_down(); // → queue
        assert_eq!(s.top_verb(), Some(Verb::Queue));
        assert_eq!(
            s.run_verb(),
            RunOutcome::Queue {
                approved: vec!["TASK-1".to_string()],
                skipped: vec![],
            }
        );
    }

    #[test]
    fn approve_targets_selected_drafts_skips_non_drafts() {
        // The mirror of `request_approval_targets_selected_drafts...`, but on
        // the `approve` verb (idx 3 in the Open Draft verb list). trace:TASK-920
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.toggle_select(); // TASK-0 (Draft)
        s.move_down();
        s.toggle_select(); // TASK-1 (Approved — should be skipped)
        s.move_down();
        s.toggle_select(); // TASK-2 (Draft)
                           // Focus back to TASK-0 (Draft) so the verb list includes the verb,
                           // then move the top highlight onto `approve` (idx 3).
        s.focus_bottom();
        s.move_up();
        s.move_up(); // → TASK-0 (Draft)
        s.focus_top();
        s.move_down();
        s.move_down();
        s.move_down();
        assert_eq!(s.top_verb(), Some(Verb::Approve));
        assert_eq!(
            s.run_verb(),
            RunOutcome::Approve {
                drafts: vec!["TASK-0".to_string(), "TASK-2".to_string()],
                skipped: vec!["TASK-1".to_string()],
            }
        );
    }

    #[test]
    fn approve_with_no_selection_uses_focused_draft() {
        // trace:TASK-920
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom(); // focus TASK-0 (Draft), nothing selected
        s.focus_top();
        s.move_down();
        s.move_down();
        s.move_down(); // → approve (idx 3)
        assert_eq!(s.top_verb(), Some(Verb::Approve));
        assert_eq!(
            s.run_verb(),
            RunOutcome::Approve {
                drafts: vec!["TASK-0".to_string()],
                skipped: vec![],
            }
        );
    }

    #[test]
    fn approve_absent_on_non_draft_focus() {
        // `approve` is Draft-conditional: an Approved focus exposes `queue`,
        // not `approve`. trace:TASK-920
        assert!(!verb_list_for(Scope::Open, Some("Approved")).contains(&Verb::Approve));
        assert!(!verb_list_for(Scope::Open, None).contains(&Verb::Approve));
        assert!(verb_list_for(Scope::Open, Some("Draft")).contains(&Verb::Approve));
    }

    #[test]
    fn open_show_why_modal_round_trips() {
        let mut s = RedesignState::new(open_items(), "advisor");
        assert!(!s.modal_open());
        s.open_verb_modal("TASK-0 — show", "spec body output");
        assert!(s.modal_open());
        assert_eq!(
            s.verb_modal,
            Some(VerbModal {
                title: "TASK-0 — show".to_string(),
                body: "spec body output".to_string(),
            })
        );
        s.close_modal();
        assert!(!s.modal_open());
        assert!(s.verb_modal.is_none());
    }

    // --- Defer verb (TASK-921) -------------------------------------------

    #[test]
    fn defer_present_on_open_scope_for_any_status() {
        // `defer` is NOT status-conditional — it appears for drafts, approved,
        // and no-focus on the Open scope. trace:TASK-921
        assert!(verb_list_for(Scope::Open, Some("Draft")).contains(&Verb::Defer));
        assert!(verb_list_for(Scope::Open, Some("Approved")).contains(&Verb::Defer));
        assert!(verb_list_for(Scope::Open, None).contains(&Verb::Defer));
        // It is the LAST verb (appended after the status-conditional ones), so
        // the existing draft/approved indices are undisturbed.
        let drafts = verb_list_for(Scope::Open, Some("Draft"));
        assert_eq!(drafts.last(), Some(&Verb::Defer));
        // Other scopes do not expose defer.
        assert!(!verb_list_for(Scope::Backlog, Some("Draft")).contains(&Verb::Defer));
    }

    #[test]
    fn defer_is_functional_set_level() {
        assert!(Verb::Defer.is_functional());
        assert!(!Verb::Defer.is_item_level());
    }

    #[test]
    fn defer_selection_uses_marked_specs() {
        // defer targets the marked selection regardless of status (any open
        // spec qualifies). trace:TASK-921
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.toggle_select(); // TASK-0 (Draft)
        s.move_down();
        s.move_down();
        s.toggle_select(); // TASK-2 (Draft)
        assert_eq!(s.defer_selection(), vec!["TASK-0", "TASK-2"]);
    }

    #[test]
    fn defer_selection_falls_back_to_focused_item() {
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.move_down(); // focus TASK-1, nothing selected
        assert_eq!(s.defer_selection(), vec!["TASK-1"]);
    }

    #[test]
    fn run_defer_opens_input_over_targets() {
        // Enter on the `defer` verb yields OpenDeferInput over the selection,
        // NOT an immediate Defer — the trigger must be captured first.
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.toggle_select(); // TASK-0
        s.focus_top();
        // Move the top highlight onto `defer` (last verb on the Open Draft
        // list: show, why, request approval, approve, defer → idx 4).
        for _ in 0..4 {
            s.move_down();
        }
        assert_eq!(s.top_verb(), Some(Verb::Defer));
        assert_eq!(
            s.run_verb(),
            RunOutcome::OpenDeferInput {
                ids: vec!["TASK-0".to_string()],
            }
        );
    }

    #[test]
    fn defer_input_push_backspace_take() {
        // The pure input buffer: type, backspace, and take out (targets +
        // trigger), closing the modal. trace:TASK-921
        let mut s = RedesignState::new(open_items(), "advisor");
        assert!(!s.defer_input_open());
        s.open_defer_input(vec!["TASK-0".to_string(), "TASK-1".to_string()]);
        assert!(s.defer_input_open());
        s.push_defer_char('w');
        s.push_defer_char('e');
        s.push_defer_char('n');
        s.push_defer_char('x'); // typo
        s.pop_defer_char(); // backspace the typo
        s.push_defer_char(' ');
        s.push_defer_char('Y');
        // buffer is "wen Y"
        assert_eq!(s.defer_input.as_ref().unwrap().buffer, "wen Y");
        let taken = s.take_defer_input();
        assert_eq!(
            taken,
            Some((
                vec!["TASK-0".to_string(), "TASK-1".to_string()],
                "wen Y".to_string()
            ))
        );
        // Taking closes the modal.
        assert!(!s.defer_input_open());
        assert!(s.take_defer_input().is_none());
    }

    #[test]
    fn defer_input_empty_trigger_uses_default() {
        // Confirming with an empty / whitespace-only buffer still records a
        // sensible default trigger rather than an empty string. trace:TASK-921
        let mut di = DeferInput::new(vec!["TASK-0".to_string()]);
        assert_eq!(di.trigger(), "revisit later");
        di.push_char(' ');
        di.push_char(' ');
        assert_eq!(di.trigger(), "revisit later");
        di.push_char('q');
        // Trigger trims surrounding whitespace.
        assert_eq!(di.trigger(), "q");
    }

    #[test]
    fn defer_input_cancel_discards() {
        let mut s = RedesignState::new(open_items(), "advisor");
        s.open_defer_input(vec!["TASK-0".to_string()]);
        s.push_defer_char('x');
        s.cancel_defer_input();
        assert!(!s.defer_input_open());
        assert!(s.take_defer_input().is_none());
    }
}
