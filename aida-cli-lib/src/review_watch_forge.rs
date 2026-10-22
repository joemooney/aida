//! Pinned, strict discovery for the review watcher, and the pinned
//! commit-keyed CI read its readiness report takes.
// trace:TASK-1945 trace:TASK-1993 | ai:codex
use super::*;
use crate::forge::ForgeKind;

#[derive(Debug, Clone)]
pub(super) struct Change {
    pub id: u64,
    pub head: String,
    // The change's own target branch; overlap is judged against it, as the
    // ordinary stale-base preflight does. trace:TASK-1993 | ai:claude
    pub base: String,
    pub author: String,
    pub gitlab: bool,
    pub overlap: Option<bool>,
    // Commits the head is behind its base, from the same probe as `overlap`.
    // trace:TASK-1945 | ai:claude
    pub behind: Option<u64>,
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

// Apply the same repository-routing hygiene to probes and both child levels.
// trace:TASK-1993 | ai:codex
fn clear_routing(command: &mut Command) {
    for name in [
        "GH_REPO",
        "GLAB_REPO",
        "GITLAB_REPO",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_NAMESPACE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "AIDA_TEST_GH_BINARY",
        "AIDA_TEST_GLAB_BINARY",
    ] {
        command.env_remove(name);
    }
}

fn output(root: &Path, program: &str, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new(program);
    command.current_dir(root).args(args).stdin(Stdio::null());
    clear_routing(&mut command);
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
// Resolve the initial root with the same bounded, sanitized Git probes as all
// later operations. Ambient Git routing cannot select another repository.
// trace:TASK-1993 | ai:codex
pub(super) fn main_root(start: &Path) -> Result<PathBuf> {
    let root = PathBuf::from(git(start, &["rev-parse", "--show-toplevel"])?.trim());
    match crate::seat_clone::main_for(&root) {
        crate::seat_clone::MainFor::Verified(main) => return Ok(main.canonicalize()?),
        crate::seat_clone::MainFor::Unverified(clone) => {
            return Err(crate::seat_clone::unverified_refusal(&clone))
        }
        crate::seat_clone::MainFor::NotAClone => {}
    }
    let listing = git(&root, &["worktree", "list", "--porcelain", "-z"])?;
    let main = listing
        .split('\0')
        .next()
        .and_then(|s| s.strip_prefix("worktree "))
        .filter(|s| !s.is_empty())
        .context("cannot resolve review-watch main worktree")?;
    Ok(PathBuf::from(main).canonicalize()?)
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
    pub fn pin_command(&self, command: &mut Command) {
        clear_routing(command);
        command
            .env("GH_HOST", &self.host)
            .env("GITLAB_HOST", &self.host);
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
            // Resolve through PATH at the bounded spawn. The ordinary resolver's
            // sanity `--version` probe is unbounded and unsuitable for a watcher.
            program = "gh";
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
                "number,headRefOid,baseRefName,isDraft,state,author",
            ]);
        } else {
            program = "glab";
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
        command.current_dir(root).stdin(Stdio::null());
        self.pin_command(&mut command);
        let out = crate::command_output_with_timeout(command, std::time::Duration::from_secs(30))
            .with_context(|| {
            format!("{program} discovery timed out or failed to spawn; ensure {program} is on PATH")
        })?;
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
        let (overlap, behind) = self.overlap(root, change)?;
        change.overlap = Some(overlap);
        change.behind = Some(behind);
        Ok(())
    }
    /// Every CI check on `sha`, through the shared commit-keyed reader,
    /// pinned to this repository like discovery. Any failure is an error.
    // trace:TASK-1945 | ai:claude
    pub fn commit_checks(&self, root: &Path, sha: &str) -> Result<crate::forge::CommitChecks> {
        anyhow::ensure!(valid_sha(sha), "invalid commit for a CI read");
        anyhow::ensure!(
            Self::load(root)?.identity == self.identity,
            "origin identity changed during the CI read"
        );
        let gitlab = self.kind == ForgeKind::GitLab;
        let program = if gitlab { "glab" } else { "gh" };
        let mut api = |endpoint: &str| -> Result<Vec<u8>> {
            let mut command = Command::new(program);
            command
                .args(["api", "-X", "GET", endpoint])
                .current_dir(root)
                .stdin(Stdio::null());
            self.pin_command(&mut command);
            let out =
                crate::command_output_with_timeout(command, std::time::Duration::from_secs(30))
                    .with_context(|| format!("{program} CI read timed out or failed to spawn"))?;
            anyhow::ensure!(
                out.status.success(),
                "{program} CI read failed ({}): {}",
                out.status,
                bounded_diagnostic(String::from_utf8_lossy(&out.stderr).trim())
            );
            anyhow::ensure!(
                out.stdout.len() as u64 <= LIMIT,
                "CI read output exceeds size limit"
            );
            Ok(out.stdout)
        };
        if gitlab {
            crate::forge::gitlab_commit_checks(&self.project, sha, &mut api)
        } else {
            crate::forge::github_commit_checks(&self.project, sha, &mut api)
        }
    }
    /// Fetch by remote name, never by URL: a stored `user:token@` origin then
    /// reaches git only through the credential gate's environment, not argv
    /// (BUG-2181). The pinned identity is rechecked first.
    // trace:TASK-1993 | ai:claude
    fn fetch(&self, root: &Path, refspecs: &[&str]) -> Result<()> {
        use aida_core::git_ops::RemoteCredentialsExt as _;
        anyhow::ensure!(
            Self::load(root)?.identity == self.identity,
            "origin identity changed during base probe"
        );
        let mut command = Command::new("git");
        command
            .current_dir(root)
            .args([
                "fetch",
                "--no-write-fetch-head",
                "--no-tags",
                "--",
                "origin",
            ])
            .args(refspecs)
            .stdin(Stdio::null());
        clear_routing(&mut command);
        command.remote_credentials(root, "origin");
        let out = crate::command_output_with_timeout(command, std::time::Duration::from_secs(30))
            .context("review-watch git fetch failed or exceeded 30 seconds")?;
        anyhow::ensure!(
            out.status.success(),
            "review-watch git fetch failed ({})",
            out.status
        );
        Ok(())
    }
    fn overlap(&self, root: &Path, change: &Change) -> Result<(bool, u64)> {
        let branch = change.base.as_str();
        git(root, &["check-ref-format", &format!("refs/heads/{branch}")])
            .context("change base branch is not a valid branch name")?;
        // The probing process's pid leads the ref name, so a sweep can tell a
        // leaked ref (its prober gone) from one a live watcher is using.
        // trace:TASK-1955 | ai:claude (TASK-1-413)
        let prefix = format!(
            "{PROBE_REF_PREFIX}{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        );
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
            self.fetch(root, &[&base_spec, &head_spec])?;
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
                return Ok((false, 0));
            }
            let ancestor = git(root, &["merge-base", &base, &head])?;
            let ancestor = ancestor.trim();
            anyhow::ensure!(valid_sha(ancestor), "invalid merge-base evidence");
            let names = |tip: &str| -> Result<BTreeSet<String>> {
                let bytes = output(
                    root,
                    "git",
                    &[
                        "diff",
                        "--no-renames",
                        "--name-only",
                        "-z",
                        ancestor,
                        tip,
                        "--",
                    ],
                )?;
                let text = String::from_utf8(bytes).context("non-UTF8 path evidence")?;
                Ok(text
                    .split('\0')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect())
            };
            Ok((paths_overlap(&names(&base)?, &names(&head)?), behind))
        })();
        // These UUID refs belong only to this probe, never another watcher.
        let _ = output(root, "git", &["update-ref", "-d", &base]);
        let _ = output(root, "git", &["update-ref", "-d", &head]);
        result
    }
}

const PROBE_REF_PREFIX: &str = "refs/aida/review-watch/";

/// Delete probe refs left by a watcher killed between its fetch and its
/// `update-ref -d`: every `refs/aida/review-watch/<pid>-<uuid>/*` whose pid
/// is not alive, and any older pid-less one. Returns how many were removed.
// trace:TASK-1955 | ai:claude (TASK-1-413)
pub(super) fn sweep_probe_refs(root: &Path) -> usize {
    let Ok(listing) = git(
        root,
        &["for-each-ref", "--format=%(refname)", PROBE_REF_PREFIX],
    ) else {
        return 0;
    };
    let mut removed = 0;
    for name in listing.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let owner = name
            .strip_prefix(PROBE_REF_PREFIX)
            .and_then(|rest| rest.split('/').next())
            .and_then(|probe| probe.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<u32>().ok());
        if owner.is_some_and(aida_core::liveness::pid_is_alive) {
            continue;
        }
        if git(root, &["update-ref", "-d", name]).is_ok() {
            removed += 1;
        }
    }
    removed
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
    let base = value
        .get(if gitlab {
            "target_branch"
        } else {
            "baseRefName"
        })
        .and_then(|v| v.as_str())
        .filter(|b| !b.is_empty() && b.len() <= 255 && !b.starts_with('-'))
        .context("missing change base branch")?
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
        base,
        author,
        gitlab,
        overlap: None,
        behind: None,
    }))
}

// A path and its descendant also conflict (file/directory replacement).
// trace:TASK-1993 | ai:codex
pub(super) fn paths_overlap(left: &BTreeSet<String>, right: &BTreeSet<String>) -> bool {
    let descendant = |paths: &BTreeSet<String>, name: &str| {
        let prefix = format!("{name}/");
        paths
            .range(prefix.clone()..)
            .next()
            .is_some_and(|path| path.starts_with(&prefix))
    };
    left.iter()
        .any(|name| right.contains(name) || descendant(right, name))
        || right.iter().any(|name| descendant(left, name))
}
