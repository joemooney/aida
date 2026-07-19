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
            author,
            parent,
            interactive,
        } => {
            // Use --content flag if provided, otherwise use positional argument
            let effective_content = content.as_ref().or(content_positional.as_ref());
            match effective_content {
                Some(c) if !*interactive => {
                    add_comment_cli(storage, id, c, author.as_deref(), parent.as_deref())?;
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
            interactive,
        } => {
            if *interactive || content.is_none() {
                edit_comment_interactive(storage, req_id, comment_id)?;
            } else {
                edit_comment_cli(storage, req_id, comment_id, content.as_ref().unwrap())?;
            }
        }
        CommentCommand::Delete { req_id, comment_id } => {
            delete_comment(storage, req_id, comment_id)?;
        }
    }
    Ok(())
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

fn add_comment_cli(
    storage: &Storage,
    req_id: &str,
    content: &str,
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
