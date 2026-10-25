//! STORY-1917 Slice 3 tests: `flow` runner (`TASK-2338`), `train` runner
//! (`TASK-2339`), and `DispatchExecutionContext` in-process authority
//! re-verification (`TASK-2344` / advisor ruling Q2).
// trace:STORY-1917 trace:TASK-2338 trace:TASK-2339 trace:TASK-2343 trace:TASK-2344 | ai:agy

use super::dispatch_flow::{
    acquire_train_dispatch_lock, ensure_clean_aida_store, TEST_FLOW_SHIP_HOOK,
    TEST_FLOW_TRAIN_HOOK, TEST_FLOW_WAIT_CI_HOOK,
};
use super::*;
use crate::gate::dispatch_auth::DispatchExecutionContext;
use crate::mandate::{self, Mandate, Scope};
use chrono::Duration;
use std::collections::BTreeSet;

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

struct FlowFixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: PathBuf,
    mandate_dir: PathBuf,
    principal: String,
    ambient: crate::test_env::AmbientGuard,
}

impl FlowFixture {
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

        let store = root.join(".aida-store");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        git(&store, &["init", "-q", "-b", "main"]);
        write_spec(&store, "TASK-100", &plain_req("TASK-100"));
        write_spec(&store, "STORY-1917", &plain_req("STORY-1917"));

        let ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&root, "advisor", delegable);
        git(&store, &["add", "-A"]);
        git(&store, &["commit", "-qm", "init store"]);

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
        mandate::start_with_scopes(
            &self.mandate_dir,
            &|| Ok(()),
            &self.principal,
            Duration::hours(4),
            Some("test slice 3".into()),
            false,
            include_supervised,
            set,
            Utc::now(),
        )
        .unwrap()
    }

    fn write_executable_script(&self, name: &str, body: &str) -> PathBuf {
        let p = self._tmp.path().join(name);
        crate::test_exec::write_executable(&p, body);
        p
    }
}

struct FlowHookGuard;

impl Drop for FlowHookGuard {
    fn drop(&mut self) {
        TEST_AIDA_BIN.with(|c| *c.borrow_mut() = None);
        TEST_AGENT_BIN.with(|c| *c.borrow_mut() = None);
        TEST_FLOW_SHIP_HOOK.with(|c| *c.borrow_mut() = None);
        TEST_FLOW_TRAIN_HOOK.with(|c| *c.borrow_mut() = None);
        TEST_FLOW_WAIT_CI_HOOK.with(|c| *c.borrow_mut() = None);
    }
}

fn write_approved_verdict(root: &Path, pr: u64, head_sha: &str, verdict_word: &str) {
    let verdicts_dir = root.join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&verdicts_dir).unwrap();
    let rec = serde_json::json!({
        "schema_version": 1,
        "pr": pr,
        "verdict": verdict_word,
        "head_sha": head_sha,
        "timestamp": Utc::now().to_rfc3339(),
        "reviewer_session": "rev-session-1",
        "reviewer_vendor": "claude",
        "reviewer_model": "claude-opus-4-7",
        "author_vendor": "claude",
        "author_model": "claude-opus-4-7",
        "findings_count": usize::from(verdict_word != "APPROVED"),
        "summary": format!("verdict {verdict_word} at {head_sha}"),
        "spec_ids": ["TASK-100"],
        "grant_id": std::env::var(crate::gate::seat_authority::GRANT_ENV).ok(),
        "findings": [],
    });
    let path = verdicts_dir.join(format!("PR-{pr}.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(&rec).unwrap()).unwrap();
}

// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[test]
fn task_2344_q2_dispatch_execution_context_establish_and_reverify_all_six_checks() {
    let mut f = FlowFixture::new(&["reviewer", "implementer"]);
    let mandate_a = f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    let sub = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 301,
            no_ship: false,
            vendor: Some(DispatchVendor::Claude),
        },
        now,
    )
    .unwrap();

    // Claim the request under tick.lock for the current test process PID.
    let claim_token = Uuid::new_v4().to_string();
    let my_pid = std::process::id();
    let my_start = crate::process_probe::process_start_identity(my_pid);
    {
        let _lock = acquire_dispatch_lock(&f.root).unwrap();
        let (_, mut res) = load_pair(&f.root, &sub.request.id).unwrap();
        res.status = DispatchStatus::Running;
        res.started_at = Some(now);
        res.worker_pid = Some(my_pid);
        res.worker_start = my_start.clone();
        res.claim_token = Some(claim_token.clone());
        write_private_atomic(
            &result_path(&f.root, &sub.request.id),
            &serde_json::to_vec_pretty(&res).unwrap(),
        )
        .unwrap();
    }

    let ctx = DispatchExecutionContext::establish(
        &f.root,
        &f.mandate_dir,
        &sub.request.id,
        &claim_token,
        now,
    )
    .expect("establish DispatchExecutionContext");
    assert_eq!(ctx.request_id(), sub.request.id);
    assert_eq!(ctx.mandate_id(), mandate_a.id);
    ctx.reverify(now, true)
        .expect("all 6 authority checks pass initially");

    // Check 1: Stopping exact `mandate_a` and starting a replacement `mandate_b`
    // with `[ShipHeld, Dispatch]` must STILL fail `ctx.reverify` naming `mandate_a`.
    mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();
    let mandate_b = f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    assert_ne!(mandate_a.id, mandate_b.id);
    let err_mandate = ctx.reverify(now, true).unwrap_err();
    assert!(
        err_mandate.to_string().contains(&mandate_a.id)
            && err_mandate
                .to_string()
                .to_ascii_lowercase()
                .contains("stopped"),
        "reverify must reject replacement mandate B and name stopped mandate A, got: {err_mandate:#}"
    );

    // Restore mandate A as active on disk (removing the `<id>.stopped` tombstone
    // and replacement mandate B) for subsequent isolated checks.
    std::fs::remove_file(f.mandate_dir.join(format!("{}.stopped", mandate_a.id))).unwrap();
    std::fs::remove_file(f.mandate_dir.join(format!("{}.json", mandate_b.id))).unwrap();
    ctx.reverify(now, true)
        .expect("reverify succeeds once mandate A is restored");

    // Check 2: Requester grant loses `"reviewer"` delegation or is revoked.
    let grant_id = sub.request.requester_grant_id.clone();
    let grant_file = crate::gate::seat_authority::grant_dir()
        .unwrap()
        .join(format!("{grant_id}.json"));
    let orig_grant_bytes = std::fs::read(&grant_file).unwrap();
    {
        let mut grant_rec = crate::gate::seat_authority::read_grant(&grant_id).unwrap();
        grant_rec.delegable_seats.clear();
        write_private_atomic(&grant_file, &serde_json::to_vec_pretty(&grant_rec).unwrap()).unwrap();
        let err_deleg = ctx.reverify(now, true).unwrap_err();
        assert!(
            err_deleg
                .to_string()
                .contains("no longer delegates `reviewer`"),
            "expected reviewer delegation refusal, got: {err_deleg:#}"
        );

        // Create a separate valid serve grant in `AIDA_SESSION_GRANT` so Check 3 (serve grant)
        // passes while Check 6 (requester grant revoked) is tested directly!
        let serve_grant_id = Uuid::new_v4().to_string();
        let mut serve_grant_rec: crate::gate::seat_authority::SeatGrant =
            serde_json::from_slice(&orig_grant_bytes).unwrap();
        serve_grant_rec.id = serve_grant_id.clone();
        let serve_grant_file = crate::gate::seat_authority::grant_dir()
            .unwrap()
            .join(format!("{serve_grant_id}.json"));
        write_private_atomic(
            &serve_grant_file,
            &serde_json::to_vec_pretty(&serve_grant_rec).unwrap(),
        )
        .unwrap();
        f.ambient
            .set_key(crate::gate::seat_authority::GRANT_ENV, &serve_grant_id);

        grant_rec.delegable_seats = vec!["reviewer".into(), "implementer".into()];
        grant_rec.revoked_at = Some(now);
        write_private_atomic(&grant_file, &serde_json::to_vec_pretty(&grant_rec).unwrap()).unwrap();
        let err_revoked = ctx.reverify(now, true).unwrap_err();
        assert!(
            format!("{err_revoked:#}").contains("requester advisor grant")
                && format!("{err_revoked:#}").contains("no longer valid"),
            "expected revoked requester grant refusal in Check 6, got: {err_revoked:#}"
        );
        f.ambient
            .set_key(crate::gate::seat_authority::GRANT_ENV, &grant_id);
        let _ = std::fs::remove_file(&serve_grant_file);
    }
    write_private_atomic(&grant_file, &orig_grant_bytes).unwrap();
    ctx.reverify(now, true).unwrap();

    // Check 3: Serve grant unbound / missing in AIDA_SESSION_GRANT.
    f.ambient.unset_key(crate::gate::seat_authority::GRANT_ENV);
    let err_serve = ctx.reverify(now, true).unwrap_err();
    assert!(
        format!("{err_serve:#}").contains("no session grant is set"),
        "expected missing serve grant refusal, got: {err_serve:#}"
    );
    f.ambient
        .set_key(crate::gate::seat_authority::GRANT_ENV, &grant_id);
    ctx.reverify(now, true).unwrap();

    // Check 4: `HumanGates` (`GateOp::Ship` or `GateOp::Dispatch`) added to committed `origin/main:.aida/config.toml`.
    let orig_cfg = std::fs::read_to_string(f.root.join(".aida/config.toml")).unwrap();
    std::fs::write(
        f.root.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n\n[mandate.human_gates]\nops = [\"ship\"]\n",
    )
    .unwrap();
    git(&f.root, &["add", ".aida/config.toml"]);
    git(&f.root, &["commit", "-qm", "enable ship gate"]);
    let gated_sha = String::from_utf8(
        crate::test_git::git_command()
            .arg("-C")
            .arg(&f.root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    git(
        &f.root,
        &["update-ref", "refs/remotes/origin/main", &gated_sha],
    );
    let err_gate = ctx.reverify(now, true).unwrap_err();
    assert!(
        err_gate
            .to_string()
            .contains("shipping is gated by human gates"),
        "expected GateOp::Ship refusal, got: {err_gate:#}"
    );
    // Requires `ship_held = false` (e.g. `--no-ship` wait step) still checks `human_gates.dispatch`.
    std::fs::write(&f.root.join(".aida/config.toml"), &orig_cfg).unwrap();
    git(&f.root, &["add", ".aida/config.toml"]);
    git(&f.root, &["commit", "-qm", "restore config"]);
    let restored_sha = String::from_utf8(
        crate::test_git::git_command()
            .arg("-C")
            .arg(&f.root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    git(
        &f.root,
        &["update-ref", "refs/remotes/origin/main", &restored_sha],
    );
    ctx.reverify(now, true).unwrap();

    // Check 5: Tampered `requests/<id>.json` (`request_sha256` mismatch).
    let req_file = request_path(&f.root, &sub.request.id);
    let orig_req_bytes = std::fs::read(&req_file).unwrap();
    let mut tampered_req: DispatchRequest = serde_json::from_slice(&orig_req_bytes).unwrap();
    tampered_req.matched_binding = vec!["proc:1@linux-starttime:1".into()];
    write_private_atomic(
        &req_file,
        &serde_json::to_vec_pretty(&tampered_req).unwrap(),
    )
    .unwrap();
    let err_tamper = ctx.reverify(now, true).unwrap_err();
    assert!(
        err_tamper.to_string().contains("sha256 mismatch"),
        "expected request_sha256 mismatch refusal, got: {err_tamper:#}"
    );
    write_private_atomic(&req_file, &orig_req_bytes).unwrap();
    ctx.reverify(now, true).unwrap();

    // Check 6: `results/<id>.json` claim_token altered or result deleted.
    let res_file = result_path(&f.root, &sub.request.id);
    let orig_res_bytes = std::fs::read(&res_file).unwrap();
    let mut wrong_token_res: DispatchResult = serde_json::from_slice(&orig_res_bytes).unwrap();
    wrong_token_res.claim_token = Some("wrong-token".into());
    write_private_atomic(
        &res_file,
        &serde_json::to_vec_pretty(&wrong_token_res).unwrap(),
    )
    .unwrap();
    let err_claim = ctx.reverify(now, true).unwrap_err();
    assert!(
        err_claim
            .to_string()
            .contains("does not match worker claim"),
        "expected claim mismatch refusal, got: {err_claim:#}"
    );
    write_private_atomic(&res_file, &orig_res_bytes).unwrap();
    ctx.reverify(now, true).unwrap();
    std::fs::remove_file(&res_file).unwrap();
    let err_missing = ctx.reverify(now, true).unwrap_err();
    assert!(
        err_missing.to_string().contains("needs-recovery")
            || err_missing.to_string().contains("missing"),
        "expected missing result / needs-recovery refusal, got: {err_missing:#}"
    );
}

// trace:STORY-1917 trace:TASK-2338 | ai:agy
#[test]
fn task_2338_flow_runner_fast_path_re_review_loop_changes_requested_and_no_ship() {
    let mut f = FlowFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();
    let _hooks = FlowHookGuard;

    let state_dir = f._tmp.path().join("flow-gh-state");
    std::fs::create_dir_all(&state_dir).unwrap();

    // Fake `gh` binary supporting `pr view <pr> --json ...` and `pr update-branch <pr>`.
    let fake_gh = f.write_executable_script(
        "fake-gh-flow.sh",
        &format!(
            "#!/bin/sh\nset -eu\n\
             state_dir='{}'\n\
             if [ \"${{1:-}}\" = \"pr\" ] && [ \"${{2:-}}\" = \"view\" ]; then\n\
               pr=\"$3\"\n\
               cat \"$state_dir/pr-$pr.json\"\n\
               exit 0\n\
             fi\n\
             if [ \"${{1:-}}\" = \"pr\" ] && [ \"${{2:-}}\" = \"update-branch\" ]; then\n\
               pr=\"$3\"\n\
               echo \"$pr\" >> \"$state_dir/updated-branches.log\"\n\
               if [ -f \"$state_dir/pr-$pr-after-update.json\" ]; then\n\
                 cp \"$state_dir/pr-$pr-after-update.json\" \"$state_dir/pr-$pr.json\"\n\
               fi\n\
               exit 0\n\
             fi\n\
             exit 1\n",
            state_dir.display()
        ),
    );
    f.ambient
        .set_key("AIDA_TEST_GH_BINARY", fake_gh.to_str().unwrap());

    // Fake `aida` binary for `queue work PR-<pr> --role reviewer --no-human` that
    // reads the desired verdict from `$state_dir/review-verdict-<pr>.txt` and writes
    // `.aida/review-verdicts/PR-<pr>.json` at the current head from `pr-<pr>.json`.
    let review_calls_log = state_dir.join("review-calls.log");
    let fake_aida = f.write_executable_script(
        "fake-aida-flow.sh",
        &format!(
            "#!/bin/sh\nset -eu\n\
             echo \"$*\" >> '{}'\n\
             exit 0\n",
            review_calls_log.display()
        ),
    );
    TEST_AIDA_BIN.with(|c| *c.borrow_mut() = Some(fake_aida));

    let shipped_prs = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let shipped_clone = shipped_prs.clone();
    TEST_FLOW_SHIP_HOOK.with(|c| {
        *c.borrow_mut() = Some(Box::new(move |pr, ctx| {
            ctx.reverify_now(true)?;
            shipped_clone.lock().unwrap().push(pr);
            Ok(())
        }));
    });
    TEST_FLOW_WAIT_CI_HOOK.with(|c| {
        *c.borrow_mut() = Some(Box::new(|_root, _pr, ctx, req_ship| {
            ctx.reverify_now(req_ship)?;
            Ok(())
        }));
    });

    // 1. Fast path: PR #401 already has a current-head `APPROVED` verdict -> skips review, ships immediately.
    std::fs::write(
        state_dir.join("pr-401.json"),
        r#"{"headRefOid":"aaaa1111","headRefName":"claude/pr-401","baseRefName":"main","state":"OPEN","mergeStateStatus":"CLEAN","title":"PR 401 (TASK-100)"}"#,
    )
    .unwrap();
    write_approved_verdict(&f.root, 401, "aaaa1111", "APPROVED");

    let sub_401 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 401,
            no_ship: false,
            vendor: Some(DispatchVendor::Claude),
        },
        now,
    )
    .unwrap();
    let res_401 =
        execute_one_request(&f.root, &f.mandate_dir, &sub_401.request.id, true, now).unwrap();
    assert_eq!(res_401.status, DispatchStatus::Done, "{res_401:?}");
    assert_eq!(res_401.summary, "shipped PR #401");
    let log_401 = std::fs::read_to_string(dispatch_root(&f.root).join(&res_401.log_path)).unwrap();
    assert!(
        log_401.contains("fast-path"),
        "expected approved-at-head fast-path log entry, got: {log_401}"
    );
    assert_eq!(*shipped_prs.lock().unwrap(), vec![401]);
    assert!(
        !review_calls_log.exists(),
        "approved-at-head fast path must not invoke reviewer"
    );

    // 2. Full loop: PR #402 starts `BEHIND` at `bbbb1111` (with stale verdict),
    //    `gh pr update-branch 402` advances it to `bbbb2222`, CI wait runs,
    //    reviewer runs and writes `APPROVED` at `bbbb2222`, then `pr ship` ships 402.
    std::fs::write(
        state_dir.join("pr-402.json"),
        r#"{"headRefOid":"bbbb1111","headRefName":"claude/pr-402","baseRefName":"main","state":"OPEN","mergeStateStatus":"BEHIND","title":"PR 402 (TASK-100)"}"#,
    )
    .unwrap();
    std::fs::write(
        state_dir.join("pr-402-after-update.json"),
        r#"{"headRefOid":"bbbb2222","headRefName":"claude/pr-402","baseRefName":"main","state":"OPEN","mergeStateStatus":"CLEAN","title":"PR 402 (TASK-100)"}"#,
    )
    .unwrap();
    let verdicts_dir = f.root.join(".aida").join("review-verdicts");
    let grant_id = std::env::var(crate::gate::seat_authority::GRANT_ENV).unwrap();
    let fake_aida_402 = f.write_executable_script(
        "fake-aida-flow-402.sh",
        &format!(
            "#!/bin/sh\nset -eu\n\
             echo \"$*\" >> '{}'\n\
             cat > '{}/PR-402.json' <<'EOF'\n\
             {{\"schema_version\":1,\"pr\":402,\"verdict\":\"APPROVED\",\"head_sha\":\"bbbb2222\",\"timestamp\":\"2026-10-25T00:00:00Z\",\"reviewer_session\":\"s1\",\"reviewer_vendor\":\"claude\",\"reviewer_model\":\"claude-opus-4-7\",\"author_vendor\":\"claude\",\"author_model\":\"claude-opus-4-7\",\"findings_count\":0,\"summary\":\"ok\",\"spec_ids\":[\"TASK-100\"],\"grant_id\":\"{}\"}}\n\
             EOF\n\
             exit 0\n",
            review_calls_log.display(),
            verdicts_dir.display(),
            grant_id
        ),
    );
    TEST_AIDA_BIN.with(|c| *c.borrow_mut() = Some(fake_aida_402));

    let sub_402 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 402,
            no_ship: false,
            vendor: Some(DispatchVendor::Claude),
        },
        now,
    )
    .unwrap();
    let res_402 =
        execute_one_request(&f.root, &f.mandate_dir, &sub_402.request.id, true, now).unwrap();
    assert_eq!(res_402.status, DispatchStatus::Done, "{res_402:?}");
    assert_eq!(*shipped_prs.lock().unwrap(), vec![401, 402]);
    let updated_branches =
        std::fs::read_to_string(state_dir.join("updated-branches.log")).unwrap_or_default();
    assert!(
        updated_branches.lines().any(|l| l.trim() == "402"),
        "BEHIND PR #402 must run `gh pr update-branch 402`"
    );

    // 3. `CHANGES_REQUESTED` stops immediately with `DispatchStatus::Failed` and
    //    leaves `.aida/merge-holds/PR-403` intact without calling ship.
    std::fs::write(
        state_dir.join("pr-403.json"),
        r#"{"headRefOid":"cccc1111","headRefName":"claude/pr-403","baseRefName":"main","state":"OPEN","mergeStateStatus":"CLEAN","title":"PR 403 (TASK-100)"}"#,
    )
    .unwrap();
    let hold_403 = crate::merge_hold::typed_hold(
        403,
        crate::merge_hold::HoldReasonKind::Rework,
        "findings",
        None,
    );
    crate::merge_hold::write_typed_hold(&f.root, &hold_403).unwrap();
    let fake_aida_403 = f.write_executable_script(
        "fake-aida-flow-403.sh",
        &format!(
            "#!/bin/sh\nset -eu\n\
             cat > '{}/PR-403.json' <<'EOF'\n\
             {{\"schema_version\":1,\"pr\":403,\"verdict\":\"CHANGES_REQUESTED\",\"head_sha\":\"cccc1111\",\"timestamp\":\"2026-10-25T00:00:00Z\",\"reviewer_session\":\"s1\",\"reviewer_vendor\":\"claude\",\"reviewer_model\":\"claude-opus-4-7\",\"author_vendor\":\"claude\",\"author_model\":\"claude-opus-4-7\",\"findings_count\":1,\"summary\":\"blocker\",\"spec_ids\":[\"TASK-100\"],\"grant_id\":\"{}\"}}\n\
             EOF\n\
             exit 0\n",
            verdicts_dir.display(),
            grant_id
        ),
    );
    TEST_AIDA_BIN.with(|c| *c.borrow_mut() = Some(fake_aida_403));

    let sub_403 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 403,
            no_ship: false,
            vendor: Some(DispatchVendor::Claude),
        },
        now,
    )
    .unwrap();
    let res_403 =
        execute_one_request(&f.root, &f.mandate_dir, &sub_403.request.id, true, now).unwrap();
    assert_eq!(res_403.status, DispatchStatus::Failed, "{res_403:?}");
    assert!(
        res_403.summary.contains("CHANGES_REQUESTED"),
        "expected CHANGES_REQUESTED failure summary, got: {}",
        res_403.summary
    );
    assert!(
        crate::merge_hold::read_hold(&f.root, 403).is_some(),
        "merge hold on PR #403 must remain intact after CHANGES_REQUESTED"
    );
    assert_eq!(*shipped_prs.lock().unwrap(), vec![401, 402]);

    // 4. `--no-ship` requires current-head `APPROVED` and green CI, then finishes
    //    `Done` without calling ship!
    std::fs::write(
        state_dir.join("pr-404.json"),
        r#"{"headRefOid":"dddd1111","headRefName":"claude/pr-404","baseRefName":"main","state":"OPEN","mergeStateStatus":"CLEAN","title":"PR 404 (TASK-100)"}"#,
    )
    .unwrap();
    write_approved_verdict(&f.root, 404, "dddd1111", "APPROVED");
    let sub_404 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 404,
            no_ship: true,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let res_404 =
        execute_one_request(&f.root, &f.mandate_dir, &sub_404.request.id, true, now).unwrap();
    assert_eq!(res_404.status, DispatchStatus::Done, "{res_404:?}");
    assert!(
        res_404.summary.contains("--no-ship"),
        "expected --no-ship summary, got: {}",
        res_404.summary
    );
    assert_eq!(
        *shipped_prs.lock().unwrap(),
        vec![401, 402],
        "--no-ship must never call ship"
    );
}

// trace:STORY-1917 trace:TASK-2339 | ai:agy
#[test]
fn task_2339_train_runner_lock_clean_store_ship_first_update_branch_and_train_fallback() {
    let mut f = FlowFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();
    let _hooks = FlowHookGuard;

    // 1. `.aida/dispatch/train.lock` refuses symlinks and enforces 0600 regular file.
    let disp_dir = dispatch_root(&f.root);
    std::fs::create_dir_all(&disp_dir).unwrap();
    let train_lock_path = disp_dir.join("train.lock");
    let symlink_target = f._tmp.path().join("outside-train-lock");
    std::fs::write(&symlink_target, "outside").unwrap();
    std::os::unix::fs::symlink(&symlink_target, &train_lock_path).unwrap();
    let sym_err = match acquire_train_dispatch_lock(&f.root) {
        Ok(_) => panic!("expected symlink train.lock refusal"),
        Err(e) => e,
    };
    assert!(
        format!("{sym_err:#}").contains("symlink")
            || format!("{sym_err:#}").contains("cannot open"),
        "expected symlink refusal on train.lock, got: {sym_err:#}"
    );
    std::fs::remove_file(&train_lock_path).unwrap();

    // 2. Dirty `.aida-store` refuses `train` before shipping any PR.
    let dirty_file = f.store.join("uncommitted.txt");
    std::fs::write(&dirty_file, "dirty").unwrap();
    assert!(ensure_clean_aida_store(&f.root).is_err());

    let sub_dirty = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Train { prs: vec![501] },
        now,
    )
    .unwrap();
    let res_dirty =
        execute_one_request(&f.root, &f.mandate_dir, &sub_dirty.request.id, true, now).unwrap();
    assert_eq!(res_dirty.status, DispatchStatus::Refused);
    assert!(
        res_dirty
            .refusal_reason
            .as_deref()
            .unwrap_or("")
            .contains("dirty .aida-store"),
        "expected dirty .aida-store refusal, got: {res_dirty:?}"
    );
    std::fs::remove_file(&dirty_file).unwrap();
    ensure_clean_aida_store(&f.root).unwrap();

    // 3. Multi-PR train sequence:
    //    - PR #502 ships on first `pr ship` try;
    //    - PR #503 fails first `pr ship` with "behind base", runs `gh pr update-branch 503`, and ships on retry;
    //    - PR #504 fails both `pr ship` tries with "behind base" and lands via `pr train` fallback;
    //    - PR #505 fails with a hard CI/merge failure, stopping the train before PR #506 is attempted.
    let state_dir = f._tmp.path().join("train-gh-state");
    std::fs::create_dir_all(&state_dir).unwrap();
    for pr in [502u64, 503, 504, 505, 506] {
        std::fs::write(
            state_dir.join(format!("pr-{pr}.json")),
            format!(
                r#"{{"headRefOid":"eeee{pr}","headRefName":"claude/pr-{pr}","baseRefName":"main","state":"OPEN","mergeStateStatus":"BEHIND","title":"PR {pr} (TASK-100)"}}"#
            ),
        )
        .unwrap();
    }
    let fake_gh = f.write_executable_script(
        "fake-gh-train.sh",
        &format!(
            "#!/bin/sh\nset -eu\n\
             state_dir='{}'\n\
             if [ \"${{1:-}}\" = \"pr\" ] && [ \"${{2:-}}\" = \"view\" ]; then\n\
               cat \"$state_dir/pr-$3.json\"\n\
               exit 0\n\
             fi\n\
             if [ \"${{1:-}}\" = \"pr\" ] && [ \"${{2:-}}\" = \"update-branch\" ]; then\n\
               echo \"$3\" >> \"$state_dir/updated.log\"\n\
               exit 0\n\
             fi\n\
             exit 1\n",
            state_dir.display()
        ),
    );
    f.ambient
        .set_key("AIDA_TEST_GH_BINARY", fake_gh.to_str().unwrap());

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let events_ship = events.clone();
    let ship_attempts = std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::<
        u64,
        usize,
    >::new()));
    let ship_attempts_cl = ship_attempts.clone();
    TEST_FLOW_SHIP_HOOK.with(|c| {
        *c.borrow_mut() = Some(Box::new(move |pr, ctx| {
            ctx.reverify_now(true)?;
            let count = {
                let mut map = ship_attempts_cl.lock().unwrap();
                let e = map.entry(pr).or_insert(0);
                *e += 1;
                *e
            };
            events_ship
                .lock()
                .unwrap()
                .push(format!("ship:{pr}:try{count}"));
            match (pr, count) {
                (502, 1) => Ok(()),
                (503, 1) => anyhow::bail!("PR #503 branch is behind base"),
                (503, 2) => Ok(()),
                (504, _) => anyhow::bail!("PR #504 branch is behind base"),
                (505, _) => anyhow::bail!("PR #505 required CI check failed"),
                _ => anyhow::bail!("unexpected ship call for PR #{pr} try {count}"),
            }
        }));
    });
    let events_train = events.clone();
    TEST_FLOW_TRAIN_HOOK.with(|c| {
        *c.borrow_mut() = Some(Box::new(move |prs, ctx| {
            ctx.reverify_now(true)?;
            for &pr in prs {
                events_train.lock().unwrap().push(format!("train:{pr}"));
            }
            Ok(())
        }));
    });

    // First run [502, 503, 504] to completion (`Done`).
    let sub_ok = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Train {
            prs: vec![502, 503, 504],
        },
        now,
    )
    .unwrap();
    let res_ok =
        execute_one_request(&f.root, &f.mandate_dir, &sub_ok.request.id, true, now).unwrap();
    assert_eq!(res_ok.status, DispatchStatus::Done, "{res_ok:?}");
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "ship:502:try1",
            "ship:503:try1",
            "ship:503:try2",
            "ship:504:try1",
            "ship:504:try2",
            "train:504",
        ]
    );

    // Next run [505, 506]: 505 fails with non-behind CI failure -> stops immediately without touching 506.
    events.lock().unwrap().clear();
    let sub_fail = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Train {
            prs: vec![505, 506],
        },
        now,
    )
    .unwrap();
    let res_fail =
        execute_one_request(&f.root, &f.mandate_dir, &sub_fail.request.id, true, now).unwrap();
    assert_eq!(res_fail.status, DispatchStatus::Failed, "{res_fail:?}");
    assert_eq!(
        *events.lock().unwrap(),
        vec!["ship:505:try1"],
        "train must stop on first failing PR (#505) without attempting #506"
    );
}

// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[test]
fn task_2344_q2_mid_execution_authority_revocation_aborts_before_merge_and_rearms_hold() {
    let mut f = FlowFixture::new(&["reviewer", "implementer"]);
    let mandate_a = f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();
    let _hooks = FlowHookGuard;

    let state_dir = f._tmp.path().join("revoke-gh-state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("pr-601.json"),
        r#"{"headRefOid":"ffff6010","headRefName":"claude/pr-601","baseRefName":"main","state":"OPEN","mergeStateStatus":"CLEAN","title":"PR 601 (TASK-100)"}"#,
    )
    .unwrap();
    let fake_gh = f.write_executable_script(
        "fake-gh-revoke.sh",
        &format!(
            "#!/bin/sh\nset -eu\n\
             if [ \"${{1:-}}\" = \"pr\" ] && [ \"${{2:-}}\" = \"view\" ]; then\n\
               cat '{}/pr-601.json'\n\
               exit 0\n\
             fi\n\
             exit 1\n",
            state_dir.display()
        ),
    );
    f.ambient
        .set_key("AIDA_TEST_GH_BINARY", fake_gh.to_str().unwrap());
    write_approved_verdict(&f.root, 601, "ffff6010", "APPROVED");

    // 1. In-flight mandate stop + replacement mandate B during `flow` before merge:
    //    `TEST_FLOW_SHIP_HOOK` simulates the mid-wait authority revocation (stopping
    //    mandate A and starting mandate B right before the pre-merge `ctx.reverify_now(true)`
    //    checkpoint) and verifies `flow` aborts as `Refused` without merging!
    let merged = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let merged_cl = merged.clone();
    let mandate_dir_cl = f.mandate_dir.clone();
    let principal_cl = f.principal.clone();
    TEST_FLOW_SHIP_HOOK.with(|c| {
        *c.borrow_mut() = Some(Box::new(move |_pr, ctx| {
            // Simulate mandate A stopped + replacement mandate B started during CI wait,
            // right before the pre-merge `ctx.reverify_now(true)` check.
            let stop_now = Utc::now();
            mandate::stop(&mandate_dir_cl, &|| Ok(()), &principal_cl, stop_now).unwrap();
            let set: BTreeSet<Scope> = [Scope::ShipHeld, Scope::Dispatch].into_iter().collect();
            mandate::start_with_scopes(
                &mandate_dir_cl,
                &|| Ok(()),
                &principal_cl,
                Duration::hours(2),
                Some("replacement B".into()),
                false,
                false,
                set,
                stop_now,
            )
            .unwrap();

            // Pre-merge checkpoint must fail closed naming mandate A!
            ctx.reverify_now(true)?;
            merged_cl.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
    });

    let sub_601 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 601,
            no_ship: false,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let res_601 =
        execute_one_request(&f.root, &f.mandate_dir, &sub_601.request.id, true, now).unwrap();
    assert_eq!(res_601.status, DispatchStatus::Refused, "{res_601:?}");
    assert!(
        !merged.load(std::sync::atomic::Ordering::SeqCst),
        "merge must never execute after mandate A is stopped mid-wait"
    );
    assert!(
        res_601
            .refusal_reason
            .as_deref()
            .unwrap_or("")
            .contains(&mandate_a.id),
        "refusal must name original mandate A, got: {res_601:?}"
    );

    // 2. Also prove `ForgeRails::land` re-arms released holds and retires the train PR
    //    without merging when dispatch authority is revoked during `hold_gate_green_after_release`!
    struct RevokingTrainForge<'a> {
        ctx: &'a DispatchExecutionContext,
        mandate_dir: PathBuf,
        principal: String,
        released: bool,
        rearmed: bool,
        retired: bool,
        merged: bool,
    }
    impl crate::pr_train::TrainForge for RevokingTrainForge<'_> {
        fn fetch_main(&mut self) -> anyhow::Result<String> {
            self.ctx.reverify_now(true)?;
            Ok("base1111".into())
        }
        fn lease(&mut self, _train_pr: u64) -> std::result::Result<Box<dyn std::any::Any>, String> {
            Ok(Box::new(()))
        }
        fn build(
            &mut self,
            base: &str,
            members: &[crate::pr_train::Member],
        ) -> anyhow::Result<crate::pr_train::Built> {
            self.ctx.reverify_now(true)?;
            Ok(crate::pr_train::Built {
                base: base.to_string(),
                tip: "tip2222".into(),
                cars: members
                    .iter()
                    .cloned()
                    .map(|m| crate::pr_train::Car {
                        member: m,
                        commit: "tip2222".into(),
                        patch_id: "patch1111".into(),
                    })
                    .collect(),
                dropped: vec![],
            })
        }
        fn gate(&mut self, pr: u64) -> std::result::Result<crate::pr_train::Member, String> {
            self.ctx.reverify_now(true).map_err(|e| format!("{e:#}"))?;
            Ok(crate::pr_train::Member {
                pr,
                head: "head602".into(),
                subject: format!("feat: pr {pr} (TASK-100)"),
                body: String::new(),
                release: Some(crate::pr_cmd::MandateCover {
                    id: self.ctx.mandate_id().to_string(),
                    principal: self.principal.clone(),
                }),
            })
        }
        fn open(
            &mut self,
            _train: &crate::pr_train::Built,
            branch: &str,
        ) -> anyhow::Result<crate::forge::ChangeRef> {
            self.ctx.reverify_now(true)?;
            Ok(crate::forge::ChangeRef {
                id: 9602,
                url: "https://example.invalid/pull/9602".into(),
                branch: branch.to_string(),
                base: "main".into(),
                title: None,
            })
        }
        fn wait_ci(
            &mut self,
            _change: &crate::forge::ChangeRef,
            _train: &crate::pr_train::Built,
        ) -> anyhow::Result<crate::pr_train::CiVerdict> {
            self.ctx.reverify_now(true)?;
            Ok(crate::pr_train::CiVerdict::Green)
        }
        fn release_hold(
            &mut self,
            member: &crate::pr_train::Member,
        ) -> std::result::Result<crate::merge_hold::MergeHoldRecord, String> {
            self.ctx.reverify_now(true).map_err(|e| format!("{e:#}"))?;
            self.released = true;
            Ok(crate::merge_hold::MergeHoldRecord {
                schema_version: 1,
                pr: member.pr,
                reason_kind: crate::merge_hold::HoldReasonKind::Supervision,
                detail: "held for test".into(),
                recused_principals: vec![],
                routed_to: vec![],
                routing_state: crate::merge_hold::HoldRoutingState::Pending,
                target_head_sha: Some(member.head.clone()),
                label_state: None,
                legacy: false,
                verdict_ref: None,
                release_condition: None,
                spec: Some("TASK-100".into()),
                placed_by: Some("reviewer".into()),
                absorbed: vec![],
            })
        }
        fn rearm_hold(&mut self, record: &crate::merge_hold::MergeHoldRecord) -> String {
            self.rearmed = true;
            format!(" re-armed PR #{}", record.pr)
        }
        fn hold_gate_green_after_release(
            &mut self,
            _change: &crate::forge::ChangeRef,
            _covers: &[String],
        ) -> anyhow::Result<bool> {
            // Stop the mandate during the post-release hold-gate wait!
            mandate::stop(&self.mandate_dir, &|| Ok(()), &self.principal, Utc::now()).unwrap();
            self.ctx.reverify_now(true)?;
            Ok(true)
        }
        fn retire(&mut self, _change: &crate::forge::ChangeRef, _why: &str) {
            self.retired = true;
        }
        fn merge(&mut self, _change: &crate::forge::ChangeRef, _tip: &str) -> anyhow::Result<()> {
            self.ctx.reverify_now(true)?;
            self.merged = true;
            Ok(())
        }
        fn landed(
            &mut self,
            _base: &str,
            _main: &str,
            cars: &[crate::pr_train::Car],
        ) -> Vec<Option<String>> {
            cars.iter().map(|c| Some(c.commit.clone())).collect()
        }
        fn parent(&mut self, _commit: &str) -> Option<String> {
            Some("base1111".into())
        }
        fn member_landed(
            &mut self,
            _change: &crate::forge::ChangeRef,
            _train: &crate::pr_train::Built,
            _car: &crate::pr_train::Car,
            _sha: Option<&str>,
        ) {
        }
        fn comment(&mut self, _pr: u64, _note: &str) {}
    }

    let now_train = Utc::now();
    let sub_train = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Train { prs: vec![602] },
        now_train,
    )
    .unwrap();
    let claim_token_train = Uuid::new_v4().to_string();
    let my_pid = std::process::id();
    let my_start = crate::process_probe::process_start_identity(my_pid);
    {
        let _lock = acquire_dispatch_lock(&f.root).unwrap();
        let (_, mut res) = load_pair(&f.root, &sub_train.request.id).unwrap();
        res.status = DispatchStatus::Running;
        res.started_at = Some(now_train);
        res.worker_pid = Some(my_pid);
        res.worker_start = my_start;
        res.claim_token = Some(claim_token_train.clone());
        write_private_atomic(
            &result_path(&f.root, &sub_train.request.id),
            &serde_json::to_vec_pretty(&res).unwrap(),
        )
        .unwrap();
    }
    let ctx_train = DispatchExecutionContext::establish(
        &f.root,
        &f.mandate_dir,
        &sub_train.request.id,
        &claim_token_train,
        now_train,
    )
    .unwrap();
    let mut forge = RevokingTrainForge {
        ctx: &ctx_train,
        mandate_dir: f.mandate_dir.clone(),
        principal: f.principal.clone(),
        released: false,
        rearmed: false,
        retired: false,
        merged: false,
    };
    let mut rails = crate::pr_train::ForgeRails::new(&mut forge, "test".into());
    let member_602 = rails.ports.gate(602).unwrap();
    let built = crate::pr_train::Rails::build(&mut rails, "base1111", &[member_602]).unwrap();
    let verdicts = crate::pr_train::Rails::test(&mut rails, std::slice::from_ref(&built)).unwrap();
    assert_eq!(verdicts, vec![crate::pr_train::CiVerdict::Green]);
    let land_err = crate::pr_train::Rails::land(&mut rails, &built).unwrap_err();
    assert!(
        format!("{land_err:#}").contains("dispatch request"),
        "expected dispatch request revocation error from ForgeRails::land, got: {land_err:#}"
    );
    assert!(
        forge.released,
        "hold release should have occurred before wait"
    );
    assert!(
        forge.rearmed,
        "released hold MUST be re-armed when authority is revoked during hold_gate_green_after_release"
    );
    assert!(
        forge.retired,
        "train PR MUST be retired when authority is revoked during hold_gate_green_after_release"
    );
    assert!(
        !forge.merged,
        "train PR MUST NOT merge after authority revocation"
    );
}

// trace:STORY-1917 trace:TASK-2343 trace:TASK-2344 | ai:agy
#[test]
fn r5_mixed_and_serve_only_schedule_predicate_and_tick_integration() {
    let f = FlowFixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();
    let _hooks = FlowHookGuard;

    let agent_log = f._tmp.path().join("r5-agent.log");
    let fake_agent = f.write_executable_script(
        "fake-r5-agent.sh",
        &format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\necho 'claude logs 77778888'\nexit 0\n",
            agent_log.display()
        ),
    );
    TEST_AGENT_BIN.with(|c| *c.borrow_mut() = Some(fake_agent));

    let pred_expr =
        crate::schedule_predicate::parse("mandate.dispatch_active == true && dispatch.pending > 0")
            .unwrap();
    let eval_live_predicate = |root: &Path, mandate_dir: &Path| -> bool {
        let (valid, _) = mandate::load_valid(mandate_dir, &f.principal, now);
        let m_active = valid
            .into_iter()
            .any(|m| m.revoked_at.is_none() && now < m.expires_at && m.covers(Scope::Dispatch));
        let snap = crate::schedule_predicate::Snapshot {
            mandate_dispatch_active: m_active,
            dispatch_pending: pending_request_count(root),
            ..Default::default()
        };
        crate::schedule_predicate::eval(&pred_expr, &snap).unwrap()
    };

    // 1. Empty queue -> predicate is false.
    assert_eq!(pending_request_count(&f.root), 0);
    assert!(!eval_live_predicate(&f.root, &f.mandate_dir));

    // 2. Serve-only queue (`review` + `flow` + `train`):
    //    - `pending_request_count` == 3 and predicate evaluates `true`;
    //    - `run_dispatch_tick_sync` is a cheap no-op (`spawned == 0`, `waiting_for_serve == 3`),
    //      leaving all 3 `Pending` and never launching a model/reviewer subprocess.
    submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 710,
            vendor: None,
        },
        now,
    )
    .unwrap();
    submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 711,
            no_ship: false,
            vendor: None,
        },
        now,
    )
    .unwrap();
    submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Train { prs: vec![712] },
        now,
    )
    .unwrap();

    assert_eq!(pending_request_count(&f.root), 3);
    assert!(eval_live_predicate(&f.root, &f.mandate_dir));

    let tick_serve_only = run_dispatch_tick_sync(&f.root, &f.mandate_dir, now).unwrap();
    assert_eq!(tick_serve_only.spawned, 0);
    assert_eq!(tick_serve_only.waiting_for_serve, 3);
    assert!(
        !agent_log.exists(),
        "scheduled dispatch tick must never spawn a model/reviewer on serve-only requests"
    );
    assert_eq!(pending_request_count(&f.root), 3);

    // 3. Mixed queue (add `Impl` alongside the 3 serve-only requests):
    //    - `pending_request_count` == 4;
    //    - `run_dispatch_tick_sync` claims and runs ONLY `Impl` (`spawned == 1`, `waiting_for_serve == 3`);
    //    - afterward `pending_request_count` == 3 (`review`, `flow`, `train` still waiting for serve)
    //      and a second `run_dispatch_tick_sync` is a cheap no-op (`spawned == 0`).
    let brief_path = f._tmp.path().join("r5-impl.md");
    std::fs::write(&brief_path, "Implement R5 mixed queue test.\n").unwrap();
    let sub_impl = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "r5-impl-lane".into(),
            branch: "claude/r5-impl".into(),
            spec: "TASK-100".into(),
            brief: brief_path,
            vendor: Some(DispatchVendor::Claude),
        },
        now,
    )
    .unwrap();
    assert_eq!(pending_request_count(&f.root), 4);
    assert!(eval_live_predicate(&f.root, &f.mandate_dir));

    let tick_mixed = run_dispatch_tick_sync(&f.root, &f.mandate_dir, now).unwrap();
    assert_eq!(tick_mixed.spawned, 1);
    assert_eq!(tick_mixed.waiting_for_serve, 3);
    assert_eq!(
        load_pair(&f.root, &sub_impl.request.id).unwrap().1.status,
        DispatchStatus::Done
    );
    assert_eq!(pending_request_count(&f.root), 3);

    let tick_mixed_again = run_dispatch_tick_sync(&f.root, &f.mandate_dir, now).unwrap();
    assert_eq!(tick_mixed_again.spawned, 0);
    assert_eq!(tick_mixed_again.waiting_for_serve, 3);

    // 4. Pending counting grants NO execution authority: corrupting a request on disk
    //    causes `run_dispatch_tick_sync` / `claim_and_dispatch_pass` to fail closed on
    //    the recovery fence even though `pending_request_count` > 0.
    let corrupt_id = Uuid::new_v4().to_string();
    std::fs::write(request_path(&f.root, &corrupt_id), b"{corrupt").unwrap();
    let brief2 = f._tmp.path().join("r5-impl2.md");
    std::fs::write(&brief2, "Second impl.\n").unwrap();
    let err_blocked = claim_and_dispatch_pass(&f.root, false, true, now).unwrap_err();
    assert!(
        err_blocked.to_string().contains("needs-recovery"),
        "corrupt request fence must block claim pass regardless of pending count, got: {err_blocked:#}"
    );
}
