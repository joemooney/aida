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
//! origin), the push is not a deletion, the sha equals origin's live tip
//! (ls-remote, not the local tracking ref), and the
//! attestation names exactly that remote, sha and ref. The attestation alone
//! never allows anything, so a hand-set variable cannot widen the guard.
// trace:BUG-2015 | ai:claude

use anyhow::{Context, Result};
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

/// AIDA's opt-in main-guard pre-push hook: refuses any push to the default
/// branch unless [`check_mirror_sync_push`] allows it.
// trace:BUG-2015 | ai:claude
pub(crate) fn main_guard_hook_script() -> &'static str {
    aida_core::templates::EMBEDDED_TEMPLATES["hooks/aida-main-guard-pre-push.sh"]
}

/// The guard's own file in the hooks directory. Not a git hook name, so git
/// only runs it through the chain block in `pre-push`.
// trace:BUG-2015 | ai:claude
pub(crate) const GUARD_FILE: &str = "aida-main-guard";

/// First line of the chain block; marks a pre-push hook that runs the guard.
// trace:BUG-2015 | ai:claude
pub(crate) const GUARD_CHAIN_MARKER: &str =
    "# aida main guard — chained by `aida remote mirror-guard-check --install-guard`.";

/// The block placed right after a pre-push hook's shebang: run the guard on
/// the ref lines, stop the push when it refuses, then hand the same ref
/// lines back on stdin so the rest of the hook (the mirror fan-out, or the
/// operator's own checks) still sees them.
// trace:BUG-2015 | ai:claude
fn guard_chain_block() -> String {
    format!(
        "{GUARD_CHAIN_MARKER}\n\
         aida_guard_refs=$(cat)\n\
         printf '%s\\n' \"$aida_guard_refs\" | \"$(dirname \"$0\")/{GUARD_FILE}\" \"$@\" || exit 1\n\
         exec 0<<AIDA_GUARD_REFS\n\
         $aida_guard_refs\n\
         AIDA_GUARD_REFS\n\
         # end aida main guard\n"
    )
}

/// Whether a shebang line names a POSIX-family shell (directly or through
/// `env`): the only interpreters that parse the sh chain block.
// trace:TASK-2164 | ai:claude
fn is_sh_shebang(first: &str) -> bool {
    let mut words = first.trim_start_matches("#!").split_whitespace();
    let mut prog = words.next().unwrap_or_default();
    if prog.rsplit('/').next() == Some("env") {
        prog = words.find(|w| !w.starts_with('-')).unwrap_or_default();
    }
    matches!(
        prog.rsplit('/').next(),
        Some("sh" | "bash" | "dash" | "zsh" | "ksh")
    )
}

/// `hook` with the guard chained in front of it (idempotent). Refuses a hook
/// whose shebang is not a shell: the sh chain block would be a syntax error
/// there and fail every push.
// trace:BUG-2015 trace:TASK-2164 | ai:claude
pub(crate) fn chain_guard_into(hook: &str) -> Result<String> {
    if hook.contains(GUARD_CHAIN_MARKER) {
        return Ok(hook.to_string());
    }
    let (first, rest) = hook.split_once('\n').unwrap_or((hook, ""));
    if !first.starts_with("#!") {
        return Ok(format!("#!/bin/sh\n{}{hook}", guard_chain_block()));
    }
    if !is_sh_shebang(first) {
        anyhow::bail!(
            "the pre-push hook starts with `{first}`, which is not a shell script — the main \
             guard chains in as sh code, so it cannot go in front of it. Add the guard call \
             to that hook yourself (docs/multi-hub-sync.md)"
        );
    }
    Ok(format!("{first}\n{}{rest}", guard_chain_block()))
}

/// Write `body` to `path` as an executable hook.
fn write_hook(path: &Path, body: &str) -> Result<()> {
    std::fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))?;
    aida_core::git_ops::set_hook_mode(path)
        .with_context(|| format!("failed to make {} executable", path.display()))
}

/// `aida remote mirror-guard-check --install-guard [--force]`: install the
/// guard as its own file and chain it in front of `pre-push`. A missing hook
/// gets a pre-push that runs only the guard; AIDA's mirror fan-out hook is
/// chained into; a foreign hook is left alone unless `force`. Idempotent.
// trace:BUG-2015 | ai:claude
pub fn install_main_guard(project_root: &Path, force: bool) -> Result<()> {
    let check = crate::glyph(crate::glyphs::Glyph::Check);
    let hooks_dir = crate::remote_create::repo_hooks_dir(project_root);
    std::fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("failed to create {}", hooks_dir.display()))?;
    let pre_push = hooks_dir.join("pre-push");
    let existing = std::fs::read_to_string(&pre_push).ok();
    let chained = match existing.as_deref() {
        None => chain_guard_into("#!/bin/sh\nexit 0\n")?,
        Some(body) if body.contains(GUARD_CHAIN_MARKER) => body.to_string(),
        Some(body) if body.contains(crate::remote_create::MIRROR_HOOK_HEADER) || force => {
            // trace:TASK-2164 | ai:claude
            chain_guard_into(body).map_err(|e| anyhow::anyhow!("{} — {e}", pre_push.display()))?
        }
        Some(_) => anyhow::bail!(
            "a custom pre-push hook exists at {} — not changing it. Re-run with --force to \
             chain the main guard in front of it, or add the guard call to it yourself \
             (docs/multi-hub-sync.md)",
            pre_push.display()
        ),
    };
    write_hook(&hooks_dir.join(GUARD_FILE), main_guard_hook_script())?;
    if existing.as_deref() != Some(chained.as_str()) {
        write_hook(&pre_push, &chained)?;
    }
    println!(
        "{check} main guard installed: {} runs {} first",
        pre_push.display(),
        GUARD_FILE
    );
    Ok(())
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
    // The guard's own resolver: origin/HEAD minus `origin/`, never a guess.
    // trace:TASK-2163 | ai:claude
    let branch = aida_core::git_ops::origin_default_branch(project_root).ok_or_else(|| {
        "cannot tell origin's default branch (refs/remotes/origin/HEAD is unset) — run \
         `git remote set-head origin --auto`"
            .to_string()
    })?;
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
    // Origin's live tip (ls-remote), never the local tracking ref: a stale
    // `refs/remotes/origin/<branch>` must not vouch for a superseded sha.
    let origin_tip = crate::remote_create::remote_branch_head(project_root, "origin", &branch)
        .ok_or_else(|| format!("origin's {branch} is unreachable — cannot confirm the tip"))?;
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
