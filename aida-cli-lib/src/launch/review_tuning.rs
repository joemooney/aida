// trace:STORY-3120 | ai:claude
//! Model and reasoning effort for review runners. Every precedence source is
//! checked against a fixed allowlist before any argv exists, and the project's
//! `.aida/review-rules.toml` defaults and the `[agents]` reviewer seat come
//! from the trusted base revision, never from the change under review.

use crate::vendors::review_runner::ReviewVendor;
use aida_core::agents_config::{resolve_agent_tuning_from, AgentSeat};
use aida_core::review_engine::ProjectReviewRulesConfig;
use anyhow::{bail, ensure, Context, Result};
use std::ffi::OsString;
use std::path::Path;

/// Effort levels for a custom runner command, which names no vendor.
pub(crate) const BASE_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh"];
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
    // trace:STORY-3120 | ai:antigravity
    pub(crate) fn set_effort_if_none(&mut self, effort: Option<&'static str>) {
        if self.effort.is_none() {
            self.effort = effort;
        }
    }
}

/// Step up reasoning effort by one tier (`low` -> `medium` -> `high` -> `xhigh`, capped at `xhigh`)
/// when Finder and Challenger run on the same vendor and no explicit Challenger effort was set.
// trace:STORY-3120 | ai:antigravity
pub(crate) fn step_up_challenger_effort(
    agent: Option<&ReviewVendor>,
    finder_effort: Option<&str>,
) -> Option<&'static str> {
    let target = match finder_effort
        .map(|e| e.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("low") => "medium",
        Some("medium") | None => "high",
        Some("high" | "xhigh" | "max") => "xhigh",
        _ => "high",
    };
    let allowed = agent.map_or(BASE_EFFORTS, |a| a.efforts);
    allowed.iter().copied().find(|&e| e == target)
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
    /// Whether this repository is a standalone repo without an initialized AIDA store.
    pub(crate) standalone_without_aida: bool,
}

/// The reviewer seat from the base blobs and the operator's global
/// `~/.aida/agents.toml`; the reviewed checkout's copies are never read.
fn base_seat_tuning(
    base: &TrustedBase,
    vendor: &str,
) -> Result<aida_core::agents_config::ResolvedAgentTuning> {
    with_base_seat_files(base, |global, config, agents| {
        resolve_agent_tuning_from(global, config, agents, vendor, AgentSeat::Reviewer)
    })
}

/// Run `read` over the operator's global `~/.aida/agents.toml` and the base
/// revision's `SEAT_PATHS` blobs (written to a private tempdir), in
/// `(global, config.toml, agents.toml)` order.
fn with_base_seat_files<T>(
    base: &TrustedBase,
    read: impl FnOnce(Option<&Path>, Option<&Path>, Option<&Path>) -> T,
) -> Result<T> {
    let dir = tempfile::tempdir()?;
    let mut paths = [None, None];
    if !base.standalone_without_aida {
        for (i, body) in base.seat_files.iter().enumerate() {
            if let Some(body) = body {
                let path = dir.path().join(format!("{i}.toml"));
                std::fs::write(&path, body)?;
                paths[i] = Some(path);
            }
        }
    }
    let global = aida_core::home::home_dir().map(|h| h.join(".aida/agents.toml"));
    let [config, agents] = paths;
    Ok(read(
        global.as_deref(),
        config.as_deref(),
        agents.as_deref(),
    ))
}

#[cfg(test)]
impl TrustedBase {
    /// A base whose `.aida/config.toml` / `.aida/agents.toml` blobs are given.
    pub(crate) fn with_seat_files(config: Option<&str>, agents: Option<&str>) -> Self {
        let blob = |b: Option<&str>| b.map(|s| s.as_bytes().to_vec());
        Self {
            seat_files: [blob(config), blob(agents)],
            ..Default::default()
        }
    }
}

/// The finder's default `[agents] vendor` from trusted sources only: the base
/// revision's `.aida/config.toml` / `.aida/agents.toml` and the operator's
/// `~/.aida/agents.toml`. The checkout under review is never read, so a change
/// cannot pick its own reviewer. Returns the resolved vendor (aida-core
/// recognizes only claude and codex) and every raw `[agents] vendor` value
/// those files name, so the caller can say why a named vendor was not used.
// trace:STORY-3202 | ai:claude
pub(crate) fn trusted_default_vendor(base: &TrustedBase) -> Result<(Option<String>, Vec<String>)> {
    with_base_seat_files(base, |global, config, agents| {
        let resolved =
            aida_core::agents_config::resolve_default_vendor_from(global, config, agents);
        let named = [agents, config, global]
            .into_iter()
            .flatten()
            .filter_map(raw_vendor)
            .collect();
        (resolved, named)
    })
}

/// The raw `[agents] vendor` string in `path`, trimmed and lowercased.
fn raw_vendor(path: &Path) -> Option<String> {
    let value: toml::Value = toml::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let raw = value.get("agents")?.get("vendor")?.as_str()?;
    Some(raw.trim().to_ascii_lowercase())
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
    let env = |key| env_value(parent_env, key);
    let (seat, seat_effort, seat_model) = seat_sources(agent, base)?;
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

/// A non-UTF-8 value is kept as an invalid one, so it refuses, never unsets.
fn env_value<'a>(parent_env: &'a [(OsString, OsString)], key: &str) -> Option<&'a str> {
    let found = parent_env.iter().find(|(k, _)| k == key);
    found.map(|(_, v)| v.to_str().unwrap_or("\u{fffd}"))
}

/// `agent`'s base reviewer seat tuning and the source names for its fields.
fn seat_sources(
    agent: Option<&ReviewVendor>,
    base: &TrustedBase,
) -> Result<(
    aida_core::agents_config::ResolvedAgentTuning,
    String,
    String,
)> {
    let seat = agent
        .map(|a| base_seat_tuning(base, a.program))
        .transpose()?;
    let name = agent.map_or("", |a| a.program);
    Ok((
        seat.unwrap_or_default(),
        format!("[agents.{name}.seats.reviewer] effort"),
        format!("[agents.{name}.seats.reviewer] model"),
    ))
}

/// The `--adversarial` challenger's tuning: effort from `--challenger-effort`
/// > `AIDA_REVIEW_CHALLENGER_EFFORT` > base `challenger_effort` > the
/// challenger vendor's base reviewer seat; model only from that seat. The
/// finder's `--effort`/`--model`, `AIDA_REVIEW_EFFORT`/`AIDA_REVIEW_MODEL` and
/// `default_effort`/`default_model` belong to the finder's vendor and are never
/// read, so one vendor's model never reaches another.
// trace:STORY-3120 | ai:claude
pub(crate) fn resolve_challenger(
    agent: Option<&ReviewVendor>,
    parent_env: &[(OsString, OsString)],
    flag: Option<&str>,
    base: &TrustedBase,
) -> Result<ReviewTuning> {
    let (seat, seat_effort, seat_model) = seat_sources(agent, base)?;
    let effort = [
        ("--challenger-effort", flag),
        (
            "AIDA_REVIEW_CHALLENGER_EFFORT",
            env_value(parent_env, "AIDA_REVIEW_CHALLENGER_EFFORT"),
        ),
        (
            "review-rules.toml challenger_effort",
            base.rules.challenger_effort.as_deref(),
        ),
        (&seat_effort, seat.effort.as_deref()),
    ];
    resolve(agent, &effort, &[(&seat_model, seat.model.as_deref())])
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
///
/// In a repository without an initialized AIDA store (`.aida-store` or legacy
/// `requirements.db`), `.aida/config.toml` and `.aida/agents.toml` inside the
/// repository are never loaded into `seat_files`, so the default review vendor
/// and reviewer seat tuning come only from the operator's `~/.aida/` config.
// trace:STORY-3120 | ai:claude
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
    let empty_tree =
        String::from_utf8_lossy(&git(root, &["hash-object", "-t", "tree", "/dev/null"])?)
            .trim()
            .to_owned();
    let base_commit = if base.eq_ignore_ascii_case(&empty_tree) {
        empty_tree
    } else {
        format!("{base}^{{commit}}")
    };
    let rules = match base_blob(root, &base_commit, RULES_PATH)? {
        None => ProjectReviewRulesConfig::default(),
        Some(raw) => toml::from_str(&String::from_utf8(raw)?)
            .with_context(|| format!("parse {RULES_PATH} at the base revision"))?,
    };
    let [config, agents] = SEAT_PATHS.map(|p| base_blob(root, &base_commit, p));
    let seat_files = [config?, agents?];
    let has_aida_store = std::fs::symlink_metadata(root.join(".aida-store")).is_ok()
        || root.join("requirements.db").is_file()
        || root.join("requirements.yaml").is_file();
    Ok(TrustedBase {
        rules,
        seat_files,
        standalone_without_aida: !has_aida_store,
    })
}

/// Read `.aida/review-rules.toml` from a committed revision (`rev`), never from
/// uncommitted working-tree files. Used when `aida review prepare` runs without
/// `--run-review`.
// trace:STORY-3120 | ai:antigravity
pub(crate) fn committed_review_rules(
    root: &Path,
    rev: Option<&str>,
) -> Result<ProjectReviewRulesConfig> {
    let Some(rev) = rev else {
        return Ok(ProjectReviewRulesConfig::default());
    };
    ensure!(!rev.starts_with('-'), "invalid revision {rev:?}");
    let empty_tree =
        String::from_utf8_lossy(&git(root, &["hash-object", "-t", "tree", "/dev/null"])?)
            .trim()
            .to_owned();
    let base_commit = if rev.eq_ignore_ascii_case(&empty_tree) {
        empty_tree
    } else {
        format!("{rev}^{{commit}}")
    };
    match base_blob(root, &base_commit, RULES_PATH)? {
        None => Ok(ProjectReviewRulesConfig::default()),
        Some(raw) => toml::from_str(&String::from_utf8(raw)?)
            .with_context(|| format!("parse {RULES_PATH}"))
            .context("load .aida/review-rules.toml"),
    }
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
