//! STORY-1917 Slice 2 tests: dispatch executor core (`tick` / `run`),
//! `aida dispatch serve`, and the `review`, `take`, and `impl` runners.
//!
//! Covers:
//! 1. Boundary B hard condition: `aida dispatch tick` executes ONLY `take` and
//!    `impl` (leaving `review`, `flow`, and `train` `Pending` as `waiting for
//!    serve`), and `execute_one_request(..., via_serve = false)` refuses
//!    `review`/`flow`/`train` before any spawn.
//! 2. `aida dispatch serve` refuses to start without a bound `advisor` grant
//!    (absent, unbound/foreign presenter, wrong seat, expired, or revoked),
//!    executes `review` when bound, and stops leaving unclaimed requests
//!    `Pending` if the grant expires mid-run.
//! 3. Guard test: no code path outside `role enter` / `--bind-pid` writes to
//!    `grant.binding` or calls `bind_grant_to_pid`, and `DispatchGrantBindGuard`
//!    does not exist.
//! 4. Tampered request JSON or frozen brief file on disk (SHA-256 mismatch or
//!    symlink swap) is refused by the executor before any spawn.
//! 5. Mandate stopped/expired, `HumanGates` enabled (`GateOp::Dispatch` /
//!    `GateOp::Ship`), or advisor grant revoked between request and execution
//!    transitions the request to `Refused`, emits a `MandateDriveAction` row,
//!    and mails `advisor`.
//! 6. Dead `Running` workers are reaped to `Failed` and concurrency caps
//!    (`MAX_CONCURRENT_REVIEWS = 2`) hold excess requests `Pending`.
//! 7. `cleanup_dead_reviewer_worktree` preserves `.aida/merge-holds/PR-<N>` in
//!    the main worktree (no-clobber) before removing a dead reviewer worktree,
//!    and refuses when a live session lease occupies the worktree or branch.
//! 8. `clean_stale_git_config_lock` removes only a 0-byte regular file with
//!    mode `0o444` and age `>= 120s`, leaving non-empty, writable, fresh, or
//!    symlinked `.git/config.lock` untouched.
// trace:STORY-1917 | ai:agy

use super::*;
use crate::mandate::{self, Mandate, Scope};
use chrono::Duration;
use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_spec(store_root: &Path, id: &str, req: &aida_core::Requirement) {
    let rel = aida_core::object_store::relative_object_path(id).unwrap();
    let path = store_root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_yaml::to_string(req).unwrap()).unwrap();
}

fn plain_req(id: &str) -> aida_core::Requirement {
    let mut req = aida_core::Requirement::new(format!("Title for {id}"), "body".into());
    req.spec_id = Some(id.to_string());
    req.req_type = aida_core::RequirementType::Task;
    req.status = aida_core::RequirementStatus::Approved;
    req.execution_mode = Some(aida_core::ExecutionMode::Drive);
    req
}

struct ExecFixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: PathBuf,
    mandate_dir: PathBuf,
    principal: String,
    _ambient: crate::test_env::AmbientGuard,
}

impl ExecFixture {
    fn new(delegable: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(
            root.join(".gitignore"),
            ".aida/*\n!.aida/config.toml\n.aida-store/\n.aida-store\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        git(&root, &["add", ".gitignore", ".aida/config.toml"]);
        git(&root, &["commit", "-qm", "init repo"]);
        let head = crate::test_git::git_command()
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let sha = String::from_utf8(head.stdout).unwrap().trim().to_string();
        git(&root, &["update-ref", "refs/remotes/origin/main", &sha]);
        git(
            &root,
            &["update-ref", "refs/remotes/origin/claude/fix-100", &sha],
        );

        let store = root.join(".aida-store");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        write_spec(&store, "TASK-100", &plain_req("TASK-100"));
        write_spec(&store, "STORY-1917", &plain_req("STORY-1917"));

        let ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&root, "advisor", delegable);
        let principal = crate::current_user_id(None);
        let mandate_dir = tmp.path().join("mandates");
        std::fs::create_dir_all(&mandate_dir).unwrap();

        Self {
            _tmp: tmp,
            root,
            store,
            mandate_dir,
            principal,
            _ambient: ambient,
        }
    }

    fn start_mandate(&self, scopes: &[Scope], include_supervised: bool) -> Mandate {
        let set: BTreeSet<Scope> = scopes.iter().copied().collect();
        mandate::start_with_scopes(
            &self.mandate_dir,
            &|| Ok(()),
            &self.principal,
            Duration::hours(4),
            Some("test dispatch".into()),
            false,
            include_supervised,
            set,
            Utc::now(),
        )
        .unwrap()
    }

    fn write_brief(&self, name: &str, content: &str) -> PathBuf {
        let p = self._tmp.path().join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    fn write_executable_script(&self, name: &str, body: &str) -> PathBuf {
        let p = self._tmp.path().join(name);
        crate::test_exec::write_executable(&p, body);
        p
    }
}

struct TestBinGuard;

impl Drop for TestBinGuard {
    fn drop(&mut self) {
        TEST_AIDA_BIN.with(|c| *c.borrow_mut() = None);
        TEST_AGENT_BIN.with(|c| *c.borrow_mut() = None);
    }
}

fn install_test_bins(aida_bin: Option<PathBuf>, agent_bin: Option<PathBuf>) -> TestBinGuard {
    TEST_AIDA_BIN.with(|c| *c.borrow_mut() = aida_bin);
    TEST_AGENT_BIN.with(|c| *c.borrow_mut() = agent_bin);
    TestBinGuard
}

fn count_advisor_mail(root: &Path) -> usize {
    let inbox = crate::mailbox_store::mailbox_dir(root);
    std::fs::read_dir(inbox)
        .map(|it| {
            it.flatten()
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
                .count()
        })
        .unwrap_or(0)
}

// trace:STORY-1917 | ai:agy
#[test]
fn tick_executes_only_take_and_impl_and_refuses_review_flow_train() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    let env_dump = f._tmp.path().join("agent_env.log");
    let fake_agent = f.write_executable_script(
        "fake-claude.sh",
        &format!(
            "#!/bin/sh\n\
             printf 'GRANT=%s\\nPURPOSE=%s\\nROLE=%s\\nSCOPE=%s\\nPWD=%s\\nARGS=%s\\n' \
             \"$AIDA_SESSION_GRANT\" \"$AIDA_SESSION_PURPOSE\" \"$AIDA_SESSION_ROLE\" \
             \"$AIDA_SESSION_SCOPE\" \"$(pwd)\" \"$*\" >> {}\n\
             echo 'claude logs deadbeef1234'\n",
            env_dump.display()
        ),
    );
    let _bins = install_test_bins(None, Some(fake_agent));

    let brief_take = f.write_brief("take.md", "Fix failing tests on PR 100.\n");
    let brief_impl = f.write_brief("impl.md", "Implement slice 2.\n");

    let rev = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 101,
            vendor: Some(DispatchVendor::Claude),
        },
        now,
    )
    .unwrap();
    let flow = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 102,
            no_ship: false,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let train = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Train { prs: vec![103] },
        now,
    )
    .unwrap();
    let take = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Take {
            pr: 100,
            branch: "claude/fix-100".into(),
            spec: "TASK-100".into(),
            brief: brief_take,
            vendor: Some(DispatchVendor::Claude),
        },
        now,
    )
    .unwrap();
    let imp = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "impl-1917".into(),
            branch: "claude/story-1917-s2".into(),
            spec: "STORY-1917".into(),
            brief: brief_impl,
            vendor: Some(DispatchVendor::Claude),
        },
        now,
    )
    .unwrap();

    // 1. `run_dispatch_tick_sync` claims and executes ONLY `take` and `impl`,
    //    leaving `review`, `flow`, and `train` Pending (`waiting_for_serve == 3`).
    let summary = run_dispatch_tick_sync(&f.root, &f.mandate_dir, now).unwrap();
    assert_eq!(summary.spawned, 2);
    assert_eq!(summary.waiting_for_serve, 3);

    let (_, take_res) = load_pair(&f.root, &take.request.id).unwrap();
    assert_eq!(take_res.status, DispatchStatus::Done, "{take_res:?}");
    assert_eq!(take_res.spawned_session.as_deref(), Some("deadbeef1234"));
    assert!(take_res.notified_at.is_some());

    let (_, impl_res) = load_pair(&f.root, &imp.request.id).unwrap();
    assert_eq!(impl_res.status, DispatchStatus::Done, "{impl_res:?}");
    assert_eq!(impl_res.spawned_session.as_deref(), Some("deadbeef1234"));
    assert!(impl_res.notified_at.is_some());

    // Verify both worktrees were created and `.aida` was linked.
    let wt_root = worktrees_root(&f.root);
    assert!(wt_root.join("take-100").is_dir());
    assert!(wt_root.join("impl-1917").is_dir());

    // Verify the spawned implementer received scrubbed GRANT="" and PURPOSE=""
    // and ROLE="implementer".
    let env_text = std::fs::read_to_string(&env_dump).unwrap();
    assert!(
        env_text.contains("GRANT=\nPURPOSE=\nROLE=implementer\nSCOPE=TASK-100"),
        "implementer env must have empty grant/purpose and role=implementer, got:\n{env_text}"
    );
    assert!(
        env_text.contains("GRANT=\nPURPOSE=\nROLE=implementer\nSCOPE=STORY-1917"),
        "impl env must have empty grant/purpose and role=implementer, got:\n{env_text}"
    );

    // Verify `review`, `flow`, and `train` are still Pending ("pending (waiting for serve)").
    for id in [&rev.request.id, &flow.request.id, &train.request.id] {
        let (rq, rs) = load_pair(&f.root, id).unwrap();
        assert_eq!(rs.status, DispatchStatus::Pending);
        assert_eq!(display_status(&rq, &rs), "pending (waiting for serve)");
    }

    // 2. Direct `execute_one_request(..., via_serve = false)` on a `review`,
    //    `flow`, or `train` request refuses with `DispatchStatus::Refused`.
    let refused_rev =
        execute_one_request(&f.root, &f.mandate_dir, &rev.request.id, false, now).unwrap();
    assert_eq!(refused_rev.status, DispatchStatus::Refused);
    assert!(
        refused_rev
            .refusal_reason
            .as_deref()
            .unwrap_or("")
            .contains("can only run under `aida dispatch serve`"),
        "expected Boundary B serve refusal, got: {refused_rev:?}"
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn serve_refuses_without_bound_advisor_grant_and_executes_review_with_grant() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    let review_log = f._tmp.path().join("fake_aida.log");
    let fake_aida = f.write_executable_script(
        "fake-aida.sh",
        &format!(
            "#!/bin/sh\n\
             printf 'AIDA_ARGS=%s\\nGRANT=%s\\nPURPOSE=%s\\n' \"$*\" \"$AIDA_SESSION_GRANT\" \"$AIDA_SESSION_PURPOSE\" >> {}\n\
             echo 'verdict: PASS'\n",
            review_log.display()
        ),
    );
    let _bins = install_test_bins(Some(fake_aida), None);

    let rev1 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 201,
            vendor: Some(DispatchVendor::Claude),
        },
        now,
    )
    .unwrap();

    // 1. With a valid bound advisor grant, `run_dispatch_serve_once_sync` succeeds
    //    and runs `review`.
    let pass1 = run_dispatch_serve_once_sync(&f.root, &f.mandate_dir, now).unwrap();
    assert_eq!(pass1.spawned, 1);
    let (_, res1) = load_pair(&f.root, &rev1.request.id).unwrap();
    assert_eq!(res1.status, DispatchStatus::Done, "{res1:?}");
    let aida_invoked = std::fs::read_to_string(&review_log).unwrap();
    assert!(
        aida_invoked.contains("queue work PR-201 --role reviewer --no-human --vendor claude"),
        "expected queue work invocation, got:\n{aida_invoked}"
    );
    assert!(
        aida_invoked.contains(&format!("GRANT={}", rev1.request.requester_grant_id)),
        "serve review worker must inherit the advisor grant in its process tree, got:\n{aida_invoked}"
    );

    // Submit a second review request, then test that `serve` refuses when the
    // advisor grant is expired, unbound, or missing, leaving `rev2` Pending.
    let rev2 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 202,
            vendor: None,
        },
        now,
    )
    .unwrap();

    // 2. Grant expired mid-run -> `run_dispatch_serve_once_sync` errors and leaves `rev2` Pending.
    let after_expiry = now + Duration::hours(25);
    let exp_err = run_dispatch_serve_once_sync(&f.root, &f.mandate_dir, after_expiry).unwrap_err();
    assert!(
        exp_err.to_string().contains("is not valid: it expired"),
        "unexpected error: {exp_err:#}"
    );
    let (_, res2) = load_pair(&f.root, &rev2.request.id).unwrap();
    assert_eq!(
        res2.status,
        DispatchStatus::Pending,
        "expired grant must leave unclaimed request Pending"
    );

    // 3. Grant bound to a foreign dead PID -> `check_serve_grant` refuses.
    let grant_id = std::env::var(crate::gate::seat_authority::GRANT_ENV).unwrap();
    let mut grant_rec = crate::gate::seat_authority::read_grant(&grant_id).unwrap();
    grant_rec.binding = vec!["proc:999999@linux-starttime:1".to_string()];
    let grant_file = crate::gate::seat_authority::grant_dir()
        .unwrap()
        .join(format!("{grant_id}.json"));
    write_private_atomic(&grant_file, &serde_json::to_vec_pretty(&grant_rec).unwrap()).unwrap();
    assert!(check_serve_grant(&f.root, now).is_err());

    // 4. Missing grant env -> `check_serve_grant` refuses.
    std::env::remove_var(crate::gate::seat_authority::GRANT_ENV);
    let missing_err = check_serve_grant(&f.root, now).unwrap_err();
    assert!(
        missing_err.to_string().contains("no session grant is set"),
        "unexpected error: {missing_err:#}"
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn no_code_path_outside_role_enter_writes_grant_binding_or_calls_bind_grant_to_pid() {
    // 1. Source-level guard: verify `dispatch.rs` (and every file outside
    //    `gate/seat_authority.rs` and `role_lifecycle.rs`) never calls
    //    `bind_grant_to_pid`, never defines `DispatchGrantBindGuard`, and never
    //    mutates `.binding` on a grant.
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let dispatch_src = std::fs::read_to_string(src_dir.join("dispatch.rs")).unwrap();
    assert!(
        !dispatch_src.contains("DispatchGrantBindGuard"),
        "DispatchGrantBindGuard is forbidden by the STORY-1917 advisor ruling"
    );
    assert!(
        !dispatch_src.contains("bind_grant_to_pid"),
        "dispatch.rs must never call bind_grant_to_pid"
    );

    fn visit_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                visit_rs_files(&p, out);
            } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(p);
            }
        }
    }

    let mut rs_files = Vec::new();
    visit_rs_files(&src_dir, &mut rs_files);
    for path in &rs_files {
        let rel = path.strip_prefix(&src_dir).unwrap_or(path);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.starts_with("tests/")
            || rel_str.starts_with("gate/tests/")
            || rel_str == "gate/seat_authority.rs"
            || rel_str == "role_lifecycle.rs"
        {
            continue;
        }
        let content = std::fs::read_to_string(path).unwrap();
        assert!(
            !content.contains("bind_grant_to_pid("),
            "only role_lifecycle.rs / gate/seat_authority.rs may call bind_grant_to_pid; found in {rel_str}"
        );
        assert!(
            !content.contains("DispatchGrantBindGuard"),
            "DispatchGrantBindGuard must not exist in {rel_str}"
        );
    }

    // 2. Runtime check: the advisor grant's `binding` vector is untouched across
    //    both `run_dispatch_tick_sync` and `run_dispatch_serve_once_sync`.
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();
    let fake_bin = f.write_executable_script("noop.sh", "#!/bin/sh\necho 'claude logs abcd1234'\n");
    let _bins = install_test_bins(Some(fake_bin.clone()), Some(fake_bin));

    let grant_id = std::env::var(crate::gate::seat_authority::GRANT_ENV).unwrap();
    let before_binding = crate::gate::seat_authority::read_grant(&grant_id)
        .unwrap()
        .binding;

    let brief = f.write_brief("b.md", "Impl brief.\n");
    submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "lane-bind-check".into(),
            branch: "claude/bind-check".into(),
            spec: "TASK-100".into(),
            brief,
            vendor: None,
        },
        now,
    )
    .unwrap();
    submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 303,
            vendor: None,
        },
        now,
    )
    .unwrap();

    run_dispatch_tick_sync(&f.root, &f.mandate_dir, now).unwrap();
    run_dispatch_serve_once_sync(&f.root, &f.mandate_dir, now).unwrap();

    let after_binding = crate::gate::seat_authority::read_grant(&grant_id)
        .unwrap()
        .binding;
    assert_eq!(
        before_binding, after_binding,
        "executor must never modify grant.binding"
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn tampered_request_json_or_frozen_brief_is_refused_before_spawn() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    let marker = f._tmp.path().join("spawned_marker");
    let fake_agent = f.write_executable_script(
        "should-not-run.sh",
        &format!("#!/bin/sh\ntouch {}\nexit 0\n", marker.display()),
    );
    let _bins = install_test_bins(Some(fake_agent.clone()), Some(fake_agent));

    // 1. Tampered brief content (SHA-256 mismatch) -> Refused before spawn.
    let brief1 = f.write_brief("b1.md", "Original brief content.\n");
    let sub1 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "tamper-sha".into(),
            branch: "claude/tamper-sha".into(),
            spec: "TASK-100".into(),
            brief: brief1,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let frozen1 = f
        .root
        .join(".aida/dispatch")
        .join(match &sub1.request.action {
            DispatchVerb::Impl { brief_relpath, .. } => brief_relpath,
            _ => unreachable!(),
        });
    write_private_atomic(&frozen1, b"Modified brief after freeze!\n").unwrap();

    let res1 = execute_one_request(&f.root, &f.mandate_dir, &sub1.request.id, false, now).unwrap();
    assert_eq!(res1.status, DispatchStatus::Refused);
    assert!(
        res1.refusal_reason
            .as_deref()
            .unwrap_or("")
            .contains("SHA-256 mismatch"),
        "expected SHA-256 mismatch refusal, got: {res1:?}"
    );
    assert!(
        !marker.exists(),
        "launcher must not be spawned on tampered brief"
    );

    // 2. Frozen brief swapped with a symlink -> Refused before spawn.
    let brief2 = f.write_brief("b2.md", "Brief before symlink swap.\n");
    let sub2 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "tamper-symlink".into(),
            branch: "claude/tamper-symlink".into(),
            spec: "TASK-100".into(),
            brief: brief2.clone(),
            vendor: None,
        },
        now,
    )
    .unwrap();
    let frozen2 = f
        .root
        .join(".aida/dispatch/briefs")
        .join(format!("{}.md", sub2.request.id));
    std::fs::remove_file(&frozen2).unwrap();
    std::os::unix::fs::symlink(&brief2, &frozen2).unwrap();

    let res2 = execute_one_request(&f.root, &f.mandate_dir, &sub2.request.id, false, now).unwrap();
    assert_eq!(res2.status, DispatchStatus::Refused);
    assert!(
        res2.refusal_reason
            .as_deref()
            .unwrap_or("")
            .contains("symlink"),
        "expected symlink refusal, got: {res2:?}"
    );
    assert!(
        !marker.exists(),
        "launcher must not be spawned on symlinked brief"
    );

    // 3. Tampered request JSON on disk (invalid branch "../escape") -> Refused before spawn.
    let brief3 = f.write_brief("b3.md", "Brief for JSON tamper test.\n");
    let sub3 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "tamper-json".into(),
            branch: "claude/tamper-json".into(),
            spec: "TASK-100".into(),
            brief: brief3,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let mut bad_req = sub3.request.clone();
    if let DispatchVerb::Impl { ref mut name, .. } = bad_req.action {
        *name = "../escape".into();
    }
    write_private_atomic(
        &request_path(&f.root, &sub3.request.id),
        &serde_json::to_vec_pretty(&bad_req).unwrap(),
    )
    .unwrap();

    let res3 = execute_one_request(&f.root, &f.mandate_dir, &sub3.request.id, false, now).unwrap();
    assert_eq!(res3.status, DispatchStatus::Refused);
    assert!(
        res3.refusal_reason
            .as_deref()
            .unwrap_or("")
            .contains("execution-time validation refused request"),
        "expected execution-time validation refusal, got: {res3:?}"
    );
    assert!(
        !marker.exists(),
        "launcher must not be spawned on tampered request JSON"
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn mandate_stopped_expired_human_gates_or_grant_revoked_at_execution_time_refuses_and_mails_advisor(
) {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    let m1 = f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();
    let brief = f.write_brief("b.md", "Impl brief.\n");

    // 1. Mandate stopped between request and execution -> Refused + audit row + advisor mail.
    let sub1 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "lane-stop".into(),
            branch: "claude/lane-stop".into(),
            spec: "TASK-100".into(),
            brief: brief.clone(),
            vendor: None,
        },
        now,
    )
    .unwrap();
    mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();

    let mail_before = count_advisor_mail(&f.root);
    let res1 = execute_one_request(&f.root, &f.mandate_dir, &sub1.request.id, false, now).unwrap();
    assert_eq!(res1.status, DispatchStatus::Refused);
    assert!(res1
        .refusal_reason
        .as_deref()
        .unwrap_or("")
        .contains("was stopped before execution"));
    assert!(res1.notified_at.is_some());
    assert_eq!(count_advisor_mail(&f.root), mail_before + 1);
    let rows1 = crate::mandate_drive::ledger_rows(&f.root, &m1.id);
    assert!(rows1.iter().any(|r| r.2.outcome == Outcome::Refused));

    // 2. Mandate expired before execution -> Refused + audit row + advisor mail.
    let m2 = f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let sub2 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "lane-exp".into(),
            branch: "claude/lane-exp".into(),
            spec: "TASK-100".into(),
            brief: brief.clone(),
            vendor: None,
        },
        now,
    )
    .unwrap();
    let after_mandate_expiry = now + Duration::hours(5);
    let res2 = execute_one_request(
        &f.root,
        &f.mandate_dir,
        &sub2.request.id,
        false,
        after_mandate_expiry,
    )
    .unwrap();
    assert_eq!(res2.status, DispatchStatus::Refused);
    assert!(
        res2.refusal_reason
            .as_deref()
            .unwrap_or("")
            .contains("expired"),
        "expected mandate expiry refusal, got: {res2:?}"
    );
    let rows2 = crate::mandate_drive::ledger_rows(&f.root, &m2.id);
    assert!(rows2.iter().any(|r| r.2.outcome == Outcome::Refused));
    mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();

    // 3. HumanGates gates `dispatch` between request and execution -> Refused with Outcome::Gated.
    let m3 = f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let sub3 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "lane-gated".into(),
            branch: "claude/lane-gated".into(),
            spec: "TASK-100".into(),
            brief: brief.clone(),
            vendor: None,
        },
        now,
    )
    .unwrap();
    std::fs::write(
        f.root.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n\n[mandate.human_gates]\nops = [\"dispatch\"]\n",
    )
    .unwrap();
    git(&f.root, &["add", ".aida/config.toml"]);
    git(&f.root, &["commit", "-qm", "gate dispatch"]);
    let head = crate::test_git::git_command()
        .arg("-C")
        .arg(&f.root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8(head.stdout).unwrap().trim().to_string();
    git(&f.root, &["update-ref", "refs/remotes/origin/main", &sha]);

    let res3 = execute_one_request(&f.root, &f.mandate_dir, &sub3.request.id, false, now).unwrap();
    assert_eq!(res3.status, DispatchStatus::Refused);
    assert!(res3
        .refusal_reason
        .as_deref()
        .unwrap_or("")
        .contains("dispatch is gated by human gates at execution time"));
    let rows3 = crate::mandate_drive::ledger_rows(&f.root, &m3.id);
    assert!(rows3.iter().any(|r| r.2.outcome == Outcome::Gated));

    // Restore un-gated config, then test advisor grant revocation before execution.
    std::fs::write(
        f.root.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    git(&f.root, &["add", ".aida/config.toml"]);
    git(&f.root, &["commit", "-qm", "ungate dispatch"]);
    let head = crate::test_git::git_command()
        .arg("-C")
        .arg(&f.root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8(head.stdout).unwrap().trim().to_string();
    git(&f.root, &["update-ref", "refs/remotes/origin/main", &sha]);

    // 4. Advisor grant revoked between request and execution -> Refused + advisor mail.
    let sub4 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "lane-revoked".into(),
            branch: "claude/lane-revoked".into(),
            spec: "TASK-100".into(),
            brief,
            vendor: None,
        },
        now,
    )
    .unwrap();
    crate::gate::seat_authority::revoke_by_id(&sub4.request.requester_grant_id).unwrap();

    let res4 = execute_one_request(&f.root, &f.mandate_dir, &sub4.request.id, false, now).unwrap();
    assert_eq!(res4.status, DispatchStatus::Refused);
    assert!(res4
        .refusal_reason
        .as_deref()
        .unwrap_or("")
        .contains("no longer valid"));
}

// trace:STORY-1917 | ai:agy
#[test]
fn reap_dead_workers_and_enforce_concurrency_caps() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    // 1. Submit a request and mark it Running under a dead PID -> next pass reaps it to Failed.
    let dead_sub = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 401,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let mut dead_res = dead_sub.result.clone();
    dead_res.status = DispatchStatus::Running;
    dead_res.started_at = Some(now);
    dead_res.worker_pid = Some(999_999_991);
    dead_res.worker_start = Some("dead-start".into());
    write_private_atomic(
        &result_path(&f.root, &dead_sub.request.id),
        &serde_json::to_vec_pretty(&dead_res).unwrap(),
    )
    .unwrap();

    let pass = claim_and_dispatch_pass(&f.root, true, false, now).unwrap();
    assert_eq!(pass.reaped, 1);
    let (_, after_reap) = load_pair(&f.root, &dead_sub.request.id).unwrap();
    assert_eq!(after_reap.status, DispatchStatus::Failed);
    assert!(after_reap.summary.contains("exited unexpectedly"));
    assert!(after_reap.notified_at.is_some());

    // 2. Concurrency cap test: with 2 live Running reviews (MAX_CONCURRENT_REVIEWS == 2),
    //    a 3rd Pending review is held Pending on the pass.
    let my_pid = std::process::id();
    let my_start = crate::process_probe::process_start_identity(my_pid);
    for pr in [402, 403] {
        let sub = submit_request(
            &f.root,
            &f.mandate_dir,
            &f.principal,
            &DispatchRequestVerb::Review { pr, vendor: None },
            now,
        )
        .unwrap();
        let mut r = sub.result;
        r.status = DispatchStatus::Running;
        r.started_at = Some(now);
        r.worker_pid = Some(my_pid);
        r.worker_start = my_start.clone();
        write_private_atomic(
            &result_path(&f.root, &sub.request.id),
            &serde_json::to_vec_pretty(&r).unwrap(),
        )
        .unwrap();
    }

    let third = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 404,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let capped_pass = claim_and_dispatch_pass(&f.root, true, false, now).unwrap();
    assert_eq!(
        capped_pass.claimed_ids.len(),
        0,
        "3rd review must wait while 2 reviews are already Running"
    );
    let (_, third_res) = load_pair(&f.root, &third.request.id).unwrap();
    assert_eq!(third_res.status, DispatchStatus::Pending);
}

// trace:STORY-1917 | ai:agy
#[test]
fn reviewer_worktree_cleanup_preserves_merge_hold_and_refuses_live_occupant() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    let wt_root = worktrees_root(&f.root);
    std::fs::create_dir_all(&wt_root).unwrap();

    // 1. Create a dead reviewer worktree `aida-pr-77` containing a merge-hold marker.
    let rev_wt = wt_root.join("aida-pr-77");
    git(
        &f.root,
        &[
            "worktree",
            "add",
            "-B",
            "pr-77",
            rev_wt.to_str().unwrap(),
            "HEAD",
        ],
    );
    let wt_hold_dir = rev_wt.join(".aida").join("merge-holds");
    std::fs::create_dir_all(&wt_hold_dir).unwrap();
    let hold_payload = r#"{"pr":77,"reason":"needs human review"}"#;
    std::fs::write(wt_hold_dir.join("PR-77"), hold_payload).unwrap();

    cleanup_dead_reviewer_worktree(&f.root, 77).unwrap();
    assert!(!rev_wt.exists(), "dead reviewer worktree must be removed");
    let main_hold = f.root.join(".aida/merge-holds/PR-77");
    assert_eq!(
        std::fs::read_to_string(&main_hold).unwrap(),
        hold_payload,
        "merge hold from dead reviewer worktree must be preserved in main worktree"
    );

    // 2. No-clobber check: if main worktree already has a hold marker, a different
    //    marker in the dead worktree must not overwrite it.
    git(
        &f.root,
        &[
            "worktree",
            "add",
            "-B",
            "pr-77",
            rev_wt.to_str().unwrap(),
            "HEAD",
        ],
    );
    let wt_hold_dir2 = rev_wt.join(".aida").join("merge-holds");
    std::fs::create_dir_all(&wt_hold_dir2).unwrap();
    std::fs::write(wt_hold_dir2.join("PR-77"), "clobber-attempt").unwrap();

    cleanup_dead_reviewer_worktree(&f.root, 77).unwrap();
    assert_eq!(
        std::fs::read_to_string(&main_hold).unwrap(),
        hold_payload,
        "existing main worktree merge hold must never be clobbered"
    );

    // 3. Live session lease occupying `PR-88` / `aida-pr-88` refuses cleanup.
    let live_wt = wt_root.join("aida-pr-88");
    std::fs::create_dir_all(&live_wt).unwrap();
    let my_pid = std::process::id();
    let my_start = crate::process_probe::process_start_identity(my_pid);
    let lease = crate::SessionLease {
        tree_kind: Default::default(),
        id: "lease-live-88".into(),
        scope: "PR-88".into(),
        slug: "pr-88".into(),
        owner: f.principal.clone(),
        worktree_path: live_wt.clone(),
        branch: "pr-88".into(),
        started_at: Utc::now(),
        hostname: "localhost".into(),
        role: Some("reviewer".into()),
        creator_pid: Some(my_pid),
        creator_pid_start_time: my_start.clone(),
        active_pid: Some(my_pid),
        active_pid_start_time: my_start,
        cargo_target_dir: None,
        parent_project_root: Some(f.root.clone()),
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: true,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: None,
        child_grant_id: None,
    };
    let lease_dir = crate::leases::leases_dir(&f.root);
    std::fs::create_dir_all(&lease_dir).unwrap();
    std::fs::write(
        lease_dir.join(format!("{}.toml", lease.id)),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();

    let err = cleanup_dead_reviewer_worktree(&f.root, 88).unwrap_err();
    assert!(
        err.to_string()
            .contains("currently occupied by live session lease"),
        "expected live lease refusal, got: {err:#}"
    );
    assert!(
        live_wt.exists(),
        "live reviewer worktree must not be removed"
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn clean_stale_git_config_lock_removes_only_empty_0444_stale_regular_file() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let lock_path = repo.join(".git").join("config.lock");
    let now = std::time::SystemTime::now();

    // 1. Absent lock -> Ok(false).
    assert!(!clean_stale_git_config_lock(&repo, now).unwrap());

    // 2. Non-empty 0444 lock even when old -> untouched.
    std::fs::write(&lock_path, b"[core]\n").unwrap();
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let future = now + std::time::Duration::from_secs(300);
    assert!(!clean_stale_git_config_lock(&repo, future).unwrap());
    assert!(lock_path.exists());
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::remove_file(&lock_path).unwrap();

    // 3. Empty writable (0644) lock even when old -> untouched.
    std::fs::write(&lock_path, b"").unwrap();
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!clean_stale_git_config_lock(&repo, future).unwrap());
    assert!(lock_path.exists());

    // 4. Empty 0444 lock that is < 120s old -> untouched.
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let too_soon = now + std::time::Duration::from_secs(30);
    assert!(!clean_stale_git_config_lock(&repo, too_soon).unwrap());
    assert!(lock_path.exists());

    // 5. Empty 0444 regular file >= 120s old -> removed!
    assert!(clean_stale_git_config_lock(&repo, future).unwrap());
    assert!(!lock_path.exists());

    // 6. Symlink at `.git/config.lock` -> untouched.
    let target = tmp.path().join("target_file");
    std::fs::write(&target, b"").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o444)).unwrap();
    std::os::unix::fs::symlink(&target, &lock_path).unwrap();
    assert!(!clean_stale_git_config_lock(&repo, future).unwrap());
    assert!(std::fs::symlink_metadata(&lock_path).is_ok());
}
