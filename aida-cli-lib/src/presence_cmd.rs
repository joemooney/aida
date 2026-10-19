//! `aida away` / `aida home` / `aida presence` command handlers — the
//! STORY-769 turn-clock human-presence trio. Extracted verbatim from `main.rs`
//! (SPIKE-78, pure movement — no behavior change). The presence oracle proper
//! (`Presence`, `human_presence`, file read/write, TTL/label helpers) lives in
//! `crate::presence`; these are just the CLI front doors that call it.

use anyhow::Result;

// trace:STORY-1728 | ai:claude
#[path = "presence_cmd_args.rs"]
pub(crate) mod presence_cmd_args;
pub(crate) use presence_cmd_args::*;

use crate::command_registry::{CmdPolicy, CommandReg};
use crate::stakeholder_role::StakeholderAction;

use colored::Colorize;

/// Worker entry point — runs `git fetch origin <branch> --prune` against
/// the orphan store at `store_path`, then stamps `last-fetch.toml` with
/// the outcome and removes the lockfile. Never panics; every error
/// path is silent. Called from `aida _bg-fetch <store-path>`, which
/// statusline spawns detached. Shares its fetch + cache-stamp logic
/// with `aida fetch --store-only --quiet` (TASK-107); the only thing
/// it adds is the lockfile lifecycle.
// trace:STORY-79 TASK-107 | ai:claude
/// Resolve the away TTL for the current project: `[presence] away_ttl` from
/// `.aida/config.toml` when present, else the 8h default.
// trace:TASK-756
fn resolve_away_ttl_secs() -> u64 {
    let project_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    crate::presence::away_ttl_secs(&crate::config_path_for_project(&project_root))
}

/// `aida away` — mark the operator away with the configured TTL.
// trace:TASK-756 | ai:claude
pub(crate) fn handle_away_command() -> Result<()> {
    let ttl = resolve_away_ttl_secs();
    crate::presence::set_away(ttl)?;
    let now = chrono::Utc::now();
    let set_at = now; // just set
    println!(
        "{} away — effective for {} (auto-flips home on any interactive command)",
        crate::glyph(crate::glyphs::Glyph::Away).to_string(),
        crate::presence::ttl_remaining_label(set_at, ttl, now)
    );
    Ok(())
}

/// `aida home` — mark the operator back at the keyboard.
// trace:TASK-756 | ai:claude
pub(crate) fn handle_home_command() -> Result<()> {
    let ttl = resolve_away_ttl_secs();
    crate::presence::set_home(ttl)?;
    println!(
        "{} home",
        crate::glyph(crate::glyphs::Glyph::Home).to_string()
    );
    Ok(())
}

/// Mark yourself away from the keyboard. Sets a machine-global presence
/// state (a timestamped `~/.aida/presence.toml` file — no daemon) that
/// stays `away` until its TTL lapses (default 8h) or you return. An
/// interactive command auto-flips you back to `home`.
///
/// Presence is ADVISORY input to mode selection: while away, an
/// `aida queue work --auto-complete` with no explicit `--no-human` /
/// `--escalate-*` flag defaults to a headless drain; explicit flags always
/// win and integrity gates (the kickoff scope-ack, CI, merge-on-green) always
/// apply. Tune under `[presence]` in `.aida/config.toml`:
///   consumers  = "on" | "off"            (master switch; default on)
///   away_drain = "headless-both"         (default) | "headless-escalate-defaults" | "headless-park"
///   escalation = "defaults" | "park"     (punt handling; unset = derive from away_drain)
///   home_offer = "surface" | "dont-block" (home-side; default surface)
///
/// Hidden alias for `aida presence away` (kept for muscle memory).
// trace:TASK-756 trace:STORY-561 | ai:claude
// trace:TASK-851 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "away", hide = true)]
pub(crate) struct AwayArgs {}

/// Mark yourself back at the keyboard (clears any away state). Hidden alias
/// for `aida presence home` (kept for muscle memory).
// trace:TASK-756 trace:STORY-561 | ai:claude
// trace:TASK-851 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "home", hide = true)]
pub(crate) struct HomeArgs {}

// Neither verb writes the requirement store; both only touch the
// machine-global presence file.
// trace:STORY-1728 | ai:claude
const PRESENCE_POLICY: CmdPolicy = CmdPolicy {
    stakeholder: StakeholderAction::Write,
    per_write_auto_push: false,
    skip_role_restore_prompt: false,
    team_identity_write: false,
};

// trace:STORY-1728 | ai:claude
pub(crate) static AWAY: CommandReg = CommandReg {
    name: "away",
    help_after: "statusline",
    command: <AwayArgs as clap::CommandFactory>::command,
    policy: |_| PRESENCE_POLICY,
    run: crate::command_registry::Handler::Early(|_, _| handle_away_command()),
};

// trace:STORY-1728 | ai:claude
pub(crate) static HOME: CommandReg = CommandReg {
    name: "home",
    help_after: "away",
    command: <HomeArgs as clap::CommandFactory>::command,
    policy: |_| PRESENCE_POLICY,
    run: crate::command_registry::Handler::Early(|_, _| handle_home_command()),
};

/// `aida presence` — print current effective presence.
// trace:TASK-756 | ai:claude
pub(crate) fn handle_presence_command() -> Result<()> {
    let now = chrono::Utc::now();
    let file = crate::presence::read_presence_file();
    let effective = crate::presence::current_presence(now);

    match file {
        Some(f) => {
            let set_at = chrono::DateTime::parse_from_rfc3339(&f.set_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or(now);
            print!(
                "{} {} (set {}",
                effective.glyph(),
                effective.word().bold(),
                crate::presence::since_label(set_at, now)
            );
            if matches!(effective, crate::presence::Presence::Away) {
                print!(
                    ", {}",
                    crate::presence::ttl_remaining_label(set_at, f.ttl_secs, now)
                );
            }
            println!(")");
        }
        None => {
            // No file written yet — default posture is home.
            println!(
                "{} {} (default — never set)",
                effective.glyph(),
                effective.word().bold()
            );
        }
    }

    // STORY-769: fold in the last-human-input ORACLE — a passive observation
    // (distinct from the explicit home/away intent above) of when the operator
    // last typed a prompt, per the per-turn `aida awaiting --notice` stamp. This
    // is what the escalation cascade branches on: operator active → an
    // interactive ask is answerable; stale → park / go headless.
    // trace:STORY-769 | ai:claude
    let project_root =
        crate::find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let thresholds =
        crate::presence::read_presence_thresholds(&crate::config_path_for_project(&project_root));
    match crate::presence::last_seen_line(now, thresholds) {
        Some(line) => println!("{}", line.dimmed()),
        None => println!(
            "{}",
            "operator last seen — unknown (no prompt stamped yet)".dimmed()
        ),
    }
    Ok(())
}

/// Operator presence — the home/away state the autonomy ladder keys on.
/// `aida presence` (or `aida presence status`) shows the current state;
/// `aida presence away` / `aida presence home` set it. While away, an
/// `aida queue work --auto-complete` with no explicit mode flag defaults to
/// a headless drain (advisory; integrity gates always apply).
// trace:TASK-756 | ai:claude
// trace:TASK-851 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "presence")]
pub(crate) struct PresenceArgs {
    #[clap(subcommand)]
    pub action: Option<crate::cli::PresenceCommand>,
}

// TASK-851: `aida presence [away|home|status]` is the canonical surface;
// bare `aida presence` shows status (back-compat). The top-level
// `aida away` / `aida home` stay as hidden aliases.
// trace:TASK-851 | ai:claude
fn run_presence(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <PresenceArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match args.action {
        Some(crate::cli::PresenceCommand::Away) => handle_away_command(),
        Some(crate::cli::PresenceCommand::Home) => handle_home_command(),
        Some(crate::cli::PresenceCommand::Status) | None => handle_presence_command(),
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static PRESENCE: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "presence",
        help_after: "home",
        command: <PresenceArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_presence),
    };
