// trace:BUG-2585 | ai:claude
use super::*;
use crate::agent_rotation::GENERATION_ENV;
use crate::gate::seat_authority::test_support::{mint_grant_for, put_grant, rewrite_grant};
use crate::gate::seat_authority::GRANT_ENV;
use crate::test_env::EnvVarsGuard;
use std::sync::atomic::{AtomicBool, Ordering};

const SUBJECT: &str = "relay-grant-fixture-user";

/// One env guard for the whole test: the fixture identity, a private grant
/// store, and every variable these tests set, restored on drop.
fn pinned_env(home: &Path) -> EnvVarsGuard {
    let mut env = EnvVarsGuard::snapshot(&[
        "AIDA_USER",
        "AIDA_HOME",
        GRANT_ENV,
        GENERATION_ENV,
        BIND_PID_ENV,
    ]);
    env.set_key("AIDA_USER", SUBJECT);
    env.set_key("AIDA_HOME", home.join("home"));
    env.unset_key(GRANT_ENV);
    env.unset_key(GENERATION_ENV);
    env.unset_key(BIND_PID_ENV);
    env
}

/// The environment a seat's command sees: its own grant and, under a relay,
/// the generation and the pid a renewed grant must be bound to.
fn seat_env(grant: &str, supervised: bool, relay_pid: Option<u32>) -> Environment {
    let mut pairs = vec![(GRANT_ENV, grant.to_string())];
    if supervised {
        pairs.push((GENERATION_ENV, "2".to_string()));
    }
    if let Some(pid) = relay_pid {
        pairs.push((BIND_PID_ENV, pid.to_string()));
    }
    Environment::from_pairs(pairs.into_iter().map(|(k, v)| (k.into(), v.into())))
}

/// Expire `id` an hour ago (issued a day before that).
fn expire(id: &str) -> SeatGrant {
    rewrite_grant(id, |g| {
        g.issued_at = Utc::now() - Duration::hours(25);
        g.expires_at = Utc::now() - Duration::hours(1);
    })
}

fn life(left: Duration, now: DateTime<Utc>) -> ParentLife {
    ParentLife {
        id: "52956836-0000-4000-8000-000000000000".into(),
        seat: "advisor".into(),
        expires_at: now + left,
        command: "eval \"$(aida role enter advisor --delegate-seat implementer --bind-pid 4242)\""
            .into(),
    }
}

/// The banner line always names the expiry; the renewal command and the
/// statusline suffix appear from two hours ahead, and after expiry.
#[test]
fn parent_life_is_shown_always_and_warns_two_hours_ahead() {
    let now = Utc::now();
    let far = life(Duration::hours(18), now);
    assert!(far
        .line(now)
        .starts_with("PARENT GRANT advisor · 18h00m left · expires "));
    assert!(!far.line(now).contains("renew"));
    assert_eq!(far.suffix(now), None);

    let near = life(Duration::minutes(119), now);
    assert!(near.line(now).contains("1h59m left"), "{}", near.line(now));
    assert!(near.line(now).contains(&near.command), "verbatim command");
    assert_eq!(
        near.suffix(now).as_deref(),
        Some("parent grant expires in 1h59m")
    );
    assert_eq!(life(WARN_WITHIN, now).suffix(now), None);

    let gone = life(Duration::minutes(-64), now);
    assert!(gone.line(now).contains("expired "), "{}", gone.line(now));
    assert!(gone.line(now).contains("keeps this session"));
    assert!(gone.line(now).contains(&gone.command));
    assert_eq!(gone.suffix(now).as_deref(), Some("parent grant expired"));
}

/// A relay whose grant expired holds (it neither ends the session nor
/// dies), prints the renewal command verbatim, and continues under a grant
/// a human renewed for it; a grant it cannot present is never adopted.
#[test]
fn an_expired_relay_grant_holds_until_a_renewed_grant_is_valid() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env(root);
    let old = mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    env.set_key(GRANT_ENV, &old);
    assert_eq!(hold(root, "implementer", false), None, "a live grant");

    expire(&old);
    let refusal = authority::validate_child_delegation(root, "implementer").unwrap_err();
    assert!(refusal.to_string().contains("expired at"), "{refusal}");
    let held = hold(root, "implementer", true).expect("an expired grant holds");
    assert!(held.resumes);
    assert_eq!(held.seat, "advisor");
    let pid = bind_pid(authority::read_grant(&old).ok().as_ref()).unwrap();
    let command =
        format!("eval \"$(aida role enter advisor --delegate-seat implementer --bind-pid {pid})\"");
    assert_eq!(held.command, command);
    for kept in [true, false] {
        let text = held.message("claude-implementer-1047", kept);
        assert!(text.contains(&command), "{text}");
        assert!(text.contains("claude-implementer-1047"), "{text}");
    }
    assert!(held.message("h", true).contains("keeps running"));

    // Renewed grants this relay may not use: another seat set, another
    // principal, or bound to a process this relay does not run under.
    let narrow = mint_grant_for(root, SUBJECT, "advisor", &["reviewer"]);
    let foreign = mint_grant_for(root, "someone-else", "advisor", &["implementer"]);
    let unbound = mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    rewrite_grant(&unbound, |g| g.binding = vec!["proc:1@1".into()]);
    assert!(hold(root, "implementer", true).is_some());
    assert_eq!(authority::current_grant_id().unwrap(), old);

    // The wait stops on Ctrl-C and has printed the command once.
    let stop = AtomicBool::new(false);
    let mut out = Vec::new();
    let proceeded = await_parent(
        root,
        "claude-implementer-1047",
        "implementer",
        &stop,
        &mut || stop.store(true, Ordering::SeqCst),
        &mut out,
    );
    assert!(!proceeded);
    let printed = String::from_utf8(out).unwrap();
    assert_eq!(printed.matches(&command).count(), 1, "{printed}");

    // A human renews the grant mid-wait: the relay adopts it and goes on.
    let stop = AtomicBool::new(false);
    let mut out = Vec::new();
    let mut renewed_id = None;
    let proceeded = await_parent(
        root,
        "claude-implementer-1047",
        "implementer",
        &stop,
        &mut || {
            if renewed_id.is_none() {
                renewed_id = Some(mint_grant_for(root, SUBJECT, "advisor", &["implementer"]));
            }
        },
        &mut out,
    );
    assert!(proceeded);
    let renewed_id = renewed_id.unwrap();
    assert_eq!(authority::current_grant_id().unwrap(), renewed_id);
    assert!(String::from_utf8(out).unwrap().contains("renewed"));
    let parent = authority::validate_child_delegation(root, "implementer").unwrap();
    assert_eq!(parent.id, renewed_id);
    assert_eq!(hold(root, "implementer", false), None);
    for id in [narrow, foreign, unbound] {
        assert_ne!(id, renewed_id);
    }
}

/// The turn-boundary check keeps the session without adopting anything,
/// and lets the rotation through once a renewed grant is usable.
#[test]
fn the_turn_boundary_check_keeps_the_session_while_the_grant_is_expired() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env(root);
    let old = mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    env.set_key(GRANT_ENV, &old);
    let mut announced = false;
    assert!(!hold_before_end(root, "h", "implementer", &mut announced));
    expire(&old);
    assert!(hold_before_end(root, "h", "implementer", &mut announced));
    assert!(announced);
    mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    assert!(!hold_before_end(root, "h", "implementer", &mut announced));
    assert_eq!(
        authority::current_grant_id().unwrap(),
        old,
        "no adoption here"
    );
}

/// A relay under a delegated grant holds too, but says the seat must be
/// started again: a renewed root is never adopted over a narrowed grant.
#[test]
fn a_delegated_relay_grant_holds_without_adopting_a_renewed_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env(root);
    let top = mint_grant_for(root, SUBJECT, "advisor", &["advisor", "implementer"]);
    let parent = authority::read_grant(&top).unwrap();
    let mut mid = parent.clone();
    mid.id = uuid::Uuid::new_v4().to_string();
    mid.parent_grant_id = Some(top.clone());
    mid.delegable_seats = vec!["implementer".into()];
    put_grant(&mid);
    env.set_key(GRANT_ENV, &mid.id);
    assert_eq!(hold(root, "implementer", true), None);
    expire(&top);
    mint_grant_for(root, SUBJECT, "advisor", &["advisor", "implementer"]);
    let held = hold(root, "implementer", true).expect("a delegated grant is not renewed");
    assert!(!held.resumes);
    assert!(!held.command.contains("--bind-pid"), "{}", held.command);
    assert!(held.message("h", false).contains("start h again"));
    assert_eq!(authority::current_grant_id().unwrap(), mid.id);
}

/// The seat of a supervised session reports its relay's grant: in the
/// banner line, the statusline suffix and `aida doctor`, for a baseline
/// seat too, and only under `--rotate`. Only a renewed grant that covers
/// the relay ends the warning.
#[test]
fn a_supervised_seat_reports_its_relays_grant() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = pinned_env(root);
    let top = mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    let mut kid = authority::read_grant(&top).unwrap();
    kid.id = uuid::Uuid::new_v4().to_string();
    kid.seat = "implementer".into();
    kid.parent_grant_id = Some(top.clone());
    kid.delegable_seats = Vec::new();
    put_grant(&kid);
    let now = Utc::now();
    assert_eq!(
        seat_life(root, &seat_env(&kid.id, false, None), now),
        None,
        "not a supervised session"
    );

    // The relay runs under this (test) process, and named it.
    let relay = std::process::id();
    let env = seat_env(&kid.id, true, Some(relay));
    let far = seat_life(root, &env, now).unwrap();
    assert_eq!(far.seat, "advisor");
    assert_eq!(far.suffix(now), None);
    assert!(doctor_scan(root, &env, now).is_empty());

    rewrite_grant(&top, |g| g.expires_at = now + Duration::minutes(90));
    let near = seat_life(root, &env, now).unwrap();
    assert_eq!(
        near.command,
        format!(
            "eval \"$(aida role enter advisor --delegate-seat implementer --bind-pid {relay})\""
        )
    );
    assert!(near.line(now).contains(&near.command));
    assert_eq!(
        near.suffix(now).as_deref(),
        Some("parent grant expires in 1h30m")
    );
    let findings = doctor_scan(root, &env, now);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].category, authority::DOCTOR_CATEGORY);
    assert!(findings[0].summary.starts_with("PARENT GRANT advisor"));
    assert!(findings[0].action.contains(&near.command));

    expire(&top);
    let gone = seat_life(root, &env, now).unwrap();
    assert_eq!(gone.suffix(now).as_deref(), Some("parent grant expired"));

    // A later grant for the same seat that the relay cannot present (the
    // operator entered the role again in another terminal, or renewed it
    // for another relay) leaves this relay held: the warning stays.
    let elsewhere = mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    rewrite_grant(&elsewhere, |g| {
        g.binding = vec!["proc:4194000@77".into(), "sid:4194000@77".into()]
    });
    let held = seat_life(root, &env, now).unwrap();
    assert_eq!(held.suffix(now).as_deref(), Some("parent grant expired"));
    assert_eq!(held.expires_at, gone.expires_at);
    assert_eq!(doctor_scan(root, &env, now).len(), 1);

    // Renewed for the relay (bound to the process it named): the seat
    // reports the renewed life. A seat whose relay named no process cannot
    // tell, and keeps warning.
    let renewed = mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    let covered = seat_life(root, &env, now).unwrap();
    assert_eq!(covered.suffix(now), None);
    assert_eq!(
        covered.expires_at,
        authority::read_grant(&renewed).unwrap().expires_at
    );
    assert!(doctor_scan(root, &env, now).is_empty());
    let unnamed = seat_life(root, &seat_env(&kid.id, true, None), now).unwrap();
    assert_eq!(unnamed.suffix(now).as_deref(), Some("parent grant expired"));
}

/// Ahead of expiry the relay mails the advisor and product seats once per
/// grant and expiry; a hold mails them again, once.
#[test]
fn the_relay_mails_the_advisor_and_product_seats_once_per_stage() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env(root);
    let old = mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    env.set_key(GRANT_ENV, &old);
    // The project has a product role, so its queue is addressable.
    let role = crate::project_role_file(root, "product");
    std::fs::create_dir_all(role.parent().unwrap()).unwrap();
    let stamp = Utc::now().to_rfc3339();
    std::fs::write(
        &role,
        format!("name = \"product\"\ncreated_at = \"{stamp}\"\nlast_active_at = \"{stamp}\"\n"),
    )
    .unwrap();
    let now = Utc::now();
    let to = |seat: &str| aida_core::mailbox::Recipient::Agent(seat.into());
    let mail = || aida_core::mailbox::read_local_messages(root).unwrap_or_default();

    warn_ahead(root, "claude-implementer-1047", "implementer", now);
    assert!(mail().is_empty(), "nothing with 24h left");

    rewrite_grant(&old, |g| g.expires_at = now + Duration::minutes(110));
    warn_ahead(root, "claude-implementer-1047", "implementer", now);
    warn_ahead(root, "claude-implementer-1047", "implementer", now);
    let soon = mail();
    assert_eq!(soon.len(), 2, "{soon:?}");
    for seat in ["advisor", "product"] {
        let message = soon.iter().find(|m| m.to == to(seat)).expect(seat);
        assert!(message.body.contains("claude-implementer-1047"));
        assert!(
            message
                .body
                .contains("aida role enter advisor --delegate-seat implementer --bind-pid"),
            "{}",
            message.body
        );
    }

    expire(&old);
    let mut announced = false;
    assert!(hold_before_end(
        root,
        "claude-implementer-1047",
        "implementer",
        &mut announced
    ));
    let mut again = false;
    assert!(hold_before_end(
        root,
        "claude-implementer-1047",
        "implementer",
        &mut again
    ));
    let all = mail();
    assert_eq!(all.len(), 4, "{all:?}");
    assert_eq!(
        all.iter().filter(|m| m.body.contains("expired at")).count(),
        2
    );
}

/// The relay grant expires DURING the relaunch countdown: the gate holds
/// again instead of letting the child grant's issuance fail, and goes on
/// only under a renewed grant that can delegate the seat.
#[test]
fn a_grant_that_expires_during_the_countdown_holds_again() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env(root);
    let old = mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    env.set_key(GRANT_ENV, &old);

    let stop = AtomicBool::new(false);
    let mut out = Vec::new();
    let mut countdowns = 0;
    let mut renewed_id: Option<String> = None;
    let proceeded = relaunch_gate(
        root,
        "claude-implementer-1047",
        "implementer",
        &stop,
        &mut || {
            // Only the second (held) wait renews the grant.
            if renewed_id.is_none() && hold(root, "implementer", false).is_some() {
                renewed_id = Some(mint_grant_for(root, SUBJECT, "advisor", &["implementer"]));
            }
        },
        &mut out,
        &mut |_, _, _| {
            countdowns += 1;
            if countdowns == 1 {
                expire(&old);
            }
            true
        },
    );
    assert!(proceeded);
    assert_eq!(countdowns, 2, "the countdown reruns after the hold");
    let renewed_id = renewed_id.expect("the gate held until a renewal");
    assert_eq!(authority::current_grant_id().unwrap(), renewed_id);
    let parent = authority::validate_child_delegation(root, "implementer").unwrap();
    assert_eq!(parent.id, renewed_id, "the next child grant can be issued");
    let printed = String::from_utf8(out).unwrap();
    assert_eq!(printed.matches("expired at").count(), 1, "{printed}");
    assert!(printed.contains("renewed"), "{printed}");

    // A Ctrl-C while held after the countdown ends the gate.
    let now_old = authority::current_grant_id().unwrap();
    let stop = AtomicBool::new(false);
    let proceeded = relaunch_gate(
        root,
        "h",
        "implementer",
        &stop,
        &mut || stop.store(true, Ordering::SeqCst),
        &mut Vec::new(),
        &mut |_, _, _| {
            expire(&now_old);
            true
        },
    );
    assert!(!proceeded);
}

/// The project's advisor and product queues are addressable.
fn seat_roles(root: &Path) {
    let role = crate::project_role_file(root, "product");
    std::fs::create_dir_all(role.parent().unwrap()).unwrap();
    let stamp = Utc::now().to_rfc3339();
    std::fs::write(
        &role,
        format!("name = \"product\"\ncreated_at = \"{stamp}\"\nlast_active_at = \"{stamp}\"\n"),
    )
    .unwrap();
}

/// Make mail delivery fail (the mailbox path is not a directory) until the
/// returned path is removed.
fn break_mailbox(root: &Path) -> std::path::PathBuf {
    let mailbox = aida_core::mailbox::local_mailbox_dir(root);
    std::fs::create_dir_all(mailbox.parent().unwrap()).unwrap();
    std::fs::write(&mailbox, b"not a mailbox\n").unwrap();
    mailbox
}

/// A failed first hold mail is retried while the same hold lasts, at the
/// turn boundary and at the relaunch gate, and is then sent once.
#[test]
fn a_failed_hold_mail_is_retried_until_delivered() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env(root);
    seat_roles(root);
    let old = mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    env.set_key(GRANT_ENV, &old);
    expire(&old);
    let expired_mail = || {
        aida_core::mailbox::read_local_messages(root)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| m.body.contains("expired at"))
            .count()
    };

    // Turn boundary: the first delivery fails, the next call sends it.
    let mailbox = break_mailbox(root);
    let mut announced = false;
    assert!(hold_before_end(root, "h", "implementer", &mut announced));
    assert!(announced);
    std::fs::remove_file(&mailbox).unwrap();
    assert_eq!(expired_mail(), 0, "the failed delivery sent nothing");
    assert!(hold_before_end(root, "h", "implementer", &mut announced));
    assert_eq!(expired_mail(), 2, "advisor and product, after recovery");
    assert!(hold_before_end(root, "h", "implementer", &mut announced));
    assert_eq!(expired_mail(), 2, "still once per grant");
}

/// The relaunch gate's wait retries a failed hold mail on its next poll.
#[test]
fn the_relaunch_wait_retries_a_failed_hold_mail() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env(root);
    seat_roles(root);
    let old = mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    env.set_key(GRANT_ENV, &old);
    expire(&old);
    let mailbox = break_mailbox(root);
    let stop = AtomicBool::new(false);
    let mut ticks = 0;
    let mut out = Vec::new();
    let proceeded = await_parent(
        root,
        "h",
        "implementer",
        &stop,
        &mut || {
            ticks += 1;
            if ticks == 1 {
                // The first poll's delivery has failed by now.
                assert!(
                    aida_core::mailbox::read_local_messages(root).is_err()
                        || aida_core::mailbox::read_local_messages(root)
                            .unwrap()
                            .is_empty()
                );
                std::fs::remove_file(&mailbox).unwrap();
            }
            if ticks == 6 {
                stop.store(true, Ordering::SeqCst);
            }
        },
        &mut out,
    );
    assert!(!proceeded);
    let mail = aida_core::mailbox::read_local_messages(root).unwrap_or_default();
    assert_eq!(
        mail.iter()
            .filter(|m| m.body.contains("expired at"))
            .count(),
        2,
        "{mail:?}"
    );
    let printed = String::from_utf8(out).unwrap();
    assert_eq!(printed.matches("expired at").count(), 1, "{printed}");
}
