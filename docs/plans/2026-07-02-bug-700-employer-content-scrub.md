# Plan: BUG-700 — history-rewrite scrub of employer-identifying content

Date: 2026-07-02
Specs: BUG-700
Status: Draft
Complexity: ~0 prod LOC (history rewrite + coordination), risk **high** (force-push public history, all clones re-sync)

<!--
  Sensitive literal strings are intentionally NOT in this file — it commits to the
  PUBLIC repo. The literal old→new map lives ONLY in a local, uncommitted file
  (scratchpad `replacements.txt`, format below). Placeholders here:
    <EMPLOYER_DOMAIN>   the employer email domain (used as owner/email + code test data)
    <FIREWALL_HOST>     the corporate firewall block-page hostname
    <POLICY_GROUP>      the named corporate web-filter policy group
    <EMPLOYER_SHORT>    the employer short name (one plan-doc reference)
-->

## Approach

Employer-identifying strings are committed into the **public** `joemooney/aida` GitHub
repo across two disjoint histories: the orphan **`aida-store`** branch (spec YAMLs,
`registry/*`, `oplog.yaml`) and the **`main`** code branch (used as example/test data).
Editing the tips is insufficient — the strings live in ancestor commits — so this is a
`git filter-repo --replace-text` history rewrite run on a fresh mirror clone, followed by a
coordinated force-push and a re-sync of every working clone. The replacement map swaps each
sensitive literal for a benign RFC-2606 `.example` equivalent, chosen so the **code tree still
compiles and its unit tests still pass** (the identity-fold tests fold any domain identically).
The rewrite cascade starts at the first sensitive commit on each branch — **`main` only from
2026-06-26**, **`aida-store` from 2026-05-11** — so most of the ~687 stale branches (disjoint or
older) are untouched. Blast radius is cut further by closing open PRs and pruning dead branches
first. gitlab.joemooney.com (personal, lower urgency) gets the same rewrite for consistency.

### Diagram

```
  fresh --mirror clone
        │
        ▼
  filter-repo --replace-text replacements.txt   (rewrites BOTH main & aida-store history)
        │
        ▼
  VERIFY: 0 sensitive strings │ cargo build+test green │ aida cache rebuild clean
        │
        ▼
  quiet window ──► force-push live refs (main, aida-store, open-PR branches) ──► GitHub + gitlab
        │
        ▼
  every clone re-syncs (fresh clone preferred; else reset main + reset .aida-store + cache rebuild)
```

## Decisions

- **Tool: `git filter-repo --replace-text`, not `filter-branch` / BFG**. **Rationale**: filter-repo
  is the maintained, fast, recommended tool; `--replace-text` does exactly literal/regex blob
  substitution across all history in one pass. BFG is coarser and Java-based; `filter-branch` is
  deprecated and slow.
- **Replace with `.example` benign equivalents, not `***REMOVED***`**. **Rationale**: the strings are
  live example/test data in code — a structured replacement (`<EMPLOYER_DOMAIN>` → `work.example`)
  keeps the identity-fold unit tests semantically valid and the tree compiling. A `***REMOVED***`
  token would break the email-shaped assertions.
- **Run on a fresh `--mirror` clone, force-push selectively**. **Rationale**: filter-repo refuses to
  run on a repo with a stale/dirty state and wants a clean mirror. Pushing selected live refs (rather
  than a blind `push --mirror` that also *deletes* absent refs) avoids nuking the 687 remote branches.
- **Reduce blast radius before rewriting** (close/merge the 5 open PRs, prune merged/dead branches).
  **Rationale**: every ref that descends from a rewritten commit needs a force-push and invalidates any
  open PR built on it; fewer live refs = smaller, safer operation.
- **No credential rotation**. **Rationale**: the exposed content is PII / employer-identifying context,
  not a secret/key. There is nothing to rotate; the fix is removal + cache purge, not revocation.
- **Tier the EPIC-44 corporate-network comment**. **Rationale**: token replacement (Tier 1) neutralizes
  the hard identifiers (`<FIREWALL_HOST>`, `<POLICY_GROUP>`, domain). The residual prose ("corporate web
  filter / corporate network") is generic and non-attributing. Full-paragraph redaction (Tier 2, via a
  `--blob-callback`) is available if the operator wants the whole narrative gone — decided at execution.

## Files (in build-order)

This is a history-rewrite operation, not a code change — the "files" are the operational inputs and the
content surfaces the rewrite touches.

### `replacements.txt` (new, LOCAL-ONLY — never committed)

- The `git filter-repo --replace-text` rule file. Format: one `old==>new` per line, applied top-to-bottom
  per blob, most-specific first (so the generic domain rule doesn't pre-empt the firewall-host rule).
  Rules cover: `<FIREWALL_HOST>`, `<POLICY_GROUP>`, both email casings, the bare `<EMPLOYER_DOMAIN>`,
  `<EMPLOYER_SHORT>`, and the standalone host token (last).

### Content surfaces rewritten on `aida-store`

- `objects/BUG/000/BUG-89.yaml`, `objects/EPIC/000/EPIC-44.yaml`, `objects/TASK/000/TASK-845.yaml`,
  `objects/TASK/000/TASK-951.yaml` — spec text referencing the domain / corporate-network detail.
- `registry/nodes.toml`, `registry/blocks.yaml` — node owner/email provenance.
- `oplog.yaml` — CRDT ops mirroring the above.

### Content surfaces rewritten on `main`

- `aida-cli/src/cli.rs` — doc-comment example (`fn` identity-resolve arg).
- `aida-core/src/alias.rs` — module doc + `[aliases]` toml example.
- `aida-core/src/node.rs` — `canonical_user_id` unit test assertions (email-shaped fold).
- `aida-core/src/team.rs` — `build_team_members` test fixture (`[[nodes]]` hostname + user).
- `docs/plans/2026-03-15-distributed-architecture-identity.md` — one `<EMPLOYER_SHORT>` reference.

## Critical Files

- `replacements.txt` (local-only rule file — the crux; a wrong rule silently mis-rewrites history)
- `aida-core/src/node.rs`, `aida-core/src/team.rs` (live test assertions — must stay green post-rewrite)
- `registry/blocks.yaml`, `registry/nodes.toml`, `oplog.yaml` (store integrity — cache must rebuild clean)
- `objects/EPIC/000/EPIC-44.yaml` (most sensitive: corporate-network detail)

## Reusable helpers (do not reimplement)

- `git filter-repo --replace-text <file>` — the substitution engine; do not hand-roll sed over `rev-list`.
- `aida cache rebuild` — rebuilds the SQLite read-projection from the rewritten `aida-store` (owner strings
  are cached; a stale cache would still surface the old values).
- `aida db reconcile-status` / `aida pull` — post-rewrite, to re-settle any Done→Completed bumps if commit
  SHAs that fed the auto-bump changed.
- `git for-each-ref` — enumerate live refs to force-push, rather than `push --mirror` (which deletes).

## Risks + gotchas

1. **Risk**: `push --mirror` back from the rewritten mirror would DELETE the ~687 remote branches not in the
   mirror's ref set / force-update everything blindly. **Mitigation**: never `--mirror` push; enumerate and
   force-push only live refs (`main`, `aida-store`, the 5 open-PR branches after rebasing them).
2. **Risk**: the 5 open PRs are built on pre-rewrite `main` SHAs → they break / show huge diffs after the
   force-push. **Mitigation**: merge or close all open PRs BEFORE the rewrite; re-cut any survivor from the
   new `main`.
3. **Risk**: every existing clone's `.aida-store` worktree + `.aida/cache.db` still holds old strings and
   old SHAs; a later `aida db sync --push` from a stale clone would RE-INTRODUCE the scrubbed content.
   **Mitigation**: coordinate — freeze pushes during the window; after the rewrite, every clone does a fresh
   clone (preferred) or `git fetch && git reset --hard` on `main` + re-attach `.aida-store` + `aida cache
   rebuild`. This is the single biggest coordination hazard.
4. **Risk**: GitHub retains old commits via cached SHA URLs, forks, and PR refs even after force-push.
   **Mitigation**: after force-push, delete stale PR refs; if strong guarantees are needed, contact GitHub
   Support to purge cached views and check for forks. Treat "removed from history" as best-effort on a
   platform that caches.
5. **Risk**: the code tree fails to compile / tests fail after replacement (a replacement broke an
   assertion). **Mitigation**: the map is chosen for consistent both-sides folding; VERIFY step runs
   `cargo build --workspace` + `env -u AIDA_SESSION_ROLE cargo test -p aida-core` on the rewritten tip
   before any push.
6. **Risk**: gitlab's `aida-store` currently holds the node-6 reconcile merge (BUG-700 context) with the
   same strings in its blobs. **Mitigation**: run the identical rewrite against gitlab; sequence it so the
   reconciled+scrubbed store lands on GitHub in one clean pass afterward (do NOT push the un-scrubbed
   reconcile to GitHub in the meantime — already held).
7. **Risk**: `aida-store` is an orphan branch inside the same repo; a naive path-scoped rewrite could
   desync the two histories. **Mitigation**: `--replace-text` is content-based (not path-scoped) and runs
   over all refs in the mirror, so both histories are covered in one invocation.

## Tests (named, not "add tests")

- `canonical_user_id` email-fold assertions (`aida-core/src/node.rs`) — must still pass with the replaced
  domain (both the input and expected output are rewritten consistently).
- `build_team_members` "three distinct owners before linking" (`aida-core/src/team.rs`) — fixture rewrite
  keeps three distinct identities.
- Post-rewrite grep gate (below) — the real acceptance test: zero sensitive strings across all refs.

## Verification

Run on the rewritten MIRROR before any push. `SENSITIVE` is sourced from the local-only map, never inlined.

```bash
# --- in the rewritten mirror clone ---
MIRROR=/path/to/aida-scrub.git   # produced by: git clone --mirror <origin> && filter-repo --replace-text replacements.txt

# 1. No sensitive string survives in ANY blob across ALL refs (the acceptance gate).
#    Build the pattern from the LOCAL map's left-hand sides (do not type them into a committed file).
PATTERN=$(awk -F'==>' '/==>/{print $1}' /abs/scratchpad/replacements.txt | paste -sd'|' -)
if git -C "$MIRROR" grep -I -E "$PATTERN" $(git -C "$MIRROR" rev-list --all) -- 2>/dev/null | head; then
  echo "FAIL: sensitive strings still present"; else echo "PASS: clean across all history"; fi

# 2. Code tip still builds + identity tests pass (work in a normal checkout of the rewritten main).
git clone "$MIRROR" /tmp/aida-verify && cd /tmp/aida-verify
cargo build --workspace 2>&1 | tail -3
env -u AIDA_SESSION_ROLE cargo test -p aida-core canonical_user_id 2>&1 | tail -5   # expect: ok
env -u AIDA_SESSION_ROLE cargo test -p aida-core build_team_members 2>&1 | tail -5  # expect: ok

# 3. Store rebuilds clean from the rewritten aida-store (owner strings updated, no schema drift).
#    (attach .aida-store from the rewritten branch, then:)
aida cache rebuild && aida list --status draft | grep -i BUG-700   # spec graph intact
```

**Worktree-aware binary path** (TASK-388): if invoking the built binary directly, use
`AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"`, not bare `target/debug/aida`.

## Followups

- Prune the ~687 stale remote branches (most are merged `bug-*` / `codex/*`) — a standalone hygiene sweep;
  shrinks the blast radius of *this* and any future rewrite.
- Add a pre-push guard/hook that greps staged content for the employer domain, so re-introduction is caught
  before it lands (substrate-as-bouncer, not a rule in CLAUDE.md).
- Decide Tier-2 (full EPIC-44 paragraph redaction via `--blob-callback`) vs Tier-1 (token replacement only).
- Post-rewrite: bring the reconciled+scrubbed store from gitlab to GitHub in one clean pass (closes the
  node-6 reconcile interim divergence noted in BUG-700).

## Related

- Fixes: BUG-700
- Context: node-6 (spock) store reconcile 2026-07-02 (pushed to gitlab only, GitHub held pending this scrub)
- Standing rule: scrub employer-identifying content before it lands on the public repo (memory:
  `feedback_public_repo_scrub_employer_content`)
