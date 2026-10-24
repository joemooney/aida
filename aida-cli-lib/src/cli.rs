use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

// Command clap types live with their commands (`<module>_args.rs`); these
// re-exports keep `crate::cli::X` paths working. trace:STORY-1728 | ai:claude
pub(crate) use crate::advisor_cmd_root::advisor_cmd_root_args::{AdvisorCommand, ScheduleCommand};
pub(crate) use crate::agent_launch::agent_launch_args::{
    AgentCommand, AgentNewCommand, SeatCommand, SessionCommand, SessionManifestCommand,
};
pub(crate) use crate::approvals_cmd::approvals_cmd_args::{
    ApprovalCommand, ProxyApprovalIndependent,
};
pub(crate) use crate::autonomy_cmd::autonomy_cmd_args::{AutonomyCommand, CalibrationSubcommand};
pub(crate) use crate::autopilot_intake::autopilot_intake_args::AutopilotCommand;
pub(crate) use crate::backlog::backlog_args::BacklogCommand;
pub(crate) use crate::brief_cmd::brief_cmd_args::BriefCommand;
pub(crate) use crate::burndown_run::burndown_run_args::BurndownCommand;
pub(crate) use crate::cache_cmd::cache_cmd_args::CacheCommand;
pub(crate) use crate::changelog_cmd::changelog_cmd_args::ChangelogCommand;
pub(crate) use crate::comment_cmd::comment_cmd_args::CommentCommand;
pub(crate) use crate::config_cmd::config_cmd_args::{
    ConfigCommand, ConfigPermissionTier, ConfigPermissionsCommand, GlyphCommand,
};
pub(crate) use crate::db_cmd::db_cmd_args::{BlockCommand, DbCommand};
pub(crate) use crate::deps_cmd::deps_cmd_args::DepsCommand;
pub(crate) use crate::dev_cmd::dev_cmd_args::{DevCleanMode, DevCommand};
pub(crate) use crate::dispatch::dispatch_args::{
    DispatchAction, DispatchRequestVerb, DispatchStatusFilter, DispatchVendor,
};
pub(crate) use crate::doc_cmd::doc_cmd_args::{DocCommand, DocsCommand};
pub(crate) use crate::doctor_cmd::doctor_cmd_args::DoctorCommand;
pub(crate) use crate::drain_cmd::drain_cmd_args::DrainCommand;
pub(crate) use crate::drain_gate::drain_gate_args::NoHumanCommand;
pub(crate) use crate::feature_cmd::feature_cmd_args::FeatureCommand;
pub(crate) use crate::field_study_cmd::field_study_cmd_args::FieldStudyCommand;
pub(crate) use crate::findings_gate::findings_gate_args::{CalibrationAction, FindingsCommand};
pub(crate) use crate::focus_cmd::focus_cmd_args::FocusCommand;
pub(crate) use crate::gate_cmd::gate_cmd_args::GateCommand;
pub(crate) use crate::graph_cmd::graph_cmd_args::GraphCommand;
pub(crate) use crate::history::history_args::{
    HistoryCommand, HistoryTemplatesCommand, HistoryViewArgs,
};
pub(crate) use crate::human_cmd::human_cmd_args::HumanCommand;
pub(crate) use crate::identity_gate::identity_gate_args::IdentityCommand;
pub(crate) use crate::init_cmd::init_cmd_args::InitFootprint;
pub(crate) use crate::internal_cmd::internal_cmd_args::InternalCommand;
pub(crate) use crate::leases::leases_args::FasttrackCommand;
pub(crate) use crate::lint_cmd::lint_cmd_args::{LintCommand, LintConstantsFailOn};
pub(crate) use crate::load_cmd::load_cmd_args::LoadCommand;
pub(crate) use crate::lock_cmd::lock_cmd_args::LockCommand;
pub(crate) use crate::mailbox_cmd::mailbox_cmd_args::MailboxCommand;
pub(crate) use crate::maintenance_schedule::maintenance_schedule_args::MaintenanceScheduleCommand;
pub(crate) use crate::mandate::mandate_args::MandateAction;
pub(crate) use crate::mandate_drive::mandate_drive_args::FleetAction;
pub(crate) use crate::mcp_cmd::mcp_cmd_args::McpCommand;
pub(crate) use crate::memories_cmd::memories_cmd_args::MemoriesCommand;
pub(crate) use crate::merge_hold_cmd::merge_hold_cmd_args::MergeHoldAction;
pub(crate) use crate::metrics_cmd::metrics_cmd_args::MetricsCommand;
pub(crate) use crate::node_cmd::node_cmd_args::NodeCommand;
pub(crate) use crate::notify::notify_args::NotifyCommand;
pub(crate) use crate::orchestrator_cmd::orchestrator_cmd_args::OrchestratorCommand;
pub(crate) use crate::plan_cmd::plan_cmd_args::PlanCommand;
pub(crate) use crate::plan_intent::plan_intent_args::SpecCommand;
pub(crate) use crate::pr_cmd::pr_cmd_args::PrCommand;
pub(crate) use crate::presence_cmd::presence_cmd_args::PresenceCommand;
pub(crate) use crate::protocol_cmd::protocol_cmd_args::ProtocolCommand;
pub(crate) use crate::punt_done::punt_done_args::PuntsCommand;
pub(crate) use crate::questions::questions_args::QuestionsCommand;
pub(crate) use crate::queue_cmd::queue_cmd_args::QueueCommand;
pub(crate) use crate::queue_misc::queue_misc_args::{HeadlessCommand, StackCommand, WorkerCommand};
pub(crate) use crate::record_cmd::record_cmd_args::RecordCommand;
pub(crate) use crate::rel_def_cmd::rel_def_cmd_args::RelDefCommand;
pub(crate) use crate::relationship_cmd::relationship_cmd_args::RelationshipCommand;
pub(crate) use crate::remote_cmd::remote_cmd_args::RemoteCommand;
pub(crate) use crate::report_cmd::report_cmd_args::ReportCommand;
pub(crate) use crate::review_cmd::review_cmd_args::{
    MassChangeAction, ReviewCommand, ReviewModeCommand,
};
pub(crate) use crate::role_cmd::role_cmd_args::{RoleCommand, RolePromptCommand, RoleScopeCommand};
pub(crate) use crate::rules_cmd::rules_cmd_args::RulesCommand;
pub(crate) use crate::sandbox_cmd::sandbox_cmd_args::SandboxCommand;
pub(crate) use crate::scaffold_cmd::scaffold_cmd_args::{ScaffoldCommand, ScaffoldUpgradeOnly};
pub(crate) use crate::server_cmd::server_cmd_args::ServerCommand;
pub(crate) use crate::shift::shift_args::ShiftCommand;
pub(crate) use crate::solo_cmd::solo_cmd_args::SoloAction;
pub(crate) use crate::statusline_cmd::statusline_cmd_args::StatuslineAction;
pub(crate) use crate::store_cmd::store_cmd_args::StoreCommand;
pub(crate) use crate::supervise_cmd::supervise_cmd_args::SuperviseCommand;
pub(crate) use crate::terminal_cmd::terminal_cmd_args::TerminalCommand;
pub(crate) use crate::trace_cmd::trace_cmd_args::{TraceCommand, TraceCoverageFailOn};
pub(crate) use crate::tracker_cmd::tracker_cmd_args::{GitHubCommand, GitLabCommand, JiraCommand};
pub(crate) use crate::triage_cmd::triage_cmd_args::TriageCommand;
pub(crate) use crate::type_cmd::type_cmd_args::TypeCommand;
pub(crate) use crate::upgrade_cmd::upgrade_cmd_args::UpgradeCommand;
pub(crate) use crate::upgrade_release::upgrade_release_args::ReleaseCommand;
pub(crate) use crate::usage_cmd::usage_cmd_args::UsageCommand;
pub(crate) use crate::user_alias::user_alias_args::AliasCommand;
pub(crate) use crate::verify_cmd::verify_cmd_args::VerifyCommand;
pub(crate) use crate::wiki::wiki_args::WikiCommand;
pub(crate) use crate::worktree_gc::worktree_gc_args::{WorktreeCommand, WorktreePoolCommand};
pub(crate) use crate::zen_cmd::zen_cmd_args::ZenCommand;

/// Explicit output-format pin. Overrides the default, which is chosen from
/// whether stdout is a terminal: the human table at a TTY, token-efficient
/// TOON when the output is piped or captured. That silent switch broke real
/// user scripts that parsed the table, so `--format` (or `AIDA_OUTPUT_FORMAT`)
/// lets a script pin the format it expects regardless of the TTY.
// trace:STORY-764 | ai:claude
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable tables / emoji (the interactive default at a TTY).
    Human,
    /// Token-efficient TOON — the compact agent default when piped/captured.
    Toon,
    /// Machine JSON, for commands that support a JSON projection.
    Json,
}

#[derive(Parser, Debug)]
#[clap(
    author,
    version,
    about = "AI-native requirements management — durable, agent-readable specs",
    after_help = "Bare `aida` / `aida help` leads with a small Getting-started set. \
                  Run `aida help --all` for the full command surface grouped by topic."
)]
pub struct Cli {
    #[clap(flatten)]
    pub globals: GlobalArgs,

    #[clap(subcommand)]
    pub command: Command,
}

// The top-level options, shared by both dispatch paths: a registered command
// has no `Command` variant to parse through, so its handler reads these from
// the same `ArgMatches`. No doc comment: on a flattened `Args` struct it would
// replace the root `about`. trace:STORY-1728 | ai:claude
#[derive(clap::Args, Debug)]
pub struct GlobalArgs {
    /// Path to the requirements file (overrides auto-detection)
    #[clap(long)]
    pub file: Option<String>,

    /// Project name to use from central registry
    #[clap(long, short = 'p')]
    pub project: Option<String>,

    /// Connect to a remote AIDA server (e.g., "localhost:50051" or "grpc://host:port")
    /// Can also be set via AIDA_SERVER environment variable
    #[clap(long, short = 's')]
    pub server: Option<String>,

    /// Record this invocation with asciinema when running in a terminal
    #[clap(long)]
    pub asciinema: bool,

    /// Output path for --asciinema recordings
    #[clap(long, value_name = "PATH", requires = "asciinema")]
    pub cast_out: Option<PathBuf>,

    /// Title for --asciinema recordings
    #[clap(long, value_name = "TITLE", requires = "asciinema")]
    pub cast_title: Option<String>,

    /// Print the absolute path to the current aida binary and exit.
    // trace:TASK-1718 | ai:antigravity
    #[clap(long)]
    pub path: bool,

    /// Pin the output format regardless of whether stdout is a terminal:
    /// human = tables, toon = compact agent output, json = machine JSON.
    /// Default (unset) is human at a TTY, toon when piped/captured. Also
    /// settable via AIDA_OUTPUT_FORMAT (the flag wins).
    #[clap(long, global = true, value_enum, value_name = "FORMAT")]
    pub format: Option<OutputFormat>,
}

// trace:FR-1-043 | ai:claude

// `session forget` (single-target .jsonl removal) and `session wakeup`
// (fallback-wakeup registry) were cut as orphaned zero-call verbs — no
// skill, harness, MCP tool, or internal command invoked them. `session
// prune` covers bulk cleanup; `session manifest` (MCP-wired) stays.
// trace:TASK-850 | ai:claude

// Team RBAC: manage durable per-user roles in the shared roster
// (`registry/team.toml`). trace:STORY-646 | ai:claude
#[derive(Subcommand, Debug)]
pub enum TeamCommand {
    /// Set a user's durable team role in the roster (`registry/team.toml`,
    /// CAS push-wins). A rostered role is the user's effective role even with
    /// no `AIDA_SESSION_ROLE` set, so the advisor guardrail survives a
    /// forgotten env var.
    ///
    /// GUARDRAIL, NOT SECURITY: the store is a shared git branch, so anyone
    /// with push access can edit any spec directly with raw git regardless of
    /// their role. This roster prevents *accidents* (an implementer
    /// accidentally approving a spec) and records team structure + an audit
    /// trail — it is NOT an access-control boundary.
    // trace:STORY-646 | ai:claude
    SetRole {
        /// The user id (the person — matches `current_user_id`: `$AIDA_USER`
        /// / `$USER`). Not a node id.
        user: String,
        /// The role to grant (e.g. `advisor`, `implementer`). Validated
        /// against the known role set.
        #[clap(long)]
        role: String,
    },

    /// Add one seat to a user's allowed-seat set. Requires a human at a TTY.
    // trace:STORY-1473 | ai:codex
    AllowSeat { user: String, seat: String },

    /// Remove one seat from a user's allowed-seat set. Requires a human at a TTY.
    // trace:STORY-1473 | ai:codex
    DisallowSeat { user: String, seat: String },

    /// Show YOUR effective role: the roster role for your user id if present,
    /// else `AIDA_SESSION_ROLE`, else the default. Says where it came from.
    // trace:STORY-646 | ai:claude
    MyRole {
        /// Machine-readable JSON output.
        #[clap(long)]
        json: bool,
    },

    /// Remove a user's entry from the roster (`registry/team.toml`, CAS push).
    /// A friendly no-op if the user isn't present. Use this to clean stray /
    /// duplicate keys — e.g. an orphaned integer key left by an older roster.
    // trace:STORY-654 | ai:claude
    UnsetRole {
        /// The user id to remove (the person key — matches `current_user_id`,
        /// `$AIDA_USER` / `$USER`; may also be a stray integer key).
        user: String,
    },
}

#[cfg(test)]
mod task_190_comment_source_parser_tests {
    use super::Cli;
    use clap::Parser;

    // trace:BUG-1534 | ai:claude
    #[test]
    fn comment_add_accepts_relayed_from() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "comment",
            "add",
            "TASK-1",
            "tally is 902",
            "--relayed-from",
            "claude-reviewer-1",
        ])
        .unwrap();
        match cli.command.registered::<crate::comment_cmd::CommentArgs>() {
            crate::comment_cmd::CommentArgs {
                inner: super::CommentCommand::Add { relayed_from, .. },
                ..
            } => {
                assert_eq!(relayed_from.as_deref(), Some("claude-reviewer-1"))
            }
            other => panic!("unexpected parse: {other:?}"),
        }
    }

    #[test]
    fn positional_comment_conflicts_with_body_file_and_stdin() {
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "comment",
            "add",
            "TASK-1",
            "positional",
            "--body-file",
            "body.md",
        ])
        .is_err());
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "comment",
            "add",
            "TASK-1",
            "positional",
            "--stdin",
        ])
        .is_err());
    }

    #[test]
    fn hidden_content_conflicts_with_body_file_and_stdin() {
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "comment",
            "add",
            "TASK-1",
            "positional",
            "--content",
            "legacy",
        ])
        .is_err());
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "comment",
            "add",
            "TASK-1",
            "--content",
            "legacy",
            "--body-file",
            "body.md",
        ])
        .is_err());
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "comment",
            "add",
            "TASK-1",
            "--content",
            "legacy",
            "--stdin",
        ])
        .is_err());
    }

    #[test]
    fn interactive_conflicts_with_every_noninteractive_source() {
        for tail in [
            vec!["positional"],
            vec!["--content", "legacy"],
            vec!["--body-file", "body.md"],
            vec!["--stdin"],
        ] {
            let mut args = vec!["aida", "comment", "add", "TASK-1"];
            args.extend(tail);
            args.push("--interactive");
            assert!(crate::command_registry::try_parse_from(args).is_err());
        }

        for tail in [
            vec!["--content", "replacement"],
            vec!["--body-file", "body.md"],
            vec!["--stdin"],
        ] {
            let mut args = vec![
                "aida",
                "comment",
                "edit",
                "--req-id",
                "TASK-1",
                "--comment-id",
                "abc",
            ];
            args.extend(tail);
            args.push("--interactive");
            assert!(crate::command_registry::try_parse_from(args).is_err());
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    // A command owned by a module in `command_registry::REGISTRY`, parsed by
    // `command_registry::parse_from`; clap never derives it.
    // trace:STORY-1728 | ai:claude
    #[clap(skip)]
    Registered(crate::command_registry::Invocation),
}

/// Stock and local skill tooling.
#[derive(Subcommand, Debug)]
pub enum SkillCommand {
    /// Render a stock skill merged with its optional local override (.claude/skills/<name>.md + .local.md) to stdout.
    Render {
        /// The name of the skill to render (e.g. `agent-contract`).
        name: String,
    },

    /// Lint skills that reference an implementation plan. Scans every skill
    /// under `.claude/skills/` for plan references (`docs/plans/*.md`
    /// paths); for each skill that references a plan it runs the same
    /// checks `aida plan verify` runs on each referenced plan (drifted
    /// refs / missing files / absent sections) and a raw-glyph check on
    /// the skill body. A skill pointing at a drifted or missing plan is
    /// an error. Read-only; never rewrites. Exits non-zero on any error so
    /// it can run as a CI gate.
    // trace:TASK-927 | ai:claude
    Lint {
        /// Lint a single skill file instead of the whole `.claude/skills/`
        /// tree. Path may be absolute or relative to the project root.
        #[clap(value_name = "SKILL")]
        skill: Option<PathBuf>,

        /// Emit the result as JSON for agents / scripts.
        #[clap(long)]
        json: bool,

        /// Suppress per-skill OK lines; print only warnings, errors, and
        /// the final verdict. Useful for CI logs.
        #[clap(long, short = 'q')]
        quiet: bool,
    },
}

/// Parse a duration threshold into whole milliseconds, accepting an optional
/// unit suffix. The `--slower-than` flag's help text is `Nms`, which invites
/// users to type `500ms`; bare `500` (the historical form) is still accepted,
/// and `2s` is sugar for `2000`.
///
/// Accepted: `500`, `500ms`, `2s` (case-insensitive, surrounding whitespace
/// trimmed). A bare number is interpreted as milliseconds.
// trace:TASK-1055 — plain `//` so the SPEC-ID stays out of `--help` output.
pub fn parse_duration_ms(raw: &str) -> Result<u64, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("expected a duration like 500, 500ms, or 2s".to_string());
    }
    let lower = s.to_ascii_lowercase();
    let (digits, multiplier) = if let Some(num) = lower.strip_suffix("ms") {
        (num.trim_end(), 1u64)
    } else if let Some(num) = lower.strip_suffix('s') {
        (num.trim_end(), 1000u64)
    } else {
        (lower.as_str(), 1u64)
    };
    let value: u64 = digits.parse().map_err(|_| {
        format!("invalid duration '{raw}' — expected a number optionally suffixed with ms or s (e.g. 500, 500ms, 2s)")
    })?;
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("duration '{raw}' overflows the millisecond range"))
}

// trace:BUG-227 | ai:claude
#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    // BUG-1141: the global `--format` (Option<OutputFormat>, global=true)
    // propagates into every subcommand. The `digest` subcommand had ALSO
    // defined `format: DigestFormat` — two `--format` args of different types
    // reached `digest`, and clap panicked on parse ("Mismatch between
    // definition and access of `format`"). debug_assert() validates the whole
    // arg tree for exactly this class of definition conflict; the parse tests
    // reproduce the exact crash path. trace:BUG-1141 | ai:claude
    #[test]
    fn cli_arg_tree_has_no_definition_conflicts() {
        crate::command_registry::built_command().debug_assert();
    }

    // trace:STORY-1693 | ai:claude
    #[test]
    fn session_handoff_standing_orders_parses_add_and_list() {
        let so = |args: &[&str]| -> Option<Vec<String>> {
            let cli = crate::command_registry::try_parse_from(args).unwrap();
            match cli.command.registered::<crate::agent_launch::SessionArgs>() {
                crate::agent_launch::SessionArgs {
                    inner:
                        SessionCommand::Handoff {
                            standing_orders, ..
                        },
                    ..
                } => standing_orders,
                _ => panic!("expected session handoff"),
            }
        };
        let base = ["aida", "session", "handoff", "--seat", "advisor"];
        let add = [&base[..], &["--standing-orders", "add", "-never page"]].concat();
        assert_eq!(
            so(&add),
            Some(vec!["add".to_string(), "-never page".to_string()])
        );
        let list = [&base[..], &["--standing-orders", "list"]].concat();
        assert_eq!(so(&list), Some(vec!["list".to_string()]));
        let both = [&base[..], &["--show", "--standing-orders", "list"]].concat();
        assert!(crate::command_registry::try_parse_from(both).is_err());
    }

    #[test]
    fn digest_parses_without_format_collision() {
        // Previously panicked at parse. Bare invocation + the renamed flag.
        assert!(crate::command_registry::try_parse_from(["aida", "digest"]).is_ok());
        let cli =
            crate::command_registry::try_parse_from(["aida", "digest", "--digest-format", "json"])
                .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::digest_cmd::DigestArgs>(),
            crate::digest_cmd::DigestArgs {
                digest_format: crate::digest::DigestFormat::Json,
                ..
            }
        ));
        // The global --format still parses alongside the digest subcommand.
        assert!(
            crate::command_registry::try_parse_from(["aida", "--format", "json", "digest"]).is_ok()
        );
    }

    // trace:TASK-1055 | ai:claude — `--slower-than` accepts a bare number
    // (ms), an explicit `ms` suffix, or an `s` suffix (seconds → ms). The
    // help text reads `Nms`, which used to reject `500ms` with "invalid
    // digit found in string".
    #[test]
    fn parse_duration_ms_accepts_bare_and_suffixed() {
        assert_eq!(parse_duration_ms("500"), Ok(500));
        assert_eq!(parse_duration_ms("500ms"), Ok(500));
        assert_eq!(parse_duration_ms("2s"), Ok(2000));
        // case-insensitive + surrounding whitespace tolerated
        assert_eq!(parse_duration_ms(" 750MS "), Ok(750));
        assert_eq!(parse_duration_ms("0"), Ok(0));
        // garbage and bare units are rejected with a helpful message
        assert!(parse_duration_ms("fast").is_err());
        assert!(parse_duration_ms("ms").is_err());
        assert!(parse_duration_ms("").is_err());
        assert!(parse_duration_ms("1.5s").is_err());
    }

    // trace:TASK-157 | ai:codex
    #[test]
    fn remote_mirror_push_hook_plumbing_parses() {
        let cli =
            crate::command_registry::try_parse_from(["aida", "remote", "mirror-push", "origin"])
                .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::remote_cmd::RemoteArgs>(),
            crate::remote_cmd::RemoteArgs { inner: RemoteCommand::MirrorPush { pushed_remote, dry_run, .. }, .. }
                if pushed_remote == "origin"
                    && !dry_run
        ));
    }

    // trace:TASK-1055 | ai:claude — the parser is actually wired onto the
    // clap flag, so `usage --slower-than 500ms events` parses to 500 (not a
    // parse error).
    #[test]
    fn slower_than_flag_accepts_ms_suffix() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "usage",
            "--slower-than",
            "500ms",
            "events",
        ])
        .expect("`--slower-than 500ms` must parse");
        match cli.command.registered::<crate::usage_cmd::UsageArgs>() {
            crate::usage_cmd::UsageArgs { slower_than, .. } => {
                assert_eq!(slower_than, Some(500));
            }
            other => panic!("expected Usage command, got {other:?}"),
        }
    }

    // trace:TASK-1228 | ai:codex
    #[test]
    fn autonomy_overview_parses_as_bare_command() {
        let cli = crate::command_registry::try_parse_from(["aida", "autonomy"])
            .expect("bare `aida autonomy` should print the overview");
        assert!(matches!(
            cli.command
                .registered::<crate::autonomy_cmd::AutonomyArgs>(),
            crate::autonomy_cmd::AutonomyArgs { command: None, .. }
        ));

        let cli = crate::command_registry::try_parse_from(["aida", "autonomy", "report"]).unwrap();
        assert!(matches!(
            cli.command
                .registered::<crate::autonomy_cmd::AutonomyArgs>(),
            crate::autonomy_cmd::AutonomyArgs {
                command: Some(AutonomyCommand::Report { .. }),
                ..
            }
        ));
    }

    // trace:TASK-1209 | ai:codex
    #[test]
    fn drain_tail_parses_same_streaming_flags_as_tail_drain() {
        let tail = crate::command_registry::try_parse_from([
            "aida",
            "tail",
            "drain",
            "--json",
            "--lines",
            "25",
            "--since",
            "10m",
            "--no-follow",
            "--with-tools",
            "--no-timestamp",
            "--annotate",
        ])
        .expect("`aida tail drain` should parse");
        let drain_tail = crate::command_registry::try_parse_from([
            "aida",
            "drain",
            "tail",
            "--json",
            "--lines",
            "25",
            "--since",
            "10m",
            "--no-follow",
            "--with-tools",
            "--no-timestamp",
            "--annotate",
        ])
        .expect("`aida drain tail` should parse");

        let crate::tail_cmd::TailArgs {
            target,
            list,
            json,
            lines,
            since,
            no_follow,
            with_tools,
            no_timestamp,
            annotate,
            ..
        } = tail.command.registered::<crate::tail_cmd::TailArgs>()
        else {
            panic!("expected Tail command");
        };
        assert_eq!(target.as_deref(), Some("drain"));
        assert!(!list);

        let crate::drain_cmd::DrainArgs {
            inner:
                DrainCommand::Tail {
                    json: drain_json,
                    lines: drain_lines,
                    since: drain_since,
                    no_follow: drain_no_follow,
                    with_tools: drain_with_tools,
                    no_timestamp: drain_no_timestamp,
                    annotate: drain_annotate,
                    ..
                },
            ..
        } = drain_tail
            .command
            .registered::<crate::drain_cmd::DrainArgs>()
        else {
            panic!("expected Drain::Tail command");
        };

        assert_eq!(json, drain_json);
        assert_eq!(lines, drain_lines);
        assert_eq!(since, drain_since);
        assert_eq!(no_follow, drain_no_follow);
        assert_eq!(with_tools, drain_with_tools);
        assert_eq!(no_timestamp, drain_no_timestamp);
        assert!(annotate);
        assert_eq!(annotate, drain_annotate);
    }

    // trace:STORY-1130 | ai:codex
    #[test]
    fn drain_start_and_stop_parse_lifecycle_flags() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "drain",
            "start",
            "next3",
            "--project",
            "/tmp/aida",
            "--no-human",
            "both",
            "--role",
            "implementer",
            "--max-failures",
            "2",
            "--max-iterations",
            "3",
            "--max-runtime",
            "45m",
            "--max-tokens",
            "1000",
            "--vendor",
            "codex",
            "--model",
            "gpt-5",
            "--json",
        ])
        .expect("`aida drain start` should parse");
        match cli.command.registered::<crate::drain_cmd::DrainArgs>() {
            crate::drain_cmd::DrainArgs {
                inner:
                    DrainCommand::Start {
                        selector,
                        project,
                        batch,
                        once,
                        no_human,
                        role,
                        max_failures,
                        max_iterations,
                        max_runtime,
                        max_tokens,
                        json,
                        vendor,
                        model,
                        ..
                    },
                ..
            } => {
                assert_eq!(selector.as_deref(), Some("next3"));
                assert_eq!(project.as_deref(), Some("/tmp/aida"));
                assert_eq!(batch, None);
                assert!(!once);
                assert_eq!(no_human.as_deref(), Some("both"));
                assert_eq!(role.as_deref(), Some("implementer"));
                assert_eq!(max_failures, Some(2));
                assert_eq!(max_iterations, Some(3));
                assert_eq!(max_runtime.as_deref(), Some("45m"));
                assert_eq!(max_tokens, Some(1000));
                assert!(json);
                assert_eq!(vendor.as_deref(), Some("codex"));
                assert_eq!(model.as_deref(), Some("gpt-5"));
            }
            other => panic!("expected Drain::Start command, got {other:?}"),
        }

        let cli =
            crate::command_registry::try_parse_from(["aida", "drain", "stop", "--now"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::drain_cmd::DrainArgs>(),
            crate::drain_cmd::DrainArgs {
                inner: DrainCommand::Stop { now: true, .. },
                ..
            }
        ));
    }

    // trace:TASK-1155 trace:ADR-11 | ai:codex
    #[test]
    fn do_command_parses_spec_positional() {
        let cli = crate::command_registry::try_parse_from(["aida", "do", "TASK-1155"])
            .expect("`aida do <spec>` should parse");
        match cli.command.registered::<crate::do_dispatch::DoArgs>() {
            crate::do_dispatch::DoArgs {
                spec, mode, force, ..
            } => {
                assert_eq!(spec, "TASK-1155");
                assert!(mode.is_none());
                assert!(!force);
            }
            other => panic!("expected Do command, got {other:?}"),
        }
    }

    // trace:STORY-776 | ai:claude
    #[test]
    fn do_command_parses_mode_override_and_force() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "do",
            "TASK-1155",
            "--mode",
            "guided",
            "--force",
        ])
        .expect("`aida do <spec> --mode <m> --force` should parse");
        match cli.command.registered::<crate::do_dispatch::DoArgs>() {
            crate::do_dispatch::DoArgs {
                spec, mode, force, ..
            } => {
                assert_eq!(spec, "TASK-1155");
                assert_eq!(mode.as_deref(), Some("guided"));
                assert!(force);
            }
            other => panic!("expected Do command, got {other:?}"),
        }
    }

    // trace:TASK-1145 | ai:codex
    #[test]
    fn worktree_gc_parses_destructive_heal_flags() {
        let cli = crate::command_registry::try_parse_from([
            "aida", "worktree", "gc", "--yes", "--force", "--json",
        ])
        .expect("`aida worktree gc` should parse");
        match cli.command.registered::<crate::worktree_gc::WorktreeArgs>() {
            crate::worktree_gc::WorktreeArgs {
                inner:
                    WorktreeCommand::Gc {
                        yes, force, json, ..
                    },
                ..
            } => {
                assert!(yes);
                assert!(force);
                assert!(json);
            }
            other => panic!("expected Worktree::Gc command, got {other:?}"),
        }
    }

    // True when `s` contains a real AIDA trace marker — the literal `trace:`
    // token followed by an uppercase SPEC-ID prefix, a dash, and a digit (e.g.
    // STORY-62, TASK-1-021). Legit help-text mentions of the trace machinery
    // use placeholders (REQ-ID, <SPEC-ID>) with no digit after the dash, so
    // this discriminates leaked breadcrumbs from prose.
    fn has_trace_marker(s: &str) -> bool {
        let mut search_from = 0;
        while let Some(pos) = s[search_from..].find("trace:") {
            let after = search_from + pos + "trace:".len();
            let rest = &s[after..];
            let prefix_len = rest.bytes().take_while(u8::is_ascii_uppercase).count();
            let tail = &rest[prefix_len..];
            if prefix_len > 0
                && tail
                    .strip_prefix('-')
                    .and_then(|t| t.chars().next())
                    .is_some_and(|c| c.is_ascii_digit())
            {
                return true;
            }
            search_from = after;
        }
        false
    }

    // Walk the clap command tree, recording every help/about string that
    // carries a leaked SPEC-ID trace marker.
    fn collect_leaks(cmd: &clap::Command, path: &str, leaks: &mut Vec<String>) {
        let here = if path.is_empty() {
            cmd.get_name().to_string()
        } else {
            format!("{path} {}", cmd.get_name())
        };
        let mut check = |label: &str, text: Option<String>| {
            if let Some(t) = text {
                if has_trace_marker(&t) {
                    leaks.push(format!("{here} :: {label}: {t}"));
                }
            }
        };
        check("about", cmd.get_about().map(ToString::to_string));
        check("long_about", cmd.get_long_about().map(ToString::to_string));
        check(
            "before_help",
            cmd.get_before_help().map(ToString::to_string),
        );
        check("after_help", cmd.get_after_help().map(ToString::to_string));
        for arg in cmd.get_arguments() {
            let id = arg.get_id();
            check(
                &format!("--{id} help"),
                arg.get_help().map(ToString::to_string),
            );
            check(
                &format!("--{id} long_help"),
                arg.get_long_help().map(ToString::to_string),
            );
        }
        for sub in cmd.get_subcommands() {
            collect_leaks(sub, &here, leaks);
        }
    }

    #[test]
    fn trace_marker_detector_discriminates_markers_from_prose() {
        assert!(has_trace_marker("limit allows. trace:STORY-59 | ai:claude"));
        assert!(has_trace_marker("trace:TASK-1-021"));
        assert!(has_trace_marker("see trace:BUG-87 here"));
        // Legit help-text mentions — placeholders with no digit after the dash.
        assert!(!has_trace_marker(
            "Scan source files for trace comments (// trace:REQ-ID format)"
        ));
        assert!(!has_trace_marker(
            "Walk the project root for `trace:<SPEC-ID>`"
        ));
        assert!(!has_trace_marker("remove `trace:<DANGLING>` annotations"));
        assert!(!has_trace_marker("no marker here at all"));
    }

    // Regression test for BUG-227: SPEC-IDs are developer breadcrumbs and must
    // never reach `--help`. A `///` doc comment on a clap field/variant doubles
    // as help text, so any trace marker on one leaks — keep it a plain `//`.
    // trace:STORY-1728.ac5ac281 | ai:claude
    #[test]
    fn help_text_carries_no_trace_markers() {
        let mut leaks = Vec::new();
        collect_leaks(&crate::command_registry::built_command(), "", &mut leaks);
        assert!(
            leaks.is_empty(),
            "SPEC-ID trace markers leaked into `--help` output — demote the \
             offending `///` doc comment to a plain `//` comment:\n  {}",
            leaks.join("\n  ")
        );
    }

    // Source-side hygiene check: no `///` doc comment in this file may contain
    // the literal `trace:` token. Catches potential leaks before clap builds
    // the tree (covers hidden subcommands, enum-level docs, even commented-out
    // `///` blocks that the runtime walker can't see). Pairs with
    // `help_text_carries_no_trace_markers` — runtime + source, belt + braces.
    #[test]
    fn source_doc_comments_carry_no_trace_token() {
        let src = include_str!("cli.rs");
        let offenders: Vec<(usize, &str)> = src
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                let trimmed = line.trim_start();
                trimmed.starts_with("///") && trimmed.contains("trace:")
            })
            .map(|(i, line)| (i + 1, line))
            .collect();
        assert!(
            offenders.is_empty(),
            "`///` doc comments in cli.rs must not contain the `trace:` token \
             (placeholders or real markers — both leak into `--help` output \
             when on a clap field). Demote to a plain `//` line above the \
             item, or reword the prose to drop the literal token:\n{}",
            offenders
                .iter()
                .map(|(n, l)| format!("  {n}: {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    // TASK-287 / BUG-629 / TASK-1516: a `///` clap doc comment doubles as
    // `--help` text, so a SPEC-ID on one — provenance (a `trace:` marker, or a
    // bare SPEC-ID standing in for the developer breadcrumb) OR a *descriptive*
    // mention inside prose (e.g. "reuses the STORY-122 usage log") — leaks an
    // internal id into user-facing output either way. BUG-629 had allowed the
    // prose case (over-correcting the original "any SPEC-ID token" rule); BUG-1652
    // found ~20 real leaks that carve-out let through, so TASK-1516 removed it —
    // a SPEC-ID on a `///` line is rejected full stop, with a per-line
    // `PROSE_SPEC_ID_ALLOWLIST` opt-out for the rare deliberately developer-/
    // operator-facing case. The discriminator is `doc_comment_is_provenance_leak`.
    // This stricter prose rule is deliberately CI-only and cli.rs-only: the
    // pre-commit hook template (TASK-903) ships to every scaffolded project and
    // scans every `*.rs` file, where ordinary rustdoc legitimately cites a
    // project's own ids, so the hook keeps the narrower BUG-629 criterion (bare
    // ids and `trace:` markers only). The two share the id pattern
    // `SPEC_ID_PATTERN` verbatim. (This comment uses `//`, not `///`, so it
    // isn't itself scanned.)
    // The clap surface spans `cli.rs`, each command's `<module>_args.rs`, and
    // the clap-derived types anywhere else below `src/` (STORY-1728), so the
    // guard scans `cli.rs` and `*_args.rs` whole and, in every other non-test
    // source file, the doc comments above and inside each clap derive.
    // trace:STORY-1728 | ai:claude
    #[test]
    fn source_doc_comments_carry_no_spec_id_provenance() {
        fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            entries.sort();
            for path in entries {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                if path.is_dir() {
                    if name != "tests" {
                        sources(&path, out);
                    }
                } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
                    out.push(path);
                }
            }
        }
        const CLAP_DERIVES: &[&str] = &["Parser", "Args", "Subcommand", "ValueEnum"];
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut paths = Vec::new();
        sources(&src_dir, &mut paths);
        let mut surface: Vec<(String, usize, String)> = Vec::new();
        for path in paths {
            let name = path.strip_prefix(&src_dir).unwrap().display().to_string();
            let text = std::fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            if name == "cli.rs" || name.ends_with("_args.rs") {
                for (i, l) in lines.iter().enumerate() {
                    surface.push((name.clone(), i + 1, l.to_string()));
                }
                continue;
            }
            for (i, l) in lines.iter().enumerate() {
                let trimmed = l.trim_start();
                if !trimmed.starts_with("#[derive(") {
                    continue;
                }
                // rustfmt wraps long derives over several lines; join until
                // the closing `)]` as `command_clap_types` does.
                // trace:TASK-1953 | ai:claude
                let mut joined = trimmed.to_string();
                let mut j = i;
                while !joined.contains(")]") && j + 1 < lines.len() {
                    j += 1;
                    joined.push_str(lines[j]);
                }
                let derive = joined.split(")]").next().unwrap_or_default();
                let is_clap = derive
                    .trim_start_matches("#[derive(")
                    .split(',')
                    .map(|d| d.trim().trim_start_matches("clap::"))
                    .any(|d| CLAP_DERIVES.contains(&d));
                if !is_clap {
                    continue;
                }
                let indent = &l[..l.len() - trimmed.len()];
                let mut start = i;
                while start > 0 && lines[start - 1].trim_start().starts_with("//") {
                    start -= 1;
                }
                let close = format!("{indent}}}");
                let mut end = i;
                while end < lines.len()
                    && lines[end] != close
                    && !lines[end].ends_with("{}")
                    && !(end > i && lines[end].starts_with(indent) && lines[end].ends_with(';'))
                {
                    end += 1;
                }
                for k in start..=end.min(lines.len() - 1) {
                    surface.push((name.clone(), k + 1, lines[k].to_string()));
                }
            }
        }
        let offenders: Vec<(String, &str)> = surface
            .iter()
            .filter(|(_, _, line)| {
                let trimmed = line.trim_start();
                trimmed.starts_with("///") && doc_comment_is_provenance_leak(trimmed)
            })
            .map(|(file, n, line)| (format!("{file}:{n}"), line.as_str()))
            .collect();
        assert!(
            offenders.is_empty(),
            "`///` clap doc comments must not carry a SPEC-ID — bare, in a \
             `trace:` marker, or mentioned in descriptive prose — because clap \
             pulls `///` into `--help` (TASK-1516). Move the id to a `//` trace \
             marker above the item, reword the prose without it, or (rare) add \
             the exact trimmed line to `PROSE_SPEC_ID_ALLOWLIST` when the help \
             is deliberately developer-/operator-facing:\n{}",
            offenders
                .iter()
                .map(|(n, l)| format!("  {n}: {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    // TASK-1516: exact, trimmed `///` lines that MAY carry a SPEC-ID because the
    // help they produce is deliberately developer-/operator-facing (not aimed at
    // an ordinary end user) and reviewed as such. Empty today — TASK-1516's sweep
    // reworded every prose-embedded id the initial audit found instead of
    // allowlisting it. Add a line here only for a genuine, reviewed exception;
    // prefer rewording first.
    const PROSE_SPEC_ID_ALLOWLIST: &[&str] = &[];

    // BUG-629 / TASK-903 / TASK-1516: the discriminator shared by the CI
    // provenance test above and the grep-based pre-commit hook. `line` is a
    // `///`-prefixed doc line (already trimmed). It is a *provenance leak* — and
    // so rejected — whenever it carries a SPEC-ID at all (bare, `trace:`-marked,
    // or mentioned in descriptive prose), UNLESS the exact trimmed line is in
    // `PROSE_SPEC_ID_ALLOWLIST`.
    //
    // The hook's `__aida_doc_is_provenance_leak` intentionally does NOT mirror
    // this prose rule (see the test comment above); only `SPEC_ID_PATTERN` is
    // shared with it.
    fn doc_comment_is_provenance_leak(line: &str) -> bool {
        doc_comment_is_provenance_leak_against(line, PROSE_SPEC_ID_ALLOWLIST)
    }

    // Same discriminator, parameterized on the allowlist so tests can exercise
    // the carve-out without mutating the shared `PROSE_SPEC_ID_ALLOWLIST` const.
    // TASK-903 / TASK-1516: the SPEC-ID pattern, byte-for-byte identical to
    // `SPEC_ID_RE` in aida-core/templates/hooks/aida-pre-commit.sh (same
    // prefixes, same explicit leading word boundary — POSIX ERE has no `\b`).
    // The boundary keeps `DEBUG-2` / `SCR-4` from matching as `BUG-2` / `CR-4`.
    // Change both together; `spec_id_pattern_matches_the_hook_template` checks.
    const SPEC_ID_PATTERN: &str =
        r"(^|[^A-Za-z0-9_])(STORY|TASK|BUG|EPIC|SPIKE|FR|CR|SPEC|ADR|PRIN|DOC)-[0-9]+";

    fn doc_comment_is_provenance_leak_against(line: &str, allowlist: &[&str]) -> bool {
        let spec_id = regex::Regex::new(SPEC_ID_PATTERN).unwrap();
        // No SPEC-ID at all → nothing to leak.
        if !spec_id.is_match(line) {
            return false;
        }
        // An explicit, reviewed carve-out for deliberately developer-/
        // operator-facing help that needs to name a real SPEC-ID.
        if allowlist.contains(&line.trim()) {
            return false;
        }
        // Any other SPEC-ID on a `///` line — bare, `trace:`-marked, or inside
        // descriptive prose — leaks into `--help`. Reject.
        true
    }

    // BUG-629 / TASK-1516: every SPEC-ID on a `///` line is rejected — bare,
    // `trace:`-marked, or embedded in otherwise-legitimate prose — unless the
    // line is explicitly allowlisted.
    #[test]
    fn doc_comment_provenance_leak_discriminates_provenance_from_prose() {
        // REJECT — trace: marker on a `///` line (the --help-leak trap).
        assert!(doc_comment_is_provenance_leak("/// trace:TASK-955"));
        assert!(doc_comment_is_provenance_leak(
            "/// resolve the path. trace:BUG-448 | ai:claude"
        ));
        // REJECT — bare SPEC-ID provenance, no surrounding prose.
        assert!(doc_comment_is_provenance_leak("/// TASK-955"));
        assert!(doc_comment_is_provenance_leak("/// BUG-448"));
        assert!(doc_comment_is_provenance_leak("/// (STORY-122, TASK-903)"));
        assert!(doc_comment_is_provenance_leak("/// FR-0042"));

        // REJECT (TASK-1516) — a descriptive prose mention of a SPEC-ID still
        // leaks the internal id into `--help`, even though it reads as normal
        // help text. This is the case BUG-629 used to allow and BUG-1652 found
        // ~20 real instances of.
        assert!(doc_comment_is_provenance_leak(
            "/// reuses the STORY-122 usage log"
        ));
        assert!(doc_comment_is_provenance_leak(
            "/// reads the SPIKE-67 field-study log"
        ));
        assert!(doc_comment_is_provenance_leak(
            "/// (e.g. `TASK-489` — treated as `--spec`)"
        ));
        assert!(doc_comment_is_provenance_leak(
            "/// Spec the verdict applies to (e.g. TASK-5)."
        ));

        // REJECT — DOC is a spec prefix too.
        assert!(doc_comment_is_provenance_leak("/// see DOC-3"));

        // ALLOW — no SPEC-ID at all.
        assert!(!doc_comment_is_provenance_leak("/// Mark a spec done"));
        // ALLOW — an id-shaped substring of a longer word is not a SPEC-ID
        // (the leading word boundary).
        assert!(!doc_comment_is_provenance_leak(
            "/// Log at DEBUG-2 verbosity"
        ));
        assert!(!doc_comment_is_provenance_leak(
            "/// Handles the SCR-4 screen"
        ));
    }

    // TASK-903 / TASK-1516: the CI guard and the pre-commit hook template must
    // share one SPEC-ID pattern so they agree on what counts as an id.
    #[test]
    fn spec_id_pattern_matches_the_hook_template() {
        let hook = include_str!("../../aida-core/templates/hooks/aida-pre-commit.sh");
        let expected = format!("SPEC_ID_RE='{SPEC_ID_PATTERN}'");
        assert!(
            hook.lines().any(|l| l.trim() == expected),
            "aida-pre-commit.sh must define `{expected}` to stay in lockstep \
             with the cli.rs guard"
        );
    }

    // TASK-1516: an explicitly allowlisted line is excused even though it
    // carries a SPEC-ID (the reviewed developer-/operator-facing carve-out);
    // an unlisted line with the same SPEC-ID is still rejected.
    #[test]
    fn doc_comment_provenance_leak_honors_the_allowlist() {
        let allowlist: &[&str] = &["/// deliberately developer-facing: STORY-1"];
        assert!(!doc_comment_is_provenance_leak_against(
            "/// deliberately developer-facing: STORY-1",
            allowlist
        ));
        // A different SPEC-ID on an otherwise-identical line is NOT excused —
        // the allowlist matches the exact trimmed line, not a pattern.
        assert!(doc_comment_is_provenance_leak_against(
            "/// deliberately developer-facing: STORY-2",
            allowlist
        ));
        // Leading/trailing whitespace differences still match (line is
        // trimmed before comparison).
        assert!(!doc_comment_is_provenance_leak_against(
            "   /// deliberately developer-facing: STORY-1   ",
            allowlist
        ));
        // The real, shared allowlist is empty today (TASK-1516 reworded every
        // known occurrence instead of allowlisting it), so nothing is excused
        // through it right now.
        assert!(PROSE_SPEC_ID_ALLOWLIST.is_empty());
    }

    // trace:TASK-0415 — the positional status shortcut parses into the List
    // command's `shortcut` field; `--status` still parses into `status`.
    #[test]
    fn list_positional_status_shortcut_parses() {
        let cli = crate::command_registry::try_parse_from(["aida", "list", "approved"]).unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs {
                shortcut, status, ..
            } => {
                assert_eq!(shortcut.as_deref(), Some("approved"));
                assert_eq!(status, None);
            }
            other => panic!("expected List, got {other:?}"),
        }

        let cli =
            crate::command_registry::try_parse_from(["aida", "list", "--status", "open"]).unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs {
                shortcut, status, ..
            } => {
                assert_eq!(shortcut, None);
                assert_eq!(status.as_deref(), Some("open"));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    // trace:BUG-1498 | ai:codex
    #[test]
    fn bug_1498_list_machine_drafts_flag_requires_explicit_draft_status() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "list",
            "--status",
            "draft",
            "--machine-drafts",
            "--short",
        ])
        .unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs {
                status,
                machine_drafts,
                short,
                ..
            } => {
                assert_eq!(status.as_deref(), Some("draft"));
                assert!(machine_drafts);
                assert!(short);
            }
            other => panic!("expected List, got {other:?}"),
        }
        assert!(
            crate::command_registry::try_parse_from(["aida", "list", "--machine-drafts"]).is_err()
        );
    }

    // trace:STORY-662 — the `--user <name>` flag parses, and the positional
    // `me` / `user:<name>` tokens land in `shortcut` (peeled into the user
    // filter at runtime, not at the clap layer).
    #[test]
    fn list_user_flag_and_positional_parse() {
        // --user flag.
        let cli =
            crate::command_registry::try_parse_from(["aida", "list", "--user", "joe"]).unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs { user, shortcut, .. } => {
                assert_eq!(user.as_deref(), Some("joe"));
                assert_eq!(shortcut, None);
            }
            other => panic!("expected List, got {other:?}"),
        }

        // Positional `me`.
        let cli = crate::command_registry::try_parse_from(["aida", "list", "me"]).unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs { shortcut, user, .. } => {
                assert_eq!(shortcut.as_deref(), Some("me"));
                assert_eq!(user, None);
            }
            other => panic!("expected List, got {other:?}"),
        }

        // Positional `user:<name>`.
        let cli = crate::command_registry::try_parse_from(["aida", "list", "user:alice"]).unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs { shortcut, user, .. } => {
                assert_eq!(shortcut.as_deref(), Some("user:alice"));
                assert_eq!(user, None);
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    // TASK-955: `--recursive` parses only WITH `--parent` (clap `requires`),
    // composes with the positional [STATUS], and the `--subtree` alias resolves
    // to the same flag. trace:TASK-955
    #[test]
    fn list_recursive_requires_parent() {
        // --recursive + --parent + a status positional: parses, recursive=true.
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "list",
            "open",
            "--parent",
            "EPIC-51",
            "--recursive",
        ])
        .unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs {
                shortcut,
                parent,
                recursive,
                ..
            } => {
                assert_eq!(shortcut.as_deref(), Some("open"));
                assert_eq!(parent.as_deref(), Some("EPIC-51"));
                assert!(recursive);
            }
            other => panic!("expected List, got {other:?}"),
        }

        // The `--subtree` alias resolves to the same `recursive` field.
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "list",
            "--parent",
            "EPIC-51",
            "--subtree",
        ])
        .unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs { recursive, .. } => assert!(recursive),
            other => panic!("expected List, got {other:?}"),
        }

        // --recursive WITHOUT --parent errors (the requires-parent guard).
        let err =
            crate::command_registry::try_parse_from(["aida", "list", "--recursive"]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--parent"),
            "error should name the missing --parent requirement: {msg}"
        );
    }

    // trace:STORY-562 — `aida list human` parses as the positional shortcut and
    // `aida list --human` sets the flag; both route to the human-attention view.
    #[test]
    fn list_human_positional_and_flag_parse() {
        let cli = crate::command_registry::try_parse_from(["aida", "list", "human"]).unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs {
                shortcut, human, ..
            } => {
                assert_eq!(shortcut.as_deref(), Some("human"));
                assert!(!human);
            }
            other => panic!("expected List, got {other:?}"),
        }

        let cli = crate::command_registry::try_parse_from(["aida", "list", "--human"]).unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs {
                shortcut, human, ..
            } => {
                assert_eq!(shortcut, None);
                assert!(human);
            }
            other => panic!("expected List, got {other:?}"),
        }

        // Composes with `--short`.
        let cli =
            crate::command_registry::try_parse_from(["aida", "list", "human", "--short"]).unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs {
                shortcut,
                human,
                short,
                ..
            } => {
                assert_eq!(shortcut.as_deref(), Some("human"));
                assert!(!human);
                assert!(short);
            }
            other => panic!("expected List, got {other:?}"),
        }

        // `--human` conflicts with `--status` / `--json` / `--tree` at the clap layer.
        assert!(
            crate::command_registry::try_parse_from(["aida", "list", "--human", "--json"]).is_err()
        );
        assert!(crate::command_registry::try_parse_from([
            "aida", "list", "--human", "--status", "draft"
        ])
        .is_err());
    }

    // trace:TASK-900 — `aida list --limit N` parses into the optional
    // `limit` field; absent it is None (full listing). The handler applies
    // the cap AFTER the recency-first sort, so N most-recent come back.
    #[test]
    fn list_limit_flag_parses() {
        // Absent → None (no cap).
        let cli = crate::command_registry::try_parse_from(["aida", "list"]).unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs { limit, .. } => assert_eq!(limit, None),
            other => panic!("expected List, got {other:?}"),
        }

        // Present → Some(N).
        let cli =
            crate::command_registry::try_parse_from(["aida", "list", "--limit", "5"]).unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs { limit, .. } => assert_eq!(limit, Some(5)),
            other => panic!("expected List, got {other:?}"),
        }

        // Composes with other filters.
        let cli = crate::command_registry::try_parse_from([
            "aida", "list", "--status", "open", "--limit", "3",
        ])
        .unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::ListArgs>()
        {
            crate::requirements_cli::ListArgs { limit, status, .. } => {
                assert_eq!(limit, Some(3));
                assert_eq!(status.as_deref(), Some("open"));
            }
            other => panic!("expected List, got {other:?}"),
        }

        // Non-numeric value is a parse error.
        assert!(
            crate::command_registry::try_parse_from(["aida", "list", "--limit", "notanumber"])
                .is_err()
        );
    }

    // trace:TASK-900 — the truncation semantics the list handler relies on:
    // `Vec::truncate(N)` keeps the FIRST N of the already-sorted vector (here a
    // recency-ordered stand-in, i.e. the N most-recent; the default list order
    // is natural ID since STORY-1644). `N >= len` is a no-op (full listing).
    #[test]
    fn list_limit_truncates_to_n_most_recent() {
        // Stand-in for the recency-first `reqs` vector (freshest first).
        let sorted = vec!["newest", "newer", "mid", "older", "oldest"];

        let mut got = sorted.clone();
        got.truncate(3);
        assert_eq!(got, vec!["newest", "newer", "mid"]);

        // limit >= len → unchanged.
        let mut all = sorted.clone();
        all.truncate(99);
        assert_eq!(all, sorted);

        // limit 0 → empty.
        let mut none = sorted.clone();
        none.truncate(0);
        assert!(none.is_empty());
    }

    // trace:TASK-770 | ai:codex
    #[test]
    fn human_presence_aliases_parse() {
        for (word, want) in [
            ("away", "away"),
            ("home", "home"),
            ("presence", "presence"),
            ("status", "status"),
        ] {
            let cli = crate::command_registry::try_parse_from(["aida", "human", word]).unwrap();
            match cli.command.registered::<crate::human_cmd::HumanArgs>() {
                crate::human_cmd::HumanArgs {
                    short: false,
                    command: Some(cmd),
                    ..
                } => {
                    let got = match cmd {
                        HumanCommand::Away => "away",
                        HumanCommand::Home => "home",
                        HumanCommand::Presence => "presence",
                        HumanCommand::Status => "status",
                        HumanCommand::Unblock { .. } => "unblock",
                        // trace:STORY-611 | ai:claude — action aliases.
                        HumanCommand::Answer { .. } => "answer",
                        HumanCommand::Decide { .. } => "decide",
                        HumanCommand::Review { .. } => "review",
                        // trace:STORY-768 | ai:claude — audit-pass trigger.
                        HumanCommand::Audit { .. } => "audit",
                        _ => "another verb",
                    };
                    assert_eq!(got, want);
                }
                other => panic!("expected human {word}, got {other:?}"),
            }
        }
    }

    // trace:STORY-818 | ai:codex
    #[test]
    fn human_review_guided_short_flag_parses() {
        let cli =
            crate::command_registry::try_parse_from(["aida", "human", "review", "TASK-123", "-g"])
                .unwrap();
        match cli.command.registered::<crate::human_cmd::HumanArgs>() {
            crate::human_cmd::HumanArgs {
                command: Some(HumanCommand::Review { spec, guided, .. }),
                ..
            } => {
                assert_eq!(spec, "TASK-123");
                assert!(guided);
            }
            other => panic!("expected human review --guided, got {other:?}"),
        }
    }

    // trace:TASK-1005 / SPIKE-70 — `--sequential` parses on `queue work`,
    // requires an autonomous drain, and conflicts with `--single-branch`.
    #[test]
    fn sequential_flag_parses_with_batch_and_autonomous() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "queue",
            "work",
            "--batch",
            "tui",
            "--auto-complete",
            "--sequential",
        ])
        .unwrap();
        match cli.command.registered::<crate::queue_cmd::QueueArgs>() {
            crate::queue_cmd::QueueArgs {
                inner:
                    QueueCommand::Work {
                        sequential,
                        single_branch,
                        batch,
                        ..
                    },
                ..
            } => {
                assert!(sequential, "--sequential should parse as true");
                assert!(!single_branch);
                assert_eq!(batch.as_deref(), Some("tui"));
            }
            other => panic!("expected queue work, got {other:?}"),
        }
    }

    // trace:TASK-1005 — `--sequential` is autonomous-only; without
    // `--auto-complete` (or `--drain`) clap rejects it via `requires`.
    #[test]
    fn sequential_requires_autonomous_drain() {
        let err = crate::command_registry::try_parse_from([
            "aida",
            "queue",
            "work",
            "--batch",
            "tui",
            "--sequential",
        ])
        .unwrap_err();
        assert!(
            err.to_string().contains("autonomous")
                || err.to_string().contains("auto-complete")
                || err.to_string().contains("required"),
            "expected a requires-autonomous error, got: {err}"
        );
    }

    // trace:TASK-1005 — `--sequential` (per-member PRs) and `--single-branch`
    // (one accumulating branch) are mutually exclusive drive shapes.
    #[test]
    fn sequential_conflicts_with_single_branch() {
        let err = crate::command_registry::try_parse_from([
            "aida",
            "queue",
            "work",
            "--batch",
            "tui",
            "--auto-complete",
            "--sequential",
            "--single-branch",
        ])
        .unwrap_err();
        assert!(
            err.to_string().contains("cannot be used with")
                || err.to_string().contains("single-branch"),
            "expected a conflicts-with error, got: {err}"
        );
    }

    // TASK-185: `--sequential` governs ORDER + per-member-PR shape, not
    // concurrency. The one-member-at-a-time property the flag's help text
    // promises comes from the drain's DEFAULT pipeline depth, so that is the
    // invariant worth pinning — the old `SEQUENTIAL_DRAIN_CONCURRENCY` const
    // asserted an engine property STORY-1091 made false.
    // trace:TASK-1005 trace:TASK-185 | ai:claude
    #[test]
    fn sequential_drain_is_one_at_a_time_via_default_pipeline_depth() {
        assert_eq!(crate::drain_state::default_pipeline_depth(), 1);
    }

    // TASK-185: the `--sequential` help text must not re-assert that
    // concurrency is pinned to 1 — STORY-1091 made `[drain] pipeline_depth`
    // live for the single-batch drain, so that sentence advertised a landed
    // feature as absent. This guard FORBIDS the old falsehood; it deliberately
    // does NOT mandate that the help name the knob at all, because an
    // unqualified depth claim is itself false: `--batches` chains run through
    // `drain_batch_chain_with_caps`, which is typed on the non-pipelined
    // `BatchDriver` and always calls `drain_batch_with_caps`, so the depth is
    // inert there. Naming the knob is therefore optional — but if the help
    // names it, the claim has to carry its scope.
    // trace:TASK-185 | ai:claude
    #[test]
    fn sequential_help_does_not_claim_concurrency_is_pinned() {
        let mut cmd = crate::command_registry::built_command();
        let help = find_subcommand_help(&mut cmd, &["queue", "work"]);
        // clap wraps long help at the terminal width, so flatten runs of
        // whitespace before matching any multi-word phrase — a line break in
        // the middle of a claim must not let it slip past this guard.
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            !flat.contains("pinned to 1"),
            "`aida queue work --help` still claims concurrency is pinned to 1; got:\n{help}"
        );
        if flat.contains("pipeline_depth") {
            assert!(
                flat.contains("chain ignores the depth"),
                "`aida queue work --help` names `[drain] pipeline_depth` without scoping it \
                 away from `--batches` chains, which are serial at any depth; got:\n{help}"
            );
        }
    }

    // BUG-1552: `aida doctor` is report-only by design — exit 0 means "ran
    // successfully," not "found nothing." That contract has to be
    // discoverable from --help itself, not just docs, so this pins that
    // the top-level `aida doctor --help` names the opt-in gate
    // (`--fail-on-findings`) a caller reaches for when it wants pass/fail.
    // trace:BUG-1552 | ai:claude
    #[test]
    fn doctor_help_names_fail_on_findings_gate() {
        let mut cmd = crate::command_registry::built_command();
        let help = find_subcommand_help(&mut cmd, &["doctor"]);
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            flat.contains("--fail-on-findings"),
            "`aida doctor --help` no longer names --fail-on-findings; got:\n{help}"
        );
    }

    // trace:TASK-200 | ai:codex
    #[test]
    fn agent_new_help_discloses_foreground_blocking_behavior() {
        let mut cmd = crate::command_registry::built_command();
        let help = find_subcommand_help(&mut cmd, &["agent", "new"]);
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            flat.contains("BLOCKS until") && flat.contains("does not exit when its turn ends"),
            "`aida agent new --help` no longer explains the foreground blocking behavior; got:\n{help}"
        );
    }

    // BUG-1920: `aida edit --type` must list `faq` among the accepted types.
    // Checks the `--type` arg's own help (not the whole subcommand help) so a
    // stray `faq` elsewhere in `aida edit --help` cannot satisfy it.
    // trace:BUG-1920 | ai:claude
    #[test]
    fn edit_type_help_lists_faq() {
        let cmd = crate::command_registry::built_command();
        let edit = cmd
            .get_subcommands()
            .find(|s| s.get_name() == "edit")
            .expect("`edit` subcommand exists");
        let type_arg = edit
            .get_arguments()
            .find(|a| a.get_long() == Some("type"))
            .expect("`aida edit` has a --type flag");
        let help = type_arg
            .get_long_help()
            .or_else(|| type_arg.get_help())
            .map(|h| h.to_string())
            .unwrap_or_default();
        let types: Vec<&str> = help
            .split(|c: char| c == ',' || c == ':' || c.is_whitespace())
            .map(str::trim)
            .collect();
        assert!(
            types.contains(&"faq"),
            "`aida edit --type` help no longer lists faq; got:\n{help}"
        );
    }

    /// Render the long help for a nested subcommand path (e.g. `queue work`).
    // trace:TASK-185 | ai:claude
    fn find_subcommand_help(cmd: &mut clap::Command, path: &[&str]) -> String {
        let mut cur = cmd.clone();
        for name in path {
            let next = cur
                .get_subcommands()
                .find(|s| s.get_name() == *name)
                .unwrap_or_else(|| panic!("subcommand `{name}` not found"))
                .clone();
            cur = next;
        }
        cur.render_long_help().to_string()
    }

    // trace:STORY-1028 | ai:codex
    #[test]
    fn mode_flag_subcommands_parse_next_to_hidden_aliases() {
        let cli = crate::command_registry::try_parse_from(["aida", "usage", "slowest"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::usage_cmd::UsageArgs>(),
            crate::usage_cmd::UsageArgs {
                action: Some(UsageCommand::Slowest),
                ..
            }
        ));
        let cli = crate::command_registry::try_parse_from(["aida", "usage", "--slowest"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::usage_cmd::UsageArgs>(),
            crate::usage_cmd::UsageArgs { slowest: true, .. }
        ));

        let cli = crate::command_registry::try_parse_from(["aida", "usage", "drains", "--pattern"])
            .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::usage_cmd::UsageArgs>(),
            crate::usage_cmd::UsageArgs {
                action: Some(UsageCommand::Drains { pattern: true, .. }),
                ..
            }
        ));
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "usage",
            "--auto-complete",
            "--pattern",
        ])
        .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::usage_cmd::UsageArgs>(),
            crate::usage_cmd::UsageArgs {
                auto_complete: true,
                pattern: true,
                ..
            }
        ));

        // TASK-1481: `timeline` is subcommand-only (no legacy `--timeline`
        // flag), and composes with the shared --cmd/--slower-than/--limit
        // flags the same way `events` does.
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "usage",
            "--cmd",
            "queue list",
            "--slower-than",
            "500ms",
            "timeline",
        ])
        .unwrap();
        match cli.command.registered::<crate::usage_cmd::UsageArgs>() {
            crate::usage_cmd::UsageArgs {
                action: Some(UsageCommand::Timeline),
                cmd,
                slower_than,
                ..
            } => {
                assert_eq!(cmd.as_deref(), Some("queue list"));
                assert_eq!(slower_than, Some(500));
            }
            other => panic!("expected Usage{{ action: Timeline, .. }}, got {other:?}"),
        }

        let cli =
            crate::command_registry::try_parse_from(["aida", "graph", "blocked-by", "STORY-1"])
                .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::graph_cmd::GraphArgs>(),
            crate::graph_cmd::GraphArgs {
                cmd: Some(GraphCommand::BlockedBy { .. }),
                ..
            }
        ));
        let cli =
            crate::command_registry::try_parse_from(["aida", "graph", "STORY-1", "--blocked-by"])
                .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::graph_cmd::GraphArgs>(),
            crate::graph_cmd::GraphArgs {
                id: Some(_),
                blocked_by: true,
                ..
            }
        ));

        let cli = crate::command_registry::try_parse_from(["aida", "release", "minor"]).unwrap();
        assert!(matches!(
            cli.command
                .registered::<crate::upgrade_release::ReleaseArgs>(),
            crate::upgrade_release::ReleaseArgs {
                cmd: Some(ReleaseCommand::Minor),
                ..
            }
        ));
        let cli = crate::command_registry::try_parse_from(["aida", "release", "--minor"]).unwrap();
        assert!(matches!(
            cli.command
                .registered::<crate::upgrade_release::ReleaseArgs>(),
            crate::upgrade_release::ReleaseArgs { minor: true, .. }
        ));

        let cli = crate::command_registry::try_parse_from(["aida", "focus", "clear"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::focus_cmd::FocusArgs>(),
            crate::focus_cmd::FocusArgs {
                cmd: Some(FocusCommand::Clear),
                ..
            }
        ));
        let cli = crate::command_registry::try_parse_from(["aida", "focus", "--clear"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::focus_cmd::FocusArgs>(),
            crate::focus_cmd::FocusArgs { clear: true, .. }
        ));

        let cli = crate::command_registry::try_parse_from(["aida", "drain", "clear"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::drain_cmd::DrainArgs>(),
            crate::drain_cmd::DrainArgs {
                inner: DrainCommand::Clear,
                ..
            }
        ));
        let cli = crate::command_registry::try_parse_from(["aida", "drain", "status", "--clear"])
            .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::drain_cmd::DrainArgs>(),
            crate::drain_cmd::DrainArgs {
                inner: DrainCommand::Status { clear: true, .. },
                ..
            }
        ));

        let cli = crate::command_registry::try_parse_from(["aida", "upgrade", "diff"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::upgrade_cmd::UpgradeArgs>(),
            crate::upgrade_cmd::UpgradeArgs {
                cmd: Some(UpgradeCommand::Diff),
                ..
            }
        ));
        let cli = crate::command_registry::try_parse_from(["aida", "upgrade", "--diff"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::upgrade_cmd::UpgradeArgs>(),
            crate::upgrade_cmd::UpgradeArgs { diff: true, .. }
        ));

        let cli = crate::command_registry::try_parse_from(["aida", "statusline", "title"]).unwrap();
        assert!(matches!(
            cli.command
                .registered::<crate::statusline_cmd::StatuslineArgs>(),
            crate::statusline_cmd::StatuslineArgs {
                action: Some(StatuslineAction::Title),
                ..
            }
        ));
        let cli =
            crate::command_registry::try_parse_from(["aida", "statusline", "--title"]).unwrap();
        assert!(matches!(
            cli.command
                .registered::<crate::statusline_cmd::StatuslineArgs>(),
            crate::statusline_cmd::StatuslineArgs { title: true, .. }
        ));

        // trace:TASK-1479 | ai:claude
        let cli = crate::command_registry::try_parse_from(["aida", "statusline"]).unwrap();
        assert!(matches!(
            cli.command
                .registered::<crate::statusline_cmd::StatuslineArgs>(),
            crate::statusline_cmd::StatuslineArgs { client: None, .. }
        ));
        let cli =
            crate::command_registry::try_parse_from(["aida", "statusline", "--client", "claude"])
                .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::statusline_cmd::StatuslineArgs>(),
            crate::statusline_cmd::StatuslineArgs { client: Some(ref c), .. } if c == "claude"
        ));
        let cli =
            crate::command_registry::try_parse_from(["aida", "statusline", "--client", "agy"])
                .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::statusline_cmd::StatuslineArgs>(),
            crate::statusline_cmd::StatuslineArgs { client: Some(ref c), .. } if c == "agy"
        ));
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "statusline",
            "--client",
            "codex"
        ])
        .is_err());

        let cli = crate::command_registry::try_parse_from(["aida", "history", "events"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::history::HistoryArgs>(),
            crate::history::HistoryArgs {
                cmd: Some(HistoryCommand::Events { .. }),
                ..
            }
        ));
        let cli = crate::command_registry::try_parse_from(["aida", "history", "--events"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::history::HistoryArgs>(),
            crate::history::HistoryArgs { events: true, .. }
        ));
    }

    // BUG-1474: CLAUDE.md documents `aida history events [--id <spec-id|uuid>]`
    // as the way to read per-spec status history, but `--id` (and the other
    // History filters) lived only on the parent `History` variant, so clap
    // rejected them once the `events` subcommand token had been consumed.
    // Marking those fields `global = true` makes them parse on either side of
    // `events`, matching the documented invocation.
    // trace:BUG-1474 | ai:claude
    #[test]
    fn history_filters_parse_after_events_subcommand() {
        let cli = crate::command_registry::try_parse_from([
            "aida", "history", "events", "--id", "BUG-1474",
        ])
        .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::history::HistoryArgs>(),
            crate::history::HistoryArgs {
                cmd: Some(HistoryCommand::Events { .. }),
                id: Some(ref id),
                ..
            } if id == "BUG-1474"
        ));

        // Still parses in the pre-existing before-the-subcommand order.
        let cli = crate::command_registry::try_parse_from([
            "aida", "history", "--id", "BUG-1474", "events",
        ])
        .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::history::HistoryArgs>(),
            crate::history::HistoryArgs {
                cmd: Some(HistoryCommand::Events { .. }),
                id: Some(ref id),
                ..
            } if id == "BUG-1474"
        ));

        // A second documented filter (--status-changes) also parses after
        // `events`, not just --id.
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "history",
            "events",
            "--id",
            "BUG-1474",
            "--status-changes",
        ])
        .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::history::HistoryArgs>(),
            crate::history::HistoryArgs {
                cmd: Some(HistoryCommand::Events { .. }),
                id: Some(ref id),
                status_changes: true,
                ..
            } if id == "BUG-1474"
        ));
    }

    // TASK-1480: `aida history <SPEC-ID>` is the positional shorthand for
    // `aida history --id <SPEC-ID>`. Asserts the alias parses into the
    // `spec` field, composes with other flags, conflicts with an explicit
    // `--id`, and doesn't collide with the `events` subcommand.
    // trace:TASK-1480 | ai:claude
    #[test]
    fn history_positional_spec_id_parses() {
        let cli =
            crate::command_registry::try_parse_from(["aida", "history", "TASK-1480"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::history::HistoryArgs>(),
            crate::history::HistoryArgs {
                spec: Some(ref s),
                id: None,
                cmd: None,
                ..
            } if s == "TASK-1480"
        ));

        // Composes with other flags in either order.
        let cli =
            crate::command_registry::try_parse_from(["aida", "history", "TASK-1480", "--oneline"])
                .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::history::HistoryArgs>(),
            crate::history::HistoryArgs {
                spec: Some(ref s),
                oneline: true,
                ..
            } if s == "TASK-1480"
        ));
        let cli =
            crate::command_registry::try_parse_from(["aida", "history", "--full", "TASK-1480"])
                .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::history::HistoryArgs>(),
            crate::history::HistoryArgs {
                spec: Some(ref s),
                full: true,
                ..
            } if s == "TASK-1480"
        ));

        // Bare `events` still resolves to the subcommand, not a spec named
        // "events" — no real spec id is shaped like that, and the
        // subcommand keeps priority.
        let cli = crate::command_registry::try_parse_from(["aida", "history", "events"]).unwrap();
        assert!(matches!(
            cli.command.registered::<crate::history::HistoryArgs>(),
            crate::history::HistoryArgs {
                spec: None,
                cmd: Some(HistoryCommand::Events { .. }),
                ..
            }
        ));

        // Passing both the positional and --id is refused, not silently
        // resolved one way or the other.
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "history",
            "TASK-1480",
            "--id",
            "TASK-9"
        ])
        .is_err());
    }

    // trace:TASK-1427 | ai:codex
    #[test]
    fn token_ledger_usage_subcommands_parse() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "usage",
            "rebuild",
            "--project",
            "project",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::usage_cmd::UsageArgs>(),
            crate::usage_cmd::UsageArgs {
                action: Some(UsageCommand::Rebuild { json: true, .. }),
                ..
            }
        ));

        let cli = crate::command_registry::try_parse_from([
            "aida",
            "usage",
            "show",
            "TASK-1427",
            "--group-by",
            "phase,vendor",
            "--cost",
        ])
        .unwrap();
        assert!(matches!(
            cli.command.registered::<crate::usage_cmd::UsageArgs>(),
            crate::usage_cmd::UsageArgs {
                action: Some(UsageCommand::Show { spec, cost: true, .. }),
                ..
            } if spec == "TASK-1427"
        ));
    }

    // BUG-1294: a prose value starting with a hyphen (most often a bug title
    // quoting the offending flag, e.g. "--format json returns human text")
    // used to be rejected by clap as an unrecognized argument, with a `--`
    // escape hint that doesn't actually work for a named flag's value. Swept
    // `allow_hyphen_values = true` across every free-text-authoring flag;
    // these tests cover the add/edit path plus the two edge cases called out
    // in the acceptance criteria: a leading single hyphen, a leading double
    // hyphen, and a value that exactly matches an existing flag name.
    // trace:BUG-1294 | ai:claude
    #[test]
    fn add_title_accepts_leading_single_hyphen() {
        let cli =
            crate::command_registry::try_parse_from(["aida", "add", "--title", "-not-a-flag"])
                .unwrap();
        match cli.command.registered::<crate::requirements_cli::AddArgs>() {
            crate::requirements_cli::AddArgs { title, .. } => {
                assert_eq!(title.as_deref(), Some("-not-a-flag"))
            }
            other => panic!("expected Command::Add, got {other:?}"),
        }
    }

    #[test]
    fn add_title_accepts_leading_double_hyphen() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "add",
            "--title",
            "--format json returns human text on aida status",
        ])
        .unwrap();
        match cli.command.registered::<crate::requirements_cli::AddArgs>() {
            crate::requirements_cli::AddArgs { title, .. } => assert_eq!(
                title.as_deref(),
                Some("--format json returns human text on aida status")
            ),
            other => panic!("expected Command::Add, got {other:?}"),
        }
    }

    #[test]
    fn add_title_accepts_value_matching_an_existing_flag_name() {
        // "--force" is itself a real flag on `aida add`; the value must still
        // bind to --title rather than being parsed as the --force flag.
        let cli = crate::command_registry::try_parse_from([
            "aida", "add", "--title", "--force", "--type", "bug",
        ])
        .unwrap();
        match cli.command.registered::<crate::requirements_cli::AddArgs>() {
            crate::requirements_cli::AddArgs {
                title,
                force_parent,
                ..
            } => {
                assert_eq!(title.as_deref(), Some("--force"));
                assert!(!force_parent);
            }
            other => panic!("expected Command::Add, got {other:?}"),
        }
    }

    #[test]
    fn add_description_accepts_leading_hyphen() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "add",
            "--title",
            "probe",
            "--description",
            "--foo does the wrong thing",
        ])
        .unwrap();
        match cli.command.registered::<crate::requirements_cli::AddArgs>() {
            crate::requirements_cli::AddArgs { description, .. } => {
                assert_eq!(description.as_deref(), Some("--foo does the wrong thing"))
            }
            other => panic!("expected Command::Add, got {other:?}"),
        }
    }

    #[test]
    fn edit_title_accepts_leading_hyphen() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "edit",
            "BUG-1",
            "--title",
            "--foo does the wrong thing",
        ])
        .unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::EditArgs>()
        {
            crate::requirements_cli::EditArgs { title, .. } => {
                assert_eq!(title.as_deref(), Some("--foo does the wrong thing"))
            }
            other => panic!("expected Command::Edit, got {other:?}"),
        }
    }

    #[test]
    fn edit_description_accepts_leading_hyphen() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "edit",
            "BUG-1",
            "--description",
            "-1 line summary",
        ])
        .unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::EditArgs>()
        {
            crate::requirements_cli::EditArgs { description, .. } => {
                assert_eq!(description.as_deref(), Some("-1 line summary"))
            }
            other => panic!("expected Command::Edit, got {other:?}"),
        }
    }

    #[test]
    fn edit_title_accepts_value_matching_an_existing_flag_name() {
        let cli = crate::command_registry::try_parse_from([
            "aida", "edit", "BUG-1", "--title", "--force",
        ])
        .unwrap();
        match cli
            .command
            .registered::<crate::requirements_cli::EditArgs>()
        {
            crate::requirements_cli::EditArgs { title, force, .. } => {
                assert_eq!(title.as_deref(), Some("--force"));
                assert!(!force);
            }
            other => panic!("expected Command::Edit, got {other:?}"),
        }
    }

    #[test]
    fn add_still_rejects_a_genuinely_mistyped_flag() {
        // allow_hyphen_values only widens what --title/--description accept
        // as their OWN value; it must not swallow an unrelated mistyped
        // flag that was never bound as a value.
        let err = crate::command_registry::try_parse_from(["aida", "add", "--titel", "probe"])
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unexpected argument") || msg.contains("unrecognized"),
            "expected a clear parse error for the mistyped flag, got: {msg}"
        );
    }

    #[test]
    fn edit_still_rejects_a_genuinely_mistyped_flag() {
        let err =
            crate::command_registry::try_parse_from(["aida", "edit", "BUG-1", "--titel", "probe"])
                .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unexpected argument") || msg.contains("unrecognized"),
            "expected a clear parse error for the mistyped flag, got: {msg}"
        );
    }

    // A review finding is free text, even when its first token looks exactly
    // like a real review-record flag. trace:TASK-182 | ai:codex
    #[test]
    fn review_record_finding_accepts_flag_like_prose_and_later_real_flags() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "review",
            "record",
            "TASK-182",
            "--verdict",
            "approved",
            "--finding",
            "--pr N was quoted in the diagnostic",
            "--pr",
            "999",
            "--finding",
            "--yes must remain inert finding text",
        ])
        .unwrap();

        match cli.command.registered::<crate::review_cmd::ReviewArgs>() {
            crate::review_cmd::ReviewArgs {
                cmd:
                    Some(ReviewCommand::Record {
                        finding,
                        pr,
                        verdict,
                        ..
                    }),
                ..
            } => {
                assert_eq!(verdict, "approved");
                assert_eq!(pr, Some(999));
                assert_eq!(
                    finding,
                    vec![
                        "--pr N was quoted in the diagnostic".to_string(),
                        "--yes must remain inert finding text".to_string(),
                    ]
                );
            }
            other => panic!("expected review record command, got {other:?}"),
        }
    }

    // trace:STORY-1417 | ai:claude
    #[test]
    fn review_record_accepts_finding_classes_and_classes_parses() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "review",
            "record",
            "TASK-1",
            "--verdict",
            "request-changes",
            "--finding",
            "left a sibling",
            "--finding-class",
            "incomplete-fix",
        ])
        .unwrap();
        match cli.command.registered::<crate::review_cmd::ReviewArgs>() {
            crate::review_cmd::ReviewArgs {
                cmd: Some(ReviewCommand::Record { finding_class, .. }),
                ..
            } => assert_eq!(finding_class, vec!["incomplete-fix".to_string()]),
            other => panic!("expected review record command, got {other:?}"),
        }
        let cli = crate::command_registry::try_parse_from([
            "aida", "findings", "classes", "--since", "7d",
        ])
        .unwrap();
        match cli
            .command
            .registered::<crate::findings_gate::FindingsArgs>()
        {
            crate::findings_gate::FindingsArgs {
                cmd: Some(FindingsCommand::Classes { since, json, .. }),
                ..
            } => {
                assert_eq!(since.as_deref(), Some("7d"));
                assert!(!json);
            }
            other => panic!("expected findings classes command, got {other:?}"),
        }
        let cli = crate::command_registry::try_parse_from([
            "aida", "review", "classes", "--since", "30d", "--json",
        ])
        .unwrap();
        match cli.command.registered::<crate::review_cmd::ReviewArgs>() {
            crate::review_cmd::ReviewArgs {
                cmd: Some(ReviewCommand::Classes { since, json, .. }),
                ..
            } => {
                assert_eq!(since.as_deref(), Some("30d"));
                assert!(json);
            }
            other => panic!("expected review classes command, got {other:?}"),
        }
    }

    #[test]
    fn review_record_still_rejects_an_unbound_mistyped_flag() {
        let err = crate::command_registry::try_parse_from([
            "aida",
            "review",
            "record",
            "TASK-182",
            "--verdict",
            "approved",
            "--findng",
            "probe",
        ])
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unexpected argument") || msg.contains("unrecognized"),
            "expected a clear parse error for the mistyped flag, got: {msg}"
        );
    }

    // trace:TASK-1307 | ai:claude
    #[test]
    fn review_stranded_parses_json_and_fix_flags() {
        let cli = crate::command_registry::try_parse_from([
            "aida", "review", "stranded", "--json", "--fix",
        ])
        .unwrap();
        match cli.command.registered::<crate::review_cmd::ReviewArgs>() {
            crate::review_cmd::ReviewArgs {
                cmd: Some(ReviewCommand::Stranded { json, fix, age, .. }),
                ..
            } => {
                assert!(json);
                assert!(fix);
                assert!(!age);
            }
            other => panic!("expected review stranded command, got {other:?}"),
        }
    }

    // trace:TASK-1423 | ai:claude
    #[test]
    fn review_stranded_parses_age_flag() {
        let cli = crate::command_registry::try_parse_from([
            "aida", "review", "stranded", "--age", "--json",
        ])
        .unwrap();
        match cli.command.registered::<crate::review_cmd::ReviewArgs>() {
            crate::review_cmd::ReviewArgs {
                cmd: Some(ReviewCommand::Stranded { json, fix, age, .. }),
                ..
            } => {
                assert!(json);
                assert!(!fix);
                assert!(age);
            }
            other => panic!("expected review stranded command, got {other:?}"),
        }
    }

    // trace:TASK-1423 | ai:claude
    #[test]
    fn review_stranded_age_and_fix_are_mutually_exclusive() {
        let err = crate::command_registry::try_parse_from([
            "aida", "review", "stranded", "--age", "--fix",
        ])
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("cannot be used with") || msg.contains("conflicts"),
            "expected a clear conflict error, got: {msg}"
        );
    }

    // trace:STORY-1415 | ai:claude
    #[test]
    fn review_mode_mass_change_parses() {
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "review",
            "mode",
            "mass-change",
            "on",
            "--days",
            "5",
        ])
        .unwrap();
        match cli.command.registered::<crate::review_cmd::ReviewArgs>() {
            crate::review_cmd::ReviewArgs {
                cmd:
                    Some(ReviewCommand::Mode {
                        mode:
                            ReviewModeCommand::MassChange {
                                action, days, json, ..
                            },
                        ..
                    }),
                ..
            } => {
                assert_eq!(action, MassChangeAction::On);
                assert_eq!(days, Some(5));
                assert!(!json);
            }
            other => panic!("expected review mode mass-change, got {other:?}"),
        }
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "review",
            "mode",
            "mass-change",
            "maybe"
        ])
        .is_err());
    }

    // trace:TASK-1307 | ai:claude
    #[test]
    fn review_stranded_defaults_to_read_only() {
        let cli = crate::command_registry::try_parse_from(["aida", "review", "stranded"]).unwrap();
        match cli.command.registered::<crate::review_cmd::ReviewArgs>() {
            crate::review_cmd::ReviewArgs {
                cmd: Some(ReviewCommand::Stranded { json, fix, age, .. }),
                ..
            } => {
                assert!(!json);
                assert!(!fix, "the sweep must default to read-only");
                assert!(!age, "the offline staleness report is opt-in");
            }
            other => panic!("expected review stranded command, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod seat_help_tests {
    use super::Cli;
    use clap::CommandFactory;

    // trace:TASK-1607 | ai:codex
    #[test]
    fn session_seat_generated_help_contains_no_internal_spec_ids() {
        let mut cli = crate::command_registry::built_command();
        let session = cli.find_subcommand_mut("session").unwrap();
        let seat = session.find_subcommand_mut("seat").unwrap();
        let help = seat.render_long_help().to_string();
        let leaves: Vec<_> = seat
            .get_subcommands_mut()
            .map(|leaf| leaf.render_long_help().to_string())
            .collect();
        for text in std::iter::once(help).chain(leaves) {
            for internal in ["STORY-1485", "TASK-1607", "trace:"] {
                assert!(
                    !text.contains(internal),
                    "internal trace leaked to generated help: {text}"
                );
            }
        }
    }
}

/// Generated control kinds; callers cannot label arbitrary roots as controls.
// trace:TASK-1731 | ai:codex
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegenControl {
    Reference,
    Stub,
}

/// Versioned regeneration artifacts.
// trace:TASK-1729 | ai:codex
#[derive(Subcommand, Debug, Clone)]
pub enum RegenCommand {
    /// Report the latest discriminating oracle score and diagnostic gap trends
    // trace:TASK-1733 | ai:codex
    Report,

    /// Run the declared installer and oracle, then record a raw R1 trial
    // trace:TASK-1730 | ai:codex
    Score {
        /// Regenerated system directory; copied into a fresh scoring root
        #[clap(required_unless_present_any = ["control", "consensus"], conflicts_with_all = ["control", "consensus"])]
        dir: Option<PathBuf>,
        /// Run generated controls instead of a caller-supplied system
        #[clap(long, value_enum, conflicts_with = "consensus")]
        control: Option<RegenControl>,
        /// Corpus directory containing corpus.json and manifest.json
        #[clap(
            long,
            required_unless_present = "consensus",
            conflicts_with = "consensus"
        )]
        corpus: Option<PathBuf>,
        #[clap(
            long,
            required_unless_present = "consensus",
            conflicts_with = "consensus"
        )]
        model: Option<String>,
        /// Explicit family identity for cross-family oracle audits (no inferred mapping)
        // trace:STORY-1676 | ai:codex
        #[clap(long, conflicts_with = "consensus")]
        model_family: Option<String>,
        #[clap(
            long,
            required_unless_present = "consensus",
            conflicts_with = "consensus"
        )]
        vendor: Option<String>,
        #[clap(
            long,
            required_unless_present = "consensus",
            conflicts_with = "consensus"
        )]
        harness: Option<String>,
        /// Compare at least three committed scored ordinary trials; never executes tools
        // trace:STORY-1679 | ai:codex
        #[clap(long, num_args = 3..)]
        consensus: Vec<uuid::Uuid>,
        /// File draft amendment tasks for consensus gaps (report-only by default)
        #[clap(long, requires = "consensus")]
        file: bool,
        /// Explicitly bypass bubblewrap; the recorded trial is never clean
        #[clap(long, conflicts_with = "consensus")]
        unconfined: bool,
    },

    /// Export committed store content as corpus.json and manifest.json
    Corpus {
        /// New or empty output directory
        dir: PathBuf,
        /// Export committed content despite local store changes (records dirty: true)
        #[clap(long)]
        allow_dirty: bool,
    },
}
