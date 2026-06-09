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
}

impl Message {
    /// Is this message visible in `agent`'s inbox? Addressed to it directly or
    /// broadcast — but not its own sent messages (an agent doesn't inbox what
    /// it sent, including its own broadcasts).
    fn addressed_to(&self, agent: &str) -> bool {
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
        .filter(|m| m.thread_id == thread_id)
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
    // Insert local first, then let canonical overwrite on collision.
    for m in local {
        by_id.insert(m.id.as_str(), m);
    }
    for m in canonical {
        by_id.insert(m.id.as_str(), m);
    }
    let mut out: Vec<Message> = by_id.into_values().cloned().collect();
    out.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
    out
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
    use std::collections::BTreeSet;
    // Known agents = every explicit direct-recipient + every sender (so an
    // agent that has only ever sent still shows once it receives a broadcast)
    // + every agent with a recorded watermark.
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
}
