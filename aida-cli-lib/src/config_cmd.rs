//! `aida config` command cluster (SPIKE-78).
//!
//! The `aida config` surface: ID-format configuration + the effective-policy
//! renderer behind `config show`, the `tui`-gated `config menu` editor, the
//! `config glyph` theme commands, and `config user` / `config hints`. The
//! `CONFIG_KNOBS` registry is the single source of truth the show/menu/edit
//! surfaces derive from. Extracted verbatim from `main.rs`; no behavior change.

use anyhow::Result;
use colored::Colorize;

use crate::*;

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
                let confirm = inquire::Confirm::new("Are you sure you want to migrate?")
                    .with_default(false)
                    .prompt()?;
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
enum PolicySource {
    /// No file or env set this — the built-in default is in force.
    Default,
    /// Set in the project's `.aida/config.toml`.
    ProjectConfig,
    /// Set in the global `~/.aida/agents.toml` (agent permission posture).
    GlobalAgents,
    /// Set in the global `~/.aida/config.toml` (user-wide default). STORY-620.
    GlobalConfig,
    /// Overridden by an environment variable (named).
    Env(&'static str),
}

impl PolicySource {
    fn label(&self) -> String {
        match self {
            PolicySource::Default => "default".dimmed().to_string(),
            PolicySource::ProjectConfig => ".aida/config.toml".dimmed().to_string(),
            PolicySource::GlobalAgents => "~/.aida/agents.toml".dimmed().to_string(),
            PolicySource::GlobalConfig => "~/.aida/config.toml".dimmed().to_string(),
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
            PolicySource::GlobalAgents => "~/.aida/agents.toml".to_string(),
            PolicySource::GlobalConfig => "~/.aida/config.toml".to_string(),
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
    // --- [burndown]. ---
    KnobSpec {
        section: "burndown",
        key: "verbose",
        doc: "Default visibility for `aida burndown run`: stream live drain progress unless `--quiet` is passed.",
        default: "false",
        edit: EditSafety::Bool { default: false },
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
    // --- [archive]. ---
    KnobSpec {
        section: "archive",
        key: "auto_after_days",
        doc: "Auto-sweep completed/rejected specs older than N days on `aida pull` (clamped >=7; env: AIDA_AUTO_ARCHIVE).",
        default: "disabled",
        edit: EditSafety::Integer { min: 7, max: 365 },
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
            (Some(v), _) => (v, PolicySource::ProjectConfig),
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
        PolicySection {
            section: "agents",
            header: "[agents] — agent permission posture".to_string(),
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
        PolicySection {
            section: "burndown",
            header: "[burndown]".to_string(),
            rows: vec![PolicyRow {
                key: "verbose",
                value: effective.to_string(),
                source,
            }],
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
            items.push(aida_tui::ConfigMenuItem {
                section: section.section.to_string(),
                name: row.key.to_string(),
                value: strip_ansi_color(&row.value),
                default: default.to_string(),
                scope: row.source.plain_label(),
                explanation: explanation.to_string(),
                edit,
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
                return Some((strip_ansi_color(&row.value), row.source.plain_label()));
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
}
