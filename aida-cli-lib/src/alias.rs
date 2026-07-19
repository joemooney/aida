//! `aida alias` / `aida alias list` — a discoverable registry of AIDA's
//! BUILT-IN shortcuts, all in one place.
//!
//! AIDA has accreted many shortcuts, each hidden in its own `--help`: the
//! `aida list` status lenses (`open` / `closed`) and status-token shortcuts
//! (`aida list approved` == `--status approved`), the `aida list <lens>` argv
//! rewrites (`queue` / `why` / `human` / `inflight` / `me` / `user:<name>`),
//! and the command-level aliases (`assess` / `intake` -> `groom`, `advisor
//! assess` -> `groom`, bare `aida agent` -> `aida agent new`). This command
//! enumerates
//! them ALL, grouped by surface, each row carrying the alias, its canonical
//! expansion, and a one-line meaning. `--json` for machine consumers.
//!
//! SINGLE SOURCE OF TRUTH: the rows are SOURCED FROM the existing resolvers,
//! not hand-duplicated. The list-lens rows are derived from
//! [`crate::LIST_LENS_ALIASES`] (the same const `rewrite_list_alias` resolves
//! against), and a unit test asserts the registry agrees with the live
//! resolvers (`rewrite_list_alias`, `rewrite_advisor_assess`,
//! `rewrite_agent_default_new`, and clap parsing of the status shortcuts) so
//! the catalog can't drift from the surface it documents.
//!
//! User-defined aliases (TASK-877) are layered on top: `aida alias add/remove`
//! manage them (see `user_alias`), and the registry appends a "User aliases"
//! group sourced from `user_alias::effective_user_aliases()` so personal +
//! project aliases show alongside built-ins, marked by source. They are shown
//! to every caller (discoverability) but EXPANDED only for an interactive human
//! shell.
//!
//! trace:STORY-667 | ai:claude
//! trace:TASK-877 | ai:claude

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;

/// One built-in shortcut row in the `aida alias` registry.
/// trace:STORY-667 | ai:claude
#[derive(Debug, Clone, Serialize)]
pub struct AliasRow {
    /// The alias as the user types it (e.g. `aida list open`).
    pub alias: String,
    /// What it canonically expands to (e.g. `aida list --status approved,...`).
    pub expands_to: String,
    /// One-line plain meaning.
    pub meaning: String,
}

/// A surface group of built-in shortcuts.
/// trace:STORY-667 | ai:claude
#[derive(Debug, Clone, Serialize)]
pub struct AliasGroup {
    /// The surface name (e.g. "Status lenses").
    pub surface: String,
    /// One-line description of the group.
    pub about: String,
    /// The shortcut rows in this group.
    pub rows: Vec<AliasRow>,
}

/// The canonical status tokens accepted as positional shortcuts on `aida list`.
/// Every entry MUST parse via `RequirementStatus::from_filter_str` (asserted by
/// a unit test) — that recognizer is the single source for "what is a valid
/// status shortcut", so this list documents the accepted set without forking
/// it. trace:STORY-667 | ai:claude
const STATUS_TOKENS: &[&str] = &[
    "draft",
    "approved",
    "planned",
    "in-progress",
    "done",
    "completed",
    "rejected",
    "needs-attention",
];

fn row(alias: &str, expands_to: &str, meaning: &str) -> AliasRow {
    AliasRow {
        alias: alias.to_string(),
        expands_to: expands_to.to_string(),
        meaning: meaning.to_string(),
    }
}

/// Build the full grouped registry of built-in shortcuts.
///
/// The List-lenses group is DERIVED from [`crate::LIST_LENS_ALIASES`] so it
/// can't drift from `rewrite_list_alias`. The other groups document resolvers
/// (status-shortcut expansion in the `List` handler, `rewrite_advisor_assess`,
/// `rewrite_agent_default_new`, and clap's `#[command(alias = "intake")]`); a
/// unit test pins each to the live resolver. trace:STORY-667 | ai:claude
pub fn registry() -> Vec<AliasGroup> {
    // --- Status lenses (positional status shortcut on `aida list`) ----------
    // `aida list <status>` == `aida list --status <status>`; the `open` /
    // `closed` aliases expand to the open/closed status sets. Resolved by
    // RequirementStatus::expand_filter_spec in the `List` handler.
    let mut status_rows = vec![
        row(
            "aida list open",
            "aida list --status open",
            "every not-yet-closed spec (draft..needs-attention)",
        ),
        row(
            "aida list closed",
            "aida list --status closed",
            "every finished/abandoned spec (done, completed, rejected)",
        ),
    ];
    // The canonical statuses are each their own positional shortcut. The tokens
    // are the user-facing spellings (hyphenated); each is validated against
    // `RequirementStatus::from_filter_str` (the single recognizer the `aida
    // list` shortcut path uses) by the unit test, so this list can't drift from
    // the accepted set. trace:STORY-667
    for tok in STATUS_TOKENS {
        status_rows.push(row(
            &format!("aida list {tok}"),
            &format!("aida list --status {tok}"),
            &format!("only {tok} specs"),
        ));
    }

    // --- List lenses (argv rewrites on `aida list <lens>`) ------------------
    // DERIVED from LIST_LENS_ALIASES — the same const rewrite_list_alias reads.
    let mut list_rows: Vec<AliasRow> = crate::LIST_LENS_ALIASES
        .iter()
        .map(|a| {
            let alias_tokens = a.tokens.join(" / aida list ");
            row(
                &format!("aida list {alias_tokens}"),
                &format!("aida {}", a.canonical.join(" ")),
                a.meaning,
            )
        })
        .collect();
    // `human` + `me` / `user:<name>` are resolved inside the `List` handler
    // (peeled off before status-shortcut expansion), not by rewrite_list_alias.
    list_rows.push(row(
        "aida list human",
        "aida list --human",
        "the \"what needs me?\" view — open specs flagged for a human nudge",
    ));
    list_rows.push(row(
        "aida list me",
        "aida list --user me",
        "specs you own or are assigned (shell identity)",
    ));
    list_rows.push(row(
        "aida list user:<name>",
        "aida list --user <name>",
        "specs owned by or assigned to <name>",
    ));

    // --- Command aliases (clap aliases + top-level argv rewrites) -----------
    let command_rows = vec![
        row(
            "aida assess",
            "aida groom",
            "deprecated alias for the headless advisor disposition pass",
        ),
        row(
            "aida intake",
            "aida groom",
            "deprecated alias for the headless advisor disposition pass",
        ),
        row(
            "aida advisor assess",
            "aida groom",
            "the advisor-seat spelling of `aida groom`",
        ),
        row(
            "aida agent <args>",
            "aida agent new <args>",
            "bare `aida agent` defaults to the launcher (git-style)",
        ),
        // TASK-881: bare `aida queue` defaults to the read view (argv rewrite
        // in main.rs: `rewrite_queue_default_list`), matching `aida list` /
        // `aida status` ergonomics.
        row(
            "aida queue <args>",
            "aida queue list <args>",
            "bare `aida queue` defaults to the queue read view",
        ),
        row(
            "aida help --all",
            "aida help-all",
            "the full grouped command inventory",
        ),
        // TASK-862: personal-view shortcuts (pre-clap argv rewrites in main.rs:
        // `rewrite_personal_view_alias`).
        row(
            "aida mylist",
            "aida list me",
            "your specs — the ones you own or are assigned (shell identity)",
        ),
        row(
            "aida myqueue",
            "aida queue list",
            "your work queue (the queue is already user-scoped)",
        ),
    ];

    let mut groups = vec![
        AliasGroup {
            surface: "Status lenses".to_string(),
            about: "positional status shortcut on `aida list` (== `--status <x>`)".to_string(),
            rows: status_rows,
        },
        AliasGroup {
            surface: "List lenses".to_string(),
            about: "`aida list <lens>` views that rewrite to another command".to_string(),
            rows: list_rows,
        },
        AliasGroup {
            surface: "Command aliases".to_string(),
            about: "alternate spellings that reach the same command".to_string(),
            rows: command_rows,
        },
    ];

    // User-defined aliases (TASK-877), grouped by source. Shown to EVERY caller
    // (including agents — discoverability), even though only an interactive
    // human shell ever has them EXPANDED at the dispatch boundary. Each row is
    // marked with its scope so the source is unambiguous.
    // trace:TASK-877 | ai:claude
    let user_aliases = crate::user_alias::effective_user_aliases();
    if !user_aliases.is_empty() {
        let rows = user_aliases
            .iter()
            .map(|a| {
                row(
                    &format!("aida {}", a.name),
                    &format!("aida {}", a.expansion),
                    &format!("[{}] user-defined", a.scope.label()),
                )
            })
            .collect();
        groups.push(AliasGroup {
            surface: "User aliases".to_string(),
            about: "your personal/project aliases (project overrides personal); \
                    expanded only for an interactive human shell"
                .to_string(),
            rows,
        });
    }

    groups
}

/// Run `aida alias` / `aida alias list`. Prints the grouped registry of
/// built-in shortcuts, or the JSON form with `--json`. trace:STORY-667
pub fn run(json: bool) -> Result<()> {
    let groups = registry();

    if json {
        println!("{}", serde_json::to_string_pretty(&groups)?);
        return Ok(());
    }

    println!(
        "{}",
        "AIDA built-in shortcuts (run `aida alias --json` for machine output)".bold()
    );
    println!();
    for group in &groups {
        println!("{}  {}", group.surface.cyan().bold(), group.about.dimmed());
        // Column width for the alias column, capped so a long lens doesn't
        // blow out the layout.
        let width = group
            .rows
            .iter()
            .map(|r| r.alias.len())
            .max()
            .unwrap_or(0)
            .min(30);
        for r in &group.rows {
            println!(
                "  {:<width$}  {}  {}",
                r.alias.green(),
                format!("-> {}", r.expands_to).dimmed(),
                r.meaning,
                width = width,
            );
        }
        println!();
    }
    println!(
        "{} define your own with `aida alias add <name> <command...>` \
         (--global personal, --project shareable).",
        "Tip:".dimmed()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry's List-lenses group must agree with the live
    /// `rewrite_list_alias` resolver for every lens it advertises — drive both
    /// from `LIST_LENS_ALIASES` and prove the rewrite actually fires.
    /// trace:STORY-667
    #[test]
    fn list_lenses_agree_with_resolver() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        for a in crate::LIST_LENS_ALIASES {
            for tok in a.tokens {
                let out = crate::rewrite_list_alias(&s(&["aida", "list", tok]));
                let mut expected = vec!["aida".to_string()];
                expected.extend(a.canonical.iter().map(|c| c.to_string()));
                assert_eq!(
                    out, expected,
                    "list lens `{tok}` must rewrite to its canonical command"
                );
            }
        }
    }

    /// Every advertised status token must be accepted by the live recognizer
    /// (`RequirementStatus::from_filter_str`) — so the registry can't advertise
    /// a status shortcut the `aida list` path would reject. trace:STORY-667
    #[test]
    fn status_tokens_agree_with_recognizer() {
        for tok in STATUS_TOKENS {
            assert!(
                aida_core::RequirementStatus::from_filter_str(tok).is_some(),
                "status token `{tok}` must be a recognized status filter"
            );
        }
        // The `open` / `closed` aliases must also expand cleanly.
        for alias in ["open", "closed"] {
            assert!(
                aida_core::RequirementStatus::expand_filter_spec(alias).is_ok(),
                "status alias `{alias}` must expand"
            );
        }
    }

    /// The registry must cover the known lenses + status aliases — guards
    /// against a lens silently dropping out of the catalog.
    /// trace:STORY-667
    #[test]
    fn registry_covers_known_shortcuts() {
        let groups = registry();
        let all_aliases: Vec<String> = groups
            .iter()
            .flat_map(|g| g.rows.iter().map(|r| r.alias.clone()))
            .collect();
        let joined = all_aliases.join("\n");

        // Status lenses
        for needle in ["aida list open", "aida list closed", "aida list approved"] {
            assert!(joined.contains(needle), "registry missing `{needle}`");
        }
        // List lenses (incl. the human / me / user views)
        for needle in [
            "aida list queue",
            "aida list why",
            "aida list inflight",
            "aida list human",
            "aida list me",
            "aida list user:<name>",
        ] {
            assert!(joined.contains(needle), "registry missing `{needle}`");
        }
        // Command aliases (incl. the TASK-862 personal-view shortcuts)
        for needle in [
            "aida intake",
            "aida advisor assess",
            "aida agent <args>",
            "aida queue <args>",
            "aida mylist",
            "aida myqueue",
        ] {
            assert!(joined.contains(needle), "registry missing `{needle}`");
        }
    }

    /// The command-alias rows must agree with the live resolvers: the
    /// `advisor assess` -> `groom` rewrite, the bare `agent` -> `agent new`
    /// default, and clap's `intake` alias on the `Groom` variant (STORY-708).
    // trace:STORY-667 trace:STORY-708
    #[test]
    fn command_aliases_agree_with_resolvers() {
        use crate::cli::{Cli, Command};
        use clap::Parser;

        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

        // advisor assess -> groom (STORY-708: canonical disposition verb)
        assert_eq!(
            crate::rewrite_advisor_assess(&s(&["aida", "advisor", "assess"])),
            s(&["aida", "groom"]),
        );
        // bare `aida agent` -> `aida agent new`
        assert_eq!(
            crate::rewrite_agent_default_new(&s(&["aida", "agent"])),
            s(&["aida", "agent", "new"]),
        );
        // TASK-881: bare `aida queue` -> `aida queue list`
        assert_eq!(
            crate::rewrite_queue_default_list(&s(&["aida", "queue"])),
            s(&["aida", "queue", "list"]),
        );
        // clap `intake` alias reaches the Groom command (STORY-708: groom is
        // canonical; assess/intake are deprecated aliases)
        assert!(matches!(
            Cli::try_parse_from(["aida", "intake"]).unwrap().command,
            Command::Groom { .. }
        ));
        // TASK-862: the personal-view shortcuts must rewrite exactly as the
        // registry advertises (`mylist` -> `list me`, `myqueue` -> `queue list`).
        assert_eq!(
            crate::rewrite_personal_view_alias(&s(&["aida", "mylist"])),
            s(&["aida", "list", "me"]),
        );
        assert_eq!(
            crate::rewrite_personal_view_alias(&s(&["aida", "myqueue"])),
            s(&["aida", "queue", "list"]),
        );
    }

    /// `--json` output is valid JSON and round-trips the group shape.
    /// trace:STORY-667
    #[test]
    fn json_output_is_valid() {
        let groups = registry();
        let json = serde_json::to_string(&groups).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());
        assert!(parsed.as_array().unwrap().len() >= 3);
    }
}
