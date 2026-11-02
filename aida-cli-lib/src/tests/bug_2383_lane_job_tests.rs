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
            "/elsewhere/x/Cargo.toml",
        ][..],
        &[
            "cargo",
            "test",
            "--profile",
            "agent",
            "--target-dir",
            "/elsewhere/t",
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
    assert_eq!(
        timeout_for(&j, false),
        Duration::from_secs(DEFAULT_TIMEOUT_SECS)
    );
    j.env.insert("AIDA_CARGO_TIMEOUT".into(), "900".into());
    assert_eq!(timeout_for(&j, false), Duration::from_secs(900));
    assert_eq!(
        timeout_for(&j, true),
        Duration::from_secs(900),
        "an explicit timeout wins"
    );
    j.env.insert("AIDA_CARGO_TIMEOUT".into(), "99999".into());
    assert_eq!(
        timeout_for(&j, false),
        Duration::from_secs(MAX_TIMEOUT_SECS)
    );
    j.env.insert("AIDA_CARGO_TIMEOUT".into(), "nonsense".into());
    assert_eq!(
        timeout_for(&j, false),
        Duration::from_secs(DEFAULT_TIMEOUT_SECS)
    );
}

// Round-3 finding 7: on a host without reflinks every seat target starts cold,
// and a cold aida-cli-lib build outlasts the default. trace:BUG-2383 | ai:claude
#[test]
fn an_unseeded_target_gets_the_largest_timeout_by_default() {
    let j = job(&["cargo", "test", "--profile", "agent"]);
    assert_eq!(timeout_for(&j, true), Duration::from_secs(MAX_TIMEOUT_SECS));
    use crate::seat_target_seed::{is_cold, Seeded};
    assert!(is_cold(&[(
        "agent".into(),
        Seeded::Skipped("no reflinks".into())
    )]));
    assert!(is_cold(&[]));
    assert!(!is_cold(&[("agent".into(), Seeded::Reflinked)]));
    assert!(!is_cold(&[("agent".into(), Seeded::AlreadyWarm)]));
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
    let seat = tmp.join("aida-seat-fixture");
    std::fs::create_dir_all(&seat).unwrap();
    let fake = tmp.join("fake-cargo");
    crate::test_exec::write_executable(&fake,
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
    );
    let lane = LaneJobs {
        inbox: seat.join(INBOX_RELATIVE),
        results: results_dir(&tmp.join("main"), "lease-fixture"),
        seat,
        lease_id: "lease-fixture".into(),
        build_env: vec![(
            "CARGO_TARGET_DIR".into(),
            tmp.join("seat-target").display().to_string(),
        )],
        host_env: scrub_host_env(dirty_host_env(tmp)),
        program: fake.display().to_string(),
        sandbox: Sandbox::TestUnwrapped,
    };
    lane.prepare().unwrap();
    lane
}

/// A launcher environment full of authority a job must never see, plus the
/// keys it may.
fn dirty_host_env(tmp: &Path) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = [
        ("AIDA_SESSION_GRANT", "grant-secret"),
        ("AIDA_SESSION_ROLE", "implementer"),
        ("AIDA_SESSION_ID", "s-1"),
        ("AIDA_HANDLE", "claude-implementer-1"),
        ("AIDA_REVIEW_VERDICT_FILE", "/host/verdict"),
        ("AIDA_ORCHESTRATOR_TOKEN", "tok"),
        ("AIDA_LANE_JOBS_INBOX", "/x"),
        ("AIDA_LANE_JOBS_RESULTS", "/y"),
        ("AIDA_CARGO_NO_SLOT", "1"),
        ("AIDA_CARGO_SLOT_HELD", "3"),
        ("SSH_AUTH_SOCK", "/run/ssh.sock"),
        ("GH_TOKEN", "ghp_secret"),
        ("GITHUB_TOKEN", "ghs_secret"),
        ("LD_PRELOAD", "/evil.so"),
        ("LANG", "C.UTF-8"),
        ("LC_ALL", "C"),
        ("CARGO_TERM_COLOR", "never"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
    .collect();
    env.push(("PATH".into(), std::env::var("PATH").unwrap_or_default()));
    env.push(("HOME".into(), tmp.join("home").display().to_string()));
    env
}

/// Every key a job may see: the allow-lists plus the lane marker.
fn allowed_job_key(key: &str) -> bool {
    host_key_allowed(key)
        || BUILD_ENV_ALLOWED.contains(&key)
        || ALLOWED_ENV.contains(&key)
        || key == crate::headless_background::LANE_MARKER_ENV
        || key == OFFLINE_ENV.0
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
    // The output names the seed path and the cargo it resolved to.
    assert!(out.contains("lane job: seat target "), "{out}");
    assert!(
        out.contains(&format!("lane job: cargo is {}", lane.program)),
        "{out}"
    );

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
    let tmp = tempfile::tempdir().unwrap();
    let mut lane = fixture_lane(tmp.path());
    let slow = tmp.path().join("slow-cargo");
    crate::test_exec::write_executable(&slow, "#!/bin/sh\necho started\nexec sleep 30\n");
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

// Round 2, ruling 2: a job starts from a cleared environment plus the
// allow-lists. trace:BUG-2383 | ai:claude
#[test]
fn job_env_carries_only_allow_listed_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let lane = LaneJobs {
        seat: tmp.path().join("seat"),
        inbox: tmp.path().join("seat").join(INBOX_RELATIVE),
        results: tmp.path().join("results"),
        lease_id: "lease-env".into(),
        build_env: vec![
            ("CARGO_TARGET_DIR".into(), "/t/seat".into()),
            ("RUSTC_WRAPPER".into(), String::new()),
            ("AIDA_SESSION_GRANT".into(), "smuggled".into()),
        ],
        // Unscrubbed on purpose: job_env filters again.
        host_env: dirty_host_env(tmp.path()),
        program: "cargo".into(),
        sandbox: Sandbox::Bwrap,
    };
    let mut j = job(&["cargo", "test", "--profile", "agent"]);
    j.env.insert("AIDA_CARGO_TIMEOUT".into(), "900".into());
    j.env.insert("GH_TOKEN".into(), "from-the-seat".into());
    let env = lane.job_env(&j);
    for key in env.keys() {
        assert!(allowed_job_key(key), "{key} reached a job");
    }
    for denied in [
        "AIDA_SESSION_GRANT",
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_ID",
        "AIDA_HANDLE",
        "AIDA_REVIEW_VERDICT_FILE",
        "AIDA_ORCHESTRATOR_TOKEN",
        "AIDA_LANE_JOBS_INBOX",
        "AIDA_CARGO_NO_SLOT",
        "AIDA_CARGO_SLOT_HELD",
        "SSH_AUTH_SOCK",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "LD_PRELOAD",
    ] {
        assert!(!env.contains_key(denied), "{denied} reached a job");
    }
    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/t/seat")
    );
    assert_eq!(
        env.get("AIDA_CARGO_TIMEOUT").map(String::as_str),
        Some("900")
    );
    assert_eq!(env.get("LC_ALL").map(String::as_str), Some("C"));
    assert_eq!(
        env.get(crate::headless_background::LANE_MARKER_ENV)
            .map(String::as_str),
        Some("lease-env")
    );
    // Every job runs offline: cargo home is read-only in the sandbox.
    assert_eq!(
        env.get("CARGO_NET_OFFLINE").map(String::as_str),
        Some("true")
    );
    // The scrub at construction drops the same keys.
    let scrubbed = scrub_host_env(dirty_host_env(tmp.path()));
    assert!(scrubbed.iter().all(|(k, _)| host_key_allowed(k)));
    assert!(!scrubbed.iter().any(|(k, _)| k == "AIDA_SESSION_GRANT"));
}

#[cfg(unix)]
#[test]
fn a_job_that_prints_its_env_shows_only_allow_listed_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lane = fixture_lane(tmp.path());
    // Unscrubbed, so the run itself (env_clear) is what keeps them out.
    lane.host_env = dirty_host_env(tmp.path());
    let printer = tmp.path().join("env-cargo");
    crate::test_exec::write_executable(&printer, "#!/bin/sh\necho ENV-BEGIN\nenv\necho ENV-END\n");
    lane.program = printer.display().to_string();
    let runner = Runner::start(lane.clone());
    let (rc, out) = run_through(&lane, &job(&["cargo", "test", "--profile", "agent"]));
    runner.stop();
    assert_eq!(rc, 0, "{out}");
    let body = out
        .split("ENV-BEGIN\n")
        .nth(1)
        .and_then(|rest| rest.split("ENV-END").next())
        .expect("the env listing");
    // The shell itself may export these.
    let shell_noise = ["PWD", "OLDPWD", "SHLVL", "_"];
    let keys: Vec<&str> = body
        .lines()
        .filter_map(|line| line.split_once('=').map(|(k, _)| k))
        .filter(|k| !shell_noise.contains(k))
        .collect();
    assert!(!keys.is_empty(), "{out}");
    for key in &keys {
        assert!(allowed_job_key(key), "{key} reached a job: {out}");
    }
    assert!(
        !out.contains("grant-secret") && !out.contains("ghp_secret"),
        "{out}"
    );
}

// Round 2, ruling 3: with os_wrap off (or bubblewrap unusable) a job is
// refused and nothing runs. trace:BUG-2383 | ai:claude
#[cfg(unix)]
#[test]
fn os_wrap_off_refuses_the_job_and_nothing_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lane = fixture_lane(tmp.path());
    // The fixture seat is no git checkout and has no `[contained] os_wrap`.
    lane.sandbox = Sandbox::Bwrap;
    let runner = Runner::start(lane.clone());
    let (rc, out) = run_through(
        &lane,
        &job(&["cargo", "test", "--profile", "agent", "--", "passing"]),
    );
    let tally = runner.stop();
    assert_eq!(rc, REFUSED_RC, "{out}");
    assert!(out.contains("lane job refused"), "{out}");
    assert!(!out.contains("running 1 test"), "the fake cargo ran: {out}");
    assert!(tally.untested());
    assert_eq!(tally.completed.load(Ordering::Relaxed), 0);
}

// Round 2, ruling 5 (AC1): the job goes through the slot limiter, the `cargo`
// on the job's PATH, and the output and lane log name it. trace:BUG-2383 | ai:claude
#[cfg(unix)]
#[test]
fn a_job_takes_the_cargo_slot_through_the_limiter_on_path() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lane = fixture_lane(tmp.path());
    let bin = tmp.path().join("bin");
    let run = tmp.path().join("run");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&run).unwrap();
    let limiter = bin.join("cargo");
    crate::test_exec::write_executable(
        &limiter,
        "#!/bin/sh\necho \"slot taken: $*\" >> \"$XDG_RUNTIME_DIR/slots.log\"\n\
         echo \"running 1 test\"\nexit 0\n",
    );
    lane.program = "cargo".into();
    lane.host_env = scrub_host_env(vec![
        (
            "PATH".to_string(),
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        ),
        ("XDG_RUNTIME_DIR".to_string(), run.display().to_string()),
    ]);
    let runner = Runner::start(lane.clone());
    let (rc, out) = run_through(
        &lane,
        &job(&["cargo", "test", "--profile", "agent", "--", "x"]),
    );
    runner.stop();
    assert_eq!(rc, 0, "{out}");
    let slots = std::fs::read_to_string(run.join("slots.log")).unwrap();
    assert!(
        slots.contains("slot taken: test --profile agent -- x"),
        "{slots}"
    );
    let named = format!("cargo is {}", limiter.display());
    assert!(out.contains(&named), "{out}");
    let log = std::fs::read_to_string(lane.results.join("jobs.log")).unwrap();
    assert!(log.contains(&named), "{log}");
}

#[test]
fn sandbox_writes_only_the_target_and_slot_state_never_cargo_home() {
    let lane = LaneJobs {
        seat: PathBuf::from("/w/aida-seat-x"),
        inbox: PathBuf::from("/w/aida-seat-x").join(INBOX_RELATIVE),
        results: PathBuf::from("/m/.aida/lane-jobs/l"),
        lease_id: "l".into(),
        build_env: vec![("CARGO_TARGET_DIR".into(), "/m/target/worktrees/x".into())],
        host_env: Vec::new(),
        program: "cargo".into(),
        sandbox: Sandbox::Bwrap,
    };
    let env: BTreeMap<String, String> = [
        ("HOME".to_string(), "/home/u".to_string()),
        ("XDG_RUNTIME_DIR".to_string(), "/run/user/7".to_string()),
    ]
    .into_iter()
    .collect();
    let rw = lane.sandbox_rw_paths(&env);
    let slot_dir = format!("/run/user/7/cargo-slots-{}", current_uid());
    assert_eq!(
        rw,
        vec![
            PathBuf::from("/m/target/worktrees/x"),
            PathBuf::from(&slot_dir),
            PathBuf::from("/home/u/.local/state/cargo-slots"),
        ]
    );
    let argv = crate::session::lane_job_bwrap_argv(
        &lane.seat,
        &rw,
        &[],
        "/home/u/.local/bin/cargo",
        vec!["test".into()],
    );
    // Read-only root; the seat and the named paths are the only rw binds.
    assert_eq!(&argv[..3], &["--ro-bind", "/", "/"]);
    let rw_binds: Vec<&str> = argv
        .windows(2)
        .filter(|w| w[0] == "--bind" || w[0] == "--bind-try")
        .map(|w| w[1].as_str())
        .collect();
    assert_eq!(
        rw_binds,
        vec![
            "/w/aida-seat-x",
            "/m/target/worktrees/x",
            slot_dir.as_str(),
            "/home/u/.local/state/cargo-slots",
        ]
    );
    assert!(!argv.iter().any(|a| a.contains(".claude")), "{argv:?}");
    // Round-3 finding 5 and the round-3 ruling: no file the host's next
    // cargo runs is writable, whether as a bind of its own or under a
    // writable parent: the rustup proxies in bin/, config.toml (a
    // build.rustc-wrapper), and the extracted dependency sources (their
    // build.rs) in registry/src/ and git/checkouts/, nor the download caches
    // they are extracted from. trace:BUG-2383 | ai:claude
    for host_code in [
        "/home/u/.cargo/bin/cargo",
        "/home/u/.cargo/config.toml",
        "/home/u/.cargo/registry/src/index.crates.io-0/serde-1.0.0/build.rs",
        "/home/u/.cargo/registry/cache/index.crates.io-0/serde-1.0.0.crate",
        "/home/u/.cargo/git/checkouts/dep-0/abc/build.rs",
        "/home/u/.cargo/git/db/dep-0/objects",
    ] {
        let covering: Vec<&&str> = rw_binds
            .iter()
            .filter(|bind| std::path::Path::new(host_code).starts_with(bind))
            .collect();
        assert!(
            covering.is_empty(),
            "{host_code} is writable through {covering:?}"
        );
    }
    // A CARGO_HOME elsewhere is cut the same way.
    let mut custom = env.clone();
    custom.insert("CARGO_HOME".into(), "/opt/cargo".into());
    let rw = lane.sandbox_rw_paths(&custom);
    assert!(
        !rw.iter()
            .any(|p| Path::new("/opt/cargo/registry/src").starts_with(p)
                || p.starts_with("/opt/cargo")),
        "{rw:?}"
    );
    assert_eq!(
        &argv[argv.len() - 2..],
        &["/home/u/.local/bin/cargo", "test"]
    );
}

/// Whether bubblewrap can create a sandbox on this host.
#[cfg(unix)]
fn bwrap_usable() -> bool {
    std::process::Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--dev", "/dev", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

// Round-3 advisor ruling: a job inside the real bubblewrap profile cannot
// change a host dependency's build.rs in registry/src/ or git/checkouts/,
// while it can still write its seat. Skipped where bubblewrap cannot run
// (unprivileged user namespaces off). trace:BUG-2383 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2383_bwrap_host_cargo_is_read_only() {
    if !bwrap_usable() {
        assert!(
            std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none(),
            "AIDA_REQUIRE_BWRAP_LIVE requires the lane job isolation test to run"
        );
        eprintln!("SKIP (bwrap unavailable)");
        return;
    }
    // Not under /tmp: the profile mounts a private tmpfs there, which would
    // hide the fixture and let the write "fail" for the wrong reason.
    let base_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/bug-2383-bwrap");
    std::fs::create_dir_all(&base_dir).unwrap();
    let base = tempfile::tempdir_in(base_dir).unwrap();
    let home = base.path().join("home");
    let cargo_home = home.join(".cargo");
    let seat = base.path().join("seat");
    let target = base.path().join("target");
    std::fs::create_dir_all(&seat).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    let original = "fn main() {}\n";
    let scripts = [
        cargo_home.join("bin/cargo"),
        cargo_home.join("bin/rustc"),
        cargo_home.join("bin/rustup"),
        cargo_home.join("config"),
        cargo_home.join("config.toml"),
        cargo_home.join("registry/cache/index.crates.io-0/dep-1.0.0.crate"),
        cargo_home.join("git/db/dep-0/objects/probe"),
        cargo_home.join("registry/src/index.crates.io-0/dep-1.0.0/build.rs"),
        cargo_home.join("git/checkouts/dep-0/abc/build.rs"),
    ];
    for script in &scripts {
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(script, original).unwrap();
    }
    let lane = LaneJobs {
        seat: seat.clone(),
        inbox: seat.join(INBOX_RELATIVE),
        results: base.path().join("results"),
        lease_id: "lease-bwrap".into(),
        build_env: vec![("CARGO_TARGET_DIR".into(), target.display().to_string())],
        host_env: Vec::new(),
        program: "cargo".into(),
        sandbox: Sandbox::Bwrap,
    };
    let env: BTreeMap<String, String> = [("HOME".to_string(), home.display().to_string())]
        .into_iter()
        .collect();
    let rw = lane.sandbox_rw_paths(&env);
    let run = |script: &str| {
        let argv = crate::session::lane_job_bwrap_argv(
            &seat,
            &rw,
            &[],
            "/bin/sh",
            vec!["-c".into(), script.into()],
        );
        std::process::Command::new("bwrap")
            .args(&argv)
            .output()
            .unwrap()
    };
    // Control: the seat and the target are writable, so a refusal below is
    // the profile's doing.
    let wrote = run(&format!(
        "echo ok > '{}/probe' && echo ok > '{}/probe'",
        seat.display(),
        target.display()
    ));
    assert!(wrote.status.success(), "{wrote:?}");
    for script in &scripts {
        let attempt = run(&format!(
            "test -f '{0}' || exit 3; echo 'compile_error!(\"pwned\");' > '{0}' || exit 1; exit 0",
            script.display()
        ));
        assert_eq!(
            attempt.status.code(),
            Some(1),
            "the write to {} was not refused: {attempt:?}",
            script.display()
        );
        assert_eq!(std::fs::read_to_string(script).unwrap(), original);
    }
}

// Finding 5: a check or build is not a test. trace:BUG-2383 | ai:claude
#[test]
fn only_a_test_job_counts_as_tested() {
    let tally = Tally::default();
    tally.record_exit(
        &job(&["cargo", "check", "--profile", "agent"]),
        &b"test result: ok. 1 passed; 0 failed"[..],
    );
    tally.record_exit(
        &job(&["cargo", "build", "--profile", "agent"]),
        &b"test result: ok. 1 passed; 0 failed"[..],
    );
    assert_eq!(tally.completed.load(Ordering::Relaxed), 2);
    assert!(tally.untested(), "check and build alone are untested");
    // Round-3 finding 6: a test build that runs no test is not a test.
    tally.record_exit(
        &job(&["cargo", "test", "--profile", "agent", "--no-run"]),
        &b"test result: ok. 1 passed; 0 failed"[..],
    );
    tally.record_exit(
        &job(&["cargo", "test", "--profile", "agent", "--", "--list"]),
        &b"test result: ok. 1 passed; 0 failed"[..],
    );
    assert!(tally.untested(), "--no-run and --list run no test");
    tally.record_exit(
        &job(&["cargo", "test", "--profile", "agent"]),
        &b"test result: ok. 1 passed; 0 failed"[..],
    );
    assert!(!tally.untested());
    // `--no-run` after `--` is a test filter, and `--list` before it is not
    // the harness's flag.
    assert!(runs_tests(&job(&[
        "cargo",
        "test",
        "--profile",
        "agent",
        "--",
        "--no-run",
    ])));
}

// Round-3 finding 6: a run killed by a signal did not exit by itself, so it
// counts as incomplete, never as tested. trace:BUG-2383 | ai:claude
#[cfg(unix)]
#[test]
fn a_job_killed_by_a_signal_is_not_tested() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lane = fixture_lane(tmp.path());
    let killed = tmp.path().join("killed-cargo");
    crate::test_exec::write_executable(&killed, "#!/bin/sh\necho \"running 1 test\"\nkill -9 $$\n");
    lane.program = killed.display().to_string();
    let runner = Runner::start(lane.clone());
    let (rc, out) = run_through(&lane, &job(&["cargo", "test", "--profile", "agent"]));
    let tally = runner.stop();
    assert_eq!(rc, STOPPED_RC, "{out}");
    assert!(out.contains("killed by a signal"), "{out}");
    assert!(tally.untested());
    assert_eq!(tally.completed.load(Ordering::Relaxed), 0);
    assert_eq!(tally.incomplete.load(Ordering::Relaxed), 1);
}

// Finding 9: an inbox the seat turned into a symlink is never read or
// emptied. trace:BUG-2383 | ai:claude
#[cfg(unix)]
#[test]
fn an_inbox_symlink_is_never_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let lane = fixture_lane(tmp.path());
    let elsewhere = tmp.path().join("host-dir");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let planted = elsewhere.join("j1.job");
    std::fs::write(
        &planted,
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
    std::fs::remove_dir_all(&lane.inbox).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &lane.inbox).unwrap();
    let runner = Runner::start(lane.clone());
    std::thread::sleep(Duration::from_secs(2));
    runner.stop();
    assert!(
        planted.exists(),
        "the runner removed a file outside the inbox"
    );
    assert_eq!(
        read_rc(&lane.results, "j1"),
        None,
        "it ran a job from outside"
    );
    let log = std::fs::read_to_string(lane.results.join("jobs.log")).unwrap_or_default();
    assert!(log.contains("cannot open the inbox"), "{log}");
    // And a fresh prepare refuses the symlinked inbox.
    assert!(lane.prepare().is_err());
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

// Finding 8 / round-3 ruling: the drain's phase-1 launcher leaves the
// untested marker for the orchestrator, and clears a stale one when the
// lane did run a test. trace:BUG-2383 | ai:claude
#[test]
fn a_drain_lane_records_whether_it_ran_a_test() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path();
    assert!(lane_was_untested(main, "lease-d"));
    record_untested(main, "lease-d", true);
    assert!(lane_was_untested(main, "lease-d"));
    assert!(lane_was_untested(main, "lease-other"));
    record_untested(main, "lease-d", false);
    assert!(!lane_was_untested(main, "lease-d"));
}

// trace:BUG-2383 | ai:codex
#[cfg(unix)]
#[test]
fn pre_test_failures_and_zero_matches_stay_untested() {
    for (name, output, rc) in [
        ("manifest", "error: failed to parse manifest", 101),
        ("offline", "error: no matching package found (offline)", 101),
        ("compile", "error: could not compile crate", 101),
        ("start", "error: could not execute test process", 101),
        ("argv", "error: unexpected argument", 1),
        (
            "zero",
            "running 0 tests\ntest result: ok. 0 passed; 0 failed; 9 filtered out",
            0,
        ),
        (
            "ignored",
            "test result: ok. 0 passed; 0 failed; 1 ignored",
            0,
        ),
        ("list", "a_test: test\n1 test, 0 benchmarks", 0),
        ("no_run", "Finished test profile\nExecutable unittests", 0),
        ("started_only", "running 1 test", 101),
        (
            "failed_test",
            "test a_test ... FAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored",
            101,
        ),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let mut lane = fixture_lane(tmp.path());
        let cargo = tmp.path().join("cargo-result");
        crate::test_exec::write_executable(
            &cargo,
            format!("#!/bin/sh\ncat <<'OUTPUT'\n{output}\nOUTPUT\nexit {rc}\n"),
        );
        lane.program = cargo.display().to_string();
        let runner = Runner::start(lane.clone());
        let (actual, _) = run_through(&lane, &job(&["cargo", "test", "--profile", "agent"]));
        let tally = runner.stop();
        assert_eq!(actual, rc, "{name}");
        assert_eq!(tally.untested(), name != "failed_test", "{name}");
    }
    let tally = Tally::default();
    tally.record_exit(
        &job(&["cargo", "test", "--profile", "agent"]),
        std::io::empty(),
    );
    assert!(tally.untested());
    struct Unreadable;
    impl std::io::Read for Unreadable {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "fixture",
            ))
        }
    }
    tally.record_exit(&job(&["cargo", "test", "--profile", "agent"]), Unreadable);
    assert!(
        tally.untested(),
        "unreadable output supplies no test evidence"
    );
}

// Exercise the production drain launch boundary with a real child submitting a
// job through the env it receives; the vendor-specific resume loop stays separate.
// trace:BUG-2383 | ai:codex
#[cfg(unix)]
#[test]
fn drain_launcher_supplies_jobs_for_all_implementer_vendors_and_reports_evidence() {
    use crate::session::{
        drain_resume_lane, implementer_job_lane, with_implementer_jobs, HeadlessVendor,
    };
    for vendor in [
        HeadlessVendor::Claude,
        HeadlessVendor::Codex,
        HeadlessVendor::Agy,
    ] {
        for refused in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let lane = fixture_lane(tmp.path());
            let main = tmp.path().join("main");
            let lease = "lease-fixture";
            let eligible =
                implementer_job_lane("implementer", Some(lease), Some((&lane.seat, "branch")));
            assert!(eligible.is_some());
            assert_eq!(
                drain_resume_lane(
                    vendor,
                    "implementer",
                    Some(lease),
                    Some((&lane.seat, "branch"))
                )
                .is_some(),
                vendor == HeadlessVendor::Claude
            );
            let request = if refused {
                job(&["sh", "-c", "false"])
            } else {
                job(&["cargo", "test", "--profile", "agent", "--", "passing"])
            };
            let request_file = tmp.path().join("request");
            std::fs::write(&request_file, serde_json::to_vec(&request).unwrap()).unwrap();
            with_implementer_jobs(
                &main,
                "implementer",
                Some(lease),
                Some((&lane.seat, "branch")),
                "implement this",
                |root, tree, offered_lease| {
                    assert_eq!(root, main);
                    assert_eq!(tree, lane.seat);
                    assert_eq!(offered_lease, lease);
                    Some(lane.clone())
                },
                |prompt, apply| {
                    assert!(prompt.contains("aida lane job run -- cargo test --profile agent"));
                    let mut cmd = std::process::Command::new("/bin/sh");
                    cmd.arg("-c").arg("test -d \"$AIDA_LANE_JOBS_RESULTS\" && cp \"$1\" \"$AIDA_LANE_JOBS_INBOX/drain.tmp\" && mv \"$AIDA_LANE_JOBS_INBOX/drain.tmp\" \"$AIDA_LANE_JOBS_INBOX/drain.job\"")
                    .arg("fixture").arg(&request_file);
                    apply(&mut cmd);
                    assert!(cmd.status().unwrap().success());
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while read_rc(&lane.results, "drain").is_none() {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    assert_eq!(
                        read_rc(&lane.results, "drain"),
                        Some(if refused { REFUSED_RC } else { 0 })
                    );
                },
            );
            let warning = phase_test_warning(&main, Some(lease));
            assert_eq!(warning.is_some(), refused);
            if let Some(line) = warning {
                assert!(line.contains("untested=true"));
            }
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    with_implementer_jobs(
        tmp.path(),
        "implementer",
        Some("no-runner"),
        Some((tmp.path(), "branch")),
        "prompt",
        |_, _, _| None,
        |prompt, _| {
            assert!(prompt.contains("untested=true"));
        },
    );
    assert!(phase_test_warning(tmp.path(), Some("no-runner")).is_some());
    assert!(phase_test_warning(tmp.path(), Some("missing")).is_some());
    assert!(phase_test_warning(tmp.path(), None).is_some());
    let marker = results_dir(tmp.path(), "unreadable").join(UNTESTED_MARKER);
    std::fs::create_dir_all(&marker).unwrap();
    assert!(phase_test_warning(tmp.path(), Some("unreadable")).is_some());
    assert!(implementer_job_lane("reviewer", Some("x"), Some((tmp.path(), "b"))).is_none());
    assert!(implementer_job_lane("implementer", None, Some((tmp.path(), "b"))).is_none());
}
