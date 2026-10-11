//! agents.toml posture readers and writers (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

pub(crate) fn load_agent_default_flags(
    project_root: &std::path::Path,
    agent_type: &str,
) -> Result<Vec<String>> {
    let mut flags = Vec::new();
    if let Some(home) = aida_home_dir() {
        merge_agent_flags_from_file(&mut flags, &home.join(".aida/agents.toml"), agent_type)?;
    }
    merge_agent_flags_from_file(
        &mut flags,
        &project_root.join(".aida/agents.toml"),
        agent_type,
    )?;
    Ok(flags)
}

/// Parse an `agents.toml` into a generic `toml::Value`. STORY-495 uses raw
/// `Value` extraction (rather than a typed struct) so the `[agents] bypass`
/// scalar and the `[agents.<tool>] default_flags` tables can coexist in one
/// `[agents]` table without serde-flatten fragility on the `toml` backend.
pub(crate) fn parse_agents_toml(path: &std::path::Path) -> Result<Option<toml::Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("reading agent defaults config {}", path.display()))?;
    let value: toml::Value = toml::from_str(&body)
        .with_context(|| format!("parsing agent defaults config {}", path.display()))?;
    Ok(Some(value))
}

pub(crate) fn merge_agent_flags_from_file(
    flags: &mut Vec<String>,
    path: &std::path::Path,
    agent_type: &str,
) -> Result<()> {
    let Some(value) = parse_agents_toml(path)? else {
        return Ok(());
    };
    if let Some(arr) = value
        .get("agents")
        .and_then(|agents| agents.get(agent_type))
        .and_then(|tool| tool.get("default_flags"))
        .and_then(|f| f.as_array())
    {
        // Project config is applied after user config, so replacing here
        // implements the documented "user base, project override" rule.
        *flags = arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
    }
    Ok(())
}

/// STORY-495: resolve the uniform `[agents] bypass` knob — user base
/// (`~/.aida/agents.toml`) overridable by project (`.aida/agents.toml`).
/// Defaults to `false` (faithful native posture) when unset in both.
pub(crate) fn load_agents_bypass(project_root: &std::path::Path) -> Result<bool> {
    let mut bypass = false;
    if let Some(home) = aida_home_dir() {
        if let Some(v) = read_agents_bypass_from_file(&home.join(".aida/agents.toml"))? {
            bypass = v;
        }
    }
    if let Some(v) = read_agents_bypass_from_file(&project_root.join(".aida/agents.toml"))? {
        bypass = v;
    }
    Ok(bypass)
}

/// TASK-1558: resolve `[agents] mcp` with the same user-base-then-project precedence as
/// `bypass` / `contained`. Absent everywhere means the default surface (`off`).
// trace:TASK-1558 | ai:claude
pub(crate) fn load_agents_mcp(project_root: &std::path::Path) -> Result<session::AgentMcpSurface> {
    let mut surface = session::AgentMcpSurface::default();
    if let Some(home) = aida_home_dir() {
        if let Some(raw) = read_agents_string_from_file(&home.join(".aida/agents.toml"), "mcp")? {
            surface = session::AgentMcpSurface::parse(&raw)?;
        }
    }
    if let Some(raw) = read_agents_string_from_file(&project_root.join(".aida/agents.toml"), "mcp")?
    {
        surface = session::AgentMcpSurface::parse(&raw)?;
    }
    Ok(surface)
}

/// Read `[agents] <key>` as a string.
///
/// A key that is PRESENT but not a string is an error, not a miss. `as_str()`
/// alone returns `None` for `mcp = true` exactly as it does for an absent key,
/// so the caller would silently fall back to its default -- which for
/// `[agents] mcp` means a seat launches on `off` while the operator believes
/// they configured `aida` or `native`. TASK-1558's documented contract is that
/// an unrecognised value FAILS the launch rather than falling back; that has to
/// cover a wrong TYPE too, or the guard fails open on the most likely typo.
// trace:TASK-1558 | ai:claude
pub(crate) fn read_agents_string_from_file(
    path: &std::path::Path,
    key: &str,
) -> Result<Option<String>> {
    let Some(value) = parse_agents_toml(path)? else {
        return Ok(None);
    };
    let Some(found) = value.get("agents").and_then(|agents| agents.get(key)) else {
        return Ok(None);
    };
    match found.as_str() {
        Some(s) => Ok(Some(s.to_string())),
        None => anyhow::bail!(
            "`[agents] {key}` in {} must be a quoted string, but is {} — \
             fix the value (or remove the key to take the default) and retry",
            path.display(),
            agents_toml_type_name(found)
        ),
    }
}

/// TOML type name for an `[agents]` value, for the wrong-type refusal above.
// trace:TASK-1558 | ai:claude
pub(crate) fn agents_toml_type_name(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "a string",
        toml::Value::Integer(_) => "an integer",
        toml::Value::Float(_) => "a float",
        toml::Value::Boolean(_) => "a boolean",
        toml::Value::Datetime(_) => "a datetime",
        toml::Value::Array(_) => "an array",
        toml::Value::Table(_) => "a table",
    }
}

// trace:STORY-567 | ai:codex
pub(crate) fn load_agents_contained(project_root: &std::path::Path) -> Result<bool> {
    let mut contained = false;
    if let Some(home) = aida_home_dir() {
        if let Some(v) = read_agents_bool_from_file(&home.join(".aida/agents.toml"), "contained")? {
            contained = v;
        }
    }
    if let Some(v) =
        read_agents_bool_from_file(&project_root.join(".aida/agents.toml"), "contained")?
    {
        contained = v;
    }
    // TASK-798: accept `[contained] enable` in the project `.aida/config.toml`
    // as an alias of `[agents] contained`, so the whole contained posture
    // (enable + `[contained] allowed_hosts`) can live in one `[contained]`
    // block instead of being split across agents.toml and config.toml. Operator
    // decision (2026-06-14): unify under `[contained]`. Last-wins: the unified
    // location overrides the legacy `[agents] contained` when explicitly set, so
    // a migrated config takes effect. trace:TASK-798 | ai:claude
    let cfg = read_project_config_value(project_root);
    if let Some(v) = config_lookup(cfg.as_ref(), "contained", "enable").and_then(|v| v.as_bool()) {
        contained = v;
    }
    Ok(contained)
}

// trace:TASK-1482 | ai:claude
/// TASK-1482: known operator-local *presentation* env vars — how output
/// renders (format pin, agent-mode force, glyph profile, notice quieting) —
/// that must NOT leak from the launching shell into a spawned agent by
/// default. The child session decides its own output mode (its own TTY, its
/// own piping) rather than inheriting the parent's. Keep in sync with the
/// rows these vars have in `docs/environment-variables.md` and with the
/// `[agents] inherit_env` doc in `docs/cli/06-roles-sessions.md`.
pub(crate) const PRESENTATION_ENV_VARS: &[&str] = &[
    "AIDA_OUTPUT_FORMAT",
    "AIDA_AGENT_OUTPUT",
    "AIDA_GLYPHS",
    "AIDA_QUIET",
    "AIDA_PUSH_QUIET",
];

/// TASK-1482: which of `PRESENTATION_ENV_VARS` a launch will strip vs. keep,
/// computed against a snapshot of the parent environment. Both lists only
/// ever name a var that was actually PRESENT in the parent — an unset var is
/// neither stripped nor kept, since there is nothing to propagate either way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PresentationEnvPolicy {
    pub(crate) stripped: Vec<&'static str>,
    pub(crate) kept: Vec<&'static str>,
}

/// TASK-1482: pure classification of the presentation env vars against an
/// INJECTABLE snapshot of the parent environment (never `std::env::vars()`
/// called from inside this function) and the resolved `[agents] inherit_env`
/// keep-list. Callers pass `std::env::vars().collect()` for a real launch;
/// tests pass a synthetic map, so this is exercised without mutating the
/// process environment or needing the shared env lock.
// trace:TASK-1482 | ai:claude
pub(crate) fn classify_presentation_env(
    parent_env: &std::collections::HashMap<String, String>,
    keep: &[String],
) -> PresentationEnvPolicy {
    let mut policy = PresentationEnvPolicy::default();
    for var in PRESENTATION_ENV_VARS {
        if !parent_env.contains_key(*var) {
            continue;
        }
        if keep.iter().any(|k| k == var) {
            policy.kept.push(var);
        } else {
            policy.stripped.push(var);
        }
    }
    policy
}

/// TASK-1482: apply a computed policy to a `Command` about to spawn the
/// child. `Command` inherits the FULL parent environment by default, so a
/// kept var needs no action here — it's already inherited; a stripped var is
/// explicitly removed with `env_remove`, which overrides that default
/// inheritance for exactly that one key.
// trace:TASK-1482 | ai:claude
pub(crate) fn apply_presentation_env_policy(
    command: &mut std::process::Command,
    policy: &PresentationEnvPolicy,
) {
    for var in &policy.stripped {
        command.env_remove(var);
    }
}

/// TASK-1482: one-line human summary of a [`PresentationEnvPolicy`] for the
/// `--no-exec` preview and the `--show-context` launch-context body, so any
/// intentional output-mode inheritance is visible rather than silent.
// trace:TASK-1482 | ai:claude
pub(crate) fn describe_presentation_env(policy: &PresentationEnvPolicy) -> String {
    if policy.stripped.is_empty() && policy.kept.is_empty() {
        return "none set in the launching shell".to_string();
    }
    let mut parts = Vec::new();
    if !policy.stripped.is_empty() {
        parts.push(format!(
            "{} stripped (child decides its own output mode)",
            policy.stripped.join(", ")
        ));
    }
    if !policy.kept.is_empty() {
        parts.push(format!(
            "{} kept ([agents] inherit_env opt-in)",
            policy.kept.join(", ")
        ));
    }
    parts.join("; ")
}

/// TASK-1482: resolve the launch's `PresentationEnvPolicy` against the REAL
/// process environment — the one production call site `classify_presentation_env`
/// feeds from; every other caller (tests, the pure classifier) supplies its
/// own injectable map instead.
// trace:TASK-1482 | ai:claude
pub(crate) fn resolve_presentation_env_policy(
    project_root: &std::path::Path,
) -> Result<PresentationEnvPolicy> {
    let keep = load_agents_inherit_env(project_root)?;
    let parent_env: std::collections::HashMap<String, String> = std::env::vars().collect();
    Ok(classify_presentation_env(&parent_env, &keep))
}

/// TASK-1482: resolve `[agents] inherit_env` — the explicit, documented
/// opt-in that preserves selected presentation env vars (see
/// `PRESENTATION_ENV_VARS`) from the launching shell into the spawned agent
/// despite the default strip. Same user-base/project precedence as
/// `[agents] bypass`/`contained`: when the project `.aida/agents.toml` sets
/// the key at all, it REPLACES (not merges with) the global
/// `~/.aida/agents.toml` list — mirroring the existing bool "last one wins"
/// rule extended to an array. Naming a var here that isn't one of
/// `PRESENTATION_ENV_VARS` is a harmless no-op: this knob only ever restores
/// inheritance for vars this task strips, it doesn't grant a new one.
// trace:TASK-1482 | ai:claude
pub(crate) fn load_agents_inherit_env(project_root: &std::path::Path) -> Result<Vec<String>> {
    let mut keep = Vec::new();
    if let Some(home) = aida_home_dir() {
        if let Some(v) =
            read_agents_string_array_from_file(&home.join(".aida/agents.toml"), "inherit_env")?
        {
            keep = v;
        }
    }
    if let Some(v) =
        read_agents_string_array_from_file(&project_root.join(".aida/agents.toml"), "inherit_env")?
    {
        keep = v;
    }
    Ok(keep)
}

// trace:TASK-1482 | ai:claude
pub(crate) fn read_agents_string_array_from_file(
    path: &std::path::Path,
    key: &str,
) -> Result<Option<Vec<String>>> {
    let Some(value) = parse_agents_toml(path)? else {
        return Ok(None);
    };
    Ok(value
        .get("agents")
        .and_then(|agents| agents.get(key))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentPermissionPosture {
    Native,
    Contained,
    Bypass,
}

impl AgentPermissionPosture {
    pub(crate) fn agents_bypass(self) -> bool {
        matches!(self, Self::Bypass)
    }
}

/// Initialize the human's fail-closed choice for supervised bypass launches.
// trace:TASK-1500 | ai:codex
pub(crate) fn maybe_prompt_confirm_bypass() -> Result<()> {
    let Some(home) = aida_home_dir() else {
        return Ok(());
    };
    let path = home.join(".aida/config.toml");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if bypass_confirm::has_user_setting(&existing) {
        return Ok(());
    }
    if !authority_stdin_is_terminal() || !authority_stdout_is_terminal() {
        return Ok(());
    }
    eprint!("\nAsk before launching agents with permission prompts turned off (bypass)? [Y/n] ");
    use std::io::Write as _;
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
        return Ok(());
    }
    let value = match answer.trim().to_ascii_lowercase().as_str() {
        "" | "y" | "yes" => true,
        "n" | "no" => false,
        _ => return Ok(()),
    };
    config_edit::set_kv(
        &path,
        "agents",
        "confirm_bypass",
        toml_edit::Value::from(value),
    )?;
    if !value {
        eprintln!(
            "  Warning: bypass confirmation is off in {}; bypass launches will not ask.",
            path.display()
        );
    }
    Ok(())
}

/// TASK-698: first-machine-setup prompt for the agent permission posture.
/// Surfaces the STORY-495 / STORY-567 posture knobs at `aida init` so the
/// operator discovers them during onboarding rather than after an agent
/// unexpectedly prompts. Writes the chosen posture to GLOBAL config:
/// `~/.aida/agents.toml`, plus `~/.codex/config.toml` for contained
/// Codex-native sandbox settings.
///
/// Guards (all must hold to prompt):
///   - `~/.aida/agents.toml` is absent — idempotent; an existing file is
///     never prompted-over or overwritten (respect the user's config).
///   - stdin AND stdout are TTYs — non-interactive init never prompts and
///     leaves the native (faithful) default in place.
///
/// Default selection is **native** (bypass = false) — safe-by-default,
/// consistent with STORY-495's faithful-launcher philosophy. Bypass requires
/// an explicit pick. "Decide later" writes nothing, so a future `aida init`
/// re-surfaces the prompt.
// trace:TASK-1233 | ai:codex
pub(crate) fn maybe_prompt_agent_posture() -> Result<()> {
    let Some(home) = aida_home_dir() else {
        return Ok(());
    };
    let path = home.join(".aida/agents.toml");
    // Idempotent: never prompt or overwrite an existing config.
    if path.exists() {
        return Ok(());
    }
    // TTY-gated: non-interactive init writes nothing (native default).
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Ok(());
    }

    eprintln!();
    eprintln!("{}", "Machine setup — agent permission posture".bold());
    eprintln!(
        "  How should agents launched by `{}` handle permissions?",
        "aida agent new".cyan()
    );
    eprintln!(
        "    {}  Native  — each tool's own default (Claude prompts)  [default]",
        "1)".bold()
    );
    eprintln!(
        "    {}  Contained — no prompts, Codex workspace-write sandbox",
        "2)".bold()
    );
    eprintln!(
        "    {}  Bypass  — skip all prompts (trusted / autonomous workflows)",
        "3)".bold()
    );
    eprintln!(
        "    {}  Decide later — ask again on the next init",
        "4)".bold()
    );
    eprint!("  Choose [1]: ");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    let mut answer = String::new();
    if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
        // Treat read failure as "decide later" — write nothing.
        eprintln!(
            "  {} no choice read; leaving native default in place.",
            "Note:".dimmed()
        );
        return Ok(());
    }

    let posture = match answer.trim() {
        // Empty (bare Enter) and "1" both select the safe native default.
        "" | "1" => AgentPermissionPosture::Native,
        "2" => AgentPermissionPosture::Contained,
        "3" => AgentPermissionPosture::Bypass,
        "4" => {
            eprintln!(
                "  {} no agent posture written; rerun `aida init` to set it.",
                "Note:".dimmed()
            );
            return Ok(());
        }
        other => {
            eprintln!(
                "  {} unrecognized choice {:?}; leaving native default in place.",
                "Note:".dimmed(),
                other
            );
            return Ok(());
        }
    };

    write_global_agents_permission_posture(&path, posture)?;
    match posture {
        AgentPermissionPosture::Native => eprintln!(
            "  {} agents will keep their native posture ({} in ~/.aida/agents.toml).",
            "+".green(),
            "[agents] bypass = false".cyan()
        ),
        AgentPermissionPosture::Contained => eprintln!(
            "  {} agents will run contained ({} in ~/.aida/agents.toml; Codex sandbox in ~/.codex/config.toml).",
            "+".green(),
            "[agents] contained = true".cyan()
        ),
        AgentPermissionPosture::Bypass => eprintln!(
            "  {} agents will run with permissions bypassed ({} in ~/.aida/agents.toml).",
            "+".green(),
            "[agents] bypass = true".cyan()
        ),
    }
    Ok(())
}

// trace:TASK-1233 | ai:codex
pub(crate) fn write_global_agents_permission_posture(
    path: &std::path::Path,
    posture: AgentPermissionPosture,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    if matches!(posture, AgentPermissionPosture::Contained) {
        config_cmd::apply_permission_posture(
            std::path::Path::new("."),
            cli::ConfigPermissionTier::Contained,
            config_cmd::PermissionPostureScope::User,
        )?;
        return Ok(());
    }
    let body = format!(
        "# Per-agent launch defaults for `aida agent new` (machine-global).\n\
         #\n\
         # Uniform permission-bypass knob (STORY-495). When `bypass = true`,\n\
         # every supervised launch (`aida agent new`, `aida session new`,\n\
         # `aida session start --launch`, `aida queue work`) runs with\n\
         # permissions bypassed (Claude: `--permission-mode=bypassPermissions`).\n\
         # When `false`, launchers are faithful: they inject no permission flag,\n\
         # so each agent keeps its native posture (Claude prompts).\n\
         #\n\
         # Set during `aida init` first-machine setup (TASK-698). Edit freely.\n\
         [agents]\n\
         bypass = {}\n",
        posture.agents_bypass()
    );
    std::fs::write(path, body)
        .with_context(|| format!("writing agent permission posture {}", path.display()))?;
    Ok(())
}

pub(crate) fn read_agents_bypass_from_file(path: &std::path::Path) -> Result<Option<bool>> {
    read_agents_bool_from_file(path, "bypass")
}

pub(crate) fn read_agents_bool_from_file(
    path: &std::path::Path,
    key: &str,
) -> Result<Option<bool>> {
    let Some(value) = parse_agents_toml(path)? else {
        return Ok(None);
    };
    Ok(value
        .get("agents")
        .and_then(|agents| agents.get(key))
        .and_then(|b| b.as_bool()))
}
