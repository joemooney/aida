# TASK-3425 round 2: cmdkit boundary evidence

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
violates this task's no-import-churn acceptance. The remaining SCC criterion
is punted to the advisor, rather than claimed complete.

Measured on 2026-11-07 with the script below:

| Tree | Modules | Largest SCC | cmdkit in largest SCC |
| --- | ---: | ---: | --- |
| base `2a7fe186bdcd` | 581 | 326 | absent |
| reviewed head `c6118acbc882` | 582 | 327 | yes |
| round-2 working tree | 582 | 326 | no |

Only `cmdkit` leaves the reviewed head's SCC; no base SCC module leaves.
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

## Host publication handoff

Git metadata is read-only in the native implementer seat. No commit or push is
performed here. Once the advisor resolves the punt/publication veto, the host
must publish on the existing PR #3635 branch and include this exact standalone
commit trailer:

```text
Help-Golden: unchanged
```

Suggested commit message (Refs while SCC acceptance remains unresolved):

```text
[AI:codex] refactor(cmdkit): detach shared vocabulary and narrow clap exemption (Refs TASK-3425)

Help-Golden: unchanged
```

Run `make check-ci-fast` after that host commit. Before it, the committed-history
help co-change guard necessarily still fails; changing the golden merely to
satisfy the guard would misrepresent this pure move. The existing merge hold
also remains in force.

## Validation

- All 39 fast gates excluding `Check aida-cli without default features`,
  `TUI seam guard`, `Check formatting`, and `Help golden snapshot co-change guard`
  passed using the existing runner's loaded workflow bodies and `run_plan`.
  The first two excluded gates compile with Cargo, which must use the launcher.
- `rustfmt --edition 2021 --check` on all four changed Rust files passed.
- `bash tests/test_check_help_golden_cochange.sh` passed. The actual branch-history
  co-change check fails until the host adds the required commit trailer.
- `git diff --check` passed.
- Launcher job `j20261108T001448544-2-0`, running
  `cargo test --profile agent -p aida-cli-lib --lib story_1728_command_registry_tests`,
  timed out after 900 seconds during compilation (exit 124), without executing
  tests. **untested=true** for the Rust suite, including the golden snapshot.
  The final test-helper refinement was made during that build; formatting and
  trace checks were rerun, but no current-source Rust test pass is claimed.
- A full `make check-ci-fast` pass is not claimed: its direct compilation was
  interrupted for the native lane rule, and Git history cannot be edited here.

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
c=measure('WORKTREE')
print('leaving SCC from reviewed head:', ', '.join(sorted(b-c)))
print('leaving SCC from base:', ', '.join(sorted(a-c)))
```
