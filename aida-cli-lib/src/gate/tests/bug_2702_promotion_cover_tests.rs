//! BUG-2702: the mandate ship cover of a promotion PR. The head branch
//! `promote/integration-N` names no spec, so it can no longer make the cover
//! refuse as unresolved; a title or commit trailer naming an unknown spec
//! still does.
// trace:BUG-2702 | ai:claude
use super::*;
use crate::gate::mandate::{judge_exclusion_spec, Mandate, MandateRefusal, ShipCheck, ShipSpec};
use std::path::Path;

fn git(dir: &Path, args: &[&str]) -> String {
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
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// The spec half of the mandate's ship decision (`hold_cover`): every spec
/// resolved, at least one named, and each judged by the exclusion rules. The
/// approval half is not fabricated; a pass here returns `"covered"`.
fn spec_cover(check: &ShipCheck<'_>) -> std::result::Result<String, MandateRefusal> {
    if !check.specs_resolved || !check.specs.iter().any(|s| s.named) {
        return Err(MandateRefusal::SpecsUnresolved);
    }
    let ledger = check
        .ledger
        .as_ref()
        .map_err(|why| MandateRefusal::LedgerUnreadable(why.clone()))?;
    for spec in check.specs {
        judge_exclusion_spec(spec, ledger, false)?;
    }
    let named: Vec<String> = check
        .specs
        .iter()
        .filter(|s| s.named)
        .map(|s| s.id.clone())
        .collect();
    Ok(format!("covered {}", named.join(" ")))
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    head: String,
    gh: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        git(&root, &["init", "-q"]);
        git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/fixture/project.git",
            ],
        );
        std::fs::write(root.join("code.rs"), "fn changed() { let x = 1; }\n").unwrap();
        git(&root, &["add", "code.rs"]);
        git(&root, &["commit", "-qm", "base"]);
        let base = git(&root, &["rev-parse", "HEAD"]);
        git(&root, &["update-ref", "refs/remotes/origin/main", &base]);
        std::fs::write(root.join("code.rs"), "fn changed() { let x = 2; }\n").unwrap();
        git(&root, &["add", "code.rs"]);
        git(&root, &["commit", "-qm", "change (STORY-1)"]);
        let head = git(&root, &["rev-parse", "HEAD"]);
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "store_path = '.aida-store'\n",
        )
        .unwrap();
        let store = root.join(".aida-store");
        std::fs::create_dir(&store).unwrap();
        git(&store, &["init", "-q"]);
        for id in ["STORY-1", "STORY-2"] {
            let rel = aida_core::object_store::relative_object_path(id).unwrap();
            let path = store.join(&rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut req = aida_core::Requirement::new(format!("{id} work"), "fixture".into());
            req.req_type = aida_core::RequirementType::Story;
            req.execution_mode = Some(aida_core::ExecutionMode::Drive);
            req.spec_id = Some(id.into());
            std::fs::write(&path, serde_yaml::to_string(&req).unwrap()).unwrap();
        }
        git(&store, &["add", "-A"]);
        git(&store, &["commit", "-qm", "two stories"]);
        let gh = root.join("gh-stub");
        Self {
            _tmp: tmp,
            root,
            head,
            gh,
        }
    }

    /// Point the forge stub at a PR with this head ref and title.
    fn forge(&self, head_ref: &str, title: &str) {
        let json = serde_json::json!({
            "baseRefName": "main",
            "headRefName": head_ref,
            "headRefOid": self.head,
            "title": title,
            "state": "OPEN",
        });
        crate::test_exec::write_executable(
            &self.gh,
            format!("#!/bin/sh\nprintf '%s\\n' '{json}'\n"),
        );
    }

    /// `aida pr ship`'s mandate cover for PR 42 on `head_ref` titled `title`,
    /// its commits carrying `commit_subjects`: the ids assembled by the
    /// production `ship_gate_known_ids`, then the production collection.
    fn cover(
        &self,
        head_ref: &str,
        title: &str,
        commit_subjects: &[&str],
    ) -> (
        std::result::Result<MandateCover, MandateRefusal>,
        Vec<(String, bool)>,
    ) {
        self.forge(head_ref, title);
        let root = self.root.as_path();
        let known = crate::pr_service::ship_gate_known_ids(root, head_ref, Some(title), Some(42));
        let result = crate::with_review_pr_subject(
            crate::ReviewPrSubject {
                head_ref: head_ref.into(),
                commit_subjects: commit_subjects.iter().map(|s| s.to_string()).collect(),
            },
            || {
                collect_mandate_cover(
                    root,
                    root,
                    root,
                    42,
                    &known,
                    Some(head_ref),
                    || Some(self.head.clone()),
                    false,
                    |check| {
                        SEEN.with(|s| {
                            *s.borrow_mut() = check
                                .specs
                                .iter()
                                .map(|spec: &ShipSpec| (spec.id.clone(), spec.named))
                                .collect()
                        });
                        spec_cover(check)
                    },
                    false,
                    None,
                    false,
                )
            },
        );
        (result, SEEN.with(|s| s.take()))
    }
}

thread_local! {
    static SEEN: std::cell::RefCell<Vec<(String, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn active_mandate() -> Mandate {
    let now = chrono::Utc::now();
    Mandate {
        id: "6f1c1d6e-0000-4000-8000-000000002702".into(),
        principal: crate::current_user_id(None),
        seat: crate::gate::mandate::MANDATE_SEAT.into(),
        scope: crate::gate::mandate::SCOPE_SHIP_HELD.into(),
        include_supervised: false,
        include_gate_paths: false,
        reason: Some("fire".into()),
        issued_at: now - chrono::Duration::minutes(1),
        expires_at: now + chrono::Duration::hours(1),
        revoked_at: None,
        revoked_by: None,
        scopes: Default::default(),
        kind: None,
        renewals: Vec::new(),
        relay_parent: None,
        revived: None,
    }
}

fn plant(m: &Mandate) {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::gate::mandate::mandate_dir().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.json", m.id));
    std::fs::write(&path, serde_json::to_vec(m).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

const PROMOTION_TITLE: &str =
    "[AI:claude] chore(integration): two stories [promotion 23] (Refs STORY-1 STORY-2)";
const PROMOTION_COMMITS: &[&str] = &[
    "[AI:claude] feat(a): first story (STORY-1)",
    "[AI:claude] feat(b): second story (STORY-2)",
];

#[cfg(unix)]
#[test]
fn bug_2702_promotion_pr_cover_resolves_its_title_specs_and_unknown_title_ids_refuse() {
    // A PATH-reading production path with its own exclusion ledger.
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = Fixture::new();
    let home = fx.root.join("home");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.root,
        "advisor",
        &[],
        &[
            ("AIDA_HOME", Some(home.to_str().unwrap())),
            ("USER", Some(crate::test_env::HERMETIC_TEST_USER)),
            ("AIDA_TEST_GH_BINARY", Some(fx.gh.to_str().unwrap())),
        ],
    );
    let store = fx.root.join(".aida-store");
    git(&store, &["add", "-A"]);
    git(
        &store,
        &["commit", "--allow-empty", "-qm", "seed fixture seat roster"],
    );
    plant(&active_mandate());
    // The cover releases a typed, releasable hold.
    let hold = crate::gate::merge_hold_core::typed_hold(
        42,
        crate::gate::merge_hold_core::HoldReasonKind::Supervision,
        "operator review",
        None,
    );
    crate::gate::merge_hold_core::place_hold(&fx.root, &hold).unwrap();

    // AC1 + AC3: the promotion branch contributes no spec; the title's
    // inherited stories resolve and the cover passes.
    let (result, specs) = fx.cover("promote/integration-23", PROMOTION_TITLE, PROMOTION_COMMITS);
    assert_eq!(
        result.as_ref().map(|c| c.id.as_str()),
        Ok("covered STORY-1 STORY-2"),
        "{result:?} {specs:?}"
    );
    assert_eq!(
        specs,
        vec![("STORY-1".into(), true), ("STORY-2".into(), true)],
        "INTEGRATION-23 must never be a spec subject"
    );

    // AC3 control: a TITLE naming an unknown spec still refuses as unresolved.
    let (result, specs) = fx.cover(
        "promote/integration-24",
        "[AI:claude] chore(integration): [promotion 24] (Refs STORY-1 STORY-999)",
        PROMOTION_COMMITS,
    );
    assert_eq!(result, Err(MandateRefusal::SpecsUnresolved), "{specs:?}");
    assert!(specs.contains(&("STORY-999".into(), true)), "{specs:?}");

    // AC3 control: so does an unknown commit trailer.
    let (result, specs) = fx.cover(
        "promote/integration-25",
        PROMOTION_TITLE,
        &[
            PROMOTION_COMMITS[0],
            "[AI:claude] fix(c): stray (BUG-99999)",
        ],
    );
    assert_eq!(result, Err(MandateRefusal::SpecsUnresolved), "{specs:?}");
    assert!(specs.contains(&("BUG-99999".into(), true)), "{specs:?}");

    // AC2: a conventional branch id that resolves nowhere, named by neither
    // the title nor a trailer, is not a named unresolved spec.
    let (result, specs) = fx.cover("story-998-promo", PROMOTION_TITLE, PROMOTION_COMMITS);
    assert_eq!(
        result.as_ref().map(|c| c.id.as_str()),
        Ok("covered STORY-1 STORY-2"),
        "{result:?} {specs:?}"
    );
    assert!(specs.contains(&("STORY-998".into(), false)), "{specs:?}");

    // AC2 control: the same branch id named by the title stays named and
    // refuses.
    let (result, specs) = fx.cover(
        "story-998-promo",
        "[AI:claude] feat: x (STORY-998)",
        PROMOTION_COMMITS,
    );
    assert_eq!(result, Err(MandateRefusal::SpecsUnresolved), "{specs:?}");
    assert!(specs.contains(&("STORY-998".into(), true)), "{specs:?}");
}

#[test]
fn bug_2702_branch_only_ids_exclude_title_and_trailer_ids_and_need_the_forge() {
    let subject = |forge_resolved: bool| crate::pr_service::PrGateSubject {
        forge_resolved,
        branch_ids: vec!["STORY-998".into(), "BUG-7".into()],
        trailer_ids: vec!["BUG-0007".into()],
        ..Default::default()
    };
    assert_eq!(
        subject(true).branch_only_ids_given_title("feat (Refs TASK-1)"),
        vec!["STORY-998".to_string()]
    );
    assert!(subject(true)
        .branch_only_ids_given_title("x (story-998)")
        .is_empty());
    // Forge unanswered: nothing is provably branch-only.
    assert!(subject(false).branch_only_ids_given_title("").is_empty());
}

#[test]
fn bug_2702_only_an_unresolved_branch_only_id_loses_its_named_weight() {
    let branch_only = ["STORY-998".to_string(), "STORY-1".to_string()];
    // Branch-only and unresolved: judged like a trace-derived ID.
    assert!(unresolved_branch_only("STORY-998", false, &branch_only));
    // Branch-only but resolves: stays named.
    assert!(!unresolved_branch_only("STORY-1", true, &branch_only));
    // Unresolved but named elsewhere (not branch-only): stays named.
    assert!(!unresolved_branch_only("STORY-999", false, &branch_only));
    // A differently padded spelling is not matched: stays named.
    assert!(!unresolved_branch_only("STORY-0998", false, &branch_only));
}
