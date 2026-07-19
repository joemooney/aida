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

use crate::cli::MailboxCommand;
use crate::*;

/// Handler for `aida mailbox` — the local layer of the hybrid inter-agent
/// mailbox (STORY-493). Reads/writes `.aida/mailbox/` via the pure
/// `aida_core::mailbox` core; the git-canonical digest is a later slice.
// trace:STORY-493 trace:TASK-603 | ai:claude
pub(crate) fn handle_mailbox_command(
    cmd: &MailboxCommand,
    store_path: &std::path::Path,
) -> Result<()> {
    use aida_core::mailbox::{inbox_for, merge_dedup, thread as thread_view, Message, Recipient};
    // store_path is the orphan-store worktree root (the canonical layer lives at
    // <store_root>/mailbox); its parent is the project root (the local layer at
    // <project_root>/.aida/mailbox).
    let store_root = store_path;
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;

    match cmd {
        MailboxCommand::Send {
            to,
            broadcast,
            body,
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
            // BUG-557: a reply must attach to the ORIGINAL message's thread, not
            // open a new one. Precedence: an explicit `--thread` wins; else
            // `--in-reply-to <id>` resolves to that target's thread so the
            // exchange chains under one `aida mailbox thread <id>`; else the
            // message starts its own thread (id == thread_id). A dangling
            // `--in-reply-to` (target not found) warns and falls back to a new
            // thread rather than silently mis-threading. trace:BUG-557 | ai:claude
            let thread_id = if let Some(t) = thread.clone() {
                t
            } else if let Some(reply_target) = in_reply_to.as_deref() {
                let local = mailbox_store::read_local_messages(project_root)?;
                let canonical = mailbox_store::read_canonical_messages(store_root)?;
                let merged = merge_dedup(&local, &canonical);
                aida_core::mailbox::reply_target_thread(reply_target, &merged).unwrap_or_else(
                    || {
                        eprintln!(
                            "{} --in-reply-to '{}' matches no known message; starting a new thread",
                            "warning:".yellow(),
                            reply_target
                        );
                        id.clone()
                    },
                )
            } else {
                id.clone()
            };
            let msg = Message {
                id: id.clone(),
                thread_id: thread_id.clone(),
                from: sender,
                to: recipient,
                timestamp: chrono::Utc::now().timestamp_millis(),
                in_reply_to: in_reply_to.clone(),
                body: body.clone(),
                urgent: *urgent,
                intent: parsed_intent,
                retracted: false,
                deleted: false,
            };
            mailbox_store::write_message(project_root, &msg)?;
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
            peek,
            unread,
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
                for m in inbox_for(who, &merged) {
                    // `--unread` filters to messages past THIS identity's
                    // watermark; the seen-mark below still advances to each
                    // identity's full-inbox newest (a filtered read must not
                    // under-advance + resurrect older-but-unread items).
                    if *unread && m.timestamp <= wm {
                        continue;
                    }
                    if seen_ids.insert(m.id.clone()) {
                        inbox.push(m);
                    }
                }
            }
            inbox.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
            let who_label = who_list.join(" + ");
            if inbox.is_empty() {
                let label = if *unread {
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
            let header = if *unread { "Unread for" } else { "Inbox for" };
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
            if *peek {
                println!(
                    "{}",
                    "  (peek — not marked seen; `aida mailbox inbox` to read + ack)".dimmed()
                );
            } else {
                for who in &who_list {
                    if let Some(newest) = inbox_for(who, &merged).iter().map(|m| m.timestamp).max()
                    {
                        let _ = mailbox_store::set_watermark(project_root, who, newest);
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
        MailboxCommand::Thread { thread_id } => {
            let local = mailbox_store::read_local_messages(project_root)?;
            let canonical = mailbox_store::read_canonical_messages(store_root)?;
            let all = merge_dedup(&local, &canonical);
            let msgs = thread_view(thread_id, &all);
            if msgs.is_empty() {
                println!(
                    "{} no messages in thread {}",
                    crate::glyph(crate::glyphs::Glyph::Mailbox).dimmed(),
                    thread_id.cyan()
                );
                return Ok(());
            }
            println!(
                "{} {}",
                format!("Thread {thread_id}").bold(),
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
