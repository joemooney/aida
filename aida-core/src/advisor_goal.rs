//! The advisor's standing goal: the operator's goal for the advisor seat,
//! kept in the store so a new or rotated advisor session (any vendor) reads
//! the same goal.
//!
//! One file on the store branch, `registry/advisor_goal.yaml`, holds every
//! goal ever set, oldest first. The last entry is the active goal; setting a
//! new goal appends, so the earlier ones stay as history. Each record carries
//! a `scope` key, today always the default branch, so per-branch goals can
//! key on it later without rewriting the data.
// trace:STORY-2243 | ai:claude

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The goal file, relative to the store root.
pub const ADVISOR_GOAL_REL: &str = "registry/advisor_goal.yaml";

/// What a clause asks to become true; the checker evaluates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClauseKind {
    /// Every spec tagged `batch:<arg>` is resolved.
    Batch,
    /// Every direct child of `<arg>` is resolved.
    Epic,
    /// Spec `<arg>` is Completed.
    Spec,
    /// PR/MR `<arg>` is merged.
    Pr,
    /// The `<arg>` role's queue is empty.
    QueueEmpty,
}

impl ClauseKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ClauseKind::Batch => "batch",
            ClauseKind::Epic => "epic",
            ClauseKind::Spec => "spec",
            ClauseKind::Pr => "pr",
            ClauseKind::QueueEmpty => "queue_empty",
        }
    }
}

/// One checkable clause, in the order the goal lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalClauseRecord {
    pub kind: ClauseKind,
    /// The clause's operand: batch name, spec ID, PR number or role.
    pub arg: String,
    /// Human-readable statement of the clause.
    pub description: String,
    /// The command that verifies it.
    pub verify: String,
}

/// One goal as set by the operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvisorGoal {
    pub text: String,
    #[serde(default)]
    pub clauses: Vec<GoalClauseRecord>,
    pub set_by: String,
    pub set_at: DateTime<Utc>,
    /// The branch the goal applies to. Today always the default branch.
    pub scope: String,
}

/// The whole file: every goal, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvisorGoalLog {
    #[serde(default)]
    pub goals: Vec<AdvisorGoal>,
}

impl AdvisorGoalLog {
    /// The active goal: the latest set.
    pub fn active(&self) -> Option<&AdvisorGoal> {
        self.goals.last()
    }

    /// Every goal, newest first.
    pub fn history(&self) -> impl Iterator<Item = &AdvisorGoal> {
        self.goals.iter().rev()
    }
}

/// Read the goal log from `store_root`. A missing file is an empty log; a
/// file that does not parse is an error (never silently "no goal").
pub fn load(store_root: &Path) -> Result<AdvisorGoalLog> {
    let path = store_root.join(ADVISOR_GOAL_REL);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AdvisorGoalLog::default()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(AdvisorGoalLog::default());
    }
    serde_yaml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// The active goal in `store_root`, or `None` when none is set or the file
/// cannot be read. For display surfaces that must not fail.
pub fn active(store_root: &Path) -> Option<AdvisorGoal> {
    load(store_root).ok()?.goals.pop()
}

fn write_log(store_root: &Path, log: &AdvisorGoalLog) -> Result<()> {
    let path = store_root.join(ADVISOR_GOAL_REL);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let yaml = serde_yaml::to_string(log)?;
    crate::fs_atomic::write_atomic(&path, yaml)?;
    Ok(())
}

/// Append `goal` as the new active goal and publish it: pull, append, commit
/// only the goal file, push; a rejected push is rolled back and retried (the
/// same push-wins loop as the team roster). With no `origin` the commit stays
/// local and the next store push carries it.
pub fn set_cas(store_root: &Path, goal: AdvisorGoal) -> Result<()> {
    use crate::git_ops;
    const MAX_RETRIES: u32 = 10;
    crate::team::guard_roster_write(store_root);
    let branch = git_ops::current_branch(store_root).unwrap_or_else(|_| "main".to_string());
    let local_only = !git_ops::has_remote(store_root, "origin");
    let message = format!("chore(registry): set advisor goal ({})", goal.set_by);

    for attempt in 0..MAX_RETRIES {
        let committed = crate::db::with_store_write_lock(store_root, || -> Result<String> {
            if attempt > 0 && !local_only {
                git_ops::pull_rebase(store_root, "origin", &branch)?;
            }
            let mut log = load(store_root)?;
            log.goals.push(goal.clone());
            write_log(store_root, &log)?;
            git_ops::add(store_root, &[ADVISOR_GOAL_REL])?;
            git_ops::commit_paths(store_root, &message, &[ADVISOR_GOAL_REL])?;
            git_ops::head_sha(store_root)
        })?;
        if local_only {
            return Ok(());
        }
        if git_ops::push_exact(store_root, "origin", &committed, &branch)? {
            return Ok(());
        }
        let _ = git_ops::rollback_commit_if_head_matches(store_root, &committed);
    }
    anyhow::bail!(
        "could not publish the advisor goal after {MAX_RETRIES} attempts (store push kept \
         being rejected); run `aida db sync --pull` and retry"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(text: &str, at: &str) -> AdvisorGoal {
        AdvisorGoal {
            text: text.to_string(),
            clauses: vec![GoalClauseRecord {
                kind: ClauseKind::Epic,
                arg: "EPIC-612".into(),
                description: "all direct children of EPIC-612 are resolved".into(),
                verify: "`aida list --parent EPIC-612` returns no rows".into(),
            }],
            set_by: "joe".into(),
            set_at: at.parse().unwrap(),
            scope: "main".into(),
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = crate::test_git::git_command()
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn local_store(dir: &Path) {
        crate::git_ops::init(dir).unwrap();
        crate::git_ops::configure_user(dir, "Test", "test@example.com").unwrap();
    }

    // AC7: the record carries `scope` (the default branch) under that name,
    // so per-branch goals can key on it.
    // trace:STORY-b6b1278890774ef993d2ebc327862470-272.accb6f41 | ai:claude
    #[test]
    fn record_pins_the_scope_field() {
        let yaml = serde_yaml::to_string(&goal("X", "2026-11-01T00:00:00Z")).unwrap();
        assert!(yaml.contains("scope: main"), "{yaml}");
        let back: AdvisorGoal = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.scope, "main");
        assert_eq!(back.clauses[0].kind, ClauseKind::Epic);
        assert!(yaml.contains("kind: epic"), "{yaml}");
    }

    #[test]
    fn missing_file_is_no_goal_and_garbage_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).unwrap().active().is_none());
        assert!(active(dir.path()).is_none());
        let path = dir.path().join(ADVISOR_GOAL_REL);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "goals: [ {").unwrap();
        assert!(load(dir.path()).is_err());
    }

    // AC5: a second set replaces the active goal; history lists both, newest
    // first. The write is committed to the store.
    // trace:STORY-b6b1278890774ef993d2ebc327862470-272.ac1f6c1e | ai:claude
    #[test]
    fn second_set_replaces_and_history_is_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        local_store(dir.path());
        set_cas(dir.path(), goal("first", "2026-11-01T00:00:00Z")).unwrap();
        set_cas(dir.path(), goal("second", "2026-11-01T01:00:00Z")).unwrap();
        let log = load(dir.path()).unwrap();
        assert_eq!(log.active().unwrap().text, "second");
        let texts: Vec<&str> = log.history().map(|g| g.text.as_str()).collect();
        assert_eq!(texts, ["second", "first"]);
        let status = crate::test_git::git_command()
            .current_dir(dir.path())
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "goal file committed"
        );
    }

    // AC2: the goal is canonical in the store: set in one clone, it reaches a
    // second clone after a pull.
    // trace:STORY-b6b1278890774ef993d2ebc327862470-272.ac0a7bc4 | ai:claude
    #[test]
    fn goal_set_in_one_clone_reaches_another_after_sync() {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let seed = root.path().join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        local_store(&seed);
        git(&seed, &["checkout", "-q", "-b", "main"]);
        std::fs::write(seed.join("metadata.yaml"), "x: 1\n").unwrap();
        git(&seed, &["add", "metadata.yaml"]);
        git(&seed, &["commit", "-q", "-m", "seed"]);
        git(
            root.path(),
            &[
                "clone",
                "-q",
                "--bare",
                seed.to_str().unwrap(),
                "origin.git",
            ],
        );
        let clone = |name: &str| {
            git(
                root.path(),
                &["clone", "-q", origin.to_str().unwrap(), name],
            );
            let p = root.path().join(name);
            crate::git_ops::configure_user(&p, "Test", "test@example.com").unwrap();
            p
        };
        let a = clone("a");
        let b = clone("b");

        set_cas(&a, goal("shared", "2026-11-01T00:00:00Z")).unwrap();
        assert!(active(&b).is_none(), "not visible before sync");
        git(&b, &["pull", "-q", "--rebase", "origin", "main"]);
        assert_eq!(active(&b).unwrap(), active(&a).unwrap());
    }
}
