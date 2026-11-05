//! Independent, clone-local observation transport. Envelope prose is data only.
// trace:TASK-3096 | ai:codex
use crate::observation_envelope::{self as envelope, Envelope, ENVELOPE_LIMIT};
use aida_core::transport_policy::{self as transport_process, Private};
use anyhow::{bail, Context, Result};
use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const BRANCH: &str = "refs/heads/aida-intent";
const INCOMING: &str = "refs/intake/incoming";
const PROFILE: &str = "profile.json";
const REPO_CONFIG: &[u8] = b"[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n\tlogallrefupdates = true\n";
const CAP: usize = 8 * 1024 * 1024;

#[derive(Debug, Subcommand)]
pub(crate) enum IntakeCommand {
    /// Enroll an independent pinned URL and reviewed deny patterns; no store required.
    Enroll {
        /// Explicit absolute local path, HTTPS URL, or SSH URL.
        #[clap(long)]
        remote: String,
        /// Confirm review of the destination and policy, including an empty policy.
        #[clap(long, required = true)]
        reviewed: bool,
        /// Reviewed regex refusing matching title or body text (repeatable).
        #[clap(long)]
        deny_pattern: Vec<String>,
        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
    /// Commit a reviewed envelope and push only aida-intent; never force.
    Publish {
        /// Exact envelope JSON; omit to publish retained commits and receipts.
        #[clap(long)]
        file: Option<PathBuf>,
        /// Required when adding an envelope. Omit --file to retry a pending push.
        #[clap(long, requires = "file")]
        reviewed: bool,
        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
    /// Fetch into quarantine, validate all history, then accept only a fast-forward.
    Pull {
        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
    /// List pending observations, or include processed observations with --all.
    List {
        /// Include processed observations.
        #[clap(long)]
        all: bool,
        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
    /// Explicitly keep pending, dismiss, create a draft task, or comment on a spec.
    Process {
        /// Full SHA-256 digest (not the abbreviated human label).
        checksum: String,
        /// Explicit processing decision.
        #[clap(long, value_enum)]
        action: Action,
        /// Existing destination spec for --action comment.
        #[clap(long)]
        target: Option<String>,
        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Action {
    Pending,
    Dismiss,
    Create,
    Comment,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    remote: String,
    deny_patterns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    checksum: String,
    action: Action,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    checksum: String,
    action: Action,
    target: Option<String>,
    /// Random, durable identity, never inferred from imported text.
    operation: String,
    /// Bind retries to the exact destination store, not just a display ID.
    store: PathBuf,
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// Remove ambient Git routing/configuration. Never inherit product indexes,
// alternates, replacement objects, URL rewrites, hooks or extra push refspecs.
fn git_command(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    cmd.current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .env("SSH_ASKPASS", "/bin/false")
        .env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_AUTHOR_NAME", "AIDA Intake")
        .env("GIT_AUTHOR_EMAIL", "intake@invalid")
        .env("GIT_COMMITTER_NAME", "AIDA Intake")
        .env("GIT_COMMITTER_EMAIL", "intake@invalid")
        .args(aida_core::git_ops::NO_AUTO_MAINTENANCE)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "push.followTags=false",
        ]);
    cmd
}

fn git_result(root: &Path, args: &[&str], input: &[u8], cap: usize) -> Result<(bool, Vec<u8>)> {
    let mut cmd = git_command(root);
    cmd.args(args);
    let (status, bytes) =
        transport_process::bounded(cmd, input, cap, Instant::now() + Duration::from_secs(30))?;
    Ok((status.success(), bytes))
}

fn git(root: &Path, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
    let (ok, bytes) = git_result(root, args, input, CAP)?;
    if !ok {
        bail!("Intake Git operation refused ({}); inspect the independent intake repository; no force/rebase is performed", args[0]);
    }
    Ok(bytes)
}

fn text(bytes: Vec<u8>) -> Result<String> {
    Ok(String::from_utf8(bytes)?.trim().to_string())
}

struct Intake {
    private: Private,
    worktree: PathBuf,
    profile: Profile,
    _lock: fs::File,
}

impl Intake {
    fn open(command: &IntakeCommand) -> Result<Self> {
        let cwd = std::env::current_dir()?;
        let common = PathBuf::from(text(git(
            &cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            &[],
        )?)?);
        let root = common.join("aida-intake");
        match fs::DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let private = Private::open(&root)?;
        // This permanent sidecar is in its own directory, separate from the
        // existing store transport's identically named permanent lock.
        let lock = private.lock()?;
        let profile = if let IntakeCommand::Enroll {
            remote,
            reviewed,
            deny_pattern,
            ..
        } = command
        {
            if !reviewed {
                bail!("Enrollment requires --reviewed");
            }
            if remote.is_empty()
                || remote.starts_with('-')
                || remote.chars().any(char::is_control)
                || remote.contains("::")
                || !(remote.starts_with("https://")
                    || remote.starts_with("ssh://")
                    || remote.starts_with('/'))
            {
                bail!("Use an explicit absolute local path, https:// URL or ssh:// URL; remote names and helper protocols refuse");
            }
            envelope::check_patterns("", "", deny_pattern)?;
            let proposed = Profile {
                version: 1,
                remote: remote.clone(),
                deny_patterns: deny_pattern.clone(),
            };
            if let Some(old) = private.read(PROFILE)? {
                if old != serde_json::to_vec(&proposed)? {
                    bail!("Intake already enrolled; destination/policy changes require a separate clone");
                }
            }
            private.write(PROFILE, &serde_json::to_vec(&proposed)?)?;
            proposed
        } else {
            serde_json::from_slice::<Profile>(&private.read(PROFILE)?.context(
                "Intake is not enrolled; use aida intent enroll --remote ... --reviewed",
            )?)?
        };
        if profile.remote.is_empty()
            || profile.remote.starts_with('-')
            || profile.remote.chars().any(char::is_control)
            || profile.remote.contains("::")
            || !(profile.remote.starts_with("https://")
                || profile.remote.starts_with("ssh://")
                || profile.remote.starts_with('/'))
        {
            bail!("Invalid pinned intake destination");
        }
        if profile.version != 1 {
            bail!("Unsupported intake policy version");
        }
        envelope::check_patterns("", "", &profile.deny_patterns)?;
        let worktree = root.join("worktree");
        if !worktree.exists() {
            // A wholly separate object database is stronger isolation than a
            // linked checkout: it cannot accidentally acquire product ancestry.
            git(
                &root,
                &[
                    "init",
                    "--quiet",
                    "--template=",
                    "--initial-branch=aida-intent",
                    "worktree",
                ],
                &[],
            )?;
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                worktree.join(".git/config"),
                fs::Permissions::from_mode(0o600),
            )?;
            Private::open(&worktree.join(".git"))?.write("config", REPO_CONFIG)?;
        }
        let meta = fs::symlink_metadata(&worktree)?;
        let git_meta = fs::symlink_metadata(worktree.join(".git"))?;
        if !meta.is_dir() || !git_meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
            bail!("Unsafe intake worktree");
        }
        let repo_private = Private::open(&worktree.join(".git"))?;
        if repo_private.read("config")?.as_deref() != Some(REPO_CONFIG) {
            bail!("Intake Git configuration changed; refusing routing or helper overrides");
        }
        for name in ["objects", "refs"] {
            let meta = fs::symlink_metadata(worktree.join(".git").join(name))?;
            if !meta.is_dir()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o022 != 0
            {
                bail!("Unsafe intake object or reference directory");
            }
        }
        for forbidden in [
            "commondir",
            "objects/info/alternates",
            "shallow",
            "info/grafts",
        ] {
            if fs::symlink_metadata(worktree.join(".git").join(forbidden)).is_ok() {
                bail!("Intake repository must have an independent complete object database");
            }
        }
        Ok(Self {
            private,
            worktree,
            profile,
            _lock: lock,
        })
    }

    fn tip(&self) -> Result<Option<String>> {
        let (ok, bytes) = git_result(&self.worktree, &["rev-parse", "--verify", BRANCH], &[], 128)?;
        if ok {
            Ok(Some(text(bytes)?))
        } else {
            Ok(None)
        }
    }

    fn blob(&self, oid: &str, limit: usize) -> Result<Vec<u8>> {
        let size: usize = text(git(&self.worktree, &["cat-file", "-s", oid], &[])?)?.parse()?;
        if size > limit {
            bail!("Intake object exceeds size limit");
        }
        let (ok, bytes) = git_result(&self.worktree, &["cat-file", "blob", oid], &[], limit)?;
        if !ok {
            bail!("Unreadable intake blob");
        }
        Ok(bytes)
    }

    /// Validate every reachable commit, including intermediate trees. Linear,
    /// additive history only; exact immutable bytes for observations/receipts.
    fn history(&self, tip: &str) -> Result<BTreeMap<String, Vec<u8>>> {
        let commits = text(git(
            &self.worktree,
            &["rev-list", "--reverse", "--topo-order", "--parents", tip],
            &[],
        )?)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut blobs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut read_bytes = 0usize;
        let mut previous: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut parent: Option<&str> = None;
        if commits.lines().count() > 4096 {
            bail!("Intake history exceeds 4096 commits");
        }
        for line in commits.lines() {
            if Instant::now() >= deadline {
                bail!("Intake validation deadline exceeded");
            }
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != if parent.is_some() { 2 } else { 1 }
                || parent.is_some_and(|p| fields.get(1).copied() != Some(p))
            {
                bail!("Intake ancestry must be a single orphan linear history");
            }
            let tree = git(
                &self.worktree,
                &["ls-tree", "-rtz", "--full-tree", fields[0]],
                &[],
            )?;
            let raw_commit = git(&self.worktree, &["cat-file", "commit", fields[0]], &[])?;
            let raw_commit = std::str::from_utf8(&raw_commit).context("Invalid intake commit")?;
            let (headers, message) = raw_commit
                .split_once("\n\n")
                .context("Invalid intake commit")?;
            let identity = regex::Regex::new(
                r"^(author|committer) AIDA Intake <intake@invalid> [0-9]+ [+-][0-9]{4}$",
            )?;
            let expected_headers = if parent.is_some() { 4 } else { 3 };
            if message != "Record reviewed intake data\n"
                || headers.lines().count() != expected_headers
                || !headers
                    .lines()
                    .any(|l| l.starts_with("author ") && identity.is_match(l))
                || !headers
                    .lines()
                    .any(|l| l.starts_with("committer ") && identity.is_match(l))
            {
                bail!("Unexpected intake commit metadata; source history is not transferable");
            }
            let mut total = 0usize;
            let mut current = BTreeMap::new();
            for entry in tree.split(|b| *b == 0).filter(|b| !b.is_empty()) {
                let entry = std::str::from_utf8(entry).context("Invalid intake tree")?;
                let (header, path) = entry.split_once('\t').context("Invalid intake entry")?;
                let parts: Vec<_> = header.split_whitespace().collect();
                if parts.len() == 3
                    && parts[0] == "040000"
                    && parts[1] == "tree"
                    && matches!(path, "observations" | "receipts")
                {
                    continue;
                }
                if parts.len() != 3 || parts[0] != "100644" || parts[1] != "blob" {
                    bail!("Intake admits only ordinary non-executable files");
                }
                let (folder, name) = path.split_once('/').context("Unexpected intake path")?;
                let checksum = name
                    .strip_suffix(".json")
                    .context("Unexpected intake filename")?;
                if !digest(checksum) || !matches!(folder, "observations" | "receipts") {
                    bail!("Unexpected intake path");
                }
                if Instant::now() >= deadline {
                    bail!("Intake validation deadline exceeded");
                }
                let bytes = if let Some(bytes) = blobs.get(parts[2]) {
                    bytes.clone()
                } else {
                    let bytes = self.blob(parts[2], ENVELOPE_LIMIT)?;
                    read_bytes += bytes.len();
                    if read_bytes > 32 * 1024 * 1024 {
                        bail!("Intake validation exceeds 32 MiB");
                    }
                    blobs.insert(parts[2].to_string(), bytes.clone());
                    bytes
                };
                total += bytes.len();
                if total > 32 * 1024 * 1024 {
                    bail!("Intake tree exceeds 32 MiB");
                }
                if folder == "observations" {
                    let value = envelope::parse(&bytes, &self.profile.deny_patterns)?;
                    if value.checksum != checksum {
                        bail!("Intake digest filename mismatch");
                    }
                } else {
                    let receipt: Receipt = serde_json::from_slice(&bytes)
                        .map_err(|_| anyhow::anyhow!("Invalid intake receipt schema or JSON"))?;
                    if receipt.version != 1
                        || receipt.checksum != checksum
                        || receipt.action == Action::Pending
                        || matches!(receipt.action, Action::Create | Action::Comment)
                            != receipt.destination.is_some()
                        || receipt
                            .destination
                            .as_ref()
                            .is_some_and(|d| uuid::Uuid::parse_str(d).is_err())
                    {
                        bail!("Invalid intake receipt fields");
                    }
                }
                if current.insert(path.to_string(), bytes).is_some() {
                    bail!("Duplicate intake path");
                }
                if current.len() > 8192 {
                    bail!("Intake tree exceeds file limit");
                }
            }
            if current.is_empty() {
                bail!("Empty intake commits refuse");
            }
            for (path, bytes) in &previous {
                if current.get(path) != Some(bytes) {
                    bail!("Intake files are immutable; rewrite or deletion refused");
                }
            }
            for path in current.keys().filter(|p| p.starts_with("receipts/")) {
                if !current.contains_key(&path.replacen("receipts/", "observations/", 1)) {
                    bail!("Receipt without observation refused");
                }
            }
            previous = current;
            parent = Some(fields[0]);
        }
        Ok(previous)
    }

    fn files(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        match self.tip()? {
            Some(tip) => self.history(&tip),
            None => Ok(BTreeMap::new()),
        }
    }

    fn add_file(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let tip = self.tip()?;
        let files = self.files()?;
        if let Some(old) = files.get(path) {
            if old != bytes {
                bail!("Conflicting bytes for immutable intake filename");
            }
            return Ok(());
        }
        // Plumbing uses this repository's index only; reset from the accepted
        // tree each time so an interrupted update cannot smuggle extra files.
        match &tip {
            Some(tip) => {
                git(&self.worktree, &["read-tree", tip], &[])?;
            }
            None => {
                git(&self.worktree, &["read-tree", "--empty"], &[])?;
            }
        }
        let blob = text(git(
            &self.worktree,
            &["hash-object", "-w", "--stdin"],
            bytes,
        )?)?;
        git(
            &self.worktree,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{blob},{path}"),
            ],
            &[],
        )?;
        let tree = text(git(&self.worktree, &["write-tree"], &[])?)?;
        let mut args = vec!["commit-tree", tree.as_str()];
        if let Some(tip) = &tip {
            args.extend(["-p", tip]);
        }
        let commit = text(git(
            &self.worktree,
            &args,
            b"Record reviewed intake data\n",
        )?)?;
        self.history(&commit)?;
        git(
            &self.worktree,
            &["update-ref", BRANCH, &commit, tip.as_deref().unwrap_or("")],
            &[],
        )?;
        // Materialize only validated files in the private checkout. No product
        // checkout/index/ref is ever involved. This is safely repeatable.
        git(
            &self.worktree,
            &["read-tree", "--reset", "-u", &commit],
            &[],
        )?;
        Ok(())
    }

    fn push(&self) -> Result<()> {
        let tip = self.tip()?.context("No intake commits to publish")?;
        self.history(&tip)?;
        git(&self.worktree, &["-c", "push.default=nothing", "push", "--no-verify", "--no-follow-tags", "--", &self.profile.remote, &format!("{tip}:{BRANCH}")], &[])
            .context("Intake push refused; local commit retained. Pull first; divergence requires manual review in a separate clone")?;
        Ok(())
    }

    fn pull(&self) -> Result<()> {
        // Clear rejected quarantine evidence before fetching, never the accepted ref.
        git(&self.worktree, &["update-ref", "-d", INCOMING], &[])?;
        git(
            &self.worktree,
            &[
                "fetch",
                "--no-tags",
                "--no-write-fetch-head",
                "--no-recurse-submodules",
                "--",
                &self.profile.remote,
                &format!("{BRANCH}:{INCOMING}"),
            ],
            &[],
        )?;
        let incoming = text(git(
            &self.worktree,
            &["rev-parse", "--verify", INCOMING],
            &[],
        )?)?;
        self.history(&incoming)?;
        let old = self.tip()?;
        if let Some(old) = &old {
            self.history(old)?;
            if !git_result(
                &self.worktree,
                &["merge-base", "--is-ancestor", old, &incoming],
                &[],
                128,
            )?
            .0
            {
                bail!("Intake diverged; accepted local branch unchanged. Review both histories in a separate clone; never force-push");
            }
        }
        git(
            &self.worktree,
            &[
                "update-ref",
                BRANCH,
                &incoming,
                old.as_deref().unwrap_or(""),
            ],
            &[],
        )?;
        git(
            &self.worktree,
            &["read-tree", "--reset", "-u", &incoming],
            &[],
        )?;
        Ok(())
    }
}

fn canonical_store() -> Result<(PathBuf, aida_core::RequirementsStore)> {
    let root = crate::find_project_root()?;
    let path = aida_core::store_locate::detect_distributed_store_from(&root)
        .context("Processing create/comment requires an attached git-canonical home store")?
        .canonicalize()?;
    let backend = aida_core::GitBackend::new(&path)?;
    let store = aida_core::DatabaseBackend::load(&backend)?;
    Ok((path, store))
}

fn evidence(journal: &Journal) -> Result<Option<String>> {
    let (path, store) = canonical_store()?;
    if path != journal.store {
        bail!("Processing journal belongs to a different store");
    }
    let marker = format!("intake-op:{}", journal.operation);
    let mut matches = Vec::new();
    for req in &store.requirements {
        let count = match journal.action {
            Action::Create => usize::from(req.tags.contains(&marker)),
            Action::Comment if journal.target.as_deref() == Some(req.id.to_string().as_str()) => {
                req.comments
                    .iter()
                    .filter(|comment| comment.content.ends_with(&format!("\n\n[{marker}]")))
                    .count()
            }
            _ => 0,
        };
        matches.extend(std::iter::repeat(req).take(count));
    }
    match matches.as_slice() {
        [] => Ok(None),
        [req] => Ok(Some(req.id.to_string())),
        _ => bail!("Ambiguous intake operation evidence; manual reconciliation required"),
    }
}

fn process(intake: &Intake, checksum: &str, action: Action, target: &Option<String>) -> Result<()> {
    if !digest(checksum) {
        bail!("Supply the full lowercase SHA-256 digest");
    }
    if (action == Action::Comment) != target.is_some() {
        bail!("Only --action comment requires --target");
    }
    let files = intake.files()?;
    let value: Envelope = envelope::parse(
        files
            .get(&format!("observations/{checksum}.json"))
            .context("Unknown intake observation")?,
        &intake.profile.deny_patterns,
    )?;
    let receipt_path = format!("receipts/{checksum}.json");
    let journal_name = format!("operation-{checksum}.json");
    if let Some(bytes) = files.get(&receipt_path) {
        let receipt: Receipt = serde_json::from_slice(bytes)?;
        if receipt.action != action {
            bail!("Observation already processed with another action");
        }
        if let Some(target) = target {
            let (_, store) = canonical_store()?;
            let req = store
                .get_requirement_unambiguous(target)?
                .context("Destination not found")?;
            if receipt.destination.as_deref() != Some(req.id.to_string().as_str()) {
                bail!("Observation already processed for another destination");
            }
        }
        println!("{}", serde_json::to_string(&receipt)?);
        return Ok(());
    }
    if action == Action::Pending {
        if intake.private.read(&journal_name)?.is_some() {
            bail!("Unresolved processing operation; reconcile before leaving pending");
        }
        println!(
            "{}",
            serde_json::json!({"checksum": checksum, "state": "pending"})
        );
        return Ok(());
    }
    let mut destination = None;
    if matches!(action, Action::Create | Action::Comment) {
        let (store_path, store) = canonical_store()?;
        let target = target
            .as_ref()
            .map(|id| {
                store
                    .get_requirement_unambiguous(id)?
                    .map(|r| r.id.to_string())
                    .context("Destination not found")
            })
            .transpose()?;
        let existing = intake.private.read(&journal_name)?;
        let journal = if let Some(bytes) = &existing {
            let journal: Journal = serde_json::from_slice(bytes)?;
            if journal.version != 1
                || journal.checksum != checksum
                || journal.action != action
                || journal.target != target
                || journal.store != store_path
                || uuid::Uuid::parse_str(&journal.operation).is_err()
            {
                bail!("Unresolved processing operation has different parameters");
            }
            journal
        } else {
            let journal = Journal {
                version: 1,
                checksum: checksum.into(),
                action,
                target,
                operation: uuid::Uuid::new_v4().to_string(),
                store: store_path,
            };
            intake
                .private
                .write(&journal_name, &serde_json::to_vec(&journal)?)?;
            journal
        };
        destination = evidence(&journal)?;
        if destination.is_none() {
            if existing.is_some() {
                bail!("Ambiguous prior spec write: durable journal retained; inspect intake-op evidence and reconcile manually. Refusing automatic replay");
            }
            let marker = format!("intake-op:{}", journal.operation);
            // Reuse normal CLI authorization and canonical writers, never bypass
            // role, transport, hypothesis or store mutation policy here.
            let mut cmd = Command::new(std::env::current_exe()?);
            cmd.env("AIDA_HEADLESS", "1");
            let body;
            if action == Action::Create {
                cmd.arg("add")
                    .arg(format!("--title={}", value.title))
                    .args([
                        "--description-stdin",
                        "--type",
                        "task",
                        "--status",
                        "draft",
                        "--tags",
                        &marker,
                        "--format",
                        "human",
                    ]);
                body = value.body;
            } else {
                cmd.args([
                    "comment",
                    "add",
                    journal.target.as_deref().unwrap(),
                    "--stdin",
                    "--format",
                    "human",
                ]);
                body = format!("{}\n\n{}\n\n[{marker}]", value.title, value.body);
            }
            let (status, _) = transport_process::bounded(
                cmd,
                body.as_bytes(),
                CAP,
                Instant::now() + Duration::from_secs(30),
            )?;
            destination = evidence(&journal)?;
            if !status.success() || destination.is_none() {
                bail!("Spec write failed or its success is ambiguous; processing journal retained. Retry only to recover confirmed evidence; never blindly repeat the write");
            }
        }
    } else if intake.private.read(&journal_name)?.is_some() {
        bail!("Unresolved spec write; cannot dismiss before reconciliation");
    }
    let receipt = Receipt {
        version: 1,
        checksum: checksum.into(),
        action,
        destination,
    };
    intake.add_file(&receipt_path, &serde_json::to_vec(&receipt)?)?;
    // Receipt is durable before journal cleanup; retry after either crash is safe.
    intake.private.remove(&journal_name)?;
    println!("{}", serde_json::to_string(&receipt)?);
    Ok(())
}

pub(crate) fn run(command: &IntakeCommand, json_format: bool) -> Result<()> {
    let intake = Intake::open(command)?;
    match command {
        IntakeCommand::Enroll { .. } => {
            println!(
                "{}",
                serde_json::json!({"enrolled": true, "branch": "aida-intent", "store_required": false})
            );
        }
        IntakeCommand::Publish { file, reviewed, .. } => {
            if let Some(file) = file {
                if !reviewed {
                    bail!("Publication requires explicit --reviewed confirmation");
                }
                let bytes = envelope::read_bounded(file, ENVELOPE_LIMIT)?;
                let value = envelope::parse(&bytes, &intake.profile.deny_patterns)?;
                intake.add_file(&format!("observations/{}.json", value.checksum), &bytes)?;
            }
            intake.push()?;
            println!(
                "{}",
                serde_json::json!({"published": true, "branch": "aida-intent"})
            );
        }
        IntakeCommand::Pull { .. } => {
            intake.pull()?;
            println!(
                "{}",
                serde_json::json!({"pulled": true, "branch": "aida-intent"})
            );
        }
        IntakeCommand::List { all, json } => {
            let files = intake.files()?;
            let mut rows = Vec::new();
            for (path, bytes) in files.iter().filter(|(p, _)| p.starts_with("observations/")) {
                let value = envelope::parse(bytes, &intake.profile.deny_patterns)?;
                let receipt = files
                    .get(&path.replacen("observations/", "receipts/", 1))
                    .map(|b| serde_json::from_slice::<Receipt>(b))
                    .transpose()?;
                if *all || receipt.is_none() {
                    rows.push(serde_json::json!({"label": value.label(), "checksum": value.checksum, "title": value.title, "state": if receipt.is_some() {"processed"} else {"pending"}, "receipt": receipt}));
                }
            }
            if *json || json_format {
                println!("{}", serde_json::to_string(&rows)?);
            } else {
                for row in rows {
                    println!(
                        "{} {} {} {}",
                        row["label"].as_str().unwrap(),
                        row["checksum"].as_str().unwrap(),
                        row["state"].as_str().unwrap(),
                        row["title"]
                    );
                }
            }
        }
        IntakeCommand::Process {
            checksum,
            action,
            target,
            ..
        } => process(&intake, checksum, *action, target)?,
    }
    Ok(())
}
