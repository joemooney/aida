// trace:STORY-2492 | ai:codex
use super::*;
use std::sync::{Arc, Barrier};
fn ok(root: &Path, args: &[&str]) -> String {
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
fn write(root: &Path, path: &str, value: impl AsRef<[u8]>) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value).unwrap();
}
struct Fixture {
    temp: tempfile::TempDir,
    a: PathBuf,
    b: PathBuf,
    hub: Hub,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let _bootstrap_env = crate::test_env::EnvVarsGuard::apply(&[
            ("GIT_CONFIG_GLOBAL", None),
            ("AIDA_CONTAINED", None),
            ("AIDA_HOME", Some(temp.path().to_str().unwrap())),
        ]);
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        ok(&a, &["init", "-q", "-b", "main"]);
        write(&a, "code", "fixture");
        write(&a, ".aida/config.toml", "[deployment]\nstore_path = '.aida-store'\n[review]\nadmission_hub = 'https://github.com/fixture/review.git'\nadmission_ref = 'refs/aida/review-admission'\n");
        ok(&a, &["add", "."]);
        ok(&a, &["commit", "-qm", "initial"]);
        ok(
            temp.path(),
            &["clone", a.to_str().unwrap(), b.to_str().unwrap()],
        );
        let store = a.join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        ok(&store, &["init", "-q", "-b", "aida-store"]);
        let mut roster = Roster {
            version: 1,
            ..Default::default()
        };
        for (root, node) in [(&a, "a"), (&b, "b")] {
            let key = temp.path().join(format!("key-{node}"));
            let mut cmd = Command::new("ssh-keygen");
            cmd.args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&key);
            run(cmd).unwrap();
            let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
            roster.nodes.insert(
                node.into(),
                public
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            write(
                root,
                CONFIG,
                serde_json::to_vec(&Publisher {
                    node: node.into(),
                    private_key: key,
                })
                .unwrap(),
            );
        }
        write(&store, ROSTER, serde_json::to_vec(&roster).unwrap());
        ok(&store, &["add", "."]);
        ok(&store, &["commit", "-qm", "roster"]);
        ok(&b, &["clone", store.to_str().unwrap(), ".aida-store"]);
        let bare = temp.path().join("hub.git");
        ok(
            temp.path(),
            &[
                "clone",
                "--bare",
                a.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        write(
            temp.path(),
            "gitconfig",
            format!(
                "[url \"{}\"]\n insteadOf = https://github.com/fixture/review.git\n",
                bare.display()
            ),
        );
        Self {
            temp,
            a,
            b,
            hub: Hub::new("https://github.com/fixture/review.git", REF).unwrap(),
        }
    }
    fn environment(&self) -> crate::test_env::EnvVarsGuard {
        crate::test_env::EnvVarsGuard::apply(&[
            (
                "GIT_CONFIG_GLOBAL",
                Some(self.temp.path().join("gitconfig").to_str().unwrap()),
            ),
            ("AIDA_SESSION_ID", Some("fixture-session")),
            ("AIDA_CONTAINED", None),
            ("AIDA_HOME", Some(self.temp.path().to_str().unwrap())),
        ])
    }
    fn step(&self, root: &Path, key: &str, head: &str, token: Uuid, state: State) -> Result<bool> {
        let evidence =
            (state != State::Reserved).then(|| "owner observed process spawn/exit receipt".into());
        update(root, &self.hub, key, head, token, state, evidence)
    }
}
// trace:STORY-b6b1278890774ef993d2ebc327862470-418.ac8cff0f | ai:codex
#[test]
fn story2492_admission_two_nodes_race_exactly_one_wins() {
    let f = Fixture::new();
    let _env = f.environment();
    use std::os::unix::fs::PermissionsExt;
    let program = f.temp.path().join("dispatcher");
    let marker = f.temp.path().join("spawns");
    std::fs::write(
        &program,
        r#"#!/bin/sh
printf 'spawn\n' >> "$1"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = [&f.a, &f.b]
            .into_iter()
            .map(|root| {
                let barrier = barrier.clone();
                let f = &f;
                scope.spawn(move || {
                    barrier.wait();
                    crate::review_watch::fixture_admit_dispatch(
                        root,
                        &"a".repeat(40),
                        &f.temp.path().join("dispatcher"),
                        &f.temp.path().join("spawns"),
                    )
                    .unwrap()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|v| **v).count(), 1);
    assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 1);
    assert!(!f
        .step(
            &f.a,
            "PR-7",
            &"b".repeat(40),
            Uuid::new_v4(),
            State::Reserved
        )
        .unwrap());
    let repo = private_repo().unwrap();
    let (_, table) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
    assert_eq!(table.changes["PR-7"].len(), 1);
    // The losing watcher does not probe the hub again on its next poll.
    let loser = if outcomes[0] { &f.b } else { &f.a };
    std::fs::rename(
        f.temp.path().join("hub.git"),
        f.temp.path().join("offline.git"),
    )
    .unwrap();
    assert!(!crate::review_watch::fixture_admit_dispatch(
        loser,
        &"a".repeat(40),
        &program,
        &marker
    )
    .unwrap());
    // A new head invalidates the remote hold, attempts admission, and records
    // its unreachable-hub error locally as Failed instead of leaking a slot.
    assert!(
        crate::review_watch::fixture_admit_dispatch(loser, &"c".repeat(40), &program, &marker)
            .is_err()
    );
    crate::review_watch::assert_failed_admission(loser, &"c".repeat(40));
    // Restore connectivity and retire the winner's fixture reservation. The
    // failed new-head attempt must actually dispatch, not just be selectable.
    std::fs::rename(
        f.temp.path().join("offline.git"),
        f.temp.path().join("hub.git"),
    )
    .unwrap();
    let winner = if outcomes[0] { &f.a } else { &f.b };
    let reservation: Transition = serde_json::from_str(&table.changes["PR-7"][0].payload).unwrap();
    f.step(
        winner,
        "PR-7",
        &reservation.head,
        reservation.token,
        State::Cancelled,
    )
    .unwrap();
    assert!(
        crate::review_watch::fixture_admit_dispatch(loser, &"c".repeat(40), &program, &marker,)
            .unwrap()
    );
    assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 2);
}
#[test]
fn story2492_admission_transitions_fence_tokens_and_terminal_heads() {
    let f = Fixture::new();
    let _env = f.environment();
    let token = Uuid::new_v4();
    let head = "c".repeat(40);
    assert!(f.step(&f.a, "PR-8", &head, token, State::Reserved).unwrap());
    assert!(f.step(&f.b, "PR-8", &head, token, State::Running).is_err());
    assert!(f.step(&f.a, "PR-8", &head, token, State::Terminal).is_err());
    assert!(f.step(&f.a, "PR-8", &head, token, State::Running).unwrap());
    assert!(f.step(&f.a, "PR-8", &head, token, State::Running).is_err());
    assert!(f
        .step(&f.a, "PR-8", &head, token, State::Cancelled)
        .is_err());
    assert!(f.step(&f.a, "PR-8", &head, token, State::Terminal).unwrap());
    assert!(!f
        .step(&f.b, "PR-8", &head, Uuid::new_v4(), State::Reserved)
        .unwrap());
    assert!(f
        .step(
            &f.b,
            "PR-8",
            &"d".repeat(40),
            Uuid::new_v4(),
            State::Reserved
        )
        .unwrap());
}
#[test]
fn story2492_admission_cancellation_preserves_other_changes_and_allows_retry() {
    let f = Fixture::new();
    let _env = f.environment();
    let token = Uuid::new_v4();
    let head = "e".repeat(40);
    assert!(f.step(&f.a, "PR-9", &head, token, State::Reserved).unwrap());
    assert!(f
        .step(&f.b, "PR-10", &head, Uuid::new_v4(), State::Reserved)
        .unwrap());
    assert!(f
        .step(&f.a, "PR-9", &head, token, State::Cancelled)
        .unwrap());
    assert!(f
        .step(&f.b, "PR-9", &head, Uuid::new_v4(), State::Reserved)
        .unwrap());
    let repo = private_repo().unwrap();
    let (_, table) = read_table(repo.path(), &f.hub, "PR-9").unwrap();
    assert_eq!(table.changes["PR-9"].len(), 3);
    let (_, table) = read_table(repo.path(), &f.hub, "PR-10").unwrap();
    assert_eq!(table.changes["PR-10"].len(), 1);
}
#[test]
fn story2492_admission_unreachable_named_and_mirror_refused() {
    let f = Fixture::new();
    let _env = f.environment();
    assert!(Hub::new("https://gitlab.com/fixture/review.git", REF)
        .unwrap_err()
        .to_string()
        .contains("admission_mirror_refused"));
    assert!(f
        .step(
            &f.a,
            "MR-7",
            &"a".repeat(40),
            Uuid::new_v4(),
            State::Reserved
        )
        .unwrap_err()
        .to_string()
        .contains("admission_mirror_refused"));
    assert!(f
        .hub
        .validate_repository("gitlab:gitlab.com/fixture/review")
        .is_err());
    assert!(f
        .hub
        .validate_repository("github:github.com/another/repository")
        .is_err());
    std::fs::rename(
        f.temp.path().join("hub.git"),
        f.temp.path().join("unreachable.git"),
    )
    .unwrap();
    let error = f
        .step(
            &f.a,
            "PR-7",
            &"a".repeat(40),
            Uuid::new_v4(),
            State::Reserved,
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("admission_hub_unreachable"));
    assert!(format!("{error:#}").contains(&f.hub.url));
}

#[test]
fn story2492_admission_rejects_tampering_revocation_and_unknown_versions() {
    let f = Fixture::new();
    let _env = f.environment();
    assert!(f
        .step(
            &f.a,
            "PR-11",
            &"a".repeat(40),
            Uuid::new_v4(),
            State::Reserved
        )
        .unwrap());
    let repo = private_repo().unwrap();
    let (_, mut table) = read_table(repo.path(), &f.hub, "PR-11").unwrap();
    let store = f.a.join(".aida-store");
    let mut roster = roster(&store, &super::super::head(&store).unwrap()).unwrap();
    let records = table.changes.get_mut("PR-11").unwrap();
    let original = records[0].payload.clone();
    records[0].payload = original.replace("reserved", "running");
    assert!(chain(&f.hub, "PR-11", records, &roster).is_err());
    records[0].payload = original.replace("\"version\":1", "\"version\":2");
    assert!(chain(&f.hub, "PR-11", records, &roster).is_err());
    records[0].payload = original;
    assert!(chain(&f.hub, "PR-11", records, &roster).is_ok());
    roster.nodes.remove("a");
    assert!(chain(&f.hub, "PR-11", records, &roster)
        .unwrap_err()
        .to_string()
        .contains("revoked"));
}

#[test]
fn story2492_admission_pin_and_explicit_trusted_settings_are_required() {
    let f = Fixture::new();
    let _env = f.environment();
    let pinned = Hub::load(&f.a).unwrap();
    assert_eq!(pinned, Some(f.hub.clone()));
    require_pinned(&f.a, &pinned).unwrap();
    // The unmerged working copy cannot select another admission authority.
    write(
        &f.a,
        ".aida/config.toml",
        "[review]\nadmission_hub = 'https://gitlab.com/mirror/repo.git'\n",
    );
    assert_eq!(Hub::load(&f.a).unwrap(), pinned);
    // An intentional default-branch removal cannot downgrade an already
    // reserved dispatcher into the local-only path.
    write(
        &f.a,
        ".aida/config.toml",
        "[deployment]\nstore_path = '.aida-store'\n",
    );
    ok(&f.a, &["add", ".aida/config.toml"]);
    ok(&f.a, &["commit", "-qm", "operator config migration"]);
    assert!(
        Hub::load(&f.a).is_err(),
        "configured publisher cannot omit the hub"
    );
    std::fs::remove_file(f.a.join(CONFIG)).unwrap();
    assert!(Hub::load(&f.a).unwrap().is_none());
    assert!(require_pinned(&f.a, &pinned).is_err());
}

#[test]
fn story2492_admission_sync_refreshes_publications_without_shared_fetch_head() {
    let f = Fixture::new();
    let _env = f.environment();
    let a = f.a.join(".aida-store");
    let b = f.b.join(".aida-store");
    write(&a, "reviews/sync-marker", "new remote evidence");
    ok(&a, &["add", "reviews/sync-marker"]);
    ok(&a, &["commit", "-qm", "publish new evidence"]);
    write(&b, ".git/FETCH_HEAD", "unrelated fetch sentinel\n");
    sync(&f.b).unwrap();
    assert_eq!(
        std::fs::read_to_string(b.join("reviews/sync-marker")).unwrap(),
        "new remote evidence"
    );
    assert_eq!(
        std::fs::read_to_string(b.join(".git/FETCH_HEAD")).unwrap(),
        "unrelated fetch sentinel\n"
    );
    assert!(ok(
        &b,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/aida/review-sync/"
        ]
    )
    .is_empty());
}

// trace:TASK-3196 | ai:codex
#[test]
fn story2492_admission_recovery_fences_old_tokens_and_recovers_revoked_owner() {
    let f = Fixture::new();
    let _env = f.environment();
    let old = Uuid::new_v4();
    let head = "a".repeat(40);
    f.step(&f.a, "PR-7", &head, old, State::Reserved).unwrap();
    f.step(&f.a, "PR-7", &head, old, State::Running).unwrap();
    assert!(require_released(&f.b, "PR-7").is_err());
    let repo = private_repo().unwrap();
    let (tip, _) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
    let tip = tip.unwrap();
    assert!(reconcile(&f.b, "PR-7", &tip, "crashed owner stopped", false).is_err());
    assert!(reconcile(&f.b, "PR-7", &"f".repeat(40), "crashed owner stopped", true).is_err());
    // Revocation blocks the old chain, but an enrolled operator can re-anchor.
    let store = f.b.join(".aida-store");
    let mut r = roster(&store, &super::super::head(&store).unwrap()).unwrap();
    r.nodes.remove("a");
    write(&store, ROSTER, serde_json::to_vec(&r).unwrap());
    ok(&store, &["add", ROSTER]);
    ok(&store, &["commit", "-qm", "revoke crashed owner"]);
    assert!(f
        .step(&f.b, "PR-7", &head, Uuid::new_v4(), State::Reserved)
        .is_err());
    reconcile(
        &f.b,
        "PR-7",
        &tip,
        "all watchers stopped; old owner and descendants stopped",
        true,
    )
    .unwrap();
    require_released(&f.b, "PR-7").unwrap();
    let (_, mut anchored) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
    validate_anchor(&f.hub, "PR-7", &anchored, &r).unwrap();
    let anchor = anchored
        .anchor
        .as_mut()
        .expect("reset must carry signed authority");
    anchor.payload = anchor
        .payload
        .replace("all watchers stopped", "unverified replacement");
    assert!(validate_anchor(&f.hub, "PR-7", &anchored, &r).is_err());
    assert!(f.step(&f.a, "PR-7", &head, old, State::Terminal).is_err());
    assert!(f
        .step(
            &f.b,
            "PR-7",
            &"b".repeat(40),
            Uuid::new_v4(),
            State::Reserved
        )
        .unwrap());
    let (current, _) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
    let names = ok(
        repo.path(),
        &["ls-tree", "-r", "--name-only", &current.unwrap()],
    );
    let audit = names
        .lines()
        .find(|p| p.starts_with("recoveries/"))
        .unwrap();
    let current = remote_tip(repo.path(), &f.hub).unwrap().unwrap();
    let signed: Signed =
        serde_json::from_slice(&blob(repo.path(), &current, audit).unwrap().unwrap()).unwrap();
    verify_signature("b", &signed, &r).unwrap();
    assert!(signed.payload.contains(&tip));
    // The historical chain is still available for forensic inspection.
    assert!(blob(repo.path(), &tip, &change_path("PR-7").unwrap())
        .unwrap()
        .is_some());
}

// trace:TASK-3196 | ai:codex
#[test]
fn story2492_admission_exit_liveness_and_hub_failure() {
    for (ambiguous, unclaimed, unreachable) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let f = Fixture::new();
        let _env = f.environment();
        let result =
            crate::review_watch::fixture_admission_exit(&f.a, ambiguous, unclaimed, || {
                if unreachable {
                    std::fs::rename(
                        f.temp.path().join("hub.git"),
                        f.temp.path().join("offline.git"),
                    )
                    .unwrap();
                }
            });
        if unreachable {
            assert!(format!("{:#}", result.unwrap_err()).contains("admission_hub_unreachable"));
            std::fs::rename(
                f.temp.path().join("offline.git"),
                f.temp.path().join("hub.git"),
            )
            .unwrap();
        } else {
            result.unwrap();
        }
        let repo = private_repo().unwrap();
        let (_, table) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
        let store = f.a.join(".aida-store");
        let history = chain(
            &f.hub,
            "PR-7",
            &table.changes["PR-7"],
            &roster(&store, &super::super::head(&store).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            history.last().unwrap().state,
            if unclaimed {
                State::Cancelled
            } else if ambiguous || unreachable {
                State::Running
            } else {
                State::Terminal
            }
        );
    }
}

// A repository with >2000 independently signed shards and >1 MiB of total
// admission content still admits independently, beyond the old global limits.
// trace:TASK-3196 | ai:codex
#[test]
fn story2492_admission_shards_exceed_the_old_global_budgets() {
    let f = Fixture::new();
    let _env = f.environment();
    f.step(
        &f.a,
        "PR-7",
        &"a".repeat(40),
        Uuid::new_v4(),
        State::Reserved,
    )
    .unwrap();
    let repo = private_repo().unwrap();
    let (tip, table) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
    let mut transition: Transition =
        serde_json::from_str(&table.changes["PR-7"][0].payload).unwrap();
    let config = publisher(&f.a).unwrap().unwrap();
    git(repo.path(), &["read-tree", tip.as_ref().unwrap()]).unwrap();
    let path = repo.path().join("shard.json");
    let mut total = 0;
    let mut args = vec!["update-index".to_string()];
    for n in 100..2200 {
        transition.change = format!("PR-{n}");
        transition.id = Uuid::new_v4();
        transition.token = Uuid::new_v4();
        let shard = Table {
            version: 1,
            changes: BTreeMap::from([(
                transition.change.clone(),
                vec![sign(&config, &transition).unwrap()],
            )]),
            ..Table::default()
        };
        let bytes = serde_json::to_vec(&shard).unwrap();
        total += bytes.len();
        std::fs::write(&path, bytes).unwrap();
        let blob = text(git(repo.path(), &["hash-object", "-w", path.to_str().unwrap()]).unwrap())
            .unwrap();
        args.extend([
            "--add".into(),
            "--cacheinfo".into(),
            "100644".into(),
            blob,
            change_path(&transition.change).unwrap(),
        ]);
    }
    assert!(total > CAP);
    git(
        repo.path(),
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .unwrap();
    let tree = text(git(repo.path(), &["write-tree"]).unwrap()).unwrap();
    let next = ok(
        repo.path(),
        &[
            "commit-tree",
            &tree,
            "-p",
            tip.as_ref().unwrap(),
            "-m",
            "many independently signed shards",
        ],
    );
    remote_git(
        repo.path(),
        &f.hub.url,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "push",
            &f.hub.url,
            &format!("{next}:{}", f.hub.reference),
        ],
    )
    .unwrap();
    assert!(f
        .step(
            &f.b,
            "PR-9",
            &"b".repeat(40),
            Uuid::new_v4(),
            State::Reserved
        )
        .unwrap());
    let (_, selected) = read_table(repo.path(), &f.hub, "PR-9").unwrap();
    assert_eq!(selected.changes.len(), 1);
    assert_eq!(selected.changes["PR-9"].len(), 1);
    let (_, unchanged) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
    assert_eq!(unchanged.changes["PR-7"].len(), 1);
}

// trace:TASK-3196 | ai:codex
#[test]
fn story2492_continuous_watcher_survives_a_sync_refusal() {
    let f = Fixture::new();
    let _env = f.environment();
    let b = f.b.join(".aida-store");
    write(&b, "dirty-marker", "uncommitted store evidence");
    assert!(crate::review_watch::fixture_sync_pass(&f.b, true).is_err());
    assert!(!crate::review_watch::fixture_sync_pass(&f.b, false).unwrap());
    std::fs::remove_file(b.join("dirty-marker")).unwrap();
    assert!(crate::review_watch::fixture_sync_pass(&f.b, false).unwrap());
}

// trace:TASK-3196 | ai:codex
#[test]
fn story2492_documented_recovery_command_parses_and_config_stays_opt_in() {
    crate::command_registry::try_parse_from([
        "aida",
        "review",
        "nodes",
        "admission-reconcile",
        "PR-123",
        "--expected-tip",
        &"a".repeat(40),
        "--all-reviewers-stopped",
        "--reason",
        "owner and descendants stopped",
    ])
    .unwrap();
    let config: toml::Value = toml::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../.aida/config.toml"
    )))
    .unwrap();
    assert!(config
        .get("review")
        .and_then(|r| r.get("admission_hub"))
        .is_none());
    assert!(config
        .get("review")
        .and_then(|r| r.get("admission_ref"))
        .is_none());
}

// trace:TASK-3196 | ai:codex
#[test]
fn story2492_admission_recovery_requires_a_direct_human() {
    let _agent =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_AGENT_NAME", Some("fixture-reviewer"))]);
    let command = NodeCommand::AdmissionReconcile {
        change: "PR-7".into(),
        expected_tip: "a".repeat(40),
        reason: "cannot self-authorize recovery".into(),
        all_reviewers_stopped: true,
    };
    let error = nodes(&command).unwrap_err();
    assert!(error
        .to_string()
        .contains("AIDA-managed agent sessions cannot issue direct TTY grants"));
}

// trace:TASK-3196 | ai:codex
#[test]
fn story2492_failed_hub_cancellation_still_frees_local_capacity() {
    let f = Fixture::new();
    let _env = f.environment();
    crate::review_watch::fixture_admission_spawn_failure(&f.a, || {
        std::fs::rename(
            f.temp.path().join("hub.git"),
            f.temp.path().join("offline.git"),
        )
        .unwrap();
    })
    .unwrap();
    std::fs::rename(
        f.temp.path().join("offline.git"),
        f.temp.path().join("hub.git"),
    )
    .unwrap();
    // Safety is independent of local capacity: the accepted claim remains.
    assert!(require_released(&f.a, "PR-7").is_err());
}

// trace:TASK-3196 | ai:codex
#[test]
fn task3196_terminal_transition_survives_store_commit_during_hub_read() {
    let f = Fixture::new();
    let _env = f.environment();
    let token = Uuid::new_v4();
    let head = "a".repeat(40);
    f.step(&f.a, "PR-7", &head, token, State::Reserved).unwrap();
    f.step(&f.a, "PR-7", &head, token, State::Running).unwrap();
    let store = f.a.join(".aida-store");
    let before = super::super::head(&store).unwrap();
    assert!(update_with_observer(
        &f.a,
        &f.hub,
        "PR-7",
        &head,
        token,
        State::Terminal,
        Some("observed worker exit".into()),
        || {
            write(
                &store,
                "unrelated-comment",
                "another agent wrote during hub fetch",
            );
            ok(&store, &["add", "unrelated-comment"]);
            ok(&store, &["commit", "-qm", "unrelated store write"]);
        },
    )
    .unwrap());
    assert_ne!(super::super::head(&store).unwrap(), before);
    let repo = private_repo().unwrap();
    let (_, table) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
    let history = chain(
        &f.hub,
        "PR-7",
        &table.changes["PR-7"],
        &roster(&store, &super::super::head(&store).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(history.last().unwrap().state, State::Terminal);
}

// All other lifecycle writes share Terminal's evidence snapshots. Unrelated
// store commits must not fail polling, preflight or pre-spawn cancellation.
// trace:TASK-3196 | ai:codex
#[test]
fn task3196_admission_lifecycle_survives_store_commits_during_hub_read() {
    for state in [State::Reserved, State::Running, State::Cancelled] {
        let f = Fixture::new();
        let _env = f.environment();
        let token = Uuid::new_v4();
        let head = "a".repeat(40);
        if state != State::Reserved {
            f.step(&f.a, "PR-7", &head, token, State::Reserved).unwrap();
        }
        let store = f.a.join(".aida-store");
        let before = super::super::head(&store).unwrap();
        let mut reads = 0;
        assert!(update_with_observer(
            &f.a,
            &f.hub,
            "PR-7",
            &head,
            token,
            state.clone(),
            (state != State::Reserved).then(|| "observed lifecycle receipt".into()),
            || {
                reads += 1;
                write(&store, "unrelated-comment", "concurrent agent comment");
                ok(&store, &["add", "unrelated-comment"]);
                ok(&store, &["commit", "-qm", "unrelated store write"]);
            },
        )
        .unwrap());
        assert_eq!(reads, 1, "unrelated writes need no admission retry");
        assert_ne!(super::super::head(&store).unwrap(), before);
        let repo = private_repo().unwrap();
        let (_, table) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
        let history = chain(
            &f.hub,
            "PR-7",
            &table.changes["PR-7"],
            &roster(&store, &super::super::head(&store).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(history.last().unwrap().state, state);
    }
}

// trace:TASK-3196 | ai:codex
#[test]
fn task3196_terminal_retries_changed_roster_evidence() {
    let f = Fixture::new();
    let _env = f.environment();
    let token = Uuid::new_v4();
    let head = "a".repeat(40);
    f.step(&f.a, "PR-7", &head, token, State::Reserved).unwrap();
    f.step(&f.a, "PR-7", &head, token, State::Running).unwrap();
    let store = f.a.join(".aida-store");
    let mut reads = 0;
    assert!(update_with_observer(
        &f.a,
        &f.hub,
        "PR-7",
        &head,
        token,
        State::Terminal,
        Some("observed worker exit".into()),
        || {
            reads += 1;
            if reads == 1 {
                let mut r = roster(&store, &super::super::head(&store).unwrap()).unwrap();
                r.nodes.insert("new-peer".into(), r.nodes["b"].clone());
                write(&store, ROSTER, serde_json::to_vec(&r).unwrap());
                ok(&store, &["add", ROSTER]);
                ok(&store, &["commit", "-qm", "enroll another peer"]);
            }
        },
    )
    .unwrap());
    assert_eq!(
        reads, 2,
        "changed evidence must rebuild within the retry loop"
    );
    let repo = private_repo().unwrap();
    let (_, table) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
    let history = chain(
        &f.hub,
        "PR-7",
        &table.changes["PR-7"],
        &roster(&store, &super::super::head(&store).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        history.len(),
        3,
        "only the refreshed Terminal was published"
    );
    assert_eq!(history.last().unwrap().state, State::Terminal);
}

// trace:BUG-2518 | ai:codex
#[test]
fn bug2518_admission_no_verdict_backoff_retry_and_exhaustion() {
    let f = Fixture::new();
    let _env = f.environment();
    crate::review_watch::fixture_admission_exit(&f.a, false, false, || {}).unwrap();
    crate::review_watch::fixture_admission_retry(&f.a, false).unwrap();
    crate::review_watch::fixture_admission_retry(&f.a, false).unwrap();
    crate::review_watch::fixture_admission_retry(&f.a, true).unwrap();
    // The authority cap also holds if a caller bypasses local backoff/state.
    assert!(!f
        .step(
            &f.b,
            "PR-7",
            &"a".repeat(40),
            Uuid::new_v4(),
            State::Reserved
        )
        .unwrap());
    assert!(f
        .step(
            &f.b,
            "PR-7",
            &"b".repeat(40),
            Uuid::new_v4(),
            State::Reserved
        )
        .unwrap());
}

// trace:BUG-2518 | ai:codex
#[test]
fn bug2518_chain_accepts_only_no_verdict_terminal_retry_edge() {
    let f = Fixture::new();
    let _env = f.environment();
    let token = Uuid::new_v4();
    let head = "a".repeat(40);
    f.step(&f.a, "PR-7", &head, token, State::Reserved).unwrap();
    f.step(&f.a, "PR-7", &head, token, State::Running).unwrap();
    let repo = private_repo().unwrap();
    let (_, table) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
    let config = publisher(&f.a).unwrap().unwrap();
    let store = f.a.join(".aida-store");
    let roster = roster(&store, &super::super::head(&store).unwrap()).unwrap();
    let base = table.changes["PR-7"].clone();
    let running: Transition = serde_json::from_str(&base.last().unwrap().payload).unwrap();
    for (state, evidence, admitted) in [
        (State::Running, "NoVerdict: stopped", false),
        (State::Terminal, "ordinary exit", false),
        (State::Terminal, "NoVerdict: stopped", true),
        // trace:BUG-2521 | ai:codex
        (State::Running, "Inconclusive: stopped", false),
        (State::Terminal, "Inconclusive: stopped", true),
    ] {
        let mut records = base[..1].to_vec();
        let mut end = running.clone();
        records.push(sign(&config, &end).unwrap());
        if state == State::Terminal {
            end.predecessor = Some(end.id);
            end.id = Uuid::new_v4();
            end.state = state;
            end.evidence = Some(evidence.into());
            records.push(sign(&config, &end).unwrap());
        }
        let mut retry = end.clone();
        retry.predecessor = Some(end.id);
        retry.id = Uuid::new_v4();
        retry.token = Uuid::new_v4();
        retry.state = State::Retry;
        retry.evidence = None;
        records.push(sign(&config, &retry).unwrap());
        assert_eq!(chain(&f.hub, "PR-7", &records, &roster).is_ok(), admitted);
        if admitted {
            let mut cancelled_records = records.clone();
            let mut cancelled = retry.clone();
            cancelled.predecessor = Some(retry.id);
            cancelled.id = Uuid::new_v4();
            cancelled.state = State::Cancelled;
            cancelled.evidence = Some("pre-spawn cancellation".into());
            cancelled_records.push(sign(&config, &cancelled).unwrap());
            let mut next = retry.clone();
            next.predecessor = Some(cancelled.id);
            next.id = Uuid::new_v4();
            next.token = Uuid::new_v4();
            cancelled_records.push(sign(&config, &next).unwrap());
            assert!(chain(&f.hub, "PR-7", &cancelled_records, &roster).is_ok());
            for invalid in 0..3 {
                let mut bad = next.clone();
                match invalid {
                    0 => bad.token = retry.token,
                    1 => bad.head = "b".repeat(40),
                    _ => bad.state = State::Reserved,
                }
                *cancelled_records.last_mut().unwrap() = sign(&config, &bad).unwrap();
                assert!(chain(&f.hub, "PR-7", &cancelled_records, &roster).is_err());
            }
            retry.state = State::Reserved;
            *records.last_mut().unwrap() = sign(&config, &retry).unwrap();
            assert!(chain(&f.hub, "PR-7", &records, &roster).is_err());
        }
    }
}

// Each cancellation path gets its own nextest process and timing budget. The
// combined regression measured 31.82s in CI, exceeding the default 30s limit.
// Keep the full repeated-cancellation and exhaustion assertions for every path.
// trace:BUG-2518 | ai:codex
fn assert_cancelled_retry_readmits(cancellation: u8) {
    let f = Fixture::new();
    let _env = f.environment();
    crate::review_watch::fixture_admission_exit(&f.a, false, false, || {}).unwrap();
    crate::review_watch::fixture_admission_retry_cancel(&f.a, false, Some(cancellation)).unwrap();
    crate::review_watch::fixture_admission_retry_cancel(&f.a, false, Some(cancellation)).unwrap();
    crate::review_watch::fixture_admission_retry(&f.a, true).unwrap();
    let head = "a".repeat(40);
    assert!(!f
        .step(&f.b, "PR-7", &head, Uuid::new_v4(), State::Reserved)
        .unwrap());
    let repo = private_repo().unwrap();
    let (_, table) = read_table(repo.path(), &f.hub, "PR-7").unwrap();
    let store = f.a.join(".aida-store");
    let history = chain(
        &f.hub,
        "PR-7",
        &table.changes["PR-7"],
        &roster(&store, &super::super::head(&store).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        history.iter().filter(|t| t.state == State::Running).count(),
        3
    );
    assert_eq!(
        history
            .iter()
            .filter(|t| t.state == State::Cancelled)
            .count(),
        4
    );
    assert_eq!(history.last().unwrap().state, State::Terminal);
}

// trace:BUG-2518 | ai:codex
#[test]
fn bug2518_cancelled_retry_readmits_after_spawn_failure() {
    assert_cancelled_retry_readmits(0);
}

// trace:BUG-2518 | ai:codex
#[test]
fn bug2518_cancelled_retry_readmits_after_preflight_failure() {
    assert_cancelled_retry_readmits(1);
}

// trace:BUG-2518 | ai:codex
#[test]
fn bug2518_cancelled_retry_readmits_after_unclaimed_exit() {
    assert_cancelled_retry_readmits(2);
}
