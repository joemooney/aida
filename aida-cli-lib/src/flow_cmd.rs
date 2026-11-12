//! Hidden substrate JSON reader for the external flow renderer.
// trace:STORY-2704 | ai:codex
use clap::CommandFactory;
#[path = "flow_cmd_args.rs"]
mod args;
// The built-in owns evidence collection; porcelain consumes its public JSON.
#[path = "flow_report.rs"]
mod report;
use args::*;
pub(crate) static FLOW: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "flow-data",
    help_after: "findings",
    command: || FlowArgs::command().name("flow-data").hide(true),
    policy: |_| crate::cmdkit::CmdPolicy {
        stakeholder: crate::cmdkit::StakeholderAction::Read,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::cmdkit::Handler::Early(run),
};

#[cfg(test)]
mod tests {
    use super::*;
    use clap::FromArgMatches;
    // trace:STORY-2699 | ai:codex
    #[test]
    fn flow_json_projection_passes_dispatch_gate() {
        for spelling in ["--format=json", "--json"] {
            let mut argv: Vec<String> = [
                "aida",
                "flow-data",
                "report",
                "--since",
                "2026-11-09",
                spelling,
            ]
            .into_iter()
            .map(str::to_string)
            .collect();
            crate::json_capability_gate(&mut argv).unwrap();
            let matches = crate::command_registry::built_command()
                .try_get_matches_from(argv)
                .unwrap();
            let flow = matches.subcommand_matches("flow-data").unwrap();
            assert!(matches!(
                FlowArgs::from_arg_matches(flow).unwrap().command,
                FlowCommand::Report { json: true, .. }
            ));
        }
    }
    #[test]
    fn flow_registered_read_only_args() {
        let cmd = crate::command_registry::built_command();
        let matches = cmd
            .try_get_matches_from([
                "aida",
                "flow-data",
                "report",
                "--since",
                "2026-11-09",
                "--format",
                "json",
            ])
            .unwrap();
        let sub = matches.subcommand_matches("flow-data").unwrap();
        assert_eq!(
            (FLOW.policy)(sub).stakeholder,
            crate::cmdkit::StakeholderAction::Read
        );
        assert!(matches!(
            FlowArgs::from_arg_matches(sub).unwrap().command,
            FlowCommand::Report { .. }
        ));
        assert_eq!(
            sub.get_one::<crate::cli::OutputFormat>("format"),
            Some(&crate::cli::OutputFormat::Json)
        );
    }
}

fn run(matches: &clap::ArgMatches, _: &crate::cmdkit::DispatchCtx) -> anyhow::Result<()> {
    report::run(matches, crate::output_format_is_json())
}
