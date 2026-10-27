//! One pinned store push or fast-forward pull, without entering the ordinary
//! synchronization path.
// trace:STORY-1742 | ai:codex

use super::*;

#[path = "transport_pull.rs"]
mod pull;
pub(super) use pull::pull;

/// Command-scoped remote that only defeats push tracking-ref inference.
const UNTRACKED_REMOTE: &str = "aida-transport-untracked";

fn attached_store(root: &Path, common: &Path) -> Result<PathBuf> {
    let output = git(root, &["worktree", "list", "--porcelain", "-z"])?;
    if !output.status.success() {
        bail!("Cannot inspect attached store worktrees; push/pull refused.");
    }
    let mut path = None;
    let mut candidates = Vec::new();
    for field in output.stdout.split(|byte| *byte == 0) {
        if let Some(value) = field.strip_prefix(b"worktree ") {
            path = std::str::from_utf8(value).ok().map(PathBuf::from);
        } else if field == b"branch refs/heads/aida-store" {
            candidates.push(path.take().context("Invalid store worktree metadata")?);
        } else if field.is_empty() {
            path = None;
        }
    }
    if candidates.len() != 1 {
        bail!("Transport push/pull requires exactly one existing, clean, attached aida-store worktree; attach it explicitly before retrying.");
    }
    let store = candidates.remove(0);
    if common_dir(&store)?.as_deref() != Some(common) || !aida_core::git_ops::is_store_tree(&store)
    {
        bail!("Attached store identity is unsupported; push/pull refused.");
    }
    Ok(store)
}

fn check_store_state(store: &Path, expected: &str) -> Result<()> {
    if git_text(store, &["symbolic-ref", "-q", "HEAD"])? != STORE_REF
        || commit(store, STORE_REF)?.as_deref() != Some(expected)
    {
        bail!("Store tip or attachment changed during transport planning; push/pull refused. Retry after inspection.");
    }
    let git_dir = PathBuf::from(git_text(store, &["rev-parse", "--absolute-git-dir"])?);
    for marker in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "rebase-apply",
        "rebase-merge",
        "sequencer",
        "index.lock",
    ] {
        match fs::symlink_metadata(git_dir.join(marker)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => bail!("Store has an in-progress or uncertain Git operation; push/pull refused."),
        }
    }
    Ok(())
}

// Freeze the resolved endpoint rather than passing an origin alias that could
// fan out after enrollment. A second URL rewrite must not redirect that pin.
// trace:STORY-1742 | ai:codex
fn pinned_endpoint(root: &Path, profile: &Profile) -> Result<String> {
    let endpoint = origin_endpoint(root)?;
    if format!("{:x}", Sha256::digest(endpoint.as_bytes())) != profile.origin_sha256 {
        bail!("Origin changed since transport profile enrollment; push/pull refused.");
    }
    refuse_endpoint_rewrites(root, &endpoint)?;
    Ok(endpoint)
}

// trace:STORY-1742 | ai:codex
fn refuse_endpoint_rewrites(root: &Path, endpoint: &str) -> Result<()> {
    if single_endpoint(
        root,
        &["ls-remote", "--get-url", "--end-of-options", endpoint],
    )? != endpoint
    {
        bail!("Origin has chained or unstable URL rewrites; push/pull refused.");
    }
    // ls-remote --get-url expands insteadOf without any network operation.
    // It is fetch-oriented, so separately refuse a pushInsteadOf prefix that
    // could rewrite the already resolved explicit destination a second time.
    let rewrites = git(
        root,
        &[
            "config",
            "--null",
            "--get-regexp",
            "^url\\..*\\.pushinsteadof$",
        ],
    )?;
    if !rewrites.status.success() && rewrites.status.code() != Some(1) {
        bail!("Cannot inspect origin URL rewrites; push/pull refused.");
    }
    for entry in rewrites
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let split = entry
            .iter()
            .position(|byte| *byte == b'\n')
            .context("Ambiguous origin URL rewrites; push/pull refused.")?;
        if !entry[..split].ends_with(b".pushinsteadof")
            || endpoint.as_bytes().starts_with(&entry[split + 1..])
        {
            bail!("Origin has chained or unstable URL rewrites; push/pull refused.");
        }
    }
    Ok(())
}

// Only static, locally authored diagnostics escape this preflight. Core lock
// and hook setup failures are handled separately from these refusal reasons.
// trace:STORY-1742 | ai:codex
fn store_preflight(store: &Path, expected: &str) -> Result<()> {
    check_store_state(store, expected)?;
    let status = aida_core::git_ops::host_git_command(store)
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .output()
        .map_err(|_| anyhow::anyhow!("Cannot inspect store cleanliness; push/pull refused."))?;
    if !status.status.success() {
        bail!("Cannot verify store cleanliness or store hooks; push/pull refused.");
    }
    if !status.stdout.is_empty() {
        bail!(
            "Store is dirty; push/pull refused. Commit or resolve it explicitly before retrying."
        );
    }
    aida_core::git_ops::refuse_embedded_repos_for_push(store, expected).map_err(|_| {
        anyhow::anyhow!("Store contains unsupported repository content; push/pull refused.")
    })?;
    Ok(())
}

// Tree inspection must never lazily fetch from a promisor remote. Partial
// clones need an explicit hydration step outside this ref-only executor.
// trace:STORY-1742 | ai:codex
fn refuse_partial_clone(root: &Path) -> Result<()> {
    let partial = git(
        root,
        &[
            "config",
            "--get-regexp",
            "^(extensions\\.partialclone|remote\\..*\\.promisor)$",
        ],
    )?;
    if partial.status.code() != Some(1) {
        bail!("Partial or uncertain clone configuration requires explicit hydration outside transport; push/pull refused.");
    }
    Ok(())
}

// trace:STORY-1742 | ai:codex
pub(super) fn push(root: &Path, profile: &Profile, expected: Option<&str>) -> Result<()> {
    let common = common_dir(root)?.context("Transport push requires a Git clone")?;
    let _configuration_lock = configuration_lock(&common)?;
    let private = private::Private::open(&common)?;
    if load(&common)?.as_ref() != Some(profile) {
        bail!("Transport profile changed during planning; push refused.");
    }
    let expected = expected.context("Local aida-store tip is missing; push refused.")?;
    pull::refuse_unfinished(&private)?;
    refuse_partial_clone(root)?;
    let endpoint = pinned_endpoint(root, profile)?;
    let store = attached_store(root, &common)?;
    let scan = scan::Scan::load(&private)?;
    // The destination is already absolute/frozen in the invoking worktree.
    // Do not reinterpret a relative origin from the nested store checkout.
    refuse_endpoint_rewrites(&store, &endpoint)?;
    // Pin and validate under the existing writer lock. Do not keep that lock
    // across network IO or hooks: the shipped identity hook is a read-only CLI.
    check_store_state(&store, expected)?;
    aida_core::db::with_store_write_lock(&store, || {
        Ok(store_preflight(&store, expected).and_then(|()| scan.history(&store, profile, expected)))
    })
        .map_err(|_| anyhow::anyhow!("Store writer lock or verified hook profile unavailable; push refused. Check for a busy writer, lock permissions, or unsupported hooks before retrying."))??;

    if aida_core::db::any_store_write_lock_held() {
        bail!("Transport cannot push while a store writer lock is held.");
    }
    if pinned_endpoint(root, profile)? != endpoint {
        bail!("Origin changed during store preflight; push refused.");
    }
    scan.revalidate(&private)?;
    if load(&common)?.as_ref() != Some(profile) {
        bail!("Transport profile changed; push refused");
    }
    check_store_state(&store, expected)?;
    let refspec = format!("{expected}:{STORE_REF}");
    let (destination, credentials) = aida_core::git_ops::split_http_url_userinfo(&endpoint);
    if credentials.is_some() && endpoint.trim() != endpoint {
        bail!("Ambiguous credentialed origin endpoint; push refused.");
    }
    // Removing HTTP userinfo must not expose a different rewrite rule. The
    // scheme/host/path still belong to the enrolled endpoint, never an alias.
    refuse_endpoint_rewrites(&store, &destination)?;
    // Git 2.56 (upstream merge 24e4e2ef8b, hn/url-push-tracking) added
    // repo_remote_for_push_tracking(): when the push destination is an
    // unconfigured URL with exactly one push URL, git scans the configured
    // remotes, and if exactly one lists that string verbatim among its push
    // URLs (remote.<name>.pushurl, else remote.<name>.url), transport_push
    // updates tracking refs through that remote's fetch refspecs. Two or more
    // matches fall back to the URL itself, which has no fetch refspec. Our
    // destination is origin's URL, so a store-only push would otherwise write
    // refs/remotes/origin/aida-store. A command-scoped remote whose url is the
    // same string makes the match ambiguous. Pre-existing config under the
    // reserved name (e.g. a pushurl, which replaces url in the match) could
    // restore a unique match, so any such key refuses the push.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-007 | ai:claude
    let untracked = git(
        &store,
        &[
            "config",
            "--get-regexp",
            &format!("^remote\\.{UNTRACKED_REMOTE}\\."),
        ],
    )?;
    if untracked.status.code() != Some(1) {
        bail!("Reserved transport remote configuration is present; push refused.");
    }
    // Exactly one attempt. Explicit SHA/refspec, endpoint, tag/submodule flags
    // defeat implicit code refs, remote refspecs and follow-tag configuration.
    // host_git_command preserves the verified store-only identity hook profile.
    let mut push = aida_core::git_ops::host_git_command(&store);
    push.args(aida_core::git_ops::NO_AUTO_MAINTENANCE)
        .args(["-c", "push.negotiate=false"])
        .arg("-c")
        .arg(format!("remote.{UNTRACKED_REMOTE}.url={destination}"))
        .env("GIT_NO_LAZY_FETCH", "1")
        .args([
            "push",
            "--no-follow-tags",
            "--recurse-submodules=no",
            "--porcelain",
            "--end-of-options",
            &destination,
            &refspec,
        ]);
    // Route the frozen endpoint's credentials, including GitLab fallback,
    // through the shared environment-backed helper. Never re-resolve origin
    // here or put HTTP secrets in the git/transport-helper argument vectors.
    crate::gitlab_project_create::configure_git_remote_credentials(
        &mut push,
        &store,
        &endpoint,
        Some(&endpoint),
    );
    // Own runner (trace:STORY-1742 | ai:claude): no terminal prompts, Ctrl+C
    // forwarded to git, and no wall-clock kill of a long first transfer.
    let (status, stdout, interrupted) = scan::publish(push, 65536).map_err(|_| {
        anyhow::anyhow!("Store push execution or settlement is uncertain; receipt unchanged. Inspect origin before retrying.")
    })?;
    if interrupted {
        bail!("Store push was interrupted; remote acceptance is uncertain and the receipt is unchanged. Inspect origin before retrying.");
    }
    if !status.success() {
        // Hook and remote diagnostics can contain emails, URLs or hostnames.
        bail!("Store push failed or was refused by a hook/remote; no retry was attempted. Remote acceptance may be uncertain; inspect origin and store policy before retrying. Terminal prompts are disabled: configure a credential helper or SSH agent (and known host key) for origin.");
    }
    // An exit status alone does not attest which ref was accepted. Require
    // exactly the inspected SHA/ref's successful porcelain status row.
    let expected_row = format!("{expected}:{STORE_REF}");
    let rows: Vec<_> = stdout
        .split(|b| *b == b'\n')
        .filter(|line| line.len() > 1 && line[1] == b'\t')
        .collect();
    if rows.len() != 1
        || !matches!(rows[0][0], b' ' | b'*' | b'=')
        || rows[0].split(|b| *b == b'\t').nth(1) != Some(expected_row.as_bytes())
    {
        bail!("Store publication may have succeeded, but exact acceptance evidence is unavailable; receipt unchanged. Inspect origin before retrying.");
    }
    if pinned_endpoint(root, profile).ok().as_deref() != Some(endpoint.as_str())
        || load(&common).ok().flatten().as_ref() != Some(profile)
        || scan.receipt(&private, profile, expected).is_err()
    {
        bail!("Store publication succeeded for the inspected tip, but durable receipt recording could not be confirmed; inspect origin before retrying.");
    }
    println!("Pushed aida-store {expected} to pinned origin (store ref only).");
    println!("Refusal-only history scan passed; exact successful-push receipt recorded.");
    Ok(())
}
