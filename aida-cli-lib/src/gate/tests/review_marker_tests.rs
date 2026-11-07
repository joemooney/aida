use super::*;

fn claim(pr: u64, head: &str, started_at: u64, ttl: u64) -> Marker {
    Marker {
        pr,
        head_sha: head.to_string(),
        spec: Some("STORY-1".into()),
        by: "reviewer-1".into(),
        host: "some-host".into(),
        pid: 0,
        pid_start: None,
        started_at,
        ttl_secs: ttl,
    }
}

#[test]
fn no_marker_is_clear_and_never_asks_for_head() {
    let dir = tempfile::tempdir().unwrap();
    let gate = merge_gate(dir.path(), 7, || panic!("head lookup with no marker"));
    assert_eq!(gate, MergeGate::Clear);
}

#[test]
fn live_marker_at_same_head_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let m = claim(7, "abcdef1234", now_secs(), CLAIM_TTL_SECS);
    write(dir.path(), &m).unwrap();
    let gate = merge_gate(dir.path(), 7, || Some("abcdef1234567890".into()));
    assert_eq!(gate, MergeGate::UnderReview(m.clone()));
    assert!(refusal_message(&m).contains("aida review claim --pr 7 --release"));
    // Still there: a refusal does not consume the marker.
    assert!(read(dir.path(), 7).is_some());
}

#[test]
fn live_marker_with_unknown_current_head_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &claim(7, "abc", now_secs(), CLAIM_TTL_SECS)).unwrap();
    assert!(matches!(
        merge_gate(dir.path(), 7, || None),
        MergeGate::UnderReview(_)
    ));
}

#[test]
fn marker_at_different_head_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &claim(7, "aaaa111", now_secs(), CLAIM_TTL_SECS)).unwrap();
    let gate = merge_gate(dir.path(), 7, || Some("bbbb222".into()));
    assert!(matches!(gate, MergeGate::OtherHead(_)));
    assert!(proceed_note(&gate).unwrap().contains("older head"));
}

#[test]
fn ttl_expired_marker_without_ownership_record_clears() {
    let dir = tempfile::tempdir().unwrap();
    let old = now_secs() - CLAIM_TTL_SECS - 60;
    write(dir.path(), &claim(7, "abc", old, CLAIM_TTL_SECS)).unwrap();
    let gate = merge_gate(dir.path(), 7, || Some("abc".into()));
    assert!(matches!(gate, MergeGate::ExpiredCleared(_, _)));
    assert!(read(dir.path(), 7).is_none());
}

#[test]
fn dead_pid_on_this_host_expires_at_once() {
    let mut m = Marker::for_this_process(7, Some("abc"), None, "drain reviewer");
    // A pid far above any real pid_max.
    m.pid = 4_000_000_000;
    let reason = expired_reason(&m, &m.host.clone(), now_secs()).unwrap();
    assert!(reason.contains("no longer running"), "{reason}");
    // Our own live pid does not expire.
    let live = Marker::for_this_process(7, Some("abc"), None, "drain reviewer");
    assert!(expired_reason(&live, &live.host.clone(), now_secs()).is_none());
}

/// TASK-1750: our own live pid with a different recorded start time is a
/// recycled pid, so the marker expires at once; the start time round-trips.
// trace:TASK-1750 | ai:claude
#[test]
fn recycled_pid_on_this_host_expires_at_once() {
    let live = Marker::for_this_process(7, Some("abc"), None, "drain reviewer");
    if live.pid_start.is_none() {
        return; // platform cannot report start times; nothing to distinguish
    }
    assert_eq!(parse(7, &serialize(&live)), live);
    let mut recycled = live.clone();
    recycled.pid_start = Some("2000-01-01T00:00:00+00:00".into());
    let reason = expired_reason(&recycled, &recycled.host.clone(), now_secs()).unwrap();
    assert!(reason.contains("no longer running"), "{reason}");
    let legacy = parse(7, "pid=1\nhost=h\nstarted_at=1\n");
    assert_eq!(legacy.pid_start, None);
}

#[test]
fn guard_releases_on_drop_but_spares_a_newer_claim() {
    let dir = tempfile::tempdir().unwrap();
    {
        let _g = hold(
            dir.path(),
            Marker::for_this_process(7, Some("abc"), None, "drain reviewer"),
        )
        .unwrap();
        assert!(read(dir.path(), 7).is_some());
    }
    assert!(read(dir.path(), 7).is_none());

    let g = hold(
        dir.path(),
        Marker::for_this_process(8, Some("abc"), None, "drain reviewer"),
    )
    .unwrap();
    let newer = claim(8, "def", now_secs(), CLAIM_TTL_SECS);
    write(dir.path(), &newer).unwrap();
    drop(g);
    assert_eq!(read(dir.path(), 8), Some(newer));
}

#[test]
fn roundtrip_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let a = claim(9, "abc", 100, 60);
    let b = claim(3, "", 100, 60);
    write(dir.path(), &a).unwrap();
    write(dir.path(), &b).unwrap();
    assert_eq!(list(dir.path()), vec![b, a]);
}

// TASK-1459: an explicit `aida review claim` has no process to watch, so
// its TTL is capped at 24h rather than left unbounded.
// trace:TASK-1459 | ai:claude
#[test]
fn claim_ttl_within_cap_is_unchanged() {
    assert_eq!(clamp_claim_ttl_mins(1), (1, false));
    assert_eq!(clamp_claim_ttl_mins(30), (30, false));
    assert_eq!(
        clamp_claim_ttl_mins(MAX_CLAIM_TTL_MINS),
        (MAX_CLAIM_TTL_MINS, false)
    );
}

#[test]
fn claim_ttl_above_cap_is_clamped_and_flagged() {
    assert_eq!(
        clamp_claim_ttl_mins(MAX_CLAIM_TTL_MINS + 1),
        (MAX_CLAIM_TTL_MINS, true)
    );
    assert_eq!(clamp_claim_ttl_mins(u64::MAX), (MAX_CLAIM_TTL_MINS, true));
}

// trace:STORY-2325 TASK-1750 | ai:codex
#[test]
fn prc_dead_or_recycled_marker_without_ownership_clears_at_merge_gate() {
    let dir = tempfile::tempdir().unwrap();
    let live = Marker::for_this_process(7, Some("abc"), None, "drain reviewer");
    let mut dead = live.clone();
    dead.pid = 4_000_000_000;
    let mut recycled = live.clone();
    recycled.pid_start = Some("linux-starttime:0".into());
    for marker in [dead, recycled] {
        write(dir.path(), &marker).unwrap();
        assert!(!is_live_at(dir.path(), &marker));
        let MergeGate::ExpiredCleared(_, reason) = merge_gate(dir.path(), 7, || {
            panic!("dead reviewer needs no forge probe")
        }) else {
            panic!("dead reviewer must clear immediately");
        };
        assert!(reason.contains("no longer running"), "{reason}");
        assert!(read(dir.path(), 7).is_none());
    }
    write(dir.path(), &live).unwrap();
    assert!(is_live_at(dir.path(), &live));
    assert_eq!(
        merge_gate(dir.path(), 7, || Some("abc".into())),
        MergeGate::UnderReview(live)
    );
}

// trace:STORY-2325 | ai:codex
#[test]
fn prc_manual_claim_expires_without_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let m = claim(7, "ABCDEF", now_secs() - 120, 60);
    write(dir.path(), &m).unwrap();
    assert!(!is_live_at(dir.path(), &m));
    assert!(matches!(
        merge_gate(dir.path(), 7, || panic!("expired")),
        MergeGate::ExpiredCleared(_, _)
    ));
    assert!(read(dir.path(), 7).is_none());
}

// trace:STORY-2325 | ai:codex
#[test]
fn prc_closed_review_never_clears_a_fresh_manual_claim() {
    use crate::ownership::{
        store::{Expect, OwnershipStore},
        Ownership, Phase, WorkKey,
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    let store = OwnershipStore::at(root.join(".aida/ownership"));
    let key = WorkKey::Review {
        change: crate::ownership::dual::change(7),
        head: "abcdef123456".into(),
    };
    store
        .update(&key, "old-review-ended", Expect::Absent, |_| {
            Some((
                Ownership {
                    key: key.clone(),
                    generation: 0,
                    holder: None,
                    phase: Phase::Closed {
                        why: "earlier review ended".into(),
                    },
                },
                None,
            ))
        })
        .unwrap();
    for head in ["ABCDEF", ""] {
        let m = claim(7, head, now_secs(), 60);
        write(root, &m).unwrap();
        assert_eq!(
            merge_gate(root, 7, || Some("abcdef123456".into())),
            MergeGate::UnderReview(m)
        );
        assert!(read(root, 7).is_some());
    }
    // Unrelated unreadable evidence cannot make manual TTL claims permanent.
    std::fs::create_dir_all(store.dir().join("train")).unwrap();
    std::fs::write(store.dir().join("train/origin-999.json"), "{broken").unwrap();
    let m = claim(7, "ABCDEF", now_secs() - 120, 60);
    write(root, &m).unwrap();
    assert!(matches!(
        merge_gate(root, 7, || panic!("TTL expired")),
        MergeGate::ExpiredCleared(_, _)
    ));
}

// trace:STORY-2325 | ai:codex
#[test]
fn prc_only_the_markers_process_ownership_overrides_its_ttl() {
    use crate::ownership::{
        store::{Expect, OwnershipStore},
        Holder, Ownership, Phase, Role, Witness, WitnessKind, WorkKey,
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    let store = OwnershipStore::at(root.join(".aida/ownership"));
    let key = WorkKey::Review {
        change: crate::ownership::dual::change(7),
        head: "abcdef123456".into(),
    };
    let mut m = claim(7, "ABCDEF", now_secs() - 120, 60);
    m.pid = 123456;
    m.pid_start = Some("linux-starttime:42".into());
    let witness = Witness {
        admission: None,
        recorded_at: chrono::Utc::now(),
        kind: WitnessKind::Process {
            host: m.host.clone(),
            id: aida_core::process_probe::ProcessIdentity {
                pid: m.pid,
                start: format!("linux:{}:42", aida_core::local_boot_id().unwrap()),
            },
            pid_ns: None,
        },
    };
    store
        .update(&key, "seed", Expect::Absent, |_| {
            Some((
                Ownership {
                    key: key.clone(),
                    generation: 0,
                    phase: Phase::Reviewing {
                        head: "abcdef123456".into(),
                    },
                    holder: Some(Holder {
                        seat: "reviewer".into(),
                        session: "review".into(),
                        role: Role::Reviewer,
                        tree: None,
                        witnesses: vec![witness],
                    }),
                },
                None,
            ))
        })
        .unwrap();
    assert!(
        is_live_at(root, &m),
        "matching unknown ownership retains the marker past TTL"
    );
    let mut other = m.clone();
    other.pid_start = Some("linux-starttime:43".into());
    assert!(!is_live_at(root, &other), "different process uses TTL");
    store
        .update(&key, "dead", Expect::Any, |cur| {
            let mut record = cur.unwrap().clone();
            record.holder.as_mut().unwrap().witnesses.push(Witness {
                admission: None,
                recorded_at: chrono::Utc::now(),
                kind: WitnessKind::Heartbeat {
                    at: chrono::Utc::now() - chrono::Duration::days(1),
                    every_secs: 10,
                },
            });
            Some((record, None))
        })
        .unwrap();
    assert!(
        !is_live_at(root, &m),
        "matching dead ownership ends the marker"
    );
}
