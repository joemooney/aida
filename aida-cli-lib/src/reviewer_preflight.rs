//! BUG-014: a contained Claude reviewer in a repository without the egress
//! allowlist or the reviewer allow entries cannot reach the forge or run
//! `aida`, so it exits with no verdict and only a terse denial count. The
//! launcher checks both before it mints a lease or a worktree and refuses,
//! naming what is missing and the command that adds it.
//!
//! The reviewer loads `.claude/settings.json` and `.aida/config.toml` from
//! its review worktree — the PR branch — so the PR branch's copies are the
//! ones checked. When the PR's base branch has them and the PR branch does
//! not, the fix is a merge of the base, not `aida scaffold upgrade`.
//! trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude

use aida_core::git_ops::RemoteCredentialsExt as _;
use anyhow::Result;
use std::path::Path;

const SETTINGS_PATH: &str = ".claude/settings.json";
const CONFIG_PATH: &str = ".aida/config.toml";

/// Whether the `permissions.allow` entry `entry` grants what the reviewer
/// entry `needed` (one of [`aida_core::scaffolding::REVIEWER_ALLOW_ENTRIES`])
/// grants. Besides the entry itself, three equivalent or broader forms are
/// accepted: the whole tool (`Bash`), every command (`Bash(*)`), and the
/// legacy colon prefix form (`Bash(aida:*)`, which matches `aida` with or
/// without arguments). Anything else — a narrower subcommand list, say —
/// does not count, so the check never passes a reviewer that would be denied.
pub(crate) fn allow_entry_covers(entry: &str, needed: &str) -> bool {
    matches!(entry.trim(), "Bash" | "Bash(*)" | "Bash(aida:*)") || entry.trim() == needed
}

/// The state of `[contained] allowed_hosts` in one copy of the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AllowedHosts {
    /// No key (or no file, or an unparseable one): `aida scaffold upgrade`
    /// adds the template hosts.
    Absent,
    /// `allowed_hosts = []`: the project's explicit choice, which the
    /// scaffold never overwrites, so the operator must edit it.
    Empty,
    Present,
}

/// How many symlinks one lookup follows before giving up (a cycle, or a
/// chain no layout needs).
const MAX_LINK_HOPS: usize = 8;

/// One revision's copies of the two files the reviewer loads. `None` is a
/// file absent at that revision. `broken_links` names each of the two paths
/// that is a symlink whose target dangles or leaves the tree; that file is
/// `None`.
#[derive(Debug, Default, Clone)]
pub(crate) struct ReviewerConfigFiles {
    pub(crate) settings: Option<String>,
    pub(crate) config: Option<String>,
    pub(crate) broken_links: Vec<String>,
}

/// The tree path a symlink at `link_path` with link text `target` points to,
/// resolved against the link's directory. `None` when the target is absolute
/// or climbs above the repository root.
/// trace:BUG-b6b1278890774ef993d2ebc327862470-016 | ai:claude
fn resolve_link_target(link_path: &str, target: &str) -> Option<String> {
    if target.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = link_path.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The entry at `path` in `rev`'s tree: its mode and object id. `None` when
/// the path is absent.
fn tree_entry(repo: &Path, rev: &str, path: &str) -> Option<(String, String)> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-tree", "-z", rev, "--", path])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let (meta, _) = line.split_once('\t')?;
    let mut fields = meta.split(' ');
    let mode = fields.next()?.to_string();
    let _kind = fields.next()?;
    Some((mode, fields.next()?.to_string()))
}

fn read_blob(repo: &Path, oid: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "blob", oid])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The content of `path` at `rev`, following committed symlinks (tree mode
/// 120000) within the same tree, as a checkout of `rev` would. `Ok(None)` is
/// a path absent at `rev`; `Err` names a link that dangles, escapes the
/// repository, or chains past [`MAX_LINK_HOPS`].
/// trace:BUG-b6b1278890774ef993d2ebc327862470-016 | ai:claude
fn read_following_links(repo: &Path, rev: &str, path: &str) -> Result<Option<String>, String> {
    let mut current = path.to_string();
    for hop in 0..=MAX_LINK_HOPS {
        let Some((mode, oid)) = tree_entry(repo, rev, &current) else {
            return if hop == 0 {
                Ok(None)
            } else {
                Err(format!(
                    "`{path}` is a symlink to `{current}`, which does not exist"
                ))
            };
        };
        if mode != "120000" {
            return Ok(read_blob(repo, &oid));
        }
        let target = read_blob(repo, &oid).unwrap_or_default();
        current =
            resolve_link_target(&current, target.trim_end_matches('\n')).ok_or_else(|| {
                format!("`{path}` is a symlink to `{target}`, which points outside the repository")
            })?;
    }
    Err(format!(
        "`{path}` is a chain of more than {MAX_LINK_HOPS} symlinks (last: `{current}`)"
    ))
}

impl ReviewerConfigFiles {
    /// The copies committed at `rev` in `repo`, following symlinks.
    fn at_revision(repo: &Path, rev: &str) -> Self {
        let mut broken_links = Vec::new();
        let mut read = |path: &str| {
            read_following_links(repo, rev, path).unwrap_or_else(|problem| {
                broken_links.push(problem);
                None
            })
        };
        let settings = read(SETTINGS_PATH);
        let config = read(CONFIG_PATH);
        Self {
            settings,
            config,
            broken_links,
        }
    }

    /// The reviewer entries no `permissions.allow` entry covers (see
    /// [`allow_entry_covers`]). A missing or unparseable file lacks them all.
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
            .filter(|needed| !allow.iter().any(|a| allow_entry_covers(a, needed)))
            .collect()
    }

    pub(crate) fn allowed_hosts(&self) -> AllowedHosts {
        let hosts = self
            .config
            .as_deref()
            .and_then(|body| toml::from_str::<toml::Value>(body).ok())
            .and_then(|v| Some(v.get("contained")?.get("allowed_hosts")?.as_array()?.len()));
        match hosts {
            None => AllowedHosts::Absent,
            Some(0) => AllowedHosts::Empty,
            Some(_) => AllowedHosts::Present,
        }
    }

    fn complete(&self) -> bool {
        self.broken_links.is_empty()
            && self.allowed_hosts() == AllowedHosts::Present
            && self.missing_allow_entries().is_empty()
    }
}

/// Decide from the PR branch's copies (`branch`) and the PR base branch's
/// (`base`, named by `base_name`; `None` when the base could not be read).
/// Pure.
pub(crate) fn contained_reviewer_preflight(
    pr: u64,
    branch: &ReviewerConfigFiles,
    base: Option<(&str, &ReviewerConfigFiles)>,
) -> Result<()> {
    // A broken link is its own problem: reporting the file's missing
    // entries would send the operator to edit a file that is not there.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-016 | ai:claude
    if !branch.broken_links.is_empty() {
        anyhow::bail!(
            "refusing to launch a contained reviewer for PR-{pr}: on the PR branch, {}. The \
             reviewer loads these files from the PR branch and cannot read them through a broken \
             link. Fix: point the link at a file inside the repository, commit, push, then re-run.",
            branch.broken_links.join("; ")
        );
    }
    let hosts = branch.allowed_hosts();
    let absent = branch.missing_allow_entries();
    if hosts == AllowedHosts::Present && absent.is_empty() {
        return Ok(());
    }
    let mut problems = Vec::new();
    match hosts {
        AllowedHosts::Present => {}
        AllowedHosts::Absent => problems.push(format!(
            "the PR branch has no contained egress allowlist (`[contained] allowed_hosts` in {CONFIG_PATH})"
        )),
        AllowedHosts::Empty => problems.push(format!(
            "allowed_hosts is empty in {CONFIG_PATH}; add github.com/api.github.com or remove the key to use the default"
        )),
    }
    if !absent.is_empty() {
        problems.push(format!(
            "the PR branch lacks the reviewer allow entries {} (`permissions.allow` in {SETTINGS_PATH})",
            absent
                .iter()
                .map(|e| format!("`{e}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let fix = match base {
        Some((name, base)) if base.complete() => format!(
            "{name} already has them: merge {name} into the PR branch first, push, then re-run"
        ),
        Some((name, _)) => format!(
            "on {name}, run `aida scaffold upgrade` (or fix the file by hand), commit {CONFIG_PATH} \
             and {SETTINGS_PATH}, merge {name} into the PR branch, then re-run"
        ),
        None => format!(
            "run `aida scaffold upgrade` (or fix the file by hand) on the PR's base branch, commit \
             {CONFIG_PATH} and {SETTINGS_PATH}, merge it into the PR branch, then re-run"
        ),
    };
    anyhow::bail!(
        "refusing to launch a contained reviewer for PR-{pr}: {}. The reviewer loads these files \
         from the PR branch; without them it cannot reach the forge or run `aida`, and exits with \
         no verdict. Fix: {fix}.",
        problems.join("; ")
    )
}

/// Fetch `remote_ref` into a private ref, read both files there, and delete
/// the ref. `None` when the fetch fails.
fn read_remote_revision(
    project_root: &Path,
    remote_ref: &str,
    local_ref: &str,
) -> Option<ReviewerConfigFiles> {
    let fetched = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["fetch", "--quiet", "--no-tags", "origin"])
        .arg(format!("+{remote_ref}:{local_ref}"))
        .remote_credentials(project_root, "origin")
        .status()
        .is_ok_and(|s| s.success());
    if !fetched {
        return None;
    }
    let files = ReviewerConfigFiles::at_revision(project_root, local_ref);
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["update-ref", "-d", local_ref])
        .status();
    Some(files)
}

/// Refuse a contained reviewer launch for PR-`pr` whose branch cannot carry
/// it, comparing against the PR's base branch from the forge metadata.
pub(crate) fn preflight_contained_reviewer(
    project_root: &Path,
    forge: crate::ReviewForge,
    pr: u64,
) -> Result<()> {
    let base_ref = crate::query_pr_metadata(forge, pr, project_root)
        .ok()
        .and_then(|meta| meta.base_ref);
    preflight_contained_reviewer_against(project_root, forge, pr, base_ref.as_deref())
}

/// [`preflight_contained_reviewer`] with the base branch already resolved.
/// When the PR head cannot be fetched the launch is not refused here — the
/// worktree fetch that follows reports it.
pub(crate) fn preflight_contained_reviewer_against(
    project_root: &Path,
    forge: crate::ReviewForge,
    pr: u64,
    base_ref: Option<&str>,
) -> Result<()> {
    let prefix = format!("refs/aida/reviewer-preflight/pr-{pr}");
    let Some(branch) = read_remote_revision(
        project_root,
        &crate::pr_head_remote_ref_for(forge, pr),
        &format!("{prefix}-head"),
    ) else {
        eprintln!(
            "  {} could not fetch PR-{pr}'s head to check its reviewer config; skipping that check",
            crate::glyph(crate::glyphs::Glyph::Warning)
        );
        return Ok(());
    };
    let base = base_ref.and_then(|name| {
        read_remote_revision(
            project_root,
            &format!("refs/heads/{name}"),
            &format!("{prefix}-base"),
        )
        .map(|files| (name, files))
    });
    contained_reviewer_preflight(pr, &branch, base.as_ref().map(|(n, f)| (*n, f)))
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
            broken_links: Vec::new(),
        }
    }

    fn decide(branch: &ReviewerConfigFiles, base: &ReviewerConfigFiles) -> Result<()> {
        contained_reviewer_preflight(7, branch, Some(("main", base)))
    }

    #[test]
    fn bug_014_preflight_passes_a_configured_branch() {
        let ok = files(HOSTS, Some(ALLOW));
        contained_reviewer_preflight(7, &ok, None).unwrap();
    }

    #[test]
    fn bug_014_preflight_refuses_without_the_egress_allowlist() {
        let branch = files("[forge]\n", Some(ALLOW));
        let err = decide(&branch, &branch).unwrap_err().to_string();
        assert!(err.contains("[contained] allowed_hosts"), "{err}");
        assert!(!err.contains("permissions.allow"), "{err}");
        assert!(err.contains("aida scaffold upgrade"), "{err}");
    }

    /// An explicit empty allowlist is the project's choice, which the
    /// scaffold never overwrites — so the refusal names the edit to make.
    #[test]
    fn bug_014_preflight_names_an_empty_allowlist_as_a_hand_fix() {
        let branch = files("[contained]\nallowed_hosts = []\n", Some(ALLOW));
        assert_eq!(branch.allowed_hosts(), AllowedHosts::Empty);
        let err = decide(&branch, &branch).unwrap_err().to_string();
        assert!(
            err.contains(
                "allowed_hosts is empty in .aida/config.toml; add github.com/api.github.com or remove the key to use the default"
            ),
            "{err}"
        );
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

    /// Equivalent and broader entries satisfy the check; narrower ones do not.
    #[test]
    fn bug_014_allow_matcher_accepts_equivalent_entries() {
        for entry in ["Bash(aida:*)", "Bash(*)", "Bash"] {
            let settings = format!(r#"{{"permissions": {{"allow": ["{entry}"]}}}}"#);
            let branch = files(HOSTS, Some(&settings));
            assert!(branch.missing_allow_entries().is_empty(), "{entry}");
        }
        for entry in ["Bash(aida show *)", "Bash(git *)", "Read(*)"] {
            assert!(!allow_entry_covers(entry, "Bash(aida *)"), "{entry}");
        }
        assert!(allow_entry_covers(" Bash(aida *) ", "Bash(aida *)"));
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

    /// End to end against git: the PR's base branch has the config committed,
    /// the PR head predates it, and the primary checkout's working tree is
    /// something else entirely. The preflight reads the head and the base
    /// ref, and leaves no private refs behind.
    #[test]
    fn bug_014_preflight_reads_the_pr_branch_and_its_base_ref() {
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
        git(origin.path(), &["init", "--quiet", "-b", "release"]);
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
        // The primary checkout's working copy is irrelevant to the decision.
        std::fs::write(clone.path().join(".aida/config.toml"), "[forge]\n").unwrap();

        let run = |base: Option<&str>| {
            preflight_contained_reviewer_against(clone.path(), crate::ReviewForge::GitHub, 7, base)
        };
        let err = run(Some("release")).unwrap_err().to_string();
        assert!(
            err.contains("merge release into the PR branch first"),
            "{err}"
        );
        let refs = crate::test_git::git_command()
            .arg("-C")
            .arg(clone.path())
            .args(["for-each-ref", "refs/aida/"])
            .output()
            .unwrap();
        assert!(refs.stdout.is_empty(), "private refs left behind");

        git(
            origin.path(),
            &["update-ref", "refs/pull/7/head", "release"],
        );
        run(Some("release")).unwrap();
    }

    /// A repository whose HEAD commits `files` (path, content) and `links`
    /// (path, link text, as tree mode 120000 — no filesystem symlinks, so it
    /// runs on every platform).
    fn repo_with_links(files: &[(&str, &str)], links: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str], stdin: Option<&str>| -> String {
            use std::io::Write as _;
            let mut child = crate::test_git::git_command()
                .arg("-C")
                .arg(dir.path())
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(stdin.unwrap_or("").as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        git(&["init", "--quiet"], None);
        for (path, content) in files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, content).unwrap();
            git(&["add", path], None);
        }
        for (path, target) in links {
            let oid = git(&["hash-object", "-w", "--stdin"], Some(target));
            git(
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("120000,{oid},{path}"),
                ],
                None,
            );
        }
        git(&["commit", "--quiet", "-m", "c"], None);
        dir
    }

    /// The template-mirror layout: `.claude/settings.json` (and the config)
    /// are committed symlinks into a template directory. The preflight reads
    /// the target, not the link text.
    #[test]
    fn bug_016_preflight_follows_symlinked_config_files() {
        let links = [
            (SETTINGS_PATH, "../templates/settings.json"),
            (CONFIG_PATH, "../templates/./config.toml"),
        ];
        let ok = repo_with_links(
            &[
                ("templates/settings.json", ALLOW),
                ("templates/config.toml", HOSTS),
            ],
            &links,
        );
        let files = ReviewerConfigFiles::at_revision(ok.path(), "HEAD");
        assert!(files.broken_links.is_empty(), "{:?}", files.broken_links);
        contained_reviewer_preflight(7, &files, None).unwrap();

        let lacking = repo_with_links(
            &[
                (
                    "templates/settings.json",
                    r#"{"permissions": {"allow": ["Bash(aida)"]}}"#,
                ),
                ("templates/config.toml", HOSTS),
            ],
            &links,
        );
        let files = ReviewerConfigFiles::at_revision(lacking.path(), "HEAD");
        let err = contained_reviewer_preflight(7, &files, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("lacks the reviewer allow entries `Bash(aida *)`"),
            "{err}"
        );

        // A link to a link is followed too.
        let chained = repo_with_links(
            &[
                ("templates/settings.json", ALLOW),
                ("templates/config.toml", HOSTS),
            ],
            &[
                (SETTINGS_PATH, "settings.mirror.json"),
                (".claude/settings.mirror.json", "../templates/settings.json"),
                (CONFIG_PATH, "../templates/config.toml"),
            ],
        );
        let files = ReviewerConfigFiles::at_revision(chained.path(), "HEAD");
        contained_reviewer_preflight(7, &files, None).unwrap();
    }

    /// A dangling, escaping, absolute, or cyclic link is named as such, not
    /// reported as a file that lacks the allow entries.
    #[test]
    fn bug_016_preflight_names_a_broken_settings_link() {
        let cases: [(&[(&str, &str)], &str); 4] = [
            (
                &[(SETTINGS_PATH, "../templates/settings.json")],
                "`.claude/settings.json` is a symlink to `templates/settings.json`, which does not exist",
            ),
            (
                &[(SETTINGS_PATH, "../../outside/settings.json")],
                "`.claude/settings.json` is a symlink to `../../outside/settings.json`, which points outside the repository",
            ),
            (
                &[(SETTINGS_PATH, "/etc/settings.json")],
                "`.claude/settings.json` is a symlink to `/etc/settings.json`, which points outside the repository",
            ),
            (
                &[(SETTINGS_PATH, "a.json"), (".claude/a.json", "settings.json")],
                "`.claude/settings.json` is a chain of more than 8 symlinks",
            ),
        ];
        for (links, expected) in cases {
            let dir = repo_with_links(&[(CONFIG_PATH, HOSTS)], links);
            let files = ReviewerConfigFiles::at_revision(dir.path(), "HEAD");
            assert!(files.settings.is_none());
            let err = contained_reviewer_preflight(7, &files, Some(("main", &files)))
                .unwrap_err()
                .to_string();
            assert!(err.contains(expected), "{err}");
            assert!(err.contains("PR-7"), "{err}");
            assert!(!err.contains("lacks the reviewer allow entries"), "{err}");
        }
    }

    #[test]
    fn bug_016_link_targets_resolve_against_the_link_directory() {
        let cases = [
            (
                ".claude/settings.json",
                "../aida-core/templates/settings.json",
                Some("aida-core/templates/settings.json"),
            ),
            (
                ".claude/settings.json",
                "./local.json",
                Some(".claude/local.json"),
            ),
            (
                ".claude/settings.json",
                "a//b/../c.json",
                Some(".claude/a/c.json"),
            ),
            ("top.json", "other.json", Some("other.json")),
            (".claude/settings.json", "../..", None),
            (".claude/settings.json", "../../x.json", None),
            (".claude/settings.json", "/abs.json", None),
            (".claude/settings.json", "..", None),
        ];
        for (link, target, expected) in cases {
            assert_eq!(
                resolve_link_target(link, target).as_deref(),
                expected,
                "{link} -> {target}"
            );
        }
    }

    /// A fresh scaffold passes: the init config section and the scaffolded
    /// reviewer entries hold what the preflight requires.
    #[test]
    fn bug_014_init_scaffold_satisfies_the_preflight() {
        let settings = serde_json::json!({
            "permissions": {"allow": aida_core::scaffolding::REVIEWER_ALLOW_ENTRIES}
        })
        .to_string();
        let scaffold = files(&crate::init_contained_config_section(), Some(&settings));
        contained_reviewer_preflight(7, &scaffold, None).unwrap();
    }

    /// The refresh/upgrade config steps add the template hosts and the
    /// reviewer entries where absent, and leave a project's own choices.
    #[test]
    fn bug_014_ensure_steps_fill_only_what_is_absent() {
        let repo = |config: &str, settings: Option<&str>| {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
            std::fs::write(dir.path().join(".aida/config.toml"), config).unwrap();
            if let Some(body) = settings {
                std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
                std::fs::write(dir.path().join(".claude/settings.json"), body).unwrap();
            }
            dir
        };
        let read = |dir: &Path| ReviewerConfigFiles {
            settings: std::fs::read_to_string(dir.join(SETTINGS_PATH)).ok(),
            config: std::fs::read_to_string(dir.join(CONFIG_PATH)).ok(),
            broken_links: Vec::new(),
        };
        let dir = repo(
            "[forge]\nprovider = \"github\"\n",
            Some(r#"{"hooks": {}, "permissions": {"allow": ["Bash(make *)"]}}"#),
        );
        assert!(crate::ensure_contained_allowed_hosts(dir.path(), true).unwrap());
        assert!(crate::ensure_reviewer_allow_entries(dir.path(), true).unwrap());
        assert!(!read(dir.path()).complete(), "dry run writes nothing");
        assert!(crate::ensure_contained_allowed_hosts(dir.path(), false).unwrap());
        assert!(crate::ensure_reviewer_allow_entries(dir.path(), false).unwrap());
        let after = read(dir.path());
        assert!(after.complete());
        assert!(after.settings.unwrap().contains("Bash(make *)"));
        assert_eq!(
            crate::session::contained_allowed_hosts(dir.path()),
            aida_core::scaffolding::template_contained_allowed_hosts()
        );
        assert!(!crate::ensure_contained_allowed_hosts(dir.path(), false).unwrap());
        assert!(!crate::ensure_reviewer_allow_entries(dir.path(), false).unwrap());

        let own = repo("[contained]\nallowed_hosts = []\n", None);
        assert!(!crate::ensure_contained_allowed_hosts(own.path(), false).unwrap());
        assert_eq!(read(own.path()).allowed_hosts(), AllowedHosts::Empty);
    }
}
