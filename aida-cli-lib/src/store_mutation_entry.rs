//! Early dispatch for the still-gated mediated profile. Ordinary handlers,
//! aliases, extensions, cache workers and telemetry never run in this path.
// trace:TASK-3223 | ai:codex
use crate::store_mutation_client::Client;
use aida_core::exec_context::Environment;
use anyhow::{ensure, Context, Result};
use clap::parser::ValueSource;
use serde_json::{json, Value};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub(crate) fn entry(env: &Environment, argv: &[String], cwd: Option<&Path>) -> Option<Result<()>> {
    match Client::from_environment(env) {
        Ok(None) => None,
        Err(error) => Some(Err(error)),
        Ok(Some(client)) => Some(
            cwd.context("mediated store cwd unavailable")
                .and_then(|cwd| run(&client, env, argv, cwd)),
        ),
    }
}

fn explicit_fields(
    matches: &clap::ArgMatches,
    allowed: &[&str],
    command: &clap::Command,
) -> Result<()> {
    // ArgMatches::ids also contains automatically populated argument groups.
    // Check the declared arguments, never mistake a group for another option.
    for arg in command.get_arguments() {
        let id = arg.get_id().as_str();
        ensure!(
            matches.value_source(id) != Some(ValueSource::CommandLine) || allowed.contains(&id),
            "UnsupportedOperation: argument {id} has no typed route"
        );
    }
    Ok(())
}

fn root(matches: &clap::ArgMatches, cwd: &Path) -> Result<PathBuf> {
    let root = match matches.get_one::<String>("file") {
        Some(path) => cwd.join(path),
        None => cwd
            .ancestors()
            .map(|p| p.join(".aida-store"))
            .find(|p| p.join("objects").is_dir())
            .context("mediated operations require an existing canonical store")?,
    };
    ensure!(
        root.join("objects").is_dir(),
        "mediated operations require an existing canonical store"
    );
    Ok(root)
}

fn run(client: &Client, env: &Environment, argv: &[String], cwd: &Path) -> Result<()> {
    let mut command = crate::command_registry::built_command();
    let matches = match command.try_get_matches_from_mut(argv) {
        Ok(matches) => matches,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error.print()?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    // No alternate project/server, recording, or other early side-effect flags.
    explicit_fields(
        &matches,
        &["file", "format", "verbose", "canonical_ids"],
        &command,
    )?;
    let (name, args) = matches.subcommand().context("UnsupportedOperation")?;
    if name == "mcp-serve" {
        return crate::mcp::run_mediated_mcp_server(client, env, &root(&matches, cwd)?);
    }
    let (tool, payload) = translate(name, args, cwd)?;
    let action = if matches!(tool, "add_comment" | "edit_comment" | "delete_comment") {
        crate::StakeholderAction::Comment
    } else {
        crate::StakeholderAction::EditRequirement
    };
    if let Ok(role) = env.var("AIDA_SESSION_ROLE") {
        ensure!(
            crate::stakeholder_action_allowed(&crate::canonical_role_name(role.trim()), action),
            "UnsupportedOperation"
        );
    }
    let store = root(&matches, cwd)?;
    let result = if matches!(tool, "edit_comment" | "delete_comment") {
        let content = if tool == "edit_comment" {
            Some(payload["text"].as_str().context("InvalidRequest")?)
        } else {
            None
        };
        client.change_comment(
            &store,
            payload["id"].as_str().context("InvalidRequest")?,
            payload["comment_id"].as_str().context("InvalidRequest")?,
            content,
        )?
    } else {
        client.mcp(&store, tool, &payload)?
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

/// The lane sends text, never a pathname, to the host. Open nonblocking first
/// so a FIFO cannot hang early dispatch; bound the read even if a regular file
/// grows after metadata inspection. No editor or ordinary backend is started.
// trace:TASK-3223 | ai:codex
fn comment_file(path: &Path) -> Result<String> {
    const LIMIT: u64 = 16384;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .context("cannot open mediated comment body file")?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= LIMIT,
        "InvalidRequest: comment body must be a regular file of at most 16384 bytes"
    );
    let mut text = String::new();
    file.take(LIMIT + 1).read_to_string(&mut text)?;
    ensure!(
        text.len() <= LIMIT as usize && !text.trim().is_empty(),
        "InvalidRequest: comment body is empty or too large"
    );
    Ok(text)
}

fn translate(name: &str, args: &clap::ArgMatches, cwd: &Path) -> Result<(&'static str, Value)> {
    let command = crate::command_registry::built_command();
    let command = command
        .find_subcommand(name)
        .context("UnsupportedOperation")?;
    match name {
        "comment" => {
            let (name, args) = args.subcommand().context("UnsupportedOperation")?;
            if matches!(name, "edit" | "delete") {
                explicit_fields(
                    args,
                    &["req_id", "comment_id", "content", "body_file"],
                    command
                        .find_subcommand(name)
                        .context("UnsupportedOperation")?,
                )?;
                let id = args.get_one::<String>("req_id").context("InvalidRequest")?;
                let comment = args
                    .get_one::<String>("comment_id")
                    .context("InvalidRequest")?;
                let mut payload = json!({"id":id,"comment_id":comment});
                if name == "delete" {
                    return Ok(("delete_comment", payload));
                }
                let text = match args.get_one::<PathBuf>("body_file") {
                    Some(path) => comment_file(&cwd.join(path))?,
                    None => args
                        .get_one::<String>("content")
                        .context("UnsupportedOperation: provide --content or --body-file")?
                        .clone(),
                };
                payload["text"] = json!(text);
                return Ok(("edit_comment", payload));
            }
            ensure!(name == "add", "UnsupportedOperation");
            explicit_fields(
                args,
                &["id", "content", "content_positional", "body_file"],
                command
                    .find_subcommand("add")
                    .context("UnsupportedOperation")?,
            )?;
            let id = args.get_one::<String>("id").context("InvalidRequest")?;
            let text = match args.get_one::<PathBuf>("body_file") {
                Some(path) => comment_file(&cwd.join(path))?,
                None => args
                    .get_one::<String>("content")
                    .or_else(|| args.get_one::<String>("content_positional"))
                    .context("UnsupportedOperation: provide inline comment content or --body-file")?
                    .clone(),
            };
            Ok(("add_comment", json!({"id":id,"text":text})))
        }
        "edit" => {
            explicit_fields(args, &["id", "title", "description"], command)?;
            let id = args.get_one::<String>("id").context("InvalidRequest")?;
            let mut payload = json!({"id":id});
            for field in ["title", "description"] {
                if let Some(value) = args.get_one::<String>(field) {
                    payload[field] = json!(value);
                }
            }
            ensure!(
                payload.as_object().unwrap().len() > 1,
                "UnsupportedOperation"
            );
            Ok(("update_requirement", payload))
        }
        _ => anyhow::bail!("UnsupportedOperation: this command has no mediated route"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_profile_never_returns_to_ordinary_dispatch() {
        let environment = Environment::from_pairs([
            ("AIDA_STORE_MUTATION_PROFILE".into(), "mediated".into()),
            (
                "AIDA_STORE_MUTATION_ENDPOINT".into(),
                format!("/run/aida-mutation/{}/service", uuid::Uuid::new_v4()).into(),
            ),
        ]);
        let argv = vec![
            "aida".into(),
            "queue".into(),
            "done".into(),
            "TASK-1".into(),
        ];
        assert!(entry(&environment, &argv, Some(Path::new("/missing")))
            .unwrap()
            .is_err());
        assert!(entry(&environment, &argv, None).unwrap().is_err());
        assert!(entry(&Environment::default(), &argv, None).is_none());
    }

    #[test]
    fn closed_cli_translation_preserves_content_and_rejects_semantic_downgrades() {
        for (argv, tool, payload) in [
            (
                vec!["aida", "comment", "add", "TASK-1", "text"],
                "add_comment",
                json!({"id":"TASK-1","text":"text"}),
            ),
            (
                vec!["aida", "edit", "TASK-1", "--description", ""],
                "update_requirement",
                json!({"id":"TASK-1","description":""}),
            ),
        ] {
            let matches = crate::command_registry::built_command()
                .try_get_matches_from(argv)
                .unwrap();
            let (name, args) = matches.subcommand().unwrap();
            assert_eq!(
                translate(name, args, Path::new("/missing")).unwrap(),
                (tool, payload)
            );
        }
        for argv in [
            vec![
                "aida", "comment", "add", "TASK-1", "text", "--author", "spoof",
            ],
            vec![
                "aida",
                "comment",
                "add",
                "TASK-1",
                "--body-file",
                "/unreadable",
            ],
            vec![
                "aida",
                "edit",
                "TASK-1",
                "--title",
                "ok",
                "--status",
                "completed",
            ],
            vec!["aida", "queue", "done", "TASK-1"],
        ] {
            let matches = crate::command_registry::built_command()
                .try_get_matches_from(argv)
                .unwrap();
            let (name, args) = matches.subcommand().unwrap();
            assert!(translate(name, args, Path::new("/missing")).is_err());
        }
    }

    #[test]
    fn body_file_routes_literal_text_and_refuses_special_or_unbounded_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("body");
        let text = "literal `command` $(command)\n\tsecond line";
        std::fs::write(&path, text).unwrap();
        let matches = crate::command_registry::built_command()
            .try_get_matches_from(["aida", "comment", "add", "TASK-1", "--body-file", "body"])
            .unwrap();
        let (name, args) = matches.subcommand().unwrap();
        assert_eq!(
            translate(name, args, tmp.path()).unwrap(),
            ("add_comment", json!({"id":"TASK-1", "text":text}))
        );
        for bytes in [vec![b'x'; 16385], vec![b' '; 20], vec![0xff]] {
            std::fs::write(&path, bytes).unwrap();
            assert!(translate(name, args, tmp.path()).is_err());
        }
        std::fs::write(&path, vec![b'x'; 16384]).unwrap();
        assert_eq!(comment_file(&path).unwrap().len(), 16384);
        assert!(comment_file(tmp.path()).is_err());
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(comment_file(&link).is_err());
        let fifo = tmp.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(comment_file(&fifo).is_err());
    }

    #[test]
    fn existing_comment_translation_never_drops_interactive_or_stdin_semantics() {
        let id = uuid::Uuid::new_v4().to_string();
        for (verb, extra, expected) in [
            (
                "edit",
                vec!["--content", "replacement"],
                Some("edit_comment"),
            ),
            ("delete", vec![], Some("delete_comment")),
            ("edit", vec!["--stdin"], None),
            ("edit", vec!["--interactive"], None),
            ("edit", vec![], None),
        ] {
            let mut argv = vec![
                "aida",
                "comment",
                verb,
                "--req-id",
                "TASK-1",
                "--comment-id",
                &id,
            ];
            argv.extend(extra);
            let matches = crate::command_registry::built_command()
                .try_get_matches_from(argv)
                .unwrap();
            let (name, args) = matches.subcommand().unwrap();
            match expected {
                Some(tool) => {
                    let (actual, payload) = translate(name, args, Path::new("/missing")).unwrap();
                    assert_eq!(actual, tool);
                    assert_eq!(payload["id"], "TASK-1");
                    assert_eq!(payload["comment_id"], id);
                    if verb == "edit" {
                        assert_eq!(payload["text"], "replacement");
                    }
                }
                None => assert!(translate(name, args, Path::new("/missing")).is_err()),
            }
        }
    }
}
