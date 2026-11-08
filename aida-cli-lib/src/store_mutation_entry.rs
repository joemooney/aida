//! Early dispatch for the still-gated mediated profile. Ordinary handlers,
//! aliases, extensions, cache workers and telemetry never run in this path.
// trace:TASK-3223 | ai:codex
use crate::store_mutation_client::Client;
use aida_core::exec_context::Environment;
use anyhow::{ensure, Context, Result};
use clap::parser::ValueSource;
use serde_json::{json, Value};
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
    let (tool, payload) = translate(name, args)?;
    let action = if tool == "add_comment" {
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
    let result = client.mcp(&root(&matches, cwd)?, tool, &payload)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

fn translate(name: &str, args: &clap::ArgMatches) -> Result<(&'static str, Value)> {
    let command = crate::command_registry::built_command();
    let command = command
        .find_subcommand(name)
        .context("UnsupportedOperation")?;
    match name {
        "comment" => {
            let (name, args) = args.subcommand().context("UnsupportedOperation")?;
            ensure!(name == "add", "UnsupportedOperation");
            explicit_fields(
                args,
                &["id", "content", "content_positional"],
                command
                    .find_subcommand("add")
                    .context("UnsupportedOperation")?,
            )?;
            let id = args.get_one::<String>("id").context("InvalidRequest")?;
            let text = args
                .get_one::<String>("content")
                .or_else(|| args.get_one::<String>("content_positional"))
                .context("UnsupportedOperation: provide inline comment content")?;
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
            assert_eq!(translate(name, args).unwrap(), (tool, payload));
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
            assert!(translate(name, args).is_err());
        }
    }
}
