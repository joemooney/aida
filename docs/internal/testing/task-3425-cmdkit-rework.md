# TASK-3425: cmdkit boundary evidence

<!-- trace:TASK-3425 | ai:codex -->

`OutputFormat` and `StakeholderAction` move unchanged into `cmdkit`; their
previous modules re-export them. No consumer imports change. `cmdkit` now
reaches only the production leaf `spec_arg` through named `crate::` module
paths. The root `crate::Storage` type remains the existing aida-core import.
The ratchet exempts only `cmdkit::GlobalArgs` (a struct), rather than all of
`cmdkit.rs`. `OutputFormat` is a ValueEnum, outside that ratchet's existing
Parser/Args/Subcommand scope.

## Finding 2 remains a scope decision

This is leaf preparation, **not evidence of the plan's promised 29-module
reduction**. The path-based module graph still counts consumer imports of
framework types through `command_registry` as reaching the registry table.
Getting the plan's hypothetical "drop edges into command_registry" result
requires resolving re-exported symbols or changing consumer imports; the latter
violates this task's no-import-churn acceptance. The SCC acceptance remains
unmet; this PR must use `Refs TASK-3425` and leave that criterion to follow-up
work. No completion is claimed.

Measured on 2026-11-07 (MST) with the script below:

| Tree | Modules | Largest SCC | cmdkit in largest SCC |
| --- | ---: | ---: | --- |
| base `2a7fe186bdcd` | 581 | 326 | absent |
| reviewed head `c6118acbc882` | 582 | 327 | yes |
| published round-2 `c15b93650737` | 582 | 326 | no |
| current main `dc41ee8da2f4` | 581 | 325 | absent |
| round-3 working tree (HEAD `67139bfb2d2`) | 582 | 325 | no |

At the published round-2 commit, only `cmdkit` leaves the reviewed head's
SCC; no original-base SCC module leaves. The subsequent main merge also removes
`drain_next_n` from the lexical SCC; current main and this branch both measure
325, with no main SCC member removed by the extraction.
This is a lexical graph over top-level module owners, combining nested files,
excluding test directories, test-prefixed files, the root dispatch module and
five test-helper owners/targets. Inline test edges other than those helper
edges are retained. It is a reproducible comparison, not a Rust name resolver,
and its absolute counts must not be mixed with differently filtered graphs
(the plan's historical 323/294 or the review's 343/344).

Direct boundary check:

```sh
rg -n 'crate::(cli|stakeholder_role|spec_arg)' aida-cli-lib/src/cmdkit.rs
# Only crate::spec_arg::ensure_alias_map() remains.
```

## Published state and round-3 disposition

Round 2 was published as `c15b93650737` on PR #3635. That commit omitted
`Help-Golden: unchanged` and used a completing `(TASK-3425)` subject despite
the unmet SCC acceptance. These are publication defects, not successful gates.
The current seat starts at `67139bfb2d2`, which merges main into that branch.

The round-3 documentation edits remove the per-change AGENTS.md paragraph and
retain only the durable cmdkit architecture sentence in OVERVIEW.md. No Rust
implementation or golden snapshot changes are needed for this round.

The host (advisor seat) added the commit that carries the standalone
`Help-Golden: unchanged` body line, because the native declaration API accepts
only a subject line. That is the commit whose subject ends
`docs(cmdkit): correct review evidence and retain open SCC acceptance (Refs TASK-3425)`;
`scripts/check-help-golden-cochange.sh origin/main` exits 0 on it. The help
surface is in fact unchanged: the help snapshot test passed in the CI lib shards
at `67139bfb2d2`.

The PR title was changed to `Refs TASK-3425` in round 3, and the native
publication declaration records the subject above. The eventual squash message
must retain `Refs TASK-3425`; the earlier completing commit subject must not
grant completion of this slice.

The SCC follow-up could not be filed: `aida add --status draft` failed during
ID allocation with read-only filesystem error and reported that no metadata or
object was written. The proposed follow-up is recorded on TASK-3425 for advisor
filing: approve a measurement/scope that either resolves re-exported symbols
without caller import churn or explicitly permits the required caller changes,
then demonstrate the agreed SCC reduction. No new scope is implemented here.
A round-3 `blocked-dependency` punt now leaves TASK-3425 in Needs Attention
for finding 1 and the remaining full-green-CI portion of finding 5. This vetoes
automatic publication until the host/advisor can preserve the commit body and
rerun the required gate. The findings are not disputed on their merits.
The existing merge hold remains in force.

## Validation

Round-2 launcher job `j20261108T001448544-2-0` timed out after 900 seconds
while compiling, without executing tests. Its result was **untested=true**.
The earlier partial fast-gate run was not a full `make check-ci-fast` pass.
Round-3 validation:

- All 39 fast gates other than the two Cargo compilation gates, Cargo formatting
  and the committed-history help guard passed using the existing runner's
  workflow bodies and `run_plan` (batches of 14, 12 and 13). This is a partial
  fast-gate run, not a full `make check-ci-fast` pass. Native lane compilation
  must go through `aida lane job`.
- `rustfmt --edition 2021 --check` on the five changed Rust source files passed.
- `bash tests/test_check_help_golden_cochange.sh` passed; the actual
  `bash scripts/check-help-golden-cochange.sh origin/main` still exits 1
  because committed history lacked the trailer. After the host commit it exits 0;
  CI (gates) at the new head is the full fast-gate run.
- `git diff --check` passed. The measurement script reproduced the table above.
- At published head `67139bfb2d2`, all six test shards passed in
  [CI run 42438168743](https://github.com/joemooney/aida/actions/runs/42438168743).
  [lib-0](https://github.com/joemooney/aida/actions/runs/42438168743/job/128601425348)
  passed 5,449 tests with zero failures;
  [lib-1](https://github.com/joemooney/aida/actions/runs/42438168743/job/128601425261)
  passed 7,485 with zero failures, including
  `command_registry::story_1728_command_registry_tests::help_surface_matches_golden_snapshot`
  (11.358s). These full library shards cover the command-registry ratchets.
  No Rust source changed in round 3.
- [CI (gates)](https://github.com/joemooney/aida/actions/runs/42438168743/job/128601425294)
  failed specifically at the help co-change guard. The `lane_fixture_fails`
  panic in its log is an expected negative fixture; its parent test passed,
  and that group finished with 88 passed / zero failed. The merge hold also
  remains red. No green aggregate CI or full fast-gate pass is claimed; the
  new host commit must receive fresh CI.
- Local ratchet job `j20261108T022140013-3-0` timed out after 900 seconds
  during compilation (exit 124), without executing tests, and reported a stale
  target overlay because the primary rewrote a seed-generation file. The
  longer-timeout golden job `j20261108T022709861-2-0` is compiling and ratchet
  retry `j20261108T023723340-2-0` is queued at handoff. No local test pass is
  claimed; the executed Rust evidence above comes from CI. These duplicate
  local runs are not needed to establish the already-passing CI test results.

## Reproduce the measurement

Run this Python from the repository root (uses only stdlib and read-only Git):

```python
import re, subprocess, sys, tarfile, io
from pathlib import Path
sys.setrecursionlimit(10000)
root = Path('aida-cli-lib/src')
helpers = {'test_env', 'test_git', 'test_home', 'test_exec', 'test_timing'}
def measure(ref):
    data = subprocess.check_output(['git', 'archive', 'HEAD' if ref == 'WORKTREE' else ref, str(root)])
    archive = tarfile.open(fileobj=io.BytesIO(data))
    paths = [m.name for m in archive.getmembers() if m.isfile()]
    graph = {}
    for path in paths:
        p = Path(path); rel = p.relative_to(root)
        if p.suffix != '.rs' or 'tests' in rel.parts or p.stem.startswith('test_'): continue
        owner = rel.parts[0].removesuffix('.rs')
        if owner == 'lib' or owner in helpers: continue
        text = p.read_text() if ref == 'WORKTREE' else archive.extractfile(path).read().decode()
        deps = set(re.findall(r'\bcrate::([A-Za-z_][A-Za-z_0-9]*)',text))
        for group in re.findall(r'\buse\s+crate::\{(.*?)\};',text,re.S):
            deps.update(re.findall(r'(?:^|,)\s*([A-Za-z_][A-Za-z_0-9]*)',group))
        graph.setdefault(owner,set()).update(deps - helpers)
    graph = {k: v & graph.keys() for k,v in graph.items()}
    index = {}; low = {}; stack=[]; active=set(); components=[]
    def visit(v):
        index[v]=low[v]=len(index); stack.append(v); active.add(v)
        for w in sorted(graph[v]):
            if w not in index: visit(w); low[v]=min(low[v],low[w])
            elif w in active: low[v]=min(low[v],index[w])
        if low[v]==index[v]:
            c=[]
            while True:
                w=stack.pop(); active.remove(w); c.append(w)
                if w==v:break
            components.append(c)
    for v in sorted(graph):
        if v not in index:visit(v)
    largest=max(components,key=len)
    print(ref, 'modules',len(graph),'largest SCC',len(largest),'cmdkit in largest', 'cmdkit' in largest)
    if 'cmdkit' in graph: print('cmdkit edges:', ', '.join(sorted(graph['cmdkit'])))
    return set(largest)
# WORKTREE uses the tracked HEAD inventory and current working file bodies.
a=measure('2a7fe186bdcd718fa356e5e4d028a3b2ff13769d')
b=measure('c6118acbc882fb4fe2598264ee80b5b987585024')
measure('c15b93650737')
current_main=measure('dc41ee8da2f44f88178936fc1a8e6cb7168a3de7')
c=measure('WORKTREE')
print('leaving current main SCC:', ', '.join(sorted(current_main-c)))
print('leaving SCC from reviewed head:', ', '.join(sorted(b-c)))
print('leaving SCC from base:', ', '.join(sorted(a-c)))
```

## TASK-3448 repeated review finding: publication trailer handoff (2026-11-12)

<!-- trace:TASK-3448 | ai:codex -->

The new native-seat publication declaration supplies the standalone
`Help-Golden: unchanged` body line again for finding 1. Preserve it verbatim in
the host-created commit and eventual squash message. Keep Refs TASK-3448 and
partial Refs TASK-3428 scope; neither spec completes with this correction.
No CLI source or golden snapshot changes are necessary for this metadata fix.

At local HEAD `44689c0547`, commit `280f913707` already contains the exact
standalone trailer. `bash scripts/check-help-golden-cochange.sh origin/main`
passes, and `bash tests/test_check_help_golden_cochange.sh` passes. These local
results do not establish fresh published-head CI success or explain the earlier
reported exact-head failure. The host must publish this new commit with the
body preserved and obtain fresh passing co-change and required checks.

`make check-ci-fast` reached the TUI seam guard, which invokes Cargo directly;
that run did not complete within the tool lifetime. Full fast-gate validation
remains incomplete under this seat's lane-runner-only Cargo policy. No Rust
source changed and no new Rust test result is claimed (`untested=true`).
The advisor's largest SCC remains 367; helper extraction and measurement with
inline-test edges retained remain the next slice after #3867 lands.

## TASK-3448 injected test support, top-level caller slice (2026-11-12)

<!-- trace:TASK-3448 | ai:codex -->

The 13:00 UTC advisor brief supersedes the earlier proposal for a helper
module that still imports `command_registry`. `cmdkit::testing::CliTestSupport<T>`
now accepts a command-tree builder and matches decoder by injection. It has no
`crate::` dependencies, including no dependency on the CLI type or registry.
`command_registry::test_support()` supplies the existing pristine-tree clone
and decoder; the crate root binds the test-only `TEST_CLI` instance. There is
no initializer, mutable global registration, or production-path change.

This directory slice repoints 192 parse calls and 15 tree-builder calls across
27 top-level caller files. The three support files are `cmdkit.rs`,
`command_registry.rs`, and `lib.rs`. Calls in nested directories (including
`src/tests/`, `gate/`, and `regen/`) remain for a separate mechanical PR so this
slice stays below the advisor's approximately 40-file split threshold.
The production parser and tree-builder entry points remain in the registry.

Using the exact measurement function above, with only the invocation footer
changed to compare `measure('HEAD')` and `measure('WORKTREE')`:

| Tree | Modules | Largest SCC | cmdkit in largest SCC |
| --- | ---: | ---: | --- |
| base `bee6758fabe4048cfccd62d95bd57df896b74bd9` | 610 | 367 | no |
| top-level caller working tree | 610 | 367 | no |

No base SCC member leaves. Inline-test edges and the original measurement
filters are retained without additional exemptions. `cmdkit` still has only
its existing `spec_arg` edge. The migrated callers no longer have direct
registry helper edges, but other paths remain; for example,
`flow_cmd -> cli -> agent_launch -> command_registry`. `cli` re-exports agent
argument types, and `agent_launch` builds the production tree. Production
registry consumers also remain in `store_mutation_entry`, `leases`,
`observation_envelope`, `help_catalog`, `external_cmd`, `user_alias`, and
`docs_sweep`. This is a partial test-infrastructure slice, **not an SCC
reduction or completion of TASK-3448**; publication must retain `Refs TASK-3448`.

### Local validation for this slice

- `cargo check --profile agent -p aida-cli-lib --tests` passed via launcher job
  `j20261112T131100619-2-0` (3m 05s).
- Changed-file `rustfmt --edition 2021 --config skip_children=true --check`
  and `git diff --check` passed. Reversing the helper-reference replacements
  in all 27 caller files reproduces the original source after formatting.
- Registry/ratchet/help job `j20261112T131141212-2-0` timed out after 900 seconds
  during compilation (exit 124), with no executed tests. The runner reported
  `overlay-stale`: the primary rewrote a file of seed generation
  `agent-fb3abc32bc5001f9` in place. **untested=true**.
- At 13:29 UTC, parser job `j20261112T131208863-935863-0`
  (`cargo test --profile agent -p aida-cli-lib --lib -- cli:: flow_cmd::`)
  is running; workspace compile-only job `j20261112T131209893-935902-0`
  (`cargo test --profile agent --workspace --no-run`) is queued. The latter
  is the exact Cargo payload of `make check-ci-compile`, not an executed-test
  result. Neither job is counted as a pass at handoff.
- Non-Cargo fast-tier batches are partial. No full `make check-ci-fast` or
  `make check-ci-compile` pass is claimed. Some source guards attempt Git
  metadata writes refused by this native seat; the Cargo gates must use the
  launcher. Full gate evidence remains for the host/CI.

The host publication subject must retain `Refs TASK-3448`, and its commit body
and eventual squash message must retain `Help-Golden: unchanged` verbatim.

## TASK-3448 remaining callers and production injection (slice C, 2026-11-12)

<!-- trace:TASK-3448 | ai:codex -->

The 15:52 UTC advisor brief supersedes the earlier validation gates. This slice
migrates the remaining 31 caller files, including production callers and the
registry-owned tests that imported the helpers through `use super::*`. Together
with the three support files and two documentation files, it stays below the
approximately 40-file split threshold.

The existing generic injected helper becomes production `cmdkit::CliSupport<T>`.
The crate root binds `CLI_SUPPORT` directly to the registry's pristine-tree
provider and matches decoder; `TEST_CLI` is a test-only alias for that same
instance. The registry's test-support factory and duplicate `try_parse_from`
implementation are removed. Its exiting parser and test command projection use
the injected parser too. No initialization hook, mutable callback registration,
new command definition, or help snapshot change is introduced. Tree clones,
match decoding, clap error formatting and the pristine-tree cache are preserved.

There are zero consumer calls to `command_registry::try_parse_from` or
`command_registry::built_command`. The sole remaining `built_command` reference
outside its definition is the callback injected at the crate root (plus the
provider's documentation). The leaf helper itself has no registry or concrete
CLI-type dependency. `rg` confirms no unqualified calls remain in registry-owned
tests. Codegraph's reviewed absolute executable is missing on this host, so no
Codegraph result is claimed.

Using the exact measurement function above with only its invocation footer
changed to `measure('HEAD')` and `measure('WORKTREE')`:

| Tree | Modules | Largest SCC | cmdkit in largest SCC |
| --- | ---: | ---: | --- |
| base `5a76ab9cff3` (includes main `d87c460e6f4`, #3881) | 622 | 367 | no |
| slice C working tree | 622 | 350 | no |

Inline-test edges and all existing filters are unchanged. The crate root is
excluded by that existing measurement, as before; no new owner or edge exemption
is added. The 17 modules leaving the largest SCC are `adoption_cmd`, `assign_cmd`,
`command_registry`, `decide_cmd`, `derisk_cmd`, `driver_cmd`, `flow_cmd`,
`health_vitals_cmd`, `import_export_cmd`, `import_plan_cmd`, `lifecycle_cmd`,
`mcp_serve`, `monitor_contract`, `rebase_cmd`, `reconstitute`, `tui_entry`, and
`ultraplan_cmd`. `cmdkit` retains only its existing `spec_arg` edge.

This meets the spec's smaller-reduction allowance rather than the historical
approximately 29-module target. A concrete remaining cycle is
`cli -> agent_launch -> cli`: CLI argument re-exports reach `agent_launch`, whose
`SessionArgs` and `AgentArgs` still wrap CLI argument types. Removing registry
helper edges cannot dissolve that cycle; a later slice can target the argument
ownership/re-export boundary. Production registry handler calls also remain in
`store_mutation_entry`; registry-specific test inspection remains intentional.

### Slice C validation and publication

- Required `cargo check --profile agent -p aida-cli-lib --tests` passed via
  launcher job `j20261112T162411769-2195721-0` (exit 0, 3m 24s).
- The initial check `j20261112T162201254-2190189-0` failed on two registry-owned
  wildcard-import test callers; both were migrated before the passing rerun.
- Changed-file `rustfmt --edition 2021 --config skip_children=true --check` and
  `git diff --check` passed. Thirty caller files reproduce exactly from the
  original source with mechanical helper substitutions followed by rustfmt;
  the registry ratchet/golden test file was migrated separately as well.
- The SCC measurement reproduced the table above twice. Repository-wide `rg`
  finds no code consumer of the registry parser/tree helpers beyond the root
  callback injection; an existing TUI documentation reference is informational.
- No local Rust tests executed (`untested=true`). Under the binding advisor
  brief, CI supplies the test/fast-tier gate after host publication.

The native seat leaves edits uncommitted and declares host publication with a
completing TASK-3448 subject because the SCC shrinks under the spec's explicit
smaller-reduction allowance. Preserve the standalone `Help-Golden: unchanged`
body trailer in the host commit and eventual squash message.
