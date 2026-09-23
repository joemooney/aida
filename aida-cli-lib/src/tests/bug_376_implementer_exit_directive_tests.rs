use super::write_implementer_complete_banner;

fn strip_ansi(s: &str) -> String {
    // Same shape as status_cleanup::tests::strip_ansi — the banner
    // uses `colored` glyphs that wrap text in ESC[…m sequences.
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            while let Some(&p) = chars.peek() {
                chars.next();
                if p.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn normalized_lowercase(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Banner carries every load-bearing substrate-as-bouncer signal:
/// the headline, the concrete PR number, the explicit Ctrl+D
/// instruction, the "Do NOT watch CI" prohibition, and the hand-off
/// to the orchestrator / next-phase agent.
#[test]
fn banner_emits_all_load_bearing_substrate_signals() {
    colored::control::set_override(false);
    let mut buf = Vec::new();
    write_implementer_complete_banner(&mut buf, 296, true).unwrap();
    colored::control::unset_override();
    let out = strip_ansi(&String::from_utf8(buf).unwrap());

    // Headline — the substrate's "you are done" verdict.
    assert!(
        out.contains("IMPLEMENTER COMPLETE — EXIT NOW"),
        "headline missing from banner: {out}"
    );
    // Concrete PR number — placeholders would defeat the load-bearing
    // moment (see TASK-291's "PR-57 / BUG-219, STORY-261" pattern).
    assert!(
        out.contains("PR-296"),
        "PR number substitution missing: {out}"
    );
    // Explicit user-action — "Press Ctrl+D" must be named verbatim;
    // a vague "session is done" is exactly what BUG-376 / TASK-291
    // forbid.
    assert!(
        out.contains("Press Ctrl+D"),
        "explicit Ctrl+D instruction missing: {out}"
    );
    // Substrate-as-bouncer prohibition — name the specific anti-
    // pattern the bug observed ("watch CI") so the agent has no
    // confusion about which behavior is being banned.
    assert!(
        out.contains("Do NOT watch CI"),
        "CI-watching prohibition missing: {out}"
    );
    // Hand-off — name who DOES own the post-ship phases so the
    // agent's "but someone has to do it!" objection is pre-empted.
    // Tokens checked independently because the banner soft-wraps
    // the phrase "next-phase\n    agent" across two lines.
    assert!(
        out.contains("orchestrator") && out.contains("next-phase") && out.contains("agent"),
        "hand-off naming missing: {out}"
    );
}

/// BUG-1537: a PR this run did NOT merge (the BUG-574 already-merged
/// no-op path) must NOT get the first-person "IMPLEMENTER COMPLETE"
/// banner — that banner claims an act ("PR-N is merged... ran in steps
/// 2-5 above") this run did not perform. It must instead get a short,
/// honest "observed merged" line that names neither the loud headline
/// nor a false first-person claim, while still telling the session
/// there's nothing left to do.
#[test]
fn already_merged_gets_observed_line_not_implementer_complete_banner() {
    colored::control::set_override(false);
    let mut buf = Vec::new();
    write_implementer_complete_banner(&mut buf, 296, false).unwrap();
    colored::control::unset_override();
    let out = strip_ansi(&String::from_utf8(buf).unwrap());

    assert!(
        !out.contains("IMPLEMENTER COMPLETE"),
        "already-merged path must not print the acted-on-it banner: {out}"
    );
    assert!(
        out.contains("PR-296"),
        "PR number must still be named: {out}"
    );
    assert!(
        out.contains("already merged") && out.contains("this run"),
        "must state the PR was already merged and name 'this run' explicitly: {out}"
    );
    assert!(
        out.contains("observed") || out.contains("Nothing was merged by this session"),
        "must be honest that this run only observed the merge, not performed it: {out}"
    );
}

/// The `aida-implement.md` skill template — the file an implementer
/// agent loads at session start via Claude Code's skills mechanism —
/// must carry the matching exit-after-ship directive. Tests the
/// embedded master template (rather than the symlinked `.claude/`
/// copy) so the assertion survives a project that has not yet run
/// `make sync-templates`.
#[test]
fn aida_implement_skill_template_carries_exit_directive() {
    let template = include_str!("../../../aida-core/templates/skills/aida-implement.md");

    // The directive must reference BUG-376 so future maintainers
    // can trace the constraint back to its origin.
    assert!(
        template.contains("BUG-376"),
        "skill template missing BUG-376 attribution"
    );
    // The headline must match the substrate banner so the agent
    // sees the same phrase on the way in (skill) and on the way out
    // (banner) — reinforcement, not divergence.
    assert!(
        template.contains("IMPLEMENTER COMPLETE — EXIT NOW"),
        "skill template missing matching banner headline"
    );
    // The skill must explicitly forbid CI-watching after `aida pr
    // ship` — that is the exact ceiling pattern BUG-376 observed.
    assert!(
        template.contains("aida pr ship") && template.contains("Watch CI"),
        "skill template missing 'no CI watching after pr ship' rule"
    );
    // The skill must name `aida queue done` as the symmetric stop
    // point — the bug description called out both lifecycle commands.
    assert!(
        template.contains("aida queue done"),
        "skill template missing aida queue done symmetric rule"
    );
}

#[test]
fn guided_implement_template_carries_pr_off_ramp_checkpoint() {
    let template = include_str!("../../../aida-core/templates/skills/aida-guided-implement.md");
    let normalized = normalized_lowercase(template);

    // trace:TASK-1229 | ai:codex
    assert!(
        normalized.contains("implementation complete and submitted for review"),
        "guided template must name the submitted-for-review state"
    );
    assert!(
        normalized.contains("ci is running / review pending"),
        "guided template must be honest about pending CI/review"
    );
    assert!(
        normalized.contains("you're free to close"),
        "guided template must provide the human off-ramp"
    );
    assert!(
        normalized.contains("keystone merge stays human/advisor"),
        "guided template must not imply keystone auto-merge"
    );
    assert!(
        normalized.contains("aida queue work <spec> --resume"),
        "guided template must name the review-changes resume command"
    );
    assert!(
        normalized.contains("adrs recorded"),
        "guided template must require the recorded ADR list"
    );
    assert!(
        !normalized.contains("verify ci green, then finish simply"),
        "guided template must not restore the old wait-for-green closing"
    );
}
