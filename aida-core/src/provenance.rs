//! Filing provenance capture (CR-8).
//!
//! Every new requirement is stamped, once, with the code state and tooling it
//! was filed against — see [`FilingProvenance`]. This module is the single
//! capture point. The store write paths call [`stamp_if_absent`] on a
//! requirement that has never been written, so every creation route (`aida
//! add`, `aida add --queue`, MCP `add_requirement`, finding/report filing, …)
//! is covered without each call site remembering to do it.
//!
//! Everything here is best-effort: a field that cannot be detected is left
//! `None`, and no failure ever propagates to the caller — provenance must
//! never block or noticeably slow a filing. No network is touched; the only
//! subprocess is one local `git status`.
//!
//! The binary's version and build SHA are compile-time facts of the *binary*
//! (`aida-cli-lib`'s build script), not of this library, so the binary
//! registers them once at startup via [`register_tool_identity`].
// trace:CR-8 | ai:claude

use crate::models::{FilingProvenance, Requirement};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The filing tool's identity, registered once by the binary at startup.
#[derive(Debug, Clone, Default)]
pub struct ToolIdentity {
    /// Package version of the running `aida` binary.
    pub version: Option<String>,
    /// Git SHA the running binary was built from.
    pub build_sha: Option<String>,
    /// Filing agent vendor (`claude` / `codex` / …), when detectable.
    pub vendor: Option<String>,
}

static TOOL_IDENTITY: OnceLock<ToolIdentity> = OnceLock::new();

/// Register the running binary's identity. First registration wins; later
/// calls are ignored (the identity of one process never changes).
pub fn register_tool_identity(identity: ToolIdentity) {
    let _ = TOOL_IDENTITY.set(identity);
}

fn tool_identity() -> ToolIdentity {
    TOOL_IDENTITY
        .get()
        .cloned()
        .unwrap_or_else(|| ToolIdentity {
            // Unregistered (library use, tests): the workspace shares one
            // version, so the library's own package version is still truthful.
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            build_sha: None,
            vendor: None,
        })
}

fn nonempty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Filing session id from the launch-path env (`AIDA_SESSION_ID`, falling
/// back to Claude Code's own `CLAUDE_CODE_SESSION_ID`).
fn session_from_env() -> Option<String> {
    ["AIDA_SESSION_ID", "CLAUDE_CODE_SESSION_ID"]
        .iter()
        .find_map(|k| nonempty(std::env::var(k).ok()))
}

/// The code-repo facts parsed from `git status --porcelain=v2 --branch`.
#[derive(Debug, Default, PartialEq, Eq)]
struct CodeState {
    sha: Option<String>,
    branch: Option<String>,
    dirty: Option<bool>,
}

/// Parse `git status --porcelain=v2 --branch --untracked-files=no` output.
/// Header lines carry the HEAD oid and branch; any non-header line is a
/// changed tracked path, i.e. a dirty tree.
fn parse_porcelain_v2(out: &str) -> CodeState {
    let mut state = CodeState {
        dirty: Some(false),
        ..CodeState::default()
    };
    for line in out.lines() {
        if let Some(oid) = line.strip_prefix("# branch.oid ") {
            let oid = oid.trim();
            if oid != "(initial)" && !oid.is_empty() {
                state.sha = Some(oid.to_string());
            }
        } else if let Some(head) = line.strip_prefix("# branch.head ") {
            let head = head.trim();
            if head != "(detached)" && !head.is_empty() {
                state.branch = Some(head.to_string());
            }
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            state.dirty = Some(true);
        }
    }
    state
}

/// Read the code repo's state at `dir` with ONE local git subprocess. Any
/// failure (no git, not a repo) yields an all-`None` state.
fn code_state(dir: &Path) -> CodeState {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=no",
        ])
        // Never take the index lock for a read-only probe (it would contend
        // with a concurrent commit in the same repo).
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let state = parse_porcelain_v2(&String::from_utf8_lossy(&out.stdout));
            // Filing from inside the store worktree itself would record the
            // orphan branch's HEAD, which is not a code state — drop it.
            if state.branch.as_deref() == Some("aida-store") {
                CodeState::default()
            } else {
                state
            }
        }
        _ => CodeState::default(),
    }
}

/// Capture filing provenance for a spec filed from the code repo at `dir`.
pub fn capture_in(dir: &Path) -> FilingProvenance {
    let code = code_state(dir);
    let tool = tool_identity();
    FilingProvenance {
        code_sha: code.sha,
        branch: code.branch,
        dirty: code.dirty,
        aida_version: nonempty(tool.version),
        aida_build_sha: nonempty(tool.build_sha).filter(|s| s != "unknown"),
        vendor: nonempty(tool.vendor),
        session: session_from_env(),
    }
}

/// Capture filing provenance for the current process (the code repo is the
/// working directory the filing command runs in).
pub fn capture() -> FilingProvenance {
    match std::env::current_dir() {
        Ok(dir) => capture_in(&dir),
        Err(_) => {
            let tool = tool_identity();
            FilingProvenance {
                aida_version: nonempty(tool.version),
                aida_build_sha: nonempty(tool.build_sha).filter(|s| s != "unknown"),
                vendor: nonempty(tool.vendor),
                session: session_from_env(),
                ..FilingProvenance::default()
            }
        }
    }
}

/// Stamp `req` with `provenance` unless it already carries one (write-once).
/// An all-empty capture is not written.
pub fn stamp_with(req: &mut Requirement, provenance: &FilingProvenance) {
    if req.filed_at.is_none() && !provenance.is_empty() {
        req.filed_at = Some(provenance.clone());
    }
}

/// Stamp `req` with a fresh capture unless it already carries provenance.
pub fn stamp_if_absent(req: &mut Requirement) {
    if req.filed_at.is_none() {
        let provenance = capture();
        stamp_with(req, &provenance);
    }
}

/// Write-once enforcement for an UPDATE of an existing spec: the on-disk
/// provenance always wins over whatever the incoming copy carries, so an edit
/// can neither change, add, nor drop it.
pub fn preserve_from_disk(incoming: &mut Requirement, disk: &Requirement) {
    if incoming.filed_at != disk.filed_at {
        incoming.filed_at = disk.filed_at.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_branch_state() {
        let out = "# branch.oid 0123456789abcdef0123456789abcdef01234567\n\
                   # branch.head feat/x\n\
                   # branch.upstream origin/feat/x\n";
        let s = parse_porcelain_v2(out);
        assert_eq!(
            s.sha.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(s.branch.as_deref(), Some("feat/x"));
        assert_eq!(s.dirty, Some(false));
    }

    #[test]
    fn parses_dirty_detached_state() {
        let out = "# branch.oid abc\n# branch.head (detached)\n1 .M N... 100644 100644 100644 a b src/x.rs\n";
        let s = parse_porcelain_v2(out);
        assert_eq!(s.sha.as_deref(), Some("abc"));
        assert_eq!(s.branch, None);
        assert_eq!(s.dirty, Some(true));
    }

    #[test]
    fn initial_commit_has_no_sha() {
        let s = parse_porcelain_v2("# branch.oid (initial)\n# branch.head main\n");
        assert_eq!(s.sha, None);
        assert_eq!(s.branch.as_deref(), Some("main"));
    }

    #[test]
    fn capture_outside_a_repo_is_best_effort() {
        let dir = tempfile::tempdir().unwrap();
        let p = capture_in(dir.path());
        assert_eq!(p.code_sha, None);
        assert_eq!(p.branch, None);
        assert_eq!(p.dirty, None);
        // The tool identity still lands even with no git repo.
        assert!(p.aida_version.is_some());
    }

    #[test]
    fn stamp_is_write_once() {
        let mut req = Requirement::new("t".into(), "d".into());
        let first = FilingProvenance {
            code_sha: Some("aaa".into()),
            ..FilingProvenance::default()
        };
        stamp_with(&mut req, &first);
        let second = FilingProvenance {
            code_sha: Some("bbb".into()),
            ..FilingProvenance::default()
        };
        stamp_with(&mut req, &second);
        assert_eq!(req.filed_at, Some(first));
    }

    #[test]
    fn empty_capture_is_not_written() {
        let mut req = Requirement::new("t".into(), "d".into());
        stamp_with(&mut req, &FilingProvenance::default());
        assert_eq!(req.filed_at, None);
    }

    #[test]
    fn preserve_from_disk_reverts_an_edit() {
        let mut disk = Requirement::new("t".into(), "d".into());
        disk.filed_at = Some(FilingProvenance {
            code_sha: Some("aaa".into()),
            ..FilingProvenance::default()
        });
        let mut incoming = disk.clone();
        incoming.filed_at = None;
        preserve_from_disk(&mut incoming, &disk);
        assert_eq!(incoming.filed_at, disk.filed_at);
    }

    #[test]
    fn summary_line_and_divergence() {
        let p = FilingProvenance {
            code_sha: Some("abc1234ffff".into()),
            branch: Some("feat/x".into()),
            dirty: Some(true),
            aida_version: Some("0.9.3".into()),
            aida_build_sha: Some("def5678".into()),
            vendor: Some("codex".into()),
            session: None,
        };
        assert_eq!(
            p.summary_line(),
            "abc1234 (feat/x, dirty) · aida 0.9.3 (def5678) · codex"
        );
        assert!(p.build_diverges_from_code());
        let same = FilingProvenance {
            code_sha: Some("def5678aaaa".into()),
            aida_build_sha: Some("def5678".into()),
            ..FilingProvenance::default()
        };
        assert!(!same.build_diverges_from_code());
    }

    #[test]
    fn requirement_without_provenance_round_trips_unchanged() {
        let req = Requirement::new("old".into(), "body".into());
        let yaml = serde_yaml::to_string(&req).unwrap();
        assert!(!yaml.contains("filed_at"));
        let back: Requirement = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.filed_at, None);
        assert_eq!(serde_yaml::to_string(&back).unwrap(), yaml);
    }

    #[test]
    fn unknown_provenance_keys_are_tolerated() {
        // Forward-compat room for STORY-1468 (originating prompt).
        let p: FilingProvenance =
            serde_yaml::from_str("code_sha: abc\nprompt: do the thing\n").unwrap();
        assert_eq!(p.code_sha.as_deref(), Some("abc"));
    }
}
