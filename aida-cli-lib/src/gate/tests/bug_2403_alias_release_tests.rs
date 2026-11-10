//! BUG-2403 Required behavior 2 (review finding 4a/5): a Rework hold armed
//! by a refusal under a spec's FRIENDLY label is answered by a fresh granted
//! approval recorded under its NATIVE label — the two labels proved equal by
//! one committed store snapshot — through the production arming readers
//! ([`read_arming_body`], [`read_arming_history`], [`with_proved_labels`]).
//! The hold still refuses a wrong head, an invalid grant, an unrelated
//! spec's approval, a new refusal or a label remap between the release's
//! passes, and any label bridge the snapshot cannot prove. Verdicts go
//! through the `aida review record` write boundary under a real reviewer
//! grant, as in the BUG-2123 tests.
// trace:BUG-2403 | ai:claude
// mandate-gate
use super::*;
use crate::test_env::AmbientGuard;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;

const OLD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MOVED: &str = "cccccccccccccccccccccccccccccccccccccccc";
const PR: u64 = 2403;
const FRIENDLY: &str = "STORY-2220";
const NATIVE: &str = "STORY-B6B1278890774EF993D2EBC327862470-262";
const OTHER_SPEC: &str = "STORY-2221";
const REVIEWER: &str = "claude reviewer seat";

fn record(root: &Path, key: &str, verdict: &str, sha: &str) {
    // Strictly ordered recording times.
    std::thread::sleep(std::time::Duration::from_millis(15));
    let path = rv::verdict_path(root, key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let findings = if verdict == "approved" {
        Vec::new()
    } else {
        vec!["the fix is untested".to_string()]
    };
    let mut obj = rv::build_verdict_object(
        root,
        &path,
        Some(verdict),
        Some(sha),
        Some("story-2220-work"),
        Some("round summary"),
        &findings,
        REVIEWER,
    )
    .unwrap();
    stamp_record_grant(root, &mut obj);
    rv::write_verdict_object(&path, &obj).unwrap();
}

fn rework(key: &str) -> mh::MergeHoldRecord {
    let mut hold = mh::typed_hold(
        PR,
        mh::HoldReasonKind::Rework,
        &format!("CHANGES REQUESTED for {key}"),
        Some(OLD.into()),
    );
    hold.verdict_ref = Some(mh::VerdictRef::new(
        key,
        Some(PR),
        Some(OLD.into()),
        Some(REVIEWER.into()),
    ));
    hold.spec = Some(key.into());
    hold
}

fn requirement(native: &str, friendly: &str) -> aida_core::Requirement {
    let mut req = aida_core::Requirement::new("Improve a list".into(), "fixture".into());
    req.req_type = aida_core::RequirementType::Story;
    req.execution_mode = Some(aida_core::ExecutionMode::Drive);
    req.spec_id = Some(native.into());
    req.agreed_id = Some(friendly.into());
    req
}

fn store_git(store: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(store)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Turn the hermetic seat's `.aida-store` into a committed git-canonical
/// store holding this spec (friendly + native) and an unrelated one.
fn commit_snapshot(root: &Path) -> PathBuf {
    let store = crate::detect_distributed_store_from(root).expect("the seat fixture's store");
    aida_core::git_ops::init(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "fixture", "fixture@localhost").unwrap();
    let backend = crate::test_env::home_fixture_cached_backend(
        &store,
        &aida_core::CachedGitBackend::default_cache_path(&store),
    )
    .unwrap();
    for req in [
        requirement(NATIVE, FRIENDLY),
        requirement("STORY-B6B1278890774EF993D2EBC327862470-263", OTHER_SPEC),
    ] {
        aida_core::db::DatabaseBackend::add_requirement(&backend, req).unwrap();
    }
    store_git(&store, &["add", "-A"]);
    store_git(&store, &["commit", "-q", "--allow-empty", "-m", "fixture"]);
    if let Err(e) = crate::gate::review_alias::snapshot_labels(root, FRIENDLY) {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&store)
            .args(["status", "--porcelain=v1", "--untracked-files=all"])
            .output()
            .unwrap();
        panic!(
            "the committed snapshot must prove identity: {e}\nstatus:\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    store
}

fn covered_store() -> aida_core::RequirementsStore {
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(requirement(NATIVE, FRIENDLY));
    store
}

fn has_clearance(root: &Path) -> bool {
    mh::recorded_clearance(root, PR).is_some()
}

/// A retained round or record that refuses (either spelling).
fn refuses(r: &Value) -> bool {
    r["verdict"]
        .as_str()
        .is_some_and(|v| rv::VerdictKind::parse(v).blocks_done())
}

/// A friendly-label refusal at OLD arms the hold.
fn arm(root: &Path) {
    record(root, FRIENDLY, "request-changes", OLD);
    mh::place_hold(root, &rework(FRIENDLY)).unwrap();
}

struct Io<'a> {
    heads: &'a [&'a str],
    grant: Option<sa::GrantCheck>,
    on_read: &'a dyn Fn(usize),
}

/// One auto-release attempt through the production arming readers.
fn release(root: &Path, io: Io<'_>) -> (Result<Outcome>, Vec<String>) {
    let hold = mh::read_hold_record(root, PR).expect("an armed hold");
    let comments = RefCell::new(Vec::new());
    let reads = Cell::new(0usize);
    let covered = covered_store();
    let result = auto_release_with(
        root,
        &hold,
        AutoReleaseIo {
            coverage: &|_| Ok(None),
            live_head: &|| {
                (io.on_read)(reads.get());
                let i = reads.get().min(io.heads.len() - 1);
                reads.set(reads.get() + 1);
                Ok(Some(mh::PinnedChange {
                    merged: false,
                    state: mh::ChangeState::Open,
                    head_sha: Some(io.heads[i].into()),
                    hold_label: true,
                    recorded_label: true,
                    cleared_label: false,
                }))
            },
            read_hold: &|| mh::read_hold_record(root, PR),
            covered_specs: &|| {
                Ok((
                    with_proved_labels(root, vec![FRIENDLY.into()]),
                    covered.clone(),
                ))
            },
            read_body: &|key| read_arming_body(root, root, key),
            read_history: &|key| read_arming_history(root, key),
            read_corpus: &|ids| {
                mh::corpus_gate_bodies(&[root], PR, ids).map_err(anyhow::Error::msg)
            },
            grant_check: &|id, subject| match &io.grant {
                Some(forced) => forced.clone(),
                None => sa::grant_check_for(root, id, subject),
            },
            approval_authority: &|_| Ok(()),
            mandate: &|| bail!("no mandate is active"),
            refusal_release: &|_, _| held_check(),
            comment: &|body| {
                comments.borrow_mut().push(body.to_string());
                Ok(())
            },
            labels: &|| Ok(()),
        },
    );
    (result, comments.into_inner())
}

fn lawful<'a>(heads: &'a [&'a str]) -> Io<'a> {
    Io {
        heads,
        grant: None,
        on_read: &|_| {},
    }
}

/// The lawful case: the native approval answers the friendly refusal. The
/// literal friendly read alone still sees the refusal (the defect), the
/// proved selection sees the approval, and the release records the hold's
/// own key.
#[test]
fn bug_2403_native_approval_releases_a_friendly_armed_hold() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _env = AmbientGuard::hermetic_with_seat(&root, "reviewer", &[]);
    commit_snapshot(&root);
    arm(&root);
    record(&root, NATIVE, "approved", HEAD);

    let literal = rv::current_verdict(&root, FRIENDLY).unwrap();
    assert!(
        refuses(&serde_json::from_str(&literal.body).unwrap()),
        "{}",
        literal.body
    );
    let mut labels = arming_labels(&root, FRIENDLY);
    labels.sort();
    assert_eq!(labels, vec![FRIENDLY.to_string(), NATIVE.to_string()]);
    let body = read_arming_body(&root, &root, FRIENDLY).unwrap();
    assert_eq!(body["verdict"], "approved");
    assert!(body.get("release_grant").is_some(), "{body}");
    let history = read_arming_history(&root, FRIENDLY).unwrap();
    assert!(
        history.archived.iter().any(refuses),
        "the friendly arming refusal stays in the retained history"
    );

    let (result, comments) = release(&root, lawful(&[HEAD]));
    assert_eq!(result.unwrap(), Outcome::Released { mandate: None });
    assert!(!mh::hold_is_open(&root, PR));
    let audit = Value::Object(mh::recorded_clearance(&root, PR).unwrap());
    assert_eq!(audit["released_by_verdict"]["key"], FRIENDLY);
    assert!(!comments.is_empty());
}

/// The hold matrix: a wrong head, an invalid grant and an unrelated spec's
/// approval each refuse and leave the hold armed.
#[test]
fn bug_2403_alias_release_still_refuses_wrong_head_invalid_grant_unrelated_spec() {
    // Wrong head.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _env = AmbientGuard::hermetic_with_seat(&root, "reviewer", &[]);
    commit_snapshot(&root);
    arm(&root);
    record(&root, NATIVE, "approved", HEAD);
    let (result, _) = release(&root, lawful(&[MOVED]));
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("stale or unverifiable head"), "{error}");
    assert!(mh::hold_is_open(&root, PR));

    // Invalid (absent) grant.
    let (result, _) = release(
        &root,
        Io {
            heads: &[HEAD],
            grant: Some(sa::GrantCheck::Absent),
            on_read: &|_| {},
        },
    );
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("no validated reviewer grant"), "{error}");
    assert!(mh::hold_is_open(&root, PR));
    drop(_env);

    // An approval of an unrelated spec (another requirement in the same
    // snapshot) answers nothing.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _env = AmbientGuard::hermetic_with_seat(&root, "reviewer", &[]);
    commit_snapshot(&root);
    arm(&root);
    record(&root, OTHER_SPEC, "approved", HEAD);
    let (result, _) = release(&root, lawful(&[HEAD]));
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("verdict is not APPROVED"), "{error}");
    assert!(mh::hold_is_open(&root, PR));
    assert!(!arming_labels(&root, FRIENDLY)
        .iter()
        .any(|l| l == OTHER_SPEC));
}

/// Races between the first pass and the second (under the hold lock): a new
/// refusal under the friendly label, or an uncommitted label remap, changes
/// the selected approval and refuses; every arming refusal stays retained.
#[test]
fn bug_2403_refusal_or_remap_between_passes_refuses_the_release() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _env = AmbientGuard::hermetic_with_seat(&root, "reviewer", &[]);
    commit_snapshot(&root);
    arm(&root);
    record(&root, NATIVE, "approved", HEAD);
    let (result, _) = release(
        &root,
        Io {
            heads: &[HEAD],
            grant: None,
            on_read: &|i| {
                if i == 2 {
                    record(&root, FRIENDLY, "request-changes", HEAD);
                }
            },
        },
    );
    // The new friendly refusal is now the selected current record: the
    // release refuses (whichever pass sees it first) and never falls back
    // to the earlier native approval.
    let error = format!("{:#}", result.unwrap_err());
    assert!(
        error.contains("approval changed before release")
            || error.contains("verdict is not APPROVED"),
        "{error}"
    );
    assert!(!has_clearance(&root));
    assert!(mh::hold_is_open(&root, PR));
    let refusals = read_arming_history(&root, FRIENDLY)
        .unwrap()
        .archived
        .iter()
        .filter(|r| refuses(r))
        .count();
    assert!(refusals >= 2, "both refusals are retained ({refusals})");
    drop(_env);

    // A remap (uncommitted mapping change) between the passes: the label
    // bridge is no longer proved, the friendly label answers literally.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _env = AmbientGuard::hermetic_with_seat(&root, "reviewer", &[]);
    let store = commit_snapshot(&root);
    arm(&root);
    record(&root, NATIVE, "approved", HEAD);
    let (result, _) = release(
        &root,
        Io {
            heads: &[HEAD],
            grant: None,
            on_read: &|i| {
                if i == 2 {
                    let registry = store.join("registry");
                    std::fs::create_dir_all(&registry).unwrap();
                    std::fs::write(
                        registry.join("node_aliases.toml"),
                        "[[binding]]\nlabel = \"AWAY\"\nnode = \"b6b1278890774ef993d2ebc327862470\"\n",
                    )
                    .unwrap();
                }
            },
        },
    );
    assert!(result.is_err(), "a remap between passes refuses");
    assert!(mh::hold_is_open(&root, PR));
}

/// Without a provable snapshot (the store is not its own git repository) the
/// friendly label bridges nothing: the native approval does not answer it.
#[test]
fn bug_2403_unproved_labels_do_not_release() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _env = AmbientGuard::hermetic_with_seat(&root, "reviewer", &[]);
    arm(&root);
    record(&root, NATIVE, "approved", HEAD);
    assert_eq!(arming_labels(&root, FRIENDLY), vec![FRIENDLY.to_string()]);
    let (result, _) = release(&root, lawful(&[HEAD]));
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("verdict is not APPROVED"), "{error}");
    assert!(mh::hold_is_open(&root, PR));
}
