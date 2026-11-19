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
        Source::NoDefaultBranch => "no default branch to read".to_string(),
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
    if effective.fails_closed() {
        out.push_str(
            "\nThe list cannot be honoured, so every finding blocks, including a finding \
you mark deferred: record request-changes whenever any finding remains.\n",
        );
    }
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

/// The reviewer launch suffix: the same policy section, so every launched
/// reviewer sees the active list and its provenance, including the all-block
/// default and an invalid or unreadable list.
// trace:STORY-2913 | ai:claude
pub(crate) fn launch_prompt(root: &Path) -> Result<String> {
    prompt(root)
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
        Source::NoDefaultBranch => "no-default-branch",
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
pub(crate) mod tests {
    use super::*;
    use crate::gate::review_verdict as rv;

    pub(crate) fn set_policy(root: &Path, setting: &str) {
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
    // trace:STORY-2913.acefe1a7 | ai:claude
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

    // trace:STORY-2913.acb249c8 | ai:claude
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
                .contains("Project review blocking policy")
        );
    }

    // trace:STORY-2913 | ai:claude — advisor invariant: an unknown or
    // unparseable list fails closed to the default (all findings block),
    // never to fewer blockers, and says so in the prompt.
    // trace:STORY-2913.acd80026 | ai:claude
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
            let findings = apply(tmp.path(), &[finding("p2"), finding("style")]).unwrap();
            for raw in &findings {
                let v: serde_json::Value = serde_json::from_str(raw).unwrap();
                assert_eq!(v["disposition"], "blocking", "{setting}");
            }
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
    // trace:STORY-2913.acefe1a7 | ai:claude
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
    // trace:STORY-2913.ac05bb6e | ai:claude
    #[test]
    fn story_2913_branch_local_edit_cannot_relax_the_list() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(
            crate::gate::kernel_deps::review_blocking::effective(root),
            crate::gate::kernel_deps::review_blocking::Effective {
                policy: Policy::AllBlock,
                source: Source::NoDefaultBranch,
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
    // trace:STORY-2913.acd80026 | ai:claude
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
    // trace:STORY-2913.ac05bb6e | ai:claude
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
    // trace:STORY-2913.ac05bb6e | ai:claude
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

    // trace:STORY-2913 | ai:claude — review round 2, finding 1: merging an
    // unlisted test gap on ordinary code with a listed class keeps the listed
    // blocker, whichever channel carries which class.
    #[test]
    fn story_2913_merged_test_gap_keeps_listed_blocker_in_both_orders() {
        use crate::gate::kernel_deps::review_blocking::apply_with_classes;
        let tmp = tempfile::tempdir().unwrap();
        set_policy(tmp.path(), "[\"data-loss\"]");
        let at = |class: &str| {
            serde_json::json!({"text": "data corruption", "blocking_tier": "p3",
                "class": class, "file": "src/report.rs"})
            .to_string()
        };
        for (embedded, positional) in [
            ("data-loss", "untested-path"),
            ("untested-path", "data-loss"),
        ] {
            let findings =
                apply_with_classes(tmp.path(), &[at(embedded)], &[positional.into()]).unwrap();
            let merged: serde_json::Value = serde_json::from_str(&findings[0]).unwrap();
            assert_eq!(merged["class"], "data-loss", "{embedded}+{positional}");
            assert_eq!(merged["disposition"], "blocking", "{embedded}+{positional}");
            assert_blocks_through_builder(tmp.path(), &findings);
        }
        // In sensitive code both classes block; the merge still blocks.
        let gate = serde_json::json!({"text": "gate corruption", "blocking_tier": "p3",
            "class": "data-loss", "file": "aida-cli-lib/src/gate/ship.rs"})
        .to_string();
        let findings = apply_with_classes(tmp.path(), &[gate], &["untested-path".into()]).unwrap();
        assert!(!rv::incoming_disposition("approved", &findings).approves());
        // Neither class blocking still defers.
        let findings = apply_with_classes(
            tmp.path(),
            &[classed("p3", Some("style"))],
            &["untested-path".into()],
        )
        .unwrap();
        assert!(rv::incoming_disposition("approved", &findings).approves());
    }

    fn deferred_findings() -> Vec<String> {
        [
            (
                "authority bypass remains",
                Some("security-authority-bypass"),
            ),
            ("wrong totals", Some("p1")),
            ("odd tier", Some("p9")),
            ("no tier", None),
        ]
        .into_iter()
        .map(|(text, tier)| {
            let mut v = serde_json::json!({"text": text, "severity": "major",
                "disposition": "deferred", "rationale": "later"});
            if let Some(t) = tier {
                v["blocking_tier"] = t.into();
            }
            v.to_string()
        })
        .collect()
    }

    fn record(
        root: &Path,
        verdict: &str,
        findings: &[String],
    ) -> std::io::Result<serde_json::Map<String, serde_json::Value>> {
        rv::build_verdict_object(
            root,
            &rv::verdict_path(root, "TASK-9821"),
            Some(verdict),
            None,
            None,
            None,
            findings,
            "reviewer",
        )
    }

    /// Through the canonical verdict builder: an approval is refused, and a
    /// request-changes record stamps every finding blocking.
    fn assert_blocks_through_builder(root: &Path, findings: &[String]) {
        let err = record(root, "approved", findings).unwrap_err();
        assert!(
            err.to_string().contains("require request-changes"),
            "{err} {findings:?}"
        );
        let record = record(root, "request-changes", findings).unwrap();
        assert_eq!(record["verdict"], "request-changes", "{findings:?}");
        assert!(!rv::disposition_verdict(&record).approves());
        for (i, _) in findings.iter().enumerate() {
            assert_eq!(
                record["finding_dispositions"][i]["disposition"], "blocking",
                "{findings:?}"
            );
        }
    }

    /// A default branch whose `.aida/config.toml` exists in the tree but
    /// cannot be read (a gitlink to an absent object).
    pub(crate) fn publish_unreadable_config(root: &Path) {
        let index = root.join("aida-test-unreadable-index");
        let git = |args: &[&str]| {
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
            assert!(out.status.success(), "git {args:?}: {out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["read-tree", "--empty"]);
        git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            "160000,1111111111111111111111111111111111111111,.aida/config.toml",
        ]);
        let tree = git(&["write-tree"]);
        let commit = git(&["commit-tree", &tree, "-m", "unreadable policy"]);
        git(&["update-ref", "refs/remotes/origin/main", &commit]);
        let _ = std::fs::remove_file(&index);
    }

    // trace:STORY-2913 | ai:claude — review round 2, finding 2: an invalid or
    // unreadable list fails closed through the canonical verdict builder,
    // overriding explicit reviewer deferrals of security, p1 and unknown-tier
    // findings; an unset list keeps the existing explicit-deferral contract.
    // trace:STORY-2913.acd80026 | ai:claude
    #[test]
    fn story_2913_failed_policy_overrides_explicit_deferrals() {
        let invalid = tempfile::tempdir().unwrap();
        set_policy(invalid.path(), "[\"p1\", \"typo\"]");
        let unreadable = tempfile::tempdir().unwrap();
        publish_unreadable_config(unreadable.path());
        assert_eq!(
            crate::gate::kernel_deps::review_blocking::effective(unreadable.path()).source,
            Source::Unavailable
        );
        for root in [invalid.path(), unreadable.path()] {
            let findings = deferred_findings();
            for one in &findings {
                assert_blocks_through_builder(root, std::slice::from_ref(one));
            }
            assert_blocks_through_builder(root, &findings);
            let text = prompt(root).unwrap();
            assert!(
                text.contains("including a finding you mark deferred"),
                "{text}"
            );
        }
        // Unset (key absent, or no config on the default branch): an
        // explicit, rationale-bearing deferral keeps its existing meaning.
        let unset = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(unset.path().join(".aida")).unwrap();
        std::fs::write(
            unset.path().join(".aida/config.toml"),
            "[review]\nmax_rounds = 3\n",
        )
        .unwrap();
        commit_config_for_test(unset.path());
        let effective = crate::gate::kernel_deps::review_blocking::effective(unset.path());
        assert_eq!(effective.source, Source::Unset);
        assert!(!effective.fails_closed());
        let plain = vec![serde_json::json!({"text": "small", "severity": "minor",
            "disposition": "deferred", "rationale": "later"})
        .to_string()];
        assert!(
            rv::disposition_verdict(&record(unset.path(), "approved", &plain).unwrap()).approves()
        );
        assert!(!prompt(unset.path()).unwrap().contains("mark deferred"));
    }

    // trace:STORY-2913 | ai:claude — review round 2, finding 3 (AC5): a
    // launched reviewer is told the active list and its provenance under the
    // unset default and an invalid list, not only while a list is in force.
    // trace:STORY-2913.acb249c8 | ai:claude
    #[test]
    fn story_2913_launch_prompt_quotes_default_and_invalid_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(".aida/config.toml"), "[review]\nmax_rounds = 3\n").unwrap();
        commit_config_for_test(root);
        let launched =
            crate::review_launch::review_prompt(root, 42, false, "/aida-review --pr 42").unwrap();
        assert!(launched.starts_with("/aida-review --pr 42\n## Project review blocking policy"));
        assert!(
            launched.contains(
                "Active blocking classes: default — every finding blocks (not set on the default branch)."
            ),
            "{launched}"
        );
        set_policy(root, "[\"p1\", \"typo\"]");
        let launched =
            crate::review_launch::review_prompt(root, 42, false, "/aida-review --pr 42").unwrap();
        assert!(
            launched.contains("Active blocking classes: default — every finding blocks (invalid on the default branch:"),
            "{launched}"
        );
        assert!(
            launched.contains("including a finding you mark deferred"),
            "{launched}"
        );
    }

    /// The three policy changes after a deferral was recorded that must make
    /// the inherited deferral block again: an invalid list, an unreadable
    /// default-branch config, and a list that now names the finding's class.
    fn reblocking_policies() -> [(&'static str, fn(&Path)); 3] {
        [
            ("invalid", |root| set_policy(root, "[\"p1\", \"typo\"]")),
            ("unavailable", publish_unreadable_config),
            ("newly-blocking", |root| {
                set_policy(root, "[\"p1\", \"p2\"]")
            }),
        ]
    }

    /// Seed `path` with an approval whose one p2 finding was deferred under
    /// `["p1"]`, as a legacy verdict file (an approval carrying deferred
    /// findings would otherwise need a canonical store for its follow-ups).
    fn seed_deferred_approval(root: &Path, path: &Path, head: Option<&str>) {
        set_policy(root, "[\"p1\"]");
        let mut first = record(root, "approved", &[p2_finding()]).unwrap();
        assert_eq!(first["finding_dispositions"][0]["disposition"], "deferred");
        if let Some(head) = head {
            first.insert("reviewed_sha".into(), head.into());
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::Value::Object(first).to_string()).unwrap();
        let same = crate::gate::kernel_deps::review_blocking::effective(root);
        assert!(crate::review_record_kind(&same, path, "approved", &[])
            .unwrap()
            .approves());
    }

    pub(crate) fn p2_finding() -> String {
        serde_json::json!({"text": "contained defect", "severity": "minor",
            "blocking_tier": "p2"})
        .to_string()
    }

    // trace:STORY-2913 | ai:claude — review round 3, finding 1: a record with
    // no new findings inherits the stored ones, and the current policy decides
    // them again. A p2 finding deferred under `["p1"]` blocks once the list is
    // invalid, unreadable or names p2: both the recorder's kind/authority
    // routing and the canonical builder refuse the approval, and the retained
    // metadata is restamped `blocking`.
    // trace:STORY-2913.acd80026 | ai:claude
    #[test]
    fn story_2913_inherited_deferrals_are_redecided_by_the_current_policy() {
        for (label, change) in reblocking_policies() {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            let path = rv::verdict_path(root, "TASK-9821");
            seed_deferred_approval(root, &path, None);
            // The unchanged policy keeps the inherited deferral approvable.
            assert!(rv::disposition_verdict(&record(root, "approved", &[]).unwrap()).approves());

            change(root);
            let now = crate::gate::kernel_deps::review_blocking::effective(root);
            assert_eq!(
                crate::review_record_kind(&now, &path, "approved", &[]).unwrap(),
                rv::VerdictKind::RequestChanges,
                "{label}: routing must see the re-decided finding"
            );
            let err = record(root, "approved", &[]).unwrap_err();
            assert!(
                err.to_string().contains("require request-changes"),
                "{label}: {err}"
            );
            let held = record(root, "request-changes", &[]).unwrap();
            assert_eq!(held["verdict"], "request-changes", "{label}");
            assert_eq!(held["findings"][0], "contained defect", "{label}");
            assert_eq!(
                held["finding_dispositions"][0]["disposition"], "blocking",
                "{label}"
            );
            // A provenance-only stamp (no verdict word) re-decides too.
            let stamped =
                rv::build_verdict_object(root, &path, None, None, None, None, &[], "drain")
                    .unwrap();
            assert!(!rv::disposition_verdict(&stamped).approves(), "{label}");
        }
    }

    // trace:STORY-2913 | ai:claude — the stored metadata is decided as a
    // structured finding only when it is one: a malformed shape is kept as
    // stored (and already refuses approval), and a stored text that merely
    // looks like JSON is never re-parsed into a fresh deferral.
    #[test]
    fn story_2913_redecision_never_promotes_a_malformed_or_text_finding() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        set_policy(root, "[\"p1\"]");
        let effective = crate::gate::kernel_deps::review_blocking::effective(root);
        let smuggled = serde_json::json!({"text": "x", "severity": "minor",
            "disposition": "deferred", "rationale": "r", "blocking_tier": "p3"})
        .to_string();
        for stored in [
            serde_json::json!({"verdict": "approved", "findings": [smuggled],
                "finding_dispositions": [null]}),
            serde_json::json!({"verdict": "approved", "findings": ["a", "b"],
                "finding_dispositions": [{"severity": "minor", "blocking_tier": "p3"}]}),
            serde_json::json!({"verdict": "approved", "findings": "a",
                "finding_dispositions": []}),
        ] {
            let mut obj = stored.as_object().unwrap().clone();
            rv::reapply_policy_to_stored_findings(&mut obj, &effective);
            assert_eq!(serde_json::Value::Object(obj.clone()), stored);
            assert!(!rv::disposition_verdict(&obj).approves(), "{stored}");
        }
    }

    /// `aida review record <spec> --verdict <verdict>` from an independent
    /// reviewer, every ambient root pinned to `root`, with no `--finding`.
    fn cli_record(root: &Path, head: &str, verdict: &str, findings: &[String]) -> Result<()> {
        let root_str = root.to_str().unwrap();
        let _pinned = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_PROJECT_ROOT", Some(root_str)),
            ("AIDA_DRIVE_ROOT", Some(root_str)),
            ("AIDA_REVIEW_VERDICT_FILE", None),
            ("AIDA_FROM_PR_HEAD_SHA", None),
            (crate::gate::seat_authority::GRANT_ENV, None),
        ]);
        let subject = crate::ReviewPrSubject {
            head_ref: "task-29130-work".into(),
            commit_subjects: vec![],
        };
        let reviewer = crate::gate::review_authority::IdentitySource {
            session_ids: vec!["independent-reviewer".into()],
            ..Default::default()
        };
        crate::with_review_pr_subject(subject, || {
            crate::gate::review_authority::with_identity_source(reviewer, || {
                crate::handle_review_record_fixture_at(
                    root.to_path_buf(),
                    "TASK-29130",
                    verdict,
                    Some(head),
                    Some("task-29130-work"),
                    Some("inherited policy regression"),
                    findings,
                    &[],
                    None,
                )
            })
        })
    }

    // trace:STORY-2913 | ai:claude — review round 3, finding 1, through the
    // supported CLI writer: `aida review record --verdict approved` with no
    // `--finding` over a deferral the current policy no longer allows is
    // refused before anything is written.
    // trace:STORY-2913.acd80026 | ai:claude
    #[test]
    fn story_2913_cli_record_refuses_approval_over_a_redecided_inherited_deferral() {
        for (label, change) in reblocking_policies() {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            std::fs::create_dir_all(root.join(".aida")).unwrap();
            let mut req = aida_core::Requirement::new("inherited".into(), String::new());
            req.spec_id = Some("TASK-29130".into());
            req.req_type = aida_core::RequirementType::Task;
            let mut store = aida_core::RequirementsStore::new();
            store.requirements = vec![req];
            crate::Storage::new(root.join("requirements.yaml"))
                .save(&store)
                .unwrap();
            let head = crate::test_git::commit_fixture(root);
            seed_deferred_approval(root, &rv::verdict_path(root, "TASK-29130"), Some(&head));
            let stored = rv::read_recorded_verdict(root, "TASK-29130").expect("recorded");
            assert!(stored.kind.approves(), "{label}");
            let before =
                rv::current_verdict_body_at(&rv::verdict_path(root, "TASK-29130")).unwrap();

            change(root);
            let err = cli_record(root, &head, "approved", &[]).unwrap_err();
            assert!(
                format!("{err:#}").contains("require request-changes"),
                "{label}: {err:#}"
            );
            // The refusal happens before any write: the stored deferral is
            // left exactly as it was (the builder test covers restamping).
            let path = rv::verdict_path(root, "TASK-29130");
            assert_eq!(
                rv::current_verdict_body_at(&path).unwrap(),
                before,
                "{label}"
            );
        }
    }

    // trace:STORY-2913 | ai:claude — `aida review record` reads the policy
    // once and hands that snapshot to the recorder, which decides with it
    // rather than reading the default branch again. With the on-disk list
    // still `["p1"]` (the inherited p2 deferral would approve), a fail-closed
    // snapshot supplied by the caller refuses the approval and writes nothing.
    #[test]
    fn story_2913_recorder_decides_with_the_callers_policy_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        let mut req = aida_core::Requirement::new("inherited".into(), String::new());
        req.spec_id = Some("TASK-29130".into());
        req.req_type = aida_core::RequirementType::Task;
        let mut store = aida_core::RequirementsStore::new();
        store.requirements = vec![req];
        crate::Storage::new(root.join("requirements.yaml"))
            .save(&store)
            .unwrap();
        let head = crate::test_git::commit_fixture(root);
        let path = rv::verdict_path(root, "TASK-29130");
        seed_deferred_approval(root, &path, Some(&head));
        let before = rv::current_verdict_body_at(&path).unwrap();
        let snapshot = Effective {
            policy: Policy::AllBlock,
            source: Source::Unavailable,
        };
        let root_str = root.to_str().unwrap();
        let _pinned = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_PROJECT_ROOT", Some(root_str)),
            ("AIDA_DRIVE_ROOT", Some(root_str)),
            ("AIDA_REVIEW_VERDICT_FILE", None),
            ("AIDA_FROM_PR_HEAD_SHA", None),
            (crate::gate::seat_authority::GRANT_ENV, None),
        ]);
        let subject = crate::ReviewPrSubject {
            head_ref: "task-29130-work".into(),
            commit_subjects: vec![],
        };
        let reviewer = crate::gate::review_authority::IdentitySource {
            session_ids: vec!["independent-reviewer".into()],
            ..Default::default()
        };
        let err = crate::with_review_pr_subject(subject, || {
            crate::gate::review_authority::with_identity_source(reviewer, || {
                crate::handle_review_record_context_with_policy_at(
                    root.to_path_buf(),
                    "TASK-29130",
                    "approved",
                    Some(&head),
                    Some("task-29130-work"),
                    Some("caller snapshot regression"),
                    &[],
                    &[],
                    None,
                    false,
                    |root, _| {
                        crate::resolve_commit_sha(root, "HEAD")
                            .ok_or_else(|| anyhow::anyhow!("fixture has no HEAD"))
                    },
                    None,
                    Some(&snapshot),
                )
            })
        })
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("require request-changes"),
            "{err:#}"
        );
        assert_eq!(rv::current_verdict_body_at(&path).unwrap(), before);
    }
}
