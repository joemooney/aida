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

// trace:STORY-1728 | ai:claude
#[path = "mailbox_cmd_args.rs"]
pub(crate) mod mailbox_cmd_args;
pub(crate) use mailbox_cmd_args::*;

use crate::cli::MailboxCommand;
use crate::*;

// trace:STORY-1843 | ai:codex
#[path = "mailbox_migration.rs"]
mod mailbox_migration;

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
        // trace:TASK-2347 | ai:codex
        MailboxCommand::WakeSnapshot {
            local_root,
            recipient,
            json: _,
        } => {
            let root = local_root.as_deref().unwrap_or(project_root);
            let snapshot = crate::mailbox_wake::snapshot(root, store_root, recipient)?;
            println!("{}", serde_json::to_string(&snapshot)?);
            Ok(())
        }
        MailboxCommand::Migrate { dry_run, json } => {
            // trace:STORY-1843 | ai:codex
            if !dry_run {
                anyhow::bail!("mailbox migration requires --dry-run; write mode is unavailable");
            }
            let report = mailbox_migration::report(project_root, store_root)?;
            mailbox_migration::print_report(&report, *json || crate::output_format_is_json())
        }
        MailboxCommand::Latency { recipient, json } => {
            let local = mailbox_store::read_local_messages(project_root)?;
            let canonical = mailbox_store::read_canonical_messages(store_root)?;
            let merged = merge_dedup(&local, &canonical);
            // STORY-1843: unread counts come from the verified audience's
            // logical view (only mail with no read evidence is unread). A
            // recipient that is not one of this session's inboxes is an
            // operator inspection over this checkout and the store.
            // trace:STORY-1843 | ai:claude
            let own = crate::mailbox_view::own_audience();
            let (view, identities) = match recipient {
                Some(r) => match own.narrowed_to(r) {
                    Some(narrowed) => {
                        let keys = narrowed.keys().map(str::to_string).collect();
                        (Some(narrowed), keys)
                    }
                    None => (None, vec![r.clone()]),
                },
                None => {
                    let keys = own.keys().map(str::to_string).collect();
                    (Some(own), keys)
                }
            };
            let view = view.map(|audience| {
                crate::mailbox_view::read_own_mail(
                    project_root,
                    store_root,
                    &audience,
                    &crate::mailbox_view::explicit_limits(),
                )
            });
            let now = chrono::Utc::now().timestamp_millis();
            let rows: Vec<serde_json::Value> = identities
                .iter()
                .map(|who| {
                    let (unread, oldest) = match &view {
                        Some(view) => {
                            let unread = view.unread_for(&[who.as_str()]);
                            let oldest = unread.iter().map(|m| m.timestamp).min();
                            (unread.len(), oldest.map(|ts| (now - ts).max(0)))
                        }
                        None => {
                            let watermark = mailbox_store::read_watermark(project_root, who);
                            let latency =
                                aida_core::mailbox::mailbox_latency(who, &merged, watermark, now);
                            (latency.unread, latency.oldest_unread_age_ms)
                        }
                    };
                    let max_read = max_read_latency_ms(project_root, who, &merged);
                    serde_json::json!({
                        "recipient": who,
                        "last_seen_at": mailbox_store::read_last_seen(project_root, who),
                        "unread": unread,
                        "oldest_unread_age_ms": oldest,
                        "max_read_latency_ms": max_read,
                        "partial": view.as_ref().is_some_and(|v| v.is_partial()),
                        "inspection": view.is_none(),
                    })
                })
                .collect();
            if let Some(note) = view.as_ref().and_then(|v| v.partial_note()) {
                eprintln!("{note}");
            }
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
            relayed_from,
            allow_second_person,
        } => {
            // Resolve before any message write; historical registry rows are not readers.
            // trace:STORY-1843 | ai:codex
            let resolution = if *broadcast {
                None
            } else {
                let target = to
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("specify --to <agent> or --broadcast"))?;
                let directory =
                    crate::mailbox_resolver::RecipientDirectory::load(project_root, store_root)?;
                for warning in directory.warnings() {
                    eprintln!("{warning}");
                }
                Some(directory.resolve(target)?)
            };
            let recipient = resolution
                .as_ref()
                .map(|r| Recipient::Agent(r.address.clone()))
                .unwrap_or(Recipient::Broadcast);
            if let Some(note) = resolution.as_ref().and_then(|r| r.reader_notice.as_ref()) {
                eprintln!("{note}");
            }
            // trace:TASK-782 | ai:claude
            let parsed_intent = aida_core::mailbox::Intent::parse(intent).ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid --intent '{intent}'; expected one of: fyi, request, handoff"
                )
            })?;
            // trace:BUG-1533 | ai:claude — resolve authorship with the same
            // precedence session identity uses (agent name / AIDA_USER /
            // role), never the BUG-89 queue-key order, and record which
            // tier won so the envelope never looks silently attributed.
            let (sender, from_source) = crate::resolve_mail_sender_identity(from.as_deref());
            // trace:BUG-1592 | ai:claude
            let from_role = crate::resolve_mail_sender_role();
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
                // Keep the typed resolution failure in the anyhow chain. Besides
                // preserving the not-found/ambiguous distinction for callers and
                // tests, the ambiguous variant tells the operator how to recover.
                // trace:BUG-1465 | ai:codex
                let target = resolve_mailbox_message(&merged, reply_target).with_context(|| {
                    format!("--in-reply-to '{reply_target}' was not resolved; reply refused")
                })?;
                resolved_reply_id = Some(target.id.clone());
                reply_target_thread = Some(target.thread_id.clone());
            }
            // BUG-1534: a second-person body that more than one seat reads
            // credits each reader with the other's claim. Refuse it at the
            // send — a prose rule failed on its own adopter within forty
            // minutes. Checked against this sender's recent mail (local +
            // canonical) so a body sent twice in two commands is caught.
            // trace:BUG-1534 | ai:claude
            // Pronoun check first: a body with no second person never loads
            // the mailbox, and a canonical-store read error never fails a
            // plain send (best-effort, as on the MCP path).
            if !*allow_second_person && !aida_core::mailbox::second_person_terms(&body).is_empty() {
                let recent = if merged.is_empty() {
                    let local = mailbox_store::read_local_messages(project_root)?;
                    let canonical =
                        mailbox_store::read_canonical_messages(store_root).unwrap_or_default();
                    merge_dedup(&local, &canonical)
                } else {
                    merged.clone()
                };
                if let Some(reason) = aida_core::mailbox::second_person_multicast_refusal(
                    &recent,
                    &sender,
                    &recipient,
                    &body,
                    chrono::Utc::now().timestamp_millis(),
                ) {
                    anyhow::bail!(
                        "send refused: {reason}.\n  Name the seat whose claim it is (\"claude-reviewer-1's tally\", not \"your tally\"), \
                         or write one body per recipient.\n  If the reader really is meant as \"you\", pass --allow-second-person."
                    );
                }
            }
            let relayed_from = relayed_from
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(str::to_string);
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
                from_source,
                from_role,
                relayed_from,
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
            // set the notice/hook spans, so reading clears exactly what the
            // notice surfaces. STORY-1843: that set is the audience the core
            // identity policy grants this process, read as one logical view
            // across this repository's worktrees and the canonical store.
            // `--agent` can only narrow it to one of its own inboxes; naming
            // anyone else's inbox is a read-only operator inspection.
            // trace:BUG-555 trace:STORY-1843 | ai:claude
            let own = crate::mailbox_view::own_audience();
            let audience = match agent {
                None => own,
                Some(a) => match own.narrowed_to(a) {
                    Some(narrowed) => narrowed,
                    None => {
                        return inspect_inbox(
                            &merged,
                            a,
                            *archived,
                            *unread,
                            *recent_read_tail,
                            project_root,
                        )
                    }
                },
            };
            let view = crate::mailbox_view::read_own_mail(
                project_root,
                store_root,
                &audience,
                &crate::mailbox_view::explicit_limits(),
            );
            let inbox = select_inbox_rows(view.records(), *archived, *unread, *recent_read_tail);
            let who_label = audience.keys().collect::<Vec<_>>().join(" + ");
            let partial = view.partial_note();
            if inbox.is_empty() {
                let label = if *archived {
                    "no archived mail for"
                } else if *unread {
                    "no unread mail for"
                } else {
                    "inbox empty for"
                };
                match &partial {
                    // A partial view never reads as an empty inbox.
                    Some(note) => println!(
                        "{} {} {} — {}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        "nothing shown for".yellow(),
                        who_label.cyan(),
                        note
                    ),
                    None => println!(
                        "{} {} {}",
                        crate::glyph(crate::glyphs::Glyph::Mailbox).dimmed(),
                        label,
                        who_label.cyan()
                    ),
                }
                return Ok(());
            }
            // Reading marks what it displays seen (STORY-539) — UNLESS
            // `--peek`, which surfaces without consuming (STORY-585 #1/#4).
            //
            // This write happens BEFORE any output below, header included.
            // BUG-1482: established mechanism — `install_sigpipe_handler`
            // (BUG-99) restores SIGPIPE's default disposition at startup so
            // `aida ... | head -N` exits with the classic "downstream closed,
            // terminate quietly" semantics instead of Rust's default
            // ignore-and-panic-on-EPIPE behavior. That means a write in the
            // print loop below can end the process via the SIGPIPE signal
            // itself, mid-loop, with no unwind — so any watermark write
            // placed AFTER printing (the old order) never ran for a reader
            // that closed early (`| head`). Doing the write first means every
            // message this call is about to *display* is already recorded as
            // seen before the first byte of it reaches a pipe that might
            // close. trace:BUG-1482 | ai:claude
            //
            // STORY-1843 B2b: the marks are per-ID receipts for exactly the
            // displayed unread messages (wherever they live); the legacy
            // watermark older readers use advances only to the newest
            // displayed message, never the full inbox's. trace:STORY-1843 | ai:claude
            if !*archived && !*peek {
                crate::mailbox_view::mark_displayed(project_root, view.audience(), &inbox)?;
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
            if let Some(note) = &partial {
                println!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    note.yellow()
                );
            }
            for record in &inbox {
                let tag = match crate::mailbox_view::ReadState::of(record) {
                    crate::mailbox_view::ReadState::ReadLegacy => {
                        Some(crate::mailbox_view::ReadState::ReadLegacy.as_str())
                    }
                    _ => None,
                };
                print_mailbox_line_tagged(&record.message, tag);
            }
            if *peek {
                println!(
                    "{}",
                    "  (peek — not marked seen; `aida mailbox inbox` to read + ack)".dimmed()
                );
            }
            Ok(())
        }
        MailboxCommand::Notice { agent, cap } => {
            // Ambient, non-marking unread summary for an agent's context (the
            // SessionStart / per-turn hook calls this). Plain text, capped,
            // scoped to the session's identity set; silent when caught up.
            // trace:STORY-585 | ai:claude
            // STORY-1843: the same verified audience and logical view as
            // `inbox`; only mail with no read evidence is new (a legacy
            // watermark is not). A partial view says so instead of going
            // quiet. `--agent` narrows to one of this session's own inboxes;
            // any other name is not this session's mail and gets no notice.
            // trace:STORY-1843 | ai:claude
            let own = crate::mailbox_view::own_audience();
            let audience = match agent {
                None => own,
                Some(a) => match own.narrowed_to(a) {
                    Some(narrowed) => narrowed,
                    None => anyhow::bail!(
                        "`{a}` is not one of this session's inboxes ({}); a notice covers only your own mail",
                        own.keys().collect::<Vec<_>>().join(", ")
                    ),
                },
            };
            let view = crate::mailbox_view::read_own_mail(
                project_root,
                store_root,
                &audience,
                &crate::mailbox_view::passive_limits(),
            );
            let identities: Vec<String> = audience.keys().map(str::to_string).collect();
            let cap = cap.unwrap_or(aida_core::mailbox::NOTICE_DEFAULT_CAP);
            print!("{}", notice_text(&view, &identities, cap));
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
        MailboxCommand::Gc {
            older_than,
            agent,
            phantoms,
        } => {
            if *phantoms {
                // trace:TASK-1766 | ai:claude — explicit, reported cleanup.
                let report = archive_phantom_inboxes(store_root)?;
                for line in &report.lines {
                    println!("{line}");
                }
                println!(
                    "Archived {} phantom-recipient message(s); re-addressed {}.",
                    report.archived, report.readdressed
                );
                return Ok(());
            }
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

/// What `aida mailbox gc --phantoms` did: counts plus one line per message.
// trace:TASK-1766 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct PhantomGcReport {
    pub archived: usize,
    pub readdressed: usize,
    pub lines: Vec<String>,
}

/// Archive live mail addressed to a phantom recipient — a junk shape `--to`
/// refuses, a commit SHA, or an `@mention` notice for a handle that is no known
/// person, role or agent. A recipient that is only whitespace-padded around a
/// deliverable id is re-addressed to that id (archive + a copy) rather than
/// lost: a known id under its stored spelling, and direct (non-mention) mail
/// to an unknown id under the trimmed id, since `--to` itself would have sent
/// it there. A known identity is never read as a SHA, so an agent named like
/// `cafe1234` keeps its mail.
// trace:TASK-1766 | ai:antigravity
// trace:TASK-1766 | ai:claude
// trace:TASK-1955 | ai:claude (TASK-1918)
pub(crate) fn archive_phantom_inboxes(store_root: &std::path::Path) -> Result<PhantomGcReport> {
    archive_phantom_inboxes_for(store_root, &crate::inbox_identities())
}

/// [`archive_phantom_inboxes`] judged against `readers`, this machine's inbox
/// identities. Re-addressing a case variant has a defined precedence: a
/// spelling a local reader reads under, or the roster's own spelling, is never
/// moved, and any other variant moves to a local reader's spelling before the
/// roster's first-registered one. So team.toml `Joe` with `USER=joe` keeps
/// `joe` mail readable here, and mail to `Joe` stays where every machine
/// agrees it belongs.
// trace:TASK-1955 | ai:claude (TASK-2122)
pub(crate) fn archive_phantom_inboxes_for(
    store_root: &std::path::Path,
    readers: &[String],
) -> Result<PhantomGcReport> {
    use aida_core::mailbox::{merge_dedup, Message, Recipient};
    use anyhow::Context as _;

    let project_root = store_root.parent().unwrap_or(store_root);
    let local = mailbox_store::read_local_messages(project_root)?;
    let canonical = mailbox_store::read_canonical_messages(store_root)?;
    let merged = merge_dedup(&local, &canonical);
    let known = crate::known_mailbox_recipients(store_root);

    let mut report = PhantomGcReport::default();
    for msg in merged {
        if msg.deleted || msg.archived {
            continue;
        }
        let Recipient::Agent(ref r) = msg.to else {
            continue;
        };
        let trimmed = r.trim();
        let roster_spelling = known.get(&trimmed.to_lowercase()).cloned();
        let known_spelling = roster_spelling.clone().map(|k| {
            readers
                .iter()
                .find(|r| r.eq_ignore_ascii_case(&k))
                .cloned()
                .unwrap_or(k)
        });
        // The roster wins over the SHA shape. trace:TASK-1955 | ai:claude (TASK-1918)
        let junk = trimmed.is_empty()
            || aida_core::mailbox::recipient_id_rejection(trimmed).is_some()
            || (known_spelling.is_none() && aida_core::mailbox::looks_like_hex_sha(trimmed));
        let unknown_mention =
            msg.body.starts_with("You were mentioned on ") && known_spelling.is_none();
        let padded = r != trimmed;
        // Mail stored to a known id in another spelling (`ADVISOR`, as MCP
        // send_message wrote before TASK-1928) reaches no inbox: `addressed_to`
        // is exact. trace:TASK-1955 | ai:claude (TASK-2085)
        // Mail already under the roster spelling is never moved: another
        // machine's reader spelling would only move it back.
        // trace:TASK-1955 | ai:claude (TASK-2122)
        let misspelled = known_spelling.as_deref().is_some_and(|k| k != trimmed)
            && roster_spelling.as_deref() != Some(trimmed)
            && !readers.iter().any(|r| r == trimmed);
        if !(padded || misspelled || junk || unknown_mention) {
            continue;
        }
        let readdress_to =
            (!junk && !unknown_mention).then(|| known_spelling.unwrap_or_else(|| trimmed.into()));
        if let Some(target) = &readdress_to {
            let copy = Message {
                id: format!("{}-readdressed", msg.id),
                to: Recipient::Agent(target.clone()),
                ..msg.clone()
            };
            mailbox_store::write_message(project_root, &copy)?;
            report.readdressed += 1;
            report
                .lines
                .push(format!("re-addressed {} '{r}' -> '{target}'", msg.id));
        } else {
            report.archived += 1;
            report.lines.push(format!("archived {} to '{r}'", msg.id));
        }
        let id = msg.id.clone();
        let marker = Message {
            archived: true,
            ..msg
        };
        mailbox_store::write_message_marker(project_root, &marker)
            .with_context(|| format!("archiving phantom-recipient message {id}"))?;
    }
    Ok(report)
}

// trace:TASK-1211 | ai:codex
fn archive_mailbox_older_than(
    project_root: &std::path::Path,
    store_root: &std::path::Path,
    agent: Option<&str>,
    duration: &str,
) -> Result<()> {
    use aida_core::mailbox::{archive_candidates, inbox_for, merge_dedup, Message};

    // trace:TASK-1509 | ai:claude
    let age = crate::queue_cmd::parse_lookback(duration, "--older-than")?;
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

/// The ambient notice for an own-audience view: the unread summary, plus a
/// line when the view is partial. Empty (a safe hook no-op) only when the
/// view is complete and nothing is unread.
// trace:STORY-1843 | ai:claude
pub(crate) fn notice_text(
    view: &crate::mailbox_view::OwnMail,
    identities: &[String],
    cap: usize,
) -> String {
    let unread: Vec<aida_core::mailbox::Message> = view.unread().into_iter().cloned().collect();
    let summary = aida_core::mailbox::build_notice(
        identities.iter().map(String::as_str),
        &unread,
        &std::collections::HashMap::new(),
        cap,
    );
    let mut out = String::new();
    if !summary.is_empty() {
        out.push_str(&render_mailbox_notice(&summary, identities));
    }
    if let Some(note) = view.partial_note() {
        out.push_str(&format!(
            "📬 {note}. `aida mailbox inbox` shows what could be read.\n"
        ));
    }
    out
}

/// The rows an own-audience inbox shows, in display order: archived (newest
/// first), unread only (oldest first), or the default view — every unread
/// message newest first, then the `recent_read_tail` newest read ones (a
/// legacy-watermark read counts as read here and is labelled).
// trace:STORY-1843 | ai:claude
pub(crate) fn select_inbox_rows(
    records: &[aida_core::mailbox_logical::SnapshotRecord],
    archived: bool,
    unread: bool,
    recent_read_tail: usize,
) -> Vec<&aida_core::mailbox_logical::SnapshotRecord> {
    use crate::mailbox_view::ReadState;
    let newest_first = |rows: &mut Vec<&aida_core::mailbox_logical::SnapshotRecord>| {
        rows.sort_by(|a, b| {
            b.message
                .timestamp
                .cmp(&a.message.timestamp)
                .then_with(|| a.message.id.cmp(&b.message.id))
        })
    };
    if archived {
        let mut rows: Vec<_> = records.iter().filter(|r| r.message.archived).collect();
        newest_first(&mut rows);
        return rows;
    }
    let live = records.iter().filter(|r| !r.message.archived);
    let (mut new, mut read): (Vec<_>, Vec<_>) =
        live.partition(|r| ReadState::of(r) == ReadState::Unread);
    if unread {
        // `records` is already oldest first.
        return new;
    }
    newest_first(&mut new);
    newest_first(&mut read);
    read.truncate(recent_read_tail);
    new.extend(read);
    new
}

/// `aida mailbox inbox --agent <someone else>`: a read-only inspection of an
/// inbox this process does not hold. It reads the source-local layers (this
/// checkout plus the canonical store) the way the pre-STORY-1843 inbox did,
/// never marks anything, and says so.
// trace:STORY-1843 | ai:claude
fn inspect_inbox(
    merged: &[aida_core::mailbox::Message],
    who: &str,
    archived: bool,
    unread: bool,
    recent_read_tail: usize,
    project_root: &std::path::Path,
) -> Result<()> {
    use aida_core::mailbox::{default_inbox_view, inbox_for, Message};
    let wm = mailbox_store::read_watermark(project_root, who).unwrap_or(i64::MIN);
    let mut rows: Vec<&Message> = if archived {
        inbox_for(who, merged)
            .into_iter()
            .filter(|m| m.archived)
            .collect()
    } else if unread {
        inbox_for(who, merged)
            .into_iter()
            .filter(|m| !m.archived && m.timestamp > wm)
            .collect()
    } else {
        default_inbox_view(inbox_for(who, merged), Some(wm), recent_read_tail)
    };
    if archived {
        rows.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| a.id.cmp(&b.id)));
    }
    println!(
        "{} {}",
        format!("Inspecting {who}").bold(),
        format!(
            "({}; not one of your inboxes — read-only, this checkout and the store only, nothing marked)",
            rows.len()
        )
        .dimmed()
    );
    for m in rows {
        print_mailbox_line(m);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command, MailboxCommand};
    use clap::Parser;

    // Send tests need actual recipients now that unknown names and empty roles refuse.
    // trace:STORY-1843 | ai:codex
    fn seed_send_recipients(store: &std::path::Path) {
        let project = store.parent().unwrap();
        std::fs::create_dir_all(store.join("registry")).unwrap();
        let roster = store.join("registry/team.toml");
        let mut body = std::fs::read_to_string(&roster).unwrap_or_else(|_| "[members]\n".into());
        body.push_str("alice = []\nbob = []\n");
        std::fs::write(roster, body).unwrap();
        for (vendor, name, role) in [
            ("codex", "claude-reviewer-1", "advisor"),
            ("claude", "agy-reviewer-task1522", "reviewer"),
            ("antigravity", "antigravity-implementer-1", "implementer"),
        ] {
            crate::agent_registry::register_existing_agent(
                project,
                vendor,
                std::process::id(),
                role.into(),
                None,
                project.to_path_buf(),
                Some(name.into()),
                None,
            )
            .unwrap();
        }
    }

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
            from_source: aida_core::mailbox::SenderSource::Explicit,
            from_role: None,
            relayed_from: None,
        }
    }

    // trace:BUG-1465 | ai:codex
    #[test]
    fn blank_subject_uses_body_in_expanded_mailbox_row() {
        let mut msg = message("abcd1234-3333", "thread-c");
        msg.body = "Body-derived title\nsecond line".into();

        msg.subject = Some(String::new());
        assert_eq!(mailbox_line_body(&msg), msg.body);

        msg.subject = Some("   \t".into());
        assert_eq!(mailbox_line_body(&msg), msg.body);
    }

    // The send surface must preserve the resolver's typed distinction through
    // its added context. This intentionally inspects variants, not prose, so a
    // later wording change cannot collapse ambiguous and missing outcomes.
    // trace:BUG-1465 | ai:codex
    #[test]
    fn reply_target_ambiguity_and_not_found_remain_distinct_typed_outcomes() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        seed_send_recipients(&store);
        mailbox_store::write_message(project.path(), &message("0fa629d6-1111", "thread-a"))
            .unwrap();
        mailbox_store::write_message(project.path(), &message("0fa629d6-2222", "thread-b"))
            .unwrap();

        let send = |reply_target: &str| MailboxCommand::Send {
            to: Some("bob".into()),
            broadcast: false,
            body: Some("reply".into()),
            subject: None,
            body_file: None,
            stdin: false,
            thread: None,
            in_reply_to: Some(reply_target.into()),
            from: Some("alice".into()),
            urgent: false,
            intent: "fyi".into(),
            relayed_from: None,
            allow_second_person: false,
        };

        let ambiguous = handle_mailbox_command(&send("0fa629d6"), &store).unwrap_err();
        assert!(matches!(
            ambiguous.downcast_ref::<crate::MailboxResolveFailure>(),
            Some(crate::MailboxResolveFailure::AmbiguousPrefix { .. })
        ));
        assert!(
            ambiguous.to_string().contains("reply refused"),
            "send context should remain visible: {ambiguous}"
        );
        assert!(
            format!("{ambiguous:#}").contains("lengthen the prefix"),
            "ambiguous cause should provide recovery guidance: {ambiguous:#}"
        );

        let missing = handle_mailbox_command(&send("missing"), &store).unwrap_err();
        assert!(matches!(
            missing.downcast_ref::<crate::MailboxResolveFailure>(),
            Some(crate::MailboxResolveFailure::NotFound { .. })
        ));
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
        let cli = crate::command_registry::try_parse_from(["aida", "mailbox", "read", "0fa629d6"])
            .unwrap();
        match cli.command.registered::<crate::mailbox_cmd::MailboxArgs>() {
            crate::mailbox_cmd::MailboxArgs {
                inner: MailboxCommand::Read { message_id },
            } => {
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

    fn plain_send(to: &str) -> MailboxCommand {
        MailboxCommand::Send {
            to: Some(to.into()),
            broadcast: false,
            body: Some("status update".into()),
            subject: None,
            body_file: None,
            stdin: false,
            thread: None,
            in_reply_to: None,
            from: Some("alice".into()),
            urgent: false,
            intent: "fyi".into(),
            relayed_from: None,
            allow_second_person: false,
        }
    }

    // BUG-2132: the word-split junk ids are refused and no mailbox is opened.
    // trace:BUG-2132 | ai:claude
    #[test]
    fn mailbox_send_refuses_junk_recipient_ids_without_writing() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        for junk in ["-include", "33492", "BUG-52", "EPIC-20", "L4425"] {
            let error = handle_mailbox_command(&plain_send(junk), &store)
                .unwrap_err()
                .to_string();
            assert!(error.contains("send refused"), "{junk}: {error}");
            assert!(error.contains(junk), "{junk}: {error}");
        }
        assert!(mailbox_store::read_local_messages(project.path())
            .unwrap()
            .is_empty());
    }

    // BUG-2132 positive control: agent ids and seat names still send.
    // trace:BUG-2132 | ai:claude
    #[test]
    fn mailbox_send_accepts_agent_ids_and_seat_names() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        seed_send_recipients(&store);
        let ok = [
            "advisor",
            "agy-reviewer-task1522",
            "antigravity-implementer-1",
        ];
        for to in ok {
            handle_mailbox_command(&plain_send(to), &store)
                .unwrap_or_else(|e| panic!("{to} must send: {e}"));
        }
        assert_eq!(
            mailbox_store::read_local_messages(project.path())
                .unwrap()
                .len(),
            ok.len()
        );
    }

    // A foreign namespace makes kill(pid, 0) uninformative, not proof of death.
    // trace:STORY-1843 | ai:codex
    #[test]
    fn mailbox_send_delivers_when_reader_pid_namespace_is_unobservable() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        let mut entry = crate::agent_registry::register_existing_agent(
            project.path(),
            "codex",
            std::process::id(),
            "advisor".into(),
            None,
            project.path().to_path_buf(),
            Some("foreign-reader".into()),
            None,
        )
        .unwrap();
        let namespace = entry
            .pid_namespace
            .expect("Linux exposes the PID namespace");
        entry.pid = u32::MAX; // Cannot be observed by the local PID probe.
        entry.pid_namespace = Some(namespace ^ 1);
        crate::agent_registry::write_entry(project.path(), &entry).unwrap();
        let directory =
            crate::mailbox_resolver::RecipientDirectory::load(project.path(), &store).unwrap();
        for target in ["advisor", "codex", "foreign-reader"] {
            let resolution = directory.resolve(target).unwrap();
            assert!(resolution.reader_notice.unwrap().contains("unknown"));
            handle_mailbox_command(&plain_send(target), &store).unwrap();
        }
        let messages = mailbox_store::read_local_messages(project.path()).unwrap();
        assert_eq!(messages.len(), 3);
        assert!(messages
            .iter()
            .any(|m| m.to == aida_core::mailbox::Recipient::Agent("advisor".into())));
        assert!(messages
            .iter()
            .any(|m| m.to == aida_core::mailbox::Recipient::Agent("foreign-reader".into())));
        entry.ended_at = Some(chrono::Utc::now());
        crate::agent_registry::write_entry(project.path(), &entry).unwrap();
        assert!(handle_mailbox_command(&plain_send("advisor"), &store).is_err());
        assert_eq!(
            mailbox_store::read_local_messages(project.path())
                .unwrap()
                .len(),
            3
        );
    }

    // trace:BUG-1297 | ai:codex
    #[test]
    fn mailbox_reply_to_unknown_message_refuses_without_writing() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        seed_send_recipients(&store);
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
            relayed_from: None,
            allow_second_person: false,
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
        seed_send_recipients(&store);
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
            relayed_from: None,
            allow_second_person: false,
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
        seed_send_recipients(&store);
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
            relayed_from: None,
            allow_second_person: false,
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
        seed_send_recipients(&store);
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
            relayed_from: None,
            allow_second_person: false,
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
        let from_file = crate::command_registry::try_parse_from([
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
        match from_file
            .command
            .registered::<crate::mailbox_cmd::MailboxArgs>()
        {
            crate::mailbox_cmd::MailboxArgs {
                inner:
                    MailboxCommand::Send {
                        body,
                        body_file,
                        subject,
                        ..
                    },
            } => {
                assert!(body.is_none());
                assert_eq!(body_file.unwrap(), std::path::PathBuf::from("mail.txt"));
                assert_eq!(subject.as_deref(), Some("Review requested"));
            }
            other => panic!("expected mailbox send, got {other:?}"),
        }

        let from_stdin = crate::command_registry::try_parse_from([
            "aida",
            "mailbox",
            "send",
            "--broadcast",
            "--stdin",
        ])
        .unwrap();
        match from_stdin
            .command
            .registered::<crate::mailbox_cmd::MailboxArgs>()
        {
            crate::mailbox_cmd::MailboxArgs {
                inner: MailboxCommand::Send { body, stdin, .. },
            } => {
                assert!(body.is_none());
                assert!(stdin);
            }
            other => panic!("expected mailbox send, got {other:?}"),
        }
    }

    // BUG-1482: a downstream reader that closes the pipe early (the reported
    // case was `aida mailbox inbox | head`) must not leave the read-watermark
    // stuck behind messages that were actually DISPLAYED. The established
    // mechanism: `install_sigpipe_handler` (BUG-99) restores SIGPIPE's
    // default disposition at binary startup so a write against a pipe whose
    // reader is gone kills the process via the signal itself — mid-loop, with
    // nothing placed AFTER the print loop ever running.
    //
    // That termination genuinely ends the process, so this cannot be a plain
    // in-process unit test; it re-execs this same test binary as a "worker"
    // that installs the same default SIGPIPE disposition and drives the real
    // `handle_mailbox_command` print path, while this test plays `head`'s
    // role: read exactly one line of the worker's stdout, then drop the pipe.
    // The second message's giant body guarantees the worker is still writing
    // (blocked on the OS pipe buffer, typically 64KiB) when that happens, so
    // it is killed mid-print rather than finishing cleanly — reproducing the
    // reported defect's actual failure shape, not a proxy for it.
    // trace:BUG-1482 | ai:claude
    #[cfg(unix)]
    #[test]
    fn piped_inbox_read_advances_watermark_even_when_stdout_closes_early() {
        if std::env::var("AIDA_BUG_1482_WORKER").is_ok() {
            let project_root = std::path::PathBuf::from(
                std::env::var("AIDA_BUG_1482_PROJECT").expect("worker needs a project dir"),
            );
            // Mirror the real `aida` binary's BUG-99 startup step so the
            // worker dies the same way `aida` does: killed by SIGPIPE, not
            // Rust's default ignore-and-panic-on-EPIPE behavior.
            unsafe {
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            }
            let store = project_root.join(".aida-store");
            let _ = handle_mailbox_command(
                &MailboxCommand::Inbox {
                    agent: Some("bob".into()),
                    all: false,
                    archived: false,
                    peek: false,
                    unread: false,
                    recent_read_tail: 5,
                },
                &store,
            );
            return;
        }

        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();

        let mut small = message("aaaaaaaa-1111", "thread-a");
        small.timestamp = 1;
        mailbox_store::write_message(project.path(), &small).unwrap();
        let mut huge = message("bbbbbbbb-2222", "thread-b");
        huge.timestamp = 2;
        // Newest-first sort means this one prints right after the header —
        // large enough on its own to exceed the OS pipe buffer, so the
        // worker blocks mid-write on this line once the parent stops
        // reading.
        huge.body = "x".repeat(200_000);
        mailbox_store::write_message(project.path(), &huge).unwrap();

        // The process-wide hardened resolver (TASK-1262): in a test process it resolves
        // to this test binary, which is what the re-exec needs. trace:BUG-1482
        let exe = crate::aida_exe_path();
        let mut child = std::process::Command::new(exe)
            // `--exact` matches on the FULLY QUALIFIED test name libtest
            // prints, not the bare function name.
            .arg("mailbox_cmd::tests::piped_inbox_read_advances_watermark_even_when_stdout_closes_early")
            .arg("--exact")
            .arg("--nocapture")
            .env("AIDA_BUG_1482_WORKER", "1")
            .env("AIDA_BUG_1482_PROJECT", project.path())
            // STORY-1843: `--agent bob` must be one of the worker's own
            // inboxes to be marked, so the worker is an unbound `bob` with
            // no inherited session evidence and no project to look one up in.
            .env("AIDA_USER", "bob")
            .env_remove("AIDA_HANDLE")
            .env_remove("AIDA_AGENT_NAME")
            .env_remove("AIDA_SESSION_ROLE")
            .env_remove("AIDA_AGENT_TYPE")
            .env_remove("AIDA_HOOK_PAYLOAD")
            .env_remove("CLAUDE_PID")
            .current_dir(project.path())
            .stdout(std::process::Stdio::piped())
            .spawn_retrying_etxtbsy()
            .expect("spawn worker");

        // Act like `| head`: read lines up through the inbox header (skipping
        // the test harness's own preamble, e.g. the blank line + "running 1
        // test" that `--nocapture` also surfaces), then close our end — the
        // worker's next big write hits the closed pipe.
        {
            let stdout = child.stdout.take().expect("worker stdout");
            let mut reader = std::io::BufReader::new(stdout);
            let mut found = false;
            for _ in 0..20 {
                let mut line = String::new();
                let n = std::io::BufRead::read_line(&mut reader, &mut line)
                    .expect("read worker stdout");
                if n == 0 {
                    break;
                }
                if line.contains("Inbox for") {
                    found = true;
                    break;
                }
            }
            assert!(
                found,
                "never saw the inbox header line in the worker's output"
            );
            // Dropping `reader` here closes the read end.
        }
        let status = child.wait().expect("wait for worker");
        assert!(
            !status.success(),
            "worker should have been killed by the closed pipe, not exited cleanly: {status:?}"
        );
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            status.signal(),
            Some(libc::SIGPIPE),
            "worker should die specifically from SIGPIPE (the BUG-99 mechanism), got: {status:?}"
        );

        let watermark = mailbox_store::read_watermark(project.path(), "bob");
        assert_eq!(
            watermark,
            Some(2),
            "watermark must advance to the newest DISPLAYED message even though \
             the worker died mid-print from the closed pipe"
        );
    }

    // ── BUG-1534: relayed-claim provenance + second-person multicast guard ──

    fn send_to(to: &str, body: &str, relayed_from: Option<&str>, allow: bool) -> MailboxCommand {
        let MailboxCommand::Send {
            broadcast,
            subject,
            body_file,
            stdin,
            thread,
            in_reply_to,
            from,
            urgent,
            intent,
            ..
        } = send_command(body)
        else {
            unreachable!()
        };
        MailboxCommand::Send {
            to: Some(to.into()),
            broadcast,
            body: Some(body.into()),
            subject,
            body_file,
            stdin,
            thread,
            in_reply_to,
            from,
            urgent,
            intent,
            relayed_from: relayed_from.map(str::to_string),
            allow_second_person: allow,
        }
    }

    // trace:BUG-1534 | ai:claude
    #[test]
    fn second_person_body_to_a_second_recipient_is_refused_at_send() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        seed_send_recipients(&store);
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_AGENT_NAME", Some("claude-product-1")),
            ("AIDA_USER", None),
            ("AIDA_SESSION_ROLE", None),
        ]);
        let body = "YOUR TALLY IS RIGHT";
        handle_mailbox_command(&send_to("claude-reviewer-1", body, None, false), &store).unwrap();
        let err = handle_mailbox_command(&send_to("advisor", body, None, false), &store)
            .expect_err("the second recipient of a second-person body must be refused");
        assert!(err.to_string().contains("claude-reviewer-1"), "{err}");
        let sent = mailbox_store::read_local_messages(project.path()).unwrap();
        assert_eq!(sent.iter().filter(|m| m.body == body).count(), 1);

        // The attributed form goes through, and the explicit override works.
        handle_mailbox_command(
            &send_to("advisor", "claude-reviewer-1's tally is right", None, false),
            &store,
        )
        .unwrap();
        handle_mailbox_command(&send_to("advisor", body, None, true), &store).unwrap();
    }

    // trace:BUG-1534 | ai:claude
    #[test]
    fn relayed_from_is_recorded_on_the_sent_message() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        seed_send_recipients(&store);
        // The sender's session handle would outrank AIDA_AGENT_NAME; isolate
        // from the session running the suite. trace:TASK-2345 | ai:claude
        let _env = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_AGENT_NAME", Some("claude-product-1")),
            ("AIDA_USER", None),
            ("AIDA_SESSION_ROLE", None),
        ]);
        handle_mailbox_command(
            &send_to(
                "advisor",
                "the tally is 902",
                Some("claude-reviewer-1"),
                false,
            ),
            &store,
        )
        .unwrap();
        let sent = mailbox_store::read_local_messages(project.path())
            .unwrap()
            .into_iter()
            .find(|m| m.body == "the tally is 902")
            .unwrap();
        assert_eq!(sent.from, "claude-product-1");
        assert_eq!(sent.relayed_from.as_deref(), Some("claude-reviewer-1"));
    }

    // ── BUG-1533: mail sender identity resolution on the send path ─────────

    fn send_command(body: &str) -> MailboxCommand {
        MailboxCommand::Send {
            to: Some("bob".into()),
            broadcast: false,
            body: Some(body.into()),
            subject: None,
            body_file: None,
            stdin: false,
            thread: None,
            in_reply_to: None,
            from: None,
            urgent: false,
            intent: "fyi".into(),
            relayed_from: None,
            allow_second_person: false,
        }
    }

    // trace:BUG-1533 | ai:claude
    #[test]
    fn send_records_agent_name_source_without_touching_aida_user() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        seed_send_recipients(&store);
        // A single EnvVarsGuard::apply for every key: the guard holds the
        // process-global env lock for its whole lifetime, and a second
        // acquisition on the same thread deadlocks (it is not reentrant) —
        // see `test_env::env_lock`.
        let _env = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_AGENT_NAME", Some("claude-product-1")),
            ("AIDA_USER", None),
            ("AIDA_SESSION_ROLE", None),
        ]);

        handle_mailbox_command(&send_command("hello"), &store).unwrap();

        let sent = mailbox_store::read_local_messages(project.path())
            .unwrap()
            .into_iter()
            .find(|m| m.body == "hello")
            .unwrap();
        assert_eq!(sent.from, "claude-product-1");
        assert_eq!(
            sent.from_source,
            aida_core::mailbox::SenderSource::AgentName
        );
        assert!(sent.from_source.is_attributed());
    }

    // trace:BUG-1533 | ai:claude
    #[test]
    fn send_falls_back_to_role_when_no_agent_name_or_aida_user_and_flags_shell_user_fallback() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        seed_send_recipients(&store);
        let role_guard = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_AGENT_NAME", None),
            ("AIDA_USER", None),
            ("AIDA_SESSION_ROLE", Some("advisor")),
        ]);

        handle_mailbox_command(&send_command("role-sourced"), &store).unwrap();
        let sent = mailbox_store::read_local_messages(project.path())
            .unwrap()
            .into_iter()
            .find(|m| m.body == "role-sourced")
            .unwrap();
        assert_eq!(sent.from, "advisor");
        assert_eq!(
            sent.from_source,
            aida_core::mailbox::SenderSource::SessionRole
        );

        // Now drop the role too (releasing the lock first): the send must
        // still succeed (never refuse), but the recorded source must mark it
        // as the ambiguous shell-user fallback rather than looking like a
        // resolved seat.
        drop(role_guard);
        let _no_role = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_AGENT_NAME", None),
            ("AIDA_USER", None),
            ("AIDA_SESSION_ROLE", None),
        ]);
        handle_mailbox_command(&send_command("shell-fallback"), &store).unwrap();
        let fallback = mailbox_store::read_local_messages(project.path())
            .unwrap()
            .into_iter()
            .find(|m| m.body == "shell-fallback")
            .unwrap();
        assert_eq!(
            fallback.from_source,
            aida_core::mailbox::SenderSource::ShellUser,
            "no attributable identity → recorded as the ShellUser fallback marker, not silent"
        );
        assert!(!fallback.from_source.is_attributed());
    }

    // trace:BUG-1533 | ai:claude
    #[test]
    fn two_seats_sharing_shell_user_send_distinguishable_envelopes_end_to_end() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        seed_send_recipients(&store);

        {
            let _env = crate::test_env::AgentSessionIsolation::apply(&[
                ("AIDA_AGENT_NAME", Some("claude-product-1")),
                ("AIDA_USER", None),
                ("AIDA_SESSION_ROLE", None),
            ]);
            handle_mailbox_command(&send_command("from-product"), &store).unwrap();
        }
        {
            let _env = crate::test_env::AgentSessionIsolation::apply(&[
                ("AIDA_AGENT_NAME", Some("claude-reviewer-1")),
                ("AIDA_USER", None),
                ("AIDA_SESSION_ROLE", None),
            ]);
            handle_mailbox_command(&send_command("from-reviewer"), &store).unwrap();
        }

        let all = mailbox_store::read_local_messages(project.path()).unwrap();
        let product = all.iter().find(|m| m.body == "from-product").unwrap();
        let reviewer = all.iter().find(|m| m.body == "from-reviewer").unwrap();
        assert_ne!(
            product.from, reviewer.from,
            "two seats must not collapse to one indistinguishable envelope identity"
        );
    }

    // ── BUG-1592: AIDA_AGENT_NAME must be a read-side inbox identity too ──

    // AC1: a seat whose AIDA_AGENT_NAME differs from AIDA_USER sends mail
    // under AIDA_AGENT_NAME (BUG-1533 precedence) but, before this fix,
    // never read that identity's inbox — replies addressed to its agent
    // name went unread. trace:BUG-1592 | ai:claude
    #[test]
    fn inbox_identities_includes_agent_name_when_distinct_from_aida_user() {
        // trace:TASK-2345 | ai:claude
        let _env = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_AGENT_NAME", Some("claude-impl-7")),
            ("AIDA_USER", Some("alice")),
        ]);
        let ids = inbox_identities();
        assert!(
            ids.iter().any(|i| i == "claude-impl-7"),
            "AIDA_AGENT_NAME must be one of the inbox identities: {ids:?}"
        );
        assert!(
            ids.iter().any(|i| i == "alice"),
            "AIDA_USER (the queue/current-user identity) must still be included: {ids:?}"
        );
    }

    // AC1 end-to-end: mail addressed to the agent name is actually visible
    // (and marked read) through the default `aida mailbox inbox` — not just
    // present in the identity list. trace:BUG-1592 | ai:claude
    #[test]
    fn seat_with_agent_name_distinct_from_aida_user_reads_mail_addressed_to_agent_name() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();

        let mut to_agent_name = message("cccccccc-3333", "thread-c");
        to_agent_name.to = aida_core::mailbox::Recipient::Agent("claude-impl-7".into());
        to_agent_name.timestamp = 42;
        mailbox_store::write_message(project.path(), &to_agent_name).unwrap();

        // trace:TASK-2345 | ai:claude
        let _env = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_AGENT_NAME", Some("claude-impl-7")),
            ("AIDA_USER", Some("alice")),
        ]);
        handle_mailbox_command(
            &MailboxCommand::Inbox {
                agent: None,
                all: false,
                archived: false,
                peek: false,
                unread: false,
                recent_read_tail: 5,
            },
            &store,
        )
        .unwrap();

        assert_eq!(
            mailbox_store::read_watermark(project.path(), "claude-impl-7"),
            Some(42),
            "the default (no --agent) inbox read must union AIDA_AGENT_NAME's inbox \
             and mark its mail seen — before the fix, mail addressed to the agent \
             name was invisible because inbox_identities() never included it"
        );
    }

    // AC2: the envelope records the sender's role alongside the agent id
    // when both are known. trace:BUG-1592 | ai:claude
    #[test]
    fn send_records_role_alongside_agent_name_when_both_are_known() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        seed_send_recipients(&store);
        let _env = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_AGENT_NAME", Some("claude-impl-7")),
            ("AIDA_USER", None),
            ("AIDA_SESSION_ROLE", Some("advisor")),
        ]);

        handle_mailbox_command(&send_command("role-and-agent"), &store).unwrap();

        let sent = mailbox_store::read_local_messages(project.path())
            .unwrap()
            .into_iter()
            .find(|m| m.body == "role-and-agent")
            .unwrap();
        assert_eq!(sent.from, "claude-impl-7");
        assert_eq!(
            sent.from_source,
            aida_core::mailbox::SenderSource::AgentName
        );
        assert_eq!(
            sent.from_role.as_deref(),
            Some("advisor"),
            "role must be recorded alongside the agent id when both are known"
        );
    }

    // AC2 (negative): no role set at send time → `from_role` stays `None`
    // rather than a guessed/forced default. trace:BUG-1592 | ai:claude
    #[test]
    fn send_leaves_from_role_none_when_no_session_role_is_set() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        seed_send_recipients(&store);
        let _env = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_AGENT_NAME", Some("claude-impl-7")),
            ("AIDA_USER", None),
            ("AIDA_SESSION_ROLE", None),
        ]);

        handle_mailbox_command(&send_command("no-role"), &store).unwrap();

        let sent = mailbox_store::read_local_messages(project.path())
            .unwrap()
            .into_iter()
            .find(|m| m.body == "no-role")
            .unwrap();
        assert_eq!(sent.from_role, None);
    }

    /// A store whose team roster lists the human `Joe` (stored spelling).
    // trace:TASK-1766 | ai:claude
    fn store_with_person_joe() -> (tempfile::TempDir, std::path::PathBuf) {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(store.join("registry")).unwrap();
        std::fs::write(
            store.join("registry").join("team.toml"),
            "[members]\nJoe = [\"advisor\"]\n",
        )
        .unwrap();
        (project, store)
    }

    // trace:TASK-1766 | ai:antigravity
    // trace:TASK-1766 | ai:claude
    #[test]
    fn send_trims_recipient_whitespace_and_archive_phantom_inboxes_cleans_all_20_shapes() {
        let (project, store) = store_with_person_joe();
        seed_send_recipients(&store);

        handle_mailbox_command(&plain_send("  advisor  "), &store).unwrap();
        let sent = mailbox_store::read_local_messages(project.path()).unwrap();
        assert_eq!(
            sent[0].to,
            aida_core::mailbox::Recipient::Agent("advisor".into()),
            "recipient whitespace must be trimmed before storing"
        );

        let phantoms = [
            "110",
            "2",
            "4211",
            "AIDA_AGENT_NAME",
            "CODEX_CI_GIT_URL",
            "L4425",
            "PR-219",
            "PR-220",
            "STORY-752",
            "TASK-605",
            "TASK-810",
            "TYPE",
            "YES",
            "--body-file",
            "--from",
            "--intent",
            "--to",
            "6e5727ce3c",
            "advisor ",
            "joe ",
        ];
        for (idx, phantom) in phantoms.iter().enumerate() {
            let mut m = message(&format!("phantom-{idx}"), &format!("thread-{idx}"));
            m.to = aida_core::mailbox::Recipient::Agent((*phantom).to_string());
            m.body = format!("You were mentioned on TASK-1 by joe (@{phantom}):\n\n  hi");
            mailbox_store::write_message(project.path(), &m).unwrap();
        }

        // A padded real recipient is re-addressed to the trimmed, stored id;
        // every other shape is archived.
        let report = archive_phantom_inboxes_for(&store, &[]).unwrap();
        assert_eq!(report.archived, phantoms.len() - 2);
        assert_eq!(report.readdressed, 2);
        assert_eq!(report.lines.len(), phantoms.len(), "{:?}", report.lines);

        let merged = aida_core::mailbox::merge_dedup(
            &mailbox_store::read_local_messages(project.path()).unwrap(),
            &mailbox_store::read_canonical_messages(&store).unwrap(),
        );
        let watermarks = mailbox_store::read_all_watermarks(project.path()).unwrap();
        let summaries = aida_core::mailbox::agent_summaries(&merged, &watermarks);
        let mut agents: Vec<&str> = summaries.iter().map(|s| s.agent.as_str()).collect();
        agents.sort_unstable();
        assert_eq!(
            agents,
            vec!["Joe", "advisor"],
            "phantom recipients are archived; padded real ones are re-addressed"
        );
        assert_eq!(
            aida_core::mailbox::unread_inbox("Joe", &merged, None).len(),
            1,
            "the 'joe ' message must reach Joe's inbox"
        );

        // Idempotent: a second pass finds nothing left to touch.
        let again = archive_phantom_inboxes_for(&store, &[]).unwrap();
        assert_eq!((again.archived, again.readdressed), (0, 0));
    }

    /// TASK-1918: a registered agent whose name is SHA-shaped keeps its
    /// mail; padded direct mail to an unknown id is re-addressed to the
    /// trimmed id, while a padded mention of an unknown handle is archived.
    // trace:TASK-1955 | ai:claude (TASK-1918)
    #[test]
    fn archive_phantom_inboxes_checks_the_roster_before_the_sha_rule_and_reroutes_padded_direct_mail(
    ) {
        let (project, store) = store_with_person_joe();
        let roles = project.path().join(".aida").join("roles");
        std::fs::create_dir_all(&roles).unwrap();
        std::fs::write(
            roles.join("cafe1234.toml"),
            "name = \"cafe1234\"\ncreated_at = \"2026-10-18T00:00:00Z\"\nlast_active_at = \"2026-10-18T00:00:00Z\"\n",
        )
        .unwrap();
        for (id, to, body) in [
            ("sha-direct", "cafe1234", "direct hi"),
            (
                "sha-mention",
                "cafe1234",
                "You were mentioned on TASK-1 by joe: @cafe1234",
            ),
            ("padded-direct", " newagent ", "direct hi"),
            (
                "padded-mention",
                "stranger ",
                "You were mentioned on TASK-1 by joe: @stranger",
            ),
            ("real-sha", "6e5727ce3c", "direct hi"),
        ] {
            let mut m = message(id, &format!("thread-{id}"));
            m.to = aida_core::mailbox::Recipient::Agent(to.into());
            m.body = body.into();
            mailbox_store::write_message(project.path(), &m).unwrap();
        }

        let report = archive_phantom_inboxes_for(&store, &[]).unwrap();
        assert_eq!(
            (report.archived, report.readdressed),
            (2, 1),
            "{:?}",
            report.lines
        );
        let merged = aida_core::mailbox::merge_dedup(
            &mailbox_store::read_local_messages(project.path()).unwrap(),
            &mailbox_store::read_canonical_messages(&store).unwrap(),
        );
        assert_eq!(
            aida_core::mailbox::unread_inbox("cafe1234", &merged, None).len(),
            2,
            "a known SHA-shaped agent keeps its mail"
        );
        assert_eq!(
            aida_core::mailbox::unread_inbox("newagent", &merged, None).len(),
            1,
            "padded direct mail reaches the trimmed id"
        );
        assert!(aida_core::mailbox::unread_inbox("stranger", &merged, None).is_empty());
    }

    /// TASK-2085: unpadded mail to a known id in the wrong case is
    /// re-addressed to the stored spelling, so `ADVISOR` mail reaches advisor.
    // trace:TASK-1955 | ai:claude (TASK-2085)
    #[test]
    fn archive_phantom_inboxes_readdresses_a_case_mismatched_known_recipient() {
        let (project, store) = store_with_person_joe();
        let mut m = message("upper", "thread-upper");
        m.to = aida_core::mailbox::Recipient::Agent("ADVISOR".into());
        m.body = "direct hi".into();
        mailbox_store::write_message(project.path(), &m).unwrap();

        let report = archive_phantom_inboxes_for(&store, &[]).unwrap();
        assert_eq!(
            (report.archived, report.readdressed),
            (0, 1),
            "{:?}",
            report.lines
        );
        let merged = aida_core::mailbox::merge_dedup(
            &mailbox_store::read_local_messages(project.path()).unwrap(),
            &mailbox_store::read_canonical_messages(&store).unwrap(),
        );
        assert_eq!(
            aida_core::mailbox::unread_inbox("advisor", &merged, None).len(),
            1
        );
        // Idempotent: the stored-spelling copy is left alone.
        let again = archive_phantom_inboxes_for(&store, &[]).unwrap();
        assert_eq!((again.archived, again.readdressed), (0, 0));
    }

    /// TASK-2122: a case variant a local reader reads under stays put, and
    /// another variant moves to that reader's spelling, not the roster's, and
    /// mail under the roster spelling itself is never moved.
    // trace:TASK-1955 | ai:claude (TASK-2122)
    #[test]
    fn archive_phantom_inboxes_never_moves_mail_away_from_a_local_reader_spelling() {
        let (project, store) = store_with_person_joe();
        for (id, to) in [("lower", "joe"), ("upper", "JOE"), ("roster", "Joe")] {
            let mut m = message(id, &format!("thread-{id}"));
            m.to = aida_core::mailbox::Recipient::Agent(to.into());
            m.body = "direct hi".into();
            mailbox_store::write_message(project.path(), &m).unwrap();
        }
        let report = archive_phantom_inboxes_for(&store, &["joe".to_string()]).unwrap();
        assert_eq!(
            (report.archived, report.readdressed),
            (0, 1),
            "{:?}",
            report.lines
        );
        assert!(
            report.lines.iter().any(|l| l.contains("'JOE' -> 'joe'")),
            "{:?}",
            report.lines
        );
        assert!(
            !report.lines.iter().any(|l| l.contains("roster")),
            "mail under the roster spelling `Joe` is never moved: {:?}",
            report.lines
        );
        let merged = aida_core::mailbox::merge_dedup(
            &mailbox_store::read_local_messages(project.path()).unwrap(),
            &mailbox_store::read_canonical_messages(&store).unwrap(),
        );
        assert_eq!(
            aida_core::mailbox::unread_inbox("joe", &merged, None).len(),
            2,
            "the local reader `joe` reads its own mail and the re-addressed `JOE` copy"
        );
        assert_eq!(
            aida_core::mailbox::unread_inbox("Joe", &merged, None)
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["roster"],
            "the roster-spelled mail stays under `Joe`, unarchived"
        );
    }

    // F1: a human's existing mention mail is never archived as a phantom.
    // trace:TASK-1766 | ai:claude
    #[test]
    fn archive_phantom_inboxes_keeps_mention_mail_for_roster_people() {
        let (project, store) = store_with_person_joe();
        let mut m = message("mention-joe", "thread-joe");
        m.to = aida_core::mailbox::Recipient::Agent("joe".into());
        m.body = "You were mentioned on TASK-1 by alice: @joe please look".into();
        mailbox_store::write_message(project.path(), &m).unwrap();

        // Never archived as a phantom; since TASK-2085 the lower-case copy is
        // re-addressed to the roster spelling `Joe`, whose inbox reads it.
        // trace:TASK-1955 | ai:claude (TASK-2085)
        let report = archive_phantom_inboxes_for(&store, &[]).unwrap();
        assert_eq!((report.archived, report.readdressed), (0, 1));
        let merged = aida_core::mailbox::merge_dedup(
            &mailbox_store::read_local_messages(project.path()).unwrap(),
            &mailbox_store::read_canonical_messages(&store).unwrap(),
        );
        assert_eq!(
            aida_core::mailbox::unread_inbox("Joe", &merged, None).len(),
            1
        );
    }

    // F2: `--to` delivers a known identity under its stored spelling, the same
    // spelling an `@mention` of it is delivered under.
    // trace:TASK-1766 | ai:claude
    #[test]
    fn send_to_known_identity_uses_stored_spelling() {
        let (project, store) = store_with_person_joe();
        seed_send_recipients(&store);
        handle_mailbox_command(&plain_send("JOE"), &store).unwrap();
        handle_mailbox_command(&plain_send("ADVISOR"), &store).unwrap();
        assert!(handle_mailbox_command(&plain_send("NewAgent"), &store).is_err());
        let mut to: Vec<_> = mailbox_store::read_local_messages(project.path())
            .unwrap()
            .into_iter()
            .map(|m| m.to)
            .collect();
        to.sort_by_key(|r| format!("{r:?}"));
        assert_eq!(
            to,
            vec![
                aida_core::mailbox::Recipient::Agent("Joe".into()),
                aida_core::mailbox::Recipient::Agent("advisor".into()),
            ]
        );
    }
}

/// Inter-agent mailbox: peer↔peer messaging (send / inbox / thread).
// trace:STORY-493 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "mailbox")]
pub(crate) struct MailboxArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::MailboxCommand,
}

fn run_mailbox(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <MailboxArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                MailboxArgs { inner: mailbox_cmd } => {
                    // trace:STORY-493 | ai:claude — local layer only; git-canonical
                    // digest is a later slice. Needs only the project root.
                    return crate::command_registry::returned(self::handle_mailbox_command(
                        mailbox_cmd,
                        store_path,
                    ));
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                MailboxArgs { inner: _ } => {
                    anyhow::bail!(
                    "aida mailbox commands are only available in git-canonical (distributed) mode. \
                     Run `aida init` (defaults to distributed) first."
                );
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static MAILBOX: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "mailbox",
        help_after: "sandbox",
        command: <MailboxArgs as clap::CommandFactory>::command,
        policy: mailbox_cli_policy,
        run: crate::command_registry::Handler::Store(run_mailbox),
    };

// Per-verb policy, carried over from the legacy stakeholder and per-write
// auto-push tables. An unparseable invocation gets the fail-closed answer.
// trace:STORY-1728 | ai:claude
fn mailbox_cli_policy(matches: &clap::ArgMatches) -> crate::command_registry::CmdPolicy {
    use crate::cli::MailboxCommand;
    use crate::stakeholder_role::StakeholderAction as SA;
    let args = <MailboxArgs as clap::FromArgMatches>::from_arg_matches(matches).ok();
    let inner = args.as_ref().map(|args| &args.inner);
    crate::command_registry::CmdPolicy {
        stakeholder: if matches!(
            inner,
            Some(
                MailboxCommand::Inbox { .. }
                    | MailboxCommand::WakeSnapshot { .. }
                    | MailboxCommand::Thread { .. }
                    | MailboxCommand::Migrate { .. }
            )
        ) {
            SA::Read
        } else {
            SA::Write
        },
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    }
}
