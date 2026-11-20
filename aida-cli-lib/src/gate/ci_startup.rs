//! Review-decision observation used to count PRs blocked by required CI startup failures.
// mandate-gate
// trace:STORY-2856 | ai:codex
use anyhow::Context;
use std::path::Path;

use crate::forge_observed::Observed;

pub(crate) fn github_startup_review_decision(
    root: &Path,
    number: u64,
    head: &str,
) -> Observed<Option<String>> {
    Observed::from_result((|| {
        let out = crate::forge::github_startup_output(
            root,
            &[
                "pr",
                "view",
                &number.to_string(),
                "--json",
                "headRefOid,reviewDecision",
            ],
        )?;
        anyhow::ensure!(out.status.success(), "PR review observation failed");
        let value: serde_json::Value = serde_json::from_slice(&out.stdout)?;
        let observed_head = value["headRefOid"].as_str().context("missing PR head")?;
        if observed_head != head {
            return Ok(None);
        }
        value["reviewDecision"]
            .as_str()
            .map(|decision| Some(decision.to_owned()))
            .context("missing review decision")
    })())
}
