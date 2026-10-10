# Claude Code plugin & marketplace ecosystem

*Last updated: 2026-05-19*

**Scope.** Claude Code is the substrate AIDA runs on — the TUI wraps Claude Code sessions (EPIC-26), and `aida init` scaffolds Claude Code skills, commands, hooks, and an MCP server. Claude Code's own plugin ecosystem is therefore both a *complement* to watch and a potential *distribution channel* for AIDA. This file tracks it, newest entry first.

---

## 2026-05-19 — the official `claude-code-setup` plugin

### What it is (verified)

Anthropic ships an official plugin, **`claude-code-setup`**, in the official **`claude-plugins-official`** marketplace (`github.com/anthropics/claude-plugins-official`). Install:

```
/plugin install claude-code-setup@claude-plugins-official
```

It contains one skill — **`claude-automation-recommender`** — which scans a codebase and recommends MCP servers, skills, hooks, subagents, and slash commands the project could adopt. Added to the marketplace 2026-01-16; v1.0.0; authored by Anthropic.

Verified 2026-05-19 against the Anthropic GitHub org and the Claude Code docs, and by installing it. It surfaced via a social-media post whose framing ("quietly released", "then sets everything up step-by-step for you") overstates it: the plugin is **read-only** — it *recommends*, it does not scaffold or modify files. The plugin, marketplace, and install command are all real and correct; the "auto-setup" claim is not.

### Why it matters to AIDA

**1. It validates the `aida init` thesis — and stops one step short of it.** Anthropic's own tooling now tells a developer their project *should* have hooks, skills, MCP servers, and subagents. That is exactly the gap `aida init` fills — except `aida init` *installs a curated, opinionated set* and wires it to a requirement graph. The official plugin ends at advice; AIDA does the scaffolding and gives the scaffolding a purpose (the spec graph, trace comments, queue, MCP server). The neighbour is a **recommender**; AIDA is an **implementer**. That is a clean, defensible seam — and a usable positioning sentence: *"Claude Code's own plugin will tell you to set this up; AIDA is what you set up."*

**2. The marketplace is a distribution channel AIDA does not yet use.** `claude-plugins-official` is the default marketplace; users discover plugins by running `/plugin`. AIDA currently reaches users only through `cargo install` / release tarballs plus a manual `aida init`. A Claude Code *plugin* form of AIDA — bundling the skills, commands, hooks, and MCP server `aida init` already scaffolds — would be discoverable to every Claude Code user browsing `/plugin`, with zero prior awareness of AIDA. That is the strongest no-prior-knowledge funnel available, and it is currently unexploited. (Whether that means a third-party marketplace entry or a submission to the official one is an open question.)

**3. The plugin primitive set matches AIDA's scaffold set exactly.** Hooks, skills, MCP servers, subagents, slash commands — the primitives the recommender names are the primitives `aida init` writes. AIDA already *is* "a plugin's worth of content," delivered through a different mechanism. The packaging gap is mechanical, not conceptual.

### Watch-items

- Does Anthropic extend `claude-code-setup` from *recommend* to *apply*? If it starts scaffolding, the seam narrows — re-evaluate the `aida init` differentiation (the graph / traces / queue still stand; the raw scaffolding does not).
- Does an official "requirements" / "spec" / "project memory" plugin appear? That would be a direct neighbour — file a `vs-*.md` in `docs/positioning/`.
- Marketplace submission terms for `claude-plugins-official` — relevant only if AIDA pursues channel (2).

### Open question for AIDA's roadmap

Should AIDA ship as a Claude Code plugin (or be submitted to a marketplace)? That is a strategic/distribution call, not a decision for this doc — captured here as a discoverability data point. If pursued, it warrants its own SPIKE.
