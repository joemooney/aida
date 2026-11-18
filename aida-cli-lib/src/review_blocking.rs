//! Opt-in project review blocking policy. Legacy findings remain conservative.
// trace:TASK-3814 | ai:codex
use anyhow::Result;
use std::path::Path;

#[cfg(test)]
use crate::gate::kernel_deps::review_blocking::apply;
use crate::gate::kernel_deps::review_blocking::enabled;

pub(crate) fn prompt(root: &Path) -> Result<&'static str> {
    Ok(if enabled(root)? {
        "\n## Project review blocking policy\n\nP1 defects, data loss, security or authority bypass, and acceptance-criterion misses block. Other P2/P3 findings become APPROVE with follow-ups. Record every finding as structured --finding JSON with text, severity and blocking_tier: p1, p2, p3, data-loss, security, acceptance, mutation-kill or untested-path. Use security for security and authority-bypass findings regardless of priority. A structured class of security or authority-bypass also blocks. In gate and sandbox code, mutation-kill and untested-path findings still block under ADR-76: use those tiers regardless of priority. Do not classify acceptance misses as P2. Unknown/unclassified findings block. The canonical writer defers eligible p2/p3 findings and durably files each as a linked follow-up before approval; retain every finding, including mixed reviews. Do not manually omit findings or bypass failed follow-up delivery.\n"
    } else {
        ""
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::review_verdict as rv;

    fn enable(root: &Path) {
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[review]\nblocking_tiers = [\"p1\", \"data-loss\", \"security\", \"acceptance\"]\n",
        )
        .unwrap();
    }

    fn finding(tier: &str) -> String {
        serde_json::json!({"text":"a concrete defect", "severity":"minor", "blocking_tier":tier, "disposition":"blocking"}).to_string()
    }

    #[test]
    fn task_3814_absent_preserves_p2_refusal_and_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let findings = vec![finding("p2")];
        assert_eq!(apply(tmp.path(), &findings).unwrap(), findings);
        assert_eq!(
            rv::incoming_disposition("request-changes", &findings),
            rv::VerdictKind::RequestChanges
        );
        assert_eq!(prompt(tmp.path()).unwrap(), "");
        let path = rv::verdict_path(tmp.path(), "TASK-9821");
        let record = rv::build_verdict_object(
            tmp.path(),
            &path,
            Some("request-changes"),
            None,
            None,
            None,
            &findings,
            "reviewer",
        )
        .unwrap();
        assert_eq!(record["verdict"], "request-changes");
    }

    #[test]
    fn task_3814_enabled_approves_p2_but_never_p1_or_exceptions() {
        let tmp = tempfile::tempdir().unwrap();
        enable(tmp.path());
        let findings = apply(tmp.path(), &[finding("p2"), finding("p3")]).unwrap();
        assert!(rv::incoming_disposition("request-changes", &findings).approves());
        for tier in [
            "p1",
            "data-loss",
            "security",
            "acceptance",
            "mutation-kill",
            "untested-path",
            "unknown",
        ] {
            let findings = apply(tmp.path(), &[finding("p2"), finding(tier)]).unwrap();
            assert_eq!(
                rv::incoming_disposition("request-changes", &findings),
                rv::VerdictKind::RequestChanges,
                "{tier}"
            );
        }
        let legacy = apply(tmp.path(), &["[P2] no structured classification".into()]).unwrap();
        assert_eq!(
            rv::incoming_disposition("approved", &legacy),
            rv::VerdictKind::RequestChanges
        );
        assert!(prompt(tmp.path()).unwrap().contains("ADR-76"));
        assert!(
            crate::review_launch::review_prompt(tmp.path(), 42, false, "review")
                .unwrap()
                .unwrap()
                .contains("Project review blocking policy")
        );
    }

    #[test]
    fn task_3814_invalid_policy_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        enable(tmp.path());
        for setting in [
            "[]",
            "[\"p2\"]",
            "true",
            "[\"p1\", \"data-loss\", \"typo\"]",
            "[\"p1\", \"data-loss\", \"acceptance\"]",
        ] {
            std::fs::write(
                tmp.path().join(".aida/config.toml"),
                format!("[review]\nblocking_tiers = {setting}"),
            )
            .unwrap();
            assert!(apply(tmp.path(), &[finding("p2")]).is_err());
        }
    }

    #[test]
    fn task_3814_sensitive_class_aliases_keep_p2_blocking() {
        let tmp = tempfile::tempdir().unwrap();
        enable(tmp.path());
        for class in [
            "untested-path",
            "missing-test",
            "untested",
            "Untested_Path",
            "MUTATION-KILL",
        ] {
            let raw = serde_json::json!({
                "text": "gate behavior lacks regression evidence",
                "blocking_tier": "p2",
                "class": class,
            })
            .to_string();
            let findings = apply(tmp.path(), &[raw]).unwrap();
            assert_eq!(
                rv::incoming_disposition("approved", &findings),
                rv::VerdictKind::RequestChanges,
                "{class}"
            );
        }
    }

    #[test]
    fn task_3814_security_class_keeps_p2_blocking() {
        let tmp = tempfile::tempdir().unwrap();
        enable(tmp.path());
        for class in ["security", "authority-bypass", " Authority_Bypass "] {
            let findings = [
                finding("p2"),
                serde_json::json!({
                    "text": "security finding",
                    "blocking_tier": "p2",
                    "class": class,
                    "disposition": "deferred",
                })
                .to_string(),
            ];
            let record = rv::build_verdict_object(
                tmp.path(),
                &rv::verdict_path(tmp.path(), "TASK-9821"),
                Some("request-changes"),
                None,
                None,
                None,
                &findings,
                "reviewer",
            )
            .unwrap();
            assert_eq!(record["verdict"], "request-changes", "{class}");
        }
        assert!(prompt(tmp.path()).unwrap().contains("authority-bypass"));
    }
}
