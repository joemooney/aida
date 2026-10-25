//! Explicit built-in host verification. trace:STORY-1673 | ai:codex

use crate::cli::VerifyCommand;
use aida_core::constraints::{verify_constraints, ConstraintResult, Status};
use anyhow::Result;

// trace:STORY-1728 | ai:claude
#[path = "verify_cmd_args.rs"]
pub(crate) mod verify_cmd_args;
pub(crate) use verify_cmd_args::*;

// trace:STORY-1673 | ai:codex
pub(crate) fn apply_verify_edit(
    req: &mut aida_core::Requirement,
    raw: Option<&str>,
    clear: bool,
) -> Result<Option<aida_core::FieldChange>> {
    if raw.is_none() && !clear {
        return Ok(None);
    }
    anyhow::ensure!(
        !(raw.is_some() && clear),
        "--verify conflicts with --clear-verify"
    );
    anyhow::ensure!(
        req.req_type == aida_core::RequirementType::Constraint,
        "verify blocks belong to constraint rows"
    );
    let value = raw
        .map(serde_json::from_str::<serde_json::Value>)
        .transpose()
        .map_err(|e| anyhow::anyhow!("--verify requires JSON data: {e}"))?;
    if req.verify == value {
        return Ok(None);
    }
    let describe = |v: &Option<serde_json::Value>| {
        v.as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "(absent)".into())
    };
    let change =
        aida_core::Requirement::field_change("verify", describe(&req.verify), describe(&value));
    req.verify = value;
    Ok(Some(change))
}

pub(crate) fn handle_verify_command(command: &VerifyCommand, file: Option<&str>) -> Result<()> {
    let VerifyCommand::Constraints {
        json,
        fail_on_drift,
        verbose,
    } = command;
    let path = match file {
        Some(path) => std::path::PathBuf::from(path),
        None => {
            let root = crate::find_project_root()?;
            // Canonical detection never falls back after a read failure.
            match crate::detect_distributed_store_from(&root) {
                Some(path) => path,
                None => aida_core::resolve_requirements_path_in(
                    &root,
                    None,
                    None,
                    std::path::Path::new(""),
                )?,
            }
        }
    };
    let rows = verify_constraints(&read_requirements(&path)?);
    println!(
        "{}",
        render(&rows, *json || crate::output_format_is_json(), *verbose)?
    );
    anyhow::ensure!(
        !fails_on_drift(&rows, *fail_on_drift),
        "constraint host drift detected"
    );
    Ok(())
}

// Avoid Storage::load: it can create a missing store or migrate old rows.
// This command reads data without creating stores, locks or caches.
// trace:STORY-1673 | ai:codex
fn read_requirements(path: &std::path::Path) -> Result<Vec<aida_core::Requirement>> {
    if std::fs::metadata(path)?.is_dir() {
        aida_core::object_store::list_objects_strict(&path.join("objects"))?
            .into_iter()
            .map(|(_, object)| aida_core::object_store::read_object_from_path(&object))
            .collect()
    } else {
        anyhow::ensure!(
            !matches!(path.extension().and_then(|e| e.to_str()), Some("db" | "sqlite" | "sqlite3")),
            "constraint verification requires canonical/YAML rows; archived SQLite does not carry verify blocks"
        );
        let file = std::fs::File::open(path)?;
        let store: aida_core::RequirementsStore =
            serde_yaml::from_reader(std::io::BufReader::new(file))?;
        Ok(store.requirements)
    }
}

fn fails_on_drift(rows: &[ConstraintResult], enabled: bool) -> bool {
    enabled && rows.iter().any(|row| row.status == Status::Drift)
}

fn render(rows: &[ConstraintResult], json: bool, verbose: bool) -> Result<String> {
    if json {
        return Ok(serde_json::to_string_pretty(rows)?);
    }
    if rows.is_empty() {
        return Ok("No constraint verify blocks configured.".into());
    }
    // Retired rows are noise in the default human view; name only the count.
    // trace:STORY-1673 | ai:claude
    let skipped = rows.iter().filter(|r| r.status == Status::Skipped).count();
    let mut output = String::new();
    for row in rows
        .iter()
        .filter(|r| verbose || r.status != Status::Skipped)
    {
        let status = match row.status {
            Status::Match => "match",
            Status::Drift => "drift",
            Status::Unknown => "unknown",
            Status::Invalid => "invalid",
            Status::Skipped => "skipped",
        };
        use std::fmt::Write;
        writeln!(
            output,
            "{}  {}  {}: {}",
            row.spec,
            status,
            row.probe.as_deref().unwrap_or("?"),
            row.detail
        )?;
        if let Some(expected) = &row.expected {
            writeln!(output, "  expected: {expected}")?;
        }
        if let Some(observed) = &row.observed {
            writeln!(output, "  observed: {observed}")?;
        }
    }
    if skipped > 0 && !verbose {
        use std::fmt::Write;
        writeln!(
            output,
            "{skipped} retired constraint(s) skipped; use --verbose to list them."
        )?;
    }
    Ok(output.trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serde_json::json;

    #[test]
    fn story1673_cli_parses_only_explicit_constraints_command() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "verify",
            "constraints",
            "--json",
            "--fail-on-drift",
        ])
        .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::verify_cmd::VerifyArgs>(),
            crate::verify_cmd::VerifyArgs {
                inner: VerifyCommand::Constraints {
                    json: true,
                    fail_on_drift: true,
                    verbose: false
                }
            }
        ));
        for args in [
            vec!["aida", "verify"],
            vec!["aida", "verify", "shell"],
            vec![
                "aida",
                "verify",
                "constraints",
                "--command",
                "touch sentinel",
            ],
        ] {
            assert!(crate::command_registry::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn story1673_json_human_and_drift_exit_gate() {
        for status in [
            Status::Match,
            Status::Drift,
            Status::Unknown,
            Status::Invalid,
        ] {
            let rows = [ConstraintResult {
                spec: "CNST-1".into(),
                title: "fixture".into(),
                probe: Some("cpu-count".into()),
                status,
                expected: Some(json!(6)),
                observed: Some(json!(4)),
                detail: "fixture detail".into(),
            }];
            let output: serde_json::Value =
                serde_json::from_str(&render(&rows, true, false).unwrap()).unwrap();
            assert_eq!(output[0]["spec"], "CNST-1");
            assert_eq!(output[0]["status"], serde_json::to_value(status).unwrap());
            let human = render(&rows, false, true).unwrap();
            assert!(
                human.contains("CNST-1")
                    && human.contains("expected: 6")
                    && human.contains("observed: 4")
            );
            assert!(!fails_on_drift(&rows, false));
            assert_eq!(fails_on_drift(&rows, true), status == Status::Drift);
        }
        assert_eq!(render(&[], true, false).unwrap(), "[]");
        assert!(render(&[], false, false).unwrap().contains("No constraint"));
    }

    // trace:STORY-1673 | ai:claude
    #[test]
    fn story1673_skipped_rows_hidden_unless_verbose_and_never_fail_drift() {
        let row = |spec: &str, status| ConstraintResult {
            spec: spec.into(),
            title: "fixture".into(),
            probe: Some("disk-rotational".into()),
            status,
            expected: Some(json!(true)),
            observed: None,
            detail: "superseded constraint not probed".into(),
        };
        let rows = [row("CON-1", Status::Match), row("CON-2", Status::Skipped)];
        let human = render(&rows, false, false).unwrap();
        assert!(human.contains("CON-1") && !human.contains("CON-2"));
        assert!(human.contains("1 retired constraint(s) skipped"));
        let verbose = render(&rows, false, true).unwrap();
        assert!(verbose.contains("CON-2  skipped") && !verbose.contains("retired constraint(s)"));
        let output: serde_json::Value =
            serde_json::from_str(&render(&rows, true, false).unwrap()).unwrap();
        assert_eq!(output[1]["status"], "skipped");
        assert!(!fails_on_drift(&rows, true));
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "verify",
            "constraints",
            "--verbose"
        ])
        .is_ok());
    }
}

#[cfg(test)]
mod edit_tests {
    use super::*;
    use crate::edit_rebase::{rebase_edit, EditMerge};
    use aida_core::{Requirement, RequirementType};
    use serde_json::json;

    #[test]
    fn story1673_verify_edit_is_data_only_idempotent_and_records_exact_delta() {
        let mut req = Requirement::new("fixture".into(), "fixture".into());
        assert!(apply_verify_edit(&mut req, Some("{}"), false).is_err());
        req.req_type = RequirementType::Constraint;
        let raw = r#"{"version":99,"probe":"shell","args":{"command":"touch sentinel"},"expect":{"op":"eq","value":true}}"#;
        assert_eq!(
            apply_verify_edit(&mut req, Some(raw), false)
                .unwrap()
                .unwrap()
                .field_name,
            "verify"
        );
        assert_eq!(req.verify.as_ref().unwrap()["version"], json!(99));
        assert!(apply_verify_edit(&mut req, Some(raw), false)
            .unwrap()
            .is_none());
        let before = req.verify.clone();
        assert!(apply_verify_edit(&mut req, Some("invalid json"), false).is_err());
        assert_eq!(req.verify, before);
        assert!(apply_verify_edit(&mut req, Some("{}"), true).is_err());
        assert!(apply_verify_edit(&mut req, None, true).unwrap().is_some());
        assert_eq!(req.verify, None);
        assert!(apply_verify_edit(&mut req, Some("null"), false)
            .unwrap()
            .is_some());
        assert_eq!(req.verify, Some(serde_json::Value::Null));
    }

    #[test]
    fn story1673_targeted_edit_merges_verify_atomically_preserving_explicit_null() {
        for (before, after) in [
            (None, Some(serde_json::Value::Null)),
            (Some(serde_json::Value::Null), None),
            (Some(json!({"probe":"old"})), Some(json!({"probe":"new"}))),
        ] {
            let mut read = Requirement::new("fixture".into(), "fixture".into());
            read.verify = before;
            let mut planned = read.clone();
            planned.verify = after.clone();
            let mut current = read.clone();
            current.title = "concurrent".into();
            let merged = rebase_edit(&read, &planned, &current, EditMerge::default())
                .unwrap()
                .unwrap();
            assert_eq!(merged.verify, after);
            assert_eq!(merged.title, "concurrent");
            current.verify = Some(json!({"args":{"unexpected":true}}));
            assert_eq!(
                rebase_edit(&read, &planned, &current, EditMerge::default())
                    .unwrap()
                    .unwrap_err(),
                vec!["verify"]
            );
        }
    }
}

#[cfg(test)]
mod read_tests {
    use super::*;
    use aida_core::{DatabaseBackend, GitBackend, Requirement, RequirementType, RequirementsStore};

    #[test]
    fn story1673_read_path_does_not_create_or_migrate_files() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing/subdir/rows.yaml");
        assert!(read_requirements(&missing).is_err());
        assert!(!dir.path().join("missing").exists());
        let empty_dir = dir.path().join("empty");
        std::fs::create_dir(&empty_dir).unwrap();
        assert!(read_requirements(&empty_dir).is_err());
        assert_eq!(std::fs::read_dir(&empty_dir).unwrap().count(), 0);
        let mut req = Requirement::new("legacy row".into(), "no id".into());
        req.req_type = RequirementType::Constraint;
        req.verify = Some(serde_json::Value::Null);
        let mut store = RequirementsStore::new();
        store.requirements.push(req);
        let yaml = serde_yaml::to_string(&store).unwrap();
        let path = dir.path().join("rows.yaml");
        std::fs::write(&path, &yaml).unwrap();
        let rows = read_requirements(&path).unwrap();
        assert!(rows[0].spec_id.is_none());
        assert_eq!(rows[0].verify, Some(serde_json::Value::Null));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), yaml);
        assert!(!path.with_extension("lock").exists());
    }

    #[test]
    fn story1673_canonical_invalid_verify_reports_but_invalid_object_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        let backend = crate::test_env::home_fixture_backend(&root).unwrap();
        let mut req = Requirement::new("fixture".into(), "fixture".into());
        req.spec_id = Some("CNST-1".into());
        req.req_type = RequirementType::Constraint;
        req.verify = Some(serde_json::Value::Null);
        let mut store = RequirementsStore::new();
        store.requirements.push(req);
        backend.save(&store).unwrap();
        let objects = aida_core::object_store::list_objects_strict(&root.join("objects")).unwrap();
        let object = &objects[0].1;
        let before = std::fs::read(object).unwrap();
        let rows = verify_constraints(&read_requirements(&root).unwrap());
        assert_eq!(rows[0].status, Status::Invalid);
        assert_eq!(std::fs::read(object).unwrap(), before);
        std::fs::write(object, "id: [invalid uuid]\n").unwrap();
        assert!(read_requirements(&root).is_err());
        let sqlite = dir.path().join("legacy.db");
        std::fs::write(&sqlite, "do not touch").unwrap();
        assert!(read_requirements(&sqlite).is_err());
        assert_eq!(std::fs::read_to_string(sqlite).unwrap(), "do not touch");
    }
}

/// Explicit, report-only host verification using built-in probes.
/// Never executes a command from the requirements store.
// trace:STORY-1673 | ai:codex
#[derive(clap::Parser, Debug)]
#[clap(name = "verify")]
pub(crate) struct VerifyArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::VerifyCommand,
}

// Explicit only: no sync/drain/hook path invokes host probes.
// trace:STORY-1673 | ai:codex
fn run_verify(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    use crate::*;
    let args = <VerifyArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let cmd = &args.inner;
    return self::handle_verify_command(
        cmd,
        ctx.globals.and_then(|globals| globals.file.as_deref()),
    );
}

// trace:STORY-1728 | ai:claude
pub(crate) static VERIFY: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "verify",
        help_after: "lint",
        command: <VerifyArgs as clap::CommandFactory>::command,
        // Main's table had no read arm for top-level `verify` (only `db block
        // verify`), so it fell through to Write; guests stay refused.
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_verify),
    };
