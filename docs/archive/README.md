# Archived Documentation

These documents describe earlier designs, abandoned features, or point-in-time
evaluations. They are kept for historical reference but **do not reflect the
current state of AIDA**. For current docs, see:

- Project root: `README.md`, `OVERVIEW.md`, `CLAUDE.md`
- `docs/getting-started.md`, `docs/admin-guide.md`, `docs/user-guide.md`
- `docs/plans/` — design docs for individual features (chronological)

## What's here, and why it was archived

### Removed surfaces (pre-2026-05-02 prune)
- `unified-gui-plan.md`, `unified-storage-architecture.md` — designs for the
  egui-based desktop / WASM clients (`aida-desktop`, `aida-web`), which were
  extracted to a separate repo and removed from the main workspace
- `DEVELOPER_GUIDE.md` / `.html` — large dev guide whose architecture sections
  describe the removed desktop app

### Pre-shipping design docs (kept as historical reference)
- `AI_INTEGRATION_DESIGN.md`, `EXTERNAL_INTEGRATION_ARCHITECTURE.md`,
  `RELATIONSHIP_DESIGN.md`, `SPRINT_EPIC_DESIGN.md` — early design docs for
  features that have since shipped; the code is the source of truth now

### Old project planning / status docs (root-level cruft)
- `FINAL_RECOMMENDATION.md`, `FINAL_STATUS.md`, `PLAN.md`,
  `IMPLEMENTATION_PLAN.md`, `INTEGRATION*.md` (six variants),
  `SIMPLIFIED_INTEGRATION.md`, `SPEC_ID_*.md`, `UUID_SPEC_ID_VERIFICATION.md` —
  early planning artifacts from when AIDA was first being designed; superseded
  by the current docs and by the requirements database itself

### Snapshot evaluations
- `PROJECT_EVALUATION_2026-02-28.md` — point-in-time project evaluation,
  superseded by `docs/competitive-analysis/` and `OVERVIEW.md`

### 2026-10 documentation cleanup
<!-- trace:TASK-1611 | ai:codex+claude -->

The October 2026 cleanup moved dated snapshots, superseded audits, briefs,
and rendered exports out of the maintained tree. The
[cleanup record](2026-10-cleanup/README.md) states the archive policy and
the evidence, and its [`inventory.tsv`](2026-10-cleanup/inventory.tsv) lists
every tracked document with its disposition and its path before the move.

| Directory | What it holds |
|-----------|---------------|
| [`competitive-analysis/`](competitive-analysis/) | Dated market snapshots, category summaries, and the 2026-05-29 Claude Code 2.1.154 decomposition. The living roster, ecosystem watch, and signals stay in `docs/competitive-analysis/`. |
| [`positioning/`](positioning/) | The per-neighbour "AIDA vs X" comparisons (written May–July 2026, last edited by September 2026) and `AIDA_IN_MY_OWN_WORDS.md`. The framing documents stay in `docs/positioning/`. |
| [`research/`](research/) | Research notes, landscape scans, and measurement write-ups from June–July 2026. |
| [`spikes/`](spikes/) | Spike reports from May–June 2026 (later spikes stay in `docs/spikes/`). |
| [`briefs/`](briefs/) | Handoff and second-opinion briefs from 2026-05-29. |
| [`audits/`](audits/) | One-off audits and analyses: the Claude-to-Codex porting analysis and the 2026-05-29 skill-catalog audit. |
| [`designs/`](designs/) | Two spike design notes (SPIKE-44, SPIKE-90) formerly filed under `docs/architecture/`. |
| [`reviews/`](reviews/) | The July 2026 flow smoke-test log. |
| [`testimonials/`](testimonials/) | LLM field reports from September 2026. |
| [`rendered/`](rendered/) | Generated HTML exports (admin guide, slideshow, demo deck, AI integration report). The user-guide HTML stays in `docs/`, where `aida user-guide` opens it. |

Monthly "what's new" notes moved to [`docs/release-notes/`](../release-notes/)
rather than here.
