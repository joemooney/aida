//! Filing provenance capture (CR-8) and per-write context (STORY-1674).
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
        harness_version: WriteContext::capture().harness_version,
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
                harness_version: WriteContext::capture().harness_version,
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

/// Small context captured without Git, transcript or model probes.
// trace:STORY-1674 | ai:codex
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteContext {
    pub session_id: Option<String>,
    pub harness_version: Option<String>,
}

impl WriteContext {
    pub fn capture() -> Self {
        Self::from_reported(
            session_from_env(),
            std::env::var("AIDA_HARNESS_VERSION").ok(),
        )
    }

    fn from_reported(session: Option<String>, harness: Option<String>) -> Self {
        Self {
            session_id: nonempty(session),
            harness_version: nonempty(harness),
        }
    }
}

/// Preserve historical context by identity, including nested replies. Only
/// newly persisted entries acquire the current writing context. The capture
/// describes the session that persisted the entry, not a guessed model.
// trace:STORY-1674 | ai:codex
pub fn stamp_write(
    incoming: &mut Requirement,
    disk: Option<&Requirement>,
    context: &WriteContext,
) -> anyhow::Result<()> {
    stamp_write_impl(incoming, disk, context, true)
}

fn stamp_write_impl(
    incoming: &mut Requirement,
    disk: Option<&Requirement>,
    context: &WriteContext,
    record_fallback: bool,
) -> anyhow::Result<()> {
    use crate::models::Comment;

    fn comments(next: &mut [Comment], old: &[Comment], context: &WriteContext) -> bool {
        let mut added = false;
        for c in next {
            let previous = old.iter().find(|p| p.id == c.id);
            if let Some(p) = previous {
                c.session_id = p.session_id.clone();
                c.harness_version = p.harness_version.clone();
            } else {
                if c.session_id.is_none() {
                    c.session_id = context.session_id.clone();
                }
                if c.harness_version.is_none() {
                    c.harness_version = context.harness_version.clone();
                }
                added = true;
            }
            added |= comments(
                &mut c.replies,
                previous.map_or(&[], |p| &p.replies),
                context,
            );
        }
        added
    }

    if let Some(old) = disk {
        preserve_from_disk(incoming, old);
        // A retry with the caller's pre-stamp copy must retain the fallback
        // write event, rather than adding a fresh one on every save.
        for entry in &old.history {
            if entry.changes.is_empty() && !incoming.history.iter().any(|e| e.id == entry.id) {
                incoming.history.push(entry.clone());
            }
        }
    }
    let mut added_history = false;
    for entry in &mut incoming.history {
        if let Some(previous) = disk.and_then(|d| d.history.iter().find(|e| e.id == entry.id)) {
            entry.session_id = previous.session_id.clone();
            entry.harness_version = previous.harness_version.clone();
        } else {
            if entry.session_id.is_none() {
                entry.session_id = context.session_id.clone();
            }
            if entry.harness_version.is_none() {
                entry.harness_version = context.harness_version.clone();
            }
            added_history = true;
        }
    }
    let added_comment = comments(
        &mut incoming.comments,
        disk.map_or(&[], |d| &d.comments),
        context,
    );
    if let Some(old) = disk {
        // Some programmatic mutations have no field-history builder. Record
        // the write with no invented field delta; a true no-op adds nothing.
        if record_fallback
            && !added_history
            && !added_comment
            && serde_yaml::to_string(incoming)? != serde_yaml::to_string(old)?
        {
            incoming.history.push(write_event(context));
        }
    }
    Ok(())
}

fn write_event(context: &WriteContext) -> crate::models::HistoryEntry {
    let mut event = crate::models::HistoryEntry::new(
        nonempty(std::env::var("AIDA_AUTHOR").ok())
            .or_else(|| nonempty(std::env::var("USER").ok()))
            .unwrap_or_else(|| "unknown".into()),
        Vec::new(),
    );
    event.session_id = context.session_id.clone();
    event.harness_version = context.harness_version.clone();
    event
}

/// Importers preserve source event context, including unknowns, and record
/// only the present-day import as a new write. Never relabel historical rows.
// trace:STORY-1674 | ai:codex
pub fn stamp_import_write(
    incoming: &mut Requirement,
    disk: Option<&Requirement>,
) -> anyhow::Result<()> {
    stamp_write_impl(incoming, disk, &WriteContext::default(), false)?;
    let changed = match disk {
        Some(old) => serde_yaml::to_string(incoming)? != serde_yaml::to_string(old)?,
        None => true,
    };
    if changed {
        incoming.history.push(write_event(&WriteContext::capture()));
    }
    Ok(())
}

pub fn stamp_store_import(
    incoming: &mut crate::models::RequirementsStore,
    disk: &[Requirement],
) -> anyhow::Result<()> {
    let old: std::collections::HashMap<_, _> = disk.iter().map(|r| (r.id, r)).collect();
    for req in &mut incoming.requirements {
        stamp_import_write(req, old.get(&req.id).copied())?;
    }
    Ok(())
}

/// Compatibility whole-store writers share the same per-object boundary.
// trace:STORY-1674 | ai:codex
pub fn stamp_store_write(
    incoming: &mut crate::models::RequirementsStore,
    disk: &[Requirement],
) -> anyhow::Result<()> {
    let old: std::collections::HashMap<_, _> = disk.iter().map(|r| (r.id, r)).collect();
    let context = WriteContext::capture();
    let mut filing = None;
    for req in &mut incoming.requirements {
        let previous = old.get(&req.id).copied();
        if previous.is_none() {
            let provenance = filing.get_or_insert_with(capture);
            stamp_with(req, provenance);
        }
        stamp_write(req, previous, &context)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:STORY-1674 | ai:codex
    fn write_context() -> WriteContext {
        WriteContext::from_reported(Some("exact-session".into()), Some("harness-1.2".into()))
    }

    #[test]
    fn story1674_transfers_preserve_historical_unknowns_in_all_backends() {
        use crate::db::{DatabaseBackend, GitBackend, SqliteBackend, YamlBackend};
        use crate::models::{Comment, HistoryEntry, RequirementsStore};
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_SESSION_ID", "importer"),
            ("AIDA_HARNESS_VERSION", "importer-harness"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let targets: Vec<Box<dyn DatabaseBackend>> = vec![
            Box::new(
                GitBackend::new(&dir.path().join("git"))
                    .unwrap()
                    .with_auto_commit(false),
            ),
            Box::new(YamlBackend::new(dir.path().join("legacy.yaml"))),
            Box::new(SqliteBackend::new(dir.path().join("legacy.db")).unwrap()),
        ];
        let mut source = RequirementsStore::new();
        let mut req = Requirement::new("historical".into(), "body".into());
        req.spec_id = Some("TASK-1".into());
        req.history
            .push(HistoryEntry::new("old author".into(), Vec::new()));
        let mut comment = Comment::new("old author".into(), "historical comment".into());
        comment.replies.push(Comment::new_reply(
            "old author".into(),
            "old reply".into(),
            comment.id,
        ));
        req.comments.push(comment);
        source.requirements.push(req.clone());
        for target in targets {
            target.save_imported(&source).unwrap();
            let row = target.get_requirement(&req.id).unwrap().unwrap();
            assert!(row.filed_at.is_none(), "{}", target.path().display());
            assert_eq!(row.history.len(), 2);
            assert!(row.history[0].session_id.is_none());
            assert!(row.history[0].harness_version.is_none());
            assert!(row.comments[0].session_id.is_none());
            assert!(row.comments[0].replies[0].harness_version.is_none());
            assert_eq!(row.history[1].session_id.as_deref(), Some("importer"));
            assert_eq!(
                row.history[1].harness_version.as_deref(),
                Some("importer-harness")
            );
            target.save_imported(&source).unwrap();
            let retried = target.get_requirement(&req.id).unwrap().unwrap();
            assert_eq!(retried.history, row.history, "import retry must be a no-op");
        }
    }

    #[test]
    fn story1674_import_retains_destination_replace_baseline_and_exports_detach_source() {
        use crate::db::{DatabaseBackend, GitBackend};
        let dir = tempfile::tempdir().unwrap();
        let backend = GitBackend::new(&dir.path().join("source"))
            .unwrap()
            .with_auto_commit(false);
        let mut old = Requirement::new("replace me".into(), "body".into());
        old.spec_id = Some("TASK-1".into());
        backend.add_requirement(old).unwrap();
        let mut destination = backend.load().unwrap();
        let mut replacement = Requirement::new("replacement".into(), "body".into());
        replacement.spec_id = Some("TASK-2".into());
        destination.requirements.clear();
        destination.requirements.push(replacement);
        backend.save_imported(&destination).unwrap();
        assert!(backend
            .get_requirement_by_spec_id("TASK-1")
            .unwrap()
            .is_none());
        assert!(backend
            .get_requirement_by_spec_id("TASK-2")
            .unwrap()
            .is_some());
        let mut foreign = backend.load().unwrap();
        assert!(foreign.loaded_objects.is_some());
        // Same transfer boundary as both db export-git dispatchers.
        foreign.loaded_objects = None;
        let target = GitBackend::new(&dir.path().join("target"))
            .unwrap()
            .with_auto_commit(false);
        target.save_imported(&foreign).unwrap();
        assert_eq!(target.load().unwrap().requirements.len(), 1);
        // A destination snapshot still fences concurrent writes.
        let mut mine = target.load().unwrap();
        let r = &mut mine.requirements[0];
        r.title = "my import".into();
        let mut peer = target.get_requirement(&r.id).unwrap().unwrap();
        peer.title = "peer edit".into();
        target.update_requirement(&peer).unwrap();
        assert!(target.save_imported(&mine).is_err());
    }

    #[test]
    fn story1674_reported_context_is_optional_and_trimmed() {
        assert_eq!(
            WriteContext::from_reported(None, None),
            WriteContext::default()
        );
        assert_eq!(
            WriteContext::from_reported(Some("  ".into()), Some(" ".into())),
            WriteContext::default()
        );
        assert_eq!(
            WriteContext::from_reported(
                Some(" exact-session ".into()),
                Some(" harness-1.2 ".into())
            ),
            write_context()
        );
    }

    #[test]
    fn story1674_capture_uses_only_the_explicit_harness_variable() {
        let mut env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_SESSION_ID", Some("exact-session")),
            ("AIDA_HARNESS_VERSION", None),
            ("CLAUDE_CODE_VERSION", Some("must-not-use")),
            ("CODEX_VERSION", Some("must-not-use")),
        ]);
        assert_eq!(WriteContext::capture().harness_version, None);
        env.set_key("AIDA_HARNESS_VERSION", "  harness-1.2  ");
        assert_eq!(WriteContext::capture(), write_context());
        assert_eq!(
            capture_in(tempfile::tempdir().unwrap().path())
                .harness_version
                .as_deref(),
            Some("harness-1.2")
        );
        env.set_key("AIDA_HARNESS_VERSION", "  ");
        assert_eq!(WriteContext::capture().harness_version, None);
    }

    #[test]
    fn story1674_legacy_history_and_comment_yaml_stay_unknown() {
        let mut req = Requirement::new("old".into(), "body".into());
        req.history.push(crate::models::HistoryEntry::new(
            "old author".into(),
            Vec::new(),
        ));
        req.comments.push(crate::models::Comment::new(
            "old author".into(),
            "old text".into(),
        ));
        let yaml = serde_yaml::to_string(&req).unwrap();
        assert!(!yaml.contains("harness_version"));
        assert!(!yaml.contains("session_id"));
        let old: Requirement = serde_yaml::from_str(&yaml).unwrap();
        let mut next = old.clone();
        next.title = "edited now".into();
        stamp_write(&mut next, Some(&old), &write_context()).unwrap();
        assert!(next.filed_at.is_none());
        assert!(next.history[0].session_id.is_none());
        assert!(next.history[0].harness_version.is_none());
        assert!(next.comments[0].session_id.is_none());
        assert_eq!(next.history.len(), 2);
        assert_eq!(next.history[1].session_id, write_context().session_id);
        assert_eq!(
            next.history[1].harness_version,
            write_context().harness_version
        );
        let back: Requirement =
            serde_yaml::from_str(&serde_yaml::to_string(&next).unwrap()).unwrap();
        assert_eq!(back.history, next.history);
        assert_eq!(back.comments, next.comments);
    }

    #[test]
    fn story1674_new_history_and_nested_reply_acquire_context() {
        use crate::models::{Comment, HistoryEntry};
        let mut old = Requirement::new("old".into(), "body".into());
        let mut parent = Comment::new("old author".into(), "old text".into());
        parent.replies.push(Comment::new_reply(
            "old".into(),
            "old reply".into(),
            parent.id,
        ));
        old.comments.push(parent);
        let mut next = old.clone();
        next.history
            .push(HistoryEntry::new("writer".into(), Vec::new()));
        let parent = &mut next.comments[0];
        parent.replies.push(Comment::new_reply(
            "writer".into(),
            "new reply".into(),
            parent.id,
        ));
        stamp_write(&mut next, Some(&old), &write_context()).unwrap();
        assert!(next.comments[0].session_id.is_none());
        assert!(next.comments[0].replies[0].session_id.is_none());
        assert_eq!(
            next.comments[0].replies[1].session_id,
            write_context().session_id
        );
        assert_eq!(
            next.comments[0].replies[1].harness_version,
            write_context().harness_version
        );
        assert_eq!(next.history.len(), 1);
        assert_eq!(next.history[0].session_id, write_context().session_id);
        assert_eq!(
            next.history[0].harness_version,
            write_context().harness_version
        );
    }

    #[test]
    fn story1674_noop_and_retry_add_no_duplicate_write_events() {
        let old = Requirement::new("old".into(), "body".into());
        let mut noop = old.clone();
        stamp_write(&mut noop, Some(&old), &write_context()).unwrap();
        assert!(noop.history.is_empty());
        let mut caller = old.clone();
        caller.title = "new".into();
        let mut persisted = caller.clone();
        stamp_write(&mut persisted, Some(&old), &write_context()).unwrap();
        assert_eq!(persisted.history.len(), 1);
        stamp_write(&mut caller, Some(&persisted), &WriteContext::default()).unwrap();
        assert_eq!(
            serde_yaml::to_string(&caller).unwrap(),
            serde_yaml::to_string(&persisted).unwrap()
        );
    }

    #[test]
    fn story1674_existing_context_cannot_be_rewritten_or_retro_attributed() {
        use crate::models::{Comment, HistoryEntry};
        let mut old = Requirement::new("old".into(), "body".into());
        old.comments
            .push(Comment::new("old".into(), "comment".into()));
        old.history
            .push(HistoryEntry::new("old".into(), Vec::new()));
        let mut next = old.clone();
        next.comments[0].session_id = Some("invented".into());
        next.comments[0].harness_version = Some("invented".into());
        next.history[0].session_id = Some("invented".into());
        next.history[0].harness_version = Some("invented".into());
        stamp_write(&mut next, Some(&old), &write_context()).unwrap();
        assert_eq!(next.comments, old.comments);
        assert_eq!(next.history, old.history);
    }

    #[test]
    fn story1674_comment_edit_records_new_writer_without_relabeling_creation() {
        let mut old = Requirement::new("old".into(), "body".into());
        old.comments
            .push(crate::models::Comment::new("old".into(), "comment".into()));
        stamp_write(&mut old, None, &write_context()).unwrap();
        let mut next = old.clone();
        next.comments[0].content = "edited".into();
        let editor = WriteContext::from_reported(Some("editor".into()), Some("harness-2".into()));
        stamp_write(&mut next, Some(&old), &editor).unwrap();
        assert_eq!(next.comments[0].session_id, write_context().session_id);
        assert_eq!(next.history.len(), 1);
        assert_eq!(next.history[0].session_id, editor.session_id);
        assert_eq!(next.history[0].harness_version, editor.harness_version);
    }

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
            harness_version: None,
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
