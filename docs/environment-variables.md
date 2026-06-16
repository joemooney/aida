# Environment variables

The canonical reference for every `AIDA_*` environment variable AIDA reads.
Until now these were documented piecemeal — scattered across `CLAUDE.md`,
per-feature docs, and source comments. This chapter is the single place to look
up *what a variable does, its default, who sets it, and its scope*.

Inventory built by grepping every read pattern (`std::env::var`, `var_os`,
`env!`, shell `${VAR}`) across the workspace and reading each call site —
defaults below are verified against source, not guessed.

## How to read this chapter

**Precedence.** Where a variable shadows a `.aida/config.toml` setting, the
order is almost always **CLI flag → environment variable → config file →
built-in default**. The environment variable overrides config; an explicit CLI
flag overrides the environment variable.

**Boolean parsing.** AIDA has two boolean idioms:

- *Opt-out knobs* (a feature that defaults **on**): disabled by setting the var
  to one of `false` / `0` / `no` / `off` (case-insensitive). Any other value —
  or leaving it unset — leaves the feature enabled.
- *Opt-in flags* (a feature that defaults **off**): enabled by `1` (some also
  accept `true` / `yes` / `on`). Unset leaves the feature off.

The "Values" notes call out which idiom each variable uses.

**"Who sets it" legend:**

| Tag | Meaning |
| --- | --- |
| **user** | You set it (shell export, `.envrc`, CI config) to change behaviour. |
| **launch-path** | AIDA sets it on a child process it spawns — orchestrator phases, headless `claude -p`, `aida agent new`. You normally don't set these by hand; they're documented so you can recognise them in a process environment. |
| **dev** | Set by the `aida dev` shell helpers or exported manually during AIDA development. |
| **test** | Read only by the test harness / unit tests. Not a product knob. |
| **build** | A compile-time stamp injected by `build.rs` via `cargo:rustc-env`, baked into the binary — not read from the runtime environment. |
| **ops** | Set by whoever deploys `aida-server` (the REST/gRPC service). |

**"Scope" legend:** *process env* = read from the current process environment
(propagates to children); *shell session* = an export that persists for the
shell until cleared; *compile-time* = baked into the binary at build.

---

## Identity & session

Who the caller is, and which session/role/spec the work belongs to. Most of
these are exported by `aida role enter` or set by `aida agent new` when it
spawns an agent.

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_USER` | Queue identity — the `user_id` the work queue shards on. | Resolution cascade: `AIDA_USER` → `USER` → `USERNAME` → `"default"`. | user | process env |
| `AIDA_AUTHOR` | Default author stamped on work items and comments. | Cascade: `AIDA_AUTHOR` → `USER` → `USERNAME` → `"Unknown"`. | user | process env |
| `AIDA_SESSION_ROLE` | Active role persona for the shell (`implementer` / `advisor` / `reviewer` …). Gates role-restricted writes (e.g. `queue add` is advisor-only). | unset = no role. | launch-path (`aida role enter` export) | shell session |
| `AIDA_SESSION_PROJECT` | Project root for the active session. | unset = derived from the statusline project root. | launch-path (`aida role enter`) | process env |
| `AIDA_SESSION_SCOPE` | The spec/requirement ID the session is scoped to. | unset = `None`. | launch-path | process env |
| `AIDA_SESSION_PURPOSE` | Free-text purpose recorded when entering a role. | unset = none. | launch-path (`aida role enter`) | process env |
| `AIDA_SESSION_ID` | Unique session identifier for telemetry/audit correlation. | unset = `None`. | launch-path (`aida session start`) | process env |
| `AIDA_AI_TOOL` | Name of the AI interface in use (audit/telemetry attribution). | unset = `None` (empty treated as unset). | user | process env |
| `AIDA_AGENT_TYPE` | Agent flavour (`claude` / `codex` / `antigravity` / …). | Sniffed from `CODEX_*` / `ANTIGRAVITY_*` / `GEMINI_*` / `CLAUDE*` env prefixes; falls back to `"other"`. | launch-path (`aida agent new`) | process env |
| `AIDA_AGENT_NAME` | Unique name of the running agent process (e.g. `claude-3f2a`). | unset = `None`. | launch-path | process env |
| `AIDA_AGENT_REGISTRY_TOKEN` | Auth token for agent-registry writes. | unset = `None`. | launch-path | process env |
| `AIDA_ADVISOR_SESSION_UUID` | UUID of the live advisor session, used for fork-from-live discovery. | unset = falls back to an mtime scan. | launch-path (advisor orchestrator) | process env |
| `AIDA_REGISTRY_PATH` | Path to the requirements registry file (legacy/centralized layouts). | Cascade: `AIDA_REGISTRY_PATH` → `REQ_REGISTRY_PATH` → default location. | user / dev | process env |

---

## Autonomy & lifecycle

Knobs that govern unattended runs, the autonomy ladder, and the auto-promote /
auto-archive / auto-followup behaviours that fire on `aida pull` and at session
close.

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_ZEN` | Marks a zen-mode session — auto-resolves mechanical confirmation prompts (including merge). | unset = off. Enabled with `1`. | user (`--zen`, inherited by children) | process env |
| `AIDA_ZEN_TOKEN` | Per-invocation UUID proving zen provenance for standalone `--zen` sessions (bare UUID only). | unset = no standalone zen token. | launch-path (`--zen` dispatch) | process env |
| `AIDA_ZEN_PAUSE_ALWAYS` | Forces a standalone zen finish-checkpoint to pause at grab-next/stop even on a clean finish. | unset = off. Enabled with `1`. | launch-path (`--pause-always`) | process env |
| `AIDA_HEADLESS` | Signals skills that they are running unattended (set by headless launchers). | unset = interactive. Set to `1` when headless. | launch-path (headless launchers) | process env |
| `AIDA_NO_HUMAN_ACKNOWLEDGED` | Acknowledges the scope of `--no-human` so an unattended run can skip the interactive confirmation. | unset = interactive acknowledgement required. Set `1`. | user | process env |
| `AIDA_NO_HUMAN_MODE` | The `--no-human` scope slug propagated to phase children for the statusline. | unset = fully interactive. | launch-path (orchestrator) | process env |
| `AIDA_PERMISSION_MODE` | Overrides the agent permission posture (beats config and the `[agents] bypass` knob). | unset = config `[behavior] permission_mode`, else the AIDA-managed-worktree default. | user | process env |
| `AIDA_CALIBRATE` | Forces advisor calibration mode on/off. | unset = config `[advisor] calibration_mode`. `1` = force on, `0` = force off. | launch-path (`--calibrate` dispatch) | process env |
| `AIDA_LIFECYCLE_TAGS` | Comma-separated lifecycle short-circuit tokens (e.g. `lifecycle:no-ci-wait`). | unset = normal lifecycle. | user | process env |
| `AIDA_HINTS` | Enables/disables workflow hints. | unset = config `[hints] workflow_hints`, else enabled. Opt-out *and* opt-in idioms both honoured. | user | process env |
| `AIDA_WITH_PLAN` | Runs a plan-prelude session before phase 1 of a drain. | unset = no prelude. Enabled with `1`. | launch-path (`--with-plan` dispatch) | process env |
| `AIDA_AUTO_FOLLOWUPS` | Files each `## Followups` bullet as a child TASK on reaching Done/Completed. | enabled. Opt-out with `false`/`0`/`no`/`off`. | user | process env |
| `AIDA_AUTO_BUMP` | Auto-promotes `done → completed` on `aida pull` when a referencing commit lands on main. | enabled. Opt-out with `false`/`0`/`no`/`off`. | user | process env |
| `AIDA_AUTO_MERGE_GATE` | Runs the merge-gate (short-ID assignment) after a pull. | enabled. Opt-out with `false`/`0`/`no`/`off`. | user | process env |
| `AIDA_AUTO_ARCHIVE` | Auto-sweeps old completed/rejected specs on `aida pull` (gated on `[archive] auto_after_days`). | enabled when the config gate is set. Opt-out with `false`/`0`/`no`/`off`. | user | process env |
| `AIDA_CAPTURE_INTERFACE_CHANGES` | Captures interface-change notes at the close checkpoint. | enabled. Opt-out with `0`/`false`/`no`. | user | process env |
| `AIDA_PR_SHIP_NO_CI_WAIT` | Skips the CI wait on `aida pr ship`. | unset = wait for CI. Opt-in (truthy). | user | process env |
| `AIDA_TEE_HEADLESS` | Streams a headless child's chatter to the console (failures always stream regardless). | enabled. Opt-out with `0`/`false`/`off`. | user (`--no-tee-headless`) | process env |
| `AIDA_ALLOW_INTERMEDIATE_ONLY` | Opts out of the reproducibility check for intermediate-only base diffs. | unset = check enabled. Enabled with `1`/`true`. | launch-path (`--allow-intermediate-only`) | process env |
| `AIDA_ALLOW_INTERMEDIATE` | Bypasses the gitignored-file substrate-as-bouncer pre-commit hook gate. | unset = gate enforces. Enabled with `1`. | user | process env |
| `AIDA_PHASE_CEILING_MINUTES` | Phase watchdog ceiling (minutes). | `45`. | launch-path (`--phase-ceiling-minutes`) | process env |
| `AIDA_NO_PROGRESS_MINUTES` | No-progress watchdog threshold (minutes). | `10`. | launch-path (`--no-progress-minutes`) | process env |
| `AIDA_WORKER_SPEC_TIMEOUT` | Per-spec watchdog timeout (seconds) for the bash `timeout` wrapper in a drain loop. | `1800`. | user (drain script) | process env |
| `AIDA_DRAIN_FORCE` | Bypasses the global drain-instance lock (`.aida/drain.lock`) so a second `burndown run` / `queue work --auto-complete` may launch against the same tree. | unset = lock enforced. Override with `1`/`true`/`yes`/`on`. | user | process env |
| `AIDA_DRAIN_LOCK_STALE_SECS` | Age (seconds) past which a still-claimed drain lock is treated as stale and reclaimed even if its pid looks alive (pid-recycle backstop). | `1800`. | user | process env |
| `AIDA_COMMIT_STRICT` | The `aida-commit-msg` git hook rejects non-conforming commit messages. | `false`. Enabled with `true`. | user | process env |

> The bare orchestration signals `AIDA_AUTO_COMPLETE`, `AIDA_AUTO_COMPLETE_PHASE`,
> and `AIDA_AUTO_COMPLETE_TOKEN` are set on phase children by the drain
> orchestrator and corroborated against the drain-state run UUID. They are
> launch-path-only — listed under *Orchestrator-internal handshake* below.

---

## Orchestrator-internal handshake

The autonomous-drain orchestrator coordinates with its phase children
(implementer, reviewer, headless advisor) through env-provisioned file paths
and provenance tokens. **You never set these by hand** — they're documented so
you can recognise them in a child process's environment. Setting them in a
normal shell does nothing useful and is rejected when the corroborating token
doesn't match.

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_AUTO_COMPLETE` | Bare "this session is orchestrator-spawned" flag (only trusted with a matching token). | unset = standalone. Set `1`. | launch-path | process env |
| `AIDA_AUTO_COMPLETE_PHASE` | 1-based phase index (`1`–`6`) for the spawned phase child. | unset = standalone. | launch-path | process env |
| `AIDA_AUTO_COMPLETE_TOKEN` | Per-run UUID matching the drain-state `run_uuid`; corroborates `AIDA_AUTO_COMPLETE`. | unset = no token. | launch-path | process env |
| `AIDA_PUNT_SIGNAL_FILE` | Path the implementer writes a punt-signal file to when it hits a design-fork. | unset = no signal. | launch-path | process env |
| `AIDA_PUNT_REQUEST_FILE` | Path of the `PuntRequest` payload for the headless advisor subprocess. | unset = no request. | launch-path | process env |
| `AIDA_PUNT_RESPONSE_FILE` | Path the advisor writes its `PuntResponse` to. | unset = no handshake. | launch-path | process env |
| `AIDA_HOLD_SIGNAL_FILE` | Path the implementer writes a PR-hold signal file to. | unset = no signal. | launch-path | process env |
| `AIDA_REVIEW_VERDICT_FILE` | Path the reviewer writes its verdict JSON to. | unset = standalone review. | launch-path | process env |
| `AIDA_EXIT_SENTINEL` | Path the skill touches to signal clean completion for the graceful-exit handshake. | unset = non-orchestrated path. | launch-path | process env |
| `AIDA_AGENT_CONTEXT_FILE` | Path to the role-context snapshot handed to a spawned agent. | unset. | launch-path (`aida agent new`) | process env |
| `AIDA_PROJECT_ROOT` | **Outbound-only.** Project root handed *down* to a spawned agent subprocess; AIDA only ever *writes* it when launching a child agent. AIDA does **not** read it on the resolution side — `find_project_root` resolves reads by walking up from CWD for `.git`, so exporting `AIDA_PROJECT_ROOT` in an interactive shell does **not** redirect where `aida` looks (it's a silent no-op for reads). To target a different project, `cd` into it or use `AIDA_STORE` for the store. (BUG-567 Finding 2: documenting the name-vs-behavior mismatch rather than changing it, the lower-risk option.) | unset = computed at launch. | launch-path | process env |
| `AIDA_ASCIINEMA_WRAPPED` | Guards against double-wrapping when `--asciinema` re-execs `aida` under a recorder. | unset. Set `1` by the recorder subprocess. | launch-path | process env |

---

## Storage, cache & paths

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_STORE` | Targets a specific git-canonical store directory instead of walking up from CWD. Strict: a missing/invalid path (not a dir, or no `objects/` subdir) falls through to normal resolution rather than erroring — never breaks a forgotten-export shell. When set-but-unusable it now emits ONE informational stderr notice naming the path + reason + that it fell back (BUG-567 Finding 1); mute with `AIDA_QUIET`. | unset = `None` (normal resolution). | user | process env |
| `AIDA_QUIET` | Suppresses non-essential informational stderr notices (e.g. the `AIDA_STORE` set-but-unusable fall-through notice). Any value other than unset / empty / `0` / `false` enables it. | unset = notices shown. | user / scripts | process env |
| `AIDA_HOME` | Redirects the global home (`~/.aida`) lookup — used for cross-platform isolation and deterministic test paths. | unset = `dirs::home_dir()`. | dev / test | process env |
| `AIDA_CACHE_LOCK_STALE_SECS` | Seconds after which a cache lock is considered stale and can be reclaimed if its owner is dead. | `300`. | user | process env |
| `AIDA_CACHE_RETRY_COUNT` | Number of retry attempts for cache operations. | `8` (length of the default backoff schedule). `0` disables retries. | user / test | process env |
| `AIDA_CACHE_RETRY_MS` | Comma-separated millisecond backoff delays for cache retries. | `100,200,400,800,1600,3200,6400,12800`. | user / test | process env |
| `AIDA_DOCTOR_COMPLETED_SINCE` | Ref/date that exempts legacy history from `aida doctor completed-without-commit`. | unset = built-in recent cutoff. | user | process env |

---

## Git, fetch, forge & tokens

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_GITHUB_TOKEN` | GitHub PAT for API access. | unset = `None` (errors when a token is required). | user / launch-path | process env |
| `AIDA_GITLAB_TOKEN` | GitLab PAT; checked first in the GitLab token resolution. | unset = `None`. | user | process env |
| `AIDA_JIRA_TOKEN` | Jira API token. Cascade: `AIDA_JIRA_TOKEN` → `JIRA_API_KEY` → `JIRA_API_TOKEN`. | unset = `None`. | user | process env |
| `AIDA_PUSH_DEFAULT` | Default scope for `aida push` when neither `--code-only` nor `--store-only` is given. | unset = both legs. Values: `code`/`code-only` or `store`/`store-only`. | user | process env |
| `AIDA_BG_FETCH` | Enables the background store fetcher. | enabled. Opt-out with `false`/`0`/`no`/`off`. | user / test | process env |
| `AIDA_BG_FETCH_INTERVAL_SECS` | Skip a background fetch if the last attempt was within this window (seconds). | `300`. | user / test | process env |
| `AIDA_FETCH_FRESHNESS_SECS` | Staleness threshold (seconds) for rendering `cache:fresh\|behind` in status. | `300`. | user / test | process env |
| `AIDA_SERVER` | Remote gRPC server address for distributed mode. Overrides the `--server` flag when the flag is absent. | unset = `None`. Values: `host:port` or `grpc://host:port`. | user | process env |
| `AIDA_MCP_PROFILE` | MCP tool access tier. Resolution: CLI flag → env → config → default. | `full`. Values: `read-only` / `core` / `full`. | user | process env |
| `AIDA_GH_VERIFY_RETRIES` | Retry count for GitHub PR-verification during pull/merge. | `3`. | user | process env |
| `AIDA_SKIP_XPLAT_CHECK` | Skips the cross-platform pre-release validation gate in `scripts/release.sh`. | unset = gate enforced. Opt-in (truthy). | launch-path (`aida release --dev`) / user | process env |
| `AIDA_DEBUG_GH` | Traces `gh` binary resolution to stderr (every candidate tried + why rejected). | unset = off. Any non-empty value except `0` enables. | dev | process env |
| `AIDA_DEBUG_GLAB` | Traces `glab` (GitLab CLI) binary resolution to stderr — the GitLab sibling of `AIDA_DEBUG_GH`. | unset = off. Any non-empty value except `0` enables. | dev | process env |
| `AIDA_DEBUG_AUTOBUMP` | Traces the auto-bump decision logic during pull (commit counts, SHAs, scan range). | unset = off. Any non-empty value except `0` enables. | dev | process env |
| `AIDA_INIT_COMMIT_SCAFFOLD` | Whether `aida init` auto-commits scaffolding files. | unset = TTY-dependent (auto on non-TTY, prompt on TTY). Values: `1`/`true`/`yes`/`on` to commit, `0`/`false`/`no`/`off` to never. | user / launch-path | process env |

---

## Display & terminal

How AIDA renders glyphs to the terminal. The glyph profile is a per-machine /
per-terminal property (some terminals — e.g. Windows ConEmu — can't render all
the emoji defaults), so it is settable per-shell via the env var or
persistently via config. Opt-in by design: the default is UNICODE, so output is
unchanged unless you choose `ascii`. Nothing auto-downgrades.

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_GLYPHS` | Selects the glyph rendering profile. Values: `unicode` (emoji/unicode, the default) or `ascii` (curated ASCII fallbacks). Highest-priority tier of the selector: env > project `.aida/config.toml` `[ui] glyphs` > user `~/.aida/config.toml` `[ui] glyphs` > default `unicode`. | `unicode`. | user | process env |

**Config equivalent — `[ui] glyphs`:** set `glyphs = "ascii"` under a `[ui]`
table in `.aida/config.toml` (project) or `~/.aida/config.toml` (user-global)
to persist the profile without an env var. `AIDA_GLYPHS` overrides both; project
overrides user.

> **Phase-1 scope (STORY-628 / EPIC-45):** only glyphs migrated to the central
> registry honor the profile today (the status glyphs in `aida list` / `aida
> show` etc.); the long tail of raw glyph literals still prints unicode until
> the phase-3 migration (TASK-835).

---

## Telemetry & exit timing

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_TELEMETRY` | Kill-switch for local usage telemetry (`~/.aida/usage.jsonl`). | enabled. Opt-out with `0`/`false`/`no`/`off`. | user | process env |
| `AIDA_EXIT_GRACE_MS` | Grace window between SIGTERM and SIGKILL during an orchestrated exit. | `2000`. | launch-path / ops | process env |
| `AIDA_EXIT_POLL_MS` | Poll interval for the sentinel-file/child-process check during graceful exit. | `100` (min 1). | launch-path / ops | process env |

---

## Advisor intake

Set by the `aida intake` launcher when it spawns a cold-boot advisor. The
launcher reads the `[intake]` config section and exports the resolved policy to
the advisor subprocess; you tune the policy via config or the `aida intake`
flags rather than these vars directly.

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_INTAKE_CANDIDATES` | CSV of requirement IDs the advisor may act on (the bounded fence). | required when launched (no default). | launch-path | process env |
| `AIDA_INTAKE_APPLY` | Whether the advisor executes decisions or proposes only. | `0` = propose-only. `1` = execute. | launch-path | process env |
| `AIDA_INTAKE_DISPOSITION_BIAS` | Worth-doing posture for the cold-boot advisor. | `approve-eligible`. Also: `park-aligned`, `park-conservative`. | launch-path | process env |
| `AIDA_INTAKE_DO_NOT_APPROVE_CLASSES` | Requirement types the advisor can never approve. | `vision,epic,principle,constraint,decision,term`. | launch-path | process env |
| `AIDA_INTAKE_MAX_APPROVALS` | Cap on approvals per run. | unset = no cap. | launch-path | process env |
| `AIDA_INTAKE_ON_APPLY` | Action after approve. | `queue`. Also: `stop`, `drain`. | launch-path | process env |
| `AIDA_INTAKE_RISK` | Risk ceiling for approvable specs. | passed from config. | launch-path | process env |

---

## AI provider & models (`aida-server`)

Read by `aida-server`'s LLM layer (chat, evaluate). Defaults target the latest
Claude models; set these only when pointing at a proxy or a non-default model.

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_AI_PROVIDER` | LLM provider selection. | `anthropic`. Also: `openai`/`codex`/`gpt`. | ops / dev | process env |
| `AIDA_CHAT_MODEL` | Model ID for chat operations. | provider-dependent (Anthropic: `claude-sonnet-4-6`). | ops | process env |
| `AIDA_EVAL_MODEL` | Model ID for evaluation operations. | provider-dependent. | ops | process env |
| `AIDA_ANTHROPIC_BASE_URL` | Anthropic API base URL. | `https://api.anthropic.com`. | ops / dev | process env |
| `AIDA_OPENAI_BASE_URL` | OpenAI API base URL. | `https://api.openai.com`. | ops / dev | process env |
| `AIDA_INTENT_MODEL` | Fallback model label stamped on a generated `aida intent` comprehension when the `/aida-intent` skill's sidecar omits its own model id. | `claude`. | dev | process env |

---

## Web server: auth & sessions (`aida-server`)

Read by `aida-server`'s web-auth layer. All optional — the server runs
unauthenticated with in-memory sessions by default.

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_WEB_AUTH_MODE` | Web-UI authentication method. | `none`. Also: `pin`, `oidc`, `both`. | ops | process env |
| `AIDA_WEB_DEFAULT_ROLE` | Role for authenticated users not in a role list. | `editor`. Also: `admin`, `viewer`. | ops | process env |
| `AIDA_WEB_ADMIN_USERS` | CSV of handles granted admin. | empty. | ops | process env |
| `AIDA_WEB_EDITOR_USERS` | CSV of handles granted editor. | empty. | ops | process env |
| `AIDA_WEB_VIEWER_USERS` | CSV of handles granted viewer. | empty. | ops | process env |
| `AIDA_WEB_SESSION_STORE` | Session-storage backend. | `memory`. Also: `sqlite`/`persistent`. | ops | process env |
| `AIDA_WEB_SESSION_SQLITE_PATH` | SQLite session DB path (when store = `sqlite`). | `/tmp/aida-web-sessions.sqlite3`. | ops | process env |
| `AIDA_WEB_SESSION_TTL_HOURS` | Session expiry (hours). | `24` (min 1). | ops | process env |
| `AIDA_SERVER_API_KEY` | API key protecting general server endpoints. | unset = no auth required. | ops | process env |
| `AIDA_ADMIN_API_KEY` | API key protecting admin-only endpoints. | unset = falls back to `AIDA_SERVER_API_KEY`. | ops | process env |
| `AIDA_DATABASE_URL` | Database URL/path for single-project/legacy server mode. | unset = `determine_requirements_path(None)`. | ops / launch-path | process env |
| `AIDA_DEV_MODE` | Enables the server's admin/rebuild endpoints. | unset = disabled. Set `1`/`true`. | dev | process env |

### OIDC (`AIDA_WEB_AUTH_MODE=oidc`)

Required to enable OIDC: `AIDA_OIDC_CLIENT_ID` and `AIDA_OIDC_REDIRECT_URL`. The
auth/token/userinfo endpoints are derived from `AIDA_OIDC_ISSUER_URL` if not set
explicitly.

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_OIDC_CLIENT_ID` | OAuth client ID (required for OIDC). | unset = OIDC disabled. | ops | process env |
| `AIDA_OIDC_REDIRECT_URL` | OAuth callback URI (required for OIDC). | unset = OIDC disabled. | ops | process env |
| `AIDA_OIDC_CLIENT_SECRET` | OAuth client secret for token exchange. | unset = secret-less flow. | ops | process env |
| `AIDA_OIDC_ISSUER_URL` | Discovery endpoint; derives the URLs below. | unset. | ops | process env |
| `AIDA_OIDC_AUTH_URL` | Authorization endpoint. | derived from issuer (`…/protocol/openid-connect/auth`). | ops | process env |
| `AIDA_OIDC_TOKEN_URL` | Token endpoint. | derived from issuer (`…/token`). | ops | process env |
| `AIDA_OIDC_USERINFO_URL` | Userinfo endpoint. | derived from issuer (`…/userinfo`). | ops | process env |
| `AIDA_OIDC_SCOPES` | Space-separated scopes requested. | `openid profile email`. | ops | process env |

---

## Dev shell & build

### `aida dev` shell helpers

Set and read by the `aida dev activate` / `deactivate` shell helpers (see the
"AIDA-developer workflow" section of `CLAUDE.md`). You install the hooks once
with `aida dev shell-init --install`; the rest is automatic.

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_DEV_REPO` | Absolute path to the AIDA repo root — lets `aida-on` activate from any directory. | unset (or `--repo`). | dev (baked into shell-init) | shell session |
| `AIDA_DEV_ACTIVE` | Marker that dev activation is live. | `1` when active. | launch-path (`aida dev activate`) | process env |
| `AIDA_DEV_BIN` | Directory of the active dev `aida` binary. | set by activate. | launch-path | process env |
| `AIDA_DEV_PROFILE` | Active cargo profile (`debug`/`release`). | set by activate. | launch-path | process env |
| `AIDA_DEV_PROFILE_PIN` | Sticky profile override across re-activations. | unset = no pin. | launch-path (`--debug`/`--release`) | shell session |
| `AIDA_DEV_PREV_PATH` | Saved `PATH` for restore on deactivate. | set by activate. | launch-path (shell code) | process env |
| `AIDA_DEV_PS1_PREFIX` | The PS1 prefix activate splices in (stripped exactly on deactivate). | set by activate. | launch-path (shell code) | process env |
| `AIDA_SHELL_WRAPPER` | Signals the `aida dev` shell wrapper is active — gates bare vs `eval`-wrapped auto-eval hints. | unset = no wrapper. | launch-path (shell wrapper) | process env |

> `AIDA_DEV_PREV_PS1` is a legacy prompt-restore variable, superseded by the
> `AIDA_DEV_PS1_PREFIX` splice semantics; it now only appears in deactivate
> cleanup.

### Build-time stamps (`cargo:rustc-env`)

Injected by `aida-cli/build.rs` at compile time and read with `env!()` — **not**
read from the runtime environment. They power `aida --version` and the agent
registry's build identity.

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_BUILD_GIT_SHA` | Short git SHA at build time. | `unknown` if git unavailable. | build | compile-time |
| `AIDA_BUILD_GIT_DIRTY` | `1` if the worktree had uncommitted changes at build. | `0`. | build | compile-time |
| `AIDA_BUILD_UNIX_TIME` | Build timestamp (Unix epoch seconds). | `0` if unavailable. | build | compile-time |

---

## Test-harness internal

These are read only by unit tests / the test harness. They are **not** product
knobs and are listed for completeness so a grep of `AIDA_*` has no unexplained
hits.

| Variable | Purpose |
| --- | --- |
| `AIDA_TEST_HOME` | Overrides the home dir in `#[cfg(test)]` advisor/session isolation. |
| `AIDA_TEST_GH_BINARY` | Injects a mock `gh` binary path in tests. |
| `AIDA_TEST_GLAB_BINARY` | Injects a mock `glab` binary path in tests — the GitLab sibling of `AIDA_TEST_GH_BINARY`. |
| `AIDA_TEST_GUARD_NESTED` / `_RESET` / `_RESTORE` / `_UNSET` | `EnvVarGuard` unit-test fixtures. |
| `AIDA_TEST_TASK_63_APPLIED` | Fixture for the `apply_session_env_to_process` test. |

---

## Shell-script & git-hook knobs

Not read by the `aida` binary — these are honoured by the bundled shell scripts
(`scripts/`) and the scaffolded git hooks (`aida-core/templates/hooks/`). They
use shell `${VAR:-default}` defaulting.

### Commit-message hook (`aida-commit-msg`)

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_REQUIRE_REQ_FOR_FEAT` | Require a `(REQ-ID)` on `feat`/`fix` commits. | `true`. | user | process env |
| `AIDA_REQUIRE_AI_TAG` | Require an `[AI:tool]` tag when the commit has `trace:` comments. | `true`. | user | process env |

(`AIDA_COMMIT_STRICT`, documented above, is read by the same hook.)

### Drain loop (`scripts/drain-loop.sh`, `aida-drain-loop.service`)

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_DRAIN_CHUNK` | Items per drain pass (`nextN`). | `10`. | user | process env |
| `AIDA_DRAIN_IDLE` | Sleep seconds when the queue is dry. | `300`. | user | process env |
| `AIDA_DRAIN_MAXFAIL` | Shelve-and-continue cap (high = never halt on shelving). | `1000`. | user | process env |
| `AIDA_DRAIN_NOHUMAN` | Headless-mode flag passed to the drain. | `--no-human=both`. | user | process env |

### Release & maintenance scripts

| Variable | What it does | Default | Who sets it | Scope |
| --- | --- | --- | --- | --- |
| `AIDA_RELEASE_YES` | Auto-confirm `scripts/release.sh` prompts (non-TTY/CI). | `0`. | user | process env |
| `AIDA_CACHE_PATH` | Cache DB path used by `scripts/migrate-tag-namespace.sh`. | `.aida/cache.db`. | user | process env |

---

## Not environment variables

A grep for `AIDA_*` also surfaces these — they are **not** environment
variables and AIDA never reads them from the environment:

- `AIDA_BLOCK_BEGIN` / `AIDA_BLOCK_END` / `AIDA_MD_SKILLS_HEADING` — Rust `const`
  string markers used to parse autogenerated blocks in scaffolded markdown.
- `AIDA_TEST_ARGV_OUT` / `AIDA_TEST_ENV_OUT` — vestigial; appear only in test
  cleanup `remove_var` calls, never set or read.
- `AIDA_DEMO_SCRIPT_DIR` / `AIDA_DEMO_SCRIPT_PATH` — internal local variables in
  `scripts/aida-demo.sh`; not exported knobs.

---

*Source of truth: this chapter is derived from the read sites in the AIDA
workspace. If you add a new `AIDA_*` read, add a row here in the same change.*
