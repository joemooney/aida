# `/aida-plan` (+ `docs/plans/`) vs `/ultraplan`

*Last updated: 2026-05-13 — /ultraplan terms (3 free uses, then Max-quota or billed; 90-minute approval window before the cloud session terminates) captured live from the launch prompt 2026-05-13. Re-verify against [code.claude.com/docs/en/claude-code-on-the-web](https://code.claude.com/docs/en/claude-code-on-the-web) before relying on the cost line.*

The TL;DR: **`/ultraplan` drafts the prose. AIDA structures, verifies, and executes it. Compose them — they're not substitutes.**

This is the sister doc to [vs-ultrareview.md](vs-ultrareview.md). The same thesis applies twice: the Claude Code cloud `/ultra*` family is great at LLM-heavy generation; AIDA owns the surrounding agent-collaboration layer (graph, IDs, traces, queue, persistence, MCP). The skeptic's question — *"I use /ultraplan and it's great, why do I need AIDA?"* — has a sharper answer once you've used `/ultraplan` twice and lost the chat history both times.

---

## Different scopes, complementary

| | `/aida-plan` + `docs/plans/` | `/ultraplan` |
|---|---|---|
| **Engine** | Local Claude session orchestrating req decomposition + plan file authoring | Cloud-based LLM generating dense, file-by-file plan documents |
| **Cost** | $0 — uses the active Claude session | 3 free uses then Max quota or billed per use (verify current terms); 90-minute approval window before timeout |
| **Output shape** | Child requirement decomposition + design-decision comments + optional plan file under `docs/plans/` | Single dense markdown document: approach, file-by-file, risks, verification script, followups |
| **Anchoring** | Symbol refs preferred (planned via TASK-93 — survives edits); plan stored as a git-tracked file linked in the req graph | Line refs (drift fast — 2 of 8 line refs were already stale on day-of-generation in the STORY-86 case study) |
| **Persistence** | First-class file in repo, linked from STORY/EPIC, queueable | Chat output — evaporates unless manually saved |
| **Integration** | Plans become part of git history, referenced by Related Requirements, surfaced in session manifests (planned TASK-95) | Standalone; manual copy/paste/save to anywhere persistent |
| **Iteration** | Edit the plan file, recommit, diff is visible | Re-run `/ultraplan` for a fresh document; old draft is gone |
| **Offline** | Works fully offline | Requires cloud round-trip |
| **Driven by** | The requirement graph — knows children, parents, sibling specs, trace links | The diff between current code and the user's prompt — no req-graph awareness |
| **Launchable from** | Anywhere a Claude session can run | User-triggered only (Claude Code controls); approval window forces human-in-the-loop |

---

## What `/ultraplan` does that bare AIDA can't (today)

The STORY-86 case study (`docs/plans/2026-05-13-story-86-done-status.md` — saved from a real `/ultraplan` cloud session) demonstrates the genuine depth `/ultraplan` brings:

| Strength | What it looked like in the STORY-86 plan |
|---|---|
| **Executive-summary discipline** | One paragraph stating the whole strategy before any details — readable in 30 seconds |
| **ASCII state-machine + control-flow diagrams** | The Draft → ... → Done → Completed lifecycle and the auto-bump helper's control flow rendered in ~10 lines of art |
| **File-by-file changes with line refs** | ~25 files enumerated, each with the specific edit shape |
| **"Critical Files" enumeration** | Must-touch paths surfaced separately from prose — at-a-glance blast radius |
| **"Reusable helpers — do not reimplement"** | Explicit list (`extract_spec_ids_from_commit`, `record_role_activity`, `Storage::update_atomically`) so the implementer doesn't re-invent parsers |
| **Risks + gotchas with mitigations** | 8 numbered: serde downgrade compat, force-push rewrites, first-pull-on-new-clone, etc. |
| **Decision callouts** | "Where does X live? Both. Here's why." — resolved decisions, not open-question lists |
| **Complexity estimate** | LOC + commits + risk level — sets the implementer's clock |
| **Test cases by name** | Specific `fn` names, not "add tests" |
| **Verification script** | ~30-line end-to-end bash smoke (positive + negative) — definition of done as executable |
| **Followups (defer)** | Explicit "file these at completion time" list — lighter than child reqs, no scope creep |
| **Order-matters annotation** | "Do edits top-to-bottom so each commit builds clean" |

AIDA's `/aida-plan` skill is more decomposition-oriented (vertical-slice child reqs, design-decision comments) and doesn't generate the dense file-by-file prose. That's a real gap when the work warrants it.

---

## What AIDA does that `/ultraplan` doesn't

The skeptic's argument starts to crack here. `/ultraplan`'s output, sitting alone in a chat window, has no answer to any of these:

- **Persistence.** Plans evaporate when the cloud session times out (90 minutes) or the chat tab closes. AIDA plans live in `docs/plans/YYYY-MM-DD-<slug>.md`, git-tracked, referenced by `Related Requirements`.
- **Graph membership.** AIDA plans are pinned to their target SPEC-ID via `aida comment add`, surfaceable from `aida show STORY-86`, queryable through the MCP server. `/ultraplan` doesn't know what your SPEC IDs are.
- **Stable identifiers.** AIDA's symbol refs survive edits; `/ultraplan`'s line refs go stale within a day. (STORY-86 plan: 2 of 8 refs drifted within hours. The `DbCommand::Sync` callsite ref was off by ~19,000 lines.)
- **Verifiability.** `aida plan verify` (TASK-93, planned) re-anchors stale line refs, validates file paths, and lints structural sections — no equivalent in `/ultraplan`'s output.
- **Queue + session integration.** `aida queue work <SPEC>` will pre-populate session manifests from matching `docs/plans/` files (TASK-95, planned). The fresh implementer session opens with the plan already loaded as context, not as something to grep for.
- **Followups auto-extraction.** `aida queue done` will parse the plan's Followups section and offer to file each as TASKs under the parent spec (TASK-96, planned). `/ultraplan`'s followups list is inert markdown.
- **MCP exposure.** AIDA plans are reachable through the MCP server alongside other reqs — agents in *other* sessions can find them. `/ultraplan`'s output isn't visible to anything outside the originating chat.
- **Cost stability.** `/ultraplan`'s terms shifted within months of launch (initially-free → 3 free uses then Max-quota or billed). AIDA's local-first cost stays $0 regardless of vendor pricing changes.
- **Independence from approval windows.** A 90-minute timeout on a planning artifact is a real workflow constraint; missed it once already on STORY-86. AIDA plans never expire.
- **Works without internet.** Field-work, transit, flaky connections — `/ultraplan` is unreachable; AIDA plans aren't.

---

## The complementary workflow

For **complex/risky work** where the dense brief is worth the cloud round-trip:

1. **Run `/ultraplan`** with the target spec ID + acceptance criteria. Pay the cloud round-trip for the file-by-file depth.
2. **Save the output** to `docs/plans/YYYY-MM-DD-<slug>.md` (AIDA convention). Commit it.
3. **`aida comment add <SPEC-ID>`** with a one-liner pointing at the plan file. The plan becomes graph-reachable.
4. **`aida queue add <SPEC-ID> --for implementer`** routes the work.
5. **`aida queue work <SPEC-ID>`** (with TASK-95: auto-pre-populates the session manifest from the plan file) launches a fresh implementer session that opens with the plan as context.
6. **Implementer executes**; `aida plan verify <file>` (TASK-93) re-anchors any stale line refs as the code shifts under the plan.
7. **`aida queue done`** (with TASK-96) parses the plan's Followups section, files each as a TASK under the parent — no out-of-scope items get forgotten.

For **routine work** (small feature, well-bounded bug fix):

1. **`/aida-plan`** alone — decompose into child reqs, comment design decisions on the parent. The decomposition often *is* the plan. No need to spend a `/ultraplan` use on it.
2. **Skip the cloud round-trip.** The session that has full context already has enough to execute.

The empirical rule: if the file-by-file list would be longer than ~15 entries, `/ultraplan` earns its keep. Below that, AIDA-only is faster.

---

## What if the skeptic skips AIDA entirely?

The honest answer: **possible, but lossy.**

A team using only `/ultraplan` + `/ultrareview` + manual git workflow can ship code. What they lose:

- **No queryable record** of why a piece of work was undertaken once the chat is gone. Tomorrow's "what was this commit's spec?" has no answer.
- **No spec-to-code traceback.** `git blame` shows who and when; it doesn't show *why* — no `// trace:STORY-86` comment, no `aida show STORY-86` to read the rationale.
- **Brittle lifecycle bookkeeping.** Plans go stale, sessions terminate, status fields drift. `/aida-pickup → /aida-pr → /aida-review`'s atomic close-out doesn't exist.
- **No agent collaboration layer.** A second agent in a separate session has no way to discover what the first agent decided. `/ultraplan`'s output is a single-chat artifact; AIDA's graph is a shared workspace.
- **No MCP exposure.** Editor-resident agents (Cursor, Continue, Aider, IDE Claude extensions) can't see the planning trail.

This is genuinely the trade-off. If your project is a 2-week prototype with one developer and zero handoffs, the loss is mostly hypothetical. If it's a 6-month codebase with multiple contributors (human or agent) and the question *"why did we do X this way?"* will come up later — AIDA's defensible niche **is** that the answer survives.

---

## Honest scope statement

`/aida-plan`'s value **isn't** *"better plans than `/ultraplan`."* `/ultraplan`'s LLM-heavy cloud cycles produce denser, more thorough planning artifacts than a single local Claude session can match. The defensible thing AIDA brings is *"plans that persist, integrate with the requirement graph, and stay anchored as the code evolves."* That's a complementary capability, not a substitute.

If a team has to pick one of the two for budget reasons, the right answer depends on what they're optimizing for:

- *"Generate the most thorough plan per work item"* → `/ultraplan`. Cloud LLM cycles dominate single-session planning on raw output density.
- *"Keep planning artifacts queryable, versioned, and integrated with the code over time"* → AIDA. No competitor in this slot.
- *"Plan without a vendor subscription or internet"* → AIDA. Local-first always works.

Both fit in most workflows. The point of this doc is that picking *one* in a binary way is almost always wrong — and the cost of compose-them is one file save + one comment.

---

## See also

- [vs-ultrareview.md](vs-ultrareview.md) — sister positioning doc, same thesis applied to code review
- STORY-112 — Plan-mode skill placeholder ("inspired by /ultraplan"); parent of the plan-tooling roadmap
- TASK-92 — Structured plan template (the 11-section convention extracted from /ultraplan dissection)
- TASK-93 — `aida plan verify` (re-anchor stale line refs to symbols)
- TASK-94 — Auto-derive "Reusable helpers" section from trace graph
- TASK-95 — `aida queue work` pre-populates session manifest from matching plan file
- TASK-96 — `aida queue done` extracts Followups section, offers to file as TASKs
- `docs/plans/2026-05-13-story-86-done-status.md` — worked example: the actual `/ultraplan` output that prompted this doc
- [composition.md](composition.md) — generic guidance on layering AIDA with other tools (future page)
