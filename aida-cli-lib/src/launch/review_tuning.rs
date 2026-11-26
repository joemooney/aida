// trace:STORY-3120 | ai:claude
//! Model and reasoning effort for review runners. Every precedence source is
//! checked against a fixed allowlist before any argv exists, and the project's
//! `.aida/review-rules.toml` defaults and the `[agents]` reviewer seat come
//! from the trusted base revision, never from the change under review.
// Wired into `--run-review` with the runner slice; only the tests call it yet.
#![cfg_attr(not(test), allow(dead_code))]

use crate::vendors::review_runner::ReviewVendor;
use aida_core::agents_config::{resolve_agent_tuning_from, AgentSeat};
use aida_core::review_engine::ProjectReviewRulesConfig;
use anyhow::{bail, ensure, Context, Result};
use std::ffi::OsString;
use std::path::Path;

/// Effort levels for a custom runner command, which names no vendor.
const BASE_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh"];
const RULES_PATH: &str = ".aida/review-rules.toml";
/// Project files that carry `[agents.<vendor>.seats.reviewer]` tuning.
const SEAT_PATHS: [&str; 2] = [".aida/config.toml", ".aida/agents.toml"];
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

/// Review settings read from the trusted base revision.
#[derive(Debug, Default)]
pub(crate) struct TrustedBase {
    pub(crate) rules: ProjectReviewRulesConfig,
    /// Base blobs of `SEAT_PATHS`, in order; `None` when absent at the base.
    seat_files: [Option<Vec<u8>>; 2],
}

/// The reviewer seat from the base blobs and the operator's global
/// `~/.aida/agents.toml`; the reviewed checkout's copies are never read.
fn base_seat_tuning(
    base: &TrustedBase,
    vendor: &str,
) -> Result<aida_core::agents_config::ResolvedAgentTuning> {
    let dir = tempfile::tempdir()?;
    let mut paths = [None, None];
    for (i, body) in base.seat_files.iter().enumerate() {
        if let Some(body) = body {
            let path = dir.path().join(format!("{i}.toml"));
            std::fs::write(&path, body)?;
            paths[i] = Some(path);
        }
    }
    let global = aida_core::home::home_dir().map(|h| h.join(".aida/agents.toml"));
    let [config, agents] = paths;
    Ok(resolve_agent_tuning_from(
        global.as_deref(),
        config.as_deref(),
        agents.as_deref(),
        vendor,
        AgentSeat::Reviewer,
    ))
}

/// Resolve in the order flag > environment > review-rules.toml > the
/// `[agents]` reviewer seat. `base` must come from `trusted_review_base`.
pub(crate) fn resolve_for_run(
    agent: Option<&ReviewVendor>,
    parent_env: &[(OsString, OsString)],
    flags: (Option<&str>, Option<&str>),
    base: &TrustedBase,
) -> Result<ReviewTuning> {
    let rules = &base.rules;
    // A non-UTF-8 value is kept as an invalid one, so it refuses, never unsets.
    let env = |key: &str| {
        let found = parent_env.iter().find(|(k, _)| k == key);
        found.map(|(_, v)| v.to_str().unwrap_or("\u{fffd}"))
    };
    let seat = agent
        .map(|a| base_seat_tuning(base, a.program))
        .transpose()?;
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
    // A refs/replace entry must not substitute the trusted base's objects.
    let out = aida_core::git_ops::host_git_command(root)
        .arg("--no-replace-objects")
        .args(args)
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    let err = String::from_utf8_lossy(&out.stderr);
    ensure!(out.status.success(), "git {}: {}", args[0], err.trim());
    Ok(out.stdout)
}

fn base_blob(root: &Path, base_commit: &str, path: &str) -> Result<Option<Vec<u8>>> {
    let entry = git(root, &["ls-tree", "-z", base_commit, "--", path])?;
    if entry.is_empty() {
        return Ok(None);
    }
    let entry = String::from_utf8_lossy(&entry);
    let blob = match entry.split([' ', '\t']).collect::<Vec<_>>()[..] {
        ["100644" | "100755", "blob", oid, _] => oid.to_owned(),
        _ => bail!("{path} is not a regular file at the base revision"),
    };
    git(root, &["cat-file", "blob", &blob]).map(Some)
}

/// Review settings (`.aida/review-rules.toml` and the reviewer seat in
/// `SEAT_PATHS`) as of `base`. Refused when `head` (or, with no head, the
/// working tree) changes any of them, so a change cannot pick its own review.
///
/// The guard diffs `base` and `head` trees directly, so `base` must be the
/// merge-base of the forge's target branch and the reviewed head SHA, never
/// the moving target tip (a later target-side edit would refuse a stale PR)
/// and never a PR-supplied value.
pub(crate) fn trusted_review_base(
    root: &Path,
    base: &str,
    head: Option<&str>,
) -> Result<TrustedBase> {
    let revs = std::iter::once(base).chain(head);
    for rev in revs.clone() {
        ensure!(!rev.starts_with('-'), "invalid revision {rev:?}");
    }
    let mut diff = vec!["diff", "--name-only", "--no-renames", "--no-ext-diff"];
    diff.extend(revs.chain(["--", RULES_PATH]).chain(SEAT_PATHS));
    let changed = git(root, &diff)?;
    ensure!(
        changed.is_empty(),
        "refusing to review: this change modifies {}; review settings come from the base revision",
        String::from_utf8_lossy(&changed).trim().replace('\n', ", ")
    );
    let base_commit = format!("{base}^{{commit}}");
    let rules = match base_blob(root, &base_commit, RULES_PATH)? {
        None => ProjectReviewRulesConfig::default(),
        Some(raw) => toml::from_str(&String::from_utf8(raw)?)
            .with_context(|| format!("parse {RULES_PATH} at the base revision"))?,
    };
    let [config, agents] = SEAT_PATHS.map(|p| base_blob(root, &base_commit, p));
    let seat_files = [config?, agents?];
    Ok(TrustedBase { rules, seat_files })
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

        // The real chain: env and the base's reviewer seat are sources too.
        let config = "[agents.codex.seats]\nreviewer = { effort = \"high\\\"\\nfoo=1\" }\n";
        let base = TrustedBase {
            seat_files: [Some(config.into()), None],
            ..Default::default()
        };
        let run =
            |env: &[(OsString, OsString)], flag| resolve_for_run(codex, env, (flag, None), &base);
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
                .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
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
        let empty = trusted_review_base(root, "empty", Some("HEAD")).unwrap();
        assert!(empty.rules.default_effort.is_none());
        write(RULES_PATH, "default_effort = \"medium\"\n");
        write(
            SEAT_PATHS[0],
            "[agents.codex.seats.reviewer]\neffort = \"high\"\n",
        );
        commit("base");
        let head = |rev: &str| trusted_review_base(root, "base", Some(rev));
        write("a.txt", "y");
        commit("ok");
        let ok = head("ok").unwrap();
        assert_eq!(ok.rules.default_effort.as_deref(), Some("medium"));
        let codex = review_vendor("codex").ok();
        let seat = |b: &TrustedBase| {
            let no_rules = TrustedBase {
                seat_files: b.seat_files.clone(),
                ..Default::default()
            };
            resolve_for_run(codex, &[], (None, None), &no_rules)
                .unwrap()
                .effort()
        };
        assert_eq!(seat(&ok), Some("high"));
        // A change lowering its own reviewer seat refuses, committed or not.
        let low = "[agents.codex.seats.reviewer]\neffort = \"low\"\n";
        write(SEAT_PATHS[0], low);
        let err = trusted_review_base(root, "base", None).unwrap_err();
        assert!(
            err.to_string().contains("modifies .aida/config.toml"),
            "{err}"
        );
        commit("seat");
        let err = head("seat").unwrap_err();
        assert!(
            err.to_string().contains("modifies .aida/config.toml"),
            "{err}"
        );
        git(&["reset", "-q", "--hard", "ok"]);
        // An untracked .aida/agents.toml in the checkout is never read.
        write(SEAT_PATHS[1], low);
        assert_eq!(seat(&head("ok").unwrap()), Some("high"));
        std::fs::remove_file(root.join(SEAT_PATHS[1])).unwrap();
        git(&["reset", "-q", "--hard", "ok"]);
        // The PR's own copy is never read: an edit refuses, committed or not.
        write(RULES_PATH, "default_effort = \"low\"\n");
        let err = trusted_review_base(root, "base", None).unwrap_err();
        assert!(err.to_string().contains("modifies .aida/review-rules.toml"));
        commit("edited");
        assert!(head("edited").unwrap_err().to_string().contains("modifies"));
        assert!(trusted_review_base(root, "--output=x", None).is_err());
        std::fs::remove_file(root.join(RULES_PATH)).unwrap();
        commit("deleted");
        let err = head("deleted").unwrap_err();
        assert!(err.to_string().contains("modifies"));
        // An injected default in the trusted file still fails validation.
        let bad = ProjectReviewRulesConfig {
            default_effort: Some(INJECTED.into()),
            ..Default::default()
        };
        let bad = TrustedBase {
            rules: bad,
            ..Default::default()
        };
        let err = resolve_for_run(codex, &[], (None, None), &bad).unwrap_err();
        assert!(err
            .to_string()
            .starts_with("review-rules.toml default_effort"));
    }
}
