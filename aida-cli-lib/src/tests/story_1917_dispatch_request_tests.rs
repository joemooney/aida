//! STORY-1917 Slice 1 tests: typed dispatch requests (`aida dispatch`).
//!
//! Covers:
//! 1. `aida mandate start --scope dispatch` (explicit scope, not on by default).
//! 2. Valid `flow`, `review`, `take`, `impl`, and `train` requests under an
//!    active dispatch (+ `ship-held`) mandate and bound `advisor` grant,
//!    freezing `--brief` with SHA-256 and emitting a `MandateDriveAction` audit row.
//! 3. Refusals when no mandate is active, when the mandate is expired or
//!    stopped, when `Scope::Dispatch` is missing, when `HumanGates` gates
//!    `dispatch` or `ship`, or when the caller has no valid bound `advisor`
//!    grant (including a `--bind-pid` / foreign-process grant handle that
//!    names `eval "$(aida role enter advisor ...)"`).
//! 4. Dual-end validation refusals for malformed `pr`, `branch` (`..`, `.lock`,
//!    `-flag`, `main`), `name` (`../x`, `a/b`), unknown or `operator-only` /
//!    `ExecutionMode::Decide` / supervised specs, and missing/symlinked/oversized
//!    `--brief` files.
//! 5. Dedupe idempotency (`dedupe_key`) and conflicting in-flight target
//!    (`target_key`) refusal, plus `list` (`waiting for serve` for
//!    `review`/`flow`/`train`), `show`, and `cancel`.
//! 6. `.gitignore` coverage: `.aida/dispatch/` keeps `git status --porcelain`
//!    clean after a request.
//! 7. Embedded prompt templates (`dispatch/take-prompt.md` and
//!    `dispatch/impl-prompt.md`) render cleanly.
// trace:STORY-1917 | ai:agy

use super::*;
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

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: PathBuf,
    mandate_dir: PathBuf,
    principal: String,
    _ambient: crate::test_env::AmbientGuard,
}

impl Fixture {
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
        let extra: BTreeSet<Scope> = scopes.iter().copied().collect();
        mandate::start_with_scopes(
            &self.mandate_dir,
            &|| Ok(()),
            &self.principal,
            Duration::hours(4),
            Some("dispatch test".into()),
            include_supervised,
            false,
            extra,
            Utc::now(),
        )
        .unwrap()
    }

    fn write_brief(&self, name: &str, body: &str) -> PathBuf {
        let p = self._tmp.path().join(name);
        std::fs::write(&p, body).unwrap();
        p
    }
}

// trace:STORY-1917 | ai:agy
#[test]
fn mandate_start_scope_dispatch_is_explicit_and_not_default() {
    let f = Fixture::new(&["reviewer", "implementer"]);
    let now = Utc::now();

    // 1. Default `start` (--ship-held only) covers ShipHeld and NOT Dispatch.
    let ship_only = mandate::start(
        &f.mandate_dir,
        &|| Ok(()),
        &f.principal,
        Duration::hours(2),
        None,
        false,
        now,
    )
    .unwrap();
    assert!(ship_only.covers(Scope::ShipHeld));
    assert!(
        !ship_only.covers(Scope::Dispatch),
        "Scope::Dispatch must stay explicit and not be enabled by default on --ship-held"
    );
    mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();

    // 2. Scope-only `--scope dispatch` (without `--ship-held` or `--drive`) covers
    //    Dispatch ONLY and does NOT imply ShipHeld.
    let dispatch_only = mandate::start_with_scopes(
        &f.mandate_dir,
        &|| Ok(()),
        &f.principal,
        Duration::hours(2),
        None,
        false,
        false,
        [Scope::Dispatch].into_iter().collect(),
        now,
    )
    .unwrap();
    assert!(dispatch_only.covers(Scope::Dispatch));
    assert!(
        !dispatch_only.covers(Scope::ShipHeld),
        "`aida mandate start --scope dispatch` without --ship-held must not cover ship-held"
    );
    assert!(!dispatch_only.is_drive());

    // `mandate::ship_decision` must also refuse a `--scope dispatch`-only mandate with ScopeNotCovered.
    let ship_state = MandateState::Active(dispatch_only.clone());
    let ship_policy = Default::default();
    let ship_ledger = Ok(Default::default());
    let ship_hold = mandate::HoldFacts::ClearedByHuman;
    let ship_paths = vec!["README.md".to_string()];
    let ship_head = "a".repeat(40);
    let ship_check = mandate::ShipCheck {
        state: &ship_state,
        grant: Some(("advisor", &f.principal)),
        now,
        override_stale_approval: false,
        hold: &ship_hold,
        specs_resolved: true,
        specs: &[],
        ledger: &ship_ledger,
        candidates: &[],
        policy: &ship_policy,
        head: Some(ship_head.as_str()),
        carry_over: None,
        changed_paths: Some(&ship_paths),
        protected: crate::mandate_gates::ProtectedPaths::floor_ref(),
        ship_gate: None,
        hold_clear_gate: None,
    };
    assert_eq!(
        mandate::ship_decision(&ship_check),
        Err(mandate::MandateRefusal::ScopeNotCovered {
            id: dispatch_only.id.clone(),
            scope: mandate::SCOPE_SHIP_HELD,
        })
    );

    // Under a dispatch-only mandate, non-shipping verbs (`review`, `flow --no-ship`) succeed,
    // while shipping verbs (`flow` without `--no-ship`, `train`) refuse with `ship-held`.
    let no_ship_flow = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 101,
            no_ship: true,
            vendor: None,
        },
        now,
    )
    .unwrap();
    assert_eq!(
        no_ship_flow.request.action,
        DispatchVerb::Flow {
            pr: 101,
            no_ship: true,
            vendor: None,
        }
    );
    let ship_flow_err = submit_request(
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
    .unwrap_err();
    assert!(
        ship_flow_err
            .to_string()
            .contains("does not cover `ship-held`"),
        "expected ship-held refusal for shipping flow, got: {ship_flow_err:#}"
    );
    let train_err = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Train { prs: vec![102] },
        now,
    )
    .unwrap_err();
    assert!(
        train_err.to_string().contains("does not cover `ship-held`"),
        "expected ship-held refusal for train, got: {train_err:#}"
    );
    mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();

    // 3. Explicit `--ship-held --scope dispatch` covers BOTH ShipHeld and Dispatch.
    let with_both = mandate::start_with_scopes(
        &f.mandate_dir,
        &|| Ok(()),
        &f.principal,
        Duration::hours(2),
        None,
        false,
        false,
        [Scope::ShipHeld, Scope::Dispatch].into_iter().collect(),
        now,
    )
    .unwrap();
    assert!(with_both.covers(Scope::ShipHeld));
    assert!(with_both.covers(Scope::Dispatch));
    assert!(!with_both.is_drive());
}

// trace:STORY-1917 | ai:agy
#[test]
fn valid_requests_succeed_freeze_brief_and_keep_git_status_clean() {
    let f = Fixture::new(&["reviewer", "implementer"]);
    let m = f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    // 1. flow
    let flow = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 2886,
            no_ship: false,
            vendor: Some(DispatchVendor::Claude),
        },
        now,
    )
    .unwrap();
    assert!(!flow.deduplicated);
    assert_eq!(flow.request.mandate_id, m.id);
    assert!(!flow.request.matched_binding.is_empty());
    assert_eq!(
        display_status(&flow.request, &flow.result),
        "pending (waiting for serve)"
    );

    // 2. review (different PR)
    let rev = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 2887,
            vendor: Some(DispatchVendor::Codex),
        },
        now,
    )
    .unwrap();
    assert_eq!(
        display_status(&rev.request, &rev.result),
        "pending (waiting for serve)"
    );

    // 3. take (freezes brief with sha256)
    let brief_file = f.write_brief("take.md", "Fix the failing test in mandate_ship.rs.\n");
    let take = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Take {
            pr: 2888,
            branch: "claude/task-100".into(),
            spec: "TASK-100".into(),
            brief: brief_file,
            vendor: Some(DispatchVendor::Agy),
        },
        now,
    )
    .unwrap();
    assert_eq!(display_status(&take.request, &take.result), "pending");
    if let DispatchVerb::Take {
        ref brief_relpath,
        ref brief_sha256,
        ..
    } = take.request.action
    {
        let body =
            verify_frozen_brief(&f.root, &take.request.id, brief_relpath, brief_sha256).unwrap();
        assert_eq!(body, "Fix the failing test in mandate_ship.rs.\n");
        let prompt = render_lane_prompt(
            &f.root,
            &f.root.join("../worktrees/take-2888"),
            &take.request.action,
            &body,
            now,
        )
        .unwrap();
        assert!(prompt.contains("PR #2888 (TASK-100)"));
        assert!(prompt.contains("Fix the failing test in mandate_ship.rs."));
    } else {
        panic!("expected Take action");
    }

    // 4. impl
    let impl_brief = f.write_brief("impl.md", "Implement slice 1 of STORY-1917.\n");
    let imp = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "story-1917-s1".into(),
            branch: "claude/story-1917-s1".into(),
            spec: "STORY-1917".into(),
            brief: impl_brief,
            vendor: None,
        },
        now,
    )
    .unwrap();
    assert_eq!(display_status(&imp.request, &imp.result), "pending");

    // 5. train
    let train = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Train {
            prs: vec![2890, 2891, 2890],
        },
        now,
    )
    .unwrap();
    assert_eq!(
        train.request.action,
        DispatchVerb::Train {
            prs: vec![2890, 2891]
        }
    );
    assert_eq!(
        display_status(&train.request, &train.result),
        "pending (waiting for serve)"
    );

    // 6. Audit rows emitted in .aida/events.jsonl
    let rows = crate::mandate_drive::ledger_rows(&f.root, &m.id);
    assert_eq!(rows.len(), 5, "each queued request emits one audit row");

    // 7. .gitignore check: git status --porcelain stays clean after requests
    let status_out = crate::test_git::git_command()
        .arg("-C")
        .arg(&f.root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    let dirty = String::from_utf8(status_out.stdout).unwrap();
    assert!(
        dirty.trim().is_empty(),
        ".aida/dispatch/ and events must stay gitignored, got dirty status:\n{dirty}"
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn refuses_without_active_dispatch_mandate_or_bound_advisor_grant() {
    let f = Fixture::new(&["reviewer", "implementer"]);
    let now = Utc::now();
    let verb = DispatchRequestVerb::Review {
        pr: 42,
        vendor: None,
    };

    // 1. No mandate active
    let err = submit_request(&f.root, &f.mandate_dir, &f.principal, &verb, now).unwrap_err();
    assert!(
        err.to_string().contains("no active mandate"),
        "unexpected error: {err:#}"
    );

    // 2. Mandate without Scope::Dispatch
    f.start_mandate(&[Scope::ShipHeld], false);
    let err = submit_request(&f.root, &f.mandate_dir, &f.principal, &verb, now).unwrap_err();
    assert!(
        err.to_string().contains("does not cover `dispatch`"),
        "unexpected error: {err:#}"
    );

    // 3. Stopped mandate
    mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();
    let err = submit_request(&f.root, &f.mandate_dir, &f.principal, &verb, now).unwrap_err();
    assert!(
        err.to_string().contains("was stopped"),
        "unexpected error: {err:#}"
    );

    // 4. Start a valid dispatch mandate, then test unbound / foreign grant refusal (Q1 ruling)
    f.start_mandate(&[Scope::Dispatch], false);
    let grant_id = std::env::var(crate::gate::seat_authority::GRANT_ENV).unwrap();
    let mut grant_rec = crate::gate::seat_authority::read_grant(&grant_id).unwrap();
    grant_rec.binding = vec!["proc:999999@linux-starttime:1".to_string()];
    let grant_file = crate::gate::seat_authority::grant_dir()
        .unwrap()
        .join(format!("{grant_id}.json"));
    write_private_atomic(&grant_file, &serde_json::to_vec_pretty(&grant_rec).unwrap()).unwrap();
    let err = submit_request(&f.root, &f.mandate_dir, &f.principal, &verb, now).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\""),
        "refusal for unbound/foreign-namespace grant must name the exact role enter fix, got: {msg}"
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn refuses_malformed_arguments_and_operator_only_or_tampered_briefs() {
    let f = Fixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::Dispatch], false);
    let now = Utc::now();
    let good_brief = f.write_brief("good.md", "Valid brief content.\n");

    // 1. Invalid PR numbers
    for bad_pr in [0, 1_000_000] {
        assert!(submit_request(
            &f.root,
            &f.mandate_dir,
            &f.principal,
            &DispatchRequestVerb::Review {
                pr: bad_pr,
                vendor: None,
            },
            now,
        )
        .is_err());
    }

    // 2. Invalid branch names
    for bad_branch in [
        "main",
        "master",
        "-flag",
        "a..b",
        "foo.lock",
        "/leading",
        "trailing/",
        "bad space",
    ] {
        assert!(
            submit_request(
                &f.root,
                &f.mandate_dir,
                &f.principal,
                &DispatchRequestVerb::Take {
                    pr: 10,
                    branch: bad_branch.into(),
                    spec: "TASK-100".into(),
                    brief: good_brief.clone(),
                    vendor: None,
                },
                now,
            )
            .is_err(),
            "branch `{bad_branch}` must be refused"
        );
    }

    // 3. Invalid lane names
    for bad_name in ["../escape", "a/b", "-dash", ".dot", ""] {
        assert!(
            submit_request(
                &f.root,
                &f.mandate_dir,
                &f.principal,
                &DispatchRequestVerb::Impl {
                    name: bad_name.into(),
                    branch: "claude/ok".into(),
                    spec: "TASK-100".into(),
                    brief: good_brief.clone(),
                    vendor: None,
                },
                now,
            )
            .is_err(),
            "lane name `{bad_name}` must be refused"
        );
    }

    // 4. Unknown spec, operator-only spec, decide-mode spec, and supervised spec
    assert!(submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "lane1".into(),
            branch: "claude/lane1".into(),
            spec: "TASK-9999".into(),
            brief: good_brief.clone(),
            vendor: None,
        },
        now,
    )
    .is_err());

    let mut op_req = plain_req("TASK-200");
    op_req.tags.insert("operator-only".into());
    write_spec(&f.store, "TASK-200", &op_req);
    let err = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "lane2".into(),
            branch: "claude/lane2".into(),
            spec: "TASK-200".into(),
            brief: good_brief.clone(),
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(err.to_string().contains("operator-only"), "{err:#}");

    let mut sec_req = plain_req("TASK-201");
    sec_req.tags.insert("security".into());
    write_spec(&f.store, "TASK-201", &sec_req);
    let err = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "lane3".into(),
            branch: "claude/lane3".into(),
            spec: "TASK-201".into(),
            brief: good_brief.clone(),
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(err.to_string().contains("--include-supervised"), "{err:#}");

    // 5. Symlinked, empty, or oversized brief
    let empty_brief = f.write_brief("empty.md", "   \n");
    assert!(freeze_brief(&f.root, &Uuid::new_v4().to_string(), &empty_brief).is_err());

    let big_brief = f.write_brief("big.md", &"x".repeat(MAX_BRIEF_BYTES + 1));
    assert!(freeze_brief(&f.root, &Uuid::new_v4().to_string(), &big_brief).is_err());

    let link_brief = f._tmp.path().join("symlink.md");
    std::os::unix::fs::symlink(&good_brief, &link_brief).unwrap();
    let err = freeze_brief(&f.root, &Uuid::new_v4().to_string(), &link_brief).unwrap_err();
    assert!(err.to_string().contains("symlink"), "{err:#}");
}

// trace:STORY-1917 | ai:agy
#[test]
fn dedupe_idempotency_target_collision_and_cancel_lifecycle() {
    let f = Fixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    // First flow request on PR 500
    let first = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 500,
            no_ship: false,
            vendor: None,
        },
        now,
    )
    .unwrap();
    assert!(!first.deduplicated);

    // Exact duplicate returns the existing request with deduplicated = true
    let dup = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 500,
            no_ship: false,
            vendor: None,
        },
        now,
    )
    .unwrap();
    assert!(dup.deduplicated);
    assert_eq!(dup.request.id, first.request.id);

    // Conflicting action on the same target (PR 500) is refused while Pending
    let brief = f.write_brief("b.md", "Takeover PR 500.\n");
    let conflict = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Take {
            pr: 500,
            branch: "claude/pr-500".into(),
            spec: "TASK-100".into(),
            brief: brief.clone(),
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(
        conflict
            .to_string()
            .contains("already has an active dispatch request"),
        "{conflict:#}"
    );

    // Cancel the first request by prefix; then the takeover request on PR 500 succeeds!
    let prefix: String = first.request.id.chars().take(8).collect();
    let cancelled = cancel_request(&f.root, &prefix, now).unwrap();
    assert_eq!(cancelled.status, DispatchStatus::Cancelled);

    let after_cancel = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Take {
            pr: 500,
            branch: "claude/pr-500".into(),
            spec: "TASK-100".into(),
            brief,
            vendor: None,
        },
        now,
    )
    .unwrap();
    assert!(!after_cancel.deduplicated);
    assert_ne!(after_cancel.request.id, first.request.id);
}

// trace:STORY-1917 | ai:agy
#[test]
fn human_gates_gate_dispatch_and_ship_with_gated_audit_rows() {
    let f = Fixture::new(&["reviewer", "implementer"]);
    let m = f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    // 1. Gate `ship` in origin/main:.aida/config.toml -> `review` and `flow --no-ship`
    //    succeed, while shipping `flow` and `train` refuse with Outcome::Gated.
    std::fs::write(
        f.root.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n\n[mandate.human_gates]\nops = [\"ship\"]\n",
    )
    .unwrap();
    git(&f.root, &["add", ".aida/config.toml"]);
    git(&f.root, &["commit", "-qm", "gate ship"]);
    let head = crate::test_git::git_command()
        .arg("-C")
        .arg(&f.root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8(head.stdout).unwrap().trim().to_string();
    git(&f.root, &["update-ref", "refs/remotes/origin/main", &sha]);

    let no_ship = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 601,
            no_ship: true,
            vendor: None,
        },
        now,
    )
    .unwrap();
    assert_eq!(no_ship.result.status, DispatchStatus::Pending);

    let ship_err = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 602,
            no_ship: false,
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(
        ship_err
            .to_string()
            .contains("shipping is gated by human gates"),
        "expected GateOp::Ship refusal, got: {ship_err:#}"
    );

    // 2. Gate `dispatch` in origin/main:.aida/config.toml -> even `review` refuses with Outcome::Gated.
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

    let disp_err = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 603,
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(
        disp_err
            .to_string()
            .contains("dispatch is gated by human gates"),
        "expected GateOp::Dispatch refusal, got: {disp_err:#}"
    );

    let rows = crate::mandate_drive::ledger_rows(&f.root, &m.id);
    let gated_count = rows
        .iter()
        .filter(|r| r.2.outcome == Outcome::Gated)
        .count();
    assert_eq!(
        gated_count, 2,
        "both GateOp::Ship and GateOp::Dispatch refusals must emit Outcome::Gated audit rows"
    );
}

// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[test]
fn load_all_pairs_and_recovery_fence_handle_orphan_and_corrupt_entries() {
    let f = Fixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();

    let good = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 700,
            vendor: None,
        },
        now,
    )
    .unwrap();

    // 1. Write an orphan request file for PR #705 with no companion result file.
    let orphan_id = Uuid::new_v4().to_string();
    let mut orphan_req = good.request.clone();
    orphan_req.id = orphan_id.clone();
    orphan_req.action = DispatchVerb::Review {
        pr: 705,
        vendor: None,
    };
    orphan_req.target_key = orphan_req.action.target_key();
    orphan_req.dedupe_key = orphan_req.action.dedupe_key();
    write_private_atomic(
        &request_path(&f.root, &orphan_id),
        &serde_json::to_vec_pretty(&orphan_req).unwrap(),
    )
    .unwrap();

    // Read-only `load_all_pairs` returns only valid pairs without mutating disk,
    // while `inspect_dispatch_inventory_readonly` surfaces `orphan_id` as `needs-recovery`.
    let pairs = load_all_pairs(&f.root).unwrap();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0.id, good.request.id);
    let inv = inspect_dispatch_inventory_readonly(&f.root).unwrap();
    assert_eq!(inv.fences.len(), 1);
    assert_eq!(inv.fences[0].id, orphan_id);
    assert!(!inv.fences[0].blocks_all_admission);

    // Submitting a second request for the orphan's target (`pr:705`) is refused
    // naming the `needs-recovery` record, while an unrelated target (`pr:701`)
    // is admitted and can be cancelled.
    let clash_err = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 705,
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(
        clash_err.to_string().contains(&orphan_id)
            && clash_err.to_string().contains("needs-recovery"),
        "expected needs-recovery target refusal for pr:705, got: {clash_err:#}"
    );

    let second = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 701,
            vendor: None,
        },
        now,
    )
    .unwrap();
    let cancelled = cancel_request(&f.root, &second.request.id, now).unwrap();
    assert_eq!(cancelled.status, DispatchStatus::Cancelled);

    // 2. A corrupt request file with an unknown target/verb sets `blocks_all_admission`
    //    and causes new `submit_request` admissions to fail closed.
    let corrupt_id = Uuid::new_v4().to_string();
    write_private_atomic(&request_path(&f.root, &corrupt_id), b"{not valid json").unwrap();
    let blocked_err = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 702,
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(
        blocked_err.to_string().contains(&corrupt_id)
            && blocked_err.to_string().contains("needs-recovery"),
        "corrupt request with unknown target must fail closed on new admission, got: {blocked_err:#}"
    );
}

// trace:STORY-1917 trace:TASK-2340 | ai:agy
#[test]
fn task_2340_scope_refusals_emit_audit_event_only_for_active_mandates() {
    let f = Fixture::new(&["reviewer", "implementer"]);
    let now = Utc::now();

    // 1. Active mandate lacking `Scope::Dispatch` emits exactly one `Outcome::Refused`
    //    row under its exact `mandate.id`.
    let m_ship_only = f.start_mandate(&[Scope::ShipHeld], false);
    let err1 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 801,
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(err1.to_string().contains("does not cover `dispatch`"));
    let rows1 = crate::mandate_drive::ledger_rows(&f.root, &m_ship_only.id);
    assert_eq!(
        rows1.len(),
        1,
        "missing Scope::Dispatch must emit 1 audit row"
    );
    assert_eq!(rows1[0].2.outcome, Outcome::Refused);
    assert_eq!(rows1[0].2.action, "review");
    assert_eq!(rows1[0].2.subject, "PR #801");

    // 2. Active mandate covering `Scope::Dispatch` but lacking `Scope::ShipHeld`
    //    emits one `Outcome::Refused` row for shipping `flow` and one for `train`.
    mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();
    let m_dispatch_only = f.start_mandate(&[Scope::Dispatch], false);
    let err_flow = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Flow {
            pr: 802,
            no_ship: false,
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(err_flow.to_string().contains("does not cover `ship-held`"));
    let err_train = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Train {
            prs: vec![802, 803],
        },
        now,
    )
    .unwrap_err();
    assert!(err_train.to_string().contains("does not cover `ship-held`"));
    let rows2 = crate::mandate_drive::ledger_rows(&f.root, &m_dispatch_only.id);
    assert_eq!(rows2.len(), 2);
    assert!(rows2.iter().all(|r| r.2.outcome == Outcome::Refused));

    // 3. Stopped mandate must NOT emit a new audit row borrowing the stopped ID.
    mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();
    let _ = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 804,
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    let rows_after_stop = crate::mandate_drive::ledger_rows(&f.root, &m_dispatch_only.id);
    assert_eq!(
        rows_after_stop.len(),
        2,
        "Stopped mandate refusal must not append ledger rows under the stopped mandate ID"
    );
}

// trace:STORY-1917 trace:TASK-2341 | ai:agy
#[test]
fn task_2341_validate_branch_rejects_verified_local_default_branch_and_missing_origin_head() {
    let f = Fixture::new(&["reviewer", "implementer"]);
    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    let now = Utc::now();
    let brief = f.write_brief("trunk.md", "Test default branch check.\n");

    let head = crate::test_git::git_command()
        .arg("-C")
        .arg(&f.root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8(head.stdout).unwrap().trim().to_string();

    // 1. Point `refs/remotes/origin/HEAD` to `refs/remotes/origin/trunk`:
    //    `trunk` must be rejected while `claude/feature-trunk` succeeds.
    git(&f.root, &["update-ref", "refs/remotes/origin/trunk", &sha]);
    git(
        &f.root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );
    let trunk_err = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "trunk-lane".into(),
            branch: "trunk".into(),
            spec: "TASK-100".into(),
            brief: brief.clone(),
            vendor: None,
        },
        now,
    )
    .unwrap_err();
    assert!(
        trunk_err
            .to_string()
            .contains("cannot target the repository default branch `trunk` directly"),
        "expected default branch `trunk` refusal, got: {trunk_err:#}"
    );

    let ok_sub = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "feature-lane".into(),
            branch: "claude/feature-trunk".into(),
            spec: "TASK-100".into(),
            brief: brief.clone(),
            vendor: None,
        },
        now,
    )
    .unwrap();
    assert_eq!(ok_sub.result.status, DispatchStatus::Pending);

    // 2. Dangling `refs/remotes/origin/HEAD` -> fails closed.
    git(
        &f.root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/nonexistent",
        ],
    );
    let dangling_err = validate_branch(&f.root, "claude/other").unwrap_err();
    assert!(
        dangling_err
            .to_string()
            .contains("is missing, dangling, or does not resolve to a commit"),
        "expected dangling default branch refusal, got: {dangling_err:#}"
    );

    // 3. Non-origin target (`refs/heads/main`) -> fails closed.
    git(
        &f.root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/heads/main",
        ],
    );
    let non_origin_err = validate_branch(&f.root, "claude/other").unwrap_err();
    assert!(
        non_origin_err
            .to_string()
            .contains("points to non-origin target"),
        "expected non-origin symref refusal, got: {non_origin_err:#}"
    );

    // 4. Missing `refs/remotes/origin/HEAD` -> fails closed without falling back to `main`.
    git(
        &f.root,
        &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"],
    );
    let missing_err = validate_branch(&f.root, "claude/other").unwrap_err();
    assert!(
        missing_err
            .to_string()
            .contains("symbolic-ref is missing or unreadable"),
        "expected missing origin/HEAD refusal, got: {missing_err:#}"
    );
}

// trace:STORY-1917 trace:TASK-2342 | ai:agy
#[test]
fn task_2342_mandate_status_line_renders_all_effective_scopes_in_canonical_order() {
    let f = Fixture::new(&["reviewer", "implementer"]);
    let now = Utc::now();

    // 1. Multi-scope non-drive mandate renders all scopes in canonical order (`ship-held, dispatch`).
    let m = f.start_mandate(&[Scope::Dispatch, Scope::ShipHeld], false);
    let line = mandate::current_status_line(&f.mandate_dir, Some(&f.root), now).unwrap();
    assert!(
        line.starts_with("MANDATE ship-held, dispatch ·"),
        "expected `MANDATE ship-held, dispatch · ...`, got: {line}"
    );
    let _ = m;

    // 2. Single-scope `dispatch` renders `MANDATE dispatch · ...`.
    mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();
    f.start_mandate(&[Scope::Dispatch], false);
    let line_single = mandate::current_status_line(&f.mandate_dir, Some(&f.root), now).unwrap();
    assert!(
        line_single.starts_with("MANDATE dispatch ·"),
        "expected `MANDATE dispatch · ...`, got: {line_single}"
    );

    // 3. Drive mandate retains `MANDATE drive · ...` shorthand.
    mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();
    let drive_scopes: BTreeSet<Scope> = [Scope::ShipHeld, Scope::Dispatch].into_iter().collect();
    mandate::start_with_scopes(
        &f.mandate_dir,
        &|| Ok(()),
        &f.principal,
        Duration::hours(2),
        None,
        false,
        true,
        drive_scopes,
        now,
    )
    .unwrap();
    let line_drive = mandate::current_status_line(&f.mandate_dir, Some(&f.root), now).unwrap();
    assert!(
        line_drive.starts_with("MANDATE drive ·"),
        "expected `MANDATE drive · ...`, got: {line_drive}"
    );
}

// trace:STORY-1917 | ai:agy
#[test]
fn schedule_predicates_and_result_first_submit_rollbacks() {
    let f = Fixture::new(&["reviewer", "implementer"]);
    let now = Utc::now();

    // Predicates parse and evaluate cleanly.
    let expr =
        crate::schedule_predicate::parse("mandate.dispatch_active == true && dispatch.pending > 0")
            .unwrap();
    let mut snap = crate::schedule_predicate::Snapshot::default();
    assert!(!crate::schedule_predicate::eval(&expr, &snap).unwrap());
    snap.mandate_dispatch_active = true;
    snap.dispatch_pending = 2;
    assert!(crate::schedule_predicate::eval(&expr, &snap).unwrap());

    f.start_mandate(&[Scope::ShipHeld, Scope::Dispatch], false);
    assert_eq!(pending_request_count(&f.root), 0);

    // Stage 1 and Stage 2 in-invocation submit failures clean up exact owned files.
    let brief = f.write_brief("stage.md", "Stage rollback test.\n");
    TEST_FAIL_SUBMIT_STAGE.with(|c| c.set(1));
    let e1 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "stage-one".into(),
            branch: "claude/stage-one".into(),
            spec: "TASK-100".into(),
            brief: brief.clone(),
            vendor: None,
        },
        now,
    );
    TEST_FAIL_SUBMIT_STAGE.with(|c| c.set(0));
    assert!(e1.is_err());
    assert_eq!(
        inspect_dispatch_inventory_readonly(&f.root)
            .unwrap()
            .fences
            .len(),
        0,
        "stage-1 failure must clean up frozen brief and leave zero recovery fences"
    );

    TEST_FAIL_SUBMIT_STAGE.with(|c| c.set(2));
    let e2 = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Impl {
            name: "stage-two".into(),
            branch: "claude/stage-two".into(),
            spec: "TASK-100".into(),
            brief: brief.clone(),
            vendor: None,
        },
        now,
    );
    TEST_FAIL_SUBMIT_STAGE.with(|c| c.set(0));
    assert!(e2.is_err());
    assert_eq!(
        inspect_dispatch_inventory_readonly(&f.root)
            .unwrap()
            .fences
            .len(),
        0,
        "same-invocation stage-2 failure must clean up Pending result + frozen brief"
    );

    // Simulated hard crash after result write leaves orphan Pending result on disk,
    // which the next inspection classifies as `needs-recovery` (`RequestMissing`)
    // without deleting it.
    TEST_SIMULATE_CRASH_AFTER_RESULT_WRITE.with(|c| c.set(true));
    let crash_res = submit_request(
        &f.root,
        &f.mandate_dir,
        &f.principal,
        &DispatchRequestVerb::Review {
            pr: 990,
            vendor: None,
        },
        now,
    );
    TEST_SIMULATE_CRASH_AFTER_RESULT_WRITE.with(|c| c.set(false));
    assert!(crash_res.is_err());
    let inv_after_crash = inspect_dispatch_inventory_readonly(&f.root).unwrap();
    assert_eq!(inv_after_crash.fences.len(), 1);
    assert_eq!(
        inv_after_crash.fences[0].known_target_key.as_deref(),
        Some("pr:990")
    );
    assert!(
        result_path(&f.root, &inv_after_crash.fences[0].id).exists(),
        "orphan Pending result after hard crash must never be auto-deleted on restart"
    );
}
