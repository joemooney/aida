//! Opt-in project review blocking policy. Legacy findings remain conservative.
// trace:TASK-3814 | ai:codex
use anyhow::{Context, Result};
use std::path::Path;

pub(crate) const TIERS: &[&str] = &["p1", "data-loss", "acceptance"];

pub(crate) fn enabled(root: &Path) -> Result<bool> {
    let text = match std::fs::read_to_string(root.join(".aida/config.toml")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).context("read review blocking policy"),
    };
    let config: toml::Value = toml::from_str(&text).context("parse review blocking policy")?;
    let Some(value) = config.get("review").and_then(|r| r.get("blocking_tiers")) else {
        return Ok(false);
    };
    let tiers = value
        .as_array()
        .context("review.blocking_tiers must be an array")?;
    anyhow::ensure!(
        tiers.len() == TIERS.len()
            && TIERS
                .iter()
                .all(|t| tiers.iter().any(|v| v.as_str() == Some(t))),
        "review.blocking_tiers must contain p1, data-loss and acceptance"
    );
    Ok(true)
}

pub(crate) fn prompt(root: &Path) -> Result<&'static str> {
    Ok(if enabled(root)? {
        "\n## Project review blocking policy\n\nOnly P1 defects, data loss and acceptance-criterion misses block. P2/P3 findings become APPROVE with follow-ups. Record every finding as structured --finding JSON with text, severity and blocking_tier: p1, p2, p3, data-loss, acceptance, mutation-kill or untested-path. In gate and sandbox code, mutation-kill and untested-path findings still block under ADR-76: use those tiers regardless of priority. Do not classify acceptance misses as P2. Unknown/unclassified findings block. The canonical writer defers p2/p3 findings and durably files each as a linked follow-up before approval; retain every finding, including mixed reviews. Do not manually omit findings or bypass failed follow-up delivery.\n"
    } else {
        ""
    })
}

/// Use the existing structured finding format, retaining the finding evidence.
/// Explicit classification is supplied by the reviewer, never guessed from prose.
pub(crate) fn apply(root: &Path, findings: &[String]) -> Result<Vec<String>> {
    if !enabled(root)? {
        return Ok(findings.to_vec());
    }
    Ok(findings
        .iter()
        .map(|raw| {
            let Ok(mut detail) = serde_json::from_str::<serde_json::Value>(raw) else {
                return raw.clone();
            };
            if !detail
                .get("text")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.trim().is_empty())
            {
                return raw.clone();
            }
            // trace:TASK-3814 | ai:codex — accepted class aliases must retain
            // the same conservative exception as their canonical spelling.
            let sensitive_class =
                detail
                    .get("class")
                    .and_then(|v| v.as_str())
                    .is_some_and(|class| {
                        class.trim().eq_ignore_ascii_case("mutation-kill")
                            || matches!(
                                crate::review_classes::parse_class(class),
                                crate::review_classes::ClassArg::Class("untested-path")
                            )
                    });
            let deferred = !sensitive_class
                && matches!(
                    detail.get("blocking_tier").and_then(|v| v.as_str()),
                    Some("p2" | "p3")
                );
            detail["disposition"] = if deferred { "deferred" } else { "blocking" }.into();
            if deferred {
                if !detail
                    .get("rationale")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.trim().is_empty() && !s.contains(['\n', '\r']))
                {
                    detail["rationale"] =
                        "Project policy routes below-tier findings to linked follow-ups".into();
                }
                if !detail
                    .get("severity")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.trim().is_empty())
                {
                    detail["severity"] = "minor".into();
                }
            }
            detail.to_string()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::review_verdict as rv;

    fn enable(root: &Path) {
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[review]\nblocking_tiers = [\"p1\", \"data-loss\", \"acceptance\"]\n",
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
}
