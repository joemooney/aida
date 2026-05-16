# Skill authoring conventions

Shared conventions for the skill templates under `aida-core/templates/skills/`.
These are the master copies — `aida init` embeds them into the binary and the
project-local `.claude/skills/` mirrors them via per-file symlinks. Edit the
master, never the symlink. See `CLAUDE.md` → "Template architecture".

## Next-steps rendering — Path / What happens / Why table

Several skills end with a hand-off prompt: `/aida-pickup` after `aida queue
done`, `/aida-pr` after the PR opens, `/aida-review` after the merge lands.
These prompts present the user with the moves available from here.

**When the prompt offers 2+ paths forward, render it as a markdown table**
with three columns:

| Column | Holds |
|--------|-------|
| **Path** | The choice, led by a glyph — `▶` primary, `⏵` alternate, `🚪` stop/exit |
| **What happens** | The concrete command(s) — name the spec, PR, session ID; no "you might consider…" |
| **Why** | The *reason the path is shaped this way* — the role / lease / worktree implication |

The **Why** column is load-bearing. It is the differentiator a reader's
decision-making attention lands on, and it must do more than rephrase the
action. "Opens a PR" is not a Why; "Same session, same lease — the batch is
done, ship it before context goes cold" is. First-users learning the workflow
get the *reasoning behind the asymmetry* — why one path reuses the session and
another needs a fresh one — baked into the skill's own output.

### Why a table, not a numbered list

A numbered list (`1. ▶ … 2. ⏵ …`) buries the *why* in a parenthetical at the
end of a long line, or omits it. A table puts the reasoning in its own column,
aligned and scannable. The box-drawing grid Claude Code's terminal renders for
a GFM table reads as a decision surface, not a recipe.

### Rendering rules

- **Emit a real GFM markdown table** — `| … | … | … |` rows — *not* wrapped in
  a code fence. A fenced table renders as literal pipes; an unfenced one
  renders as the box-rule grid. The skill templates show the table unfenced
  for exactly this reason.
- **Glyphs live in the Path cell.** `▶` = primary recommended action, `⏵` =
  alternative path, `🚪` = stop/exit. Keep them — they carry the semantics and
  AIDA's CLI output uses them deliberately. Don't ASCII-degrade by default.
- **Print exactly one table.** Skills with several state-dependent templates
  (cluster drained / partial / simple) pick one based on detected state and
  render only it.
- A prose lead-in line (`✓ TASK-260 done. 2 more items queued…`) can precede
  the table; print it as a normal sentence above the grid.

### When NOT to use a table

A **single linear next-step** stays a compact one-liner or short bullet — a
table for one row is overhead. The table earns its keep only at 2+ options.
Likewise, autonomous flows (`/goal`, `aida queue work --auto-complete`) drive
the workflow themselves and never reach a human decision point, so they emit
no next-steps prompt at all — tables are for human-driven flow only.

### The one-line rule for skill authors

> When presenting 2+ paths forward, render as a markdown table with columns
> Path / What happens / Why. Use ▶ ⏵ 🚪 glyphs in the Path cell for the
> primary / alternate / exit semantics.

This sentence is repeated verbatim in each affected skill's glyph-convention
block so the rule travels with the template.

### Skills that follow this convention

- `aida-pickup.md` — Step 6, four state-aware templates
- `aida-pr.md` — Step 12, two auto-queue-outcome templates
- `aida-review.md` — Step 11, two post-merge templates

Any new skill that ends with a multi-option hand-off prompt should adopt the
same table. Pairs with the pre-action banner convention in `/aida-pr` (TASK-259)
— banner is pre-action, the table is post-action.

trace:TASK-260 | ai:claude
