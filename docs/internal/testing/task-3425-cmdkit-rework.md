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

The host must add a new commit on the same branch with this exact message:

```text
[AI:codex] docs(cmdkit): correct review evidence and retain open SCC acceptance (Refs TASK-3425)

Help-Golden: unchanged
```

The native declaration API accepts only one subject line, so it cannot carry
the standalone help trailer. The host must preserve the body above explicitly.
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
  because committed history lacks the trailer. It must be rerun after the
  host commit, along with the full `make check-ci-fast` gate.
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
