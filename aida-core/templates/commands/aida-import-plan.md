# Import a Saved Plan into AIDA

Turn a free-floating plan file (e.g. saved from `/ultraplan`'s teleport-back)
into first-class AIDA state.

## Instructions

Follow the workflow in `.claude/skills/aida-import-plan.md`:

1. Read the file and detect its target SPEC-ID (frontmatter → heading → filename → ask)
2. Move it to `docs/plans/YYYY-MM-DD-<slug>.md` per AIDA convention
3. Pin the plan to its spec with `aida comment add` (and note secondary specs)
4. Parse the Critical Files / Followups / Verification sections and surface them
5. Run `aida plan verify` on the moved file and report drift findings
6. With `--queue`, route the spec to the implementer queue

Flags: `--queue` (queue for implementer), `--auto-anchor` (auto-fix drifted
refs), `--dry-run` (report only, no side effects).

Use after a `/ultraplan` session saves a plan to a loose local file.
