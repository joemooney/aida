//! Real OpenSSH + two independent code/store clones and a bare hub.
// trace:STORY-2492 | ai:codex
// trace:STORY-2492 | ai:claude
use super::super::{approval, ledger, review_approval, review_carryover, ship};
use super::*;

fn git_ok(root: &Path, args: &[&str]) -> String {
    let mut cmd = crate::test_git::git_command();
    cmd.current_dir(root)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args);
    text(run(cmd).unwrap()).unwrap()
}
pub(super) fn write(root: &Path, rel: &str, bytes: impl AsRef<[u8]>) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
fn key(root: &Path, name: &str) -> Publisher {
    let path = root.join(name);
    let mut cmd = Command::new("ssh-keygen");
    cmd.args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&path);
    run(cmd).unwrap();
    Publisher {
        node: name.into(),
        private_key: path,
    }
}
pub(super) fn available() -> bool {
    let _env = crate::test_env::env_lock();
    if Command::new("ssh-keygen").arg("-V").output().is_err() {
        eprintln!("SKIP: ssh-keygen is absent; trusted-node signature tests require OpenSSH");
        return false;
    }
    true
}
fn env(home: &Path) -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("AIDA_CONTAINED", None),
        ("AIDA_SESSION_ID", None),
    ])
}
fn file(path: &str, bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    vec![(path.to_string(), bytes.to_vec())]
}
pub(super) struct Fixture {
    pub(super) temp: tempfile::TempDir,
    pub(super) a: PathBuf,
    pub(super) b: PathBuf,
    pub(super) spec: aida_core::Requirement,
    finding: aida_core::Requirement,
    pub(super) sha: String,
    signer: Publisher,
    third: Publisher,
}
impl Fixture {
    fn new() -> Self {
        Self::new_with_forge(false)
    }
    pub(super) fn new_with_forge(forge: bool) -> Self {
        let config = if forge {
            "[deployment]\nstore_path = '.aida-store'\n[review]\nadmission_hub = 'https://github.com/fixture/review.git'\nadmission_ref = 'refs/aida/review-admission'\n"
        } else {
            "[deployment]\nstore_path = '.aida-store'\n"
        };
        let temp = tempfile::tempdir().unwrap();
        let _env = env(&temp.path().join("bootstrap-home"));
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        git_ok(&a, &["init", "-q", "-b", "main"]);
        write(&a, ".gitignore", ".aida/\n.aida-store/\n");
        write(&a, "code", "initial change\n");
        if forge {
            write(&a, ".aida/config.toml", config);
            git_ok(&a, &["add", "-f", ".aida/config.toml"]);
        }
        git_ok(&a, &["add", "."]);
        git_ok(&a, &["commit", "-qm", "change"]);
        let sha = head(&a).unwrap();
        let codehub = temp.path().join("code.git");
        git_ok(
            temp.path(),
            &[
                "clone",
                "--bare",
                a.to_str().unwrap(),
                codehub.to_str().unwrap(),
            ],
        );
        git_ok(&a, &["remote", "add", "origin", codehub.to_str().unwrap()]);
        git_ok(&a, &["fetch", "-q", "origin"]);
        git_ok(
            temp.path(),
            &["clone", codehub.to_str().unwrap(), b.to_str().unwrap()],
        );
        for root in [&a, &b] {
            write(root, ".aida/config.toml", config);
        }
        if forge {
            // Explicitly known empty local author history on the reviewer node;
            // unreadable evidence remains a refusal in the production gate.
            write(&b, ".aida/review-authorship.jsonl", "");
            for root in [&a, &b] {
                git_ok(
                    root,
                    &[
                        "remote",
                        "set-url",
                        "origin",
                        "https://github.com/fixture/review.git",
                    ],
                );
            }
        }
        let store_a = a.join(".aida-store");
        std::fs::create_dir_all(&store_a).unwrap();
        git_ok(&store_a, &["init", "-q", "-b", "aida-store"]);
        git_ok(&store_a, &["config", "user.name", "Fixture"]);
        git_ok(
            &store_a,
            &["config", "user.email", "fixture@example.invalid"],
        );
        let mut spec = aida_core::Requirement::new("review fixture".into(), "fixture".into());
        spec.spec_id = Some(
            if forge {
                "STORY-NODEA-1"
            } else {
                "STORY-nodea-1"
            }
            .into(),
        );
        spec.agreed_id = Some("STORY-1".into());
        if forge {
            spec.execution_mode = Some(aida_core::ExecutionMode::Drain);
        }
        let mut finding = aida_core::Requirement::new("blocking finding".into(), "fixture".into());
        finding.spec_id = Some(
            if forge {
                "TASK-NODEB-2"
            } else {
                "TASK-nodeb-2"
            }
            .into(),
        );
        finding.agreed_id = Some("TASK-42".into());
        write(
            &store_a,
            if forge {
                "objects/STORY/000/STORY-NODEA-1.yaml"
            } else {
                "objects/STORY/000/STORY-nodea-1.yaml"
            },
            serde_yaml::to_string(&spec).unwrap(),
        );
        write(
            &store_a,
            if forge {
                "objects/TASK/000/TASK-NODEB-2.yaml"
            } else {
                "objects/TASK/000/TASK-nodeb-2.yaml"
            },
            serde_yaml::to_string(&finding).unwrap(),
        );
        let mut spec2 = aida_core::Requirement::new("second spec".into(), "fixture".into());
        spec2.spec_id = Some(
            if forge {
                "STORY-NODEA-3"
            } else {
                "STORY-nodea-3"
            }
            .into(),
        );
        spec2.agreed_id = Some("STORY-2".into());
        write(
            &store_a,
            if forge {
                "objects/STORY/000/STORY-NODEA-3.yaml"
            } else {
                "objects/STORY/000/STORY-nodea-3.yaml"
            },
            serde_yaml::to_string(&spec2).unwrap(),
        );
        let author_key = key(temp.path(), "nodea");
        let signer = key(temp.path(), "nodeb");
        let third = key(temp.path(), "nodec");
        let mut roster = Roster {
            version: 1,
            ..Default::default()
        };
        for config in [&author_key, &signer, &third] {
            let public = std::fs::read_to_string(config.private_key.with_extension("pub")).unwrap();
            roster.nodes.insert(
                config.node.clone(),
                public
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        write(&store_a, ROSTER, serde_json::to_vec(&roster).unwrap());
        if forge {
            aida_core::team::write_member_seats(
                &store_a,
                crate::test_env::HERMETIC_TEST_USER,
                None,
                |_| Some(vec!["advisor".into(), "reviewer".into()]),
            )
            .unwrap();
        }
        git_ok(&store_a, &["add", "."]);
        git_ok(&store_a, &["commit", "-qm", "canonical store"]);
        let hub = temp.path().join("store.git");
        git_ok(
            temp.path(),
            &[
                "clone",
                "--bare",
                store_a.to_str().unwrap(),
                hub.to_str().unwrap(),
            ],
        );
        git_ok(
            &store_a,
            &["remote", "add", "origin", hub.to_str().unwrap()],
        );
        git_ok(&store_a, &["push", "-u", "origin", "aida-store"]);
        git_ok(&b, &["clone", hub.to_str().unwrap(), ".aida-store"]);
        for root in [&a, &b] {
            git_ok(
                &root.join(".aida-store"),
                &["config", "user.name", "Fixture"],
            );
            git_ok(
                &root.join(".aida-store"),
                &["config", "user.email", "fixture@example.invalid"],
            );
        }
        write(&a, CONFIG, serde_json::to_vec(&author_key).unwrap());
        write(&b, CONFIG, serde_json::to_vec(&signer).unwrap());
        let record = authority::AuthorshipRecord {
            scope: spec.spec_id.clone().unwrap(),
            lease_id: "author-lease".into(),
            branch: "main".into(),
            tokens: vec!["session:author-session".into(), "proc:123@start".into()],
            recorded_at: chrono::Utc::now().to_rfc3339(),
            launch: None,
        };
        write(
            &a,
            ".aida/review-authorship.jsonl",
            format!("{}\n", serde_json::to_string(&record).unwrap()),
        );
        let f = Self {
            temp,
            a,
            b,
            spec,
            finding,
            sha,
            signer,
            third,
        };
        publish_authorship(
            &f.a,
            ChangeKey::pr(7),
            &["STORY-1".into()],
            &f.sha,
            Some("main"),
        )
        .unwrap();
        f.sync(&f.b);
        f
    }
    pub(super) fn sync(&self, node: &Path) {
        git_ok(
            &node.join(".aida-store"),
            &["pull", "-q", "--ff-only", "origin", "aida-store"],
        );
    }
    fn record_at(&self, decision: &str, identity: &str, sha: &str) -> rv::JsonObj {
        let identity = vec![format!("session:{identity}"), "proc:123@start".into()];
        let id = authority::write_human_receipt(sha, "STORY-1", &identity, false, true).unwrap();
        let att = rv::RecorderAttestation {
            sha: sha.to_string(),
            human_at_tty: true,
            receipt_id: Some(id),
            identity,
            ..Default::default()
        };
        // An ordinary refusal, as `aida review record --finding` writes it:
        // prose findings, no filed finding origin IDs.
        let mut obj = serde_json::json!({"verdict":decision,"reviewed_sha":sha,"reviewed_branch":"main", "recorded_by":"reviewer B", "recorded_at":chrono::Utc::now().to_rfc3339(), "recorder_attestation":att}).as_object().unwrap().clone();
        if decision != "approved" {
            obj.insert("findings".into(), serde_json::json!(["fix the regression"]));
        }
        obj
    }
    /// A refusal that also names its filed blocking finding by origin ID.
    fn record_with_origin(&self, origin: &str) -> rv::JsonObj {
        let mut obj = self.record("request-changes", "reviewer-session");
        obj.insert(
            "blocking_finding_origin_ids".into(),
            serde_json::json!([origin]),
        );
        obj
    }
    fn record(&self, decision: &str, identity: &str) -> rv::JsonObj {
        self.record_at(decision, identity, &self.sha.clone())
    }
    fn collect_for(&self, pr: u64, record: &rv::JsonObj) -> Result<usize, std::io::Error> {
        ledger::record_collected_verdict(
            &self.b,
            ChangeKey::pr(pr),
            record,
            &[(
                ledger::GateSpec::with_key("STORY-1", Some(SpecKey(self.spec.id))),
                record.clone(),
            )],
        )
    }
    fn collect(&self, record: &rv::JsonObj) -> Result<usize, std::io::Error> {
        self.collect_for(7, record)
    }
    pub(super) fn candidates(&self, root: &Path, head: &str) -> Vec<rv::RecordedVerdict> {
        ship::merge_gate_verdict_candidates(&[root], 7, &["STORY-1".into()], Some(head))
    }
    fn selected(&self) -> rv::RecordedVerdict {
        let candidates = self.candidates(&self.a, &self.sha);
        assert_eq!(candidates.len(), 1);
        candidates[0].clone()
    }
    pub(super) fn receipts(&self, root: &Path, change: &str) -> Vec<(Receipt, Signed, String)> {
        let store = root.join(".aida-store");
        load_receipts(root, &store, &head(&store).unwrap(), change).unwrap()
    }
    /// The author reworks on A and pushes a new head of the PR branch.
    fn rework(&self) -> String {
        write(&self.a, "code", "reworked change\n");
        git_ok(&self.a, &["commit", "-qam", "rework"]);
        git_ok(&self.a, &["push", "-q", "origin", "main"]);
        head(&self.a).unwrap()
    }
    /// The train's approval decision on A (the merge path after its inputs).
    fn train_decision(&self, head: &str) -> std::result::Result<Option<String>, String> {
        let candidates = self.candidates(&self.a, head);
        let policy =
            approval::ship_approval_policy(&self.a, &["STORY-1".into()], Some("main"), &candidates);
        let site = review_carryover::CarryOverSite::new(&self.a, None);
        approval::merge_path_approval_decision(
            &self.a,
            7,
            &candidates,
            &policy,
            Some(head),
            &site,
            &|| None,
        )
    }
}
fn kind_of(v: Option<rv::CurrentVerdict>) -> Option<rv::VerdictKind> {
    v.and_then(|v| rv::parse_recorded_verdict(&v.body))
        .map(|v| v.kind)
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-418.acafc49c | ai:codex
// trace:STORY-b6b1278890774ef993d2ebc327862470-418.ac4d8743 | ai:codex
// trace:STORY-2492.acafc49c | ai:codex
// trace:STORY-2492.ac4d8743 | ai:codex
#[test]
fn two_clones_collect_sync_and_gate_approval_refusal_and_revocation() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    // B is now behind the hub. Publication must preserve A's concurrent write
    // and rebase/push its immutable receipt without resetting sibling work.
    {
        let _env = env(&f.temp.path().join("home-a"));
        persist(
            &f.a.join(".aida-store"),
            &file("operator-note.txt", b"concurrent store work"),
            true,
        )
        .unwrap();
    }
    {
        let _env = env(&f.temp.path().join("home-b"));
        let record = f.record("approved", "reviewer-session");
        f.collect(&record).unwrap();
    }
    // Neither the review grant nor the local human receipt goes to A.
    std::fs::remove_dir_all(f.temp.path().join("home-b")).unwrap();
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    assert_eq!(
        std::fs::read(f.a.join(".aida-store/operator-note.txt")).unwrap(),
        b"concurrent store work"
    );
    let verdict = f.selected();
    // trace:STORY-2698 | ai:codex — clean signed publication survives remote projection.
    assert_eq!(verdict.kind, rv::VerdictKind::Approved);
    let stale = f.candidates(&f.a, &"b".repeat(40));
    assert!(stale.iter().all(|v| !v.kind.approves()));
    let uncovered = ship::merge_gate_verdict_candidates(
        &[&f.a],
        7,
        &["STORY-1".into(), "TASK-42".into()],
        Some(&f.sha),
    );
    assert!(
        uncovered.iter().all(|v| !v.kind.approves()),
        "a receipt cannot authorize another canonical spec on the same PR"
    );
    let attestation = verdict.attestation.as_ref().unwrap();
    assert!(
        !attestation.human_at_tty && attestation.grant_id.is_none(),
        "remote credit confers no local human receipt or seat grant"
    );
    assert!(ship::approval_head_refusal(&[verdict.clone()], Some(&f.sha)).is_none());
    assert!(ship::approval_head_refusal(&[verdict.clone()], Some(&"b".repeat(40))).is_some());
    assert!(review_approval::independent_approval_authorized(
        &f.a,
        &["STORY-1".into()],
        Some("main"),
        &verdict
    ));
    let bytes = git(
        &f.a.join(".aida-store"),
        &[
            "grep",
            "-n",
            "grant_binding\\|grant_id\\|human_at_tty\\|receipt_id",
            "HEAD",
            "--",
            CHANGES,
        ],
    )
    .unwrap_err();
    assert!(
        bytes.to_string().contains("refused"),
        "no raw authority evidence in synced publications"
    );
    drop(_env);
    {
        let _env = env(&f.temp.path().join("home-b"));
        f.collect(&f.record("request-changes", "reviewer-session"))
            .unwrap();
    }
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    let refusal = f.selected();
    assert!(refusal.kind.blocks_done());
    assert_eq!(refusal.findings, vec!["fix the regression"]);
    assert!(
        current(&f.a, &["TASK-42"]).is_none(),
        "finding identities do not accidentally become review subjects"
    );
    // A newer local approval must not erase the synced refusal, and the
    // disagreement at the same head is reported (AC3).
    let local = serde_json::json!({"verdict":"approved", "reviewed_sha":f.sha,"recorded_at":chrono::Utc::now().to_rfc3339()}).as_object().unwrap().clone();
    ledger::store::append_one(
        &f.a,
        ledger::event::verdict_event(&f.a, ChangeKey::pr(7), &local, None),
    )
    .unwrap();
    take_reports();
    assert!(f.selected().kind.blocks_done());
    // The merge gate itself reports the disagreement (AC3), not just the
    // pure formatter.
    let reports = take_reports();
    assert!(
        reports
            .iter()
            .any(|r| r.contains("PR-7") && r.contains("disagreement")),
        "{reports:?}"
    );
    // An unknown PR head falls back to the branch-resolved head: the synced
    // refusal still excludes the local approval (finding 4).
    let headless = ship::merge_gate_verdict_candidates(&[&f.a], 7, &["STORY-1".into()], None);
    assert!(
        !headless.is_empty() && headless.iter().all(|v| v.kind.blocks_done()),
        "{headless:?}"
    );
    // Remove B from the CURRENT committed roster. Older signatures stop counting.
    let store = f.a.join(".aida-store");
    let mut roster = roster(&store, &head(&store).unwrap()).unwrap();
    roster.nodes.remove("nodeb");
    persist(
        &store,
        &file(ROSTER, &serde_json::to_vec(&roster).unwrap()),
        false,
    )
    .unwrap();
    assert!(verified_identity(&f.a, &verdict).is_err());
    // Revoked-only evidence is absent with a report, never a standing
    // refusal: B's receipts no longer answer, and a fresh local review does
    // (finding 5).
    take_reports();
    assert!(current(&f.a, &["PR-7"]).is_none());
    let reports = take_reports();
    assert!(reports.iter().any(|r| r.contains("revoked")), "{reports:?}");
    assert!(f.candidates(&f.a, &f.sha).iter().all(|v| v
        .attestation
        .as_ref()
        .and_then(|a| a.receipt_id.as_deref())
        .is_none_or(|id| !id.starts_with("publication:"))));
}

/// Request-changes -> rework -> re-review by another node -> A's train approval
/// decision. This is decision-boundary coverage, not an AC5 train entry test. A stale refusal at the old head neither blocks the new head nor trips
/// a cross-head disagreement, and a second PR on the same spec is selected
/// per change (findings 3, 5, 6, 9 of round 2).
#[test]
fn rework_cycle_rereviews_on_another_node_and_checks_the_train_approval_decision() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    {
        let _env = env(&f.temp.path().join("home-b"));
        f.collect(&f.record("request-changes", "reviewer-session"))
            .unwrap();
    }
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    assert!(f.selected().kind.blocks_done());
    assert!(
        f.train_decision(&f.sha).is_err(),
        "the train refuses the refused head"
    );
    // Rework: AIDA's push of the new head republishes author sessions.
    let h2 = f.rework();
    publish_change_authorship(&f.a, ChangeKey::pr(7), "main", &["STORY-1".into()]);
    let store_a = f.a.join(".aida-store");
    let authored_heads = git_ok(
        &store_a,
        &[
            "grep",
            "-c",
            &h2,
            "HEAD",
            "--",
            &evidence_dir("PR-7", AUTHORSHIPS),
        ],
    );
    assert!(
        !authored_heads.is_empty(),
        "authorship bound to the reworked head"
    );
    drop(_env);
    // Node C (a different reviewer on the second machine) approves H2.
    {
        let _env = env(&f.temp.path().join("home-b"));
        write(&f.b, CONFIG, serde_json::to_vec(&f.third).unwrap());
        git_ok(&f.b, &["fetch", "-q", "origin"]);
        f.sync(&f.b);
        f.collect(&f.record_at("approved", "reviewer-c", &h2))
            .unwrap();
        assert_eq!(f.receipts(&f.b, "PR-7").len(), 2, "approval published");
    }
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    // Head-unfiltered readers see the current head only (finding 5).
    assert_eq!(
        kind_of(current(&f.a, &["PR-7"])),
        Some(rv::VerdictKind::Approved),
        "B's stale H1 refusal does not block C's H2 approval"
    );
    assert!(f.candidates(&f.a, &h2).iter().all(|v| v.kind.approves()));
    // The approval decision accepts H2; this does not execute a train merge.
    let landed = f.train_decision(&h2);
    assert!(
        matches!(&landed, Ok(Some(pin)) if crate::forge::head_matches_pin(pin, &h2)),
        "{landed:?}"
    );
    drop(_env);
    // A second PR carries the same spec and is refused there. Selection is per
    // change: PR-7 keeps its approval, the spec reads its newest change.
    {
        let _env = env(&f.temp.path().join("home-a"));
        publish_authorship(
            &f.a,
            ChangeKey::pr(8),
            &["STORY-1".into()],
            &h2,
            Some("main"),
        )
        .unwrap();
    }
    {
        let _env = env(&f.temp.path().join("home-b"));
        f.sync(&f.b);
        f.collect_for(8, &f.record_at("request-changes", "reviewer-c", &h2))
            .unwrap();
    }
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    assert_eq!(
        kind_of(current(&f.a, &["PR-7"])),
        Some(rv::VerdictKind::Approved)
    );
    assert!(kind_of(current(&f.a, &["STORY-1"])).is_some_and(|k| k.blocks_done()));
    assert!(rv::read_recorded_verdict(&f.a, "STORY-1").is_some_and(|v| v.kind.blocks_done()));
    // A node that cannot resolve the branch while PR-7's publications name two
    // heads treats them as absent with a report, not a standing refusal;
    // a head-bound gate still sees the publication at its head (finding 5).
    git_ok(&f.b, &["checkout", "-q", "--detach"]);
    git_ok(&f.b, &["update-ref", "-d", "refs/heads/main"]);
    git_ok(&f.b, &["update-ref", "-d", "refs/remotes/origin/main"]);
    take_reports();
    assert!(current(&f.b, &["PR-7"]).is_none());
    let reports = take_reports();
    assert!(
        reports.iter().any(|r| r.contains("several heads")),
        "{reports:?}"
    );
    assert_eq!(
        kind_of(current_at_head(&f.b, &["PR-7"], &h2)),
        Some(rv::VerdictKind::Approved)
    );
}

/// One malformed publication refuses its own change only; a change or spec
/// with no publications reads nothing from the publication store (finding 4).
#[test]
fn a_malformed_publication_refuses_only_its_own_change() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    {
        let _env = env(&f.temp.path().join("home-b"));
        f.collect(&f.record("approved", "reviewer-session"))
            .unwrap();
    }
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    let store = f.a.join(".aida-store");
    persist(
        &store,
        &file(
            &evidence_path("PR-99", PUBLICATIONS, "nodeb", Uuid::new_v4()),
            b"not json",
        ),
        true,
    )
    .unwrap();
    assert_eq!(
        kind_of(current(&f.a, &["PR-7"])),
        Some(rv::VerdictKind::Approved)
    );
    assert!(kind_of(current(&f.a, &["PR-99"])).is_some_and(|k| k.blocks_done()));
    assert!(current(&f.a, &["PR-5"]).is_none());
    assert!(current(&f.a, &["TASK-42"]).is_none());
}

/// An ordinary refusal (prose findings, no origin IDs) publishes through the
/// real collector; a named origin ID that does not resolve leaves the
/// verdict recorded and its publication a reported pending state (round 3
/// finding 2).
#[test]
fn an_ordinary_refusal_publishes_and_an_unresolvable_origin_stays_pending() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    {
        let _env = env(&f.temp.path().join("home-b"));
        let unresolvable = f.record_with_origin("TASK-nodeb-404");
        assert!(
            f.collect(&unresolvable).unwrap() > 0,
            "the local verdict is written"
        );
        assert!(f.receipts(&f.b, "PR-7").is_empty(), "nothing was published");
        assert!(
            publish(&f.b, ChangeKey::pr(7), &unresolvable, &["STORY-1".into()])
                .unwrap_err()
                .to_string()
                .contains("finding origin")
        );
        let filed = f.finding.spec_id.clone().unwrap();
        f.collect(&f.record_with_origin(&filed)).unwrap();
        assert_eq!(f.receipts(&f.b, "PR-7").len(), 1);
        f.collect(&f.record("request-changes", "reviewer-session"))
            .unwrap();
        assert_eq!(
            f.receipts(&f.b, "PR-7").len(),
            2,
            "a refusal without origin IDs publishes"
        );
    }
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    let refusal = f.selected();
    assert!(refusal.kind.blocks_done());
    assert_eq!(refusal.findings, vec!["fix the regression"]);
}

/// A two-spec change publishes from the direct recorder (one named spec) and
/// from an adopted verdict file (no spec): the receipt covers the change's
/// bound spec set, and a rework head naming another ID does not grow it
/// (round 3 finding 3).
#[test]
fn a_two_spec_change_publishes_from_the_direct_recorder() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    {
        let _env = env(&f.temp.path().join("home-a"));
        publish_authorship(
            &f.a,
            ChangeKey::pr(11),
            &["STORY-1".into(), "STORY-2".into()],
            &f.sha,
            Some("main"),
        )
        .unwrap();
        // A rework tip whose commit names an extra ID.
        publish_authorship(
            &f.a,
            ChangeKey::pr(11),
            &["STORY-1".into(), "TASK-42".into()],
            &f.sha,
            Some("main"),
        )
        .unwrap();
    }
    let path = f.b.join(".aida/review-verdicts/PR-11.json");
    {
        let _env = env(&f.temp.path().join("home-b"));
        f.sync(&f.b);
        ledger::record_verdict_at(
            &path,
            &f.record("request-changes", "reviewer-session"),
            Some(&ledger::GateSpec::with_key(
                "STORY-1",
                Some(SpecKey(f.spec.id)),
            )),
        )
        .unwrap();
        assert_eq!(
            f.receipts(&f.b, "PR-11").len(),
            1,
            "direct recorder published"
        );
        ledger::adopt_verdict_file(&path, &f.record("approved", "reviewer-session")).unwrap();
        let receipts = f.receipts(&f.b, "PR-11");
        assert_eq!(receipts.len(), 2, "adopted verdict published");
        assert!(
            receipts.iter().all(|(r, ..)| r.specs.len() == 2),
            "bound spec set"
        );
    }
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    assert_eq!(
        kind_of(current(&f.a, &["PR-11", "STORY-1", "STORY-2"])),
        Some(rv::VerdictKind::Approved)
    );
    let candidates = ship::merge_gate_verdict_candidates(
        &[&f.a],
        11,
        &["STORY-1".into(), "STORY-2".into()],
        Some(&f.sha),
    );
    assert!(
        !candidates.is_empty() && candidates.iter().all(|v| v.kind.approves()),
        "{candidates:?}"
    );
}

/// Ship opens the change from the primary checkout (HEAD = main) while the
/// PR branch tip is elsewhere: authorship binds the branch tip (finding 2),
/// and the open succeeds even when publication cannot (finding 8).
#[test]
fn opening_from_a_non_head_checkout_binds_the_branch_tip() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let _env = env(&f.temp.path().join("home-a"));
    git_ok(&f.a, &["checkout", "-q", "-b", "feature"]);
    write(&f.a, "code", "feature work\n");
    git_ok(&f.a, &["commit", "-qam", "feature"]);
    let tip = head(&f.a).unwrap();
    git_ok(&f.a, &["push", "-q", "origin", "feature"]);
    git_ok(&f.a, &["checkout", "-q", "main"]);
    assert_ne!(head(&f.a).unwrap(), tip);
    let mut forge = crate::forge::fake::RecordingForge::new();
    forge.opens_as = Some(9);
    let req = crate::forge::OpenChange {
        branch: "feature".into(),
        base: "main".into(),
        title: "t".into(),
        body: String::new(),
        draft: false,
    };
    let change = ledger::open_change_bound(&forge, &f.a, req.clone(), &["STORY-1".into()]).unwrap();
    assert_eq!(change.id, 9);
    let store = f.a.join(".aida-store");
    let names = list(
        &store,
        &head(&store).unwrap(),
        &evidence_dir("PR-9", AUTHORSHIPS),
    )
    .unwrap();
    assert_eq!(names.len(), 1);
    let signed: Signed =
        serde_json::from_str(&git_ok(&store, &["show", &format!("HEAD:{}", names[0])])).unwrap();
    let evidence: Authorship = serde_json::from_str(&signed.payload).unwrap();
    assert_eq!(evidence.head, tip, "authorship names the pushed branch tip");
    assert_eq!(evidence.branch.as_deref(), Some("feature"));
    // An unresolvable branch reports a pending publication; the open stands.
    let mut forge = crate::forge::fake::RecordingForge::new();
    forge.opens_as = Some(10);
    let req = crate::forge::OpenChange {
        branch: "never-pushed".into(),
        ..req
    };
    assert_eq!(
        ledger::open_change_bound(&forge, &f.a, req, &["STORY-1".into()])
            .unwrap()
            .id,
        10
    );
}

/// AC3: the disagreement report is produced for the same head only.
// trace:STORY-b6b1278890774ef993d2ebc327862470-418.ac596603 | ai:codex
// trace:STORY-2492.ac596603 | ai:codex
#[test]
fn disagreement_is_reported_for_the_same_head_only() {
    let parse = |kind: &str, sha: &str| {
        rv::parse_recorded_verdict(
            &serde_json::json!({"verdict":kind,"reviewed_sha":sha}).to_string(),
        )
        .unwrap()
    };
    let h1 = "1".repeat(40);
    let h2 = "2".repeat(40);
    let report = disagreement(
        "PR-7",
        &parse("approved", &h1),
        &parse("request-changes", &h1),
    )
    .unwrap();
    assert!(
        report.contains("PR-7") && report.contains("disagreement"),
        "{report}"
    );
    assert!(disagreement(
        "PR-7",
        &parse("approved", &h1),
        &parse("request-changes", &h2)
    )
    .is_none());
    assert!(disagreement("PR-7", &parse("approved", &h1), &parse("approved", &h1)).is_none());
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-418.ac4d8743 | ai:codex
#[test]
fn signed_receipts_refuse_resumed_author_tampering_and_invalid_local_authority() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let _env = env(&f.temp.path().join("home-b"));
    let labels = ["STORY-1".to_string()];
    let own = f.record("approved", "author-session");
    assert!(publish(&f.b, ChangeKey::pr(7), &own, &labels)
        .unwrap_err()
        .to_string()
        .contains("author across nodes"));
    let mut invalid = f.record("approved", "reviewer-session");
    invalid.remove(rv::ATTESTATION_KEY);
    assert!(publish(&f.b, ChangeKey::pr(7), &invalid, &labels).is_err());
    // No portable author evidence: refuse rather than claiming independence.
    let store_b = f.b.join(".aida-store");
    git_ok(&store_b, &["rm", "-qr", &evidence_dir("PR-7", AUTHORSHIPS)]);
    git_ok(&store_b, &["commit", "-qm", "remove author evidence"]);
    assert!(publish(
        &f.b,
        ChangeKey::pr(7),
        &f.record("approved", "reviewer-session"),
        &labels
    )
    .unwrap_err()
    .to_string()
    .contains("missing portable author"));
    // Restore for a valid publication, then corrupt its signed payload.
    git_ok(&store_b, &["revert", "--no-edit", "HEAD"]);
    publish(
        &f.b,
        ChangeKey::pr(7),
        &f.record("approved", "reviewer-session"),
        &labels,
    )
    .unwrap();
    let (mut receipt, _, rel) = f.receipts(&f.b, "PR-7").pop().unwrap();
    let path = store_b.join(rel);
    let mut signed: Signed = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    receipt.head = "c".repeat(40);
    signed.payload = serde_json::to_string(&receipt).unwrap();
    assert!(verify(
        &receipt,
        &signed,
        &roster(&store_b, &head(&store_b).unwrap()).unwrap()
    )
    .is_err());
    receipt.authors.clear();
    let signed = sign(&f.signer, &receipt).unwrap();
    assert!(verify(
        &receipt,
        &signed,
        &roster(&store_b, &head(&store_b).unwrap()).unwrap()
    )
    .is_err());
    receipt.version = 99;
    let signed = sign(&f.signer, &receipt).unwrap();
    assert!(verify(
        &receipt,
        &signed,
        &roster(&store_b, &head(&store_b).unwrap()).unwrap()
    )
    .is_err());
    // A forged committed publication must refuse positive cache fallback.
    std::fs::write(&path, serde_json::to_vec(&signed).unwrap()).unwrap();
    git_ok(&store_b, &["add", "-A"]);
    git_ok(
        &store_b,
        &["commit", "-qm", "malformed publication fixture"],
    );
    let current = current(&f.b, &["PR-7"]).unwrap();
    assert!(rv::parse_recorded_verdict(&current.body)
        .unwrap()
        .kind
        .blocks_done());
    // A committed symlink is never read as a receipt blob.
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("/dev/null", &path).unwrap();
    git_ok(&store_b, &["add", "-A"]);
    git_ok(&store_b, &["commit", "-qm", "unsafe publication fixture"]);
    let refused = super::current(&f.b, &["PR-7"]).unwrap();
    assert!(rv::parse_recorded_verdict(&refused.body)
        .unwrap()
        .kind
        .blocks_done());
}

#[test]
fn roster_conflict_never_auto_merges_revocation_away() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let _env = env(&f.temp.path().join("home-a"));
    let a = f.a.join(".aida-store");
    let b = f.b.join(".aida-store");
    // B has a pending operator edit, while A independently revokes B.
    let mut pending = roster(&b, &head(&b).unwrap()).unwrap();
    pending.nodes.remove("nodea");
    write(&b, ROSTER, serde_json::to_vec(&pending).unwrap());
    git_ok(&b, &["add", ROSTER]);
    git_ok(&b, &["commit", "-qm", "pending operator roster edit"]);
    let mut revoked = roster(&a, &head(&a).unwrap()).unwrap();
    revoked.nodes.remove("nodeb");
    persist(
        &a,
        &file(ROSTER, &serde_json::to_vec(&revoked).unwrap()),
        false,
    )
    .unwrap();
    // The ordinary pull helper (run inside the store writer lock) aborts a
    // conflicting rebase and retains both local commits for operator repair;
    // it must never merge away revocation.
    let error = persist(
        &b,
        &file("notes/pending-receipt.json", b"immutable evidence"),
        true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("rebase"), "{error:#}");
    let retained: Roster =
        serde_json::from_str(&git_ok(&b, &["show", &format!("HEAD:{ROSTER}")])).unwrap();
    assert!(retained.nodes.contains_key("nodeb") && !retained.nodes.contains_key("nodea"));
    assert_eq!(
        git_ok(&b, &["show", "HEAD:notes/pending-receipt.json"]),
        "immutable evidence"
    );
    let current = roster(&a, &head(&a).unwrap()).unwrap();
    assert!(!current.nodes.contains_key("nodeb"));
    // A raced roster push names the idempotent rerun as its recovery path.
    let mut raced = roster(&b, &head(&b).unwrap()).unwrap();
    raced.nodes.remove("nodec");
    let error = persist(
        &b,
        &file(ROSTER, &serde_json::to_vec(&raced).unwrap()),
        false,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("rerun the same command"),
        "{error:#}"
    );
}

// trace:STORY-2492 | ai:codex
// trace:STORY-b6b1278890774ef993d2ebc327862470-418.ac8cff0f | ai:codex
// trace:STORY-2492.ac8cff0f | ai:codex
#[test]
fn synced_publication_skips_watcher_without_a_local_verdict_directory() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    {
        let _env = env(&f.temp.path().join("home-b"));
        let record = f.record("request-changes", "reviewer-session");
        f.collect(&record).unwrap();
    }
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    assert!(!f.a.join(".aida/review-verdicts").exists());
    assert_eq!(
        crate::review_watch::synced_history_precheck(&f.a, 7, &f.sha)
            .unwrap()
            .as_deref(),
        Some("head already has a recorded review")
    );
    write(
        &f.a,
        ".aida/review-verdicts/PR-7.json",
        serde_json::to_vec(&serde_json::json!({
            "verdict":"approved", "reviewed_sha":f.sha, "recorded_by":"local cache"
        }))
        .unwrap(),
    );
    take_reports();
    assert!(
        crate::review_watch::synced_history_precheck(&f.a, 7, &f.sha)
            .unwrap()
            .is_some()
    );
    assert!(take_reports()
        .iter()
        .any(|report| report.contains("contradictory local watcher history")));
    assert!(
        crate::review_watch::synced_history_precheck(&f.a, 7, &"a".repeat(40))
            .unwrap()
            .is_none(),
        "stale publications do not suppress a new head"
    );
}

// trace:STORY-2696 | ai:codex
#[test]
fn story_2696_signed_no_carry_projection_and_supersession() {
    assert!(available(), "OpenSSH is required for this regression");
    let f = Fixture::new();
    {
        let _env = env(&f.temp.path().join("home-b"));
        let mut record = f.record("approved", "reviewer-session");
        let mut att: rv::RecorderAttestation =
            serde_json::from_value(record["recorder_attestation"].clone()).unwrap();
        att.carry_eligible = false;
        att.receipt_id = Some(
            authority::write_human_receipt(&f.sha, "STORY-1", &att.identity, false, false).unwrap(),
        );
        record.insert(
            "recorder_attestation".into(),
            serde_json::to_value(att).unwrap(),
        );
        f.collect(&record).unwrap();
    }
    {
        let _env = env(&f.temp.path().join("home-a"));
        f.sync(&f.a);
        let mut verdict = f.selected();
        assert!(!verdict.attestation.as_ref().unwrap().carry_eligible);
        assert!(!authority::verified_authority_keys(&f.a, &[verdict.clone()]).is_empty());
        assert!(ship::approval_head_refusal(&[verdict.clone()], Some(&f.sha)).is_none());
        assert!(matches!(
            ship::approval_head_refusal(&[verdict.clone()], Some(&"b".repeat(40))),
            Some(ship::ApprovalHeadRefusal::NoCarry { .. })
        ));
        verdict.attestation.as_mut().unwrap().carry_eligible = true;
        assert!(
            authority::verified_authority_keys(&f.a, &[verdict]).is_empty(),
            "projection tampering cannot restore eligibility"
        );
    }
    {
        let _env = env(&f.temp.path().join("home-b"));
        f.collect(&f.record("approved", "reviewer-session"))
            .unwrap();
    }
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    let verdict = f.selected();
    assert!(verdict.attestation.as_ref().unwrap().carry_eligible);
    assert!(!authority::verified_authority_keys(&f.a, &[verdict]).is_empty());
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_synced_wait_closes_locally_without_rewriting_signed_review() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    {
        let _env = env(&f.temp.path().join("home-b"));
        let mut record = f.record("waiting", "reviewer-session");
        record.remove("findings");
        record.insert("summary".into(), "waiting for batch CI".into());
        f.collect(&record).unwrap();
    }
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    assert!(!f.a.join(".aida/review-verdicts").exists());
    assert!(rv::verdict_keys(&f.a).contains(&"PR-7".into()));
    // Independent local ledger rounds differ from the effective signed source.
    // They remain intact in raw projection while the signed wait closes locally.
    for key in ["STORY-1", "PR-7"] {
        let record = serde_json::json!({
            "verdict":"request-changes", "reviewed_sha":f.sha,
            "summary":"independent local findings", "findings":["local defect"]
        });
        let spec = (key == "STORY-1").then(|| crate::gate::ledger::GateSpec::resolve(&f.a, key));
        let root = crate::gate::ledger::store_root(&f.a).unwrap();
        crate::gate::ledger::store::append_one(
            &root,
            crate::gate::ledger::event::verdict_event(
                &root,
                ChangeKey::pr(7),
                record.as_object().unwrap(),
                spec.as_ref(),
            ),
        )
        .unwrap();
    }
    let published = current(&f.a, &["STORY-1"]).unwrap().body;
    let keys = vec!["STORY-1".into(), "PR-7".into()];
    crate::review_waiting::reconcile_waiting_verdicts_with(&f.a, Some(&keys), |c| {
        assert_eq!(c, ChangeKey::pr(7));
        Some((crate::forge::ChangeState::Merged, f.sha.clone()))
    });
    for key in ["STORY-1", "PR-7"] {
        let v = rv::read_recorded_verdict(&f.a, key).unwrap();
        assert!(v.is_closed(), "{key}: {v:?}");
        assert_eq!(v.kind, rv::VerdictKind::Waiting);
    }
    assert_eq!(current(&f.a, &["STORY-1"]).unwrap().body, published);
    for key in ["STORY-1", "PR-7"] {
        assert_eq!(
            crate::gate::ledger::project::newest_verdict(&f.a, key).unwrap()["summary"],
            "independent local findings"
        );
    }
    crate::queue_cmd::refuse_waiting_review(&f.a, "STORY-1", "STORY-1", None).unwrap();
    drop(_env);
    {
        let _env = env(&f.temp.path().join("home-b"));
        let mut next = f.record("waiting", "reviewer-session");
        next.remove("findings");
        next.insert("summary".into(), "new independent evidence".into());
        f.collect(&next).unwrap();
    }
    let _env = env(&f.temp.path().join("home-a"));
    f.sync(&f.a);
    for key in ["STORY-1", "PR-7"] {
        let v = rv::read_recorded_verdict(&f.a, key).unwrap();
        assert!(
            !v.is_closed(),
            "a new signed round invalidates the local annotation"
        );
        assert_eq!(v.summary.as_deref(), Some("new independent evidence"));
    }
}
