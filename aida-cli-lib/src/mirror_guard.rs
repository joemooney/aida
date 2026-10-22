//! The narrowly scoped allowance a project's pre-push main guard consults
//! before refusing a push to the default branch.
//!
//! A repo with no server-side branch protection guards its default branch
//! with a project-local pre-push hook that refuses ANY push to it. `aida
//! remote mirror-sync` pushes origin's default-branch tip to each registered
//! mirror hub, so the guard refused it too, and mirrors fell behind. That tip
//! already passed PR, CI and the merge hold on origin.
//!
//! mirror-sync attests each push in [`MIRROR_SYNC_ATTEST_ENV`]; the guard
//! pipes its ref lines to `aida remote mirror-guard-check <remote>` and
//! allows the push only when [`check_mirror_sync_push`] confirms every
//! default-branch ref independently: the remote is a registered mirror (never
//! origin), the push is not a deletion, the sha equals origin's tip, and the
//! attestation names exactly that remote, sha and ref. The attestation alone
//! never allows anything, so a hand-set variable cannot widen the guard.
// trace:BUG-2015 | ai:claude

use anyhow::Result;
use std::path::Path;

/// Environment variable `aida remote mirror-sync` sets on each mirror push:
/// one `<mirror> <sha> refs/heads/<branch>` line naming that push exactly.
// trace:BUG-2015 | ai:claude
pub(crate) const MIRROR_SYNC_ATTEST_ENV: &str = "AIDA_MIRROR_SYNC_ATTEST";

/// The attestation line for one mirror-sync push.
// trace:BUG-2015 | ai:claude
pub(crate) fn mirror_sync_attestation(mirror: &str, sha: &str, branch: &str) -> String {
    format!("{mirror} {sha} refs/heads/{branch}")
}

/// Resolve `refspec` to a commit sha in `repo`, or None.
fn rev_parse(repo: &Path, refspec: &str) -> Option<String> {
    let out = aida_core::git_ops::host_git_command(repo)
        .args(["rev-parse", "--verify", "--quiet", refspec])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Decide whether a push of `ref_lines` (the pre-push hook's stdin) to
/// `remote` is an attested mirror-sync of origin's default-branch tip. Only
/// refs aimed at the default branch are judged; every one of them must pass,
/// and a push with none is refused (there is nothing to allow). `Err` says
/// why the push was refused.
// trace:BUG-2015 | ai:claude
pub(crate) fn check_mirror_sync_push(
    project_root: &Path,
    remote: &str,
    ref_lines: &str,
    attestation: Option<&str>,
) -> std::result::Result<(), String> {
    let branch = aida_core::git_ops::default_branch_name(project_root);
    let protected = format!("refs/heads/{branch}");
    let judged: Vec<(&str, &str)> = ref_lines
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let _local_ref = it.next()?;
            let local_sha = it.next()?;
            let remote_ref = it.next()?;
            (remote_ref == protected).then_some((local_sha, remote_ref))
        })
        .collect();
    if judged.is_empty() {
        return Err(format!(
            "no push to {protected} in the ref list — nothing to allow"
        ));
    }
    if remote == "origin" {
        return Err("a push to origin's default branch is never a mirror sync".into());
    }
    let cfg = crate::read_store_sync_config(project_root).unwrap_or_default();
    if !cfg.mirror_remotes.iter().any(|m| m == remote)
        || !aida_core::git_ops::has_remote(project_root, remote)
    {
        return Err(format!(
            "`{remote}` is not a registered mirror ([store.sync] mirror_remotes)"
        ));
    }
    let origin_tip = rev_parse(
        project_root,
        &format!("refs/remotes/origin/{branch}^{{commit}}"),
    )
    .ok_or_else(|| format!("origin/{branch} is unknown here — cannot confirm the tip"))?;
    let attested: Vec<&str> = attestation
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .collect();
    for (sha, remote_ref) in judged {
        if sha.chars().all(|c| c == '0') {
            return Err(format!(
                "deleting {remote_ref} on `{remote}` is not a mirror sync"
            ));
        }
        if sha != origin_tip {
            return Err(format!(
                "{sha} is not origin/{branch}'s tip ({origin_tip}) — only origin's reviewed tip may be mirrored"
            ));
        }
        let expected = format!("{remote} {sha} {remote_ref}");
        if !attested.contains(&expected.as_str()) {
            return Err(format!(
                "this push was not attested by `aida remote mirror-sync` ({MIRROR_SYNC_ATTEST_ENV})"
            ));
        }
    }
    Ok(())
}

/// `aida remote mirror-guard-check <remote>`: read the pre-push ref lines
/// from stdin, exit 0 when [`check_mirror_sync_push`] allows them, else say
/// why on stderr and exit non-zero (so a guard that cannot run this keeps
/// refusing).
// trace:BUG-2015 | ai:claude
pub fn handle_mirror_guard_check(project_root: &Path, remote: &str) -> Result<()> {
    let mut ref_lines = String::new();
    use std::io::Read;
    std::io::stdin().lock().read_to_string(&mut ref_lines)?;
    let attestation = std::env::var(MIRROR_SYNC_ATTEST_ENV).ok();
    match check_mirror_sync_push(project_root, remote, &ref_lines, attestation.as_deref()) {
        Ok(()) => {
            eprintln!(
                "mirror-guard-check: allowed — `{remote}` receives origin's reviewed tip (aida remote mirror-sync)"
            );
            Ok(())
        }
        Err(why) => anyhow::bail!("mirror-guard-check: refused — {why}"),
    }
}
