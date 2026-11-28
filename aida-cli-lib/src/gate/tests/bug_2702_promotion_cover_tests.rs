//! BUG-2702: the mandate ship cover of a promotion PR, through the entry
//! `aida pr ship` calls ([`mandate_ship_cover_for_ship`]) with the production
//! decision, the production forge transport (commit subjects read from real
//! fixture commits) and a re-validated reviewer approval. The head branch
//! `promote/integration-N` names no spec, so it no longer makes the cover
//! refuse as unresolved; every ID that IS extracted — a spec-type branch
//! token, a title ID — keeps its fail-closed weight.
// trace:BUG-2702 | ai:claude
use super::*;
use crate::gate::mandate::{Mandate, MandateRefusal};
use std::path::Path;

pub(super) fn git(dir: &Path, args: &[&str]) -> String {
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

pub(super) const MANDATE_ID: &str = "6f1c1d6e-0000-4000-8000-000000002702";

pub(super) const PROMOTION_TITLE: &str =
    "[AI:claude] chore(integration): two stories [promotion 23] (Refs STORY-1 STORY-2)";

/// A project whose PR head carries two real commits, one per stored story,
/// and a store holding exactly those two stories.
pub(super) struct Fixture {
    _tmp: tempfile::TempDir,
    pub(super) root: std::path::PathBuf,
    pub(super) head: String,
    gh: std::path::PathBuf,
}

impl Fixture {
    pub(super) fn new() -> Self {
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
        for (n, subject) in [
            (2, "[AI:claude] feat(a): first story (STORY-1)"),
            (3, "[AI:claude] feat(b): second story (STORY-2)"),
        ] {
            std::fs::write(
                root.join("code.rs"),
                format!("fn changed() {{ let x = {n}; }}\n"),
            )
            .unwrap();
            git(&root, &["add", "code.rs"]);
            git(&root, &["commit", "-qm", subject]);
        }
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

    /// Point the forge stub at PR 42 with this head ref and title.
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

    /// Record an approval of the head by an independent reviewer seat: a
    /// live `reviewer` grant for the same principal, re-validated at ship
    /// time, its recorder identity distinct from the shipping session.
    pub(super) fn approve(&self) {
        let grant_id = crate::gate::seat_authority::test_support::mint_grant_for(
            &self.root,
            crate::test_env::HERMETIC_TEST_USER,
            "reviewer",
            &[],
        );
        let grant = crate::gate::seat_authority::read_grant(&grant_id).unwrap();
        let recorded_at = (chrono::Utc::now() + chrono::Duration::seconds(1)).to_rfc3339();
        let body = serde_json::json!({
            "verdict": "approved",
            "reviewed_sha": self.head,
            "recorded_at": recorded_at,
            "recorded_by": "review-pr-42 (claude reviewer seat)",
            crate::gate::review_verdict::ATTESTATION_KEY: {
                "sha": self.head,
                "seat": "reviewer",
                "grant_id": grant_id,
                "identity": ["session:reviewer-2702"],
                "grant_binding": grant.binding,
            },
        });
        let path = crate::gate::review_verdict::verdict_path(&self.root, "PR-42");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body.to_string()).unwrap();
    }

    /// `aida pr ship`'s mandate cover for PR 42 on `head_ref` titled `title`:
    /// the ids assembled by the production `ship_gate_known_ids`, then the
    /// production entry and decision.
    pub(super) fn ship_cover(
        &self,
        head_ref: &str,
        title: &str,
    ) -> std::result::Result<MandateCover, MandateRefusal> {
        self.forge(head_ref, title);
        let root = self.root.as_path();
        let known = crate::pr_service::ship_gate_known_ids(root, head_ref, Some(title), Some(42));
        mandate_ship_cover_for_ship(
            root,
            root,
            root,
            42,
            &known,
            Some(head_ref),
            || Some(self.head.clone()),
            false,
            false,
        )
    }
}

pub(super) fn active_mandate() -> Mandate {
    let now = chrono::Utc::now();
    Mandate {
        id: MANDATE_ID.into(),
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

pub(super) fn plant(m: &Mandate) {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::gate::mandate::mandate_dir().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.json", m.id));
    std::fs::write(&path, serde_json::to_vec(m).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(unix)]
#[test]
fn bug_2702_promotion_pr_is_covered_and_unknown_spec_ids_still_refuse() {
    // The production forge transport and a fresh mandate/exclusion home: run
    // in a PATH-isolated child process.
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let _transport = crate::review_pr_transport_fixture();
    let fx = Fixture::new();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.root,
        "advisor",
        &[],
        &[
            ("USER", Some(crate::test_env::HERMETIC_TEST_USER)),
            ("AIDA_TEST_GH_BINARY", Some(fx.gh.to_str().unwrap())),
            ("AIDA_CONTAINED", None),
            // The shipping advisor's session: anchored, and distinct from
            // the reviewer's recorded identity (two-person rule).
            ("AIDA_SESSION_ID", Some("advisor-2702")),
        ],
    );
    let store = fx.root.join(".aida-store");
    git(&store, &["add", "-A"]);
    git(
        &store,
        &["commit", "--allow-empty", "-qm", "seed fixture seat roster"],
    );
    plant(&active_mandate());
    fx.approve();
    // The cover releases a typed, releasable hold.
    let hold = crate::gate::merge_hold_core::typed_hold(
        42,
        crate::gate::merge_hold_core::HoldReasonKind::Supervision,
        "operator review",
        None,
    );
    crate::gate::merge_hold_core::place_hold(&fx.root, &hold).unwrap();

    // (a) AC1 + AC3: `promote/integration-23` names no spec; the inherited
    // stories resolve and the active mandate covers the ship.
    let covered = fx.ship_cover("promote/integration-23", PROMOTION_TITLE);
    assert_eq!(
        covered.as_ref().map(|c| c.id.as_str()),
        Ok(MANDATE_ID),
        "{covered:?}"
    );

    // (b) A spec-type branch token is still extracted, and an unresolvable
    // one still refuses (no demotion of branch-derived IDs).
    assert_eq!(
        fx.ship_cover("story-998-promo", PROMOTION_TITLE),
        Err(MandateRefusal::SpecsUnresolved)
    );

    // (c) AC3 control: a TITLE naming an unknown spec still refuses.
    assert_eq!(
        fx.ship_cover(
            "promote/integration-24",
            "[AI:claude] chore(integration): [promotion 24] (Refs STORY-1 STORY-999)",
        ),
        Err(MandateRefusal::SpecsUnresolved)
    );
}
