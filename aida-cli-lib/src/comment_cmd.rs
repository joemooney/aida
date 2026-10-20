//! `aida comment` command cluster (add / list / edit / delete on a spec).
//!
//! Comment authoring and rendering against a requirement's comment thread:
//! interactive + `--content` add, list, interactive + CLI edit, and delete.
//! Extracted verbatim from `main.rs` (SPIKE-78); no behavior change. Shared
//! helpers (`parse_requirement_id`, `get_default_author`,
//! `resolve_current_session_id`, `print_comment`) stay in `main.rs` and are
//! reached via `crate::`; the `Comment` model / store methods live in
//! `aida-core`.

use anyhow::{Context, Result};
use colored::Colorize;
use uuid::Uuid;

// trace:STORY-1728 | ai:claude
#[path = "comment_cmd_args.rs"]
pub(crate) mod comment_cmd_args;
pub(crate) use comment_cmd_args::*;

use aida_core::Comment;
use aida_core::Storage;

use crate::cli::CommentCommand;
use crate::{get_default_author, parse_requirement_id, print_comment, resolve_current_session_id};

pub(crate) fn handle_comment_command(cmd: &CommentCommand, storage: &Storage) -> Result<()> {
    match cmd {
        CommentCommand::Add {
            id,
            content,
            content_positional,
            body_file,
            stdin,
            author,
            parent,
            interactive,
            relayed_from,
        } => {
            // Use --content flag if provided, otherwise use positional argument
            let effective_content = resolve_body(
                content.clone().or_else(|| content_positional.clone()),
                body_file.as_deref(),
                *stdin,
            )?;
            match effective_content {
                Some(c) if !*interactive => {
                    // trace:BUG-1534 | ai:claude
                    add_comment_cli_relayed(
                        storage,
                        id,
                        &c,
                        author.as_deref(),
                        parent.as_deref(),
                        relayed_from.as_deref(),
                    )?;
                }
                _ => {
                    add_comment_interactive(storage, id, author.as_deref(), parent.as_deref())?;
                }
            }
        }
        CommentCommand::List { id } => {
            list_comments(storage, id)?;
        }
        CommentCommand::Edit {
            req_id,
            comment_id,
            content,
            body_file,
            stdin,
            interactive,
        } => {
            let replacement = resolve_body(content.clone(), body_file.as_deref(), *stdin)?;
            if *interactive || replacement.is_none() {
                edit_comment_interactive(storage, req_id, comment_id)?;
            } else {
                edit_comment_cli(storage, req_id, comment_id, replacement.as_ref().unwrap())?;
            }
        }
        CommentCommand::Delete { req_id, comment_id } => {
            delete_comment(storage, req_id, comment_id)?;
        }
    }
    Ok(())
}

// trace:TASK-190 | ai:codex
fn resolve_body(
    content: Option<String>,
    body_file: Option<&std::path::Path>,
    stdin: bool,
) -> Result<Option<String>> {
    resolve_body_with_reader(content, body_file, stdin, &mut std::io::stdin())
}

fn resolve_body_with_reader(
    content: Option<String>,
    body_file: Option<&std::path::Path>,
    stdin: bool,
    reader: &mut impl std::io::Read,
) -> Result<Option<String>> {
    if let Some(path) = body_file {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read comment body from {}", path.display()))?;
        anyhow::ensure!(!body.trim().is_empty(), "comment body file is empty");
        return Ok(Some(body));
    }
    if stdin {
        let mut body = String::new();
        reader
            .read_to_string(&mut body)
            .context("failed to read comment body from stdin")?;
        anyhow::ensure!(!body.trim().is_empty(), "comment body from stdin is empty");
        return Ok(Some(body));
    }
    Ok(content)
}

/// Append a marker-bearing comment to a spec, or — when one carrying the same
/// signature is already there — bump that one's recurrence counter in place.
///
/// The quiet counterpart of [`add_comment_cli`]: it prints nothing, because its
/// caller is the drain, whose stdout may be a `--json` document. Returns the
/// comment's id either way, so the caller can point at it.
// trace:TASK-1564 | ai:claude
pub(crate) fn bump_or_add_marked_comment(
    storage: &Storage,
    req_id: &str,
    signature: &str,
    body: &str,
    author: &str,
    latest_at: &str,
) -> Result<Uuid> {
    let mut store = storage.load()?;
    let id = parse_requirement_id(req_id, &store)?;
    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == id)
        .context("Requirement not found")?;

    let existing = req
        .comments
        .iter_mut()
        .find(|c| crate::drain_failure_note::matches_signature(&c.content, signature));
    let comment_id = match existing {
        Some(note) => {
            note.content = crate::drain_failure_note::increment_auto_failure_attempts(
                &note.content,
                latest_at,
            );
            note.touch();
            note.id
        }
        None => {
            let comment = Comment::new(author.to_string(), body.to_string())
                .with_session_id(resolve_current_session_id());
            let comment_id = comment.id;
            req.add_comment(comment);
            comment_id
        }
    };

    storage.save(&store)?;
    Ok(comment_id)
}

#[cfg(test)]
mod task_190_tests {
    use super::{resolve_body, resolve_body_with_reader};

    #[test]
    fn body_file_preserves_shell_metacharacters_literally() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("comment.md");
        std::fs::write(&path, "keep `date` and $(pwd) literal\n").unwrap();
        assert_eq!(
            resolve_body(None, Some(&path), false).unwrap().unwrap(),
            "keep `date` and $(pwd) literal\n"
        );
    }

    #[test]
    fn empty_body_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.md");
        std::fs::write(&path, " \n").unwrap();
        assert!(resolve_body(None, Some(&path), false)
            .unwrap_err()
            .to_string()
            .contains("empty"));
    }

    #[test]
    fn empty_stdin_is_rejected() {
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        assert!(resolve_body_with_reader(None, None, true, &mut input)
            .unwrap_err()
            .to_string()
            .contains("empty"));
    }
}

fn add_comment_interactive(
    storage: &Storage,
    req_id: &str,
    author: Option<&str>,
    parent_id: Option<&str>,
) -> Result<()> {
    let mut store = storage.load()?;
    let id = parse_requirement_id(req_id, &store)?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == id)
        .context("Requirement not found")?;

    let author = if let Some(a) = author {
        a.to_string()
    } else {
        let default_author = get_default_author();
        inquire::Text::new("Author:")
            .with_default(&default_author)
            .prompt()?
    };

    let content = inquire::Editor::new("Comment content:").prompt()?;

    // trace:TASK-330 | ai:claude — stamp the session that produced this comment
    let session_id = resolve_current_session_id();
    let comment = if let Some(parent_str) = parent_id {
        let parent_uuid = Uuid::parse_str(parent_str).context("Invalid parent comment ID")?;
        Comment::new_reply(author, content, parent_uuid).with_session_id(session_id)
    } else {
        Comment::new(author, content).with_session_id(session_id)
    };

    if let Some(parent_str) = parent_id {
        let parent_uuid = Uuid::parse_str(parent_str)?;
        req.add_reply(parent_uuid, comment)?;
    } else {
        req.add_comment(comment);
    }

    storage.save(&store)?;
    println!("{}", "Comment added successfully".green());
    Ok(())
}

pub(crate) fn add_comment_cli(
    storage: &Storage,
    req_id: &str,
    content: &str,
    author: Option<&str>,
    parent_id: Option<&str>,
) -> Result<()> {
    add_comment_cli_relayed(storage, req_id, content, author, parent_id, None)
}

/// [`add_comment_cli`] plus the seat whose claim the comment relays.
// trace:BUG-1534 | ai:claude
pub(crate) fn add_comment_cli_relayed(
    storage: &Storage,
    req_id: &str,
    content: &str,
    author: Option<&str>,
    parent_id: Option<&str>,
    relayed_from: Option<&str>,
) -> Result<()> {
    let mut store = storage.load()?;
    let id = parse_requirement_id(req_id, &store)?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == id)
        .context("Requirement not found")?;

    let author = author
        .map(|a| a.to_string())
        .unwrap_or_else(get_default_author);

    // trace:TASK-330 | ai:claude — stamp the session that produced this comment
    let session_id = resolve_current_session_id();
    let comment = if let Some(parent_str) = parent_id {
        let parent_uuid = Uuid::parse_str(parent_str).context("Invalid parent comment ID")?;
        Comment::new_reply(author, content.to_string(), parent_uuid).with_session_id(session_id)
    } else {
        Comment::new(author, content.to_string()).with_session_id(session_id)
    }
    .with_relayed_from(relayed_from);

    if let Some(parent_str) = parent_id {
        let parent_uuid = Uuid::parse_str(parent_str)?;
        req.add_reply(parent_uuid, comment)?;
    } else {
        req.add_comment(comment);
    }

    storage.save(&store)?;
    println!("{}", "Comment added successfully".green());
    Ok(())
}

fn list_comments(storage: &Storage, req_id: &str) -> Result<()> {
    let store = storage.load()?;
    let id = parse_requirement_id(req_id, &store)?;

    let req = store
        .requirements
        .iter()
        .find(|r| r.id == id)
        .context("Requirement not found")?;

    println!("{}: {}", "Requirement".cyan(), req.title);
    println!();

    if req.comments.is_empty() {
        println!("{}", "No comments yet".dimmed());
        return Ok(());
    }

    println!("{}:", "Comments".green().bold());
    for comment in &req.comments {
        print_comment(comment, 0);
    }

    Ok(())
}

fn edit_comment_interactive(storage: &Storage, req_id: &str, comment_id: &str) -> Result<()> {
    let mut store = storage.load()?;
    let req_uuid = parse_requirement_id(req_id, &store)?;
    let comment_uuid = Uuid::parse_str(comment_id).context("Invalid comment ID")?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == req_uuid)
        .context("Requirement not found")?;

    let comment = req
        .find_comment_mut(&comment_uuid)
        .context("Comment not found")?;

    let new_content = inquire::Editor::new("Comment content:")
        .with_predefined_text(&comment.content)
        .prompt()?;

    comment.content = new_content;
    comment.touch();

    storage.save(&store)?;
    println!("{}", "Comment updated successfully".green());
    Ok(())
}

fn edit_comment_cli(
    storage: &Storage,
    req_id: &str,
    comment_id: &str,
    content: &str,
) -> Result<()> {
    let mut store = storage.load()?;
    let req_uuid = parse_requirement_id(req_id, &store)?;
    let comment_uuid = Uuid::parse_str(comment_id).context("Invalid comment ID")?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == req_uuid)
        .context("Requirement not found")?;

    let comment = req
        .find_comment_mut(&comment_uuid)
        .context("Comment not found")?;

    comment.content = content.to_string();
    comment.touch();

    storage.save(&store)?;
    println!("{}", "Comment updated successfully".green());
    Ok(())
}

fn delete_comment(storage: &Storage, req_id: &str, comment_id: &str) -> Result<()> {
    let mut store = storage.load()?;
    let req_uuid = parse_requirement_id(req_id, &store)?;
    let comment_uuid = Uuid::parse_str(comment_id).context("Invalid comment ID")?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == req_uuid)
        .context("Requirement not found")?;

    req.delete_comment(&comment_uuid)?;

    storage.save(&store)?;
    println!("{}", "Comment deleted successfully".green());
    Ok(())
}

/// Manage comments on requirements
#[derive(clap::Parser, Debug)]
#[clap(name = "comment")]
pub(crate) struct CommentArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::CommentCommand,
}

fn run_comment(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <CommentArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
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
                CommentArgs {
                    inner:
                        CommentCommand::Add {
                            id: req_id,
                            content,
                            content_positional,
                            body_file,
                            stdin,
                            author,
                            relayed_from,
                            ..
                        },
                } => {
                    // BUG-68: lookup first, record activity after the body
                    // sanity check below also passes. trace:BUG-68 | ai:claude
                    // BUG-1535: refuse an ambiguous id. trace:BUG-1535 | ai:claude
                    let mut req =
                        backend
                            .get_requirement_unambiguous(req_id)?
                            .ok_or_else(|| {
                                not_found::requirement_not_found(req_id, Some(store_path))
                            })?;

                    // `aida comment add <REQ> "text"` — the text comes through as
                    // `content_positional`. Earlier the git-backend dispatch only
                    // looked at `--content`, so positional invocations silently
                    // wrote empty comments. trace:BUG-28 | ai:claude
                    let body = resolve_comment_body(
                        content.clone().or_else(|| content_positional.clone()),
                        body_file.as_deref(),
                        *stdin,
                    )?;
                    if body.trim().is_empty() {
                        anyhow::bail!(
                        "comment body required: pass it positionally `aida comment add {} \"...\"` \
                         or via `--content`",
                        req_id
                    );
                    }

                    let now = chrono::Utc::now();
                    let comment_author = author.clone().unwrap_or_else(get_default_author);
                    let mention_body = body.clone();
                    let comment = aida_core::Comment {
                        id: Uuid::now_v7(),
                        content: body,
                        author: comment_author.clone(),
                        created_at: now,
                        modified_at: now,
                        parent_id: None,
                        replies: Vec::new(),
                        reactions: Vec::new(),
                        // trace:TASK-330 | ai:claude — stamp the producing session
                        session_id: resolve_current_session_id(),
                        harness_version: None,
                        relayed_from: None,
                    }
                    // trace:BUG-1534 | ai:claude — record whose claim this relays.
                    .with_relayed_from(relayed_from.as_deref());

                    req.comments.push(comment);
                    req.modified_at = now;
                    backend.update_requirement(&req)?;
                    // BUG-68: record after successful write, with the canonical
                    // spec_id from the resolved req. trace:BUG-68 | ai:claude
                    record_role_activity(req.spec_id.as_deref().unwrap_or(req_id), "comment");
                    println!("Comment added to {}", req_id);

                    // STORY-644 / TASK-1766: notify each @mentioned user via the mailbox
                    // only when the handle resolves to a known person, role or registered agent.
                    // Unknown handles (including unrecognized all-uppercase handles)
                    // emit a warning and are never delivered.
                    // trace:STORY-644 | ai:claude
                    // trace:TASK-1766 | ai:antigravity
                    {
                        let (mentions, warnings) = crate::punt_done::resolve_comment_mentions(
                            store_path,
                            &comment_author,
                            &mention_body,
                        );
                        for warning in &warnings {
                            eprintln!(
                                "  {} {}",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                warning
                            );
                        }
                        if !mentions.is_empty() {
                            let display = req.spec_id.as_deref().unwrap_or(req_id);
                            let snippet = mention_snippet(&mention_body, 80);
                            for handle in &mentions {
                                send_notification(
                                    store_path,
                                    &comment_author,
                                    handle,
                                    format!(
                                        "You were mentioned on {display} by {comment_author}: {snippet}"
                                    ),
                                );
                            }
                        }
                    }
                }
                CommentArgs {
                    inner: CommentCommand::List { id },
                } => {
                    // trace:TASK-1-020 | ai:claude
                    // BUG-68: record after successful lookup. trace:BUG-68 | ai:claude
                    let req = backend
                        .get_requirement_unambiguous_for_read(id)? // read-only; trace:BUG-1670 | ai:claude
                        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
                    record_role_activity(req.spec_id.as_deref().unwrap_or(id), "show");
                    println!("{}: {}", "Requirement".cyan(), req.title);
                    println!();
                    if req.comments.is_empty() {
                        println!("{}", "No comments yet".dimmed());
                    } else {
                        println!("{}:", "Comments".green().bold());
                        for c in &req.comments {
                            print_comment(c, 0);
                        }
                    }
                }
                // SPIKE-2: edit/delete an existing comment on a git-backed store.
                // Uses prefix-friendly resolution so users can pass the leading 8
                // chars of the UUID instead of the full 36-char form.
                // trace:SPIKE-2 | ai:claude
                CommentArgs {
                    inner:
                        CommentCommand::Edit {
                            req_id,
                            comment_id,
                            content,
                            body_file,
                            stdin,
                            interactive,
                        },
                } => {
                    // BUG-68: record after successful lookup. trace:BUG-68 | ai:claude
                    let mut req =
                        backend
                            .get_requirement_unambiguous(req_id)?
                            .ok_or_else(|| {
                                not_found::requirement_not_found(req_id, Some(store_path))
                            })?;
                    record_role_activity(req.spec_id.as_deref().unwrap_or(req_id), "comment");
                    let comment_uuid = resolve_comment_uuid(&req, comment_id)?;

                    let new_content =
                        if *interactive || (content.is_none() && body_file.is_none() && !stdin) {
                            let existing = req
                                .find_comment_mut(&comment_uuid)
                                .map(|c| c.content.clone())
                                .unwrap_or_default();
                            inquire::Editor::new("Edit comment")
                                .with_predefined_text(&existing)
                                .prompt()
                                .context("Editor cancelled")?
                        } else {
                            resolve_comment_body(content.clone(), body_file.as_deref(), *stdin)?
                        };

                    let comment = req
                        .find_comment_mut(&comment_uuid)
                        .context("Comment not found (vanished between resolve and edit?)")?;
                    comment.content = new_content;
                    comment.touch();
                    req.modified_at = chrono::Utc::now();
                    backend.update_requirement(&req)?;
                    println!(
                        "{} comment {} on {}",
                        "Updated".green(),
                        comment_uuid,
                        req_id
                    );
                }
                CommentArgs {
                    inner: CommentCommand::Delete { req_id, comment_id },
                } => {
                    // BUG-68: record after successful lookup. trace:BUG-68 | ai:claude
                    let mut req =
                        backend
                            .get_requirement_unambiguous(req_id)?
                            .ok_or_else(|| {
                                not_found::requirement_not_found(req_id, Some(store_path))
                            })?;
                    record_role_activity(req.spec_id.as_deref().unwrap_or(req_id), "comment");
                    let comment_uuid = resolve_comment_uuid(&req, comment_id)?;
                    req.delete_comment(&comment_uuid)?;
                    backend.update_requirement(&req)?;
                    println!(
                        "{} comment {} from {}",
                        "Deleted".green(),
                        comment_uuid,
                        req_id
                    );
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
                CommentArgs { inner: comment_cmd } => {
                    self::handle_comment_command(comment_cmd, &storage)?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static COMMENT: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "comment",
        help_after: "rel-def",
        command: <CommentArgs as clap::CommandFactory>::command,
        policy: comment_cli_policy,
        run: crate::command_registry::Handler::Store(run_comment),
    };

// Per-verb policy, carried over from the legacy stakeholder and per-write
// auto-push tables. An unparseable invocation gets the fail-closed answer.
// trace:STORY-1728 | ai:claude
fn comment_cli_policy(matches: &clap::ArgMatches) -> crate::command_registry::CmdPolicy {
    use crate::cli::CommentCommand;
    use crate::stakeholder_role::StakeholderAction as SA;
    let args = <CommentArgs as clap::FromArgMatches>::from_arg_matches(matches).ok();
    let inner = args.as_ref().map(|args| &args.inner);
    crate::command_registry::CmdPolicy {
        stakeholder: if matches!(inner, Some(CommentCommand::List { .. })) {
            SA::Read
        } else {
            SA::Comment
        },
        per_write_auto_push: true,
        skip_role_restore_prompt: false,
        team_identity_write: true,
    }
}
