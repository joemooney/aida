use super::*;

/// Seed a git-canonical store at `root` with one finding requirement;
/// returns the requirement's UUID.
fn seed_finding(root: &std::path::Path, spec_id: &str) -> Uuid {
    let backend = aida_core::GitBackend::new(root).unwrap();
    let mut store = RequirementsStore::new();
    let mut finding = Requirement::new("Review finding".into(), "A finding from review".into());
    finding.spec_id = Some(spec_id.to_string());
    let id = finding.id;
    store.requirements.push(finding);
    backend.save(&store).unwrap();
    id
}

/// A promoted finding lands in the `implementer` queue by default, and
// `queue_list` (the consumer side) sees it. trace:BUG-231
#[test]
fn promote_routes_to_implementer_queue_by_default() {
    // BUG-632: pin a UNIQUE AIDA_USER for the test's lifetime. The queue is
    // a per-user file, and `queue_promoted_finding` + `queue_list` both
    // resolve the user via the ambient-env `current_user_id(None)`. Under
    // parallel `cargo test` a sibling test mutating AIDA_USER/USER could slip
    // between the add and the list, making the two see different users → the
    // added item vanishes (false CI red). EnvVarGuard holds ENV_LOCK for the
    // whole body, so it both isolates the identity AND serialises against
    // every other env-mutating test. trace:BUG-632
    let _user = crate::test_env::EnvVarGuard::set("AIDA_USER", "bug632-promote-default-user");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let fid = seed_finding(&root, "TASK-900");

    let role = queue_promoted_finding(&root, fid, "TASK-900", None).unwrap();
    assert_eq!(role, "implementer");

    let user_id = current_user_id(None);
    let entries = Storage::new(&root).queue_list(&user_id, false).unwrap();
    let entry = entries
        .iter()
        .find(|e| e.requirement_id == fid)
        .expect("promoted finding must be present in the queue");
    assert_eq!(entry.for_role.as_deref(), Some("implementer"));
}

// STORY-652: `--node-name` is used verbatim (validated). trace:STORY-652
#[test]
fn resolve_node_name_flag_override_wins() {
    let got = resolve_node_name(Some("my-box"), "imac", "joe", "1").unwrap();
    assert_eq!(got, "my-box");
}

/// STORY-652: with no flag and no TTY (the test harness has no terminal on
/// stdin), resolve_node_name returns the computed default WITHOUT blocking
// on stdin. trace:STORY-652
#[test]
fn resolve_node_name_non_interactive_uses_default() {
    let got = resolve_node_name(None, "imac", "joe", "1").unwrap();
    assert_eq!(got, "imac-joe-1");
}

// STORY-652: an invalid (non-slug) name is rejected. trace:STORY-652
#[test]
fn resolve_node_name_rejects_invalid() {
    assert!(resolve_node_name(Some("bad name!"), "imac", "joe", "1").is_err());
}

/// TASK-859: appending the telemetry opt-out leaves prior config intact and
/// produces a `[telemetry] enabled = false` that `parse_telemetry_enabled`
/// (the live reader) recognises — proving the init prompt's write round-trips
// to the actual opt-out path. trace:TASK-859
#[test]
fn append_telemetry_disabled_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "[id_format]\npolicy = \"blocks-then-fallback\"\n").unwrap();
    append_telemetry_disabled(&cfg).unwrap();
    let body = std::fs::read_to_string(&cfg).unwrap();
    // Prior content survives.
    assert!(body.contains("[id_format]"));
    // The opt-out is readable by the live telemetry reader.
    assert_eq!(crate::usage::parse_telemetry_enabled(&body), Some(false));
}

// `--for <role>` overrides the default queue route. trace:BUG-231
#[test]
fn promote_honors_for_override() {
    // BUG-632: unique AIDA_USER + ENV_LOCK isolation (see
    // promote_routes_to_implementer_queue_by_default for the rationale).
    // trace:BUG-632
    let _user = crate::test_env::EnvVarGuard::set("AIDA_USER", "bug632-promote-for-override-user");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let fid = seed_finding(&root, "TASK-901");

    let role = queue_promoted_finding(&root, fid, "TASK-901", Some("reviewer")).unwrap();
    assert_eq!(role, "reviewer");

    let user_id = current_user_id(None);
    let entries = Storage::new(&root).queue_list(&user_id, false).unwrap();
    let entry = entries
        .iter()
        .find(|e| e.requirement_id == fid)
        .expect("promoted finding must be present in the queue");
    assert_eq!(entry.for_role.as_deref(), Some("reviewer"));
}

/// BUG-90 (BUG-89 follow-up): `--user` override wins over any env.
#[test]
fn current_user_id_override_wins() {
    assert_eq!(current_user_id(Some("alice")), "alice");
}

/// STORY-662: `aida list --user me` (and the positional `me`) resolves to
/// the current shell identity; any other value is a literal handle, casing
// of the `me` token is ignored. trace:STORY-662 | ai:claude
#[test]
fn resolve_list_user_filter_maps_me_to_current_user() {
    assert_eq!(resolve_list_user_filter("me", "joe"), "joe");
    assert_eq!(resolve_list_user_filter("ME", "joe"), "joe");
    assert_eq!(resolve_list_user_filter("Me", "joe"), "joe");
    // A real handle passes through unchanged — even one that contains "me".
    assert_eq!(resolve_list_user_filter("alice", "joe"), "alice");
    assert_eq!(resolve_list_user_filter("mehmet", "joe"), "mehmet");
}

// ── STORY-644: assignment + mention notifications ────────────────────────

/// The @mention parser extracts the right handles and conservatively
/// ignores email local-parts and `@` glued to a word (code-ish text).
#[test]
fn extract_mentions_picks_handles_ignores_emails_and_code() {
    // Basic: leading and mid-sentence mentions, deduped, first-seen order.
    assert_eq!(
        extract_mentions("@bob please look, and @carol too, also @bob again"),
        vec!["bob".to_string(), "carol".to_string()]
    );
    // Trailing sentence punctuation is trimmed.
    assert_eq!(extract_mentions("ping @dave."), vec!["dave".to_string()]);
    // Dotted / hyphenated handles survive (e.g. codex-implementer-1).
    assert_eq!(
        extract_mentions("hand off to @codex-implementer-1 now"),
        vec!["codex-implementer-1".to_string()]
    );
    // Email local-parts are NOT mentions (the `@` is preceded by a word char).
    assert!(extract_mentions("mail joe@example.com about it").is_empty());
    // A bare `@` with no following handle is ignored.
    assert!(extract_mentions("the @ symbol alone").is_empty());
    // Nothing at all.
    assert!(extract_mentions("no mentions here").is_empty());
}

/// The mention snippet collapses whitespace and truncates with an ellipsis.
#[test]
fn mention_snippet_collapses_and_truncates() {
    assert_eq!(mention_snippet("  hi   there  ", 80), "hi there");
    let s = mention_snippet("abcdefghij", 4);
    assert_eq!(s, "abcd…");
}

/// Assign-to-self sends no mailbox message: with `current_user_id == target`
/// the notify branch is skipped, so the local mailbox dir stays empty.
// trace:STORY-644 | ai:claude
#[test]
fn assign_to_self_sends_no_notification() {
    // Drive the same self-skip predicate the assign handler uses, then
    // assert send_notification writes nothing for the self case (and writes
    // exactly one message for a real assignee — the positive control).
    let tmp = std::env::temp_dir().join(format!("aida-story644-{}", uuid::Uuid::new_v4()));
    let store_path = tmp.join(".aida-store");
    std::fs::create_dir_all(&store_path).unwrap();
    let mailbox = tmp.join(".aida").join("mailbox");

    let me = "alice";
    let target_self = "alice";
    let target_other = "bob";

    // Self-assign → skipped (no send) per the handler's `assigner != target`.
    if me != target_self {
        send_notification(&store_path, me, target_self, "self".to_string());
    }
    let after_self = std::fs::read_dir(&mailbox).map(|d| d.count()).unwrap_or(0);
    assert_eq!(after_self, 0, "assign-to-self writes no mailbox message");

    // Assign to someone else → exactly one message lands.
    if me != target_other {
        send_notification(&store_path, me, target_other, "hello bob".to_string());
    }
    let after_other = std::fs::read_dir(&mailbox).map(|d| d.count()).unwrap_or(0);
    assert_eq!(after_other, 1, "assigning to another user sends one notice");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// BUG-679: the dead-letter known-identity set includes every canonical
/// role (case-insensitively) so a recipient like `advisor` / `ADVISOR` is
/// recognized, while an arbitrary unrecognized recipient is not — the
/// classification behind the send-time warning and the `mailbox list
/// --stranded` surface.
// trace:BUG-679 | ai:claude
#[test]
fn known_mailbox_identities_recognizes_roles_case_insensitively() {
    let tmp = tempfile::tempdir().unwrap();
    let known = known_mailbox_identities(tmp.path());
    for role in AGENT_ROLES {
        assert!(
            known.contains(&role.to_lowercase()),
            "canonical role `{role}` must be a known recipient"
        );
    }
    // The send/stranded paths lowercase the recipient before lookup, so a
    // mixed-case address for a known role still resolves.
    assert!(known.contains(&"ADVISOR".trim().to_lowercase()));
    // A recipient that matches no role/registered-agent (a typo'd name, a
    // wrong role) is NOT known → it would warn on send and show under
    // `--stranded`. A fresh uuid can't collide with any real identity, so
    // this stays hermetic regardless of the machine's global roles.
    let bogus = format!("no-such-recipient-{}", uuid::Uuid::new_v4());
    assert!(!known.contains(&bogus));
}

/// TASK-818: when both the stable per-instance name (`AIDA_USER`) and the
/// short agent TYPE name (`AIDA_AGENT_TYPE`) are set, `inbox_identities()`
/// returns BOTH — so a coordinator addressing the type (`--to codex`) AND
/// one addressing the stable name (`--to codex-implementer-1`) both reach
/// the mailbox. (`AIDA_SESSION_ROLE` cleared so it doesn't perturb the set.)
#[test]
fn inbox_identities_unions_stable_name_and_agent_type() {
    let _g = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_USER", "codex-implementer-1"),
        ("AIDA_AGENT_TYPE", "codex"),
        ("AIDA_SESSION_ROLE", ""),
    ]);
    let ids = inbox_identities();
    assert!(
        ids.iter().any(|i| i == "codex-implementer-1"),
        "stable AIDA_USER name must be an inbox identity: {ids:?}"
    );
    assert!(
        ids.iter().any(|i| i == "codex"),
        "short AIDA_AGENT_TYPE name must be an inbox identity: {ids:?}"
    );
}

/// TASK-818: with only `AIDA_USER` set (no `AIDA_AGENT_TYPE`), the identity
/// set is just the stable name — the type union must not invent an entry.
#[test]
fn inbox_identities_stable_name_only_when_no_agent_type() {
    let _g = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_USER", "solo-user"),
        ("AIDA_AGENT_TYPE", ""),
        ("AIDA_SESSION_ROLE", ""),
    ]);
    let ids = inbox_identities();
    assert_eq!(
        ids,
        vec!["solo-user".to_string()],
        "no AIDA_AGENT_TYPE must yield only the stable name: {ids:?}"
    );
}

/// TASK-818: a message addressed to the short TYPE name (`--to codex`) is
/// delivered to the agent's inbox, mirroring how brief routing reaches the
/// short type name. Exercises the real delivery predicate
/// (`build_notice` over the identity union) rather than only the id list.
#[test]
fn message_to_agent_type_reaches_inbox() {
    use aida_core::mailbox::{build_notice, Intent, Message, Recipient};
    let _g = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_USER", "codex-implementer-1"),
        ("AIDA_AGENT_TYPE", "codex"),
        ("AIDA_SESSION_ROLE", ""),
    ]);
    let identities = inbox_identities();
    let to_type = Message {
        id: "m1".to_string(),
        thread_id: "t1".to_string(),
        from: "coordinator".to_string(),
        to: Recipient::Agent("codex".to_string()),
        timestamp: 100,
        in_reply_to: None,
        body: "addressed to the short type name".to_string(),
        urgent: false,
        intent: Intent::Request,
        retracted: false,
        deleted: false,
    };
    let watermarks = std::collections::HashMap::new();
    let summary = build_notice(
        identities.iter().map(String::as_str),
        std::slice::from_ref(&to_type),
        &watermarks,
        aida_core::mailbox::NOTICE_DEFAULT_CAP,
    );
    assert_eq!(
        summary.total, 1,
        "a --to <type> message must be visible to the agent: {summary:?}"
    );
}

// trace:STORY-585 | ai:claude
#[test]
fn render_mailbox_notice_frames_count_urgency_overflow_and_guidance() {
    use aida_core::mailbox::{Intent, NoticeItem, NoticeSummary};
    let summary = NoticeSummary {
        total: 4,
        urgent: 1,
        overflow: 2,
        shown: vec![
            NoticeItem {
                id: "a".into(),
                from: "codex".into(),
                thread_id: "t".into(),
                subject: "heads up: rebasing forge".into(),
                urgent: false,
                intent: Intent::Fyi,
            },
            NoticeItem {
                id: "b".into(),
                from: "advisor".into(),
                thread_id: "t".into(),
                subject: "STOP — CI is red".into(),
                urgent: true,
                intent: Intent::Request,
            },
        ],
    };
    let out = render_mailbox_notice(&summary, &["joe".to_string(), "advisor".to_string()]);
    assert!(out.contains("4 unread"), "count: {out}");
    assert!(out.contains("(1 urgent)"), "urgent count: {out}");
    assert!(out.contains("joe/advisor"), "identity set: {out}");
    assert!(out.contains("heads up: rebasing forge") && out.contains("from codex"));
    assert!(
        out.contains(&format!("{} ", crate::glyph(crate::glyphs::Glyph::Warning)))
            && out.contains("STOP — CI is red"),
        "urgent mark: {out}"
    );
    // Actionable intent is tagged; the fyi default stays unmarked (TASK-790).
    assert!(out.contains("[request]"), "actionable intent tag: {out}");
    assert!(!out.contains("[fyi]"), "fyi stays unmarked: {out}");
    assert!(out.contains("…and 2 more."), "overflow: {out}");
    assert!(out.contains("aida mailbox inbox"), "ack guidance: {out}");
    assert!(
        out.contains("not a command"),
        "trust-boundary framing: {out}"
    );
    // Plain text only — no ANSI escapes (it goes into a context window).
    assert!(!out.contains('\u{1b}'), "no ANSI: {out:?}");
}

// trace:STORY-583 | ai:codex
#[test]
fn mailbox_policy_defaults_allowed_and_reads_false_overrides() {
    use aida_core::mailbox::ActOnMail;
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        mailbox_policy(dir.path()),
        MailboxPolicy {
            allow_retract: true,
            allow_delete: true,
            act_on_mail: ActOnMail::SurfaceAndRecommend,
        }
    );

    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[mailbox]\nallow_retract = false\nallow_delete = false\nact_on_mail = \"escalate-per-cascade\"\n",
        )
        .unwrap();
    assert_eq!(
        mailbox_policy(dir.path()),
        MailboxPolicy {
            allow_retract: false,
            allow_delete: false,
            act_on_mail: ActOnMail::EscalatePerCascade,
        }
    );
}

// trace:TASK-782 | ai:claude
#[test]
fn mailbox_policy_act_on_mail_typo_falls_back_to_safe_default() {
    use aida_core::mailbox::ActOnMail;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[mailbox]\nact_on_mail = \"yolo-auto-act\"\n",
    )
    .unwrap();
    assert_eq!(
        mailbox_policy(dir.path()).act_on_mail,
        ActOnMail::SurfaceAndRecommend,
        "an unrecognized value must never escalate autonomy"
    );
}

// trace:STORY-583 | ai:codex
#[test]
fn mailbox_mutation_allowed_for_sender_or_operator_only() {
    let msg = aida_core::mailbox::Message {
        id: "m1".into(),
        thread_id: "t1".into(),
        from: "codex".into(),
        to: aida_core::mailbox::Recipient::Agent("claude".into()),
        timestamp: 1,
        in_reply_to: None,
        body: "hi".into(),
        urgent: false,
        intent: aida_core::mailbox::Intent::Fyi,
        retracted: false,
        deleted: false,
    };
    assert!(mailbox_mutation_allowed(&msg, "codex", "joe"));
    assert!(mailbox_mutation_allowed(&msg, "joe", "joe"));
    assert!(!mailbox_mutation_allowed(&msg, "other", "joe"));
}

// ---- TASK-754: `aida add --queue` eligibility gate ----

/// AC1/AC2: an Approved spec (the file-approve-and-queue happy path) is
/// allowed onto the queue — no refusal.
#[test]
fn queue_at_filing_allows_approved() {
    assert_eq!(
        queue_at_filing_refusal(&RequirementStatus::Approved, false, None),
        None
    );
}

// ---- TASK-784: `aida whoami` user-id source label ----

/// AIDA_USER wins over USER/USERNAME — the source label names it, matching
/// `current_user_id`'s resolution precedence (BUG-89 queue identity).
// trace:TASK-784
#[test]
fn whoami_user_source_prefers_aida_user() {
    assert_eq!(
        crate::session_misc_cmd::whoami_user_source(Some("alice"), Some("bob"), Some("carol")),
        "from AIDA_USER"
    );
}

/// USER is used when AIDA_USER is unset (or empty); USERNAME when both
// above are unset; "default" when none resolve. trace:TASK-784
#[test]
fn whoami_user_source_falls_through_to_user_then_username_then_default() {
    assert_eq!(
        crate::session_misc_cmd::whoami_user_source(None, Some("bob"), None),
        "from USER"
    );
    assert_eq!(
        crate::session_misc_cmd::whoami_user_source(Some(""), Some("bob"), None),
        "from USER"
    );
    assert_eq!(
        crate::session_misc_cmd::whoami_user_source(None, None, Some("carol")),
        "from USERNAME"
    );
    assert_eq!(
        crate::session_misc_cmd::whoami_user_source(None, None, None),
        "default"
    );
    assert_eq!(
        crate::session_misc_cmd::whoami_user_source(Some(""), Some(""), Some("")),
        "default"
    );
}

/// AC5: a requested-Approved intake that the advisor-authority gate
/// downgraded to Draft is refused as a downgrade (re-run as advisor), not
/// as a plain status problem.
#[test]
fn queue_at_filing_refuses_downgraded_as_authority() {
    assert_eq!(
        queue_at_filing_refusal(&RequirementStatus::Draft, true, None),
        Some(QueueAtFilingRefusal::Downgraded)
    );
}

/// AC2: an explicit non-Approved status (no downgrade) is refused as
/// not-enqueueable, matching `aida backlog groom`'s Approved-only policy.
#[test]
fn queue_at_filing_refuses_explicit_draft() {
    assert_eq!(
        queue_at_filing_refusal(&RequirementStatus::Draft, false, None),
        Some(QueueAtFilingRefusal::NotApproved)
    );
}

/// A non-Draft, non-Approved status (e.g. Planned) is likewise refused as
/// not-enqueueable — only Approved grooms.
#[test]
fn queue_at_filing_refuses_non_approved_status() {
    assert_eq!(
        queue_at_filing_refusal(&RequirementStatus::Planned, false, None),
        Some(QueueAtFilingRefusal::NotApproved)
    );
}

// ---- BUG-631: dispatch-vs-request route classification ----

/// The request/review routes (advisor / human / reviewer, plus the
/// `dialog`→`advisor` and casing aliases) are exempt from the
/// dispatch-for-execution advisor-authority gate.
#[test]
fn request_routes_are_exempt_from_dispatch_gate() {
    for route in [
        "advisor", "human", "reviewer", "dialog", "Human", "REVIEWER",
    ] {
        assert!(
            !for_target_requires_dispatch_authority(Some(route)),
            "{route} should be a request route (exempt)"
        );
        assert!(
            for_route_is_request_only(route),
            "{route} should classify as request-only"
        );
    }
}

/// Execution-dispatch routes (implementer, unknown/custom roles) and the
/// unrouted cases (`--for any`, no `--for`) stay gated.
#[test]
fn dispatch_and_unrouted_targets_require_authority() {
    for route in ["implementer", "architect", "integrator", "whoknows"] {
        assert!(
            for_target_requires_dispatch_authority(Some(route)),
            "{route} should require dispatch authority"
        );
        assert!(
            !for_route_is_request_only(route),
            "{route} should not classify as request-only"
        );
    }
    // Unrouted: explicit `--for any` and the absent default both gated.
    assert!(for_target_requires_dispatch_authority(Some("any")));
    assert!(for_target_requires_dispatch_authority(None));
}

// trace:BUG-631
/// At the filing surface: a draft routed `--for advisor` is allowed onto the
/// queue even after the authority gate downgraded it, while the same
/// downgraded draft with no request route stays refused.
#[test]
fn queue_at_filing_request_route_bypasses_downgrade() {
    // Request route → allowed despite the downgrade and non-Approved status.
    assert_eq!(
        queue_at_filing_refusal(&RequirementStatus::Draft, true, Some("advisor")),
        None
    );
    assert_eq!(
        queue_at_filing_refusal(&RequirementStatus::Draft, false, Some("reviewer")),
        None
    );
    // Execution route → still gated as before.
    assert_eq!(
        queue_at_filing_refusal(&RequirementStatus::Draft, true, Some("implementer")),
        Some(QueueAtFilingRefusal::Downgraded)
    );
}

// ---- BUG-528: `aida add --queue` routing ----

/// Default (no `--for`): `add --queue` routes to the `implementer` queue —
/// the common target for filed work — NOT the filer's session role.
// trace:BUG-528
#[test]
fn add_queue_routes_to_implementer_by_default() {
    assert_eq!(add_queue_route_role(None), Some("implementer".to_string()));
}

/// `--for <role>` routes to that role's queue, canonicalized (so `dialog`
// normalizes to `advisor`, matching `aida queue add --for`). trace:BUG-528
#[test]
fn add_queue_routes_to_explicit_role_canonicalized() {
    assert_eq!(
        add_queue_route_role(Some("advisor")),
        Some("advisor".to_string())
    );
    assert_eq!(
        add_queue_route_role(Some("reviewer")),
        Some("reviewer".to_string())
    );
    // dialog is the deprecated alias for advisor — canonicalized on route.
    assert_eq!(
        add_queue_route_role(Some("dialog")),
        Some("advisor".to_string())
    );
}

/// `--for any` leaves the spec unrouted (explicit opt-out), mirroring the
// `aida queue add --for any` write-side semantic. trace:BUG-528
#[test]
fn add_queue_for_any_is_unrouted() {
    assert_eq!(add_queue_route_role(Some("any")), None);
}

// ---- TASK-618: default-queue cross-machine collision predicate ----

/// Non-"default" user_ids shard cleanly per user, so a foreign
/// fingerprint there is NOT a collision (alice.yaml vs bob.yaml never
/// share a file). The predicate must stay silent regardless.
#[test]
fn collision_predicate_ignores_non_default_user() {
    assert_eq!(
        default_queue_collision_fingerprint("alice", "host-a", [Some("host-b")]),
        None
    );
}

/// "default" + an existing entry from a DIFFERENT machine = the hazard:
/// return the foreign fingerprint to name in the warning.
#[test]
fn collision_predicate_flags_default_foreign_machine() {
    assert_eq!(
        default_queue_collision_fingerprint("default", "host-a", [Some("host-a"), Some("host-b")]),
        Some("host-b".to_string())
    );
}

/// "default" but every recorded fingerprint is THIS machine = no
/// collision (the common single-machine-unconfigured case).
#[test]
fn collision_predicate_silent_when_all_same_machine() {
    assert_eq!(
        default_queue_collision_fingerprint("default", "host-a", [Some("host-a"), Some("host-a")]),
        None
    );
}

/// Entries with no recorded fingerprint (pre-TASK-618 / non-CLI writers)
/// and empty strings are ignored — they can't be attributed to a machine,
/// so they never raise a false alarm.
#[test]
fn collision_predicate_ignores_unknown_fingerprints() {
    assert_eq!(
        default_queue_collision_fingerprint("default", "host-a", [None, Some("")]),
        None
    );
    // An empty on-disk file (no entries) is silent too.
    let empty: [Option<&str>; 0] = [];
    assert_eq!(
        default_queue_collision_fingerprint("default", "host-a", empty),
        None
    );
}

/// BUG-90: with no override, `AIDA_USER` is the highest-precedence env
/// source (over the ambient `USER`/`USERNAME`).
#[test]
fn current_user_id_prefers_aida_user_env() {
    let _g = crate::test_env::EnvVarGuard::set("AIDA_USER", "env-bob");
    assert_eq!(current_user_id(None), "env-bob");
}

/// BUG-605: the groom/handoff queue identity SKIPS the agent's `AIDA_USER`
/// mailbox id and targets the draining shell `USER`, so groomed work lands
// where the human's drain looks. trace:BUG-605 | ai:claude
#[test]
fn drain_queue_user_id_skips_aida_user_for_shell_user() {
    // Both vars under ONE guard: EnvVarGuard holds ENV_LOCK for its whole
    // lifetime, so two separate `set` calls deadlock (CI hung 25m on this).
    // EnvVarsGuard takes the lock once for several keys. trace:BUG-611
    let _g =
        crate::test_env::EnvVarsGuard::set(&[("AIDA_USER", "claude-advisor-1"), ("USER", "joe")]);
    // current_user_id prefers the agent's mailbox id…
    assert_eq!(current_user_id(None), "claude-advisor-1");
    // …but drain-queue work targets the human/shell USER (the drainer).
    assert_eq!(drain_queue_user_id(None), "joe");
    // An explicit --user still overrides.
    assert_eq!(drain_queue_user_id(Some("alice")), "alice");
}

/// BUG-90 acceptance: queue add + queue list from the same shell show the
/// just-added item WITHOUT a `--user` flag (both route through the single
/// `current_user_id` resolver, BUG-89's fix), and a different user_id does
/// not see it.
#[test]
fn queue_add_then_list_same_shell_is_consistent() {
    // BUG-632: this test was flaky under parallel `cargo test --workspace`.
    // The queue is a per-user file; both `queue_promoted_finding` (the add)
    // and the `current_user_id(None)` below (the list) resolve the user from
    // the ambient AIDA_USER/USER env. A sibling test mutating those vars
    // between the add and the list made the add-user and list-user diverge,
    // so the just-added item appeared absent (false CI red). Pinning a UNIQUE
    // AIDA_USER under EnvVarGuard isolates this test's identity AND holds
    // ENV_LOCK for the whole body, serialising it against every other
    // env-mutating test so nothing can race in between. The value is distinct
    // from the "a-different-user" literal below so the negative assertion
    // stays meaningful. trace:BUG-632
    let _user = crate::test_env::EnvVarGuard::set("AIDA_USER", "bug632-same-shell-consistent-user");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let fid = seed_finding(&root, "TASK-902");
    // Add via the real route (uses current_user_id internally).
    queue_promoted_finding(&root, fid, "TASK-902", None).unwrap();

    let me = current_user_id(None);
    let mine = Storage::new(&root).queue_list(&me, false).unwrap();
    assert!(
        mine.iter().any(|e| e.requirement_id == fid),
        "same-shell add+list must show the item without --user"
    );

    let theirs = Storage::new(&root)
        .queue_list("a-different-user", false)
        .unwrap();
    assert!(
        !theirs.iter().any(|e| e.requirement_id == fid),
        "a different user_id must not see another user's queue item"
    );
}

/// Build a `CachedGitBackend` over a seeded store dir, mirroring the
// production `handle_git_backend_command` setup. trace:STORY-639 | ai:claude
#[cfg(test)]
fn test_cached_backend(root: &std::path::Path) -> aida_core::CachedGitBackend {
    let inner = aida_core::GitBackend::new(root).unwrap();
    let cache_path = aida_core::CachedGitBackend::default_cache_path(root);
    aida_core::CachedGitBackend::with_inner(inner, &cache_path).unwrap()
}

/// Seed a single edge-free spec with the given spec_id; returns its UUID.
// Used by the BUG-615 combined-flag regression test. trace:BUG-615 | ai:claude
#[cfg(test)]
fn seed_plain_spec(root: &std::path::Path, spec_id: &str) -> Uuid {
    let backend = aida_core::GitBackend::new(root).unwrap();
    let mut store = backend.load().unwrap_or_default();
    let mut req = Requirement::new(format!("Spec {spec_id}"), "desc".into());
    req.spec_id = Some(spec_id.to_string());
    let id = req.id;
    store.requirements.push(req);
    backend.save(&store).unwrap();
    id
}

/// BUG-615 regression: `aida add --parent X --blocked-by Y` in a SINGLE
/// invocation must persist BOTH edges. The bug was that the `--parent`
/// block re-saved a STALE pre-blocked-by snapshot of the child
/// (`let mut child = last.clone()`), clobbering the BlockedBy edge that
/// `add_blocked_by_edge` (the `--blocked-by` step) had just written.
///
/// This reproduces the exact handler sequence: first apply the blocked-by
/// edge, then perform the parent-edge read-modify-write the way the fixed
/// handler does (re-LOAD the freshly-saved child rather than reusing the
/// stale snapshot), and assert the child carries BOTH a Child edge to the
/// parent AND a BlockedBy edge to the blocker. With the pre-fix
/// `last.clone()` snapshot this assertion fails: the BlockedBy edge is gone.
// trace:BUG-615 | ai:claude
#[test]
fn add_parent_and_blocked_by_combined_keeps_both_edges() {
    use aida_core::models::{Relationship, RelationshipType};
    use aida_core::DatabaseBackend;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");

    // Seed the three actors: the new child, its parent, and its blocker.
    let child_uuid = seed_plain_spec(&root, "STORY-615");
    let parent_uuid = seed_plain_spec(&root, "EPIC-615");
    let blocker_uuid = seed_plain_spec(&root, "TASK-615");

    let backend = test_cached_backend(&root);

    // 1. The `--blocked-by` step (runs first in the real handler): writes
    //    the BlockedBy edge on the child + the inverse Blocks edge.
    add_blocked_by_edge(&backend, "STORY-615", "TASK-615")
        .expect("add_blocked_by_edge should persist the blocked-by edge");

    // Confirm the blocked-by edge landed before the parent step runs.
    let after_block = backend
        .get_requirement_by_spec_id("STORY-615")
        .unwrap()
        .unwrap();
    assert!(
        after_block
            .relationships
            .iter()
            .any(|r| matches!(r.rel_type, RelationshipType::BlockedBy)
                && r.target_id == blocker_uuid),
        "precondition: blocked-by edge must be present after the --blocked-by step"
    );

    // 2. The `--parent` step, mirroring the FIXED handler: re-LOAD the
    //    freshly-saved child (so it carries the blocked-by edge), append
    //    the Child edge, and save once.
    let mut child = backend
        .get_requirement_by_spec_id("STORY-615")
        .unwrap()
        .unwrap();
    child.relationships.push(Relationship {
        target_id: parent_uuid,
        rel_type: RelationshipType::Child,
        created_at: Some(chrono::Utc::now()),
        created_by: None,
    });
    backend.update_requirement(&child).unwrap();

    // 3. Load the persisted child back from the store and assert BOTH
    //    edges survive — this is the crux of BUG-615.
    let reloaded = backend
        .get_requirement_by_spec_id("STORY-615")
        .unwrap()
        .unwrap();
    let _ = child_uuid; // child's UUID is stable across the reload
    assert!(
        reloaded
            .relationships
            .iter()
            .any(|r| matches!(r.rel_type, RelationshipType::Child) && r.target_id == parent_uuid),
        "child edge to the parent must persist"
    );
    assert!(
        reloaded
            .relationships
            .iter()
            .any(|r| matches!(r.rel_type, RelationshipType::BlockedBy)
                && r.target_id == blocker_uuid),
        "BUG-615: blocked-by edge must NOT be clobbered by the parent re-save"
    );
}

/// Seed a single spec that carries one DANGLING relationship — an edge
/// whose `target_id` resolves to no requirement in the store. Mirrors the
/// real-world state `aida rel list --dangling` reports.
// trace:BUG-573 | ai:claude
#[cfg(test)]
fn seed_spec_with_dangling_rel(root: &std::path::Path, spec_id: &str) {
    let backend = aida_core::GitBackend::new(root).unwrap();
    let mut store = RequirementsStore::new();
    let mut req = Requirement::new("Has a dangling edge".into(), "desc".into());
    req.spec_id = Some(spec_id.to_string());
    req.relationships.push(aida_core::models::Relationship {
        rel_type: RelationshipType::References,
        // A random UUID that no requirement in the store owns → dangling.
        target_id: uuid::Uuid::new_v4(),
        created_at: None,
        created_by: None,
    });
    store.requirements.push(req);
    backend.save(&store).unwrap();
}

/// BUG-573: `aida rel list` is a READ-ONLY query, so it must exit 0 (return
/// `Ok`) whenever it COMPLETES — even when it surfaces dangling-relationship
/// warnings or when the named spec doesn't resolve. Reserving a non-zero
/// exit for warnings/empty-results was the single biggest distortion in
/// `aida usage --errors` (~50% phantom failures across 8k calls) and
/// papercut every hook/loop that called it. Genuine bad-arguments still
// bail non-zero. trace:BUG-573 | ai:claude
#[test]
fn rel_list_exits_zero_on_dangling_and_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    seed_spec_with_dangling_rel(&root, "TASK-573");
    let backend = test_cached_backend(&root);

    // Global listing over a store that CONTAINS a dangling edge: the
    // warning is surfaced (in the row output) but the query completed, so
    // it must return Ok — not a non-zero exit.
    handle_rel_list_modern(&backend, &root, None, None, None, false, true, Some(0))
        .expect("global rel list over a store with a dangling edge must exit 0");

    // `--dangling` explicitly asks for the dangling edges; finding them is a
    // successful read, not a failure.
    handle_rel_list_modern(&backend, &root, None, None, None, true, true, Some(0))
        .expect("`rel list --dangling` finding dangling edges must exit 0");

    // Source-scoped query whose spec resolves: trivially Ok.
    handle_rel_list_modern(
        &backend,
        &root,
        Some("TASK-573"),
        None,
        None,
        false,
        true,
        Some(0),
    )
    .expect("rel list for a resolvable source must exit 0");

    // BUG-573 core: a source spec that DOESN'T resolve is an empty
    // read-only result (the not-found message goes to stderr), NOT a
    // non-zero exit.
    handle_rel_list_modern(
        &backend,
        &root,
        Some("NOPE-999"),
        None,
        None,
        false,
        true,
        Some(0),
    )
    .expect("rel list for a not-found source must exit 0 (empty read-only result)");

    // Same for an unresolvable --target.
    handle_rel_list_modern(
        &backend,
        &root,
        None,
        Some("NOPE-999"),
        None,
        false,
        true,
        Some(0),
    )
    .expect("rel list for a not-found target must exit 0 (empty read-only result)");

    // Guardrail: a genuine bad-argument (source AND target both) STILL
    // fails — the fix must not swallow real errors.
    let bad = handle_rel_list_modern(
        &backend,
        &root,
        Some("TASK-573"),
        Some("TASK-573"),
        None,
        false,
        true,
        Some(0),
    );
    assert!(
        bad.is_err(),
        "passing both source and target is a real bad-argument error and must stay non-zero"
    );
}

/// STORY-639: `aida assign --to <user>` sets the assignee AND routes the
/// spec into that user's queue. Re-running is idempotent (no duplicate
/// queue entry). `aida unassign` clears the assignee and, by default,
/// leaves the spec in the queue; `--from-queue` removes it.
// trace:STORY-639 | ai:claude
#[test]
fn assign_sets_assignee_and_routes_queue_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let fid = seed_finding(&root, "TASK-639");
    let backend = test_cached_backend(&root);

    // Assign to alice.
    assign_cmd::handle_assign_command("TASK-639", "alice", &backend, &root).unwrap();
    let req = backend
        .get_requirement_by_spec_id("TASK-639")
        .unwrap()
        .unwrap();
    assert_eq!(req.assignee.as_deref(), Some("alice"));
    let alice_q = Storage::new(&root).queue_list("alice", false).unwrap();
    assert_eq!(
        alice_q.iter().filter(|e| e.requirement_id == fid).count(),
        1,
        "assign must route the spec into alice's queue exactly once"
    );

    // Idempotent re-assign: no duplicate queue entry, field unchanged.
    assign_cmd::handle_assign_command("TASK-639", "alice", &backend, &root).unwrap();
    let alice_q = Storage::new(&root).queue_list("alice", false).unwrap();
    assert_eq!(
        alice_q.iter().filter(|e| e.requirement_id == fid).count(),
        1,
        "re-assigning to the same user must not duplicate the queue entry"
    );

    // Unassign (default): assignee cleared, queue entry retained.
    assign_cmd::handle_unassign_command("TASK-639", false, &backend, &root).unwrap();
    let req = backend
        .get_requirement_by_spec_id("TASK-639")
        .unwrap()
        .unwrap();
    assert_eq!(req.assignee, None, "unassign must clear the assignee");
    let alice_q = Storage::new(&root).queue_list("alice", false).unwrap();
    assert_eq!(
        alice_q.iter().filter(|e| e.requirement_id == fid).count(),
        1,
        "default unassign leaves the spec in the queue"
    );

    // Re-assign then unassign --from-queue: queue entry removed too.
    assign_cmd::handle_assign_command("TASK-639", "alice", &backend, &root).unwrap();
    assign_cmd::handle_unassign_command("TASK-639", true, &backend, &root).unwrap();
    let alice_q = Storage::new(&root).queue_list("alice", false).unwrap();
    assert!(
        !alice_q.iter().any(|e| e.requirement_id == fid),
        "--from-queue must remove the spec from the queue"
    );

    // Unassigning an already-unassigned spec errors.
    assert!(
        assign_cmd::handle_unassign_command("TASK-639", false, &backend, &root).is_err(),
        "unassigning an unassigned spec must error"
    );
}

/// STORY-672: the fleet-wide `--all-users` view aggregates EVERY user's
/// queue, not just the current shell identity. Seed two distinct users'
/// queues (alice + bob), then verify the aggregation enumerates both users
/// and that `render_all_users_queue` runs read-only over the combined set
/// without error. The default per-user `queue_list` only ever sees one
// user; the fleet view must span them. trace:STORY-672
#[test]
fn all_users_view_aggregates_every_users_queue() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");

    // Two specs, each queued for a different user.
    let alice_spec = seed_plain_spec(&root, "TASK-6720");
    let bob_spec = seed_plain_spec(&root, "TASK-6721");

    let storage = Storage::new(&root);
    let now = chrono::Utc::now();
    storage
        .queue_add(aida_core::QueueEntry {
            user_id: "alice".to_string(),
            requirement_id: alice_spec,
            position: 1000,
            added_by: "alice".to_string(),
            note: None,
            added_at: now,
            for_role: Some("implementer".to_string()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        })
        .unwrap();
    storage
        .queue_add(aida_core::QueueEntry {
            user_id: "bob".to_string(),
            requirement_id: bob_spec,
            position: 1000,
            added_by: "bob".to_string(),
            note: None,
            added_at: now,
            for_role: Some("reviewer".to_string()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        })
        .unwrap();

    // The per-user default would only ever see ONE of these. The fleet
    // enumeration must surface BOTH users.
    let mut users = storage.queue_users().unwrap();
    users.sort();
    assert_eq!(
        users,
        vec!["alice".to_string(), "bob".to_string()],
        "queue_users must enumerate every user with a stored queue"
    );

    // Each user's queue carries exactly its own entry — proving the
    // aggregate spans queues the current shell's `queue_list` never reads.
    assert_eq!(
        storage.queue_list("alice", false).unwrap().len(),
        1,
        "alice's queue holds her single entry"
    );
    assert_eq!(
        storage.queue_list("bob", false).unwrap().len(),
        1,
        "bob's queue holds his single entry"
    );

    // The fleet renderer runs read-only over the combined set without
    // error (it prints; we assert it doesn't blow up and leaves the
    // underlying queues untouched).
    let backend = test_cached_backend(&root);
    let summaries = backend
        .list_summaries(&aida_core::ListFilter::default())
        .unwrap();
    render_all_users_queue(&storage, &summaries, None, false, false).unwrap();

    // Read-only: both queues are unchanged after rendering.
    assert_eq!(storage.queue_list("alice", false).unwrap().len(), 1);
    assert_eq!(storage.queue_list("bob", false).unwrap().len(), 1);
}

/// Failure injection: pointing the store at a path that is neither a
/// git directory nor a SQLite file makes `Storage::queue_backend` bail.
/// The error must propagate (non-zero exit) instead of a silent success,
// and name the route + the not-promoted outcome. trace:BUG-231
#[test]
fn promote_surfaces_queue_add_failure() {
    let dir = tempfile::tempdir().unwrap();
    // A plain file (no `.db`/`.sqlite` extension, not a directory) —
    // `queue_backend()` refuses it.
    let bogus = dir.path().join("not-a-store");
    std::fs::write(&bogus, b"not a store").unwrap();

    let err = queue_promoted_finding(&bogus, Uuid::now_v7(), "TASK-902", None)
        .expect_err("queue-add against a non-store path must fail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not promoted") && msg.contains("implementer"),
        "error must name the failed route and the not-promoted outcome: {msg}"
    );
}

// ── BUG-625: statusline mailbox count clears when the inbox is read ──────
//
// The operator's #1 recurring frustration: the statusline kept showing a
// high urgent-mailbox count that never cleared, no matter how many times the
// inbox was read. Root cause: `read_urgent_unread_count` (the statusline)
// read only the LOCAL `.aida/mailbox/` layer, while `aida mailbox inbox`
// reads the MERGED (local + canonical orphan-store) set and advances each
// identity's watermark against the MERGED newest. When the sets differed the
// watermark a read advanced could not clear a count computed over a
// different set. These tests pin: (1) the count SEES a canonical-only urgent
// message (proving the merge), and (2) advancing the watermark to the
// merged-newest — exactly what the inbox-read does — drives the count to 0.
// trace:BUG-625 | ai:claude

/// Build an urgent broadcast `Message` with the given id, sender, timestamp.
fn urgent_broadcast(id: &str, from: &str, ts: i64) -> aida_core::mailbox::Message {
    aida_core::mailbox::Message {
        id: id.to_string(),
        thread_id: id.to_string(),
        from: from.to_string(),
        to: aida_core::mailbox::Recipient::Broadcast,
        timestamp: ts,
        in_reply_to: None,
        body: "URGENT: stop the drain".to_string(),
        urgent: true,
        intent: aida_core::mailbox::Intent::Request,
        retracted: false,
        deleted: false,
    }
}

/// An urgent broadcast that lives ONLY in the canonical orphan-store layer
/// (`.aida-store/mailbox/`) — not the local fast layer — is still counted by
/// the statusline. The pre-fix local-only read missed it, so the statusline
/// and `aida mailbox inbox` (which merges both layers) disagreed about
/// whether there was unread urgent mail. (Repro #1.)
// trace:BUG-625 | ai:claude
#[test]
fn read_urgent_unread_count_sees_canonical_only_message() {
    let proj = tempfile::tempdir().unwrap();
    let root = proj.path();
    let store_root = root.join(".aida-store");
    // Seed an urgent broadcast in the CANONICAL layer only.
    let cdir = mailbox_store::canonical_dir(&store_root);
    std::fs::create_dir_all(&cdir).unwrap();
    let msg = urgent_broadcast("c1", "codex", 1_000);
    std::fs::write(
        cdir.join("c1.json"),
        serde_json::to_string_pretty(&msg).unwrap(),
    )
    .unwrap();

    // Identity that did NOT send the message, no watermark yet → unread.
    let _g = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_USER", "operator"),
        ("AIDA_AGENT_TYPE", ""),
        ("AIDA_SESSION_ROLE", ""),
    ]);
    assert_eq!(
        read_urgent_unread_count(root),
        Some(1),
        "a canonical-only urgent broadcast must be counted (merge, not local-only)"
    );
}

/// The reproduction the operator hit. Seed an urgent broadcast, confirm the
/// statusline count is > 0, then advance the reader's watermark to the
/// merged-newest exactly as `aida mailbox inbox`'s mark-seen does, and assert
/// the count drops to 0. Before the fix the count was computed over a
/// different (local-only) set, so the inbox-read's watermark advance could
/// not clear it and the nag stuck forever. (Repro #2.)
// trace:BUG-625 | ai:claude
#[test]
fn read_urgent_unread_count_clears_after_inbox_read_advances_watermark() {
    let proj = tempfile::tempdir().unwrap();
    let root = proj.path();
    let store_root = root.join(".aida-store");

    // The urgent broadcast is in the canonical layer (digested); a second,
    // newer fyi broadcast is still local-only — together they exercise the
    // merge + per-identity max-watermark "newest" the inbox-read advances to.
    let cdir = mailbox_store::canonical_dir(&store_root);
    std::fs::create_dir_all(&cdir).unwrap();
    let urgent = urgent_broadcast("u1", "codex", 1_000);
    std::fs::write(
        cdir.join("u1.json"),
        serde_json::to_string_pretty(&urgent).unwrap(),
    )
    .unwrap();
    let mut later = urgent_broadcast("u2", "codex", 2_000);
    later.urgent = false;
    later.intent = aida_core::mailbox::Intent::Fyi;
    mailbox_store::write_message(root, &later).unwrap();

    let _g = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_USER", "operator"),
        ("AIDA_AGENT_TYPE", ""),
        ("AIDA_SESSION_ROLE", ""),
    ]);

    // Before reading: the urgent broadcast is unread.
    assert_eq!(
        read_urgent_unread_count(root),
        Some(1),
        "urgent broadcast is unread before the inbox is read"
    );

    // Simulate `aida mailbox inbox`'s mark-seen: advance the reader identity's
    // watermark to the MERGED inbox newest (the same computation the command
    // does at the mark-seen step). The reader is `operator`, which did not
    // send either message, so both are in its inbox.
    let local = mailbox_store::read_local_messages(root).unwrap();
    let canonical = mailbox_store::read_canonical_messages(&store_root).unwrap();
    let merged = aida_core::mailbox::merge_dedup(&local, &canonical);
    let newest = aida_core::mailbox::inbox_for("operator", &merged)
        .iter()
        .map(|m| m.timestamp)
        .max()
        .expect("operator has merged inbox mail");
    mailbox_store::set_watermark(root, "operator", newest).unwrap();

    // After the read advanced the watermark, the statusline count is 0 —
    // the two surfaces now agree. This is the regression the operator hit.
    assert_eq!(
        read_urgent_unread_count(root),
        Some(0),
        "reading the inbox (advancing the watermark) must clear the statusline count"
    );
}
