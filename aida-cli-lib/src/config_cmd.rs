//! `aida config` command cluster (SPIKE-78).
//!
//! The `aida config` surface: ID-format configuration + the effective-policy
//! renderer behind `config show`, the `tui`-gated `config menu` editor, the
//! `config glyph` theme commands, and `config user` / `config hints`. The
//! `CONFIG_KNOBS` registry is the single source of truth the show/menu/edit
//! surfaces derive from. Extracted verbatim from `main.rs`; no behavior change.

use anyhow::Result;
use colored::Colorize;

use crate::cli::{ConfigPermissionTier, ConfigPermissionsCommand};
use crate::*;
use toml_edit::{value, Array, DocumentMut, Item, Table};

/// Handle ID configuration commands
pub(crate) fn handle_config_command(cmd: &ConfigCommand, storage: &Storage) -> Result<()> {
    let mut store = storage.load()?;

    match cmd {
        ConfigCommand::Show { section } => {
            if let Some(section) = section {
                match section.as_str() {
                    "store.sync" => {
                        let project_root = store_sync_config_project_root(storage);
                        let cfg = read_store_sync_config(&project_root)?;
                        println!("{}", "Store Sync Configuration:".blue().bold());
                        println!("{}: {}", "auto_push".cyan(), cfg.auto_push.as_str());
                        println!(
                            "{}: {}",
                            "periodic_threshold".cyan(),
                            cfg.periodic_threshold
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "<unset>".to_string())
                        );
                        println!(
                            "{}: {}",
                            "periodic_interval".cyan(),
                            cfg.periodic_interval.as_deref().unwrap_or("<unset>")
                        );
                        println!("{}: {}", "source".cyan(), cfg.source);
                        if cfg.auto_push == StoreAutoPushMode::Periodic {
                            warn_if_periodic_auto_push(&project_root);
                        }
                        return Ok(());
                    }
                    other => {
                        anyhow::bail!("unknown config section `{}` (supported: store.sync)", other)
                    }
                }
            }
            println!("{}", "ID Configuration:".blue().bold());
            println!();

            let format_str = match store.id_config.format {
                IdFormat::SingleLevel => "Single-level (PREFIX-NNN)",
                IdFormat::TwoLevel => "Two-level (FEATURE-TYPE-NNN)",
            };
            println!("{}: {}", "Format".cyan(), format_str);

            let numbering_str = match store.id_config.numbering {
                NumberingStrategy::Global => "Global (one counter for all)",
                NumberingStrategy::PerPrefix => "Per-prefix (separate counter per prefix)",
                NumberingStrategy::PerFeatureType => "Per feature+type combination",
            };
            println!("{}: {}", "Numbering".cyan(), numbering_str);

            println!("{}: {}", "Digits".cyan(), store.id_config.digits);
            println!(
                "{}: {}",
                "Next global number".cyan(),
                store.next_spec_number
            );

            if !store.prefix_counters.is_empty() {
                println!("\n{}", "Prefix Counters:".blue());
                for (prefix, counter) in &store.prefix_counters {
                    println!("  {}: {}", prefix, counter);
                }
            }

            // BUG-533: ID config alone hides the whole effective-policy
            // surface (agent bypass posture, mailbox, advisor, archive,
            // telemetry, intake, presence). Render every known section with
            // its effective value + source so `config show` is the runtime
            // complement to docs/environment-variables.md.
            // trace:BUG-533
            let project_root = store_sync_config_project_root(storage);
            render_effective_policy(&project_root);
        }
        ConfigCommand::Format { format } => {
            store.id_config.format = match format.to_lowercase().as_str() {
                "single" | "single-level" | "1" => IdFormat::SingleLevel,
                "two" | "two-level" | "2" => IdFormat::TwoLevel,
                _ => anyhow::bail!("Invalid format. Use 'single' or 'two'."),
            };
            storage.save(&store)?;
            println!(
                "{} ID format set to {:?}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                store.id_config.format
            );
        }
        ConfigCommand::Numbering { strategy } => {
            store.id_config.numbering = match strategy.to_lowercase().as_str() {
                "global" => NumberingStrategy::Global,
                "per-prefix" | "prefix" => NumberingStrategy::PerPrefix,
                "per-feature-type" | "feature-type" => NumberingStrategy::PerFeatureType,
                _ => anyhow::bail!(
                    "Invalid strategy. Use 'global', 'per-prefix', or 'per-feature-type'."
                ),
            };
            storage.save(&store)?;
            println!(
                "{} Numbering strategy set to {:?}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                store.id_config.numbering
            );
        }
        ConfigCommand::Digits { digits } => {
            if *digits < 1 || *digits > 6 {
                anyhow::bail!("Digits must be between 1 and 6");
            }
            store.id_config.digits = *digits;
            storage.save(&store)?;
            println!(
                "{} ID digits set to {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                digits
            );
        }
        ConfigCommand::Migrate { yes } => {
            if !*yes {
                println!(
                    "{}",
                    "This will regenerate all requirement IDs based on current configuration."
                        .yellow()
                );
                println!("Current requirements: {}", store.requirements.len());
                // trace:STORY-809 | ai:claude
                let card = crate::context_prompt::ContextCard {
                    decision: "whether to regenerate every requirement ID under the current configuration".to_string(),
                    provenance: vec![format!(
                        "{} requirements are in the store; every spec_id may change",
                        store.requirements.len()
                    )],
                    answers: vec![
                        "y: IDs regenerate — existing trace: comments, commit trailers, and external references keep the OLD ids".to_string(),
                        "n: cancel; nothing changes".to_string(),
                    ],
                    recommended_default: "n — run only on a young store; on an established one this severs code-to-spec traces".to_string(),
                };
                let confirm = crate::context_prompt::confirm_with_context(
                    "Are you sure you want to migrate?",
                    false,
                    &card,
                )?;
                if !confirm {
                    println!("Migration cancelled.");
                    return Ok(());
                }
            }

            store.migrate_to_new_id_format();
            storage.save(&store)?;
            println!(
                "{} Successfully migrated {} requirements to new ID format.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                store.requirements.len()
            );
        }
        ConfigCommand::User {
            node_id,
            email,
            toml: emit_toml,
        } => {
            // trace:STORY-44 | ai:claude
            handle_config_user(node_id.as_deref(), email.as_deref(), *emit_toml)?;
        }
        ConfigCommand::Hints { enabled } => {
            // trace:STORY-106 | ai:claude
            handle_config_hints(enabled.as_deref(), storage)?;
        }
        // STORY-633: glyph commands are intercepted before storage init in
        // both dispatch paths, so they never reach this generic handler.
        // trace:STORY-633 | ai:claude
        ConfigCommand::Glyph(_) => {
            unreachable!("`aida config glyph` is dispatched before handle_config_command")
        }
        ConfigCommand::Permissions(cmd) => {
            // trace:STORY-1127 | ai:codex
            handle_config_permissions_command(cmd)?;
        }
        // STORY-661: `aida config menu` is intercepted before storage init in
        // the early-dispatch block, so it never reaches this generic handler.
        // trace:STORY-661 | ai:claude
        ConfigCommand::Menu => {
            unreachable!("`aida config menu` is dispatched before handle_config_command")
        }
    }

    Ok(())
}

/// Where an effective config value came from. Rendered beside each value by
/// `aida config show` so the operator can tell a deliberate override from an
/// inherited default at a glance.
// trace:BUG-533 | ai:claude
#[derive(Debug, Clone, Copy)]
enum PolicySource {
    /// No file or env set this — the built-in default is in force.
    Default,
    /// Set in the project's `.aida/config.toml`.
    ProjectConfig,
    /// Set in the project's `.aida/agents.toml`.
    ProjectAgents,
    /// Set in the global `~/.aida/agents.toml` (agent permission posture).
    GlobalAgents,
    /// Set in the global `~/.aida/config.toml` (user-wide default). STORY-620.
    GlobalConfig,
    /// Set in the project's `.codex/config.toml`.
    ProjectCodexConfig,
    /// Set in the global `~/.codex/config.toml`.
    GlobalCodexConfig,
    /// Set by project-local `.mcp.json` file presence.
    ProjectMcpJson,
    /// Overridden by an environment variable (named).
    Env(&'static str),
}

impl PolicySource {
    fn label(&self) -> String {
        match self {
            PolicySource::Default => "default".dimmed().to_string(),
            PolicySource::ProjectConfig => ".aida/config.toml".dimmed().to_string(),
            PolicySource::ProjectAgents => ".aida/agents.toml".dimmed().to_string(),
            PolicySource::GlobalAgents => "~/.aida/agents.toml".dimmed().to_string(),
            PolicySource::GlobalConfig => "~/.aida/config.toml".dimmed().to_string(),
            PolicySource::ProjectCodexConfig => ".codex/config.toml".dimmed().to_string(),
            PolicySource::GlobalCodexConfig => "~/.codex/config.toml".dimmed().to_string(),
            PolicySource::ProjectMcpJson => ".mcp.json".dimmed().to_string(),
            PolicySource::Env(name) => format!("{name} (env)").yellow().to_string(),
        }
    }

    /// Color-free scope label for the `aida config menu` TUI, which does its
    /// own styling.
    // trace:STORY-661 | ai:claude
    fn plain_label(&self) -> String {
        match self {
            PolicySource::Default => "default".to_string(),
            PolicySource::ProjectConfig => ".aida/config.toml".to_string(),
            PolicySource::ProjectAgents => ".aida/agents.toml".to_string(),
            PolicySource::GlobalAgents => "~/.aida/agents.toml".to_string(),
            PolicySource::GlobalConfig => "~/.aida/config.toml".to_string(),
            PolicySource::ProjectCodexConfig => ".codex/config.toml".to_string(),
            PolicySource::GlobalCodexConfig => "~/.codex/config.toml".to_string(),
            PolicySource::ProjectMcpJson => ".mcp.json".to_string(),
            PolicySource::Env(name) => format!("{name} (env)"),
        }
    }
}

/// One rendered policy row: a knob's effective value and where it resolved
/// from.
// trace:BUG-533 | ai:claude
struct PolicyRow {
    key: &'static str,
    value: String,
    source: PolicySource,
}

impl PolicyRow {
    fn print(&self) {
        println!(
            "  {}: {}  {}",
            self.key.cyan(),
            self.value,
            self.source.label()
        );
    }
}

/// One config section in the central policy registry: a `[section]` header and
/// the resolved knob rows under it.
///
/// TASK-793 (anti-drift, slice 2 of BUG-533): the registry — built by
/// [`policy_registry`] — is the single source of truth for which config knobs
/// `aida config show` renders. `render_effective_policy` iterates this list
/// rather than open-coding each section, so adding a knob is one registry entry
/// and it surfaces in `config show` automatically. The
/// [`KNOWN_CONFIG_SECTIONS`] const plus the `policy_registry_covers_*` tests are
/// the bouncer: a section read elsewhere in the codebase but absent from the
/// registry fails CI.
// trace:TASK-793 | ai:claude
struct PolicySection {
    /// The bare `[section]` name (no brackets) — the registry key matched
    /// against [`KNOWN_CONFIG_SECTIONS`] by the completeness test.
    section: &'static str,
    /// Bold header line shown above the rows (may add an inline gloss after the
    /// `[section]` token, e.g. "— agent permission posture").
    header: String,
    /// Resolved knob rows under this section.
    rows: Vec<PolicyRow>,
}

impl PolicySection {
    fn print(&self) {
        println!();
        println!("{}", self.header.bold());
        for row in &self.rows {
            row.print();
        }
    }
}

/// How a config knob may be edited from `aida config menu`, declared once per
/// knob in [`CONFIG_KNOBS`] (STORY-671 — the single source the editor, the
/// menu's `EditKind`, and the doc default all derive from). The value-type
/// carries the parse target the editor needs; [`EditSafety::ReadOnly`] knobs
/// (`id_format.*`, `deployment.*`, `agents.bypass`, `contained.*`, …) declare
/// *why* they are not live-editable so the menu can explain it.
///
/// This collapses the former hand-maintained `config_knob_meta` /
/// `config_knob_edit_kind` tables into the registry: an editable knob's type +
/// allowed set + range live with its doc + default in one declaration.
// trace:STORY-671 | ai:claude
#[derive(Clone, Copy)]
enum EditSafety {
    /// A boolean knob — the menu toggles it. Carries the built-in default.
    Bool { default: bool },
    /// An enum knob over a fixed allowed set — the menu cycles it. The first
    /// value is the built-in default.
    Enum { allowed: &'static [&'static str] },
    /// An integer knob over an inclusive `[min, max]` range — the menu prompts.
    Integer { min: i64, max: i64 },
    /// Not live-editable from the menu. `reason` is the short why shown on Enter
    /// (env-shadowed knobs are detected at resolve time and override this).
    ReadOnly { reason: &'static str },
}

/// One config knob, declared once. This is the **single source of truth**
/// STORY-671 consolidates to: `aida config show`'s rows, `aida config menu`'s
/// `EditKind`, the menu's per-knob default + explanation, `aida config edit`'s
/// validation, AND the anti-drift test all derive from this table. Adding a
/// knob is one entry here (plus its resolution branch in [`policy_registry`],
/// which renders the live value) — no separate `config_knob_doc` /
/// `config_knob_meta` / `KNOWN_CONFIG_SECTIONS` edits to forget.
// trace:STORY-671 | ai:claude
struct KnobSpec {
    /// The bare `[section]` name (no brackets).
    section: &'static str,
    /// The knob's bare key, or `"*"` for a section-wildcard entry whose doc +
    /// edit-safety apply to every key the resolver emits under that section
    /// (used where the key set is data-driven, e.g. `[seats]`, the `[team]`
    /// permission map). A concrete `(section, key)` entry always wins over the
    /// section wildcard.
    key: &'static str,
    /// One-line explanation (the framing `docs/environment-variables.md` uses).
    doc: &'static str,
    /// The built-in default shown in the menu (what you get with no config + no
    /// env override). String form so it renders uniformly across types.
    default: &'static str,
    /// Value-type + edit-safety: the one declaration the menu's `EditKind`, the
    /// editor's validation, and the read-only reason all derive from.
    edit: EditSafety,
}

/// The central config-knob registry — STORY-671's single source of truth. Each
/// knob declares its section/key, doc, default, and value-type + edit-safety
/// once. Every config surface derives from this table:
/// - `config show` rows are rendered by [`policy_registry`], whose section set
///   the anti-drift test asserts equals this table's sections;
/// - `config menu` rows take their default + explanation + `EditKind` from here
///   ([`config_knob_doc`], [`config_knob_edit_kind`]);
/// - `config edit` validation derives from the same [`EditSafety`]
///   ([`config_knob_meta`]).
///
/// A `key: "*"` entry is a section wildcard: its doc + edit-safety cover every
/// key the resolver emits under that section whose `(section, key)` is not
/// otherwise declared (used where the key set is data-driven). A concrete
/// `(section, key)` entry always takes precedence.
// trace:STORY-671 | ai:claude
const CONFIG_KNOBS: &[KnobSpec] = &[
    // --- [agents] — agent permission posture (read-only: security-relevant). ---
    KnobSpec {
        section: "agents",
        key: "bypass",
        doc: "Agent permission posture: native = Claude prompts (faithful launcher); bypass = agents skip permission prompts.",
        default: "native",
        edit: EditSafety::ReadOnly {
            reason: "security-relevant — edit ~/.aida/agents.toml deliberately",
        },
    },
    // trace:STORY-807 | ai:codex
    KnobSpec {
        section: "agents",
        key: "claude",
        doc: "Whether future AIDA scaffold/refresh passes manage the Claude Code profile.",
        default: "enabled",
        edit: EditSafety::Bool { default: true },
    },
    KnobSpec {
        section: "agents",
        key: "codex",
        doc: "Whether future AIDA scaffold/refresh passes manage the Codex profile.",
        default: "enabled",
        edit: EditSafety::Bool { default: true },
    },
    KnobSpec {
        section: "agents",
        key: "antigravity",
        doc: "Whether future AIDA scaffold/refresh passes manage the Antigravity profile.",
        default: "enabled",
        edit: EditSafety::Bool { default: true },
    },
    // --- [contained] — sandbox + egress posture (read-only: security-relevant). ---
    KnobSpec {
        section: "contained",
        key: "enable",
        doc: "Sandbox posture: run agents under Claude Code's native --settings sandbox.",
        default: "disabled",
        edit: EditSafety::ReadOnly {
            reason: "sandbox posture — edit .aida/config.toml deliberately",
        },
    },
    KnobSpec {
        section: "contained",
        key: "allowed_hosts",
        doc: "Egress allowlist for sandboxed agents; empty means no egress restriction.",
        default: "(none)",
        edit: EditSafety::ReadOnly {
            reason: "egress allowlist — edit .aida/config.toml deliberately",
        },
    },
    KnobSpec {
        section: "contained",
        key: "os_wrap",
        doc: "The bwrap OS-sandbox master switch (distinct from `enable`); strictly opt-in.",
        default: "false",
        edit: EditSafety::ReadOnly {
            reason: "OS-sandbox switch — edit .aida/config.toml deliberately",
        },
    },
    KnobSpec {
        section: "contained",
        key: "read_allowlist",
        doc: "Strict read-confinement paths under os_wrap; empty binds the host root read-only.",
        default: "(none)",
        edit: EditSafety::ReadOnly {
            reason: "read confinement — edit .aida/config.toml deliberately",
        },
    },
    KnobSpec {
        section: "contained",
        key: "managed_domains_only",
        doc: "Hard egress deny (managed set + allowed_hosts only), no approval prompt.",
        default: "false",
        edit: EditSafety::ReadOnly {
            reason: "egress deny — edit .aida/config.toml deliberately",
        },
    },
    // --- [permissions] — computed per-agent launch posture, menu-editable via
    // `config permissions set` (STORY-1131). ---
    // trace:STORY-1127 trace:STORY-1131 | ai:codex
    KnobSpec {
        section: "permissions",
        key: "claude",
        doc: "Computed effective Claude launch permission tier; cycle to write the selected posture.",
        default: "native",
        edit: EditSafety::Enum {
            allowed: &["native", "contained", "bypass"],
        },
    },
    KnobSpec {
        section: "permissions",
        key: "codex",
        doc: "Computed effective Codex launch permission tier; cycle to write the selected posture.",
        default: "native",
        edit: EditSafety::Enum {
            allowed: &["native", "contained", "bypass"],
        },
    },
    KnobSpec {
        section: "permissions",
        key: "antigravity",
        doc: "Computed effective Antigravity launch permission tier; cycle to write the selected posture.",
        default: "native",
        edit: EditSafety::Enum {
            allowed: &["native", "contained", "bypass"],
        },
    },
    // --- [mcp] — derived project-local AIDA MCP registration. ---
    // trace:STORY-1131 | ai:codex
    KnobSpec {
        section: "mcp",
        key: "aida_registered",
        doc: "Whether this project has a local `.mcp.json` AIDA MCP server registration.",
        default: "no",
        edit: EditSafety::Bool { default: false },
    },
    // --- [burndown]. ---
    KnobSpec {
        section: "burndown",
        key: "verbose",
        doc: "Default visibility for `aida burndown run`: stream live drain progress unless `--quiet` is passed.",
        default: "false",
        edit: EditSafety::Bool { default: false },
    },
    // trace:TASK-1172 trace:TASK-1175
    KnobSpec {
        section: "burndown",
        key: "order",
        doc: "How the ready set is ordered before a wave is fanned: by priority (high first, ties by queue-insertion time), or strict queue-insertion order (oldest queued first).",
        default: "priority",
        edit: EditSafety::Enum {
            allowed: &["priority", "queue"],
        },
    },
    // --- [mailbox]. ---
    KnobSpec {
        section: "mailbox",
        key: "act_on_mail",
        doc: "How a session reacts to unread mail: surface-and-recommend, or escalate-per-cascade.",
        default: "surface-and-recommend",
        edit: EditSafety::Enum {
            allowed: &["surface-and-recommend", "escalate-per-cascade"],
        },
    },
    KnobSpec {
        section: "mailbox",
        key: "autosync",
        doc: "Auto-publish the local mailbox on the pull/push store legs (env: AIDA_MAILBOX_AUTOSYNC).",
        default: "true",
        edit: EditSafety::ReadOnly {
            reason: "resolution involves AIDA_MAILBOX_AUTOSYNC — set via config.toml or env",
        },
    },
    // --- [advisor]. ---
    KnobSpec {
        section: "advisor",
        key: "calibration_mode",
        doc: "When on, every advisor punt emits two verdicts to mine substrate gaps (cost: both runs fire).",
        default: "off",
        edit: EditSafety::ReadOnly {
            reason: "doubles drain cost — set deliberately in .aida/config.toml",
        },
    },
    // --- [review]. trace:STORY-1415 | ai:claude ---
    KnobSpec {
        section: "review",
        key: "mass_change_mode",
        doc: "Temporary mass-change review mode: acceptance gaps deferred, not blocking, until its window closes.",
        default: "off",
        edit: EditSafety::ReadOnly {
            reason: "toggle with `aida review mode mass-change on|off` — the on-verb stamps the clock",
        },
    },
    // --- [protocol]. trace:TASK-1290 ---
    KnobSpec {
        section: "protocol",
        key: "enforce",
        doc: "Untraced acceptance criteria at `aida queue done`: warn (default) or refuse.",
        default: "warn",
        edit: EditSafety::Enum {
            allowed: &["warn", "refuse"],
        },
    },
    // --- [archive]. ---
    KnobSpec {
        section: "archive",
        key: "auto_after_days",
        doc: "Auto-sweep completed/rejected specs older than N days on `aida pull` (clamped >=7; env: AIDA_AUTO_ARCHIVE).",
        default: "disabled",
        edit: EditSafety::Integer { min: 7, max: 365 },
    },
    // --- [list]. trace:BUG-783 ---
    KnobSpec {
        section: "list",
        key: "show_hidden_hints",
        doc: "Footer the archived/deferred hidden-count nudges on the default `aida list` view (off = quiet).",
        default: "off",
        edit: EditSafety::Bool { default: false },
    },
    // --- [telemetry]. ---
    KnobSpec {
        section: "telemetry",
        key: "enabled",
        doc: "Local usage telemetry at ~/.aida/usage.jsonl (env: AIDA_TELEMETRY; never phoned home).",
        default: "enabled",
        edit: EditSafety::Bool { default: true },
    },
    // --- [field_study] (SPIKE-67) — the formerly-drifted knob (STORY-671 #3). ---
    KnobSpec {
        section: "field_study",
        key: "enabled",
        doc: "Observe-only rule-adherence field study log at .aida (env: AIDA_FIELD_STUDY; honors AIDA_TELEMETRY=0).",
        default: "disabled",
        edit: EditSafety::Bool { default: false },
    },
    // --- [intake]. ---
    KnobSpec {
        section: "intake",
        key: "disposition_bias",
        doc: "Headless advisor INTAKE pass bias when proposing approve/reject/park/queue per open spec.",
        default: "(built-in)",
        edit: EditSafety::ReadOnly {
            reason: "INTAKE policy — set deliberately in .aida/config.toml",
        },
    },
    KnobSpec {
        section: "intake",
        key: "on_apply",
        doc: "What an `aida intake --apply` pass executes for each proposed disposition.",
        default: "(built-in)",
        edit: EditSafety::ReadOnly {
            reason: "INTAKE policy — set deliberately in .aida/config.toml",
        },
    },
    KnobSpec {
        section: "intake",
        key: "do_not_approve_classes",
        doc: "Spec classes the INTAKE pass will never auto-approve.",
        default: "(built-in)",
        edit: EditSafety::ReadOnly {
            reason: "INTAKE policy — set deliberately in .aida/config.toml",
        },
    },
    // --- [ultraplan]. ---
    KnobSpec {
        section: "ultraplan",
        key: "mode",
        doc: "Whether AIDA proactively suggests `aida ultraplan <SPEC>`: never / on-demand / suggested.",
        default: "on-demand",
        edit: EditSafety::Enum {
            allowed: &["never", "on-demand", "suggested"],
        },
    },
    // --- [presence] (STORY-561). ---
    KnobSpec {
        section: "presence",
        key: "consumers",
        doc: "Whether presence-aware consumers act on the away/home signal.",
        default: "on",
        edit: EditSafety::ReadOnly {
            reason: "presence wiring — set deliberately in .aida/config.toml",
        },
    },
    KnobSpec {
        section: "presence",
        key: "away_drain",
        doc: "Drain mode used while you are away (e.g. headless-both).",
        default: "headless-both",
        edit: EditSafety::Enum {
            allowed: &[
                "headless-both",
                "headless-escalate-defaults",
                "headless-park",
            ],
        },
    },
    KnobSpec {
        section: "presence",
        key: "home_offer",
        doc: "What presence offers when you return home.",
        default: "surface",
        edit: EditSafety::Enum {
            allowed: &["surface", "dont-block"],
        },
    },
    // --- [hints]. ---
    KnobSpec {
        section: "hints",
        key: "workflow_hints",
        doc: "Inline state-transition hints (queue drained -> open PR, etc.); env: AIDA_HINTS overrides per-shell.",
        default: "enabled",
        edit: EditSafety::Bool { default: true },
    },
    // --- [ui] — glyph profile + theme. ---
    KnobSpec {
        section: "ui",
        key: "glyphs",
        doc: "Active glyph profile (unicode / ascii / ...); env: AIDA_GLYPHS overrides.",
        default: "unicode",
        edit: EditSafety::ReadOnly {
            reason: "use `aida config glyph` to change the glyph profile",
        },
    },
    KnobSpec {
        section: "ui",
        key: "theme",
        doc: "Named glyph theme applied on top of the profile.",
        default: "(none)",
        edit: EditSafety::ReadOnly {
            reason: "use `aida config glyph` to change the theme",
        },
    },
    // --- [seats] (STORY-620) — data-driven key set; section wildcard. ---
    KnobSpec {
        section: "seats",
        key: "*",
        doc: "Which seat (operator `aida human` vs advisor `aida advisor`) this configurable bucket shows on.",
        default: "(per-key default)",
        edit: EditSafety::ReadOnly {
            reason: "seat routing — set deliberately in .aida/config.toml",
        },
    },
    // --- [team] (STORY-647) — RBAC guardrail; concrete rows + permission-map wildcard. ---
    KnobSpec {
        section: "team",
        key: "strict",
        doc: "RBAC guardrail strict mode (NOT security): non-rostered = least-privilege, refusals roster-authoritative.",
        default: "false",
        edit: EditSafety::ReadOnly {
            reason: "RBAC guardrail — set deliberately in .aida/config.toml",
        },
    },
    KnobSpec {
        section: "team",
        key: "protected_tags",
        doc: "Tags whose specs require the protected role to modify (guardrail, not access control).",
        default: "(none)",
        edit: EditSafety::ReadOnly {
            reason: "RBAC guardrail — set deliberately in .aida/config.toml",
        },
    },
    KnobSpec {
        section: "team",
        key: "protected_role",
        doc: "Minimum role required to modify a protected-tag spec.",
        default: "(default)",
        edit: EditSafety::ReadOnly {
            reason: "RBAC guardrail — set deliberately in .aida/config.toml",
        },
    },
    KnobSpec {
        section: "team",
        key: "*",
        doc: "Per-operation minimum role in the RBAC guardrail permission map.",
        default: "(default)",
        edit: EditSafety::ReadOnly {
            reason: "RBAC guardrail — set deliberately in .aida/config.toml",
        },
    },
];

/// Look up a knob's declaration in [`CONFIG_KNOBS`]: an exact `(section, key)`
/// match wins; failing that, the `(section, "*")` section-wildcard entry covers
/// data-driven key sets. `None` when the section is undeclared entirely.
// trace:STORY-671 | ai:claude
fn config_knob_spec(section: &str, key: &str) -> Option<&'static KnobSpec> {
    CONFIG_KNOBS
        .iter()
        .find(|k| k.section == section && k.key == key)
        .or_else(|| {
            CONFIG_KNOBS
                .iter()
                .find(|k| k.section == section && k.key == "*")
        })
}

/// Every config section `aida config show` is expected to render — DERIVED from
/// [`CONFIG_KNOBS`] (STORY-671), not a hand-maintained list. This is the drift
/// tripwire's known-section set: each `[section]` AIDA reads from
/// `.aida/config.toml`, `~/.aida/agents.toml`, or `~/.aida/config.toml` is
/// declared by at least one [`KnobSpec`], and the anti-drift test asserts
/// [`policy_registry`] emits exactly these. A new knob in a brand-new section
/// shows up here automatically the moment its `KnobSpec` is declared — there is
/// no separate list to forget. Consumed by the anti-drift tests (the only
/// caller; `policy_registry` enumerates the resolvers directly).
// trace:STORY-671 trace:TASK-793 | ai:claude
#[cfg(test)]
fn known_config_sections() -> Vec<&'static str> {
    let mut seen = Vec::new();
    for knob in CONFIG_KNOBS {
        if !seen.contains(&knob.section) {
            seen.push(knob.section);
        }
    }
    seen
}

/// Render the full effective policy surface for `aida config show`. Iterates the
/// central [`policy_registry`] — each registered section is shown with its
/// resolved value + source (default / project `.aida/config.toml` / global
/// `~/.aida/agents.toml` / global `~/.aida/config.toml` / env). This is the
/// runtime complement to `docs/environment-variables.md`.
///
/// Anti-drift (BUG-533 slice 2 / TASK-793): this renderer no longer hardcodes
/// the section list — it walks whatever [`policy_registry`] returns, so a knob
/// added there surfaces here for free. [`KNOWN_CONFIG_SECTIONS`] +
/// `policy_registry_covers_known_sections` keep the registry from silently
/// falling behind a newly-added config section.
// trace:TASK-793 trace:BUG-533 | ai:claude
fn render_effective_policy(project_root: &std::path::Path) {
    println!();
    println!("{}", "Effective Policy:".blue().bold());

    for section in policy_registry(project_root) {
        section.print();
    }

    println!();
    println!(
        "  {}",
        "Override any AIDA_* env var per docs/environment-variables.md.".dimmed()
    );
}

// trace:STORY-1127 | ai:codex
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PermissionPostureReport {
    pub agents: Vec<PermissionPostureRow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<PermissionPostureFinding>,
}

// trace:STORY-1127 | ai:codex
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PermissionPostureRow {
    pub agent: String,
    pub tier: String,
    pub tier_source: String,
    pub flags: Vec<String>,
    pub interactive_flags: Vec<String>,
    pub headless_flags: Vec<String>,
    pub flags_source: String,
    pub prompts: String,
    pub interactive_prompts: String,
    pub headless_prompts: String,
    pub sandboxed: String,
    pub network: String,
    pub writable_roots: Vec<String>,
    pub net_effect: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<PermissionPostureFinding>,
}

// trace:STORY-1127 | ai:codex
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PermissionPostureFinding {
    pub severity: String,
    pub id: String,
    pub agent: String,
    pub summary: String,
    pub source: String,
}

#[derive(Debug, Clone)]
struct ScopedValue<T> {
    value: T,
    source: PolicySource,
}

#[derive(Debug, Clone, Default)]
struct CodexConfigPosture {
    sandbox_mode: Option<ScopedValue<String>>,
    approval_policy: Option<ScopedValue<String>>,
    workspace_table_source: Option<PolicySource>,
    network_access: Option<ScopedValue<bool>>,
    writable_roots: Option<ScopedValue<Vec<String>>>,
}

/// `aida config permissions show` — read-only effective permission posture.
// trace:STORY-1127 | ai:codex
pub(crate) fn handle_config_permissions_command(cmd: &ConfigPermissionsCommand) -> Result<()> {
    match cmd {
        ConfigPermissionsCommand::Show { json } => {
            let project_root = main_worktree_root_from(&find_project_root()?);
            let report = permission_posture_report(&project_root);
            if *json || output_format_is_json() || aida_agent_output_truthy() {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                render_permission_posture_report(&report);
            }
        }
        ConfigPermissionsCommand::Set { tier, user, local } => {
            let project_root = main_worktree_root_from(&find_project_root()?);
            let scope = PermissionPostureScope::from_flags(*user, *local)?;
            let result = apply_permission_posture(&project_root, *tier, scope)?;
            if *tier == ConfigPermissionTier::Bypass {
                println!(
                    "{}",
                    "WARNING: bypass writes a nuclear full-access default; use only as an explicit operator opt-in."
                        .red()
                        .bold()
                );
            }
            println!(
                "{} permission posture set to {:?} ({})",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                tier,
                scope.label()
            );
            for path in result.edited_paths {
                println!("  wrote {}", path.display());
            }
            for path in result.backup_paths {
                println!("  backup {}", path.display());
            }
        }
    }
    Ok(())
}

// trace:STORY-1128 | ai:codex
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PermissionPostureScope {
    Local,
    User,
}

impl PermissionPostureScope {
    fn from_flags(user: bool, _local: bool) -> Result<Self> {
        if user {
            Ok(Self::User)
        } else {
            Ok(Self::Local)
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Local => "project-local",
            Self::User => "user-global",
        }
    }
}

// trace:STORY-1128 | ai:codex
#[derive(Debug, Clone)]
pub(crate) struct PermissionPostureWriteResult {
    pub edited_paths: Vec<std::path::PathBuf>,
    pub backup_paths: Vec<std::path::PathBuf>,
}

// trace:STORY-1128 | ai:codex
pub(crate) fn apply_permission_posture(
    project_root: &std::path::Path,
    tier: ConfigPermissionTier,
    scope: PermissionPostureScope,
) -> Result<PermissionPostureWriteResult> {
    let paths = permission_posture_paths(project_root, scope)?;
    let mut edited_paths = Vec::new();
    let mut backup_paths = Vec::new();

    backup_paths.push(backup_file(&paths.agents)?);
    write_agents_posture(&paths.agents, tier)?;
    edited_paths.push(paths.agents);

    backup_paths.push(backup_file(&paths.codex)?);
    write_codex_posture(&paths.codex, tier)?;
    edited_paths.push(paths.codex);

    Ok(PermissionPostureWriteResult {
        edited_paths,
        backup_paths,
    })
}

#[derive(Debug, Clone)]
struct PermissionPosturePaths {
    agents: std::path::PathBuf,
    codex: std::path::PathBuf,
}

fn permission_posture_paths(
    project_root: &std::path::Path,
    scope: PermissionPostureScope,
) -> Result<PermissionPosturePaths> {
    match scope {
        PermissionPostureScope::Local => Ok(PermissionPosturePaths {
            agents: project_root.join(".aida/agents.toml"),
            codex: project_root.join(".codex/config.toml"),
        }),
        PermissionPostureScope::User => {
            let home = aida_home_dir().context("cannot resolve home directory")?;
            Ok(PermissionPosturePaths {
                agents: home.join(".aida/agents.toml"),
                codex: home.join(".codex/config.toml"),
            })
        }
    }
}

fn backup_file(path: &std::path::Path) -> Result<std::path::PathBuf> {
    let backup = backup_path(path);
    if let Some(parent) = backup.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    match std::fs::read(path) {
        Ok(bytes) => aida_core::write_atomic(&backup, bytes)
            .with_context(|| format!("failed to write {}", backup.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            aida_core::write_atomic(&backup, b"")
                .with_context(|| format!("failed to write {}", backup.display()))?;
        }
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
    Ok(backup)
}

fn backup_path(path: &std::path::Path) -> std::path::PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config");
    path.with_file_name(format!("{name}.bak"))
}

fn load_edit_doc(path: &std::path::Path) -> Result<DocumentMut> {
    match std::fs::read_to_string(path) {
        Ok(body) => body
            .parse::<DocumentMut>()
            .with_context(|| format!("failed to parse {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn save_edit_doc(path: &std::path::Path, doc: &DocumentMut) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    aida_core::write_atomic(path, doc.to_string())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn write_agents_posture(path: &std::path::Path, tier: ConfigPermissionTier) -> Result<()> {
    let mut doc = load_edit_doc(path)?;
    let agents = ensure_root_table(&mut doc, "agents");
    match tier {
        ConfigPermissionTier::Contained => {
            agents["contained"] = value(true);
            agents["bypass"] = value(false);
            let codex = ensure_child_table(agents, "codex");
            codex["default_flags"] = value(string_array([
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "never",
            ]));
        }
        ConfigPermissionTier::Native => {
            agents.remove("contained");
            agents.remove("bypass");
            if let Some(codex) = agents.get_mut("codex").and_then(Item::as_table_mut) {
                codex.remove("default_flags");
                if codex.is_empty() {
                    agents.remove("codex");
                }
            }
        }
        ConfigPermissionTier::Bypass => {
            agents["contained"] = value(false);
            agents["bypass"] = value(true);
            let codex = ensure_child_table(agents, "codex");
            codex["default_flags"] =
                value(string_array(["--dangerously-bypass-approvals-and-sandbox"]));
        }
    }
    save_edit_doc(path, &doc)
}

fn write_codex_posture(path: &std::path::Path, tier: ConfigPermissionTier) -> Result<()> {
    let mut doc = load_edit_doc(path)?;
    match tier {
        ConfigPermissionTier::Contained => {
            doc["sandbox_mode"] = value("workspace-write");
            doc["approval_policy"] = value("never");
            let sandbox = ensure_root_table(&mut doc, "sandbox_workspace_write");
            sandbox["network_access"] = value(true);
            sandbox["writable_roots"] = value(string_array(["~/.cargo", "~/.rustup", "~/.aida"]));
        }
        ConfigPermissionTier::Native => {
            doc.remove("sandbox_mode");
            doc.remove("approval_policy");
            doc.remove("sandbox_workspace_write");
        }
        ConfigPermissionTier::Bypass => {
            doc["sandbox_mode"] = value("danger-full-access");
            doc["approval_policy"] = value("never");
            doc.remove("sandbox_workspace_write");
        }
    }
    save_edit_doc(path, &doc)?;
    verify_codex_posture_file(path)
}

fn ensure_root_table<'a>(doc: &'a mut DocumentMut, key: &str) -> &'a mut Table {
    if !doc.contains_table(key) {
        doc.insert(key, Item::Table(Table::new()));
    }
    doc[key]
        .as_table_mut()
        .expect("just-inserted/confirmed table")
}

fn ensure_child_table<'a>(table: &'a mut Table, key: &str) -> &'a mut Table {
    if !table.contains_table(key) {
        table.insert(key, Item::Table(Table::new()));
    }
    table[key]
        .as_table_mut()
        .expect("just-inserted/confirmed child table")
}

fn string_array<const N: usize>(items: [&str; N]) -> Array {
    let mut array = Array::default();
    for item in items {
        array.push(item);
    }
    array
}

fn verify_codex_posture_file(path: &std::path::Path) -> Result<()> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let parsed: toml::Value =
        toml::from_str(&body).with_context(|| format!("failed to parse {}", path.display()))?;
    if body.contains("[sandbox_workspace_write]") {
        anyhow::ensure!(
            parsed
                .get("sandbox_mode")
                .and_then(|v| v.as_str())
                .is_some(),
            "Codex config root sandbox_mode was captured by a table in {}",
            path.display()
        );
        anyhow::ensure!(
            parsed
                .get("approval_policy")
                .and_then(|v| v.as_str())
                .is_some(),
            "Codex config root approval_policy was captured by a table in {}",
            path.display()
        );
    }
    Ok(())
}

fn aida_agent_output_truthy() -> bool {
    std::env::var("AIDA_AGENT_OUTPUT")
        .ok()
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off" | "human"
            )
        })
        .unwrap_or(false)
}

/// Doctor bridge: convert permission-posture warnings into normal doctor
/// findings so `aida doctor --category permission-posture` exits non-zero.
// trace:STORY-1127 | ai:codex
pub(crate) fn scan_permission_posture_findings(
    project_root: &std::path::Path,
) -> Vec<DoctorFinding> {
    permission_posture_report(project_root)
        .findings
        .into_iter()
        .map(|f| DoctorFinding {
            category: "permission-posture".to_string(),
            id: f.id,
            summary: format!("{}: {} ({})", f.agent, f.summary, f.severity),
            action: format!(
                "apply contained posture with `aida config permissions set contained`; current source: {}",
                f.source
            ),
            safe_heal: true,
        })
        .collect()
}

// trace:TASK-1233 | ai:codex
pub(crate) fn maybe_offer_permission_posture_fix(project_root: &std::path::Path) -> Result<()> {
    let report = permission_posture_report(project_root);
    if report.findings.is_empty() {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Ok(());
    }

    eprintln!();
    eprintln!("{}", "Project setup — agent permission posture".bold());
    for finding in &report.findings {
        eprintln!(
            "  {} {}: {} ({})",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            finding.agent.cyan(),
            finding.summary,
            finding.source.dimmed()
        );
    }
    let accepted = crate::prompt_yes_no(
        "  Set contained posture now (project-local agents + Codex sandbox)? [Y/n]: ",
        true,
    )?;
    if !accepted {
        eprintln!(
            "  {} left permission posture unchanged; run {} anytime.",
            "Note:".dimmed(),
            "aida config permissions set contained".cyan()
        );
        return Ok(());
    }

    let result = apply_permission_posture(
        project_root,
        ConfigPermissionTier::Contained,
        PermissionPostureScope::Local,
    )?;
    eprintln!("  {} contained permission posture written.", "+".green());
    for path in result.edited_paths {
        eprintln!("    wrote {}", path.display());
    }
    Ok(())
}

// trace:STORY-1127 | ai:codex
pub(crate) fn permission_posture_report(project_root: &std::path::Path) -> PermissionPostureReport {
    let codex = codex_config_posture(project_root);
    let mut agents = Vec::new();
    for agent in ["claude", "codex", "antigravity"] {
        agents.push(permission_posture_for_agent(project_root, agent, &codex));
    }
    let findings = agents
        .iter()
        .flat_map(|a| a.findings.iter().cloned())
        .collect();
    PermissionPostureReport { agents, findings }
}

fn render_permission_posture_report(report: &PermissionPostureReport) {
    println!("{}", "Permission Posture:".blue().bold());
    println!(
        "  {}",
        "Read-only view of AIDA agent launch defaults and native Codex sandbox config.".dimmed()
    );
    println!();
    println!(
        "  {:<12} {:<10} {:<24} {:<12} {:<12} {}",
        "agent".cyan(),
        "tier".cyan(),
        "flags".cyan(),
        "prompts".cyan(),
        "sandbox".cyan(),
        "net effect".cyan()
    );
    for row in &report.agents {
        println!(
            "  {:<12} {:<10} {:<24} {:<12} {:<12} {}",
            row.agent,
            row.tier,
            truncate_middle(&row.flags.join(" "), 24),
            row.prompts,
            row.sandboxed,
            row.net_effect
        );
        println!(
            "    launch: interactive flags={} prompts={} · headless flags={} prompts={}",
            truncate_middle(&row.interactive_flags.join(" "), 72).dimmed(),
            row.interactive_prompts.dimmed(),
            truncate_middle(&row.headless_flags.join(" "), 72).dimmed(),
            row.headless_prompts.dimmed()
        );
        println!(
            "    scope: tier={} · flags={} · network={} · writable_roots={}",
            row.tier_source.dimmed(),
            row.flags_source.dimmed(),
            row.network.dimmed(),
            if row.writable_roots.is_empty() {
                "(none)".dimmed().to_string()
            } else {
                row.writable_roots.join(", ").dimmed().to_string()
            }
        );
        for finding in &row.findings {
            let sev = if finding.severity == "high" {
                finding.severity.red().bold().to_string()
            } else {
                finding.severity.yellow().to_string()
            };
            println!(
                "    {} {}: {}",
                "finding".yellow().bold(),
                sev,
                finding.summary
            );
        }
    }
    if report.findings.is_empty() {
        println!();
        println!(
            "  {} no dangerous permission-posture inconsistencies detected",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    }
}

fn truncate_middle(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let left = keep / 2;
    let right = keep.saturating_sub(left);
    let start: String = s.chars().take(left).collect();
    let end: String = s
        .chars()
        .rev()
        .take(right)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{start}…{end}")
}

fn permission_posture_for_agent(
    project_root: &std::path::Path,
    agent: &str,
    codex: &CodexConfigPosture,
) -> PermissionPostureRow {
    let per_tool = agent_default_flags_with_source(project_root, agent);
    let bypass = agents_bool_with_source(project_root, "bypass").unwrap_or(ScopedValue {
        value: false,
        source: PolicySource::Default,
    });
    let contained = agents_contained_with_source(project_root).unwrap_or(ScopedValue {
        value: false,
        source: PolicySource::Default,
    });

    let (tier, tier_source, flags, flags_source) = if let Some(flags) = per_tool {
        let tier = if flags.value.iter().any(|f| nuclear_permission_flag(f)) {
            "bypass"
        } else if flags.value.iter().any(|f| contained_permission_flag(f)) {
            "contained"
        } else {
            "native"
        };
        (
            tier.to_string(),
            flags.source.plain_label(),
            flags.value,
            flags.source.plain_label(),
        )
    } else if contained.value {
        (
            "contained".to_string(),
            contained.source.plain_label(),
            display_tool_contained_flags(agent),
            contained.source.plain_label(),
        )
    } else if bypass.value {
        (
            "bypass".to_string(),
            bypass.source.plain_label(),
            display_tool_bypass_flags(agent),
            bypass.source.plain_label(),
        )
    } else {
        (
            "native".to_string(),
            PolicySource::Default.plain_label(),
            Vec::new(),
            PolicySource::Default.plain_label(),
        )
    };

    let (interactive_flags, headless_flags) = launch_kind_permission_flags(agent, &tier, &flags);
    let interactive_prompts = prompts_for_agent_flags(agent, &tier, &interactive_flags, codex);
    let headless_prompts = prompts_for_agent_flags(agent, &tier, &headless_flags, codex);

    let mut findings = Vec::new();
    let codex_sandbox_mode = codex
        .sandbox_mode
        .as_ref()
        .map(|v| v.value.as_str())
        .unwrap_or("native-default");
    let codex_approval = codex
        .approval_policy
        .as_ref()
        .map(|v| v.value.as_str())
        .unwrap_or("native-default");
    let codex_has_workspace_table = codex.workspace_table_source.is_some();
    let network = if agent == "codex" {
        match &codex.network_access {
            Some(v) => format!("{} ({})", v.value, v.source.plain_label()),
            None => "native-default".to_string(),
        }
    } else {
        "native/tool-specific".to_string()
    };
    let writable_roots = if agent == "codex" {
        codex
            .writable_roots
            .as_ref()
            .map(|v| v.value.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let prompts = if interactive_prompts == headless_prompts {
        interactive_prompts.clone()
    } else {
        format!("interactive {interactive_prompts}; headless {headless_prompts}")
    };
    let sandboxed = if tier == "bypass" {
        "no".to_string()
    } else if tier == "contained" {
        if agent == "codex" && !codex_has_workspace_table {
            "intended".to_string()
        } else {
            "yes".to_string()
        }
    } else if agent == "codex" {
        match codex_sandbox_mode {
            "danger-full-access" => "no".to_string(),
            "workspace-write" | "read-only" => "yes".to_string(),
            _ => "native".to_string(),
        }
    } else {
        "native".to_string()
    };

    if tier == "bypass" && !codex_has_workspace_table {
        findings.push(PermissionPostureFinding {
            severity: "high".to_string(),
            id: format!("{agent}-full-access-no-sandbox"),
            agent: agent.to_string(),
            summary:
                "FULL ACCESS: bypass posture with no Codex sandbox_workspace_write table visible"
                    .to_string(),
            source: tier_source.clone(),
        });
    }
    if tier == "contained" && !codex_has_workspace_table {
        findings.push(PermissionPostureFinding {
            severity: "high".to_string(),
            id: format!("{agent}-contained-missing-codex-sandbox-table"),
            agent: agent.to_string(),
            summary:
                "contained posture is configured, but Codex lacks [sandbox_workspace_write]; network/cargo may silently break"
                    .to_string(),
            source: tier_source.clone(),
        });
    }
    if agent == "codex"
        && codex.sandbox_mode.is_some()
        && flags.iter().any(|f| nuclear_permission_flag(f))
    {
        findings.push(PermissionPostureFinding {
            severity: "high".to_string(),
            id: "codex-sandbox-overridden-by-default-flags".to_string(),
            agent: agent.to_string(),
            summary:
                "Codex sandbox_mode is set, but nuclear default_flags override it with full access"
                    .to_string(),
            source: flags_source.clone(),
        });
    }

    let net_effect = if tier == "bypass" {
        "no prompts; unsandboxed".to_string()
    } else if tier == "contained" {
        "AIDA-contained launch".to_string()
    } else if agent == "codex" {
        format!("codex sandbox={codex_sandbox_mode}; approval={codex_approval}")
    } else {
        "tool native defaults".to_string()
    };

    PermissionPostureRow {
        agent: agent.to_string(),
        tier,
        tier_source,
        flags,
        interactive_flags,
        headless_flags,
        flags_source,
        prompts,
        interactive_prompts,
        headless_prompts,
        sandboxed,
        network,
        writable_roots,
        net_effect,
        findings,
    }
}

fn display_tool_bypass_flags(agent: &str) -> Vec<String> {
    match agent {
        "claude" => vec!["--permission-mode".into(), "bypassPermissions".into()],
        "codex" => vec!["--dangerously-bypass-approvals-and-sandbox".into()],
        "antigravity" => vec!["--dangerously-skip-permissions".into()],
        _ => Vec::new(),
    }
}

fn display_tool_contained_flags(agent: &str) -> Vec<String> {
    match agent {
        "claude" => vec!["--settings".into(), "<contained-settings>".into()],
        "codex" => vec![
            "--sandbox".into(),
            "workspace-write".into(),
            "--ask-for-approval".into(),
            "never".into(),
        ],
        _ => Vec::new(),
    }
}

// trace:BUG-1178 | ai:codex
fn display_tool_headless_contained_flags(agent: &str) -> Vec<String> {
    match agent {
        "claude" => vec![
            "--permission-mode".into(),
            "dontAsk".into(),
            "--settings".into(),
            "<contained-settings>".into(),
        ],
        _ => display_tool_contained_flags(agent),
    }
}

// trace:BUG-1178 | ai:codex
fn launch_kind_permission_flags(
    agent: &str,
    tier: &str,
    flags: &[String],
) -> (Vec<String>, Vec<String>) {
    if tier == "contained" && agent == "claude" && flags == display_tool_contained_flags(agent) {
        (
            display_tool_contained_flags(agent),
            display_tool_headless_contained_flags(agent),
        )
    } else {
        (flags.to_vec(), flags.to_vec())
    }
}

// trace:BUG-1178 | ai:codex
fn prompts_for_agent_flags(
    agent: &str,
    tier: &str,
    flags: &[String],
    codex: &CodexConfigPosture,
) -> String {
    let codex_approval = codex
        .approval_policy
        .as_ref()
        .map(|v| v.value.as_str())
        .unwrap_or("native-default");
    if tier == "bypass"
        || flags
            .iter()
            .any(|f| f == "bypassPermissions" || f.contains("dontAsk"))
    {
        "no".to_string()
    } else if agent == "codex" && codex_approval == "never" {
        "no".to_string()
    } else {
        "native".to_string()
    }
}

fn nuclear_permission_flag(flag: &str) -> bool {
    matches!(
        flag,
        "bypassPermissions"
            | "--dangerously-bypass-approvals-and-sandbox"
            | "--dangerously-skip-permissions"
    )
}

fn contained_permission_flag(flag: &str) -> bool {
    matches!(flag, "dontAsk" | "--sandbox" | "--settings")
}

fn agent_default_flags_with_source(
    project_root: &std::path::Path,
    agent: &str,
) -> Option<ScopedValue<Vec<String>>> {
    let global = aida_home_dir().map(|h| h.join(".aida/agents.toml"));
    let project = project_root.join(".aida/agents.toml");
    let global_flags = global
        .as_deref()
        .and_then(|p| agent_default_flags_from_file(p, agent));
    let project_flags = agent_default_flags_from_file(&project, agent);
    match (project_flags, global_flags) {
        (Some(value), _) => Some(ScopedValue {
            value,
            source: PolicySource::ProjectAgents,
        }),
        (None, Some(value)) => Some(ScopedValue {
            value,
            source: PolicySource::GlobalAgents,
        }),
        (None, None) => None,
    }
}

fn agent_default_flags_from_file(path: &std::path::Path, agent: &str) -> Option<Vec<String>> {
    let value = parse_agents_toml(path).ok().flatten()?;
    let flags = value
        .get("agents")?
        .get(agent)?
        .get("default_flags")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    Some(flags)
}

fn agents_bool_with_source(project_root: &std::path::Path, key: &str) -> Option<ScopedValue<bool>> {
    let global = aida_home_dir().map(|h| h.join(".aida/agents.toml"));
    let project = project_root.join(".aida/agents.toml");
    let global_value = global
        .as_deref()
        .and_then(|p| read_agents_bool_from_file(p, key).ok().flatten());
    let project_value = read_agents_bool_from_file(&project, key).ok().flatten();
    match (project_value, global_value) {
        (Some(value), _) => Some(ScopedValue {
            value,
            source: PolicySource::ProjectAgents,
        }),
        (None, Some(value)) => Some(ScopedValue {
            value,
            source: PolicySource::GlobalAgents,
        }),
        (None, None) => None,
    }
}

fn agents_contained_with_source(project_root: &std::path::Path) -> Option<ScopedValue<bool>> {
    let legacy = agents_bool_with_source(project_root, "contained");
    let cfg = read_project_config_value(project_root);
    let unified = config_lookup(cfg.as_ref(), "contained", "enable").and_then(|v| v.as_bool());
    unified
        .map(|value| ScopedValue {
            value,
            source: PolicySource::ProjectConfig,
        })
        .or(legacy)
}

fn policy_source_from_plain(label: &str) -> PolicySource {
    match label {
        ".aida/config.toml" => PolicySource::ProjectConfig,
        ".aida/agents.toml" => PolicySource::ProjectAgents,
        "~/.aida/agents.toml" => PolicySource::GlobalAgents,
        "~/.aida/config.toml" => PolicySource::GlobalConfig,
        ".codex/config.toml" => PolicySource::ProjectCodexConfig,
        "~/.codex/config.toml" => PolicySource::GlobalCodexConfig,
        ".mcp.json" => PolicySource::ProjectMcpJson,
        _ => PolicySource::Default,
    }
}

fn codex_config_posture(project_root: &std::path::Path) -> CodexConfigPosture {
    let global = aida_home_dir().map(|h| h.join(".codex/config.toml"));
    let project = project_root.join(".codex/config.toml");
    let global_cfg = global.as_deref().and_then(read_toml_file);
    let project_cfg = read_toml_file(&project);
    let get_string = |key: &str| {
        scoped_root_string(project_cfg.as_ref(), PolicySource::ProjectCodexConfig, key).or_else(
            || scoped_root_string(global_cfg.as_ref(), PolicySource::GlobalCodexConfig, key),
        )
    };
    let get_workspace_bool = |key: &str| {
        scoped_table_bool(
            project_cfg.as_ref(),
            PolicySource::ProjectCodexConfig,
            "sandbox_workspace_write",
            key,
        )
        .or_else(|| {
            scoped_table_bool(
                global_cfg.as_ref(),
                PolicySource::GlobalCodexConfig,
                "sandbox_workspace_write",
                key,
            )
        })
    };
    let get_workspace_array = |key: &str| {
        scoped_table_string_array(
            project_cfg.as_ref(),
            PolicySource::ProjectCodexConfig,
            "sandbox_workspace_write",
            key,
        )
        .or_else(|| {
            scoped_table_string_array(
                global_cfg.as_ref(),
                PolicySource::GlobalCodexConfig,
                "sandbox_workspace_write",
                key,
            )
        })
    };
    CodexConfigPosture {
        sandbox_mode: get_string("sandbox_mode"),
        approval_policy: get_string("approval_policy"),
        workspace_table_source: if project_cfg
            .as_ref()
            .and_then(|v| v.get("sandbox_workspace_write"))
            .and_then(|v| v.as_table())
            .is_some()
        {
            Some(PolicySource::ProjectCodexConfig)
        } else if global_cfg
            .as_ref()
            .and_then(|v| v.get("sandbox_workspace_write"))
            .and_then(|v| v.as_table())
            .is_some()
        {
            Some(PolicySource::GlobalCodexConfig)
        } else {
            None
        },
        network_access: get_workspace_bool("network_access"),
        writable_roots: get_workspace_array("writable_roots"),
    }
}

fn read_toml_file(path: &std::path::Path) -> Option<toml::Value> {
    let body = std::fs::read_to_string(path).ok()?;
    toml::from_str(&body).ok()
}

fn scoped_root_string(
    cfg: Option<&toml::Value>,
    source: PolicySource,
    key: &str,
) -> Option<ScopedValue<String>> {
    let value = cfg?.get(key)?.as_str()?.trim().to_string();
    (!value.is_empty()).then_some(ScopedValue { value, source })
}

fn scoped_table_bool(
    cfg: Option<&toml::Value>,
    source: PolicySource,
    table: &str,
    key: &str,
) -> Option<ScopedValue<bool>> {
    Some(ScopedValue {
        value: cfg?.get(table)?.get(key)?.as_bool()?,
        source,
    })
}

fn scoped_table_string_array(
    cfg: Option<&toml::Value>,
    source: PolicySource,
    table: &str,
    key: &str,
) -> Option<ScopedValue<Vec<String>>> {
    Some(ScopedValue {
        value: cfg?
            .get(table)?
            .get(key)?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        source,
    })
}

/// The central config-policy registry (BUG-533 slice 2 / TASK-793). Returns one
/// [`PolicySection`] per known config section, each carrying its resolved knob
/// rows. This is the **single source of truth** consumed by `aida config show`
/// ([`render_effective_policy`]); adding a knob is a one-spot edit here and it
/// appears in `config show` automatically — no separate renderer edit, which is
/// exactly the drift that produced BUG-533.
///
/// Each section's resolution reuses the existing reader helpers
/// (`load_agents_contained`, `IntakeConfig::load`, `seats::*`,
/// `glyphs::active_*`, `workflow_hints::enabled`, …) so the rendered values
/// match what the rest of the binary actually reads — the registry is a view
/// over the real readers, not a parallel re-derivation.
// trace:TASK-793 trace:BUG-533 | ai:claude
fn policy_registry(project_root: &std::path::Path) -> Vec<PolicySection> {
    let cfg = read_project_config_value(project_root);
    let mut sections: Vec<PolicySection> = Vec::new();

    // --- Agent permission posture (security-relevant — lead with it). ---
    // Resolution: global ~/.aida/agents.toml base, project .aida/agents.toml
    // override; default false (faithful native posture). trace:STORY-495
    sections.push({
        let mut rows = Vec::new();
        let global_path = aida_home_dir().map(|h| h.join(".aida/agents.toml"));
        let project_agents = project_root.join(".aida/agents.toml");
        let global_bypass = global_path
            .as_deref()
            .and_then(|p| read_agents_bypass_from_file(p).ok().flatten());
        let project_bypass = read_agents_bypass_from_file(&project_agents).ok().flatten();
        let (effective, source) = match (project_bypass, global_bypass) {
            (Some(v), _) => (v, PolicySource::ProjectAgents),
            (None, Some(v)) => (v, PolicySource::GlobalAgents),
            (None, None) => (false, PolicySource::Default),
        };
        let rendered = if effective {
            format!("{} (agents skip permission prompts)", "bypass".red().bold())
        } else {
            format!("{} (Claude prompts; faithful launcher)", "native".green())
        };
        rows.push(PolicyRow {
            key: "bypass",
            value: rendered,
            source,
        });
        let selection_with_source =
            crate::init_cmd::read_enabled_agent_selection_with_source(project_root);
        let selection = selection_with_source
            .as_ref()
            .map(|s| s.selection)
            .unwrap_or(crate::init_cmd::AgentSelection {
                claude: true,
                codex: true,
                antigravity: true,
            });
        let enabled_source = selection_with_source
            .as_ref()
            .map(|s| {
                if s.path == project_root.join(".aida/agents.toml") {
                    PolicySource::ProjectAgents
                } else if s.path == project_root.join(".aida/config.toml") {
                    PolicySource::ProjectConfig
                } else {
                    PolicySource::GlobalAgents
                }
            })
            .unwrap_or(PolicySource::Default);
        for (key, enabled) in [
            ("claude", selection.claude),
            ("codex", selection.codex),
            ("antigravity", selection.antigravity),
        ] {
            rows.push(PolicyRow {
                key,
                value: if enabled {
                    "enabled".to_string()
                } else {
                    "disabled".to_string()
                },
                source: enabled_source,
            });
        }
        PolicySection {
            section: "agents",
            header: "[agents] — enabled project profiles + permission posture".to_string(),
            rows,
        }
    });

    // --- Contained posture (sandbox enable + egress allowlist). After TASK-798
    // unified the posture under `[contained]` (`enable` alias of legacy
    // `[agents] contained`, plus `allowed_hosts`), surface both rows here so
    // `aida config show` reflects the resolved sandbox/egress stance. Reuses the
    // existing resolution helpers rather than re-deriving. trace:TASK-802 | ai:claude
    sections.push({
        let mut rows = Vec::new();
        // `enable`: resolve the source by precedence (last-wins, mirroring
        // load_agents_contained): unified `[contained] enable` overrides legacy
        // `[agents] contained` (project, then global); else default false.
        let unified = config_lookup(cfg.as_ref(), "contained", "enable").and_then(|v| v.as_bool());
        let project_agents = project_root.join(".aida/agents.toml");
        let project_legacy = read_agents_bool_from_file(&project_agents, "contained")
            .ok()
            .flatten();
        let global_legacy = aida_home_dir()
            .and_then(|h| {
                read_agents_bool_from_file(&h.join(".aida/agents.toml"), "contained").ok()
            })
            .flatten();
        let effective = load_agents_contained(project_root).unwrap_or(false);
        let source = if unified.is_some() || project_legacy.is_some() {
            PolicySource::ProjectConfig
        } else if global_legacy.is_some() {
            PolicySource::GlobalAgents
        } else {
            PolicySource::Default
        };
        let rendered = if effective {
            format!("{} (agents run sandboxed)", "enabled".green())
        } else {
            format!("{} (agents run unsandboxed)", "disabled".dimmed())
        };
        rows.push(PolicyRow {
            key: "enable",
            value: rendered,
            source,
        });

        // `allowed_hosts`: the egress allowlist; empty = no egress restriction.
        let hosts = crate::session::contained_allowed_hosts(project_root);
        let (value, source) = if hosts.is_empty() {
            (
                "(none — no egress restriction)".dimmed().to_string(),
                PolicySource::Default,
            )
        } else {
            (hosts.join(", "), PolicySource::ProjectConfig)
        };
        rows.push(PolicyRow {
            key: "allowed_hosts",
            value,
            source,
        });

        // TASK-866 (SPIKE-68): surface the three bwrap-specific knobs that were
        // previously invisible here — operators conflate `enable` (Claude Code's
        // native --settings sandbox) with `os_wrap` (the bwrap OS boundary). Each
        // reuses the existing session.rs resolver rather than re-deriving.

        // `os_wrap`: the bwrap OS-sandbox master switch. Distinct from `enable`;
        // default OFF (the OS boundary is strictly opt-in). trace:TASK-866 | ai:claude
        let os_wrap = crate::session::os_wrap_enabled(project_root);
        let (value, source) = if os_wrap {
            (
                format!("{} (the bwrap OS sandbox)", "true".green()),
                PolicySource::ProjectConfig,
            )
        } else {
            (
                "false (default; the bwrap OS sandbox — off unless set)"
                    .dimmed()
                    .to_string(),
                PolicySource::Default,
            )
        };
        rows.push(PolicyRow {
            key: "os_wrap",
            value,
            source,
        });

        // `read_allowlist`: strict read-confinement paths under os_wrap. Empty =
        // no read confinement (binds the host root ro). trace:TASK-866 | ai:claude
        let read_allowlist = crate::session::contained_read_allowlist(project_root);
        let (value, source) = if read_allowlist.is_empty() {
            (
                "(none — no read confinement)".dimmed().to_string(),
                PolicySource::Default,
            )
        } else {
            (read_allowlist.join(", "), PolicySource::ProjectConfig)
        };
        rows.push(PolicyRow {
            key: "read_allowlist",
            value,
            source,
        });

        // `managed_domains_only`: hard egress deny (managed set + allowed_hosts),
        // no approval prompt. Default OFF. trace:TASK-866 | ai:claude
        let managed_only = crate::session::contained_managed_domains_only(project_root);
        let (value, source) = if managed_only {
            (
                format!(
                    "{} (hard egress deny — managed set + allowed_hosts)",
                    "true".green()
                ),
                PolicySource::ProjectConfig,
            )
        } else {
            (
                "false (default; egress not hard-denied)"
                    .dimmed()
                    .to_string(),
                PolicySource::Default,
            )
        };
        rows.push(PolicyRow {
            key: "managed_domains_only",
            value,
            source,
        });

        PolicySection {
            section: "contained",
            header: "[contained] — sandbox + egress posture".to_string(),
            rows,
        }
    });

    // --- Computed permission posture (STORY-1127). Compact rows for `aida
    // config show` / `aida config menu`; the menu can cycle the tier through
    // the same writer as `aida config permissions set` (STORY-1131). The
    // detailed table and JSON live at `aida config permissions show`.
    // trace:STORY-1127 trace:STORY-1131 | ai:codex
    sections.push({
        let report = permission_posture_report(project_root);
        let rows = report
            .agents
            .into_iter()
            .map(|row| PolicyRow {
                key: match row.agent.as_str() {
                    "claude" => "claude",
                    "codex" => "codex",
                    "antigravity" => "antigravity",
                    _ => "unknown",
                },
                value: format!("{} ({})", row.tier, row.net_effect),
                source: policy_source_from_plain(&row.tier_source),
            })
            .collect();
        PolicySection {
            section: "permissions",
            header: "[permissions] — computed per-agent launch posture (detail command available)"
                .to_string(),
            rows,
        }
    });

    // --- Local AIDA MCP registration (ADR-30 / STORY-1131). This is a derived
    // row over `.mcp.json` presence, not a persistent config knob.
    // trace:STORY-1131 | ai:codex
    sections.push({
        let registered = local_aida_mcp_registered(project_root);
        let (value, source) = if registered {
            (
                "yes (local .mcp.json)".to_string(),
                PolicySource::ProjectMcpJson,
            )
        } else {
            ("no".to_string(), PolicySource::Default)
        };
        PolicySection {
            section: "mcp",
            header: "[mcp] — project-local MCP registration".to_string(),
            rows: vec![PolicyRow {
                key: "aida_registered",
                value,
                source,
            }],
        }
    });

    // --- Mailbox act-on-mail policy. trace:TASK-782 ---
    sections.push({
        let raw = config_lookup(cfg.as_ref(), "mailbox", "act_on_mail")
            .and_then(|v| v.as_str())
            .and_then(aida_core::mailbox::ActOnMail::parse);
        let (value, source) = match raw {
            Some(aida_core::mailbox::ActOnMail::SurfaceAndRecommend) => (
                "surface-and-recommend".to_string(),
                PolicySource::ProjectConfig,
            ),
            Some(aida_core::mailbox::ActOnMail::EscalatePerCascade) => (
                "escalate-per-cascade".to_string(),
                PolicySource::ProjectConfig,
            ),
            None => ("surface-and-recommend".to_string(), PolicySource::Default),
        };
        // STORY-643: auto mailbox sync on the pull/push store legs. Env wins
        // over config; default on. trace:STORY-643
        let (autosync_value, autosync_source) =
            match std::env::var("AIDA_MAILBOX_AUTOSYNC").ok().as_deref() {
                Some(v) if !v.is_empty() => {
                    let on = !matches!(
                        v.trim().to_ascii_lowercase().as_str(),
                        "false" | "0" | "no" | "off"
                    );
                    (on.to_string(), PolicySource::Env("AIDA_MAILBOX_AUTOSYNC"))
                }
                _ => match config_lookup(cfg.as_ref(), "mailbox", "autosync")
                    .and_then(|v| v.as_bool())
                {
                    Some(b) => (b.to_string(), PolicySource::ProjectConfig),
                    None => ("true".to_string(), PolicySource::Default),
                },
            };
        PolicySection {
            section: "mailbox",
            header: "[mailbox]".to_string(),
            rows: vec![
                PolicyRow {
                    key: "act_on_mail",
                    value,
                    source,
                },
                PolicyRow {
                    key: "autosync",
                    value: autosync_value,
                    source: autosync_source,
                },
            ],
        }
    });

    // --- Burndown launcher visibility. trace:TASK-1159 ---
    sections.push({
        let project = config_lookup(cfg.as_ref(), "burndown", "verbose").and_then(|v| v.as_bool());
        let global_path = aida_home_dir().map(|h| h.join(".aida/config.toml"));
        let global_cfg = global_path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|body| {
                toml::from_str::<toml::Value>(&body).ok().and_then(|v| {
                    config_lookup(Some(&v), "burndown", "verbose").and_then(|b| b.as_bool())
                })
            });
        let (effective, source) = match (project, global_cfg) {
            (Some(v), _) => (v, PolicySource::ProjectConfig),
            (None, Some(v)) => (v, PolicySource::GlobalConfig),
            (None, None) => (false, PolicySource::Default),
        };
        // TASK-1172: ready-set ordering, resolved on the same project → global →
        // default ladder. Only the two recognized values are surfaced; anything
        // else reads as the default rather than echoing a typo back as policy.
        let order_of = |v: Option<&toml::Value>| -> Option<String> {
            v.and_then(|v| v.as_str())
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| s == "priority" || s == "queue")
        };
        let order_project = order_of(config_lookup(cfg.as_ref(), "burndown", "order"));
        let order_global = aida_home_dir()
            .map(|h| h.join(".aida/config.toml"))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|body| toml::from_str::<toml::Value>(&body).ok())
            .and_then(|v| order_of(config_lookup(Some(&v), "burndown", "order")));
        let (order_value, order_source) = match (order_project, order_global) {
            (Some(v), _) => (v, PolicySource::ProjectConfig),
            (None, Some(v)) => (v, PolicySource::GlobalConfig),
            (None, None) => ("priority".to_string(), PolicySource::Default),
        };
        PolicySection {
            section: "burndown",
            header: "[burndown]".to_string(),
            rows: vec![
                PolicyRow {
                    key: "verbose",
                    value: effective.to_string(),
                    source,
                },
                PolicyRow {
                    key: "order",
                    value: order_value,
                    source: order_source,
                },
            ],
        }
    });

    // --- Advisor calibration mode. trace:STORY-347 ---
    sections.push({
        let raw =
            config_lookup(cfg.as_ref(), "advisor", "calibration_mode").and_then(|v| v.as_str());
        let (value, source) = match raw {
            Some(s)
                if matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "on" | "true" | "1" | "yes"
                ) =>
            {
                ("on".to_string(), PolicySource::ProjectConfig)
            }
            Some(_) => ("off".to_string(), PolicySource::ProjectConfig),
            None => ("off".to_string(), PolicySource::Default),
        };
        PolicySection {
            section: "advisor",
            header: "[advisor]".to_string(),
            rows: vec![PolicyRow {
                key: "calibration_mode",
                value,
                source,
            }],
        }
    });

    // --- Mass-change review mode. trace:STORY-1415 | ai:claude ---
    sections.push({
        let now = chrono::Utc::now();
        let rec = crate::mass_change::read_record(project_root);
        let st = crate::mass_change::state_at(&rec, now);
        let configured = config_lookup(cfg.as_ref(), "review", "mass_change_mode").is_some();
        let value = match st {
            crate::mass_change::MassChangeState::Off => "off".to_string(),
            crate::mass_change::MassChangeState::Unclocked => {
                "on (never clocked: inactive)".to_string()
            }
            crate::mass_change::MassChangeState::Expired { .. } => "expired (inactive)".to_string(),
            crate::mass_change::MassChangeState::Active { expires_at, .. } => {
                format!("on until {}", expires_at.format("%Y-%m-%d %H:%M UTC"))
            }
        };
        PolicySection {
            section: "review",
            header: "[review]".to_string(),
            rows: vec![PolicyRow {
                key: "mass_change_mode",
                value,
                source: if configured {
                    PolicySource::ProjectConfig
                } else {
                    PolicySource::Default
                },
            }],
        }
    });

    // --- Untraced-acceptance-criteria gate at `aida queue done`. trace:TASK-1290 (no env) ---
    sections.push({
        let configured =
            config_lookup(cfg.as_ref(), "protocol", "enforce").and_then(|v| v.as_str());
        let (value, source) = match configured {
            Some(s) if s.eq_ignore_ascii_case("refuse") => {
                ("refuse".to_string(), PolicySource::ProjectConfig)
            }
            Some(_) => (
                "warn (unrecognized value, default)".to_string(),
                PolicySource::Default,
            ),
            None => ("warn (default)".to_string(), PolicySource::Default),
        };
        PolicySection {
            section: "protocol",
            header: "[protocol]".to_string(),
            rows: vec![PolicyRow {
                key: "enforce",
                value,
                source,
            }],
        }
    });

    // --- Archive auto-sweep. trace:STORY-441 (env: AIDA_AUTO_ARCHIVE) ---
    sections.push({
        let env_off = std::env::var("AIDA_AUTO_ARCHIVE")
            .map(|v| v.trim() == "0")
            .unwrap_or(false);
        let configured =
            config_lookup(cfg.as_ref(), "archive", "auto_after_days").and_then(|v| v.as_integer());
        let (value, source) = if env_off {
            (
                "disabled".to_string(),
                PolicySource::Env("AIDA_AUTO_ARCHIVE"),
            )
        } else {
            match configured {
                Some(days) => {
                    let clamped = days.max(7);
                    (format!("after {clamped} days"), PolicySource::ProjectConfig)
                }
                None => ("disabled (unset)".to_string(), PolicySource::Default),
            }
        };
        PolicySection {
            section: "archive",
            header: "[archive]".to_string(),
            rows: vec![PolicyRow {
                key: "auto_after_days",
                value,
                source,
            }],
        }
    });

    // --- List view-tier hints. trace:BUG-783 (no env) ---
    sections.push({
        let configured =
            config_lookup(cfg.as_ref(), "list", "show_hidden_hints").and_then(|v| v.as_bool());
        let (value, source) = match configured {
            Some(true) => ("shown".to_string(), PolicySource::ProjectConfig),
            Some(false) => ("suppressed".to_string(), PolicySource::ProjectConfig),
            None => ("suppressed (default)".to_string(), PolicySource::Default),
        };
        PolicySection {
            section: "list",
            header: "[list]".to_string(),
            rows: vec![PolicyRow {
                key: "show_hidden_hints",
                value,
                source,
            }],
        }
    });

    // --- Telemetry. trace:STORY-122 (env: AIDA_TELEMETRY) ---
    sections.push({
        let env_off = std::env::var("AIDA_TELEMETRY")
            .map(|v| matches!(v.trim(), "0" | "false" | "no" | "off"))
            .unwrap_or(false);
        let configured =
            config_lookup(cfg.as_ref(), "telemetry", "enabled").and_then(|v| v.as_bool());
        let (value, source) = if env_off {
            ("disabled".to_string(), PolicySource::Env("AIDA_TELEMETRY"))
        } else {
            match configured {
                Some(true) => ("enabled".to_string(), PolicySource::ProjectConfig),
                Some(false) => ("disabled".to_string(), PolicySource::ProjectConfig),
                None => ("enabled".to_string(), PolicySource::Default),
            }
        };
        PolicySection {
            section: "telemetry",
            header: "[telemetry]".to_string(),
            rows: vec![PolicyRow {
                key: "enabled",
                value,
                source,
            }],
        }
    });

    // --- Field study (SPIKE-67): observe-only rule-adherence study. This is the
    // formerly-DRIFTED knob STORY-671 closes — it was declared nowhere and never
    // appeared in `config show`. Resolution reuses `field_study::is_enabled` (the
    // real reader) for the effective value; the source mirrors telemetry's env >
    // config > default precedence, with the AIDA_TELEMETRY=0 kill-switch noted.
    // trace:STORY-671 trace:SPIKE-67 (env: AIDA_FIELD_STUDY) | ai:claude ---
    sections.push({
        let env_on = std::env::var("AIDA_FIELD_STUDY")
            .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        let configured = crate::field_study::parse_field_study_enabled(
            &std::fs::read_to_string(config_path_for_project(project_root)).unwrap_or_default(),
        );
        let effective = crate::field_study::is_enabled(Some(project_root));
        // The global telemetry kill-switch forces the study off regardless of
        // the field's own opt-in — surface that so the value reads honestly.
        let telemetry_off = !crate::usage::is_enabled(Some(project_root));
        let (value, source) = if telemetry_off {
            (
                "disabled (AIDA_TELEMETRY kill-switch)".to_string(),
                PolicySource::Env("AIDA_TELEMETRY"),
            )
        } else if env_on {
            ("enabled".to_string(), PolicySource::Env("AIDA_FIELD_STUDY"))
        } else if let Some(b) = configured {
            (
                if b { "enabled" } else { "disabled" }.to_string(),
                PolicySource::ProjectConfig,
            )
        } else {
            (
                if effective {
                    "enabled"
                } else {
                    "disabled (unset)"
                }
                .to_string(),
                PolicySource::Default,
            )
        };
        PolicySection {
            section: "field_study",
            header: "[field_study] — observe-only rule-adherence study".to_string(),
            rows: vec![PolicyRow {
                key: "enabled",
                value,
                source,
            }],
        }
    });

    // --- Intake policy. trace:STORY-560 ---
    sections.push({
        let intake = crate::intake::IntakeConfig::load(project_root);
        let default = crate::intake::IntakeConfig::default();
        let src = |is_default: bool| {
            if is_default {
                PolicySource::Default
            } else {
                PolicySource::ProjectConfig
            }
        };
        PolicySection {
            section: "intake",
            header: "[intake]".to_string(),
            rows: vec![
                PolicyRow {
                    key: "disposition_bias",
                    value: intake.disposition_bias.as_str().to_string(),
                    source: src(intake.disposition_bias == default.disposition_bias),
                },
                PolicyRow {
                    key: "on_apply",
                    value: intake.on_apply.as_str().to_string(),
                    source: src(intake.on_apply == default.on_apply),
                },
                PolicyRow {
                    key: "do_not_approve_classes",
                    value: intake.do_not_approve_classes.join(", "),
                    source: src(intake.do_not_approve_classes == default.do_not_approve_classes),
                },
            ],
        }
    });

    // --- Ultraplan suggestion mode. trace:TASK-304 (surfaced for STORY-677) ---
    sections.push({
        let mode = read_ultraplan_config(project_root).mode;
        let configured = config_lookup(cfg.as_ref(), "ultraplan", "mode")
            .and_then(|v| v.as_str())
            .and_then(UltraplanMode::from_token)
            .is_some();
        let (value, source) = (
            match mode {
                UltraplanMode::Never => "never".to_string(),
                UltraplanMode::OnDemand => "on-demand".to_string(),
                UltraplanMode::Suggested => "suggested".to_string(),
            },
            if configured {
                PolicySource::ProjectConfig
            } else {
                PolicySource::Default
            },
        );
        PolicySection {
            section: "ultraplan",
            header: "[ultraplan]".to_string(),
            rows: vec![PolicyRow {
                key: "mode",
                value,
                source,
            }],
        }
    });

    // --- Presence settings. trace:STORY-561 ---
    sections.push({
        let render_str = |section: &str, key: &str, default: &str| match config_lookup(
            cfg.as_ref(),
            section,
            key,
        )
        .and_then(|v| v.as_str())
        {
            Some(s) => (s.trim().to_string(), PolicySource::ProjectConfig),
            None => (default.to_string(), PolicySource::Default),
        };
        let mut rows = Vec::new();
        for (key, default) in [
            ("consumers", "on"),
            ("away_drain", "headless-both"),
            ("home_offer", "surface"),
        ] {
            let (value, source) = render_str("presence", key, default);
            rows.push(PolicyRow { key, value, source });
        }
        PolicySection {
            section: "presence",
            header: "[presence]".to_string(),
            rows,
        }
    });

    // --- Workflow hints. trace:STORY-106 (env: AIDA_HINTS) ---
    sections.push({
        let env = std::env::var("AIDA_HINTS").ok().filter(|s| {
            matches!(
                s.trim().to_ascii_lowercase().as_str(),
                "true" | "1" | "yes" | "on" | "false" | "0" | "no" | "off"
            )
        });
        let configured =
            config_lookup(cfg.as_ref(), "hints", "workflow_hints").and_then(|v| v.as_bool());
        let effective = workflow_hints::enabled(Some(project_root));
        let source = if env.is_some() {
            PolicySource::Env("AIDA_HINTS")
        } else if configured.is_some() {
            PolicySource::ProjectConfig
        } else {
            PolicySource::Default
        };
        PolicySection {
            section: "hints",
            header: "[hints]".to_string(),
            rows: vec![PolicyRow {
                key: "workflow_hints",
                value: if effective {
                    "enabled".to_string()
                } else {
                    "disabled".to_string()
                },
                source,
            }],
        }
    });

    // --- UI glyphs/theme rendering (EPIC-45 / STORY-633). The glyph profile +
    // theme are read by the `glyphs` module via its own precedence chain
    // (`AIDA_GLYPHS` env > project > user > default); surface the resolved
    // values so `config show` covers the visible-rendering knobs too. Reuses
    // the module's own resolvers — no parallel parse. trace:TASK-793 | ai:claude
    sections.push({
        let mut rows = Vec::new();

        // `glyphs` profile: env > project `[ui] glyphs` > user > default unicode.
        let env_glyphs = std::env::var("AIDA_GLYPHS")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let project_glyphs =
            config_lookup(cfg.as_ref(), "ui", "glyphs").and_then(|v| v.as_str().map(String::from));
        let profile = crate::glyphs::active_profile(Some(project_root));
        let glyphs_source = if env_glyphs.is_some() {
            PolicySource::Env("AIDA_GLYPHS")
        } else if project_glyphs.is_some() {
            PolicySource::ProjectConfig
        } else {
            PolicySource::Default
        };
        rows.push(PolicyRow {
            key: "glyphs",
            value: profile.name().to_string(),
            source: glyphs_source,
        });

        // `theme`: project `[ui] theme` > user > none. `AIDA_GLYPHS` does not
        // name a theme, so there is no env tier here (mirrors `active_theme`).
        let project_theme =
            config_lookup(cfg.as_ref(), "ui", "theme").and_then(|v| v.as_str().map(String::from));
        let (theme_value, theme_source) = match crate::glyphs::active_theme(Some(project_root)) {
            Some(t) => {
                let source = if project_theme.is_some() {
                    PolicySource::ProjectConfig
                } else {
                    PolicySource::GlobalConfig
                };
                (t.name.to_string(), source)
            }
            None => ("(none)".dimmed().to_string(), PolicySource::Default),
        };
        rows.push(PolicyRow {
            key: "theme",
            value: theme_value,
            source: theme_source,
        });

        PolicySection {
            section: "ui",
            header: "[ui] — glyph profile + theme".to_string(),
            rows,
        }
    });

    // --- Seat policy: which configurable buckets show on the operator vs the
    // advisor worklist. trace:STORY-620 ---
    sections.push({
        let project_cfg = project_root.join(".aida/config.toml");
        let global_cfg = aida_home_dir().map(|h| h.join(".aida/config.toml"));
        let mut rows = Vec::new();
        for &key in seats::CONFIGURABLE_KEYS {
            // Re-derive the source by precedence (project > user-global >
            // default) so the row shows where the effective value came from.
            let project = seats::seat_in_file(&project_cfg, key);
            let global = global_cfg
                .as_deref()
                .and_then(|p| seats::seat_in_file(p, key));
            let (seat, source) = match (project, global) {
                (Some(s), _) => (s, PolicySource::ProjectConfig),
                (None, Some(s)) => (s, PolicySource::GlobalConfig),
                (None, None) => (seats::default_seat(key), PolicySource::Default),
            };
            rows.push(PolicyRow {
                key,
                value: seat.as_str().to_string(),
                source,
            });
        }
        PolicySection {
            section: "seats",
            header: "[seats] — operator (aida human) vs advisor (aida advisor) worklist"
                .to_string(),
            rows,
        }
    });

    // --- Team RBAC guardrail (STORY-647): the gated-op permission map, the
    // protected-tag set + its required role, and strict mode. GUARDRAIL, NOT
    // SECURITY — the store is a shared branch; this stops accidents + leaves an
    // audit trail, it is not access control. Resolution reuses the same
    // `permissions::TeamPermissions` reader the gates consult, so `config show`
    // reflects what the binary actually enforces. trace:STORY-647 | ai:claude
    sections.push({
        let team = permissions::TeamPermissions::from_config(cfg.as_ref());
        let mut rows = Vec::new();
        let team_present = cfg
            .as_ref()
            .map(|c| c.get("team").is_some())
            .unwrap_or(false);
        let src = |configured: bool| {
            if configured {
                PolicySource::ProjectConfig
            } else {
                PolicySource::Default
            }
        };
        rows.push(PolicyRow {
            key: "strict",
            value: if team.strict {
                "true (non-rostered = least-privilege; refusals roster-authoritative)".to_string()
            } else {
                "false (slice-1: env/default fallback)".to_string()
            },
            source: src(team_present
                && cfg
                    .as_ref()
                    .and_then(|c| c.get("team"))
                    .and_then(|t| t.get("strict"))
                    .is_some()),
        });
        rows.push(PolicyRow {
            key: "protected_tags",
            value: team.protected_tags_display(),
            source: src(cfg
                .as_ref()
                .and_then(|c| c.get("team"))
                .and_then(|t| t.get("protected_tags"))
                .is_some()),
        });
        // The per-op minimum roles (the permission map).
        for (op, key) in permissions::POLICY_DISPLAY_OPS {
            rows.push(PolicyRow {
                key,
                value: team.min_role(*op),
                source: src(cfg
                    .as_ref()
                    .and_then(|c| c.get("team"))
                    .and_then(|t| t.get("permissions"))
                    .and_then(|p| p.get(key))
                    .is_some()),
            });
        }
        rows.push(PolicyRow {
            key: "protected_role",
            value: team.min_role(permissions::GatedOp::ProtectedSpec),
            source: src(cfg
                .as_ref()
                .and_then(|c| c.get("team"))
                .and_then(|t| t.get("protected_role"))
                .is_some()),
        });
        PolicySection {
            section: "team",
            header:
                "[team] — RBAC guardrail (NOT security: shared store; stops accidents + audits)"
                    .to_string(),
            rows,
        }
    });

    sections
}

/// Handle `aida config hints [true|false]` — show or persist the
/// `[hints] workflow_hints` setting.
// trace:STORY-106 | ai:claude
pub(crate) fn handle_config_hints(arg: Option<&str>, storage: &Storage) -> Result<()> {
    let project_root = storage
        .path()
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("could not resolve project root from storage path"))?;

    match arg {
        None => {
            let effective = workflow_hints::enabled(Some(&project_root));
            // BUG-93: only attribute the source to the env var when its value
            // is one `enabled()` actually recognizes. `enabled()` silently
            // ignores unrecognized values (`AIDA_HINTS=garbage`) and falls
            // through to config/default, so claiming "(env)" there is wrong.
            let env = std::env::var("AIDA_HINTS").ok().filter(|s| {
                matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "true" | "1" | "yes" | "on" | "false" | "0" | "no" | "off"
                )
            });
            println!(
                "Workflow hints: {}",
                if effective {
                    "enabled".green().to_string()
                } else {
                    "disabled".yellow().to_string()
                }
            );
            if let Some(v) = env {
                println!("  source: AIDA_HINTS={} (env)", v);
            } else {
                println!("  source: .aida/config.toml (or default if unset)");
            }
        }
        Some(raw) => {
            let value = match raw.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => true,
                "false" | "0" | "no" | "off" => false,
                _ => anyhow::bail!("Invalid value `{}` — use `true` or `false`.", raw),
            };
            let prior = workflow_hints::persist_setting(&project_root, value)?;
            println!(
                "{} workflow_hints {} {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                if value { "enabled" } else { "disabled" },
                match prior {
                    Some(p) if p == value => "(no change)".dimmed().to_string(),
                    Some(p) => format!("(was {})", p).dimmed().to_string(),
                    None => "(was unset, using default)".dimmed().to_string(),
                }
            );
            if std::env::var("AIDA_HINTS").is_ok() {
                eprintln!(
                    "{} AIDA_HINTS env var is set — it overrides this config until unset.",
                    "Note:".yellow()
                );
            }
        }
    }
    Ok(())
}

/// A one-line explanation + built-in default for a config knob, keyed by
/// `(section, key)` — DERIVED from the central [`CONFIG_KNOBS`] registry
/// (STORY-671). The framing matches `docs/environment-variables.md` and the
/// `aida config show` rationale so the TUI carries the same human story the docs
/// do. Knobs with no declaration fall back to a generic placeholder.
// trace:STORY-671 trace:STORY-661 | ai:claude
fn config_knob_doc(section: &str, key: &str) -> (&'static str, &'static str) {
    match config_knob_spec(section, key) {
        Some(spec) => (spec.doc, spec.default),
        None => ("(no description available)", "(see config show)"),
    }
}

/// `aida config menu` — assemble the configurable-item rows from the live
/// policy registry (the same source `aida config show` walks) and launch the
/// navigable TUI. Read + navigate only for this slice; inline editing is a
/// follow-up. The registry resolves every knob's value + source directly from
/// `.aida/config.toml`, the global files, and the `AIDA_*` env knobs, so this
/// is a view over the real readers — never a parallel re-derivation.
///
/// No-TTY degrades gracefully: prints a pointer to `aida config show` and
/// exits 0, mirroring how `aida tui` / `aida --asciinema` handle a missing
/// terminal.
// trace:STORY-661 | ai:claude
#[cfg(feature = "tui")]
pub(crate) fn handle_config_menu_command() -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        println!("config menu needs a TTY; use `aida config show` to view config from here.");
        return Ok(());
    }

    let project_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let items = build_config_menu_items(&project_root);
    // STORY-669: the edit callback lives cli-side (the tui crate stays free of
    // an aida-cli dependency) — it writes through `config_edit::set_kv` and
    // re-resolves the row from the live registry.
    aida_tui::run_config_menu(items, |item, requested| {
        cli_edit_config_knob(&project_root, item, requested)
    })
}

/// Stub for binaries built without the `tui` feature — `aida config menu`
/// points the user at `aida config show` rather than half-running.
// trace:STORY-661 | ai:claude
#[cfg(not(feature = "tui"))]
pub(crate) fn handle_config_menu_command() -> Result<()> {
    println!("config menu requires a build with the TUI enabled; use `aida config show`.");
    Ok(())
}

/// Project the policy registry into plain-text [`aida_tui::ConfigMenuItem`]
/// rows: strip ANSI from the resolved value, flatten the source to a plain
/// scope label, and attach the per-knob default + explanation.
// trace:STORY-661
#[cfg(feature = "tui")]
fn build_config_menu_items(project_root: &std::path::Path) -> Vec<aida_tui::ConfigMenuItem> {
    let mut items = Vec::new();
    for section in policy_registry(project_root) {
        for row in &section.rows {
            let (explanation, default) = config_knob_doc(section.section, row.key);
            let edit = config_knob_edit_kind(section.section, row.key);
            let read_only_reason = config_knob_readonly_reason(section.section, row.key);
            items.push(aida_tui::ConfigMenuItem {
                section: section.section.to_string(),
                name: row.key.to_string(),
                value: menu_row_value(section.section, row.key, &row.value),
                default: default.to_string(),
                scope: row.source.plain_label(),
                explanation: explanation.to_string(),
                edit,
                read_only_reason,
            });
        }
    }
    items
}

/// The edit metadata the menu's write-back path needs for a config knob, keyed
/// by `(section, key)` — DERIVED from the central [`CONFIG_KNOBS`] registry
/// (STORY-671). Returns the editable [`EditSafety`] variants (`Bool` / `Enum` /
/// `Integer`) only; a `ReadOnly` declaration or an undeclared knob yields `None`
/// so the editor refuses it. This replaces the former hand-maintained
/// `config_knob_meta` table — the SAFE set is now whatever the registry declares
/// editable.
// trace:STORY-671 trace:STORY-669 trace:STORY-677 | ai:claude
#[cfg(feature = "tui")]
fn config_knob_meta(section: &str, key: &str) -> Option<EditSafety> {
    match config_knob_spec(section, key)?.edit {
        edit @ (EditSafety::Bool { .. } | EditSafety::Enum { .. } | EditSafety::Integer { .. }) => {
            Some(edit)
        }
        EditSafety::ReadOnly { .. } => None,
    }
}

/// Project a knob's registry edit-safety into the TUI [`aida_tui::EditKind`].
/// A `ReadOnly` declaration or an undeclared knob → `ReadOnly`.
// trace:STORY-671 trace:STORY-677 | ai:claude
#[cfg(feature = "tui")]
fn config_knob_edit_kind(section: &str, key: &str) -> aida_tui::EditKind {
    match config_knob_spec(section, key).map(|s| s.edit) {
        Some(EditSafety::Bool { .. }) => aida_tui::EditKind::Bool,
        Some(EditSafety::Enum { allowed }) => {
            aida_tui::EditKind::Enum(allowed.iter().map(|s| s.to_string()).collect())
        }
        Some(EditSafety::Integer { min, max }) => aida_tui::EditKind::Integer { min, max },
        Some(EditSafety::ReadOnly { .. }) | None => aida_tui::EditKind::ReadOnly,
    }
}

/// The registry-declared reason a knob is `ReadOnly`, for the `?` help
/// overlay's operational-consequence line (STORY-1470). `None` for an
/// editable knob or an undeclared one — the overlay simply omits the line.
// trace:STORY-1470 | ai:claude
#[cfg(feature = "tui")]
fn config_knob_readonly_reason(section: &str, key: &str) -> Option<String> {
    match config_knob_spec(section, key)?.edit {
        EditSafety::ReadOnly { reason } => Some(reason.to_string()),
        EditSafety::Bool { .. } | EditSafety::Enum { .. } | EditSafety::Integer { .. } => None,
    }
}

/// Re-resolve one knob's (value, scope) strings from the live registry, exactly
/// as `build_config_menu_items` does — so a freshly-written value shows live in
/// the menu.
// trace:STORY-669 | ai:claude
#[cfg(feature = "tui")]
fn resolve_config_menu_row(
    project_root: &std::path::Path,
    section: &str,
    key: &str,
) -> Option<(String, String)> {
    for s in policy_registry(project_root) {
        if s.section != section {
            continue;
        }
        for row in &s.rows {
            if row.key == key {
                return Some((
                    menu_row_value(section, key, &row.value),
                    row.source.plain_label(),
                ));
            }
        }
    }
    None
}

/// The cli-side edit callback the config menu invokes on Enter/Space over an
/// editable row (STORY-669, extended STORY-677). `requested` is `None` for a
/// `Bool` toggle (this fn flips the stored value) and `Some(value)` for the
/// enum value cycled to or the integer typed in — the TUI derives those; this
/// fn just writes the TOML value through the section-preserving writer to the
/// file the value currently lives in, then re-resolves the row. Env-shadowed
/// knobs are refused (the var still wins).
// trace:STORY-669 trace:STORY-677
#[cfg(feature = "tui")]
fn cli_edit_config_knob(
    project_root: &std::path::Path,
    item: &aida_tui::ConfigMenuItem,
    requested: Option<&str>,
) -> aida_tui::EditOutcome {
    use aida_tui::EditOutcome;

    // Env-shadowed: writing config.toml wouldn't change the effective value.
    if item.scope.contains("(env)") {
        let var = item
            .scope
            .split_whitespace()
            .next()
            .unwrap_or("the env var");
        return EditOutcome::Blocked(format!("overridden by {var} — unset it to edit"));
    }
    if item.section == "agents" && matches!(item.name.as_str(), "claude" | "codex" | "antigravity")
    {
        return cli_edit_agent_profile(project_root, item);
    }
    if item.section == "permissions"
        && matches!(item.name.as_str(), "claude" | "codex" | "antigravity")
    {
        return cli_edit_permission_posture(project_root, item, requested);
    }
    if item.section == "mcp" && item.name == "aida_registered" {
        return cli_toggle_local_aida_mcp_registration(project_root);
    }
    let Some(meta) = config_knob_meta(&item.section, &item.name) else {
        return EditOutcome::Blocked(format!("{} is not editable here", item.name));
    };

    // Write to the file the value currently lives in: user-scoped → the global
    // config; project or unset → the project config.
    let scope = if item.scope.starts_with("~/.aida") {
        crate::glyph_config::Scope::User
    } else {
        crate::glyph_config::Scope::Project
    };
    let path = match crate::glyph_config::config_path_for(scope) {
        Ok(p) => p,
        Err(e) => return EditOutcome::Blocked(format!("cannot resolve config path: {e}")),
    };

    // Derive the toml value to write per edit kind. Bool toggles the current
    // stored value (read back from disk, not the rendered row, so a failed
    // write surfaces honestly); enum/integer write the TUI-supplied value.
    let new_value: toml_edit::Value = match meta {
        EditSafety::Bool { default } => {
            let current = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
                .and_then(|cfg| {
                    config_lookup(Some(&cfg), &item.section, &item.name).and_then(|v| v.as_bool())
                })
                .unwrap_or(default);
            toml_edit::Value::from(!current)
        }
        EditSafety::Enum { allowed } => {
            let Some(v) = requested else {
                return EditOutcome::Blocked(format!("no value supplied for {}", item.name));
            };
            if !allowed.contains(&v) {
                return EditOutcome::Blocked(format!(
                    "{v:?} is not an allowed value for {}",
                    item.name
                ));
            }
            toml_edit::Value::from(v)
        }
        EditSafety::Integer { min, max } => {
            let Some(v) = requested else {
                return EditOutcome::Blocked(format!("no value supplied for {}", item.name));
            };
            let Ok(n) = v.parse::<i64>() else {
                return EditOutcome::Blocked(format!("{v:?} is not an integer"));
            };
            if n < min || n > max {
                return EditOutcome::Blocked(format!("{n} is out of range [{min}, {max}]"));
            }
            toml_edit::Value::from(n)
        }
        // `config_knob_meta` filters ReadOnly out above, so this is unreachable —
        // but matched explicitly so a new EditSafety variant trips the compiler.
        EditSafety::ReadOnly { reason } => {
            return EditOutcome::Blocked(format!("{} is read-only: {reason}", item.name));
        }
    };

    if let Err(e) = crate::config_edit::set_kv(&path, &item.section, &item.name, new_value.clone())
    {
        return EditOutcome::Blocked(format!("write failed: {e}"));
    }

    match resolve_config_menu_row(project_root, &item.section, &item.name) {
        Some((value, scope)) => EditOutcome::Updated { value, scope },
        None => EditOutcome::Updated {
            value: rendered_toml_value(&new_value),
            scope: path.display().to_string(),
        },
    }
}

#[cfg(feature = "tui")]
fn menu_row_value(section: &str, key: &str, raw: &str) -> String {
    let plain = strip_ansi_color(raw);
    if section == "permissions" && matches!(key, "claude" | "codex" | "antigravity") {
        plain
            .split_whitespace()
            .next()
            .unwrap_or(plain.as_str())
            .to_string()
    } else if section == "mcp" && key == "aida_registered" {
        plain
            .split_whitespace()
            .next()
            .unwrap_or(plain.as_str())
            .to_string()
    } else {
        plain
    }
}

#[cfg(feature = "tui")]
fn cli_edit_permission_posture(
    project_root: &std::path::Path,
    item: &aida_tui::ConfigMenuItem,
    requested: Option<&str>,
) -> aida_tui::EditOutcome {
    use aida_tui::EditOutcome;

    let Some(requested) = requested else {
        return EditOutcome::Blocked(format!("no tier supplied for {}", item.name));
    };
    let tier = match requested {
        "contained" => ConfigPermissionTier::Contained,
        "native" => ConfigPermissionTier::Native,
        "bypass" => ConfigPermissionTier::Bypass,
        other => return EditOutcome::Blocked(format!("{other:?} is not an allowed tier")),
    };
    let scope = if item.scope.starts_with("~/.") {
        PermissionPostureScope::User
    } else {
        PermissionPostureScope::Local
    };
    if let Err(e) = apply_permission_posture(project_root, tier, scope) {
        return EditOutcome::Blocked(format!("write failed: {e}"));
    }
    match resolve_config_menu_row(project_root, &item.section, &item.name) {
        Some((value, scope)) => EditOutcome::Updated { value, scope },
        None => EditOutcome::Updated {
            value: requested.to_string(),
            scope: scope.label().to_string(),
        },
    }
}

// trace:STORY-1131 | ai:codex
fn local_aida_mcp_registered(project_root: &std::path::Path) -> bool {
    read_local_mcp_json(project_root)
        .ok()
        .flatten()
        .and_then(|root| {
            root.get("mcpServers")
                .and_then(|servers| servers.get("aida"))
                .cloned()
        })
        .is_some()
}

#[cfg(feature = "tui")]
fn cli_toggle_local_aida_mcp_registration(project_root: &std::path::Path) -> aida_tui::EditOutcome {
    use aida_tui::EditOutcome;

    let result = if local_aida_mcp_registered(project_root) {
        remove_local_aida_mcp_registration(project_root)
    } else {
        add_local_aida_mcp_registration(project_root)
    };
    if let Err(e) = result {
        return EditOutcome::Blocked(format!("MCP registration toggle failed: {e}"));
    }
    match resolve_config_menu_row(project_root, "mcp", "aida_registered") {
        Some((value, scope)) => EditOutcome::Updated { value, scope },
        None => EditOutcome::Updated {
            value: if local_aida_mcp_registered(project_root) {
                "yes".to_string()
            } else {
                "no".to_string()
            },
            scope: ".mcp.json".to_string(),
        },
    }
}

// trace:STORY-1131 | ai:codex
fn add_local_aida_mcp_registration(project_root: &std::path::Path) -> Result<()> {
    let mut root = read_local_mcp_json(project_root)?.unwrap_or_else(|| serde_json::json!({}));
    let root_obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!(".mcp.json must be a JSON object"))?;
    let servers = root_obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("mcpServers must be a JSON object"))?;
    let aida_exe = resolve_aida_exe();
    servers_obj.insert(
        "aida".to_string(),
        serde_json::json!({
            "command": aida_exe.to_string_lossy(),
            "args": ["mcp-serve"],
            "description": "AIDA — spec graph + coordination surface (STORY-361)"
        }),
    );
    write_local_mcp_json(project_root, &root)
}

// trace:STORY-1131 | ai:codex
fn remove_local_aida_mcp_registration(project_root: &std::path::Path) -> Result<()> {
    let Some(mut root) = read_local_mcp_json(project_root)? else {
        return Ok(());
    };
    let root_obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!(".mcp.json must be a JSON object"))?;
    if let Some(servers) = root_obj
        .get_mut("mcpServers")
        .and_then(serde_json::Value::as_object_mut)
    {
        servers.remove("aida");
    }
    write_local_mcp_json(project_root, &root)
}

// trace:STORY-1131 | ai:codex
fn read_local_mcp_json(project_root: &std::path::Path) -> Result<Option<serde_json::Value>> {
    let path = project_root.join(".mcp.json");
    match std::fs::read_to_string(&path) {
        Ok(body) => serde_json::from_str(&body)
            .map(Some)
            .with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

// trace:STORY-1131 | ai:codex
fn write_local_mcp_json(project_root: &std::path::Path, root: &serde_json::Value) -> Result<()> {
    let path = project_root.join(".mcp.json");
    let body = serde_json::to_string_pretty(root)?;
    aida_core::write_atomic(&path, format!("{body}\n"))
        .with_context(|| format!("writing {}", path.display()))
}

#[cfg(feature = "tui")]
fn cli_edit_agent_profile(
    project_root: &std::path::Path,
    item: &aida_tui::ConfigMenuItem,
) -> aida_tui::EditOutcome {
    use aida_tui::EditOutcome;

    let mut selection = crate::init_cmd::read_enabled_agent_selection(project_root).unwrap_or(
        crate::init_cmd::AgentSelection {
            claude: true,
            codex: true,
            antigravity: true,
        },
    );
    let currently_enabled = match item.name.as_str() {
        "claude" => selection.claude,
        "codex" => selection.codex,
        "antigravity" => selection.antigravity,
        _ => return EditOutcome::Blocked(format!("unknown agent profile {}", item.name)),
    };
    let enabling = !currently_enabled;
    match item.name.as_str() {
        "claude" => selection.claude = enabling,
        "codex" => selection.codex = enabling,
        "antigravity" => selection.antigravity = enabling,
        _ => {}
    }

    if let Err(e) = crate::init_cmd::write_enabled_agent_selection(project_root, selection) {
        return EditOutcome::Blocked(format!("write failed: {e}"));
    }
    if enabling {
        if let Err(e) = scaffold_enabled_agent_profile(project_root, &item.name) {
            return EditOutcome::Blocked(format!("enabled, but scaffold failed: {e}"));
        }
    }

    match resolve_config_menu_row(project_root, &item.section, &item.name) {
        Some((value, scope)) => EditOutcome::Updated { value, scope },
        None => EditOutcome::Updated {
            value: if enabling { "enabled" } else { "disabled" }.to_string(),
            scope: ".aida/config.toml".to_string(),
        },
    }
}

#[cfg(feature = "tui")]
fn scaffold_enabled_agent_profile(project_root: &std::path::Path, profile: &str) -> Result<()> {
    let mut selection = crate::init_cmd::AgentSelection {
        claude: false,
        codex: false,
        antigravity: false,
    };
    match profile {
        "claude" => selection.claude = true,
        "codex" => selection.codex = true,
        "antigravity" => selection.antigravity = true,
        _ => return Ok(()),
    }
    let mut config = ScaffoldConfig::default();
    selection.apply_to_scaffold_config(&mut config, false);
    let db_path = project_root.join(".aida").join("cache.db");
    let mut scaffolder = aida_core::scaffolding::Scaffolder::with_database(
        project_root.to_path_buf(),
        config,
        db_path,
    );
    let store = aida_core::RequirementsStore::default();
    let preview = scaffolder.preview(&store);
    for artifact in preview.artifacts {
        let rel = artifact.path.to_string_lossy().replace('\\', "/");
        let wanted = match profile {
            "claude" => rel == "CLAUDE.md" || rel == ".mcp.json" || rel.starts_with(".claude/"),
            "codex" => rel == "AGENTS.md" || rel.starts_with(".codex/"),
            "antigravity" => rel == "AGENTS.md" || rel.starts_with(".antigravity/"),
            _ => false,
        };
        if !wanted {
            continue;
        }
        let dest = project_root.join(&artifact.path);
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(dest, artifact.content)?;
    }
    Ok(())
}

/// Render a freshly-written toml value as the plain fallback display string
/// (used only when the live re-resolve can't find the row).
// trace:STORY-677
#[cfg(feature = "tui")]
fn rendered_toml_value(v: &toml_edit::Value) -> String {
    match v {
        toml_edit::Value::String(s) => s.value().to_string(),
        toml_edit::Value::Integer(i) => i.value().to_string(),
        toml_edit::Value::Boolean(b) => b.value().to_string(),
        other => other.to_string().trim().to_string(),
    }
}

/// Handle `aida config glyph ...` — the CLI surface over the glyph registry,
/// themes, and per-symbol override table (EPIC-45 phase 4). Pure ergonomics +
/// theme presets + a format-preserving TOML writer on top of the tested
/// resolution layer in [`crate::glyphs`] / [`crate::glyph_config`]. Adds NO new
/// resolution logic.
// trace:STORY-633 | ai:claude
pub(crate) fn handle_config_glyph(cmd: &GlyphCommand) -> Result<()> {
    use crate::glyph_config::{self, Scope};
    use crate::glyphs::{self, Glyph};

    let project_root = find_project_root().ok();

    match cmd {
        // List every glyph with its currently-resolved rendering + the unicode
        // registry default — the missing discoverability surface.
        GlyphCommand::List => {
            println!("{}", "Glyphs (name | current | unicode default)".bold());
            println!(
                "  Resolution: [glyphs] override > [ui] theme > [ui] glyphs profile > default"
            );
            if let Some(theme) = glyphs::active_theme(project_root.as_deref()) {
                println!("  Active theme: {}", theme.name.cyan());
            }
            println!();
            for g in Glyph::ALL {
                let current = glyphs::resolve_with_theme(g, project_root.as_deref());
                let default = g.unicode();
                let changed = current != default;
                let marker = if changed {
                    " *".yellow().to_string()
                } else {
                    String::new()
                };
                println!(
                    "  {:<14} {:<8} {}{}",
                    g.name().cyan(),
                    current,
                    default.dimmed(),
                    marker
                );
            }
            println!();
            println!(
                "  {} = differs from the unicode default. Customize: `aida config glyph set <name> <value>` or `aida config glyph theme <name>`.",
                "*".yellow()
            );
        }

        // Theme: no NAME (or `list`) → list embedded themes with a preview row.
        // A NAME → apply that theme (writing a reference, or --expand the bundle).
        GlyphCommand::Theme { name, expand, user } => {
            let list_mode = matches!(name.as_deref(), None | Some("list"));
            if list_mode {
                println!("{}", "Available glyph themes:".bold());
                println!();
                // Implicit default first.
                println!(
                    "  {:<12} {}",
                    "unicode".cyan(),
                    "Default — full emoji/unicode (no theme set).".dimmed()
                );
                for t in glyphs::THEMES {
                    // One-line preview: a handful of representative glyphs as
                    // this theme would render them.
                    let preview: String = [
                        Glyph::Check,
                        Glyph::Cross,
                        Glyph::Arrow,
                        Glyph::Done,
                        Glyph::Robot,
                    ]
                    .iter()
                    .map(|g| t.render(*g))
                    .collect::<Vec<_>>()
                    .join(" ");
                    println!("  {:<12} {}", t.name.cyan(), preview);
                    println!("  {:<12} {}", "", t.description.dimmed());
                }
                println!();
                println!(
                    "  Apply: `aida config glyph theme <name>` (add `--expand` to materialize into [glyphs])."
                );
                return Ok(());
            }

            let raw = name.as_deref().unwrap();
            let scope = if *user { Scope::User } else { Scope::Project };

            // "unicode" = clear any theme (return to the implicit default).
            if raw.eq_ignore_ascii_case("unicode") {
                let path = glyph_config::config_path_for(scope)?;
                // Clearing = unset the [ui] theme key by setting nothing; we
                // model it via expand-free path: just drop the reference.
                clear_theme_reference(&path)?;
                println!(
                    "{} cleared theme → unicode default ({})",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    scope_label(scope, &path)
                );
                return Ok(());
            }

            let theme = glyphs::theme_by_name(raw).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown theme `{}` — valid themes: {} (and `unicode` to reset)",
                    raw,
                    glyphs::valid_theme_names()
                )
            })?;

            let path = glyph_config::config_path_for(scope)?;
            if *expand {
                let base = match theme.base {
                    glyphs::GlyphProfile::Unicode => "unicode",
                    glyphs::GlyphProfile::Ascii => "ascii",
                };
                let bundle: Vec<(&str, &str)> =
                    theme.bundle.iter().map(|(g, s)| (g.name(), *s)).collect();
                glyph_config::expand_theme(&path, base, &bundle)?;
                println!(
                    "{} expanded theme `{}` into [glyphs] ({})",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    theme.name,
                    scope_label(scope, &path)
                );
            } else {
                glyph_config::set_theme(&path, theme.name)?;
                println!(
                    "{} theme = {} ({})",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    theme.name.cyan(),
                    scope_label(scope, &path)
                );
            }
        }

        GlyphCommand::Set { name, value, user } => {
            let glyph = Glyph::from_name(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown glyph `{}` — valid names: {}",
                    name,
                    valid_glyph_names()
                )
            })?;
            let scope = if *user { Scope::User } else { Scope::Project };
            let path = glyph_config::config_path_for(scope)?;
            glyph_config::set_override(&path, glyph.name(), value)?;
            println!(
                "{} {} = {} ({})",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                glyph.name().cyan(),
                value,
                scope_label(scope, &path)
            );
        }

        GlyphCommand::Unset { name, user } => {
            let glyph = Glyph::from_name(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown glyph `{}` — valid names: {}",
                    name,
                    valid_glyph_names()
                )
            })?;
            let scope = if *user { Scope::User } else { Scope::Project };
            let path = glyph_config::config_path_for(scope)?;
            let removed = glyph_config::unset_override(&path, glyph.name())?;
            if removed {
                println!(
                    "{} unset {} ({})",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    glyph.name().cyan(),
                    scope_label(scope, &path)
                );
            } else {
                println!(
                    "{} no override for {} ({})",
                    "·".dimmed(),
                    glyph.name(),
                    scope_label(scope, &path)
                );
            }
        }

        GlyphCommand::Reset { user } => {
            let scope = if *user { Scope::User } else { Scope::Project };
            let path = glyph_config::config_path_for(scope)?;
            let removed = glyph_config::reset_overrides(&path)?;
            if removed {
                println!(
                    "{} cleared all glyph overrides ({})",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    scope_label(scope, &path)
                );
            } else {
                println!(
                    "{} no glyph overrides to clear ({})",
                    "·".dimmed(),
                    scope_label(scope, &path)
                );
            }
        }
    }
    Ok(())
}

/// Drop the `[ui] theme` key, preserving the rest of the file. Used by
/// `aida config glyph theme unicode` to return to the implicit default.
// trace:STORY-633 | ai:claude
fn clear_theme_reference(path: &std::path::Path) -> Result<()> {
    use toml_edit::DocumentMut;
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let mut doc: DocumentMut = body.parse()?;
    if let Some(ui) = doc.get_mut("ui").and_then(|i| i.as_table_mut()) {
        ui.remove("theme");
    }
    aida_core::write_atomic(path, doc.to_string())?;
    Ok(())
}

/// The comma-separated valid glyph names, for `set`/`unset` error messages.
// trace:STORY-633 | ai:claude
fn valid_glyph_names() -> String {
    crate::glyphs::Glyph::ALL
        .iter()
        .map(|g| g.name())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A short "(scope: path)" suffix for glyph-command confirmations.
// trace:STORY-633 | ai:claude
fn scope_label(scope: crate::glyph_config::Scope, path: &std::path::Path) -> String {
    let which = match scope {
        crate::glyph_config::Scope::Project => "project",
        crate::glyph_config::Scope::User => "user",
    };
    format!("{}: {}", which, path.display())
}

/// Handle `aida config user` — show or update `~/.aida/preferences.toml`.
// trace:STORY-44 | ai:claude
pub(crate) fn handle_config_user(
    node_id: Option<&str>,
    email: Option<&str>,
    emit_toml: bool,
) -> Result<()> {
    let mut prefs = aida_core::UserPreferences::load()?;
    let mut changed = false;

    if let Some(id) = node_id {
        if id.is_empty() {
            if prefs.preferred_node_id.is_some() {
                prefs.preferred_node_id = None;
                changed = true;
            }
        } else {
            aida_core::node::validate_node_id(id)
                .map_err(|m| anyhow::anyhow!("invalid node id: {}", m))?;
            if prefs.preferred_node_id.as_deref() != Some(id) {
                prefs.preferred_node_id = Some(id.to_string());
                changed = true;
            }
        }
    }

    if let Some(em) = email {
        if em.is_empty() {
            if prefs.email.is_some() {
                prefs.email = None;
                changed = true;
            }
        } else if prefs.email.as_deref() != Some(em) {
            prefs.email = Some(em.to_string());
            changed = true;
        }
    }

    if changed {
        let path = prefs.save()?;
        println!("{} Saved preferences to {}", "".green(), path.display());
    }

    if emit_toml {
        print!("{}", toml::to_string_pretty(&prefs).unwrap_or_default());
        return Ok(());
    }

    let path_display = aida_core::UserPreferences::path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unknown home dir>".to_string());
    println!("User preferences ({})", path_display);
    if prefs.is_empty() {
        println!("  (no preferences set — try `aida config user --node-id JM`)");
    } else {
        println!(
            "  preferred_node_id: {}",
            prefs.preferred_node_id.as_deref().unwrap_or("(unset)")
        );
        println!(
            "  email:             {}",
            prefs.email.as_deref().unwrap_or("(unset)")
        );
    }
    Ok(())
}

/// BUG-533: `aida config show` effective-policy renderer helpers.
#[cfg(test)]
mod bug_533_config_show_tests {
    use super::*;

    #[test]
    fn config_lookup_finds_nested_key() {
        let cfg: toml::Value = "[telemetry]\nenabled = false\n".parse().unwrap();
        let v = config_lookup(Some(&cfg), "telemetry", "enabled").and_then(|v| v.as_bool());
        assert_eq!(v, Some(false));
    }

    #[test]
    fn config_lookup_absent_section_is_none() {
        let cfg: toml::Value = "[other]\nx = 1\n".parse().unwrap();
        assert!(config_lookup(Some(&cfg), "telemetry", "enabled").is_none());
        assert!(config_lookup(None, "telemetry", "enabled").is_none());
    }

    #[test]
    fn read_project_config_value_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_project_config_value(dir.path()).is_none());
    }

    #[test]
    fn read_project_config_value_parses_existing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[archive]\nauto_after_days = 30\n",
        )
        .unwrap();
        let cfg = read_project_config_value(dir.path()).unwrap();
        let days =
            config_lookup(Some(&cfg), "archive", "auto_after_days").and_then(|v| v.as_integer());
        assert_eq!(days, Some(30));
    }

    #[test]
    fn policy_source_labels_distinct() {
        // Labels carry ANSI color codes; assert the underlying text differs so
        // a default never reads the same as an env override.
        assert_ne!(
            PolicySource::Default.label(),
            PolicySource::ProjectConfig.label()
        );
        assert_ne!(
            PolicySource::ProjectConfig.label(),
            PolicySource::GlobalAgents.label()
        );
        assert!(PolicySource::Env("AIDA_TELEMETRY")
            .label()
            .contains("AIDA_TELEMETRY"));
    }

    // STORY-671 / TASK-793 anti-drift tests: `CONFIG_KNOBS` is the single source
    // of truth. `config show` (via policy_registry), `config menu` (default +
    // explanation + EditKind), and `config edit` validation all derive from it.
    // These assert the live-rendered registry stays in lockstep with the
    // declared knob table, so a knob added in code without a declaration — or a
    // declaration with no resolver — fails CI instead of silently vanishing (the
    // exact failure that produced BUG-533). trace:STORY-671 trace:TASK-793

    /// Every section a [`KnobSpec`] declares must be emitted by
    /// `policy_registry` — i.e. `aida config show` renders a live row for every
    /// declared section. A new section declared in `CONFIG_KNOBS` but with no
    /// resolution branch in the registry trips here.
    #[test]
    fn policy_registry_covers_known_sections() {
        let dir = tempfile::tempdir().unwrap();
        let rendered: std::collections::HashSet<&'static str> = policy_registry(dir.path())
            .iter()
            .map(|s| s.section)
            .collect();
        for section in known_config_sections() {
            assert!(
                rendered.contains(section),
                "config section `[{section}]` is declared in CONFIG_KNOBS but \
                 `policy_registry` does not emit it — add its resolution branch so \
                 `aida config show` renders it (anti-drift, STORY-671)"
            );
        }
    }

    /// The reverse guard: every section the registry emits must be declared in
    /// `CONFIG_KNOBS` (no stray/typo'd section name) AND the two section sets
    /// must be the same size, so neither can quietly drift out of sync. This is
    /// the true auto-discovery: a `[section]` resolved in `policy_registry` with
    /// no `KnobSpec` declaration fails CI.
    #[test]
    fn policy_registry_emits_only_known_sections() {
        let dir = tempfile::tempdir().unwrap();
        let sections = policy_registry(dir.path());
        let known: std::collections::HashSet<&'static str> =
            known_config_sections().into_iter().collect();
        for s in &sections {
            assert!(
                known.contains(s.section),
                "`policy_registry` emits section `[{}]` which is not declared in \
                 CONFIG_KNOBS — add a KnobSpec for it (anti-drift, STORY-671)",
                s.section
            );
        }
        // Each registered section appears exactly once and the counts match, so
        // neither set can carry an entry the other lacks.
        assert_eq!(
            sections.len(),
            known.len(),
            "policy_registry section count != CONFIG_KNOBS section count — \
             the registry and the declared knob table have diverged (STORY-671)"
        );
    }

    /// Every emitted section must carry at least one knob row — an empty section
    /// header in `config show` is a bug (the renderer would print a bare
    /// `[section]` with nothing under it).
    // trace:TASK-793
    #[test]
    fn policy_registry_sections_have_rows() {
        let dir = tempfile::tempdir().unwrap();
        for s in policy_registry(dir.path()) {
            assert!(
                !s.rows.is_empty(),
                "config section `[{}]` rendered no knob rows",
                s.section
            );
        }
    }

    /// Per-knob anti-drift (STORY-671): every concrete row `policy_registry`
    /// renders must resolve to a `KnobSpec` declaration — directly or via the
    /// section wildcard. A knob rendered in `config show` (and therefore offered
    /// in `config menu`) with no declaration would carry no doc/default/edit
    /// metadata, which is exactly the per-knob drift STORY-671 closes.
    #[test]
    fn every_rendered_knob_has_a_declaration() {
        let dir = tempfile::tempdir().unwrap();
        for s in policy_registry(dir.path()) {
            for row in &s.rows {
                assert!(
                    config_knob_spec(s.section, row.key).is_some(),
                    "config knob `[{}] {}` is rendered by policy_registry but has \
                     no KnobSpec declaration — declare it in CONFIG_KNOBS so its \
                     doc/default/edit-kind are not generic placeholders (STORY-671)",
                    s.section,
                    row.key
                );
            }
        }
    }

    /// The formerly-drifted `[field_study] enabled` knob (SPIKE-67) must now
    /// appear in `aida config show` — regression guard for STORY-671 acceptance
    /// criterion 3. It was invisible before because it was never added to the
    /// old hand-maintained lists.
    // trace:STORY-671
    #[test]
    fn field_study_enabled_appears_in_config_show() {
        let dir = tempfile::tempdir().unwrap();
        let found = policy_registry(dir.path())
            .iter()
            .any(|s| s.section == "field_study" && s.rows.iter().any(|r| r.key == "enabled"));
        assert!(
            found,
            "`[field_study] enabled` is declared in CONFIG_KNOBS but not rendered \
             by policy_registry — the SPIKE-67 knob regressed back to invisible \
             (STORY-671 acceptance #3)"
        );
    }

    #[test]
    fn agents_enabled_profiles_appear_in_config_show() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[agents]\nenabled = [\"codex\"]\n",
        )
        .unwrap();
        let agents = policy_registry(dir.path())
            .into_iter()
            .find(|s| s.section == "agents")
            .expect("agents section");
        let value_for = |key: &str| {
            agents
                .rows
                .iter()
                .find(|r| r.key == key)
                .map(|r| r.value.as_str())
                .unwrap_or("")
                .to_string()
        };
        assert_eq!(value_for("claude"), "disabled");
        assert_eq!(value_for("codex"), "enabled");
        assert_eq!(value_for("antigravity"), "disabled");
    }

    #[test]
    fn permission_posture_flags_full_access_without_codex_sandbox_table() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path());
        std::fs::create_dir_all(home.path().join(".aida")).unwrap();
        std::fs::write(
            home.path().join(".aida/agents.toml"),
            "[agents]\nbypass = true\n",
        )
        .unwrap();

        let report = permission_posture_report(dir.path());

        assert!(
            report
                .findings
                .iter()
                .any(|f| f.severity == "high" && f.id == "claude-full-access-no-sandbox"),
            "bypass without a Codex sandbox table must be called out loudly: {report:?}"
        );
    }

    #[test]
    fn permission_posture_flags_contained_without_codex_sandbox_table() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path());
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[contained]\nenable = true\n",
        )
        .unwrap();

        let report = permission_posture_report(dir.path());

        assert!(
            report.findings.iter().any(|f| {
                f.severity == "high" && f.id == "codex-contained-missing-codex-sandbox-table"
            }),
            "contained without [sandbox_workspace_write] must be flagged: {report:?}"
        );
    }

    // trace:BUG-1178 | ai:codex
    #[test]
    fn permission_posture_shows_claude_contained_per_launch_kind() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path());
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[contained]\nenable = true\n",
        )
        .unwrap();

        let report = permission_posture_report(dir.path());
        let claude = report
            .agents
            .iter()
            .find(|row| row.agent == "claude")
            .expect("claude row");

        assert_eq!(claude.tier, "contained");
        assert_eq!(claude.interactive_prompts, "native");
        assert_eq!(claude.headless_prompts, "no");
        assert!(!claude
            .interactive_flags
            .windows(2)
            .any(|w| w[0] == "--permission-mode" && w[1] == "dontAsk"));
        assert!(claude
            .headless_flags
            .windows(2)
            .any(|w| w[0] == "--permission-mode" && w[1] == "dontAsk"));
    }

    #[test]
    fn permission_posture_accepts_codex_workspace_sandbox_table() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path());
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[contained]\nenable = true\n",
        )
        .unwrap();
        std::fs::write(
            home.path().join(".codex/config.toml"),
            "sandbox_mode = \"workspace-write\"\n\
             approval_policy = \"on-request\"\n\
             [sandbox_workspace_write]\n\
             network_access = true\n\
             writable_roots = [\"/tmp/cache\"]\n",
        )
        .unwrap();

        let report = permission_posture_report(dir.path());

        assert!(
            report.findings.is_empty(),
            "workspace-write table satisfies contained diagnostics: {report:?}"
        );
        let codex = report
            .agents
            .iter()
            .find(|row| row.agent == "codex")
            .expect("codex row");
        assert_eq!(codex.network, "true (~/.codex/config.toml)");
        assert_eq!(codex.writable_roots, vec!["/tmp/cache".to_string()]);
    }

    // trace:STORY-1128 | ai:codex
    #[test]
    fn permissions_set_contained_is_parseable_idempotent_and_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
        std::fs::write(
            dir.path().join(".aida/agents.toml"),
            "[agents]\nclaude = true\n[agents.antigravity]\ndefault_flags = []\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".codex/config.toml"),
            "model = \"gpt-5\"\n[mcp_servers.aida]\ncommand = \"aida\"\n",
        )
        .unwrap();

        let first = apply_permission_posture(
            dir.path(),
            ConfigPermissionTier::Contained,
            PermissionPostureScope::Local,
        )
        .unwrap();
        let agents_once = std::fs::read_to_string(dir.path().join(".aida/agents.toml")).unwrap();
        let codex_once = std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap();
        apply_permission_posture(
            dir.path(),
            ConfigPermissionTier::Contained,
            PermissionPostureScope::Local,
        )
        .unwrap();

        assert_eq!(
            agents_once,
            std::fs::read_to_string(dir.path().join(".aida/agents.toml")).unwrap()
        );
        assert_eq!(
            codex_once,
            std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap()
        );
        toml::from_str::<toml::Value>(&agents_once).unwrap();
        let parsed_codex: toml::Value = toml::from_str(&codex_once).unwrap();
        assert_eq!(
            parsed_codex.get("sandbox_mode").and_then(|v| v.as_str()),
            Some("workspace-write")
        );
        assert_eq!(
            parsed_codex
                .get("sandbox_workspace_write")
                .and_then(|v| v.get("network_access"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(dir.path().join(".aida/agents.toml.bak").exists());
        assert!(dir.path().join(".codex/config.toml.bak").exists());
        assert_eq!(first.edited_paths.len(), 2);
        assert_eq!(first.backup_paths.len(), 2);
    }

    // trace:STORY-1128 | ai:codex
    #[test]
    fn permissions_set_scope_selects_local_or_user_paths() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path());

        apply_permission_posture(
            dir.path(),
            ConfigPermissionTier::Contained,
            PermissionPostureScope::Local,
        )
        .unwrap();
        assert!(dir.path().join(".aida/agents.toml").exists());
        assert!(dir.path().join(".codex/config.toml").exists());
        assert!(!home.path().join(".aida/agents.toml").exists());

        apply_permission_posture(
            dir.path(),
            ConfigPermissionTier::Contained,
            PermissionPostureScope::User,
        )
        .unwrap();
        assert!(home.path().join(".aida/agents.toml").exists());
        assert!(home.path().join(".codex/config.toml").exists());
    }

    // trace:STORY-1128 | ai:codex
    #[test]
    fn permissions_set_codex_table_stays_below_top_level_scalars() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
        std::fs::write(
            dir.path().join(".codex/config.toml"),
            "sandbox_mode = \"danger-full-access\"\napproval_policy = \"on-request\"\nmodel = \"gpt-5\"\n[mcp_servers.aida]\ncommand = \"aida\"\n",
        )
        .unwrap();

        apply_permission_posture(
            dir.path(),
            ConfigPermissionTier::Contained,
            PermissionPostureScope::Local,
        )
        .unwrap();

        let body = std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&body).unwrap();
        assert_eq!(
            parsed.get("sandbox_mode").and_then(|v| v.as_str()),
            Some("workspace-write"),
            "root scalar must remain root, not under a table: {body}"
        );
        assert!(
            body.find("model = \"gpt-5\"").unwrap()
                < body.find("[sandbox_workspace_write]").unwrap(),
            "workspace table must not land above leading root scalars: {body}"
        );
    }

    // trace:STORY-1128 | ai:codex
    #[test]
    fn permissions_set_native_removes_injected_posture() {
        let dir = tempfile::tempdir().unwrap();
        apply_permission_posture(
            dir.path(),
            ConfigPermissionTier::Contained,
            PermissionPostureScope::Local,
        )
        .unwrap();
        apply_permission_posture(
            dir.path(),
            ConfigPermissionTier::Native,
            PermissionPostureScope::Local,
        )
        .unwrap();

        let agents = std::fs::read_to_string(dir.path().join(".aida/agents.toml")).unwrap();
        let codex = std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap();
        assert!(!agents.contains("default_flags"), "{agents}");
        assert!(!agents.contains("contained"), "{agents}");
        assert!(!agents.contains("bypass"), "{agents}");
        assert!(!codex.contains("sandbox_mode"), "{codex}");
        assert!(!codex.contains("approval_policy"), "{codex}");
        assert!(!codex.contains("sandbox_workspace_write"), "{codex}");
    }

    // trace:STORY-1128 | ai:codex
    #[test]
    fn permissions_set_bypass_is_the_only_tier_with_codex_nuclear_flag() {
        let dir = tempfile::tempdir().unwrap();
        apply_permission_posture(
            dir.path(),
            ConfigPermissionTier::Contained,
            PermissionPostureScope::Local,
        )
        .unwrap();
        let contained = std::fs::read_to_string(dir.path().join(".aida/agents.toml")).unwrap();
        assert!(
            !contained.contains("--dangerously-bypass-approvals-and-sandbox"),
            "{contained}"
        );

        apply_permission_posture(
            dir.path(),
            ConfigPermissionTier::Bypass,
            PermissionPostureScope::Local,
        )
        .unwrap();
        let bypass = std::fs::read_to_string(dir.path().join(".aida/agents.toml")).unwrap();
        assert!(
            bypass.contains("--dangerously-bypass-approvals-and-sandbox"),
            "{bypass}"
        );
    }

    /// The doc table is now a view over the registry (STORY-671): a declared
    /// knob's `(doc, default)` come straight from its `KnobSpec`, never the
    /// generic fallback. Spot-checks the consolidation kept the framing.
    #[test]
    fn config_knob_doc_derives_from_registry() {
        // A declared knob returns its own doc + default, not the placeholder.
        let (doc, default) = config_knob_doc("telemetry", "enabled");
        assert!(doc.contains("telemetry"), "doc should describe telemetry");
        assert_eq!(default, "enabled");
        // The section wildcard covers a data-driven key.
        let (seats_doc, _) = config_knob_doc("seats", "anything_at_all");
        assert!(seats_doc.contains("seat"), "seats.* wildcard should apply");
        // An undeclared section falls back to the placeholder.
        let (fallback, _) = config_knob_doc("no_such_section", "no_such_key");
        assert_eq!(fallback, "(no description available)");
    }

    // trace:STORY-1131 | ai:codex
    #[test]
    fn mcp_registration_row_reflects_local_file_presence() {
        let dir = tempfile::tempdir().unwrap();
        let section = policy_registry(dir.path())
            .into_iter()
            .find(|s| s.section == "mcp")
            .expect("mcp section");
        let row = section
            .rows
            .iter()
            .find(|r| r.key == "aida_registered")
            .expect("aida_registered row");
        assert_eq!(row.value, "no");
        assert!(matches!(row.source, PolicySource::Default));

        add_local_aida_mcp_registration(dir.path()).unwrap();
        let section = policy_registry(dir.path())
            .into_iter()
            .find(|s| s.section == "mcp")
            .expect("mcp section");
        let row = section
            .rows
            .iter()
            .find(|r| r.key == "aida_registered")
            .expect("aida_registered row");
        assert!(row.value.starts_with("yes"), "{}", row.value);
        assert!(matches!(row.source, PolicySource::ProjectMcpJson));
    }

    // trace:STORY-1131 | ai:codex
    #[test]
    fn mcp_registration_remove_preserves_unrelated_servers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{
  "mcpServers": {
    "aida": { "command": "aida", "args": ["mcp-serve"] },
    "other": { "command": "other" }
  },
  "keep": true
}"#,
        )
        .unwrap();

        remove_local_aida_mcp_registration(dir.path()).unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap())
                .unwrap();
        assert!(parsed["mcpServers"].get("aida").is_none(), "{parsed}");
        assert_eq!(parsed["mcpServers"]["other"]["command"], "other");
        assert_eq!(parsed["keep"], true);
    }
}

/// STORY-671: the menu/edit surfaces derive their editability from `CONFIG_KNOBS`
/// too — these guards live in a `tui`-gated module so they compile only with the
/// editor present.
#[cfg(all(test, feature = "tui"))]
mod story_671_edit_kind_tests {
    use super::*;

    /// `config_knob_edit_kind` must derive from the registry: every editable
    /// `EditSafety` variant maps to the matching `EditKind`, and read-only /
    /// undeclared knobs map to `ReadOnly`.
    // trace:STORY-671
    #[test]
    fn edit_kind_derives_from_registry() {
        // Bool.
        assert_eq!(
            config_knob_edit_kind("telemetry", "enabled"),
            aida_tui::EditKind::Bool
        );
        // The formerly-drifted field_study knob is editable as a bool now.
        assert_eq!(
            config_knob_edit_kind("field_study", "enabled"),
            aida_tui::EditKind::Bool
        );
        // Enum carries its allowed set.
        assert_eq!(
            config_knob_edit_kind("ultraplan", "mode"),
            aida_tui::EditKind::Enum(vec![
                "never".to_string(),
                "on-demand".to_string(),
                "suggested".to_string(),
            ])
        );
        // Integer carries its range.
        assert_eq!(
            config_knob_edit_kind("archive", "auto_after_days"),
            aida_tui::EditKind::Integer { min: 7, max: 365 }
        );
        assert_eq!(
            config_knob_edit_kind("agents", "codex"),
            aida_tui::EditKind::Bool
        );
        assert_eq!(
            config_knob_edit_kind("permissions", "codex"),
            aida_tui::EditKind::Enum(vec![
                "native".to_string(),
                "contained".to_string(),
                "bypass".to_string(),
            ])
        );
        assert_eq!(
            config_knob_edit_kind("mcp", "aida_registered"),
            aida_tui::EditKind::Bool
        );
        // Read-only declarations and undeclared knobs are not editable.
        assert_eq!(
            config_knob_edit_kind("agents", "bypass"),
            aida_tui::EditKind::ReadOnly
        );
        assert_eq!(
            config_knob_edit_kind("contained", "enable"),
            aida_tui::EditKind::ReadOnly
        );
        assert_eq!(
            config_knob_edit_kind("no_such", "knob"),
            aida_tui::EditKind::ReadOnly
        );
    }

    /// `config_knob_meta` (the editor's write-back gate) returns the editable
    /// variants only — a read-only declaration yields `None` so the editor
    /// refuses it, preserving the STORY-669/677 read-only set.
    // trace:STORY-671
    #[test]
    fn config_knob_meta_filters_read_only() {
        assert!(matches!(
            config_knob_meta("telemetry", "enabled"),
            Some(EditSafety::Bool { default: true })
        ));
        assert!(matches!(
            config_knob_meta("archive", "auto_after_days"),
            Some(EditSafety::Integer { min: 7, max: 365 })
        ));
        // The read-only set stays read-only (no live edit).
        assert!(config_knob_meta("agents", "bypass").is_none());
        assert!(config_knob_meta("contained", "os_wrap").is_none());
        assert!(config_knob_meta("ui", "glyphs").is_none());
        assert!(config_knob_meta("seats", "anything").is_none());
    }

    /// STORY-1470: the `?` help overlay's operational-consequence line comes
    /// straight from the registry's `ReadOnly { reason }` — no separate
    /// hand-maintained table. An editable knob has no reason to surface.
    #[test]
    fn readonly_reason_derives_from_registry() {
        assert_eq!(
            config_knob_readonly_reason("agents", "bypass").as_deref(),
            Some("security-relevant — edit ~/.aida/agents.toml deliberately")
        );
        assert!(config_knob_readonly_reason("contained", "os_wrap").is_some());
        // Editable knobs carry no read-only reason.
        assert!(config_knob_readonly_reason("telemetry", "enabled").is_none());
        assert!(config_knob_readonly_reason("archive", "auto_after_days").is_none());
        // Undeclared knob: no reason either (falls back to a generic message
        // in the menu, not a fabricated one here).
        assert!(config_knob_readonly_reason("no_such", "knob").is_none());
    }

    // trace:STORY-1131 | ai:codex
    #[test]
    fn menu_permission_posture_edit_uses_posture_writer() {
        let dir = tempfile::tempdir().unwrap();
        let item = aida_tui::ConfigMenuItem {
            section: "permissions".to_string(),
            name: "codex".to_string(),
            value: "native".to_string(),
            default: "native".to_string(),
            scope: "default".to_string(),
            explanation: "".to_string(),
            edit: config_knob_edit_kind("permissions", "codex"),
            read_only_reason: None,
        };

        let outcome = cli_edit_permission_posture(dir.path(), &item, Some("contained"));
        match outcome {
            aida_tui::EditOutcome::Updated { value, .. } => assert_eq!(value, "contained"),
            aida_tui::EditOutcome::Blocked(reason) => panic!("{reason}"),
        }
        let codex = std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap();
        assert!(
            codex.contains("sandbox_mode = \"workspace-write\""),
            "{codex}"
        );
        assert!(dir.path().join(".aida/agents.toml.bak").exists());
    }

    // trace:STORY-1131 | ai:codex
    #[test]
    fn menu_mcp_toggle_adds_then_removes_local_registration() {
        let dir = tempfile::tempdir().unwrap();

        let added = cli_toggle_local_aida_mcp_registration(dir.path());
        match added {
            aida_tui::EditOutcome::Updated { value, .. } => assert_eq!(value, "yes"),
            aida_tui::EditOutcome::Blocked(reason) => panic!("{reason}"),
        }
        assert!(local_aida_mcp_registered(dir.path()));

        let removed = cli_toggle_local_aida_mcp_registration(dir.path());
        match removed {
            aida_tui::EditOutcome::Updated { value, .. } => assert_eq!(value, "no"),
            aida_tui::EditOutcome::Blocked(reason) => panic!("{reason}"),
        }
        assert!(!local_aida_mcp_registered(dir.path()));
    }
}
