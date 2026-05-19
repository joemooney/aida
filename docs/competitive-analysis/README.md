# AIDA competitive analysis

The AI/dev-tooling landscape moves fast. This directory is the **living record** of where AIDA sits in it — kept current, time-stamped, and honest about staleness.

Two kinds of document live here:

- **Dated landscape snapshots** — `YYYY-MM-DD-<slug>.md`. A point-in-time scan of the whole field. The foundational one is [2026-03-17-landscape-scan.md](2026-03-17-landscape-scan.md).
- **Per-topic living files** — `<topic>.md`. A single neighbour or ecosystem that moves fast enough to warrant its own continuously-updated page, with dated entries appended inside it.

## Sibling: `docs/positioning/`

[`docs/positioning/`](../positioning/) answers *"should I use AIDA or X?"* — sharp, paired `vs-X.md` comparisons. This directory is the wider-angle, time-stamped view: the whole landscape, the ecosystem AIDA runs inside, and where it is heading. Positioning is the argument; competitive-analysis is the evidence and the watch-list.

## The living-doc rule

Competitive intelligence rots — a snapshot is only trustworthy at its date stamp.

- Every file carries a **`Last updated`** line.
- New observations are **appended as dated entries** — never silently overwrite an old claim; the diff between dates is itself signal.
- When a neighbour tool ships something material, or an AIDA release shifts a comparison, update the relevant file and bump its date.
- Anyone who notices drift fixes it — open the file, edit, commit with a `docs(competitive):` scope.

This is **best-effort calibration**, not auditable market research. Pricing, feature parity, and roadmap claims about other tools should be re-verified against the vendor's own docs before any high-stakes decision.

## Index

| File | Scope |
|---|---|
| [2026-03-17-landscape-scan.md](2026-03-17-landscape-scan.md) | Foundational broad scan — PM tools adding AI, AI code editors, requirements tools, git-native trackers, the MCP ecosystem; feature matrix; AIDA's honest strengths and weaknesses. |
| [claude-code-plugin-ecosystem.md](claude-code-plugin-ecosystem.md) | Claude Code's own plugin & marketplace ecosystem — the substrate AIDA runs on, and an unexploited discoverability channel for AIDA itself. |

## See also

- [docs/positioning/](../positioning/) — the focused "vs X" comparisons.
- [OVERVIEW.md](../../OVERVIEW.md) — AIDA's vision and defensible niche.
