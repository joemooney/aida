//! The real builder/open/M1/member gates/final GitHub consumer. Only gh and
//! origin's network transport are simulated; all review authority is fixture-owned.
// trace:BUG-b6b1278890774ef993d2ebc327862470-179 | ai:codex
use super::*;
use crate::forge::GitHubForge;
use std::fs;

// Every case gets its own repository, HOME, and ordinary refusal holds.
macro_rules! fixture_case {
    ($name:ident, $run:ident, $value:expr) => {
        #[test]
        fn $name() {
            $run($value);
        }
    };
}

fn g(root: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}
fn commit(root: &Path, message: &str) -> String {
    g(root, &["add", "."]);
    g(root, &["commit", "-qm", message]);
    g(root, &["rev-parse", "HEAD"])
}
fn save(root: &Path, state: &Value) {
    fs::write(
        root.join("transport.json"),
        serde_json::to_vec(state).unwrap(),
    )
    .unwrap();
}
fn calls(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("merge-calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

const GH: &str = r##"#!/usr/bin/python3
import json,sys,pathlib,subprocess
if sys.argv[1:]==['--version']:
    print('gh fixture version 1'); sys.exit(0)
r=pathlib.Path.cwd(); a=sys.argv[1:]; s=json.loads((r/'transport.json').read_text())
def emit(v): print(json.dumps(v)); sys.exit(0)
def meta(n): return s['prs'][str(n)]
with (r/'transport-calls').open('a') as f: f.write(' '.join(a)+'\n')
if a[:2]==['pr','create']:
    def opt(n): return a[a.index(n)+1]
    branch=opt('--head') if '--head' in a else opt('--branch')
    sha=subprocess.check_output(['git','rev-parse',branch],text=True).strip() if not branch.startswith('train/') else subprocess.check_output(['git','ls-remote','origin','refs/heads/'+branch],text=True).split()[0]
    s['prs']['77']=dict(number=77,url='https://github.com/fixture/train/pull/77',state='OPEN',title=opt('--title'),body=opt('--body'),headRefName=branch,headRefOid=sha,baseRefName='main',isDraft=False,isCrossRepository=False,labels=[])
    (r/'transport.json').write_text(json.dumps(s)); print(s['prs']['77']['url']); sys.exit(0)
if a[:2]==['pr','view']:
    v=meta(a[2])
    if '-q' in a or '--jq' in a:
        q=a[a.index('-q')+1] if '-q' in a else a[a.index('--jq')+1]
        if q=='.headRefOid': print(v['headRefOid']); sys.exit(0)
        if q=='.baseRefName': print(v['baseRefName']); sys.exit(0)
        if q=='.body': print(v['body']); sys.exit(0)
        if '@tsv' in q:
            if '.state' in q: print('OPEN\tMERGEABLE\tAPPROVED\t'+v['headRefOid']); sys.exit(0)
    emit(v)
if a[:2]==['pr','list']: emit([])
if a[:2]==['pr','checks']:
    if '--watch' in a: sys.exit(0)
    emit([dict(name=n,bucket='pass',state='SUCCESS',workflow='CI',link='') for n in ['Build (ubuntu-latest)','merge-hold-gate']])
if a[:2]==['pr','merge']:
    with (r/'merge-calls').open('a') as f: f.write(' '.join(a)+'\n')
    if s.get('retry'):
        s['retry']=False; s['prs']['11']['headRefOid']='f'*40
        (r/'transport.json').write_text(json.dumps(s)); print('error connecting to api.github.com',file=sys.stderr); sys.exit(1)
    sys.exit(0)
if a[0]=='api':
    endpoint=next((x for x in a[1:] if x.startswith('repos/')),a[1])
    if '/check-runs' in endpoint:
        sha=endpoint.split('/commits/')[1].split('/')[0]
        emit(dict(total_count=2,check_runs=[dict(name=n,head_sha=s.get('ci_sha',sha),status='completed',conclusion=s.get('ci','success')) for n in ['Build (ubuntu-latest)','merge-hold-gate']]))
    if endpoint.split('?',1)[0].endswith('/status'):
        sha=endpoint.split('/commits/')[1].split('/')[0]; emit(dict(sha=sha,total_count=0,statuses=[]))
    if '/pulls/' in endpoint: emit(meta(endpoint.split('/pulls/')[1].split('/')[0]))
if a[:2]==['repo','view']: emit(dict(nameWithOwner='fixture/train',defaultBranchRef=dict(name='main')))
print('unexpected gh '+repr(a),file=sys.stderr); sys.exit(2)
"##;

struct Fixture {
    dir: tempfile::TempDir,
    _origin: tempfile::TempDir,
    _real_subject: crate::ReviewPrTransportGuard,
    _env: Option<crate::test_env::EnvVarsGuard>,
    path: String,
    records: Vec<Value>,
}
impl Fixture {
    fn root(&self) -> &Path {
        self.dir.path()
    }
    /// Re-apply the fixture PATH plus `extra` (the env lock is not reentrant,
    /// so the old guard is dropped first).
    fn reenv(&mut self, extra: &[(&'static str, Option<&str>)]) {
        self._env = None;
        let mut pairs = vec![("PATH", Some(self.path.as_str()))];
        pairs.extend_from_slice(extra);
        let guard = crate::test_env::EnvVarsGuard::apply(&pairs);
        self._env = Some(guard);
    }
    fn new(bound: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let origin = tempfile::tempdir().unwrap();
        let root = dir.path();
        g(origin.path(), &["init", "--bare", "-q", "-b", "main"]);
        g(root, &["init", "-q", "-b", "main"]);
        g(root, &["config", "user.name", "Fixture author"]);
        g(root, &["config", "user.email", "author@example.invalid"]);
        g(root, &["config", "commit.gpgsign", "false"]);
        g(
            root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/fixture/train.git",
            ],
        );
        g(
            root,
            &[
                "config",
                &format!("url.{}.insteadOf", origin.path().display()),
                "https://github.com/fixture/train.git",
            ],
        );
        fs::write(
            root.join(".git/info/exclude"),
            ".aida/\n.aida-store/\ntransport*\nmerge-calls\nbin/\n",
        )
        .unwrap();
        fs::write(root.join("base"), "base\n").unwrap();
        commit(root, "base");
        g(root, &["push", "-q", "origin", "main"]);
        g(root, &["remote", "set-head", "origin", "main"]);
        let mut store = aida_core::RequirementsStore::new();
        let mut records = Vec::new();
        let mut prs = serde_json::Map::new();
        for (n, alias, canonical) in [
            (11, "BUG-2400", "BUG-b6b1278890774ef993d2ebc327862470-173"),
            (
                12,
                "STORY-2220",
                "STORY-b6b1278890774ef993d2ebc327862470-123",
            ),
        ] {
            let title = format!("[AI:codex] fix: fixture {n} ({alias})");
            let branch = format!("source/{alias}");
            g(root, &["checkout", "-qb", &branch, "main"]);
            fs::write(root.join(format!("car{n}")), format!("reviewed {n}\n")).unwrap();
            let sha = commit(root, &title);
            g(root, &["push", "-q", "origin", &branch]);
            g(root, &["checkout", "-q", "main"]);
            let mut req = aida_core::Requirement::new(title.clone(), String::new());
            req.spec_id = Some(canonical.into());
            req.agreed_id = Some(alias.into());
            req.req_type = if n == 11 {
                aida_core::RequirementType::Bug
            } else {
                aida_core::RequirementType::Story
            };
            req.status = aida_core::RequirementStatus::Approved;
            req.execution_mode = Some(aida_core::ExecutionMode::Drain);
            store.requirements.push(req);
            let identity = vec![format!("session:fixture-reviewer-{n}")];
            let receipt =
                crate::gate::review_authority::write_human_receipt(&sha, alias, &identity, false)
                    .unwrap();
            let record = json!({"verdict":"approved", "reviewed_sha":sha, "reviewed_branch":branch,
                "recorded_at":chrono::Utc::now().to_rfc3339(), "recorded_by":"fixture independent human",
                "recorder_attestation": {"sha":sha,"human_at_tty":true,"receipt_id":receipt,"identity":identity}});
            let obj = record.as_object().unwrap().clone();
            ledger::store::append_one(
                root,
                ledger::event::verdict_event(root, ledger::ChangeKey::pr(n), &obj, Some(canonical)),
            )
            .unwrap();
            if bound {
                ledger::store::append_unbound(root, alias, &obj).unwrap();
            }
            records.push(record);
            prs.insert(
                n.to_string(),
                json!({"number":n,"url":format!("https://github.com/fixture/train/pull/{n}"),
                "state":"OPEN","title":title,"body":"", "headRefName":branch,"headRefOid":sha,
                "baseRefName":"main","isDraft":false,"isCrossRepository":false,"labels":[]}),
            );
        }
        fs::create_dir_all(root.join(".aida")).unwrap();
        fs::write(
            root.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n[forge]\nprovider = \"github\"\n",
        )
        .unwrap();
        let backend = crate::test_env::home_fixture_backend(&root.join(".aida-store"))
            .unwrap()
            .with_auto_commit(false);
        aida_core::DatabaseBackend::save(&backend, &store).unwrap();
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        crate::test_exec::write_executable(&bin.join("gh"), GH);
        // Simulate only Git's external transport. Identity reads retain the
        // real pinned GitHub origin; all object/build operations use real git.
        g(
            root,
            &[
                "config",
                "--unset-all",
                &format!("url.{}.insteadOf", origin.path().display()),
            ],
        );
        crate::test_exec::write_executable(&bin.join("git"), format!(
            "#!/bin/sh\nfor arg in \"$@\"; do\ncase \"$arg\" in fetch|push|ls-remote) exec /usr/bin/git -c 'url.{}.insteadOf=https://github.com/fixture/train.git' \"$@\";; esac\ndone\nexec /usr/bin/git \"$@\"\n", origin.path().display()));

        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let env = crate::test_env::EnvVarsGuard::apply(&[("PATH", Some(&path))]);
        let state = json!({"prs":prs});
        save(root, &state);
        Self {
            dir,
            _origin: origin,
            _real_subject: crate::review_pr_transport_fixture(),
            _env: Some(env),
            path,
            records,
        }
    }
    fn live<'a>(&self, forge: &'a dyn Forge) -> LiveTrainForge<'a> {
        LiveTrainForge {
            project_root: self.root().into(),
            main_worktree: self.root().into(),
            base: "main".into(),
            forge,
            named: true,
            dispatch_ctx: None,
            dispatch_abort: None,
        }
    }
    fn opened(&self, live: &mut LiveTrainForge<'_>) -> (Built, ChangeRef) {
        self.opened_on(live, "train/bug2406-fixture")
    }
    fn opened_on(&self, live: &mut LiveTrainForge<'_>, branch: &str) -> (Built, ChangeRef) {
        let members: Vec<_> = [11, 12]
            .into_iter()
            .map(|n| live.gate(n).unwrap())
            .collect();
        let base = live.fetch_main().unwrap();
        let train = live.build(&base, &members).unwrap();
        assert_eq!(train.cars.len(), 2);
        let change = live.open(&train, branch).unwrap();
        assert_eq!(live.wait_ci(&change, &train).unwrap(), CiVerdict::Green);
        (train, change)
    }
}

fixture_case!(
    bug2406_real_builder_binding_and_production_gate_true,
    bug2406_real_builder_binding_and_production_gate,
    true
);
fixture_case!(
    bug2406_real_builder_binding_and_production_gate_false,
    bug2406_real_builder_binding_and_production_gate,
    false
);
fn bug2406_real_builder_binding_and_production_gate(bound: bool) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(bound);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let originals: Vec<_> = [11, 12]
        .into_iter()
        .map(|n| {
            fs::read(ledger::store::ledger_file(
                f.root(),
                ledger::ChangeKey::pr(n),
            ))
            .unwrap()
        })
        .collect();
    let aggregate = ledger::project::newest_verdict(f.root(), "PR-77");
    assert_eq!(
        aggregate.is_some(),
        bound,
        "open/M1 binding must remain real"
    );
    let opts = MergeOptions {
        method: MergeMethod::Rebase,
        squash_subject: None,
        squash_body: None,
        delete_branch: true,
        match_head: Some(train.tip.clone()),
    };
    if bound {
        // Current ordinary consumer demonstrates the retained failure's
        // domain on this fixture; this is not a claimed pre-fix binary run.
        assert!(forge
            .merge_change(&change, &opts, &mut crate::network_retry::NoopSink)
            .is_err());
        assert!(calls(f.root()).is_empty());
    }
    let before_train = ledger::project::events(f.root(), ledger::ChangeKey::pr(77));
    if bound {
        assert!(before_train
            .iter()
            .any(|e| matches!(e.body, ledger::EventBody::CarryOverRefused { .. })));
    }
    let lease = live.lease(change.id).unwrap();
    live.merge_verified(&change, &train, lease.as_ref())
        .unwrap();
    let after_train = ledger::project::events(f.root(), ledger::ChangeKey::pr(77));
    assert!(
        after_train.starts_with(&before_train),
        "old generic refusal remains append-only history"
    );
    let transport = calls(f.root());
    assert_eq!(transport.len(), 1);
    assert!(
        transport[0].contains(&format!("--match-head-commit {}", train.tip))
            && transport[0].contains("--rebase")
    );
    for (i, n) in [11, 12].into_iter().enumerate() {
        assert_eq!(
            fs::read(ledger::store::ledger_file(
                f.root(),
                ledger::ChangeKey::pr(n)
            ))
            .unwrap(),
            originals[i]
        );
        assert_eq!(
            ledger::project::newest_verdict(f.root(), &format!("PR-{n}")).unwrap(),
            *f.records[i].as_object().unwrap()
        );
    }
    assert!(ledger::project::events(f.root(), ledger::ChangeKey::pr(77))
        .iter()
        .any(|e| matches!(e.body, ledger::EventBody::TrainCoverage { .. })));
    assert!(ledger::project::events(f.root(), ledger::ChangeKey::pr(77))
        .iter()
        .all(|e| e.verdict_record().is_none_or(
            |v| v.get("reviewed_sha").and_then(Value::as_str) != Some(train.tip.as_str())
        )));
}

fixture_case!(
    bug2406_forged_build_refuses_patch,
    bug2406_forged_build_refuses,
    "patch"
);
fixture_case!(
    bug2406_forged_build_refuses_source,
    bug2406_forged_build_refuses,
    "source"
);
fixture_case!(
    bug2406_forged_build_refuses_order,
    bug2406_forged_build_refuses,
    "order"
);
fixture_case!(
    bug2406_forged_build_refuses_extra,
    bug2406_forged_build_refuses,
    "extra"
);
fixture_case!(
    bug2406_forged_build_refuses_tip,
    bug2406_forged_build_refuses,
    "tip"
);
fixture_case!(
    bug2406_forged_build_refuses_message,
    bug2406_forged_build_refuses,
    "message"
);
fixture_case!(
    bug2406_forged_build_refuses_base,
    bug2406_forged_build_refuses,
    "base"
);
fixture_case!(
    bug2406_forged_build_refuses_expired_release_cover,
    bug2406_forged_build_refuses,
    "expired-release-cover"
);
fn bug2406_forged_build_refuses(fault: &str) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    let mut bad = train.clone();
    match fault {
        "patch" => bad.cars[0].patch_id = "forged".into(),
        "source" => bad.cars[0].member.head = train.base.clone(),
        "order" => bad.cars.reverse(),
        "extra" => bad.cars.push(bad.cars[0].clone()),
        "tip" => bad.tip = bad.base.clone(),
        "message" => bad.cars[0].member.subject = "forged".into(),
        "base" => bad.base = bad.tip.clone(),
        "expired-release-cover" => {
            bad.cars[0].member.release = Some(crate::gate::mandate_ship::MandateCover {
                id: uuid::Uuid::new_v4().to_string(),
                principal: "fixture".into(),
            })
        }
        _ => unreachable!(),
    }
    assert!(
        live.merge_verified(&change, &bad, lease.as_ref()).is_err(),
        "{fault}"
    );
    assert!(calls(f.root()).is_empty(), "{fault}");
}

fixture_case!(
    bug2406_live_change_refuses_source_head,
    bug2406_live_change_refuses,
    "source-head"
);
fixture_case!(
    bug2406_live_change_refuses_train_head,
    bug2406_live_change_refuses,
    "train-head"
);
fixture_case!(
    bug2406_live_change_refuses_train_base,
    bug2406_live_change_refuses,
    "train-base"
);
fixture_case!(
    bug2406_live_change_refuses_train_label,
    bug2406_live_change_refuses,
    "train-label"
);
fixture_case!(
    bug2406_live_change_refuses_source_label,
    bug2406_live_change_refuses,
    "source-label"
);
fixture_case!(
    bug2406_live_change_refuses_ci,
    bug2406_live_change_refuses,
    "ci"
);
fixture_case!(
    bug2406_live_change_refuses_ci_missing,
    bug2406_live_change_refuses,
    "ci-missing"
);
fixture_case!(
    bug2406_live_change_refuses_ci_unreadable,
    bug2406_live_change_refuses,
    "ci-unreadable"
);
fixture_case!(
    bug2406_live_change_refuses_ci_wrong_head,
    bug2406_live_change_refuses,
    "ci-wrong-head"
);
fn bug2406_live_change_refuses(fault: &str) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    let mut state: Value =
        serde_json::from_slice(&fs::read(f.root().join("transport.json")).unwrap()).unwrap();
    match fault {
        "source-head" => state["prs"]["11"]["headRefOid"] = json!("f".repeat(40)),
        "train-head" => state["prs"]["77"]["headRefOid"] = json!(train.base),
        "train-base" => state["prs"]["77"]["baseRefName"] = json!("other"),
        "train-label" => state["prs"]["77"]["labels"] = json!([{"name":"aida:merge-hold"}]),
        "source-label" => state["prs"]["11"]["labels"] = json!([{"name":"aida:merge-hold"}]),
        "ci" => state["ci"] = json!("failure"),
        "ci-missing" => state["ci"] = json!("skipped"),
        "ci-unreadable" => state["ci"] = json!("unknown-result"),
        "ci-wrong-head" => state["ci_sha"] = json!(train.base),
        _ => unreachable!(),
    }
    save(f.root(), &state);
    assert!(
        live.merge_verified(&change, &train, lease.as_ref())
            .is_err(),
        "{fault}"
    );
    assert!(calls(f.root()).is_empty(), "{fault}");
}

fixture_case!(
    bug2406_current_claim_refuses_11,
    bug2406_current_claim_refuses,
    11
);
fixture_case!(
    bug2406_current_claim_refuses_77,
    bug2406_current_claim_refuses,
    77
);
fn bug2406_current_claim_refuses(pr: u64) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    let head = if pr == 77 {
        &train.tip
    } else {
        &train.cars[0].member.head
    };
    let marker = crate::gate::review_marker::Marker::for_this_process(
        pr,
        Some(head),
        None,
        "fixture reviewer",
    );
    crate::gate::review_marker::write(f.root(), &marker).unwrap();
    assert!(live
        .merge_verified(&change, &train, lease.as_ref())
        .is_err());
    assert!(calls(f.root()).is_empty());
}

fixture_case!(
    bug2406_new_refusal_refuses_bug_2400,
    bug2406_new_refusal_refuses,
    "BUG-2400"
);
fixture_case!(
    bug2406_new_refusal_refuses_bug_b6b1278890774ef993d2ebc327862470_173,
    bug2406_new_refusal_refuses,
    "BUG-B6B1278890774EF993D2EBC327862470-173"
);
fixture_case!(
    bug2406_new_refusal_refuses_pr_77,
    bug2406_new_refusal_refuses,
    "PR-77"
);
fixture_case!(
    bug2406_new_refusal_refuses_unrelated,
    bug2406_new_refusal_refuses,
    "unrelated"
);
fixture_case!(
    bug2406_new_refusal_refuses_historical_train_blocker,
    bug2406_new_refusal_refuses,
    "historical-train-blocker"
);
fn bug2406_new_refusal_refuses(key: &str) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    let mut record = f.records[0].clone();
    record["recorded_at"] = json!(chrono::Utc::now().to_rfc3339());
    record["verdict"] = json!("request-changes");
    if key == "PR-77" {
        record["reviewed_sha"] = json!(train.tip);
    }
    if key == "historical-train-blocker" {
        record["verdict"] = json!("approved");
        record["rounds"] = json!([{"verdict":"request-changes","reviewed_sha":train.tip,
            "recorded_at":chrono::Utc::now().to_rfc3339(),"recorded_by":"independent train reviewer"}]);
    }
    if key == "unrelated" {
        ledger::record_event(
            f.root(),
            ledger::event::verdict_event(
                f.root(),
                ledger::ChangeKey::pr(77),
                record.as_object().unwrap(),
                Some("UNRELATED-1"),
            ),
        )
        .unwrap();
    } else {
        let key = if key == "historical-train-blocker" {
            "BUG-B6B1278890774EF993D2EBC327862470-173"
        } else {
            key
        };
        ledger::record_verdict_at(
            &rv::verdict_path(f.root(), key),
            record.as_object().unwrap(),
            None,
        )
        .unwrap();
    }
    assert!(
        live.merge_verified(&change, &train, lease.as_ref())
            .is_err(),
        "{key}"
    );
    assert!(calls(f.root()).is_empty(), "{key}");
}

#[test]
fn bug2406_retry_refresh_refuses_changed_source_without_second_transport() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let mut state: Value =
        serde_json::from_slice(&fs::read(f.root().join("transport.json")).unwrap()).unwrap();
    state["retry"] = json!(true);
    save(f.root(), &state);
    let lease = live.lease(change.id).unwrap();
    assert!(live
        .merge_verified(&change, &train, lease.as_ref())
        .is_err());
    assert_eq!(
        calls(f.root()).len(),
        1,
        "failed transport may not retry stale source authority"
    );
}

fixture_case!(
    bug2406_sealed_context_refuses_forged_audit,
    bug2406_sealed_context_refuses,
    "forged-audit"
);
fixture_case!(
    bug2406_sealed_context_refuses_missing_audit,
    bug2406_sealed_context_refuses,
    "missing-audit"
);
fixture_case!(
    bug2406_sealed_context_refuses_linked_audit,
    bug2406_sealed_context_refuses,
    "linked-audit"
);
fixture_case!(
    bug2406_sealed_context_refuses_invalid_ledger,
    bug2406_sealed_context_refuses,
    "invalid-ledger"
);
fixture_case!(
    bug2406_sealed_context_refuses_wrong_change,
    bug2406_sealed_context_refuses,
    "wrong-change"
);
fixture_case!(
    bug2406_sealed_context_refuses_wrong_pin,
    bug2406_sealed_context_refuses,
    "wrong-pin"
);
fixture_case!(
    bug2406_sealed_context_refuses_wrong_repo,
    bug2406_sealed_context_refuses,
    "wrong-repo"
);
fixture_case!(
    bug2406_sealed_context_refuses_new_refusal,
    bug2406_sealed_context_refuses,
    "new-refusal"
);
fixture_case!(
    bug2406_sealed_context_refuses_unrelated_copy,
    bug2406_sealed_context_refuses,
    "unrelated-copy"
);
fixture_case!(
    bug2406_sealed_context_refuses_metadata_change,
    bug2406_sealed_context_refuses,
    "metadata-change"
);
fixture_case!(
    bug2406_sealed_context_refuses_main_moved,
    bug2406_sealed_context_refuses,
    "main-moved"
);
fixture_case!(
    bug2406_sealed_context_refuses_authority_lost,
    bug2406_sealed_context_refuses,
    "authority-lost"
);
fn bug2406_sealed_context_refuses(fault: &str) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    assert!(
        live.merge_verified(&change, &train, &()).is_err(),
        "a manifest alone has no lease"
    );
    let lease = crate::merge_lock::acquire(
        f.root(),
        "main",
        Some(77),
        TRAIN_LEASE_NOTE,
        std::time::Duration::ZERO,
    )
    .unwrap();
    assert!(crate::merge_lock::acquire(
        f.root(),
        "main",
        Some(77),
        TRAIN_LEASE_NOTE,
        std::time::Duration::ZERO
    )
    .is_err());
    let opts = MergeOptions {
        method: MergeMethod::Rebase,
        squash_subject: None,
        squash_body: None,
        delete_branch: true,
        match_head: Some(train.tip.clone()),
    };
    let mut context = verified(&mut live, &change, &train, &lease).unwrap();
    let path = ledger::store::ledger_file(f.root(), ledger::ChangeKey::pr(77));
    let original = fs::read(&path).unwrap();
    let mut target = change.clone();
    let mut options = opts.clone();
    let foreign = tempfile::tempdir().unwrap();
    match fault {
        "forged-audit" => {
            let body = String::from_utf8(original.clone())
                .unwrap()
                .lines()
                .map(|line| {
                    let mut event: Value = serde_json::from_str(line).unwrap();
                    if event["kind"] == "train-coverage" {
                        event["proof"]["v"] = json!(999);
                    }
                    serde_json::to_string(&event).unwrap()
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            fs::write(&path, body).unwrap();
        }
        "missing-audit" => {
            fs::remove_file(&path).unwrap();
        }
        "linked-audit" => {
            fs::remove_file(&path).unwrap();
            fs::write(foreign.path().join("ledger"), &original).unwrap();
            std::os::unix::fs::symlink(foreign.path().join("ledger"), &path).unwrap();
        }
        "invalid-ledger" => {
            fs::write(&path, "torn-refusal\n").unwrap();
        }
        "wrong-change" => target.id += 1,
        "wrong-pin" => options.match_head = Some(train.base.clone()),
        "wrong-repo" => {}
        "new-refusal" | "unrelated-copy" => {
            let mut record = f.records[0].as_object().unwrap().clone();
            let spec = if fault == "new-refusal" {
                record.insert("verdict".into(), json!("request-changes"));
                "BUG-2400"
            } else {
                "UNRELATED-1"
            };
            ledger::record_event(
                f.root(),
                ledger::event::verdict_event(
                    f.root(),
                    ledger::ChangeKey::pr(77),
                    &record,
                    Some(spec),
                ),
            )
            .unwrap();
        }
        "metadata-change" => {
            let backend = aida_core::GitBackend::new(&f.root().join(".aida-store"))
                .unwrap()
                .with_auto_commit(false);
            let mut specs = aida_core::DatabaseBackend::load(&backend).unwrap();
            specs.requirements[0]
                .tags
                .insert("fixture-policy-change".into());
            aida_core::DatabaseBackend::save(&backend, &specs).unwrap();
        }
        "main-moved" => {
            g(f.root(), &["checkout", "-q", "main"]);
            fs::write(f.root().join("main-moved"), "new main\n").unwrap();
            commit(f.root(), "main moved");
            g(f.root(), &["push", "-q", "origin", "main"]);
        }
        "authority-lost" => {
            let id = f.records[0]["recorder_attestation"]["receipt_id"]
                .as_str()
                .unwrap();
            let path = crate::gate::kernel_deps::aida_home_dir()
                .unwrap()
                .join(".aida/review-receipts")
                .join(format!("{id}.json"));
            fs::remove_file(path).unwrap();
        }
        _ => unreachable!(),
    }
    let result = if fault == "wrong-repo" {
        context.enforce(&forge, foreign.path(), &target, &options)
    } else {
        forge
            .merge_train_change(
                &target,
                &options,
                &mut crate::network_retry::NoopSink,
                &mut context,
            )
            .map(|_| ())
    };
    assert!(result.is_err(), "{fault}");
    assert!(calls(f.root()).is_empty(), "{fault}");
    // Each adversarial case owns a fresh fixture: ordinary refusal holds and
    // append-only source history remain intact until that fixture is dropped.
}

#[test]
fn bug2406_source_grant_is_validated_at_original_recording_time() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::gate::seat_authority as seats;
    let f = Fixture::new(true);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let id = seats::test_support::mint_grant_for(f.root(), "fixture-reviewer", "reviewer", &[]);
    let grant = seats::read_grant(&id).unwrap();
    assert!(seats::grant_authorized_at(
        f.root(),
        &id,
        "reviewer",
        chrono::Utc::now(),
        &grant.binding
    ));
    let mut record = f.records[0].clone();
    record["recorder_attestation"] = json!({"sha":record["reviewed_sha"],"seat":"reviewer","grant_id":id,
        "grant_binding":grant.binding,"identity":["session:fixture-reviewer"]});
    let at = record["recorded_at"]
        .as_str()
        .unwrap()
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    assert!(at < grant.issued_at);
    assert!(!seats::grant_authorized_at(
        f.root(),
        &id,
        "reviewer",
        at,
        &grant.binding
    ));
    ledger::record_event(
        f.root(),
        ledger::event::verdict_event(
            f.root(),
            ledger::ChangeKey::pr(11),
            record.as_object().unwrap(),
            Some("BUG-B6B1278890774EF993D2EBC327862470-173"),
        ),
    )
    .unwrap();
    let lease = live.lease(change.id).unwrap();
    let error = live
        .merge_verified(&change, &train, lease.as_ref())
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("authority") || format!("{error:#}").contains("re-validate"),
        "{error:#}"
    );
    assert!(calls(f.root()).is_empty());
}

fn hold_on(pr: u64, head: &str) -> crate::gate::merge_hold_core::MergeHoldRecord {
    use crate::gate::merge_hold_core as hold;
    hold::MergeHoldRecord {
        schema_version: 2,
        pr,
        reason_kind: hold::HoldReasonKind::Supervision,
        detail: format!("fixture supervision hold on PR-{pr}"),
        recused_principals: vec![],
        routed_to: vec![],
        routing_state: hold::HoldRoutingState::Pending,
        target_head_sha: Some(head.into()),
        label_state: None,
        legacy: false,
        verdict_ref: None,
        release_condition: None,
        spec: None,
        placed_by: Some("fixture reviewer".into()),
        absorbed: vec![],
    }
}

/// An active ship-held mandate in the fixture's isolated HOME.
fn plant_mandate() -> crate::gate::mandate_ship::MandateCover {
    use crate::gate::mandate;
    let principal = crate::gate::kernel_deps::current_user_id(None);
    let m = mandate::Mandate {
        id: uuid::Uuid::new_v4().to_string(),
        principal: principal.clone(),
        seat: mandate::MANDATE_SEAT.into(),
        scope: mandate::SCOPE_SHIP_HELD.into(),
        include_supervised: false,
        reason: Some("fixture fire".into()),
        issued_at: chrono::Utc::now() - chrono::Duration::minutes(1),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        revoked_at: None,
        revoked_by: None,
        scopes: Default::default(),
        kind: None,
        renewals: Vec::new(),
    };
    let dir = mandate::mandate_dir().unwrap();
    fs::create_dir_all(&dir).unwrap();
    mandate::write_private(
        &dir.join(format!("{}.json", m.id)),
        &serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    assert!(crate::gate::mandate_ship::mandate_still_active(&m.id));
    crate::gate::mandate_ship::MandateCover {
        id: m.id,
        principal,
    }
}

// BUG-2214 release-under-mandate through the verified final gate: the land
// step released the member's hold under an active mandate before merging, and
// collect() re-runs the member gate, labels and the mandate AFTER that release.
#[test]
fn bug2406_held_member_released_under_mandate_lands() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (mut train, change) = f.opened(&mut live);
    let cover = plant_mandate();
    let record = hold_on(11, &train.cars[0].member.head);
    crate::gate::merge_hold_core::place_hold(f.root(), &record).unwrap();
    assert!(crate::gate::merge_hold_core::read_hold(f.root(), 11).is_some());
    crate::gate::merge_hold_core::release_hold_under_mandate(
        f.root(),
        &record,
        &cover.principal,
        &cover.id,
        None,
    )
    .unwrap();
    assert!(crate::gate::merge_hold_core::read_hold(f.root(), 11).is_none());
    train.cars[0].member.release = Some(cover);
    let lease = live.lease(change.id).unwrap();
    live.merge_verified(&change, &train, lease.as_ref())
        .unwrap();
    let transport = calls(f.root());
    assert_eq!(transport.len(), 1);
    assert!(transport[0].contains(&format!("--match-head-commit {}", train.tip)));
    let audit = ledger::project::events(f.root(), ledger::ChangeKey::pr(77));
    assert!(audit.iter().any(|e| matches!(&e.body,
        ledger::EventBody::TrainCoverage { proof } if !proof["cars"][0]["release_cover"].is_null())));
}

fixture_case!(
    bug2406_train_gate_refuses_live_drive,
    bug2406_train_gate_refuses,
    "drive"
);
fixture_case!(
    bug2406_train_gate_refuses_train_hold,
    bug2406_train_gate_refuses,
    "train-hold"
);
fn bug2406_train_gate_refuses(fault: &str) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let mut f = Fixture::new(true);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    let expect = match fault {
        // A drive seat may not land its own pipeline (probe_ship_drive_block).
        "drive" => {
            f.reenv(&[("AIDA_HEADLESS", Some("1"))]);
            "a live drive owns the train"
        }
        "train-hold" => {
            crate::gate::merge_hold_core::place_hold(f.root(), &hold_on(77, &train.tip)).unwrap();
            "train has a merge hold"
        }
        _ => unreachable!(),
    };
    let error = live
        .merge_verified(&change, &train, lease.as_ref())
        .unwrap_err();
    assert!(format!("{error:#}").contains(expect), "{fault}: {error:#}");
    assert!(calls(f.root()).is_empty(), "{fault}");
}

// The verified final gate judges the train change's OWN ordinary hints (its
// branch/title/trailer ids) too, so it is never narrower than the standalone
// corpus gate: a refusal recorded only under the train branch's id holds it.
#[test]
fn bug2406_final_gate_includes_the_train_change_hints() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened_on(&mut live, "train/TASK-9001-fixture");
    assert!(
        crate::forge::corpus_gate_spec_hints(f.root(), &change)
            .iter()
            .any(|id| id.eq_ignore_ascii_case("TASK-9001")),
        "fixture premise: the train branch names TASK-9001"
    );
    let mut record = f.records[0].clone();
    record["recorded_at"] = json!(chrono::Utc::now().to_rfc3339());
    record["verdict"] = json!("request-changes");
    record["reviewed_sha"] = json!(train.tip);
    ledger::record_verdict_at(
        &rv::verdict_path(f.root(), "TASK-9001"),
        record.as_object().unwrap(),
        None,
    )
    .unwrap();
    let lease = live.lease(change.id).unwrap();
    assert!(live
        .merge_verified(&change, &train, lease.as_ref())
        .is_err());
    assert!(calls(f.root()).is_empty());
}

#[test]
fn bug2406_required_check_read_classifies_gh_output() {
    use crate::forge::required_checks_from_output as read;
    // gh's literal answer for a repository without required checks is an
    // empty policy, not a failed read; the exact-tip judgment still requires
    // every reported check to pass and at least one check to exist.
    assert_eq!(
        read(
            false,
            "",
            "no required checks reported on the 'train/x' branch\n"
        )
        .unwrap(),
        Vec::<String>::new()
    );
    // Every other failure stays fail-closed.
    assert!(read(false, "", "HTTP 502: Bad Gateway").is_err());
    assert!(read(false, "", "").is_err());
    assert!(read(true, "not json", "").is_err());
    assert_eq!(
        read(
            true,
            r#"[{"name":"Build","bucket":"pass","workflow":"CI","link":""}]"#,
            ""
        )
        .unwrap(),
        vec!["Build".to_string()]
    );
    let none = crate::forge::CommitChecks {
        sha: "a".repeat(40),
        checks: Vec::new(),
    };
    assert_ne!(
        crate::forge::judge_commit_checks(&none, &[]),
        crate::forge::CommitCi::Green
    );
}
