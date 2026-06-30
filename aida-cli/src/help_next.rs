//! AXI principle #9 (contextual disclosure): a centralized, table-driven
//! next-step suggestion surface keyed by (command, lifecycle state).
//!
//! gh-axi / tasks-axi end every output with a flat, templated list of next-step
//! commands. AIDA's hints were scattered across skills; this module centralizes
//! them. In AGENT MODE only (`agent_output_mode()`), the list / show / status /
//! queue / add / edit surfaces get a trailing TOON `next` block listing the
//! likely next commands, templated with the relevant spec id.
//!
//! The richness over gh-axi's flat template is AIDA's LIFECYCLE STATE MACHINE:
//! for a spec in a given status we suggest the VALID NEXT TRANSITION
//! (Draft -> approve, Approved -> work, In Progress -> done, Done -> merge, ...)
//! rather than a generic list. The set of valid transitions is read from
//! `aida_core::lifecycle::LifecycleModel::declared()` so the suggestions can
//! never drift from the declared model (and a new declared edge is suggested for
//! free). This module only supplies the concrete CLI string for each declared
//! edge; the model decides which edges exist.
//!
//! The human TTY path is left byte-identical — it already has the scattered
//! workflow hints + the emoji UX; this surface is agent-mode-only.
// trace:TASK-974

use aida_core::lifecycle::{LifecycleModel, State};

/// One suggested next command plus a short token naming the lifecycle target
/// (or the kind of step) it leads to.
pub struct NextStep {
    /// The concrete `aida ...` command, templated with the relevant spec id.
    pub cmd: String,
    /// A short reason / target token (e.g. `approved`, `in-progress`, `detail`).
    pub to: String,
}

impl NextStep {
    fn new(cmd: impl Into<String>, to: impl Into<String>) -> Self {
        NextStep {
            cmd: cmd.into(),
            to: to.into(),
        }
    }
}

/// The stable lowercase token for a lifecycle target state — the `to` column of
/// the `next` block, so the agent sees WHICH state a suggestion advances to.
fn state_token(s: State) -> &'static str {
    match s {
        State::Start => "start",
        State::Draft => "draft",
        State::Approved => "approved",
        State::Planned => "planned",
        State::InProgress => "in-progress",
        State::Done => "done",
        State::Completed => "completed",
        State::Released => "released",
        State::Rejected => "rejected",
        State::NeedsAttention => "needs-attention",
    }
}

/// Presentation rank for a target state, so the most-likely mainline-forward
/// transition lists first and the off-ramps (reject, archive) last. Lower sorts
/// earlier. Stable sort preserves declared order within a rank.
fn rank(to: State) -> u8 {
    match to {
        // Mainline-forward moves: the overwhelmingly common next step.
        State::InProgress | State::Done | State::Completed => 0,
        State::Approved => 1,
        State::Planned => 2,
        // Off-ramps.
        State::Rejected => 8,
        _ => 5,
    }
}

/// The concrete CLI command that drives a declared `from -> to` transition for
/// spec `id`, or `None` when the edge is not a user-driven *forward suggestion*:
///   * `Done -> Completed` is the merge auto-bump — suggested as `aida pull`,
///     not spec-targeted (the merge of the PR is what completes it).
///   * `Done -> InProgress` is reviewer-driven (RequestChanges), not something a
///     human should be nudged to do by hand.
///   * `Completed -> Released` is a repo-level release act, not a per-spec verb.
fn transition_command(from: State, to: State, id: &str) -> Option<String> {
    use State::*;
    Some(match (from, to) {
        (_, Approved) => format!("aida edit {id} --status approved"),
        (_, Planned) => format!("aida edit {id} --status planned"),
        (Approved, InProgress) | (Planned, InProgress) => format!("aida queue work {id}"),
        (NeedsAttention, InProgress) => format!("aida edit {id} --status in-progress"),
        (Done, InProgress) => return None, // reviewer RequestChanges, not a nudge
        (InProgress, Done) => format!("aida queue done {id}"),
        (Done, Completed) => "aida pull".to_string(), // merge auto-bump
        (Completed, Released) => return None,         // repo-level release act
        (_, Rejected) => format!("aida edit {id} --status rejected"),
        _ => return None,
    })
}

/// Lifecycle-aware next steps for a single spec in `status`: the valid declared
/// transitions out of its current state, templated with `id`, plus an archive
/// off-ramp for the terminal states. Drives the `show` / `add` / `edit`
/// surfaces. An unrecognized status yields no steps.
pub fn spec_next(status: &str, id: &str) -> Vec<NextStep> {
    let Some(state) = State::from_status_str(status) else {
        return Vec::new();
    };
    let mut ranked: Vec<(u8, NextStep)> = Vec::new();
    // STORY-727: a buildable spec (Approved / Planned) LEADS with the headline
    // autonomous drive (`aida zen <id>`) — the fastest thought-to-merged path —
    // ahead of the manual `queue work`, matching the front-door (`status_next`)
    // pattern. Pushed first at rank 0; the stable sort keeps it ahead of the
    // (also rank-0) `queue work` move.
    if matches!(state, State::Approved | State::Planned) {
        ranked.push((0, NextStep::new(format!("aida zen {id}"), "merged")));
    }
    for t in LifecycleModel::declared().transitions {
        if t.from != state {
            continue;
        }
        if let Some(cmd) = transition_command(t.from, t.to, id) {
            ranked.push((rank(t.to), NextStep::new(cmd, state_token(t.to))));
        }
    }
    // Terminal long-tail: the only meaningful next move is to archive it.
    if state.is_terminal() {
        ranked.push((9, NextStep::new(format!("aida archive {id}"), "archived")));
    }
    ranked.sort_by_key(|(r, _)| *r);
    ranked.into_iter().map(|(_, s)| s).collect()
}

/// Active filter context carried forward into the `list` surface's suggestions.
/// The status filter is the load-bearing one: `aida list --status draft` should
/// suggest the draft state's next transition (`edit --status approved`), so the
/// filter the agent just used flows into the next step.
#[derive(Default)]
pub struct ListContext<'a> {
    /// The `--status <s>` filter in effect, if any.
    pub status: Option<&'a str>,
}

/// Next steps after a `list`: drill into a row, and — when a status filter is
/// active — the valid next transition for that filtered state (the filter
/// carried forward). `list` is a MULTI-ROW surface, so the commands template a
/// literal `<id>` placeholder rather than picking one row's concrete id: that
/// matches the AXI #9 example and, critically, keeps a real spec id from being
/// echoed a second time into the machine-parseable id stream (a concrete id in
/// the trailing block would make a `list | grep id | uniq -d` collision check
/// see a false duplicate). The per-spec `show` view supplies the concrete id.
pub fn list_next(ctx: &ListContext) -> Vec<NextStep> {
    let id = "<id>";
    let mut steps = vec![NextStep::new(format!("aida show {id}"), "detail")];
    // Carry the active status filter forward into the lifecycle-aware next move.
    if let Some(status) = ctx.status {
        if let Some(state) = State::from_status_str(status) {
            for t in LifecycleModel::declared().transitions {
                if t.from != state {
                    continue;
                }
                // Only the single highest-priority forward transition, to keep
                // the list-level hint tight (the per-spec `show` view fans out
                // the full set).
                if rank(t.to) <= 2 {
                    if let Some(cmd) = transition_command(t.from, t.to, id) {
                        steps.push(NextStep::new(cmd, state_token(t.to)));
                        break;
                    }
                }
            }
        }
    }
    steps
}

/// Next steps after a bare `status` snapshot. Leads with the FASTEST
/// thought-to-merged path for the ACTIONABLE queue head: `aida zen <id>` for an
/// approved/planned spec (one-shot autonomous implement + ship) or `aida ship
/// <id>` for one already in flight — then the manual `queue work` / `show`.
/// Falls back to the approvable backlog when nothing actionable is queued.
/// `top` carries the head spec's (id, status) so the suggestion matches its
/// lifecycle state; the caller has already filtered out archived/completed/
/// deferred corpses so anything passed here is safe to nudge.
pub fn status_next(_queue_depth: usize, top: Option<(&str, &str)>) -> Vec<NextStep> {
    match top {
        Some((id, status)) => {
            let st = status.to_ascii_lowercase();
            let in_flight = st.contains("progress") || st.replace('-', "") == "needsattention";
            let mut steps = Vec::new();
            if in_flight {
                // Already being implemented — finish it: commit, PR, CI, merge.
                steps.push(NextStep::new(format!("aida ship {id}"), "merged"));
                steps.push(NextStep::new(
                    format!("aida queue work {id}"),
                    "in-progress",
                ));
            } else {
                // Approved/planned head — the autonomous one-shot is the shortest
                // path from here to merged.
                steps.push(NextStep::new(format!("aida zen {id}"), "merged"));
                steps.push(NextStep::new(
                    format!("aida queue work {id}"),
                    "in-progress",
                ));
            }
            steps.push(NextStep::new(format!("aida show {id}"), "detail"));
            steps
        }
        None => vec![NextStep::new("aida list --status approved", "fill-queue")],
    }
}

/// Next steps after `queue list`: start the head item (or show it) when the
/// queue is non-empty, else point at the approvable backlog to fill it.
pub fn queue_next(first_id: Option<&str>) -> Vec<NextStep> {
    match first_id {
        Some(id) => vec![
            NextStep::new(format!("aida queue work {id}"), "in-progress"),
            NextStep::new(format!("aida show {id}"), "detail"),
        ],
        None => vec![NextStep::new("aida list --status approved", "fill-queue")],
    }
}

/// Render a slice of next steps as a TOON `next[N]{cmd,to}:` block, or `None`
/// when there is nothing to suggest (so the caller emits no trailing block).
/// Round-trippable TOON consistent with the rest of the agent surface.
pub fn render(steps: &[NextStep]) -> Option<String> {
    if steps.is_empty() {
        return None;
    }
    let rows: Vec<Vec<String>> = steps
        .iter()
        .map(|s| vec![s.cmd.clone(), s.to.clone()])
        .collect();
    Some(crate::toon::table_raw("next", &["cmd", "to"], &rows))
}

/// A short human-facing gloss for a next step's target token, used by the
/// `Next:` block on the per-spec HUMAN views (`show` / `why` / `status <id>`).
/// An unmapped token glosses to the empty string, so the command renders on its
/// own with no trailing description.
// trace:STORY-727 | ai:claude
fn human_hint(to: &str) -> &'static str {
    match to {
        "merged" => "build + ship it autonomously, end-to-end",
        "in-progress" => "start implementing it yourself",
        "approved" => "approve it",
        "planned" => "mark it planned — the design is settled",
        "done" => "mark it done (finished on a branch)",
        "completed" => "sync after merge to auto-complete it",
        "rejected" => "reject it",
        "archived" => "hide it from the default views",
        "detail" => "see the full detail",
        "triage" => "triage the draft inbox",
        "fill-queue" => "fill the queue from the approved backlog",
        _ => "",
    }
}

/// Render the next steps as a HUMAN `Next:` block — a bold header plus one cyan
/// `aida ...` command per line with a dimmed gloss. The human-TTY analog of
/// [`render`] (which emits the agent-mode TOON `next` block); the two never both
/// fire on one surface. `None` when there is nothing to suggest, so the caller
/// emits no trailing block.
// trace:STORY-727 | ai:claude
pub fn render_human(steps: &[NextStep]) -> Option<String> {
    if steps.is_empty() {
        return None;
    }
    use colored::Colorize;
    let arrow = crate::glyph(crate::glyphs::Glyph::Arrow);
    let mut lines: Vec<String> = vec![format!("\n{}", "Next:".bold())];
    for s in steps {
        let hint = human_hint(&s.to);
        if hint.is_empty() {
            lines.push(format!("  {} {}", arrow, s.cmd.cyan()));
        } else {
            lines.push(format!("  {} {}   {}", arrow, s.cmd.cyan(), hint.dimmed()));
        }
    }
    Some(lines.join("\n"))
}

/// STORY-737 (delight #2): the HUMAN footer shown after `aida add`. A brand-new
/// user files spec #1 at the TTY and needs the next-step nudge MOST, yet the
/// post-add block used to fire only in agent mode — the agent got guided, the
/// human got a bare "Added: …". This pairs the lifecycle `Next:` block (the same
/// idiom `aida show` uses — approve it / reject it for a fresh draft) with a
/// trace-link breadcrumb teaching the ONE move that wires code to the spec.
/// Returns the full multi-line footer; the breadcrumb always renders even when
/// `status` yields no transitions, so the newcomer always learns the trace step.
// trace:STORY-737 | ai:claude
pub fn render_human_add_footer(status: &str, id: &str) -> String {
    use colored::Colorize;
    let mut out = String::new();
    if let Some(block) = render_human(&spec_next(status, id)) {
        out.push_str(&block);
        out.push('\n');
    }
    let arrow = crate::glyph(crate::glyphs::Glyph::SubArrow);
    out.push_str(&format!(
        "  {} Link your code to it: add a {} comment where you implement it.",
        arrow.dimmed(),
        format!("// trace:{id}").cyan(),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds(steps: &[NextStep]) -> Vec<String> {
        steps.iter().map(|s| s.cmd.clone()).collect()
    }

    // The (command, state) -> suggestion mapping is lifecycle-aware: each spec
    // status yields the VALID next transition(s) for that state, templated.
    #[test]
    fn spec_next_reflects_lifecycle_state() {
        // Draft -> approve / reject (approve ranks first).
        let draft = spec_next("draft", "TASK-1");
        assert_eq!(
            cmds(&draft),
            vec![
                "aida edit TASK-1 --status approved",
                "aida edit TASK-1 --status rejected"
            ]
        );

        // Approved -> STORY-727: lead with the autonomous one-shot (`aida zen`),
        // then the manual `queue work`, then plan, then reject.
        let approved = spec_next("approved", "TASK-2");
        assert_eq!(approved[0].cmd, "aida zen TASK-2");
        assert!(cmds(&approved).contains(&"aida queue work TASK-2".to_string()));
        assert!(cmds(&approved).contains(&"aida edit TASK-2 --status planned".to_string()));

        // Planned -> STORY-727: also leads with `aida zen` (design settled,
        // ready to build), then the manual `queue work`.
        let planned = spec_next("planned", "TASK-2b");
        assert_eq!(planned[0].cmd, "aida zen TASK-2b");
        assert!(cmds(&planned).contains(&"aida queue work TASK-2b".to_string()));

        // In Progress -> done. Tolerant of the Display spelling.
        let in_prog = spec_next("In Progress", "TASK-3");
        assert_eq!(cmds(&in_prog), vec!["aida queue done TASK-3"]);

        // Done -> merge auto-bump (aida pull), NOT a reviewer-driven reopen.
        let done = spec_next("done", "TASK-4");
        assert_eq!(cmds(&done), vec!["aida pull"]);

        // Needs Attention -> resume / approve / reject.
        let na = spec_next("needs-attention", "TASK-5");
        assert!(cmds(&na).contains(&"aida edit TASK-5 --status in-progress".to_string()));
        assert!(cmds(&na).contains(&"aida edit TASK-5 --status approved".to_string()));
    }

    // Terminal states have no forward transition — the only move is to archive.
    #[test]
    fn terminal_states_suggest_archive() {
        assert_eq!(
            cmds(&spec_next("completed", "TASK-9")),
            vec!["aida archive TASK-9"]
        );
        assert_eq!(
            cmds(&spec_next("rejected", "TASK-9")),
            vec!["aida archive TASK-9"]
        );
    }

    // An unparseable status yields no suggestions rather than a wrong one.
    #[test]
    fn unknown_status_yields_nothing() {
        assert!(spec_next("frobnicated", "TASK-1").is_empty());
    }

    // list: drills into a row (placeholder id) + carries the active status
    // filter forward into that state's next transition.
    #[test]
    fn list_next_carries_status_filter_forward() {
        let ctx = ListContext {
            status: Some("draft"),
        };
        let steps = list_next(&ctx);
        assert_eq!(
            cmds(&steps),
            vec!["aida show <id>", "aida edit <id> --status approved"]
        );

        // No status filter -> just the drill-in. The list surface always uses a
        // placeholder id (never echoes a concrete spec id into the id stream).
        let bare = list_next(&ListContext::default());
        assert_eq!(cmds(&bare), vec!["aida show <id>"]);
    }

    #[test]
    fn status_and_queue_next_steps() {
        // An approved actionable head -> lead with the autonomous one-shot
        // (`aida zen`) — the fastest thought-to-merged path. trace:STORY-723
        let approved = status_next(3, Some(("TASK-8", "approved")));
        assert_eq!(approved[0].cmd, "aida zen TASK-8");
        assert_eq!(approved[0].to, "merged");
        assert!(cmds(&approved).contains(&"aida queue work TASK-8".to_string()));

        // An in-flight head -> lead with `aida ship` (finish + merge it).
        let in_flight = status_next(1, Some(("TASK-9", "in-progress")));
        assert_eq!(in_flight[0].cmd, "aida ship TASK-9");

        // Nothing actionable queued -> point at the approvable backlog.
        assert_eq!(
            cmds(&status_next(0, None)),
            vec!["aida list --status approved"]
        );

        let q = queue_next(Some("TASK-8"));
        assert_eq!(q[0].cmd, "aida queue work TASK-8");
        assert_eq!(cmds(&queue_next(None)), vec!["aida list --status approved"]);
    }

    // The rendered block is a valid, round-trippable TOON table; empty -> None.
    #[test]
    fn render_emits_toon_next_block() {
        let steps = spec_next("draft", "TASK-1");
        let block = render(&steps).expect("non-empty");
        assert!(block.starts_with("next[2]{cmd,to}:"));
        let parsed = crate::toon::parse_table(&block).expect("round-trips");
        assert_eq!(parsed.name, "next");
        assert_eq!(parsed.fields, vec!["cmd", "to"]);
        assert_eq!(parsed.rows[0][0], "aida edit TASK-1 --status approved");

        assert!(render(&[]).is_none());
    }

    // STORY-727: the HUMAN per-spec views render a `Next:` block that NAMES the
    // concrete next command (Approved leads with `aida zen`), and empty -> None.
    #[test]
    fn render_human_emits_next_command_block() {
        let steps = spec_next("approved", "STORY-7");
        let block = render_human(&steps).expect("non-empty");
        assert!(block.contains("Next:"));
        // Leads with the autonomous one-shot for an Approved spec.
        assert!(block.contains("aida zen STORY-7"));
        // And still offers the manual implement path.
        assert!(block.contains("aida queue work STORY-7"));

        // An in-progress spec gets the "finish it" command, not zen.
        let in_prog = render_human(&spec_next("In Progress", "STORY-8")).expect("non-empty");
        assert!(in_prog.contains("aida queue done STORY-8"));

        assert!(render_human(&[]).is_none());
    }

    // STORY-737 (delight #2): the HUMAN add footer pairs the lifecycle `Next:`
    // block with the trace-link breadcrumb. A freshly-filed draft leads with
    // approve/reject AND always teaches the trace step.
    #[test]
    fn render_human_add_footer_pairs_next_block_with_trace_breadcrumb() {
        let footer = render_human_add_footer("draft", "TASK-1");
        // The lifecycle Next: block (same idiom as `aida show`).
        assert!(
            footer.contains("Next:"),
            "footer missing Next: block:\n{footer}"
        );
        assert!(
            footer.contains("aida edit TASK-1 --status approved"),
            "draft footer should offer the approve move:\n{footer}"
        );
        // The trace-link breadcrumb with the spec's real id.
        assert!(
            footer.contains("// trace:TASK-1"),
            "footer missing trace breadcrumb:\n{footer}"
        );
        assert!(
            footer.contains("Link your code to it"),
            "footer missing the link-your-code breadcrumb:\n{footer}"
        );
        // It is the HUMAN render, not the agent TOON block.
        assert!(
            !footer.contains("cmd,to"),
            "human footer must not emit the agent TOON `next` block:\n{footer}"
        );
    }

    // The breadcrumb renders even when the status yields no transitions, so a
    // newcomer always learns the trace step regardless of lifecycle state.
    #[test]
    fn render_human_add_footer_always_emits_trace_breadcrumb() {
        let footer = render_human_add_footer("frobnicated", "BUG-9");
        assert!(!footer.contains("Next:"), "no transitions → no Next: block");
        assert!(footer.contains("// trace:BUG-9"));
    }
}
