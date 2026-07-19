//! `aida away` / `aida home` / `aida presence` command handlers — the
//! STORY-769 turn-clock human-presence trio. Extracted verbatim from `main.rs`
//! (SPIKE-78, pure movement — no behavior change). The presence oracle proper
//! (`Presence`, `human_presence`, file read/write, TTL/label helpers) lives in
//! `crate::presence`; these are just the CLI front doors that call it.

use anyhow::Result;

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
