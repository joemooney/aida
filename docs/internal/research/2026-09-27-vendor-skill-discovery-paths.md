# Real vendor skill discovery: which directories Codex and Antigravity actually read

**Date:** 2026-09-27 · **Spec:** TASK-1521 (child C of BUG-1639) · **Status:** verification complete, three of four questions settled by direct observation
**Consumer:** TASK-1519 (migrating existing Codex/Antigravity installs to `.agents/skills`) — its legacy-pack behaviour depends on these answers.

**Questions** (carried over from the BUG-1639 sketch's open items 2 and 3):

1. Do Codex and Antigravity still read the legacy `.codex/skills/` and `.antigravity/skills/` packs?
2. When the same `aida-*` name exists in two packs, which copy wins — and specifically, which `aida-orchestrate` body (portable vs the Claude master) does each vendor pick?
3. Does Codex read `.claude/skills/` at all?
4. Is an AIDA-delivered skill file — YAML frontmatter followed by the `<!-- AIDA Generated: … -->` header — actually discovered?

**Method.** No vendor agent session was started and no model call was made, so none of this cost API quota. For Codex the probe is `codex debug prompt-input`, which renders the model-visible prompt input as JSON locally; its `<skills_instructions>` block names every skill root Codex resolved and every skill it indexed, which is exactly the discovery result. Probe projects were disposable git repos in the session scratchpad, and `CODEX_HOME` pointed at a scratch directory so nothing under `~/.codex` was read or written. For Antigravity, `agy` has no equivalent dry-run renderer, so the evidence is (a) the customization documentation the vendor ships *inside* the `agy` binary, (b) a literal-string census of that binary, and (c) direct observation of the `Available skills:` block in `agy`'s own persisted conversation stores under `~/.gemini/antigravity-cli/conversations/` — i.e. what real past sessions were actually told. Nothing under a real vendor install directory was modified.

**Versions probed.** `codex-cli 0.157.1` (`@openai/codex-linux-x64`, musl native binary, installed 2026-09-26) and `agy 1.2.12` (installed 2026-09-26 23:38). The prior matrix on BUG-1639 was run against Codex 0.157.0 and agy 1.2.11.

---

## Answers

| Question | Answer | Evidence class |
|---|---|---|
| Codex still reads `.codex/skills/` | **Yes** | Direct observation, 0.157.1 |
| Antigravity still reads `.antigravity/skills/` | **No** — and as far as 1.2.12 can be read, it never did | Vendor documentation shipped in the binary + zero-literal census, corroborated by a direct negative observation on an earlier build |
| Codex reads `.claude/skills/` | **No** | Direct observation, 0.157.1 |
| Antigravity reads `.claude/skills/` | **No** | Vendor documentation + one direct negative observation |
| Duplicate `aida-*` name in two Codex-visible packs | **Both are listed**, as two entries under the same `name`, with no dedup and no suppression — even when the bytes are identical | Direct observation, 0.157.1 |
| Which copy a Codex `$name` invocation resolves to | **UNVERIFIED** | See "What remains unverified" |
| Which `aida-orchestrate` body each vendor picks | The **portable** body, necessarily: neither vendor reads `.claude/skills/`, so the Claude master is never visible to them | Follows from the two `.claude/skills` negatives |
| An AIDA-delivered skill file is discovered | **Yes**, both vendors, with `name` and `description` indexed correctly | Direct observation (Codex probe with real scaffolder output; agy's own persisted prompts) |
| Codex follows a symlinked `SKILL.md` | **No**, silently skipped — in `.codex/skills/` as well as `.agents/skills/` | Direct observation, 0.157.1 (confirms the 0.157.0 result) |

## Codex 0.157.1 — direct observation

`codex debug prompt-input`, run in a probe project holding a distinctly-marked skill in each of `.agents/skills/`, `.codex/skills/`, `.antigravity/skills/` and `.claude/skills/`, resolved exactly three skill roots:

```text
- `r0` = <project>/.codex/skills
- `r1` = $CODEX_HOME/skills/.system
- `r2` = <project>/.agents/skills
```

Only the `.agents/skills` and `.codex/skills` probes appeared in the indexed list. The `.antigravity/skills` and `.claude/skills` probes produced no entry at all. The literal `.claude/skills` does not occur anywhere in the Codex native binary.

Further observations from the same probe family:

- **Both legacy and canonical packs are live.** `.codex/skills/` is a first-class project skill root in 0.157.1, not a compatibility shim. `.antigravity/skills/` is not a Codex root.
- **No duplicate handling.** A skill named `aida-probe-dup` present in both `.codex/skills/` and `.agents/skills/` was listed **twice**, once per root, under the same `name`, distinguished only by the root-prefixed path. Nothing in the binary handles a skill-name collision; there is no "shadowed", "ambiguous" or "duplicate" diagnostic string.
- **Link forms.** A symlinked `SKILL.md` is silently skipped in both packs. A hard link is indexed. A symlinked skill *directory* containing a regular `SKILL.md` is indexed.
- **Root walking.** Discovery walks up from the cwd to the repository root: from `<project>/sub/deep` both project roots were still resolved.
- **Only `.agents`.** `_agents/skills/` and `.agent/skills/` — both of which Antigravity accepts — are **not** Codex roots. `.agents/skills/` is the only shared spelling.

### The AIDA-delivered pack, measured

A real scaffolder pack was produced by running `aida scaffold apply --project-root` into a disposable tempdir project (23 portable skills), then copied into both `.agents/skills/` and `.codex/skills/` of a probe project that also carried the full 56-skill `.claude/skills/` pack. Codex indexed **51** skills: 23 from `.agents/skills`, 23 from `.codex/skills`, 5 built-ins, and **0** from `.claude/skills`. Every one of the 23 `aida-*` names appeared twice.

So AIDA's generated file form is fine — frontmatter at byte 0 with the `<!-- AIDA Generated: … -->` marker placed *after* the frontmatter parses cleanly, and each skill's `name` and `description` are indexed as written. The defect is not the file; it is the second pack.

## Antigravity (agy) 1.2.12

The `agy` binary embeds the customization documentation it serves to its own agent. On discovery locations it is explicit:

> 1. **Workspace Customizations** (Project-Specific): Path: `.agents/` (or `.agent/`, `_agents/`, `_agent/`) at the root of your project. … The agent walks from your current working directory up to the repository root … 2. **Directory & Project Rules** (Hierarchical) … 3. **Global Configuration** (Machine-Local): Path: `~/.gemini/config/`

and on collisions:

> When multiple customizations are discovered, they are loaded and applied in a specific order. If there are naming conflicts (e.g., two skills with the same name), the higher-priority customization overrides the lower-priority one. The priority order (from highest to lowest) is: 1. **Workspace Project** … 2. **Declared Configurations** (`skills.json` / `plugins.json`) … 3. **Global Discovery** (`~/.gemini/config/`) … 4. **Built-in Customizations** … 5. **Global Declared Configurations**.

`.antigravity/` and `.claude/` are not on that list. A literal census of the 1.2.12 binary agrees: `.agents/skills` occurs 13 times, `.antigravity/skills` **zero** times, `.claude/skills` **zero**, `.codex/skills` **zero**. Unlike Codex, Antigravity *does* override on a name collision rather than list both — but only among the roots it actually reads, so no AIDA pack can collide with another AIDA pack there.

The load-bearing direct observation comes from `agy`'s own persisted prompts. Across all 44 conversation stores under `~/.gemini/antigravity-cli/conversations/`, every entry in the `Available skills:` block resolves to one of three roots: `/home/joe/ai/aida/.agents/skills` (248 entries), `/home/joe/ai/market-watcher/.agents/skills` (116), and the bundled `~/.gemini/antigravity-cli/builtin/skills` (139). Not one entry came from a `.antigravity/skills`, `.codex/skills` or `.claude/skills` path.

That is a genuine negative, not an absence of opportunity. At the time of the 2026-09-08 23:45 session in `market-watcher`, that project had at least 15 AIDA skills sitting in `.antigravity/skills/`, at least 15 in `.codex/skills/`, one in `.claude/skills/`, and 37 in `.agents/skills/`. Antigravity listed only the `.agents/skills/` ones.

**Version caveat, stated plainly.** That `market-watcher` session predates the 1.2.12 install, so it proves the negative for the build running on 2026-09-08, not for 1.2.12 specifically. For 1.2.12 the `.agents/skills` **positive** is directly observed — sessions in this repo through 2026-09-27 05:24 list its 26-skill `.agents/skills` pack by name and path — but this repo has no `.antigravity/skills` for a 1.2.12 session to ignore. The 1.2.12 negative therefore rests on the binary's own documentation plus the zero-literal census. Given that `.antigravity/skills` appears nowhere in the shipped binary in any form, the most likely history is that it was **never** an Antigravity discovery surface and AIDA invented it by analogy with `.codex/skills`.

## What this contradicts

1. **BUG-1639 advisor decision (b) is inverted for Codex.** It reasoned that because legacy discovery was ungrounded, the fail-safe move was to keep maintaining an existing real `.codex/skills` from the same derived inventory, since "keeping it identical is strictly safer than leaving stale duplicates." Measured, the opposite holds: Codex reads both roots, so a maintained legacy pack doubles every AIDA skill in the Codex skills list — 46 entries where 23 are intended, two entries per name that the model cannot tell apart. Making the packs identical does not remove the duplicate, it makes the duplicate *undetectable*. For Codex the safe state is exactly one pack.
2. **`.antigravity/skills` buys nothing.** AIDA has been scaffolding and refreshing roughly 22 files per install into a directory that the current Antigravity CLI does not read, and shows no sign of ever having read.
3. **Docs still present the legacy packs as the discovery surface.** `docs/agents/antigravity-mcp-setup.md` (~L210) says `.antigravity/skills/` is scaffolded by `aida init` and kept level by refresh; `docs/agents/codex-mcp-setup.md` (~L336, ~L358) presents `.codex/skills/` as the Codex pack. Both files are generated mirrors of masters under `aida-core/templates/docs/agents/`, so correcting them is template work, not a doc edit — TASK-1520-shaped. `docs/agents/aida-mcp-install-matrix.md` is likewise a generated mirror, which is why this note, and not that file, carries the finding.
4. **The symlink guard is still unimplemented, and now needs wider scope.** The BUG-1639 sketch's `scaffold-drift/agents-skill-symlink` doctor finding does not exist in the code yet. Codex skips a symlinked `SKILL.md` in `.codex/skills/` exactly as it does in `.agents/skills/`, so for as long as a legacy pack is maintained the guard has to cover it too.
5. **The dev-activated `aida` binary predates slice 1.** The `aida` on PATH at the time of this probe (0.15.0, sha `7c6faed690`) still writes `.codex/skills` and `.antigravity/skills` and no `.agents/skills`: a `scaffold apply` run with it installs only legacy packs. Reported, not fixed — rebuilding is a separate action.

## Consequences for TASK-1519

- Retiring `.antigravity/skills` is safe on this evidence: nothing reads it. It can be frozen or removed without a vendor-behaviour risk.
- Retiring `.codex/skills` is not merely safe, it is the *fix*. Leaving it maintained is the duplicate-listing defect, so "freeze and report, with a manual removal hint" is the weakest acceptable outcome and pruning unedited AIDA-headed `aida-*` directories is the right one. The pruning safety rules on TASK-1519 (dry-run first, only unedited AIDA-headed `aida-*` dirs, never anything unverified) are unaffected by this finding.
- Duplicate `aida-*` names across `.claude/skills` and `.agents/skills` are a non-issue for both non-Claude vendors, because neither reads `.claude/skills`. That also removes one of the stated risks from the carved-out "single-location" option: Codex and Antigravity would see only the `.agents/skills` copy either way.
- No migration logic needs to reason about which `aida-orchestrate` body a vendor picks. Codex and Antigravity can only see the portable one.

## What remains unverified

- **Which copy a Codex `$name` invocation resolves to** when two roots offer the same name. The listing shows both; `codex debug prompt-input` does not expand `$name` mentions, and the root numbering is assignment order, not precedence. What would settle it: a live Codex session in a probe project with the same name in `.codex/skills/` and `.agents/skills/` carrying distinguishable bodies, invoking `$aida-probe-dup` and reading which path it opened. This matters only if a legacy pack is kept; if TASK-1519 prunes it, the question is moot, which is another argument for pruning.
- **`.antigravity/skills` under agy 1.2.12 specifically** (see the version caveat above). What would settle it: one cheap `agy` turn in a tempdir project whose only AIDA skill sits in `.antigravity/skills/`, then grep that conversation's store under `~/.gemini/antigravity-cli/conversations/` for the `Available skills:` block. This is an operator action, since it starts a vendor session.
- **Older supported vendor versions.** Everything here is 0.157.1 and 1.2.12. Whether some older Antigravity build read `.antigravity/skills` is unknown and, for a migration that only ever needs to stop *writing* a directory, does not need to be known.
