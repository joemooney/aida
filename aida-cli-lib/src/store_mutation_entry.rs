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

pub(crate) fn entry(
    env: &Environment,
    argv: &[std::ffi::OsString],
    cwd: Option<&Path>,
) -> Option<Result<()>> {
    match Client::from_environment(env) {
        Ok(None) => None,
        Err(error) => Some(Err(error)),
        Ok(Some(client)) => Some(
            cwd.context("mediated store cwd unavailable")
                .and_then(|cwd| {
                    let argv = argv
                        .iter()
                        .map(|arg| {
                            arg.to_str()
                                .map(str::to_owned)
                                .context("InvalidRequest: non-Unicode argument")
                        })
                        .collect::<Result<Vec<_>>>()?;
                    run(&client, env, &argv, cwd)
                }),
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

// Staged restriction: literal store link/directory only; no config or Git fallback.
// trace:TASK-3223 | ai:codex
fn root(cwd: &Path) -> Result<PathBuf> {
    let root = cwd
        .ancestors()
        .map(|p| p.join(".aida-store"))
        .find(|p| p.join("objects").is_dir())
        .context("mediated operations require an existing canonical store")?;
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
    explicit_fields(&matches, &[], &command)?;
    let (name, args) = matches.subcommand().context("UnsupportedOperation")?;
    if name == "mcp-serve" {
        return crate::command_registry::run_mediated_mcp_handler(client, env, &root(cwd)?);
    }
    let (tool, payload) = translate(name, args, cwd)?;
    let action = if tool == "add_comment" {
        crate::StakeholderAction::Comment
    } else {
        crate::StakeholderAction::EditRequirement
    };
    if let Some(role) = crate::store_mutation_client::session_role(env)? {
        ensure!(
            crate::stakeholder_action_allowed(&role, action),
            "UnsupportedOperation"
        );
    }
    let store = root(cwd)?;
    let result = client.mcp(&store, tool, &payload)?;
    // A committed mutation stays successful when a downstream consumer closes
    // stdout. Other I/O failures are returned without a println! panic.
    write_result(&mut std::io::stdout().lock(), &result)
        .context("mutation committed; output failed; do not repeat the mutation")?;
    Ok(())
}

// trace:TASK-3223 | ai:codex
fn write_result(output: &mut impl std::io::Write, result: &Value) -> Result<()> {
    match writeln!(output, "{}", serde_json::to_string(result)?) {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result?),
    }
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
    comment_text(file)
}

/// Bound every text source before sending it to the host. No ordinary comment
/// handler, editor, backend or cache setup runs for streamed input.
// trace:TASK-3223 | ai:codex
fn comment_text(reader: impl Read) -> Result<String> {
    const LIMIT: u64 = 16384;
    let mut text = String::new();
    reader.take(LIMIT + 1).read_to_string(&mut text)?;
    ensure!(
        text.len() <= LIMIT as usize && !text.trim().is_empty(),
        "InvalidRequest: comment body is empty or too large"
    );
    Ok(text)
}

fn translate(name: &str, args: &clap::ArgMatches, cwd: &Path) -> Result<(&'static str, Value)> {
    translate_with_reader(name, args, cwd, &mut std::io::stdin().lock())
}

fn translate_with_reader(
    name: &str,
    args: &clap::ArgMatches,
    cwd: &Path,
    stdin: &mut impl Read,
) -> Result<(&'static str, Value)> {
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
                &["id", "content", "content_positional", "body_file", "stdin"],
                command
                    .find_subcommand("add")
                    .context("UnsupportedOperation")?,
            )?;
            let id = args.get_one::<String>("id").context("InvalidRequest")?;
            let text = match args.get_one::<PathBuf>("body_file") {
                Some(path) => comment_file(&cwd.join(path))?,
                None if args.get_flag("stdin") => comment_text(stdin)?,
                None => args
                    .get_one::<String>("content")
                    .or_else(|| args.get_one::<String>("content_positional"))
                    .context("UnsupportedOperation: provide inline comment content, --body-file or --stdin")?
                    .clone(),
            };
            Ok(("add_comment", json!({"id":id,"text":text})))
        }
        "edit" => {
            explicit_fields(
                args,
                &["id", "title", "description", "priority", "weight", "tags"],
                command,
            )?;
            let id = args.get_one::<String>("id").context("InvalidRequest")?;
            let mut payload = json!({"id":id});
            for field in ["title", "description", "priority"] {
                if let Some(value) = args.get_one::<String>(field) {
                    payload[field] = json!(value);
                }
            }
            if let Some(value) = args.get_one::<String>("tags") {
                payload["tags"] = json!(crate::requirements_cli::parse_tag_list(value)?);
            }
            if let Some(value) = args.get_one::<String>("weight") {
                payload["weight"] = if value.trim().is_empty() {
                    serde_json::Value::Null
                } else {
                    let weight: f32 = value
                        .trim()
                        .parse()
                        .context("InvalidRequest: weight must be finite")?;
                    ensure!(weight.is_finite(), "InvalidRequest: weight must be finite");
                    json!(weight)
                };
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

    // trace:TASK-3223 | ai:codex
    #[test]
    fn staged_root_refuses_custom_store_and_linked_worktree_without_store_link() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        std::fs::create_dir_all(project.join("custom-store/objects")).unwrap();
        std::fs::write(
            project.join(".aida/config.toml"),
            "[deployment]\nstore_path = 'custom-store'\n",
        )
        .unwrap();
        let before = std::fs::read(project.join(".aida/config.toml")).unwrap();
        let error = root(&project).unwrap_err();
        assert!(error.to_string().contains("existing canonical store"));
        assert_eq!(
            std::fs::read(project.join(".aida/config.toml")).unwrap(),
            before
        );
        assert!(!project.join(".aida-store").exists());

        // Model a linked worktree's read-only .git pointer. Staged discovery
        // must not consult or modify it, or implicitly adopt the main store.
        let linked = tmp.path().join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(
            linked.join(".git"),
            "gitdir: ../project/.git/worktrees/linked\n",
        )
        .unwrap();
        assert!(root(&linked).is_err());
        std::os::unix::fs::symlink(project.join("custom-store"), linked.join(".aida-store"))
            .unwrap();
        assert_eq!(root(&linked).unwrap(), linked.join(".aida-store"));
    }

    #[test]
    fn root_discovery_is_read_only_and_global_semantics_never_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        let nested = tmp.path().join("src/nested");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(root(&nested).unwrap(), store);
        assert!(!store.join("metadata.yaml").exists());
        assert!(!store.join(".git").exists());
        let environment = Environment::from_pairs([
            ("AIDA_STORE_MUTATION_PROFILE".into(), "mediated".into()),
            (
                "AIDA_STORE_MUTATION_ENDPOINT".into(),
                format!("/run/aida-mutation/{}/service", uuid::Uuid::new_v4()).into(),
            ),
        ]);
        for flags in [
            vec!["--file", "/unreadable"],
            vec!["--format", "json"],
            vec!["--canonical-ids"],
        ] {
            let mut argv = vec!["aida"];
            argv.extend(flags);
            argv.extend(["comment", "add", "TASK-1", "hello"]);
            let argv = argv
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>();
            let error = entry(&environment, &argv, Some(&nested))
                .unwrap()
                .unwrap_err();
            assert!(
                error.to_string().contains("UnsupportedOperation"),
                "{error}"
            );
        }
        assert!(root(Path::new("/nonexistent-store-cwd")).is_err());
    }

    #[test]
    fn non_unicode_mediated_arguments_refuse_without_dispatch_or_panic() {
        use std::os::unix::ffi::OsStringExt;
        let environment = Environment::from_pairs([
            ("AIDA_STORE_MUTATION_PROFILE".into(), "mediated".into()),
            (
                "AIDA_STORE_MUTATION_ENDPOINT".into(),
                format!("/run/aida-mutation/{}/service", uuid::Uuid::new_v4()).into(),
            ),
        ]);
        let mut argv: Vec<std::ffi::OsString> = ["aida", "comment", "add", "TASK-1", "--body-file"]
            .into_iter()
            .map(Into::into)
            .collect();
        argv.push(std::ffi::OsString::from_vec(vec![0xff]));
        let error = entry(&environment, &argv, Some(Path::new("/missing")))
            .unwrap()
            .unwrap_err();
        assert_eq!(error.to_string(), "InvalidRequest: non-Unicode argument");
        assert!(entry(&Environment::default(), &argv, None).is_none());
    }

    #[test]
    fn committed_result_output_tolerates_closed_consumers_without_a_panic() {
        struct Failing(std::io::ErrorKind);
        impl std::io::Write for Failing {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(self.0.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let result = json!({"spec_id":"TASK-1"});
        write_result(&mut Failing(std::io::ErrorKind::BrokenPipe), &result).unwrap();
        assert!(write_result(&mut Failing(std::io::ErrorKind::PermissionDenied), &result).is_err());
        let mut bytes = Vec::new();
        write_result(&mut bytes, &result).unwrap();
        assert_eq!(bytes, b"{\"spec_id\":\"TASK-1\"}\n");
    }

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
                vec!["aida", "edit", "TASK-1", "--tags", "alpha, beta,alpha"],
                "update_requirement",
                json!({"id":"TASK-1", "tags":["alpha", "beta", "alpha"]}),
            ),
            (
                vec!["aida", "edit", "TASK-1", "--tags", ""],
                "update_requirement",
                json!({"id":"TASK-1", "tags":[]}),
            ),
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
            (
                vec!["aida", "edit", "TASK-1", "--weight", "0.75"],
                "update_requirement",
                json!({"id":"TASK-1","weight":0.75}),
            ),
            (
                vec!["aida", "edit", "TASK-1", "--weight", ""],
                "update_requirement",
                json!({"id":"TASK-1","weight":null}),
            ),
            (
                vec!["aida", "edit", "TASK-1", "--priority", "med"],
                "update_requirement",
                json!({"id":"TASK-1","priority":"med"}),
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
            vec!["aida", "edit", "TASK-1", "--tags", "a b"],
            vec!["aida", "edit", "TASK-1", "--tags", "a", "--force"],
            vec!["aida", "edit", "TASK-1", "--add-tag", "a"],
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
    fn streamed_comments_preserve_literal_text_and_bound_input_before_transport() {
        for (argv, expected) in [(
            vec!["aida", "comment", "add", "TASK-1", "--stdin"],
            "add_comment",
        )] {
            let matches = crate::command_registry::built_command()
                .try_get_matches_from(argv)
                .unwrap();
            let (name, args) = matches.subcommand().unwrap();
            let text = "literal `command` $(command)\n\tsecond line";
            let (tool, payload) =
                translate_with_reader(name, args, Path::new("/missing"), &mut text.as_bytes())
                    .unwrap();
            assert_eq!(tool, expected);
            assert_eq!(payload["text"], text);
            for bytes in [vec![], vec![b' '; 20], vec![0xff], vec![b'x'; 16385]] {
                assert!(translate_with_reader(
                    name,
                    args,
                    Path::new("/missing"),
                    &mut bytes.as_slice()
                )
                .is_err());
            }
            // A hostile stream is consumed only up to limit + one byte.
            let bytes = vec![b'x'; 32768];
            let mut reader = std::io::Cursor::new(bytes);
            assert!(translate_with_reader(name, args, Path::new("/missing"), &mut reader).is_err());
            assert_eq!(reader.position(), 16385);
            let bytes = vec![b'x'; 16384];
            assert_eq!(
                translate_with_reader(name, args, Path::new("/missing"), &mut bytes.as_slice())
                    .unwrap()
                    .1["text"]
                    .as_str()
                    .unwrap()
                    .len(),
                16384
            );
        }
    }

    #[test]
    fn unsupported_comment_authority_fields_refuse_before_consuming_stdin() {
        struct Unreadable;
        impl Read for Unreadable {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("unsupported operation consumed stdin");
            }
        }
        for extra in [
            vec!["--author", "other"],
            vec!["--relayed-from", "advisor"],
            vec!["--parent", "parent"],
        ] {
            let mut argv = vec!["aida", "comment", "add", "TASK-1", "--stdin"];
            argv.extend(extra);
            let matches = crate::command_registry::built_command()
                .try_get_matches_from(argv)
                .unwrap();
            let (name, args) = matches.subcommand().unwrap();
            let error = translate_with_reader(name, args, Path::new("/missing"), &mut Unreadable)
                .unwrap_err();
            assert!(error.to_string().contains("UnsupportedOperation"));
        }
    }

    #[test]
    fn existing_comment_mutations_refuse_without_an_approved_ownership_policy() {
        let id = uuid::Uuid::new_v4().to_string();
        for (verb, extra) in [
            ("edit", vec!["--content", "replacement"]),
            ("delete", vec![]),
            ("edit", vec!["--interactive"]),
            ("edit", vec![]),
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
            assert!(translate(name, args, Path::new("/missing"))
                .unwrap_err()
                .to_string()
                .contains("UnsupportedOperation"));
        }
    }
}
