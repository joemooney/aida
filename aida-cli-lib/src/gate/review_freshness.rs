//! Trusted review freshness policy. Branch-authored configuration never lowers assurance.
// mandate-gate
// trace:STORY-2763 | ai:codex
use anyhow::{Context, Result};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Freshness {
    Fast,
    Strict,
}

#[derive(serde::Deserialize, Default)]
struct Policy {
    freshness: Option<String>,
    #[serde(default)]
    freshness_strict_paths: Vec<String>,
}

impl Policy {
    fn parse(text: &str) -> Result<Self> {
        #[derive(serde::Deserialize)]
        struct Config {
            #[serde(default)]
            review: Policy,
        }
        let policy = toml::from_str::<Config>(text)?.review;
        anyhow::ensure!(
            matches!(policy.freshness.as_deref(), None | Some("fast" | "strict")),
            "review.freshness must be fast or strict"
        );
        anyhow::ensure!(
            policy.freshness_strict_paths.len() <= 128,
            "too many strict paths"
        );
        for pattern in &policy.freshness_strict_paths {
            glob(pattern)?;
        }
        Ok(policy)
    }

    fn for_paths(&self, paths: &[&str]) -> Result<Freshness> {
        if self.freshness.as_deref() == Some("strict") {
            return Ok(Freshness::Strict);
        }
        for pattern in &self.freshness_strict_paths {
            let matcher = glob(pattern)?;
            if paths.iter().any(|path| matcher.is_match(path)) {
                return Ok(Freshness::Strict);
            }
        }
        Ok(Freshness::Fast)
    }
}

// Deliberately closed glob grammar: path literals, *, ** and ?. Unsupported
// syntax refuses configuration rather than silently weakening a protected path.
fn glob(pattern: &str) -> Result<regex::Regex> {
    anyhow::ensure!(
        !pattern.is_empty()
            && pattern.len() <= 1024
            && !pattern.starts_with('/')
            && !pattern.split('/').any(|p| p == "..")
            && !pattern
                .chars()
                .any(|c| matches!(c, '[' | ']' | '{' | '}' | '\\' | '\0')),
        "invalid review.freshness_strict_paths glob"
    );
    let mut expression = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    expression.push_str("(?:.*/)?");
                } else {
                    expression.push_str(".*");
                }
            }
            '*' => expression.push_str("[^/]*"),
            '?' => expression.push_str("[^/]"),
            _ => expression.push_str(&regex::escape(&c.to_string())),
        }
    }
    expression.push('$');
    Ok(regex::Regex::new(&expression)?)
}

/// Read the default-branch policy, never the PR's copy. Unknown paths or bad
/// policy select strict. Missing keys retain legacy carry-over eligibility.
pub(crate) fn for_head(root: &Path, head: &str) -> Freshness {
    effective(root, head).unwrap_or(Freshness::Strict)
}

/// Whether the trusted project-wide setting admits fast freshness at all
/// (unreadable policy is strict). Per-path overrides still need [`for_head`].
// trace:STORY-2763 | ai:claude
pub(crate) fn project_admits_fast(root: &Path) -> bool {
    load_policy(root).is_ok_and(|p| p.freshness.as_deref() != Some("strict"))
}

fn load_policy(root: &Path) -> Result<Policy> {
    let text = crate::trusted_config::read_trusted_config_toml_local(root)
        .context("trusted review configuration unavailable")?;
    Policy::parse(&text)
}

fn effective(root: &Path, head: &str) -> Result<Freshness> {
    let policy = load_policy(root)?;
    if policy.freshness_strict_paths.is_empty() {
        return policy.for_paths(&[]);
    }
    let base = crate::trusted_config::local_trusted_default_branch_sha(root)
        .context("trusted base unavailable")?;
    let ancestor = super::review_carryover::single_merge_base(root, &base, head)
        .map_err(|e| anyhow::anyhow!(e.reason()))?;
    let paths = git(
        root,
        &[
            "diff",
            "--no-renames",
            "--name-only",
            "-z",
            &ancestor,
            head,
            "--",
        ],
    )?;
    policy.for_paths(
        &paths
            .split('\0')
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = aida_core::git_ops::host_git_command(root)
        .args(args)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git {} failed: {}",
        args[0],
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

/// A synthetic merge tree is review evidence only; never a publication head or
/// approval identity. The reviewer still attests the original PR head.
pub(crate) fn landed_tree(root: &Path, base: &str, head: &str) -> Result<String> {
    let output = aida_core::git_ops::host_git_command(root)
        .args(["merge-tree", "--write-tree", "--no-messages", base, head])
        .output()?;
    if output.status.code() == Some(1) {
        return Err(NeedsRework.into());
    }
    anyhow::ensure!(
        output.status.success(),
        "merge-tree could not establish review evidence"
    );
    let tree = String::from_utf8(output.stdout)?.trim().to_string();
    anyhow::ensure!(
        matches!(tree.len(), 40 | 64) && tree.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid merge tree"
    );
    Ok(tree)
}

#[derive(Debug)]
pub(crate) struct NeedsRework;
impl std::fmt::Display for NeedsRework {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("review needs rework: the PR conflicts with the protected base")
    }
}
impl std::error::Error for NeedsRework {}

pub(crate) fn decision_exists(root: &Path, pr: u64, head: &str, decision: &str) -> Result<bool> {
    use super::ledger::{self, ChangeKey, EventBody};
    let store = ledger::store_root(root)?;
    let events = ledger::checked::read_change_events(
        &store,
        ChangeKey::pr(pr),
        &mut ledger::checked::Budget::default(),
    )
    .map_err(|e| anyhow::anyhow!("freshness ledger unreadable: {e}"))?;
    Ok(events.iter().any(|e| {
        e.head.as_deref() == Some(head)
            && matches!(&e.body, EventBody::FreshnessDecision { decision: d, .. } if d == decision)
    }))
}

/// Idempotent advisory evidence. A failed write cannot masquerade as an
/// audited admission. Readers use the original proof, never this event.
pub(crate) fn record(
    root: &Path,
    pr: u64,
    head: &str,
    policy: Freshness,
    decision: &str,
    proof: serde_json::Value,
) -> Result<()> {
    use super::ledger::{self, ChangeKey, EventBody, GateEvent};
    let change = ChangeKey::pr(pr);
    let policy = match policy {
        Freshness::Fast => "fast",
        Freshness::Strict => "strict",
    };
    let store = ledger::store_root(root)?;
    let events = ledger::checked::read_change_events(
        &store,
        change,
        &mut ledger::checked::Budget::default(),
    )
    .map_err(|error| anyhow::anyhow!("freshness ledger unreadable: {error}"))?;
    if events.iter().any(|event| event.head.as_deref() == Some(head)
        && matches!(&event.body, EventBody::FreshnessDecision { policy: p, decision: d, proof: v }
            if p == policy && d == decision && v == &proof)) {
        return Ok(());
    }
    let mut event = GateEvent::new(
        change,
        EventBody::FreshnessDecision {
            policy: policy.into(),
            decision: decision.into(),
            proof,
        },
    );
    event.head = Some(head.into());
    ledger::record_event(root, event)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn review_watch_freshness_policy_is_closed_and_paths_raise_assurance() {
        let fast = Policy::parse("[review]\nfreshness = 'fast'\nfreshness_strict_paths = ['src/gate/**', '**/secret?.rs']").unwrap();
        assert_eq!(fast.for_paths(&["src/a.rs"]).unwrap(), Freshness::Fast);
        assert_eq!(
            fast.for_paths(&["src/gate/a.rs"]).unwrap(),
            Freshness::Strict
        );
        assert_eq!(fast.for_paths(&["secret1.rs"]).unwrap(), Freshness::Strict);
        assert_eq!(
            fast.for_paths(&["src/secret1.rs"]).unwrap(),
            Freshness::Strict
        );
        assert!(Policy::parse("[review]\nfreshness = 'typo'").is_err());
        assert!(Policy::parse("[review]\nfreshness_strict_paths = ['[x]']").is_err());
        let strict = Policy::parse("[review]\nfreshness = 'strict'").unwrap();
        assert_eq!(strict.for_paths(&[]).unwrap(), Freshness::Strict);
    }
}
