// trace:STORY-3120 | ai:claude
//! Model and reasoning effort for review runners. Every precedence source is
//! checked against a fixed allowlist before any argv exists, and the project's
//! `.aida/review-rules.toml` defaults come from the trusted base revision.
// Wired into `--run-review` with the runner slice; only the tests call it yet.
#![cfg_attr(not(test), allow(dead_code))]

use crate::vendors::review_runner::ReviewVendor;
use aida_core::agents_config::{resolve_agent_tuning, AgentSeat};
use aida_core::review_engine::ProjectReviewRulesConfig;
use anyhow::{bail, ensure, Context, Result};
use std::ffi::OsString;
use std::path::Path;

/// Effort levels for a custom runner command, which names no vendor.
const BASE_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh"];
const RULES_PATH: &str = ".aida/review-rules.toml";
const MODEL_MAX: usize = 128;

/// A validated model and effort; only `resolve` builds a non-empty one.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ReviewTuning {
    model: Option<String>,
    effort: Option<&'static str>,
}

impl ReviewTuning {
    pub(crate) fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
    pub(crate) fn effort(&self) -> Option<&'static str> {
        self.effort
    }
}

/// `(source name, raw value)` pairs, highest precedence first.
pub(crate) type Sources<'a> = [(&'a str, Option<&'a str>)];

fn present(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|v| !v.is_empty())
}

/// Validate every source, then take the first one set. A bad value refuses
/// the run even when a higher-precedence source overrides it.
pub(crate) fn resolve(
    agent: Option<&ReviewVendor>,
    effort: &Sources<'_>,
    model: &Sources<'_>,
) -> Result<ReviewTuning> {
    let allowed = agent.map_or(BASE_EFFORTS, |a| a.efforts);
    let mut tuning = ReviewTuning::default();
    for (source, raw) in effort {
        let Some(raw) = present(*raw) else { continue };
        let Some(level) = allowed.iter().copied().find(|e| *e == raw) else {
            bail!(
                "{source}: effort {raw:?} is not one of {}",
                allowed.join("|")
            );
        };
        tuning.effort = tuning.effort.or(Some(level));
    }
    for (source, raw) in model {
        let Some(raw) = present(*raw) else { continue };
        let charset = |c: char| c.is_ascii_alphanumeric() || "._:/-".contains(c);
        ensure!(
            raw.len() <= MODEL_MAX && !raw.starts_with('-') && raw.chars().all(charset),
            "{source}: model {raw:?} must be up to {MODEL_MAX} of [A-Za-z0-9._:/-], not starting with '-'"
        );
        tuning.model = tuning.model.or_else(|| Some(raw.to_owned()));
    }
    Ok(tuning)
}

/// Resolve in the order flag > environment > review-rules.toml > the
/// `[agents]` reviewer seat. `rules` must come from `trusted_review_rules`.
pub(crate) fn resolve_for_run(
    agent: Option<&ReviewVendor>,
    project_root: &Path,
    parent_env: &[(OsString, OsString)],
    flags: (Option<&str>, Option<&str>),
    rules: &ProjectReviewRulesConfig,
) -> Result<ReviewTuning> {
    // A non-UTF-8 value is kept as an invalid one, so it refuses, never unsets.
    let env = |key: &str| {
        let found = parent_env.iter().find(|(k, _)| k == key);
        found.map(|(_, v)| v.to_str().unwrap_or("\u{fffd}"))
    };
    let seat = agent.map(|a| resolve_agent_tuning(project_root, a.program, AgentSeat::Reviewer));
    let (seat, name) = (seat.unwrap_or_default(), agent.map_or("", |a| a.program));
    let seat_effort = format!("[agents.{name}.seats.reviewer] effort");
    let seat_model = format!("[agents.{name}.seats.reviewer] model");
    let (rules_effort, rules_model) = (&rules.default_effort, &rules.default_model);
    let effort = [
        ("--effort", flags.0),
        ("AIDA_REVIEW_EFFORT", env("AIDA_REVIEW_EFFORT")),
        ("review-rules.toml default_effort", rules_effort.as_deref()),
        (&seat_effort, seat.effort.as_deref()),
    ];
    let model = [
        ("--model", flags.1),
        ("AIDA_REVIEW_MODEL", env("AIDA_REVIEW_MODEL")),
        ("review-rules.toml default_model", rules_model.as_deref()),
        (&seat_model, seat.model.as_deref()),
    ];
    resolve(agent, &effort, &model)
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = aida_core::git_ops::host_git_command(root)
        .args(args)
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    let err = String::from_utf8_lossy(&out.stderr);
    ensure!(out.status.success(), "git {}: {}", args[0], err.trim());
    Ok(out.stdout)
}

/// `.aida/review-rules.toml` as of `base`. Refused when `head` (or, with no
/// head, the working tree) changes it, so a change cannot pick its own review.
pub(crate) fn trusted_review_rules(
    root: &Path,
    base: &str,
    head: Option<&str>,
) -> Result<ProjectReviewRulesConfig> {
    let revs = std::iter::once(base).chain(head);
    for rev in revs.clone() {
        ensure!(!rev.starts_with('-'), "invalid revision {rev:?}");
    }
    let mut diff = vec!["diff", "--name-only", "--no-renames", "--no-ext-diff"];
    diff.extend(revs.chain(["--", RULES_PATH]));
    ensure!(
        git(root, &diff)?.is_empty(),
        "refusing to review: this change modifies {RULES_PATH}; review settings come from the base revision"
    );
    let base_commit = format!("{base}^{{commit}}");
    let entry = git(root, &["ls-tree", "-z", &base_commit, "--", RULES_PATH])?;
    if entry.is_empty() {
        return Ok(ProjectReviewRulesConfig::default());
    }
    let entry = String::from_utf8_lossy(&entry);
    let blob = match entry.split([' ', '\t']).collect::<Vec<_>>()[..] {
        ["100644" | "100755", "blob", oid, _] => oid.to_owned(),
        _ => bail!("{RULES_PATH} is not a regular file at the base revision"),
    };
    let raw = String::from_utf8(git(root, &["cat-file", "blob", &blob])?)?;
    toml::from_str(&raw).with_context(|| format!("parse {RULES_PATH} at the base revision"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendors::review_runner::review_vendor;

    const INJECTED: &str = "high\"\nfoo=1";

    fn sources<'a>(at: usize, bad: &'a str, good: &'a str) -> [(&'a str, Option<&'a str>); 4] {
        let mut s = [
            ("flag", None),
            ("env", None),
            ("rules", None),
            ("seat", None),
        ];
        s[at].1 = Some(bad);
        // A valid higher source must not mask a bad lower one.
        if at > 0 {
            s[0].1 = Some(good);
        }
        s
    }

    #[test]
    fn s3b_tuning_refuses_injected_values_from_every_source() {
        let (claude, codex) = (review_vendor("claude").ok(), review_vendor("codex").ok());
        let long = "m".repeat(129);
        for at in 0..4 {
            let name = ["flag", "env", "rules", "seat"][at];
            for agent in [claude, codex, None] {
                let err = resolve(agent, &sources(at, INJECTED, "high"), &[]).unwrap_err();
                assert!(err.to_string().starts_with(name), "{err}");
            }
            for bad in ["-oops", "a b", "m\"x", long.as_str()] {
                let err = resolve(claude, &[], &sources(at, bad, "opus")).unwrap_err();
                assert!(err.to_string().starts_with(name), "{bad}: {err}");
            }
            assert!(resolve(codex, &sources(at, "max", "high"), &[]).is_err());
        }
        let effort = [
            ("flag", Some(" ")),
            ("env", Some("max")),
            ("rules", Some("low")),
        ];
        let model = [
            ("flag", None),
            ("env", Some("claude-opus-5-5")),
            ("x", Some("o3")),
        ];
        let tuning = resolve(claude, &effort, &model).unwrap();
        assert_eq!(
            (tuning.effort(), tuning.model()),
            (Some("max"), Some("claude-opus-5-5"))
        );
        assert_eq!(resolve(None, &[], &[]).unwrap(), ReviewTuning::default());

        // The real chain: env and the project's reviewer seat are sources too.
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".aida")).unwrap();
        let config = "[agents.codex.seats]\nreviewer = { effort = \"high\\\"\\nfoo=1\" }\n";
        std::fs::write(root.path().join(".aida/config.toml"), config).unwrap();
        let rules = ProjectReviewRulesConfig::default();
        let run = |env: &[(OsString, OsString)], flag| {
            resolve_for_run(codex, root.path(), env, (flag, None), &rules)
        };
        let err = run(&[], Some("high")).unwrap_err().to_string();
        assert!(
            err.starts_with("[agents.codex.seats.reviewer] effort"),
            "{err}"
        );
        let env = [("AIDA_REVIEW_EFFORT".into(), INJECTED.into())];
        let err = run(&env, None).unwrap_err().to_string();
        assert!(err.starts_with("AIDA_REVIEW_EFFORT"), "{err}");
        let argv = codex.unwrap().argv_with(Some("o3"), Some("xhigh"));
        let at = argv.iter().position(|a| a == "--").unwrap();
        assert_eq!(
            argv[at - 4..at],
            ["--model", "o3", "-c", "model_reasoning_effort=xhigh"]
        );
        let argv = claude.unwrap().argv_with(None, Some("max"));
        assert_eq!(argv[argv.len() - 2..], ["--effort", "max"]);
    }

    #[test]
    fn s3b_review_rules_come_from_the_trusted_base() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .status();
            assert!(ok.unwrap().success(), "{args:?}");
        };
        let commit = |tag: &str| {
            git(&["add", "-A"]);
            git(&["commit", "-qm", tag]);
            git(&["tag", tag]);
        };
        let write = |path: &str, body: &str| std::fs::write(root.join(path), body).unwrap();
        git(&["init", "-q", "."]);
        std::fs::create_dir(root.join(".aida")).unwrap();
        write("a.txt", "x");
        commit("empty");
        let empty = trusted_review_rules(root, "empty", Some("HEAD")).unwrap();
        assert!(empty.default_effort.is_none());
        write(RULES_PATH, "default_effort = \"medium\"\n");
        commit("base");
        let head = |rev: &str| trusted_review_rules(root, "base", Some(rev));
        write("a.txt", "y");
        commit("ok");
        let ok = head("ok").unwrap();
        assert_eq!(ok.default_effort.as_deref(), Some("medium"));
        // The PR's own copy is never read: an edit refuses, committed or not.
        write(RULES_PATH, "default_effort = \"low\"\n");
        let err = trusted_review_rules(root, "base", None).unwrap_err();
        assert!(err.to_string().contains("modifies .aida/review-rules.toml"));
        commit("edited");
        assert!(head("edited").unwrap_err().to_string().contains("modifies"));
        assert!(trusted_review_rules(root, "--output=x", None).is_err());
        std::fs::remove_file(root.join(RULES_PATH)).unwrap();
        commit("deleted");
        let err = head("deleted").unwrap_err();
        assert!(err.to_string().contains("modifies"));
        // An injected default in the trusted file still fails validation.
        let bad = ProjectReviewRulesConfig {
            default_effort: Some(INJECTED.into()),
            ..Default::default()
        };
        let codex = review_vendor("codex").ok();
        let err = resolve_for_run(codex, root, &[], (None, None), &bad).unwrap_err();
        assert!(err
            .to_string()
            .starts_with("review-rules.toml default_effort"));
    }
}
