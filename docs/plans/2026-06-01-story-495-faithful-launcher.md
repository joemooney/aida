# STORY-495 — Faithful launcher: native permission default + uniform bypass knob

- **Date:** 2026-06-01
- **Specs:** STORY-495 (builds on TASK-557, STORY-54, SPIKE-34)
- **Status:** In progress
- **Complexity:** Medium (broad but mechanical; one keystone safety invariant)

## Approach

Flip every **interactive/foreground** Claude launcher from "inject
`--permission-mode bypassPermissions` by default" to "inject nothing — honor
Claude's native prompting posture." Deviation moves to ONE uniform, auditable
opt-in: `[agents] bypass = true` in `~/.aida/agents.toml` (user base) overridable
by `.aida/agents.toml` (project). Codex/Antigravity were already faithful; the
knob now flips all three uniformly.

```
launcher flag (--permission-mode / --bypass-sandbox)   ─┐ explicit → wins
AIDA_PERMISSION_MODE env (queue work only)             ─┤
[behavior] permission_mode (queue work only)           ─┤
per-tool [agents.<tool>] default_flags  (TASK-557)     ─┤ present → overrides knob
[agents] bypass = true  (STORY-495 uniform knob)       ─┤ on  → tool's bypass flag
otherwise                                              ─┘ off → NATIVE (no inject)
```

Safety invariant: the headless drain (`claude_headless_args`) hardcodes
`bypassPermissions` and is structurally separate from every interactive path —
flipping the interactive default does NOT touch it. `--bg` (no answerable TTY)
force-injects bypass with a one-line note.

## Sites flipped

1. `aida session new`         — clap default (cli.rs ~458) → `Option<String>` None
2. `aida session start --launch` — clap default (cli.rs ~558) → `Option<String>` None
3. `aida agent new claude`    — clap default (cli.rs ~4062) → `Option<String>` None
4. `aida queue work` (interactive) — `resolve_queue_work_permission_mode` worktree default flipped

## Files (build order)

- `aida-cli/src/session.rs` — make `permission_mode` optional at the exec layer:
  `claude_session_args`, `exec_claude`, `exec_claude_new`, `exec_claude_with_session`,
  `spawn_claude_session`, `new_session` all take `Option<&str>`; only push
  `--permission-mode` when `Some`. Launch-log records `native` when None.
- `aida-cli/src/cli.rs` — three clap fields `permission_mode: String (default bypassPermissions)`
  → `Option<String>` (None when absent); refresh doc comments to describe the
  faithful default + `[agents] bypass` knob.
- `aida-cli/src/main.rs`:
  - `load_agents_bypass(project_root) -> Result<bool>` + `read_agents_bypass_from_file`
    (toml::Value based, robust against the existing default_flags table).
  - `tool_bypass_flags(agent_type) -> Vec<String>`.
  - `apply_agent_default_flags(..., explicit_permission: bool)` — inject the
    tool's bypass flag when `!explicit && per_tool.is_empty() && knob`.
  - `agent_new_claude/codex/antigravity` — drop the always-on bypass injection;
    set `explicit` from the launcher flag; thread it through
    `agent_new_with_config` / `agent_new_bg_dispatch`. `--bg` force-injects bypass.
  - `resolve_queue_work_permission_mode(flag, env, config, bypass_knob) -> (Option<String>, &str)`.
  - queue-work + `run_standalone_reviewer` thread `Option<String>`/`Option<&str>`.
  - `maybe_show_faithful_launcher_notice()` — one-time pointer to the knob,
    marker at `~/.aida/.faithful-launcher-notice`.
  - rewrite `merge_agent_flags_from_file` onto `toml::Value`; drop the
    `AgentsFlagConfigFile`/`AgentFlagConfig` structs.

## Critical Files

- `aida-cli/src/session.rs:659` `claude_session_args` — the single arg-builder both interactive launch paths share.
- `aida-cli/src/main.rs:76645` `resolve_queue_work_permission_mode` — queue-work resolution.
- `aida-cli/src/main.rs:20777` `apply_agent_default_flags` — where the knob is applied for `agent new`.
- `aida-cli/src/session.rs:812` `claude_headless_args` — the invariant that must NOT change.

## Reusable helpers

- `aida_home_dir()` — base for `~/.aida/agents.toml` + the notice marker.
- `load_agent_default_flags` / `merge_agent_flags_from_file` — existing TASK-557 loader to extend.
- `find_main_worktree_root()` — project root for the knob in session-new/start paths.

## Risks + gotchas

- serde `#[serde(flatten)]` + `toml` is flaky → use `toml::Value` extraction instead.
- 14 `AgentLaunchConfig {…}` literals → avoid adding struct fields; thread
  `explicit` as a function param instead.
- Headless/orchestrator paths must stay forced-bypass → verified separate.
- `--bg` must not silently hang → force bypass + note.

## Tests (named)

- `resolve_queue_work_permission_mode_*` — rewrite worktree/non-aida → native; add knob-on → bypass.
- `agents_bypass_knob_*` — user base, project override, off-by-default.
- `apply_agent_default_flags_knob_injects` / `_explicit_wins` / `_per_tool_flags_override`.
- `claude_session_args_native_omits_permission_mode` / `_some_injects`.
- regression: `headless_args_force_bypass_regardless_of_knob`.

## Verification

```
cargo build -p aida-cli
cargo test -p aida-cli permission_mode
cargo test -p aida-cli agents_bypass
cargo test -p aida-cli faithful
cargo fmt --all -- --check
```

## Followups

- Document the `[agents] bypass` knob in the discipline pack / CLAUDE.md agents.toml section.
- Consider extending session-new/start to honor AIDA_PERMISSION_MODE env like queue work (deferred — not required by acceptance).

## Related

- TASK-557 (agents.toml default_flags), STORY-54 (queue work --launch), SPIKE-34 (--bg dispatch), TASK-84 (queue-work permission resolution).
