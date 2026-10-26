//! Own-audience mail views over the logical snapshot (STORY-1843 slice B2b).
//!
//! Every ordinary reader of "my mail" (CLI inbox/notice/latency, statusline,
//! status/awaiting, MCP `read_inbox`) reads through [`read_own_mail`]: one
//! [`aida_core::mailbox_logical::logical_snapshot`] across the repository's
//! product worktrees and the canonical store, filtered to the audience the
//! core identity policy grants this process. A source that could not be
//! fully read makes the view [`Coverage::Partial`], and readers say so rather
//! than reporting an empty inbox.
//!
//! Read state follows the B2 ruling: only [`ReadEvidence::Unread`] is new
//! mail. Passive counts and notices treat a legacy watermark as not-new; the
//! explicit inbox labels it "read (legacy)". Marking stays the existing
//! BUG-1482 write-before-print path ([`mark_displayed`]); it writes per-ID
//! receipts for the displayed unread IDs only, never a legacy watermark. No
//! new receipt anchor exists before the retention barrier (slice C), and the
//! post-flush ACK is B2c.
//!
//! Every verb that takes a message ID (`read`, `thread`, `archive`,
//! `retract`, `delete`, replies) resolves against [`Addressable`]: the same
//! snapshot the listing used, plus the reader's own sent mail, so an ID a
//! listing displayed always resolves. Writes stay source-local until slice
//! C: a write to a message only another worktree holds refuses and names it.
//!
//! Readers that address someone else's inbox (`--all`, `--agent` naming an
//! inbox this process does not hold, launch context for a child, the
//! resume brief, fleet-wide counts) are operator inspection and keep their
//! own source-local reads: an audience is never built from a string.
// trace:STORY-1843 | ai:claude

use aida_core::mailbox::Message;
use aida_core::mailbox_logical::{
    logical_snapshot, LogicalSnapshot, MailboxContext, Query, ReadEvidence, ReadLimits,
    ResolvedAudience, SnapshotRecord, SourceErrorCode, SourceKind, SourceStatus,
};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Deadline for a passive read (statusline, per-turn notice, status).
const PASSIVE_DEADLINE: Duration = Duration::from_millis(500);

/// Whether every source of a view was fully observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Coverage {
    Complete,
    /// Some mail may be missing; never read as "caught up". `codes` are
    /// safe to show (no paths, IDs or recipients).
    Partial {
        codes: Vec<SourceErrorCode>,
        /// Visible IDs whose replicas disagree (never shown).
        conflicts: usize,
    },
}

/// One reader's mail as the logical snapshot saw it.
#[derive(Debug, Clone)]
pub(crate) struct OwnMail {
    audience: ResolvedAudience,
    records: Vec<SnapshotRecord>,
    coverage: Coverage,
}

/// How a displayed record reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadState {
    Unread,
    Read,
    /// Covered only by a legacy timestamp watermark.
    ReadLegacy,
}

impl ReadState {
    pub(crate) fn of(record: &SnapshotRecord) -> Self {
        match record.read {
            ReadEvidence::Unread => Self::Unread,
            ReadEvidence::Acknowledged { .. } => Self::Read,
            ReadEvidence::LegacyWatermark { .. } => Self::ReadLegacy,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Read => "read",
            Self::ReadLegacy => "read (legacy)",
        }
    }
}

impl OwnMail {
    pub(crate) fn audience(&self) -> &ResolvedAudience {
        &self.audience
    }

    /// Visible records, oldest first.
    pub(crate) fn records(&self) -> &[SnapshotRecord] {
        &self.records
    }

    pub(crate) fn coverage(&self) -> &Coverage {
        &self.coverage
    }

    pub(crate) fn is_partial(&self) -> bool {
        matches!(self.coverage, Coverage::Partial { .. })
    }

    /// New mail: no read evidence and not archived. A legacy watermark is
    /// not new (passive surfaces stay quiet about historic mail).
    pub(crate) fn unread(&self) -> Vec<&Message> {
        self.records
            .iter()
            .filter(|r| !r.message.archived && ReadState::of(r) == ReadState::Unread)
            .map(|r| &r.message)
            .collect()
    }

    /// [`Self::unread`] addressed to any of `keys`.
    pub(crate) fn unread_for<'a>(&'a self, keys: &[&str]) -> Vec<&'a Message> {
        self.unread()
            .into_iter()
            .filter(|m| {
                self.audience
                    .matching_keys(m)
                    .iter()
                    .any(|k| keys.iter().any(|want| want.eq_ignore_ascii_case(k)))
            })
            .collect()
    }

    /// One line naming why the view is partial, or `None` when complete.
    pub(crate) fn partial_note(&self) -> Option<String> {
        let Coverage::Partial { codes, conflicts } = &self.coverage else {
            return None;
        };
        let mut names: Vec<String> = codes.iter().map(|c| format!("{c:?}")).collect();
        names.sort();
        names.dedup();
        let mut parts = Vec::new();
        if !codes.is_empty() {
            parts.push(format!(
                "{} source(s) not fully read ({})",
                codes.len(),
                names.join(", ")
            ));
        }
        if *conflicts > 0 {
            parts.push(format!("{conflicts} message(s) with conflicting copies"));
        }
        Some(format!(
            "mail view partial: {}; some mail may be missing",
            parts.join(", ")
        ))
    }
}

/// The audience this process's verified identity facts grant.
pub(crate) fn own_audience() -> ResolvedAudience {
    ResolvedAudience::from_facts(&crate::collect_mail_identity_facts())
}

/// [`own_audience`] with the session lookup in the project at
/// `project_root` (a long-lived server serving one project).
pub(crate) fn own_audience_in(project_root: &Path) -> ResolvedAudience {
    ResolvedAudience::from_facts(&crate::collect_mail_identity_facts_in(Some(project_root)))
}

/// The limits for an explicit read (a command the reader ran on purpose).
pub(crate) fn explicit_limits() -> ReadLimits {
    ReadLimits::explicit_read()
}

/// The limits for a passive read.
pub(crate) fn passive_limits() -> ReadLimits {
    ReadLimits::with_deadline(Instant::now() + PASSIVE_DEADLINE)
}

/// Read `audience`'s mail as seen from the checkout `project_root`, with the
/// canonical store at `store_root` when one exists there.
pub(crate) fn read_own_mail(
    project_root: &Path,
    store_root: &Path,
    audience: &ResolvedAudience,
    limits: &ReadLimits,
) -> OwnMail {
    let (_, view) = snapshot_view(project_root, store_root, audience, &Query::Inbox, limits);
    view
}

/// One snapshot for `query`, with the context it was taken in (`None` when
/// the checkout could not be resolved; the view is then typed partial).
fn snapshot_view(
    project_root: &Path,
    store_root: &Path,
    audience: &ResolvedAudience,
    query: &Query,
    limits: &ReadLimits,
) -> (Option<(MailboxContext, LogicalSnapshot)>, OwnMail) {
    let store = std::fs::symlink_metadata(store_root)
        .is_ok()
        .then_some(store_root);
    let ctx = match MailboxContext::resolve_or_local(project_root, store) {
        Ok(ctx) => ctx,
        Err(code) => {
            return (
                None,
                OwnMail {
                    audience: audience.clone(),
                    records: Vec::new(),
                    coverage: Coverage::Partial {
                        codes: vec![code],
                        conflicts: 0,
                    },
                },
            )
        }
    };
    let snapshot = logical_snapshot(&ctx, audience, query, limits);
    let coverage = if snapshot.complete() {
        Coverage::Complete
    } else {
        let mut codes: Vec<SourceErrorCode> = snapshot
            .sources
            .iter()
            .filter_map(|s| match s.status {
                SourceStatus::Error(code) => Some(code),
                _ => None,
            })
            .collect();
        codes.extend(snapshot.inventory_error);
        if let aida_core::mailbox_logical::CanonicalCoverage::Refused(code) = snapshot.canonical {
            codes.push(code);
        }
        Coverage::Partial {
            codes,
            conflicts: snapshot.conflicts.len(),
        }
    };
    let view = OwnMail {
        audience: audience.clone(),
        records: snapshot.records.clone(),
        coverage,
    };
    (Some((ctx, snapshot)), view)
}

/// Where a resolvable message physically lives, for the source-local writes
/// (archive, retract, delete) that slice C has not yet made cross-worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Holder {
    /// The requesting checkout or the canonical store holds a copy.
    Here,
    /// Only other worktrees of this repository hold it.
    Elsewhere(Vec<PathBuf>),
}

/// Every message an ID-accepting verb (`read`, `thread`, `archive`,
/// `retract`, `delete`, `send --in-reply-to`/`--thread`, MCP `send_message`
/// replies) resolves against: the SAME logical snapshot the own-audience
/// inbox lists, widened only by the audience's own sent mail, so every ID
/// an inbox or notice displays resolves here (ruling on F1, PR #2952). This
/// checkout's and the canonical store's other mail stays resolvable as
/// operator inspection, exactly as before B2b.
pub(crate) struct Addressable {
    messages: Vec<Message>,
    records: Vec<SnapshotRecord>,
    holders: HashMap<String, Holder>,
    partial: Option<String>,
}

impl Addressable {
    /// Every resolvable message: reconciled snapshot records first, then
    /// this checkout's and the store's other mail.
    pub(crate) fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// The own-audience record for `id`, when the snapshot holds one.
    pub(crate) fn record(&self, id: &str) -> Option<&SnapshotRecord> {
        self.records.iter().find(|r| r.message.id == id)
    }

    /// Resolve an exact ID or unique prefix. A miss on a partial view says
    /// the view was partial, so "not found" is never read as "absent".
    pub(crate) fn resolve(&self, query: &str) -> Result<&Message> {
        let found = crate::resolve_mailbox_message(&self.messages, query);
        match (&found, &self.partial) {
            (Err(e), Some(note))
                if matches!(
                    e.downcast_ref::<crate::MailboxResolveFailure>(),
                    Some(crate::MailboxResolveFailure::NotFound { .. })
                ) =>
            {
                found.with_context(|| note.clone())
            }
            _ => found,
        }
    }

    /// Refuse a source-local write to a message no copy of which lives in
    /// this checkout or the canonical store, naming the worktree(s) that
    /// hold it. Writes stay source-local until slice C.
    pub(crate) fn ensure_writable_here(&self, message: &Message, verb: &str) -> Result<()> {
        match self.holders.get(&message.id) {
            Some(Holder::Elsewhere(paths)) => {
                let named = paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!(
                    "message {} lives only in another worktree ({named}); {verb} it from there \
                     (cross-worktree mailbox writes are not supported yet)",
                    message.id
                )
            }
            _ => Ok(()),
        }
    }
}

/// [`Addressable`] for this process's own audience.
pub(crate) fn addressable(project_root: &Path, store_root: &Path) -> Addressable {
    addressable_for(project_root, store_root, &own_audience())
}

/// [`Addressable`] for `audience`, read from the checkout `project_root`.
pub(crate) fn addressable_for(
    project_root: &Path,
    store_root: &Path,
    audience: &ResolvedAudience,
) -> Addressable {
    let (taken, view) = snapshot_view(
        project_root,
        store_root,
        audience,
        &Query::Addressable,
        &explicit_limits(),
    );
    let mut holders = HashMap::new();
    if let Some((ctx, snapshot)) = &taken {
        let here = std::fs::canonicalize(ctx.requested_checkout().path())
            .unwrap_or_else(|_| ctx.requested_checkout().path().to_path_buf());
        for record in &snapshot.records {
            let mut elsewhere = Vec::new();
            let mut local_here = false;
            for p in &record.provenance {
                match snapshot.sources.get(p.source).map(|s| &s.kind) {
                    Some(SourceKind::Canonical(_)) => local_here = true,
                    Some(SourceKind::Local(path)) => {
                        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                        if path == here {
                            local_here = true;
                        } else {
                            elsewhere.push(path);
                        }
                    }
                    None => {}
                }
            }
            let holder = if local_here || elsewhere.is_empty() {
                Holder::Here
            } else {
                Holder::Elsewhere(elsewhere)
            };
            holders.insert(record.message.id.clone(), holder);
        }
    }
    // This checkout's and the store's other mail: operator inspection, as
    // before B2b. Its sources are also snapshot sources, so a read failure
    // here already makes the view partial; it never aborts the lookup.
    let local = crate::mailbox_store::read_local_messages(project_root).unwrap_or_default();
    let canonical = crate::mailbox_store::read_canonical_messages(store_root).unwrap_or_default();
    let mut messages: Vec<Message> = view.records.iter().map(|r| r.message.clone()).collect();
    for m in aida_core::mailbox::merge_dedup(&local, &canonical) {
        if !holders.contains_key(&m.id) {
            messages.push(m);
        }
    }
    Addressable {
        messages,
        partial: view.partial_note(),
        records: view.records,
        holders,
    }
}

/// Mark the displayed records read, before they are printed (BUG-1482: a
/// reader that closes the pipe early still has what it was shown marked).
/// Writes a per-ID receipt for each displayed UNREAD record under every key
/// it is addressed to, in the requesting checkout, and nothing else (ruling
/// 76 Q3): the legacy timestamp watermark is never advanced, because a
/// displayed record may come from a sibling or the canonical store, and a
/// watermark at its timestamp would hide a later-arriving older local
/// message from every passive surface. A record covered only by a legacy
/// watermark gets no receipt: its real read time is unknown (TASK-1271),
/// and resolving it is the explicit ACK of slice B2c.
pub(crate) fn mark_displayed(
    project_root: &Path,
    audience: &ResolvedAudience,
    displayed: &[&SnapshotRecord],
) -> Result<()> {
    let mut receipts: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for record in displayed {
        if ReadState::of(record) != ReadState::Unread {
            continue;
        }
        for key in audience.matching_keys(&record.message) {
            receipts.entry(key).or_default().push(&record.message.id);
        }
    }
    for (key, ids) in &receipts {
        crate::mailbox_store::record_seen(project_root, key, ids)?;
    }
    Ok(())
}

/// The identity facts an unbound process with the current environment
/// observes (no session-row lookup). Test-only: the caller controls the env.
#[cfg(test)]
pub(crate) fn observed_unbound_facts() -> crate::MailIdentityFacts {
    use aida_core::mailbox_identity::{IdentityEnv, PeopleCatalogView, SessionBinding};
    crate::MailIdentityFacts::observed(
        SessionBinding::Unbound,
        &PeopleCatalogView::default(),
        IdentityEnv::observed(),
    )
}

/// Unbound facts for `user` with an optional role and agent type, observed
/// under a scoped environment. Test-only; takes the env lock itself.
#[cfg(test)]
pub(crate) fn unbound_facts(
    user: &str,
    role: Option<&str>,
    vendor: Option<&str>,
) -> crate::MailIdentityFacts {
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_USER", Some(user)),
        ("AIDA_HANDLE", None),
        ("AIDA_AGENT_NAME", None),
        ("AIDA_SESSION_ROLE", role),
        ("AIDA_AGENT_TYPE", vendor),
        ("AIDA_HOOK_PAYLOAD", None),
    ]);
    observed_unbound_facts()
}

/// Facts for a session whose registry row names `handle` (vendor `vendor`)
/// and is this test process: written to a scratch registry file and
/// verified by core exactly as the collector does. Test-only; the caller
/// controls the env.
#[cfg(test)]
pub(crate) fn bound_facts(handle: &str, vendor: &str) -> crate::MailIdentityFacts {
    use aida_core::mailbox_identity::{IdentityEnv, PeopleCatalogView};
    let dir = tempfile::tempdir().unwrap();
    let row = dir.path().join("test-row.toml");
    std::fs::write(
        &row,
        format!(
            "id = \"test-row\"\nagent_type = \"{vendor}\"\npid = {}\nname = \"{handle}\"\n",
            std::process::id()
        ),
    )
    .unwrap();
    let env = IdentityEnv::observed();
    let binding = crate::session_binding_from_row(Some(&row), &env);
    crate::MailIdentityFacts::observed(binding, &PeopleCatalogView::default(), env)
}

#[cfg(test)]
#[path = "tests/story_1843_mailbox_view_tests.rs"]
pub(crate) mod tests;
