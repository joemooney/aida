//! Clone-local transport policy, read-only plans, and ref-only store pushes.
//! Pull execution and intake sanitization remain separate work.
// trace:STORY-1742 | ai:codex

use anyhow::{bail, Context, Result};
use clap::{Subcommand, ValueEnum};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Output;

#[path = "transport_executor.rs"]
mod executor;
#[path = "transport_private.rs"]
mod private;
#[path = "transport_scan.rs"]
mod scan;

const PROFILE_FILE: &str = "aida-transport.json";
const STORE_REF: &str = "refs/heads/aida-store";
const TRACKING_REF: &str = "refs/remotes/origin/aida-store";
const PUSH_REFSPEC: &str = "refs/heads/aida-store:refs/heads/aida-store";
const PULL_REFSPEC: &str = "refs/heads/aida-store:refs/remotes/origin/aida-store";
const BYPASS_LIMIT: &str =
    "The profile governs aida push/pull only; raw Git bypasses it. No server-side enforcement.";
const PENDING: &str =
    "Transport pull execution is not available in this slice; use --dry-run to inspect the plan.";
const SANITIZATION_PENDING: &str = "Intake sanitization is not available in this slice. Outbound scanning requires an explicitly reviewed private policy and trust root; unsafe history refuses.";
const FORBIDDEN: &str = "Flag forbidden by the work transport profile: code push/pull, commit messages, and automatic rebases are disabled. Inspect `aida config transport show`.";

#[derive(Subcommand, Debug)]
pub enum TransportCommand {
    /// Enable a named clone-local store-only profile.
    Enable {
        #[clap(value_enum)]
        name: ProfileName,
        /// Enroll an explicitly reviewed public-safe store commit for later scanning.
        #[clap(long)]
        baseline: Option<String>,
        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
    /// Show effective policy, fixed refs, and the raw-Git bypass limitation.
    Show {
        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
    /// Remove the clone-local profile; restore ordinary AIDA push/pull behavior.
    Disable {
        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
    /// Enroll a reviewed owner-only JSON deny policy and pseudonymous identity.
    Policy {
        /// Owner-only JSON file; values are never printed or shared.
        #[clap(long)]
        file: PathBuf,
        /// Confirm operator review, including an intentionally empty deny list.
        #[clap(long, required = true)]
        reviewed: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileName {
    Work,
}

/// Hash the effective single origin endpoint rather than saving credentials or
/// endpoint text. Fixed refspecs are code-owned, not an extensible local language.
// trace:STORY-1742 | ai:codex
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    name: ProfileName,
    origin_sha256: String,
    baseline: Option<String>,
}

impl Profile {
    fn validate(&self) -> Result<()> {
        if self.version != 1
            || !is_hex_id(&self.origin_sha256, &[64])
            || self
                .baseline
                .as_ref()
                .is_some_and(|sha| !is_hex_id(sha, &[40, 64]))
        {
            bail!("Invalid or unsupported clone-local transport profile; push/pull refused.");
        }
        Ok(())
    }
}

fn is_hex_id(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len()) && value.bytes().all(|c| c.is_ascii_hexdigit())
}

/// These calls inspect Git metadata only; do not fetch, refresh the store cache,
/// or invoke any ordinary sync handler. Never forward raw diagnostics (URLs may
/// carry credentials). The closed callers below use only rev-parse, config,
/// remote get-url, ls-remote --get-url (URL expansion only), worktree list,
/// symbolic-ref and merge-base: none executes hooks, filters, fsmonitor or
/// remote helpers. Avoid host_git_command here because its store-hook profile
/// installation would make inspection write state when invoked from the store.
// trace:STORY-1742 | ai:codex
fn git(root: &Path, args: &[&str]) -> Result<Output> {
    // raw-git-ok(BUG-1929): closed metadata-only lookups; no hook/filter/network execution.
    std::process::Command::new("git")
        .current_dir(root)
        .args(["-c", "core.fsmonitor=false"])
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .context("Could not inspect Git transport metadata")
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let output = git(root, args)?;
    if !output.status.success() {
        bail!("Could not inspect Git transport metadata; check local origin configuration.");
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|_| anyhow::anyhow!("Invalid text in Git transport metadata"))
}

fn common_dir(root: &Path) -> Result<Option<PathBuf>> {
    let output = git(root, &["rev-parse", "--git-common-dir"])?;
    if !output.status.success() {
        // Only a directory outside Git may fall through to ordinary discovery.
        // A damaged repository must not silently disable an enabled profile.
        if root.ancestors().any(|p| p.join(".git").exists()) {
            bail!("Cannot resolve clone-local Git metadata; transport policy is unknown.");
        }
        return Ok(None);
    }
    let path = PathBuf::from(
        String::from_utf8(output.stdout)
            .context("Invalid Git metadata directory")?
            .trim(),
    );
    if path.as_os_str().is_empty() {
        bail!("Missing Git metadata directory");
    }
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    Ok(Some(
        path.canonicalize()
            .context("Cannot resolve Git metadata directory")?,
    ))
}

fn load(common: &Path) -> Result<Option<Profile>> {
    // Ordinary shared/group-writable clones retain their existing behavior
    // until explicit enrollment. Presence requires the full private trust
    // checks; disappearance after this probe must never disable enforcement.
    match fs::symlink_metadata(common.join(PROFILE_FILE)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("Cannot determine clone-local transport policy; refused"),
        Ok(_) => {}
    }
    let bytes = private::Private::open(common)?
        .read(PROFILE_FILE)?
        .context("Transport profile changed during discovery; refused")?;
    // serde errors can quote values; do not echo a corrupt profile's content.
    let profile: Profile = serde_json::from_slice(&bytes).map_err(|_| {
        anyhow::anyhow!("Invalid clone-local transport profile; push/pull refused.")
    })?;
    profile.validate()?;
    Ok(Some(profile))
}

fn origin_endpoint(root: &Path) -> Result<String> {
    let fetch = single_endpoint(root, &["remote", "get-url", "--all", "origin"])?;
    let push = single_endpoint(root, &["remote", "get-url", "--push", "--all", "origin"])?;
    if push != fetch {
        bail!("Transport requires exactly one identical origin fetch/push endpoint.");
    }
    let mirror = git(root, &["config", "--bool", "--get", "remote.origin.mirror"])?;
    if mirror.status.code() != Some(1) && !(mirror.status.success() && mirror.stdout == b"false\n")
    {
        bail!("Transport refuses mirror or invalid origin configuration.");
    }
    // Git resolves relative filesystem URLs from the invoking worktree. Pin
    // that effective location, so moving into a nested store cannot silently
    // select a different local remote with the same configured spelling.
    let scp_style = fetch
        .split('/')
        .next()
        .is_some_and(|part| part.contains(':'));
    let endpoint = if !Path::new(&fetch).is_absolute() && scp_style {
        fetch
    } else {
        let base = if Path::new(&fetch).is_absolute() {
            PathBuf::new()
        } else {
            let bare = git_text(root, &["rev-parse", "--is-bare-repository"])?;
            PathBuf::from(git_text(
                root,
                &[
                    "rev-parse",
                    if bare == "true" {
                        "--absolute-git-dir"
                    } else {
                        "--show-toplevel"
                    },
                ],
            )?)
        };
        filesystem_endpoint(&base.join(&fetch))?
            .to_str()
            .context("Origin path must be valid UTF-8")?
            .to_owned()
    };
    Ok(endpoint)
}

// Resolve symlinks BEFORE parent components, just as Git's filesystem access
// does. Lexically popping `link/..` could select a different repository. Both
// enrollment and every origin verification use this same destination identity.
// trace:STORY-1742 | ai:codex
fn filesystem_endpoint(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(resolved) => Ok(resolved),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Plans may name a repository not created yet. Only an absent final
            // entry under a resolvable parent is unambiguous; dangling links,
            // missing intermediate components and unreadable paths refuse.
            if !matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
            {
                bail!("Cannot resolve filesystem origin safely; push/pull refused.");
            }
            let name = path
                .file_name()
                .context("Ambiguous filesystem origin; push/pull refused.")?;
            let parent = path
                .parent()
                .context("Ambiguous filesystem origin; push/pull refused.")?
                .canonicalize()
                .map_err(|_| {
                    anyhow::anyhow!("Cannot resolve filesystem origin safely; push/pull refused.")
                })?;
            Ok(parent.join(name))
        }
        Err(_) => bail!("Cannot resolve filesystem origin safely; push/pull refused."),
    }
}

// Preserve endpoint bytes rather than trimming a filesystem destination into a
// different path. Ambiguous line-based remote output refuses without echoing it.
// trace:STORY-1742 | ai:codex
fn single_endpoint(root: &Path, args: &[&str]) -> Result<String> {
    let output = git(root, args)?;
    let text = String::from_utf8(output.stdout)
        .map_err(|_| anyhow::anyhow!("Invalid origin endpoint encoding; push/pull refused."))?;
    let endpoint = text.strip_suffix('\n').unwrap_or(&text);
    if !output.status.success() || endpoint.is_empty() || endpoint.contains(['\n', '\r']) {
        bail!("Transport requires exactly one unambiguous origin endpoint.");
    }
    Ok(endpoint.to_owned())
}

fn origin_fingerprint(root: &Path) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(origin_endpoint(root)?.as_bytes())
    ))
}

fn verify_origin(root: &Path, profile: &Profile) -> Result<()> {
    if origin_fingerprint(root)? != profile.origin_sha256 {
        bail!("Origin changed since transport profile enrollment; inspect origin, then explicitly disable/re-enable the profile to repin it.");
    }
    Ok(())
}

fn commit(root: &Path, reference: &str) -> Result<Option<String>> {
    let revision = format!("{reference}^{{commit}}");
    let output = git(
        root,
        &["rev-parse", "--verify", "--end-of-options", &revision],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let sha = String::from_utf8(output.stdout).context("Invalid Git object identity")?;
    let sha = sha.trim();
    if !is_hex_id(sha, &[40, 64]) {
        bail!("Invalid Git object identity");
    }
    Ok(Some(sha.to_owned()))
}

fn enroll_baseline(root: &Path, value: &str) -> Result<String> {
    if !is_hex_id(value, &[40, 64]) {
        bail!("--baseline requires a full reviewed store commit SHA");
    }
    let sha = commit(root, value)?.context("Baseline commit is not available locally")?;
    let tip = commit(root, STORE_REF)?.context("Local aida-store commit is missing")?;
    if !git(root, &["merge-base", "--is-ancestor", &sha, &tip])?
        .status
        .success()
    {
        bail!("Baseline must be an ancestor of the local aida-store tip");
    }
    Ok(sha)
}

#[derive(Debug, Serialize)]
struct ProfileStatus<'a> {
    enabled: bool,
    profile: Option<&'a Profile>,
    remote: &'static str,
    push_refspec: &'static str,
    pull_refspec: &'static str,
    code_leg: &'static str,
    execution_available: bool,
    pull_execution_available: bool,
    sanitization_available: bool,
    scanner_version: u32,
    scan_categories: Vec<&'static str>,
    origin_matches: Option<bool>,
    scan_policy_ready: bool,
    sanitized_intake_available: bool,
    limitations: Vec<&'static str>,
}

fn show(root: &Path, profile: Option<&Profile>, json: bool) -> Result<()> {
    let status = ProfileStatus {
        enabled: profile.is_some(),
        profile,
        remote: "origin",
        push_refspec: PUSH_REFSPEC,
        pull_refspec: PULL_REFSPEC,
        code_leg: if profile.is_some() {
            "forbidden"
        } else {
            "ordinary behavior"
        },
        execution_available: profile.is_some(),
        pull_execution_available: false,
        sanitization_available: true,
        scanner_version: aida_core::transport_sanitization::SCANNER_VERSION,
        scan_categories: vec![
            "email",
            "IPv4",
            "IPv6",
            "hostname",
            "host-bearing URL",
            "reviewed deny token",
        ],
        scan_policy_ready: common_dir(root)?
            .and_then(|dir| private::Private::open(&dir).ok())
            .is_some_and(|dir| scan::Scan::load(&dir).is_ok()),
        sanitized_intake_available: false,
        origin_matches: profile
            .map(|p| origin_fingerprint(root).map(|hash| hash == p.origin_sha256))
            .transpose()?,
        limitations: vec![PENDING, SANITIZATION_PENDING, BYPASS_LIMIT],
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!(
            "Transport profile: {}",
            if status.enabled {
                "work (clone-local)"
            } else {
                "disabled; ordinary push/pull behavior"
            }
        );
        if let Some(profile) = profile {
            println!("Origin fingerprint: {}", profile.origin_sha256);
            println!(
                "Origin matches enrollment: {}",
                status.origin_matches.unwrap_or(false)
            );
            println!(
                "Baseline: {}",
                profile.baseline.as_deref().unwrap_or("not enrolled")
            );
            println!("Code leg: forbidden");
            println!("Push: origin {PUSH_REFSPEC}");
            println!("Push execution: available (clean attached store required)");
            println!("Pull plan: origin {PULL_REFSPEC} (fast-forward only)");
        }
        for limitation in status.limitations {
            println!("{limitation}");
        }
    }
    Ok(())
}

// trace:STORY-1742 | ai:codex
pub(crate) fn configure(command: &TransportCommand) -> Result<()> {
    let root = std::env::current_dir()?;
    configure_in(&root, command)
}

fn configure_in(root: &Path, command: &TransportCommand) -> Result<()> {
    let common = common_dir(root)?.context("Transport configuration requires a Git clone")?;
    if let TransportCommand::Show { json } = command {
        return show(root, load(&common)?.as_ref(), *json);
    }
    // Disable must clear any safely stored profile, including a corrupt or
    // legacy one, without parsing it (TASK-2336). Unsafe storage refuses
    // with a recovery hint rather than following or deleting it.
    // trace:STORY-1742 | ai:claude
    if let TransportCommand::Disable { json } = command {
        if let Err(error) = fs::symlink_metadata(common.join(PROFILE_FILE)) {
            if error.kind() == std::io::ErrorKind::NotFound {
                return show(root, None, *json);
            }
        }
        let _lock = configuration_lock(&common)?;
        let private = private::Private::open(&common)?;
        private.remove("aida-transport-receipt.json")?;
        private.remove(PROFILE_FILE)?;
        return show(root, None, *json);
    }
    let _lock = configuration_lock(&common)?;
    let previous = load(&common)?;
    match command {
        TransportCommand::Enable {
            name,
            baseline,
            json,
        } => {
            let fingerprint = origin_fingerprint(root)?;
            let baseline = baseline
                .as_deref()
                .map(|sha| enroll_baseline(root, sha))
                .transpose()?;
            if let Some(previous) = previous.as_ref() {
                if previous.origin_sha256 != fingerprint
                    || (baseline.is_some() && previous.baseline != baseline)
                {
                    bail!("Transport profile already enrolled; disable it explicitly before repinning origin or baseline.");
                }
                return show(root, Some(previous), *json);
            }
            let profile = Profile {
                version: 1,
                name: *name,
                origin_sha256: fingerprint,
                baseline,
            };
            private::Private::open(&common)?
                .write(PROFILE_FILE, &serde_json::to_vec_pretty(&profile)?)?;
            show(root, Some(&profile), *json)
        }
        TransportCommand::Show { .. } | TransportCommand::Disable { .. } => unreachable!(),
        TransportCommand::Policy { file, reviewed } => {
            if !reviewed || previous.is_none() {
                bail!("Enable the WORK profile and explicitly review the policy first");
            }
            let parent = file
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .context("Invalid policy source")?;
            let source = private::Private::open(parent)?;
            let bytes = source.read(name)?.context("Policy source missing")?;
            let policy: aida_core::transport_sanitization::Policy = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("Invalid policy encoding"))?;
            policy
                .validate()
                .map_err(|_| anyhow::anyhow!("Invalid or unreviewed policy"))?;
            private::Private::open(&common)?.write(scan::POLICY_FILE, &bytes)?;
            println!("Reviewed private transport policy enrolled. Future pushes rescan from the original trust root.");
            Ok(())
        }
    }
}

// Permanent sidecar: never unlink a flock inode while other writers wait.
// Execution retains it so profile edits cannot race a pinned push.
// trace:STORY-1742 | ai:codex
fn configuration_lock(common: &Path) -> Result<File> {
    private::Private::open(common)?.lock()
}

#[derive(Debug, Serialize)]
struct TransportPlan {
    profile: ProfileName,
    operation: &'static str,
    remote: &'static str,
    source_ref: &'static str,
    source_sha: Option<String>,
    destination_ref: &'static str,
    refspec: &'static str,
    refs_that_may_move: Vec<&'static str>,
    code_leg: &'static str,
    flags: BTreeMap<&'static str, &'static str>,
    execution_available: bool,
    executed: bool,
    blockers: Vec<&'static str>,
    limitations: Vec<&'static str>,
}

fn plan(
    root: &Path,
    profile: &Profile,
    push: bool,
    flags: BTreeMap<&'static str, &'static str>,
) -> Result<TransportPlan> {
    verify_origin(root, profile)?;
    let source_sha = commit(root, if push { STORE_REF } else { TRACKING_REF })?;
    let mut blockers = if push { vec![] } else { vec![PENDING] };
    if flags.values().any(|value| *value == "forbidden") {
        blockers.push(FORBIDDEN);
    }
    if source_sha.is_none() {
        blockers.push(if push {
            "Local aida-store tip is missing."
        } else {
            "Remote aida-store tip is unknown; this plan does not fetch."
        });
    }
    Ok(TransportPlan {
        profile: profile.name,
        operation: if push { "push" } else { "pull" },
        remote: "origin",
        source_ref: STORE_REF,
        source_sha,
        destination_ref: if push { STORE_REF } else { TRACKING_REF },
        refspec: if push { PUSH_REFSPEC } else { PULL_REFSPEC },
        refs_that_may_move: if push { vec![STORE_REF] } else { vec![TRACKING_REF, STORE_REF] },
        code_leg: "forbidden",
        flags,
        execution_available: push,
        executed: false,
        blockers,
        limitations: vec![BYPASS_LIMIT, SANITIZATION_PENDING, "Plans inspect local metadata only; no network, scanning, cleanliness, or fast-forward validation is performed."],
    })
}

fn render_plan(plan: &TransportPlan, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(plan)?);
    } else {
        println!("Transport profile: work (clone-local)");
        println!("{}: {} {}", plan.operation, plan.remote, plan.refspec);
        println!(
            "Source SHA (locally known): {}",
            plan.source_sha.as_deref().unwrap_or("unknown")
        );
        println!(
            "Refs that may move during execution: {}",
            plan.refs_that_may_move.join(", ")
        );
        println!("Code leg: forbidden");
        for (flag, disposition) in &plan.flags {
            println!("{flag}: {disposition}");
        }
        for note in plan.blockers.iter().chain(&plan.limitations) {
            println!("{note}");
        }
    }
    Ok(())
}

fn active(root: &Path) -> Result<Option<Profile>> {
    common_dir(root)?
        .map(|common| load(&common))
        .transpose()
        .map(Option::flatten)
}

fn handle(
    root: &Path,
    explicit_file: Option<&str>,
    push: bool,
    dry_run: bool,
    json: bool,
    flags: BTreeMap<&'static str, &'static str>,
) -> Result<bool> {
    let profile = active(root)?;
    if let Some(file) = explicit_file {
        let path = root.join(file);
        let selected = path
            .ancestors()
            .find(|path| path.is_dir())
            .context("Cannot resolve explicit store location")?;
        if profile.is_some() || active(selected)?.is_some() {
            bail!("The work transport profile refuses --file overrides; run from the intended clone without --file.");
        }
        return Ok(false);
    }
    let Some(profile) = profile else {
        return Ok(false);
    };
    let forbidden = flags.values().any(|value| *value == "forbidden");
    let plan = plan(root, &profile, push, flags)?;
    if dry_run || json {
        render_plan(&plan, json)?;
    }
    if forbidden {
        if json {
            return Err(crate::TypedPayloadEmitted.into());
        }
        bail!("{FORBIDDEN}");
    }
    if !dry_run && !json {
        for (flag, disposition) in &plan.flags {
            println!("{flag}: {disposition}");
        }
        if push {
            executor::push(root, &profile, plan.source_sha.as_deref())?;
        } else {
            bail!("{PENDING}");
        }
    }
    Ok(true)
}

// trace:STORY-1742 | ai:codex
pub(crate) fn handle_push(
    args: &crate::store_push_pull::PushArgs,
    explicit_file: Option<&str>,
) -> Result<bool> {
    let mut flags = BTreeMap::new();
    if args.code_only {
        flags.insert("--code-only", "forbidden");
    }
    if args.store_only {
        flags.insert("--store-only", "redundant");
    }
    if args.message.is_some() {
        flags.insert("--message", "forbidden");
    }
    if args.no_rebase_check {
        flags.insert("--no-rebase-check", "redundant");
    }
    if args.no_notice {
        flags.insert("--no-notice", "redundant");
    }
    handle(
        &std::env::current_dir()?,
        explicit_file,
        true,
        args.dry_run,
        args.json,
        flags,
    )
}

// trace:STORY-1742 | ai:codex
pub(crate) fn handle_pull(
    args: &crate::store_push_pull::PullArgs,
    explicit_file: Option<&str>,
) -> Result<bool> {
    let mut flags = BTreeMap::new();
    if args.code_only {
        flags.insert("--code-only", "forbidden");
    }
    if args.store_only {
        flags.insert("--store-only", "redundant");
    }
    if args.auto {
        flags.insert("--auto", "forbidden");
    }
    if args.no_gate {
        flags.insert("--no-gate", "redundant");
    }
    if args.quiet {
        flags.insert("--quiet", "redundant");
    }
    handle(
        &std::env::current_dir()?,
        explicit_file,
        false,
        args.dry_run,
        args.json,
        flags,
    )
}
