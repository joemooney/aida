# Chapter 6 — Roles & sessions

This chapter is about **who is working, and on what** — the machinery that lets AIDA keep a single human, a fleet of agents, and a pile of concurrent worktrees from stepping on each other. It's the least "spec-shaped" chapter in the manual and the most *operational*: roles, sessions, leases, the supervised launcher, and the channels agents use to talk to each other.

> Manual contract reminder: rationale, not flag tables. `aida <cmd> --help` is the source of truth for exact flags and defaults — this manual owns *when and why*.

---

## Mental model for the whole chapter

AIDA has a **seat model**. At any moment a working context is wearing one of a small set of *hats* — **advisor** (the persistent strategic + tactical partner: routes work, captures friction, grooms the queue, signs off architecture), **implementer** (writes the code for one spec on one branch), **reviewer** (reads the *diff* against acceptance criteria and votes merge/rework), and the human **product** owner who disposes drafts and sets direction. These aren't job titles bolted on for flavour — they're *authority boundaries*. Approval and queueing are advisor-gated; review verdicts come from the reviewer seat; the implementer's job is to satisfy a spec's contract, not to re-litigate it. `aida role enter` is how a human at a terminal *takes a seat*: it issues a session-bound **grant** for a roster-allowed seat, and that grant is what the rest of the CLI (queue/approve gating) and the MCP server validate before an authority-gated action proceeds. The role name also shapes what you *see* (scope filters, role-context injection) — but seeing and being allowed are two different things, and only the grant decides the second.

A **session** is a unit of work with a *lease* on a *scope*. When an implementer picks up `<spec-id>`, `aida session start` spins a sibling git worktree on a fresh branch and writes a **lease** — a small file that says "this session owns `<spec-id>`, on this branch, in this worktree, held by this owner." The lease is the collision-avoidance substrate: before two agents both grab the same spec, `aida session leases` answers "who holds what right now?" so the second one routes around it. v1 leases are *advisory* (not yet hard-enforced by `edit`/pickup), but in a multi-agent drain they are the coordination contract that keeps the fan-out from double-driving a spec. Note the deliberate split between `aida session leases` (active *scoped work* — the "what's running" view) and `aida session conversations` (historical *Claude Code conversations* — a different, JSONL-backed concept; once `session list`, now a deprecated alias); reach for `leases` when you mean "who's working."

Around that core sit the **launchers** and the **comms**. `aida agent new` and `aida session new`/`start --launch` spawn an actual Claude/Codex process with the right cwd, env, role, and a startup context snapshot — the supervised way to put an agent on a spec rather than copy-pasting a prompt. And because a fleet of agents needs to coordinate without sharing scrollback, two file-backed channels carry the traffic: **briefs** (`aida brief` — one-directional "here's your next pickup," operator → agent) and the **mailbox** (`aida mailbox` — peer↔peer messaging between agents). The `aida advisor` family is the seam where the *live* advisor session registers itself so any headless advisor pass — the away **watch loop** *and* the in-drain escalation tier — forks its full context when registered (cold-booting only as fallback; see `aida advisor` below). Keep the three substrates distinct as you read: **leases** coordinate *work*, **briefs** route *assignments*, the **mailbox** carries *conversation*.

---

### `aida role`

**One line** — declare which hat this shell is wearing, persistently across sessions.

**Mental model.** A role is a *named, resumable context* — a persona (advisor / implementer / reviewer / …) stored under `.aida/roles/` that survives shell restarts. Entering a role does four load-bearing things: at an interactive TTY it **issues a session grant** for that seat (checked against your team roster — the roster caps which seats you may take, it is not itself the active seat), it exports `$AIDA_SESSION_ROLE` as a **display/routing hint** (statusline, pickers, queue routing — never authority), it applies that role's **scope filter** to your views, and it injects that role's **system-prompt addendum** into a launched Claude's context. Everything authority-gated (approving, queueing, review verdicts, advisor-gated MCP tools) keys off the *grant*, which is re-validated on every gated call. The documented exceptions:
- a live orchestrated drain's own queueing and headless phases carry advisor authority by corroboration with the running drain rather than by grant;
- holds (the emergency brake) stay reachable by a human at an interactive terminal even without a grant;
- `aida edit --force` on a protected spec skips the protected-spec check, and `aida queue integrate --force` and `aida integrate --force` skip the team-role check;
- clearing an advisor's escalation to a human requires an interactive terminal, and no grant can stand in for it.

`$AIDA_SESSION_ROLE` is a label the grant happens to match. See [Entering seats and delegating child seats](#entering-seats-and-delegating-child-seats) below for the grant model. Because entering/ending a role must mutate the *calling* shell's environment, those subcommands emit shell code — run them through `eval "$(aida role enter advisor)"` on a raw binary, or just `aida role enter advisor` once the `aida dev shell-init` helper is installed (it auto-evals).

**Reach for it when** — at the start of any working shell: declare your seat so gating and views are correct. `aida role enter` to resume an existing persona, `aida role add <name>` to define a new one. `aida role list` to see what's defined; `aida role active` / `current` are the pure, store-free reads scripts and statuslines use.

**Don't reach for it when** — you expect an environment variable to stand in for it. Prefixing a command with `AIDA_SESSION_ROLE=<role>` changes only the label: gated commands resolve the validated grant, find none for that seat, and refuse (or fall back to least-privilege `implementer`). For a one-off advisor-gated command from a shell wearing another hat, enter the seat at your terminal — `eval "$(aida role enter advisor)"`, run the command, `eval "$(aida role end)"` — the grant is cheap and the `end` revokes it. A headless or AIDA-managed agent context cannot mint a grant at all; it works under whatever its launcher gave it, which differs by launcher (see [How each launcher sets the agent's grant](#how-each-launcher-sets-the-agents-grant) below).

**Key options (rationale only).**
- `enter` / `end` — the env-mutating pair; both *output shell code* and must be `eval`'d (or run bare under the shell helper). This is why they feel different from every other subcommand — they can't change your shell from a child process, so they hand you the code to do it. `enter` mints a direct grant (interactive TTY, seat in your roster), and so does `add` when it can; [the launcher table](#how-each-launcher-sets-the-agents-grant) lists both. `end` revokes the grant.
- `enter --delegate-seat <seat>` (repeatable) — declare which child seats *this* session may hand to agents it launches. Omitted means **none**: a session cannot delegate by default, so before a launcher can issue an agent a `reviewer` child grant, your session must have been entered with `--delegate-seat reviewer`. Entering `advisor` at a TTY with no `--delegate-seat` asks once whether to include `advisor` in the delegation set (the advisor's own headless sub-launches need it; the default answer is no).
- `enter --bind-pid <pid>` — mint the grant for *another* running session instead of this terminal: name that session's agent harness (claude, codex, agy) or shell by pid. It must be a live process of yours, and not pid 1 or anything above your own terminal session (a session manager, terminal emulator or multiplexer would bind far too much). Your shell's role is left unchanged; the output names the grant id that session exports as `AIDA_SESSION_GRANT`. A grant id copied into a session from `~/.aida/session-grants/` or a log is refused, so hand-carrying a grant id into a background session no longer works. `--bind-pid` binds the process only: that session's hooks and MCP server can present the grant, but its sandboxed tool calls cannot (they run in their own process namespace). A background or relay session that needs its seat in tool calls must be started from the role shell. <!-- trace:BUG-1934 | ai:claude -->
- `status` (`--json`) — this session's grant: seat, expiry, the processes it is bound to, whether and how this process may present it (`via`: `key`, `proc` or `none`, plus the refusal reason when not), and this session's agent harness pid for `--bind-pid`. A command run from an agent's tool shell never offers that transient shell's pid; when no stable harness pid can be seen (a sandboxed tool call), it says so. Read-only. <!-- trace:BUG-1934 | ai:claude --> <!-- trace:TASK-2159 | ai:claude -->
- `add` (with `--global`) — define a new role; `--global` stores it under `~/.aida/roles/` for personas you carry across projects (triage, code-review) rather than per-project.
- `scope` (`set` / `show` / `clear`) — attach a standing filter to a role so `aida list` and `aida queue list/next` auto-narrow while you wear that hat (the canonical example: a `triage` role scoped to `--tags inbox --status draft`). Override per-command with explicit `--tags`/`--status` or `--no-scope`. This is *why* a role is more than a label — it reshapes what you see.
- `prompt` (`set` / `show` / `clear`) — the per-role system-prompt addendum injected into Claude at session start via the role-context hook. The seam where a role carries *instructions*, not just identity.
- `scaffold` — install the starter `implementer` / `advisor` / `reviewer` set globally; idempotent, safe to re-run. The one-time "give me the standard taxonomy."
- `active` vs `current` — both print the active role, but with different exit-code contracts (`active` exits 1 with empty stdout when none; `current` exits 0 either way, `--check` flips it to 1). Pick by whether your script wants "fail if no role" or "tell me, never fail."
- `expiry` — how close this session's seat grant is to its 24h expiry. By default it prints one warning line when the grant expires within `--warn-within` (2h), naming the expiry time and the `eval "$(aida role enter <seat>)"` that renews it, and prints nothing otherwise. The SessionStart hook shows that line, and the statusline tags a held seat `(expires in 42m)` inside the same window. With `--refuse-within 1h` it instead prints a refusal on stderr and exits 3 when less than that is left: a relay runs it before each launch so a session never loses its seat halfway through. With `--refuse-within`, every exported grant is validated through the authority gate, including subject, roster, parent and handle checks. A rejected grant prints its rejection reason and renewal command on stderr and exits 3. Every valid exported grant, including an implementer grant, supplies expiry metadata, bounded by its parent's expiry. With no grant it is silent and exits 0. Warning-only callers and the statusline preserve informational expired-record timestamps, stay quiet for unreadable or revoked records, and suppress baseline implementer metadata. <!-- trace:TASK-1736 | ai:claude --> <!-- trace:BUG-2011 | ai:codex -->
- `repair` — quarantine unparseable activity-log lines and rewrite a corrupted role file cleanly. The recovery hatch for a role whose log got mangled.

**Gotchas.** `advisor` is the canonical role name everywhere (config, `$AIDA_SESSION_ROLE`, queue routing, statusline); `dialog` is a deprecated, silently-accepted alias normalized to `advisor` — so an un-migrated machine keeps working, but write `advisor` in anything new. Setting `AIDA_SESSION_ROLE` by hand never confers authority — if a gated command refuses while your prompt says `(role:advisor)`, the grant is missing, expired, revoked, or was minted in another shell; re-enter the seat at your terminal. And remember the whole reason `enter`/`end` emit shell code: forgetting the `eval` (on a raw binary) means the role *looks* like it switched but your shell never received the grant.

**Chains with** — enter your seat first thing in a shell; the queue ([Chapter 3](03-work-autonomy.md)) and approval/queue gating validate its grant, and launchers such as `aida agent new` and `aida queue work` issue a launched agent a *child* grant for the seat it runs in, which works only if your session was entered with that seat in `--delegate-seat`. What each launcher does with and without a requested seat is in [How each launcher sets the agent's grant](#how-each-launcher-sets-the-agents-grant).

---

### Entering seats and delegating child seats

**One line** — authority is a grant minted at an interactive terminal and scoped to one session; launchers that issue child grants hand it down one explicitly named child seat at a time.

**Mental model.** Use `eval "$(aida role enter <seat> [--delegate-seat <child-seat> ...] [--ttl <dur>])"` at an interactive human TTY to establish a session seat. The requested seat must be in your **team roster**; add or remove roster seats with `aida team allow-seat <user> <seat>` and `aida team disallow-seat <user> <seat>`, which themselves require an interactive TTY (a terminal check, not proof that a human is typing). The roster is a *ceiling* (which seats you may ever take), the grant is the *active seat* (which one this session holds). Child delegation is **empty by default**: list each child seat this session may hand out with a repeated `--delegate-seat`. A **child grant** is a distinct grant for one seat: one per launched session, bound to the parent and limited to the parent's delegation set. Children cannot delegate onward. Which launchers issue one, and when, is in the table below. `AIDA_SESSION_ROLE` is only a display/routing hint and cannot grant authority. An **MCP server captures the grant it starts with** and re-validates it on every gated call (so revoking the grant takes effect immediately), but it cannot acquire a *different* one while running: to change a server's authority, enter the seat at a TTY and restart/reconnect the server. Grant lifetime defaults to 24 hours. The operator can pass `--ttl <dur>` (e.g. `72h` or `90m`) to set a custom duration, capped by `[seat] max_grant_hours` (default 24h, hard ceiling 168h; bare numbers without a unit are refused). Effective cap is `min(global, project)` when both are configured; project configuration alone can only lower the default 24h ceiling. All grants are subject to an unconfigurable hard ceiling of 168 hours. A cap below 24 does not shorten the default 24h grant when `--ttl` is omitted. Managed agent sessions cannot mint direct grants. <!-- trace:TASK-1787 | ai:antigravity -->

**Grants are bound to a process tree.** A grant id is not a bearer token. `aida role enter` binds the grant to the shell that ran it and that shell's terminal session: any process in this terminal session can use this grant (an agent you start there, its tool calls and hooks), and no process outside it can, even with the id exported. A child grant is bound to the launcher that minted it, so a worker it execs or spawns holds it. `aida role enter <seat> --bind-pid <pid>` binds a grant to a named running session's process, which serves only that session's unsandboxed hooks and MCP server. Matching uses each process's kernel start identity, so a recycled pid never matches. Every refusal (`aida doctor --category seat-grant`, `aida role status`) names the fix. A grant minted before binding existed is refused outright: after upgrading, re-enter each seat with `eval "$(aida role enter <seat>)"`, and start background and relay seats from that role shell or an aida launcher (not with `--bind-pid`, whose grant their sandboxed tool calls cannot present). A review approval records which bound process presented its grant, and `aida pr ship` re-checks that against the grant. <!-- trace:BUG-1934 | ai:claude -->

**Sandboxed tool calls hold the grant's session key.** An agent harness's sandbox (Claude Code's bubblewrap sandbox, for one) runs each tool call in its own process namespace, where the bound shell is invisible. So `aida role enter` also puts a one-time secret into the entering shell's Linux session keyring, as a key only that shell and its descendants can read. The grant record holds only the secret's digest. Session keyrings are inherited into sandboxes, so a tool call of an agent started from that shell (foreground, or a background/relay agent launched there with `aida agent new` without `--bg` or `nohup`) presents the grant by holding the key. A session the agent daemon (`--bg`) or a tmux server starts does not descend from the role shell. It holds no key and is refused with a sentence naming the fix. A launcher puts each child grant's key into its own keyring before starting the worker. `aida role end`, re-entering, or revoking a grant invalidates its key, and the key also expires with the grant. Where the kernel refuses keyrings (some containers), a grant falls back to its process binding only, and a sandboxed caller is refused with that reason, never allowed. The secret never appears in argv, the environment, a file or shell history: the eval text turns xtrace off and passes it to `aida role adopt-key` in a quoted heredoc. <!-- trace:BUG-1934 | ai:claude -->

#### How each launcher sets the agent's grant

`aida role enter` and `aida role add` set the grant of the shell you run them in. The other rows set the grant of the agent they launch.

| Launcher | With a seat requested | With no seat requested |
|---|---|---|
| `aida role enter` | Mints a direct grant for this shell (interactive TTY, seat in your roster) with the `--delegate-seat` set you list, and revokes the shell's previous grant. The grant lifetime defaults to 24h; pass `--ttl <dur>` (e.g. `72h` or `90m`) to set a custom duration, capped by `[seat] max_grant_hours`. | At a terminal, opens a role picker and mints a direct grant for the chosen seat. Without a terminal it refuses. |
| `aida role add` | Mints a direct grant for the new role (interactive TTY, seat in your roster) with no delegation set, and revokes the previous grant. If it cannot mint one, it creates the role as a label only and unsets the shell's grant variable. | Not applicable: the role name is required. |
| `aida session new` | Issues no grant: `--role` only labels the session. The agent inherits the parent's grant from the environment. | Issues no grant. The agent inherits the parent's grant from the environment. |
| `aida session resume` | Not applicable: there is no seat option. | Issues no grant. The resumed agent inherits the parent's grant from the environment. |
| `aida agent new` | Removes the parent's grant from the agent's environment and issues a child grant for the `--role` seat. With `--bg`, the session runs under the agent's background daemon, outside the launcher's process tree, so no grant can be bound to it: an implementer seat runs ungranted (it needs none) and any other seat is refused before launch. | No grant: the parent's grant is removed from the environment. With `--spec`, the session records a default seat (implementer, or reviewer for a PR) and the agent gets a child grant for it. |
| `aida queue work` (drain phases also launch through it) | Issues a child grant for the seat. Two paths issue none and leave the parent's grant in the environment: a fresh `--no-human` launch that is not a PR review, and a resumed interactive PR review. | There is no seatless launch: it picks a seat from the queued items' routing, else implementer (reviewer for a PR), then behaves as in the previous column. |

**Reach for it when**
- Starting an advisor shell that will launch reviewers and implementers: `eval "$(aida role enter advisor --delegate-seat implementer --delegate-seat reviewer)"`. Without those flags, `aida agent new --role` and `aida queue work` refuse the launch and name the `--delegate-seat` to add.
- An agent reports "needs advisor authority" from a headless context: do not reach for an env prefix — the parent session either lacks the seat or didn't delegate it. Re-enter at your terminal with the seat in the delegation set and relaunch.
- Advisor-gated MCP tools refuse although you entered advisor: the server was started before the grant existed. Restart the server and reconnect the client.

**Don't reach for it when** — you want to grant yourself a seat your roster doesn't allow; that is a roster change, made at an interactive TTY with `aida team allow-seat`, not something a session can mint. And don't try to mint a direct grant from inside an AIDA-managed agent session (one launched with an agent name/type): those sessions hold whatever their launcher gave them (see the table above).

**Gotchas.** Grants expire after at most 24 hours; a child grant shares its parent's expiry and becomes invalid when the parent is revoked (`aida role end`) or the roster no longer allows the seat. Entering a seat revokes the grant the shell held before, so one shell holds one seat at a time. The model separates *sessions*, not OS users: it does not claim protection against another process running as the same user that copies a grant handle — that residual same-user limit is recorded in the accepted decision on TTY-established, session-bound seats (see the [decisions index](../aida/05-decisions/README.md)) and is deliberately out of scope here. <!-- trace:ADR-66 -->

**Chains with** — `aida team` (roster ceiling, TTY-only writes), `aida agent new` / `aida queue work` (child issuance; see the launcher table), the MCP server authority line in `aida status` ([Chapter 8](08-reporting.md)), and the role-gated verbs in [Chapter 3](03-work-autonomy.md). <!-- trace:DOC-6 | ai:claude -->

---

### `aida session`

**One line** — work sessions and the worktree *leases* that stop two agents colliding on one spec.

**Mental model.** This command spans two genuinely different concepts that share a name — internalize the split or it confuses. (1) **Scoped sessions + leases** (`start` / `end` / `leases` / `show` / `manifest`): a *lease* is a claim on a logical or physical scope (an EPIC, a SPEC-ID, a path glob), backed by a sibling git worktree on a fresh branch. `aida session leases` is the canonical "who holds what scoped work right now" view. (2) **Historical Claude Code conversations** (`conversations` / `resume` / `new` / `prune`): a wrapper over `claude --resume`, enriched with the role + spec each.jsonl session was about. The first concept is *work coordination*; the second is *conversation management*. When you want "what's running?", reach for `leases`, **not** `conversations`. (`conversations` was once called `session list`; the old name is kept as a deprecated alias, so muscle-memory `session list` still works.)

**Reach for it when**
- `aida session start --owns <scope>` — beginning isolated work on a spec/epic/path: it creates the worktree, symlinks AIDA state, and writes the lease in one step. `--launch` collapses the usual "start → cd → session new" into a single command.
- `aida session leases` — the standing "is anyone already on this?" check before you (or an agent) claim a spec; the multi-agent drain's collision guard.
- `aida session end` — finishing isolated work: removes the worktree and lease, *leaves the branch alone* (merge/discard is your call), and as a safety net files a reviewer item if the branch has an open PR. It refuses to remove a worktree that another live process is still using (its working directory, an open or memory-mapped file, or a program running from inside it, such as a cargo build), names each pid and command, and changes nothing until you stop them or pass `--include-in-use` (`--force` does not override it). It checks again after you confirm and once more just before the worktree is removed; if a process started in the meantime, the worktree is kept. The same check applies when the worktree would be reset and returned to the warm pool (the default), since that rewrites its files. Your own shell, its parents and background jobs, and the helpers of the agent you are ending from (its MCP server, for example) never count, so you can end your session from inside its worktree; a process in another terminal or tmux pane still counts, even one started in that worktree. `aida session reap`, `aida session gc`, `aida worktree dismiss`, doctor's lease and worktree heals, automatic stale-lease release, and the warm pool's reclaim of an expired reservation keep a tree that is in use, so a scripted bulk sweep can't delete live work. So do `aida worktree pool destroy` and `aida worktree pool return`, unless you pass `--include-in-use`; `pool return` also keeps a dirty tree as it is, without resetting it, when it cannot save its uncommitted changes first. They also keep it when the process list can't be read in full (for example inside a pid namespace). In that case `session end` only warns, and only when no process in the tree is visible: inside a container, a process the check can see still makes `session end` refuse. In a container that owns its worktrees, set `AIDA_TRUST_PID_NAMESPACE=1` to trust the check there.
- `aida session conversations` / `resume` — finding and re-entering a *past conversation* by its role+spec context instead of grepping Claude's auto-generated subjects. (`conversations` is the renamed `session list`; the old verb is a deprecated alias.) Each row also names the **process behind it** when one can be found — a PID and TTY column (`pts/3`) so a forgotten tab is findable — and the liveness glyph is three-state: `●` means a process was resolved and is alive (the AGE cell is then the process's elapsed time), `◐` means the transcript was written in the last five minutes but no process could be resolved, blank means neither. Resolution is best-effort and never invented: a session lease's recorded pid first, else a live claude/codex process whose cwd matches the transcript and whose start time falls inside the transcript's launch/resume window; two candidates render `?` and `--format json` lists both (`pids`), alongside `liveness` (`alive|file-touched|none`), `resolution` (`lease|proc-scan|none`) and the covering lease's `lease_role` when it differs from the transcript's hook role — the same role rule `aida ps` uses.
- `aida session reap` — clearing up after sessions that already finished: a headless agent exits the moment its work lands, and because a session can't tear down its own worktree (it *is* the live cwd), every spec boundary otherwise needs a human to end the session, remove the worktree, and delete the branch. This pass does all three in one go for the sessions it can prove are done. It never reaps a lease held on the main checkout (or the store tree, or a protected branch such as `main`), but it can **release** one: it deletes only the lease file and never touches a file of the checkout. It does this when the agent process the lease recorded is provably gone, meaning no process has that pid or the pid now belongs to a different process (its start time differs). If the start time can't be read, the process does not count as gone. A lease that records no agent process, or only the short-lived shell that ran `aida session start`, has an **unknown owner**: it is neither live nor dead. Reap lists it and keeps it unless you pass `--release-unknown-owner`. A lease whose recorded process is still running is always kept, as is a lease taken on another host. Releasing follows the same `--dry-run` / `--yes` / confirmation rules as reaping, and the reap that runs after `aida pr ship` merges never releases a lease. `aida session leases` marks such leases "unknown owner" (`owner_unknown` in `--json`), and `aida session handoff --check` doesn't count them as a reason to compact. A lease with an unknown owner is never credited to the process that is looking at it. <!-- trace:BUG-2197 | ai:claude -->

**Don't reach for it when** — you want the "who's working" view but typed `conversations` (that's historical conversations — use `leases`). And don't `start` a scoped worktree for trivial in-place work; the worktree-per-scope model pays off for *parallel* or *long-running* work, not a two-line fix on your current branch.

**Key options (rationale only).**
- `start --owns <scope>` (alias `--spec`) — the scope is the lease's identity: an EPIC, any SPEC-ID, a `path/**` glob, or a free-form `feature:auth` tag. Path globs are *stored, not validated* — they're advisory coordination hints.
- `start --reuse-branch` — check out an existing branch instead of forking fresh: the fixup-on-an-existing-PR-branch flow. Without it, an explicitly-named existing `--branch` is reused automatically and an auto-derived name always forks.
- `start --launch` (+ `--title` / `--name` / `--role` / `--permission-mode` / `--sandbox`) — create the worktree *and* exec Claude inside it. The role is **derived from the scope** by default (`PR-N`/`MR-N` → reviewer, everything else → implementer); `--role` overrides, but when it disagrees with the scope the scope wins with a warning — because the scope is the more reliable signal of what the work *is*.
- `start --force-claim` — claim a spec in an *ambiguous* state (In-Progress with no local lease, or NeedsAttention awaiting triage). It deliberately does **not** override Done/Completed/Rejected/Draft — those always refuse. The escape hatch for "yes, I'm taking over this stuck claim."
- `end --force` — discard uncommitted changes and skip the confirmation. The refusal-without-it is the guard against silently nuking dirty work. It does not override the live-process check, so a script that passes `--force` for a dirty tree or a missing terminal still keeps a worktree in use.
- `end --include-in-use` — the one override for the live-process check: remove (or return) the worktree even while processes use it, and terminate live `claude` processes inside it. Without it, `session end` refuses, which guards against killing a running build or agent and against an orphaned claude with a deleted working directory.
- `end --wait-ci` vs `--watch-ci` — both block until the PR's CI is terminal before releasing the lease; `--wait-ci` is the *silent* poll (overnight builds), `--watch-ci` is the *live* stream (interactive). Same decision tree once CI resolves; pick by how much screen noise you want.
- `new --permission-mode` — the faithful-launcher knob: omit it and Claude uses its native posture (prompts); pass `bypassPermissions`/`acceptEdits`/`auto`/etc. to opt in. Fleet-wide bypass is `[agents] bypass = true` in `agents.toml`, not a baked-in default.
- Supervised launches that resolve to bypass ask for confirmation by default. The setting is `[agents] confirm_bypass = true` in `.aida/config.toml`; only `~/.aida/config.toml` may set it to `false`. Project config can require confirmation but cannot disable it. A non-TTY configured bypass is downgraded to native; an explicit bypass request and detached agent launch require a terminal confirmation. This guard does not stop edits by an agent already running in bypass, a fake TTY, or direct edits to the user config with an editor that already has write authority.
- `[agents] vendor = "codex"` (agents.toml/config.toml; precedence: project `.aida/agents.toml` per-clone > project `.aida/config.toml` TRACKED & worktree-inherited > user-global `~/.aida/agents.toml`) — the set-once default-vendor knob for codex-first/codex-mandated machines: every launch surface (interactive `queue work` host, orchestrator headless phases, TUI tabs, and queue-work launch chains) falls back to it when no per-surface flag/env/config picks a vendor. Per-surface knobs keep priority.
- `reap --release-unknown-owner` — also release main-checkout leases with an unknown owner. It is opt-in because the lease may belong to a live session that never recorded its process; any recorded process that is still running keeps the lease even with this flag. <!-- trace:BUG-2197 | ai:claude -->
- `leases --verbose` / `--all` — `-v` probes live claude PIDs (and flags `(deleted)`-cwd zombies); every view has a state column. The default view includes every spec-scoped lease that can block dispatch, even when stale, and hides only stale harness leases; `--all` adds those stale harness leases back. <!-- trace:BUG-2002 | ai:codex --> <!-- trace:TASK-1367 | ai:claude -->
- `manifest` — record which specs a session *intends* to work, so other commands can surface "planned by another session" cues. Coordination *before* the lease, for a planned cluster.
- `prune` — bulk disk-cleanup of old session.jsonl files by age (and skips dirs with active leases, defensively). It only touches conversation metadata; worktree cleanup is a separate concern — `gc` removes stale agent isolation worktrees, `reap` tears down finished scoped sessions (see below).
- `reap --dry-run` / `--yes` / `--json` — the reap predicate is deliberately narrow: spec Done or Completed, **and** branch merged, **and** process exited. All three are read from state (the spec's status, git ancestry or a merged PR, the process table) — never from what a terminal printed, because inferring "the agent looks finished" from scrollback is exactly the class of unreliable signal that produces confidently wrong cleanups. A session whose process is still *alive* fails the predicate and is left completely alone: its worktree is its working directory, and closing a running agent is your call, not the tool's. It gets reaped on a later pass, once it exits on its own.
- **The same pass notifies a live-but-finished session.** An interactive session sits idle at its prompt after its spec merges — alive, so never reaped. Rather than leave it guessing, the pass sends it a one-time mailbox FYI (surface-only, no action): *"spec merged, you are free to exit; the worktree reaps once your process exits."* It arrives on the session's next turn via the per-turn `aida awaiting` notice, is deduped per session, and — like the reap — never scrapes the terminal and never closes the agent. `--json` reports these under a `notifiable` array; `--dry-run` previews them.

**Gotchas.** The `leases` ≠ `conversations` split is the one that bites: `aida session conversations` (formerly `session list`) will *not* show the scoped leases that `aida session start` created — they're different substrates. `aida session show` defaults to the lease covering your cwd (or matched by ancestor PID), so running it from inside a worktree "just works." And `end` leaves the branch untouched by design — ending a session is *not* merging or discarding the work. Note the split between `end` and `reap` on branches too: `end` leaves the branch for you to merge or discard, while `reap` only ever runs *after* the branch has already merged, so deleting it there loses nothing.

A direct `aida session start --role reviewer` keeps ordinary source-branch
conflict handling: a live implementer lease refuses headless callers and retains
the conflict prompt at a terminal. Automatic coexistence requires an explicit
reviewer pickup through `aida queue work PR-N --role reviewer` (or `MR-N`),
with the backing spec verified against the change's title trailer or branch.
An inherited reviewer route, an unrelated anchor, or unavailable change metadata
does not authorize coexistence.
<!-- trace:TASK-1801 | ai:codex -->

**Chains with** — `aida queue work` ([Chapter 3](03-work-autonomy.md)) leases under the hood; `aida session start --launch` is the manual equivalent. After review + merge, `aida session end` tears the worktree down. Cross-reference `aida agent new` for the multi-process launcher.

---

### `aida agent`

**One line** — launch and track real agent processes (Claude / Codex / Antigravity) on a spec.

**Mental model.** `aida agent new <tool>` spawns a *one-shot* agent with project-correct cwd/env, a role, registry tracking, and a startup **context snapshot** — the supervised way to put an agent on work instead of opening a terminal and pasting a prompt. The critical framing in its own help: this lane *does its work, ships a PR, and exits* — it is **not** the orchestrated pipeline. It does not run CI, the reviewer phase, or the merge. For a supervised end-to-end drain (implementer → CI → reviewer → merge → pull), that's `aida queue work <SPEC> --auto-complete` ([Chapter 3](03-work-autonomy.md)), not this.

**Reach for it when** — you want to *dispatch* a bounded agent run: `aida agent new claude --role implementer --spec <spec-id>` spins the worktree+lease and launches Claude already pointed at the work. `aida agent list` (aliases `ls` and `status`) to see what's running; `pause` / `resume` to mark an agent budget-exhausted/rate-limited so brief-time dispatch and `aida status` flag it (without stopping the process).

**Don't reach for it when** — you want the full lifecycle driven for you (CI, review, merge) — that's `aida queue work --auto-complete`, not `agent new`. And `pause` does **not** stop a process — it's a *status marker* for dispatch logic; use `aida agent stop` to actually terminate one.

**Cwd and failed setup.** An explicit `--cwd` is used for the child and the
preview. With `--spec`, an inactive lease for that exact directory and role is
re-entered; `--force` does not authorize taking over live work. A fresh explicit
path bypasses pooling. Worktree errors name the requested path, underlying
cause and recovery commands. A refused launch rolls back this attempt's
allocations; existing leases and directories are preserved, and cleanup refuses
a tree with live users. A failed spawn revokes only its newly issued grant.

For AIDA delegation, keep the advisor shell's validated parent grant and clear
`AIDA_SESSION_PURPOSE`; the launcher issues the child's own grant. If the grant
was scrubbed, enter the advisor role in a human terminal by evaluating the
output of `aida role enter advisor --delegate-seat implementer` (wrap it in
`eval "$(...)"`), then retry the original launch in that shell. Direct vendor/relay launches still scrub the
advisor grant. An ambient `AIDA_SESSION_ROLE=implementer` alone does not authorize
a delegated launch.
<!-- trace:BUG-2188 BUG-2168 | ai:codex -->

**Key options (rationale only).**
- `new <claude|codex|antigravity>` — the tool is a subcommand, not a flag, because each agent has a slightly different spawn shape. `list-roles` enumerates the supported role taxonomy.
- `new claude --spec <ID>` — creates a scoped worktree + lease *before* launch, so the agent starts already isolated on its spec. An ownership conflict is checked before bypass/resume prompts. To re-enter an inactive same-role lease, pass `--cwd` with its exact owning worktree; the existing session identity is retained. Live agents, branch/role mismatches, and reasoned holds refuse re-entry. Inspect with `aida session leases --all`; do not end the session just to redispatch it. <!-- trace:BUG-2002 | ai:codex -->
- For non-interactive background rework, first set `[agents] confirm_bypass = false` in `~/.aida/config.toml` (trusted project config can still require confirmation). Then run: `aida agent new claude --bg --role implementer --spec <ID> --cwd <WORKTREE> --prompt-file <FILE> --no-resume --permission-mode bypassPermissions`. <!-- trace:BUG-2002 | ai:codex -->
- `new claude --show-context` / `--no-context` — print, or suppress, the launch-context snapshot. The snapshot is a *startup* picture only; a launched agent must keep polling briefs/MCP for work filed after launch.
- `new claude --no-exec` (aliases `--noexec`, `--print-command`) — print the **complete resolved launch contract** (argv, active role, prompt source, the launch-context snapshot path it *would* write, the `AIDA_*` environment the child would see, and which repository guidance files — `CLAUDE.md`/`AGENTS.md` — are present) and exit. A pure dry preview: no process, worktree, lease, status transition, or network sync. Pairs with `--show-context` (the full snapshot *body*, still a preview) and the narrower `--show-prompt` (just the initial message). <!-- trace:TASK-1467 | ai:claude -->
- `new claude --show-prompt` — print only the exact initial prompt that would be sent, labeled `explicit (--prompt/--prompt-file)`, `generated (role launch prompt)` or `none (--no-prompt)`, and exit; same no-side-effects contract as `--no-exec`. <!-- trace:TASK-1467 | ai:claude -->
- `new claude --verbose` — for a launch that actually executes (unlike `--no-exec`, which previews and exits), print launch-time diagnostics before/around spawn: the resolved contract shared with `--no-exec` (agent/client, role, spec/session/name, working directory, prompt source, whether context was injected, guidance files discovered, generated child argv), plus process registration and child exit/result information once the process is running. The initial prompt text and any secret-shaped argv value are always redacted — never printed, with or without `--verbose`. Diagnostics go to stderr; normal (non-verbose) output is unchanged. <!-- trace:TASK-1498 | ai:claude -->
- `new` initial prompt — every launch gets a short **role-aware** first message: with `--spec`, that role's contract for the spec (implementer builds, reviewer evaluates and records a verdict, advisor judges and gates, product grooms and orders, integrator lands the PR mechanically); without `--spec`, an orientation naming the role, its boundary and where to find work. A role file can replace the embedded body with `launch_prompt` / `launch_prompt_spec` (placeholders `{role}`, `{agent}`, `{spec}`). `--prompt` still wins; `--no-prompt` suppresses only this automatic message — `AIDA_SESSION_ROLE` and the launch-context snapshot are unaffected. <!-- trace:STORY-1471 | ai:claude -->
- `new <vendor> --prompt-file <PATH>` — read the initial message from a file instead of the command line, mutually exclusive with `--prompt` and `--no-prompt`. This is the flag an orchestrator should use: a brief passed as `--prompt "$(cat brief.txt)"` through a nested `bash -lc` can lose its quoting, and the launcher would then spawn an interactive agent that sits idle at an empty prompt while the caller blocks on it. An empty or whitespace-only prompt — from either source — now refuses the launch rather than producing that silent idle seat. <!-- trace:BUG-1696 | ai:claude -->
- `new codex --no-mail-wake` — do not wake this session's thread when its own inbox has mail. By default `aida agent new codex` arms the [mail wake](../agents/codex-mail-wake.md) once the session's SessionStart hook records its thread. `[agents.codex] mail_wake = false` in `agents.toml` makes off the default, and Codex older than 0.164.0 degrades to a warning. `aida agent list` shows the state in its `WAKE` column. <!-- trace:STORY-HOME-096 | ai:claude -->
- `new <vendor> --role implementer` (no `--spec`) — launches another member of the **implementer pool**: any number of generic implementers, of any vendor, may run per repo, each with its own handle and mail inbox. Exclusivity applies only where it matters — the spec claim/lease when a member picks up work, so two members never hold the same spec. Mail addressed to the role (`--to implementer`) is taken by exactly one member; mail to a handle reaches only that agent. `aida agent list` lists the pool (handle, role, spec — `(none)` while idle). Advisor, product, integrator and reviewer stay one per scope; their refusal is checked before the bypass and resume prompts and names the fix (`--spec <ID>` for a spec-scoped role). At a terminal, a refused spec-scoped launch (a reviewer, or an implementer whose `--spec` is already held) offers the approved, unclaimed specs queued for its role and launches on the one you pick; headless or piped launches just refuse. `--resume <id|latest>` reopens the ended agent in its own scope and skips this check. <!-- trace:TASK-b6b1278890774ef993d2ebc327862470-121 | ai:claude -->
- `new <vendor> --no-duplicate-check` — skip the same-vendor/same-role singleton check entirely, including on the new-launch path. The escape hatch when the live holder of a role is itself wedged and you need a second seat anyway. `--allow-duplicate` is the narrower flag: it drops the confirmation prompt but still lets the check refuse. <!-- trace:BUG-1697 | ai:claude -->
- `[agents] mcp` in `agents.toml` — which AIDA surface a supervised seat gets: `off` (the default — no MCP servers, the `aida` CLI is the surface), `aida` (attach only AIDA's own `aida mcp-serve` definition), or `native` (inject nothing; the vendor's own MCP configuration applies). The default is `off` because a controlled 72-cell measurement put MCP at ~1.8–2x the CLI's token cost for identical or worse task success, and traced the overhead to the protocol's per-call structure rather than a one-off upfront-schema tax. `off` and `aida` both suppress Claude Code's project-MCP trust prompt, since neither consults the repository's `.mcp.json` — so a checkout cannot smuggle a command in under the name `aida`. Same user-base-then-project precedence as `bypass`/`contained`; an unrecognised value fails the launch rather than falling back. Only Claude Code exposes launch-time MCP flags, so for codex and antigravity MCP stays configured in their own files and this knob cannot enforce a surface at launch. <!-- trace:TASK-1558 | ai:claude -->
- `new claude --bg` — detach to Claude Code's background supervisor (`claude --bg`); the session shows up in `claude agents` and `aida status`, and with `--spec` the captured sessionId is recorded on the lease so the cross-substrate view links them.
- `new claude --rotate` (or `[agents.claude] rotate = true` in `agents.toml`) — supervise the session across **context rotations**. When the session exits after writing a fresh seat handoff (`aida session handoff --seat <seat> --write -`), the launcher relaunches Claude in the same worktree under the **same handle, lease and mail recipient**, and the relaunched session's SessionStart loads the handoff. A handoff counts only when it carries this session's own handle and seeded session id, so a sibling seat's or an older session's handoff never triggers a relaunch. A 5 s `relaunching <handle> #N in 5s, Ctrl-C to stop` countdown runs first. The run stops for real on an exit with no fresh handoff, on `aida session handoff --write - --final`, on an exit within 60 s with no handoff (reported as a crash), or after 3 relaunches in 10 minutes. A relay run holds the handle in the gap between generations, so mail sent then reaches the next generation. The launcher records itself as the lease's live process for the whole run. `aida agent ls` shows a rotated session as `<handle> #N` with its last rotation time, and a non-advisor seat's advisor hello names its generation and the session it replaced. Claude only; codex, `--bg` and headless lanes do not rotate. <!-- trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude -->
- `new claude --permission-mode` / `--sandbox` — the same faithful-launcher posture as `session new`: omit for native (prompts), opt in explicitly, or `--sandbox` for contained mode.
- Presentation env is NOT inherited — `AIDA_OUTPUT_FORMAT`, `AIDA_AGENT_OUTPUT`, `AIDA_GLYPHS`, `AIDA_QUIET`, and `AIDA_PUSH_QUIET` are how *your* shell wants output rendered, not how the spawned agent's shell should render — so every launch (direct, `--bg`, and any `[contained] os_wrap`-wrapped launch, across `claude`/`codex`/`agy`) strips them by default and lets the child decide its own output mode from its own TTY/piping. `[agents] inherit_env = ["AIDA_GLYPHS", …]` in `agents.toml` (same user-base/project precedence as `bypass`/`contained`) is the explicit, named opt-in when you genuinely want one preserved. `new --no-exec` and `--show-context` both print a `presentation_env:` / `Presentation env:` line naming exactly what was stripped and what was kept, so an intentional inheritance is never silent.
- `new` (no `--role`, interactive) — the role picker's cursor starts on your active shell role (`AIDA_SESSION_ROLE`, e.g. from `aida role enter`) when it's in the project's role list, falling back to `implementer` otherwise; a non-interactive launch with no `--role` still always defaults to `implementer`. The optional agent-description prompt (shown for advisor/product launches) is explicitly labeled optional — blank Enter skips it. <!-- trace:TASK-1462 | ai:claude -->
- `list` (aliases `ls` and `status`) — reports each registered seat as `busy` or `idle`. `busy` means this seat has work in flight; `idle` means its own process tree shows none. The evidence is the *age* of the seat's descendant processes, which leaves one case deliberately uncounted: a build the seat launched **detached** and then stopped attending to, whose start predates the seat's last heartbeat. So `idle` means "this seat has nothing in flight", not "nothing is running under this seat". The window is bounded the other way — a descendant that started after the last heartbeat keeps the seat `busy` for as long as it runs, however long that is. <!-- trace:BUG-1726 | ai:claude -->
- `stop` — terminates the **registered seat pids only**, and signals no descendants: there is no process-group kill and no descendant walk on this path, so a `cargo`/`rustc` child the seat started is reparented to init and runs to completion. This is what makes a stale `idle` benign — such a build is *stranded*, never killed — but stranded is not free: an ownerless build still holds one of the host's cargo slots, with no seat left to read its result. After stopping a seat, check for leftover children yourself. <!-- trace:BUG-1726 | ai:claude -->
- `pause` / `resume` — budget/rate-limit signalling for the fleet, distinct from `stop` (which terminates).

**Gotchas.** The startup context snapshot is *not* live — it's a point-in-time brief; an agent that only reads it will miss anything filed after it launched. The "one-shot, ships-a-PR-and-exits" lane is easy to mistake for the orchestrated drain — they are different tools with different guarantees. And `idle` describes the *seat*, not the machine: a detached build the seat started before its last heartbeat keeps running — and survives `aida agent stop` — while the seat reads `idle`.

**Chains with** — `aida brief` files the assignment, `aida agent new` launches the agent to pick it up; `aida session leases` / `aida agent list` show the result running. For the full reviewed pipeline instead, `aida queue work --auto-complete`.

---

### `aida advisor`

**One line** — register the live advisor session, and run its presence-gated maintenance loops.

**Mental model.** The advisor seat is the persistent live Claude session wearing the advisor hat (by convention, one per project today); `aida advisor` is the seam that lets the rest of the system *find* it and run its maintenance loops. `aida advisor register` writes `~/.aida/advisor.toml` recording the current Claude session as the live advisor — and that one fact changes how *every* headless advisor pass boots:

- **Registered** → the pass **forks from your live session** (copy-then-resume its transcript) so it inherits your *full in-flight context* — ~$0.03 warm. This is the good path, and it applies to **both** the `--no-human=both` *in-drain escalation tier* (when an implementer punts mid-drain) **and** the `aida advisor watch` away-loop.
- **Not registered** → the pass **cold-boots** a fresh headless advisor: same model, but only the *persistent* substrate (memory, CLAUDE.md, discipline, spec graph), none of your conversation. Cold-boot is the *fallback*, not the default.

So registration is what turns "a stranger handles the punt" into "your advisor, with everything it knows, handles it" — across the board, in-drain and away alike. Beyond registration, the family carries the advisor's *operational* loops: a situational dashboard, the watch loop, and recurring scheduled tasks.

**Reach for it when**
- `aida advisor register` — at the start of a live advisor session, so headless escalations can fork *you* instead of a cold boot. `unregister` to clear it (idempotent).
- `aida advisor status` — the advisor's read-only dashboard: one screen of counts (intake drafts, pending decisions, findings, backlog, queue depth, live sessions), each row pointing at the command to act on it. The "what needs my attention?" entry point for the advisor seat.
- `aida advisor watch` — while you're `away`, periodically fork the live session and run a bounded garden + mailbox-triage + escalate pass headless. The "keep gardening while I'm gone, but only safe work" loop.
- `aida advisor schedule` — register recurring maintenance/research tasks that land in the queue on a cadence (no daemon — they fire on `aida pull`).

**Don't reach for it when** — you want a *fresh* headless advisor regardless of context: just don't register (the orchestrator cold-boots when no live advisor is on file). And `watch` is opt-in by invocation and presence-gated — don't run it expecting it to act aggressively; `--triage-only` makes the fork *surface* mailbox items without acting on them.

**Key options (rationale only).**
- `register --uuid` — defaults to `$CLAUDE_CODE_SESSION_ID` (Claude sets it every session), so the bare `register` is usually right; pass `--uuid` only to register a *specific* session id.
- `status --json` / `--registration` — `--json` for machine consumers; `--registration` narrows to just the live-advisor block (back-compat for muscle memory and scripts that predate the full dashboard).
- `watch --dry-run` / `--once` / `--triage-only` — `--dry-run` previews each tick's decision *and the estimated fork cost* without forking (a warm fork is ~$0.03 but accrues over a long away window, so the preview matters); `--once` is the cron-friendly single tick; `--triage-only` is the conservative "surface, don't act" mode. `--fork-interval` / `--poll-interval` tune cost vs responsiveness.
- `schedule add` / `list` / `enable` / `disable` / `remove` / `run` — the no-daemon scheduler: due schedules fire on every `aida pull`; `run` force-fires now (the same logic `pull` uses).
- `handoff --to <project> --focus <topic>` — generate a checked-in advisor handoff brief for a *sibling* project: a dated Markdown template (parent identity auto-filled, strategic sections as placeholders the operator authors). The cross-project knowledge-transfer surface.

**Gotchas.** Registration is the switch, and it governs *both* headless paths the same way: with a live advisor registered, the `--no-human=both` in-drain escalation tier *and* `aida advisor watch` both **fork** from your context; with none registered, both **cold-boot** a stranger. An unregistered advisor session silently gets the worse path everywhere — so `aida advisor register` at the start of a live advisor session is the cheap, high-leverage habit. The watch forks aren't free (~$0.03 warm each over the away window) — `--dry-run` previews the cost. (The "two verdicts fire" double-run is *calibration mode* only, not normal operation.)

**Chains with** — `aida advisor register` pairs with the headless drain ([Chapter 3](03-work-autonomy.md), `--no-human=both`); `aida advisor watch` is gated by the away/home presence verbs (Chapter 3); `aida advisor status` is the advisor seat's daily entry point alongside `aida status` ([Chapter 8](08-reporting.md)).

---

### `aida brief`

**One line** — route a pickup assignment to a specific agent, without sharing scrollback.

**Mental model.** A brief is a *one-directional* "here's your next thing to work" note, written by the operator (or advisor) to a named agent, landing as a file under `.aida/agent-briefs/<agent>/`. It exists because a fleet of agents can't share one terminal's scrollback — the brief is how an assignment reaches an agent that wasn't in the room when you decided it. The agent reads it (`read`), works it, and `ack`s it so it stops re-appearing. Briefs are *runtime, per-clone* state; the equivalent MCP tools (`list_briefs` / `read_brief` / `ack_brief`) let MCP-speaking agents do the same without the CLI.

**Reach for it when** — assigning bounded work to a specific agent (`aida brief codex <spec-id> --note "..."`); checking what's pending for an agent (`aida brief list --for-agent codex`); an agent reading or acknowledging its queue.

**Don't reach for it when** — you want a *conversation* (two-way, threaded) — that's `aida mailbox`, not a brief. A brief is fire-and-forget assignment; the mailbox is dialogue. And don't rely on a brief to *interrupt* an idle agent unless you mark it `--notify` (otherwise it's FYI-only and waits to be polled).

**Key options (rationale only).**
- `--note` — operator context for the pickup; `-` reads a multi-line note from stdin (for anything past a one-liner).
- `--notify` — mark the brief *urgent*: writes a `.pending` sentinel so an idle agent's `aida status` / statusline surfaces it without a heartbeat. Omit it for FYI briefs that shouldn't interrupt — the flag *is* the interrupt/no-interrupt choice.
- `--depends-on <SPEC-ID>` — gate the brief behind another spec landing first; the ordering hint for chained assignments.
- `--as-deep-link` — print a `claude-cli://open` deep link so a click opens Claude Code in the spec's worktree with the pickup prompt pre-loaded, eliminating the paste step (needs a recent Claude Code for the URL scheme).
- `list --for-agent` / `--include-acked` — narrow to one agent, or include already-acked briefs (for audit; the default hides them).

**Gotchas.** A brief without `--notify` is *passive* — it sits until the agent polls; if you expected an idle agent to wake on it, you wanted `--notify`. Briefs are per-clone runtime state, not synced specs — they live in the working tree, not the orphan store.

**Chains with** — file a brief, then `aida agent new` launches (or an already-running agent polls) to pick it up; the agent `ack`s it on completion. For two-way coordination instead, `aida mailbox`.

---

### `aida mailbox`

**One line** — peer↔peer messaging between agents (send / inbox / thread), durable across clones.

**Mental model.** Where a *brief* is operator→agent assignment, the **mailbox** is agent↔agent *conversation* — threaded messages addressed to a peer (or broadcast to all), with an inbox, threads, retract/delete, and a git-canonical sync. It's the channel a fleet uses to say "I'm taking the auth specs, you take storage" or "heads up, my PR touches your file." Two layers: a fast *local* layer for the working session, and `aida mailbox sync` which digests that local layer into the durable orphan-branch store so messages are replayable and shareable across clones. The operator views (`list` / `inbox --all`) give a fleet-wide read without being a participant.

Contained launches create and grant the shared `.aida/mailbox/` directory before sandbox startup. The rest of the main checkout's `.aida/` state remains outside that grant. A failed send names the message path and the filesystem error. Restart an existing contained session to receive the new binding. <!-- trace:BUG-2218 | ai:codex -->

**Advisor handshake.** An agent started by `aida agent new` with its own handle, in any role but advisor, mails the `advisor` role once, at session start. A child that only inherited a parent's identity, and a headless `aida queue work` child, send no hello. The hello names its handle, role, spec and worktree or clone path. With no live advisor, the hello is queued for the role and the agent is told `no live advisor; hello queued`. An advisor's per-turn hook lists the hellos still waiting. Reading them with `aida mailbox inbox` (or MCP `read_inbox` with `mark_seen`) acks each one with a reply to the sender's handle. If no ack arrives within `[mailbox] handshake_timeout_mins` (10 by default), the agent's per-turn notice and `aida agent ls` show `advisor unreachable: <reason>`. `aida doctor` (category `advisor-handshake`) names the mailbox the hello was written to, for example a seat clone's local mailbox that the primary inbox does not read. `aida mailbox hello` shows the same status on demand. Turn the handshake off with `[mailbox] advisor_handshake = false`, or for one session with `AIDA_ADVISOR_HANDSHAKE=0`. <!-- trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude -->

**Reach for it when** — agents coordinating directly: `aida mailbox send "..." --to codex` (or `--broadcast`), `aida mailbox inbox` to read (which marks it seen), `aida mailbox thread` to see a full conversation. The operator's `aida mailbox list` for "who has mail waiting, who has unread/urgent."

Explicit sends and comment mentions resolve against live named agents, role/vendor readers, and known people. Unknown or dead agent recipients refuse before creating mail. Role/vendor sends report the current live readers and warn when several can read the shared inbox; confirmed zero-reader sends refuse. If PID namespaces differ or liveness cannot be observed, an entry counts as an unverified reader only while its `last_active_at` is within the past 24 hours. Stale entries and entries without that heartbeat do not count. Delivery to fresh unverified readers continues to the durable inbox with a notice separating live and unverified readers. Notices list at most five names, live readers first, then report how many more remain. Sending never deletes or rewrites registry records. Instead, `aida agent gc`, `aida agent list` and each session's start hook mark a retained resumable record ended once its process is observably gone, so a finished session stops counting as an unverified reader. Only the start hook always runs outside the command sandbox. `gc` and `list` run wherever they are invoked, and from a sandboxed shell they skip rows whose processes they cannot observe, so ending host rows needs a host-side `aida agent gc`. <!-- trace:TASK-2238 | ai:claude --> Garbage collection never prunes or ends a record whose PID namespace it cannot observe. Human inboxes remain addressable without a running agent. A role wins over a same-named person; use `person:<name>` to address that person explicitly. Refused sends can instead reach a known person with `--to person:<name>` or leave a spec comment with `aida comment add <SPEC>`. The hint names the operator only when `AIDA_USER` resolves to a known person, or exactly one person holds an explicit `operator` seat in the team roster (`registry/team.toml`). Otherwise it names no recipient. It never falls back to an arbitrary catalog person. Mention-drop warnings suggest contacting the operator, without suggesting another comment command. Historical inbox inspection remains available after an agent ends.

Antigravity installs the mailbox relay on both `SessionStart` and `PreInvocation`.
The latter consumes the native `conversationId` payload and wraps the
non-marking `mailbox notice` output in an `injectSteps` system message, so
handle-addressed mail appears on the next model invocation. SessionStart
keeps the `awaiting --notice` path that registers the raw client. Notices leave mail unread; `aida mailbox inbox` acknowledges
it. Raw clients resolve their startup-registered handle from their nearest
vendor process, ahead of potentially inherited vendor/session variables.
Native session evidence remains the fallback inside a PID namespace where
the host client process is invisible.
Once registered, a raw child's default inbox and notices use its own handle
for private mail. Inherited `AIDA_USER`, `AIDA_HANDLE` and `AIDA_AGENT_NAME`
cannot add a parent's private inbox or acknowledge its messages. A cataloged
human `AIDA_USER` remains an additional inbox; session role and vendor inboxes
remain shared audiences. This mail filtering does not change the principal
used for grants or queue ownership. <!-- trace:STORY-1843 | ai:codex -->
Refresh existing wiring with `aida init --force` to install the turn handler.
<!-- trace:STORY-1843.ac0eef56 | ai:codex -->

Headless children scrub parent session identity before receiving their own
handle and launch token. Failed spawns release their unbound reservation;
successful spawns retain it while the child starts. Generated handles use
persisted monotonic counters and at most 64 candidate probes. Allocation
scans cap each directory at 4,096 entries, 64 KiB per record and 250 ms;
namespace-lock acquisition also caps its wait at 250 ms. Exhaustion refuses
allocation rather than ignoring possible owners. Run `aida agent gc` on the
host to collect expired reservations with no live or recently unverified
holder. GC archives them under `handles/retired/` and retains a native-session
index for resume. Archived spellings and their mail remain intact and are
never issued to a new generated recipient. `--dry-run` reports the count
without moving records. GC also archives ended registry rows under `agents/retired/`, preserving
history and resume views while removing them from allocation scans.
SessionStart does not perform either archive sweep.
<!-- trace:STORY-1843 | ai:codex -->

An invalid or unreadable individual agent record produces a warning while other inboxes remain available. Role, vendor, and person catalog names retain their meaning when an older agent uses the same name; addressing that agent by ID does not grant it the reserved inbox.
<!-- trace:STORY-1843 | ai:codex -->

**Don't reach for it when** — you're *assigning* work rather than discussing it — that's `aida brief` (one-directional, with `aida brief --notify` for urgency). And don't expect a message to survive a fresh clone until you `aida mailbox sync` — the local layer is per-clone until digested into the store.

**Key options (rationale only).**
<!-- trace:TASK-176 | ai:codex -->
- `send --to` vs `--broadcast` — single recipient or everyone; mutually the two addressing modes (omit `--to` and pass `--broadcast` to reach all).
- `aida mailbox send <BODY>` vs `aida mailbox send --body-file <path>` vs `aida mailbox send --stdin` — exactly one body source. Prefer `--body-file` or `--stdin` for rich multi-line agent notes so the shell cannot expand backticks or `$()` before AIDA receives the message.
- `send --thread` / `--in-reply-to` — attach to an existing conversation rather than starting a new thread; how a back-and-forth stays grouped.
- `send --urgent` — surface out-of-band (statusline nag) instead of sitting unseen in a chronological inbox. Lightweight: normal-vs-urgent only, the same interrupt/no-interrupt choice `brief --notify` makes.
- `send --from` — override the sender id (default is this shell's agent/user identity); for when you're sending on another identity's behalf.
- `inbox --all` — the operator-wide read-only view across *every* agent, distinct from a single agent's `inbox` (which marks that agent's mail seen).
- `sync` — digest local → git-canonical store and commit on the orphan branch; idempotent. The durability/shareability step; until you run it, messages are local-only.
- `retract` vs `delete` — `retract` leaves a withdrawn *tombstone* visible in views (you take back what you said, transparently); `delete` removes it with a marker so sync won't resurrect it. Different intents: walk-it-back vs erase.

**Gotchas.** Reading an `inbox` *marks it seen* — that's a side effect, so use `inbox --all` (read-only, operator view) when you want to peek without clearing unread state. And the local-vs-synced split surprises people: a message is durable and cross-clone only *after* `aida mailbox sync`.

`aida mailbox migrate --dry-run` previews legacy mailbox reconciliation. Add `--json` for the versioned report. It inventories local and canonical messages, original read-state filenames and timestamps, unresolved recipients, divergent copies of message IDs, and read-state filename collisions. Message bodies are omitted. Malformed or unsupported inputs and observed mailbox changes refuse the preview rather than silently skipping evidence.

The report's fingerprint covers mailbox and read-state content; recipient liveness is sampled advisory information. Historical shared group watermarks cannot reconstruct private acknowledgements. The command does not assign recipient UUIDs, acknowledge mail, rewrite messages or enable a new layout. Write-mode migration is unavailable pending advisor review of the real-store report and compatibility policy.
<!-- trace:STORY-1843 | ai:codex -->

#### Mail retention when a worktree goes

Removing, resetting or reusing a worktree first moves its local mailbox (messages, read receipts and watermarks) into a private retention area under the repository's Git directory. This applies to `aida session end`, pool return and reuse, `aida worktree pool destroy`, `aida worktree gc` and `dismiss`, reaps and doctor heals, and seat-clone teardown. The next occupant of a reused tree starts with an empty mailbox. Retained mail is kept, not acknowledged or archived.

If the mail cannot be retained safely, the worktree is kept. In that case the session is not ended: its lease, claim and manifest stay, and the lease is marked `ending_retained_at`. Only its grant is revoked. `aida ps`, `aida session list` and the doctor show such a session as `ending-retained`, neither active nor cleaned up. Retry once the cause is fixed.

A worktree whose mail cannot be fenced is always kept, even when its mailbox looks empty: a seat clone with no verified host binding to its main checkout, or a worktree outside any Git repository the host can verify.

A worktree whose mailbox holds mail is kept until an operator records that every AIDA process on the machine that can write mail takes a write permit. Run `aida mailbox retention activate --dry-run` to see where that stands; add `--json` for the report. It lists each running `aida` and `aida-server` process and whether it runs this build. The report is incomplete, and nothing can be recorded, when it runs inside a container's pid namespace or cannot read a process's command line. When the report shows every writer taking a permit, run `aida mailbox retention activate` from an interactive terminal to record it. The command re-runs the scan and refuses for agents. A worktree with an empty mailbox is not affected.

The record names the source build that made it: the version and git commit that `aida` and `aida-server` built from the same checkout share. Both honour it, and so does the same source built and promoted again. A build from a different commit (after an upgrade or `aida dev rollback`) does not: the report then says the activation was recorded by a different build, and the refusal names both builds. Run `aida mailbox retention activate` again with the new build.

The dry run also lists mailbox leftovers. Reusing a worktree moves its emptied mailbox aside to `.aida/.mailbox-tomb-<id>/` before removing it. If a message lands there in that window and cannot be put back, the directory is kept as stranded mail: the report names it, and AIDA never deletes it. Move its messages out by hand. `aida agent gc` removes empty tombs and staging directories left by a crashed mailbox creation once they are ten minutes old, and never sooner. It also removes per-worktree markers whose worktree has since been removed or recreated. A mailbox that lost its identity file is refused with the command that moves it aside, so the next write creates a new one.
<!-- trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude -->

A worktree deleted from disk without AIDA (for example, by another tool) stays registered with Git. While it is registered, every mail read reports the view as partial and mail cannot be acknowledged. The deleted worktree's mailbox is gone, so any mail it held cannot be recovered. Run `aida mailbox retention prune` to drop those registrations; add `--dry-run` to list them first, or `--json` for the report. Git keeps locked worktrees through its own prune, because a lock usually marks a worktree on removable media. AIDA does the same and keeps a locked worktree unless you pass `--include-locked`. A worktree whose parent directory is also missing is never dropped, since that directory may be on media that is not mounted.
<!-- trace:TASK-b6b1278890774ef993d2ebc327862470-118 | ai:claude -->

The mail is checked once more right before the worktree is removed or reset. If something wrote to it after it was retired, the worktree is kept, that mail stays in place, and the session is not ended, exactly as above. Neither `aida session end` nor the automatic cleanups suggest removing such a worktree with a raw `git worktree remove --force`, which would delete that mail. Use `aida worktree dismiss <path>`, which runs the same check.
<!-- trace:STORY-1843 | ai:claude -->

#### Mailbox layout upgrade order

The recipient-UUID mailbox layout replaces each legacy mailbox directory (`.aida/mailbox/` and the store's `mailbox/`) with a small manifest file. The file starts with `# aida-mailbox-layout-manifest` and names the new layout. The legacy directory is archived beside it, and the original paths of its unresolved addresses stay readable. A binary older than the manifest fails at that path: creating or listing a directory there is an error, so it exits nonzero without writing. A binary that knows the marker refuses with "migrated to layout N … Upgrade aida on this clone first". Neither can silently write mail that the new layout would never read.

Upgrade in this order:

1. Upgrade `aida`, `aida-server` and the MCP and hook binaries on every clone and machine that sends or reads this project's mail, to a release whose supported layout includes the new one.
2. The operator runs `aida mailbox migrate --dry-run --json` and approves that exact report (its SHA). Group cohorts and divergent message IDs are decided in an explicit input file. The tool never picks them.
3. The operator, never an agent, runs write mode. It reruns the dry run and refuses unless the report is byte-identical to the approved one. It takes a backup ref first, archives the legacy directories, installs the manifests, and is idempotent on rerun. *(Write mode has not shipped yet.)*
4. Each clone passes its own admission check before UUID delivery starts there. A clone that has not been upgraded meets the manifest and refuses, rather than writing to the old layout.

A manifest that claims the layout this build already uses is reported as damaged. Restore the directory from the backup ref instead of editing the manifest.
<!-- trace:STORY-1843 | ai:claude -->

**Chains with** — agents launched via `aida agent new` coordinate over the mailbox; the advisor's `watch` loop runs a mailbox-triage pass; briefs route *assignments*, the mailbox carries the *conversation* around them.

---

For host notification adapters, `aida mailbox wake-snapshot --recipient advisor
--json` emits a version-1 envelope of unread message IDs and acknowledged IDs
from the logical mail view: every worktree of the repository and the canonical
store, with read state from the common-Git receipt anchor. `--recipient` must be
one of the calling process's own inboxes (the audience `mailbox notice`
resolves); it narrows and never widens. `partial: true` (with safe
`partial_codes`) means some source was not fully read, so more mail may exist.
`pending` counts queued role mail no reader has taken; the snapshot never takes
it. It never acknowledges mail. `--local-root` selects the checkout the view is
read from. See the [Codex mail wake adapter](../agents/codex-mail-wake.md)
for opt-in setup, deduplication, and uncertain-delivery recovery.

`--recipient <handle>` naming a registered agent reads that agent's own inbox
(its handle plus its recorded role) when the agent is armed for mail wake and
still live. `aida mailbox wake-run` watches every armed Codex agent of the
project and wakes its thread; `aida mailbox wake-service install|uninstall|status`
manages the one user service that runs it. Both are described in the
[Codex mail wake adapter](../agents/codex-mail-wake.md).
<!-- trace:STORY-1843 | ai:claude -->
<!-- trace:TASK-2347 | ai:codex -->

#### Relay mail lifetime (`aida relay-mail`)

A relay run is one mail recipient for its whole life. Its label is the handle other agents address, and it stays the same while the relay rotates through child sessions. The relay controller calls these commands; operators rarely do.

- `aida relay-mail probe --json` prints `{"protocol": "aida-relay-mail/1", "ops": [...], "terminal_reconcile": true}`. `ops` lists `probe`, `bind`, `child-bind`, `end`, `reuse` (the `bind` choice for an ended run's label, below) and `outbox`. A relay compares the protocol exactly and refuses to advertise its label on a mismatch.
- `aida relay-mail bind --label <L> --controller-pid <PID> --run-lock <PATH> --protocol aida-relay-mail/1` runs before the relay launches any session. It refuses on a different protocol before anything is reserved. The controller must hold its run lock: an exclusive `flock` on a regular file whose first line is the controller's pid, held through one of the controller's own open descriptors. On Linux this is checked in `/proc/<pid>/fdinfo`, which attributes a lock to the processes sharing the open file that holds it. A relay that runs `exec 9>"$lock"; flock -x 9` therefore qualifies, and a lock held by any other process does not. `bind` refuses when 1024 live run records already exist. It reserves the label for a new run and prints the run id and, once, the run capability (`run` and `capability` in `--json`; two lines otherwise). The capability is 256 random bits. The record stores only its SHA-256, so it cannot be recovered later. The controller keeps it private and never exports it to a child session's environment. The run record, under `.aida/agents/relay-runs/`, stores the controller's pid, process start identity, boot id and PID namespace, and the run lock's identity.
- `aida relay-mail child-bind --run <ID>` binds the next child session to the same run. Like `end`, it reads the run capability from stdin, or from `--cap-file <PATH>` (a regular file of mode 0600 that you own). The capability is never accepted as an argument, because process arguments are visible to every user on the machine. A wrong or missing capability is refused before anything is written. The run id and controller pid are public, so neither grants anything on its own. With `--native-session <SID>` it binds that session now. Without it, it prints a launch token. Start the child with that token in `AIDA_HANDLE_LAUNCH_TOKEN` and its agent name set to the label, and the child's startup binds the label. The run id never changes across rotations.
- `aida relay-mail end --run <ID> [--cap-file <PATH>] [--reason stop|exit|blocked]` records the run's end once, for the holder of the run capability. A repeat reports the recorded end and changes nothing. An ended run's record stays in `.aida/agents/relay-runs/` until each of its end notices (below) is delivered. It then moves, with its outbox, to `.aida/agents/relay-runs/ended/`. Only open runs and ended runs that still owe notices count against the limit of 1024 run records. Past that limit, readers consider the oldest 1024 records (with a warning) rather than failing, so reconciliation can still end dead runs and bring the count back down.

Mail to the label is accepted between sessions while the run is open and its controller is alive, including while the relay is paused. If the controller's process cannot be observed from the sending shell (another PID namespace), the mail is still delivered, with a warning. After the end is recorded, mail to the label is refused unless a live session still holds it. A controller that dies without calling `end` (a crash or `kill -9`) is ended by the next `relay-mail bind` or host-side `aida agent gc`, once its process is provably gone: the machine rebooted, the pid is free, or the pid belongs to another process. A run whose controller cannot be observed is never ended this way. This crash path needs no capability, because it only ends runs whose controller is provably gone. If relay reconciliation fails, `aida agent gc` prints a warning and continues with the rest of its maintenance.

A new run can take an ended run's label. If the ended run left mail unread, `bind` needs a choice about it. Without one, it refuses with the exact count and reserves nothing. The two choices are:
- `--leave-with-old` leaves that mail with the ended run, and the new run does not see it.
- `--handover` makes each unread message readable by the new run too. Nothing is moved, renamed or marked read: the ended run's read receipts stay as they were, and the new run acknowledges under its own run id. `--json` reports the previous run, the unread count and how many messages were handed over.

The count is taken only by `bind`, after it has checked the controller and its run lock, and only for the ended run that holds the label's reservation. It is read without holding the handle namespace lock; `bind` then re-checks under that lock that the same run still holds the label, and refuses if it changed. No run capability is involved: the new run's capability is minted only by this bind. It reports counts, never message contents. `bind` refuses the label whichever choice is given in two cases: the ended run's mail could not be fully counted, or the label has unread mail sent with no delivery record (written before relay delivery records, or by a writer that bypasses them). Mail of that second kind is readable by whoever holds the label, so the new run would read it. Choose a new label instead.

A worktree that is missing or prunable never makes the count incomplete. A live worktree whose mailbox could not be read, a worktree list that could not be completed, or read receipts that could not be read do. While read receipts cannot be read, mail already read would look unread, so no end notice is recorded and nothing is handed over until they can be read again.

A session or launch that takes an ended run's label by name (`-n <label>`) cannot make that choice, so it is refused the way a `bind` with no choice is: when the ended run has any unread mail, or its mail could not be fully counted. With no unread mail the name is free.

Mail sent to a relay label while its run is open belongs to that run. The message is unchanged, and a delivery record beside it names the run. Every session of the run reads that mail and acknowledges it as the run. A later run under the same label does not see it. This applies to `aida mailbox send`, the MCP send tool and comment mentions.

When a run ends with mail its sessions never read, that mail stays with the run: it is not deleted, renamed or marked read. The sender of each unread message gets a notice, as a reply in the same thread. The notice names the ended run and the message's subject, never its body. The advisor role also gets one notice per unread message, without the subject. A sender who is also the only advisor reader is told once. A notice that reached a relay which then ended is never reported back.

Each notice is recorded once in the run's outbox, however often the end is recorded or retried. If a sender cannot be reached now (its session ended, or its handle now belongs to someone else), or no advisor with a recipient id is live, the notice stays pending. The next `relay-mail end`, `relay-mail bind` or `aida agent gc` retries it. An outbox entry that cannot be read is skipped with a warning and the other notices still go out; its run stays live until the entry is fixed or removed. If notice delivery fails, `aida agent gc` prints a warning and continues. `aida relay-mail outbox [--run <ID>] [--json]` lists each notice and whether it was delivered or is pending, with the last error. Sending mail ends a run whose controller is provably gone and records its notices, but delivers none of them. Read-only commands, such as `aida mailbox migrate --dry-run`, never end a run.
<!-- trace:STORY-1843 | ai:claude -->

### `aida driver`

**One line** — register the long-running loops (relays, drains, headless agents) that work in-progress specs, so every view can say *who* is driving each one.

**Mental model.** A spec's status says *what stage* it is at; it does not say whether anything is working it right now. A session lease answers that for one interactive seat, but a relay that cycles through a batch of specs outlives every child it spawns, so a dead child pid used to make a perfectly healthy spec look abandoned. The driver registry fixes that. A driver writes a small record under the main checkout's `.aida/drivers/` (the same place from every linked worktree, as with session leases) with its pid and, optionally, the tag selector it works through (`batch:next`). From that registry, the leases, the live drain lock and the spec's batch tags, AIDA *derives* one driver per in-progress spec on every read, and never stores it on the spec. The readings are `relay:<run>`, `drain:<pid>`, `agent:<session>`, `queued (batch:X)` (in a batch whose driver is not running) or `idle <age>` (nothing is working it). `aida ps` and `aida list` print the driver beside the status (`In Progress · relay:lib-split`), and their JSON/TOON carry it as separate `driver` and `driver_kind` fields. A spec a live driver covers is never listed as orphaned. When a session's own process has exited, its row reads live only if its lease names that live driver or a live drain covers the spec, and even then only a stalled or unknown reading is replaced: uncommitted work still reads salvageable, and finished work still reads as waiting on integration.

**Reach for it when** — you are writing or running a loop that drives specs without a human seat. Call `aida driver register --run <run> --pid "$$"` once when it starts (re-running it refreshes the heartbeat, but nothing requires a cadence), and call `aida driver deregister` when it exits cleanly. Export `AIDA_RELAY_RUN=<run>` (or `AIDA_DRIVER=<kind>:<id>`) in the loop's environment, and every worktree, claim and session it starts records that driver on its lease, so the attribution survives even when no batch tag links them. Use `aida driver list` to see what is registered and which records are still live.

**Don't reach for it when** — you are an interactive seat working one spec. Your session lease already makes you `agent:<session>`, so there is nothing to register. Nor is it a cleanup tool: a dead driver's record is harmless (it simply reads as not live), readers never delete it, and `deregister` only removes the record you name (it does not check who registered it).

**Key options (rationale only).**
- `register --tag` — the selector that links a relay to the specs it will work. An in-progress spec carrying that tag resolves to the relay while it is live, and to `queued (batch:X)` once it is not.
- `register --pid` — the driver's own long-lived pid. Pass `--pid "$$"` from the loop's own shell: called from `$(...)` or a pipe, the parent of `aida` is a throwaway subshell, so a relay must always pass it (other kinds fall back to the parent process), and pids 0 and 1 are refused. A record is live exactly as long as that pid is alive; the process start time recorded at registration guards against a recycled pid. Because that start time is the only reuse guard, `register` refuses a pid that is not running (or whose start time cannot be read) unless `--ttl` is given. There is no heartbeat expiry by default, so a relay that registers once per hour-long session stays live throughout.
- `register --ttl` — opt-in heartbeat expiry in seconds. With it, the record also stops being live if `register` is not re-run within that window, so a wedged loop that still has a live pid ages out. Use it only from a loop that refreshes on a fixed cadence shorter than the TTL.
- `register --run` — the driver id. Only letters, digits, `.`, `_` and `-` are accepted, and `deregister` applies the same rule.
- `list` — every record with its live/not-live reading and heartbeat age. A record that reads not live is a driver that exited (or, with `--ttl`, stopped heartbeating); its specs fall back to `queued` or `idle` in the other views.

**Gotchas.** The driver is an operator/developer surface: the record is local runtime state, not synced across clones. "Idle" is measured from the newer of the lease start and the spec's last modification; branch commits do not count yet. `aida awaiting` measures it differently: an In Progress spec whose driver is `idle` is listed as *in-progress but nothing is working it* once it has had no commits on its branch and no status change for `[awaiting] idle_in_progress_hours` (default 24, set in the project or `~/.aida/config.toml`), so its age there can be longer than the `idle` age `aida ps` shows. A spec a subagent may be working stays on the orphan list instead; one a live fan-out is plausibly building is not listed. <!-- trace:STORY-1680 | ai:claude -->

**Chains with** — `aida ps` and `aida list` show the derived driver; `aida session start` and `aida worktree add` stamp the lease from `AIDA_RELAY_RUN`/`AIDA_DRIVER`.

---

## Where to go next

You now own the seat model and the coordination substrates: who's wearing which hat, who holds which scoped work, and how agents are launched and talk to each other. Next:
- **[Chapter 3 — Work & autonomy](03-work-autonomy.md)**: the queue, pickup, and the autonomous drain — where leases get taken and the away/home presence verbs that gate `advisor watch`.
- **[Chapter 7 — Project setup](07-project-setup.md)**: `node` (per-clone identity, the writer-of-record this chapter's leases and mailbox sync assume) and the rest of the machine-global setup.
- **[Chapter 8 — Reporting](08-reporting.md)**: `aida status` — the unified "what's going on here?" view that the advisor dashboard complements.
- [`docs/review-process.md`](../review-process.md) — **who reviews by mode**, and the precise reviewer-vs-advisor-tier distinction (the reviewer gates code in every mode; the cold-boot advisor tier fires only on a punt). The authoritative source for the seat/review lore in this chapter.
- The autonomy + escalation architecture in full — the three-mode ladder, the implementer→advisor→human cascade, the file-based handshake substrate — is [`docs/architecture/autonomy-and-escalation.md`](../architecture/autonomy-and-escalation.md).

### Mid-session expiry notices in Claude Code, Codex, and Antigravity

The Claude Code (`settings.json`) and Codex (`codex-hooks.json`) templates
install an informational `PreToolUse` hook covering every tool, and the
Antigravity (`antigravity-hooks.json`) template installs a `PreInvocation` hook
(`--client agy`) emitting `injectSteps` ephemeral context. Each emits context
once when a seat grant crosses 2h, 30m and expiry, including the terminal
renewal command. It never prompts or decides tool permissions. New grants or
renewal timestamps reset the notice epoch. Child expiry uses the earlier of
child and parent expiry. Baseline seats stay silent. The Antigravity template
also installs a `SessionStart` hook running `aida-mail-notice.sh --client agy
--event SessionStart`, the same mail notice and host-side dead-registry-row gc
the Claude Code and Codex `SessionStart` hooks run.

Without `AIDA_SESSION_GRANT`, the shell exits without launching the CLI
(printing `{}` for Antigravity). With a grant it caps the notice command below
2 seconds. The notice path initializes no requirements graph or cache; valid
grants use the shared validator, including the local team roster lookup.
Runtime notice ledgers live beside grant records and use permanent lock sidecars
plus atomic replacement. Unreadable, corrupt or unwritable ledgers yield no
context (`{}` for Antigravity), one stderr diagnostic and exit 0. Existing
SessionStart and statusline warnings remain available. See
[verified hook delivery](../testing/bug-2014-grant-expiry-notices.md).
<!-- trace:BUG-2014 | ai:codex -->
<!-- trace:TASK-1744 | ai:antigravity -->
<!-- trace:TASK-2253 | ai:claude -->

## Watching for new review heads

`aida review watch` discovers open, non-draft GitHub PRs or GitLab MRs and
starts ordinary attested reviewers as soon as a head is eligible. It does not
wait for CI. Run it from a shell with a validated grant that delegates the
reviewer seat. `--once` performs one discovery pass and leaves admitted reviews
running; the ordinary loop polls every 10 seconds. `--interval` accepts 1–3600
seconds, and `--max-concurrent` accepts one or two reviewers (default two).
`--vendor` uses the ordinary queue-work vendor selection when omitted.

The selected forge CLI (`gh` or `glab`) must be on PATH. Git/forge probes have
per-command timeouts; the watcher avoids unbounded version-detection probes.
Initial root resolution and later probes clear ambient Git routing variables.
Discovery pins origin's forge host/project and fetches each change's own
target branch (GitHub `baseRefName`, GitLab `target_branch`) and head into
private temporary refs. It fetches by remote name, so a credential stored in
the origin URL never appears in process arguments. A missing or invalid base,
unknown metadata, head movement, or unreadable base evidence refuses dispatch.
A stale base with overlapping changed paths receives a local mailbox request to
merge the change's base branch, named in the notice; a stale non-overlapping head remains reviewable. The
recipient is the exact forge author login/username, with no inferred mapping to
an agent seat. Read that identity's mailbox with `aida mailbox inbox <login>`.
The watcher never updates a branch.

Version-1 atomic state lives under the main worktree's `.aida/review-watch/`.
Permanent admission and per-change lock files coordinate watcher instances.
Each launch is reserved before spawn, and its dispatcher records PID, process
start identity and boot ID before calling `queue work ... --role reviewer
--no-human`. Pending/running receipts count toward global capacity, even after
a PID disappears or is reused. When the dispatcher observes queue work exit
unsuccessfully (a stale-base or intermediate-only refusal, a reviewer lease
already held, or a launch-boundary revalidation failure), it records the
attempt as exited with the last line of its log. That frees the slot, the head
is not relaunched, and a later head dispatches normally. The parent records
each dispatcher's process identity; if a dispatcher exits before claiming its
reservation (a refused grant, an unusable store, a timed-out claim), any
watcher, including a restarted one, records the reservation as exited and
frees its slot. Only a dispatcher lost after claiming, before recording an
outcome, stays reserved until its exact process (start identity and boot ID)
is gone. The review is carried by the reviewer process that queue work
launches, and queue work and the reviewer can each outlive their parent. So
the dispatcher records queue work's identity, queue work records the
reviewer's identity before waiting on it, and the slot is freed (recorded as
exited) only once the dispatcher, queue work and the reviewer are all gone. If
either process was being spawned but never recorded, the attempt stays held
and the capacity refusal names it with the recovery step: confirm no `queue
work` or reviewer process for that change remains, then record the attempt as
exited while no watcher runs. The watcher signals a process only to supersede
a review (below). Terminal receipts
retry through lock contention for the same window as the claim. The capacity refusal names each
holder's change, head and token. A missing/corrupt/unsafe state file refuses
reinitialization; inspect the state, dispatcher logs in `review-watch/logs/`,
and `aida session leases --all` before operator recovery. Do not delete lock
files or clear ambiguous receipts while a reviewer may still be running.
Changes that are skipped leave no state, and state for changes no longer open
and non-draft is pruned on each complete discovery pass unless an attempt is
still live. Observed blocking review identities are kept per change in
`review-watch/history/`, which pruning never removes, so a draft toggle or a
close and reopen cannot reset the round count. In the continuous loop a refusal for one change is logged and the
pass continues; only an unusable state file stops the watcher. `--once`
reports any refusal.

Stale-head notifications have deterministic IDs and retry without overwriting
mailbox reader flags. Delivery is recorded only after successful publication.
A head is launched once. Rounds follow the ordinary review policy: only
request-changes or rejected verdicts (the reviews that leave a findings block)
consume a round, so an approved head later merged with its base branch is
re-reviewed at round 1 with the full-sweep contract, and a third blocking
round is refused. Existing manual/drain verdicts and their retained archives
count the same way; current/archive
copies deduplicate using the verdict writer's recording identity. Admission
retains those identities in private watcher state before queue work clears the
current verdict, including legacy rounds without an archive. Canonical history
uses ordinary writer permissions; watcher state and locks require mode 0600. History is
rechecked before queue work and at the final vendor boundary; unreadable or
unidentifiable history refuses dispatch. The round prompt comes from that
combined count. Ordinary queue-work risk and review authority checks still apply. Review instructions forbid local
Cargo, tests/builds, merge, and branch updates. The watcher adds no verdict
authority and takes no merge action; its ship-readiness report (below) only
reports.

### Superseding a review when the head changes
<!-- trace:TASK-1945 | ai:claude -->

When discovery sees an open, non-draft change at a new head while a review of
an earlier head is reserved or running, the watcher re-reads the head from the
forge once and then supersedes the earlier review. A closed or draft change is
never a trigger. First it records a durable supersede receipt in the attempt
(the new head and the time). From then on the attempt can record no new queue
work or reviewer process, so a dispatcher that has not spawned yet spawns
nothing. The attempt keeps its capacity slot until every one of its processes
is confirmed gone.

It then asks the review's processes to terminate (SIGTERM) and kills them
(SIGKILL) if they are still running 20 seconds later. Each signal is claimed in
the receipt before it is sent, so two watchers, or a restarted one, never
signal twice. Every signal goes through a pidfd, and the process it pins must
still match the recorded identity (kernel start time and boot ID), be owned by
the same user, and sit in the dispatcher's recorded process group. A reused PID
is treated as gone and is never signalled. Other members of that group (the
vendor's own children, say) are signalled only when their parent chain reaches
a recorded process that is still running. A member that cannot be proved to
belong to the review is never signalled. A process that left the group is not
covered by group signalling: Claude Code's Bash tool, for example, runs each
command under `setsid`, so those commands sit in a session and group of their
own. Recorded processes are signalled leaf first: reviewer, queue work, then
dispatcher. The watcher never signals a
whole process group, and it never touches session leases, worktrees, verdict
files or merge-hold markers; `aida session reap` handles those as usual.

The new head is admitted only after the earlier review's recorded processes,
its process group and any process it was still spawning are all confirmed
gone. A superseded review that was stopped before it recorded a verdict uses
no review round; one that had already recorded a verdict counts toward the
round history like any other review. The slot
stays held, and nothing is signalled blind, when signalling is unsafe or
fails: no pidfd support (ENOSYS) or a platform other than Linux, a permission
error, an identity in an unexpected process group, a process that survives
SIGKILL, or a group member that cannot be proved ours. Each pass reports the
cause, and the capacity refusal and `aida ps` (a "Review-watch slots held"
section; `review_watch_held` in `--json`) show it. Once you have stopped the
remaining processes yourself, `aida review watch --release PR-N` clears the
attempt. It stays refused while any recorded process is still running. An
attempt recorded before process groups were kept gets per-process signals
only, with no group sweep.

### Ship-readiness report
<!-- trace:TASK-1945 | ai:claude -->

Once a change has a recorded verdict and no review of it is reserved, running
or being superseded, the watcher judges whether it is ready to ship, at most
once every two minutes per head. It reads the evidence `aida pr ship` checks,
read-only, and stops at the first refusal:

- no live review marker covers the head;
- no merge hold of any kind is set (a marker, an unreadable marker, or the
  forge label). The report names the hold; the watcher never releases one;
- the head is 0 commits behind its base. A head behind only on paths it does
  not touch is "ready after its branch is updated" (`gh pr update-branch N`,
  or on GitLab a merge of the target branch into the source branch); one
  behind with overlapping paths is not ready. The advice is always a merge:
  a rebase rewrites the change's own commits, so its approval no longer
  carries over;
- an authorized, non-author approval covers the head, through the same
  decision `aida pr ship` makes, including the carry-over of an approval over
  clean merges of the base branch. The verdict corpus must not refuse the
  head;
- every CI check on the head commit has finished and passed (success,
  neutral or skipped), and every check the project requires is among them.
  A project names its required checks in `.aida/config.toml`; unset
  requires none beyond every check passing. AIDA requires its
  `Build (ubuntu-latest)` check, which requires every CI lane:

  ```toml
  [review_watch]
  required_checks = ["Build (ubuntu-latest)"]
  ```

  CI is read by commit SHA: GitHub check runs and commit statuses, or GitLab
  job statuses. No checks, a pending or failed check, a missing required
  check, an unreadable config, or a failed read is not ready;
- a final forge read finds the head unchanged.

Each judgement prints `PR-N at <head>: ready | ready after its branch is
updated | not ready: <reason>` on stderr. The last one per change is shown
by `aida ps` (a "Review-watch readiness" section; `review_watch_ready` in
`--json`). It is display only: `aida pr ship` checks everything again. A
head with a wake still waiting for delivery is judged again on the next
pass; otherwise it waits out the two minutes.

When a change becomes ready, the watcher mails the advisor once per head,
readiness and approval. The mail names the command to run (`aida pr ship N`,
or the branch update first) and an actionable `ReviewWatchReady` event lets
`aida watch --emit-wakes` wake a supervisor. The mail has a deterministic ID
and is recorded as sent only after both the mail and the event succeed, so a
failure is retried without a duplicate mail. The watcher never merges, pushes
or updates a branch; an active fire-mode mandate consumer acts through the
ordinary `aida pr ship`.

### Review notices
<!-- trace:TASK-1945 | ai:claude -->

The same pass sends two more notices, each once:

- **Findings.** When a review of the current head requested changes (or
  rejected it), and no approval of that head followed, the change author gets
  one mail per verdict. It goes to the AIDA identity the forge login maps to
  (the advisor when none does), the same mapping the merge-base notice uses.
  The mail names the findings file (`.aida/review-verdicts/PR-N.json`, plus
  the archived round file when the verdict was also archived), the round
  number, and whether this was the last round under the two-round cap. A new
  blocking verdict, on a new head or the same one, is a new mail.
- **Round cap.** When a change has used both review rounds and its head has
  no review, the watcher starts none, and mails the advisor once for the
  change. The mail names the reviewed heads, the unreviewed head, and the
  findings file.

Like the ready wake, each notice has a deterministic ID and is recorded as
sent only after delivery. A failed delivery is retried on a later pass and
does not stop the readiness report.
<!-- trace:TASK-1993 | ai:codex -->

Launch setup keeps invocation receipts before Git allocation. Fresh worktrees
materialize at a private staging path and publish to the requested absent path
only after checkout succeeds. Even a failing post-checkout hook triggers cleanup.
Cleanup verifies the worktree's `.git` pointer and deletes a newly created branch
only while its ref still equals the recorded start commit. Pre-existing paths,
branches, adopted leases, and changed identities are retained with recovery
instructions. Shared claims carry optional invocation IDs; rollback compares the
exact acquired claim under the store lock and publishes outside that lock.
Legacy claims remain readable and are never adopted as rollback authority.
<!-- trace:BUG-2188 | ai:codex -->
