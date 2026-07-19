//! `aida state-snapshot --spec <SPEC-ID>` — the deterministic finish-state
//! preamble (TASK-391).
//!
//! Every `/aida-pickup` and `/aida-pr` next-steps template carries a fixed
//! seven-row "State:" block — Spec, Branch, PR, Drain, Tests, Fmt, Plan.
//! Until this command existed each skill author filled those rows by hand
//! from `git status`, `gh pr view`, `aida show`, `aida session show`, and
//! the rows drifted: between what the skill printed and what was actually
//! true, and between sibling skills phrasing the same row differently. A
//! headless implementer (Step 5b finding metadata) could not synthesize the
//! rows at all without an unsupervised `gh pr view`.
//!
//! This module owns the rendering. The CLI handler in `main.rs` gathers the
//! inputs from the existing helpers (`current_branch_at`,
//! `ahead_behind_vs_ref`, `detect_open_pr_for_branch`,
//! `DrainState::read`, `orchestrator::detect`, the session manifest's
//! `PlanContext`) and hands them in as a `StateSnapshot`. The renderers
//! here are pure — same inputs, byte-identical output — so the unit tests
//! pin the template shape.
//!
//! The output shape mirrors the templates in
//! `aida-core/templates/skills/aida-pickup.md` and
//! `aida-core/templates/skills/aida-pr.md`:
//!
//! ```text
//! State:
//!   Spec:    <SPEC-ID>  <title>  (Status: <Status>)
//!   Branch:  <branch>   <N> commits ahead of main   <pushed|local>
//!   PR:      #<N> open: <url>   |   no PR yet
//!   Drain:   phase <N>/6 <mode>   orchestrator <on|off>
//!   Tests:   <summary or "not run">
//!   Fmt:     <summary or "not run">
//!   Plan:    <docs/plans/...md or "none">
//! ```
//!
//! Every row prints with a stable fallback string rather than being omitted
//! — a deterministic command should not have variable row counts.
//!
//! trace:TASK-391 | ai:claude

use serde::Serialize;

/// The Spec row — identity + headline status for the requirement.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SpecRow {
    pub id: String,
    pub title: String,
    pub status: String,
}

/// The Branch row — current branch, commits ahead of main, push status.
/// `None` when the snapshot is taken outside a git repo (the row prints
/// "(no branch)" in that case).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct BranchRow {
    pub name: String,
    /// Commits ahead of `origin/main` (or local `main` if no origin ref).
    /// `None` when the comparison ref could not be resolved.
    pub ahead_main: Option<u32>,
    /// `"pushed"` — upstream tracks and is at HEAD;
    /// `"local"`   — no upstream, or local has commits beyond upstream;
    /// `"unknown"` — git query failed.
    pub push_status: String,
}

/// The PR row — open PR for the current branch, or one of the fallback
/// reasons (`no PR yet`, `gh not installed`, `gh unreachable`, `gh failed`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub(crate) enum PrRow {
    None,
    Open { number: u64, url: String },
    GhMissing,
    GhUnreachable { detail: String },
    GhFailed { detail: String },
}

/// The Drain row — current phase + drain mode + orchestrator on/off.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DrainRow {
    /// `"phase 1/6"` style — `None` when no phase is known (interactive
    /// non-orchestrated session).
    pub phase: Option<u32>,
    /// Drain mode descriptor: `"interactive"` outside a drain;
    /// `"single (<spec>)"`, `"batch <NAME> (<done>/<total> done, <q> queued)"`,
    /// `"next-N (<done>/<total> done, <q> queued)"` inside one.
    pub mode: String,
    /// `true` when `aida orchestrator status` corroborates this process as
    /// an `--auto-complete` phase child.
    pub orchestrator: bool,
}

/// The Plan row — repo-relative path of the matching `docs/plans/` file
/// (`None` → prints "none").
// Internally-tagged shape matches PrRow so `--json` consumers see one
// uniform `{"state":"..."}` envelope across all enum rows. trace:TASK-416
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub(crate) enum PlanRow {
    None,
    File { path: String },
}

/// The complete preamble — everything the finish-state templates render
/// above their next-steps table.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct StateSnapshot {
    pub spec: SpecRow,
    pub branch: Option<BranchRow>,
    pub pr: PrRow,
    pub drain: DrainRow,
    /// Caller-supplied summary of the last `cargo test` run, or
    /// `"not run"`. Free-form so callers can pass `"123 passed, 0 failed"`
    /// or `"FAILED (cargo test aida_cli::foo)"` without this module
    /// imposing a schema.
    pub tests: String,
    /// Caller-supplied summary of the last `cargo fmt --check`, or
    /// `"not run"`. Same shape rationale as `tests`.
    pub fmt: String,
    pub plan: PlanRow,
}

impl StateSnapshot {
    /// Render the seven-row "State:" block as fixed-width text. Always
    /// deterministic — same inputs produce the byte-identical output the
    /// finish-state templates expect.
    pub fn render_text(&self) -> String {
        let mut out = String::with_capacity(512);
        out.push_str("State:\n");
        out.push_str(&format!(
            "  Spec:    {}  {}  (Status: {})\n",
            self.spec.id, self.spec.title, self.spec.status
        ));
        out.push_str(&format!("  Branch:  {}\n", self.branch_line()));
        out.push_str(&format!("  PR:      {}\n", self.pr_line()));
        out.push_str(&format!("  Drain:   {}\n", self.drain_line()));
        out.push_str(&format!("  Tests:   {}\n", self.tests));
        out.push_str(&format!("  Fmt:     {}\n", self.fmt));
        out.push_str(&format!("  Plan:    {}\n", self.plan_line()));
        out
    }

    /// Render the snapshot as pretty-printed JSON. The shape is the
    /// `Serialize` derivation — callers parsing this in Step 5b finding
    /// metadata can rely on stable field names.
    pub fn render_json(&self) -> String {
        // unwrap: the struct is `Serialize`-derived from owned strings and
        // primitives — serialization cannot fail.
        serde_json::to_string_pretty(self).expect("serialize StateSnapshot")
    }

    fn branch_line(&self) -> String {
        match &self.branch {
            None => "(no branch)".to_string(),
            Some(b) => {
                let ahead = match b.ahead_main {
                    Some(n) => format!("{} commits ahead of main", n),
                    None => "main comparison unavailable".to_string(),
                };
                format!("{}   {}   {}", b.name, ahead, b.push_status)
            }
        }
    }

    fn pr_line(&self) -> String {
        match &self.pr {
            PrRow::None => "no PR yet".to_string(),
            PrRow::Open { number, url } => format!("#{} open: {}", number, url),
            PrRow::GhMissing => "gh not installed".to_string(),
            PrRow::GhUnreachable { detail } => format!("gh unreachable ({})", detail),
            PrRow::GhFailed { detail } => format!("gh failed ({})", detail),
        }
    }

    fn drain_line(&self) -> String {
        let phase = match self.drain.phase {
            Some(n) => format!("phase {}/6", n),
            None => "phase ?/6".to_string(),
        };
        let orch = if self.drain.orchestrator { "on" } else { "off" };
        format!("{} {}   orchestrator {}", phase, self.drain.mode, orch)
    }

    fn plan_line(&self) -> String {
        match &self.plan {
            PlanRow::None => "none".to_string(),
            PlanRow::File { path } => path.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> StateSnapshot {
        StateSnapshot {
            spec: SpecRow {
                id: "TASK-391".to_string(),
                title: "aida state-snapshot --spec <ID>: emit the finish-state State preamble \
                        deterministically"
                    .to_string(),
                status: "In Progress".to_string(),
            },
            branch: Some(BranchRow {
                name: "task-391".to_string(),
                ahead_main: Some(3),
                push_status: "pushed".to_string(),
            }),
            pr: PrRow::Open {
                number: 131,
                url: "https://github.com/joemooney/aida/pull/131".to_string(),
            },
            drain: DrainRow {
                phase: Some(1),
                mode: "single (TASK-391)".to_string(),
                orchestrator: true,
            },
            tests: "not run".to_string(),
            fmt: "not run".to_string(),
            plan: PlanRow::File {
                path: "docs/plans/2026-05-20-task-391.md".to_string(),
            },
        }
    }

    #[test]
    fn render_text_matches_template_shape() {
        let expected = "\
State:
  Spec:    TASK-391  aida state-snapshot --spec <ID>: emit the finish-state State preamble deterministically  (Status: In Progress)
  Branch:  task-391   3 commits ahead of main   pushed
  PR:      #131 open: https://github.com/joemooney/aida/pull/131
  Drain:   phase 1/6 single (TASK-391)   orchestrator on
  Tests:   not run
  Fmt:     not run
  Plan:    docs/plans/2026-05-20-task-391.md
";
        assert_eq!(baseline().render_text(), expected);
    }

    #[test]
    fn render_text_uses_stable_fallbacks_for_absent_rows() {
        // No PR, no plan, no branch upstream, drain off — the deterministic
        // command never omits a row; it prints the fallback string.
        let snap = StateSnapshot {
            spec: SpecRow {
                id: "TASK-001".to_string(),
                title: "a thing".to_string(),
                status: "Approved".to_string(),
            },
            branch: Some(BranchRow {
                name: "task-001".to_string(),
                ahead_main: Some(0),
                push_status: "local".to_string(),
            }),
            pr: PrRow::None,
            drain: DrainRow {
                phase: None,
                mode: "interactive".to_string(),
                orchestrator: false,
            },
            tests: "not run".to_string(),
            fmt: "not run".to_string(),
            plan: PlanRow::None,
        };
        let text = snap.render_text();
        assert!(text.contains("PR:      no PR yet"), "{text}");
        assert!(text.contains("Plan:    none"), "{text}");
        assert!(
            text.contains("Drain:   phase ?/6 interactive   orchestrator off"),
            "{text}"
        );
        assert!(text.contains("Branch:  task-001   0 commits ahead of main   local"));
        // Always exactly 8 lines: header + 7 rows + trailing newline.
        assert_eq!(text.lines().count(), 8, "{text}");
    }

    #[test]
    fn render_text_handles_branch_absent() {
        let mut snap = baseline();
        snap.branch = None;
        assert!(snap.render_text().contains("Branch:  (no branch)"));
    }

    #[test]
    fn render_text_handles_main_comparison_missing() {
        let mut snap = baseline();
        snap.branch = Some(BranchRow {
            name: "task-391".to_string(),
            ahead_main: None,
            push_status: "pushed".to_string(),
        });
        assert!(snap
            .render_text()
            .contains("Branch:  task-391   main comparison unavailable   pushed"));
    }

    #[test]
    fn render_text_pr_fallback_variants() {
        let mut snap = baseline();
        snap.pr = PrRow::GhMissing;
        assert!(snap.render_text().contains("PR:      gh not installed"));
        snap.pr = PrRow::GhUnreachable {
            detail: "dial tcp".to_string(),
        };
        assert!(snap
            .render_text()
            .contains("PR:      gh unreachable (dial tcp)"));
        snap.pr = PrRow::GhFailed {
            detail: "exited 2".to_string(),
        };
        assert!(snap.render_text().contains("PR:      gh failed (exited 2)"));
    }

    #[test]
    fn render_json_round_trips() {
        let snap = baseline();
        let json = snap.render_json();
        // Cheap structural assertions — full deserialization round-trip
        // would need #[derive(Deserialize)] on every variant, which the
        // consumer (Step 5b finding metadata) does not need.
        assert!(json.contains("\"spec\""), "{json}");
        assert!(json.contains("\"TASK-391\""), "{json}");
        assert!(json.contains("\"state\": \"open\""), "{json}");
        assert!(json.contains("\"number\": 131"), "{json}");
        assert!(json.contains("\"orchestrator\": true"), "{json}");
        // PlanRow mirrors PrRow's internally-tagged shape — both variants
        // render as `{"state":"..."}`, never as a bare string or
        // externally-tagged `{"File":"..."}`. trace:TASK-416
        assert!(json.contains("\"plan\""), "{json}");
        assert!(json.contains("\"state\": \"file\""), "{json}");
        assert!(
            json.contains("\"path\": \"docs/plans/2026-05-20-task-391.md\""),
            "{json}"
        );
    }

    #[test]
    fn render_json_plan_none_uses_tagged_object() {
        // The PlanRow::None variant must serialize as `{"state":"none"}`
        // — never the bare string `"None"` (the pre-fix shape that prompted
        // TASK-416, where consumers had to branch string-vs-object just for
        // `plan`).
        let mut snap = baseline();
        snap.plan = PlanRow::None;
        let json = snap.render_json();
        assert!(
            json.contains("\"plan\": {\n    \"state\": \"none\"\n  }"),
            "{json}"
        );
        assert!(!json.contains("\"plan\": \"None\""), "{json}");
    }

    #[test]
    fn drain_line_formats_phase_and_orchestrator() {
        let mut snap = baseline();
        snap.drain = DrainRow {
            phase: Some(3),
            mode: "batch foo (2/5 done, 2 queued)".to_string(),
            orchestrator: true,
        };
        assert_eq!(
            snap.drain_line(),
            "phase 3/6 batch foo (2/5 done, 2 queued)   orchestrator on"
        );
    }

    #[test]
    fn rendered_block_starts_with_state_header() {
        // The finish-state templates rely on the literal "State:" header
        // so a skimmer recognises the block at a glance. Pin it.
        assert!(baseline().render_text().starts_with("State:\n"));
    }
}
