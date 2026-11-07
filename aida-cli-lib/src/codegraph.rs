//! Shared primary-checkout code navigation. This link grants no sandbox access;
//! containment read admission is a separate boundary.
// trace:STORY-2630 | ai:codex
use crate::doctor_root::DoctorFinding;
use anyhow::Result;
use rusqlite::OptionalExtension;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub(crate) const CATEGORY: &str = "codegraph";
const INDEX: &str = ".codegraph/codegraph.db";
const MAX_AGE: Duration = Duration::from_secs(3600);

/// Plan the link for the existing seat-clone/linked-worktree setup writer. Never copy or
/// build a private index, replace existing content, or chmod the shared index.
/// A symlink is routing only, not a read-only security boundary.
#[cfg(unix)]
pub(crate) fn primary_index_link(
    primary: &Path,
    seat: &Path,
) -> Result<Option<(PathBuf, PathBuf)>> {
    if primary == seat || !primary.join(INDEX).is_file() {
        return Ok(None);
    }
    let source = primary.join(".codegraph").canonicalize()?;
    let destination = seat.join(".codegraph");
    match std::fs::symlink_metadata(&destination) {
        Ok(meta) => {
            anyhow::ensure!(
                meta.file_type().is_symlink() && destination.canonicalize().ok().as_ref() == Some(&source),
                "{} already exists and does not link to the primary codegraph index; preserve it and resolve manually",
                destination.display()
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some((source, destination)));
        }
        Err(e) => return Err(e.into()),
    }
    Ok(None)
}

pub(crate) fn scan(root: &Path) -> Vec<DoctorFinding> {
    assess(
        root,
        crate::which_binary("codegraph").is_some(),
        SystemTime::now(),
    )
}

fn finding(id: &str, summary: String, action: &str) -> DoctorFinding {
    DoctorFinding {
        category: CATEGORY.into(),
        id: id.into(),
        summary,
        action: action.into(),
        safe_heal: false,
    }
}

/// Read-only and store-free: no codegraph subprocess, indexer or repair. Use
/// the database mtime for freshness; a WAL write can be newer than its db.
fn assess(root: &Path, binary_present: bool, now: SystemTime) -> Vec<DoctorFinding> {
    let mut findings = Vec::new();
    let index_present = root.join(".codegraph").is_dir();
    if !index_present && !binary_present {
        return findings;
    }
    if index_present && !binary_present {
        findings.push(finding(
            "missing",
            "codegraph is missing from PATH".into(),
            "Install codegraph on this host; maintain its index in the primary checkout.",
        ));
    }
    let path = root.join(INDEX);
    let reachable = (|| -> Result<SystemTime> {
        let meta = std::fs::metadata(&path)?;
        anyhow::ensure!(meta.is_file(), "index is not a regular file");
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        conn.busy_timeout(Duration::from_millis(100))?;
        // Probe the files table without scanning a potentially large graph.
        // An empty but valid index is reachable too.
        let _: Option<i64> = conn
            .query_row("SELECT 1 FROM files LIMIT 1", [], |row| row.get(0))
            .optional()?;
        let mut modified = meta.modified()?;
        if let Ok(wal) = std::fs::metadata(root.join(".codegraph/codegraph.db-wal")) {
            if wal.len() > 0 {
                modified = modified.max(wal.modified()?);
            }
        }
        Ok(modified)
    })();
    match reachable {
        Err(error) => findings.push(finding("unreachable", format!("codegraph index unreachable from {}: {error:#}", root.display()),
            "Check the primary index, seat .codegraph link and read-only containment access; do not initialize a lane index.")),
        Ok(modified) if now.duration_since(modified).is_ok_and(|age| age > MAX_AGE) => {
            findings.push(finding("stale", format!("codegraph index at {} is older than one hour", path.display()),
                "Check the primary checkout's codegraph sync job; refresh only on the host."));
        }
        Ok(_) => {}
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(root: &Path) {
        std::fs::create_dir_all(root.join(".codegraph")).unwrap();
        rusqlite::Connection::open(root.join(INDEX))
            .unwrap()
            .execute_batch("CREATE TABLE files (path TEXT);")
            .unwrap();
    }

    #[test]
    fn seat_uses_primary_index_without_copying_or_replacing() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = tmp.path().join("primary");
        let seat = tmp.path().join("seat");
        std::fs::create_dir_all(&seat).unwrap();
        index(&primary);
        aida_core::git_ops::init(&seat).unwrap();
        crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
        crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
        assert!(seat.join(".codegraph").is_symlink());
        assert_eq!(
            seat.join(INDEX).canonicalize().unwrap(),
            primary.join(INDEX).canonicalize().unwrap()
        );
        assert!(assess(&seat, true, SystemTime::now()).is_empty());
        // Primary removal is visible immediately, rather than leaving a stale copy.
        std::fs::remove_file(primary.join(INDEX)).unwrap();
        assert_eq!(assess(&seat, true, SystemTime::now())[0].id, "unreachable");
    }

    #[test]
    fn setup_skips_absent_index_and_preserves_conflicting_content() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = tmp.path().join("primary");
        let seat = tmp.path().join("seat");
        std::fs::create_dir_all(&seat).unwrap();
        crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
        assert!(!seat.join(".codegraph").exists());
        index(&primary);
        index(&seat);
        std::fs::create_dir(primary.join(".aida-store")).unwrap();
        std::fs::create_dir_all(primary.join(".aida/sessions")).unwrap();
        crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
        assert!(seat.join(".aida-store").is_symlink());
        assert!(seat.join(".aida/sessions").is_symlink());
        assert!(seat.join(INDEX).is_file());
        assert!(!seat.join(".codegraph").is_symlink());
        // A foreign link (including a dangling one) is preserved too.
        std::fs::remove_dir_all(seat.join(".codegraph")).unwrap();
        let foreign = tmp.path().join("foreign");
        for exists in [false, true] {
            if exists {
                std::fs::create_dir(&foreign).unwrap();
            }
            std::os::unix::fs::symlink(&foreign, seat.join(".codegraph")).unwrap();
            std::fs::remove_file(seat.join(".aida-store")).unwrap();
            std::fs::remove_file(seat.join(".aida/sessions")).unwrap();
            crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
            assert_eq!(
                std::fs::read_link(seat.join(".codegraph")).unwrap(),
                foreign
            );
            assert!(seat.join(".aida-store").is_symlink());
            assert!(seat.join(".aida/sessions").is_symlink());
            std::fs::remove_file(seat.join(".codegraph")).unwrap();
        }
    }

    #[test]
    fn doctor_reports_missing_unreachable_stale_and_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(assess(root, false, SystemTime::now()).is_empty());
        assert_eq!(assess(root, true, SystemTime::now())[0].id, "unreachable");
        assert!(
            !root.join(".codegraph").exists(),
            "doctor must not create an index"
        );
        std::fs::create_dir(root.join(".codegraph")).unwrap();
        let findings = assess(root, false, SystemTime::now());
        assert_eq!(
            findings.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["missing", "unreachable"]
        );
        index(root);
        assert_eq!(assess(root, false, SystemTime::now())[0].id, "missing");
        let modified = std::fs::metadata(root.join(INDEX))
            .unwrap()
            .modified()
            .unwrap();
        assert!(assess(root, true, modified + MAX_AGE).is_empty());
        assert_eq!(
            assess(root, true, modified + MAX_AGE + Duration::from_secs(1))[0].id,
            "stale"
        );
        std::fs::write(root.join(INDEX), "not sqlite").unwrap();
        assert_eq!(assess(root, true, SystemTime::now())[0].id, "unreachable");
    }

    #[test]
    fn assembled_skill_prompts_keep_invocation_first_and_include_navigation() {
        use crate::queue_cmd::{QueueWorkMode, QueueWorkPlan};
        use crate::session_start::ReviewForge;
        let plan = QueueWorkPlan {
            mode: QueueWorkMode::Item,
            entries: Vec::new(),
            scope: "STORY-2630".into(),
            review_target: Some((ReviewForge::GitHub, 3607)),
            anchor_display: "STORY-2630".into(),
            anchor_title: "navigation".into(),
        };
        for (role, gate) in [
            ("implementer", None),
            ("reviewer", None),
            ("security", Some("security")),
        ] {
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("AIDA_AGENT_GATE_NAME", gate),
                ("AIDA_AGENT_GATE_ROLE", gate),
                ("AIDA_REVIEW_ROUND", Some("1")),
            ]);
            let prompt = crate::rework_direction::assemble_pickup_prompt(
                &crate::rework_direction::PickupInputs {
                    plan: &plan,
                    role,
                    plan_only: false,
                    guided: false,
                    review_findings: None,
                    spec_comments: &[],
                    conflict: None,
                    ci_failures: None,
                },
            );
            let invocation = if role == "implementer" {
                "/aida-pickup STORY-2630"
            } else {
                "/aida-review --pr 3607"
            };
            assert_eq!(prompt.lines().next(), Some(invocation), "{prompt}");
            if gate.is_some() {
                assert!(prompt.contains("AIDA_REVIEW_VERDICT_FILE"), "{prompt}");
            } else if role == "reviewer" {
                assert!(prompt.contains("ROUND 1"), "{prompt}");
            }
            assert!(prompt.contains(
                include_str!("../../aida-core/templates/dispatch/code-navigation.md").trim()
            ));
        }
    }

    // trace:STORY-2630 | ai:codex
    #[test]
    fn root_agents_navigation_matches_template_master() {
        let agents = include_str!("../../AGENTS.md");
        let navigation = agents
            .split("For code navigation, ")
            .nth(1)
            .unwrap()
            .split("\n<!-- trace:STORY-2630")
            .next()
            .unwrap();
        assert_eq!(
            format!("For code navigation, {navigation}").trim(),
            include_str!("../../aida-core/templates/dispatch/code-navigation.md").trim()
        );
    }

    // trace:STORY-2630 | ai:codex
    #[test]
    fn reentered_old_seat_excludes_navigation_link_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = tmp.path().join("primary");
        let seat = tmp.path().join("seat");
        index(&primary);
        aida_core::git_ops::init(&seat).unwrap();
        std::fs::write(seat.join(".gitignore"), ".codegraph/\n").unwrap();
        let exclude = seat.join(".git/info/exclude");
        std::fs::write(&exclude, "# old seat\n/.aida/*\n.aida-store\n").unwrap();
        for _ in 0..2 {
            crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
            assert!(seat.join(".codegraph").is_symlink());
            let ignored = crate::test_git::git_command()
                .current_dir(&seat)
                .args(["check-ignore", ".codegraph"])
                .output()
                .unwrap();
            assert!(ignored.status.success(), "{ignored:?}");
        }
        let body = std::fs::read_to_string(exclude).unwrap();
        assert!(body.starts_with("# old seat\n"), "{body}");
        assert_eq!(body.lines().filter(|line| *line == ".codegraph").count(), 1);
    }

    #[test]
    fn wal_activity_is_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        index(tmp.path());
        let old = SystemTime::now() - Duration::from_secs(7200);
        std::fs::File::open(tmp.path().join(INDEX))
            .unwrap()
            .set_modified(old)
            .unwrap();
        // A valid live WAL is held open by its writer.
        let conn = rusqlite::Connection::open(tmp.path().join(INDEX)).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; INSERT INTO files VALUES ('fresh.rs');")
            .unwrap();
        std::fs::File::open(tmp.path().join(INDEX))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(assess(tmp.path(), true, SystemTime::now()).is_empty());
        assert_eq!(
            crate::normalize_doctor_category("code-graph").unwrap(),
            CATEGORY
        );
    }
}
