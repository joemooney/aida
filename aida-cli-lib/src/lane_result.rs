//! Host-owned admission-through-publication receipts. An absent terminal result
//! means interrupted/unfinished, never a claimed successful launch.
// trace:STORY-2690 | ai:codex
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Kind {
    Pushed,
    NoChangeWithReason,
    RefusedBeforeStart,
    Punted,
    Held,
    FailedAfterStart,
}

#[derive(Debug, Serialize, Deserialize)]
struct Receipt {
    version: u8,
    attempt: uuid::Uuid,
    spec: String,
    branch: Option<String>,
    pr: Option<u64>,
    initial_head: Option<String>,
    head: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    started: bool,
    child_pid: Option<u32>,
    finished_at: Option<chrono::DateTime<chrono::Utc>>,
    result: Option<Kind>,
    reason_code: Option<String>,
    detail: Option<String>,
    untested: bool,
}

pub(crate) struct Attempt {
    path: PathBuf,
    receipt: RefCell<Receipt>,
    start_error: RefCell<Option<String>>,
}

impl Attempt {
    pub(crate) fn begin(main: &Path, spec: &str, branch: Option<&str>) -> Result<Self> {
        let id = uuid::Uuid::new_v4();
        let attempt = Self {
            path: crate::main_worktree_root_from(main)
                .join(".aida/lane-results")
                .join(format!("{id}.json")),
            receipt: RefCell::new(Receipt {
                version: 1,
                attempt: id,
                spec: spec.into(),
                branch: branch.map(str::to_owned),
                pr: None,
                initial_head: None,
                head: None,
                created_at: chrono::Utc::now(),
                started: false,
                child_pid: None,
                finished_at: None,
                result: None,
                reason_code: None,
                detail: None,
                untested: true,
            }),
            start_error: RefCell::new(None),
        };
        attempt.persist()?;
        Ok(attempt)
    }

    fn persist(&self) -> Result<()> {
        std::fs::create_dir_all(self.path.parent().unwrap())?;
        let pending = self.path.with_extension("pending");
        std::fs::write(
            &pending,
            serde_json::to_vec_pretty(&*self.receipt.borrow())?,
        )?;
        std::fs::rename(&pending, &self.path).context("could not persist lane result receipt")
    }

    pub(crate) fn bind(
        &self,
        spec: &str,
        branch: &str,
        pr: Option<&(u64, Option<String>)>,
        seat_head: Option<&str>,
    ) -> Result<()> {
        {
            let mut r = self.receipt.borrow_mut();
            r.spec = spec.into();
            r.branch = Some(branch.into());
            r.pr = pr.map(|p| p.0);
            r.initial_head = pr
                .and_then(|p| p.1.clone())
                .or_else(|| seat_head.map(str::to_owned));
        }
        self.persist()
    }

    // The spawn hook cannot return an error. Retain it and refuse publication
    // at the next fallible boundary, while remembering the actual spawn.
    pub(crate) fn started(&self, pid: u32) {
        {
            let mut r = self.receipt.borrow_mut();
            r.started = true;
            r.child_pid = Some(pid);
        }
        if let Err(e) = self.persist() {
            *self.start_error.borrow_mut() = Some(format!("{e:#}"));
        }
    }

    pub(crate) fn check_started(&self) -> Result<()> {
        if let Some(e) = &*self.start_error.borrow() {
            anyhow::bail!("lane start receipt failed: {e}");
        }
        anyhow::ensure!(
            self.receipt.borrow().started,
            "child returned without a successful spawn receipt"
        );
        Ok(())
    }

    pub(crate) fn finish(
        &self,
        kind: Kind,
        reason: &str,
        detail: &str,
        pr: Option<u64>,
        head: Option<&str>,
        untested: bool,
    ) -> Result<()> {
        {
            let mut r = self.receipt.borrow_mut();
            anyhow::ensure!(r.result.is_none(), "lane attempt already finalized");
            anyhow::ensure!(
                kind == Kind::RefusedBeforeStart || r.started,
                "unstarted lane cannot claim a child result"
            );
            anyhow::ensure!(
                kind != Kind::RefusedBeforeStart || !r.started,
                "started lane cannot claim admission refusal"
            );
            if kind == Kind::Pushed {
                anyhow::ensure!(
                    pr.is_some()
                        && head.is_some()
                        && r.initial_head.is_some()
                        && head != r.initial_head.as_deref(),
                    "pushed requires an observed new published head"
                );
            }
            r.result = Some(kind);
            r.reason_code = Some(reason.into());
            r.detail = Some(detail.into());
            r.finished_at = Some(chrono::Utc::now());
            r.untested = untested;
            if pr.is_some() {
                r.pr = pr;
            }
            r.head = head.map(str::to_owned);
        }
        self.persist()
    }

    // Stage enums remain internal; this is the terminal result contract.
    pub(crate) fn finish_outcome(
        &self,
        outcome: &crate::standalone_implementer::Outcome,
        success: bool,
        unshipped: bool,
        untested: bool,
    ) -> Result<Kind> {
        use crate::standalone_implementer::Outcome;
        let initial = self.receipt.borrow().initial_head.clone();
        let (kind, reason, detail, pr, head) = match outcome {
            Outcome::Punted { reason } => {
                (Kind::Punted, "explicit-punt", reason.as_str(), None, None)
            }
            Outcome::Held { .. } => (Kind::Held, "explicit-hold", "manual gate", None, None),
            Outcome::PrOpened {
                pr,
                head: Some(head),
            } if initial.as_deref().is_some_and(|old| old != head) => (
                Kind::Pushed,
                "new-published-head",
                "observed new published head",
                Some(*pr),
                Some(head.as_str()),
            ),
            Outcome::PrOpened {
                pr,
                head: Some(head),
            } if success && !unshipped && initial.as_deref() == Some(head.as_str()) => (
                Kind::NoChangeWithReason,
                "unchanged-published-head",
                "published head unchanged and no pending work",
                Some(*pr),
                Some(head.as_str()),
            ),
            Outcome::NoChange if success => (
                Kind::NoChangeWithReason,
                "no-work-produced",
                "no commits beyond base",
                None,
                None,
            ),
            _ => (
                Kind::FailedAfterStart,
                "no-new-published-head",
                "lane did not publish a new head",
                None,
                None,
            ),
        };
        self.finish(kind, reason, detail, pr, head, untested)?;
        Ok(kind)
    }

    pub(crate) fn kind(&self) -> Option<Kind> {
        self.receipt.borrow().result
    }

    pub(crate) fn set_branch(&self, branch: &str) -> Result<()> {
        self.receipt.borrow_mut().branch = Some(branch.into());
        self.persist()
    }

    pub(crate) fn is_started(&self) -> bool {
        self.receipt.borrow().started
    }
    pub(crate) fn is_finished(&self) -> bool {
        self.receipt.borrow().result.is_some()
    }

    pub(crate) fn finish_result(&self, result: &Result<()>) -> Result<()> {
        if self.receipt.borrow().result.is_some() {
            return self.persist();
        }
        let started = self.receipt.borrow().started;
        let detail = result
            .as_ref()
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_else(|| "no terminal lane outcome recorded".into());
        self.finish(
            if started {
                Kind::FailedAfterStart
            } else {
                Kind::RefusedBeforeStart
            },
            if started {
                "lane-failed"
            } else {
                "admission-refused"
            },
            &detail,
            None,
            None,
            true,
        )?;
        println!("{}", self.line()?);
        Ok(())
    }

    pub(crate) fn line(&self) -> Result<String> {
        let r = self.receipt.borrow();
        let kind = serde_json::to_value(r.result.context("unfinished lane receipt")?)?;
        Ok(format!(
            "implementer: {} attempt={} started={} reason={} pr={} head={} untested={}",
            kind.as_str().unwrap(),
            r.attempt,
            r.started,
            r.reason_code.as_deref().unwrap_or("unknown"),
            r.pr.map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".into()),
            r.head.as_deref().unwrap_or("unknown"),
            r.untested
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_2690_admission_and_spawn_failures_are_distinct_and_durable() {
        let tmp = tempfile::tempdir().unwrap();
        for started in [false, true] {
            let a = Attempt::begin(tmp.path(), "STORY-test", Some("work")).unwrap();
            if started {
                a.started(123);
            }
            a.finish_result(&Err(anyhow::anyhow!("injected failure")))
                .unwrap();
            let r: Receipt = serde_json::from_slice(&std::fs::read(&a.path).unwrap()).unwrap();
            assert_eq!(r.started, started);
            assert_eq!(
                r.result,
                Some(if started {
                    Kind::FailedAfterStart
                } else {
                    Kind::RefusedBeforeStart
                })
            );
            assert!(r.finished_at.is_some());
        }
    }

    #[test]
    fn story_2690_existing_pr_is_not_a_push_and_explicit_signals_survive_error_finalization() {
        let tmp = tempfile::tempdir().unwrap();
        for kind in [
            Kind::Pushed,
            Kind::Punted,
            Kind::Held,
            Kind::NoChangeWithReason,
        ] {
            let a = Attempt::begin(tmp.path(), "STORY-test", Some("work")).unwrap();
            a.bind("STORY-test", "work", Some(&(42, Some("old".into()))), None)
                .unwrap();
            a.started(123);
            if kind == Kind::Pushed {
                assert!(a
                    .finish(kind, "push", "", Some(42), Some("old"), false)
                    .is_err());
            }
            a.finish(kind, "test", "reason", Some(42), Some("new"), false)
                .unwrap();
            a.finish_result(&Err(anyhow::anyhow!("later error")))
                .unwrap();
            assert_eq!(a.receipt.borrow().result, Some(kind));
        }
    }
    #[test]
    fn story_2690_stage_results_require_new_head_and_preserve_vetoes() {
        use crate::standalone_implementer::Outcome;
        let tmp = tempfile::tempdir().unwrap();
        for (outcome, success, unshipped, expected) in [
            (
                Outcome::PrOpened {
                    pr: 42,
                    head: Some("old".into()),
                },
                true,
                false,
                Kind::NoChangeWithReason,
            ),
            (
                Outcome::PrOpened {
                    pr: 42,
                    head: Some("old".into()),
                },
                true,
                true,
                Kind::FailedAfterStart,
            ),
            (
                Outcome::PrOpened {
                    pr: 42,
                    head: Some("new".into()),
                },
                true,
                false,
                Kind::Pushed,
            ),
            (
                Outcome::PrOpened { pr: 42, head: None },
                true,
                false,
                Kind::FailedAfterStart,
            ),
            (Outcome::NoChange, false, false, Kind::FailedAfterStart),
            (Outcome::NoPr, false, false, Kind::FailedAfterStart),
            (
                Outcome::Punted {
                    reason: "manual decision".into(),
                },
                false,
                true,
                Kind::Punted,
            ),
            (
                Outcome::Held {
                    branch: "work".into(),
                },
                false,
                true,
                Kind::Held,
            ),
        ] {
            let a = Attempt::begin(tmp.path(), "STORY-test", Some("work")).unwrap();
            a.bind("STORY-test", "work", None, Some("old")).unwrap();
            a.started(123);
            assert_eq!(
                a.finish_outcome(&outcome, success, unshipped, false)
                    .unwrap(),
                expected
            );
            let persisted: Receipt =
                serde_json::from_slice(&std::fs::read(&a.path).unwrap()).unwrap();
            assert_eq!(persisted.result, Some(expected));
        }
    }
}
