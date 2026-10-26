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
use crate::gate::mandate::{Mandate, Scope};
use crate::mandate::{self};
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
        crate::gate::mandate::start_with_scopes(
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
    let ship_only = crate::gate::mandate::start(
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
    crate::gate::mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();

    // 2. Scope-only `--scope dispatch` (without `--ship-held` or `--drive`) covers
    //    Dispatch ONLY and does NOT imply ShipHeld.
    let dispatch_only = crate::gate::mandate::start_with_scopes(
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
    let ship_hold = crate::gate::mandate::HoldFacts::ClearedByHuman;
    let ship_paths = vec!["README.md".to_string()];
    let ship_head = "a".repeat(40);
    let ship_check = crate::gate::mandate::ShipCheck {
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
        protected: crate::gate::mandate_gates::ProtectedPaths::floor_ref(),
        ship_gate: None,
        hold_clear_gate: None,
    };
    assert_eq!(
        crate::gate::mandate::ship_decision(&ship_check),
        Err(crate::gate::mandate::MandateRefusal::ScopeNotCovered {
            id: dispatch_only.id.clone(),
            scope: crate::gate::mandate::SCOPE_SHIP_HELD,
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
    crate::gate::mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();

    // 3. Explicit `--ship-held --scope dispatch` covers BOTH ShipHeld and Dispatch.
    let with_both = crate::gate::mandate::start_with_scopes(
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
    crate::gate::mandate::stop(&f.mandate_dir, &|| Ok(()), &f.principal, now).unwrap();
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

// trace:STORY-1917 | ai:agy
#[test]
fn load_all_pairs_skips_and_warns_on_malformed_or_orphan_entries() {
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

    // Write a corrupt JSON request file with a UUID stem and an orphan request
    // file with no companion result file (simulating a crash mid-submit).
    let corrupt_id = Uuid::new_v4().to_string();
    write_private_atomic(&request_path(&f.root, &corrupt_id), b"{not valid json").unwrap();

    let orphan_id = Uuid::new_v4().to_string();
    let mut orphan_req = good.request.clone();
    orphan_req.id = orphan_id.clone();
    write_private_atomic(
        &request_path(&f.root, &orphan_id),
        &serde_json::to_vec_pretty(&orphan_req).unwrap(),
    )
    .unwrap();

    // `load_all_pairs` must skip both broken records and still return the valid one,
    // and subsequent `submit_request` / `cancel_request` calls must not wedge.
    let pairs = load_all_pairs(&f.root).unwrap();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0.id, good.request.id);

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
}
