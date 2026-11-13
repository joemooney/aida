//! Standalone headless reviewers stop after recording their verdict.
// trace:BUG-2546 | ai:codex
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct Watch {
    roots: Vec<PathBuf>,
    key: String,
    keys: Vec<String>,
    since: chrono::DateTime<chrono::Utc>,
    grant: Option<String>,
    seen: Option<Instant>,
    recorded_at: Option<chrono::DateTime<chrono::Utc>>,
}
thread_local! { static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) }; }
pub(crate) struct Scope;
impl Scope {
    pub(crate) fn new(root: &Path, tree: &Path, pr: u64, specs: &[String]) -> Self {
        WATCH.with(|w| {
            *w.borrow_mut() = Some(Watch {
                roots: vec![root.into(), tree.into()],
                key: format!("PR-{pr}"),
                keys: specs
                    .iter()
                    .cloned()
                    .chain(std::iter::once(format!("PR-{pr}")))
                    .collect(),
                since: chrono::Utc::now(),
                grant: None,
                seen: None,
                recorded_at: None,
            })
        });
        Self
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        // Capture even a vendor that recorded and exited between polls.
        let _ = should_stop();
        WATCH.with(|w| {
            if let Some(w) = w.borrow_mut().take() {
                let runtime = w
                    .recorded_at
                    .map(|at| {
                        chrono::Utc::now()
                            .signed_duration_since(at)
                            .num_milliseconds()
                            .max(0)
                            .to_string()
                    })
                    .unwrap_or_else(|| "unknown".into());
                println!("reviewer: {} post_verdict_runtime_ms={runtime}", w.key);
            }
        });
    }
}
pub(crate) fn bind_grant(grant: &str) {
    WATCH.with(|w| {
        if let Some(w) = w.borrow_mut().as_mut() {
            w.grant = Some(grant.into());
        }
    });
}
// Interactive launches have no scope and retain their ordinary wait.
pub(crate) fn wait(child: &mut std::process::Child) -> std::io::Result<std::process::ExitStatus> {
    if !WATCH.with(|w| w.borrow().is_some()) {
        return child.wait();
    }
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if should_stop() {
            crate::headless_linger::terminate_tree(child)?;
            use std::os::unix::process::ExitStatusExt;
            return Ok(std::process::ExitStatus::from_raw(0));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn matches(
    v: &crate::gate::review_verdict::RecordedVerdict,
    grant: &str,
    since: chrono::DateTime<chrono::Utc>,
) -> bool {
    use crate::gate::review_verdict::VerdictKind;
    matches!(
        v.kind,
        VerdictKind::Approved
            | VerdictKind::RequestChanges
            | VerdictKind::Rejected
            | VerdictKind::Waiting
    ) && v
        .recorded_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .is_some_and(|at| at >= since)
        && v.attestation.as_ref().is_some_and(|a| {
            a.grant_id.as_deref() == Some(grant)
                && v.reviewed_sha.as_deref() == Some(a.sha.as_str())
        })
}
/// This is a stop signal only, never review acceptance or publication authority.
pub(crate) fn should_stop() -> bool {
    WATCH.with(|w| {
        let mut slot = w.borrow_mut();
        let Some(w) = slot.as_mut() else {
            return false;
        };
        if w.seen.is_none() {
            let Some(grant) = w.grant.as_deref() else {
                return false;
            };
            let verdict = w.roots.iter().find_map(|root| {
                w.keys.iter().find_map(|key| {
                    // Read the selected source once and inspect its raw delivery barrier.
                    // Normalization makes a pending approval look like a refusal;
                    // it must not arm shutdown while finding delivery is in flight.
                    // trace:BUG-2546 | ai:codex
                    let current = crate::gate::review_verdict::current_verdict(root, key)?;
                    let raw: serde_json::Value = serde_json::from_str(&current.body).ok()?;
                    if crate::gate::review_verdict::delivery_pending(&raw) {
                        return None;
                    }
                    crate::gate::review_verdict::parse_recorded_verdict(&current.body)
                        .filter(|v| matches(v, grant, w.since))
                })
            });
            if let Some(v) = verdict {
                w.recorded_at = v
                    .recorded_at
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|at| at.with_timezone(&chrono::Utc));
                w.seen = Some(Instant::now());
            }
        }
        w.seen
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(2))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::review_verdict::parse_recorded_verdict;

    #[test]
    fn bug2546_only_fresh_attested_terminal_verdicts_stop_the_lane() {
        let since = chrono::Utc::now();
        let mut body = serde_json::json!({
            "verdict": "APPROVED", "reviewed_sha": "abc",
            "recorded_at": (since + chrono::Duration::seconds(1)).to_rfc3339(),
            "recorder_attestation": { "sha": "abc", "grant_id": "child" }
        });
        let valid = |body: &serde_json::Value| {
            matches(
                &parse_recorded_verdict(&body.to_string()).unwrap(),
                "child",
                since,
            )
        };
        assert!(valid(&body));
        body["recorder_attestation"]["grant_id"] = "other".into();
        assert!(!valid(&body));
        body["recorder_attestation"]["grant_id"] = "child".into();
        body["recorded_at"] = (since - chrono::Duration::seconds(1)).to_rfc3339().into();
        assert!(!valid(&body));
        body["recorded_at"] = (since + chrono::Duration::seconds(1)).to_rfc3339().into();
        body["verdict"] = "WAITING".into();
        assert!(valid(&body));
        body["verdict"] = "unknown".into();
        assert!(!valid(&body));
        body["verdict"] = "REQUEST_CHANGES".into();
        assert!(valid(&body));
        body["reviewed_sha"] = "different".into();
        assert!(!valid(&body));
    }

    // trace:BUG-2546 | ai:codex
    #[test]
    fn bug2546_delayed_delivery_does_not_arm_shutdown_until_completed() {
        use crate::gate::review_verdict::{read_recorded_verdict, verdict_path, VerdictKind};
        let tmp = tempfile::tempdir().unwrap();
        let _scope = Scope::new(tmp.path(), tmp.path(), 7, &[]);
        bind_grant("child");
        let path = verdict_path(tmp.path(), "PR-7");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut body = serde_json::json!({
            "verdict": "APPROVED", "reviewed_sha": "abc", "sweep_pending": true,
            "recorded_at": chrono::Utc::now().to_rfc3339(),
            "recorder_attestation": { "sha": "abc", "grant_id": "child" }
        });
        std::fs::write(&path, body.to_string()).unwrap();
        assert_eq!(
            read_recorded_verdict(tmp.path(), "PR-7").unwrap().kind,
            VerdictKind::RequestChanges
        );
        assert!(!should_stop());
        // Delivery can outlast the shutdown grace while blocked on the store lock.
        std::thread::sleep(Duration::from_millis(2100));
        assert!(!should_stop());
        WATCH.with(|w| {
            let slot = w.borrow();
            let w = slot.as_ref().unwrap();
            assert!(w.seen.is_none());
            assert!(w.recorded_at.is_none());
        });
        body["sweep_pending"] = false.into();
        std::fs::write(&path, body.to_string()).unwrap();
        assert_eq!(
            read_recorded_verdict(tmp.path(), "PR-7").unwrap().kind,
            VerdictKind::Approved
        );
        assert!(
            !should_stop(),
            "completed delivery starts a fresh grace period"
        );
        WATCH.with(|w| {
            let mut slot = w.borrow_mut();
            let w = slot.as_mut().unwrap();
            assert!(w.seen.is_some());
            assert!(w.recorded_at.is_some());
            w.seen = Some(Instant::now() - Duration::from_secs(3));
        });
        assert!(should_stop());
    }

    // A pending barrier must keep the actual wait loop alive beyond its grace,
    // including a WAITING disposition. The completed twin must then reap it.
    // trace:BUG-2546 | ai:codex
    #[test]
    fn bug2546_wait_loop_preserves_pending_child_and_reaps_completed_twin() {
        use crate::gate::review_verdict::verdict_path;
        for kind in ["APPROVED", "WAITING"] {
            let tmp = tempfile::tempdir().unwrap();
            let _scope = Scope::new(tmp.path(), tmp.path(), 7, &[]);
            bind_grant("child");
            let path = verdict_path(tmp.path(), "PR-7");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut body = serde_json::json!({
                "verdict": kind, "reviewed_sha": "abc", "sweep_pending": true,
                "recorded_at": chrono::Utc::now().to_rfc3339(),
                "recorder_attestation": { "sha": "abc", "grant_id": "child" }
            });
            std::fs::write(&path, body.to_string()).unwrap();
            // Run the child past the grace before publishing completed delivery.
            // The child itself publishes its completed twin, so an early kill
            // leaves the barrier pending and makes the assertion below fail.
            body["sweep_pending"] = false.into();
            let completed = tmp.path().join("completed.json");
            std::fs::write(&completed, body.to_string()).unwrap();
            let mut child = std::process::Command::new("sh")
                .args([
                    "-c",
                    "sleep 3; mv -- \"$1\" \"$2\"; exec sleep 30",
                    "reviewer",
                ])
                .arg(&completed)
                .arg(&path)
                .spawn()
                .unwrap();
            assert!(wait(&mut child).unwrap().success());
            assert!(
                !child.try_wait().unwrap().unwrap().success(),
                "{kind}: the completion watcher must terminate the lingering child"
            );
            let raw: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(
                !crate::gate::review_verdict::delivery_pending(&raw),
                "{kind}"
            );
            WATCH.with(|w| {
                let slot = w.borrow();
                let w = slot.as_ref().unwrap();
                assert!(w.recorded_at.is_some(), "{kind}");
            });
        }
    }

    // WAITING completes the reviewer process, but never approves the change.
    // trace:BUG-2546 | ai:codex
    #[test]
    fn bug2546_waiting_stops_and_retains_runtime_timestamp_without_approval() {
        use crate::gate::review_verdict::{read_recorded_verdict, verdict_path, VerdictKind};
        let tmp = tempfile::tempdir().unwrap();
        let _scope = Scope::new(tmp.path(), tmp.path(), 7, &[]);
        bind_grant("child");
        let path = verdict_path(tmp.path(), "PR-7");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let recorded_at = chrono::Utc::now();
        let body = serde_json::json!({
            "verdict": "WAITING", "reviewed_sha": "abc",
            "recorded_at": recorded_at.to_rfc3339(),
            "recorder_attestation": { "sha": "abc", "grant_id": "child" }
        });
        std::fs::write(&path, body.to_string()).unwrap();
        let verdict = read_recorded_verdict(tmp.path(), "PR-7").unwrap();
        assert_eq!(verdict.kind, VerdictKind::Waiting);
        assert!(!verdict.kind.approves());
        assert!(!should_stop());
        WATCH.with(|w| {
            let mut slot = w.borrow_mut();
            let w = slot.as_mut().unwrap();
            assert_eq!(w.recorded_at, Some(recorded_at));
            assert!(w.seen.is_some());
            w.seen = Some(Instant::now() - Duration::from_secs(3));
        });
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        assert!(wait(&mut child).unwrap().success());
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn bug2546_verdict_stop_ignores_a_background_hold_and_reaps_child() {
        use std::process::Command;
        let tmp = tempfile::tempdir().unwrap();
        let _scope = Scope::new(tmp.path(), tmp.path(), 7, &[]);
        WATCH.with(|w| {
            w.borrow_mut().as_mut().unwrap().seen = Some(Instant::now() - Duration::from_secs(3))
        });
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let waited = crate::headless_linger::wait_reaping_after_result_holding(
            &mut child,
            &tmp.path().join("absent.jsonl"),
            Duration::from_secs(30),
            &|| true,
        )
        .unwrap();
        assert!(waited.reaped);
        assert!(waited.status.success());
        assert!(child.try_wait().unwrap().is_some());
        let mut resumed = Command::new("sleep").arg("30").spawn().unwrap();
        assert!(wait(&mut resumed).unwrap().success());
        assert!(resumed.try_wait().unwrap().is_some());
    }
}
