# Codex Migration Validation Runbook

*Audience: a team moving some or all of its AIDA work from Claude Code to OpenAI Codex CLI, who needs to prove — concretely, not on faith — that a Codex agent can do real AIDA-tracked work. 2026-06-25.*

This is a numbered, copy-pasteable dogfood. It walks one spec end to end through a Codex
session: discover AIDA's MCP tools, read a spec, claim it, make a traced edit, run a
targeted test, write coordination state back, and verify from the CLI that the writes
landed on the same substrate the rest of the team sees. It closes with the negative-path
checks (blocked approval, session-communication gaps, sandbox posture) that a migration has
to confirm, and a per-step troubleshooting table.

It is the executable companion to three reference docs — read them once before you run this:

- `docs/agents/codex-mcp-setup.md` — the empirical Codex MCP setup (STORY-398), the
  authoritative source for registration, tool discovery, and response-shape expectations.
- `docs/agents/cross-agent-onboarding.md` — the shared MCP operating model and the full
  tool/resource catalog for any non-Claude agent.
- `docs/agents/porting-claude-code-to-codex.md` — the migration gap analysis: which
  Claude Code runtime controls (hook `defer`, input/output rewriting, command-backed
  status lines) do **not** port directly, and the replacement paths.

The grounding rule throughout: **`tools/list` over MCP is canonical for tool and argument
names.** If a command in this runbook disagrees with what your `aida <cmd> --help` or
`tools/list` shows, trust the live surface and file a finding — that is itself a valid
outcome of this runbook (see Step 9).

---

## Prerequisites

Before Step 1, confirm:

- `aida` is on `PATH` (`aida --version`), or you have its absolute path. From an AIDA
  source checkout, run `aida dev activate` so the in-repo build shadows any installed release.
- Codex CLI is installed (`codex --version`).
- `git` is configured with `user.name` and `user.email`.
- You have a project to validate in. Either:
  - an existing AIDA project (a directory containing `.aida/`), or
  - a throwaway project you initialize in Step 1.

This runbook does its real-work steps against a **throwaway spec** so it never parks one of
your real specs `NeedsAttention` or pollutes your live queue. If you run it against an
existing project, use a spec you are happy to drive end to end (or file a fresh one in
Step 3).

---

## Step 1 — Scaffold the Codex MCP registration

`aida init` scaffolds the Codex-side MCP registration as part of project setup. The
`--agent` flag selects which agent profiles get scaffolded; the default is `both`.

For a fresh validation project:

```bash
mkdir aida-codex-validation && cd aida-codex-validation
git init
aida init --agent codex      # or `--agent both` (the default) for Claude + Codex
```

For an existing AIDA project that predates the Codex scaffold, re-run init to add it:

```bash
aida init --agent codex --force
```

This writes a project-local `.codex/config.toml` with an `[mcp_servers.aida]` block plus a
baseline `project_trust_level = "trusted"` line — the Codex-side parallel to the `.mcp.json`
AIDA writes for Claude Code. Confirm the file:

```bash
cat .codex/config.toml
```

Expected content (the load-bearing lines):

```toml
project_trust_level = "trusted"

[mcp_servers.aida]
command = "aida"
args = ["mcp-serve"]
```

If `aida` is not on `PATH`, edit the scaffolded `command` to the absolute binary path.

Now verify Codex sees the registration:

```bash
codex mcp list
```

Expected shape:

```text
Name  Command  Args       Env  Cwd  Status   Auth
aida  aida     mcp-serve  -    -    enabled  Unsupported
```

`Auth: Unsupported` is expected — this is a local stdio transport with no HTTP server or
bearer token. If `codex mcp list` shows nothing, the project-local `.codex/config.toml`
was not picked up; register manually as the fallback:

```bash
codex mcp add aida -- aida mcp-serve
```

(or the absolute-path form `codex mcp add aida -- /absolute/path/to/aida mcp-serve`).

---

## Step 2 — Start a Codex session and confirm tool discovery

Start Codex from the project root so `aida mcp-serve` discovers the correct project:

```bash
cd /path/to/aida-codex-validation
codex
```

or `codex --cd /path/to/aida-codex-validation` from elsewhere. Codex launches the MCP
server itself over stdio — you do **not** run `aida mcp-serve` in a separate terminal.

Optionally, prefer the supervised launcher for an AIDA project — it sets
`AIDA_AGENT_TYPE=codex`, propagates the role/scope env, registers the process under
`.aida/agents/`, and deregisters on exit:

```bash
aida agent new codex --role implementer
```

Inside the session, confirm the AIDA tools are discovered. In Codex the tool namespace is
`mcp__aida__*`. Use Codex's MCP inspector (`/mcp` in the TUI) or simply ask the session to
list its AIDA tools. You should see the spec-graph cluster (`list_requirements`,
`show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`,
`add_comment`, …), the coordination cluster (`claim_task`, `release_task`,
`list_active_leases`, `post_punt`, `file_finding`, `list_briefs`, `read_brief`,
`ack_brief`, …), and the queue/session/role/workflow mirrors.

Shell-side cross-check that the same server starts and advertises its tool surface (this is
the black-box stdio path Codex itself uses):

```bash
tests/test_mcp_stdio.sh --skip-agent-contract
tests/test_mcp_doc_consistency.sh
```

Expected: both suites pass. `test_mcp_doc_consistency.sh` also asserts the tool count in
the doc matches the live `tools/list`, so a green run is your independent confirmation that
the documented surface and the actual surface agree.

> The two test scripts live in an AIDA **source checkout**. If you are validating a
> downstream project that does not vendor them, skip the shell cross-check and rely on the
> in-session `/mcp` discovery plus the round-trip in Steps 3-8.

---

## Step 3 — Read a spec and claim it

Pick (or file) the spec the Codex session will drive. To file a fresh throwaway spec via
MCP from inside the Codex session, call:

```text
add_requirement({
  title: "Codex validation walkthrough — throwaway",
  description: "Scratch spec used to validate the Codex MCP round-trip. Safe to reject after.",
  type: "task",
  status: "approved"
})
```

`type` is required and must be a lowercase taxonomy value; AIDA derives the ID prefix from
it (`task` → `TASK-N`). Do **not** invent `SPEC-N` IDs. Capture the returned ID — call it
`<SPEC>` for the rest of the runbook.

> The shell equivalent, if you prefer the CLI: `aida add --title "..." --type task --status approved`.

Read the spec back through MCP (the read path you'd use on any real pickup):

```text
show_requirement({id: "<SPEC>"})
```

Before claiming, check for an in-flight lease so you don't race another agent:

```text
list_active_leases()
```

Then claim the spec — this writes a lease under `.aida/sessions/`:

```text
claim_task({spec_id: "<SPEC>", role: "implementer"})
```

> Note the surface boundary: `queue_work` over MCP is a **peek**, not a launcher, and the
> CLI `aida queue work` launches a *Claude* session in a fresh worktree. For a Codex flow,
> claim the spec with `claim_task` (above), or bootstrap an isolated worktree + lease with
> `aida session start --owns <SPEC> --role implementer --base origin/main` and run Codex
> from that worktree. Do not expect `aida queue work` to launch Codex.

If a brief was routed to you instead of a direct pickup, list/read/ack it through MCP
(`list_briefs({agent: "codex"})` → `read_brief({path})` → `ack_brief({path})`); see
`docs/agents/codex-brief-pickup.md`.

---

## Step 4 — Make a small traced edit and run a targeted test

Do implementation work in a sibling worktree (no `.aida-store` symlink is needed — a
sibling worktree resolves the canonical store at the main worktree automatically). For this
validation, a minimal traced change is enough — the point is to exercise the loop, not to
ship a feature.

Add an inline trace comment where you touch code, in the project's comment syntax:

```rust
// trace:<SPEC> | ai:codex
```

Format: `// trace:<SPEC-ID> | ai:<agent>[:<confidence>]`. The agent token is `codex`.
Keep SPEC-IDs in developer artifacts (code, commits, plans) — never leak them into
user-facing CLI output or `--help` text.

Run the project's targeted test for the area you touched, plus the format gate. For an AIDA
source checkout:

```bash
cargo test -p <crate> <test_name>
cargo fmt --all -- --check
```

> Use `--check` on `fmt`: bare `cargo fmt --all` rewrites in place and silently masks a
> dirty diff that CI (which runs `--check`) will then fail on.

---

## Step 5 — Write coordination state back through MCP

The whole point of the migration validation is that Codex's writes land on the shared
substrate. Write a progress note and update spec state through MCP:

```text
add_comment({id: "<SPEC>", text: "Codex validation: traced edit + targeted test green."})
update_requirement({id: "<SPEC>", status: "in-progress"})
```

> `add_comment`'s argument is `text`, not `body`. Trust `tools/list` if a doc disagrees.
>
> Status-transition gating: `update_requirement` does **not** let an agent self-promote
> through the advisor-only or merge-driven transitions — `approved`/`planned` are
> advisor-only and `completed` is merge-driven (set by the auto-bump scanner when a
> trailered commit lands on the default branch). `draft`/`in-progress`/`done` are the
> agent-settable states. If a transition is refused, that is the gate working as designed,
> not a bug.

When you finish the work item, mark it done — this flips status and removes it from the
queue in one step:

```bash
aida queue done <SPEC>
```

(`queue_done` exists as an MCP tool too; the CLI form is shown here because the rest of the
ship flow — `aida pr ship`, `aida pull` — is CLI-side anyway.)

---

## Step 6 — Verify the state landed (CLI reads the MCP writes)

This is the round-trip assertion: a *different surface* (the CLI) must see what Codex wrote
through MCP. From the shell:

```bash
aida show <SPEC>
aida queue list
```

Expected:

- `aida show <SPEC>` displays the comment you added in Step 5 and the status you set.
- The lease and queue state reflect your claim/done (the spec is no longer queued after
  `aida queue done`).

This proves Codex is not writing to a private shadow state — it is operating on the same
spec graph, lease files, and queue the CLI and every other agent read. That cross-surface
agreement is the definition of a successful round-trip (per the STORY-398 verdict in
`docs/agents/codex-mcp-roundtrip-verdict.md`).

---

## Step 7 — Commit with the Codex trailer and verify the trace gate

Commit the traced change using the Codex prefix and a trailing spec trailer:

```bash
git add -A
git commit -m "[AI:codex] docs(scope): codex validation walkthrough (<SPEC>)"
```

Format: `[AI:codex] type(scope): description (<SPEC>)`. Two load-bearing pieces:

- The `[AI:codex]` prefix is expected whenever the commit includes a file carrying an
  AI-authored `trace:` comment (one with an `ai:` marker). The commit-msg hook warns if a
  traced commit lacks the `[AI:tool]` tag.
- The trailing `(<SPEC>)` parens are read by the auto-bump scanner; when the squash commit
  lands on the default branch it promotes the referenced spec `Done → Completed`. If a PR
  closes multiple specs, list every ID in one trailing-parens group:
  `(STORY-417 TASK-485 TASK-484)`.

The trace gate (the `aida-commit-msg` hook scaffolded by `aida init`) is **permissive by
default** — a malformed message prints a warning but the commit proceeds. To prove the gate
*enforces* the format (the posture a migrating team likely wants in CI), run it strict:

```bash
AIDA_COMMIT_STRICT=true git commit --amend -m "[AI:codex] docs(scope): codex validation walkthrough (<SPEC>)"
```

Negative check — confirm the gate rejects a non-conforming message under strict mode:

```bash
# Expected: REJECTED (no [AI:tool] prefix on an AI-traced commit, no conventional type)
AIDA_COMMIT_STRICT=true git commit --amend -m "wip codex test"
```

Under `AIDA_COMMIT_STRICT=true` the malformed message exits non-zero and the commit is
refused; the well-formed message above is accepted. (Bypass in a genuine emergency with
`git commit --no-verify`.)

---

## Step 8 — Ship and confirm the auto-bump (optional, full-loop)

To validate the complete ship loop, push and open a PR, then let the merge promote the
spec:

```bash
aida pr ship
```

`aida pr ship` watches CI, squash-merges the green-and-reviewed PR, runs the post-merge
`aida pull`, and the auto-bump scanner promotes the trailered `<SPEC>` to `Completed`.
Confirm:

```bash
aida show <SPEC>      # status: Completed
```

> Use `aida pull` (not raw `git pull`) after a merge — the `Done → Completed` auto-bump
> lives in `aida pull`'s sync leg. Raw `git pull` strands the spec at `Done` (recover with
> `aida db reconcile-status --spec <SPEC>`).

If you used a throwaway spec, reject or archive it when done:

```bash
aida edit <SPEC> --status rejected   # or: aida archive <SPEC>
```

---

## Step 9 — Negative-path checks (required for a real migration)

A migration validation is not complete until the failure and boundary paths are confirmed.
These are the ones EPIC-0419's acceptance calls out.

### 9a — Blocked decision routes to the punt → advisor → human tier

A Codex agent that hits a design fork it cannot safely resolve must **punt**, not guess.
From inside the session:

```text
post_punt({spec_id: "<SPEC>", detail: "Ambiguous: <describe the fork>", category: "design-fork"})
update_requirement({id: "<SPEC>", status: "needs-attention"})
```

`post_punt` appends a record to `.aida/punts.jsonl` but does **not** change spec status —
pair it with the `update_requirement` flip above. Verify the punt is visible to the rest of
the system:

```bash
aida punts list          # or MCP list_punts({})
aida show <SPEC>         # status: Needs Attention
```

This confirms a blocked Codex decision lands on the same punt ledger a Claude punt would,
and routes to the same advisor → human escalation tier (STORY-306). Resolve it
(`resolve_punt`) or escalate it (`escalate_punt`) to close the loop.

### 9b — Session-communication gap: there is no Codex `defer`

The single most important capability that does **not** port from Claude Code is the hook
`defer` primitive. In Claude Code headless flows, a `PreToolUse` hook can defer a pending
tool call, hand the decision to an external authority, and resume the *same* session. Codex
CLI does not document an equivalent hook-level `defer` — its model is approve/deny at the
point of execution, then continue or stop.

Practical consequence for a migrating team: a flow that relied on Claude's
`permissionDecision: "defer"` plus an external resume loop must be re-expressed for Codex as
**punt → external decision → new Codex run after approval**, not an in-session resume.
Confirm your team's headless approval flows do not assume in-session deferral on Codex. The
full mapping table (`allow`/`deny`/`ask`/`defer`, input/output rewriting) is in
`docs/agents/porting-claude-code-to-codex.md`; the cross-agent pause/abort/defer semantics
are in `docs/agents/session-communication.md`. Do not assume a later hook can ask whether to
continue after an earlier hook has halted the run.

### 9c — Sandbox posture is explicit, not inherited

Codex's sandbox and approval profile is its own — it does not inherit a Claude sandbox
profile, and the two cover different surfaces. The supervised launcher keeps the unsafe
autonomous posture explicit:

```bash
aida agent new codex --spec <SPEC> --role implementer --bypass-sandbox
```

`--bypass-sandbox` passes Codex's `--dangerously-bypass-approvals-and-sandbox` and is **not**
the interactive default. Confirm the migrating team's expected posture: the default is a
prompting/sandboxed Codex; the bypass is a single explicit opt-in, never a silently-baked
default. Validate that a command your policy should block is actually blocked under the
sandboxed (non-bypass) profile before trusting an unattended Codex drain.

### 9d — Capture gaps as follow-up specs, not loose notes

Per EPIC-0419's acceptance, anywhere the real surface did not match the intended flow,
**file a follow-up spec** rather than leaving an unresolved note in this doc. From inside the
session:

```text
file_finding({
  title: "Codex validation gap: <short description>",
  description: "<what broke, expected vs actual, repro>",
  source: "from-implementer:<SPEC>",
  kind: "doc-drift"
})
```

`file_finding` is for **triageable items only** (a real bug or task) — not session journals
or "phase complete" checkpoints (those belong in `add_comment` or the session manifest).

---

## Troubleshooting

| Step | Symptom | Cause / fix |
|---|---|---|
| 1 | `codex mcp list` shows no `aida` row | Project-local `.codex/config.toml` not picked up — start Codex from the project root, or register manually: `codex mcp add aida -- aida mcp-serve`. |
| 1 | Row present but `Status: disabled` | `command` points at a missing binary — edit `.codex/config.toml` `command` to the absolute `aida` path. |
| 2 | Tools missing inside the session | Codex not started from the project root (`aida mcp-serve` discovered the wrong / no project). Restart with `codex --cd /path/to/project`. |
| 2 | `tests/test_mcp_doc_consistency.sh` fails on tool count | A doc drifted from the live `tools/list`. Trust `tools/list`, fix the doc, file a finding (Step 9d). |
| 3 | `add_requirement` rejects the `type` | `type` must be a lowercase taxonomy value (`task`, `bug`, `story`, …). Do not invent `SPEC-N`. |
| 3 | `claim_task` succeeds for two agents on one spec | Known TOCTOU race (TASK-438). Check `list_active_leases()` first; avoid concurrent same-spec claims until it closes. |
| 5 | `update_requirement` refuses `approved`/`planned`/`completed` | Gate working as designed — those are advisor-only / merge-driven. Use `draft`/`in-progress`/`done`. |
| 5 | `add_comment` errors on a `body` arg | The arg is `text`, not `body`. |
| 6 | CLI doesn't show the MCP write | Cache staleness — run `aida cache status`; a stale MCP-created spec is the BUG-310 class (now fixed). Re-read; if it persists, file a finding. |
| 7 | Trace gate didn't reject a malformed message | The gate is permissive by default. Set `AIDA_COMMIT_STRICT=true` to enforce. |
| 8 | Spec stuck at `Done` after merge | You used raw `git pull`. The auto-bump lives in `aida pull`. Recover: `aida db reconcile-status --spec <SPEC>`. |
| any | MCP responses look stale after a rebuild | `aida mcp-serve` self-respawns on a newer on-disk `aida --version` or build SHA. If still stale, kill the agent's `aida mcp-serve` process and let Codex respawn it. |
| 9b | Headless approval flow hangs on Codex | No Codex `defer` — re-express as punt → external decision → new Codex run. See `docs/agents/porting-claude-code-to-codex.md`. |

---

## What a green run proves

A clean pass of Steps 1-9 demonstrates the EPIC-0419 acceptance for a Codex migration:

- A Codex session discovers AIDA's MCP tools out of the box from the scaffolded
  `.codex/config.toml` (Steps 1-2).
- Codex reads an owning spec, claims it, makes a traced edit, runs targeted tests, and
  writes coordination state back through MCP (Steps 3-5).
- The same state is visible through the AIDA CLI after the MCP writes — Codex operates on
  the shared substrate, not a private shadow (Step 6).
- The commit trailer + trace gate accept the `[AI:codex]` / `(SPEC)` convention and enforce
  it under strict mode (Step 7), and the auto-bump promotes the spec on merge (Step 8).
- The negative paths hold: blocked decisions punt to the advisor/human tier, the Codex
  session-communication gaps (no `defer`) are accounted for, and the sandbox posture is an
  explicit opt-in (Step 9).

Anywhere the real surface diverged from the intended flow, the divergence was captured as a
finding/follow-up spec (Step 9d) rather than left as an unresolved note — which is itself
part of the acceptance.

trace:TASK-0425 | ai:claude
