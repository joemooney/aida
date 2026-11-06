#!/usr/bin/env python3
"""Exact-site ambient-environment migration ledger (STORY-2538).

Token-aware Rust import/alias scan, plus AST-based Python harness scan. No
parser dependency. Conservative: all mutations are inventoried, not only tests.
A reference to an env function is itself a site, so passing it through helpers
cannot hide it. See docs/internal/testing/exec-context-migration.md for limits.
"""
# trace:STORY-2538 | ai:codex
from __future__ import annotations

import argparse
import ast
import fnmatch
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
LEDGER = Path("scripts/exec-context-legacy.json")
SCOPE = "All Rust workspace members and tests/**/*.rs, tests/**/*.py; reads, all mutations and import bindings. Migration owner: R4 slices B-D."
OPS = {"var", "var_os", "vars", "vars_os", "set_var", "remove_var"}
RAW = re.compile(r'(?:br|cr|r)(#*)"')
QUOTED = re.compile(r'(?:b|c)?"')
CHAR = re.compile(r"(?:b)?'(?:\\(?:u\{[0-9a-fA-F]+\}|.)|[^'\\\n])'")
IDENT = re.compile(r"(?:r#)?[A-Za-z_][A-Za-z_0-9]*")
# Strict, reserved and weak Rust keywords are expression/item boundaries, not
# identifiers extending a preceding path. Include historical/future editions.
RUST_KEYWORDS = set("""
    Self abstract as async auto await become box break const continue crate do
    dyn else enum extern false final fn for gen if impl in let loop macro macro_rules match
    mod move mut override priv pub raw ref return safe self static struct super
    trait true try type typeof union unsafe unsized use virtual where while yield
""".split())


def tokens(source):
    """Keep literals whole, discard comments (including nested block comments)."""
    out, i = [], 0
    while i < len(source):
        if source[i].isspace():
            i += 1
            continue
        if source.startswith("//", i):
            end = source.find("\n", i)
            i = len(source) if end < 0 else end
            continue
        if source.startswith("/*", i):
            depth, i = 1, i + 2
            while i < len(source) and depth:
                if source.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif source.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            if depth:
                raise ValueError("unterminated block comment")
            continue
        start = i
        raw = RAW.match(source, i)
        quoted = QUOTED.match(source, i)
        char = CHAR.match(source, i)
        if raw:
            close = '"' + raw[1]
            end = source.find(close, raw.end())
            if end < 0:
                raise ValueError("unterminated raw literal")
            i = end + len(close)
        elif quoted:
            i = quoted.end()
            while i < len(source) and source[i] != '"':
                i += 2 if source[i] == "\\" else 1
            i += 1
        elif char:
            i = char.end()
        elif match := IDENT.match(source, i):
            i = match.end()
        elif source.startswith("::", i):
            i += 2
        else:
            i += 1
        token = source[start:i]
        out.append(token.removeprefix("r#") if IDENT.fullmatch(token) else token)
    return out


def use_paths(items, prefix=()):
    """Expand grouped use trees, including aliases and glob imports."""
    result, current, depth = [], [], 0
    for item in items + [","]:
        if item == "," and depth == 0:
            if "{" in current:
                at = current.index("{")
                head = tuple(t for t in current[:at] if t != "::")
                result.extend(use_paths(current[at + 1:-1], prefix + head))
            elif current:
                if "as" in current:
                    at = current.index("as")
                    path = prefix + tuple(t for t in current[:at] if t != "::")
                    if path[-1] == "self":
                        path = path[:-1]
                    result.append((current[at + 1], path))
                else:
                    path = prefix + tuple(t for t in current if t != "::")
                    if path[-1] == "self":
                        path = path[:-1]
                    result.append((path[-1], path))
            current = []
        else:
            current.append(item)
            depth += (item == "{") - (item == "}")
    return result


def rust_sites(source):
    ts = tokens(source)
    aliases = {"std": ("std",), "env": ("std", "env")}
    imports, excluded = [], set()
    for i, t in enumerate(ts):
        # trace:STORY-2538 | ai:codex
        # Crate renames feed the same resolution as `use std as alias`.
        if (ts[i:i + 4] == ["extern", "crate", "std", "as"]
                and i + 5 < len(ts) and ts[i + 5] == ";"
                and IDENT.fullmatch(ts[i + 4])):
            imports.append((ts[i + 4], ("std",)))
            excluded.update(range(i, i + 6))
        if t == "use":
            end = i + 1
            while end < len(ts) and ts[end] != ";":
                end += 1
            imports.extend(use_paths(ts[i + 1:end]))
            excluded.update(range(i, end + 1))

    def resolve(path):
        for _ in range(20):
            replacement = aliases.get(path[0]) if path else None
            expanded = replacement + path[1:] if replacement else path
            if expanded == path:
                break
            path = expanded
        return path

    for _ in range(20):
        changed = False
        for alias, path in imports:
            path = resolve(path)
            if path == ("std", "env", "*"):
                for op in OPS:
                    aliases[op] = ("std", "env", op)
            elif alias != "std" and path and path[0] == "std" and aliases.get(alias) != path:
                aliases[alias] = path
                changed = True
        if not changed:
            break

    found = []
    # Delimiter depth bounds each function body; returning from a nested fn
    # restores its enclosing fn, and leaving the outer fn restores <module>.
    delimiters, functions = [], []
    pending_fn = None
    for alias, path in imports:
        canonical = resolve(path)
        if canonical[:2] == ("std", "env"):
            digest = hashlib.sha256((alias + "=" + "::".join(canonical)).encode()).hexdigest()[:20]
            found.append(f"<import>:binding:{digest}")
    for i, t in enumerate(ts):
        if t == "fn" and i + 1 < len(ts) and IDENT.fullmatch(ts[i + 1]):
            pending_fn = (ts[i + 1], len(delimiters))
        if t in {"(", "[", "{"}:
            if t == "{" and pending_fn and pending_fn[1] == len(delimiters):
                functions.append((pending_fn[0], len(delimiters) + 1))
                pending_fn = None
            delimiters.append(t)
        elif t in {")", "]", "}"}:
            if functions and t == "}" and functions[-1][1] == len(delimiters):
                functions.pop()
            if delimiters:
                delimiters.pop()
        elif t == ";" and pending_fn and pending_fn[1] == len(delimiters):
            pending_fn = None  # Trait/extern declaration without a body.
        scope = functions[-1][0] if functions else "<module>"
        if i in excluded or not IDENT.fullmatch(t) or (i > 1 and ts[i - 1] == "::" and IDENT.fullmatch(ts[i - 2]) and ts[i - 2] not in RUST_KEYWORDS):
            continue
        path, end = [t], i + 1
        while end + 1 < len(ts) and ts[end] == "::" and IDENT.fullmatch(ts[end + 1]):
            path.append(ts[end + 1])
            end += 2
        canonical = resolve(tuple(path))
        if len(canonical) != 3 or canonical[:2] != ("std", "env") or canonical[2] not in OPS:
            continue
        # Include the call's exact arguments, or just the reference when an env
        # function is passed to a helper/assigned to an alias. References count
        # even when the eventual alias call is not statically resolvable.
        stop = end
        if end < len(ts) and ts[end] == "(":
            depth, stop = 1, end + 1
            while stop < len(ts) and depth:
                depth += (ts[stop] == "(") - (ts[stop] == ")")
                stop += 1
        expression = " ".join(ts[i:stop])
        digest = hashlib.sha256(expression.encode()).hexdigest()[:20]
        found.append(f"{scope}:{canonical[2]}:{digest}")
    return found


def python_expression(node):
    """Canonical AST encoding, independent of ast.dump's versioned defaults.

    Ignore locations and absent optional fields (including Constant.kind,
    removed in Python 3.14). Keep node types, field names and ordered values
    so argument/key/operator changes still produce different fingerprints.
    """
    def normalize(value):
        if isinstance(value, ast.AST):
            return [type(value).__name__, {
                field: normalize(child) for field, child in ast.iter_fields(value)
                if child is not None and child != []
            }]
        if isinstance(value, list):
            return [normalize(child) for child in value]
        # repr also handles bytes, complex numbers and Ellipsis constants,
        # which are valid AST values but are not JSON scalar types.
        return [type(value).__name__, repr(value)]

    return json.dumps(normalize(node), sort_keys=True, separators=(",", ":"))


def python_sites(source):
    tree = ast.parse(source)
    aliases = {"os": "os"}
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name == "os":
                    aliases[alias.asname or alias.name] = "os"
        if isinstance(node, ast.ImportFrom) and node.module == "os":
            for alias in node.names:
                if alias.name == "*":
                    for key in ("environ", "environb", "getenv", "getenvb", "putenv", "unsetenv"):
                        aliases[key] = "os." + key
                else:
                    aliases[alias.asname or alias.name] = "os." + alias.name

    def name(node):
        if isinstance(node, ast.Name):
            return aliases.get(node.id, node.id)
        if isinstance(node, ast.Attribute):
            return name(node.value) + "." + node.attr
        return ""

    parents = {child: parent for parent in ast.walk(tree) for child in ast.iter_child_nodes(parent)}
    found = []
    for node in ast.walk(tree):
        if isinstance(node, (ast.Name, ast.Attribute)) and name(node) in {
            "os.environ", "os.getenv", "os.getenvb", "os.environb", "os.putenv", "os.unsetenv"
        }:
            # Inventory references as well as calls/writes: helper indirection
            # still has a tracked environment source at the binding boundary.
            expression_node = node
            while isinstance(parents.get(expression_node), (ast.Attribute, ast.Subscript, ast.Call)):
                expression_node = parents[expression_node]
            ancestor, scope = node, "<module>"
            while ancestor in parents:
                ancestor = parents[ancestor]
                if isinstance(ancestor, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    scope = ancestor.name
                    break
            expression = python_expression(expression_node)
            digest = hashlib.sha256(expression.encode()).hexdigest()[:20]
            found.append(f"{scope}:{name(node)}:{digest}")
    return found


def scan(root=ROOT):
    members = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["members"]
    # Scan the index's source inventory, reading working-tree contents. Scratch
    # files, ignored virtualenvs and build output cannot add local-only sites.
    tracked = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--", *members, "tests"],
        cwd=root, capture_output=True, check=True,
    ).stdout.decode("utf-8").split("\0")
    sites = Counter()
    for rel in sorted(set(tracked) - {""}):
        path = root / rel
        in_member = any(fnmatch.fnmatch(rel, member.rstrip("/") + "/*") for member in members)
        in_tests = rel.startswith("tests/")
        if not ((in_member or in_tests) and path.suffix == ".rs" or in_tests and path.suffix == ".py"):
            continue
        if not path.exists():
            continue
        try:
            source = path.read_text(encoding="utf-8")
        except UnicodeError as error:
            raise ValueError(f"tracked source is not UTF-8: {rel}") from error
        entries = python_sites(source) if path.suffix == ".py" else rust_sites(source)
        for entry in entries:
            sites[f"{rel}:{entry}"] += 1
    # One exact approved capture, not a module/function-wide exemption.
    capture_digest = hashlib.sha256(b"std :: env :: vars_os ( )").hexdigest()[:20]
    capture = f"aida-core/src/exec_context.rs:capture:vars_os:{capture_digest}"
    if sites[capture] != 1:
        raise ValueError("expected exactly one approved ambient capture")
    del sites[capture]
    return sites


def check(actual, ledger):
    expected = Counter(ledger["sites"])
    if ledger["version"] != 1 or ledger["owner"] != "STORY-2538" or sum(expected.values()) != ledger["ceiling"]:
        raise ValueError("invalid ledger header/ceiling")
    new, stale = actual - expected, expected - actual
    if new or stale:
        raise ValueError(f"new sites (forbidden): {dict(new)}\nretired/changed sites (prune ledger): {dict(stale)}")


def check_growth(ledger, previous):
    if not previous:
        return
    # Relocation may change only navigation (file/function), never the operation
    # or exact expression, and cannot borrow multiplicity from a retained site.
    current, old = Counter(ledger["sites"]), Counter(previous["sites"])

    def fingerprints(sites):
        counts = Counter()
        for key, count in sites.items():
            parts = key.rsplit(":", 2)
            if len(parts) != 3:
                raise ValueError(f"invalid legacy site key: {key}")
            counts[tuple(parts[-2:])] += count
        return counts

    additions = fingerprints(current - old)
    retirements = fingerprints(old - current)
    if additions - retirements or ledger["ceiling"] > previous["ceiling"]:
        raise ValueError("legacy ledger may only shrink or relocate identical operation/expression sites relative to the base revision")


def main():
    # trace:STORY-2538 | ai:codex
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", metavar="REF", help="refuse ledger growth relative to REF")
    parser.add_argument("--write-ledger", action="store_true", help="regenerate the exact-site ledger")
    args = parser.parse_args()
    actual = Counter(scan())
    if args.write_ledger:
        ledger = {"version": 1, "owner": "STORY-2538", "scope": SCOPE,
                  "ceiling": sum(actual.values()), "sites": dict(actual)}
    else:
        ledger = json.loads((ROOT / LEDGER).read_text())
    check(actual, ledger)
    # CI supplies its PR base; local runs may explicitly use --base REF too.
    # The initial introduction has no old ledger, so only that case bootstraps.
    if args.base:
        base = subprocess.run(["git", "rev-parse", "--verify", args.base + "^{commit}"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
        files = subprocess.run(["git", "ls-tree", "--name-only", base, str(LEDGER)], cwd=ROOT, capture_output=True, text=True, check=True).stdout.splitlines()
        if str(LEDGER) in files:
            previous = subprocess.run(["git", "show", f"{base}:{LEDGER}"], cwd=ROOT, capture_output=True, text=True, check=True).stdout
            check_growth(ledger, json.loads(previous))
    # Validate against the base before touching an existing ledger. The initial
    # introduction can bootstrap even when no working-tree ledger exists.
    if args.write_ledger:
        (ROOT / LEDGER).write_text(json.dumps(ledger, indent=2, sort_keys=True) + "\n")
    print(f"ExecContext legacy ledger: {ledger['ceiling']} exact sites; zero additions")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
