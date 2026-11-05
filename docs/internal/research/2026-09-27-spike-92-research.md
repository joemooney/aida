# SPIKE-92 research: held wake hooks across harnesses

Date: 2026-09-27  
Scope: research only; no behavior, configuration, or requirement changes.  
Primary question: can a Stop/idle hook wait for addressed work without making a human-present seat unusable, and can the four harnesses reliably continue the turn?

## Executive finding

The safety-critical question—whether a TUI remains usable while its hook sleeps—was **NOT MEASURED** on any harness. No keyboard was sent to a live TUI: the spike expressly forbids keystroke injection, and there was no human-operated TUI probe available in this session. Hook documentation and reports that hooks are synchronous establish that hooks can hold agent-loop progress; they do not establish whether the prompt editor still accepts input.

Recommendation for CR-7 item 6: until a human-operated probe establishes otherwise, use **hold=0 for human-present seats** and reserve long held waits for headless seats, with a bounded timeout and resume fallback. A short grace period (for example 5 seconds) is a possible product choice, not a measured safe value. Do not describe a short grace as validated.

The four harnesses do not currently have equally strong evidence for the primary mechanism. In particular, the Antigravity CLI has an open report of hooks not executing for `GEMINI_API_KEY` headless runs, and OpenCode's `session.idle` occurs after the loop breaks; its issue tracker describes a teardown race in `opencode run`. Use resume-on-addressed-work as the fallback for those paths until tested on current releases.

## Evidence and provenance

**Verified by this researcher:** local executable presence and versions, the existing Codex hook trust records, official vendor documentation, and the linked primary issue reports as retrieved on 2026-09-27.  
**Not verified by live harness execution:** sleep duration behavior, TUI input, cancellation, actual timeout/kill ceilings, hook decision payloads, Antigravity headless firing, or OpenCode headless plugin execution. Vendor documentation is labeled as documented behavior, not a measurement. Issue reports are attributed to their reporters and versions; they are not reproduced here.

Local versions: Claude Code `2.1.283`; Codex CLI `0.157.1`; Antigravity CLI `1.2.12` and `agy 1.107.0`; OpenCode executable absent. The Codex config contains persisted trust hashes for hooks in this checkout, including a Stop entry. This confirms existing trust state here, not that a newly scaffolded Stop handler would be trusted.

## Per-harness findings

### Claude Code

| Question | Finding |
|---|---|
| TUI accepts/buffers input during `sleep 30` Stop hook | **NOT MEASURED.** No TUI sleep probe was run. Official docs define Stop as firing when Claude finishes responding, but do not say whether the prompt editor remains interactive while a command hook runs. |
| Ctrl+C / Esc cancellation and aftermath | **NOT MEASURED.** No live held hook was interrupted. |
| Actual hold limit | **NOT MEASURED.** Documentation says command hooks default to 600 seconds on most events and can set a per-hook timeout; it does not establish the actual cap/kill behavior on this installed build. |
| Exit 2 + stderr continuation | **NOT MEASURED live.** Documented: exit code 2 blocks/stops normal completion; on Stop, stderr is fed back as continuation context. |
| Headless hook behavior | **NOT MEASURED for this spike.** No `claude -p` run with a Stop hook was launched. |

Primary source: [Claude Code hooks reference](https://code.claude.com/docs/en/hooks) documents Stop, exit-code handling, and timeout defaults. The docs also describe command hooks as awaited/synchronous in the lifecycle. That does not answer TUI input responsiveness.

### Codex

| Question | Finding |
|---|---|
| TUI accepts/buffers input during `sleep 30` Stop hook | **NOT MEASURED.** No TUI sleep probe was run. |
| Ctrl+C / Esc cancellation and aftermath | **NOT MEASURED.** |
| Actual hold limit | **NOT MEASURED.** Official documentation gives a 600-second default for most hooks and says `timeout` is in seconds; this is configuration guidance, not an observed maximum. |
| Exit 2 + stderr continuation | **NOT MEASURED live.** Documented: a Stop hook may exit 2 and emit the reason on stderr to continue the turn. |
| Headless hook behavior | **NOT MEASURED.** No `codex exec` Stop-hook run was launched. |
| One-time `/hooks` trust | **DOCUMENTED; local trust state observed.** Codex presents hook definitions in `/hooks`; the user must review and trust the exact non-managed hook definitions. Trust is bound to the current hook hash, so adding/changing a definition requires renewed review. Until trusted, an unmanaged hook is skipped. This is a per-definition human gate, so project scaffolding cannot make a new hook trusted unattended through the ordinary user-facing flow. This checkout has persisted hashes already, which is not evidence that another project or changed command is pre-trusted. |

Primary source: [Codex hooks documentation](https://developers.openai.com/codex/hooks) documents Stop output, exit 2, timeout units/defaults, and hook trust. The currently installed Codex CLI also exposes `--dangerously-bypass-hook-trust`; that is an explicit bypass, not a recommended unattended setup path.

### Antigravity CLI (`agy`)

| Question | Finding |
|---|---|
| TUI accepts/buffers input during `sleep 30` Stop hook | **NOT MEASURED.** |
| Ctrl+C / Esc cancellation and aftermath | **NOT MEASURED.** |
| Actual hold limit | **NOT MEASURED.** Current hook docs describe a per-handler `timeout` in seconds. The installed CLI version is 1.2.12; no runtime timeout boundary was exercised. |
| `{"decision":"continue","reason":...}` dialect; whether `allow` is valid | **NOT MEASURED live.** Current official docs specify the Stop output as `decision: "continue"` to continue and say any other value allows stop. `allow` is not a documented Stop decision. The docs do not present an enum of `stop|continue|block`; they specify an open string with `continue` as the sole continuation value. Thus the safe documented contract is `continue` / `stop`, not `allow`. |
| Hooks fire headless with `GEMINI_API_KEY` | **NOT MEASURED on current version.** Open issue [antigravity-cli #893](https://github.com/google-antigravity/antigravity-cli/issues/893) reports that `.agents/hooks.json` was loaded but its hooks did not execute in `agy -p` with `GEMINI_API_KEY` on 1.1.22; the reporter says OAuth control runs did execute them. The issue remains open in the retrieved source. Because the report predates installed 1.2.12 and is not reproduced here, it is a live risk signal, not proof of current behavior. Until tested, headless API-key runs need the resume fallback. |

Primary sources: [Antigravity hook documentation](https://antigravity.google/docs/hooks?tab=cli) documents Stop payload/output and hook configuration; [issue #893](https://github.com/google-antigravity/antigravity-cli/issues/893) is the version-specific headless report. Antigravity's changelog notes a fix to hook ordering so Stop hooks run before built-in termination checks, but it does not say issue #893 was fixed.

### OpenCode

| Question | Finding |
|---|---|
| TUI accepts/buffers input during a held idle callback | **NOT MEASURED.** No OpenCode binary is installed here. |
| Ctrl+C / Esc cancellation and aftermath | **NOT MEASURED.** |
| Actual hold limit | **NOT MEASURED.** `session.idle` is an event callback, not a documented Stop hook with a timeout setting. No held callback was exercised. |
| Continue dialect | **NOT MEASURED live.** There is no vendor Stop-hook dialect in the proposed design: a plugin observes `session.idle` and calls `client.session.prompt` with a brief. |
| Plugin loads in headless runs | **NOT MEASURED.** Official plugin docs say local `.opencode/plugins` files are auto-loaded at startup and list `session.idle`; they do not provide a specific assurance for a long-lived held callback in each headless mode. The open [issue #16626](https://github.com/anomalyco/opencode/issues/16626) states `session.idle` fires after the loop has broken and reports a teardown race when re-prompting in `opencode run`. A persistent server/headless mode may have a different lifecycle, but this was not tested. Treat one-shot `opencode run` as requiring resume fallback unless a current-version reproduction proves the plugin can safely re-enter. |

Primary sources: [OpenCode plugin docs](https://dev.opencode.ai/docs/plugins/) list plugin discovery and `session.idle`; [issue #16626](https://github.com/anomalyco/opencode/issues/16626) is the maintainer-tracked request and rationale for the missing pre-stop seam. The issue does not establish that all headless plugin execution fails.

## Cross-harness answers by question

1. **TUI input while held:** NOT MEASURED for Claude, Codex, Antigravity, and OpenCode. No safe non-injection route was available for typing during the probe; do not infer input lock or responsiveness from synchronous hook docs.
2. **Ctrl+C / Esc and recovery:** NOT MEASURED on all four. No claims about resume, abort, or wedge behavior are supported.
3. **Actual hold timeout/kill limit:** NOT MEASURED on all four. Documented config defaults are not actual runtime limits: Claude 600s default on most hook events; Codex 600s default on most hooks; Antigravity exposes a configured seconds timeout; OpenCode idle plugins have no documented hook timeout. Measure boundary and cancellation separately on current versions before selecting production hold values.
4. **Continue dialect:** NOT MEASURED live on all four. Documented contracts: Claude exit 2 + stderr on Stop; Codex exit 2 + stderr for Stop; Antigravity stdout JSON with `decision: "continue"` and optional `reason` (not `allow`); OpenCode plugin prompt via SDK. No dialect should be represented as tested.
5. **Antigravity `GEMINI_API_KEY` headless:** NOT MEASURED here. Open issue #893 reports non-execution at 1.1.22. Use resume fallback until a current-version test closes this risk.
6. **OpenCode headless plugin:** NOT MEASURED. Local OpenCode binary absent. Docs establish auto-discovery and event existence, while issue #16626 flags a one-shot `opencode run` teardown race. Resume fallback is the safe path for one-shot headless runs; evaluate persistent server mode separately.
7. **Codex trust:** documented, and this checkout has prior persisted hook hashes. A new unmanaged Stop hook must be reviewed/enabled in Codex's `/hooks` UI before it runs; changing its definition/hash repeats the trust gate. Ordinary scaffolding alone cannot fulfill that interactive step.

## Options and trade-offs for CR-7 item 6

1. **Human-present hold=0 (recommended pending measurement).** No waiting hook can hold a shared interactive TUI. This preserves immediate human input and routes wake-up through resume-on-addressed-work. It requires the already-proposed fallback and session lifecycle handling.
2. **Human-present short grace (e.g. 5 seconds).** Gives a brief chance for an event to arrive without creating a long wait. There is no measurement showing 5 seconds is safe, and even a short sleep may affect a given harness's TUI; choose only as a provisional experiment, then validate interactively.
3. **Long held hook for human-present seats.** Avoids resume machinery when successful, but the critical input/cancel/timeout behavior is unknown. Not recommended until all four interactive probes pass on supported versions.
4. **Long held hook for headless seats only.** Avoids TUI input risk, but remains conditional on each headless harness actually firing the hook and honoring its continuation contract. Keep the resume fallback for Antigravity API-key mode and OpenCode one-shot mode until measured.

## Recommendation

For CR-7 item 6, set the human-present seat policy to **hold=0** pending direct TUI measurement. Keep any longer wait for explicitly headless seats only, and put a finite cap below the tested harness timeout once those limits are measured. Use resume-on-addressed-work for Antigravity `GEMINI_API_KEY` headless runs and OpenCode `opencode run` unless current-version tests establish working hook delivery and safe re-entry. This is a conservative policy recommendation from missing critical evidence plus documented failure modes; it is not an empirical finding that the TUI locks.

Next useful measurement is a human-operated run on each TUI with a literal `sleep 30` Stop handler: note prompt acceptance/buffering, try Ctrl+C and Esc separately, record the post-cancel state, then repeat with gradually longer sleeps to observe the actual ceiling. Separately run disposable, isolated headless sessions for each dialect, Antigravity API-key auth, and OpenCode persistent versus one-shot modes. Do not use polling, tmux scraping, or keystroke injection.

## CR-7 recording status

Per the operator instruction, this research did **not** edit SPIKE-92 or CR-7. The findings are recorded in this report and in the associated plan. CR-7 therefore still needs an advisor/operator-side note or link if its requirement history must contain the result directly.
