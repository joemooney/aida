# Plan: TASK-281 — auto-claim a new block on spec creation when available IDs cross threshold

Date: 2026-05-20
Specs: TASK-281
Status: In Progress
Complexity: ~250 prod LOC, ~200 test LOC, 1 commit, risk low

## Approach

Today the `add_requirement_cli` flow in `aida-cli/src/main.rs` calls
`BlockRegistry::dispense`, and if `BlockRegistry::aggregate_is_low` returns
true, it prints a yellow `WARNING: <TYPE> block running low ... Run aida db
block claim ... soon` — but does nothing to make the user not have to do it.
TASK-281 makes that warning the *fallback* behavior and the new default the
quiet auto-claim. We add a `BlockAllocationConfig` read from
`.aida/config.toml`, a single `ensure_block_capacity(type, n_needed)` helper
that composes the existing `auto_allocate_block_inner` CAS-loop, and wire
that helper into `add_requirement_cli` *before* the dispense step. The
existing init-time block claim (`auto_allocate_initial_blocks`) already
covers the first-time-setup acceptance criterion. `aida pr
auto-queue-review` shells out to `aida add` (`aida_subcmd_add_review_story`)
so it inherits the hook transitively. `aida queue add` doesn't create
specs — it only enqueues an existing one — so no wiring there.

### Diagram

```
                          before this change
   aida add ─► dispense ─► (aggregate_is_low?) ─► WARN, user runs claim manually

                          after this change
   aida add ─► ensure_block_capacity ─► (under threshold? auto_claim enabled?)
                                            │
                       no ─► continue ──────┤
                       yes ─► claim block ──┴─► info notice ─► dispense
                       (opt-out config)    ─► WARN (fallback to old behavior)
```

## Decisions

- **Decision A**: Reuse `auto_allocate_block_inner` rather than write a fresh
  CAS loop. **Rationale**: that helper already handles the push-wins-retry
  semantics, the counter floor, and the local-only short-circuit. Duplication
  would let the two paths drift.
- **Decision B**: Wire into `add_requirement_cli` only — not into `aida pr
  auto-queue-review` directly. **Rationale**: that command shells out to
  `aida add` via `aida_subcmd_add_review_story`, so the single hook covers
  both paths. Adding a second call site would double-fire the claim.
- **Decision C**: Use the `toml` crate for the new `[block_allocation.*]`
  parser instead of extending the hand-rolled line walker in
  `read_id_format_settings`. **Rationale**: nested tables (`[block_allocation.bug]`)
  are awkward in the line walker; `toml = "0.8"` is already a workspace
  dependency. Keep `[id_format]` reader as-is to minimize blast radius.
- **Decision D**: Defaults match the acceptance criteria verbatim:
  `auto_claim = true`, `threshold = 20`, `size = 100`. No config file
  required for new projects to benefit.
- **Decision E**: Scope-aware. Under `IdCounterScope::Global` the threshold
  check and the claim both apply to the shared `*` block, not to a
  per-type block. The user's per-type config (`[block_allocation.bug]`)
  is honored only under `PerType`; under `Global` the global section
  drives sizing.
- **Decision F**: On auto-claim push failure, log a warning and continue
  with the existing (low) block. The add itself still succeeds — the
  user has SOME IDs left. They can retry the claim by running `aida add`
  again later. This matches the acceptance criterion: "Network failure
  during claim: surface clear error, fall back to existing block."

## Files (in build-order)

### `aida-core/src/block_allocation.rs` — new module

- `struct BlockAllocationConfig`: global `auto_claim: bool` + `HashMap<String, BlockAllocationTypeConfig>`.
- `struct BlockAllocationTypeConfig`: optional per-type `auto_claim`, `auto_claim_threshold`, `auto_claim_size` (each `Option<T>`, falling back to global / built-in defaults).
- `impl BlockAllocationConfig`:
  - `pub const DEFAULT_THRESHOLD: u32 = 20`
  - `pub const DEFAULT_SIZE: u32 = 100`
  - `fn is_enabled_for(&self, type_prefix: &str) -> bool`
  - `fn threshold_for(&self, type_prefix: &str) -> u32`
  - `fn size_for(&self, type_prefix: &str) -> u32`
- Re-export from `aida-core/src/lib.rs`.

### `aida-cli/src/main.rs` — config reader + helper + call-site

- `fn read_block_allocation_config(project_dir: &Path) -> BlockAllocationConfig`: parse `.aida/config.toml` via `toml::from_str`, walk `[block_allocation]` + `[block_allocation.<type>]`. Returns defaults when file absent or section absent.
- `fn ensure_block_capacity(store_path, project_dir, node_id, type_prefix, n_needed) -> Result<Option<AutoClaimOutcome>>`:
  1. Read cfg via `read_block_allocation_config`.
  2. Read `IdCounterScope` via `read_id_counter_scope`.
  3. Pick `effective_prefix`: under `Global` use `IdCounterScope::GLOBAL_TYPE_PREFIX` (`"*"`); under `PerType` use `type_prefix`.
  4. If `!cfg.is_enabled_for(effective_prefix)`, return `Ok(None)`.
  5. Load `BlockRegistry`, compute `aggregate_remaining(node_id, effective_prefix)`.
  6. If `remaining >= cfg.threshold_for(effective_prefix)`, return `Ok(None)`.
  7. Call `auto_allocate_block_with_size(store_path, node_id, hostname(), email, effective_prefix, cfg.size_for(effective_prefix))`. Return the label + previous_remaining + new_remaining wrapped in `AutoClaimOutcome`.
- `struct AutoClaimOutcome { label: String, previous_remaining: u32, new_remaining: u32 }`.
- Edit `add_requirement_cli` (block beginning around `if id_policy.uses_blocks() { ... }`): before the dispense logic, call `ensure_block_capacity`. On `Ok(Some(outcome))` print the one-line info notice; on `Ok(None)` continue silently; on `Err` print a warning and continue (do NOT abort the add).
- Edit the BUG-115 WARNING block (around the existing `aggregate_is_low` check after dispense): only print the WARNING when `!cfg.is_enabled_for(...)`. When auto-claim is enabled but didn't fire (e.g., race / push failure absorbed earlier), the warning would be stale, so guard it.

### `aida-core/templates/...` / init template — discoverability

- Two call sites in `aida-cli/src/main.rs` (`handle_init_distributed` line ~5190 and `handle_init_post_clone` line ~5318) write `.aida/config.toml`. Append a commented-out `[block_allocation]` block to each so users discover the knob. Same content in both; consider factoring into a helper if both diverge significantly (don't refactor for the sake of it).

## Critical Files

- `aida-core/src/block_allocation.rs` (new)
- `aida-core/src/lib.rs` (re-export)
- `aida-cli/src/main.rs` (reader + helper + call-site + init template)

## Reusable helpers (do not reimplement)

- `aida_core::BlockRegistry` — `aggregate_remaining`, `aggregate_is_low`, `find_active_block_or_global`, `dispense` (all in `aida-core/src/node.rs`).
- `aida_core::BlockRegistry::AGGREGATE_LOW_THRESHOLD` — currently 20; new code does NOT use this constant directly (it's the default for the *warning*, kept for the disabled-auto-claim path). The new threshold defaults live on `BlockAllocationConfig::DEFAULT_THRESHOLD`.
- `auto_allocate_block_with_size` (`aida-cli/src/main.rs`) — single-block CAS-loop allocator. Already handles the push-wins-retry, counter-floor, and local-only short-circuit.
- `auto_allocate_block_inner` — the underlying impl. Don't call directly; go through `_with_size` for clarity.
- `read_id_counter_scope` (`aida-cli/src/main.rs`) — needed to pick per-type vs `*` prefix.
- `aida_core::IdCounterScope::GLOBAL_TYPE_PREFIX` — `"*"`.
- `hostname()` (`aida-cli/src/main.rs`).

## Risks + gotchas

1. **Risk**: Behavior change for existing projects — they upgrade and suddenly `aida add` makes a network call. **Mitigation**: documented, info-line not warning, opt-out via single line in config.toml. The behavior change is what the spec asked for.
2. **Risk**: Push contention with concurrent `aida add` runs on the same node — both detect under-threshold, both try to claim. **Mitigation**: `auto_allocate_block_inner`'s push-wins CAS loop already handles this. The loser detects the winner's block on re-pull and the next dispense uses it.
3. **Risk**: A push failure inside `ensure_block_capacity` should not block the spec creation. **Mitigation**: catch the `Err` in the call site, print a warning, fall through to the existing dispense (which still has IDs in the active block). The user can retry by running `aida add` again.
4. **Risk**: Global counter scope (`*` block) — the per-type config sections look misleading. **Mitigation**: `ensure_block_capacity` reads scope and uses `*` as the effective prefix when Global; document in the config comment.
5. **Risk**: Tests that already assert the WARNING fires will break. **Mitigation**: search for assertions referencing "running low" and adjust to either disable auto-claim in the test or assert the new info-line.

## Tests (named, not "add tests")

In `aida-core/src/block_allocation.rs`:
- `block_allocation_config_defaults_enable_with_threshold_20_size_100`
- `block_allocation_global_opt_out_disables_all_types`
- `block_allocation_per_type_opt_out_disables_only_that_type`
- `block_allocation_per_type_override_threshold_and_size`
- `block_allocation_type_prefix_case_insensitive`

In `aida-cli` tests (or inline in main.rs `#[cfg(test)]` module):
- `read_block_allocation_config_returns_defaults_when_file_missing`
- `read_block_allocation_config_parses_per_type_section`
- `read_block_allocation_config_handles_missing_section_gracefully`

End-to-end shell smoke (in plan Verification, not as a Rust test):
- Fresh `aida init`, add specs to push the BUG block below 20 remaining, assert auto-claim fired (block count = 2, info notice in stdout, no WARNING).
- Same but with `[block_allocation] auto_claim = false`, assert no auto-claim and the WARNING fires (back-compat).

## Verification

```bash
# Build + test the workspace
cargo build --workspace --quiet
cargo test --workspace --quiet --lib block_allocation
cargo fmt --all -- --check

# Smoke 1: auto-claim fires when enabled (default)
TMP=$(mktemp -d); cd "$TMP" && git init -q && aida init -q
# init claims a BUG-1..100 block. Drain to <20 remaining (i.e. add 81 BUGs)
for i in $(seq 1 81); do aida add --type bug --title "smoke-$i" --status approved -q >/dev/null; done
# The 82nd add should auto-claim a new block.
out=$(aida add --type bug --title "trigger" --status approved 2>&1)
echo "$out" | grep -q "Auto-claimed" || { echo "FAIL: expected auto-claim notice"; exit 1; }
echo "$out" | grep -q "WARNING.*running low" && { echo "FAIL: WARNING should be suppressed"; exit 1; }
aida db block list | grep -c "^1.*BUG" | grep -q "^2$" || { echo "FAIL: expected 2 BUG blocks"; exit 1; }
echo "smoke 1 OK"

# Smoke 2: opt-out keeps the old warning
TMP2=$(mktemp -d); cd "$TMP2" && git init -q && aida init -q
cat >> .aida/config.toml <<'EOF'

[block_allocation]
auto_claim = false
EOF
for i in $(seq 1 81); do aida add --type bug --title "smoke-$i" --status approved -q >/dev/null; done
out=$(aida add --type bug --title "trigger" --status approved 2>&1)
echo "$out" | grep -q "WARNING.*running low" || { echo "FAIL: expected WARNING with opt-out"; exit 1; }
echo "$out" | grep -q "Auto-claimed" && { echo "FAIL: auto-claim should be off"; exit 1; }
echo "smoke 2 OK"
```

## Followups

- Surface `[block_allocation]` knobs in `aida db block status` output so users see their effective threshold/size at a glance.
- Composable telemetry: emit a usage-log line for auto-claim events so we can graph "how often does auto-claim fire" across alpha users.
- Consider extending the same auto-claim hook to `aida db merge-gate` (which can also create new agreed IDs as it promotes node-aware → short).

## Related

- Composes with: BUG-115 (aggregate-low warning logic — same family; auto-claim makes warnings rare, BUG-115 makes them precise when they do fire)
- See also: `docs/positioning/vs-karpathy-md.md` (first-users alpha framing — block concept should be invisible)
