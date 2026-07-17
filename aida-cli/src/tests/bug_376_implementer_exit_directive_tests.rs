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

/// Banner carries every load-bearing substrate-as-bouncer signal:
/// the headline, the concrete PR number, the explicit Ctrl+D
/// instruction, the "Do NOT watch CI" prohibition, and the hand-off
/// to the orchestrator / next-phase agent.
#[test]
fn banner_emits_all_load_bearing_substrate_signals() {
    colored::control::set_override(false);
    let mut buf = Vec::new();
    write_implementer_complete_banner(&mut buf, 296).unwrap();
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
