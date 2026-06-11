//! Declared spec-state transition model — the single source of truth for the
//! lifecycle the Mermaid diagram is generated from.
//!
//! Phase 1 (SPIKE-56 / TASK-737) is **generate-only**: this module declares the
//! status chain (Region 1 of the lifecycle diagram in `docs/lifecycle.md`) and
//! renders it to a Mermaid `stateDiagram-v2`. Later phases reuse this same
//! declared model for guard enforcement and declared-vs-empirical diffing; they
//! are deliberately NOT implemented here. Encode only what `docs/lifecycle.md`
//! and the README "Spec lifecycle" section already document — do not invent new
//! transitions.
//
// trace:TASK-737 | ai:claude

/// Which kind of trigger most often drives a transition (and, for entry-into-a
/// -state, colours the node). Mirrors the three-trigger-kinds legend in
/// `docs/lifecycle.md`: blue = CLI/human, purple = LLM decision, green =
/// system/git-event.
// trace:TASK-737 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    /// 🔵 A person (or script) runs an `aida` verb.
    Cli,
    /// 🟣 A Claude session decides and acts.
    Llm,
    /// 🟢 A git event or background sweep fires it, no human in the loop.
    Git,
}

impl TriggerKind {
    /// The Mermaid `classDef` name this trigger kind maps onto.
    fn class_name(self) -> &'static str {
        match self {
            TriggerKind::Cli => "cli",
            TriggerKind::Llm => "llm",
            TriggerKind::Git => "git",
        }
    }
}

/// A declared spec-state. `[*]` (the Mermaid pseudo start/end node) is modelled
/// as [`State::Start`] / it is rendered specially; the rest are the real
/// `status` values from `docs/lifecycle.md`.
// trace:TASK-737 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Start,
    Draft,
    Approved,
    Planned,
    InProgress,
    Done,
    Completed,
    Released,
    Rejected,
    NeedsAttention,
}

impl State {
    /// The node label as it appears in the Mermaid diagram.
    fn label(self) -> &'static str {
        match self {
            State::Start => "[*]",
            State::Draft => "Draft",
            State::Approved => "Approved",
            State::Planned => "Planned",
            State::InProgress => "InProgress",
            State::Done => "Done",
            State::Completed => "Completed",
            State::Released => "Released",
            State::Rejected => "Rejected",
            State::NeedsAttention => "NeedsAttention",
        }
    }

    /// The trigger kind that most often drives **entry** into this state — this
    /// is what colours the node. `None` = not classified (the start/end
    /// pseudo-node).
    fn entry_trigger(self) -> Option<TriggerKind> {
        match self {
            State::Start => None,
            State::Draft | State::Approved | State::Planned | State::Rejected => {
                Some(TriggerKind::Cli)
            }
            State::InProgress | State::NeedsAttention => Some(TriggerKind::Llm),
            State::Done | State::Completed | State::Released => Some(TriggerKind::Git),
        }
    }
}

/// One legal transition in the declared status chain.
// trace:TASK-737 | ai:claude
#[derive(Debug, Clone, Copy)]
pub struct Transition {
    pub from: State,
    pub to: State,
    /// The precise verb / command that drives the transition (the Mermaid edge
    /// label).
    pub verb: &'static str,
}

/// The declared status-chain model: the states plus the legal transitions, each
/// with its verb. This is **the single source** — the diagram is derived from
/// it, never hand-maintained. Encodes Region 1 of `docs/lifecycle.md` exactly
/// (Draft → Approved → Planned → In Progress → Done → Completed → Released, plus
/// the Rejected / Needs Attention branches).
// trace:TASK-737 | ai:claude
#[derive(Debug, Clone)]
pub struct LifecycleModel {
    pub transitions: Vec<Transition>,
}

impl Default for LifecycleModel {
    fn default() -> Self {
        Self::declared()
    }
}

impl LifecycleModel {
    /// The canonical declared model. Order matters: the diagram renders edges in
    /// declaration order, grouped to match the committed `docs/lifecycle.md`
    /// block (mainline chain, then the Needs Attention branch, then the
    /// off-mainline edges).
    // trace:TASK-737 | ai:claude
    pub fn declared() -> Self {
        use State::*;
        let t = |from, to, verb| Transition { from, to, verb };
        LifecycleModel {
            transitions: vec![
                // Entry.
                t(Start, Draft, "aida add (LLM/human)"),
                // Mainline chain.
                t(Draft, Approved, "aida edit --status approved"),
                t(Approved, Planned, "aida edit --status planned"),
                t(Approved, InProgress, "aida queue work"),
                t(Planned, InProgress, "aida queue work"),
                t(InProgress, Done, "aida queue done / aida-pr"),
                t(Done, Completed, "merge auto-bump (aida pull)"),
                t(Completed, Released, "release tag (scripts/release.sh)"),
                // Needs Attention branch (the off-mainline pause state).
                t(InProgress, NeedsAttention, "punt (design-fork)"),
                t(NeedsAttention, InProgress, "aida edit --status in-progress"),
                t(NeedsAttention, Approved, "aida edit --status approved"),
                t(NeedsAttention, Rejected, "aida edit --status rejected"),
                // Off-mainline edges.
                t(Draft, Rejected, "aida edit --status rejected"),
                t(Approved, Rejected, "aida edit --status rejected"),
                t(Done, InProgress, "reviewer RequestChanges"),
            ],
        }
    }

    /// Render the declared status chain as a Mermaid `stateDiagram-v2`,
    /// byte-for-byte matching the committed Region 1 block in
    /// `docs/lifecycle.md` (so the doc-pin check can compare them directly).
    /// The body is grouped — entry, mainline, Needs Attention branch,
    /// off-mainline — and terminal states emit `--> [*]` edges, then the
    /// `classDef` legend + per-state class assignments derived from each
    /// state's [`State::entry_trigger`].
    // trace:TASK-737 | ai:claude
    pub fn to_mermaid(&self) -> String {
        let mut out = String::new();
        out.push_str("stateDiagram-v2\n");
        out.push_str("    direction LR\n");

        // Edges, grouped with blank lines exactly as the committed block.
        // Group boundaries are derived from the declared `from` state so the
        // grouping tracks the model, not a hand-kept layout: the entry edge,
        // then the mainline chain (everything up to the first NeedsAttention
        // edge), then the NeedsAttention branch, then the remaining
        // off-mainline edges.
        let mut emitted_entry = false;
        let mut in_needs_attention = false;
        let mut after_needs_attention = false;
        for tr in &self.transitions {
            if !emitted_entry {
                // entry edge
                out.push_str(&format!(
                    "    {} --> {}: {}\n",
                    tr.from.label(),
                    tr.to.label(),
                    tr.verb
                ));
                out.push('\n');
                emitted_entry = true;
                continue;
            }
            let touches_na = tr.from == State::NeedsAttention || tr.to == State::NeedsAttention;
            if touches_na && !in_needs_attention {
                // start of the Needs Attention branch group
                out.push('\n');
                in_needs_attention = true;
            } else if !touches_na && in_needs_attention && !after_needs_attention {
                // start of the off-mainline group
                out.push('\n');
                after_needs_attention = true;
            }
            out.push_str(&format!(
                "    {} --> {}: {}\n",
                tr.from.label(),
                tr.to.label(),
                tr.verb
            ));
        }

        // Terminal `--> [*]` edges for the end states, in the committed order.
        out.push('\n');
        for term in [State::Released, State::Rejected, State::Completed] {
            out.push_str(&format!("    {} --> [*]\n", term.label()));
        }

        // The classDef legend.
        out.push('\n');
        out.push_str("    classDef cli fill:#1f6feb,stroke:#0d3b8a,color:#fff\n");
        out.push_str("    classDef llm fill:#8957e5,stroke:#5a2ca0,color:#fff\n");
        out.push_str("    classDef git fill:#2da44e,stroke:#176b2e,color:#fff\n");

        // Per-class state assignments, derived from entry_trigger, in the
        // committed cli/llm/git order.
        out.push('\n');
        for kind in [TriggerKind::Cli, TriggerKind::Llm, TriggerKind::Git] {
            let members: Vec<&str> = self
                .states_in_order()
                .into_iter()
                .filter(|s| s.entry_trigger() == Some(kind))
                .map(|s| s.label())
                .collect();
            if !members.is_empty() {
                out.push_str(&format!(
                    "    class {} {}\n",
                    members.join(","),
                    kind.class_name()
                ));
            }
        }

        out
    }

    /// The real (non-pseudo) states in the committed presentation order, deduped
    /// — used to render the per-class `class A,B,C cli` lines deterministically.
    // trace:TASK-737 | ai:claude
    fn states_in_order(&self) -> Vec<State> {
        use State::*;
        // Fixed presentation order matching docs/lifecycle.md.
        vec![
            Draft,
            Approved,
            Planned,
            InProgress,
            Done,
            Completed,
            Released,
            Rejected,
            NeedsAttention,
        ]
    }
}

/// Wrap the generated Mermaid body in a fenced ```mermaid code block.
// trace:TASK-737 | ai:claude
pub fn fenced_mermaid(model: &LifecycleModel) -> String {
    format!("```mermaid\n{}```\n", model.to_mermaid())
}

/// Extract the **first** fenced ```mermaid block body from a markdown document
/// (Region 1 of `docs/lifecycle.md` is the first one). Returns the inner body
/// WITHOUT the fence lines, or `None` if there is no mermaid block.
// trace:TASK-737 | ai:claude
pub fn first_mermaid_block(markdown: &str) -> Option<String> {
    let mut lines = markdown.lines();
    let mut body = String::new();
    // Find opening fence.
    for line in lines.by_ref() {
        if line.trim_start().starts_with("```mermaid") {
            break;
        }
    }
    let mut found_close = false;
    for line in lines {
        if line.trim_start().starts_with("```") {
            found_close = true;
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    if body.is_empty() && !found_close {
        // We never entered a block (or it was empty with no close).
        return None;
    }
    if !found_close {
        return None;
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_model_has_all_states_reachable() {
        let m = LifecycleModel::declared();
        // Every mainline + branch state appears as a `from` or `to`.
        for s in [
            State::Draft,
            State::Approved,
            State::Planned,
            State::InProgress,
            State::Done,
            State::Completed,
            State::Released,
            State::Rejected,
            State::NeedsAttention,
        ] {
            let touched = m.transitions.iter().any(|t| t.from == s || t.to == s);
            assert!(touched, "state {:?} must appear in a transition", s);
        }
    }

    #[test]
    fn mermaid_is_a_state_diagram() {
        let body = LifecycleModel::declared().to_mermaid();
        assert!(body.starts_with("stateDiagram-v2\n"));
        assert!(body.contains("[*] --> Draft"));
        assert!(body.contains("Done --> Completed: merge auto-bump (aida pull)"));
        assert!(body.contains("Completed --> [*]"));
        assert!(body.contains("class Draft,Approved,Planned,Rejected cli"));
    }

    #[test]
    fn fenced_round_trips_through_extractor() {
        let m = LifecycleModel::declared();
        let fenced = fenced_mermaid(&m);
        let extracted = first_mermaid_block(&fenced).expect("has a mermaid block");
        assert_eq!(extracted, m.to_mermaid());
    }

    #[test]
    fn extractor_returns_none_without_a_block() {
        assert!(first_mermaid_block("no fences here\njust prose\n").is_none());
    }

    #[test]
    fn extractor_grabs_first_block_only() {
        let md = "intro\n```mermaid\nstateDiagram-v2\n    A --> B\n```\nmiddle\n```mermaid\nOTHER\n```\n";
        let body = first_mermaid_block(md).unwrap();
        assert!(body.contains("A --> B"));
        assert!(!body.contains("OTHER"));
    }
}
