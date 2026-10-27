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
use crate::dispatch::*;
use crate::gate::dispatch_io::*;
use crate::gate::dispatch_ledger::{
    check_recovery_index_bounds, inspect_dispatch_inventory_readonly,
    MAX_DISPATCH_INVENTORY_ENTRIES, MAX_RECOVERY_FILE_BYTES,
};
use crate::gate::dispatch_lock::{
    acquire_dispatch_lock, activate_dispatch_v2_lock_quiescent, proc_mount_options,
    proc_view_refusal, verify_sole_open_fd_for_inode, TEST_FAIL_ROLLBACK_EXCHANGE,
    TEST_POST_EXCHANGE_HOOK, TEST_PRE_FLOCK_V2_HOOK, TEST_PROC_PID_ALLOWLIST, TEST_PROC_VIEW,
};
use crate::gate::mandate::{self, Mandate, Scope};
use chrono::Duration;
use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

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
    ambient: crate::test_env::AmbientGuard,
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
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
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
            ambient,
        }
    }

    fn start_mandate(&self, scopes: &[Scope], include_supervised: bool) -> Mandate {
        let set: BTreeSet<Scope> = scopes.iter().copied().collect();
        crate::gate::mandate::start_with_scopes(
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
    let mut f = ExecFixture::new(&["reviewer", "implementer"]);
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
    f.ambient.unset_key(crate::gate::seat_authority::GRANT_ENV);
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
        .contains("was stopped"));
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
    dead_res.worker_start = Some("linux-starttime:12345".into());
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

// trace:STORY-1917 | ai:agy
#[test]
fn f1_log_symlink_and_directory_symlink_refused_without_disclosing_or_modifying_outside_file() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    let outside_file = f._tmp.path().join("private-outside-secret.txt");
    let secret_payload = "SYNTHETIC-PRIVATE-CONTENT-DO-NOT-DISCLOSE\n";
    std::fs::write(&outside_file, secret_payload).unwrap();

    let sub = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 601,
            vendor: None,
        },
        now,
    )
    .unwrap();

    // Plant a symlink at `.aida/dispatch/logs/<id>.log` pointing to the outside private file.
    let logs = f.root.join(".aida/dispatch/logs");
    std::fs::create_dir_all(&logs).unwrap();
    let log_symlink = logs.join(format!("{}.log", sub.request.id));
    std::os::unix::fs::symlink(&outside_file, &log_symlink).unwrap();

    // 1. `append_dispatch_log` must refuse the symlink without reading or replacing it.
    let err = append_dispatch_log(&f.root, &sub.request.id, "refusal log line\n").unwrap_err();
    assert!(
        err.to_string().contains("symlink"),
        "expected symlink refusal from append_dispatch_log, got: {err:#}"
    );
    assert!(
        std::fs::symlink_metadata(&log_symlink)
            .unwrap()
            .file_type()
            .is_symlink(),
        "log symlink must not be replaced by a regular file"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_file).unwrap(),
        secret_payload,
        "outside file must remain completely untouched"
    );

    // 2. `spawn_detached_worker` must also refuse when the log file is a symlink.
    let spawn_err = spawn_detached_worker(&f.root, &sub.request.id, true, "tok-1").unwrap_err();
    assert!(
        spawn_err.to_string().contains("symlink"),
        "expected symlink refusal from spawn_detached_worker, got: {spawn_err:#}"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_file).unwrap(),
        secret_payload,
        "outside file must not be written by worker stdout/stderr redirect"
    );

    // 3. Even when `execute_one_request` takes a refusal path (e.g. revoked grant),
    //    no file under `.aida/dispatch/` may contain the outside secret content.
    crate::gate::seat_authority::revoke_by_id(&sub.request.requester_grant_id).unwrap();
    let res = execute_one_request(&f.root, &f.mandate_dir, &sub.request.id, true, now).unwrap();
    assert_eq!(res.status, DispatchStatus::Refused);
    assert!(
        std::fs::symlink_metadata(&log_symlink)
            .unwrap()
            .file_type()
            .is_symlink(),
        "log symlink must still not be replaced with a regular file containing outside bytes"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_file).unwrap(),
        secret_payload
    );

    // 4. Replacing `.aida/dispatch/logs` itself with a symlink to an outside directory must be refused.
    std::fs::remove_file(&log_symlink).unwrap();
    std::fs::remove_dir(&logs).unwrap();
    let outside_dir = f._tmp.path().join("outside-logs-dir");
    std::fs::create_dir_all(&outside_dir).unwrap();
    std::os::unix::fs::symlink(&outside_dir, &logs).unwrap();

    let dir_err = append_dispatch_log(&f.root, &sub.request.id, "attempt\n").unwrap_err();
    assert!(
        dir_err.to_string().contains("symlink"),
        "expected directory symlink refusal, got: {dir_err:#}"
    );
    assert!(
        !outside_dir.join(format!("{}.log", sub.request.id)).exists(),
        "must not create log inside symlinked directory"
    );
}

// Invoked by the synthetic `aida` wrapper inside an actual detached child
// process spawned by `claim_and_dispatch_pass(..., spawn_detached = true)` or
// `spawn_detached_worker`. When run directly by the test harness without
// `AIDA_TEST_1917_CHILD_ID`, returns immediately.
// trace:STORY-1917 | ai:agy
#[test]
fn story_1917_detached_worker_child_entry() {
    if let Ok(lock_path) = std::env::var("AIDA_TEST_1917_OPEN_LOCK_PATH") {
        let opened_marker =
            PathBuf::from(std::env::var("AIDA_TEST_1917_OPENED_MARKER").expect("OPENED_MARKER"));
        let release_marker =
            PathBuf::from(std::env::var("AIDA_TEST_1917_RELEASE_MARKER").expect("RELEASE_MARKER"));
        let _open_fd = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("open legacy lock RW before flock");
        if std::env::var("AIDA_TEST_1917_NON_DUMPABLE").ok().as_deref() == Some("1") {
            unsafe {
                libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
            }
        }
        std::fs::write(&opened_marker, std::process::id().to_string()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if release_marker.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("open-before-flock child timed out waiting for release marker");
    }
    let Some(req_id) = std::env::var("AIDA_TEST_1917_CHILD_ID").ok() else {
        return;
    };
    let via_serve = std::env::var("AIDA_TEST_1917_CHILD_VIA_SERVE")
        .ok()
        .as_deref()
        == Some("1");
    let root = PathBuf::from(
        std::env::var("AIDA_TEST_1917_MAIN_WORKTREE").expect("AIDA_TEST_1917_MAIN_WORKTREE"),
    );
    let mandate_dir = PathBuf::from(
        std::env::var("AIDA_TEST_1917_MANDATE_DIR").expect("AIDA_TEST_1917_MANDATE_DIR"),
    );
    let barrier_dir = PathBuf::from(
        std::env::var("AIDA_TEST_1917_BARRIER_DIR").expect("AIDA_TEST_1917_BARRIER_DIR"),
    );
    let synth_aida = PathBuf::from(
        std::env::var("AIDA_TEST_1917_SYNTHETIC_AIDA").expect("AIDA_TEST_1917_SYNTHETIC_AIDA"),
    );

    let _prev_ambient = crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
        project_root: root.clone(),
        stdin_is_terminal: false,
        stdout_is_terminal: false,
        stderr_is_terminal: false,
    }));
    TEST_AIDA_BIN.with(|c| *c.borrow_mut() = Some(synth_aida));

    // Requirement 1 proof: before `execute_one_request` runs, inspect the
    // on-disk result record to prove `Running` + `claim_token` are already
    // durable before child execution begins.
    let child_pid = std::process::id();
    let child_start = crate::process_probe::process_start_identity(child_pid);
    let env_token = std::env::var(DISPATCH_CLAIM_TOKEN_ENV).ok();
    if let Ok((_, on_disk_res)) = load_pair(&root, &req_id) {
        let obs = serde_json::json!({
            "child_pid": child_pid,
            "child_start": child_start,
            "env_token": env_token,
            "on_disk_status": on_disk_res.status.as_str(),
            "on_disk_claim_token": on_disk_res.claim_token,
        });
        let _ = std::fs::write(
            barrier_dir.join(format!("pre-exec-{req_id}.json")),
            serde_json::to_vec_pretty(&obs).unwrap(),
        );
    }

    if let Err(err) = execute_one_request(&root, &mandate_dir, &req_id, via_serve, Utc::now()) {
        let _ = std::fs::write(
            barrier_dir.join(format!("child-err-{req_id}.txt")),
            format!("{err:#}"),
        );
    }
}

struct DetachedWorkerWatchdog {
    barrier_dir: PathBuf,
    pids: Vec<(u32, Option<String>)>,
}

impl DetachedWorkerWatchdog {
    fn new(barrier_dir: PathBuf) -> Self {
        Self {
            barrier_dir,
            pids: Vec::new(),
        }
    }

    fn track(&mut self, pid: u32, start: Option<String>) {
        self.pids.push((pid, start));
    }
}

impl Drop for DetachedWorkerWatchdog {
    fn drop(&mut self) {
        let _ = std::fs::write(self.barrier_dir.join("release-all"), b"1");
        for (pid, start) in &self.pids {
            if *pid > 1 && crate::process_probe::process_identity_is_alive(*pid, start.as_deref()) {
                unsafe {
                    libc::kill(*pid as i32, libc::SIGKILL);
                }
            }
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        for (pid, start) in &self.pids {
            while *pid > 1
                && crate::process_probe::process_identity_is_alive(*pid, start.as_deref())
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

fn wait_for_barrier_file(path: &Path, timeout: std::time::Duration) -> String {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(path) {
            if !text.trim().is_empty() {
                return text.trim().to_string();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    panic!(
        "watchdog timeout waiting for barrier file {}",
        path.display()
    );
}

fn wait_for_request_status(
    root: &Path,
    id: &str,
    expected: DispatchStatus,
    timeout: std::time::Duration,
) -> DispatchResult {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let Ok((_, res)) = load_pair(root, id) {
            if res.status == expected {
                return res;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    let last = load_pair(root, id).ok().map(|(_, r)| r);
    panic!(
        "watchdog timeout waiting for request {id} to reach status {}, last={last:?}",
        expected.as_str()
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn f2_worker_claim_persisted_before_spawn_and_fenced_against_unowned_or_concurrent_run() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    let barrier_dir = f._tmp.path().join("f2-barriers");
    std::fs::create_dir_all(&barrier_dir).unwrap();
    let mut watchdog = DetachedWorkerWatchdog::new(barrier_dir.clone());

    let parent_home = crate::aida_home_dir().expect("hermetic test home");
    let test_bin = crate::aida_bin::current_executable_path().expect("current test binary");
    let fake_aida_path = f._tmp.path().join("fake-aida-f2.sh");
    let fake_aida = f.write_executable_script(
        "fake-aida-f2.sh",
        &format!(
            "#!/bin/sh\nset -eu\n\
             if [ \"${{1:-}}\" = \"dispatch\" ] && [ \"${{2:-}}\" = \"run\" ]; then\n\
               req_id=\"$3\"\n\
               via_serve=\"0\"\n\
               if [ \"${{4:-}}\" = \"--via-serve\" ]; then\n\
                 via_serve=\"1\"\n\
               fi\n\
               export AIDA_HOME='{}'\n\
               export AIDA_TEST_1917_CHILD_ID=\"$req_id\"\n\
               export AIDA_TEST_1917_CHILD_VIA_SERVE=\"$via_serve\"\n\
               export AIDA_TEST_1917_MAIN_WORKTREE='{}'\n\
               export AIDA_TEST_1917_MANDATE_DIR='{}'\n\
               export AIDA_TEST_1917_BARRIER_DIR='{}'\n\
               export AIDA_TEST_1917_SYNTHETIC_AIDA='{}'\n\
               exec '{}' --exact gate::dispatch_authority::story_1917_dispatch_exec_tests::story_1917_detached_worker_child_entry --nocapture --test-threads=1\n\
             fi\n\
             if [ \"${{1:-}}\" = \"queue\" ] && [ \"${{2:-}}\" = \"work\" ]; then\n\
               pr_tag=\"${{3:-PR-0}}\"\n\
               echo \"$$:$pr_tag\" >> '{}'\n\
               echo \"$$\" > '{}/entered-'\"$pr_tag\"\n\
               i=0\n\
               while [ \"$i\" -lt 500 ]; do\n\
                 if [ -f '{}/release-'\"$pr_tag\" ] || [ -f '{}/release-all' ]; then\n\
                   echo \"done\" > '{}/completed-'\"$pr_tag\"\n\
                   exit 0\n\
                 fi\n\
                 sleep 0.02\n\
                 i=$((i + 1))\n\
               done\n\
               echo \"watchdog timeout waiting for release-$pr_tag\" >&2\n\
               exit 124\n\
             fi\n\
             exit 1\n",
            parent_home.display(),
            f.root.display(),
            f.mandate_dir.display(),
            barrier_dir.display(),
            fake_aida_path.display(),
            test_bin.display(),
            barrier_dir.join("runner-invocations.log").display(),
            barrier_dir.display(),
            barrier_dir.display(),
            barrier_dir.display(),
            barrier_dir.display(),
        ),
    );
    let _bins = install_test_bins(Some(fake_aida), None);
    let watchdog_timeout = std::time::Duration::from_secs(10);

    // ── 1. Real detached worker handoff: Running claim/token is durable before
    //       the child can execute, and parent publishes (child_pid, child_start).
    let sub1 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 601,
            vendor: None,
        },
        now,
    )
    .unwrap();

    let pass1 = claim_and_dispatch_pass(&f.root, true, true, now).unwrap();
    assert_eq!(pass1.spawned, 1);
    assert_eq!(pass1.claimed_ids, vec![sub1.request.id.clone()]);

    // Wait for the real detached child to enter the synthetic review runner.
    let _runner_pid_str =
        wait_for_barrier_file(&barrier_dir.join("entered-PR-601"), watchdog_timeout);

    // Inspect the child's pre-execution observation written before `execute_one_request`.
    let pre_exec_bytes =
        std::fs::read(barrier_dir.join(format!("pre-exec-{}.json", sub1.request.id))).unwrap();
    let pre_exec: serde_json::Value = serde_json::from_slice(&pre_exec_bytes).unwrap();
    assert_eq!(
        pre_exec["on_disk_status"].as_str(),
        Some("running"),
        "Running status must be durable on disk before the detached child can execute"
    );
    let pre_tok = pre_exec["on_disk_claim_token"]
        .as_str()
        .expect("claim_token must be durable on disk before child execution");
    assert_eq!(
        Some(pre_tok),
        pre_exec["env_token"].as_str(),
        "detached child's inherited claim_token must match durable on-disk claim_token"
    );

    let (req1_live, res1_live) = load_pair(&f.root, &sub1.request.id).unwrap();
    assert_eq!(res1_live.status, DispatchStatus::Running);
    let child_pid = res1_live
        .worker_pid
        .expect("real detached worker_pid must be published on disk");
    let child_start = res1_live.worker_start.clone();
    let owner_token = res1_live
        .claim_token
        .clone()
        .expect("claim_token must be present on Running record");
    watchdog.track(child_pid, child_start.clone());
    assert_ne!(
        child_pid,
        std::process::id(),
        "detached worker must be a separate OS child process, not the current test PID"
    );
    assert!(
        crate::process_probe::process_identity_is_alive(child_pid, child_start.as_deref()),
        "detached worker child (pid {child_pid}) must be alive while paused at runner barrier"
    );

    // ── 2. Competing `run` while the real detached owner is alive cannot
    //       duplicate execution or overwrite its result (even with tampered JSON).
    let req1_file = request_path(&f.root, &sub1.request.id);
    let orig_req1_bytes = std::fs::read(&req1_file).unwrap();
    let mut tampered_val: serde_json::Value = serde_json::from_slice(&orig_req1_bytes).unwrap();
    tampered_val["action"]["pr"] = serde_json::json!(0);
    write_private_atomic(
        &req1_file,
        &serde_json::to_vec_pretty(&tampered_val).unwrap(),
    )
    .unwrap();

    // 2a. Competing run with no token -> refused before touching result.
    let no_tok_err =
        execute_one_request_with_token(&f.root, &f.mandate_dir, &sub1.request.id, true, None, now)
            .unwrap_err();
    assert!(
        no_tok_err.to_string().contains("claim token mismatch"),
        "expected claim token mismatch for competing run without token, got: {no_tok_err:#}"
    );
    // 2b. Competing run with wrong token -> refused before touching result.
    let wrong_tok_err = execute_one_request_with_token(
        &f.root,
        &f.mandate_dir,
        &sub1.request.id,
        true,
        Some("wrong-token"),
        now,
    )
    .unwrap_err();
    assert!(
        wrong_tok_err.to_string().contains("claim token mismatch"),
        "expected claim token mismatch for wrong token, got: {wrong_tok_err:#}"
    );
    // 2c. Competing run from a different process presenting the REAL `owner_token` ->
    //     refused because `(self_pid, self_start) != (child_pid, child_start)`!
    let stolen_tok_err = execute_one_request_with_token(
        &f.root,
        &f.mandate_dir,
        &sub1.request.id,
        true,
        Some(&owner_token),
        now,
    )
    .unwrap_err();
    assert!(
        stolen_tok_err
            .to_string()
            .contains(&format!("already running under worker pid {child_pid}")),
        "expected PID ownership refusal for competing run with stolen token, got: {stolen_tok_err:#}"
    );

    write_private_atomic(&req1_file, &orig_req1_bytes).unwrap();

    let (_, after_competing) = load_pair(&f.root, &sub1.request.id).unwrap();
    assert_eq!(
        after_competing.status,
        DispatchStatus::Running,
        "competing runs must never overwrite live owner's Running status"
    );
    assert_eq!(after_competing.worker_pid, Some(child_pid));
    assert_eq!(
        after_competing.claim_token.as_deref(),
        Some(owner_token.as_str())
    );
    let invocations_while_live =
        std::fs::read_to_string(barrier_dir.join("runner-invocations.log")).unwrap();
    assert_eq!(
        invocations_while_live
            .lines()
            .filter(|l| l.ends_with(":PR-601"))
            .count(),
        1,
        "competing runs must not duplicate runner execution"
    );

    // ── 3. PID/start/token mismatches and stale terminal writers cannot mutate
    //       the live owner's result or the finished result.
    let wrong_pid_claim = WorkerClaim {
        worker_pid: std::process::id(),
        worker_start: child_start.clone(),
        claim_token: owner_token.clone(),
    };
    assert!(
        finalize_request(
            &f.root,
            &req1_live,
            &wrong_pid_claim,
            res1_live.clone(),
            DispatchStatus::Failed,
            Outcome::Skipped,
            None,
            Some(99),
            "wrong pid finalize".into(),
            Some("wrong pid".into()),
            now,
        )
        .is_err(),
        "mismatched worker_pid must be refused by terminal fence"
    );

    let wrong_start_claim = WorkerClaim {
        worker_pid: child_pid,
        worker_start: Some("mismatched-start-time".into()),
        claim_token: owner_token.clone(),
    };
    assert!(
        finalize_request(
            &f.root,
            &req1_live,
            &wrong_start_claim,
            res1_live.clone(),
            DispatchStatus::Failed,
            Outcome::Skipped,
            None,
            Some(99),
            "wrong start finalize".into(),
            Some("wrong start".into()),
            now,
        )
        .is_err(),
        "mismatched worker_start must be refused by terminal fence"
    );

    let wrong_token_claim = WorkerClaim {
        worker_pid: child_pid,
        worker_start: child_start.clone(),
        claim_token: "wrong-claim-token".into(),
    };
    let mut scratch_res = res1_live.clone();
    assert!(
        record_tampered_refusal(
            &f.root,
            &sub1.request.id,
            &wrong_token_claim,
            &mut scratch_res,
            "wrong token tampered",
            now,
        )
        .is_err(),
        "mismatched claim_token must be refused by terminal fence"
    );

    // Release the real detached child so it finishes and records `Done`.
    std::fs::write(barrier_dir.join("release-PR-601"), b"1").unwrap();
    let done_res1 = wait_for_request_status(
        &f.root,
        &sub1.request.id,
        DispatchStatus::Done,
        watchdog_timeout,
    );
    assert_eq!(done_res1.worker_pid, Some(child_pid));
    assert_eq!(done_res1.exit_code, Some(0));

    // Once finished (`Done`), even a stale terminal writer presenting the exact
    // `(child_pid, child_start, owner_token)` from the completed run is refused!
    let stale_owner_claim = WorkerClaim {
        worker_pid: child_pid,
        worker_start: child_start.clone(),
        claim_token: owner_token.clone(),
    };
    let stale_fin_err = finalize_request(
        &f.root,
        &req1_live,
        &stale_owner_claim,
        done_res1.clone(),
        DispatchStatus::Failed,
        Outcome::Skipped,
        None,
        Some(1),
        "stale overwrite".into(),
        Some("stale overwrite".into()),
        now,
    )
    .unwrap_err();
    assert!(
        stale_fin_err
            .to_string()
            .contains("fenced result transition refused"),
        "stale terminal writer must be refused after completion, got: {stale_fin_err:#}"
    );
    let mut stale_tampered_res = done_res1.clone();
    assert!(
        record_tampered_refusal(
            &f.root,
            &sub1.request.id,
            &stale_owner_claim,
            &mut stale_tampered_res,
            "stale tampered overwrite",
            now,
        )
        .is_err(),
        "stale record_tampered_refusal must be refused after completion"
    );
    let late_run_err = execute_one_request_with_token(
        &f.root,
        &f.mandate_dir,
        &sub1.request.id,
        true,
        Some(&owner_token),
        now,
    )
    .unwrap_err();
    assert!(
        late_run_err
            .to_string()
            .contains("already in terminal state"),
        "late execute_one_request on finished request must fail, got: {late_run_err:#}"
    );
    let (_, after_stale) = load_pair(&f.root, &sub1.request.id).unwrap();
    assert_eq!(
        after_stale.status,
        DispatchStatus::Done,
        "finished Done result must remain immutable against stale writers"
    );
    assert_eq!(after_stale.exit_code, Some(0));

    // ── 4. Concurrent detached execution preserves concurrency caps (`MAX_CONCURRENT_REVIEWS == 2`).
    let rev_610 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 610,
            vendor: None,
        },
        now + Duration::seconds(1),
    )
    .unwrap();
    let rev_611 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 611,
            vendor: None,
        },
        now + Duration::seconds(2),
    )
    .unwrap();
    let rev_612 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 612,
            vendor: None,
        },
        now + Duration::seconds(3),
    )
    .unwrap();

    let cap_pass1 = claim_and_dispatch_pass(&f.root, true, true, now).unwrap();
    assert_eq!(
        cap_pass1.spawned, 2,
        "only 2 detached review workers may spawn concurrently"
    );
    assert_eq!(
        cap_pass1.claimed_ids,
        vec![rev_610.request.id.clone(), rev_611.request.id.clone()]
    );

    wait_for_barrier_file(&barrier_dir.join("entered-PR-610"), watchdog_timeout);
    wait_for_barrier_file(&barrier_dir.join("entered-PR-611"), watchdog_timeout);

    let (_, res_610_live) = load_pair(&f.root, &rev_610.request.id).unwrap();
    let (_, res_611_live) = load_pair(&f.root, &rev_611.request.id).unwrap();
    let pid_610 = res_610_live.worker_pid.unwrap();
    let pid_611 = res_611_live.worker_pid.unwrap();
    watchdog.track(pid_610, res_610_live.worker_start.clone());
    watchdog.track(pid_611, res_611_live.worker_start.clone());
    assert_ne!(
        pid_610, pid_611,
        "concurrent detached workers must be distinct child processes"
    );

    // While both real detached review children are alive at the barrier, a new
    // pass AND a direct `execute_one_request` on the 3rd review must both hold it Pending.
    let cap_pass_blocked = claim_and_dispatch_pass(&f.root, true, true, now).unwrap();
    assert_eq!(cap_pass_blocked.spawned, 0);
    let direct_cap_err =
        execute_one_request(&f.root, &f.mandate_dir, &rev_612.request.id, true, now).unwrap_err();
    assert!(
        direct_cap_err.to_string().contains("concurrency cap"),
        "direct run must enforce concurrency cap while 2 detached workers are live, got: {direct_cap_err:#}"
    );
    let (_, res_612_pending) = load_pair(&f.root, &rev_612.request.id).unwrap();
    assert_eq!(res_612_pending.status, DispatchStatus::Pending);

    // Release the 2 running detached workers, wait for both to finish `Done`,
    // then verify the 3rd review spawns and completes `Done`.
    std::fs::write(barrier_dir.join("release-PR-610"), b"1").unwrap();
    std::fs::write(barrier_dir.join("release-PR-611"), b"1").unwrap();
    wait_for_request_status(
        &f.root,
        &rev_610.request.id,
        DispatchStatus::Done,
        watchdog_timeout,
    );
    wait_for_request_status(
        &f.root,
        &rev_611.request.id,
        DispatchStatus::Done,
        watchdog_timeout,
    );

    let cap_pass2 = claim_and_dispatch_pass(&f.root, true, true, now).unwrap();
    assert_eq!(cap_pass2.spawned, 1);
    assert_eq!(cap_pass2.claimed_ids, vec![rev_612.request.id.clone()]);
    wait_for_barrier_file(&barrier_dir.join("entered-PR-612"), watchdog_timeout);
    let (_, res_612_live) = load_pair(&f.root, &rev_612.request.id).unwrap();
    watchdog.track(
        res_612_live.worker_pid.unwrap(),
        res_612_live.worker_start.clone(),
    );
    std::fs::write(barrier_dir.join("release-PR-612"), b"1").unwrap();
    wait_for_request_status(
        &f.root,
        &rev_612.request.id,
        DispatchStatus::Done,
        watchdog_timeout,
    );

    // ── 5. Parent publication failure cannot leave an unowned child that
    //       performs work or a replaceable live claim.
    // 5a. Post-spawn child-PID publication fails inside `claim_and_dispatch_pass`:
    //     parent kills + waits the spawned child under `tick.lock` and rolls back
    //     the record to `Failed` with `worker_pid = None, claim_token = None`.
    let sub_pub_fail = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 620,
            vendor: None,
        },
        now,
    )
    .unwrap();
    TEST_FAIL_POST_SPAWN_PUBLICATION.with(|c| c.set(true));
    let fail_pass = claim_and_dispatch_pass(&f.root, true, true, now);
    TEST_FAIL_POST_SPAWN_PUBLICATION.with(|c| c.set(false));
    let fail_pass = fail_pass.unwrap();
    assert_eq!(fail_pass.spawned, 0);
    assert!(fail_pass.claimed_ids.is_empty());

    let (_, pub_fail_res) = load_pair(&f.root, &sub_pub_fail.request.id).unwrap();
    assert_eq!(
        pub_fail_res.status,
        DispatchStatus::Failed,
        "post-spawn publication failure must transition claim to Failed, got: {pub_fail_res:?}"
    );
    assert!(
        pub_fail_res.worker_pid.is_none() && pub_fail_res.claim_token.is_none(),
        "failed publication must clear worker_pid and claim_token so no replaceable claim remains"
    );
    assert!(
        pub_fail_res
            .summary
            .contains("failed to publish dispatch worker claim"),
        "expected publication failure summary, got: {}",
        pub_fail_res.summary
    );
    assert!(
        !barrier_dir.join("entered-PR-620").exists(),
        "child killed on parent publication failure must never enter the runner"
    );

    // 5b. Simulated parent crash between pre-spawn claim write (`worker_pid = None`)
    //     and post-spawn child-PID publication: a real spawned child presenting
    //     `claim_token` must be refused as unowned (`worker_pid is None`), transition
    //     the incomplete claim to `Failed`, and never perform work or allow replacement.
    let sub_unowned = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 621,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let unowned_token = "unowned-pre-spawn-token-621";
    let mut pre_spawn_only = sub_unowned.result.clone();
    pre_spawn_only.status = DispatchStatus::Running;
    pre_spawn_only.started_at = Some(now);
    pre_spawn_only.worker_pid = None;
    pre_spawn_only.worker_start = None;
    pre_spawn_only.claim_token = Some(unowned_token.into());
    write_private_atomic(
        &result_path(&f.root, &sub_unowned.request.id),
        &serde_json::to_vec_pretty(&pre_spawn_only).unwrap(),
    )
    .unwrap();

    let (mut unowned_child, unowned_pid, unowned_start) =
        spawn_detached_worker(&f.root, &sub_unowned.request.id, true, unowned_token).unwrap();
    watchdog.track(unowned_pid, unowned_start);
    let _ = unowned_child.wait().unwrap();

    assert!(
        !barrier_dir.join("entered-PR-621").exists(),
        "unowned child whose worker_pid was never published by parent must never perform work"
    );
    let (_, after_unowned_child) = load_pair(&f.root, &sub_unowned.request.id).unwrap();
    assert_eq!(
        after_unowned_child.status,
        DispatchStatus::Failed,
        "incomplete Running claim without worker_pid must fail closed to Failed"
    );
    assert!(
        after_unowned_child.worker_pid.is_none() && after_unowned_child.claim_token.is_none(),
        "incomplete claim must clear worker_pid and claim_token so it cannot be replaced"
    );
    assert!(
        after_unowned_child
            .refusal_reason
            .as_deref()
            .unwrap_or("")
            .contains("worker_pid is None"),
        "expected incomplete worker_pid refusal reason, got: {after_unowned_child:?}"
    );
    let replace_attempt = execute_one_request_with_token(
        &f.root,
        &f.mandate_dir,
        &sub_unowned.request.id,
        true,
        Some(unowned_token),
        now,
    )
    .unwrap_err();
    assert!(
        replace_attempt
            .to_string()
            .contains("already in terminal state"),
        "failed unowned claim must not be replaceable by a later caller, got: {replace_attempt:#}"
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn f3_stopped_or_expired_mandate_a_followed_by_replacement_mandate_b_refuses_request_under_a() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    let now = Utc::now();
    let brief = f.write_brief("f3.md", "F3 brief.\n");

    // 1. Start Mandate A (without `--include-supervised`), queue request `sub_a`,
    //    stop Mandate A, then start replacement Mandate B WITH `--include-supervised`.
    let m_a = f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let sub_a = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "lane-f3-a".into(),
            branch: "claude/lane-f3-a".into(),
            spec: "TASK-100".into(),
            brief: brief.clone(),
            vendor: None,
        },
        now,
    )
    .unwrap();
    assert_eq!(sub_a.request.mandate_id, m_a.id);

    mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();
    let m_b = f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], true);
    assert_ne!(m_a.id, m_b.id);

    // Executing `sub_a` while replacement Mandate B is active must refuse citing Mandate A!
    let res_a =
        execute_one_request(&f.root, &f.mandate_dir, &sub_a.request.id, false, now).unwrap();
    assert_eq!(res_a.status, DispatchStatus::Refused);
    let reason_a = res_a.refusal_reason.as_deref().unwrap_or("");
    assert!(
        reason_a.contains(&m_a.id) && reason_a.contains("was stopped or superseded"),
        "expected refusal citing stopped/superseded mandate A ({}), got: {reason_a}",
        m_a.id
    );

    // 2. Also test when Mandate B expires and Mandate C is started later: a request queued
    //    under Mandate B must still be refused when executed under Mandate C.
    let sub_b = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "lane-f3-b".into(),
            branch: "claude/lane-f3-b".into(),
            spec: "TASK-100".into(),
            brief,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let after_b_expiry = now + Duration::hours(5);
    let m_c = mandate::start_with_scopes(
        &f.mandate_dir,
        &|| Ok(()),
        &f.principal,
        Duration::hours(4),
        Some("test dispatch c".into()),
        false,
        true,
        [Scope::ShipHeld, Scope::Dispatch].into_iter().collect(),
        after_b_expiry,
    )
    .unwrap();
    assert_ne!(m_b.id, m_c.id);

    let res_b = execute_one_request(
        &f.root,
        &f.mandate_dir,
        &sub_b.request.id,
        false,
        after_b_expiry,
    )
    .unwrap();
    assert_eq!(res_b.status, DispatchStatus::Refused);
    let reason_b = res_b.refusal_reason.as_deref().unwrap_or("");
    assert!(
        reason_b.contains(&m_b.id),
        "expected refusal citing superseded mandate B ({}), got: {reason_b}",
        m_b.id
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn f4_reviewer_hold_preservation_failure_aborts_cleanup_and_concurrent_destination_is_no_clobber() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    let wt_root = worktrees_root(&f.root);
    std::fs::create_dir_all(&wt_root).unwrap();

    // Create dead reviewer worktree `aida-pr-78` with a merge-hold marker.
    let rev_wt = wt_root.join("aida-pr-78");
    git(
        &f.root,
        &[
            "worktree",
            "add",
            "-B",
            "pr-78",
            rev_wt.to_str().unwrap(),
            "HEAD",
        ],
    );
    let wt_hold_dir = rev_wt.join(".aida/merge-holds");
    std::fs::create_dir_all(&wt_hold_dir).unwrap();
    let src_hold = wt_hold_dir.join("PR-78");
    let hold_payload = r#"{"pr":78,"reason":"hold-from-78"}"#;
    std::fs::write(&src_hold, hold_payload).unwrap();

    // 1a. Unreadable source hold (mode 0o000) -> preservation fails and aborts worktree removal.
    std::fs::set_permissions(&src_hold, std::fs::Permissions::from_mode(0o000)).unwrap();
    let err1a = cleanup_dead_reviewer_worktree(&f.root, 78).unwrap_err();
    std::fs::set_permissions(&src_hold, std::fs::Permissions::from_mode(0o600)).unwrap();
    let err1a_full = format!("{err1a:#}");
    assert!(
        err1a_full.contains("cannot open") || err1a_full.contains("Permission denied"),
        "expected unreadable source hold error, got: {err1a_full}"
    );
    assert!(
        rev_wt.exists() && src_hold.exists(),
        "reviewer worktree and source hold must remain intact when source hold is unreadable"
    );

    // 1b. Invalid/unwritable destination directory (`.aida/merge-holds` is a regular file) ->
    //     preservation fails and aborts worktree removal.
    let main_hold_dir = f.root.join(".aida/merge-holds");
    std::fs::write(&main_hold_dir, "blocking-file").unwrap();
    let err1b = cleanup_dead_reviewer_worktree(&f.root, 78).unwrap_err();
    std::fs::remove_file(&main_hold_dir).unwrap();
    std::fs::create_dir_all(&main_hold_dir).unwrap();
    let err1b_full = format!("{err1b:#}");
    assert!(
        err1b_full.contains("cannot create")
            || err1b_full.contains("cannot stage")
            || err1b_full.contains("File exists"),
        "expected hold destination error, got: {err1b_full}"
    );
    assert!(
        rev_wt.exists() && src_hold.exists(),
        "reviewer worktree and source hold must remain intact when destination staging fails"
    );

    // 2. Empty/corrupt source hold -> preservation refuses and aborts worktree removal.
    std::fs::write(&src_hold, b"").unwrap();
    let err2 = cleanup_dead_reviewer_worktree(&f.root, 78).unwrap_err();
    assert!(
        err2.to_string().contains("empty"),
        "expected empty hold refusal, got: {err2:#}"
    );
    assert!(
        rev_wt.exists(),
        "reviewer worktree must not be removed when source hold is empty/corrupt"
    );

    // 3. Symlink at destination `.aida/merge-holds/PR-78` -> no-clobber verification refuses
    //    because destination is a symlink, aborting worktree removal.
    std::fs::write(&src_hold, hold_payload).unwrap();
    let outside_target = f._tmp.path().join("outside-hold-target");
    std::fs::write(&outside_target, "outside").unwrap();
    let main_hold = main_hold_dir.join("PR-78");
    std::os::unix::fs::symlink(&outside_target, &main_hold).unwrap();

    let err3 = cleanup_dead_reviewer_worktree(&f.root, 78).unwrap_err();
    let err3_full = format!("{err3:#}");
    assert!(
        err3_full.contains("symlink"),
        "expected destination symlink refusal, got: {err3_full}"
    );
    assert!(
        rev_wt.exists(),
        "reviewer worktree must not be removed when destination hold is a symlink"
    );
    std::fs::remove_file(&main_hold).unwrap();

    // 4. Valid existing destination hold -> atomic no-clobber (`AlreadyExists`) verifies
    //    the existing destination hold and preserves it without overwriting.
    let winner_payload = r#"{"pr":78,"reason":"existing-winner-hold"}"#;
    std::fs::write(&main_hold, winner_payload).unwrap();
    cleanup_dead_reviewer_worktree(&f.root, 78).unwrap();
    assert!(
        !rev_wt.exists(),
        "dead reviewer worktree should be removed once hold is verified"
    );
    assert_eq!(
        std::fs::read_to_string(&main_hold).unwrap(),
        winner_payload,
        "existing destination hold must win without clobber"
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn f5_live_registered_agent_or_unknown_occupant_without_lease_blocks_cleanup_and_branch_reset() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();
    let wt_root = worktrees_root(&f.root);
    std::fs::create_dir_all(&wt_root).unwrap();

    // 1. Live registered agent in `.aida/agents/` with NO session lease in `.aida/sessions/`
    //    occupying `aida-pr-91` / `PR-91` blocks `cleanup_dead_reviewer_worktree`.
    let rev_wt = wt_root.join("aida-pr-91");
    git(
        &f.root,
        &[
            "worktree",
            "add",
            "-B",
            "pr-91",
            rev_wt.to_str().unwrap(),
            "HEAD",
        ],
    );
    let my_pid = std::process::id();
    let agents_dir = f.root.join(".aida/agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let rev_agent = crate::agent_registry::AgentRegistryEntry {
        id: "rev91".into(),
        agent_type: "codex".into(),
        pid: my_pid,
        pid_start: crate::process_probe::process_start_identity(my_pid),
        pid_namespace: crate::agent_registry::current_pid_namespace(),
        name: Some("rev91".into()),
        description: None,
        tty: None,
        terminal: None,
        started_at: now,
        last_active_at: now,
        role: Some("reviewer".into()),
        current_spec: Some("PR-91".into()),
        worktree_path: rev_wt.clone(),
        source: "test".into(),
        binary_version: None,
        build_sha: None,
        availability: Default::default(),
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        claude_session_id: None,
        ended_at: None,
        resumed_from: None,
        spec_status_at_end: None,
    };
    let rev_agent_file = agents_dir.join("rev91.toml");
    std::fs::write(&rev_agent_file, toml::to_string_pretty(&rev_agent).unwrap()).unwrap();

    let err_rev = cleanup_dead_reviewer_worktree(&f.root, 91).unwrap_err();
    assert!(
        err_rev
            .to_string()
            .contains("currently occupied by live registered agent"),
        "expected live registered agent refusal without lease, got: {err_rev:#}"
    );
    assert!(
        rev_wt.exists(),
        "occupied reviewer worktree must be retained"
    );
    std::fs::remove_file(&rev_agent_file).unwrap();

    // 2. Unobservable-namespace registered agent (`pid_namespace` mismatch, `ended_at: None`)
    //    is treated as an unknown occupant (fail closed) and also blocks cleanup.
    let foreign_ns_agent = crate::agent_registry::AgentRegistryEntry {
        id: "foreign91".into(),
        agent_type: "codex".into(),
        pid: 999_999_992,
        pid_start: Some("foreign-start".into()),
        pid_namespace: Some(1),
        name: Some("foreign91".into()),
        description: None,
        tty: None,
        terminal: None,
        started_at: now,
        last_active_at: now,
        role: Some("reviewer".into()),
        current_spec: Some("PR-91".into()),
        worktree_path: rev_wt.clone(),
        source: "test".into(),
        binary_version: None,
        build_sha: None,
        availability: Default::default(),
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        claude_session_id: None,
        ended_at: None,
        resumed_from: None,
        spec_status_at_end: None,
    };
    let foreign_file = agents_dir.join("foreign91.toml");
    std::fs::write(
        &foreign_file,
        toml::to_string_pretty(&foreign_ns_agent).unwrap(),
    )
    .unwrap();
    let err_foreign = cleanup_dead_reviewer_worktree(&f.root, 91).unwrap_err();
    assert!(
        err_foreign.to_string().contains("unknown-liveness")
            || err_foreign.to_string().contains("occupied"),
        "expected unobservable-namespace agent refusal, got: {err_foreign:#}"
    );
    assert!(rev_wt.exists());
    std::fs::remove_file(&foreign_file).unwrap();

    // 3. Live registered agent occupying `take-100` / branch `claude/fix-100` (with NO lease)
    //    blocks `take` runner BEFORE `git checkout -B` can reset the branch or touch the worktree.
    let take_wt = wt_root.join("take-100");
    git(
        &f.root,
        &[
            "worktree",
            "add",
            "-B",
            "claude/fix-100",
            take_wt.to_str().unwrap(),
            "HEAD",
        ],
    );
    let dirty_marker = take_wt.join("uncommitted-occupant-work.txt");
    std::fs::write(&dirty_marker, "do not lose my work").unwrap();

    let take_agent = crate::agent_registry::AgentRegistryEntry {
        id: "impl100".into(),
        agent_type: "claude".into(),
        pid: my_pid,
        pid_start: crate::process_probe::process_start_identity(my_pid),
        pid_namespace: crate::agent_registry::current_pid_namespace(),
        name: Some("impl100".into()),
        description: None,
        tty: None,
        terminal: None,
        started_at: now,
        last_active_at: now,
        role: Some("implementer".into()),
        current_spec: Some("TASK-100".into()),
        worktree_path: take_wt.clone(),
        source: "test".into(),
        binary_version: None,
        build_sha: None,
        availability: Default::default(),
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        claude_session_id: None,
        ended_at: None,
        resumed_from: None,
        spec_status_at_end: None,
    };
    let take_agent_file = agents_dir.join("impl100.toml");
    std::fs::write(
        &take_agent_file,
        toml::to_string_pretty(&take_agent).unwrap(),
    )
    .unwrap();

    let brief = f.write_brief("f5-take.md", "Take brief.\n");
    let sub_take = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Take {
            pr: 100,
            branch: "claude/fix-100".into(),
            spec: "TASK-100".into(),
            brief,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let res_take =
        execute_one_request(&f.root, &f.mandate_dir, &sub_take.request.id, false, now).unwrap();
    assert_eq!(res_take.status, DispatchStatus::Refused);
    assert!(
        res_take
            .refusal_reason
            .as_deref()
            .unwrap_or("")
            .contains("currently occupied by live registered agent"),
        "expected live registered agent refusal in take runner, got: {res_take:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&dirty_marker).unwrap(),
        "do not lose my work",
        "occupant worktree files must remain untouched"
    );
}

// trace:STORY-1917 trace:TASK-2341 | ai:agy
#[test]
fn task_2341_default_branch_change_between_submit_and_execute_refuses_and_impl_uses_verified_default(
) {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    let head = crate::test_git::git_command()
        .arg("-C")
        .arg(&f.root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8(head.stdout).unwrap().trim().to_string();
    git(&f.root, &["update-ref", "refs/remotes/origin/trunk", &sha]);

    // 1. Submit request targeting branch `trunk` while `origin/HEAD` points to `origin/main`.
    let brief1 = f.write_brief("b-trunk-drift.md", "Default branch drift test.\n");
    let sub_drift = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "drift-lane".into(),
            branch: "trunk".into(),
            spec: "TASK-100".into(),
            brief: brief1,
            vendor: None,
        },
        now,
    )
    .unwrap();

    // Switch `refs/remotes/origin/HEAD` to `refs/remotes/origin/trunk` BEFORE execution.
    git(
        &f.root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );

    let res_drift =
        execute_one_request(&f.root, &f.mandate_dir, &sub_drift.request.id, false, now).unwrap();
    assert_eq!(res_drift.status, DispatchStatus::Refused);
    assert!(
        res_drift
            .refusal_reason
            .as_deref()
            .unwrap_or("")
            .contains("cannot target the repository default branch `trunk` directly"),
        "execution must re-verify default branch and refuse `trunk`, got: {res_drift:?}"
    );

    // 2. With `origin/HEAD` -> `origin/trunk` (and `origin/trunk` advanced one
    //    commit ahead of `origin/main`), `impl` creates its worktree from the
    //    verified `origin/trunk` commit rather than hardcoded `origin/main`.
    std::fs::write(f.root.join("trunk-marker.txt"), "on trunk\n").unwrap();
    git(&f.root, &["add", "trunk-marker.txt"]);
    git(&f.root, &["commit", "-qm", "advance trunk"]);
    let trunk_head = crate::test_git::git_command()
        .arg("-C")
        .arg(&f.root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let trunk_sha = String::from_utf8(trunk_head.stdout)
        .unwrap()
        .trim()
        .to_string();
    git(
        &f.root,
        &["update-ref", "refs/remotes/origin/trunk", &trunk_sha],
    );
    let fake_agent = f.write_executable_script(
        "ok-agent-trunk.sh",
        "#!/bin/sh\necho 'Started background session (aida claude logs 11112222)'\nexit 0\n",
    );
    let _bins = install_test_bins(Some(fake_agent.clone()), Some(fake_agent));

    let brief2 = f.write_brief("b-trunk-impl.md", "Impl off trunk.\n");
    let sub_ok = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "trunk-ok-lane".into(),
            branch: "claude/from-trunk".into(),
            spec: "TASK-100".into(),
            brief: brief2,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let res_ok =
        execute_one_request(&f.root, &f.mandate_dir, &sub_ok.request.id, false, now).unwrap();
    assert_eq!(
        res_ok.status,
        DispatchStatus::Done,
        "impl must succeed using verified `origin/trunk`: {res_ok:?}"
    );
    let wt_head = crate::test_git::git_command()
        .arg("-C")
        .arg(worktrees_root(&f.root).join("trunk-ok-lane"))
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let wt_sha = String::from_utf8(wt_head.stdout)
        .unwrap()
        .trim()
        .to_string();
    assert_eq!(
        wt_sha, trunk_sha,
        "impl worktree must be based on verified default branch `origin/trunk`, not `origin/main`"
    );
}

// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[test]
fn task_2344_real_detached_worker_missing_result_and_no_duplicate_replay() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    let barrier_dir = f._tmp.path().join("task-2344-barriers");
    std::fs::create_dir_all(&barrier_dir).unwrap();
    let mut watchdog = DetachedWorkerWatchdog::new(barrier_dir.clone());

    let parent_home = crate::aida_home_dir().expect("hermetic test home");
    let test_bin = crate::aida_bin::current_executable_path().expect("current test binary");
    let fake_aida_path = f._tmp.path().join("fake-aida-2344.sh");
    let fake_aida = f.write_executable_script(
        "fake-aida-2344.sh",
        &format!(
            "#!/bin/sh\nset -eu\n\
             if [ \"${{1:-}}\" = \"dispatch\" ] && [ \"${{2:-}}\" = \"run\" ]; then\n\
               req_id=\"$3\"\n\
               via_serve=\"0\"\n\
               if [ \"${{4:-}}\" = \"--via-serve\" ]; then\n\
                 via_serve=\"1\"\n\
               fi\n\
               export AIDA_HOME='{}'\n\
               export AIDA_TEST_1917_CHILD_ID=\"$req_id\"\n\
               export AIDA_TEST_1917_CHILD_VIA_SERVE=\"$via_serve\"\n\
               export AIDA_TEST_1917_MAIN_WORKTREE='{}'\n\
               export AIDA_TEST_1917_MANDATE_DIR='{}'\n\
               export AIDA_TEST_1917_BARRIER_DIR='{}'\n\
               export AIDA_TEST_1917_SYNTHETIC_AIDA='{}'\n\
               exec '{}' --exact gate::dispatch_authority::story_1917_dispatch_exec_tests::story_1917_detached_worker_child_entry --nocapture --test-threads=1\n\
             fi\n\
             if [ \"${{1:-}}\" = \"queue\" ] && [ \"${{2:-}}\" = \"work\" ]; then\n\
               pr_tag=\"${{3:-PR-0}}\"\n\
               echo \"$$:$pr_tag\" >> '{}'\n\
               echo \"$$\" > '{}/entered-'\"$pr_tag\"\n\
               i=0\n\
               while [ \"$i\" -lt 500 ]; do\n\
                 if [ -f '{}/release-'\"$pr_tag\" ] || [ -f '{}/release-all' ]; then\n\
                   echo \"done\" > '{}/completed-'\"$pr_tag\"\n\
                   exit 0\n\
                 fi\n\
                 sleep 0.02\n\
                 i=$((i + 1))\n\
               done\n\
               echo \"watchdog timeout waiting for release-$pr_tag\" >&2\n\
               exit 124\n\
             fi\n\
             exit 1\n",
            parent_home.display(),
            f.root.display(),
            f.mandate_dir.display(),
            barrier_dir.display(),
            fake_aida_path.display(),
            test_bin.display(),
            barrier_dir.join("runner-invocations.log").display(),
            barrier_dir.display(),
            barrier_dir.display(),
            barrier_dir.display(),
            barrier_dir.display(),
        ),
    );
    let _bins = install_test_bins(Some(fake_aida), None);
    let watchdog_timeout = std::time::Duration::from_secs(10);

    let rev_810 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 810,
            vendor: None,
        },
        now + Duration::seconds(1),
    )
    .unwrap();
    let rev_811 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 811,
            vendor: None,
        },
        now + Duration::seconds(2),
    )
    .unwrap();
    let rev_812 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 812,
            vendor: None,
        },
        now + Duration::seconds(3),
    )
    .unwrap();

    // 1. Spawn two real detached workers (PR #810 and PR #811), hitting MAX_CONCURRENT_REVIEWS = 2.
    let pass1 = claim_and_dispatch_pass(&f.root, true, true, now).unwrap();
    assert_eq!(pass1.spawned, 2);
    wait_for_barrier_file(&barrier_dir.join("entered-PR-810"), watchdog_timeout);
    wait_for_barrier_file(&barrier_dir.join("entered-PR-811"), watchdog_timeout);

    let (_, res_810_live) = load_pair(&f.root, &rev_810.request.id).unwrap();
    let (_, res_811_live) = load_pair(&f.root, &rev_811.request.id).unwrap();
    let pid_810 = res_810_live.worker_pid.expect("real child PID for 810");
    let pid_811 = res_811_live.worker_pid.expect("real child PID for 811");
    watchdog.track(pid_810, res_810_live.worker_start.clone());
    watchdog.track(pid_811, res_811_live.worker_start.clone());
    assert_eq!(
        probe_worker_identity(Some(pid_810), res_810_live.worker_start.as_deref()),
        aida_core::process_probe::Probe::Alive
    );

    // 2. Delete `results/<id_810>.json` while the real detached child for 810 is STILL ALIVE.
    std::fs::remove_file(result_path(&f.root, &rev_810.request.id)).unwrap();

    // 3. `claim_and_dispatch_pass` must:
    //    - NOT reset 810 to Pending or spawn a replacement worker for 810;
    //    - count 810 (`needs-recovery`, known verb `review`) + 811 (`Running`, `review`) = 2 slots,
    //      holding 812 `Pending` (`spawned == 0`) so concurrency cap is never exceeded!
    let pass2 = claim_and_dispatch_pass(&f.root, true, true, now).unwrap();
    assert_eq!(pass2.reaped, 0);
    assert_eq!(
        pass2.spawned, 0,
        "broken/live review record 810 must still consume 1 review cap slot so 812 cannot exceed MAX_CONCURRENT_REVIEWS"
    );
    assert!(
        !result_path(&f.root, &rev_810.request.id).exists(),
        "reconciler must never synthesize a Pending result for a missing-result record"
    );
    assert!(
        recovery_path(&f.root).exists(),
        "reconciler must persist .aida/dispatch/recovery.json under tick.lock"
    );

    // 4. Submitting a duplicate or conflicting request for `pr:810` must be refused
    //    naming the `needs-recovery` record.
    let dup_err = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 810,
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(
        dup_err.to_string().contains(&rev_810.request.id)
            && dup_err.to_string().contains("needs-recovery"),
        "expected needs-recovery refusal on pr:810, got: {dup_err:#}"
    );

    // 5. Release Child 810 so it completes its runner side effect and exits.
    //    Even after Child 810 is `Probe::Dead`, subsequent passes must NEVER replay
    //    `rev_810` (`runner_count == 1`).
    std::fs::write(barrier_dir.join("release-PR-810"), b"1").unwrap();
    wait_for_barrier_file(&barrier_dir.join("completed-PR-810"), watchdog_timeout);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while crate::process_probe::pid_is_alive(pid_810) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        !result_path(&f.root, &rev_810.request.id).exists(),
        "verify_claim_locked must refuse to recreate a deleted result file when child 810 exits"
    );

    // Release Child 811 so 812 can now claim the 2nd review slot while 810 holds 1 slot.
    std::fs::write(barrier_dir.join("release-PR-811"), b"1").unwrap();
    let res_811_done = wait_for_request_status(
        &f.root,
        &rev_811.request.id,
        DispatchStatus::Done,
        watchdog_timeout,
    );
    assert_eq!(res_811_done.status, DispatchStatus::Done);

    let pass3 = claim_and_dispatch_pass(&f.root, true, true, now).unwrap();
    assert_eq!(
        pass3.spawned, 1,
        "only 812 may spawn; 810 (`needs-recovery`) must NEVER be replayed"
    );
    assert_eq!(pass3.claimed_ids, vec![rev_812.request.id.clone()]);
    wait_for_barrier_file(&barrier_dir.join("entered-PR-812"), watchdog_timeout);
    let (_, res_812_live) = load_pair(&f.root, &rev_812.request.id).unwrap();
    watchdog.track(
        res_812_live.worker_pid.unwrap(),
        res_812_live.worker_start.clone(),
    );
    std::fs::write(barrier_dir.join("release-PR-812"), b"1").unwrap();
    let res_812_done = wait_for_request_status(
        &f.root,
        &rev_812.request.id,
        DispatchStatus::Done,
        watchdog_timeout,
    );
    assert_eq!(res_812_done.status, DispatchStatus::Done);

    let invocations =
        std::fs::read_to_string(barrier_dir.join("runner-invocations.log")).unwrap_or_default();
    let pr810_runs = invocations
        .lines()
        .filter(|l| l.ends_with(":PR-810"))
        .count();
    assert_eq!(
        pr810_runs, 1,
        "missing-result record 810 must never be replayed after child exit; log:\n{invocations}"
    );

    // 6. Sticky recovery evidence when BOTH halves are absent (`(Missing, Missing)`):
    //    Delete the remaining `requests/<id_810>.json` after `recovery.json` has been
    //    persisted. Loss of the remaining half is NOT positive recovery: subsequent
    //    inspection and reconciliation must retain the unresolved fence for 810 in
    //    `recovery.json` with its known verb/target/dedupe/mandate metadata, still
    //    refuse duplicate submissions on `pr:810`, still reserve 1 review cap slot,
    //    and never replay 810.
    std::fs::remove_file(request_path(&f.root, &rev_810.request.id)).unwrap();
    assert!(!request_path(&f.root, &rev_810.request.id).exists());
    assert!(!result_path(&f.root, &rev_810.request.id).exists());

    let inv_both_missing = inspect_dispatch_inventory_readonly(&f.root).unwrap();
    let fence_810 = inv_both_missing.find_fence(&rev_810.request.id).expect(
        "sticky fence for 810 must survive even when both request and result halves are missing",
    );
    assert_eq!(fence_810.known_verb.as_deref(), Some("review"));
    assert_eq!(fence_810.known_target_key.as_deref(), Some("pr:810"));
    assert_eq!(fence_810.known_dedupe_key.as_deref(), Some("review:pr:810"));
    assert_eq!(
        fence_810.known_mandate_id.as_deref(),
        Some(rev_810.request.mandate_id.as_str())
    );

    // Reconcile under tick.lock and verify `recovery.json` is retained (NOT deleted).
    {
        let _lock = acquire_dispatch_lock(&f.root).unwrap();
        let rec_inv = reconcile_inventory_locked(&f.root, now + Duration::seconds(10)).unwrap();
        assert!(rec_inv.find_fence(&rev_810.request.id).is_some());
    }
    assert!(
        recovery_path(&f.root).exists(),
        "reconcile_inventory_locked must retain recovery.json when both halves of a fenced record disappear"
    );

    // Duplicate submission on `pr:810` must still be refused naming `rev_810.request.id`.
    let dup_after_both_missing = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 810,
            vendor: None,
        },
        now + Duration::seconds(11),
    )
    .unwrap_err();
    assert!(
        dup_after_both_missing
            .to_string()
            .contains(&rev_810.request.id)
            && dup_after_both_missing
                .to_string()
                .contains("needs-recovery"),
        "sticky fence must still refuse duplicate pr:810 after both halves disappear, got: {dup_after_both_missing:#}"
    );

    // Concurrency cap reservation must also survive: with 810 holding 1 review cap slot,
    // submitting two new review requests (813 and 814) and running a dispatch pass may
    // spawn at most ONE of them (`spawned == 1`, `MAX_CONCURRENT_REVIEWS == 2`).
    let rev_813 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 813,
            vendor: None,
        },
        now + Duration::seconds(12),
    )
    .unwrap();
    let rev_814 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 814,
            vendor: None,
        },
        now + Duration::seconds(13),
    )
    .unwrap();
    let pass4 = claim_and_dispatch_pass(&f.root, true, true, now + Duration::seconds(14)).unwrap();
    assert_eq!(
        pass4.spawned, 1,
        "sticky fence for 810 must continue occupying 1 review slot so only 813 spawns"
    );
    assert_eq!(pass4.claimed_ids, vec![rev_813.request.id.clone()]);
    wait_for_barrier_file(&barrier_dir.join("entered-PR-813"), watchdog_timeout);
    let (_, res_813_live) = load_pair(&f.root, &rev_813.request.id).unwrap();
    watchdog.track(
        res_813_live.worker_pid.unwrap(),
        res_813_live.worker_start.clone(),
    );
    assert_eq!(
        load_pair(&f.root, &rev_814.request.id).unwrap().1.status,
        DispatchStatus::Pending,
        "814 must remain Pending while sticky fence 810 + live 813 occupy the 2 review slots"
    );
    std::fs::write(barrier_dir.join("release-PR-813"), b"1").unwrap();
    wait_for_request_status(
        &f.root,
        &rev_813.request.id,
        DispatchStatus::Done,
        watchdog_timeout,
    );
    std::fs::write(barrier_dir.join("release-all"), b"1").unwrap();
}

// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[test]
fn task_2344_tri_state_probe_retains_unknown_missing_pid_missing_start_and_corrupt_request() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    let sub1 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 820,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let sub2 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 821,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let sub3 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 822,
            vendor: None,
        },
        now,
    )
    .unwrap();

    // 1. Sub1: Running with `worker_pid = None` -> `Probe::Unknown`, retained as `Running`.
    let (_, mut res1) = load_pair(&f.root, &sub1.request.id).unwrap();
    res1.status = DispatchStatus::Running;
    res1.started_at = Some(now);
    res1.worker_pid = None;
    res1.worker_start = None;
    res1.claim_token = Some(Uuid::new_v4().to_string());
    write_private_atomic(
        &result_path(&f.root, &sub1.request.id),
        &serde_json::to_vec_pretty(&res1).unwrap(),
    )
    .unwrap();

    // 2. Sub2: Running with `worker_pid = Some(999_991)` but `worker_start = None` -> `Probe::Unknown`, retained as `Running`.
    let (_, mut res2) = load_pair(&f.root, &sub2.request.id).unwrap();
    res2.status = DispatchStatus::Running;
    res2.started_at = Some(now);
    res2.worker_pid = Some(999_991);
    res2.worker_start = None;
    res2.claim_token = Some(Uuid::new_v4().to_string());
    write_private_atomic(
        &result_path(&f.root, &sub2.request.id),
        &serde_json::to_vec_pretty(&res2).unwrap(),
    )
    .unwrap();

    // 3. Sub3: Running with dead PID, but `TEST_FORCE_WORKER_PROBE` injects `Probe::Unknown` -> retained as `Running`.
    let (_, mut res3) = load_pair(&f.root, &sub3.request.id).unwrap();
    res3.status = DispatchStatus::Running;
    res3.started_at = Some(now);
    res3.worker_pid = Some(999_992);
    res3.worker_start = Some("linux-starttime:12345".into());
    res3.claim_token = Some(Uuid::new_v4().to_string());
    write_private_atomic(
        &result_path(&f.root, &sub3.request.id),
        &serde_json::to_vec_pretty(&res3).unwrap(),
    )
    .unwrap();

    TEST_FORCE_WORKER_PROBE.with(|c| {
        *c.borrow_mut() = Some(aida_core::process_probe::Probe::Unknown(
            "simulated /proc permission error".into(),
        ));
    });
    let reaped_unknown = {
        let _lock = acquire_dispatch_lock(&f.root).unwrap();
        reap_dead_workers_locked(&f.root, now).unwrap()
    };
    TEST_FORCE_WORKER_PROBE.with(|c| {
        *c.borrow_mut() = None;
    });
    assert_eq!(
        reaped_unknown, 0,
        "Probe::Unknown must never reap a Running worker"
    );

    // Without the forced override, sub1 (missing pid) and sub2 (missing start) are still Unknown,
    // while sub3 (dead pid + valid linux-starttime format) is Probe::Dead and gets reaped!
    let reaped_real = {
        let _lock = acquire_dispatch_lock(&f.root).unwrap();
        reap_dead_workers_locked(&f.root, now).unwrap()
    };
    assert_eq!(
        reaped_real, 1,
        "only sub3 has positive Probe::Dead evidence"
    );
    assert_eq!(
        load_pair(&f.root, &sub1.request.id).unwrap().1.status,
        DispatchStatus::Running
    );
    assert_eq!(
        load_pair(&f.root, &sub2.request.id).unwrap().1.status,
        DispatchStatus::Running
    );
    assert_eq!(
        load_pair(&f.root, &sub3.request.id).unwrap().1.status,
        DispatchStatus::Failed
    );
}

/// The activation hooks and the `/proc` allowlist are process-global, so the
/// tests that set them run one at a time.
// trace:STORY-1917 | ai:claude
static Q1_HOOK_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[test]
fn task_2344_q1_physical_tick_lock_directory_migration_and_legacy_mutator_refusal() {
    use fs2::FileExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let _serial = Q1_HOOK_SERIAL.lock().unwrap_or_else(|e| e.into_inner());

    struct ResetQ1Hooks;
    impl Drop for ResetQ1Hooks {
        fn drop(&mut self) {
            if let Ok(mut g) = TEST_PROC_PID_ALLOWLIST.lock() {
                *g = None;
            }
            if let Ok(mut g) = TEST_POST_EXCHANGE_HOOK.lock() {
                *g = None;
            }
            if let Ok(mut g) = TEST_PRE_FLOCK_V2_HOOK.lock() {
                *g = None;
            }
        }
    }
    let _reset_hooks = ResetQ1Hooks;

    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    let disp_dir = dispatch_root(&f.root);
    let _ = open_pinned_dispatch_subdir(&f.root, None).unwrap();
    let legacy_lock_path = disp_dir.join("tick.lock");

    // 1. Create a legacy Slice 1/2 regular-file `.aida/dispatch/tick.lock` (mode 0600)
    //    and record its exact `(st_dev, st_ino)` identity.
    let legacy_holder = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&legacy_lock_path)
        .unwrap();
    let orig_meta = legacy_holder.metadata().unwrap();
    let (orig_dev, orig_ino) = (orig_meta.dev(), orig_meta.ino());
    drop(legacy_holder);

    // 2. Ordinary mutation entrypoints (`submit_request`, `cancel_request`,
    //    `run_dispatch_tick_sync`, `run_dispatch_serve_once_sync`, `execute_one_request`)
    //    MUST NOT silently replace the legacy regular-file `tick.lock`; even when
    //    unlocked, they refuse fail-closed with concrete recovery guidance and leave
    //    `tick.lock` untouched as the original regular file inode.
    let sub_refused = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 880,
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(
        sub_refused
            .to_string()
            .contains("legacy regular-file dispatch lock")
            && sub_refused
                .to_string()
                .contains("automatic live lock replacement is refused"),
        "ordinary submit_request must refuse legacy tick.lock without silent migration, got: {sub_refused:#}"
    );
    assert!(cancel_request(&f.root, "00000000", now)
        .unwrap_err()
        .to_string()
        .contains("legacy regular-file dispatch lock"));
    assert!(run_dispatch_tick_sync(&f.root, &f.mandate_dir, now)
        .unwrap_err()
        .to_string()
        .contains("legacy regular-file dispatch lock"));
    assert!(run_dispatch_serve_once_sync(&f.root, &f.mandate_dir, now)
        .unwrap_err()
        .to_string()
        .contains("legacy regular-file dispatch lock"));
    assert!(execute_one_request(
        &f.root,
        &f.mandate_dir,
        "00000000-0000-0000-0000-000000000000",
        false,
        now
    )
    .unwrap_err()
    .to_string()
    .contains("legacy regular-file dispatch lock"));
    let meta_after_ordinary = std::fs::symlink_metadata(&legacy_lock_path).unwrap();
    assert!(
        meta_after_ordinary.is_file()
            && meta_after_ordinary.dev() == orig_dev
            && meta_after_ordinary.ino() == orig_ino,
        "ordinary mutation entrypoints must leave legacy tick.lock regular file inode untouched"
    );

    // 3a. Open-before-flock / waiter race: spawn a real child process that opens
    //     `.aida/dispatch/tick.lock` RW BEFORE calling `flock` and waits on a barrier.
    //     Even though `flock` is NOT yet held on the inode,
    //     `activate_dispatch_v2_lock_quiescent` must detect the open descriptor in
    //     the child process via `/proc` and refuse fail-closed without unlinking!
    let self_pid = std::process::id();
    let race_dir = f._tmp.path().join("q1-open-race");
    std::fs::create_dir_all(&race_dir).unwrap();
    let opened_marker = race_dir.join("child-opened");
    let release_marker = race_dir.join("child-release");
    let mut open_waiter_child = std::process::Command::new(crate::resolve_aida_exe())
        .arg("--exact")
        .arg("gate::dispatch_authority::story_1917_dispatch_exec_tests::story_1917_detached_worker_child_entry")
        .arg("--nocapture")
        .env("AIDA_TEST_1917_OPEN_LOCK_PATH", &legacy_lock_path)
        .env("AIDA_TEST_1917_OPENED_MARKER", &opened_marker)
        .env("AIDA_TEST_1917_RELEASE_MARKER", &release_marker)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn open-before-flock child");

    wait_for_barrier_file(&opened_marker, std::time::Duration::from_secs(5));
    let child_pid: u32 = std::fs::read_to_string(&opened_marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    *TEST_PROC_PID_ALLOWLIST.lock().unwrap() = Some(vec![self_pid, child_pid]);

    let open_race_err = match activate_dispatch_v2_lock_quiescent(&f.root) {
        Ok(_) => {
            let _ = std::fs::write(&release_marker, b"1");
            let _ = open_waiter_child.wait();
            panic!("expected quiescent activation refusal while child holds open-before-flock FD on legacy tick.lock");
        }
        Err(e) => e,
    };
    assert!(
        open_race_err
            .to_string()
            .contains("open-before-flock or waiter race detected"),
        "expected open-before-flock refusal naming child FD, got: {open_race_err:#}"
    );
    let meta_after_race = std::fs::symlink_metadata(&legacy_lock_path).unwrap();
    assert!(
        meta_after_race.is_file()
            && meta_after_race.dev() == orig_dev
            && meta_after_race.ino() == orig_ino,
        "legacy tick.lock must remain the exact unreplaced regular file inode when open-before-flock race is detected"
    );

    // Release the open-before-flock child and wait for it to close FD and exit.
    std::fs::write(&release_marker, b"1").unwrap();
    let _ = open_waiter_child.wait();

    // 3b. Unreadable same-UID `/proc/<pid>/fd` (non-dumpable process via
    //     `prctl(PR_SET_DUMPABLE, 0)`): `verify_sole_open_fd_for_inode` treats
    //     unreadable same-UID `/proc/<pid>/fd` (`EACCES`) as unknown FD coverage
    //     and fails closed instead of silently skipping the process.
    let nd_opened_marker = race_dir.join("nd-child-opened");
    let nd_release_marker = race_dir.join("nd-child-release");
    let mut nd_child = std::process::Command::new(crate::resolve_aida_exe())
        .arg("--exact")
        .arg("gate::dispatch_authority::story_1917_dispatch_exec_tests::story_1917_detached_worker_child_entry")
        .arg("--nocapture")
        .env("AIDA_TEST_1917_OPEN_LOCK_PATH", &legacy_lock_path)
        .env("AIDA_TEST_1917_NON_DUMPABLE", "1")
        .env("AIDA_TEST_1917_OPENED_MARKER", &nd_opened_marker)
        .env("AIDA_TEST_1917_RELEASE_MARKER", &nd_release_marker)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn non-dumpable open-before-flock child");

    wait_for_barrier_file(&nd_opened_marker, std::time::Duration::from_secs(5));
    let nd_pid: u32 = std::fs::read_to_string(&nd_opened_marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    *TEST_PROC_PID_ALLOWLIST.lock().unwrap() = Some(vec![self_pid, nd_pid]);

    let nd_err = match activate_dispatch_v2_lock_quiescent(&f.root) {
        Ok(_) => {
            let _ = std::fs::write(&nd_release_marker, b"1");
            let _ = nd_child.wait();
            panic!(
                "expected fail-closed refusal when same-UID process has unreadable proc fd directory"
            );
        }
        Err(e) => e,
    };
    assert!(
        nd_err.to_string().contains("unknown FD coverage; fail-closed")
            || nd_err
                .to_string()
                .contains("open-before-flock or waiter race detected"),
        "expected fail-closed unknown FD coverage refusal for non-dumpable same-UID process, got: {nd_err:#}"
    );
    std::fs::write(&nd_release_marker, b"1").unwrap();
    let _ = nd_child.wait();

    // 4. Held-flock check: when a legacy mutator holds an exclusive `flock` on
    //    `tick.lock`, `activate_dispatch_v2_lock_quiescent` refuses fail-closed.
    *TEST_PROC_PID_ALLOWLIST.lock().unwrap() = Some(vec![self_pid]);
    let active_holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&legacy_lock_path)
        .unwrap();
    active_holder.lock_exclusive().unwrap();
    let held_err = match activate_dispatch_v2_lock_quiescent(&f.root) {
        Ok(_) => panic!("expected migration refusal while legacy tick.lock is actively flocked"),
        Err(e) => e,
    };
    assert!(
        held_err
            .to_string()
            .contains("actively held by another process"),
        "expected active legacy lock refusal, got: {held_err:#}"
    );
    let _ = fs2::FileExt::unlock(&active_holder);
    drop(active_holder);

    // 5. Post-exchange second-scan refusal + concurrent `acquire_dispatch_lock` waiter rollback:
    //    Pause immediately after `RENAME_EXCHANGE` (while `activate_dispatch_v2_lock_quiescent`
    //    holds `flock` on BOTH `leg_file` and `v2.lock` and before `tick.lock/version` is written):
    //    - Park a concurrent `acquire_dispatch_lock` waiter on `tick.lock/v2.lock`.
    //    - Open a second descriptor to the swapped-out legacy regular file (`.tick.lock.v2-staging`,
    //      which has `(orig_dev, orig_ino)`) so the second `verify_sole_open_fd_for_inode` scan
    //      fails!
    //    - Verify `activate_dispatch_v2_lock_quiescent` rolls back via `RENAME_EXCHANGE`,
    //      restoring the EXACT original regular-file inode `(orig_dev, orig_ino)` to
    //      `.aida/dispatch/tick.lock`, unlinking `v2.lock`, and causing the parked
    //      `acquire_dispatch_lock` waiter to refuse fail-closed after waking from `flock`!
    let extra_legacy_fd: std::sync::Arc<std::sync::Mutex<Option<std::fs::File>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let extra_legacy_fd_hook = extra_legacy_fd.clone();
    let waiter_handle_slot: std::sync::Arc<
        std::sync::Mutex<Option<std::thread::JoinHandle<anyhow::Result<()>>>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(None));
    let waiter_handle_slot_hook = waiter_handle_slot.clone();
    let root_for_hook = f.root.clone();
    let staged_legacy_path = disp_dir.join(".tick.lock.v2-staging");
    let staged_legacy_path_hook = staged_legacy_path.clone();

    *TEST_POST_EXCHANGE_HOOK.lock().unwrap() = Some(Box::new(move || {
        let (tx_pre, rx_pre) = std::sync::mpsc::channel();
        *TEST_PRE_FLOCK_V2_HOOK.lock().unwrap() = Some(Box::new(move || {
            let _ = tx_pre.send(());
        }));
        let root_waiter = root_for_hook.clone();
        let h = std::thread::spawn(move || {
            let _guard = acquire_dispatch_lock(&root_waiter)?;
            Ok(())
        });
        rx_pre
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("concurrent acquire_dispatch_lock waiter reached pre-flock hook");
        // Give the waiter thread a brief moment to enter the blocking `lock_exclusive` syscall.
        std::thread::sleep(std::time::Duration::from_millis(30));
        *waiter_handle_slot_hook.lock().unwrap() = Some(h);

        // Open a second descriptor to the original legacy inode (now at `.tick.lock.v2-staging`)
        // so the second `verify_sole_open_fd_for_inode` scan detects an extra open FD.
        let second_fd = std::fs::OpenOptions::new()
            .read(true)
            .open(&staged_legacy_path_hook)
            .expect("open swapped-out legacy inode to trigger second-scan refusal");
        *extra_legacy_fd_hook.lock().unwrap() = Some(second_fd);
    }));

    let second_scan_err = activate_dispatch_v2_lock_quiescent(&f.root)
        .expect_err("second-scan open FD must cause activate_dispatch_v2_lock_quiescent to refuse and roll back");
    assert!(
        second_scan_err
            .to_string()
            .contains("open-before-flock or waiter race detected"),
        "expected second-scan open-before-flock refusal, got: {second_scan_err:#}"
    );
    *TEST_POST_EXCHANGE_HOOK.lock().unwrap() = None;
    *TEST_PRE_FLOCK_V2_HOOK.lock().unwrap() = None;
    // Drop the extra FD we opened during the hook.
    *extra_legacy_fd.lock().unwrap() = None;

    // Join the concurrent `acquire_dispatch_lock` waiter and prove it failed closed on wakeup!
    let waiter_res = waiter_handle_slot
        .lock()
        .unwrap()
        .take()
        .expect("waiter thread spawned")
        .join()
        .expect("waiter thread panicked");
    let waiter_err =
        waiter_res.expect_err("concurrent acquire_dispatch_lock waiter must refuse after rollback");
    assert!(
        waiter_err
            .to_string()
            .contains("was unlinked or rolled back while waiting for dispatch lock")
            || waiter_err
                .to_string()
                .contains("directory identity changed or rolled back"),
        "concurrent waiter must fail closed on post-flock rollback verification, got: {waiter_err:#}"
    );

    // Verify `.aida/dispatch/tick.lock` was restored to the EXACT original regular-file inode!
    let restored_meta = std::fs::symlink_metadata(&legacy_lock_path).unwrap();
    assert!(
        restored_meta.is_file()
            && restored_meta.dev() == orig_dev
            && restored_meta.ino() == orig_ino
            && restored_meta.nlink() == 1,
        "rollback must restore the exact original regular-file inode (dev={}, ino={}), got is_file={}, dev={}, ino={}, nlink={}",
        orig_dev,
        orig_ino,
        restored_meta.is_file(),
        restored_meta.dev(),
        restored_meta.ino(),
        restored_meta.nlink()
    );
    assert!(
        !staged_legacy_path.exists(),
        ".tick.lock.v2-staging must be cleaned up after rollback"
    );

    // 6. Explicit quiescent activation: with zero extra open FDs and verified
    //    pathname-to-handle identity, `activate_dispatch_v2_lock_quiescent` atomically
    //    exchanges `.aida/dispatch/tick.lock` with the 0700 directory containing
    //    `v2.lock` and `version` ("2\n") and unlinks the swapped-out legacy inode.
    let v2_guard = activate_dispatch_v2_lock_quiescent(&f.root).unwrap();
    drop(v2_guard);
    assert!(
        legacy_lock_path.is_dir(),
        ".aida/dispatch/tick.lock must now be a 0700 directory"
    );
    assert!(
        legacy_lock_path.join("v2.lock").is_file(),
        ".aida/dispatch/tick.lock/v2.lock must exist as the v2 lock file"
    );
    assert_eq!(
        std::fs::read_to_string(legacy_lock_path.join("version"))
            .unwrap()
            .trim(),
        "2",
        ".aida/dispatch/tick.lock/version must record schema version 2"
    );
    assert!(
        !staged_legacy_path.exists(),
        ".tick.lock.v2-staging must be unlinked on committed v2 activation"
    );

    // 7. All 5 pre-Slice-3 mutation entrypoints (`submit`, `tick`, `serve`, `run`,
    //    `cancel`/`finalizer`) that call the legacy `OpenOptions::new().create(true).truncate(false).read(true).write(true).open(dir.join("tick.lock"))`
    //    now fail at the OS boundary with `EISDIR`, while v2 mutators succeed.
    for entrypoint in ["submit", "tick", "serve", "run", "cancel"] {
        let legacy_open_err = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&legacy_lock_path)
            .unwrap_err();
        assert_eq!(
            legacy_open_err.raw_os_error(),
            Some(libc::EISDIR),
            "legacy `{entrypoint}` mutator lock open must fail with EISDIR on migrated tick.lock directory"
        );
    }

    let sub_v2 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 881,
            vendor: None,
        },
        now,
    )
    .expect("v2 submit_request succeeds after explicit quiescent activation");
    assert_eq!(sub_v2.result.status, DispatchStatus::Pending);
}

/// Review round 1, findings 3 and 4 and the legacy-mode minor: a legacy lock
/// with the umask's default mode is accepted; a failed rollback is reported
/// and leaves a marker-less v2 directory that every mutator refuses; the
/// activation command then resumes only after quiescence is proven again.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
#[test]
fn task_2344_q1_failed_rollback_is_fenced_and_activation_resumes_only_when_quiescent() {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let _serial = Q1_HOOK_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            if let Ok(mut g) = TEST_PROC_PID_ALLOWLIST.lock() {
                *g = None;
            }
            if let Ok(mut g) = TEST_POST_EXCHANGE_HOOK.lock() {
                *g = None;
            }
            TEST_FAIL_ROLLBACK_EXCHANGE.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let _reset = Reset;

    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();
    let disp_dir = dispatch_root(&f.root);
    let _ = open_pinned_dispatch_subdir(&f.root, None).unwrap();
    let lock_path = disp_dir.join("tick.lock");
    let staging = disp_dir.join(".tick.lock.v2-staging");

    // Slice 1/2 created the lock with OpenOptions::create, i.e. the umask's mode.
    let legacy = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o644)
        .open(&lock_path)
        .unwrap();
    let orig_ino = legacy.metadata().unwrap().ino();
    drop(legacy);
    *TEST_PROC_PID_ALLOWLIST.lock().unwrap() = Some(vec![std::process::id()]);

    // Second scan sees an extra descriptor AND the exchange-back fails.
    let extra: std::sync::Arc<std::sync::Mutex<Option<std::fs::File>>> = Default::default();
    let extra_hook = extra.clone();
    let staging_hook = staging.clone();
    *TEST_POST_EXCHANGE_HOOK.lock().unwrap() = Some(Box::new(move || {
        *extra_hook.lock().unwrap() = Some(std::fs::File::open(&staging_hook).unwrap());
        TEST_FAIL_ROLLBACK_EXCHANGE.store(true, std::sync::atomic::Ordering::SeqCst);
    }));
    let err = activate_dispatch_v2_lock_quiescent(&f.root).expect_err("second scan must refuse");
    let text = format!("{err:#}");
    assert!(
        text.contains("rolling back the dispatch lock activation failed")
            && text.contains("open-before-flock or waiter race detected")
            && text.contains("aida dispatch activate-lock"),
        "{text}"
    );
    // The fenced state: v2 directory without its marker; legacy inode at staging.
    assert!(lock_path.is_dir());
    assert!(!lock_path.join("version").exists());
    assert_eq!(std::fs::symlink_metadata(&staging).unwrap().ino(), orig_ino);

    // Every v2 mutator refuses the marker-less directory; nothing auto-adopts it.
    let refused = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 882,
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    let refused = format!("{refused:#}");
    assert!(
        refused.contains("has no version marker")
            && refused.contains("aida dispatch activate-lock"),
        "{refused}"
    );
    assert!(
        !lock_path.join("version").exists(),
        "a refusal must not write the marker"
    );
    assert!(run_dispatch_tick_sync(&f.root, &f.mandate_dir, now)
        .unwrap_err()
        .to_string()
        .contains("has no version marker"));
    // Old binaries fail at the OS boundary.
    let old_open = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap_err();
    assert_eq!(old_open.raw_os_error(), Some(libc::EISDIR));

    // Resume re-proves quiescence: the extra descriptor still refuses it.
    let resume_err = activate_dispatch_v2_lock_quiescent(&f.root).unwrap_err();
    assert!(
        format!("{resume_err:#}").contains("open-before-flock or waiter race detected"),
        "{resume_err:#}"
    );
    assert!(!lock_path.join("version").exists());

    // Once quiescent, the activation command finishes the migration.
    *extra.lock().unwrap() = None;
    drop(activate_dispatch_v2_lock_quiescent(&f.root).expect("resume once quiescent"));
    assert_eq!(
        std::fs::read_to_string(lock_path.join("version")).unwrap(),
        "2\n"
    );
    assert!(!staging.exists());
    submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 883,
            vendor: None,
        },
        now,
    )
    .expect("dispatch works after the resumed activation");
}

/// Review round 1, finding 4: a fresh root gets its marker atomically, and a
/// marker-less directory with no legacy inode left is refused by both the
/// mutators and the activation command, with manual guidance.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
#[test]
fn task_2344_q1_missing_version_is_never_initialised_outside_fresh_creation() {
    use std::os::unix::fs::PermissionsExt;
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();
    let disp_dir = dispatch_root(&f.root);
    let lock_path = disp_dir.join("tick.lock");

    // Fresh creation: the directory appears with its marker, no leftovers.
    drop(acquire_dispatch_lock(&f.root).unwrap());
    assert_eq!(
        std::fs::read_to_string(lock_path.join("version")).unwrap(),
        "2\n"
    );
    assert_eq!(
        std::fs::metadata(&lock_path).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    let leftovers: Vec<_> = std::fs::read_dir(&disp_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".tick.lock"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // Simulate an activation interrupted after its exchange whose legacy
    // inode is gone: the marker is missing and nothing proves quiescence.
    std::fs::remove_file(lock_path.join("version")).unwrap();
    let err = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 884,
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("has no version marker"),
        "{err:#}"
    );
    let act = activate_dispatch_v2_lock_quiescent(&f.root).unwrap_err();
    let act = format!("{act:#}");
    assert!(
        act.contains("no legacy lock file remains") && act.contains("remove"),
        "{act}"
    );
    assert!(!lock_path.join("version").exists());
}

/// Review round 1, suspected item: a fence reserves its target at execution
/// too. A request queued on a target before another record on that target
/// lost its evidence is neither claimed by a pass nor run directly.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
#[test]
fn task_2344_fenced_target_is_reserved_at_execution_not_only_submission() {
    let f = ExecFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();
    let verb = DispatchRequestVerb::Review {
        pr: 891,
        vendor: None,
    };
    // A: finished earlier on pr:891.
    let a = submit_request(&f.root, &f.mandate_dir, &f.principal, &verb, now).unwrap();
    {
        let _lock = acquire_dispatch_lock(&f.root).unwrap();
        let (_, mut res) = load_pair(&f.root, &a.request.id).unwrap();
        res.status = DispatchStatus::Done;
        res.finished_at = Some(now);
        write_private_atomic(
            &result_path(&f.root, &a.request.id),
            &serde_json::to_vec_pretty(&res).unwrap(),
        )
        .unwrap();
    }
    // B: queued on the same target once A was terminal.
    let b = submit_request(&f.root, &f.mandate_dir, &f.principal, &verb, now).unwrap();
    assert_ne!(a.request.id, b.request.id);
    assert_eq!(b.result.status, DispatchStatus::Pending);
    // A's result is lost: A becomes a fence on pr:891.
    std::fs::remove_file(result_path(&f.root, &a.request.id)).unwrap();

    let pass = claim_and_dispatch_pass(&f.root, true, false, now).unwrap();
    assert!(pass.claimed_ids.is_empty(), "{:?}", pass.claimed_ids);
    assert_eq!(
        load_pair(&f.root, &b.request.id).unwrap().1.status,
        DispatchStatus::Pending
    );
    let err = execute_one_request(&f.root, &f.mandate_dir, &b.request.id, true, now).unwrap_err();
    let text = format!("{err:#}");
    assert!(
        text.contains("is reserved by dispatch record") && text.contains(&a.request.id),
        "{text}"
    );
    assert!(
        text.contains("recovery.json"),
        "names the recovery procedure: {text}"
    );
    assert_eq!(
        load_pair(&f.root, &b.request.id).unwrap().1.status,
        DispatchStatus::Pending
    );
    // Unknown state is never a false zero for the schedule predicate.
    assert_eq!(pending_request_count(&f.root), 2, "B pending + A's fence");
}

/// Review round 1: the recovery index writer holds itself to the reader's
/// bounds, so it never writes an index every later read would reject.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
#[test]
fn task_2344_recovery_index_writer_respects_reader_bounds() {
    assert!(check_recovery_index_bounds(MAX_DISPATCH_INVENTORY_ENTRIES, 1024).is_ok());
    let over_count = check_recovery_index_bounds(MAX_DISPATCH_INVENTORY_ENTRIES + 1, 1024)
        .unwrap_err()
        .to_string();
    assert!(
        over_count.contains("more than the recovery index can hold"),
        "{over_count}"
    );
    assert!(over_count.contains("recovery.json"), "{over_count}");
    assert!(check_recovery_index_bounds(1, MAX_RECOVERY_FILE_BYTES + 1).is_err());
}

/// Review round 3 (fail-open minor): `/proc` inside its own PID namespace,
/// or mounted `hidepid`, cannot show another process holding the legacy
/// lock, so the quiescence proof refuses instead of reporting it unused.
// trace:STORY-1917 | ai:claude
#[test]
fn story_1917_proc_view_refusal_covers_namespaces_and_hidepid() {
    use crate::agent_registry::PidNamespaceProbe;
    let host = PidNamespaceProbe {
        self_ns: Some(4026531836),
        pid1_ns: Some(4026531836),
        nspid_levels: 1,
        pid1_comm: Some("systemd".into()),
    };
    let opts = "rw,nosuid,nodev,noexec,relatime,rw";
    assert_eq!(proc_view_refusal(&host, Some(opts)), None);
    for options in ["rw,hidepid=0", "rw,relatime,rw,hidepid=off"] {
        assert_eq!(proc_view_refusal(&host, Some(options)), None, "{options}");
    }
    for (v, options) in [
        ("2", "rw,relatime,rw,hidepid=2"),
        ("invisible", "rw,rw,hidepid=invisible,subset=pid"),
        ("1", "rw,hidepid=1"),
    ] {
        let why = proc_view_refusal(&host, Some(options)).expect(options);
        assert!(why.contains(&format!("hidepid={v}")), "{why}");
    }
    assert!(proc_view_refusal(&host, None)
        .unwrap()
        .contains("could not be read"));
    let nested = PidNamespaceProbe {
        nspid_levels: 2,
        ..host.clone()
    };
    let other_ns = PidNamespaceProbe {
        pid1_ns: Some(4026532001),
        ..host.clone()
    };
    let sandbox_init = PidNamespaceProbe {
        pid1_ns: None,
        pid1_comm: Some("bwrap".into()),
        ..host.clone()
    };
    for probe in [nested, other_ns, sandbox_init] {
        let why = proc_view_refusal(&probe, Some(opts)).expect("isolated");
        assert!(why.contains("own PID namespace"), "{probe:?}: {why}");
    }

    let mountinfo = "22 1 0:21 / /sys rw,nosuid - sysfs sysfs rw\n\
                     23 1 0:22 / /proc rw,nosuid,nodev - proc proc rw\n\
                     41 23 0:40 / /proc rw,relatime - proc proc rw,hidepid=2\n";
    assert_eq!(
        proc_mount_options(mountinfo).as_deref(),
        Some("rw,relatime,rw,hidepid=2")
    );
    assert_eq!(
        proc_mount_options("22 1 0:21 / /sys rw - sysfs sysfs rw\n"),
        None
    );
}

/// The activation path consults the view: in a PID namespace of its own, or
/// under `hidepid`, `aida dispatch activate-lock` refuses and the legacy lock
/// stays a regular file at its path.
// trace:STORY-1917 | ai:claude
#[test]
fn story_1917_activate_lock_refuses_when_proc_cannot_see_other_processes() {
    use crate::agent_registry::PidNamespaceProbe;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let _serial = Q1_HOOK_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            if let Ok(mut g) = TEST_PROC_VIEW.lock() {
                *g = None;
            }
        }
    }
    let _reset = Reset;

    let f = ExecFixture::new(&["reviewer", "implementer"]);
    let disp_dir = dispatch_root(&f.root);
    let _ = open_pinned_dispatch_subdir(&f.root, None).unwrap();
    let lock_path = disp_dir.join("tick.lock");
    let legacy = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
        .unwrap();
    let orig_ino = legacy.metadata().unwrap().ino();
    drop(legacy);

    let host = PidNamespaceProbe {
        self_ns: Some(4026531836),
        pid1_ns: Some(4026531836),
        nspid_levels: 1,
        pid1_comm: Some("systemd".into()),
    };
    let sandboxed = PidNamespaceProbe {
        nspid_levels: 2,
        ..host.clone()
    };
    for (probe, options, expected) in [
        (sandboxed, Some("rw"), "own PID namespace"),
        (host, Some("rw,hidepid=2"), "hidepid=2"),
    ] {
        *TEST_PROC_VIEW.lock().unwrap() = Some((probe, options.map(str::to_string)));
        let err = activate_dispatch_v2_lock_quiescent(&f.root).expect_err("must refuse");
        let text = format!("{err:#}");
        assert!(
            text.contains(expected) && text.contains("fail-closed"),
            "{text}"
        );
        let meta = std::fs::symlink_metadata(&lock_path).unwrap();
        assert!(meta.is_file(), "the legacy lock stays a regular file");
        assert_eq!(meta.ino(), orig_ino);
    }
}
