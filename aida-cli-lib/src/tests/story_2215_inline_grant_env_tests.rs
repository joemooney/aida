//! STORY-2215 acceptance (3)(4)(5): the env matrix for the one-time grant
//! offer. Only a human terminal with no grant at all, outside any headless
//! run, agent harness or contained seat, with `--grant-inline auto`, is
//! offered the grant; every other case keeps today's refusal.
// trace:STORY-2215 | ai:claude

use super::*;

fn human_terminal() -> InlineGrantContext {
    InlineGrantContext {
        offer_enabled: true,
        grant_absent: true,
        stdin_tty: true,
        stdout_tty: true,
        stderr_tty: true,
        contained: false,
        roster_allows: true,
        env: vec![
            ("HOME".into(), "/home/person".into()),
            ("AIDA_SESSION_ROLE".into(), "advisor".into()),
        ],
    }
}

#[test]
fn a_human_terminal_with_no_grant_is_offered_the_grant() {
    assert!(inline_grant_eligible(&human_terminal()));
}

#[test]
fn each_missing_condition_withholds_the_offer() {
    type Edit = fn(&mut InlineGrantContext);
    let cases: &[(&str, Edit)] = &[
        ("--grant-inline=no", |c| c.offer_enabled = false),
        ("a grant is present (valid, invalid or expired)", |c| {
            c.grant_absent = false
        }),
        ("stdin not a TTY", |c| c.stdin_tty = false),
        ("stdout not a TTY", |c| c.stdout_tty = false),
        ("stderr not a TTY", |c| c.stderr_tty = false),
        ("contained seat", |c| c.contained = true),
        (
            "the roster cannot back the grant or delegate the seat",
            |c| c.roster_allows = false,
        ),
    ];
    for (label, edit) in cases {
        let mut ctx = human_terminal();
        edit(&mut ctx);
        assert!(!inline_grant_eligible(&ctx), "{label} must not prompt");
    }
}

#[test]
fn headless_and_agent_harness_env_withhold_the_offer() {
    for name in [
        "AIDA_HEADLESS",
        "CLAUDECODE",
        "CODEX_THREAD_ID",
        "CODEX_SANDBOX",
        "AIDA_AGENT_NAME",
        "AIDA_AGENT_TYPE",
        "AIDA_AGENT_CMD",
    ] {
        let mut ctx = human_terminal();
        ctx.env.push((name.into(), "1".into()));
        assert!(!inline_grant_eligible(&ctx), "{name} must not prompt");
    }
    // A blank value is unset, as the other human-presence checks read it.
    let mut ctx = human_terminal();
    ctx.env.push(("AIDA_HEADLESS".into(), " ".into()));
    assert!(inline_grant_eligible(&ctx));
}

/// `--bg` never offers: its child either cannot carry a grant or needs none.
#[test]
fn bg_launches_and_grant_inline_no_never_enable_the_offer() {
    use crate::agent_launch_args::GrantInline;
    assert!(inline_offer_enabled(GrantInline::Auto, false));
    assert!(!inline_offer_enabled(GrantInline::Auto, true));
    assert!(!inline_offer_enabled(GrantInline::No, false));
    assert!(!inline_offer_enabled(GrantInline::No, true));
}

/// Without `aida agent new` turning it on, no launcher prompts: other paths
/// that reach the preflight keep the pure refusal.
#[test]
fn the_offer_is_off_unless_agent_new_enables_it() {
    assert!(!GRANT_INLINE_OFFER.load(std::sync::atomic::Ordering::Relaxed));
}

/// (5): a present grant that does not delegate the seat keeps the BUG-052
/// refusal and its widening command, never the inline offer.
#[test]
fn a_grant_that_does_not_delegate_keeps_the_bug_052_refusal() {
    let grant = crate::gate::seat_authority::SeatGrant {
        id: "00000000-0000-4000-8000-000000000000".into(),
        principal: "p".into(),
        subject: "p".into(),
        session_id: String::new(),
        seat: "advisor".into(),
        tty_issued_at: chrono::Utc::now(),
        delegable_seats: vec!["reviewer".into()],
        parent_grant_id: None,
        issued_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now(),
        revoked_at: None,
        binding: Vec::new(),
        origin: None,
    };
    let text = agent_new_delegation_refusal(
        std::path::Path::new("/repo"),
        "implementer",
        Some(&grant),
        &anyhow::anyhow!("x"),
    );
    assert!(text.contains("does not delegate `implementer`"), "{text}");
    assert!(
        text.contains(
            "aida role enter advisor --delegate-seat reviewer --delegate-seat implementer"
        ),
        "{text}"
    );
}

/// The offer's card names what a yes mints: the launched seat, the advisor
/// parent delegating only that seat, the expiry, and the one-time binding.
// trace:STORY-2215 | ai:claude
#[test]
fn inline_grant_card_states_what_is_granted() {
    use chrono::TimeZone;
    let now = chrono::Local
        .with_ymd_and_hms(2026, 11, 1, 9, 0, 0)
        .unwrap();
    let card = inline_grant_card("reviewer", 8, now);
    let text = card.render();
    assert!(text.contains("Launched seat: reviewer"), "{text}");
    assert!(
        text.contains("Parent seat: advisor, delegating only the reviewer seat"),
        "{text}"
    );
    assert!(
        text.contains("Expires: 8h after you answer (about 2026-11-01 17:00"),
        "{text}"
    );
    assert!(
        text.contains("bound to this launcher process only, never exported to your shell"),
        "{text}"
    );
    assert!(text.contains("Recommended default: n"), "{text}");
}

#[test]
fn inline_grant_lifetime_is_capped_by_max_grant_hours() {
    let dir = tempfile::tempdir().unwrap();
    let empty_home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[(
        "AIDA_HOME",
        Some(empty_home.path().to_str().unwrap()),
    )]);
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[seat]\nmax_grant_hours = 3\n",
    )
    .unwrap();
    assert_eq!(inline_grant_lifetime_hours(dir.path()), 3);
}
