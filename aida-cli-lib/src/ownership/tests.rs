//! STORY-2325 PR-A: matrix, combination, heartbeat, host-view and property
//! tests for the pure ownership judge.
// trace:STORY-2325 | ai:claude

use super::*;
use crate::gate::canon::SpecKey;
use aida_core::process_probe::{Probe, ProbeSource, ProcessIdentity, INITIAL_PID_NAMESPACE_INODE};
use chrono::{DateTime, Duration, TimeZone, Utc};
use proptest::prelude::*;

const HOST: &str = "box";
const BOOT: &str = "boot-a";
const HOST_NS: u64 = INITIAL_PID_NAMESPACE_INODE;
const SANDBOX_NS: u64 = 4_026_535_667;

/// pid 1 runs, pid 2 has exited, anything else cannot be probed.
fn probe(id: &ProcessIdentity) -> Probe {
    match id.pid {
        1 => Probe::Alive,
        2 => Probe::Dead(format!("process {} no longer exists", id.pid)),
        _ => Probe::Unknown("cannot read /proc".to_string()),
    }
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap()
}

fn host() -> HostFacts<'static> {
    HostFacts {
        host: HOST.to_string(),
        boot: Some(BOOT.to_string()),
        pid_ns: Some(HOST_NS),
        probe: &probe,
    }
}

fn ident(pid: u32, boot: &str) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        start: format!("linux:{boot}:12345"),
    }
}

fn proc_w(host: &str, id: ProcessIdentity, pid_ns: Option<u64>) -> Witness {
    Witness {
        kind: WitnessKind::Process {
            host: host.to_string(),
            id,
            pid_ns,
        },
        recorded_at: now(),
    }
}

/// A process witness recorded on this host, this boot, the host namespace.
fn local(pid: u32) -> Witness {
    proc_w(HOST, ident(pid, BOOT), Some(HOST_NS))
}

fn beat(age_secs: i64, every_secs: u64) -> Witness {
    Witness {
        kind: WitnessKind::Heartbeat {
            at: now() - Duration::seconds(age_secs),
            every_secs,
        },
        recorded_at: now(),
    }
}

fn stale_beat() -> Witness {
    beat(LONGEST_TOOL_TIMEOUT_SECS as i64 + 60, 30)
}

fn holder(witnesses: Vec<Witness>) -> Holder {
    Holder {
        seat: "aida-story9".to_string(),
        role: Role::Implementer,
        session: "lease-1".to_string(),
        tree: None,
        witnesses,
    }
}

fn record(phase: Phase, witnesses: Vec<Witness>) -> Ownership {
    Ownership {
        key: WorkKey::Spec {
            spec: SpecKey(uuid::Uuid::nil()),
        },
        generation: 1,
        holder: Some(holder(witnesses)),
        phase,
    }
}

fn change(n: u64) -> ChangeId {
    ChangeId {
        forge: "github".to_string(),
        n,
    }
}

fn label(r: &WitnessReading) -> &'static str {
    match r {
        WitnessReading::Alive => "alive",
        WitnessReading::Dead(_) => "dead",
        WitnessReading::Unknown(_) => "unknown",
    }
}

fn owner_label(v: &OwnerVerdict) -> &'static str {
    match v {
        OwnerVerdict::Alive { .. } => "alive",
        OwnerVerdict::Dead { .. } => "dead",
        OwnerVerdict::Unknown { .. } => "unknown",
    }
}

fn never_ancestor(_: &str, _: &str) -> bool {
    false
}

fn clean(head: Option<&str>) -> TreeReading {
    TreeReading::Read {
        dirty: vec![],
        unpushed: vec![],
        head: head.map(str::to_string),
    }
}

/// Every process-witness axis: host, boot, recorded and reader pid
/// namespace, start identity, and the probe's own answer.
#[test]
fn process_witness_matrix() {
    let no_start = ProcessIdentity {
        pid: 2,
        start: String::new(),
    };
    let cases: Vec<(&str, Witness, HostFacts<'static>, &str)> = vec![
        ("same host, live pid", local(1), host(), "alive"),
        ("same host, exited pid", local(2), host(), "dead"),
        ("same host, unprobeable pid", local(3), host(), "unknown"),
        (
            "another host, exited pid",
            proc_w("other", ident(2, BOOT), Some(HOST_NS)),
            host(),
            "unknown",
        ),
        (
            "another boot: the host rebooted",
            proc_w(HOST, ident(1, "boot-old"), Some(HOST_NS)),
            host(),
            "dead",
        ),
        (
            "another boot, recorded in a foreign namespace: still rebooted",
            proc_w(HOST, ident(1, "boot-old"), Some(SANDBOX_NS)),
            host(),
            "dead",
        ),
        (
            "this host's boot id unreadable",
            local(2),
            HostFacts {
                boot: None,
                ..host()
            },
            "unknown",
        ),
        (
            "foreign pid namespace, pid reads dead from here",
            proc_w(HOST, ident(2, BOOT), Some(SANDBOX_NS)),
            host(),
            "unknown",
        ),
        (
            "unrecorded pid namespace, pid reads dead from here",
            proc_w(HOST, ident(2, BOOT), None),
            host(),
            "unknown",
        ),
        (
            "reader's own namespace unreadable",
            local(2),
            HostFacts {
                pid_ns: None,
                ..host()
            },
            "unknown",
        ),
        (
            "reader sandboxed, owner on the host",
            local(2),
            HostFacts {
                pid_ns: Some(SANDBOX_NS),
                ..host()
            },
            "unknown",
        ),
        (
            "missing start identity is never dead",
            proc_w(HOST, no_start.clone(), Some(HOST_NS)),
            host(),
            "unknown",
        ),
        (
            "missing start identity is never alive",
            proc_w(HOST, ProcessIdentity { pid: 1, ..no_start }, Some(HOST_NS)),
            host(),
            "unknown",
        ),
    ];
    for (name, w, h, want) in cases {
        assert_eq!(label(&judge_witness(&w, &h, now())), want, "{name}");
    }
}

/// A3: the heartbeat window is at least three intervals and never shorter
/// than the longest tool timeout.
#[test]
fn heartbeat_window_covers_three_intervals_and_the_longest_tool_call() {
    assert_eq!(heartbeat_window(30), LONGEST_TOOL_TIMEOUT_SECS);
    assert_eq!(heartbeat_window(3 * 60 * 60), 9 * 60 * 60);
    assert!(heartbeat_window(u64::MAX) >= LONGEST_TOOL_TIMEOUT_SECS);

    let h = host();
    let window = heartbeat_window(30) as i64;
    let cases = [
        ("fresh beat", beat(10, 30), "alive"),
        ("beat at the window edge", beat(window, 30), "alive"),
        // Idle far past 3 x 30s but inside one long tool call.
        ("idle inside one tool call", beat(90 * 60, 30), "alive"),
        ("beat past the window", beat(window + 1, 30), "dead"),
        ("beat from the future (skew)", beat(-600, 30), "alive"),
        ("beat with no interval", beat(10, 0), "unknown"),
    ];
    for (name, w, want) in cases {
        assert_eq!(label(&judge_witness(&w, &h, now())), want, "{name}");
    }
}

/// A2: Unknown process witnesses abstain; Dead needs one Dead and no Alive;
/// Alive always wins.
#[test]
fn combination_rule_matrix() {
    let foreign = proc_w(HOST, ident(2, BOOT), Some(SANDBOX_NS));
    let cases: Vec<(&str, Vec<Witness>, &str)> = vec![
        (
            "foreign pidns + stale heartbeat",
            vec![foreign.clone(), stale_beat()],
            "dead",
        ),
        (
            "live process + stale heartbeat (idle interactive seat)",
            vec![local(1), stale_beat()],
            "alive",
        ),
        (
            "stale heartbeat + live process (order does not matter)",
            vec![stale_beat(), local(1)],
            "alive",
        ),
        (
            "exited process + fresh heartbeat",
            vec![local(2), beat(5, 30)],
            "alive",
        ),
        ("foreign pidns alone", vec![foreign.clone()], "unknown"),
        ("unprobeable + exited", vec![local(3), local(2)], "dead"),
        ("exited process alone", vec![local(2)], "dead"),
        ("no witness", vec![], "unknown"),
    ];
    let h = host();
    for (name, ws, want) in cases {
        let got = judge_owner(&holder(ws), &h, now());
        assert_eq!(owner_label(&got), want, "{name}: {got:?}");
    }
}

/// A1: only a writer in the host's initial pid namespace records a process
/// witness, and only with a start identity.
#[test]
fn process_witness_is_recorded_from_the_host_view_only() {
    let id = ident(42, BOOT);
    let w = process_witness(Some(HOST_NS), HOST, id.clone(), now()).expect("host writer");
    assert_eq!(
        w.kind,
        WitnessKind::Process {
            host: HOST.to_string(),
            id: id.clone(),
            pid_ns: Some(HOST_NS),
        }
    );
    assert_eq!(
        process_witness(Some(SANDBOX_NS), HOST, id.clone(), now()),
        None,
        "a sandboxed writer records no process witness"
    );
    assert_eq!(
        process_witness(None, HOST, id, now()),
        None,
        "an unreadable writer namespace records none"
    );
    let no_start = ProcessIdentity {
        pid: 42,
        start: String::new(),
    };
    assert_eq!(
        process_witness(Some(HOST_NS), HOST, no_start, now()),
        None,
        "a pid without a start identity is not recorded"
    );
}

/// The capture half of A1: read a pid's namespace from a fixture procfs.
#[cfg(unix)]
#[test]
fn pid_namespace_capture_reads_the_ns_link() {
    let dir = tempfile::tempdir().unwrap();
    let ns = dir.path().join("7").join("ns");
    std::fs::create_dir_all(&ns).unwrap();
    std::os::unix::fs::symlink("pid:[4026531836]", ns.join("pid")).unwrap();
    let src = ProbeSource::at(dir.path());
    assert_eq!(src.pid_namespace(7), Ok(HOST_NS));
    assert!(src.pid_namespace(8).is_err(), "missing pid");
    let bad = dir.path().join("9").join("ns");
    std::fs::create_dir_all(&bad).unwrap();
    std::os::unix::fs::symlink("net:[1]", bad.join("pid")).unwrap();
    assert!(src.pid_namespace(9).is_err(), "malformed link");
}

/// Every phase x owner state x tree reading x PR head.
#[test]
fn reclaim_matrix() {
    let ancestor = |a: &str, b: &str| a == "old" && b == "pr";
    let phases = [
        ("leased", Phase::Leased),
        ("reviewing", Phase::Reviewing { head: "pr".into() }),
        ("training", Phase::Training { train: change(9) }),
        (
            "in-pr",
            Phase::InPr {
                change: change(5),
                handed_off_at: now(),
                head: "pr".into(),
            },
        ),
        (
            "closed",
            Phase::Closed {
                why: "merged".into(),
            },
        ),
    ];
    let trees: Vec<(&str, TreeReading, Option<&str>, &str)> = vec![
        ("missing", TreeReading::Missing, None, "end"),
        (
            "clean, HEAD == PR head",
            clean(Some("pr")),
            Some("pr"),
            "end",
        ),
        ("clean, HEAD in PR", clean(Some("old")), Some("pr"), "end"),
        ("clean, unborn HEAD", clean(None), None, "end"),
        ("clean, no PR head", clean(Some("x")), None, "keep"),
        (
            "clean, HEAD not in PR",
            clean(Some("x")),
            Some("pr"),
            "refuse",
        ),
        (
            "dirty",
            TreeReading::Read {
                dirty: vec!["M a.rs".into()],
                unpushed: vec![],
                head: Some("pr".into()),
            },
            Some("pr"),
            "refuse",
        ),
        (
            "unpushed",
            TreeReading::Read {
                dirty: vec![],
                unpushed: vec!["abc".into()],
                head: Some("pr".into()),
            },
            Some("pr"),
            "refuse",
        ),
        (
            "not a git tree",
            TreeReading::NotAGitTree,
            Some("pr"),
            "refuse",
        ),
        (
            "unreadable",
            TreeReading::Unreadable("EACCES".into()),
            Some("pr"),
            "keep",
        ),
    ];
    let owners: [(&str, Vec<Witness>); 3] = [
        ("alive", vec![local(1)]),
        ("dead", vec![local(2)]),
        ("unknown", vec![local(3)]),
    ];
    let h = host();
    for (pname, phase) in &phases {
        for (oname, ws) in &owners {
            for (tname, tree, pr_head, dead_want) in &trees {
                let rec = record(phase.clone(), ws.clone());
                let v = judge(&rec, &h, tree, *pr_head, &ancestor, now());
                let want = match (phase, *oname) {
                    (Phase::InPr { .. } | Phase::Closed { .. }, _) => "free",
                    (_, "dead") => dead_want,
                    _ => "keep",
                };
                let got = match v.reclaim {
                    Reclaim::Free => "free",
                    Reclaim::EndBySafeRule => "end",
                    Reclaim::Refuse(_) => "refuse",
                    Reclaim::Keep(_) => "keep",
                };
                assert_eq!(got, want, "{pname} / {oname} / {tname}: {v:?}");
                assert_eq!(owner_label(&v.owner), *oname, "{pname} / {oname} / {tname}");
            }
        }
    }
}

#[test]
fn no_holder_is_free() {
    let rec = Ownership {
        holder: None,
        ..record(Phase::Leased, vec![])
    };
    let v = judge(
        &rec,
        &host(),
        &TreeReading::Missing,
        None,
        &never_ancestor,
        now(),
    );
    assert_eq!(v.reclaim, Reclaim::Free);
}

/// Incident fixture, STORY-2315: a finished lane's dead, clean lease whose
/// HEAD is the PR head ends by rule — no `--force` involved.
#[test]
fn story_2315_dead_clean_in_pr_lease_ends_by_rule() {
    let rec = record(Phase::Leased, vec![local(2), stale_beat()]);
    let v = judge(
        &rec,
        &host(),
        &clean(Some("abc123")),
        Some("abc123"),
        &never_ancestor,
        now(),
    );
    assert_eq!(v.reclaim, Reclaim::EndBySafeRule, "{v:?}");
}

/// Incident fixture, BUG-2412 r1 / F2: a train owner recorded from a
/// sandboxed (foreign) pid namespace whose pid reads dead from here is never
/// judged gone while nothing else proves it, so its train is kept.
#[test]
fn bug_2412_foreign_pidns_train_owner_is_kept() {
    let rec = record(
        Phase::Training {
            train: change(3301),
        },
        vec![proc_w(HOST, ident(2, BOOT), Some(SANDBOX_NS))],
    );
    let v = judge(
        &rec,
        &host(),
        &TreeReading::Missing,
        None,
        &never_ancestor,
        now(),
    );
    assert!(matches!(v.owner, OwnerVerdict::Unknown { .. }), "{v:?}");
    assert!(matches!(v.reclaim, Reclaim::Keep(_)), "{v:?}");
}

/// Records persist as JSON in PR-B; the shape must round-trip now.
#[test]
fn ownership_record_round_trips_as_json() {
    let rec = Ownership {
        key: WorkKey::Review {
            change: change(12),
            head: "deadbeef".into(),
        },
        generation: 7,
        holder: Some(Holder {
            tree: Some(SeatTree {
                path: "/w/aida-pr-12".into(),
                kind: TreeKind::Worktree,
                branch: Some("story-9".into()),
            }),
            ..holder(vec![local(1), beat(3, 30)])
        }),
        phase: Phase::InPr {
            change: change(12),
            handed_off_at: now(),
            head: "deadbeef".into(),
        },
    };
    let json = serde_json::to_string(&rec).unwrap();
    assert_eq!(serde_json::from_str::<Ownership>(&json).unwrap(), rec);
    let spec = serde_json::to_value(WorkKey::Spec {
        spec: SpecKey(uuid::Uuid::nil()),
    })
    .unwrap();
    assert_eq!(
        spec,
        serde_json::json!({"kind": "spec", "spec": "00000000-0000-0000-0000-000000000000"})
    );
    let _ = MailAnchorKey("anchor".into());
}

fn witness_strategy() -> impl Strategy<Value = Witness> {
    prop_oneof![
        Just(local(1)),
        Just(local(2)),
        Just(local(3)),
        Just(proc_w(HOST, ident(2, BOOT), Some(SANDBOX_NS))),
        Just(proc_w("other", ident(2, BOOT), Some(HOST_NS))),
        Just(proc_w(HOST, ident(1, "boot-old"), Some(HOST_NS))),
        Just(beat(5, 30)),
        Just(stale_beat()),
    ]
}

proptest! {
    /// Adding a witness never takes Alive away; adding one that abstains
    /// never changes the verdict; Dead always has a Dead witness and no
    /// Alive one.
    #[test]
    fn combination_properties(
        ws in proptest::collection::vec(witness_strategy(), 0..6),
        extra in witness_strategy(),
    ) {
        let h = host();
        let before = judge_owner(&holder(ws.clone()), &h, now());
        let mut more = ws.clone();
        more.push(extra.clone());
        let after = judge_owner(&holder(more), &h, now());
        let extra_reading = judge_witness(&extra, &h, now());

        if matches!(before, OwnerVerdict::Alive { .. }) {
            prop_assert_eq!(owner_label(&after), "alive");
        }
        if matches!(extra_reading, WitnessReading::Unknown(_)) && !ws.is_empty() {
            prop_assert_eq!(owner_label(&after), owner_label(&before));
        }
        let readings: Vec<_> = ws.iter().map(|w| judge_witness(w, &h, now())).collect();
        if matches!(before, OwnerVerdict::Dead { .. }) {
            let any_dead = readings.iter().any(|r| matches!(r, WitnessReading::Dead(_)));
            let any_alive = readings.iter().any(|r| matches!(r, WitnessReading::Alive));
            prop_assert!(any_dead);
            prop_assert!(!any_alive);
        }
    }

    /// EndBySafeRule only ever follows a Dead owner and a tree that holds
    /// nothing uncommitted or unpushed.
    #[test]
    fn end_by_safe_rule_needs_a_dead_owner_and_a_clean_tree(
        ws in proptest::collection::vec(witness_strategy(), 0..4),
        dirty in any::<bool>(),
        unpushed in any::<bool>(),
        missing in any::<bool>(),
    ) {
        let tree = if missing {
            TreeReading::Missing
        } else {
            TreeReading::Read {
                dirty: if dirty { vec!["M x".into()] } else { vec![] },
                unpushed: if unpushed { vec!["c1".into()] } else { vec![] },
                head: Some("pr".into()),
            }
        };
        let v = judge(&record(Phase::Leased, ws), &host(), &tree, Some("pr"), &never_ancestor, now());
        if v.reclaim == Reclaim::EndBySafeRule {
            let owner_dead = matches!(v.owner, OwnerVerdict::Dead { .. });
            prop_assert!(owner_dead);
            prop_assert!(missing || (!dirty && !unpushed));
        }
    }
}
