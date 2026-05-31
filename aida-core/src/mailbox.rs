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
}
