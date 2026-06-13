//! Inter-agent mailbox — pure message model + inbox/thread/merge logic
//! (P3 slice 1, STORY-493 under SPIKE-45).
//!
//! Agent↔agent peer messaging, distinct from the operator→agent *briefs* and
//! the top-down *directives*. The shipped shape is **hybrid** (operator
//! decision, 2026-05-31): a fast `.aida/mailbox/` local layer for live exchange
//! plus a git-canonical durable digest on the orphan store for replay / audit /
//! cross-clone sharing — the round-2 differentiator vs competitors' ephemeral
//! local mailboxes.
//!
//! This module is **only the pure, side-effect-free core**: the message model
//! and the read-side logic (an agent's inbox, a thread view, and the merge that
//! reconciles the local + canonical layers). The local-file I/O, the orphan
//! store digest, the CLI, and the MCP tools are separate slices — keeping this
//! pure makes it exhaustively unit-testable and decouples the message semantics
//! from the storage mechanics. Messages are **append-only and id-keyed**, so
//! concurrent digests from two agents merge without edit conflicts.
//!
//! trace:STORY-493 (P3) trace:TASK-602 | ai:claude

use serde::{Deserialize, Serialize};

/// A message recipient: a specific agent, or every agent (broadcast).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "agent", rename_all = "snake_case")]
pub enum Recipient {
    Agent(String),
    Broadcast,
}

/// One append-only inter-agent message. `id` is a time-ordered unique id
/// (uuid7 / HLC at the write boundary); `timestamp` is an epoch-millis stamp
/// passed in by the writer (kept out of this pure module so it has no clock
/// dependency). `thread_id` groups a conversation; `in_reply_to` chains replies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub to: Recipient,
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    pub body: String,
    /// Light urgency flag (STORY-539): `true` marks an out-of-band escalation /
    /// "stop" that should be surfaced (e.g. statusline nag) instead of sitting
    /// unseen in a purely-chronological inbox. Defaults to `false`, so messages
    /// written before this field existed (and the common informational case)
    /// deserialize unchanged — append-only, non-breaking. Deliberately a single
    /// bool, not a priority scheme: this is a lightweight channel.
    /// trace:STORY-539 | ai:claude
    #[serde(default)]
    pub urgent: bool,
    /// Sender/operator withdrawal: visible as a tombstone, body hidden in
    /// normal views. Defaults false for older messages.
    // trace:STORY-583 | ai:codex
    #[serde(default)]
    pub retracted: bool,
    /// Sender/operator delete marker: suppresses the message from mailbox
    /// views and wins during local/canonical merge so sync cannot resurrect it.
    // trace:STORY-583 | ai:codex
    #[serde(default)]
    pub deleted: bool,
}

impl Message {
    /// Is this message visible in `agent`'s inbox? Addressed to it directly or
    /// broadcast — but not its own sent messages (an agent doesn't inbox what
    /// it sent, including its own broadcasts).
    fn addressed_to(&self, agent: &str) -> bool {
        if self.deleted {
            return false;
        }
        if self.from == agent {
            return false;
        }
        match &self.to {
            Recipient::Agent(a) => a == agent,
            Recipient::Broadcast => true,
        }
    }
}

/// An agent's inbox: messages addressed to it (directly or by broadcast),
/// excluding its own sent messages, ordered oldest-first (`timestamp`, then
/// `id` as a stable tiebreaker). trace:P3
pub fn inbox_for<'a>(agent: &str, messages: &'a [Message]) -> Vec<&'a Message> {
    let mut out: Vec<&Message> = messages.iter().filter(|m| m.addressed_to(agent)).collect();
    out.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
    out
}

/// All messages in a thread, ordered oldest-first. The whole conversation
/// (every participant), not filtered by recipient. trace:P3
pub fn thread<'a>(thread_id: &str, messages: &'a [Message]) -> Vec<&'a Message> {
    let mut out: Vec<&Message> = messages
        .iter()
        .filter(|m| m.thread_id == thread_id && !m.deleted)
        .collect();
    out.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
    out
}

/// Reconcile the two hybrid layers into one deduped view: the union of the
/// git-canonical durable record and the not-yet-digested local messages, keyed
/// by `id` (a message present in both — already digested but still local —
/// appears once). Canonical wins on an id collision (it is the durable record);
/// result is ordered oldest-first. This is the read-side of the hybrid model.
/// trace:P3
pub fn merge_dedup(local: &[Message], canonical: &[Message]) -> Vec<Message> {
    use std::collections::HashMap;
    let mut by_id: HashMap<&str, &Message> = HashMap::new();
    for m in local {
        by_id.insert(m.id.as_str(), m);
    }
    for m in canonical {
        match by_id.get(m.id.as_str()) {
            Some(existing) if message_state_rank(existing) > message_state_rank(m) => {}
            _ => {
                by_id.insert(m.id.as_str(), m);
            }
        }
    }
    let mut out: Vec<Message> = by_id.into_values().cloned().collect();
    out.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
    out
}

// trace:STORY-583 | ai:codex
pub fn message_state_rank(m: &Message) -> u8 {
    if m.deleted {
        2
    } else if m.retracted {
        1
    } else {
        0
    }
}

/// One row of the operator overview (`aida mailbox list`): an agent that has
/// mail waiting, with how much of it is unread / urgent-unread relative to that
/// agent's read-watermark. trace:STORY-539 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMailSummary {
    /// The recipient agent.
    pub agent: String,
    /// Total messages in this agent's inbox (direct + broadcast).
    pub total: usize,
    /// Messages newer than the agent's read-watermark.
    pub unread: usize,
    /// Of the unread, how many are flagged urgent.
    pub urgent_unread: usize,
    /// Timestamp of the most recent message in the inbox (for sort + display).
    pub latest_ts: i64,
}

/// Count how many of `agent`'s inbox messages are unread — i.e. have a
/// timestamp strictly greater than `read_watermark` (the timestamp of the last
/// message the agent has seen; `None` = never read, so everything is unread).
/// Returns `(unread, urgent_unread)`. trace:STORY-539 | ai:claude
pub fn unread_counts(
    agent: &str,
    messages: &[Message],
    read_watermark: Option<i64>,
) -> (usize, usize) {
    let mark = read_watermark.unwrap_or(i64::MIN);
    let mut unread = 0usize;
    let mut urgent = 0usize;
    for m in inbox_for(agent, messages) {
        if m.timestamp > mark {
            unread += 1;
            if m.urgent {
                urgent += 1;
            }
        }
    }
    (unread, urgent)
}

/// Build the operator overview across every agent that appears as a recipient.
/// `watermarks` maps an agent id to its read-watermark timestamp (absent = the
/// agent has read nothing). Broadcasts count toward every *known* recipient's
/// inbox, so the agent set is the union of explicit `Agent(..)` recipients and
/// the watermark keys (operators who have read at least once are "known"). The
/// rows are ordered most-recent-activity-first. trace:STORY-539 | ai:claude
pub fn agent_summaries(
    messages: &[Message],
    watermarks: &std::collections::HashMap<String, i64>,
) -> Vec<AgentMailSummary> {
    agent_summaries_for_agents(messages, watermarks, std::iter::empty::<&str>())
}

/// Build the operator overview, seeded with externally-known agents (for
/// example the role registry). Broadcasts are delivered lazily, so a known
/// agent with no materialized inbox still has pending broadcast mail.
// trace:BUG-513 | ai:codex
pub fn agent_summaries_for_agents<'a, I>(
    messages: &[Message],
    watermarks: &std::collections::HashMap<String, i64>,
    known_agents: I,
) -> Vec<AgentMailSummary>
where
    I: IntoIterator<Item = &'a str>,
{
    use std::collections::BTreeSet;
    // Known agents = every explicit direct-recipient + every sender (so an
    // agent that has only ever sent still shows once it receives a broadcast)
    // + every agent with a recorded watermark + externally-known recipients.
    let mut agents: BTreeSet<String> = BTreeSet::new();
    for m in messages {
        agents.insert(m.from.clone());
        if let Recipient::Agent(a) = &m.to {
            agents.insert(a.clone());
        }
    }
    for k in watermarks.keys() {
        agents.insert(k.clone());
    }
    for agent in known_agents {
        let trimmed = agent.trim();
        if !trimmed.is_empty() {
            agents.insert(trimmed.to_string());
        }
    }

    let mut out: Vec<AgentMailSummary> = Vec::new();
    for agent in agents {
        let inbox = inbox_for(&agent, messages);
        if inbox.is_empty() {
            continue; // only list agents that actually have mail waiting
        }
        let latest_ts = inbox.iter().map(|m| m.timestamp).max().unwrap_or(0);
        let (unread, urgent_unread) =
            unread_counts(&agent, messages, watermarks.get(&agent).copied());
        out.push(AgentMailSummary {
            agent,
            total: inbox.len(),
            unread,
            urgent_unread,
            latest_ts,
        });
    }
    out.sort_by(|a, b| {
        b.latest_ts
            .cmp(&a.latest_ts)
            .then_with(|| a.agent.cmp(&b.agent))
    });
    out
}

/// The unread slice of one agent's inbox: messages strictly newer than its
/// read-watermark, oldest-first. `None` watermark = never read, so the whole
/// inbox is unread. Read side of the notice surface — pure so the CLI's
/// `mailbox notice` / `inbox --unread` and any hook compose over it without
/// re-deriving the watermark comparison. trace:STORY-585 | ai:claude
pub fn unread_inbox<'a>(
    agent: &str,
    messages: &'a [Message],
    read_watermark: Option<i64>,
) -> Vec<&'a Message> {
    let mark = read_watermark.unwrap_or(i64::MIN);
    inbox_for(agent, messages)
        .into_iter()
        .filter(|m| m.timestamp > mark)
        .collect()
}

/// One unread message, flattened for the agent-facing notice: who sent it, a
/// one-line subject (first non-empty body line, truncated), urgency, and the
/// thread id to read the rest. trace:STORY-585 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeItem {
    pub id: String,
    pub from: String,
    pub thread_id: String,
    pub subject: String,
    pub urgent: bool,
}

/// A capped, identity-scoped unread summary for surfacing into an agent's
/// context (the `mailbox notice` verb the SessionStart / per-turn hook calls).
/// `total` is the full unread count across the resolved identities; `shown`
/// holds at most `cap` items (newest-last, like the inbox); `overflow` is how
/// many were elided so the notice can say "+N more". `urgent` counts urgent
/// unread across all of `total`. Empty (`total == 0`) when nothing is unread,
/// so the caller stays silent. trace:STORY-585 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeSummary {
    pub total: usize,
    pub urgent: usize,
    pub overflow: usize,
    pub shown: Vec<NoticeItem>,
}

impl NoticeSummary {
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// Default cap on items rendered in a notice — keep the per-turn context
/// injection bounded; the footer points at `aida mailbox inbox` for the rest.
pub const NOTICE_DEFAULT_CAP: usize = 5;

/// First non-empty line of `body`, trimmed and truncated to `max` chars (with
/// an ellipsis when cut). A retracted message has no readable body, so it
/// renders as a `[withdrawn]` placeholder. trace:STORY-585 | ai:claude
fn subject_line(m: &Message, max: usize) -> String {
    if m.retracted {
        return "[withdrawn]".to_string();
    }
    let first = m
        .body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut chars = first.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Build the unread notice for the union of `identities` (e.g. the shell user
/// id plus the session role). Messages are deduped by id across identities (a
/// broadcast is in every identity's inbox but counts once), and each identity's
/// own watermark gates its unread set — a message is unread if it is past the
/// watermark of *any* identity that can see it. Ordered oldest-first; `shown`
/// keeps the newest `cap`. trace:STORY-585 | ai:claude
pub fn build_notice<'a, I>(
    identities: I,
    messages: &[Message],
    watermarks: &std::collections::HashMap<String, i64>,
    cap: usize,
) -> NoticeSummary
where
    I: IntoIterator<Item = &'a str>,
{
    use std::collections::HashMap;
    // For each message any identity can see, track the message and the HIGHEST
    // watermark among its viewing identities. A message is unread iff its
    // timestamp is past *every* viewer's watermark — equivalently, past the max
    // — so reading it as ANY identity (which advances that identity's
    // watermark) clears it. Order-independent. A direct message has one viewer
    // (its recipient); a broadcast is viewed by every identity.
    let mut by_id: HashMap<&str, (&Message, i64)> = HashMap::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for id in identities {
        let id = id.trim();
        if id.is_empty() || !seen_ids.insert(id.to_string()) {
            continue;
        }
        let mark = watermarks.get(id).copied().unwrap_or(i64::MIN);
        for m in inbox_for(id, messages) {
            by_id
                .entry(m.id.as_str())
                .and_modify(|(_, wm)| *wm = (*wm).max(mark))
                .or_insert((m, mark));
        }
    }
    let mut unread: Vec<&Message> = by_id
        .into_values()
        .filter(|(m, max_wm)| m.timestamp > *max_wm)
        .map(|(m, _)| m)
        .collect();
    unread.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));

    let total = unread.len();
    let urgent = unread.iter().filter(|m| m.urgent).count();
    let overflow = total.saturating_sub(cap);
    let shown = unread
        .iter()
        .skip(overflow) // keep the newest `cap`
        .map(|m| NoticeItem {
            id: m.id.clone(),
            from: m.from.clone(),
            thread_id: m.thread_id.clone(),
            subject: subject_line(m, 60),
            urgent: m.urgent,
        })
        .collect();
    NoticeSummary {
        total,
        urgent,
        overflow,
        shown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, thread: &str, from: &str, to: Recipient, ts: i64) -> Message {
        Message {
            id: id.to_string(),
            thread_id: thread.to_string(),
            from: from.to_string(),
            to,
            timestamp: ts,
            in_reply_to: None,
            body: format!("body-{id}"),
            urgent: false,
            retracted: false,
            deleted: false,
        }
    }

    fn urgent_msg(id: &str, thread: &str, from: &str, to: Recipient, ts: i64) -> Message {
        Message {
            urgent: true,
            ..msg(id, thread, from, to, ts)
        }
    }

    #[test]
    fn inbox_for_returns_direct_and_broadcast_excluding_own() {
        let msgs = vec![
            msg("1", "t", "codex", Recipient::Agent("claude".into()), 10),
            msg("2", "t", "agy", Recipient::Broadcast, 20),
            msg("3", "t", "claude", Recipient::Agent("codex".into()), 30), // claude sent
            msg("4", "t", "claude", Recipient::Broadcast, 40),             // claude's own broadcast
            msg("5", "t", "codex", Recipient::Agent("agy".into()), 50),    // to someone else
        ];
        let ids: Vec<&str> = inbox_for("claude", &msgs)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["1", "2"],
            "direct-to-claude + broadcast, not own-sent, not to-others"
        );
    }

    #[test]
    fn inbox_is_ordered_oldest_first() {
        let msgs = vec![
            msg("b", "t", "x", Recipient::Broadcast, 200),
            msg("a", "t", "x", Recipient::Broadcast, 100),
        ];
        let ids: Vec<&str> = inbox_for("claude", &msgs)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn thread_returns_whole_conversation_ordered() {
        let msgs = vec![
            msg("3", "T1", "claude", Recipient::Agent("codex".into()), 30),
            msg("1", "T1", "codex", Recipient::Agent("claude".into()), 10),
            msg("9", "T2", "agy", Recipient::Broadcast, 5), // other thread
            msg("2", "T1", "claude", Recipient::Agent("codex".into()), 20),
        ];
        let ids: Vec<&str> = thread("T1", &msgs).iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["1", "2", "3"],
            "T1 only, oldest-first; T2 excluded"
        );
    }

    #[test]
    fn broadcast_appears_in_every_inbox() {
        let msgs = vec![msg("1", "t", "codex", Recipient::Broadcast, 10)];
        for agent in ["claude", "agy", "antigravity"] {
            assert_eq!(
                inbox_for(agent, &msgs).len(),
                1,
                "{agent} should see the broadcast"
            );
        }
        // ...but not the sender.
        assert_eq!(inbox_for("codex", &msgs).len(), 0);
    }

    #[test]
    fn merge_dedup_unions_by_id_no_double_count() {
        let shared = msg("1", "t", "codex", Recipient::Broadcast, 10);
        let local = vec![
            shared.clone(),
            msg("2", "t", "agy", Recipient::Broadcast, 20), // local-only (not yet digested)
        ];
        let canonical = vec![
            shared,                                           // same id in both
            msg("0", "t", "claude", Recipient::Broadcast, 5), // canonical-only history
        ];
        let merged = merge_dedup(&local, &canonical);
        let ids: Vec<&str> = merged.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["0", "1", "2"],
            "union by id, deduped, oldest-first"
        );
    }

    #[test]
    fn recipient_serde_roundtrips() {
        for r in [Recipient::Agent("claude".into()), Recipient::Broadcast] {
            let s = serde_json::to_string(&r).unwrap();
            let back: Recipient = serde_json::from_str(&s).unwrap();
            assert_eq!(r, back);
        }
    }

    #[test]
    fn message_without_urgent_field_deserializes_as_not_urgent() {
        // A message JSON written before the urgent field existed (no `urgent`
        // key) must round-trip as not-urgent — append-only, non-breaking.
        let legacy = r#"{
            "id":"m1","thread_id":"t","from":"codex",
            "to":{"kind":"broadcast"},"timestamp":10,"body":"hi"
        }"#;
        let m: Message = serde_json::from_str(legacy).unwrap();
        assert!(!m.urgent, "absent urgent field defaults to false");
        assert!(!m.retracted, "absent retracted field defaults to false");
        assert!(!m.deleted, "absent deleted field defaults to false");
    }

    // trace:STORY-583 | ai:codex
    #[test]
    fn merge_dedup_tombstone_and_delete_markers_win_over_regular_message() {
        let original = msg("1", "t", "codex", Recipient::Agent("claude".into()), 10);
        let retracted = Message {
            retracted: true,
            body: String::new(),
            ..original.clone()
        };
        let deleted = Message {
            deleted: true,
            body: String::new(),
            ..original.clone()
        };

        let merged = merge_dedup(
            std::slice::from_ref(&retracted),
            std::slice::from_ref(&original),
        );
        assert!(merged[0].retracted);
        assert!(!merged[0].deleted);

        let merged = merge_dedup(
            std::slice::from_ref(&original),
            std::slice::from_ref(&deleted),
        );
        assert!(merged[0].deleted);
        assert!(inbox_for("claude", &merged).is_empty());
        assert!(thread("t", &merged).is_empty());
    }

    #[test]
    fn unread_counts_splits_unread_and_urgent_by_watermark() {
        let msgs = vec![
            msg("1", "t", "codex", Recipient::Agent("claude".into()), 10),
            urgent_msg("2", "t", "agy", Recipient::Broadcast, 20),
            msg("3", "t", "codex", Recipient::Agent("claude".into()), 30),
        ];
        // Read up to ts=10: messages 2 (urgent) and 3 are unread.
        let (unread, urgent) = unread_counts("claude", &msgs, Some(10));
        assert_eq!((unread, urgent), (2, 1));
        // Never read: all three unread, one urgent.
        let (unread, urgent) = unread_counts("claude", &msgs, None);
        assert_eq!((unread, urgent), (3, 1));
        // Caught up: nothing unread.
        let (unread, urgent) = unread_counts("claude", &msgs, Some(30));
        assert_eq!((unread, urgent), (0, 0));
    }

    #[test]
    fn agent_summaries_lists_agents_with_mail_ordered_by_recency() {
        let msgs = vec![
            msg("1", "t", "codex", Recipient::Agent("claude".into()), 10),
            urgent_msg("2", "t", "claude", Recipient::Agent("codex".into()), 50),
            msg("3", "t", "agy", Recipient::Broadcast, 30),
        ];
        let mut wm = std::collections::HashMap::new();
        wm.insert("claude".to_string(), 10i64); // claude read msg 1 but not the broadcast
        let rows = agent_summaries(&msgs, &wm);
        // codex (latest 50) first, then claude (broadcast at 30), then agy
        // (only inbox is... agy sent the broadcast so excludes own; agy has no
        // inbox → not listed).
        let agents: Vec<&str> = rows.iter().map(|r| r.agent.as_str()).collect();
        assert_eq!(agents, vec!["codex", "claude"]);

        let codex = &rows[0];
        assert_eq!(codex.agent, "codex");
        assert_eq!(codex.total, 2); // broadcast(30) + direct urgent(50)
        assert_eq!(codex.unread, 2); // no watermark for codex
        assert_eq!(codex.urgent_unread, 1);

        let claude = &rows[1];
        assert_eq!(claude.total, 2); // direct(10) + broadcast(30)
        assert_eq!(claude.unread, 1); // only the broadcast(30) is past the wm(10)
        assert_eq!(claude.urgent_unread, 0);
    }

    #[test]
    fn agent_summaries_skips_agents_with_empty_inbox() {
        // An agent that has only sent (no inbound) is not listed.
        let msgs = vec![msg(
            "1",
            "t",
            "codex",
            Recipient::Agent("claude".into()),
            10,
        )];
        let wm = std::collections::HashMap::new();
        let rows = agent_summaries(&msgs, &wm);
        let agents: Vec<&str> = rows.iter().map(|r| r.agent.as_str()).collect();
        assert_eq!(agents, vec!["claude"], "only the recipient is listed");
    }

    // trace:BUG-513 | ai:codex
    #[test]
    fn agent_summaries_for_agents_lists_broadcasts_for_unmaterialized_known_inboxes() {
        let msgs = vec![msg("1", "t", "codex", Recipient::Broadcast, 10)];
        let wm = std::collections::HashMap::new();
        let rows = agent_summaries_for_agents(&msgs, &wm, ["advisor"]);
        let agents: Vec<&str> = rows.iter().map(|r| r.agent.as_str()).collect();

        assert_eq!(agents, vec!["advisor"]);
        assert_eq!(rows[0].total, 1);
        assert_eq!(rows[0].unread, 1);
    }

    // ── STORY-585: the notice/read half ──────────────────────────────────

    #[test]
    fn unread_inbox_filters_strictly_past_the_watermark() {
        let msgs = vec![
            msg("a", "t", "codex", Recipient::Agent("claude".into()), 10),
            msg("b", "t", "agy", Recipient::Broadcast, 20),
            msg("c", "t", "codex", Recipient::Agent("claude".into()), 30),
        ];
        // Read up to ts=20 → only c (30) is unread; b (20) is NOT (strictly >).
        let unread: Vec<&str> = unread_inbox("claude", &msgs, Some(20))
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(unread, vec!["c"]);
        // Never read → everything, oldest-first.
        let all: Vec<&str> = unread_inbox("claude", &msgs, None)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(all, vec!["a", "b", "c"]);
    }

    #[test]
    fn build_notice_counts_urgent_and_caps_with_overflow() {
        let msgs = vec![
            msg("1", "t", "x", Recipient::Agent("claude".into()), 10),
            urgent_msg("2", "t", "x", Recipient::Agent("claude".into()), 20),
            msg("3", "t", "x", Recipient::Agent("claude".into()), 30),
            msg("4", "t", "x", Recipient::Agent("claude".into()), 40),
        ];
        let wm = std::collections::HashMap::new();
        let n = build_notice(["claude"], &msgs, &wm, 2);
        assert_eq!(n.total, 4);
        assert_eq!(n.urgent, 1);
        assert_eq!(n.overflow, 2);
        // Keeps the NEWEST 2.
        let shown: Vec<&str> = n.shown.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(shown, vec!["3", "4"]);
    }

    #[test]
    fn build_notice_dedups_broadcast_across_identities_and_uses_max_watermark() {
        // A broadcast both identities see; joe has read it, advisor has not.
        // Reading as EITHER clears it → not unread (past the max watermark).
        let msgs = vec![msg("b", "t", "codex", Recipient::Broadcast, 50)];
        let mut wm = std::collections::HashMap::new();
        wm.insert("joe".to_string(), 99i64); // joe read past it
        wm.insert("advisor".to_string(), 10i64); // advisor has not
        let n = build_notice(["joe", "advisor"], &msgs, &wm, 5);
        assert!(
            n.is_empty(),
            "a broadcast read by any identity is not unread"
        );

        // Neither has read it → unread once (deduped), not twice.
        let wm2 = std::collections::HashMap::new();
        let n2 = build_notice(["joe", "advisor"], &msgs, &wm2, 5);
        assert_eq!(n2.total, 1);
    }

    #[test]
    fn build_notice_surfaces_role_addressed_mail_invisible_to_the_shell_user() {
        // A `--to advisor` handoff: shell user "joe" never sees it, but the
        // session is the advisor — the notice must surface it (the STORY-569
        // handoff case). trace:STORY-585
        let msgs = vec![msg(
            "h",
            "t",
            "implementer",
            Recipient::Agent("advisor".into()),
            10,
        )];
        let wm = std::collections::HashMap::new();
        let joe_only = build_notice(["joe"], &msgs, &wm, 5);
        assert!(joe_only.is_empty(), "not in the shell user's inbox");
        let with_role = build_notice(["joe", "advisor"], &msgs, &wm, 5);
        assert_eq!(with_role.total, 1);
        assert_eq!(with_role.shown[0].from, "implementer");
    }

    #[test]
    fn build_notice_subject_is_first_nonempty_body_line_truncated() {
        let mut m = msg("1", "t", "x", Recipient::Broadcast, 10);
        m.body = "\n  PR ready for review on STORY-585 — please look at the diff and the resolved-design block\nsecond line".to_string();
        let wm = std::collections::HashMap::new();
        let n = build_notice(["claude"], &[m], &wm, 5);
        let subj = &n.shown[0].subject;
        assert!(subj.starts_with("PR ready for review"));
        assert!(subj.ends_with('…'), "long subject is truncated: {subj}");
        assert!(!subj.contains('\n'));
    }
}
