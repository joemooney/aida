//! Tests for STORY-1706: the `relay.launch` parser and the restart lineage
//! file, both fail closed.
//
// trace:STORY-1706 | ai:claude

use super::*;
use std::os::unix::fs::PermissionsExt;

fn body(extra: &[&str]) -> String {
    let mut lines = vec![
        "v=1",
        "run=r1",
        "launched_at=2026-10-22T04:00:00-0700",
        "launch_cwd=/work",
        "project_root=/work",
        "relay_sh=/work/bin/relay.sh",
        "agent=claude",
        "session_role=implementer",
        "force_seat=0",
        "max_sessions=10",
        "start_session=1",
        "profile_ref=",
        "profile_file=",
        "profile_sha256=",
        "principal=joe",
        "launched_from_tty=1",
        "arg=--run",
        "arg=r1",
    ];
    lines.extend_from_slice(extra);
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// `body` with the line starting `key=` replaced (or dropped for `None`).
fn with(key: &str, value: Option<&str>) -> String {
    body(&[])
        .lines()
        .filter_map(|l| {
            if l.starts_with(&format!("{key}=")) {
                value.map(|v| format!("{key}={v}\n"))
            } else {
                Some(format!("{l}\n"))
            }
        })
        .collect()
}

#[test]
fn a_record_parses_with_its_args_in_order() {
    let r = parse(body(&["arg=do it%0Anow%25", "orig_arg=--label"]).as_bytes()).unwrap();
    assert_eq!(r.run, "r1");
    assert_eq!(r.principal, "joe");
    assert!(r.launched_from_tty && !r.force_seat);
    assert_eq!((r.max_sessions, r.start_session), (10, 1));
    assert_eq!(r.profile_file, None);
    assert_eq!(r.args, ["--run", "r1", "do it\nnow%"].map(String::from));
    assert_eq!(r.sha256.len(), 64);
}

// trace:STORY-1706 | ai:claude
#[test]
fn the_spec_field_is_optional() {
    assert_eq!(parse(body(&[]).as_bytes()).unwrap().spec, None);
    assert_eq!(parse(body(&["spec="]).as_bytes()).unwrap().spec, None);
    let r = parse(body(&["spec=STORY-1706"]).as_bytes()).unwrap();
    assert_eq!(r.spec.as_deref(), Some("STORY-1706"));
    let err = parse(body(&["spec=A-1", "spec=B-2"]).as_bytes()).unwrap_err();
    assert!(err.contains("twice"), "{err}");
}

#[test]
fn decode_refuses_escapes_relay_sh_never_writes() {
    assert_eq!(decode("a%25b%0Ac%0Dd").as_deref(), Some("a%b\nc\rd"));
    assert_eq!(decode("100%"), None);
    assert_eq!(decode("%2"), None);
    assert_eq!(decode("%41"), None);
}

#[test]
fn malformed_records_are_refused() {
    for (text, needle) in [
        (with("v", Some("2")), "unknown record version"),
        (with("v", None), "missing `v`"),
        (with("principal", None), "missing `principal`"),
        (with("principal", Some(" ")), "empty"),
        (with("force_seat", Some("yes")), "not 0 or 1"),
        (with("launched_from_tty", Some("")), "not 0 or 1"),
        (with("max_sessions", Some("-1")), "not a number"),
        (with("start_session", Some("0")), "is 0"),
        (with("relay_sh", Some("bin/relay.sh")), "absolute"),
        (with("profile_file", Some("/p.toml")), "profile without"),
        (body(&["run=r2"]), "appears twice"),
        (body(&["no equals"]), "malformed line"),
        (body(&["arg=100%"]), "bad escape"),
    ] {
        let err = parse(text.as_bytes()).unwrap_err();
        assert!(err.contains(needle), "{needle}: {err}");
    }
    let no_args: String = body(&[])
        .lines()
        .filter(|l| !l.starts_with("arg="))
        .map(|l| format!("{l}\n"))
        .collect();
    assert!(parse(no_args.as_bytes()).unwrap_err().contains("no `arg`"));
    assert!(parse(&[0xff, 0xfe]).unwrap_err().contains("UTF-8"));
}

#[test]
fn load_checks_the_file_itself() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(RECORD_FILE);
    std::fs::write(&p, body(&[])).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load(dir.path()).unwrap_err().contains("not 600"));
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    let r = load(dir.path()).unwrap();
    assert_eq!(r.sha256, sha256_hex(body(&[]).as_bytes()));
    // A symlink to a good record is still refused.
    let other = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(&p, other.path().join(RECORD_FILE)).unwrap();
    assert!(load(other.path()).is_err());
    assert!(load(tempfile::tempdir().unwrap().path()).is_err());
}

#[test]
fn the_lineage_round_trips_and_is_keyed_by_launch() {
    let dir = tempfile::tempdir().unwrap();
    assert!(restarts(dir.path(), "a").unwrap().is_empty());
    let at: DateTime<Utc> = "2026-10-22T12:00:00Z".parse().unwrap();
    let line = |launch: &str, generation| RestartLine {
        generation,
        at,
        launch: launch.into(),
        mandate: "m-1".into(),
        max_sessions: 9,
        used_before: 1,
    };
    append_restart(dir.path(), &line("a", 1)).unwrap();
    append_restart(dir.path(), &line("b", 1)).unwrap();
    append_restart(dir.path(), &line("a", 2)).unwrap();
    let got = restarts(dir.path(), "a").unwrap();
    assert_eq!(got, vec![line("a", 1), line("a", 2)]);
    let mode = std::fs::metadata(dir.path().join(RESTARTS_FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_malformed_or_foreign_lineage_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(RESTARTS_FILE);
    std::fs::write(&p, "gen=1 at=yesterday launch=a\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(restarts(dir.path(), "a").unwrap_err().contains("malformed"));
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(restarts(dir.path(), "a").is_err());
    // A symlinked lineage is neither read nor appended through.
    let real = dir.path().join("real");
    std::fs::rename(&p, &real).unwrap();
    std::os::unix::fs::symlink(&real, &p).unwrap();
    assert!(restarts(dir.path(), "a").is_err());
    let at: DateTime<Utc> = "2026-10-22T12:00:00Z".parse().unwrap();
    let line = RestartLine {
        generation: 1,
        at,
        launch: "a".into(),
        mandate: "m".into(),
        max_sessions: 1,
        used_before: 0,
    };
    assert!(append_restart(dir.path(), &line).is_err());
}
