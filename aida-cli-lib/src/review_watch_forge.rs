//! Pinned, strict discovery for the review watcher. No CI queries occur here.
// trace:TASK-1945 trace:TASK-1993 | ai:codex
use super::*;
use crate::forge::ForgeKind;

#[derive(Debug, Clone)]
pub(super) struct Change {
    pub id: u64,
    pub head: String,
    pub author: String,
    pub gitlab: bool,
    pub overlap: Option<bool>,
}
impl Change {
    pub fn key(&self) -> String {
        format!("{}-{}", if self.gitlab { "MR" } else { "PR" }, self.id)
    }
}
pub(super) struct Repository {
    pub identity: String,
    kind: ForgeKind,
    host: String,
    project: String,
    remote: String,
}

fn output(root: &Path, program: &str, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new(program);
    command
        .current_dir(root)
        .args(args)
        .stdin(Stdio::null())
        .env_remove("GH_REPO")
        .env_remove("GLAB_REPO")
        .env_remove("GITLAB_REPO")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE");
    let out = crate::command_output_with_timeout(command, std::time::Duration::from_secs(30))
        .with_context(|| format!("review-watch {program} probe failed or exceeded 30 seconds"))?;
    anyhow::ensure!(
        out.status.success(),
        "review-watch {program} probe failed ({})",
        out.status
    );
    anyhow::ensure!(
        out.stdout.len() as u64 <= LIMIT,
        "review-watch probe output exceeds size limit"
    );
    Ok(out.stdout)
}
fn git(root: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(output(root, "git", args)?).context("non-UTF8 Git probe output")
}
impl Repository {
    pub fn load(root: &Path) -> Result<Self> {
        let remote = git(root, &["remote", "get-url", "origin"])?;
        let remote = remote.trim().to_string();
        let kind = crate::forge::detect_forge_kind(&remote);
        anyhow::ensure!(
            matches!(kind, ForgeKind::GitHub | ForgeKind::GitLab),
            "review-watch needs a recognized GitHub or GitLab origin"
        );
        let host = crate::forge::forge_host_of(&remote).context("cannot pin forge host")?;
        let project = crate::forge::project_path_of(&remote).context("cannot pin forge project")?;
        anyhow::ensure!(
            project.split('/').count() >= 2
                && project.split('/').all(|part| !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))),
            "unsupported forge project path"
        );
        // SSH/API port mappings need explicit forge support; do not silently
        // query a different endpoint than the origin establishes.
        if let Some((scheme, rest)) = remote.split_once("://") {
            let authority = rest
                .split('/')
                .next()
                .unwrap_or("")
                .rsplit('@')
                .next()
                .unwrap_or("");
            anyhow::ensure!(
                !authority.contains(':') || (scheme == "ssh" && authority.ends_with(":22")),
                "review-watch cannot pin an explicit custom origin port"
            );
        }
        Ok(Self {
            identity: format!(
                "{}:{host}/{project}",
                if kind == ForgeKind::GitLab {
                    "gitlab"
                } else {
                    "github"
                }
            ),
            kind,
            host,
            project,
            remote,
        })
    }
    fn json(&self, root: &Path, id: Option<u64>, page: u32) -> Result<serde_json::Value> {
        let pinned = Self::load(root)?;
        anyhow::ensure!(
            pinned.identity == self.identity,
            "origin identity changed during review-watch"
        );
        let mut command;
        let program;
        if self.kind == ForgeKind::GitHub {
            program = crate::resolve_forge_cli(self.kind).context("GitHub CLI is unavailable")?;
            command = Command::new(program);
            command
                .env("GH_HOST", &self.host)
                .args(["pr", if id.is_some() { "view" } else { "list" }]);
            if let Some(id) = id {
                command.arg(id.to_string());
            } else {
                command.args(["--state", "open", "--limit", "1000"]);
            }
            command.args([
                "--repo",
                &format!("{}/{}", self.host, self.project),
                "--json",
                "number,headRefOid,isDraft,state,author",
            ]);
        } else {
            program = crate::resolve_forge_cli(self.kind).context("GitLab CLI is unavailable")?;
            command = Command::new(program);
            command
                .env("GITLAB_HOST", &self.host)
                .args(["api", "-X", "GET"]);
            let mut endpoint = format!(
                "projects/{}/merge_requests",
                self.project.replace('/', "%2F")
            );
            if let Some(id) = id {
                endpoint.push_str(&format!("/{id}"));
            }
            command.arg(endpoint);
            if id.is_none() {
                command.args([
                    "-f",
                    "state=opened",
                    "-f",
                    "per_page=100",
                    "-f",
                    &format!("page={page}"),
                ]);
            }
        }
        command
            .current_dir(root)
            .stdin(Stdio::null())
            .env_remove("GH_REPO")
            .env_remove("GLAB_REPO")
            .env_remove("GITLAB_REPO");
        let out = crate::command_output_with_timeout(command, std::time::Duration::from_secs(30))
            .context("forge discovery timed out or failed to spawn")?;
        anyhow::ensure!(
            out.status.success(),
            "forge discovery failed ({})",
            out.status
        );
        anyhow::ensure!(
            out.stdout.len() as u64 <= LIMIT,
            "forge discovery output exceeds size limit"
        );
        serde_json::from_slice(&out.stdout).context("invalid forge discovery JSON")
    }
    pub fn discover(&self, root: &Path) -> Result<Vec<Change>> {
        let mut changes = Vec::new();
        let mut ids = BTreeSet::new();
        for page in 1..=10 {
            let value = self.json(root, None, page)?;
            let rows = value
                .as_array()
                .context("forge discovery did not return an array")?;
            let cap = if self.kind == ForgeKind::GitLab {
                100
            } else {
                1000
            };
            for row in rows {
                if let Some(change) = parse_change(row, self.kind == ForgeKind::GitLab)? {
                    anyhow::ensure!(ids.insert(change.id), "duplicate change in forge discovery");
                    changes.push(change);
                }
            }
            if rows.len() < cap {
                return Ok(changes);
            }
            anyhow::ensure!(
                self.kind == ForgeKind::GitLab && page < 10,
                "forge discovery reached its bounded page limit; no reviews dispatched"
            );
        }
        anyhow::bail!("incomplete forge discovery")
    }
    pub fn change(&self, root: &Path, id: u64) -> Result<Change> {
        let value = self.json(root, Some(id), 1)?;
        let change = parse_change(&value, self.kind == ForgeKind::GitLab)?
            .context("change is closed or draft; no review started")?;
        anyhow::ensure!(change.id == id, "forge returned the wrong change identity");
        Ok(change)
    }
    pub fn probe(&self, root: &Path, change: &mut Change) -> Result<()> {
        anyhow::ensure!(
            Self::load(root)?.identity == self.identity,
            "origin identity changed during base probe"
        );
        change.overlap = Some(self.overlap(root, change)?);
        Ok(())
    }
    fn overlap(&self, root: &Path, change: &Change) -> Result<bool> {
        let default = git(
            root,
            &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
        )?;
        let branch = default
            .trim()
            .strip_prefix("refs/remotes/origin/")
            .context("default branch is unknown; set origin/HEAD before review-watch")?;
        git(root, &["check-ref-format", &format!("refs/heads/{branch}")])?;
        let prefix = format!("refs/aida/review-watch/{}", uuid::Uuid::new_v4());
        let base = format!("{prefix}/base");
        let head = format!("{prefix}/head");
        let base_spec = format!("+refs/heads/{branch}:{base}");
        let remote_head = if change.gitlab {
            format!("refs/merge-requests/{}/head", change.id)
        } else {
            format!("refs/pull/{}/head", change.id)
        };
        let head_spec = format!("+{remote_head}:{head}");
        let result = (|| {
            output(
                root,
                "git",
                &[
                    "fetch",
                    "--no-write-fetch-head",
                    "--no-tags",
                    "--",
                    &self.remote,
                    &base_spec,
                    &head_spec,
                ],
            )?;
            let resolved = git(root, &["rev-parse", "--verify", &head])?;
            anyhow::ensure!(
                resolved.trim() == change.head,
                "change head moved during discovery; retry the next pass"
            );
            let behind = git(root, &["rev-list", "--count", &format!("{head}..{base}")])?
                .trim()
                .parse::<u64>()
                .context("invalid behind count")?;
            if behind == 0 {
                return Ok(false);
            }
            let ancestor = git(root, &["merge-base", &base, &head])?;
            let ancestor = ancestor.trim();
            anyhow::ensure!(valid_sha(ancestor), "invalid merge-base evidence");
            let names = |tip: &str| -> Result<BTreeSet<String>> {
                let bytes = output(
                    root,
                    "git",
                    &["diff", "--name-only", "-z", ancestor, tip, "--"],
                )?;
                let text = String::from_utf8(bytes).context("non-UTF8 path evidence")?;
                Ok(text
                    .split('\0')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect())
            };
            Ok(!names(&base)?.is_disjoint(&names(&head)?))
        })();
        // These UUID refs belong only to this probe, never another watcher.
        let _ = output(root, "git", &["update-ref", "-d", &base]);
        let _ = output(root, "git", &["update-ref", "-d", &head]);
        result
    }
}

pub(super) fn parse_change(value: &serde_json::Value, gitlab: bool) -> Result<Option<Change>> {
    let state = value
        .get("state")
        .and_then(|s| s.as_str())
        .context("missing change state")?;
    anyhow::ensure!(
        if gitlab {
            matches!(state, "opened" | "closed" | "merged")
        } else {
            matches!(state, "OPEN" | "CLOSED" | "MERGED")
        },
        "unknown change state"
    );
    if gitlab {
        if let (Some(a), Some(b)) = (
            value.get("draft").and_then(|v| v.as_bool()),
            value.get("work_in_progress").and_then(|v| v.as_bool()),
        ) {
            anyhow::ensure!(a == b, "conflicting GitLab draft evidence");
        }
    }
    let draft = value
        .get(if gitlab { "draft" } else { "isDraft" })
        .or_else(|| gitlab.then(|| value.get("work_in_progress")).flatten())
        .and_then(|v| v.as_bool())
        .context("missing draft evidence")?;
    if draft || state != if gitlab { "opened" } else { "OPEN" } {
        return Ok(None);
    }
    let id = value
        .get(if gitlab { "iid" } else { "number" })
        .and_then(|v| v.as_u64())
        .filter(|n| *n > 0)
        .context("missing change number")?;
    let head = value
        .get(if gitlab { "sha" } else { "headRefOid" })
        .and_then(|v| v.as_str())
        .filter(|h| valid_sha(h))
        .context("missing valid change head")?
        .to_string();
    let author = value
        .get("author")
        .and_then(|v| v.get(if gitlab { "username" } else { "login" }))
        .and_then(|v| v.as_str())
        .filter(|a| !a.is_empty() && a.len() <= 256)
        .context("missing change author")?
        .to_string();
    anyhow::ensure!(
        aida_core::mailbox::recipient_id_rejection(&author).is_none(),
        "change author is not a valid mailbox identity"
    );
    Ok(Some(Change {
        id,
        head,
        author,
        gitlab,
        overlap: None,
    }))
}
