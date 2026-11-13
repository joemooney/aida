//! Content identities for CI definitions, independent of their commit history.
// trace:STORY-2401 | ai:codex

use crate::forge::ForgeKind;
use anyhow::{Context, Result};
use std::path::Path;

/// CI evidence is valid only for this exact commit and workflow definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) struct CiEvidenceKey {
    pub head_sha: String,
    pub workflow_digest: String,
}

impl CiEvidenceKey {
    pub(crate) fn read(repo: &Path, rev: &str, forge: ForgeKind) -> Result<Self> {
        // Pin once: a moving symbolic ref must not mix a head with another
        // commit's definition. Missing commits are errors, never empty CI.
        let head_sha = git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{rev}^{{commit}}"),
            ],
        )?;
        let (path, kind, namespace) = match forge {
            // Preserve the workflow-path convention for local/forge-less reads.
            ForgeKind::GitHub | ForgeKind::None => (".github/workflows", "tree", "github"),
            ForgeKind::GitLab => (".gitlab-ci.yml", "blob", "gitlab"),
        };
        let entry = git(repo, &["ls-tree", &head_sha, "--", path])?;
        let workflow_digest = if entry.is_empty() {
            // Absence at a validated commit is evidence too; distinguish it
            // from a failed Git read and from the other forge's definitions.
            format!("{namespace}:absent")
        } else {
            let fields: Vec<_> = entry.split_whitespace().collect();
            anyhow::ensure!(
                fields.len() == 4
                    && fields[1] == kind
                    && fields[3] == path
                    && fields[0] != "120000",
                "invalid CI definition object at {head_sha}:{path}"
            );
            let oid = git(
                repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{head_sha}:{path}"),
                ],
            )?;
            anyhow::ensure!(
                oid == fields[2],
                "CI definition object changed while reading"
            );
            format!("{namespace}:{kind}:{oid}")
        };
        Ok(Self {
            head_sha,
            workflow_digest,
        })
    }
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("reading CI definition: git {}", args[0]))?;
    anyhow::ensure!(
        out.status.success(),
        "reading CI definition: git {} failed: {}",
        args[0],
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

#[cfg(test)]
#[path = "tests/story_2401_ci_digest_tests.rs"]
mod story_2401_ci_digest_tests;
