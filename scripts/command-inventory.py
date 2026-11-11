#!/usr/bin/env python3
"""Reproduce SPIKE-714's lexical baseline; this is not a call-graph classifier.
trace:SPIKE-714 | ai:codex
"""
import collections
import functools
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]

# Reuse the writer audit's offset-preserving Rust literal/comment masker.
# This only imports definitions; it does not run the writer audit.
_spec = importlib.util.spec_from_file_location('store_writer_mask', ROOT / 'scripts/check-store-writers.py')
_guard = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_guard)
mask_rust = functools.lru_cache(maxsize=128)(_guard.mask_rust)


def delimited_end(masked, start):
    """Return the exclusive end of a balanced Rust (), [] or {} expression.

    Angle brackets are intentionally excluded: Rust also uses them as operators.
    Input must have literals/comments masked so their delimiters stay opaque.
    """
    closing = {'(': ')', '[': ']', '{': '}'}
    if masked[start] not in closing:
        raise ValueError('expected opening delimiter')
    stack = []
    for at in range(start, len(masked)):
        token = masked[at]
        if token in closing:
            stack.append(closing[token])
        elif token in ')]}':
            if not stack or stack.pop() != token:
                raise ValueError('mismatched Rust delimiter')
            if not stack:
                return at + 1
    raise ValueError('unclosed Rust expression')


def static_block(source, symbol):
    """Locate a real static initializer, ignoring commented-out declarations."""
    masked = mask_rust(source)
    match = re.search(r'\bstatic\s+' + re.escape(symbol) + r'\s*:[^=]+=\s*[^\{]+\{', masked)
    if match is None:
        return None
    start = match.end() - 1
    end = delimited_end(masked, start)
    return match.start(), source[start + 1:end - 1]


def extract_handler(block):
    masked = _guard.mask_rust(block)
    match = re.search(r'\brun\s*:\s*(?:\w+::)*Handler::(\w+)\s*\(', masked)
    if match is None:
        raise ValueError('registration has no supported Handler constructor')
    start = match.end() - 1
    end = delimited_end(masked, start)
    return match.group(1), block[start + 1:end - 1].strip()


def main():
    generate(ROOT)


def generate(root):
    base = root / 'aida-cli-lib/src'
    registry = (base / 'command_registry.rs').read_text().split('static REGISTRY:', 1)[1].split('];', 1)[0]
    entries = re.findall(r'&crate::(\w+)::(\w+)', registry)
    writers = tomllib.loads((root / 'scripts/store-writers.toml').read_text())['writer']
    lines = ['# Generated lexical evidence only; not migration permission.', '# trace:SPIKE-714 | ai:codex', 'version = 1', 'complete = false', 'source_commit = ' + json.dumps(subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()), f'registry_count = {len(entries)}']
    counts = collections.Counter()
    for module, symbol in entries:
        path = base / (module + '.rs')
        if not path.exists():
            path = base / module / 'mod.rs'
        source = path.read_text()
        match = static_block(source, symbol)
        if not match:
            candidates = []
            for candidate in (base / module).rglob('*.rs'):
                text = candidate.read_text()
                found = static_block(text, symbol)
                if found:
                    candidates.append((candidate, text, found))
            if len(candidates) != 1:
                raise ValueError(f'unresolved registration {module}::{symbol}')
            path, source, match = candidates[0]
        offset, block = match
        name = re.search(r'\bname:\s*"([^"]+)"', block).group(1)
        dispatch, handler = extract_handler(block)
        refs = sorted(set(re.findall(r'\bcrate::([A-Za-z_]\w*)', source)) - {module})
        globs = [f'{path.relative_to(root)}:{i}' for i, line in enumerate(source.splitlines(), 1) if re.search(r'\buse\s+crate::.*\*', line)]
        local = [w['id'] for w in writers if w['path'] == str(path.relative_to(root))]
        # Same-file candidates include unrelated handlers and tests: never infer reachability.
        row = dict(name=name, registration=f'{module}::{symbol}', source=f'{path.relative_to(root)}:{source[:offset].count(chr(10))+1}', handler=handler, dispatch=dispatch, classification='unresolved', classification_evidence='Requires production call-path review; policy and same-file sinks are not proof.', lexical_crate_roots=refs, glob_sites=globs, same_file_writer_candidates=local, helpers_to_widen='unresolved: explicit re-exports, traits and globs require item-level review', move_stage='unresolved', home_option='advisor decision pending')
        counts['with_globs' if globs else 'without_globs'] += 1
        lines.append('\n[[command]]')
        for key, value in row.items():
            lines.append(key + ' = ' + json.dumps(value))
    (root / 'scripts/command-inventory.toml').write_text('\n'.join(lines) + '\n')
    print(json.dumps({'registry_entries': len(entries), **counts}))


if __name__ == "__main__":
    main()
