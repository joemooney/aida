//! `aida status --awaiting` — the top-priority "Awaiting you" section that
//! surfaces every item where the operator is the gate. Aggregates open PRs
//! that are mergeable with no pending CI / requested-changes, unacked
//! agent briefs, findings awaiting triage, queued reviewer-verdict items,
//! and NeedsAttention escalations into one scannable list. Hidden on
//! quiet days so the section appearing is itself the signal.
//!
//! The classifier is pure: callers gather facts (gh PR rollup, brief
//! directory walk, findings view, queue snapshot, requirement summaries)
//! and hand them in as inputs. This module owns the shape + the renderer
//! so the layout invariants stay unit-testable.
//!
//! trace:STORY-465 | ai:claude

use crate::status_cleanup::OpenPrItem;
use colored::Colorize;
use std::io::Write;

/// Default cap on rendered lines before `--verbose` lifts it.
pub(crate) const DEFAULT_CAP: usize = 5;

/// One snapshot of every actionable category. An empty report renders
/// nothing — the absence of the section IS the "nothing awaits you"
/// signal, so a quiet `aida status` stays quiet.
#[derive(Debug, Default, Clone)]
pub(crate) struct AwaitingReport {
    /// Open PRs that pass the awaiting-you gate: `MERGEABLE` + no failing
    /// or pending CI + reviewer verdict is not `CHANGES_REQUESTED`. The
    /// aida-chat motivating case: 5 PRs sat open for hours because the
    /// system was waiting on the human's merge button and nothing said so.
    pub mergeable_prs: Vec<MergeablePrItem>,
    /// Unacked briefs filed for the running agent (or every agent when
    /// the caller can't narrow). Each one is a hand-off the operator
    /// hasn't picked up yet.
    pub pending_briefs: Vec<PendingBriefItem>,
    /// Total findings awaiting triage (`aida findings list` count). The
    /// renderer collapses this to a single line because the triage view
    /// is the real surface — we just want one breadcrumb at the top.
    pub findings_total: usize,
    /// Queue items routed to the current role where the role is a
    /// reviewer-class seat (the human-verdict gate). Empty when the
    /// caller is not acting as a reviewer.
    pub reviewer_queue_items: Vec<ReviewerQueueItem>,
    /// Specs parked in `NeedsAttention` — the implementer-→-advisor-→-
    /// human escalation cascade landed here.
    pub escalations: Vec<EscalationItem>,
    /// Unread inter-agent mail for the OPERATOR's own handle, plus a separate
    /// count for the shared role / agent-type inboxes (BUG-767). Folded in from
    /// the mailbox core so the coordination inbox is ONE surface, not split
    /// between mail (per-turn hook) and everything-else (`aida status`). Cheap:
    /// derived from the local + canonical mailbox files, never a network call,
    /// so it can ride the per-turn notice. Zero when the inbox is caught up.
    // trace:STORY-741 | ai:claude
    // trace:BUG-767 | ai:claude
    pub mail: MailChannel,
    /// Pending worker directives from the local directive file. The enqueue
    /// path (e.g. a human-audit request) lands here, and today it only
    /// surfaces via the worker poll view — folding it in makes the unified
    /// inbox the one place the advisor has to look. Cheap: a single local
    /// file read, never a network call, so it rides the per-turn notice.
    /// Renders as ONE collapsed line (like findings) because the worker
    /// directives view is the real surface — this is the breadcrumb.
    // trace:TASK-1146 | ai:claude
    pub worker_directives: DirectivesChannel,
}

/// Pending-worker-directives summary for the awaiting-you report: how many
/// directives sit in the local directive file plus the FIFO head's one-line
/// summary (what the worker/advisor will pick up next). `pending == 0` →
/// the channel renders nothing.
// trace:TASK-1146 | ai:claude
#[derive(Debug, Default, Clone)]
pub(crate) struct DirectivesChannel {
    pub pending: usize,
    /// One-line summary of the next (FIFO-head) directive, e.g.
    /// `human-audit /aida-human-audit` or `drain batch:x --zen`.
    pub next: Option<String>,
}

/// Unread-mail summary for the awaiting-you report: the full unread count
/// plus how many of those are flagged urgent. Both zero → the inbox is
/// caught up and the mail channel renders nothing.
///
/// BUG-767: `unread`/`urgent` are the OPERATOR's OWN inbox (the handle this
/// session sends and receives as) — deterministic and monotone under deletion:
/// clearing your inbox drives it to 0, never to some wider number. Mail sitting
/// in the SHARED inboxes this session also reads (its session role, its agent
/// type — where agent-to-agent traffic lands) is counted separately in
/// `shared_unread` and rendered on its own labelled line, so the fleet-wide
/// view is kept but can never leak into the operator-gated number.
// trace:STORY-741 | ai:claude
#[derive(Debug, Default, Clone)]
pub(crate) struct MailChannel {
    pub unread: usize,
    pub urgent: usize,
    /// Unread mail in the shared role / agent-type inboxes this session also
    /// reads, EXCLUDING anything already counted in `unread` (a broadcast is
    /// in both, and must count once, in the operator's own number).
    // trace:BUG-767 | ai:claude
    pub shared_unread: usize,
    /// Which shared inboxes `shared_unread` spans, e.g. `advisor + claude` —
    /// the label that keeps this surface distinct from the operator's own.
    // trace:BUG-767 | ai:claude
    pub shared_scope: Option<String>,
}

/// BUG-767: split unread mail into the operator's OWN inbox and the SHARED
/// (session-role / agent-type) inboxes the same session reads.
///
/// The `awaiting` contract is "channels where the OPERATOR is the gate", so the
/// headline number must be a function of the operator handle's inbox alone:
/// empty inbox → 0, always. Before this split the count spanned the whole
/// ambient identity set, so an operator whose own inbox was empty still saw the
/// role inbox's agent-to-agent backlog reported as *their* unread mail — and
/// because that set is env-derived (`AIDA_SESSION_ROLE` / `AIDA_AGENT_TYPE`),
/// the number could GROW as you cleaned your inbox.
///
/// PURE: takes the already-read messages + watermarks, so the counting scope is
/// unit-testable without touching the filesystem or env.
// trace:BUG-767 | ai:claude
pub(crate) fn split_mail_scopes(
    operator: &str,
    shared: &[String],
    messages: &[aida_core::mailbox::Message],
    watermarks: &std::collections::HashMap<String, i64>,
) -> MailChannel {
    use aida_core::mailbox::unread_for_identities;
    let own = unread_for_identities([operator], messages, watermarks);
    let own_ids: std::collections::HashSet<&str> = own.iter().map(|m| m.id.as_str()).collect();
    let shared_ids: Vec<&str> = shared
        .iter()
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty() && s.trim() != operator.trim())
        .collect();
    let shared_unread = unread_for_identities(shared_ids.iter().copied(), messages, watermarks)
        .into_iter()
        .filter(|m| !own_ids.contains(m.id.as_str()))
        .count();
    MailChannel {
        unread: own.len(),
        urgent: own.iter().filter(|m| m.urgent).count(),
        shared_unread,
        shared_scope: if shared_unread > 0 {
            Some(shared_ids.join(" + "))
        } else {
            None
        },
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MergeablePrItem {
    pub number: u64,
    pub title: String,
    pub head_branch: String,
    /// `None` when the PR has no CI checks set up; `Some("pass")` when
    /// every check is green. The classifier already excluded `fail` /
    /// `pending`, so this is informational.
    pub ci_rollup: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingBriefItem {
    pub agent: String,
    pub spec_id: String,
    pub path: std::path::PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewerQueueItem {
    pub spec_id: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub(crate) struct EscalationItem {
    pub spec_id: String,
    pub title: String,
}

impl AwaitingReport {
    /// Count of *lines* this report will render (PRs + briefs + 1 line
    /// for findings if any + 1 for unread mail + 1 for pending worker
    /// directives + reviewer items + escalations). Drives the `(N)` in
    /// the section header and the empty-report short-circuit.
    pub fn total(&self) -> usize {
        self.mergeable_prs.len()
            + self.pending_briefs.len()
            + (if self.findings_total > 0 { 1 } else { 0 })
            + (if self.mail.unread > 0 { 1 } else { 0 })
            // trace:BUG-767 | ai:claude — the shared-inbox line is its own row.
            + (if self.mail.shared_unread > 0 { 1 } else { 0 })
            + (if self.worker_directives.pending > 0 {
                1
            } else {
                0
            })
            + self.reviewer_queue_items.len()
            + self.escalations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// Render the section. Returns `Ok(true)` when something was written
    /// (caller can blank-line spacing accordingly), `Ok(false)` on the
    /// hide-when-empty fast path. `verbose` lifts the cap from
    /// [`DEFAULT_CAP`] to "every item."
    pub fn render(&self, verbose: bool, mut w: impl Write) -> std::io::Result<bool> {
        if self.is_empty() {
            return Ok(false);
        }
        let cap = if verbose { usize::MAX } else { DEFAULT_CAP };

        writeln!(
            w,
            "{}",
            format!("─── Awaiting you ({}) ───", self.total())
                .bold()
                .yellow()
        )?;

        let mut budget = cap;
        let mut overflow = 0usize;

        // Order: PRs first (most actionable — the unblocked-merge case),
        // then briefs (handoffs you owe), then findings line (a triage
        // pointer rather than a per-finding list), then the mail line,
        // then the worker-directives line (another collapsed breadcrumb),
        // then reviewer-queue items, then escalations.
        for pr in &self.mergeable_prs {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            let ci = match pr.ci_rollup.as_deref() {
                Some("pass") => "CI green".green().to_string(),
                None | Some("?") => "no CI".dimmed().to_string(),
                Some(other) => other.dimmed().to_string(),
            };
            writeln!(
                w,
                "  {} PR-{} ready to merge — {} · {}",
                "🟢".green(),
                pr.number.to_string().bold(),
                pr.title,
                ci,
            )?;
            budget -= 1;
        }
        for b in &self.pending_briefs {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            let id = if b.spec_id.is_empty() {
                String::new()
            } else {
                format!(" {}", b.spec_id.bold())
            };
            writeln!(
                w,
                "  📬 brief filed for {}:{} {}",
                b.agent.cyan(),
                id,
                b.path.display().to_string().dimmed(),
            )?;
            budget -= 1;
        }
        if self.findings_total > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                writeln!(
                    w,
                    "  🔍 {} finding{} awaiting triage — `{}`",
                    self.findings_total,
                    if self.findings_total == 1 { "" } else { "s" },
                    "aida findings list".cyan(),
                )?;
                budget -= 1;
            }
        }
        if self.mail.unread > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                let urgent = if self.mail.urgent > 0 {
                    format!(" ({} urgent)", self.mail.urgent)
                } else {
                    String::new()
                };
                writeln!(
                    w,
                    "  {} {} unread mail{}{} — `{}`",
                    crate::glyph(crate::glyphs::Glyph::IncomingMail),
                    self.mail.unread,
                    if self.mail.unread == 1 { "" } else { "s" },
                    urgent,
                    "aida mailbox inbox".cyan(),
                )?;
                budget -= 1;
            }
        }
        // BUG-767: the SHARED role / agent-type inboxes get their own labelled
        // line — kept visible (an advisor still sees its role hand-offs) but
        // never folded into the operator's own unread number.
        // trace:BUG-767 | ai:claude
        if self.mail.shared_unread > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                let scope = self.mail.shared_scope.as_deref().unwrap_or("shared");
                let first = scope.split(" + ").next().unwrap_or(scope);
                writeln!(
                    w,
                    "  {} {} unread mail in the shared {} inbox — `{}`",
                    crate::glyph(crate::glyphs::Glyph::IncomingMail),
                    self.mail.shared_unread,
                    scope,
                    format!("aida mailbox inbox {first}").cyan(),
                )?;
                budget -= 1;
            }
        }
        // trace:TASK-1146 | ai:claude
        if self.worker_directives.pending > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                let next = match self.worker_directives.next.as_deref() {
                    Some(n) if !n.is_empty() => format!(" (next: {n})"),
                    _ => String::new(),
                };
                writeln!(
                    w,
                    "  ⚙️ {} pending worker directive{}{} — `{}`",
                    self.worker_directives.pending,
                    if self.worker_directives.pending == 1 {
                        ""
                    } else {
                        "s"
                    },
                    next,
                    "aida worker directives".cyan(),
                )?;
                budget -= 1;
            }
        }
        for q in &self.reviewer_queue_items {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            writeln!(w, "  👀 verdict needed: {} — {}", q.spec_id.bold(), q.title,)?;
            budget -= 1;
        }
        for e in &self.escalations {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            writeln!(w, "  🗣️ escalation: {} — {}", e.spec_id.bold(), e.title,)?;
            budget -= 1;
        }

        if overflow > 0 {
            writeln!(
                w,
                "  {} {} more — `{}`",
                "…".dimmed(),
                overflow,
                "aida status --awaiting --verbose".cyan(),
            )?;
        }
        writeln!(w)?;
        Ok(true)
    }

    /// Machine-readable JSON shape for `--json` consumers. Stable contract:
    /// adding a field is non-breaking; renaming or removing is breaking.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "total": self.total(),
            "mergeable_prs": self.mergeable_prs.iter().map(|p| serde_json::json!({
                "number": p.number,
                "title": p.title,
                "head_branch": p.head_branch,
                "ci_rollup": p.ci_rollup,
            })).collect::<Vec<_>>(),
            "pending_briefs": self.pending_briefs.iter().map(|b| serde_json::json!({
                "agent": b.agent,
                "spec_id": b.spec_id,
                "path": b.path.display().to_string(),
            })).collect::<Vec<_>>(),
            "findings_total": self.findings_total,
            "mail": {
                "unread": self.mail.unread,
                "urgent": self.mail.urgent,
                // trace:BUG-767 | ai:claude — shared role/agent-type inboxes,
                // reported alongside (never inside) the operator's own count.
                "shared_unread": self.mail.shared_unread,
                "shared_scope": self.mail.shared_scope,
            },
            "worker_directives": {
                "pending": self.worker_directives.pending,
                "next": self.worker_directives.next,
            },
            "reviewer_queue_items": self.reviewer_queue_items.iter().map(|q| serde_json::json!({
                "spec_id": q.spec_id,
                "title": q.title,
            })).collect::<Vec<_>>(),
            "escalations": self.escalations.iter().map(|e| serde_json::json!({
                "spec_id": e.spec_id,
                "title": e.title,
            })).collect::<Vec<_>>(),
        })
    }

    /// One compact line spanning every populated channel, for the per-turn
    /// notice — the can't-miss signal that SOMETHING awaits across ANY
    /// coordination channel (not just mail). Returns `None` when nothing
    /// awaits so the caller stays silent. Pure: renders only the counts the
    /// caller already gathered — it does NO I/O, so cheapness is the caller's
    /// job (build the report with `no_ci` on the per-turn path so PRs, the one
    /// network-backed channel, are omitted; the full `aida awaiting` includes
    /// them). Example (awaiting-glyph prefix elided): `Awaiting you: 2 briefs ·
    /// 1 finding · 3 mail (1 urgent) · 1 escalation — run `aida awaiting``.
    // trace:STORY-741 | ai:claude
    pub fn compact_line(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        if !self.mergeable_prs.is_empty() {
            parts.push(pluralize(self.mergeable_prs.len(), "PR", "PRs"));
        }
        if !self.pending_briefs.is_empty() {
            parts.push(pluralize(self.pending_briefs.len(), "brief", "briefs"));
        }
        if self.findings_total > 0 {
            parts.push(pluralize(self.findings_total, "finding", "findings"));
        }
        if self.mail.unread > 0 {
            let urgent = if self.mail.urgent > 0 {
                format!(" ({} urgent)", self.mail.urgent)
            } else {
                String::new()
            };
            parts.push(format!(
                "{}{}",
                pluralize(self.mail.unread, "mail", "mail"),
                urgent
            ));
        }
        // trace:BUG-767 | ai:claude
        if self.mail.shared_unread > 0 {
            parts.push(format!("{} shared mail", self.mail.shared_unread));
        }
        // trace:TASK-1146 | ai:claude
        if self.worker_directives.pending > 0 {
            parts.push(pluralize(
                self.worker_directives.pending,
                "directive",
                "directives",
            ));
        }
        if !self.reviewer_queue_items.is_empty() {
            parts.push(pluralize(
                self.reviewer_queue_items.len(),
                "verdict",
                "verdicts",
            ));
        }
        if !self.escalations.is_empty() {
            parts.push(pluralize(
                self.escalations.len(),
                "escalation",
                "escalations",
            ));
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!(
            "{} Awaiting you: {} — run `aida awaiting`",
            crate::glyph(crate::glyphs::Glyph::Awaiting),
            parts.join(" · ")
        ))
    }
}

/// `"{n} {singular|plural}"` — the tiny count formatter the compact line uses.
fn pluralize(n: usize, singular: &str, plural: &str) -> String {
    format!("{} {}", n, if n == 1 { singular } else { plural })
}

/// Classify a single open PR as "awaiting you." The aida-chat motivating
/// case (5 mergeable PRs OPEN for hours) lives or dies on this filter:
///   - `mergeable == "MERGEABLE"` (excludes CONFLICTING / UNKNOWN)
///   - CI is not failing or pending (pass / no-checks / `?` are fine)
///   - reviewer verdict is not `CHANGES_REQUESTED`
///
/// A `REVIEW_REQUIRED` PR still qualifies: if the operator is the only
/// reviewer on a solo project, the human merge button is the only gate.
/// trace:STORY-465 | ai:claude
pub(crate) fn is_awaiting_you(pr: &OpenPrItem) -> bool {
    let mergeable = pr.mergeable.as_deref().unwrap_or("");
    if !mergeable.eq_ignore_ascii_case("MERGEABLE") {
        return false;
    }
    match pr.ci_rollup.as_deref() {
        Some("fail") | Some("pending") => return false,
        _ => {}
    }
    let verdict = pr
        .review_decision
        .as_deref()
        .unwrap_or("")
        .to_ascii_uppercase();
    if verdict == "CHANGES_REQUESTED" {
        return false;
    }
    true
}

/// Filter a snapshot of open PRs down to the "Awaiting you" subset. Used
/// by the renderer and exercised directly in tests.
pub(crate) fn classify_open_prs(prs: &[OpenPrItem]) -> Vec<MergeablePrItem> {
    prs.iter()
        .filter(|pr| is_awaiting_you(pr))
        .map(|pr| MergeablePrItem {
            number: pr.number,
            title: pr.title.clone(),
            head_branch: pr.head_branch.clone(),
            ci_rollup: pr.ci_rollup.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(
        number: u64,
        mergeable: Option<&str>,
        ci: Option<&str>,
        verdict: Option<&str>,
    ) -> OpenPrItem {
        OpenPrItem {
            number,
            title: format!("PR {number}"),
            head_branch: format!("branch-{number}"),
            ci_rollup: ci.map(String::from),
            mergeable: mergeable.map(String::from),
            review_decision: verdict.map(String::from),
        }
    }

    // ── BUG-767: the mail counting SCOPE ────────────────────────────────────
    // trace:BUG-767 | ai:claude

    fn mail(
        id: &str,
        from: &str,
        to: aida_core::mailbox::Recipient,
        ts: i64,
    ) -> aida_core::mailbox::Message {
        aida_core::mailbox::Message {
            id: id.to_string(),
            thread_id: "t".to_string(),
            from: from.to_string(),
            to,
            timestamp: ts,
            in_reply_to: None,
            body: "b".to_string(),
            urgent: false,
            intent: aida_core::mailbox::Intent::default(),
            retracted: false,
            deleted: false,
        }
    }

    fn to(agent: &str) -> aida_core::mailbox::Recipient {
        aida_core::mailbox::Recipient::Agent(agent.to_string())
    }

    /// The regression: three messages in the operator's own inbox, a big
    /// backlog in the shared role inbox. Deleting the operator's three drives
    /// the operator-gated count to 0 — it must NEVER shift to the role
    /// backlog's size (the observed 3 → 18 jump).
    #[test]
    fn operator_mail_count_goes_to_zero_when_own_inbox_is_emptied() {
        let wm = std::collections::HashMap::new();
        let mut msgs: Vec<_> = (0..3)
            .map(|i| mail(&format!("own-{i}"), "codex", to("joe"), 100 + i))
            .collect();
        // The shared `advisor` inbox carries a fat agent-to-agent backlog.
        msgs.extend((0..18).map(|i| mail(&format!("role-{i}"), "codex", to("advisor"), 200 + i)));

        let shared = vec!["advisor".to_string()];
        let before = split_mail_scopes("joe", &shared, &msgs, &wm);
        assert_eq!(before.unread, 3, "operator's own three must be the count");
        assert_eq!(before.shared_unread, 18, "role backlog is counted apart");

        // Delete the operator's three (the mailbox delete marker).
        for m in msgs.iter_mut().filter(|m| m.id.starts_with("own-")) {
            m.deleted = true;
        }
        let after = split_mail_scopes("joe", &shared, &msgs, &wm);
        assert_eq!(
            after.unread, 0,
            "emptying your own inbox must read 0, never the shared backlog"
        );
        assert_eq!(after.shared_unread, 18, "the shared surface is unchanged");
        assert_eq!(after.shared_scope.as_deref(), Some("advisor"));
    }

    /// A broadcast lands in every inbox; it must count ONCE, in the operator's
    /// own number, and never be double-counted as shared mail.
    #[test]
    fn broadcast_counts_once_in_the_operator_scope() {
        let wm = std::collections::HashMap::new();
        let msgs = vec![mail(
            "bcast",
            "codex",
            aida_core::mailbox::Recipient::Broadcast,
            10,
        )];
        let c = split_mail_scopes("joe", &["advisor".to_string()], &msgs, &wm);
        assert_eq!(c.unread, 1);
        assert_eq!(c.shared_unread, 0, "already counted as the operator's own");
    }

    /// No session role / agent type → nothing shared, and the operator's own
    /// inbox is the whole story.
    #[test]
    fn no_shared_identities_means_no_shared_channel() {
        let wm = std::collections::HashMap::new();
        let msgs = vec![mail("m1", "codex", to("advisor"), 10)];
        let c = split_mail_scopes("joe", &[], &msgs, &wm);
        assert_eq!(c.unread, 0);
        assert_eq!(c.shared_unread, 0);
        assert!(c.shared_scope.is_none());
    }

    /// The shared line is its own row in the report — the fleet-wide capability
    /// is kept and labelled, just not folded into `mail.unread`.
    #[test]
    fn shared_mail_renders_on_its_own_labelled_line() {
        let r = AwaitingReport {
            mail: MailChannel {
                unread: 0,
                urgent: 0,
                shared_unread: 18,
                shared_scope: Some("advisor".to_string()),
            },
            ..Default::default()
        };
        assert!(!r.is_empty());
        assert_eq!(r.total(), 1);
        let mut buf = Vec::new();
        assert!(r.render(false, &mut buf).unwrap());
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(
            s.contains("18 unread mail in the shared advisor inbox"),
            "shared mail must be labelled distinctly:\n{s}"
        );
        let line = r
            .compact_line()
            .expect("shared mail yields a per-turn line");
        assert!(line.contains("18 shared mail"), "compact line: {line}");
    }

    #[test]
    fn empty_report_renders_nothing_and_returns_false() {
        let r = AwaitingReport::default();
        assert!(r.is_empty());
        let mut buf = Vec::new();
        let wrote = r.render(false, &mut buf).unwrap();
        assert!(!wrote);
        assert!(buf.is_empty(), "section must be hidden when count is 0");
    }

    #[test]
    fn five_mergeable_prs_render_under_cap_with_count_in_header() {
        let prs = (1..=5)
            .map(|n| pr(n, Some("MERGEABLE"), Some("pass"), None))
            .collect::<Vec<_>>();
        let classified = classify_open_prs(&prs);
        assert_eq!(classified.len(), 5);
        let r = AwaitingReport {
            mergeable_prs: classified,
            ..Default::default()
        };
        assert_eq!(r.total(), 5);

        let mut buf = Vec::new();
        let wrote = r.render(false, &mut buf).unwrap();
        assert!(wrote);
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(
            s.contains("Awaiting you (5)"),
            "header must show the total count, got:\n{s}"
        );
        for n in 1..=5 {
            assert!(
                s.contains(&format!("PR-{n}")),
                "PR-{n} missing from output:\n{s}"
            );
        }
        assert!(
            !s.contains("more"),
            "no overflow line at exactly the cap:\n{s}"
        );
    }

    #[test]
    fn mixed_gate_states_only_surface_the_mergeable_pass_no_changes_requested() {
        let prs = vec![
            // Awaiting you — mergeable, CI pass, no review verdict.
            pr(1, Some("MERGEABLE"), Some("pass"), None),
            // Awaiting CI (different gate) — excluded.
            pr(2, Some("MERGEABLE"), Some("pending"), None),
            // CI fail (broken) — excluded.
            pr(3, Some("MERGEABLE"), Some("fail"), None),
            // RequestChanges (needs rebase/fix) — excluded.
            pr(
                4,
                Some("MERGEABLE"),
                Some("pass"),
                Some("CHANGES_REQUESTED"),
            ),
            // Awaiting you — APPROVED + green is the cleanest case.
            pr(5, Some("MERGEABLE"), Some("pass"), Some("APPROVED")),
            // Conflicting — excluded.
            pr(6, Some("CONFLICTING"), Some("pass"), None),
            // Awaiting you — no CI checks set up at all (aida-chat case).
            pr(7, Some("MERGEABLE"), None, None),
        ];
        let classified = classify_open_prs(&prs);
        let nums: Vec<_> = classified.iter().map(|p| p.number).collect();
        assert_eq!(nums, vec![1, 5, 7], "only the awaiting-you PRs surface");
    }

    #[test]
    fn header_count_collapses_findings_to_one_line_regardless_of_total() {
        let r = AwaitingReport {
            mergeable_prs: classify_open_prs(&[pr(1, Some("MERGEABLE"), Some("pass"), None)]),
            findings_total: 17,
            ..Default::default()
        };
        // Header (N) = 1 PR + 1 findings line = 2 (NOT 18).
        assert_eq!(r.total(), 2);
        let mut buf = Vec::new();
        r.render(false, &mut buf).unwrap();
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(s.contains("Awaiting you (2)"), "{s}");
        assert!(s.contains("17 findings awaiting triage"), "{s}");
    }

    #[test]
    fn overflow_caps_at_five_unless_verbose() {
        // 7 PRs all classify as awaiting; default cap shows 5 + overflow line.
        let prs = (1..=7)
            .map(|n| pr(n, Some("MERGEABLE"), Some("pass"), None))
            .collect::<Vec<_>>();
        let r = AwaitingReport {
            mergeable_prs: classify_open_prs(&prs),
            ..Default::default()
        };
        let mut buf = Vec::new();
        r.render(false, &mut buf).unwrap();
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        let pr_lines = s.lines().filter(|l| l.contains("PR-")).count();
        assert_eq!(pr_lines, 5, "default cap = 5 PR lines, got:\n{s}");
        assert!(
            s.contains("2 more"),
            "overflow line must show remainder, got:\n{s}"
        );

        // --verbose lifts the cap.
        let mut buf = Vec::new();
        r.render(true, &mut buf).unwrap();
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        let pr_lines = s.lines().filter(|l| l.contains("PR-")).count();
        assert_eq!(pr_lines, 7, "verbose lifts the cap, got:\n{s}");
        assert!(!s.contains(" more"), "no overflow line under verbose:\n{s}");
    }

    #[test]
    fn json_shape_is_stable_and_includes_total() {
        let r = AwaitingReport {
            mergeable_prs: vec![MergeablePrItem {
                number: 7,
                title: "demo".into(),
                head_branch: "feat".into(),
                ci_rollup: Some("pass".into()),
            }],
            findings_total: 4,
            escalations: vec![EscalationItem {
                spec_id: "SPIKE-12".into(),
                title: "advisor punt".into(),
            }],
            ..Default::default()
        };
        let v = r.to_json();
        // 1 PR + 1 findings line + 1 escalation = 3 actionable lines.
        assert_eq!(v["total"], 3);
        assert_eq!(v["mergeable_prs"][0]["number"], 7);
        assert_eq!(v["findings_total"], 4);
        assert_eq!(v["escalations"][0]["spec_id"], "SPIKE-12");
    }

    // STORY-741: unread mail is now a first-class channel in the report — the
    // count folds into the header total and renders its own line, so the
    // coordination inbox is ONE surface instead of mail-here / everything-
    // else-there. trace:STORY-741
    #[test]
    fn unread_mail_folds_into_report_and_renders_with_urgent() {
        let r = AwaitingReport {
            mail: MailChannel {
                unread: 3,
                urgent: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        // A report with ONLY mail is non-empty and counts as one line.
        assert!(!r.is_empty());
        assert_eq!(r.total(), 1);
        let mut buf = Vec::new();
        let wrote = r.render(false, &mut buf).unwrap();
        assert!(wrote);
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(s.contains("3 unread mail"), "mail line missing:\n{s}");
        assert!(s.contains("(1 urgent)"), "urgent flag missing:\n{s}");
        assert!(
            s.contains("aida mailbox inbox"),
            "mail line must point at the inbox:\n{s}"
        );
    }

    // STORY-741: the header total counts mail as ONE line regardless of how
    // many messages are unread — the notice is a breadcrumb, not the inbox.
    #[test]
    fn mail_counts_as_one_line_in_the_header_total() {
        let r = AwaitingReport {
            mail: MailChannel {
                unread: 42,
                urgent: 0,
                ..Default::default()
            },
            findings_total: 5,
            ..Default::default()
        };
        // 1 findings line + 1 mail line = 2 (NOT 47).
        assert_eq!(r.total(), 2);
    }

    // STORY-741: the compact per-turn line spans EVERY populated channel and is
    // `None` when nothing awaits.
    //
    // STORY-769 note: the *command* `aida awaiting --notice` is no longer silent
    // when nothing awaits — it ALWAYS leads with a "Current date/time + Timing"
    // line (emitted separately by `emit_notice_time_line`, before this report is
    // even built). This `compact_line()` is the AWAITING-CHANNELS half of that
    // output and legitimately stays `None` when those channels are empty; the
    // time line carries the always-on signal. So the two halves compose: time
    // line every turn, awaiting line only when something awaits. trace:STORY-769
    #[test]
    fn compact_line_spans_all_channels_and_is_empty_when_nothing_awaits() {
        assert!(
            AwaitingReport::default().compact_line().is_none(),
            "an empty report must produce no awaiting-channels line (the time line \
             is emitted separately and is always present — STORY-769)"
        );

        let r = AwaitingReport {
            pending_briefs: vec![
                PendingBriefItem {
                    agent: "claude".into(),
                    spec_id: "TASK-1".into(),
                    path: std::path::PathBuf::from("/b/1"),
                },
                PendingBriefItem {
                    agent: "claude".into(),
                    spec_id: "TASK-2".into(),
                    path: std::path::PathBuf::from("/b/2"),
                },
            ],
            findings_total: 1,
            mail: MailChannel {
                unread: 3,
                urgent: 1,
                ..Default::default()
            },
            escalations: vec![EscalationItem {
                spec_id: "SPIKE-9".into(),
                title: "punt".into(),
            }],
            ..Default::default()
        };
        let line = r.compact_line().expect("populated report must have a line");
        // One line, spanning every channel, with counts + the urgent flag.
        assert_eq!(line.lines().count(), 1, "must be exactly one line: {line}");
        assert!(line.contains("2 briefs"), "briefs channel missing: {line}");
        assert!(
            line.contains("1 finding"),
            "findings channel missing: {line}"
        );
        assert!(line.contains("3 mail"), "mail channel missing: {line}");
        assert!(line.contains("(1 urgent)"), "urgent flag missing: {line}");
        assert!(
            line.contains("1 escalation"),
            "escalation channel missing: {line}"
        );
        assert!(
            line.contains("aida awaiting"),
            "line must point at the full view: {line}"
        );
    }

    // STORY-741: PRs are the one network-backed channel, so the per-turn line
    // omits them (the caller builds the report with `no_ci`). This asserts the
    // compact line renders cleanly from the cheap channels alone.
    #[test]
    fn compact_line_renders_from_cheap_channels_without_prs() {
        let r = AwaitingReport {
            mail: MailChannel {
                unread: 1,
                urgent: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(r.mergeable_prs.is_empty());
        let line = r.compact_line().unwrap();
        assert!(line.contains("1 mail"), "{line}");
        assert!(!line.contains(" PR"), "no PR channel expected: {line}");
    }

    // STORY-741: the JSON contract gains a stable `mail` object.
    #[test]
    fn json_shape_includes_mail_channel() {
        let r = AwaitingReport {
            mail: MailChannel {
                unread: 4,
                urgent: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        let v = r.to_json();
        assert_eq!(v["mail"]["unread"], 4);
        assert_eq!(v["mail"]["urgent"], 2);
        assert_eq!(v["total"], 1);
    }

    // TASK-1146: pending worker directives are now a channel in the report —
    // the count folds into the header total as ONE collapsed line (like
    // findings), names the FIFO-head directive, and points at the worker
    // directives view. trace:TASK-1146
    #[test]
    fn worker_directives_fold_into_report_as_one_line() {
        let r = AwaitingReport {
            worker_directives: DirectivesChannel {
                pending: 3,
                next: Some("human-audit /aida-human-audit".into()),
            },
            ..Default::default()
        };
        // A report with ONLY directives is non-empty and counts as one line
        // regardless of how many directives are queued.
        assert!(!r.is_empty());
        assert_eq!(r.total(), 1);
        let mut buf = Vec::new();
        let wrote = r.render(false, &mut buf).unwrap();
        assert!(wrote);
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(
            s.contains("3 pending worker directives"),
            "directives line missing:\n{s}"
        );
        assert!(
            s.contains("(next: human-audit /aida-human-audit)"),
            "FIFO-head summary missing:\n{s}"
        );
        assert!(
            s.contains("aida worker directives"),
            "line must point at the worker directives view:\n{s}"
        );
    }

    // TASK-1146: the compact per-turn notice line names the directives channel,
    // and the channel is local-file-backed so it legitimately rides the
    // network-free `--notice` path.
    #[test]
    fn compact_line_includes_worker_directives_channel() {
        let r = AwaitingReport {
            worker_directives: DirectivesChannel {
                pending: 1,
                next: Some("drain batch:x --zen".into()),
            },
            ..Default::default()
        };
        let line = r.compact_line().expect("directives alone must fire a line");
        assert!(
            line.contains("1 directive"),
            "directives channel missing: {line}"
        );
        assert!(
            line.contains("aida awaiting"),
            "line must point at the full view: {line}"
        );
    }

    // TASK-1146: the JSON contract gains a stable `worker_directives` object.
    #[test]
    fn json_shape_includes_worker_directives_channel() {
        let r = AwaitingReport {
            worker_directives: DirectivesChannel {
                pending: 2,
                next: Some("human-audit /aida-human-audit".into()),
            },
            ..Default::default()
        };
        let v = r.to_json();
        assert_eq!(v["worker_directives"]["pending"], 2);
        assert_eq!(
            v["worker_directives"]["next"],
            "human-audit /aida-human-audit"
        );
        assert_eq!(v["total"], 1);

        // Empty channel serializes as pending 0 / next null (stable shape).
        let v = AwaitingReport::default().to_json();
        assert_eq!(v["worker_directives"]["pending"], 0);
        assert!(v["worker_directives"]["next"].is_null());
    }

    /// Mirrors `status_cleanup::tests::strip_ansi` so assertions can check
    /// plain text without coupling to the colour codes.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut in_esc = false;
        for c in s.chars() {
            if in_esc {
                if c == 'm' {
                    in_esc = false;
                }
                continue;
            }
            if c == '\u{1b}' {
                in_esc = true;
                continue;
            }
            out.push(c);
        }
        out
    }
}
