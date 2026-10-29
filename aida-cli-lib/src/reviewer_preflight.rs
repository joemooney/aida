//! BUG-014: a contained Claude reviewer in a repository without the egress
//! allowlist or the reviewer allow entries cannot reach the forge or run
//! `aida`, so it exits with no verdict and only a terse denial count. The
//! launcher checks both before it mints a lease or a worktree and refuses,
//! naming what is missing and the command that adds it.
//!
//! The reviewer loads `.claude/settings.json` and `.aida/config.toml` from
//! its review worktree — the PR branch — so the PR branch's copies are the
//! ones checked. When the base has them and the branch does not, the fix is
//! a merge of the base, not `aida scaffold upgrade`.
//! trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude

use anyhow::Result;
use std::path::Path;

const SETTINGS_PATH: &str = ".claude/settings.json";
const CONFIG_PATH: &str = ".aida/config.toml";

/// One revision's copies of the two files the reviewer loads. `None` is a
/// file absent at that revision.
#[derive(Debug, Default, Clone)]
pub(crate) struct ReviewerConfigFiles {
    pub(crate) settings: Option<String>,
    pub(crate) config: Option<String>,
}

impl ReviewerConfigFiles {
    /// The working-tree copies under `root`.
    pub(crate) fn from_worktree(root: &Path) -> Self {
        Self {
            settings: std::fs::read_to_string(root.join(SETTINGS_PATH)).ok(),
            config: std::fs::read_to_string(root.join(CONFIG_PATH)).ok(),
        }
    }

    /// The copies committed at `rev`, read with `git show` from `repo`.
    fn at_revision(repo: &Path, rev: &str) -> Self {
        let show = |path: &str| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["show", &format!("{rev}:{path}")])
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        };
        Self {
            settings: show(SETTINGS_PATH),
            config: show(CONFIG_PATH),
        }
    }

    /// The [`aida_core::scaffolding::REVIEWER_ALLOW_ENTRIES`] absent from
    /// `permissions.allow`. A missing or unparseable file lacks them all.
    pub(crate) fn missing_allow_entries(&self) -> Vec<&'static str> {
        let allow: Vec<String> = self
            .settings
            .as_deref()
            .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
            .and_then(|v| v.pointer("/permissions/allow")?.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|e| e.as_str().map(str::to_string))
            .collect();
        aida_core::scaffolding::REVIEWER_ALLOW_ENTRIES
            .iter()
            .copied()
            .filter(|entry| !allow.iter().any(|a| a == entry))
            .collect()
    }

    /// Whether `[contained] allowed_hosts` is a non-empty array.
    pub(crate) fn has_allowed_hosts(&self) -> bool {
        self.config
            .as_deref()
            .and_then(|body| toml::from_str::<toml::Value>(body).ok())
            .and_then(|v| {
                v.get("contained")?
                    .get("allowed_hosts")?
                    .as_array()
                    .map(|a| !a.is_empty())
            })
            .unwrap_or(false)
    }

    /// What this revision lacks, in operator prose; empty when complete.
    fn missing(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if !self.has_allowed_hosts() {
            missing.push(format!(
                "the contained egress allowlist (`[contained] allowed_hosts` in {CONFIG_PATH})"
            ));
        }
        let absent = self.missing_allow_entries();
        if !absent.is_empty() {
            missing.push(format!(
                "the reviewer allow entries {} (`permissions.allow` in {SETTINGS_PATH})",
                absent
                    .iter()
                    .map(|e| format!("`{e}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        missing
    }
}

/// Decide from the PR branch's copies (`branch`) and the base checkout's
/// (`base`, named `base_name`). Pure.
pub(crate) fn contained_reviewer_decision(
    pr: u64,
    branch: &ReviewerConfigFiles,
    base: &ReviewerConfigFiles,
    base_name: &str,
) -> Result<()> {
    let missing = branch.missing();
    if missing.is_empty() {
        return Ok(());
    }
    let fix = if base.missing().is_empty() {
        format!(
            "{base_name} already has it: merge {base_name} into the PR branch first, push, then re-run"
        )
    } else {
        format!(
            "run `aida scaffold upgrade` on {base_name} to add it, commit {CONFIG_PATH} and \
             {SETTINGS_PATH}, merge {base_name} into the PR branch, then re-run"
        )
    };
    anyhow::bail!(
        "refusing to launch a contained reviewer for PR-{pr}: the PR branch lacks {}. The \
         reviewer loads these files from the PR branch; without them it cannot reach the \
         forge or run `aida`, and exits with no verdict. Fix: {fix}.",
        missing.join(" and ")
    )
}

/// The base branch name: `origin/HEAD`'s target, else `main`.
fn base_branch_name(project_root: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .and_then(|r| r.strip_prefix("origin/").map(str::to_string))
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "main".to_string())
}

/// Refuse a contained reviewer launch for PR-`pr` whose branch cannot carry
/// it. The PR head is fetched into a private ref; when the fetch fails the
/// launch is not refused here — the worktree fetch that follows reports it.
pub(crate) fn preflight_contained_reviewer(
    project_root: &Path,
    forge: crate::ReviewForge,
    pr: u64,
) -> Result<()> {
    let local_ref = format!("refs/aida/reviewer-preflight/pr-{pr}");
    let fetched = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["fetch", "--quiet", "--no-tags", "origin"])
        .arg(format!(
            "+{}:{local_ref}",
            crate::pr_head_remote_ref_for(forge, pr)
        ))
        .status()
        .is_ok_and(|s| s.success());
    if !fetched {
        eprintln!(
            "  {} could not fetch PR-{pr}'s head to check its reviewer config; skipping that check",
            crate::glyph(crate::glyphs::Glyph::Warning)
        );
        return Ok(());
    }
    let branch = ReviewerConfigFiles::at_revision(project_root, &local_ref);
    let base = ReviewerConfigFiles::from_worktree(project_root);
    contained_reviewer_decision(pr, &branch, &base, &base_branch_name(project_root))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOW: &str = r#"{"permissions": {"allow": ["Bash(aida)", "Bash(aida *)"]}}"#;
    const HOSTS: &str = "[contained]\nallowed_hosts = [\"api.github.com\"]\n";

    fn files(config: &str, settings: Option<&str>) -> ReviewerConfigFiles {
        ReviewerConfigFiles {
            settings: settings.map(str::to_string),
            config: Some(config.to_string()),
        }
    }

    fn decide(branch: &ReviewerConfigFiles, base: &ReviewerConfigFiles) -> Result<()> {
        contained_reviewer_decision(7, branch, base, "main")
    }

    #[test]
    fn bug_014_preflight_passes_a_configured_branch() {
        let ok = files(HOSTS, Some(ALLOW));
        decide(&ok, &ReviewerConfigFiles::default()).unwrap();
    }

    #[test]
    fn bug_014_preflight_refuses_without_the_egress_allowlist() {
        let branch = files("[forge]\n", Some(ALLOW));
        let err = decide(&branch, &branch).unwrap_err().to_string();
        assert!(err.contains("[contained] allowed_hosts"), "{err}");
        assert!(!err.contains("permissions.allow"), "{err}");
        assert!(err.contains("aida scaffold upgrade"), "{err}");
    }

    #[test]
    fn bug_014_preflight_refuses_without_the_reviewer_allow_entries() {
        let branch = files(HOSTS, Some(r#"{"permissions": {"allow": ["Bash(aida)"]}}"#));
        let err = decide(&branch, &branch).unwrap_err().to_string();
        assert!(err.contains("`Bash(aida *)`"), "{err}");
        assert!(!err.contains("`Bash(aida)`"), "{err}");
        assert!(!err.contains("allowed_hosts"), "{err}");

        let bare = files(HOSTS, None);
        let err = decide(&bare, &bare).unwrap_err().to_string();
        assert!(err.contains("permissions.allow"), "{err}");
    }

    /// The advisor addendum: the base has the config, the PR branch (cut
    /// before it landed) does not — the fix is merging the base.
    #[test]
    fn bug_014_preflight_names_the_base_merge_when_only_the_base_has_it() {
        let base = files(HOSTS, Some(ALLOW));
        let branch = files("[forge]\n", Some(ALLOW));
        let err = decide(&branch, &base).unwrap_err().to_string();
        assert!(err.contains("merge main into the PR branch first"), "{err}");
        assert!(err.contains("PR-7"), "{err}");
        assert!(!err.contains("aida scaffold upgrade"), "{err}");
    }

    /// End to end against git: the base checkout has the config committed,
    /// the PR head predates it, and the preflight reads the head's copy.
    #[test]
    fn bug_014_preflight_reads_the_pr_branch_copy_not_the_base() {
        let git = |dir: &Path, args: &[&str]| {
            let ok = crate::test_git::git_command()
                .arg("-C")
                .arg(dir)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        let origin = tempfile::tempdir().unwrap();
        git(origin.path(), &["init", "--quiet", "-b", "main"]);
        std::fs::create_dir_all(origin.path().join(".aida")).unwrap();
        std::fs::write(origin.path().join(".aida/config.toml"), "[forge]\n").unwrap();
        git(origin.path(), &["add", "."]);
        git(origin.path(), &["commit", "--quiet", "-m", "base"]);
        git(origin.path(), &["update-ref", "refs/pull/7/head", "HEAD"]);
        std::fs::write(origin.path().join(".aida/config.toml"), HOSTS).unwrap();
        std::fs::create_dir_all(origin.path().join(".claude")).unwrap();
        std::fs::write(origin.path().join(".claude/settings.json"), ALLOW).unwrap();
        git(origin.path(), &["add", "."]);
        git(origin.path(), &["commit", "--quiet", "-m", "config"]);

        let clone = tempfile::tempdir().unwrap();
        let ok = crate::test_git::git_command()
            .args(["clone", "--quiet"])
            .arg(origin.path())
            .arg(clone.path())
            .status()
            .unwrap()
            .success();
        assert!(ok);

        let err = preflight_contained_reviewer(clone.path(), crate::ReviewForge::GitHub, 7)
            .unwrap_err()
            .to_string();
        assert!(err.contains("merge main into the PR branch first"), "{err}");

        git(origin.path(), &["update-ref", "refs/pull/7/head", "main"]);
        preflight_contained_reviewer(clone.path(), crate::ReviewForge::GitHub, 7).unwrap();
    }

    /// A fresh scaffold passes: the init config section and the template
    /// master's allow entries (which the generated settings carry, see the
    /// core scaffold test) hold what the preflight requires.
    #[test]
    fn bug_014_init_scaffold_satisfies_the_preflight() {
        let settings = serde_json::json!({
            "permissions": {"allow": aida_core::scaffolding::template_permissions_allow()}
        })
        .to_string();
        let scaffold = files(&crate::init_contained_config_section(), Some(&settings));
        decide(&scaffold, &ReviewerConfigFiles::default()).unwrap();
    }

    /// `aida scaffold upgrade`'s config step adds the template hosts to a
    /// config without them, and leaves a project's own allowlist alone.
    #[test]
    fn bug_014_upgrade_adds_hosts_only_when_absent() {
        let repo = |config: &str| {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
            std::fs::write(dir.path().join(".aida/config.toml"), config).unwrap();
            dir
        };
        let dir = repo("[forge]\nprovider = \"github\"\n");
        assert!(crate::ensure_contained_allowed_hosts(dir.path(), true).unwrap());
        assert!(
            !ReviewerConfigFiles::from_worktree(dir.path()).has_allowed_hosts(),
            "dry run writes nothing"
        );
        assert!(crate::ensure_contained_allowed_hosts(dir.path(), false).unwrap());
        assert_eq!(
            crate::session::contained_allowed_hosts(dir.path()),
            aida_core::scaffolding::template_contained_allowed_hosts()
        );

        let own = repo(HOSTS);
        assert!(!crate::ensure_contained_allowed_hosts(own.path(), false).unwrap());
        assert_eq!(
            crate::session::contained_allowed_hosts(own.path()),
            vec!["api.github.com".to_string()]
        );
    }
}
