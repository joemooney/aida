//! BUG-2778: a pre-start writer-admission refusal names the unverified
//! writer and retries only while every such writer runs a replaced binary.
//! trace:BUG-2778 | ai:claude

use super::activation::{admission_gap, BuildIdentity, ProcessView};
use super::guard::{retry_transient_admission_with, RetentionRefusal};
use super::retire::RefusalReason;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

const ME: (u64, u64) = (1, 10);

fn me() -> BuildIdentity {
    BuildIdentity {
        exe_id: ME,
        digest: Some("mine".into()),
        permit_marker: true,
    }
}

fn writer(pid: u32, exe: &str, id: (u64, u64)) -> ProcessView {
    ProcessView {
        pid,
        exe: Some(exe.into()),
        exe_id: Some(id),
        exe_digest: Some("0123456789abcdef".into()),
        permit_marker: Some(false),
        cmdline: vec![exe.into(), "mcp".into()],
    }
}

fn refused(transient: bool) -> Result<(), RetentionRefusal> {
    Err(RetentionRefusal {
        checkout: "/seat".into(),
        reason: "writer admission".into(),
        needs_activation: true,
        transient_admission: transient,
        retained_dir: None,
    })
}

fn transient(out: &Result<(), RetentionRefusal>) -> bool {
    matches!(out, Err(r) if r.transient_admission)
}

#[test]
fn a_deleted_binary_writer_is_named_and_transient() {
    let procs = [writer(4242, "/live/aida (deleted)", (1, 99))];
    let gap = admission_gap(&procs, Some(&me()), 1, None).expect("unverified");
    assert!(gap.transient, "{gap:?}");
    let named = gap.writers.join("; ");
    assert!(
        named.contains(
            "pid 4242 (aida, /live/aida (deleted), sha 0123456789ab, on a deleted binary)"
        ),
        "{named}"
    );
    let reason = RefusalReason::WriterAdmissionUnknown {
        writers: gap.writers,
        transient: gap.transient,
    }
    .to_string();
    assert!(
        reason.contains(
            "fresh writer-admission scan could not verify every mail writer (unverified: pid 4242"
        ),
        "{reason}"
    );
    // TASK-3922 AC2: each pid carries its own stop/restart command.
    assert!(
        named.contains(
            "pid 4242 (aida, /live/aida (deleted), sha 0123456789ab, on a deleted \
             binary): stop it with `kill 4242`, then reconnect it from its MCP client"
        ),
        "{named}"
    );
    assert!(!reason.contains("should exit on their own"), "{reason}");
    assert!(reason.contains("stop or restart each one"), "{reason}");
}

#[test]
fn a_live_unknown_writer_is_named_and_never_transient() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("aida");
    std::fs::write(&exe, b"other build").unwrap();
    let meta = std::fs::metadata(&exe).unwrap();
    let live = writer(77, exe.to_str().unwrap(), (meta.dev(), meta.ino()));
    let replaced = writer(78, "/gone/aida (deleted)", (1, 98));
    let gap = admission_gap(&[live, replaced], Some(&me()), 1, None).expect("unverified");
    assert!(!gap.transient, "{gap:?}");
    assert!(gap.writers[0].starts_with("pid 77 (aida, "), "{gap:?}");
    assert!(
        gap.writers[0].ends_with(
            "stop it with `kill 77`, then reconnect it from its MCP client (`/mcp` in Claude Code)"
        ),
        "{gap:?}"
    );
    assert!(
        gap.writers[0].contains("live: it runs a different build"),
        "{gap:?}"
    );
    let reason = RefusalReason::WriterAdmissionUnknown {
        writers: gap.writers,
        transient: gap.transient,
    }
    .to_string();
    assert!(
        reason.contains("stop or restart each live writer"),
        "{reason}"
    );
    assert!(reason.contains("activate --dry-run"), "{reason}");
}

#[test]
fn a_stale_binary_writer_is_transient_and_an_incomplete_scan_is_not() {
    // The writer's path now holds another file (or none): stale.
    let stale = writer(5, "/nonexistent/bug2778/aida", (1, 97));
    let gap = admission_gap(std::slice::from_ref(&stale), Some(&me()), 1, None).unwrap();
    assert!(
        gap.transient && gap.writers[0].contains("stale binary"),
        "{gap:?}"
    );
    let gap = admission_gap(&[stale], Some(&me()), 1, Some("hidepid".into())).unwrap();
    assert!(!gap.transient, "{gap:?}");
    assert!(gap.writers.iter().any(|w| w.contains("hidepid")), "{gap:?}");
}

#[test]
fn transient_then_verified_on_retry_lets_the_launch_proceed() {
    let mut outcomes = vec![refused(true), Ok(())].into_iter();
    let mut slept = Vec::new();
    let out =
        retry_transient_admission_with(|| outcomes.next().unwrap(), transient, |d| slept.push(d));
    assert!(out.is_ok());
    assert_eq!(slept.len(), 1);
    assert!(
        slept[0] >= Duration::from_secs(80) && slept[0] <= Duration::from_secs(120),
        "{slept:?}"
    );
}

#[test]
fn persistently_transient_refuses_after_three_bounded_tries() {
    let mut tries = 0;
    let mut slept = Duration::ZERO;
    let out = retry_transient_admission_with(
        || {
            tries += 1;
            refused(true)
        },
        transient,
        |d| slept += d,
    );
    assert!(out.is_err());
    assert_eq!(tries, 3);
    assert!(
        slept >= Duration::from_secs(200) && slept <= Duration::from_secs(300),
        "{slept:?}"
    );
}

#[test]
fn a_live_unknown_writer_fails_closed_without_retry() {
    let mut tries = 0;
    let out = retry_transient_admission_with(
        || {
            tries += 1;
            refused(false)
        },
        transient,
        |_| panic!("never waits past a live unknown writer"),
    );
    assert!(out.is_err());
    assert_eq!(tries, 1);
}

// TASK-3922 AC2: a recognised deputy is restarted with its allowlisted
// flags, an unrecognised command line is never echoed, and an unreadable one
// is named by its executable.
// trace:TASK-3922 | ai:claude
#[test]
fn a_non_mcp_writer_is_restarted_with_its_own_arguments() {
    let mut deputy = writer(9, "/gone/aida (deleted)", (1, 96));
    deputy.cmdline = ["/gone/aida", "deputy", "run", "--name", "d1"]
        .map(String::from)
        .to_vec();
    let mut long = writer(10, "/gone/aida-server (deleted)", (1, 95));
    long.cmdline = (0..9).map(|n| format!("a{n}")).collect();
    let mut bare = writer(11, "/gone/aida (deleted)", (1, 94));
    bare.cmdline.clear();
    let gap = admission_gap(&[deputy, long, bare], Some(&me()), 1, None).unwrap();
    assert!(gap.transient, "{gap:?}");
    assert!(
        gap.writers[0].ends_with("stop it with `kill 9`, then rerun `aida deputy run --name d1`"),
        "{gap:?}"
    );
    assert!(
        gap.writers[1].ends_with("then start `aida-server` again as it was started"),
        "{gap:?}"
    );
    assert!(!gap.writers[1].contains("a1"), "{gap:?}");
    assert!(
        gap.writers[2]
            .ends_with("stop it with `kill 11`, then start `aida` again as it was started"),
        "{gap:?}"
    );
}

// TASK-3922 AC3: retire_with maps a transient writer-admission refusal to
// `transient_admission`, and a live unknown writer to a non-transient one.
// trace:TASK-3922 | ai:claude
#[test]
fn retire_with_maps_a_transient_admission_refusal() {
    use super::retire::RetireOutcome;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    for transient in [true, false] {
        let gate = super::guard::retire_with(dir.path(), "test", |_| {
            Ok((
                RetireOutcome::Refused {
                    reason: RefusalReason::WriterAdmissionUnknown {
                        writers: vec!["pid 1".into()],
                        transient,
                    },
                    retained_dir: None,
                },
                None,
            ))
        });
        let refusal = gate.err().expect("refused");
        assert_eq!(refusal.transient_admission, transient, "{refusal:?}");
        assert!(refusal.needs_activation, "{refusal:?}");
    }
    let gate = super::guard::retire_with(dir.path(), "test", |_| {
        Ok((
            RetireOutcome::Refused {
                reason: RefusalReason::MailBearingBeforeActivation,
                retained_dir: None,
            },
            None,
        ))
    });
    assert!(!gate.err().expect("refused").transient_admission);
}

// TASK-3922 r1: a credential on a writer's command line never reaches the
// refusal, whether passed as `--flag VALUE`, `--flag=VALUE` or `key=value`,
// while a recognised script deputy still gets its allowlisted restart hint.
// trace:TASK-3922 | ai:claude
#[test]
fn a_writer_refusal_never_echoes_a_credential_argument() {
    let argv = |args: &[&str]| args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    let mut deputy = writer(21, "/gone/aida (deleted)", (1, 93));
    deputy.cmdline = argv(&[
        "/gone/aida",
        "--session-grant",
        "XYZ",
        "deputy",
        "shell",
        "run",
        "--token=ABC",
        "token=DEF",
        "--name",
        "ops",
        "--loop",
    ]);
    let mut other = writer(22, "/gone/aida (deleted)", (1, 92));
    other.cmdline = argv(&[
        "/gone/aida",
        "--Session-Grant",
        "XYZ",
        "--token=ABC",
        "token=DEF",
    ]);
    let gap = admission_gap(&[deputy.clone(), other], Some(&me()), 1, None).unwrap();
    let reason = RefusalReason::WriterAdmissionUnknown {
        writers: gap.writers.clone(),
        transient: gap.transient,
    }
    .to_string();
    for secret in ["XYZ", "ABC", "DEF"] {
        assert!(!reason.contains(secret), "{secret} leaked: {reason}");
    }
    assert!(
        gap.writers[0].ends_with(
            "stop it with `kill 21`, then rerun `aida deputy shell run --name ops --loop`"
        ),
        "{gap:?}"
    );
    assert!(
        gap.writers[1].ends_with("then start `aida` again as it was started"),
        "{gap:?}"
    );
    // The dry-run listing redacts the same values.
    let listed = super::activation::redacted_command(&deputy.cmdline, 11);
    for secret in ["XYZ", "ABC", "DEF"] {
        assert!(!listed.contains(secret), "{secret} leaked: {listed}");
    }
    assert!(listed.contains("--session-grant <redacted>"), "{listed}");
    assert!(
        listed.contains("--token=<redacted> token=<redacted>"),
        "{listed}"
    );
}
