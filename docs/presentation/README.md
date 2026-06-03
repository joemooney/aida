# AIDA presentations

Audience-targeted slide decks for AIDA, authored in [Marp](https://marp.app/)
markdown (one `# slide` per `---`-separated section; speaker notes live in HTML
comments under each slide). Pick the deck for your audience:

| Deck | Audience | What it covers |
|---|---|---|
| [`aida-executive-briefing.md`](aida-executive-briefing.md) | Executives / leadership / investors | The problem, the wedge, the moat, the proof, the ask — outcome-first, ~10 min, almost no CLI. |
| [`aida-developer-deep-dive.md`](aida-developer-deep-dive.md) | Engineers (understand / extend / trust the internals) | Crates, git-canonical storage + cache, distributed IDs (HLC/node/dispenser), the graph + history, the six-phase orchestrator, shelve-and-advance + escalation, MCP, traceability. |
| [`aida-administrator-guide.md`](aida-administrator-guide.md) | Whoever stands up / operates AIDA for a team | Init variants, fresh-clone auto-attach, the shared/local split, multi-node + queue identity, `config.toml` sections, maintenance (cache / db sync / reconcile / doctor / telemetry), the security checklist. |
| [`aida-user-walkthrough.md`](aida-user-walkthrough.md) | Developers using AIDA day to day | Requirement-first habit, the daily loop, filing / finding / picking up work, the autonomous drain + autonomy modes, traces + commit convention, the lifecycle, skills, the TUI, a cheat sheet. |

There is also a semi-technical **live-demo** deck —
[`2026-06-management-demo.md`](2026-06-management-demo.md) — whose argument is a
live terminal demo (runbook: [`demo-runbook.md`](demo-runbook.md)); the slides
are the spine + fallback.

## Rendering

The decks need no build to read (they're plain markdown). To render them all to
HTML **plus a browsable `index.html`**, run the build script:

```bash
./docs/presentation/build.sh           # → docs/presentation/build/ (gitignored)
PDF=1 ./docs/presentation/build.sh     # also emit a .pdf per deck
./docs/presentation/build.sh /tmp/out  # render to a custom dir
```

Then open `docs/presentation/build/index.html`. The script renders every
`aida-*.md` deck here (and the management live-demo deck when present) and
generates the index that links them.

To render a single deck by hand:

```bash
# HTML
npx --yes @marp-team/marp-cli@latest docs/presentation/aida-executive-briefing.md -o aida-executive-briefing.html
# PDF
npx --yes @marp-team/marp-cli@latest docs/presentation/aida-executive-briefing.md --pdf -o aida-executive-briefing.pdf
```

(The exact `RENDER:` command for each deck is also in an HTML comment at the top
of that deck.) Generated HTML/PDF and the `build/` dir are gitignored — they're
artifacts; commit the markdown, not the renders.

## Keeping them current

The decks quote live numbers (spec counts, releases, commits) and version
strings. Refresh before presenting:

```bash
aida list --all                       # total specs
aida list --all --status completed    # completed
git tag | grep -c '^v'                # releases
git rev-list --count HEAD             # commits
grep -m1 '^version' Cargo.toml        # version string
```

Source material the decks are built from: `OVERVIEW.md`, `CLAUDE.md`,
`docs/autonomous-drain.md`, `docs/lifecycle.md`, `docs/storage-modes.md`, and
`docs/positioning/`.
