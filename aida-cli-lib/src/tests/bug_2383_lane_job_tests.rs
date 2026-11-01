//! BUG-2383: the lane job runner. A fixture lane hands filtered cargo test
//! runs to the runner and reads pass/fail back; argv outside the allow-list
//! is refused.
// trace:BUG-2383 | ai:claude

use super::*;

fn job(argv: &[&str]) -> JobRequest {
    JobRequest {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        env: BTreeMap::new(),
    }
}

#[test]
fn allow_list_accepts_targeted_agent_profile_runs() {
    for argv in [
        &[
            "cargo",
            "test",
            "--profile",
            "agent",
            "-p",
            "aida-cli-lib",
            "--lib",
            "--",
            "lane_job",
        ][..],
        &["cargo", "check", "--profile=agent", "-p", "aida-core"][..],
        &["cargo", "build", "--profile", "agent", "--bin", "aida"][..],
        // Anything after `--` belongs to the test binary.
        &[
            "cargo",
            "test",
            "--profile",
            "agent",
            "--",
            "--config",
            "-Z",
        ][..],
    ] {
        assert_eq!(validate(&job(argv)), Ok(()), "{argv:?}");
    }
}

#[test]
fn allow_list_refuses_other_argv() {
    for argv in [
        &["sh", "-c", "cargo test --profile agent"][..],
        &["cargo", "run", "--profile", "agent"][..],
        &["cargo", "install", "--profile", "agent", "x"][..],
        &["cargo", "+nightly", "test", "--profile", "agent"][..],
        &["cargo", "test"][..],
        &["cargo", "test", "--profile", "release"][..],
        &["cargo", "test", "--release"][..],
        &[
            "cargo",
            "test",
            "--profile",
            "agent",
            "--config",
            "build.rustc-wrapper='x'",
        ][..],
        &["cargo", "test", "--profile", "agent", "--config=x"][..],
        &[
            "cargo",
            "test",
            "--profile",
            "agent",
            "--manifest-path",
            "/tmp/x/Cargo.toml",
        ][..],
        &[
            "cargo",
            "test",
            "--profile",
            "agent",
            "--target-dir",
            "/tmp/t",
        ][..],
        &["cargo", "test", "--profile", "agent", "-Zunstable-options"][..],
        &["cargo", "test", "--profile", "agent", "-C", "/elsewhere"][..],
        &["cargo", "test", "--profile", "agent", "+nightly"][..],
        &[][..],
    ] {
        assert!(validate(&job(argv)).is_err(), "{argv:?} must be refused");
    }
    let mut env_job = job(&["cargo", "test", "--profile", "agent"]);
    env_job.env.insert("LD_PRELOAD".into(), "/x.so".into());
    assert!(validate(&env_job).is_err(), "only allow-listed env");
}

#[test]
fn timeout_honours_aida_cargo_timeout_within_bounds() {
    let mut j = job(&["cargo", "test", "--profile", "agent"]);
    assert_eq!(timeout_for(&j), Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    j.env.insert("AIDA_CARGO_TIMEOUT".into(), "900".into());
    assert_eq!(timeout_for(&j), Duration::from_secs(900));
    j.env.insert("AIDA_CARGO_TIMEOUT".into(), "99999".into());
    assert_eq!(timeout_for(&j), Duration::from_secs(MAX_TIMEOUT_SECS));
    j.env.insert("AIDA_CARGO_TIMEOUT".into(), "nonsense".into());
    assert_eq!(timeout_for(&j), Duration::from_secs(DEFAULT_TIMEOUT_SECS));
}

#[test]
fn job_ids_are_bounded_and_path_safe() {
    assert!(valid_id(&new_id()));
    assert_ne!(new_id(), new_id());
    let long = "x".repeat(65);
    for bad in ["", "../x", "a/b", "a.b", long.as_str()] {
        assert!(!valid_id(bad), "{bad:?}");
    }
}

#[test]
fn seat_side_job_carries_only_allow_listed_env() {
    let job = crate::lane_cmd::job_from(
        vec![
            "cargo".into(),
            "test".into(),
            "--profile".into(),
            "agent".into(),
        ],
        |k| match k {
            "AIDA_CARGO_TIMEOUT" => Some("900".into()),
            "PATH" => Some("/evil".into()),
            _ => None,
        },
    );
    assert_eq!(job.env.len(), 1);
    assert_eq!(
        job.env.get("AIDA_CARGO_TIMEOUT").map(String::as_str),
        Some("900")
    );
}

/// A fixture lane: a seat dir, a host results dir, and a fake `cargo` that
/// passes a test filter named `passing` and fails any other.
#[cfg(unix)]
fn fixture_lane(tmp: &Path) -> LaneJobs {
    use std::os::unix::fs::PermissionsExt;
    let seat = tmp.join("aida-seat-fixture");
    std::fs::create_dir_all(&seat).unwrap();
    let fake = tmp.join("fake-cargo");
    std::fs::write(
        &fake,
        "#!/bin/sh\n\
         filter=\"\"\n\
         seen=0\n\
         for a in \"$@\"; do\n\
           if [ \"$seen\" = 1 ]; then filter=\"$a\"; fi\n\
           [ \"$a\" = -- ] && seen=1\n\
         done\n\
         echo \"cwd=$(pwd) target=$CARGO_TARGET_DIR lane=$AIDA_HEADLESS_LANE timeout=$AIDA_CARGO_TIMEOUT\"\n\
         echo \"running 1 test\"\n\
         if [ \"$filter\" = passing ]; then\n\
           echo \"test $filter ... ok\"; echo \"test result: ok. 1 passed; 0 failed\"; exit 0\n\
         fi\n\
         echo \"test $filter ... FAILED\"; echo \"test result: FAILED. 0 passed; 1 failed\"; exit 101\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let lane = LaneJobs {
        inbox: seat.join(INBOX_RELATIVE),
        results: results_dir(&tmp.join("main"), "lease-fixture"),
        seat,
        lease_id: "lease-fixture".into(),
        build_env: vec![(
            "CARGO_TARGET_DIR".into(),
            tmp.join("seat-target").display().to_string(),
        )],
        program: fake.display().to_string(),
    };
    lane.prepare().unwrap();
    lane
}

#[cfg(unix)]
fn run_through(lane: &LaneJobs, j: &JobRequest) -> (i32, String) {
    let id = new_id();
    submit(&lane.inbox, &id, j).unwrap();
    match wait_for(
        &lane.inbox,
        &lane.results,
        &id,
        Duration::from_secs(30),
        Duration::from_secs(60),
        |_, _| {},
    ) {
        Waited::Done(rc) => (
            rc,
            std::fs::read_to_string(out_path(&lane.results, &id)).unwrap(),
        ),
        other => panic!("job {id} did not finish: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn fixture_lane_runs_a_filtered_test_through_the_runner_and_reports_pass_fail() {
    let tmp = tempfile::tempdir().unwrap();
    let lane = fixture_lane(tmp.path());
    let runner = Runner::start(lane.clone());

    let mut pass = job(&[
        "cargo",
        "test",
        "--profile",
        "agent",
        "-p",
        "aida-cli-lib",
        "--lib",
        "--",
        "passing",
    ]);
    pass.env.insert("AIDA_CARGO_TIMEOUT".into(), "900".into());
    let (rc, out) = run_through(&lane, &pass);
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains("test result: ok"), "{out}");
    // It ran in the seat, with the seat's target, the lane marker and the
    // job's timeout.
    assert!(
        out.contains(&format!("cwd={}", lane.seat.display())),
        "{out}"
    );
    assert!(
        out.contains("target=") && out.contains("seat-target"),
        "{out}"
    );
    assert!(out.contains("lane=lease-fixture"), "{out}");
    assert!(out.contains("timeout=900"), "{out}");

    let fail = job(&[
        "cargo",
        "test",
        "--profile",
        "agent",
        "-p",
        "aida-cli-lib",
        "--lib",
        "--",
        "broken",
    ]);
    let (rc, out) = run_through(&lane, &fail);
    assert_eq!(rc, 101, "{out}");
    assert!(out.contains("test result: FAILED"), "{out}");

    let tally = runner.stop();
    assert_eq!(tally.completed.load(Ordering::Relaxed), 2);
    assert!(!tally.untested());
    let log = std::fs::read_to_string(lane.results.join("jobs.log")).unwrap();
    assert_eq!(log.matches(" started: cargo test").count(), 2, "{log}");
}

#[cfg(unix)]
#[test]
fn runner_refuses_non_allow_listed_argv_and_never_runs_it() {
    let tmp = tempfile::tempdir().unwrap();
    let lane = fixture_lane(tmp.path());
    let runner = Runner::start(lane.clone());

    let (rc, out) = run_through(&lane, &job(&["cargo", "run", "--profile", "agent"]));
    assert_eq!(rc, REFUSED_RC);
    assert!(out.contains("refused"), "{out}");
    assert!(
        !out.contains("running 1 test"),
        "the fake cargo never ran: {out}"
    );

    // A job file that is a symlink is refused, not followed.
    let id = new_id();
    let elsewhere = tmp.path().join("elsewhere.json");
    std::fs::write(
        &elsewhere,
        serde_json::to_vec(&job(&[
            "cargo",
            "test",
            "--profile",
            "agent",
            "--",
            "passing",
        ]))
        .unwrap(),
    )
    .unwrap();
    std::os::unix::fs::symlink(&elsewhere, lane.inbox.join(format!("{id}.job"))).unwrap();
    let waited = wait_for(
        &lane.inbox,
        &lane.results,
        &id,
        Duration::from_secs(30),
        Duration::from_secs(60),
        |_, _| {},
    );
    assert_eq!(waited, Waited::Done(REFUSED_RC));

    let tally = runner.stop();
    assert_eq!(tally.completed.load(Ordering::Relaxed), 0);
    assert!(
        tally.untested(),
        "a lane with only refused jobs is untested"
    );
}

#[cfg(unix)]
#[test]
fn a_job_still_running_at_lane_end_is_stopped() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let mut lane = fixture_lane(tmp.path());
    let slow = tmp.path().join("slow-cargo");
    std::fs::write(&slow, "#!/bin/sh\necho started\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();
    lane.program = slow.display().to_string();
    let runner = Runner::start(lane.clone());
    let id = new_id();
    submit(
        &lane.inbox,
        &id,
        &job(&["cargo", "test", "--profile", "agent"]),
    )
    .unwrap();
    let waited = wait_for(
        &lane.inbox,
        &lane.results,
        &id,
        Duration::from_secs(2),
        Duration::from_secs(60),
        |_, _| {},
    );
    assert!(matches!(waited, Waited::Running { .. }), "{waited:?}");
    let tally = runner.stop();
    assert_eq!(read_rc(&lane.results, &id), Some(STOPPED_RC));
    assert!(tally.untested());
}

#[test]
fn result_line_says_untested_when_no_job_completed() {
    use crate::standalone_implementer::{with_untested, Outcome};
    let opened = Outcome::PrOpened {
        pr: 7,
        head: Some("abc".into()),
    };
    assert_eq!(
        with_untested("implementer: pr-opened pr=7 head=abc".into(), &opened, true),
        "implementer: pr-opened pr=7 head=abc untested=true"
    );
    assert_eq!(
        with_untested(
            "implementer: pr-opened pr=7 head=abc".into(),
            &opened,
            false
        ),
        "implementer: pr-opened pr=7 head=abc"
    );
    let punted = Outcome::Punted { reason: "x".into() };
    assert_eq!(
        with_untested("implementer: punted reason=x".into(), &punted, true),
        "implementer: punted reason=x"
    );
}

#[test]
fn implementer_prompt_names_the_sanctioned_path() {
    let contract = crate::standalone_implementer::LANE_JOB_CONTRACT;
    assert!(contract.contains("aida lane job run -- cargo test --profile agent"));
    assert!(contract.contains("aida lane job wait"));
    assert!(contract.contains("untested=true"));
    assert!(contract.contains("do not run cargo yourself"));
    assert!(contract.contains("setsid nohup"), "names what it replaces");
}
