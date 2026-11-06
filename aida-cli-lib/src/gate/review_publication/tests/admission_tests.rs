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
    let (_, table) = read_table(repo.path(), &f.hub).unwrap();
    assert_eq!(table.changes["PR-7"].len(), 1);
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
    let (_, table) = read_table(repo.path(), &f.hub).unwrap();
    assert_eq!(table.changes["PR-10"].len(), 1);
    assert_eq!(table.changes["PR-9"].len(), 3);
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
    let (_, mut table) = read_table(repo.path(), &f.hub).unwrap();
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
