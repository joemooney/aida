// trace:STORY-B6B1278890774EF993D2EBC327862470-150 | ai:claude
//
// Command-level: `aida agent new codex --rotate`'s supervisor
// (`run_rotating_agent`) with a real `sh` child standing in for codex.
// Codex takes no seeded session id: its thread id is known only after
// startup, when its SessionStart hook records it on the session's registry
// row. A fake generation does that by writing `claude_session_id` onto its
// own row (`.aida/agents/codex-$$.toml`) once the launcher has stamped the
// row's mail wake, so nothing else writes the row afterwards.
use super::*;
use crate::agent_rotation::{RotationOutcome, RotationPolicy, Supervision};

/// Shell prelude every fake generation runs. Defines:
/// - `bind_thread`: wait for this generation's row and its mail-wake stamp,
///   log the stamped state, then record a thread id on the row (the hook);
/// - `handoff <body> [thread]`: write a seat handoff the way `aida session
///   handoff --write` does, with `native_session_id` only when given.
const PRELUDE: &str = r#"#!/bin/sh
case "$1" in --version) echo "codex-cli 0.50.0"; exit 0;; esac
gen="$AIDA_ROTATION_GENERATION"
row="$AIDA_PROJECT_ROOT/.aida/agents/codex-$$.toml"
thread="019a0000-0000-7000-8000-00000000000$gen"
wake=""
i=0
while [ $i -lt 100 ]; do
  wake=$(sed -n 's/^mail_wake = "\(.*\)"$/\1/p' "$row" 2>/dev/null)
  [ -n "$wake" ] && break
  i=$((i + 1))
  sleep 0.05
done
bind_thread() {
  sed -i.bak "1i claude_session_id = \"$thread\"" "$row" && rm -f "$row.bak"
}
echo "gen=$gen name=$AIDA_AGENT_NAME seat=${AIDA_ROTATION_SEAT:-} lease=${AIDA_SESSION_ID:-} cwd=$PWD row=codex-$$ wake=$wake thread=$thread handoff=${AIDA_ROTATION_HANDOFF:-} previous=${AIDA_ROTATION_PREVIOUS_SESSION:-}" >> "$AIDA_PROJECT_ROOT/generations.log"
handoff() {
  d="$AIDA_PROJECT_ROOT/.aida/handoff/$AIDA_ROTATION_SEAT"
  mkdir -p "$d"
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  f="gen$gen.md"
  printf '<!-- seat handoff: seat=%s written=%s handle=%s -->\n%s\n' "$AIDA_ROTATION_SEAT" "$ts" "$AIDA_AGENT_NAME" "$1" > "$d/$f"
  sid=""
  [ -n "$2" ] && sid=$(printf ',"native_session_id":"%s"' "$2")
  printf '{"ts":"%s","seat":"%s","event":"handoff","path":".aida/handoff/%s/%s","handle":"%s"%s}\n' \
    "$ts" "$AIDA_ROTATION_SEAT" "$AIDA_ROTATION_SEAT" "$f" "$AIDA_AGENT_NAME" "$sid" >> "$d/log.jsonl"
}
"#;

const HANDLE: &str = "codex-implementer-03";

struct Fixture {
    _tmp: tempfile::TempDir,
    project: std::path::PathBuf,
    binary: std::path::PathBuf,
}

fn fixture(body: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let project = base.join("project");
    std::fs::create_dir_all(project.join(".aida/agents")).unwrap();
    std::fs::write(project.join(".aida/config.toml"), "").unwrap();
    let binary = base.join("fake-codex");
    crate::test_exec::write_executable(&binary, &format!("{PRELUDE}{body}"));
    Fixture {
        _tmp: tmp,
        project,
        binary,
    }
}

fn codex_config() -> AgentLaunchConfig {
    AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: vec![CODEX_NO_DAEMON_FLAG.to_string()],
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    }
}

fn plan(project: &std::path::Path, lease_id: Option<&str>) -> AgentLaunchPlan {
    AgentLaunchPlan {
        project_root: project.to_path_buf(),
        launch_cwd: project.to_path_buf(),
        role: None,
        role_instance: RoleInstanceKind::Driver,
        current_spec: None,
        name: HANDLE.into(),
        lease_id: lease_id.map(str::to_string),
        // Codex: no seeded session id.
        native_session_id: agent_native_session_id_for_new("codex"),
        resumed_from: None,
    }
}

fn quick() -> RotationPolicy {
    RotationPolicy {
        countdown_secs: 0,
        ..RotationPolicy::default()
    }
}

/// Supervise a codex launch with its mail wake requested (`pending`).
fn supervise(fx: &Fixture, plan: AgentLaunchPlan, on_gap: &mut dyn FnMut(u64)) -> RotationOutcome {
    let _serial = crate::agent_rotation::supervisor_test_lock();
    run_rotating_agent(
        &fx.binary,
        &codex_config(),
        &plan,
        None,
        &["first prompt".to_string()],
        None,
        false,
        false,
        Some(agent_registry::MailWake::Pending),
        Supervision {
            policy: quick(),
            on_gap,
        },
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

fn field<'a>(line: &'a str, key: &str) -> &'a str {
    let tag = format!("{key}=");
    line.split(' ')
        .find_map(|kv| kv.strip_prefix(tag.as_str()))
        .unwrap_or_default()
}

fn write_lease(project: &std::path::Path, id: &str) {
    std::fs::create_dir_all(leases_dir(project)).unwrap();
    let lease: SessionLease = serde_json::from_value(serde_json::json!({
        "id": id, "scope": "STORY-ROT-CODEX", "slug": "rotation",
        "owner": "tester", "worktree_path": project, "branch": "story-rotation",
        "started_at": chrono::Utc::now().to_rfc3339(),
        "hostname": crate::coordination::hostname(), "role": "implementer"
    }))
    .unwrap();
    std::fs::write(lease_path(project, id), toml::to_string(&lease).unwrap()).unwrap();
}

/// Acceptance (1): `--rotate` relaunches codex under the same handle, lease
/// and worktree, with the lease Live in the gap; generation 2 is told the
/// thread it replaced and loads generation 1's handoff.
// trace:STORY-B6B1278890774EF993D2EBC327862470-150 | ai:claude
#[cfg(unix)]
#[test]
fn codex_relaunches_under_the_same_handle_lease_and_worktree() {
    let fx = fixture(
        r#"bind_thread
[ "$gen" = 1 ] && handoff "gen 1 handoff" "$thread"
exit 0
"#,
    );
    write_lease(&fx.project, "codex-lease-1");
    let project = fx.project.clone();
    let mut gap_states = Vec::new();
    let mut probe = |next: u64| {
        let lease = list_leases(&project)
            .into_iter()
            .find(|l| l.id == "codex-lease-1")
            .expect("lease present in the gap");
        gap_states.push((
            next,
            crate::leases::lease_state_for(&lease, &[], chrono::Utc::now()),
        ));
    };
    let outcome = supervise(&fx, plan(&fx.project, Some("codex-lease-1")), &mut probe);
    assert!(matches!(outcome, RotationOutcome::Exited(s) if s.success()));
    let lines = generations(&fx);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(gap_states.len(), 1);
    assert_eq!(gap_states[0].0, 2);
    assert!(
        matches!(gap_states[0].1, LeaseState::Live),
        "{:?}",
        gap_states[0].1
    );
    for line in &lines {
        assert_eq!(field(line, "name"), HANDLE, "{line}");
        assert_eq!(field(line, "lease"), "codex-lease-1", "{line}");
        assert_eq!(field(line, "cwd"), fx.project.display().to_string());
        // No --role: the codex launch rotates on the agent-type seat.
        assert_eq!(field(line, "seat"), "codex", "{line}");
    }
    assert_ne!(field(&lines[0], "row"), field(&lines[1], "row"));
    assert_eq!(
        field(&lines[1], "previous"),
        field(&lines[0], "thread"),
        "generation 2 names the thread it replaced"
    );
    assert_eq!(
        field(&lines[1], "handoff"),
        fx.project
            .join(".aida/handoff/codex/gen1.md")
            .display()
            .to_string()
    );
    assert!(list_leases(&fx.project)
        .iter()
        .any(|l| l.id == "codex-lease-1"));
}

/// Acceptance (2), thread known: once the hook recorded generation 1's
/// thread, a same-handle handoff written after the spawn by another thread
/// does not count (the `ts` fallback alone would have relaunched it).
// trace:STORY-B6B1278890774EF993D2EBC327862470-150 | ai:claude
#[cfg(unix)]
#[test]
fn a_known_thread_rejects_a_same_handle_handoff_from_another_thread() {
    let fx = fixture(
        r#"bind_thread
handoff "not mine" "019a0000-0000-7000-8000-0000000000ff"
exit 0
"#,
    );
    supervise(&fx, plan(&fx.project, None), &mut |_| {});
    assert_eq!(generations(&fx).len(), 1, "no relaunch");
}

/// Acceptance (2), thread unknown: a generation whose hook never recorded a
/// thread falls back to `ts >= spawn` plus the handle, so its own handoff
/// relaunches and a stale one from before the spawn does not.
// trace:STORY-B6B1278890774EF993D2EBC327862470-150 | ai:claude
#[cfg(unix)]
#[test]
fn before_the_thread_is_known_freshness_falls_back_to_the_spawn_time() {
    let fresh = fixture(
        // The fake stamps whole seconds; wait one so the stamp is past the spawn.
        r#"[ "$gen" = 1 ] && sleep 1 && handoff "gen 1, thread not yet known"
exit 0
"#,
    );
    supervise(&fresh, plan(&fresh.project, None), &mut |_| {});
    let lines = generations(&fresh);
    assert_eq!(
        lines.len(),
        2,
        "the post-spawn handoff relaunched: {lines:?}"
    );
    assert_eq!(field(&lines[1], "previous"), "", "no thread to name");

    let stale = fixture("exit 0\n");
    crate::seat_rotation::write_handoff_by(
        &stale.project,
        "codex",
        "an older session's handoff",
        chrono::Utc::now() - chrono::Duration::minutes(5),
        &crate::seat_rotation::HandoffWriter {
            handle: Some(HANDLE.into()),
            ..Default::default()
        },
    )
    .unwrap();
    supervise(&stale, plan(&stale.project, None), &mut |_| {});
    assert_eq!(generations(&stale).len(), 1, "a pre-spawn handoff is stale");
}

/// Acceptance (3): every generation's own registry row is stamped with the
/// launch's mail wake (`pending`, which the codex hook arms once it records
/// the thread), and each ended row is retired so the watcher stops waking
/// a generation that is gone.
// trace:STORY-B6B1278890774EF993D2EBC327862470-150 | ai:claude
#[cfg(unix)]
#[test]
fn mail_wake_is_rearmed_for_every_generation() {
    let fx = fixture(
        r#"bind_thread
[ "$gen" -lt 3 ] && handoff "gen $gen" "$thread"
exit 0
"#,
    );
    supervise(&fx, plan(&fx.project, None), &mut |_| {});
    let lines = generations(&fx);
    assert_eq!(lines.len(), 3, "{lines:?}");
    let mut rows = std::collections::BTreeSet::new();
    for line in &lines {
        assert_eq!(field(line, "wake"), "pending", "{line}");
        rows.insert(field(line, "row").to_string());
    }
    assert_eq!(rows.len(), 3, "one row per generation");
    for row in rows {
        let body =
            std::fs::read_to_string(fx.project.join(format!(".aida/agents/{row}.toml"))).unwrap();
        let entry: agent_registry::AgentRegistryEntry = toml::from_str(&body).unwrap();
        assert_eq!(entry.name.as_deref(), Some(HANDLE));
        assert!(entry.ended_at.is_some());
        assert_eq!(entry.mail_wake, Some(agent_registry::MailWake::Retired));
    }
}
