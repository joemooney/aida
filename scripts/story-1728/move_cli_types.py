#!/usr/bin/env python3
"""Move each command's clap types out of `cli.rs` into its owning module
(STORY-1728), so changing a nested subcommand's flags touches only that
command's files.

    python3 scripts/story-1728/move_cli_types.py [--dry-run]

A type (enum/struct, with its docs, attributes and `impl` blocks) moves when
exactly one module's registered `*Args` struct reaches it, directly or
through other `cli.rs` types. Each owner gets a sibling `<module>_args.rs`
holding its types in their original order under `cli.rs`'s own imports; the
owner declares it with `#[path]` and re-exports it, and `cli.rs` keeps a
`pub(crate) use` line per moved type so every `crate::cli::X` path still
resolves. Shared types, `Cli`, `GlobalArgs` and the `Command` enum stay.
trace:STORY-1728 | ai:claude
"""
import argparse
import collections
import os
import re

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
SRC = os.path.join(ROOT, "aida-cli-lib", "src")
KEEP = {"Cli", "GlobalArgs", "Command", "OutputFormat"}


def item_end(lines, i):
    if lines[i].rstrip().endswith(";"):
        return i
    depth = 0
    seen = False
    for j in range(i, len(lines)):
        code = re.sub(r'"(?:[^"\\]|\\.)*"', '""', lines[j].split("//")[0])
        depth += code.count("{") - code.count("}")
        seen = seen or "{" in code
        if seen and depth <= 0:
            return j
    raise SystemExit("unbalanced item at line %d" % (i + 1))


def lead(lines, i):
    k = i
    while k > 0:
        s = lines[k - 1].strip()
        if s.startswith("///") or s.startswith("//") or s.startswith("#["):
            k -= 1
            continue
        # a multi-line attribute's tail (`))]`)
        if s.endswith("]") and not s.startswith("#["):
            j = k - 1
            while j > 0 and not lines[j].strip().startswith("#["):
                j -= 1
            if lines[j].strip().startswith("#["):
                k = j
                continue
        break
    return k


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    cli_path = os.path.join(SRC, "cli.rs")
    lines = open(cli_path).read().split("\n")
    test_at = next(
        (i for i, l in enumerate(lines) if l.strip() == "#[cfg(test)]" and lines[i + 1].startswith("mod tests")),
        len(lines),
    )
    items = {}  # name -> (start, end)
    impls = collections.defaultdict(list)
    i = 0
    while i < test_at:
        l = lines[i]
        m = re.match(r"^pub (?:enum|struct) (\w+)", l)
        im = re.match(r"^impl(?:<[^>]*>)?\s+(?:[\w:<>]+\s+for\s+)?(\w+)", l)
        if m or im:
            end = item_end(lines, i)
            start = lead(lines, i)
            if m:
                items[m.group(1)] = (start, end)
            else:
                impls[im.group(1)].append((start, end))
            i = end + 1
            continue
        i += 1
    text = {n: "\n".join(lines[s : e + 1]) for n, (s, e) in items.items()}
    names = set(items)
    code = {n: "\n".join(l for l in text[n].split("\n") if not l.strip().startswith("//")) for n in names}
    refs = {n: {m for m in names if m != n and re.search(r"\b" + m + r"\b", code[n])} for n in names}

    # owners: modules whose registered Args structs name the type
    owners = collections.defaultdict(set)
    for f in os.listdir(SRC):
        if not f.endswith(".rs") or f == "cli.rs" or f.endswith("_args.rs"):
            continue
        t = open(os.path.join(SRC, f)).read()
        for m in re.finditer(r"pub\(crate\) struct \w+Args \{(\}|\n.*?\n\})", t, re.S):
            for n in names:
                if re.search(r"crate::cli::" + n + r"\b", m.group(1)):
                    owners[n].add(f[:-3])
    reach = collections.defaultdict(set)
    for n, mods in owners.items():
        stack, seen = [n], set()
        while stack:
            x = stack.pop()
            if x in seen:
                continue
            seen.add(x)
            stack += list(refs[x])
            reach[x] |= mods
    moving = {n: next(iter(m)) for n, m in reach.items() if len(m) == 1 and n not in KEEP}
    # a type referenced by a staying type must stay too
    staying = names - set(moving)
    changed = True
    while changed:
        changed = False
        for n in list(moving):
            if any(n in refs[s] for s in staying):
                del moving[n]
                staying.add(n)
                changed = True

    by_mod = collections.defaultdict(list)
    for n, mod in moving.items():
        by_mod[mod].append(n)
    cli_free = set(re.findall(r"^pub fn (\w+)", "\n".join(lines[:test_at]), re.M))
    header = [l for l in lines[:test_at] if re.match(r"^use ", l)]

    drop = set()
    for mod, ns in sorted(by_mod.items()):
        spans = []
        for n in ns:
            spans.append(items[n])
            spans += impls.get(n, [])
        spans.sort()
        body = []
        for s, e in spans:
            body.append("\n".join(lines[s : e + 1]))
            drop.update(range(s, e + 1))
        content = "\n\n".join(body)
        imported = sorted(
            {m for m in (staying | set(moving)) - set(ns) if re.search(r"\b" + m + r"\b", content)}
            | {fn for fn in cli_free if re.search(r"\b" + fn + r"\b", content)}
        )
        out = [
            f"//! `aida {mod}` clap types, owned by `{mod}.rs` and re-exported from",
            "//! `cli.rs`. trace:STORY-1728 | ai:claude",
            "",
            *header,
        ]
        if imported:
            out.append("use crate::cli::{" + ", ".join(imported) + "};")
        out += ["", content, ""]
        path = os.path.join(SRC, f"{mod}_args.rs")
        mod_path = os.path.join(SRC, f"{mod}.rs")
        mt = open(mod_path).read()
        decl = f'#[path = "{mod}_args.rs"]\npub(crate) mod {mod}_args;\npub(crate) use {mod}_args::*;\n'
        if not args.dry_run:
            open(path, "w").write("\n".join(out))
            if decl not in mt:
                # after the module's leading docs / attributes / uses
                ml = mt.split("\n")
                k = 0
                while k < len(ml) and (ml[k].startswith("//") or ml[k].startswith("#!") or ml[k].strip() == ""):
                    k += 1
                while k < len(ml) and (ml[k].startswith("use ") or ml[k].startswith("pub use ") or ml[k].startswith("pub(crate) use ")):
                    while not ml[k].rstrip().endswith(";"):
                        k += 1
                    k += 1
                ml.insert(k, "\n// trace:STORY-1728 | ai:claude\n" + decl.rstrip("\n"))
                open(mod_path, "w").write("\n".join(ml))
        print(f"{mod}: {len(ns)} type(s), {sum(e - s + 1 for s, e in spans)} lines")

    kept = [l for i, l in enumerate(lines) if i not in drop]
    reexports = [
        "",
        "// Command clap types live with their commands (`<module>_args.rs`); these",
        "// re-exports keep `crate::cli::X` paths working. trace:STORY-1728 | ai:claude",
    ]
    for mod, ns in sorted(by_mod.items()):
        reexports.append(f"pub(crate) use crate::{mod}::{mod}_args::{{{', '.join(sorted(ns))}}};")
    at = max(i for i, l in enumerate(kept) if re.match(r"^use ", l)) + 1
    kept[at:at] = reexports
    new = re.sub(r"\n{3,}", "\n\n", "\n".join(kept))
    if not args.dry_run:
        open(cli_path, "w").write(new)
    print(f"moved {len(moving)} types into {len(by_mod)} modules; stayed: {sorted(staying)}")


if __name__ == "__main__":
    main()
