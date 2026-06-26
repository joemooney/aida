//! Blocked/waiting board — the TUI flow-cockpit home view (STORY-686).
//!
//! Re-orients the `aida tui` home from a status browser to a flow cockpit:
//! one board that answers "why isn't this moving, who must act, how do I
//! unblock it." Each open spec is grouped by the **reason** it is not
//! progressing; selecting a reason fills the list pane with its items;
//! Enter dispatches that reason's lightest unblock action.
//!
//! The board is a *read-cockpit-that-dispatches*: it composes only
//! cache-fast `aida` reads (never `aida status`, ~3.75s — BUG-616), and
//! when the operator acts it launches the relevant `aida` subcommand
//! rather than reimplementing approve / questions / triage inside the TUI.
//!
//! Precedence (so each spec lands in exactly ONE group): in_flight >
//! blocked > needs-attention > awaiting-review > needs-answer >
//! needs-approval > deferred.
//!
//! trace:STORY-686 | ai:claude

use crate::dashboard::{parse_list_json, ListRow, RowKind};
use std::process::Command;
use std::time::Duration;

/// One of the seven reason-groups the cockpit surfaces. Ordered by the
/// precedence used to assign a spec to exactly one group (highest first):
/// a spec matching several flags belongs to the first reason it matches.
/// trace:STORY-686 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A live lease holds the spec — work is moving, shown for awareness.
    InFlight,
    /// A `BlockedBy` edge points at an incomplete spec — waiting on a dep.
    Blocked,
    /// Parked in `NeedsAttention` (a punt) — an implementer must triage.
    NeedsAttention,
    /// Done on a branch with an open PR — a reviewer must act.
    AwaitingReview,
    /// A pending `DecisionRequest` — a human must answer.
    NeedsAnswer,
    /// A `Draft` spec — you / the advisor must approve (or reject).
    NeedsApproval,
    /// Parked on the deferred shelf — returns on its revisit trigger.
    Deferred,
}

impl Reason {
    /// Precedence-ordered list, highest priority first. The classifier
    /// walks this order so each spec lands in exactly one group.
    pub fn all() -> [Reason; 7] {
        [
            Reason::InFlight,
            Reason::Blocked,
            Reason::NeedsAttention,
            Reason::AwaitingReview,
            Reason::NeedsAnswer,
            Reason::NeedsApproval,
            Reason::Deferred,
        ]
    }

    /// Short label shown in the Nav pane (before the `(count) · owner`).
    pub fn label(self) -> &'static str {
        match self {
            Reason::InFlight => "in flight",
            Reason::Blocked => "blocked by dep",
            Reason::NeedsAttention => "needs attention",
            Reason::AwaitingReview => "awaiting review",
            Reason::NeedsAnswer => "needs an answer",
            Reason::NeedsApproval => "needs approval",
            Reason::Deferred => "deferred",
        }
    }

    /// Who must act on this reason — shown after the count in the Nav.
    pub fn owner(self) -> &'static str {
        match self {
            Reason::InFlight => "impl",
            Reason::Blocked => "wait",
            Reason::NeedsAttention => "impl",
            Reason::AwaitingReview => "reviewer",
            Reason::NeedsAnswer => "you",
            Reason::NeedsApproval => "you",
            Reason::Deferred => "trigger",
        }
    }

    /// The [`RowKind`] every row in this group carries, so the launcher's
    /// Enter handler can pick the right dispatch Intent.
    pub fn row_kind(self) -> RowKind {
        match self {
            Reason::InFlight => RowKind::ReasonInFlight,
            Reason::Blocked => RowKind::ReasonBlocked,
            Reason::NeedsAttention => RowKind::ReasonNeedsAttention,
            Reason::AwaitingReview => RowKind::ReasonAwaitingReview,
            Reason::NeedsAnswer => RowKind::ReasonNeedsAnswer,
            Reason::NeedsApproval => RowKind::ReasonNeedsApproval,
            Reason::Deferred => RowKind::ReasonDeferred,
        }
    }
}

/// A spec the classifier has assigned to exactly one reason-group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedItem {
    pub spec_id: String,
    pub title: String,
    pub status: String,
    pub reason: Reason,
}

/// The cheap (non-network) inputs the classifier needs. Each Vec is the
/// raw `aida list … --json` row set for its status filter; the `--blocked`
/// set carries the `blocked` / `in_flight` flags. Kept as a struct so the
/// pure classifier is unit-testable from fixtures without shelling out.
/// trace:STORY-686 | ai:claude
#[derive(Debug, Clone, Default)]
pub struct BoardInputs {
    /// `aida list --json --blocked`: every open spec, carrying the
    /// `blocked` / `in_flight` / `queued` routing flags. Source for the
    /// in-flight and blocked reasons.
    pub all_rows: Vec<crate::dashboard::ListJsonRow>,
    /// `aida list --status draft --json`: drafts awaiting approval.
    pub draft_rows: Vec<crate::dashboard::ListJsonRow>,
    /// `aida list --status needsattention --json`: punted specs.
    pub needs_attention_rows: Vec<crate::dashboard::ListJsonRow>,
    /// `aida list --status done --json`: Done-on-branch, awaiting merge.
    pub done_rows: Vec<crate::dashboard::ListJsonRow>,
    /// `aida list --deferred --json`: the primed shelf.
    pub deferred_rows: Vec<crate::dashboard::ListJsonRow>,
    /// Spec ids with a pending `DecisionRequest` (parsed from
    /// `aida questions list`).
    pub pending_question_ids: Vec<String>,
}

/// Classify every input row into exactly one reason-group, honoring the
/// precedence in [`Reason::all`]. A spec id is claimed by the highest
/// reason it matches and never double-counted. Returns the items grouped
/// per reason, preserving each source's row order.
///
/// Precedence is enforced by tracking which ids are already claimed: the
/// in-flight pass runs first, then blocked, …, then deferred, and each
/// later pass skips ids an earlier (higher-priority) pass took.
/// trace:STORY-686 | ai:claude
pub fn classify(inputs: &BoardInputs) -> Vec<ClassifiedItem> {
    use std::collections::HashSet;
    let mut claimed: HashSet<String> = HashSet::new();
    let mut out: Vec<ClassifiedItem> = Vec::new();

    let mut take =
        |id: &str, title: &str, status: &str, reason: Reason, claimed: &mut HashSet<String>| {
            if claimed.insert(id.to_string()) {
                out.push(ClassifiedItem {
                    spec_id: id.to_string(),
                    title: title.to_string(),
                    status: status.to_string(),
                    reason,
                });
            }
        };

    // 1. in flight — from the --blocked enrichment's in_flight flag.
    for r in inputs.all_rows.iter().filter(|r| r.in_flight) {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::InFlight,
            &mut claimed,
        );
    }
    // 2. blocked by dependency — the --blocked enrichment's blocked flag.
    for r in inputs.all_rows.iter().filter(|r| r.blocked) {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::Blocked,
            &mut claimed,
        );
    }
    // 3. needs attention — the NeedsAttention status query.
    for r in &inputs.needs_attention_rows {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::NeedsAttention,
            &mut claimed,
        );
    }
    // 4. awaiting review — Done-on-branch (PR rows lazy-fill on top later).
    for r in &inputs.done_rows {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::AwaitingReview,
            &mut claimed,
        );
    }
    // 5. needs an answer — specs with a pending DecisionRequest. Title is
    //    looked up from the all-rows set when present; falls back to the id.
    for id in &inputs.pending_question_ids {
        let (title, status) = inputs
            .all_rows
            .iter()
            .find(|r| &r.spec_id == id)
            .map(|r| (r.title.clone(), r.status.clone()))
            .unwrap_or_else(|| (String::new(), "Needs input".to_string()));
        take(id, &title, &status, Reason::NeedsAnswer, &mut claimed);
    }
    // 6. needs approval — drafts.
    for r in &inputs.draft_rows {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::NeedsApproval,
            &mut claimed,
        );
    }
    // 7. deferred — the primed shelf.
    for r in &inputs.deferred_rows {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::Deferred,
            &mut claimed,
        );
    }

    out
}

/// Count the items per reason in a classified set — drives the Nav-label
/// `(count)` and the empty-reason hide. trace:STORY-686 | ai:claude
pub fn counts(items: &[ClassifiedItem]) -> std::collections::HashMap<&'static str, usize> {
    let mut m = std::collections::HashMap::new();
    for it in items {
        *m.entry(it.reason.label()).or_insert(0) += 1;
    }
    m
}

/// Project the classified items for one reason into [`ListRow`]s the
/// dashboard list pane renders. Each row carries the reason's [`RowKind`]
/// so the launcher dispatches the right Enter action.
pub fn rows_for(items: &[ClassifiedItem], reason: Reason) -> Vec<ListRow> {
    items
        .iter()
        .filter(|it| it.reason == reason)
        .map(|it| ListRow {
            id: it.spec_id.clone(),
            title: it.title.clone(),
            status: it.status.clone(),
            kind: reason.row_kind(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Fast-source fetchers (shell-outs). Each composes a single cache-fast
// `aida` read; none ever calls `aida status`. trace:STORY-686 | ai:claude
// ---------------------------------------------------------------------------

/// Run a cache-fast `aida list …` variant and parse the JSON rows. Returns
/// an empty Vec on any failure so the board paints rather than crashing.
fn list_json(args: &[&str]) -> Vec<crate::dashboard::ListJsonRow> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.arg("list");
    cmd.args(args);
    cmd.arg("--json");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => parse_list_json(&o.stdout),
        _ => Vec::new(),
    }
}

/// Gather the cheap (non-network) board inputs by composing the cache-fast
/// `aida list` reads plus the `aida questions list` decision-inbox parse.
/// The `gh pr list` network source is intentionally NOT touched here — it
/// lazy-fills the awaiting-review group separately.
///
/// The six legs are each a separate `aida` process; firing them
/// **concurrently** (one thread per leg) makes the home-view paint latency
/// the *max* of the legs (the `--blocked` graph walk dominates) rather than
/// their sum, keeping it well under the `aida status` scan the board exists
/// to avoid (BUG-616). trace:STORY-686 | ai:claude
pub fn fetch_inputs() -> BoardInputs {
    // Each leg owns its args so it can move into a thread.
    let all = std::thread::spawn(|| list_json(&["--blocked"]));
    let draft = std::thread::spawn(|| list_json(&["--status", "draft"]));
    let needs_attention = std::thread::spawn(|| list_json(&["--status", "needsattention"]));
    let done = std::thread::spawn(|| list_json(&["--status", "done"]));
    let deferred = std::thread::spawn(|| list_json(&["--deferred"]));
    let questions = std::thread::spawn(fetch_pending_question_ids);

    // A panicked leg (join Err) degrades to an empty set — the board still
    // paints its other reasons.
    BoardInputs {
        all_rows: all.join().unwrap_or_default(),
        draft_rows: draft.join().unwrap_or_default(),
        needs_attention_rows: needs_attention.join().unwrap_or_default(),
        done_rows: done.join().unwrap_or_default(),
        deferred_rows: deferred.join().unwrap_or_default(),
        pending_question_ids: questions.join().unwrap_or_default(),
    }
}

/// Parse the spec ids of pending decision requests from
/// `aida questions list`. There is no `--json` for this surface, so we
/// strip ANSI and pull the SPEC-ID out of each `Decision needed: <ID> …`
/// line (rendered by `print_decision_request`). Robust to colour codes and
/// to the empty / answered-only inboxes. trace:STORY-686 | ai:claude
pub fn fetch_pending_question_ids() -> Vec<String> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["questions", "list"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_pending_question_ids(&String::from_utf8_lossy(&out.stdout))
}

/// Pure parse of `aida questions list` stdout → pending spec ids. Splits on
/// the `Decision needed:` marker the pending block prints, then extracts
/// the first SPEC-ID-shaped token after it. Lines outside the Pending block
/// (the `Answered (N)` section uses a different `<ID> title → label` shape
/// with no `Decision needed:` marker) are ignored.
pub fn parse_pending_question_ids(stdout: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in stdout.lines() {
        let clean = strip_ansi(line);
        if let Some(rest) = clean.split("Decision needed:").nth(1) {
            if let Some(id) = first_spec_id(rest) {
                ids.push(id);
            }
        }
    }
    ids
}

/// Strip ANSI SGR escape sequences (`\x1b[…m`) so plain-text parsing isn't
/// foiled by the colourised CLI output.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip until the terminating letter of the CSI sequence.
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract the first `LETTERS-DIGITS[-DIGITS…]` SPEC-ID-shaped token from a
/// fragment. Returns the canonical id (e.g. `BUG-543`, `FR-1-042`).
fn first_spec_id(fragment: &str) -> Option<String> {
    for tok in fragment.split_whitespace() {
        let t = tok.trim();
        if is_spec_id(t) {
            return Some(t.to_string());
        }
    }
    None
}

/// True for a token shaped like a SPEC-ID: an uppercase-letter prefix, a
/// dash, then one or more dash-separated digit groups.
fn is_spec_id(tok: &str) -> bool {
    let Some((prefix, rest)) = tok.split_once('-') else {
        return false;
    };
    if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_uppercase()) {
        return false;
    }
    !rest.is_empty()
        && rest
            .split('-')
            .all(|g| !g.is_empty() && g.chars().all(|c| c.is_ascii_digit()))
}

/// Lazy-fill the awaiting-review group with the open PRs from `gh pr list`
/// (the one ~1s network source). Returns the PR rows so the caller can
/// merge them on top of the Done-on-branch rows already painted. Best
/// effort: any `gh` failure (offline / unauth) yields an empty Vec and the
/// board just shows the Done rows. trace:STORY-686 | ai:claude
pub fn fetch_open_pr_rows() -> Vec<ListRow> {
    let Some(out) = run_gh_pr_list(Duration::from_secs(5)) else {
        return Vec::new();
    };
    crate::dashboard::parse_pr_json_rows(&out)
}

/// Shell `gh pr list --state open --json …` with a wall-clock timeout so an
/// offline / wedged `gh` never stalls the board. Returns the raw stdout
/// bytes on success.
fn run_gh_pr_list(timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = Command::new("gh")
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--json",
            "number,title,headRefName,statusCheckRollup",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut buf = Vec::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_end(&mut buf);
                }
                return Some(buf);
            }
            Ok(Some(_)) => return None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::ListJsonRow;

    fn row(id: &str, status: &str, queued: bool, in_flight: bool, blocked: bool) -> ListJsonRow {
        ListJsonRow {
            spec_id: id.to_string(),
            title: format!("title of {id}"),
            req_type: "story".to_string(),
            status: status.to_string(),
            tags: vec![],
            queued,
            in_flight,
            blocked,
        }
    }

    #[test]
    fn flags_map_to_the_right_group() {
        let inputs = BoardInputs {
            all_rows: vec![
                row("STORY-1", "InProgress", false, true, false),
                row("STORY-2", "Approved", false, false, true),
            ],
            draft_rows: vec![row("STORY-3", "Draft", false, false, false)],
            needs_attention_rows: vec![row("STORY-4", "NeedsAttention", false, false, false)],
            done_rows: vec![row("STORY-5", "Done", false, false, false)],
            deferred_rows: vec![row("STORY-6", "Approved", false, false, false)],
            pending_question_ids: vec!["STORY-7".to_string()],
        };
        let items = classify(&inputs);
        let by_id = |id: &str| items.iter().find(|i| i.spec_id == id).map(|i| i.reason);
        assert_eq!(by_id("STORY-1"), Some(Reason::InFlight));
        assert_eq!(by_id("STORY-2"), Some(Reason::Blocked));
        assert_eq!(by_id("STORY-3"), Some(Reason::NeedsApproval));
        assert_eq!(by_id("STORY-4"), Some(Reason::NeedsAttention));
        assert_eq!(by_id("STORY-5"), Some(Reason::AwaitingReview));
        assert_eq!(by_id("STORY-6"), Some(Reason::Deferred));
        assert_eq!(by_id("STORY-7"), Some(Reason::NeedsAnswer));
    }

    #[test]
    fn precedence_in_flight_beats_blocked() {
        // A spec that is both in_flight AND blocked lands in in_flight only.
        let inputs = BoardInputs {
            all_rows: vec![row("STORY-1", "InProgress", false, true, true)],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        assert_eq!(items.len(), 1, "no double-count");
        assert_eq!(items[0].reason, Reason::InFlight);
    }

    #[test]
    fn precedence_needs_attention_beats_question_and_draft() {
        // Same spec id appears in NeedsAttention, pending-questions, and the
        // (hypothetical) draft set — NeedsAttention (higher) wins, once.
        let inputs = BoardInputs {
            needs_attention_rows: vec![row("BUG-9", "NeedsAttention", false, false, false)],
            pending_question_ids: vec!["BUG-9".to_string()],
            draft_rows: vec![row("BUG-9", "Draft", false, false, false)],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].reason, Reason::NeedsAttention);
    }

    #[test]
    fn counts_match_grouping() {
        let inputs = BoardInputs {
            draft_rows: vec![
                row("A-1", "Draft", false, false, false),
                row("A-2", "Draft", false, false, false),
            ],
            deferred_rows: vec![row("D-1", "Approved", false, false, false)],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        let c = counts(&items);
        assert_eq!(c.get(Reason::NeedsApproval.label()), Some(&2));
        assert_eq!(c.get(Reason::Deferred.label()), Some(&1));
        assert_eq!(c.get(Reason::Blocked.label()), None);
    }

    #[test]
    fn empty_reason_yields_no_rows() {
        let items = classify(&BoardInputs::default());
        assert!(items.is_empty());
        for reason in Reason::all() {
            assert!(rows_for(&items, reason).is_empty());
        }
    }

    #[test]
    fn rows_for_carries_reason_row_kind() {
        let inputs = BoardInputs {
            draft_rows: vec![row("STORY-3", "Draft", false, false, false)],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        let rows = rows_for(&items, Reason::NeedsApproval);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "STORY-3");
        assert_eq!(rows[0].kind, RowKind::ReasonNeedsApproval);
    }

    #[test]
    fn parse_pending_questions_extracts_spec_ids() {
        // Mirrors the `print_decision_request` pending-block shape, with a
        // colour-coded `Decision needed:` marker; the Answered block (a
        // different shape, no marker) must be ignored.
        let stdout = "\
Pending decisions (2)

\u{1b}[1;33mDecision needed:\u{1b}[0m \u{1b}[36mBUG-543\u{1b}[0m  Epics don't auto-complete
  Some question text
    1. choice — consequence

Decision needed: FR-1-042  Another fork
  q2

Answered (1)
  TASK-798       Config coherence → Unify under [contained]
";
        let ids = parse_pending_question_ids(stdout);
        assert_eq!(ids, vec!["BUG-543".to_string(), "FR-1-042".to_string()]);
    }

    #[test]
    fn parse_pending_questions_empty_inbox() {
        assert!(
            parse_pending_question_ids("Decision inbox empty — no questions recorded.").is_empty()
        );
        assert!(parse_pending_question_ids("").is_empty());
        // Answered-only inbox: no `Decision needed:` markers.
        let answered_only = "Answered (1)\n  TASK-798  foo → bar\n";
        assert!(parse_pending_question_ids(answered_only).is_empty());
    }

    #[test]
    fn is_spec_id_classifies() {
        assert!(is_spec_id("BUG-543"));
        assert!(is_spec_id("FR-1-042"));
        assert!(is_spec_id("STORY-686"));
        assert!(!is_spec_id("lowercase-1"));
        assert!(!is_spec_id("NODASH"));
        assert!(!is_spec_id("BUG-"));
        assert!(!is_spec_id("BUG-x"));
        assert!(!is_spec_id("title"));
    }

    #[test]
    fn strip_ansi_removes_sgr() {
        assert_eq!(strip_ansi("\u{1b}[1;33mhi\u{1b}[0m"), "hi");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn all_reasons_have_distinct_row_kinds() {
        let kinds: Vec<RowKind> = Reason::all().iter().map(|r| r.row_kind()).collect();
        for (i, a) in kinds.iter().enumerate() {
            for b in kinds.iter().skip(i + 1) {
                assert_ne!(a, b, "row kinds must be distinct per reason");
            }
        }
    }
}
