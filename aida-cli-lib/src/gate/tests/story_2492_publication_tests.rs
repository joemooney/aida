//! Real OpenSSH + two independent code/store clones and a bare hub.
// trace:STORY-2492 | ai:codex
use super::super::{ledger, review_approval, ship};
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
fn write(root: &Path, rel: &str, bytes: impl AsRef<[u8]>) {
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
fn available() -> bool {
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
struct Fixture {
    temp: tempfile::TempDir,
    a: PathBuf,
    b: PathBuf,
    spec: aida_core::Requirement,
    finding: aida_core::Requirement,
    sha: String,
    signer: Publisher,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let _env = env(&temp.path().join("bootstrap-home"));
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        git_ok(&a, &["init", "-q", "-b", "main"]);
        write(&a, ".gitignore", ".aida/\n.aida-store/\n");
        write(&a, "code", "initial change\n");
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
        git_ok(
            temp.path(),
            &["clone", codehub.to_str().unwrap(), b.to_str().unwrap()],
        );
        for root in [&a, &b] {
            write(
                root,
                ".aida/config.toml",
                "[deployment]\nstore_path = '.aida-store'\n",
            );
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
        spec.spec_id = Some("STORY-nodea-1".into());
        spec.agreed_id = Some("STORY-1".into());
        let mut finding = aida_core::Requirement::new("blocking finding".into(), "fixture".into());
        finding.spec_id = Some("TASK-nodeb-2".into());
        finding.agreed_id = Some("TASK-42".into());
        write(
            &store_a,
            "objects/STORY/000/STORY-nodea-1.yaml",
            serde_yaml::to_string(&spec).unwrap(),
        );
        write(
            &store_a,
            "objects/TASK/000/TASK-nodeb-2.yaml",
            serde_yaml::to_string(&finding).unwrap(),
        );
        let author_key = key(temp.path(), "nodea");
        let signer = key(temp.path(), "nodeb");
        let mut roster = Roster {
            version: 1,
            ..Default::default()
        };
        for config in [&author_key, &signer] {
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
        };
        publish_authorship(&f.a, ChangeKey::pr(7), &["STORY-1".into()]).unwrap();
        f.sync(&f.b);
        f
    }
    fn sync(&self, node: &Path) {
        git_ok(
            &node.join(".aida-store"),
            &["pull", "--ff-only", "origin", "aida-store"],
        );
    }
    fn record(&self, decision: &str, identity: &str) -> rv::JsonObj {
        let identity = vec![format!("session:{identity}"), "proc:123@start".into()];
        let id = authority::write_human_receipt(&self.sha, "STORY-1", &identity, false).unwrap();
        let att = rv::RecorderAttestation {
            sha: self.sha.clone(),
            human_at_tty: true,
            receipt_id: Some(id),
            identity,
            ..Default::default()
        };
        let mut obj = serde_json::json!({"verdict":decision,"reviewed_sha":self.sha,"reviewed_branch":"main", "recorded_by":"reviewer B", "recorded_at":chrono::Utc::now().to_rfc3339(), "recorder_attestation":att}).as_object().unwrap().clone();
        if decision != "approved" {
            obj.insert("findings".into(), serde_json::json!(["fix the regression"]));
            obj.insert(
                "blocking_finding_origin_ids".into(),
                serde_json::json!([self.finding.spec_id]),
            );
        }
        obj
    }
    fn collect(&self, record: &rv::JsonObj) -> Result<usize, std::io::Error> {
        ledger::record_collected_verdict(
            &self.b,
            ChangeKey::pr(7),
            record,
            &[(
                ledger::GateSpec::with_key("STORY-1", Some(SpecKey(self.spec.id))),
                record.clone(),
            )],
        )
    }
    fn selected(&self) -> rv::RecordedVerdict {
        let candidates = ship::merge_gate_verdict_candidates(
            &[&self.a],
            7,
            &["STORY-1".into()],
            Some(&self.sha),
        );
        assert_eq!(candidates.len(), 1);
        candidates[0].clone()
    }
}

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
            "operator-note.txt",
            b"concurrent store work",
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
    let stale =
        ship::merge_gate_verdict_candidates(&[&f.a], 7, &["STORY-1".into()], Some(&"b".repeat(40)));
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
            PUBLICATIONS,
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
    let projected = current(&f.a, &["TASK-42"]).is_none();
    assert!(
        projected,
        "finding identities do not accidentally become review subjects"
    );
    // A newer local approval must not erase the synced refusal.
    let local = serde_json::json!({"verdict":"approved", "reviewed_sha":f.sha,"recorded_at":chrono::Utc::now().to_rfc3339()}).as_object().unwrap().clone();
    ledger::store::append_one(
        &f.a,
        ledger::event::verdict_event(&f.a, ChangeKey::pr(7), &local, None),
    )
    .unwrap();
    assert!(f.selected().kind.blocks_done());
    // Remove B from the CURRENT committed roster. Older signatures stop counting.
    let store = f.a.join(".aida-store");
    let mut roster = roster(&store, &head(&store).unwrap()).unwrap();
    roster.nodes.remove("nodeb");
    persist(&store, ROSTER, &serde_json::to_vec(&roster).unwrap(), false).unwrap();
    assert!(verified_identity(&f.a, &verdict).is_err());
    assert!(f.selected().kind.blocks_done());
}

#[test]
fn signed_receipts_refuse_resumed_author_tampering_and_invalid_local_authority() {
    if !available() {
        return;
    }
    let f = Fixture::new();
    let _env = env(&f.temp.path().join("home-b"));
    let own = f.record("approved", "author-session");
    assert!(f
        .collect(&own)
        .unwrap_err()
        .to_string()
        .contains("author across nodes"));
    let mut invalid = f.record("approved", "reviewer-session");
    invalid.remove(rv::ATTESTATION_KEY);
    assert!(f.collect(&invalid).is_err());
    // No portable author evidence: refuse rather than claiming independence.
    let store_b = f.b.join(".aida-store");
    git_ok(&store_b, &["rm", "-r", AUTHORSHIPS]);
    git_ok(&store_b, &["commit", "-qm", "remove author evidence"]);
    assert!(publish(
        &f.b,
        ChangeKey::pr(7),
        &f.record("approved", "reviewer-session"),
        &["STORY-1".into()]
    )
    .unwrap_err()
    .to_string()
    .contains("missing portable author"));
    // Restore for a valid publication, then corrupt its signed payload.
    git_ok(&store_b, &["revert", "--no-edit", "HEAD"]);
    f.collect(&f.record("approved", "reviewer-session"))
        .unwrap();
    let (mut receipt, path) = publications(&f.b).unwrap().pop().unwrap();
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
    persist(&a, ROSTER, &serde_json::to_vec(&revoked).unwrap(), false).unwrap();
    // The ordinary pull helper aborts a conflicting rebase and retains both
    // local commits for operator repair; it must never merge away revocation.
    let error = persist(
        &b,
        "notes/pending-receipt.json",
        b"immutable evidence",
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
}
