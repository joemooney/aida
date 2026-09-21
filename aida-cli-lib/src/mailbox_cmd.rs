//! `aida mailbox` command cluster — the inter-agent comms surface.
//!
//! Extracted verbatim from `main.rs` (SPIKE-78, pure movement; no behavior
//! change). Covers `aida mailbox send / inbox / notice / list / retract /
//! delete / thread / sync`: sending direct/broadcast messages, reading an
//! agent's inbox (with watermark advance), the ambient unread notice, the
//! per-agent overview + stranded-mail view, retract/delete markers, thread
//! views, and the local→canonical digest.
//!
//! The mailbox-only private helpers (`mailbox_policy`, `resolve_mailbox_message`,
//! `ensure_mailbox_mutation_allowed`, `print_mailbox_line`, …) and the
//! shared ones the notice/statusline surfaces also use (`inbox_identities`,
//! `render_mailbox_notice`, `known_mailbox_identities`,
//! `digest_mailbox_to_canonical`) stay in `main.rs`; this dispatcher reaches
//! them via `crate::`.

use anyhow::Result;
use colored::Colorize;
use std::io::Read;

use crate::cli::MailboxCommand;
use crate::*;

/// Largest recorded read latency among the recipient's 100 most recent
/// acknowledgements. Keeping this calculation shared lets transport-level
/// acknowledgement tests verify the same history consumed by the CLI.
// trace:TASK-1271 | ai:codex
pub(crate) fn max_read_latency_ms(
    project_root: &std::path::Path,
    recipient: &str,
    messages: &[aida_core::mailbox::Message],
) -> Option<i64> {
    let receipts = mailbox_store::read_receipts(project_root, recipient);
    let mut read_latencies: Vec<(i64, i64)> = messages
        .iter()
        .filter_map(|message| {
            receipts
                .get(&message.id)
                .map(|seen| (*seen, seen.saturating_sub(message.timestamp).max(0)))
        })
        .collect();
    read_latencies.sort_by_key(|(seen, _)| std::cmp::Reverse(*seen));
    read_latencies
        .into_iter()
        .take(100)
        .map(|(_, latency)| latency)
        .max()
}

/// Handler for `aida mailbox` — the local layer of the hybrid inter-agent
/// mailbox (STORY-493). Reads/writes `.aida/mailbox/` via the pure
/// `aida_core::mailbox` core; the git-canonical digest is a later slice.
// trace:STORY-493 trace:TASK-603 | ai:claude
pub(crate) fn handle_mailbox_command(
    cmd: &MailboxCommand,
    store_path: &std::path::Path,
) -> Result<()> {
    use aida_core::mailbox::{
        default_inbox_view, inbox_for, merge_dedup, thread as thread_view, Message, Recipient,
    };
    // store_path is the orphan-store worktree root (the canonical layer lives at
    // <store_root>/mailbox); its parent is the project root (the local layer at
    // <project_root>/.aida/mailbox).
    let store_root = store_path;
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;

    match cmd {
        MailboxCommand::Latency { recipient, json } => {
            let local = mailbox_store::read_local_messages(project_root)?;
            let canonical = mailbox_store::read_canonical_messages(store_root)?;
            let merged = merge_dedup(&local, &canonical);
            let identities = recipient.clone().map(|v| vec![v]).unwrap_or_else(|| {
                std::env::var("AIDA_SESSION_ROLE")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .map(|v| vec![v])
                    .unwrap_or_else(|| vec![current_user_id(None)])
            });
            let now = chrono::Utc::now().timestamp_millis();
            let rows: Vec<serde_json::Value> = identities
                .iter()
                .map(|who| {
                    let watermark = mailbox_store::read_watermark(project_root, who);
                    let latency = aida_core::mailbox::mailbox_latency(who, &merged, watermark, now);
                    let max_read = max_read_latency_ms(project_root, who, &merged);
                    serde_json::json!({
                        "recipient": who,
                        "last_seen_at": mailbox_store::read_last_seen(project_root, who),
                        "unread": latency.unread,
                        "oldest_unread_age_ms": latency.oldest_unread_age_ms,
                        "max_read_latency_ms": max_read,
                    })
                })
                .collect();
            if *json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for row in rows {
                    let who = row["recipient"].as_str().unwrap_or("?");
                    let seen = row["last_seen_at"]
                        .as_i64()
                        .map(format_mail_timestamp)
                        .unwrap_or_else(|| "never".into());
                    let unread = row["unread"].as_u64().unwrap_or(0);
                    let oldest = row["oldest_unread_age_ms"]
                        .as_i64()
                        .map(format_mail_age)
                        .unwrap_or_else(|| "—".into());
                    let max_read = row["max_read_latency_ms"]
                        .as_i64()
                        .map(format_mail_age)
                        .unwrap_or_else(|| "—".into());
                    println!("{who}: last read {seen} · {unread} unread · oldest {oldest} · max read latency {max_read}");
                }
            }
            Ok(())
        }
        MailboxCommand::Send {
            to,
            broadcast,
            body,
            subject,
            body_file,
            stdin,
            thread,
            in_reply_to,
            from,
            urgent,
            intent,
        } => {
            let recipient = if *broadcast {
                Recipient::Broadcast
            } else if let Some(agent) = to {
                Recipient::Agent(agent.clone())
            } else {
                anyhow::bail!("specify --to <agent> or --broadcast");
            };
            // BUG-679: dead-letter guard. A direct message whose recipient
            // matches no known role AND no registered agent may never be read
            // (a typo'd name, a wrong role) — warn but still send, since the
            // recipient might be a legitimate not-yet-live identity. Reuses the
            // same known-identity set as the stranded-mail surface. Broadcasts
            // reach everyone, so they are never flagged. trace:BUG-679 | ai:claude
            if let Recipient::Agent(agent) = &recipient {
                let known = known_mailbox_identities(project_root);
                if !known.contains(&agent.trim().to_lowercase()) {
                    eprintln!(
                        "{} '{}' matches no known role or registered agent — this message may never be read.\n  Known roles: {}. Register agents with `aida agent new`.",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        agent.yellow(),
                        AGENT_ROLES.join(", ")
                    );
                }
            }
            // trace:TASK-782 | ai:claude
            let parsed_intent = aida_core::mailbox::Intent::parse(intent).ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid --intent '{intent}'; expected one of: fyi, request, handoff"
                )
            })?;
            let sender = from.clone().unwrap_or_else(|| current_user_id(None));
            let id = uuid::Uuid::new_v4().to_string();
            let body = read_send_body(body.as_deref(), body_file.as_deref(), *stdin)?;
            // BUG-557: a reply must attach to the ORIGINAL message's thread, not
            // open a new one. Precedence: an explicit `--thread` wins; else
            // `--in-reply-to <id>` resolves to that target's thread so the
            // exchange chains under one `aida mailbox thread <id>`; else the
            // message starts its own thread (id == thread_id). A dangling
            // `--in-reply-to` (target not found) is rejected: detaching a reply
            // loses the conversation. Message-id prefixes resolve consistently
            // across every mailbox surface. trace:BUG-557 trace:BUG-1297 | ai:codex
            // BUG-1297 F1: the reply target is resolved WHENEVER --in-reply-to is
            // given, independent of --thread. It used to sit in an ELSE-IF, so
            // `--thread X --in-reply-to Y` took the thread branch, never resolved
            // Y, and wrote the raw user string into the message — an unknown or
            // ambiguous reply id landed unvalidated. Threading still prefers an
            // explicit --thread; validation no longer depends on which branch
            // wins. trace:BUG-1297 | ai:claude
            let merged = if thread.is_some() || in_reply_to.is_some() {
                let local = mailbox_store::read_local_messages(project_root)?;
                let canonical = mailbox_store::read_canonical_messages(store_root)?;
                merge_dedup(&local, &canonical)
            } else {
                Vec::new()
            };
            let mut resolved_reply_id = None;
            let mut reply_target_thread = None;
            if let Some(reply_target) = in_reply_to.as_deref() {
                let target = resolve_mailbox_message(&merged, reply_target).map_err(|error| {
                    anyhow::anyhow!(
                        "--in-reply-to '{reply_target}' was not resolved; reply refused: {error}"
                    )
                })?;
                resolved_reply_id = Some(target.id.clone());
                reply_target_thread = Some(target.thread_id.clone());
            }
            let thread_id = if let Some(t) = thread.as_deref() {
                resolve_mailbox_thread(&merged, t, true)?
            } else if let Some(t) = reply_target_thread {
                t
            } else {
                id.clone()
            };
            let msg = Message {
                id: id.clone(),
                thread_id: thread_id.clone(),
                from: sender,
                to: recipient,
                timestamp: chrono::Utc::now().timestamp_millis(),
                in_reply_to: resolved_reply_id,
                body,
                subject: subject.clone(),
                urgent: *urgent,
                intent: parsed_intent,
                retracted: false,
                deleted: false,
                archived: false,
            };
            mailbox_store::write_message(project_root, &msg)?;
            // STORY-1226: the event fast-path for `on = ["MailReceived"]`
            // schedule jobs (mailbox-triage). Best-effort, never fails the send.
            // trace:STORY-1226 | ai:claude
            crate::events::emit(
                project_root,
                &crate::events::Event::new(
                    None,
                    "",
                    crate::events::EventKind::MailReceived {
                        to: match &msg.to {
                            aida_core::mailbox::Recipient::Agent(a) => a.clone(),
                            aida_core::mailbox::Recipient::Broadcast => "*".to_string(),
                        },
                    },
                ),
            );
            let mut flag = String::new();
            if *urgent {
                flag.push_str(&format!(" {}", "[urgent]".red().bold()));
            }
            // Echo a non-default intent so the sender confirms it landed; fyi is
            // the default, so it stays quiet (mirrors the urgent flag).
            if parsed_intent.is_actionable() {
                flag.push_str(&format!(
                    " {}",
                    format!("[{}]", parsed_intent.as_str()).blue()
                ));
            }
            println!(
                "{} sent {} (thread {}){}",
                crate::glyph(crate::glyphs::Glyph::Mailbox).green(),
                id.cyan(),
                thread_id.dimmed(),
                flag
            );
            Ok(())
        }
        MailboxCommand::Inbox {
            agent,
            all,
            archived,
            peek,
            unread,
            recent_read_tail,
        } => {
            let local = mailbox_store::read_local_messages(project_root)?;
            let canonical = mailbox_store::read_canonical_messages(store_root)?;
            let merged = merge_dedup(&local, &canonical);

            // Operator-wide read-only view: every message across all agents.
            if *all {
                let mut msgs: Vec<&Message> = merged.iter().filter(|m| !m.deleted).collect();
                msgs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
                if msgs.is_empty() {
                    println!(
                        "{} no messages",
                        crate::glyph(crate::glyphs::Glyph::Mailbox).dimmed()
                    );
                    return Ok(());
                }
                println!(
                    "{} {}",
                    "All messages".bold(),
                    format!("({})", msgs.len()).dimmed()
                );
                for m in msgs {
                    print_mailbox_line(m);
                }
                return Ok(());
            }

            // BUG-555: with no explicit --agent, read across the SAME identity
            // set the notice/hook spans (`inbox_identities()` = shell user +
            // session role), not just the shell user. Role-addressed mail (e.g.
            // a handoff to `advisor`) lands in the role's inbox, invisible to
            // the shell user — so the old shell-user-only read never SHOWED nor
            // marked-seen role mail, and the unread nag never cleared. We now
            // union every identity's inbox (dedup by id) and advance EACH
            // identity's watermark, so reading clears exactly what the notice
            // surfaces. trace:BUG-555 | ai:claude
            let who_list: Vec<String> = match agent {
                Some(a) => vec![a.clone()],
                None => inbox_identities(),
            };
            let mut seen_ids = std::collections::HashSet::new();
            let mut inbox: Vec<&Message> = Vec::new();
            for who in &who_list {
                let wm = mailbox_store::read_watermark(project_root, who).unwrap_or(i64::MIN);
                let visible = if *archived {
                    let mut rows: Vec<&Message> = inbox_for(who, &merged)
                        .into_iter()
                        .filter(|m| m.archived)
                        .collect();
                    rows.sort_by(|a, b| {
                        b.timestamp.cmp(&a.timestamp).then_with(|| a.id.cmp(&b.id))
                    });
                    rows
                } else if *unread {
                    inbox_for(who, &merged)
                        .into_iter()
                        .filter(|m| !m.archived && m.timestamp > wm)
                        .collect()
                } else {
                    default_inbox_view(inbox_for(who, &merged), Some(wm), *recent_read_tail)
                };
                for m in visible {
                    // `--unread` filters to messages past THIS identity's
                    // watermark; the seen-mark below still advances to each
                    // identity's full-inbox newest (a filtered read must not
                    // under-advance + resurrect older-but-unread items).
                    if seen_ids.insert(m.id.clone()) {
                        inbox.push(m);
                    }
                }
            }
            if *unread {
                inbox.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
            } else {
                inbox.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| a.id.cmp(&b.id)));
            }
            let who_label = who_list.join(" + ");
            if inbox.is_empty() {
                let label = if *archived {
                    "no archived mail for"
                } else if *unread {
                    "no unread mail for"
                } else {
                    "inbox empty for"
                };
                println!(
                    "{} {} {}",
                    crate::glyph(crate::glyphs::Glyph::Mailbox).dimmed(),
                    label,
                    who_label.cyan()
                );
                return Ok(());
            }
            let header = if *archived {
                "Archived for"
            } else if *unread {
                "Unread for"
            } else {
                "Inbox for"
            };
            println!(
                "{} {}",
                format!("{header} {who_label}").bold(),
                format!("({})", inbox.len()).dimmed()
            );
            for m in &inbox {
                print_mailbox_line(m);
            }
            // Reading marks each identity's inbox seen up to its newest message,
            // so the unread / urgent surfacing clears (STORY-539) — UNLESS
            // `--peek`, which surfaces without consuming (STORY-585 #1/#4). Each
            // mark advances to that identity's FULL inbox newest, not the
            // filtered view's. trace:BUG-555 | ai:claude
            if *archived {
                // Read-only audit view.
            } else if *peek {
                println!(
                    "{}",
                    "  (peek — not marked seen; `aida mailbox inbox` to read + ack)".dimmed()
                );
            } else {
                for who in &who_list {
                    let prior_watermark =
                        mailbox_store::read_watermark(project_root, who).unwrap_or(i64::MIN);
                    let full_inbox = inbox_for(who, &merged);
                    if let Some(newest) = full_inbox.iter().map(|m| m.timestamp).max() {
                        // Receipts describe reads observed by this version of AIDA.
                        // Do not backfill messages already acknowledged by a legacy
                        // watermark: their actual read time is unknowable.
                        // trace:TASK-1271 | ai:codex
                        let ids: Vec<&str> = full_inbox
                            .iter()
                            .filter(|m| m.timestamp > prior_watermark)
                            .map(|m| m.id.as_str())
                            .collect();
                        mailbox_store::record_seen(project_root, who, &ids)?;
                        mailbox_store::set_watermark(project_root, who, newest)?;
                    }
                }
            }
            Ok(())
        }
        MailboxCommand::Notice { agent, cap } => {
            // Ambient, non-marking unread summary for an agent's context (the
            // SessionStart / per-turn hook calls this). Plain text, capped,
            // scoped to the session's identity set; silent when caught up.
            // trace:STORY-585 | ai:claude
            let local = mailbox_store::read_local_messages(project_root)?;
            let canonical = mailbox_store::read_canonical_messages(store_root)?;
            let merged = merge_dedup(&local, &canonical);
            let watermarks = mailbox_store::read_all_watermarks(project_root)?;
            let identities: Vec<String> = match agent {
                Some(a) => vec![a.clone()],
                None => inbox_identities(),
            };
            let cap = cap.unwrap_or(aida_core::mailbox::NOTICE_DEFAULT_CAP);
            let summary = aida_core::mailbox::build_notice(
                identities.iter().map(String::as_str),
                &merged,
                &watermarks,
                cap,
            );
            if summary.is_empty() {
                return Ok(()); // caught up → emit nothing (safe hook no-op)
            }
            print!("{}", render_mailbox_notice(&summary, &identities));
            Ok(())
        }
        MailboxCommand::List { stranded } => {
            let local = mailbox_store::read_local_messages(project_root)?;
            let canonical = mailbox_store::read_canonical_messages(store_root)?;
            let merged = merge_dedup(&local, &canonical);
            let watermarks = mailbox_store::read_all_watermarks(project_root)?;
            // BUG-679: dead-letter view — recipients with unread mail that
            // match no known role / registered agent. Pure classification over
            // the mailbox + the same known-identity set the send-time warning
            // uses, so a misaddressed handoff is visible instead of silently
            // stranded. trace:BUG-679 | ai:claude
            if *stranded {
                let known = known_mailbox_identities(project_root);
                let rows = aida_core::mailbox::stranded_recipients(&merged, &watermarks, |r| {
                    known.contains(&r.trim().to_lowercase())
                });
                if rows.is_empty() {
                    println!(
                        "{} no stranded mail — every recipient with unread mail matches a known role or registered agent",
                        crate::glyph(crate::glyphs::Glyph::Mailbox).dimmed()
                    );
                    return Ok(());
                }
                println!(
                    "{} {}",
                    "Stranded mail".bold(),
                    "(unread mail addressed to no known role or registered agent)".dimmed()
                );
                for s in &rows {
                    let when = chrono::DateTime::from_timestamp_millis(s.latest_ts)
                        .map(|dt| humanize_relative(dt.with_timezone(&chrono::Utc)))
                        .unwrap_or_else(|| "?".to_string());
                    println!(
                        "  {} {:<14} {} unread / {} total  {}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        s.recipient.yellow().bold(),
                        s.unread.to_string().cyan(),
                        s.total,
                        when.dimmed()
                    );
                }
                println!(
                    "{}",
                    "  Check for a typo'd recipient, or register the intended reader with `aida agent new`.".dimmed()
                );
                return Ok(());
            }
            // trace:BUG-513 | ai:codex
            let known_agents: Vec<String> = list_roles(project_root)
                .unwrap_or_default()
                .into_iter()
                .map(|role| role.name)
                .collect();
            let summaries = aida_core::mailbox::agent_summaries_for_agents(
                &merged,
                &watermarks,
                known_agents.iter().map(String::as_str),
            );
            if summaries.is_empty() {
                println!(
                    "{} no agents have mail",
                    crate::glyph(crate::glyphs::Glyph::Mailbox).dimmed()
                );
                return Ok(());
            }
            println!("{}", "Mailbox overview".bold());
            for s in &summaries {
                let when = chrono::DateTime::from_timestamp_millis(s.latest_ts)
                    .map(|dt| humanize_relative(dt.with_timezone(&chrono::Utc)))
                    .unwrap_or_else(|| "?".to_string());
                let unread = if s.unread > 0 {
                    format!("{} unread", s.unread).cyan().to_string()
                } else {
                    "all read".dimmed().to_string()
                };
                let urgent = if s.urgent_unread > 0 {
                    format!(
                        " {}",
                        format!(
                            "{} {} urgent",
                            crate::glyph(crate::glyphs::Glyph::Warning),
                            s.urgent_unread
                        )
                        .red()
                        .bold()
                    )
                } else {
                    String::new()
                };
                println!(
                    "  {:<14} {} total · {}{}  {}",
                    s.agent.yellow().bold(),
                    s.total,
                    unread,
                    urgent,
                    when.dimmed()
                );
            }
            Ok(())
        }
        // A targeted read is deliberately non-consuming: unlike `inbox`, it
        // never advances a watermark or writes a receipt. trace:BUG-1297 | ai:codex
        MailboxCommand::Read { message_id } => {
            let local = mailbox_store::read_local_messages(project_root)?;
            let canonical = mailbox_store::read_canonical_messages(store_root)?;
            let merged = merge_dedup(&local, &canonical);
            let msg = resolve_mailbox_message(&merged, message_id)?;
            print_mailbox_line(msg);
            Ok(())
        }
        MailboxCommand::Retract { message_id } => {
            let policy = mailbox_policy(project_root);
            if !policy.allow_retract {
                anyhow::bail!("mailbox retract is disabled by [mailbox] allow_retract = false");
            }
            let local = mailbox_store::read_local_messages(project_root)?;
            let canonical = mailbox_store::read_canonical_messages(store_root)?;
            let merged = merge_dedup(&local, &canonical);
            let msg = resolve_mailbox_message(&merged, message_id)?;
            if msg.deleted {
                anyhow::bail!("message {} is deleted", message_id);
            }
            ensure_mailbox_mutation_allowed(msg)?;
            let marker = Message {
                retracted: true,
                body: String::new(),
                ..msg.clone()
            };
            mailbox_store::write_message_marker(project_root, &marker)?;
            println!(
                "{} retracted {}",
                crate::glyph(crate::glyphs::Glyph::Mailbox).green(),
                msg.id.cyan()
            );
            Ok(())
        }
        MailboxCommand::Delete { message_id } => {
            let policy = mailbox_policy(project_root);
            if !policy.allow_delete {
                anyhow::bail!("mailbox delete is disabled by [mailbox] allow_delete = false");
            }
            let local = mailbox_store::read_local_messages(project_root)?;
            let canonical = mailbox_store::read_canonical_messages(store_root)?;
            let merged = merge_dedup(&local, &canonical);
            let msg = resolve_mailbox_message(&merged, message_id)?;
            ensure_mailbox_mutation_allowed(msg)?;
            let marker = Message {
                deleted: true,
                retracted: false,
                body: String::new(),
                ..msg.clone()
            };
            mailbox_store::write_message_marker(project_root, &marker)?;
            println!(
                "{} deleted {}",
                crate::glyph(crate::glyphs::Glyph::Mailbox).green(),
                msg.id.cyan()
            );
            Ok(())
        }
        MailboxCommand::Archive {
            message_id,
            older_than,
            read_only,
            agent,
        } => {
            if older_than.is_some() && !*read_only {
                anyhow::bail!("mailbox archive --older-than requires --read-only");
            }
            match (message_id.as_deref(), older_than.as_deref()) {
                (Some(id), None) => {
                    let local = mailbox_store::read_local_messages(project_root)?;
                    let canonical = mailbox_store::read_canonical_messages(store_root)?;
                    let merged = merge_dedup(&local, &canonical);
                    let msg = resolve_mailbox_message(&merged, id)?;
                    if msg.deleted {
                        anyhow::bail!("message {id} is deleted");
                    }
                    if msg.archived {
                        println!(
                            "{} message {} is already archived",
                            crate::glyph(crate::glyphs::Glyph::Mailbox).dimmed(),
                            msg.id.cyan()
                        );
                        return Ok(());
                    }
                    let who_list: Vec<String> = match agent {
                        Some(a) => vec![a.clone()],
                        None => inbox_identities(),
                    };
                    if !message_read_by_all_selected_visible_identities(
                        msg,
                        &merged,
                        project_root,
                        &who_list,
                    ) {
                        anyhow::bail!(
                            "message {id} is unread or not visible to the selected inbox; read it before archiving"
                        );
                    }
                    let marker = Message {
                        archived: true,
                        ..msg.clone()
                    };
                    mailbox_store::write_message_marker(project_root, &marker)?;
                    println!(
                        "{} archived {}",
                        crate::glyph(crate::glyphs::Glyph::Mailbox).green(),
                        msg.id.cyan()
                    );
                    Ok(())
                }
                (None, Some(duration)) => {
                    archive_mailbox_older_than(project_root, store_root, agent.as_deref(), duration)
                }
                _ => anyhow::bail!(
                    "pass a message id or --older-than <duration> (for example: aida mailbox archive <id>)"
                ),
            }
        }
        MailboxCommand::Gc { older_than, agent } => {
            archive_mailbox_older_than(project_root, store_root, agent.as_deref(), older_than)
        }
        MailboxCommand::Thread { thread_id } => {
            let local = mailbox_store::read_local_messages(project_root)?;
            let canonical = mailbox_store::read_canonical_messages(store_root)?;
            let all = merge_dedup(&local, &canonical);
            let resolved_thread = resolve_mailbox_thread(&all, thread_id, false)?;
            let msgs = thread_view(&resolved_thread, &all);
            if msgs.is_empty() {
                anyhow::bail!("thread {resolved_thread} exists but has no visible messages");
            }
            println!(
                "{} {}",
                format!("Thread {resolved_thread}").bold(),
                format!("({})", msgs.len()).dimmed()
            );
            for m in msgs {
                print_mailbox_line(m);
            }
            Ok(())
        }
        MailboxCommand::Sync => {
            // Digest the local layer into the git-canonical layer (orphan
            // store), then stage + commit it. Append-only/id-keyed, so this is
            // idempotent. The orphan branch is pushed by the normal store-sync
            // path (`aida db sync --push` / `aida pull`); this only advances it
            // locally. The same digest is auto-triggered (best-effort) at
            // session-end and drain-end via `maybe_digest_mailbox_best_effort`,
            // so a manual `mailbox sync` is rarely needed. trace:TASK-605
            // trace:STORY-493 | ai:claude
            let n = digest_mailbox_to_canonical(store_root, project_root)?;
            if n == 0 {
                println!(
                    "{} mailbox already in sync (nothing new to digest)",
                    crate::glyph(crate::glyphs::Glyph::Mailbox).dimmed()
                );
                return Ok(());
            }
            println!(
                "{} digested {} message(s) to the canonical store",
                crate::glyph(crate::glyphs::Glyph::Mailbox).green(),
                n.to_string().cyan(),
            );
            Ok(())
        }
    }
}

/// Resolve either an exact thread id or a message id/prefix naming that thread.
/// `--thread` retains support for starting a caller-named thread; the read-only
/// thread verb instead reports an unknown reference distinctly.
// trace:BUG-1297 | ai:codex
fn resolve_mailbox_thread(
    messages: &[aida_core::mailbox::Message],
    query: &str,
    allow_new: bool,
) -> Result<String> {
    if messages.iter().any(|m| m.thread_id == query) {
        return Ok(query.to_string());
    }
    match resolve_mailbox_message(messages, query) {
        Ok(message) => Ok(message.thread_id.clone()),
        // BUG-1297: match the TYPE, not the wording. The previous
        // `error.to_string().contains("ambiguous")` meant rewording that bail
        // message rerouted an ambiguous prefix into the allow_new arm below,
        // silently creating a detached thread. trace:BUG-1297 | ai:claude
        Err(error)
            if matches!(
                error.downcast_ref::<crate::MailboxResolveFailure>(),
                Some(crate::MailboxResolveFailure::AmbiguousPrefix { .. })
            ) =>
        {
            Err(error)
        }
        Err(_) if allow_new => Ok(query.to_string()),
        Err(_) => anyhow::bail!("no such mailbox message or thread: {query}"),
    }
}

// trace:TASK-1250 | ai:codex
fn read_send_body(
    body: Option<&str>,
    body_file: Option<&std::path::Path>,
    stdin: bool,
) -> Result<String> {
    match (body, body_file, stdin) {
        (Some(body), None, false) => Ok(body.to_string()),
        (None, Some(path), false) => std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read --body-file {}: {e}", path.display())),
        (None, None, true) => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| anyhow::anyhow!("failed to read mailbox body from stdin: {e}"))?;
            Ok(buf)
        }
        (None, None, false) => anyhow::bail!(
            "provide exactly one mailbox body source; valid forms: `aida mailbox send <BODY>`, `aida mailbox send --body-file <PATH>`, or `aida mailbox send --stdin`"
        ),
        _ => anyhow::bail!(
            "choose exactly one mailbox body source; valid forms: `aida mailbox send <BODY>`, `aida mailbox send --body-file <PATH>`, or `aida mailbox send --stdin`"
        ),
    }
}

// trace:TASK-1211 | ai:codex
fn archive_mailbox_older_than(
    project_root: &std::path::Path,
    store_root: &std::path::Path,
    agent: Option<&str>,
    duration: &str,
) -> Result<()> {
    use aida_core::mailbox::{archive_candidates, inbox_for, merge_dedup, Message};

    let age = parse_days_arg(duration)
        .map_err(|e| anyhow::anyhow!("invalid --older-than `{duration}`: {e}"))?;
    let cutoff = (chrono::Utc::now() - age).timestamp_millis();
    let local = mailbox_store::read_local_messages(project_root)?;
    let canonical = mailbox_store::read_canonical_messages(store_root)?;
    let merged = merge_dedup(&local, &canonical);
    let who_list: Vec<String> = match agent {
        Some(a) => vec![a.to_string()],
        None => inbox_identities(),
    };

    let mut by_id: std::collections::BTreeMap<String, Message> = std::collections::BTreeMap::new();
    for who in &who_list {
        let wm = mailbox_store::read_watermark(project_root, who);
        for m in archive_candidates(inbox_for(who, &merged), wm, cutoff) {
            if !message_read_by_all_selected_visible_identities(m, &merged, project_root, &who_list)
            {
                continue;
            }
            by_id.entry(m.id.clone()).or_insert_with(|| m.clone());
        }
    }

    if by_id.is_empty() {
        println!(
            "{} no read mailbox messages older than {} to archive",
            crate::glyph(crate::glyphs::Glyph::Mailbox).dimmed(),
            duration
        );
        return Ok(());
    }

    let count = by_id.len();
    for msg in by_id.into_values() {
        let marker = Message {
            archived: true,
            ..msg
        };
        mailbox_store::write_message_marker(project_root, &marker)?;
    }
    println!(
        "{} archived {} read mailbox message(s) older than {}",
        crate::glyph(crate::glyphs::Glyph::Mailbox).green(),
        count.to_string().cyan(),
        duration
    );
    Ok(())
}

// trace:TASK-1211 | ai:codex
fn message_read_by_all_selected_visible_identities(
    msg: &aida_core::mailbox::Message,
    messages: &[aida_core::mailbox::Message],
    project_root: &std::path::Path,
    identities: &[String],
) -> bool {
    let mut visible_to_any = false;
    for who in identities {
        if !aida_core::mailbox::inbox_for(who, messages)
            .iter()
            .any(|m| m.id == msg.id)
        {
            continue;
        }
        visible_to_any = true;
        let wm = mailbox_store::read_watermark(project_root, who).unwrap_or(i64::MIN);
        if msg.timestamp > wm {
            return false;
        }
    }
    visible_to_any
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command, MailboxCommand};
    use clap::Parser;

    fn message(id: &str, thread_id: &str) -> aida_core::mailbox::Message {
        aida_core::mailbox::Message {
            id: id.into(),
            thread_id: thread_id.into(),
            from: "alice".into(),
            to: aida_core::mailbox::Recipient::Agent("bob".into()),
            timestamp: 1,
            in_reply_to: None,
            body: "body".into(),
            subject: None,
            urgent: false,
            intent: aida_core::mailbox::Intent::Fyi,
            retracted: false,
            deleted: false,
            archived: false,
        }
    }

    // BUG-1297: an ambiguous prefix must be REFUSED by resolve_mailbox_thread,
    // and the refusal must not depend on the wording of the error. The previous
    // implementation branched on `to_string().contains("ambiguous")`, so this
    // asserts the discriminant is carried by the TYPE: the message text is
    // deliberately never inspected here.
    // trace:BUG-1297 | ai:claude
    #[test]
    fn ambiguous_prefix_is_refused_by_type_not_by_message_text() {
        let messages = vec![
            message("0fa629d6-1111", "thread-a"),
            message("0fa629d6-2222", "thread-b"),
        ];

        // allow_new = TRUE is the dangerous direction: under the old prose match,
        // a reworded message fell through to this arm and silently created a
        // detached thread from an ambiguous id.
        let err = resolve_mailbox_thread(&messages, "0fa629d6", true)
            .expect_err("an ambiguous prefix must never become a new thread");
        assert!(
            matches!(
                err.downcast_ref::<crate::MailboxResolveFailure>(),
                Some(crate::MailboxResolveFailure::AmbiguousPrefix { .. })
            ),
            "the refusal must carry the typed discriminant, got: {err}"
        );

        // and an UNKNOWN id under allow_new is still allowed through — the two
        // failures must stay distinguishable, or this fix would just refuse
        // everything and look correct.
        assert_eq!(
            resolve_mailbox_thread(&messages, "no-such-id", true).unwrap(),
            "no-such-id"
        );
    }

    // trace:BUG-1297 | ai:codex
    #[test]
    fn mailbox_message_prefix_resolves_and_ambiguity_lists_candidates() {
        let messages = vec![
            message("0fa629d6-1111", "thread-a"),
            message("0fa629d6-2222", "thread-b"),
            message("abcd1234-3333", "thread-c"),
        ];

        assert_eq!(
            resolve_mailbox_message(&messages, "abcd1234").unwrap().id,
            "abcd1234-3333"
        );
        let error = resolve_mailbox_message(&messages, "0fa629d6")
            .unwrap_err()
            .to_string();
        assert!(error.contains("ambiguous"), "{error}");
        assert!(error.contains("0fa629d6-1111"), "{error}");
        assert!(error.contains("0fa629d6-2222"), "{error}");
    }

    // trace:BUG-1297 | ai:codex
    #[test]
    fn mailbox_thread_accepts_message_prefix_and_rejects_unknown_reference() {
        let messages = vec![message("abcd1234-3333", "thread-c")];

        assert_eq!(
            resolve_mailbox_thread(&messages, "abcd1234", false).unwrap(),
            "thread-c"
        );
        let error = resolve_mailbox_thread(&messages, "missing", false)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "no such mailbox message or thread: missing");
        assert_eq!(
            resolve_mailbox_thread(&messages, "new-thread", true).unwrap(),
            "new-thread"
        );
    }

    // trace:BUG-1297 | ai:codex
    #[test]
    fn mailbox_read_cli_accepts_a_message_prefix() {
        let cli = Cli::try_parse_from(["aida", "mailbox", "read", "0fa629d6"]).unwrap();
        match cli.command {
            Command::Mailbox(MailboxCommand::Read { message_id }) => {
                assert_eq!(message_id, "0fa629d6")
            }
            other => panic!("expected mailbox read, got {other:?}"),
        }
    }

    // A message remains addressable after an inbox watermark has passed it;
    // targeted reads neither consult nor mutate that watermark.
    // trace:BUG-1297 | ai:codex
    #[test]
    fn mailbox_read_can_reread_seen_message_without_changing_watermark() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        let msg = message("abcd1234-3333", "thread-c");
        mailbox_store::write_message(project.path(), &msg).unwrap();
        mailbox_store::set_watermark(project.path(), "bob", 99).unwrap();

        handle_mailbox_command(
            &MailboxCommand::Read {
                message_id: "abcd1234".into(),
            },
            &store,
        )
        .unwrap();
        handle_mailbox_command(
            &MailboxCommand::Read {
                message_id: "abcd1234".into(),
            },
            &store,
        )
        .unwrap();

        assert_eq!(
            mailbox_store::read_watermark(project.path(), "bob"),
            Some(99)
        );
    }

    // trace:BUG-1297 | ai:codex
    #[test]
    fn mailbox_reply_to_unknown_message_refuses_without_writing() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        let command = MailboxCommand::Send {
            to: Some("bob".into()),
            broadcast: false,
            body: Some("reply".into()),
            subject: None,
            body_file: None,
            stdin: false,
            thread: None,
            in_reply_to: Some("missing".into()),
            from: Some("alice".into()),
            urgent: false,
            intent: "fyi".into(),
        };

        let error = handle_mailbox_command(&command, &store)
            .unwrap_err()
            .to_string();
        assert!(error.contains("reply refused"), "{error}");
        assert!(mailbox_store::read_local_messages(project.path())
            .unwrap()
            .is_empty());
    }

    // BUG-1297 F1: --thread AND --in-reply-to together used to bypass reply
    // validation entirely. The --thread branch was taken first and the
    // in-reply-to resolution sat in an ELSE-IF, so the raw user string was
    // written into the message unvalidated. Both assertions matter: the unknown
    // id must be REFUSED even though --thread is present, and nothing must be
    // written.
    // trace:BUG-1297 | ai:claude
    #[test]
    fn thread_flag_does_not_bypass_reply_id_validation() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        let existing = message("abcd1234-3333", "thread-c");
        mailbox_store::write_message(project.path(), &existing).unwrap();

        let command = MailboxCommand::Send {
            to: Some("bob".into()),
            broadcast: false,
            body: Some("reply".into()),
            subject: None,
            body_file: None,
            stdin: false,
            thread: Some("thread-c".into()),
            in_reply_to: Some("definitely-not-a-real-id".into()),
            from: Some("alice".into()),
            urgent: false,
            intent: "fyi".into(),
        };

        let error = handle_mailbox_command(&command, &store)
            .expect_err("an unknown --in-reply-to must be refused even with --thread given")
            .to_string();
        assert!(error.contains("reply refused"), "{error}");
        assert_eq!(
            mailbox_store::read_local_messages(project.path())
                .unwrap()
                .len(),
            1,
            "only the pre-existing message may be present; the refused send must not write"
        );
    }

    // BUG-1297 F1, the other direction: with BOTH flags and a VALID reply
    // target, --thread still decides threading and the parent id is recorded in
    // full. Without this, the fix could satisfy the refusal test by simply
    // refusing whenever both flags appear.
    // trace:BUG-1297 | ai:claude
    #[test]
    fn thread_flag_wins_threading_while_reply_id_is_still_resolved() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        mailbox_store::write_message(project.path(), &message("abcd1234-3333", "thread-c"))
            .unwrap();
        mailbox_store::write_message(project.path(), &message("eeee5555-7777", "thread-z"))
            .unwrap();

        let command = MailboxCommand::Send {
            to: Some("bob".into()),
            broadcast: false,
            body: Some("reply".into()),
            subject: None,
            body_file: None,
            stdin: false,
            thread: Some("thread-z".into()),
            in_reply_to: Some("abcd1234".into()),
            from: Some("alice".into()),
            urgent: false,
            intent: "fyi".into(),
        };
        handle_mailbox_command(&command, &store).expect("valid reply target must succeed");

        let sent = mailbox_store::read_local_messages(project.path())
            .unwrap()
            .into_iter()
            .find(|m| m.body == "reply")
            .expect("the reply must be written");
        assert_eq!(
            sent.in_reply_to.as_deref(),
            Some("abcd1234-3333"),
            "the PREFIX must be resolved to the full parent id, not stored raw"
        );
        assert_eq!(
            sent.thread_id, "thread-z",
            "--thread must still decide threading"
        );
    }

    // trace:BUG-1297 | ai:codex
    #[test]
    fn mailbox_reply_resolves_prefix_and_records_full_parent_id() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        let original = message("abcd1234-3333", "thread-c");
        mailbox_store::write_message(project.path(), &original).unwrap();
        let command = MailboxCommand::Send {
            to: Some("alice".into()),
            broadcast: false,
            body: Some("reply".into()),
            subject: None,
            body_file: None,
            stdin: false,
            thread: None,
            in_reply_to: Some("abcd1234".into()),
            from: Some("bob".into()),
            urgent: false,
            intent: "fyi".into(),
        };

        handle_mailbox_command(&command, &store).unwrap();

        let messages = mailbox_store::read_local_messages(project.path()).unwrap();
        let reply = messages.iter().find(|m| m.body == "reply").unwrap();
        assert_eq!(reply.thread_id, "thread-c");
        assert_eq!(reply.in_reply_to.as_deref(), Some("abcd1234-3333"));
    }

    // trace:TASK-1250 | ai:codex
    #[test]
    fn mailbox_send_body_file_preserves_shell_sensitive_text() {
        let dir = tempfile::tempdir().unwrap();
        let body_path = dir.path().join("mail.txt");
        let expected = "please run `aida queue next`\nnot $(clear)\n";
        std::fs::write(&body_path, expected).unwrap();

        let body = read_send_body(None, Some(&body_path), false).unwrap();

        assert_eq!(body, expected);
    }

    // trace:TASK-1250 | ai:codex
    #[test]
    fn mailbox_send_requires_exactly_one_body_source() {
        let dir = tempfile::tempdir().unwrap();
        let body_path = dir.path().join("mail.txt");
        std::fs::write(&body_path, "from file").unwrap();

        let err = read_send_body(None, None, false).unwrap_err().to_string();
        assert!(err.contains("send <BODY>"));
        assert!(err.contains("send --body-file <PATH>"));
        assert!(err.contains("send --stdin"));
        assert!(read_send_body(Some("arg"), Some(&body_path), false).is_err());
        assert!(read_send_body(Some("arg"), None, true).is_err());
        assert!(read_send_body(None, Some(&body_path), true).is_err());
    }

    // trace:TASK-1250 | ai:codex
    #[test]
    fn mailbox_send_cli_accepts_file_or_stdin_without_positional_body() {
        let from_file = Cli::try_parse_from([
            "aida",
            "mailbox",
            "send",
            "--to",
            "codex",
            "--body-file",
            "mail.txt",
            "--subject",
            "Review requested",
        ])
        .unwrap();
        match from_file.command {
            Command::Mailbox(MailboxCommand::Send {
                body,
                body_file,
                subject,
                ..
            }) => {
                assert!(body.is_none());
                assert_eq!(body_file.unwrap(), std::path::PathBuf::from("mail.txt"));
                assert_eq!(subject.as_deref(), Some("Review requested"));
            }
            other => panic!("expected mailbox send, got {other:?}"),
        }

        let from_stdin =
            Cli::try_parse_from(["aida", "mailbox", "send", "--broadcast", "--stdin"]).unwrap();
        match from_stdin.command {
            Command::Mailbox(MailboxCommand::Send { body, stdin, .. }) => {
                assert!(body.is_none());
                assert!(stdin);
            }
            other => panic!("expected mailbox send, got {other:?}"),
        }
    }
}
