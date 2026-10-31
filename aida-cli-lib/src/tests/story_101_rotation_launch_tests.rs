// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
//
// Command-level: `aida agent new --rotate`'s supervisor (`run_rotating_agent`)
// with a real `sh` child standing in for claude. Each generation appends its
// env to `generations.log`; a generation that writes a handoff does it the
// way `aida session handoff --write` does (a stamped file plus a log record
// on the seat the supervisor named in AIDA_ROTATION_SEAT).
use super::*;
use crate::agent_rotation::{RotationOutcome, RotationPolicy, Supervision};

/// Shell prelude every fake generation runs: log its env, and define
/// `handoff <body>`.
const PRELUDE: &str = r#"#!/bin/sh
case "$1" in --version) echo "2.0.0 (Claude Code)"; exit 0;; esac
sid=""
prev=""
for a in "$@"; do
  [ "$prev" = "--session-id" ] && sid="$a"
  prev="$a"
done
echo "gen=$AIDA_ROTATION_GENERATION name=$AIDA_AGENT_NAME sid=$sid seat=${AIDA_ROTATION_SEAT:-} lease=${AIDA_SESSION_ID:-} cwd=$PWD handoff=${AIDA_ROTATION_HANDOFF:-} stamp=${AIDA_ROTATION_HANDOFF_STAMP:-} previous=${AIDA_ROTATION_PREVIOUS_SESSION:-}" >> "$AIDA_PROJECT_ROOT/generations.log"
handoff() {
  d="$AIDA_PROJECT_ROOT/.aida/handoff/$AIDA_ROTATION_SEAT"
  mkdir -p "$d"
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  f="gen$AIDA_ROTATION_GENERATION.md"
  printf '<!-- seat handoff: seat=%s written=%s handle=%s -->\n%s\n' "$AIDA_ROTATION_SEAT" "$ts" "$AIDA_AGENT_NAME" "$1" > "$d/$f"
  printf '{"ts":"%s","seat":"%s","event":"handoff","path":".aida/handoff/%s/%s","handle":"%s","native_session_id":"%s"}\n' \
    "$ts" "$AIDA_ROTATION_SEAT" "$AIDA_ROTATION_SEAT" "$f" "$AIDA_AGENT_NAME" "$sid" >> "$d/log.jsonl"
}
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    project: std::path::PathBuf,
    binary: std::path::PathBuf,
}

/// A project and a fake claude whose body (after [`PRELUDE`]) is `body`.
fn fixture(body: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let project = base.join("project");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(project.join(".aida/config.toml"), "").unwrap();
    let binary = base.join("fake-claude");
    crate::test_exec::write_executable(&binary, &format!("{PRELUDE}{body}"));
    Fixture {
        _tmp: tmp,
        project,
        binary,
    }
}

fn claude_config(sid: Option<&str>) -> AgentLaunchConfig {
    let mut config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: vec![],
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    config
        .default_args
        .extend(agent_seed_session_args("claude", sid));
    config
}

fn plan(project: &std::path::Path, role: Option<&str>, lease_id: Option<&str>) -> AgentLaunchPlan {
    AgentLaunchPlan {
        project_root: project.to_path_buf(),
        launch_cwd: project.to_path_buf(),
        role: role.map(str::to_string),
        role_instance: RoleInstanceKind::Driver,
        current_spec: None,
        name: "claude-advisor-08".into(),
        lease_id: lease_id.map(str::to_string),
        native_session_id: None,
        resumed_from: None,
    }
}

/// No countdown, so a test spends no wall time between generations.
fn quick() -> RotationPolicy {
    RotationPolicy {
        countdown_secs: 0,
        ..RotationPolicy::default()
    }
}

/// Supervise `plan` under `policy`, calling `on_gap` between generations.
fn supervise(
    fx: &Fixture,
    mut plan: AgentLaunchPlan,
    policy: RotationPolicy,
    on_gap: &mut dyn FnMut(u64),
) -> RotationOutcome {
    // trace:STORY-B6B1278890774EF993D2EBC327862470-150 | ai:claude
    let _serial = crate::agent_rotation::supervisor_test_lock();
    let sid = agent_native_session_id_for_new("claude");
    plan.native_session_id = sid.clone();
    run_rotating_agent(
        &fx.binary,
        &claude_config(sid.as_deref()),
        &plan,
        None,
        &["first prompt".to_string()],
        None,
        false,
        false,
        None,
        Supervision { policy, on_gap },
        || {},
    )
    .unwrap()
}

fn generations(fx: &Fixture) -> Vec<String> {
    std::fs::read_to_string(fx.project.join("generations.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// The `key=value` field of a generations.log line.
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    let tag = format!("{key}=");
    line.split(' ')
        .find_map(|kv| kv.strip_prefix(tag.as_str()))
        .unwrap_or_default()
}

fn handle_released(project: &std::path::Path) -> bool {
    let reservation = crate::agent_handle::load_reservation(project, "claude-advisor-08").unwrap();
    crate::relay_mail::run_holding(project, &reservation).is_none()
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-101.ac433c65 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-101.ac5c23ac | ai:claude
#[cfg(unix)]
#[test]
fn a_fresh_handoff_relaunches_the_same_handle_and_a_bare_fast_exit_stops() {
    let fx = fixture(
        r#"[ "$AIDA_ROTATION_GENERATION" = 1 ] && handoff "gen 1 handoff"
exit 0
"#,
    );
    let outcome = supervise(&fx, plan(&fx.project, None, None), quick(), &mut |_| {});
    assert!(matches!(outcome, RotationOutcome::Exited(s) if s.success()));
    let lines = generations(&fx);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(field(&lines[0], "gen"), "1");
    assert_eq!(field(&lines[1], "gen"), "2");
    for line in &lines {
        assert_eq!(field(line, "name"), "claude-advisor-08", "{line}");
        // No --role: the supervisor's seat is the agent type, and it names it.
        assert_eq!(field(line, "seat"), "claude", "{line}");
    }
    let sid1 = field(&lines[0], "sid");
    assert!(!sid1.is_empty());
    assert_ne!(
        field(&lines[1], "sid"),
        sid1,
        "generation 2 is a new session"
    );
    assert_eq!(field(&lines[1], "previous"), sid1);
    assert_eq!(
        field(&lines[1], "handoff"),
        fx.project
            .join(".aida/handoff/claude/gen1.md")
            .display()
            .to_string()
    );
    assert!(!field(&lines[1], "stamp").is_empty());
    assert!(
        handle_released(&fx.project),
        "the run no longer holds the handle"
    );
}

/// Finding 1: a sibling session on the same seat writes a newer handoff
/// (and so the seat's latest.md) between generations. Generation 2 still
/// gets the file its supervisor matched, by path and stamp.
// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
#[cfg(unix)]
#[test]
fn generation_two_gets_the_matched_handoff_not_a_siblings_newer_one() {
    let fx = fixture(
        r#"[ "$AIDA_ROTATION_GENERATION" = 1 ] && handoff "MINE: continue the rotation work"
exit 0
"#,
    );
    let project = fx.project.clone();
    let mut sibling_writes = |_next: u64| {
        crate::seat_rotation::write_handoff_by(
            &project,
            "claude",
            "SIBLING: another advisor's notes",
            chrono::Utc::now() + chrono::Duration::seconds(2),
            &crate::seat_rotation::HandoffWriter {
                handle: Some("claude-advisor-09".into()),
                ..Default::default()
            },
        )
        .unwrap();
    };
    supervise(
        &fx,
        plan(&fx.project, None, None),
        quick(),
        &mut sibling_writes,
    );
    let lines = generations(&fx);
    assert_eq!(lines.len(), 2, "{lines:?}");
    let latest =
        std::fs::read_to_string(fx.project.join(".aida/handoff/claude/latest.md")).unwrap();
    assert!(
        latest.contains("SIBLING"),
        "the seat's latest is the sibling's"
    );
    let loaded = crate::agent_rotation::matched_handoff(
        field(&lines[1], "handoff"),
        field(&lines[1], "stamp"),
    )
    .unwrap();
    assert!(loaded.contains("MINE"), "{loaded}");
    assert!(!loaded.contains("SIBLING"), "{loaded}");
    // A re-stamped (rewritten) file is not loaded in its place.
    std::fs::write(
        field(&lines[1], "handoff"),
        "<!-- seat handoff: seat=claude written=2030-01-01T00:00:00Z -->\nREWRITTEN\n",
    )
    .unwrap();
    let err = crate::agent_rotation::matched_handoff(
        field(&lines[1], "handoff"),
        field(&lines[1], "stamp"),
    )
    .unwrap_err();
    assert!(err.contains("instead of"), "{err}");
}

fn write_lease(project: &std::path::Path, id: &str, spec: &str) -> SessionLease {
    std::fs::create_dir_all(leases_dir(project)).unwrap();
    let lease: SessionLease = serde_json::from_value(serde_json::json!({
        "id": id, "scope": spec, "slug": "rotation",
        "owner": "tester", "worktree_path": project, "branch": "story-rotation",
        "started_at": chrono::Utc::now().to_rfc3339(),
        "hostname": crate::coordination::hostname(), "role": "implementer"
    }))
    .unwrap();
    std::fs::write(lease_path(project, id), toml::to_string(&lease).unwrap()).unwrap();
    lease
}

fn load_lease(project: &std::path::Path, id: &str) -> Option<SessionLease> {
    list_leases(project).into_iter().find(|l| l.id == id)
}

/// Gate (a): the lease, its claim and the worktree are the same across a
/// relaunch, and the lease reads Live in the gap because the supervisor
/// stamped itself as its active process.
// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
#[cfg(unix)]
#[test]
fn lease_claim_and_worktree_survive_a_relaunch_and_the_lease_is_live_in_the_gap() {
    let fx = fixture(
        r#"[ "$AIDA_ROTATION_GENERATION" = 1 ] && handoff "gen 1"
exit 0
"#,
    );
    let before = write_lease(&fx.project, "rot-lease-1", "STORY-ROT-1");
    let project = fx.project.clone();
    let mut gap_states = Vec::new();
    let mut probe = |next: u64| {
        let lease = load_lease(&project, "rot-lease-1").expect("lease present in the gap");
        gap_states.push((
            next,
            lease.active_pid,
            crate::leases::lease_state_for(&lease, &[], chrono::Utc::now()),
        ));
    };
    let outcome = supervise(
        &fx,
        plan(&fx.project, None, Some("rot-lease-1")),
        quick(),
        &mut probe,
    );
    assert!(matches!(outcome, RotationOutcome::Exited(_)));
    assert_eq!(gap_states.len(), 1, "one gap");
    let (next, pid, state) = &gap_states[0];
    assert_eq!(*next, 2);
    assert_eq!(
        *pid,
        Some(std::process::id()),
        "the supervisor holds active_pid"
    );
    assert!(matches!(state, LeaseState::Live), "{state:?}");
    let lines = generations(&fx);
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        assert_eq!(field(line, "lease"), "rot-lease-1", "{line}");
        assert_eq!(
            field(line, "cwd"),
            fx.project.display().to_string(),
            "{line}"
        );
        assert_eq!(field(line, "seat"), "claude", "{line}");
    }
    let after = load_lease(&fx.project, "rot-lease-1").expect("the lease outlives the run");
    assert_eq!(after.id, before.id);
    assert_eq!(after.scope, before.scope, "the spec claim is unchanged");
    assert_eq!(after.worktree_path, before.worktree_path);
    assert_eq!(after.branch, before.branch);
}

/// Gate (b): three relaunches in the window is the limit; a real run stops
/// at the fourth generation's exit even though it wrote a fresh handoff.
// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
#[cfg(unix)]
#[test]
fn the_three_in_ten_minutes_limit_stops_a_real_run() {
    let fx = fixture(
        r#"handoff "gen $AIDA_ROTATION_GENERATION"
exit 0
"#,
    );
    let mut gaps = Vec::new();
    let outcome = supervise(&fx, plan(&fx.project, None, None), quick(), &mut |next| {
        gaps.push(next)
    });
    assert!(matches!(outcome, RotationOutcome::Exited(_)));
    assert_eq!(gaps, vec![2, 3, 4]);
    let lines = generations(&fx);
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert_eq!(field(&lines[3], "gen"), "4");
    assert!(handle_released(&fx.project));
}

/// Gate (c): SIGINT during the countdown stops the run (the launcher then
/// exits 130) and releases the lease instead of leaving it on a dead pid.
// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
#[cfg(unix)]
#[test]
fn sigint_during_the_countdown_stops_the_run_and_releases_the_lease() {
    // Generation 1 hands off, then a detached helper sends SIGINT to the
    // launcher (this test process) once the 10s countdown is running.
    let fx = fixture(
        r#"handoff "gen 1"
( sleep 1; kill -INT "$PPID" ) >/dev/null 2>&1 &
exit 0
"#,
    );
    write_lease(&fx.project, "rot-lease-c", "STORY-ROT-C");
    let mut gaps = 0;
    let started = std::time::Instant::now();
    let outcome = supervise(
        &fx,
        plan(&fx.project, None, Some("rot-lease-c")),
        RotationPolicy {
            countdown_secs: 10,
            ..RotationPolicy::default()
        },
        &mut |_| gaps += 1,
    );
    assert!(matches!(outcome, RotationOutcome::Cancelled), "{outcome:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(9),
        "the countdown stopped early"
    );
    assert_eq!(gaps, 0, "no relaunch began");
    assert_eq!(generations(&fx).len(), 1);
    assert!(
        load_lease(&fx.project, "rot-lease-c").is_none(),
        "the lease is released"
    );
    assert!(handle_released(&fx.project), "the relay run ended");
}

/// Gate (d): mail sent to the handle in the gap is accepted for the run and
/// reaches generation 2 exactly once; reading it twice writes one receipt.
// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
#[cfg(unix)]
#[test]
fn mail_sent_in_the_gap_reaches_generation_two_exactly_once() {
    let fx = fixture(
        r#"[ "$AIDA_ROTATION_GENERATION" = 1 ] && handoff "gen 1"
if [ "$AIDA_ROTATION_GENERATION" = 2 ]; then
  n=$(grep -l GAP-MAIL-MARKER "$AIDA_PROJECT_ROOT"/.aida/mailbox/*.json 2>/dev/null | wc -l)
  echo "gap_mail=$n" >> "$AIDA_PROJECT_ROOT/generations.log"
fi
exit 0
"#,
    );
    let project = fx.project.clone();
    let store = project.join(".aida-store");
    let mut sent: Option<aida_core::mailbox::Message> = None;
    let mut send_in_gap = |_next: u64| {
        let directory =
            crate::mailbox_resolver::RecipientDirectory::load_for_send(&project, &store).unwrap();
        let resolved = directory.resolve_send("claude-advisor-08", false).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis();
        let (deliveries, _) =
            crate::mailbox_resolver::delivery_sidecar(&resolved, &id, now, false).unwrap();
        let msg = aida_core::mailbox::Message {
            id: id.clone(),
            thread_id: id,
            from: "operator".into(),
            to: aida_core::mailbox::Recipient::Agent(resolved.address.clone()),
            timestamp: now,
            in_reply_to: None,
            body: "GAP-MAIL-MARKER".into(),
            subject: None,
            urgent: false,
            intent: aida_core::mailbox::Intent::Fyi,
            retracted: false,
            deleted: false,
            archived: false,
            from_source: aida_core::mailbox::SenderSource::Explicit,
            from_role: None,
            relayed_from: None,
        };
        crate::mailbox_store::write_message_with_deliveries(&project, &msg, deliveries.as_ref())
            .unwrap();
        sent = Some(msg);
    };
    supervise(
        &fx,
        plan(&fx.project, None, None),
        quick(),
        &mut send_in_gap,
    );
    let sent = sent.expect("the gap ran");
    assert_eq!(
        sent.to,
        aida_core::mailbox::Recipient::Agent("claude-advisor-08".into()),
        "the run held the handle in the gap, so the mail is addressed to it"
    );
    let lines = generations(&fx);
    assert!(
        lines.iter().any(|l| l == "gap_mail=1"),
        "generation 2 saw the gap mail exactly once: {lines:?}"
    );
    let unread = |project: &std::path::Path| {
        let merged = crate::mailbox_store::read_local_messages(project).unwrap();
        let evidence = crate::mailbox_store::ReadEvidence::load(project).unwrap();
        crate::mailbox_store::unread_by_recipient(&merged, &evidence)
            .get("claude-advisor-08")
            .map(|u| u.count)
            .unwrap_or(0)
    };
    assert_eq!(
        unread(&fx.project),
        1,
        "one copy, unread until generation 2 reads it"
    );
    // Generation 2 reads it (and a second read changes nothing).
    for _ in 0..2 {
        crate::mailbox_store::record_seen(&fx.project, "claude-advisor-08", &[sent.id.as_str()])
            .unwrap();
    }
    assert_eq!(unread(&fx.project), 0);
    let receipts = crate::mailbox_store::read_receipts(&fx.project, "claude-advisor-08");
    assert_eq!(receipts.len(), 1, "one receipt: {receipts:?}");
}

/// Gate (e): the relaunched session sends its own hello, as generation 2,
/// on the handle's one advisor thread. The child's SessionStart hook runs
/// `aida mailbox hello --hook`; here the hello is sent from exactly the
/// identity generation 2's env carried.
// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
#[cfg(unix)]
#[test]
fn the_relaunched_session_sends_its_hello() {
    use crate::advisor_handshake::{send_hello, HelloContext, Rotation, SendOutcome};
    let fx = fixture(
        r#"echo "started gen=$AIDA_ROTATION_GENERATION at=$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$AIDA_PROJECT_ROOT/starts.log"
[ "$AIDA_ROTATION_GENERATION" = 1 ] && handoff "gen 1"
[ "$AIDA_ROTATION_GENERATION" = 1 ] && sleep 1
exit 0
"#,
    );
    supervise(
        &fx,
        // No --role: a role launch mints a child seat grant from a parent
        // grant this test process does not hold; the hello's role is set below.
        plan(&fx.project, None, None),
        quick(),
        &mut |_| {},
    );
    let lines = generations(&fx);
    assert_eq!(lines.len(), 2, "{lines:?}");
    let starts: Vec<chrono::DateTime<chrono::Utc>> =
        std::fs::read_to_string(fx.project.join("starts.log"))
            .unwrap()
            .lines()
            .map(|l| {
                chrono::DateTime::parse_from_rfc3339(l.rsplit("at=").next().unwrap())
                    .unwrap()
                    .with_timezone(&chrono::Utc)
            })
            .collect();
    assert_eq!(starts.len(), 2);
    let home = crate::mailbox_store::mailbox_home(&fx.project);
    let store = fx.project.join(".aida-store");
    let ctx_for = |line: &str, started| HelloContext {
        handle: field(line, "name").to_string(),
        from_source: aida_core::mailbox::SenderSource::Explicit,
        role: "implementer".into(),
        spec: None,
        path: fx.project.clone(),
        rotation: field(line, "gen")
            .parse()
            .ok()
            .filter(|g| *g >= 2)
            .map(|generation| Rotation {
                generation,
                previous_native_session_id: Some(field(line, "previous").to_string())
                    .filter(|p| !p.is_empty()),
            }),
        session_started: Some(started),
    };
    let gen1 = send_hello(&home, &store, &ctx_for(&lines[0], starts[0]), starts[0]).unwrap();
    assert!(!matches!(gen1, SendOutcome::AlreadySent(_)));
    let gen2 = send_hello(&home, &store, &ctx_for(&lines[1], starts[1]), starts[1]).unwrap();
    let (SendOutcome::Sent(rec) | SendOutcome::Queued(rec)) = &gen2 else {
        panic!("generation 2 must say hello again: {gen2:?}");
    };
    let rotation = rec.rotation.as_ref().expect("the hello names the rotation");
    assert_eq!(rotation.generation, 2);
    assert_eq!(
        rotation.previous_native_session_id.as_deref(),
        Some(field(&lines[0], "sid"))
    );
    let hellos: Vec<_> = crate::mailbox_store::read_local_messages(&fx.project)
        .unwrap()
        .into_iter()
        .filter(crate::advisor_handshake::is_hello)
        .collect();
    assert_eq!(hellos.len(), 2);
    assert!(hellos
        .iter()
        .all(|m| m.thread_id == "aida-hello-claude-advisor-08"));
}
