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
    /// Does this spec's description carry a `## Test Plan` section? Populated
    /// only for the [`Scope::Test`] item set (the in-process load checks the
    /// description); `false` everywhere else. Drives the per-row "has a test
    /// plan" marker in the Test scope. trace:STORY-699 | ai:claude
    pub has_test_plan: bool,
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

    /// Is this item in the Done state? The mirror of [`Self::is_approved`] for
    /// the `accept` verb — the reviewer's implementation-approval, which only
    /// applies to finished-on-a-branch (Done) work. Matched case-insensitively.
    /// trace:TASK-933 | ai:claude
    pub fn is_done(&self) -> bool {
        self.status.eq_ignore_ascii_case("done")
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
    /// The shipped-work-to-verify scope (STORY-699): Done + Completed specs in
    /// the active focus, whose `## Test Plan` sections the preview surfaces.
    Test,
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
            Scope::Test,
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
            Scope::Test => "Test",
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
            Scope::Test => "shipped specs to verify (Done + Completed)",
            Scope::Queue => "your routed work",
            Scope::Prs => "open pull requests",
            Scope::History => "completed specs",
            Scope::Findings => "triage items",
            Scope::Sessions => "recorded conversations",
        }
    }

    /// The fuller help text shown in the '?' help popup when this scope is
    /// highlighted — expands [`Self::hint`] into a sentence describing what the
    /// scope shows (and whether it is wired yet). trace:TASK-922 | ai:claude
    pub fn help(self) -> &'static str {
        match self {
            Scope::Backlog => {
                "Shows approved + planned specs — the groomed work waiting to be \
                 picked up. ↵ descends to the items; → opens the verbs to act on \
                 the set: groom, approve, or archive."
            }
            Scope::Open => {
                "Shows the whole open backlog — every unfinished spec regardless of \
                 status. ↵ descends to the items; → opens the per-item verbs (show, \
                 why) plus the status-conditional ones (request approval / approve \
                 for drafts, queue for approved) and defer."
            }
            Scope::Test => {
                "Shows the shipped specs in focus — Done + Completed work ready to \
                 verify. ↵ descends to the items; → opens the verbs, or press p on a \
                 row to open its ## Test Plan (the do→expect steps) in the preview \
                 modal; rows carrying a test plan are marked, and the full \
                 description shows when a spec has none."
            }
            Scope::Queue => "Shows the work routed to your role. Not wired yet (Slice 1).",
            Scope::Prs => "Shows the open pull requests. Not wired yet (Slice 1).",
            Scope::History => "Shows completed specs. Not wired yet (Slice 1).",
            Scope::Findings => "Shows triage items (findings). Not wired yet (Slice 1).",
            Scope::Sessions => "Shows recorded conversations. Not wired yet (Slice 1).",
        }
    }

    /// Is this scope wired for real? Backlog, Open, and Test all drill.
    pub fn is_functional(self) -> bool {
        matches!(self, Scope::Backlog | Scope::Open | Scope::Test)
    }

    /// The *static* verbs this scope exposes — those that do not depend on
    /// the focused item's state. For the Open scope this is the always-on
    /// pair (`show` / `why`); item-state-conditional verbs (`request
    /// approval`, only for Draft specs) are layered on by
    /// [`verb_list_for`]. Slice 1 hardcodes the sets (the §5 "lean registry"
    /// fork is deferred). trace:STORY-690 | ai:claude
    pub fn verbs(self) -> Vec<Verb> {
        match self {
            Scope::Backlog => vec![Verb::Groom, Verb::Approve, Verb::Reject, Verb::Archive],
            Scope::Open => vec![Verb::Show, Verb::Why, Verb::Status],
            // Test scope: `show` previews the focused spec's ## Test Plan in the
            // modal (the same gesture as `p` on a row). trace:STORY-699
            Scope::Test => vec![Verb::Show],
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
            // `reject` is the advisor's DIRECT draft → rejected disposition, the
            // sibling of `approve` (accept vs decline the same Draft). Same
            // Draft-conditional, set-level shape; ordered immediately after
            // `approve` so the existing draft-verb indices are undisturbed.
            // trace:TASK-949 | ai:claude
            verbs.push(Verb::Reject);
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
        // `accept` is the Done-conditional reviewer counterpart to `approve`:
        // implementation-approval (the reviewer accepting finished work),
        // driving the spec Done → Completed. The symmetric mirror of `queue`
        // one lifecycle stage later. Ordered after `queue` (and before the
        // always-on `defer`) so the existing draft/approved verb indices are
        // undisturbed. trace:TASK-933 | ai:claude
        let focused_is_done = focused_status
            .map(|s| s.eq_ignore_ascii_case("done"))
            .unwrap_or(false);
        if focused_is_done {
            verbs.push(Verb::Accept);
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
    /// Open scope, Draft-only: the advisor's DIRECT reject — run the
    /// `aida edit <id> --status rejected` transition (advisor-gated, so the
    /// spawned command carries advisor authority) on the selected drafts. The
    /// sibling of [`Verb::Approve`]: accept vs decline the same Draft.
    // trace:TASK-949
    Reject,
    Archive,
    /// Open scope: `aida show <id> --no-git` on the focused item → modal.
    Show,
    /// Open scope: `aida why <id>` on the focused item → modal.
    Why,
    /// Open scope: `aida status <id>` on the focused item → modal. The per-spec
    /// LIVE work-state lens — queued / In-Progress / live ● / STALE ⚠ plus the
    /// backing session / pid / started / elapsed. Reuses the STORY-694 per-spec
    /// liveness probe wholesale (shells out to `aida status <spec>`); distinct
    /// from `show` (content) and `why` (still-open reason). An item-level read
    /// verb, so role-agnostic (any role).
    // trace:TASK-953 | ai:claude
    Status,
    /// Open scope, Draft-only: route the selected drafts to the advisor
    /// queue via `aida queue add --for advisor`. trace:STORY-690
    RequestApproval,
    /// Open scope, Approved-only: route the selected Approved specs to the
    /// implementer queue via `aida queue add --for implementer`. The mirror
    /// of [`Verb::RequestApproval`]. trace:TASK-915
    Queue,
    /// Open scope, Done-only: the reviewer's IMPLEMENTATION-approval — accept
    /// the finished work on the selected Done specs, driving them Done →
    /// Completed (`aida edit <id> --status completed`, run with reviewer
    /// authority) and recording a reviewer-acceptance comment. The Done-status
    /// counterpart to [`Verb::Approve`] (which is SPEC-approval, Draft →
    /// Approved). Distinct from `approve`. trace:TASK-933
    Accept,
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
            Verb::Reject => "reject",
            Verb::Archive => "archive",
            Verb::Show => "show",
            Verb::Why => "why",
            Verb::Status => "status",
            Verb::RequestApproval => "request approval",
            Verb::Queue => "queue",
            Verb::Accept => "accept",
            Verb::Defer => "defer",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Verb::Groom => "cross-spec grooming + disposition",
            Verb::Approve => "advisor-only: draft → approved",
            Verb::Reject => "advisor-only: draft → rejected",
            Verb::Archive => "mark non-core specs archived",
            Verb::Show => "show this spec (aida show --no-git)",
            Verb::Why => "why is this spec still open? (aida why)",
            Verb::Status => "live work-state: queued / in-progress / live / STALE (aida status)",
            Verb::RequestApproval => "route selected drafts to the advisor queue",
            Verb::Queue => "route selected Approved specs to the implementer queue",
            Verb::Accept => "reviewer: accept finished work (Done → Completed)",
            Verb::Defer => "park selected specs off the active view with a revisit trigger",
        }
    }

    /// The fuller help text shown in the '?' help popup when this verb is
    /// highlighted — what it does plus what set it operates on (item-level vs
    /// set-level) and any status condition. Expands [`Self::hint`].
    /// trace:TASK-922 | ai:claude
    pub fn help(self) -> &'static str {
        match self {
            Verb::Groom => {
                "Cross-spec grooming and routine disposition of the backlog. \
                 Set-level: runs over the selected specs, or all of them when \
                 nothing is selected (confirms first)."
            }
            Verb::Approve => {
                "Advisor's direct draft → approved transition. Draft-only: \
                 set-level over the selected drafts (non-drafts are skipped), or \
                 the focused draft when nothing is selected."
            }
            Verb::Reject => {
                "Advisor's direct draft → rejected transition — the sibling of \
                 approve (decline rather than accept the Draft). Draft-only: \
                 set-level over the selected drafts (non-drafts are skipped), or \
                 the focused draft when nothing is selected."
            }
            Verb::Archive => {
                "Mark non-core specs archived (hidden from default views, audit \
                 trail kept). Set-level. Not wired yet (Slice 1)."
            }
            Verb::Show => {
                "Show this spec's details (aida show --no-git) in a modal. \
                 Item-level: acts on the single focused row."
            }
            Verb::Why => {
                "Explain why this spec is still open (aida why) in a modal. \
                 Item-level: acts on the single focused row."
            }
            Verb::Status => {
                "Show this spec's LIVE work-state (aida status) in a modal: is it \
                 queued (and where), leased/In-Progress, backed by a live session \
                 (pid/started/elapsed), or STALE. Distinct from show (content) and \
                 why (still-open reason). Item-level: acts on the single focused row."
            }
            Verb::RequestApproval => {
                "Route the selected drafts to the advisor queue for review. \
                 Draft-only, set-level (non-drafts skipped); falls back to the \
                 focused draft when nothing is selected."
            }
            Verb::Queue => {
                "Route the selected Approved specs to the implementer queue. \
                 Approved-only, set-level (non-approved skipped); falls back to \
                 the focused approved spec when nothing is selected."
            }
            Verb::Accept => {
                "Reviewer's implementation-approval: accept the finished work \
                 and drive the spec Done → Completed, recording a reviewer \
                 acceptance comment. Done-only, set-level (non-Done skipped); \
                 falls back to the focused Done spec when nothing is selected. \
                 Note: in the full flow Completed is merge-driven — this is the \
                 reviewer's accept for the walkthrough."
            }
            Verb::Defer => {
                "Park the selected specs off the active view with a revisit \
                 trigger (aida defer --until). Any open spec qualifies (not \
                 status-conditional), set-level; falls back to the focused item."
            }
        }
    }

    /// Does this verb operate on the single focused item (N=1), rather than
    /// the multi-select target set? `show` / `why` are item-level; they
    /// open a modal on the focused row. `request approval` is set-level.
    /// trace:STORY-690 | ai:claude
    pub fn is_item_level(self) -> bool {
        matches!(self, Verb::Show | Verb::Why | Verb::Status)
    }

    /// Is this verb wired to do real work? `groom` (stubbed), `show`, `why`,
    /// `request approval`, `queue`, `approve`, `reject`, `accept`, and `defer`
    /// execute; `archive` is still present-but-stubbed to prove the verb list +
    /// breadcrumb.
    // trace:TASK-920 trace:TASK-921 trace:TASK-933 trace:TASK-949
    pub fn is_functional(self) -> bool {
        matches!(
            self,
            Verb::Groom
                | Verb::Approve
                | Verb::Reject
                | Verb::Show
                | Verb::Why
                | Verb::Status
                | Verb::RequestApproval
                | Verb::Queue
                | Verb::Accept
                | Verb::Defer
        )
    }

    /// The role lens that owns this verb's underlying lifecycle act — the THIRD
    /// grey-out axis on top of status-applicability and selection. The redesign
    /// palette gates by role so an operator is never offered a verb the
    /// substrate would refuse for their role:
    ///
    /// - `Some("advisor")` — the advisor-authority dispositions (`groom`,
    ///   `approve`, `reject`, `queue`, `archive`). The substrate gates the
    ///   underlying transitions to advisor authority (the Draft/NeedsAttention →
    ///   Approved/Rejected gate, and the advisor-gated enqueue, TASK-647), so
    ///   these stay advisor-only.
    /// - `Some("reviewer")` — `accept`, the reviewer's implementation-approval
    ///   (Done → Completed).
    /// - `None` — the role-agnostic verbs (`show`, `why`, `status`, `request
    ///   approval`, `defer`): any role may run them (`request approval` is open to any role
    ///   post-BUG-631; reads and parking are unrestricted).
    ///
    /// Permission itself is decided by [`role_permits_verb`] (advisor is the
    /// senior superset). Display-only verbs that aren't permitted render greyed
    /// + non-selectable with a "requires the &lt;role&gt; role" hint.
    // trace:BUG-638 | ai:claude
    pub fn required_role(self) -> Option<&'static str> {
        match self {
            Verb::Groom | Verb::Approve | Verb::Reject | Verb::Queue | Verb::Archive => {
                Some("advisor")
            }
            Verb::Accept => Some("reviewer"),
            Verb::Show | Verb::Why | Verb::Status | Verb::RequestApproval | Verb::Defer => None,
        }
    }
}

/// Canonicalize a role name for the palette's role gate: trim + lowercase, and
/// fold the deprecated `dialog` alias onto `advisor` (mirrors the CLI's
/// `canonical_role`). Comparisons are case-insensitive so `Advisor` matches
/// `advisor`.
// trace:BUG-638 | ai:claude
fn canonical_role_name(role: &str) -> String {
    let r = role.trim().to_ascii_lowercase();
    if r == "dialog" {
        "advisor".to_string()
    } else {
        r
    }
}

/// Whether `active_role` may run a verb whose [`Verb::required_role`] is
/// `required`. `None` required → any role. Otherwise the active role must equal
/// the requirement, with `advisor` as the senior superset: the advisor may run
/// any role's verbs (the substrate never refuses an advisor for these acts, so
/// the palette must not be stricter than the substrate). A non-advisor role is
/// refused any verb it is not the named owner of. trace below.
// trace:BUG-638 | ai:claude
pub fn role_permits_verb(active_role: &str, required: Option<&str>) -> bool {
    match required {
        None => true,
        Some(req) => {
            let active = canonical_role_name(active_role);
            active == canonical_role_name(req) || active == "advisor"
        }
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

/// A pending single-line text-input modal for the `new` action — the title of
/// a fresh Draft spec. Holds only the typed-so-far `buffer` (no targets, unlike
/// [`DeferInput`]). Enter confirms (creates the Draft from the trimmed title);
/// Esc cancels; an empty/whitespace title cancels without creating. Kept pure
/// (push_char / backspace / title) so it is unit-testable without a terminal.
/// trace:TASK-931 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewSpecInput {
    /// The spec title typed so far.
    pub buffer: String,
}

impl NewSpecInput {
    /// Open a fresh input with an empty buffer.
    pub fn new() -> Self {
        NewSpecInput {
            buffer: String::new(),
        }
    }

    /// Append a typed char to the title buffer.
    pub fn push_char(&mut self, c: char) {
        self.buffer.push(c);
    }

    /// Backspace the title buffer (no-op when empty).
    pub fn backspace(&mut self) {
        self.buffer.pop();
    }

    /// The trimmed title to create with, or `None` when nothing meaningful was
    /// typed — so the confirm path cancels rather than creating an empty-titled
    /// draft. trace:TASK-931 | ai:claude
    pub fn title(&self) -> Option<String> {
        let t = self.buffer.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    }
}

/// The braille spinner frames cycled while a verb runs in the background.
/// trace:BUG-633 | ai:claude
pub const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The DISPLAY half of a background verb execution (BUG-633): the label shown
/// while it runs (e.g. `approving TASK-930…`) plus the current spinner frame
/// index. Pure (no IO, no channel) so the spinner cycling + status-line
/// rendering are unit-testable; the parent module pairs this with the
/// completion channel in its own integration shim. trace:BUG-633 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOp {
    /// The human label of the in-flight verb (no spinner glyph).
    pub label: String,
    /// The current spinner frame index (mod `SPINNER_FRAMES.len()`).
    pub frame: usize,
}

impl PendingOp {
    /// Start a pending op at the first spinner frame.
    pub fn new(label: impl Into<String>) -> Self {
        PendingOp {
            label: label.into(),
            frame: 0,
        }
    }

    /// Advance the spinner one frame (wrapping). Called once per idle event-loop
    /// tick while the op is in flight. trace:BUG-633 | ai:claude
    pub fn tick(&mut self) {
        self.frame = (self.frame + 1) % SPINNER_FRAMES.len();
    }

    /// The current spinner glyph.
    pub fn spinner(&self) -> char {
        SPINNER_FRAMES[self.frame % SPINNER_FRAMES.len()]
    }

    /// The status-line text while pending: spinner glyph + label.
    pub fn status_line(&self) -> String {
        format!("{} {}", self.spinner(), self.label)
    }
}

/// One selectable row in the EPIC focus picker (STORY-697): an open epic's
/// display id + title (+ its status, carried for an optional progress hint).
/// The picker fuzzy-filters over `id` + `title`. trace:STORY-697 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicRow {
    pub id: String,
    pub title: String,
    pub status: String,
}

/// The fuzzy EPIC picker modal (STORY-697): replaces the old type-an-id focus
/// input with a selectable, fuzzy-filterable LIST of open epics. The operator
/// opens it with `F`; navigates with ↑/↓; types to fuzzy-filter (reusing
/// [`crate::cmd_palette::fuzzy_score`]); Enter focuses the highlighted epic;
/// Esc cancels. Kept PURE (filter / navigate / select are IO-free) so the
/// selection logic is unit-tested without a terminal. trace:STORY-697 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicPicker {
    /// The full open-epic list (fetched once when the picker opens).
    pub epics: Vec<EpicRow>,
    /// The fuzzy-filter buffer typed so far.
    pub filter: String,
    /// The highlighted row index — an index into the *filtered* list (the
    /// result of [`Self::filtered_indices`]), not the full `epics` vec.
    pub selected: usize,
}

impl EpicPicker {
    /// Open a fresh picker over `epics`, highlighting the first row.
    pub fn new(epics: Vec<EpicRow>) -> Self {
        EpicPicker {
            epics,
            filter: String::new(),
            selected: 0,
        }
    }

    /// The indices (into `epics`) that survive the current fuzzy filter, in
    /// display order. An empty filter passes every row. Matches against each
    /// epic's `"<id> <title>"` so typing a known id narrows to it too.
    /// trace:STORY-697 | ai:claude
    pub fn filtered_indices(&self) -> Vec<usize> {
        if self.filter.trim().is_empty() {
            return (0..self.epics.len()).collect();
        }
        (0..self.epics.len())
            .filter(|&i| {
                let e = &self.epics[i];
                let hay = format!("{} {}", e.id, e.title);
                crate::cmd_palette::fuzzy_score(&self.filter, &hay).is_some()
            })
            .collect()
    }

    /// Move the highlight down within the filtered list (saturating at the end).
    pub fn move_down(&mut self) {
        let len = self.filtered_indices().len();
        if len == 0 {
            return;
        }
        if self.selected + 1 < len {
            self.selected += 1;
        }
    }

    /// Move the highlight up within the filtered list (saturating at the top).
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Append a char to the fuzzy filter, clamping the highlight into the
    /// (possibly newly-narrowed) filtered range.
    pub fn push_char(&mut self, c: char) {
        self.filter.push(c);
        self.clamp();
    }

    /// Backspace the fuzzy filter (no-op when empty), re-clamping the highlight.
    pub fn backspace(&mut self) {
        self.filter.pop();
        self.clamp();
    }

    fn clamp(&mut self) {
        let len = self.filtered_indices().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    /// The display id of the highlighted epic, or `None` when the filtered list
    /// is empty (nothing to focus). trace:STORY-697 | ai:claude
    pub fn selected_epic(&self) -> Option<String> {
        let idxs = self.filtered_indices();
        idxs.get(self.selected).map(|&i| self.epics[i].id.clone())
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
    /// Pending new-spec title input for the `new` action, if open. Holds the
    /// typed title buffer; Enter creates a Draft from it, Esc (or an empty
    /// title) cancels. trace:TASK-931 | ai:claude
    pub new_input: Option<NewSpecInput>,
    /// The EPIC focus lens (STORY-695): when `Some`, the whole TUI is scoped to
    /// this epic id + its transitive children. Set from `AIDA_TUI_EPIC` at
    /// launch or by the change-focus key; cleared by the clear-focus key.
    /// Ambient context the status line shows; every scope-list fetch respects
    /// it. trace:STORY-695 | ai:claude
    pub focus_epic: Option<String>,
    /// A short progress summary of the focus set (e.g. "6 done · 2 draft"),
    /// computed by the parent from the filtered set when the focus is set.
    /// `None` when unfocused. trace:STORY-695 | ai:claude
    pub focus_summary: Option<String>,
    /// The open EPIC focus picker, if any (STORY-697): a fuzzy-filterable list
    /// of open epics. Opened by `F`; Enter focuses the highlighted epic, Esc
    /// cancels. Replaces the old type-an-id `FocusInput`. trace:STORY-697 | ai:claude
    pub epic_picker: Option<EpicPicker>,
    /// Is the context-sensitive '?' help popup open? Its content is derived
    /// purely from the current focus via [`Self::help_content`]; the popup
    /// only tracks that it is showing. trace:TASK-922 | ai:claude
    pub help: bool,
    /// Is the explicit `/` FIND mode active? When `false` (NORMAL mode)
    /// printable chars are hotkeys and the top-level list filter is frozen;
    /// `/` enters find mode, where printable chars live-filter the focused
    /// list. Enter confirms (keeps the filter, returns to normal), Esc cancels
    /// (clears the filter, returns to normal). The dedicated modal/picker text
    /// inputs own their own typing and are unaffected by this flag.
    /// trace:TASK-945 | ai:claude
    pub find_mode: bool,
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
            new_input: None,
            focus_epic: None,
            focus_summary: None,
            epic_picker: None,
            help: false,
            find_mode: false,
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

    /// Whether the active role lens ([`Self::role`]) may run `verb` — the
    /// BUG-638 role gate. A verb the role cannot run renders greyed +
    /// non-selectable in the palette and refuses to execute (see
    /// [`Self::run_verb`]). Delegates to [`role_permits_verb`].
    // trace:BUG-638 | ai:claude
    pub fn verb_role_permitted(&self, verb: Verb) -> bool {
        role_permits_verb(&self.role, verb.required_role())
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

    /// Right arrow → OPEN THE VERBS: drill into the highlighted scope's verbs
    /// (only functional scopes drill). Invoked from the scopes panel (Right on
    /// a scope) AND from the items panel at the scope level (Right on an item —
    /// the resulting verb list reflects the focused item's status, because
    /// `bottom_idx` is preserved and [`Self::current_verbs`] keys off the
    /// focused item). Always lands the keyboard on the verbs (top) panel.
    /// Returns `true` if a drill happened. trace:TASK-944 | ai:claude
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

    /// The ids of the currently-selected items that are Done, with the ids of
    /// any selected non-Done that were skipped. If nothing is selected, the
    /// focused item stands in (the N=1 default) when it is itself Done. The
    /// mirror of [`Self::approved_selection`], used by the `accept` verb (the
    /// reviewer's implementation-approval), which only accepts Done specs.
    /// Returns `(done_ids, skipped_non_done_ids)`. trace:TASK-933 | ai:claude
    pub fn done_selection(&self) -> (Vec<String>, Vec<String>) {
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
        let mut done = Vec::new();
        let mut skipped = Vec::new();
        for item in targets {
            if item.is_done() {
                done.push(item.id.clone());
            } else {
                skipped.push(item.id.clone());
            }
        }
        (done, skipped)
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

    // --- New-spec title input modal (TASK-931) ---------------------------

    /// Open the new-spec title input modal with an empty buffer. The parent
    /// calls this when the `new` key is pressed; the operator types the title
    /// and Enter creates a Draft. trace:TASK-931 | ai:claude
    pub fn open_new_input(&mut self) {
        self.new_input = Some(NewSpecInput::new());
    }

    /// Is the new-spec title input modal open? trace:TASK-931
    pub fn new_input_open(&self) -> bool {
        self.new_input.is_some()
    }

    /// Append a char to the open new-spec title buffer (no-op when closed).
    pub fn push_new_char(&mut self, c: char) {
        if let Some(ni) = &mut self.new_input {
            ni.push_char(c);
        }
    }

    /// Backspace the open new-spec title buffer (no-op when closed).
    pub fn pop_new_char(&mut self) {
        if let Some(ni) = &mut self.new_input {
            ni.backspace();
        }
    }

    /// Cancel the new-spec input (Esc) — discards the buffer.
    pub fn cancel_new_input(&mut self) {
        self.new_input = None;
    }

    /// Confirm the new-spec input (Enter) — take the pending input out and
    /// return the trimmed title for the parent to create, closing the modal.
    /// `None` when the buffer is empty/whitespace (cancel — no creation) or no
    /// input is open. trace:TASK-931 | ai:claude
    pub fn take_new_input(&mut self) -> Option<String> {
        self.new_input.take().and_then(|ni| ni.title())
    }

    // --- EPIC focus lens (STORY-695) -------------------------------------

    /// Is an EPIC focus lens active? trace:STORY-695 | ai:claude
    pub fn focused(&self) -> bool {
        self.focus_epic.is_some()
    }

    /// Clear the EPIC focus lens (back to all items). The parent re-fetches the
    /// unfiltered scope set after this. trace:STORY-695 | ai:claude
    pub fn clear_focus(&mut self) {
        self.focus_epic = None;
        self.focus_summary = None;
    }

    // --- EPIC focus picker (STORY-697) -----------------------------------

    /// Open the EPIC focus picker over `epics` (the open-epic list fetched by
    /// the parent from the store). Replaces the old type-an-id focus input with
    /// a fuzzy-filterable selectable list. trace:STORY-697 | ai:claude
    pub fn open_epic_picker(&mut self, epics: Vec<EpicRow>) {
        self.epic_picker = Some(EpicPicker::new(epics));
    }

    /// Is the EPIC picker open? trace:STORY-697 | ai:claude
    pub fn epic_picker_open(&self) -> bool {
        self.epic_picker.is_some()
    }

    /// Move the picker highlight down (no-op when closed). trace:STORY-697
    pub fn picker_move_down(&mut self) {
        if let Some(p) = &mut self.epic_picker {
            p.move_down();
        }
    }

    /// Move the picker highlight up (no-op when closed). trace:STORY-697
    pub fn picker_move_up(&mut self) {
        if let Some(p) = &mut self.epic_picker {
            p.move_up();
        }
    }

    /// Append a char to the picker's fuzzy filter (no-op when closed).
    pub fn push_picker_char(&mut self, c: char) {
        if let Some(p) = &mut self.epic_picker {
            p.push_char(c);
        }
    }

    /// Backspace the picker's fuzzy filter (no-op when closed).
    pub fn pop_picker_char(&mut self) {
        if let Some(p) = &mut self.epic_picker {
            p.backspace();
        }
    }

    /// Cancel the picker (Esc) — closes it, leaves the current focus unchanged.
    /// trace:STORY-697 | ai:claude
    pub fn cancel_epic_picker(&mut self) {
        self.epic_picker = None;
    }

    /// Confirm the picker (Enter) — take the highlighted epic id out and close
    /// the modal. `None` when the filtered list was empty (nothing to focus) or
    /// no picker is open. trace:STORY-697 | ai:claude
    pub fn take_epic_selection(&mut self) -> Option<String> {
        self.epic_picker.take().and_then(|p| p.selected_epic())
    }

    // --- Help popup (TASK-922) -------------------------------------------

    /// Open the context-sensitive '?' help popup. trace:TASK-922 | ai:claude
    pub fn open_help(&mut self) {
        self.help = true;
    }

    /// Close the '?' help popup. trace:TASK-922 | ai:claude
    pub fn close_help(&mut self) {
        self.help = false;
    }

    /// Is the '?' help popup open? trace:TASK-922 | ai:claude
    pub fn help_open(&self) -> bool {
        self.help
    }

    /// The current focus, distilled to the element the '?' help popup should
    /// describe: the highlighted scope, the highlighted verb, or the focused
    /// item. Pure — drives [`Self::help_content`] (and `help_for`).
    /// trace:TASK-922 | ai:claude
    pub fn focus_target(&self) -> FocusTarget {
        match self.focus {
            Focus::Bottom => match self.focused_item() {
                Some(item) => FocusTarget::Item {
                    id: item.id.clone(),
                    status: item.status.clone(),
                },
                None => FocusTarget::ItemsEmpty,
            },
            Focus::Top => match self.level {
                Level::Scopes => match self.top_scope() {
                    Some(scope) => FocusTarget::ScopeEntry(scope),
                    None => FocusTarget::ScopesEmpty,
                },
                Level::Verbs => match (self.scope, self.top_verb()) {
                    (Some(scope), Some(verb)) => FocusTarget::VerbEntry { scope, verb },
                    (Some(scope), None) => FocusTarget::VerbsEmpty(scope),
                    _ => FocusTarget::ScopesEmpty,
                },
            },
        }
    }

    /// The context-sensitive help content for the current focus. Pure wrapper
    /// over [`help_for`] so the parent renders without re-deriving the target.
    /// trace:TASK-922 | ai:claude
    pub fn help_content(&self) -> HelpContent {
        help_for(self.focus_target())
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
        // Role gate (BUG-638): refuse a verb the active role lens may not run —
        // the substrate would reject it (approve/queue/groom/archive are
        // advisor-authority acts; accept is the reviewer's). Mirrors the greyed,
        // non-selectable palette rendering: Enter on a role-disabled verb is a
        // no-op with a helpful status, never a refused subprocess. Checked
        // before `is_functional` so a non-advisor gets the role reason rather
        // than a stub notice. trace:BUG-638 | ai:claude
        if !self.verb_role_permitted(verb) {
            let req = verb.required_role().unwrap_or("advisor");
            self.status = Some(format!("{} requires the {} role", verb.label(), req));
            return RunOutcome::None;
        }
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

        // reject: directly reject the marked drafts (or focused draft),
        // skipping non-drafts. The sibling of `approve` — same Draft selection
        // and advisor gate, but runs the rejected-status transition rather than
        // approved. trace:TASK-949
        if verb == Verb::Reject {
            let (drafts, skipped) = self.draft_selection();
            return RunOutcome::Reject { drafts, skipped };
        }

        // queue: route the marked Approved specs (or focused Approved item)
        // to the implementer queue. trace:TASK-915
        if verb == Verb::Queue {
            let (approved, skipped) = self.approved_selection();
            return RunOutcome::Queue { approved, skipped };
        }

        // accept: the reviewer accepts the finished work on the marked Done
        // specs (or focused Done item), driving them Done → Completed and
        // recording a reviewer-acceptance comment; non-Done are skipped.
        // trace:TASK-933
        if verb == Verb::Accept {
            let (done, skipped) = self.done_selection();
            return RunOutcome::Accept { done, skipped };
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

    // --- Find mode (TASK-945) --------------------------------------------

    /// Is a top-level list filter currently applied (find mode confirmed a
    /// non-empty query)? Drives the Esc layering: a live filter is cleared
    /// before a level is popped. trace:TASK-945 | ai:claude
    pub fn filter_active(&self) -> bool {
        !self.filter.trim().is_empty()
    }

    /// `/` → ENTER find mode. Starts a FRESH query (clears any prior filter)
    /// so the prompt opens empty, vim/less-style. trace:TASK-945 | ai:claude
    pub fn enter_find_mode(&mut self) {
        self.filter.clear();
        self.find_mode = true;
        self.clamp_indices();
    }

    /// Enter in find mode → CONFIRM: keep the typed filter applied and return
    /// to normal mode so hotkeys act on the filtered list. trace:TASK-945
    pub fn confirm_find(&mut self) {
        self.find_mode = false;
    }

    /// Esc in find mode → CANCEL: clear the filter and return to normal mode.
    /// trace:TASK-945 | ai:claude
    pub fn cancel_find(&mut self) {
        self.filter.clear();
        self.find_mode = false;
        self.clamp_indices();
    }

    /// Clear an applied filter (without touching `find_mode`). Used by the
    /// normal-mode Esc layering. trace:TASK-945 | ai:claude
    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.clamp_indices();
    }

    /// Esc semantics for the top-level list in NORMAL mode: if a filter is
    /// applied, clear it and report `true` (handled — consume the Esc);
    /// otherwise report `false` so the caller pops a level. The vim-like
    /// layering — clear the filter before unwinding the stack. trace:TASK-945
    pub fn esc_clears_filter(&mut self) -> bool {
        if self.filter_active() {
            self.clear_filter();
            true
        } else {
            false
        }
    }

    /// Route a printable char by mode: in FIND mode it extends the top-level
    /// filter (and is consumed → `true`); in NORMAL mode it is NOT a filter
    /// keystroke (the filter is untouched → `false`, leaving it to act as a
    /// hotkey). The key router shares this gate. trace:TASK-945 | ai:claude
    pub fn type_char(&mut self, c: char) -> bool {
        if self.find_mode {
            self.push_filter(c);
            true
        } else {
            false
        }
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
    /// `reject` on the Draft selection: directly reject `drafts` (the
    /// advisor-gated `aida edit <id> --status rejected` transition), report
    /// `skipped` non-drafts. The sibling of [`Self::Approve`].
    // trace:TASK-949
    Reject {
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
    /// `accept` on the Done selection: the reviewer accepts the finished work
    /// on `done` (the `aida edit <id> --status completed` transition, run with
    /// reviewer authority, plus a reviewer-acceptance comment), reporting
    /// `skipped` non-Done. The Done-status mirror of [`Self::Approve`].
    /// trace:TASK-933
    Accept {
        done: Vec<String>,
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

// ---------------------------------------------------------------------------
// Context-sensitive '?' help (TASK-922)
// ---------------------------------------------------------------------------

/// A short, human-readable label for the top-level quit key — surfaced in the
/// help popup's key legend so the quit gesture is documented where the user
/// asks for help. trace:TASK-922 | ai:claude
pub const QUIT_KEY_LABEL: &str = "q (or Esc at the top) / Ctrl-C: quit";

/// The EPIC focus-lens key legend entry, surfaced in every '?' help context so
/// the ambient lens is discoverable wherever the operator asks for help.
/// trace:STORY-695 | ai:claude
pub const FOCUS_KEY_LABEL: &str =
    "F: pick an EPIC to focus the whole TUI on (+ its children) · C: clear the focus";

/// The `new` action key legend entry — surfaced in every '?' help context so
/// creating a Draft spec is discoverable wherever the operator asks for help.
/// trace:TASK-931 | ai:claude
pub const NEW_KEY_LABEL: &str = "n: new — create a Draft spec (opens a title input)";

/// The live-refresh key legend entry — surfaced in every '?' help context so
/// re-reading the store (to witness state changes made outside the TUI) is
/// discoverable wherever the operator asks for help. trace:TASK-934 | ai:claude
pub const REFRESH_KEY_LABEL: &str = "r: refresh — re-read the store (pick up external changes)";

/// The element the '?' help popup should describe, distilled from the focus
/// state. A pure value so [`help_for`] is a total, unit-testable function.
/// trace:TASK-922 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusTarget {
    /// Scopes panel, a scope highlighted.
    ScopeEntry(Scope),
    /// Scopes panel, but the (filtered) list is empty.
    ScopesEmpty,
    /// Verbs panel, a verb highlighted (within its scope).
    VerbEntry { scope: Scope, verb: Verb },
    /// Verbs panel within a scope, but the (filtered) list is empty.
    VerbsEmpty(Scope),
    /// Items panel, a spec focused (carry its id + status for the header).
    Item { id: String, status: String },
    /// Items panel, but there is no focused item (empty / filtered-out).
    ItemsEmpty,
}

/// The content of the '?' help popup: a header (where you are / what's
/// selected), the focused element's help body, and a short key legend for the
/// current context. Pure data so the parent render and the unit tests both
/// consume the same thing. trace:TASK-922 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpContent {
    /// "Where you are / what's selected" — the popup title line.
    pub header: String,
    /// The focused element's fuller help text.
    pub body: String,
    /// The keys that work in this context, each `"key: meaning"`. Always
    /// includes the quit-key legend ([`QUIT_KEY_LABEL`]).
    pub legend: Vec<String>,
}

/// Derive the '?' help popup content for a given focus target. PURE and total
/// — every focus shape maps to a header + body + key legend, so the unit
/// tests assert the right help is selected without a terminal. The verb/scope
/// bodies come straight from their `help()` strings (data-driven).
/// trace:TASK-922 | ai:claude
pub fn help_for(target: FocusTarget) -> HelpContent {
    match target {
        FocusTarget::ScopeEntry(scope) => HelpContent {
            header: format!("Scopes › {}", scope.label()),
            body: scope.help().to_string(),
            legend: scope_legend(),
        },
        FocusTarget::ScopesEmpty => HelpContent {
            header: "Scopes".to_string(),
            body: "No scope matches the current filter. Backspace to widen it.".to_string(),
            legend: scope_legend(),
        },
        FocusTarget::VerbEntry { scope, verb } => HelpContent {
            header: format!("{} › {}", scope.label(), verb.label()),
            body: verb.help().to_string(),
            legend: verb_legend(),
        },
        FocusTarget::VerbsEmpty(scope) => HelpContent {
            header: format!("{} › verbs", scope.label()),
            body: "No verb matches the current filter. Backspace to widen it.".to_string(),
            legend: verb_legend(),
        },
        FocusTarget::Item { id, status } => {
            let status_note = if status.is_empty() {
                String::new()
            } else {
                format!(" (status: {status})")
            };
            HelpContent {
                header: format!("Items › {id}{status_note}"),
                body: "Item-level actions for the focused spec: p / ↵ previews it in \
                       a modal. → opens the verbs for this item (show, why, and the \
                       status-conditional ones act on this row). ← / Esc goes back a \
                       level. Space toggles it into the multi-select set."
                    .to_string(),
                legend: item_legend(),
            }
        }
        FocusTarget::ItemsEmpty => HelpContent {
            header: "Items".to_string(),
            body: "No item is focused (the list is empty or filtered out). \
                   Backspace to widen the filter, or ⇧Tab to return to the verbs."
                .to_string(),
            legend: item_legend(),
        },
    }
}

/// The key legend for the scopes panel context. trace:TASK-922 | ai:claude
fn scope_legend() -> Vec<String> {
    vec![
        "↵ / Tab: descend to the items panel".to_string(),
        "→: drill into the highlighted scope's verbs".to_string(),
        "↑/↓: move highlight".to_string(),
        "/: find — filter the list (↵ keep · Esc clear)".to_string(),
        "?: toggle this help".to_string(),
        NEW_KEY_LABEL.to_string(),
        REFRESH_KEY_LABEL.to_string(),
        FOCUS_KEY_LABEL.to_string(),
        QUIT_KEY_LABEL.to_string(),
    ]
}

/// The key legend for the verbs panel context. trace:TASK-922 | ai:claude
fn verb_legend() -> Vec<String> {
    vec![
        "↵: run the highlighted verb".to_string(),
        "← / Esc: back to scopes".to_string(),
        "↑/↓: move highlight".to_string(),
        "Tab: focus the items panel".to_string(),
        "/: find — filter the list (↵ keep · Esc clear)".to_string(),
        "?: toggle this help".to_string(),
        NEW_KEY_LABEL.to_string(),
        REFRESH_KEY_LABEL.to_string(),
        FOCUS_KEY_LABEL.to_string(),
        QUIT_KEY_LABEL.to_string(),
    ]
}

/// The key legend for the items panel context. trace:TASK-922 | ai:claude
fn item_legend() -> Vec<String> {
    vec![
        "Space: toggle-select this item".to_string(),
        "a / A: select all / none".to_string(),
        "→: open the verbs for this item".to_string(),
        "p / ↵: preview this spec".to_string(),
        "← / Esc / ⇧Tab: back a level".to_string(),
        "/: find — filter the items (↵ keep · Esc clear)".to_string(),
        "?: toggle this help".to_string(),
        NEW_KEY_LABEL.to_string(),
        REFRESH_KEY_LABEL.to_string(),
        FOCUS_KEY_LABEL.to_string(),
        QUIT_KEY_LABEL.to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_op_new_starts_at_first_frame() {
        let op = PendingOp::new("approving TASK-930…");
        assert_eq!(op.frame, 0);
        assert_eq!(op.spinner(), SPINNER_FRAMES[0]);
        assert_eq!(op.label, "approving TASK-930…");
    }

    #[test]
    fn pending_op_tick_advances_and_wraps() {
        let mut op = PendingOp::new("queueing…");
        op.tick();
        assert_eq!(op.frame, 1);
        assert_eq!(op.spinner(), SPINNER_FRAMES[1]);
        // Tick through a full cycle: it wraps back to frame 0.
        for _ in 0..(SPINNER_FRAMES.len() - 1) {
            op.tick();
        }
        assert_eq!(op.frame, 0);
        assert_eq!(op.spinner(), SPINNER_FRAMES[0]);
    }

    #[test]
    fn pending_op_status_line_shows_spinner_and_label() {
        let op = PendingOp::new("deferring 2 spec(s)…");
        let line = op.status_line();
        assert!(line.starts_with(SPINNER_FRAMES[0]));
        assert!(line.contains("deferring 2 spec(s)…"));
    }

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
                has_test_plan: false,
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
                has_test_plan: false,
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
        // Move highlight onto Queue (index 3 — Backlog, Open, Test, Queue, …).
        s.move_down(); // → Open
        s.move_down(); // → Test
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
        s.move_down(); // → reject
        assert_eq!(s.breadcrumb(), "Backlog › reject");
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

    // --- Role gate (BUG-638) ---------------------------------------------

    #[test]
    fn required_role_maps_advisor_and_reviewer_verbs() {
        // Advisor-authority dispositions.
        assert_eq!(Verb::Groom.required_role(), Some("advisor"));
        assert_eq!(Verb::Approve.required_role(), Some("advisor"));
        assert_eq!(Verb::Reject.required_role(), Some("advisor"));
        assert_eq!(Verb::Queue.required_role(), Some("advisor"));
        assert_eq!(Verb::Archive.required_role(), Some("advisor"));
        // Reviewer's implementation-approval.
        assert_eq!(Verb::Accept.required_role(), Some("reviewer"));
        // Role-agnostic: any role may run these.
        assert_eq!(Verb::Show.required_role(), None);
        assert_eq!(Verb::Why.required_role(), None);
        assert_eq!(Verb::RequestApproval.required_role(), None);
        assert_eq!(Verb::Defer.required_role(), None);
    }

    #[test]
    fn role_permits_verb_advisor_is_superset_others_strict() {
        // Advisor runs everything (the senior superset).
        assert!(role_permits_verb("advisor", Some("advisor")));
        assert!(role_permits_verb("advisor", Some("reviewer")));
        assert!(role_permits_verb("advisor", None));
        // Implementer: refused advisor + reviewer verbs, allowed role-agnostic.
        assert!(!role_permits_verb("implementer", Some("advisor")));
        assert!(!role_permits_verb("implementer", Some("reviewer")));
        assert!(role_permits_verb("implementer", None));
        // Reviewer: its own verb + agnostic, refused advisor.
        assert!(role_permits_verb("reviewer", Some("reviewer")));
        assert!(!role_permits_verb("reviewer", Some("advisor")));
        assert!(role_permits_verb("reviewer", None));
        // Case-insensitive + the deprecated `dialog` alias folds to advisor.
        assert!(role_permits_verb("Advisor", Some("advisor")));
        assert!(role_permits_verb("dialog", Some("advisor")));
    }

    #[test]
    fn verb_role_permitted_reflects_active_role() {
        let imp = RedesignState::new(items(3), "implementer");
        assert!(!imp.verb_role_permitted(Verb::Approve));
        assert!(!imp.verb_role_permitted(Verb::Reject));
        assert!(!imp.verb_role_permitted(Verb::Groom));
        assert!(!imp.verb_role_permitted(Verb::Accept));
        assert!(imp.verb_role_permitted(Verb::Show));
        assert!(imp.verb_role_permitted(Verb::RequestApproval));
        assert!(imp.verb_role_permitted(Verb::Defer));

        let adv = RedesignState::new(items(3), "advisor");
        assert!(adv.verb_role_permitted(Verb::Approve));
        assert!(adv.verb_role_permitted(Verb::Reject));
        assert!(adv.verb_role_permitted(Verb::Groom));
        // Advisor is the superset: it may also run the reviewer's accept.
        assert!(adv.verb_role_permitted(Verb::Accept));

        let rev = RedesignState::new(items(3), "reviewer");
        assert!(rev.verb_role_permitted(Verb::Accept));
        assert!(!rev.verb_role_permitted(Verb::Approve));
        assert!(!rev.verb_role_permitted(Verb::Reject));
    }

    #[test]
    fn run_verb_refuses_role_disallowed_verb_for_implementer() {
        // Backlog verbs are [groom, approve, reject, archive]; an implementer must
        // not be able to RUN the advisor-only approve even though it renders (greyed).
        let mut s = RedesignState::new(items(3), "implementer");
        s.drill(); // Backlog → verbs
        s.move_down(); // groom → approve
        assert_eq!(s.top_verb(), Some(Verb::Approve));
        let out = s.run_verb();
        assert_eq!(out, RunOutcome::None);
        assert_eq!(
            s.status.as_deref(),
            Some("approve requires the advisor role")
        );
    }

    #[test]
    fn run_verb_allows_role_permitted_verb_for_advisor() {
        // The same approve verb runs for an advisor (no role refusal).
        let mut s = state(3); // advisor
        s.drill();
        s.move_down(); // → approve
        assert_eq!(s.top_verb(), Some(Verb::Approve));
        let out = s.run_verb();
        assert!(matches!(out, RunOutcome::Approve { .. }));
        assert_ne!(
            s.status.as_deref(),
            Some("approve requires the advisor role")
        );
    }

    #[test]
    fn run_verb_allows_request_approval_for_implementer() {
        // The role-agnostic `request approval` is runnable by an implementer:
        // it routes drafts to the advisor queue (open to any role post-BUG-631).
        let mut s = RedesignState::new(open_items(), "implementer");
        drill_open(&mut s); // focus is on the first (Draft) item
                            // Open+Draft verbs: [show, why, status, request approval, approve, defer].
        s.move_down(); // show → why
        s.move_down(); // why → status
        s.move_down(); // status → request approval
        assert_eq!(s.top_verb(), Some(Verb::RequestApproval));
        let out = s.run_verb();
        assert!(matches!(out, RunOutcome::RequestApproval { .. }));
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
        // verb idx 3, after the inserted `reject`) remains the present-but-stubbed verb.
        let mut s = state(3);
        s.drill();
        s.move_down(); // → approve
        s.move_down(); // → reject
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
        // Only "archive" survives (now idx 3 in [groom, approve, reject, archive]).
        assert_eq!(idxs, vec![3]);
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

    // --- Find mode (TASK-945) --------------------------------------------

    #[test]
    fn slash_enters_find_mode_fresh() {
        let mut s = state(3);
        assert!(!s.find_mode, "starts in normal mode");
        // A pre-existing filter is cleared on entry (starts fresh).
        s.filter = "stale".to_string();
        s.enter_find_mode();
        assert!(s.find_mode);
        assert!(s.filter.is_empty(), "find mode opens with a fresh query");
    }

    #[test]
    fn typing_in_find_mode_filters_the_focused_list() {
        let mut s = state(12);
        s.drill();
        s.focus_bottom();
        s.enter_find_mode();
        // Titles are digit-free, so "11" narrows to STORY-11 alone.
        assert!(s.type_char('1'));
        assert!(s.type_char('1'));
        let idxs = s.bottom_indices();
        assert_eq!(idxs.len(), 1);
        assert_eq!(s.items[idxs[0]].id, "STORY-11");
    }

    #[test]
    fn enter_confirms_find_keeping_filter() {
        let mut s = state(3);
        s.drill();
        s.enter_find_mode();
        s.push_filter('a');
        s.push_filter('r');
        s.push_filter('c');
        s.push_filter('h'); // narrows verbs to "archive"
        s.confirm_find();
        assert!(!s.find_mode, "confirm returns to normal mode");
        assert!(s.filter_active(), "confirm KEEPS the filter applied");
        // Hotkeys now act on the filtered list (only "archive" survives).
        assert_eq!(s.top_verb(), Some(Verb::Archive));
    }

    #[test]
    fn esc_in_find_mode_clears_and_exits() {
        let mut s = state(3);
        s.drill();
        s.enter_find_mode();
        s.push_filter('a');
        s.push_filter('r');
        assert!(s.filter_active());
        s.cancel_find();
        assert!(!s.find_mode, "cancel returns to normal mode");
        assert!(!s.filter_active(), "cancel CLEARS the filter");
    }

    #[test]
    fn hotkey_char_in_normal_mode_does_not_filter() {
        let mut s = state(3);
        s.drill();
        assert!(!s.find_mode);
        // In normal mode a printable char is NOT a filter keystroke — it is
        // left to act as a hotkey, and the filter is untouched.
        assert!(!s.type_char('n'));
        assert!(!s.type_char('q'));
        assert!(s.filter.is_empty());
    }

    #[test]
    fn esc_in_normal_mode_clears_filter_before_popping() {
        let mut s = state(3);
        s.drill(); // now at the verb level
        s.enter_find_mode();
        s.push_filter('g');
        s.confirm_find(); // back to normal, filter still applied
        assert!(!s.find_mode);
        assert!(s.filter_active());
        // First Esc clears the filter and stays at the verb level (handled).
        assert!(s.esc_clears_filter());
        assert!(!s.filter_active());
        assert_eq!(s.level, Level::Verbs, "did NOT pop a level yet");
        // Second Esc: no filter to clear → caller is free to pop.
        assert!(!s.esc_clears_filter());
        assert!(s.pop());
        assert_eq!(s.level, Level::Scopes);
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
        assert_eq!(
            Scope::Open.verbs(),
            vec![Verb::Show, Verb::Why, Verb::Status]
        );
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
                Verb::Status,
                Verb::RequestApproval,
                Verb::Approve,
                Verb::Reject,
                Verb::Defer
            ]
        );
        // Case-insensitive.
        assert_eq!(
            verb_list_for(Scope::Open, Some("draft")),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::Status,
                Verb::RequestApproval,
                Verb::Approve,
                Verb::Reject,
                Verb::Defer
            ]
        );
        // Focused on Approved → queue (not request approval); see
        // `verb_list_for_adds_queue_only_on_approved`. trace:TASK-915
        assert_eq!(
            verb_list_for(Scope::Open, Some("Approved")),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::Status,
                Verb::Queue,
                Verb::Defer
            ]
        );
        // No focused item → show / why / defer.
        assert_eq!(
            verb_list_for(Scope::Open, None),
            vec![Verb::Show, Verb::Why, Verb::Status, Verb::Defer]
        );
        // Other scopes ignore the status argument.
        assert_eq!(
            verb_list_for(Scope::Backlog, Some("Draft")),
            vec![Verb::Groom, Verb::Approve, Verb::Reject, Verb::Archive]
        );
    }

    #[test]
    fn current_verbs_tracks_focused_item_status() {
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        // bottom_idx 0 = TASK-0 (Draft) → request approval + approve + reject + defer.
        assert_eq!(
            s.current_verbs(),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::Status,
                Verb::RequestApproval,
                Verb::Approve,
                Verb::Reject,
                Verb::Defer
            ]
        );
        s.move_down(); // → TASK-1 (Approved) → queue + defer (trace:TASK-915)
        assert_eq!(
            s.current_verbs(),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::Status,
                Verb::Queue,
                Verb::Defer
            ]
        );
        s.move_down(); // → TASK-2 (Draft)
        assert_eq!(
            s.current_verbs(),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::Status,
                Verb::RequestApproval,
                Verb::Approve,
                Verb::Reject,
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
    fn status_verb_is_item_level_read_role_agnostic() {
        // `status` is the per-spec live work-state lens — an item-level READ
        // verb (like show / why), so it is role-agnostic (any role) and yields
        // a ShowItem on the focused row. trace:TASK-953
        assert!(Verb::Status.is_item_level());
        assert!(Verb::Status.is_functional());
        assert_eq!(Verb::Status.required_role(), None);
        // Runnable by a non-advisor (implementer): role-agnostic, never gated.
        let mut s = RedesignState::new(open_items(), "implementer");
        drill_open(&mut s);
        s.focus_bottom();
        s.move_down(); // focus TASK-1
        s.focus_top();
        s.move_down();
        s.move_down(); // show → why → status (idx 2)
        assert_eq!(s.top_verb(), Some(Verb::Status));
        assert_eq!(
            s.run_verb(),
            RunOutcome::ShowItem {
                verb: Verb::Status,
                id: "TASK-1".to_string(),
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
            vec![
                Verb::Show,
                Verb::Why,
                Verb::Status,
                Verb::Queue,
                Verb::Defer
            ]
        );
        // Case-insensitive.
        assert_eq!(
            verb_list_for(Scope::Open, Some("approved")),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::Status,
                Verb::Queue,
                Verb::Defer
            ]
        );
        // Focused on a Draft → request approval + approve + reject, not queue.
        assert_eq!(
            verb_list_for(Scope::Open, Some("Draft")),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::Status,
                Verb::RequestApproval,
                Verb::Approve,
                Verb::Reject,
                Verb::Defer
            ]
        );
        // No focused item → show / why / defer.
        assert_eq!(
            verb_list_for(Scope::Open, None),
            vec![Verb::Show, Verb::Why, Verb::Status, Verb::Defer]
        );
        // Other scopes ignore the status argument.
        assert_eq!(
            verb_list_for(Scope::Backlog, Some("Approved")),
            vec![Verb::Groom, Verb::Approve, Verb::Reject, Verb::Archive]
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
        s.move_down();
        s.move_down(); // → approve (idx 4)
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

    // --- Reject verb (TASK-949) ------------------------------------------

    #[test]
    fn reject_targets_selected_drafts_skips_non_drafts() {
        // The sibling of `approve_targets_selected_drafts...`, but on the
        // `reject` verb (idx 5 in the Open Draft verb list). trace:TASK-949
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.toggle_select(); // TASK-0 (Draft)
        s.move_down();
        s.toggle_select(); // TASK-1 (Approved — should be skipped)
        s.move_down();
        s.toggle_select(); // TASK-2 (Draft)
                           // Focus back to TASK-0 (Draft) so the verb list includes the verb,
                           // then move the top highlight onto `reject` (idx 5).
        s.focus_bottom();
        s.move_up();
        s.move_up(); // → TASK-0 (Draft)
        s.focus_top();
        s.move_down();
        s.move_down();
        s.move_down();
        s.move_down();
        s.move_down();
        assert_eq!(s.top_verb(), Some(Verb::Reject));
        assert_eq!(
            s.run_verb(),
            RunOutcome::Reject {
                drafts: vec!["TASK-0".to_string(), "TASK-2".to_string()],
                skipped: vec!["TASK-1".to_string()],
            }
        );
    }

    #[test]
    fn reject_with_no_selection_uses_focused_draft() {
        // trace:TASK-949
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom(); // focus TASK-0 (Draft), nothing selected
        s.focus_top();
        s.move_down();
        s.move_down();
        s.move_down();
        s.move_down();
        s.move_down(); // → reject (idx 5)
        assert_eq!(s.top_verb(), Some(Verb::Reject));
        assert_eq!(
            s.run_verb(),
            RunOutcome::Reject {
                drafts: vec!["TASK-0".to_string()],
                skipped: vec![],
            }
        );
    }

    #[test]
    fn reject_absent_on_non_draft_focus() {
        // `reject` is Draft-conditional, the sibling of `approve`: an Approved
        // focus exposes `queue`, not `reject`. trace:TASK-949
        assert!(!verb_list_for(Scope::Open, Some("Approved")).contains(&Verb::Reject));
        assert!(!verb_list_for(Scope::Open, None).contains(&Verb::Reject));
        assert!(verb_list_for(Scope::Open, Some("Draft")).contains(&Verb::Reject));
    }

    // --- Accept verb (TASK-933) ------------------------------------------

    /// Items for the accept tests: a Done spec mixed with non-Done so the
    /// Done-conditional verb + the done-selection filtering can be exercised.
    /// Index 0 + 2 are Done; 1 is Approved, 3 is Draft. trace:TASK-933
    fn accept_items() -> Vec<TargetItem> {
        ["Done", "Approved", "Done", "Draft"]
            .iter()
            .enumerate()
            .map(|(i, status)| TargetItem {
                id: format!("TASK-{i}"),
                title: format!("open item {i}"),
                req_type: "Task".into(),
                status: (*status).into(),
                priority: "high".into(),
                body: String::new(),
                has_test_plan: false,
            })
            .collect()
    }

    #[test]
    fn verb_list_for_adds_accept_only_on_done() {
        // Focused on a Done → accept is present (after show / why), defer last.
        assert_eq!(
            verb_list_for(Scope::Open, Some("Done")),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::Status,
                Verb::Accept,
                Verb::Defer
            ]
        );
        // Case-insensitive.
        assert_eq!(
            verb_list_for(Scope::Open, Some("done")),
            vec![
                Verb::Show,
                Verb::Why,
                Verb::Status,
                Verb::Accept,
                Verb::Defer
            ]
        );
        // accept is Done-only: Draft / Approved / no-focus do NOT expose it.
        assert!(!verb_list_for(Scope::Open, Some("Draft")).contains(&Verb::Accept));
        assert!(!verb_list_for(Scope::Open, Some("Approved")).contains(&Verb::Accept));
        assert!(!verb_list_for(Scope::Open, None).contains(&Verb::Accept));
        // Other scopes ignore the status argument (no accept).
        assert!(!verb_list_for(Scope::Backlog, Some("Done")).contains(&Verb::Accept));
        // accept is the LAST status-conditional verb (before the always-on
        // defer), so the existing draft/approved indices are undisturbed.
        let done = verb_list_for(Scope::Open, Some("Done"));
        assert_eq!(done.last(), Some(&Verb::Defer));
    }

    #[test]
    fn accept_is_functional_set_level() {
        assert!(Verb::Accept.is_functional());
        assert!(!Verb::Accept.is_item_level());
    }

    #[test]
    fn accept_targets_selected_done_skips_non_done() {
        // The mirror of `queue_targets_selected_approved...`, but on the
        // `accept` verb over Done specs. trace:TASK-933
        let mut s = RedesignState::new(accept_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.toggle_select(); // TASK-0 (Done)
        s.move_down();
        s.toggle_select(); // TASK-1 (Approved — should be skipped)
        s.move_down();
        s.toggle_select(); // TASK-2 (Done)
                           // Focus back to TASK-0 (Done) so the verb list includes the verb,
                           // then move the top highlight onto `accept` (idx 2).
        s.focus_bottom();
        s.move_up();
        s.move_up(); // → TASK-0 (Done)
        s.focus_top();
        s.move_down();
        s.move_down();
        s.move_down();
        assert_eq!(s.top_verb(), Some(Verb::Accept));
        assert_eq!(
            s.run_verb(),
            RunOutcome::Accept {
                done: vec!["TASK-0".to_string(), "TASK-2".to_string()],
                skipped: vec!["TASK-1".to_string()],
            }
        );
    }

    #[test]
    fn accept_with_no_selection_uses_focused_done() {
        // trace:TASK-933
        let mut s = RedesignState::new(accept_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom(); // focus TASK-0 (Done), nothing selected
        s.focus_top();
        s.move_down();
        s.move_down();
        s.move_down(); // → accept (idx 3)
        assert_eq!(s.top_verb(), Some(Verb::Accept));
        assert_eq!(
            s.run_verb(),
            RunOutcome::Accept {
                done: vec!["TASK-0".to_string()],
                skipped: vec![],
            }
        );
    }

    #[test]
    fn done_selection_skips_non_done_targets() {
        // Marking a Done + an Approved spec yields the Done in `done` and the
        // Approved in `skipped`. trace:TASK-933
        let mut s = RedesignState::new(accept_items(), "advisor");
        drill_open(&mut s);
        s.focus_bottom();
        s.toggle_select(); // TASK-0 (Done)
        s.move_down();
        s.toggle_select(); // TASK-1 (Approved)
        let (done, skipped) = s.done_selection();
        assert_eq!(done, vec!["TASK-0"]);
        assert_eq!(skipped, vec!["TASK-1"]);
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
        // list: show, why, status, request approval, approve, reject, defer → idx 6).
        for _ in 0..6 {
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

    // --- New-spec title input (TASK-931) ---------------------------------

    #[test]
    fn new_input_push_backspace_take() {
        // Open, type a title (with an edit), confirm: take returns the trimmed
        // title and closes the modal. trace:TASK-931
        let mut s = RedesignState::new(open_items(), "advisor");
        assert!(!s.new_input_open());
        s.open_new_input();
        assert!(s.new_input_open());
        s.push_new_char('h');
        s.push_new_char('i');
        s.push_new_char('x'); // typo
        s.pop_new_char(); // backspace the typo
        s.push_new_char(' ');
        s.push_new_char('t');
        // buffer is "hi t" (surrounding whitespace is trimmed on take)
        assert_eq!(s.new_input.as_ref().unwrap().buffer, "hi t");
        let taken = s.take_new_input();
        assert_eq!(taken, Some("hi t".to_string()));
        // Taking closes the modal.
        assert!(!s.new_input_open());
        assert!(s.take_new_input().is_none());
    }

    #[test]
    fn new_input_empty_title_cancels() {
        // Confirming with an empty / whitespace-only buffer yields None (cancel
        // — no creation) and still closes the modal. trace:TASK-931
        let mut s = RedesignState::new(open_items(), "advisor");
        s.open_new_input();
        s.push_new_char(' ');
        s.push_new_char(' ');
        assert_eq!(s.take_new_input(), None);
        assert!(!s.new_input_open());
        // A title with surrounding whitespace trims to the inner text.
        let mut ni = NewSpecInput::new();
        ni.push_char(' ');
        ni.push_char('a');
        ni.push_char(' ');
        assert_eq!(ni.title(), Some("a".to_string()));
        // An untouched / whitespace buffer has no title.
        assert_eq!(NewSpecInput::new().title(), None);
    }

    #[test]
    fn new_input_cancel_discards() {
        let mut s = RedesignState::new(open_items(), "advisor");
        s.open_new_input();
        s.push_new_char('x');
        s.cancel_new_input();
        assert!(!s.new_input_open());
        assert!(s.take_new_input().is_none());
    }

    // --- '?' help popup (TASK-922) ---------------------------------------

    #[test]
    fn help_for_verb_returns_that_verbs_help() {
        // Verb-focused → the popup body is that verb's help string, header is
        // the breadcrumb-style "scope › verb", legend documents the quit key.
        let hc = help_for(FocusTarget::VerbEntry {
            scope: Scope::Backlog,
            verb: Verb::Groom,
        });
        assert_eq!(hc.header, "Backlog › groom");
        assert_eq!(hc.body, Verb::Groom.help());
        assert!(hc.body.starts_with("Cross-spec grooming"));
        assert!(hc
            .legend
            .iter()
            .any(|l| l.contains("run the highlighted verb")));
        assert!(hc.legend.iter().any(|l| l == QUIT_KEY_LABEL));
    }

    #[test]
    fn help_for_scope_returns_that_scopes_help() {
        let hc = help_for(FocusTarget::ScopeEntry(Scope::Open));
        assert_eq!(hc.header, "Scopes › Open");
        assert_eq!(hc.body, Scope::Open.help());
        assert!(hc.legend.iter().any(|l| l.contains("drill")));
        assert!(hc.legend.iter().any(|l| l == QUIT_KEY_LABEL));
    }

    #[test]
    fn help_for_item_returns_item_help_with_status() {
        let hc = help_for(FocusTarget::Item {
            id: "TASK-7".to_string(),
            status: "Draft".to_string(),
        });
        assert!(hc.header.contains("TASK-7"));
        assert!(hc.header.contains("Draft"), "header surfaces the status");
        assert!(hc.body.contains("preview"), "item body covers item actions");
        assert!(hc.legend.iter().any(|l| l.contains("toggle-select")));
        assert!(hc.legend.iter().any(|l| l == QUIT_KEY_LABEL));
    }

    #[test]
    fn help_legend_always_documents_the_quit_key() {
        // Every context's legend names the (new) quit key. trace:TASK-922
        for target in [
            FocusTarget::ScopeEntry(Scope::Backlog),
            FocusTarget::ScopesEmpty,
            FocusTarget::VerbEntry {
                scope: Scope::Open,
                verb: Verb::Show,
            },
            FocusTarget::VerbsEmpty(Scope::Open),
            FocusTarget::Item {
                id: "X-1".into(),
                status: String::new(),
            },
            FocusTarget::ItemsEmpty,
        ] {
            let hc = help_for(target.clone());
            assert!(
                hc.legend.iter().any(|l| l == QUIT_KEY_LABEL),
                "legend documents quit for {target:?}"
            );
        }
    }

    #[test]
    fn focus_target_tracks_panel_and_selection() {
        // Scope level, top focus → the highlighted scope.
        let mut s = RedesignState::new(open_items(), "advisor");
        assert_eq!(s.focus_target(), FocusTarget::ScopeEntry(Scope::Backlog));
        // Verb level → the highlighted verb within the drilled scope.
        drill_open(&mut s);
        assert_eq!(
            s.focus_target(),
            FocusTarget::VerbEntry {
                scope: Scope::Open,
                verb: Verb::Show,
            }
        );
        // Bottom focus → the focused item (TASK-0, Draft).
        s.focus_bottom();
        assert_eq!(
            s.focus_target(),
            FocusTarget::Item {
                id: "TASK-0".to_string(),
                status: "Draft".to_string(),
            }
        );
    }

    #[test]
    fn help_content_matches_focus_target() {
        // The state-bound `help_content` agrees with the pure `help_for` for
        // the same focus. trace:TASK-922
        let mut s = RedesignState::new(open_items(), "advisor");
        drill_open(&mut s);
        assert_eq!(s.help_content(), help_for(s.focus_target()));
        assert_eq!(s.help_content().body, Verb::Show.help());
    }

    #[test]
    fn help_open_close_toggles_flag() {
        let mut s = RedesignState::new(open_items(), "advisor");
        assert!(!s.help_open());
        s.open_help();
        assert!(s.help_open());
        s.close_help();
        assert!(!s.help_open());
    }

    // --- EPIC focus lens (STORY-695) -------------------------------------

    #[test]
    fn focus_clear_resets_epic_and_summary() {
        let mut s = state(3);
        s.focus_epic = Some("EPIC-54".to_string());
        s.focus_summary = Some("3 draft".to_string());
        assert!(s.focused());
        s.clear_focus();
        assert!(!s.focused());
        assert!(s.focus_epic.is_none());
        assert!(s.focus_summary.is_none());
    }

    // --- EPIC focus picker (STORY-697) -----------------------------------

    fn epic_rows() -> Vec<EpicRow> {
        vec![
            EpicRow {
                id: "EPIC-54".into(),
                title: "aida tui redesign".into(),
                status: "Approved".into(),
            },
            EpicRow {
                id: "EPIC-26".into(),
                title: "the TUI is the product".into(),
                status: "InProgress".into(),
            },
            EpicRow {
                id: "EPIC-42".into(),
                title: "advisor grooming".into(),
                status: "Planned".into(),
            },
        ]
    }

    #[test]
    fn picker_opens_navigates_and_selects() {
        let mut s = state(3);
        assert!(!s.epic_picker_open());
        s.open_epic_picker(epic_rows());
        assert!(s.epic_picker_open());
        // Highlight starts on the first row.
        assert_eq!(
            s.epic_picker.as_ref().unwrap().selected_epic(),
            Some("EPIC-54".into())
        );
        s.picker_move_down(); // → EPIC-26
        s.picker_move_down(); // → EPIC-42
        s.picker_move_down(); // saturates at the end
        assert_eq!(
            s.epic_picker.as_ref().unwrap().selected_epic(),
            Some("EPIC-42".into())
        );
        s.picker_move_up(); // → EPIC-26
        assert_eq!(s.take_epic_selection(), Some("EPIC-26".into()));
        assert!(!s.epic_picker_open(), "taking closes the modal");
    }

    #[test]
    fn picker_fuzzy_filter_narrows_and_reselects() {
        let mut s = state(3);
        s.open_epic_picker(epic_rows());
        // Move off the first row, then type a filter that excludes the current
        // highlight — the highlight must clamp back into the filtered range.
        s.picker_move_down();
        s.picker_move_down(); // → EPIC-42
                              // "redesign" only matches EPIC-54's title.
        for c in "redesign".chars() {
            s.push_picker_char(c);
        }
        let p = s.epic_picker.as_ref().unwrap();
        assert_eq!(p.filtered_indices().len(), 1);
        assert_eq!(p.selected_epic(), Some("EPIC-54".into()));
        // Typing a known id narrows to it too (fuzzy over "<id> <title>").
        s.pop_picker_char(); // remove enough to widen, then refilter by id
        for _ in 0.."redesig".len() {
            s.pop_picker_char();
        }
        for c in "26".chars() {
            s.push_picker_char(c);
        }
        let p = s.epic_picker.as_ref().unwrap();
        assert_eq!(p.selected_epic(), Some("EPIC-26".into()));
    }

    #[test]
    fn picker_cancel_leaves_focus_unchanged() {
        let mut s = state(3);
        s.focus_epic = Some("EPIC-54".to_string());
        s.open_epic_picker(epic_rows());
        s.push_picker_char('x');
        s.cancel_epic_picker();
        assert!(!s.epic_picker_open());
        // Focus epic is untouched by a cancelled pick.
        assert_eq!(s.focus_epic, Some("EPIC-54".to_string()));
    }

    #[test]
    fn picker_selection_is_none_when_filter_excludes_all() {
        let mut s = state(3);
        s.open_epic_picker(epic_rows());
        for c in "zzzzz".chars() {
            s.push_picker_char(c);
        }
        let p = s.epic_picker.as_ref().unwrap();
        assert!(p.filtered_indices().is_empty());
        assert_eq!(p.selected_epic(), None);
        assert_eq!(s.take_epic_selection(), None);
    }

    // --- Directional navigation (TASK-944) -------------------------------

    #[test]
    fn enter_on_scope_descends_to_items_not_verbs() {
        // NEW model: Enter on a scope DESCENDS to the items panel (focus →
        // Bottom) while the top panel KEEPS showing the scopes (no drill).
        // trace:TASK-944
        let mut s = state(3);
        assert_eq!(s.focus, Focus::Top);
        assert_eq!(s.level, Level::Scopes);
        s.focus_bottom(); // the Enter-on-scope transition
        assert_eq!(s.focus, Focus::Bottom);
        assert_eq!(s.level, Level::Scopes, "top panel still shows scopes");
        assert!(s.scope.is_none(), "no scope was drilled to verbs");
    }

    #[test]
    fn right_on_scope_opens_verbs() {
        // NEW model: Right on a scope OPENS THE VERBS (drill), landing the
        // keyboard on the verbs (top) panel. trace:TASK-944
        let mut s = state(3);
        assert!(s.drill()); // the Right-on-scope transition
        assert_eq!(s.level, Level::Verbs);
        assert_eq!(s.scope, Some(Scope::Backlog));
        assert_eq!(s.focus, Focus::Top);
        assert_eq!(s.top_verb(), Some(Verb::Groom));
    }

    #[test]
    fn right_on_item_opens_verbs_reflecting_focused_item_status() {
        // NEW model: from the items panel (reached by Enter-descend), Right
        // OPENS THE VERBS for the focused item — the verb list reflects the
        // focused item's STATUS. trace:TASK-944
        let mut s = RedesignState::new(open_items(), "advisor");
        s.move_down(); // highlight the Open scope (index 1)
        assert_eq!(s.top_scope(), Some(Scope::Open));
        s.focus_bottom(); // Enter-on-scope descends to the items
        assert_eq!(s.focus, Focus::Bottom);
        // bottom_idx 0 = TASK-0 (Draft).
        assert!(s.drill()); // Right-on-item opens the verbs
        assert_eq!(s.scope, Some(Scope::Open));
        assert_eq!(s.level, Level::Verbs);
        assert_eq!(s.focus, Focus::Top);
        // The verb list reflects the focused Draft item.
        let verbs = s.current_verbs();
        assert!(verbs.contains(&Verb::RequestApproval));
        assert!(verbs.contains(&Verb::Approve));
        assert!(!verbs.contains(&Verb::Queue));

        // Back out, focus an Approved item, re-open the verbs: the list now
        // reflects the Approved status (queue, not request approval).
        assert!(s.pop()); // Left: verbs → scopes
        assert_eq!(s.level, Level::Scopes);
        assert_eq!(
            s.top_scope(),
            Some(Scope::Open),
            "highlight restored to Open"
        );
        s.focus_bottom(); // descend again
        s.move_down(); // → TASK-1 (Approved)
        assert!(s.drill()); // Right-on-item
        let verbs = s.current_verbs();
        assert!(verbs.contains(&Verb::Queue));
        assert!(!verbs.contains(&Verb::RequestApproval));
    }

    #[test]
    fn left_goes_back_a_level_everywhere_without_exiting() {
        // NEW model: Left = pop one level (items → scopes, verbs → scopes);
        // at the top of the stack it returns false (the caller does NOT exit —
        // that stays Esc's job). trace:TASK-944
        let mut s = state(3);
        // Items panel (descended) → back to scopes.
        s.focus_bottom();
        assert_eq!(s.focus, Focus::Bottom);
        assert!(s.pop()); // Left
        assert_eq!(s.focus, Focus::Top);
        assert_eq!(s.level, Level::Scopes);
        // Verbs panel (drilled) → back to scopes.
        assert!(s.drill());
        assert_eq!(s.level, Level::Verbs);
        assert!(s.pop()); // Left
        assert_eq!(s.level, Level::Scopes);
        // Top of the stack: Left (pop) is a no-op that returns false.
        assert!(!s.pop());
        assert_eq!(s.level, Level::Scopes);
    }
}
