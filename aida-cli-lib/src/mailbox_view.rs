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
//! receipts for the displayed unread IDs only. No new receipt anchor exists
//! before the retention barrier (slice C), and the post-flush ACK is B2c.
//!
//! Readers that address someone else's inbox (`--all`, `--agent` naming an
//! inbox this process does not hold, launch context for a child, the
//! resume brief, fleet-wide counts) are operator inspection and keep their
//! own source-local reads: an audience is never built from a string.
// trace:STORY-1843 | ai:claude

use aida_core::mailbox::Message;
use aida_core::mailbox_logical::{
    logical_snapshot, MailboxContext, Query, ReadEvidence, ReadLimits, ResolvedAudience,
    SnapshotRecord, SourceErrorCode, SourceStatus,
};
use anyhow::Result;
use std::path::Path;
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
    let store = std::fs::symlink_metadata(store_root)
        .is_ok()
        .then_some(store_root);
    let ctx = match MailboxContext::resolve_or_local(project_root, store) {
        Ok(ctx) => ctx,
        Err(code) => {
            return OwnMail {
                audience: audience.clone(),
                records: Vec::new(),
                coverage: Coverage::Partial {
                    codes: vec![code],
                    conflicts: 0,
                },
            }
        }
    };
    let snapshot = logical_snapshot(&ctx, audience, &Query::Inbox, limits);
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
    OwnMail {
        audience: audience.clone(),
        records: snapshot.records,
        coverage,
    }
}

/// Mark the displayed records read, before they are printed (BUG-1482: a
/// reader that closes the pipe early still has what it was shown marked).
/// Writes a per-ID receipt for each displayed UNREAD record under every key
/// it is addressed to, in the requesting checkout, and keeps each key's
/// legacy watermark advanced to its newest displayed record for older
/// readers. A record covered only by a legacy watermark gets no receipt:
/// its real read time is unknown (TASK-1271), and resolving it is the
/// explicit ACK of slice B2c.
pub(crate) fn mark_displayed(
    project_root: &Path,
    audience: &ResolvedAudience,
    displayed: &[&SnapshotRecord],
) -> Result<()> {
    let mut receipts: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    let mut newest: std::collections::BTreeMap<&str, i64> = Default::default();
    for record in displayed {
        for key in audience.matching_keys(&record.message) {
            let slot = newest.entry(key).or_insert(record.message.timestamp);
            *slot = (*slot).max(record.message.timestamp);
            if ReadState::of(record) == ReadState::Unread {
                receipts.entry(key).or_default().push(&record.message.id);
            }
        }
    }
    for (key, ids) in &receipts {
        crate::mailbox_store::record_seen(project_root, key, ids)?;
    }
    for (key, ts) in newest {
        crate::mailbox_store::set_watermark(project_root, key, ts)?;
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
