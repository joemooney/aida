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
use std::path::{Path, PathBuf};

/// A message recipient: a specific agent, or every agent (broadcast).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "agent", rename_all = "snake_case")]
pub enum Recipient {
    Agent(String),
    Broadcast,
}

/// The *interpreted intent* of a message (TASK-782): how the recipient should
/// treat it, an axis **orthogonal** to `urgent` (urgency is "how loud"; intent
/// is "what kind").
///
/// - `Fyi` — purely informational. Surface only; no action is expected.
/// - `Request` — asks the recipient to do or answer something. Actionable.
/// - `Handoff` — transfers a piece of work to the recipient. Actionable.
///
/// Defaults to `Fyi` so messages written before this field existed (and the
/// common informational case) deserialize unchanged — append-only and
/// non-breaking, the same pattern as `urgent`. Mail is **interpreted input,
/// not a command channel**: an actionable intent is a *recommendation* to act,
/// never an authenticated directive — see [`mail_disposition`].
/// trace:TASK-782 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    #[default]
    Fyi,
    Request,
    Handoff,
}

impl Intent {
    /// Parse a CLI/MCP intent token (case-insensitive); `None` if unrecognized.
    pub fn parse(s: &str) -> Option<Intent> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fyi" => Some(Intent::Fyi),
            "request" => Some(Intent::Request),
            "handoff" => Some(Intent::Handoff),
            _ => None,
        }
    }

    /// The lowercase token form (matches the serde rename and CLI input).
    pub fn as_str(&self) -> &'static str {
        match self {
            Intent::Fyi => "fyi",
            Intent::Request => "request",
            Intent::Handoff => "handoff",
        }
    }

    /// Does this intent expect the recipient to act, versus purely inform?
    /// `request` / `handoff` are actionable; `fyi` is not.
    pub fn is_actionable(&self) -> bool {
        matches!(self, Intent::Request | Intent::Handoff)
    }
}

/// Project policy for how an agent treats *actionable* received mail
/// (TASK-782, the act-vs-prompt knob; configured under `[mailbox] act_on_mail`).
/// The safe default is [`ActOnMail::SurfaceAndRecommend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActOnMail {
    /// Interactive sessions: surface the message and recommend an action, but
    /// never auto-act — the human (or the agent at the keyboard) decides.
    #[default]
    SurfaceAndRecommend,
    /// Headless sessions: route actionable mail through the implementer →
    /// advisor → human escalation cascade rather than acting on it blindly.
    EscalatePerCascade,
}

impl ActOnMail {
    /// Parse a config token (case-insensitive, accepts `-` or `_`); `None` if
    /// unrecognized so callers can fall back to the default.
    pub fn parse(s: &str) -> Option<ActOnMail> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "surface-and-recommend" => Some(ActOnMail::SurfaceAndRecommend),
            "escalate-per-cascade" => Some(ActOnMail::EscalatePerCascade),
            _ => None,
        }
    }
}

/// What the read pipeline should do with a received message, given its intent
/// and the project's [`ActOnMail`] policy (TASK-782). This encodes the integrity
/// floor in the type system: the strongest disposition is *escalate* — never
/// "auto-execute blindly". `Fyi` always merely surfaces; an actionable message
/// surfaces-and-recommends in interactive sessions and escalates-per-cascade in
/// headless ones. Bounded-safe auto-action (if any) is the caller's judgment
/// layered on top of this; ambiguous or destructive actions always surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailDisposition {
    /// Inform only — no action expected (every `fyi`).
    Surface,
    /// Surface the message and recommend an action; do not auto-act.
    SurfaceAndRecommend,
    /// Route the actionable message through the escalation cascade.
    EscalatePerCascade,
}

/// Decide a received message's disposition from its `intent` and the project
/// `policy`. Pure and total — the act-vs-prompt seam the read surface calls so
/// the policy is interpreted in exactly one place. trace:TASK-782 | ai:claude
pub fn mail_disposition(intent: Intent, policy: ActOnMail) -> MailDisposition {
    if !intent.is_actionable() {
        return MailDisposition::Surface;
    }
    match policy {
        ActOnMail::SurfaceAndRecommend => MailDisposition::SurfaceAndRecommend,
        ActOnMail::EscalatePerCascade => MailDisposition::EscalatePerCascade,
    }
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
    /// Interpreted intent (TASK-782): `fyi` (informational, surface only) vs
    /// `request` / `handoff` (actionable). Defaults to `fyi` for messages
    /// written before this field existed — append-only and non-breaking, the
    /// same pattern as `urgent`. Orthogonal to `urgent`: urgency is "how loud",
    /// intent is "what kind". trace:TASK-782 | ai:claude
    #[serde(default)]
    pub intent: Intent,
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

/// BUG-557: the thread a reply joins when sent `--in-reply-to <target>`: the
/// target message's own `thread_id`, or `None` when no message with that id is
/// known among `messages` (a dangling reference — the caller starts a fresh
/// thread and warns). Without this the send path ignored `--in-reply-to` and
/// every reply opened a new thread. trace:BUG-557 | ai:claude
pub fn reply_target_thread(target: &str, messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .find(|m| m.id == target)
        .map(|m| m.thread_id.clone())
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

// trace:BUG-679 | ai:claude
/// One dead-letter recipient (`aida mailbox list --stranded` / `aida doctor`):
/// a direct recipient string that has unread mail addressed to it but matches
/// no known role / registered agent — a misaddressed handoff no live identity
/// will ever read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrandedRecipient {
    /// The unrecognized recipient string (as addressed).
    pub recipient: String,
    /// Total non-deleted direct messages addressed to it.
    pub total: usize,
    /// Of those, how many are unread relative to the recipient's watermark.
    pub unread: usize,
    /// Timestamp of the most recent direct message (for sort + display).
    pub latest_ts: i64,
}

/// Dead-letter detection: recipients with UNREAD direct mail that match no
/// known identity. `is_known(recipient)` decides membership — the caller owns
/// the known-identity set (canonical roles + role files + registered agents)
/// and any case-folding. Only explicit `Recipient::Agent(..)` targets are
/// considered — a broadcast reaches everyone, so it can never be "stranded".
/// A recipient is listed only when it has at least one unread direct message,
/// so a fully-read misaddressed thread stops nagging. Ordered
/// most-recent-first. Pure over the mailbox + the known set (no I/O), so the
/// CLI surface and `aida doctor` compose over it without re-deriving the
/// classification.
// trace:BUG-679 | ai:claude
pub fn stranded_recipients<F>(
    messages: &[Message],
    watermarks: &std::collections::HashMap<String, i64>,
    is_known: F,
) -> Vec<StrandedRecipient>
where
    F: Fn(&str) -> bool,
{
    use std::collections::BTreeSet;
    // Distinct direct-recipient strings across all live (non-deleted) mail.
    let mut recipients: BTreeSet<&str> = BTreeSet::new();
    for m in messages {
        if m.deleted {
            continue;
        }
        if let Recipient::Agent(a) = &m.to {
            recipients.insert(a.as_str());
        }
    }
    let mut out: Vec<StrandedRecipient> = Vec::new();
    for r in recipients {
        if is_known(r) {
            continue;
        }
        // Direct messages addressed to this recipient (broadcasts excluded; a
        // recipient's own sends excluded to mirror `inbox_for` semantics).
        let direct: Vec<&Message> = messages
            .iter()
            .filter(|m| !m.deleted && m.from != r && matches!(&m.to, Recipient::Agent(a) if a == r))
            .collect();
        if direct.is_empty() {
            continue;
        }
        let mark = watermarks.get(r).copied().unwrap_or(i64::MIN);
        let unread = direct.iter().filter(|m| m.timestamp > mark).count();
        if unread == 0 {
            continue; // read (even if by no live reader) — not a live dead-letter
        }
        let latest_ts = direct.iter().map(|m| m.timestamp).max().unwrap_or(0);
        out.push(StrandedRecipient {
            recipient: r.to_string(),
            total: direct.len(),
            unread,
            latest_ts,
        });
    }
    out.sort_by(|a, b| {
        b.latest_ts
            .cmp(&a.latest_ts)
            .then_with(|| a.recipient.cmp(&b.recipient))
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
/// one-line subject (first non-empty body line, truncated), urgency, the
/// interpreted intent (so an agent can tell an FYI from an actionable
/// request/handoff at the notice level, not only after opening the inbox —
/// TASK-790), and the thread id to read the rest. trace:STORY-585 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeItem {
    pub id: String,
    pub from: String,
    pub thread_id: String,
    pub subject: String,
    pub urgent: bool,
    /// Interpreted intent (TASK-782): `fyi` vs `request` / `handoff`. The notice
    /// renderer surfaces only the actionable ones, matching `aida mailbox inbox`.
    // trace:TASK-790 | ai:claude
    pub intent: Intent,
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
            // trace:TASK-790 | ai:claude
            intent: m.intent,
        })
        .collect();
    NoticeSummary {
        total,
        urgent,
        overflow,
        shown,
    }
}

// ── Local mailbox file I/O (the fast `.aida/mailbox/` layer) ─────────────────
//
// The hybrid mailbox's LOCAL layer is one JSON file per message under
// `<project_root>/.aida/mailbox/`. The CLI (`aida-cli::mailbox_store`) and the
// REST server (`aida-server`) are both writers of this layer — e.g. an
// assignment notification fires from `aida assign` AND from
// `PUT /api/v2/requirements/:id/assignee`. Hosting the file-write side here in
// aida-core keeps the two surfaces from drifting (the recurring STORY-82
// hazard). trace:STORY-650 | ai:claude

/// The local mailbox directory: `<project_root>/.aida/mailbox/`.
pub fn local_mailbox_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("mailbox")
}

/// Neutralize an id into a safe filename component (path separators, etc.).
/// Mirrors the CLI's `mailbox_store::sanitize_id`. trace:STORY-650 | ai:claude
fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Append one message to the LOCAL layer (`<project_root>/.aida/mailbox/`),
/// written atomically and named by id. Append-only: ids are unique, so this
/// never clobbers an existing message. trace:STORY-650 | ai:claude
pub fn write_local_message(project_root: &Path, msg: &Message) -> std::io::Result<()> {
    let dir = local_mailbox_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", sanitize_id(&msg.id)));
    let json = serde_json::to_string_pretty(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::write_atomic(&path, json.as_bytes())
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
            intent: Intent::Fyi,
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

    /// BUG-557: a reply `--in-reply-to <orig>` resolves to the original's
    /// thread (so the exchange chains), and a dangling reference resolves to
    /// `None` so the caller can start a fresh thread.
    #[test]
    fn reply_target_thread_resolves_original_thread_or_none() {
        let msgs = vec![
            msg(
                "orig",
                "thread-A",
                "alice",
                Recipient::Agent("bob".into()),
                10,
            ),
            msg(
                "other",
                "thread-B",
                "carol",
                Recipient::Agent("bob".into()),
                20,
            ),
        ];
        // Reply to "orig" joins its thread, not a new one.
        assert_eq!(
            reply_target_thread("orig", &msgs),
            Some("thread-A".to_string())
        );
        // A reply to a message in another thread joins THAT thread.
        assert_eq!(
            reply_target_thread("other", &msgs),
            Some("thread-B".to_string())
        );
        // Dangling --in-reply-to → None (caller starts a fresh thread + warns).
        assert_eq!(reply_target_thread("does-not-exist", &msgs), None);
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

    // trace:STORY-643 | ai:claude
    #[test]
    fn merge_dedup_surfaces_a_canonical_message_addressed_to_an_identity() {
        // The auto-sync receive side: a foreign message exists ONLY in the
        // canonical layer (another clone published it; we pulled it down). With
        // an empty local layer, merge_dedup + inbox_for must still surface it to
        // its addressee — this is what makes a pulled message visible without a
        // manual digest. A broadcast must likewise reach a clone that never sent
        // or received locally.
        let local: Vec<Message> = Vec::new();
        let canonical = vec![
            msg("direct", "t", "alice", Recipient::Agent("bob".into()), 10),
            msg("bcast", "t", "alice", Recipient::Broadcast, 20),
        ];
        let merged = merge_dedup(&local, &canonical);
        let bob_inbox: Vec<&str> = inbox_for("bob", &merged)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(
            bob_inbox,
            vec!["direct", "bcast"],
            "canonical-only direct + broadcast reach bob after a pull"
        );
        // A third clone (never the recipient of the direct) still sees the broadcast.
        let carol_inbox: Vec<&str> = inbox_for("carol", &merged)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(carol_inbox, vec!["bcast"], "broadcast reaches carol too");
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
        assert_eq!(m.intent, Intent::Fyi, "absent intent field defaults to fyi");
    }

    // trace:TASK-782 | ai:claude
    #[test]
    fn intent_parses_and_round_trips_through_snake_case() {
        assert_eq!(Intent::parse("fyi"), Some(Intent::Fyi));
        assert_eq!(Intent::parse("REQUEST"), Some(Intent::Request));
        assert_eq!(Intent::parse(" Handoff "), Some(Intent::Handoff));
        assert_eq!(Intent::parse("nope"), None);
        for i in [Intent::Fyi, Intent::Request, Intent::Handoff] {
            let s = serde_json::to_string(&i).unwrap();
            assert_eq!(s, format!("\"{}\"", i.as_str()));
            assert_eq!(serde_json::from_str::<Intent>(&s).unwrap(), i);
            assert_eq!(Intent::parse(i.as_str()), Some(i));
        }
        assert!(!Intent::Fyi.is_actionable());
        assert!(Intent::Request.is_actionable());
        assert!(Intent::Handoff.is_actionable());
    }

    // trace:TASK-782 | ai:claude
    #[test]
    fn mail_disposition_keeps_integrity_floor_fyi_never_acts() {
        // FYI only ever surfaces, regardless of policy.
        for policy in [
            ActOnMail::SurfaceAndRecommend,
            ActOnMail::EscalatePerCascade,
        ] {
            assert_eq!(
                mail_disposition(Intent::Fyi, policy),
                MailDisposition::Surface
            );
        }
        // Actionable mail follows the policy; the strongest disposition is
        // escalate — never blind auto-execution.
        assert_eq!(
            mail_disposition(Intent::Request, ActOnMail::SurfaceAndRecommend),
            MailDisposition::SurfaceAndRecommend
        );
        assert_eq!(
            mail_disposition(Intent::Handoff, ActOnMail::EscalatePerCascade),
            MailDisposition::EscalatePerCascade
        );
    }

    // trace:TASK-782 | ai:claude
    #[test]
    fn act_on_mail_parses_and_defaults_safe() {
        assert_eq!(ActOnMail::default(), ActOnMail::SurfaceAndRecommend);
        assert_eq!(
            ActOnMail::parse("surface-and-recommend"),
            Some(ActOnMail::SurfaceAndRecommend)
        );
        assert_eq!(
            ActOnMail::parse("escalate_per_cascade"),
            Some(ActOnMail::EscalatePerCascade)
        );
        assert_eq!(ActOnMail::parse("bogus"), None);
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

    // ── BUG-679: dead-letter detection ───────────────────────────────────

    #[test]
    fn stranded_recipients_flags_only_unknown_unread_direct_recipients() {
        let msgs = vec![
            // Unknown recipient with unread direct mail → stranded.
            msg("1", "t1", "advisor", Recipient::Agent("bob".into()), 10),
            msg("2", "t2", "advisor", Recipient::Agent("bob".into()), 20),
            // Known recipient with unread direct mail → NOT stranded.
            msg(
                "3",
                "t3",
                "advisor",
                Recipient::Agent("implementer".into()),
                30,
            ),
            // Broadcast → never stranded (reaches everyone).
            msg("4", "t4", "advisor", Recipient::Broadcast, 40),
        ];
        let wm = std::collections::HashMap::new();
        let known: std::collections::HashSet<&str> =
            ["advisor", "implementer", "reviewer", "integrator"]
                .into_iter()
                .collect();
        let rows = stranded_recipients(&msgs, &wm, |r| known.contains(r));
        let names: Vec<&str> = rows.iter().map(|s| s.recipient.as_str()).collect();
        assert_eq!(names, vec!["bob"], "only the unknown recipient is stranded");
        assert_eq!(rows[0].total, 2);
        assert_eq!(rows[0].unread, 2);
        assert_eq!(rows[0].latest_ts, 20);
    }

    #[test]
    fn stranded_recipients_excludes_fully_read_and_deleted_mail() {
        let msgs = vec![
            msg("1", "t1", "advisor", Recipient::Agent("ghost".into()), 10),
            msg("2", "t2", "advisor", Recipient::Agent("ghost".into()), 20),
            Message {
                deleted: true,
                ..msg("3", "t3", "advisor", Recipient::Agent("gone".into()), 30)
            },
        ];
        // ghost has read everything (watermark past its newest) → not stranded.
        let mut wm = std::collections::HashMap::new();
        wm.insert("ghost".to_string(), 20i64);
        let none_known = |_: &str| false;
        let rows = stranded_recipients(&msgs, &wm, none_known);
        assert!(
            rows.is_empty(),
            "fully-read + deleted-only recipients are not stranded, got {rows:?}"
        );
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
