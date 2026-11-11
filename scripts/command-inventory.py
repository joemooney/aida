#!/usr/bin/env python3
"""Reproduce SPIKE-714's lexical baseline; this is not a call-graph classifier.
trace:SPIKE-714 | ai:codex
"""
import collections
import json
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
base = ROOT / 'aida-cli-lib/src'
registry = (base / 'command_registry.rs').read_text().split('static REGISTRY:', 1)[1].split('];', 1)[0]
entries = re.findall(r'&crate::(\w+)::(\w+)', registry)
writers = tomllib.loads((ROOT / 'scripts/store-writers.toml').read_text())['writer']
lines = ['# Generated lexical evidence only; not migration permission.', '# trace:SPIKE-714 | ai:codex', 'version = 1', 'complete = false', 'source_commit = ' + json.dumps(subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()), f'registry_count = {len(entries)}']
counts = collections.Counter()
for module, symbol in entries:
    path = base / (module + '.rs')
    if not path.exists():
        path = base / module / 'mod.rs'
    source = path.read_text()
    match = re.search(r'\bstatic\s+' + symbol + r'\s*:[^=]+=\s*[^\{]+\{(.*?)\n\s*\};', source, re.S)
    if not match:
        candidates = []
        for candidate in (base / module).rglob('*.rs'):
            text = candidate.read_text()
            found = re.search(r'\bstatic\s+' + symbol + r'\s*:[^=]+=\s*[^\{]+\{(.*?)\n\s*\};', text, re.S)
            if found:
                candidates.append((candidate, text, found))
        if len(candidates) != 1:
            raise ValueError(f'unresolved registration {module}::{symbol}')
        path, source, match = candidates[0]
    block = match.group(1)
    name = re.search(r'\bname:\s*"([^"]+)"', block).group(1)
    handler = re.search(r'Handler::(\w+)\(([^)]+)\)', block)
    refs = sorted(set(re.findall(r'\bcrate::([A-Za-z_]\w*)', source)) - {module})
    globs = [f'{path.relative_to(ROOT)}:{i}' for i, line in enumerate(source.splitlines(), 1) if re.search(r'\buse\s+crate::.*\*', line)]
    local = [w['id'] for w in writers if w['path'] == str(path.relative_to(ROOT))]
    # Same-file candidates include unrelated handlers and tests: never infer reachability.
    row = dict(name=name, registration=f'{module}::{symbol}', source=f'{path.relative_to(ROOT)}:{source[:match.start()].count(chr(10))+1}', handler=handler.group(2) if handler else 'unresolved', dispatch=handler.group(1) if handler else 'unresolved', classification='unresolved', classification_evidence='Requires production call-path review; policy and same-file sinks are not proof.', lexical_crate_roots=refs, glob_sites=globs, same_file_writer_candidates=local, helpers_to_widen='unresolved: explicit re-exports, traits and globs require item-level review', move_stage='unresolved', home_option='advisor decision pending')
    counts['with_globs' if globs else 'without_globs'] += 1
    lines.append('\n[[command]]')
    for key, value in row.items():
        lines.append(key + ' = ' + json.dumps(value))
(ROOT / 'scripts/command-inventory.toml').write_text('\n'.join(lines) + '\n')
print(json.dumps({'registry_entries': len(entries), **counts}))
