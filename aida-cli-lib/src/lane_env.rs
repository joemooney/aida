//! STORY-1662 PR2: lane identity from the worktree lease, for sessions whose
//! environment is not their own.
//!
//! A `claude --bg` session runs under the long-lived `claude daemon`, so its
//! environment is the daemon's: often the advisor's `AIDA_SESSION_ROLE`,
//! grant and purpose, whatever the launching command set. The lane's
//! worktree lease, which the advisor wrote with `aida worktree add <SPEC>
//! --role implementer`, is the record of who the lane is. This module reads
//! it and computes the correction the SessionStart hook writes to
//! `$CLAUDE_ENV_FILE` (via `aida internal lane-env`) and the MCP server
//! applies to itself at start.
//!
//! It corrects the agent's commands; it is not an authority boundary. Grants
//! are still validated wherever they are used.
// trace:STORY-1662 | ai:claude

use crate::*;
use std::path::Path;

/// The variables a lane correction may touch. ONE list: the renderer only
/// emits these, and the hook accepts only these.
pub(crate) const ROLE_ENV: &str = "AIDA_SESSION_ROLE";
pub(crate) const PURPOSE_ENV: &str = "AIDA_SESSION_PURPOSE";

/// The longest role or grant value a lane line may carry.
const MAX_VALUE_LEN: usize = 64;

/// What to do with `AIDA_SESSION_GRANT` in a lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GrantFix {
    /// Export the lease's own child grant.
    Export(String),
    /// Drop the inherited grant: it is not for the lease's seat, or it does
    /// not validate.
    Unset,
    /// The inherited grant is for the lease's seat; leave it.
    Keep,
}

/// The correction for a session inside a lane worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaneFix {
    pub(crate) role: String,
    pub(crate) grant: GrantFix,
    /// Unset `AIDA_SESSION_PURPOSE`: the inherited role was not the lane's.
    pub(crate) unset_purpose: bool,
}

/// What the inherited grant (`AIDA_SESSION_GRANT`) is, as far as a lane
/// cares: absent, a valid grant for a seat, or present but invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InheritedGrant {
    Absent,
    Valid { id: String, seat: String },
    Invalid,
}

/// A role or grant value safe to quote into a shell line: ASCII letters,
/// digits, `-` and `_`, at most [`MAX_VALUE_LEN`] long. Anything else (shell
/// metacharacters, quotes, whitespace, newlines) is refused.
pub(crate) fn validate_value(what: &str, value: &str) -> Result<String> {
    let ok = !value.is_empty()
        && value.len() <= MAX_VALUE_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !ok {
        anyhow::bail!("lane {what} {value:?} is not a plain name; refusing to emit it");
    }
    Ok(value.to_string())
}

/// The lane lease covering `cwd`: a worktree lease (not an `aida claim`,
/// review or MCP claim marker, whose path is informational) whose worktree is
/// not the primary checkout.
pub(crate) fn lane_lease_for_cwd(project_root: &Path, cwd: &Path) -> Option<SessionLease> {
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let main_root = crate::main_worktree_root_from(project_root);
    let main = main_root
        .canonicalize()
        .unwrap_or_else(|_| main_root.clone());
    let mcp_claims = crate::mcp_claim_lease_ids(&main_root);
    list_leases(&main_root)
        .into_iter()
        .filter(|l| !crate::lease_is_advisory(l, &mcp_claims))
        .filter(|l| l.worktree_path != main)
        .filter(|l| crate::lease_covers_cwd(l, &canon))
        // The innermost worktree wins when one nests in another.
        .max_by_key(|l| l.worktree_path.as_os_str().len())
}

/// The lane role a lease records, validated and canonicalized. `None` when
/// the lease names no role.
pub(crate) fn lease_role(lease: &SessionLease) -> Result<Option<String>> {
    match lease.role.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(role) => {
            validate_value("role", role)?;
            Ok(Some(crate::canonical_role_name(role)))
        }
    }
}

/// The correction for a session in a lane whose lease records `lease_role`
/// (and optionally a child grant), given the inherited role label and grant.
/// `None` when the environment is already the lane's.
pub(crate) fn fix_for(
    lease_role: &str,
    lease_child_grant: Option<&str>,
    env_role: Option<&str>,
    inherited: &InheritedGrant,
) -> Result<Option<LaneFix>> {
    let role = crate::canonical_role_name(&validate_value("role", lease_role)?);
    let env_role = env_role
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(crate::canonical_role_name);
    let role_differs = env_role.as_deref() != Some(role.as_str());
    let grant = match (
        lease_child_grant.map(str::trim).filter(|g| !g.is_empty()),
        inherited,
    ) {
        (Some(child), InheritedGrant::Valid { id, .. }) if id == child => GrantFix::Keep,
        (Some(child), _) => GrantFix::Export(validate_value("grant", child)?),
        (None, InheritedGrant::Absent) => GrantFix::Keep,
        (None, InheritedGrant::Valid { seat, .. }) if crate::canonical_role_name(seat) == role => {
            GrantFix::Keep
        }
        (None, _) => GrantFix::Unset,
    };
    if !role_differs && grant == GrantFix::Keep {
        return Ok(None);
    }
    Ok(Some(LaneFix {
        role,
        grant,
        unset_purpose: role_differs,
    }))
}

/// The shell lines for `fix`: only `AIDA_SESSION_*` variables, values
/// single-quoted (and already validated as plain names).
pub(crate) fn render(fix: &LaneFix) -> String {
    let grant_env = crate::seat_authority::GRANT_ENV;
    let mut out = format!("export {ROLE_ENV}='{}'\n", fix.role);
    match &fix.grant {
        GrantFix::Export(id) => out.push_str(&format!("export {grant_env}='{id}'\n")),
        GrantFix::Unset => out.push_str(&format!("unset {grant_env}\n")),
        GrantFix::Keep => {}
    }
    if fix.unset_purpose {
        out.push_str(&format!("unset {PURPOSE_ENV}\n"));
    }
    out
}

/// The inherited grant, classified against the store.
fn inherited_grant(project_root: &Path) -> InheritedGrant {
    match crate::seat_authority::grant_check(project_root) {
        crate::seat_authority::GrantCheck::Absent => InheritedGrant::Absent,
        crate::seat_authority::GrantCheck::Valid(grant) => InheritedGrant::Valid {
            id: grant.id,
            seat: grant.seat,
        },
        crate::seat_authority::GrantCheck::Rejected { .. } => InheritedGrant::Invalid,
    }
}

/// The correction for a session at `cwd`, read from its lane lease. `None`
/// outside a lane or when nothing needs changing; an error when the lease
/// cannot be turned into safe lines.
pub(crate) fn fix_at(project_root: &Path, cwd: &Path) -> Result<Option<LaneFix>> {
    let Some(lease) = lane_lease_for_cwd(project_root, cwd) else {
        return Ok(None);
    };
    let Some(role) = lease_role(&lease)? else {
        return Ok(None);
    };
    fix_for(
        &role,
        lease.child_grant_id.as_deref(),
        std::env::var(ROLE_ENV).ok().as_deref(),
        &inherited_grant(project_root),
    )
}

/// `aida internal lane-env [--cwd PATH]`: print the correction lines, or
/// nothing. Read-only. On any error it exits 1 with the reason on stderr and
/// nothing on stdout (not the agent-mode error block, which goes to stdout),
/// so the hook fails closed.
pub(crate) fn handle_lane_env(cwd: Option<&Path>) -> Result<()> {
    match lane_env_lines(cwd) {
        Ok(lines) => {
            print!("{lines}");
            Ok(())
        }
        Err(e) => {
            eprintln!("aida internal lane-env: {e:#}");
            std::process::exit(1)
        }
    }
}

fn lane_env_lines(cwd: Option<&Path>) -> Result<String> {
    let cwd = match cwd {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().context("reading the current directory")?,
    };
    let root = crate::find_project_root_from(&cwd)
        .with_context(|| format!("no AIDA project at {}", cwd.display()))?;
    Ok(fix_at(&root, &cwd)?
        .map(|fix| render(&fix))
        .unwrap_or_default())
}

/// Whether `cwd` is inside a linked git worktree (not the primary checkout).
/// The fail-closed test when the lease cannot be read.
pub(crate) fn in_linked_worktree(cwd: &Path) -> bool {
    let rev_parse = |arg: &str| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["rev-parse", "--path-format=absolute", arg])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    match (rev_parse("--git-dir"), rev_parse("--git-common-dir")) {
        (Some(dir), Some(common)) => dir != common,
        _ => false,
    }
}

/// Signoff C2: the MCP server a `--bg` session spawns inherits the daemon
/// environment too, so it applies the same correction to itself at start.
/// Fails closed: on an error inside a linked worktree it drops the grant and
/// purpose and sets no role.
pub(crate) fn apply_in_process(project_root: &Path, cwd: &Path) {
    let grant_env = crate::seat_authority::GRANT_ENV;
    match fix_at(project_root, cwd) {
        Ok(None) => {}
        Ok(Some(fix)) => {
            std::env::set_var(ROLE_ENV, &fix.role);
            match &fix.grant {
                GrantFix::Export(id) => std::env::set_var(grant_env, id),
                GrantFix::Unset => std::env::remove_var(grant_env),
                GrantFix::Keep => {}
            }
            if fix.unset_purpose {
                std::env::remove_var(PURPOSE_ENV);
            }
        }
        Err(e) => {
            if in_linked_worktree(cwd) {
                eprintln!(
                    "AIDA MCP: lane identity unreadable ({e:#}); dropping the inherited grant"
                );
                std::env::remove_var(grant_env);
                std::env::remove_var(PURPOSE_ENV);
            }
        }
    }
}

/// Signoff C4: the operator-side root fix is to start the claude daemon from a
/// shell with no `AIDA_SESSION_*` env. The pids under `proc_root` (`/proc`)
/// whose command line is `claude daemon ...` and whose environment holds a
/// non-empty `AIDA_SESSION_GRANT`: every `claude --bg` session they host
/// starts with that grant. Unreadable entries are skipped; empty off Linux.
pub(crate) fn daemons_holding_a_grant(proc_root: &Path) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let grant_prefix = format!("{}=", crate::seat_authority::GRANT_ENV);
    let mut pids: Vec<u32> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let dir = proc_root.join(pid.to_string());
            let Ok(cmdline) = std::fs::read(dir.join("cmdline")) else {
                return false;
            };
            let argv: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
            let is_claude = |arg: &[u8]| arg.rsplit(|b| *b == b'/').next() == Some(b"claude");
            // An interpreter-run stand-in shifts the argv one slot.
            let is_daemon = (0..2).any(|i| {
                argv.get(i).is_some_and(|a| is_claude(a))
                    && argv.get(i + 1).is_some_and(|a| *a == b"daemon")
            });
            is_daemon
                && std::fs::read(dir.join("environ")).is_ok_and(|env| {
                    env.split(|b| *b == 0).any(|var| {
                        var.strip_prefix(grant_prefix.as_bytes())
                            .is_some_and(|value| !value.is_empty())
                    })
                })
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// Record `role` on lease `lease_id`/// Record `role` on lease `lease_id` (an existing worktree re-entered by
/// `aida worktree add --role`).
pub(crate) fn stamp_lease_role_at(project_root: &Path, lease_id: &str, role: &str) -> Result<()> {
    use toml_edit::{value, DocumentMut};
    let path = crate::lease_path(&crate::main_worktree_root_from(project_root), lease_id);
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading lease {}", path.display()))?;
    let mut doc = body
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing lease {}", path.display()))?;
    doc["role"] = value(role);
    aida_core::write_atomic(&path, doc.to_string())
        .with_context(|| format!("writing lease {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_1662_lane_env_tests.rs"]
mod story_1662_lane_env_tests;
