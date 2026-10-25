//! Review authority (STORY-1889, from `lib_part5.rs`): which sessions are
//! the authors of the work under review, and the refusals for a recorder
//! that authored it (BUG-1918, STORY-1655 solo mode), an unattributed
//! recorder (BUG-1978) and a headless review blocked by a stale lease.
// mandate-gate
// trace:STORY-1889 | ai:claude

use crate::*;

pub(crate) fn review_stale_lease_refusal(spec_id: &str, conflict: &SessionLease) -> String {
    format!(
        "`{spec_id}` is already in flight behind stale lease {} (owner {}, since {}). \
         Headless review cannot ask whether to release it. Re-run from a TTY to release-and-proceed, \
         or end it explicitly with `aida session end {} --yes` and then re-run `aida review {spec_id}`.",
        &conflict.id[..conflict.id.len().min(8)],
        conflict.owner,
        conflict.started_at.format("%Y-%m-%d %H:%M UTC"),
        &conflict.id[..conflict.id.len().min(8)],
    )
}

/// BUG-1918: the author sessions an approval recorded from `project_root` is
/// checked against — authoring leases (live, or durably recorded when taken)
/// on `subject_ids` or on the reviewed `branch`. With no subject spec
/// resolvable, every live authoring lease and every authorship-ledger record
/// counts: non-authorship cannot be established, so the check fails closed.
// trace:BUG-1918 | ai:claude
pub(crate) fn review_recorder_authors(
    project_root: &std::path::Path,
    subject_ids: &[String],
    branch: Option<&str>,
) -> Vec<crate::gate::ship::AuthorSession> {
    if subject_ids.is_empty() {
        return crate::gate::review_authority::all_author_sessions(project_root, branch);
    }
    crate::gate::review_authority::author_sessions_at(project_root, subject_ids, branch)
}

/// BUG-1918: refuse an approval whose recorder shares an identity with one of
/// the work's author sessions — the same lease, the same agent session id,
/// the same agent process, or the shell that took the lease. Identity, never
/// the free-text `recorded_by` label, decides.
// trace:BUG-1918 | ai:claude
pub(crate) fn review_author_refusal(
    authors: &[crate::gate::ship::AuthorSession],
    recorder: &crate::gate::review_authority::RecorderIdentity,
    subject_resolved: bool,
) -> Option<String> {
    let author = authors
        .iter()
        .find(|a| crate::gate::review_authority::identities_overlap(&a.tokens, &recorder.tokens))?;
    Some(if subject_resolved {
        format!(
            "refused: this session is the one that claimed or implemented the work ({}) — a session cannot approve its own work; record the approval from an independent reviewer session",
            author.label
        )
    } else {
        format!(
            "refused: this session claimed or implemented work ({}) and the review's subject could not be resolved, so non-authorship cannot be established — record the approval from an independent reviewer session",
            author.label
        )
    })
}

/// STORY-1655: what the author gate decides for an approval.
// trace:STORY-1655 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewAuthorGate {
    /// The recorder is not one of the work's authors.
    Independent,
    /// Refused, with the BUG-1918 text unchanged.
    Refused(String),
    /// The recorder authored the work, but solo-human mode is in force and the
    /// recorder passed the human-at-TTY floor: the typed terminal challenge
    /// decides.
    SoloSelfApproval,
}

/// STORY-1655: the author gate with solo-human mode. Solo applies only to a
/// recorder that passed the human-at-TTY floor (`human_at_tty`), never to an
/// agent; `solo_mode` is consulted only when the BUG-1918 check refuses.
// trace:STORY-1655 | ai:claude
pub(crate) fn review_author_gate(
    authors: &[crate::gate::ship::AuthorSession],
    recorder: &crate::gate::review_authority::RecorderIdentity,
    subject_resolved: bool,
    human_at_tty: bool,
    solo_mode: impl FnOnce() -> bool,
) -> ReviewAuthorGate {
    match review_author_refusal(authors, recorder, subject_resolved) {
        None => ReviewAuthorGate::Independent,
        Some(_) if human_at_tty && solo_mode() => ReviewAuthorGate::SoloSelfApproval,
        Some(refusal) => ReviewAuthorGate::Refused(refusal),
    }
}

/// Error for `aida review record` when no agent identity is set and the
/// direct-human check refused. With a valid seat grant, the caller is an
/// agent seat that has not identified itself, so point at the identity
/// variables rather than `aida role enter` (which it cannot use anyway).
/// Without a grant, keep the existing role-enter guidance.
// trace:BUG-1978 | ai:claude
pub(crate) fn review_record_attribution_refusal(
    err: anyhow::Error,
    grant_seat: Option<&str>,
) -> anyhow::Error {
    match grant_seat {
        Some(seat) => anyhow::anyhow!(
            "aida review record attribution fallback: a valid {seat} seat grant was found, \
             but no agent identity is set; export AIDA_AGENT_TYPE (for example claude) and/or \
             AIDA_AGENT_NAME in this session, then re-run the review record command"
        ),
        None => err.context("aida review record attribution fallback"),
    }
}

#[cfg(test)]
#[path = "tests/bug_1978_attribution_message_tests.rs"]
mod bug_1978_attribution_message_tests;
