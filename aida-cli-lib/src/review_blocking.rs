//! Project review blocking policy: the review-prompt text that quotes the
//! active `[review] blocking_tiers` list, and the authority-checked
//! `aida review blocking-tiers` command that changes it.
// trace:TASK-3814 | ai:codex
// trace:STORY-2913 | ai:claude
use anyhow::Result;
use std::path::Path;

#[cfg(test)]
use crate::gate::kernel_deps::review_blocking::apply;
use crate::gate::kernel_deps::review_blocking::{self as policy, Effective, Policy, Source};

/// How the trusted source is named in output.
// trace:STORY-2913 | ai:claude
pub(crate) const DEFAULT_BRANCH_CONFIG: &str = "default-branch .aida/config.toml";

/// One line naming the active list and where it came from.
// trace:STORY-2913 | ai:claude
pub(crate) fn describe(effective: &Effective) -> String {
    let list = match &effective.policy {
        Policy::AllBlock => "default — every finding blocks".to_string(),
        Policy::Tiers(set) => set.iter().copied().collect::<Vec<_>>().join(", "),
    };
    let from = match &effective.source {
        Source::Unset => "not set on the default branch".to_string(),
        Source::DefaultBranch => DEFAULT_BRANCH_CONFIG.to_string(),
        Source::Unavailable => "default-branch config unreadable".to_string(),
        Source::Invalid(why) => format!("invalid on the default branch: {why}"),
    };
    format!("{list} ({from})")
}

/// The review-prompt section. Every prompt quotes the active list; the
/// classification instructions follow only when a list is in force.
// trace:STORY-2913 | ai:claude
pub(crate) fn prompt(root: &Path) -> Result<String> {
    let effective = policy::effective(root);
    let mut out = format!(
        "\n## Project review blocking policy\n\nActive blocking classes: {}.\n",
        describe(&effective)
    );
    if let Policy::Tiers(set) = &effective.policy {
        let defer: Vec<&str> = policy::registry_names()
            .into_iter()
            .filter(|c| !set.contains(c))
            .collect();
        out.push_str(&format!(
            "\nFindings in a listed class block. Findings only in {} become APPROVE with follow-ups. \
Record every finding as structured --finding JSON with text, severity and blocking_tier, one of: {}; add a `class` member when a second class applies (a finding blocks when its tier or its class is listed). \
Use security-authority-bypass for security and authority-bypass findings regardless of priority: it blocks whatever the list says. \
In gate, sandbox, authority, seat and containment code, test-gap findings still block under ADR-76: use the blocking_tier mutation-kill or untested-path, or give the finding a test-gap class plus a `file` member naming that code. The same class elsewhere (for example reporting or documentation) follows the list. \
Do not classify an unmet acceptance criterion the change claims as anything but unmet-claimed-ac. Unknown/unclassified findings block. \
The canonical writer defers eligible findings and durably files each as a linked follow-up before approval; retain every finding, including mixed reviews. Do not manually omit findings or bypass failed follow-up delivery.\n",
            if defer.is_empty() { "no class".to_string() } else { defer.join(", ") },
            policy::registry_names().join(", "),
        ));
    }
    Ok(out)
}

/// The reviewer launch suffix: the policy section while a list is in force,
/// nothing under the all-block default (the launch then stays the ordinary
/// prompt, and the review worksheet still quotes the default).
// trace:STORY-2913 | ai:claude
pub(crate) fn launch_prompt(root: &Path) -> Result<String> {
    Ok(if policy::enabled(root)? {
        prompt(root)?
    } else {
        String::new()
    })
}

/// Audit keys written next to the list by `set`.
// trace:STORY-2913 | ai:claude
pub(crate) const SET_BY_KEY: &str = "blocking_tiers_set_by";
pub(crate) const SET_AT_KEY: &str = "blocking_tiers_set_at";
pub(crate) const AUTHORITY_KEY: &str = "blocking_tiers_authority";

/// The working tree's audit stamp, when present: `(by, at, authority)`.
// trace:STORY-2913 | ai:claude
pub(crate) fn audit_stamp(text: &str) -> Option<(String, String, String)> {
    let config: toml::Value = toml::from_str(text).ok()?;
    let review = config.get("review")?;
    let get = |k: &str| review.get(k).and_then(|v| v.as_str()).map(str::to_string);
    Some((get(SET_BY_KEY)?, get(SET_AT_KEY)?, get(AUTHORITY_KEY)?))
}

/// Write `classes` (strictly validated) with its audit stamp to
/// `<root>/.aida/config.toml`. The caller has already established
/// `authority`; nothing is written on any refusal.
// trace:STORY-2913 | ai:claude
pub(crate) fn write_list(
    root: &Path,
    classes: &[String],
    authority: &policy::ChangeAuthority,
    actor: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<std::collections::BTreeSet<&'static str>> {
    let set = policy::parse_names(classes)?;
    let mut array = toml_edit::Array::new();
    for class in &set {
        array.push(*class);
    }
    let path = crate::mass_change::config_path(root);
    crate::config_edit::set_kvs(
        &path,
        "review",
        &[
            ("blocking_tiers", toml_edit::Value::Array(array)),
            (SET_BY_KEY, toml_edit::Value::from(actor)),
            (
                SET_AT_KEY,
                toml_edit::Value::from(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            ),
            (AUTHORITY_KEY, toml_edit::Value::from(authority.label())),
        ],
    )?;
    Ok(set)
}

/// `aida review blocking-tiers set <CLASS>...`.
// trace:STORY-2913 | ai:claude
pub(crate) fn handle_set(classes: &[String]) -> Result<()> {
    let root = crate::mass_change::mode_root()?;
    let authority = policy::resolve_change_authority(&root)?;
    let actor = crate::current_user_id(None);
    let now = chrono::Utc::now();
    let set = write_list(&root, classes, &authority, &actor, now)?;
    println!(
        "Blocking classes set to {} by {actor} ({}) in {}.",
        set.iter().copied().collect::<Vec<_>>().join(", "),
        authority.label(),
        crate::mass_change::config_path(&root).display()
    );
    println!(
        "Reviews keep using {} until this edit lands on the default branch. \
         The file is gate-protected: commit it and land it through a human merge.",
        describe(&policy::effective(&root))
    );
    Ok(())
}

/// `aida review blocking-tiers show [--json]`.
// trace:STORY-2913 | ai:claude
pub(crate) fn handle_show(json: bool) -> Result<()> {
    let root = crate::mass_change::mode_root()?;
    let effective = policy::effective(&root);
    let stamp = crate::trusted_config::read_trusted_config_toml_local(&root)
        .as_deref()
        .and_then(audit_stamp);
    if json {
        let (classes, source) = show_fields(&effective);
        let mut v = serde_json::json!({
            "classes": classes,
            "source": source,
            "registry": policy::CLASSES.iter().map(|(n, d)| serde_json::json!({"class": n, "meaning": d})).collect::<Vec<_>>(),
        });
        if let Source::Invalid(why) = &effective.source {
            v["invalid"] = why.clone().into();
        }
        if let Some((by, at, authority)) = stamp {
            v["set_by"] = by.into();
            v["set_at"] = at.into();
            v["authority"] = authority.into();
        }
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    println!("Blocking classes: {}", describe(&effective));
    if let Some((by, at, authority)) = stamp {
        println!("Last set by {by} at {at} ({authority}).");
    }
    println!("Registry:");
    for (name, meaning) in policy::CLASSES {
        println!("  {name:<26} {meaning}");
    }
    Ok(())
}

/// `(classes, source)` for JSON: `classes` is `null` under the all-block default.
fn show_fields(effective: &Effective) -> (serde_json::Value, &'static str) {
    let classes = match &effective.policy {
        Policy::AllBlock => serde_json::Value::Null,
        Policy::Tiers(set) => set.iter().copied().collect::<Vec<_>>().into(),
    };
    let source = match effective.source {
        Source::Unset => "default",
        Source::DefaultBranch => "default-branch",
        Source::Unavailable => "unavailable",
        Source::Invalid(_) => "invalid",
    };
    (classes, source)
}

/// Test support: publish the working tree's `.aida/config.toml` as the
/// default branch's copy (`origin/main`), the only one the policy reads.
/// HEAD, the index and the working tree are left untouched.
// trace:STORY-2913 | ai:claude
#[cfg(test)]
pub(crate) fn commit_config_for_test(root: &Path) {
    let index = root.join(".git").join("aida-test-policy-index");
    let git = |args: &[&str]| -> (bool, String) {
        let out = crate::test_git::git_command()
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_INDEX_FILE", &index)
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
    };
    let must = |args: &[&str]| {
        let (ok, out) = git(args);
        assert!(ok, "git {args:?}");
        out
    };
    if !root.join(".git").exists() {
        must(&["init", "-q", "-b", "main"]);
    }
    let _ = std::fs::remove_file(&index);
    let (has_head, head) = git(&["rev-parse", "--verify", "-q", "HEAD^{commit}"]);
    if has_head {
        must(&["read-tree", &head]);
    } else {
        must(&["read-tree", "--empty"]);
    }
    let blob = must(&["hash-object", "-w", ".aida/config.toml"]);
    must(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("100644,{blob},.aida/config.toml"),
    ]);
    let tree = must(&["write-tree"]);
    let commit = if has_head {
        must(&["commit-tree", &tree, "-p", &head, "-m", "review policy"])
    } else {
        must(&["commit-tree", &tree, "-m", "review policy"])
    };
    must(&["update-ref", "refs/remotes/origin/main", &commit]);
    let _ = std::fs::remove_file(&index);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::review_verdict as rv;

    fn set_policy(root: &Path, setting: &str) {
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            format!("[review]\nblocking_tiers = {setting}\n"),
        )
        .unwrap();
        commit_config_for_test(root);
    }

    fn enable(root: &Path) {
        set_policy(
            root,
            "[\"p1\", \"data-loss\", \"security\", \"acceptance\"]",
        );
    }

    fn finding(tier: &str) -> String {
        serde_json::json!({"text":"a concrete defect", "severity":"minor", "blocking_tier":tier, "disposition":"blocking"}).to_string()
    }

    // trace:STORY-2913 | ai:claude — the default quotes itself in the prompt.
    #[test]
    fn task_3814_absent_preserves_p2_refusal_and_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let findings = vec![finding("p2")];
        assert_eq!(apply(tmp.path(), &findings).unwrap(), findings);
        assert_eq!(
            rv::incoming_disposition("request-changes", &findings),
            rv::VerdictKind::RequestChanges
        );
        let text = prompt(tmp.path()).unwrap();
        assert!(text.contains("every finding blocks"), "{text}");
        assert!(!text.contains("ADR-76"), "{text}");
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
        let text = prompt(tmp.path()).unwrap();
        assert!(text.contains("ADR-76"), "{text}");
        // The prompt quotes the active list, canonical names in order.
        assert!(
            text.contains(
                "Active blocking classes: data-loss, p1, security-authority-bypass, unmet-claimed-ac"
            ),
            "{text}"
        );
        assert!(
            crate::review_launch::review_prompt(tmp.path(), 42, false, "review")
                .unwrap()
                .unwrap()
                .contains("Project review blocking policy")
        );
    }

    // trace:STORY-2913 | ai:claude — advisor invariant: an unknown or
    // unparseable list fails closed to the default (all findings block),
    // never to fewer blockers, and says so in the prompt.
    #[test]
    fn task_3814_invalid_policy_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        for setting in [
            "[]",
            "true",
            "\"p1\"",
            "[1]",
            "[\"p1\", \"data-loss\", \"typo\"]",
            "[\"p1\", \"\"]",
            "[\"p1\"\n[broken",
        ] {
            set_policy(tmp.path(), setting);
            let effective = crate::gate::kernel_deps::review_blocking::effective(tmp.path());
            assert_eq!(effective.policy, Policy::AllBlock, "{setting}");
            assert!(
                matches!(effective.source, Source::Invalid(_)),
                "{setting}: {effective:?}"
            );
            let findings = vec![finding("p2"), finding("style")];
            assert_eq!(apply(tmp.path(), &findings).unwrap(), findings, "{setting}");
            assert_eq!(
                rv::incoming_disposition("approved", &findings),
                rv::VerdictKind::RequestChanges,
                "{setting}"
            );
            assert!(
                !crate::gate::kernel_deps::review_blocking::enabled(tmp.path()).unwrap(),
                "{setting}"
            );
            assert!(prompt(tmp.path()).unwrap().contains("invalid"), "{setting}");
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
                "file": "aida-cli-lib/src/gate/review_verdict.rs",
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

    // trace:TASK-3814 | ai:claude — the same low-priority class blocks inside
    // gate/sandbox code and defers outside it (paired regression).
    #[test]
    fn task_3814_adr76_exception_is_scoped_to_sensitive_code() {
        let tmp = tempfile::tempdir().unwrap();
        enable(tmp.path());
        let at = |class: &str, file: Option<&str>| {
            let mut v = serde_json::json!({
                "text": format!("{class} observation"),
                "blocking_tier": "p2",
                "class": class,
            });
            if let Some(f) = file {
                v["file"] = f.into();
            }
            v.to_string()
        };
        for class in ["untested-path", "missing-test", "mutation-kill"] {
            for sensitive in [
                "aida-cli-lib/src/gate/kernel_deps.rs",
                "aida-cli-lib/src/findings_gate.rs",
                "aida-core/src/sandbox/policy.rs",
                "aida-cli-lib/src/seat_authority.rs",
            ] {
                let findings = apply(tmp.path(), &[at(class, Some(sensitive))]).unwrap();
                assert_eq!(
                    rv::incoming_disposition("approved", &findings),
                    rv::VerdictKind::RequestChanges,
                    "{class} in {sensitive}"
                );
            }
            for ordinary in [
                None,
                Some("aida-cli-lib/src/report.rs"),
                Some("docs/cli/06-roles-sessions.md"),
                Some("docs/internal/testing/gate-dependency-ratchet.md"),
            ] {
                let findings = apply(tmp.path(), &[at(class, ordinary)]).unwrap();
                assert_eq!(
                    rv::incoming_disposition("approved", &findings),
                    rv::VerdictKind::Approved,
                    "{class} in {ordinary:?}"
                );
            }
        }
        // Security and authority bypass stay unconditional.
        for class in ["security", "authority-bypass"] {
            let findings =
                apply(tmp.path(), &[at(class, Some("docs/getting-started.md"))]).unwrap();
            assert_eq!(
                rv::incoming_disposition("approved", &findings),
                rv::VerdictKind::RequestChanges,
                "{class}"
            );
        }
    }

    // trace:TASK-3814 | ai:claude — positional --finding-class values reach
    // the policy before dispositions, in canonical and alias spellings.
    #[test]
    fn task_3814_positional_classes_join_policy_input() {
        use crate::gate::kernel_deps::review_blocking::apply_with_classes;
        let tmp = tempfile::tempdir().unwrap();
        enable(tmp.path());
        let gate = serde_json::json!({
            "text": "gate refusal path untested",
            "blocking_tier": "p2",
            "file": "aida-cli-lib/src/gate/ledger/mod.rs",
        })
        .to_string();
        let plain =
            serde_json::json!({"text": "rename a helper", "blocking_tier": "p2"}).to_string();
        for class in ["untested-path", "missing-test", "Untested", "mutation-kill"] {
            let findings =
                apply_with_classes(tmp.path(), &[gate.clone(), plain.clone()], &[class.into()])
                    .unwrap();
            assert_eq!(
                rv::incoming_disposition("approved", &findings),
                rv::VerdictKind::RequestChanges,
                "{class}"
            );
            // The merged class is retained in the recorded metadata.
            let merged: serde_json::Value = serde_json::from_str(&findings[0]).unwrap();
            assert_eq!(merged["disposition"], "blocking");
            assert!(merged["class"].as_str().is_some());
        }
        for class in ["security", "authority-bypass"] {
            let findings =
                apply_with_classes(tmp.path(), &[plain.clone()], &[class.into()]).unwrap();
            assert_eq!(
                rv::incoming_disposition("approved", &findings),
                rv::VerdictKind::RequestChanges,
                "{class}"
            );
        }
        // An ordinary positional class never displaces a JSON exception class.
        let secure = serde_json::json!({
            "text": "token leak", "blocking_tier": "p2", "class": "security",
        })
        .to_string();
        let findings = apply_with_classes(tmp.path(), &[secure], &["perf".into()]).unwrap();
        assert_eq!(
            rv::incoming_disposition("approved", &findings),
            rv::VerdictKind::RequestChanges
        );
        // trace:TASK-3814 | ai:claude — a positional security/authority-bypass
        // class outranks an embedded path-scoped exception on ordinary code.
        for embedded in ["untested-path", "mutation-kill"] {
            for positional in ["security", "authority-bypass"] {
                let raw = serde_json::json!({
                    "text": "security defect", "blocking_tier": "p2",
                    "class": embedded, "file": "aida-cli-lib/src/report.rs",
                })
                .to_string();
                let findings =
                    apply_with_classes(tmp.path(), &[raw], &[positional.into()]).unwrap();
                let merged: serde_json::Value = serde_json::from_str(&findings[0]).unwrap();
                assert_eq!(merged["class"], positional, "{embedded}+{positional}");
                assert_eq!(merged["disposition"], "blocking", "{embedded}+{positional}");
            }
        }
        // Positional classes alone, without a sensitive location, still defer.
        let findings =
            apply_with_classes(tmp.path(), &[plain.clone()], &["untested-path".into()]).unwrap();
        assert!(rv::incoming_disposition("approved", &findings).approves());
        // The builder's own re-application keeps the merged classification.
        let findings =
            apply_with_classes(tmp.path(), &[gate.clone()], &["untested-path".into()]).unwrap();
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
        assert_eq!(record["verdict"], "request-changes");
        assert_eq!(record["finding_dispositions"][0]["class"], "untested-path");
        assert_eq!(record["finding_dispositions"][0]["disposition"], "blocking");
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

    // ── STORY-2913 ────────────────────────────────────────────────────────

    fn classed(tier: &str, class: Option<&str>) -> String {
        let mut v = serde_json::json!({"text": "observation", "blocking_tier": tier});
        if let Some(c) = class {
            v["class"] = c.into();
        }
        v.to_string()
    }

    fn blocks_under(root: &Path, raw: String) -> bool {
        let findings = apply(root, &[raw]).unwrap();
        !rv::incoming_disposition("approved", &findings).approves()
    }

    // trace:STORY-2913 | ai:claude — AC1/AC6: a custom list blocks only its
    // classes; the floors and unclassified findings still block.
    #[test]
    fn story_2913_custom_list_blocks_only_its_classes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        set_policy(root, "[\"p2\", \"Style\"]");
        assert!(blocks_under(root, classed("p2", None)));
        assert!(!blocks_under(root, classed("p3", None)));
        assert!(!blocks_under(root, classed("p1", None)), "p1 is not listed");
        assert!(!blocks_under(root, classed("data-loss", None)));
        assert!(
            blocks_under(root, classed("p3", Some("style"))),
            "class listed"
        );
        assert!(!blocks_under(root, classed("p3", Some("perf"))));
        assert!(blocks_under(root, classed("style", None)), "tier listed");
        // Floors: security/authority bypass anywhere; ADR-76 test gaps in
        // gate code; an unregistered or missing tier.
        assert!(blocks_under(root, classed("p3", Some("authority-bypass"))));
        assert!(blocks_under(
            root,
            classed("security-authority-bypass", None)
        ));
        assert!(blocks_under(root, classed("mutation-kill", None)));
        let gate = serde_json::json!({"text": "gate path untested", "blocking_tier": "p3",
            "class": "test-gap", "file": "aida-cli-lib/src/gate/ship.rs"});
        assert!(blocks_under(root, gate.to_string()));
        assert!(!blocks_under(root, classed("p3", Some("test-gap"))));
        assert!(blocks_under(root, classed("p9", None)));
        assert!(blocks_under(
            root,
            serde_json::json!({"text": "no tier"}).to_string()
        ));
        // A reviewer-supplied deferral of a listed class is overridden.
        let forged = serde_json::json!({"text": "x", "blocking_tier": "p2",
            "disposition": "deferred", "rationale": "trust me"});
        assert!(blocks_under(root, forged.to_string()));
    }

    // trace:STORY-2913 | ai:claude — neither class channel demotes the other.
    #[test]
    fn story_2913_positional_class_cannot_demote_a_listed_class() {
        use crate::gate::kernel_deps::review_blocking::apply_with_classes;
        let tmp = tempfile::tempdir().unwrap();
        set_policy(tmp.path(), "[\"data-loss\"]");
        let raw = classed("p3", Some("data-loss"));
        for positional in ["style", "perf", "p3"] {
            let findings =
                apply_with_classes(tmp.path(), &[raw.clone()], &[positional.into()]).unwrap();
            assert!(
                !rv::incoming_disposition("approved", &findings).approves(),
                "{positional}"
            );
        }
        // And a positional listed class promotes an ordinary embedded one.
        let findings = apply_with_classes(
            tmp.path(),
            &[classed("p3", Some("perf"))],
            &["data-loss".into()],
        )
        .unwrap();
        assert!(!rv::incoming_disposition("approved", &findings).approves());
    }

    // trace:STORY-2913 | ai:claude — AC4: the list is read from the default
    // branch only; a working-tree edit (a branch under review) changes nothing,
    // and no default branch at all is the all-block default.
    #[test]
    fn story_2913_branch_local_edit_cannot_relax_the_list() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(
            crate::gate::kernel_deps::review_blocking::effective(root),
            crate::gate::kernel_deps::review_blocking::Effective {
                policy: Policy::AllBlock,
                source: Source::Unavailable,
            }
        );
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[review]\nblocking_tiers = [\"p1\"]\n",
        )
        .unwrap();
        assert!(
            blocks_under(root, classed("p2", None)),
            "uncommitted, no branch"
        );
        set_policy(root, "[\"p1\", \"p2\"]");
        std::fs::write(
            root.join(".aida/config.toml"),
            "[review]\nblocking_tiers = [\"p1\"]\n",
        )
        .unwrap();
        assert!(
            blocks_under(root, classed("p2", None)),
            "working tree ignored"
        );
        let effective = crate::gate::kernel_deps::review_blocking::effective(root);
        assert_eq!(effective.source, Source::DefaultBranch);
        assert!(describe(&effective).starts_with("p1, p2 ("));
    }

    // trace:STORY-2913 | ai:claude — AC1: the documented registry parses,
    // aliases fold, and anything unknown refuses.
    #[test]
    fn story_2913_registry_parses_strictly() {
        use crate::gate::kernel_deps::review_blocking::{canonical, parse_names, CLASSES};
        for required in [
            "p1",
            "p2",
            "p3",
            "data-loss",
            "security-authority-bypass",
            "unmet-claimed-ac",
            "test-gap",
            "style",
        ] {
            assert!(CLASSES.iter().any(|(n, _)| *n == required), "{required}");
            assert_eq!(canonical(required), Some(required));
        }
        assert_eq!(canonical(" Security "), Some("security-authority-bypass"));
        assert_eq!(canonical("acceptance"), Some("unmet-claimed-ac"));
        assert_eq!(canonical("Mutation_Kill"), Some("test-gap"));
        assert_eq!(canonical("perf"), None);
        assert!(parse_names(&["p1".into(), "bogus".into()]).is_err());
        assert!(parse_names(&[]).is_err());
        assert_eq!(
            parse_names(&["security".into(), "security-authority-bypass".into()])
                .unwrap()
                .len(),
            1
        );
    }

    fn mandate(scopes: &[&str], principal: &str) -> crate::gate::mandate::Mandate {
        let now = chrono::Utc::now();
        serde_json::from_value(serde_json::json!({
            "id": "m-1",
            "principal": principal,
            "seat": "advisor",
            "scope": "ship-held",
            "issued_at": now - chrono::Duration::hours(1),
            "expires_at": now + chrono::Duration::hours(1),
            "scopes": scopes,
        }))
        .unwrap()
    }

    // trace:STORY-2913 | ai:claude — AC4/AC6: only the operator, or the
    // advisor seat under a mandate covering review-policy, may change it.
    #[test]
    fn story_2913_unauthorized_change_is_refused() {
        use crate::gate::kernel_deps::review_blocking::{decide_change_authority, ChangeAuthority};
        use crate::gate::mandate::MandateState;
        let now = chrono::Utc::now();
        assert_eq!(
            decide_change_authority(true, None, &MandateState::None, now).unwrap(),
            ChangeAuthority::Operator
        );
        let covered = mandate(&["review-policy"], "joe");
        let advisor = Some(("advisor", "joe"));
        assert_eq!(
            decide_change_authority(false, advisor, &MandateState::Active(covered.clone()), now)
                .unwrap(),
            ChangeAuthority::Mandate {
                id: "m-1".into(),
                principal: "joe".into()
            }
        );
        let refusals: Vec<(Option<(&str, &str)>, MandateState)> = vec![
            (advisor, MandateState::None),
            (advisor, MandateState::Active(mandate(&["dispatch"], "joe"))),
            (advisor, MandateState::Active(mandate(&[], "joe"))),
            (advisor, MandateState::Expired(covered.clone())),
            (advisor, MandateState::Stopped(covered.clone())),
            (
                Some(("implementer", "joe")),
                MandateState::Active(covered.clone()),
            ),
            (
                Some(("advisor", "eve")),
                MandateState::Active(covered.clone()),
            ),
            (None, MandateState::Active(covered.clone())),
        ];
        for (grant, state) in refusals {
            let err = decide_change_authority(false, grant, &state, now).unwrap_err();
            assert!(
                format!("{err:#}").contains("operator or mandate authority"),
                "{grant:?} {state:?}"
            );
        }
        // A covering record past its expiry refuses even if reported active.
        let mut stale = covered;
        stale.expires_at = now - chrono::Duration::minutes(1);
        assert!(
            decide_change_authority(false, advisor, &MandateState::Active(stale), now).is_err()
        );
        // A drive mandate never implies review-policy.
        assert!(crate::gate::mandate::Scope::ReviewPolicy.explicit_only());
    }

    // trace:STORY-2913 | ai:claude — AC4: the write is validated and stamped
    // with who, when and under what authority; a refused list writes nothing.
    #[test]
    fn story_2913_set_writes_an_audited_canonical_list() {
        use crate::gate::kernel_deps::review_blocking::ChangeAuthority;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(".aida/config.toml"), "[review]\nmax_rounds = 4\n").unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-18T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let auth = ChangeAuthority::Mandate {
            id: "m-7".into(),
            principal: "joe".into(),
        };
        let before = std::fs::read_to_string(root.join(".aida/config.toml")).unwrap();
        assert!(write_list(root, &["p1".into(), "nope".into()], &auth, "joe", now).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join(".aida/config.toml")).unwrap(),
            before
        );
        write_list(root, &["Security".into(), "p1".into()], &auth, "joe", now).unwrap();
        let text = std::fs::read_to_string(root.join(".aida/config.toml")).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(v["review"]["max_rounds"].as_integer(), Some(4));
        assert_eq!(
            v["review"]["blocking_tiers"].as_array().unwrap(),
            &vec![
                toml::Value::from("p1"),
                toml::Value::from("security-authority-bypass")
            ]
        );
        assert_eq!(
            audit_stamp(&text),
            Some((
                "joe".into(),
                "2026-11-18T12:00:00Z".into(),
                "mandate m-7".into()
            ))
        );
        // Not effective until it lands on the default branch.
        assert_eq!(
            crate::gate::kernel_deps::review_blocking::effective(root).policy,
            Policy::AllBlock
        );
        commit_config_for_test(root);
        assert!(crate::gate::kernel_deps::review_blocking::enabled(root).unwrap());
    }
}
