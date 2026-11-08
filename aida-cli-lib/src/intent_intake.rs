//! Independent, clone-local observation transport. Envelope prose is data only.
// trace:TASK-3096 | ai:codex
use crate::observation_envelope::{self as envelope, Envelope, ENVELOPE_LIMIT};
use aida_core::transport_policy::{self as transport_process, Private};
use anyhow::{bail, Context, Result};
use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const LEGACY_BRANCH: &str = "refs/heads/aida-intent";
const INCOMING: &str = "refs/intake/incoming";
const PROFILE: &str = "profile.json";
const REPO_CONFIG: &[u8] = b"[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n\tlogallrefupdates = true\n";
// Bound inventory work on cold, fingerprint-invalidated and incremental passes.
// trace:STORY-2480 | ai:codex
const CI_HISTORY_REVALIDATION_ATTEMPTS: usize = 32;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    validator: String,
    tip: String,
    // One bit per lexically sorted CI path at tip; <=8192 bytes, well below
    // Private's 64-KiB cap even when every retained result has been verified.
    verified_ci: String,
}

const CAP: usize = 8 * 1024 * 1024;

#[derive(Debug, Subcommand)]
pub(crate) enum IntakeCommand {
    /// Configure or run the restricted work CI return channel.
    Ci {
        /// Emit a JSON result object.
        #[clap(long = "json", global = true)]
        ci_json: bool,
        #[clap(subcommand)]
        command: crate::ci_relay::CiCommand,
    },
    /// Enroll an independent pinned URL and reviewed deny patterns; no store required.
    Enroll {
        /// Explicit absolute local path, HTTPS URL, or SSH URL.
        #[clap(long)]
        remote: String,
        /// Opt into per-writer histories (omitting this preserves legacy enrollment).
        #[clap(long, value_enum)]
        role: Option<IntakeRole>,
        /// Stable opaque writer ID; defaults to WORK binding or a clone-local UUID.
        #[clap(long, requires = "role")]
        writer_id: Option<String>,
        /// Producer to read; required for a processor, otherwise the writer itself.
        #[clap(long, requires = "role")]
        producer_id: Option<String>,
        /// Authorized receipt writer to read (repeatable; currently one processor).
        #[clap(long, requires = "role")]
        receipt_writer: Vec<String>,
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
    /// Commit a reviewed envelope and push only the enrolled producer branch.
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
        /// Confirm the prior writer has stopped; reconcile its journal with store evidence.
        #[clap(long)]
        reconcile: bool,
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

// trace:STORY-2652 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum IntakeRole {
    Producer,
    Processor,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    remote: String,
    deny_patterns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    writer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<IntakeRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    producer_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    receipt_branches: Vec<String>,
}

// IDs label writers; neither the ID nor a branch name authenticates a writer.
// trace:STORY-2652 | ai:codex
fn writer_id_valid(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Profile {
    fn branch(&self) -> &str {
        match self.role {
            None => LEGACY_BRANCH,
            Some(IntakeRole::Producer) => self.producer_branch.as_deref().unwrap(),
            Some(IntakeRole::Processor) => &self.receipt_branches[0],
        }
    }

    fn output_branch(&self) -> &str {
        if self.version == 1 {
            "aida-intent"
        } else {
            self.branch()
        }
    }

    fn branches(&self) -> Vec<&str> {
        if self.version == 1 {
            vec![LEGACY_BRANCH]
        } else {
            std::iter::once(self.producer_branch.as_deref().unwrap())
                .chain(self.receipt_branches.iter().map(String::as_str))
                .collect()
        }
    }

    fn validate(&self) -> Result<()> {
        if self.version == 1 {
            if self.role.is_some()
                || self.writer_id.is_some()
                || self.producer_branch.is_some()
                || !self.receipt_branches.is_empty()
            {
                bail!("Legacy enrollment cannot contain per-writer routing");
            }
            return Ok(());
        }
        if self.version != 2
            || self.role.is_none()
            || !self.writer_id.as_deref().is_some_and(writer_id_valid)
        {
            bail!("Unsupported or invalid intake writer profile");
        }
        let producer = self
            .producer_branch
            .as_deref()
            .context("Missing producer branch")?;
        if !producer
            .strip_prefix("refs/heads/aida-feedback-")
            .is_some_and(writer_id_valid)
            || self.receipt_branches.len() > 1
            || self.receipt_branches.iter().any(|b| {
                !b.strip_prefix("refs/heads/aida-feedback-receipts-")
                    .is_some_and(writer_id_valid)
            })
        {
            bail!("Invalid per-writer branches; this enrollment supports one authorized processor");
        }
        let writer = self.writer_id.as_deref().unwrap();
        match self.role.unwrap() {
            IntakeRole::Producer if producer != format!("refs/heads/aida-feedback-{writer}") => {
                bail!("Producer branch does not match enrolled writer")
            }
            IntakeRole::Processor
                if self.receipt_branches
                    != vec![format!("refs/heads/aida-feedback-receipts-{writer}")] =>
            {
                bail!("Processor must enroll its own receipt branch")
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    checksum: String,
    action: Action,
    /// Identity of the operation that published this immutable receipt.
    operation: String,
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

// trace:TASK-3096 | ai:codex
pub(super) fn validate_remote(remote: &str) -> Result<()> {
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
    if let Some(url) = remote.strip_prefix("https://") {
        let authority = url.split(['/', '?', '#']).next().unwrap_or("");
        if authority.contains('@') {
            bail!("HTTPS userinfo is refused; use ssh:// with keys or an SSH agent for authenticated intake destinations");
        }
    }
    Ok(())
}

fn validator_prefix() -> String {
    use sha2::{Digest, Sha256};
    let mut validator = Sha256::new();
    validator.update(include_bytes!("intent_intake.rs"));
    validator.update(include_bytes!("observation_envelope.rs"));
    validator.update(include_bytes!("ci_receipt.rs"));
    validator.update(include_bytes!("ci_relay.rs"));
    validator.update(include_bytes!("ci_inventory.rs"));
    format!("v1:{:x}:", validator.finalize())
}

// Remove ambient Git routing/configuration. Never inherit product indexes,
// alternates, replacement objects, URL rewrites, hooks or extra push refspecs.
pub(super) fn git_command(root: &Path) -> Command {
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

pub(super) fn git(root: &Path, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
    // trace:TASK-3096 | ai:codex
    let mut cmd = git_command(root);
    cmd.args(args);
    let (status, bytes, stderr) = transport_process::bounded_with_stderr(
        cmd,
        input,
        CAP,
        Instant::now() + Duration::from_secs(30),
    )?;
    if !status.success() {
        bail!("Intake Git operation refused ({}): {}; inspect the independent intake repository; no force/rebase is performed", args[0], String::from_utf8_lossy(&stderr));
    }
    Ok(bytes)
}

fn text(bytes: Vec<u8>) -> Result<String> {
    Ok(String::from_utf8(bytes)?.trim().to_string())
}

pub(super) struct Intake {
    pub private: Private,
    pub worktree: PathBuf,
    ci_snapshot: RefCell<Option<crate::ci_relay::Snapshot>>,
    verified_ci: RefCell<BTreeSet<String>>,
    profile: Profile,
    _lock: fs::File,
    blobs: RefCell<BTreeMap<String, Vec<u8>>>,
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
            role,
            writer_id,
            producer_id,
            receipt_writer,
            ..
        } = command
        {
            if !reviewed {
                bail!("Enrollment requires --reviewed");
            }
            validate_remote(remote)?;
            envelope::check_patterns("", "", deny_pattern)?;
            let old_profile = private
                .read(PROFILE)?
                .map(|b| serde_json::from_slice::<Profile>(&b))
                .transpose()?;
            let writer = if role.is_some() {
                Some(match writer_id {
                    Some(id) => id.clone(),
                    None => match old_profile.as_ref().and_then(|p| p.writer_id.clone()) {
                        Some(id) => id,
                        None => aida_core::work_binding::load(&cwd)?
                            .map(|b| b.node_id.to_ascii_lowercase())
                            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()),
                    },
                })
            } else {
                None
            };
            if writer.as_deref().is_some_and(|id| !writer_id_valid(id)) {
                bail!("Invalid writer identity");
            }
            let producer_branch = if let Some(role) = role {
                let producer = match role {
                    IntakeRole::Producer => producer_id.as_ref().or(writer.as_ref()).unwrap(),
                    IntakeRole::Processor => producer_id
                        .as_ref()
                        .context("Processor enrollment requires --producer-id")?,
                };
                if !writer_id_valid(producer) {
                    bail!("Invalid producer identity");
                }
                Some(format!("refs/heads/aida-feedback-{producer}"))
            } else {
                None
            };
            let mut receipts = receipt_writer.clone();
            if *role == Some(IntakeRole::Processor) {
                if !receipts.is_empty() && receipts != vec![writer.clone().unwrap()] {
                    bail!("Processor may enroll only its own receipt branch");
                }
                receipts = vec![writer.clone().unwrap()];
            }
            if receipts.iter().any(|id| !writer_id_valid(id)) {
                bail!("Invalid receipt writer identity");
            }
            let proposed = Profile {
                version: if role.is_some() { 2 } else { 1 },
                remote: remote.clone(),
                deny_patterns: deny_pattern.clone(),
                writer_id: writer,
                role: *role,
                producer_branch,
                receipt_branches: receipts
                    .iter()
                    .map(|id| format!("refs/heads/aida-feedback-receipts-{id}"))
                    .collect(),
            };
            proposed.validate()?;
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
        validate_remote(&profile.remote)?;
        profile.validate()?;
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
            ci_snapshot: RefCell::new(None),
            verified_ci: RefCell::new(BTreeSet::new()),
            profile,
            _lock: lock,
            blobs: RefCell::new(BTreeMap::new()),
        })
    }

    pub(super) fn profile_remote(&self) -> String {
        self.profile.remote.clone()
    }

    fn ci_admit(&self, path: &str) -> Result<()> {
        if self.ci_snapshot.borrow().is_none() {
            *self.ci_snapshot.borrow_mut() = Some(crate::ci_relay::Snapshot::load(self)?);
        }
        self.ci_snapshot.borrow().as_ref().unwrap().admit(path)?;
        Ok(())
    }

    pub(super) fn ci_validate(&self, path: &str, bytes: &[u8]) -> Result<()> {
        if self.ci_snapshot.borrow().is_none() {
            *self.ci_snapshot.borrow_mut() = Some(crate::ci_relay::Snapshot::load(self)?);
        }
        self.ci_snapshot
            .borrow()
            .as_ref()
            .unwrap()
            .validate(path, bytes)?;
        self.verified_ci.borrow_mut().insert(path.to_owned());
        Ok(())
    }

    fn tip(&self) -> Result<Option<String>> {
        self.branch_tip(self.profile.branch())
    }

    fn branch_tip(&self, branch: &str) -> Result<Option<String>> {
        let (ok, bytes) = git_result(&self.worktree, &["rev-parse", "--verify", branch], &[], 128)?;
        if ok {
            Ok(Some(text(bytes)?))
        } else {
            Ok(None)
        }
    }

    fn blob(&self, oid: &str, limit: usize) -> Result<Vec<u8>> {
        if let Some(bytes) = self.blobs.borrow().get(oid) {
            if bytes.len() > limit {
                bail!("Intake object exceeds size limit");
            }
            return Ok(bytes.clone());
        }
        let size: usize = text(git(&self.worktree, &["cat-file", "-s", oid], &[])?)?.parse()?;
        if size > limit {
            bail!("Intake object exceeds size limit");
        }
        let (ok, bytes) = git_result(&self.worktree, &["cat-file", "blob", oid], &[], limit)?;
        if !ok {
            bail!("Unreadable intake blob");
        }
        self.blobs
            .borrow_mut()
            .insert(oid.to_string(), bytes.clone());
        Ok(bytes)
    }

    /// Validate every reachable commit, including intermediate trees. Linear,
    /// additive history only; exact immutable bytes for observations/receipts.
    fn history(&self, tip: &str) -> Result<BTreeMap<String, Vec<u8>>> {
        self.branch_history(tip, self.profile.branch())
    }

    // Each orphan history is validated independently, before unioning views.
    // trace:STORY-2652 | ai:codex
    fn branch_history(&self, tip: &str, branch: &str) -> Result<BTreeMap<String, Vec<u8>>> {
        let commits = text(git(
            &self.worktree,
            &["rev-list", "--reverse", "--topo-order", "--parents", tip],
            &[],
        )?)?;
        // A checkpoint preserves the exact subset with inventory authority. A
        // validator change (including dirty sources) takes the same bounded cold path.
        let checkpoint = self
            .private
            .read("validated-tip")?
            .and_then(|bytes| serde_json::from_slice::<Checkpoint>(&bytes).ok())
            .filter(|c| {
                self.profile.version == 1
                    && c.validator == validator_prefix()
                    && c.verified_ci.len() <= 8192
                    && c.verified_ci.bytes().all(|b| matches!(b, b'0' | b'1'))
                    && commits
                        .lines()
                        .any(|line| line.split_whitespace().next() == Some(c.tip.as_str()))
            });
        self.verified_ci.borrow_mut().clear();
        if let Some(c) = &checkpoint {
            let paths = self.ci_paths(&c.tip)?;
            if paths.len() != c.verified_ci.len() {
                bail!("Invalid CI checkpoint verification set");
            }
            *self.verified_ci.borrow_mut() = paths
                .into_iter()
                .zip(c.verified_ci.bytes())
                .filter_map(|(path, bit)| (bit == b'1').then_some(path))
                .collect();
        }
        let checkpoint = checkpoint.map(|c| c.tip);
        let mut reached = checkpoint.is_none();
        let mut deadline = Instant::now() + Duration::from_secs(30);
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
            if !reached && checkpoint.as_deref() != Some(fields[0]) {
                parent = Some(fields[0]);
                continue;
            }
            reached = true;
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
                if self.profile.version == 2 {
                    let allowed = if branch.starts_with("refs/heads/aida-feedback-receipts-") {
                        "receipts"
                    } else {
                        "observations"
                    };
                    if path != allowed && !path.starts_with(&format!("{allowed}/")) {
                        bail!("Path is forbidden for this intake branch role");
                    }
                }
                if parts.len() == 3
                    && parts[0] == "040000"
                    && parts[1] == "tree"
                    && (matches!(path, "observations" | "receipts" | "ci-results")
                        || path
                            .strip_prefix("ci-results/")
                            .is_some_and(crate::ci_receipt::oid))
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
                if folder != "ci-results"
                    && (!digest(checksum) || !matches!(folder, "observations" | "receipts"))
                {
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
                if folder == "ci-results" {
                    // Retained refs remain live authority. One remote ref
                    // snapshot checks all identities without fetching sources.
                    self.ci_admit(path)?;
                    let snapshot = self.ci_snapshot.borrow();
                    let (sha, n) = snapshot.as_ref().unwrap().admit(path)?;
                    let r = crate::ci_receipt::Receipt::parse_history(&bytes, sha, n)?;
                    if r.path() != path {
                        bail!("Noncanonical CI result path");
                    }
                    // Bound both cold and warm passes to the newest retained
                    // attempts. Excess new results remain audit data until an
                    // exact reader requests them; cached proof remains valid.
                    // trace:STORY-2480 | ai:codex
                    let recent = snapshot
                        .as_ref()
                        .unwrap()
                        .requests
                        .keys()
                        .rev()
                        .take(CI_HISTORY_REVALIDATION_ATTEMPTS)
                        .any(|attempt| *attempt == n);
                    let selected = recent
                        && (checkpoint.is_none()
                            || (checkpoint.as_deref() != Some(fields[0])
                                && previous.get(path) != Some(&bytes)));
                    drop(snapshot);
                    if selected && !self.ci_verified(path) {
                        // Source validation owns its separate bounded snapshot
                        // budget. Do not charge 48+ MiB parsing against the
                        // intake ancestry/metadata budget as well.
                        // trace:TASK-3232 | ai:codex
                        let started = Instant::now();
                        self.ci_validate(path, &bytes)?;
                        deadline += started.elapsed();
                    }
                } else if folder == "observations" {
                    let value = envelope::parse(&bytes, &self.profile.deny_patterns)?;
                    if value.checksum != checksum {
                        bail!("Intake digest filename mismatch");
                    }
                } else {
                    let receipt: Receipt = serde_json::from_slice(&bytes)
                        .map_err(|_| anyhow::anyhow!("Invalid intake receipt schema or JSON"))?;
                    if receipt.version != 1
                        || receipt.checksum != checksum
                        || uuid::Uuid::parse_str(&receipt.operation).is_err()
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
            if self.profile.version == 2 && parent.is_some() && current.len() <= previous.len() {
                bail!("Per-writer commits must add immutable files");
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
                if self.profile.version == 1
                    && !current.contains_key(&path.replacen("receipts/", "observations/", 1))
                {
                    bail!("Receipt without observation refused");
                }
            }
            previous = current;
            parent = Some(fields[0]);
        }
        let unverified = previous
            .keys()
            .filter(|p| p.starts_with("ci-results/") && !self.ci_verified(p))
            .count();
        if unverified > 0 {
            eprintln!("CI intake: {unverified} unverified-history results (audit only; exact attempt/SHA read required)");
        }
        Ok(previous)
    }

    fn ci_paths(&self, tip: &str) -> Result<Vec<String>> {
        let tree = git(
            &self.worktree,
            &["ls-tree", "-rz", "--name-only", "--full-tree", tip],
            &[],
        )?;
        let mut paths: Vec<_> = tree
            .split(|b| *b == 0)
            .filter(|p| p.starts_with(b"ci-results/"))
            .map(|p| String::from_utf8(p.to_vec()))
            .collect::<std::result::Result<_, _>>()?;
        paths.sort();
        Ok(paths)
    }

    // Candidate/quarantine validation never replaces proof of an accepted tip.
    // trace:TASK-3096 | ai:codex
    fn checkpoint(&self, tip: &str) -> Result<()> {
        let verified_ci = self
            .ci_paths(tip)?
            .iter()
            .map(|path| if self.ci_verified(path) { '1' } else { '0' })
            .collect();
        self.private.write(
            "validated-tip",
            &serde_json::to_vec(&Checkpoint {
                validator: validator_prefix(),
                tip: tip.to_owned(),
                verified_ci,
            })?,
        )
    }

    pub(super) fn ci_verified(&self, path: &str) -> bool {
        self.verified_ci.borrow().contains(path)
    }

    pub(super) fn verify_ci_read(&self, path: &str, bytes: &[u8]) -> Result<()> {
        if !self.ci_verified(path) {
            self.ci_validate(path, bytes)?;
            if let Some(tip) = self.tip()? {
                self.checkpoint(&tip)?;
            }
        }
        Ok(())
    }

    pub(super) fn files(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut files = BTreeMap::new();
        for branch in self.profile.branches() {
            if let Some(tip) = self.branch_tip(branch)? {
                for (path, bytes) in self.branch_history(&tip, branch)? {
                    if files.insert(path, bytes).is_some() {
                        bail!("Conflicting intake paths across enrolled histories");
                    }
                }
                if self.profile.version == 1 {
                    self.checkpoint(&tip)?;
                }
            }
        }
        Ok(files)
    }

    /// Append one immutable file and publish it before the accepted branch
    /// moves. The new commit is built on the freshly synchronized tip and only
    /// becomes local history after the remote accepted it, so no site ever holds
    /// an unpublished local commit that a later remote write could diverge from.
    /// A refused push (another writer won the race) re-synchronizes and builds a
    /// fresh commit on the new tip: accepted commits are never rewritten.
    // trace:TASK-3096 | ai:claude
    pub(super) fn append(&self, path: &str, bytes: &[u8]) -> Result<()> {
        for _ in 0..4 {
            self.sync()?;
            let tip = self.tip()?;
            let files = self.files()?;
            if let Some(old) = files.get(path) {
                if old != bytes {
                    bail!("Conflicting bytes for immutable intake filename");
                }
                return self.push();
            }
            let commit = self.build(tip.as_deref(), path, bytes)?;
            if self.push_commit(&commit, tip.as_deref())? {
                git(
                    &self.worktree,
                    &[
                        "update-ref",
                        self.profile.branch(),
                        &commit,
                        tip.as_deref().unwrap_or(""),
                    ],
                    &[],
                )?;
                self.checkpoint(&commit)?;
                // Materialize only validated files in the private checkout. No
                // product checkout/index/ref is ever involved. Safely repeatable.
                git(
                    &self.worktree,
                    &["read-tree", "--reset", "-u", &commit],
                    &[],
                )?;
                return Ok(());
            }
        }
        bail!("Intake publication lost the race to other writers repeatedly; the accepted local branch is unchanged. Retry the same command")
    }

    /// Build and validate a commit adding `path` on `tip`, without referencing it.
    fn build(&self, tip: Option<&str>, path: &str, bytes: &[u8]) -> Result<String> {
        // Plumbing uses this repository's index only; reset from the accepted
        // tree each time so an interrupted update cannot smuggle extra files.
        match tip {
            Some(tip) => git(&self.worktree, &["read-tree", tip], &[])?,
            None => git(&self.worktree, &["read-tree", "--empty"], &[])?,
        };
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
        if let Some(tip) = tip {
            args.extend(["-p", tip]);
        }
        let commit = text(git(
            &self.worktree,
            &args,
            b"Record reviewed intake data\n",
        )?)?;
        self.history(&commit)?;
        Ok(commit)
    }

    /// Retry only a confirmed remote-tip race; preserve other push diagnostics.
    // trace:TASK-3096 | ai:codex
    fn push_commit(&self, commit: &str, base: Option<&str>) -> Result<bool> {
        let refspec = format!("{commit}:{}", self.profile.branch());
        let mut cmd = git_command(&self.worktree);
        // remote-creds-ok(TASK-2001): separately enrolled URL; ambient product credentials/config are intentionally excluded.
        cmd.args([
            "-c",
            "push.default=nothing",
            "push",
            "--no-verify",
            "--no-follow-tags",
            "--",
            &self.profile.remote,
            &refspec,
        ]);
        let (status, _, stderr) = transport_process::bounded_with_stderr(
            cmd,
            &[],
            CAP,
            Instant::now() + Duration::from_secs(30),
        )?;
        if status.success() {
            return Ok(true);
        }
        let diagnostic = String::from_utf8_lossy(&stderr);
        let remote = self
            .remote_tip()
            .with_context(|| format!("Intake push refused: {diagnostic}"))?;
        if remote.as_deref() != base {
            return Ok(false);
        }
        bail!("Intake push refused: {diagnostic}");
    }

    fn remote_tip(&self) -> Result<Option<String>> {
        self.remote_branch_tip(self.profile.branch())
    }

    fn remote_branch_tip(&self, branch: &str) -> Result<Option<String>> {
        // remote-creds-ok(TASK-2001): independently pinned intake URL, never product remote credential routing.
        let advertised = text(git(
            &self.worktree,
            &["ls-remote", "--", &self.profile.remote, branch],
            &[],
        )?)?;
        Ok(advertised.lines().find_map(|line| {
            let mut fields = line.split_whitespace();
            let tip = fields.next()?;
            (fields.next() == Some(branch)).then(|| tip.to_owned())
        }))
    }

    /// Publish the accepted tip; a no-op when the remote already has it.
    fn push(&self) -> Result<()> {
        let Some(tip) = self.tip()? else {
            return Ok(());
        };
        self.history(&tip)?;
        let base = self.remote_tip()?;
        if !self.push_commit(&tip, base.as_deref())? {
            bail!("Intake push refused; the accepted local branch is unchanged. Retry; the tool never forces, merges or rebases");
        }
        Ok(())
    }

    fn ancestor(&self, older: &str, newer: &str) -> Result<bool> {
        Ok(git_result(
            &self.worktree,
            &["merge-base", "--is-ancestor", older, newer],
            &[],
            128,
        )?
        .0)
    }

    /// Fetch into quarantine, validate all incoming history, then accept only a
    /// fast-forward. Returns `false` when the remote has no intake branch yet.
    // trace:TASK-3096 | ai:claude
    pub(super) fn sync(&self) -> Result<bool> {
        let mut found = false;
        for branch in self.profile.branches() {
            found |= self.sync_branch(branch)?;
        }
        Ok(found)
    }

    fn sync_branch(&self, branch: &str) -> Result<bool> {
        // Clear rejected quarantine evidence before fetching, never the accepted ref.
        git(&self.worktree, &["update-ref", "-d", INCOMING], &[])?;
        if self.remote_branch_tip(branch)?.is_none() {
            return Ok(false);
        }
        let refspec = format!("{branch}:{INCOMING}");
        // remote-creds-ok(TASK-2001): independently pinned intake URL, never product remote credential routing.
        git(
            &self.worktree,
            &[
                "fetch",
                "--no-tags",
                "--no-write-fetch-head",
                "--no-recurse-submodules",
                "--",
                &self.profile.remote,
                &refspec,
            ],
            &[],
        )?;
        let incoming = text(git(
            &self.worktree,
            &["rev-parse", "--verify", INCOMING],
            &[],
        )?)?;
        self.branch_history(&incoming, branch)?;
        let old = self.branch_tip(branch)?;
        if let Some(old) = &old {
            if *old == incoming {
                self.checkpoint(&incoming)?;
                return Ok(true);
            }
            // old is already accepted. Only incoming needs validation; proof
            // advances after its fast-forward acceptance below.
            if self.ancestor(&incoming, old)? {
                // Local is ahead (its push failed after acceptance): keep it;
                // publication retries it as a fast-forward.
                return Ok(true);
            }
            if !self.ancestor(old, &incoming)? {
                bail!("Intake diverged; accepted local branch unchanged. Remote history was rewritten outside AIDA (a force push); review both histories with an advisor. Never force-push");
            }
        }
        git(
            &self.worktree,
            &[
                "update-ref",
                branch,
                &incoming,
                old.as_deref().unwrap_or(""),
            ],
            &[],
        )?;
        self.checkpoint(&incoming)?;
        git(
            &self.worktree,
            &["read-tree", "--reset", "-u", &incoming],
            &[],
        )?;
        Ok(true)
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

fn process(
    intake: &Intake,
    checksum: &str,
    action: Action,
    target: &Option<String>,
    reconcile: bool,
    json: bool,
) -> Result<()> {
    if intake.profile.role == Some(IntakeRole::Producer) {
        bail!("Only an enrolled processor may process observations");
    }
    if !digest(checksum) {
        bail!("Supply the full lowercase SHA-256 digest");
    }
    if (action == Action::Comment) != target.is_some() {
        bail!("Only --action comment requires --target");
    }
    if action != Action::Pending || reconcile {
        // A receipt is published as part of processing, so see every receipt
        // another site already published before deciding anything.
        // trace:TASK-3096 | ai:claude
        intake.sync()?;
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
        // A receipt may have won a concurrent publication, or cleanup may
        // have crashed after our own push. Never hide a confirmed local write.
        // trace:TASK-3096 | ai:codex
        if let Some(bytes) = intake.private.read(&journal_name)? {
            let journal: Journal = serde_json::from_slice(&bytes)?;
            if journal.version != 1
                || journal.checksum != checksum
                || !matches!(journal.action, Action::Create | Action::Comment)
                || (journal.action == Action::Comment) != journal.target.is_some()
                || uuid::Uuid::parse_str(&journal.operation).is_err()
            {
                bail!("Invalid processing journal; inspect manually");
            }
            match evidence(&journal)? {
                None if reconcile => {} // Explicit assertion that the old writer stopped.
                None => bail!("Receipt exists but local operation evidence is missing; journal retained for manual reconciliation; rerun with --reconcile after confirming the old writer stopped"),
                Some(destination) => {
                    if receipt.operation != journal.operation
                        || receipt.action != journal.action
                        || receipt.destination.as_deref() != Some(destination.as_str())
                    {
                        let (_, store) = canonical_store()?;
                        let spec = store
                            .get_requirement_unambiguous(&destination)?
                            .context("Local operation destination not found")?
                            .friendly_id();
                        bail!("Orphaned duplicate spec write {spec} ({destination}): published receipt belongs to another operation; journal retained for operator reconciliation");
                    }
                }
            }
            intake.private.remove(&journal_name)?;
        }
        if reconcile && action == Action::Pending {
            print_receipt(&receipt, json)?;
            return Ok(());
        }
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
        print_receipt(&receipt, json)?;
        return Ok(());
    }
    if reconcile {
        if let Some(bytes) = intake.private.read(&journal_name)? {
            let journal: Journal = serde_json::from_slice(&bytes)?;
            if journal.version != 1
                || journal.checksum != checksum
                || uuid::Uuid::parse_str(&journal.operation).is_err()
            {
                bail!("Invalid processing journal; inspect manually");
            }
            if evidence(&journal)?.is_none() {
                // Explicit operator assertion that the old writer is stopped,
                // plus a fresh store scan, permits abandoning an unconfirmed op.
                intake.private.remove(&journal_name)?;
            } else if matches!(action, Action::Pending | Action::Dismiss) {
                bail!("Confirmed prior spec write; repeat its original action and target to recover the receipt");
            }
        }
    }
    if action == Action::Pending {
        if intake.private.read(&journal_name)?.is_some() {
            bail!("Unresolved processing operation; reconcile before leaving pending");
        }
        if json {
            println!(
                "{}",
                serde_json::json!({"checksum": checksum, "state": "pending"})
            );
        } else {
            println!("{checksum} pending");
        }
        return Ok(());
    }
    let mut destination = None;
    let mut operation = uuid::Uuid::new_v4().to_string();
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
        operation = journal.operation.clone();
        destination = evidence(&journal)?;
        if destination.is_none() {
            if existing.is_some() {
                bail!("Ambiguous prior spec write: durable journal retained; inspect intake-op evidence and reconcile manually. Refusing automatic replay");
            }
            let marker = format!("intake-op:{}", journal.operation);
            // Reuse normal CLI authorization and canonical writers, never bypass
            // role, transport, hypothesis or store mutation policy here.
            let mut cmd = Command::new(crate::resolve_aida_exe());
            let (ceiling_key, ceiling_value) = crate::bg_wait_ceiling_env(None);
            cmd.env("AIDA_HEADLESS", "1")
                .envs(crate::headless_wait_envs(ceiling_key, &ceiling_value));
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
            let result = transport_process::bounded_with_stderr(
                cmd,
                body.as_bytes(),
                CAP,
                Instant::now() + Duration::from_secs(30),
            );
            // The bounded runner has stopped/reaped its group even on timeout,
            // spawn failure or overflow. Fresh canonical evidence decides recovery.
            destination = evidence(&journal)?;
            if destination.is_none() {
                intake.private.remove(&journal_name)?;
                match result {
                    Ok((_, _, stderr)) => bail!("Spec writer made no confirmed write; retry is available: {}", String::from_utf8_lossy(&stderr)),
                    Err(error) => bail!("Spec writer stopped without a confirmed write; retry is available: {error:#}"),
                }
            }
            if let Ok((_, _, stderr)) = result {
                if !stderr.is_empty() {
                    eprint!("{}", String::from_utf8_lossy(&stderr));
                }
            }
        }
    } else if intake.private.read(&journal_name)?.is_some() {
        bail!("Unresolved spec write; cannot dismiss before reconciliation");
    }
    let receipt = Receipt {
        version: 1,
        checksum: checksum.into(),
        action,
        operation,
        destination,
    };
    // The receipt is published before it becomes local history. If that fails
    // after a spec write, the journal stays and the same command recovers the
    // confirmed write from its marker and publishes the receipt without a
    // second write. trace:TASK-3096 | ai:claude
    intake
        .append(&receipt_path, &serde_json::to_vec(&receipt)?)
        .map_err(|error| {
            let recovery = if receipt.destination.is_some() {
                "Spec write confirmed but its receipt is not published yet; rerun the same process command to publish it (no second spec write)"
            } else {
                "Receipt not published; nothing changed, rerun the same process command"
            };
            anyhow::anyhow!("{recovery}: {error:#}")
        })?;
    // Receipt is durable before journal cleanup; retry after either crash is safe.
    intake.private.remove(&journal_name)?;
    print_receipt(&receipt, json)?;
    Ok(())
}

fn receipt_view(
    receipt: &Receipt,
    store: Option<&aida_core::RequirementsStore>,
) -> Result<serde_json::Value> {
    let mut view = serde_json::to_value(receipt)?;
    if let Some(id) = &receipt.destination {
        // Listing an observation clone needs no store. Keep UUID if unavailable.
        if let Some(store) = store {
            if let Some(req) = store.get_requirement_unambiguous(id)? {
                view["destination"] = serde_json::Value::String(req.friendly_id());
            }
        }
    }
    Ok(view)
}

fn print_receipt(receipt: &Receipt, json: bool) -> Result<()> {
    let store = receipt
        .destination
        .as_ref()
        .and_then(|_| canonical_store().ok().map(|(_, store)| store));
    let view = receipt_view(receipt, store.as_ref())?;
    if json {
        println!("{}", view);
    } else {
        println!(
            "{} {}{}",
            receipt.checksum,
            serde_json::to_value(receipt.action)?.as_str().unwrap(),
            view["destination"]
                .as_str()
                .map(|id| format!(" → {id}"))
                .unwrap_or_default()
        );
    }
    Ok(())
}

pub(crate) fn run(command: &IntakeCommand, json_format: bool) -> Result<()> {
    let intake = Intake::open(command)?;
    let json = json_format
        || match command {
            IntakeCommand::Ci { ci_json, .. } => *ci_json,
            IntakeCommand::Enroll { json, .. }
            | IntakeCommand::Publish { json, .. }
            | IntakeCommand::Pull { json }
            | IntakeCommand::List { json, .. }
            | IntakeCommand::Process { json, .. } => *json,
        };
    match command {
        IntakeCommand::Ci { command, .. } => {
            if intake.profile.version == 2 {
                bail!("CI relay requires legacy enrollment");
            }
            crate::ci_relay::run(&intake, command, json)?;
        }
        IntakeCommand::Enroll { .. } => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"enrolled": true, "branch": intake.profile.output_branch(), "profile": intake.profile, "protection": "unverified", "store_required": false})
                );
            } else {
                println!("Enrolled independent intake destination.");
                if intake.profile.version == 2 {
                    println!("Writer protection is unverified; verify remote branch restrictions independently.");
                }
            }
        }
        IntakeCommand::Publish { file, reviewed, .. } => {
            if intake.profile.role == Some(IntakeRole::Processor) {
                bail!("Only an enrolled producer may publish observations; processors use process");
            }
            if let Some(file) = file {
                if !reviewed {
                    bail!("Publication requires explicit --reviewed confirmation");
                }
                let bytes = envelope::read_bounded(file, ENVELOPE_LIMIT)?;
                let value = envelope::parse(&bytes, &intake.profile.deny_patterns)?;
                intake.append(&format!("observations/{}.json", value.checksum), &bytes)?;
            } else {
                intake.sync()?;
                intake.push()?;
            }
            if json {
                println!(
                    "{}",
                    serde_json::json!({"published": true, "branch": intake.profile.output_branch()})
                );
            } else {
                println!("Published intake observations and receipts.");
            }
        }
        IntakeCommand::Pull { .. } => {
            let found = intake.sync()?;
            let files = intake.files()?;
            let unknown: Vec<_> = files
                .keys()
                .filter(|path| {
                    path.starts_with("receipts/")
                        && !files.contains_key(&path.replacen("receipts/", "observations/", 1))
                })
                .map(|path| {
                    path.trim_start_matches("receipts/")
                        .trim_end_matches(".json")
                })
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::json!({"pulled": true, "branch": intake.profile.output_branch(), "remote_empty": !found, "receipts_for_unknown_observations": unknown})
                );
            } else if found {
                println!("Pulled validated intake history.");
                if !unknown.is_empty() {
                    println!(
                        "{} receipt(s) for unknown observations (audit only).",
                        unknown.len()
                    );
                }
            } else {
                println!("Pulled: the intake destination has no history yet.");
            }
        }
        IntakeCommand::List { all, .. } => {
            let files = intake.files()?;
            let store = if *all {
                canonical_store().ok().map(|(_, store)| store)
            } else {
                None
            };
            let mut rows = Vec::new();
            for (path, bytes) in files.iter().filter(|(p, _)| p.starts_with("observations/")) {
                let value = envelope::parse(bytes, &intake.profile.deny_patterns)?;
                let receipt = files
                    .get(&path.replacen("observations/", "receipts/", 1))
                    .map(|b| serde_json::from_slice::<Receipt>(b))
                    .transpose()?;
                if *all || receipt.is_none() {
                    rows.push(serde_json::json!({"label": value.label(), "checksum": value.checksum, "title": value.title, "state": if receipt.is_some() {"processed"} else {"pending"}, "receipt": receipt.as_ref().map(|r| receipt_view(r, store.as_ref())).transpose()?}));
                }
            }
            if json {
                println!("{}", serde_json::to_string(&rows)?);
            } else {
                for row in rows {
                    println!(
                        "{} {} {} {}{}",
                        row["label"].as_str().unwrap(),
                        row["checksum"].as_str().unwrap(),
                        row["state"].as_str().unwrap(),
                        row["title"].as_str().unwrap(),
                        row["receipt"]["destination"]
                            .as_str()
                            .map(|id| format!(" → {id}"))
                            .unwrap_or_default()
                    );
                }
            }
        }
        IntakeCommand::Process {
            checksum,
            action,
            target,
            reconcile,
            ..
        } => process(&intake, checksum, *action, target, *reconcile, json)?,
    }
    Ok(())
}
