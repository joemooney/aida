use super::*;
use crate::cli::{BacklogCommand, Command, LoadCommand, PrCommand, QueueCommand};
use chrono::TimeZone;
use clap::Parser;

/// TASK-822 / TASK-824: the `aida list <lens>` aliases rewrite to their
/// canonical commands (flags pass through), and unmatched input is unchanged.
#[test]
fn list_lens_aliases_rewrite_to_canonical_commands() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

    // aida list queue [flags] → aida queue list [flags]
    let out = rewrite_list_alias(&s(&["aida", "list", "queue", "--batch", "X"]));
    assert_eq!(out, s(&["aida", "queue", "list", "--batch", "X"]));
    assert!(matches!(
        Cli::try_parse_from(out).unwrap().command,
        Command::Queue(QueueCommand::List { .. })
    ));

    // aida list why [--json] → aida burndown explain [--json]
    let out = rewrite_list_alias(&s(&["aida", "list", "why", "--json"]));
    assert_eq!(out, s(&["aida", "burndown", "explain", "--json"]));

    // aida list advisor [--short] → aida advisor [--short] (single-token target)
    let out = rewrite_list_alias(&s(&["aida", "list", "advisor", "--short"]));
    assert_eq!(out, s(&["aida", "advisor", "--short"]));
    assert!(matches!(
        Cli::try_parse_from(out).unwrap().command,
        Command::Advisor { .. }
    ));

    // aida list inflight (and in-flight) → aida burndown status
    assert_eq!(
        rewrite_list_alias(&s(&["aida", "list", "inflight"])),
        s(&["aida", "burndown", "status"])
    );
    assert_eq!(
        rewrite_list_alias(&s(&["aida", "list", "in-flight"])),
        s(&["aida", "burndown", "status"])
    );

    // Unmatched: plain `aida list open` and a bare `aida list` are untouched.
    assert_eq!(
        rewrite_list_alias(&s(&["aida", "list", "open"])),
        s(&["aida", "list", "open"])
    );
    assert_eq!(
        rewrite_list_alias(&s(&["aida", "list"])),
        s(&["aida", "list"])
    );
}

/// TASK-862: `aida mylist` rewrites to `aida list me` (forwarding flags) and
/// `aida myqueue` rewrites to `aida queue list`; everything else passes
/// through untouched, and the `aida list` default scope is never changed.
#[test]
fn personal_view_shortcuts_rewrite() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

    // `aida mylist` → `aida list me`
    assert_eq!(
        rewrite_personal_view_alias(&s(&["aida", "mylist"])),
        s(&["aida", "list", "me"])
    );
    // extra flags forward through the rewrite
    assert_eq!(
        rewrite_personal_view_alias(&s(&["aida", "mylist", "--status", "open"])),
        s(&["aida", "list", "me", "--status", "open"])
    );
    // `aida myqueue` → `aida queue list`
    assert_eq!(
        rewrite_personal_view_alias(&s(&["aida", "myqueue"])),
        s(&["aida", "queue", "list"])
    );
    assert_eq!(
        rewrite_personal_view_alias(&s(&["aida", "myqueue", "--no-in-flight"])),
        s(&["aida", "queue", "list", "--no-in-flight"])
    );

    // The rewrites reach the real commands once clap parses them.
    assert!(matches!(
        Cli::try_parse_from(rewrite_personal_view_alias(&s(&["aida", "mylist"])))
            .unwrap()
            .command,
        Command::List { .. }
    ));
    assert!(matches!(
        Cli::try_parse_from(rewrite_personal_view_alias(&s(&["aida", "myqueue"])))
            .unwrap()
            .command,
        Command::Queue(_)
    ));

    // Plain `aida list` / `aida queue list` are untouched — the default
    // scope question stays unactioned.
    assert_eq!(
        rewrite_personal_view_alias(&s(&["aida", "list"])),
        s(&["aida", "list"])
    );
    assert_eq!(
        rewrite_personal_view_alias(&s(&["aida", "queue", "list"])),
        s(&["aida", "queue", "list"])
    );
}

/// STORY-708: `aida groom` is canonical; `aida assess` / `aida intake` are
/// deprecated aliases (clap alias + pre-clap normalization), and `aida
/// advisor assess` rewrites straight to `aida groom` — all reach the same
/// Groom command with flags passed through.
#[test]
fn groom_assess_intake_and_advisor_assess_all_reach_groom() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

    // canonical
    assert!(matches!(
        Cli::try_parse_from(["aida", "groom", "--apply"])
            .unwrap()
            .command,
        Command::Groom { apply: true, .. }
    ));
    // clap visible aliases still parse to Groom
    assert!(matches!(
        Cli::try_parse_from(["aida", "intake"]).unwrap().command,
        Command::Groom { .. }
    ));
    assert!(matches!(
        Cli::try_parse_from(["aida", "assess"]).unwrap().command,
        Command::Groom { .. }
    ));

    // STORY-708: the pre-clap normalization rewrites the deprecated verbs to
    // the canonical `groom` and reports which deprecated spelling was seen.
    let (out, dep) = rewrite_groom_alias(&s(&["aida", "assess", "--apply"]));
    assert_eq!(out, s(&["aida", "groom", "--apply"]));
    assert_eq!(dep.as_deref(), Some("assess"));
    assert!(matches!(
        Cli::try_parse_from(out).unwrap().command,
        Command::Groom { apply: true, .. }
    ));
    let (out, dep) = rewrite_groom_alias(&s(&["aida", "intake", "--dry-run"]));
    assert_eq!(out, s(&["aida", "groom", "--dry-run"]));
    assert_eq!(dep.as_deref(), Some("intake"));
    assert!(matches!(
        Cli::try_parse_from(out).unwrap().command,
        Command::Groom { dry_run: true, .. }
    ));
    // canonical `groom` is NOT flagged as deprecated.
    let (out, dep) = rewrite_groom_alias(&s(&["aida", "groom"]));
    assert_eq!(out, s(&["aida", "groom"]));
    assert_eq!(dep, None);

    // advisor-seat spelling rewrites straight to the canonical top-level groom
    let out = rewrite_advisor_assess(&s(&["aida", "advisor", "assess", "--dry-run"]));
    assert_eq!(out, s(&["aida", "groom", "--dry-run"]));
    assert!(matches!(
        Cli::try_parse_from(out).unwrap().command,
        Command::Groom { dry_run: true, .. }
    ));
    // unrelated advisor subcommands untouched
    assert_eq!(
        rewrite_advisor_assess(&s(&["aida", "advisor", "status"])),
        s(&["aida", "advisor", "status"])
    );
    // a positional value that reads `assess` (not the subcommand) is left alone
    assert_eq!(
        rewrite_groom_alias(&s(&["aida", "search", "assess"])).0,
        s(&["aida", "search", "assess"])
    );
}

/// TASK-858: bare `aida agent` (and `aida agent <flags>` with no recognized
/// subcommand) rewrites to `aida agent new`, forwarding flags; recognized
/// subcommands and `--help`/`-h` pass through unchanged.
#[test]
fn bare_agent_defaults_to_agent_new() {
    use crate::cli::{AgentCommand, Command};
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

    // bare `aida agent` → `aida agent new`
    let out = rewrite_agent_default_new(&s(&["aida", "agent"]));
    assert_eq!(out, s(&["aida", "agent", "new"]));
    assert!(matches!(
        Cli::try_parse_from(out).unwrap().command,
        Command::Agent(AgentCommand::New { command: None })
    ));

    // `aida agent --role X` (a `new` flag, no subcommand) → spliced `new`
    assert_eq!(
        rewrite_agent_default_new(&s(&["aida", "agent", "--show-context"])),
        s(&["aida", "agent", "new", "--show-context"])
    );

    // Recognized subcommands pass through unchanged.
    for sub in [
        "new",
        "ls",
        "status",
        "dispatch-health",
        "register",
        "pause",
        "resume",
        "stop",
    ] {
        assert_eq!(
            rewrite_agent_default_new(&s(&["aida", "agent", sub])),
            s(&["aida", "agent", sub]),
            "subcommand {sub} should pass through"
        );
    }
    assert_eq!(
        rewrite_agent_default_new(&s(&["aida", "agent", "list-roles", "--json"])),
        s(&["aida", "agent", "list-roles", "--json"])
    );

    // `aida agent ls` still reaches the Ls variant after the rewrite.
    assert!(matches!(
        Cli::try_parse_from(rewrite_agent_default_new(&s(&["aida", "agent", "ls"])))
            .unwrap()
            .command,
        Command::Agent(AgentCommand::Ls)
    ));
    assert!(matches!(
        Cli::try_parse_from(rewrite_agent_default_new(&s(&[
            "aida",
            "agent",
            "dispatch-health"
        ])))
        .unwrap()
        .command,
        Command::Agent(AgentCommand::DispatchHealth { force: false })
    ));

    // Help flags + the `help` verb keep clap's parent help (untouched).
    assert_eq!(
        rewrite_agent_default_new(&s(&["aida", "agent", "--help"])),
        s(&["aida", "agent", "--help"])
    );
    assert_eq!(
        rewrite_agent_default_new(&s(&["aida", "agent", "-h"])),
        s(&["aida", "agent", "-h"])
    );
    assert_eq!(
        rewrite_agent_default_new(&s(&["aida", "agent", "help"])),
        s(&["aida", "agent", "help"])
    );

    // Non-agent commands are untouched.
    assert_eq!(
        rewrite_agent_default_new(&s(&["aida", "list"])),
        s(&["aida", "list"])
    );
}

// trace:TASK-881 | ai:claude
#[test]
fn bare_queue_defaults_to_queue_list() {
    use crate::cli::{Command, QueueCommand};
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

    // bare `aida queue` → `aida queue list`
    let out = rewrite_queue_default_list(&s(&["aida", "queue"]));
    assert_eq!(out, s(&["aida", "queue", "list"]));
    assert!(matches!(
        Cli::try_parse_from(out).unwrap().command,
        Command::Queue(QueueCommand::List { .. })
    ));

    // Flags-only `aida queue --for implementer` (no subcommand) → spliced `list`
    assert_eq!(
        rewrite_queue_default_list(&s(&["aida", "queue", "--for", "implementer"])),
        s(&["aida", "queue", "list", "--for", "implementer"])
    );

    // Recognized subcommands pass through unchanged.
    for sub in [
        "list",
        "add",
        "load",
        "remove",
        "move",
        "clear",
        "prune",
        "gc",
        "next",
        "advance",
        "done",
        "work",
        "progress",
        "rework",
        "recover",
        "integrate",
    ] {
        assert_eq!(
            rewrite_queue_default_list(&s(&["aida", "queue", sub])),
            s(&["aida", "queue", sub]),
            "subcommand {sub} should pass through"
        );
    }

    // `aida queue work TASK-1` still reaches the Work variant after the rewrite.
    assert!(matches!(
        Cli::try_parse_from(rewrite_queue_default_list(&s(&[
            "aida", "queue", "work", "TASK-1"
        ])))
        .unwrap()
        .command,
        Command::Queue(QueueCommand::Work { .. })
    ));

    // Help flags + the `help` verb keep clap's parent help (untouched).
    for help in [
        &["aida", "queue", "--help"][..],
        &["aida", "queue", "-h"][..],
        &["aida", "queue", "help"][..],
    ] {
        assert_eq!(
            rewrite_queue_default_list(&s(help)),
            s(help),
            "help form {help:?} should pass through"
        );
    }

    // Non-queue commands are untouched.
    assert_eq!(
        rewrite_queue_default_list(&s(&["aida", "list"])),
        s(&["aida", "list"])
    );
}

#[test]
fn canonical_asciinema_flags_parse_as_top_level_wrapper() {
    let cli = Cli::try_parse_from([
        "aida",
        "--asciinema",
        "--cast-out",
        "/tmp/demo.cast",
        "--cast-title",
        "Demo: overnight drain",
        "queue",
        "work",
        "--batch",
        "overnight-X",
        "--auto-complete",
    ])
    .unwrap();

    assert!(cli.asciinema);
    assert_eq!(
        cli.cast_out,
        Some(std::path::PathBuf::from("/tmp/demo.cast"))
    );
    assert_eq!(cli.cast_title.as_deref(), Some("Demo: overnight drain"));
    match cli.command {
        Command::Queue(QueueCommand::Work {
            batch,
            auto_complete,
            ..
        }) => {
            assert_eq!(batch.as_deref(), Some("overnight-X"));
            assert!(auto_complete.is_some());
        }
        other => panic!("expected queue work command, got {other:?}"),
    }
}

#[test]
fn story_451_effort_flags_and_load_aliases_parse() {
    let add = Cli::try_parse_from([
        "aida",
        "add",
        "--title",
        "estimate me",
        "--type",
        "task",
        "--effort",
        "1d",
    ])
    .unwrap();
    match add.command {
        Command::Add { effort, .. } => {
            assert_eq!(effort, Some(effort_calibration::EffortBucket::OneDay));
        }
        other => panic!("expected add, got {other:?}"),
    }

    let queue = Cli::try_parse_from(["aida", "queue", "work", "TASK-1", "--effort", "4h"]).unwrap();
    match queue.command {
        Command::Queue(QueueCommand::Work { effort, .. }) => {
            assert_eq!(effort, Some(effort_calibration::EffortBucket::FourHours));
        }
        other => panic!("expected queue work, got {other:?}"),
    }

    let ship = Cli::try_parse_from(["aida", "pr", "ship", "--effort", "15m"]).unwrap();
    match ship.command {
        Command::Pr(PrCommand::Ship { effort, .. }) => {
            assert_eq!(
                effort,
                Some(effort_calibration::EffortBucket::FifteenMinutes)
            );
        }
        other => panic!("expected pr ship, got {other:?}"),
    }

    assert!(matches!(
        Cli::try_parse_from(["aida", "load", "queue"])
            .unwrap()
            .command,
        Command::Load(LoadCommand::Queue)
    ));
    assert!(matches!(
        Cli::try_parse_from(["aida", "queue", "load"])
            .unwrap()
            .command,
        Command::Queue(QueueCommand::Load { .. })
    ));
    assert!(matches!(
        Cli::try_parse_from(["aida", "backlog", "load"])
            .unwrap()
            .command,
        Command::Backlog(BacklogCommand::Load)
    ));
}

// TASK-777: `aida fasttrack <title>` parses with `task` as the default type
// and accepts a `--type` override. The verb rewrites to the equivalent
// `Command::Add` at dispatch, so this parse test pins the surface; the
// filing convention (Approved + queue + batch:fasttrack + lifecycle:no-review)
// is exercised by the Add path it delegates to. trace:TASK-777
#[test]
fn task_777_fasttrack_parses_with_default_and_override_type() {
    match Cli::try_parse_from(["aida", "fasttrack", "fix a typo"])
        .unwrap()
        .command
    {
        Command::Fasttrack {
            title,
            r#type,
            express,
            command,
        } => {
            assert_eq!(title.as_deref(), Some("fix a typo"));
            assert_eq!(r#type, "task");
            assert!(!express, "trivial tier is the default — no --express");
            assert!(command.is_none());
        }
        other => panic!("expected fasttrack, got {other:?}"),
    }

    match Cli::try_parse_from(["aida", "fasttrack", "papercut", "--type", "bug"])
        .unwrap()
        .command
    {
        Command::Fasttrack {
            title,
            r#type,
            express,
            command,
        } => {
            assert_eq!(title.as_deref(), Some("papercut"));
            assert_eq!(r#type, "bug");
            assert!(!express);
            assert!(command.is_none());
        }
        other => panic!("expected fasttrack, got {other:?}"),
    }

    // TASK-905: `aida fasttrack status` parses as the lane-status
    // subcommand, not a titled file. trace:TASK-905
    match Cli::try_parse_from(["aida", "fasttrack", "status"])
        .unwrap()
        .command
    {
        Command::Fasttrack { title, command, .. } => {
            assert!(title.is_none(), "`status` must not be read as a title");
            assert!(matches!(
                command,
                Some(crate::cli::FasttrackCommand::Status { json: false })
            ));
        }
        other => panic!("expected fasttrack status, got {other:?}"),
    }

    match Cli::try_parse_from(["aida", "fasttrack", "status", "--json"])
        .unwrap()
        .command
    {
        Command::Fasttrack { command, .. } => {
            assert!(matches!(
                command,
                Some(crate::cli::FasttrackCommand::Status { json: true })
            ));
        }
        other => panic!("expected fasttrack status --json, got {other:?}"),
    }

    // STORY-692: `--express` parses as the express tier; the title still
    // files and the lane subcommand stays absent. trace:STORY-692
    match Cli::try_parse_from(["aida", "fasttrack", "fix an easy bug", "--express"])
        .unwrap()
        .command
    {
        Command::Fasttrack {
            title,
            express,
            command,
            ..
        } => {
            assert_eq!(title.as_deref(), Some("fix an easy bug"));
            assert!(express, "--express must select the express tier");
            assert!(command.is_none());
        }
        other => panic!("expected fasttrack --express, got {other:?}"),
    }
}

// STORY-692: the express-tier filing tags `batch:express`, queues
// (status approved + the Add `queue: true` path), and crucially carries NO
// `lifecycle:*` skip tag — so the full CI + reviewer + build gate runs. The
// trivial tier, by contrast, rides `batch:fasttrack` + `lifecycle:no-review`.
// Pinned against the shared `fasttrack_lane_filing` helper the handler uses,
// so the invariant can't drift without this test catching it.
// trace:STORY-692 | ai:claude
#[test]
fn fasttrack_express_files_approved_queued_with_batch_express_no_lifecycle() {
    // Express tier: batch:express + NO lifecycle skip (full gate).
    let (bucket, lane_tags) = fasttrack_lane_filing(true);
    assert_eq!(
        bucket, "express",
        "express tier rides the batch:express bucket"
    );
    assert!(
        lane_tags.is_none(),
        "express tier must carry NO lifecycle:* skip tag — got {lane_tags:?}"
    );

    // Trivial tier (default): batch:fasttrack + lifecycle:no-review.
    let (bucket, lane_tags) = fasttrack_lane_filing(false);
    assert_eq!(bucket, "fasttrack");
    assert_eq!(lane_tags.as_deref(), Some("lifecycle:no-review"));
}

// STORY-692: the express invariant, stated as the negative the design names —
// the express tier never sets ANY `lifecycle:*` skip tag (no-review,
// no-ci-wait, no-build, trivial). "Fast because reliably routed, not because
// less gated." trace:STORY-692
#[test]
fn fasttrack_express_carries_no_lifecycle_skip_tag() {
    let (_bucket, lane_tags) = fasttrack_lane_filing(true);
    let tags = lane_tags.unwrap_or_default();
    assert!(
        !tags.contains("lifecycle:"),
        "express tier must not skip any lifecycle phase — got tags {tags:?}"
    );
}

// TASK-905: the lane stage projection maps fixture lane items onto the
// right stage from their status + routing signals. Pure function — no
// store, queue dir, or lease probe. trace:TASK-905
#[test]
fn task_905_fasttrack_stage_projection_maps_fixtures() {
    use FasttrackStage::*;
    // (status_str, is_queued, is_running, has_punt) → expected stage.
    let cases: &[(&str, bool, bool, bool, FasttrackStage)] = &[
        // Draft is always "requested", regardless of stray signals.
        ("Draft", false, false, false, Requested),
        // Approved: accepted by default, queued on a queue, running under a lease.
        ("Approved", false, false, false, Accepted),
        ("Approved", true, false, false, Queued),
        ("Approved", true, true, false, Running),
        // Planned: queued, or running once a lease holds it.
        ("Planned", false, false, false, Queued),
        ("Planned", false, true, false, Running),
        // InProgress is always running.
        ("InProgress", false, false, false, Running),
        // NeedsAttention splits punted (has a punt record) vs blocked.
        ("NeedsAttention", false, false, false, Blocked),
        ("NeedsAttention", false, false, true, Punted),
        // Done / Completed both project to shipped.
        ("Done", false, false, false, Shipped),
        ("Completed", false, false, false, Shipped),
        // Rejected.
        ("Rejected", false, false, false, Rejected),
        // Case-insensitive on the stored Debug string.
        ("approved", true, false, false, Queued),
        // Unknown / custom status falls back via routing signals.
        ("Blocked-by-design", false, false, false, Requested),
        ("Blocked-by-design", true, false, false, Queued),
        ("Blocked-by-design", false, true, false, Running),
    ];
    for (status, q, r, p, expected) in cases {
        assert_eq!(
            project_fasttrack_stage(status, *q, *r, *p),
            *expected,
            "status={status} queued={q} running={r} punt={p}",
        );
    }
    // The shipped state wins even if a stale lease still names it.
    assert_eq!(
        project_fasttrack_stage("Completed", false, true, false),
        Shipped
    );
}

// TASK-926 (Followup B of TASK-0438/TASK-905): a spec parked at
// NeedsAttention projects to "punted" when a punt record names it and to
// "blocked" otherwise. The punt record is the only signal that distinguishes
// the two — queue/lease signals are irrelevant in the NeedsAttention arm.
// trace:TASK-926
#[test]
fn lane_status_maps_needsattention_to_blocked_or_punted() {
    // A punt record naming the spec → punted.
    assert_eq!(
        project_fasttrack_stage("NeedsAttention", false, false, true),
        FasttrackStage::Punted,
        "NeedsAttention with a punt record should project to Punted",
    );
    // No punt record → blocked.
    assert_eq!(
        project_fasttrack_stage("NeedsAttention", false, false, false),
        FasttrackStage::Blocked,
        "NeedsAttention without a punt record should project to Blocked",
    );
    // The has_punt flag is the sole discriminator: stray queue/lease signals
    // do not change the blocked-vs-punted verdict in the NeedsAttention arm.
    assert_eq!(
        project_fasttrack_stage("NeedsAttention", true, true, true),
        FasttrackStage::Punted,
        "a punt record wins over queue/lease signals at NeedsAttention",
    );
    assert_eq!(
        project_fasttrack_stage("NeedsAttention", true, true, false),
        FasttrackStage::Blocked,
        "no punt record stays Blocked even with queue/lease signals",
    );
}

#[test]
fn strip_wrapper_flags_preserves_inner_command() {
    let raw = vec![
        "aida".to_string(),
        "--asciinema".to_string(),
        "--cast-out".to_string(),
        "/tmp/demo.cast".to_string(),
        "--cast-title=with spaces".to_string(),
        "queue".to_string(),
        "work".to_string(),
        "--batch".to_string(),
        "overnight-X".to_string(),
        "--auto-complete".to_string(),
    ];

    assert_eq!(
        strip_asciinema_wrapper_args(&raw),
        vec![
            "aida",
            "queue",
            "work",
            "--batch",
            "overnight-X",
            "--auto-complete"
        ]
    );
}

#[test]
fn default_title_is_inner_aida_command() {
    // queue work with batch name
    let inner = vec![
        "aida".to_string(),
        "queue".to_string(),
        "work".to_string(),
        "--batch".to_string(),
        "overnight-X".to_string(),
        "--auto-complete".to_string(),
    ];
    assert_eq!(default_cast_title(&inner), "AIDA drain: batch overnight-X");

    // queue work with specs
    let inner_specs = vec![
        "aida".to_string(),
        "queue".to_string(),
        "work".to_string(),
        "STORY-316".to_string(),
        "TASK-479".to_string(),
    ];
    assert_eq!(
        default_cast_title(&inner_specs),
        "AIDA drain: STORY-316, TASK-479"
    );

    // pr ship with PR id
    let pr_ship = vec![
        "aida".to_string(),
        "pr".to_string(),
        "ship".to_string(),
        "123".to_string(),
    ];
    assert_eq!(default_cast_title(&pr_ship), "AIDA pr ship: PR-123");

    // generic fallback
    let generic = vec![
        "aida".to_string(),
        "show".to_string(),
        "TASK-123".to_string(),
    ];
    assert_eq!(default_cast_title(&generic), "aida show TASK-123");
}

#[test]
fn cast_slug_is_windows_safe_and_truncated_visibly() {
    // queue work with batch name
    let inner = vec![
        "aida".to_string(),
        "queue".to_string(),
        "work".to_string(),
        "--batch".to_string(),
        "overnight-X".to_string(),
        "--auto-complete".to_string(),
    ];
    assert_eq!(asciinema_command_slug(&inner), "batch-overnight-x");

    // queue work with specs
    let inner_specs = vec![
        "aida".to_string(),
        "queue".to_string(),
        "work".to_string(),
        "STORY-316".to_string(),
        "TASK-479".to_string(),
    ];
    assert_eq!(asciinema_command_slug(&inner_specs), "story-316-task-479");

    // pr ship with PR id
    let pr_ship = vec![
        "aida".to_string(),
        "pr".to_string(),
        "ship".to_string(),
        "123".to_string(),
    ];
    assert_eq!(asciinema_command_slug(&pr_ship), "pr-123");

    // show with spec ID
    let generic_spec = vec![
        "aida".to_string(),
        "show".to_string(),
        "TASK-123".to_string(),
    ];
    assert_eq!(asciinema_command_slug(&generic_spec), "task-123");

    let long = "x".repeat(120);
    let truncated = truncate_asciinema_slug(&long);
    assert_eq!(truncated.chars().count(), ASCIINEMA_SLUG_MAX_CHARS);
    assert!(truncated.ends_with("-trunc"));
    assert!(!truncated.contains(':'));
}

#[test]
fn cast_timestamp_format_has_hhmmss_and_no_colons() {
    let stamp = chrono::Utc
        .with_ymd_and_hms(2026, 5, 23, 1, 30, 45)
        .unwrap()
        .format("%Y-%m-%dT%H%M%SZ")
        .to_string();

    assert_eq!(stamp, "2026-05-23T013045Z");
    assert!(!stamp.contains(':'));
}

#[test]
fn shell_command_quotes_spaces_and_single_quotes() {
    let exe = std::path::Path::new("/tmp/aida dev/aida");
    let inner = vec![
        "aida".to_string(),
        "comment".to_string(),
        "add".to_string(),
        "TASK-1".to_string(),
        "with spaces and 'quote'".to_string(),
    ];

    assert_eq!(
        shell_command_for_asciinema(exe, &inner),
        "'/tmp/aida dev/aida' comment add TASK-1 'with spaces and '\"'\"'quote'\"'\"''"
    );
}
