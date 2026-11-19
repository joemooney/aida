//! STORY-2893: `aida review verdict <KEY>` names the PR's next actor. Every
//! test drives the handler's own composition
//! (`review_verdict_show_with_next_actor`); only the forge observations are
//! injected.
// trace:STORY-2893 | ai:claude

use super::*;
use crate::gate::review_verdict::{self as rv, TipRelation};
use crate::pr_train::NextActor;
use std::cell::Cell;
use std::path::Path;

const BRANCH: &str = "story-2893-feature";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A checkout on `main` (an unrelated branch) holding a legacy verdict file
/// for `key` that approves `reviewed` on the feature branch.
fn checkout_on_main(key: &str, reviewed: &str, extra: serde_json::Value) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "main\n").unwrap();
    git(root, &["add", "a.txt"]);
    git(root, &["commit", "-qm", "main"]);
    let mut body = serde_json::json!({
        "verdict": "approved",
        "reviewed_sha": reviewed,
        "reviewed_branch": BRANCH,
        "recorded_by": "reviewer",
        "recorded_at": chrono::Utc::now().to_rfc3339(),
    });
    for (k, v) in extra.as_object().into_iter().flatten() {
        body[k] = v.clone();
    }
    let path = rv::verdict_path(root, key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body.to_string()).unwrap();
    tmp
}

fn train_at(head: &str) -> NextActor {
    NextActor {
        actor: "train",
        gate: "train-eligible",
        reason: "train admission passed".into(),
        head: Some(head.to_string()),
    }
}

/// The real local relation the handler computes: the checkout's branch.
fn local(root: &Path) -> impl FnOnce(Option<&str>) -> TipRelation + '_ {
    move |sha| verdict_tip_relation(root, current_branch_at(root).as_deref(), sha)
}

// Finding 2: from `main`, a legacy SPEC-keyed approval of an open PR's exact
// head resolves the PR through the verdict's reviewed branch.
#[test]
fn legacy_spec_verdict_resolves_its_pr_from_an_unrelated_checkout() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let head = "e5c1041".repeat(6)[..40].to_string();
    let tmp = checkout_on_main("STORY-1", &head, serde_json::json!({}));
    let root = tmp.path();
    assert_eq!(current_branch_at(root).as_deref(), Some("main"));

    let asked = Cell::new(None);
    let (_, value, _) = review_verdict_show_with_next_actor(
        root,
        "STORY-1",
        local(root),
        |_| true, // authority store substituted; see authority tests below
        |branch| {
            asked.set(Some(branch.to_string()));
            Some(4318)
        },
        |pr| {
            assert_eq!(pr, 4318);
            Some(train_at(&head))
        },
    )
    .unwrap();
    assert_eq!(asked.take().as_deref(), Some(BRANCH));
    assert_eq!(value["actionability"], "current-head-approved", "{value}");
    assert_eq!(value["next_actor"]["actor"], "train", "{value}");
    assert_eq!(value["next_actor"]["gate"], "train-eligible");

    // The PR head moved past the approval: still needs review, no actor.
    let (_, moved, _) = review_verdict_show_with_next_actor(
        root,
        "STORY-1",
        local(root),
        |_| true,
        |_| Some(4318),
        |_| Some(train_at(&"a".repeat(40))),
    )
    .unwrap();
    assert_eq!(moved["actionability"], "needs-review", "{moved}");
    assert!(moved.get("next_actor").is_none(), "{moved}");

    // No open PR for the reviewed branch: nothing is observed or claimed.
    let (_, none, _) = review_verdict_show_with_next_actor(
        root,
        "STORY-1",
        local(root),
        |_| true,
        |_| None,
        |_| panic!("no PR was resolved"),
    )
    .unwrap();
    assert_eq!(none["actionability"], "needs-review", "{none}");
    assert!(none.get("next_actor").is_none(), "{none}");
}

/// The handler's authority closure: the gate's own approval check, unchanged.
fn gate_authorized(root: &Path, key: &str, v: &rv::RecordedVerdict) -> bool {
    crate::gate::review_approval::independent_approval_authorized(
        root,
        &[key.to_string()],
        v.reviewed_branch.as_deref(),
        v,
    )
}

// Finding 3: an approval the authority check rejects is never relabelled,
// whether its head is the local HEAD or the observed PR head. Uses the real
// gate check over the invalid-authority fixtures.
#[test]
fn unauthorized_approval_stays_needs_review_through_the_handler() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    for label in ["unattested", "forged grant"] {
        let tmp = checkout_on_main("STORY-1", &"0".repeat(40), serde_json::json!({}));
        let root = tmp.path();
        // The approval names this checkout's own tip.
        let head = git(root, &["rev-parse", "HEAD"]);
        let path = rv::verdict_path(root, "STORY-1");
        let mut body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        body["reviewed_sha"] = serde_json::json!(head);
        if label == "forged grant" {
            body[rv::ATTESTATION_KEY] = serde_json::json!({
                "sha": head, "seat": "reviewer", "grant_id": "g-never-minted"
            });
        }
        std::fs::write(&path, body.to_string()).unwrap();
        let v = rv::parse_recorded_verdict(&body.to_string()).unwrap();
        assert!(v.kind.approves(), "{label}");
        assert!(
            !gate_authorized(root, "STORY-1", &v),
            "{label}: fixture must be an invalid authority"
        );

        let observed = Cell::new(false);
        let (_, value, _) = review_verdict_show_with_next_actor(
            root,
            "STORY-1",
            |_| TipRelation::AtReviewedSha,
            |v| gate_authorized(root, "STORY-1", v),
            |_| Some(4318),
            |_| {
                observed.set(true);
                Some(train_at(&head))
            },
        )
        .unwrap();
        assert_eq!(value["actionability"], "needs-review", "{label}: {value}");
        assert!(value.get("next_actor").is_none(), "{label}: {value}");
        assert!(
            !observed.get(),
            "{label}: no forge read for a refused approval"
        );

        // The same refusal holds for a PR-keyed record at the PR's head.
        let path = rv::verdict_path(root, "PR-4318");
        std::fs::write(&path, body.to_string()).unwrap();
        let (_, pr, _) = review_verdict_show_with_next_actor(
            root,
            "PR-4318",
            |_| TipRelation::Unknown,
            |v| gate_authorized(root, "PR-4318", v),
            |_| panic!("a PR key needs no branch lookup"),
            |_| Some(train_at(&head)),
        )
        .unwrap();
        assert_eq!(pr["actionability"], "needs-review", "{label}: {pr}");
        assert!(pr.get("next_actor").is_none(), "{label}: {pr}");
    }
}

// The renderer still separates a STRICT pre-merge review from the actor.
#[test]
fn approved_head_with_pending_reviewer_names_the_strict_review() {
    let root = tempfile::tempdir().unwrap();
    let head = "e5c1041".repeat(6)[..40].to_string();
    let mut value = serde_json::json!({"actionability": "needs-review"});
    render_verdict_next_actor(
        root.path(),
        &head,
        TipRelation::Unknown,
        Some(NextActor {
            actor: "reviewer",
            gate: "approval",
            reason: "observed gate".into(),
            head: Some(head.clone()),
        }),
        &mut value,
    );
    assert_eq!(value["actionability"], "current-head-approved");
    assert_eq!(
        value["strict_pre_merge_review_pending"],
        "independent reviewer"
    );
}
