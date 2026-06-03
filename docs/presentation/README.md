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

## Casts (recorded terminal sessions)

`build.sh` also embeds an **in-browser [asciinema](https://asciinema.org)
player** for every `.cast` it finds in `docs/casts/` (curated, tracked) and
`docs/presentation/` (e.g. a live-demo cast) — generating a `casts.html` gallery
and a **Casts** card on the index. Clones play them inline with **no asciinema
install**.

To add a cast:

```bash
# record straight into the tracked location
aida --asciinema --cast-out docs/casts/<name>.cast queue work <SPEC> --auto-complete
git add docs/casts/<name>.cast      # docs/casts/ is outside the .aida/* gitignore
./docs/presentation/build.sh        # regenerate — the cast now appears in the index
```

The player JS/CSS are **vendored** in `vendor/` (Apache-2.0) so casts play
offline / air-gapped; the script falls back to a CDN with a warning if the
vendored assets are missing. **Review casts for secrets before committing** —
asciinema records whatever was on screen, and it's permanent in history.

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
