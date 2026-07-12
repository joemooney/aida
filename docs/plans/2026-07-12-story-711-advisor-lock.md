# Plan: STORY-711 — advisor-directed branch/worktree lock

- **Date**: 2026-07-12
- **Specs**: STORY-711 (generalizes BUG-637)
- **Status**: design signed off (operator, 2026-07-12) — ready to build
- **Complexity**: medium (coordination substrate; correctness bar is high)

## Approach

Bind a **worktree** to an authorizing **advisor**, and make an implementer agent
**verify-or-refuse** before it acts. Fail-safe: an agent that cannot confirm an
authorizing advisor-lock on its worktree refuses to implement. This kills the
three coordination failures observed repeatedly: two agents on the same spec, a
live spec rejected because no lease showed the other agent, and two agents
colliding on one working tree's index.

Reuse existing machinery — do **not** invent a parallel locking system:
- **Leases** (`.aida/sessions/*.toml`, `coordination.rs` writer / `liveness.rs`
  `SessionLeaseLite` reader) hold the lock data.
- **Briefs** (`.aida/agent-briefs/<agent>/`) carry the authorizing-advisor token
  to the implementer.
- **The bouncer** mirrors `advisor_code_gate.rs` (STORY-670/684): a programmatic
  gate, not a CLAUDE.md rule — an invariant against a confident LLM has to be a
  bouncer.

### Signed-off design forks (operator 2026-07-12)
1. **Where**: extend the session-lease registry (`authorized_by` field), not a
   new `.aida/locks/` dir. One source of truth for "who owns this tree."
2. **Proof**: a token in the brief + the role-context snapshot, not a bare
   `AIDA_ADVISOR_LOCK` env var (env is spoofable + vanishes on respawn).
3. **Enforcement**: a substrate-as-bouncer pre-work / pre-commit gate that
   verifies match-or-refuse.
4. **Composition**: the lock is the *enforceable* layer atop today's *advisory*
   leases; worktree isolation is the *physical* separation. They compose.
5. **Granularity**: lock the **worktree** (branch derived) — solves
   two-agents-one-tree, not just the same-spec case.

## Decisions

- **`authorized_by: Option<String>` on the lease** (serde default → old leases
  read `None`, fully backward-compatible; confirmed against `SessionLeaseLite`).
  Value = the authorizing advisor's session/agent id.
- **Two slices.** Slice 1 ships the lock model + a pure verifier + an `aida lock`
  CLI (usable as a *manual* bouncer immediately, and additive — it touches no
  existing enforcement path, so it cannot break current flows). Slice 2 wires the
  *automatic* gate into the agent pre-work flow (the higher-risk enforcement).
- **Pure core.** All lock/verify logic lives in a pure, unit-tested function
  `verify_worktree_lock(lock: Option<&Lock>, my_token: Option<&str>) -> Verdict`
  (`Authorized` / `Refused{by}` / `Unlocked`), so the substrate contract is
  testable without a live agent.
- **Fail-safe default is opt-in per posture.** Requiring a lock everywhere would
  break every current solo/manual flow. Gate behavior is a `[locking]` config
  posture (`off` default; `warn`; `enforce`) so adoption is deliberate — matches
  the STORY-495 native-default discipline. Slice 1 ships `off`+manual; slice 2
  wires `warn`/`enforce`.

## Files (build order)

**Slice 1 — model + verifier + CLI (additive, safe):**
1. `aida-core/src/liveness.rs` — add `authorized_by: Option<String>` (serde
   default) to `SessionLeaseLite`.
2. `aida-cli/src/coordination.rs` — add the same field to the lease-claim writer
   struct (the one `toml::to_string_pretty`'d at ~L443/L781); set it when an
   advisor authorizes a worktree.
3. `aida-core/src/` (new small module or in liveness) — the pure
   `verify_worktree_lock` + `Verdict` enum + the `Lock` view (worktree +
   authorized_by + ttl/started_at). Unit tests here.
4. `aida-cli/src/cli.rs` + `aida-cli/src/main.rs` — `aida lock` subcommand:
   `acquire <worktree> --advisor <id>` (writes/updates the lease's authorized_by),
   `verify [<worktree>] --as <token>` (prints Verdict, exit 0/non-zero for
   scripting), `release <worktree>`, `status` (show locks). Honors
   `AIDA_AGENT_OUTPUT`.

**Slice 2 — automatic bouncer (higher-risk, separate PR):**
5. `aida-cli/src/coordination.rs` / `aida-cli/src/main.rs` — brief gains an
   `authorized_by` token (`aida brief` writes it; `list_briefs`/`read_brief`
   surface it); the role-context snapshot includes it.
6. A pre-work gate (mirror `advisor_code_gate.rs`) invoked by the implementer's
   first action / a PreToolUse hook + a pre-commit check: read my worktree's
   lock, read my brief token, run `verify_worktree_lock`; on `Refused` under
   `enforce`, refuse and print the authorizing advisor. `[locking]` posture gates
   `off`/`warn`/`enforce`.
7. `docs/environment-variables.md` + a discipline doc — the `[locking]` posture
   + `AIDA_LOCKING` override row.

## Tests

- `verify_worktree_lock_*`: Unlocked → Authorized (no lock = no gate in slice 1);
  matching token → Authorized; mismatched token → Refused{by}; missing token
  under a present lock → Refused (fail-safe).
- lease round-trips `authorized_by` (write → read → same); an old TOML without
  the field reads `None` (backward-compat).
- `aida lock acquire` then `verify --as <other>` exits non-zero; `--as <same>`
  exits 0.
- Slice 2: brief carries the token; the gate refuses a mismatched agent under
  `enforce`, warns under `warn`, is silent under `off`.

## Verification (executable)

```bash
env -u AIDA_SESSION_ROLE cargo test -p aida-core -p aida-cli verify_worktree_lock lock_
# manual: two worktrees, advisor A locks WT1; an agent with A's token verifies OK,
# an agent with B's token (or none) is Refused.
aida lock acquire <WT1> --advisor A && aida lock verify <WT1> --as A && ! aida lock verify <WT1> --as B
```

## Risks + gotchas

- **Breaking current flows**: every solo/manual session runs without a lock today.
  Mitigate with the `off`-default `[locking]` posture; only `enforce` refuses.
- **Lease vs lock lifecycle**: a lock is a lease field, so it dies with the lease
  (stale-lease reaping already exists) — good, no orphaned locks. Confirm the
  reaper clears `authorized_by` with the lease.
- **Token spoofing**: the brief token is only as trustworthy as brief delivery;
  acceptable for cooperative agents (the threat model is *accidental* collision,
  not adversarial). Document this boundary.
- **Cross-clone**: leases are per-clone local state; a lock does not travel across
  clones. In scope only for same-machine multi-agent (the observed failure). Note
  it; cross-clone lock is a followup if needed.

## Followups

- Slice 2 (the automatic bouncer) as its own PR once slice 1 lands.
- Consider surfacing locks in `aida ps` (a locked-by column) once slice 1 exists.
- Cross-clone lock propagation (only if a real cross-clone collision is observed).

## Related

- BUG-637 (spec-scoped claim — the narrower ancestor).
- `advisor_code_gate.rs` (STORY-670/684) — the bouncer pattern to mirror.
- STORY-495 (native-default discipline) — why the posture defaults `off`.
