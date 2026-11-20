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
def meta(n):
    # trace:STORY-2903 | ai:claude — r4: the grant is revoked during a
    # chosen metadata read of PR `revoke_on` (after `revoke_on_skip` reads).
    if s.get('revoke_on')==str(n):
        s['revoke_on_reads']=s.get('revoke_on_reads',0)+1
        (r/'transport.json').write_text(json.dumps(s))
    if s.get('revoke_on')==str(n) and s.get('revoke_dir'):
        if s.get('revoke_on_skip',0)>0: s['revoke_on_skip']-=1
        else:
            for g in pathlib.Path(s.pop('revoke_dir')).glob('*.json'): g.unlink()
        (r/'transport.json').write_text(json.dumps(s))
    return s['prs'][str(n)]
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
if a[:2]==['pr','list']:
    if '--state' in a and a[a.index('--state')+1]=='open':
        for n in ['11','12']:
            v=meta(n)
            print(str(v['number'])+'\t'+v['title']+'\t'+v['url']+'\t'+v['headRefName'])
    sys.exit(0)
if a[:2]==['pr','checks']:
    if '--watch' in a: sys.exit(0)
    emit([dict(name=n,bucket='pass',state='SUCCESS',workflow='CI',link='') for n in ['Build (ubuntu-latest)','merge-hold-gate']])
if a[:2]==['pr','edit'] and s.get('land_for_real'):
    v=meta(a[2])
    if '--body' in a: v['body']=a[a.index('--body')+1]
    for i, flag in enumerate(a[:-1]):
        labels=a[i+1].split(',')
        if flag=='--add-label': v['labels'].extend(dict(name=label) for label in labels)
        if flag=='--remove-label': v['labels']=[l for l in v['labels'] if l['name'] not in labels]
    (r/'transport.json').write_text(json.dumps(s)); sys.exit(0)
if a[:2]==['pr','comment'] and s.get('land_for_real'): sys.exit(0)
if a[:2]==['pr','close'] and s.get('land_for_real'):
    meta(a[2])['state']='CLOSED'
    (r/'transport.json').write_text(json.dumps(s)); sys.exit(0)
if a[:2]==['pr','merge']:
    with (r/'merge-calls').open('a') as f: f.write(' '.join(a)+'\n')
    if s.get('retry'):
        s['retry']=False; s['prs']['11']['headRefOid']='f'*40
        (r/'transport.json').write_text(json.dumps(s)); print('error connecting to api.github.com',file=sys.stderr); sys.exit(1)
    if s.get('land_for_real'):
        v=meta(a[2]); subprocess.run(['git','push','origin',v['headRefOid']+':refs/heads/main'],check=True)
        v['state']='MERGED'
        (r/'transport.json').write_text(json.dumps(s))
    sys.exit(0)
if a[0]=='api':
    # All-open association examines unrelated riders too. Supply their real
    # commit evidence instead of treating a new GraphQL read as a transport fault.
    # trace:BUG-2451 | ai:codex
    if a[1]=='graphql':
        repo={}
        for n,v in s['prs'].items():
            message=subprocess.check_output(['git','show','-s','--format=%B',v['headRefOid']],text=True)
            repo['p'+n]=dict(headRefOid=v['headRefOid'],commits=dict(totalCount=1,nodes=[dict(commit=dict(oid=v['headRefOid'],message=message))]))
        emit(dict(data=dict(repository=repo)))
    endpoint=next((x for x in a[1:] if x.startswith('repos/')),a[1])
    if '/check-runs' in endpoint:
        # trace:STORY-2905 | ai:claude — a grant lapsing during the CI wait.
        if s.get('revoke_dir') and not s.get('revoke_on'):
            if s.get('revoke_skip',0)>0: s['revoke_skip']-=1
            else:
                for g in pathlib.Path(s.pop('revoke_dir')).glob('*.json'): g.unlink()
            (r/'transport.json').write_text(json.dumps(s))
        sha=endpoint.split('/commits/')[1].split('/')[0]
        # trace:BUG-2528 | ai:codex
        pending=s.get('pending_ci_reads',0)>0
        if pending:
            s['pending_ci_reads']-=1
            (r/'transport.json').write_text(json.dumps(s))
        emit(dict(total_count=2,check_runs=[dict(name=n,head_sha=s.get('ci_sha',sha),status='in_progress' if pending and n=='merge-hold-gate' else 'completed',conclusion=None if pending and n=='merge-hold-gate' else s.get('ci','success')) for n in ['Build (ubuntu-latest)','merge-hold-gate']]))
    if endpoint.split('?',1)[0].endswith('/status'):
        sha=endpoint.split('/commits/')[1].split('/')[0]; emit(dict(sha=sha,total_count=0,statuses=[]))
    if '/pulls/' in endpoint: emit(meta(endpoint.split('/pulls/')[1].split('/')[0]))
    # trace:BUG-2401 | ai:claude — the train's required-check policy reads.
    if endpoint.endswith('/branches/main/protection'): emit(dict(required_status_checks=dict(contexts=['Build (ubuntu-latest)','merge-hold-gate'])))
    if '/rules/branches/main' in endpoint: emit([])
if a[:2]==['repo','view']: emit(dict(nameWithOwner='fixture/train',defaultBranchRef=dict(name='main')))
print('unexpected gh '+repr(a),file=sys.stderr); sys.exit(2)
"##;

const FIXTURE_CONFIG: &str = "store_path = \".aida-store\"\n[forge]\nprovider = \"github\"\n";

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
        let mut pairs = vec![
            ("PATH", Some(self.path.as_str())),
            ("USER", Some("train-test-operator")),
        ];
        pairs.extend_from_slice(extra);
        let guard = crate::test_env::EnvVarsGuard::apply(&pairs);
        self._env = Some(guard);
    }
    fn new(bound: bool) -> Self {
        Self::with_member_metadata(bound, None, None)
    }
    // trace:BUG-2441 | ai:codex
    fn with_member_metadata(bound: bool, branch: Option<&str>, title: Option<&str>) -> Self {
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
        // The default branch carries the project config, so review freshness
        // reads the fast policy; without a trusted config it is strict and no
        // carry-over proof is attempted (or refused) at all.
        // trace:STORY-2763 | ai:claude
        fs::create_dir_all(root.join(".aida")).unwrap();
        fs::write(root.join(".aida/config.toml"), FIXTURE_CONFIG).unwrap();
        g(root, &["add", "-f", ".aida/config.toml"]);
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
            let title = title
                .filter(|_| n == 11)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("[AI:codex] fix: fixture {n} ({alias})"));
            let branch = branch
                .filter(|_| n == 11)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("source/{alias}"));
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
            let receipt = crate::gate::review_authority::write_human_receipt(
                &sha, alias, &identity, false, true,
            )
            .unwrap();
            let record = json!({"verdict":"approved", "reviewed_sha":sha, "reviewed_branch":branch,
                "recorded_at":chrono::Utc::now().to_rfc3339(), "recorded_by":"fixture independent human",
                "recorder_attestation": {"sha":sha,"human_at_tty":true,"receipt_id":receipt,"identity":identity}});
            let obj = record.as_object().unwrap().clone();
            ledger::store::append_one(
                root,
                ledger::event::verdict_event(
                    root,
                    ledger::ChangeKey::pr(n),
                    &obj,
                    Some(&ledger::GateSpec::with_key(canonical, None)),
                ),
            )
            .unwrap();
            if bound {
                ledger::store::append_unbound(root, &ledger::GateSpec::with_key(alias, None), &obj)
                    .unwrap();
            }
            records.push(record);
            prs.insert(
                n.to_string(),
                json!({"number":n,"url":format!("https://github.com/fixture/train/pull/{n}"),
                "state":"OPEN","title":title,"body":"", "headRefName":branch,"headRefOid":sha,
                "baseRefName":"main","isDraft":false,"isCrossRepository":false,"labels":[]}),
            );
        }
        // A spec car 11's title only references (`(Refs X)`) exists in the
        // store, owned by no car (STORY-2905 F6). trace:STORY-2905 | ai:claude
        if let Some((_, refs)) = title.and_then(|t| t.split_once("(Refs ")) {
            let id = refs.trim_end_matches(')').trim();
            let known = store
                .requirements
                .iter()
                .any(|r| r.agreed_id.as_deref() == Some(id) || r.spec_id.as_deref() == Some(id));
            // A car's own spec is already in the store.
            if !known {
                let mut req =
                    aida_core::Requirement::new(format!("referenced {id}"), String::new());
                req.spec_id = Some(id.into());
                req.agreed_id = Some(id.into());
                req.req_type = aida_core::RequirementType::Task;
                req.status = aida_core::RequirementStatus::Approved;
                req.execution_mode = Some(aida_core::ExecutionMode::Drain);
                store.requirements.push(req);
            }
        }
        fs::create_dir_all(root.join(".aida")).unwrap();
        fs::write(root.join(".aida/config.toml"), FIXTURE_CONFIG).unwrap();
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
        let env = crate::test_env::EnvVarsGuard::apply(&[
            ("PATH", Some(&path)),
            ("USER", Some("train-test-operator")),
        ]);
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

// Exercise the real builder and final gate with queue-work's suffixed branches.
// trace:BUG-2441 | ai:codex
fixture_case!(
    bug2441_rework_branch_with_resolved_title_trains,
    bug2441_rework_branch_trains,
    false
);
fixture_case!(
    bug2441_rework_branch_with_resolved_commit_trailer_trains,
    bug2441_rework_branch_trains,
    true
);
fn bug2441_rework_branch_trains(trailer_only: bool) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::with_member_metadata(
        true,
        Some("bug-b6b1278890774ef993d2ebc327862470-173-2"),
        Some(if trailer_only {
            "fix: rework (BUG-2400)"
        } else {
            "fix: rework (Refs BUG-2400)"
        }),
    );
    if trailer_only {
        // Keep the committed subject's spec while removing the PR-title hint.
        let mut state: Value =
            serde_json::from_slice(&fs::read(f.root().join("transport.json")).unwrap()).unwrap();
        state["prs"]["11"]["title"] = json!("fix: rework");
        save(f.root(), &state);
    }
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    live.merge_verified(&change, &train, lease.as_ref())
        .unwrap_or_else(|e| panic!("rework member must train: {e:#}"));
    assert_eq!(calls(f.root()).len(), 1);
}

fixture_case!(
    bug2441_unknown_rework_branch_without_resolved_spec_refuses,
    bug2441_unresolved_member_refuses,
    (
        "bug-b6b1278890774ef993d2ebc327862470-999999-2",
        "fix: rework",
        "unresolved train source spec BUG-B6B1278890774EF993D2EBC327862470-999999-2"
    )
);
fixture_case!(
    bug2441_member_without_spec_ids_refuses,
    bug2441_unresolved_member_refuses,
    ("feature/rework", "fix: rework", "no resolvable spec")
);
fixture_case!(
    bug2441_explicit_unknown_id_with_resolved_spec_refuses,
    bug2441_unresolved_member_refuses,
    (
        "feature/rework",
        "fix: rework (BUG-2400 TASK-999999)",
        "unresolved train source spec TASK-999999"
    )
);
// trace:TASK-3111 trace:TASK-2845 | ai:codex
fn bug2441_unresolved_member_refuses((branch, title, cause): (&str, &str, &str)) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::with_member_metadata(true, Some(branch), Some(title));
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    // trace:BUG-2538 | ai:codex — evidence preflight now refuses before open.
    let members: Vec<_> = [11, 12]
        .into_iter()
        .map(|n| live.gate(n).unwrap())
        .collect();
    let base = live.fetch_main().unwrap();
    let error = live.build(&base, &members).unwrap_err();
    assert!(format!("{error:#}").contains(cause), "{error:#}");
    assert!(calls(f.root()).is_empty());
}

// The lease claims STORY-2220, while only the title resolves to BUG-2400:
// even after rework-suffix stripping the branch base must stay unresolved,
// so all resolved gate IDs are filtered out, as in PR #3351.
// trace:TASK-3111 trace:TASK-2843 trace:TASK-2845 | ai:codex
#[test]
fn bug2441_resolved_but_uncovered_member_names_coverage_refusal() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let branch = "story-b6b1278890774ef993d2ebc327862470-999999-2";
    let f = Fixture::with_member_metadata(true, Some(branch), Some("fix: rework (Refs BUG-2400)"));
    let dir = crate::leases::leases_dir(f.root());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("coverage.toml"), format!(
        "id = \"coverage\"\nscope = \"STORY-2220\"\nslug = \"story-2220\"\nowner = \"fixture\"\nworktree_path = {:?}\nbranch = {branch:?}\nstarted_at = \"2026-11-05T00:00:00Z\"\nhostname = \"fixture\"\n",
        f.root().to_str().unwrap()
    )).unwrap();
    assert_eq!(
        crate::trailer_coverage::lease_scopes_for_branch(f.root(), branch),
        ["STORY-2220"]
    );
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    let error = live
        .merge_verified(&change, &train, lease.as_ref())
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("coverage filter"),
        "{error:#}"
    );
    assert!(calls(f.root()).is_empty());
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
    // trace:STORY-2401 | ai:codex — the live CI read carries both identities.
    let ci_key = crate::ci_digest::CiEvidenceKey::read(
        f.root(),
        &train.tip,
        crate::forge::ForgeKind::GitHub,
    )
    .unwrap();
    assert!(ledger::project::events(f.root(), ledger::ChangeKey::pr(77))
        .iter()
        .any(
            |e| matches!(&e.body, ledger::EventBody::TrainCoverage { proof }
            if proof["ci_key"] == serde_json::to_value(&ci_key).unwrap())
        ));
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
                gate_match: None,
                issued_supervised: false,
                supervised_by_mode: false,
                checked_hold: None,
                checked_head: None,
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
    bug2530_train_claim(pr, false);
}

// trace:BUG-2530 | ai:codex
#[test]
fn bug2530_post_approval_claim_allows_train() {
    bug2530_train_claim(11, true);
}

// trace:BUG-2530 | ai:codex
#[test]
fn bug2530_post_approval_legacy_claim_allows_train() {
    bug2530_train_claim_with_precision(11, true, false);
}

fn bug2530_train_claim(pr: u64, redundant: bool) {
    bug2530_train_claim_with_precision(pr, redundant, true);
}

fn bug2530_train_claim_with_precision(pr: u64, redundant: bool, precise: bool) {
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
    let mut marker = crate::gate::review_marker::Marker::for_this_process(
        pr,
        Some(head),
        None,
        "fixture reviewer",
    );
    // trace:BUG-2530 | ai:codex — keep this a pre-approval claim, independent
    // of whether fixture setup and marker creation cross a clock second.
    let approved_at =
        chrono::DateTime::parse_from_rfc3339(f.records[0]["recorded_at"].as_str().unwrap())
            .unwrap()
            .timestamp() as u64;
    marker.started_at = if redundant {
        approved_at + 1
    } else {
        approved_at - 1
    };
    if !precise {
        marker.started_at_nanos = None;
    }
    crate::gate::review_marker::write(f.root(), &marker).unwrap();
    let result = live.merge_verified(&change, &train, lease.as_ref());
    if redundant {
        result.unwrap();
        assert_eq!(calls(f.root()).len(), 1);
    } else {
        assert!(result.is_err());
        assert!(calls(f.root()).is_empty());
    }
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
// trace:BUG-2438 | ai:claude — a current-head refusal under the PR key ejects.
fixture_case!(
    bug2438_current_head_refusal_refuses_pr_11,
    bug2406_new_refusal_refuses,
    "PR-11"
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
                Some(&ledger::GateSpec::with_key("UNRELATED-1", None)),
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

/// BUG-2438: append the incident's PR-3330 ledger shape under PR-11: an
/// earlier spec-stamped refusal reviewed at `refusal_sha`, then `approval`
/// recorded with no spec (review watch). Returns the canonical spec key.
// The displaced refusal deliberately has a clock ahead of the later approval.
// trace:BUG-2438 trace:TASK-2964 | ai:codex
fn bug2438_ledger(f: &Fixture, refusal_sha: &str, approval: &Value) -> &'static str {
    let earlier = (chrono::Utc::now() + chrono::Duration::minutes(3)).to_rfc3339();
    let stale = json!({"verdict":"request-changes", "reviewed_sha":refusal_sha,
        "reviewed_branch":"source/BUG-2400", "recorded_at":earlier,
        "recorded_by":"fixture earlier-round reviewer"});
    let canonical = "BUG-b6b1278890774ef993d2ebc327862470-173";
    for (record, spec) in [(&stale, Some(canonical)), (approval, None)] {
        let spec = spec.map(|key| ledger::GateSpec::with_key(key, None));
        ledger::store::append_one(
            f.root(),
            ledger::event::verdict_event(
                f.root(),
                ledger::ChangeKey::pr(11),
                record.as_object().unwrap(),
                spec.as_ref(),
            ),
        )
        .unwrap();
    }
    canonical
}

/// BUG-2438: open the train on the fixture's current-head approvals, then
/// `record` the case's ledger. Assert its expected refusal without transport:
/// old-head approval fails carry-over before the train's supersession loop;
/// malformed refusal SHAs reach the train's refusing-review check.
// trace:BUG-2438 | ai:claude
// trace:TASK-2831 | ai:codex
fn bug2438_refuses(f: &Fixture, why: &str, expected_message: &str, record: impl FnOnce()) {
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    record();
    let error = live
        .merge_verified(&change, &train, lease.as_ref())
        .expect_err(why);
    assert!(
        format!("{error:#}").contains(expected_message),
        "{why}: expected {expected_message:?}, got {error:#}"
    );
    assert!(calls(f.root()).is_empty(), "{why}");
}

/// BUG-2438: with NO current-head verdict, the older-head approval fails
/// carry-over before the train can evaluate the older-head refusal.
// trace:BUG-2438 | ai:claude
#[test]
fn bug2438_old_head_refusal_without_current_head_verdict_refuses() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(false);
    let old_head = g(f.root(), &["rev-parse", "main"]);
    let mut approval = f.records[0].clone();
    approval["reviewed_sha"] = json!(old_head);
    bug2438_refuses(
        &f,
        "old-head refusal with no current-head verdict",
        "The approval did not carry over",
        || {
            bug2438_ledger(&f, &old_head, &approval);
        },
    );
}

/// BUG-2438: only a well-formed SHA naming another commit is an older head. A
/// refusal whose reviewed_sha is malformed or too short to compare keeps
/// counting (fail closed), even beside a current-head approval.
// trace:BUG-2438 | ai:claude
#[test]
fn bug2438_malformed_reviewed_sha_refusal_is_not_superseded() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    for bad in ["HEAD", "short"] {
        let f = Fixture::new(false);
        let head = f.records[0]["reviewed_sha"].as_str().unwrap().to_string();
        let sha = if bad == "short" {
            head[..6].to_string()
        } else {
            bad.to_string()
        };
        bug2438_refuses(&f, bad, "train source has a refusing review", || {
            bug2438_ledger(&f, &sha, &f.records[0]);
        });
    }
}

/// BUG-2438: an earlier round's refusal, stamped with the spec and reviewed at
/// an older head, is superseded by the PR's approval at the current head
/// (recorded with no spec, so the spec key's newest verdict is still the old
/// refusal). The train lands.
// trace:BUG-2438 | ai:claude
#[test]
fn bug2438_old_head_spec_keyed_refusal_is_superseded_by_current_head_approval() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(false);
    let old_head = g(f.root(), &["rev-parse", "main"]);
    let canonical = bug2438_ledger(&f, &old_head, &f.records[0]);
    let by_spec = ledger::project::newest_verdict(f.root(), canonical).unwrap();
    assert_eq!(
        by_spec["verdict"],
        json!("request-changes"),
        "spec key still names the old refusal"
    );
    assert_eq!(
        ledger::project::newest_verdict(f.root(), "PR-11").unwrap(),
        *f.records[0].as_object().unwrap()
    );
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    live.merge_verified(&change, &train, lease.as_ref())
        .unwrap_or_else(|e| panic!("train must land: {e:?}"));
    let transport = calls(f.root());
    assert_eq!(transport.len(), 1, "the train merges once");
    assert!(transport[0].contains(&format!("--match-head-commit {}", train.tip)));
}

// trace:BUG-2474 | ai:codex
#[test]
fn bug_2474_train_admits_newer_pr_approval_over_same_head_spec_refusal() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(false);
    let head = f.records[0]["reviewed_sha"].as_str().unwrap();
    bug2438_ledger(&f, head, &f.records[0]);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    live.merge_verified(&change, &train, lease.as_ref())
        .unwrap_or_else(|e| panic!("newer same-head approval must admit train: {e:#}"));
    assert_eq!(calls(f.root()).len(), 1);
}

// trace:BUG-2474 | ai:codex
#[test]
fn bug_2474_train_refuses_newer_same_head_spec_refusal_after_pr_approval() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(false);
    let head = f.records[0]["reviewed_sha"].as_str().unwrap();
    bug2438_ledger(&f, head, &f.records[0]);
    bug2438_refuses(
        &f,
        "newer same-head refusal",
        "no approval recorded at its head",
        || {
            let refusal = json!({"verdict":"request-changes", "reviewed_sha":head,
            "recorded_at":chrono::Utc::now().to_rfc3339(), "recorded_by":"later reviewer"});
            ledger::store::append_one(
                f.root(),
                ledger::event::verdict_event(
                    f.root(),
                    ledger::ChangeKey::pr(11),
                    refusal.as_object().unwrap(),
                    Some(&ledger::GateSpec::with_key(
                        "BUG-b6b1278890774ef993d2ebc327862470-173",
                        None,
                    )),
                ),
            )
            .unwrap();
        },
    );
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
                    Some(&ledger::GateSpec::with_key(spec, None)),
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
            Some(&ledger::GateSpec::with_key(
                "BUG-B6B1278890774EF993D2EBC327862470-173",
                None,
            )),
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
        spec_key: None,
        placed_by: Some("fixture reviewer".into()),
        absorbed: vec![],
    }
}

/// An active ship-held mandate in the fixture's isolated HOME.
fn plant_mandate() -> crate::gate::mandate_ship::MandateCover {
    plant_mandate_with_gate_paths(false)
}

// trace:STORY-2733 | ai:codex
fn plant_mandate_with_gate_paths(
    include_gate_paths: bool,
) -> crate::gate::mandate_ship::MandateCover {
    use crate::gate::mandate;
    let principal = crate::gate::mandate::operator_principal();
    let m = mandate::Mandate {
        id: uuid::Uuid::new_v4().to_string(),
        principal: principal.clone(),
        seat: mandate::MANDATE_SEAT.into(),
        scope: mandate::SCOPE_SHIP_HELD.into(),
        include_supervised: false,
        include_gate_paths,
        reason: Some("fixture fire".into()),
        issued_at: chrono::Utc::now() - chrono::Duration::minutes(1),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        revoked_at: None,
        revoked_by: None,
        scopes: Default::default(),
        kind: None,
        renewals: Vec::new(),
        relay_parent: None,
        revived: None,
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
        gate_match: None,
        issued_supervised: false,
        supervised_by_mode: false,
        checked_hold: None,
        checked_head: None,
        id: m.id,
        principal,
    }
}

// trace:BUG-2663 BUG-2666 | ai:codex
fixture_case!(
    bug_2663_unheld_supervised_member_lands_with_mandate,
    supervised_member_lands,
    false
);
fixture_case!(
    bug_2666_held_supervised_member_lands_with_mandate,
    supervised_member_lands,
    true
);

fn supervised_member_lands(held: bool) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use aida_core::DatabaseBackend;
    let mut f = Fixture::new(true);
    let store_root = f.root().join(".aida-store");
    g(&store_root, &["init", "-q", "-b", "aida-store"]);
    let backend = crate::test_env::home_fixture_backend(&store_root)
        .unwrap()
        .with_auto_commit(false);
    let mut store = backend.load().unwrap();
    store
        .requirements
        .iter_mut()
        .find(|req| req.agreed_id.as_deref() == Some("BUG-2400"))
        .unwrap()
        .execution_mode = None;
    backend.save(&store).unwrap();
    let principal = crate::gate::mandate::operator_principal();
    let grant = crate::gate::seat_authority::test_support::mint_grant_for(
        f.root(),
        &principal,
        "advisor",
        &[],
    );
    f.reenv(&[
        ("AIDA_SESSION_GRANT", Some(&grant)),
        ("AIDA_USER", Some(&principal)),
        ("AIDA_SESSION_ID", Some("fixture-independent-advisor")),
    ]);
    g(&store_root, &["config", "user.name", "Fixture store"]);
    g(
        &store_root,
        &["config", "user.email", "store@example.invalid"],
    );
    commit(&store_root, "Commit mandate evidence");
    assert!(crate::gate::seat_authority::current_grant(f.root()).is_some());
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let mut state: Value =
        serde_json::from_slice(&fs::read(f.root().join("transport.json")).unwrap()).unwrap();
    state["land_for_real"] = json!(true);
    if held {
        state["prs"]["11"]["labels"] = json!([
            {"name": "aida:merge-hold"}, {"name": "aida:merge-hold-recorded"}
        ]);
    }
    save(f.root(), &state);
    let head = state["prs"]["11"]["headRefOid"].as_str().unwrap();
    if held {
        crate::gate::merge_hold_core::place_hold(f.root(), &hold_on(11, head)).unwrap();
    }
    assert!(
        live.gate(11).is_err(),
        "a supervised member requires coverage"
    );
    let cover = plant_mandate();
    let path = crate::gate::mandate::mandate_dir()
        .unwrap()
        .join(format!("{}.json", cover.id));
    let mut mandate: crate::gate::mandate::Mandate =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    mandate.include_supervised = true;
    crate::gate::mandate::write_private(&path, &serde_json::to_vec(&mandate).unwrap()).unwrap();
    let gate = |request_full| {
        gate_member_with(
            &mut LiveChecks {
                project_root: f.root(),
                main_worktree: f.root(),
                forge: &forge,
                base: "main",
                named: true,
                read_only: false,
                actor_probe: None,
                request_full,
                released_member: None,
            },
            "main",
            11,
        )
    };
    let planned = gate(false).expect("dry-run admission");
    let member = gate(true).expect("live admission");
    assert_eq!(planned, member);
    assert_eq!(member.release.is_some(), held);
    assert_eq!(member.live_cover().unwrap().supervised_by_mode, !held);
    if held {
        let mut wrong_head = member.clone();
        wrong_head.head = "f".repeat(40);
        assert!(live.release_hold(&wrong_head).is_err());
        assert!(crate::gate::merge_hold_core::read_hold(f.root(), 11).is_some());
    } else {
        let mut label_only = state.clone();
        label_only["prs"]["11"]["labels"] = json!([{"name": "aida:merge-hold"}]);
        save(f.root(), &label_only);
        assert!(gate(false).is_err());
        assert!(gate(true).is_err());
        save(f.root(), &state);
    }
    let mut rails = ForgeRails::new(&mut live, "bug-2666".into());
    let outcome = drive(&mut rails, vec![member]).unwrap();
    assert_eq!(
        outcome.landed.iter().map(|(pr, _)| *pr).collect::<Vec<_>>(),
        vec![11],
        "{outcome:?}"
    );
    assert_eq!(calls(f.root()).len(), 1);
    let state: Value =
        serde_json::from_slice(&fs::read(f.root().join("transport.json")).unwrap()).unwrap();
    assert_eq!(state["prs"]["11"]["state"], "CLOSED");

    let store = ledger::store_root(f.root()).unwrap();
    if held {
        let events = ledger::project::events(&store, ledger::ChangeKey::pr(11));
        let decision = events.iter().position(|event| {
            event.head.as_deref() == Some(head)
                && matches!(&event.body, ledger::EventBody::TrainMandateCoverage { mandate, .. } if mandate == &cover.id)
        }).unwrap();
        let release = events
            .iter()
            .position(|event| matches!(event.body, ledger::EventBody::HoldRelease { .. }))
            .unwrap();
        assert!(decision < release);
    } else {
        let events = ledger::project::events(&store, ledger::ChangeKey::pr(77));
        assert!(events.iter().any(|event| matches!(&event.body,
            ledger::EventBody::TrainCoverage { proof }
                if proof["sources"][0]["mandate_ship_reason"] == "supervised-by-mode")));
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
    // The release has re-run a context on this exact tip. The real final
    // consumer must wait, then land instead of ejecting the member.
    // trace:BUG-2528 | ai:codex
    let mut state: Value =
        serde_json::from_slice(&fs::read(f.root().join("transport.json")).unwrap()).unwrap();
    state["pending_ci_reads"] = json!(1);
    save(f.root(), &state);
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

// trace:STORY-2733 | ai:codex
fixture_case!(
    mandate_train_final_audit_survives_hold_release,
    mandate_final_audit,
    "same"
);
fixture_case!(
    mandate_train_final_audit_refuses_replacement,
    mandate_final_audit,
    "replacement"
);
fixture_case!(
    mandate_train_final_audit_refuses_changed_paths,
    mandate_final_audit,
    "paths"
);
fixture_case!(
    mandate_train_final_audit_refuses_new_protected_path,
    mandate_final_audit,
    "added"
);
fn mandate_final_audit(change_audit: &str) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let mut cover = plant_mandate_with_gate_paths(true);
    let dir = crate::gate::mandate::mandate_dir().unwrap();
    let path = dir.join(format!("{}.json", cover.id));
    let config = crate::gate::kernel_deps::aida_home_dir()
        .unwrap()
        .join(".aida/config.toml");
    let protected_config = "[mandate.human_gates]\nprotected_paths = ['car11']\n";
    fs::write(
        &config,
        if change_audit == "added" {
            ""
        } else {
            protected_config
        },
    )
    .unwrap();
    let gates = crate::gate::mandate_gates::load_human_gates(f.root());
    cover = crate::gate::mandate_ship::MandateCover::from_checked_paths(
        cover.id,
        cover.principal,
        &["car11".into()],
        &crate::gate::mandate_gates::ProtectedPaths::of(&gates.map_err(|e| e.to_string())),
        false,
    );
    let mut members: Vec<_> = [11, 12]
        .into_iter()
        .map(|pr| live.gate(pr).unwrap())
        .collect();
    members[0].gate_audit = cover.gate_audit();
    assert_eq!(members[0].gate_audit.is_some(), change_audit != "added");
    members[0].release = Some(cover.clone());
    let base = live.fetch_main().unwrap();
    let train = live.build(&base, &members).unwrap();
    let change = live.open(&train, "train/mandate-audit").unwrap();
    assert_eq!(live.wait_ci(&change, &train).unwrap(), CiVerdict::Green);
    let hold = hold_on(11, &members[0].head);
    crate::gate::merge_hold_core::place_hold(f.root(), &hold).unwrap();
    crate::gate::merge_hold_core::release_hold_under_mandate(
        f.root(),
        &hold,
        &cover.principal,
        &cover.id,
        None,
    )
    .unwrap();
    match change_audit {
        "replacement" => {
            fs::remove_file(path).unwrap();
            plant_mandate_with_gate_paths(true);
        }
        "paths" => fs::write(config, "[mandate.human_gates]\nprotected_paths = []\n").unwrap(),
        "same" => {}
        "added" => fs::write(config, protected_config).unwrap(),
        _ => unreachable!(),
    }
    let lease = live.lease(change.id).unwrap();
    let result = live.merge_verified(&change, &train, lease.as_ref());
    if change_audit == "same" {
        result.unwrap();
        assert_eq!(calls(f.root()).len(), 1);
    } else {
        let error = format!("{:#}", result.unwrap_err());
        assert!(
            error.contains("audit changed") || error.contains("mandate expired"),
            "{error}"
        );
        assert!(calls(f.root()).is_empty());
    }
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

// trace:TASK-3112 | ai:codex
#[test]
fn task3112_rework_hint_retains_owning_spec_beside_refs_metadata() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let ids = resolved_member_spec_ids(
        f.root(),
        "bug-b6b1278890774ef993d2ebc327862470-173-2",
        "fix: rework (Refs STORY-2220)",
        "",
        "",
    )
    .unwrap();
    assert!(
        ids.iter()
            .any(|id| id.eq_ignore_ascii_case("BUG-b6b1278890774ef993d2ebc327862470-173")),
        "{ids:?}"
    );
    assert!(ids.contains(&"STORY-2220".to_string()));
    let backend = aida_core::GitBackend::new(&f.root().join(".aida-store"))
        .unwrap()
        .with_auto_commit(false);
    let mut store = aida_core::DatabaseBackend::load(&backend).unwrap();
    store.requirements[0].execution_mode = Some(aida_core::ExecutionMode::Operator);
    aida_core::DatabaseBackend::save(&backend, &store).unwrap();
    let policies =
        crate::pr_cmd::ship_gate_spec_records_for_ids(f.root(), aliases(f.root(), &ids).unwrap());
    assert!(
        policies
            .iter()
            .any(|(_, _, mode)| mode.mode == Some(aida_core::ExecutionMode::Operator)),
        "{policies:?}"
    );
}

// trace:BUG-2528 | ai:codex
#[test]
fn bug2528_exact_tip_ci_wait_is_bounded_and_fails_closed() {
    use crate::forge::{CheckOutcome, CommitCheck, CommitChecks};
    let checks = |sha: &str, outcome| CommitChecks {
        sha: sha.into(),
        checks: vec![CommitCheck {
            name: "merge-hold-gate".into(),
            outcome,
            run_id: None,
            workflow_id: Some("ci".to_string()),
        }],
    };
    let time = ExactTipCiClock::default();
    let reads = std::cell::Cell::new(0);
    let error = wait_exact_tip_ci(
        "tip",
        &["merge-hold-gate"],
        180,
        || {
            reads.set(reads.get() + 1);
            Ok(checks("tip", CheckOutcome::Pending))
        },
        || time.elapsed(),
        || time.sleep(),
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("still pending after 180s: merge-hold-gate"));
    assert_eq!(time.elapsed(), 180);
    assert_eq!(reads.get(), 19);
    for (sha, outcome) in [
        ("other", CheckOutcome::Passed),
        ("tip", CheckOutcome::Failed),
        ("tip", CheckOutcome::Skipped),
    ] {
        wait_exact_tip_ci(
            "tip",
            &["merge-hold-gate"],
            180,
            || Ok(checks(sha, outcome)),
            || 0,
            || panic!("must refuse immediately"),
        )
        .unwrap_err();
    }
    wait_exact_tip_ci(
        "tip",
        &["merge-hold-gate"],
        180,
        || anyhow::bail!("unreadable CI"),
        || 0,
        || panic!("must refuse immediately"),
    )
    .unwrap_err();
}

// trace:BUG-2538 | ai:codex
#[test]
fn bug2538_long_review_history_trains_with_compact_audit() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let mut record = f.records[0].clone();
    record["rounds"] = json!((0..41).map(|_| f.records[0].clone()).collect::<Vec<_>>());
    for sequence in 0..110 {
        record["audit_sequence"] = json!(sequence);
        ledger::store::append_one(
            f.root(),
            ledger::event::verdict_event(
                f.root(),
                ledger::ChangeKey::pr(11),
                record.as_object().unwrap(),
                Some(&ledger::GateSpec::with_key(
                    "BUG-b6b1278890774ef993d2ebc327862470-173",
                    None,
                )),
            ),
        )
        .unwrap();
    }
    assert!(
        fs::read_to_string(ledger::store::ledger_file(
            f.root(),
            ledger::ChangeKey::pr(11)
        ))
        .unwrap()
        .lines()
        .count()
            > 100
    );
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    live.merge_verified(&change, &train, lease.as_ref())
        .unwrap();
    assert_eq!(calls(f.root()).len(), 1);
    let events = ledger::project::events(f.root(), ledger::ChangeKey::pr(77));
    let proof = events
        .iter()
        .find_map(|event| match &event.body {
            ledger::EventBody::TrainCoverage { proof } => Some(proof),
            _ => None,
        })
        .unwrap();
    assert!(serde_json::to_vec(proof).unwrap().len() < 32 * 1024);
    for source in proof["sources"].as_array().unwrap() {
        for original in source["originals"].as_array().unwrap() {
            assert!(original.get("record").is_none());
            assert_eq!(original["sha256"].as_str().unwrap().len(), 64);
        }
    }
    assert_ne!(digest(&record).unwrap(), digest(&f.records[0]).unwrap());
}

// trace:BUG-2538 | ai:codex
#[test]
fn bug2538_oversized_import_ejects_before_publication() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(true);
    let mut record = f.records[0].clone();
    record["audit_padding"] = json!("x".repeat(9 * 1024 * 1024));
    ledger::store::append_unbound(
        f.root(),
        &ledger::GateSpec::with_key("BUG-2400", None),
        record.as_object().unwrap(),
    )
    .unwrap();
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let member = live.gate(11).unwrap();
    let base = live.fetch_main().unwrap();
    let train = live.build(&base, &[member]).unwrap();
    assert!(train.cars.is_empty());
    assert_eq!(train.dropped[0].pr, 11);
    assert!(train.dropped[0].reason.contains("16 MiB"));
    let transport = fs::read_to_string(f.root().join("transport-calls")).unwrap();
    assert!(!transport.lines().any(|line| line.starts_with("pr create")));
    assert!(ensure_budget(MAX_EVIDENCE_BYTES).is_ok());
    assert!(ensure_budget(MAX_EVIDENCE_BYTES + 1).is_err());
}

/// BUG-2569: a promotion branch claims no spec, so the coverage filter keeps
/// every spec its car message names (promotion #3966 named 45).
// trace:BUG-2569 | ai:claude
const BUG2569_BRANCH: &str = "promote/fixture-6";

/// BUG-2569: move main, merge it cleanly into PR 11's branch with `message`
/// and point the forge at the new head. PR 11's approval stays at the old
/// head, so it reaches the new one only by carry-over.
// trace:BUG-2569 | ai:claude
fn bug2569_carried_head(f: &Fixture, message: &str) -> String {
    const BRANCH: &str = BUG2569_BRANCH;
    let root = f.root();
    fs::write(root.join("moved"), "main moved\n").unwrap();
    commit(root, "main moves");
    g(root, &["push", "-q", "origin", "main"]);
    g(root, &["checkout", "-q", BRANCH]);
    g(root, &["merge", "-q", "--no-ff", "-m", message, "main"]);
    let head = g(root, &["rev-parse", "HEAD"]);
    g(root, &["push", "-q", "origin", BRANCH]);
    g(root, &["checkout", "-q", "main"]);
    let mut state: Value =
        serde_json::from_slice(&fs::read(root.join("transport.json")).unwrap()).unwrap();
    state["prs"]["11"]["headRefOid"] = json!(head);
    save(root, &state);
    head
}

/// BUG-2569: a refusal for STORY-2220 recorded on `change` at `sha`.
// trace:BUG-2569 | ai:claude
fn bug2569_refusal(f: &Fixture, change: u64, sha: &str) {
    let record = json!({"verdict":"request-changes", "reviewed_sha":sha,
        "recorded_at":chrono::Utc::now().to_rfc3339(), "recorded_by":"fixture other reviewer"});
    let spec = ledger::GateSpec::with_key("STORY-b6b1278890774ef993d2ebc327862470-123", None);
    ledger::store::append_one(
        f.root(),
        ledger::event::verdict_event(
            f.root(),
            ledger::ChangeKey::pr(change),
            record.as_object().unwrap(),
            Some(&spec),
        ),
    )
    .unwrap();
}

/// BUG-2569 (train 9): PR 11's approval carried over a clean merge of main.
/// Its car message names STORY-2220, whose newest verdict is a refusal of
/// another change at another commit. The member gate admitted it and the
/// merge step refused it after the CI run. Both now read that refusal as
/// being about the other change, and the train lands.
// trace:BUG-2569 | ai:claude
#[test]
fn bug_2569_train_lands_a_carried_member_whose_car_names_a_spec_refused_elsewhere() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::with_member_metadata(false, Some(BUG2569_BRANCH), None);
    bug2569_carried_head(&f, "Merge main into the member (STORY-2220)");
    bug2569_refusal(&f, 99, &"c".repeat(40));
    let forge = GitHubForge::new(f.root());
    let mut live = f.live(&forge);
    let (train, change) = f.opened(&mut live);
    let lease = live.lease(change.id).unwrap();
    live.merge_verified(&change, &train, lease.as_ref())
        .unwrap_or_else(|e| panic!("the carried member must land: {e:#}"));
    assert_eq!(calls(f.root()).len(), 1, "the train merges once");
}

/// BUG-2569: the member gate asks about the specs the car message names, as
/// the merge step does. Change 99 stands refused at a commit PR 11's head
/// contains, under a spec only PR 11's car message names: that stops PR 11
/// at the gate, before a train is built, not at the merge after the CI run.
// trace:BUG-2569 | ai:claude
#[test]
fn bug_2569_member_gate_refuses_what_the_merge_step_would_refuse() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    // Every constituent claim remains visible, including comma-separated groups.
    // trace:BUG-2635 | ai:codex
    for subject in [
        "Merge main into the member (STORY-2220)",
        "Merge main into the member (BUG-2400,STORY-2220)",
        "Merge main into the member (BUG-2400 Refs: STORY-2220,BUG-2400)",
    ] {
        let f = Fixture::with_member_metadata(false, Some(BUG2569_BRANCH), None);
        bug2569_carried_head(&f, subject);
        let inside = f.records[0]["reviewed_sha"].as_str().unwrap().to_string();
        bug2569_refusal(&f, 99, &inside);
        let forge = GitHubForge::new(f.root());
        let mut live = f.live(&forge);
        let refused = match live.gate(11) {
            Ok(_) => panic!("the member gate admitted a member the merge step refuses"),
            Err(why) => format!("{why:#}"),
        };
        assert!(refused.contains("do not approve"), "{refused}");
        assert!(calls(f.root()).is_empty());
    }
}

// STORY-2905 S1t-b round 2 finding 4: the per-car merge authority through
// the production boundary (`VerifiedTrainMerge::enforce`, reached through
// the forge's train transport), in review-gated mode.
// trace:STORY-2905 | ai:claude
mod story_2905_enforce {
    use super::*;

    /// A fixture whose default branch is review-gated, with an advisor
    /// grant exported and an (empty) authorship ledger, its train opened
    /// and verified under the live lease.
    fn review_gated_advisor(case: impl FnOnce(&Fixture, &GitHubForge, &Built, &ChangeRef)) {
        review_gated_advisor_titled(None, case)
    }

    /// [`review_gated_advisor`] with car 11's PR title set to `title`.
    // trace:STORY-2905 | ai:claude
    fn review_gated_advisor_titled(
        title: Option<&str>,
        case: impl FnOnce(&Fixture, &GitHubForge, &Built, &ChangeRef),
    ) {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        review_gated_advisor_in_child(title, case)
    }

    /// [`review_gated_advisor_titled`]'s body, for a test already running in
    /// its isolated child (one that builds more than one fixture).
    fn review_gated_advisor_in_child(
        title: Option<&str>,
        case: impl FnOnce(&Fixture, &GitHubForge, &Built, &ChangeRef),
    ) {
        let mut f = Fixture::with_member_metadata(true, None, title);
        fs::write(
            f.root().join(".aida/config.toml"),
            format!("{FIXTURE_CONFIG}[landing]\nmode = \"review-gated\"\n"),
        )
        .unwrap();
        g(f.root(), &["add", "-f", ".aida/config.toml"]);
        commit(f.root(), "review-gated landing");
        g(f.root(), &["push", "-q", "origin", "main"]);
        fs::write(f.root().join(".aida/review-authorship.jsonl"), "").unwrap();
        let grant = crate::gate::seat_authority::test_support::mint_grant_for(
            f.root(),
            "train-test-operator",
            "advisor",
            &[],
        );
        // The advisor session's identity, so an authorship record can name
        // it (F6). trace:STORY-2905 | ai:claude
        f.reenv(&[
            ("AIDA_SESSION_GRANT", Some(grant.as_str())),
            ("AIDA_SESSION_ID", Some("advisor-session")),
        ]);
        assert_eq!(
            crate::gate::seat_authority::current_seat(f.root()).as_deref(),
            Some("advisor"),
            "{:?}",
            crate::gate::seat_authority::grant_check(f.root())
        );
        let forge = GitHubForge::new(f.root());
        let mut live = f.live(&forge);
        let (train, change) = f.opened(&mut live);
        case(&f, &forge, &train, &change);
    }

    fn merge_verified(
        f: &Fixture,
        forge: &GitHubForge,
        train: &Built,
        change: &ChangeRef,
    ) -> Result<()> {
        let mut live = f.live(forge);
        let lease = live.lease(change.id).unwrap();
        live.merge_verified(change, train, lease.as_ref())
    }

    fn set_state(f: &Fixture, key: &str, value: Value) {
        let mut state: Value =
            serde_json::from_slice(&fs::read(f.root().join("transport.json")).unwrap()).unwrap();
        state[key] = value;
        save(f.root(), &state);
    }

    /// A valid advisor grant merges the train: one pinned rebase merge.
    #[test]
    fn a_valid_advisor_grant_merges_the_train() {
        review_gated_advisor(|f, forge, train, change| {
            merge_verified(f, forge, train, change).expect("the advisor merges the train");
            let transport = calls(f.root());
            assert_eq!(transport.len(), 1, "{transport:?}");
            assert!(transport[0].contains(&format!("--match-head-commit {}", train.tip)));
        });
    }

    /// A grant that lapses after the first authority check (here during the
    /// refresh's CI read, the train's long wait) refuses at the check that
    /// runs last before the transport: no forge merge call.
    #[test]
    fn a_grant_lapsing_after_the_first_recheck_refuses_without_a_forge_call() {
        review_gated_advisor(|f, forge, train, change| {
            // The proof reads CI once for the train and once per car; the
            // grant is revoked on the first CI read after those, the final
            // gate's refresh, which runs after the first authority check.
            let dir = crate::gate::seat_authority::grant_dir().unwrap();
            set_state(f, "revoke_skip", json!(1 + train.cars.len()));
            set_state(f, "revoke_dir", json!(dir.display().to_string()));
            let why = format!("{:#}", merge_verified(f, forge, train, change).unwrap_err());
            assert!(why.contains("has the advisor perform"), "{why}");
            assert!(calls(f.root()).is_empty(), "no forge merge call: {why}");
            let state: Value =
                serde_json::from_slice(&fs::read(f.root().join("transport.json")).unwrap())
                    .unwrap();
            assert!(
                state.get("revoke_dir").is_none(),
                "the grant lapsed mid-gate"
            );
        });
    }

    /// The base the train was built and proved on moved: refused, no call.
    /// (`train_recheck`'s own base pin is exercised directly in
    /// `landing_merge`'s `story_2905_train_recheck_tests`.)
    #[test]
    fn a_base_move_since_the_proof_refuses_the_train() {
        review_gated_advisor(|f, forge, train, change| {
            g(f.root(), &["checkout", "-q", "main"]);
            fs::write(f.root().join("base-moved"), "moved\n").unwrap();
            commit(f.root(), "base moved");
            g(f.root(), &["push", "-q", "origin", "main"]);
            let why = format!("{:#}", merge_verified(f, forge, train, change).unwrap_err());
            // Refused by whichever check sees it first (the proof's own base
            // read, or `train_recheck`'s base pin); never merged.
            assert!(why.contains("moved"), "{why}");
            assert!(calls(f.root()).is_empty(), "{why}");
        });
    }

    /// Record that the session running this test (the granted advisor)
    /// authored `spec`, through a lease since released: only the
    /// authorship ledger names it.
    // trace:STORY-2905 | ai:claude
    fn advisor_authored(f: &Fixture, spec: &str) {
        let tokens = crate::gate::review_authority::RecorderIdentity::current(
            &crate::gate::review_authority::visible_leases(f.root()),
        )
        .tokens;
        assert!(!tokens.is_empty(), "the advisor session has an identity");
        let record = json!({
            "scope": spec,
            "lease_id": "released-author-lease",
            "branch": format!("source/{spec}"),
            "tokens": tokens,
            "recorded_at": "2026-11-19T00:00:00Z",
        });
        fs::write(
            f.root().join(".aida/review-authorship.jsonl"),
            format!("{record}\n"),
        )
        .unwrap();
    }

    /// Promotion 23 review F6 (STORY-2905 C2): the second car was authored
    /// by the granted advisor (its lease since released), so the advisor
    /// may not merge the train: refused at the per-car author rule, naming
    /// that car, with zero forge merge calls.
    #[test]
    fn a_car_authored_by_the_granted_advisor_refuses_the_train() {
        review_gated_advisor(|f, forge, train, change| {
            advisor_authored(f, "STORY-2220");
            let why = format!("{:#}", merge_verified(f, forge, train, change).unwrap_err());
            assert!(why.contains("authored the change"), "{why}");
            assert!(why.contains("STORY-2220"), "{why}");
            assert!(calls(f.root()).is_empty(), "zero forge merges: {why}");
        });
    }

    /// F6 control: an authorship record by someone else (an independent
    /// advisor merges) leaves the train mergeable: one pinned merge.
    #[test]
    fn an_independent_advisor_merges_a_train_whose_cars_others_authored() {
        review_gated_advisor(|f, forge, train, change| {
            let record = json!({
                "scope": "STORY-2220",
                "lease_id": "implementer-lease",
                "branch": "source/STORY-2220",
                "tokens": ["session:some-implementer"],
                "recorded_at": "2026-11-19T00:00:00Z",
            });
            fs::write(
                f.root().join(".aida/review-authorship.jsonl"),
                format!("{record}\n"),
            )
            .unwrap();
            merge_verified(f, forge, train, change).expect("an independent advisor merges");
            assert_eq!(calls(f.root()).len(), 1);
        });
    }

    /// F6 control: the advisor authored only an unrelated spec, which no
    /// car names: the train still merges (the author rule is per car's
    /// subject, not every author the ledger ever recorded).
    // trace:STORY-2905 | ai:claude
    #[test]
    fn an_advisor_who_authored_only_unrelated_work_merges_the_train() {
        review_gated_advisor(|f, forge, train, change| {
            advisor_authored(f, "TASK-9999");
            merge_verified(f, forge, train, change).expect("unrelated authorship");
            assert_eq!(calls(f.root()).len(), 1);
        });
    }

    /// F6: a spec a car only references (`(Refs X)` in its subject, owned
    /// by no car) is part of the full authorship subject: the advisor
    /// authored TASK-4242, which car 11 merely references, so the train is
    /// refused with zero forge merges.
    // trace:STORY-2905 | ai:claude
    #[test]
    fn a_refs_only_contributor_counts_as_an_author() {
        review_gated_advisor_titled(
            Some("[AI:codex] fix: fixture 11 (BUG-2400) (Refs TASK-4242)"),
            |f, forge, train, change| {
                advisor_authored(f, "TASK-4242");
                let why = format!("{:#}", merge_verified(f, forge, train, change).unwrap_err());
                assert!(why.contains("authored the change"), "{why}");
                assert!(why.contains("TASK-4242"), "{why}");
                assert!(calls(f.root()).is_empty(), "{why}");
            },
        );
    }

    /// Arm the fake forge to revoke every grant during the `skip + 1`-th
    /// metadata read of PR `pr`; `None` arms the read count only.
    // trace:STORY-2903 | ai:claude
    fn arm_revoke_on(f: &Fixture, pr: u64, skip: Option<u64>) {
        set_state(f, "revoke_on", json!(pr.to_string()));
        set_state(f, "revoke_on_reads", json!(0));
        match skip {
            Some(skip) => {
                let dir = crate::gate::seat_authority::grant_dir().unwrap();
                set_state(f, "revoke_on_skip", json!(skip));
                set_state(f, "revoke_dir", json!(dir.display().to_string()));
            }
            None => set_state(f, "revoke_on_skip", json!(u64::MAX / 2)),
        }
    }

    fn state_u64(f: &Fixture, key: &str) -> u64 {
        let state: Value =
            serde_json::from_slice(&fs::read(f.root().join("transport.json")).unwrap()).unwrap();
        state[key].as_u64().unwrap_or(0)
    }

    /// Promotion 23 review r4 (TOCTOU), the train's last car: the grant is
    /// revoked during the LAST metadata read of car 12 (its author lookup in
    /// the final authority check before the transport). The seat is
    /// validated after that read, so the train is not merged. The read count
    /// comes from a control run on an identical fixture, where the grant
    /// holds throughout and the train merges.
    #[test]
    fn a_grant_revoked_during_the_last_cars_author_lookup_refuses_the_train() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let mut reads = 0;
        review_gated_advisor_in_child(None, |f, forge, train, change| {
            arm_revoke_on(f, 12, None);
            merge_verified(f, forge, train, change).expect("control: the grant holds");
            assert_eq!(calls(f.root()).len(), 1, "control merges once");
            reads = state_u64(f, "revoke_on_reads");
        });
        assert!(reads > 0, "car 12's metadata is read before the merge");
        review_gated_advisor_in_child(None, |f, forge, train, change| {
            arm_revoke_on(f, 12, Some(reads - 1));
            let why = format!("{:#}", merge_verified(f, forge, train, change).unwrap_err());
            assert!(why.contains("has the advisor perform"), "{why}");
            assert!(calls(f.root()).is_empty(), "no train merge: {why}");
        });
    }

    /// Promotion 23 review r4 (TOCTOU), a ship through the merge boundary:
    /// the grant is revoked during the LAST metadata read of PR 11 (its
    /// author lookup inside the per-attempt guard), so no `gh pr merge` is
    /// sent; the control (grant held) merges once.
    #[test]
    fn a_grant_revoked_during_a_ships_author_lookup_refuses_the_merge() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        fn ship(f: &Fixture, forge: &GitHubForge) -> std::result::Result<(), String> {
            let head: Value =
                serde_json::from_slice(&fs::read(f.root().join("transport.json")).unwrap())
                    .unwrap();
            let head = head["prs"]["11"]["headRefOid"]
                .as_str()
                .unwrap()
                .to_string();
            let change = ChangeRef {
                id: 11,
                url: String::new(),
                branch: "source/BUG-2400".into(),
                base: "main".into(),
                title: None,
            };
            let opts = MergeOptions {
                method: MergeMethod::Squash,
                squash_subject: None,
                squash_body: None,
                delete_branch: false,
                match_head: Some(head),
            };
            crate::gate::landing_merge::merge(
                forge,
                f.root(),
                crate::gate::landing_merge::Caller::Ship,
                &crate::gate::landing_merge::Entry::capture(f.root()),
                &change,
                &["BUG-2400".to_string()],
                &opts,
                &mut crate::network_retry::NoopSink,
                &crate::pr_train::pr_head_pin_journals,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        }
        let mut reads = 0;
        review_gated_advisor_in_child(None, |f, forge, _, _| {
            arm_revoke_on(f, 11, None);
            ship(f, forge).expect("control: the grant holds");
            assert_eq!(calls(f.root()).len(), 1, "control merges once");
            reads = state_u64(f, "revoke_on_reads");
        });
        assert!(reads > 0, "PR 11's metadata is read before the merge");
        review_gated_advisor_in_child(None, |f, forge, _, _| {
            arm_revoke_on(f, 11, Some(reads - 1));
            let why = ship(f, forge).unwrap_err();
            assert!(why.contains("has the advisor perform"), "{why}");
            assert!(calls(f.root()).is_empty(), "no merge sent: {why}");
        });
    }

    /// Round 2 finding 3: an authorship ledger that cannot be trusted
    /// refuses the train's car author rule; nothing is merged.
    #[test]
    fn a_corrupt_authorship_ledger_refuses_the_train() {
        review_gated_advisor(|f, forge, train, change| {
            fs::write(f.root().join(".aida/review-authorship.jsonl"), "{torn\n").unwrap();
            let why = format!("{:#}", merge_verified(f, forge, train, change).unwrap_err());
            assert!(why.contains("authors cannot be established"), "{why}");
            assert!(calls(f.root()).is_empty(), "{why}");
        });
    }
}

/// TASK-3792: a complete, eligible member gate run by `aida pr train --check`
/// spawns nothing outside the check's deadline. The fixture member has a
/// fetchable head, an attested approval and green commit CI, so the report
/// runs every member stage (head, approval, full-ci, message, corpus, spec
/// policy) through the real forge and git; a wrapper in front of both binaries
/// records any spawn that lacks the scoped-deadline marker.
// trace:TASK-3792 | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn task_3792_complete_member_gate_spawns_nothing_outside_the_deadline() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::pr_train::check::{collect_report, CheckOptions, Evidence};
    use std::time::Duration;

    let mut f = Fixture::new(true);
    let root = f.root().to_path_buf();
    let fixture_bin = root.join("bin");
    let guard_bin = root.join("bin/guard");
    fs::create_dir_all(&guard_bin).unwrap();
    // aida-core bounds its stored-URL read with its own deadline transport;
    // every other spawn must carry the marker. The fixture forge has no
    // live-train listing, so the guard answers it: no train is open.
    for tool in ["git", "gh"] {
        crate::test_exec::write_executable(
            &guard_bin.join(tool),
            r#"#!/bin/sh
case "$*" in
  *'config --get-regexp ^remote'*) ;;
  *) [ -n "$@G@" ] || echo "@T@ $*" >> '@R@/transport-unbounded' ;;
esac
echo "@T@ $*" >> '@R@/transport-all'
# The live-train listing (discovery): no train is open.
case "@T@ $*" in
  'gh pr list'*'number,headRefName,body'*) echo '[]'; exit 0 ;;
esac
exec '@B@/@T@' "$@"
"#
            .replace(
                "@G@",
                crate::bounded_process::test_hooks::SCOPED_DEADLINE_SPAWN_ENV,
            )
            .replace("@T@", tool)
            .replace("@R@", &root.display().to_string())
            .replace("@B@", &fixture_bin.display().to_string()),
        );
    }
    f.path = format!("{}:{}", guard_bin.display(), f.path);
    f.reenv(&[]);
    let _ = fs::remove_file(root.join("transport-calls"));

    let opts = CheckOptions {
        prs: &[11],
        all_open: false,
        skip_full_check: false,
        json: true,
        timeout: Duration::from_secs(60),
    };
    let (base, rows) = collect_report(&opts, || Ok((root.clone(), root.clone()))).unwrap();
    assert_eq!(base.as_deref(), Some("main"));
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert!(row.eligible, "the complete member gate refused: {row:?}");
    assert_eq!(row.gate, None, "{row:?}");
    assert_eq!(row.evidence, Evidence::Read);
    assert_eq!(
        row.head.as_deref(),
        f.records[0]["reviewed_sha"].as_str(),
        "{row:?}"
    );

    // The late member stages really ran under the scope: the head fetch,
    // the full-ci commit read and the message's title/body read.
    let all = fs::read_to_string(root.join("transport-all")).unwrap();
    let forge_calls = fs::read_to_string(root.join("transport-calls")).unwrap();
    assert!(
        all.lines()
            .any(|l| l.starts_with("git ") && l.contains("fetch --quiet --no-tags origin")),
        "head fetch never ran:\n{all}"
    );
    assert!(
        forge_calls.lines().any(|l| l.contains("/check-runs")),
        "full-ci commit read never ran:\n{forge_calls}"
    );
    assert!(
        forge_calls
            .lines()
            .any(|l| l.starts_with("pr view 11") || l.contains("/pulls/11")),
        "member title/body read never ran:\n{forge_calls}"
    );
    let unbounded = fs::read_to_string(root.join("transport-unbounded")).unwrap_or_default();
    assert!(
        unbounded.is_empty(),
        "spawned without the check's deadline:\n{unbounded}"
    );
}
