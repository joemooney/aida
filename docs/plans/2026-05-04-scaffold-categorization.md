# Scaffold file categorization design (SPIKE-1-029)

Investigation deliverable for SPIKE-1-029. Unblocks FR-1-028 (`aida scaffold upgrade`) and refines FR-1-027 (`aida scaffold diff`).

## Related Requirements

- Spike: **SPIKE-1-029** — this doc
- Parent: **EPIC-1-026** — scaffold upgrade workflow
- Implements alongside: **FR-1-028** (`aida scaffold upgrade` subcommand)
- Sibling: **FR-1-027** (`aida scaffold diff`)
- Depends-on context: today's `FileStatus` enum at `aida-core/src/scaffolding/mod.rs:169` (5 variants, no category awareness)

## Status

**Completed.** Five questions answered with recommendations and tradeoffs; ready for FR-1-028 implementation.

---

## Status quo (problem this fixes)

Today the scaffolder treats every file with one strategy:

```
write file → embed AIDA header w/ checksum → on next run compare checksum →
  ↳ Unmodified → overwrite OK
  ↳ Modified   → warn, skip (or `apply --force` clobbers everything)
  ↳ NoHeader   → warn, skip
  ↳ OlderVersion → warn, offer upgrade (but no real upgrade — apply with --force)
```

This collapses three semantically different ownership models into one signal. From the originating context (`~/ai/paradox` drift):

- `CLAUDE.md` reported drift. **False positive** — that file is meant to be user-edited after `init`.
- `.claude/hooks/aida-validate-commit.sh` reported drift. **True positive** — but the only fix is `apply --force`, which clobbers everything else too.
- `.claude/settings.json` reported drift. **True positive AND false positive** — AIDA owns the `hooks` and `statusLine` blocks, user owns whatever else they added; whole-file checksum can't tell them apart.

All three failure modes show up at the same time, so `apply --force` is the only motion available and it loses user content.

---

## Q1: Category enumeration

The three categories proposed in EPIC-1-026 are correct as a foundation. Add one bookkeeping pseudo-category so every file in the scaffold manifest has a defined category — no "unspecified" tail:

| Category | Ownership | Drift semantics | Upgrade strategy |
|---|---|---|---|
| **template** | AIDA-owned | Drift = stale on-disk copy | Overwrite without prompt |
| **seed** | User-owned after init | Drift expected | Skip; never report drift after first apply |
| **managed-merge** | Slot-shared | Drift on AIDA slots = stale | Patch AIDA slots; leave user keys |
| **detached** (NEW) | User-owned by explicit opt-out | n/a | Skip entirely |

`detached` is the result state of `scaffold upgrade --detach <path>`. Modeled as a category (not a separate flag) so the dispatch in `scaffold upgrade` is uniform: "look up category, dispatch to handler". Transitioning from any category to `detached` is one-way (re-attaching = `aida scaffold apply --force` and accepting the overwrite).

### Edge cases reviewed

- **`.git/hooks/commit-msg`** → category `template`. Lives outside `.claude/` but follows the same "AIDA wholly owns this script" rule. The fact that it's git-managed is orthogonal.
- **`.gitignore`** patches → category `managed-merge`. AIDA owns specific lines (e.g. `.aida/cache.db`, `.aida-store/`) but user owns everything else. Slot model: line-set, additive only.
- **`docs/plans/`** → the directory itself is `seed` (created at init, then user-owned). Files inside are NOT in the scaffold manifest at all — they're user content, not scaffolded.
- **`AGENTS.md`** → `seed`. Currently same checksum-on-create-only behavior the bug fix needs.
- **`.claude/skills/*` / `.claude/commands/*`** → `template`. Each *file* is a separate manifest entry (per-file symlinks in this dogfood repo confirm the model). New skills appearing in master templates auto-arrive on the next `scaffold apply`.
- **`.claude/hooks/*.sh`** → `template`. EPIC-1-026 lumped this with `managed-merge`; the spike's recommendation is to split: each hook *script* is `template` (AIDA wholly owns the file body), but the *list of which hooks are wired up* lives in `settings.json` (`managed-merge`).
- **`.mcp.json`** → `managed-merge`. AIDA owns `mcpServers.aida`, user might add other MCP servers as siblings.
- **`.claude/settings.json`** → `managed-merge`. AIDA owns `hooks.PreToolUse`, `hooks.PostToolUse`, `hooks.SessionStart`, and `statusLine` (after FR-1-013); user owns `permissions`, additional `hooks.*` matchers, etc.

---

## Q2: Declaration mechanism

**Recommendation: per-file frontmatter for markdown templates + a single `templates/manifest.toml` for non-markdown and special cases.** Hybrid, but the parts where each shines are non-overlapping.

### Options weighed

**(a) Hard-coded in Rust scaffolder source.** ✗
- Fastest to ship, no parsing needed.
- But every new template requires editing Rust. Friction against the "add a new skill = drop a markdown file" promise. Already strained today: `aida-core/src/scaffolding/mod.rs` has 25 hand-written `let path = PathBuf::from(".claude/skills/aida-X.md")` blocks (lines 576–875) — each new skill needs a code edit. This is exactly the scaling problem the manifest must NOT inherit.

**(b) Per-file frontmatter in `aida-core/templates/`.** ✓ for markdown.
- Skills already use YAML frontmatter (the `name:`, `description:`, `allowed-tools:` block). Adding `category: template` to that frontmatter is one line per file; `serde_yaml` already in use.
- Locality: looking at a template tells you its category. No second file to keep in sync.
- Doesn't work for non-markdown (JSON, shell scripts, the `.git/hooks/commit-msg` file) without polluting them with comment-conventions per language.

**(c) Single `templates/manifest.toml`.** ✓ for non-markdown.
- Central source of truth for files that don't have a natural frontmatter slot — JSON, shell scripts, generated artifacts (CLAUDE.md, AGENTS.md, .mcp.json).
- Risk: drifts from the actual file set in `templates/`. Mitigation: `build.rs` (already walks `templates/` for embedding) checks every embedded file appears in the manifest and fails compilation otherwise. Same trick the cache uses for stale-detection.

**Recommended hybrid:**

```
templates/skills/aida-pickup.md      → category in YAML frontmatter
templates/commands/aida-pickup.md    → category in YAML frontmatter  (or implicit "template" if absent)
templates/hooks/aida-validate-commit.sh → manifest.toml
templates/settings.json              → manifest.toml (with slot map)
                                       generated CLAUDE.md/AGENTS.md/.mcp.json   → manifest.toml (synthesized files,
                                       not in templates/ on disk; manifest declares
                                       category + generator function name)
```

Rationale for split: skills/commands are bulk and homogeneous (one per file, all `template`), so the per-file approach scales. Special cases are few and irregular (different slot models, generators), so a central manifest beats scattered annotations.

`manifest.toml` schema sketch:

```toml
schema_version = "1"

[[file]]
path = ".claude/hooks/aida-validate-commit.sh"
category = "template"
source = "templates/hooks/aida-validate-commit.sh"
exec = true

[[file]]
path = ".claude/settings.json"
category = "managed-merge"
source = "templates/settings.json"
slots = ["$.hooks.PreToolUse", "$.hooks.PostToolUse", "$.hooks.SessionStart", "$.statusLine"]

[[file]]
path = ".mcp.json"
category = "managed-merge"
generator = "generate_mcp_json"   # since this is synthesized, not a static file
slots = ["$.mcpServers.aida"]

[[file]]
path = "CLAUDE.md"
category = "seed"
generator = "generate_claude_md"

[[file]]
path = "docs/plans/.gitkeep"
category = "seed"
content = ""                       # tiny inline content for trivial files
```

JSONPath-style slot expressions because they're standard, easy to reason about, and have a tiny existing Rust crate (`jsonpath_lib` or `serde_json_path`). Not all options need slots — only `managed-merge` does.

---

## Q3: Slot model for managed-merge

**Recommendation: JSON pointer / JSONPath expressions in the manifest, with deep-merge semantics: "AIDA fully owns the value at each declared slot path; user owns everything else."**

### Why not the alternatives

**(a) JSON pointer paths** — chosen. Standard (RFC 6901), unambiguous, copy-paste-able into messages. Library support trivial. Works for `settings.json`, `.mcp.json`, future `.aida/config.toml` if it grows AIDA-owned sections.

**(b) Marker comments** — impossible in JSON. Even if it weren't, comments scatter the source of truth across files instead of centralizing in the manifest.

**(c) Deep-merge with allowlist** — equivalent in expressive power to JSON pointer, but harder to express precisely (nested object keys vs. atomic slots). The "AIDA owns these keys" wording also obscures the fact that AIDA owns *values at paths*, not keys themselves — if user has a different value at the same path, that's a slot conflict, not a key conflict.

### Semantics

For each file with category `managed-merge`:

```
upgrade(file):
    user_doc = parse(file)
    aida_doc = render_template(manifest.source or manifest.generator)
    for slot in manifest.slots:
        user_value_at_slot = jsonpath_get(user_doc, slot)
        aida_value_at_slot = jsonpath_get(aida_doc, slot)
        if user_value_at_slot == aida_value_at_slot:
            continue                                  # no drift in this slot
        else if user_value_at_slot is missing:
            jsonpath_set(user_doc, slot, aida_value_at_slot)   # AIDA-owned key newly added
        else:
            # User has modified an AIDA slot
            present diff to user
            ask y/n unless --yes
            if yes: jsonpath_set(user_doc, slot, aida_value_at_slot)
    write(file, user_doc)
```

User keys outside any AIDA slot are read in, never compared, written back unchanged.

### Worked example: `.claude/settings.json`

Manifest declares slots `[$.hooks.PreToolUse, $.hooks.PostToolUse, $.hooks.SessionStart, $.statusLine]`.

User adds:
```json
{
  "permissions": {"deny": ["rm -rf /"]},
  "hooks": {
    "PreToolUse": [...AIDA's content stale...],
    "PostToolUse": [...AIDA's content current...],
    "Stop": [...user's own stop hook...]   ← outside any AIDA slot
  },
  "statusLine": {...AIDA's content current...}
}
```

`scaffold upgrade` finds:
- `$.hooks.PreToolUse` differs from new template → prompt y/n, replace value
- `$.hooks.PostToolUse` matches → no-op
- `$.hooks.SessionStart` missing in user doc → AIDA adds it (newly-owned slot)
- `$.statusLine` matches → no-op
- User's `permissions` block and `hooks.Stop` array → untouched

User content preserved, AIDA slots converged.

### Edge case: array slots

`hooks.PreToolUse` is an array of objects, not a scalar. Simplest semantics: AIDA owns the *whole array value* at that slot. If the user has added their own PreToolUse entries inside that array, they need to use a different `matcher:` or they're effectively in a sub-slot conflict. Document this in `aida scaffold upgrade --help`. Refining to per-array-element slots is possible later but not needed for the initial cut.

---

## Q4: Detach semantics

**Recommendation: `.aida/config.toml` records detached paths in a single `[scaffold].detached` array.**

### Options weighed

**(a) `.aida/config.toml`.** ✓
- Project-scoped, version-controlled with the project, already exists, already has `[scaffold]` section in scope. Single source of truth: one file lists what's detached.
- Simple read: load config once at start of `scaffold` command, has-membership check per file.
- `scaffold upgrade --reattach <path>` is straightforward (remove from list).

**(b) Per-file marker.** ✗
- Adding a "DETACHED" sentinel inside the file body conflicts with managed-merge (where the file is JSON), conflicts with the AIDA-Generated header (where the marker would race with the header), and is invisible to anyone reading the manifest list.
- Only advantage is "the marker is local to the file" — but `aida scaffold status` already loads config, so locality isn't free.

**(c) Checksum-based implicit detach.** ✗
- "If the user changed the file at all, treat it as detached" reverses the entire current model and removes the most useful feature: knowing when the master template has new content the user *would* want. The whole point of categories is to pick the right behavior on intentional drift, not to fall back to "ignore everything modified."

### Schema

```toml
[scaffold]
detached = [
  ".claude/skills/aida-implement.md",   # I want to keep my custom version
  ".mcp.json",                          # I have my own MCP config, hands off
]
```

`scaffold upgrade --detach <path>` appends; `--reattach <path>` removes. Idempotent.

### Interaction with categories

Detached overrides category. A file at `.claude/skills/aida-implement.md` declared `template` in the manifest but listed in `[scaffold].detached` is treated as user-owned: never overwritten, never reported as drift, but `scaffold status` does mention "(detached)" so the user remembers the opt-out exists.

---

## Q5: `scaffold status` output reform

Once categories exist, the output groups by category and only flags drift that's actionable.

### Current output (problem)

```
Modified files (4):
  CLAUDE.md
  .claude/hooks/aida-validate-commit.sh
  .claude/hooks/aida-track-commits.sh
  .claude/settings.json

Run `aida scaffold apply --force` to overwrite.
```

Three of the four shouldn't be reported the same way. The hint at the bottom is the wrong fix for two of them.

### Proposed output

```
$ aida scaffold status

Templates (would be upgraded by `scaffold upgrade`):
  ⟲ .claude/hooks/aida-validate-commit.sh   v2.0.0 → v2.0.1
  ⟲ .claude/hooks/aida-track-commits.sh     v2.0.0 → v2.0.1

Managed-merge (slot drift; review with `scaffold diff`):
  ◐ .claude/settings.json
      hooks.PreToolUse: stale (1 entry differs)

Seed (user-owned, no action needed):
  ✓ CLAUDE.md (last scaffolded 2026-04-12)

Detached:
  (none)

Summary: 2 templates upgradeable, 1 managed-merge needs review.
Run `aida scaffold upgrade` to apply, or `scaffold diff <path>` to inspect.
```

### `--verbose` adds

Detached file list (always, even when empty), seed-file checksums (so the user can spot if they're tracking what they think they are), and `aida scaffold diff` previews inline.

### `--quiet` collapses

To one line per category with counts:

```
Templates: 2 upgradeable. Managed-merge: 1 needs review. Seed: 1 (no action). Detached: 0.
```

### Exit codes

- `0` if no upgradeable templates AND no managed-merge slot drift AND no missing AIDA slots
- `1` if any actionable drift (templates upgradeable OR managed-merge slot drift)
- `2` on real errors (config parse failures, manifest mismatch with embedded set, etc.)

Seed drift never affects the exit code — that's the whole point of the category.

---

## Recommended implementation order for FR-1-028

Roughly two-day shape, breakable across multiple sessions:

1. **Manifest schema + loader** (~3-4h). Add `aida-core/templates/manifest.toml` covering every file the scaffolder currently writes. Parse into a `ScaffoldManifest` struct. Add `build.rs` consistency check (every embedded file appears in manifest; every manifest entry has a corresponding source). Existing scaffolder still uses its hard-coded paths — manifest is parallel data initially.

2. **Frontmatter on skills/commands** (~1h). Add `category: template` to the YAML frontmatter of every file in `templates/skills/` and `templates/commands/`. Treat absence of the key as default `template` so back-compat is automatic. (No skills/commands ever need other categories.)

3. **Reroute scaffolder to use the manifest** (~3-4h). Replace the 25 hand-written `let path = PathBuf::from(...)` blocks with a loop over manifest entries. New skills now just appear by being added to `templates/skills/` — no code edit. This is the line-count win EPIC-1-026 implicitly promised.

4. **Add category to `FileStatus` reporting** (~1h). Each `Artifact` knows its category (from manifest). `ScaffoldPreview` groups by category. `scaffold status` output reformed per Q5.

5. **`scaffold diff` (FR-1-027)** (~2-3h). Independent of upgrade — just byte-level unified diffs for any drifted file regardless of category. Use the `similar` crate (likely already in workspace; if not, small dep).

6. **`scaffold upgrade` template path** (~2h). Templates: overwrite. Easy.

7. **`scaffold upgrade` managed-merge path** (~4-6h). The bulk of the value. Pull in `serde_json_path` (or equivalent), implement the slot-merge loop from Q3, prompt UX.

8. **`scaffold upgrade --detach` / `--reattach`** (~1-2h). Edits `.aida/config.toml`'s `[scaffold].detached` array. Update `scaffold status` to surface the detached list.

9. **Tests** (~3-4h). Per-category fixture projects; before/after assertions on slot semantics; detach idempotency.

Total: ~20-24 focused-coding-hours. Step 1-2 are independent and can land first to de-risk; everything after that builds on the manifest.

---

## Open questions for follow-up

- **Audit log of upgrade events.** FR-1-028 mentions recording when each file was last touched by AIDA. Could live alongside `[scaffold].detached` as `[scaffold].last_upgraded.<path> = "<ISO date>"`. Defer to FR-1-028 implementation.
- **Discovery of new templates.** When master templates add a new file (e.g. the `aida-role-context.sh` hook from TASK-1-022), `scaffold status` should report "new template available" not just on `init` but on every status check. Easy: manifest-vs-disk diff.
- **Manifest in a published binary.** When users `aida upgrade` to a new release, the manifest comes along (embedded). Their `.aida/config.toml` doesn't change. So `scaffold status` post-upgrade automatically reflects the new category model. No migration needed — confirms `.aida/config.toml` is the right home for `detached`.
