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
